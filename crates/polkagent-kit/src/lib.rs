//! Product kit manifest parsing, validation, and install/uninstall lifecycle.
//!
//! A *product kit* bundles multiple skills into a single installable unit
//! with shared policy, capability requirements, and UX metadata. This
//! crate provides:
//!
//! - [`KitManifest`] / [`KitId`] — TOML manifest parsing and validation.
//! - [`manager`] — install, uninstall, and validation helpers.
//! - [`KitError`] — error types for the kit subsystem.

pub mod error;
pub mod manifest;
pub mod manager;

pub use error::KitError;
pub use manifest::{KitId, KitManifest, SkillEntry, SkillRole};
pub use manager::{InstalledKit, KitValidation, prepare_install, skills_to_unregister, validate_kit};
