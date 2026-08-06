//! Async storage trait for group persistence.
//!
//! [`GroupStore`] defines the interface that all group storage backends must
//! implement. The in-memory implementation is provided in [`crate::memory_store`]
//! for testing; production use-cases should implement this trait against a
//! real database (e.g. `SQLite` via `polkagent-store-sqlite`).

use async_trait::async_trait;

use polkagent_core::ids::AgentId;

use crate::error::GroupResult;
use crate::types::{Group, GroupBudget, GroupId, GroupMember, QuorumPolicy};

// ---------------------------------------------------------------------------
// GroupStore trait
// ---------------------------------------------------------------------------

/// Async storage interface for [`Group`] records and their members.
///
/// All methods are async to support database-backed implementations.
/// Implementors must be `Send + Sync` so that stores can be wrapped in
/// `Arc<dyn GroupStore>` and shared across threads.
#[async_trait]
pub trait GroupStore: Send + Sync {
    // -----------------------------------------------------------------------
    // Group CRUD
    // -----------------------------------------------------------------------

    /// Persist a new group.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::GroupError::AlreadyExists`] if a group with
    /// the same ID already exists.
    async fn create_group(&self, group: Group) -> GroupResult<()>;

    /// Retrieve a group by its ID.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::GroupError::NotFound`] if no group with the
    /// given ID exists.
    async fn get_group(&self, group_id: &GroupId) -> GroupResult<Group>;

    /// Replace the stored group record with an updated version.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::GroupError::NotFound`] if the group does not
    /// exist.
    async fn update_group(&self, group: Group) -> GroupResult<()>;

    /// Atomically replace only the group's quorum and budget policy.
    ///
    /// This narrow mutation must not replace membership or other group fields,
    /// so a concurrent member change cannot be lost.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::GroupError::NotFound`] if the group does not
    /// exist.
    async fn update_policy(
        &self,
        group_id: &GroupId,
        quorum_policy: QuorumPolicy,
        budget: GroupBudget,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> GroupResult<()>;

    /// Delete a group and all its associated membership records.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::GroupError::NotFound`] if the group does not
    /// exist.
    async fn delete_group(&self, group_id: &GroupId) -> GroupResult<()>;

    /// List all groups managed by this store.
    async fn list_groups(&self) -> GroupResult<Vec<Group>>;

    // -----------------------------------------------------------------------
    // Member management
    // -----------------------------------------------------------------------

    /// Add a member to the specified group.
    ///
    /// # Errors
    ///
    /// - [`crate::error::GroupError::NotFound`] if the group does not exist.
    /// - [`crate::error::GroupError::AlreadyMember`] if the agent is already a
    ///   member.
    async fn add_member(&self, group_id: &GroupId, member: GroupMember) -> GroupResult<()>;

    /// Remove a member from the specified group.
    ///
    /// # Errors
    ///
    /// - [`crate::error::GroupError::NotFound`] if the group does not exist.
    /// - [`crate::error::GroupError::NotMember`] if the agent is not a member.
    async fn remove_member(&self, group_id: &GroupId, agent_id: &AgentId) -> GroupResult<()>;

    /// List all members of the specified group.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::GroupError::NotFound`] if the group does not
    /// exist.
    async fn list_members(&self, group_id: &GroupId) -> GroupResult<Vec<GroupMember>>;
}
