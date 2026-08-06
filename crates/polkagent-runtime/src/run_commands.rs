//! Conversation-scoped, redaction-safe read model for run slash commands.

use std::str::FromStr;

use polkagent_core::{AgentId, ArtifactId, ConversationId, RunId};
use polkagent_interaction::{
    InteractionError, InteractionErrorCode, RunDetailView, RunSummaryView,
    RUN_COMMAND_ARTIFACT_LIMIT, RUN_COMMAND_RESULT_LIMIT,
};
use polkagent_store_sqlite::SqlitePool;
use rusqlite::OptionalExtension as _;

/// The only durable read boundary used by interactive run commands.
///
/// Queries always include the selected conversation identity. The projection
/// excludes prompt parameters, provider payloads, artifact bodies and stored
/// reason suffixes before a surface can observe them.
#[derive(Debug, Clone)]
pub struct RunCommandReadModel {
    pool: SqlitePool,
}

impl RunCommandReadModel {
    /// Build a read model over the shared runtime database pool.
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// List newest runs belonging to exactly one conversation.
    pub async fn list_runs(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<RunSummaryView>, InteractionError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conversation_id = conversation_id.to_string();
            let connection = pool.writer();
            let mut statement = connection
                .prepare(
                    "SELECT id, agent_id, state
                     FROM runs
                     WHERE conversation_id = ?1
                     ORDER BY created_at DESC, id DESC
                     LIMIT ?2",
                )
                .map_err(read_failure)?;
            let rows = statement
                .query_map(
                    rusqlite::params![conversation_id, limit_i64(RUN_COMMAND_RESULT_LIMIT)],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .map_err(read_failure)?;
            rows.map(|row| row.map_err(read_failure).and_then(project_run))
                .collect()
        })
        .await
        .map_err(join_failure)?
    }

    /// Inspect a run only when it belongs to exactly one conversation.
    ///
    /// A run in another conversation is deliberately indistinguishable from
    /// a missing run.
    pub async fn inspect_run(
        &self,
        conversation_id: ConversationId,
        run_id: RunId,
    ) -> Result<RunDetailView, InteractionError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conversation_id = conversation_id.to_string();
            let run_id = run_id.to_string();
            let connection = pool.writer();
            let row = connection
                .query_row(
                    "SELECT id, agent_id, state
                     FROM runs
                     WHERE id = ?1 AND conversation_id = ?2",
                    rusqlite::params![run_id, conversation_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(read_failure)?
                .ok_or_else(run_not_found)?;
            let run = project_run(row)?;

            let mut statement = connection
                .prepare(
                    "SELECT id
                     FROM artifacts
                     WHERE run_id = ?1
                     ORDER BY created_at DESC, id DESC
                     LIMIT ?2",
                )
                .map_err(read_failure)?;
            let rows = statement
                .query_map(
                    rusqlite::params![run_id, limit_i64(RUN_COMMAND_ARTIFACT_LIMIT)],
                    |row| row.get::<_, String>(0),
                )
                .map_err(read_failure)?;
            let artifacts = rows
                .map(|row| {
                    let raw = row.map_err(read_failure)?;
                    ArtifactId::from_str(&raw)
                        .map(|id| id.to_string())
                        .map_err(|_| invalid_projection())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let error = terminal_error(&run.state);

            Ok(RunDetailView {
                run,
                artifacts,
                error,
            })
        })
        .await
        .map_err(join_failure)?
    }
}

fn project_run(row: (String, String, String)) -> Result<RunSummaryView, InteractionError> {
    let (run_id, agent_id, state) = row;
    Ok(RunSummaryView {
        run_id: RunId::from_str(&run_id).map_err(|_| invalid_projection())?,
        agent_id: AgentId::from_str(&agent_id).map_err(|_| invalid_projection())?,
        state: normalize_state(&state)
            .ok_or_else(invalid_projection)?
            .to_owned(),
        summary: None,
    })
}

fn normalize_state(state: &str) -> Option<&'static str> {
    match state {
        "created" => Some("created"),
        "queued" => Some("queued"),
        "running" | "started" | "working" | "executing" => Some("running"),
        "completing" => Some("completing"),
        "completed" => Some("completed"),
        "failed" => Some("failed"),
        "cancelled" => Some("cancelled"),
        "timed_out" => Some("timed_out"),
        value if value.starts_with("awaiting_approval:") => Some("awaiting_approval"),
        value if value.starts_with("waiting_effect:") => Some("waiting_effect"),
        value if value.starts_with("failed:") => Some("failed"),
        value if value.starts_with("cancelled:") => Some("cancelled"),
        _ => None,
    }
}

fn terminal_error(state: &str) -> Option<String> {
    match state {
        "failed" => Some("run failed".to_owned()),
        "cancelled" => Some("run cancelled".to_owned()),
        "timed_out" => Some("run timed out".to_owned()),
        _ => None,
    }
}

