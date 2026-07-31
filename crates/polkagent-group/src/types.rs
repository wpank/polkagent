//! Domain types for the polkagent-group crate.
//!
//! This module defines the core group vocabulary: [`GroupId`], [`Group`],
//! [`GroupMember`], [`MemberRole`], [`QuorumPolicy`], [`GroupBudget`], and
//! [`GrantSpec`].

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use polkagent_core::ids::AgentId;

// ---------------------------------------------------------------------------
// GroupId
// ---------------------------------------------------------------------------

/// Unique identifier for a [`Group`].
///
/// A newtype over [`Uuid`] using UUID v7 for time-ordered generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GroupId(Uuid);

impl GroupId {
    /// Generate a new time-ordered (v7) group identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing [`Uuid`].
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Return the inner [`Uuid`].
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for GroupId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for GroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for GroupId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for GroupId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<GroupId> for Uuid {
    fn from(id: GroupId) -> Self {
        id.0
    }
}

// ---------------------------------------------------------------------------
// MemberRole
// ---------------------------------------------------------------------------

/// The role an agent plays within a group.
///
/// Roles determine which quorum decisions an agent may participate in and
/// what budget overrides they may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberRole {
    /// The group leader. Can initiate group-level decisions, has broader
    /// authority, and their vote alone satisfies [`QuorumPolicy::LeaderOnly`].
    Leader,

    /// A standard worker agent. Participates in quorum votes and executes
    /// tasks on behalf of the group.
    Worker,

    /// An observer agent. Receives group events and evidence but cannot vote
    /// in quorum decisions or initiate group-level actions.
    Observer,
}

impl MemberRole {
    /// Returns `true` if this role may cast a vote in quorum decisions.
    #[must_use]
    pub fn can_vote(&self) -> bool {
        matches!(self, MemberRole::Leader | MemberRole::Worker)
    }

    /// Returns `true` if this role is the leader.
    #[must_use]
    pub fn is_leader(&self) -> bool {
        matches!(self, MemberRole::Leader)
    }
}

// ---------------------------------------------------------------------------
// GrantSpec
// ---------------------------------------------------------------------------

/// A specification of grant capabilities that can be applied at the group or
/// member level.
///
/// When resolving effective grants, the member-level `GrantSpec` is
/// intersected with the group-level `GrantSpec`: a member can never gain
/// capabilities beyond what the group permits.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GrantSpec {
    /// The capability names this spec permits (e.g. `"chain.transfer"`,
    /// `"model.inference"`). An empty list means no capabilities.
    pub capabilities: Vec<String>,

    /// Maximum budget this spec allows, in the asset's smallest unit.
    /// `None` means the group or parent limit applies.
    pub max_budget: Option<f64>,

    /// The pallets that on-chain interactions may target under this spec.
    /// An empty list means all pallets are allowed (subject to group limits).
    pub allowed_pallets: Vec<String>,
}

impl GrantSpec {
    /// Compute the intersection of `self` and `other`.
    ///
    /// The resulting spec contains only capabilities present in both, the
    /// lower of the two budget limits, and only pallets allowed by both.
    /// An empty `allowed_pallets` in either side is treated as "allow all",
    /// so the intersection follows set-intersection semantics only when
    /// both sides are non-empty.
    #[must_use]
    pub fn intersect(&self, other: &GrantSpec) -> GrantSpec {
        // Capabilities: keep those present in both sets.
        let capabilities: Vec<String> = self
            .capabilities
            .iter()
            .filter(|c| other.capabilities.contains(c))
            .cloned()
            .collect();

        // Budget: take the minimum of the two limits.
        let max_budget = match (self.max_budget, other.max_budget) {
            (Some(a), Some(b)) => Some(f64::min(a, b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };

        // Pallets: if both sides specify a list, take the intersection.
        // If either side is empty (meaning "allow all"), use the other side.
        let allowed_pallets = match (self.allowed_pallets.is_empty(), other.allowed_pallets.is_empty()) {
            (true, true) => Vec::new(),
            (true, false) => other.allowed_pallets.clone(),
            (false, true) => self.allowed_pallets.clone(),
            (false, false) => self
                .allowed_pallets
                .iter()
                .filter(|p| other.allowed_pallets.contains(p))
                .cloned()
                .collect(),
        };

        GrantSpec {
            capabilities,
            max_budget,
            allowed_pallets,
        }
    }

    /// Returns `true` if `capability` is contained in this spec's capability
    /// list.
    #[must_use]
    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }
}

// ---------------------------------------------------------------------------
// GroupMember
// ---------------------------------------------------------------------------

/// A member of a [`Group`].
///
/// Each member carries a role and an optional [`GrantSpec`] that, when
/// present, narrows (intersects with) the group-level grant specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMember {
    /// The agent that holds this membership.
    pub agent_id: AgentId,

