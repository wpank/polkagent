//! Integration tests for the group coordination subsystem.
//!
//! Exercises the full group lifecycle: creation, membership, grant resolution,
//! quorum voting, budget enforcement, and member removal — all wired through
//! the `polkagent-group` crate boundary.

// This assertion-oriented integration target uses `expect`/`unwrap` to identify
// the exact cross-crate fixture step or behavioral contract that failed.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use polkagent_core::AgentId;
use polkagent_group::{
    check_quorum, count_approvals, count_denials, Decision, GrantSpec, GroupBudget,
    GroupCoordinator, GroupError, GroupMember, MemberRole, QuorumPolicy, QuorumResult, Vote,
    VoteDecision,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_coord_with_group(name: &str) -> (GroupCoordinator, polkagent_group::GroupId, AgentId) {
    let mut coord = GroupCoordinator::new();
    let owner = AgentId::new();
    let group = coord.create_group(name, owner);
    (coord, group.id, owner)
}

fn approve_vote(role: MemberRole) -> Vote {
    Vote::new(AgentId::new(), VoteDecision::Approve, role)
}

fn deny_vote(role: MemberRole) -> Vote {
    Vote::new(AgentId::new(), VoteDecision::Deny, role)
}

fn abstain_vote(role: MemberRole) -> Vote {
    Vote::new(AgentId::new(), VoteDecision::Abstain, role)
}

// ---------------------------------------------------------------------------
// IT-GROUP-01: Group lifecycle — create with leader → add members → resolve grants → check budget
// ---------------------------------------------------------------------------

#[test]
fn create_group_adds_leader_as_first_member() {
    let (coord, group_id, owner) = make_coord_with_group("team-alpha");
    let group = coord.get_group(&group_id).expect("group exists");
    assert_eq!(group.name, "team-alpha");
    assert_eq!(group.members.len(), 1);
    assert_eq!(group.members[0].agent_id, owner);
    assert!(matches!(group.members[0].role, MemberRole::Leader));
}

#[test]
fn add_multiple_members_to_group() {
    let (mut coord, group_id, _owner) = make_coord_with_group("multi-member");
    let worker1 = AgentId::new();
    let worker2 = AgentId::new();
    let observer = AgentId::new();

    coord
        .add_member(&group_id, GroupMember::new(worker1, MemberRole::Worker))
        .expect("add worker1");
    coord
        .add_member(&group_id, GroupMember::new(worker2, MemberRole::Worker))
        .expect("add worker2");
    coord
        .add_member(&group_id, GroupMember::new(observer, MemberRole::Observer))
        .expect("add observer");

    let group = coord.get_group(&group_id).expect("group exists");
    assert_eq!(group.members.len(), 4);
}

#[test]
fn resolve_grants_with_no_override_returns_base_spec() {
    let (coord, group_id, owner) = make_coord_with_group("grant-test");
    let base = GrantSpec {
        capabilities: vec!["chain.transfer".to_string(), "model.inference".to_string()],
        max_budget: Some(1000.0),
        allowed_pallets: vec!["Balances".to_string()],
    };

    let eg = coord
        .resolve_effective_grant_with_base(&group_id, &owner, &base)
        .expect("resolve ok");

    assert_eq!(
        eg.grant_spec.capabilities,
        vec!["chain.transfer", "model.inference"]
    );
    assert_eq!(eg.grant_spec.max_budget, Some(1000.0));
    assert!(eg.grant_spec.has_capability("chain.transfer"));
}

#[test]
fn resolve_grants_with_override_narrows_capabilities() {
    let mut coord = GroupCoordinator::new();
    let owner = AgentId::new();
    let group = coord.create_group("narrow-grant", owner);
    let group_id = group.id;

    let worker = AgentId::new();
    let member_grant = GrantSpec {
        capabilities: vec!["chain.transfer".to_string(), "governance.vote".to_string()],
        max_budget: Some(9999.0),
        allowed_pallets: vec![],
    };
    coord
        .add_member(
            &group_id,
            GroupMember::with_grant(worker, MemberRole::Worker, member_grant),
        )
        .expect("add worker");

    let group_base = GrantSpec {
        capabilities: vec!["chain.transfer".to_string()],
        max_budget: Some(500.0),
        allowed_pallets: vec![],
    };

    let eg = coord
        .resolve_effective_grant_with_base(&group_id, &worker, &group_base)
        .expect("resolve ok");

    assert_eq!(eg.grant_spec.capabilities, vec!["chain.transfer"]);
    assert!(!eg.grant_spec.has_capability("governance.vote"));
    // Budget is narrowed to the group limit
    assert_eq!(eg.grant_spec.max_budget, Some(500.0));
}

#[test]
fn check_budget_within_limit_succeeds() {
    let (mut coord, group_id, _owner) = make_coord_with_group("budget-ok");
    coord
        .set_budget(&group_id, GroupBudget::new(1000, None, None))
        .expect("set budget");

    assert!(coord.check_budget(&group_id, 500.0).expect("check"));
    assert!(coord.check_budget(&group_id, 1000.0).expect("check"));
}

#[test]
fn check_budget_over_limit_returns_false() {
    let (mut coord, group_id, _owner) = make_coord_with_group("budget-over");
    coord
        .set_budget(&group_id, GroupBudget::new(100, None, None))
        .expect("set budget");

    assert!(!coord.check_budget(&group_id, 101.0).expect("check"));
}

#[test]
fn record_spend_tracks_group_and_member_totals() {
    let (mut coord, group_id, owner) = make_coord_with_group("spend-track");
    let worker = AgentId::new();
    coord
        .add_member(&group_id, GroupMember::new(worker, MemberRole::Worker))
        .expect("add worker");
    coord
        .set_budget(&group_id, GroupBudget::new(10_000, Some(1000), None))
        .expect("set budget");

    coord
        .record_spend(&group_id, &owner, 300.0)
        .expect("spend owner");
    coord
        .record_spend(&group_id, &worker, 200.0)
        .expect("spend worker");

    let group = coord.get_group(&group_id).expect("exists");
    assert_eq!(group.budget.total_spent(), 500);

    let owner_spent = group
        .budget
        .member_spent
        .get(&owner.to_string())
        .copied()
        .unwrap_or(0);
    assert_eq!(owner_spent, 300);

    let worker_spent = group
        .budget
        .member_spent
        .get(&worker.to_string())
        .copied()
        .unwrap_or(0);
    assert_eq!(worker_spent, 200);
}

// ---------------------------------------------------------------------------
// IT-GROUP-02: Quorum vote — unanimous, majority, threshold
// ---------------------------------------------------------------------------

#[test]
fn quorum_unanimous_all_approve() {
    let votes = vec![
        approve_vote(MemberRole::Leader),
        approve_vote(MemberRole::Worker),
        approve_vote(MemberRole::Worker),
    ];
    let result = check_quorum(&QuorumPolicy::Unanimous, &votes, 3);
    assert!(
        matches!(
            result,
            QuorumResult::Reached {
                decision: Decision::Approved
            }
        ),
        "unanimous all-approve should reach quorum"
    );
}

#[test]
fn quorum_unanimous_single_deny_fails() {
    let votes = vec![
        approve_vote(MemberRole::Leader),
        deny_vote(MemberRole::Worker),
        approve_vote(MemberRole::Worker),
    ];
    let result = check_quorum(&QuorumPolicy::Unanimous, &votes, 3);
    assert!(
        matches!(result, QuorumResult::Failed(_)),
        "unanimous with deny should fail"
    );
}

#[test]
fn quorum_unanimous_abstain_fails() {
    let votes = vec![
        approve_vote(MemberRole::Leader),
        abstain_vote(MemberRole::Worker),
    ];
    let result = check_quorum(&QuorumPolicy::Unanimous, &votes, 2);
    assert!(
        matches!(result, QuorumResult::Failed(_)),
        "unanimous with abstain should fail"
    );
}

#[test]
fn quorum_majority_more_than_half_approve() {
    let votes = vec![
        approve_vote(MemberRole::Leader),
        approve_vote(MemberRole::Worker),
        deny_vote(MemberRole::Worker),
    ];
    let result = check_quorum(&QuorumPolicy::Majority, &votes, 3);
    assert!(
        matches!(
            result,
            QuorumResult::Reached {
                decision: Decision::Approved
            }
        ),
        "majority 2/3 should reach quorum"
    );
}

#[test]
fn quorum_majority_half_is_not_enough() {
    // 2 of 4: 50% is not > 50%, threshold = 3, max_possible = 2+0 = 2 < 3
    let votes = vec![
        approve_vote(MemberRole::Leader),
        approve_vote(MemberRole::Worker),
        deny_vote(MemberRole::Worker),
        deny_vote(MemberRole::Worker),
    ];
    let result = check_quorum(&QuorumPolicy::Majority, &votes, 4);
    assert!(
        matches!(result, QuorumResult::Failed(_)),
        "2/4 is not a majority"
    );
}

#[test]
fn quorum_majority_pending_when_more_votes_possible() {
    let votes = vec![approve_vote(MemberRole::Leader)];
    // threshold = 4/2+1 = 3, approve_count = 1, uncast = 3, max_possible = 4 >= 3 → pending
    let result = check_quorum(&QuorumPolicy::Majority, &votes, 4);
    assert!(
        matches!(result, QuorumResult::Pending { .. }),
        "should be pending with votes still possible"
    );
}

#[test]
fn quorum_threshold_75_percent_three_of_four() {
    let votes = vec![
        approve_vote(MemberRole::Leader),
        approve_vote(MemberRole::Worker),
        approve_vote(MemberRole::Worker),
        deny_vote(MemberRole::Worker),
    ];
    let result = check_quorum(&QuorumPolicy::Threshold { fraction: 0.75 }, &votes, 4);
    // ceil(4 * 0.75) = 3 approvals required; 3 cast → reached
    assert!(
        matches!(
            result,
            QuorumResult::Reached {
                decision: Decision::Approved
            }
        ),
        "3/4 = 75% should meet threshold"
    );
}

#[test]
fn quorum_threshold_fails_when_impossible_to_reach() {
    let votes = vec![
        deny_vote(MemberRole::Leader),
        deny_vote(MemberRole::Worker),
        deny_vote(MemberRole::Worker),
    ];
    let result = check_quorum(&QuorumPolicy::Threshold { fraction: 0.75 }, &votes, 3);
    // ceil(3 * 0.75) = 3, 0 approvals, 0 uncast → cannot reach
    assert!(
        matches!(result, QuorumResult::Failed(_)),
        "0 approvals cannot reach 75% threshold"
    );
}

#[test]
fn quorum_leader_only_leader_approve() {
    let votes = vec![
        approve_vote(MemberRole::Leader),
        deny_vote(MemberRole::Worker), // worker deny is irrelevant
    ];
    let result = check_quorum(&QuorumPolicy::LeaderOnly, &votes, 2);
    assert!(
        matches!(
            result,
            QuorumResult::Reached {
                decision: Decision::Approved
            }
        ),
        "leader approval satisfies LeaderOnly"
    );
}

#[test]
fn quorum_leader_only_leader_deny_fails() {
    let votes = vec![
        deny_vote(MemberRole::Leader),
        approve_vote(MemberRole::Worker),
    ];
    let result = check_quorum(&QuorumPolicy::LeaderOnly, &votes, 2);
    assert!(
        matches!(result, QuorumResult::Failed(_)),
        "leader deny should fail LeaderOnly"
    );
}

#[test]
fn quorum_observer_votes_ignored() {
    // 2 voting members: leader + worker both approve. Observer deny should be ignored.
    let votes = vec![
        approve_vote(MemberRole::Leader),
        approve_vote(MemberRole::Worker),
        deny_vote(MemberRole::Observer),
    ];
    let result = check_quorum(&QuorumPolicy::Unanimous, &votes, 2);
    assert!(
        matches!(
            result,
            QuorumResult::Reached {
                decision: Decision::Approved
            }
        ),
        "observer vote should not affect quorum"
    );
}

#[test]
fn count_helpers_exclude_observers() {
    let votes = vec![
        approve_vote(MemberRole::Leader),
        approve_vote(MemberRole::Worker),
        deny_vote(MemberRole::Worker),
        approve_vote(MemberRole::Observer), // excluded
        deny_vote(MemberRole::Observer),    // excluded
    ];
    assert_eq!(count_approvals(&votes), 2);
    assert_eq!(count_denials(&votes), 1);
}

#[test]
fn quorum_leader_only_no_leader_vote_is_pending() {
    let votes = vec![approve_vote(MemberRole::Worker)];
    let result = check_quorum(&QuorumPolicy::LeaderOnly, &votes, 2);
    assert!(
        matches!(result, QuorumResult::Pending { .. }),
        "no leader vote → pending"
    );
}

#[test]
fn quorum_empty_group_fails() {
    let result = check_quorum(&QuorumPolicy::Unanimous, &[], 0);
    assert!(matches!(result, QuorumResult::Failed(_)));
}

// ---------------------------------------------------------------------------
// IT-GROUP-03: Grant intersection narrows child grant
// ---------------------------------------------------------------------------

#[test]
fn grant_intersection_keeps_only_common_capabilities() {
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
    let intersected = a.intersect(&b);
    assert_eq!(intersected.capabilities, vec!["chain.transfer"]);
    assert_eq!(intersected.max_budget, Some(500.0));
    assert!(!intersected.has_capability("model.inference"));
    assert!(!intersected.has_capability("governance.vote"));
}

#[test]
fn grant_intersection_pallet_restriction_narrows() {
    let group = GrantSpec {
        capabilities: vec!["chain.transfer".to_string()],
        max_budget: None,
        allowed_pallets: vec!["Balances".to_string(), "Staking".to_string()],
    };
    let member = GrantSpec {
        capabilities: vec!["chain.transfer".to_string()],
        max_budget: None,
        allowed_pallets: vec!["Balances".to_string(), "Governance".to_string()],
    };
    let result = group.intersect(&member);
    assert_eq!(result.allowed_pallets, vec!["Balances"]);
}

#[test]
fn grant_intersection_empty_pallet_means_all_allowed() {
    let group = GrantSpec {
        capabilities: vec!["chain.transfer".to_string()],
        max_budget: None,
        allowed_pallets: vec![], // empty = all
    };
    let member = GrantSpec {
        capabilities: vec!["chain.transfer".to_string()],
        max_budget: None,
        allowed_pallets: vec!["Balances".to_string()],
    };
    let result = group.intersect(&member);
    // Empty on one side → use the non-empty side
    assert_eq!(result.allowed_pallets, vec!["Balances"]);
}

#[test]
fn grant_none_budget_propagates_other() {
    let a = GrantSpec {
        capabilities: vec!["a".to_string()],
        max_budget: None,
        allowed_pallets: vec![],
    };
    let b = GrantSpec {
        capabilities: vec!["a".to_string()],
        max_budget: Some(42.0),
        allowed_pallets: vec![],
    };
    assert_eq!(a.intersect(&b).max_budget, Some(42.0));
    assert_eq!(b.intersect(&a).max_budget, Some(42.0));
}

#[test]
fn grant_intersection_takes_minimum_budget() {
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
    assert_eq!(b.intersect(&a).max_budget, Some(1000.0));
}

// ---------------------------------------------------------------------------
// IT-GROUP-04: Budget enforcement across group members
// ---------------------------------------------------------------------------

#[test]
fn per_member_budget_enforced_independently() {
    let (mut coord, group_id, owner) = make_coord_with_group("per-member-budget");
    let worker = AgentId::new();
    coord
        .add_member(&group_id, GroupMember::new(worker, MemberRole::Worker))
        .expect("add worker");
    coord
        .set_budget(&group_id, GroupBudget::new(10_000, Some(500), None))
        .expect("set budget");

    // Owner can spend up to 500
    coord
        .record_spend(&group_id, &owner, 300.0)
        .expect("owner spend");
    // Owner now has 200 headroom
    assert!(coord
        .check_member_budget(&group_id, &owner, 200.0)
        .expect("check owner"));
    assert!(!coord
        .check_member_budget(&group_id, &owner, 201.0)
        .expect("check owner over"));

    // Worker starts fresh
    assert!(coord
        .check_member_budget(&group_id, &worker, 500.0)
        .expect("check worker"));
}

#[test]
fn record_spend_over_total_rejected_atomically() {
    let (mut coord, group_id, owner) = make_coord_with_group("atomic-reject");
    coord
        .set_budget(&group_id, GroupBudget::new(100, None, None))
        .expect("set budget");

    // First spend succeeds
    coord
        .record_spend(&group_id, &owner, 80.0)
        .expect("first spend");

    // Second spend would exceed total
    let result = coord.record_spend(&group_id, &owner, 25.0);
    assert!(
        matches!(result, Err(GroupError::BudgetExceeded(..))),
        "over-budget spend must be rejected"
    );

    // Verify total was not updated
    let group = coord.get_group(&group_id).expect("exists");
    assert_eq!(group.budget.total_spent(), 80);
}

#[test]
fn per_run_budget_enforced() {
    let (mut coord, group_id, _owner) = make_coord_with_group("per-run");
    coord
        .set_budget(&group_id, GroupBudget::new(10_000, None, Some(50)))
        .expect("set budget");

    // Amount within per-run limit
    assert!(coord.check_budget(&group_id, 50.0).expect("check"));
    // Amount exceeds per-run limit
    assert!(!coord.check_budget(&group_id, 51.0).expect("check over"));
}

#[test]
fn multiple_members_track_spend_separately() {
    let (mut coord, group_id, owner) = make_coord_with_group("multi-spend");
    let worker = AgentId::new();
    coord
        .add_member(&group_id, GroupMember::new(worker, MemberRole::Worker))
        .expect("add worker");
    coord
        .set_budget(&group_id, GroupBudget::new(1000, Some(300), None))
        .expect("set budget");

    coord.record_spend(&group_id, &owner, 100.0).expect("owner");
    coord
        .record_spend(&group_id, &worker, 200.0)
        .expect("worker");

    let group = coord.get_group(&group_id).expect("exists");
    assert_eq!(group.budget.total_spent(), 300);
    assert_eq!(
        group
            .budget
            .member_spent
            .get(&owner.to_string())
            .copied()
            .unwrap_or(0),
        100
    );
    assert_eq!(
        group
            .budget
            .member_spent
            .get(&worker.to_string())
            .copied()
            .unwrap_or(0),
        200
    );
}

// ---------------------------------------------------------------------------
// IT-GROUP-05: Remove member cleans up properly
// ---------------------------------------------------------------------------

#[test]
fn remove_member_decrements_member_count() {
    let (mut coord, group_id, _owner) = make_coord_with_group("remove-test");
    let worker = AgentId::new();
    coord
        .add_member(&group_id, GroupMember::new(worker, MemberRole::Worker))
        .expect("add");

    let before = coord.get_group(&group_id).expect("exists").members.len();
    coord.remove_member(&group_id, &worker).expect("remove");
    let after = coord.get_group(&group_id).expect("exists").members.len();
    assert_eq!(after, before - 1);
}

#[test]
fn remove_member_cannot_find_removed_agent() {
    let (mut coord, group_id, _owner) = make_coord_with_group("remove-check");
    let worker = AgentId::new();
    coord
        .add_member(&group_id, GroupMember::new(worker, MemberRole::Worker))
        .expect("add");
    coord.remove_member(&group_id, &worker).expect("remove");

    let group = coord.get_group(&group_id).expect("exists");
    assert!(
        group.find_member(&worker).is_none(),
        "removed member should not be found"
    );
    assert!(!group.is_member(&worker));
}

#[test]
fn remove_owner_is_forbidden() {
    let (mut coord, group_id, owner) = make_coord_with_group("owner-guard");
    let result = coord.remove_member(&group_id, &owner);
    assert!(
        matches!(result, Err(GroupError::PermissionDenied(..))),
        "removing owner must be forbidden"
    );
}

#[test]
fn remove_nonexistent_member_returns_not_member_error() {
    let (mut coord, group_id, _owner) = make_coord_with_group("ghost-member");
    let stranger = AgentId::new();
    let result = coord.remove_member(&group_id, &stranger);
    assert!(
        matches!(result, Err(GroupError::NotMember(..))),
        "removing non-member must return NotMember"
    );
}

#[test]
fn resolve_grant_for_removed_member_returns_not_member() {
    let (mut coord, group_id, _owner) = make_coord_with_group("removed-grant");
    let worker = AgentId::new();
    coord
        .add_member(&group_id, GroupMember::new(worker, MemberRole::Worker))
        .expect("add");
    coord.remove_member(&group_id, &worker).expect("remove");

    let result = coord.resolve_effective_grant(&group_id, &worker);
    assert!(
        matches!(result, Err(GroupError::NotMember(..))),
        "removed member cannot resolve grant"
    );
}

#[test]
fn delete_group_removes_from_coordinator() {
    let (mut coord, group_id, _owner) = make_coord_with_group("delete-me");
    coord.delete_group(&group_id).expect("delete");
    let result = coord.get_group(&group_id);
    assert!(
        matches!(result, Err(GroupError::NotFound(..))),
        "deleted group must not be found"
    );
}

#[test]
fn coordinator_lists_all_groups() {
    let mut coord = GroupCoordinator::new();
    let owner = AgentId::new();
    coord.create_group("g1", owner);
    coord.create_group("g2", owner);
    coord.create_group("g3", owner);
    assert_eq!(coord.list_group_ids().len(), 3);
}

#[test]
fn group_is_member_and_find_member_work_correctly() {
    let (coord, group_id, owner) = make_coord_with_group("find-check");
    let group = coord.get_group(&group_id).expect("exists");

    assert!(group.is_member(&owner));
    assert!(group.find_member(&owner).is_some());

    let stranger = AgentId::new();
    assert!(!group.is_member(&stranger));
    assert!(group.find_member(&stranger).is_none());
}

#[test]
fn group_voting_member_count_excludes_observers() {
    let (mut coord, group_id, _owner) = make_coord_with_group("voter-count");
    let worker = AgentId::new();
    let observer = AgentId::new();
    coord
        .add_member(&group_id, GroupMember::new(worker, MemberRole::Worker))
        .expect("add worker");
    coord
        .add_member(&group_id, GroupMember::new(observer, MemberRole::Observer))
        .expect("add observer");

    let group = coord.get_group(&group_id).expect("exists");
    // 1 leader + 1 worker = 2 voting, 1 observer excluded
    assert_eq!(group.voting_member_count(), 2);
    assert_eq!(group.leaders().len(), 1);
}
