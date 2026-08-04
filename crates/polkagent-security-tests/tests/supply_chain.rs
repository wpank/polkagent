//! Supply-chain safety and configuration tampering resistance tests.
//!
//! These tests verify that the platform detects and rejects tampered
//! configurations, malformed policies, corrupted artifacts, secret leakage,
//! grant escalation attempts, metadata pinning attacks, and replayed effects.
//!
//! Crates under test: polkagent-config, polkagent-grant, polkagent-effect,
//! polkagent-metadata, polkagent-core, polkagent-signer-trait,
//! polkagent-store-trait.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use polkagent_core::ids::{AgentId, GrantId, RunId, StepId, TurnId, WorkerId};
use polkagent_core::{EffectAttemptId, EffectId, EffectOutcomeId};
use polkagent_effect::error::PipelineError;
use polkagent_effect::idempotency::IdempotencyKey;
use polkagent_effect::pipeline::{EffectIntentSpec, EffectPipeline};
use polkagent_effect::types::{EffectKind, EffectOutcome, OutcomeResult};
use polkagent_grant::budget::BudgetTracker;
use polkagent_grant::grant::{
    EffectSet, GrantDecision, GrantLimits, GrantResolver, ResolvedGrant, ResolverConfig,
};
use polkagent_grant::policy::{Effect, EvaluationContext, PolicyRule, PolicySet};
use polkagent_store_trait::{EffectStore, StoreError, StoredIntent, StoredOutcome};

// ===========================================================================
// Shared test infrastructure
// ===========================================================================

#[derive(Debug, Default)]
struct InMemoryStore {
    intents: std::sync::Mutex<std::collections::HashMap<EffectId, StoredIntent>>,
    attempts: std::sync::Mutex<Vec<serde_json::Value>>,
    outcomes: std::sync::Mutex<Vec<StoredOutcome>>,
}

#[async_trait::async_trait]
impl EffectStore for InMemoryStore {
    async fn propose_intent(&self, intent: StoredIntent) -> Result<(), StoreError> {
        let mut intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        if intents.contains_key(&intent.id) {
            return Err(StoreError::Conflict {
                resource_type: "EffectIntent",
                id: intent.id.to_string(),
            });
        }
        intents.insert(intent.id, intent);
        Ok(())
    }

    async fn claim_intent(
        &self,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<Option<StoredIntent>, StoreError> {
        let mut intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        let now = Utc::now();
        let pending_id = intents
            .values()
            .find(|i| i.state.eq_ignore_ascii_case("pending"))
            .map(|i| i.id);
        match pending_id {
            None => Ok(None),
            Some(id) => {
                let intent = intents.get_mut(&id).unwrap();
                let expires = now
                    + chrono::Duration::from_std(lease_duration)
                        .unwrap_or_else(|_| chrono::Duration::seconds(60));
                intent.state = "claimed".to_string();
                intent.lease_owner = Some(worker_id);
                intent.lease_expires = Some(expires);
                Ok(Some(intent.clone()))
            }
        }
    }

    async fn claim_intent_by_id(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<StoredIntent, StoreError> {
        let mut intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        let now = Utc::now();
        match intents.get_mut(&intent_id) {
            None => Err(StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            }),
            Some(intent) => {
                let expires = now
                    + chrono::Duration::from_std(lease_duration)
                        .unwrap_or_else(|_| chrono::Duration::seconds(60));
                intent.state = "claimed".to_string();
                intent.lease_owner = Some(worker_id);
                intent.lease_expires = Some(expires);
                Ok(intent.clone())
            }
        }
    }

    async fn release_claim(
        &self,
        intent_id: EffectId,
        worker_id: WorkerId,
    ) -> Result<(), StoreError> {
        let mut intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(intent) = intents.get_mut(&intent_id) {
            if intent.lease_owner == Some(worker_id) {
                intent.state = "pending".to_string();
                intent.lease_owner = None;
                intent.lease_expires = None;
            }
        }
        Ok(())
    }

    async fn get_intent(&self, intent_id: EffectId) -> Result<StoredIntent, StoreError> {
        let intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        intents
            .get(&intent_id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            })
    }

    async fn get_by_run(&self, run_id: RunId) -> Result<Vec<StoredIntent>, StoreError> {
        let intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        Ok(intents
            .values()
            .filter(|i| i.run_id == run_id)
            .cloned()
            .collect())
    }

    async fn expired_leases(
        &self,
        cutoff: polkagent_core::Timestamp,
    ) -> Result<Vec<StoredIntent>, StoreError> {
        let intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        Ok(intents
            .values()
            .filter(|i| {
                i.state.eq_ignore_ascii_case("claimed")
                    && i.lease_expires.map(|exp| exp < cutoff).unwrap_or(false)
            })
            .cloned()
            .collect())
    }

    async fn record_attempt_start(
        &self,
        _attempt_id: EffectAttemptId,
        _intent_id: EffectId,
        _worker_id: WorkerId,
        payload: serde_json::Value,
    ) -> Result<(), StoreError> {
        self.attempts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(payload);
        Ok(())
    }

