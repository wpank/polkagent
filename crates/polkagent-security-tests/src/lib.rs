//! Security verification tests for the Polkagent platform.
//!
//! This crate contains integration-level security tests that verify the
//! security invariants documented in PRD-15. All tests are read-only
//! verifications of the public API surface; they do not require any code
//! changes to the main crates.
//!
//! # Test modules
//!
//! | Module | Coverage |
//! |---|---|
//! | `secrets_not_in_debug` | Debug/Display impls do not leak secrets |
//! | `classification_enforcement` | DataClassification boundary enforcement |
//! | `grant_denial` | Default-deny, expiry, budget, rate-limit, deny-override |
//! | `effect_safety` | Idempotency, outcome immutability, claim exclusivity |
//! | `injection_resistance` | Input validation, path traversal, OOM limits |
//! | `card_safety` | Canonical/narrative separation, risk consistency |
