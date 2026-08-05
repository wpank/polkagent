//! Shared CLI projection over the retained runtime's durable agent registry.

use polkagent_core::AgentId;
use polkagent_interaction::{AgentTargetView, InteractionError, InteractionErrorCode};
use polkagent_store_sqlite::{SqlitePool, SqliteRunStore};

/// List active targets from the runtime's already-open durable registry.
pub(crate) fn list_active_agent_targets(
    pool: &SqlitePool,
) -> Result<Vec<AgentTargetView>, InteractionError> {
    SqliteRunStore::new(pool.clone())
        .list_agents(Some("active"), false)
        .map_err(|_| registry_error("list active runtime agents"))?
        .into_iter()
        .map(|row| {
            let agent_id = row
                .id
                .parse::<AgentId>()
                .map_err(|_| registry_error("decode runtime agent identity"))?;
            Ok(AgentTargetView {
                agent_id,
                name: row.name,
                state: row.state,
                // Active lifecycle is durable; per-agent provider/model
                // readiness has no exact CLI read model yet. Surfaces render
                // lifecycle only and leave readiness unclaimed.
                ready: false,
            })
        })
        .collect()
}

/// Resolve an exact name or UUID, rejecting unknown and ambiguous selectors.
pub(crate) fn resolve_active_agent_target(
    pool: &SqlitePool,
    selector: &str,
) -> Result<AgentTargetView, InteractionError> {
    let matches = list_active_agent_targets(pool)?
        .into_iter()
        .filter(|agent| agent.name == selector || agent.agent_id.to_string() == selector)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [agent] => Ok(agent.clone()),
        [] => Err(InteractionError::new(
            InteractionErrorCode::NotFound,
            "no active agent matches the supplied name or ID",
        )),
        _ => Err(InteractionError::new(
            InteractionErrorCode::Conflict,
            "agent selector matches more than one active agent; use an unambiguous UUID",
        )),
    }
}

/// Resolve one exact active UUID for rendering a persisted target.
pub(crate) fn active_agent_by_id(
    pool: &SqlitePool,
    agent_id: AgentId,
) -> Result<AgentTargetView, InteractionError> {
    list_active_agent_targets(pool)?
        .into_iter()
        .find(|agent| agent.agent_id == agent_id)
        .ok_or_else(|| {
            InteractionError::new(
                InteractionErrorCode::NotFound,
                "durable interaction target is not an active runtime agent",
            )
        })
}

/// Resolve a durable UUID even if lifecycle changes immediately after an
/// accepted target mutation, so surfaces never report failure after commit.
pub(crate) fn registered_agent_by_id(
    pool: &SqlitePool,
    agent_id: AgentId,
) -> Result<AgentTargetView, InteractionError> {
    let row = SqliteRunStore::new(pool.clone())
        .list_agents(None, true)
        .map_err(|_| registry_error("list runtime agents"))?
        .into_iter()
        .find(|row| row.id == agent_id.to_string())
        .ok_or_else(|| {
            InteractionError::new(
                InteractionErrorCode::NotFound,
                "durable interaction target is not a registered runtime agent",
            )
        })?;
    Ok(AgentTargetView {
        agent_id,
        name: row.name,
        state: row.state,
        ready: false,
    })
}

fn registry_error(action: &'static str) -> InteractionError {
    InteractionError::new(
        InteractionErrorCode::Internal,
        format!("failed to {action}"),
    )
}
