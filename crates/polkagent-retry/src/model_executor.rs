//! Retry-aware [`ModelExecutor`] wrapper.
//!
//! [`RetryModelExecutor`] implements [`ModelExecutor`] by delegating to an
//! inner executor and automatically retrying on transient, retryable failures
//! (rate limits, transient transport errors, timeouts). Authentication,
//! validation, context-window, and cancellation errors are **never** retried.
//!
//! An optional [`CircuitBreaker`] can be attached so that sustained failures
//! cause the wrapper to fail fast without hitting the downstream provider.

use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use tracing::{debug, warn};

use polkagent_executor_trait::{
    ExecutorError, InferenceRequest, InferenceResponse, ModelExecutor, StreamEvent,
};

use crate::circuit_breaker::CircuitBreaker;
use crate::policy::RetryPolicy;

// ---------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------

/// Classify an [`ExecutorError`] for retry decisions.
///
/// Returns `true` when the error is transient and the request should be
/// retried. The following errors are considered retryable:
///
/// - [`ExecutorError::RateLimit`]
/// - [`ExecutorError::Timeout`]
/// - [`ExecutorError::Transport`] **only** when `retryable` is `true`
///
/// Everything else (authentication, context-window, cancellation, invalid
/// response, internal, non-retryable transport) is permanent.
fn is_retryable(error: &ExecutorError) -> bool {
    error.is_retryable()
}

// ---------------------------------------------------------------------------
// RetryModelExecutor
// ---------------------------------------------------------------------------

/// A [`ModelExecutor`] decorator that retries transient failures.
///
/// # Retry policy
///
/// The wrapper uses a configurable [`RetryPolicy`] to govern the maximum
/// number of retry attempts and backoff strategy. Only errors classified as
/// retryable (rate limit, timeout, retryable transport) are retried.
///
/// # Circuit breaker
///
/// An optional [`CircuitBreaker`] can be provided. When present:
///
/// - Before each attempt the circuit breaker is checked. If the circuit is
///   open, the call fails immediately with an [`ExecutorError::Transport`]
///   carrying the circuit-open description.
/// - Successes are recorded to help close the circuit.
/// - Retryable failures are recorded to help open the circuit.
/// - Non-retryable failures are **not** recorded against the circuit breaker
///   (they reflect caller errors, not provider outages).
///
/// # Streaming
///
/// [`RetryModelExecutor::stream`] retries the initial connection. Once the
/// inner executor returns a stream successfully, the stream itself is passed
/// through without per-chunk retry (the caller is expected to handle
/// mid-stream errors at a higher level).
pub struct RetryModelExecutor {
    inner: Arc<dyn ModelExecutor>,
    policy: RetryPolicy,
    circuit_breaker: Option<Arc<CircuitBreaker>>,
}

impl RetryModelExecutor {
    /// Wrap an existing executor with retry logic.
    ///
    /// ```rust,no_run
    /// # use std::sync::Arc;
    /// # use polkagent_retry::RetryPolicy;
    /// # use polkagent_retry::model_executor::RetryModelExecutor;
    /// # use polkagent_executor_trait::ModelExecutor;
    /// # fn example(inner: Arc<dyn ModelExecutor>) {
    /// use std::time::Duration;
    ///
    /// let policy = RetryPolicy::exponential(3, Duration::from_millis(100));
    /// let retry_executor = RetryModelExecutor::new(inner, policy);
    /// # }
    /// ```
    pub fn new(inner: Arc<dyn ModelExecutor>, policy: RetryPolicy) -> Self {
        Self {
            inner,
            policy,
            circuit_breaker: None,
        }
    }

    /// Attach a circuit breaker to the retry wrapper.
    #[must_use]
    pub fn with_circuit_breaker(mut self, cb: Arc<CircuitBreaker>) -> Self {
        self.circuit_breaker = Some(cb);
        self
    }

    /// Return a reference to the retry policy.
    pub fn policy(&self) -> &RetryPolicy {
        &self.policy
    }

