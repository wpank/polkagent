//! Artifact domain types.
//!
//! An [`Artifact`] is a durable, attributable, immutable piece of content or
//! evidence produced during a run. Artifacts form a DAG via parent links,
//! creating an evidence lineage that can be traced from any artifact back to
//! its root inputs.
//!
//! Content is stored separately from metadata: the [`BlobRef`] points to the
//! raw bytes (in a blob store), while the `Artifact` record holds the
//! metadata, digest, and parent references.
//!
//! # Invariants
//!
//! - An artifact's `digest` is computed over its body content with BLAKE3
//!   and verified on read.
//! - Artifacts are immutable once created. A new artifact must be created for
//!   any modification.
//! - `SecretForbidden`-classified artifacts must never appear in model
//!   context, logs, or public projections.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::DataClassification;
use crate::ids::{ArtifactId, RunId, StepId};

// ---------------------------------------------------------------------------
// BlobRef
// ---------------------------------------------------------------------------

/// A content-addressed reference to raw artifact bytes in the blob store.
///
/// The `digest` is a BLAKE3 hash of the raw byte content. The blob store
/// uses this hash as the storage key, deduplicated across all runs.
///
/// Where interoperability requires SHA-256 (Sigstore, IPFS CIDv0, on-chain
/// hash references), the optional `sha256_hex` field carries the parallel
/// digest.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlobRef {
    /// BLAKE3 hex digest of the raw content bytes.
    ///
    /// This is the primary storage key and integrity check.
    pub blake3_hex: String,
    /// Optional parallel SHA-256 hex digest for interoperability.
    pub sha256_hex: Option<String>,
    /// Total byte size of the raw content.
    pub size_bytes: u64,
}

impl BlobRef {
    /// Compute a `BlobRef` from raw bytes using BLAKE3.
    ///
    /// # Note
    ///
    /// The SHA-256 digest is not computed here; callers that require it must
    /// compute it separately and set `sha256_hex` explicitly.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let hash = blake3::hash(bytes);
        Self {
            blake3_hex: hash.to_hex().to_string(),
            sha256_hex: None,
            size_bytes: bytes.len() as u64,
        }
    }

    /// Returns `true` if `bytes` match the stored BLAKE3 digest.
    ///
    /// This is the integrity check performed by `ContentVerifier` on every
    /// read.
    #[must_use]
    pub fn verify(&self, bytes: &[u8]) -> bool {
        let hash = blake3::hash(bytes);
        hash.to_hex().as_str() == self.blake3_hex
    }
}

// ---------------------------------------------------------------------------
// ArtifactKind
// ---------------------------------------------------------------------------

/// The semantic type of an artifact.
///
/// This determines how the artifact is displayed, linked, and used. The
/// `polkagent-app` layer uses `ArtifactKind` to route artifact content to
/// the appropriate viewer or processor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ArtifactKind {
    /// A file produced or modified during a run.
    File {
        /// Filesystem path relative to the workspace root.
        path: String,
        /// MIME type, if known.
        mime_type: Option<String>,
    },
    /// A code diff or patch.
    Diff {
        /// Git ref the diff is based on.
        base_ref: Option<String>,
        /// Git ref the diff targets.
        target_ref: Option<String>,
    },
    /// A structured plan produced by a model or workflow.
    Plan {
        /// Format of the plan (e.g. `"json"`, `"markdown"`, `"toml"`).
        format: String,
    },
    /// A decoded on-chain call with full metadata context.
    DecodedCall {
        /// Which chain profile's metadata was used.
        chain_profile: String,
        /// Hex digest of the metadata used for decoding.
        metadata_hash: String,
    },
    /// A simulation result for a proposed chain action.
    Simulation {
        /// Which chain profile the simulation ran against.
        chain_profile: String,
        /// Block reference at which the simulation was run.
        block_ref: Option<String>,
    },
    /// A signed receipt from a completed chain action.
    Receipt {
        /// Which chain profile submitted the transaction.
        chain_profile: String,
        /// On-chain transaction hash.
        tx_hash: String,
    },
    /// A test result or evaluation output.
    TestResult {
        /// Name of the test suite.
        suite: String,
        /// Whether the suite passed overall.
        passed: bool,
    },
    /// A runtime metadata snapshot (Polkadot `rpc_metadata`).
    RuntimeMetadata {
        /// Chain profile.
        chain_profile: String,
        /// Substrate spec version.
        spec_version: u32,
    },
    /// The assembled context pack passed to a model inference.
    ContextPack {
        /// BLAKE3 digest of the pack contents (for deduplication).
        digest: String,
    },
    /// A raw model response captured as durable evidence.
    ModelResponse {
        /// Model identifier in `provider/model-id` format.
        model_id: String,
    },
    /// Custom artifact type from an extension or adapter.
    Custom {
        /// A URI identifying the artifact type
        /// (e.g. `"myapp://result-v1"`).
        type_uri: String,
    },
}

