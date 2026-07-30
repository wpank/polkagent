# Definitive Roko research for Polkagent

## Purpose, status, and how to read this document

This is a self-contained design-research dossier for the proposed **Polkagent**
product. Polkagent is intended to be a Rust-first, Polkadot-native agent
platform: it should help users and developers understand, prepare, approve,
and execute Polkadot actions through configurable autonomy. A deployment may
be fully autonomous when its owner deliberately configures that mode, but a
language model (LLM) still does not receive raw wallet keys or silently expand
its own authority.

The source studied here is **Roko**, a separate Rust workspace at
`/Users/will/dev/nunchi/roko/roko`. Roko is an ambitious agent-runtime project.
It combines an implemented set of Rust crates with a broad v2 design corpus
covering artifacts, event streams, workflows, agents, tools, permissions,
extensions, memory, groups, chains, markets, and operations. It is not a
drop-in dependency recommendation and it is not evidence that all ideas in its
documentation are implemented or suitable for Polkagent.

**Evidence snapshot:** Roko base commit
`7f2d98fcc2e92e8d7238be80c0d8434008601c93`, inspected 2026-07-29. The local
working tree contained an unrelated change under
`tmp/status-quo/backlog/plans/`; none of the source paths used for the
adaptation matrix depended on that file. Future implementation should pin a
source revision before copying or comparing a concrete behavior.

This document makes three kinds of statement:

| Label | Meaning |
| --- | --- |
| **Verified Roko behavior** | Directly supported by inspected Rust source, README, or operational material. Exact paths are included as provenance. |
| **Roko design intent** | A v2 specification or design artifact describes it; it should not be treated as shipped behavior without separate verification. |
| **Polkagent proposal** | A recommendation for Polkagent. It is a PRD-level decision candidate, not current implementation. |

The path references are evidence pointers, not required reading. All context
needed to decide whether a pattern belongs in Polkagent is reproduced below.
Where a section says **Roko evidence and intent**, it intentionally discusses a
specification and corresponding code together. Only behavior explicitly
described as implemented by a named concrete source file is treated as
verified; the broader capability remains design intent. File existence alone
does not prove production readiness, conformance, or deployment.

## Executive conclusion

Roko’s strongest contribution is a small set of architectural disciplines, not
its entire “agent economy” vocabulary:

1. **Durable facts differ from live activity.** Keep material evidence and
   recovery state; stream typing/progress separately.
2. **A deterministic core decides; narrowly scoped drivers perform I/O.** A
   restart must not accidentally repeat a payment, signature request, or chain
   broadcast.
3. **Capabilities and safety gates are owned outside agent-controlled text.**
   An LLM may propose an action, but policy and approval decide whether it can
   happen.
4. **Providers, long-running harnesses, tools, transport, chain read access,
   signing, broadcast, and finality are different responsibilities.** Treating
   them as one generic “tool call” loses the authority boundaries Polkagent
   needs.
5. **Operational views are projections, not a second mutable runtime.** UIs
   and support tooling should recover from durable records rather than inspect
   in-memory task objects.

The recommended Polkagent v1 kernel is deliberately narrower than Roko:

```text
Ingress -> one durable turn actor -> reducer -> transactionally stored
state change + EffectIntents -> isolated workers -> EffectOutcomes -> reducer
                         \-> versioned read-only projections -> UI/API
```

An **actor** here means the sole logical writer for one conversation or action
run. A **reducer** is deterministic code that turns the existing durable state
plus an input into new state and requested side effects. An `EffectIntent` is
the durable command recorded before I/O; an `EffectAttempt` records one claim,
lease, retry, and idempotency lifecycle; the **external effect** is the actual
model call, remote request, signer request, or finality watch; and an
`EffectOutcome` is the immutable result observed for exactly one attempt. An
**outbox** is the durable list of intents waiting to be performed. This
separation is the essential adaptation; generic graphs,
autonomous learning loops, token economics, and a marketplace are not
requirements of the first kernel implementation.

The long-term owner direction is broader than that first kernel. Polkagent is
expected to support permissionless marketplaces/registries, payments, funded
agent accounts, fully autonomous configured action, multi-agent groups,
memory/learning, and equal local/managed-cloud operation. This dossier decides
which Roko abstractions should underlie those products; “not v1” below means
phased behind explicit contracts and gates, not rejected from the platform.

## What Roko is, in practical terms

Roko is a workspace of Rust crates and applications that aims to provide a
general substrate for agentic systems. Its design vocabulary is intentionally
broad:

- A **Signal** is its durable, attributable unit of information. It has an
  identity, type, provenance (where it came from), and can be stored.
- A **Pulse** is an ephemeral, sequence-oriented live event, used for things
  like progress and streaming output.
- A **Cell** is a typed executable unit in a declarative graph. Cells can be
  composed into workflows with inputs, outputs, budgets, and conditions.
- A **Run ledger** records lifecycle outcomes for a workflow: starts,
  transitions, gates, artifacts, commits, cancellations, and timeouts.
- A **gate** independently decides whether a proposed action is allowed or
  safe enough to proceed.
- An **extension** adds prompts, tools, triggers, or code under a manifest and
  capability model.

The workspace is not one monolithic executable. `roko-core` provides shared
types and traits; `roko-runtime` contains workflow/effect/state machinery;
`roko-agent` contains model/provider/harness dispatch; `roko-graph` contains
the optional graph layer; `roko-chain` contains chain client/wallet/gate ideas;
and applications such as `agent-relay` and `roko-chain-watcher` demonstrate
connectivity and watcher concerns. The v2 docs additionally describe more
speculative systems including cognition, groups, reputation, marketplace, and
payments.

Roko therefore offers two useful things to Polkagent:

