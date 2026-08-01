//! Integration tests for the feed processing subsystem.
//!
//! Exercises feed creation, item enqueueing, trigger condition evaluation,
//! recipe instantiation, cooldown enforcement, and cursor-backed at-least-once
//! delivery — all wired through the `polkagent-feed` crate boundary.

use std::collections::HashMap;
use std::sync::Arc;

use polkagent_core::AgentId;
use polkagent_feed::{
    CompOp, EventFilter, Feed, FeedId, FeedItem, FeedProcessor, FeedSource, FeedStatus,
    FeedStore, MemoryStore, ParamType, Recipe, RecipeId, RecipeParameter, Trigger, TriggerAction,
    TriggerCondition, TriggerResult, evaluate_trigger, instantiate_recipe,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_store() -> Arc<MemoryStore> {
    Arc::new(MemoryStore::new())
}

fn make_processor(store: Arc<MemoryStore>) -> FeedProcessor {
    FeedProcessor::new(store as Arc<dyn polkagent_feed::FeedStore>)
}

fn make_feed(name: &str) -> Feed {
    Feed::new(
        name,
        FeedSource::EventBus {
            filter: EventFilter::allow_all(),
        },
        AgentId::new(),
    )
}

fn make_item(feed_id: FeedId, payload: serde_json::Value) -> FeedItem {
    FeedItem::new(feed_id, payload)
}

fn always_trigger(feed_id: FeedId) -> Trigger {
    Trigger::new(
        "always",
        feed_id,
        TriggerCondition::Always,
        TriggerAction::PublishEvent {
            kind: "test.event".to_string(),
            payload: serde_json::json!({}),
        },
    )
}

// ---------------------------------------------------------------------------
// IT-FEED-01: Create feed → enqueue items → process with triggers → advance cursor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_feed_and_enqueue_items() {
    let store = make_store();
    let feed = make_feed("test-feed");
    let feed_id = feed.id;

    store.create_feed(feed).await.expect("create feed");

    let item1 = make_item(feed_id, serde_json::json!({"event": "a"}));
    let item2 = make_item(feed_id, serde_json::json!({"event": "b"}));
    let item3 = make_item(feed_id, serde_json::json!({"event": "c"}));

    store.enqueue_item(item1).await.expect("enqueue 1");
    store.enqueue_item(item2).await.expect("enqueue 2");
    store.enqueue_item(item3).await.expect("enqueue 3");

    let items = store.dequeue_items(&feed_id, 10).await.expect("dequeue");
    assert_eq!(items.len(), 3);
}

#[tokio::test]
async fn dequeue_respects_batch_size_limit() {
    let store = make_store();
    let feed = make_feed("batch-feed");
    let feed_id = feed.id;
    store.create_feed(feed).await.expect("create");

    for i in 0..5 {
        store
            .enqueue_item(make_item(feed_id, serde_json::json!({"i": i})))
            .await
            .expect("enqueue");
    }

    let batch = store.dequeue_items(&feed_id, 3).await.expect("dequeue");
    assert_eq!(batch.len(), 3);
}

#[tokio::test]
async fn process_batch_evaluates_always_trigger() {
    let store = make_store();
    let feed = make_feed("always-feed");
    let feed_id = feed.id;
    store.create_feed(feed.clone()).await.expect("create");

    let trigger = always_trigger(feed_id);
    store.create_trigger(trigger).await.expect("create trigger");

    store
        .enqueue_item(make_item(feed_id, serde_json::json!({"x": 1})))
        .await
        .expect("enqueue");
    store
        .enqueue_item(make_item(feed_id, serde_json::json!({"x": 2})))
        .await
        .expect("enqueue");

    let processor = make_processor(store.clone());
    let batch_results = processor.process_batch(&feed, 10).await.expect("process");

    assert_eq!(batch_results.len(), 2);
    for (_item, results) in &batch_results {
        assert_eq!(results.len(), 1);
        assert!(matches!(&results[0], TriggerResult::Fire(_)));
    }
}

#[tokio::test]
async fn advance_cursor_increments_position_and_count() {
    let store = make_store();
    let feed = make_feed("cursor-feed");
    let feed_id = feed.id;
    store.create_feed(feed.clone()).await.expect("create");

    let processor = make_processor(store.clone());

    // Advance cursor twice
    processor
        .advance_cursor(&feed_id, "position-1")
        .await
        .expect("advance 1");
    processor
        .advance_cursor(&feed_id, "position-2")
        .await
        .expect("advance 2");

    let updated_feed = store.get_feed(&feed_id).await.expect("get");
    assert_eq!(updated_feed.cursor.position, "position-2");
    assert_eq!(updated_feed.cursor.items_processed, 2);
}