    /// The role of this agent within the group.
    pub role: MemberRole,

    /// When this agent joined the group.
    pub joined_at: DateTime<Utc>,

    /// An optional member-specific grant specification.
    ///
    /// When `Some`, the effective grant for this member is the intersection
    /// of the group's base `GrantSpec` and this override. When `None`, the
    /// member inherits the group's full grant spec.
    pub grant_override: Option<GrantSpec>,
}

impl GroupMember {
    /// Create a new member with the given agent ID and role, joining now.
    #[must_use]
    pub fn new(agent_id: AgentId, role: MemberRole) -> Self {
        Self {
            agent_id,
            role,
            joined_at: Utc::now(),
            grant_override: None,
        }
    }

    /// Create a new member with a grant override.
    #[must_use]
    pub fn with_grant(agent_id: AgentId, role: MemberRole, grant: GrantSpec) -> Self {
        Self {
            agent_id,
            role,
            joined_at: Utc::now(),
            grant_override: Some(grant),
        }
    }
}

// ---------------------------------------------------------------------------
// QuorumPolicy
// ---------------------------------------------------------------------------

/// The quorum policy governing group-level decisions.
///
/// Quorum is evaluated against the set of votes cast by voting-eligible
/// members (i.e. those with [`MemberRole::Leader`] or [`MemberRole::Worker`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum QuorumPolicy {
    /// All voting members must approve. A single deny or abstain prevents
    /// the decision from reaching quorum.
    Unanimous,

    /// More than 50% of voting members must approve.
    Majority,

    /// A custom fraction of voting members must approve.
    /// The value must be in the range (0.0, 1.0].
    Threshold {
        /// The fraction of voting members required (e.g. 0.75 for 75%).
        fraction: f64,
    },

    /// Only the group leader's vote is required. All other votes are ignored.
    LeaderOnly,
}

// ---------------------------------------------------------------------------
// GroupBudget
// ---------------------------------------------------------------------------

/// Budget configuration and live accounting for a group.
///
/// Amounts are stored as `u64` representing the asset's smallest indivisible
/// unit (e.g. planck for DOT). The `spent` total is protected by a
/// [`Mutex`] so that concurrent spend recordings are race-free.
#[derive(Debug, Serialize, Deserialize)]
pub struct GroupBudget {
    /// Maximum total spend across all members and all runs.
    pub max_total: u64,

    /// Maximum spend per member across all their runs.
    pub max_per_member: Option<u64>,

    /// Maximum spend in a single run.
    pub max_per_run: Option<u64>,

    /// Accumulated total spend, protected for concurrent access.
    #[serde(skip)]
    pub spent: Arc<Mutex<u64>>,

    /// Per-member spend totals, keyed by agent ID string for serialisation
    /// simplicity.
    pub member_spent: std::collections::HashMap<String, u64>,
}

impl GroupBudget {
    /// Create a new budget with the given limits and zero spend.
    #[must_use]
    pub fn new(max_total: u64, max_per_member: Option<u64>, max_per_run: Option<u64>) -> Self {
        Self {
            max_total,
            max_per_member,
            max_per_run,
            spent: Arc::new(Mutex::new(0)),
            member_spent: std::collections::HashMap::new(),
        }
    }