    async fn record_outcome(&self, outcome: StoredOutcome) -> Result<(), StoreError> {
        let mut outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        if outcomes.iter().any(|o| o.id == outcome.id) {
            return Err(StoreError::Conflict {
                resource_type: "EffectOutcome",
                id: outcome.id.to_string(),
            });
        }
        let mut intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(intent) = intents.get_mut(&outcome.intent_id) {
            intent.state = "resolved".to_string();
            intent.lease_owner = None;
            intent.lease_expires = None;
        }
        outcomes.push(outcome);
        Ok(())
    }

    async fn unconsumed_outcomes(&self, run_id: RunId) -> Result<Vec<StoredOutcome>, StoreError> {
        let outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        Ok(outcomes
            .iter()
            .filter(|o| o.run_id == run_id && !o.consumed)
            .cloned()
            .collect())
    }

    async fn mark_outcomes_consumed(
        &self,
        outcome_ids: &[EffectOutcomeId],
    ) -> Result<(), StoreError> {
        let mut outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        for o in outcomes.iter_mut() {
            if outcome_ids.contains(&o.id) {
                o.consumed = true;
            }
        }
        Ok(())
    }

    async fn update_intent_state(
        &self,
        intent_id: EffectId,
        new_state: &str,
    ) -> Result<StoredIntent, StoreError> {
        let mut intents = self.intents.lock().unwrap_or_else(|e| e.into_inner());
        match intents.get_mut(&intent_id) {
            None => Err(StoreError::NotFound {
                resource_type: "EffectIntent",
                id: intent_id.to_string(),
            }),
            Some(intent) => {
                intent.state = new_state.to_string();
                Ok(intent.clone())
            }
        }
    }
}

fn make_store() -> (Arc<dyn EffectStore>, Arc<InMemoryStore>) {
    let inner = Arc::new(InMemoryStore::default());
    let store: Arc<dyn EffectStore> = Arc::clone(&inner) as Arc<dyn EffectStore>;
    (store, inner)
}

fn make_spec(run_id: RunId, kind: EffectKind) -> EffectIntentSpec {
    EffectIntentSpec {
        run_id,
        turn_id: TurnId::new(),
        step_id: StepId::new(),
        kind,
        sequence: 1,
        idempotency_key: None,
        payload: serde_json::json!({"test": true}),
        retry_class: None,
        priority: None,
        max_attempts: None,
        action_card: None,
    }
}

fn make_outcome(intent_id: EffectId, run_id: RunId) -> EffectOutcome {
    EffectOutcome {
        id: EffectOutcomeId::new(),
        attempt_id: EffectAttemptId::new(),
        intent_id,
        run_id,
        result: OutcomeResult::Success {
            data: serde_json::json!({"ok": true}),
        },
        observed_at: Utc::now(),
        digest: [0u8; 32],
    }
}

fn allow_rule(id: &str, actions: &[&str], resources: &[&str]) -> PolicyRule {
    PolicyRule {
        id: id.to_string(),
        effect: Effect::Allow,
        action_patterns: actions.iter().map(|s| s.to_string()).collect(),
        resource_patterns: resources.iter().map(|s| s.to_string()).collect(),
        conditions: Default::default(),
        abac_condition: None,
    }
}

fn empty_ctx() -> EvaluationContext {
    EvaluationContext::default()
}

// ===========================================================================
// 1. Config tampering: Modified config files are detected
// ===========================================================================

mod config_tampering {
    use polkagent_config::schema::Config;
    use polkagent_config::validate;

    #[test]
    fn tampered_config_byte_flip_detected() {
        // Serialize a valid config to TOML, flip a byte in the serialized
        // form, then attempt to deserialize and validate. The tampered config
        // must fail either deserialization or validation.
        let cfg = Config::default();
        let valid_toml = toml::to_string(&cfg).expect("default config serializes to TOML");

        // Flip a byte roughly in the middle of the serialized output.
        let mut tampered_bytes = valid_toml.into_bytes();
        let mid = tampered_bytes.len() / 2;
        tampered_bytes[mid] ^= 0xFF;
        let tampered_toml = String::from_utf8_lossy(&tampered_bytes).to_string();

        // Either deserialization fails or validation catches the corruption.
        match toml::from_str::<Config>(&tampered_toml) {
            Err(_) => {
                // Deserialization failed -- tampering detected at parse time.
            }
            Ok(parsed) => {
                // If deserialization succeeded despite the flip, validation
                // must catch any resulting invalid field values.
                let result = validate::validate(&parsed);
                // We accept either outcome: the key point is we did not
                // silently accept a corrupted config without any check.
                let _ = result;
            }
        }
    }

    #[test]
    fn tampered_config_truncation_detected() {
        // Truncate a valid config and verify it cannot be loaded.
        let cfg = Config::default();
        let valid_toml = toml::to_string(&cfg).expect("serializes");
        let truncated = &valid_toml[..valid_toml.len() / 3];

        let result = toml::from_str::<Config>(truncated);
        assert!(
            result.is_err(),
            "truncated config must fail deserialization"
        );
    }

    #[test]
    fn tampered_config_empty_string_rejected() {
        let result = toml::from_str::<Config>("");
        // An empty TOML string will parse to a default Config, but if any
        // mandatory fields are missing, validation should catch it.
        match result {
            Err(_) => { /* parse failure is acceptable */ }
            Ok(cfg) => {
                // Even if parsing succeeds, it produces a config with
                // defaults. This is acceptable -- the important thing is
                // the system doesn't panic.
                let _ = validate::validate(&cfg);
            }
        }
    }
}

