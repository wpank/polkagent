//! Byte-level oracle for the opener emitted by the pinned PCA reference.
//!
//! This proves that the checked-in vector has the exact SCALE/P-256/HKDF/
//! AES-GCM framing recorded in its immutable provenance. It does not make the
//! bespoke `TcpPcaTransport` wire protocol PCA C0-compatible.

#![allow(
    clippy::expect_used,
    reason = "an immutable compatibility fixture should fail fast at the exact malformed field"
)]

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};
use hkdf::Hkdf;
use p256::ecdh::diffie_hellman;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use serde_json::Value;
use sha2::Sha256;

const FIXTURE_JSON: &str = include_str!("fixtures/pca_reference_opener_v2.json");
const REFERENCE_COMMIT: &str = "2adddcc8cfd732804cd9bbcbcd26974b44b47f66";
const REFERENCE_TREE: &str = "6b24bc2bab72e355bd86b810bd652914c37164ee";
const CODEC_BLOB: &str = "34512a48d7453c2e2bc6fe86778e18fc95d88e65";
const PROTOCOL_BLOB: &str = "79740a9df0d463921acbfb76b2a1f5d5cfac11fd";
const LOCK_BLOB: &str = "53781eb1669ccdf01a8b5c0d503f839a966805b3";

fn field<'a>(root: &'a Value, path: &[&str]) -> &'a str {
    let mut value = root;
    for component in path {
        value = value
            .get(component)
            .unwrap_or_else(|| panic!("missing fixture field {}", path.join(".")));
    }
    value
        .as_str()
        .unwrap_or_else(|| panic!("fixture field {} is not a string", path.join(".")))
}

fn decode_hex(value: &str) -> Vec<u8> {
    assert!(
        value.len().is_multiple_of(2),
        "hex must contain complete bytes"
    );
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| (nibble(pair[0]) << 4) | nibble(pair[1]))
        .collect()
}

fn nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => panic!("fixture contains non-hex byte"),
    }
}

fn decode_compact(input: &[u8], offset: &mut usize) -> usize {
    let first = *input.get(*offset).expect("SCALE compact prefix");
    *offset += 1;
    match first & 3 {
        0 => usize::from(first >> 2),
        1 => {
            let second = *input.get(*offset).expect("two-byte SCALE compact value");
            *offset += 1;
            usize::from((u16::from(first) | (u16::from(second) << 8)) >> 2)
        }
        2 => {
            let rest: [u8; 3] = input
                .get(*offset..*offset + 3)
                .expect("four-byte SCALE compact value")
                .try_into()
                .expect("three-byte compact suffix");
            *offset += 3;
            usize::try_from(
                (u32::from(first)
                    | (u32::from(rest[0]) << 8)
                    | (u32::from(rest[1]) << 16)
                    | (u32::from(rest[2]) << 24))
                    >> 2,
            )
            .expect("u32 SCALE length fits usize")
        }
        _ => panic!("fixture unexpectedly uses SCALE big-integer compact mode"),
    }
}

fn decode_scale_bytes<'a>(input: &'a [u8], offset: &mut usize) -> &'a [u8] {
    let length = decode_compact(input, offset);
    let end = offset
        .checked_add(length)
        .expect("SCALE length does not overflow");
    let value = input.get(*offset..end).expect("complete SCALE byte string");
    *offset = end;
    value
}

fn public_key(private_key: &[u8]) -> Vec<u8> {
    SecretKey::from_slice(private_key)
        .expect("valid fixture P-256 private key")
        .public_key()
        .to_encoded_point(false)
        .as_bytes()
        .to_vec()
}

fn shared_secret(private_key: &[u8], public_key: &[u8]) -> [u8; 32] {
    let secret = SecretKey::from_slice(private_key).expect("valid fixture P-256 private key");
    let public = PublicKey::from_sec1_bytes(public_key).expect("valid fixture P-256 public key");
    diffie_hellman(secret.to_nonzero_scalar(), public.as_affine())
        .raw_secret_bytes()
        .as_slice()
        .try_into()
        .expect("P-256 shared secret is 32 bytes")
}

fn aes_key(shared_secret: &[u8; 32]) -> [u8; 32] {
    let mut output = [0_u8; 32];
    Hkdf::<Sha256>::new(Some(&[]), shared_secret)
        .expand(&[], &mut output)
        .expect("32-byte HKDF output is valid");
    output
}

