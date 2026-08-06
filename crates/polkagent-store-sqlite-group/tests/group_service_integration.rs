//! File-backed durability and concurrency tests for the public group service.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use chrono::Utc;
use polkagent_core::ids::AgentId;
use polkagent_group::{
    GrantSpec, Group, GroupBudget, GroupError, GroupId, GroupMember, GroupPolicyUpdate,
    GroupService, GroupStore, MemberRole, QuorumPolicy,
};
use polkagent_store_sqlite::SqlitePool;
use polkagent_store_sqlite_group::{migrate_groups, SqliteGroupStore};
use tempfile::TempDir;

fn open_store(path: &std::path::Path) -> SqliteGroupStore {
    let pool = SqlitePool::open(path).expect("open file sqlite");
    {
        let writer = pool.writer();
        migrate_groups(&writer).expect("migrate group tables");
    }
    SqliteGroupStore::new(pool)
}

fn open_service(path: &std::path::Path) -> GroupService {
    let store: Arc<dyn GroupStore> = Arc::new(open_store(path));
    GroupService::new(store)
}

fn group_with_worker(owner: AgentId, worker: GroupMember) -> Group {
    let now = Utc::now();
    Group {
        id: GroupId::new(),
        name: "durable-research".to_string(),
        description: "restart-safe group definition".to_string(),
        owner_agent_id: owner,
        members: vec![GroupMember::new(owner, MemberRole::Leader), worker],
        quorum_policy: QuorumPolicy::Threshold { fraction: 0.75 },
        budget: GroupBudget::new(10_000, Some(2_000), Some(500)),
        created_at: now,
        updated_at: now,
    }
}

fn fixture_path() -> (TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("create temp directory");
    let path = directory.path().join("groups.sqlite3");
    (directory, path)
}

#[tokio::test]
async fn public_service_round_trip_preserves_exact_ids_policy_budget_and_grant_narrowing() {
    let (_directory, path) = fixture_path();
    let owner = AgentId::new();
    let worker_id = AgentId::new();
    let override_grant = GrantSpec {
        capabilities: vec!["chain.read".to_string()],
        max_budget: Some(20.0),
        allowed_pallets: vec!["System".to_string()],
    };
    let worker = GroupMember::with_grant(worker_id, MemberRole::Worker, override_grant.clone());
    let mut group = group_with_worker(owner, worker);
    group.budget.member_spent.insert(worker_id.to_string(), 300);
    *group.budget.spent.lock() = 300;
    let exact_id = group.id;

    let service = open_service(&path);
    let created = service
        .create_group(group.clone())
        .await
        .expect("create valid group");
    assert_eq!(created.id, exact_id);
    drop(service);

    let restarted = open_service(&path);
    let loaded = restarted
        .get_group(&exact_id)
        .await
        .expect("load after reopen");
    assert_eq!(loaded.id, exact_id);
    assert_eq!(loaded.owner_agent_id, owner);
    assert_eq!(loaded.quorum_policy, group.quorum_policy);
    assert_eq!(loaded.budget.max_total, 10_000);
    assert_eq!(loaded.budget.max_per_member, Some(2_000));
    assert_eq!(loaded.budget.max_per_run, Some(500));
    assert_eq!(loaded.budget.total_spent(), 300);
    assert_eq!(
        loaded.budget.member_spent.get(&worker_id.to_string()),
        Some(&300)
    );

    let persisted_override = loaded
        .find_member(&worker_id)
        .and_then(|member| member.grant_override.as_ref())
        .expect("persisted grant override");
    assert_eq!(persisted_override, &override_grant);
    let group_base = GrantSpec {
        capabilities: vec!["chain.read".to_string(), "chain.write".to_string()],
        max_budget: Some(100.0),
        allowed_pallets: vec!["System".to_string(), "Balances".to_string()],
    };
    let effective = group_base.intersect(persisted_override);
    assert_eq!(effective.capabilities, vec!["chain.read"]);
    assert_eq!(effective.max_budget, Some(20.0));
    assert_eq!(effective.allowed_pallets, vec!["System"]);

    let updated = restarted
        .update_policy(
            &exact_id,
            GroupPolicyUpdate {
                quorum_policy: QuorumPolicy::Unanimous,
                max_total: 8_000,
                max_per_member: Some(1_000),
                max_per_run: Some(250),
            },
        )
        .await
        .expect("update policy");
    assert_eq!(updated.quorum_policy, QuorumPolicy::Unanimous);
    assert_eq!(updated.budget.total_spent(), 300);
    drop(restarted);

    let reopened = open_service(&path);
    let final_group = reopened
        .get_group(&exact_id)
        .await
        .expect("load updated policy after second reopen");
    assert_eq!(final_group.budget.total_spent(), 300);
    assert_eq!(final_group.budget.max_total, 8_000);
    assert_eq!(final_group.members.len(), 2);
    let removed = reopened
        .remove_member(&exact_id, &worker_id)
        .await
        .expect("remove worker while retaining historical spend");
    assert_eq!(removed.members.len(), 1);
    assert_eq!(removed.budget.total_spent(), 300);
}