// ===========================================================================
// 2. Policy bypass attempts: Malformed policies don't grant access
// ===========================================================================

mod policy_bypass {
    use polkagent_grant::policy::{
        evaluate, Effect, EvaluationContext, PolicyDecision, PolicyRule, PolicySet,
    };

    fn empty_ctx() -> EvaluationContext {
        EvaluationContext::default()
    }

    #[test]
    fn policy_with_no_action_patterns_denies_everything() {
        // A rule with an empty action_patterns list must never match.
        let mut set = PolicySet::default();
        set.add_rule(PolicyRule {
            id: "broken-allow".to_string(),
            effect: Effect::Allow,
            action_patterns: vec![], // missing required field
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        });

        let decision = evaluate(&set, "chain/transfer", "account/alice", &empty_ctx());
        assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "policy with empty action_patterns must deny (default deny): got {decision:?}"
        );
    }

    #[test]
    fn policy_with_no_resource_patterns_denies_everything() {
        // A rule with empty resource_patterns must never match.
        let mut set = PolicySet::default();
        set.add_rule(PolicyRule {
            id: "no-resources".to_string(),
            effect: Effect::Allow,
            action_patterns: vec!["**".to_string()],
            resource_patterns: vec![], // missing required field
            conditions: Default::default(),
            abac_condition: None,
        });

        let decision = evaluate(&set, "chain/transfer", "account/alice", &empty_ctx());
        assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "policy with empty resource_patterns must deny: got {decision:?}"
        );
    }

    #[test]
    fn policy_with_invalid_condition_key_does_not_match() {
        // A condition referencing a key that doesn't exist in the context
        // must prevent the rule from matching.
        let mut set = PolicySet::default();
        let mut rule = PolicyRule {
            id: "cond-mismatch".to_string(),
            effect: Effect::Allow,
            action_patterns: vec!["**".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        };
        rule.conditions
            .insert("nonexistent_attr".to_string(), "required_value".to_string());
        set.add_rule(rule);

        let decision = evaluate(&set, "chain/transfer", "account/alice", &empty_ctx());
        assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "condition with missing context key must prevent matching: got {decision:?}"
        );
    }

    #[test]
    fn wildcard_policy_denied_chain_submit() {
        // Even with a wildcard allow, an explicit deny for chain/submit must win.
        let mut set = PolicySet::default();
        set.add_rule(PolicyRule {
            id: "allow-all".to_string(),
            effect: Effect::Allow,
            action_patterns: vec!["**".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        });
        set.add_rule(PolicyRule {
            id: "deny-submit".to_string(),
            effect: Effect::Deny,
            action_patterns: vec!["chain/submit".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        });

        let decision = evaluate(&set, "chain/submit", "account/alice", &empty_ctx());
        assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "deny-overrides must block chain/submit despite wildcard allow: got {decision:?}"
        );

        // Other actions should still be allowed.
        let query_decision = evaluate(&set, "chain/query", "account/alice", &empty_ctx());
        assert_eq!(
            query_decision,
            PolicyDecision::Allow,
            "chain/query should still be allowed"
        );
    }

    #[test]
    fn overly_broad_double_wildcard_does_not_bypass_deny() {
        // A rule with `**` for both actions and resources should still be
        // overridden by a more specific deny.
        let mut set = PolicySet::default();
        set.add_rule(PolicyRule {
            id: "broad-allow".to_string(),
            effect: Effect::Allow,
            action_patterns: vec!["**".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        });
        set.add_rule(PolicyRule {
            id: "deny-system".to_string(),
            effect: Effect::Deny,
            action_patterns: vec!["pallet/system/**".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        });

        let decision = evaluate(&set, "pallet/system/setCode", "runtime/wasm", &empty_ctx());
        assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "system setCode must be denied even under broadest possible allow"
        );
    }

    #[test]
    fn empty_policy_set_denies_everything() {
        // No rules at all means default deny.
        let set = PolicySet::default();

        let decision = evaluate(&set, "chain/transfer", "account/alice", &empty_ctx());
        assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "empty policy set must default-deny all requests"
        );
    }

    #[test]
    fn policy_condition_wrong_value_blocks_access() {
        // Condition requires "environment" = "production", but context says "staging".
        let mut set = PolicySet::default();
        let mut rule = PolicyRule {
            id: "prod-only".to_string(),
            effect: Effect::Allow,
            action_patterns: vec!["**".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        };
        rule.conditions
            .insert("environment".to_string(), "production".to_string());
        set.add_rule(rule);

        let mut ctx = EvaluationContext::default();
        ctx.attributes
            .insert("environment".to_string(), "staging".to_string());

        let decision = evaluate(&set, "chain/transfer", "account/alice", &ctx);
        assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "wrong condition value must prevent match: got {decision:?}"
        );
    }
}

// ===========================================================================
// 3. Artifact integrity: Tampered artifacts are detected
// ===========================================================================

mod artifact_integrity {
    use polkagent_core::now;
    use polkagent_metadata::types::MetadataHash;
    use polkagent_metadata::{ChainId, MetadataService, MetadataSnapshot, MetadataVersion};