#[tokio::test]
async fn mark_processed_prevents_redelivery() {
    let store = make_store();
    let feed = make_feed("redelivery-feed");
    let feed_id = feed.id;
    store.create_feed(feed).await.expect("create");

    let item = make_item(feed_id, serde_json::json!({"val": 42}));
    let item_id = item.id;
    store.enqueue_item(item).await.expect("enqueue");

    // Mark it processed
    store.mark_processed(item_id).await.expect("mark processed");

    // Should not appear in next dequeue
    let items = store.dequeue_items(&feed_id, 10).await.expect("dequeue");
    assert!(items.is_empty(), "processed item must not be re-delivered");
}

// ---------------------------------------------------------------------------
// IT-FEED-02: Trigger condition evaluation with JSON payloads
// ---------------------------------------------------------------------------

#[test]
fn trigger_condition_always_fires() {
    let condition = TriggerCondition::Always;
    let payload = serde_json::json!({"anything": true});
    assert!(condition.evaluate(&payload));
}

#[test]
fn trigger_condition_json_path_matches_string_field() {
    let condition = TriggerCondition::JsonPath {
        path: "/status".to_string(),
        expected: serde_json::json!("active"),
    };
    let payload = serde_json::json!({"status": "active", "value": 10});
    assert!(condition.evaluate(&payload));
}

#[test]
fn trigger_condition_json_path_rejects_mismatch() {
    let condition = TriggerCondition::JsonPath {
        path: "/status".to_string(),
        expected: serde_json::json!("active"),
    };
    let payload = serde_json::json!({"status": "paused"});
    assert!(!condition.evaluate(&payload));
}

#[test]
fn trigger_condition_json_path_nested_field() {
    let condition = TriggerCondition::JsonPath {
        path: "/data/nested/value".to_string(),
        expected: serde_json::json!(99),
    };
    let payload = serde_json::json!({"data": {"nested": {"value": 99}}});
    assert!(condition.evaluate(&payload));
}

#[test]
fn trigger_condition_threshold_gt() {
    let condition = TriggerCondition::Threshold {
        field: "/amount".to_string(),
        op: CompOp::Gt,
        value: 100.0,
    };
    assert!(condition.evaluate(&serde_json::json!({"amount": 101})));
    assert!(!condition.evaluate(&serde_json::json!({"amount": 100})));
    assert!(!condition.evaluate(&serde_json::json!({"amount": 99})));
}

#[test]
fn trigger_condition_threshold_lte() {
    let condition = TriggerCondition::Threshold {
        field: "/balance".to_string(),
        op: CompOp::Lte,
        value: 50.0,
    };
    assert!(condition.evaluate(&serde_json::json!({"balance": 50})));
    assert!(condition.evaluate(&serde_json::json!({"balance": 49})));
    assert!(!condition.evaluate(&serde_json::json!({"balance": 51})));
}

#[test]
fn trigger_condition_and_all_must_pass() {
    let condition = TriggerCondition::And(vec![
        TriggerCondition::JsonPath {
            path: "/type".to_string(),
            expected: serde_json::json!("transfer"),
        },
        TriggerCondition::Threshold {
            field: "/amount".to_string(),
            op: CompOp::Gte,
            value: 1000.0,
        },
    ]);
    // Both conditions satisfied
    assert!(condition.evaluate(&serde_json::json!({"type": "transfer", "amount": 1500})));
    // Only first satisfied
    assert!(!condition.evaluate(&serde_json::json!({"type": "transfer", "amount": 500})));
    // Only second satisfied
    assert!(!condition.evaluate(&serde_json::json!({"type": "stake", "amount": 1500})));
}

#[test]
fn trigger_condition_or_any_must_pass() {
    let condition = TriggerCondition::Or(vec![
        TriggerCondition::JsonPath {
            path: "/alert".to_string(),
            expected: serde_json::json!(true),
        },
        TriggerCondition::Threshold {
            field: "/severity".to_string(),
            op: CompOp::Gt,
            value: 8.0,
        },
    ]);
    assert!(condition.evaluate(&serde_json::json!({"alert": true, "severity": 5})));
    assert!(condition.evaluate(&serde_json::json!({"alert": false, "severity": 9})));
    assert!(!condition.evaluate(&serde_json::json!({"alert": false, "severity": 5})));
}

#[test]
fn trigger_condition_not_inverts() {
    let condition = TriggerCondition::Not(Box::new(TriggerCondition::JsonPath {
        path: "/processed".to_string(),
        expected: serde_json::json!(true),
    }));
    assert!(condition.evaluate(&serde_json::json!({"processed": false})));
    assert!(!condition.evaluate(&serde_json::json!({"processed": true})));
}

#[test]
fn trigger_bare_field_name_without_leading_slash() {
    let condition = TriggerCondition::JsonPath {
        path: "status".to_string(), // no leading slash
        expected: serde_json::json!("ok"),
    };
    assert!(condition.evaluate(&serde_json::json!({"status": "ok"})));
}

