//! [`GroupStore`] trait implementation for [`SqlitePool`].
//!
//! Wraps synchronous `rusqlite` calls in [`tokio::task::spawn_blocking`] to
//! satisfy the async trait interface. The writer connection is protected by a
//! `parking_lot::Mutex` inside [`SqlitePool`], so each method acquires it
//! briefly within the blocking closure.
//!
//! # Serialization choices
//!
//! - [`QuorumPolicy`] is stored as JSON (tagged enum with `kind` discriminant).
//! - [`GroupBudget`] is stored as a JSON object containing the persisted
//!   fields (`max_total`, `max_per_member`, `max_per_run`, `member_spent`).
//!   The runtime `spent` mutex is reconstructed from the stored `member_spent`
//!   aggregation — on load the in-memory `spent` counter starts at zero
//!   (consistent with the policy that the live spend counter is ephemeral and
//!   managed by the coordinator, not the store).
//! - [`GrantSpec`] overrides on members are stored as JSON (`NULL` when
//!   absent).
//! - [`MemberRole`] is stored as its lowercase string (`leader`, `worker`,
//!   `observer`).

use async_trait::async_trait;
use chrono::DateTime;

use polkagent_core::ids::AgentId;
use polkagent_group::{
    GroupError, GroupResult, GroupStore,
    types::{GrantSpec, Group, GroupBudget, GroupId, GroupMember, MemberRole, QuorumPolicy},
};

use crate::pool::SqlitePool;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Map a `rusqlite::Error` to `GroupError`.
fn map_err(e: rusqlite::Error) -> GroupError {
    GroupError::Internal(format!("sqlite error: {e}"))
}

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
        other => Err(GroupError::Internal(format!("unknown member role: '{other}'"))),
    }
}

/// Serialize a [`QuorumPolicy`] to JSON for storage.
fn encode_quorum_policy(policy: &QuorumPolicy) -> GroupResult<String> {
    serde_json::to_string(policy)
        .map_err(|e| GroupError::Internal(format!("failed to serialize quorum policy: {e}")))
}

/// Deserialize a [`QuorumPolicy`] from its stored JSON.
fn decode_quorum_policy(s: &str) -> GroupResult<QuorumPolicy> {
    serde_json::from_str(s)
        .map_err(|e| GroupError::Internal(format!("failed to deserialize quorum policy '{s}': {e}")))
}

/// Serialize a [`GroupBudget`] to JSON for storage.
///
/// Uses [`serde_json::Value`] directly rather than a derived struct to avoid
/// serde-derive trait-solver recursion depth issues when `GroupBudget`
/// (which contains `Arc<parking_lot::Mutex<u64>>`) is in scope.
fn encode_budget(budget: &GroupBudget) -> GroupResult<String> {
    let mut obj = serde_json::Map::new();
    obj.insert("max_total".to_string(), serde_json::Value::from(budget.max_total));
    obj.insert(
        "max_per_member".to_string(),
        budget.max_per_member.map_or(serde_json::Value::Null, serde_json::Value::from),
    );
    obj.insert(
        "max_per_run".to_string(),
        budget.max_per_run.map_or(serde_json::Value::Null, serde_json::Value::from),
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
            .and_then(|v| v.as_u64())
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

    if let Some(serde_json::Value::Object(spent_map)) = val.get("member_spent") {
        for (agent_id, amount) in spent_map {
            if let Some(n) = amount.as_u64() {
                budget.member_spent.insert(agent_id.clone(), n);
            }
        }
    }

    Ok(budget)
}

/// Serialize an optional [`GrantSpec`] to JSON (`NULL` for `None`).
fn encode_grant_override(grant: &Option<GrantSpec>) -> GroupResult<Option<String>> {
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
        Some(json) => serde_json::from_str(&json)
            .map(Some)
            .map_err(|e| {
                GroupError::Internal(format!("failed to deserialize grant spec '{json}': {e}"))
            }),
    }
}

// ---------------------------------------------------------------------------
// Row -> GroupMember
// ---------------------------------------------------------------------------