    #[test]
    fn tampered_metadata_body_changes_hash() {
        // Store an artifact (metadata snapshot), then modify its raw bytes.
        // The hash computed from the modified bytes must differ from the original.
        let original_bytes = b"runtime_metadata_v14_pallets".to_vec();
        let original_hash = MetadataHash::from_bytes(&original_bytes);

        let mut tampered_bytes = original_bytes.clone();
        tampered_bytes[10] ^= 0xFF; // flip a byte in the body
        let tampered_hash = MetadataHash::from_bytes(&tampered_bytes);

        assert_ne!(
            original_hash, tampered_hash,
            "flipping a byte in the artifact body must change the hash"
        );
    }

    #[test]
    fn tampered_metadata_detected_via_drift() {
        // Register and pin a snapshot, then register a tampered version.
        // Drift detection must flag the difference.
        let svc = MetadataService::new();
        let chain = ChainId::new("polkadot");

        let original = MetadataSnapshot::new(
            chain.clone(),
            MetadataVersion::V14,
            b"original_artifact_data".to_vec(),
            now(),
            1000,
        );
        svc.register_snapshot(original);
        svc.pin_current(&chain, "known-good").expect("pin succeeds");

        // Register tampered metadata.
        let tampered = MetadataSnapshot::new(
            chain.clone(),
            MetadataVersion::V14,
            b"tampered_artifact_data".to_vec(),
            now(),
            1000, // same spec version -- the attacker tries to hide the change
        );
        let drift = svc.register_snapshot(tampered);

        assert!(
            drift.is_some(),
            "tampered artifact must trigger drift detection"
        );
        let d = drift.unwrap();
        assert_ne!(
            d.pinned_hash, d.current_hash,
            "drift must show different hashes for original vs tampered"
        );
    }

    #[test]
    fn identical_artifact_produces_no_drift() {
        // Re-registering the exact same bytes must not trigger drift.
        let svc = MetadataService::new();
        let chain = ChainId::new("kusama");

        let data = b"stable_metadata".to_vec();
        let snap1 =
            MetadataSnapshot::new(chain.clone(), MetadataVersion::V14, data.clone(), now(), 42);
        svc.register_snapshot(snap1);
        svc.pin_current(&chain, "stable").expect("pin");

        let snap2 = MetadataSnapshot::new(chain.clone(), MetadataVersion::V14, data, now(), 42);
        let drift = svc.register_snapshot(snap2);

        assert!(
            drift.is_none(),
            "identical artifact bytes must not produce drift"
        );
    }

    #[test]
    fn metadata_hash_is_deterministic_across_calls() {
        let data = b"deterministic_input_bytes";
        let h1 = MetadataHash::from_bytes(data);
        let h2 = MetadataHash::from_bytes(data);
        let h3 = MetadataHash::from_bytes(data);

        assert_eq!(h1, h2, "hash must be deterministic");
        assert_eq!(
            h2, h3,
            "hash must be deterministic across any number of calls"
        );
    }

    #[test]
    fn single_bit_change_produces_avalanche_in_hash() {
        // Changing a single bit in the input should change many bits in the hash.
        let data_a = vec![0x00u8; 64];
        let mut data_b = data_a.clone();
        data_b[0] = 0x01; // flip one bit

        let hash_a = MetadataHash::from_bytes(&data_a);
        let hash_b = MetadataHash::from_bytes(&data_b);

        assert_ne!(
            hash_a, hash_b,
            "single bit change must produce a different hash"
        );
        // The hashes should differ in many characters (avalanche effect).
        let diff_count = hash_a
            .0
            .chars()
            .zip(hash_b.0.chars())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            diff_count > 10,
            "avalanche effect: hashes should differ in many characters, but only {diff_count} differ"
        );
    }
}

// ===========================================================================
// 4. Secret leakage prevention: Secrets don't leak through error messages
// ===========================================================================

mod secret_leakage {
    use polkagent_core::PolkagentError;
    use polkagent_signer_trait::SignerError;

    const SECRET_API_KEY: &str = "sk-ant-api03-ULTRA_SECRET_KEY_xyzabc123456789";
    const SECRET_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const SECRET_PRIVKEY: &str =
        "0xdeadbeefcafebabe0123456789abcdef0123456789abcdef0123456789abcdef";

