//! Shared domain types for governance tools.
//!
//! These types model the core OpenGov primitives: referenda, tracks, votes,
//! delegations, and treasury state. All types derive [`Serialize`] and
//! [`Deserialize`] so they can be returned as structured JSON from tool
//! handlers.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ReferendumStatus
// ---------------------------------------------------------------------------

/// The lifecycle status of a referendum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferendumStatus {
    /// The referendum is in the preparation period before deciding begins.
    Preparing,
    /// The referendum is in its decision period and votes are being tallied.
    Deciding,
    /// The referendum has met approval and support thresholds and is confirming.
    Confirming,
    /// The referendum was approved and enacted.
    Approved,
    /// The referendum was rejected (did not meet thresholds).
    Rejected,
    /// The referendum was cancelled by governance.
    Cancelled,
    /// The referendum timed out without reaching a decision.
    TimedOut,
    /// The referendum was killed (slashed).
    Killed,
}

impl std::fmt::Display for ReferendumStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Preparing => "preparing",
            Self::Deciding => "deciding",
            Self::Confirming => "confirming",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Killed => "killed",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// TimelineEvent
// ---------------------------------------------------------------------------

/// A significant event in the referendum lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    /// The kind of event (e.g. "submitted", "decision_started").
    pub event: String,
    /// The block number at which the event occurred.
    pub block_number: u64,
    /// The timestamp of the event, if known.
    pub timestamp: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Referendum
// ---------------------------------------------------------------------------

/// A governance referendum with its current state and tally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Referendum {
    /// The referendum index (monotonically increasing).
    pub index: u32,
    /// The governance track this referendum belongs to.
    pub track: u16,
    /// The dispatch origin required (e.g. "Root", "Treasurer").
    pub origin: String,
    /// Current lifecycle status.
    pub status: ReferendumStatus,
    /// Total ayes in planck units.
    pub ayes: u128,
    /// Total nays in planck units.
    pub nays: u128,
    /// Total support (turnout) in planck units.
    pub support: u128,
    /// The account that submitted the referendum.
    pub proposer: String,
    /// Significant lifecycle events with block numbers.
    pub timeline: Vec<TimelineEvent>,
}

// ---------------------------------------------------------------------------
// Track
// ---------------------------------------------------------------------------

/// An OpenGov governance track with its parameters.
///
/// Tracks define the rules under which referenda are decided, including
/// time periods and approval/support curves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    /// The track ID.
    pub id: u16,
    /// Human-readable track name (e.g. "Root", "SmallTipper").
    pub name: String,
    /// Maximum number of referenda that can be decided simultaneously.
    pub max_deciding: u32,
    /// Decision period in blocks.
    pub decision_period: u32,
    /// Confirmation period in blocks.
    pub confirm_period: u32,
    /// Description of the minimum approval curve (e.g. "linear_decreasing(0.5, 0.0)").
    pub min_approval_curve: String,
    /// Description of the minimum support curve (e.g. "reciprocal(0.01)").
    pub min_support_curve: String,
    /// Preparation period in blocks (before deciding can start).
    pub prepare_period: u32,
    /// Minimum deposit required to submit a referendum on this track (in planck).
    pub min_deposit: u128,
}

// ---------------------------------------------------------------------------
// VoteDirection
// ---------------------------------------------------------------------------

/// The direction of a vote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoteDirection {
    /// A vote in favor of the referendum.
    Aye,
    /// A vote against the referendum.
    Nay,
    /// An abstain vote (counts toward support but not approval).
    Abstain,
    /// A split vote (part aye, part nay).
    Split,
}

impl std::fmt::Display for VoteDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Aye => "aye",
            Self::Nay => "nay",
            Self::Abstain => "abstain",
            Self::Split => "split",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// Conviction
// ---------------------------------------------------------------------------

/// The conviction multiplier applied to a vote's lock period and weight.
///
/// Higher conviction multiplies the vote's weight but extends the lock
/// period for the staked tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Conviction {
    /// No conviction: 0.1x vote weight, no lock.
    #[serde(rename = "none")]
    None,
    /// 1x conviction: 1x vote weight, locked for 1 period.
    #[serde(rename = "locked_1x")]
    Locked1X,
    /// 2x conviction: 2x vote weight, locked for 2 periods.
    #[serde(rename = "locked_2x")]
    Locked2X,
    /// 3x conviction: 3x vote weight, locked for 4 periods.
    #[serde(rename = "locked_3x")]
    Locked3X,
    /// 4x conviction: 4x vote weight, locked for 8 periods.
    #[serde(rename = "locked_4x")]
    Locked4X,
    /// 5x conviction: 5x vote weight, locked for 16 periods.
    #[serde(rename = "locked_5x")]
    Locked5X,
    /// 6x conviction: 6x vote weight, locked for 32 periods.
    #[serde(rename = "locked_6x")]
    Locked6X,
}

