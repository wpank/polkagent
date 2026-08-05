//! Property coverage for the serialized interaction boundary.

#![allow(clippy::expect_used)]

use polkagent_core::ids::ConversationId;
use polkagent_interaction::{
    CommandParseError, CommandRegistry, InteractionConfig, InteractionEvent,
    InteractionEventEnvelope, InteractionEventId, InteractionOverrides, InteractionTarget,
    InteractionTurnId, ParsedLine, PlanEntryId, ToolCallId,
};
use proptest::prelude::*;
use uuid::Uuid;

proptest! {
    #[test]
    fn interaction_ids_preserve_arbitrary_uuid_values(bytes in any::<[u8; 16]>()) {
        let uuid = Uuid::from_bytes(bytes);

        let turn = InteractionTurnId::from_uuid(uuid);
        prop_assert_eq!(turn.to_string().parse(), Ok(turn));
        let decoded = serde_json::from_str::<InteractionTurnId>(
            &serde_json::to_string(&turn).expect("serialize turn ID")
        ).expect("deserialize turn ID");
        prop_assert_eq!(decoded, turn);

        let call = ToolCallId::from_uuid(uuid);
        prop_assert_eq!(call.to_string().parse(), Ok(call));
        let decoded = serde_json::from_str::<ToolCallId>(
            &serde_json::to_string(&call).expect("serialize call ID")
        ).expect("deserialize call ID");
        prop_assert_eq!(decoded, call);

        let entry = PlanEntryId::from_uuid(uuid);
        prop_assert_eq!(entry.to_string().parse(), Ok(entry));
        let decoded = serde_json::from_str::<PlanEntryId>(
            &serde_json::to_string(&entry).expect("serialize entry ID")
        ).expect("deserialize entry ID");
        prop_assert_eq!(decoded, entry);

        let event = InteractionEventId::from_uuid(uuid);
        prop_assert_eq!(event.to_string().parse(), Ok(event));
        let decoded = serde_json::from_str::<InteractionEventId>(
            &serde_json::to_string(&event).expect("serialize event ID")
        ).expect("deserialize event ID");
        prop_assert_eq!(decoded, event);
    }

    #[test]
    fn ordinary_prompts_are_never_normalized(input in any::<String>()) {
        prop_assume!(!input.trim_start().starts_with('/'));
        let parsed = CommandRegistry::mvp().parse(&input);
        prop_assert_eq!(parsed, Ok(ParsedLine::Prompt(input)));
    }

    #[test]
    fn unknown_slash_commands_return_typed_errors(name in "[a-z]{1,16}") {
        let registry = CommandRegistry::mvp();
        prop_assume!(registry.resolve(&name).is_none());
        let parsed = registry.parse(&format!("/{name}"));
        prop_assert_eq!(
            parsed,
            Err(CommandParseError::UnknownCommand { name })
        );
    }

    #[test]
    fn event_envelopes_round_trip_without_losing_sequence_or_text(
        sequence in 1_u64..=u64::MAX,
        text in any::<String>(),
    ) {
        let envelope = InteractionEventEnvelope::new(
            ConversationId::new(),
            InteractionTurnId::new(),
            sequence,
            InteractionEvent::TurnCancelled {
                reason: Some(text),
            },
        )
        .expect("nonzero sequence");
        let json = serde_json::to_string(&envelope).expect("serialize envelope");
        let decoded: InteractionEventEnvelope =
            serde_json::from_str(&json).expect("deserialize envelope");
        prop_assert_eq!(decoded, envelope);
    }
}

#[test]
fn inherited_overrides_are_an_identity_operation() {
    let base = InteractionConfig::new(InteractionTarget::Auto);
    let resolved = InteractionOverrides::default()
        .apply_to(&base)
        .expect("valid inherited configuration");
    assert_eq!(resolved, base);
}

#[test]
fn every_catalog_spelling_is_unique_and_resolves_to_its_owner() {
    let registry = CommandRegistry::mvp();
    let mut spellings = std::collections::BTreeSet::new();
    for spec in registry.specs() {
        for spelling in std::iter::once(&spec.name).chain(&spec.aliases) {
            assert!(spellings.insert(spelling));
            assert_eq!(
                registry.resolve(spelling).map(|resolved| resolved.command),
                Some(spec.command)
            );
        }
    }
    assert!(registry.validate().is_ok());
}
