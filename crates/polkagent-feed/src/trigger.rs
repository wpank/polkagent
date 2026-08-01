//! Trigger evaluation — conditions, actions, and firing logic.
//!
//! A [`Trigger`] binds a [`TriggerCondition`] to a [`TriggerAction`] for a
//! specific feed.  When the feed processor dequeues a [`FeedItem`] it calls
//! [`evaluate_trigger`] for every trigger registered on that feed.  The result
//! is a [`TriggerResult`] indicating whether the action should fire, or why it
//! was skipped.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{FeedId, FeedItem};

// ---------------------------------------------------------------------------
// TriggerId
// ---------------------------------------------------------------------------

/// Unique identifier for a [`Trigger`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TriggerId(Uuid);

impl TriggerId {
    /// Generate a new time-ordered (v7) identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing [`Uuid`].
    #[must_use]
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Return the inner [`Uuid`].
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for TriggerId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TriggerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for TriggerId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for TriggerId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<TriggerId> for Uuid {
    fn from(id: TriggerId) -> Self {
        id.0
    }
}

// ---------------------------------------------------------------------------
// CompOp
// ---------------------------------------------------------------------------

/// Comparison operator used in [`TriggerCondition::Threshold`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompOp {
    /// Greater than (`>`).
    Gt,
    /// Greater than or equal to (`>=`).
    Gte,
    /// Less than (`<`).
    Lt,
    /// Less than or equal to (`<=`).
    Lte,
    /// Equal to (`==`).
    Eq,
    /// Not equal to (`!=`).
    Neq,
}

impl CompOp {
    /// Apply this operator to two floating-point values.
    #[must_use]
    pub fn evaluate(self, lhs: f64, rhs: f64) -> bool {
        match self {
            Self::Gt => lhs > rhs,
            Self::Gte => lhs >= rhs,
            Self::Lt => lhs < rhs,
            Self::Lte => lhs <= rhs,
            Self::Eq => (lhs - rhs).abs() < f64::EPSILON,
            Self::Neq => (lhs - rhs).abs() >= f64::EPSILON,
        }
    }
}

// ---------------------------------------------------------------------------
// TriggerCondition
// ---------------------------------------------------------------------------

/// Determines whether a trigger should fire for a given [`FeedItem`].
///
/// `Serialize` and `Deserialize` are implemented manually (below) to avoid
/// the deep monomorphisation chains that the derive macros generate for
/// recursive, internally-tagged enums.  The on-wire JSON format is identical
/// to `#[serde(tag = "type", rename_all = "snake_case")]`.
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerCondition {
    /// Always fires; useful for unconditional triggers such as "run on every
    /// item".
    Always,

    /// Fires when the value at a JSON-Pointer `path` within the item payload
    /// equals `expected`.
    JsonPath {
        /// A JSON Pointer (RFC 6901) path such as `"/status"` or `"/data/0/value"`.
        path: String,

        /// The value that the resolved field must equal.
        expected: serde_json::Value,
    },

    /// Fires when a numeric field extracted from the payload satisfies a
    /// comparison against a constant threshold.
    Threshold {
        /// A JSON Pointer path to the numeric field.
        field: String,

        /// The comparison operator to apply.
        op: CompOp,

        /// The right-hand-side value of the comparison.
        value: f64,
    },

    /// Fires only when **all** inner conditions fire.
    And(Vec<TriggerCondition>),

    /// Fires when **any** inner condition fires.
    Or(Vec<TriggerCondition>),

    /// Fires when the inner condition does **not** fire.
    Not(Box<TriggerCondition>),
}

// ---------------------------------------------------------------------------
// Manual Serialize / Deserialize for TriggerCondition
//
// The recursive variants (And, Or, Not) cause the Rust compiler to hit its
// recursion limit when serde's derive macros try to monomorphise the
// internally-tagged-enum code path (which routes through an intermediate
// `Content` representation).  By going through `serde_json::Value` manually
// the recursive monomorphisation chain is broken: we only ever serialize /
// deserialize `serde_json::Value`, not `TriggerCondition` itself.
// ---------------------------------------------------------------------------

impl Serialize for TriggerCondition {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        // Build a serde_json::Value and then serialize *that*.
        let value = tc_to_value(self).map_err(serde::ser::Error::custom)?;
        value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TriggerCondition {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        tc_from_value(&value).map_err(serde::de::Error::custom)
    }
}