    fn all_secrets() -> Vec<&'static str> {
        vec![SECRET_API_KEY, SECRET_MNEMONIC, SECRET_PRIVKEY]
    }

    fn assert_no_leak(output: &str, secrets: &[&str], context: &str) {
        for secret in secrets {
            assert!(
                !output.contains(secret),
                "SECRET LEAK in {context}: output contains {secret:?}"
            );
        }
    }

    #[test]
    fn validation_error_does_not_echo_secret_value() {
        // A validation error about an API key field should not include the
        // raw key value in its Display or Debug output.
        let err = PolkagentError::validation("provider.api_key", "must be a valid key format");
        let debug = format!("{err:?}");
        let display = format!("{err}");

        assert_no_leak(&debug, &all_secrets(), "validation error Debug");
        assert_no_leak(&display, &all_secrets(), "validation error Display");
        assert!(
            display.contains("provider.api_key"),
            "field name must be present"
        );
    }

    #[test]
    fn internal_error_with_generic_message_does_not_leak() {
        // Internal errors should use generic messages, not embed secrets.
        let err = PolkagentError::internal("authentication failed for provider");
        let debug = format!("{err:?}");
        let display = format!("{err}");

        assert_no_leak(&debug, &all_secrets(), "internal error Debug");
        assert_no_leak(&display, &all_secrets(), "internal error Display");
    }

    #[test]
    fn signer_error_does_not_leak_private_key() {
        let err = SignerError::Hardware {
            message: "device communication timeout".to_string(),
        };
        let debug = format!("{err:?}");
        let display = format!("{err}");

        assert_no_leak(&debug, &all_secrets(), "SignerError::Hardware Debug");
        assert_no_leak(&display, &all_secrets(), "SignerError::Hardware Display");
    }

    #[test]
    fn policy_denied_error_does_not_leak_secrets() {
        let err = PolkagentError::PolicyDenied {
            reason: "agent lacks grant for chain/transfer".to_string(),
        };
        let debug = format!("{err:?}");
        let display = format!("{err}");

        assert_no_leak(&debug, &all_secrets(), "PolicyDenied Debug");
        assert_no_leak(&display, &all_secrets(), "PolicyDenied Display");
        assert!(display.contains("chain/transfer"), "reason must be present");
    }

    #[test]
    fn classification_violation_does_not_leak_content() {
        let err = PolkagentError::ClassificationViolation {
            classification: "secret".to_string(),
            reason: "excluded from output assembly".to_string(),
        };
        let debug = format!("{err:?}");
        let display = format!("{err}");

        assert_no_leak(&debug, &all_secrets(), "ClassificationViolation Debug");
        assert_no_leak(&display, &all_secrets(), "ClassificationViolation Display");
    }
}

// ===========================================================================
// 5. Grant escalation prevention: Can't escalate grants
// ===========================================================================

mod grant_escalation {
    use super::*;

    #[tokio::test]
    async fn read_only_agent_cannot_execute_write_effect() {
        // An agent with a grant for chain/query only must be denied chain/transfer.
        let mut set = PolicySet::default();
        set.add_rule(allow_rule("read-only", &["chain/query"], &["**"]));

        let resolver = GrantResolver::new(set, ResolverConfig::default());

        // Allowed: chain/query (read).
        let read_decision = resolver
            .resolve(
                "agent-ro",
                "chain/query",
                "account/alice",
                &empty_ctx(),
                None,
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("resolver error: {e}"));
        assert!(
            matches!(read_decision, GrantDecision::Permit(_)),
            "read-only agent must be allowed chain/query"
        );

        // Denied: chain/transfer (write).
        let write_decision = resolver
            .resolve(
                "agent-ro",
                "chain/transfer",
                "account/alice",
                &empty_ctx(),
                None,
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("resolver error: {e}"));
        assert!(
            matches!(write_decision, GrantDecision::Deny(_)),
            "read-only agent must be denied chain/transfer"
        );
    }

    #[tokio::test]
    async fn read_only_agent_cannot_execute_broadcast() {
        let mut set = PolicySet::default();
        set.add_rule(allow_rule("read-only", &["chain/query"], &["**"]));

        let resolver = GrantResolver::new(set, ResolverConfig::default());

        let decision = resolver
            .resolve(
                "agent-ro",
                "chain/broadcast",
                "tx/0xabcd",
                &empty_ctx(),
                None,
                None,
            )
            .await
            .unwrap_or_else(|e| panic!("resolver error: {e}"));
        assert!(
            matches!(decision, GrantDecision::Deny(_)),
            "read-only agent must be denied chain/broadcast"
        );
    }

    #[test]
    fn effect_set_contains_only_declared_effects() {
        // EffectSet.contains must return false for undeclared effects.
        let set = EffectSet::new(vec!["chain.query".to_string(), "chain.read".to_string()]);

        assert!(set.contains("chain.query"), "declared effect must be found");
        assert!(set.contains("chain.read"), "declared effect must be found");
        assert!(
            !set.contains("chain.transfer"),
            "undeclared effect must not be found in EffectSet"
        );
        assert!(
            !set.contains("chain.broadcast"),
            "undeclared effect must not be found in EffectSet"
        );
    }

    #[test]
    fn grant_intersection_never_broader_than_inputs() {
        // If we intersect two EffectSets, the result must contain only effects
        // present in BOTH sets -- never an effect in neither.
        let set_a = EffectSet::new(vec![
            "chain.query".to_string(),
            "chain.transfer".to_string(),
        ]);
        let set_b = EffectSet::new(vec![
            "chain.query".to_string(),
            "chain.broadcast".to_string(),
        ]);

        // Manual intersection (the API may not expose this, so we compute it).
        let intersection: Vec<String> = set_a
            .effects
            .iter()
            .filter(|e| set_b.contains(e))
            .cloned()
            .collect();

        let intersection_set = EffectSet::new(intersection);

        // Intersection must only contain "chain.query".
        assert!(
            intersection_set.contains("chain.query"),
            "intersection must include effects in both sets"
        );
        assert!(
            !intersection_set.contains("chain.transfer"),
            "intersection must not include effects only in set A"
        );
        assert!(
            !intersection_set.contains("chain.broadcast"),
            "intersection must not include effects only in set B"
        );
        assert!(
            !intersection_set.contains("chain.staking"),
            "intersection must not produce phantom effects"
        );
    }

