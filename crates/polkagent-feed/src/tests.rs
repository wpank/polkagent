//! Integration tests for `polkagent-feed`.
//!
//! Tests are grouped by area:
//!   - Trigger condition evaluation (Always, JsonPath, Threshold, And/Or/Not)
//!   - Cooldown mechanics
//!   - Recipe instantiation
//!   - Cursor mechanics
//!   - Feed processing engine
//!   - FeedStore CRUD (via MemoryStore)
//!   - Idempotent processing
//!   - EventFilter matching
//!   - FeedSource / schedule handling
//!   - Multiple triggers on the same feed

#[cfg(test)]
mod trigger_condition {
    use serde_json::json;

    use crate::trigger::{CompOp, TriggerCondition};

    fn eval(cond: &TriggerCondition, payload: serde_json::Value) -> bool {
        cond.evaluate(&payload)
    }

    // -----------------------------------------------------------------------
    // Always
    // -----------------------------------------------------------------------

    #[test]
    fn always_fires_on_any_payload() {
        let cond = TriggerCondition::Always;
        assert!(eval(&cond, json!({})));
        assert!(eval(&cond, json!(null)));
        assert!(eval(&cond, json!(42)));
    }

    // -----------------------------------------------------------------------
    // JsonPath
    // -----------------------------------------------------------------------

    #[test]
    fn json_path_matches_string_value() {
        let cond = TriggerCondition::JsonPath {
            path: "/status".to_string(),
            expected: json!("active"),
        };
        assert!(eval(&cond, json!({ "status": "active" })));
    }

    #[test]
    fn json_path_does_not_match_different_value() {
        let cond = TriggerCondition::JsonPath {
            path: "/status".to_string(),
            expected: json!("active"),
        };
        assert!(!eval(&cond, json!({ "status": "paused" })));
    }

    #[test]
    fn json_path_missing_field_returns_false() {
        let cond = TriggerCondition::JsonPath {
            path: "/missing".to_string(),
            expected: json!("value"),
        };
        assert!(!eval(&cond, json!({})));
    }

    #[test]
    fn json_path_matches_nested_field() {
        let cond = TriggerCondition::JsonPath {
            path: "/data/0/value".to_string(),
            expected: json!(99),
        };
        assert!(eval(&cond, json!({ "data": [{ "value": 99 }] })));
    }

    #[test]
    fn json_path_bare_field_name_works() {
        // Without leading slash — convenience shorthand.
        let cond = TriggerCondition::JsonPath {
            path: "count".to_string(),
            expected: json!(3),
        };
        assert!(eval(&cond, json!({ "count": 3 })));
        assert!(!eval(&cond, json!({ "count": 4 })));
    }

    #[test]
    fn json_path_matches_boolean() {
        let cond = TriggerCondition::JsonPath {
            path: "/enabled".to_string(),
            expected: json!(true),
        };
        assert!(eval(&cond, json!({ "enabled": true })));
        assert!(!eval(&cond, json!({ "enabled": false })));
    }

    // -----------------------------------------------------------------------
    // Threshold
    // -----------------------------------------------------------------------

    #[test]
    fn threshold_gt_fires_when_above() {
        let cond = TriggerCondition::Threshold {
            field: "/price".to_string(),
            op: CompOp::Gt,
            value: 100.0,
        };
        assert!(eval(&cond, json!({ "price": 101.0 })));
        assert!(!eval(&cond, json!({ "price": 100.0 })));
        assert!(!eval(&cond, json!({ "price": 99.0 })));
    }

    #[test]
    fn threshold_gte_fires_when_equal_or_above() {
        let cond = TriggerCondition::Threshold {
            field: "/price".to_string(),
            op: CompOp::Gte,
            value: 100.0,
        };
        assert!(eval(&cond, json!({ "price": 100.0 })));
        assert!(eval(&cond, json!({ "price": 200.0 })));
        assert!(!eval(&cond, json!({ "price": 99.9 })));
    }

    #[test]
    fn threshold_lt_fires_when_below() {
        let cond = TriggerCondition::Threshold {
            field: "/temp".to_string(),
            op: CompOp::Lt,
            value: 0.0,
        };
        assert!(eval(&cond, json!({ "temp": -1.0 })));
        assert!(!eval(&cond, json!({ "temp": 0.0 })));
    }

    #[test]
    fn threshold_lte_fires_when_equal_or_below() {
        let cond = TriggerCondition::Threshold {
            field: "/temp".to_string(),
            op: CompOp::Lte,
            value: 0.0,
        };
        assert!(eval(&cond, json!({ "temp": 0.0 })));
        assert!(eval(&cond, json!({ "temp": -5.0 })));
        assert!(!eval(&cond, json!({ "temp": 0.1 })));
    }

