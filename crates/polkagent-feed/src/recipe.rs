//! Reusable trigger-to-action templates (recipes).
//!
//! A [`Recipe`] bundles a [`FeedSource`] template, a [`TriggerCondition`]
//! template, and a [`TriggerAction`] template together with a parameter schema.
//! Callers supply concrete parameter values and receive a ready-to-use
//! ([`Feed`], [`Trigger`]) pair via [`instantiate_recipe`].

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{FeedError, Result};
use crate::trigger::{Trigger, TriggerAction, TriggerCondition};
use crate::types::{Feed, FeedId, FeedSource};

// ---------------------------------------------------------------------------
// RecipeId
// ---------------------------------------------------------------------------

/// Unique identifier for a [`Recipe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RecipeId(Uuid);

impl RecipeId {
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

impl Default for RecipeId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RecipeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for RecipeId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

impl From<Uuid> for RecipeId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl From<RecipeId> for Uuid {
    fn from(id: RecipeId) -> Self {
        id.0
    }
}

// ---------------------------------------------------------------------------
// ParamType
// ---------------------------------------------------------------------------

/// The expected type for a [`RecipeParameter`] value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
    /// A UTF-8 string.
    String,
    /// A JSON number.
    Number,
    /// A boolean.
    Bool,
    /// An arbitrary JSON value.
    Json,
}

impl ParamType {
    /// Return `true` if `value` conforms to this parameter type.
    #[must_use]
    pub fn validate(self, value: &serde_json::Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Number => value.is_number(),
            Self::Bool => value.is_boolean(),
            Self::Json => true, // Any JSON value is acceptable.
        }
    }
}

// ---------------------------------------------------------------------------
// RecipeParameter
// ---------------------------------------------------------------------------

/// Describes a single configurable parameter in a [`Recipe`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeParameter {
    /// The parameter name; used as the key when providing values to
    /// [`instantiate_recipe`].
    pub name: String,

    /// Human-readable description of what this parameter controls.
    pub description: String,

    /// The expected value type.
    pub param_type: ParamType,

    /// Optional default value used when the caller does not supply a value.
    pub default: Option<String>,

    /// When `true`, instantiation fails if this parameter has no value and no
    /// default.
    pub required: bool,
}

// ---------------------------------------------------------------------------
// Recipe
// ---------------------------------------------------------------------------

/// A versioned, reusable blueprint that produces a ([`Feed`], [`Trigger`]) pair
/// when instantiated with concrete parameter values.
///
/// Recipe templates use simple `{{param_name}}` placeholder syntax within
/// string-typed fields.  The [`instantiate_recipe`] function performs the
/// substitution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    /// Unique identifier for this recipe.
    pub id: RecipeId,

    /// Human-readable name for this recipe.
    pub name: String,

    /// Longer description of what this recipe does.
    pub description: String,

    /// Semantic version string (e.g. `"1.0.0"`).
    pub version: String,

    /// Feed source template.  String fields may contain `{{param_name}}`
    /// placeholders.
    pub source: FeedSource,

    /// Trigger condition template.
    pub trigger: TriggerCondition,

    /// Trigger action template.  String fields may contain `{{param_name}}`
    /// placeholders.
    pub action: TriggerAction,

    /// Ordered list of parameters that the recipe accepts.
    pub parameters: Vec<RecipeParameter>,
}

// ---------------------------------------------------------------------------
// instantiate_recipe
// ---------------------------------------------------------------------------