// ---------------------------------------------------------------------------
// Artifact
// ---------------------------------------------------------------------------

/// A durable, immutable unit of evidence produced during a run.
///
/// Artifacts are content-addressed: the [`BlobRef`] points to the raw bytes
/// and carries the BLAKE3 digest used for integrity verification.
///
/// The `parents` field forms a DAG of evidence lineage: a `Receipt` artifact
/// links to the `Simulation`, `ApprovalRequest`, and `DecodedCall` artifacts
/// that preceded it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Stable, globally unique identifier.
    pub id: ArtifactId,
    /// The run that produced this artifact.
    pub run_id: Option<RunId>,
    /// The step within the run that produced this artifact.
    pub step_id: Option<StepId>,
    /// The semantic type of content.
    pub kind: ArtifactKind,
    /// Content-addressed reference to the raw bytes.
    pub blob_ref: BlobRef,
    /// Data classification controlling access, retention, and projection
    /// visibility.
    pub classification: DataClassification,
    /// Parent artifacts whose content this artifact derives from.
    ///
    /// Parent links form a DAG that can be traversed to reconstruct the
    /// complete evidence chain for any action.
    pub parents: Vec<ArtifactId>,
    /// When this artifact was created.
    pub created_at: DateTime<Utc>,
    /// Arbitrary key–value metadata (extension-provided labels, commit SHAs,
    /// tool names, etc.).
    pub metadata: std::collections::HashMap<String, String>,
}

impl Artifact {
    /// Create an artifact record from raw bytes.
    ///
    /// Computes the BLAKE3 digest and sets default classification to `Public`.
    #[must_use]
    pub fn from_bytes(
        id: ArtifactId,
        kind: ArtifactKind,
        bytes: &[u8],
    ) -> Self {
        Self {
            id,
            run_id: None,
            step_id: None,
            kind,
            blob_ref: BlobRef::from_bytes(bytes),
            classification: DataClassification::default(),
            parents: Vec::new(),
            created_at: Utc::now(),
            metadata: std::collections::HashMap::new(),
        }
    }

    /// Returns `true` if the supplied bytes match this artifact's stored
    /// BLAKE3 digest.
    #[must_use]
    pub fn verify_integrity(&self, bytes: &[u8]) -> bool {
        self.blob_ref.verify(bytes)
    }

