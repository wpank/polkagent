//! `polkagent-skill` — Skill package loading, validation, dependency
//! resolution, and execution preparation for the Polkagent platform.
//!
//! This crate implements the skill package system that allows agents to
//! load and activate modular "skills" — declarative packages that bundle
//! a system prompt, tool requirements, grant requirements, and
//! configuration into a single unit.
//!
//! # Module overview
//!
//! | Module | Responsibility |
//! |--------|---------------|
//! | [`manifest`] | [`SkillManifest`] deserialization from TOML, [`SkillId`] type, validation. |
//! | [`loader`] | [`SkillLoader`] for discovering and loading manifests from the filesystem. |
//! | [`resolver`] | Dependency resolution with topological sort and semver checking. |
//! | [`runner`] | [`SkillRunner`] and [`PreparedSkill`] for execution preparation. |
//! | [`error`] | [`SkillError`] enum covering all failure modes. |
//!
//! # Quick start
//!
//! ```rust,no_run
//! use polkagent_skill::loader::SkillLoader;
//! use polkagent_skill::resolver;
//!
//! // Discover all installed skills.
//! let loader = SkillLoader::with_default_paths();
//! let manifests = loader.discover_skills();
//!
//! // Resolve the dependency graph.
//! let order = resolver::resolve(&manifests).expect("no cycles");
//!
//! println!("Skills in dependency order:");
//! for id in &order {
//!     println!("  {id}");
//! }
//! ```

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod error;
pub mod loader;
pub mod manifest;
pub mod resolver;
pub mod runner;

// ---------------------------------------------------------------------------
// Convenience re-exports
// ---------------------------------------------------------------------------

pub use error::SkillError;
pub use loader::SkillLoader;
pub use manifest::{SkillId, SkillManifest};
pub use resolver::resolve;
pub use runner::{PreparedSkill, SkillRunner, ToolRegistry};
