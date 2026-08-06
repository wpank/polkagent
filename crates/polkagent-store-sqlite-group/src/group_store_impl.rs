//! [`GroupStore`] trait implementation for [`SqliteGroupStore`].
//!
//! Wraps synchronous `rusqlite` calls in [`tokio::task::spawn_blocking`] to
//! satisfy the async trait interface.  The writer connection is protected by a
//! `parking_lot::Mutex` inside [`SqlitePool`], so each method acquires it
//! briefly within the blocking closure.
//!
//! # Why a separate crate?
//!
//! The `polkagent-group` type graph — specifically [`GroupBudget`] which
//! contains `Arc<parking_lot::Mutex<u64>>` — pushes `rustc`'s trait-solver
//! recursion limit when compiled alongside the other serde-heavy store
//! modules in `polkagent-store-sqlite`.  Isolating this implementation in its
//! own crate keeps each compilation unit below the limit without requiring a
//! global `#![recursion_limit]` bump.
//!
//! # Serialization choices
//!
//! - [`QuorumPolicy`] is stored as JSON (tagged enum with `kind` discriminant).
//! - [`GroupBudget`] is stored as a JSON object containing the persisted
//!   fields (`max_total`, `max_per_member`, `max_per_run`, `total_spent`,
//!   `member_spent`). Legacy rows without `total_spent` reconstruct it from the
//!   checked `member_spent` sum.
//! - [`GrantSpec`] overrides on members are stored as JSON (`NULL` when
//!   absent).
//! - [`MemberRole`] is stored as its lowercase string (`leader`, `worker`,
//!   `observer`).

use async_trait::async_trait;
use chrono::DateTime;

use polkagent_core::ids::AgentId;
use polkagent_group::{
    types::{GrantSpec, Group, GroupBudget, GroupId, GroupMember, MemberRole, QuorumPolicy},
    GroupError, GroupResult, GroupStore,
};
use polkagent_store_sqlite::SqlitePool;

use crate::error::{is_constraint_violation, map_err};

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse an ISO-8601 timestamp string.
fn parse_ts(s: &str) -> GroupResult<chrono::DateTime<chrono::Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| GroupError::Internal(format!("invalid timestamp '{s}': {e}")))
}

/// Parse a [`GroupId`] from its string form.
fn parse_group_id(s: &str) -> GroupResult<GroupId> {
    s.parse::<GroupId>()
        .map_err(|e| GroupError::Internal(format!("invalid group id '{s}': {e}")))
}

/// Parse an [`AgentId`] from its string form.
fn parse_agent_id(s: &str) -> GroupResult<AgentId> {
    s.parse::<AgentId>()
        .map_err(|e| GroupError::Internal(format!("invalid agent id '{s}': {e}")))
}

/// Encode a [`MemberRole`] as its stored string.
fn encode_role(role: MemberRole) -> &'static str {
    match role {
        MemberRole::Leader => "leader",
        MemberRole::Worker => "worker",
        MemberRole::Observer => "observer",
    }
}

/// Decode a [`MemberRole`] from its stored string.
fn decode_role(s: &str) -> GroupResult<MemberRole> {
    match s {
        "leader" => Ok(MemberRole::Leader),
        "worker" => Ok(MemberRole::Worker),
        "observer" => Ok(MemberRole::Observer),
        other => Err(GroupError::Internal(format!(
            "unknown member role: '{other}'"
        ))),
    }
}

/// Serialize a [`QuorumPolicy`] to JSON for storage.
fn encode_quorum_policy(policy: &QuorumPolicy) -> GroupResult<String> {
    serde_json::to_string(policy)
        .map_err(|e| GroupError::Internal(format!("failed to serialize quorum policy: {e}")))
}

/// Deserialize a [`QuorumPolicy`] from its stored JSON.
fn decode_quorum_policy(s: &str) -> GroupResult<QuorumPolicy> {
    serde_json::from_str(s).map_err(|e| {
        GroupError::Internal(format!("failed to deserialize quorum policy '{s}': {e}"))
    })
}

/// Serialize a [`GroupBudget`] to JSON for storage.
///
/// Uses [`serde_json::Value`] directly rather than a derived struct to avoid
/// serde-derive trait-solver recursion depth issues when `GroupBudget`
/// (which contains `Arc<parking_lot::Mutex<u64>>`) is in scope.
fn encode_budget(budget: &GroupBudget) -> GroupResult<String> {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "max_total".to_string(),
        serde_json::Value::from(budget.max_total),
    );
    obj.insert(
        "max_per_member".to_string(),
        budget
            .max_per_member
            .map_or(serde_json::Value::Null, serde_json::Value::from),
    );
    obj.insert(
        "max_per_run".to_string(),
        budget
            .max_per_run
            .map_or(serde_json::Value::Null, serde_json::Value::from),
    );
    obj.insert(
        "total_spent".to_string(),
        serde_json::Value::from(budget.total_spent()),
    );
    let spent_map: serde_json::Map<String, serde_json::Value> = budget
        .member_spent
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::from(*v)))
        .collect();
    obj.insert(
        "member_spent".to_string(),
        serde_json::Value::Object(spent_map),
    );
    serde_json::to_string(&serde_json::Value::Object(obj))
        .map_err(|e| GroupError::Internal(format!("failed to serialize budget: {e}")))
}

