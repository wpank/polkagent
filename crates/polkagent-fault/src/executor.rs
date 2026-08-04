//! Fault-injecting [`ModelExecutor`] wrapper.
//!
//! [`FaultExecutor`] wraps any [`ModelExecutor`] implementation and intercepts
//! calls at named injection points, applying faults according to the
//! configured [`FaultInjector`].
//!
//! # Injection points
//!
//! - `"before_execute"` — checked before forwarding to the inner executor.
//! - `"after_execute"` — checked after the inner executor returns successfully.
//! - `"during_stream"` — checked when building a stream response.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream;
use polkagent_executor_trait::{
    ExecutorError, InferenceRequest, InferenceResponse, ModelExecutor, StreamEvent,
};

use crate::injector::FaultInjector;
use crate::types::Fault;

// ---------------------------------------------------------------------------
// FaultExecutor
// ---------------------------------------------------------------------------

/// A fault-injecting wrapper around any [`ModelExecutor`].
///
/// Constructed via [`FaultExecutor::new`]; an `Arc<FaultInjector>` is shared
/// with the test so the test can dynamically add, remove, and reset faults.
pub struct FaultExecutor<E: ModelExecutor> {
    inner: E,
    injector: Arc<FaultInjector>,
}

impl<E: ModelExecutor> FaultExecutor<E> {
    /// Wrap `inner` with a fault injector.
    pub fn new(inner: E, injector: Arc<FaultInjector>) -> Self {
        Self { inner, injector }
    }
}

#[async_trait]
impl<E: ModelExecutor> ModelExecutor for FaultExecutor<E> {
    async fn complete(
        &self,
        request: InferenceRequest,
    ) -> Result<InferenceResponse, ExecutorError> {
        // --- before_execute ---
        if let Some(fault) = self.injector.check("before_execute") {
            apply_executor_fault(fault).await?;
        }

        let mut response = self.inner.complete(request).await?;

        // --- after_execute ---
        if let Some(fault) = self.injector.check("after_execute") {
            match fault {
                Fault::Crash => panic!("FaultExecutor: crash at after_execute"),
                Fault::Error { message } => {
                    return Err(ExecutorError::Internal { message });
                }
                Fault::Timeout { ms } => {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    return Err(ExecutorError::Timeout { elapsed_ms: ms });
                }
                Fault::CorruptData { corruption } => {
                    corruption.apply(&mut response.text.as_bytes().to_vec());
                    // Corrupt text: prepend a marker so tests can observe it.
                    response.text = format!("[CORRUPTED]{}", response.text);
                    response.stop_reason = "corrupted".into();
                }
                Fault::SlowDown { ms } => {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                }
                Fault::PartialWrite => {
                    let half = response.text.len() / 2;
                    response.text.truncate(half);
                }
            }
        }

        Ok(response)
    }

    async fn stream(
        &self,
        request: InferenceRequest,
    ) -> Result<
        Box<dyn futures::Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
        ExecutorError,
    > {
        // --- before_execute (shared with stream) ---
        if let Some(fault) = self.injector.check("before_execute") {
            apply_executor_fault(fault).await?;
        }

        // --- during_stream ---
        if let Some(fault) = self.injector.check("during_stream") {
            match fault {
                Fault::Crash => panic!("FaultExecutor: crash during_stream"),
                Fault::Error { message } => {
                    let err = ExecutorError::Internal { message };
                    return Ok(Box::new(stream::iter(vec![Err(err)])));
                }
                Fault::Timeout { ms } => {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    return Err(ExecutorError::Timeout { elapsed_ms: ms });
                }
                Fault::CorruptData { .. } | Fault::SlowDown { .. } | Fault::PartialWrite => {
                    // For stream, fall through to inner executor for these.
                }
            }
        }

        self.inner.stream(request).await
    }

