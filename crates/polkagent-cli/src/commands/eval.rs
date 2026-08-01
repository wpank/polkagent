//! `polkagent eval` — run, list, report, and compare evaluation suites.
//!
//! Subcommands:
//!   eval run <suite-path>         — run a suite against the fake executor
//!   eval list [dir]               — list available suites under fixtures/evals/
//!   eval report <report-path>     — display a saved report
//!   eval compare <baseline> <cur> — compare two reports for regressions

use std::path::Path;

use anyhow::{Context, Result};

use polkagent_eval::corpus::load_suite_from_json;
use polkagent_eval::regression::RegressionDetector;
use polkagent_eval::report::{EvalReport, report_to_markdown, report_to_json, report_summary};
use polkagent_eval::runner::{EvalRunner, EvalRunnerConfig};

use polkagent_executor_fake::FakeExecutor;

use crate::cli::{EvalCmd, EvalRunCmd, EvalListCmd, EvalReportCmd, EvalCompareCmd};

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Dispatch an `eval` subcommand.
pub fn run(cmd: &EvalCmd) -> Result<()> {
    match cmd {
        EvalCmd::Run(c) => run_suite(c),
        EvalCmd::List(c) => list_suites(c),
        EvalCmd::Report(c) => show_report(c),
        EvalCmd::Compare(c) => compare_reports(c),
    }
}

// ---------------------------------------------------------------------------
// eval run
// ---------------------------------------------------------------------------

fn run_suite(cmd: &EvalRunCmd) -> Result<()> {
    // Resolve suite path: if a directory is given try <dir>/suite.json.
    let suite_path = if cmd.suite_path.is_dir() {
        cmd.suite_path.join("suite.json")
    } else {
        cmd.suite_path.clone()
    };

    let suite = load_suite_from_json(&suite_path)
        .with_context(|| format!("loading eval suite from {}", suite_path.display()))?;

    eprintln!(
        "Running eval suite {:?} ({} cases) ...",
        suite.name,
        suite.len()
    );

    // Build runner — uses the fake executor so the CLI works without a live
    // model key. In production workflows the executor would come from config.
    let config = EvalRunnerConfig {
        concurrency: cmd.concurrency,
        model_id: cmd.model.clone(),
        max_tokens: 2048,
        temperature: Some(0.0),
    };
    let runner = EvalRunner::new(FakeExecutor::new(), config);

    // Run the suite on a temporary Tokio runtime.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("building Tokio runtime")?;

    let report = rt.block_on(runner.run_suite(&suite));

    // Print result.
    if cmd.json {
        let json = report_to_json(&report);
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        print!("{}", report_to_markdown(&report));
        eprintln!("{}", report_summary(&report));
    }

    // Optionally save JSON report.
    if let Some(output_path) = &cmd.output {
        save_report_json(&report, output_path)?;
        eprintln!("Report saved to {}", output_path.display());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// eval list
// ---------------------------------------------------------------------------

fn list_suites(cmd: &EvalListCmd) -> Result<()> {
    let dir = &cmd.dir;

    if !dir.exists() {
        anyhow::bail!(
            "Eval suite directory not found: {}. \
             Create fixtures/evals/ with suite subdirectories.",
            dir.display()
        );
    }

    let mut suites: Vec<SuiteEntry> = Vec::new();

    let read_dir = std::fs::read_dir(dir)
        .with_context(|| format!("reading directory {}", dir.display()))?;

    for entry in read_dir.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        // Each subdirectory may contain a suite.json.
        let suite_json = path.join("suite.json");
        if suite_json.exists() {
            match load_suite_from_json(&suite_json) {
                Ok(suite) => suites.push(SuiteEntry {
                    path: suite_json,
                    name: suite.name.clone(),
                    description: suite.description.clone(),
                    version: suite.version.clone(),
                    case_count: suite.len(),
                }),
                Err(e) => {
                    eprintln!("Warning: could not load {}: {e}", suite_json.display());
                }
            }
        }
    }

    suites.sort_by(|a, b| a.name.cmp(&b.name));

    if cmd.json {
        let items: Vec<serde_json::Value> = suites
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "description": s.description,
                    "version": s.version,
                    "cases": s.case_count,
                    "path": s.path.display().to_string(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&serde_json::json!(items))?);
    } else {
        println!("Available eval suites in {}:", dir.display());
        println!("{}", "-".repeat(60));
        if suites.is_empty() {
            println!("  (none found)");
        } else {
            for s in &suites {
                println!("  {} (v{}) — {} cases", s.name, s.version, s.case_count);
                println!("    {}", s.description);
                println!("    Path: {}", s.path.display());
                println!();
            }
        }
    }

    Ok(())
}

