//! Corpus manifest: integrity verification and TOML loading for eval corpora.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::types::EvalCase;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur when working with corpus manifests.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// An I/O error occurred while reading the file.
    #[error("I/O error: {message}")]
    Io {
        /// Human-readable description.
        message: String,
    },
    /// The file could not be parsed as valid TOML.
    #[error("TOML parse error in {path}: {message}")]
    Parse {
        /// The file path that triggered the error.
        path: String,
        /// Human-readable description.
        message: String,
    },
    /// The digest recorded in the manifest does not match the computed digest.
    #[error("Corpus integrity check failed: expected {expected}, got {actual}")]
    IntegrityViolation {
        /// The digest stored in the manifest.
        expected: String,
        /// The digest computed from the cases.
        actual: String,
    },
}

/// Convenience alias for manifest results.
pub type Result<T> = std::result::Result<T, ManifestError>;

// ---------------------------------------------------------------------------
// CorpusManifest
// ---------------------------------------------------------------------------

/// A versioned, integrity-checked collection of evaluation cases.
///
/// The `digest` field is a SHA-256 hex string computed from the sorted
/// case hashes (see [`compute_corpus_digest`]).  Call
/// [`verify_corpus_integrity`] to confirm that `cases` matches `digest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusManifest {
    /// Machine-readable corpus identifier.
    pub id: String,
    /// Semantic version of this corpus snapshot (e.g. `"1.0.0"`).
    pub version: String,
    /// SHA-256 hex digest of the sorted case hashes.
    pub digest: String,
    /// The evaluation cases that make up the corpus.
    pub cases: Vec<EvalCase>,
    /// UTC timestamp when this manifest was created.
    pub created_at: DateTime<Utc>,
}

