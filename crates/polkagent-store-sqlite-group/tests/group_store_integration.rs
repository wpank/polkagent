//! Integration tests for the [`GroupStore`] implementation on [`SqliteGroupStore`].
//!
//! These tests exercise the full async [`GroupStore`] trait surface via
//! [`SqliteGroupStore`], which wraps [`SqlitePool`] in a newtype.

// Integration assertions unwrap controlled fixtures to preserve failure context.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::Utc;

use polkagent_core::ids::AgentId;
use polkagent_group::{
    types::{GrantSpec, Group, GroupBudget, GroupId, GroupMember, MemberRole, QuorumPolicy},
    GroupError, GroupStore,
};
use polkagent_store_sqlite::SqlitePool;
use polkagent_store_sqlite_group::SqliteGroupStore;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_store() -> SqliteGroupStore {
    let pool = SqlitePool::open_in_memory().expect("open in-memory pool");
    {
        let writer = pool.writer();
        polkagent_store_sqlite_group::migrate_groups(&writer).expect("migrate");
    }
    SqliteGroupStore::new(pool)
}

fn make_group(name: &str) -> Group {
    let owner = AgentId::new();
    Group {
        id: GroupId::new(),
        name: name.to_string(),
        description: String::new(),
        owner_agent_id: owner,
        members: Vec::new(),
        quorum_policy: QuorumPolicy::Majority,
        budget: GroupBudget::new(10_000, None, None),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn make_member(role: MemberRole) -> GroupMember {
    GroupMember::new(AgentId::new(), role)
}

// ---------------------------------------------------------------------------
// Group CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_and_get_group() {
    let store = test_store();
    let group = make_group("alpha");
    let gid = group.id;

    store
        .create_group(group.clone())
        .await
        .expect("create group");

    let fetched = store.get_group(&gid).await.expect("get group");

    assert_eq!(fetched.id, gid);
    assert_eq!(fetched.name, "alpha");
    assert_eq!(fetched.description, "");
    assert_eq!(fetched.owner_agent_id, group.owner_agent_id);
    assert!(fetched.members.is_empty());
}

#[tokio::test]
async fn create_group_with_description() {
    let store = test_store();
    let mut group = make_group("beta");
    group.description = "A description".to_string();
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let fetched = store.get_group(&gid).await.expect("get");
    assert_eq!(fetched.description, "A description");
}

#[tokio::test]
async fn get_group_not_found() {
    let store = test_store();
    let gid = GroupId::new();

    let err = store.get_group(&gid).await.expect_err("should be NotFound");
    assert!(matches!(err, GroupError::NotFound(_)));
}

#[tokio::test]
async fn create_duplicate_group_returns_already_exists() {
    let store = test_store();
    let group = make_group("gamma");

    store
        .create_group(group.clone())
        .await
        .expect("first create");

    let err = store
        .create_group(group)
        .await
        .expect_err("duplicate should fail");
    assert!(
        matches!(err, GroupError::AlreadyExists(_)),
        "expected AlreadyExists, got: {err:?}"
    );
}

#[tokio::test]
async fn update_group() {
    let store = test_store();
    let mut group = make_group("delta");
    let gid = group.id;

    store.create_group(group.clone()).await.expect("create");

    group.name = "delta-updated".to_string();
    group.description = "now has a description".to_string();
    group.quorum_policy = QuorumPolicy::Unanimous;

    store.update_group(group).await.expect("update");

    let fetched = store.get_group(&gid).await.expect("get");
    assert_eq!(fetched.name, "delta-updated");
    assert_eq!(fetched.description, "now has a description");
    assert_eq!(fetched.quorum_policy, QuorumPolicy::Unanimous);
}

#[tokio::test]
async fn update_group_not_found() {
    let store = test_store();
    let group = make_group("epsilon");

    let err = store
        .update_group(group)
        .await
        .expect_err("should be NotFound");
    assert!(matches!(err, GroupError::NotFound(_)));
}

#[tokio::test]
async fn delete_group() {
    let store = test_store();
    let group = make_group("zeta");
    let gid = group.id;

    store.create_group(group).await.expect("create");
    store.delete_group(&gid).await.expect("delete");

    let err = store.get_group(&gid).await.expect_err("should be gone");
    assert!(matches!(err, GroupError::NotFound(_)));
}

#[tokio::test]
async fn delete_group_not_found() {
    let store = test_store();
    let gid = GroupId::new();

    let err = store
        .delete_group(&gid)
        .await
        .expect_err("should be NotFound");
    assert!(matches!(err, GroupError::NotFound(_)));
}

#[tokio::test]
async fn list_groups_empty() {
    let store = test_store();
    let groups = store.list_groups().await.expect("list");
    assert!(groups.is_empty());
}

#[tokio::test]
async fn list_groups_returns_all() {
    let store = test_store();

    for name in ["g1", "g2", "g3"] {
        store.create_group(make_group(name)).await.expect("create");
    }

    let groups = store.list_groups().await.expect("list");
    assert_eq!(groups.len(), 3);
}

// ---------------------------------------------------------------------------
// Member operations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_and_list_member() {
    let store = test_store();
    let group = make_group("eta");
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let member = make_member(MemberRole::Worker);
    let agent_id = member.agent_id;

    store.add_member(&gid, member).await.expect("add member");

    let members = store.list_members(&gid).await.expect("list members");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].agent_id, agent_id);
    assert_eq!(members[0].role, MemberRole::Worker);
}