#[test]
fn pinned_reference_opener_matches_crypto_and_scale_oracle() {
    let fixture: Value = serde_json::from_str(FIXTURE_JSON).expect("valid fixture JSON");
    assert_eq!(fixture["schema_version"], 1);
    assert_eq!(fixture["fixture_id"], "pca-opener-v2-0001");
    assert_eq!(field(&fixture, &["provenance", "commit"]), REFERENCE_COMMIT);
    assert_eq!(field(&fixture, &["provenance", "tree"]), REFERENCE_TREE);
    assert_eq!(
        field(
            &fixture,
            &[
                "provenance",
                "source_blobs",
                "bot-core/vendor/app-chat-codec.mjs"
            ]
        ),
        CODEC_BLOB
    );
    assert_eq!(
        field(
            &fixture,
            &["provenance", "source_blobs", "docs/explanation/protocol.md"]
        ),
        PROTOCOL_BLOB
    );
    assert_eq!(
        field(
            &fixture,
            &["provenance", "source_blobs", "bot-core/package-lock.json"]
        ),
        LOCK_BLOB
    );
    assert_eq!(
        field(&fixture, &["provenance", "entrypoint"]),
        "encodeNativeChatRequestV2"
    );

    let sender_private = decode_hex(field(&fixture, &["inputs", "sender_p256_private_key_hex"]));
    let recipient_private = decode_hex(field(
        &fixture,
        &["inputs", "recipient_p256_private_key_hex"],
    ));
    let ephemeral_private = decode_hex(field(
        &fixture,
        &["inputs", "ephemeral_p256_private_key_hex"],
    ));
    let expected_sender_public =
        decode_hex(field(&fixture, &["expected", "sender_p256_public_key_hex"]));
    let expected_recipient_public = decode_hex(field(
        &fixture,
        &["expected", "recipient_p256_public_key_hex"],
    ));
    let expected_envelope_public = decode_hex(field(
        &fixture,
        &["expected", "envelope_p256_public_key_hex"],
    ));
    assert_eq!(public_key(&sender_private), expected_sender_public);
    assert_eq!(public_key(&recipient_private), expected_recipient_public);
    assert_eq!(public_key(&ephemeral_private), expected_envelope_public);

    let payload = decode_hex(field(&fixture, &["expected", "scale_encoded_payload_hex"]));
    let expected_statement = decode_hex(field(&fixture, &["expected", "statement_data_hex"]));
    let expected_encrypted =
        decode_hex(field(&fixture, &["expected", "encrypted_remote_model_hex"]));
    let expected_remote_model = decode_hex(field(&fixture, &["expected", "remote_model_hex"]));

    let mut outer_offset = 0;
    let statement = decode_scale_bytes(&payload, &mut outer_offset);
    assert_eq!(
        outer_offset,
        payload.len(),
        "outer SCALE Bytes must be total"
    );
    assert_eq!(statement, expected_statement);

    let mut statement_offset = 0;
    let envelope_public = decode_scale_bytes(statement, &mut statement_offset);
    let encrypted = decode_scale_bytes(statement, &mut statement_offset);
    assert_eq!(
        statement_offset,
        statement.len(),
        "opener statement framing must be total"
    );
    assert_eq!(envelope_public, expected_envelope_public);
    assert_eq!(encrypted, expected_encrypted);

    let nonce = decode_hex(field(&fixture, &["inputs", "aes_gcm_nonce_hex"]));
    assert_eq!(encrypted.get(..12), Some(nonce.as_slice()));
    let ciphertext_end = encrypted.len().checked_sub(16).expect("AES-GCM tag");
    let ciphertext = &encrypted[12..ciphertext_end];
    let tag = &encrypted[ciphertext_end..];

    let sender_shared = shared_secret(&ephemeral_private, &expected_recipient_public);
    let recipient_shared = shared_secret(&recipient_private, envelope_public);
    assert_eq!(sender_shared, recipient_shared, "P-256 ECDH must agree");
    let key = aes_key(&sender_shared);
    let cipher = Aes256Gcm::new_from_slice(&key).expect("32-byte AES key");

    let mut reproduced_ciphertext = expected_remote_model.clone();
    let reproduced_tag = cipher
        .encrypt_in_place_detached(Nonce::from_slice(&nonce), &[], &mut reproduced_ciphertext)
        .expect("fixture plaintext encrypts");
    assert_eq!(reproduced_ciphertext, ciphertext);
    assert_eq!(reproduced_tag.as_slice(), tag);

    let mut decrypted = ciphertext.to_vec();
    cipher
        .decrypt_in_place_detached(
            Nonce::from_slice(&nonce),
            &[],
            &mut decrypted,
            Tag::from_slice(tag),
        )
        .expect("fixture ciphertext authenticates");
    assert_eq!(decrypted, expected_remote_model);
}