- **implemented structural examples** of good seams in a Rust codebase; and
- **a rejection list** of abstractions that sound powerful but would make a
  security-sensitive Polkadot product harder to explain, test, and operate.

## Research scope and source map

The review covered Roko’s v2 specification, its indexes, workspace/crates,
applications, examples, deployment material, selected temporary design work,
and the concrete files named below. The repository mixes code and proposals;
the distinction above applies throughout.

| Area | Roko specification | Concrete implementation / operational source | Why Polkagent should care |
| --- | --- | --- | --- |
| Kernel data and traits | `docs/v2/{00-INDEX.md,01-SIGNAL.md,02-CELL.md}` | `crates/roko-core/src/{signal.rs,pulse.rs,engram.rs,traits.rs,context.rs,loop_tick.rs}` | Establishes a useful durable-artifact/live-event split and small dependency ports. |
| Graph and execution | `docs/v2/{03-GRAPH.md,04-EXECUTION.md}` | `crates/roko-graph/src/{cell.rs,engine.rs,registry.rs,loader.rs,hot.rs}`; `crates/roko-runtime/src/{workflow_engine.rs,run_ledger.rs,effect_driver.rs,pipeline_state.rs}` | Separates workflow decisions from external activity and records lifecycle outcomes. |
| Agent/provider/gateway | `docs/v2/{05-AGENT.md,08-GATEWAY.md}` | `crates/roko-agent/src/{agent.rs,provider/,translate/,tool_loop/,dispatcher/,harness/}` | Demonstrates why model protocol, harness process lifecycle, tool execution, and request routing must not collapse into one adapter. |
| Tools/extensions/connectivity | `docs/v2/{12-EXTENSIONS.md,14-TOOLS.md,11-CONNECTIVITY.md}` | `crates/roko-core/src/{extension.rs,connector.rs,tool/}`; `crates/roko-plugin/src/manifest.rs`; `crates/roko-mcp-*`; `apps/agent-relay/src/{protocol.rs,bus.rs,state.rs}` | Supports a future capability-scoped extension and external-protocol model. |
| Memory/learning | `docs/v2/{06-MEMORY.md,07-LEARNING.md,26-CROSS-CUTS.md}` | `crates/roko-{learn,neuro,dreams,daimon}/src/` | Helps distinguish bounded evidence promotion from unsafe autonomous self-modification. |
| Groups/feeds/triggers | `docs/v2/{09-FEEDS.md,10-GROUPS.md,13-TRIGGERS.md}` | `crates/roko-core/src/{feed.rs,job.rs,namespace.rs}`; `apps/agent-relay/src/` | Gives useful ingress, subscription, and future isolation patterns. |
| Security/auth/config | `docs/v2/{16-SECURITY.md,17-AUTH.md,19-CONFIG.md}` | `crates/roko-agent/src/safety/`; `crates/roko-core/src/{secrets/,config/,immune.rs,obs/}`; `crates/roko-agent-server/src/auth/` | Provides the strongest reusable pattern: independently resolved permissions and classified information flow. |
| Chains/payments/registries | `docs/v2/{18-PAYMENTS.md,21-MARKETPLACE.md,22-REGISTRIES.md,24-DEFI.md}` | `crates/roko-chain/src/{client.rs,wallet.rs,gate/,witness.rs,block_watcher.rs,mock.rs}` | Suggests a safe separation between read/simulate, signing, broadcast, and proof anchoring. |
| Evals/ops/deployment | `docs/v2/{15-TELEMETRY.md,23-ARENAS.md,25-DEPLOYMENT.md,27-ORCHESTRATOR.md}` | `crates/roko-{runtime,conductor,serve,orchestrator}/src/`; `deploy/`, `docker/`, `apps/roko-chain-watcher/` | Informs projections, contract tests, configuration, local-first service operation, and later deployment. |

## Proposed Polkagent vocabulary and baseline model

Using one vocabulary consistently will make future PRDs and crates easier to
read. The following is a **Polkagent proposal**, informed by Roko but not
borrowed as a requirement to reproduce Roko names.

| Term | Definition | Example |
| --- | --- | --- |
| **Run** | A durable unit of work started by one ingress event. A chat turn can create one run; a chain action may span multiple resumed attempts. | “Explain this transfer” run. |
| **Turn actor** | The sole logical writer for a run or conversation key. It serializes decisions, not all network I/O. | Only one reducer changes pending approval state. |
| **Artifact** | Immutable retained evidence or output with type, provenance, digest, classification, and parent references. | Metadata snapshot, decoded call, simulation report, signed receipt. |
| **Run event** | Ordered lifecycle record for recovery/audit. It is not necessarily a user-visible live stream. | `ApprovalRequested`, `BroadcastConfirmed`. |
| **EffectIntent** | Durable instruction for a worker to perform a specific external action. It is created before I/O. | Request a signer to sign canonical bytes. |
| **EffectAttempt** | One claim, lease, retry number, idempotency key, timing, and worker lifecycle for an intent. | Second RPC submission attempt after the first timed out. |
| **EffectOutcome** | Immutable success, failure, timeout, cancellation, or unknown result observed for exactly one attempt. | Wallet refused; RPC submission returned transaction hash. |
| **Grant** | A concrete, time-bounded authorization for an exact operation. | Extension v1 may read host A but cannot write it. |
| **Gate** | Independent deterministic policy/verifier that allows, denies, or requests escalation. | Metadata mismatch denies a stale signing plan. |
| **Projection** | Recoverable read model derived from durable records for UI/API/operations. | Approval sheet state and finality progress. |
| **Harness** | A long-lived external service/process that may run its own protocol/session lifecycle. | Existing chat-agent backend or coding agent daemon. |

