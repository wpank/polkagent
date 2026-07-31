//! Cancellation and evidence propagation across group members.
//!
//! When a coordinating run is cancelled, all child runs spawned by the group
//! must also be cancelled. This module provides [`propagate_cancellation`]
//! to compute which runs need to be cancelled.
//!
//! After member runs complete, [`aggregate_evidence`] collects their results
//! into a unified [`GroupEvidence`] record.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use polkagent_core::ids::{ArtifactId, RunId};

use crate::types::{Group, GroupId, RunResult};

// ---------------------------------------------------------------------------
// GroupEvidence
// ---------------------------------------------------------------------------

/// Aggregated evidence from all member runs that contributed to a group
/// decision or outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupEvidence {
    /// The group that produced this evidence.
    pub group_id: GroupId,

    /// The run IDs from member agents that contributed to this evidence.
    pub contributing_runs: Vec<RunId>,

    /// All artifact IDs produced across all contributing runs.
    pub aggregated_artifacts: Vec<ArtifactId>,

    /// Whether a quorum decision was reached for this outcome.
    pub quorum_met: bool,

    /// The number of successful member runs.
    pub successful_runs: usize,

    /// The number of failed member runs.
    pub failed_runs: usize,

    /// A human-readable summary of the aggregated outcome.
    pub summary: String,

    /// When this evidence was aggregated.
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// propagate_cancellation
// ---------------------------------------------------------------------------

/// Compute the list of child run IDs that should be cancelled when
/// `source_run_id` is cancelled within `group`.
///
/// In the current model, all member runs that are *not* the source run are
/// returned. Callers are responsible for actually sending cancellation signals
/// to those runs via the run management layer.
///
/// # Arguments
///
/// * `group` — the group whose member runs should be inspected.
/// * `source_run_id` — the run that triggered the cancellation; this run
///   is excluded from the returned list.
/// * `active_run_ids` — the set of currently active run IDs associated with
///   group members. Only IDs in this set are returned.
///
/// # Returns
///
/// A `Vec<RunId>` of runs to cancel (may be empty if the source is the only
/// active run or no member runs are active).
pub fn propagate_cancellation(
    _group: &Group,
    source_run_id: &RunId,
    active_run_ids: &[RunId],
) -> Vec<RunId> {
    active_run_ids
        .iter()
        .filter(|id| *id != source_run_id)
        .copied()
        .collect()
}

// ---------------------------------------------------------------------------
// aggregate_evidence
// ---------------------------------------------------------------------------