impl Conviction {
    /// Return the vote weight multiplier for this conviction level.
    ///
    /// `None` returns 1 (representing 0.1x in the actual runtime, but
    /// here we return the integer multiplier used for the lock period).
    #[must_use]
    pub fn multiplier(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Locked1X => 1,
            Self::Locked2X => 2,
            Self::Locked3X => 3,
            Self::Locked4X => 4,
            Self::Locked5X => 5,
            Self::Locked6X => 6,
        }
    }

    /// Return the lock period multiplier for this conviction level.
    ///
    /// `None` returns 0 (no lock). Other levels return `2^(conviction - 1)`.
    #[must_use]
    pub fn lock_periods(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Locked1X => 1,
            Self::Locked2X => 2,
            Self::Locked3X => 4,
            Self::Locked4X => 8,
            Self::Locked5X => 16,
            Self::Locked6X => 32,
        }
    }
}

impl std::fmt::Display for Conviction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::None => "none",
            Self::Locked1X => "locked_1x",
            Self::Locked2X => "locked_2x",
            Self::Locked3X => "locked_3x",
            Self::Locked4X => "locked_4x",
            Self::Locked5X => "locked_5x",
            Self::Locked6X => "locked_6x",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// Vote
// ---------------------------------------------------------------------------

/// A single vote cast by an account on a referendum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    /// The SS58-encoded account that cast the vote.
    pub account: String,
    /// The index of the referendum voted on.
    pub referendum_index: u32,
    /// The conviction level applied to the vote.
    pub conviction: Conviction,
    /// The balance locked for this vote (in planck units).
    pub balance: u128,
    /// The direction of the vote.
    pub direction: VoteDirection,
}

// ---------------------------------------------------------------------------
// Delegation
// ---------------------------------------------------------------------------

/// A delegation of voting power from one account to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delegation {
    /// The account delegating their voting power.
    pub delegator: String,
    /// The account receiving delegated voting power.
    pub delegate: String,
    /// The governance track this delegation applies to.
    pub track: u16,
    /// The conviction applied to the delegation.
    pub conviction: Conviction,
    /// The balance delegated (in planck units).
    pub balance: u128,
}

// ---------------------------------------------------------------------------
// TreasuryProposal
// ---------------------------------------------------------------------------

/// A pending treasury spend proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreasuryProposal {
    /// Proposal index.
    pub index: u32,
    /// The account proposing the spend.
    pub proposer: String,
    /// The beneficiary of the spend.
    pub beneficiary: String,
    /// The requested amount in planck units.
    pub value: u128,
}

// ---------------------------------------------------------------------------
// TreasuryInfo
// ---------------------------------------------------------------------------