An illustrative minimal type model follows. Field names are examples; the
important property is the ownership and immutability boundary.

```rust
struct Artifact {
    id: ArtifactId,
    kind: ArtifactKind,             // e.g. RuntimeMetadata, SimulationReport
    digest: Sha256Digest,
    parents: Vec<ArtifactId>,       // evidence lineage
    provenance: Provenance,         // source, time, software/version
    classification: Classification, // Public | Private | Sensitive | SecretForbidden
    body_ref: BlobRef,              // bytes live in controlled storage
}

enum EffectIntent {
    ExecuteModel { route: ModelRoute, context: ContextPackRef },
    InvokeTool { tool: PinnedTool, grant: ResolvedGrant },
    RequestSignature { payload: CanonicalPayloadRef, approval: ApprovalId },
    Broadcast { signed_payload: ArtifactId },
    ObserveFinality { transaction: TransactionId },
}

struct ResolvedGrant {
    subject: SubjectId,
    resource: ResourceSelector,
    allowed_effects: EffectSet,
    limits: Limits,                 // requests, bytes, spend, deadline
    approvals: Vec<ApprovalId>,
    expires_at: Timestamp,
    policy_revision: Digest,
}
```

`SecretForbidden` means an artifact must not contain wallet seed phrases,
unredacted API tokens, or other material which would make ordinary retention,
projection, or model context unsafe. Secret references belong in a dedicated
secret store; logs and artifacts receive handles or redacted facts instead.

## Pattern 1 — durable artifacts and ephemeral live events

**Roko evidence and intent.** `docs/v2/00-INDEX.md` defines the conceptual
Signal/Pulse and Store/Bus split. The kernel source at
`crates/roko-core/src/{signal.rs,pulse.rs,provenance.rs,attestation.rs}` gives
the design a concrete home, while `crates/roko-graph/src/cells/graduation.rs`
describes a “graduation” boundary. The intended rule is that durable outputs
are typed, content-addressable, and lineage-bearing, while immediate activity
is carried as a sequence-oriented pulse. The boundary prevents every token,
counter, or transient progress notification becoming permanent audit data.

**Why it matters.** A Polkadot agent must be able to later answer: Which
runtime metadata was used? What did the signer see? Did simulation precede the
signature? A live token stream cannot answer that question after a crash. At
the same time, retaining every streaming chunk wastes storage and can retain
unnecessary private text.

**Polkagent proposal.** Retain material evidence and recovery events, then
publish lossy/coalescible UI activity separately. Every chain intent, metadata
snapshot, decoded call, simulation, approval, signature outcome, broadcast
receipt, and finality observation becomes an `Artifact` with parent links.
Use ordinary SQLite primary keys internally and compute cryptographic digests
at evidence boundaries; do not make every database row globally
content-addressed.

```text
user request (artifact) -> action intent (artifact) -> metadata snapshot
  -> decoded call -> simulation -> approval -> signed payload
  -> broadcast receipt -> finality observation
```

Each arrow means “this later fact names the relevant earlier fact,” not merely
“these records happened near one another.” This lineage makes a user-visible
explanation and an operator audit derive from the same evidence.

**Risk and trade-off.** Immutable artifacts cost storage and require retention,
classification, encryption, and deletion policy. Storing too little creates an
unexplainable system; storing raw prompts, secrets, or every token creates a
privacy liability. The v1 default should retain canonical evidence and bounded
redacted diagnostics, with explicit retention configuration.

**Implementation implication.** The durable schema needs an artifact table,
body/blob reference, parent-edge table, classification, source/version/time,
and integrity digest. Run events need a per-run monotonic sequence and a
durability flag. SSE (Server-Sent Events) or WebSocket progress is a
projection/feed and must not be the only record of state.

## Pattern 2 — pure decisions, effect drivers, and a run ledger

**Roko evidence and intent.** `docs/v2/03-GRAPH.md` §§6/10 and
`docs/v2/04-EXECUTION.md` §§8/16 describe a workflow/activity split.
`crates/roko-runtime/src/run_ledger.rs` explicitly records phase, agent, gate,
artifact, commit, timeout, and cancellation outcomes. Its comments state the
run ledger must not reconstruct canonical state merely by replaying the event
bus. Related drivers live in `effect_driver.rs`, `workflow_engine.rs`, and
`pipeline_state.rs`.

**What this means in plain language.** Code that decides “a simulation is now
required” should be deterministic and replayable. Code that actually contacts
an RPC endpoint is fallible I/O, may run twice at the transport level, and must
return an explicit result. A durable ledger/outbox is the handoff between the
two. This prevents the common failure mode: a process crashes after sending a
transaction but before recording that it sent it, then blindly sends it again.

**Polkagent proposal.** The turn actor runs a `TurnReducer`:

```rust
fn reduce(state: TurnState, input: TurnInput) -> (TurnState, Vec<EffectIntent>);
```

It commits the new state and its requested `EffectIntent`s in one SQLite
transaction. Workers claim effects, attach a stable idempotency key, perform
the I/O, and store one `EffectOutcome`. The actor consumes that outcome to
decide the next state. The canonical request digest, effect identifier, and
attempt identifier together make ambiguous retries visible and manageable.

For a chain action, use a **saga**: a durable, compensatable sequence in which
each step has its own terminal status. It is not one giant `execute_transaction`
tool call.

```text
IntentCreated
  -> MetadataEvidenceCaptured
  -> CallDecoded
  -> SimulationCompleted
  -> ApprovalRequested / ApprovalGranted
  -> SignatureRequested / SignatureObtained
  -> BroadcastAttempted / BroadcastConfirmed
  -> FinalityObserved (or timeout/reorg/review required)
```

