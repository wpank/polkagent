#!/usr/bin/env node

// Generates the PCA opener oracle from the exact pinned reference checkout.
//
// This is intentionally not part of the default Rust test run: it needs the
// separate polkadot-chat-agents repository and its npm dependencies. The
// checked-in Rust test consumes the generated, immutable JSON vector without
// relying on either at test time.

import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import path from "node:path";

const REFERENCE_COMMIT = "2adddcc8cfd732804cd9bbcbcd26974b44b47f66";
const REFERENCE_TREE = "6b24bc2bab72e355bd86b810bd652914c37164ee";
const CODEC_PATH = "bot-core/vendor/app-chat-codec.mjs";
const CODEC_BLOB = "34512a48d7453c2e2bc6fe86778e18fc95d88e65";
const PROTOCOL_PATH = "docs/explanation/protocol.md";
const PROTOCOL_BLOB = "79740a9df0d463921acbfb76b2a1f5d5cfac11fd";
const LOCK_PATH = "bot-core/package-lock.json";
const LOCK_BLOB = "53781eb1669ccdf01a8b5c0d503f839a966805b3";

const hex = (bytes) => Buffer.from(bytes).toString("hex");
const fromHex = (value) => Buffer.from(value, "hex");

function git(referenceRoot, ...args) {
  return execFileSync("git", ["-C", referenceRoot, ...args], {
    encoding: "utf8",
  }).trim();
}

function assertReference(referenceRoot) {
  const actualCommit = git(referenceRoot, "rev-parse", "HEAD");
  if (actualCommit !== REFERENCE_COMMIT) {
    throw new Error(`reference HEAD is ${actualCommit}, expected ${REFERENCE_COMMIT}`);
  }
  const actualTree = git(referenceRoot, "rev-parse", `${REFERENCE_COMMIT}^{tree}`);
  if (actualTree !== REFERENCE_TREE) {
    throw new Error(`reference tree is ${actualTree}, expected ${REFERENCE_TREE}`);
  }
  for (const [file, expected] of [
    [CODEC_PATH, CODEC_BLOB],
    [PROTOCOL_PATH, PROTOCOL_BLOB],
    [LOCK_PATH, LOCK_BLOB],
  ]) {
    const actual = git(referenceRoot, "rev-parse", `${REFERENCE_COMMIT}:${file}`);
    if (actual !== expected) {
      throw new Error(`${file} blob is ${actual}, expected ${expected}`);
    }
  }
}

function compactAt(bytes, offset = 0) {
  const first = bytes[offset];
  const mode = first & 3;
  if (mode === 0) return { value: first >> 2, offset: offset + 1 };
  if (mode === 1) {
    return {
      value: (first | (bytes[offset + 1] << 8)) >> 2,
      offset: offset + 2,
    };
  }
  if (mode === 2) {
    const raw =
      first |
      (bytes[offset + 1] << 8) |
      (bytes[offset + 2] << 16) |
      (bytes[offset + 3] << 24);
    return { value: raw >>> 2, offset: offset + 4 };
  }
  throw new Error("fixture does not use SCALE compact big-integer mode");
}

function bytesAt(bytes, offset = 0) {
  const length = compactAt(bytes, offset);
  const end = length.offset + length.value;
  if (end > bytes.length) throw new Error("truncated SCALE bytes");
  return { value: bytes.subarray(length.offset, end), offset: end };
}

function decryptRemoteModel(recipientPrivateKey, payload) {
  const outer = bytesAt(payload);
  if (outer.offset !== payload.length) throw new Error("outer payload has trailing bytes");
  const envelopeKey = bytesAt(outer.value);
  const encrypted = bytesAt(outer.value, envelopeKey.offset);
  if (encrypted.offset !== outer.value.length) throw new Error("statement data has trailing bytes");

  const ecdh = crypto.createECDH("prime256v1");
  ecdh.setPrivateKey(recipientPrivateKey);
  const sharedSecret = ecdh.computeSecret(envelopeKey.value);
  const aesKey = crypto.hkdfSync("sha256", sharedSecret, Buffer.alloc(0), Buffer.alloc(0), 32);
  const nonce = encrypted.value.subarray(0, 12);
  const tag = encrypted.value.subarray(encrypted.value.length - 16);
  const ciphertext = encrypted.value.subarray(12, encrypted.value.length - 16);
  const decipher = crypto.createDecipheriv("aes-256-gcm", aesKey, nonce);
  decipher.setAuthTag(tag);
  const remoteModel = Buffer.concat([decipher.update(ciphertext), decipher.final()]);
  return { outer: outer.value, envelopeKey: envelopeKey.value, encrypted: encrypted.value, remoteModel };
}