    /// Return a reference to the circuit breaker, if one is configured.
    pub fn circuit_breaker(&self) -> Option<&Arc<CircuitBreaker>> {
        self.circuit_breaker.as_ref()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Check the circuit breaker (if any). Returns `Ok(())` when the request
    /// is allowed, or an [`ExecutorError::Transport`] when the circuit is
    /// open.
    fn check_circuit(&self) -> Result<(), ExecutorError> {
        if let Some(cb) = &self.circuit_breaker {
            if let Err(open) = cb.check() {
                return Err(ExecutorError::Transport {
                    message: format!(
                        "circuit breaker open; retry after {:?}",
                        open.remaining
                    ),
                    retryable: false,
                });
            }
        }
        Ok(())
    }

    /// Record a success with the circuit breaker (if any).
    fn record_success(&self) {
        if let Some(cb) = &self.circuit_breaker {
            cb.record_success();
        }
    }

    /// Record a retryable failure with the circuit breaker (if any).
    fn record_failure(&self) {
        if let Some(cb) = &self.circuit_breaker {
            cb.record_failure();
        }
    }

    /// Compute the backoff delay for the given zero-based attempt number.
    fn backoff_delay(&self, attempt: u32) -> std::time::Duration {
        self.policy.backoff.delay_with_random_jitter(attempt)
    }
}

#[async_trait]
impl ModelExecutor for RetryModelExecutor {
    async fn complete(
        &self,
        request: InferenceRequest,
    ) -> Result<InferenceResponse, ExecutorError> {
        let total_attempts = self.policy.max_retries + 1;

        for attempt in 0..total_attempts {
            // Circuit breaker gate
            self.check_circuit()?;

            debug!(attempt, total_attempts, "RetryModelExecutor::complete attempt");

            let result = self.inner.complete(request.clone()).await;

            match result {
                Ok(response) => {
                    if attempt > 0 {
                        debug!(attempt, "RetryModelExecutor::complete succeeded after retry");
                    }
                    self.record_success();
                    return Ok(response);
                }
                Err(e) => {
                    if !is_retryable(&e) {
                        debug!(
                            attempt,
                            error = %e,
                            "RetryModelExecutor::complete non-retryable error, giving up"
                        );
                        // Non-retryable errors are not recorded against the
                        // circuit breaker.
                        return Err(e);
                    }

                    self.record_failure();

                    // Last attempt -- do not backoff, just return the error.
                    if attempt + 1 >= total_attempts {
                        warn!(
                            attempt,
                            error = %e,
                            "RetryModelExecutor::complete exhausted all retries"
                        );
                        return Err(e);
                    }

                    let delay = self.backoff_delay(attempt);
                    debug!(
                        attempt,
                        ?delay,
                        error = %e,
                        "RetryModelExecutor::complete retrying after backoff"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }

        // Unreachable: the loop always returns.
        unreachable!("retry loop exited without returning")
    }

    async fn stream(
        &self,
        request: InferenceRequest,
    ) -> Result<
        Box<dyn Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
        ExecutorError,
    > {
        let total_attempts = self.policy.max_retries + 1;

        for attempt in 0..total_attempts {
            self.check_circuit()?;

            debug!(attempt, total_attempts, "RetryModelExecutor::stream attempt");

            let result = self.inner.stream(request.clone()).await;

            match result {
                Ok(stream) => {
                    if attempt > 0 {
                        debug!(attempt, "RetryModelExecutor::stream connected after retry");
                    }
                    self.record_success();
                    return Ok(stream);
                }
                Err(e) => {
                    if !is_retryable(&e) {
                        debug!(
                            attempt,
                            error = %e,
                            "RetryModelExecutor::stream non-retryable error, giving up"
                        );
                        return Err(e);
                    }

                    self.record_failure();

                    if attempt + 1 >= total_attempts {
                        warn!(
                            attempt,
                            error = %e,
                            "RetryModelExecutor::stream exhausted all retries"
                        );
                        return Err(e);
                    }

                    let delay = self.backoff_delay(attempt);
                    debug!(
                        attempt,
                        ?delay,
                        error = %e,
                        "RetryModelExecutor::stream retrying after backoff"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }

        unreachable!("retry loop exited without returning")
    }

    async fn health(&self) -> Result<(), ExecutorError> {
        // Health checks are not retried -- they are diagnostic.
        self.inner.health().await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::{RunId, StepId};
    use polkagent_executor_trait::{
        ContentBlock, InferenceMessage, InferenceResponse, MessageRole, TokenUsage,
    };
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    // -----------------------------------------------------------------------
    // Test helper: a ModelExecutor that fails N times then succeeds.
    // -----------------------------------------------------------------------

    /// A test-only executor that returns errors for the first `fail_count`
    /// calls, then delegates to a success response.
    struct FailThenSucceed {
        fail_count: u32,
        error_factory: Box<dyn Fn() -> ExecutorError + Send + Sync>,
        calls: AtomicU32,
    }

    impl FailThenSucceed {
        fn new(
            fail_count: u32,
            error_factory: impl Fn() -> ExecutorError + Send + Sync + 'static,
        ) -> Arc<Self> {
            Arc::new(Self {
                fail_count,
                error_factory: Box::new(error_factory),
                calls: AtomicU32::new(0),
            })
        }

        fn call_count(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }

        fn success_response() -> InferenceResponse {
            InferenceResponse {
                text: "ok".into(),
                tool_calls: vec![],
                stop_reason: "end_turn".into(),
                usage: TokenUsage::default(),
                provider_request_id: Some("test-req".into()),
            }
        }
    }

    #[async_trait]
    impl ModelExecutor for FailThenSucceed {
        async fn complete(
            &self,
            _request: InferenceRequest,
        ) -> Result<InferenceResponse, ExecutorError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_count {
                Err((self.error_factory)())
            } else {
                Ok(Self::success_response())
            }
        }

        async fn stream(
            &self,
            _request: InferenceRequest,
        ) -> Result<
            Box<dyn Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
            ExecutorError,
        > {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_count {
                Err((self.error_factory)())
            } else {
                let resp = Self::success_response();
                let events: Vec<Result<StreamEvent, ExecutorError>> = vec![
                    Ok(StreamEvent::TextDelta {
                        delta: resp.text.clone(),
                    }),
                    Ok(StreamEvent::Completed { result: resp }),
                ];
                Ok(Box::new(futures::stream::iter(events)))
            }
        }

        async fn health(&self) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    /// A test executor that records call timestamps for backoff verification.
    struct TimingExecutor {
        calls: AtomicU32,
        timestamps: Mutex<Vec<Instant>>,
        fail_count: u32,
    }

    impl TimingExecutor {
        fn new(fail_count: u32) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicU32::new(0),
                timestamps: Mutex::new(Vec::new()),
                fail_count,
            })
        }

        fn timestamps(&self) -> Vec<Instant> {
            self.timestamps.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ModelExecutor for TimingExecutor {
        async fn complete(
            &self,
            _request: InferenceRequest,
        ) -> Result<InferenceResponse, ExecutorError> {
            self.timestamps.lock().unwrap().push(Instant::now());
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_count {
                Err(ExecutorError::RateLimit {
                    retry_after_secs: None,
                })
            } else {
                Ok(FailThenSucceed::success_response())
            }
        }

        async fn stream(
            &self,
            _request: InferenceRequest,
        ) -> Result<
            Box<dyn Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
            ExecutorError,
        > {
            Err(ExecutorError::Internal {
                message: "not implemented".into(),
            })
        }

        async fn health(&self) -> Result<(), ExecutorError> {
            Ok(())
        }
    }

    fn minimal_request() -> InferenceRequest {
        InferenceRequest {
            run_id: RunId::new(),
            step_id: StepId::new(),
            messages: vec![InferenceMessage {
                role: MessageRole::User,
                content: vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
            }],
            system: None,
            tools: vec![],
            model_id: "test-model".into(),
            max_tokens: 128,
            temperature: None,
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: Succeeds on first try (no retry needed)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn succeeds_on_first_try() {
        let inner = FailThenSucceed::new(0, || ExecutorError::Cancelled);
        let policy = RetryPolicy::fixed(3, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy);

        let resp = executor.complete(minimal_request()).await.expect("should succeed");
        assert_eq!(resp.text, "ok");
        assert_eq!(inner.call_count(), 1, "should have called inner exactly once");
    }

    // -----------------------------------------------------------------------
    // Test 2: Retries on transient failure and eventually succeeds
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn retries_on_transient_failure_then_succeeds() {
        // Fail twice with rate limit, then succeed.
        let inner = FailThenSucceed::new(2, || ExecutorError::RateLimit {
            retry_after_secs: None,
        });
        let policy = RetryPolicy::fixed(5, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy);

        let resp = executor.complete(minimal_request()).await.expect("should succeed after retries");
        assert_eq!(resp.text, "ok");
        assert_eq!(inner.call_count(), 3, "should have tried 3 times total");
    }

    // -----------------------------------------------------------------------
    // Test 3: Gives up after max retries
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn gives_up_after_max_retries() {
        // Always fail with a retryable error. Policy allows 2 retries.
        let inner = FailThenSucceed::new(100, || ExecutorError::Timeout { elapsed_ms: 5000 });
        let policy = RetryPolicy::fixed(2, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy);

        let err = executor
            .complete(minimal_request())
            .await
            .expect_err("should exhaust retries");

        assert!(
            matches!(err, ExecutorError::Timeout { .. }),
            "should return the last error"
        );
        // 1 initial + 2 retries = 3 total
        assert_eq!(inner.call_count(), 3);
    }

    // -----------------------------------------------------------------------
    // Test 4: Doesn't retry non-retryable errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn does_not_retry_authentication_error() {
        let inner = FailThenSucceed::new(100, || ExecutorError::Authentication {
            message: "bad key".into(),
        });
        let policy = RetryPolicy::fixed(5, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy);

        let err = executor
            .complete(minimal_request())
            .await
            .expect_err("should fail immediately");

        assert!(matches!(err, ExecutorError::Authentication { .. }));
        assert_eq!(inner.call_count(), 1, "must not retry authentication errors");
    }

    #[tokio::test]
    async fn does_not_retry_context_window_exceeded() {
        let inner = FailThenSucceed::new(100, || ExecutorError::ContextWindowExceeded {
            tokens_requested: 200_000,
            tokens_allowed: 100_000,
        });
        let policy = RetryPolicy::fixed(5, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy);

        let err = executor
            .complete(minimal_request())
            .await
            .expect_err("should fail immediately");

        assert!(matches!(err, ExecutorError::ContextWindowExceeded { .. }));
        assert_eq!(inner.call_count(), 1);
    }

    #[tokio::test]
    async fn does_not_retry_cancelled() {
        let inner = FailThenSucceed::new(100, || ExecutorError::Cancelled);
        let policy = RetryPolicy::fixed(5, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy);

        let err = executor
            .complete(minimal_request())
            .await
            .expect_err("should fail immediately");

        assert!(matches!(err, ExecutorError::Cancelled));
        assert_eq!(inner.call_count(), 1);
    }

    #[tokio::test]
    async fn does_not_retry_non_retryable_transport() {
        let inner = FailThenSucceed::new(100, || ExecutorError::Transport {
            message: "DNS failure".into(),
            retryable: false,
        });
        let policy = RetryPolicy::fixed(5, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy);

        let err = executor
            .complete(minimal_request())
            .await
            .expect_err("should fail immediately");

        assert!(matches!(err, ExecutorError::Transport { retryable: false, .. }));
        assert_eq!(inner.call_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 5: Circuit breaker trips after consecutive failures
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn circuit_breaker_trips_after_consecutive_failures() {
        // Circuit breaker opens after 2 consecutive failures.
        let cb = Arc::new(CircuitBreaker::new(2, 1, Duration::from_secs(60)));
        // Always fail with a retryable error.
        let inner = FailThenSucceed::new(100, || ExecutorError::RateLimit {
            retry_after_secs: None,
        });
        // Allow 5 retries so the circuit breaker is the limiting factor.
        let policy = RetryPolicy::fixed(5, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy)
            .with_circuit_breaker(cb.clone());

        let err = executor
            .complete(minimal_request())
            .await
            .expect_err("should fail");

        // After 2 retryable failures the circuit breaker opens. The third
        // attempt sees the open circuit and returns a transport error.
        // So we expect 2 real calls + 1 circuit-blocked attempt.
        assert_eq!(inner.call_count(), 2);
        assert!(
            matches!(err, ExecutorError::Transport { retryable: false, .. }),
            "circuit open should surface as non-retryable transport: got {err:?}"
        );

        // Verify the circuit is indeed open.
        assert!(cb.check().is_err(), "circuit breaker should be open");
    }

    // -----------------------------------------------------------------------
    // Test 6: Circuit breaker resets after success
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn circuit_breaker_resets_after_success() {
        let cb = Arc::new(CircuitBreaker::new(3, 1, Duration::from_secs(60)));
        // Fail once, then succeed.
        let inner = FailThenSucceed::new(1, || ExecutorError::RateLimit {
            retry_after_secs: None,
        });
        let policy = RetryPolicy::fixed(5, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy)
            .with_circuit_breaker(cb.clone());

        let resp = executor.complete(minimal_request()).await.expect("should succeed");
        assert_eq!(resp.text, "ok");

        // The circuit breaker should be closed because the success reset
        // the failure counter.
        assert!(cb.check().is_ok(), "circuit breaker should be closed after success");
    }

    // -----------------------------------------------------------------------
    // Test 7: Backoff delays increase (exponential)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn backoff_delays_increase_with_exponential_policy() {
        let inner = TimingExecutor::new(3);
        // Exponential backoff: base 20ms. Delays: 20ms, 40ms, 80ms.
        let policy = RetryPolicy::exponential(3, Duration::from_millis(20));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy);

        let resp = executor.complete(minimal_request()).await.expect("should succeed");
        assert_eq!(resp.text, "ok");

        let ts = inner.timestamps();
        assert_eq!(ts.len(), 4, "should have 4 attempts");

        // Verify that gaps between attempts increase.
        let gap1 = ts[1].duration_since(ts[0]);
        let gap2 = ts[2].duration_since(ts[1]);
        let gap3 = ts[3].duration_since(ts[2]);

        // gap1 ~20ms, gap2 ~40ms, gap3 ~80ms (with some tolerance)
        assert!(
            gap1 >= Duration::from_millis(15),
            "first gap {gap1:?} should be >= 15ms"
        );
        assert!(
            gap2 > gap1,
            "second gap {gap2:?} should be > first gap {gap1:?}"
        );
        assert!(
            gap3 > gap2,
            "third gap {gap3:?} should be > second gap {gap2:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: Streaming responses are retried correctly
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn streaming_retries_on_transient_failure() {
        use futures::StreamExt;

        // Fail twice with retryable transport error, then stream ok.
        let inner = FailThenSucceed::new(2, || ExecutorError::Transport {
            message: "connection reset".into(),
            retryable: true,
        });
        let policy = RetryPolicy::fixed(5, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy);

        let stream = executor
            .stream(minimal_request())
            .await
            .expect("should connect after retries");

        let events: Vec<_> = stream.collect().await;
        // Should have TextDelta + Completed
        assert!(events.len() >= 2, "should have at least 2 events");
        assert!(
            matches!(events.last().unwrap(), Ok(StreamEvent::Completed { .. })),
            "last event should be Completed"
        );
        assert_eq!(inner.call_count(), 3, "should have tried 3 times");
    }

    // -----------------------------------------------------------------------
    // Test 9: Retries retryable transport errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn retries_retryable_transport_errors() {
        let inner = FailThenSucceed::new(1, || ExecutorError::Transport {
            message: "connection reset".into(),
            retryable: true,
        });
        let policy = RetryPolicy::fixed(3, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy);

        let resp = executor.complete(minimal_request()).await.expect("should succeed");
        assert_eq!(resp.text, "ok");
        assert_eq!(inner.call_count(), 2);
    }

    // -----------------------------------------------------------------------
    // Test 10: Retries timeout errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn retries_timeout_errors() {
        let inner = FailThenSucceed::new(1, || ExecutorError::Timeout { elapsed_ms: 30000 });
        let policy = RetryPolicy::fixed(3, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy);

        let resp = executor.complete(minimal_request()).await.expect("should succeed");
        assert_eq!(resp.text, "ok");
        assert_eq!(inner.call_count(), 2);
    }

    // -----------------------------------------------------------------------
    // Test 11: Health check is not retried
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn health_check_delegates_without_retry() {
        use polkagent_executor_fake::FakeExecutor;

        let inner = FakeExecutor::failing(ExecutorError::Internal {
            message: "down".into(),
        });
        let policy = RetryPolicy::fixed(5, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy);

        let err = executor.health().await.expect_err("should fail");
        assert!(matches!(err, ExecutorError::Internal { .. }));
        // Health should not retry, so call_count is only 1 (from health()).
        // Note: FakeExecutor::health() does not increment call_count, it
        // only checks the mode. That is fine -- the key assertion is that
        // health returns the error directly.
    }

    // -----------------------------------------------------------------------
    // Test 12: Streaming does not retry non-retryable errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn streaming_does_not_retry_auth_error() {
        let inner = FailThenSucceed::new(100, || ExecutorError::Authentication {
            message: "expired token".into(),
        });
        let policy = RetryPolicy::fixed(5, Duration::from_millis(1));
        let executor = RetryModelExecutor::new(inner.clone() as Arc<dyn ModelExecutor>, policy);

        match executor.stream(minimal_request()).await {
            Err(err) => {
                assert!(
                    matches!(err, ExecutorError::Authentication { .. }),
                    "expected Authentication error, got: {err}"
                );
            }
            Ok(_) => panic!("should have failed immediately with auth error"),
        }
        assert_eq!(inner.call_count(), 1);
    }
}
