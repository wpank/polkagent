//! Shared, bounded rendering for read-only run slash commands.

use std::fmt::Write as _;

use crate::{CommandOutput, RunDetailView, RunSummaryView};

/// Maximum run rows returned and rendered by `/runs`.
pub const RUN_COMMAND_RESULT_LIMIT: usize = 20;

/// Maximum artifact identities returned and rendered by `/inspect`.
pub const RUN_COMMAND_ARTIFACT_LIMIT: usize = 20;

/// Render run-command output identically across terminal and editor surfaces.
///
/// This formatter is intentionally narrow: summaries are not rendered, raw
/// error strings are allowlisted, and artifact strings are normalized before
/// display. Adapters should use it instead of formatting projections locally.
pub fn format_run_command_output(output: &CommandOutput) -> Option<String> {
    match output {
        CommandOutput::Runs { runs } => Some(format_run_list(runs)),
        CommandOutput::RunInspected { run } => Some(format_run_inspection(run)),
        _ => None,
    }
}

/// Render a bounded list produced by `/runs`.
pub fn format_run_list(runs: &[RunSummaryView]) -> String {
    if runs.is_empty() {
        return "No runs are linked to the selected conversation.".to_owned();
    }

    let mut output =
        format!("Recent runs for the selected conversation (up to {RUN_COMMAND_RESULT_LIMIT}):");
    for run in runs.iter().take(RUN_COMMAND_RESULT_LIMIT) {
        let _ = write!(
            output,
            "\n  {}  {}  agent={}",
            run.run_id,
            safe_state(&run.state),
            run.agent_id
        );
    }
    output
}

/// Render one bounded, redaction-safe `/inspect` projection.
pub fn format_run_inspection(detail: &RunDetailView) -> String {
    let mut output = format!(
        "Run: {}\nState: {}\nAgent: {}",
        detail.run.run_id,
        safe_state(&detail.run.state),
        detail.run.agent_id
    );
    if detail.artifacts.is_empty() {
        output.push_str("\nArtifacts: none");
    } else {
        let _ = write!(output, "\nArtifacts (up to {RUN_COMMAND_ARTIFACT_LIMIT}):");
        for artifact_id in detail.artifacts.iter().take(RUN_COMMAND_ARTIFACT_LIMIT) {
            let _ = write!(output, "\n  {}", safe_identifier(artifact_id));
        }
    }
    if let Some(error) = detail.error.as_deref().and_then(safe_error) {
        let _ = write!(output, "\nError: {error}");
    }
    output
}

fn safe_state(state: &str) -> &'static str {
    match state {
        "created" => "created",
        "queued" => "queued",
        "running" | "started" | "working" | "executing" => "running",
        "awaiting_approval" => "awaiting_approval",
        "waiting_effect" => "waiting_effect",
        "completing" => "completing",
        "completed" => "completed",
        "failed" => "failed",
        "cancelled" => "cancelled",
        "timed_out" => "timed_out",
        _ => "unknown",
    }
}

fn safe_identifier(value: &str) -> String {
    const MAX_IDENTIFIER_CHARS: usize = 80;
    let normalized: String = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(MAX_IDENTIFIER_CHARS)
        .collect();
    if normalized.is_empty() {
        "<redacted>".to_owned()
    } else {
        normalized
    }
}

fn safe_error(error: &str) -> Option<&'static str> {
    match error {
        "run failed" => Some("run failed"),
        "run cancelled" => Some("run cancelled"),
        "run timed out" => Some("run timed out"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use polkagent_core::{AgentId, RunId};

    use super::*;

    fn run(state: &str) -> RunSummaryView {
        RunSummaryView {
            run_id: RunId::new(),
            agent_id: AgentId::new(),
            state: state.to_owned(),
            summary: Some("sensitive prompt contents".to_owned()),
        }
    }

    #[test]
    fn run_list_is_bounded_and_does_not_render_summaries() {
        let runs = (0..(RUN_COMMAND_RESULT_LIMIT + 5))
            .map(|_| run("running"))
            .collect();
        let rendered = format_run_command_output(&CommandOutput::Runs { runs })
            .expect("run output is supported");

        assert_eq!(rendered.matches("agent=").count(), RUN_COMMAND_RESULT_LIMIT);
        assert!(!rendered.contains("sensitive prompt contents"));
    }

    #[test]
    fn detail_redacts_state_suffixes_errors_and_control_characters() {
        let mut summary = run("failed:token=secret");
        summary.summary = Some("never render this".to_owned());
        let rendered = format_run_command_output(&CommandOutput::RunInspected {
            run: RunDetailView {
                run: summary,
                artifacts: vec!["safe-id\nsecret".to_owned()],
                error: Some("provider returned api-key".to_owned()),
            },
        })
        .expect("inspection output is supported");

        assert!(rendered.contains("State: unknown"));
        assert!(rendered.contains("safe-idsecret"));
        assert!(!rendered.contains("token=secret"));
        assert!(!rendered.contains("api-key"));
        assert!(!rendered.contains("never render this"));
    }
}