Simulation success is not broadcast success; broadcast success is not
finality. The model cannot skip transitions by emitting persuasive prose.

**Risk and trade-off.** This increases schema and state-machine work before a
chat prototype looks impressive. It pays off exactly where Polkagent has the
most asymmetric risk: duplicated external actions, failures during wallet
handoff, RPC disagreement, and restarts. The correct simplification is a
small explicit state machine, not removing durability.

**Implementation implication.** PRDs must specify effect claim leases,
idempotency behavior, retry classes, cancellation semantics, terminal states,
and recovery after a worker dies. They must distinguish “execution failed”
from “delivery to a client failed”; a UI reconnect must never rerun an effect.

## Pattern 3 — optional cells and graphs, not a universal runtime

**Roko evidence and intent.** `docs/v2/03-GRAPH.md` and
`crates/roko-graph/src/cell.rs` describe a `Cell` with identity, version,
protocols, cost/duration information, and asynchronous execution. `engine.rs`,
`registry.rs`, `loader.rs`, `condition.rs`, and `budget.rs` support validation,
loading, conditions, and resource accounting. The v2 design extends this to
hot graphs and nested cognition; those ambitions exceed what Polkagent needs
to prove its first use case.

**Why it can help later.** A declarative graph is useful when the same
multi-stage workflow appears repeatedly and needs inspection, versioning, or
controlled fan-out/fan-in. For example, a governance researcher might retrieve
referendum data, decode runtime facts, obtain independent sources, and render
a comparison before asking for a human decision.

**Polkagent proposal.** Keep the v1 reducer and effects as the execution
kernel. Introduce `WorkflowSpec` only after two or more real workflows need
the same declarative representation. Each future node must declare input and
output artifact kinds, required grants, idempotency semantics, budget/deadline,
failure transition, and version. Compile such a graph to the same effect/outbox
mechanism rather than creating a second executor.

**Rejected/deferred.** Do not make “everything is a Cell” a v1 abstraction;
do not add resident cognitive loops, hot runtime graph editing, or generic
nested reasoning graphs. They obscure simple action flow, make recovery harder
to prove, and invite a configuration language before there are stable
workflows to configure.

## Pattern 4 — provider, harness, tool, transport, and chain ports differ

**Roko evidence and intent.** `crates/roko-agent/src/agent.rs` exposes an
`AgentResult` with output, ordered trace, usage, and success data. Provider
protocol translation and streaming live under `provider/`, `translate/`,
`openai_compat_backend.rs`, and `streaming.rs`; dispatch/tool loop code is in
`dispatcher/` and `tool_loop/`. In contrast, `harness/{service.rs,registry.rs,
child_process_runner.rs,events.rs}` treats a harness as a long-lived process:
it has idempotent start/stop, status, healthcheck, endpoint, and process ID.
`HarnessRegistry` owns lifecycle rather than request routing.

**Definitions.** A **model provider** speaks an inference API. A **harness**
is an external agent service with its own process or session lifetime. A
**tool** performs a bounded operation requested by a run. **Transport**
receives/sends messages and acknowledgements. A chain **client** reads or
simulates; a **signer** can authorize bytes; a **broadcaster** submits signed
bytes. These can be implemented by the same vendor, but should not share one
authority interface.

**Polkagent proposal.** Use narrow ports, with raw vendor frames retained only
as classified diagnostic artifacts and the core accepting a small normalized
event vocabulary:

```rust
trait TurnExecutor { fn start(&self, req: TurnRequest, sink: TurnEventSink) -> ExecutionHandle; }
trait HarnessService { fn start(&self) -> Result<Endpoint>; fn stop(&self); fn status(&self) -> Status; fn health(&self) -> Health; }
trait ToolHost { fn invoke(&self, intent: ToolIntent, grant: &ResolvedGrant, ctx: &RequestContext) -> ToolResult; }
trait Transport { fn receive(&self) -> Incoming; fn ack(&self, id: DeliveryId); fn send(&self, out: Outgoing); }
```

Vendor-compatible model APIs should mostly be configuration profiles; genuine
protocol differences belong in leaf crates. Harness process/session metadata is
attempt-owned diagnostic state, not durable business state.

**Risk and trade-off.** More traits can become ceremony. Avoid a giant
universal adapter trait and avoid public plugin interfaces before the internal
boundaries prove stable. The test for keeping a port separate is whether it
has different failure, custody, retry, lifecycle, or authorization semantics.

## Pattern 5 — an ordered gateway pipeline, not magical middleware

**Roko evidence and intent.** `docs/v2/08-GATEWAY.md` and
`crates/roko-agent/src/dispatcher/{hook_chain.rs,timeout.rs,dedup_cache.rs,
result_cache.rs,tool_selector.rs,validate.rs}` show ordered request processing.
`tool_loop/{checkpoint.rs,compaction.rs,max_iter.rs,prune.rs}` supplies
checkpointing, compaction, iteration limits, and pruning. Provider health is
addressed in `crates/roko-learn/src/provider_health.rs`.

**Polkagent proposal.** A run has a fixed, named pipeline:

```text
admission -> grant/policy resolution -> context assembly -> executor/harness
-> mediated tools -> evidence + verifiers -> outbox/egress -> accounting
```

Every stage has bounded input/output, emits an event, and can short-circuit
with a legible denial or failure. Admission enforces payload/rate limits;
policy resolution produces immutable grants; context assembly is pure; tool
calls are mediated; verifiers create evidence; egress sends only an already
recorded result. Add per-provider concurrency, queue depth, byte/token caps,
deadlines, and typed retry classes.

