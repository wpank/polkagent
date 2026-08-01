use std::fmt;
use std::future::Future;
use std::time::Instant;

use tracing::{debug, warn};

use crate::classifier::{AlwaysRetry, ErrorClass, ErrorClassifier};
use crate::error::RetryExhausted;
use crate::policy::RetryPolicy;

/// Execute an async operation with the given retry policy and the default
/// classifier (all errors are retryable).
///
/// Returns `Ok(T)` on success or `Err(RetryExhausted<E>)` if all attempts fail.
pub async fn retry_with_policy<F, Fut, T, E>(
    policy: &RetryPolicy,
    mut f: F,
) -> Result<T, RetryExhausted<E>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: fmt::Debug + fmt::Display,
{
    retry_with_policy_and_classifier(policy, &AlwaysRetry, &mut f).await
}

/// Execute an async operation with the given retry policy and error classifier.
///
/// The classifier determines whether a given error should be retried,
/// treated as permanent, or used to trip a circuit breaker.
pub async fn retry_with_policy_and_classifier<F, Fut, T, E, C>(
    policy: &RetryPolicy,
    classifier: &C,
    f: &mut F,
) -> Result<T, RetryExhausted<E>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: fmt::Debug + fmt::Display,
    C: ErrorClassifier<E>,
{
    let start = Instant::now();
    let total_attempts = policy.max_retries + 1;
    let mut last_error: Option<E> = None;

    for attempt in 0..total_attempts {
        debug!(attempt, total_attempts, "executing retry attempt");

        let result = if let Some(timeout) = policy.per_attempt_timeout {
            match tokio::time::timeout(timeout, f()).await {
                Ok(inner) => inner,
                Err(_elapsed) => {
                    warn!(attempt, ?timeout, "attempt timed out");
                    // On timeout, we treat it as a retryable failure if we have retries left.
                    // Store a synthetic description. We need to continue the loop.
                    last_error = None;
                    if attempt < policy.max_retries {
                        let delay = policy.backoff.delay_with_random_jitter(attempt);
                        debug!(?delay, attempt, "backing off after timeout");
                        tokio::time::sleep(delay).await;
                    }
                    continue;
                }
            }
        } else {
            f().await
        };

        match result {
            Ok(value) => {
                if attempt > 0 {
                    debug!(attempt, "succeeded after retry");
                }
                return Ok(value);
            }
            Err(e) => {
                let class = classifier.classify(&e);
                debug!(attempt, ?class, error = %e, "attempt failed");

                match class {
                    ErrorClass::NonRetryable | ErrorClass::CircuitBreak => {
                        return Err(RetryExhausted {
                            last_error: e,
                            attempts: attempt + 1,
                            total_elapsed: start.elapsed(),
                        });
                    }
                    ErrorClass::Retryable => {
                        last_error = Some(e);
                        if attempt < policy.max_retries {
                            let delay = policy.backoff.delay_with_random_jitter(attempt);
                            debug!(?delay, attempt, "backing off before next attempt");
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
            }
        }
    }

    // If we reach here, all attempts failed. `last_error` should be `Some`
    // unless the final attempt was a timeout (in which case we create a synthetic error).
    // Because the final attempt timeout continues the loop and the loop ends,
    // `last_error` could be None if the last attempt timed out.
    // We handle this by returning the best error we have.
    match last_error {
        Some(e) => Err(RetryExhausted {
            last_error: e,
            attempts: total_attempts,
            total_elapsed: start.elapsed(),
        }),
        None => {
            // This path is only reachable if every single attempt timed out
            // and E cannot be constructed. We panic here because the types
            // guarantee at least one real error unless all attempts time out.
            // In practice, callers should handle timeout at a higher level.
            panic!("all attempts timed out with no error produced; use TimeoutWrapper for proper timeout handling");
        }
    }
}