#[tokio::test]
async fn validation_duplicate_and_not_found_contracts_fail_closed() {
    let (_directory, path) = fixture_path();
    let service = open_service(&path);
    let owner = AgentId::new();
    let worker_id = AgentId::new();
    let worker = GroupMember::new(worker_id, MemberRole::Worker);
    let group = group_with_worker(owner, worker.clone());
    let group_id = group.id;

    let mut invalid = group.clone();
    invalid.id = GroupId::new();
    invalid.members.retain(|member| member.agent_id != owner);
    assert!(matches!(
        service.create_group(invalid).await,
        Err(GroupError::InvalidGroup(_, _))
    ));
    assert!(service
        .list_groups()
        .await
        .expect("list valid groups")
        .is_empty());

    service
        .create_group(group.clone())
        .await
        .expect("first create");
    let identical = service
        .create_group(group.clone())
        .await
        .expect("identical retry");
    assert_eq!(identical.id, group_id);
    assert_eq!(service.list_groups().await.expect("list").len(), 1);

    let mut conflicting = group;
    conflicting.name = "different-state-same-id".to_string();
    assert!(matches!(
        service.create_group(conflicting).await,
        Err(GroupError::AlreadyExists(id)) if id == group_id
    ));

    let without_worker = service
        .remove_member(&group_id, &worker_id)
        .await
        .expect("remove non-owner member");
    assert!(!without_worker.is_member(&worker_id));
    service
        .add_member(&group_id, worker.clone())
        .await
        .expect("add worker again");
    service
        .add_member(&group_id, worker)
        .await
        .expect("identical member retry");
    assert!(matches!(
        service
            .add_member(
                &group_id,
                GroupMember::new(worker_id, MemberRole::Observer),
            )
            .await,
        Err(GroupError::AlreadyMember(agent, id)) if agent == worker_id && id == group_id
    ));

    assert!(matches!(
        service.remove_member(&group_id, &owner).await,
        Err(GroupError::PermissionDenied(agent, id, _)) if agent == owner && id == group_id
    ));
    let stranger = AgentId::new();
    assert!(matches!(
        service.remove_member(&group_id, &stranger).await,
        Err(GroupError::NotMember(agent, id)) if agent == stranger && id == group_id
    ));

    assert!(matches!(
        service
            .update_policy(
                &group_id,
                GroupPolicyUpdate {
                    quorum_policy: QuorumPolicy::Threshold { fraction: f64::NAN },
                    max_total: 10_000,
                    max_per_member: None,
                    max_per_run: None,
                },
            )
            .await,
        Err(GroupError::InvalidGroup(id, _)) if id == group_id
    ));

    service
        .delete_group(&group_id)
        .await
        .expect("delete existing");
    assert!(matches!(
        service.delete_group(&group_id).await,
        Err(GroupError::NotFound(id)) if id == group_id
    ));
    assert!(matches!(
        service.get_group(&GroupId::new()).await,
        Err(GroupError::NotFound(_))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_exact_retries_are_idempotent_and_policy_does_not_lose_members() {
    let (_directory, path) = fixture_path();
    let service = open_service(&path);
    let owner = AgentId::new();
    let initial_worker = GroupMember::new(AgentId::new(), MemberRole::Worker);
    let group = group_with_worker(owner, initial_worker);
    let group_id = group.id;

    let mut create_tasks = Vec::new();
    for _ in 0..16 {
        let service = service.clone();
        let group = group.clone();
        create_tasks.push(tokio::spawn(
            async move { service.create_group(group).await },
        ));
    }
    for task in create_tasks {
        let created = task.await.expect("join create").expect("idempotent create");
        assert_eq!(created.id, group_id);
    }
    assert_eq!(service.list_groups().await.expect("list").len(), 1);

    let repeated_member = GroupMember::with_grant(
        AgentId::new(),
        MemberRole::Worker,
        GrantSpec {
            capabilities: vec!["chain.read".to_string()],
            max_budget: Some(5.0),
            allowed_pallets: Vec::new(),
        },
    );
    let mut add_tasks = Vec::new();
    for _ in 0..16 {
        let service = service.clone();
        let member = repeated_member.clone();
        add_tasks.push(tokio::spawn(async move {
            service.add_member(&group_id, member).await
        }));
    }
    for task in add_tasks {
        task.await.expect("join add").expect("idempotent add");
    }

    let mut mixed_tasks = Vec::new();
    for _ in 0..8 {
        let service = service.clone();
        mixed_tasks.push(tokio::spawn(async move {
            service
                .add_member(
                    &group_id,
                    GroupMember::new(AgentId::new(), MemberRole::Observer),
                )
                .await
                .map(|_| ())
        }));
    }
    for _ in 0..8 {
        let service = service.clone();
        mixed_tasks.push(tokio::spawn(async move {
            service
                .update_policy(
                    &group_id,
                    GroupPolicyUpdate {
                        quorum_policy: QuorumPolicy::LeaderOnly,
                        max_total: 9_000,
                        max_per_member: Some(2_000),
                        max_per_run: Some(400),
                    },
                )
                .await
                .map(|_| ())
        }));
    }
    for task in mixed_tasks {
        task.await
            .expect("join mixed operation")
            .expect("mixed operation");
    }
    drop(service);

    let restarted = open_service(&path);
    let loaded = restarted
        .get_group(&group_id)
        .await
        .expect("load after concurrent operations and restart");
    assert_eq!(loaded.members.len(), 11);
    assert_eq!(
        loaded
            .members
            .iter()
            .filter(|member| member.agent_id == repeated_member.agent_id)
            .count(),
        1
    );
    assert_eq!(loaded.quorum_policy, QuorumPolicy::LeaderOnly);
    assert_eq!(loaded.budget.max_total, 9_000);
}

#[tokio::test]
async fn sqlite_create_and_update_member_batches_roll_back_atomically() {
    let (_directory, path) = fixture_path();
    let store = open_store(&path);
    let owner = AgentId::new();
    let duplicate = GroupMember::new(owner, MemberRole::Leader);
    let mut invalid_create = group_with_worker(owner, duplicate.clone());
    invalid_create.members.push(duplicate);
    let create_id = invalid_create.id;
    assert!(matches!(
        store.create_group(invalid_create).await,
        Err(GroupError::AlreadyMember(_, id)) if id == create_id
    ));
    assert!(matches!(
        store.get_group(&create_id).await,
        Err(GroupError::NotFound(id)) if id == create_id
    ));

    let valid = group_with_worker(
        AgentId::new(),
        GroupMember::new(AgentId::new(), MemberRole::Worker),
    );
    let valid_id = valid.id;
    store
        .create_group(valid.clone())
        .await
        .expect("create valid baseline");
    let mut invalid_update = valid.clone();
    invalid_update.name = "must-roll-back".to_string();
    invalid_update.members.push(valid.members[0].clone());
    assert!(matches!(
        store.update_group(invalid_update).await,
        Err(GroupError::AlreadyMember(_, id)) if id == valid_id
    ));
    drop(store);

    let restarted = open_store(&path);
    assert!(matches!(
        restarted.get_group(&create_id).await,
        Err(GroupError::NotFound(id)) if id == create_id
    ));
    let unchanged = restarted
        .get_group(&valid_id)
        .await
        .expect("baseline survives failed update and restart");
    assert_eq!(unchanged.name, valid.name);
    assert_eq!(unchanged.members.len(), valid.members.len());
}