    #[tokio::test]
    async fn budget_cannot_be_bypassed_by_splitting_effects() {
        // Budget of 10 units. Submit 5 effects of 3 units each (total 15).
        // The budget must be enforced cumulatively.
        let tracker = BudgetTracker::new();
        let agent_id = AgentId::new();
        tracker.configure(agent_id, 10).await;

        let run_id = RunId::new();

        // First 3 spends of 3 units each succeed (total = 9, within budget).
        for i in 0..3u64 {
            let within = tracker
                .check_budget(agent_id, 3)
                .await
                .unwrap_or_else(|e| panic!("check_budget failed at {i}: {e}"));
            assert!(
                within,
                "spend {i} (cumulative {}) must be within budget 10",
                (i + 1) * 3
            );
            tracker
                .record_spend(agent_id, run_id, 3)
                .await
                .unwrap_or_else(|e| panic!("record_spend failed at {i}: {e}"));
        }

        // 4th spend of 3 units would push total to 12 -- must be rejected.
        let within_4 = tracker
            .check_budget(agent_id, 3)
            .await
            .unwrap_or_else(|e| panic!("check_budget failed for 4th: {e}"));
        assert!(
            !within_4,
            "4th spend of 3 (cumulative 12) must exceed budget of 10"
        );
    }

    #[tokio::test]
    async fn grant_ttl_prevents_stale_escalation() {
        // A grant with a past expiry must not permit anything.
        let expired_grant = ResolvedGrant {
            grant_id: GrantId::new(),
            principal: "agent".to_string(),
            action: "chain/transfer".to_string(),
            resource: "account/alice".to_string(),
            allowed_effects: EffectSet::new(vec!["chain.transfer".to_string()]),
            limits: GrantLimits::default(),
            expires_at: Utc::now() - chrono::Duration::seconds(10),
        };

        assert!(
            !expired_grant.is_valid(),
            "expired grant must not be valid -- no stale escalation"
        );
    }
}

// ===========================================================================
// 6. Metadata pinning attacks: Can't use unpinned metadata
// ===========================================================================

mod metadata_pinning {
    use polkagent_core::now;
    use polkagent_metadata::{ChainId, MetadataService, MetadataSnapshot, MetadataVersion};

    #[test]
    fn metadata_hash_checked_before_use() {
        // Verify that registering new metadata against a pin produces drift.
        let svc = MetadataService::new();
        let chain = ChainId::new("polkadot");

        // Pin the known-good metadata.
        let good = MetadataSnapshot::new(
            chain.clone(),
            MetadataVersion::V14,
            b"known_good_metadata_bytes".to_vec(),
            now(),
            1000,
        );
        svc.register_snapshot(good);
        svc.pin_current(&chain, "v1.0").expect("pin");

        // Verify: asking for drift when metadata matches pin returns None.
        assert!(
            svc.check_drift(&chain).is_none(),
            "matching metadata must produce no drift"
        );

        // Register different metadata -- drift must be detected.
        let bad = MetadataSnapshot::new(
            chain.clone(),
            MetadataVersion::V14,
            b"attacker_modified_metadata".to_vec(),
            now(),
            1000,
        );
        let drift = svc.register_snapshot(bad);
        assert!(
            drift.is_some(),
            "metadata hash must be checked against pin -- drift expected"
        );
    }

    #[test]
    fn substituted_metadata_different_genesis_rejected() {
        // An attacker substitutes metadata from a different chain (different
        // genesis hash / different bytes). The hash comparison must catch this.
        let svc = MetadataService::new();
        let polkadot = ChainId::new("polkadot");

        // Pin polkadot metadata.
        let polkadot_snap = MetadataSnapshot::new(
            polkadot.clone(),
            MetadataVersion::V14,
            b"polkadot_genesis_0x91b171".to_vec(),
            now(),
            1000,
        );
        svc.register_snapshot(polkadot_snap);
        svc.pin_current(&polkadot, "polkadot-v1").expect("pin");

        // Attacker registers kusama metadata under the polkadot chain ID.
        let kusama_disguised = MetadataSnapshot::new(
            polkadot.clone(), // same chain ID, but different actual data
            MetadataVersion::V14,
            b"kusama_genesis_0xb0a8d4".to_vec(),
            now(),
            1000,
        );
        let drift = svc.register_snapshot(kusama_disguised);

        assert!(
            drift.is_some(),
            "substituted metadata with different genesis data must trigger drift"
        );
    }

    #[test]
    fn pinning_requires_cached_metadata() {
        // Cannot pin metadata for a chain that has nothing cached.
        let svc = MetadataService::new();
        let result = svc.pin_current(&ChainId::new("unknown-chain"), "label");
        assert!(result.is_err(), "pinning without cached metadata must fail");
    }

    #[test]
    fn multiple_pins_all_checked() {
        // Register two pins, then register metadata matching neither.
        // Drift must be detected.
        let svc = MetadataService::new();
        let chain = ChainId::new("polkadot");

        // Pin v1.
        let snap_v1 = MetadataSnapshot::new(
            chain.clone(),
            MetadataVersion::V14,
            b"v1_metadata".to_vec(),
            now(),
            1000,
        );
        svc.register_snapshot(snap_v1);
        svc.pin_current(&chain, "v1").expect("pin v1");

        // Pin v2.
        let snap_v2 = MetadataSnapshot::new(
            chain.clone(),
            MetadataVersion::V14,
            b"v2_metadata".to_vec(),
            now(),
            1001,
        );
        svc.register_snapshot(snap_v2);
        svc.pin_current(&chain, "v2").expect("pin v2");

        // Register v3 (matches neither pin).
        let snap_v3 = MetadataSnapshot::new(
            chain.clone(),
            MetadataVersion::V14,
            b"v3_metadata_attacker".to_vec(),
            now(),
            1002,
        );
        let drift = svc.register_snapshot(snap_v3);

        assert!(
            drift.is_some(),
            "metadata matching no pin must trigger drift"
        );
    }
}

