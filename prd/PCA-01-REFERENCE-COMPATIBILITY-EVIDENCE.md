# PCA-01 pinned-reference compatibility evidence

**Decision date:** 2026-08-06

**Status:** accepted reference oracle; production compatibility remains open

**Scope:** PCA-01 / PRD-06 TEST-01 and C0-T01

## Outcome

The PCA source named by PRD-06 is locally available and verifiable at exact
commit `2adddcc8cfd732804cd9bbcbcd26974b44b47f66`
(`v0.7.0-8-g2adddcc`). An immutable opener vector now freezes the exact
reference source objects, deterministic inputs, encrypted output, and decoded
remote-model bytes. A default Rust test independently reproduces the vector's
SCALE framing, P-256 ECDH shared secret, HKDF-SHA256 key, and AES-256-GCM
ciphertext and authentication tag.

This evidence does **not** make the current `TcpPcaTransport` C0-compatible.
The current transport and the pinned PCA application speak different protocols.
TEST-01 remains open until a production codec path or versioned JS bridge uses
the fixture and passes cross-process reference interoperability.

## Immutable provenance

| Item | Pinned value |
|---|---|
| Repository | `https://github.com/shawntabrizi/polkadot-chat-agents.git` |
| Commit | `2adddcc8cfd732804cd9bbcbcd26974b44b47f66` |
| Tree | `6b24bc2bab72e355bd86b810bd652914c37164ee` |
| `bot-core/vendor/app-chat-codec.mjs` Git blob | `34512a48d7453c2e2bc6fe86778e18fc95d88e65` |
| `docs/explanation/protocol.md` Git blob | `79740a9df0d463921acbfb76b2a1f5d5cfac11fd` |
| `bot-core/package-lock.json` Git blob | `53781eb1669ccdf01a8b5c0d503f839a966805b3` |
| Reference entrypoint | `encodeNativeChatRequestV2` |
| Generator runtime | Node.js `v22.22.2` |

The generator refuses a checkout whose HEAD, tree, or any pinned source blob
differs. It replaces only the reference function's two randomness sources for
the duration of generation: the ephemeral P-256 private key and the 12-byte
AES-GCM nonce. The sr25519 signer also receives a fixed test-only nonce. The
exact pinned encoder produces the vector and the exact pinned decoder must
recover its fixed message ID, timestamp, and text before output is emitted.

The checked-in artifacts are:

- [opener fixture](../crates/polkagent-transport-pca/tests/fixtures/pca_reference_opener_v2.json)
- [deterministic reference generator](../crates/polkagent-transport-pca/tests/reference/generate_opener_fixture.mjs)
- [independent Rust crypto/framing oracle](../crates/polkagent-transport-pca/tests/pca_reference_opener.rs)

To regenerate from the exact reference checkout after installing its locked
`bot-core` dependencies:

```sh
node crates/polkagent-transport-pca/tests/reference/generate_opener_fixture.mjs \
  /Users/will/dev/par/polkadot-chat-agents
```

The command prints JSON; review it against the checked-in fixture. Generation
is deliberately not automatic because an adjacent source checkout is not a
valid CI dependency.

## Exact pinned opener construction

At the pinned commit, the relevant opener construction is:

1. PCA derives the long-lived chat P-256 private key with `blake2b-256` over
   the first 32 bytes of the chat sr25519 private
   key and encodes P-256 public keys as 65-byte uncompressed SEC1 points.
2. `encodeNativeChatRequestV2` constructs the SCALE message model, including
   fixed message identity/timestamp/content fields, the sender's sr25519
   statement account, its keyed identity proof, and its P-256 public key.
3. It signs `message_bytes || SCALE Bytes(recipient_account_id)` with sr25519
   and appends the proof signer.
4. It generates an ephemeral P-256 key, computes ECDH with the recipient
   identifier key, and derives a 32-byte AES key with HKDF-SHA256 using empty
   salt and empty info.
5. It encrypts the remote model with AES-256-GCM. The encrypted field is
   `12-byte nonce || ciphertext || 16-byte tag`.
