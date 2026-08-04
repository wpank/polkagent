//! [`PaymentStore`] trait implementation for [`SqlitePool`].
//!
//! Wraps synchronous `rusqlite` calls in [`tokio::task::spawn_blocking`] to
//! satisfy the async trait interface. The writer connection is protected by a
//! `parking_lot::Mutex` inside `SqlitePool`, so each method acquires it briefly
//! within the blocking closure.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use polkagent_payment::{
    types::{
        Amount, AssetId, CostRecord, PaymentIntent, PaymentReceipt, PaymentStatus, UsageSummary,
    },
    BalanceSummary, PaymentError, PaymentStore,
};

use crate::pool::SqlitePool;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Map a `rusqlite::Error` to `PaymentError`.
fn map_err(e: rusqlite::Error) -> PaymentError {
    PaymentError::store(format!("sqlite error: {e}"))
}

/// Map a `rusqlite::Error` that may be a UNIQUE violation.
fn map_err_unique(e: rusqlite::Error, key: &str) -> PaymentError {
    match &e {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                ..
            },
            _,
        ) => PaymentError::IdempotencyConflict {
            key: key.to_string(),
        },
        _ => map_err(e),
    }
}

/// Parse an ISO-8601 timestamp string.
fn parse_ts(s: &str) -> Result<DateTime<Utc>, PaymentError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| PaymentError::store(format!("invalid timestamp '{s}': {e}")))
}

/// Encode `u128` as a decimal string (SQLite has no native 128-bit integer).
fn encode_u128(v: u128) -> String {
    v.to_string()
}

/// Decode a decimal string back to `u128`.
fn decode_u128(s: &str) -> Result<u128, PaymentError> {
    s.parse::<u128>()
        .map_err(|e| PaymentError::store(format!("invalid u128 '{s}': {e}")))
}

/// Encode an `AssetId` as a JSON string for storage.
fn encode_asset(asset: &AssetId) -> Result<String, PaymentError> {
    serde_json::to_string(asset)
        .map_err(|e| PaymentError::store(format!("asset serialization: {e}")))
}

/// Decode an `AssetId` from a JSON string.
fn decode_asset(s: &str) -> Result<AssetId, PaymentError> {
    serde_json::from_str(s).map_err(|e| PaymentError::store(format!("asset deserialization: {e}")))
}

/// Encode a `PaymentStatus` as its string representation.
fn encode_status(status: PaymentStatus) -> String {
    status.to_string()
}

/// Decode a `PaymentStatus` from its string representation.
fn decode_status(s: &str) -> Result<PaymentStatus, PaymentError> {
    match s {
        "pending" => Ok(PaymentStatus::Pending),
        "approved" => Ok(PaymentStatus::Approved),
        "submitted" => Ok(PaymentStatus::Submitted),
        "confirmed" => Ok(PaymentStatus::Confirmed),
        "failed" => Ok(PaymentStatus::Failed),
        "cancelled" => Ok(PaymentStatus::Cancelled),
        other => Err(PaymentError::store(format!(
            "unknown payment status: '{other}'"
        ))),
    }
}

