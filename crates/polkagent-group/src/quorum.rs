//! Quorum evaluation for group-level decisions.
//!
//! This module implements the [`check_quorum`] function and the supporting
//! [`Vote`] and [`QuorumResult`] types.  Quorum is used to gate high-value
//! group actions (e.g. approving a large transaction, changing group policy)
//! that require collective agreement from group members.
//!
//! # Voting model
//!
//! - Only members with [`MemberRole::Leader`] or [`MemberRole::Worker`] may
//!   cast binding votes.
//! - Observers are ignored in quorum calculations.
//! - The `total_members` parameter reflects the total count of *voting-eligible*
//!   members (i.e. non-observer members), not the full member list.
//! - Absent (non-casting) members are counted as neither approve nor deny —
//!   some policies (e.g. Unanimous) treat them as blocking.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use polkagent_core::ids::AgentId;

use crate::types::{MemberRole, QuorumPolicy};

// ---------------------------------------------------------------------------
// Vote
// ---------------------------------------------------------------------------

/// A vote cast by a group member on a group-level decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    /// The agent that cast this vote.
    pub agent_id: AgentId,

    /// The decision the agent made.
    pub decision: VoteDecision,

    /// The role of the voting agent at the time of casting.
    pub role: MemberRole,

    /// When this vote was cast.
    pub timestamp: DateTime<Utc>,
}

impl Vote {
    /// Create a new vote for the current time.
    #[must_use]
    pub fn new(agent_id: AgentId, decision: VoteDecision, role: MemberRole) -> Self {
        Self {
            agent_id,
            decision,
            role,
            timestamp: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// VoteDecision
// ---------------------------------------------------------------------------

/// The decision component of a [`Vote`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoteDecision {
    /// The agent approves the decision.
    Approve,

    /// The agent explicitly denies the decision.
    Deny,

    /// The agent neither approves nor denies (neutral).
    Abstain,
}

// ---------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------

/// The final outcome of a quorum evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    /// The decision is approved.
    Approved,

    /// The decision is denied.
    Denied,
}

// ---------------------------------------------------------------------------
// QuorumResult
// ---------------------------------------------------------------------------

/// The result of evaluating a [`QuorumPolicy`] against a set of votes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum QuorumResult {
    /// Quorum has been reached and a final decision produced.
    Reached {
        /// The final decision.
        decision: Decision,
    },

    /// Quorum has not yet been reached. More votes are needed.
    Pending {
        /// The number of additional approvals needed to reach quorum
        /// (assuming no further denials).
        needed: usize,
    },

    /// Quorum has irrevocably failed (e.g. a deny in an unanimous policy,
    /// or a deny from the leader in a `LeaderOnly` policy).
    Failed(String),
}

// ---------------------------------------------------------------------------
// check_quorum
// ---------------------------------------------------------------------------

