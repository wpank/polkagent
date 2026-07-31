//! SS58 address encoding and decoding.
//!
//! SS58 is the address format used by Substrate-based chains (Polkadot, Kusama,
//! etc.). An SS58 address encodes a network prefix, 32-byte account ID, and a
//! 2-byte checksum into a Base58 string.
//!
//! # Checksum note
//!
//! The canonical SS58 specification uses Blake2b-512 for the checksum. This
//! implementation uses Blake3 as a simplified substitute since the workspace
//! does not include a Blake2b dependency. Production deployments should use
//! Blake2b-512 to ensure interoperability with other Substrate tooling.

use base58::{FromBase58, ToBase58};

use crate::error::IdentityError;

/// The SS58 context prefix used before hashing: `b"SS58PRE"`.
const SS58_PREFIX: &[u8] = b"SS58PRE";

/// Encode a 32-byte account ID with the given network prefix into an SS58
/// address string.
///
/// Handles both simple prefixes (0..=63, encoded as a single byte) and
/// "canary" prefixes (64..=16383, encoded as two bytes per the SS58 spec).
pub fn encode_ss58(prefix: u16, data: &[u8; 32]) -> String {
    let prefix_bytes = encode_prefix(prefix);
    let checksum = compute_checksum(&prefix_bytes, data);

    let mut payload = Vec::with_capacity(prefix_bytes.len() + 32 + 2);
    payload.extend_from_slice(&prefix_bytes);
    payload.extend_from_slice(data);
    payload.extend_from_slice(&checksum[..2]);

    payload.to_base58()
}

/// Decode an SS58 address string, returning the network prefix and 32-byte
/// account ID.
///
/// Returns an error if the address is malformed, has an invalid checksum, or
/// contains an unsupported prefix encoding.
pub fn decode_ss58(address: &str) -> Result<(u16, [u8; 32]), IdentityError> {
    let decoded = address
        .from_base58()
        .map_err(|_| IdentityError::InvalidSS58Address(address.to_string()))?;

    // Determine prefix length: single byte (0..=63) or two bytes (64..=16383).
    if decoded.is_empty() {
        return Err(IdentityError::InvalidSS58Address(
            "empty address".to_string(),
        ));
    }

    let (prefix, prefix_len) = decode_prefix(&decoded)?;

    // After the prefix we need exactly 32 bytes of account data + 2 bytes of
    // checksum.
    let expected_len = prefix_len + 32 + 2;
    if decoded.len() != expected_len {
        return Err(IdentityError::InvalidSS58Address(format!(
            "expected {} bytes, got {}",
            expected_len,
            decoded.len()
        )));
    }

    let account_start = prefix_len;
    let account_end = account_start + 32;
    let mut account = [0u8; 32];
    account.copy_from_slice(&decoded[account_start..account_end]);

    let checksum_bytes = &decoded[account_end..account_end + 2];
    let prefix_bytes = &decoded[..prefix_len];
    let expected_checksum = compute_checksum(prefix_bytes, &account);

    if checksum_bytes != &expected_checksum[..2] {
        return Err(IdentityError::InvalidChecksum);
    }

    Ok((prefix, account))
}

/// Encode a network prefix into its byte representation per the SS58 spec.
///
/// - Prefixes 0..=63 use a single byte.
/// - Prefixes 64..=16383 use two bytes with a special encoding:
///   the first byte has bits `01XXXXXX` and the second byte holds the
///   remaining bits.
fn encode_prefix(prefix: u16) -> Vec<u8> {
    if prefix < 64 {
        vec![prefix as u8]
    } else {
        // Two-byte encoding per SS58 spec:
        // first  = ((prefix & 0b0000_0000_1111_1100) >> 2) | 0b0100_0000
        // second = (prefix >> 8) | ((prefix & 0b0000_0000_0000_0011) << 6)
        let first = ((prefix & 0xFC) >> 2) | 0x40;
        let second = (prefix >> 8) | ((prefix & 0x03) << 6);
        vec![first as u8, second as u8]
    }
}

/// Decode a network prefix from the raw decoded bytes.
///
/// Returns `(prefix_value, prefix_byte_length)`.
fn decode_prefix(data: &[u8]) -> Result<(u16, usize), IdentityError> {
    let first = data[0];

    if first < 64 {
        Ok((u16::from(first), 1))
    } else if first < 128 {
        // Two-byte prefix
        if data.len() < 2 {
            return Err(IdentityError::InvalidSS58Address(
                "truncated two-byte prefix".to_string(),
            ));
        }
        let second = data[1];
        // Reverse the encoding:
        // prefix = ((first & 0b0011_1111) << 2) | (second >> 6)
        //        | ((second & 0b0011_1111) << 8)
        let lower = u16::from(first & 0x3F) << 2 | u16::from(second >> 6);
        let upper = u16::from(second & 0x3F) << 8;
        let prefix = lower | upper;
        Ok((prefix, 2))
    } else {
        Err(IdentityError::InvalidPrefix(u16::from(first)))
    }
}