#[tokio::test]
async fn add_member_to_nonexistent_group() {
    let store = test_store();
    let gid = GroupId::new();
    let member = make_member(MemberRole::Worker);

    let err = store
        .add_member(&gid, member)
        .await
        .expect_err("should be NotFound");
    assert!(matches!(err, GroupError::NotFound(_)));
}

#[tokio::test]
async fn add_duplicate_member_returns_typed_error() {
    let store = test_store();
    let group = make_group("theta");
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let member = make_member(MemberRole::Worker);
    let agent_id = member.agent_id;

    store.add_member(&gid, member).await.expect("first add");

    // Adding the same agent again should be an error.
    let member2 = GroupMember::new(agent_id, MemberRole::Observer);
    let err = store
        .add_member(&gid, member2)
        .await
        .expect_err("duplicate member");
    assert!(matches!(
        err,
        GroupError::AlreadyMember(agent, id) if agent == agent_id && id == gid
    ));
}

#[tokio::test]
async fn remove_member() {
    let store = test_store();
    let group = make_group("iota");
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let member = make_member(MemberRole::Worker);
    let agent_id = member.agent_id;

    store.add_member(&gid, member).await.expect("add");
    store.remove_member(&gid, &agent_id).await.expect("remove");

    let members = store.list_members(&gid).await.expect("list");
    assert!(members.is_empty());
}

#[tokio::test]
async fn remove_member_not_member() {
    let store = test_store();
    let group = make_group("kappa");
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let agent_id = AgentId::new();
    let err = store
        .remove_member(&gid, &agent_id)
        .await
        .expect_err("should be NotMember");
    assert!(matches!(err, GroupError::NotMember(_, _)));
}

#[tokio::test]
async fn remove_member_group_not_found() {
    let store = test_store();
    let gid = GroupId::new();
    let agent_id = AgentId::new();

    let err = store
        .remove_member(&gid, &agent_id)
        .await
        .expect_err("should be NotFound");
    assert!(matches!(err, GroupError::NotFound(_)));
}

#[tokio::test]
async fn list_members_group_not_found() {
    let store = test_store();
    let gid = GroupId::new();

    let err = store
        .list_members(&gid)
        .await
        .expect_err("should be NotFound");
    assert!(matches!(err, GroupError::NotFound(_)));
}

#[tokio::test]
async fn list_members_multiple_roles() {
    let store = test_store();
    let group = make_group("lambda");
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let leader = make_member(MemberRole::Leader);
    let worker = make_member(MemberRole::Worker);
    let observer = make_member(MemberRole::Observer);

    store.add_member(&gid, leader).await.expect("add leader");
    store.add_member(&gid, worker).await.expect("add worker");
    store
        .add_member(&gid, observer)
        .await
        .expect("add observer");

    let members = store.list_members(&gid).await.expect("list");
    assert_eq!(members.len(), 3);

    let roles: Vec<MemberRole> = members.iter().map(|m| m.role).collect();
    assert!(roles.contains(&MemberRole::Leader));
    assert!(roles.contains(&MemberRole::Worker));
    assert!(roles.contains(&MemberRole::Observer));
}

// ---------------------------------------------------------------------------
// QuorumPolicy serialization
// ---------------------------------------------------------------------------

