//! In-memory implementation of [`GroupStore`] for tests and prototyping.
//!
//! [`MemoryGroupStore`] stores all group data in a `tokio::sync::RwLock`-
//! protected `HashMap`. It is not suitable for production use (no durability,
//! no persistence across restarts), but provides a fast, dependency-free
//! backend for unit and integration tests.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use polkagent_core::ids::AgentId;

use crate::error::{GroupError, GroupResult};
use crate::store::GroupStore;
use crate::types::{Group, GroupId, GroupMember};

// ---------------------------------------------------------------------------
// MemoryGroupStore
// ---------------------------------------------------------------------------

/// An in-memory [`GroupStore`] backed by a `RwLock<HashMap>`.
///
/// Wrap in `Arc<MemoryGroupStore>` to share across async tasks.
#[derive(Debug, Default)]
pub struct MemoryGroupStore {
    inner: RwLock<HashMap<GroupId, Group>>,
}

impl MemoryGroupStore {
    /// Create a new, empty store.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[async_trait]
impl GroupStore for MemoryGroupStore {
    async fn create_group(&self, group: Group) -> GroupResult<()> {
        let mut map = self.inner.write().await;
        if map.contains_key(&group.id) {
            return Err(GroupError::AlreadyExists(group.id));
        }
        map.insert(group.id, group);
        Ok(())
    }

    async fn get_group(&self, group_id: &GroupId) -> GroupResult<Group> {
        let map = self.inner.read().await;
        map.get(group_id)
            .cloned()
            .ok_or(GroupError::NotFound(*group_id))
    }

    async fn update_group(&self, group: Group) -> GroupResult<()> {
        let mut map = self.inner.write().await;
        if !map.contains_key(&group.id) {
            return Err(GroupError::NotFound(group.id));
        }
        map.insert(group.id, group);
        Ok(())
    }

    async fn delete_group(&self, group_id: &GroupId) -> GroupResult<()> {
        let mut map = self.inner.write().await;
        map.remove(group_id)
            .ok_or(GroupError::NotFound(*group_id))?;
        Ok(())
    }

    async fn list_groups(&self) -> GroupResult<Vec<Group>> {
        let map = self.inner.read().await;
        Ok(map.values().cloned().collect())
    }

    async fn add_member(&self, group_id: &GroupId, member: GroupMember) -> GroupResult<()> {
        let mut map = self.inner.write().await;
        let group = map
            .get_mut(group_id)
            .ok_or(GroupError::NotFound(*group_id))?;

        if group.is_member(&member.agent_id) {
            return Err(GroupError::Internal(format!(
                "agent {} is already a member of group {}",
                member.agent_id, group_id
            )));
        }

        group.members.push(member);
        Ok(())
    }

    async fn remove_member(&self, group_id: &GroupId, agent_id: &AgentId) -> GroupResult<()> {
        let mut map = self.inner.write().await;
        let group = map
            .get_mut(group_id)
            .ok_or(GroupError::NotFound(*group_id))?;

        let initial_len = group.members.len();
        group.members.retain(|m| &m.agent_id != agent_id);

        if group.members.len() == initial_len {
            return Err(GroupError::NotMember(*agent_id, *group_id));
        }

        Ok(())
    }

