# Polkagent — established product and architecture baseline

**Status:** owner-confirmed planning baseline

**Audience:** readers with no prior Polkagent, Roko, or
`polkadot-chat-agents` context

**Purpose:** record what has been established before the definitive PRDs are
written

**Implementation status:** design direction only; this document does not claim
that the platform has been implemented or validated in production

## 1. How to read this document

This is the current source of truth for product intent. It reconciles the
iterative documents under `tmp`, the Roko and `polkadot-chat-agents` source
reviews, deep-research reports 17–19, current ecosystem research, and the
owner’s subsequent clarifications.

The following labels are used throughout:

| Label | Meaning |
|---|---|
| **Established** | An owner-confirmed product or architecture direction that the definitive PRDs must preserve. |
| **Default** | The recommended out-of-box behavior. A user or operator may select another supported mode. |
| **Configurable** | A deliberate choice exposed through a safe, understandable UX and a versioned configuration contract. |
| **Phased** | A committed part of the long-term platform, delivered after its dependencies and release gates. |
| **Experimental** | A valuable direction that requires research, an implementation spike, and explicit maturity labeling. |
| **Open** | Evidence or a lower-level implementation decision is still required. |

Older documents sometimes describe a narrow first MVP as if it were the whole
product. That language is superseded by the complete-platform direction below.
The narrow work remains useful as a validation slice and architectural proof.

### 1.1 Project starting point

Polkagent is a new repository and platform design. At the time of this
baseline, the repository is principally a researched product and architecture
corpus; the capabilities below are not claims about running production code.
The next major deliverables are definitive PRDs, accepted architecture decision
records, conformance fixtures, implementation spikes, and a Rust workspace.

Polkagent is informed by, but is not a fork of, two projects:

- **`polkadot-chat-agents` (PCA)** is an existing reference product for
  controlling local or remote coding/agent processes through
  Polkadot-oriented encrypted chat. Its important external behavior includes
  bot identities, durable message admission, mobile-friendly conversations,
  projects/files, live progress, direct coding-agent processes, framework
  bridges, configuration, and deployment. Polkagent must preserve or improve
  those user and compatibility contracts while replacing internal coupling.
- **Roko** is a separate, ambitious agent-runtime/platform project. It provides
  useful implemented and designed patterns around durable artifacts/events,
  effectful execution, narrow ports, policy grants, graphs, groups, memory,
  learning, feeds/triggers, extensions, evaluation, and operations. Polkagent
  adopts those patterns selectively and translates them into a smaller
  Polkadot-focused vocabulary.

Earlier deep research proposed an **Explain Before Sign** product: decode and
explain a proposed chain action before the user signs it. That remains an
excellent first proof of chain evidence, policy, approval, signing, finality,
and comprehension. It is one platform slice, not the whole product.

### 1.2 Polkadot context for a newcomer

Polkadot is an ecosystem for interoperable blockchains, shared security,
applications, assets, governance, and emerging general-purpose computation.
Polkagent needs to understand several related but distinct surfaces:

- **Polkadot SDK** is the Rust framework and library collection used to build
  compatible runtimes, nodes, chains, and system logic.
- **A runtime** defines a chain’s state-transition logic. A **pallet** is a
  modular runtime component.
- **Polkadot Hub and system-chain functionality** expose assets, applications,
  governance, identity, messaging, and other ecosystem capabilities. Exact
  names and production support must be verified against current primary
  sources before implementation.
- **XCM**, or Cross-Consensus Messaging, describes interactions across
  consensus systems. It involves locations, versions, fees, execution, and
  outcome uncertainty; it is not merely a generic token-send call.
- **Subxt** is a Rust library for reading state, submitting extrinsics, and
  following events against Polkadot SDK-based chains.
- **PAPI**, the Polkadot API, is a TypeScript-oriented client option useful in
  browser and application integrations.
- **PVM/PolkaVM**, the Polkadot Virtual Machine and associated tooling, is a
  deterministic execution environment with emerging product and contract
  uses.
- **JAM**, the Join-Accumulate Machine, is a developing computation protocol
  and service model. Polkagent treats it as a strategic, experimental product
  area until APIs, tooling, networks, and operational guarantees are verified.
- **Finality** is durable chain agreement that an included action will not be
  reverted under the protocol’s normal model. Signing, broadcasting, and block
  inclusion are not the same as finality.

Polkagent’s purpose is not to make these distinctions disappear. It should
translate them into understandable user choices while retaining precise typed
evidence internally.

