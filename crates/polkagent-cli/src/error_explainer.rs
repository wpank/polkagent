//! Error explainer for failed runs (PRD-13 §12.1, deliverable 3.7).
//!
//! Classifies a free-form failure reason string into a known [`ErrorCategory`],
//! then produces a structured [`ErrorExplanation`] with a human-readable
//! description of what happened, what is safe, and actionable next steps.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Timeout,
    ProviderError,
    PolicyDenial,
    ChainError,
    BudgetExceeded,
    AuthError,
    HarnessError,
    Unknown,
}

impl ErrorCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Timeout => "Timeout",
            Self::ProviderError => "Provider Error",
            Self::PolicyDenial => "Policy Denial",
            Self::ChainError => "Chain Error",
            Self::BudgetExceeded => "Budget Exceeded",
            Self::AuthError => "Authentication Error",
            Self::HarnessError => "Harness Error",
            Self::Unknown => "Error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ErrorExplanation {
    pub category: ErrorCategory,
    pub what_happened: &'static str,
    pub what_is_safe: &'static str,
    pub next_steps: &'static [&'static str],
}

impl ErrorExplanation {
    pub fn next_step_summary(&self) -> &'static str {
        self.next_steps
            .first()
            .copied()
            .unwrap_or("Inspect logs for details")
    }
}

/// Classify a failure reason string and produce a structured explanation.
pub fn explain(reason: &str) -> ErrorExplanation {
    let lower = reason.to_lowercase();
    let category = classify(&lower);

    match category {
        ErrorCategory::Timeout => ErrorExplanation {
            category,
            what_happened: "The run exceeded its deadline and was terminated.",
            what_is_safe:
                "All effects committed before the timeout are persisted. No partial writes.",
            next_steps: &[
                "Retry with a longer deadline (--timeout flag or config)",
                "Reduce task scope to fit within the time budget",
                "Check if a provider was slow to respond",
            ],
        },
        ErrorCategory::ProviderError => ErrorExplanation {
            category,
            what_happened: "The LLM provider returned an error or was unreachable.",
            what_is_safe:
                "No model output was applied. Run state is unchanged from before this turn.",
            next_steps: &[
                "Check provider status page for outages",
                "Verify your API key is valid and has quota",
                "Retry the run — transient errors often resolve",
            ],
        },
        ErrorCategory::PolicyDenial => ErrorExplanation {
            category,
            what_happened: "A policy rule blocked the requested action.",
            what_is_safe: "The denied action was never executed. All prior state is intact.",
            next_steps: &[
                "Review the agent policy in polkagent.toml",
                "Adjust allowed actions or approval rules",
                "Grant the required permission and retry",
            ],
        },
        ErrorCategory::ChainError => ErrorExplanation {
            category,
            what_happened:
                "An on-chain operation failed (RPC error, submission rejected, or revert).",
            what_is_safe:
                "Reverted extrinsics consumed fees but had no state effect. Check balances.",
            next_steps: &[
                "Verify RPC endpoint connectivity",
                "Check account balance and nonce",
                "Inspect the extrinsic error in a block explorer",
            ],
        },
        ErrorCategory::BudgetExceeded => ErrorExplanation {
            category,
            what_happened: "The run exhausted its token or spend budget.",
            what_is_safe:
                "All completed turns and effects are persisted. No partial work was lost.",
            next_steps: &[
                "Increase the budget limit in agent config",
                "Reduce task complexity to use fewer tokens",
                "Review token usage in the run detail to find hotspots",
            ],
        },
        ErrorCategory::AuthError => ErrorExplanation {
            category,
            what_happened: "Authentication or authorization failed.",
            what_is_safe: "No actions were taken with invalid credentials. State is unchanged.",
            next_steps: &[
                "Check your API key with `polkagent auth status`",
                "Regenerate or rotate the key if expired",
                "Verify the key has the required scopes",
            ],
        },
        ErrorCategory::HarnessError => ErrorExplanation {
            category,
            what_happened: "The execution harness encountered an internal error.",
            what_is_safe: "The run was stopped before any inconsistent state could be written.",
            next_steps: &[
                "Check polkagent version and update if needed",
                "Run `polkagent doctor` to diagnose setup issues",
                "Report the error if it persists",
            ],
        },
        ErrorCategory::Unknown => ErrorExplanation {
            category,
            what_happened: "The run failed for an unrecognized reason.",
            what_is_safe: "Completed effects are persisted. The run can be inspected for details.",
            next_steps: &[
                "Inspect the full error in the run timeline",
                "Check logs with `polkagent logs`",
                "Retry the run if the error seems transient",
            ],
        },
    }
}

