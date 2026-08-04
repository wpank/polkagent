//! Group coordination: creation, membership management, grant resolution, and
//! budget accounting for multi-agent groups.
//!
//! The [`GroupCoordinator`] is the primary entry point for all group
//! mutations. It holds a map of groups and provides methods for the full
//! membership lifecycle, effective-grant computation, and spend tracking.

use std::collections::HashMap;

use chrono::Utc;

use polkagent_core::ids::AgentId;

use crate::error::{GroupError, GroupResult};
use crate::types::{
    EffectiveGrant, GrantSpec, Group, GroupBudget, GroupId, GroupMember, MemberRole, QuorumPolicy,
};

// ---------------------------------------------------------------------------
// GroupCoordinator
// ---------------------------------------------------------------------------

/// Coordinates group state: membership, grants, and budget.
///
/// For production use, wrap in `Arc<tokio::sync::RwLock<GroupCoordinator>>`
/// to share across async tasks. For simpler in-process usage the struct can
/// be owned directly.
#[derive(Debug, Default)]
pub struct GroupCoordinator {
    /// All managed groups, keyed by their [`GroupId`].
    groups: HashMap<GroupId, Group>,
}

impl GroupCoordinator {
    /// Create a new, empty coordinator.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // -----------------------------------------------------------------------
    // Group lifecycle
    // -----------------------------------------------------------------------

    /// Create a new group with the given `name` and `owner`.
    ///
    /// The owner is automatically added as a [`MemberRole::Leader`]. The
    /// group is given a default [`QuorumPolicy::Majority`] and a generous
    /// default budget that callers can replace via [`Self::set_budget`].
    pub fn create_group(&mut self, name: &str, owner: AgentId) -> Group {
        let id = GroupId::new();
        let now = Utc::now();
        let owner_member = GroupMember::new(owner, MemberRole::Leader);
        let group = Group {
            id,
            name: name.to_string(),
            description: String::new(),
            owner_agent_id: owner,
            members: vec![owner_member],
            quorum_policy: QuorumPolicy::Majority,
            budget: GroupBudget::new(u64::MAX, None, None),
            created_at: now,
            updated_at: now,
        };
        self.groups.insert(id, group.clone());
        group
    }

    /// Register a pre-built group into the coordinator.
    ///
    /// # Errors
    ///
    /// Returns [`GroupError::AlreadyExists`] if a group with the same ID is
    /// already registered.
    pub fn register_group(&mut self, group: Group) -> GroupResult<()> {
        if self.groups.contains_key(&group.id) {
            return Err(GroupError::AlreadyExists(group.id));
        }
        self.groups.insert(group.id, group);
        Ok(())
    }

    /// Get an immutable reference to a group.
    ///
    /// # Errors
    ///
    /// Returns [`GroupError::NotFound`] if the group does not exist.
    pub fn get_group(&self, group_id: &GroupId) -> GroupResult<&Group> {
        self.groups
            .get(group_id)
            .ok_or(GroupError::NotFound(*group_id))
    }

    /// Get a mutable reference to a group.
    ///
    /// # Errors
    ///
    /// Returns [`GroupError::NotFound`] if the group does not exist.
    pub fn get_group_mut(&mut self, group_id: &GroupId) -> GroupResult<&mut Group> {
        self.groups
            .get_mut(group_id)
            .ok_or(GroupError::NotFound(*group_id))
    }

    /// Replace the budget configuration for a group.
    ///
    /// # Errors
    ///
    /// Returns [`GroupError::NotFound`] if the group does not exist.
    pub fn set_budget(&mut self, group_id: &GroupId, budget: GroupBudget) -> GroupResult<()> {
        let group = self.get_group_mut(group_id)?;
        group.budget = budget;
        group.updated_at = Utc::now();
        Ok(())
    }

