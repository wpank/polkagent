//! `polkagent-card` — ActionCard UX surface for irreversible on-chain effects.
//!
//! An [`ActionCard`] is the safety-critical display element shown to a user
//! before they approve an irreversible or value-moving chain action. It
//! enforces a hard separation between:
//!
//! - **Canonical sections** (`canonical_sections`) — typed, metadata-derived
//!   facts that the model can never alter.
//! - **Narrative sections** (`narrative_sections`) — model-generated prose
//!   that is always displayed *after* canonical data and always labeled
//!   `"AI-generated explanation"`.
//!
//! Approval decisions are bound to the card's `payload_hash`, not to any
//! model description.
//!
//! # Quick start
//!
//! ```rust
//! use polkagent_card::builder::ActionCardBuilder;
//! use polkagent_card::sections::{RiskFlag, RiskFlagType, SectionSource, Severity};
//! use polkagent_card::render::render_text;
//!
//! let card = ActionCardBuilder::new("Transfer 10 DOT")
//!     .add_canonical("Amount",    "10 DOT",    SectionSource::Metadata)
//!     .add_canonical("Recipient", "5GrwvaEF\u{2026}", SectionSource::Chain)
//!     .add_narrative("Why?", "Agent is rebalancing your staking position.")
//!     .add_risk_flag(RiskFlag::new(
//!         RiskFlagType::FirstTimeRecipient,
//!         "Recipient has not been seen before",
//!         Severity::Medium,
//!     ))
//!     .with_payload_hash("deadbeef1234")
//!     .build();
//!
//! let text = render_text(&card);
//! assert!(text.contains("Transfer 10 DOT"));
//! assert!(text.contains("deadbeef1234"));
//! ```
//!
//! # Module layout
//!
//! | Module | Contents |
//! |---|---|
//! | [`card`] | [`ActionCard`], [`RiskLevel`], [`SimulationSummary`] |
//! | [`sections`] | [`CanonicalSection`], [`NarrativeSection`], [`RiskFlag`], [`RiskFlagType`], [`Severity`], [`SectionSource`] |
//! | [`builder`] | [`ActionCardBuilder`] |
//! | [`render`] | [`render_text`], [`render_tui`] |

pub mod builder;
pub mod card;
pub mod render;
pub mod sections;

// Re-export the most commonly used types at the crate root so callers can
// write `polkagent_card::ActionCard` without knowing which module it lives in.
pub use builder::ActionCardBuilder;
pub use card::{ActionCard, RiskLevel, SimulationSummary};
pub use render::{render_text, render_tui};
pub use sections::{
    CanonicalSection, NarrativeSection, RiskFlag, RiskFlagType, SectionSource, Severity,
};
