//! Identity types for Polkadot chain integration.
//!
//! This module defines the core identity primitives used to represent accounts
//! on Substrate-based chains: [`AccountId32`], [`NetworkId`], [`SS58Address`],
//! and [`ChainAccount`].

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::IdentityError;
use crate::ss58;

// ---------------------------------------------------------------------------
// AccountId32
// ---------------------------------------------------------------------------

/// A 32-byte account identifier, equivalent to Polkadot's `AccountId`.
///
/// Displayed and serialized as a lowercase hex string (64 characters).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AccountId32([u8; 32]);

impl AccountId32 {
    /// Create an `AccountId32` from a raw 32-byte array.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return the underlying 32-byte array.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Parse a hex-encoded string (with or without `0x` prefix) into an
    /// `AccountId32`.
    pub fn from_hex(hex: &str) -> Result<Self, IdentityError> {
        let hex = hex.strip_prefix("0x").unwrap_or(hex);
        if hex.len() != 64 {
            return Err(IdentityError::InvalidHex(format!(
                "expected 64 hex characters, got {}",
                hex.len()
            )));
        }
        let mut bytes = [0u8; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|e| {
                IdentityError::InvalidHex(format!("invalid hex at position {}: {e}", i * 2))
            })?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Debug for AccountId32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AccountId32(0x{})", hex_encode(&self.0))
    }
}

impl fmt::Display for AccountId32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{}", hex_encode(&self.0))
    }
}

impl Serialize for AccountId32 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let hex = format!("0x{}", hex_encode(&self.0));
        serializer.serialize_str(&hex)
    }
}

impl<'de> Deserialize<'de> for AccountId32 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// NetworkId
// ---------------------------------------------------------------------------

/// Identifies a Substrate-based network by its SS58 address prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NetworkId {
    /// Polkadot relay chain (prefix 0).
    Polkadot,
    /// Kusama relay chain (prefix 2).
    Kusama,
    /// Westend test network (prefix 42).
    Westend,
    /// Any other network identified by its numeric prefix.
    Generic(u16),
}

impl NetworkId {
    /// Return the SS58 address prefix for this network.
    #[must_use]
    pub fn prefix(&self) -> u16 {
        match self {
            Self::Polkadot => 0,
            Self::Kusama => 2,
            Self::Westend => 42,
            Self::Generic(p) => *p,
        }
    }

    /// Return a human-readable name for this network.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Polkadot => "Polkadot",
            Self::Kusama => "Kusama",
            Self::Westend => "Westend",
            Self::Generic(_) => "Generic",
        }
    }

    /// Construct a `NetworkId` from a numeric SS58 prefix.
    #[must_use]
    pub fn from_prefix(prefix: u16) -> Self {
        match prefix {
            0 => Self::Polkadot,
            2 => Self::Kusama,
            42 => Self::Westend,
            p => Self::Generic(p),
        }
    }
}

impl fmt::Display for NetworkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Generic(p) => write!(f, "Generic({p})"),
            _ => write!(f, "{}", self.name()),
        }
    }
}

// ---------------------------------------------------------------------------
// SS58Address
// ---------------------------------------------------------------------------

/// An SS58-encoded address string.
///
/// This is a thin wrapper around a `String` that represents a validated SS58
/// address. Use [`encode`](SS58Address::encode) to create one from an
/// [`AccountId32`] and [`NetworkId`], or [`decode`](SS58Address::decode) to
/// parse an existing address string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SS58Address(String);

impl SS58Address {
    /// Encode an account ID for the given network into an SS58 address.
    #[must_use]
    pub fn encode(account: &AccountId32, network: NetworkId) -> Self {
        let encoded = ss58::encode_ss58(network.prefix(), &account.0);
        Self(encoded)
    }

    /// Decode an SS58 address string into an [`AccountId32`] and [`NetworkId`].
    pub fn decode(address: &str) -> Result<(AccountId32, NetworkId), IdentityError> {
        let (prefix, account_bytes) = ss58::decode_ss58(address)?;
        let account = AccountId32(account_bytes);
        let network = NetworkId::from_prefix(prefix);
        Ok((account, network))
    }

    /// Return the inner SS58 string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SS58Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for SS58Address {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// ChainAccount
// ---------------------------------------------------------------------------

/// An account on a specific chain, with an optional human-readable label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainAccount {
    /// The 32-byte account identifier.
    pub account: AccountId32,
    /// The network this account belongs to.
    pub network: NetworkId,
    /// An optional human-readable label (e.g., "staking-hot-wallet").
    pub label: Option<String>,
}

impl ChainAccount {
    /// Create a new `ChainAccount`.
    #[must_use]
    pub fn new(account: AccountId32, network: NetworkId) -> Self {
        Self {
            account,
            network,
            label: None,
        }
    }