    /// Return the current total spend.
    #[must_use]
    pub fn total_spent(&self) -> u64 {
        *self.spent.lock()
    }

    /// Return the remaining budget headroom.
    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.max_total.saturating_sub(self.total_spent())
    }

    /// Return `true` if `amount` can be spent without exceeding `max_total`
    /// or `max_per_run`.
    #[must_use]
    pub fn can_spend(&self, amount: u64) -> bool {
        let spent = *self.spent.lock();
        if spent.saturating_add(amount) > self.max_total {
            return false;
        }
        if let Some(per_run) = self.max_per_run {
            if amount > per_run {
                return false;
            }
        }
        true
    }

    /// Return `true` if `amount` can be spent by `agent_id` without
    /// exceeding `max_per_member`.
    #[must_use]
    pub fn member_can_spend(&self, agent_id: &AgentId, amount: u64) -> bool {
        if let Some(max) = self.max_per_member {
            let key = agent_id.to_string();
            let current = self.member_spent.get(&key).copied().unwrap_or(0);
            current.saturating_add(amount) <= max
        } else {
            true
        }
    }
}

impl Clone for GroupBudget {
    fn clone(&self) -> Self {
        let spent_val = *self.spent.lock();
        Self {
            max_total: self.max_total,
            max_per_member: self.max_per_member,
            max_per_run: self.max_per_run,
            spent: Arc::new(Mutex::new(spent_val)),
            member_spent: self.member_spent.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Group
// ---------------------------------------------------------------------------

/// A multi-agent coordination group.
///
/// A group owns a set of [`GroupMember`]s, a [`QuorumPolicy`] that governs
/// group-level decisions, a base [`GrantSpec`] that bounds every member's
/// effective grant, and a [`GroupBudget`] that controls aggregate spend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    /// Stable, globally unique identifier.
    pub id: GroupId,

    /// Human-readable name for the group.
    pub name: String,

    /// Optional description of the group's purpose.
    pub description: String,

    /// The agent that owns and administers this group.
    pub owner_agent_id: AgentId,

    /// All current members of the group.
    pub members: Vec<GroupMember>,

    /// The quorum policy used to evaluate group-level decisions.
    pub quorum_policy: QuorumPolicy,

    /// Budget configuration for the group.
    pub budget: GroupBudget,

    /// When this group was created.
    pub created_at: DateTime<Utc>,

    /// When this group was last modified.
    pub updated_at: DateTime<Utc>,
}

impl Group {
    /// Return the member record for `agent_id`, if present.
    #[must_use]
    pub fn find_member(&self, agent_id: &AgentId) -> Option<&GroupMember> {
        self.members.iter().find(|m| &m.agent_id == agent_id)
    }

    /// Return `true` if `agent_id` is a member of this group.
    #[must_use]
    pub fn is_member(&self, agent_id: &AgentId) -> bool {
        self.find_member(agent_id).is_some()
    }

    /// Return the number of voting-eligible members (Leader + Worker).
    #[must_use]
    pub fn voting_member_count(&self) -> usize {
        self.members.iter().filter(|m| m.role.can_vote()).count()
    }

    /// Return all members whose role is [`MemberRole::Leader`].
    #[must_use]
    pub fn leaders(&self) -> Vec<&GroupMember> {
        self.members.iter().filter(|m| m.role.is_leader()).collect()
    }
}

// ---------------------------------------------------------------------------
// EffectiveGrant
// ---------------------------------------------------------------------------

/// The resolved, effective grant for a specific member within a group.
///
/// This is computed by [`crate::coordinator::GroupCoordinator::resolve_effective_grant`]
/// and represents what the member is actually permitted to do, after
/// intersecting the group-level grant with any member-specific override.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveGrant {
    /// The group this grant is scoped to.
    pub group_id: GroupId,

    /// The member this grant applies to.
    pub agent_id: AgentId,

    /// The resolved, intersected capability specification.
    pub grant_spec: GrantSpec,
}

// ---------------------------------------------------------------------------
// RunResult (used in evidence aggregation)
// ---------------------------------------------------------------------------

/// The outcome of a single agent run within a group context.
///
/// Used by [`crate::propagation::aggregate_evidence`] to combine results
/// from multiple member runs into a [`crate::propagation::GroupEvidence`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    /// The run ID from polkagent-core.
    pub run_id: polkagent_core::ids::RunId,

