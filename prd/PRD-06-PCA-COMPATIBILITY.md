# PRD-06: PCA Compatibility, Chat/Mobile and Messaging

> **Implementation note (audited 2026-08-05):** This PRD remains normative,
> but its embedded implementation statements and checklists are not current
> status evidence. Use [STATUS.md](STATUS.md) and
> [IMPLEMENTATION-BACKLOG.md](IMPLEMENTATION-BACKLOG.md) for verified state and
> the dependency-ordered execution queue.

**Status:** definitive product requirements document

**Audience:** engineers, product designers, and operators who may have no
prior knowledge of PCA, Polkadot, or the Polkagent project

**Purpose:** specify every requirement for preserving, migrating, and
improving the user-visible behavior of `polkadot-chat-agents` (PCA) within
the Polkagent platform

**Implementation status:** requirements and design proposals; this document
does not claim that the Polkagent successor is implemented or validated in
production

**Evidence snapshot:** PCA repository commit
`2adddcc8cfd732804cd9bbcbcd26974b44b47f66` (`v0.7.0-8-g2adddcc`), inspected
2026-07-29. All compatibility fixtures must record the source commit/tag they
represent; future PCA changes do not silently change C0-C3 claims.

**Cross-document interfaces:** this PRD depends on vocabulary from PRD-02
(Vocabulary, Invariants and System Architecture), execution model from PRD-03
(Agent/Run/Effect/Graph Execution Model), provider/harness contracts from
PRD-04 (Providers, Models, Harnesses, Tools and Skills), and identity/policy
from PRD-07 (Identity, Accounts, Signers, Policy and Security). Required
interfaces are summarized inline so this document can be read independently.

---

## Table of contents