**Important constraint.** Do not automatically switch models after a tool,
signature, or chain effect has begun. A fallback at that point can produce a
different interpretation of partially completed work. Any reroute creates a
new attempt, preserves route evidence, and resumes only through explicit state
transitions.

**Trade-off.** A fixed pipeline is less superficially extensible than a
generic ordered hook chain. It is safer for v1 because every decision point is
known, reviewable, and observable. Extension points should be added only where
the product proves a stable need.

## Pattern 6 — pure, attributable prompt and context composition

**Roko evidence and intent.** `crates/roko-compose/src/` and its README show a
`PromptComposer` that consumes supplied sections, has no file I/O, obeys token
budgets, and records omitted-section bookkeeping. `docs/v2/06-MEMORY.md` and
`crates/roko-neuro/src/{context.rs,knowledge_store.rs,distiller.rs}` provide
the wider memory context.

**Why it matters.** Context is an authority surface. Unattributed web text,
old chain facts, or an unreviewed memory can manipulate a model’s answer and
be impossible to explain afterwards. Pure assembly also makes a bad answer
reproducible: one can inspect exactly which evidence was offered to the model.

**Polkagent proposal.** A `ContextSource` returns attributed candidates and a
pure `ContextAssembler` selects them under explicit classification and budget
rules:

```rust
struct ContextItem { artifact_ref: ArtifactId, reason: String, score: f32, token_estimate: u32, classification: Classification }
struct ContextPack { included: Vec<ContextItem>, excluded: Vec<Exclusion>, digest: Digest, budget: TokenBudget }
```

The assembled pack names included and excluded content, the rule that selected
it, its digest, and the token budget. It never silently turns user text,
unverified RPC output, or raw vendor diagnostics into durable “memory.”

**Trade-off.** Attribution has a token/storage cost and may make early chats
feel less free-form. For Polkagent’s first action-review experience, that cost
is worth paying: citations and exclusions are part of the trust model.

## Pattern 7 — independent layered gates and chain custody boundaries

**Roko evidence and intent.** `docs/v2/16-SECURITY.md`, `crates/roko-gate/src/`,
and `crates/roko-chain/src/gate/{tx_sim_gate.rs,wallet_gate.rs,mev_gate.rs}`
describe independently evaluable safety checks. The concrete `TxSimGate`
consumes a planned transaction, calls a simulator trait, rejects simulation
error/revert, and applies a gas-buffer policy. `client.rs` and `wallet.rs`
separate a chain reader/client from a wallet authority. Agent safety modules
are under `crates/roko-agent/src/safety/`.

**Polkagent proposal.** No LLM, prompt, extension, provider, or harness may be
the only verifier for a consequential action. Required gates are layered:

| Phase | Required independent checks |
| --- | --- |
| Before execution | sender/attachment validation, ingress limits, grant and budget checks |
| Before signing | pinned metadata and network identity, call decode, simulation where supported, deterministic policy, and either explicit approval or a valid configured autonomous mandate |
| Before egress | destination and classification rules, approved transport scope |
| After submission | receipt integrity, finality/reorg observation, clear pending/unknown state |

Polkagent must split these ports: `ChainClient` reads and simulates;
`Signer` signs only canonical payload bytes tied to a `ResolvedGrant`, policy
revision, and `AuthorizationDecision`; `Broadcaster` submits signed bytes;
`FinalityObserver` observes inclusion/finality. The authorization decision may
reference a human/quorum approval or an unexpired configured autonomous
mandate. A signer request is an effect with explicit authorization state, not a
generic tool call.

**Example.** If the runtime upgrades after the agent creates an approval card,
the metadata hash gate denies signing the old payload. Polkagent records the
drift and regenerates fresh evidence if permitted. It asks for review again
when policy requires approval; an autonomous mandate may authorize the fresh
payload only if its evidence, metadata-drift, and action-family rules
explicitly allow it. Polkagent never silently reuses a stale authorization.

**Rejected.** Do not use LLM-as-sole-verifier, mutable agent-owned safety
configuration, or a weighted aggregate “safety score” that can allow a hard
denial to be compensated by other points. A denial on network identity,
metadata, grant, spend limit, or required authorization remains a denial.

## Pattern 8 — capability intersection and classified information flow

**Roko evidence and intent.** `docs/v2/16-SECURITY.md` §§2–7 and
`docs/v2/12-EXTENSIONS.md` §§2/7 describe effective permissions as the
intersection of declared capability, workflow/graph allow-list, and Space
grant. Support code lives in `crates/roko-core/src/{extension.rs,immune.rs,
secrets/,obs/scrub.rs}` and `crates/roko-agent/src/safety/{capabilities.rs,
authz.rs,network.rs,path.rs,spending.rs,provenance.rs}`. The full Roko CaMeL
classification scheme is richer than Polkagent v1 requires.

**Why it matters.** A plugin manifest saying it “needs network access” is not
authorization to call every host. A model deciding that a secret would help is
not authorization to reveal it. Intersection means permission only exists if
every governing layer grants it; missing permission is deny by default.

**Polkagent proposal.** Materialize an immutable `ResolvedGrant` during
planning, covering exact tool ID/version, allowed filesystem roots, network
hosts, read/write/economic effect categories, request/byte/spend caps,
approval identifiers, expiry, and policy/config revision digest. Classify data
as `Public`, `Private`, `Sensitive`, or `SecretForbidden`. Tool/harness output
inherits the most restrictive input classification unless a named, audited
declassification policy permits a narrower result.

```text
effective permission = platform policy
                     ∩ workspace policy
                     ∩ workflow/action policy
                     ∩ requested extension capability
                     ∩ current approval and budget
```

This prevents a broad configuration flag from silently overriding a pending
approval. It also makes authorization decisions inspectable in the timeline.