    #[test]
    fn threshold_eq_fires_when_equal() {
        let cond = TriggerCondition::Threshold {
            field: "/count".to_string(),
            op: CompOp::Eq,
            value: 42.0,
        };
        assert!(eval(&cond, json!({ "count": 42 })));
        assert!(!eval(&cond, json!({ "count": 43 })));
    }

    #[test]
    fn threshold_neq_fires_when_different() {
        let cond = TriggerCondition::Threshold {
            field: "/count".to_string(),
            op: CompOp::Neq,
            value: 42.0,
        };
        assert!(eval(&cond, json!({ "count": 41 })));
        assert!(!eval(&cond, json!({ "count": 42 })));
    }

    #[test]
    fn threshold_missing_field_returns_false() {
        let cond = TriggerCondition::Threshold {
            field: "/missing".to_string(),
            op: CompOp::Gt,
            value: 0.0,
        };
        assert!(!eval(&cond, json!({})));
    }

    #[test]
    fn threshold_non_numeric_field_returns_false() {
        let cond = TriggerCondition::Threshold {
            field: "/value".to_string(),
            op: CompOp::Gt,
            value: 0.0,
        };
        assert!(!eval(&cond, json!({ "value": "hello" })));
    }

    // -----------------------------------------------------------------------
    // And / Or / Not composition
    // -----------------------------------------------------------------------

    #[test]
    fn and_requires_all_conditions() {
        let cond = TriggerCondition::And(vec![
            TriggerCondition::JsonPath {
                path: "/a".to_string(),
                expected: json!(1),
            },
            TriggerCondition::JsonPath {
                path: "/b".to_string(),
                expected: json!(2),
            },
        ]);
        assert!(eval(&cond, json!({ "a": 1, "b": 2 })));
        assert!(!eval(&cond, json!({ "a": 1, "b": 9 })));
        assert!(!eval(&cond, json!({ "a": 9, "b": 2 })));
    }

    #[test]
    fn or_requires_any_condition() {
        let cond = TriggerCondition::Or(vec![
            TriggerCondition::JsonPath {
                path: "/a".to_string(),
                expected: json!("yes"),
            },
            TriggerCondition::JsonPath {
                path: "/b".to_string(),
                expected: json!("yes"),
            },
        ]);
        assert!(eval(&cond, json!({ "a": "yes", "b": "no" })));
        assert!(eval(&cond, json!({ "a": "no", "b": "yes" })));
        assert!(!eval(&cond, json!({ "a": "no", "b": "no" })));
    }

    #[test]
    fn not_inverts_condition() {
        let cond =
            TriggerCondition::Not(Box::new(TriggerCondition::JsonPath {
                path: "/status".to_string(),
                expected: json!("ok"),
            }));
        assert!(eval(&cond, json!({ "status": "error" })));
        assert!(!eval(&cond, json!({ "status": "ok" })));
    }

    #[test]
    fn deeply_nested_and_or_not() {
        // (a == 1 AND NOT (b == 2)) OR (c == 3)
        let cond = TriggerCondition::Or(vec![
            TriggerCondition::And(vec![
                TriggerCondition::JsonPath {
                    path: "/a".to_string(),
                    expected: json!(1),
                },
                TriggerCondition::Not(Box::new(TriggerCondition::JsonPath {
                    path: "/b".to_string(),
                    expected: json!(2),
                })),
            ]),
            TriggerCondition::JsonPath {
                path: "/c".to_string(),
                expected: json!(3),
            },
        ]);

        // a==1, b!=2 → And branch fires
        assert!(eval(&cond, json!({ "a": 1, "b": 99, "c": 0 })));
        // c==3 → Or branch fires
        assert!(eval(&cond, json!({ "a": 0, "b": 99, "c": 3 })));
        // a==1, b==2, c!=3 → neither
        assert!(!eval(&cond, json!({ "a": 1, "b": 2, "c": 0 })));
    }

    #[test]
    fn empty_and_is_vacuously_true() {
        let cond = TriggerCondition::And(vec![]);
        assert!(eval(&cond, json!({})));
    }

    #[test]
    fn empty_or_is_vacuously_false() {
        let cond = TriggerCondition::Or(vec![]);
        assert!(!eval(&cond, json!({})));
    }
}