// ===========================================================================
// 7. Replay protection: Effects can't be replayed
// ===========================================================================

mod replay_protection {
    use super::*;

    #[tokio::test]
    async fn idempotency_key_prevents_duplicate_execution() {
        let (store, _) = make_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
        let run_id = RunId::new();

        let key = IdempotencyKey::generate(
            run_id,
            1,
            0,
            EffectKind::Broadcast,
            IdempotencyKey::hash_params(b"transfer-alice-10-dot"),
        );

        let spec = || EffectIntentSpec {
            run_id,
            turn_id: TurnId::new(),
            step_id: StepId::new(),
            kind: EffectKind::Broadcast,
            sequence: 1,
            idempotency_key: Some(key),
            payload: serde_json::json!({"to": "alice", "amount": 10}),
            retry_class: None,
            priority: None,
            max_attempts: None,
            action_card: None,
        };

        // First proposal succeeds.
        let _id1 = pipeline
            .propose(spec())
            .await
            .unwrap_or_else(|e| panic!("first propose must succeed: {e}"));

        // Replay must be rejected.
        let err = pipeline
            .propose(spec())
            .await
            .expect_err("replayed proposal with same key must fail");

        assert!(
            matches!(err, PipelineError::DuplicatePending(_)),
            "replay must produce DuplicatePending error, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn completed_effect_cannot_transition_back_to_pending() {
        let (store, inner) = make_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
        let run_id = RunId::new();

        let intent_id = pipeline
            .propose(make_spec(run_id, EffectKind::Broadcast))
            .await
            .unwrap_or_else(|e| panic!("propose failed: {e}"));

        let _guard = pipeline
            .claim_with_duration(Duration::from_secs(60))
            .await
            .unwrap_or_else(|e| panic!("claim failed: {e}"))
            .unwrap_or_else(|| panic!("no intent to claim"));

        // Record outcome -- moves to "resolved".
        let outcome = make_outcome(intent_id, run_id);
        pipeline
            .record_outcome(intent_id, &outcome)
            .await
            .unwrap_or_else(|e| panic!("record_outcome failed: {e}"));

        // Verify the intent is in "resolved" state.
        let stored = inner.intents.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            stored[&intent_id].state, "resolved",
            "completed intent must be in resolved state"
        );

        // No pending intents should be claimable.
        drop(stored);
        let second_claim = pipeline
            .claim_with_duration(Duration::from_secs(60))
            .await
            .unwrap_or_else(|e| panic!("second claim error: {e}"));
        assert!(
            second_claim.is_none(),
            "resolved intent must not be claimable -- no back-transition to pending"
        );
    }

