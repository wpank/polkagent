//! Domain types for treasury and portfolio analysis.
//!
//! These structs model the on-chain financial state of an account: balances,
//! staking positions, portfolio entries, transfer history, and vesting
//! schedules. All types derive `Serialize` / `Deserialize` for JSON output
//! from tool handlers.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// AccountBalance
// ---------------------------------------------------------------------------

/// Balance breakdown for a single asset held by an account.
///
/// Mirrors the `frame_system::AccountInfo` / `pallet_balances` storage
/// layout: free + reserved + frozen = total, with optional asset metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountBalance {
    /// Transferable balance (not locked, reserved, or frozen).
    pub free: u128,

    /// Balance reserved by the runtime (e.g. for identity deposits, proxies).
    pub reserved: u128,

    /// Balance frozen by locks (staking, vesting, democracy).
    ///
    /// Frozen funds overlap with free — they are still "owned" but not
    /// transferable until the lock expires.
    pub frozen: u128,

    /// Total balance: `free + reserved`.
    pub total: u128,

    /// Asset identifier. `None` for the chain's native token.
    pub asset_id: Option<u32>,

    /// Human-readable ticker symbol (e.g. `"DOT"`, `"KSM"`).
    pub symbol: String,

    /// Number of decimal places for display formatting.
    pub decimals: u8,
}

// ---------------------------------------------------------------------------
// StakingPosition
// ---------------------------------------------------------------------------

/// The staking role of an account on the relay chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StakingRole {
    /// The account is an active or waiting validator.
    Validator,
    /// The account is a nominator backing one or more validators.
    Nominator,
    /// The account has bonded funds but is neither validating nor nominating.
    Idle,
}

/// A snapshot of an account's staking position on the relay chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StakingPosition {
    /// Whether the account is validating, nominating, or idle.
    pub role: StakingRole,

    /// Actively staked amount (participating in the current era).
    pub active_stake: u128,

    /// Total bonded amount (active + any chunks being unlocked).
    pub total_stake: u128,

    /// Amounts currently in the unbonding queue, each with the era at which
    /// they become withdrawable.
    pub unlocking: Vec<UnlockingChunk>,

    /// Where staking rewards are deposited.
    pub reward_destination: String,

    /// For nominators: the list of validator stash addresses being nominated.
    /// For validators: empty.
    pub nominations: Vec<String>,
}

/// A single chunk of stake that is being unbonded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnlockingChunk {
    /// Amount being unbonded (in planck).
    pub value: u128,
    /// The era index at which this chunk becomes withdrawable.
    pub era: u32,
}

// ---------------------------------------------------------------------------
// PortfolioEntry
// ---------------------------------------------------------------------------

/// The source category of a portfolio line item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortfolioSource {
    /// Free native token balance.
    Native,
    /// Actively staked or bonded balance.
    Staking,
    /// Balance locked under a vesting schedule.
    Vesting,
    /// Balance contributed to a parachain crowdloan.
    Crowdloan,
}

/// A single line item in an account's aggregated portfolio view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortfolioEntry {
    /// Asset name or symbol (e.g. `"DOT"`, `"KSM"`).
    pub asset: String,

    /// Balance in the smallest denomination (planck).
    pub balance: u128,

    /// Estimated USD value, if a price feed is available.
    pub value_usd: Option<f64>,

    /// Where this balance originates.
    pub source: PortfolioSource,
}

// ---------------------------------------------------------------------------
// Transfer
// ---------------------------------------------------------------------------

/// A single transfer event observed on-chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    /// Sender SS58 address.
    pub from: String,

    /// Recipient SS58 address.
    pub to: String,

    /// Transfer amount in planck.
    pub amount: u128,

    /// Asset symbol (e.g. `"DOT"`).
    pub asset: String,

    /// Block number in which the transfer was included.
    pub block_number: u64,

    /// ISO-8601 timestamp of the block, if known.
    pub timestamp: Option<String>,

    /// Hex-encoded extrinsic hash for on-chain lookup.
    pub extrinsic_hash: Option<String>,
}

// ---------------------------------------------------------------------------
// VestingInfo
// ---------------------------------------------------------------------------

/// A single vesting schedule attached to an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VestingSchedule {
    /// Total amount locked by this schedule (in planck).
    pub locked: u128,

    /// Amount released per block (in planck).
    pub per_block: u128,

    /// Block number at which vesting began.
    pub starting_block: u64,
}

/// Aggregated vesting state for an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VestingInfo {
    /// Individual vesting schedules (an account may have multiple).
    pub schedules: Vec<VestingSchedule>,

    /// Total amount that has already vested and is available.
    pub total_unlocked: u128,

    /// Total amount still locked across all schedules.
    pub total_locked: u128,

    /// The block number at which the next vesting unlock occurs, if any.
    pub next_unlock_block: Option<u64>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