// ---------------------------------------------------------------------------
// Cooldown tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod cooldown {
    use chrono::{Duration, Utc};
    use serde_json::json;

    use crate::trigger::{
        Trigger, TriggerAction, TriggerCondition, TriggerResult, evaluate_trigger,
    };
    use crate::types::{FeedId, FeedItem};

    fn make_item() -> FeedItem {
        FeedItem::new(FeedId::new(), json!({ "x": 1 }))
    }

    fn make_trigger(cooldown_secs: Option<u64>, last_fired_at: Option<chrono::DateTime<Utc>>) -> Trigger {
        let mut t = Trigger::new(
            "t",
            FeedId::new(),
            TriggerCondition::Always,
            TriggerAction::Notify {
                channel: "c".to_string(),
                message: "m".to_string(),
            },
        );
        t.cooldown_secs = cooldown_secs;
        t.last_fired_at = last_fired_at;
        t
    }

    #[test]
    fn no_cooldown_always_fires() {
        let trigger = make_trigger(None, None);
        let item = make_item();
        assert!(matches!(evaluate_trigger(&trigger, &item), TriggerResult::Fire(_)));
    }

    #[test]
    fn cooldown_active_within_window() {
        // Last fired 5 seconds ago; cooldown is 60 seconds.
        let last = Utc::now() - Duration::seconds(5);
        let trigger = make_trigger(Some(60), Some(last));
        let item = make_item();
        assert!(matches!(
            evaluate_trigger(&trigger, &item),
            TriggerResult::CooldownActive
        ));
    }

    #[test]
    fn cooldown_expired_fires_again() {
        // Last fired 120 seconds ago; cooldown is 60 seconds.
        let last = Utc::now() - Duration::seconds(120);
        let trigger = make_trigger(Some(60), Some(last));
        let item = make_item();
        assert!(matches!(evaluate_trigger(&trigger, &item), TriggerResult::Fire(_)));
    }

    #[test]
    fn cooldown_with_no_last_fired_fires() {
        // Cooldown set but never fired before.
        let trigger = make_trigger(Some(60), None);
        let item = make_item();
        assert!(matches!(evaluate_trigger(&trigger, &item), TriggerResult::Fire(_)));
    }

    #[test]
    fn disabled_trigger_always_skips() {
        let mut trigger = make_trigger(None, None);
        trigger.enabled = false;
        let item = make_item();
        assert!(matches!(evaluate_trigger(&trigger, &item), TriggerResult::Skip(_)));
    }
}