/// Compute the SS58 checksum for the given prefix bytes and account data.
///
/// Uses Blake3 as a simplified substitute for the canonical Blake2b-512.
/// The checksum is the first 2 bytes of `Blake3(SS58_PREFIX || prefix || account)`.
fn compute_checksum(prefix_bytes: &[u8], account: &[u8; 32]) -> [u8; 2] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SS58_PREFIX);
    hasher.update(prefix_bytes);
    hasher.update(account);
    let hash = hasher.finalize();
    let bytes = hash.as_bytes();
    [bytes[0], bytes[1]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_simple_prefix() {
        let account = [42u8; 32];
        let prefix = 0u16; // Polkadot

        let encoded = encode_ss58(prefix, &account);
        let (decoded_prefix, decoded_account) =
            decode_ss58(&encoded).expect("decode should succeed");

        assert_eq!(decoded_prefix, prefix);
        assert_eq!(decoded_account, account);
    }

    #[test]
    fn round_trip_kusama_prefix() {
        let account = [7u8; 32];
        let prefix = 2u16; // Kusama

        let encoded = encode_ss58(prefix, &account);
        let (decoded_prefix, decoded_account) =
            decode_ss58(&encoded).expect("decode should succeed");

        assert_eq!(decoded_prefix, prefix);
        assert_eq!(decoded_account, account);
    }

    #[test]
    fn round_trip_westend_prefix() {
        let account = [99u8; 32];
        let prefix = 42u16; // Westend / generic substrate

        let encoded = encode_ss58(prefix, &account);
        let (decoded_prefix, decoded_account) =
            decode_ss58(&encoded).expect("decode should succeed");

        assert_eq!(decoded_prefix, prefix);
        assert_eq!(decoded_account, account);
    }

    #[test]
    fn round_trip_two_byte_prefix() {
        let account = [0xAB; 32];
        let prefix = 252u16; // Two-byte prefix range (>= 64)

        let encoded = encode_ss58(prefix, &account);
        let (decoded_prefix, decoded_account) =
            decode_ss58(&encoded).expect("decode should succeed");

        assert_eq!(decoded_prefix, prefix);
        assert_eq!(decoded_account, account);
    }

    #[test]
    fn round_trip_high_two_byte_prefix() {
        let account = [0xCD; 32];
        let prefix = 4096u16;

        let encoded = encode_ss58(prefix, &account);
        let (decoded_prefix, decoded_account) =
            decode_ss58(&encoded).expect("decode should succeed");

        assert_eq!(decoded_prefix, prefix);
        assert_eq!(decoded_account, account);
    }

    #[test]
    fn invalid_base58_characters() {
        let result = decode_ss58("0OIl"); // characters not in Base58 alphabet
        assert!(result.is_err());
    }

    #[test]
    fn empty_address_fails() {
        let result = decode_ss58("");
        assert!(result.is_err());
    }

    #[test]
    fn corrupted_checksum_fails() {
        let account = [1u8; 32];
        let encoded = encode_ss58(0, &account);

        // Corrupt the last character
        let mut chars: Vec<char> = encoded.chars().collect();
        let last = chars.len() - 1;
        chars[last] = if chars[last] == 'A' { 'B' } else { 'A' };
        let corrupted: String = chars.into_iter().collect();

        let result = decode_ss58(&corrupted);
        assert!(
            result.is_err(),
            "corrupted checksum should produce an error"
        );
    }

    #[test]
    fn prefix_encoding_single_byte_range() {
        for p in 0..64u16 {
            let bytes = encode_prefix(p);
            assert_eq!(bytes.len(), 1, "prefix {p} should be 1 byte");
            let (decoded, len) = decode_prefix(&bytes).expect("decode prefix");
            assert_eq!(len, 1);
            assert_eq!(decoded, p);
        }
    }

    #[test]
    fn prefix_encoding_two_byte_range() {
        // Test a selection of two-byte prefixes
        for &p in &[64u16, 100, 255, 1000, 4096, 16383] {
            let bytes = encode_prefix(p);
            assert_eq!(bytes.len(), 2, "prefix {p} should be 2 bytes");
            let (decoded, len) = decode_prefix(&bytes).expect("decode prefix");
            assert_eq!(len, 2);
            assert_eq!(decoded, p, "prefix round-trip failed for {p}");
        }
    }

    #[test]
    fn all_zero_account() {
        let account = [0u8; 32];
        let encoded = encode_ss58(0, &account);
        let (prefix, decoded) = decode_ss58(&encoded).expect("decode");
        assert_eq!(prefix, 0);
        assert_eq!(decoded, account);
    }

    #[test]
    fn all_ff_account() {
        let account = [0xFF; 32];
        let encoded = encode_ss58(0, &account);
        let (prefix, decoded) = decode_ss58(&encoded).expect("decode");
        assert_eq!(prefix, 0);
        assert_eq!(decoded, account);
    }

    #[test]
    fn different_prefixes_produce_different_addresses() {
        let account = [42u8; 32];
        let polkadot = encode_ss58(0, &account);
        let kusama = encode_ss58(2, &account);
        let westend = encode_ss58(42, &account);

        assert_ne!(polkadot, kusama);
        assert_ne!(polkadot, westend);
        assert_ne!(kusama, westend);
    }
}