    /// Replace the quorum policy for a group.
    ///
    /// # Errors
    ///
    /// Returns [`GroupError::NotFound`] if the group does not exist.
    pub fn set_quorum_policy(
        &mut self,
        group_id: &GroupId,
        policy: QuorumPolicy,
    ) -> GroupResult<()> {
        let group = self.get_group_mut(group_id)?;
        group.quorum_policy = policy;
        group.updated_at = Utc::now();
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Membership
    // -----------------------------------------------------------------------

    /// Add `member` to the group identified by `group_id`.
    ///
    /// # Errors
    ///
    /// - [`GroupError::NotFound`] if the group does not exist.
    /// - [`GroupError::AlreadyExists`] if the agent is already a member
    ///   (wrapped as `Internal` to reuse the error type cleanly).
    pub fn add_member(&mut self, group_id: &GroupId, member: GroupMember) -> GroupResult<()> {
        let group = self.get_group_mut(group_id)?;
        if group.is_member(&member.agent_id) {
            return Err(GroupError::Internal(format!(
                "agent {} is already a member of group {}",
                member.agent_id, group_id
            )));
        }
        group.members.push(member);
        group.updated_at = Utc::now();
        Ok(())
    }

    /// Remove the agent identified by `agent_id` from the group.
    ///
    /// # Errors
    ///
    /// - [`GroupError::NotFound`] if the group does not exist.
    /// - [`GroupError::NotMember`] if the agent is not a member.
    /// - [`GroupError::PermissionDenied`] if the agent is the owner (owners
    ///   cannot be removed via this method).
    pub fn remove_member(&mut self, group_id: &GroupId, agent_id: &AgentId) -> GroupResult<()> {
        let group = self.get_group_mut(group_id)?;

        if &group.owner_agent_id == agent_id {
            return Err(GroupError::PermissionDenied(
                *agent_id,
                *group_id,
                "owner cannot be removed from the group".to_string(),
            ));
        }

        let initial_len = group.members.len();
        group.members.retain(|m| &m.agent_id != agent_id);

        if group.members.len() == initial_len {
            return Err(GroupError::NotMember(*agent_id, *group_id));
        }

        group.updated_at = Utc::now();
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Grant resolution
    // -----------------------------------------------------------------------

    /// Compute the effective grant for `agent_id` within `group_id`.
    ///
    /// The effective grant is the intersection of the group's base
    /// [`GrantSpec`] (stored in `group.quorum_policy` context) and the
    /// member's optional `grant_override`. If the group has no base spec,
    /// the member's override (or an empty spec) is returned as-is.
    ///
    /// In practice the group's base spec is passed as the second argument to
    /// allow callers to supply different base specs without mutating the group.
    ///
    /// # Errors
    ///
    /// - [`GroupError::NotFound`] if the group does not exist.
    /// - [`GroupError::NotMember`] if the agent is not a member of the group.
    pub fn resolve_effective_grant(
        &self,
        group_id: &GroupId,
        agent_id: &AgentId,
    ) -> GroupResult<EffectiveGrant> {
        self.resolve_effective_grant_with_base(group_id, agent_id, &GrantSpec::default())
    }

    /// Compute the effective grant using an explicit `group_base` spec.
    ///
    /// The `group_base` is intersected with the member's `grant_override`
    /// (if any). When the member has no override the `group_base` is
    /// returned verbatim.
    ///
    /// # Errors
    ///
    /// - [`GroupError::NotFound`] if the group does not exist.
    /// - [`GroupError::NotMember`] if the agent is not a member.
    pub fn resolve_effective_grant_with_base(
        &self,
        group_id: &GroupId,
        agent_id: &AgentId,
        group_base: &GrantSpec,
    ) -> GroupResult<EffectiveGrant> {
        let group = self.get_group(group_id)?;
        let member = group
            .find_member(agent_id)
            .ok_or(GroupError::NotMember(*agent_id, *group_id))?;

        let grant_spec = match &member.grant_override {
            Some(override_spec) => group_base.intersect(override_spec),
            None => group_base.clone(),
        };

        Ok(EffectiveGrant {
            group_id: *group_id,
            agent_id: *agent_id,
            grant_spec,
        })
    }

    // -----------------------------------------------------------------------
    // Budget
    // -----------------------------------------------------------------------

    /// Check whether the group can spend `amount` without exceeding its
    /// configured limits.
    ///
    /// Returns `Ok(true)` if within budget, `Ok(false)` if the ceiling would
    /// be exceeded.
    ///
    /// # Errors
    ///
    /// Returns [`GroupError::NotFound`] if the group does not exist.
    pub fn check_budget(&self, group_id: &GroupId, amount: f64) -> GroupResult<bool> {
        let group = self.get_group(group_id)?;
        // Convert f64 to u64 safely: clamp negatives to 0, round.
        let amount_u64 = if amount < 0.0 {
            0
        } else {
            // Use saturating cast: f64::MAX > u64::MAX, so clamp.
            if amount > u64::MAX as f64 {
                return Ok(false);
            }
            amount as u64
        };
        Ok(group.budget.can_spend(amount_u64))
    }

    /// Check whether `agent_id` in `group_id` can spend `amount` within
    /// their per-member limit.
    ///
    /// Returns `Ok(true)` if within the member's budget, `Ok(false)` if not.
    ///
    /// # Errors
    ///
    /// - [`GroupError::NotFound`] if the group does not exist.
    /// - [`GroupError::NotMember`] if the agent is not a member.
    pub fn check_member_budget(
        &self,
        group_id: &GroupId,
        agent_id: &AgentId,
        amount: f64,
    ) -> GroupResult<bool> {
        let group = self.get_group(group_id)?;
        if !group.is_member(agent_id) {
            return Err(GroupError::NotMember(*agent_id, *group_id));
        }
        let amount_u64 = if amount < 0.0 {
            0
        } else if amount > u64::MAX as f64 {
            return Ok(false);
        } else {
            amount as u64
        };
        Ok(group.budget.member_can_spend(agent_id, amount_u64))
    }

    /// Record a spend of `amount` by `agent_id` in `group_id`.
    ///
    /// Updates both the group total and the member's per-agent total.
    /// The spend is validated against group and member limits before being
    /// recorded; if either limit would be exceeded the spend is rejected.
    ///
    /// # Errors
    ///
    /// - [`GroupError::NotFound`] if the group does not exist.
    /// - [`GroupError::NotMember`] if the agent is not a member.
    /// - [`GroupError::BudgetExceeded`] if the spend would exceed the group
    ///   total or the per-member limit.
    pub fn record_spend(
        &mut self,
        group_id: &GroupId,
        agent_id: &AgentId,
        amount: f64,
    ) -> GroupResult<()> {
        let amount_u64 = if amount < 0.0 {
            0
        } else if amount > u64::MAX as f64 {
            return Err(GroupError::BudgetExceeded(
                *group_id,
                format!("amount {amount} overflows u64"),
            ));
        } else {
            amount as u64
        };

        // Validate membership.
        {
            let group = self.get_group(group_id)?;
            if !group.is_member(agent_id) {
                return Err(GroupError::NotMember(*agent_id, *group_id));
            }

            // Check group total.
            if !group.budget.can_spend(amount_u64) {
                return Err(GroupError::BudgetExceeded(
                    *group_id,
                    format!(
                        "requested {} exceeds remaining {} (total limit {})",
                        amount_u64,
                        group.budget.remaining(),
                        group.budget.max_total
                    ),
                ));
            }

            // Check per-member limit.
            if !group.budget.member_can_spend(agent_id, amount_u64) {
                return Err(GroupError::BudgetExceeded(
                    *group_id,
                    format!(
                        "per-member limit exceeded for agent {agent_id}: requested {amount_u64}"
                    ),
                ));
            }
        }

        // Apply the spend.
        let group = self.get_group_mut(group_id)?;

        // Update the atomic total.
        let mut spent = group.budget.spent.lock();
        *spent = spent.saturating_add(amount_u64);
        drop(spent);

        // Update the per-member map.
        let key = agent_id.to_string();
        let member_entry = group.budget.member_spent.entry(key).or_insert(0);
        *member_entry = member_entry.saturating_add(amount_u64);

        group.updated_at = Utc::now();
        Ok(())
    }

    /// List all group IDs managed by this coordinator.
    #[must_use]
    pub fn list_group_ids(&self) -> Vec<GroupId> {
        self.groups.keys().copied().collect()
    }

    /// Remove a group from the coordinator.
    ///
    /// # Errors
    ///
    /// Returns [`GroupError::NotFound`] if the group does not exist.
    pub fn delete_group(&mut self, group_id: &GroupId) -> GroupResult<Group> {
        self.groups
            .remove(group_id)
            .ok_or(GroupError::NotFound(*group_id))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{GrantSpec, GroupBudget, MemberRole, QuorumPolicy};

    fn make_coordinator_with_group() -> (GroupCoordinator, GroupId, AgentId) {
        let mut coord = GroupCoordinator::new();
        let owner = AgentId::new();
        let group = coord.create_group("test-group", owner);
        (coord, group.id, owner)
    }

    // ---- Group creation ----------------------------------------------------

    #[test]
    fn create_group_adds_owner_as_leader() {
        let mut coord = GroupCoordinator::new();
        let owner = AgentId::new();
        let group = coord.create_group("alpha", owner);

        assert_eq!(group.name, "alpha");
        assert_eq!(group.owner_agent_id, owner);
        assert_eq!(group.members.len(), 1);
        assert_eq!(group.members[0].agent_id, owner);
        assert!(matches!(group.members[0].role, MemberRole::Leader));
    }

    #[test]
    fn create_group_default_quorum_is_majority() {
        let mut coord = GroupCoordinator::new();
        let owner = AgentId::new();
        let group = coord.create_group("beta", owner);
        assert!(matches!(group.quorum_policy, QuorumPolicy::Majority));
    }

    #[test]
    fn get_group_returns_created_group() {
        let (coord, group_id, _) = make_coordinator_with_group();
        let g = coord.get_group(&group_id).expect("group should exist");
        assert_eq!(g.id, group_id);
    }

    #[test]
    fn get_group_missing_returns_not_found() {
        let coord = GroupCoordinator::new();
        let missing = GroupId::new();
        assert!(matches!(
            coord.get_group(&missing),
            Err(GroupError::NotFound(_))
        ));
    }

    #[test]
    fn delete_group_removes_it() {
        let (mut coord, group_id, _) = make_coordinator_with_group();
        coord.delete_group(&group_id).expect("should delete");
        assert!(matches!(
            coord.get_group(&group_id),
            Err(GroupError::NotFound(_))
        ));
    }

    #[test]
    fn register_group_duplicate_returns_already_exists() {
        let (mut coord, group_id, owner) = make_coordinator_with_group();
        let dup = Group {
            id: group_id,
            name: "dup".to_string(),
            description: String::new(),
            owner_agent_id: owner,
            members: vec![],
            quorum_policy: QuorumPolicy::Majority,
            budget: GroupBudget::new(100, None, None),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        assert!(matches!(
            coord.register_group(dup),
            Err(GroupError::AlreadyExists(_))
        ));
    }

    // ---- Membership --------------------------------------------------------

    #[test]
    fn add_member_succeeds_for_new_agent() {
        let (mut coord, group_id, _) = make_coordinator_with_group();
        let worker = AgentId::new();
        coord
            .add_member(&group_id, GroupMember::new(worker, MemberRole::Worker))
            .expect("add should succeed");
        let group = coord.get_group(&group_id).expect("group exists");
        assert_eq!(group.members.len(), 2);
    }

    #[test]
    fn add_member_duplicate_returns_internal_error() {
        let (mut coord, group_id, owner) = make_coordinator_with_group();
        let result = coord.add_member(&group_id, GroupMember::new(owner, MemberRole::Worker));
        assert!(result.is_err(), "adding duplicate member should fail");
    }

    #[test]
    fn remove_member_succeeds_for_existing_non_owner() {
        let (mut coord, group_id, _) = make_coordinator_with_group();
        let worker = AgentId::new();
        coord
            .add_member(&group_id, GroupMember::new(worker, MemberRole::Worker))
            .expect("add ok");
        coord.remove_member(&group_id, &worker).expect("remove ok");
        let group = coord.get_group(&group_id).expect("exists");
        assert_eq!(group.members.len(), 1);
    }

    #[test]
    fn remove_owner_returns_permission_denied() {
        let (mut coord, group_id, owner) = make_coordinator_with_group();
        assert!(matches!(
            coord.remove_member(&group_id, &owner),
            Err(GroupError::PermissionDenied(..))
        ));
    }

    #[test]
    fn remove_non_member_returns_not_member() {
        let (mut coord, group_id, _) = make_coordinator_with_group();
        let stranger = AgentId::new();
        assert!(matches!(
            coord.remove_member(&group_id, &stranger),
            Err(GroupError::NotMember(..))
        ));
    }

    // ---- Grant resolution --------------------------------------------------

    #[test]
    fn resolve_effective_grant_inherits_group_base_when_no_override() {
        let (coord, group_id, owner) = make_coordinator_with_group();
        let base = GrantSpec {
            capabilities: vec!["chain.transfer".to_string()],
            max_budget: Some(1000.0),
            allowed_pallets: vec!["Balances".to_string()],
        };
        let eg = coord
            .resolve_effective_grant_with_base(&group_id, &owner, &base)
            .expect("resolve ok");
        assert_eq!(eg.grant_spec.capabilities, vec!["chain.transfer"]);
        assert_eq!(eg.grant_spec.max_budget, Some(1000.0));
    }

    #[test]
    fn resolve_effective_grant_member_cannot_exceed_group_capabilities() {
        let mut coord = GroupCoordinator::new();
        let owner = AgentId::new();
        let group = coord.create_group("g", owner);
        let group_id = group.id;

        let worker = AgentId::new();
        let member_grant = GrantSpec {
            capabilities: vec!["chain.transfer".to_string(), "governance.vote".to_string()],
            max_budget: None,
            allowed_pallets: vec![],
        };
        coord
            .add_member(
                &group_id,
                GroupMember::with_grant(worker, MemberRole::Worker, member_grant),
            )
            .expect("add ok");

        // Group base only allows chain.transfer
        let group_base = GrantSpec {
            capabilities: vec!["chain.transfer".to_string()],
            max_budget: None,
            allowed_pallets: vec![],
        };
        let eg = coord
            .resolve_effective_grant_with_base(&group_id, &worker, &group_base)
            .expect("resolve ok");
        // governance.vote is not in group base so should be excluded
        assert_eq!(eg.grant_spec.capabilities, vec!["chain.transfer"]);
        assert!(!eg.grant_spec.has_capability("governance.vote"));
    }

    #[test]
    fn resolve_effective_grant_for_non_member_returns_not_member() {
        let (coord, group_id, _) = make_coordinator_with_group();
        let stranger = AgentId::new();
        assert!(matches!(
            coord.resolve_effective_grant(&group_id, &stranger),
            Err(GroupError::NotMember(..))
        ));
    }

    #[test]
    fn resolve_effective_grant_member_budget_narrowed_by_group() {
        let mut coord = GroupCoordinator::new();
        let owner = AgentId::new();
        let group = coord.create_group("g", owner);
        let group_id = group.id;

        let worker = AgentId::new();
        let member_grant = GrantSpec {
            capabilities: vec!["chain.transfer".to_string()],
            max_budget: Some(9999.0), // higher than group allows
            allowed_pallets: vec![],
        };
        coord
            .add_member(
                &group_id,
                GroupMember::with_grant(worker, MemberRole::Worker, member_grant),
            )
            .expect("add ok");

        let group_base = GrantSpec {
            capabilities: vec!["chain.transfer".to_string()],
            max_budget: Some(500.0), // group caps at 500
            allowed_pallets: vec![],
        };
        let eg = coord
            .resolve_effective_grant_with_base(&group_id, &worker, &group_base)
            .expect("resolve ok");
        // Member's 9999 is narrowed to group's 500
        assert_eq!(eg.grant_spec.max_budget, Some(500.0));
    }

    // ---- Budget ------------------------------------------------------------

    #[test]
    fn check_budget_within_limit_returns_true() {
        let (mut coord, group_id, _) = make_coordinator_with_group();
        coord
            .set_budget(&group_id, GroupBudget::new(1000, None, None))
            .expect("set ok");
        assert!(coord.check_budget(&group_id, 500.0).expect("check ok"));
    }

    #[test]
    fn check_budget_over_limit_returns_false() {
        let (mut coord, group_id, _) = make_coordinator_with_group();
        coord
            .set_budget(&group_id, GroupBudget::new(1000, None, None))
            .expect("set ok");
        assert!(!coord.check_budget(&group_id, 1001.0).expect("check ok"));
    }

    #[test]
    fn record_spend_succeeds_within_budget() {
        let (mut coord, group_id, owner) = make_coordinator_with_group();
        coord
            .set_budget(&group_id, GroupBudget::new(1000, None, None))
            .expect("set ok");
        coord
            .record_spend(&group_id, &owner, 300.0)
            .expect("spend ok");
        let group = coord.get_group(&group_id).expect("exists");
        assert_eq!(group.budget.total_spent(), 300);
    }

    #[test]
    fn record_spend_over_total_budget_returns_budget_exceeded() {
        let (mut coord, group_id, owner) = make_coordinator_with_group();
        coord
            .set_budget(&group_id, GroupBudget::new(100, None, None))
            .expect("set ok");
        assert!(matches!(
            coord.record_spend(&group_id, &owner, 101.0),
            Err(GroupError::BudgetExceeded(..))
        ));
        // Total should remain at 0 — spend was not applied.
        let group = coord.get_group(&group_id).expect("exists");
        assert_eq!(group.budget.total_spent(), 0);
    }

    #[test]
    fn record_spend_over_per_member_limit_returns_budget_exceeded() {
        let (mut coord, group_id, owner) = make_coordinator_with_group();
        coord
            .set_budget(&group_id, GroupBudget::new(10_000, Some(50), None))
            .expect("set ok");
        assert!(matches!(
            coord.record_spend(&group_id, &owner, 51.0),
            Err(GroupError::BudgetExceeded(..))
        ));
    }

    #[test]
    fn record_spend_accumulates_per_member() {
        let (mut coord, group_id, owner) = make_coordinator_with_group();
        let worker = AgentId::new();
        coord
            .add_member(&group_id, GroupMember::new(worker, MemberRole::Worker))
            .expect("add ok");
        coord
            .set_budget(&group_id, GroupBudget::new(1000, Some(300), None))
            .expect("set ok");

        coord.record_spend(&group_id, &owner, 100.0).expect("ok");
        coord.record_spend(&group_id, &worker, 200.0).expect("ok");

        let group = coord.get_group(&group_id).expect("exists");
        assert_eq!(group.budget.total_spent(), 300);
        let owner_spent = group
            .budget
            .member_spent
            .get(&owner.to_string())
            .copied()
            .unwrap_or(0);
        assert_eq!(owner_spent, 100);
        let worker_spent = group
            .budget
            .member_spent
            .get(&worker.to_string())
            .copied()
            .unwrap_or(0);
        assert_eq!(worker_spent, 200);
    }

    #[test]
    fn record_spend_for_non_member_returns_not_member() {
        let (mut coord, group_id, _) = make_coordinator_with_group();
        coord
            .set_budget(&group_id, GroupBudget::new(1000, None, None))
            .expect("set ok");
        let stranger = AgentId::new();
        assert!(matches!(
            coord.record_spend(&group_id, &stranger, 10.0),
            Err(GroupError::NotMember(..))
        ));
    }

    #[test]
    fn set_quorum_policy_updates_group() {
        let (mut coord, group_id, _) = make_coordinator_with_group();
        coord
            .set_quorum_policy(&group_id, QuorumPolicy::Unanimous)
            .expect("set ok");
        let group = coord.get_group(&group_id).expect("exists");
        assert!(matches!(group.quorum_policy, QuorumPolicy::Unanimous));
    }

    #[test]
    fn list_group_ids_returns_all_registered_groups() {
        let mut coord = GroupCoordinator::new();
        let owner = AgentId::new();
        coord.create_group("a", owner);
        coord.create_group("b", owner);
        coord.create_group("c", owner);
        assert_eq!(coord.list_group_ids().len(), 3);
    }
}