**Trade-off.** Grant resolution needs a clear resource grammar and makes early
integration work slower. V1 should start with simple explicit selectors
(specific RPC host, known signer, fixed workspace directory) rather than an
ambitious policy language.

## Pattern 9 — extensions and marketplace: trust tiers before hook systems

**Roko evidence and intent.** `docs/v2/12-EXTENSIONS.md`,
`crates/roko-core/src/extension.rs`, and `crates/roko-plugin/src/manifest.rs`
describe extension layers, hooks, a manifest with metadata, prompt templates,
tool profiles, declarative tools, triggers, and dependencies. `docs/v2/21-
MARKETPLACE.md` and `docs/v2/25-DEPLOYMENT.md` §4 discuss a practical trust
ladder: prompts/configuration, declarative tools, WebAssembly (WASM), then
native code.

**Polkagent proposal.** V1 has built-ins and process/RPC (remote procedure
call) extensions only. Discovery does not activate anything. An extension
manifest must contain a stable ID, version, host-protocol version, config
schema, requested capabilities, entrypoint, resource limits, integrity data,
and declared outputs. Config pins versions. WASM becomes an option only after
fuel, memory, time, host-call, and filesystem/network constraints are tested.

**Why this supports future marketplace work.** A marketplace is not merely a
catalogue UI; it is a supply-chain and authority product. Version pinning,
manifest validation, capability display, reproducible execution, billing
boundaries, review/withdrawal, and support ownership must exist first.

**Rejected/deferred.** No generic collection of 22 hooks, marketplace
economics, on-chain reputation, third-party native code loading, self-updating
plugins, paid feeds, or automated installation in v1. These are product
programs, not incidental technical features.

## Pattern 10 — feeds, triggers, relays, and chain watchers

**Roko evidence and intent.** `docs/v2/{09-FEEDS.md,11-CONNECTIVITY.md,
13-TRIGGERS.md}` describes feeds, reconnection snapshots, gap detection,
ring-buffer recovery, and trigger source types (cron, webhook, file, bus,
chain, manual, pattern). `apps/agent-relay/src/{protocol.rs,bus.rs,state.rs}`
demonstrates agent hello/card, request correlation, subscriptions, topic
publication, sequenced envelopes, and feed registration. Runtime event buses
are in `crates/roko-runtime/src/{event_bus.rs,pulse_bus.rs,heartbeat.rs}`;
chain watcher code in `apps/roko-chain-watcher/src/{watcher.rs,
block_observer.rs,reactions.rs}`.

**Polkagent proposal.** Define an `Ingress` port and a durable
`TriggerBinding` policy for chat, webhook, scheduled, and chain-event inputs.
An adapter persists acceptance before acknowledging delivery, serializes work
per conversation/account as appropriate, and stores the source cursor. Live
subscriptions improve UX but are best effort; durable records recover state.
A `ChainEventWatch` emits observations only. It may launch a policy-gated
**prepare-only** workflow, but never directly signs or broadcasts a write.

**Example.** A governance watcher sees a new referendum. It stores a time- and
network-attributed observation, begins a research run, and produces a draft
summary with evidence. It cannot vote, change delegation, or move funds.

**Trade-off.** Exactly-once delivery across arbitrary webhooks and chain nodes
is unrealistic. Design for at-least-once ingress plus idempotent durable
processing, explicit cursors, and deduplication—not a false exactly-once
claim.

## Pattern 11 — telemetry and UI are versioned read-only projections

**Roko evidence and intent.** `docs/v2/15-TELEMETRY.md`,
`crates/roko-runtime/src/{state_hub.rs,projection.rs,metrics.rs}`, and
`crates/roko-core/src/obs/` show a StateHub/Lens style which keeps operators
and UIs away from volatile runtime objects. `crates/roko-serve/README.md`
describes a projection contract containing version, cursor, computed time,
recovery, and freshness state.

**Polkagent proposal.** Derive and version `TurnProjection`,
`DeliveryProjection`, `PolicyProjection`, `ProviderProjection`,
`ChainActionProjection`, and `ApprovalProjection` from SQLite state and
artifacts. Serve deltas over SSE/WebSocket if useful, but every client can
recover from cursor/version snapshots. Include trace and correlation IDs but
never unredacted secrets or raw private model context by default.

**Why it matters to UX.** The trusted approval sheet must be a projection of
canonical decoded payload, pinned metadata, simulation evidence, policy result,
and approval state—not a model-authored natural-language summary. A model may
explain the card; it must not be its source of truth.

## Pattern 12 — memory and learning only through evidence promotion

**Roko evidence and intent.** `docs/v2/{06-MEMORY.md,07-LEARNING.md,
26-CROSS-CUTS.md}` and the READMEs for `roko-learn`, `roko-neuro`,
`roko-dreams`, and `roko-daimon` cover episode logs, playbooks, provider
health, cost/quality views, confidence staging, and tier progression. They
also contain speculative concepts such as hyperdimensional computing (HDC),
demurrage, affect, dreams, and emergent goals.

**Polkagent proposal.** After a completed run, append a redacted `Episode`.
A separate, opt-in job can propose `CandidateKnowledge` or `PlaybookRule`; it
must name source artifacts, confidence, expiry, review owner, classification,
and an approval/policy decision before it becomes retrievable. Use measured
provider success, latency, and cost as operations data before attempting
machine-learned routing.

For Polkadot facts, retrieval must carry network, runtime/metadata version,
block/time provenance, and expiry. “How this referendum works” without those
qualifiers is dangerous when networks and metadata change.