/// Deserialize a [`GroupBudget`] from its stored JSON.
fn decode_budget(s: &str) -> GroupResult<GroupBudget> {
    let val: serde_json::Value = serde_json::from_str(s)
        .map_err(|e| GroupError::Internal(format!("failed to deserialize budget '{s}': {e}")))?;

    let get_u64 = |key: &str| -> GroupResult<u64> {
        val.get(key)
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| GroupError::Internal(format!("budget missing or invalid field '{key}'")))
    };
    let get_opt_u64 = |key: &str| -> GroupResult<Option<u64>> {
        match val.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .map(Some)
                .ok_or_else(|| GroupError::Internal(format!("budget invalid field '{key}'"))),
        }
    };

    let max_total = get_u64("max_total")?;
    let max_per_member = get_opt_u64("max_per_member")?;
    let max_per_run = get_opt_u64("max_per_run")?;

    let mut budget = GroupBudget::new(max_total, max_per_member, max_per_run);

    let mut inferred_total = 0_u64;
    match val.get("member_spent") {
        None => {}
        Some(serde_json::Value::Object(spent_map)) => {
            for (agent_id, amount) in spent_map {
                let n = amount.as_u64().ok_or_else(|| {
                    GroupError::Internal(format!(
                        "budget has invalid member spend for '{agent_id}'"
                    ))
                })?;
                inferred_total = inferred_total.checked_add(n).ok_or_else(|| {
                    GroupError::Internal("budget member spend total overflow".to_string())
                })?;
                budget.member_spent.insert(agent_id.clone(), n);
            }
        }
        Some(_) => {
            return Err(GroupError::Internal(
                "budget invalid field 'member_spent'".to_string(),
            ));
        }
    }

    let total_spent = match val.get("total_spent") {
        None => inferred_total,
        Some(value) => value.as_u64().ok_or_else(|| {
            GroupError::Internal("budget invalid field 'total_spent'".to_string())
        })?,
    };
    *budget.spent.lock() = total_spent;

    Ok(budget)
}

/// Serialize an optional [`GrantSpec`] to JSON (`NULL` for `None`).
fn encode_grant_override(grant: Option<&GrantSpec>) -> GroupResult<Option<String>> {
    match grant {
        None => Ok(None),
        Some(g) => serde_json::to_string(g)
            .map(Some)
            .map_err(|e| GroupError::Internal(format!("failed to serialize grant spec: {e}"))),
    }
}

/// Deserialize an optional [`GrantSpec`] from its stored JSON.
fn decode_grant_override(s: Option<String>) -> GroupResult<Option<GrantSpec>> {
    match s {
        None => Ok(None),
        Some(json) => serde_json::from_str(&json).map(Some).map_err(|e| {
            GroupError::Internal(format!("failed to deserialize grant spec '{json}': {e}"))
        }),
    }
}

// ---------------------------------------------------------------------------
// Row -> GroupMember
// ---------------------------------------------------------------------------

/// Raw column tuple from `group_members`.
///
/// Columns: `agent_id`, role, `grant_override_json`, `joined_at`
type RawMemberRow = (String, String, Option<String>, String);

fn raw_to_member(raw: RawMemberRow) -> GroupResult<GroupMember> {
    let (agent_id_s, role_s, grant_json, joined_at_s) = raw;
    Ok(GroupMember {
        agent_id: parse_agent_id(&agent_id_s)?,
        role: decode_role(&role_s)?,
        grant_override: decode_grant_override(grant_json)?,
        joined_at: parse_ts(&joined_at_s)?,
    })
}

// ---------------------------------------------------------------------------
// Newtype wrapper (orphan-rule workaround)
// ---------------------------------------------------------------------------

/// A thin wrapper around [`SqlitePool`] that allows this crate to implement
/// the [`GroupStore`] trait without violating Rust's orphan rule (both
/// `GroupStore` and `SqlitePool` are defined in external crates).
///
/// Construction is cheap — `SqlitePool` is `Arc`-based, so cloning the inner
/// value is just a reference-count bump.
#[derive(Debug, Clone)]
pub struct SqliteGroupStore(pub SqlitePool);

impl SqliteGroupStore {
    /// Create a new `SqliteGroupStore` wrapping the given pool.
    pub fn new(pool: SqlitePool) -> Self {
        Self(pool)
    }
}

impl std::ops::Deref for SqliteGroupStore {
    type Target = SqlitePool;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// GroupStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl GroupStore for SqliteGroupStore {
    // -----------------------------------------------------------------------
    // create_group
    // -----------------------------------------------------------------------

