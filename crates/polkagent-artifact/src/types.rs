//! Re-exports of artifact domain types from [`polkagent_core`].
//!
//! All artifact domain types (`Artifact`, `ArtifactKind`, `BlobRef`) are
//! defined in `polkagent-core` to prevent duplication across the workspace.
//! This module simply re-exports them for convenience so that callers of
//! `polkagent-artifact` can import from a single crate.

pub use polkagent_core::artifact::{Artifact, ArtifactKind, BlobRef};