#[tokio::test]
async fn quorum_policy_unanimous_round_trip() {
    let store = test_store();
    let mut group = make_group("mu");
    group.quorum_policy = QuorumPolicy::Unanimous;
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let fetched = store.get_group(&gid).await.expect("get");
    assert_eq!(fetched.quorum_policy, QuorumPolicy::Unanimous);
}

#[tokio::test]
async fn quorum_policy_majority_round_trip() {
    let store = test_store();
    let mut group = make_group("nu");
    group.quorum_policy = QuorumPolicy::Majority;
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let fetched = store.get_group(&gid).await.expect("get");
    assert_eq!(fetched.quorum_policy, QuorumPolicy::Majority);
}

#[tokio::test]
async fn quorum_policy_leader_only_round_trip() {
    let store = test_store();
    let mut group = make_group("xi");
    group.quorum_policy = QuorumPolicy::LeaderOnly;
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let fetched = store.get_group(&gid).await.expect("get");
    assert_eq!(fetched.quorum_policy, QuorumPolicy::LeaderOnly);
}

#[tokio::test]
async fn quorum_policy_threshold_round_trip() {
    let store = test_store();
    let mut group = make_group("omicron");
    group.quorum_policy = QuorumPolicy::Threshold { fraction: 0.75 };
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let fetched = store.get_group(&gid).await.expect("get");
    assert_eq!(
        fetched.quorum_policy,
        QuorumPolicy::Threshold { fraction: 0.75 }
    );
}

// ---------------------------------------------------------------------------
// Budget JSON
// ---------------------------------------------------------------------------

#[tokio::test]
async fn budget_round_trip_full() {
    let store = test_store();
    let mut group = make_group("pi");
    group.budget = GroupBudget::new(50_000, Some(1_000), Some(500));
    group
        .budget
        .member_spent
        .insert("some-agent".to_string(), 200);
    *group.budget.spent.lock() = 200;
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let fetched = store.get_group(&gid).await.expect("get");
    assert_eq!(fetched.budget.max_total, 50_000);
    assert_eq!(fetched.budget.max_per_member, Some(1_000));
    assert_eq!(fetched.budget.max_per_run, Some(500));
    assert_eq!(
        fetched.budget.member_spent.get("some-agent").copied(),
        Some(200)
    );
    assert_eq!(fetched.budget.total_spent(), 200);
}

#[tokio::test]
async fn budget_round_trip_minimal() {
    let store = test_store();
    let mut group = make_group("rho");
    group.budget = GroupBudget::new(100, None, None);
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let fetched = store.get_group(&gid).await.expect("get");
    assert_eq!(fetched.budget.max_total, 100);
    assert!(fetched.budget.max_per_member.is_none());
    assert!(fetched.budget.max_per_run.is_none());
}

// ---------------------------------------------------------------------------
// Grant override serialization
// ---------------------------------------------------------------------------

#[tokio::test]
async fn member_grant_override_round_trip() {
    let store = test_store();
    let group = make_group("sigma");
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let grant = GrantSpec {
        capabilities: vec!["chain.transfer".to_string(), "model.inference".to_string()],
        max_budget: Some(999.0),
        allowed_pallets: vec!["Balances".to_string()],
    };
    let member = GroupMember::with_grant(AgentId::new(), MemberRole::Worker, grant.clone());
    let agent_id = member.agent_id;

    store.add_member(&gid, member).await.expect("add");

    let members = store.list_members(&gid).await.expect("list");
    assert_eq!(members.len(), 1);

    let fetched_grant = members[0]
        .grant_override
        .as_ref()
        .expect("should have grant");
    assert_eq!(fetched_grant.capabilities, grant.capabilities);
    assert_eq!(fetched_grant.max_budget, grant.max_budget);
    assert_eq!(fetched_grant.allowed_pallets, grant.allowed_pallets);

    // Also verify via get_group.
    let group = store.get_group(&gid).await.expect("get");
    let member_in_group = group
        .find_member(&agent_id)
        .expect("member should be in group");
    assert!(member_in_group.grant_override.is_some());
}

#[tokio::test]
async fn member_no_grant_override() {
    let store = test_store();
    let group = make_group("tau");
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let member = GroupMember::new(AgentId::new(), MemberRole::Observer);
    store.add_member(&gid, member).await.expect("add");

    let members = store.list_members(&gid).await.expect("list");
    assert_eq!(members.len(), 1);
    assert!(members[0].grant_override.is_none());
}

