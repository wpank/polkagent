use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cost::{AggregateCost, RunCostCalculator};
use crate::export;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingSummaryResponse {
    pub run_count: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub period_start: String,
    pub period_end: String,
}

impl BillingSummaryResponse {
    #[must_use]
    pub fn from_aggregate(
        agg: &AggregateCost,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
    ) -> Self {
        Self {
            run_count: agg.run_count,
            total_input_tokens: agg.total_input_tokens,
            total_output_tokens: agg.total_output_tokens,
            total_cost_usd: agg.total_cost_usd,
            period_start: period_start.to_rfc3339(),
            period_end: period_end.to_rfc3339(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillingExportResponse {
    pub content_type: String,
    pub data: String,
    pub record_count: usize,
}

pub fn build_summary(
    records: &[polkagent_payment::CostRecord],
    period_start: DateTime<Utc>,
    period_end: DateTime<Utc>,
) -> BillingSummaryResponse {
    let calc = RunCostCalculator::new();
    let agg = calc.compute_aggregate(records);
    BillingSummaryResponse::from_aggregate(&agg, period_start, period_end)
}

pub fn build_export(
    records: &[polkagent_payment::CostRecord],
) -> Result<BillingExportResponse, crate::error::BillingError> {
    let csv = export::export_csv(records)?;
    Ok(BillingExportResponse {
        content_type: "text/csv".to_owned(),
        data: csv,
        record_count: records.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_records() -> Vec<polkagent_payment::CostRecord> {
        vec![
            polkagent_payment::CostRecord {
                run_id: "run-1".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4".into(),
                input_tokens: 1000,
                output_tokens: 500,
                estimated_usd: 0.01,
                recorded_at: Utc::now(),
            },
            polkagent_payment::CostRecord {
                run_id: "run-2".into(),
                provider: "openai".into(),
                model: "gpt-4o".into(),
                input_tokens: 2000,
                output_tokens: 1000,
                estimated_usd: 0.02,
                recorded_at: Utc::now(),
            },
        ]
    }

    #[test]
    fn build_summary_aggregates() {
        let records = sample_records();
        let now = Utc::now();
        let start = now - chrono::Duration::days(30);
        let summary = build_summary(&records, start, now);

        assert_eq!(summary.run_count, 2);
        assert_eq!(summary.total_input_tokens, 3000);
        assert_eq!(summary.total_output_tokens, 1500);
        assert!((summary.total_cost_usd - 0.03).abs() < 1e-9);
        assert!(!summary.period_start.is_empty());
        assert!(!summary.period_end.is_empty());
    }

    #[test]
    fn build_summary_empty() {
        let now = Utc::now();
        let start = now - chrono::Duration::days(7);
        let summary = build_summary(&[], start, now);

        assert_eq!(summary.run_count, 0);
        assert_eq!(summary.total_input_tokens, 0);
        assert_eq!(summary.total_cost_usd, 0.0);
    }

    #[test]
    fn build_export_produces_csv() {
        let records = sample_records();
        let export = build_export(&records).expect("export");

        assert_eq!(export.content_type, "text/csv");
        assert_eq!(export.record_count, 2);
        assert!(export.data.starts_with("run_id,"));
        assert!(export.data.contains("run-1"));
        assert!(export.data.contains("run-2"));
    }

    #[test]
    fn build_export_empty() {
        let export = build_export(&[]).expect("export");

        assert_eq!(export.record_count, 0);
        assert!(export.data.starts_with("run_id,"));
    }

    #[test]
    fn summary_response_serde_round_trip() {
        let now = Utc::now();
        let start = now - chrono::Duration::days(7);
        let summary = build_summary(&sample_records(), start, now);

        let json = serde_json::to_string(&summary).expect("serialize");
        let back: BillingSummaryResponse = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(summary.run_count, back.run_count);
        assert_eq!(summary.total_input_tokens, back.total_input_tokens);
    }

    #[test]
    fn export_response_serde_round_trip() {
        let export = build_export(&sample_records()).expect("export");
        let json = serde_json::to_string(&export).expect("serialize");
        let back: BillingExportResponse = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(export.content_type, back.content_type);
        assert_eq!(export.record_count, back.record_count);
    }
}