    /// The agent that produced this result.
    pub agent_id: AgentId,

    /// Whether the run completed successfully.
    pub success: bool,

    /// Artifact IDs produced by this run.
    pub artifact_ids: Vec<polkagent_core::ids::ArtifactId>,

    /// A human-readable summary of the run outcome.
    pub summary: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- GroupId -----------------------------------------------------------

    #[test]
    fn group_id_new_is_unique() {
        let a = GroupId::new();
        let b = GroupId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn group_id_display_and_from_str_round_trip() {
        let id = GroupId::new();
        let s = id.to_string();
        let parsed: GroupId = s.parse().expect("valid UUID string");
        assert_eq!(id, parsed);
    }

    #[test]
    fn group_id_from_str_invalid_returns_error() {
        let result: Result<GroupId, _> = "not-a-uuid".parse();
        assert!(result.is_err());
    }

    #[test]
    fn group_id_serde_transparent() {
        let id = GroupId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        assert!(json.starts_with('"'), "should serialize as plain UUID string");
        let back: GroupId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn group_id_from_uuid_round_trip() {
        let uuid = uuid::Uuid::now_v7();
        let id = GroupId::from_uuid(uuid);
        assert_eq!(id.as_uuid(), uuid);
        let back: uuid::Uuid = id.into();
        assert_eq!(back, uuid);
    }

    // ---- MemberRole --------------------------------------------------------

    #[test]
    fn leader_can_vote_and_is_leader() {
        assert!(MemberRole::Leader.can_vote());
        assert!(MemberRole::Leader.is_leader());
    }

    #[test]
    fn worker_can_vote_but_is_not_leader() {
        assert!(MemberRole::Worker.can_vote());
        assert!(!MemberRole::Worker.is_leader());
    }

    #[test]
    fn observer_cannot_vote_and_is_not_leader() {
        assert!(!MemberRole::Observer.can_vote());
        assert!(!MemberRole::Observer.is_leader());
    }

    // ---- GrantSpec ---------------------------------------------------------

    #[test]
    fn grant_spec_intersect_keeps_common_capabilities() {
        let a = GrantSpec {
            capabilities: vec!["chain.transfer".to_string(), "model.inference".to_string()],
            max_budget: Some(1000.0),
            allowed_pallets: vec![],
        };
        let b = GrantSpec {
            capabilities: vec!["chain.transfer".to_string(), "governance.vote".to_string()],
            max_budget: Some(500.0),
            allowed_pallets: vec![],
        };
        let result = a.intersect(&b);
        assert_eq!(result.capabilities, vec!["chain.transfer"]);
        assert_eq!(result.max_budget, Some(500.0));
    }

    #[test]
    fn grant_spec_intersect_takes_lower_budget() {
        let a = GrantSpec {
            capabilities: vec!["a".to_string()],
            max_budget: Some(2000.0),
            allowed_pallets: vec![],
        };
        let b = GrantSpec {
            capabilities: vec!["a".to_string()],
            max_budget: Some(1000.0),
            allowed_pallets: vec![],
        };
        assert_eq!(a.intersect(&b).max_budget, Some(1000.0));
    }

    #[test]
    fn grant_spec_intersect_none_budget_uses_other() {
        let a = GrantSpec {
            capabilities: vec!["a".to_string()],
            max_budget: None,
            allowed_pallets: vec![],
        };
        let b = GrantSpec {
            capabilities: vec!["a".to_string()],
            max_budget: Some(500.0),
            allowed_pallets: vec![],
        };
        assert_eq!(a.intersect(&b).max_budget, Some(500.0));
    }

    #[test]
    fn grant_spec_intersect_pallets_intersection() {
        let a = GrantSpec {
            capabilities: vec![],
            max_budget: None,
            allowed_pallets: vec!["Balances".to_string(), "Staking".to_string()],
        };
        let b = GrantSpec {
            capabilities: vec![],
            max_budget: None,
            allowed_pallets: vec!["Balances".to_string(), "Governance".to_string()],
        };
        let result = a.intersect(&b);
        assert_eq!(result.allowed_pallets, vec!["Balances"]);
    }

    #[test]
    fn grant_spec_intersect_empty_pallets_means_all_allowed() {
        let a = GrantSpec {
            capabilities: vec![],
            max_budget: None,
            allowed_pallets: vec![],
        };
        let b = GrantSpec {
            capabilities: vec![],
            max_budget: None,
            allowed_pallets: vec!["Balances".to_string()],
        };
        // Empty side means "allow all" so intersection returns b's list.
        let result = a.intersect(&b);
        assert_eq!(result.allowed_pallets, vec!["Balances"]);
    }

    #[test]
    fn grant_spec_has_capability() {
        let spec = GrantSpec {
            capabilities: vec!["chain.transfer".to_string()],
            max_budget: None,
            allowed_pallets: vec![],
        };
        assert!(spec.has_capability("chain.transfer"));
        assert!(!spec.has_capability("governance.vote"));
    }

    // ---- GroupBudget -------------------------------------------------------

    #[test]
    fn group_budget_can_spend_within_total() {
        let budget = GroupBudget::new(1000, None, None);
        assert!(budget.can_spend(500));
        assert!(budget.can_spend(1000));
    }

    #[test]
    fn group_budget_cannot_spend_over_total() {
        let budget = GroupBudget::new(1000, None, None);
        assert!(!budget.can_spend(1001));
    }

    #[test]
    fn group_budget_per_run_limit_enforced() {
        let budget = GroupBudget::new(1000, None, Some(100));
        assert!(budget.can_spend(100));
        assert!(!budget.can_spend(101));
    }

    #[test]
    fn group_budget_remaining_decreases_after_spend() {
        let budget = GroupBudget::new(1000, None, None);
        {
            let mut spent = budget.spent.lock();
            *spent = 300;
        }
        assert_eq!(budget.remaining(), 700);
    }

    #[test]
    fn group_budget_member_can_spend_within_limit() {
        let budget = GroupBudget::new(10_000, Some(500), None);
        let agent = AgentId::new();
        assert!(budget.member_can_spend(&agent, 500));
        assert!(!budget.member_can_spend(&agent, 501));
    }

    // ---- Group helper methods ----------------------------------------------

    #[test]
    fn group_find_member_returns_correct_member() {
        let owner = AgentId::new();
        let worker = AgentId::new();
        let group = Group {
            id: GroupId::new(),
            name: "test".to_string(),
            description: String::new(),
            owner_agent_id: owner,
            members: vec![
                GroupMember::new(owner, MemberRole::Leader),
                GroupMember::new(worker, MemberRole::Worker),
            ],
            quorum_policy: QuorumPolicy::Majority,
            budget: GroupBudget::new(1000, None, None),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        assert!(group.find_member(&owner).is_some());
        assert!(group.find_member(&worker).is_some());
        assert!(group.find_member(&AgentId::new()).is_none());
    }

    #[test]
    fn group_voting_member_count_excludes_observers() {
        let owner = AgentId::new();
        let worker = AgentId::new();
        let observer = AgentId::new();
        let group = Group {
            id: GroupId::new(),
            name: "test".to_string(),
            description: String::new(),
            owner_agent_id: owner,
            members: vec![
                GroupMember::new(owner, MemberRole::Leader),
                GroupMember::new(worker, MemberRole::Worker),
                GroupMember::new(observer, MemberRole::Observer),
            ],
            quorum_policy: QuorumPolicy::Majority,
            budget: GroupBudget::new(1000, None, None),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        assert_eq!(group.voting_member_count(), 2);
        assert_eq!(group.leaders().len(), 1);
    }
}