// ---------------------------------------------------------------------------
// Delete cascades to members
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_group_cascades_members() {
    let store = test_store();
    let group = make_group("upsilon");
    let gid = group.id;

    store.create_group(group).await.expect("create");

    store
        .add_member(&gid, make_member(MemberRole::Worker))
        .await
        .expect("add member");

    store.delete_group(&gid).await.expect("delete");

    // Verify member rows are gone via a raw count.
    // Access the inner SqlitePool through Deref.
    let count: i64 = {
        let writer = store.writer();
        writer
            .query_row(
                "SELECT COUNT(*) FROM group_members WHERE group_id = ?1",
                [&gid.to_string()],
                |r| r.get(0),
            )
            .expect("count")
    };
    assert_eq!(count, 0);
}

// ---------------------------------------------------------------------------
// update_group replaces members
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_group_replaces_members() {
    let store = test_store();
    let mut group = make_group("phi");
    let gid = group.id;
    let old_agent = AgentId::new();
    group
        .members
        .push(GroupMember::new(old_agent, MemberRole::Worker));

    store.create_group(group.clone()).await.expect("create");

    // Update with a completely different member set.
    let new_agent = AgentId::new();
    group.members = vec![GroupMember::new(new_agent, MemberRole::Leader)];
    store.update_group(group).await.expect("update");

    let members = store.list_members(&gid).await.expect("list");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].agent_id, new_agent);
    assert_eq!(members[0].role, MemberRole::Leader);
}

// ---------------------------------------------------------------------------
// Groups created with members in the same call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_group_with_initial_members() {
    let store = test_store();
    let leader = AgentId::new();
    let worker = AgentId::new();

    let mut group = make_group("psi");
    group
        .members
        .push(GroupMember::new(leader, MemberRole::Leader));
    group
        .members
        .push(GroupMember::new(worker, MemberRole::Worker));
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let fetched = store.get_group(&gid).await.expect("get");
    assert_eq!(fetched.members.len(), 2);
    assert!(fetched.find_member(&leader).is_some());
    assert!(fetched.find_member(&worker).is_some());
}

// ---------------------------------------------------------------------------
// Concurrent access
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_group_creation() {
    use std::sync::Arc;

    let store = Arc::new(test_store());
    let mut handles = Vec::new();

    for i in 0..10u32 {
        let store_clone = store.clone();
        let handle = tokio::spawn(async move {
            let group = make_group(&format!("concurrent-{i}"));
            store_clone
                .create_group(group)
                .await
                .expect("concurrent create");
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.expect("task completed");
    }

    let groups = store.list_groups().await.expect("list");
    assert_eq!(groups.len(), 10);
}

#[tokio::test]
async fn concurrent_member_addition() {
    use std::sync::Arc;

    let store = Arc::new(test_store());
    let group = make_group("chi");
    let gid = group.id;

    store.create_group(group).await.expect("create");

    let mut handles = Vec::new();
    for _ in 0..5u32 {
        let store_clone = store.clone();
        let handle = tokio::spawn(async move {
            let member = make_member(MemberRole::Worker);
            store_clone
                .add_member(&gid, member)
                .await
                .expect("concurrent add member");
        });
        handles.push(handle);
    }

    for h in handles {
        h.await.expect("task completed");
    }

    let members = store.list_members(&gid).await.expect("list");
    assert_eq!(members.len(), 5);
}

// ---------------------------------------------------------------------------
// list_groups returns members for each group
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_groups_includes_members() {
    let store = test_store();

    let mut group_a = make_group("list-a");
    group_a.members.push(make_member(MemberRole::Leader));
    group_a.members.push(make_member(MemberRole::Worker));

    let group_b = make_group("list-b");

    store.create_group(group_a).await.expect("create a");
    store.create_group(group_b).await.expect("create b");

    let groups = store.list_groups().await.expect("list");
    assert_eq!(groups.len(), 2);

    let a = groups.iter().find(|g| g.name == "list-a").expect("group a");
    let b = groups.iter().find(|g| g.name == "list-b").expect("group b");
    assert_eq!(a.members.len(), 2);
    assert_eq!(b.members.len(), 0);
}

// ---------------------------------------------------------------------------
// SqliteGroupStore object safety
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn _store_is_group_store() {
    fn assert_group_store<T: GroupStore>() {}
    assert_group_store::<SqliteGroupStore>();
}