**Rejected/deferred.** Affect/vitality/mortality models, automatic
self-modification, semantic demurrage as default retention, dream/imagination
loops, collective intelligence score optimization, and autonomous goal
formation that is not grounded in an owner-configured mandate do not belong in
the product baseline. Fully autonomous pursuit of configured goals remains
supported; silently inventing new authority does not.

## Pattern 13 — workspaces, evaluation, witnesses; phase markets and groups

**Roko evidence and intent.** `docs/v2/{10-GROUPS.md,18-PAYMENTS.md,
21-MARKETPLACE.md,22-REGISTRIES.md,23-ARENAS.md}` and
`crates/roko-chain/src/{marketplace.rs,agent_registry.rs,reputation_registry.rs,
trace_rank.rs,x402.rs,witness.rs}` cover groups, payments, registries, and
evaluation arenas. The most reusable idea is **partitioned ownership**: a
group/space owns its membership, data partition, event partition, and grants.
Arenas offer attempt/evaluation/ground-truth concepts. Witness primitives show
how one might anchor an evidence digest without publishing the evidence.

**Polkagent proposal.** A `Workspace` is an isolation partition before it is a
multi-agent social space. It must separately scope data, secrets, grants,
models, extensions, budgets, and audit visibility. Add deterministic evaluator
fixtures for specific domain workflows. Later, an opt-in chain witness can
anchor the hash of a finalized run or approval while sensitive artifacts remain
off-chain.

Permissionless registries/marketplaces, reputation inputs, streaming or
metered payments, paid feeds, service/job listings, and autonomous groups are
established long-term Polkagent product areas. They should not reuse Roko’s
speculative economy types as if those types solved governance, abuse response,
consumer protection, accounting, legal, or product-market concerns. Model
their minimal stable domain contracts in the definitive PRDs, then implement
them in phases after registry trust, payment reconciliation, group authority,
and incident operations pass their own gates.

## Pattern 14 — configuration, operations, and deployment

**Roko evidence and intent.** `docs/v2/{19-CONFIG.md,25-DEPLOYMENT.md,
27-ORCHESTRATOR.md}`, `crates/roko-core/src/{config/,secrets/}`, and
`crates/roko-runtime/src/{process.rs,lifecycle.rs,resource.rs,cancel.rs}`
describe strict configuration, secrets, process supervision, health checks,
resource caps, and cancellation. `crates/roko-orchestrator/README.md` describes
a single owner loop plus asynchronous I/O; `docker/README.md`, `deploy/README.md`,
and `apps/roko-chain-watcher/README.md` cover operational packaging.

**Polkagent proposal.** Configuration precedence is defaults < file <
environment < command-line interface (CLI). Reject unknown keys. Store an
effective config snapshot/digest for each run. Put only secret handles in run
records. Hot reload only changes settings safe for future turns; it cannot
retroactively change an in-flight approval, resolved grant, or canonical chain
payload. Ship local binary plus SQLite first; add Docker/systemd/launchd next;
use that bootstrap to validate the shared portable data-plane contract.
Managed workers, remote harness services, and the control-plane contract are
designed from the outset and delivered on independent evidence-gated tracks;
this sequencing does not make cloud a secondary product mode.

**Trade-off.** Local-first creates fewer moving parts and clearer custody, but
requires thoughtful backups, upgrade/migration, and device support. A cloud
control plane is an equally first-class Polkagent deployment product while
remaining optional for any particular user. It must share portable contracts
with the local data plane and must not silently become the authority for local
wallets or durable user evidence.

## Explicitly rejected or deferred Roko patterns

The following are not judgments that the ideas are universally bad. They are
outside the earliest kernel release, imported only in a different
Polkagent-native form, or rejected because Polkagent must earn trust with
legible, auditable, Polkadot-specific action preparation.

| Defer/reject | Why it is not a baseline requirement | Reconsider only when |
| --- | --- | --- |
| Universal Cells/Graphs and hot graphs | A generic execution language hides the small state machine users must trust. | Repeated production workflows need declarative inspection/versioning. |
| Resident cognitive loops/nested cognition | Creates unclear authority, cost, and stop conditions. | A bounded workflow demonstrably needs it and has evaluation gates. |
| HDC, demurrage, affect, dreams, vitality/mortality | Speculative cognitive/economic metaphors are not user safety features. | Independent research evidence and a product need justify them. |
| Self-authored authority expansion and unreviewed self-modification | An autonomous agent may pursue configured goals, but cannot safely grant itself new accounts, tools, policy, code, or spending authority. | Versioned learning/policy promotion and an independently authorized mandate make the change explicit. |
| Generic hook chains | Makes policy order and extension authority difficult to audit. | A concrete, stable extension seam proves necessary. |
| Native third-party plugins/self-updates | Supply-chain and host-compromise risk. | Sandboxing, signing, revocation, resource controls, and support process exist. |
| Importing Roko’s marketplace/reputation/token/payment economy wholesale | Polkagent’s permissionless marketplace and payments are real product areas, but they are governance, trust, accounting, and operations products rather than a crate feature. | The Polkagent-native manifest, registry, settlement, dispute, and commercial operating models are specified and gated. |
| Ungoverned autonomous multi-agent groups | Social coordination does not substitute for workspace isolation; fully autonomous groups still need explicit configured authority. | Product has solved membership, mandates, shared budgets, cancellation, incident response, and evaluations. |
| On-chain storage of evidence/knowledge | Public permanence conflicts with sensitive action context. | Only hash anchoring is needed and user consent/retention policies are explicit. |

## Adoption matrix

This matrix turns the research into sequencing guidance. “Adopt now” means
include as a requirement of the first implementable architecture, not that a
full production implementation must precede every prototype.

