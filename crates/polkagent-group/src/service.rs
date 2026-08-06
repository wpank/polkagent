//! Durable, surface-neutral application service for group definitions.
//!
//! [`GroupService`] validates domain invariants and delegates every read and
//! mutation to one supplied [`GroupStore`]. It deliberately keeps no in-memory
//! group map, so a database-backed store remains the authoritative state across
//! process restarts.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use polkagent_core::ids::AgentId;

use crate::error::{GroupError, GroupResult};
use crate::store::GroupStore;
use crate::types::{GrantSpec, Group, GroupBudget, GroupId, GroupMember, MemberRole, QuorumPolicy};

/// The mutable policy limits accepted by [`GroupService::update_policy`].
///
/// Spend accounting is intentionally absent. Policy updates retain the
/// authoritative total and per-member spend already stored for the group.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupPolicyUpdate {
    /// Quorum rule for group decisions.
    pub quorum_policy: QuorumPolicy,
    /// Maximum aggregate spend.
    pub max_total: u64,
    /// Optional maximum aggregate spend for one member.
    pub max_per_member: Option<u64>,
    /// Optional maximum spend for one run.
    pub max_per_run: Option<u64>,
}

/// Async application service for durable group definition management.
///
/// Clones share only the supplied store. No coordinator or secondary cache is
/// created, so reads after mutation always come from the authoritative store.
#[derive(Clone)]
pub struct GroupService {
    store: Arc<dyn GroupStore>,
}

impl GroupService {
    /// Build a service over one authoritative group store.
    #[must_use]
    pub fn new(store: Arc<dyn GroupStore>) -> Self {
        Self { store }
    }

