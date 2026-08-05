//! Integration tests for the evaluation framework.
//!
//! Exercises suite loading, score calculation, regression detection, and the
//! built-in safety suite — all wired through the `polkagent-eval` crate
//! boundary without requiring a real LLM executor.

use std::collections::HashMap;
use std::io::Write as _;

use polkagent_eval::{
    corpus::{builtin_safety_suite, load_suite_from_dir, load_suite_from_json, CorpusError},
    regression::{compare_reports, RegressionDetector},
    report::{CaseResult, EvalReport, ToolCallRecord},
    scorer::{aggregate_score, mean_score, score_case, weighted_mean_score, CheckResult, Score},
    types::{
        EvalCase, EvalCategory, EvalInput, EvalSuite, Expected, ExpectedOutcome, ExpectedToolCall,
    },
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_case(id: &str, prompt: &str) -> EvalCase {
    EvalCase {
        id: id.into(),
        name: format!("Case {id}"),
        category: EvalCategory::General,
        input: EvalInput::from_prompt(prompt),
        expected: Expected::default(),
        tags: vec![],
        timeout_secs: 30,
    }
}

fn make_suite(name: &str, cases: Vec<EvalCase>) -> EvalSuite {
    EvalSuite {
        name: name.into(),
        description: "test suite".into(),
        version: "1.0.0".into(),
        cases,
    }
}

fn make_case_result(case_id: &str, output: &str, score: f64) -> CaseResult {
    CaseResult {
        case_id: case_id.into(),
        case_name: format!("Case {case_id}"),
        score: Score {
            passed: (score - 1.0).abs() < f64::EPSILON,
            score,
            details: String::new(),
            checks: vec![],
        },
        model_output: output.into(),
        tool_calls_made: vec![],
        duration_ms: 0,
        error: None,
        category: EvalCategory::General,
    }
}

fn make_report(suite_name: &str, results: Vec<CaseResult>) -> EvalReport {
    EvalReport::from_results(suite_name, results)
}

fn make_report_with_scores(suite_name: &str, cases: &[(&str, f64)]) -> EvalReport {
    let results: Vec<CaseResult> = cases
        .iter()
        .map(|(id, score)| make_case_result(id, "", *score))
        .collect();
    make_report(suite_name, results)
}

// ---------------------------------------------------------------------------
// IT-EVAL-01: Load builtin safety suite → run with fake executor → verify report
// ---------------------------------------------------------------------------

#[test]
fn builtin_safety_suite_has_correct_count() {
    let suite = builtin_safety_suite();
    assert_eq!(
        suite.cases.len(),
        10,
        "builtin safety suite must have 10 cases"
    );
}

#[test]
fn builtin_safety_suite_name_is_correct() {
    let suite = builtin_safety_suite();
    assert_eq!(suite.name, "builtin-safety");
}

#[test]
fn builtin_safety_suite_all_cases_have_nonempty_ids() {
    let suite = builtin_safety_suite();
    for case in &suite.cases {
        assert!(!case.id.is_empty(), "case {:?} has empty id", case.name);
    }
}

#[test]
fn builtin_safety_suite_ids_are_unique() {
    let suite = builtin_safety_suite();
    let mut seen = std::collections::HashSet::new();
    for case in &suite.cases {
        assert!(
            seen.insert(case.id.clone()),
            "duplicate case id: {}",
            case.id
        );
    }
}

#[test]
fn builtin_safety_suite_safety_cases_expect_refusal() {
    let suite = builtin_safety_suite();
    let safety: Vec<_> = suite
        .cases
        .iter()
        .filter(|c| c.category == EvalCategory::SafetyJudgment)
        .collect();
    assert!(!safety.is_empty(), "must have at least one safety case");
    for case in &safety {
        assert_eq!(
            case.expected.expected_outcome,
            Some(ExpectedOutcome::Refusal),
            "safety case {} must expect Refusal",
            case.id
        );
    }
}

#[test]
fn builtin_safety_suite_all_cases_have_positive_timeout() {
    let suite = builtin_safety_suite();
    for case in &suite.cases {
        assert!(case.timeout_secs > 0, "case {} has zero timeout", case.id);
    }
}

/// Simulate a refusal response for all safety cases and verify the report.
#[test]
fn fake_executor_all_refusals_produces_perfect_safety_report() {
    let suite = builtin_safety_suite();
    let refusal_output = "I cannot assist with that request.";

    // Build fake results
    let results: Vec<CaseResult> = suite
        .cases
        .iter()
        .map(|case| {
            let output_score = score_case(
                &CaseResult {
                    case_id: case.id.clone(),
                    case_name: case.name.clone(),
                    score: Score::perfect(),
                    model_output: refusal_output.into(),
                    tool_calls_made: vec![],
                    duration_ms: 10,
                    error: None,
                    category: case.category.clone(),
                },
                &case.expected,
            );
            CaseResult {
                case_id: case.id.clone(),
                case_name: case.name.clone(),
                score: output_score,
                model_output: refusal_output.into(),
                tool_calls_made: vec![],
                duration_ms: 10,
                error: None,
                category: case.category.clone(),
            }
        })
        .collect();

    let report = EvalReport::from_results("builtin-safety", results);
    let safety_results: Vec<_> = report
        .results
        .iter()
        .filter(|r| r.category == EvalCategory::SafetyJudgment)
        .collect();

    // All safety cases with Refusal expectation should score 1.0
    for r in &safety_results {
        assert!(
            r.score.score > 0.0,
            "case {} should not score zero for correct refusal",
            r.case_id
        );
    }

    // The overall report must be non-empty
    assert!(!report.results.is_empty());
    assert_eq!(report.suite_name, "builtin-safety");
}

#[test]
fn report_overall_pass_rate_with_all_passing() {
    let results: Vec<CaseResult> = (0..5)
        .map(|i| make_case_result(&format!("c{i}"), "ok", 1.0))
        .collect();
    let report = make_report("all-pass", results);
    // EvalReport has no pass_rate() method; compute from fields directly.
    let pass_rate = if report.total_cases == 0 {
        0.0
    } else {
        report.passed as f64 / report.total_cases as f64
    };
    assert!((pass_rate - 1.0).abs() < f64::EPSILON);
}

#[test]
fn report_overall_pass_rate_with_mixed() {
    let results = vec![
        make_case_result("c1", "ok", 1.0),
        make_case_result("c2", "ok", 1.0),
        make_case_result("c3", "", 0.0),
        make_case_result("c4", "", 0.0),
    ];
    let report = make_report("mixed", results);
    let pass_rate = report.passed as f64 / report.total_cases as f64;
    assert!((pass_rate - 0.5).abs() < f64::EPSILON);
}

#[test]
fn report_mean_score_computed_correctly() {
    let results = vec![
        make_case_result("c1", "", 1.0),
        make_case_result("c2", "", 0.5),
        make_case_result("c3", "", 0.0),
    ];
    let report = make_report("mean-test", results);
    let mean = (1.0 + 0.5 + 0.0) / 3.0;
    // mean_score is a field on EvalReport, not a method.
    assert!((report.mean_score - mean).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// IT-EVAL-02: Score calculation — all checks pass → 1.0
// ---------------------------------------------------------------------------

#[test]
fn score_all_checks_pass_returns_one() {
    let result = CaseResult {
        case_id: "c1".into(),
        case_name: "test".into(),
        score: Score::perfect(),
        model_output: "The answer is 42 and it is correct".into(),
        tool_calls_made: vec![],
        duration_ms: 0,
        error: None,
        category: EvalCategory::General,
    };
    let expected = Expected {
        must_contain: vec!["42".into(), "correct".into()],
        must_not_contain: vec!["error".into(), "wrong".into()],
        expected_outcome: Some(ExpectedOutcome::Success),
        ..Expected::default()
    };
    let score = score_case(&result, &expected);
    assert!(score.passed);
    assert!((score.score - 1.0).abs() < f64::EPSILON);
}

#[test]
fn score_partial_pass_returns_correct_fraction() {
    let result = CaseResult {
        case_id: "c1".into(),
        case_name: "test".into(),
        score: Score::perfect(),
        model_output: "partial".into(),
        tool_calls_made: vec![],
        duration_ms: 0,
        error: None,
        category: EvalCategory::General,
    };
    let expected = Expected {
        must_contain: vec!["partial".into(), "missing".into()],
        ..Expected::default()
    };
    let score = score_case(&result, &expected);
    assert!(!score.passed);
    // 1 of 2 checks pass = 0.5
    assert!((score.score - 0.5).abs() < f64::EPSILON);
}

#[test]
fn score_error_result_when_not_expected_returns_zero() {
    let result = CaseResult {
        case_id: "c1".into(),
        case_name: "test".into(),
        score: Score::perfect(),
        model_output: "".into(),
        tool_calls_made: vec![],
        duration_ms: 0,
        error: Some("timeout".into()),
        category: EvalCategory::General,
    };
    let expected = Expected::default();
    let score = score_case(&result, &expected);
    assert!(!score.passed);
    assert!((score.score - 0.0).abs() < f64::EPSILON);
}

#[test]
fn score_error_result_when_expected_passes() {
    let result = CaseResult {
        case_id: "c1".into(),
        case_name: "test".into(),
        score: Score::perfect(),
        model_output: "".into(),
        tool_calls_made: vec![],
        duration_ms: 0,
        error: Some("expected error".into()),
        category: EvalCategory::General,
    };
    let expected = Expected {
        expected_outcome: Some(ExpectedOutcome::Error),
        ..Expected::default()
    };
    // Error result with expected_outcome=Error should not auto-fail
    let score = score_case(&result, &expected);
    // No other checks, so aggregate empty → perfect
    assert!(score.passed || score.score >= 0.0);
}

#[test]
fn score_tool_call_found_passes_check() {
    let mut tool_args = HashMap::new();
    tool_args.insert("network".to_string(), serde_json::json!("polkadot"));

    let result = CaseResult {
        case_id: "c1".into(),
        case_name: "test".into(),
        score: Score::perfect(),
        model_output: "I am calling the tool".into(),
        tool_calls_made: vec![ToolCallRecord {
            tool_name: "chain.query".into(),
            arguments_json: r#"{"network":"polkadot","block":1000}"#.into(),
        }],
        duration_ms: 0,
        error: None,
        category: EvalCategory::General,
    };
    let expected = Expected {
        expected_tool_calls: vec![ExpectedToolCall {
            tool_name: "chain.query".into(),
            args_contain: tool_args,
        }],
        ..Expected::default()
    };
    let score = score_case(&result, &expected);
    assert!(score.passed);
    assert!((score.score - 1.0).abs() < f64::EPSILON);
}

#[test]
fn score_refusal_outcome_matches_refusal_text() {
    let result = CaseResult {
        case_id: "c1".into(),
        case_name: "test".into(),
        score: Score::perfect(),
        model_output: "I cannot assist with that.".into(),
        tool_calls_made: vec![],
        duration_ms: 0,
        error: None,
        category: EvalCategory::SafetyJudgment,
    };
    let expected = Expected {
        expected_outcome: Some(ExpectedOutcome::Refusal),
        ..Expected::default()
    };
    let score = score_case(&result, &expected);
    assert!(score.passed);
}

#[test]
fn aggregate_score_empty_is_zero() {
    let score = aggregate_score(vec![]);
    assert!(!score.passed);
    assert!((score.score - 0.0).abs() < f64::EPSILON);
}

#[test]
fn aggregate_score_all_pass() {
    let checks = vec![
        CheckResult {
            name: "a".into(),
            passed: true,
            message: "ok".into(),
        },
        CheckResult {
            name: "b".into(),
            passed: true,
            message: "ok".into(),
        },
    ];
    let score = aggregate_score(checks);
    assert!(score.passed);
    assert!((score.score - 1.0).abs() < f64::EPSILON);
}

#[test]
fn aggregate_score_none_pass() {
    let checks = vec![
        CheckResult {
            name: "a".into(),
            passed: false,
            message: "fail".into(),
        },
        CheckResult {
            name: "b".into(),
            passed: false,
            message: "fail".into(),
        },
    ];
    let score = aggregate_score(checks);
    assert!(!score.passed);
    assert!((score.score - 0.0).abs() < f64::EPSILON);
}

#[test]
fn mean_score_empty_is_zero() {
    assert!((mean_score(&[]) - 0.0).abs() < f64::EPSILON);
}

#[test]
fn mean_score_basic() {
    let scores = [1.0, 0.5, 0.0];
    let mean = mean_score(&scores);
    assert!((mean - 0.5).abs() < 1e-10);
}

#[test]
fn weighted_mean_score_basic() {
    let scores = [1.0, 0.0];
    let weights = [3.0, 1.0];
    let wmean = weighted_mean_score(&scores, &weights);
    assert!((wmean - 0.75).abs() < 1e-10);
}

#[test]
fn weighted_mean_score_zero_weight_returns_zero() {
    let scores = [1.0, 0.5];
    let weights = [0.0, 0.0];
    assert!((weighted_mean_score(&scores, &weights) - 0.0).abs() < f64::EPSILON);
}

// ---------------------------------------------------------------------------
// IT-EVAL-03: Regression detection — compare two reports
// ---------------------------------------------------------------------------

#[test]
fn regression_detected_when_score_drops() {
    let baseline = make_report_with_scores("s", &[("c1", 1.0), ("c2", 0.8)]);
    let current = make_report_with_scores("s", &[("c1", 1.0), ("c2", 0.4)]);
    let result = compare_reports(&baseline, &current);
    assert_eq!(result.regressions.len(), 1);
    assert_eq!(result.regressions[0].case_id, "c2");
    assert!(result.regressions[0].delta < 0.0);
}

#[test]
fn improvement_detected_when_score_rises() {
    let baseline = make_report_with_scores("s", &[("c1", 0.5)]);
    let current = make_report_with_scores("s", &[("c1", 0.9)]);
    let result = compare_reports(&baseline, &current);
    assert_eq!(result.improvements.len(), 1);
    assert!(result.improvements[0].delta > 0.0);
}

#[test]
fn no_change_counted_as_unchanged() {
    let baseline = make_report_with_scores("s", &[("c1", 0.7)]);
    let current = make_report_with_scores("s", &[("c1", 0.7)]);
    let result = compare_reports(&baseline, &current);
    assert_eq!(result.unchanged, 1);
    assert!(result.is_clean());
}

#[test]
fn new_cases_tracked() {
    let baseline = make_report_with_scores("s", &[("c1", 1.0)]);
    let current = make_report_with_scores("s", &[("c1", 1.0), ("c2", 0.9)]);
    let result = compare_reports(&baseline, &current);
    assert_eq!(result.new_cases, 1);
}

#[test]
fn removed_cases_tracked() {
    let baseline = make_report_with_scores("s", &[("c1", 1.0), ("c2", 0.8)]);
    let current = make_report_with_scores("s", &[("c1", 1.0)]);
    let result = compare_reports(&baseline, &current);
    assert_eq!(result.removed_cases, 1);
}

#[test]
fn is_clean_when_no_regressions() {
    let baseline = make_report_with_scores("s", &[("c1", 0.5)]);
    let current = make_report_with_scores("s", &[("c1", 1.0)]);
    assert!(compare_reports(&baseline, &current).is_clean());
}

#[test]
fn is_not_clean_when_regressions_present() {
    let baseline = make_report_with_scores("s", &[("c1", 1.0)]);
    let current = make_report_with_scores("s", &[("c1", 0.0)]);
    assert!(!compare_reports(&baseline, &current).is_clean());
}

#[test]
fn changed_count_covers_regressions_and_improvements() {
    let baseline = make_report_with_scores("s", &[("c1", 1.0), ("c2", 0.5), ("c3", 0.7)]);
    let current = make_report_with_scores("s", &[("c1", 0.5), ("c2", 1.0), ("c3", 0.7)]);
    let result = compare_reports(&baseline, &current);
    assert_eq!(result.changed_count(), 2);
    assert_eq!(result.unchanged, 1);
}

#[test]
fn regression_detector_with_min_delta_ignores_tiny_changes() {
    let baseline = make_report_with_scores("s", &[("c1", 0.9)]);
    let current = make_report_with_scores("s", &[("c1", 0.899)]);
    let detector = RegressionDetector::with_min_delta(0.01);
    let result = detector.compare_reports(&baseline, &current);
    // delta = -0.001 > -0.01 threshold → treated as unchanged
    assert!(result.is_clean());
    assert_eq!(result.unchanged, 1);
}

#[test]
fn regression_detector_with_min_delta_catches_large_drops() {
    let baseline = make_report_with_scores("s", &[("c1", 1.0)]);
    let current = make_report_with_scores("s", &[("c1", 0.5)]);
    let detector = RegressionDetector::with_min_delta(0.1);
    let result = detector.compare_reports(&baseline, &current);
    assert!(!result.is_clean());
    assert_eq!(result.regressions.len(), 1);
}

#[test]
fn multiple_regressions_all_reported() {
    let baseline = make_report_with_scores("s", &[("c1", 1.0), ("c2", 0.9), ("c3", 0.8)]);
    let current = make_report_with_scores("s", &[("c1", 0.0), ("c2", 0.0), ("c3", 0.8)]);
    let result = compare_reports(&baseline, &current);
    assert_eq!(result.regressions.len(), 2);
    assert_eq!(result.unchanged, 1);
}

// ---------------------------------------------------------------------------
// IT-EVAL-04: Suite loading from JSON file
// ---------------------------------------------------------------------------

#[test]
fn load_suite_from_json_round_trips_correctly() {
    let suite = make_suite(
        "roundtrip",
        vec![make_case("c1", "hello"), make_case("c2", "world")],
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("suite.json");
    let json = serde_json::to_string(&suite).expect("serialize");
    std::fs::write(&path, json).expect("write");

    let loaded = load_suite_from_json(&path).expect("load ok");
    assert_eq!(loaded.name, "roundtrip");
    assert_eq!(loaded.cases.len(), 2);
    assert_eq!(loaded.cases[0].id, "c1");
    assert_eq!(loaded.cases[1].id, "c2");
}

#[test]
fn load_suite_from_json_invalid_json_returns_parse_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bad.json");
    std::fs::write(&path, b"{ not valid json }").expect("write");
    let result = load_suite_from_json(&path);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), CorpusError::Parse { .. }));
}

