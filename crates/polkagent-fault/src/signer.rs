//! Fault-injecting [`Signer`] wrapper.
//!
//! [`FaultSigner`] wraps any [`Signer`] implementation and intercepts calls
//! at named injection points, applying faults according to the configured
//! [`FaultInjector`].
//!
//! # Injection points
//!
//! - `"before_sign"` — checked before forwarding to the inner signer.
//! - `"after_sign"` — checked after the inner signer returns successfully.
//!   The `CorruptData` fault at this point modifies the raw signature bytes.

use std::sync::Arc;

use async_trait::async_trait;
use polkagent_signer_trait::{
    CanonicalSignRequest, SignedPayload, Signer, SignerCapabilities, SignerError,
};

use crate::injector::FaultInjector;
use crate::types::Fault;

// ---------------------------------------------------------------------------
// FaultSigner
// ---------------------------------------------------------------------------

/// A fault-injecting wrapper around any [`Signer`].
pub struct FaultSigner<S: Signer> {
    inner: S,
    injector: Arc<FaultInjector>,
}

impl<S: Signer> FaultSigner<S> {
    /// Wrap `inner` with a fault injector.
    pub fn new(inner: S, injector: Arc<FaultInjector>) -> Self {
        Self { inner, injector }
    }
}

#[async_trait]
impl<S: Signer> Signer for FaultSigner<S> {
    async fn describe(&self) -> Result<SignerCapabilities, SignerError> {
        self.inner.describe().await
    }

    async fn sign(&self, request: CanonicalSignRequest) -> Result<SignedPayload, SignerError> {
        // --- before_sign ---
        if let Some(fault) = self.injector.check("before_sign") {
            apply_signer_fault_pre(fault)?;
        }

        let mut payload = self.inner.sign(request).await?;

        // --- after_sign ---
        if let Some(fault) = self.injector.check("after_sign") {
            match fault {
                Fault::Crash => panic!("FaultSigner: crash at after_sign"),
                Fault::Error { message } => {
                    return Err(SignerError::Internal { message });
                }
                Fault::Timeout { ms } => {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    return Err(SignerError::Timeout { elapsed_ms: ms });
                }
                Fault::CorruptData { corruption } => {
                    corruption.apply(&mut payload.signature);
                    // Also corrupt the signed extrinsic so callers observe it.
                    if !payload.signed_extrinsic.is_empty() {
                        payload.signed_extrinsic[0] ^= 0xFF;
                    }
                }
                Fault::SlowDown { ms } => {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                }
                Fault::PartialWrite => {
                    let half = payload.signature.len() / 2;
                    payload.signature.truncate(half);
                }
            }
        }

        Ok(payload)
    }

    async fn health(&self) -> Result<(), SignerError> {
        self.inner.health().await
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn apply_signer_fault_pre(fault: Fault) -> Result<(), SignerError> {
    match fault {
        Fault::Crash => panic!("FaultSigner: crash at before_sign"),
        Fault::Error { message } => Err(SignerError::Internal { message }),
        Fault::Timeout { .. } | Fault::SlowDown { .. } => {
            // Synchronous pre-check; timeout/slow-down would need async,
            // so we treat them as errors in the synchronous path.
            Err(SignerError::Internal {
                message: "timeout injected at before_sign".into(),
            })
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
    use polkagent_core::now;
    use polkagent_signer_trait::{
        AccountRef, ApprovalId, ChainProfileId, GrantDigest, MetadataDigest,
    };

    struct OkSigner;

    #[async_trait]
    impl Signer for OkSigner {
        async fn describe(&self) -> Result<SignerCapabilities, SignerError> {
            Ok(SignerCapabilities {
                accounts: vec![AccountRef::from_bytes([0u8; 32])],
                chain_profiles: vec![ChainProfileId::new("polkadot")],
                hardware_backed: false,
                display_name: "OkSigner".into(),
                can_sign: true,
            })
        }

        async fn sign(&self, request: CanonicalSignRequest) -> Result<SignedPayload, SignerError> {
            let signature = vec![0xAA; 64];
            let mut signed_extrinsic = request.payload.clone();
            signed_extrinsic.extend_from_slice(&signature);
            Ok(SignedPayload {
                signed_extrinsic,
                public_key: vec![0xBB; 32],
                signature,
            })
        }

        async fn health(&self) -> Result<(), SignerError> {
            Ok(())
        }
    }

    fn valid_request() -> CanonicalSignRequest {
        CanonicalSignRequest {
            request_id: "req-1".into(),
            payload: vec![1, 2, 3, 4],
            account: AccountRef::from_bytes([0u8; 32]),
            chain_profile: ChainProfileId::new("polkadot"),
            metadata_hash: MetadataDigest(vec![0u8; 32]),
            grant_digest: GrantDigest(vec![0u8; 32]),
            approval_id: ApprovalId::new("a-1"),
            expires_at: now() + chrono::Duration::hours(1),
        }
    }

    #[tokio::test]
    async fn no_fault_passthrough_produces_signature() {
        let injector = Arc::new(FaultInjector::new());
        let signer = FaultSigner::new(OkSigner, Arc::clone(&injector));
        let payload = signer.sign(valid_request()).await.expect("sign ok");
        assert_eq!(payload.signature.len(), 64);
        assert!(payload.signature.iter().all(|&b| b == 0xAA));
    }

    #[tokio::test]
    async fn corrupt_data_modifies_signature() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "after_sign",
            Fault::CorruptData {
                corruption: Corruption::FlipBit(0),
            },
            FaultSchedule::Always,
        );
        let signer = FaultSigner::new(OkSigner, Arc::clone(&injector));
        let payload = signer.sign(valid_request()).await.expect("sign ok");
        // First byte of signature should be 0xAA ^ 0xFF = 0x55.
        assert_eq!(payload.signature[0], 0xAA ^ 0xFF);
        // The rest should still be 0xAA.
        assert!(payload.signature[1..].iter().all(|&b| b == 0xAA));
    }

    #[tokio::test]
    async fn error_fault_before_sign_returns_error() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault(
            "before_sign",
            Fault::Error {
                message: "injected pre-sign error".into(),
            },
            FaultSchedule::Always,
        );
        let signer = FaultSigner::new(OkSigner, Arc::clone(&injector));
        let result = signer.sign(valid_request()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn crash_fault_panics() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault("before_sign", Fault::Crash, FaultSchedule::Always);
        let signer = Arc::new(FaultSigner::new(OkSigner, injector));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let rt = tokio::runtime::Runtime::new().expect("rt");
            rt.block_on(signer.sign(valid_request()))
        }));
        assert!(result.is_err(), "crash fault should panic");
    }

    #[tokio::test]
    async fn partial_write_truncates_signature() {
        let injector = Arc::new(FaultInjector::new());
        injector.add_fault("after_sign", Fault::PartialWrite, FaultSchedule::Always);
        let signer = FaultSigner::new(OkSigner, Arc::clone(&injector));
        let payload = signer.sign(valid_request()).await.expect("ok");
        assert_eq!(payload.signature.len(), 32); // half of 64
    }
}