// ---------------------------------------------------------------------------
// IT-FEED-03: Recipe instantiation creates feed+trigger pair
// ---------------------------------------------------------------------------

fn make_threshold_recipe() -> Recipe {
    Recipe {
        id: RecipeId::new(),
        name: "Threshold Alert {{threshold}}".to_string(),
        description: "Fire when value exceeds threshold".to_string(),
        version: "1.0.0".to_string(),
        source: FeedSource::EventBus {
            filter: EventFilter::allow_all(),
        },
        trigger: TriggerCondition::Threshold {
            field: "/value".to_string(),
            op: CompOp::Gt,
            value: 0.0, // overridden by param substitution at condition level (not substituted in condition)
        },
        action: TriggerAction::Notify {
            channel: "{{channel}}".to_string(),
            message: "Value exceeded threshold".to_string(),
        },
        parameters: vec![
            RecipeParameter {
                name: "threshold".to_string(),
                description: "The threshold value".to_string(),
                param_type: ParamType::Number,
                default: None,
                required: true,
            },
            RecipeParameter {
                name: "channel".to_string(),
                description: "Notification channel".to_string(),
                param_type: ParamType::String,
                default: Some("default-channel".to_string()),
                required: false,
            },
        ],
    }
}

#[test]
fn recipe_instantiation_creates_feed_and_trigger_pair() {
    let recipe = make_threshold_recipe();
    let mut params = HashMap::new();
    params.insert("threshold".to_string(), serde_json::json!(500));

    let (feed, trigger) = instantiate_recipe(&recipe, &params).expect("instantiate ok");

    // Feed should be active
    assert!(matches!(feed.status, FeedStatus::Active));

    // Trigger should reference the feed
    assert_eq!(trigger.feed_id, feed.id);

    // Default channel is applied
    if let TriggerAction::Notify { channel, .. } = &trigger.action {
        assert_eq!(channel, "default-channel");
    } else {
        panic!("expected Notify action");
    }
}

#[test]
fn recipe_instantiation_with_explicit_params_substitutes_correctly() {
    let recipe = make_threshold_recipe();
    let mut params = HashMap::new();
    params.insert("threshold".to_string(), serde_json::json!(1000));
    params.insert("channel".to_string(), serde_json::json!("slack:#alerts"));

    let (feed, trigger) = instantiate_recipe(&recipe, &params).expect("instantiate ok");

    // Feed name contains substituted value
    assert!(feed.name.contains("1000"));

    if let TriggerAction::Notify { channel, .. } = &trigger.action {
        assert_eq!(channel, "slack:#alerts");
    } else {
        panic!("expected Notify action");
    }
}

#[test]
fn recipe_missing_required_param_returns_error() {
    let recipe = make_threshold_recipe();
    let params = HashMap::new(); // empty — required "threshold" is missing

    let result = instantiate_recipe(&recipe, &params);
    assert!(result.is_err(), "missing required param should fail");
}

#[test]
fn recipe_wrong_type_param_returns_error() {
    let recipe = make_threshold_recipe();
    let mut params = HashMap::new();
    // threshold expects Number, but we supply a String
    params.insert("threshold".to_string(), serde_json::json!("not-a-number"));

    let result = instantiate_recipe(&recipe, &params);
    assert!(result.is_err(), "wrong type param should fail");
}

#[test]
fn recipe_trigger_feed_ids_are_consistent() {
    let recipe = make_threshold_recipe();
    let mut params = HashMap::new();
    params.insert("threshold".to_string(), serde_json::json!(100));

    let (feed, trigger) = instantiate_recipe(&recipe, &params).expect("instantiate ok");
    assert_eq!(feed.id, trigger.feed_id, "trigger must reference the produced feed");
}

// ---------------------------------------------------------------------------
// IT-FEED-04: Cooldown prevents rapid re-firing
// ---------------------------------------------------------------------------

#[test]
fn cooldown_active_prevents_fire() {
    use chrono::Utc;
    let feed_id = FeedId::new();
    let mut trigger = always_trigger(feed_id);
    trigger.cooldown_secs = Some(3600); // 1 hour cooldown
    trigger.last_fired_at = Some(Utc::now()); // just fired

    let item = make_item(feed_id, serde_json::json!({}));
    let result = evaluate_trigger(&trigger, &item);
    assert!(
        matches!(result, TriggerResult::CooldownActive),
        "trigger in cooldown should return CooldownActive"
    );
}

#[test]
fn cooldown_expired_allows_fire() {
    use chrono::{Duration, Utc};
    let feed_id = FeedId::new();
    let mut trigger = always_trigger(feed_id);
    trigger.cooldown_secs = Some(60); // 1 minute cooldown
    trigger.last_fired_at = Some(Utc::now() - Duration::seconds(120)); // fired 2 minutes ago

    let item = make_item(feed_id, serde_json::json!({}));
    let result = evaluate_trigger(&trigger, &item);
    assert!(
        matches!(result, TriggerResult::Fire(_)),
        "cooldown expired trigger should fire"
    );
}

