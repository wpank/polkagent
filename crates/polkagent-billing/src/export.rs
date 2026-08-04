use crate::error::BillingError;
use polkagent_payment::CostRecord;

const CSV_HEADER: &str = "run_id,provider,model,input_tokens,output_tokens,estimated_usd,recorded_at";

pub fn export_csv(records: &[CostRecord]) -> Result<String, BillingError> {
    let mut buf = String::with_capacity(CSV_HEADER.len() + records.len() * 120);
    buf.push_str(CSV_HEADER);
    buf.push('\n');

    for record in records {
        buf.push_str(&escape_csv_field(&record.run_id));
        buf.push(',');
        buf.push_str(&escape_csv_field(&record.provider));
        buf.push(',');
        buf.push_str(&escape_csv_field(&record.model));
        buf.push(',');
        buf.push_str(&record.input_tokens.to_string());
        buf.push(',');
        buf.push_str(&record.output_tokens.to_string());
        buf.push(',');
        buf.push_str(&format!("{:.6}", record.estimated_usd));
        buf.push(',');
        buf.push_str(&record.recorded_at.to_rfc3339());
        buf.push('\n');
    }

    Ok(buf)
}

fn escape_csv_field(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') {
        let escaped = field.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        field.to_owned()
    }
}

pub fn export_csv_bytes(records: &[CostRecord]) -> Result<Vec<u8>, BillingError> {
    export_csv(records).map(String::into_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_records() -> Vec<CostRecord> {
        vec![
            CostRecord {
                run_id: "run-1".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4".into(),
                input_tokens: 1000,
                output_tokens: 500,
                estimated_usd: 0.0105,
                recorded_at: Utc::now(),
            },
            CostRecord {
                run_id: "run-2".into(),
                provider: "openai".into(),
                model: "gpt-4o".into(),
                input_tokens: 2000,
                output_tokens: 1000,
                estimated_usd: 0.015,
                recorded_at: Utc::now(),
            },
        ]
    }

    #[test]
    fn export_csv_header() {
        let csv = export_csv(&[]).expect("export");
        assert!(csv.starts_with(CSV_HEADER));
        // Header + trailing newline only.
        let lines: Vec<&str> = csv.trim().split('\n').collect();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn export_csv_with_records() {
        let records = sample_records();
        let csv = export_csv(&records).expect("export");
        let lines: Vec<&str> = csv.trim().split('\n').collect();

        // Header + 2 data rows.
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("run_id,"));
        assert!(lines[1].starts_with("run-1,"));
        assert!(lines[2].starts_with("run-2,"));
    }

    #[test]
    fn export_csv_fields_correct() {
        let records = vec![CostRecord {
            run_id: "r1".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4".into(),
            input_tokens: 100,
            output_tokens: 50,
            estimated_usd: 0.001,
            recorded_at: Utc::now(),
        }];
        let csv = export_csv(&records).expect("export");
        let lines: Vec<&str> = csv.trim().split('\n').collect();
        let fields: Vec<&str> = lines[1].split(',').collect();

        assert_eq!(fields[0], "r1");
        assert_eq!(fields[1], "anthropic");
        assert_eq!(fields[2], "claude-sonnet-4");
        assert_eq!(fields[3], "100");
        assert_eq!(fields[4], "50");
        assert_eq!(fields[5], "0.001000");
    }

    #[test]
    fn escape_csv_field_plain() {
        assert_eq!(escape_csv_field("hello"), "hello");
    }

    #[test]
    fn escape_csv_field_with_comma() {
        assert_eq!(escape_csv_field("a,b"), "\"a,b\"");
    }

    #[test]
    fn escape_csv_field_with_quote() {
        assert_eq!(escape_csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn export_csv_bytes_matches_string() {
        let records = sample_records();
        let csv_string = export_csv(&records).expect("export");
        let csv_bytes = export_csv_bytes(&records).expect("export bytes");
        assert_eq!(csv_bytes, csv_string.as_bytes());
    }
}