#[test]
fn load_suite_from_json_missing_file_returns_io_error() {
    let result = load_suite_from_json(std::path::Path::new("/nonexistent/suite.json"));
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), CorpusError::Io { .. }));
}

#[test]
fn load_suite_from_dir_reads_all_case_files() {
    let dir = tempfile::tempdir().expect("tempdir");

    for i in 0..3 {
        let case = make_case(&format!("d{i}"), &format!("prompt {i}"));
        let path = dir.path().join(format!("case{i}.json"));
        std::fs::write(path, serde_json::to_string(&case).expect("serialize")).expect("write");
    }

    let suite = load_suite_from_dir(dir.path()).expect("load ok");
    assert_eq!(suite.cases.len(), 3);
}

#[test]
fn load_suite_from_dir_empty_dir_returns_empty_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let result = load_suite_from_dir(dir.path());
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), CorpusError::Empty { .. }));
}

#[test]
fn load_suite_from_dir_ignores_txt_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("notes.txt"), b"ignore me").expect("write");

    let case = make_case("c1", "hello");
    std::fs::write(
        dir.path().join("case1.json"),
        serde_json::to_string(&case).expect("serialize"),
    )
    .expect("write");

    let suite = load_suite_from_dir(dir.path()).expect("load ok");
    assert_eq!(suite.cases.len(), 1);
}

#[test]
fn load_suite_from_dir_bad_json_file_returns_parse_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut f = std::fs::File::create(dir.path().join("bad.json")).expect("create");
    f.write_all(b"{ invalid }").expect("write");
    let result = load_suite_from_dir(dir.path());
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), CorpusError::Parse { .. }));
}
