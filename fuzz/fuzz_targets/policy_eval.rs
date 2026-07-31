#![no_main]

use libfuzzer_sys::fuzz_target;
use polkagent_grant::policy::{
    evaluate, Effect, EvaluationContext, PolicyRule, PolicySet, pattern_matches,
};
use std::collections::HashMap;

fuzz_target!(|data: &[u8]| {
    if data.len() < 6 {
        return;
    }

    // Carve up the input into segments for different parts of the policy.
    let num_rules = (data[0] % 8) as usize;
    let mut offset = 1usize;

    let mut rules = Vec::new();

    for i in 0..num_rules {
        if offset + 4 >= data.len() {
            break;
        }

        let effect = if data[offset] % 2 == 0 {
            Effect::Allow
        } else {
            Effect::Deny
        };
        offset += 1;

        // Extract action pattern from bytes.
        let action_pat_len = (data.get(offset).copied().unwrap_or(0) % 32) as usize;
        offset += 1;
        let action_pat = extract_str(data, &mut offset, action_pat_len);

        // Extract resource pattern.
        let resource_pat_len = (data.get(offset).copied().unwrap_or(0) % 32) as usize;
        offset += 1;
        let resource_pat = extract_str(data, &mut offset, resource_pat_len);

        // Optional conditions.
        let num_conditions = data.get(offset).copied().unwrap_or(0) % 4;
        offset += 1;
        let mut conditions = HashMap::new();
        for _ in 0..num_conditions {
            let klen = (data.get(offset).copied().unwrap_or(0) % 16) as usize;
            offset += 1;
            let key = extract_str(data, &mut offset, klen);
            let vlen = (data.get(offset).copied().unwrap_or(0) % 16) as usize;
            offset += 1;
            let val = extract_str(data, &mut offset, vlen);
            conditions.insert(key, val);
        }

        rules.push(PolicyRule {
            id: format!("rule-{i}"),
            effect,
            action_patterns: vec![action_pat],
            resource_patterns: vec![resource_pat],
            conditions,
        });
    }

    let policy_set = PolicySet::new(rules);

    // Extract action and resource strings for evaluation.
    let action_len = (data.get(offset).copied().unwrap_or(0) % 32) as usize;
    offset += 1;
    let action = extract_str(data, &mut offset, action_len);

    let resource_len = (data.get(offset).copied().unwrap_or(0) % 32) as usize;
    offset += 1;
    let resource = extract_str(data, &mut offset, resource_len);

    // Build evaluation context attributes.
    let num_attrs = data.get(offset).copied().unwrap_or(0) % 4;
    offset += 1;
    let mut attributes = HashMap::new();
    for _ in 0..num_attrs {
        let klen = (data.get(offset).copied().unwrap_or(0) % 16) as usize;
        offset += 1;
        let key = extract_str(data, &mut offset, klen);
        let vlen = (data.get(offset).copied().unwrap_or(0) % 16) as usize;
        offset += 1;
        let val = extract_str(data, &mut offset, vlen);
        attributes.insert(key, val);
    }

    let ctx = EvaluationContext {
        attributes,
        evaluated_at: None,
    };

    // evaluate() must never panic.
    let _decision = evaluate(&policy_set, &action, &resource, &ctx);

    // pattern_matches with raw fuzzed strings must never panic.
    let _ = pattern_matches(&action, &resource);
    let _ = pattern_matches(&resource, &action);
    let _ = pattern_matches("**", &action);
    let _ = pattern_matches(&action, "**");
});

/// Extract a string of up to `max_len` bytes from `data` starting at `offset`.
/// Advances `offset` past the consumed bytes.
fn extract_str(data: &[u8], offset: &mut usize, max_len: usize) -> String {
    let start = *offset;
    let end = core::cmp::min(start + max_len, data.len());
    *offset = end;
    String::from_utf8_lossy(&data[start..end]).into_owned()
}