// ---------------------------------------------------------------------------
// Recipe tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod recipe {
    use std::collections::HashMap;

    use serde_json::json;

    use crate::error::FeedError;
    use crate::recipe::{
        ParamType, Recipe, RecipeId, RecipeParameter, instantiate_recipe,
    };
    use crate::trigger::{TriggerAction, TriggerCondition};
    use crate::types::FeedSource;

    fn simple_recipe(params: Vec<RecipeParameter>) -> Recipe {
        Recipe {
            id: RecipeId::new(),
            name: "Test Recipe".to_string(),
            description: "For testing".to_string(),
            version: "1.0.0".to_string(),
            source: FeedSource::Schedule {
                cron: "{{cron}}".to_string(),
            },
            trigger: TriggerCondition::Always,
            action: TriggerAction::Notify {
                channel: "{{channel}}".to_string(),
                message: "{{message}}".to_string(),
            },
            parameters: params,
        }
    }

    #[test]
    fn instantiate_with_all_params_provided() {
        let recipe = simple_recipe(vec![
            RecipeParameter {
                name: "cron".to_string(),
                description: "Schedule".to_string(),
                param_type: ParamType::String,
                default: None,
                required: true,
            },
            RecipeParameter {
                name: "channel".to_string(),
                description: "Notification channel".to_string(),
                param_type: ParamType::String,
                default: None,
                required: true,
            },
            RecipeParameter {
                name: "message".to_string(),
                description: "Message".to_string(),
                param_type: ParamType::String,
                default: None,
                required: true,
            },
        ]);

        let mut params = HashMap::new();
        params.insert("cron".to_string(), json!("0 * * * *"));
        params.insert("channel".to_string(), json!("slack:#ops"));
        params.insert("message".to_string(), json!("hourly ping"));

        let result = instantiate_recipe(&recipe, &params);
        assert!(result.is_ok(), "expected Ok, got: {:?}", result);

        let (feed, trigger) = result.expect("ok");
        assert!(matches!(feed.source, crate::types::FeedSource::Schedule { .. }));
        if let crate::types::FeedSource::Schedule { cron } = &feed.source {
            assert_eq!(cron, "0 * * * *");
        }
        assert!(matches!(
            trigger.action,
            TriggerAction::Notify { .. }
        ));
        if let TriggerAction::Notify { channel, message } = &trigger.action {
            assert_eq!(channel, "slack:#ops");
            assert_eq!(message, "hourly ping");
        }
    }

    #[test]
    fn instantiate_missing_required_param_returns_error() {
        let recipe = simple_recipe(vec![RecipeParameter {
            name: "cron".to_string(),
            description: "Schedule".to_string(),
            param_type: ParamType::String,
            default: None,
            required: true,
        }]);

        // cron is required but not supplied.
        let params = HashMap::new();
        let result = instantiate_recipe(&recipe, &params);
        assert!(matches!(result, Err(FeedError::InvalidRecipeParams(_))));
    }

    #[test]
    fn instantiate_uses_default_when_param_absent() {
        let recipe = simple_recipe(vec![
            RecipeParameter {
                name: "cron".to_string(),
                description: "Schedule".to_string(),
                param_type: ParamType::String,
                default: Some("*/5 * * * *".to_string()),
                required: false,
            },
            RecipeParameter {
                name: "channel".to_string(),
                description: "Channel".to_string(),
                param_type: ParamType::String,
                default: Some("default".to_string()),
                required: false,
            },
            RecipeParameter {
                name: "message".to_string(),
                description: "Message".to_string(),
                param_type: ParamType::String,
                default: Some("default message".to_string()),
                required: false,
            },
        ]);

        let params = HashMap::new();
        let (feed, _trigger) = instantiate_recipe(&recipe, &params).expect("ok");
        if let crate::types::FeedSource::Schedule { cron } = &feed.source {
            assert_eq!(cron, "*/5 * * * *");
        } else {
            panic!("expected Schedule source");
        }
    }

    #[test]
    fn instantiate_wrong_type_returns_error() {
        let recipe = simple_recipe(vec![RecipeParameter {
            name: "cron".to_string(),
            description: "Schedule".to_string(),
            param_type: ParamType::Number, // expects a number
            default: None,
            required: true,
        }]);

        let mut params = HashMap::new();
        params.insert("cron".to_string(), json!("not-a-number")); // wrong type

        let result = instantiate_recipe(&recipe, &params);
        assert!(matches!(result, Err(FeedError::InvalidRecipeParams(_))));
    }

    #[test]
    fn instantiate_optional_param_absent_not_required() {
        let recipe = Recipe {
            id: RecipeId::new(),
            name: "No Params".to_string(),
            description: "Recipe with no required params".to_string(),
            version: "1.0.0".to_string(),
            source: FeedSource::Webhook {
                path: "/hook".to_string(),
                secret_hash: None,
            },
            trigger: TriggerCondition::Always,
            action: TriggerAction::Notify {
                channel: "ops".to_string(),
                message: "event".to_string(),
            },
            parameters: vec![RecipeParameter {
                name: "secret".to_string(),
                description: "Optional secret".to_string(),
                param_type: ParamType::String,
                default: None,
                required: false, // not required
            }],
        };

        let params = HashMap::new();
        assert!(instantiate_recipe(&recipe, &params).is_ok());
    }

    #[test]
    fn instantiate_recipe_trigger_linked_to_feed() {
        let recipe = Recipe {
            id: RecipeId::new(),
            name: "Link Test".to_string(),
            description: "Test".to_string(),
            version: "1.0.0".to_string(),
            source: FeedSource::Webhook {
                path: "/wh".to_string(),
                secret_hash: None,
            },
            trigger: TriggerCondition::Always,
            action: TriggerAction::Notify {
                channel: "c".to_string(),
                message: "m".to_string(),
            },
            parameters: vec![],
        };

        let (feed, trigger) = instantiate_recipe(&recipe, &HashMap::new()).expect("ok");
        assert_eq!(trigger.feed_id, feed.id);
    }

    #[test]
    fn param_type_validate_string() {
        assert!(ParamType::String.validate(&json!("hello")));
        assert!(!ParamType::String.validate(&json!(42)));
    }

    #[test]
    fn param_type_validate_number() {
        assert!(ParamType::Number.validate(&json!(3.14)));
        assert!(!ParamType::Number.validate(&json!("pi")));
    }

    #[test]
    fn param_type_validate_bool() {
        assert!(ParamType::Bool.validate(&json!(true)));
        assert!(!ParamType::Bool.validate(&json!("true")));
    }

    #[test]
    fn param_type_validate_json_accepts_anything() {
        assert!(ParamType::Json.validate(&json!({"x": 1})));
        assert!(ParamType::Json.validate(&json!(null)));
        assert!(ParamType::Json.validate(&json!([1, 2, 3])));
    }
}