impl CorpusManifest {
    /// Create a new manifest, computing the digest automatically.
    #[must_use]
    pub fn new(id: impl Into<String>, version: impl Into<String>, cases: Vec<EvalCase>) -> Self {
        let digest = compute_corpus_digest(&cases);
        Self {
            id: id.into(),
            version: version.into(),
            digest,
            cases,
            created_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Digest helpers
// ---------------------------------------------------------------------------

/// Compute a deterministic SHA-256 digest for a slice of [`EvalCase`] values.
///
/// The digest is produced by:
/// 1. Serialising each case to canonical JSON.
/// 2. Hashing each JSON blob with SHA-256 to get a per-case hex digest.
/// 3. Sorting the per-case hex digests lexicographically.
/// 4. Hashing the sorted digests joined by newlines with SHA-256.
///
/// This approach is order-independent: two manifests containing the same
/// logical cases will always yield the same digest regardless of the order
/// in which the cases were appended.
#[must_use]
pub fn compute_corpus_digest(cases: &[EvalCase]) -> String {
    let mut case_hashes: Vec<String> = cases
        .iter()
        .map(|c| {
            // Serialise to canonical JSON; fall back to the case id if serialisation
            // somehow fails (should not happen in practice).
            let json = serde_json::to_string(c).unwrap_or_else(|_| c.id.clone());
            let hash = Sha256::digest(json.as_bytes());
            bytes_to_hex(&hash)
        })
        .collect();

    case_hashes.sort();

    let combined = case_hashes.join("\n");
    let final_hash = Sha256::digest(combined.as_bytes());
    bytes_to_hex(&final_hash)
}

/// Encode a byte slice as a lowercase hexadecimal string.
fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Verify that the cases in a [`CorpusManifest`] match its recorded digest.
///
/// Returns `true` when the digest is valid, `false` otherwise.
#[must_use]
pub fn verify_corpus_integrity(manifest: &CorpusManifest) -> bool {
    let computed = compute_corpus_digest(&manifest.cases);
    computed == manifest.digest
}

// ---------------------------------------------------------------------------
// TOML loading
// ---------------------------------------------------------------------------

/// TOML-serialisable representation of a corpus manifest.
///
/// This is a separate type so that the on-disk format can differ slightly
/// from the in-memory [`CorpusManifest`] (e.g., `created_at` stored as an
/// RFC 3339 string rather than a `DateTime<Utc>`).
#[derive(Debug, Deserialize)]
struct TomlManifest {
    id: String,
    version: String,
    digest: String,
    created_at: String,
    cases: Vec<EvalCase>,
}

/// Load a [`CorpusManifest`] from a TOML file at the given path.
///
/// The file must contain a top-level TOML table with at minimum the fields
/// `id`, `version`, `digest`, `created_at`, and `cases`.
///
/// # Errors
///
/// Returns [`ManifestError::Io`] when the file cannot be read, and
/// [`ManifestError::Parse`] when the file is not valid TOML or does not
/// match the expected schema.
pub fn load_corpus_from_toml(path: &Path) -> Result<CorpusManifest> {
    let content = std::fs::read_to_string(path).map_err(|e| ManifestError::Io {
        message: format!("Failed to read {}: {e}", path.display()),
    })?;

    let raw: TomlManifest = toml::from_str(&content).map_err(|e| ManifestError::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;

    let created_at = raw
        .created_at
        .parse::<DateTime<Utc>>()
        .map_err(|e| ManifestError::Parse {
            path: path.display().to_string(),
            message: format!("Invalid created_at timestamp: {e}"),
        })?;

    Ok(CorpusManifest {
        id: raw.id,
        version: raw.version,
        digest: raw.digest,
        cases: raw.cases,
        created_at,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EvalCategory, EvalInput, Expected};

    fn make_case(id: &str) -> EvalCase {
        EvalCase {
            id: id.into(),
            name: format!("Case {id}"),
            category: EvalCategory::General,
            input: EvalInput::from_prompt("test prompt"),
            expected: Expected::default(),
            tags: Vec::new(),
            timeout_secs: 30,
        }
    }

    // -----------------------------------------------------------------------
    // compute_corpus_digest
    // -----------------------------------------------------------------------

    #[test]
    fn digest_is_deterministic() {
        let cases = vec![make_case("a"), make_case("b")];
        let d1 = compute_corpus_digest(&cases);
        let d2 = compute_corpus_digest(&cases);
        assert_eq!(d1, d2);
    }

    #[test]
    fn digest_is_order_independent() {
        let cases_ab = vec![make_case("a"), make_case("b")];
        let cases_ba = vec![make_case("b"), make_case("a")];
        assert_eq!(
            compute_corpus_digest(&cases_ab),
            compute_corpus_digest(&cases_ba)
        );
    }

    #[test]
    fn digest_changes_when_case_changes() {
        let cases1 = vec![make_case("a")];
        let mut case_b = make_case("a");
        case_b.name = "Different name".into();
        let cases2 = vec![case_b];
        assert_ne!(
            compute_corpus_digest(&cases1),
            compute_corpus_digest(&cases2)
        );
    }

    #[test]
    fn digest_changes_when_case_added() {
        let cases1 = vec![make_case("a")];
        let cases2 = vec![make_case("a"), make_case("b")];
        assert_ne!(
            compute_corpus_digest(&cases1),
            compute_corpus_digest(&cases2)
        );
    }

    #[test]
    fn digest_of_empty_slice_is_stable() {
        let d1 = compute_corpus_digest(&[]);
        let d2 = compute_corpus_digest(&[]);
        assert_eq!(d1, d2);
        // Should be a 64-character hex string (SHA-256).
        assert_eq!(d1.len(), 64);
    }

    #[test]
    fn digest_is_hex_string_of_correct_length() {
        let cases = vec![make_case("x")];
        let digest = compute_corpus_digest(&cases);
        assert_eq!(digest.len(), 64);
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // -----------------------------------------------------------------------
    // verify_corpus_integrity
    // -----------------------------------------------------------------------

    #[test]
    fn verify_valid_manifest_returns_true() {
        let cases = vec![make_case("v1"), make_case("v2")];
        let manifest = CorpusManifest::new("test-corpus", "1.0.0", cases);
        assert!(verify_corpus_integrity(&manifest));
    }

    #[test]
    fn verify_tampered_digest_returns_false() {
        let cases = vec![make_case("t1")];
        let mut manifest = CorpusManifest::new("test-corpus", "1.0.0", cases);
        manifest.digest = "0".repeat(64);
        assert!(!verify_corpus_integrity(&manifest));
    }

    #[test]
    fn verify_tampered_cases_returns_false() {
        let cases = vec![make_case("u1")];
        let mut manifest = CorpusManifest::new("test-corpus", "1.0.0", cases);
        // Alter a case after the digest was computed.
        manifest.cases[0].name = "Tampered name".into();
        assert!(!verify_corpus_integrity(&manifest));
    }

    #[test]
    fn verify_empty_corpus_valid() {
        let manifest = CorpusManifest::new("empty", "0.0.1", vec![]);
        assert!(verify_corpus_integrity(&manifest));
    }

    // -----------------------------------------------------------------------
    // CorpusManifest::new
    // -----------------------------------------------------------------------

    #[test]
    fn new_sets_id_and_version() {
        let m = CorpusManifest::new("my-corpus", "2.3.4", vec![]);
        assert_eq!(m.id, "my-corpus");
        assert_eq!(m.version, "2.3.4");
    }

    #[test]
    fn new_computes_digest_automatically() {
        let cases = vec![make_case("auto")];
        let m = CorpusManifest::new("c", "1.0.0", cases.clone());
        assert_eq!(m.digest, compute_corpus_digest(&cases));
    }

    // -----------------------------------------------------------------------
    // load_corpus_from_toml
    // -----------------------------------------------------------------------

    fn write_toml_manifest(dir: &std::path::Path, manifest: &CorpusManifest) -> std::path::PathBuf {
        // Build a TOML string manually since CorpusManifest derives Serialize
        // using serde, which round-trips through JSON for nested EvalCase values.
        // For test purposes we use serde_json + a wrapper struct.
        #[derive(Serialize)]
        struct TomlOut<'a> {
            id: &'a str,
            version: &'a str,
            digest: &'a str,
            created_at: String,
            cases: &'a Vec<EvalCase>,
        }
        let out = TomlOut {
            id: &manifest.id,
            version: &manifest.version,
            digest: &manifest.digest,
            created_at: manifest.created_at.to_rfc3339(),
            cases: &manifest.cases,
        };
        let toml_str = toml::to_string(&out).expect("serialize to toml");
        let path = dir.join("manifest.toml");
        std::fs::write(&path, toml_str).expect("write manifest");
        path
    }

    #[test]
    fn load_corpus_from_toml_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cases = vec![make_case("rt1"), make_case("rt2")];
        let original = CorpusManifest::new("round-trip", "1.0.0", cases);
        let path = write_toml_manifest(dir.path(), &original);

        let loaded = load_corpus_from_toml(&path).expect("load");
        assert_eq!(loaded.id, original.id);
        assert_eq!(loaded.version, original.version);
        assert_eq!(loaded.digest, original.digest);
        assert_eq!(loaded.cases.len(), original.cases.len());
        assert!(verify_corpus_integrity(&loaded));
    }

    #[test]
    fn load_corpus_from_toml_missing_file_errors() {
        let result = load_corpus_from_toml(Path::new("/nonexistent/manifest.toml"));
        assert!(matches!(result, Err(ManifestError::Io { .. })));
    }

    #[test]
    fn load_corpus_from_toml_invalid_toml_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, b"not = [ valid toml ???").expect("write");
        let result = load_corpus_from_toml(&path);
        assert!(matches!(result, Err(ManifestError::Parse { .. })));
    }
}