fn classify(lower: &str) -> ErrorCategory {
    // Timeout patterns
    if lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("deadline exceeded")
        || lower.contains("deadline")
    {
        return ErrorCategory::Timeout;
    }

    // Budget patterns
    if lower.contains("budget")
        || lower.contains("quota exceeded")
        || lower.contains("rate limit")
        || lower.contains("token limit")
    {
        return ErrorCategory::BudgetExceeded;
    }

    // Auth patterns
    if lower.contains("api key")
        || lower.contains("api_key")
        || lower.contains("unauthorized")
        || lower.contains("authentication")
        || lower.contains("auth failed")
        || lower.contains("invalid key")
        || lower.contains("403")
        || lower.contains("401")
    {
        return ErrorCategory::AuthError;
    }

    // Policy denial patterns (checked early — "policy denied" takes priority over chain keywords
    // that may appear in the description, e.g. "policy denied: sign_extrinsic not allowed").
    if lower.contains("policy")
        || lower.contains("denied")
        || lower.contains("not allowed")
        || lower.contains("forbidden")
        || lower.contains("permission")
        || lower.contains("approval denied")
    {
        return ErrorCategory::PolicyDenial;
    }

    // Chain error patterns (checked before provider to prioritise "rpc" over generic network terms)
    if lower.contains("chain")
        || lower.contains("rpc")
        || lower.contains("extrinsic")
        || lower.contains("revert")
        || lower.contains("substrate")
        || lower.contains("nonce")
        || lower.contains("insufficient balance")
        || lower.contains("dispatch error")
    {
        return ErrorCategory::ChainError;
    }

    // Provider error patterns
    if lower.contains("provider")
        || lower.contains("model error")
        || lower.contains("llm error")
        || lower.contains("500")
        || lower.contains("502")
        || lower.contains("503")
        || lower.contains("overloaded")
        || lower.contains("service unavailable")
        || lower.contains("connection refused")
        || lower.contains("unreachable")
    {
        return ErrorCategory::ProviderError;
    }

    // Harness error patterns
    if lower.contains("harness")
        || lower.contains("capability mismatch")
        || lower.contains("internal error")
        || lower.contains("serialization")
    {
        return ErrorCategory::HarnessError;
    }

    ErrorCategory::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_timeout() {
        let e = explain("run timed out");
        assert_eq!(e.category, ErrorCategory::Timeout);
    }

    #[test]
    fn classify_timeout_deadline() {
        let e = explain("deadline exceeded for run abc123");
        assert_eq!(e.category, ErrorCategory::Timeout);
    }

    #[test]
    fn classify_provider_503() {
        let e = explain("503 service unavailable from provider");
        assert_eq!(e.category, ErrorCategory::ProviderError);
    }

    #[test]
    fn classify_provider_connection() {
        let e = explain("connection refused to model endpoint");
        assert_eq!(e.category, ErrorCategory::ProviderError);
    }

    #[test]
    fn classify_policy_denial() {
        let e = explain("policy denied: sign_extrinsic not allowed");
        assert_eq!(e.category, ErrorCategory::PolicyDenial);
    }

    #[test]
    fn classify_approval_denied() {
        let e = explain("approval denied by operator");
        assert_eq!(e.category, ErrorCategory::PolicyDenial);
    }

    #[test]
    fn classify_chain_rpc() {
        let e = explain("rpc error: connection to node lost");
        assert_eq!(e.category, ErrorCategory::ChainError);
    }

    #[test]
    fn classify_chain_extrinsic() {
        let e = explain("extrinsic dispatch error: insufficient balance");
        assert_eq!(e.category, ErrorCategory::ChainError);
    }

    #[test]
    fn classify_budget_exceeded() {
        let e = explain("budget exceeded: token limit reached");
        assert_eq!(e.category, ErrorCategory::BudgetExceeded);
    }

    #[test]
    fn classify_auth_api_key() {
        let e = explain("invalid API key");
        assert_eq!(e.category, ErrorCategory::AuthError);
    }

    #[test]
    fn classify_auth_unauthorized() {
        let e = explain("401 unauthorized");
        assert_eq!(e.category, ErrorCategory::AuthError);
    }

    #[test]
    fn classify_harness() {
        let e = explain("harness capability mismatch");
        assert_eq!(e.category, ErrorCategory::HarnessError);
    }

    #[test]
    fn classify_unknown() {
        let e = explain("something completely unexpected happened");
        assert_eq!(e.category, ErrorCategory::Unknown);
    }

    #[test]
    fn explanation_has_next_steps() {
        let e = explain("run timed out");
        assert!(!e.next_steps.is_empty());
        assert!(!e.next_step_summary().is_empty());
    }

    #[test]
    fn explanation_fields_non_empty() {
        let e = explain("provider 503 error");
        assert!(!e.what_happened.is_empty());
        assert!(!e.what_is_safe.is_empty());
        assert!(!e.next_steps.is_empty());
    }

    #[test]
    fn classify_case_insensitive() {
        let e = explain("RPC Error: node unreachable");
        assert_eq!(e.category, ErrorCategory::ChainError);
    }

    #[test]
    fn cancelled_reason_maps_to_unknown() {
        let e = explain("cancelled: user requested stop");
        assert_eq!(e.category, ErrorCategory::Unknown);
    }
}