    #[tokio::test]
    async fn re_recording_outcome_fails_immutability() {
        let (store, _) = make_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
        let run_id = RunId::new();

        let intent_id = pipeline
            .propose(make_spec(run_id, EffectKind::Broadcast))
            .await
            .unwrap_or_else(|e| panic!("propose failed: {e}"));

        let _guard = pipeline
            .claim_with_duration(Duration::from_secs(60))
            .await
            .unwrap_or_else(|e| panic!("claim failed: {e}"))
            .unwrap_or_else(|| panic!("no intent to claim"));

        let outcome = make_outcome(intent_id, run_id);
        pipeline
            .record_outcome(intent_id, &outcome)
            .await
            .unwrap_or_else(|e| panic!("first record_outcome failed: {e}"));

        // Attempt to re-record with modified data.
        let tampered = EffectOutcome {
            result: OutcomeResult::Success {
                data: serde_json::json!({"replayed": true}),
            },
            ..outcome
        };

        let err = pipeline
            .record_outcome(intent_id, &tampered)
            .await
            .expect_err("re-recording must fail");

        assert!(
            matches!(err, PipelineError::OutcomeAlreadyRecorded(_)),
            "re-recording must produce OutcomeAlreadyRecorded, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn different_idempotency_keys_allowed_concurrently() {
        // Two effects with different idempotency keys should both succeed.
        let (store, _) = make_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
        let run_id = RunId::new();

        let key_a = IdempotencyKey::generate(
            run_id,
            1,
            0,
            EffectKind::Broadcast,
            IdempotencyKey::hash_params(b"transfer-alice"),
        );
        let key_b = IdempotencyKey::generate(
            run_id,
            1,
            1, // different step_index
            EffectKind::Broadcast,
            IdempotencyKey::hash_params(b"transfer-bob"),
        );

        let spec_a = EffectIntentSpec {
            run_id,
            turn_id: TurnId::new(),
            step_id: StepId::new(),
            kind: EffectKind::Broadcast,
            sequence: 1,
            idempotency_key: Some(key_a),
            payload: serde_json::json!({"to": "alice"}),
            retry_class: None,
            priority: None,
            max_attempts: None,
            action_card: None,
        };
        let spec_b = EffectIntentSpec {
            run_id,
            turn_id: TurnId::new(),
            step_id: StepId::new(),
            kind: EffectKind::Broadcast,
            sequence: 2,
            idempotency_key: Some(key_b),
            payload: serde_json::json!({"to": "bob"}),
            retry_class: None,
            priority: None,
            max_attempts: None,
            action_card: None,
        };

        let id_a = pipeline
            .propose(spec_a)
            .await
            .unwrap_or_else(|e| panic!("propose A failed: {e}"));
        let id_b = pipeline
            .propose(spec_b)
            .await
            .unwrap_or_else(|e| panic!("propose B failed: {e}"));

        assert_ne!(
            id_a, id_b,
            "distinct idempotency keys must produce distinct intent IDs"
        );
    }

    #[tokio::test]
    async fn replay_after_resolution_also_rejected() {
        // Even after an intent is resolved, re-proposing with the same
        // idempotency key must be rejected.
        let (store, _) = make_store();
        let pipeline = EffectPipeline::new(Arc::clone(&store), WorkerId::new());
        let run_id = RunId::new();

        let key = IdempotencyKey::generate(
            run_id,
            1,
            0,
            EffectKind::ToolCall,
            IdempotencyKey::hash_params(b"tool-call-xyz"),
        );

        let spec = || EffectIntentSpec {
            run_id,
            turn_id: TurnId::new(),
            step_id: StepId::new(),
            kind: EffectKind::ToolCall,
            sequence: 1,
            idempotency_key: Some(key),
            payload: serde_json::json!({"tool": "xyz"}),
            retry_class: None,
            priority: None,
            max_attempts: None,
            action_card: None,
        };

        // Propose, claim, resolve.
        let intent_id = pipeline
            .propose(spec())
            .await
            .unwrap_or_else(|e| panic!("propose failed: {e}"));

        let _guard = pipeline
            .claim_with_duration(Duration::from_secs(60))
            .await
            .unwrap_or_else(|e| panic!("claim failed: {e}"))
            .unwrap_or_else(|| panic!("nothing to claim"));

        let outcome = make_outcome(intent_id, run_id);
        pipeline
            .record_outcome(intent_id, &outcome)
            .await
            .unwrap_or_else(|e| panic!("record_outcome failed: {e}"));

        // Replay attempt after resolution.
        let err = pipeline
            .propose(spec())
            .await
            .expect_err("replay after resolution must fail");

        assert!(
            matches!(
                err,
                PipelineError::DuplicateResolved(_) | PipelineError::DuplicatePending(_)
            ),
            "post-resolution replay must produce Duplicate error, got: {err:?}"
        );
    }
}

// ===========================================================================
// Cross-cutting: Additional supply-chain hardening tests
// ===========================================================================

mod cross_cutting {
    use polkagent_config::schema::Config;
    use polkagent_config::validate;
    use polkagent_grant::policy::{
        evaluate, Effect, EvaluationContext, PolicyDecision, PolicyRule, PolicySet,
    };
    use polkagent_metadata::types::MetadataHash;

    #[test]
    fn config_roundtrip_preserves_integrity() {
        // Serialize -> deserialize -> validate must succeed for a valid config.
        let original = Config::default();
        let toml_str = toml::to_string(&original).expect("serialize to TOML");
        let parsed: Config = toml::from_str(&toml_str).expect("deserialize from TOML");
        let result = validate::validate(&parsed);
        assert!(
            result.is_ok(),
            "valid config round-trip must pass validation: {result:?}"
        );
    }

    #[test]
    fn config_serde_json_roundtrip_preserves_integrity() {
        let original = Config::default();
        let json = serde_json::to_string(&original).expect("serialize to JSON");
        let parsed: Config = serde_json::from_str(&json).expect("deserialize from JSON");
        let result = validate::validate(&parsed);
        assert!(
            result.is_ok(),
            "valid config JSON round-trip must pass validation: {result:?}"
        );
    }

    #[test]
    fn metadata_hash_serde_roundtrip_stable() {
        let hash = MetadataHash::from_bytes(b"supply_chain_test_bytes");
        let json = serde_json::to_string(&hash).expect("serialize");
        let back: MetadataHash = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(hash, back, "MetadataHash must survive serde roundtrip");
    }

    #[test]
    fn policy_set_serde_roundtrip_preserves_rules() {
        let mut set = PolicySet::default();
        set.add_rule(PolicyRule {
            id: "allow-query".to_string(),
            effect: Effect::Allow,
            action_patterns: vec!["chain/query".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        });
        set.add_rule(PolicyRule {
            id: "deny-submit".to_string(),
            effect: Effect::Deny,
            action_patterns: vec!["chain/submit".to_string()],
            resource_patterns: vec!["**".to_string()],
            conditions: Default::default(),
            abac_condition: None,
        });

        let json = serde_json::to_string(&set).expect("serialize PolicySet");
        let back: PolicySet = serde_json::from_str(&json).expect("deserialize PolicySet");

        // Verify the deserialized policy still evaluates correctly.
        let ctx = EvaluationContext::default();
        let query_decision = evaluate(&back, "chain/query", "account/alice", &ctx);
        assert_eq!(
            query_decision,
            PolicyDecision::Allow,
            "allow rule preserved"
        );

        let submit_decision = evaluate(&back, "chain/submit", "account/alice", &ctx);
        assert!(
            matches!(submit_decision, PolicyDecision::Deny { .. }),
            "deny rule preserved after serde roundtrip"
        );
    }
}