/// Evaluate `policy` against the provided `votes` given `total_members` total
/// voting-eligible members.
///
/// # Arguments
///
/// * `policy` — the [`QuorumPolicy`] to evaluate.
/// * `votes` — the votes cast so far. Only votes from voting-eligible members
///   (Leader or Worker roles) are considered. Observer votes are ignored.
/// * `total_members` — the total count of voting-eligible members in the
///   group. This is used to determine whether additional votes are possible.
///
/// # Returns
///
/// A [`QuorumResult`] indicating whether quorum is reached, pending, or
/// failed.
pub fn check_quorum(
    policy: &QuorumPolicy,
    votes: &[Vote],
    total_members: usize,
) -> QuorumResult {
    // Filter to only voting-eligible votes.
    let eligible_votes: Vec<&Vote> = votes
        .iter()
        .filter(|v| v.role.can_vote())
        .collect();

    let approve_count = eligible_votes
        .iter()
        .filter(|v| matches!(v.decision, VoteDecision::Approve))
        .count();

    let deny_count = eligible_votes
        .iter()
        .filter(|v| matches!(v.decision, VoteDecision::Deny))
        .count();

    let cast_count = eligible_votes.len();
    // Votes not yet cast by eligible members.
    let uncast = total_members.saturating_sub(cast_count);

    if total_members == 0 {
        // Edge case: no voting members — quorum cannot be reached.
        return QuorumResult::Failed("no voting members in group".to_string());
    }

    match policy {
        QuorumPolicy::Unanimous => {
            // All voting members must approve.
            if deny_count > 0 {
                return QuorumResult::Failed(format!(
                    "{deny_count} deny vote(s) prevent unanimous quorum"
                ));
            }
            // Check for any abstains blocking unanimous.
            let abstain_count = eligible_votes
                .iter()
                .filter(|v| matches!(v.decision, VoteDecision::Abstain))
                .count();
            if abstain_count > 0 {
                return QuorumResult::Failed(format!(
                    "{abstain_count} abstain vote(s) prevent unanimous quorum"
                ));
            }
            if approve_count >= total_members {
                QuorumResult::Reached {
                    decision: Decision::Approved,
                }
            } else {
                let needed = total_members.saturating_sub(approve_count);
                QuorumResult::Pending { needed }
            }
        }

        QuorumPolicy::Majority => {
            // Strict majority: more than half of all voting members must approve.
            let threshold = total_members / 2 + 1;

            // If denials make it mathematically impossible to reach majority,
            // fail early.
            let max_possible_approvals = approve_count + uncast;
            if max_possible_approvals < threshold {
                return QuorumResult::Failed(format!(
                    "not enough remaining votes to reach majority: need {threshold}, can reach at most {max_possible_approvals}"
                ));
            }

            if approve_count >= threshold {
                QuorumResult::Reached {
                    decision: Decision::Approved,
                }
            } else {
                let needed = threshold.saturating_sub(approve_count);
                QuorumResult::Pending { needed }
            }
        }

        QuorumPolicy::Threshold { fraction } => {
            // Custom fraction: ceil(total_members * fraction) approvals needed.
            let fraction = fraction.clamp(0.0, 1.0);
            let threshold = f64::ceil(fraction * total_members as f64) as usize;
            let threshold = threshold.max(1); // at least 1

            let max_possible_approvals = approve_count + uncast;
            if max_possible_approvals < threshold {
                return QuorumResult::Failed(format!(
                    "not enough remaining votes to reach threshold {fraction}: need {threshold}, can reach at most {max_possible_approvals}"
                ));
            }

            if approve_count >= threshold {
                QuorumResult::Reached {
                    decision: Decision::Approved,
                }
            } else {
                let needed = threshold.saturating_sub(approve_count);
                QuorumResult::Pending { needed }
            }
        }

        QuorumPolicy::LeaderOnly => {
            // Only the leader's vote matters. A deny from the leader fails
            // immediately; an approve from the leader succeeds immediately.
            let leader_votes: Vec<&Vote> = eligible_votes
                .iter()
                .filter(|v| v.role.is_leader())
                .copied()
                .collect();

            if leader_votes.is_empty() {
                // No leader has voted yet.
                return QuorumResult::Pending { needed: 1 };
            }

            // Use the most recent leader vote (last in the slice).
            let leader_decision = &leader_votes.last().map(|v| v.decision);
            match leader_decision {
                Some(VoteDecision::Approve) => QuorumResult::Reached {
                    decision: Decision::Approved,
                },
                Some(VoteDecision::Deny) => {
                    QuorumResult::Failed("leader denied the decision".to_string())
                }
                Some(VoteDecision::Abstain) => QuorumResult::Pending { needed: 1 },
                None => QuorumResult::Pending { needed: 1 },
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Count the number of approve votes from voting-eligible members in `votes`.
#[must_use]
pub fn count_approvals(votes: &[Vote]) -> usize {
    votes
        .iter()
        .filter(|v| v.role.can_vote() && matches!(v.decision, VoteDecision::Approve))
        .count()
}

/// Count the number of deny votes from voting-eligible members in `votes`.
#[must_use]
pub fn count_denials(votes: &[Vote]) -> usize {
    votes
        .iter()
        .filter(|v| v.role.can_vote() && matches!(v.decision, VoteDecision::Deny))
        .count()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn approve(role: MemberRole) -> Vote {
        Vote::new(AgentId::new(), VoteDecision::Approve, role)
    }

    fn deny(role: MemberRole) -> Vote {
        Vote::new(AgentId::new(), VoteDecision::Deny, role)
    }

    fn abstain(role: MemberRole) -> Vote {
        Vote::new(AgentId::new(), VoteDecision::Abstain, role)
    }

    // ---- Unanimous --------------------------------------------------------

    #[test]
    fn unanimous_all_approve_reaches_quorum() {
        let votes = vec![
            approve(MemberRole::Leader),
            approve(MemberRole::Worker),
            approve(MemberRole::Worker),
        ];
        let result = check_quorum(&QuorumPolicy::Unanimous, &votes, 3);
        assert!(matches!(
            result,
            QuorumResult::Reached { decision: Decision::Approved }
        ));
    }

    #[test]
    fn unanimous_single_deny_fails_quorum() {
        let votes = vec![
            approve(MemberRole::Leader),
            deny(MemberRole::Worker),
            approve(MemberRole::Worker),
        ];
        let result = check_quorum(&QuorumPolicy::Unanimous, &votes, 3);
        assert!(matches!(result, QuorumResult::Failed(_)));
    }

    #[test]
    fn unanimous_abstain_fails_quorum() {
        let votes = vec![
            approve(MemberRole::Leader),
            abstain(MemberRole::Worker),
        ];
        let result = check_quorum(&QuorumPolicy::Unanimous, &votes, 2);
        assert!(matches!(result, QuorumResult::Failed(_)));
    }

    #[test]
    fn unanimous_partial_approvals_returns_pending() {
        let votes = vec![approve(MemberRole::Leader)];
        let result = check_quorum(&QuorumPolicy::Unanimous, &votes, 3);
        assert!(matches!(result, QuorumResult::Pending { needed: 2 }));
    }

    // ---- Majority ---------------------------------------------------------

    #[test]
    fn majority_more_than_half_approve_reaches_quorum() {
        let votes = vec![
            approve(MemberRole::Leader),
            approve(MemberRole::Worker),
            deny(MemberRole::Worker),
        ];
        let result = check_quorum(&QuorumPolicy::Majority, &votes, 3);
        assert!(matches!(
            result,
            QuorumResult::Reached { decision: Decision::Approved }
        ));
    }

    #[test]
    fn majority_exactly_half_not_sufficient() {
        // 2 of 4: 50% is not >50%, so we need 3.
        let votes = vec![
            approve(MemberRole::Leader),
            approve(MemberRole::Worker),
            deny(MemberRole::Worker),
            deny(MemberRole::Worker),
        ];
        // 2 approvals, 2 denials, threshold = 4/2+1 = 3, can't reach 3 with 0 uncast.
        let result = check_quorum(&QuorumPolicy::Majority, &votes, 4);
        assert!(matches!(result, QuorumResult::Failed(_)));
    }

    #[test]
    fn majority_pending_when_more_votes_possible() {
        // 1 of 4 cast approve, 3 uncast — can still reach majority.
        let votes = vec![approve(MemberRole::Leader)];
        let result = check_quorum(&QuorumPolicy::Majority, &votes, 4);
        // threshold = 3, approve_count = 1, needed = 2
        assert!(matches!(result, QuorumResult::Pending { needed: 2 }));
    }

    #[test]
    fn majority_fails_when_impossible_to_reach_threshold() {
        // 2 denials, 1 approval, 0 uncast (3 total members). Threshold=2, max possible=1+0=1.
        let votes = vec![
            approve(MemberRole::Worker),
            deny(MemberRole::Leader),
            deny(MemberRole::Worker),
        ];
        let result = check_quorum(&QuorumPolicy::Majority, &votes, 3);
        assert!(matches!(result, QuorumResult::Failed(_)));
    }

    // ---- Threshold --------------------------------------------------------

    #[test]
    fn threshold_75_percent_with_three_of_four_approvals() {
        let votes = vec![
            approve(MemberRole::Leader),
            approve(MemberRole::Worker),
            approve(MemberRole::Worker),
            deny(MemberRole::Worker),
        ];
        let result = check_quorum(
            &QuorumPolicy::Threshold { fraction: 0.75 },
            &votes,
            4,
        );
        // ceil(4 * 0.75) = 3, 3 approvals => reached
        assert!(matches!(
            result,
            QuorumResult::Reached { decision: Decision::Approved }
        ));
    }

    #[test]
    fn threshold_custom_pending_when_not_enough_yet() {
        let votes = vec![approve(MemberRole::Leader)];
        let result = check_quorum(
            &QuorumPolicy::Threshold { fraction: 0.5 },
            &votes,
            4,
        );
        // ceil(4 * 0.5) = 2, 1 approval => pending, needed = 1
        assert!(matches!(result, QuorumResult::Pending { needed: 1 }));
    }

    #[test]
    fn threshold_fails_when_cannot_reach_target() {
        let votes = vec![
            deny(MemberRole::Leader),
            deny(MemberRole::Worker),
            deny(MemberRole::Worker),
        ];
        let result = check_quorum(
            &QuorumPolicy::Threshold { fraction: 0.75 },
            &votes,
            3,
        );
        // ceil(3*0.75) = 3, 0 approvals, 0 uncast → can't reach 3
        assert!(matches!(result, QuorumResult::Failed(_)));
    }

    // ---- LeaderOnly -------------------------------------------------------

    #[test]
    fn leader_only_leader_approve_reaches_quorum() {
        let votes = vec![
            approve(MemberRole::Leader),
            deny(MemberRole::Worker), // worker deny irrelevant
        ];
        let result = check_quorum(&QuorumPolicy::LeaderOnly, &votes, 2);
        assert!(matches!(
            result,
            QuorumResult::Reached { decision: Decision::Approved }
        ));
    }

    #[test]
    fn leader_only_leader_deny_fails_quorum() {
        let votes = vec![
            deny(MemberRole::Leader),
            approve(MemberRole::Worker),
        ];
        let result = check_quorum(&QuorumPolicy::LeaderOnly, &votes, 2);
        assert!(matches!(result, QuorumResult::Failed(_)));
    }

    #[test]
    fn leader_only_no_leader_vote_returns_pending() {
        let votes = vec![approve(MemberRole::Worker)];
        let result = check_quorum(&QuorumPolicy::LeaderOnly, &votes, 2);
        assert!(matches!(result, QuorumResult::Pending { needed: 1 }));
    }

    #[test]
    fn leader_only_leader_abstain_returns_pending() {
        let votes = vec![abstain(MemberRole::Leader)];
        let result = check_quorum(&QuorumPolicy::LeaderOnly, &votes, 2);
        assert!(matches!(result, QuorumResult::Pending { needed: 1 }));
    }

    // ---- Observer votes are ignored ---------------------------------------

    #[test]
    fn observer_vote_is_ignored_in_unanimous() {
        // 2 voting members, 1 observer — observer deny should not fail quorum.
        let votes = vec![
            approve(MemberRole::Leader),
            approve(MemberRole::Worker),
            deny(MemberRole::Observer), // should be ignored
        ];
        let result = check_quorum(&QuorumPolicy::Unanimous, &votes, 2);
        assert!(matches!(
            result,
            QuorumResult::Reached { decision: Decision::Approved }
        ));
    }

    // ---- Empty group -------------------------------------------------------

    #[test]
    fn empty_group_quorum_fails() {
        let result = check_quorum(&QuorumPolicy::Unanimous, &[], 0);
        assert!(matches!(result, QuorumResult::Failed(_)));
    }

    #[test]
    fn empty_group_majority_fails() {
        let result = check_quorum(&QuorumPolicy::Majority, &[], 0);
        assert!(matches!(result, QuorumResult::Failed(_)));
    }

    // ---- count helpers ----------------------------------------------------

    #[test]
    fn count_approvals_counts_only_eligible_approvals() {
        let votes = vec![
            approve(MemberRole::Leader),
            approve(MemberRole::Worker),
            deny(MemberRole::Worker),
            approve(MemberRole::Observer), // observer — excluded
        ];
        assert_eq!(count_approvals(&votes), 2);
    }

    #[test]
    fn count_denials_counts_only_eligible_denials() {
        let votes = vec![
            deny(MemberRole::Leader),
            deny(MemberRole::Worker),
            approve(MemberRole::Worker),
            deny(MemberRole::Observer), // observer — excluded
        ];
        assert_eq!(count_denials(&votes), 2);
    }
}