async function generate(referenceRoot) {
  assertReference(referenceRoot);

  const senderSeed = fromHex("11".repeat(32));
  const signatureNonce = fromHex("22".repeat(32));
  const senderP256PrivateKey = fromHex("33".repeat(32));
  const recipientP256PrivateKey = fromHex("44".repeat(32));
  const ephemeralP256PrivateKey = fromHex("55".repeat(32));
  const aesNonce = fromHex("66".repeat(12));
  const recipientAccountId = fromHex("77".repeat(32));
  const messageId = "PCA-REFERENCE-OPENER-0001";
  const timestamp = 1_783_334_456_789n;
  const text = "hello from the pinned PCA opener fixture";

  const scureUrl = pathToFileURL(
    path.join(referenceRoot, "bot-core/node_modules/@scure/sr25519/index.js"),
  );
  const { getPublicKey, secretFromSeed, sign } = await import(scureUrl.href);
  const senderSecret = secretFromSeed(senderSeed);
  const walletPair = {
    publicKey: getPublicKey(senderSecret),
    sign: (message) => sign(senderSecret, message, signatureNonce),
  };

  const codecUrl = pathToFileURL(path.join(referenceRoot, CODEC_PATH));
  const originalCreateEcdh = crypto.createECDH;
  const originalRandomBytes = crypto.randomBytes;
  let ephemeralGenerations = 0;
  crypto.createECDH = (...args) => {
    const ecdh = originalCreateEcdh(...args);
    ecdh.generateKeys = () => {
      ephemeralGenerations += 1;
      if (ephemeralGenerations !== 1) {
        throw new Error("unexpected additional ephemeral P-256 generation");
      }
      ecdh.setPrivateKey(ephemeralP256PrivateKey);
      return ecdh.getPublicKey();
    };
    return ecdh;
  };
  crypto.randomBytes = (size) => {
    if (size !== aesNonce.length) throw new Error(`unexpected randomBytes(${size})`);
    return Buffer.from(aesNonce);
  };

  let codec;
  let encoded;
  try {
    codec = await import(`${codecUrl.href}?fixture=${REFERENCE_COMMIT}`);
    const recipientPublicKey = codec.p256PublicKeyFromPrivateKey(recipientP256PrivateKey);
    encoded = codec.encodeNativeChatRequestV2({
      walletPair,
      botAccountId: recipientAccountId,
      botIdentifierKey: recipientPublicKey,
      ownP256PrivateKey: senderP256PrivateKey,
      text,
      messageId,
      timestamp,
    });
  } finally {
    crypto.createECDH = originalCreateEcdh;
    crypto.randomBytes = originalRandomBytes;
  }

  const recipientPublicKey = codec.p256PublicKeyFromPrivateKey(recipientP256PrivateKey);
  const senderPublicKey = codec.p256PublicKeyFromPrivateKey(senderP256PrivateKey);
  const decoded = codec.decodeEncryptedChatRequestPayload(
    encoded.payload,
    recipientP256PrivateKey,
    recipientAccountId,
  );
  if (
    decoded.messageId !== messageId ||
    decoded.text !== text ||
    BigInt(decoded.timestamp) !== timestamp
  ) {
    throw new Error("pinned reference decoder did not recover the generated opener");
  }
  const decrypted = decryptRemoteModel(recipientP256PrivateKey, encoded.payload);

  return {
    schema_version: 1,
    fixture_id: "pca-opener-v2-0001",
    provenance: {
      repository: "https://github.com/shawntabrizi/polkadot-chat-agents.git",
      commit: REFERENCE_COMMIT,
      tree: REFERENCE_TREE,
      describe: "v0.7.0-8-g2adddcc",
      generated_with_node: process.version,
      source_blobs: {
        [CODEC_PATH]: CODEC_BLOB,
        [PROTOCOL_PATH]: PROTOCOL_BLOB,
        [LOCK_PATH]: LOCK_BLOB,
      },
      entrypoint: "encodeNativeChatRequestV2",
    },
    algorithm: {
      key_agreement: "P-256 ECDH (prime256v1), uncompressed SEC1 public keys",
      key_derivation: "HKDF-SHA256, empty salt, empty info, 32-byte output",
      encryption: "AES-256-GCM, payload nonce || ciphertext || 16-byte tag",
      framing: "SCALE Bytes(SCALE Bytes(ephemeral_public_key) || SCALE Bytes(encrypted_remote_model))",
      signature: "sr25519 over message_bytes || SCALE Bytes(recipient_account_id)",
    },
    inputs: {
      sender_sr25519_seed_hex: hex(senderSeed),
      sender_sr25519_signature_nonce_hex: hex(signatureNonce),
      sender_p256_private_key_hex: hex(senderP256PrivateKey),
      recipient_p256_private_key_hex: hex(recipientP256PrivateKey),
      ephemeral_p256_private_key_hex: hex(ephemeralP256PrivateKey),
      aes_gcm_nonce_hex: hex(aesNonce),
      recipient_account_id_hex: hex(recipientAccountId),
      message_id: messageId,
      timestamp_ms: timestamp.toString(),
      text,
    },
    expected: {
      sender_sr25519_public_key_hex: hex(walletPair.publicKey),
      sender_p256_public_key_hex: hex(senderPublicKey),
      recipient_p256_public_key_hex: hex(recipientPublicKey),
      envelope_p256_public_key_hex: hex(decrypted.envelopeKey),
      remote_model_hex: hex(decrypted.remoteModel),
      encrypted_remote_model_hex: hex(decrypted.encrypted),
      statement_data_hex: hex(encoded.statementData),
      scale_encoded_payload_hex: hex(encoded.payload),
    },
  };
}

const referenceRoot = path.resolve(
  process.argv[2] ?? process.env.PCA_REFERENCE_ROOT ?? "/Users/will/dev/par/polkadot-chat-agents",
);
const fixture = await generate(referenceRoot);
process.stdout.write(`${JSON.stringify(fixture, null, 2)}\n`);
