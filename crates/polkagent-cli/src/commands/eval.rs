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
use polkagent_eval::report::{report_summary, report_to_json, report_to_markdown, EvalReport};
use polkagent_eval::runner::{EvalRunner, EvalRunnerConfig};

use polkagent_executor_fake::FakeExecutor;

use crate::cli::{EvalCmd, EvalCompareCmd, EvalListCmd, EvalReportCmd, EvalRunCmd};

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

    if !suite_path.exists() {
        anyhow::bail!(
            "Eval suite file not found: {}\n\n\
             Hint: Provide a path to a valid suite JSON file.\n\
             Example: polkagent eval run fixtures/evals/my-suite/suite.json\n\
             \n\
             To list available suites: polkagent eval list",
            suite_path.display()
        );
    }

    let suite = load_suite_from_json(&suite_path)
        .with_context(|| format!("loading eval suite from {}", suite_path.display()))?;

    let agent_label = cmd.agent.as_deref().unwrap_or("(default)");

    eprintln!(
        "Running eval suite {:?} ({} cases) against agent {} ...",
        suite.name,
        suite.len(),
        agent_label,
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

    // Print verbose per-case scoring details.
    if cmd.details {
        eprintln!();
        for result in &report.results {
            let status = if result.error.is_some() {
                "SKIP"
            } else if result.score.passed {
                "PASS"
            } else {
                "FAIL"
            };
            eprintln!(
                "  [{status}] {id} ({name}) — score: {score:.3}",
                id = result.case_id,
                name = result.case_name,
                score = result.score.score,
            );
            for check in &result.score.checks {
                let mark = if check.passed { "+" } else { "-" };
                eprintln!("         [{mark}] {}: {}", check.name, check.message);
            }
            if let Some(err) = &result.error {
                eprintln!("         error: {err}");
            }
        }
        eprintln!();
    }

    // Print result.
    if cmd.json {
        let mut json = report_to_json(&report);
        // Include the agent label in JSON output when specified.
        if let Some(agent) = &cmd.agent {
            if let Some(obj) = json.as_object_mut() {
                obj.insert("agent".into(), serde_json::Value::String(agent.clone()));
            }
        }
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        println!("{}", report_to_markdown(&report));
        println!("{}", report_summary(&report));
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

    let read_dir =
        std::fs::read_dir(dir).with_context(|| format!("reading directory {}", dir.display()))?;

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
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!(items))?
        );
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
        println!(
            "Baseline: {} ({})",
            baseline.suite_name,
            cmd.baseline.display()
        );
        println!(
            "Current:  {} ({})",
            current.suite_name,
            cmd.current.display()
        );
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
            println!("{} regression(s) detected.", result.regressions.len());
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
    std::fs::write(path, json).with_context(|| format!("writing report to {}", path.display()))?;
    Ok(())
}