    async fn list_members(&self, group_id: &GroupId) -> GroupResult<Vec<GroupMember>> {
        let map = self.inner.read().await;
        let group = map
            .get(group_id)
            .ok_or(GroupError::NotFound(*group_id))?;
        Ok(group.members.clone())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::GroupStore;
    use crate::types::{GroupBudget, GroupMember, MemberRole, QuorumPolicy};
    use polkagent_core::ids::AgentId;

    fn make_group(id: GroupId, owner: AgentId) -> Group {
        let now = chrono::Utc::now();
        Group {
            id,
            name: "test".to_string(),
            description: String::new(),
            owner_agent_id: owner,
            members: vec![GroupMember::new(owner, MemberRole::Leader)],
            quorum_policy: QuorumPolicy::Majority,
            budget: GroupBudget::new(1000, None, None),
            created_at: now,
            updated_at: now,
        }
    }

    // ---- create / get / update / delete / list ----------------------------

    #[tokio::test]
    async fn create_and_get_group_round_trip() {
        let store = MemoryGroupStore::new();
        let owner = AgentId::new();
        let id = GroupId::new();
        let group = make_group(id, owner);

        store.create_group(group.clone()).await.expect("create ok");
        let retrieved = store.get_group(&id).await.expect("get ok");
        assert_eq!(retrieved.id, id);
        assert_eq!(retrieved.owner_agent_id, owner);
    }

    #[tokio::test]
    async fn create_duplicate_returns_already_exists() {
        let store = MemoryGroupStore::new();
        let owner = AgentId::new();
        let id = GroupId::new();
        let group = make_group(id, owner);

        store.create_group(group.clone()).await.expect("first ok");
        assert!(matches!(
            store.create_group(group).await,
            Err(GroupError::AlreadyExists(_))
        ));
    }

    #[tokio::test]
    async fn get_missing_group_returns_not_found() {
        let store = MemoryGroupStore::new();
        let missing = GroupId::new();
        assert!(matches!(
            store.get_group(&missing).await,
            Err(GroupError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn update_group_replaces_record() {
        let store = MemoryGroupStore::new();
        let owner = AgentId::new();
        let id = GroupId::new();
        let mut group = make_group(id, owner);
        store.create_group(group.clone()).await.expect("create ok");

        group.name = "updated".to_string();
        store.update_group(group.clone()).await.expect("update ok");

        let retrieved = store.get_group(&id).await.expect("get ok");
        assert_eq!(retrieved.name, "updated");
    }

    #[tokio::test]
    async fn update_missing_group_returns_not_found() {
        let store = MemoryGroupStore::new();
        let owner = AgentId::new();
        let group = make_group(GroupId::new(), owner);
        assert!(matches!(
            store.update_group(group).await,
            Err(GroupError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn delete_group_removes_it() {
        let store = MemoryGroupStore::new();
        let owner = AgentId::new();
        let id = GroupId::new();
        store.create_group(make_group(id, owner)).await.expect("ok");

        store.delete_group(&id).await.expect("delete ok");
        assert!(matches!(
            store.get_group(&id).await,
            Err(GroupError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn delete_missing_group_returns_not_found() {
        let store = MemoryGroupStore::new();
        let missing = GroupId::new();
        assert!(matches!(
            store.delete_group(&missing).await,
            Err(GroupError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn list_groups_returns_all() {
        let store = MemoryGroupStore::new();
        let owner = AgentId::new();
        for _ in 0..4 {
            store
                .create_group(make_group(GroupId::new(), owner))
                .await
                .expect("ok");
        }
        let groups = store.list_groups().await.expect("list ok");
        assert_eq!(groups.len(), 4);
    }

    // ---- Membership via store --------------------------------------------

    #[tokio::test]
    async fn add_member_via_store_appears_in_list() {
        let store = MemoryGroupStore::new();
        let owner = AgentId::new();
        let id = GroupId::new();
        store.create_group(make_group(id, owner)).await.expect("ok");

        let worker = AgentId::new();
        store
            .add_member(&id, GroupMember::new(worker, MemberRole::Worker))
            .await
            .expect("add ok");

        let members = store.list_members(&id).await.expect("list ok");
        assert_eq!(members.len(), 2);
        assert!(members.iter().any(|m| m.agent_id == worker));
    }

    #[tokio::test]
    async fn add_duplicate_member_returns_error() {
        let store = MemoryGroupStore::new();
        let owner = AgentId::new();
        let id = GroupId::new();
        store.create_group(make_group(id, owner)).await.expect("ok");

        let result = store
            .add_member(&id, GroupMember::new(owner, MemberRole::Worker))
            .await;
        assert!(result.is_err(), "duplicate member should fail");
    }

    #[tokio::test]
    async fn remove_member_via_store_removes_correctly() {
        let store = MemoryGroupStore::new();
        let owner = AgentId::new();
        let id = GroupId::new();
        store.create_group(make_group(id, owner)).await.expect("ok");

        let worker = AgentId::new();
        store
            .add_member(&id, GroupMember::new(worker, MemberRole::Worker))
            .await
            .expect("add ok");

        store.remove_member(&id, &worker).await.expect("remove ok");

        let members = store.list_members(&id).await.expect("list ok");
        assert_eq!(members.len(), 1);
        assert!(!members.iter().any(|m| m.agent_id == worker));
    }

    #[tokio::test]
    async fn remove_non_member_returns_not_member() {
        let store = MemoryGroupStore::new();
        let owner = AgentId::new();
        let id = GroupId::new();
        store.create_group(make_group(id, owner)).await.expect("ok");

        let stranger = AgentId::new();
        assert!(matches!(
            store.remove_member(&id, &stranger).await,
            Err(GroupError::NotMember(..))
        ));
    }

    // ---- Concurrent budget tracking (using coordinator) ------------------

    #[tokio::test]
    async fn concurrent_spend_tracking_does_not_race() {
        use crate::coordinator::GroupCoordinator;
        use std::sync::{Arc, Mutex};

        let mut coord = GroupCoordinator::new();
        let owner = AgentId::new();
        let group = coord.create_group("concurrent-test", owner);
        let group_id = group.id;
        coord
            .set_budget(&group_id, crate::types::GroupBudget::new(10_000, None, None))
            .expect("set ok");

        // Add 9 workers.
        let mut workers: Vec<AgentId> = Vec::new();
        for _ in 0..9 {
            let w = AgentId::new();
            coord
                .add_member(&group_id, GroupMember::new(w, MemberRole::Worker))
                .expect("add ok");
            workers.push(w);
        }

        // Wrap coordinator in Arc<Mutex> for sharing across threads.
        let coord = Arc::new(Mutex::new(coord));
        let mut handles = vec![];

        for worker in workers {
            let coord_clone = Arc::clone(&coord);
            let handle = std::thread::spawn(move || {
                let mut c = coord_clone.lock().expect("lock ok");
                c.record_spend(&group_id, &worker, 100.0).expect("spend ok");
            });
            handles.push(handle);
        }

        for h in handles {
            h.join().expect("thread ok");
        }

        let c = coord.lock().expect("lock ok");
        let group = c.get_group(&group_id).expect("exists");
        // 9 workers * 100 = 900
        assert_eq!(group.budget.total_spent(), 900);
    }
}