/// Raw column tuple from `group_members`.
///
/// Columns: agent_id, role, grant_override_json, joined_at
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
// GroupStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl GroupStore for SqlitePool {
    // -----------------------------------------------------------------------
    // create_group
    // -----------------------------------------------------------------------

    async fn create_group(&self, group: Group) -> GroupResult<()> {
        let pool = self.clone();

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
        let members: Vec<(String, String, Option<String>, String)> = group
            .members
            .iter()
            .map(|m| {
                let grant_json = encode_grant_override(&m.grant_override)?;
                Ok((
                    m.agent_id.to_string(),
                    encode_role(m.role).to_string(),
                    grant_json,
                    m.joined_at.to_rfc3339(),
                ))
            })
            .collect::<GroupResult<_>>()?;

        let group_id_str_clone = id_str.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // Insert the group row.
            writer
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
                .map_err(|e| match &e {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error {
                            code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                            ..
                        },
                        _,
                    ) => GroupError::AlreadyExists(group_id_str_clone.parse().unwrap_or_default()),
                    _ => map_err(e),
                })?;

            // Insert members.
            for (agent_id_s, role_s, grant_json, joined_at_s) in members {
                writer
                    .execute(
                        "INSERT INTO group_members
                             (group_id, agent_id, role, grant_override_json, joined_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![
                            id_str,
                            agent_id_s,
                            role_s,
                            grant_json,
                            joined_at_s,
                        ],
                    )
                    .map_err(map_err)?;
            }

            Ok(())
        })
        .await
        .map_err(|e| GroupError::Internal(format!("blocking task panicked: {e}")))?
    }

    // -----------------------------------------------------------------------
    // get_group
    // -----------------------------------------------------------------------

    async fn get_group(&self, group_id: &GroupId) -> GroupResult<Group> {
        let pool = self.clone();
        let id_str = group_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // Fetch the group row.
            let (
                id_s,
                name,
                description,
                owner_s,
                quorum_s,
                budget_s,
                created_s,
                updated_s,
            ) = writer
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
        let pool = self.clone();

        let id_str = group.id.to_string();
        let owner_str = group.owner_agent_id.to_string();
        let quorum_json = encode_quorum_policy(&group.quorum_policy)?;
        let budget_json = encode_budget(&group.budget)?;
        let updated_str = chrono::Utc::now().to_rfc3339();
        let name = group.name.clone();
        let description = group.description.clone();

        // Prepare new member rows.
        let members: Vec<(String, String, Option<String>, String)> = group
            .members
            .iter()
            .map(|m| {
                let grant_json = encode_grant_override(&m.grant_override)?;
                Ok((
                    m.agent_id.to_string(),
                    encode_role(m.role).to_string(),
                    grant_json,
                    m.joined_at.to_rfc3339(),
                ))
            })
            .collect::<GroupResult<_>>()?;

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            let n = writer
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
                return Err(GroupError::NotFound(
                    id_str.parse().unwrap_or_default(),
                ));
            }

            // Replace members: delete all then re-insert.  This is simpler
            // and correct given that update_group owns the full new state.
            writer
                .execute(
                    "DELETE FROM group_members WHERE group_id = ?1",
                    [&id_str],
                )
                .map_err(map_err)?;

            for (agent_id_s, role_s, grant_json, joined_at_s) in members {
                writer
                    .execute(
                        "INSERT INTO group_members
                             (group_id, agent_id, role, grant_override_json, joined_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![
                            id_str,
                            agent_id_s,
                            role_s,
                            grant_json,
                            joined_at_s,
                        ],
                    )
                    .map_err(map_err)?;
            }

            Ok(())
        })
        .await
        .map_err(|e| GroupError::Internal(format!("blocking task panicked: {e}")))?
    }

    // -----------------------------------------------------------------------
    // delete_group
    // -----------------------------------------------------------------------

    async fn delete_group(&self, group_id: &GroupId) -> GroupResult<()> {
        let pool = self.clone();
        let id_str = group_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            // Members are removed via ON DELETE CASCADE on the FK.
            let n = writer
                .execute("DELETE FROM groups WHERE id = ?1", [&id_str])
                .map_err(map_err)?;

            if n == 0 {
                return Err(GroupError::NotFound(
                    id_str.parse().unwrap_or_default(),
                ));
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
        let pool = self.clone();

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

            let raw_groups: Vec<(String, String, String, String, String, String, String, String)> =
                stmt.query_map([], |row| {
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
        let pool = self.clone();
        let id_str = group_id.to_string();
        let agent_id_str = member.agent_id.to_string();
        let role_str = encode_role(member.role).to_string();
        let grant_json = encode_grant_override(&member.grant_override)?;
        let joined_at_str = member.joined_at.to_rfc3339();

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
                return Err(GroupError::NotFound(
                    id_str.parse().unwrap_or_default(),
                ));
            }

            writer
                .execute(
                    "INSERT INTO group_members
                         (group_id, agent_id, role, grant_override_json, joined_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        id_str,
                        agent_id_str,
                        role_str,
                        grant_json,
                        joined_at_str,
                    ],
                )
                .map_err(|e| match &e {
                    rusqlite::Error::SqliteFailure(
                        rusqlite::ffi::Error {
                            code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                            ..
                        },
                        _,
                    ) => GroupError::Internal(format!(
                        "agent '{}' is already a member of group '{}'",
                        agent_id_str, id_str
                    )),
                    _ => map_err(e),
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
        let pool = self.clone();
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
        let pool = self.clone();
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

// Tests for this module live in tests/group_store.rs as an integration test
// to avoid serde derive recursion limit issues caused by importing GroupBudget
// (which contains Arc<Mutex<u64>> with a parking_lot Mutex) in the same
// compilation unit as many other serde-derived types.