    /// Returns `true` if this artifact may appear in model context and public
    /// projections.
    #[must_use]
    pub fn is_context_safe(&self) -> bool {
        !matches!(
            self.classification,
            DataClassification::SecretForbidden
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- BlobRef ---

    #[test]
    fn blob_ref_from_bytes_produces_correct_digest() {
        let data = b"hello polkagent";
        let blob = BlobRef::from_bytes(data);
        assert_eq!(blob.size_bytes, data.len() as u64);
        assert!(!blob.blake3_hex.is_empty());
        assert!(blob.sha256_hex.is_none());
    }

    #[test]
    fn blob_ref_verify_succeeds_on_matching_bytes() {
        let data = b"deterministic content";
        let blob = BlobRef::from_bytes(data);
        assert!(blob.verify(data));
    }

    #[test]
    fn blob_ref_verify_fails_on_tampered_bytes() {
        let data = b"original content";
        let blob = BlobRef::from_bytes(data);
        assert!(!blob.verify(b"tampered content"));
    }

    #[test]
    fn blob_ref_digest_is_deterministic() {
        let data = b"consistent input";
        let a = BlobRef::from_bytes(data);
        let b = BlobRef::from_bytes(data);
        assert_eq!(a.blake3_hex, b.blake3_hex);
    }

    #[test]
    fn blob_ref_serde_round_trip() {
        let blob = BlobRef::from_bytes(b"test");
        let json = serde_json::to_string(&blob).unwrap();
        let back: BlobRef = serde_json::from_str(&json).unwrap();
        assert_eq!(blob, back);
    }

    // --- ArtifactKind ---

    #[test]
    fn artifact_kind_file_serde_round_trip() {
        let kind = ArtifactKind::File {
            path: "src/main.rs".into(),
            mime_type: Some("text/x-rust".into()),
        };
        let json = serde_json::to_string(&kind).unwrap();
        let back: ArtifactKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }

    #[test]
    fn artifact_kind_receipt_serde_round_trip() {
        let kind = ArtifactKind::Receipt {
            chain_profile: "polkadot-mainnet".into(),
            tx_hash: "0xdeadbeef".into(),
        };
        let json = serde_json::to_string(&kind).unwrap();
        let back: ArtifactKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }

    #[test]
    fn artifact_kind_custom_serde_round_trip() {
        let kind = ArtifactKind::Custom {
            type_uri: "myapp://result-v1".into(),
        };
        let json = serde_json::to_string(&kind).unwrap();
        let back: ArtifactKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }

    // --- Artifact ---

    #[test]
    fn artifact_from_bytes_computes_blob_ref() {
        let data = b"artifact content";
        let art = Artifact::from_bytes(
            ArtifactId::new(),
            ArtifactKind::Custom {
                type_uri: "test://v1".into(),
            },
            data,
        );
        assert_eq!(art.blob_ref.size_bytes, data.len() as u64);
        assert!(art.verify_integrity(data));
        assert!(!art.verify_integrity(b"wrong content"));
    }

    #[test]
    fn artifact_default_classification_is_public() {
        let art = Artifact::from_bytes(
            ArtifactId::new(),
            ArtifactKind::Plan {
                format: "json".into(),
            },
            b"{}",
        );
        assert_eq!(art.classification, DataClassification::Public);
        assert!(art.is_context_safe());
    }

    #[test]
    fn artifact_secret_forbidden_is_not_context_safe() {
        let mut art = Artifact::from_bytes(
            ArtifactId::new(),
            ArtifactKind::Custom {
                type_uri: "secret://key".into(),
            },
            b"secret",
        );
        art.classification = DataClassification::SecretForbidden;
        assert!(!art.is_context_safe());
    }

    #[test]
    fn artifact_serde_round_trip() {
        let data = b"hello";
        let mut art = Artifact::from_bytes(
            ArtifactId::new(),
            ArtifactKind::File {
                path: "out.txt".into(),
                mime_type: None,
            },
            data,
        );
        art.run_id = Some(RunId::new());
        art.parents.push(ArtifactId::new());
        art.metadata.insert("tool".into(), "write_file".into());

        let json = serde_json::to_string(&art).unwrap();
        let back: Artifact = serde_json::from_str(&json).unwrap();
        assert_eq!(art.id, back.id);
        assert_eq!(art.classification, back.classification);
        assert_eq!(art.parents, back.parents);
        assert_eq!(art.metadata, back.metadata);
    }

    #[test]
    fn artifact_with_parents_preserves_lineage() {
        let parent_id = ArtifactId::new();
        let mut art = Artifact::from_bytes(
            ArtifactId::new(),
            ArtifactKind::Diff {
                base_ref: Some("main".into()),
                target_ref: Some("feature".into()),
            },
            b"diff content",
        );
        art.parents = vec![parent_id];
        assert_eq!(art.parents.len(), 1);
        assert_eq!(art.parents[0], parent_id);
    }
}
