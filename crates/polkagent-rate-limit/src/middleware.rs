//! Tower middleware for rate limiting.
//!
//! Provides [`RateLimitLayer`] which implements [`tower::Layer`] and wraps
//! services with automatic rate-limit enforcement.  The key extractor
//! function determines which key to use for each request.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tower::{Layer, Service};
use tracing::warn;

use crate::quota::QuotaResult;
use crate::RateLimiter;

// ---------------------------------------------------------------------------
// Key extractor
// ---------------------------------------------------------------------------

/// A function that extracts a rate-limit key from a request.
///
/// Common extractors might pull from headers (e.g., `X-Agent-Id`), path,
/// or IP address.
pub trait KeyExtractor<Req>: Send + Sync {
    /// Extract the rate-limit key from the request.
    ///
    /// Returns `None` to skip rate limiting for this request.
    fn extract(&self, req: &Req) -> Option<String>;
}

/// Blanket implementation for closures.
impl<F, Req> KeyExtractor<Req> for F
where
    F: Fn(&Req) -> Option<String> + Send + Sync,
{
    fn extract(&self, req: &Req) -> Option<String> {
        (self)(req)
    }
}

// ---------------------------------------------------------------------------
// RateLimitLayer
// ---------------------------------------------------------------------------

/// A [`tower::Layer`] that applies rate limiting to an inner service.
///
/// # Example
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use polkagent_rate_limit::{RateLimitLayer, TokenBucket};
/// use std::time::Duration;
///
/// let limiter = Arc::new(TokenBucket::per_second(100, 100.0));
/// let extractor = |_req: &http::Request<()>| Some("global".to_string());
/// let layer = RateLimitLayer::new(limiter, extractor);
/// ```
pub struct RateLimitLayer<Req> {
    limiter: Arc<dyn RateLimiter>,
    extractor: Arc<dyn KeyExtractor<Req>>,
    cost: u32,
}

impl<Req> Clone for RateLimitLayer<Req> {
    fn clone(&self) -> Self {
        Self {
            limiter: Arc::clone(&self.limiter),
            extractor: Arc::clone(&self.extractor),
            cost: self.cost,
        }
    }
}

impl<Req> fmt::Debug for RateLimitLayer<Req> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RateLimitLayer")
            .field("cost", &self.cost)
            .finish_non_exhaustive()
    }
}

impl<Req> RateLimitLayer<Req> {
    /// Create a new rate-limit layer.
    ///
    /// # Arguments
    ///
    /// * `limiter` -- the rate limiter to apply.
    /// * `extractor` -- extracts the rate-limit key from each request.
    pub fn new(limiter: Arc<dyn RateLimiter>, extractor: impl KeyExtractor<Req> + 'static) -> Self {
        Self {
            limiter,
            extractor: Arc::new(extractor),
            cost: 1,
        }
    }

    /// Set the cost per request (default: 1).
    #[must_use]
    pub fn with_cost(mut self, cost: u32) -> Self {
        self.cost = cost;
        self
    }
}

impl<S, Req> Layer<S> for RateLimitLayer<Req> {
    type Service = RateLimitService<S, Req>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService {
            inner,
            limiter: Arc::clone(&self.limiter),
            extractor: Arc::clone(&self.extractor),
            cost: self.cost,
        }
    }
}

// ---------------------------------------------------------------------------
// RateLimitService
// ---------------------------------------------------------------------------

/// The Tower [`Service`] produced by [`RateLimitLayer`].
pub struct RateLimitService<S, Req> {
    inner: S,
    limiter: Arc<dyn RateLimiter>,
    extractor: Arc<dyn KeyExtractor<Req>>,
    cost: u32,
}

impl<S: Clone, Req> Clone for RateLimitService<S, Req> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            limiter: Arc::clone(&self.limiter),
            extractor: Arc::clone(&self.extractor),
            cost: self.cost,
        }
    }
}

impl<S: fmt::Debug, Req> fmt::Debug for RateLimitService<S, Req> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RateLimitService")
            .field("inner", &self.inner)
            .field("cost", &self.cost)
            .finish_non_exhaustive()
    }
}

/// Error type for the rate-limit service.
#[derive(Debug)]
pub enum RateLimitServiceError<E> {
    /// The request was rate-limited.
    RateLimited(QuotaResult),
    /// The inner service produced an error.
    Inner(E),
}

impl<E: fmt::Display> fmt::Display for RateLimitServiceError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RateLimited(result) => {
                write!(
                    f,
                    "rate limited: remaining={}, retry_after={:?}",
                    result.remaining, result.retry_after
                )
            }
            Self::Inner(e) => write!(f, "{e}"),
        }
    }
}

impl<S, Req> Service<Req> for RateLimitService<S, Req>
where
    S: Service<Req> + Clone + Send + 'static,
    S::Future: Send,
    Req: Send + 'static,
{
    type Response = S::Response;
    type Error = RateLimitServiceError<S::Error>;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner
            .poll_ready(cx)
            .map_err(RateLimitServiceError::Inner)
    }

    fn call(&mut self, req: Req) -> Self::Future {
        let key = self.extractor.extract(&req);
        let limiter = Arc::clone(&self.limiter);
        let cost = self.cost;
        let mut inner = self.inner.clone();

        Box::pin(async move {
            if let Some(key) = key {
                let result = limiter.try_acquire(&key, cost);
                if !result.allowed {
                    warn!(
                        key,
                        remaining = result.remaining,
                        retry_after = ?result.retry_after,
                        "request rate limited"
                    );
                    return Err(RateLimitServiceError::RateLimited(result));
                }
            }
            inner.call(req).await.map_err(RateLimitServiceError::Inner)
        })
    }
}
