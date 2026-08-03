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