// ---------------------------------------------------------------------------
// Cursor tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod cursor {
    use crate::types::Cursor;

    #[test]
    fn new_cursor_starts_at_zero_items() {
        let c = Cursor::new("start");
        assert_eq!(c.position, "start");
        assert_eq!(c.items_processed, 0);
    }

    #[test]
    fn advance_increments_items_processed() {
        let c = Cursor::new("0");
        let c2 = c.advance("1");
        assert_eq!(c2.position, "1");
        assert_eq!(c2.items_processed, 1);
    }

    #[test]
    fn advance_multiple_times() {
        let c = Cursor::new("a").advance("b").advance("c").advance("d");
        assert_eq!(c.position, "d");
        assert_eq!(c.items_processed, 3);
    }

    #[test]
    fn advance_updates_last_processed_at() {
        let before = chrono::Utc::now();
        let c = Cursor::new("x").advance("y");
        assert!(c.last_processed_at >= before);
    }
}

// ---------------------------------------------------------------------------
// FeedProcessor tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod processor {
    use std::sync::Arc;

    use serde_json::json;

    use crate::memory_store::MemoryStore;
    use crate::processor::FeedProcessor;
    use crate::store::FeedStore;
    use crate::trigger::{Trigger, TriggerAction, TriggerCondition, TriggerResult};
    use crate::types::{Feed, FeedItem, FeedSource};

    fn make_store() -> Arc<dyn FeedStore> {
        Arc::new(MemoryStore::new())
    }

    fn make_feed() -> Feed {
        Feed::new(
            "test-feed",
            FeedSource::Webhook {
                path: "/test".to_string(),
                secret_hash: None,
            },
            polkagent_core::AgentId::new(),
        )
    }

    #[tokio::test]
    async fn process_item_with_no_triggers_returns_empty() {
        let store = make_store();
        let feed = store.create_feed(make_feed()).await.expect("create feed");
        let processor = FeedProcessor::new(Arc::clone(&store));

        let item = FeedItem::new(feed.id, json!({ "x": 1 }));
        let results = processor.process_item(&feed, &item).await.expect("process");
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn process_item_evaluates_all_triggers() {
        let store = make_store();
        let feed = store.create_feed(make_feed()).await.expect("create feed");

        // Register two triggers on the feed.
        let t1 = Trigger::new(
            "always",
            feed.id,
            TriggerCondition::Always,
            TriggerAction::Notify {
                channel: "c".to_string(),
                message: "m".to_string(),
            },
        );
        let t2 = Trigger::new(
            "never",
            feed.id,
            TriggerCondition::JsonPath {
                path: "/x".to_string(),
                expected: json!(999), // won't match
            },
            TriggerAction::Notify {
                channel: "c".to_string(),
                message: "m".to_string(),
            },
        );
        store.create_trigger(t1).await.expect("t1");
        store.create_trigger(t2).await.expect("t2");

        let processor = FeedProcessor::new(Arc::clone(&store));
        let item = FeedItem::new(feed.id, json!({ "x": 1 }));
        let results = processor.process_item(&feed, &item).await.expect("process");

        assert_eq!(results.len(), 2);
        let fires: Vec<_> = results.iter().filter(|r| matches!(r, TriggerResult::Fire(_))).collect();
        let skips: Vec<_> = results.iter().filter(|r| matches!(r, TriggerResult::Skip(_))).collect();
        assert_eq!(fires.len(), 1);
        assert_eq!(skips.len(), 1);
    }

    #[tokio::test]
    async fn advance_cursor_updates_position() {
        let store = make_store();
        let feed = store.create_feed(make_feed()).await.expect("create feed");
        let processor = FeedProcessor::new(Arc::clone(&store));

        processor
            .advance_cursor(&feed.id, "block-100")
            .await
            .expect("advance");

        let updated = store.get_feed(&feed.id).await.expect("get feed");
        assert_eq!(updated.cursor.position, "block-100");
        assert_eq!(updated.cursor.items_processed, 1);
    }

    #[tokio::test]
    async fn process_batch_marks_items_processed() {
        let store = make_store();
        let feed = store.create_feed(make_feed()).await.expect("create feed");

        let item = FeedItem::new(feed.id, json!({ "v": 1 }));
        store.enqueue_item(item).await.expect("enqueue");

        let processor = FeedProcessor::new(Arc::clone(&store));
        let batch = processor.process_batch(&feed, 10).await.expect("batch");
        assert_eq!(batch.len(), 1);

        // After processing, the item should be flagged as processed.
        let remaining = store.dequeue_items(&feed.id, 10).await.expect("dequeue");
        assert!(remaining.is_empty(), "item should be marked processed");
    }

    #[tokio::test]
    async fn idempotent_processing_same_result() {
        // Processing the same item twice with the same trigger configuration
        // should produce identical results.
        let store = make_store();
        let feed = store.create_feed(make_feed()).await.expect("create feed");

        let trigger = Trigger::new(
            "always",
            feed.id,
            TriggerCondition::Always,
            TriggerAction::Notify {
                channel: "c".to_string(),
                message: "m".to_string(),
            },
        );
        store.create_trigger(trigger).await.expect("trigger");

        let processor = FeedProcessor::new(Arc::clone(&store));
        let item = FeedItem::new(feed.id, json!({ "k": "v" }));

        let r1 = processor.process_item(&feed, &item).await.expect("first");
        let r2 = processor.process_item(&feed, &item).await.expect("second");

        assert_eq!(r1.len(), r2.len());
        // Both should be Fire results.
        assert!(matches!(r1[0], TriggerResult::Fire(_)));
        assert!(matches!(r2[0], TriggerResult::Fire(_)));
    }

    #[tokio::test]
    async fn multiple_triggers_same_feed() {
        let store = make_store();
        let feed = store.create_feed(make_feed()).await.expect("create feed");

        for i in 0..5_u32 {
            let t = Trigger::new(
                format!("trigger-{i}"),
                feed.id,
                TriggerCondition::Always,
                TriggerAction::Notify {
                    channel: format!("ch-{i}"),
                    message: "msg".to_string(),
                },
            );
            store.create_trigger(t).await.expect("create");
        }

        let processor = FeedProcessor::new(Arc::clone(&store));
        let item = FeedItem::new(feed.id, json!({}));
        let results = processor.process_item(&feed, &item).await.expect("process");
        assert_eq!(results.len(), 5);
        assert!(results.iter().all(|r| matches!(r, TriggerResult::Fire(_))));
    }
}