/// Convert a `TriggerCondition` into a `serde_json::Value`.
///
/// The format mirrors `#[serde(tag = "type", rename_all = "snake_case")]`:
///
/// - `{"type":"always"}`
/// - `{"type":"json_path","path":"...","expected":...}`
/// - `{"type":"threshold","field":"...","op":"...","value":...}`
/// - `{"type":"and","conditions":[...]}`
/// - `{"type":"or","conditions":[...]}`
/// - `{"type":"not","condition":{...}}`
fn tc_to_value(tc: &TriggerCondition) -> std::result::Result<serde_json::Value, serde_json::Error> {
    match tc {
        TriggerCondition::Always => {
            Ok(serde_json::json!({ "type": "always" }))
        }
        TriggerCondition::JsonPath { path, expected } => {
            Ok(serde_json::json!({
                "type": "json_path",
                "path": path,
                "expected": expected,
            }))
        }
        TriggerCondition::Threshold { field, op, value } => {
            let op_value = serde_json::to_value(op)?;
            Ok(serde_json::json!({
                "type": "threshold",
                "field": field,
                "op": op_value,
                "value": value,
            }))
        }
        TriggerCondition::And(conditions) => {
            let items: Vec<serde_json::Value> = conditions
                .iter()
                .map(tc_to_value)
                .collect::<std::result::Result<_, _>>()?;
            Ok(serde_json::json!({
                "type": "and",
                "conditions": items,
            }))
        }
        TriggerCondition::Or(conditions) => {
            let items: Vec<serde_json::Value> = conditions
                .iter()
                .map(tc_to_value)
                .collect::<std::result::Result<_, _>>()?;
            Ok(serde_json::json!({
                "type": "or",
                "conditions": items,
            }))
        }
        TriggerCondition::Not(inner) => {
            let inner_value = tc_to_value(inner)?;
            Ok(serde_json::json!({
                "type": "not",
                "condition": inner_value,
            }))
        }
    }
}

/// Reconstruct a `TriggerCondition` from a `serde_json::Value`.
fn tc_from_value(value: &serde_json::Value) -> std::result::Result<TriggerCondition, String> {
    let obj = value.as_object().ok_or("expected JSON object for TriggerCondition")?;
    let type_str = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or("missing or non-string \"type\" field")?;

    match type_str {
        "always" => Ok(TriggerCondition::Always),

        "json_path" => {
            let path = obj
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or("json_path: missing \"path\"")?
                .to_string();
            let expected = obj
                .get("expected")
                .cloned()
                .ok_or("json_path: missing \"expected\"")?;
            Ok(TriggerCondition::JsonPath { path, expected })
        }

        "threshold" => {
            let field = obj
                .get("field")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "threshold: missing \"field\"".to_string())?
                .to_string();
            let op_val = obj
                .get("op")
                .ok_or_else(|| "threshold: missing \"op\"".to_string())?;
            let op: CompOp = serde_json::from_value(op_val.clone())
                .map_err(|e| format!("threshold: bad op: {e}"))?;
            let val = obj
                .get("value")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "threshold: missing or non-numeric \"value\"".to_string())?;
            Ok(TriggerCondition::Threshold { field, op, value: val })
        }

        "and" => {
            let arr = obj
                .get("conditions")
                .and_then(|v| v.as_array())
                .ok_or("and: missing \"conditions\" array")?;
            let conditions: Vec<TriggerCondition> = arr
                .iter()
                .map(tc_from_value)
                .collect::<std::result::Result<_, _>>()?;
            Ok(TriggerCondition::And(conditions))
        }

        "or" => {
            let arr = obj
                .get("conditions")
                .and_then(|v| v.as_array())
                .ok_or("or: missing \"conditions\" array")?;
            let conditions: Vec<TriggerCondition> = arr
                .iter()
                .map(tc_from_value)
                .collect::<std::result::Result<_, _>>()?;
            Ok(TriggerCondition::Or(conditions))
        }

        "not" => {
            let inner_value = obj
                .get("condition")
                .ok_or("not: missing \"condition\"")?;
            let inner = tc_from_value(inner_value)?;
            Ok(TriggerCondition::Not(Box::new(inner)))
        }

        other => Err(format!("unknown TriggerCondition type: \"{other}\"")),
    }
}

impl TriggerCondition {
    /// Evaluate this condition against a payload.  Returns `true` if the
    /// condition is satisfied.
    #[must_use]
    pub fn evaluate(&self, payload: &serde_json::Value) -> bool {
        match self {
            Self::Always => true,

            Self::JsonPath { path, expected } => {
                resolve_json_pointer(payload, path)
                    .map(|v| v == expected)
                    .unwrap_or(false)
            }

            Self::Threshold { field, op, value } => {
                resolve_json_pointer(payload, field)
                    .and_then(|v| v.as_f64())
                    .map(|f| op.evaluate(f, *value))
                    .unwrap_or(false)
            }

            Self::And(conditions) => conditions.iter().all(|c| c.evaluate(payload)),

            Self::Or(conditions) => conditions.iter().any(|c| c.evaluate(payload)),

            Self::Not(inner) => !inner.evaluate(payload),
        }
    }
}

