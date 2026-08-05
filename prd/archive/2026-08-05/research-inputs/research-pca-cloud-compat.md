# Research: PCA compatibility, cloud control plane, and successor architecture

**Status:** definitive research input for final Polkagent PRDs. It consolidates
PCA source behavior, the current Polkagent research set, and current official
Polkadot product surfaces. It does not claim that the proposed Rust successor
or cloud platform is implemented. Earlier research recommended a narrow
Explain Before Sign validation release; that release discipline does not
override the owner-confirmed complete platform, PCA/mobile compatibility,
managed-cloud, marketplace, payment, or configurable-autonomy goals.

**Evidence snapshot:** PCA repository commit
`2adddcc8cfd732804cd9bbcbcd26974b44b47f66`
(`v0.7.0-8-g2adddcc`), inspected 2026-07-29. Compatibility fixtures must record
the source commit/tag they represent; future source changes do not silently
change C0–C3 claims. Official Product SDK links were also accessed
2026-07-29 and must be rechecked before implementation.

## Read this first: what exists today and what is proposed

This document uses two names throughout:

- **PCA** means **Polkadot Chat Agents**, the existing Node.js reference
  project in `/Users/will/dev/par/polkadot-chat-agents`. It already runs real
  encrypted chat bots through Polkadot-app chat and T3ams. Statements beginning
  “PCA does” describe observed/documented current behavior.
- **Polkagent** means the proposed Rust-first successor in
  `/Users/will/dev/par/polkagent`. Statements beginning “Polkagent will” or
  “proposal” are requirements/design proposals, not implemented behavior.

The reader does not need prior Polkadot knowledge to use this document. PCA is
best understood as a bot runtime placed between a private chat transport and
an AI agent:

```text
phone or T3ams user
       │ writes a message
       ▼
encrypted store-and-forward transport
       │ (no bot webhook/public HTTP port)
       ▼
PCA bot process ──► direct AI CLI, or local framework bridge ──► answer
       │
       └──► encrypted reply reaches the same conversation
```

The bot holds its own network identity and can run on a laptop or small server
with only outbound network access. PCA can call an agent CLI itself (a
**direct brain**) or expose a local authenticated HTTP API for an external
agent framework (a **harness**). Polkagent preserves those user-visible modes,
but gives them one durable state model and a safer Rust implementation.

### Glossary

| Term | Meaning in plain language |
| --- | --- |
| **Polkadot app transport** | PCA’s observed private-chat integration with the Polkadot App and its application-layer session protocol. It is not the same thing as the public Statement Store primitive. |
| **Statement Store** | A public Polkadot/People Chain statement-gossip primitive: signed, short-lived statements can be submitted and retrieved by topics/destination. Its public RPC and higher-level sign-in/app wire formats remain provisional, so it is not by itself a stable chat-bot SDK. |
| **T3ams** | A separate Polkadot chat surface supporting DMs, workspace/channel messages, mentions, threads, reactions, typing, and rich media. |
| **Transport** | The component that authenticates, receives, ACKs, decrypts, and sends chat traffic. It must not decide which model to run. |
| **Conversation** | A private peer chat, T3ams DM, or T3ams channel/thread context whose user turns must remain ordered. |
| **Opener** | The first encrypted request that establishes a new chat session. Subsequent messages are **follow-ups**. |
| **ACK** | An acknowledgement. PCA/Polkagent use several different ACKs: source transport receipt, peer acknowledgement of outbound data, and harness lease acknowledgement. They are not interchangeable. |
| **Owed reply** | PCA’s durable record that a message was accepted and still needs agent handling. Polkagent calls this a pending turn/delivery. |
| **Lane** | A per-peer outbound queue that prevents later Statement Store submissions from replacing an earlier, unfetched one. |
| **Direct brain** | PCA starts an AI CLI such as Claude Code, Codex, or OpenCode for a turn. |
| **Harness** | A separately running agent framework, such as Hermes or OpenClaw, which performs turns through the bridge. |
| **Bridge** | PCA’s local authenticated HTTP compatibility API for a harness. It provides leased inbound work and safe outbound/artifact operations. |
| **Lease** | A short-lived claim on a bridge delivery. A worker renews it while working and ACKs only when it safely handled the message. Expiry makes it available again. |
| **Outbox** | Durable records of replies/actions that need sending. Polkagent makes this explicit rather than relying only on in-memory queues. |
| **HOP / Bulletin / BCTS** | PCA transport-specific storage/delivery mechanisms for encrypted attachments and files. They are never generic web URLs trusted by an agent. |
| **Control plane** | Optional-per-deployment management service for tenancy, fleet configuration, policy rollout and observability; it is deliberately separate from private data-plane state whether that plane is local or managed. |

### Public primitives, PCA-observed behavior, and open compatibility work

The following distinction prevents a reader from mistaking either a public
Polkadot primitive or PCA's working Node.js integration for a complete, stable
Rust transport specification.