// ---------------------------------------------------------------------------
// MemoryStore CRUD tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod store_crud {
    use serde_json::json;

    use crate::error::FeedError;
    use crate::memory_store::MemoryStore;
    use crate::recipe::{Recipe, RecipeId};
    use crate::store::FeedStore;
    use crate::trigger::{Trigger, TriggerAction, TriggerCondition};
    use crate::types::{Feed, FeedItem, FeedSource};

    fn make_feed() -> Feed {
        Feed::new(
            "store-test",
            FeedSource::Webhook {
                path: "/x".to_string(),
                secret_hash: None,
            },
            polkagent_core::AgentId::new(),
        )
    }

    #[tokio::test]
    async fn create_and_get_feed() {
        let store = MemoryStore::new();
        let feed = make_feed();
        let id = feed.id;
        store.create_feed(feed).await.expect("create");
        let got = store.get_feed(&id).await.expect("get");
        assert_eq!(got.id, id);
    }

    #[tokio::test]
    async fn get_feed_not_found_error() {
        let store = MemoryStore::new();
        let id = crate::types::FeedId::new();
        let result = store.get_feed(&id).await;
        assert!(matches!(result, Err(FeedError::NotFound(_))));
    }

    #[tokio::test]
    async fn list_feeds_returns_all() {
        let store = MemoryStore::new();
        store.create_feed(make_feed()).await.expect("1");
        store.create_feed(make_feed()).await.expect("2");
        store.create_feed(make_feed()).await.expect("3");

        let feeds = store.list_feeds().await.expect("list");
        assert_eq!(feeds.len(), 3);
    }

    #[tokio::test]
    async fn create_and_get_trigger() {
        let store = MemoryStore::new();
        let feed = store.create_feed(make_feed()).await.expect("feed");
        let t = Trigger::new(
            "t",
            feed.id,
            TriggerCondition::Always,
            TriggerAction::Notify {
                channel: "c".to_string(),
                message: "m".to_string(),
            },
        );
        let tid = t.id;
        store.create_trigger(t).await.expect("create trigger");
        let got = store.get_trigger(&tid).await.expect("get trigger");
        assert_eq!(got.id, tid);
    }

    #[tokio::test]
    async fn get_trigger_not_found() {
        let store = MemoryStore::new();
        let id = crate::trigger::TriggerId::new();
        assert!(matches!(store.get_trigger(&id).await, Err(FeedError::NotFound(_))));
    }

    #[tokio::test]
    async fn list_triggers_filters_by_feed() {
        let store = MemoryStore::new();
        let feed_a = store.create_feed(make_feed()).await.expect("a");
        let feed_b = store.create_feed(make_feed()).await.expect("b");

        let ta = Trigger::new(
            "a",
            feed_a.id,
            TriggerCondition::Always,
            TriggerAction::Notify {
                channel: "c".to_string(),
                message: "m".to_string(),
            },
        );
        let tb = Trigger::new(
            "b",
            feed_b.id,
            TriggerCondition::Always,
            TriggerAction::Notify {
                channel: "c".to_string(),
                message: "m".to_string(),
            },
        );
        store.create_trigger(ta).await.expect("ta");
        store.create_trigger(tb).await.expect("tb");

        let for_a = store.list_triggers(&feed_a.id).await.expect("list a");
        assert_eq!(for_a.len(), 1);
        assert_eq!(for_a[0].feed_id, feed_a.id);
    }

    #[tokio::test]
    async fn update_trigger_persists_last_fired_at() {
        let store = MemoryStore::new();
        let feed = store.create_feed(make_feed()).await.expect("feed");
        let mut t = Trigger::new(
            "t",
            feed.id,
            TriggerCondition::Always,
            TriggerAction::Notify {
                channel: "c".to_string(),
                message: "m".to_string(),
            },
        );
        store.create_trigger(t.clone()).await.expect("create");

        let now = chrono::Utc::now();
        t.last_fired_at = Some(now);
        store.update_trigger(t.clone()).await.expect("update");

        let got = store.get_trigger(&t.id).await.expect("get");
        assert_eq!(got.last_fired_at, Some(now));
    }

    #[tokio::test]
    async fn create_and_get_recipe() {
        let store = MemoryStore::new();
        let recipe = Recipe {
            id: RecipeId::new(),
            name: "R".to_string(),
            description: "D".to_string(),
            version: "1.0.0".to_string(),
            source: FeedSource::Schedule {
                cron: "0 * * * *".to_string(),
            },
            trigger: TriggerCondition::Always,
            action: TriggerAction::Notify {
                channel: "c".to_string(),
                message: "m".to_string(),
            },
            parameters: vec![],
        };
        let rid = recipe.id;
        store.create_recipe(recipe).await.expect("create");
        let got = store.get_recipe(&rid).await.expect("get");
        assert_eq!(got.id, rid);
    }

    #[tokio::test]
    async fn get_recipe_not_found() {
        let store = MemoryStore::new();
        let id = RecipeId::new();
        assert!(matches!(store.get_recipe(&id).await, Err(FeedError::NotFound(_))));
    }

    #[tokio::test]
    async fn list_recipes_returns_all() {
        let store = MemoryStore::new();
        for _ in 0..3 {
            let r = Recipe {
                id: RecipeId::new(),
                name: "R".to_string(),
                description: "D".to_string(),
                version: "1.0.0".to_string(),
                source: FeedSource::Schedule {
                    cron: "0 * * * *".to_string(),
                },
                trigger: TriggerCondition::Always,
                action: TriggerAction::Notify {
                    channel: "c".to_string(),
                    message: "m".to_string(),
                },
                parameters: vec![],
            };
            store.create_recipe(r).await.expect("create");
        }
        let recipes = store.list_recipes().await.expect("list");
        assert_eq!(recipes.len(), 3);
    }

    #[tokio::test]
    async fn enqueue_and_dequeue_items() {
        let store = MemoryStore::new();
        let feed = store.create_feed(make_feed()).await.expect("feed");

        let item = FeedItem::new(feed.id, json!({ "n": 1 }));
        store.enqueue_item(item.clone()).await.expect("enqueue");

        let items = store.dequeue_items(&feed.id, 10).await.expect("dequeue");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, item.id);
    }

    #[tokio::test]
    async fn mark_processed_hides_from_dequeue() {
        let store = MemoryStore::new();
        let feed = store.create_feed(make_feed()).await.expect("feed");

        let item = FeedItem::new(feed.id, json!({}));
        store.enqueue_item(item.clone()).await.expect("enqueue");
        store.mark_processed(item.id).await.expect("mark");

        let items = store.dequeue_items(&feed.id, 10).await.expect("dequeue");
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn mark_processed_is_idempotent() {
        let store = MemoryStore::new();
        let feed = store.create_feed(make_feed()).await.expect("feed");
        let item = FeedItem::new(feed.id, json!({}));
        store.enqueue_item(item.clone()).await.expect("enqueue");

        // Mark twice — should not error.
        store.mark_processed(item.id).await.expect("first");
        store.mark_processed(item.id).await.expect("second");
    }

    #[tokio::test]
    async fn dequeue_respects_limit() {
        let store = MemoryStore::new();
        let feed = store.create_feed(make_feed()).await.expect("feed");

        for i in 0..5_u32 {
            let item = FeedItem::new(feed.id, json!({ "i": i }));
            store.enqueue_item(item).await.expect("enqueue");
        }

        let items = store.dequeue_items(&feed.id, 3).await.expect("dequeue");
        assert_eq!(items.len(), 3);
    }
}