    /// Set an optional label.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Encode this account as an SS58 address for its network.
    #[must_use]
    pub fn ss58_address(&self) -> SS58Address {
        SS58Address::encode(&self.account, self.network)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Encode bytes as a lowercase hex string (no `0x` prefix).
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_id32_from_bytes_round_trip() {
        let bytes = [0xABu8; 32];
        let id = AccountId32::from_bytes(bytes);
        assert_eq!(id.to_bytes(), bytes);
    }

    #[test]
    fn account_id32_display_hex() {
        let bytes = [0u8; 32];
        let id = AccountId32::from_bytes(bytes);
        let display = id.to_string();
        assert_eq!(
            display,
            "0x0000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn account_id32_from_hex_with_prefix() {
        let hex = "0x0101010101010101010101010101010101010101010101010101010101010101";
        let id = AccountId32::from_hex(hex).expect("valid hex");
        assert_eq!(id.to_bytes(), [1u8; 32]);
    }

    #[test]
    fn account_id32_from_hex_without_prefix() {
        let hex = "0202020202020202020202020202020202020202020202020202020202020202";
        let id = AccountId32::from_hex(hex).expect("valid hex");
        assert_eq!(id.to_bytes(), [2u8; 32]);
    }

    #[test]
    fn account_id32_from_hex_wrong_length() {
        let result = AccountId32::from_hex("0xABCD");
        assert!(result.is_err());
    }

    #[test]
    fn account_id32_serde_round_trip() {
        let id = AccountId32::from_bytes([0x42; 32]);
        let json = serde_json::to_string(&id).expect("serialize");
        let back: AccountId32 = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn account_id32_serde_is_hex_string() {
        let id = AccountId32::from_bytes([0; 32]);
        let json = serde_json::to_string(&id).expect("serialize");
        assert!(json.starts_with('"'));
        assert!(json.contains("0x"));
        assert!(json.ends_with('"'));
    }

    #[test]
    fn network_id_prefix() {
        assert_eq!(NetworkId::Polkadot.prefix(), 0);
        assert_eq!(NetworkId::Kusama.prefix(), 2);
        assert_eq!(NetworkId::Westend.prefix(), 42);
        assert_eq!(NetworkId::Generic(100).prefix(), 100);
    }

    #[test]
    fn network_id_name() {
        assert_eq!(NetworkId::Polkadot.name(), "Polkadot");
        assert_eq!(NetworkId::Kusama.name(), "Kusama");
        assert_eq!(NetworkId::Westend.name(), "Westend");
        assert_eq!(NetworkId::Generic(100).name(), "Generic");
    }

    #[test]
    fn network_id_from_prefix() {
        assert_eq!(NetworkId::from_prefix(0), NetworkId::Polkadot);
        assert_eq!(NetworkId::from_prefix(2), NetworkId::Kusama);
        assert_eq!(NetworkId::from_prefix(42), NetworkId::Westend);
        assert_eq!(NetworkId::from_prefix(5), NetworkId::Generic(5));
    }

    #[test]
    fn ss58_encode_decode_polkadot() {
        let account = AccountId32::from_bytes([1u8; 32]);
        let addr = SS58Address::encode(&account, NetworkId::Polkadot);

        let (decoded_account, decoded_network) =
            SS58Address::decode(addr.as_str()).expect("decode should succeed");

        assert_eq!(decoded_account, account);
        assert_eq!(decoded_network, NetworkId::Polkadot);
    }

    #[test]
    fn ss58_encode_decode_kusama() {
        let account = AccountId32::from_bytes([7u8; 32]);
        let addr = SS58Address::encode(&account, NetworkId::Kusama);

        let (decoded_account, decoded_network) =
            SS58Address::decode(addr.as_str()).expect("decode should succeed");

        assert_eq!(decoded_account, account);
        assert_eq!(decoded_network, NetworkId::Kusama);
    }

    #[test]
    fn ss58_address_display() {
        let account = AccountId32::from_bytes([0xAA; 32]);
        let addr = SS58Address::encode(&account, NetworkId::Polkadot);
        let display = addr.to_string();
        // SS58 addresses are Base58, so the display should be non-empty and
        // not contain the raw hex.
        assert!(!display.is_empty());
    }

    #[test]
    fn ss58_address_invalid_string() {
        let result = SS58Address::decode("not-a-valid-address!!!");
        assert!(result.is_err());
    }

    #[test]
    fn chain_account_ss58() {
        let account = AccountId32::from_bytes([42u8; 32]);
        let chain = ChainAccount::new(account, NetworkId::Westend).with_label("test-wallet");

        assert_eq!(chain.label.as_deref(), Some("test-wallet"));

        let addr = chain.ss58_address();
        let (decoded, network) = SS58Address::decode(addr.as_str()).expect("decode");
        assert_eq!(decoded, account);
        assert_eq!(network, NetworkId::Westend);
    }

    #[test]
    fn chain_account_serde_round_trip() {
        let chain = ChainAccount::new(AccountId32::from_bytes([0xBB; 32]), NetworkId::Kusama)
            .with_label("my-wallet");

        let json = serde_json::to_string(&chain).expect("serialize");
        let back: ChainAccount = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(chain.account, back.account);
        assert_eq!(chain.network, back.network);
        assert_eq!(chain.label, back.label);
    }

    #[test]
    fn same_account_different_networks_different_ss58() {
        let account = AccountId32::from_bytes([0xCC; 32]);
        let polkadot = SS58Address::encode(&account, NetworkId::Polkadot);
        let kusama = SS58Address::encode(&account, NetworkId::Kusama);
        let westend = SS58Address::encode(&account, NetworkId::Westend);

        assert_ne!(polkadot.as_str(), kusama.as_str());
        assert_ne!(polkadot.as_str(), westend.as_str());
        assert_ne!(kusama.as_str(), westend.as_str());
    }
}