| Category | What is established as of 2026-07-29 | What Polkagent may conclude |
| --- | --- | --- |
| **Public primitive** | Statement Store is a signed, allowance-gated, best-effort gossip/store-and-forward primitive. Public documentation describes the surface as evolving; a statement is short-lived and a later statement can replace an earlier current channel value. [Polkadot SDK statement RPC source](https://github.com/paritytech/polkadot-sdk/tree/master/substrate/client/rpc-api/src/statement), [Polkadot data-storage reference](https://docs.polkadot.com/reference/polkadot-hub/data-storage/) | Treat it as a transport dependency behind a versioned adapter, never as the durable source of truth or a generic reliable queue. |
| **PCA-observed application behavior** | PCA at the pinned commit implements encrypted opener/follow-up sessions, device-channel handling, ACK distinctions, polling/reconciliation, ordered outbound lanes, and attachment handling on top of its own codec and state. | These are C0 compatibility behaviors to preserve through fixtures. They are evidence from PCA, not claims that every behavior is a public protocol guarantee. |
| **Public Product/App host** | The Product SDK is a TypeScript SDK for Product/App host or web-gateway integrations; it is not a generic standalone daemon runtime. The documented external sign-in/approval wire remains subject to host/protocol maturity. [Platform Services SDK](https://docs.polkadotcommunity.foundation/guides/platform-services-sdk/), [developer quickstart](https://docs.polkadotcommunity.foundation/getting-started/developers/) | A Product/App companion can be an optional surface. It cannot be a requirement for bot delivery, local operation, or a Rust daemon. |
| **Unknown / unverified for a native successor** | No stable public Rust codec or canonical public test corpus was established here for PCA's application-layer encryption envelope, session-topic derivation, per-device discovery, ACK lifecycle, or external-agent mobile signing request. | Do not claim native Rust C0 interoperability until byte-level vectors, a real-client smoke test, restart tests, and an explicit compatibility decision exist. A JS compatibility bridge remains an acceptable phased implementation route. |

The links above are evidence for the public boundary, not substitutes for PCA
fixtures. Network/profile metadata, Product SDK support, and Statement Store
semantics must be rechecked at implementation time.

## PCA: the user experience being preserved

An operator can install PCA, create a bot identity and username, choose a
brain, and run it. A private bot normally has an allowlist, so only named
accounts can trigger model cost or tools. A user opens the bot in the Polkadot
app, sends a message, sees a “thinking” indicator for slow work, and receives
an ordered answer. The same bot can retain a per-conversation native AI session,
accept a temporary attachment, explicitly save a file to that conversation,
work in an approved project, switch an approved model, and use commands such
as `/reset`, `/project`, `/model`, `/reasoning`, `/usage`, and `/file`.

PCA can instead be created for T3ams. There it can answer direct messages and
workspace mentions, keep replies in a thread, show typing/progress/reactions,
and exchange encrypted rich files. The operator still runs the process and
keeps the identity keys; neither chat path requires exposing a webhook.

The corresponding first-time Polkagent experience should be:

```text
1. Create or import an agent identity.
2. Select a transport profile: local test, PCA-compatible Polkadot app/T3ams
   where its evidence gate has passed, or another separately supported adapter.
3. Select an executor profile: echo, direct CLI, or bridge harness.
4. Review the effective sender/model/tool/workspace/budget policy.
5. Run a transport self-test before declaring the agent ready.
6. Use chat, CLI, web, or an optional Polkadot-app companion to inspect status,
   artifacts, policy decisions, and any requested approval.
```

PCA is the compatibility baseline for private chat operation. Polkagent’s
first proof can still be narrower—such as decoding and explaining one proposed
chain action before external signing. That proof exercises the durable kernel
without redefining Polkagent as a single-purpose product, and supporting PCA
does not require shipping every future agent/product feature in the same
release.

## Decision summary

Polkagent should be a **Rust-first durable agent runtime** with equal,
portable local/self-hosted and managed multi-tenant modes plus an optional
Polkadot-app companion. It is neither a rewrite that drops PCA compatibility
nor a cloud product that makes local operation second class. “Optional control
plane” means a deployment can operate without it, not that managed cloud is an
unimportant product track.

Three layers remain intentionally separable. The data plane is required, but
it may run on a user device, self-hosted server, organization cluster, or
isolated managed worker:

```text
Portable data plane (required)       Control plane (optional per deployment) Optional user surfaces
Rust daemon + store + artifacts   ↔  tenancy/fleet/policy/observability    ↔ Polkadot app, web, CLI, chat
transport + executor + outbox        no plaintext by default across plane    signed approval / rich UX
```

The successor must preserve PCA’s hard-won transport semantics (E2E sessions, device channels, durable-before-ACK, Statement Store outbound lanes), its bridge contract, and its operator-friendly bot lifecycle. It should improve PCA by making every effect/outbound send durable, splitting the huge process entrypoint into typed Rust ports, and treating cloud/admin/approval UX as independent, least-privilege products.

## How PCA works today

### Two execution paths

PCA has one transport core and two ways to obtain an answer:

```text
                         PCA transport core
                     identity + sessions + ACKs
                          files + outbound lane
                          /                 \
                         /                   \
                direct brain                 bridge brain
            spawn a local agent CLI      local authenticated HTTP API
          Claude/Codex/OpenCode/echo      Hermes/OpenClaw/custom harness
```

**Direct brain:** PCA starts a headless coding-agent CLI for an incoming
message. Its runner converts each CLI’s streaming JSON into a small common
vocabulary: session started, tool/action progress, partial/final text, usage,
or error. PCA owns the conversation queue, native session resume identifier,
commands, thinking state, file staging, and final reply delivery. It starts
with no tools; the operator can deliberately grant portable capabilities such
as `read`, `write`, `bash`, `web`, and `subagents`.

**Bridge brain:** PCA still owns all Polkadot/T3ams cryptography and message
delivery, but a harness long-polls a local bridge for leased inbound messages.
The harness renews its lease during slow work, sends reply/artifact operations
back to the bridge, then ACKs the lease. Hermes is a Python agent plugin;
OpenClaw is a TypeScript channel plugin. A custom framework can implement the
same loop.

| Concern | PCA direct brain | PCA bridge brain | Polkagent proposal |
| --- | --- | --- | --- |
| Agent loop | PCA child process | External framework process/service | `TurnExecutor` contract for both. |
| Session memory | CLI-native resume ID per peer | Framework-owned | Opaque `ExecutorSessionRef`, scoped to executor/model/workspace revision. |
| Work claim | PCA internal queue and owed work | HTTP lease and ACK | One durable delivery/turn claim state machine. |
| Tools/files | CLI-specific policy compiled by PCA | Framework handles agent tools; bridge mediates chat vault | Immutable resolved grant and mediated artifact/tool ports. |
| Reply | PCA sends itself | Harness requests bridge send | Durable egress intent/outbox for either path. |

### Protocol and transport architecture

The default transport’s critical sequence is more than “receive text, call a
model, send text.” A new conversation is an encrypted opener. PCA verifies its
identity proof, establishes session keys, and derives session channels. The
phone may subsequently send follow-ups on **device-specific** channels rather
than only the identity channel; missing those channels makes a bot appear to
work in a test client but fail with a real phone.

```text
new chat:
phone ── encrypted opener ──► bot
phone ◄─ accept/session info ─ bot
                          │
follow-up:                 ▼
phone ── encrypted device-session request ──► bot
phone ◄─ session ACK ─────────────────────── bot
```

PCA uses both a push subscription and a periodic polling/reconciliation loop.
Neither alone is sufficient: subscriptions reduce latency; polling recovers
from missed events, restarts, and unhealthy connections. The transport must
also process batches defensively, so one unknown future content item does not
discard valid text in the same batch.

### The three acknowledgement concepts

| Acknowledgement | Who sends it | What it means | What it must never be mistaken for |
| --- | --- | --- | --- |
| **Inbound transport ACK** | Bot → chat peer | The source message is durably accepted; peer can stop resending. | “The model answered” or “the reply was delivered.” |
| **Outbound peer ACK** | Chat peer → bot | Peer fetched/acknowledged the current outbound Statement Store request. | A harness lease ACK or final model success. |
| **Bridge lease ACK** | Harness → bot | Framework safely completed/accepted that leased bridge delivery. | Source transport ACK or remote peer receipt. |

This distinction is non-negotiable. It permits fast source acknowledgement
while a slow model works, prevents resend storms, and still ensures accepted
work survives crashes.

### Example: inbound message to final reply

```text
1. Transport receives/decrypts a source request.
2. Validate sender, session, message class and bounded admission.
3. In one durable operation, record semantic dedup + owed reply/pending turn.
4. Send source transport ACK. If step 3 failed, do not ACK.
5. Give work to the conversation queue or a bridge lease.
6. Executor/harness emits progress, tool actions, artifacts and final text.
7. Create ordered reply intents, including chunks and placeholder finalization.
8. Lane submits safely; track peer ACK/reconciliation independently.
```

PCA persists step 3 in `session-state.json`; its code calls this an “owed
reply.” Polkagent will perform it in a SQLite transaction containing delivery,
dedup, pending turn, and any admission reservation. That is the central
reliability improvement, not a cosmetic storage substitution.

### Why an outbound lane exists

The Statement Store keeps one current statement per account/channel. If a bot
submits a second independent statement before the recipient fetches the first,
the new statement can replace the older one. PCA’s lane therefore does this:

```text
lane empty ── submit message A ──► current(A, request-id-1)
                                   │ peer ACK request-id-1
                                   ▼
                              submit queued B

while A is current:
  - if B fits, replace A with a new statement containing [A, B] under request-id-2;
  - if it cannot fit, keep B in a bounded FIFO queue;
  - ignore ACKs for request-id-1 after replacement; they do not ACK request-id-2.
```

The replacement is safe only because the new payload is a superset and client
message IDs are deduplicated. A liveness grace policy may eventually replace an
unfetched current slot with queued work for an unreachable peer; that trade-off
must be observable because it can sacrifice eventual visibility of the old
message for future progress. Polkagent preserves this behavior behind a
transport-specific durable lane, never through a generic unordered “send.”

### Existing PCA versus proposal boundary

| Existing PCA behavior | Polkagent successor requirement |
| --- | --- |
| Node.js service, atomic JSON snapshots, in-memory lane/queues. | Rust service, SQLite/WAL ledger and durable outbox/lane state. |
| Individual direct runners compile policy to each CLI. | Same runner compatibility plus a canonical `ResolvedGrant` and enforcement evidence. |
| Bridge is a local HTTP poll API. | Preserve it as C1; add versioned schema and later local streaming/RPC options. |
| Operator deploys one bot per local/server directory. | Preserve local operation; optionally enroll agents in a managed fleet without centralizing secrets. |
| PCA manages chat and agent calls. | Also support trusted approval/evidence workflows, but do not widen initial product scope automatically. |

## Evidence base and exact PCA source map

All paths beginning with `/` in the following source maps are
**PCA-repository-relative**, not filesystem-root paths. For example,
`/bot-core/index.mjs` means
`polkadot-chat-agents/bot-core/index.mjs`. Each row summarizes the relevant
behavior inline; opening the path is optional provenance verification.

### System behavior and security ground truth

| Subject | PCA source | Required reading / successor consequence |
| --- | --- | --- |
| Product topology and package layout | `/Users/will/dev/par/polkadot-chat-agents/README.md`; `/bot-core/README.md` | Preserve outbound-only bot topology and direct-vs-bridge choice. |
| Security boundary | `/docs/explanation/architecture.md`; `/docs/guide/access.md` | Separate transport identity/state from agent provider homes/workspaces; private-by-default is the product default. |
| Wire/session invariants | `/docs/explanation/protocol.md`; `/bot-core/vendor/app-chat-codec.mjs` | Port only after byte-level fixtures and interop; do not “simplify” session/ACK/channel behavior. |
| Core default transport composition | `/bot-core/index.mjs` | Source of session restore, ingress, owed work, HTTP bridge, direct runtime, egress wiring; successor decomposes this composition root. |
| Network profiles and metadata | `/bot-core/lib/network-config.mjs`; `/bot-core/lib/descriptors.mjs`; `/bot-core/scripts/sync-descriptors.mjs` | Network/config profile is versioned data, not arbitrary per-turn input. |
| State write discipline | `/bot-core/lib/session-store.mjs` | Preserve critical durable write before ACK; upgrade JSON snapshot to transactional SQLite. |
| Tests/reference clients | `/bot-core/test/`; `/bot-core/test/t3ams/`; `/bot-core/test-client.mjs`; `/bot-core/test-client-device.mjs`; `/docs/guide/testing.md` | Adopt fixtures and mocks as the compatibility corpus. |

### Runtime, provider, and tool behavior

| Subject | PCA source | Required successor treatment |
| --- | --- | --- |
| Direct turn runtime | `/bot-core/lib/agent-runtime.mjs` | Separate generic external-process execution, staging, cancellation, native session IDs, progress, artifacts, and per-peer state into Rust executor/runtime ports. |
| CLI runners | `/bot-core/lib/runners.mjs` | Retain runner adapter pattern for Claude, Codex, OpenCode, and custom JSONL; parse to one small typed event vocabulary. |
| Portable policy | `/bot-core/lib/tool-policy.mjs` | Retain outcome-based capabilities; improve with verified sandbox/effect/network/secret enforcement reports. |
| In-chat command semantics | `/bot-core/lib/commands.mjs`; `/bot-core/lib/file-commands.mjs` | Move engine-agnostic commands/policy into core so bridge and direct paths behave consistently. |
| Model project workspaces | `/bot-core/lib/workspaces.mjs` | Preserve alias validation, branch/worktree isolation, bounded subprocess execution; make workspace grants auditable. |
| Per-conversation scheduling | `/bot-core/lib/keyed-dispatcher.mjs` | One serial logical actor per conversation, bounded global concurrency, fair cross-conversation work. |

### Transport UX, artifacts, and framework integration

| Subject | PCA source | Required successor treatment |
| --- | --- | --- |
| One-slot outbound reliability | `/bot-core/lib/outbound-lanes.mjs` | Durable per-channel outbox lane; current statement, superset extension, ACK, grace/takeover behavior are transport-specific invariants. |
| Live response UX | `/bot-core/lib/live-reply.mjs`; `/bot-core/lib/chunk.mjs` | Preserve ACK-gated edits and final-as-new-message behavior; expose typed progress internally. |
| Attachment HOP | `/bot-core/lib/hop-client.mjs`; `/bot-core/lib/media-store.mjs` | Treat remote refs as hostile; bounded cache and trusted endpoint policy. |
| Durable chat files | `/bot-core/lib/file-store.mjs`; `/bot-core/lib/file-commands.mjs` | Conversation-scoped vault/artifact namespace only; no arbitrary host paths. |
| Bridge contract | `/docs/reference/bridge.md`; routes in `/bot-core/index.mjs` | Keep C1 compatibility under versioned Polkagent routes and fixture tests. |
| Hermes adapter | `/hermes-plugin/polkadot/adapter.py`; `/hermes-plugin/polkadot/plugin.yaml` | Preserve poll/lease/renew/ACK behavior; offer a supported migration adapter. |
| OpenClaw adapter | `/openclaw-plugin/polkadot/src/{bridge,gateway,channel,accounts}.ts`; `/openclaw-plugin/polkadot/index.ts` | Preserve account config, channel security, keyed dispatch, proactive fence; offer a supported migration adapter. |
| T3ams composition | `/bot-core/t3ams.mjs`; `/bot-core/transports/t3ams/index.mjs` | Implement as a first-class transport crate, not a pile of conditionals. |
| T3ams subdomains | `/bot-core/transports/t3ams/t3ams-{identity,protocol,routing,submission,message-lifecycle,agent-session,agent-turn,attachments,media,channel-context,subscription-set,health,doctor,live-revocation,direct-capacity,delivery-failure,media-budget,media-analyzer}.mjs` | Use this already modular decomposition as the Rust transport module map. |

### Bot lifecycle, configuration, and deployment

| Subject | PCA source | Required successor treatment |
| --- | --- | --- |
| CLI/configuration | `/bot-core/cli.mjs`; `/docs/reference/cli.md`; `/docs/reference/configuration.md` | Preserve common operational flows but replace secret-bearing environment blobs with typed config + secret references. |
| Create/register identity | `/bot-core/lib/register.mjs`; `/bot-core/vendor/lib/wallet-keys.mjs`; `/tools/bandersnatch-cli/` | Keep explicit identity ownership and registration recovery; import/migrate only under operator control. |
| Testnet storage allowance | `/bot-core/lib/testnet-file-allowance.mjs`; `/docs/guide/devnet.md`; `/docs/guide/files.md` | Preserve as a named-testnet optional operational adapter; never infer a production funding authority. |
| Deploy | `/docs/guide/deploy.md`; deployment generation in `/bot-core/cli.mjs` | Retain one-command local/SSH/Docker path as a UX target, but generate immutable deployment manifests and secret bindings. |
| T3ams onboarding/operations | `/docs/guide/t3ams.md`; `/docs/reference/configuration.md` §§T3ams | Preserve setup, doctor, capacity, media and channel guidance with typed readiness states. |

## Compatibility promise and matrix

Compatibility is behavioral and versioned. Polkagent must not read a PCA state directory concurrently, claim exact internal-file compatibility indefinitely, or silently reinterpret a key/configuration.

### Compatibility levels: C0 through C3

| Level | Promise | User meaning | Release boundary |
| --- | --- | --- | --- |
| **C0 — transport/protocol** | A selected Polkagent transport speaks the same relevant encrypted chat/session protocol as PCA. | A phone/T3ams user can start, continue, restart, receive ordered replies, and use enabled rich features without losing messages. | Requires byte-level fixtures, device-channel tests, no-ACK/lane tests, and a disposable live identity smoke test. |
| **C1 — bridge/harness** | A PCA-shaped bridge poller can safely consume Polkagent deliveries and publish allowed responses. | Existing Hermes, OpenClaw, or custom framework integration can migrate without redesigning its poll/renew/ACK/send loop. | Requires OpenAPI/JSON fixtures and stale-lease, media/vault, edit, proactive-token tests. |
| **C2 — state/operator migration** | An operator can deliberately import a PCA bot with an explainable report and rollback point. | Existing identity, policy, sessions, files, and operational workflow move without accidental parallel service or hidden replay. | Requires validated importer, backup/export, pending-work review, deploy/doctor parity checks. |
| **C3 — managed product continuity** | Cloud/companion surfaces operate enrolled local/self-hosted data planes or isolated managed workers without reducing C0–C2 privacy or authority boundaries. | Teams gain fleet policy, health, approvals and collaboration while portable runtimes remain independently operable and exportable. | Requires tenancy isolation, signed desired-state rollout, recovery drill, portability proof, and no cross-plane plaintext/seed access by default. |

C0 is not a claim that every PCA feature is enabled on every Polkagent profile;
it is a promise that advertised transport capabilities behave correctly. C1 does
not grant a harness more authority than a direct executor. C2 is an explicit
migration product, not a file-format accident. C3 is optional and cannot become
a required dependency for private chat delivery.

| PCA capability | C0 transport parity | C1 bridge/harness parity | C2 migration/UX parity | Successor improvement |
| --- | --- | --- | --- | --- |
| Outbound-only encrypted chat | Required | N/A | Bot `run`/status workflow | Rust transport port, typed connection/health state. |
| Bot identity / registration | Required for selected transport | Health identity fields | Explicit import/export/create/register recovery | Secret references, rotation metadata, backups, no seed leaked to executor. |
| Opener and follow-up sessions | Required | Inbound projection | Import sessions only by validated one-time migration | Durable per-session records and protocol-vector tests. |
| Per-device channels | Required | Transparent | Preserve on import | Dedicated session-watch projection and diagnostics. |
| Durable-before-ACK / dedup / owed work | Required | Leased delivery must map to same durable turn | Import pending owed work as unknown/reviewable attempts | SQLite transaction, effect intent, explicit recovery state. |
| One-slot outbound lanes | Required | Bridge send reflects durable outbox order | Preserve pending outbound reconciliation where possible | Durable lane/outbox, explicit uncertainty and event trace. |
| Direct Claude/Codex/OpenCode | Not transport requirement | Optional | Match `brain` mappings or clear importer mapping | `TurnExecutor` + subprocess service, version probes, typed normalized stream. |
| Hermes/OpenClaw | N/A | Required route/lease semantics and maintained adapters | Generated deployment migration guide | SDK/RPC harness support beyond polling bridge. |
| Bridge HTTP | N/A | Required C1 field and operation behavior | Alias/deprecation path, token migration | `/v1/compat/pca`, OpenAPI, contract fixtures, per-turn scopes. |
| Files/media | Required for enabled capabilities | Same vault/media behavior | Migrate files with manifest/hash/audit | Artifact store classification/encryption, object-store option. |
| Live replies | Required where transport supports | C1 send/edit/typing semantics | Preserve settings where sensible | Unified capability model and traceable progress. |
| T3ams DM/workspace/thread | Required when T3ams enabled | Required rich fields/lease fencing | Explicit bot/channel import/onboard path | First-class typed transport, no default-transport leakage. |
| Projects/worktrees and commands | N/A | Bridge compatibility does not require direct commands | Map PCA projects to workspace grants | Commands/policy independent of executor kind. |
| CLI/deploy/doctor | N/A | N/A | High-priority operational parity | Declarative manifests, fleet readiness, repair plans. |

### Functional surface inventory

This inventory prevents a “transport works” milestone from silently dropping user-visible PCA behavior. Each item is either parity-required, explicitly deferred, or must yield an operator-visible incompatibility during import.

| PCA surface | PCA evidence | Polkagent compatibility disposition |
| --- | --- | --- |
| `create`, deterministic seed/account/chat key generation, username registration/retry, Products Devnet/Paseo profiles | `/bot-core/cli.mjs`; `/bot-core/lib/register.mjs`; `/docs/guide/{first-bot,devnet}.md` | Required local lifecycle capability; use typed network profiles and secret references. |
| `run`, greeting owner on first start, `info`, `status`, `logs`, `stop` | `/bot-core/cli.mjs`; `/docs/reference/cli.md` | Required CLI job parity; greeting is a durable first-contact effect with allowlist guard. |
| Private allowlist vs public posture, public model-switching restrictions, sender prefilter | `/docs/guide/access.md`; `/bot-core/index.mjs` | Required policy parity; all access decisions become audit records. |
| Direct brain selection, model pinning/allow/open/lock, reasoning effort, native session resume | `/docs/guide/brains.md`; `/bot-core/lib/{runners,commands,agent-runtime}.mjs` | Required direct-runner parity for supported adapters; core policy handles switching. |
| Chat commands `/help`, `/reset`, `/stop`, `/ping`, `/model`, `/reasoning`, `/project`, `/usage`, `/file` | `/docs/guide/commands.md`; `/bot-core/lib/{commands,file-commands}.mjs` | Required where corresponding executor/artifact capability is enabled; unsupported command is explicit, never model-improvised. |
| Project registry, aliases, branch worktrees, one selected project/session per peer | `/docs/guide/projects.md`; `/bot-core/lib/workspaces.mjs` | Required builder-profile parity; project is a policy grant, not a chat filesystem path. |
| Plain/rich/reply/edited text; reactions; call-offer decline; informational coinage/contact/left signals | `/docs/explanation/protocol.md`; `/bot-core/index.mjs` | Required message classification; only text-bearing kinds create turns by default. |
| Attachment staging, caption synthesis, temporary media cache, explicit durable vault, return file | `/docs/guide/files.md`; `/bot-core/lib/{hop-client,media-store,file-store}.mjs` | Required when media/files enabled; use artifact handles and quotas. |
| Long answer chunking, thinking placeholder, progress edits, ACK fallback | `/bot-core/lib/{chunk,live-reply,outbound-lanes}.mjs` | Required rich UX parity for capable transports. |
| Bridge health/inbound/lease/renew/ACK/media/files/send/react/typing/proactive token | `/docs/reference/bridge.md` | Required C1. |
| Hermes deployment and manual OAuth lifecycle | `/docs/guide/harnesses.md`; `/hermes-plugin/polkadot/adapter.py` | Required migration adapter + clear credential/readiness UX; do not import OAuth token by default. |
| OpenClaw channel plugin, pair/allowlist, attached result proactive send | `/docs/guide/harnesses.md`; `/openclaw-plugin/polkadot/` | Required migration adapter; its framework policy remains additional defense, core remains authoritative. |
| T3ams DMs/workspaces/mentions/threads/context, BCTS media, channel admission, doctor | `/docs/guide/t3ams.md`; `/bot-core/transports/t3ams/` | Required only in enabled T3ams profile; all rich fields/fences retained. |
| SSH/Docker compose direct and two-container harness deploy; non-root agent setup; optional media analyzer | `/docs/guide/deploy.md`; deployment generator in `/bot-core/cli.mjs` | Required deployment jobs, but manifests/secret bindings replace shell-concatenated environment ownership. |
| File allowance `status/grant/recover`, profile-specific faucet behavior | `/bot-core/lib/testnet-file-allowance.mjs`; `/docs/reference/cli.md` | Named-testnet compatibility adapter; visible unsupported/disabled state on other networks. |

### Non-negotiable C0 wire and delivery rules

The following are derived directly from `/docs/explanation/protocol.md` and code cited above:

1. A bot uses outbound RPC only; there is no inbound webhook requirement.
2. Session opener identity proof is verified before session/work acceptance; either side can initiate.
3. Follow-ups can arrive on distinct per-device channels; identity-only watching is incompatible with the mobile client.
4. Before ACKing a valid inbound request, persist both semantic dedup and accepted pending work. On failure/full admission, leave it unacknowledged for source retry.
5. ACK means transport receipt, not agent completion. A slow model must not cause inbound resend storms.
6. Decoding is per batch item where possible. Unknown/undecodable items do not erase valid siblings.
7. The Statement Store has one outbound statement slot per account/channel. Do not publish independent messages concurrently; use current-slot ACK, safe superset extension, queue, and documented liveness takeover.
8. Store session/device material, dedup state, pending work, and unresolved outbound state to survive restart.
9. Subscription accelerates ingress, but polling/reconciliation remains a correctness path.
10. Remote attachment endpoints and metadata are attacker input. Use allowlisted transport endpoints, caps, content integrity checks, bounded storage, and no raw ticket/reference exposure outside transport.

## Bridge compatibility contract

### C1 routes and semantics

Polkagent exposes PCA compatibility at `/v1/compat/pca`; a temporary unprefixed alias is optional and must be disabled by default after migration. The source contract is `/docs/reference/bridge.md`.

| Operation | Required request/response behavior | Runtime binding |
| --- | --- | --- |
| `GET /health` | Bearer or `x-bridge-token`; identity, transport, media/files/live capabilities, degraded state. | Read-only health projection; no secrets. |
| `GET /inbound?wait=&limit=&events=` | Bounded long-poll, message rows leased with `delivery_id`, `lease_id`, `lease_ms`, `chat_id`, `text`, `message_id`; signals opt-in. | Atomically claims a durable accepted delivery; never drains unbounded backlog. |
| `POST /inbound/ack` | Single/batch exact lease ACK; stale claim acknowledges zero. | Releases bridge claim only; does not imply remote peer ACK or egress finality. |
| `POST /inbound/renew` | Exact active claim renewal. | Extends lease/fence; finite cap/expiry remains observable. |
| `GET /media/:id` | Authenticated opaque scoped bytes; may materialize lazy media. | Artifact capability, TTL, quota, audit. |
| `GET/PUT/DELETE /files/:chat_id[/path]` | Same-conversation vault only; type/size/path rules. | Artifact manifest transaction; no host absolute path. |
| `POST /send` | Text/reply/edit/file operations; returns durable outgoing `message_id`. | Validate live lease/proactive authority and create ordered outbox intent atomically. |
| `POST /react`, `/typing` | Capability-gated, best effort where needed. | Same fence before egress. |

Every bridge request needs ordinary bridge authentication. Proactive activity additionally needs an independent proactive token and only authorizes entirely unleased actions; it cannot turn a stale lease into a valid one. T3ams compatibility requires active delivery claim fencing for outbound actions and prompt-edit/delete revocation behavior.

### Example bridge exchange

The following is illustrative C1 JSON, preserving PCA field names so existing
pollers can understand it. It is not an authorization token in itself.

```http
GET /v1/compat/pca/inbound?wait=25&limit=4 HTTP/1.1
Authorization: Bearer <bridge-token>
```

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

While the harness works, it renews the exact `(delivery_id, lease_id)` pair.
To reply, it submits the same claim with its operation:

```json
{
  "chat_id": "0xpeer-account-or-t3ams:dm:...",
  "text": "I found a cat sitting on a windowsill.",
  "delivery_id": "d_01J...",
  "lease_id": "l_01J..."
}
```

The bridge atomically checks the lease and creates an ordered outbox item. A
stale lease returns a conflict and creates **no** send. After the harness has
safely completed its turn it calls `/inbound/ack`; if it crashes instead, the
lease expires and Polkagent re-leases the delivery. This is at-least-once work
delivery, so the harness must retain its own idempotency/correlation record.

### Legacy adapters

- **Hermes:** provide an adapter package/config template that preserves its bounded keyed dispatch, lease renewal during a turn, attachment materialization and reply handoff. Reference: `/hermes-plugin/polkadot/adapter.py`.
- **OpenClaw:** retain account/config mapping, allowlist defense in depth, per-chat/thread keyed dispatch, attached-result proactive capability, and delivery routing. Reference: `/openclaw-plugin/polkadot/src/`.
- **Custom pollers:** publish OpenAPI + JSON Schema + executable C1 fixtures. An adapter written against PCA should need endpoint/version/token configuration changes only for baseline message send/ACK flows.

Bridge compatibility is a migration layer. It projects durable core state; it is never the system of record, cannot bypass policy, and cannot directly obtain chain keys or database handles.

## Successor module architecture

The responsibility map below incorporates the useful earlier workspace
decisions inline: use a Rust workspace with domain/types at the dependency
center; keep adapters as leaf crates; record inbound work before ACK; enqueue
effects transactionally; use one logical writer per conversation/partition;
keep provider/model/executor/harness distinct; make grants runtime-owned; treat
the HTTP bridge as a compatibility projection; and use SQLite/WAL as the first
local authority behind storage ports. The earlier blueprint and decision
records remain provenance, not required reading.

```text
polkagent-types       IDs, schemas, safe envelopes, protocol versions
polkagent-core        pure delivery/turn/effect/outbox transitions and ports
polkagent-runtime     keyed actors, admission, leases, cancellation, recovery
polkagent-store       SQLite/WAL events/projections/outbox/leases/migrations
polkagent-artifacts   staging, vaults, content hashes, classification, retention
polkagent-policy      sender/model/tool/workspace/effect/egress decisions
polkagent-executor    normalized event protocol + process/provider adapters
polkagent-transport   normalized records + contract suite
 ├─ polkadot-statement-store  fixture-gated PCA-compatible transport; the
 │                            versioned JS codec bridge is allowed initially
 └─ t3ams                    rich chat transport
polkagent-bridge-http C1 projection/leases/files/media
polkagent-control      optional control-plane API/agent enrollment/fleet policy
polkagent-ui           optional web/app approvals and operator experience
polkagent-cli          local operator workflow and deploy targets
```

### Domain abstractions

| Abstraction | Owns | Must not own |
| --- | --- | --- |
| `Transport` | Identity/session protocol, receive/ACK, safe artifact fetch, transport capabilities, egress submit. | Model selection, persistent core state, broad policy. |
| `DeliveryLedger` | Dedup, durable acceptance, lease, source cursor, origin/request IDs. | Provider/harness-specific sessions. |
| `ConversationActor` | Ordered turn/context policy and cancellation for one conversation/thread. | Direct SQL/HTTP/chain codec calls. |
| `EffectIntent` | Immutable request to invoke executor/tool/signer/egress with digest and idempotency class. | Inference or access policy. |
| `TurnExecutor` | Runs an immutable turn request and emits normalized events. | Transport key, store handle, authority expansion. |
| `HarnessService` | Optional daemon lifecycle/health/version for an external framework. | Conversation scheduling or state ownership. |
| `ArtifactStore` | Conversation-scoped read/write handles, classification, retention and safe materialization. | Arbitrary caller filesystem paths. |
| `Signer` | Exact canonical-payload authorization/signature, where authorization is a human/quorum/service approval or an unexpired configured autonomous mandate. | Broad wallet/seed exposure or authority chosen by a model. |
| `ControlPlane` | Fleet enrollment, desired config/policy release, rollout, telemetry summary, support posture. | User plaintext, bot seed, session keys, raw artifacts by default. |

### One durable state machine, all execution modes

```text
received → accepted(dedup + pending durable) → source-ack-pending/acked
       → leased to executor or bridge → running → execution terminal
       → egress intents durable → lane submission → confirmation/reconcile
```

Each external action has its own `EffectIntent`, one or more
`EffectAttempt`s, a policy/grant digest, deadline, idempotency classification,
and an immutable `EffectOutcome` for every completed attempt. Execution
terminality and transport delivery terminality are different projections. A
completed model response cannot be mistaken for delivered chat text or a
finalized chain action.

### Minimal durable records (proposal)

The database is not a generic event dump. It stores the minimum durable facts
needed to safely recover an agent after a process, provider, network, or
harness failure:

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

Important distinctions:

- `accepted` means a source message is durable and may be ACKed; it does not
  mean the model ran.
- `execution_succeeded` means an executor completed an attempt; it does not
  mean a peer saw a reply.
- `submitted` means an outbound transport accepted a statement/action; it does
  not necessarily mean the peer fetched it or that a chain action finalized.
- an uncertain external effect is reconciled using its canonical request and
  receipt; it is never blindly re-run just because a process restarted.

## Direct runners and native successor behavior

PCA direct engines are `claude`, `codex`, `opencode`, `echo`, and `bridge`; current mapping is documented in `/docs/guide/brains.md` and implemented in `/bot-core/lib/runners.mjs`.

| PCA behavior | Backward-compatible successor | Improvement |
| --- | --- | --- |
| Claude stream-json CLI, resume ID, tools/progress/usage parsing | `exec-claude-code` runner or generic signed/pinned process runner profile | Version probe; fixture parser per CLI version; immutable resolved grant. |
| Codex `exec --json`, session resume, reasoning/model flags | `exec-codex` profile | Native sandbox capability report and exact config evidence. |
| OpenCode JSON format / many providers | `exec-opencode` profile | Provider catalog stays a config profile, not arbitrary chat-supplied slug. |
| Custom Claude-shaped stream JSON command | Generic `jsonl-subprocess` executor | Strict protocol handshake/schema and resource isolation. |
| Native CLI sessions | Opaque `ExecutorSessionRef` per conversation + engine/model/workspace revision | Session is invalidated deterministically when its compatibility scope changes. |
| `/model`, `/reasoning`, `/reset`, `/project`, `/usage`, `/stop`, `/file` | Core command service before executor dispatch | Uniform where meaningful for bridge/direct; policy rather than engine accident. |
| Tool policy `read/write/bash/web/subagents`, workspace/container scope | `ResolvedGrant` with capability and enforcement backend | Network, secret, filesystem roots, resource limits, approval, audit are first-class. |

Direct runner deploys must preserve PCA’s useful security choices: scrub inherited environment, run unprivileged, keep transport state and signing seed out of agent workspace, stage attachment/output leaves safely, bound stdout/time/idle progress, kill process groups on cancellation, and preserve artifact snapshots before deleting staging directories. See `/bot-core/lib/agent-runtime.mjs` and `/bot-core/lib/runners.mjs`.

## Transport, files, and live UX parity

### Files and media

- Incoming media is transient, bounded cache material. It is available to a turn only via a safe artifact handle, not raw peer-provided URL/ticket.
- Durable file retention is an explicit per-conversation action (`/file put` semantics), with list/info/get/remove and per-file/per-peer/global caps.
- Framework-generated file flow remains: upload bytes to the same conversation vault, then send by vault path. No arbitrary `/tmp` or harness path is deliverable.
- HOP download uses trusted host policy, encryption/integrity verification, request/response limits, and no claim ticket leakage. Upload endpoint is operator-pinned and allowance/funding state is explicit.
- PCA’s named-testnet automated Bulletin allowance is retained only as a profile-specific convenience. Production funding, HOP endpoint choice, and public bot file delivery require explicit operator policy.

Source: `/docs/guide/files.md`, `/docs/explanation/protocol.md` §§attachments/files, `/bot-core/lib/{hop-client,media-store,file-store,file-commands,testnet-file-allowance}.mjs`.

### Live reply rules

1. Slow work may generate thinking/typing state only when transport capability and policy permit it.
2. The placeholder must be fetched/ACKed before it is edited; otherwise edit could replace its sole Statement Store slot and create a dangling message.
3. Progress updates are latest-wins/coalesced/throttled; full replacement frames make skipped intermediate updates safe.
4. The final answer is sent as a **new** message after compact placeholder status, preserving phone notification behavior.
5. With no placeholder ACK, final text supersedes the unfetched placeholder as a normal send.
6. Long replies are UTF-8-safe, paragraph/code-fence-aware ordered chunks under transport limits.
7. Lease/cancellation/prompt-edit fences are checked immediately before queued edits/sends/reactions/typing.

Source: `/bot-core/lib/live-reply.mjs`, `/bot-core/lib/chunk.mjs`, `/docs/reference/bridge.md`.

### T3ams requirements

T3ams must not be a default-transport feature flag. Its transport profile includes DMs, workspace/channel mentions, threads, channel-context snapshots, known chats/admission, BCTS/Bulletin media, exact message operation reconciliation, rich attachment fields, public enrollment policy, and SDK/identity key pins. Preserve opaque `t3ams:` IDs and `thread_root_id`; never turn channel context into an independent message to answer. See `/docs/guide/t3ams.md` and `/bot-core/transports/t3ams/`.

## Bot identity, configuration, state, and migration

### Target local layout

```text
<data-root>/agents/<agent-id>/
  config.toml                 # strict typed config; no secret literals
  state.sqlite                # 0600 SQLite/WAL, delivery/turn/outbox/session projections
  artifacts/                  # classified content-addressed storage
  metadata/                   # pinned chain/runtime descriptors
  workspaces/                 # optional operator-owned granted roots/worktrees
  exports/                    # explicit encrypted backups only
```

Secrets live in OS keychain, secret manager, mounted secret file, hardware signer, or a tightly permissioned local secret store and are referenced by ID. The bot seed, session material, bridge token, provider credential, and payment signer are separate secrets with separate rotation/revocation stories.

### PCA import/export requirements

1. Operator stops PCA first; no two services may serve one identity.
2. A migration command backs up and validates PCA bot directory/config/secret/state file permissions and versions before writing Polkagent state.
3. Import maps PCA bot account/identifier/session material, allowlist, brain/model/tool/project policy, settings, dedup markers, sessions, pending owed records, durable vault manifest/files, and unresolved outbound state where available.
4. PCA `session-state.json` is an input format, not a live database. Validate/bound it; transform into SQLite rows transactionally; retain source hash/version and report every dropped/unsupported field.
5. Every owed/pending action imports as `ExecutionStatus::UnknownAfterMigration`, never automatically re-runs an irreversible external effect. Operator chooses review/retry/abandon with evidence.
6. Bridge tokens are regenerated by default; a deliberate short compatibility window may retain old token only if explicitly requested and logged.
7. Old deployment configuration becomes a generated import report plus a typed `config.toml` draft; unknown env variables fail validation instead of silently becoming behavior.
8. Rollback remains possible until a signed/exported cutover confirmation. Do not delete PCA data during import.

### CLI and deployment UX

PCA’s strengths are discoverable commands and simple operator lifecycle: create, run, deploy, status, logs, stop, trust, doctor, project, model, storage, and T3ams setup. Retain these jobs, but make desired state explicit:

```text
polkagent init / import-pca / agent create / agent run
polkagent agent deploy --target local|docker|ssh|kubernetes
polkagent agent status / logs / doctor / backup / restore / rotate-secret
polkagent policy inspect / simulate / approve / revoke
polkagent migration plan / apply / verify / rollback
```

`deploy` produces a reviewed manifest (image digest, config revision, required
secret references, volumes, network policy, execution profile) and applies it
with an idempotent deployment adapter. Direct runners use a dedicated
unprivileged container/process; bridge harnesses use separately scoped service
containers. The same deployment contract targets a local host, self-hosted
cluster, or isolated managed worker. Local operation remains first-class and
never requires the control plane.

## First-class managed cloud and optional control-plane attachment

### Product boundary

Managed cloud can host both an isolated data plane and the control-plane
experience. It is not limited to fleet management, and local-only operation is
still fully functional. A user may:

- run a local/self-hosted data plane without a control plane;
- enroll that data plane in a managed or self-hosted control plane;
- run the data plane as an isolated managed worker; or
- combine private local workers with managed scheduling and collaboration.

Enrollment uses public-key and mutual TLS/device authorization. Root chat
identity or wallet seeds are not uploaded by default. A user may explicitly
choose managed KMS/HSM/MPC or encrypted secret/custody services under separate
consent, keys, recovery, access, and audit policy.

| Plane | May hold | Must not hold by default |
| --- | --- | --- |
| Portable data plane: local, self-hosted, or isolated managed worker | Decrypted messages, scoped session keys, state DB, artifact plaintext, provider connection and bot identity material required for that deployment. | Other tenants’ data; control-plane root credentials; unscoped support access. |
| Control plane | Tenant/org/member metadata, agent public identity, desired config/policy revisions, extension digests, rollout/health summaries, redacted metrics/audit hashes, support tickets. | Chat plaintext, bot seed, session keys, provider OAuth refresh tokens, raw vault/media artifacts. |
| Optional encrypted backup | Customer-managed or tenant envelope-encrypted state/artifacts with retention/key policy. | Provider to inspect/use plaintext without explicit recovery authorization. |
| Approval UI | Canonical intent/evidence summary and signed approval decision. | General signing key material; arbitrary executor control. |

### Multi-tenancy model

```text
Organization → project/environment → agent → transport identity + executor profiles
                       │
              membership / roles / policy revisions / audit retention
```

- Tenant isolation is enforced in control-plane database queries, encryption key hierarchy, object prefix/bucket policy, rate/cost quotas, and audit access—not only UI filters.
- An agent belongs to one environment at a time. Moving it requires explicit export/import or re-enrollment, preserves audit lineage, and rotates scoped credentials.
- Roles are `owner`, `operator`, `approver`, `auditor`, `developer`, `support-limited`; no support role can view secrets/plaintext by default.
- Desired policy/config is signed/versioned and pulled by the data plane,
  whether locally or managed. The data plane reports its applied revision and
  can reject incompatible/dangerous changes. A control plane cannot silently
  widen an active turn’s grant.
- Fleet controls are pause, rollout, drain, rotate bridge credential, revoke extension, disable signer/payout, and request encrypted backup—not remote shell access.

### Cloud trust and compliance constraints

Cloud adoption changes the threat model: customer data processing,
processor/subprocessor contracts, regional storage,
retention/deletion/export, audit log access, abuse handling,
availability/service-level objectives, incident disclosure, and
paid-agent/payment regulation all need product/legal review. Payments require
typed quote/authorization/settlement/receipt states, reconciliation,
refund/dispute handling, fraud controls, accounting, and unknown-finality
operations before real-value rollout. A control plane must not accidentally
become a custodian merely by storing or relaying signing authority; managed
KMS/HSM/MPC or funded-agent custody is a separate explicit mode with recovery,
rotation, revocation, and audit.

### Polkadot App/Product companion

The public Product SDK is a TypeScript integration surface for Product/App hosts
and their web gateway, not a generic runtime for a standalone Rust bot daemon.
That distinction matters: a host may help with mobile identity, rendering, or
user-mediated signing, but Polkagent's durable admission, executor, outbox,
and recovery semantics remain in the portable data plane. [Platform Services
SDK](https://docs.polkadotcommunity.foundation/guides/platform-services-sdk/),
[developer quickstart](https://docs.polkadotcommunity.foundation/getting-started/developers/)

The public app/Product material does not establish a stable external-agent
chat, encrypted-session, or signing-request wire contract that a native Rust
bot can implement from documentation alone. PCA demonstrates working behavior
at its pinned source revision, but that is compatibility evidence rather than a
public stability promise. Therefore a companion can offer mobile approval,
account identity/proof, attachment visualization, and operator run cards only
after its host capability is proven. It is **not** the bot daemon, its
availability cannot be required for message delivery, and a host signing action
is bound to an immutable local/remote `ApprovalRequest` digest.

**Product/App evidence gate:** pin the host and SDK version; prove that a
companion can render the exact canonical approval facts, bind a response to an
unexpired `ApprovalRequest` digest, report cancellation/refusal, and leave the
data plane fully functional when the host is unavailable. If the external wire
contract remains undocumented, retain a web/CLI/external-wallet approval route
and do not advertise native mobile approval as compatible.

## Top-tier UX requirements

### First-run experience

1. **Choose mode:** local private bot, local builder agent, T3ams workspace agent, import PCA, or connect a managed tenant.
2. **Identity card:** account, username/status, network, transport capability, backup/secret location, no exposure of seed.
3. **Provider/harness card:** model/provider, credential readiness, tool grant, workspace scope, budget, and exact enforcement limitations.
4. **Test before trust:** deterministic echo/local test plus actual transport self-test; no blank “running” state.
5. **Operator-ready deploy:** deployment plan diff, secrets required, volumes/network mounts, capability warning, health/rollback readiness.

### Conversation UX

- Make delivery state legible: received, queued, working, waiting for approval, sending, delivered/uncertain/retry—not merely “the model is thinking.”
- Preserve PCA’s compact placeholder/progress/final message behavior; show direct model token/cost only when trustworthy and policy allows.
- Render attachments/files as scoped artifacts with expiry/retention and a clear explicit “save to chat” action.
- Commands remain discoverable, but any capability-sensitive command yields a structured explanation/action card rather than vague model prose.
- T3ams thread/channel context is visibly labeled background context; the reply is routed to the triggering thread.

### Operator UX

- A single **agent overview** shows identity, transport health, current config/policy revision, executor/harness health, queue/outbox depth, artifact storage, costs/budgets, latest failures, and a safe action list.
- **Run timeline** shows causal delivery → policy → executor/tool/effect → egress evidence with redaction/classification. It differentiates execution success from actual delivery/finality.
- **Doctor** produces actionable repair plans: chain endpoint unavailable, bridge token mismatch, metadata drift, provider login expired, file allowance unavailable, T3ams SDK/key mismatch, insufficient disk/quota, stale lease, or deployment-policy conflict.
- **Approval cards** are canonical trusted UI: actor/requestor, network/account, recipient/contract, asset/amount/fees, tool/process/network effects, policy revision, exact payload hash, expiry, and expected proof. Model text is supplemental, never authoritative.
- **Migration center** previews PCA import mapping, secrets requested, unsupported configuration, pending work disposition, test/verification results, and rollback point.

## Feature improvements beyond PCA

| PCA limitation / cost | Polkagent improvement | Guardrail |
| --- | --- | --- |
| Large Node entrypoints intermix transport/runtime/bridge state. | Rust leaf crates and pure state transitions. | Dependency direction and contract tests. |
| JSON snapshot state is atomic but coarse. | SQLite/WAL delivery ledger, turn/effect/outbox tables, projections. | Critical acceptance transaction before source ACK. |
| Bridge is polling-only generic ABI. | Keep C1 long-poll; add versioned OpenAPI, Unix socket/gRPC/SSE process SDK later. | Same lease/fence semantics, capability negotiation. |
| Direct vs bridge policies/commands differ. | Core policy/command service and one durable turn model. | Executor never owns authorization. |
| Tool enforcement depends on CLI capabilities. | Resolved grant plus explicit enforcement backend report. | No false sandbox claims; process/container constraints documented. |
| Owed/direct completion is implementation-specific. | First-class EffectIntent/attempt/outbox states. | No blind replay of external effects. |
| Single-host deployment focus. | One portable data-plane contract plus declarative local, Docker/SSH/Kubernetes, hybrid and managed-worker adapters. | Control plane cannot access data-plane secrets by default. |
| Minimal operational visibility. | Correlated timeline, metrics, doctor/remediation, config/policy diff and rollout states. | Redaction/classification default. |
| Chat is the primary product surface. | Chat remains supported; signed approval/evidence and operator UI become first-class. | The first release remains evidence-gated while later surfaces reuse the same durable state. |
| T3ams/default transport code divergence risk. | Both implement same typed transport/effect/artifact contracts. | Transport-specific features capability-negotiated. |

## Risks and required mitigations

| Risk | Failure a first-time operator would see | Required mitigation |
| --- | --- | --- |
| Protocol drift | Bot receives openers but misses phone follow-ups after an app/runtime update. | Metadata/protocol fixture suite, device-channel live test, explicit incompatible/degraded status. |
| ACK ordering bug | User repeatedly resends or loses a message after a crash. | Transactional durable acceptance before source ACK; source retry on failure. |
| Outbound slot overwrite | Only the last reply bubble appears. | Per-channel durable lane, peer ACK tracking, superset extension and bounded queue. |
| Stale harness worker | A cancelled/edited prompt receives an old answer later. | Lease expiry/revocation fence checked at outbox creation and immediately before send/edit. |
| Prompt injection / tool escalation | An attachment persuades the agent to read a seed or run an unapproved command. | Untrusted content classification, immutable resolved grant, mediated tools, no secret in executor environment. |
| Artifact cross-chat leak | One user fetches another conversation’s file. | Conversation-scoped handles/vaults, auth, capability TTL, quota, audit. |
| Migration replay | Cutover repeats an old paid/tool action. | Stop source service, import pending work as unknown, require review/reconciliation before retry. |
| Cloud overreach | Hosted admin can read chat or use a local bot’s signing key. | Local-first data plane; public-key enrollment; scoped desired-state; no plaintext/seeds by default. |
| Misleading UX | “Done” means only a model finished, not that a reply/transaction arrived. | Timeline and UI distinguish accepted, running, submitted, delivered, finalized, failed, and uncertain. |
| Public-bot abuse | Unknown senders exhaust budget or cause unsafe tools. | Allowlist/private default, admission quotas, low-risk public profile, no public free model switching. |

## Acceptance criteria

### C0 transport acceptance

- Before a native Rust Polkadot-app transport is advertised, record the PCA
  source revision and target network/profile; obtain reproducible byte vectors
  for opener/follow-up encryption, topic/channel derivation, message IDs, ACKs,
  and attachment references. Public Statement Store primitives alone do not
  satisfy this gate.
- A real/disposable client opens a chat, sends follow-ups through a distinct
  device channel, restarts the bot, and receives one ordered semantic reply per
  accepted message.
- A simulated persistence failure prevents source ACK; after recovery and
  resend, exactly one durable delivery/turn exists.
- A malformed item in a batch does not prevent independent valid siblings from
  being accepted.
- With a peer that does not fetch outbound work, the lane neither overwrites
  an earlier independent message nor leaks an unbounded queue; takeover, if
  enabled, is recorded.
- Attachment endpoint, size, integrity, cache and vault boundary tests pass;
  raw encryption tickets never reach an executor/harness.
- If a temporary JS codec bridge is used, byte-level interoperability and the
  same restart/duplicate/lane fixtures must pass across the bridge boundary;
  the bridge is a versioned adapter, not a reason to weaken C0 semantics.

### C1 bridge/harness acceptance

- PCA-shaped Hermes/OpenClaw/custom fixtures can poll bounded deliveries,
  renew a lease, fetch authorized media, send a reply, and ACK it.
- An expired, mismatched, cancelled, or prompt-invalidated lease cannot ACK,
  send, edit, react, or type against the current conversation.
- `events=1` exposes signals; default polling does not accidentally create an
  agent reply to a reaction.
- A bridge file is sent only after it is written to the same chat vault; path
  traversal, cross-chat access, arbitrary host paths, and quota bypass fail.

### C2 migration acceptance

- Import produces a complete mapping report for identity, configuration,
  allowlist, sessions, files, projects, bridge settings, and unsupported
  fields; no PCA file is modified.
- Import refuses a live/ambiguous concurrent identity service and preserves a
  rollback export.
- Every pending owed reply/outbound uncertainty appears in an operator review
  queue; no irreversible effect is automatically replayed.
- `doctor` validates the imported identity, state permissions, transport
  connectivity, provider/harness readiness, workspace grants, and file
  delivery readiness before cutover.

### C3 control-plane acceptance

- Disconnecting the cloud leaves a local enrolled agent able to serve its
  existing transport/policy and report a clear degraded-management state.
- A tenant can see only its agent metadata, summaries, and approved backup
  objects; cross-tenant access attempts are denied and audited.
- A signed desired-config revision cannot silently alter an active grant or
  reveal secrets; the data plane reports acceptance/rejection and reason.
- Backup/recovery, agent revocation, bridge-token rotation, and fleet pause
  are exercised in a documented recovery drill.

## Phased delivery and gates

1. **Foundation:** implement local SQLite-ledger echo runtime and memory transport. Prove duplicate/restart/outbox/lease property tests.
2. **C0 spike:** build PCA-codec/Statement Store compatibility fixtures with
   the pinned codec plus raw RPC or the bounded bridge path, then run a
   disposable identity echo bot. Native Rust transport remains disabled until
   byte vectors and real-client interoperability pass; there is no real
   provider or cloud dependency.
3. **C1 bridge:** `compat/pca` health/inbound/ACK/renew/send/files/media contracts; migrate one Hermes or OpenClaw fixture.
4. **Native runtime:** one direct runner with scrubbed environment, explicit policy/grant, staging/artifact and cancellation. Add CLI/doctor/deploy vertical slice.
5. **T3ams:** typed DM/thread/channel/media integration, rich-operation fencing and readiness doctor.
6. **Product companion:** optional PAPI/Product SDK/web approval and run-view UX; remains non-critical to bot delivery.
7. **Control plane:** opt-in fleet enrollment, desired-config rollouts, redacted health/audit; tenant isolation and recovery drills before paid tier.
8. **Value-moving/public surfaces:** only after payment/public-agent gates, legal/security review, support/abuse operations, and independent risk acceptance.

The earlier compatibility and spike documents are provenance for these rules,
but a reader does not need them to understand the release bar. A compatibility
claim requires the C0–C3 acceptance criteria above plus manual real-device
checks for: opener, device follow-up, restart, unknown batch item, a peer that
does not ACK its lane, attachment/file delivery, reaction without accidental
auto-reply, live response edits, T3ams DM/thread/channel behavior where
enabled, stale bridge lease fencing, migration rollback, and deployment
recovery. The reference PCA testing guide remains a fixture source, not a
substitute for this contract.

## PCA-release defaults and permanent integrity boundaries

- **Not a default:** central custody of chat identities, wallet seeds, user
  provider credentials, or decrypted artifacts. Explicit managed custody may
  be offered through a separately consented, isolated, recoverable signer or
  secret service; models, harnesses, and ordinary extensions still receive
  handles rather than raw keys.
- Automatic migration/cutover that starts Polkagent beside PCA for one identity.
- **Not part of the initial PCA compatibility release:** public unmetered
  agents, permissionless marketplace activation, user billing, autonomous
  spending, XCM/bridge transfers, generic browser automation, or smart-contract
  dependencies. They remain established platform tracks with their own
  security, payment, policy, and operations gates.
- A cloud service whose admin/support access silently overrides local policy, reads private conversations, or signs/broadcasts transactions.
- Claiming a runtime sandbox or chain confirmation is stronger than the actual configured enforcement/finality evidence.