// ---------------------------------------------------------------------------
// EventFilter tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod event_filter {
    use crate::types::EventFilter;

    #[test]
    fn allow_all_matches_any_kind_and_agent() {
        let f = EventFilter::allow_all();
        assert!(f.matches("anything", "any-agent"));
        assert!(f.matches("", ""));
    }

    #[test]
    fn filter_on_kind_accepts_matching_kind() {
        let f = EventFilter {
            event_kinds: vec!["run.started".to_string()],
            agent_ids: vec![],
        };
        assert!(f.matches("run.started", "any-agent"));
        assert!(!f.matches("run.completed", "any-agent"));
    }

    #[test]
    fn filter_on_agent_accepts_matching_agent() {
        let f = EventFilter {
            event_kinds: vec![],
            agent_ids: vec!["agent-42".to_string()],
        };
        assert!(f.matches("anything", "agent-42"));
        assert!(!f.matches("anything", "agent-99"));
    }

    #[test]
    fn filter_on_both_requires_both_match() {
        let f = EventFilter {
            event_kinds: vec!["k".to_string()],
            agent_ids: vec!["a".to_string()],
        };
        assert!(f.matches("k", "a"));
        assert!(!f.matches("k", "b"));
        assert!(!f.matches("x", "a"));
    }

    #[test]
    fn multiple_kinds_accepted() {
        let f = EventFilter {
            event_kinds: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            agent_ids: vec![],
        };
        assert!(f.matches("a", "x"));
        assert!(f.matches("b", "x"));
        assert!(f.matches("c", "x"));
        assert!(!f.matches("d", "x"));
    }
}

