use std::fmt;

/// Classification of an error for retry/circuit-breaker decision making.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// The error is transient and the operation should be retried.
    Retryable,
    /// The error is permanent and the operation should NOT be retried.
    NonRetryable,
    /// The error indicates a systemic failure; the circuit breaker should trip.
    CircuitBreak,
}

/// Trait for classifying errors to determine retry behavior.
///
/// Implement this trait to control which errors are retried, which cause
/// immediate failure, and which should trip the circuit breaker.
pub trait ErrorClassifier<E> {
    /// Classify the given error.
    fn classify(&self, error: &E) -> ErrorClass;
}

/// Default classifier that treats all errors as retryable.
#[derive(Debug, Clone, Copy)]
pub struct AlwaysRetry;

impl<E> ErrorClassifier<E> for AlwaysRetry {
    fn classify(&self, _error: &E) -> ErrorClass {
        ErrorClass::Retryable
    }
}

/// Classifier that treats all errors as non-retryable.
#[derive(Debug, Clone, Copy)]
pub struct NeverRetry;

impl<E> ErrorClassifier<E> for NeverRetry {
    fn classify(&self, _error: &E) -> ErrorClass {
        ErrorClass::NonRetryable
    }
}

/// Classifier that uses the `Display` representation of an error
/// to match against a list of retryable message patterns.
#[derive(Debug, Clone)]
pub struct PatternClassifier {
    /// Substrings that, if found in the error message, indicate a retryable error.
    pub retryable_patterns: Vec<String>,
    /// Substrings that indicate the circuit breaker should open.
    pub circuit_break_patterns: Vec<String>,
}

impl PatternClassifier {
    /// Create a new `PatternClassifier`.
    pub fn new(retryable: Vec<String>, circuit_break: Vec<String>) -> Self {
        Self {
            retryable_patterns: retryable,
            circuit_break_patterns: circuit_break,
        }
    }
}

impl<E: fmt::Display> ErrorClassifier<E> for PatternClassifier {
    fn classify(&self, error: &E) -> ErrorClass {
        let msg = error.to_string();

        for pattern in &self.circuit_break_patterns {
            if msg.contains(pattern.as_str()) {
                return ErrorClass::CircuitBreak;
            }
        }

        for pattern in &self.retryable_patterns {
            if msg.contains(pattern.as_str()) {
                return ErrorClass::Retryable;
            }
        }

        ErrorClass::NonRetryable
    }
}

/// Classifier backed by a closure.
pub struct FnClassifier<F> {
    f: F,
}

impl<F> FnClassifier<F> {
    /// Create a new classifier from a closure.
    pub fn new(f: F) -> Self {
        Self { f }
    }
}

impl<E, F> ErrorClassifier<E> for FnClassifier<F>
where
    F: Fn(&E) -> ErrorClass,
{
    fn classify(&self, error: &E) -> ErrorClass {
        (self.f)(error)
    }
}

impl<F> fmt::Debug for FnClassifier<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FnClassifier").finish()
    }
}