    async fn health(&self) -> Result<(), ExecutorError> {
        self.inner.health().await
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Apply a fault in a context where we have a `Result<_, ExecutorError>` to
/// return. Crash faults panic immediately; others return errors.
async fn apply_executor_fault(fault: Fault) -> Result<(), ExecutorError> {
    match fault {
        Fault::Crash => panic!("FaultExecutor: crash at before_execute"),
        Fault::Timeout { ms } => {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            Err(ExecutorError::Timeout { elapsed_ms: ms })
        }
        Fault::Error { message } => Err(ExecutorError::Internal { message }),
        Fault::SlowDown { ms } => {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            Ok(())
        }
        Fault::CorruptData { .. } | Fault::PartialWrite => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Corruption, FaultSchedule};
    use polkagent_core::{RunId, StepId};
    use polkagent_executor_trait::{
        ContentBlock, ExecutorError, InferenceMessage, MessageRole, TokenUsage,
    };

    // A minimal always-succeeding executor for testing.
    struct OkExecutor;

    #[async_trait]
    impl ModelExecutor for OkExecutor {
        async fn complete(
            &self,
            _request: InferenceRequest,
        ) -> Result<InferenceResponse, ExecutorError> {
            Ok(InferenceResponse {
                text: "hello".into(),
                tool_calls: vec![],
                stop_reason: "end_turn".into(),
                usage: TokenUsage::default(),
                provider_request_id: None,
            })
        }

        async fn stream(
            &self,
            _request: InferenceRequest,
        ) -> Result<
            Box<dyn futures::Stream<Item = Result<StreamEvent, ExecutorError>> + Send + Unpin>,
            ExecutorError,
        > {
            Ok(Box::new(stream::iter(vec![Ok(StreamEvent::Completed {
                result: InferenceResponse {
                    text: "hello".into(),
                    tool_calls: vec![],
                    stop_reason: "end_turn".into(),
                    usage: TokenUsage::default(),
                    provider_request_id: None,
                },
            })])))
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
                content: vec![ContentBlock::Text { text: "hi".into() }],
            }],
            system: None,
            tools: vec![],
            model_id: "test-model".into(),
            max_tokens: 256,
            temperature: None,
        }
    }

    #[tokio::test]
    async fn no_fault_passthrough_works_normally() {
        let injector = Arc::new(FaultInjector::new());
        let executor = FaultExecutor::new(OkExecutor, Arc::clone(&injector));
        let resp = executor.complete(minimal_request()).await.expect("ok");
        assert_eq!(resp.text, "hello");
    }

    #[tokio::test]
    async fn error_fault_before_execute_returns_error() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "before_execute",
            Fault::Error {
                message: "injected".into(),
            },
            FaultSchedule::Always,
        );
        let executor = FaultExecutor::new(OkExecutor, Arc::clone(&injector));
        let result = executor.complete(minimal_request()).await;
        assert!(result.is_err());
        let msg = format!("{:?}", result.unwrap_err());
        assert!(msg.contains("injected"));
    }

    #[tokio::test]
    async fn timeout_fault_returns_timeout_error() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "before_execute",
            Fault::Timeout { ms: 1 }, // 1ms to keep test fast
            FaultSchedule::Always,
        );
        let executor = FaultExecutor::new(OkExecutor, Arc::clone(&injector));
        let result = executor.complete(minimal_request()).await;
        assert!(matches!(result, Err(ExecutorError::Timeout { .. })));
    }

    #[tokio::test]
    async fn corrupt_data_fault_modifies_response() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "after_execute",
            Fault::CorruptData {
                corruption: Corruption::FlipBit(0),
            },
            FaultSchedule::Always,
        );
        let executor = FaultExecutor::new(OkExecutor, Arc::clone(&injector));
        let resp = executor.complete(minimal_request()).await.expect("ok");
        assert!(resp.text.contains("[CORRUPTED]"));
        assert_eq!(resp.stop_reason, "corrupted");
    }

    #[tokio::test]
    async fn crash_fault_panics() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault("before_execute", Fault::Crash, FaultSchedule::Always);
        let executor = Arc::new(FaultExecutor::new(OkExecutor, injector));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let rt = tokio::runtime::Runtime::new().expect("rt");
            rt.block_on(executor.complete(minimal_request()))
        }));
        assert!(result.is_err(), "crash fault should panic");
    }

    #[tokio::test]
    async fn removing_fault_restores_normal_behavior() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "before_execute",
            Fault::Error {
                message: "injected".into(),
            },
            FaultSchedule::Always,
        );
        let executor = FaultExecutor::new(OkExecutor, Arc::clone(&injector));
        // With fault: fails.
        assert!(executor.complete(minimal_request()).await.is_err());
        // Remove fault.
        injector.remove_fault("before_execute");
        // Without fault: succeeds.
        assert!(executor.complete(minimal_request()).await.is_ok());
    }
}
