//! Persisted agent normalization.

use polkagent_core::{AgentId, AgentSpec};
use polkagent_store_sqlite::AgentRow;

use crate::RuntimeError;

/// Result of reconstructing a persisted agent specification.
pub(crate) struct RehydratedAgent {
    pub(crate) spec: AgentSpec,
    pub(crate) normalized_legacy_shape: bool,
}

/// Reconstruct an `AgentSpec`, including the legacy CLI representation that
/// stored the generated ID only in the relational row.
pub(crate) fn rehydrate(
    row: &AgentRow,
    model_override: Option<&str>,
) -> Result<RehydratedAgent, RuntimeError> {
    let agent_id = row
        .id
        .parse::<AgentId>()
        .map_err(|error| RuntimeError::AgentSpec {
            agent_id: row.id.clone(),
            message: format!("invalid row identifier: {error}"),
        })?;

    let mut value: serde_json::Value =
        serde_json::from_str(&row.spec_json).map_err(|error| RuntimeError::AgentSpec {
            agent_id: row.id.clone(),
            message: format!("spec_json is not valid JSON: {error}"),
        })?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| RuntimeError::AgentSpec {
            agent_id: row.id.clone(),
            message: "spec_json must contain a JSON object".to_owned(),
        })?;

    let mut normalized_legacy_shape = !object.contains_key("id");
    object.insert(
        "id".to_owned(),
        serde_json::Value::String(agent_id.to_string()),
    );
    // The relational row is authoritative for identity and display name.
    object.insert(
        "name".to_owned(),
        serde_json::Value::String(row.name.clone()),
    );
    // `polkagent agent create` historically persisted the generated ID only
    // in the row and omitted a few nullable/defaultable AgentSpec fields.
    // Supply only structural defaults; never invent a missing model.
    let legacy_defaults = [
        (
            "description",
            row.description
                .clone()
                .map_or(serde_json::Value::Null, serde_json::Value::String),
        ),
        ("tools", serde_json::Value::Array(Vec::new())),
        ("system_prompt", serde_json::Value::Null),
        (
            "autonomy_level",
            serde_json::Value::String("supervised".to_owned()),
        ),
        (
            "created_at",
            serde_json::Value::String(row.created_at.clone()),
        ),
        (
            "updated_at",
            serde_json::Value::String(row.updated_at.clone()),
        ),
    ];
    for (key, value) in legacy_defaults {
        if !object.contains_key(key) {
            object.insert(key.to_owned(), value);
            normalized_legacy_shape = true;
        }
    }

    let mut spec: AgentSpec =
        serde_json::from_value(value).map_err(|error| RuntimeError::AgentSpec {
            agent_id: row.id.clone(),
            message: format!("spec_json does not match AgentSpec: {error}"),
        })?;
    spec.id = agent_id;
    spec.name.clone_from(&row.name);
    if let Some(model) = model_override {
        model.clone_into(&mut spec.model);
    }
    spec.validate().map_err(|error| RuntimeError::AgentSpec {
        agent_id: row.id.clone(),
        message: error.to_string(),
    })?;

    Ok(RehydratedAgent {
        spec,
        normalized_legacy_shape,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(spec_json: String) -> AgentRow {
        AgentRow {
            id: AgentId::new().to_string(),
            name: "legacy".to_owned(),
            description: None,
            state: "active".to_owned(),
            spec_json,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn injects_relational_id_into_legacy_cli_spec() {
        let now = "2026-01-01T00:00:00Z";
        let row = row(serde_json::json!({
            "name": "legacy",
            "description": null,
            "model": "fake/model",
            "tools": [],
            "system_prompt": null,
            "autonomy_level": "supervised",
            "created_at": now,
            "updated_at": now
        })
        .to_string());

        let rehydrated = rehydrate(&row, None).expect("legacy spec should normalize");
        assert!(rehydrated.normalized_legacy_shape);
        assert_eq!(rehydrated.spec.id.to_string(), row.id);
    }

    #[test]
    fn rejects_non_object_json() {
        let row = row("[]".to_owned());
        assert!(matches!(
            rehydrate(&row, None),
            Err(RuntimeError::AgentSpec { .. })
        ));
    }
}