    async fn create_group(&self, group: Group) -> GroupResult<()> {
        let pool = self.0.clone();

        // Pre-serialize outside the blocking closure to surface errors early.
        let id_str = group.id.to_string();
        let owner_str = group.owner_agent_id.to_string();
        let quorum_json = encode_quorum_policy(&group.quorum_policy)?;
        let budget_json = encode_budget(&group.budget)?;
        let created_str = group.created_at.to_rfc3339();
        let updated_str = group.updated_at.to_rfc3339();
        let name = group.name.clone();
        let description = group.description.clone();
        // Serialize members for batch insert.
        let members: Vec<(AgentId, String, String, Option<String>, String)> = group
            .members
            .iter()
            .map(|m| {
                let grant_json = encode_grant_override(m.grant_override.as_ref())?;
                Ok((
                    m.agent_id,
                    m.agent_id.to_string(),
                    encode_role(m.role).to_string(),
                    grant_json,
                    m.joined_at.to_rfc3339(),
                ))
            })
            .collect::<GroupResult<_>>()?;

        let group_id_for_err = group.id;

        tokio::task::spawn_blocking(move || {
            let mut writer = pool.writer();
            let transaction = writer.transaction().map_err(map_err)?;

            // Insert the group row.
            transaction
                .execute(
                    "INSERT INTO groups
                         (id, name, description, owner_agent_id,
                          quorum_policy, budget_json, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        id_str,
                        name,
                        description,
                        owner_str,
                        quorum_json,
                        budget_json,
                        created_str,
                        updated_str,
                    ],
                )
                .map_err(|e| {
                    if is_constraint_violation(&e) {
                        GroupError::AlreadyExists(group_id_for_err)
                    } else {
                        map_err(e)
                    }
                })?;

            // Insert members.
            for (agent_id, agent_id_s, role_s, grant_json, joined_at_s) in members {
                transaction
                    .execute(
                        "INSERT INTO group_members
                             (group_id, agent_id, role, grant_override_json, joined_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![id_str, agent_id_s, role_s, grant_json, joined_at_s,],
                    )
                    .map_err(|error| {
                        if is_constraint_violation(&error) {
                            GroupError::AlreadyMember(agent_id, group_id_for_err)
                        } else {
                            map_err(error)
                        }
                    })?;
            }

            transaction.commit().map_err(map_err)?;
            Ok(())
        })
        .await
        .map_err(|e| GroupError::Internal(format!("blocking task panicked: {e}")))?
    }

    // -----------------------------------------------------------------------
    // get_group
    // -----------------------------------------------------------------------

    async fn get_group(&self, group_id: &GroupId) -> GroupResult<Group> {
        let pool = self.0.clone();
        let id_str = group_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // Fetch the group row.
            let (id_s, name, description, owner_s, quorum_s, budget_s, created_s, updated_s) =
                writer
                    .query_row(
                        "SELECT id, name, description, owner_agent_id,
                             quorum_policy, budget_json, created_at, updated_at
                      FROM groups
                      WHERE id = ?1",
                        [&id_str],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, String>(5)?,
                                row.get::<_, String>(6)?,
                                row.get::<_, String>(7)?,
                            ))
                        },
                    )
                    .map_err(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => {
                            GroupError::NotFound(id_str.parse().unwrap_or_default())
                        }
                        other => map_err(other),
                    })?;

            let group_id_parsed = parse_group_id(&id_s)?;
            let owner_id = parse_agent_id(&owner_s)?;
            let quorum_policy = decode_quorum_policy(&quorum_s)?;
            let budget = decode_budget(&budget_s)?;
            let created_at = parse_ts(&created_s)?;
            let updated_at = parse_ts(&updated_s)?;

            // Fetch members.
            let mut stmt = writer
                .prepare(
                    "SELECT agent_id, role, grant_override_json, joined_at
                     FROM group_members
                     WHERE group_id = ?1
                     ORDER BY joined_at ASC",
                )
                .map_err(map_err)?;

            let raw_rows: Vec<RawMemberRow> = stmt
                .query_map([&id_s], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(map_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_err)?;

            let members: Vec<GroupMember> = raw_rows
                .into_iter()
                .map(raw_to_member)
                .collect::<GroupResult<_>>()?;

            Ok(Group {
                id: group_id_parsed,
                name,
                description,
                owner_agent_id: owner_id,
                members,
                quorum_policy,
                budget,
                created_at,
                updated_at,
            })
        })
        .await
        .map_err(|e| GroupError::Internal(format!("blocking task panicked: {e}")))?
    }

    // -----------------------------------------------------------------------
    // update_group
    // -----------------------------------------------------------------------

    async fn update_group(&self, group: Group) -> GroupResult<()> {
        let pool = self.0.clone();

        let id_str = group.id.to_string();
        let owner_str = group.owner_agent_id.to_string();
        let quorum_json = encode_quorum_policy(&group.quorum_policy)?;
        let budget_json = encode_budget(&group.budget)?;
        let updated_str = chrono::Utc::now().to_rfc3339();
        let name = group.name.clone();
        let description = group.description.clone();
        let group_id_for_err = group.id;

        // Prepare new member rows.
        let members: Vec<(AgentId, String, String, Option<String>, String)> = group
            .members
            .iter()
            .map(|m| {
                let grant_json = encode_grant_override(m.grant_override.as_ref())?;
                Ok((
                    m.agent_id,
                    m.agent_id.to_string(),
                    encode_role(m.role).to_string(),
                    grant_json,
                    m.joined_at.to_rfc3339(),
                ))
            })
            .collect::<GroupResult<_>>()?;

        tokio::task::spawn_blocking(move || {
            let mut writer = pool.writer();
            let transaction = writer.transaction().map_err(map_err)?;

            let n = transaction
                .execute(
                    "UPDATE groups
                     SET name = ?1, description = ?2, owner_agent_id = ?3,
                         quorum_policy = ?4, budget_json = ?5, updated_at = ?6
                     WHERE id = ?7",
                    rusqlite::params![
                        name,
                        description,
                        owner_str,
                        quorum_json,
                        budget_json,
                        updated_str,
                        id_str,
                    ],
                )
                .map_err(map_err)?;

            if n == 0 {
                return Err(GroupError::NotFound(id_str.parse().unwrap_or_default()));
            }

            // Replace members: delete all then re-insert.  This is simpler
            // and correct given that update_group owns the full new state.
            transaction
                .execute("DELETE FROM group_members WHERE group_id = ?1", [&id_str])
                .map_err(map_err)?;

            for (agent_id, agent_id_s, role_s, grant_json, joined_at_s) in members {
                transaction
                    .execute(
                        "INSERT INTO group_members
                             (group_id, agent_id, role, grant_override_json, joined_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![id_str, agent_id_s, role_s, grant_json, joined_at_s,],
                    )
                    .map_err(|error| {
                        if is_constraint_violation(&error) {
                            GroupError::AlreadyMember(agent_id, group_id_for_err)
                        } else {
                            map_err(error)
                        }
                    })?;
            }

            transaction.commit().map_err(map_err)?;
            Ok(())
        })
        .await
        .map_err(|e| GroupError::Internal(format!("blocking task panicked: {e}")))?
    }

    async fn update_policy(
        &self,
        group_id: &GroupId,
        quorum_policy: QuorumPolicy,
        budget: GroupBudget,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> GroupResult<()> {
        let pool = self.0.clone();
        let id_str = group_id.to_string();
        let group_id_for_err = *group_id;
        let quorum_json = encode_quorum_policy(&quorum_policy)?;
        let budget_json = encode_budget(&budget)?;
        let updated_str = updated_at.to_rfc3339();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let changed = writer
                .execute(
                    "UPDATE groups
                     SET quorum_policy = ?1, budget_json = ?2, updated_at = ?3
                     WHERE id = ?4",
                    rusqlite::params![quorum_json, budget_json, updated_str, id_str],
                )
                .map_err(map_err)?;
            if changed == 0 {
                return Err(GroupError::NotFound(group_id_for_err));
            }
            Ok(())
        })
        .await
        .map_err(|error| GroupError::Internal(format!("blocking task panicked: {error}")))?
    }

    // -----------------------------------------------------------------------
    // delete_group
    // -----------------------------------------------------------------------

    async fn delete_group(&self, group_id: &GroupId) -> GroupResult<()> {
        let pool = self.0.clone();
        let id_str = group_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            // Members are removed via ON DELETE CASCADE on the FK.
            let n = writer
                .execute("DELETE FROM groups WHERE id = ?1", [&id_str])
                .map_err(map_err)?;

            if n == 0 {
                return Err(GroupError::NotFound(id_str.parse().unwrap_or_default()));
            }
            Ok(())
        })
        .await
        .map_err(|e| GroupError::Internal(format!("blocking task panicked: {e}")))?
    }

    // -----------------------------------------------------------------------
    // list_groups
    // -----------------------------------------------------------------------

    async fn list_groups(&self) -> GroupResult<Vec<Group>> {
        let pool = self.0.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // Fetch all group rows.
            let mut stmt = writer
                .prepare(
                    "SELECT id, name, description, owner_agent_id,
                             quorum_policy, budget_json, created_at, updated_at
                     FROM groups
                     ORDER BY created_at ASC",
                )
                .map_err(map_err)?;

            let raw_groups: Vec<(
                String,
                String,
                String,
                String,
                String,
                String,
                String,
                String,
            )> = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                })
                .map_err(map_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_err)?;

            let mut groups = Vec::with_capacity(raw_groups.len());

            for (id_s, name, description, owner_s, quorum_s, budget_s, created_s, updated_s) in
                raw_groups
            {
                let group_id = parse_group_id(&id_s)?;
                let owner_id = parse_agent_id(&owner_s)?;
                let quorum_policy = decode_quorum_policy(&quorum_s)?;
                let budget = decode_budget(&budget_s)?;
                let created_at = parse_ts(&created_s)?;
                let updated_at = parse_ts(&updated_s)?;

                // Fetch members for this group.
                let mut mstmt = writer
                    .prepare(
                        "SELECT agent_id, role, grant_override_json, joined_at
                         FROM group_members
                         WHERE group_id = ?1
                         ORDER BY joined_at ASC",
                    )
                    .map_err(map_err)?;

                let raw_members: Vec<RawMemberRow> = mstmt
                    .query_map([&id_s], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    })
                    .map_err(map_err)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(map_err)?;

                let members: Vec<GroupMember> = raw_members
                    .into_iter()
                    .map(raw_to_member)
                    .collect::<GroupResult<_>>()?;

                groups.push(Group {
                    id: group_id,
                    name,
                    description,
                    owner_agent_id: owner_id,
                    members,
                    quorum_policy,
                    budget,
                    created_at,
                    updated_at,
                });
            }

            Ok(groups)
        })
        .await
        .map_err(|e| GroupError::Internal(format!("blocking task panicked: {e}")))?
    }

    // -----------------------------------------------------------------------
    // add_member
    // -----------------------------------------------------------------------

    async fn add_member(&self, group_id: &GroupId, member: GroupMember) -> GroupResult<()> {
        let pool = self.0.clone();
        let id_str = group_id.to_string();
        let agent_id_str = member.agent_id.to_string();
        let role_str = encode_role(member.role).to_string();
        let grant_json = encode_grant_override(member.grant_override.as_ref())?;
        let joined_at_str = member.joined_at.to_rfc3339();
        let group_id_for_err = *group_id;
        let agent_id_for_err = member.agent_id;

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // Verify the group exists.
            let exists: bool = writer
                .query_row(
                    "SELECT COUNT(*) FROM groups WHERE id = ?1",
                    [&id_str],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(map_err)
                .map(|n| n > 0)?;

            if !exists {
                return Err(GroupError::NotFound(id_str.parse().unwrap_or_default()));
            }

            writer
                .execute(
                    "INSERT INTO group_members
                         (group_id, agent_id, role, grant_override_json, joined_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![id_str, agent_id_str, role_str, grant_json, joined_at_str,],
                )
                .map_err(|e| {
                    if is_constraint_violation(&e) {
                        GroupError::AlreadyMember(agent_id_for_err, group_id_for_err)
                    } else {
                        map_err(e)
                    }
                })?;

            Ok(())
        })
        .await
        .map_err(|e| GroupError::Internal(format!("blocking task panicked: {e}")))?
    }

    // -----------------------------------------------------------------------
    // remove_member
    // -----------------------------------------------------------------------

    async fn remove_member(&self, group_id: &GroupId, agent_id: &AgentId) -> GroupResult<()> {
        let pool = self.0.clone();
        let id_str = group_id.to_string();
        let agent_str = agent_id.to_string();
        let gid_for_err = *group_id;
        let aid_for_err = *agent_id;

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // Check the group exists first.
            let group_exists: bool = writer
                .query_row(
                    "SELECT COUNT(*) FROM groups WHERE id = ?1",
                    [&id_str],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(map_err)
                .map(|n| n > 0)?;

            if !group_exists {
                return Err(GroupError::NotFound(gid_for_err));
            }

            let n = writer
                .execute(
                    "DELETE FROM group_members WHERE group_id = ?1 AND agent_id = ?2",
                    rusqlite::params![id_str, agent_str],
                )
                .map_err(map_err)?;

            if n == 0 {
                return Err(GroupError::NotMember(aid_for_err, gid_for_err));
            }

            Ok(())
        })
        .await
        .map_err(|e| GroupError::Internal(format!("blocking task panicked: {e}")))?
    }

    // -----------------------------------------------------------------------
    // list_members
    // -----------------------------------------------------------------------

    async fn list_members(&self, group_id: &GroupId) -> GroupResult<Vec<GroupMember>> {
        let pool = self.0.clone();
        let id_str = group_id.to_string();
        let gid_for_err = *group_id;

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // Verify the group exists.
            let group_exists: bool = writer
                .query_row(
                    "SELECT COUNT(*) FROM groups WHERE id = ?1",
                    [&id_str],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(map_err)
                .map(|n| n > 0)?;

            if !group_exists {
                return Err(GroupError::NotFound(gid_for_err));
            }

            let mut stmt = writer
                .prepare(
                    "SELECT agent_id, role, grant_override_json, joined_at
                     FROM group_members
                     WHERE group_id = ?1
                     ORDER BY joined_at ASC",
                )
                .map_err(map_err)?;

            let raw_rows: Vec<RawMemberRow> = stmt
                .query_map([&id_str], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(map_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_err)?;

            raw_rows
                .into_iter()
                .map(raw_to_member)
                .collect::<GroupResult<_>>()
        })
        .await
        .map_err(|e| GroupError::Internal(format!("blocking task panicked: {e}")))?
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::migrate_groups;
    use chrono::Utc;
    use polkagent_core::ids::AgentId;
    use polkagent_group::types::{
        GrantSpec, Group, GroupBudget, GroupId, GroupMember, MemberRole, QuorumPolicy,
    };
    use std::sync::Arc;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn test_store() -> SqliteGroupStore {
        let pool = SqlitePool::open_in_memory().expect("open in-memory pool");
        {
            let writer = pool.writer();
            migrate_groups(&writer).expect("migrate");
        }
        SqliteGroupStore::new(pool)
    }

    fn make_group(owner: AgentId, members: Vec<GroupMember>) -> Group {
        Group {
            id: GroupId::new(),
            name: "Test Group".to_string(),
            description: "A test group".to_string(),
            owner_agent_id: owner,
            members,
            quorum_policy: QuorumPolicy::Majority,
            budget: GroupBudget::new(10_000, None, None),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn make_group_with_policy(
        owner: AgentId,
        members: Vec<GroupMember>,
        policy: QuorumPolicy,
    ) -> Group {
        Group {
            id: GroupId::new(),
            name: "Policy Group".to_string(),
            description: "Group with custom policy".to_string(),
            owner_agent_id: owner,
            members,
            quorum_policy: policy,
            budget: GroupBudget::new(10_000, None, None),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    // -----------------------------------------------------------------------
    // Group CRUD tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn create_and_get_group() {
        let store = test_store();
        let owner = AgentId::new();
        let worker = AgentId::new();
        let group = make_group(
            owner,
            vec![
                GroupMember::new(owner, MemberRole::Leader),
                GroupMember::new(worker, MemberRole::Worker),
            ],
        );
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        assert_eq!(fetched.id, gid);
        assert_eq!(fetched.name, "Test Group");
        assert_eq!(fetched.description, "A test group");
        assert_eq!(fetched.owner_agent_id, owner);
        assert_eq!(fetched.members.len(), 2);
    }

    #[tokio::test]
    async fn create_group_duplicate_returns_already_exists() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group(owner, vec![GroupMember::new(owner, MemberRole::Leader)]);
        let gid = group.id;

        GroupStore::create_group(&store, group.clone())
            .await
            .expect("first create");

        let err = GroupStore::create_group(&store, group)
            .await
            .expect_err("should fail for duplicate");
        assert!(
            matches!(err, GroupError::AlreadyExists(id) if id == gid),
            "expected AlreadyExists, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn get_group_not_found() {
        let store = test_store();
        let gid = GroupId::new();
        let err = GroupStore::get_group(&store, &gid)
            .await
            .expect_err("should not find");
        assert!(matches!(err, GroupError::NotFound(_)));
    }

    #[tokio::test]
    async fn update_group_changes_fields() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group(owner, vec![GroupMember::new(owner, MemberRole::Leader)]);
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let mut fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        fetched.name = "Updated Name".to_string();
        fetched.description = "Updated description".to_string();

        GroupStore::update_group(&store, fetched)
            .await
            .expect("update");

        let updated = GroupStore::get_group(&store, &gid)
            .await
            .expect("get updated");
        assert_eq!(updated.name, "Updated Name");
        assert_eq!(updated.description, "Updated description");
    }

    #[tokio::test]
    async fn update_group_not_found() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group(owner, vec![]);

        let err = GroupStore::update_group(&store, group)
            .await
            .expect_err("should fail for nonexistent");
        assert!(matches!(err, GroupError::NotFound(_)));
    }

    #[tokio::test]
    async fn update_group_replaces_members() {
        let store = test_store();
        let owner = AgentId::new();
        let worker1 = AgentId::new();
        let worker2 = AgentId::new();

        let group = make_group(
            owner,
            vec![
                GroupMember::new(owner, MemberRole::Leader),
                GroupMember::new(worker1, MemberRole::Worker),
            ],
        );
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let mut fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        // Replace members: remove worker1, add worker2.
        fetched.members = vec![
            GroupMember::new(owner, MemberRole::Leader),
            GroupMember::new(worker2, MemberRole::Worker),
        ];

        GroupStore::update_group(&store, fetched)
            .await
            .expect("update");

        let updated = GroupStore::get_group(&store, &gid)
            .await
            .expect("get updated");
        assert_eq!(updated.members.len(), 2);
        assert!(updated.members.iter().any(|m| m.agent_id == worker2));
        assert!(!updated.members.iter().any(|m| m.agent_id == worker1));
    }

    #[tokio::test]
    async fn delete_group_succeeds() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group(owner, vec![GroupMember::new(owner, MemberRole::Leader)]);
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");
        GroupStore::delete_group(&store, &gid)
            .await
            .expect("delete");

        let err = GroupStore::get_group(&store, &gid)
            .await
            .expect_err("should not find deleted group");
        assert!(matches!(err, GroupError::NotFound(_)));
    }

    #[tokio::test]
    async fn delete_group_not_found() {
        let store = test_store();
        let gid = GroupId::new();
        let err = GroupStore::delete_group(&store, &gid)
            .await
            .expect_err("should fail for nonexistent");
        assert!(matches!(err, GroupError::NotFound(_)));
    }

    #[tokio::test]
    async fn delete_group_cascades_to_members() {
        let store = test_store();
        let owner = AgentId::new();
        let worker = AgentId::new();
        let group = make_group(
            owner,
            vec![
                GroupMember::new(owner, MemberRole::Leader),
                GroupMember::new(worker, MemberRole::Worker),
            ],
        );
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");
        GroupStore::delete_group(&store, &gid)
            .await
            .expect("delete");

        // Verify members were cascade-deleted.
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

    // -----------------------------------------------------------------------
    // list_groups
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_groups_empty() {
        let store = test_store();
        let result = GroupStore::list_groups(&store).await.expect("list empty");
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn list_groups_returns_all() {
        let store = test_store();
        let owner = AgentId::new();

        for i in 0..3 {
            let mut group = make_group(owner, vec![GroupMember::new(owner, MemberRole::Leader)]);
            group.name = format!("Group {i}");
            GroupStore::create_group(&store, group)
                .await
                .expect("create");
        }

        let groups = GroupStore::list_groups(&store).await.expect("list");
        assert_eq!(groups.len(), 3);
    }

    #[tokio::test]
    async fn list_groups_includes_members() {
        let store = test_store();
        let owner = AgentId::new();
        let worker = AgentId::new();

        let group = make_group(
            owner,
            vec![
                GroupMember::new(owner, MemberRole::Leader),
                GroupMember::new(worker, MemberRole::Worker),
            ],
        );
        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let groups = GroupStore::list_groups(&store).await.expect("list");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].members.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Member operations
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn add_member_to_group() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group(owner, vec![GroupMember::new(owner, MemberRole::Leader)]);
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let new_worker = AgentId::new();
        GroupStore::add_member(
            &store,
            &gid,
            GroupMember::new(new_worker, MemberRole::Worker),
        )
        .await
        .expect("add member");

        let members = GroupStore::list_members(&store, &gid).await.expect("list");
        assert_eq!(members.len(), 2);
        assert!(members.iter().any(|m| m.agent_id == new_worker));
    }

    #[tokio::test]
    async fn add_member_to_nonexistent_group() {
        let store = test_store();
        let gid = GroupId::new();
        let member = GroupMember::new(AgentId::new(), MemberRole::Worker);

        let err = GroupStore::add_member(&store, &gid, member)
            .await
            .expect_err("should fail");
        assert!(matches!(err, GroupError::NotFound(_)));
    }

    #[tokio::test]
    async fn add_duplicate_member_fails() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group(owner, vec![GroupMember::new(owner, MemberRole::Leader)]);
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let err = GroupStore::add_member(&store, &gid, GroupMember::new(owner, MemberRole::Worker))
            .await
            .expect_err("should fail for duplicate member");
        assert!(
            matches!(&err, GroupError::AlreadyMember(agent_id, group_id)
                if *agent_id == owner && *group_id == gid),
            "expected already-a-member error, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn remove_member_from_group() {
        let store = test_store();
        let owner = AgentId::new();
        let worker = AgentId::new();
        let group = make_group(
            owner,
            vec![
                GroupMember::new(owner, MemberRole::Leader),
                GroupMember::new(worker, MemberRole::Worker),
            ],
        );
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");
        GroupStore::remove_member(&store, &gid, &worker)
            .await
            .expect("remove");

        let members = GroupStore::list_members(&store, &gid).await.expect("list");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].agent_id, owner);
    }

    #[tokio::test]
    async fn remove_member_from_nonexistent_group() {
        let store = test_store();
        let gid = GroupId::new();
        let agent = AgentId::new();

        let err = GroupStore::remove_member(&store, &gid, &agent)
            .await
            .expect_err("should fail");
        assert!(matches!(err, GroupError::NotFound(_)));
    }

    #[tokio::test]
    async fn remove_nonexistent_member_returns_not_member() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group(owner, vec![GroupMember::new(owner, MemberRole::Leader)]);
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let stranger = AgentId::new();
        let err = GroupStore::remove_member(&store, &gid, &stranger)
            .await
            .expect_err("should fail");
        assert!(matches!(err, GroupError::NotMember(_, _)));
    }

    #[tokio::test]
    async fn list_members_returns_ordered_by_join_time() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group(owner, vec![GroupMember::new(owner, MemberRole::Leader)]);
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        // Add members with different timestamps.
        for _ in 0..3 {
            let new_agent = AgentId::new();
            GroupStore::add_member(
                &store,
                &gid,
                GroupMember::new(new_agent, MemberRole::Worker),
            )
            .await
            .expect("add member");
        }

        let members = GroupStore::list_members(&store, &gid).await.expect("list");
        assert_eq!(members.len(), 4);
        // Verify chronological ordering.
        for i in 0..members.len() - 1 {
            assert!(
                members[i].joined_at <= members[i + 1].joined_at,
                "members not ordered by joined_at at index {i}"
            );
        }
    }

    #[tokio::test]
    async fn list_members_on_nonexistent_group() {
        let store = test_store();
        let gid = GroupId::new();
        let err = GroupStore::list_members(&store, &gid)
            .await
            .expect_err("should fail");
        assert!(matches!(err, GroupError::NotFound(_)));
    }

    // -----------------------------------------------------------------------
    // Quorum policy round-trips
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn quorum_policy_unanimous_round_trip() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group_with_policy(
            owner,
            vec![GroupMember::new(owner, MemberRole::Leader)],
            QuorumPolicy::Unanimous,
        );
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        assert_eq!(fetched.quorum_policy, QuorumPolicy::Unanimous);
    }

    #[tokio::test]
    async fn quorum_policy_majority_round_trip() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group_with_policy(
            owner,
            vec![GroupMember::new(owner, MemberRole::Leader)],
            QuorumPolicy::Majority,
        );
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        assert_eq!(fetched.quorum_policy, QuorumPolicy::Majority);
    }

    #[tokio::test]
    async fn quorum_policy_threshold_round_trip() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group_with_policy(
            owner,
            vec![GroupMember::new(owner, MemberRole::Leader)],
            QuorumPolicy::Threshold { fraction: 0.75 },
        );
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        assert_eq!(
            fetched.quorum_policy,
            QuorumPolicy::Threshold { fraction: 0.75 }
        );
    }

    #[tokio::test]
    async fn quorum_policy_leader_only_round_trip() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group_with_policy(
            owner,
            vec![GroupMember::new(owner, MemberRole::Leader)],
            QuorumPolicy::LeaderOnly,
        );
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        assert_eq!(fetched.quorum_policy, QuorumPolicy::LeaderOnly);
    }

    // -----------------------------------------------------------------------
    // Budget JSON round-trips
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn budget_basic_round_trip() {
        let store = test_store();
        let owner = AgentId::new();
        let mut group = make_group(owner, vec![GroupMember::new(owner, MemberRole::Leader)]);
        group.budget = GroupBudget::new(50_000, Some(5_000), Some(1_000));
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        assert_eq!(fetched.budget.max_total, 50_000);
        assert_eq!(fetched.budget.max_per_member, Some(5_000));
        assert_eq!(fetched.budget.max_per_run, Some(1_000));
    }

    #[tokio::test]
    async fn budget_with_member_spent_round_trip() {
        let store = test_store();
        let owner = AgentId::new();
        let worker = AgentId::new();
        let mut group = make_group(
            owner,
            vec![
                GroupMember::new(owner, MemberRole::Leader),
                GroupMember::new(worker, MemberRole::Worker),
            ],
        );
        group.budget.member_spent.insert(worker.to_string(), 1_500);
        *group.budget.spent.lock() = 1_500;
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        assert_eq!(
            fetched.budget.member_spent.get(&worker.to_string()),
            Some(&1_500)
        );
        assert_eq!(fetched.budget.total_spent(), 1_500);
    }

    #[tokio::test]
    async fn budget_with_no_optional_limits() {
        let store = test_store();
        let owner = AgentId::new();
        let mut group = make_group(owner, vec![]);
        group.budget = GroupBudget::new(100, None, None);
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        assert_eq!(fetched.budget.max_total, 100);
        assert_eq!(fetched.budget.max_per_member, None);
        assert_eq!(fetched.budget.max_per_run, None);
        assert!(fetched.budget.member_spent.is_empty());
    }

    #[test]
    fn legacy_budget_without_total_reconstructs_checked_member_sum() {
        let budget = decode_budget(
            r#"{"max_total":1000,"max_per_member":null,"max_per_run":null,"member_spent":{"a":125,"b":75}}"#,
        )
        .expect("decode legacy budget");
        assert_eq!(budget.total_spent(), 200);
    }

    // -----------------------------------------------------------------------
    // Grant override round-trips
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn member_with_grant_override_round_trip() {
        let store = test_store();
        let owner = AgentId::new();
        let worker = AgentId::new();
        let grant = GrantSpec {
            capabilities: vec!["chain.transfer".to_string(), "model.inference".to_string()],
            max_budget: Some(500.0),
            allowed_pallets: vec!["Balances".to_string()],
        };
        let group = make_group(
            owner,
            vec![
                GroupMember::new(owner, MemberRole::Leader),
                GroupMember::with_grant(worker, MemberRole::Worker, grant),
            ],
        );
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        let w = fetched
            .members
            .iter()
            .find(|m| m.agent_id == worker)
            .expect("find worker");
        let g = w.grant_override.as_ref().expect("grant override present");
        assert_eq!(g.capabilities, vec!["chain.transfer", "model.inference"]);
        assert_eq!(g.max_budget, Some(500.0));
        assert_eq!(g.allowed_pallets, vec!["Balances"]);
    }

    #[tokio::test]
    async fn member_without_grant_override_round_trip() {
        let store = test_store();
        let owner = AgentId::new();
        let worker = AgentId::new();
        let group = make_group(
            owner,
            vec![
                GroupMember::new(owner, MemberRole::Leader),
                GroupMember::new(worker, MemberRole::Worker),
            ],
        );
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        let w = fetched
            .members
            .iter()
            .find(|m| m.agent_id == worker)
            .expect("find worker");
        assert!(w.grant_override.is_none());
    }

    #[tokio::test]
    async fn add_member_with_grant_override() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group(owner, vec![GroupMember::new(owner, MemberRole::Leader)]);
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let new_worker = AgentId::new();
        let grant = GrantSpec {
            capabilities: vec!["governance.vote".to_string()],
            max_budget: Some(1000.0),
            allowed_pallets: vec![],
        };
        GroupStore::add_member(
            &store,
            &gid,
            GroupMember::with_grant(new_worker, MemberRole::Worker, grant),
        )
        .await
        .expect("add");

        let members = GroupStore::list_members(&store, &gid).await.expect("list");
        let w = members
            .iter()
            .find(|m| m.agent_id == new_worker)
            .expect("find new worker");
        let g = w.grant_override.as_ref().expect("grant present");
        assert_eq!(g.capabilities, vec!["governance.vote"]);
        assert_eq!(g.max_budget, Some(1000.0));
    }

    // -----------------------------------------------------------------------
    // Member role round-trips
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn all_member_roles_round_trip() {
        let store = test_store();
        let owner = AgentId::new();
        let worker = AgentId::new();
        let observer = AgentId::new();
        let group = make_group(
            owner,
            vec![
                GroupMember::new(owner, MemberRole::Leader),
                GroupMember::new(worker, MemberRole::Worker),
                GroupMember::new(observer, MemberRole::Observer),
            ],
        );
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let members = GroupStore::list_members(&store, &gid).await.expect("list");
        assert_eq!(members.len(), 3);
        assert!(members.iter().any(|m| m.role == MemberRole::Leader));
        assert!(members.iter().any(|m| m.role == MemberRole::Worker));
        assert!(members.iter().any(|m| m.role == MemberRole::Observer));
    }

    // -----------------------------------------------------------------------
    // Empty group round-trip
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn group_with_no_members() {
        let store = test_store();
        let owner = AgentId::new();
        let group = make_group(owner, vec![]);
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        assert!(fetched.members.is_empty());
    }

    // -----------------------------------------------------------------------
    // Concurrent access
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn concurrent_group_creation() {
        let store = Arc::new(test_store());

        let mut handles = Vec::new();
        for i in 0..10 {
            let store_clone = store.clone();
            let handle = tokio::spawn(async move {
                let owner = AgentId::new();
                let mut group =
                    make_group(owner, vec![GroupMember::new(owner, MemberRole::Leader)]);
                group.name = format!("Concurrent Group {i}");
                GroupStore::create_group(store_clone.as_ref(), group)
                    .await
                    .expect("concurrent create");
            });
            handles.push(handle);
        }

        for h in handles {
            h.await.expect("task completed");
        }

        let groups = GroupStore::list_groups(store.as_ref()).await.expect("list");
        assert_eq!(groups.len(), 10);
    }

    #[tokio::test]
    async fn concurrent_member_addition() {
        let store = Arc::new(test_store());
        let owner = AgentId::new();
        let group = make_group(owner, vec![GroupMember::new(owner, MemberRole::Leader)]);
        let gid = group.id;

        GroupStore::create_group(store.as_ref(), group)
            .await
            .expect("create");

        let mut handles = Vec::new();
        for _ in 0..10 {
            let store_clone = store.clone();
            let handle = tokio::spawn(async move {
                let new_agent = AgentId::new();
                GroupStore::add_member(
                    store_clone.as_ref(),
                    &gid,
                    GroupMember::new(new_agent, MemberRole::Worker),
                )
                .await
                .expect("concurrent add member");
            });
            handles.push(handle);
        }

        for h in handles {
            h.await.expect("task completed");
        }

        let members = GroupStore::list_members(store.as_ref(), &gid)
            .await
            .expect("list");
        assert_eq!(members.len(), 11); // 1 leader + 10 workers
    }

    // -----------------------------------------------------------------------
    // Group with empty description
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn group_with_empty_description() {
        let store = test_store();
        let owner = AgentId::new();
        let mut group = make_group(owner, vec![]);
        group.description = String::new();
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        assert!(fetched.description.is_empty());
    }

    // -----------------------------------------------------------------------
    // Grant spec with empty fields
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn grant_spec_empty_capabilities_round_trip() {
        let store = test_store();
        let owner = AgentId::new();
        let worker = AgentId::new();
        let grant = GrantSpec {
            capabilities: vec![],
            max_budget: None,
            allowed_pallets: vec![],
        };
        let group = make_group(
            owner,
            vec![
                GroupMember::new(owner, MemberRole::Leader),
                GroupMember::with_grant(worker, MemberRole::Worker, grant),
            ],
        );
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        let w = fetched
            .members
            .iter()
            .find(|m| m.agent_id == worker)
            .expect("find worker");
        let g = w.grant_override.as_ref().expect("grant present");
        assert!(g.capabilities.is_empty());
        assert_eq!(g.max_budget, None);
        assert!(g.allowed_pallets.is_empty());
    }

    // -----------------------------------------------------------------------
    // Large group with many members
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn large_group_many_members() {
        let store = test_store();
        let owner = AgentId::new();
        let mut members = vec![GroupMember::new(owner, MemberRole::Leader)];
        for _ in 0..50 {
            members.push(GroupMember::new(AgentId::new(), MemberRole::Worker));
        }
        let group = make_group(owner, members);
        let gid = group.id;

        GroupStore::create_group(&store, group)
            .await
            .expect("create");

        let fetched = GroupStore::get_group(&store, &gid).await.expect("get");
        assert_eq!(fetched.members.len(), 51);
    }
}