1. [Plain-language purpose and user outcome](#1-plain-language-purpose-and-user-outcome)
2. [Definitions](#2-definitions)
3. [PCA primer for newcomers](#3-pca-primer-for-newcomers)
4. [Personas and jobs affected](#4-personas-and-jobs-affected)
5. [Scope, non-goals, defaults and maturity](#5-scope-non-goals-defaults-and-maturity)
6. [Complete PCA feature audit](#6-complete-pca-feature-audit)
7. [C0-C3 compatibility tiers](#7-c0-c3-compatibility-tiers)
8. [Feature-by-feature compatibility matrix](#8-feature-by-feature-compatibility-matrix)
9. [Legacy-to-canonical domain mapping](#9-legacy-to-canonical-domain-mapping)
10. [Replacement architecture](#10-replacement-architecture)
11. [State, config and identity import/export](#11-state-config-and-identity-importexport)
12. [Bridge and OpenAPI compatibility plan](#12-bridge-and-openapi-compatibility-plan)
13. [Native and compatibility test corpora](#13-native-and-compatibility-test-corpora)
14. [UX improvements without breaking consumers](#14-ux-improvements-without-breaking-consumers)
15. [Rolling migration strategy](#15-rolling-migration-strategy)
16. [User journeys](#16-user-journeys)
17. [Functional requirements](#17-functional-requirements)
18. [Non-functional requirements](#18-non-functional-requirements)
19. [Architecture boundaries and dependencies](#19-architecture-boundaries-and-dependencies)
20. [Rust traits/types and wire/schema examples](#20-rust-traitstypes-and-wireschema-examples)
21. [Security, privacy, tenancy, custody and abuse implications](#21-security-privacy-tenancy-custody-and-abuse-implications)
22. [Observability and operator/support requirements](#22-observability-and-operatorsupport-requirements)
23. [Acceptance criteria and verification checklist](#23-acceptance-criteria-and-verification-checklist)
24. [Phased delivery and release gates](#24-phased-delivery-and-release-gates)

---

## 1. Plain-language purpose and user outcome

PCA (`polkadot-chat-agents`) is the existing, working product that lets users
interact with AI coding agents and other agent runtimes through Polkadot's
private, encrypted mobile and desktop chat. It is a Node.js application that
an operator runs on a laptop or server. Users message the bot from their phone
or desktop through the Polkadot App or T3ams; the bot processes the message
with a local AI CLI or an external agent framework, and sends back an ordered,
encrypted reply.

PCA is the reference product that Polkagent must preserve and improve.
Polkagent replaces PCA's Node.js internals with a modular Rust runtime while
keeping every user-visible behavior intact. An existing PCA user should be
able to migrate their bot -- identity, sessions, files, projects,
configuration -- to Polkagent and continue using it without losing messages,
breaking their phone's chat thread, or needing to re-register with their
contacts.

The user outcome is:

- **For chat users:** the bot continues to work exactly as before -- same
  identity, same encrypted chat, same commands, same file handling, same
  ordered replies -- with improved reliability and new capabilities arriving
  over time.
- **For bot operators:** a safer, more observable runtime with typed
  configuration, durable state, deployment manifests, fleet management, and
  clear migration tooling.
- **For framework integrators:** the bridge API they already use continues to
  work, with versioned compatibility, better documentation, and a path to
  richer integration.

---

## 2. Definitions

These terms are used throughout this document. They are defined here for
readers unfamiliar with PCA or Polkadot.

| Term | Definition |
|---|---|
| **PCA** | `polkadot-chat-agents`, the existing Node.js reference product in `/Users/will/dev/par/polkadot-chat-agents`. Statements beginning "PCA does" describe observed current behavior at the pinned commit. |
| **Polkagent** | The proposed Rust-first successor platform in `/Users/will/dev/par/polkagent`. Statements beginning "Polkagent will" are requirements or design proposals, not implemented behavior. |
| **Polkadot App** | A mobile and desktop application for the Polkadot ecosystem. PCA uses its private chat transport to receive and send encrypted messages. |
| **T3ams** | A separate Polkadot-based chat surface supporting DMs, workspace channels, mentions, threads, reactions, typing indicators, and rich media. |
| **Statement Store** | A public Polkadot/People Chain primitive: signed, short-lived, store-and-forward statements organized by topics. PCA uses it as its underlying transport mechanism. A statement is not a durable message; a later statement can replace an earlier one in the same channel. The Statement Store is a best-effort gossip layer: one-slot replacement, allowance-gated, Sr25519-signed. Reliability must be composed at the application layer (ACK protocol, ordered lanes, idempotent processing). Do not treat it as a durable ordered queue. |
| **Bulletin Chain** | A Polkadot-ecosystem chain for anchoring content-addressed payloads (CIDs). Testnet provides approximately two weeks of retention. Used to anchor content that must outlive the Statement Store TTL, not as a primary storage layer. IPFS-compatible content addressing, approximately 8 MiB per transaction, authorization-gated. Parity's "Levity" project (Bulletin + Handoff Protocol) is the emerging decentralized storage/CDN layer built on this primitive. |
| **Transport** | The component that authenticates, receives, ACKs, decrypts, and sends chat traffic. It must not decide which model to run or own persistent application state. |
| **Conversation** | A private peer chat, T3ams DM, or T3ams channel/thread context. User turns within one conversation must remain ordered. |
| **Opener** | The first encrypted request that establishes a new chat session between a user and a bot. All subsequent messages in that session are **follow-ups**. |
| **Device channel** | After an opener, a phone may send follow-up messages on a device-specific channel rather than the identity channel. A bot that only watches the identity channel will appear to work in test clients but fail with real phones. |
| **ACK (acknowledgement)** | PCA and Polkagent use three distinct acknowledgement concepts that must never be confused. See section 3.5 for details. |
| **Owed reply** | PCA's durable record that a message was accepted and still requires agent handling. Polkagent calls this a **pending turn** or **pending delivery**. |
| **Outbound lane** | A per-peer outbound queue that prevents later Statement Store submissions from replacing an earlier, unfetched one. See section 3.7 for the full mechanism. |
| **Direct brain** | PCA starts a local AI CLI process (Claude Code, Codex, OpenCode, or a custom command) to handle a user's message. The bot owns the conversation lifecycle. |
| **Bridge brain / Harness** | Instead of starting a local CLI, PCA exposes a local authenticated HTTP API. An external agent framework (Hermes, OpenClaw, or custom) long-polls this API for work, processes it, and sends replies back through the bridge. |
| **Bridge** | PCA's local authenticated HTTP compatibility API for an external harness. It provides leased inbound work and safe outbound/artifact operations. |
| **Lease** | A short-lived claim on a bridge delivery. A worker renews the lease while it is still processing and ACKs only when it has safely handled the message. If the lease expires (worker crashed), the delivery becomes available again. |
| **HOP** | PCA's transport-specific storage/delivery mechanism for encrypted attachments and files. HOP endpoints are operator-pinned; raw tickets never reach an executor. |
| **BCTS / Bulletin** | T3ams transport-specific media storage/delivery. |
| **ResolvedGrant** | The exact, resolved set of permissions and limits available to a particular run or effect, computed from policy. |
| **EffectIntent** | An immutable command recorded before any external I/O (model call, tool invocation, egress send, signing request). It includes a canonical request digest and idempotency classification. |
| **EffectAttempt** | One claim, lease, retry, and idempotency lifecycle for a single execution of an EffectIntent. |
| **EffectOutcome** | The immutable result (success, failure, timeout, cancelled, or unknown) observed for one EffectAttempt. |
| **Data plane** | The runtime that owns conversations, runs, workspaces, effects, artifacts, and execution. It may run locally, self-hosted, or as an isolated managed worker. |
| **Control plane** | Optional management services for tenancy, fleet configuration, policy rollout, observability, billing, and deployment coordination. It is deliberately separate from private data-plane state. |
| **C0-C3** | Compatibility tiers defined in section 7. C0 = transport/protocol, C1 = bridge/harness, C2 = state/operator migration, C3 = managed product continuity. |

---

## 3. PCA primer for newcomers

### 3.1 What PCA is and why it matters

PCA is a bot runtime that sits between two things: a private encrypted chat
transport (Polkadot App chat or T3ams) and an AI agent (a coding CLI like
Claude Code, or an agent framework like Hermes). It handles everything in
between: identity, encryption, message ordering, acknowledgements,
deduplication, file handling, live progress indicators, and reliable reply
delivery.

PCA matters because it already works. Real users message real bots through
their phones and get useful AI-powered responses, all through Polkadot's
privacy-preserving infrastructure. Any successor must preserve this working
experience before improving it.

### 3.2 The user journey: message from mobile to response

Here is what happens when a user sends a message to a PCA bot:

```text
Step 1: User opens the Polkadot App on their phone, navigates to the
        bot's chat, and types a message.

Step 2: The app encrypts the message using established session keys and
        submits it as a signed Statement Store statement to the network.

Step 3: The bot process, running on the operator's laptop or server,
        discovers the new statement through its subscription and/or
        polling loop.

Step 4: The bot decrypts the message, verifies the sender's identity
        and session, and checks the allowlist.

Step 5: The bot durably records two things in a single operation:
        (a) a deduplication marker so the same message is never processed
            twice, and
        (b) an "owed reply" record so the message will survive a crash.
        Only after this durable write does the bot ACK the message to
        the transport, telling the sender's app to stop resending.

Step 6: The bot dispatches the message to the configured brain:
        - Direct brain: spawns a local AI CLI (e.g., Claude Code) with
          the user's message, capturing its streaming output.
        - Bridge brain: makes the message available via the bridge HTTP
          API for an external harness to claim with a lease.

Step 7: The AI processes the message -- possibly using tools, reading
        files, writing code, running commands -- while the bot captures
        progress events and optionally shows a "thinking" indicator in
        the chat.

Step 8: When the AI produces a response, the bot:
        (a) records the response as a durable outbox item,
        (b) encrypts it,
        (c) submits it to the Statement Store in the correct outbound
            lane position, respecting the one-slot-per-channel constraint.

Step 9: The user's app discovers and decrypts the response, displaying
        it in the chat thread.
```

This sequence has many subtleties -- device channels, session key derivation,
ordered lanes, lease mechanics -- each explained in the following sections.

### 3.3 Runtime topology: transport, bot loop, brains, bridges

PCA has one transport core and two execution paths:

```text
                     PCA transport core
                 identity + sessions + ACKs
                      files + outbound lane
                      /                 \
                     /                   \
            direct brain                 bridge brain
        spawn a local agent CLI      local authenticated HTTP API
      Claude/Codex/OpenCode/echo     Hermes/OpenClaw/custom harness
```

**Transport core.** This is the heart of PCA. It manages the bot's network
identity (a Polkadot account with associated encryption keys), establishes
encrypted sessions with users, polls for incoming messages, validates senders,
manages acknowledgements, handles file attachments, and maintains ordered
outbound lanes for reliable reply delivery. It connects to the Polkadot
network using only outbound RPC calls -- no inbound webhook or public HTTP
port is required.

**Direct brain.** PCA spawns a headless coding-agent CLI process (Claude Code,
Codex, OpenCode, or a custom command) for each incoming message. Its runner
converts the CLI's streaming JSON output into a small common event vocabulary:
session started, tool/action progress, partial/final text, usage, or error.
PCA owns the conversation queue, native session resume identifier, commands,
thinking state, file staging, and final reply delivery. Tools start disabled;
the operator deliberately grants portable capabilities such as `read`,
`write`, `bash`, `web`, and `subagents`.

**Bridge brain.** PCA still owns all transport cryptography and message
delivery, but an external agent framework long-polls the bridge HTTP API for
leased inbound messages. The harness renews its lease during slow work, sends
reply/artifact operations back through the bridge, then ACKs the lease when
done. Hermes is a Python agent plugin; OpenClaw is a TypeScript channel
plugin. A custom framework can implement the same polling loop.

| Concern | PCA direct brain | PCA bridge brain | Polkagent proposal |
|---|---|---|---|
| Agent loop | PCA child process | External framework process | `TurnExecutor` contract for both |
| Session memory | CLI-native resume ID per peer | Framework-owned | Opaque `ExecutorSessionRef`, scoped to executor/model/workspace revision |
| Work claim | PCA internal queue and owed work | HTTP lease and ACK | One durable delivery/turn claim state machine |
| Tools/files | CLI-specific policy compiled by PCA | Framework handles agent tools; bridge mediates chat vault | Immutable resolved grant and mediated artifact/tool ports |
| Reply | PCA sends reply itself | Harness requests bridge send | Durable egress intent/outbox for either path |

### 3.4 Protocol and transport architecture

The transport's critical sequence is more than "receive text, call a model,
send text." A new conversation is an encrypted opener. PCA verifies the
sender's identity proof, establishes session keys, and derives session
channels. The phone may subsequently send follow-ups on **device-specific**
channels rather than the identity channel; missing those channels makes a bot
appear to work in a test client but fail with a real phone.

```text
new chat:
phone -- encrypted opener --> bot
phone <-- accept/session info -- bot
                          |
follow-up:                v
phone -- encrypted device-session request --> bot
phone <-- session ACK ----------------------- bot
```

PCA uses both a push subscription and a periodic polling/reconciliation loop.
Neither alone is sufficient: subscriptions reduce latency; polling recovers
from missed events, restarts, and unhealthy connections. The transport must
process batches defensively: one unknown future content item must not discard
valid text in the same batch.

### 3.5 The three acknowledgement concepts

PCA and Polkagent use three distinct ACK types. Confusing them breaks
reliability.

| Acknowledgement | Who sends it | What it means | What it must never mean |
|---|---|---|---|
| **Inbound transport ACK** | Bot to chat peer | The source message is durably accepted; the peer can stop resending it. | "The model answered" or "the reply was delivered." |
| **Outbound peer ACK** | Chat peer to bot | The peer fetched/acknowledged the current outbound Statement Store statement. | A harness lease ACK or final model success. |
| **Bridge lease ACK** | Harness to bot | The framework safely completed/accepted that leased bridge delivery. | Source transport ACK or remote peer receipt. |

This distinction is non-negotiable. It permits fast source acknowledgement
while a slow model works, prevents resend storms, and ensures accepted work
survives crashes.

### 3.6 Why durable-before-ACK matters

The reliability contract is: **never ACK a message until you can prove you
will not lose it.** PCA enforces this by writing to `session-state.json`
(recording both the dedup marker and the owed-reply record) before sending
the transport ACK. If the write fails or the process crashes before the
write, the message is never ACKed and the sender's app retries delivery.

Polkagent replaces the JSON file with a SQLite/WAL transaction, but the
invariant is identical: a single atomic operation records dedup + pending
delivery, and only after that operation succeeds does the transport ACK go
out.

If this invariant is violated:
- **ACK without durable record:** the user's message is silently lost on crash.
- **Durable record without dedup:** the same message is processed twice,
  potentially running tools, sending duplicates, or spending credits.
- **ACK before model completes:** this is correct and intentional. The user
  should not see their message bouncing while the model thinks.

#### Reliability patterns for durable bot operation

The Statement Store is a best-effort gossip layer, not a reliable ordered
queue. Application-layer reliability must be composed on top of it. The
following patterns -- drawn from analogous high-reliability bot systems --
apply directly to Polkagent:

- **Ordered delivery via per-conversation sequence numbers.** Each
  conversation maintains a monotonically increasing sequence number on
  outbound messages. Recipients can detect gaps and request retransmission
  or discard duplicates by sequence position.
- **Crash recovery via durable offset.** The last successfully processed
  inbound position (e.g., Statement Store cursor or message ID) is stored
  durably before ACK. On restart, processing resumes from that offset rather
  than from the beginning or a potentially stale in-memory position.
- **Idempotent message processing by message-ID dedup.** Every inbound
  message carries a stable message ID. The dedup table records processed IDs
  so that a redelivered message (from resend, reconnect, or crash recovery)
  is recognized and its effects are not repeated.
- **Graceful drain on shutdown.** When the process receives a shutdown
  signal, it completes in-flight work (or records it as pending), flushes
  the outbox lane, persists state, and only then exits. Abrupt termination
  without drain leaves uncertain outbox state that must be reconciled on
  restart.

These patterns are not optional optimizations: they are the minimum required
to make a Statement Store-backed bot reliable under real operating
conditions.

### 3.7 Why outbound lanes matter

The Statement Store keeps **one current statement** per account/channel and
operates as a best-effort gossip layer: it makes no delivery guarantees beyond
eventual propagation. If a bot submits a second independent statement before
the recipient fetches the first, the new statement **replaces** the older one
-- the user never sees the first reply. Reliable ordered delivery is entirely
the application's responsibility.

PCA's outbound lane prevents this:

```text
lane empty -- submit message A --> current(A, request-id-1)
                                   | peer ACK request-id-1
                                   v
                              submit queued B

while A is current:
  - if B fits, replace A with a new statement containing [A, B]
    under request-id-2;
  - if it cannot fit, keep B in a bounded FIFO queue;
  - ignore ACKs for request-id-1 after replacement;
    they do not ACK request-id-2.
```

The replacement is safe only because the new payload is a superset and client
message IDs are deduplicated. A liveness grace policy may eventually replace
an unfetched current slot with queued work for an unreachable peer; that
trade-off must be observable because it can sacrifice eventual visibility of
the old message for future progress.

Polkagent preserves this behavior behind a transport-specific durable lane,
never through a generic unordered "send."

### 3.8 Why leases and owed replies matter under crashes and retries

In a system where AI model calls take seconds to minutes, crashes and retries
are inevitable. PCA's design handles this through two mechanisms:

**Owed replies** (Polkagent: pending turns): When a message is accepted, a
durable record is created saying "this message needs an answer." If the
process crashes mid-model-call, the owed reply survives and will be retried
on restart. Without this, a crash during model processing silently drops the
user's message.

**Bridge leases:** When a harness claims work through the bridge, it gets a
time-limited lease. If the harness crashes without ACKing, the lease expires
and the work becomes available for re-leasing. Without leases, a harness
crash permanently locks a message, and the user never gets a response.

Both mechanisms provide **at-least-once delivery** to the execution layer.
This means executors and harnesses must handle idempotency -- if they receive
the same message twice, they must not produce duplicate side effects.

---

## 4. Personas and jobs affected

| Persona | Job to be done | How this PRD affects them |
|---|---|---|
| **Mobile chat user** | Message a private AI bot and receive ordered, reliable replies through encrypted Polkadot chat | Must experience zero regressions: same identity, same chat thread, same commands, same file handling, same reply ordering |
| **T3ams user** | Interact with an AI bot in T3ams DMs, workspace channels, and threads | Must retain DM, mention, thread, reaction, typing, and rich media behavior |
| **Bot operator** | Create, configure, deploy, monitor, and maintain a chat bot | Gains typed config, durable state, deployment manifests, doctor/repair tooling, and migration path from PCA |
| **Direct brain developer** | Extend or customize the AI CLI integration | Benefits from normalized runner adapters, typed events, and explicit tool policy |
| **Harness/framework integrator** | Connect an agent framework (Hermes, OpenClaw, custom) to the bot via the bridge API | C1 bridge compatibility ensures existing integrations work; versioned API and OpenAPI spec improve development experience |
| **Product team** | Ship a polished bot product to users | Gains observability, fleet management, approval workflows, and deployment automation |
| **Organization operator** | Run multiple bots across environments with policy and audit | C3 managed product continuity with tenant isolation, fleet controls, and portable state |

---

## 5. Scope, non-goals, defaults and maturity

### 5.1 In scope

| Area | Scope |
|---|---|
| Transport compatibility | Polkadot App encrypted chat and T3ams, matching PCA behavior at the pinned commit |
| Bridge compatibility | PCA bridge HTTP API for Hermes, OpenClaw, and custom harnesses |
| State migration | Import of PCA identity, sessions, configuration, files, projects, pending work |
| Direct runners | Claude Code, Codex, OpenCode, echo, and custom JSONL subprocess adapters |
| Commands | All PCA user-facing commands: `/help`, `/reset`, `/stop`, `/ping`, `/model`, `/reasoning`, `/project`, `/usage`, `/file` |
| Files and media | Attachment staging, durable file vault, HOP/BCTS media handling |
| Live reply UX | Thinking indicators, progress edits, chunked long answers, final-as-new-message |
| Outbound lane | Per-channel durable ordered delivery with superset extension and peer ACK tracking |
| Configuration | All PCA configuration options mapped to typed Polkagent equivalents |
| Deployment | Local, SSH, Docker deployment paths with generated manifests |
| Testing | Compatibility fixtures derived from PCA test suite and documentation |

### 5.2 Non-goals

| Non-goal | Rationale |
|---|---|
| Rewriting the Polkadot App or T3ams client | Polkagent is a bot runtime, not a chat client |
| Public unmetered agents | Requires separate admission, billing, and abuse controls (see PRD-08) |
| Marketplace activation | Separate extension/marketplace PRD (see PRD-12) |
| Autonomous spending | Requires payment and custody controls (see PRD-08) |
| XCM/bridge transfers | Requires chain action lifecycle (see PRD-05) |
| Generic browser automation | Not part of chat bot functionality |
| Smart contract dependencies | Not required for chat compatibility |
| Replacing Polkadot's transport protocol | Polkagent uses the transport as-is through adapters |

### 5.3 Defaults and configuration

| Default | Configurable? |
|---|---|
| Private bot with sender allowlist | Yes: can be set to public with admission controls |
| No tools granted to executor | Yes: operator grants `read`, `write`, `bash`, `web`, `subagents` |
| Local deployment, no control plane | Yes: can enroll in managed/self-hosted control plane |
| Polkadot App transport | Yes: T3ams or other transports can be selected |
| Echo brain for testing | Yes: Claude, Codex, OpenCode, bridge, or custom brain |
| SQLite/WAL local state | No: this is the required durable store for the data plane |
| Durable-before-ACK | No: this is a non-negotiable reliability invariant |

### 5.4 Maturity labels

| Label | Meaning |
|---|---|
| **Established** | Owner-confirmed direction; definitive PRD must preserve |
| **Required** | Must be present at the specified compatibility tier |
| **Phased** | Committed but delivered after dependencies and release gates |
| **Experimental** | Requires research, spike, and explicit maturity labeling |
| **Open** | Evidence or lower-level design decision still required |

---

## 6. Complete PCA feature audit

This section documents every PCA behavior and capability that Polkagent must
address. Each subsection corresponds to a functional area in PCA, with
source evidence from the pinned commit.

### 6.1 Bot identity and registration

**PCA behavior.** The operator runs `pca create` to generate a deterministic
bot identity: a Polkadot account derived from a seed, associated
Bandersnatch/encryption keys, and a chat identity. The operator then
registers a human-readable username on the Products Devnet or Paseo network.
The seed and keys are stored locally in the bot directory; they never leave
the operator's machine.

**Source evidence:** `/bot-core/cli.mjs` (create command), `/bot-core/lib/register.mjs`
(registration logic), `/bot-core/vendor/lib/wallet-keys.mjs` (key derivation),
`/tools/bandersnatch-cli/` (Bandersnatch key tooling).

**Polkagent requirement:** Preserve the complete identity lifecycle --
generate, register, recover, import, export, rotate -- using typed network
profiles and secret references. Seeds must never be exposed to executors,
harnesses, or the control plane by default.

### 6.2 Products Devnet/Paseo profiles

**PCA behavior.** PCA supports named network profiles (Products Devnet and
Paseo) that determine RPC endpoints, chain metadata, registration targets,
file storage allowance behavior, and HOP endpoints. The profile is configured
at creation time and determines the bot's entire network context.

**Source evidence:** `/bot-core/lib/network-config.mjs` (network profiles),
`/bot-core/lib/descriptors.mjs` (chain metadata/descriptors),
`/bot-core/scripts/sync-descriptors.mjs` (metadata synchronization).

**Polkagent requirement:** Network profiles become versioned configuration
data, not arbitrary per-turn input. Each profile pins RPC endpoints, metadata
versions, registration targets, and transport parameters. Profile changes are
operator-controlled configuration updates, not user-initiated chat commands.

### 6.3 Encrypted opener/session/device transport

**PCA behavior.** When a user first messages a bot, the transport handles an
encrypted "opener" -- a key exchange that establishes session keys. Subsequent
messages use these session keys. Critically, a phone may send follow-up
messages on device-specific channels derived from the session, not just the
bot's identity channel. PCA must watch all relevant channels to receive all
messages.

The transport uses both a push subscription (for low latency) and periodic
polling/reconciliation (for crash recovery). Batches are processed
defensively: one malformed item does not prevent processing of valid siblings.

**Source evidence:** `/docs/explanation/protocol.md` (protocol specification),
`/bot-core/vendor/app-chat-codec.mjs` (encryption/codec implementation),
`/bot-core/index.mjs` (session restore, ingress wiring).

**Polkagent requirement:** Implement or bridge the exact same encryption,
session, and device-channel behavior. Before advertising native Rust transport
compatibility, produce byte-level test vectors for opener encryption,
follow-up device channels, topic/channel derivation, message IDs, ACKs, and
attachment references. A temporary JS codec bridge is acceptable as a phased
implementation route, but the same C0 fixtures must pass across the bridge
boundary.

**JS codec bridge (do-now).** Native Rust re-implementation of the App Chat
codec is not feasible this quarter. The validated path is: keep a JS codec
bridge (Node process invoked via Rust FFI or subprocess) to handle
encryption/decryption, while owning the state machine, delivery ledger, and
outbox lane entirely in Rust. The bridge boundary must be narrow and
well-tested; the Rust side owns all durable state. Once byte-level fixtures
are green across the bridge, native Rust transport becomes a deferred
migration rather than a blocker.

**Encrypted transport options (validate-next / defer).** The App Chat protocol
is the current encrypted transport. For evaluation in future group-messaging
contexts: Double Ratchet provides strong forward secrecy for pairwise
sessions; MLS (Messaging Layer Security) is suitable for group key agreement
but requires more infrastructure. Bandersnatch is Polkadot-native and already
used for VRF/identity operations in PCA, but its use as a general encrypted
transport primitive is experimental and should not be committed to without a
dedicated spike. Defer native-Rust transport and MLS groups; do not commit
Bandersnatch as a transport dependency without explicit maturity validation.

### 6.4 ACK/dedup/owed-reply rules

**PCA behavior.** When a valid inbound message arrives:

1. Validate sender identity, session, message class, and admission.
2. In one durable operation: record semantic dedup marker + owed reply.
3. Send source transport ACK. If step 2 failed, do not ACK.
4. Queue the message for executor dispatch.

The dedup marker prevents processing the same message twice. The owed reply
survives process crashes. The ACK tells the sender to stop resending. These
three operations are tightly coupled: dedup and owed reply are written
atomically, and ACK follows only after a successful write.

**Source evidence:** `/bot-core/lib/session-store.mjs` (durable state writes),
`/bot-core/index.mjs` (admission and ACK flow).

**Polkagent requirement:** Replace JSON snapshot with SQLite transaction:
`INSERT delivery + dedup_key + pending_turn` in one atomic operation. ACK
only after successful transaction. On failure/full admission, leave
unacknowledged for source retry.

The migration from `session-state.json` to SQLite must use a versioned schema.
Each schema version is recorded in the database; the importer reads the source
JSON format, validates it, and writes normalized rows into the versioned SQLite
schema. Unknown or unsupported JSON fields are reported in the migration
report, never silently dropped. The schema version is the authority for what
data the migration tool can interpret; mismatched versions fail loudly rather
than producing silently wrong data.

### 6.5 Outbound lanes

**PCA behavior.** The Statement Store keeps one current statement per
account/channel. PCA's outbound lane manages this constraint:

- Submit the first message to the current slot.
- Wait for peer ACK before submitting the next queued message.
- If a new message arrives while the current slot is unacknowledged, either
  extend the current statement as a superset (if it fits) or queue it.
- ACKs are tracked by request ID; after a superset replacement, ACKs for the
  old request ID are ignored.
- A liveness grace policy may eventually replace an unfetched slot for an
  unreachable peer, but this is observable and documented.

**Source evidence:** `/bot-core/lib/outbound-lanes.mjs`.

**Polkagent requirement:** Durable per-channel outbox lane with the same
semantics. The lane is a transport-specific concern, not a generic "send
queue." Lane state (current statement, pending queue, peer ACK state) must
survive process restarts.

### 6.6 Files/media/storage

**PCA behavior.** PCA handles files and media at several levels:

- **Incoming attachments:** Transient, bounded-cache material. Available to a
  turn only via a safe artifact handle. Raw peer-provided URLs/tickets never
  reach the executor.
- **Durable file vault:** Per-conversation explicit file storage via `/file put`
  command. List, info, get, remove operations with per-file/per-peer/global caps.
- **HOP client:** Downloads encrypted attachments from trusted operator-pinned
  endpoints with integrity verification, size limits, and no ticket leakage.
- **Media store:** Bounded local cache of materialized attachments with TTL.
- **Framework-generated files:** Upload bytes to the same conversation vault,
  then send by vault path. No arbitrary `/tmp` or harness path is deliverable.
- **Testnet file allowance:** Named-testnet automated Bulletin storage
  allowance via faucet. Profile-specific convenience only.

**Source evidence:** `/bot-core/lib/hop-client.mjs`, `/bot-core/lib/media-store.mjs`,
`/bot-core/lib/file-store.mjs`, `/bot-core/lib/file-commands.mjs`,
`/bot-core/lib/testnet-file-allowance.mjs`, `/docs/guide/files.md`.

**Polkagent requirement:** Conversation-scoped artifact store with content
hashing, classification, retention policies, and quota enforcement. HOP/BCTS
downloads use trusted endpoint policy, encryption/integrity verification,
bounded storage, and no raw ticket/reference exposure. Testnet allowance is
a profile-specific optional adapter.

**Bulletin Chain for durable content anchoring (validate-next).** When content
must outlive the Statement Store TTL, anchor it to the Bulletin Chain using a
content-addressed CID payload. The Bulletin Chain uses IPFS-compatible content
addressing, supports approximately 8 MiB per transaction, and is
authorization-gated. On testnet, retention is approximately two weeks; treat
it as an anchor layer, not a primary storage layer. Parity's "Levity" project
(Bulletin Chain combined with the Handoff Protocol) is the emerging
decentralized storage/CDN layer for the Polkadot ecosystem. The correct
integration pattern is: store content in a durable local or CDN-backed store,
anchor the CID on-chain to provide a verifiable content pointer that survives
Statement Store TTL expiry. Do not rely on Bulletin Chain as a primary
retrieval path during the testnet period.

### 6.7 Direct brains: Claude, Codex, OpenCode, custom

**PCA behavior.** PCA supports four direct brain types plus echo:

| Brain | Implementation | Key behavior |
|---|---|---|
| **Claude Code** | Spawn `claude` CLI with stream-json output | Resume ID per conversation, tools/progress/usage parsing |
| **Codex** | Spawn `codex exec --json` | Session resume, reasoning/model flags |
| **OpenCode** | Spawn `opencode` with JSON format | Many providers via config |
| **Custom** | Spawn arbitrary command with Claude-shaped stream JSON | Strict schema expected |
| **Echo** | Built-in test brain | Returns input as output |

Each runner converts the CLI's streaming JSON into a normalized event
vocabulary. PCA compiles tool policy into CLI-specific flags/configuration.
The operator grants capabilities; the runner translates them into the
appropriate CLI arguments.

**Source evidence:** `/bot-core/lib/runners.mjs` (runner implementations),
`/docs/guide/brains.md` (brain configuration).

**Polkagent requirement:** Each runner becomes a `TurnExecutor` adapter with:
version probe, fixture parser per CLI version, immutable resolved grant,
normalized streaming events, scrubbed environment (no transport keys/seeds
in executor workspace), bounded stdout/time/idle progress, process group
cancellation, and artifact snapshot preservation.

### 6.8 Sessions/resume

**PCA behavior.** Each direct brain maintains a native session per
conversation partner, identified by a resume ID. When a user sends a
follow-up message, the brain resumes the existing session rather than
starting fresh. This preserves conversation context across messages.

Sessions are conversation-scoped and brain-specific. A session resume ID from
Claude Code is meaningless to Codex. Switching brains, models, or project
context invalidates the session.

**Source evidence:** `/bot-core/lib/agent-runtime.mjs` (session management),
`/bot-core/lib/runners.mjs` (per-brain session handling).

**Polkagent requirement:** Opaque `ExecutorSessionRef` per conversation,
scoped to executor + model + workspace revision. Session is invalidated
deterministically when its compatibility scope changes (brain switch, model
switch, project change, or explicit `/reset`).

### 6.9 Model switching and reasoning

**PCA behavior.** Users can switch the active model and reasoning effort via
chat commands:

- `/model <name>` -- switch to a different model within the current brain
- `/reasoning <level>` -- adjust reasoning effort (where supported)

Model switching is governed by policy: a bot can be pinned to one model
(locked), allow a set of approved models, or permit open switching. Public
bots restrict model switching to prevent abuse.

**Source evidence:** `/bot-core/lib/commands.mjs` (command handling),
`/docs/guide/brains.md` (model configuration).

**Polkagent requirement:** Core command service handles model/reasoning
commands before executor dispatch. Policy evaluates the request: is this
model/reasoning level allowed for this sender in this context? Policy
decisions become audit records. Unsupported or denied commands produce
explicit, structured responses rather than model-improvised text.

### 6.10 Projects/worktrees

**PCA behavior.** An operator can register project directories with aliases.
A user activates a project with `/project <alias>`. PCA creates an isolated
branch/worktree for that conversation, scopes the executor's file access to
the worktree, and tracks one active project per conversation peer.

Project registration validates aliases, enforces path boundaries, and
prevents escape to parent directories. Branch/worktree isolation prevents
one conversation from interfering with another's code changes.

**Source evidence:** `/bot-core/lib/workspaces.mjs` (workspace management),
`/docs/guide/projects.md` (project configuration).

**Polkagent requirement:** Projects become workspace grants with alias
validation, branch/worktree isolation, bounded subprocess execution, and
auditable grant records. A project is a policy grant, not a filesystem path
visible to the chat protocol.

### 6.11 Portable tool policy

**PCA behavior.** PCA implements a portable tool policy system that governs
what an executor can do. Capabilities include:

| Capability | Meaning |
|---|---|
| `read` | Read files in the workspace |
| `write` | Write/modify files |
| `bash` | Execute shell commands |
| `web` | Access web resources |
| `subagents` | Spawn child agent processes |

Each capability is compiled into the appropriate CLI arguments for the
active brain. The policy is operator-configured and consistent across
conversations.

**Source evidence:** `/bot-core/lib/tool-policy.mjs`.

**Polkagent requirement:** Retain the outcome-based capability model. Improve
with: verified sandbox/effect/network/secret enforcement reports, explicit
enforcement backend documentation (what actually prevents a `bash` command
from running when `bash` is not granted), and audit records for every
capability evaluation.

### 6.12 Rich replies

**PCA behavior.** PCA supports several reply types through the transport:

- Plain text messages
- Rich text with formatting
- Reply-to-message (quoting a previous message)
- Edit of a previously sent message
- Reactions (emoji responses)
- Typing indicators
- Call-offer decline
- Informational signals (contact, left, coinage)

Not all message types create turns. Only text-bearing messages trigger agent
processing by default. Reactions, typing indicators, and informational signals
are handled by the transport without invoking the executor.

**Source evidence:** `/docs/explanation/protocol.md`, `/bot-core/index.mjs`.

**Polkagent requirement:** Preserve message classification. Text-bearing
kinds create turns; reactions/signals are transport-handled. Bridge
compatibility includes rich fields (reply targets, edit targets, reactions).

### 6.13 Commands

**PCA behavior.** PCA supports the following chat commands:

| Command | Function |
|---|---|
| `/help` | Show available commands |
| `/reset` | Clear the current session |
| `/stop` | Cancel the current operation |
| `/ping` | Health check |
| `/model [name]` | Show or switch the active model |
| `/reasoning [level]` | Show or set reasoning effort |
| `/project [alias]` | Show or switch the active project |
| `/usage` | Show token/cost usage for the current session |
| `/file put/get/list/info/remove` | Manage conversation files |

Commands are processed by PCA before executor dispatch. They are
engine-agnostic and work consistently across direct and bridge brains.

**Source evidence:** `/bot-core/lib/commands.mjs`, `/bot-core/lib/file-commands.mjs`,
`/docs/guide/commands.md`.

**Polkagent requirement:** Move engine-agnostic commands into a core command
service. Commands work consistently for bridge and direct execution paths.
Unsupported commands produce explicit structured responses. Capability-
sensitive commands yield structured explanation/action cards rather than
vague model prose.

### 6.14 T3ams

**PCA behavior.** T3ams is a separate transport with its own identity,
protocol, and capabilities:

- **DMs:** Direct messages between a user and a bot
- **Workspace mentions:** Bot is mentioned in a workspace channel
- **Threads:** Bot replies within a thread context
- **Channel context:** Snapshot of recent channel history for context
- **Reactions:** Emoji responses
- **Typing indicators:** Show when the bot is processing
- **Rich media:** Encrypted file attachments via BCTS/Bulletin
- **Public enrollment:** Channel/workspace admission control
- **SDK/identity key pins:** Separate identity management

T3ams is not a feature flag on the default transport. It has its own identity
management, encryption, message lifecycle, media handling, and delivery
semantics. PCA's T3ams implementation is already modular, split across
dedicated files.

**Source evidence:** `/bot-core/t3ams.mjs` (composition root),
`/bot-core/transports/t3ams/` (modular transport files including
`t3ams-identity.mjs`, `t3ams-protocol.mjs`, `t3ams-routing.mjs`,
`t3ams-submission.mjs`, `t3ams-message-lifecycle.mjs`,
`t3ams-agent-session.mjs`, `t3ams-agent-turn.mjs`,
`t3ams-attachments.mjs`, `t3ams-media.mjs`, `t3ams-channel-context.mjs`,
`t3ams-subscription-set.mjs`, `t3ams-health.mjs`, `t3ams-doctor.mjs`,
`t3ams-live-revocation.mjs`, `t3ams-direct-capacity.mjs`,
`t3ams-delivery-failure.mjs`, `t3ams-media-budget.mjs`,
`t3ams-media-analyzer.mjs`), `/docs/guide/t3ams.md`.

**Polkagent requirement:** Implement as a first-class transport crate, not
conditional logic in the default transport. Use PCA's already-modular
decomposition as the Rust module map. Preserve opaque `t3ams:` conversation
IDs and `thread_root_id`. Never turn channel context into an independent
message to answer.

### 6.15 Bridge leases and proactive authority

**PCA behavior.** The bridge supports two authority modes:

- **Leased delivery:** A harness polls `/inbound`, receives a leased message
  with a `delivery_id` and `lease_id`, renews the lease while working,
  performs operations (send, file, react, typing) scoped to that lease, and
  ACKs when done. All operations require an active, unexpired lease.

- **Proactive authority:** A separate proactive token allows a harness to
  perform unleased actions (e.g., sending a message without receiving one
  first). Proactive authority is independently granted and cannot turn a
  stale lease into a valid one.

Lease fencing is critical for T3ams: prompt-edit/delete events must revoke
the active lease, preventing a harness from replying to a message the user
has already edited or deleted.

**Source evidence:** `/docs/reference/bridge.md`, `/bot-core/index.mjs`
(bridge routes).

**Polkagent requirement:** Preserve lease/renew/ACK/fence semantics
exactly. Proactive tokens are independently managed. Stale, mismatched,
cancelled, or prompt-invalidated leases cannot ACK, send, edit, react, or
type against the current conversation.

### 6.16 Hermes/OpenClaw integrations

**PCA behavior.**

**Hermes** is a Python agent plugin that connects to PCA via the bridge. It
uses bounded keyed dispatch, lease renewal during turns, attachment
materialization, and reply handoff. Configuration includes OAuth lifecycle
management.

**OpenClaw** is a TypeScript channel plugin that connects similarly. It
provides account/config mapping, allowlist defense in depth, per-chat/thread
keyed dispatch, attached-result proactive capability, and delivery routing.

Both are maintained as separate packages within the PCA repository.

**Source evidence:** `/hermes-plugin/polkadot/adapter.py`,
`/hermes-plugin/polkadot/plugin.yaml`, `/openclaw-plugin/polkadot/src/`,
`/openclaw-plugin/polkadot/index.ts`, `/docs/guide/harnesses.md`.

**Polkagent requirement:** Provide maintained migration adapters for both.
Preserve poll/lease/renew/ACK behavior. Credential migration is explicit
and logged; OAuth tokens are not imported by default. Offer clear upgrade
paths to native Polkagent SDK/RPC harness integration.

### 6.17 Deployment/SSH/Docker

**PCA behavior.** PCA supports multiple deployment modes:

- **Local:** Direct execution on the operator's machine
- **SSH:** Remote deployment to a server
- **Docker:** Container deployment with compose files
- **Two-container harness:** Separate containers for bot and harness

The `deploy` command generates deployment artifacts (compose files, environment
configuration) and handles the deployment workflow. Non-root agent execution
is supported. Optional components include a media analyzer container.

**Source evidence:** `/docs/guide/deploy.md`, deployment generator in
`/bot-core/cli.mjs`.

**Polkagent requirement:** Retain one-command deployment paths. Replace
shell-concatenated environment variable ownership with generated immutable
deployment manifests that include: image digest, config revision, required
secret references, volumes, network policy, and execution profile. The same
deployment contract targets local host, self-hosted cluster, or isolated
managed worker.

### 6.18 Configuration

**PCA behavior.** PCA uses a combination of:
- Environment variables for secrets and deployment configuration
- CLI arguments for operational commands
- In-bot configuration for runtime behavior (brains, models, tools, projects)
- Network profile configuration for chain endpoints and metadata

Configuration is documented but uses shell-style environment blobs that can
contain secrets alongside non-sensitive settings.

**Source evidence:** `/docs/reference/configuration.md`,
`/docs/reference/cli.md`, `/bot-core/cli.mjs`.

**Polkagent requirement:** Replace secret-bearing environment blobs with:
- `config.toml` for typed, non-secret configuration
- Secret references for credentials (OS keychain, secret manager, mounted
  file, or tightly permissioned local store)
- Separate secret management for bot seed, session material, bridge token,
  provider credential, and payment signer -- each with its own
  rotation/revocation story

Unknown environment variables fail validation instead of silently becoming
behavior.

### 6.19 Testing

**PCA behavior.** PCA includes:
- Unit tests for core components
- Test clients (`test-client.mjs`, `test-client-device.mjs`) that simulate
  phone-like behavior
- T3ams-specific tests (`test/t3ams/`)
- Testing documentation (`/docs/guide/testing.md`)
- Mocks and fixtures for various transport scenarios

**Source evidence:** `/bot-core/test/`, `/bot-core/test/t3ams/`,
`/bot-core/test-client.mjs`, `/bot-core/test-client-device.mjs`,
`/docs/guide/testing.md`.

**Polkagent requirement:** Adopt PCA fixtures and mocks as the compatibility
test corpus. Extend with: byte-level protocol vectors, device-channel
interop tests, lease lifecycle tests, migration round-trip tests, and
deployment verification tests. See section 13 for the complete test corpora
specification.

---

## 7. C0-C3 compatibility tiers

Compatibility is behavioral and versioned. Polkagent must not read a PCA
state directory concurrently, claim exact internal-file compatibility
indefinitely, or silently reinterpret a key/configuration.

### 7.1 C0 -- Transport/protocol compatibility

**Promise:** A selected Polkagent transport speaks the same relevant
encrypted chat/session protocol as PCA.

**User meaning:** A phone/T3ams user can start, continue, restart, receive
ordered replies, and use enabled rich features without losing messages.

**Release boundary:** Requires byte-level fixtures, device-channel tests,
no-ACK/lane tests, and a disposable live identity smoke test.

**What C0 is not:** C0 does not claim that every PCA feature is enabled on
every Polkagent profile. It is a promise that *advertised* transport
capabilities behave correctly.

**Non-negotiable C0 wire and delivery rules:**

| Rule ID | Rule | Rationale |
|---|---|---|
| C0-R01 | A bot uses outbound RPC only; no inbound webhook requirement | Operator deploys to any environment with outbound internet |
| C0-R02 | Session opener identity proof is verified before session/work acceptance | Prevents impersonation |
| C0-R03 | Follow-ups can arrive on distinct per-device channels | Required for real phone compatibility |
| C0-R04 | Before ACKing, persist both semantic dedup and accepted pending work | Crash safety |
| C0-R05 | ACK means transport receipt, not agent completion | Prevents resend storms during slow model calls |
| C0-R06 | Decoding is per batch item; unknown items do not erase valid siblings | Forward compatibility |
| C0-R07 | One outbound statement slot per account/channel; use current-slot ACK, safe superset extension, queue, and documented liveness takeover | Prevents message loss |
| C0-R08 | Store session/device material, dedup state, pending work, and unresolved outbound state to survive restart | Crash recovery |
| C0-R09 | Subscription accelerates ingress; polling/reconciliation remains a correctness path | Recovery from missed events |
| C0-R10 | Remote attachment endpoints and metadata are attacker input; use allowlisted transport endpoints, caps, content integrity, bounded storage, and no raw ticket/reference exposure | Security boundary |

### 7.2 C1 -- Bridge/harness compatibility

**Promise:** A PCA-shaped bridge poller can safely consume Polkagent
deliveries and publish allowed responses.

**User meaning:** Existing Hermes, OpenClaw, or custom framework integration
can migrate without redesigning its poll/renew/ACK/send loop.

**Release boundary:** Requires OpenAPI/JSON fixtures and stale-lease,
media/vault, edit, proactive-token tests.

**What C1 is not:** C1 does not grant a harness more authority than a direct
executor. The bridge is a compatibility projection, never the system of
record.

### 7.3 C2 -- State/operator migration

**Promise:** An operator can deliberately import a PCA bot with an
explainable report and rollback point.

**User meaning:** Existing identity, policy, sessions, files, and operational
workflow move without accidental parallel service or hidden replay.

**Release boundary:** Requires validated importer, backup/export,
pending-work review, deploy/doctor parity checks.

**What C2 is not:** C2 is an explicit migration product, not a file-format
accident. It does not promise indefinite PCA state directory compatibility.

### 7.4 C3 -- Managed product continuity

**Promise:** Cloud/companion surfaces operate enrolled local/self-hosted data
planes or isolated managed workers without reducing C0-C2 privacy or authority
boundaries.

**User meaning:** Teams gain fleet policy, health, approvals, and
collaboration while portable runtimes remain independently operable and
exportable.

**Release boundary:** Requires tenancy isolation, signed desired-state
rollout, recovery drill, portability proof, and no cross-plane
plaintext/seed access by default.

**What C3 is not:** C3 is optional. It cannot become a required dependency
for private chat delivery.

### 7.5 Tier relationship diagram

```text
C0 Transport/Protocol  <-- foundation; all others depend on this
  |
  +-- C1 Bridge/Harness  <-- framework integration layer
  |
  +-- C2 Migration  <-- operator tooling for PCA-to-Polkagent transition
  |
  +-- C3 Managed Cloud  <-- optional fleet/collaboration layer
```

C0 must be solid before C1-C3 work begins. C1, C2, and C3 are independent
of each other but all require C0.

---

## 8. Feature-by-feature compatibility matrix

| # | PCA Feature | PCA Source Evidence | Polkagent Equivalent | Tier | Migration Path |
|---|---|---|---|---|---|
| F01 | Outbound-only encrypted chat | `/bot-core/index.mjs` | Rust transport port with typed connection/health state | C0 | Native implementation or JS codec bridge |
| F02 | Bot identity / registration | `/bot-core/lib/register.mjs` | `AgentIdentity` with secret references, rotation, backup | C0 | Import via `import-pca` command |
| F03 | Opener and follow-up sessions | `/docs/explanation/protocol.md` | Durable per-session records with protocol-vector tests | C0 | Import by validated one-time migration |
| F04 | Per-device channels | `/bot-core/vendor/app-chat-codec.mjs` | Dedicated session-watch projection and diagnostics | C0 | Preserved on import |
| F05 | Durable-before-ACK / dedup / owed work | `/bot-core/lib/session-store.mjs` | SQLite transaction: delivery + dedup + pending turn | C0 | Import pending work as `UnknownAfterMigration` |
| F06 | One-slot outbound lanes | `/bot-core/lib/outbound-lanes.mjs` | Durable lane/outbox with explicit uncertainty and event trace | C0 | Preserve pending outbound reconciliation |
| F07 | Direct Claude Code brain | `/bot-core/lib/runners.mjs` | `exec-claude-code` runner profile with version probe | C0 | Map PCA `brain` config to executor profile |
| F08 | Direct Codex brain | `/bot-core/lib/runners.mjs` | `exec-codex` profile with sandbox capability report | C0 | Map PCA `brain` config to executor profile |
| F09 | Direct OpenCode brain | `/bot-core/lib/runners.mjs` | `exec-opencode` profile with provider catalog | C0 | Map PCA `brain` config to executor profile |
| F10 | Direct echo brain | `/bot-core/lib/runners.mjs` | Built-in echo executor | C0 | Direct mapping |
| F11 | Custom JSONL brain | `/bot-core/lib/runners.mjs` | `jsonl-subprocess` executor with strict schema | C0 | Map command to executor config |
| F12 | Native CLI sessions | `/bot-core/lib/agent-runtime.mjs` | Opaque `ExecutorSessionRef` per conversation | C0 | Import resume IDs where compatible |
| F13 | Hermes adapter | `/hermes-plugin/polkadot/adapter.py` | C1 bridge + maintained migration adapter | C1 | Config template + credential migration guide |
| F14 | OpenClaw adapter | `/openclaw-plugin/polkadot/src/` | C1 bridge + maintained migration adapter | C1 | Config mapping + proactive-fence preservation |
| F15 | Bridge HTTP API (health) | `/docs/reference/bridge.md` | `GET /v1/compat/pca/health` | C1 | Endpoint rename + token migration |
| F16 | Bridge HTTP API (inbound) | `/docs/reference/bridge.md` | `GET /v1/compat/pca/inbound` | C1 | Same field names, versioned schema |
| F17 | Bridge HTTP API (ACK) | `/docs/reference/bridge.md` | `POST /v1/compat/pca/inbound/ack` | C1 | Same semantics |
| F18 | Bridge HTTP API (renew) | `/docs/reference/bridge.md` | `POST /v1/compat/pca/inbound/renew` | C1 | Same semantics |
| F19 | Bridge HTTP API (media) | `/docs/reference/bridge.md` | `GET /v1/compat/pca/media/:id` | C1 | Same capability model |
| F20 | Bridge HTTP API (files) | `/docs/reference/bridge.md` | `GET/PUT/DELETE /v1/compat/pca/files/:chat_id[/path]` | C1 | Same vault scoping |
| F21 | Bridge HTTP API (send) | `/docs/reference/bridge.md` | `POST /v1/compat/pca/send` | C1 | Same lease-fenced operations |
| F22 | Bridge HTTP API (react) | `/docs/reference/bridge.md` | `POST /v1/compat/pca/react` | C1 | Same capability gate |
| F23 | Bridge HTTP API (typing) | `/docs/reference/bridge.md` | `POST /v1/compat/pca/typing` | C1 | Same capability gate |
| F24 | Bridge proactive token | `/docs/reference/bridge.md` | Independent proactive authority | C1 | Regenerate by default |
| F25 | Files/media staging | `/bot-core/lib/hop-client.mjs` | Artifact store with trusted endpoint policy | C0 | Migrate vault manifest with hash/audit |
| F26 | Durable file vault | `/bot-core/lib/file-store.mjs` | Conversation-scoped artifact namespace | C0 | Import files with manifest and hashes |
| F27 | File commands | `/bot-core/lib/file-commands.mjs` | Core command service | C0 | Same command syntax |
| F28 | Live reply / thinking | `/bot-core/lib/live-reply.mjs` | Unified capability model with traceable progress | C0 | Same UX behavior |
| F29 | Long answer chunking | `/bot-core/lib/chunk.mjs` | UTF-8-safe, paragraph/code-fence-aware chunks | C0 | Same transport limits |
| F30 | Chat commands (`/help` etc.) | `/bot-core/lib/commands.mjs` | Core command service | C0 | Same command syntax |
| F31 | Portable tool policy | `/bot-core/lib/tool-policy.mjs` | `ResolvedGrant` with enforcement backend report | C0 | Map PCA capabilities to grants |
| F32 | Model switching | `/bot-core/lib/commands.mjs` | Core policy-evaluated model switch | C0 | Preserve policy (pinned/allowed/open) |
| F33 | Reasoning effort | `/bot-core/lib/commands.mjs` | Core policy-evaluated reasoning control | C0 | Preserve configuration |
| F34 | Projects/worktrees | `/bot-core/lib/workspaces.mjs` | Workspace grants with alias validation | C0 | Map PCA projects to workspace grants |
| F35 | Per-conversation scheduling | `/bot-core/lib/keyed-dispatcher.mjs` | One serial logical actor per conversation | C0 | Architectural decision |
| F36 | Private allowlist | `/docs/guide/access.md` | Policy-based access with audit records | C0 | Import allowlist |
| F37 | Public bot restrictions | `/docs/guide/access.md` | Admission controls + restricted model switching | C0 | Import posture |
| F38 | T3ams DMs | `/bot-core/transports/t3ams/` | First-class T3ams transport crate | C0 | Enable in transport profile |
| F39 | T3ams workspace mentions | `/bot-core/transports/t3ams/` | First-class T3ams transport crate | C0 | Enable in transport profile |
| F40 | T3ams threads | `/bot-core/transports/t3ams/` | First-class T3ams transport crate | C0 | Enable in transport profile |
| F41 | T3ams reactions | `/bot-core/transports/t3ams/` | First-class T3ams transport crate | C0 | Enable in transport profile |
| F42 | T3ams typing | `/bot-core/transports/t3ams/` | First-class T3ams transport crate | C0 | Enable in transport profile |
| F43 | T3ams rich media / BCTS | `/bot-core/transports/t3ams/` | T3ams artifact adapter | C0 | Enable in transport profile |
| F44 | T3ams channel context | `/bot-core/transports/t3ams/` | Background context labeling | C0 | Enable in transport profile |
| F45 | T3ams doctor | `/bot-core/transports/t3ams/t3ams-doctor.mjs` | Typed readiness diagnostics | C0 | Reimplemented in Rust |
| F46 | CLI create command | `/bot-core/cli.mjs` | `polkagent agent create` | C2 | Direct mapping |
| F47 | CLI run command | `/bot-core/cli.mjs` | `polkagent agent run` | C2 | Direct mapping |
| F48 | CLI info/status | `/bot-core/cli.mjs` | `polkagent agent status` | C2 | Direct mapping |
| F49 | CLI deploy | `/bot-core/cli.mjs` | `polkagent agent deploy` with manifest | C2 | Generated deployment migration |
| F50 | CLI doctor | `/bot-core/cli.mjs` | `polkagent agent doctor` | C2 | Reimplemented with typed checks |
| F51 | SSH deployment | `/docs/guide/deploy.md` | Deployment adapter for SSH targets | C2 | Manifest-based migration |
| F52 | Docker deployment | `/docs/guide/deploy.md` | Deployment adapter for Docker targets | C2 | Manifest-based migration |
| F53 | Two-container harness deploy | `/docs/guide/deploy.md` | Multi-container deployment manifest | C2 | Updated compose generation |
| F54 | Network profile config | `/bot-core/lib/network-config.mjs` | Versioned network profile data | C2 | Import profile mappings |
| F55 | Chain metadata/descriptors | `/bot-core/lib/descriptors.mjs` | Pinned metadata with drift detection | C2 | Import and re-verify |
| F56 | Testnet file allowance | `/bot-core/lib/testnet-file-allowance.mjs` | Profile-specific optional adapter | C2 | Named-testnet only |
| F57 | PCA state import | `/bot-core/lib/session-store.mjs` | `import-pca` migration command | C2 | Validated importer |
| F58 | Greeting on first start | `/bot-core/cli.mjs` | Durable first-contact effect with allowlist guard | C2 | Architectural decision |
| F59 | Fleet enrollment | N/A (new) | Optional control-plane enrollment | C3 | New capability |
| F60 | Fleet policy rollout | N/A (new) | Signed desired-state revisions | C3 | New capability |
| F61 | Managed workers | N/A (new) | Isolated managed worker deployment | C3 | New capability |
| F62 | Tenant isolation | N/A (new) | Database/encryption/bucket/quota isolation | C3 | New capability |

---

## 9. Legacy-to-canonical domain mapping

This table maps PCA's internal concepts and vocabulary to Polkagent's
canonical domain model. The left column is PCA terminology; the right column
is the Polkagent equivalent.

### 9.1 Core concepts

| PCA concept | PCA implementation | Polkagent canonical term | Notes |
|---|---|---|---|
| Bot | One PCA process serving one identity | Agent | An agent has a spec, identity, transport, executor, and policy |
| Brain | `claude`, `codex`, `opencode`, `echo`, `bridge` | Executor / TurnExecutor | One of several executor adapters; "brain" conflates model and execution path |
| Direct brain | Subprocess AI CLI | Direct executor / subprocess executor | CLI process spawned per turn |
| Bridge brain | External framework via HTTP | Bridge executor / harness | External process communicating via bridge API |
| Owed reply | Durable record of accepted, unprocessed message | Pending delivery / pending turn | Part of the delivery ledger |
| Session state | `session-state.json` | Delivery ledger (SQLite) | Transactional durable state |
| Lane / outbound lane | Per-peer outbound queue | Outbox lane | Durable per-channel outbox with lane state |
| Chat | Conversation with one peer | Conversation | Includes conversation ID, history, session |
| Opener | First encrypted message establishing session | Session opener | Transport-level concept |
| Follow-up | Subsequent encrypted message | Session follow-up | May arrive on device channels |
| Device channel | Per-device message channel | Device session channel | Transport-specific session projection |
| HOP | Attachment storage/delivery | Artifact materialization adapter | Transport-specific remote storage |
| Vault / file store | Per-conversation durable files | Artifact store | Conversation-scoped, content-hashed |
| Live reply | Progress/thinking/chunked response | Egress progress / streaming reply | Capability-negotiated UX feature |
| Tool policy | `read/write/bash/web/subagents` | ResolvedGrant / capability grant | Immutable, auditable capability set |
| Runner | Per-brain CLI adapter | TurnExecutor adapter | Normalized streaming event interface |
| Project | Registered directory with alias | Workspace grant | Policy-controlled, alias-validated |
| Worktree | Git worktree per conversation | Workspace isolation branch | Part of workspace grant lifecycle |
| Keyed dispatcher | Per-conversation serial queue | Conversation actor | One logical actor per conversation |
| Registration | Username registration on network | Agent registration | Network-profile-specific |
| Allowlist | Permitted sender accounts | Access policy / sender policy | Audit-producing policy evaluation |
| Bridge token | Authentication for bridge HTTP | Bridge credential | Separately rotatable secret reference |
| Proactive token | Authority for unleased bridge actions | Proactive authority credential | Independent from bridge token |

### 9.2 Transport concepts

| PCA concept | Polkagent canonical term |
|---|---|
| Statement Store channel | Transport channel |
| Statement request ID | Outbox submission ID |
| Peer ACK | Outbound peer acknowledgement |
| Source ACK | Inbound transport acknowledgement |
| Topic derivation | Session channel derivation |
| Batch item | Inbound transport item |
| Current slot | Active outbound statement |
| Superset extension | Outbound statement merge |

### 9.3 Configuration concepts

| PCA concept | Polkagent canonical term |
|---|---|
| Environment variables | Config values + secret references |
| Network config | Network profile |
| Descriptors | Chain metadata / runtime descriptors |
| Brain config | Executor profile |
| Deploy config | Deployment manifest |
| Bot directory | Agent data directory |

---

## 10. Replacement architecture

### 10.1 Design principles

The replacement architecture decomposes PCA's monolithic Node.js process into
independently versioned, testable Rust crates. Each crate owns exactly one
concern and communicates through typed ports (trait boundaries). The principles
are:

1. **Transport is independent of execution.** The transport authenticates,
   receives, ACKs, and sends. It does not decide which model to run.
2. **Conversation is independent of transport.** A conversation actor manages
   turn ordering and context. It works the same whether messages arrive from
   Polkadot App chat, T3ams, or a future transport.
3. **Execution is independent of conversation.** A `TurnExecutor` takes a
   turn request and emits normalized events. It does not know about transport
   encryption or outbound lanes.
4. **Harness lifecycle is independent of execution.** A harness service has
   its own health, version, and lifecycle management. It is not the same
   thing as executing a turn.
5. **Files and artifacts are independent of transport storage.** The artifact
   store is conversation-scoped and content-addressed. Transport-specific
   storage (HOP, BCTS) is an adapter behind the artifact port.
6. **Capabilities are independently resolved.** A `ResolvedGrant` is computed
   from policy before execution begins. The executor receives an immutable
   grant, not policy rules to evaluate.
7. **Deployment is independently specified.** A deployment manifest declares
   what is needed (image, config, secrets, volumes, network). The deployment
   adapter applies it to a specific target.

### 10.2 Crate map

```text
polkagent-types         IDs, schemas, safe envelopes, protocol versions
polkagent-core          Pure delivery/turn/effect/outbox transitions and ports
polkagent-runtime       Keyed actors, admission, leases, cancellation, recovery
polkagent-store         SQLite/WAL events/projections/outbox/leases/migrations
polkagent-artifacts     Staging, vaults, content hashes, classification, retention
polkagent-policy        Sender/model/tool/workspace/effect/egress decisions
polkagent-executor      Normalized event protocol + process/provider adapters
polkagent-transport     Normalized records + contract suite
 +-- polkadot-app       Fixture-gated PCA-compatible transport;
 |                      JS codec bridge (Node→Rust FFI) required initially
 |                      while native Rust codec is deferred
 +-- t3ams              Rich chat transport
polkagent-bridge-http   C1 projection/leases/files/media
polkagent-control       Optional control-plane API/agent enrollment/fleet policy
polkagent-ui            Optional web/app approvals and operator experience
polkagent-cli           Local operator workflow and deploy targets
```

**Dependency direction:** `polkagent-types` is at the center.
`polkagent-core` depends only on `polkagent-types`. All adapters
(transport implementations, executor implementations, store implementations)
are leaf crates that depend on core types and traits but never on each
other. The CLI depends on everything; nothing depends on the CLI.

### 10.3 Domain abstractions

| Abstraction | Owns | Must not own |
|---|---|---|
| `Transport` | Identity/session protocol, receive/ACK, safe artifact fetch, transport capabilities, egress submit | Model selection, persistent core state, broad policy |
| `DeliveryLedger` | Dedup, durable acceptance, lease, source cursor, origin/request IDs | Provider/harness-specific sessions |
| `ConversationActor` | Ordered turn/context policy and cancellation for one conversation/thread | Direct SQL/HTTP/chain codec calls |
| `EffectIntent` | Immutable request to invoke executor/tool/signer/egress with digest and idempotency class | Inference or access policy |
| `TurnExecutor` | Runs an immutable turn request and emits normalized events | Transport key, store handle, authority expansion |
| `HarnessService` | Optional daemon lifecycle/health/version for an external framework | Conversation scheduling or state ownership |
| `ArtifactStore` | Conversation-scoped read/write handles, classification, retention and safe materialization | Arbitrary caller filesystem paths |
| `Signer` | Exact canonical-payload authorization/signature | Broad wallet/seed exposure; authority chosen by a model |
| `ControlPlane` | Fleet enrollment, desired config/policy release, rollout, telemetry summary | User plaintext, bot seed, session keys, raw artifacts by default |

### 10.4 Durable state machine

```text
received
  --> accepted(dedup + pending durable)
    --> source-ack-pending / acked
      --> leased to executor or bridge
        --> running
          --> execution terminal
            --> egress intents durable
              --> lane submission
                --> confirmation / reconcile
```

Each external action has its own `EffectIntent`, one or more `EffectAttempt`s,
a policy/grant digest, deadline, idempotency classification, and an immutable
`EffectOutcome` for every completed attempt.

**Critical distinctions:**
- `accepted` means a source message is durable and may be ACKed; it does not
  mean the model ran.
- `execution_succeeded` means an executor completed an attempt; it does not
  mean a peer saw a reply.
- `submitted` means an outbound transport accepted a statement/action; it
  does not necessarily mean the peer fetched it or that a chain action
  finalized.
- An uncertain external effect is reconciled using its canonical request and
  receipt; it is never blindly re-run because a process restarted.

### 10.5 Minimal durable records

```text
Delivery {
  id, transport, remote_request_id, remote_message_id,
  conversation_id, sender_id, dedup_key, source_ack_state,
  accepted_at, lease_state
}
Turn {
  id, delivery_id, executor_profile_revision, resolved_grant_digest,
  state, active_attempt_id, executor_session_ref?, terminal_outcome?
}
EffectIntent {
  id, turn_id, kind(execute|tool|egress|sign), canonical_request_digest,
  idempotency_class, deadline, state
}
EffectAttempt {
  id, effect_intent_id, attempt_no, idempotency_key,
  lease_owner?, lease_expires_at?, started_at?, completed_at?, state
}
EffectOutcome {
  id, effect_attempt_id, observed_at,
  status(success|failure|timeout|cancelled|unknown),
  result_artifact?, error_artifact?, external_reference?
}
OutboxItem {
  id, conversation_id, ordinal, lane_key, message_id, kind,
  payload_artifact, reply_or_edit_target?, supersedes[], submission_state
}
Artifact {
  id, conversation_id, classification, content_hash, size, mime,
  retention, transport_reference?
}
```

---

## 11. State, config and identity import/export

### 11.1 Import preconditions

1. Operator stops PCA first. No two services may serve one identity
   simultaneously.
2. The migration command validates PCA bot directory/config/secret/state
   file permissions and versions before writing any Polkagent state.
3. Rollback remains possible until a signed/exported cutover confirmation.
   PCA data is never deleted during import.

### 11.2 Import field mapping

| PCA state element | Source | Import behavior |
|---|---|---|
| Bot account/identifier | Seed + derived keys | Import to `AgentIdentity` with secret reference; seed stored in configured secret backend |
| Session material | `session-state.json` | Transform into SQLite session records transactionally using a versioned schema; retain source hash/version; report unsupported fields |
| Allowlist | Config/env | Import as sender access policy entries |
| Brain/model/tool policy | Config/env | Map to executor profile + `ResolvedGrant` template |
| Project aliases | Config | Map to workspace grant entries |
| Settings | Config/env | Map to typed `config.toml`; unknown env variables reported, not silently dropped |
| Dedup markers | `session-state.json` | Import to delivery ledger dedup table |
| Pending owed replies | `session-state.json` | Import as `Turn { state: UnknownAfterMigration }`; operator reviews before retry |
| Durable vault files | File store directory | Import with manifest, content hashes, and audit trail |
| Unresolved outbound state | `session-state.json` + in-memory | Import where available; flag uncertain state |
| Bridge token | Config/env | Regenerate by default; short compatibility window only if explicitly requested and logged |
| Deployment configuration | Environment/compose | Generate import report + typed `config.toml` draft |

### 11.3 Import safety rules

| Rule ID | Rule |
|---|---|
| IMP-01 | `session-state.json` is an input format, not a live database. Validate, bound, and transform into SQLite rows transactionally. Retain source hash/version and report every dropped/unsupported field. |
| IMP-02 | Every owed/pending action imports as `ExecutionStatus::UnknownAfterMigration`. Never automatically re-run an irreversible external effect. Operator chooses review/retry/abandon with evidence. |
| IMP-03 | Bridge tokens are regenerated by default. A deliberate short compatibility window may retain old token only if explicitly requested and logged. |
| IMP-04 | Old deployment configuration becomes a generated import report plus a typed `config.toml` draft. Unknown env variables fail validation. |
| IMP-05 | Import refuses a live/ambiguous concurrent identity service and preserves a rollback export. |
| IMP-06 | No PCA file is modified during import. |

### 11.4 Export specification

Polkagent must support exporting agent state for:
- Backup and disaster recovery
- Migration between deployments (local to cloud, cloud to local, etc.)
- Audit and compliance

Export produces an encrypted archive containing:
- Agent identity (public keys and metadata; seed only with explicit authorization)
- Configuration (typed, without secrets)
- State snapshot (delivery ledger, turns, outbox, sessions)
- Artifact manifest with content hashes
- Artifact content (optionally, based on retention/size policy)
- Audit trail entries

The export format is versioned and self-describing. Import from an export
validates the version, integrity, and compatibility before applying.

### 11.5 Migration CLI

```text
polkagent import-pca <pca-bot-directory>
  --preview          Show what will be imported without writing
  --secret-backend   Where to store imported secrets (keychain|file|env)
  --skip-files       Skip vault file import
  --network-profile  Override network profile mapping

polkagent export <agent-id>
  --output           Output directory or archive path
  --include-artifacts Include artifact content
  --encrypt          Encrypt with specified key/method

polkagent migration verify <agent-id>
  Run post-migration verification checks

polkagent migration rollback <agent-id>
  Restore from pre-import backup point
```

---

## 12. Bridge and OpenAPI compatibility plan

### 12.1 Compatibility endpoint prefix

Polkagent exposes PCA compatibility at `/v1/compat/pca`. A temporary
unprefixed alias is optional and must be disabled by default after the
migration period.

### 12.2 C1 route specification

| Operation | Method/Path | Auth | Behavior |
|---|---|---|---|
| Health | `GET /v1/compat/pca/health` | Bearer or `x-bridge-token` | Identity, transport, media/files/live capabilities, degraded state. Read-only; no secrets. |
| Inbound | `GET /v1/compat/pca/inbound?wait=&limit=&events=` | Bearer | Bounded long-poll. Message rows leased with `delivery_id`, `lease_id`, `lease_ms`, `chat_id`, `text`, `message_id`. Signals opt-in via `events=1`. |
| ACK | `POST /v1/compat/pca/inbound/ack` | Bearer | Single/batch exact lease ACK. Stale claim acknowledges zero. |
| Renew | `POST /v1/compat/pca/inbound/renew` | Bearer | Exact active claim renewal. Finite cap/expiry remains observable. |
| Media | `GET /v1/compat/pca/media/:id` | Bearer | Authenticated opaque scoped bytes. May materialize lazy media. |
| Files | `GET/PUT/DELETE /v1/compat/pca/files/:chat_id[/path]` | Bearer | Same-conversation vault only. Type/size/path rules enforced. |
| Send | `POST /v1/compat/pca/send` | Bearer | Text/reply/edit/file operations. Returns durable outgoing `message_id`. Validates live lease/proactive authority. Creates ordered outbox intent atomically. |
| React | `POST /v1/compat/pca/react` | Bearer | Capability-gated, best effort. |
| Typing | `POST /v1/compat/pca/typing` | Bearer | Capability-gated, best effort. |

### 12.3 Bridge authentication

Every bridge request requires ordinary bridge authentication. Proactive
activity additionally requires an independent proactive token and only
authorizes entirely unleased actions; it cannot turn a stale lease into a
valid one. T3ams compatibility requires active delivery claim fencing for
outbound actions and prompt-edit/delete revocation behavior.

### 12.4 Example bridge exchange

```http
GET /v1/compat/pca/inbound?wait=25&limit=4 HTTP/1.1
Authorization: Bearer <bridge-token>
```

Response:
```json
[
  {
    "delivery_id": "d_01J...",
    "lease_id": "l_01J...",
    "lease_ms": 30000,
    "chat_id": "0xpeer-account-or-t3ams:dm:...",
    "message_id": "remote-message-id",
    "kind": "richText",
    "text": "Please summarize this photo",
    "attachments": [{
      "id": "attachment-id",
      "mime": "image/jpeg",
      "size": 245123,
      "media_id": "opaque-media-capability",
      "url": "/v1/compat/pca/media/opaque-media-capability"
    }]
  }
]
```

Lease renewal while working:
```http
POST /v1/compat/pca/inbound/renew HTTP/1.1
Authorization: Bearer <bridge-token>
Content-Type: application/json

{
  "delivery_id": "d_01J...",
  "lease_id": "l_01J..."
}
```

Sending a reply (lease-fenced):
```http
POST /v1/compat/pca/send HTTP/1.1
Authorization: Bearer <bridge-token>
Content-Type: application/json

{
  "chat_id": "0xpeer-account-or-t3ams:dm:...",
  "text": "I found a cat sitting on a windowsill.",
  "delivery_id": "d_01J...",
  "lease_id": "l_01J..."
}
```

The bridge atomically checks the lease and creates an ordered outbox item.
A stale lease returns a conflict and creates no send. After the harness has
safely completed its turn, it calls `/inbound/ack`; if it crashes instead,
the lease expires and Polkagent re-leases the delivery.

### 12.5 OpenAPI specification

Polkagent will publish an OpenAPI 3.1 specification for the C1 bridge that:
- Documents every route, field, and error code
- Includes JSON Schema for request and response payloads
- Provides executable examples for common flows
- Is versioned alongside the bridge implementation
- Can be used to generate client libraries for harness development

### 12.6 Legacy adapter support

| Adapter | Compatibility approach | Migration support |
|---|---|---|
| **Hermes** | Adapter package/config template preserving bounded keyed dispatch, lease renewal, attachment materialization, reply handoff | OAuth credential migration guide; token not imported by default |
| **OpenClaw** | Retain account/config mapping, allowlist defense in depth, per-chat/thread keyed dispatch, attached-result proactive capability, delivery routing | Framework policy remains additional defense; core remains authoritative |
| **Custom pollers** | OpenAPI + JSON Schema + executable C1 fixtures | Endpoint/version/token configuration changes only for baseline flows |

### 12.7 Future bridge evolution

The C1 polling bridge is a migration layer. Future evolution may include:

- **Versioned schema:** `/v2/bridge/` with capability negotiation
- **Unix socket / gRPC:** Local IPC for lower-latency harness communication
- **SSE / WebSocket:** Server-push for real-time event delivery
- **Process SDK:** Rust and TypeScript libraries for native harness development

These are additive. The C1 endpoint remains available for backward
compatibility during migration periods.

---

## 13. Native and compatibility test corpora

### 13.1 Test corpus structure

```text
tests/
  compat/
    c0-transport/
      opener-encryption/        Byte-level opener/session vectors
      device-channels/          Device-channel follow-up tests
      dedup-restart/            Dedup and restart recovery
      outbound-lane/            Lane ordering, superset, peer ACK
      batch-defensive/          Malformed batch item isolation
      attachment-security/      HOP/BCTS boundary and integrity
      live-reply/               Progress, edit, chunk, final
      t3ams-dm/                 T3ams DM lifecycle
      t3ams-workspace/          T3ams workspace mention/thread
      t3ams-media/              T3ams BCTS media handling
    c1-bridge/
      health/                   Health endpoint verification
      inbound-lease/            Lease lifecycle (claim, renew, expire, stale)
      ack/                      ACK semantics (valid, stale, batch)
      send-fenced/              Lease-fenced send operations
      media/                    Media materialization and scoping
      files/                    File vault operations and path traversal
      proactive/                Proactive token and authority
      hermes-compat/            Hermes adapter fixture
      openclaw-compat/          OpenClaw adapter fixture
    c2-migration/
      import-identity/          Identity import and secret management
      import-state/             State transformation and validation
      import-files/             Vault file import with integrity
      import-pending/           Pending work import as unknown
      rollback/                 Rollback verification
      config-mapping/           Configuration transformation
      deploy-parity/            Deployment workflow equivalence
    c3-cloud/
      disconnect-local/         Local operation during cloud disconnect
      tenant-isolation/         Cross-tenant access prevention
      config-rollout/           Signed config revision lifecycle
      recovery-drill/           Backup/restore/revocation exercise
      portability/              Export/import between deployments
  native/
    delivery-ledger/            Pure state machine property tests
    turn-lifecycle/             Turn/effect/attempt/outcome transitions
    outbox/                     Outbox ordering and submission
    policy/                     Grant resolution and enforcement
    executor/                   Normalized event protocol
    artifacts/                  Content hashing, classification, retention
    actor/                      Conversation actor ordering
```

### 13.2 C0 transport acceptance tests

| Test ID | Test | Pass criteria |
|---|---|---|
| C0-T01 | Byte-level opener encryption vectors | Ciphertext matches PCA output for known inputs at pinned commit |
| C0-T02 | Device-channel follow-up | Bot receives follow-up on device channel, not just identity channel |
| C0-T03 | Durable-before-ACK | Simulated persistence failure prevents source ACK; after recovery and resend, exactly one delivery/turn exists |
| C0-T04 | Dedup on restart | Process restart + message resend produces exactly zero duplicate turns |
| C0-T05 | Malformed batch isolation | One invalid batch item does not prevent valid siblings from being accepted |
| C0-T06 | Outbound lane ordering | Two sequential replies arrive in order; first is not overwritten by second |
| C0-T07 | Superset extension | While first reply is unacknowledged, second reply is merged as superset |
| C0-T08 | Peer ACK tracking | ACKs for superseded request IDs are ignored; only current request ID ACK advances lane |
| C0-T09 | Unreachable peer | Lane queue remains bounded; takeover (if enabled) is recorded |
| C0-T10 | Attachment endpoint security | Raw tickets never reach executor; allowlisted endpoint, size cap, integrity check |
| C0-T11 | Live reply placeholder ACK | Placeholder is fetched/ACKed before edit; otherwise edit supersedes |
| C0-T12 | Final-as-new-message | Final answer is sent as new message after compact placeholder |
| C0-T13 | Long answer chunking | UTF-8-safe, paragraph/code-fence-aware chunks within transport limits |
| C0-T14 | Subscription + polling | Both paths deliver messages; polling recovers from missed subscription events |
| C0-T15 | JS codec bridge interop | If JS codec bridge is used, same fixtures pass across bridge boundary |
| C0-T16 | Real device smoke test | Disposable identity bot answers real phone opener, device follow-up, and restart |
| C0-T17 | T3ams DM lifecycle | DM open, message, reply, thread, reaction |
| C0-T18 | T3ams workspace mention | Mention in channel, contextual reply in thread |
| C0-T19 | T3ams channel context | Context is background, not independent message |
| C0-T20 | T3ams prompt-edit revocation | Edited/deleted prompt revokes active lease |

### 13.3 C1 bridge/harness acceptance tests

| Test ID | Test | Pass criteria |
|---|---|---|
| C1-T01 | Lease lifecycle | Poll, receive lease, renew, ACK -- full happy path |
| C1-T02 | Stale lease rejection | Expired/mismatched lease cannot ACK, send, edit, react, or type |
| C1-T03 | Cancelled lease | Cancelled or prompt-invalidated lease creates no outbox items |
| C1-T04 | Signal opt-in | `events=1` exposes signals; default polling does not create auto-reply to reaction |
| C1-T05 | Media scoping | Media ID scoped to conversation; cross-conversation access denied |
| C1-T06 | File vault path traversal | Path traversal attempts denied; cross-chat access denied |
| C1-T07 | File quota | File size/count quota enforced |
| C1-T08 | Proactive send | Proactive token allows unleased send; stale lease + proactive token does not validate lease |
| C1-T09 | Hermes adapter fixture | Hermes poll/lease/renew/ACK/send flow completes |
| C1-T10 | OpenClaw adapter fixture | OpenClaw poll/lease/renew/ACK/send flow completes with proactive |

### 13.4 C2 migration acceptance tests

| Test ID | Test | Pass criteria |
|---|---|---|
| C2-T01 | Import mapping report | Complete report for identity, config, allowlist, sessions, files, projects, bridge settings, unsupported fields |
| C2-T02 | No PCA modification | No PCA file modified during import |
| C2-T03 | Live service refusal | Import refuses live/ambiguous concurrent identity service |
| C2-T04 | Rollback preservation | Rollback export preserved and functional |
| C2-T05 | Pending work review | Every pending owed reply/outbound uncertainty in operator review queue |
| C2-T06 | No automatic replay | No irreversible effect automatically replayed |
| C2-T07 | Doctor post-import | Identity, state, transport, provider, workspace, file delivery all validated |
| C2-T08 | Config validation | Unknown env variables fail validation, not silently apply |

### 13.5 C3 control-plane acceptance tests

| Test ID | Test | Pass criteria |
|---|---|---|
| C3-T01 | Cloud disconnect | Local agent continues serving with degraded-management status |
| C3-T02 | Tenant isolation | Cross-tenant access denied and audited |
| C3-T03 | Config revision safety | Signed desired-config cannot silently alter active grant or reveal secrets |
| C3-T04 | Recovery drill | Backup/recovery, revocation, bridge-token rotation, fleet pause all exercised |
| C3-T05 | Portability proof | Export from managed, import to local (and vice versa) preserves behavior |

---

## 14. UX improvements without breaking consumers

### 14.1 Principle

Improvements must be additive. No existing PCA user or framework integrator
should experience a regression. New capabilities are opt-in or
backward-compatible.

### 14.2 Improvement catalog

| PCA limitation | Polkagent improvement | Backward compatibility |
|---|---|---|
| Large Node.js entrypoints intermix transport/runtime/bridge state | Rust leaf crates with pure state transitions | Same external behavior |
| JSON snapshot state is atomic but coarse | SQLite/WAL delivery ledger with transactional guarantees | Improved reliability; same functional behavior |
| Bridge is polling-only generic ABI | Keep C1 long-poll; add versioned OpenAPI, Unix socket/gRPC/SSE later | C1 endpoint unchanged; new endpoints are additive |
| Direct vs bridge policies/commands differ | Core policy/command service with one durable turn model | Consistent behavior across execution paths |
| Tool enforcement depends on CLI capabilities | Resolved grant plus explicit enforcement backend report | Same capability names; better verification |
| Owed/direct completion is implementation-specific | First-class EffectIntent/attempt/outbox states | Improved observability; same user behavior |
| Single-host deployment focus | Portable data-plane contract with multiple deployment adapters | Local operation unchanged; fleet optional |
| Minimal operational visibility | Correlated timeline, metrics, doctor/remediation, config/policy diff | Operator tools are additive |
| T3ams/default transport code divergence risk | Both implement same typed transport/effect/artifact contracts | Same user behavior; better maintainability |

### 14.3 Conversation UX improvements

- **Delivery state legibility:** Show received, queued, working, waiting for
  approval, sending, delivered/uncertain/retry -- not merely "the model is
  thinking."
- **Compact progress:** Preserve PCA's placeholder/progress/final message
  pattern. Show direct model token/cost only when trustworthy and policy
  allows.
- **Scoped artifacts:** Render attachments/files as scoped artifacts with
  expiry/retention and clear explicit "save to chat" action.
- **Structured commands:** Capability-sensitive commands yield structured
  explanation/action cards rather than vague model prose.
- **T3ams context labeling:** Thread/channel context is visibly labeled
  background context; reply routed to triggering thread.

### 14.4 Operator UX improvements

- **Agent overview:** Single view showing identity, transport health, config
  revision, executor health, queue depth, artifact storage, costs, and
  recent failures.
- **Run timeline:** Causal delivery to policy to executor/tool/effect to
  egress evidence with redaction/classification. Differentiates execution
  success from actual delivery/finality.
- **Doctor:** Actionable repair plans for: chain endpoint unavailable, bridge
  token mismatch, metadata drift, provider login expired, file allowance
  unavailable, T3ams SDK/key mismatch, insufficient disk/quota, stale
  lease, deployment-policy conflict.
- **Approval cards:** Canonical trusted UI showing actor/requestor, network,
  account, recipient, asset/amount/fees, tool/process effects, policy
  revision, exact payload hash, expiry, and expected proof. Model text is
  supplemental, never authoritative.
- **Migration center:** Previews PCA import mapping, secrets requested,
  unsupported config, pending work disposition, test results, and rollback
  point.

### 14.5 First-run experience

```text
1. Choose mode: local private bot, local builder agent, T3ams workspace
   agent, import PCA, or connect a managed tenant.
2. Identity card: account, username/status, network, transport capability,
   backup/secret location, no exposure of seed.
3. Provider/harness card: model/provider, credential readiness, tool grant,
   workspace scope, budget, and exact enforcement limitations.
4. Test before trust: deterministic echo/local test plus actual transport
   self-test; no blank "running" state.
5. Operator-ready deploy: deployment plan diff, secrets required,
   volumes/network mounts, capability warning, health/rollback readiness.
```

---

## 15. Rolling migration strategy

### 15.1 Migration phases

```text
Phase 0: Assessment
  - Operator runs `polkagent import-pca --preview <pca-dir>`
  - Produces a detailed mapping report: what will import, what is
    unsupported, what requires manual decisions
  - No state is modified

Phase 1: Import
  - Operator stops PCA
  - Runs `polkagent import-pca <pca-dir>`
  - PCA state is read, validated, transformed, and written to Polkagent
    SQLite database
  - PCA data is not modified
  - Rollback export is created

Phase 2: Verification
  - Operator runs `polkagent migration verify`
  - Checks: identity integrity, session material, config completeness,
    pending work disposition, file vault integrity, transport connectivity,
    provider/harness readiness, workspace grants
  - Any failure blocks cutover

Phase 3: Trial run
  - Operator runs `polkagent agent run` with the imported agent
  - Uses echo brain or a controlled test conversation to verify transport
  - Monitors delivery, ACK, outbound lane, and bridge (if applicable)

Phase 4: Cutover
  - Operator confirms cutover with `polkagent migration cutover`
  - This is the point of no return; rollback is still possible from the
    Phase 1 export but requires manual work
  - Bridge tokens regenerated (unless compatibility window requested)
  - Agent is now running on Polkagent

Phase 5: Post-migration
  - Operator reviews pending work queue (UnknownAfterMigration items)
  - Decides review/retry/abandon for each with evidence
  - Updates deployment configuration to Polkagent format
  - Notifies harness integrators of endpoint and token changes
```

### 15.2 Rollback

Rollback is always possible:

- **Before cutover:** `polkagent migration rollback` restores from Phase 1
  export. PCA directory was never modified.
- **After cutover:** Operator can restore PCA from its unchanged directory
  and the Phase 1 export. Manual reconciliation may be needed for any work
  completed during the Polkagent trial period.

### 15.3 Parallel operation prohibition

Two services must never serve the same identity simultaneously. This is
enforced by:
- Import refusing if PCA appears to be running (PID check, lock file)
- Clear documentation that parallel operation corrupts transport state
- Cutover creating a sentinel file that prevents PCA restart

### 15.4 Harness migration timeline

```text
Week 0:  Announce migration plan to harness integrators
Week 1:  Publish C1 endpoint documentation and test fixtures
Week 2:  Provide adapter migration guides for Hermes and OpenClaw
Week 4:  Begin offering C1 compatibility testing
Week 8:  Complete migration; C1 endpoint stable
Week 12: Unprefixed alias disabled by default (if it was enabled)
```

This timeline is illustrative. Actual dates depend on implementation progress.

---

## 16. User journeys

### 16.1 Success: mobile user messages bot

```text
1. User opens Polkadot App, navigates to the bot's chat.
2. User types a message and sends it.
3. Phone encrypts and submits the message as a Statement Store statement.
4. Bot discovers, decrypts, validates, and durably accepts the message.
5. Bot sends transport ACK; phone stops showing "sending" indicator.
6. Bot dispatches to executor. Phone may see "thinking" indicator.
7. Executor produces a response; bot records it as a durable outbox item.
8. Bot encrypts and submits the response in the correct lane position.
9. User's phone displays the response in the chat thread.
```

### 16.2 Success: operator migrates from PCA

```text
1. Operator previews import with `polkagent import-pca --preview`.
2. Reviews mapping report; resolves any manual decisions.
3. Stops PCA process.
4. Runs `polkagent import-pca`.
5. Runs `polkagent migration verify` -- all checks pass.
6. Starts Polkagent with echo brain for trial.
7. Sends test message from phone; receives response.
8. Confirms cutover with `polkagent migration cutover`.
9. Switches to production brain; reviews pending work queue.
```

### 16.3 Success: harness integrator migrates

```text
1. Integrator receives migration notice with new endpoint prefix and docs.
2. Updates bridge URL from `/inbound` to `/v1/compat/pca/inbound`.
3. Updates bridge token (regenerated during migration).
4. Runs C1 fixture tests against new endpoint -- all pass.
5. Resumes production operation.
```

### 16.4 Denial: sender not in allowlist

```text
1. Unauthorized user sends a message to a private bot.
2. Bot validates sender against access policy.
3. Policy denies: sender not in allowlist.
4. Bot records the denial as an audit event.
5. Message is not processed; no model is invoked.
6. Bot may optionally send a polite "not authorized" response
   (configurable).
```

### 16.5 Cancellation: user cancels during model processing

```text
1. User sends a message; bot dispatches to executor.
2. User sends `/stop` command.
3. Bot receives and validates the stop command.
4. Bot cancels the active turn: kills the executor process (direct brain)
   or revokes the bridge lease (bridge brain).
5. Partial work is recorded as a cancelled effect.
6. Bot sends confirmation that processing was stopped.
```

### 16.6 Failure: executor crashes mid-turn

```text
1. User sends a message; bot dispatches to direct executor.
2. Executor process crashes unexpectedly.
3. Bot detects the crash (process exit, timeout).
4. Turn is marked as failed; owed reply record persists.
5. Bot may retry (configurable) or report failure to user.
6. If retry succeeds, user receives response.
7. If retries exhausted, user receives a "processing failed" message.
8. Operator sees the failure in the run timeline with evidence.
```

### 16.7 Failure: bridge lease expires

```text
1. User sends a message; bot leases it to bridge.
2. Harness crashes without ACKing or renewing.
3. Lease timer expires.
4. Bot reclaims the delivery and makes it available for re-lease.
5. Next harness poll receives the delivery again.
6. Harness processes it (with idempotency) and ACKs.
7. User receives response.
```

### 16.8 Unknown: outbound delivery uncertain

```text
1. Bot submits an encrypted reply to the Statement Store.
2. Network error or timeout makes submission result unknown.
3. Bot records the outbox item as submission_state::uncertain.
4. Polling/reconciliation loop checks whether the statement arrived.
5. If confirmed: mark as submitted, advance lane.
6. If not found: re-submit with the same content/message ID.
7. Client deduplicates by message ID if it arrives twice.
```

### 16.9 Recovery: process restart with pending work

```text
1. Bot process crashes or is restarted.
2. On startup, bot loads durable state from SQLite.
3. Pending deliveries (accepted but not completed) are identified.
4. Source ACK state is evaluated: re-ACK if needed, or let source retry.
5. Incomplete turns are retried or marked for operator review.
6. Outbound lane state is restored; submission continues from last known
   position.
7. Transport subscription and polling resume.
```

### 16.10 Migration: import with pending owed replies

```text
1. PCA had accepted messages but crashed before responding.
2. Import transforms owed replies to Turn { state: UnknownAfterMigration }.
3. Operator runs `polkagent migration verify` -- sees pending items.
4. Operator reviews each item in the pending work queue.
5. For each item, operator chooses: retry, abandon, or review further.
6. Retry dispatches the original message to the executor.
7. Abandon marks the turn as operator-cancelled with evidence.
```

---

## 17. Functional requirements

Requirements use stable IDs for traceability. Priority levels are:
- **P0:** Must be present at the specified compatibility tier
- **P1:** Should be present at launch; deferral requires explicit decision
- **P2:** Planned improvement; delivery timing flexible

### 17.1 Transport requirements

| ID | Requirement | Priority | Tier |
|---|---|---|---|
| FR-T01 | Implement or bridge PCA-compatible encrypted opener/follow-up/device-channel protocol | P0 | C0 |
| FR-T02 | Verify session opener identity proof before accepting work | P0 | C0 |
| FR-T03 | Watch all relevant channels including device-specific channels | P0 | C0 |
| FR-T04 | Persist dedup marker and pending delivery atomically before transport ACK | P0 | C0 |
| FR-T05 | Process batch items independently; one failure does not discard valid siblings | P0 | C0 |
| FR-T06 | Maintain per-channel durable outbound lane with superset extension | P0 | C0 |
| FR-T07 | Track outbound peer ACK by request ID | P0 | C0 |
| FR-T08 | Implement bounded queue with documented liveness takeover policy | P0 | C0 |
| FR-T09 | Use both subscription and polling for ingress | P0 | C0 |
| FR-T10 | Treat remote attachment endpoints as attacker input | P0 | C0 |
| FR-T11 | Implement live reply placeholder/edit/final-as-new-message behavior | P1 | C0 |
| FR-T12 | Implement UTF-8-safe, paragraph/code-fence-aware chunking | P1 | C0 |
| FR-T13 | Support T3ams DMs, workspace mentions, threads | P1 | C0 |
| FR-T14 | Support T3ams reactions, typing, rich media (BCTS) | P1 | C0 |
| FR-T15 | Implement T3ams channel context as background (not independent message) | P1 | C0 |
| FR-T16 | Implement T3ams prompt-edit/delete lease revocation | P1 | C0 |
| FR-T17 | Support network profile configuration (Devnet/Paseo) | P0 | C0 |

### 17.2 Execution requirements

| ID | Requirement | Priority | Tier |
|---|---|---|---|
| FR-E01 | Support direct executor for Claude Code CLI | P0 | C0 |
| FR-E02 | Support direct executor for Codex CLI | P0 | C0 |
| FR-E03 | Support direct executor for OpenCode CLI | P1 | C0 |
| FR-E04 | Support echo executor for testing | P0 | C0 |
| FR-E05 | Support custom JSONL subprocess executor | P1 | C0 |
| FR-E06 | Normalize streaming events from all executors into common vocabulary | P0 | C0 |
| FR-E07 | Maintain opaque `ExecutorSessionRef` per conversation | P0 | C0 |
| FR-E08 | Invalidate session when compatibility scope changes | P0 | C0 |
| FR-E09 | Scrub executor environment of transport keys and seeds | P0 | C0 |
| FR-E10 | Bound stdout, time, and idle progress for direct executors | P0 | C0 |
| FR-E11 | Kill process groups on cancellation | P0 | C0 |
| FR-E12 | Preserve artifact snapshots before deleting staging directories | P1 | C0 |
| FR-E13 | Compile resolved grant into CLI-specific flags | P0 | C0 |

### 17.3 Conversation and command requirements

| ID | Requirement | Priority | Tier |
|---|---|---|---|
| FR-C01 | Maintain one serial logical actor per conversation | P0 | C0 |
| FR-C02 | Support bounded global concurrency with fair cross-conversation work | P0 | C0 |
| FR-C03 | Implement all PCA chat commands: help, reset, stop, ping, model, reasoning, project, usage, file | P0 | C0 |
| FR-C04 | Process commands in core service before executor dispatch | P0 | C0 |
| FR-C05 | Apply policy to model-switching and reasoning-effort commands | P0 | C0 |
| FR-C06 | Return explicit structured responses for unsupported/denied commands | P1 | C0 |

### 17.4 File and artifact requirements

| ID | Requirement | Priority | Tier |
|---|---|---|---|
| FR-F01 | Implement conversation-scoped artifact store with content hashing | P0 | C0 |
| FR-F02 | Support file commands: put, get, list, info, remove | P0 | C0 |
| FR-F03 | Enforce per-file, per-peer, and global caps | P0 | C0 |
| FR-F04 | Implement HOP client with trusted endpoint, integrity, size limits | P0 | C0 |
| FR-F05 | Implement bounded media cache with TTL | P0 | C0 |
| FR-F06 | Support framework file flow: upload to vault, then send by vault path | P1 | C1 |
| FR-F07 | Implement testnet file allowance as profile-specific adapter | P2 | C0 |

### 17.5 Policy and access requirements

| ID | Requirement | Priority | Tier |
|---|---|---|---|
| FR-P01 | Implement sender allowlist / access policy | P0 | C0 |
| FR-P02 | Support private (default) and public bot postures | P0 | C0 |
| FR-P03 | Implement portable tool policy (read, write, bash, web, subagents) | P0 | C0 |
| FR-P04 | Compute immutable ResolvedGrant before execution | P0 | C0 |
| FR-P05 | Restrict public-bot model switching | P0 | C0 |
| FR-P06 | Record all policy evaluations as audit events | P1 | C0 |

### 17.6 Bridge requirements

| ID | Requirement | Priority | Tier |
|---|---|---|---|
| FR-B01 | Implement C1 health, inbound, ACK, renew, media, files, send, react, typing routes | P0 | C1 |
| FR-B02 | Implement lease lifecycle with time-limited claims | P0 | C1 |
| FR-B03 | Reject stale, mismatched, cancelled, or prompt-invalidated leases | P0 | C1 |
| FR-B04 | Implement independent proactive authority tokens | P0 | C1 |
| FR-B05 | Support T3ams lease fencing for prompt-edit/delete | P1 | C1 |
| FR-B06 | Publish OpenAPI 3.1 specification | P1 | C1 |
| FR-B07 | Provide Hermes migration adapter | P1 | C1 |
| FR-B08 | Provide OpenClaw migration adapter | P1 | C1 |

### 17.7 Migration requirements

| ID | Requirement | Priority | Tier |
|---|---|---|---|
| FR-M01 | Import PCA identity, sessions, config, files, projects, pending work | P0 | C2 |
| FR-M02 | Validate PCA state before import; refuse live services | P0 | C2 |
| FR-M03 | Produce complete mapping report with unsupported field disclosure | P0 | C2 |
| FR-M04 | Import pending owed replies as UnknownAfterMigration | P0 | C2 |
| FR-M05 | Preserve rollback export | P0 | C2 |
| FR-M06 | Never modify PCA files during import | P0 | C2 |
| FR-M07 | Regenerate bridge tokens by default | P0 | C2 |
| FR-M08 | Fail on unknown env variables rather than silently applying | P0 | C2 |
| FR-M09 | Implement doctor for post-import verification | P0 | C2 |
| FR-M10 | Support export for backup and cross-deployment migration | P1 | C2 |

### 17.8 Deployment requirements

| ID | Requirement | Priority | Tier |
|---|---|---|---|
| FR-D01 | Support local deployment | P0 | C2 |
| FR-D02 | Support Docker deployment with generated manifests | P1 | C2 |
| FR-D03 | Support SSH deployment | P1 | C2 |
| FR-D04 | Support multi-container harness deployment | P2 | C2 |
| FR-D05 | Generate immutable deployment manifests with image digest, config revision, secret references | P1 | C2 |

### 17.9 Cloud and control-plane requirements

| ID | Requirement | Priority | Tier |
|---|---|---|---|
| FR-CL01 | Support optional control-plane enrollment | P1 | C3 |
| FR-CL02 | Local operation functional without control plane | P0 | C3 |
| FR-CL03 | Implement signed desired-state config revisions | P1 | C3 |
| FR-CL04 | Enforce tenant isolation in database, encryption, object store, and quotas | P0 | C3 |
| FR-CL05 | Support fleet pause, rollout, drain, credential rotation, extension revocation | P2 | C3 |
| FR-CL06 | Data plane reports applied config revision and can reject incompatible changes | P1 | C3 |

---

## 18. Non-functional requirements

| ID | Requirement | Category |
|---|---|---|
| NFR-01 | Transport ACK latency: source ACK within 500ms of receiving valid message (excluding model processing) | Performance |
| NFR-02 | Outbound lane submission: submit within 200ms of outbox item creation (when lane is clear) | Performance |
| NFR-03 | Bridge inbound response: return leased deliveries within the client's `wait` parameter | Performance |
| NFR-04 | SQLite durability: WAL mode with fsync on commit for all durable writes | Reliability |
| NFR-05 | Process restart recovery: resume all pending work within 5 seconds of restart | Reliability |
| NFR-06 | Executor isolation: no transport keys, seeds, or session material in executor environment | Security |
| NFR-07 | Artifact scoping: cross-conversation artifact access prevented at storage layer | Security |
| NFR-08 | Bridge authentication: per-request token validation; no session cookies | Security |
| NFR-09 | Import safety: PCA data never modified; rollback always available | Reliability |
| NFR-10 | Tenant isolation: cross-tenant access prevented at database, encryption, and API layers | Security |
| NFR-11 | Config validation: reject unknown or malformed configuration at startup, not at runtime | Usability |
| NFR-12 | Deployment manifest: reproducible from config + version; no shell-injected values | Security |
| NFR-13 | Memory footprint: idle bot under 64MB RSS | Performance |
| NFR-14 | Log structure: structured JSON logs with correlation IDs for delivery/turn/effect traces | Observability |
| NFR-15 | Graceful shutdown: complete or durably record in-flight work, flush and persist outbox lane, persist durable cursor offset, then exit | Reliability |

---

## 19. Architecture boundaries and dependencies

### 19.1 Dependency diagram

```text
                      polkagent-types
                           |
                     polkagent-core
                    /       |       \
          polkagent-  polkagent-  polkagent-
          store       policy     executor
            |           |          |   \
      polkagent-   polkagent-    exec-  exec-
      runtime     artifacts    claude  codex  ...
         |             |
   polkagent-    polkagent-
   transport     bridge-http
    /      \
polkadot-  t3ams
app
                              polkagent-cli
                             (depends on all)

                           polkagent-control
                           (optional; depends on types, core)
```

### 19.2 Cross-PRD interfaces

| Interface | This PRD (PRD-06) requires | Provided by |
|---|---|---|
| `TurnExecutor` trait | Executor adapters for Claude/Codex/OpenCode/echo/JSONL | PRD-04 |
| `HarnessService` trait | Harness lifecycle/health for bridge-connected frameworks | PRD-04 |
| `ResolvedGrant` | Immutable capability set computed from policy | PRD-07 |
| `AgentIdentity` | Identity/key management with secret references | PRD-07 |
| `EffectIntent/Attempt/Outcome` | Effect lifecycle state machine | PRD-03 |
| `ConversationActor` scheduling | Per-conversation serial execution | PRD-03 |
| `ArtifactStore` | Content-addressed, conversation-scoped storage | PRD-10 |
| Deployment manifest format | Typed, reproducible deployment spec | PRD-11 |
| Tenant isolation model | Database/encryption/API isolation | PRD-11 |

### 19.3 Boundary rules

1. Transport crates never import executor crates.
2. Executor crates never import transport crates.
3. Store implementations never import transport or executor specifics.
4. Policy crates never import transport, executor, or store implementations.
5. Bridge-HTTP imports core types and store interfaces, never transport
   internals.
6. Control-plane imports core types only; never data-plane state directly.
7. CLI may import anything; nothing imports CLI.

---

## 20. Rust traits/types and wire/schema examples

### 20.1 Transport trait

```rust
/// A transport receives messages from external sources and submits
/// outbound messages. It owns session/identity protocol details but
/// not conversation state or execution decisions.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Unique transport identifier (e.g., "polkadot-app", "t3ams").
    fn transport_id(&self) -> &TransportId;

    /// Reported transport capabilities.
    fn capabilities(&self) -> TransportCapabilities;

    /// Start receiving messages. Returns a stream of inbound items.
    async fn start(&self, config: &TransportConfig) -> Result<InboundStream>;

    /// Send a transport-level ACK for an accepted delivery.
    async fn ack_inbound(&self, delivery_id: &DeliveryId) -> Result<()>;

    /// Submit an outbound item to the transport's lane.
    async fn submit_outbound(&self, item: &OutboxItem) -> Result<SubmissionReceipt>;

    /// Query the current outbound lane state.
    async fn lane_state(&self, conversation_id: &ConversationId) -> Result<LaneState>;

    /// Health check.
    async fn health(&self) -> TransportHealth;
}

pub struct InboundItem {
    pub remote_request_id: String,
    pub remote_message_id: String,
    pub sender_id: SenderId,
    pub conversation_id: ConversationId,
    pub kind: MessageKind,
    pub content: MessageContent,
    pub attachments: Vec<AttachmentRef>,
    pub received_at: Timestamp,
}

pub enum MessageKind {
    PlainText,
    RichText,
    Reply { target: MessageId },
    Edit { target: MessageId },
    Reaction { target: MessageId, emoji: String },
    Typing,
    Signal(SignalKind),
}

pub struct TransportCapabilities {
    pub live_reply: bool,
    pub edit: bool,
    pub reactions: bool,
    pub typing: bool,
    pub rich_text: bool,
    pub attachments: bool,
    pub threads: bool,
}
```

### 20.2 Delivery ledger types

```rust
pub struct Delivery {
    pub id: DeliveryId,
    pub transport: TransportId,
    pub remote_request_id: String,
    pub remote_message_id: String,
    pub conversation_id: ConversationId,
    pub sender_id: SenderId,
    pub dedup_key: DedupKey,
    pub source_ack_state: AckState,
    pub accepted_at: Timestamp,
    pub lease_state: LeaseState,
}

pub enum AckState {
    Pending,
    Acked,
    Failed { reason: String },
}

pub enum LeaseState {
    Unleased,
    Leased {
        lease_id: LeaseId,
        owner: LeaseOwner,
        expires_at: Timestamp,
    },
    Completed,
    Expired,
}
```

### 20.3 Turn and effect types

```rust
pub struct Turn {
    pub id: TurnId,
    pub delivery_id: DeliveryId,
    pub executor_profile_revision: u64,
    pub resolved_grant_digest: GrantDigest,
    pub state: TurnState,
    pub active_attempt_id: Option<AttemptId>,
    pub executor_session_ref: Option<ExecutorSessionRef>,
    pub terminal_outcome: Option<TerminalOutcome>,
}

pub enum TurnState {
    Pending,
    Leased,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    UnknownAfterMigration,
}

pub struct OutboxItem {
    pub id: OutboxItemId,
    pub conversation_id: ConversationId,
    pub ordinal: u64,
    pub lane_key: LaneKey,
    pub message_id: MessageId,
    pub kind: OutboxKind,
    pub payload_artifact: ArtifactId,
    pub reply_or_edit_target: Option<MessageId>,
    pub supersedes: Vec<OutboxItemId>,
    pub submission_state: SubmissionState,
}

pub enum SubmissionState {
    Pending,
    Submitted { at: Timestamp },
    PeerAcked { at: Timestamp },
    Uncertain { reason: String },
    Failed { reason: String },
}
```

### 20.4 Bridge C1 wire format

```rust
/// C1 bridge inbound delivery (JSON wire format)
#[derive(Serialize, Deserialize)]
pub struct BridgeDelivery {
    pub delivery_id: String,
    pub lease_id: String,
    pub lease_ms: u64,
    pub chat_id: String,
    pub message_id: String,
    pub kind: String,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<BridgeAttachment>,
}

#[derive(Serialize, Deserialize)]
pub struct BridgeAttachment {
    pub id: String,
    pub mime: String,
    pub size: u64,
    pub media_id: String,
    pub url: String,
}

/// C1 bridge ACK request
#[derive(Serialize, Deserialize)]
pub struct BridgeAckRequest {
    pub delivery_id: String,
    pub lease_id: String,
}

/// C1 bridge send request
#[derive(Serialize, Deserialize)]
pub struct BridgeSendRequest {
    pub chat_id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<String>,
}

/// C1 bridge health response
#[derive(Serialize, Deserialize)]
pub struct BridgeHealth {
    pub identity: BridgeIdentityInfo,
    pub transport: BridgeTransportInfo,
    pub capabilities: BridgeCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded: Option<Vec<String>>,
}
```

### 20.5 Configuration schema

```toml
# config.toml -- Polkagent agent configuration

[agent]
name = "my-bot"
network_profile = "products-devnet"
transport = "polkadot-app"  # or "t3ams"

[access]
posture = "private"  # or "public"
allowlist = ["5GrwvaEF...", "5FHneW46..."]

[executor]
type = "claude"  # or "codex", "opencode", "echo", "bridge", "custom"
# For custom:
# command = "/path/to/custom-brain"

[executor.capabilities]
read = true
write = true
bash = false
web = false
subagents = false

[model]
policy = "pinned"  # or "allowed", "open"
default = "claude-sonnet-4-6"
allowed = ["claude-sonnet-4-6", "claude-haiku-4-5"]

[projects]
# Workspace grants
[projects.my-project]
path = "/home/user/code/my-project"
branch_isolation = true

[bridge]
enabled = false
# token_secret_ref = "bridge-token"

[deployment]
non_root = true
```

---

## 21. Security, privacy, tenancy, custody and abuse implications

### 21.1 Security boundaries

| Boundary | Enforced by | Threat if violated |
|---|---|---|
| Transport identity/session from executor environment | `polkagent-runtime` scrubs env before spawn | Executor reads bot seed or session keys |
| Conversation-scoped artifacts | `polkagent-artifacts` enforces conversation ID on all operations | Cross-conversation file leak |
| Bridge lease fencing | `polkagent-bridge-http` checks active lease before every operation | Stale harness sends reply to wrong conversation |
| Executor tool permissions | `ResolvedGrant` computed before execution; executor cannot self-elevate | Prompt injection escalates to filesystem/network/shell |
| Untrusted content classification | Attachment content, URLs, and remote metadata are classified as attacker input | Prompt injection via attachment caption or filename |
| Control-plane/data-plane boundary | No plaintext, seeds, or session keys cross to control plane by default | Cloud admin reads private conversations or uses signing keys |

### 21.2 Privacy

- All chat content is end-to-end encrypted between user and bot.
- The control plane does not receive chat plaintext, session keys, bot seeds,
  or artifact content by default.
- Bot operates with outbound-only RPC; no inbound webhook exposes the
  operator's network.
- Managed custody (if chosen) is a separate explicit consent with its own
  keys, audit, and recovery.

### 21.3 Tenancy

- Tenant isolation is enforced at database query, encryption key hierarchy,
  object prefix/bucket policy, rate/cost quotas, and audit access levels --
  not only UI filters.
- An agent belongs to one environment at a time.
- Roles: `owner`, `operator`, `approver`, `auditor`, `developer`,
  `support-limited`. No support role views secrets/plaintext by default.
- Cross-tenant access attempts are denied and audited.

### 21.4 Custody

- Bot seed, session material, bridge token, provider credential, and
  payment signer are separate secrets with separate rotation/revocation.
- Secrets are referenced by ID, never stored as plaintext config values.
- Managed KMS/HSM/MPC custody is a separate explicit mode with its own
  consent, keys, recovery, rotation, revocation, and audit.

### 21.5 Abuse

| Risk | Mitigation |
|---|---|
| Public bot abuse: unknown senders exhaust budget/cause unsafe tools | Private/allowlist default; admission quotas; low-risk public profile; no public free model switching |
| Prompt injection via attachment | Untrusted content classification; immutable resolved grant; mediated tools; no secret in executor environment |
| Artifact cross-chat leak | Conversation-scoped handles/vaults; auth; capability TTL; quota; audit |
| Migration replay | Stop source service; import pending work as unknown; require review/reconciliation before retry |
| Misleading UX | Timeline and UI distinguish accepted, running, submitted, delivered, finalized, failed, and uncertain |

---

## 22. Observability and operator/support requirements

### 22.1 Structured logging

All log entries must include:
- Timestamp (UTC, sub-second precision)
- Correlation ID (delivery, turn, effect, outbox item)
- Component (transport, executor, bridge, policy, store)
- Level (trace, debug, info, warn, error)
- Structured data (not just message strings)

### 22.2 Metrics

| Metric | Type | Description |
|---|---|---|
| `polkagent_deliveries_total` | Counter | Total deliveries by transport, outcome |
| `polkagent_turns_total` | Counter | Total turns by executor, outcome |
| `polkagent_outbox_depth` | Gauge | Current outbox queue depth by conversation |
| `polkagent_lane_pending` | Gauge | Pending outbound items by lane |
| `polkagent_bridge_leases_active` | Gauge | Currently active bridge leases |
| `polkagent_bridge_leases_expired` | Counter | Expired leases by reason |
| `polkagent_executor_duration_seconds` | Histogram | Executor turn duration |
| `polkagent_transport_ack_latency_seconds` | Histogram | Time from receive to ACK |
| `polkagent_artifacts_stored_bytes` | Gauge | Total artifact storage by conversation |

### 22.3 Health checks

- Transport connectivity and subscription status
- Executor/harness availability and version
- Store integrity (SQLite PRAGMA integrity_check)
- Outbox drain rate and queue depth
- Bridge token validity
- Network profile endpoint reachability
- File storage capacity

### 22.4 Doctor command

`polkagent agent doctor` produces actionable repair plans:

| Check | Failure condition | Repair action |
|---|---|---|
| Chain endpoint | Unreachable or wrong network | Update RPC endpoint in config |
| Bridge token | Mismatch or expired | Regenerate with `rotate-secret` |
| Metadata drift | Local descriptors behind chain | Run metadata sync |
| Provider login | Expired API key or OAuth token | Update provider credential |
| File allowance | Unavailable on current network | Switch profile or fund allowance |
| T3ams SDK/keys | Mismatch or stale | Re-onboard T3ams identity |
| Disk/quota | Insufficient space | Clean artifacts or expand storage |
| Stale lease | Unresolved expired leases | Clear or review stale deliveries |
| Deployment policy | Config/manifest conflict | Reconcile deployment configuration |

### 22.5 Run timeline

The run timeline provides a causal trace from delivery through policy
evaluation through execution through egress:

```text
[2026-07-30 10:00:01] DELIVERY d_01 accepted from 5Grw...
  dedup: new | ack: sent
[2026-07-30 10:00:01] POLICY evaluated for d_01
  sender: allowed | model: claude-sonnet-4-6 | tools: read,write
[2026-07-30 10:00:02] TURN t_01 started
  executor: exec-claude-code | session: s_abc123
[2026-07-30 10:00:15] EFFECT e_01 tool:read /src/main.rs
  outcome: success | 2,340 bytes
[2026-07-30 10:00:45] TURN t_01 completed
  usage: 1,234 input / 567 output tokens
[2026-07-30 10:00:45] OUTBOX o_01 created
  kind: text | 1,203 chars | 1 chunk
[2026-07-30 10:00:46] LANE submitted o_01
  state: submitted | request_id: r_xyz
[2026-07-30 10:01:02] LANE peer_ack for r_xyz
  state: confirmed
```

---

## 23. Acceptance criteria and verification checklist

### 23.1 C0 transport acceptance

- [ ] Reproducible byte vectors for opener/follow-up encryption, topic/channel
      derivation, message IDs, ACKs, and attachment references at pinned PCA
      commit.
- [ ] Real/disposable client opens a chat, sends follow-ups through a distinct
      device channel, restarts the bot, and receives one ordered semantic
      reply per accepted message.
- [ ] Simulated persistence failure prevents source ACK; after recovery and
      resend, exactly one durable delivery/turn exists.
- [ ] Malformed item in a batch does not prevent valid siblings from being
      accepted.
- [ ] With a peer that does not fetch outbound work, the lane neither
      overwrites an earlier independent message nor leaks an unbounded
      queue; takeover (if enabled) is recorded.
- [ ] Attachment endpoint, size, integrity, cache, and vault boundary tests
      pass; raw encryption tickets never reach an executor/harness.
- [ ] If a temporary JS codec bridge is used, byte-level interoperability and
      the same restart/duplicate/lane fixtures pass across the bridge boundary.
- [ ] T3ams DM, workspace mention, thread, reaction, and typing function
      correctly (where T3ams profile is enabled).
- [ ] T3ams prompt-edit revokes active bridge lease.
- [ ] Channel context is treated as background, not an independent message.

### 23.2 C1 bridge/harness acceptance

- [ ] PCA-shaped Hermes/OpenClaw/custom fixtures can poll bounded deliveries,
      renew a lease, fetch authorized media, send a reply, and ACK it.
- [ ] Expired, mismatched, cancelled, or prompt-invalidated lease cannot ACK,
      send, edit, react, or type against the current conversation.
- [ ] `events=1` exposes signals; default polling does not accidentally
      create an agent reply to a reaction.
- [ ] Bridge file is sent only after it is written to the same chat vault;
      path traversal, cross-chat access, arbitrary host paths, and quota
      bypass fail.
- [ ] OpenAPI specification matches implemented behavior.

### 23.3 C2 migration acceptance

- [ ] Import produces a complete mapping report for identity, configuration,
      allowlist, sessions, files, projects, bridge settings, and unsupported
      fields; no PCA file is modified.
- [ ] Import refuses a live/ambiguous concurrent identity service and
      preserves a rollback export.
- [ ] Every pending owed reply/outbound uncertainty appears in an operator
      review queue; no irreversible effect is automatically replayed.
- [ ] `doctor` validates the imported identity, state permissions, transport
      connectivity, provider/harness readiness, workspace grants, and file
      delivery readiness before cutover.
- [ ] Unknown environment variables fail validation.
- [ ] Config transformation produces valid `config.toml` from PCA env.

### 23.4 C3 control-plane acceptance

- [ ] Disconnecting the cloud leaves a local enrolled agent able to serve its
      existing transport/policy and report a clear degraded-management state.
- [ ] A tenant can see only its agent metadata, summaries, and approved
      backup objects; cross-tenant access attempts are denied and audited.
- [ ] A signed desired-config revision cannot silently alter an active grant
      or reveal secrets; the data plane reports acceptance/rejection and
      reason.
- [ ] Backup/recovery, agent revocation, bridge-token rotation, and fleet
      pause are exercised in a documented recovery drill.
- [ ] Export from managed, import to local (and reverse) preserves behavior.

### 23.5 Manual verification checklist

The following checks require manual execution with real devices/services and
cannot be fully automated:

- [ ] Real phone (not test client) opens encrypted chat with bot
- [ ] Real phone sends follow-up on device channel
- [ ] Bot restart during pending work, followed by correct recovery
- [ ] Unknown batch item type does not break valid message processing
- [ ] Peer that never ACKs outbound -- lane behavior observed
- [ ] Attachment delivery end-to-end (upload, download, vault save)
- [ ] Reaction received without auto-reply
- [ ] Live response edits visible on phone
- [ ] T3ams DM, thread, and channel mention (where enabled)
- [ ] Stale bridge lease fencing observed
- [ ] Migration rollback from Phase 1 export
- [ ] Deployment recovery from generated manifest

---

## 24. Phased delivery and release gates

### 24.1 Phase 1: Foundation

**Goal:** Prove the durable state model works.

**Deliverables:**
- Local SQLite-ledger echo runtime with memory transport
- Delivery/turn/effect/outbox state machine implementation
- Property tests for duplicate/restart/outbox/lease invariants

**Gate:** All property tests pass. Durable-before-ACK invariant verified
under simulated crashes.

### 24.2 Phase 2: C0 transport spike

**Goal:** Prove transport compatibility with PCA.

**Deliverables:**
- PCA-codec/Statement Store compatibility fixtures at pinned commit
- JS codec bridge (Node→Rust FFI for crypto codec); native Rust transport
  is deferred until fixtures are green and the bridge is operational
- Rust state machine owning delivery ledger, outbox lane, and dedup;
  the bridge handles only the codec boundary
- Idempotent inbound processing (message-ID dedup table, durable offset
  persistence, graceful drain on shutdown)
- Disposable identity echo bot answering real messages

**Gate:** Byte-level fixture tests pass across the JS codec bridge boundary.
Real client interoperability confirmed. Idempotent processing verified under
simulated crash/restart. No real provider or cloud dependency.

### 24.3 Phase 3: C1 bridge

**Goal:** Prove harness compatibility.

**Deliverables:**
- `/v1/compat/pca/` health/inbound/ACK/renew/send/files/media routes
- Lease lifecycle implementation
- One Hermes or OpenClaw fixture migration

**Gate:** C1 acceptance tests pass. Existing harness completes a full
poll/lease/work/reply/ACK cycle.

### 24.4 Phase 4: Native runtime

**Goal:** Support real direct brains.

**Deliverables:**
- One direct runner (Claude Code) with scrubbed environment
- Explicit policy/grant compilation
- Staging/artifact management and cancellation
- CLI/doctor/deploy vertical slice

**Gate:** Claude Code runner processes a real message, respects tool
policy, and delivers an ordered reply. Doctor validates all subsystems.

### 24.5 Phase 5: T3ams

**Goal:** Full T3ams transport support.

**Deliverables:**
- Typed DM/thread/channel/media integration
- Rich-operation fencing
- T3ams readiness doctor

**Gate:** T3ams DM, workspace mention, thread, reaction, typing, and
media all function. Prompt-edit revocation tested.

### 24.6 Phase 6: Product companion

**Goal:** Optional enhanced UX surfaces.

**Deliverables:**
- PAPI/Product SDK/web approval and run-view UX
- Optional, non-critical to bot delivery

**Gate:** Companion renders canonical approval facts, binds response to
`ApprovalRequest` digest, reports cancellation/refusal, and leaves data
plane fully functional when host is unavailable.

### 24.7 Phase 7: Control plane

**Goal:** Optional fleet management.

**Deliverables:**
- Opt-in fleet enrollment
- Desired-config rollouts
- Redacted health/audit
- Tenant isolation

**Gate:** Tenant isolation proven. Recovery drill passed. Portability
proof (export managed, import local) verified. No paid tier until security
review complete.

### 24.8 Phase 8: Value-moving and public surfaces

**Goal:** Public bots and payment integration.

**Deliverables:** Only after payment/public-agent gates, legal/security
review, support/abuse operations, and independent risk acceptance.

**Gate:** See PRD-08 (Payments) for detailed gates.

### 24.9 Gate progression diagram

```text
Phase 1: Foundation
  |  property tests pass
  v
Phase 2: C0 Transport Spike
  |  byte-level fixtures + real client interop
  v
Phase 3: C1 Bridge             Phase 4: Native Runtime
  |  harness fixture pass        |  direct runner + doctor
  |                              |
  +----------+-------------------+
             |
             v
Phase 5: T3ams
  |  full T3ams transport verified
  v
Phase 6: Product Companion (optional)
  |  approval UX proven
  v
Phase 7: Control Plane (optional)
  |  tenant isolation + recovery drill
  v
Phase 8: Public/Payments (gated)
```

### 24.10 Staged implementation recommendations

The following recommendations summarize validated research findings on the
order and classification of implementation work for this domain.

| Stage | Work item | Rationale |
|---|---|---|
| **do-now** | JS codec bridge (Node→Rust FFI for App Chat crypto codec) | Native Rust transport not feasible this quarter; bridge the codec, own the state machine in Rust |
| **do-now** | Rust-owned state machine (delivery ledger, outbox lane, dedup) | Rust must own all durable state regardless of codec bridge |
| **do-now** | Idempotent inbound processing: message-ID dedup, durable cursor offset, graceful drain on shutdown | Statement Store is best-effort gossip; reliability must be composed at app layer |
| **validate-next** | App-layer ACK statements for outbound delivery confirmation | Confirms peer receipt without relying on Statement Store delivery guarantees |
| **validate-next** | Bulletin Chain CID anchoring for content that must outlive Statement Store TTL | Testnet only; ~2-week retention; anchor, do not rely on for retrieval |
| **defer** | Native Rust transport (replace JS codec bridge) | Blocked on byte-level fixture suite; bridge is the correct interim approach |
| **defer** | MLS group messaging | Infrastructure overhead not justified until group chat is a validated requirement |
| **avoid** | Treating Statement Store as a durable ordered queue | It is a best-effort gossip layer; ordered delivery and reliability are app-layer concerns |
| **avoid** | Committing Bandersnatch as an encrypted transport primitive | Polkadot-native but experimental in this context; requires dedicated spike before any commitment |

---

## Appendix A: Risks and required mitigations

| Risk | User-visible failure | Required mitigation |
|---|---|---|
| Protocol drift | Bot receives openers but misses phone follow-ups after an app/runtime update | Metadata/protocol fixture suite, device-channel live test, explicit incompatible/degraded status |
| ACK ordering bug | User repeatedly resends or loses a message after a crash | Transactional durable acceptance before source ACK; source retry on failure |
| Outbound slot overwrite | Only the last reply bubble appears | Per-channel durable lane, peer ACK tracking, superset extension and bounded queue |
| Stale harness worker | A cancelled/edited prompt receives an old answer later | Lease expiry/revocation fence checked at outbox creation and immediately before send/edit |
| Prompt injection / tool escalation | An attachment persuades the agent to read a seed or run an unapproved command | Untrusted content classification, immutable resolved grant, mediated tools, no secret in executor environment |
| Artifact cross-chat leak | One user fetches another conversation's file | Conversation-scoped handles/vaults, auth, capability TTL, quota, audit |
| Migration replay | Cutover repeats an old paid/tool action | Stop source service, import pending work as unknown, require review/reconciliation before retry |
| Cloud overreach | Hosted admin reads chat or uses a local bot's signing key | Local-first data plane; public-key enrollment; scoped desired-state; no plaintext/seeds by default |
| Misleading UX | "Done" means only a model finished, not that a reply/transaction arrived | Timeline and UI distinguish accepted, running, submitted, delivered, finalized, failed, and uncertain |
| Public-bot abuse | Unknown senders exhaust budget or cause unsafe tools | Allowlist/private default, admission quotas, low-risk public profile, no public free model switching |

---

## Appendix B: PCA source map reference

All paths below are relative to the PCA repository root at
`/Users/will/dev/par/polkadot-chat-agents`.

### System behavior and security

| Subject | PCA source | Successor consequence |
|---|---|---|
| Product topology and package layout | `README.md`; `bot-core/README.md` | Preserve outbound-only bot topology and direct-vs-bridge choice |
| Security boundary | `docs/explanation/architecture.md`; `docs/guide/access.md` | Separate transport identity/state from agent provider homes/workspaces |
| Wire/session invariants | `docs/explanation/protocol.md`; `bot-core/vendor/app-chat-codec.mjs` | Port only after byte-level fixtures and interop |
| Core composition | `bot-core/index.mjs` | Decompose into typed Rust ports |
| Network profiles and metadata | `bot-core/lib/network-config.mjs`; `bot-core/lib/descriptors.mjs` | Versioned profile data, not arbitrary per-turn input |
| State write discipline | `bot-core/lib/session-store.mjs` | Transactional SQLite |
| Tests/reference clients | `bot-core/test/`; `bot-core/test-client.mjs`; `bot-core/test-client-device.mjs` | Compatibility test corpus |

### Runtime, provider, and tool behavior

| Subject | PCA source | Successor treatment |
|---|---|---|
| Direct turn runtime | `bot-core/lib/agent-runtime.mjs` | Rust executor/runtime ports |
| CLI runners | `bot-core/lib/runners.mjs` | Runner adapter pattern for each CLI |
| Portable policy | `bot-core/lib/tool-policy.mjs` | Verified enforcement reports |
| In-chat commands | `bot-core/lib/commands.mjs`; `bot-core/lib/file-commands.mjs` | Core command service |
| Workspaces | `bot-core/lib/workspaces.mjs` | Auditable workspace grants |
| Per-conversation scheduling | `bot-core/lib/keyed-dispatcher.mjs` | Conversation actor model |

### Transport, artifacts, and framework integration

| Subject | PCA source | Successor treatment |
|---|---|---|
| Outbound reliability | `bot-core/lib/outbound-lanes.mjs` | Durable per-channel outbox lane |
| Live response UX | `bot-core/lib/live-reply.mjs`; `bot-core/lib/chunk.mjs` | ACK-gated edits, final-as-new-message |
| Attachment HOP | `bot-core/lib/hop-client.mjs`; `bot-core/lib/media-store.mjs` | Trusted endpoint policy |
| Durable chat files | `bot-core/lib/file-store.mjs` | Conversation-scoped artifact namespace |
| Bridge contract | `docs/reference/bridge.md` | C1 compatibility under `/v1/compat/pca` |
| Hermes adapter | `hermes-plugin/polkadot/adapter.py` | Migration adapter |
| OpenClaw adapter | `openclaw-plugin/polkadot/src/` | Migration adapter |
| T3ams composition | `bot-core/t3ams.mjs`; `bot-core/transports/t3ams/` | First-class transport crate |

### Bot lifecycle, configuration, and deployment

| Subject | PCA source | Successor treatment |
|---|---|---|
| CLI/configuration | `bot-core/cli.mjs`; `docs/reference/cli.md`; `docs/reference/configuration.md` | Typed config + secret references |
| Create/register identity | `bot-core/lib/register.mjs`; `bot-core/vendor/lib/wallet-keys.mjs` | Explicit identity lifecycle |
| Testnet storage allowance | `bot-core/lib/testnet-file-allowance.mjs` | Named-testnet optional adapter |
| Deploy | `docs/guide/deploy.md`; `bot-core/cli.mjs` | Immutable deployment manifests |
| T3ams onboarding | `docs/guide/t3ams.md` | Typed readiness states |

---

## Appendix C: Permanent integrity boundaries

The following are non-negotiable, regardless of configuration:

1. **Not a default:** Central custody of chat identities, wallet seeds, user
   provider credentials, or decrypted artifacts. Managed custody is a
   separate explicit consent.

2. **Not automatic:** Migration/cutover that starts Polkagent beside PCA for
   the same identity.

3. **Not part of initial PCA compatibility release:** Public unmetered agents,
   permissionless marketplace activation, user billing, autonomous spending,
   XCM/bridge transfers, generic browser automation, or smart-contract
   dependencies. They remain established platform tracks with their own
   gates.

4. **Never:** A cloud service whose admin/support access silently overrides
   local policy, reads private conversations, or signs/broadcasts transactions.

5. **Never:** Claiming a runtime sandbox or chain confirmation is stronger
   than the actual configured enforcement/finality evidence.

---

## Appendix D: Polkadot App/Product companion constraints

The public Product SDK is a TypeScript integration surface for Product/App
hosts, not a generic runtime for a standalone Rust bot daemon. A companion
can offer:

- Mobile approval rendering
- Account identity/proof visualization
- Attachment visualization
- Operator run cards

But only after its host capability is proven. It is **not** the bot daemon.
Its availability cannot be required for message delivery. A host signing
action must bind to an immutable local/remote `ApprovalRequest` digest.

**Evidence gate:** Pin the host and SDK version; prove that a companion can
render the exact canonical approval facts, bind a response to an unexpired
`ApprovalRequest` digest, report cancellation/refusal, and leave the data
plane fully functional when the host is unavailable. If the external wire
contract remains undocumented, retain a web/CLI/external-wallet approval
route and do not advertise native mobile approval as compatible.

---

## APPENDIX A: PCA MIGRATION IMPLEMENTATION BLUEPRINT

This appendix provides the engineering-level detail needed to implement each
PCA compatibility requirement. Sections mirror the compatibility tiers
(C0-C3) from section 7. All Rust code blocks are design sketches; they are
not guaranteed to compile against the exact crate structure in
`/Users/will/dev/par/polkagent`.

### A.1 ACK Protocol Implementation

PCA uses three distinct acknowledgement types (section 3.5). The Rust
implementation must treat them as separate state transitions in the delivery
ledger, never collapsing them.

#### Three ACK types with exact semantics

```rust
// polkagent-core/src/ack.rs

/// The three ACK types must never be conflated. Each represents a distinct
/// state transition in the delivery lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckKind {
    /// Bot -> chat peer: source message durably accepted; peer may stop
    /// resending. Sent after atomic write of (dedup_key, pending_turn) to
    /// SQLite. Must NOT mean "model answered" or "reply delivered."
    InboundTransport,

    /// Chat peer -> bot: peer fetched/acknowledged the current outbound
    /// Statement Store statement for a given request_id. Advances the
    /// outbound lane. Must NOT be confused with a bridge lease ACK.
    OutboundPeer { request_id: OutboxSubmissionId },

    /// Harness -> bot: bridge framework safely completed/accepted a leased
    /// delivery. Transitions lease from Leased to Completed. Must NOT mean
    /// source transport was ACKed or that the remote peer received the reply.
    BridgeLease { delivery_id: DeliveryId, lease_id: LeaseId },
}

/// Atomic precondition for InboundTransport ACK.
///
/// Both writes must succeed in a single SQLite transaction before the
/// transport ACK is sent. If either fails, the message is left unacknowledged
/// and the sender retries delivery.
pub struct InboundAcceptRecord {
    pub dedup_key: DedupKey,
    pub delivery: Delivery,
    pub pending_turn: Turn,
}

impl DeliveryLedger {
    /// Write dedup + delivery + pending_turn atomically, then return Ok to
    /// signal that the transport ACK may be sent.
    ///
    /// INVARIANT: source transport ACK is only sent after this returns Ok.
    pub async fn accept_inbound(
        &self,
        record: InboundAcceptRecord,
    ) -> Result<(), AcceptError> {
        self.store
            .transaction(|tx| {
                tx.insert_dedup(&record.dedup_key)?;
                tx.insert_delivery(&record.delivery)?;
                tx.insert_turn(&record.pending_turn)?;
                Ok(())
            })
            .await
    }
}
```

#### Lease renewal mechanism

A harness that holds an active bridge lease must renew it before expiry or
the delivery becomes re-leasable. The renewal window is the lease duration
minus a configurable safety margin (default: 5 seconds).

```rust
// polkagent-bridge-http/src/lease.rs

pub struct LeaseRenewRequest {
    pub delivery_id: DeliveryId,
    pub lease_id: LeaseId,
}

pub struct LeaseRenewResponse {
    /// New absolute expiry timestamp after renewal.
    pub new_expires_at: Timestamp,
    /// Remaining TTL in milliseconds.
    pub remaining_ms: u64,
}

impl BridgeLeaseService {
    /// Renew an active lease. Returns Err if the lease is expired, cancelled,
    /// or the delivery_id/lease_id pair does not match the current active
    /// lease. The renewed expiry is based on wall time at renewal, not the
    /// previous expiry, so slow workers accumulate renewals safely.
    pub async fn renew(
        &self,
        req: LeaseRenewRequest,
    ) -> Result<LeaseRenewResponse, LeaseError> {
        let mut state = self.store.load_lease(&req.delivery_id).await?;
        match &state {
            LeaseState::Leased { lease_id, expires_at, .. }
                if lease_id == &req.lease_id && *expires_at > Timestamp::now() =>
            {
                let new_expiry = Timestamp::now() + self.lease_duration;
                state = LeaseState::Leased {
                    lease_id: req.lease_id.clone(),
                    owner: state.owner().clone(),
                    expires_at: new_expiry,
                };
                self.store.update_lease(&req.delivery_id, &state).await?;
                Ok(LeaseRenewResponse {
                    new_expires_at: new_expiry,
                    remaining_ms: self.lease_duration.as_millis() as u64,
                })
            }
            LeaseState::Expired | LeaseState::Completed => {
                Err(LeaseError::AlreadyTerminal)
            }
            _ => Err(LeaseError::StaleOrMismatch),
        }
    }

    /// Background expiry sweeper. Runs every `sweep_interval` and transitions
    /// leases past their expiry time to LeaseState::Expired, making the
    /// delivery available for re-leasing.
    pub async fn run_expiry_sweeper(&self) {
        loop {
            tokio::time::sleep(self.sweep_interval).await;
            self.store.expire_stale_leases(Timestamp::now()).await
                .unwrap_or_else(|e| tracing::warn!("lease sweep error: {e}"));
        }
    }
}
```

#### Timeout and retry behavior

```rust
// polkagent-runtime/src/delivery_recovery.rs

/// On process restart, pending deliveries are recovered according to their
/// state. This table is the authoritative recovery policy.
///
/// | State at crash          | Recovery action                              |
/// |-------------------------|----------------------------------------------|
/// | Accepted, not ACKed     | Re-send transport ACK (source will retry)    |
/// | ACKed, turn pending     | Re-queue turn for executor dispatch          |
/// | Turn leased (direct)    | Mark attempt failed; retry if within budget  |
/// | Turn leased (bridge)    | Expire lease; make available for re-lease    |
/// | Turn running            | Mark attempt failed; retry or flag review    |
/// | Outbox pending          | Resume lane submission                       |
/// | Outbox uncertain        | Re-submit with same content/message_id       |
pub struct RecoveryPolicy {
    /// Maximum turn retry attempts before flagging for operator review.
    pub max_turn_retries: u32,
    /// Duration after which a running direct executor is presumed crashed.
    pub executor_timeout: Duration,
    /// Duration after which a bridge lease is presumed expired on restart.
    pub bridge_lease_grace: Duration,
}

impl DeliveryRecovery {
    /// Called at startup to recover all pending work from the delivery ledger.
    pub async fn recover_pending(&self, policy: &RecoveryPolicy) -> Result<RecoveryReport> {
        let pending = self.store.load_pending_deliveries().await?;
        let mut report = RecoveryReport::default();
        for delivery in pending {
            let action = self.classify_recovery_action(&delivery, policy);
            self.apply_recovery_action(delivery, action, &mut report).await?;
        }
        Ok(report)
    }
}
```

#### Ordering guarantees

The delivery ledger assigns a monotonically increasing `ordinal` to each
outbox item per conversation. The outbound lane submits items strictly in
ordinal order. No item at ordinal `N+1` is submitted until the item at
ordinal `N` has been peer-ACKed or superseded by a superset statement.

```rust
// polkagent-store/src/outbox.rs

/// Insert an outbox item with the next ordinal for its conversation.
/// The ordinal is assigned atomically within the transaction to prevent gaps.
pub async fn enqueue_outbox_item(
    &self,
    tx: &mut Transaction,
    item: NewOutboxItem,
) -> Result<OutboxItem> {
    let ordinal = tx.next_ordinal(&item.conversation_id).await?;
    let outbox_item = OutboxItem {
        id: OutboxItemId::new(),
        ordinal,
        submission_state: SubmissionState::Pending,
        ..item.into()
    };
    tx.insert_outbox_item(&outbox_item).await?;
    Ok(outbox_item)
}
```

### A.2 Device Channel Management

Polkadot App sends follow-up messages on device-specific channels derived
from the opener session. A bot watching only the identity channel appears to
work in test clients but fails silently with real phones.

#### Channel discovery protocol

```rust
// polkagent-transport/src/polkadot_app/channels.rs

/// All channels the transport must watch for a given conversation session.
///
/// The identity channel receives openers. Device channels receive follow-ups
/// from specific phone sessions. Both must be watched simultaneously.
#[derive(Debug, Clone)]
pub struct ChannelSubscriptionSet {
    /// The bot's identity channel. Always watched.
    pub identity_channel: TopicKey,
    /// Device-specific channels derived from session material.
    /// Added when a device session is established; removed on session expiry.
    pub device_channels: HashMap<DeviceSessionId, TopicKey>,
}

impl ChannelSubscriptionSet {
    /// Derive device channel topic key from session material.
    ///
    /// The derivation must match the JS codec bridge output exactly.
    /// Reference: /Users/will/dev/par/polkadot-chat-agents/bot-core/vendor/app-chat-codec.mjs
    pub fn derive_device_channel(
        session: &SessionMaterial,
        device_id: &DeviceId,
    ) -> TopicKey {
        // Topic key derivation is transport-specific and must produce
        // byte-identical output to the PCA JS codec for C0 compliance.
        // During the JS codec bridge phase, this is delegated to the bridge.
        todo!("delegate to JS codec bridge until native Rust codec is ready")
    }

    /// Register a new device channel after a device session opener is accepted.
    pub fn add_device_channel(&mut self, device_id: DeviceSessionId, topic: TopicKey) {
        self.device_channels.insert(device_id, topic);
    }

    /// Remove a device channel when its session expires or is reset.
    pub fn remove_device_channel(&mut self, device_id: &DeviceSessionId) {
        self.device_channels.remove(device_id);
    }

    /// All topic keys currently requiring polling and subscription.
    pub fn all_topics(&self) -> impl Iterator<Item = &TopicKey> {
        std::iter::once(&self.identity_channel)
            .chain(self.device_channels.values())
    }
}
```

#### Session lifecycle across devices

```rust
// polkagent-transport/src/polkadot_app/session.rs

/// Durable record of an active session with one peer.
///
/// Persisted to SQLite so device channels survive process restarts.
/// A session covers all device channels for one peer/conversation pair.
#[derive(Debug, Clone)]
pub struct PeerSession {
    pub conversation_id: ConversationId,
    pub peer_id: PeerId,
    /// Session encryption keys derived from opener.
    pub session_keys: SessionKeys,
    /// Device channels currently active for this session.
    pub device_channels: Vec<DeviceChannel>,
    pub established_at: Timestamp,
    pub last_activity: Timestamp,
}

#[derive(Debug, Clone)]
pub struct DeviceChannel {
    pub device_id: DeviceSessionId,
    pub topic_key: TopicKey,
    pub established_at: Timestamp,
}

impl SessionStore {
    /// Restore all active sessions on startup.
    /// Called before the subscription/polling loop starts so no messages
    /// on device channels are missed during the restart window.
    pub async fn restore_sessions(&self) -> Result<Vec<PeerSession>> {
        self.db.load_active_sessions().await
    }
}
```

#### Channel handoff between devices

When a user switches devices (e.g., phone to tablet), a new device opener
arrives on the identity channel establishing a new device session. The
transport must:

1. Accept the new device opener and derive a new device channel.
2. Add the new device channel to the subscription set without dropping the
   existing device channel (the old device may still send messages).
3. Persist the updated session state to SQLite before ACKing the opener.

```rust
// polkagent-transport/src/polkadot_app/ingress.rs

pub async fn handle_opener(
    &mut self,
    opener: InboundItem,
) -> Result<()> {
    // 1. Verify opener identity proof and derive session keys.
    let session = self.codec_bridge.process_opener(&opener).await?;

    // 2. Persist new device channel atomically with session update.
    // MUST complete before transport ACK is sent.
    self.session_store
        .transaction(|tx| {
            tx.upsert_session(&session)?;
            tx.add_device_channel(
                &session.conversation_id,
                &session.new_device_channel,
            )
        })
        .await?;

    // 3. Register new topic in the subscription set.
    self.subscriptions
        .add_device_channel(session.device_id, session.new_device_channel.topic_key);

    // 4. ACK opener only after durable write.
    self.transport.ack_inbound(&opener.delivery_id).await?;

    Ok(())
}
```

#### Conflict resolution for concurrent sessions

If two devices send openers concurrently, both sessions are accepted. The
transport does not arbitrate between devices; the conversation actor serializes
turns across all channels for the same peer. A single conversation ID is
used regardless of which device sent the message.

### A.3 Outbound Lane Implementation

The Statement Store keeps one current statement per account/channel. PCA's
outbound lane prevents message loss from slot replacement (section 3.7).

#### Message ordering guarantees

```rust
// polkagent-store/src/outbound_lane.rs

/// Per-conversation durable outbound lane state.
///
/// Invariant: items are submitted in strictly increasing ordinal order.
/// No item at ordinal N+1 is submitted until the item at ordinal N is
/// either peer-ACKed or safely superseded by a superset statement.
#[derive(Debug, Clone)]
pub struct LaneState {
    pub conversation_id: ConversationId,
    pub lane_key: LaneKey,
    /// Currently submitted statement, if any.
    pub current_submission: Option<ActiveSubmission>,
    /// Items waiting to be submitted once the current slot is cleared.
    pub pending_queue: VecDeque<OutboxItem>,
    /// Maximum pending queue depth before applying liveness takeover policy.
    pub max_queue_depth: usize,
}

#[derive(Debug, Clone)]
pub struct ActiveSubmission {
    /// The request_id of the currently active Statement Store statement.
    pub request_id: OutboxSubmissionId,
    /// Ordinal of the first item in this submission.
    pub base_ordinal: u64,
    /// All outbox item IDs included in this submission (superset may include
    /// multiple items).
    pub item_ids: Vec<OutboxItemId>,
    pub submitted_at: Timestamp,
}

impl OutboundLane {
    /// Attempt to submit the next pending item, or extend the current
    /// submission as a superset if the slot is occupied but not yet ACKed.
    pub async fn advance(&mut self, transport: &dyn Transport) -> Result<LaneEvent> {
        match &self.state.current_submission {
            None => {
                // Lane is clear: submit the next pending item.
                if let Some(item) = self.state.pending_queue.pop_front() {
                    let receipt = transport.submit_outbound(&item).await?;
                    self.state.current_submission = Some(ActiveSubmission {
                        request_id: receipt.request_id,
                        base_ordinal: item.ordinal,
                        item_ids: vec![item.id],
                        submitted_at: Timestamp::now(),
                    });
                    self.persist_state().await?;
                    Ok(LaneEvent::Submitted { ordinal: item.ordinal })
                } else {
                    Ok(LaneEvent::Idle)
                }
            }
            Some(active) => {
                // Lane is occupied: try superset extension if a new item fits.
                if let Some(next) = self.state.pending_queue.front() {
                    if self.fits_in_superset(active, next) {
                        let superset = self.build_superset(active, next);
                        let receipt = transport.submit_outbound(&superset).await?;
                        // After superset submission, ACKs for the old request_id
                        // must be ignored; only the new request_id advances the lane.
                        let new_submission = ActiveSubmission {
                            request_id: receipt.request_id,
                            base_ordinal: active.base_ordinal,
                            item_ids: {
                                let mut ids = active.item_ids.clone();
                                ids.push(self.state.pending_queue.pop_front().unwrap().id);
                                ids
                            },
                            submitted_at: Timestamp::now(),
                        };
                        self.state.current_submission = Some(new_submission);
                        self.persist_state().await?;
                        Ok(LaneEvent::SupersetExtended)
                    } else {
                        Ok(LaneEvent::Queued)
                    }
                } else {
                    Ok(LaneEvent::WaitingForPeerAck)
                }
            }
        }
    }

    /// Handle a peer ACK for a given request_id.
    ///
    /// ACKs for superseded request_ids are silently ignored; they do not
    /// advance the lane. Only the current active request_id advances the lane.
    pub async fn handle_peer_ack(&mut self, request_id: &OutboxSubmissionId) -> Result<LaneEvent> {
        match &self.state.current_submission {
            Some(active) if &active.request_id == request_id => {
                // Valid ACK: advance the lane.
                self.state.current_submission = None;
                self.persist_state().await?;
                Ok(LaneEvent::PeerAcked { request_id: request_id.clone() })
            }
            _ => {
                // Stale ACK for a superseded or unknown request_id. Ignore.
                Ok(LaneEvent::StaleAckIgnored)
            }
        }
    }
}
```

#### Lane persistence and recovery

The lane state is persisted to SQLite after every mutation. On restart,
the lane is restored from the database and the submission loop resumes
from the last known position. Uncertain submissions (where the network
error prevented confirmation) are re-submitted with the same content and
message ID; the peer client deduplicates by message ID.

```rust
// polkagent-store/src/outbound_lane.rs (continued)

impl OutboundLane {
    async fn persist_state(&self) -> Result<()> {
        self.store
            .transaction(|tx| {
                tx.upsert_lane_state(&self.state)
            })
            .await
    }

    /// Restore lane state from SQLite on process startup.
    pub async fn restore(
        store: &Store,
        conversation_id: &ConversationId,
    ) -> Result<Self> {
        let state = store.load_lane_state(conversation_id).await?
            .unwrap_or_else(|| LaneState::new(conversation_id.clone()));
        Ok(Self { state, store: store.clone() })
    }

    /// Reconcile uncertain outbox items on startup.
    /// For each item with SubmissionState::Uncertain, re-submit using the
    /// same payload (content_hash + message_id) so the peer can deduplicate.
    pub async fn reconcile_uncertain(&mut self, transport: &dyn Transport) -> Result<()> {
        let uncertain = self.store.load_uncertain_outbox_items(&self.state.lane_key).await?;
        for item in uncertain {
            match transport.submit_outbound(&item).await {
                Ok(receipt) => {
                    self.store.update_submission_state(
                        &item.id,
                        SubmissionState::Submitted { at: Timestamp::now() },
                    ).await?;
                    tracing::info!(
                        item_id = %item.id,
                        request_id = %receipt.request_id,
                        "reconciled uncertain outbox item"
                    );
                }
                Err(e) => {
                    tracing::warn!(item_id = %item.id, error = %e, "reconcile failed; will retry");
                }
            }
        }
        Ok(())
    }
}
```

#### Conflict resolution

Two outbox items for the same conversation slot cannot both be "current."
The lane enforces a strict serial order via ordinal assignment. If a
concurrent write race is detected (two writers attempting to claim the
same ordinal), the SQLite `UNIQUE` constraint on `(conversation_id, ordinal)`
rejects the second writer with a conflict error, which the caller must retry
with a new ordinal.

#### Backpressure handling

```rust
// polkagent-store/src/outbound_lane.rs (continued)

impl OutboundLane {
    /// Apply liveness takeover policy when the pending queue exceeds
    /// max_queue_depth for an unreachable peer.
    ///
    /// The takeover replaces the current unfetched slot with the queued work,
    /// sacrificing the old message's eventual visibility. This event is always
    /// recorded as an observable LaneEvent::LivenessTakeover for operator
    /// review. It is disabled by default; enable with [config] lane.liveness_takeover = true.
    pub async fn apply_liveness_takeover_if_needed(
        &mut self,
        transport: &dyn Transport,
    ) -> Result<()> {
        if self.state.pending_queue.len() < self.state.max_queue_depth {
            return Ok(());
        }
        if !self.config.liveness_takeover_enabled {
            tracing::warn!(
                conversation_id = %self.state.conversation_id,
                queue_depth = self.state.pending_queue.len(),
                "outbound lane queue full; liveness takeover disabled; blocking"
            );
            return Ok(());
        }
        // Record the takeover event before executing.
        let sacrificed_id = self.state.current_submission.as_ref().map(|s| s.request_id.clone());
        tracing::warn!(
            conversation_id = %self.state.conversation_id,
            sacrificed_request_id = ?sacrificed_id,
            "outbound lane liveness takeover; old slot will be replaced"
        );
        self.metrics.lane_liveness_takeovers.increment(1);
        self.state.current_submission = None;
        self.advance(transport).await?;
        Ok(())
    }
}
```

### A.4 State Import/Export

#### PCA state format (JSON/SQLite schema)

PCA's `session-state.json` is the primary migration source. At the pinned
commit (`2adddcc8`), the format is an object with the following top-level
keys (all optional; absent keys are treated as empty):

```json
{
  "sessions": {
    "<peer-account-id>": {
      "sessionKeys": { ... },
      "deviceChannels": [ { "deviceId": "...", "topicKey": "..." } ],
      "lastActivity": "2026-07-29T10:00:00Z"
    }
  },
  "dedupMarkers": {
    "<message-id>": { "acceptedAt": "2026-07-29T10:00:01Z" }
  },
  "owedReplies": [
    {
      "messageId": "...",
      "peerId": "...",
      "text": "...",
      "acceptedAt": "2026-07-29T10:00:01Z"
    }
  ],
  "outboundLane": {
    "<lane-key>": {
      "currentRequestId": "...",
      "pendingItems": [ { "messageId": "...", "content": "..." } ]
    }
  },
  "vaultFiles": {
    "<peer-account-id>": {
      "<filename>": { "path": "...", "size": 12345, "mime": "text/plain" }
    }
  }
}
```

The importer reads this format and transforms it into Polkagent's SQLite
schema. The SQLite schema is versioned; schema version 1 corresponds to the
above JSON format at the pinned PCA commit.

```sql
-- Schema version tracking (polkagent-store)
CREATE TABLE schema_versions (
    component TEXT NOT NULL,
    version   INTEGER NOT NULL,
    applied_at TEXT NOT NULL,
    PRIMARY KEY (component)
);

-- Delivery ledger (replaces session-state.json dedupMarkers + owedReplies)
CREATE TABLE deliveries (
    id                  TEXT PRIMARY KEY,
    transport           TEXT NOT NULL,
    remote_request_id   TEXT NOT NULL,
    remote_message_id   TEXT NOT NULL,
    conversation_id     TEXT NOT NULL,
    sender_id           TEXT NOT NULL,
    dedup_key           TEXT NOT NULL UNIQUE,
    source_ack_state    TEXT NOT NULL DEFAULT 'pending',
    accepted_at         TEXT NOT NULL,
    lease_state         TEXT NOT NULL DEFAULT 'unleased'
);

-- Turns (replaces owedReplies)
CREATE TABLE turns (
    id                          TEXT PRIMARY KEY,
    delivery_id                 TEXT NOT NULL REFERENCES deliveries(id),
    executor_profile_revision   INTEGER NOT NULL DEFAULT 0,
    resolved_grant_digest       TEXT,
    state                       TEXT NOT NULL DEFAULT 'pending',
    active_attempt_id           TEXT,
    executor_session_ref        TEXT,
    terminal_outcome            TEXT,
    created_at                  TEXT NOT NULL
);

-- Outbox items (replaces outboundLane)
CREATE TABLE outbox_items (
    id                   TEXT PRIMARY KEY,
    conversation_id      TEXT NOT NULL,
    ordinal              INTEGER NOT NULL,
    lane_key             TEXT NOT NULL,
    message_id           TEXT NOT NULL,
    kind                 TEXT NOT NULL DEFAULT 'text',
    payload_artifact     TEXT,
    reply_or_edit_target TEXT,
    supersedes           TEXT NOT NULL DEFAULT '[]',
    submission_state     TEXT NOT NULL DEFAULT 'pending',
    UNIQUE (conversation_id, ordinal)
);

-- Sessions (replaces sessions in session-state.json)
CREATE TABLE peer_sessions (
    conversation_id TEXT PRIMARY KEY,
    peer_id         TEXT NOT NULL,
    session_keys    BLOB NOT NULL,   -- encrypted at rest
    established_at  TEXT NOT NULL,
    last_activity   TEXT NOT NULL
);

CREATE TABLE device_channels (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES peer_sessions(conversation_id),
    device_id       TEXT NOT NULL,
    topic_key       TEXT NOT NULL,
    established_at  TEXT NOT NULL,
    UNIQUE (conversation_id, device_id)
);
```

#### Import validation and transformation

```rust
// polkagent-cli/src/import_pca.rs

/// The import pipeline runs in four phases, each of which must succeed
/// before the next begins.
pub async fn import_pca(
    pca_dir: &Path,
    opts: &ImportOptions,
) -> Result<ImportReport> {
    // Phase 1: Validate preconditions.
    let preconditions = validate_preconditions(pca_dir).await?;

    // Phase 2: Parse and validate PCA state (read-only).
    let pca_state = parse_pca_state(pca_dir, &preconditions).await?;

    // If --preview, stop here and render the report.
    if opts.preview {
        return Ok(ImportReport::preview(pca_state));
    }

    // Phase 3: Create rollback export of any existing Polkagent state.
    let rollback_export = create_rollback_export(&opts.agent_dir).await?;

    // Phase 4: Transform and write to SQLite (idempotent transaction).
    let written = transform_and_write(pca_state, &opts).await?;

    Ok(ImportReport::complete(written, rollback_export))
}

async fn validate_preconditions(pca_dir: &Path) -> Result<PreconditionReport> {
    // IMP-05: Refuse if PCA appears to be running.
    if is_pca_running(pca_dir).await? {
        return Err(ImportError::PcaStillRunning);
    }
    // IMP-06: PCA directory must be readable; we never write to it.
    check_pca_dir_readable(pca_dir)?;
    // Validate that session-state.json exists and is parseable.
    let state_path = pca_dir.join("session-state.json");
    let _: PcaStateJson = read_and_parse_json(&state_path)?;
    Ok(PreconditionReport::ok())
}

/// Transform PCA state into Polkagent SQLite rows.
///
/// IMP-01: session-state.json is an input format; transform transactionally.
/// IMP-02: owedReplies import as Turn { state: UnknownAfterMigration }.
async fn transform_and_write(
    pca_state: PcaState,
    opts: &ImportOptions,
) -> Result<ImportWriteReport> {
    let mut report = ImportWriteReport::default();

    opts.db.transaction(|tx| {
        // Dedup markers -> deliveries table
        for (msg_id, marker) in &pca_state.dedup_markers {
            tx.insert_delivery(&Delivery::from_dedup_marker(msg_id, marker))?;
            report.deliveries_imported += 1;
        }

        // Owed replies -> turns table with UnknownAfterMigration state
        // IMP-02: Never auto-retry; operator reviews each item.
        for reply in &pca_state.owed_replies {
            tx.insert_turn(&Turn {
                state: TurnState::UnknownAfterMigration,
                ..Turn::from_owed_reply(reply)
            })?;
            report.pending_turns_imported += 1;
        }

        // Sessions -> peer_sessions + device_channels tables
        for (peer_id, session) in &pca_state.sessions {
            tx.insert_peer_session(&PeerSession::from_pca(peer_id, session))?;
            for device_ch in &session.device_channels {
                tx.insert_device_channel(&DeviceChannel::from_pca(
                    peer_id, device_ch,
                ))?;
            }
            report.sessions_imported += 1;
        }

        // Outbound lane -> outbox_items table
        for (lane_key, lane) in &pca_state.outbound_lane {
            if let Some(current_id) = &lane.current_request_id {
                // Mark as uncertain: we don't know if the peer ACKed it.
                tx.insert_outbox_item(&OutboxItem::from_pca_uncertain(
                    lane_key, current_id,
                ))?;
                report.uncertain_outbox_items += 1;
            }
            for item in &lane.pending_items {
                tx.insert_outbox_item(&OutboxItem::from_pca_pending(lane_key, item))?;
                report.outbox_items_imported += 1;
            }
        }

        Ok(())
    }).await?;

    Ok(report)
}
```

#### Configuration translation (PCA config -> Polkagent config)

```rust
// polkagent-cli/src/import_pca/config.rs

/// Map PCA environment variable configuration to typed config.toml.
///
/// IMP-04: Unknown env variables are reported, never silently dropped.
pub fn translate_config(
    pca_env: &HashMap<String, String>,
) -> Result<(PolkagentConfig, ConfigTranslationReport)> {
    let mut config = PolkagentConfig::default();
    let mut report = ConfigTranslationReport::default();

    // Known PCA env vars and their Polkagent equivalents.
    let mappings: &[(&str, fn(&str, &mut PolkagentConfig) -> Result<()>)] = &[
        ("NETWORK", |v, c| { c.agent.network_profile = v.to_string(); Ok(()) }),
        ("BRAIN",   |v, c| { c.executor.kind = ExecutorKind::from_pca_brain(v)?; Ok(()) }),
        ("MODEL",   |v, c| { c.model.default = v.to_string(); Ok(()) }),
        ("TOOLS",   |v, c| { c.executor.capabilities = Capabilities::from_pca_tools(v)?; Ok(()) }),
        ("ALLOWLIST", |v, c| { c.access.allowlist = parse_allowlist(v)?; Ok(()) }),
        // ... additional mappings for all documented PCA env vars
    ];

    for (key, value) in pca_env {
        if let Some((_, mapper)) = mappings.iter().find(|(k, _)| k == key) {
            if let Err(e) = mapper(value, &mut config) {
                report.translation_errors.push(format!("{key}: {e}"));
            } else {
                report.translated.push(key.clone());
            }
        } else {
            // IMP-04: Unknown var is reported but does not silently apply.
            report.unknown_vars.push(key.clone());
        }
    }

    Ok((config, report))
}
```

#### Identity migration (key preservation)

```rust
// polkagent-cli/src/import_pca/identity.rs

/// Import PCA bot identity. The seed is never written to config.toml.
/// It is stored in the configured secret backend.
///
/// PCA stores: $PCA_DIR/keys/seed (raw seed phrase or hex)
/// Polkagent stores: secret backend reference (keychain|file|vault)
pub async fn import_identity(
    pca_dir: &Path,
    secret_backend: &SecretBackend,
) -> Result<ImportedIdentity> {
    let seed_path = pca_dir.join("keys").join("seed");
    // IMP-06: Read from PCA dir; never write back.
    let seed_bytes = std::fs::read(&seed_path)
        .map_err(|e| ImportError::SeedReadFailed(e.to_string()))?;

    // Derive the same Polkadot account as PCA.
    let keypair = derive_keypair_from_seed(&seed_bytes)?;
    let account_id = keypair.public_key().to_account_id();

    // Store seed in the configured backend; never in config.toml.
    let secret_ref = secret_backend.store_seed(&seed_bytes).await?;

    // Burn the in-memory seed bytes before returning.
    // (Actual zeroization requires the `zeroize` crate on sensitive types.)
    drop(seed_bytes);

    Ok(ImportedIdentity {
        account_id,
        public_keys: keypair.public_keys(),
        secret_ref,
    })
}
```

---

## APPENDIX B: COMPATIBILITY TESTING FRAMEWORK

### B.1 Compatibility Assertion Framework

The compatibility testing framework lives at
`/Users/will/dev/par/polkagent/tests/compat/`. Each C-tier has its own
subdirectory with fixture-driven tests. Tests are ordinary Rust integration
tests (using `cargo test` or `cargo nextest`) with no external dependencies
beyond a local SQLite instance.

#### Automated compatibility tests for each C0-C3 tier

```rust
// tests/compat/c0_transport/dedup_restart.rs

/// C0-T03: Simulated persistence failure prevents source ACK.
/// After recovery and resend, exactly one delivery+turn exists.
#[tokio::test]
async fn test_c0_dedup_on_restart() {
    let db = TestDb::new().await;
    let ledger = DeliveryLedger::new(db.clone());

    let item = InboundItem::fixture_text("hello");

    // First delivery attempt: persistence fails.
    db.set_fail_on_next_write(true);
    let result = ledger.accept_inbound(InboundAcceptRecord::from(&item)).await;
    assert!(result.is_err(), "persistence failure must prevent acceptance");

    // ACK must not have been sent (transport mock tracks ACKs).
    let transport = MockTransport::default();
    assert_eq!(transport.acks_sent(), 0);

    // Source retries: same message arrives again.
    db.set_fail_on_next_write(false);
    ledger.accept_inbound(InboundAcceptRecord::from(&item)).await.unwrap();

    // Exactly one delivery and one turn must exist.
    assert_eq!(db.count_deliveries().await, 1);
    assert_eq!(db.count_turns().await, 1);

    // Duplicate resend: dedup must reject it.
    let dup_result = ledger.accept_inbound(InboundAcceptRecord::from(&item)).await;
    assert!(matches!(dup_result, Err(AcceptError::Duplicate)));
    assert_eq!(db.count_deliveries().await, 1);
}

/// C0-T06: Two sequential replies arrive in order; first is not overwritten.
#[tokio::test]
async fn test_c0_outbound_lane_ordering() {
    let transport = MockTransport::default();
    let mut lane = OutboundLane::new_empty("conv-1".into(), transport.clone());

    let item_a = OutboxItem::fixture_text("first reply", 1);
    let item_b = OutboxItem::fixture_text("second reply", 2);

    lane.enqueue(item_a.clone()).await.unwrap();
    lane.advance(&transport).await.unwrap();

    // item_a is now current; item_b must be queued, not submitted.
    lane.enqueue(item_b.clone()).await.unwrap();
    lane.advance(&transport).await.unwrap();

    assert_eq!(transport.submitted_count(), 1, "only item_a should be submitted");
    assert_eq!(transport.current_content(), "first reply");

    // Peer ACKs item_a; lane advances to item_b.
    let req_id = transport.last_request_id();
    lane.handle_peer_ack(&req_id).await.unwrap();
    lane.advance(&transport).await.unwrap();

    assert_eq!(transport.submitted_count(), 2);
    assert_eq!(transport.current_content(), "second reply");
}
```

#### PCA message format round-trip tests

```rust
// tests/compat/c0_transport/message_format.rs

/// Round-trip test: encode a message through the JS codec bridge, then
/// decode it. The decoded content must match the original.
///
/// This test requires a running JS codec bridge (set CODEC_BRIDGE_SOCKET).
#[tokio::test]
#[cfg_attr(not(feature = "codec-bridge"), ignore)]
async fn test_c0_message_roundtrip() {
    let bridge = CodecBridge::from_env().await.expect("CODEC_BRIDGE_SOCKET must be set");
    let session = bridge.create_test_session().await.unwrap();

    let original = "Hello from Polkagent compatibility test";
    let encrypted = bridge.encrypt(&session, original.as_bytes()).await.unwrap();
    let decrypted = bridge.decrypt(&session, &encrypted).await.unwrap();

    assert_eq!(decrypted, original.as_bytes());
}

/// Byte-level vector test: verify the codec bridge produces the same
/// ciphertext as PCA at the pinned commit for a known input.
///
/// Fixture file: tests/compat/c0_transport/fixtures/opener-vectors.json
/// Source commit: 2adddcc8cfd732804cd9bbcbcd26974b44b47f66
#[tokio::test]
#[cfg_attr(not(feature = "codec-bridge"), ignore)]
async fn test_c0_opener_byte_vectors() {
    let vectors: Vec<OpenerVector> = load_fixture("opener-vectors.json");
    let bridge = CodecBridge::from_env().await.unwrap();

    for vector in vectors {
        let result = bridge
            .process_opener_with_known_seed(&vector.seed, &vector.opener_bytes)
            .await
            .unwrap();
        assert_eq!(
            result.session_key_bytes, vector.expected_session_key,
            "session key mismatch for vector {}",
            vector.id
        );
    }
}
```

#### Session lifecycle compatibility tests

```rust
// tests/compat/c0_transport/device_channels.rs

/// C0-T02: Bot receives follow-up on device channel, not just identity channel.
#[tokio::test]
#[cfg_attr(not(feature = "codec-bridge"), ignore)]
async fn test_c0_device_channel_followup() {
    let mut harness = TransportTestHarness::new().await;

    // Simulate a phone opener on the identity channel.
    let opener = harness.send_opener("test-peer").await.unwrap();
    harness.wait_for_ack(&opener).await.unwrap();

    // Simulate a follow-up from the same peer on a device channel.
    let device_channel = harness.derive_device_channel(&opener.session).await;
    let followup = harness.send_on_channel(&device_channel, "follow-up text").await.unwrap();

    // Bot must receive and accept the device-channel message.
    let received = harness.received_messages().await;
    assert!(received.iter().any(|m| m.text == "follow-up text"),
        "bot must receive follow-up on device channel");
}
```

#### Regression test suite

```rust
// tests/compat/regression/mod.rs

/// Regression test: ensure that adding a new transport does not break
/// existing C0 delivery semantics.
#[test]
fn transport_crate_must_not_import_executor() {
    // This test reads Cargo.toml dependency graphs to enforce boundary rules
    // from section 19.3. It fails if polkagent-transport gains a dependency
    // on polkagent-executor or any of its sub-crates.
    let manifest = workspace_manifest();
    let transport_deps = manifest.crate_deps("polkagent-transport");
    assert!(
        !transport_deps.iter().any(|d| d.starts_with("polkagent-executor")),
        "polkagent-transport must not depend on polkagent-executor (boundary rule 1)"
    );
}
```

### B.2 Migration Testing Corpus

#### Representative PCA state snapshots for testing

The migration test corpus lives at
`tests/compat/c2_migration/fixtures/`. Each fixture is a complete PCA
bot directory snapshot (minus real secrets) that exercises a specific
import scenario.

| Fixture | Description | Key assertions |
|---|---|---|
| `minimal/` | Bare PCA install; no sessions, no owed replies | Import succeeds with empty report |
| `typical/` | One session, 3 dedup markers, 1 owed reply, 2 vault files | Full round-trip; owed reply imports as UnknownAfterMigration |
| `large_history/` | 50 sessions, 5000 dedup markers | Import completes within 30 seconds; all rows inserted |
| `encrypted_session/` | Session keys present; seed required for decryption | Import reports key material; prompts for secret backend |
| `multi_device/` | One session with 3 device channels | All device channels imported; subscription set correct |
| `pending_outbound/` | 3 outbound items, 1 uncertain, 1 pending, 1 peer-acked | Uncertain item flagged for reconciliation |
| `unknown_env_vars/` | Config with unrecognized environment variables | Import fails validation; lists unknown vars |
| `live_pca/` | PCA lock file present (simulated running process) | Import refuses with PcaStillRunning error |

```rust
// tests/compat/c2_migration/import_typical.rs

#[tokio::test]
async fn test_c2_import_typical_snapshot() {
    let fixture = load_fixture_dir("typical");
    let db = TestDb::new().await;
    let opts = ImportOptions {
        pca_dir: fixture.path(),
        db: db.clone(),
        preview: false,
        secret_backend: SecretBackend::InMemory,
        ..Default::default()
    };

    let report = import_pca(&fixture.path(), &opts).await.unwrap();

    // IMP-02: owed replies import as UnknownAfterMigration, never auto-retried.
    assert_eq!(report.pending_turns_imported, 1);
    let turn = db.load_turns().await.first().unwrap().clone();
    assert_eq!(turn.state, TurnState::UnknownAfterMigration);

    // IMP-06: fixture PCA dir must not have been modified.
    assert_eq!(fixture.checksum_before(), fixture.checksum_after(),
        "PCA fixture directory must not be modified during import");

    // Dedup markers imported correctly.
    assert_eq!(db.count_deliveries().await, 3);
    // Sessions and device channels imported.
    assert_eq!(db.count_peer_sessions().await, 1);
    assert_eq!(db.count_device_channels().await, 2);
}
```

#### Edge cases

```rust
// tests/compat/c2_migration/import_large_history.rs

#[tokio::test]
async fn test_c2_import_large_history_within_time_limit() {
    let fixture = generate_large_fixture(50, 5000); // 50 sessions, 5000 markers
    let start = Instant::now();
    import_pca(&fixture.path(), &default_opts()).await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(30),
        "large import must complete within 30 seconds");
}

#[tokio::test]
async fn test_c2_import_multi_device_preserves_all_channels() {
    let fixture = load_fixture_dir("multi_device");
    let report = import_pca(&fixture.path(), &default_opts()).await.unwrap();
    let db = report.db;

    let channels = db.count_device_channels().await;
    assert_eq!(channels, 3, "all three device channels must be imported");
}
```

#### Performance benchmarks

```rust
// benches/migration.rs (criterion benchmark)
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_import_large(c: &mut Criterion) {
    let fixture = generate_large_fixture(50, 5000);
    c.bench_function("import_large_pca_state", |b| {
        b.to_async(tokio::runtime::Runtime::new().unwrap()).iter(|| async {
            let db = TestDb::new().await;
            import_pca(&fixture.path(), &opts_with_db(db)).await.unwrap()
        });
    });
}

fn bench_outbound_lane_advance(c: &mut Criterion) {
    c.bench_function("outbound_lane_advance_1000_items", |b| {
        b.to_async(tokio::runtime::Runtime::new().unwrap()).iter(|| async {
            let transport = MockTransport::default();
            let mut lane = OutboundLane::new_empty("conv-bench".into(), transport.clone());
            for i in 0..1000u64 {
                lane.enqueue(OutboxItem::fixture_text(&format!("msg-{i}"), i)).await.unwrap();
                lane.handle_peer_ack(&transport.last_request_id()).await.unwrap();
                lane.advance(&transport).await.unwrap();
            }
        });
    });
}

criterion_group!(benches, bench_import_large, bench_outbound_lane_advance);
criterion_main!(benches);
```

---

## APPENDIX C: ROLLING MIGRATION STRATEGY

This appendix provides the implementation detail behind the high-level rolling
migration strategy in section 15. It describes each phase with specific
acceptance gates, Rust code hooks, and observable state transitions.

### Phase 1: Read-only PCA import

**Objective:** Import PCA state into Polkagent with zero modification to PCA
files. The operator retains full rollback capability by running PCA from the
unchanged directory.

**Implementation hooks:**

```rust
// polkagent-cli/src/commands/import_pca.rs

/// Phase 1 entry point: import without starting any Polkagent runtime.
/// PCA is stopped before this runs. Polkagent does not serve any transport.
pub async fn cmd_import_pca(args: ImportPcaArgs) -> Result<()> {
    println!("Phase 1: Read-only PCA import");
    println!("PCA directory: {}", args.pca_dir.display());

    // Guard: refuse if any Polkagent runtime is running for this agent.
    check_no_polkagent_runtime(&args.agent_id).await?;

    let report = import_pca(&args.pca_dir, &args.into_opts()).await?;

    // Create rollback export as a compressed SQLite snapshot.
    let rollback_path = args.data_dir.join("rollback-before-import.sqlite.gz");
    create_rollback_export(&args.data_dir, &rollback_path).await?;
    println!("Rollback export created: {}", rollback_path.display());

    print_import_report(&report);

    // Phase 1 sentinel: records that import completed but Polkagent has
    // not yet started serving this identity.
    write_migration_sentinel(&args.data_dir, MigrationPhase::ImportComplete).await?;

    println!("\nPhase 1 complete. Run `polkagent migration verify` before starting.");
    Ok(())
}
```

**Observable state:**
- `migration_sentinel` in data directory contains `phase: import_complete`.
- SQLite database populated but no runtime started.
- PCA directory unchanged; operator can restart PCA at any time.

**Acceptance gate:**
- `polkagent migration verify` passes all precondition checks.
- Import report shows no unknown fields (or operator has reviewed and accepted them).
- Rollback export is present and readable.

### Phase 2: Bidirectional sync

**Objective:** Polkagent runs in shadow mode alongside the stopped PCA,
accepting messages and writing to its own state, but not serving the
transport identity (PCA is stopped). Used for testing the runtime
without user-visible traffic.

In practice, true bidirectional sync between a live PCA and a live Polkagent
is prohibited (section 15.3). "Bidirectional" in this context means the
operator can switch back to PCA from the Phase 1 export without data loss
from Polkagent's side, because Polkagent has not yet served any real traffic.

**Implementation hooks:**

```rust
// polkagent-cli/src/commands/migration.rs

/// Phase 2: Trial run with echo brain. No real executor; no real user traffic.
pub async fn cmd_migration_trial(args: MigrationTrialArgs) -> Result<()> {
    ensure_migration_phase(&args.data_dir, MigrationPhase::ImportComplete).await?;

    println!("Phase 2: Starting Polkagent in trial mode (echo brain)");
    println!("Transport is ACTIVE. Send a test message from your phone.");
    println!("To stop: Ctrl-C. This does NOT constitute cutover.");

    let mut runtime = PolkagentRuntime::new_with_echo_brain(&args.config).await?;
    runtime.run_until_signal().await?;

    println!("\nTrial run complete. Run `polkagent migration cutover` to commit.");
    Ok(())
}
```

### Phase 3: Full migration with rollback capability

**Objective:** Operator has verified the trial run and commits to Polkagent as
the active runtime. Bridge tokens are regenerated. The migration sentinel
advances to `CutoverComplete`. Rollback is still technically possible from
the Phase 1 export, but requires manual reconciliation for any messages
received during the trial period.

```rust
// polkagent-cli/src/commands/migration.rs

pub async fn cmd_migration_cutover(args: MigrationCutoverArgs) -> Result<()> {
    ensure_migration_phase(&args.data_dir, MigrationPhase::ImportComplete).await?;

    println!("=== MIGRATION CUTOVER ===");
    println!("This will regenerate bridge tokens and commit Polkagent as active.");
    confirm_interactive("Proceed with cutover? [yes/no]: ").await?;

    // IMP-03: Regenerate bridge tokens by default.
    if !args.retain_bridge_token {
        let new_token = regenerate_bridge_token(&args.data_dir).await?;
        println!("Bridge token regenerated. Notify harness integrators.");
        println!("New token stored in: {}", args.data_dir.join("bridge-token").display());
    } else {
        // Short compatibility window must be explicitly requested and logged.
        audit_log(&args.data_dir, AuditEvent::BridgeTokenRetained {
            reason: args.retain_reason.clone().unwrap_or_default(),
        }).await?;
        println!("WARNING: Old bridge token retained. This is a short compatibility window only.");
    }

    // Write cutover sentinel; PCA restart will be blocked by this file.
    write_migration_sentinel(&args.data_dir, MigrationPhase::CutoverComplete).await?;

    // Create PCA sentinel file to prevent accidental PCA restart.
    let pca_sentinel = args.pca_dir.join(".polkagent-cutover");
    std::fs::write(&pca_sentinel, "Polkagent cutover completed. Do not restart PCA.\n")?;

    println!("\nCutover complete. Polkagent is now the active runtime.");
    println!("Review pending work: `polkagent migration review-pending`");
    Ok(())
}

pub async fn cmd_migration_rollback(args: MigrationRollbackArgs) -> Result<()> {
    let rollback_path = args.data_dir.join("rollback-before-import.sqlite.gz");
    if !rollback_path.exists() {
        return Err(MigrationError::NoRollbackExport.into());
    }

    println!("Restoring from rollback export: {}", rollback_path.display());
    restore_from_rollback_export(&rollback_path, &args.data_dir).await?;
    remove_migration_sentinel(&args.data_dir).await?;

    println!("Rollback complete. PCA directory was not modified; restart PCA to resume.");
    Ok(())
}
```

### Phase 4: PCA deprecation

**Objective:** After a stable Polkagent operation period (operator-defined;
suggested minimum 30 days), the operator formally deprecates PCA. This
consists of:

1. Archiving the PCA directory (not deleting; operator keeps a local copy).
2. Removing the cutover sentinel from the PCA directory.
3. Updating documentation and harness integrators to use native Polkagent
   endpoints.
4. Running a final export of Polkagent state as a portable archive.

There is no automated `deprecate` command; this is a manual operator decision.
Polkagent provides the export tooling:

```bash
polkagent export <agent-id> \
  --output /backups/polkagent-$(date +%Y%m%d).tar.gz \
  --include-artifacts \
  --encrypt
```

---

## APPENDIX D: IMPLEMENTATION CHECKLIST

Ordered tasks with acceptance criteria, grouped by subsystem. Each task
maps to one or more functional requirements from section 17 and one or more
test IDs from section 13.

### ACK Protocol

- [ ] **ACK-01:** Implement `DeliveryLedger::accept_inbound` with atomic
  SQLite transaction (dedup_key + delivery + pending_turn). Acceptance
  criteria: C0-T03 passes; any write failure leaves the message unacknowledged.
  Maps to: FR-T04, C0-R04.

- [ ] **ACK-02:** Implement `OutboundLane::handle_peer_ack` with stale
  request_id filtering. Acceptance criteria: C0-T08 passes; ACKs for
  superseded request IDs do not advance the lane.
  Maps to: FR-T07, C0-R07.

- [ ] **ACK-03:** Implement `BridgeLeaseService::ack` with exact
  delivery_id/lease_id matching. Acceptance criteria: C1-T02 passes;
  stale or mismatched leases return conflict.
  Maps to: FR-B02, FR-B03.

- [ ] **ACK-04:** Implement `BridgeLeaseService::renew` with wall-time
  renewal. Acceptance criteria: C1-T01 passes; renewed expiry is based
  on current time, not previous expiry.
  Maps to: FR-B02.

- [ ] **ACK-05:** Implement background lease expiry sweeper.
  Acceptance criteria: C1-T02 passes; expired leases become re-leasable
  within one sweep interval.
  Maps to: FR-B02.

### Device Channels

- [ ] **CHAN-01:** Implement `ChannelSubscriptionSet` with identity +
  device channels. Acceptance criteria: C0-T02 passes; follow-up on
  device channel is received and accepted.
  Maps to: FR-T03, C0-R03.

- [ ] **CHAN-02:** Implement `SessionStore::restore_sessions` called before
  subscription loop starts. Acceptance criteria: after restart, device
  channels from previous sessions are re-subscribed before any polling.
  Maps to: FR-T03, C0-R08.

- [ ] **CHAN-03:** Implement device channel handoff for multi-device openers.
  Acceptance criteria: two concurrent device openers produce two device
  channels for the same conversation; neither is dropped.
  Maps to: FR-T03.

- [ ] **CHAN-04:** Persist updated `ChannelSubscriptionSet` to SQLite before
  ACKing any opener. Acceptance criteria: restart after opener preserves
  device channel; no re-subscription required.
  Maps to: C0-R08.

### Outbound Lane

- [ ] **LANE-01:** Implement `OutboundLane::advance` with single-slot
  submission. Acceptance criteria: C0-T06 passes; two sequential replies
  are ordered, not overwritten.
  Maps to: FR-T06, C0-R07.

- [ ] **LANE-02:** Implement superset extension logic.
  Acceptance criteria: C0-T07 passes; second reply merges with first
  under a new request_id.
  Maps to: FR-T06, C0-R07.

- [ ] **LANE-03:** Implement `OutboundLane::persist_state` after every
  mutation. Acceptance criteria: lane state survives process restart;
  lane resumes from last known position.
  Maps to: FR-T06, C0-R08.

- [ ] **LANE-04:** Implement `OutboundLane::reconcile_uncertain` on startup.
  Acceptance criteria: uncertain items are re-submitted; peer deduplicates
  by message_id.
  Maps to: C0-T09 (partial).

- [ ] **LANE-05:** Implement liveness takeover with observable event.
  Acceptance criteria: C0-T09 passes; takeover event is logged with
  sacrificed request_id; disabled by default.
  Maps to: FR-T08, C0-R07.

- [ ] **LANE-06:** Implement bounded pending queue with backpressure.
  Acceptance criteria: queue depth never exceeds `max_queue_depth`;
  enqueue returns `Err(LaneError::QueueFull)` when full.
  Maps to: FR-T08.

### State Import

- [ ] **IMP-01:** Implement `validate_preconditions` with PCA live-process
  detection. Acceptance criteria: C2-T03 passes; import refuses if PCA
  lock file or PID is detected.
  Maps to: FR-M02, IMP-05.

- [ ] **IMP-02:** Implement `transform_and_write` with full field mapping.
  Acceptance criteria: C2-T01 passes; complete mapping report generated.
  Maps to: FR-M01, FR-M03.

- [ ] **IMP-03:** Implement owed reply import as `UnknownAfterMigration`.
  Acceptance criteria: C2-T05 passes; every owed reply appears in
  operator review queue; none are auto-retried.
  Maps to: FR-M04, IMP-02.

- [ ] **IMP-04:** Implement rollback export before any write.
  Acceptance criteria: C2-T04 passes; rollback export is created before
  SQLite writes; `migration rollback` restores it.
  Maps to: FR-M05, IMP-05.

- [ ] **IMP-05:** Implement immutable PCA directory assertion.
  Acceptance criteria: C2-T02 passes; SHA-256 checksum of PCA directory
  matches before and after import.
  Maps to: FR-M06, IMP-06.

- [ ] **IMP-06:** Implement `translate_config` with unknown-var detection.
  Acceptance criteria: C2-T08 passes; unknown env vars cause import to
  fail validation with a list of unknown keys.
  Maps to: FR-M08, IMP-04.

- [ ] **IMP-07:** Implement `import_identity` with seed extraction to
  secret backend. Acceptance criteria: seed is not written to config.toml;
  account ID is derivable from stored secret reference.
  Maps to: FR-M01.

- [ ] **IMP-08:** Implement vault file import with manifest and content hashes.
  Acceptance criteria: all vault files imported; content hash matches;
  audit trail entry created for each file.
  Maps to: FR-M01.

### Compatibility Tests

- [ ] **TEST-01:** Implement byte-level opener encryption fixture suite at
  pinned PCA commit (`2adddcc8`). Acceptance criteria: C0-T01 passes
  against JS codec bridge.
  Maps to: C0-T01.

- [ ] **TEST-02:** Implement device-channel follow-up integration test.
  Acceptance criteria: C0-T02 passes.
  Maps to: C0-T02.

- [ ] **TEST-03:** Implement C1 lease lifecycle fixture.
  Acceptance criteria: C1-T01 through C1-T03 pass.
  Maps to: FR-B01, FR-B02, FR-B03.

- [ ] **TEST-04:** Implement C2 migration fixture for `typical/` snapshot.
  Acceptance criteria: C2-T01 through C2-T06 pass against fixture.
  Maps to: FR-M01 through FR-M08.

- [ ] **TEST-05:** Implement Hermes adapter compatibility fixture.
  Acceptance criteria: C1-T09 passes; full poll/lease/renew/ACK/send cycle.
  Maps to: FR-B07.

- [ ] **TEST-06:** Implement OpenClaw adapter compatibility fixture.
  Acceptance criteria: C1-T10 passes; proactive send path verified.
  Maps to: FR-B08.

### Migration

- [ ] **MIG-01:** Implement `cmd_import_pca` with preview mode.
  Acceptance criteria: `--preview` shows full mapping report without
  writing any Polkagent state.
  Maps to: FR-M01, FR-M03.

- [ ] **MIG-02:** Implement migration sentinel file lifecycle.
  Acceptance criteria: sentinel advances through
  `ImportComplete -> CutoverComplete`; rollback clears sentinel.
  Maps to: FR-M05.

- [ ] **MIG-03:** Implement `cmd_migration_cutover` with bridge token
  regeneration and PCA sentinel. Acceptance criteria: bridge token
  regenerated by default; PCA sentinel file prevents PCA restart.
  Maps to: FR-M07, IMP-03.

- [ ] **MIG-04:** Implement `cmd_migration_rollback`.
  Acceptance criteria: rollback restores pre-import SQLite state;
  PCA directory unchanged; operator can restart PCA.
  Maps to: FR-M05.

- [ ] **MIG-05:** Implement `polkagent migration verify` post-import checks.
  Acceptance criteria: C2-T07 passes; identity, state, transport,
  provider, workspace, file delivery all validated.
  Maps to: FR-M09.

---

## APPENDIX E: REFERENCE FILE MAP

This table maps each Polkagent subsystem to the reference files that
informed its design, with the key patterns each reference contributes.

| Component | Roko Files | Bardo Files | Key Patterns |
|---|---|---|---|
| Role-based prompt composition | `/Users/will/dev/nunchi/roko/roko/crates/roko-compose/src/templates/mod.rs` | - | `RolePromptTemplate` trait: typed input structs, no filesystem I/O in templates, section priority + cache layer model |
| Conductor/orchestration role | `/Users/will/dev/nunchi/roko/roko/crates/roko-compose/src/templates/conductor.rs` | - | Plan-execute-gate-persist loop; DAG-aware task scheduling; escalation after N retries; state snapshot after each task (maps to delivery/turn recovery in Polkagent) |
| Integration test harness | `/Users/will/dev/nunchi/roko/roko/crates/roko-compose/src/templates/integration.rs` | - | Fixture manifest + dependency manifest driven tests; report-only role (no fixes); workspace-wide verification (maps to C0-C3 compatibility test structure) |
| Implementer role + workspace rules | `/Users/will/dev/nunchi/roko/roko/crates/roko-compose/src/templates/implementer.rs` | - | Leaf crate zero-dependency rule; no unwrap() in library crates; cargo check before signal done (maps to polkagent-core boundary rules in section 19.3) |
| Text truncation + budget management | `/Users/will/dev/nunchi/roko/roko/crates/roko-compose/src/templates/mod.rs` (`truncate`, `truncate_tail`) | - | UTF-8-safe truncation at newline boundaries with marker (maps to long-answer chunking in PCA outbound lane) |
| TUI panel layout (two-column split) | - | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/clade/overview.rs` | `Layout::Horizontal` 60/40 split; `Block` with `Borders::ALL`; display state extracted once per frame via watch channel snapshot |
| Status indicators with color coding | - | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/clade/overview.rs` (`vitality_color_for`, `mood_color_for`) | Three-band color scheme: `SUCCESS` (green ≥ 70%), `WARNING` (amber 40-70%), `DANGER` (rose < 40%); maps to delivery/turn health indicators |
| Sibling/node list rendering | - | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/clade/overview.rs` (`sibling_line`) | Glyph + name + role + metric per row; `●/○/◉/✝` status glyphs; `Gauge` for percentage metrics |
| Screen trait + key handling | - | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/clade/overview.rs` (`Screen` impl) | `ScreenId` enum; `render(frame, area, state)` + `handle_key -> Option<AppAction>`; `Tab/BackTab` navigation |
| Display state snapshot pattern | - | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/clade/overview.rs` (`CladeOverviewState`) | `from_app_state(state)` extracts owned data from watch channel once per frame; render path holds no borrows |

---

## APPENDIX F: TUI SURFACE FOR CHAT/MESSAGING

This appendix specifies the terminal UI surfaces required for Polkagent's
operator-facing chat, device/session management, migration, and compatibility
status views. Patterns are derived from the Bardo terminal
(`/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/clade/overview.rs`).

All screens implement the `Screen` trait with `render(frame, area, state)` and
`handle_key -> Option<AppAction>`. Display state is extracted once per frame
from watch channels (the `from_app_state` pattern from Bardo's
`CladeOverviewState`).

### Chat Interface View

The chat interface shows the message thread, typing indicator, delivery status,
and an input area.

```
┌─ Chat: 5GrwvaEF... (Kairi) ──────────────────────────────────────────┐
│ ● online    Session: abc123    Device: phone-a    Lane: clear         │
├──────────────────────────────────────────────────────────────────────┤
│  [2026-07-30 10:00:01]  5Grw...                                      │
│  > please summarize the main.rs file                                 │
│                                                                      │
│  [2026-07-30 10:00:02]  bot  [accepted ✓] [working...]               │
│  ┌─ thinking ──────────────────────────────────────────────────────┐ │
│  │  Reading /src/main.rs (2,340 bytes)...                          │ │
│  └─────────────────────────────────────────────────────────────────┘ │
│                                                                      │
│  [2026-07-30 10:00:45]  bot  [delivered ✓]                           │
│  The main.rs file initializes the Polkagent runtime and sets up      │
│  the transport, delivery ledger, and executor chain...               │
│                                                                      │
│  [2026-07-30 10:01:05]  5Grw...                                      │
│  > what about the outbound lane?                                     │
│                                                                      │
│  [2026-07-30 10:01:06]  bot  [accepted ✓] [running]                  │
│  ├─ progress ────────────────────────────────────────────────────── │
│  │  tool:read /src/outbound_lane.rs ✓                               │
│  └─────────────────────────────────────────────────────────────────  │
│                                                      ... (6 messages)│
├──────────────────────────────────────────────────────────────────────┤
│ [typing]  Kairi is typing...                                         │
├──────────────────────────────────────────────────────────────────────┤
│ Lane: current(r_xyz) | queue: 0 | peer ACK: pending                  │
└──────────────────────────────────────────────────────────────────────┘
  [Tab] next  [r] reset  [s] stop  [f] files  [q] quit
```

**Implementation sketch:**

```rust
// polkagent-ui/src/screens/chat.rs

pub struct ChatScreen {
    conversation_id: ConversationId,
}

#[derive(Debug, Clone, Default)]
pub struct ChatState {
    pub peer_id: String,
    pub peer_display: String,
    pub online: bool,
    pub session_id: String,
    pub device_id: String,
    pub lane_state: String,
    pub messages: Vec<ChatMessage>,
    pub typing_indicator: Option<String>,
    pub pending_turn: Option<PendingTurnInfo>,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub timestamp: String,
    pub sender: MessageSender,
    pub text: String,
    pub delivery_status: DeliveryStatus,
    pub progress_lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeliveryStatus {
    Accepted,
    Working,
    Running,
    Delivered,
    Failed(String),
    Uncertain,
}

impl DeliveryStatus {
    /// Map to the color scheme from Bardo's vitality_color_for pattern.
    pub fn color(&self) -> Color {
        match self {
            Self::Delivered => palette::SUCCESS,
            Self::Working | Self::Running => palette::CYAN,
            Self::Accepted => palette::TEXT_DIM,
            Self::Failed(_) => palette::DANGER,
            Self::Uncertain => palette::WARNING,
        }
    }

    /// Short display glyph for status column.
    pub fn glyph(&self) -> &'static str {
        match self {
            Self::Delivered => "[delivered ✓]",
            Self::Working => "[working...]",
            Self::Running => "[running]",
            Self::Accepted => "[accepted ✓]",
            Self::Failed(_) => "[failed ✗]",
            Self::Uncertain => "[uncertain ~]",
        }
    }
}

impl Screen for ChatScreen {
    fn id(&self) -> ScreenId { ScreenId::Chat }
    fn title(&self) -> &str { "Chat" }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, state: &AppState) {
        let display = ChatState::from_app_state(state, &self.conversation_id);

        // Three-panel vertical: header | messages | footer
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // header: peer info + lane state
                Constraint::Min(0),     // message thread
                Constraint::Length(3),  // typing indicator
                Constraint::Length(2),  // lane status bar
            ])
            .split(area);

        render_chat_header(frame, rows[0], &display);
        render_message_thread(frame, rows[1], &display);
        render_typing_row(frame, rows[2], &display);
        render_lane_status(frame, rows[3], &display);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<AppAction> {
        match key.code {
            KeyCode::Char('r') => Some(AppAction::Chat(ChatAction::Reset)),
            KeyCode::Char('s') => Some(AppAction::Chat(ChatAction::Stop)),
            KeyCode::Char('f') => Some(AppAction::Navigate(ScreenId::Files)),
            KeyCode::Tab => Some(AppAction::NextScreen),
            KeyCode::BackTab => Some(AppAction::PrevScreen),
            KeyCode::Char('q') => Some(AppAction::Quit),
            _ => None,
        }
    }
}
```

### Device/Session Management Panel

Shows all active peer sessions and their device channels.

```
┌─ Sessions ────────────────────────────────────────────────────────────┐
│  2 sessions   3 device channels   1 pending turn                     │
├───────────────────────────────────────────────────────────────────────┤
│  ● 5GrwvaEF... (Kairi)                              active  2h ago   │
│      ├─ phone-a    topic: 0xabc...   established: 2026-07-30 08:00   │
│      └─ tablet-b   topic: 0xdef...   established: 2026-07-30 09:30   │
│                                                                       │
│  ○ 5FHneW46... (Lena)                               idle   3d ago    │
│      └─ phone-c    topic: 0x123...   established: 2026-07-27 14:00   │
├───────────────────────────────────────────────────────────────────────┤
│  Pending turns:                                                       │
│    turn-01  5GrwvaEF...  state: running   started: 10:01:06          │
└───────────────────────────────────────────────────────────────────────┘
  [Tab] next  [d] detail  [r] reset session  [q] quit
```

**Implementation sketch:**

```rust
// polkagent-ui/src/screens/sessions.rs

fn render_session_row(frame: &mut Frame<'_>, area: Rect, session: &SessionDisplay) {
    // Reuse the sibling_line glyph pattern from Bardo overview.rs:
    // ● online  ○ idle  (no ✝ dead; sessions are either active or expired)
    let glyph = if session.online { ("● ", palette::SUCCESS) }
                else              { ("○ ", palette::TEXT_GHOST) };

    let lines: Vec<Line> = std::iter::once(
        Line::from(vec![
            Span::styled(glyph.0, Style::default().fg(glyph.1)),
            Span::styled(session.peer_display.clone(), Style::default().fg(palette::TEXT_PRIMARY)),
            Span::styled(format!("  {}", session.status), Style::default().fg(palette::TEXT_DIM)),
            Span::styled(format!("  {}", session.last_activity), Style::default().fg(palette::TEXT_DIM)),
        ])
    )
    .chain(session.device_channels.iter().enumerate().map(|(i, ch)| {
        let is_last = i == session.device_channels.len() - 1;
        let tree = if is_last { "    └─" } else { "    ├─" };
        Line::from(vec![
            Span::styled(tree.to_string(), Style::default().fg(palette::BORDER)),
            Span::styled(format!(" {}", ch.device_id), Style::default().fg(palette::CYAN)),
            Span::styled(format!("    topic: {}", truncate_id(&ch.topic_key, 10)),
                Style::default().fg(palette::TEXT_DIM)),
        ])
    }))
    .collect();

    frame.render_widget(Paragraph::new(lines), area);
}
```

### Migration Progress Dashboard

Shows live migration progress during `polkagent import-pca`.

```
┌─ Migration: PCA Import ───────────────────────────────────────────────┐
│  Source:  /home/operator/pca-bot/                                    │
│  Phase:   1 - Read-only import                                       │
│  Status:  running                                                    │
├───────────────────────────────────────────────────────────────────────┤
│  Progress                                                            │
│                                                                      │
│  Identity       [████████████████████] 100%  imported               │
│  Sessions       [████████████████████] 100%  3 sessions, 5 channels  │
│  Dedup markers  [████████████████████] 100%  847 markers             │
│  Owed replies   [████████████████████] 100%  2 → UnknownAfterMigr.  │
│  Vault files    [████████████░░░░░░░░]  62%  14 / 23 files           │
│  Config         [░░░░░░░░░░░░░░░░░░░░]   0%  pending                │
│                                                                      │
├───────────────────────────────────────────────────────────────────────┤
│  Issues (review required)                                            │
│  ~ 2 owed replies imported as UnknownAfterMigration                  │
│  ~ 3 unknown env vars: LEGACY_BRAIN_OPTS, OLD_TIMEOUT, BETA_FLAG     │
│  ~ 1 outbound item uncertain (may have been delivered)               │
├───────────────────────────────────────────────────────────────────────┤
│  Rollback:  /data/rollback-before-import.sqlite.gz  [ready]          │
└───────────────────────────────────────────────────────────────────────┘
  Elapsed: 00:00:12   ETA: 00:00:05   [q] abort (safe)
```

**Implementation sketch:**

```rust
// polkagent-ui/src/screens/migration.rs

pub struct MigrationDashboard;

#[derive(Debug, Clone, Default)]
pub struct MigrationState {
    pub source_dir: String,
    pub phase: String,
    pub status: MigrationStatus,
    pub steps: Vec<MigrationStep>,
    pub issues: Vec<String>,
    pub rollback_path: String,
    pub rollback_ready: bool,
    pub elapsed_secs: u64,
    pub eta_secs: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct MigrationStep {
    pub name: String,
    pub progress: f64,    // 0.0 - 1.0
    pub status: StepStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum StepStatus {
    #[default]
    Pending,
    Running,
    Complete,
    Warning,
    Failed,
}

impl StepStatus {
    fn color(&self) -> Color {
        match self {
            Self::Complete => palette::SUCCESS,
            Self::Running  => palette::CYAN,
            Self::Warning  => palette::WARNING,
            Self::Failed   => palette::DANGER,
            Self::Pending  => palette::TEXT_GHOST,
        }
    }
}

fn render_migration_step(frame: &mut Frame<'_>, area: Rect, step: &MigrationStep) {
    // Reuse Bardo's Gauge pattern for progress bars.
    let gauge_color = step.status.color();
    let label = format!("{:<16} {}", step.name, step.detail);
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(gauge_color).bg(palette::BG_RAISED))
        .label(label)
        .ratio(step.progress.clamp(0.0, 1.0));
    frame.render_widget(gauge, area);
}
```

### Compatibility Status Indicators

Shows the C0-C3 compatibility tier status for the running agent.

```
┌─ Compatibility Status ────────────────────────────────────────────────┐
│  Agent: my-bot   Network: products-devnet   Transport: polkadot-app  │
├───────────────────────────────────────────────────────────────────────┤
│  C0  Transport/Protocol    ● VERIFIED    fixtures: 20/20             │
│  C1  Bridge/Harness        ● VERIFIED    fixtures: 10/10             │
│  C2  State Migration       ● COMPLETE    imported: 2026-07-30        │
│  C3  Managed Cloud         ○ NOT ENROLLED                            │
├───────────────────────────────────────────────────────────────────────┤
│  Active checks                                                        │
│    Chain endpoint      ● reachable   wss://rpc.polkadot.io           │
│    Transport session   ● healthy     3 active peers                  │
│    Outbound lane       ● clear       0 pending items                 │
│    Bridge              ● active      1 harness connected             │
│    Executor            ● ready       claude-code v1.2.3              │
│    Vault               ● ok          142 MB / 512 MB                 │
└───────────────────────────────────────────────────────────────────────┘
  [Tab] next  [d] doctor  [t] run tests  [q] quit
```

**Implementation sketch:**

```rust
// polkagent-ui/src/screens/compat_status.rs

pub struct CompatStatusScreen;

#[derive(Debug, Clone, Default)]
pub struct CompatStatus {
    pub c0: TierStatus,
    pub c1: TierStatus,
    pub c2: TierStatus,
    pub c3: TierStatus,
    pub health_checks: Vec<HealthCheck>,
}

#[derive(Debug, Clone, Default)]
pub struct TierStatus {
    pub tier: String,
    pub label: String,
    pub state: TierState,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum TierState {
    #[default]
    Unknown,
    Verified,
    Complete,
    NotEnrolled,
    Degraded(String),
    Failed(String),
}

impl TierState {
    fn glyph_and_color(&self) -> (&'static str, Color) {
        match self {
            Self::Verified | Self::Complete => ("●", palette::SUCCESS),
            Self::NotEnrolled               => ("○", palette::TEXT_GHOST),
            Self::Degraded(_)               => ("~", palette::WARNING),
            Self::Failed(_)                 => ("✗", palette::DANGER),
            Self::Unknown                   => ("?", palette::TEXT_DIM),
        }
    }
}
```

---

## APPENDIX G: CONFIGURATION GUIDE

This appendix specifies all Polkagent configuration settings relevant to
PCA compatibility, migration, and transport operation. Configuration is
stored in `config.toml`; secrets are stored in a separate secret backend.

### PCA Import Settings

These settings control the behavior of `polkagent import-pca`.

```toml
# config.toml -- Migration settings

[migration]
# Source PCA bot directory. Set by the import command; retained for reference.
pca_source_dir = "/home/operator/pca-bot"

# PCA state format version (set by importer; do not edit manually).
pca_state_version = 1

# Path to the rollback export created before import.
rollback_export = "/data/rollback-before-import.sqlite.gz"

# Migration phase. One of: import_complete, cutover_complete.
# Do not edit manually.
phase = "import_complete"

# Whether to retain the old bridge token during cutover (default: false).
# Must be explicitly set to true with a reason; creates an audit log entry.
retain_bridge_token = false
retain_bridge_token_reason = ""

# How to handle unknown PCA env variables during config translation.
# "fail" (default): abort import; "warn": continue with warning; "ignore": silently skip.
unknown_env_policy = "fail"
```

### Transport Configuration

```toml
[transport]
# Transport type: "polkadot-app" or "t3ams".
type = "polkadot-app"

# JS codec bridge configuration (used until native Rust transport is ready).
[transport.codec_bridge]
# Path to the Node.js codec bridge process.
bridge_binary = "/usr/local/bin/pca-codec-bridge"
# Unix socket for Rust<->Node IPC.
bridge_socket = "/run/polkagent/codec-bridge.sock"
# Timeout for individual codec operations (milliseconds).
op_timeout_ms = 5000
# Restart the bridge process if it fails.
auto_restart = true

# Polkadot network RPC endpoint.
[transport.network]
profile = "products-devnet"
rpc_endpoint = "wss://rpc.polkadot.io"
# Interval between polling reconciliation runs (seconds).
poll_interval_secs = 10
# Subscription reconnect delay (seconds).
reconnect_delay_secs = 5
```

### Session Timeout Settings

```toml
[sessions]
# How long an inactive session is retained in the database (days).
# After this period, the session is eligible for cleanup.
inactive_session_ttl_days = 90

# How long a device channel is retained after the last message (hours).
device_channel_ttl_hours = 48

# Maximum number of device channels per conversation.
max_device_channels_per_conversation = 10
```

### Outbound Lane Settings

```toml
[outbound_lane]
# Maximum number of items in a per-conversation pending queue before
# backpressure is applied (default: 100).
max_queue_depth = 100

# Whether to apply liveness takeover when the queue is full (default: false).
# When true, the current unfetched slot is replaced with queued work.
# The replaced message may not be delivered. Always observable in logs.
liveness_takeover_enabled = false

# How long to wait for a peer ACK before considering the slot abandoned (seconds).
peer_ack_timeout_secs = 300
```

### Bridge Settings

```toml
[bridge]
# Enable the C1 bridge HTTP API.
enabled = true

# Listen address for the bridge HTTP server.
listen_addr = "127.0.0.1:8080"

# Bridge token secret reference (stored in secret backend, not here).
# Set by: polkagent secret set bridge-token <value>
token_secret_ref = "bridge-token"

# Lease duration (milliseconds). Harnesses must renew before this expires.
lease_duration_ms = 30000

# Safety margin for lease renewal (milliseconds).
# Recommend renewing at (lease_duration_ms - renewal_margin_ms).
renewal_margin_ms = 5000

# Lease expiry sweep interval (seconds).
lease_sweep_interval_secs = 5

# Proactive token secret reference.
proactive_token_secret_ref = "bridge-proactive-token"

# Unprefixed alias for legacy harnesses (disabled after migration period).
legacy_unprefixed_alias = false
```

### Encryption Key Management

```toml
[secrets]
# Secret backend type: "os-keychain", "file", "env", "in-memory" (testing only).
backend = "os-keychain"

# For "file" backend: directory for secret files.
# Must be mode 0600 and owned by the running user.
file_dir = "/home/operator/.polkagent/secrets"

# Secret references (names only; values stored in backend).
# These are set by the import command or by `polkagent secret set <name> <value>`.
#
# bot_seed_ref         = "bot-seed"          -- Bot account seed phrase
# session_keys_ref     = "session-keys"      -- Encrypted session key store
# bridge_token_ref     = "bridge-token"      -- Bridge HTTP authentication token
# proactive_token_ref  = "bridge-proactive"  -- Bridge proactive authority token
# provider_api_key_ref = "provider-api-key"  -- AI provider API key

# Key rotation: each secret has an independent rotation story.
# To rotate: polkagent secret rotate <ref> [--notify-harnesses]
# Bridge token rotation notifies connected harnesses via the health endpoint.

[secrets.rotation_policy]
# Automatically rotate bridge token every N days (0 = manual only).
bridge_token_rotation_days = 0
# Warn if bot seed has not been backed up within N days.
seed_backup_warning_days = 7
```

### Executor and Tool Settings

```toml
[executor]
# Executor type: "claude", "codex", "opencode", "echo", "bridge", "custom".
type = "claude"

# For custom executor:
# command = "/path/to/custom-brain"
# args = ["--json"]

# Tool capabilities granted to the executor.
[executor.capabilities]
read = true
write = true
bash = false
web = false
subagents = false

# Executor process limits.
[executor.limits]
# Maximum stdout bytes per turn (0 = unlimited; recommended: 10 MB).
max_stdout_bytes = 10_485_760
# Maximum turn duration (seconds; 0 = unlimited; recommended: 300).
max_turn_secs = 300
# Maximum idle time without progress (seconds).
max_idle_secs = 60

# Environment variables to scrub from executor process environment.
# Bot seed, session keys, bridge tokens are always scrubbed.
# Add additional vars here if the executor environment may contain secrets.
[executor.env_scrub]
additional_vars = []
```

### Observability Settings

```toml
[observability]
# Log format: "json" (structured, for production) or "pretty" (for development).
log_format = "json"

# Log level: "trace", "debug", "info", "warn", "error".
log_level = "info"

# Prometheus metrics endpoint (empty = disabled).
metrics_addr = "127.0.0.1:9090"

# Run timeline retention: how many completed turn records to keep.
timeline_retention = 1000

# Correlation ID header for bridge HTTP requests.
correlation_id_header = "x-polkagent-request-id"
```