fn load_report_json(path: &Path) -> Result<EvalReport> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading report from {}", path.display()))?;
    serde_json::from_str::<EvalReport>(&content)
        .with_context(|| format!("parsing report JSON from {}", path.display()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use clap::Parser;

    use polkagent_eval::corpus::load_suite_from_json;
    use polkagent_eval::report::{report_to_json, EvalReport};
    use polkagent_eval::types::{EvalCase, EvalCategory, EvalInput, EvalSuite, Expected};

    use crate::cli::Cli;

    // ------------------------------------------------------------------
    // Helper: create a minimal valid suite JSON string
    // ------------------------------------------------------------------
    fn minimal_suite_json() -> String {
        let suite = EvalSuite {
            name: "test-suite".into(),
            description: "A test suite for CLI tests".into(),
            version: "1.0.0".into(),
            cases: vec![EvalCase {
                id: "tc-1".into(),
                name: "basic case".into(),
                category: EvalCategory::General,
                input: EvalInput::from_prompt("What is 2+2?"),
                expected: Expected {
                    must_contain: vec!["4".into()],
                    ..Expected::default()
                },
                tags: vec!["math".into()],
                timeout_secs: 30,
            }],
        };
        serde_json::to_string_pretty(&suite).expect("serialize suite")
    }

    // ------------------------------------------------------------------
    // Test 1: Arg parsing — eval run with all flags
    // ------------------------------------------------------------------
    #[test]
    fn parse_eval_run_all_flags() {
        let cli = Cli::try_parse_from([
            "polkagent",
            "eval",
            "run",
            "path/to/suite.json",
            "--agent",
            "my-agent",
            "--output",
            "/tmp/report.json",
            "--concurrency",
            "8",
            "--model",
            "claude-sonnet-4-6",
            "--json",
            "--details",
        ])
        .expect("should parse");

        match cli.command {
            Some(crate::cli::Commands::Eval(crate::cli::EvalCmd::Run(cmd))) => {
                assert_eq!(
                    cmd.suite_path,
                    std::path::PathBuf::from("path/to/suite.json")
                );
                assert_eq!(cmd.agent.as_deref(), Some("my-agent"));
                assert_eq!(
                    cmd.output,
                    Some(std::path::PathBuf::from("/tmp/report.json"))
                );
                assert_eq!(cmd.concurrency, 8);
                assert_eq!(cmd.model, "claude-sonnet-4-6");
                assert!(cmd.json);
                assert!(cmd.details);
            }
            other => panic!("expected Eval::Run, got {:?}", other),
        }
    }

    // ------------------------------------------------------------------
    // Test 2: Arg parsing — eval run defaults
    // ------------------------------------------------------------------
    #[test]
    fn parse_eval_run_defaults() {
        let cli =
            Cli::try_parse_from(["polkagent", "eval", "run", "suite.json"]).expect("should parse");

        match cli.command {
            Some(crate::cli::Commands::Eval(crate::cli::EvalCmd::Run(cmd))) => {
                assert_eq!(cmd.suite_path, std::path::PathBuf::from("suite.json"));
                assert!(cmd.agent.is_none());
                assert!(cmd.output.is_none());
                assert_eq!(cmd.concurrency, 4);
                assert_eq!(cmd.model, "claude-opus-4-6");
                assert!(!cmd.json);
                assert!(!cmd.details);
            }
            other => panic!("expected Eval::Run, got {:?}", other),
        }
    }

    // ------------------------------------------------------------------
    // Test 3: Arg parsing — short flags (-a, -v)
    // ------------------------------------------------------------------
    #[test]
    fn parse_eval_run_short_flags() {
        let cli = Cli::try_parse_from([
            "polkagent",
            "eval",
            "run",
            "suite.json",
            "-a",
            "agent-42",
            "--details",
        ])
        .expect("should parse");

        match cli.command {
            Some(crate::cli::Commands::Eval(crate::cli::EvalCmd::Run(cmd))) => {
                assert_eq!(cmd.agent.as_deref(), Some("agent-42"));
                assert!(cmd.details);
            }
            other => panic!("expected Eval::Run, got {:?}", other),
        }
    }

    // ------------------------------------------------------------------
    // Test 4: Suite loading from a valid JSON file
    // ------------------------------------------------------------------
    #[test]
    fn suite_loading_from_json_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let suite_path = dir.path().join("suite.json");
        std::fs::write(&suite_path, minimal_suite_json()).expect("write");

        let suite = load_suite_from_json(&suite_path).expect("load suite");
        assert_eq!(suite.name, "test-suite");
        assert_eq!(suite.cases.len(), 1);
        assert_eq!(suite.cases[0].id, "tc-1");
        assert_eq!(suite.cases[0].expected.must_contain, vec!["4"]);
    }

    // ------------------------------------------------------------------
    // Test 5: Helpful error on missing suite file
    // ------------------------------------------------------------------
    #[test]
    fn missing_suite_file_gives_helpful_error() {
        let result = super::run_suite(&crate::cli::EvalRunCmd {
            suite_path: std::path::PathBuf::from("/nonexistent/path/suite.json"),
            agent: None,
            output: None,
            concurrency: 4,
            model: "test-model".into(),
            json: false,
            details: false,
        });

        let err = result.expect_err("should fail for missing file");
        let msg = format!("{err}");
        assert!(
            msg.contains("not found"),
            "Error should mention 'not found', got: {msg}"
        );
        assert!(
            msg.contains("polkagent eval"),
            "Error should contain a helpful hint, got: {msg}"
        );
    }

    // ------------------------------------------------------------------
    // Test 6: JSON output format has expected structure
    // ------------------------------------------------------------------
    #[test]
    fn json_output_format_has_expected_fields() {
        let report = EvalReport::from_results("cli-json-test", Vec::new());
        let json = report_to_json(&report);

        // Verify top-level fields exist.
        assert_eq!(json["suite_name"], "cli-json-test");
        assert!(json["timestamp"].is_string());
        assert_eq!(json["total_cases"], 0);
        assert_eq!(json["passed"], 0);
        assert_eq!(json["failed"], 0);
        assert!(json["mean_score"].is_number());
        assert!(json["results"].is_array());
    }

    // ------------------------------------------------------------------
    // Test 7: JSON output includes agent when specified
    // ------------------------------------------------------------------
    #[test]
    fn json_output_includes_agent_field() {
        let report = EvalReport::from_results("agent-test", Vec::new());
        let mut json = report_to_json(&report);

        // Simulate the agent injection done in run_suite.
        if let Some(obj) = json.as_object_mut() {
            obj.insert("agent".into(), serde_json::Value::String("my-agent".into()));
        }

        assert_eq!(json["agent"], "my-agent");
        assert_eq!(json["suite_name"], "agent-test");
    }

    // ------------------------------------------------------------------
    // Test 8: eval run with output file saves report
    // ------------------------------------------------------------------
    #[test]
    fn run_suite_saves_output_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let suite_path = dir.path().join("suite.json");
        std::fs::write(&suite_path, minimal_suite_json()).expect("write");

        let output_path = dir.path().join("report.json");

        let result = super::run_suite(&crate::cli::EvalRunCmd {
            suite_path: suite_path.clone(),
            agent: Some("test-agent".into()),
            output: Some(output_path.clone()),
            concurrency: 1,
            model: "test-model".into(),
            json: false,
            details: false,
        });

        assert!(result.is_ok(), "run_suite should succeed: {:?}", result);
        assert!(output_path.exists(), "report file should have been created");

        // Verify the saved file is valid JSON and deserializes to an EvalReport.
        let content = std::fs::read_to_string(&output_path).expect("read report");
        let report: EvalReport = serde_json::from_str(&content).expect("parse saved report");
        assert_eq!(report.suite_name, "test-suite");
        assert_eq!(report.total_cases, 1);
    }

    // ------------------------------------------------------------------
    // Test 9: eval list subcommand arg parsing
    // ------------------------------------------------------------------
    #[test]
    fn parse_eval_list_args() {
        let cli = Cli::try_parse_from(["polkagent", "eval", "list", "/tmp/suites", "--json"])
            .expect("should parse");

        match cli.command {
            Some(crate::cli::Commands::Eval(crate::cli::EvalCmd::List(cmd))) => {
                assert_eq!(cmd.dir, std::path::PathBuf::from("/tmp/suites"));
                assert!(cmd.json);
            }
            other => panic!("expected Eval::List, got {:?}", other),
        }
    }

    // ------------------------------------------------------------------
    // Test 10: eval compare subcommand arg parsing
    // ------------------------------------------------------------------
    #[test]
    fn parse_eval_compare_args() {
        let cli = Cli::try_parse_from([
            "polkagent",
            "eval",
            "compare",
            "baseline.json",
            "current.json",
            "--min-delta",
            "0.05",
            "--json",
        ])
        .expect("should parse");

        match cli.command {
            Some(crate::cli::Commands::Eval(crate::cli::EvalCmd::Compare(cmd))) => {
                assert_eq!(cmd.baseline, std::path::PathBuf::from("baseline.json"));
                assert_eq!(cmd.current, std::path::PathBuf::from("current.json"));
                assert!((cmd.min_delta - 0.05).abs() < f64::EPSILON);
                assert!(cmd.json);
            }
            other => panic!("expected Eval::Compare, got {:?}", other),
        }
    }
}