/// Overview of the on-chain treasury state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreasuryInfo {
    /// Free balance available in the treasury pot (in planck units).
    pub free_balance: u128,
    /// Pending treasury spend proposals.
    pub pending_proposals: Vec<TreasuryProposal>,
    /// Block number of the next spend period.
    pub next_spend_period: u64,
    /// Number of approved but not yet paid proposals.
    pub approved_count: u32,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referendum_status_display() {
        assert_eq!(ReferendumStatus::Preparing.to_string(), "preparing");
        assert_eq!(ReferendumStatus::Deciding.to_string(), "deciding");
        assert_eq!(ReferendumStatus::Confirming.to_string(), "confirming");
        assert_eq!(ReferendumStatus::Approved.to_string(), "approved");
        assert_eq!(ReferendumStatus::Rejected.to_string(), "rejected");
        assert_eq!(ReferendumStatus::Cancelled.to_string(), "cancelled");
        assert_eq!(ReferendumStatus::TimedOut.to_string(), "timed_out");
        assert_eq!(ReferendumStatus::Killed.to_string(), "killed");
    }

    #[test]
    fn referendum_status_serde_round_trip() {
        let status = ReferendumStatus::Confirming;
        let json = serde_json::to_string(&status).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json, r#""confirming""#);
        let back: ReferendumStatus = serde_json::from_str(&json).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back, status);
    }

    #[test]
    fn vote_direction_display() {
        assert_eq!(VoteDirection::Aye.to_string(), "aye");
        assert_eq!(VoteDirection::Nay.to_string(), "nay");
        assert_eq!(VoteDirection::Abstain.to_string(), "abstain");
        assert_eq!(VoteDirection::Split.to_string(), "split");
    }

    #[test]
    fn vote_direction_serde_round_trip() {
        let dir = VoteDirection::Abstain;
        let json = serde_json::to_string(&dir).unwrap_or_else(|e| panic!("{e}"));
        let back: VoteDirection = serde_json::from_str(&json).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back, dir);
    }

    #[test]
    fn conviction_multiplier() {
        assert_eq!(Conviction::None.multiplier(), 0);
        assert_eq!(Conviction::Locked1X.multiplier(), 1);
        assert_eq!(Conviction::Locked6X.multiplier(), 6);
    }

    #[test]
    fn conviction_lock_periods() {
        assert_eq!(Conviction::None.lock_periods(), 0);
        assert_eq!(Conviction::Locked1X.lock_periods(), 1);
        assert_eq!(Conviction::Locked2X.lock_periods(), 2);
        assert_eq!(Conviction::Locked3X.lock_periods(), 4);
        assert_eq!(Conviction::Locked4X.lock_periods(), 8);
        assert_eq!(Conviction::Locked5X.lock_periods(), 16);
        assert_eq!(Conviction::Locked6X.lock_periods(), 32);
    }

    #[test]
    fn conviction_display() {
        assert_eq!(Conviction::None.to_string(), "none");
        assert_eq!(Conviction::Locked3X.to_string(), "locked_3x");
    }

    #[test]
    fn conviction_serde_round_trip() {
        let conv = Conviction::Locked4X;
        let json = serde_json::to_string(&conv).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json, r#""locked_4x""#);
        let back: Conviction = serde_json::from_str(&json).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(back, conv);
    }

    #[test]
    fn referendum_serializes() {
        let r = Referendum {
            index: 100,
            track: 1,
            origin: "Root".to_string(),
            status: ReferendumStatus::Deciding,
            ayes: 1_000_000_000_000,
            nays: 500_000_000_000,
            support: 1_500_000_000_000,
            proposer: "5GrwvaEF...".to_string(),
            timeline: vec![TimelineEvent {
                event: "submitted".to_string(),
                block_number: 10_000,
                timestamp: None,
            }],
        };
        let json = serde_json::to_value(&r).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["index"], 100);
        assert_eq!(json["track"], 1);
        assert_eq!(json["status"], "deciding");
        assert!(json["timeline"].as_array().is_some());
    }

    #[test]
    fn track_serializes() {
        let t = Track {
            id: 0,
            name: "Root".to_string(),
            max_deciding: 1,
            decision_period: 403_200,
            confirm_period: 28_800,
            min_approval_curve: "reciprocal(0.0, 0.5, 0.5)".to_string(),
            min_support_curve: "linear_decreasing(0.5, 0.0)".to_string(),
            prepare_period: 1_200,
            min_deposit: 100_000_000_000_000,
        };
        let json = serde_json::to_value(&t).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["id"], 0);
        assert_eq!(json["name"], "Root");
        assert_eq!(json["max_deciding"], 1);
    }

    #[test]
    fn vote_serializes() {
        let v = Vote {
            account: "5GrwvaEF...".to_string(),
            referendum_index: 42,
            conviction: Conviction::Locked3X,
            balance: 10_000_000_000_000,
            direction: VoteDirection::Aye,
        };
        let json = serde_json::to_value(&v).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["referendum_index"], 42);
        assert_eq!(json["conviction"], "locked_3x");
        assert_eq!(json["direction"], "aye");
    }

    #[test]
    fn delegation_serializes() {
        let d = Delegation {
            delegator: "5Alice...".to_string(),
            delegate: "5Bob...".to_string(),
            track: 0,
            conviction: Conviction::Locked1X,
            balance: 5_000_000_000_000,
        };
        let json = serde_json::to_value(&d).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["delegator"], "5Alice...");
        assert_eq!(json["delegate"], "5Bob...");
        assert_eq!(json["track"], 0);
    }

    #[test]
    fn treasury_info_serializes() {
        let ti = TreasuryInfo {
            free_balance: 50_000_000_000_000_000,
            pending_proposals: vec![TreasuryProposal {
                index: 0,
                proposer: "5Alice...".to_string(),
                beneficiary: "5Bob...".to_string(),
                value: 1_000_000_000_000,
            }],
            next_spend_period: 1_234_567,
            approved_count: 3,
        };
        let json = serde_json::to_value(&ti).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["approved_count"], 3);
        assert_eq!(json["pending_proposals"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn treasury_info_deserializes() {
        let json = serde_json::json!({
            "free_balance": 100_000_000_000_000_u128,
            "pending_proposals": [],
            "next_spend_period": 999_999_u64,
            "approved_count": 0
        });
        let ti: TreasuryInfo = serde_json::from_value(json).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(ti.free_balance, 100_000_000_000_000);
        assert!(ti.pending_proposals.is_empty());
    }

    #[test]
    fn timeline_event_with_timestamp() {
        let ts = Utc::now();
        let event = TimelineEvent {
            event: "decision_started".to_string(),
            block_number: 20_000,
            timestamp: Some(ts),
        };
        let json = serde_json::to_value(&event).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["event"], "decision_started");
        assert!(json["timestamp"].is_string());
    }

    #[test]
    fn timeline_event_without_timestamp() {
        let event = TimelineEvent {
            event: "submitted".to_string(),
            block_number: 10_000,
            timestamp: None,
        };
        let json = serde_json::to_value(&event).unwrap_or_else(|e| panic!("{e}"));
        assert!(json["timestamp"].is_null());
    }
}