/// Resolve a JSON Pointer path (RFC 6901) against a value.
///
/// Handles both absolute paths starting with `/` and bare field names without
/// the leading slash.
fn resolve_json_pointer<'a>(
    value: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    if path.is_empty() {
        return Some(value);
    }
    // If the path starts with '/' treat it as a proper JSON Pointer.
    if path.starts_with('/') {
        value.pointer(path)
    } else {
        // Allow bare field names as a convenience (no leading slash).
        value.pointer(&format!("/{path}"))
    }
}

// ---------------------------------------------------------------------------
// TriggerAction
// ---------------------------------------------------------------------------

/// The action to execute when a trigger fires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerAction {
    /// Start an agent run with the given prompt template.
    StartRun {
        /// A prompt template; the literal string `{{payload}}` is replaced with
        /// the serialised feed item payload.
        prompt_template: String,

        /// The agent to invoke.
        agent_id: String,
    },

    /// Publish a platform event onto the internal event bus.
    PublishEvent {
        /// The event kind string (e.g. `"threshold.breached"`).
        kind: String,

        /// Static payload to attach to the published event.
        payload: serde_json::Value,
    },

    /// Send a notification to an external channel.
    Notify {
        /// The notification channel identifier (e.g. `"slack:#alerts"`).
        channel: String,

        /// The message body to deliver.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// TriggerResult
// ---------------------------------------------------------------------------

/// Outcome returned by [`evaluate_trigger`].
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerResult {
    /// The trigger fired — the enclosed action should be executed.
    Fire(TriggerAction),

    /// The trigger's condition was not satisfied; the reason is human-readable.
    Skip(String),

    /// The trigger cannot fire yet because its cooldown window has not elapsed.
    CooldownActive,
}

// ---------------------------------------------------------------------------
// Trigger
// ---------------------------------------------------------------------------

/// A named rule that fires a [`TriggerAction`] when a [`TriggerCondition`] is
/// satisfied for a feed item, subject to an optional cooldown period.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger {
    /// Unique identifier for this trigger.
    pub id: TriggerId,

    /// Human-readable name.
    pub name: String,

    /// The feed whose items this trigger evaluates.
    pub feed_id: FeedId,

    /// The condition that must be satisfied for the trigger to fire.
    pub condition: TriggerCondition,

    /// The action to execute when the trigger fires.
    pub action: TriggerAction,

    /// Minimum number of seconds that must elapse between firings. `None`
    /// means no cooldown is enforced.
    pub cooldown_secs: Option<u64>,

    /// When this trigger last fired. Used to enforce cooldown.
    pub last_fired_at: Option<DateTime<Utc>>,

    /// When `false`, the trigger is not evaluated regardless of conditions.
    pub enabled: bool,
}

impl Trigger {
    /// Create a new enabled trigger with no cooldown.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        feed_id: FeedId,
        condition: TriggerCondition,
        action: TriggerAction,
    ) -> Self {
        Self {
            id: TriggerId::new(),
            name: name.into(),
            feed_id,
            condition,
            action,
            cooldown_secs: None,
            last_fired_at: None,
            enabled: true,
        }
    }

    /// Return `true` if the trigger is currently within its cooldown window.
    #[must_use]
    pub fn is_in_cooldown(&self, now: DateTime<Utc>) -> bool {
        match (self.cooldown_secs, self.last_fired_at) {
            (Some(secs), Some(last)) => {
                let elapsed = now.signed_duration_since(last).num_seconds();
                elapsed < secs as i64
            }
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// evaluate_trigger
// ---------------------------------------------------------------------------

/// Evaluate a [`Trigger`] against a [`FeedItem`] and return the firing result.
///
/// # Semantics
///
/// 1. If the trigger is disabled, it always produces [`TriggerResult::Skip`].
/// 2. If the trigger is within its cooldown window, it produces
///    [`TriggerResult::CooldownActive`].
/// 3. If the condition evaluates to `false`, it produces
///    [`TriggerResult::Skip`].
/// 4. Otherwise it produces [`TriggerResult::Fire`] with a clone of the
///    trigger's action.
#[must_use]
pub fn evaluate_trigger(trigger: &Trigger, item: &FeedItem) -> TriggerResult {
    if !trigger.enabled {
        return TriggerResult::Skip("trigger is disabled".to_string());
    }

    let now = Utc::now();
    if trigger.is_in_cooldown(now) {
        return TriggerResult::CooldownActive;
    }

    if trigger.condition.evaluate(&item.payload) {
        TriggerResult::Fire(trigger.action.clone())
    } else {
        TriggerResult::Skip("condition not satisfied".to_string())
    }
}