| Pattern | Decision | First implementation shape | Main acceptance evidence |
| --- | --- | --- | --- |
| Artifact/event split | Adopt now | SQLite artifact + ordered run-event tables; lossy live stream | A completed action can show linked metadata, simulation, approval, and finality artifacts after restart. |
| Single writer + reducer + outbox | Adopt now | Per-run/conversation actor, transactional effect intents, leased workers | Kill/restart tests never duplicate an external action. |
| Chain saga | Adopt now for write-capable flows | Separate decode/simulate/approve/sign/broadcast/finality states | Stale metadata, signer refusal, broadcast uncertainty, and reorg cases have legible terminal/review states. |
| Capability intersection | Adopt now | Immutable `ResolvedGrant`, deny-by-default resource selectors | An extension cannot exceed manifest, workspace, action, and approval limits. |
| Classified information flow | Adopt now | Four classes, inherited restriction, secret handles | Secret material cannot reach logs, model context, or projections through ordinary code paths. |
| Narrow adapter ports | Adopt now | Separate executor, harness, tool, transport, chain client, signer, broadcaster | Contract tests show a provider/harness cannot sign or bypass policy. |
| Pure context pack | Adopt now | Attributed inputs/exclusions and digest | A model response can be traced to exactly the context/evidence offered. |
| Fixed gateway pipeline | Adopt now | Named stages and typed short-circuits | Route/fallback/retry history is visible and no fallback repeats effects. |
| Versioned projections | Adopt now | Recoverable API/UI read models | UI reconnect recovers status without touching live worker memory. |
| Triggers/watchers | Adapt after core | Cursor-based ingress and prepare-only chain watches | Duplicate delivery/reconnect tests retain at-most-once logical action creation. |
| Workflow graph | Adapt later | Compile `WorkflowSpec` to existing outbox | Two real workflows share it without introducing a second executor. |
| Evidence promotion/memory | Adapt later | Redacted episodes and reviewable candidate knowledge | Retrieved knowledge shows provenance/version/expiry and can be disabled. |
| Extension SDK/WASM | Adapt later | Manifest-pinned process/RPC first, sandboxed WASM later | Capability/resource/revocation tests and supply-chain review pass. |
| Workspaces/evals/witness anchoring | Adapt later | Isolated workspace partitions, deterministic fixtures, optional hash witness | Cross-workspace isolation and consent tests pass. |
| Marketplace/payments/autonomy | Define contracts now; implement in phases | Permissionless package/registry, payment intent, mandate, signer and receipt types remain outside the minimal executor | Commercial, custody, governance, security and operational PRDs pass their gates. |

## PRD requirements derived from this research

Any Polkagent implementation PRD that introduces a model, tool, harness,
transport, chain action, watcher, extension, or UI surface should answer the
following explicitly. These are requirements, not suggestions to “use Roko.”

1. **Authority and custody:** Who may propose, approve, sign, broadcast,
   cancel, retry, and view the action? Which of those powers is impossible for
   the LLM, provider, harness, or extension to obtain?
2. **Durable state:** What is the run state machine? Which artifacts and events
   are retained, classified, hashed, and linked? What is deliberately ephemeral?
3. **Effect semantics:** What `EffectIntent` is stored before I/O? What is its
   idempotency key, lease, retry class, deadline, cancellation rule, and
   unknown-outcome behavior?
4. **Evidence and correctness:** For chain-facing output, what network,
   metadata/runtime version, block/time, decoder, simulation, and finality
   evidence is required? What invalidates earlier evidence?
5. **Gates and grants:** Which independent hard gates apply, where is the
   immutable `ResolvedGrant` derived, and which effects require fresh human or
   quorum approval versus a valid configured autonomous mandate?
6. **Information flow:** Which data classifications enter and leave every
   adapter? How are secrets redacted, held, and prevented from reaching models,
   logs, artifacts, or projections?
7. **Adapter boundary:** Is this a model provider, harness, tool, transport,
   chain client, signer, broadcaster, or watcher? Why does it need its own
   lifecycle/retry/authority contract?
8. **UX truth source:** Which projection drives the user-visible state? If it
   presents an approval, is the displayed payload/evidence canonical rather
   than model-authored? How does reconnect/recovery work?
9. **Operations:** What config is snapshotted per run? What can hot reload?
   What health, resource, audit, backup, migration, and incident behavior is
   required for local-first operation?
10. **Maturity and exclusion:** Is the capability implemented, gated research,
    or deferred? Which rejected/deferred pattern must it not smuggle into scope?

## Minimum source files for implementation provenance

These are the highest-value Roko files for a future engineer who wants to
verify a detail against source; the prior sections remain sufficient without
opening them.

- Kernel/ports: `crates/roko-core/src/{traits.rs,signal.rs,pulse.rs,context.rs,
  extension.rs,config/schema.rs,secrets/resolve.rs}`.
- Durable runtime: `crates/roko-runtime/src/{run_ledger.rs,effect_driver.rs,
  workflow_engine.rs,state_hub.rs,event_bus.rs,cancel.rs}`.
- Provider/harness resilience: `crates/roko-agent/src/{agent.rs,harness/service.rs,
  harness/registry.rs,dispatcher/hook_chain.rs,tool_loop/checkpoint.rs,
  safety/capabilities.rs}`.
- Graph only if needed: `crates/roko-graph/src/{cell.rs,engine.rs,registry.rs,
  loader.rs,budget.rs}`.
- Chain safety baseline: `crates/roko-chain/src/{client.rs,wallet.rs,
  gate/tx_sim_gate.rs,mock.rs,witness.rs,block_watcher.rs}`.
- Relay/operations: `apps/agent-relay/src/{protocol.rs,bus.rs,state.rs}` and
  `apps/roko-chain-watcher/src/{watcher.rs,block_observer.rs,reactions.rs}`.