// ---------------------------------------------------------------------------
// FeedSource / schedule tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod feed_source {
    use crate::types::{EventFilter, FeedSource};

    #[test]
    fn schedule_source_stores_cron() {
        let s = FeedSource::Schedule {
            cron: "0 0 * * *".to_string(),
        };
        if let FeedSource::Schedule { cron } = s {
            assert_eq!(cron, "0 0 * * *");
        } else {
            panic!("expected Schedule");
        }
    }

    #[test]
    fn webhook_source_stores_path_and_secret() {
        let s = FeedSource::Webhook {
            path: "/hooks/my-app".to_string(),
            secret_hash: Some("abc123".to_string()),
        };
        if let FeedSource::Webhook { path, secret_hash } = s {
            assert_eq!(path, "/hooks/my-app");
            assert_eq!(secret_hash, Some("abc123".to_string()));
        } else {
            panic!("expected Webhook");
        }
    }

    #[test]
    fn webhook_source_optional_secret_is_none() {
        let s = FeedSource::Webhook {
            path: "/hooks/open".to_string(),
            secret_hash: None,
        };
        if let FeedSource::Webhook { secret_hash, .. } = s {
            assert!(secret_hash.is_none());
        }
    }

    #[test]
    fn event_bus_source_stores_filter() {
        let f = EventFilter {
            event_kinds: vec!["run.started".to_string()],
            agent_ids: vec!["a1".to_string()],
        };
        let s = FeedSource::EventBus { filter: f.clone() };
        if let FeedSource::EventBus { filter } = s {
            assert_eq!(filter, f);
        } else {
            panic!("expected EventBus");
        }
    }

    #[test]
    fn chain_state_source_stores_query_and_interval() {
        let s = FeedSource::ChainState {
            query: "SELECT balance FROM accounts".to_string(),
            interval_secs: 30,
        };
        if let FeedSource::ChainState {
            query,
            interval_secs,
        } = s
        {
            assert_eq!(query, "SELECT balance FROM accounts");
            assert_eq!(interval_secs, 30);
        } else {
            panic!("expected ChainState");
        }
    }

    #[test]
    fn feed_source_serde_round_trip_schedule() {
        let s = FeedSource::Schedule {
            cron: "*/10 * * * *".to_string(),
        };
        let json = serde_json::to_string(&s).expect("serialize");
        let back: FeedSource = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
    }

    #[test]
    fn feed_source_serde_round_trip_webhook() {
        let s = FeedSource::Webhook {
            path: "/test".to_string(),
            secret_hash: None,
        };
        let json = serde_json::to_string(&s).expect("serialize");
        let back: FeedSource = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
    }
}