/// Instantiate a [`Recipe`] by substituting `params` into the templates,
/// returning a ready-to-use ([`Feed`], [`Trigger`]) pair.
///
/// # Errors
///
/// Returns [`FeedError::InvalidRecipeParams`] if:
/// - A required parameter has no value and no default.
/// - A supplied value does not match the declared [`ParamType`].
pub fn instantiate_recipe(
    recipe: &Recipe,
    params: &HashMap<String, serde_json::Value>,
) -> Result<(Feed, Trigger)> {
    // Resolve all parameter values.
    let resolved = resolve_params(recipe, params)?;

    // Build the agent_id for the feed.  We use a default AgentId if none is
    // supplied (the caller can override it afterwards).
    let agent_id = polkagent_core::AgentId::new();

    // Clone and substitute the feed source.
    let source = substitute_source(&recipe.source, &resolved)?;

    // Build a name for the feed from the recipe name plus any substitution.
    let feed_name = substitute_str(&recipe.name, &resolved);

    let mut feed = Feed::new(feed_name, source, agent_id);

    // Ensure the FeedId is freshly generated (it is, via Feed::new).
    let feed_id: FeedId = feed.id;

    // Clone and substitute the trigger action.
    let action = substitute_action(&recipe.action, &resolved)?;
    let condition = recipe.trigger.clone();

    let trigger = Trigger::new(
        substitute_str(&recipe.name, &resolved),
        feed_id,
        condition,
        action,
    );

    // Apply recipe description as part of feed name for traceability.
    feed.name = format!("{} (recipe: {})", feed.name, recipe.id);

    Ok((feed, trigger))
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Build a map of parameter name → resolved JSON value, applying defaults and
/// validating types.
fn resolve_params(
    recipe: &Recipe,
    supplied: &HashMap<String, serde_json::Value>,
) -> Result<HashMap<String, serde_json::Value>> {
    let mut resolved: HashMap<String, serde_json::Value> = HashMap::new();

    for param in &recipe.parameters {
        if let Some(value) = supplied.get(&param.name) {
            // Validate the supplied value's type.
            if !param.param_type.validate(value) {
                return Err(FeedError::InvalidRecipeParams(format!(
                    "parameter '{}' expected {:?} but got {:?}",
                    param.name, param.param_type, value
                )));
            }
            resolved.insert(param.name.clone(), value.clone());
        } else if let Some(default_str) = &param.default {
            // Parse the default string as JSON or use it as a raw string.
            let value = parse_default(default_str, param.param_type);
            resolved.insert(param.name.clone(), value);
        } else if param.required {
            return Err(FeedError::InvalidRecipeParams(format!(
                "required parameter '{}' is missing",
                param.name
            )));
        }
    }

    Ok(resolved)
}

/// Parse a default string value according to the declared type.
fn parse_default(default_str: &str, param_type: ParamType) -> serde_json::Value {
    match param_type {
        ParamType::String => serde_json::Value::String(default_str.to_string()),
        ParamType::Number => default_str
            .parse::<f64>()
            .ok()
            .and_then(|f| serde_json::Number::from_f64(f))
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::String(default_str.to_string())),
        ParamType::Bool => match default_str {
            "true" => serde_json::Value::Bool(true),
            "false" => serde_json::Value::Bool(false),
            other => serde_json::Value::String(other.to_string()),
        },
        ParamType::Json => serde_json::from_str(default_str)
            .unwrap_or(serde_json::Value::String(default_str.to_string())),
    }
}

/// Replace `{{key}}` placeholders in `s` with the string representation of the
/// resolved parameter value.
fn substitute_str(s: &str, params: &HashMap<String, serde_json::Value>) -> String {
    let mut result = s.to_string();
    for (key, value) in params {
        let placeholder = format!("{{{{{key}}}}}");
        let replacement = match value {
            serde_json::Value::String(sv) => sv.clone(),
            other => other.to_string(),
        };
        result = result.replace(&placeholder, &replacement);
    }
    result
}

/// Perform placeholder substitution on a [`FeedSource`].
fn substitute_source(
    source: &FeedSource,
    params: &HashMap<String, serde_json::Value>,
) -> Result<FeedSource> {
    Ok(match source {
        FeedSource::Schedule { cron } => FeedSource::Schedule {
            cron: substitute_str(cron, params),
        },
        FeedSource::Webhook { path, secret_hash } => FeedSource::Webhook {
            path: substitute_str(path, params),
            secret_hash: secret_hash.as_deref().map(|s| substitute_str(s, params)),
        },
        FeedSource::EventBus { filter } => FeedSource::EventBus {
            filter: filter.clone(),
        },
        FeedSource::ChainState {
            query,
            interval_secs,
        } => FeedSource::ChainState {
            query: substitute_str(query, params),
            interval_secs: *interval_secs,
        },
    })
}

/// Perform placeholder substitution on a [`TriggerAction`].
fn substitute_action(
    action: &TriggerAction,
    params: &HashMap<String, serde_json::Value>,
) -> Result<TriggerAction> {
    Ok(match action {
        TriggerAction::StartRun {
            prompt_template,
            agent_id,
        } => TriggerAction::StartRun {
            prompt_template: substitute_str(prompt_template, params),
            agent_id: substitute_str(agent_id, params),
        },
        TriggerAction::PublishEvent { kind, payload } => TriggerAction::PublishEvent {
            kind: substitute_str(kind, params),
            payload: payload.clone(),
        },
        TriggerAction::Notify { channel, message } => TriggerAction::Notify {
            channel: substitute_str(channel, params),
            message: substitute_str(message, params),
        },
    })
}
