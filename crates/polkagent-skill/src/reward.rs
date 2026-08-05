//! **EXPERIMENTAL** — Skill reward definitions for PRD-08 deliverable 5.2.
//!
//! Defines reward values that an agent receives upon completing a skill
//! task. Rewards are simulated micropayments credited to the agent's
//! payment ledger (see `polkagent-payment::ledger`).
//!
//! # Status
//!
//! **Research prototype — not production-ready.**

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// SkillReward
// ---------------------------------------------------------------------------

/// A reward earned by an agent for completing a skill task.
///
/// **EXPERIMENTAL**: This is a simulated micropayment for prototyping
/// agent earn/spend mechanics. Reward amounts are denominated in the
/// smallest unit of the specified asset (e.g. planck for DOT).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillReward {
    /// The skill that was completed.
    pub skill_name: String,
    /// Reward amount in the smallest denomination (e.g. planck).
    pub amount_planck: u128,
    /// Asset identifier (e.g. `"NATIVE"`, `"USDT"`).
    pub asset: String,
    /// Human-readable description of the reward.
    pub description: String,
}

impl SkillReward {
    /// Create a new skill reward.
    #[must_use]
    pub fn new(
        skill_name: impl Into<String>,
        amount_planck: u128,
        asset: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            skill_name: skill_name.into(),
            amount_planck,
            asset: asset.into(),
            description: description.into(),
        }
    }

    /// Create a reward denominated in the native chain token.
    #[must_use]
    pub fn native(
        skill_name: impl Into<String>,
        amount_planck: u128,
        description: impl Into<String>,
    ) -> Self {
        Self::new(skill_name, amount_planck, "NATIVE", description)
    }
}

// ---------------------------------------------------------------------------
// RewardPolicy
// ---------------------------------------------------------------------------

/// A policy that maps skill names to reward amounts.
///
/// **EXPERIMENTAL**: This is a simple lookup table for prototyping.
/// A production system would likely integrate with on-chain governance
/// or a more sophisticated pricing model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewardPolicy {
    entries: Vec<RewardPolicyEntry>,
    /// Default reward for skills not listed in the policy.
    pub default_amount_planck: u128,
    /// Default asset for rewards.
    pub default_asset: String,
}

/// A single entry in a reward policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewardPolicyEntry {
    /// Skill name (exact match for prototype).
    pub skill_name: String,
    /// Reward amount in the smallest denomination.
    pub amount_planck: u128,
    /// Asset identifier.
    pub asset: String,
}

impl RewardPolicy {
    /// Create a policy with a default reward for unlisted skills.
    #[must_use]
    pub fn new(default_amount_planck: u128, default_asset: impl Into<String>) -> Self {
        Self {
            entries: Vec::new(),
            default_amount_planck,
            default_asset: default_asset.into(),
        }
    }

    /// Add a reward entry for a specific skill.
    pub fn add_entry(
        &mut self,
        skill_name: impl Into<String>,
        amount_planck: u128,
        asset: impl Into<String>,
    ) {
        self.entries.push(RewardPolicyEntry {
            skill_name: skill_name.into(),
            amount_planck,
            asset: asset.into(),
        });
    }

    /// Look up the reward for a completed skill.
    ///
    /// Returns the specific entry if one exists, otherwise falls back to
    /// the default reward amount and asset.
    #[must_use]
    pub fn reward_for(&self, skill_name: &str) -> SkillReward {
        let (amount, asset) = self
            .entries
            .iter()
            .find(|e| e.skill_name == skill_name)
            .map(|e| (e.amount_planck, e.asset.clone()))
            .unwrap_or((self.default_amount_planck, self.default_asset.clone()));

        SkillReward {
            skill_name: skill_name.to_owned(),
            amount_planck: amount,
            asset,
            description: format!("reward for completing '{skill_name}'"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Test fixtures use explicit panic boundaries to identify reward serialization
// invariant failures.
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "unit-test skill reward assertions intentionally panic with focused diagnostics"
)]
mod tests {
    use super::*;

    #[test]
    fn skill_reward_new() {
        let r = SkillReward::new("my-skill", 1000, "NATIVE", "test reward");
        assert_eq!(r.skill_name, "my-skill");
        assert_eq!(r.amount_planck, 1000);
        assert_eq!(r.asset, "NATIVE");
        assert_eq!(r.description, "test reward");
    }

    #[test]
    fn skill_reward_native() {
        let r = SkillReward::native("my-skill", 500, "half a reward");
        assert_eq!(r.asset, "NATIVE");
        assert_eq!(r.amount_planck, 500);
    }

    #[test]
    fn reward_policy_default() {
        let policy = RewardPolicy::new(1000, "NATIVE");
        let r = policy.reward_for("unknown-skill");
        assert_eq!(r.amount_planck, 1000);
        assert_eq!(r.asset, "NATIVE");
    }

    #[test]
    fn reward_policy_specific_entry() {
        let mut policy = RewardPolicy::new(1000, "NATIVE");
        policy.add_entry("premium-skill", 5000, "NATIVE");

        let r1 = policy.reward_for("premium-skill");
        assert_eq!(r1.amount_planck, 5000);

        let r2 = policy.reward_for("other-skill");
        assert_eq!(r2.amount_planck, 1000);
    }

    #[test]
    fn skill_reward_serde_round_trip() {
        let r = SkillReward::new("test", 42, "DOT", "round trip");
        let json = serde_json::to_string(&r).expect("serialize");
        let back: SkillReward = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(r, back);
    }

    #[test]
    fn reward_policy_serde_round_trip() {
        let mut policy = RewardPolicy::new(100, "NATIVE");
        policy.add_entry("special", 500, "USDT");
        let json = serde_json::to_string(&policy).expect("serialize");
        let back: RewardPolicy = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.default_amount_planck, 100);
        assert_eq!(back.reward_for("special").amount_planck, 500);
    }
}