#[test]
fn no_cooldown_always_fires() {
    let feed_id = FeedId::new();
    let trigger = always_trigger(feed_id); // no cooldown set
    let item = make_item(feed_id, serde_json::json!({}));
    let result = evaluate_trigger(&trigger, &item);
    assert!(matches!(result, TriggerResult::Fire(_)));
}

#[test]
fn disabled_trigger_never_fires() {
    let feed_id = FeedId::new();
    let mut trigger = always_trigger(feed_id);
    trigger.enabled = false;

    let item = make_item(feed_id, serde_json::json!({"a": 1}));
    let result = evaluate_trigger(&trigger, &item);
    assert!(
        matches!(result, TriggerResult::Skip(_)),
        "disabled trigger must always skip"
    );
}

#[test]
fn condition_not_met_returns_skip() {
    let feed_id = FeedId::new();
    let trigger = Trigger::new(
        "conditional",
        feed_id,
        TriggerCondition::JsonPath {
            path: "/status".to_string(),
            expected: serde_json::json!("active"),
        },
        TriggerAction::PublishEvent {
            kind: "noop".to_string(),
            payload: serde_json::json!({}),
        },
    );
    let item = make_item(feed_id, serde_json::json!({"status": "inactive"}));
    let result = evaluate_trigger(&trigger, &item);
    assert!(matches!(result, TriggerResult::Skip(_)));
}

// ---------------------------------------------------------------------------
// IT-FEED-05: Cursor-backed at-least-once delivery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unprocessed_items_reappear_before_cursor_advance() {
    let store = make_store();
    let feed = make_feed("at-least-once");
    let feed_id = feed.id;
    store.create_feed(feed.clone()).await.expect("create");

    // Enqueue two items
    store
        .enqueue_item(make_item(feed_id, serde_json::json!({"seq": 1})))
        .await
        .expect("enqueue 1");
    store
        .enqueue_item(make_item(feed_id, serde_json::json!({"seq": 2})))
        .await
        .expect("enqueue 2");

    // Dequeue without marking processed or advancing cursor
    let first_batch = store.dequeue_items(&feed_id, 10).await.expect("first dequeue");
    assert_eq!(first_batch.len(), 2);

    // Dequeue again — items should still appear (not yet marked processed)
    let second_batch = store.dequeue_items(&feed_id, 10).await.expect("second dequeue");
    assert_eq!(second_batch.len(), 2, "unprocessed items must reappear for at-least-once delivery");
}

#[tokio::test]
async fn cursor_advances_after_batch_processing() {
    let store = make_store();
    let feed = make_feed("cursor-advance");
    let feed_id = feed.id;
    store.create_feed(feed.clone()).await.expect("create");

    let trigger = always_trigger(feed_id);
    store.create_trigger(trigger).await.expect("create trigger");

    store
        .enqueue_item(make_item(feed_id, serde_json::json!({"n": 1})))
        .await
        .expect("enqueue");

    let processor = make_processor(store.clone());
    processor.process_batch(&feed, 10).await.expect("process");
    processor
        .advance_cursor(&feed_id, "seq-1")
        .await
        .expect("advance");

    let updated = store.get_feed(&feed_id).await.expect("get");
    assert_eq!(updated.cursor.position, "seq-1");
    assert!(updated.cursor.items_processed >= 1);
}

#[tokio::test]
async fn feed_status_active_on_creation() {
    let store = make_store();
    let feed = make_feed("status-check");
    store.create_feed(feed).await.expect("create");
    let feeds = store.list_feeds().await.expect("list");
    assert_eq!(feeds.len(), 1);
    assert!(matches!(feeds[0].status, FeedStatus::Active));
}

#[tokio::test]
async fn multiple_feeds_isolated_queues() {
    let store = make_store();
    let feed_a = make_feed("feed-a");
    let feed_b = make_feed("feed-b");
    let id_a = feed_a.id;
    let id_b = feed_b.id;

    store.create_feed(feed_a).await.expect("create a");
    store.create_feed(feed_b).await.expect("create b");

    // Enqueue items only to feed_a
    for _ in 0..3 {
        store
            .enqueue_item(make_item(id_a, serde_json::json!({"for": "a"})))
            .await
            .expect("enqueue");
    }

    let items_a = store.dequeue_items(&id_a, 10).await.expect("dequeue a");
    let items_b = store.dequeue_items(&id_b, 10).await.expect("dequeue b");

    assert_eq!(items_a.len(), 3, "feed A should have 3 items");
    assert_eq!(items_b.len(), 0, "feed B should have no items");
}
