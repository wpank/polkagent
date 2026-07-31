#![no_main]

use libfuzzer_sys::fuzz_target;
use polkagent_core::{EffectIntentState, EffectOutcomeId};
use chrono::Utc;

/// The transitions we model for fuzzing the EffectIntentState state machine.
/// Each variant represents an attempted state transition; the fuzzer picks
/// random sequences of these.
#[derive(Debug, Clone)]
enum FuzzTransition {
    /// Transition from Pending to Claimed.
    Claim { worker_id: String },
    /// Transition from Claimed to Executing.
    BeginExecution,
    /// Transition to Resolved (terminal).
    Resolve,
    /// Transition to Retrying (with backoff).
    Retry { error_msg: String },
    /// Transition to Superseded (terminal).
    Supersede { reason: String },
    /// Reset to Pending (simulates retry cycle completion).
    ResetToPending,
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }

    // Start in Pending state.
    let mut state = EffectIntentState::Pending;
    let mut offset = 0usize;

    // Generate a sequence of transitions from the fuzz data.
    while offset < data.len() {
        let transition_byte = data[offset];
        offset += 1;

        let transition = match transition_byte % 6 {
            0 => {
                let wid_len = (data.get(offset).copied().unwrap_or(0) % 16) as usize;
                offset += 1;
                let end = core::cmp::min(offset + wid_len, data.len());
                let worker_id = String::from_utf8_lossy(&data[offset..end]).into_owned();
                offset = end;
                FuzzTransition::Claim { worker_id }
            }
            1 => FuzzTransition::BeginExecution,
            2 => FuzzTransition::Resolve,
            3 => {
                let msg_len = (data.get(offset).copied().unwrap_or(0) % 32) as usize;
                offset += 1;
                let end = core::cmp::min(offset + msg_len, data.len());
                let error_msg = String::from_utf8_lossy(&data[offset..end]).into_owned();
                offset = end;
                FuzzTransition::Retry { error_msg }
            }
            4 => {
                let reason_len = (data.get(offset).copied().unwrap_or(0) % 32) as usize;
                offset += 1;
                let end = core::cmp::min(offset + reason_len, data.len());
                let reason = String::from_utf8_lossy(&data[offset..end]).into_owned();
                offset = end;
                FuzzTransition::Supersede { reason }
            }
            _ => FuzzTransition::ResetToPending,
        };

        // Apply the transition. Invalid transitions should simply be
        // skipped — the state machine must never panic.
        state = apply_transition(state, &transition);

        // Calling is_terminal on every state must never panic.
        let _ = state.is_terminal();

        // If we reached a terminal state, no further transitions are valid.
        if state.is_terminal() {
            break;
        }
    }

    // Serialize the final state — must never panic.
    let _ = serde_json::to_string(&state);
});

/// Apply a transition to the current state, returning the new state.
///
/// This simulates the state machine logic. Invalid transitions return the
/// current state unchanged (the real pipeline would return an error).
fn apply_transition(current: EffectIntentState, transition: &FuzzTransition) -> EffectIntentState {
    match (&current, transition) {
        // Pending → Claimed
        (EffectIntentState::Pending, FuzzTransition::Claim { worker_id }) => {
            EffectIntentState::Claimed {
                worker_id: worker_id.clone(),
                lease_expires: Utc::now(),
            }
        }

        // Claimed → Executing
        (EffectIntentState::Claimed { worker_id, .. }, FuzzTransition::BeginExecution) => {
            EffectIntentState::Executing {
                worker_id: worker_id.clone(),
                started_at: Utc::now(),
            }
        }

        // Executing → Resolved (terminal)
        (EffectIntentState::Executing { .. }, FuzzTransition::Resolve) => {
            EffectIntentState::Resolved {
                outcome_id: EffectOutcomeId::new(),
            }
        }

        // Executing → Retrying
        (EffectIntentState::Executing { .. }, FuzzTransition::Retry { error_msg }) => {
            EffectIntentState::Retrying {
                next_attempt_after: Utc::now(),
                last_error: error_msg.clone(),
            }
        }

        // Any non-terminal → Superseded (terminal)
        (s, FuzzTransition::Supersede { reason }) if !s.is_terminal() => {
            EffectIntentState::Superseded {
                reason: reason.clone(),
            }
        }

        // Retrying → back to Pending (retry cycle)
        (EffectIntentState::Retrying { .. }, FuzzTransition::ResetToPending) => {
            EffectIntentState::Pending
        }

        // Invalid transition — no change.
        _ => current,
    }
}