struct SuiteEntry {
    path: std::path::PathBuf,
    name: String,
    description: String,
    version: String,
    case_count: usize,
}

// ---------------------------------------------------------------------------
// eval report
// ---------------------------------------------------------------------------

fn show_report(cmd: &EvalReportCmd) -> Result<()> {
    let report = load_report_json(&cmd.report_path)?;

    if cmd.json {
        let json = report_to_json(&report);
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        print!("{}", report_to_markdown(&report));
        println!();
        println!("{}", report_summary(&report));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// eval compare
// ---------------------------------------------------------------------------

fn compare_reports(cmd: &EvalCompareCmd) -> Result<()> {
    let baseline = load_report_json(&cmd.baseline)
        .with_context(|| format!("loading baseline from {}", cmd.baseline.display()))?;
    let current = load_report_json(&cmd.current)
        .with_context(|| format!("loading current from {}", cmd.current.display()))?;

    let detector = RegressionDetector::with_min_delta(cmd.min_delta);
    let result = detector.compare_reports(&baseline, &current);

    if cmd.json {
        let json = serde_json::json!({
            "baseline_suite": baseline.suite_name,
            "current_suite": current.suite_name,
            "regressions": result.regressions,
            "improvements": result.improvements,
            "unchanged": result.unchanged,
            "new_cases": result.new_cases,
            "removed_cases": result.removed_cases,
            "is_clean": result.is_clean(),
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        println!("# Eval Comparison");
        println!();
        println!("Baseline: {} ({})", baseline.suite_name, cmd.baseline.display());
        println!("Current:  {} ({})", current.suite_name, cmd.current.display());
        println!();

        // Summary table.
        println!("## Summary");
        println!();
        println!("| Metric | Value |");
        println!("|--------|-------|");
        println!("| Regressions | {} |", result.regressions.len());
        println!("| Improvements | {} |", result.improvements.len());
        println!("| Unchanged | {} |", result.unchanged);
        println!("| New cases | {} |", result.new_cases);
        println!("| Removed cases | {} |", result.removed_cases);
        println!();

        // Baseline vs current mean score.
        println!("| Baseline mean score | {:.3} |", baseline.mean_score);
        println!("| Current mean score  | {:.3} |", current.mean_score);
        let delta = current.mean_score - baseline.mean_score;
        let sign = if delta >= 0.0 { "+" } else { "" };
        println!("| Delta               | {sign}{delta:.3} |");
        println!();

        if !result.regressions.is_empty() {
            println!("## Regressions");
            println!();
            println!("| Case ID | Baseline | Current | Delta |");
            println!("|---------|----------|---------|-------|");
            for r in &result.regressions {
                println!(
                    "| {} | {:.3} | {:.3} | {:.3} |",
                    r.case_id, r.baseline_score, r.current_score, r.delta
                );
            }
            println!();
        }

        if !result.improvements.is_empty() {
            println!("## Improvements");
            println!();
            println!("| Case ID | Baseline | Current | Delta |");
            println!("|---------|----------|---------|-------|");
            for i in &result.improvements {
                println!(
                    "| {} | {:.3} | {:.3} | +{:.3} |",
                    i.case_id, i.baseline_score, i.current_score, i.delta
                );
            }
            println!();
        }

        if result.is_clean() {
            println!("No regressions detected.");
        } else {
            println!(
                "{} regression(s) detected.",
                result.regressions.len()
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn save_report_json(report: &EvalReport, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(report).context("serializing report")?;
    std::fs::write(path, json)
        .with_context(|| format!("writing report to {}", path.display()))?;
    Ok(())
}

fn load_report_json(path: &Path) -> Result<EvalReport> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading report from {}", path.display()))?;
    serde_json::from_str::<EvalReport>(&content)
        .with_context(|| format!("parsing report JSON from {}", path.display()))
}
