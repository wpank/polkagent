//! `polkagent-eval` — Evaluation framework for measuring Polkagent agent performance.
//!
//! This crate provides a complete evaluation harness for benchmarking and
//! regression-testing AI agents in the Polkagent platform.
//!
//! # Module overview
//!
//! | Module | Contents |
//! |--------|----------|
//! | [`types`] | Core eval types: [`EvalSuite`](types::EvalSuite), [`EvalCase`](types::EvalCase), [`EvalCategory`](types::EvalCategory), [`EvalInput`](types::EvalInput), [`Expected`](types::Expected) |
//! | [`runner`] | [`EvalRunner`](runner::EvalRunner): executes suites against a [`ModelExecutor`](polkagent_executor_trait::ModelExecutor) |
//! | [`scorer`] | [`score_case`](scorer::score_case), [`Score`](scorer::Score), [`CheckResult`](scorer::CheckResult) |
//! | [`report`] | [`EvalReport`](report::EvalReport), [`CaseResult`](report::CaseResult), report rendering |
//! | [`corpus`] | Suite loading from disk; [`builtin_safety_suite`](corpus::builtin_safety_suite) |
//! | [`regression`] | [`RegressionDetector`](regression::RegressionDetector), [`compare_reports`](regression::compare_reports) |
//! | [`corpus_manifest`] | [`CorpusManifest`](corpus_manifest::CorpusManifest), [`compute_corpus_digest`](corpus_manifest::compute_corpus_digest), [`verify_corpus_integrity`](corpus_manifest::verify_corpus_integrity), [`load_corpus_from_toml`](corpus_manifest::load_corpus_from_toml) |
//! | [`judge`] | [`ModelAsJudgeScorer`](judge::ModelAsJudgeScorer), [`JudgeConfig`](judge::JudgeConfig), [`JudgeCriterion`](judge::JudgeCriterion), [`JudgeScore`](judge::JudgeScore) |
//! | [`promotion`] | [`PromotionCandidate`](promotion::PromotionCandidate) |
//! | `variant_runner` | **EXPERIMENTAL** (`evolutionary` feature) — multi-variant evaluation comparison |
//! | `thompson` | **EXPERIMENTAL** (`evolutionary` feature) — Thompson sampling selector |

#![forbid(unsafe_code)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod corpus;
pub mod corpus_manifest;
pub mod judge;
pub mod promotion;
pub mod regression;
pub mod report;
pub mod runner;
pub mod scorer;
pub mod types;

#[cfg(feature = "evolutionary")]
pub mod thompson;
#[cfg(feature = "evolutionary")]
pub mod variant_runner;

/// Convert a platform-sized count to `f64` without an unchecked precision-
/// losing cast. Large values are composed from exactly representable 32-bit
/// halves; the final floating-point rounding is appropriate for ratios.
pub(crate) fn usize_to_f64(value: usize) -> f64 {
    let value = u64::try_from(value).unwrap_or(u64::MAX);
    let high = u32::try_from(value >> 32).unwrap_or(u32::MAX);
    let low = u32::try_from(value & u64::from(u32::MAX)).unwrap_or(u32::MAX);
    f64::from(high) * 4_294_967_296.0 + f64::from(low)
}