fn limit_i64(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

fn run_not_found() -> InteractionError {
    InteractionError::new(
        InteractionErrorCode::NotFound,
        "run was not found in the selected conversation",
    )
}

fn invalid_projection() -> InteractionError {
    InteractionError::new(
        InteractionErrorCode::Internal,
        "stored run projection is invalid",
    )
}

fn read_failure(_error: rusqlite::Error) -> InteractionError {
    InteractionError::new(
        InteractionErrorCode::Unavailable,
        "durable run projection is unavailable",
    )
    .retryable()
}

fn join_failure(_error: tokio::task::JoinError) -> InteractionError {
    InteractionError::new(
        InteractionErrorCode::Internal,
        "durable run projection task failed",
    )
}

#[cfg(test)]
mod tests {
    use polkagent_store_sqlite::migrations;

    use super::*;

    fn fixture() -> (SqlitePool, AgentId, ConversationId, ConversationId) {
        let pool = SqlitePool::open_in_memory().expect("open SQLite");
        migrations::migrate(&pool.writer()).expect("migrate SQLite");
        let agent_id = AgentId::new();
        let selected = ConversationId::new();
        let other = ConversationId::new();
        pool.writer()
            .execute(
                "INSERT INTO agents
                 (id, name, description, state, spec_json, created_at, updated_at)
                 VALUES (?1, 'agent', NULL, 'active', '{}', ?2, ?2)",
                rusqlite::params![agent_id.to_string(), "2026-01-01T00:00:00Z"],
            )
            .expect("seed agent");
        (pool, agent_id, selected, other)
    }

    fn seed_run(
        pool: &SqlitePool,
        run_id: RunId,
        agent_id: AgentId,
        conversation_id: ConversationId,
        state: &str,
        created_at: &str,
    ) {
        pool.writer()
            .execute(
                "INSERT INTO runs
                 (id, agent_id, conversation_id, state, params_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, '{\"prompt\":\"secret\"}', ?5, ?5)",
                rusqlite::params![
                    run_id.to_string(),
                    agent_id.to_string(),
                    conversation_id.to_string(),
                    state,
                    created_at
                ],
            )
            .expect("seed run");
    }

    #[tokio::test]
    async fn list_is_conversation_scoped_newest_first_and_bounded() {
        let (pool, agent_id, selected, other) = fixture();
        let read_model = RunCommandReadModel::new(pool.clone());
        let foreign_run = RunId::new();
        seed_run(
            &pool,
            foreign_run,
            agent_id,
            other,
            "running",
            "2026-12-31T00:00:00Z",
        );
        let mut selected_runs = Vec::new();
        for index in 0..(RUN_COMMAND_RESULT_LIMIT + 3) {
            let run_id = RunId::new();
            seed_run(
                &pool,
                run_id,
                agent_id,
                selected,
                "created",
                &format!("2026-01-{day:02}T00:00:00Z", day = index + 1),
            );
            selected_runs.push(run_id);
        }

        let runs = read_model.list_runs(selected).await.expect("list runs");

        assert_eq!(runs.len(), RUN_COMMAND_RESULT_LIMIT);
        assert_eq!(runs[0].run_id, *selected_runs.last().expect("newest run"));
        assert!(!runs.iter().any(|run| run.run_id == foreign_run));
        assert!(runs.iter().all(|run| run.summary.is_none()));
    }

    #[tokio::test]
    async fn inspect_rejects_missing_and_foreign_runs_identically() {
        let (pool, agent_id, selected, other) = fixture();
        let foreign_run = RunId::new();
        seed_run(
            &pool,
            foreign_run,
            agent_id,
            other,
            "running",
            "2026-01-01T00:00:00Z",
        );
        let read_model = RunCommandReadModel::new(pool);

        let foreign = read_model
            .inspect_run(selected, foreign_run)
            .await
            .expect_err("foreign run must not be visible");
        let missing = read_model
            .inspect_run(selected, RunId::new())
            .await
            .expect_err("missing run must not be visible");

        assert_eq!(foreign.code, InteractionErrorCode::NotFound);
        assert_eq!(foreign, missing);
    }

    #[tokio::test]
    async fn inspect_redacts_reason_and_bounds_canonical_artifact_ids() {
        let (pool, agent_id, selected, _) = fixture();
        let run_id = RunId::new();
        seed_run(
            &pool,
            run_id,
            agent_id,
            selected,
            "failed:provider secret token",
            "2026-01-01T00:00:00Z",
        );
        for index in 0..(RUN_COMMAND_ARTIFACT_LIMIT + 3) {
            let artifact_id = ArtifactId::new();
            pool.writer()
                .execute(
                    "INSERT INTO artifacts
                     (id, run_id, kind, digest_hex, size_bytes, metadata_json, created_at)
                     VALUES (?1, ?2, 'text', ?3, 0, '{\"secret\":\"value\"}', ?4)",
                    rusqlite::params![
                        artifact_id.to_string(),
                        run_id.to_string(),
                        format!("digest-{index}"),
                        format!("2026-01-{day:02}T00:00:00Z", day = index + 1)
                    ],
                )
                .expect("seed artifact");
        }
        let read_model = RunCommandReadModel::new(pool);

        let detail = read_model
            .inspect_run(selected, run_id)
            .await
            .expect("inspect selected run");

        assert_eq!(detail.run.state, "failed");
        assert_eq!(detail.run.summary, None);
        assert_eq!(detail.error.as_deref(), Some("run failed"));
        assert_eq!(detail.artifacts.len(), RUN_COMMAND_ARTIFACT_LIMIT);
        assert!(!format!("{detail:?}").contains("secret"));
    }
}