/// Aggregate the results of multiple member runs into a single
/// [`GroupEvidence`] record.
///
/// # Arguments
///
/// * `group` — the group that coordinated the runs.
/// * `run_results` — the individual outcomes from each member run.
///
/// # Returns
///
/// A [`GroupEvidence`] summarising all contributions.
pub fn aggregate_evidence(group: &Group, run_results: &[RunResult]) -> GroupEvidence {
    let contributing_runs: Vec<RunId> = run_results.iter().map(|r| r.run_id).collect();

    let aggregated_artifacts: Vec<ArtifactId> = run_results
        .iter()
        .flat_map(|r| r.artifact_ids.iter().copied())
        .collect();

    let successful_runs = run_results.iter().filter(|r| r.success).count();
    let failed_runs = run_results.iter().filter(|r| !r.success).count();

    // Quorum is considered met if more than half of the contributing runs
    // succeeded. This is a simple heuristic; production code would use the
    // group's QuorumPolicy.
    let quorum_met = if run_results.is_empty() {
        false
    } else {
        successful_runs > run_results.len() / 2
    };

    let summary = format!(
        "Group '{}' aggregated {} run(s): {} succeeded, {} failed",
        group.name,
        run_results.len(),
        successful_runs,
        failed_runs
    );

    GroupEvidence {
        group_id: group.id,
        contributing_runs,
        aggregated_artifacts,
        quorum_met,
        successful_runs,
        failed_runs,
        summary,
        timestamp: Utc::now(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_core::ids::{AgentId, ArtifactId, RunId};

    use crate::types::{GroupBudget, GroupMember, MemberRole, QuorumPolicy};

    fn make_group(name: &str) -> Group {
        let owner = AgentId::new();
        Group {
            id: GroupId::new(),
            name: name.to_string(),
            description: String::new(),
            owner_agent_id: owner,
            members: vec![GroupMember::new(owner, MemberRole::Leader)],
            quorum_policy: QuorumPolicy::Majority,
            budget: GroupBudget::new(1000, None, None),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    // ---- propagate_cancellation -------------------------------------------

    #[test]
    fn cancellation_excludes_source_run() {
        let group = make_group("g");
        let source = RunId::new();
        let child1 = RunId::new();
        let child2 = RunId::new();
        let active = vec![source, child1, child2];

        let to_cancel = propagate_cancellation(&group, &source, &active);
        assert_eq!(to_cancel.len(), 2);
        assert!(!to_cancel.contains(&source));
        assert!(to_cancel.contains(&child1));
        assert!(to_cancel.contains(&child2));
    }

    #[test]
    fn cancellation_empty_active_runs_returns_empty() {
        let group = make_group("g");
        let source = RunId::new();
        let to_cancel = propagate_cancellation(&group, &source, &[]);
        assert!(to_cancel.is_empty());
    }

    #[test]
    fn cancellation_single_active_run_is_source_returns_empty() {
        let group = make_group("g");
        let source = RunId::new();
        let to_cancel = propagate_cancellation(&group, &source, &[source]);
        assert!(to_cancel.is_empty());
    }

    #[test]
    fn cancellation_returns_all_non_source_runs() {
        let group = make_group("g");
        let source = RunId::new();
        let others: Vec<RunId> = (0..5).map(|_| RunId::new()).collect();
        let mut active = vec![source];
        active.extend_from_slice(&others);

        let to_cancel = propagate_cancellation(&group, &source, &active);
        assert_eq!(to_cancel.len(), 5);
        for r in &others {
            assert!(to_cancel.contains(r));
        }
    }

    // ---- aggregate_evidence -----------------------------------------------

    #[test]
    fn aggregate_evidence_combines_artifacts() {
        let group = make_group("g");
        let art1 = ArtifactId::new();
        let art2 = ArtifactId::new();
        let results = vec![
            RunResult {
                run_id: RunId::new(),
                agent_id: AgentId::new(),
                success: true,
                artifact_ids: vec![art1],
                summary: "run 1".to_string(),
            },
            RunResult {
                run_id: RunId::new(),
                agent_id: AgentId::new(),
                success: true,
                artifact_ids: vec![art2],
                summary: "run 2".to_string(),
            },
        ];

        let evidence = aggregate_evidence(&group, &results);
        assert_eq!(evidence.group_id, group.id);
        assert_eq!(evidence.contributing_runs.len(), 2);
        assert_eq!(evidence.aggregated_artifacts.len(), 2);
        assert!(evidence.aggregated_artifacts.contains(&art1));
        assert!(evidence.aggregated_artifacts.contains(&art2));
        assert!(evidence.quorum_met);
        assert_eq!(evidence.successful_runs, 2);
        assert_eq!(evidence.failed_runs, 0);
    }

    #[test]
    fn aggregate_evidence_quorum_not_met_if_majority_failed() {
        let group = make_group("g");
        let results = vec![
            RunResult {
                run_id: RunId::new(),
                agent_id: AgentId::new(),
                success: false,
                artifact_ids: vec![],
                summary: "fail".to_string(),
            },
            RunResult {
                run_id: RunId::new(),
                agent_id: AgentId::new(),
                success: false,
                artifact_ids: vec![],
                summary: "fail".to_string(),
            },
            RunResult {
                run_id: RunId::new(),
                agent_id: AgentId::new(),
                success: true,
                artifact_ids: vec![],
                summary: "ok".to_string(),
            },
        ];

        let evidence = aggregate_evidence(&group, &results);
        assert!(!evidence.quorum_met);
        assert_eq!(evidence.successful_runs, 1);
        assert_eq!(evidence.failed_runs, 2);
    }

    #[test]
    fn aggregate_evidence_empty_results() {
        let group = make_group("g");
        let evidence = aggregate_evidence(&group, &[]);
        assert!(evidence.contributing_runs.is_empty());
        assert!(evidence.aggregated_artifacts.is_empty());
        assert!(!evidence.quorum_met);
        assert_eq!(evidence.successful_runs, 0);
        assert_eq!(evidence.failed_runs, 0);
    }
}