6. The opener payload is
   `SCALE Bytes(SCALE Bytes(ephemeral_public_key) || SCALE Bytes(encrypted_remote_model))`.
7. PCA publishes that payload through its signed Statement Store submission
   path with the all-peer and day-pagination topics. Follow-ups use separately
   derived session/device topics and are not opener frames.

The reference decoder unwraps either the outer SCALE byte string or the raw
statement model, requires a 65-byte P-256 envelope key, decrypts the remote
model, verifies its sr25519 request proof, and exposes the sender/session
identity only after those checks.

## Compatibility audit against `polkagent-transport-pca`

| Boundary | Pinned PCA reference | Current Polkagent implementation | Result |
|---|---|---|---|
| Network | Outbound RPC to Statement Store; no inbound listener | Direct TCP listener and configured peer socket | Incompatible |
| Opener framing | Nested SCALE byte strings inside a signed statement | Length-prefixed JSON `WireFrame::Hello` / `HelloAck` | Incompatible |
| Key agreement | P-256 ECDH, 65-byte uncompressed SEC1 keys | X25519, 32-byte public keys | Incompatible |
| Key derivation | HKDF-SHA256, empty salt/info | Raw X25519 shared secret used as AEAD key | Incompatible |
| AEAD | AES-256-GCM with random nonce prepended to ciphertext/tag | ChaCha20-Poly1305 with session sequence nonce | Incompatible |
| Identity | sr25519 request signature plus keyed identity proof | Configured SS58 string equality in JSON handshake/message | Incompatible and not cryptographic authentication |
| Message model | SCALE opener/session models and per-message decoding | Serde JSON application payload | Incompatible |
| Channels | Identity/day opener topics, then deterministic identity/device session topics | One TCP peer connection | Incompatible |
| ACK semantics | Session-response request IDs over one-slot statement lanes | Bespoke durable wire ACK by delivery ID | Similar durability intent, different wire behavior |

The current TCP tests remain useful evidence for persistence, reconnect, dedup,
and application-ACK boundaries, but they are tests of Polkagent's bounded
adapter seam. They cannot be cited as PCA C0 protocol evidence.

## What the executable oracle proves

The default Rust test proves all of the following without the adjacent PCA
checkout or Node.js:

- the fixture carries the exact pinned commit/tree/blob identities;
- sender, recipient, and ephemeral fixed P-256 secrets derive the frozen SEC1
  public keys;
- the outer and inner SCALE byte-string boundaries consume every byte;
- sender and recipient P-256 ECDH derive the same shared secret;
- HKDF-SHA256 and AES-256-GCM reproduce the exact reference ciphertext/tag;
- the ciphertext authenticates and decrypts to the frozen remote-model bytes.

The reference generator additionally proves that the exact pinned JS decoder
accepts the generated opener and verifies its sr25519 request signature. The
Rust test does not yet expose a production opener decoder, verify the sr25519
proof itself, submit a signed Statement Store statement, or handle device
follow-ups.

## Implementation handoff and release gate

The next PCA-01 packet can proceed without rediscovering the wire algorithm:

- [ ] Introduce a versioned PCA reference codec boundary distinct from the
  existing `TcpPcaTransport`; do not silently repurpose the TCP protocol.
- [ ] Implement bounded SCALE opener encode/decode and make the production
  codec consume this fixture in both directions.
- [ ] Verify the sr25519 statement proof, keyed identity proof, exact recipient
  account binding, and P-256 key validity before accepting work.
- [ ] Implement the signed Statement Store submission/query boundary, opener
  topics, session topics, and per-device channel discovery.
- [ ] Add a versioned JS codec-bridge conformance runner that regenerates and
  consumes the same fixture at the pinned commit.
- [ ] Add negative vectors for malformed SCALE lengths, invalid SEC1 points,
  wrong recipient/account proof, altered ciphertext/tag, trailing bytes, and
  oversized message batches.
- [ ] Pass C0-T01 through the production adapter, then C0-T02 through C0-T14
  and the disposable real-device smoke before any C0 compatibility claim.

Until every relevant gate passes, user-facing and planning language must call
the TCP implementation a durable PCA transport seam, not PCA-compatible wire
support.