// Test assertions deliberately unwrap fixtures so failures retain precise context.
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn account_balance_serde_round_trip() {
        let balance = AccountBalance {
            free: 1_000_000_000_000,
            reserved: 500_000_000,
            frozen: 200_000_000_000,
            total: 1_000_500_000_000,
            asset_id: None,
            symbol: "DOT".to_string(),
            decimals: 10,
        };

        let json = serde_json::to_string(&balance).expect("serialize");
        let back: AccountBalance = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, balance);
    }

    #[test]
    fn account_balance_with_asset_id() {
        let balance = AccountBalance {
            free: 5000,
            reserved: 0,
            frozen: 0,
            total: 5000,
            asset_id: Some(1984),
            symbol: "USDT".to_string(),
            decimals: 6,
        };

        let json = serde_json::to_string(&balance).expect("serialize");
        assert!(json.contains("1984"));
        assert!(json.contains("USDT"));
    }

    #[test]
    fn staking_role_serializes_as_snake_case() {
        let val = serde_json::to_string(&StakingRole::Validator).expect("serialize");
        assert_eq!(val, r#""validator""#);

        let nom = serde_json::to_string(&StakingRole::Nominator).expect("serialize");
        assert_eq!(nom, r#""nominator""#);

        let idle = serde_json::to_string(&StakingRole::Idle).expect("serialize");
        assert_eq!(idle, r#""idle""#);
    }

    #[test]
    fn staking_position_serde_round_trip() {
        let pos = StakingPosition {
            role: StakingRole::Nominator,
            active_stake: 10_000_000_000_000,
            total_stake: 12_000_000_000_000,
            unlocking: vec![UnlockingChunk {
                value: 2_000_000_000_000,
                era: 1234,
            }],
            reward_destination: "Staked".to_string(),
            nominations: vec!["5GrwvaEF...".to_string()],
        };

        let json = serde_json::to_string(&pos).expect("serialize");
        let back: StakingPosition = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, pos);
    }

    #[test]
    fn unlocking_chunk_fields() {
        let chunk = UnlockingChunk {
            value: 500_000_000_000,
            era: 999,
        };
        let json = serde_json::to_string(&chunk).expect("serialize");
        assert!(json.contains("999"));
    }

    #[test]
    fn portfolio_source_serializes_as_snake_case() {
        let native = serde_json::to_string(&PortfolioSource::Native).expect("serialize");
        assert_eq!(native, r#""native""#);

        let staking = serde_json::to_string(&PortfolioSource::Staking).expect("serialize");
        assert_eq!(staking, r#""staking""#);

        let vesting = serde_json::to_string(&PortfolioSource::Vesting).expect("serialize");
        assert_eq!(vesting, r#""vesting""#);

        let crowdloan = serde_json::to_string(&PortfolioSource::Crowdloan).expect("serialize");
        assert_eq!(crowdloan, r#""crowdloan""#);
    }

    #[test]
    fn portfolio_entry_with_usd_value() {
        let entry = PortfolioEntry {
            asset: "DOT".to_string(),
            balance: 100_000_000_000_000,
            value_usd: Some(750.50),
            source: PortfolioSource::Native,
        };

        let json = serde_json::to_string(&entry).expect("serialize");
        assert!(json.contains("750.5"));
    }

    #[test]
    fn portfolio_entry_without_usd_value() {
        let entry = PortfolioEntry {
            asset: "KSM".to_string(),
            balance: 1_000_000_000_000,
            value_usd: None,
            source: PortfolioSource::Staking,
        };

        let json = serde_json::to_string(&entry).expect("serialize");
        assert!(json.contains("null"));
    }

    #[test]
    fn transfer_serde_round_trip() {
        let transfer = Transfer {
            from: "5GrwvaEFcWWqn1bRJJpPi8HBdhQ".to_string(),
            to: "5FHneW46xGXgs5mUiveU4sbTyGBzmst".to_string(),
            amount: 1_000_000_000_000,
            asset: "DOT".to_string(),
            block_number: 18_500_000,
            timestamp: Some("2024-06-15T12:30:00Z".to_string()),
            extrinsic_hash: Some("0xabcdef1234567890".to_string()),
        };

        let json = serde_json::to_string(&transfer).expect("serialize");
        let back: Transfer = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, transfer);
    }

    #[test]
    fn transfer_optional_fields_none() {
        let transfer = Transfer {
            from: "5Abc".to_string(),
            to: "5Def".to_string(),
            amount: 100,
            asset: "KSM".to_string(),
            block_number: 1000,
            timestamp: None,
            extrinsic_hash: None,
        };

        let json = serde_json::to_string(&transfer).expect("serialize");
        let back: Transfer = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.timestamp, None);
        assert_eq!(back.extrinsic_hash, None);
    }

    #[test]
    fn vesting_schedule_serde_round_trip() {
        let schedule = VestingSchedule {
            locked: 10_000_000_000_000,
            per_block: 1_000_000,
            starting_block: 15_000_000,
        };

        let json = serde_json::to_string(&schedule).expect("serialize");
        let back: VestingSchedule = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, schedule);
    }

    #[test]
    fn vesting_info_with_schedules() {
        let info = VestingInfo {
            schedules: vec![
                VestingSchedule {
                    locked: 5_000_000_000_000,
                    per_block: 500_000,
                    starting_block: 10_000_000,
                },
                VestingSchedule {
                    locked: 3_000_000_000_000,
                    per_block: 300_000,
                    starting_block: 12_000_000,
                },
            ],
            total_unlocked: 2_000_000_000_000,
            total_locked: 6_000_000_000_000,
            next_unlock_block: Some(18_000_100),
        };

        let json = serde_json::to_string(&info).expect("serialize");
        let back: VestingInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.schedules.len(), 2);
        assert_eq!(back.total_unlocked, 2_000_000_000_000);
        assert_eq!(back.next_unlock_block, Some(18_000_100));
    }

    #[test]
    fn vesting_info_no_schedules() {
        let info = VestingInfo {
            schedules: vec![],
            total_unlocked: 0,
            total_locked: 0,
            next_unlock_block: None,
        };

        let json = serde_json::to_string(&info).expect("serialize");
        let back: VestingInfo = serde_json::from_str(&json).expect("deserialize");
        assert!(back.schedules.is_empty());
        assert_eq!(back.next_unlock_block, None);
    }
}