    /// Validate and persist a caller-supplied group with its exact ID.
    ///
    /// An identical retry is idempotent and returns the already-persisted
    /// group. Reusing the ID for different state returns
    /// [`GroupError::AlreadyExists`].
    pub async fn create_group(&self, group: Group) -> GroupResult<Group> {
        validate_group(&group)?;
        match self.store.create_group(group.clone()).await {
            Ok(()) => self.get_group(&group.id).await,
            Err(GroupError::AlreadyExists(id)) => {
                let existing = self.get_group(&id).await?;
                if groups_equivalent(&existing, &group) {
                    Ok(existing)
                } else {
                    Err(GroupError::AlreadyExists(id))
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Load and validate one group by exact ID.
    pub async fn get_group(&self, group_id: &GroupId) -> GroupResult<Group> {
        let group = self.store.get_group(group_id).await?;
        validate_group(&group)?;
        Ok(group)
    }

    /// List all valid groups in deterministic ID order.
    pub async fn list_groups(&self) -> GroupResult<Vec<Group>> {
        let mut groups = self.store.list_groups().await?;
        for group in &groups {
            validate_group(group)?;
        }
        groups.sort_by_key(|group| group.id.as_uuid());
        Ok(groups)
    }

    /// Add one member without replacing any other group state.
    ///
    /// An exact retry of the same member record is idempotent. A retry with the
    /// same agent ID but different membership data returns
    /// [`GroupError::AlreadyMember`].
    pub async fn add_member(&self, group_id: &GroupId, member: GroupMember) -> GroupResult<Group> {
        validate_member(*group_id, &member)?;
        let group = self.get_group(group_id).await?;
        if let Some(existing) = group.find_member(&member.agent_id) {
            return if existing == &member {
                Ok(group)
            } else {
                Err(GroupError::AlreadyMember(member.agent_id, *group_id))
            };
        }

        match self.store.add_member(group_id, member.clone()).await {
            Ok(()) => self.get_group(group_id).await,
            Err(GroupError::AlreadyMember(agent_id, id)) => {
                let current = self.get_group(&id).await?;
                if current.find_member(&agent_id) == Some(&member) {
                    Ok(current)
                } else {
                    Err(GroupError::AlreadyMember(agent_id, id))
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Remove a non-owner member without replacing any other group state.
    pub async fn remove_member(
        &self,
        group_id: &GroupId,
        agent_id: &AgentId,
    ) -> GroupResult<Group> {
        let group = self.get_group(group_id).await?;
        if group.owner_agent_id == *agent_id {
            return Err(GroupError::PermissionDenied(
                *agent_id,
                *group_id,
                "owner cannot be removed from the group".to_string(),
            ));
        }
        if !group.is_member(agent_id) {
            return Err(GroupError::NotMember(*agent_id, *group_id));
        }

        self.store.remove_member(group_id, agent_id).await?;
        self.get_group(group_id).await
    }

    /// Atomically update quorum and budget limits while retaining spend state.
    ///
    /// An identical update is a no-op. The store's narrow policy mutation
    /// ensures concurrent membership changes cannot be overwritten.
    pub async fn update_policy(
        &self,
        group_id: &GroupId,
        update: GroupPolicyUpdate,
    ) -> GroupResult<Group> {
        validate_quorum(*group_id, &update.quorum_policy)?;
        let current = self.get_group(group_id).await?;
        let mut budget = current.budget.clone();
        budget.max_total = update.max_total;
        budget.max_per_member = update.max_per_member;
        budget.max_per_run = update.max_per_run;
        validate_budget(*group_id, &budget)?;

        if current.quorum_policy == update.quorum_policy
            && current.budget.max_total == update.max_total
            && current.budget.max_per_member == update.max_per_member
            && current.budget.max_per_run == update.max_per_run
        {
            return Ok(current);
        }

        self.store
            .update_policy(group_id, update.quorum_policy, budget, Utc::now())
            .await?;
        self.get_group(group_id).await
    }

    /// Delete a group and its memberships by exact ID.
    pub async fn delete_group(&self, group_id: &GroupId) -> GroupResult<()> {
        self.store.delete_group(group_id).await
    }
}

fn validate_group(group: &Group) -> GroupResult<()> {
    if group.name.trim().is_empty() {
        return Err(invalid(group.id, "name must not be empty"));
    }
    if group.updated_at < group.created_at {
        return Err(invalid(group.id, "updated_at precedes created_at"));
    }
    validate_quorum(group.id, &group.quorum_policy)?;

    let mut agent_ids = HashSet::with_capacity(group.members.len());
    for member in &group.members {
        validate_member(group.id, member)?;
        if !agent_ids.insert(member.agent_id) {
            return Err(invalid(
                group.id,
                format!("duplicate member {}", member.agent_id),
            ));
        }
    }

    let owner = group.find_member(&group.owner_agent_id).ok_or_else(|| {
        invalid(
            group.id,
            format!("owner {} is not a member", group.owner_agent_id),
        )
    })?;
    if owner.role != MemberRole::Leader {
        return Err(invalid(group.id, "owner must have the leader role"));
    }
    if group.leaders().is_empty() {
        return Err(invalid(group.id, "group must retain a leader"));
    }

    validate_budget(group.id, &group.budget)
}

fn validate_quorum(group_id: GroupId, policy: &QuorumPolicy) -> GroupResult<()> {
    if let QuorumPolicy::Threshold { fraction } = policy {
        if !fraction.is_finite() || *fraction <= 0.0 || *fraction > 1.0 {
            return Err(invalid(
                group_id,
                "quorum threshold must be finite and in (0, 1]",
            ));
        }
    }
    Ok(())
}

fn validate_member(group_id: GroupId, member: &GroupMember) -> GroupResult<()> {
    if let Some(grant) = &member.grant_override {
        validate_grant(group_id, grant)?;
    }
    Ok(())
}

fn validate_grant(group_id: GroupId, grant: &GrantSpec) -> GroupResult<()> {
    if grant
        .max_budget
        .is_some_and(|amount| !amount.is_finite() || amount < 0.0)
    {
        return Err(invalid(
            group_id,
            "member grant budget must be finite and non-negative",
        ));
    }
    if grant
        .capabilities
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err(invalid(
            group_id,
            "member grant capabilities must not contain empty names",
        ));
    }
    if grant
        .allowed_pallets
        .iter()
        .any(|value| value.trim().is_empty())
    {
        return Err(invalid(
            group_id,
            "member grant pallets must not contain empty names",
        ));
    }
    Ok(())
}

fn validate_budget(group_id: GroupId, budget: &GroupBudget) -> GroupResult<()> {
    if budget
        .max_per_member
        .is_some_and(|limit| limit > budget.max_total)
    {
        return Err(invalid(group_id, "per-member budget exceeds total budget"));
    }
    if budget
        .max_per_run
        .is_some_and(|limit| limit > budget.max_total)
    {
        return Err(invalid(group_id, "per-run budget exceeds total budget"));
    }

    let mut accounted_total = 0_u64;
    for (agent_id, amount) in &budget.member_spent {
        if agent_id.parse::<AgentId>().is_err() {
            return Err(invalid(
                group_id,
                format!("budget accounting has invalid agent ID {agent_id}"),
            ));
        }
        if budget.max_per_member.is_some_and(|limit| *amount > limit) {
            return Err(invalid(
                group_id,
                format!("member {agent_id} spend exceeds its limit"),
            ));
        }
        accounted_total = accounted_total
            .checked_add(*amount)
            .ok_or_else(|| invalid(group_id, "member spend total overflow"))?;
    }

    let total_spent = budget.total_spent();
    if total_spent != accounted_total {
        return Err(invalid(
            group_id,
            "total spend does not match per-member accounting",
        ));
    }
    if total_spent > budget.max_total {
        return Err(invalid(group_id, "recorded spend exceeds total budget"));
    }
    Ok(())
}

fn groups_equivalent(left: &Group, right: &Group) -> bool {
    left.id == right.id
        && left.name == right.name
        && left.description == right.description
        && left.owner_agent_id == right.owner_agent_id
        && members_equivalent(&left.members, &right.members)
        && left.quorum_policy == right.quorum_policy
        && budgets_equivalent(&left.budget, &right.budget)
        && left.created_at == right.created_at
        && left.updated_at == right.updated_at
}

fn members_equivalent(left: &[GroupMember], right: &[GroupMember]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter().all(|member| {
        right
            .iter()
            .any(|candidate| candidate.agent_id == member.agent_id && candidate == member)
    })
}

fn budgets_equivalent(left: &GroupBudget, right: &GroupBudget) -> bool {
    left.max_total == right.max_total
        && left.max_per_member == right.max_per_member
        && left.max_per_run == right.max_per_run
        && left.total_spent() == right.total_spent()
        && left.member_spent == right.member_spent
}

fn invalid(group_id: GroupId, reason: impl Into<String>) -> GroupError {
    GroupError::InvalidGroup(group_id, reason.into())
}