**Evidence snapshot, 2026-07-29.** The ecosystem orientation above is grounded
in the official [Polkadot Hub reference](https://docs.polkadot.com/reference/polkadot-hub/),
[Polkadot SDK/parachain development guide](https://docs.polkadot.com/develop/parachains/),
[Subxt reference](https://docs.polkadot.com/reference/tools/subxt/),
[PAPI reference](https://docs.polkadot.com/reference/tools/papi/),
[XCM introduction](https://docs.polkadot.com/parachains/interoperability/get-started/),
[PVM design/status](https://docs.polkadot.com/polkadot-protocol/smart-contract-basics/polkavm-design/),
and [JAM overview](https://wiki.polkadot.com/learn/learn-jam-chain/).
These links establish public documentation, not production readiness for every
proposed Polkagent workflow. The PCA behavior review used repository commit
`2adddcc8cfd732804cd9bbcbcd26974b44b47f66`; the Roko pattern review used
commit `7f2d98fcc2e92e8d7238be80c0d8434008601c93`. Future implementation must
record the actual source/network revisions used by its fixtures.

### 1.3 Core vocabulary

| Term | Meaning |
|---|---|
| **Agent** | A versioned product/runtime definition that combines behavior, execution route, tools/skills, context/memory, surfaces, policy, and deployment requirements. It is not synonymous with one model. |
| **`AgentSpec`** | The portable, versioned declarative record that defines an agent’s behavior, routes, capabilities, memory, surfaces, policy references, and deployment requirements. |
| **`ProductSpec`** | A portable, versioned composition of one or more agents plus product-kit assets, workflows, UX, tests, policies, and deployment defaults for a user outcome. |
| **Model provider** | A service or connection that exposes one or more AI models. |
| **Executor** | The adapter that performs a particular run and emits normalized events. |
| **Harness** | A long-lived coding-agent or framework process/service with sessions, health, cancellation, and its own tool behavior. |
| **Transport** | A channel through which authenticated input, output, files, edits, and acknowledgements move, such as web, API, or Polkadot chat. |
| **Tool** | A typed operation an agent may invoke only through an effective grant. |
| **Skill** | A versioned bundle of instructions, context, schemas, examples, tests, and required capabilities. |
| **Product kit** | A reviewed starter for a user outcome that composes agents, skills, policies, UI assets, fixtures, and deployment defaults. |
| **Run** | One durable execution instance, potentially containing model, tool, approval, delivery, and chain effects. |
| **Artifact** | Durable, attributable content or evidence such as a file, plan, diff, decoded call, test result, simulation, or receipt. |
| **Event** | An ordered observation of lifecycle or streaming activity. |
| **Effect / `EffectIntent` / `EffectAttempt` / `EffectOutcome`** | An effect is actual external work such as invoking a model/tool, sending, signing, or submitting. `EffectIntent` is the durable command recorded before the I/O. `EffectAttempt` records one claim, lease, retry, and idempotency lifecycle. `EffectOutcome` is the immutable success, failure, timeout, cancellation, or unknown result observed for one attempt. |
| **Policy** | Versioned rules that decide what is permitted, denied, approval-gated, routed, or limited. |
| **Grant** | The exact resolved capabilities and limits available to a run/effect. |
| **Intent** | A typed proposal for a chain, payment, deployment, message, or other effect; it is not evidence that the effect occurred. |
| **Signer** | An isolated component or external wallet that signs an exact canonical payload under account policy. |
| **Data plane** | The runtime that owns conversations, runs, workspaces, effects, artifacts, and execution. |
| **Control plane** | Optional management services for tenancy, fleets, policy distribution, observability, billing, and deployment coordination. |
| **PCA** | `polkadot-chat-agents`, the reference chat-agent product. |
| **PVM** | Polkadot Virtual Machine/PolkaVM execution surface. |
| **JAM** | Join-Accumulate Machine, an emerging Polkadot-related protocol and computation architecture. |

## 2. Concise definition

> **Polkagent is a Rust-first, Polkadot-native product-engineering and agent
> platform for building, using, publishing, operating, and paying agents that
> understand Polkadot and can safely perform Polkadot work.**

It brings together three equally central product pillars:

1. **Build:** create and operate agents, applications, contracts, runtimes,
   chains/parachains, JAM services, integrations, and other Polkadot products.
2. **Act:** research, explain, prepare, simulate, approve, sign, submit, watch,
   and—when configured—autonomously perform payments and on-chain actions.
3. **Reach users:** provide the useful behavior and mobile/chat experience of
   `polkadot-chat-agents`, with a more modular runtime and the best surface for
   each job: Polkadot mobile/app integrations, web, desktop, CLI, API, or other
   transports.

The platform is not defined by one model vendor, one harness, one chat
transport, one custody model, or one deployment topology. Those are
replaceable, separately versioned integrations around a small durable kernel.

## 3. Product outcome

The long-term user journey is:

```text
idea or recurring job
  → choose a safe starter or describe the desired outcome
  → create/version an AgentSpec or ProductSpec (typed, versioned definitions
    of an agent or reusable product)
  → attach models, harnesses, tools, skills, data, chain profiles, and policy
  → test with fixtures, simulations, evals, and a preview environment
  → inspect permissions, custody, costs, and expected effects
  → publish locally, to a team, to managed cloud, or to a registry/marketplace
  → use it through chat, mobile, web, CLI, API, feeds, or triggers
  → approve actions or enable policy-driven autonomy
  → observe evidence, receipts, costs, finality, failures, and recovery
  → export, migrate, update, pause, revoke, or delete it
```

The simple path should feel like configuring a polished product, not assembling
an agent framework. Advanced users must still be able to replace every major
adapter and inspect every meaningful decision.

### 3.1 Who Polkagent is for

| Persona | Job to be done | Desired experience |
|---|---|---|
| Polkadot user | Understand, monitor, approve, and automate account or governance activity | Plain-language outcomes backed by canonical evidence; wallet and mobile handoff that does not expose keys |
| Application developer | Build a Polkadot-aware application or embedded agent | Product kits, SDKs, typed chain adapters, tests, deploy previews, and portable runtime APIs |
| Contract/PVM developer | Scaffold, build, test, deploy, monitor, and upgrade programs | Repository-aware coding harnesses, deterministic toolchains, simulation, artifacts, review, and policy-gated deployment |
| Runtime/chain engineer | Work on pallets, runtimes, chains/parachains, upgrades, and migrations | Deep Polkadot SDK context, local networks, benchmarks, migration checks, metadata diffs, and strong production gates |
| Product team | Turn a repeatable workflow into a polished agent product | Studio, shared projects, permissions, evals, publishing, deployment, Inbox, usage, and support |
| Organization/operator | Run private or public agents safely across users and environments | Tenancy, policy, fleets, custody options, observability, cost controls, audit, backup, and incident response |
| Extension publisher | Publish a skill, tool, model route, harness, feed, service, or product kit | Permissionless packaging and discovery, signatures, compatibility tests, pricing, reputation, updates, and revocation |
| Autonomous-agent owner | Fund and authorize an agent to act continuously | Understandable mandates, isolated signing, broad configurability, monitoring, receipts, pause/revoke, and recovery |
| End customer | Ask or pay a public/private agent for a useful result | Fast onboarding, clear price and permissions, trustworthy progress/results, receipts, privacy, and recourse |

The same person may occupy several roles. Authorization is still explicit: being
an agent creator does not automatically make someone a deployment operator,
account owner, payer, approver, or tenant administrator.

### 3.2 Three illustrative end-to-end experiences

#### Builder Copilot

1. A developer opens an existing Polkadot SDK repository in the local CLI or
   Studio.
2. Polkagent detects the workspace type, toolchain, network profiles, and
   available product kits; the user confirms a constrained workspace grant.
3. A selected coding harness analyzes the repository and emits a plan artifact.
4. Tool effects create a worktree, edit files, compile, test, benchmark, and
   capture logs as evidence.
5. The user reviews code, test evidence, migration/metadata changes, and a
   deployment preview.
6. Depending on policy, deployment is prepared for approval or performed
   autonomously in an allowed environment.
7. The entire run can be resumed through CLI, web, or mobile Inbox without
   losing project or effect state.

#### Mobile Polkadot assistant

1. A user messages a private agent through the best supported Polkadot
   mobile/app surface.
2. The transport authenticates the sender, durably admits the message, and
   acknowledges it only after recoverable state exists.
3. The agent reads permitted chain data and returns live progress.
4. For an action, the user receives a canonical approval sheet in a surface
   capable of showing complete evidence.
5. The selected wallet/signer confirms the exact payload.
6. Polkagent reports separately that it was signed, submitted, included, and
   finalized—or that the outcome is unknown.
7. Web remains available for richer evidence, configuration, and recovery.

#### Fully autonomous paid service

1. A publisher packages a monitored service as an agent/product kit and
   publishes it through one or more permissionless registries.
2. A customer or organization installs a pinned version, reviews permissions,
   selects providers and compute, configures payment terms, and chooses a
   signer/custody mode.
3. The owner grants the deployed agent continuous authority over specified
   accounts/action families. The mandate may be narrow or deliberately broad.
4. Feeds or chain events trigger runs. The agent quotes or charges, performs
   work, settles payments, and issues receipts without routine human approval.
5. Deterministic policy, isolated signing, idempotent effects, budgets if
   configured, monitoring, circuit breakers, revocation, reconciliation, and
   audit remain active.
6. The owner can export or move the agent between managed and self-hosted
   operation.

## 4. Established product principles

### 4.1 Excellent UX and progressive disclosure

**Established**

- Common jobs must work with sensible presets and minimal mandatory
  configuration.
- Complexity should appear only when a user needs it.
- Every powerful choice must have a plain-language explanation, consequence
  preview, and safe default.
- A beginner can start from a product kit; an expert can edit the underlying
  versioned specs, policies, routes, and manifests.
- Configuration should be possible through a polished UI, declarative files,
  CLI, and API without creating divergent semantics.
- Long-running work must expose progress, waiting states, requested approvals,
  costs, failures, and a definitive final outcome.
- Errors must describe what happened, what remains safe, and the next recovery
  action.

### 4.2 Polkadot depth before generic breadth

**Established**

- Polkagent is primarily for Polkadot, Polkadot SDK, Polkadot Hub and system
  functionality, JAM/PVM, and related Parity/Web3 Foundation product surfaces.
- Polkadot concepts are first-class domain types, not thin labels over a generic
  multichain wallet agent.
- External protocols may be integrated when useful, especially model,
  harness, tool, identity, payment, and interoperability standards.
- The public product model should not be generalized into a lowest-common-
  denominator multichain abstraction before Polkadot depth is proven.

### 4.3 Rust-first, not Rust-only

**Established**

- Rust is the primary language for the runtime, domain model, execution engine,
  policy, security, chain and payment logic, CLI, services, storage adapters,
  extension host, and native SDKs.
- TypeScript is appropriate for browser UI, Polkadot Product SDK/PAPI
  integration, generated clients, and ecosystems where it materially improves
  compatibility or user experience.
- Language boundaries use versioned protocols and schemas rather than shared
  internal state.

### 4.4 Local and cloud are equal product modes

**Established**

- Local/self-hosted and managed multi-tenant cloud are equally intentional
  platform modes.
- They must implement the same public domain contracts, specs, policy
  semantics, export format, and compatibility tests.
- State, identities, agent definitions, artifacts, policies, and audit history
  must be portable.
- Managed services may add fleet management, collaboration, hosted execution,
  billing, policy distribution, observability, backups, and support without
  making the local runtime a second-class client.
- No forced cloud dependency or silent control-plane access to private
  conversations, wallet material, or workspace contents.

### 4.5 Extensibility without a bloated kernel

**Established**

- Providers, models, executors, harnesses, tools, skills, transports, chain
  clients, signers, stores, memory systems, triggers, surfaces, and control-
  plane services are distinct extension axes.
- Stable traits and protocols should be narrow and capability-oriented.
- Optional integrations remain leaf dependencies; the core must not import
  provider, harness, transport, wallet, or chain-specific application logic.
- Extensions declare identity, version, compatibility, permissions, inputs,
  outputs, effects, costs, and health behavior.
- Built-in, process/RPC, WASM/component, and remote-service extensions may be
  supported at different trust and maturity tiers.
- Adding an integration should not require modifying unrelated core crates.

## 5. Product pillars in detail

### 5.1 Build Polkadot products

Polkagent should help individuals and teams:

- Explore and understand Polkadot SDK, runtime, pallet, XCM, contract, PVM,
  JAM, governance, wallet, indexer, and application code.
- Create project plans, architecture, code, tests, migrations, deployment
  artifacts, documentation, threat models, and operational runbooks.
- Run direct model APIs or coding harnesses such as Codex-, Claude Code-,
  OpenCode-, ACP-, A2A-, or custom-compatible executors.
- Maintain durable projects and workspaces with explicit roots, branches,
  sessions, files, artifacts, and tool grants.
- Compile, test, lint, simulate, benchmark, launch local networks, deploy to
  development environments, and prepare reviewed production changes.
- Inspect runtime metadata and upgrades, storage changes, contract behavior,
  chain state, governance activity, fees, and cross-chain effects.
- Turn repeated workflows into versioned skills, agents, product kits, feeds,
  monitors, and services.
- Use the same runtime for a local coding assistant, a team automation, a public
  service, or a policy-bounded autonomous operator.

Generated outputs are attributable artifacts. Agent prose is not treated as
proof that code compiled, a test passed, a deployment occurred, or chain state
changed.

### 5.2 Perform Polkadot actions and payments

Polkagent should support a complete action lifecycle:

```text
request or trigger
  → typed intent
  → chain/network/account resolution
  → canonical decode and metadata evidence
  → read-only evidence
  → fee/preflight/simulation evidence where available
  → policy evaluation
  → optional human, quorum, or external-system approval
  → exact-payload signing
  → submission/broadcast
  → inclusion/finality or explicit unknown state
  → reconciliation, receipt, and durable audit trail
```

Potential action families include:

- Native and asset payments.
- Recurring, metered, escrowed, conditional, and service payments.
- XCM and cross-chain asset operations.
- Contract and PVM calls.
- Identity, proxy, multisig, staking, nomination, delegation, and governance
  activity.
- Treasury and organizational workflows.
- Contract deployments and upgrades.
- Runtime, pallet, chain/parachain, and infrastructure operations.
- JAM/PVM service workflows as the ecosystem matures.

Each family requires its own decoder, evidence policy, simulation capabilities,
fixtures, risk model, and acceptance gates. A generic `send_transaction` tool
is not an adequate safety boundary.

### 5.3 Reach users with the best available experience

**Established surface rule**

Polkagent must provide at least the important functional and operational
capabilities of `polkadot-chat-agents`, while choosing or combining surfaces
according to the best user experience.

That means:

- Preserve private/encrypted Polkadot chat and mobile-oriented behavior where
  current Polkadot product APIs make it viable.
- Preserve durable message admission, ACK, retries, files/media, live progress,
  final replies, projects, sessions, model/harness selection, and operational
  recovery.
- Use one durable Inbox/conversation model across supported surfaces.
- Provide a rich web surface for configuration, timelines, artifacts,
  approvals, simulations, dashboards, marketplace discovery, and
  administration.
- Provide CLI and API access for builders, automation, and headless operation.
- Add a desktop shell or a Polkagent-owned native mobile client only where it
  improves the experience beyond Polkadot App/Product-hosted and responsive web
  surfaces.
- Do not make the entire product wait on one experimental mobile or messaging
  API. A user must have a high-quality fallback surface.

The definitive UX PRD must compare Polkadot App/Product integration, responsive
web/PWA, desktop shell, and a dedicated native client against real API support,
security, notification, signing, offline, and distribution requirements.

## 6. Definitive architecture direction

### 6.1 Small durable kernel

The kernel owns:

- Stable IDs and versioned specifications.
- Authoritative aggregate state transitions.
- Run, turn, event, artifact, effect, approval, and grant lifecycles.
- Deterministic policy evaluation.
- Idempotency, causality, sequencing, deadlines, cancellation, and recovery.
- Configuration and policy revision binding.
- Audit evidence and projection inputs.

The kernel does not own:

- Provider-specific HTTP clients.
- Harness-specific session formats.
- Transport-specific polling or UI.
- Wallet UI or raw signing keys.
- Polkadot product presentation logic.
- Marketplace ranking policy.
- A universal workflow language for every future use case.

### 6.1.1 Layered system view

```text
User and product surfaces
  Polkadot/mobile chat | Web/PWA | Studio | Inbox | CLI/TUI | API/SDK
                              │
Transport and presentation adapters
  authentication | messages | files | live updates | action sheets
                              │
Durable application/runtime services
  conversations | runs | orchestration | context | policy | projections
                              │
Minimal domain kernel
  stable IDs | state machines | artifacts | events | effects | grants
                              │
Execution and product ports
  models | harnesses | tools | memory | chain | signer | payment | registry
                              │
Infrastructure adapters
  SQLite/Postgres | object store | queues | RPCs | wallets | KMS | telemetry
```

Only downward-facing contracts connect the layers. A mobile transport cannot
write a “completed” turn directly; a model provider cannot submit a
transaction; a marketplace package cannot grant itself network or signer
access; and a control plane cannot silently reinterpret a local agent’s policy.

### 6.1.2 One request through the architecture

For a request that eventually transfers an asset:

1. `Transport` authenticates and durably admits a message.
2. The conversation service creates a `Turn` and `Run`.
3. The context assembler records which messages, artifacts, memories, and
   chain evidence enter the model context.
4. `TurnExecutor` emits a structured proposed action.
5. The application service validates it into a typed `ChainIntent`.
6. `ChainClient` effects decode metadata and collect read-only/preflight
   evidence.
7. `PolicyEvaluator` creates a `PolicyDecision` and `ResolvedGrant`.
8. The approval service either records an exact approval, finds an applicable
   autonomous mandate, or denies the action.
9. `Signer` receives only the canonical payload and binding data.
10. Submission and finality watchers run as separate idempotent effects.
11. Projections render the same truthful state in chat, web, CLI, audit, and
    operator views.

At no point is a model sentence such as “the transfer succeeded” authoritative.

### 6.2 Durable records

At minimum, the architecture must define:

| Record | Purpose |
|---|---|
| `AgentSpec` | Versioned definition of behavior, inputs, capabilities, routes, memory, surfaces, policy, and deployment requirements. |
| `ProductSpec` or product kit | Versioned packaging of agents, UI assets, skills, policies, tests, and deployment defaults for a user outcome. |
| `Conversation` / `Turn` / `Run` | Durable interaction and execution ownership with explicit lifecycle states. |
| `Artifact` | Immutable or versioned output/evidence with content digest, type, classification, provenance, and parent links. |
| `RunEvent` | Ordered activity or lifecycle observation with correlation, causation, sequence, and durability class. |
| `EffectIntent` | Durable request for external work such as model execution, tool use, delivery, signing, broadcast, or finality watch. |
| `EffectAttempt` | Claim, lease, retry number, idempotency key, timing, and worker lifecycle for one attempt; it does not double as the result. |
| `EffectOutcome` | Immutable observed result for exactly one attempt, including success, failure, timeout, cancellation, or unknown status plus typed result/error evidence and external references. |
| `ResolvedGrant` | Immutable, hashed snapshot of effective permissions and limits. |
| `PolicyDecision` | Allow, deny, require approval, or route decision with reasons and policy revisions. |
| `Approval` | Human, quorum, service, or pre-authorized approval bound to exact effect/action data. |
| `ChainIntent` / `PaymentIntent` | Typed, network-bound proposal with constraints and lifecycle. |
| `Identity` / `AccountBinding` | Local, pseudonymous, on-chain, organizational, or service identities and their scoped relationships. |
| `Projection` | Versioned read model for UI, CLI, APIs, operations, analytics, and export. |

### 6.3 Core invariants

**Established**

- External input is at-least-once; accepted semantic work is deduplicated.
- Authoritative state change and effect enqueue are atomic.
- Effects use stable idempotency keys and durable attempt records.
- A restart must not repeat an external side effect merely because its parent
  run resumed.
- Delivery completion and execution completion are separate states.
- Signing, submission, inclusion, finality, failure, and unknown are distinct
  outcomes.
- One logical writer owns an aggregate/partition at a time; unrelated work may
  execute concurrently within quotas.
- Normalized streams have explicit ordering and at most one terminal event.
- Policy and configuration revisions used by a run are recorded.
- Adapters cannot directly mutate authoritative core state.
- Evidence, inference, and model claims are distinguishable.
- Secret material is excluded from model context, logs, events, and ordinary
  extension APIs by default.

### 6.4 Stable ports

The definitive design should specify narrow ports for:

- `Transport`
- `TurnExecutor`
- `HarnessService`
- `ModelProvider`
- `ModelCatalog` and `Router`
- `ToolHost`
- `SkillResolver`
- `Workspace`
- `ArtifactStore`
- `EventStore`
- `EffectStore`
- `PolicyEvaluator`
- `ApprovalProvider`
- `ContextSource` and `ContextAssembler`
- `MemoryStore` and optional knowledge/graph projections
- `ChainClient`
- `Signer`
- `PaymentRail`
- `IdentityProvider`
- `Trigger` / `Feed`
- `ExtensionRegistry`
- `SecretResolver`
- `Clock`
- `EventSink` / telemetry exporter

Provider, model, executor, and harness are not synonyms. A long-lived coding
harness is not implemented as a model-name string.

### 6.5 Illustrative Rust boundary shapes

The final traits may differ, but their responsibilities should remain this
narrow:

```rust
#[async_trait]
pub trait TurnExecutor {
    async fn execute(
        &self,
        request: ExecuteRequest,
        sink: &dyn RunEventSink,
    ) -> Result<ExecutionTerminal, ExecuteError>;

    async fn cancel(&self, run_id: RunId) -> Result<(), ExecuteError>;
}

#[async_trait]
pub trait ToolHost {
    async fn invoke(
        &self,
        call: ToolCall,
        grant: &ResolvedGrant,
    ) -> Result<ToolResult, ToolError>;
}

#[async_trait]
pub trait Signer {
    async fn describe(&self) -> Result<SignerCapabilities, SignerError>;
    async fn sign(&self, request: CanonicalSignRequest)
        -> Result<SignedPayload, SignerError>;
}

#[async_trait]
pub trait ChainClient {
    async fn resolve_profile(
        &self,
        profile: ChainProfileRef,
    ) -> Result<ResolvedChainProfile, ChainError>;

    async fn prepare(
        &self,
        intent: ChainIntent,
    ) -> Result<PreparedAction, ChainError>;

    async fn submit(
        &self,
        signed: SignedPayload,
    ) -> Result<SubmissionReceipt, ChainError>;

    async fn watch(
        &self,
        receipt: SubmissionReceipt,
    ) -> Result<FinalityOutcome, ChainError>;
}
```

These are conceptual examples, not accepted compile-ready APIs. They show that
the executor can propose work but cannot bypass `ToolHost`, `PolicyEvaluator`,
`Signer`, or `ChainClient`.

## 7. Models, harnesses, tools, skills, and workflows

### 7.1 Provider and model flexibility

**Established**

- Support hosted model APIs, OpenAI-compatible APIs, Anthropic-style APIs,
  gateways/routers, local models, and future providers through adapters.
- Model identity, provider connection, execution mode, context constraints,
  tool support, pricing metadata, and routing policy are separate data.
- Users can choose a simple preset or an explicit route.
- Routing decisions are recorded and explainable.
- Secrets are resolved at execution time and never embedded in portable specs.
- Provider onboarding requires contract tests for streaming, cancellation,
  tool calls, errors, limits, and usage accounting.

### 7.2 Harness flexibility

Harnesses may include direct coding CLIs, ACP/A2A services, Roko-like runtimes,
framework bridges, remote workers, and custom processes. The platform must
support:

- Start, health, capability discovery, execute/resume, cancel, interrupt,
  upgrade, and shutdown lifecycles.
- Session identity and portable or explicitly non-portable resume tokens.
- Normalized progress, text, tool request/result, artifact, usage, warning,
  error, and terminal events.
- Sandboxed workspaces and mediated tool access.
- Version/capability negotiation and conformance fixtures.

### 7.3 Skills and tools

- A **tool** performs a typed operation with declared effects and permissions.
- A **skill** packages instructions, context, schemas, tool requirements,
  examples, tests, cost/risk metadata, and compatibility constraints.
- A **context pack** supplies attributed knowledge without executable
  authority.
- A **product kit** composes agents, skills, UI/prompt assets, policies,
  fixtures, and deployment defaults for a user outcome.
- An **extension** is a versioned executable or service integration behind a
  stable contract.

These concepts remain separate in manifests, UX, policy, and marketplace
filtering.

### 7.4 Workflows and multi-agent systems

Roko-style graphs, groups, feeds, triggers, memory, and learning are valuable
platform capabilities. They should be represented as optional orchestration
and projection layers over the same run/effect kernel, not a second execution
engine.

The simple API starts a run without requiring a graph. Advanced workflows may
add:

- Typed nodes and edges.
- Conditional routing, joins, fan-out, retries, and compensations.
- Human or service approval nodes.
- Agent roles, groups, delegation, quorum, and escalation.
- Scheduled, event, chain, webhook, feed, and message triggers.
- Versioned checkpoints and deterministic replay boundaries.
- Shared artifacts and attributed memory with explicit visibility.

Learning or evaluation output may propose a new configuration or policy, but it
cannot silently expand its own authority.

## 8. Custody, policy, and fully configurable autonomy

### 8.1 Custody is configurable

**Established**

Polkagent must support multiple signer and custody modes behind one signer
contract, including:

- User wallet or mobile-host signing.
- Hardware wallet.
- Proxy or multisig.
- Local encrypted agent keystore.
- Remote organizational signer.
- Managed KMS/HSM signer.
- MPC or threshold signer.
- Smart/proxy/programmable account.
- Independently funded agent account.

**Default:** external or user-controlled signing for valuable production
actions, with no raw key material exposed to models, harnesses, ordinary tools,
or marketplace extensions.

Hosted or agent-controlled custody is an explicit configuration choice with
clear recovery, rotation, export, revocation, monitoring, and liability
information.

### 8.2 Autonomy is a policy mode, not a hard-coded product ceiling

**Established**

Every supported action family may ultimately run without per-action human
approval when the owner explicitly configures that mode. This can include
payments, contract calls and deployments, governance actions, infrastructure
changes, runtime/chain operations, and other high-impact work.

Supported autonomy levels should include:

| Level | Behavior |
|---|---|
| Observe | Read, monitor, explain, and alert only. |
| Prepare | Produce plans, code, intents, simulations, and approval-ready artifacts. |
| Per-action approval | Require a person, quorum, or external policy service for each effect. |
| Session or batch approval | Pre-authorize an exact bounded set of effects for a time/window. |
| Policy-autonomous | Execute without per-action approval while deterministic policies permit it. |
| Fully autonomous | Operate continuously with a configured signer/account and no routine human approval. |

**Default:** observation/preparation for unknown or high-risk integrations,
then per-action approval as capabilities are enabled.

**Configurable:** users may deliberately choose broader autonomy, higher limits,
or no per-action approval. “Fully autonomous” means the runtime can proceed
without a person in the loop; it does not mean the model receives raw keys or
that durability, audit, signer isolation, revocation, and status invariants
disappear.

A high-autonomy configuration must make the following understandable:

- Accounts, networks, assets, contracts, pallets, calls, and destinations in
  scope.
- Per-action, rolling, and lifetime budgets or an explicit decision to use
  broad/unlimited values.
- Fee, slippage, rate, frequency, and concurrency constraints.
- Required evidence and simulation rules.
- Upgrade and policy-change authority.
- Failure, unknown-finality, retry, and compensation behavior.
- Circuit breakers, pause, revocation, and emergency recovery.
- Notification and audit destinations.
- Who may change the policy or signer.

No UI may represent “autonomous” as a vague toggle detached from these
consequences.

### 8.3 Effective grants

The effective grant is the deterministic intersection or resolution of:

- Agent/product policy.
- Deployment and environment policy.
- Tenant/organization policy.
- Sender, conversation, or trigger policy.
- Model/harness/tool/extension capability.
- Workflow-node or task policy.
- Account/signer policy.
- Chain-specific risk policy.
- Time-bound approvals or autonomous mandate.

The resulting `ResolvedGrant` is immutable, hashed, attributable, and bound to
the effect attempt.

### 8.4 Threat model in plain language

The design assumes that any of the following can be wrong, malicious,
compromised, stale, or misleading:

- A user or public sender.
- A model response or coding harness.
- Retrieved web content, repository text, chat attachment, or memory.
- A marketplace extension, tool, skill, or update.
- A model provider, remote worker, RPC endpoint, indexer, or chain fork.
- A transport or notification service.
- A cloud tenant, administrator, support workflow, or neighboring workload.
- A wallet integration or signer response.
- A displayed asset name, network name, simulation, fee estimate, or finality
  observation.

The system therefore uses authenticated principals, data classification,
content provenance, typed intents, deterministic policy, exact grants,
sandboxed/mediated effects, isolated signers, pinned chain identity and
metadata, independent outcome observation, tenant isolation, and durable audit.

Flexibility changes policy, not reality. A fully autonomous owner may choose a
very broad mandate, but the product must still report precisely which mandate
authorized an action, which account signed it, which payload was submitted,
and what outcome was observed.

## 9. Permissionless extension and marketplace direction

### 9.1 Permissionless base

**Established**

Publishing and discovering compatible agents, skills, tools, models, compute,
feeds, services, extensions, and product kits should be as permissionless and
flexible as practical.

The architecture should support:

- Multiple registries and marketplaces.
- Local folders and direct package references.
- Organization-private registries.
- Community registries.
- Federated and on-chain registries where useful.
- Self-publishing without approval from a single Polkagent operator.
- Mirrors, pinned dependencies, offline bundles, and portable manifests.
- Free, paid, metered, subscription, sponsored, and negotiated offerings.
- Configurable creator, operator, registry, protocol, and referral fees.

### 9.2 Safe discovery is a UX layer, not a publication gate

Permissionless publishing does not require unsafe default execution. The
default user experience should provide:

- Signed content and publisher identity where available.
- Content-addressed versions and reproducible manifests.
- Permission, data-access, effect, cost, and custody labels before install.
- Automated compatibility, malware, secret-leakage, and conformance checks.
- Reviews, reputation, provenance, usage history, and verified-build signals.
- Curated and verified discovery views without preventing an expert from
  selecting another registry or sideloading.
- Sandboxed preview and dry-run.
- Version pinning, lockfiles, staged updates, rollback, revocation, and security
  advisories.
- Organization allow/deny lists and regulated deployment policies.

Trust signals must remain plural and inspectable. A central ranking service is
optional, replaceable, and not the source of execution authority.

### 9.3 Package lifecycle

```text
author source
  → build and test
  → generate manifest, digest, provenance and signatures
  → publish to one or more registries
  → index/rank/filter in discovery views
  → inspect permissions, data, effects, cost and compatibility
  → preview in isolation
  → install a pinned version and lock dependencies
  → grant capabilities separately
  → activate in a deployment
  → observe, update in stages, rollback, revoke or remove
```

Discovery, installation, activation, authorization, and payment are separate
states. Finding a package does not install it; installing it does not grant
tools, data, money, or public reach; paying for it does not make it trusted.

## 10. Polkadot-native integration scope

The integration architecture and roadmap should cover, with current maturity
verified before implementation:

- Polkadot SDK and Substrate-style runtime development.
- Subxt-based Rust chain access and generated/static interfaces where useful.
- PAPI and browser/client integrations where they improve Product or wallet UX.
- Polkadot Hub and relevant system functionality.
- Native assets, asset operations, fees, locations, decimals, and identity.
- XCM construction, dry-run/preflight where available, delivery tracking, and
  cross-chain outcome uncertainty.
- OpenGov research, referendum explanation, delegation, voting, treasury, and
  organizational workflows.
- Identity, people/personhood, proxy, multisig, and reputation signals as
  optional capabilities or policy inputs.
- Contracts and PVM/PolkaVM development and execution.
- Runtime metadata, upgrade, migration, benchmark, and compatibility work.
- Wallets, mobile hosts, external signers, hardware, organizational custody,
  and agent accounts.
- Statement/Bulletin/chat-style transport and notification surfaces where
  publicly supported.
- JAM SDK/service development, testing, deployment, operation, and agent
  coordination as experimental interfaces become stable.

Network identity must be based on pinned genesis/spec/profile information.
Asset identity must not rely on ticker text alone. Runtime metadata and versions
used to decode or approve an action are durable evidence.

## 11. Identity, memory, learning, and reputation

### 11.1 Identity

- Local-only and pseudonymous agents remain possible.
- Users, agents, services, organizations, devices, accounts, and publishers are
  distinct identity types.
- Polkadot-native identity, personhood, registries, credentials, and reputation
  can improve discovery, policy, recovery, payment, and accountability.
- No on-chain identity is mandatory for a private local agent.
- Identity claims are attributed evidence, not automatic authority.

### 11.2 Memory and knowledge

Polkagent should support:

- Short-lived working context.
- Durable episodic run history.
- Semantic/project knowledge.
- Procedural skills and playbooks.
- User/team preferences and policies.
- Artifact links, provenance, citations, classification, retention, and
  visibility.
- Optional vector, full-text, graph, and external knowledge-store projections.

Memory assembly is bounded, attributed, policy-filtered, and observable.
Deletion and export must affect derived indexes as well as primary records.

### 11.3 Learning and evaluation

- Evals measure correctness, safety, comprehension, usefulness, latency, cost,
  and reliability.
- Feedback and evals may recommend prompt, route, model, skill, workflow, or
  policy changes.
- Changes are versioned, tested, compared, and promoted through explicit
  policy.
- A learning subsystem cannot grant itself tools, money, credentials, or
  production deployment authority.
- Multi-agent review can add evidence but does not replace deterministic
  verification or human accountability where configured.

## 12. Self-hosted and managed cloud topology

### 12.1 Portable data plane

The runtime/data plane may run on a laptop, server, edge host, organization
cluster, or managed worker. It owns:

- Conversations, runs, effects, local workspaces, and classified artifacts.
- Provider/harness/tool execution.
- Transport sessions.
- Policy evaluation and grants.
- Chain reads, signer coordination, submission, and watching.
- Local secrets through an appropriate resolver.

### 12.2 Optional control plane

A managed or self-hosted control plane may provide:

- Tenant, organization, project, environment, user, and service-account
  administration.
- Fleet registration, deployment, upgrades, health, and rollback.
- Policy/configuration distribution and drift visibility.
- Shared registries, marketplace, billing, quotas, and usage accounting.
- Collaboration, approval routing, audit search, and incident response.
- Managed storage, backup, scheduling, feeds, notifications, and observability.

Tenant identity and authorization are evaluated on every control-plane and
data-plane boundary. Queue messages, object paths, encryption keys, caches,
telemetry, and support tooling are tenant-scoped.

### 12.3 Portability contract

A user must be able to export, subject to secret-provider limitations:

- Agent and product specs.
- Policies and configuration.
- Marketplace/extension lockfiles.
- Identity and account bindings.
- Conversations, runs, events, approvals, receipts, and artifacts.
- Memory records and derived-index rebuild inputs.
- Deployment metadata and audit evidence.

Imports are versioned, previewable, resumable, and auditable.

### 12.4 Illustrative deployment modes

| Mode | Runtime location | Management | Typical custody | Intended user |
|---|---|---|---|---|
| Local personal | User laptop/workstation | Local CLI/Studio | Wallet/hardware/local opt-in | Individual builder or private assistant |
| Self-hosted server | User/organization server | Self-hosted console or CLI | Wallet, proxy/multisig, remote signer | Team or always-on private service |
| Hybrid | Local/private workers plus control plane | Managed or self-hosted control plane | Keys remain in chosen data plane | Organization needing central fleet UX |
| Managed | Isolated managed worker/data plane | Managed control plane | External signer by default; managed KMS/HSM/MPC opt-in | Team/public service seeking low operations burden |
| Embedded | Inside another Polkadot product | Host plus Polkagent SDK | Host-mediated signer/policy | Product developer |

The same `AgentSpec`, policies, package lockfile, run semantics, and export
contract apply. Some secret backends cannot export keys; migration must still
export account identity, signer requirements, and re-binding instructions
rather than silently losing capability.

## 13. `polkadot-chat-agents` compatibility baseline

The PCA source/code review identifies the following as observable or
documented reference-product behavior. Each must still be pinned to a source
revision and executable fixture before Polkagent claims compatibility:

- Private/outbound-compatible encrypted Polkadot chat.
- Device/session polling, durable-before-ACK admission, deduplication, leases,
  owed-work recovery, and ordered egress.
- Text, files, images/media, scoped downloads, and generated artifacts.
- Progress updates, chunks/edits, live replies, final delivery, and
  notifications.
- Projects/workspaces, conversation history, model switching, direct model and
  coding-harness modes, session continuation, and cancellation.
- T3ams and supported framework/bridge integrations such as Hermes/OpenClaw.
- CLI lifecycle, doctor/status/logs, installation, upgrades, configuration,
  and deployment.

The following are Polkagent successor requirements, not claims that PCA already
implements a complete migration or cloud architecture:

- Identity, state, project, session, file, bridge, and configuration
  import/export.
- Shadow/conformance mode, operator-reviewed cutover, rollback, and unknown
  pending-work reconciliation.
- One canonical durable state model across direct and bridge execution.
- A richer cross-surface Inbox, approval/evidence views, managed cloud, and
  portable local/cloud deployments.
- Improved capability, secret, signer, artifact, observability, and recovery
  boundaries.

Compatibility is delivered through explicit levels, versioned protocols,
fixtures, shadow/conformance tests, and operator-visible migration. It does not
require reproducing PCA’s internal coupling.

The target is:

- **C0:** externally visible user and transport behavior.
- **C1:** bridge/protocol compatibility for existing integrations.
- **C2:** state/config/identity/project migration and operational replacement.
- **C3:** a better native Polkagent experience using the same durable domain
  model.

## 14. Product surfaces

The complete platform should provide coherent projections of the same state:

| Surface | Primary use |
|---|---|
| Agent Studio | Create, test, configure, evaluate, publish, and deploy agents/products. |
| Agent Inbox | Conversations, tasks, progress, approvals, receipts, artifacts, and follow-ups. |
| Polkadot mobile/app integration | Mobile chat, identity, notifications, host-mediated signing/approval, and lightweight use. |
| Web/PWA | Rich configuration, review, approval, monitoring, marketplace, and fallback access. |
| CLI/TUI | Local builders, automation, scripting, diagnostics, and headless use. |
| API and SDKs | Embed agents and platform functions into Polkadot products. |
| Operator console | Tenancy, fleets, policy, audit, cost, incidents, and support. |
| Registry/marketplace | Permissionless publication, discovery, install, purchase, update, and reputation. |

An approval or action sheet is rendered from canonical typed intent and
evidence, not solely from model prose. It clearly distinguishes proposed,
approved, signed, submitted, included, finalized, failed, reverted, expired,
cancelled, and unknown states.

## 15. Phased delivery without shrinking the north star

The roadmap should be risk-ordered, but each long-term pillar remains explicit.

### Phase 0 — contracts and seams

- Establish Rust workspace boundaries and stable vocabulary.
- Implement minimal run, artifact, event, effect, grant, and projection
  contracts.
- Prove crash-safe ingress, outbox, retries, cancellation, and streaming.
- Create provider, harness, tool, transport, signer, and chain conformance
  suites.
- Capture PCA compatibility fixtures and current Polkadot integration evidence.

### Phase 1 — three-pillar demonstrator

- **Build:** one useful repository/product-engineering workflow with a direct
  executor or coding harness and mediated workspace tools.
- **Act:** an Explain Before Sign flow for one pinned chain/action class, using
  a fake or external signer and canonical evidence.
- **Reach:** one excellent web/CLI experience plus a PCA-compatible or
  Polkadot-mobile integration spike with a defined fallback.

This validates the shared kernel. It is not the final boundary of Polkagent.

### Phase 2 — safe useful platform

- More providers, harnesses, tools, read-only chain skills, project memory, and
  product kits.
- PCA C0/C1 compatibility and explicit migration work.
- External signing, limited chain actions, approvals, policies, simulations,
  notifications, and recovery.
- Self-host deployment and an initial managed control-plane slice.

### Phase 3 — make, publish, operate, and pay

- Agent Studio and Inbox maturation.
- Product SDKs, embedded surfaces, team workflows, feeds, triggers, and
  multi-agent orchestration.
- Permissionless registries/marketplace with safe discovery and execution.
- Payment intents, quotes, receipts, reconciliation, budgets, and agent
  accounts.
- Cloud tenancy, fleets, billing, collaboration, and portable migration.

### Phase 4 — policy-autonomous products

- Fully autonomous operation for configured accounts and action families.
- Strong monitoring, policy simulation, circuit breakers, incident tooling,
  recovery, and autonomous payment/service operations.
- Contract/runtime/infrastructure workflows after family-specific gates.
- Reputation, credentials, personhood, and richer economic coordination.

### Phase 5 — advanced Polkadot and JAM/PVM systems

- PVM/PolkaVM product and contract automation.
- JAM service-building, operation, markets, and agent coordination where
  protocols are stable enough.
- Advanced memory, learning, federated agents, economic groups, and on-chain
  provenance where they demonstrate user value.

Phases express dependency order, not a promise that every capability waits for
the prior phase to be completely finished. Independent tracks may advance when
their contracts and gates are satisfied.

## 16. Good defaults

Unless a product kit or operator policy deliberately changes them:

- Run locally or in a user-selected deployment.
- Start private; do not publish an endpoint or marketplace listing
  accidentally.
- Use a recommended, explicit model/harness preset with transparent cost and
  data handling.
- Give no tools, filesystem roots, network hosts, accounts, or chain-write
  permissions until required.
- Use read-only chain access before write access.
- Use an external/user-controlled signer for valuable production actions.
- Require per-action approval for newly enabled or high-risk action families.
- Pin networks, runtime metadata, package versions, and extension lockfiles.
- Preserve durable events/effects before acknowledging work.
- Encrypt sensitive artifacts and minimize retention.
- Show a preview/dry-run and plain-language policy summary.
- Use verified/curated discovery views over a permissionless registry while
  allowing an expert to select other sources.
- Keep managed telemetry and support access opt-in and visible.

All of these defaults are designed to be understandable and configurable.
Changing a default must not silently disable core integrity properties such as
idempotency, signer isolation, tenant isolation, or truthful outcome states.

### 16.1 Illustrative configuration

The final schema will be versioned and validated. This example shows how a
simple default and advanced autonomy can share one model:

```yaml
apiVersion: polkagent.dev/v1alpha1
kind: Agent
metadata:
  name: treasury-monitor

spec:
  execution:
    route: recommended-coding-harness
    budget:
      modelUsdPerRun: 2.00

  surfaces:
    - type: web_inbox
    - type: polkadot_chat
      profile: private-mobile

  skills:
    - package: registry.example/opengov-monitor
      version: 1.4.2

  chainProfiles:
    - ref: polkadot-production

  autonomy:
    mode: policy_autonomous
    accounts:
      - ref: agent-proxy-1
    allow:
      - actionFamily: governance.vote
        networks: [polkadot-production]
    deny:
      - actionFamily: runtime.upgrade
    onUnknownOutcome: pause
    circuitBreakers:
      failuresPerHour: 3

  signer:
    type: remote_organization_signer
    ref: treasury-policy-service

  deployment:
    target: self-hosted
```

A user choosing `fully_autonomous` could configure broader allowed action
families and limits. The UI must preview the resulting mandate, require the
authorized policy owner to confirm the change, and retain a revocation path.
The model never receives the signer credential.

## 17. Invariants versus configurable policy

### Platform integrity invariants

These remain true in local, cloud, approval-based, and fully autonomous modes:

- Durable, idempotent effect execution.
- Authenticated and scoped identities.
- Exact signer payload/network binding.
- Explicit policy/configuration revision binding.
- Auditability of permission, approval, signing, submission, and outcome.
- No raw signing keys in model context.
- Clear unknown/failure states instead of false success.
- Tenant and secret isolation.
- Reproducible package/extension identity.
- Revocation and incident-recovery paths.

### User-configurable policy

Users or authorized operators may configure:

- Providers, models, harnesses, routes, fallbacks, and budgets.
- Tools, skills, extensions, workspaces, hosts, data visibility, and retention.
- Transports, public endpoints, senders, rate limits, and paid access.
- Identity requirements and reputation inputs.
- Accounts, custody backends, signers, proxies, multisigs, and agent accounts.
- Allowed action families, networks, assets, destinations, contracts, and
  amounts.
- Approval topology or fully autonomous execution.
- Marketplace/registry sources and trust filters.
- Local, self-hosted, hybrid, or managed deployment.
- Memory, learning, evaluation, workflow, group, feed, and trigger behavior.

## 18. Validation and evidence posture

The platform vision is established; individual product and protocol assumptions
still require evidence. Every major capability should carry:

- Maturity: verified current, planned, experimental, or research-only.
- Source and access date for time-sensitive ecosystem facts.
- User and job hypothesis.
- Security and trust boundary.
- Fixture or conformance corpus.
- Measurable acceptance and release criteria.
- Failure/stop condition.
- Operational owner and recovery plan.

Explain Before Sign remains a strong early validation slice because it exercises
chain decoding, evidence, policy, approvals, signing, finality, and user
comprehension. It must not be used to erase the equally central builder,
mobile/chat, payment, cloud, marketplace, or autonomy goals.

### 18.1 What success eventually means

Product success is not “the model produced a plausible answer.” The definitive
PRDs should establish measurable outcomes such as:

- A new user can create and run a useful private agent with safe defaults
  without understanding the internal adapter model.
- A PCA user can migrate identity, state, projects, and supported integrations
  without losing messages or replies.
- A Polkadot builder can complete a real repository or product workflow with
  evidence-backed artifacts and less manual coordination.
- Users correctly understand canonical approval and autonomy consequences and
  reject dangerous fixtures.
- Duplicate delivery, retries, restarts, and unknown chain outcomes do not
  create duplicate irreversible effects.
- An organization can move an agent between local and managed operation with a
  documented, testable portability result.
- A third party can publish an extension or product kit without central
  permission, while a default user can understand and constrain its risk.
- A configured autonomous agent operates within its selected mandate, produces
  receipts and audit evidence, and can be paused/revoked/recovered.
- Provider, harness, transport, signer, chain, and store adapters pass shared
  conformance suites.
- Operational reliability, latency, cost, privacy, and security meet explicit
  service-level and release objectives.

## 19. Superseded constraints from older planning

The following older statements are not current platform constraints:

- “Polkagent is only an Explain Before Sign product.”
- “PCA/mobile compatibility is out of the product rather than phased.”
- “Managed cloud is secondary to self-hosting.”
- “Agent accounts, autonomous payments, or autonomous production actions can
  never be supported.”
- “Every irreversible action must always have a human approval.”
- “The marketplace must be centrally curated or publication-gated.”
- “Payments, marketplace, multi-agent, memory/learning, product Studio, or
  JAM/PVM ideas should be discarded because they are not in the first slice.”

The useful safety rationale behind those statements survives as defaults,
explicit configuration, maturity labels, validation gates, and architecture
invariants.

## 20. Remaining research, not remaining vision questions

The following work should refine implementation and sequencing:

- Verify the current public capabilities and stability of Polkadot App/Product,
  mobile chat, Statement/Bulletin transport, signing, notification, and identity
  APIs.
- Select the best initial Polkadot network profile, signer, and action fixtures.
- Define exact PCA C0–C3 conformance and migration corpora.
- Prove Subxt/PAPI/Product SDK division of responsibility.
- Define family-specific XCM, governance, contract/PVM, runtime, and JAM safety
  models.
- Select isolation mechanisms for process/RPC, WASM/component, remote, and
  marketplace extensions.
- Compare self-hosted and managed signer/custody backends and recovery models.
- Define cloud tenant, region, encryption, billing, support, and SLO contracts.
- Validate marketplace demand, payments, dispute handling, abuse prevention,
  licensing, and trust signals.
- Validate Agent Studio, Inbox, mobile, and approval UX with representative
  users.
- Establish performance, cost, reliability, comprehension, and security gates
  for each phase.

These questions determine how Polkagent is delivered, not whether the complete
platform remains the north star.

## 21. Required interpretation by future PRDs

Every definitive PRD must:

1. Be understandable without reading the iterative `tmp` corpus.
2. Distinguish long-term platform requirements from the release that first
   validates them.
3. Preserve simple defaults and advanced configurability together.
4. Treat PCA compatibility and improved mobile/chat UX as core product work.
5. Treat product building, on-chain actions/payments, and user reach as equal
   pillars.
6. Allow permissionless ecosystems while providing safe default discovery and
   execution.
7. Allow fully autonomous configured operation without weakening durable,
   truthful, isolated execution.
8. Keep local/self-hosted and managed cloud contracts portable and equivalent.
9. Use Roko’s strongest patterns selectively: artifacts, events, effects,
   narrow ports, grants, projections, graphs/groups/memory where useful, and
   explicit gates.
10. Include implementation-level traits, schemas, state machines, APIs,
    configuration examples, UX flows, deployment models, migration,
    acceptance criteria, and roadmap gates.