// ---------------------------------------------------------------------------
// PaymentStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl PaymentStore for SqlitePool {
    async fn record_cost(&self, cost_record: CostRecord) -> Result<(), PaymentError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let id = Uuid::now_v7().to_string();
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO cost_records
                         (id, run_id, provider, model, input_tokens, output_tokens,
                          estimated_usd, recorded_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        id,
                        cost_record.run_id,
                        cost_record.provider,
                        cost_record.model,
                        cost_record.input_tokens as i64,
                        cost_record.output_tokens as i64,
                        cost_record.estimated_usd,
                        cost_record.recorded_at.to_rfc3339(),
                    ],
                )
                .map_err(map_err)?;
            Ok(())
        })
        .await
        .map_err(|e| PaymentError::store(format!("blocking task panicked: {e}")))?
    }

    async fn get_costs(&self, run_id: &str) -> Result<Vec<CostRecord>, PaymentError> {
        let pool = self.clone();
        let run_id = run_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT run_id, provider, model, input_tokens, output_tokens,
                             estimated_usd, recorded_at
                      FROM cost_records
                      WHERE run_id = ?1
                      ORDER BY recorded_at ASC",
                )
                .map_err(map_err)?;

            let rows = stmt
                .query_map([&run_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?, // run_id
                        row.get::<_, String>(1)?, // provider
                        row.get::<_, String>(2)?, // model
                        row.get::<_, i64>(3)?,    // input_tokens
                        row.get::<_, i64>(4)?,    // output_tokens
                        row.get::<_, f64>(5)?,    // estimated_usd
                        row.get::<_, String>(6)?, // recorded_at
                    ))
                })
                .map_err(map_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_err)?;

            rows.into_iter()
                .map(
                    |(
                        run_id,
                        provider,
                        model,
                        input_tokens,
                        output_tokens,
                        estimated_usd,
                        recorded_at_str,
                    )| {
                        let recorded_at = parse_ts(&recorded_at_str)?;
                        Ok(CostRecord {
                            run_id,
                            provider,
                            model,
                            input_tokens: input_tokens as u64,
                            output_tokens: output_tokens as u64,
                            estimated_usd,
                            recorded_at,
                        })
                    },
                )
                .collect()
        })
        .await
        .map_err(|e| PaymentError::store(format!("blocking task panicked: {e}")))?
    }

    async fn get_usage(
        &self,
        agent_id: &str,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<UsageSummary, PaymentError> {
        let pool = self.clone();
        let agent_id = agent_id.to_string();
        let since_str = since.to_rfc3339();
        let until_str = until.to_rfc3339();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // We join cost_records to runs via run_id to filter by agent_id.
            // The cost_records table stores run_id; we look up agent_id from runs.
            let row = writer
                .query_row(
                    "SELECT
                         COUNT(DISTINCT cr.run_id),
                         COALESCE(SUM(cr.input_tokens + cr.output_tokens), 0),
                         COALESCE(SUM(cr.estimated_usd), 0.0)
                     FROM cost_records cr
                     INNER JOIN runs r ON r.id = cr.run_id
                     WHERE r.agent_id = ?1
                       AND cr.recorded_at >= ?2
                       AND cr.recorded_at < ?3",
                    rusqlite::params![agent_id, since_str, until_str],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, f64>(2)?,
                        ))
                    },
                )
                .map_err(map_err)?;

            Ok(UsageSummary {
                total_runs: row.0 as u64,
                total_tokens: row.1 as u64,
                estimated_usd: row.2,
                period_start: since,
                period_end: until,
            })
        })
        .await
        .map_err(|e| PaymentError::store(format!("blocking task panicked: {e}")))?
    }

    async fn create_intent(&self, intent: PaymentIntent) -> Result<(), PaymentError> {
        let pool = self.clone();

        let amount_value = encode_u128(intent.amount.value);
        let amount_asset = encode_asset(&intent.amount.asset)?;
        let amount_decimals = intent.amount.decimals;
        let status = encode_status(intent.status);
        let id_str = intent.id.to_string();
        let idempotency_key = intent.idempotency_key.clone();

        tokio::task::spawn_blocking(move || {
            let now = Utc::now().to_rfc3339();
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO payment_intents
                         (id, agent_id, run_id, amount_value, amount_asset, amount_decimals,
                          recipient, idempotency_key, status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    rusqlite::params![
                        id_str,
                        intent.agent_id,
                        intent.run_id,
                        amount_value,
                        amount_asset,
                        amount_decimals as i64,
                        intent.recipient,
                        intent.idempotency_key,
                        status,
                        intent.created_at.to_rfc3339(),
                        now,
                    ],
                )
                .map_err(|e| map_err_unique(e, &idempotency_key))?;
            Ok(())
        })
        .await
        .map_err(|e| PaymentError::store(format!("blocking task panicked: {e}")))?
    }

    async fn get_intent(&self, id: Uuid) -> Result<PaymentIntent, PaymentError> {
        let pool = self.clone();
        let id_str = id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let row = writer
                .query_row(
                    "SELECT id, agent_id, run_id, amount_value, amount_asset, amount_decimals,
                             recipient, idempotency_key, status, created_at
                      FROM payment_intents
                      WHERE id = ?1",
                    [&id_str],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, String>(8)?,
                            row.get::<_, String>(9)?,
                        ))
                    },
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => PaymentError::IntentNotFound { id },
                    other => map_err(other),
                })?;

            let (
                id_s,
                agent_id,
                run_id,
                amt_val,
                amt_asset_s,
                amt_dec,
                recipient,
                idempotency_key,
                status_s,
                created_at_s,
            ) = row;
            let intent_id = id_s
                .parse::<Uuid>()
                .map_err(|e| PaymentError::store(format!("invalid uuid: {e}")))?;
            let asset = decode_asset(&amt_asset_s)?;
            let value = decode_u128(&amt_val)?;
            let created_at = parse_ts(&created_at_s)?;
            let status = decode_status(&status_s)?;

            Ok(PaymentIntent {
                id: intent_id,
                agent_id,
                run_id,
                amount: Amount::new(value, asset, amt_dec as u8),
                recipient,
                idempotency_key,
                created_at,
                status,
                metadata: None,
            })
        })
        .await
        .map_err(|e| PaymentError::store(format!("blocking task panicked: {e}")))?
    }

    async fn update_intent_status(
        &self,
        id: Uuid,
        status: PaymentStatus,
    ) -> Result<(), PaymentError> {
        let pool = self.clone();
        let id_str = id.to_string();
        let status_str = encode_status(status);

        tokio::task::spawn_blocking(move || {
            let now = Utc::now().to_rfc3339();
            let writer = pool.writer();
            let n = writer
                .execute(
                    "UPDATE payment_intents
                     SET status = ?1, updated_at = ?2
                     WHERE id = ?3",
                    rusqlite::params![status_str, now, id_str],
                )
                .map_err(map_err)?;

            if n == 0 {
                return Err(PaymentError::IntentNotFound { id });
            }
            Ok(())
        })
        .await
        .map_err(|e| PaymentError::store(format!("blocking task panicked: {e}")))?
    }

    async fn create_receipt(&self, receipt: PaymentReceipt) -> Result<(), PaymentError> {
        let pool = self.clone();

        let fee_value = encode_u128(receipt.fee_paid.value);
        let fee_asset = encode_asset(&receipt.fee_paid.asset)?;
        let fee_decimals = receipt.fee_paid.decimals;
        let intent_id_str = receipt.intent_id.to_string();

        tokio::task::spawn_blocking(move || {
            let id = Uuid::now_v7().to_string();
            let writer = pool.writer();
            writer
                .execute(
                    "INSERT INTO payment_receipts
                         (id, intent_id, tx_hash, block_number, fee_value, fee_asset,
                          fee_decimals, confirmed_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        id,
                        intent_id_str,
                        receipt.tx_hash,
                        receipt.block_number as i64,
                        fee_value,
                        fee_asset,
                        fee_decimals as i64,
                        receipt.confirmed_at.to_rfc3339(),
                    ],
                )
                .map_err(|e| {
                    // A UNIQUE violation on intent_id means the receipt already exists.
                    match &e {
                        rusqlite::Error::SqliteFailure(
                            rusqlite::ffi::Error {
                                code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                                ..
                            },
                            _,
                        ) => PaymentError::IdempotencyConflict {
                            key: format!("receipt for intent {}", receipt.intent_id),
                        },
                        _ => map_err(e),
                    }
                })?;
            Ok(())
        })
        .await
        .map_err(|e| PaymentError::store(format!("blocking task panicked: {e}")))?
    }

    async fn list_receipts(&self) -> Result<Vec<PaymentReceipt>, PaymentError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let mut stmt = writer
                .prepare(
                    "SELECT intent_id, tx_hash, block_number, fee_value, fee_asset,
                            fee_decimals, confirmed_at
                      FROM payment_receipts
                      ORDER BY confirmed_at DESC",
                )
                .map_err(map_err)?;

            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?, // intent_id
                        row.get::<_, String>(1)?, // tx_hash
                        row.get::<_, i64>(2)?,    // block_number
                        row.get::<_, String>(3)?, // fee_value
                        row.get::<_, String>(4)?, // fee_asset
                        row.get::<_, i64>(5)?,    // fee_decimals
                        row.get::<_, String>(6)?, // confirmed_at
                    ))
                })
                .map_err(map_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_err)?;

            rows.into_iter()
                .map(
                    |(
                        intent_id_s,
                        tx_hash,
                        block_number,
                        fee_val,
                        fee_asset_s,
                        fee_dec,
                        confirmed_at_s,
                    )| {
                        let intent_id = intent_id_s
                            .parse::<Uuid>()
                            .map_err(|e| PaymentError::store(format!("invalid uuid: {e}")))?;
                        let asset = decode_asset(&fee_asset_s)?;
                        let value = decode_u128(&fee_val)?;
                        let confirmed_at = parse_ts(&confirmed_at_s)?;

                        Ok(PaymentReceipt {
                            intent_id,
                            tx_hash,
                            block_number: block_number as u64,
                            fee_paid: Amount::new(value, asset, fee_dec as u8),
                            confirmed_at,
                        })
                    },
                )
                .collect()
        })
        .await
        .map_err(|e| PaymentError::store(format!("blocking task panicked: {e}")))?
    }

    async fn get_receipt(&self, intent_id: Uuid) -> Result<PaymentReceipt, PaymentError> {
        let pool = self.clone();
        let intent_id_str = intent_id.to_string();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();
            let row = writer
                .query_row(
                    "SELECT intent_id, tx_hash, block_number, fee_value, fee_asset,
                            fee_decimals, confirmed_at
                      FROM payment_receipts
                      WHERE intent_id = ?1",
                    [&intent_id_str],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, String>(6)?,
                        ))
                    },
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        PaymentError::IntentNotFound { id: intent_id }
                    }
                    other => map_err(other),
                })?;

            let (intent_id_s, tx_hash, block_number, fee_val, fee_asset_s, fee_dec, confirmed_at_s) = row;
            let id = intent_id_s.parse::<Uuid>()
                .map_err(|e| PaymentError::store(format!("invalid uuid: {e}")))?;
            let asset = decode_asset(&fee_asset_s)?;
            let value = decode_u128(&fee_val)?;
            let confirmed_at = parse_ts(&confirmed_at_s)?;

            Ok(PaymentReceipt {
                intent_id: id,
                tx_hash,
                block_number: block_number as u64,
                fee_paid: Amount::new(value, asset, fee_dec as u8),
                confirmed_at,
            })
        })
        .await
        .map_err(|e| PaymentError::store(format!("blocking task panicked: {e}")))?
    }

    async fn get_balance(&self) -> Result<BalanceSummary, PaymentError> {
        let pool = self.clone();

        tokio::task::spawn_blocking(move || {
            let writer = pool.writer();

            // Sum all cost records to compute total spend.
            let total_usd: f64 = writer
                .query_row(
                    "SELECT COALESCE(SUM(estimated_usd), 0.0) FROM cost_records",
                    [],
                    |row| row.get(0),
                )
                .map_err(map_err)?;

            // Convert USD to micro-dollars for the integer field.
            let total_spent_micro = (total_usd * 1_000_000.0) as u128;

            Ok(BalanceSummary {
                available: None,
                currency: "USD".to_owned(),
                total_spent: total_spent_micro,
                budget_configured: false,
                budget_limit: None,
            })
        })
        .await
        .map_err(|e| PaymentError::store(format!("blocking task panicked: {e}")))?
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations;
    use chrono::Utc;

    fn test_pool() -> SqlitePool {
        let pool = SqlitePool::open_in_memory().expect("open in-memory pool");
        {
            let writer = pool.writer();
            migrations::migrate(&writer).expect("migrate");
        }
        pool
    }

    fn make_intent(agent_id: &str, run_id: &str) -> PaymentIntent {
        PaymentIntent {
            id: Uuid::now_v7(),
            agent_id: agent_id.to_string(),
            run_id: run_id.to_string(),
            amount: Amount::new(1_000_000_000, AssetId::Native, 10),
            recipient: "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY".to_string(),
            idempotency_key: Uuid::now_v7().to_string(),
            created_at: Utc::now(),
            status: PaymentStatus::Pending,
            metadata: None,
        }
    }

    fn make_cost_record(run_id: &str) -> CostRecord {
        CostRecord {
            run_id: run_id.to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4".to_string(),
            input_tokens: 1000,
            output_tokens: 500,
            estimated_usd: 0.0105,
            recorded_at: Utc::now(),
        }
    }

    // --- Intent CRUD ---

    #[tokio::test]
    async fn create_and_get_intent() {
        let pool = test_pool();
        let intent = make_intent("agent-1", "run-1");
        let id = intent.id;

        PaymentStore::create_intent(&pool, intent.clone())
            .await
            .expect("create intent");

        let fetched = PaymentStore::get_intent(&pool, id)
            .await
            .expect("get intent");

        assert_eq!(fetched.id, id);
        assert_eq!(fetched.agent_id, "agent-1");
        assert_eq!(fetched.run_id, "run-1");
        assert_eq!(fetched.status, PaymentStatus::Pending);
        assert_eq!(fetched.amount.value, 1_000_000_000);
    }

    #[tokio::test]
    async fn get_intent_not_found() {
        let pool = test_pool();
        let id = Uuid::now_v7();
        let err = PaymentStore::get_intent(&pool, id)
            .await
            .expect_err("should not find non-existent intent");
        assert!(matches!(err, PaymentError::IntentNotFound { .. }));
    }

    #[tokio::test]
    async fn create_duplicate_idempotency_key_fails() {
        let pool = test_pool();
        let mut intent1 = make_intent("agent-1", "run-1");
        let mut intent2 = make_intent("agent-1", "run-2");
        // Same idempotency key on different IDs.
        let shared_key = Uuid::now_v7().to_string();
        intent1.idempotency_key = shared_key.clone();
        intent2.idempotency_key = shared_key.clone();

        PaymentStore::create_intent(&pool, intent1)
            .await
            .expect("first create");

        let err = PaymentStore::create_intent(&pool, intent2)
            .await
            .expect_err("should fail on duplicate idempotency key");
        assert!(matches!(err, PaymentError::IdempotencyConflict { .. }));
    }

    #[tokio::test]
    async fn update_intent_status() {
        let pool = test_pool();
        let intent = make_intent("agent-1", "run-1");
        let id = intent.id;

        PaymentStore::create_intent(&pool, intent)
            .await
            .expect("create");

        PaymentStore::update_intent_status(&pool, id, PaymentStatus::Approved)
            .await
            .expect("update");

        let fetched = PaymentStore::get_intent(&pool, id).await.expect("get");
        assert_eq!(fetched.status, PaymentStatus::Approved);
    }

    #[tokio::test]
    async fn update_intent_status_not_found() {
        let pool = test_pool();
        let err =
            PaymentStore::update_intent_status(&pool, Uuid::now_v7(), PaymentStatus::Confirmed)
                .await
                .expect_err("should fail for non-existent intent");
        assert!(matches!(err, PaymentError::IntentNotFound { .. }));
    }

    #[tokio::test]
    async fn all_status_transitions() {
        let pool = test_pool();
        let intent = make_intent("agent-1", "run-1");
        let id = intent.id;
        PaymentStore::create_intent(&pool, intent)
            .await
            .expect("create");

        for status in [
            PaymentStatus::Approved,
            PaymentStatus::Submitted,
            PaymentStatus::Confirmed,
        ] {
            PaymentStore::update_intent_status(&pool, id, status)
                .await
                .expect("transition");
            let fetched = PaymentStore::get_intent(&pool, id).await.expect("get");
            assert_eq!(fetched.status, status);
        }
    }

    #[tokio::test]
    async fn intent_with_token_asset() {
        let pool = test_pool();
        let asset = AssetId::Token {
            chain: "polkadot-asset-hub".into(),
            symbol: "USDT".into(),
            decimals: 6,
        };
        let mut intent = make_intent("agent-1", "run-1");
        intent.amount = Amount::new(1_500_000, asset.clone(), 6);
        let id = intent.id;

        PaymentStore::create_intent(&pool, intent)
            .await
            .expect("create");

        let fetched = PaymentStore::get_intent(&pool, id).await.expect("get");
        assert_eq!(fetched.amount.value, 1_500_000);
        assert_eq!(fetched.amount.decimals, 6);
        assert_eq!(fetched.amount.asset, asset);
    }

    #[tokio::test]
    async fn intent_with_max_u128_value() {
        let pool = test_pool();
        let mut intent = make_intent("agent-1", "run-1");
        intent.amount = Amount::new(u128::MAX, AssetId::Native, 10);
        let id = intent.id;

        PaymentStore::create_intent(&pool, intent)
            .await
            .expect("create");

        let fetched = PaymentStore::get_intent(&pool, id).await.expect("get");
        assert_eq!(fetched.amount.value, u128::MAX);
    }

    #[tokio::test]
    async fn intent_status_failed_and_cancelled() {
        let pool = test_pool();

        let mut intent_fail = make_intent("agent-1", "run-1");
        intent_fail.status = PaymentStatus::Failed;
        let fail_id = intent_fail.id;
        PaymentStore::create_intent(&pool, intent_fail)
            .await
            .expect("create failed intent");
        let fetched = PaymentStore::get_intent(&pool, fail_id).await.expect("get");
        assert_eq!(fetched.status, PaymentStatus::Failed);

        let mut intent_cancel = make_intent("agent-1", "run-2");
        intent_cancel.status = PaymentStatus::Cancelled;
        let cancel_id = intent_cancel.id;
        PaymentStore::create_intent(&pool, intent_cancel)
            .await
            .expect("create cancelled intent");
        let fetched = PaymentStore::get_intent(&pool, cancel_id)
            .await
            .expect("get");
        assert_eq!(fetched.status, PaymentStatus::Cancelled);
    }

    // --- Receipt tests ---

    #[tokio::test]
    async fn create_receipt() {
        let pool = test_pool();
        let intent = make_intent("agent-1", "run-1");
        let intent_id = intent.id;
        PaymentStore::create_intent(&pool, intent)
            .await
            .expect("create intent");

        let receipt = PaymentReceipt {
            intent_id,
            tx_hash: "0xdeadbeef".to_string(),
            block_number: 12345,
            fee_paid: Amount::new(10_000_000, AssetId::Native, 10),
            confirmed_at: Utc::now(),
        };
        PaymentStore::create_receipt(&pool, receipt)
            .await
            .expect("create receipt");
    }

    #[tokio::test]
    async fn create_duplicate_receipt_fails() {
        let pool = test_pool();
        let intent = make_intent("agent-1", "run-1");
        let intent_id = intent.id;
        PaymentStore::create_intent(&pool, intent)
            .await
            .expect("create intent");

        let receipt = PaymentReceipt {
            intent_id,
            tx_hash: "0xdeadbeef".to_string(),
            block_number: 12345,
            fee_paid: Amount::new(10_000_000, AssetId::Native, 10),
            confirmed_at: Utc::now(),
        };
        PaymentStore::create_receipt(&pool, receipt.clone())
            .await
            .expect("first receipt");

        let err = PaymentStore::create_receipt(&pool, receipt)
            .await
            .expect_err("should fail on duplicate receipt for same intent");
        assert!(matches!(err, PaymentError::IdempotencyConflict { .. }));
    }

    #[tokio::test]
    async fn receipt_fk_requires_existing_intent() {
        let pool = test_pool();
        let receipt = PaymentReceipt {
            intent_id: Uuid::now_v7(),
            tx_hash: "0xdeadbeef".to_string(),
            block_number: 1,
            fee_paid: Amount::new(0, AssetId::Native, 10),
            confirmed_at: Utc::now(),
        };
        let err = PaymentStore::create_receipt(&pool, receipt)
            .await
            .expect_err("should fail: no matching intent");
        // Could be a constraint violation or generic store error.
        assert!(matches!(
            err,
            PaymentError::Store(_) | PaymentError::IdempotencyConflict { .. }
        ));
    }

    #[tokio::test]
    async fn receipt_with_token_fee() {
        let pool = test_pool();
        let intent = make_intent("agent-1", "run-1");
        let intent_id = intent.id;
        PaymentStore::create_intent(&pool, intent)
            .await
            .expect("create intent");

        let fee_asset = AssetId::Token {
            chain: "polkadot-asset-hub".to_string(),
            symbol: "USDT".to_string(),
            decimals: 6,
        };
        let receipt = PaymentReceipt {
            intent_id,
            tx_hash: "0xabcdef".to_string(),
            block_number: 9999,
            fee_paid: Amount::new(1_000, fee_asset, 6),
            confirmed_at: Utc::now(),
        };
        PaymentStore::create_receipt(&pool, receipt)
            .await
            .expect("create receipt with token fee");
    }

    // --- Cost record tests ---

    #[tokio::test]
    async fn record_and_get_costs() {
        let pool = test_pool();
        // Insert a dummy agent and run for the FK join in get_usage.
        {
            let writer = pool.writer();
            writer.execute(
                "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                 VALUES ('agent-1', 'Test', 'active', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                [],
            ).expect("insert agent");
            writer.execute(
                "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at)
                 VALUES ('run-1', 'agent-1', 'completed', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                [],
            ).expect("insert run");
        }

        let record = make_cost_record("run-1");
        PaymentStore::record_cost(&pool, record)
            .await
            .expect("record cost");

        let costs = PaymentStore::get_costs(&pool, "run-1")
            .await
            .expect("get costs");
        assert_eq!(costs.len(), 1);
        assert_eq!(costs[0].provider, "anthropic");
        assert_eq!(costs[0].model, "claude-sonnet-4");
        assert_eq!(costs[0].input_tokens, 1000);
        assert_eq!(costs[0].output_tokens, 500);
    }

    #[tokio::test]
    async fn get_costs_empty_for_unknown_run() {
        let pool = test_pool();
        let costs = PaymentStore::get_costs(&pool, "nonexistent-run")
            .await
            .expect("get costs for unknown run");
        assert!(costs.is_empty());
    }

    #[tokio::test]
    async fn record_multiple_costs_for_same_run() {
        let pool = test_pool();
        {
            let writer = pool.writer();
            writer.execute(
                "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                 VALUES ('agent-1', 'Test', 'active', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                [],
            ).expect("insert agent");
            writer.execute(
                "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at)
                 VALUES ('run-1', 'agent-1', 'completed', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                [],
            ).expect("insert run");
        }

        for i in 0..5u64 {
            let record = CostRecord {
                run_id: "run-1".to_string(),
                provider: "openai".to_string(),
                model: format!("gpt-4-{i}"),
                input_tokens: 100 * i,
                output_tokens: 50 * i,
                estimated_usd: 0.001 * i as f64,
                recorded_at: Utc::now(),
            };
            PaymentStore::record_cost(&pool, record)
                .await
                .expect("record");
        }

        let costs = PaymentStore::get_costs(&pool, "run-1")
            .await
            .expect("get costs");
        assert_eq!(costs.len(), 5);
    }

    #[tokio::test]
    async fn get_usage_aggregates_tokens_and_cost() {
        let pool = test_pool();
        {
            let writer = pool.writer();
            writer.execute(
                "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                 VALUES ('agent-1', 'Test', 'active', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                [],
            ).expect("insert agent");
            writer.execute(
                "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at)
                 VALUES ('run-1', 'agent-1', 'completed', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                [],
            ).expect("insert run");
        }

        // Record 3 cost entries for the same run.
        for _ in 0..3 {
            let record = CostRecord {
                run_id: "run-1".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4".to_string(),
                input_tokens: 1000,
                output_tokens: 500,
                estimated_usd: 0.01,
                recorded_at: Utc::now(),
            };
            PaymentStore::record_cost(&pool, record)
                .await
                .expect("record");
        }

        let since = DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let until = DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let summary = PaymentStore::get_usage(&pool, "agent-1", since, until)
            .await
            .expect("get usage");

        assert_eq!(summary.total_runs, 1); // All 3 records belong to 1 run.
        assert_eq!(summary.total_tokens, 3 * (1000 + 500));
        assert!((summary.estimated_usd - 0.03).abs() < 1e-9);
    }

    #[tokio::test]
    async fn get_usage_empty_for_unknown_agent() {
        let pool = test_pool();
        let since = DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let until = DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let summary = PaymentStore::get_usage(&pool, "no-such-agent", since, until)
            .await
            .expect("get usage for unknown agent");

        assert_eq!(summary.total_runs, 0);
        assert_eq!(summary.total_tokens, 0);
        assert_eq!(summary.estimated_usd, 0.0);
    }

    #[tokio::test]
    async fn get_usage_respects_time_range() {
        let pool = test_pool();
        {
            let writer = pool.writer();
            writer.execute(
                "INSERT INTO agents (id, name, state, spec_json, created_at, updated_at)
                 VALUES ('agent-1', 'Test', 'active', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                [],
            ).expect("insert agent");
            writer.execute(
                "INSERT INTO runs (id, agent_id, state, params_json, created_at, updated_at)
                 VALUES ('run-1', 'agent-1', 'completed', '{}', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                [],
            ).expect("insert run");
        }

        // Insert a cost record with a fixed timestamp in the past.
        {
            let writer = pool.writer();
            writer.execute(
                "INSERT INTO cost_records (id, run_id, provider, model, input_tokens, output_tokens, estimated_usd, recorded_at)
                 VALUES ('cr-1', 'run-1', 'anthropic', 'claude-sonnet-4', 500, 250, 0.005, '2023-06-01T12:00:00Z')",
                [],
            ).expect("insert cost record");
        }

        // Query a range that includes 2023-06-01.
        let since = DateTime::parse_from_rfc3339("2023-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let until = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let summary = PaymentStore::get_usage(&pool, "agent-1", since, until)
            .await
            .expect("get usage in range");
        assert_eq!(summary.total_tokens, 750);

        // Query a range that excludes it.
        let since2 = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let until2 = DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let summary2 = PaymentStore::get_usage(&pool, "agent-1", since2, until2)
            .await
            .expect("get usage outside range");
        assert_eq!(summary2.total_tokens, 0);
    }

    // --- Concurrent access test ---

    #[tokio::test]
    async fn concurrent_intent_creation() {
        use std::sync::Arc;
        let pool = Arc::new(test_pool());
        let mut handles = Vec::new();

        for i in 0..10 {
            let pool_clone = pool.clone();
            let handle = tokio::spawn(async move {
                let mut intent = make_intent("agent-1", &format!("run-{i}"));
                intent.idempotency_key = format!("key-concurrent-{i}");
                PaymentStore::create_intent(pool_clone.as_ref(), intent)
                    .await
                    .expect("concurrent create intent");
            });
            handles.push(handle);
        }

        for h in handles {
            h.await.expect("task completed");
        }

        // Verify all 10 intents were created.
        let writer = pool.writer();
        let count: i64 = writer
            .query_row("SELECT COUNT(*) FROM payment_intents", [], |r| r.get(0))
            .expect("count intents");
        assert_eq!(count, 10);
    }

    #[tokio::test]
    async fn intent_round_trip_preserves_fields() {
        let pool = test_pool();
        let id = Uuid::now_v7();
        let created_at = Utc::now();
        let intent = PaymentIntent {
            id,
            agent_id: "my-agent".to_string(),
            run_id: "my-run".to_string(),
            amount: Amount::new(42_000_000_000, AssetId::Native, 10),
            recipient: "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty".to_string(),
            idempotency_key: "unique-key-abc".to_string(),
            created_at,
            status: PaymentStatus::Submitted,
            metadata: None,
        };

        PaymentStore::create_intent(&pool, intent)
            .await
            .expect("create");
        let fetched = PaymentStore::get_intent(&pool, id).await.expect("get");

        assert_eq!(fetched.id, id);
        assert_eq!(fetched.agent_id, "my-agent");
        assert_eq!(fetched.run_id, "my-run");
        assert_eq!(fetched.amount.value, 42_000_000_000);
        assert_eq!(
            fetched.recipient,
            "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty"
        );
        assert_eq!(fetched.idempotency_key, "unique-key-abc");
        assert_eq!(fetched.status, PaymentStatus::Submitted);
    }
}
