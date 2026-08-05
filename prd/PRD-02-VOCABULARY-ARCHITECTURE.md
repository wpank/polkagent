# PRD-02 — Vocabulary, Invariants and System Architecture

> **Implementation note (audited 2026-08-05):** This PRD remains normative,
> but its embedded implementation statements and checklists are not current
> status evidence. Use [STATUS.md](STATUS.md) and
> [IMPLEMENTATION-BACKLOG.md](IMPLEMENTATION-BACKLOG.md) for verified state and
> the dependency-ordered execution queue.

**Status:** definitive PRD
**Audience:** engineers, architects, product designers, and security reviewers with no prior Polkagent context
**Normative authority:** `01-ESTABLISHED-BASELINE.md` is the owner-confirmed product direction. This PRD converts that direction into precise, implementable architecture without contradicting or narrowing the established scope.
**Implementation status:** architecture specification; no production code exists yet
**Date:** 2026-07-30

---

## 1. Purpose and reader orientation

### 1.1 What this document does

This PRD defines the **canonical vocabulary**, **system invariants**, and **layered architecture** of Polkagent. Every subsequent PRD in the suite uses the terms, layers, dependency rules, and invariants established here. If a later document needs a term not defined below, it must define it locally and propose it for inclusion here.

This document is self-contained. A competent engineer who has never read the Polkagent planning corpus, the Roko source, or the PCA codebase should be able to understand:

- What every domain term means and how it relates to other terms.
- Which properties hold regardless of configuration, deployment, or autonomy mode.
- How the system is layered and which dependencies are allowed.
- Where trust boundaries fall and how they are enforced.
- What the proposed Rust workspace looks like and why.

### 1.2 What Polkagent is

Polkagent is a proposed Rust-first, Polkadot-native product-engineering and agent platform with three equal pillars:

1. **Build:** help users create, test, deploy, and operate Polkadot SDK runtimes, chains, contracts/PVM programs, JAM services, applications, and integrations.
2. **Act:** safely research, explain, prepare, simulate, approve, sign, submit, watch, and -- when deliberately configured -- fully automate Polkadot payments and on-chain actions.
3. **Reach:** preserve and improve the private mobile/chat, coding-agent, project/file, and deployment experience of `polkadot-chat-agents` (PCA), with a cleaner, more modular Rust-first architecture.

The platform supports equally first-class local/self-hosted and managed multi-tenant cloud operation through portable contracts.

### 1.3 What exists today

No production implementation exists. The repository contains research, architecture decisions, and delivery plans. Two external codebases inform the design:

- **PCA** (`polkadot-chat-agents`) is the Node.js reference product. It provides encrypted Polkadot chat bots with direct coding-agent runners, framework bridges, files, and bot operations. Polkagent must preserve compatible user-visible behavior through cleaner abstractions.
- **Roko** is a separate Rust agent-platform project. It provides useful patterns for durable artifacts, events, effects, narrow ports, grants, and projections. Polkagent adopts its strongest patterns selectively.

### 1.4 How to read this document

Labels used throughout:

| Label | Meaning |
|---|---|
| **Established** | Owner-confirmed direction that definitive PRDs must preserve. |
| **Default** | Recommended out-of-box behavior; user may select another mode. |
| **Configurable** | A deliberate choice exposed through safe, understandable UX and versioned configuration. |
| **Invariant** | A property that holds in every configuration, deployment mode, and autonomy level. |
| **Phased** | Committed long-term scope, delivered after its dependencies and release gates. |
| **Experimental** | Requires research, spike, and explicit maturity labeling before production use. |
| **Proposal** | A design choice in this document, not existing behavior. |

### 1.5 Relationship to other PRDs

| PRD | Relationship |
|---|---|
| PRD-01 | Defines vision, principles, personas, pillars. This PRD provides the architectural foundation for those goals. |
| PRD-03 | Defines the execution model in detail. This PRD provides the vocabulary and invariants PRD-03 builds on. |
| PRD-04 | Defines providers, models, harnesses, tools, skills. This PRD defines the port interfaces they implement. |
| PRD-05 through PRD-15 | All depend on vocabulary and architecture rules defined here. |

---

## 2. Canonical vocabulary

Every term below is normative. If two PRDs use a term differently, this document wins. Terms are grouped by domain. Cross-references use `-->` notation.

### 2.1 Execution domain

#### Agent

A versioned product/runtime definition that combines behavior, execution route, tools/skills, context/memory, surfaces, policy, and deployment requirements. An Agent is not synonymous with a single model; it may use multiple models, harnesses, and tools across its lifetime.

**Durable record:** `AgentSpec`
**Acceptance:** An AgentSpec can be serialized, versioned, exported, imported, and instantiated in any supported deployment mode without loss of semantics.

#### AgentSpec

The portable, versioned declarative record that defines an Agent's behavior, execution routes, capability requirements, memory configuration, surface bindings, policy references, and deployment requirements. It is the unit of agent identity and versioning.

```rust
struct AgentSpec {
    id: AgentId,
    version: SemVer,
    name: String,
    description: String,
    execution: ExecutionConfig,
    skills: Vec<SkillRef>,
    tools: Vec<ToolRef>,
    surfaces: Vec<SurfaceBinding>,
    chain_profiles: Vec<ChainProfileRef>,
    memory: MemoryConfig,
    policy: PolicyRef,
    autonomy: AutonomyConfig,
    deployment: DeploymentConfig,
}
```

#### ProductSpec

A portable, versioned composition of one or more Agents plus product-kit assets, workflows, UX, tests, policies, and deployment defaults for a concrete user outcome. A ProductSpec is how a complete product experience is packaged and distributed.

#### Run

One durable execution instance. A Run is started by an ingress event (a user message, a trigger, an API call). It contains model effects, tool effects, approval effects, delivery effects, and chain effects. A Run has exactly one owner conversation or trigger binding.

A Run progresses through a defined lifecycle: `Created -> Queued -> Executing -> AwaitingApproval -> Completing -> Completed | Failed | Cancelled | TimedOut`. The `Unknown` terminal state exists for effects whose outcomes cannot be determined.

```rust
struct Run {
    id: RunId,
    conversation_id: ConversationId,
    agent_id: AgentId,
    agent_version: SemVer,
    status: RunStatus,
    created_at: Timestamp,
    started_at: Option<Timestamp>,
    completed_at: Option<Timestamp>,
    config_revision: ConfigDigest,
    policy_revision: PolicyDigest,
}

enum RunStatus {
    Created,
    Queued,
    Executing,
    AwaitingApproval,
    Completing,
    Completed,
    Failed { error: RunError },
    Cancelled { reason: CancelReason },
    TimedOut,
}
```

**Invariant:** A Run's `config_revision` and `policy_revision` are recorded at creation and immutable for the Run's lifetime.

#### Turn

A single request-response cycle within a conversation. A Turn may create one or more Runs. Turns are sequenced within a Conversation.

```rust
struct Turn {
    id: TurnId,
    conversation_id: ConversationId,
    sequence: u64,
    input: TurnInput,
    runs: Vec<RunId>,
    created_at: Timestamp,
}
```

#### Step

A discrete unit of work within a Run. Steps are ordered and represent individual actions: model inference, tool invocation, approval request, chain action preparation, signing, or delivery. Each Step produces Events and may create EffectIntents.

```rust
struct Step {
    id: StepId,
    run_id: RunId,
    sequence: u64,
    kind: StepKind,
    status: StepStatus,
    effect_intents: Vec<EffectIntentId>,
    artifacts: Vec<ArtifactId>,
    started_at: Timestamp,
    completed_at: Option<Timestamp>,
}

enum StepKind {
    ModelInference,
    ToolInvocation,
    ApprovalRequest,
    ChainActionPreparation,
    Signing,
    Broadcast,
    FinalityObservation,
    Delivery,
}
```

### 2.2 Event domain

#### Event (RunEvent)

An ordered observation that describes lifecycle or streaming activity within a Run. Events are the primary audit and recovery mechanism. Each Event has a monotonic sequence number within its Run, a durability class, correlation and causation identifiers, and a timestamp.

```rust
struct RunEvent {
    id: EventId,
    run_id: RunId,
    sequence: u64,
    kind: EventKind,
    durability: Durability,
    correlation_id: CorrelationId,
    causation_id: Option<EventId>,
    timestamp: Timestamp,
    payload: EventPayload,
}

enum Durability {
    /// Must survive crash; required for correctness.
    Durable,
    /// May be lost on crash; used for progress/streaming.
    BestEffort,
}

enum EventKind {
    RunCreated,
    RunStarted,
    StepStarted,
    StepCompleted,
    EffectIntentCreated,
    EffectAttemptStarted,
    EffectOutcomeRecorded,
    ApprovalRequested,
    ApprovalGranted,
    ApprovalDenied,
    ArtifactCreated,
    DeliveryStarted,
    DeliveryCompleted,
    RunCompleted,
    RunFailed,
    RunCancelled,
    RunTimedOut,
    // Streaming / progress (BestEffort durability)
    TextChunk,
    ProgressUpdate,
    ToolCallStarted,
    ToolCallCompleted,
}
```

**Invariant:** Durable Events are written transactionally with the state changes they describe. A crash between state change and Event write is not possible in a correct implementation.

#### EventStream

A time-ordered, filterable view of Events. EventStreams are used for live UX updates (SSE/WebSocket), audit queries, and recovery replay. An EventStream is a projection -- it derives from durable Events and can be rebuilt.

### 2.3 Artifact domain

#### Artifact

Durable, attributable content or evidence. Artifacts are immutable once created. They carry a content digest, type, classification, provenance, and parent references forming an evidence lineage.

```rust
struct Artifact {
    id: ArtifactId,
    kind: ArtifactKind,
    digest: ContentDigest,
    parents: Vec<ArtifactId>,
    provenance: Provenance,
    classification: Classification,
    body_ref: BlobRef,
    created_at: Timestamp,
    run_id: Option<RunId>,
    step_id: Option<StepId>,
}

enum ArtifactKind {
    /// A file produced or modified during a Run.
    File { path: String, mime_type: String },
    /// A code diff or patch.
    Diff { base_ref: Option<String>, target_ref: Option<String> },
    /// A plan produced by a model or workflow.
    Plan { format: PlanFormat },
    /// A decoded chain call with full metadata context.
    DecodedCall { chain_profile: ChainProfileId, metadata_hash: MetadataDigest },
    /// A simulation result for a proposed chain action.
    Simulation { chain_profile: ChainProfileId, block_ref: BlockRef },
    /// A signed receipt from a completed chain action.
    Receipt { chain_profile: ChainProfileId, tx_hash: TxHash },
    /// A test result or evaluation output.
    TestResult { suite: String, passed: bool },
    /// Runtime metadata snapshot.
    RuntimeMetadata { chain_profile: ChainProfileId, spec_version: u32 },
    /// Context pack used for a model inference.
    ContextPack { digest: ContentDigest },
    /// Model response captured as evidence.
    ModelResponse { model_id: ModelId, usage: Usage },
    /// Custom artifact type from an extension.
    Custom { type_uri: String },
}

struct Provenance {
    source: ProvenanceSource,
    software: String,
    software_version: String,
    timestamp: Timestamp,
}

enum ProvenanceSource {
    Agent { agent_id: AgentId },
    User { user_id: UserId },
    System,
    External { uri: String },
    ChainState { chain_profile: ChainProfileId, block: BlockRef },
}

enum Classification {
    /// May appear in public projections, logs, and model context.
    Public,
    /// Visible to authorized users and operators; excluded from public views.
    Private,
    /// Requires additional access controls; excluded from default operator views.
    Sensitive,
    /// Must never contain secret material. Stored in secret-excluded paths.
    /// Violated by: wallet seeds, raw API keys, unredacted tokens.
    SecretForbidden,
}
```

**Invariant:** An Artifact's `digest` is computed over its body content and verified on read. Tampered artifacts fail verification.
**Invariant:** `SecretForbidden` classification means the artifact must not contain raw signing keys, unredacted API tokens, or other material that would make retention, projection, or model context unsafe.

#### Content addressing algorithm

**Proposal:** Use **BLAKE3** for all internal artifact digests and content-addressed storage. BLAKE3 offers significantly faster hashing than SHA-2 and is tree-parallelizable, which is advantageous for large artifacts (files, runtime metadata, model responses). Where interoperability requires SHA-256 — Sigstore attestations, IPFS CIDv0 anchoring, on-chain hash references — a parallel SHA-256 digest is computed and stored alongside the BLAKE3 digest. The `ContentDigest` type carries an algorithm tag so callers are not required to assume a single global algorithm.

**Acceptance criteria:**
1. A completed chain action can show linked metadata, decoded call, simulation, approval, signed payload, and finality artifacts after a process restart.
2. Artifact lineage (parent links) form a directed acyclic graph (DAG) traceable from any artifact to its root inputs.
3. Classification is enforced at storage, projection, and model-context boundaries.

### 2.4 Effect domain

#### Effect

An Effect is actual external work: calling a model, running a tool, sending a message, requesting a signature, submitting a transaction, or observing finality. Effects are the mechanism by which a Run interacts with the world outside the kernel.

The Effect lifecycle has three distinct records:

#### EffectIntent

The durable command recorded **before** I/O occurs. An EffectIntent describes what must be done, not what has happened. It is created transactionally with the state change that requests it.

```rust
struct EffectIntent {
    id: EffectIntentId,
    run_id: RunId,
    step_id: StepId,
    kind: EffectKind,
    idempotency_key: IdempotencyKey,
    payload: EffectPayload,
    grant_snapshot: ResolvedGrantDigest,
    deadline: Option<Timestamp>,
    retry_policy: RetryPolicy,
    created_at: Timestamp,
}

enum EffectKind {
    ExecuteModel,
    InvokeTool,
    RequestSignature,
    BroadcastTransaction,
    ObserveFinality,
    DeliverMessage,
    ReadChainState,
    SimulateAction,
    ExternalService,
}
```

**Invariant:** An EffectIntent is durable before the corresponding I/O begins. If the process crashes after creating the intent but before starting I/O, the intent is recovered and processing resumes without loss.

#### EffectAttempt

Records one claim, lease, retry number, idempotency key, timing, and worker lifecycle for one attempt at executing an EffectIntent. Multiple EffectAttempts may exist for one EffectIntent (due to retries, timeouts, or worker failures).

```rust
struct EffectAttempt {
    id: EffectAttemptId,
    intent_id: EffectIntentId,
    attempt_number: u32,
    idempotency_key: IdempotencyKey,
    worker_id: WorkerId,
    lease_expires_at: Timestamp,
    started_at: Timestamp,
    completed_at: Option<Timestamp>,
    status: AttemptStatus,
}

enum AttemptStatus {
    Claimed,
    Executing,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}
```

**Invariant:** At most one EffectAttempt for a given EffectIntent is in `Claimed` or `Executing` status at any time.

#### EffectOutcome

The immutable observed result for exactly one EffectAttempt. An EffectOutcome records success, failure, timeout, cancellation, or unknown status plus typed result/error evidence and external references.

```rust
struct EffectOutcome {
    id: EffectOutcomeId,
    attempt_id: EffectAttemptId,
    intent_id: EffectIntentId,
    status: OutcomeStatus,
    result: Option<EffectResult>,
    error: Option<EffectError>,
    external_refs: Vec<ExternalRef>,
    observed_at: Timestamp,
}

enum OutcomeStatus {
    Success,
    Failure,
    Timeout,
    Cancelled,
    /// The outcome could not be determined. This is NOT failure.
    /// Used when: RPC did not respond, process crashed during observation,
    /// chain inclusion is uncertain, or signer returned ambiguously.
    Unknown,
}
```

**Invariant:** An `Unknown` outcome is never collapsed to `Success` or `Failure` without fresh, independent evidence. The system preserves the `Unknown` state and surfaces it to operators and users.
**Invariant:** An EffectOutcome is immutable once written. Subsequent observations create new EffectAttempts and Outcomes rather than modifying existing records.

### 2.5 Authorization domain

#### Grant (ResolvedGrant)

The exact, resolved set of permissions and limits available to a Run or Effect. A Grant is the immutable, hashed intersection of all applicable policies. It is computed outside model-controlled text and bound to the specific effect or run it authorizes.

```rust
struct ResolvedGrant {
    id: GrantId,
    digest: GrantDigest,
    subject: SubjectId,
    resource: ResourceSelector,
    allowed_effects: EffectSet,
    limits: Limits,
    approvals: Vec<ApprovalId>,
    autonomous_mandate: Option<MandateRef>,
    policy_revision: PolicyDigest,
    config_revision: ConfigDigest,
    expires_at: Timestamp,
    created_at: Timestamp,
}

struct Limits {
    max_requests: Option<u64>,
    max_bytes: Option<u64>,
    max_spend: Option<MoneyAmount>,
    max_duration: Option<Duration>,
    rate_limit: Option<RateLimit>,
}

struct ResourceSelector {
    tools: Option<Vec<ToolSelector>>,
    filesystem_roots: Option<Vec<PathPattern>>,
    network_hosts: Option<Vec<HostPattern>>,
    chain_profiles: Option<Vec<ChainProfileId>>,
    accounts: Option<Vec<AccountSelector>>,
    action_families: Option<Vec<ActionFamily>>,
}
```

**Invariant:** Grants are resolved outside model-controlled text. An LLM may propose an action, but only deterministic policy evaluation produces a Grant.

**Grant resolution formula:**
```
effective_permission = platform_policy
                     ∩ tenant_policy
                     ∩ workspace_policy
                     ∩ agent_policy
                     ∩ workflow_policy
                     ∩ extension_capability
                     ∩ current_approval_or_mandate
                     ∩ account_policy
                     ∩ chain_risk_policy
                     ∩ budget_remaining
```

**Acceptance criteria:**
1. An extension cannot exceed manifest, workspace, action, and approval limits.
2. A broad configuration flag cannot silently override a pending approval requirement.
3. Grant resolution is deterministic: same inputs always produce the same Grant.

#### Capability

A declared ability of an extension, tool, harness, or agent. Capabilities are declared in manifests and requested at installation or activation time. They are inputs to Grant resolution, not authorization themselves.

```rust
struct Capability {
    id: CapabilityId,
    kind: CapabilityKind,
    description: String,
    required: bool,
}

enum CapabilityKind {
    FileSystem { roots: Vec<PathPattern>, access: FileAccess },
    Network { hosts: Vec<HostPattern>, protocols: Vec<Protocol> },
    ChainRead { profiles: Vec<ChainProfileId> },
    ChainWrite { profiles: Vec<ChainProfileId>, action_families: Vec<ActionFamily> },
    Tool { tool_id: ToolId },
    Secret { secret_refs: Vec<SecretRef> },
    Model { model_refs: Vec<ModelRef> },
    Custom { type_uri: String, scope: String },
}
```

#### Policy

Versioned rules that decide what is permitted, denied, approval-gated, routed, or limited. Policies are declarative, versioned, and deterministically evaluable. They are inputs to Grant resolution.

```rust
struct Policy {
    id: PolicyId,
    version: SemVer,
    revision: PolicyDigest,
    rules: Vec<PolicyRule>,
    defaults: PolicyDefaults,
}

enum PolicyRule {
    Allow { selector: ResourceSelector, conditions: Vec<Condition> },
    Deny { selector: ResourceSelector, conditions: Vec<Condition> },
    RequireApproval { selector: ResourceSelector, approver: ApproverRef },
    Route { selector: ResourceSelector, target: RouteTarget },
    Limit { selector: ResourceSelector, limits: Limits },
}
```

#### Policy evaluation engine

**Proposal:** Use **Cedar** (Amazon's policy language, Rust-embeddable, formally verified in Lean) as the evaluation engine for `PolicyRule` conditions. Cedar provides: deny-by-default semantics that align with INV-06; sub-millisecond in-process evaluation with no sidecar requirement; a PARC (Principal, Action, Resource, Context) model that maps directly to agent capability grants; and a schema validator that catches policy errors before deployment. This makes it preferable to OPA/Rego, which requires a sidecar process or CGo bindings and adds operational complexity in the embedded/local-first deployment target.

The Cedar engine is encapsulated behind the `PolicyEvaluator` application service. The raw `PolicyRule` types above are the Polkagent domain vocabulary; Cedar policy documents are the serialization format that the `PolicyEvaluator` compiles and evaluates internally. Schema validation runs at configuration load time, so invalid policies are rejected before any Run is created.

#### Approval

A human, quorum, service, or pre-authorized approval bound to exact effect/action data. An Approval is not a vague "go ahead" -- it references the specific payload, grant, and evidence it authorizes.

```rust
struct Approval {
    id: ApprovalId,
    kind: ApprovalKind,
    approver: ApproverId,
    effect_intent_id: Option<EffectIntentId>,
    payload_digest: Option<ContentDigest>,
    grant_digest: GrantDigest,
    evidence_refs: Vec<ArtifactId>,
    expires_at: Timestamp,
    created_at: Timestamp,
}

enum ApprovalKind {
    Human { user_id: UserId },
    Quorum { required: u32, received: Vec<ApproverId> },
    Service { service_id: ServiceId },
    AutonomousMandate { mandate_ref: MandateRef },
}
```

### 2.6 Execution infrastructure domain

#### Provider

A connection or service that exposes one or more AI models. A Provider is not a model; it is a transport/authentication/protocol configuration for reaching models.

```rust
struct Provider {
    id: ProviderId,
    name: String,
    protocol: ProviderProtocol,
    endpoint: Endpoint,
    auth: AuthConfig,
    capabilities: ProviderCapabilities,
    health: HealthConfig,
}

enum ProviderProtocol {
    Anthropic,
    OpenAICompatible,
    Local,
    Gateway { routing: RoutingConfig },
    Custom { protocol_uri: String },
}
```

#### Model

A particular model identity and capability set, distinct from the Provider connection that serves it. Models have specific context limits, tool-call support, streaming behavior, and pricing.

```rust
struct Model {
    id: ModelId,
    provider_id: ProviderId,
    name: String,
    capabilities: ModelCapabilities,
    context_limit: TokenCount,
    pricing: Option<ModelPricing>,
}

struct ModelCapabilities {
    streaming: bool,
    tool_calls: bool,
    structured_output: bool,
    vision: bool,
    reasoning_controls: bool,
    max_output_tokens: Option<TokenCount>,
}
```

#### Executor (TurnExecutor)

An adapter that performs one Run/Turn and emits normalized Events. An Executor is the bridge between the kernel's durable state machine and the actual model/harness execution. It does not own policy, signing, or chain submission.

```rust
#[async_trait]
pub trait TurnExecutor: Send + Sync {
    async fn execute(
        &self,
        request: ExecuteRequest,
        sink: &dyn RunEventSink,
    ) -> Result<ExecutionTerminal, ExecuteError>;

    async fn cancel(&self, run_id: RunId) -> Result<(), ExecuteError>;

    fn capabilities(&self) -> ExecutorCapabilities;
}
```

#### Harness (HarnessService)

A long-lived coding-agent or agent-framework process/service with its own sessions, health, cancellation, tools, and lifecycle. A Harness is distinguished from a simple model call by having persistent process state, its own tool implementations, and a service lifecycle.

```rust
#[async_trait]
pub trait HarnessService: Send + Sync {
    async fn start(&self, config: HarnessConfig) -> Result<HarnessEndpoint, HarnessError>;
    async fn stop(&self, id: HarnessId) -> Result<(), HarnessError>;
    async fn health(&self, id: HarnessId) -> Result<HarnessHealth, HarnessError>;
    async fn status(&self, id: HarnessId) -> Result<HarnessStatus, HarnessError>;
    fn capabilities(&self) -> HarnessCapabilities;
}
```

### 2.7 Tool and skill domain

#### Tool

A typed operation an agent may invoke under an explicit capability Grant. Tools have declared inputs, outputs, effects, and permission requirements. Tool invocation is mediated by the ToolHost, never performed directly by the model.

```rust
#[async_trait]
pub trait ToolHost: Send + Sync {
    async fn invoke(
        &self,
        call: ToolCall,
        grant: &ResolvedGrant,
        ctx: &RequestContext,
    ) -> Result<ToolResult, ToolError>;

    fn available_tools(&self, grant: &ResolvedGrant) -> Vec<ToolDescriptor>;
}

struct ToolDescriptor {
    id: ToolId,
    name: String,
    version: SemVer,
    description: String,
    input_schema: JsonSchema,
    output_schema: JsonSchema,
    declared_effects: Vec<EffectKind>,
    required_capabilities: Vec<Capability>,
}
```

#### Skill

A versioned package of instructions, schemas, examples, tests, context, and declared tool/capability requirements. A Skill provides domain knowledge and workflow guidance to a model without executable authority of its own. Authority comes from the Grant system, not from Skill content.

```rust
struct Skill {
    id: SkillId,
    name: String,
    version: SemVer,
    description: String,
    instructions: String,
    schemas: Vec<SchemaRef>,
    examples: Vec<Example>,
    tests: Vec<TestRef>,
    required_tools: Vec<ToolRef>,
    required_capabilities: Vec<Capability>,
    context_items: Vec<ContextItemRef>,
}
```

#### Context Pack

Attributed knowledge supplied to a model inference without executable authority. A Context Pack records which items were included, excluded, the selection reason, token budget, and a digest for reproducibility.

```rust
struct ContextPack {
    digest: ContentDigest,
    included: Vec<ContextItem>,
    excluded: Vec<Exclusion>,
    budget: TokenBudget,
    assembled_at: Timestamp,
}

struct ContextItem {
    artifact_ref: ArtifactId,
    reason: String,
    score: f32,
    token_estimate: u32,
    classification: Classification,
}

struct Exclusion {
    artifact_ref: ArtifactId,
    reason: ExclusionReason,
}

enum ExclusionReason {
    BudgetExceeded,
    ClassificationRestricted,
    PolicyDenied,
    Expired,
    LowRelevance,
}
```

### 2.8 Transport and surface domain

#### Transport

A channel through which authenticated input, output, files, edits, and acknowledgements move. Transports implement a narrow contract for receiving messages and sending responses; they do not contain business logic.

```rust
#[async_trait]
pub trait Transport: Send + Sync {
    async fn receive(&self) -> Result<IncomingMessage, TransportError>;
    async fn ack(&self, delivery_id: DeliveryId) -> Result<(), TransportError>;
    async fn send(&self, message: OutgoingMessage) -> Result<DeliveryReceipt, TransportError>;
    fn capabilities(&self) -> TransportCapabilities;
}
```

Transport implementations include: Polkadot encrypted chat, WebSocket, HTTP API, CLI stdin/stdout, webhook ingress.

#### Surface

A user-facing presentation layer that renders Projections. Surfaces include CLI/TUI, web/PWA, Agent Studio, Agent Inbox, Polkadot mobile/app integration, and operator consoles. Surfaces are read-only consumers of Projections and senders of user input through Transports.

#### Adapter

A leaf implementation of a Port trait. Adapters are independently testable, bounded, and replaceable. Each Adapter implements exactly one Port and has no direct dependencies on other Adapters.

#### Port

A trait-defined boundary between the kernel/application layer and an external system. Ports are the stable interfaces that Adapters implement. The set of Ports is defined by the architecture; adding a new Port is an architecture decision.

### 2.9 Signing and custody domain

#### Signer

An isolated component or external wallet that signs an exact canonical payload under account policy. The Signer never receives model context, prompt content, or conversation data. It receives only the canonical bytes to sign, the account reference, and binding metadata.

```rust
#[async_trait]
pub trait Signer: Send + Sync {
    async fn describe(&self) -> Result<SignerCapabilities, SignerError>;

    async fn sign(
        &self,
        request: CanonicalSignRequest,
    ) -> Result<SignedPayload, SignerError>;
}

struct CanonicalSignRequest {
    payload: Vec<u8>,
    account: AccountRef,
    chain_profile: ChainProfileId,
    metadata_hash: MetadataDigest,
    grant_digest: GrantDigest,
    approval_id: ApprovalId,
}
```

**Invariant:** Models never receive raw signing keys. The Signer port is the exclusive path to signature production, and it receives only canonical payload bytes plus binding references.

#### Custody

The configuration of how signing keys are stored, accessed, and protected. Custody is configurable per account and may include: external wallet, hardware wallet, proxy/multisig, local encrypted keystore, remote organizational signer, managed KMS/HSM, MPC, programmable account, or funded agent account.

#### Account

A Polkadot account or account-like identity bound to a Signer and Custody configuration. Accounts are referenced by stable identifiers, not by raw key material.

### 2.10 Data and storage domain

#### Store

A trait-defined persistence boundary. Stores provide durable, consistent access to domain records. Multiple Store implementations exist for different deployment modes.

```rust
#[async_trait]
pub trait RunStore: Send + Sync {
    async fn create_run(&self, run: &Run) -> Result<(), StoreError>;
    async fn get_run(&self, id: RunId) -> Result<Run, StoreError>;
    async fn update_run_status(&self, id: RunId, status: RunStatus) -> Result<(), StoreError>;
    // ... additional methods
}

#[async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn store(&self, artifact: &Artifact, body: &[u8]) -> Result<(), StoreError>;
    async fn get(&self, id: ArtifactId) -> Result<Artifact, StoreError>;
    async fn get_body(&self, body_ref: BlobRef) -> Result<Vec<u8>, StoreError>;
    async fn verify(&self, id: ArtifactId) -> Result<bool, StoreError>;
}

#[async_trait]
pub trait EventStore: Send + Sync {
    async fn append(&self, event: &RunEvent) -> Result<(), StoreError>;
    async fn events_for_run(&self, run_id: RunId, since: u64) -> Result<Vec<RunEvent>, StoreError>;
}

#[async_trait]
pub trait EffectStore: Send + Sync {
    async fn create_intent(&self, intent: &EffectIntent) -> Result<(), StoreError>;
    async fn claim_next(&self, worker: WorkerId) -> Result<Option<EffectIntent>, StoreError>;
    async fn record_attempt(&self, attempt: &EffectAttempt) -> Result<(), StoreError>;
    async fn record_outcome(&self, outcome: &EffectOutcome) -> Result<(), StoreError>;
}
```

#### Bus

An internal event distribution mechanism for delivering Events to interested consumers (projections, live streams, telemetry). The Bus is not the source of truth -- it distributes copies of Events that are already durably stored.

#### Outbox

The durable list of EffectIntents waiting to be performed. The Outbox pattern ensures that effects are not lost on crash and are performed exactly once (or with explicit retry/dedup semantics).

The kernel uses an **event-sourced, CQRS, transactional-outbox** architecture on SQLite. State changes are produced by a **pure reducer** function with the signature `(State, Input) -> (NewState, Vec<EffectIntent>)`. Both the new state and the effect intents are written in a single SQLite transaction. This guarantees INV-03 and INV-10 without distributed coordination, and makes replay and crash recovery a function of re-running the reducer over the stored event log. CQRS means that write and read paths are segregated: the reducer owns writes through the single-writer actor; Projections and read models derive from the event log without touching the write path.

### 2.11 Identity and tenancy domain

#### Profile (ChainProfile)

A named, pinned configuration for interacting with a specific chain or network. Profiles bind a genesis hash, runtime version, RPC endpoints, metadata snapshot, and other chain-specific settings.

```rust
struct ChainProfile {
    id: ChainProfileId,
    name: String,
    genesis_hash: GenesisHash,
    spec_version: Option<u32>,
    rpc_endpoints: Vec<Endpoint>,
    metadata_snapshot: Option<ArtifactId>,
    network_type: NetworkType,
}

enum NetworkType {
    Production,
    Testnet,
    Development,
    Local,
}
```

#### Tenant

An isolation boundary for a group of users, agents, data, and policies in a managed deployment. Tenants share infrastructure but have strictly isolated data, secrets, and authorization.

#### Organization

A grouping of users, agents, and resources under shared policies and administration. An Organization may span multiple Tenants or exist within a single Tenant.

### 2.12 Extension and marketplace domain

#### Registry

A source of published packages (skills, tools, extensions, product kits). Registries may be local folders, organization-private servers, community registries, or federated on-chain registries. Multiple registries can be configured simultaneously.

#### Marketplace

A discovery and commerce layer over one or more Registries. Marketplaces add search, ratings, pricing, and transactions. A Marketplace is not a publication gate -- it is a UX layer over permissionless Registry publication.

#### Product Kit

A reviewed starter for a user outcome that composes agents, skills, policies, UI assets, fixtures, and deployment defaults. Product Kits are the primary mechanism for packaging complete user experiences.

### 2.13 Projection domain

#### Projection

A versioned, recoverable read model derived from durable records for UI, API, operations, analytics, and export. Projections are not authoritative -- they are computed from durable state and can be rebuilt.

```rust
struct Projection<T> {
    version: u64,
    cursor: EventId,
    data: T,
    computed_at: Timestamp,
    freshness: Freshness,
}

enum Freshness {
    Current,
    Stale { last_update: Timestamp },
    Rebuilding,
}
```

Named projections include: `TurnProjection`, `DeliveryProjection`, `PolicyProjection`, `ProviderProjection`, `ChainActionProjection`, `ApprovalProjection`.

**Invariant:** A trusted approval sheet is a Projection of canonical decoded payload, pinned metadata, simulation evidence, policy result, and approval state -- not a model-authored natural-language summary.

---

## 3. System invariants

System invariants are properties that hold regardless of configuration, deployment mode, autonomy level, custody choice, or extension set. They are the non-negotiable safety and correctness guarantees of the platform.

### INV-01: Models never receive raw signing keys

**Statement:** No model, harness, tool, extension, or marketplace package receives raw private key material, seed phrases, or unredacted signing credentials in any form: not in prompt context, tool results, memory, artifacts, logs, events, or configuration.

**Enforcement:**
- The `Signer` port receives only canonical payload bytes and binding metadata.
- Secret stores classify signing material as `SecretForbidden`.
- Context assembly excludes `SecretForbidden` items.
- Contract tests verify that no code path from model context reaches signing material.

**Acceptance test:** Inject a prompt-injected request for signing keys into every available context path (model prompt, tool result, memory retrieval, artifact body, event payload). Verify that no path produces key material in model context.

### INV-02: Grants are resolved outside model-controlled text

**Statement:** The `ResolvedGrant` for any Effect is produced by deterministic policy evaluation. Model output may propose an action; it cannot produce, modify, or forge a Grant.

**Enforcement:**
- Grant resolution is a pure function of policy configuration, approvals, and resource state.
- Grants are content-addressed (hashed) and immutable.
- Tool invocations and chain actions reference a specific Grant by digest.

**Acceptance test:** An adversarial model output that claims authorization or modifies grant fields has no effect on the actual ResolvedGrant used for the effect.

### INV-03: Effects are durable before I/O

**Statement:** An `EffectIntent` is durably stored before the corresponding external I/O begins. If the process crashes after creating the intent but before starting I/O, the intent is recovered and processing resumes.

**Enforcement:**
- EffectIntent creation and state transition are in the same database transaction.
- Workers read from the Outbox; they do not receive intents through in-memory channels that could be lost.
- Recovery on restart scans the Outbox for unclaimed or lease-expired intents.

**Acceptance test:** Kill the process at every point in the effect lifecycle. Verify that no EffectIntent is lost, no effect is silently skipped, and no effect is duplicated.

### INV-04: Unknown outcomes are never collapsed to success or failure

**Statement:** When the outcome of an effect cannot be determined (RPC timeout, process crash during observation, ambiguous chain state), the system records `OutcomeStatus::Unknown` and preserves it until fresh, independent evidence resolves the status.

**Enforcement:**
- `Unknown` is a first-class status in EffectOutcome, RunStatus, and all projections.
- No automatic retry converts `Unknown` to `Failure` without evidence.
- UI surfaces display `Unknown` distinctly from `Success`, `Failure`, and `Pending`.
- Chain actions with `Unknown` outcome trigger an investigation workflow, not a silent retry.

**Acceptance test:** Simulate an RPC timeout during transaction broadcast. Verify that the system displays `Unknown`, does not retry the broadcast without explicit evidence, and does not tell the user the action failed or succeeded.

### INV-05: Reputation never authorizes

**Statement:** Reputation scores, ratings, reviews, identity claims, registrar judgments, and display data are informational signals. They may influence discovery, ranking, and display but never affect capability resolution or grant production.

**Enforcement:**
- The `PolicyEvaluator` does not accept reputation as an input to allow/deny decisions.
- Grant resolution has no reputation parameter.
- Tests prove that changing reputation scores has zero effect on resolved grants.

**Acceptance test:** Set an extension's reputation to maximum. Verify that it still cannot exceed its declared capabilities or bypass a required approval.

### INV-06: Least privilege by default

**Statement:** A newly created agent has no tools, filesystem access, network hosts, chain write permissions, signing authority, or economic effects until explicit, time-bounded grants exist. The default for any unconfigured permission is deny.

**Enforcement:**
- Default policy is empty (no allowed effects).
- Resource selectors default to empty (no allowed resources).
- Tools are unavailable until explicitly granted.

**Acceptance test:** Create a new Agent with no policy configuration. Verify that every tool invocation, chain write, and filesystem operation is denied.

### INV-07: Local correctness without cloud dependency

**Statement:** A local/self-hosted Polkagent deployment operates correctly without any cloud control plane. When the control plane is unavailable, the data plane follows locally cached policy and pauses actions outside cached policy scope.

**Enforcement:**
- Policy evaluation uses locally stored policy revisions.
- Effect execution uses local stores and configured providers.
- Control plane unavailability triggers a degradation mode, not a failure mode.
- No grant is widened or silently executed when the control plane is unreachable.

**Acceptance test:** Start a local deployment, perform several runs, disconnect from the control plane, and verify that cached-policy-authorized work continues while out-of-scope work pauses with explicit status.

### INV-08: Durable idempotent effect execution

**Statement:** Effects use stable idempotency keys and durable attempt records. A restart does not repeat an external side effect merely because its parent run resumed. Transport-level retries are distinguished from logical duplicates.

**Enforcement:**
- Every EffectIntent has a stable `IdempotencyKey`.
- Workers check idempotency before performing I/O.
- Attempt records include the idempotency key used.
- External systems that support idempotency keys receive them.

**Acceptance test:** Kill and restart during every phase of an effect lifecycle. Verify that no external effect (model call, chain submission, message delivery) is duplicated.

### INV-09: Tenant and secret isolation

**Statement:** In multi-tenant deployments, no tenant can access another tenant's data, secrets, runs, artifacts, events, policies, or configurations. Secret material is stored in tenant-scoped secret stores and never crosses tenant boundaries.

**Enforcement:**
- All queries include tenant scoping.
- Secret resolvers are tenant-scoped.
- Cross-tenant access attempts fail with authorization errors.
- Queue messages, object paths, encryption keys, and telemetry are tenant-scoped.

**Acceptance test:** In a multi-tenant deployment, attempt cross-tenant data access through every available interface (API, store, bus, projection). Verify denial.

### INV-10: Authoritative state change and effect enqueue are atomic

**Statement:** When a Run's state machine transitions and creates EffectIntents, both operations occur in the same database transaction. There is no window where state has changed but effects are missing, or effects exist but state has not changed.

**Enforcement:**
- The reducer produces `(NewState, Vec<EffectIntent>)` as a single unit.
- Both are written in one SQLite/Postgres transaction.
- No async gap exists between state write and intent write.

**Acceptance test:** Inject crashes between state write and intent write (using transactional guarantees, verify this gap does not exist).

### INV-11: Delivery completion and execution completion are separate states

**Statement:** Delivering a response to a user (through chat, web, API) is independent of the execution state of the Run. A Run may complete execution but fail delivery. A user may receive a response but the Run may still have pending effects.

**Enforcement:**
- Separate delivery tracking with its own status.
- UI reconnect recovers truthful status from durable state, not from delivery status.

### INV-12: Signing, submission, inclusion, finality, failure, and unknown are distinct outcomes

**Statement:** For chain actions, the system distinguishes and reports separately: payload signed, transaction submitted, transaction included in a block, transaction finalized, transaction failed (with error), and outcome unknown.

**Enforcement:**
- Chain action saga has distinct states for each phase.
- Each phase produces its own EffectOutcome.
- UI displays the most recent known phase, not a collapsed success/failure.

### INV-13: Normalized streams have explicit ordering and at most one terminal event

**Statement:** Event streams within a Run have monotonically increasing sequence numbers. A Run has at most one terminal event (Completed, Failed, Cancelled, TimedOut).

### INV-14: Secret material is excluded from model context, logs, events, and extension APIs by default

**Statement:** Data classified as `Sensitive` or `SecretForbidden` does not appear in model prompts, event payloads, log entries, or standard extension API responses without explicit, audited declassification.

### INV-15: Evidence, inference, and model claims are distinguishable

**Statement:** The system never presents model-generated text as if it were verified evidence. Canonical data (decoded calls, metadata hashes, finality observations) is distinguished from model explanation in all projections and surfaces.

---

## 4. Architecture layers

### 4.1 Layer diagram

```
+===========================================================================+
|                           USER AND PRODUCT SURFACES                        |
|  Polkadot Chat | Web/PWA | Studio | Inbox | CLI/TUI | API/SDK | Operator  |
+===========================================================================+
         |                                                    |
         v                                                    v
+===========================================================================+
|                     TRANSPORT AND PRESENTATION ADAPTERS                     |
|  Authentication | Messages | Files | Live Updates | Action Sheets          |
|  Structured Cards | Notifications | Session Management                    |
+===========================================================================+
         |                                                    |
         v                                                    v
+===========================================================================+
|                    DURABLE APPLICATION / RUNTIME SERVICES                   |
|  Conversations | Runs | Orchestration | Context Assembly | Policy          |
|  Approval | Delivery | Projections | Memory | Workflows                   |
+===========================================================================+
         |                                                    |
         v                                                    v
+===========================================================================+
|                          MINIMAL DOMAIN KERNEL                              |
|  Stable IDs | State Machines | Artifacts | Events | Effects | Grants      |
|  Reducer | Outbox | Idempotency | Classification | Provenance             |
+===========================================================================+
         |                                                    |
         v                                                    v
+===========================================================================+
|                       EXECUTION AND PRODUCT PORTS                          |
|  TurnExecutor | HarnessService | ModelProvider | ToolHost | SkillResolver |
|  ChainClient | Signer | PaymentRail | ContextSource | MemoryStore        |
|  IdentityProvider | Feed/Trigger | ExtensionRegistry | SecretResolver     |
|  Clock | EventSink                                                        |
+===========================================================================+
         |                                                    |
         v                                                    v
+===========================================================================+
|                        INFRASTRUCTURE ADAPTERS                              |
|  SQLite | Postgres | Object Store | Queues | RPCs | Wallets | KMS        |
|  Telemetry Exporters | Docker/Systemd | Cloud Services                    |
+===========================================================================+
```

### 4.2 Layer descriptions

#### Surfaces (top)

User-facing presentation layers. Surfaces render Projections and accept user input. They do not contain business logic, policy evaluation, or direct data access. All Surfaces see the same durable state through Projections.

**Responsibility:** rendering, input capture, local UX state, notifications.
**Forbidden:** direct state mutation, policy evaluation, signing, chain access, effect creation.

#### Transport and Presentation Adapters

Adapters that bridge between external communication protocols and the application layer. They handle authentication, message format conversion, file transfer, and live update delivery.

**Responsibility:** protocol translation, authentication, ACK/delivery tracking, session management.
**Forbidden:** business logic, policy evaluation, direct kernel state mutation.

#### Durable Application / Runtime Services

The application layer that implements user-visible product behavior. It manages conversations, runs, orchestration, context assembly, policy evaluation, approval workflows, delivery, and projections.

**Responsibility:** conversation management, run orchestration, context assembly, policy evaluation, approval routing, delivery management, projection computation.
**Forbidden:** direct infrastructure access (uses Ports), direct adapter-specific logic.

#### Minimal Domain Kernel

The stable, minimal core that owns durable domain records and state transitions. The kernel defines the canonical types, state machines, and invariants. It is the smallest layer and changes the least frequently.

**Responsibility:** stable IDs, type definitions, state machine transitions, artifact/event/effect records, grant resolution, classification, provenance, idempotency, outbox management.
**Forbidden:** I/O, network calls, provider-specific logic, transport-specific logic, UI concerns.

#### Execution and Product Ports

Trait-defined boundaries for all external integrations. Each Port defines a narrow contract with specific failure, retry, lifecycle, and authorization semantics.

**Responsibility:** defining the interface contract for each external system type.
**Forbidden:** implementation details (those live in Adapters).

#### Infrastructure Adapters

Leaf implementations of Ports. Each Adapter connects to one external system and implements one Port. Adapters are independently testable, replaceable, and bounded.

**Responsibility:** implementing Port contracts against specific external systems.
**Forbidden:** cross-adapter dependencies, business logic, policy evaluation.

### 4.3 Data flow through the layers

#### Example: user message requesting a DOT transfer

```
1. Surface: User sends "Transfer 10 DOT to Alice" via mobile chat
       |
2. Transport Adapter: Authenticates sender, creates IncomingMessage,
   durably admits before ACK
       |
3. Application Service: Creates Turn and Run, assembles context
   (chain profile, account state, metadata)
       |
4. Kernel: Records Run in Created status with config/policy revision
       |
5. Application Service: TurnExecutor produces structured action proposal
       |
6. Application Service: Validates proposal into typed ChainIntent
       |
7. Port (ChainClient): Decodes metadata, collects read-only/preflight evidence
       |
8. Application Service (PolicyEvaluator): Creates PolicyDecision and ResolvedGrant
       |
9. Application Service (Approval): Records approval requirement,
   renders canonical approval card
       |
10. Surface: Displays approval card with canonical evidence
        |
11. User: Approves via Surface
        |
12. Kernel: Records Approval bound to exact payload/grant/evidence
        |
13. Port (Signer): Receives ONLY canonical payload + binding data, returns signature
        |
14. Port (ChainClient): Submits signed transaction
        |
15. Kernel: Records EffectOutcome (submission receipt)
        |
16. Port (ChainClient): Watches for finality
        |
17. Kernel: Records EffectOutcome (finality observation)
        |
18. Application Service: Creates receipt artifact, delivery
        |
19. Surface: Displays truthful final status via Projection
```

**Critical observation:** At no point is a model sentence such as "the transfer succeeded" authoritative. Every status comes from durable records with evidence.

---

## 5. Dependency rules

### 5.1 Dependency direction

Dependencies flow strictly downward through the architecture layers. Upper layers depend on lower layers; lower layers never depend on upper layers.

```
Surfaces                  --> Transport Adapters (through interfaces only)
Transport Adapters        --> Application Services
Application Services      --> Kernel, Ports
Kernel                    --> (no dependencies outside std/core crates)
Ports                     --> Kernel types (for parameter/return types)
Infrastructure Adapters   --> Ports, Kernel types
```

### 5.2 Forbidden dependencies

| From | To | Why forbidden |
|---|---|---|
| Kernel | Any Adapter | Kernel must be testable without I/O |
| Kernel | Any Provider/Harness/Transport crate | Kernel must not import vendor-specific logic |
| Any Adapter | Another Adapter | Adapters are independent leaves; only the kernel orchestrates across ports. Cross-adapter calls are forbidden unconditionally (see AP-06). |
| Application Services | Infrastructure details | Use Ports; never import SQLite/Postgres/wallet crates directly |
| Surfaces | Kernel directly | Surfaces use Projections, not raw kernel state |
| Marketplace/Extension | Signer/Chain write | Extensions cannot grant themselves signing or chain write |
| Control Plane | Data Plane secrets | Control plane manages; it does not access private data plane content |
| Policy Evaluator | Reputation | Reputation is informational; it never affects grant resolution (INV-05) |

### 5.3 Circular dependency prohibition

No circular dependencies exist between crates in the workspace. The `cargo` workspace enforces this at compile time. If a circular dependency is detected, it is a bug to be resolved by extracting shared types into a lower-level crate.

### 5.4 Port stability rule

Port traits (in the `polkagent-ports` crate) change infrequently. Adding a method to a Port trait is a semver-minor change requiring adapter updates. Removing or changing a Port method signature is a semver-major change requiring a migration plan.

---

## 6. Rust workspace layout

### 6.1 Proposed crate structure

```
polkagent/
  Cargo.toml                          # workspace root

  crates/
    polkagent-types/                   # Stable IDs, enums, core type definitions
      src/
        ids.rs                         # RunId, ArtifactId, EffectIntentId, etc.
        artifact.rs                    # Artifact, ArtifactKind, Classification
        event.rs                       # RunEvent, EventKind, Durability
        effect.rs                      # EffectIntent, EffectAttempt, EffectOutcome
        grant.rs                       # ResolvedGrant, Limits, ResourceSelector
        policy.rs                      # Policy, PolicyRule, PolicyDecision
        approval.rs                    # Approval, ApprovalKind
        agent.rs                       # AgentSpec, ProductSpec
        chain.rs                       # ChainProfile, ChainIntent, PaymentIntent
        config.rs                      # Configuration types
        provenance.rs                  # Provenance, ProvenanceSource
        error.rs                       # Domain error types

    polkagent-kernel/                  # Minimal domain kernel: state machines, reducers
      src/
        reducer.rs                     # TurnReducer: (State, Input) -> (State, Effects)
        state_machine.rs               # Run lifecycle state machine
        outbox.rs                      # Durable outbox management
        idempotency.rs                 # Idempotency key generation and checking
        grant_resolver.rs              # Deterministic grant resolution
        classification.rs              # Data classification enforcement
        digest.rs                      # Content hashing and verification

    polkagent-ports/                   # Port trait definitions
      src/
        executor.rs                    # TurnExecutor trait
        harness.rs                     # HarnessService trait
        provider.rs                    # ModelProvider trait
        tool.rs                        # ToolHost trait
        transport.rs                   # Transport trait
        chain.rs                       # ChainClient trait
        signer.rs                      # Signer trait
        payment.rs                     # PaymentRail trait
        store.rs                       # RunStore, ArtifactStore, EventStore, EffectStore
        context.rs                     # ContextSource, ContextAssembler traits
        memory.rs                      # MemoryStore trait
        identity.rs                    # IdentityProvider trait
        trigger.rs                     # Feed, Trigger traits
        extension.rs                   # ExtensionRegistry trait
        secret.rs                      # SecretResolver trait
        clock.rs                       # Clock trait (for testing)
        telemetry.rs                   # EventSink trait

    polkagent-app/                     # Application services layer
      src/
        conversation.rs                # Conversation management
        run_orchestrator.rs            # Run lifecycle orchestration
        context_assembler.rs           # Pure context assembly
        policy_evaluator.rs            # Deterministic policy evaluation
        approval_service.rs            # Approval routing and tracking
        delivery_service.rs            # Response delivery management
        projection_builder.rs          # Projection computation
        workflow_engine.rs             # Optional workflow/graph orchestration (phased)
        memory_service.rs              # Memory admission and retrieval

    polkagent-cli/                     # CLI application
      src/
        main.rs
        commands/
        tui/

    polkagent-server/                  # HTTP/WebSocket API server
      src/
        main.rs
        api/
        auth/
        websocket/

  adapters/
    adapter-store-sqlite/              # SQLite implementation of store ports
    adapter-store-postgres/            # Postgres implementation (phased)
    adapter-provider-anthropic/        # Anthropic API provider
    adapter-provider-openai/           # OpenAI-compatible provider
    adapter-provider-local/            # Local model provider
    adapter-executor-direct/           # Direct model executor
    adapter-executor-coding/           # Coding harness executor (Claude Code, etc.)
    adapter-transport-polkadot-chat/   # PCA-compatible Polkadot chat transport
    adapter-transport-websocket/       # WebSocket transport
    adapter-transport-http/            # HTTP API transport
    adapter-chain-subxt/               # Subxt-based chain client
    adapter-signer-external/           # External wallet signer
    adapter-signer-proxy/              # Proxy/multisig signer
    adapter-telemetry-otel/            # OpenTelemetry exporter

  extensions/
    # Future: WASM and process extensions

  tests/
    contract-tests/                    # Port contract test suites
      executor-contracts/
      harness-contracts/
      provider-contracts/
      transport-contracts/
      chain-contracts/
      signer-contracts/
      store-contracts/
    integration-tests/                 # Cross-crate integration tests
    fixture-corpus/                    # Test fixtures and evidence corpora
```

### 6.2 Crate responsibilities

| Crate | Responsibility | May depend on |
|---|---|---|
| `polkagent-types` | Stable type definitions, IDs, enums, serialization | `std`, `serde`, `uuid` |
| `polkagent-kernel` | State machines, reducers, outbox, grant resolution, classification | `polkagent-types` |
| `polkagent-ports` | Port trait definitions | `polkagent-types` |
| `polkagent-app` | Application services, orchestration, policy, context, delivery | `polkagent-types`, `polkagent-kernel`, `polkagent-ports` |
| `polkagent-cli` | CLI entry point and TUI | `polkagent-app`, all configured adapters |
| `polkagent-server` | API server entry point | `polkagent-app`, all configured adapters |
| `adapter-*` | Individual port implementations | `polkagent-types`, `polkagent-ports`, external vendor crates |

### 6.3 Dependency graph

```
polkagent-types          (zero platform dependencies)
      |
      +--- polkagent-kernel     (depends only on types)
      |         |
      +--- polkagent-ports      (depends only on types)
      |         |
      +--- polkagent-app        (depends on types, kernel, ports)
      |         |
      |    +--- polkagent-cli   (depends on app + selected adapters)
      |    +--- polkagent-server (depends on app + selected adapters)
      |
      +--- adapter-*            (each depends on types + ports + vendor crate)
```

**Key property:** `polkagent-types` and `polkagent-kernel` have zero dependencies on providers, transports, chains, wallets, or any external service. They can be tested with `cargo test` and no network.

#### 6.3.1 Compile-time and generics discipline

A large Rust workspace with trait-heavy generic code can produce prohibitively long compile times through monomorphization. The following rules mitigate this:

- **`polkagent-types` must remain dependency-light.** No `tokio`, no `async-std`, no heavy vendor crates. Dependencies are limited to `std`, `serde`, `uuid`, and similar zero-cost serialization crates. This keeps incremental compilation of the bottom of the dependency graph fast.
- **Port boundaries use `dyn`.** The Port traits in `polkagent-ports` and their usage sites in `polkagent-app` use `dyn Trait` (dynamic dispatch) rather than `impl Trait` generics. This avoids monomorphization of the application service layer across all adapter combinations. The cost is one pointer indirection per port call, which is negligible compared to the I/O latency of any actual port operation.
- **Hot paths may use `impl Trait` / concrete types.** Reducers, state machine transitions, and content hashing (inner loops) may use concrete types or `impl Trait` where benchmarks show the indirection overhead is measurable.
- **Adapter crates are compiled independently.** Each `adapter-*` crate only pulls in the vendor crate it wraps, so a change to `adapter-chain-subxt` does not trigger recompilation of `adapter-provider-anthropic`.

### 6.4 Acceptance criteria for workspace layout

1. `polkagent-kernel` compiles and passes all tests with no network access.
2. Each adapter crate compiles independently and passes its contract test suite.
3. Adding a new provider, transport, or chain adapter requires zero changes to `polkagent-kernel` or `polkagent-types`.
4. The full workspace has no circular dependencies (`cargo build` enforces this).
5. `polkagent-types` has no dependency on `async-trait`, `tokio`, or any async runtime.
6. Clean build time of `polkagent-kernel` alone does not exceed a documented baseline; violations are a build regression.

---

## 7. Trust boundaries

### 7.1 Trust boundary diagram

```
+================================================================+
|                    UNTRUSTED BOUNDARY                            |
|                                                                  |
|  User input | Public senders | Web content | Chat attachments   |
|  Model responses | Harness output | Extension output            |
|  Marketplace packages | Retrieved memory | RPC responses        |
|  Provider responses | Remote workers | Webhook payloads         |
+================================================================+
         |  (validation, classification, sanitization)
         v
+================================================================+
|                    MEDIATED BOUNDARY                             |
|                                                                  |
|  Transport Adapters (authenticated, rate-limited)               |
|  Tool invocations (grant-checked, sandboxed)                    |
|  Extension calls (capability-scoped, resource-limited)          |
|  Chain reads (profile-bound, metadata-pinned)                   |
|  Memory retrieval (classified, attributed, budget-limited)      |
+================================================================+
         |  (policy evaluation, grant resolution)
         v
+================================================================+
|                    TRUSTED KERNEL                                |
|                                                                  |
|  State machines | Reducers | Grant resolution                   |
|  Outbox | Idempotency | Classification enforcement              |
|  Artifact integrity | Event ordering                            |
+================================================================+
         |  (canonical payload, binding data only)
         v
+================================================================+
|                    ISOLATED SIGNING BOUNDARY                     |
|                                                                  |
|  Signer receives: canonical bytes + account ref +               |
|    chain profile + metadata hash + grant digest + approval ID   |
|  Signer never receives: model context, prompts, conversation,  |
|    tool results, memory, extension data                         |
+================================================================+
```

### 7.2 Trust boundary rules

| Boundary | What crosses it | What is blocked |
|---|---|---|
| **Untrusted -> Mediated** | Validated, classified, rate-limited inputs | Raw injection, oversized payloads, unauthenticated access |
| **Mediated -> Kernel** | Typed domain commands, policy decisions | Direct state mutation, unvalidated effects |
| **Kernel -> Signer** | Canonical payload bytes and binding references | Model context, conversation data, tool results, memory |
| **Adapter -> Adapter** | Nothing. Adapters do not communicate directly. | Cross-adapter dependencies |
| **Extension -> Kernel** | Declared capability requests, typed results | Direct state mutation, grant modification, signer access |
| **Control Plane -> Data Plane** | Policy distribution, fleet management commands | Direct data access, secret access, conversation content |

### 7.3 Threat model summary

The design assumes that any of the following can be wrong, malicious, compromised, stale, or misleading:

- A user or public sender.
- A model response or coding harness output.
- Retrieved web content, repository text, chat attachment, or memory.
- A marketplace extension, tool, skill, or update.
- A model provider, remote worker, RPC endpoint, indexer, or chain fork.
- A transport or notification service.
- A cloud tenant, administrator, support workflow, or neighboring workload.
- A wallet integration or signer response.
- A displayed asset name, network name, simulation, fee estimate, or finality observation.

The system therefore uses: authenticated principals, data classification, content provenance, typed intents, deterministic policy, exact grants, sandboxed/mediated effects, isolated signers, pinned chain identity and metadata, independent outcome observation, tenant isolation, and durable audit.

---

## 8. Extension safety model

### 8.1 Extension types and trust tiers

Extensions are versioned executable or service integrations that add capabilities to Polkagent. They operate at different trust tiers:

| Trust tier | Examples | Isolation | Available |
|---|---|---|---|
| **Built-in** | Core tools, default skills | In-process, full kernel trust | Phase 0 |
| **Process/RPC** | MCP servers, coding harnesses, external services | Process boundary, network isolation | Phase 0 |
| **WASM/Component** | Sandboxed computation extensions | Wasmtime Component Model + WIT interfaces; fuel + epoch metering; capability-scoped host calls | Validate-next |
| **Remote Service** | Third-party API services | Network boundary, timeout, capability scoping | Phase 1 |

#### Plugin isolation rationale

**Proposal:** WASM/Component-tier plugins use the **Wasmtime Component Model** with **WIT (WASM Interface Types)** interface definitions. This is chosen over OS-level process isolation for two reasons: (1) WIT interfaces are capability-typed, so a plugin can only call host functions that are explicitly included in its component interface — the sandbox boundary is part of the type system; (2) components are portable across operating systems and architectures without requiring container runtimes or OS-specific sandboxing primitives. Fuel and epoch metering enforce CPU time limits in-process.

Process isolation (Phase 0 Process/RPC tier) remains the deployment model for MCP servers and harnesses that have their own process lifecycle. The Component tier is for plugins distributed as WASM artifacts that run inside the Polkagent host process with capability-scoped access.

### 8.2 Extension manifest

Every extension (beyond built-ins) declares a manifest:

```rust
struct ExtensionManifest {
    id: ExtensionId,
    name: String,
    version: SemVer,
    publisher: PublisherId,
    host_protocol_version: SemVer,
    entrypoint: Entrypoint,
    capabilities: Vec<Capability>,
    config_schema: JsonSchema,
    resource_limits: ResourceLimits,
    integrity: IntegrityData,
    declared_outputs: Vec<OutputDeclaration>,
    license: LicenseRef,
}

struct ResourceLimits {
    max_memory_bytes: Option<u64>,
    max_cpu_time_ms: Option<u64>,
    max_network_bytes: Option<u64>,
    max_filesystem_bytes: Option<u64>,
    max_concurrent_requests: Option<u32>,
}

struct IntegrityData {
    content_digest: ContentDigest,
    signature: Option<Signature>,
    build_provenance: Option<BuildProvenance>,
}
```

### 8.3 Extension lifecycle

```
Published to Registry
  -> Discovered in Marketplace/Registry search
  -> Manifest inspected: capabilities, data access, effects, cost
  -> Installed: version pinned, dependencies locked, digest verified
  -> Activated in a deployment: capabilities granted separately
  -> Runtime: invoked under ResolvedGrant, resource-limited
  -> Updated: staged, tested, rollback available
  -> Revoked: deactivated, cleanup, security advisory
```

**Critical rule:** Discovery does not install. Installation does not activate. Activation does not grant capabilities. Each transition is explicit.

### 8.4 Extension sandbox enforcement

| Constraint | Enforcement mechanism |
|---|---|
| Capability scope | Extension receives only resources matching its ResolvedGrant |
| Memory/CPU | WASM fuel limits; process cgroup/rlimits |
| Network | Allowed hosts only; proxy through host |
| Filesystem | Allowed roots only; chroot/sandbox |
| Time | Deadlines on all calls; lease expiry |
| Secret access | SecretForbidden classification; extension API excludes secrets |
| Effect creation | Extensions propose effects; kernel validates and schedules |
| State mutation | Extensions cannot directly write kernel state |

### 8.5 Acceptance criteria for extension safety

1. An extension cannot exceed its declared capabilities regardless of its runtime behavior.
2. An extension cannot access another extension's data or resources.
3. A WASM extension that exceeds fuel/memory/time limits is terminated cleanly.
4. Revoking an extension immediately prevents further invocation and triggers cleanup.
5. An extension update cannot silently change its capability requirements.

---

## 9. Configuration architecture

### 9.1 Configuration layers

Configuration follows a strict precedence order:

```
defaults < file < environment < CLI flags
```

**Rules:**
- Unknown keys are rejected (fail-closed, not silently ignored).
- Each run records an effective config snapshot/digest.
- Hot reload may change settings safe for future turns; it cannot retroactively change an in-flight approval, resolved grant, or canonical chain payload.
- Only secret handles appear in run records, never raw secret values.

#### Content-addressed configuration snapshots

Configuration is loaded via a **pull-based, content-addressed** mechanism: the effective configuration for any point in time is stored as an immutable snapshot identified by its content hash (`ConfigDigest`). Changing a configuration value produces a new content hash, creating a new snapshot. This means:

- Every Run's `config_revision` field is an auditable pointer to an exact, reproducible configuration snapshot.
- Rolling back configuration is equivalent to pointing at a previous content-addressed snapshot.
- Hot reload is implemented by loading a new snapshot; in-flight Runs continue to reference the snapshot they started with.
- Diff between two configuration states is a diff between two content-addressed snapshots — there is no mutation of existing snapshots.

This property is already implied by `Run.config_revision` being immutable for a Run's lifetime; the content-addressed snapshot model makes it operationally concrete.

### 9.2 Configuration schema

```yaml
apiVersion: polkagent.dev/v1alpha1
kind: Agent
metadata:
  name: <agent-name>

spec:
  execution:
    route: <executor-or-harness-ref>
    model: <model-ref>
    budget:
      modelUsdPerRun: <amount>
      maxTurnsPerConversation: <count>

  surfaces:
    - type: web_inbox
    - type: polkadot_chat
      profile: <chat-profile-ref>
    - type: cli

  skills:
    - package: <registry/skill-name>
      version: <semver>

  tools:
    - id: <tool-id>
      config: <tool-specific-config>

  chainProfiles:
    - ref: <profile-name>

  memory:
    episodic: { enabled: true, retention: 90d }
    semantic: { enabled: false }

  autonomy:
    mode: <observe | prepare | per_action | session | policy_autonomous | fully_autonomous>
    accounts:
      - ref: <account-ref>
    allow:
      - actionFamily: <family>
        networks: [<profile-refs>]
        budgets:
          perAction: <amount>
          rolling24h: <amount>
    deny:
      - actionFamily: <family>
    onUnknownOutcome: <pause | alert | retry_with_evidence>
    circuitBreakers:
      failuresPerHour: <count>

  signer:
    type: <external | hardware | proxy | local_encrypted | remote_org | managed_kms | mpc>
    ref: <signer-ref>

  policy:
    ref: <policy-ref>
    overrides: <inline-policy-rules>

  deployment:
    target: <local | self_hosted | managed>
    region: <optional-region>
```

### 9.3 Configuration validation

All configuration is validated against a versioned JSON Schema before acceptance. Invalid configuration is rejected with specific error messages naming the invalid field, expected type, and constraint violated.

### 9.4 Configuration versioning

Configuration schemas are versioned with `apiVersion`. Migration between versions follows explicit rules with documented breaking changes.

### 9.5 Acceptance criteria for configuration

1. An unknown key in configuration causes a clear validation error.
2. Every run's effective configuration can be reconstructed from its stored `config_revision`.
3. Hot reload of provider endpoints does not affect in-flight grant resolution.
4. Configuration validation catches: missing required fields, type mismatches, invalid references, budget/limit constraint violations.

---

## 10. Cross-cutting concerns

### 10.1 Telemetry and observability

**Architecture:** Telemetry is a projection, not a mutable runtime. All observability data derives from durable Events and Artifacts.

| Signal | Source | Sensitivity |
|---|---|---|
| Metrics | Event counts, latencies, error rates, usage | Tenant-scoped, no PII |
| Traces | Correlation/causation IDs in Events | Tenant-scoped, may reference private data by ID |
| Logs | Structured event summaries | Classified; secrets and private content redacted |
| Audit | Effect lifecycle, grants, approvals, outcomes | Tenant-scoped, durable, tamper-evident |

**Port:** `EventSink` receives classified event summaries. Adapter implementations include OpenTelemetry, structured log files, and audit-specific stores.

**Invariant:** Telemetry never contains raw secrets, unredacted PII, or model context beyond token counts and classified references.

### 10.2 Error handling

**Error taxonomy:**
```rust
enum DomainError {
    /// Input validation failed; client can fix and retry.
    Validation { field: String, message: String },
    /// Required resource not found.
    NotFound { resource_type: String, id: String },
    /// Authorization denied by policy.
    PolicyDenied { policy_revision: PolicyDigest, reason: String },
    /// External system error; may be transient.
    External { system: String, error: String, retryable: bool },
    /// Internal invariant violation; indicates a bug.
    Internal { message: String },
    /// Operation timed out.
    Timeout { operation: String, deadline: Timestamp },
    /// Operation cancelled by user or policy.
    Cancelled { reason: CancelReason },
}
```

**Rules:**
- Errors describe what happened, what remains safe, and the next recovery action.
- Internal errors are logged with full context but presented to users without sensitive internals.
- External errors preserve the upstream error for operators while presenting user-friendly summaries.
- `Unknown` is not an error -- it is a legitimate outcome state.

### 10.3 Versioning strategy

| Artifact | Versioning scheme | Stability expectation |
|---|---|---|
| AgentSpec/ProductSpec | Semantic versioning (SemVer) | Breaking changes require major version |
| Port traits | SemVer in crate version | Minor for additive changes; major for breaking |
| Configuration schema | `apiVersion` field | Migration rules between versions |
| Wire protocols / API | URL-versioned (`/v1/`, `/v2/`) | Deprecation period before removal |
| Marketplace packages | SemVer with lockfiles | Pinned versions; staged updates |
| Database schema | Sequential migrations | Forward-only; rollback requires backup |
| Events | Schema version in event type | Readers handle known versions; unknown versions are preserved |

---

## 11. Design patterns from Roko synthesis

These patterns are adopted or adapted from the Roko research and subsequent architecture research. Each includes the decision, rationale, and implementation shape.

### 11.1 Adopted patterns (do-now)

| Pattern | Decision | Implementation shape |
|---|---|---|
| **Artifact/event split** | Adopt now | SQLite artifact + ordered run-event tables; lossy live stream |
| **Single writer + reducer + outbox** | Adopt now | Per-run actor, pure `(State, Input) -> (State, Vec<EffectIntent>)` reducer, transactional outbox, leased workers |
| **Event sourcing + CQRS** | Adopt now | Event log is authoritative; projections and read models are derived; reducer is replay-safe |
| **Chain saga** | Adopt now for write-capable flows | Separate decode/simulate/approve/sign/broadcast/finality states |
| **Capability intersection** | Adopt now | Immutable `ResolvedGrant`, deny-by-default resource selectors |
| **Cedar policy-as-code** | Adopt now | Cedar engine behind `PolicyEvaluator`; PARC model; schema-validated policies; deny-by-default |
| **Classified information flow** | Adopt now | Four classes, inherited restriction, secret handles |
| **Narrow adapter ports + no-cross-talk** | Adopt now | Separate executor, harness, tool, transport, chain client, signer; kernel is the sole orchestrator |
| **Pure context pack** | Adopt now | Attributed inputs/exclusions and digest |
| **Fixed gateway pipeline** | Adopt now | Named stages and typed short-circuits |
| **Versioned projections** | Adopt now | Recoverable API/UI read models |
| **Content-addressed config snapshots** | Adopt now | Pull-based; changing config = new content hash = auditable; hot reload = new snapshot pointer |
| **BLAKE3 content addressing** | Adopt now | BLAKE3 for internal artifacts; SHA-256 where interop requires (Sigstore, IPFS) |
| **Kernel+types+ports skeleton** | Adopt now (initial milestone) | Build `polkagent-types`, `polkagent-kernel`, `polkagent-ports` as dependency-light stubs first |
| **Architecture fitness functions in CI** | Adopt now | Assert no adapter-to-adapter deps; no key material reachable from provider/tool crates; policy gate on every effect path |

### 11.2 Adapted patterns (validate-next)

| Pattern | Decision | Trigger for adoption |
|---|---|---|
| **WIT plugin host** | Validate next | Wasmtime Component Model host; WIT interface definitions for tool plugins; fuel + epoch metering confirmed |
| **Fitness function suite** | Validate next | Full set of structural assertions in CI; regression baselines established |
| **Triggers/watchers** | Adapt after core | Cursor-based ingress and prepare-only chain watches |
| **Workflow graph** | Adapt later | Two or more real workflows share declarative representation |
| **Evidence promotion/memory** | Adapt later | Redacted episodes and reviewable candidate knowledge |
| **Workspaces/evals/witness** | Adapt later | Cross-workspace isolation tests pass |

### 11.3 Deferred patterns

| Pattern | Reconsidered when |
|---|---|
| **WASM plugin marketplace** | Validate-next WIT host passes capability/resource/revocation tests |
| **Marketplace/payments/autonomy** | Commercial, custody, governance, security PRDs pass gates |
| **Multi-agent groups** | Membership, mandates, shared budgets, cancellation designed |
| **On-chain evidence anchoring** | Hash-only, user-consented, retention policies explicit |

### 11.4 Rejected patterns

| Pattern | Why rejected |
|---|---|
| **Universal Cells/Graphs as v1** | Hides the small state machine users must trust |
| **Resident cognitive loops** | Unclear authority, cost, and stop conditions |
| **HDC, demurrage, affect, dreams** | Speculative cognitive metaphors; not user safety features |
| **Self-authored authority expansion** | An agent cannot grant itself new tools, policy, or spending |
| **Generic hook chains** | Makes policy order and extension authority difficult to audit |
| **Native third-party plugins** | Supply-chain and host-compromise risk without sandboxing |
| **LLM-as-sole-verifier** | Model classification alone cannot be the safety boundary |
| **Weighted aggregate safety score** | A denial cannot be compensated by other points |
| **OPA/Rego sidecar for policy** | Requires sidecar process or CGo bindings; adds operational overhead for embedded/local-first target; Cedar provides equivalent deny-by-default semantics in-process |
| **Cross-adapter calls** | Violates hexagonal no-cross-talk rule; makes audit and replacement impossible |
| **Secrets in agent-reachable crates** | INV-01, INV-14 require that no code path from model context reaches signing material; placing secrets in provider or tool crates creates such paths |

---

## 12. Anti-patterns

The following approaches are explicitly forbidden in Polkagent implementation:

### AP-01: Model prose as authority
Using model-generated text as the source of truth for action status, chain state, account balance, or approval status. All authoritative data comes from typed records with evidence.

### AP-02: Generic `send_transaction` tool
A single generic tool that submits arbitrary chain transactions without family-specific evidence, simulation, policy, and approval. Each action family requires its own safety boundary.

### AP-03: Collapsing Provider/Model/Executor/Harness
Treating a model name string as if it were a complete execution environment. Each has different lifecycle, retry, authorization, and failure semantics.

### AP-04: Mutable grants
Modifying a ResolvedGrant after creation. Grants are immutable snapshots. Policy changes produce new grants for future effects.

### AP-05: Secret logging
Including raw API keys, signing keys, seed phrases, or unredacted tokens in logs, events, artifacts, telemetry, or model context.

### AP-06: Adapter cross-talk
One adapter depending on or communicating with another adapter. Adapters are independent leaves.

### AP-07: Reputation-based authorization
Using reputation scores, ratings, or reviews as inputs to grant resolution or policy evaluation.

### AP-08: Silent retry after unknown
Automatically retrying a chain submission after an `Unknown` outcome without fresh evidence that the original transaction was not included.

### AP-09: Delivery as completion
Treating "message delivered to user" as equivalent to "run completed" or "effect succeeded." These are independent states.

### AP-10: Extension self-activation
Allowing an extension to activate itself, grant itself capabilities, or modify its own manifest at runtime.

### AP-11: Model fallback during effect
Automatically switching models after a tool, signature, or chain effect has begun. A fallback creates a new attempt with new evidence.

### AP-12: Cloud as authority
A managed control plane silently reading private data plane content, reinterpreting local policy, or accessing signing material.

---

## 13. Architecture decision records (ADRs)

### 13.1 ADR template

Every significant architecture decision should be recorded using this template:

```markdown
# ADR-NNN: <Title>

**Status:** proposed | accepted | rejected | superseded by ADR-XXX
**Date:** YYYY-MM-DD
**Deciders:** <names>

## Context
What is the issue that we are seeing that motivates this decision?

## Decision
What is the change that we are proposing?

## Consequences
What becomes easier or more difficult because of this change?

## Alternatives considered
What other options were evaluated and why were they rejected?

## Acceptance criteria
How do we verify this decision was correctly implemented?
```

### 13.2 Key architecture decisions

#### ADR-001: Rust-first with bounded TypeScript

**Status:** accepted
**Context:** Polkagent needs a primary implementation language. The platform handles signing, policy, chain operations, and security-critical state machines.
**Decision:** Rust for the kernel, application services, CLI, API server, adapters, and SDKs. TypeScript for browser UI, Polkadot Product SDK/PAPI integration, and generated clients.
**Consequences:** Strong type safety and memory safety for security-critical paths. Higher initial development cost. Language boundary uses versioned protocols and schemas.
**Acceptance:** All kernel, policy, and signing code is Rust. TypeScript is limited to surfaces and ecosystem integration.

#### ADR-002: SQLite-first local storage

**Status:** accepted
**Context:** Local deployment must work without external database infrastructure.
**Decision:** SQLite as the default local store. Postgres as an alternative for managed/team deployments. Both implement the same Store port traits.
**Consequences:** Simple local deployment. Single-writer constraint shapes the actor model. Postgres adds concurrent access.
**Acceptance:** Local deployment starts with `polkagent init` and requires no database setup.

#### ADR-003: Outbox pattern for effect durability

**Status:** accepted
**Context:** External effects (model calls, chain submissions, signing) must not be lost on crash and must not be duplicated on restart.
**Decision:** Use the transactional outbox pattern: state change and effect intents are committed atomically; workers claim and execute from the outbox with leases and idempotency keys.
**Consequences:** Crash-safe effect execution. Added complexity in worker lifecycle. Clear retry and duplicate handling.
**Acceptance:** Kill/restart tests at every lifecycle point show zero lost and zero duplicated effects.

#### ADR-004: Deny-by-default capability model

**Status:** accepted
**Context:** Agents, tools, and extensions need permissions. The safe default must be minimal.
**Decision:** Empty default permissions. All capabilities explicitly granted. Grant resolution uses intersection of all applicable policies.
**Consequences:** New agents start with no capabilities. Every tool, filesystem, network, and chain access is explicitly allowed. Slightly higher initial configuration.
**Acceptance:** New agent with no policy has all tool, chain, and filesystem operations denied.

#### ADR-005: Separate adapter crates

**Status:** accepted
**Context:** Provider, transport, chain, and signer implementations should be independently testable and replaceable.
**Decision:** Each adapter is a separate crate implementing one Port trait. No adapter depends on another adapter.
**Consequences:** Clear boundaries. Easy to add new adapters. Each adapter has its own dependency tree.
**Acceptance:** Adding a new provider adapter requires zero changes to kernel, ports, or other adapters.

#### ADR-006: Projections as the UI truth source

**Status:** accepted
**Context:** Multiple surfaces (CLI, web, mobile, operator) must show consistent state.
**Decision:** Surfaces consume versioned Projections derived from durable state. Projections are recoverable from stored events and artifacts.
**Consequences:** UI reconnect is reliable. No "ghost state" from in-memory-only data. Added latency for projection computation.
**Acceptance:** Kill and restart the server; all surfaces recover correct state.

#### ADR-007: Cedar as the policy evaluation engine

**Status:** proposed
**Context:** The `PolicyEvaluator` requires a deterministic, deny-by-default, formally verified evaluation engine. Candidates are Cedar (Rust-native, in-process), OPA/Rego (sidecar or CGo), and custom Rust evaluation.
**Decision:** Use Cedar. Cedar is Rust-embeddable, formally verified in Lean, evaluates policies in sub-millisecond time in-process, and uses a PARC (Principal, Action, Resource, Context) model that maps directly to `ResolvedGrant` parameters. Schema validation catches policy errors before any Run is created.
**Consequences:** No sidecar dependency. Policy files are Cedar documents compiled and validated at configuration load time. The `PolicyRule` Rust types defined in Section 2.5 are the Polkagent domain vocabulary; Cedar is the execution format. Teams that know OPA/Rego will need to learn Cedar syntax.
**Alternatives considered:** OPA/Rego requires a sidecar process (adds operational overhead for local-first deployment) or CGo bindings (breaks `no_std` portability goals). Custom Rust evaluation lacks formal verification and would require maintaining a complete policy semantics implementation.
**Acceptance:** Policy schema validation rejects malformed Cedar documents at startup. A policy that should deny an action cannot be circumvented by model output. Deny-by-default: a freshly loaded policy with no rules denies all actions.

#### ADR-008: Wasmtime Component Model for plugin isolation

**Status:** proposed
**Context:** The WASM/Component plugin tier (Section 8.1) needs a host runtime and an interface definition mechanism that enforces capability typing at the sandbox boundary.
**Decision:** Use Wasmtime as the WASM host runtime with the Component Model and WIT (WASM Interface Types) for interface definitions. Fuel and epoch metering enforce CPU time limits.
**Consequences:** Plugins can only call host functions explicitly included in their WIT component interface — the capability scope is part of the type system, not a runtime check. Components are portable across OSes without container runtimes. The Component Model is still maturing; this tier is gated behind a validate-next milestone.
**Alternatives considered:** OS process isolation (used for the Process/RPC tier) is appropriate for MCP servers and harnesses with their own lifecycle, but does not provide typed capability interfaces or portable distribution as WASM artifacts.
**Acceptance:** A WASM component that does not declare a host function in its interface cannot call it at runtime. Fuel exhaustion terminates the component cleanly without affecting the host process. A component update cannot silently expand its interface without manifest review.

---

## 14. Verification criteria

### 14.1 Architecture compliance checks

Each invariant has an automated verification:

| Invariant | Verification method |
|---|---|
| INV-01 (No keys in model context) | Static analysis: no code path from secret store to model context. Fuzzing: inject secret-like strings into all inputs. |
| INV-02 (Grants outside model text) | Unit test: grant resolution is a pure function of policy/config, not model output. |
| INV-03 (Effects durable before I/O) | Kill test: crash at every lifecycle point; verify no lost intents. |
| INV-04 (Unknown preserved) | State machine test: no transition from Unknown to Success/Failure without evidence. |
| INV-05 (Reputation never authorizes) | Property test: varying reputation has zero effect on resolved grants. |
| INV-06 (Least privilege) | Integration test: default agent has all operations denied. |
| INV-07 (Local correctness) | Disconnection test: local deployment operates correctly offline. |
| INV-08 (Idempotent effects) | Kill/restart test: no duplicate external effects. |
| INV-09 (Tenant isolation) | Cross-tenant access test: all cross-tenant queries denied. |
| INV-10 (Atomic state+effect) | Transaction test: state and effects written in same transaction. |
| INV-11 (Delivery != execution) | State test: run completes with failed delivery; delivery succeeds with pending run. |
| INV-12 (Distinct chain outcomes) | State machine test: each chain phase has independent outcome. |

### 14.2 Dependency rule verification

Automated in CI:

```bash
# Verify kernel has no adapter dependencies
cargo tree -p polkagent-kernel | grep -c "adapter-" # must be 0

# Verify no circular dependencies
cargo build 2>&1 | grep -c "cyclic" # must be 0

# Verify types crate has no async runtime dependency
cargo tree -p polkagent-types | grep -c "tokio\|async-std" # must be 0
```

### 14.3 Port contract test suites

Every Port has a contract test suite that any Adapter must pass:

| Port | Contract tests |
|---|---|
| `TurnExecutor` | Start, cancel, streaming events, terminal states, error handling |
| `HarnessService` | Start, stop, health, status, capability discovery, timeout |
| `Transport` | Receive, ACK, send, authentication, rate limiting, reconnect |
| `ChainClient` | Resolve profile, prepare action, submit, watch finality, metadata pinning |
| `Signer` | Describe capabilities, sign canonical request, reject invalid requests |
| `ToolHost` | Invoke with grant, deny without grant, respect resource limits |
| `RunStore` | CRUD, idempotency, transaction semantics, concurrent access |
| `ArtifactStore` | Store, retrieve, verify digest, classification enforcement |
| `EventStore` | Append, query by run, ordering guarantees |
| `EffectStore` | Create intent, claim, record attempt, record outcome, lease expiry |

### 14.4 Architecture fitness functions

Continuous assertions executed in CI to verify structural invariants. These are not optional quality metrics; a failing fitness function blocks merge.

| Metric | Target | Alert threshold |
|---|---|---|
| Kernel crate compile time | Baseline | >2x baseline |
| Kernel crate dependency count | <10 | >15 |
| Port trait method count (per trait) | <10 | >15 |
| Adapter crate count | Tracks integration needs | N/A |
| Contract test pass rate | 100% | <100% |
| Cross-crate dependency violations | 0 | >0 |
| Adapter-to-adapter import edges | 0 | >0 |
| Policy gate present on every effect path | 100% of effect creation sites | Any ungated site |
| Key material reachable from provider/tool crates | 0 reachable paths | Any reachable path |

**Structural assertions (implemented as CI steps):**

```bash
# No adapter-to-adapter dependencies
cargo tree --edges normal | grep "adapter-" | ... # zero cross-adapter edges

# No key material reachable from provider or tool crates
# (static analysis: no import of secret-store modules from provider/tool crates)
cargo tree -p adapter-provider-anthropic | grep "secret" # must be 0
cargo tree -p adapter-provider-openai    | grep "secret" # must be 0

# Policy gate on every EffectIntent creation site
# (ensured by the fact that EffectIntent.grant_snapshot is non-optional and
#  set only by the PolicyEvaluator; grep for direct EffectIntent construction
#  outside policy_evaluator.rs must return 0)
```

These assertions encode the hexagonal no-cross-talk rule (adapters never call each other; only the kernel orchestrates) and the key-material isolation invariant (INV-01, INV-14) as executable checks rather than documentation conventions.

---

## 15. Glossary of secondary terms

Terms that appear in PRDs but do not need full canonical definitions here:

| Term | Brief definition |
|---|---|
| **ACK** | Acknowledgement of message receipt by a transport. |
| **ACP / A2A** | Agent Client Protocol / Agent-to-Agent protocol; interoperability standards. |
| **Bulletin** | Polkadot Statement Store; potential evidence anchoring surface. |
| **Chopsticks** | Fork-testing tool for Polkadot SDK chains. |
| **DAG** | Directed acyclic graph (artifact lineage structure). |
| **DTO** | Data transfer object; versioned wire-level data shape. |
| **ED** | Existential deposit; target runtime's minimum balance. |
| **JAM** | Join-Accumulate Machine; emerging Polkadot computation protocol. |
| **JTBD** | Job to be done; user outcome a product supports. |
| **MCP** | Model Context Protocol; tool/integration protocol. |
| **PAPI** | Polkadot API; TypeScript client approach. |
| **PCA** | `polkadot-chat-agents`; the reference chat-agent product. |
| **PVM / PolkaVM** | Polkadot Virtual Machine; deterministic execution environment. |
| **SCALE** | Binary encoding used by Polkadot SDK runtimes. |
| **SLO** | Service-level objective. |
| **SSE** | Server-Sent Events; streaming update protocol. |
| **Subxt** | Rust client library for Polkadot SDK chains. |
| **WASM** | WebAssembly; sandboxed extension format. |
| **XCM** | Cross-Consensus Messaging; cross-chain interaction format. |
| **Zombienet** | Polkadot network testing tool. |

---

## 16. Document acceptance criteria

This PRD is accepted when:

1. Every term used in PRDs 03-15 is either defined in Section 2 or in the glossary.
2. Every invariant in Section 3 has an automated verification in the CI pipeline.
3. The workspace layout in Section 6 compiles with `cargo build` (structure only; stub implementations).
4. The dependency rules in Section 5 are enforced by CI checks.
5. Every Port in Section 6 has a contract test suite template.
6. The ADRs in Section 13.2 have been reviewed and accepted by the project owner.
7. No subsequent PRD contradicts a definition or invariant established here.

---

## Appendix A: Complete trait hierarchy

```
Port Traits (polkagent-ports)
|
+-- TurnExecutor              # Performs one run/turn, emits events
+-- HarnessService            # Manages long-lived agent processes
+-- ModelProvider              # Connects to model inference APIs
|     +-- ModelCatalog         # Lists available models
|     +-- ModelRouter          # Routes requests to models
+-- ToolHost                   # Mediates tool invocations under grants
+-- SkillResolver              # Resolves skill references to content
+-- Transport                  # Receives/sends authenticated messages
+-- ChainClient                # Reads chain state, prepares actions
+-- Signer                     # Signs canonical payloads in isolation
+-- PaymentRail                # Handles payment lifecycle
+-- ContextSource              # Provides attributed context candidates
+-- ContextAssembler           # Selects context items under budget
+-- MemoryStore                # Durable memory storage and retrieval
+-- IdentityProvider           # Identity resolution and verification
+-- Feed                       # Event/data subscription source
+-- Trigger                    # Condition-based run activation
+-- ExtensionRegistry          # Extension discovery and lifecycle
+-- SecretResolver             # Resolves secret references to values
+-- Clock                      # Time source (mockable for testing)
+-- EventSink                  # Telemetry/observability export

Store Traits (polkagent-ports)
|
+-- RunStore                   # Run lifecycle persistence
+-- ArtifactStore              # Artifact and blob persistence
+-- EventStore                 # Event append and query
+-- EffectStore                # Effect intent/attempt/outcome persistence
+-- PolicyStore                # Policy storage and versioning
+-- ApprovalStore              # Approval persistence and query
+-- ConfigStore                # Configuration snapshot persistence
```

## Appendix B: State machine diagrams

### B.1 Run lifecycle

```
         +----------+
         | Created  |
         +----+-----+
              |
              v
         +----------+
         |  Queued   |
         +----+-----+
              |
              v
       +------+-------+
       |  Executing   |<---------+
       +------+-------+          |
              |                   |
     +--------+--------+         |
     |                  |         |
     v                  v         |
+----+------+   +-------+---+    |
| Awaiting  |   | Completing |    |
| Approval  |   +-------+---+    |
+----+------+           |        |
     |                   |        |
     +------+    +-------+--------+-------+-------+
            |    |       |                |       |
            v    v       v                v       v
        +---------+ +--------+ +----------+ +--------+
        |Completed| | Failed | | Cancelled| |TimedOut|
        +---------+ +--------+ +----------+ +--------+
```

### B.2 Effect lifecycle

```
                    +---------------+
                    | EffectIntent  |
                    | (durable)     |
                    +-------+-------+
                            |
                    +-------v-------+
                    | EffectAttempt |
                    | (claimed)     |
                    +-------+-------+
                            |
                    +-------v-------+
                    | EffectAttempt |
                    | (executing)   |
                    +-------+-------+
                            |
              +-------------+-------------+
              |             |             |
     +--------v---+ +------v----+ +------v------+
     |  Success   | |  Failure  | |   Unknown   |
     | (outcome)  | | (outcome) | |  (outcome)  |
     +------------+ +-----------+ +------+------+
                                         |
                                  [fresh evidence]
                                         |
                                +--------v--------+
                                | New Attempt or  |
                                | Resolution      |
                                +-----------------+
```

### B.3 Chain action saga

```
IntentCreated
    |
    v
MetadataEvidenceCaptured ----[metadata mismatch]----> Denied
    |
    v
CallDecoded
    |
    v
SimulationCompleted ----[simulation failure]----> Denied
    |
    v
PolicyEvaluated ----[policy denial]----> Denied
    |
    v
ApprovalRequested ----[approval denied]----> Denied
    |
    v
ApprovalGranted (or AutonomousMandateValid)
    |
    v
SignatureRequested ----[signer refused]----> Denied
    |
    v
SignatureObtained
    |
    v
BroadcastAttempted ----[broadcast error]----> Unknown/Retry
    |
    v
BroadcastConfirmed
    |
    v
FinalityObserved ----[timeout/reorg]----> Unknown/Review
    |
    v
Completed (with full evidence chain)
```

---

## APPENDIX A: COMPLETE RUST TRAIT DEFINITIONS

This appendix provides complete Rust trait definitions for every port/adapter boundary listed in the architecture. Each trait is production-ready, includes documentation comments that encode the contract, and is accompanied by a minimal mock implementation suitable for unit and contract tests.

### A.1 `ProviderPort` — Model Inference Provider

The `ProviderPort` is the boundary between `polkagent-app` and a concrete model inference endpoint. It does not own retry logic (that belongs to the outbox/worker layer) but does report retryability so the caller can decide.

```rust
// crates/polkagent-ports/src/provider.rs

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use polkagent_types::{
    ContentDigest, ModelId, ProviderId, RunId, StepId, TokenCount, Timestamp,
};

/// A single token-level chunk emitted during a streaming inference response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextChunk {
    pub text: String,
    pub token_count: u32,
}

/// A typed tool-call request from the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments_json: String,
}

/// The complete, non-streaming result of one inference call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceResponse {
    /// The full text content of the response (may be empty when only tool
    /// calls are present).
    pub text: String,
    /// Tool call requests from the model, if any.
    pub tool_calls: Vec<ToolCallRequest>,
    /// Stop reason: "end_turn", "max_tokens", "tool_use", or "stop_sequence".
    pub stop_reason: String,
    /// Token usage for billing, budgeting, and telemetry.
    pub usage: TokenUsage,
    /// Stable provider-assigned request identifier (for audit / idempotency).
    pub provider_request_id: Option<String>,
}

/// Token usage breakdown for one inference call.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,
    pub cache_write_tokens: Option<u32>,
}

/// A prepared inference request: context + tools + sampling parameters.
///
/// This type is deliberately opaque about provider-specific encoding.
/// The provider adapter is responsible for translating it into the
/// wire format expected by the model API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceRequest {
    pub run_id: RunId,
    pub step_id: StepId,
    /// The assembled messages in a provider-agnostic format.
    pub messages: Vec<InferenceMessage>,
    /// System prompt assembled by the context layer.
    pub system: Option<String>,
    /// Tool schemas available for this inference call.
    pub tools: Vec<ToolSchema>,
    pub model_id: ModelId,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    /// SHA-256 digest of the assembled context for audit purposes.
    pub context_digest: ContentDigest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceMessage {
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolResult { tool_call_id: String, content: String, is_error: bool },
    ToolUse { tool_call_id: String, tool_name: String, arguments_json: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema_json: String,
}

/// Capabilities the provider reports for a given model.
#[derive(Debug, Clone)]
pub struct ProviderCapabilities {
    pub provider_id: ProviderId,
    pub available_models: Vec<ModelCapabilityEntry>,
}

#[derive(Debug, Clone)]
pub struct ModelCapabilityEntry {
    pub model_id: ModelId,
    pub max_context_tokens: u32,
    pub supports_tool_calls: bool,
    pub supports_streaming: bool,
    pub supports_vision: bool,
    pub supports_structured_output: bool,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("authentication failed: {message}")]
    Authentication { message: String },

    #[error("rate limit exceeded; retry after {retry_after_secs:?} seconds")]
    RateLimit { retry_after_secs: Option<u64> },

    #[error("model context window exceeded: {tokens_requested} > {tokens_allowed}")]
    ContextWindowExceeded { tokens_requested: u32, tokens_allowed: u32 },

    #[error("provider returned invalid response: {message}")]
    InvalidResponse { message: String },

    #[error("provider request timed out after {elapsed_ms}ms")]
    Timeout { elapsed_ms: u64 },

    #[error("provider network error: {message}")]
    Network { message: String, retryable: bool },

    #[error("provider internal error: {message}")]
    Internal { message: String },
}

impl ProviderError {
    /// Whether the caller should retry this error via the outbox worker.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimit { .. } | Self::Timeout { .. } | Self::Network { retryable: true, .. }
        )
    }
}

/// The narrow provider port. One implementation per AI provider (Anthropic,
/// OpenAI-compatible, local, gateway).
///
/// # Contract
///
/// - Implementations must be `Send + Sync + 'static`.
/// - `complete` blocks until the full response is received.
/// - `complete` is idempotent for the same `InferenceRequest` (same
///   `step_id`); the provider may use the step_id as an idempotency hint.
/// - Implementations must never log raw prompt content at levels that would
///   reach telemetry exporters. Only token counts and metadata are safe to log.
#[async_trait]
pub trait ProviderPort: Send + Sync + 'static {
    /// Perform one inference call, returning the complete response.
    async fn complete(
        &self,
        request: InferenceRequest,
    ) -> Result<InferenceResponse, ProviderError>;

    /// Describe the provider's available models and capabilities.
    async fn capabilities(&self) -> Result<ProviderCapabilities, ProviderError>;

    /// Health check: return `Ok(())` if the provider is reachable and responsive.
    async fn health(&self) -> Result<(), ProviderError>;
}

// ---------------------------------------------------------------------------
// Mock implementation for contract tests
// ---------------------------------------------------------------------------

/// Deterministic mock provider for unit and contract tests.
///
/// Responses are configured at construction time.
pub struct MockProvider {
    response: InferenceResponse,
    capabilities: ProviderCapabilities,
    should_fail: bool,
}

impl MockProvider {
    pub fn new(response: InferenceResponse, capabilities: ProviderCapabilities) -> Self {
        Self { response, capabilities, should_fail: false }
    }

    pub fn failing() -> Self {
        Self {
            response: InferenceResponse {
                text: String::new(),
                tool_calls: vec![],
                stop_reason: "error".into(),
                usage: TokenUsage::default(),
                provider_request_id: None,
            },
            capabilities: ProviderCapabilities {
                provider_id: ProviderId::new("mock"),
                available_models: vec![],
            },
            should_fail: true,
        }
    }
}

#[async_trait]
impl ProviderPort for MockProvider {
    async fn complete(
        &self,
        _request: InferenceRequest,
    ) -> Result<InferenceResponse, ProviderError> {
        if self.should_fail {
            return Err(ProviderError::Network {
                message: "mock network failure".into(),
                retryable: true,
            });
        }
        Ok(self.response.clone())
    }

    async fn capabilities(&self) -> Result<ProviderCapabilities, ProviderError> {
        Ok(self.capabilities.clone())
    }

    async fn health(&self) -> Result<(), ProviderError> {
        if self.should_fail {
            return Err(ProviderError::Network {
                message: "mock unhealthy".into(),
                retryable: false,
            });
        }
        Ok(())
    }
}
```

### A.2 `SignerPort` — Isolated Payload Signing

The `SignerPort` enforces INV-01: the signer receives only canonical bytes and binding references, never model context.

```rust
// crates/polkagent-ports/src/signer.rs

use async_trait::async_trait;
use thiserror::Error;

use polkagent_types::{
    AccountRef, ApprovalId, ChainProfileId, GrantDigest, MetadataDigest,
};

/// The exact canonical bytes that must be signed, plus binding metadata.
///
/// This struct is the *only* input to the signer. It must not contain
/// model output, conversation history, tool results, or any data sourced
/// from the untrusted boundary. The kernel constructs this from typed
/// domain records only.
#[derive(Debug, Clone)]
pub struct CanonicalSignRequest {
    /// The SCALE-encoded transaction payload.
    pub payload: Vec<u8>,
    /// The account that must sign.
    pub account: AccountRef,
    /// The chain profile this transaction targets.
    pub chain_profile: ChainProfileId,
    /// Hash of the runtime metadata used to construct the payload.
    pub metadata_hash: MetadataDigest,
    /// Digest of the ResolvedGrant that authorized this effect.
    pub grant_digest: GrantDigest,
    /// The approval that authorized signing.
    pub approval_id: ApprovalId,
}

/// The signed payload returned by the signer.
#[derive(Debug, Clone)]
pub struct SignedPayload {
    /// The full signed extrinsic ready for submission.
    pub signed_extrinsic: Vec<u8>,
    /// The public key that signed (for audit evidence).
    pub public_key: Vec<u8>,
    /// The raw signature bytes (for audit evidence).
    pub signature: Vec<u8>,
}

/// Capabilities declared by this signer.
#[derive(Debug, Clone)]
pub struct SignerCapabilities {
    /// Account references this signer can sign for.
    pub accounts: Vec<AccountRef>,
    /// Chain profiles this signer is configured for.
    pub chain_profiles: Vec<ChainProfileId>,
    /// Whether this signer is backed by HSM/hardware.
    pub hardware_backed: bool,
    /// Human-readable signer name for operator display.
    pub display_name: String,
}

#[derive(Debug, Error)]
pub enum SignerError {
    #[error("account not found: {account:?}")]
    AccountNotFound { account: AccountRef },

    #[error("approval {approval_id:?} is not valid for this request")]
    InvalidApproval { approval_id: ApprovalId },

    #[error("grant digest mismatch: signing request does not match grant")]
    GrantMismatch,

    #[error("metadata hash mismatch: payload was built with a different runtime version")]
    MetadataMismatch,

    #[error("user rejected the signing request")]
    UserRejected,

    #[error("hardware signer error: {message}")]
    Hardware { message: String },

    #[error("signer timeout after {elapsed_ms}ms")]
    Timeout { elapsed_ms: u64 },

    #[error("signer internal error: {message}")]
    Internal { message: String },
}

/// The isolated signing port.
///
/// # Contract
///
/// - Implementations MUST NOT accept prompt text, conversation data,
///   model output, or memory content as inputs, even as optional fields.
/// - Implementations MUST verify that `approval_id` is valid and bound to
///   the exact `payload` before producing a signature.
/// - Implementations MUST verify that `metadata_hash` matches the hash of
///   the runtime metadata used to construct `payload`.
/// - A hardware-backed signer MUST display the canonical call to the user
///   for confirmation before signing.
#[async_trait]
pub trait SignerPort: Send + Sync + 'static {
    /// Describe accounts and chain profiles this signer can operate.
    async fn describe(&self) -> Result<SignerCapabilities, SignerError>;

    /// Sign the canonical payload under the given account and chain profile.
    ///
    /// This is the only method that produces a signature. Calling code in
    /// `polkagent-app` must not pass any data not present in
    /// `CanonicalSignRequest` to the signer.
    async fn sign(
        &self,
        request: CanonicalSignRequest,
    ) -> Result<SignedPayload, SignerError>;
}

/// Deterministic mock signer for tests. Always succeeds; does not verify
/// approval or metadata.
pub struct MockSigner {
    capabilities: SignerCapabilities,
}

impl MockSigner {
    pub fn new(capabilities: SignerCapabilities) -> Self {
        Self { capabilities }
    }
}

#[async_trait]
impl SignerPort for MockSigner {
    async fn describe(&self) -> Result<SignerCapabilities, SignerError> {
        Ok(self.capabilities.clone())
    }

    async fn sign(&self, request: CanonicalSignRequest) -> Result<SignedPayload, SignerError> {
        // In tests we produce a deterministic fake signature.
        let signature = vec![0u8; 64];
        let public_key = vec![0u8; 32];
        let mut signed_extrinsic = request.payload.clone();
        signed_extrinsic.extend_from_slice(&signature);
        Ok(SignedPayload { signed_extrinsic, public_key, signature })
    }
}
```

### A.3 `EventStore` — Ordered Event Persistence

```rust
// crates/polkagent-ports/src/store.rs (EventStore section)

use async_trait::async_trait;
use thiserror::Error;

use polkagent_types::{EventId, RunId, RunEvent, Durability};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("record not found: {resource_type} id={id}")]
    NotFound { resource_type: &'static str, id: String },

    #[error("sequence conflict: event {sequence} already exists for run {run_id}")]
    SequenceConflict { run_id: RunId, sequence: u64 },

    #[error("store backend error: {message}")]
    Backend { message: String, retryable: bool },

    #[error("serialization error: {message}")]
    Serialization { message: String },
}

/// Append-only ordered event store.
///
/// # Contract
///
/// - `append` is transactional: either the event is durably written or an
///   error is returned. There is no partial write.
/// - Events within a run are ordered by `sequence`. The implementation must
///   reject attempts to write a duplicate sequence number for the same run.
/// - `events_for_run` returns events in ascending sequence order.
/// - Events with `Durability::BestEffort` may be silently dropped on crash
///   but must never corrupt the durable event log.
/// - Implementations must enforce that a run has at most one terminal event
///   (RunCompleted, RunFailed, RunCancelled, RunTimedOut).
#[async_trait]
pub trait EventStore: Send + Sync + 'static {
    /// Durably append one event. Returns an error if the sequence number is
    /// already used for this run.
    async fn append(&self, event: &RunEvent) -> Result<(), StoreError>;

    /// Return all events for a run at or above `since_sequence`, in ascending
    /// sequence order.
    async fn events_for_run(
        &self,
        run_id: RunId,
        since_sequence: u64,
    ) -> Result<Vec<RunEvent>, StoreError>;

    /// Return the highest sequence number written for a run, or `None` if the
    /// run has no events.
    async fn last_sequence(&self, run_id: RunId) -> Result<Option<u64>, StoreError>;

    /// Return all terminal events in the store (for recovery/replay on restart).
    async fn terminal_events(
        &self,
        run_id: RunId,
    ) -> Result<Vec<RunEvent>, StoreError>;
}

/// In-memory event store for tests. Not crash-safe; do not use in production.
pub struct InMemoryEventStore {
    events: std::sync::Mutex<std::collections::BTreeMap<(RunId, u64), RunEvent>>,
}

impl InMemoryEventStore {
    pub fn new() -> Self {
        Self { events: std::sync::Mutex::new(std::collections::BTreeMap::new()) }
    }
}

#[async_trait]
impl EventStore for InMemoryEventStore {
    async fn append(&self, event: &RunEvent) -> Result<(), StoreError> {
        let mut events = self.events.lock().expect("mutex poisoned");
        let key = (event.run_id.clone(), event.sequence);
        if events.contains_key(&key) {
            return Err(StoreError::SequenceConflict {
                run_id: event.run_id.clone(),
                sequence: event.sequence,
            });
        }
        events.insert(key, event.clone());
        Ok(())
    }

    async fn events_for_run(
        &self,
        run_id: RunId,
        since_sequence: u64,
    ) -> Result<Vec<RunEvent>, StoreError> {
        let events = self.events.lock().expect("mutex poisoned");
        let result = events
            .range((run_id.clone(), since_sequence)..=(run_id.clone(), u64::MAX))
            .map(|(_, e)| e.clone())
            .collect();
        Ok(result)
    }

    async fn last_sequence(&self, run_id: RunId) -> Result<Option<u64>, StoreError> {
        let events = self.events.lock().expect("mutex poisoned");
        let last = events
            .range((run_id.clone(), 0)..=(run_id.clone(), u64::MAX))
            .next_back()
            .map(|((_, seq), _)| *seq);
        Ok(last)
    }

    async fn terminal_events(&self, run_id: RunId) -> Result<Vec<RunEvent>, StoreError> {
        use polkagent_types::EventKind;
        let events = self.events.lock().expect("mutex poisoned");
        let result = events
            .range((run_id.clone(), 0)..=(run_id.clone(), u64::MAX))
            .filter_map(|(_, e)| {
                matches!(
                    e.kind,
                    EventKind::RunCompleted
                        | EventKind::RunFailed
                        | EventKind::RunCancelled
                        | EventKind::RunTimedOut
                ).then(|| e.clone())
            })
            .collect();
        Ok(result)
    }
}
```

### A.4 `ArtifactStore` — Content-Addressed Artifact Storage

```rust
// crates/polkagent-ports/src/store.rs (ArtifactStore section)

use async_trait::async_trait;

use polkagent_types::{Artifact, ArtifactId, BlobRef, Classification, ContentDigest};

/// Content-addressed, classification-enforcing artifact store.
///
/// # Contract
///
/// - `store` is idempotent: storing the same artifact twice (same
///   `artifact.digest`) succeeds on the second call without error.
/// - `get_body` MUST verify the BLAKE3 digest of returned bytes against
///   `artifact.digest`. If verification fails, return `StoreError::DigestMismatch`.
/// - `Classification::SecretForbidden` artifacts must be rejected at store
///   boundaries that are not secret-specific stores. The default SQLite
///   artifact store must reject `SecretForbidden` writes.
/// - All reads enforce that the calling context is authorized for the
///   artifact's `Classification`. The store is not responsible for policy
///   evaluation, but it must expose classification metadata for callers.
#[async_trait]
pub trait ArtifactStore: Send + Sync + 'static {
    /// Store an artifact and its body. `body` must hash to `artifact.digest`.
    async fn store(&self, artifact: &Artifact, body: &[u8]) -> Result<(), StoreError>;

    /// Retrieve artifact metadata by ID.
    async fn get(&self, id: &ArtifactId) -> Result<Artifact, StoreError>;

    /// Retrieve the raw artifact body.
    ///
    /// Implementations MUST verify the body digest against the artifact
    /// record before returning bytes.
    async fn get_body(&self, body_ref: &BlobRef) -> Result<Vec<u8>, StoreError>;

    /// Verify the stored body matches the artifact digest without returning
    /// the body.
    async fn verify(&self, id: &ArtifactId) -> Result<bool, StoreError>;

    /// List artifact IDs for a given run, in creation order.
    async fn list_for_run(&self, run_id: &polkagent_types::RunId) -> Result<Vec<ArtifactId>, StoreError>;
}
```

### A.5 `PolicyEngine` — Deterministic Grant Resolution

```rust
// crates/polkagent-ports/src/policy.rs

use async_trait::async_trait;
use thiserror::Error;

use polkagent_types::{
    AgentId, ConfigDigest, EffectKind, GrantDigest, PolicyDigest, ResolvedGrant,
    ResourceSelector, SubjectId,
};

/// All inputs to one policy evaluation request.
///
/// All fields are typed domain records — no strings from model output.
#[derive(Debug, Clone)]
pub struct PolicyEvaluationRequest {
    pub subject: SubjectId,
    pub agent_id: AgentId,
    pub effect_kind: EffectKind,
    pub resource: ResourceSelector,
    pub policy_revision: PolicyDigest,
    pub config_revision: ConfigDigest,
    /// Current approval IDs active for this evaluation context.
    pub active_approval_ids: Vec<polkagent_types::ApprovalId>,
    /// Remaining budget state snapshot.
    pub budget_state: BudgetState,
}

#[derive(Debug, Clone)]
pub struct BudgetState {
    pub spend_remaining_usd: Option<rust_decimal::Decimal>,
    pub requests_remaining: Option<u64>,
    pub bytes_remaining: Option<u64>,
}

/// The result of one policy evaluation.
#[derive(Debug, Clone)]
pub enum PolicyDecision {
    /// The effect is permitted under the specified grant.
    Allow { grant: ResolvedGrant },
    /// The effect is denied for the given reason.
    Deny { reason: String, policy_revision: PolicyDigest },
    /// The effect requires explicit human (or quorum) approval before proceeding.
    RequireApproval {
        approver: polkagent_types::ApproverRef,
        reason: String,
        policy_revision: PolicyDigest,
    },
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("policy revision not found: {revision:?}")]
    RevisionNotFound { revision: PolicyDigest },

    #[error("policy schema validation failed: {message}")]
    SchemaInvalid { message: String },

    #[error("policy evaluation internal error: {message}")]
    Internal { message: String },
}

/// Pure, deterministic policy evaluation engine.
///
/// # Contract
///
/// - `evaluate` is a pure function: same inputs ALWAYS produce the same
///   `PolicyDecision`. There must be no randomness, clock reads, or I/O
///   inside `evaluate`.
/// - `evaluate` MUST NOT accept reputation scores, display names, or any
///   data derived from model output as inputs to the allow/deny decision.
/// - `load_revision` validates the Cedar policy document at load time.
///   An invalid policy document must be rejected before any Run is created.
/// - The default decision for an unmatched resource is `Deny`.
#[async_trait]
pub trait PolicyEngine: Send + Sync + 'static {
    /// Evaluate one effect request against the active policy revision.
    async fn evaluate(
        &self,
        request: PolicyEvaluationRequest,
    ) -> Result<PolicyDecision, PolicyError>;

    /// Load and validate a new policy revision. Returns the digest of the
    /// loaded policy for auditing.
    async fn load_revision(
        &self,
        policy_document: &str,
        format: PolicyFormat,
    ) -> Result<PolicyDigest, PolicyError>;

    /// Return the currently active policy revision digest.
    async fn current_revision(&self) -> Result<PolicyDigest, PolicyError>;
}

#[derive(Debug, Clone, Copy)]
pub enum PolicyFormat {
    Cedar,
}
```

### A.6 `MemoryStore` — Durable Memory Storage

```rust
// crates/polkagent-ports/src/memory.rs

use async_trait::async_trait;
use thiserror::Error;

use polkagent_types::{AgentId, ArtifactId, Classification, ContentDigest, RunId, Timestamp};

/// One stored memory item: attributed, classified, and content-addressed.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: MemoryEntryId,
    pub agent_id: AgentId,
    pub kind: MemoryKind,
    pub content_digest: ContentDigest,
    /// The summarized or redacted content safe for context assembly.
    pub content_summary: String,
    pub classification: Classification,
    pub source_run: Option<RunId>,
    pub source_artifact: Option<ArtifactId>,
    pub relevance_score: f32,
    pub created_at: Timestamp,
    pub expires_at: Option<Timestamp>,
    pub access_count: u64,
    pub last_accessed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    /// Episodic: a summarized, redacted record of a past interaction.
    Episodic,
    /// Semantic: extracted, reviewed candidate knowledge.
    Semantic,
    /// Working: short-lived state for the current conversation.
    Working,
}

#[derive(Debug, Clone)]
pub struct MemoryEntryId(pub uuid::Uuid);

/// Query parameters for memory retrieval.
#[derive(Debug, Clone)]
pub struct MemoryQuery {
    pub agent_id: AgentId,
    pub query_text: Option<String>,
    pub kinds: Vec<MemoryKind>,
    pub max_results: u32,
    pub min_relevance: f32,
    /// Exclude entries whose `Classification` exceeds this level.
    pub max_classification: Classification,
    pub run_context: Option<RunId>,
}

#[derive(Debug, Error)]
pub enum MemoryError {
    #[error("entry not found: {id:?}")]
    NotFound { id: MemoryEntryId },

    #[error("classification violation: entry {id:?} classification exceeds allowed maximum")]
    ClassificationViolation { id: MemoryEntryId },

    #[error("memory store backend error: {message}")]
    Backend { message: String },
}

/// Durable, classified, attributed memory store.
///
/// # Contract
///
/// - `admit` must enforce `Classification` at write time: `SecretForbidden`
///   entries must be rejected.
/// - `query` must filter returned entries by `max_classification`. An entry
///   whose classification exceeds the query's `max_classification` must not
///   appear in results, even if it would otherwise match.
/// - Memory retrieval must attribute each result to its source run and
///   artifact for audit and evidence tracing.
/// - Expired entries (past `expires_at`) must not be returned by `query`.
#[async_trait]
pub trait MemoryStore: Send + Sync + 'static {
    /// Store a new memory entry.
    async fn admit(&self, entry: MemoryEntry) -> Result<MemoryEntryId, MemoryError>;

    /// Retrieve a memory entry by ID.
    async fn get(&self, id: &MemoryEntryId) -> Result<MemoryEntry, MemoryError>;

    /// Query memory entries for context assembly.
    ///
    /// Implementations should apply relevance scoring, classification
    /// filtering, expiry filtering, and result limiting in this method.
    async fn query(&self, query: MemoryQuery) -> Result<Vec<MemoryEntry>, MemoryError>;

    /// Update the relevance score and access metadata for an entry.
    async fn record_access(&self, id: &MemoryEntryId) -> Result<(), MemoryError>;

    /// Expire entries older than `cutoff` for the given agent.
    async fn expire_before(
        &self,
        agent_id: &AgentId,
        cutoff: Timestamp,
    ) -> Result<u64, MemoryError>;
}
```

### A.7 `TransportPort` — Authenticated Message Transport

```rust
// crates/polkagent-ports/src/transport.rs

use async_trait::async_trait;
use thiserror::Error;
use std::time::Duration;

use polkagent_types::{Classification, ConversationId, RunId, Timestamp, UserId};

/// A message received from an external actor.
#[derive(Debug, Clone)]
pub struct IncomingMessage {
    /// Stable delivery identifier, used for ACK.
    pub delivery_id: DeliveryId,
    pub conversation_id: ConversationId,
    pub sender: AuthenticatedSender,
    pub body: MessageBody,
    /// Transport-assigned receive timestamp (before processing).
    pub received_at: Timestamp,
}

#[derive(Debug, Clone)]
pub struct DeliveryId(pub String);

#[derive(Debug, Clone)]
pub struct AuthenticatedSender {
    pub user_id: UserId,
    pub display_name: Option<String>,
    /// Trust tier of this sender channel.
    pub trust_tier: SenderTrustTier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenderTrustTier {
    /// Verified authenticated user.
    Authenticated,
    /// Webhook or external callback (lower trust).
    External,
}

#[derive(Debug, Clone)]
pub enum MessageBody {
    Text { content: String },
    File { name: String, mime_type: String, body: Vec<u8> },
    StructuredCommand { command_type: String, payload_json: String },
}

/// A message to deliver to an external recipient.
#[derive(Debug, Clone)]
pub struct OutgoingMessage {
    pub conversation_id: ConversationId,
    pub run_id: Option<RunId>,
    pub body: OutgoingBody,
    pub classification: Classification,
}

#[derive(Debug, Clone)]
pub enum OutgoingBody {
    Text { content: String },
    StructuredCard { card_type: String, payload_json: String },
    ApprovalRequest { payload_json: String },
}

#[derive(Debug, Clone)]
pub struct DeliveryReceipt {
    pub delivery_id: DeliveryId,
    pub delivered_at: Timestamp,
}

/// Capabilities reported by this transport.
#[derive(Debug, Clone)]
pub struct TransportCapabilities {
    pub supports_streaming: bool,
    pub supports_structured_cards: bool,
    pub supports_file_transfer: bool,
    pub max_message_bytes: u64,
    pub supported_auth_methods: Vec<String>,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("authentication failed for sender: {message}")]
    Authentication { message: String },

    #[error("delivery failed for conversation {conversation_id:?}: {message}")]
    DeliveryFailed { conversation_id: ConversationId, message: String },

    #[error("transport connection lost")]
    ConnectionLost,

    #[error("rate limit exceeded; retry after {retry_after:?}")]
    RateLimit { retry_after: Option<Duration> },

    #[error("transport internal error: {message}")]
    Internal { message: String },
}

/// The narrow transport port. One implementation per channel type
/// (Polkadot chat, WebSocket, HTTP, CLI).
///
/// # Contract
///
/// - `receive` blocks until a message arrives or the transport is closed.
/// - `ack` must be called exactly once per `IncomingMessage` after the
///   message is durably admitted. Not calling `ack` causes the transport
///   to re-deliver the message.
/// - `send` does not guarantee delivery; delivery tracking is separate.
/// - Implementations must not add business logic. All validation happens
///   in the application layer.
#[async_trait]
pub trait TransportPort: Send + Sync + 'static {
    /// Wait for the next incoming message.
    async fn receive(&self) -> Result<IncomingMessage, TransportError>;

    /// Acknowledge that a message has been durably admitted to the system.
    async fn ack(&self, delivery_id: DeliveryId) -> Result<(), TransportError>;

    /// Send an outgoing message. Returns a receipt for delivery tracking.
    async fn send(&self, message: OutgoingMessage) -> Result<DeliveryReceipt, TransportError>;

    /// Describe what this transport can deliver.
    fn capabilities(&self) -> TransportCapabilities;
}
```

### A.8 `MetadataPort` (ChainClient) — Chain State and Metadata

```rust
// crates/polkagent-ports/src/chain.rs

use async_trait::async_trait;
use thiserror::Error;

use polkagent_types::{
    BlockRef, ChainProfileId, GenesisHash, MetadataDigest, TxHash,
};

/// Pinned runtime metadata fetched from a specific block.
#[derive(Debug, Clone)]
pub struct PinnedMetadata {
    pub chain_profile: ChainProfileId,
    pub spec_version: u32,
    pub metadata_digest: MetadataDigest,
    /// The raw SCALE-encoded metadata bytes.
    pub metadata_bytes: Vec<u8>,
    pub block_ref: BlockRef,
    pub fetched_at: polkagent_types::Timestamp,
}

/// A decoded call with human-readable rendering.
#[derive(Debug, Clone)]
pub struct DecodedCall {
    pub pallet: String,
    pub call_name: String,
    /// JSON rendering of decoded call arguments.
    pub arguments_json: String,
    pub metadata_digest: MetadataDigest,
}

/// Result of a dry-run / simulation of a transaction.
#[derive(Debug, Clone)]
pub struct SimulationResult {
    pub success: bool,
    pub fee_estimate: Option<u128>,
    pub error_message: Option<String>,
    pub storage_changes_preview: Vec<StorageChange>,
    pub block_ref: BlockRef,
}

#[derive(Debug, Clone)]
pub struct StorageChange {
    pub key_prefix: String,
    pub change_type: StorageChangeType,
}

#[derive(Debug, Clone, Copy)]
pub enum StorageChangeType { Write, Delete, Read }

/// A watched finality outcome.
#[derive(Debug, Clone)]
pub enum FinalityObservation {
    Finalized { block_ref: BlockRef, tx_index: u32 },
    Failed { block_ref: BlockRef, error_message: String },
    /// The observation timed out without confirming inclusion.
    Unknown { last_checked_block: BlockRef },
}

#[derive(Debug, Error)]
pub enum ChainClientError {
    #[error("chain profile not found: {chain_profile:?}")]
    ProfileNotFound { chain_profile: ChainProfileId },

    #[error("RPC error from {endpoint}: {message}")]
    Rpc { endpoint: String, message: String, retryable: bool },

    #[error("genesis hash mismatch: expected {expected:?}, got {actual:?}")]
    GenesisHashMismatch { expected: GenesisHash, actual: GenesisHash },

    #[error("metadata fetch failed: {message}")]
    MetadataFetch { message: String },

    #[error("simulation failed: {message}")]
    SimulationFailed { message: String },

    #[error("finality observation timeout after {elapsed_ms}ms")]
    FinalityTimeout { elapsed_ms: u64 },
}

/// The chain client port: read-only chain state access, metadata pinning,
/// call decoding, simulation, submission, and finality observation.
///
/// # Contract
///
/// - `fetch_metadata` must pin to the block specified in the chain profile.
///   It must verify that the fetched genesis hash matches the profile's
///   `genesis_hash`. A mismatch must return `ChainClientError::GenesisHashMismatch`.
/// - `decode_call` must use only the provided `metadata_bytes`; it must not
///   make additional RPC calls.
/// - `simulate` runs a dry-run against the specified block. The result is
///   evidence, not authorization.
/// - `submit` sends the signed extrinsic and returns the transaction hash.
///   It does NOT wait for inclusion or finality.
/// - `watch_finality` observes inclusion and finality for a transaction hash.
///   If the observation times out, return `FinalityObservation::Unknown`.
///   NEVER return `Finalized` without verified on-chain evidence.
#[async_trait]
pub trait ChainClientPort: Send + Sync + 'static {
    /// Fetch and pin runtime metadata for the given chain profile.
    async fn fetch_metadata(
        &self,
        chain_profile: ChainProfileId,
    ) -> Result<PinnedMetadata, ChainClientError>;

    /// Decode a SCALE-encoded call using the provided pinned metadata.
    async fn decode_call(
        &self,
        call_bytes: &[u8],
        metadata: &PinnedMetadata,
    ) -> Result<DecodedCall, ChainClientError>;

    /// Dry-run a signed extrinsic against the specified block.
    async fn simulate(
        &self,
        signed_extrinsic: &[u8],
        block_ref: &BlockRef,
        metadata: &PinnedMetadata,
    ) -> Result<SimulationResult, ChainClientError>;

    /// Submit a signed extrinsic to the network. Returns the tx hash.
    async fn submit(
        &self,
        signed_extrinsic: &[u8],
        chain_profile: ChainProfileId,
    ) -> Result<TxHash, ChainClientError>;

    /// Watch for finality of a submitted transaction.
    /// Returns `Unknown` on timeout rather than erroring.
    async fn watch_finality(
        &self,
        tx_hash: TxHash,
        chain_profile: ChainProfileId,
        timeout_ms: u64,
    ) -> Result<FinalityObservation, ChainClientError>;
}
```

---

## APPENDIX B: WORKSPACE CRATE MAP

This appendix provides the complete mapping from the proposed workspace structure to crate purposes, dependencies, public API surface, and feature flags. The structure follows the pattern established by Roko (18 crates, grouped by layer) and Bardo (apps/ and crates/ separation).

```
polkagent/
├── Cargo.toml                            # Workspace root, shared dependencies
│
├── crates/                               # Library crates (no main.rs)
│   │
│   ├── polkagent-types/                  # Layer 0: Stable domain types
│   ├── polkagent-kernel/                 # Layer 1: State machines, reducers
│   ├── polkagent-ports/                  # Layer 1: Port trait definitions
│   ├── polkagent-app/                    # Layer 2: Application services
│   │
│   └── polkagent-runtime/                # Cross-cutting: tokio runtime config
│
├── adapters/                             # Layer 3: Port implementations
│   ├── adapter-store-sqlite/
│   ├── adapter-store-postgres/
│   ├── adapter-provider-anthropic/
│   ├── adapter-provider-openai/
│   ├── adapter-provider-local/
│   ├── adapter-executor-direct/
│   ├── adapter-executor-coding/
│   ├── adapter-transport-polkadot-chat/
│   ├── adapter-transport-websocket/
│   ├── adapter-transport-http/
│   ├── adapter-chain-subxt/
│   ├── adapter-signer-external/
│   ├── adapter-signer-local-encrypted/
│   ├── adapter-signer-proxy/
│   └── adapter-telemetry-otel/
│
├── apps/                                 # Layer 4: Binary entry points
│   ├── polkagent-cli/                    # CLI + TUI (roko-cli pattern)
│   └── polkagent-serve/                  # HTTP/WebSocket API server
│
└── tests/                                # Workspace-level tests
    ├── contract-tests/                   # One suite per port
    └── integration-tests/                # Cross-crate scenarios
```

### B.1 `polkagent-types` — Stable Domain Types

**Purpose:** The foundational type library. Contains all domain type definitions, IDs, enums, and serialization. Has zero async, zero I/O, and zero provider-specific dependencies. Every other crate in the workspace that needs domain types depends on this one.

**Analogues:** `roko-primitives` + `roko-core` (type portions); `bardo/crates/bardo-primitives`.

**Dependencies:**
```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
blake3 = "1"
thiserror = "2"
rust_decimal = { version = "1", features = ["serde-with-str"] }
```

**Forbidden dependencies:** `tokio`, `async-trait`, `reqwest`, `rusqlite`, any vendor-specific API crate.

**Public API surface:**
- All ID types: `RunId`, `StepId`, `ArtifactId`, `EffectIntentId`, `EffectAttemptId`, `EffectOutcomeId`, `EventId`, `AgentId`, `ConversationId`, `TurnId`, `ModelId`, `ProviderId`, `ToolId`, `SkillId`, `ChainProfileId`, `PolicyId`, `GrantId`, `ApprovalId`, `UserId`, `TenantId`
- Core structs: `Run`, `Step`, `Turn`, `RunEvent`, `Artifact`, `EffectIntent`, `EffectAttempt`, `EffectOutcome`, `ResolvedGrant`, `Policy`, `Approval`, `AgentSpec`, `ChainProfile`
- All enums: `RunStatus`, `EventKind`, `Durability`, `ArtifactKind`, `EffectKind`, `Classification`, `OutcomeStatus`, `AttemptStatus`
- Content addressing: `ContentDigest` (BLAKE3 + algorithm tag), `MetadataDigest`, `GrantDigest`, `PolicyDigest`, `ConfigDigest`
- Error taxonomy: `DomainError`

**Feature flags:**
```toml
[features]
default = []
# Enable proptest Arbitrary impls for all domain types
proptest = ["dep:proptest"]
# Enable test fixture factories
test-fixtures = []
```

### B.2 `polkagent-kernel` — Minimal Domain Kernel

**Purpose:** The state machine, reducer, outbox manager, grant resolver, and content-digest verifier. Contains no I/O. All functions are pure or operate on an abstract store interface defined in `polkagent-ports`.

**Analogues:** Core orchestration logic in `roko-core` (conductor, immune, arbitration modules); Bardo's `golem-runtime`.

**Dependencies:**
```toml
[dependencies]
polkagent-types = { path = "../polkagent-types" }
serde = { version = "1", features = ["derive"] }
blake3 = "1"
thiserror = "2"
tracing = "0.1"
```

**Forbidden dependencies:** `tokio`, `async-trait`, `rusqlite`, any adapter crate.

**Public API surface:**
- `TurnReducer`: `fn reduce(state: RunState, input: KernelInput) -> (RunState, Vec<EffectIntent>)`
- `StateMachine`: `fn transition(current: RunStatus, event: LifecycleEvent) -> Result<RunStatus, StateMachineError>`
- `GrantResolver`: `fn resolve(inputs: GrantResolutionInputs) -> ResolvedGrant`
- `ContentVerifier`: `fn verify(artifact: &Artifact, body: &[u8]) -> bool`
- `IdempotencyKeyGen`: `fn generate(intent: &EffectIntent) -> IdempotencyKey`
- `ClassificationGuard`: `fn check(item: &dyn Classified, context: &ClassificationContext) -> ClassificationResult`
- `OutboxManager`: manages the durable intent queue (pure in-memory; persistence via EventStore port)

**Feature flags:**
```toml
[features]
default = []
# Enable the reducer fuzzing harness
fuzz = []
```

### B.3 `polkagent-ports` — Port Trait Definitions

**Purpose:** Defines all trait boundaries between the kernel/application layer and external systems. Does not contain implementations. Traits use `dyn Trait` at call sites to avoid monomorphization.

**Analogues:** The Gate/Substrate/Composer traits in `roko-core`; the port declarations in `bardo/crates/golem-core`.

**Dependencies:**
```toml
[dependencies]
polkagent-types = { path = "../polkagent-types" }
async-trait = "0.1"
thiserror = "2"
serde = { version = "1", features = ["derive"] }
rust_decimal = { version = "1", features = ["serde-with-str"] }
```

**Public API surface:**

| Trait | File | Description |
|---|---|---|
| `ProviderPort` | `src/provider.rs` | Model inference |
| `SignerPort` | `src/signer.rs` | Payload signing |
| `ChainClientPort` | `src/chain.rs` | Chain reads, decode, simulate, submit, finality |
| `TransportPort` | `src/transport.rs` | Authenticated message I/O |
| `ToolHostPort` | `src/tool.rs` | Mediated tool invocation |
| `MemoryStore` | `src/memory.rs` | Durable memory storage |
| `EventStore` | `src/store.rs` | Ordered event persistence |
| `ArtifactStore` | `src/store.rs` | Content-addressed artifact storage |
| `RunStore` | `src/store.rs` | Run lifecycle persistence |
| `EffectStore` | `src/store.rs` | Effect intent/attempt/outcome persistence |
| `PolicyEngine` | `src/policy.rs` | Deterministic policy evaluation |
| `SecretResolver` | `src/secret.rs` | Secret handle resolution |
| `IdentityProvider` | `src/identity.rs` | Principal resolution and verification |
| `ExtensionRegistry` | `src/extension.rs` | Extension lifecycle |
| `Clock` | `src/clock.rs` | Mockable time source |
| `EventSink` | `src/telemetry.rs` | Telemetry/observability export |

**Feature flags:**
```toml
[features]
default = []
# Enable mock implementations of all ports
mocks = []
```

### B.4 `polkagent-app` — Application Services

**Purpose:** The application layer that orchestrates conversations, runs, context assembly, policy evaluation, approval workflows, delivery, and projections. Depends on kernel and ports; never imports adapter crates.

**Analogues:** `roko-orchestrator` for orchestration; Bardo's `bardo-gateway` for routing and `golem-runtime` for execution context.

**Dependencies:**
```toml
[dependencies]
polkagent-types = { path = "../polkagent-types" }
polkagent-kernel = { path = "../polkagent-kernel" }
polkagent-ports = { path = "../polkagent-ports" }
async-trait = "0.1"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
```

**Public API surface:**

| Service | File | Responsibility |
|---|---|---|
| `ConversationService` | `src/conversation.rs` | Create, continue, close conversations and turns |
| `RunOrchestrator` | `src/run_orchestrator.rs` | Run lifecycle, step sequencing, outbox dispatch |
| `ContextAssembler` | `src/context_assembler.rs` | Pure context pack assembly under budget and classification constraints |
| `PolicyEvaluator` | `src/policy_evaluator.rs` | Cedar-backed grant resolution via `PolicyEngine` port |
| `ApprovalService` | `src/approval_service.rs` | Approval request routing, tracking, canonical card rendering |
| `DeliveryService` | `src/delivery_service.rs` | Response delivery management, independent of run completion |
| `ProjectionBuilder` | `src/projection_builder.rs` | Versioned read-model computation from event log |
| `MemoryService` | `src/memory_service.rs` | Memory admission, retrieval, expiry scheduling |

**Feature flags:**
```toml
[features]
default = []
# Enable workflow graph orchestration (phased)
workflow-graph = []
```

### B.5 `apps/polkagent-cli` — CLI and TUI

**Purpose:** The user-facing CLI binary. Wires the application layer to selected adapter implementations. Contains the ratatui-based TUI for local operation.

**Pattern:** Follows `roko-cli` architecture: `App` struct holding `TuiState`, background I/O channels, watcher handles, and an event loop. Rendering path does zero I/O. System metrics and git data run on background threads. See `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/app.rs` for the reference pattern.

**Dependencies:**
```toml
[dependencies]
polkagent-app = { path = "../../crates/polkagent-app" }
# Selected adapters (feature-gated)
adapter-store-sqlite = { path = "../../adapters/adapter-store-sqlite" }
adapter-provider-anthropic = { path = "../../adapters/adapter-provider-anthropic" }
adapter-executor-direct = { path = "../../adapters/adapter-executor-direct" }
adapter-chain-subxt = { path = "../../adapters/adapter-chain-subxt" }
# TUI
ratatui = "0.28"
crossterm = "0.28"
# CLI parsing
clap = { version = "4", features = ["derive"] }
tokio = { version = "1", features = ["full"] }
```

**Key modules:**

```
apps/polkagent-cli/src/
├── main.rs                    # Clap root command dispatch
├── commands/
│   ├── run.rs                 # polkagent run <prompt>
│   ├── init.rs                # polkagent init
│   ├── agent.rs               # polkagent agent {create,list,show}
│   ├── chain.rs               # polkagent chain {status,simulate,submit}
│   └── config.rs              # polkagent config {validate,show}
└── tui/
    ├── app.rs                 # App: event loop, background I/O, rendering
    ├── state.rs               # TuiState: all mutable TUI state
    ├── views/                 # Per-tab views (Runs, Approvals, Events, Chain)
    ├── modals.rs              # Approval modal, confirm dialogs
    └── theme.rs               # Color scheme
```

### B.6 `apps/polkagent-serve` — HTTP/WebSocket API Server

**Purpose:** The API server binary for managed, team, and SDK-integration deployments. Exposes REST + SSE/WebSocket endpoints backed by `polkagent-app`.

**Pattern:** Follows `roko-serve` + `bardo/apps/bardo-gateway` patterns: Axum-based, tower middleware for rate limiting and auth, SSE for live event streams, WebSocket for chat surfaces.

**Dependencies:**
```toml
[dependencies]
polkagent-app = { path = "../../crates/polkagent-app" }
axum = { version = "0.8", features = ["ws"] }
tower = { version = "0.5", features = ["limit", "load-shed"] }
tower-http = { version = "0.6", features = ["cors", "trace", "limit"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

### B.7 Adapter Crates

Each adapter crate implements exactly one port trait. This table documents the full set:

| Crate | Port Implemented | External Dependency |
|---|---|---|
| `adapter-store-sqlite` | `RunStore`, `ArtifactStore`, `EventStore`, `EffectStore`, `PolicyStore`, `ApprovalStore` | `rusqlite` (bundled) |
| `adapter-store-postgres` | Same stores | `tokio-postgres`, `deadpool-postgres` |
| `adapter-provider-anthropic` | `ProviderPort` | `reqwest`, Anthropic API |
| `adapter-provider-openai` | `ProviderPort` | `reqwest`, OpenAI-compatible API |
| `adapter-provider-local` | `ProviderPort` | `llama.cpp` bindings or `ollama` HTTP |
| `adapter-executor-direct` | `TurnExecutor` | `polkagent-ports` only |
| `adapter-executor-coding` | `TurnExecutor`, `HarnessService` | MCP stdio, subprocess |
| `adapter-transport-polkadot-chat` | `TransportPort` | PCA-compatible Matrix/chat API |
| `adapter-transport-websocket` | `TransportPort` | `tokio-tungstenite` |
| `adapter-transport-http` | `TransportPort` | `axum` (webhook receiver) |
| `adapter-chain-subxt` | `ChainClientPort` | `subxt` |
| `adapter-signer-external` | `SignerPort` | External wallet protocol |
| `adapter-signer-local-encrypted` | `SignerPort` | `age` encryption, local keystore |
| `adapter-signer-proxy` | `SignerPort` | Proxy/multisig RPC |
| `adapter-telemetry-otel` | `EventSink` | `opentelemetry`, `tracing-opentelemetry` |

---

## APPENDIX C: INVARIANT ENFORCEMENT

This appendix specifies, for each system invariant in Section 3, exactly how enforcement is implemented in code, how it is tested, what happens on violation, and how the system recovers.

### C.1 INV-01: Models never receive raw signing keys

**Compile-time enforcement:**
- `polkagent-types` and `polkagent-kernel` have no dependency on secret-store crates. Running `cargo tree -p polkagent-types | grep -c "secret"` in CI must return 0.
- The `CanonicalSignRequest` struct in `polkagent-ports` contains only `Vec<u8>` payload, `AccountRef`, `ChainProfileId`, `MetadataDigest`, `GrantDigest`, and `ApprovalId`. It has no field for private key material. The type system prevents accidental inclusion.
- `ClassificationGuard::check` returns an error at compile time for any code path that passes a `SecretForbidden`-classified value to a function accepting `ContextItem`.

**Runtime enforcement:**
- The `ContextAssembler` calls `ClassificationGuard::check` before adding each item to a context pack. Items classified `SecretForbidden` are excluded and logged as `ExclusionReason::ClassificationRestricted`.
- The `SecretResolver` port returns opaque `SecretHandle` values, not raw secret bytes, to any caller outside the signing path.
- The `SignerPort` adapter implementations hold key material in process memory only; they do not log it, include it in error messages, or return it.

**Test strategy:**
```rust
#[test]
fn secret_forbidden_excluded_from_context_pack() {
    let assembler = ContextAssembler::new_for_test();
    let secret_item = ContextItem {
        artifact_ref: ArtifactId::new(),
        classification: Classification::SecretForbidden,
        ..Default::default()
    };
    let pack = assembler.assemble(vec![secret_item], TokenBudget::default());
    assert!(pack.included.is_empty());
    assert_eq!(pack.excluded[0].reason, ExclusionReason::ClassificationRestricted);
}

#[test]
fn cargo_tree_shows_no_secret_crate_in_provider_adapter() {
    // This is a CI script test; see tests/contract-tests/ci-fitness/
    // cargo tree -p adapter-provider-anthropic | grep "secret" must produce 0 lines
}
```

**Violation behavior:** Any attempt to include a `SecretForbidden` item in model context causes the `ContextAssembler` to emit an `InvariantViolation` event and exclude the item. The event is classified as an internal audit event and not surfaced to the model.

**Recovery:** No recovery needed; the violation is prevented before it can have any effect. If a violation is detected in a running system, the affected run is failed with `DomainError::Internal` and an alert is raised.

### C.2 INV-02: Grants are resolved outside model-controlled text

**Compile-time enforcement:**
- `ResolvedGrant` has no public constructor outside `polkagent-kernel::GrantResolver`. The `new()` function is `pub(crate)`. External code can only receive a `ResolvedGrant` from `PolicyEngine::evaluate`.
- `EffectIntent.grant_snapshot` is `GrantDigest` (a hash), not `String`. The type system prevents a model text string from being used as a grant.

**Runtime enforcement:**
- `PolicyEvaluator` is the only caller of `PolicyEngine::evaluate`. Its inputs are typed domain records, not model output strings.
- `GrantDigest` is verified against the live `ResolvedGrant` before an effect is executed. A tampered digest causes the attempt to be rejected.

**Test strategy:**
```rust
#[tokio::test]
async fn model_output_cannot_produce_grant() {
    // Demonstrate that the type system prevents this, not just convention.
    // ResolvedGrant::new() is not accessible outside polkagent-kernel.
    // The only way to get a ResolvedGrant is through PolicyEngine::evaluate().
    let engine = MockPolicyEngine::deny_all();
    let request = PolicyEvaluationRequest {
        effect_kind: EffectKind::InvokeTool,
        ..test_request()
    };
    let decision = engine.evaluate(request).await.unwrap();
    assert!(matches!(decision, PolicyDecision::Deny { .. }));
}
```

**Violation behavior:** If code attempts to construct a `ResolvedGrant` outside `GrantResolver`, the compiler rejects it (private constructor). A digest mismatch at execution time causes the attempt to be `Failed` with `GrantMismatch` and an audit event.

### C.3 INV-03: Effects are durable before I/O

**Compile-time enforcement:**
- `RunOrchestrator` is the only code that creates `EffectIntent` records. Its internal signature is `async fn create_intent_and_transition(&self, ...) -> Result<EffectIntent, OrchestratorError>` which calls `EffectStore::create_intent` and the state transition in a single SQLite `BEGIN IMMEDIATE` transaction.
- Workers receive intents by calling `EffectStore::claim_next`; they cannot receive intents through in-memory channels (no `mpsc` channel carries intents to workers).

**Runtime enforcement:**
- SQLite `BEGIN IMMEDIATE` transaction covers both state write and intent write. If the process crashes after the transaction commits, the intent is recoverable on restart.
- On restart, `RunOrchestrator::recover_on_start` calls `EffectStore::claim_next` to find any unclaimed or lease-expired intents and re-queues them.

**Test strategy:**
```rust
#[tokio::test]
async fn crash_before_io_intent_survives() {
    let store = InMemoryEffectStore::new();
    let orchestrator = RunOrchestrator::new_for_test(store.clone());

    // Simulate: orchestrator creates intent but process "crashes" before worker runs
    let intent = orchestrator.create_intent_for_test(test_effect_payload()).await.unwrap();

    // New orchestrator instance recovers
    let new_orchestrator = RunOrchestrator::new_for_test(store.clone());
    new_orchestrator.recover_on_start().await.unwrap();

    // Intent is available for claiming
    let claimed = store.claim_next(WorkerId::new("worker-1")).await.unwrap();
    assert!(claimed.is_some());
    assert_eq!(claimed.unwrap().id, intent.id);
}
```

**Violation behavior:** If an EffectIntent is not committed before I/O begins (a bug), the corresponding external effect may be silently lost on crash. This is a correctness violation that must be detected by the crash-recovery test suite before it reaches production.

**Recovery:** On process restart, `recover_on_start` scans for orphaned intents. Any intent in `Created` status without a live worker claim is re-queued. Lease-expired `Claimed` intents are released for re-claiming.

### C.4 INV-04: Unknown outcomes are never collapsed

**Compile-time enforcement:**
- `OutcomeStatus::Unknown` is a variant that cannot be implicitly converted to `Success` or `Failure`. Pattern matches on `OutcomeStatus` must explicitly handle `Unknown`.
- `RunStatus` has no transition from any state to `Completed` if an outstanding effect has `OutcomeStatus::Unknown`. The state machine's `transition` function rejects such a transition.

**Runtime enforcement:**
- The chain saga's finality observer returns `FinalityObservation::Unknown` on timeout (never an assumed `Finalized`).
- `RunOrchestrator` checks for unknown-outcome effects before marking a run `Completed`.

**Test strategy:**
```rust
#[test]
fn state_machine_blocks_completion_with_unknown_effect() {
    let machine = StateMachine::new();
    let result = machine.transition(
        RunStatus::Completing,
        LifecycleEvent::EffectOutcomeUnknown { effect_id: test_id() },
    );
    assert!(matches!(result, Err(StateMachineError::UnknownEffectPreventsCompletion)));
}
```

**Violation behavior:** If code tries to collapse `Unknown` to `Failure` or `Success`, the state machine transition is rejected. The violation is logged as `InvariantViolation::UnknownCollapsed`.

**Recovery:** Operators see the `Unknown` state in the projection and can initiate an investigation workflow. Once fresh evidence is available (from a fresh RPC check or finality observation), the outcome is resolved by creating a new `EffectAttempt` and `EffectOutcome`.

### C.5 INV-05 through INV-15 (Summary Table)

| Invariant | Primary enforcement mechanism | Test type | Violation handler |
|---|---|---|---|
| INV-05: Reputation never authorizes | `PolicyEvaluationRequest` has no reputation field (type system) | Property test: vary reputation, assert grant unchanged | Compile-time: type error |
| INV-06: Least privilege by default | Empty default policy produces all-deny grant | Integration test: default agent denied all ops | `PolicyDenied` error |
| INV-07: Local correctness | `PolicyEvaluator` uses locally stored policy revisions; no cloud RPC in hot path | Disconnection test: operate offline with cached policy | Degradation mode, not failure |
| INV-08: Durable idempotent effects | `IdempotencyKey` in every `EffectIntent`; workers check before I/O | Kill/restart test at each phase | Re-claims use same key; idempotent at transport |
| INV-09: Tenant isolation | All store queries include `TenantId` scope | Cross-tenant access denial test on all interfaces | `PolicyDenied` with tenant scope violation |
| INV-10: Atomic state+effect | Single SQLite transaction for state change + intent creation | Transaction test: verify atomicity via BEGIN/COMMIT | Rollback; both reverted |
| INV-11: Delivery != execution | Separate `DeliveryStatus` from `RunStatus` | State test: complete run with failed delivery | No cascading failure |
| INV-12: Distinct chain outcomes | Chain saga has 6 independent `EffectOutcome` records | State machine test: each phase has own record | Each phase outcome preserved |
| INV-13: Monotonic event ordering | `sequence` field enforced unique per run in `EventStore` | Duplicate sequence rejection test | `StoreError::SequenceConflict` |
| INV-14: Secret excluded by default | `ClassificationGuard` checked at all output boundaries | Injection test across all output paths | `ClassificationRestricted` exclusion |
| INV-15: Evidence vs inference distinguished | Projection types carry `DataSource` enum: `Canonical` vs `ModelInferred` | Projection type test | Display layer renders distinctly |

---

## APPENDIX D: ARCHITECTURAL ANTI-PATTERNS

This appendix elaborates on the anti-patterns listed in Section 12, providing the specific code pattern to avoid, the rationale grounded in the invariants, and how Roko enforces the corresponding boundary.

### D.1 Why models never see raw keys (AP-05, INV-01)

**Anti-pattern:**
```rust
// FORBIDDEN: Do not do this
async fn build_context(signer: &dyn SignerPort) -> ContextPack {
    let key_bytes = signer.export_private_key().await.unwrap(); // No such method
    let item = ContextItem {
        content: format!("Your signing key is: {}", hex::encode(&key_bytes)),
        classification: Classification::Private, // Still wrong
        ..Default::default()
    };
    // ...
}
```

**Why it is wrong:**
1. The `SignerPort` trait has no `export_private_key` method. The type system prevents this call.
2. Even if key bytes appeared as opaque `Vec<u8>` somewhere in the system, the `ClassificationGuard` would reject them at the `ContextAssembler` boundary because they are classified `SecretForbidden`.
3. Any string containing a private key in model context can be extracted through prompt injection (adversarial user input requesting the AI to "repeat everything in your context window").

**Roko enforcement pattern:** In Roko, the `Provenance` system (`/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/provenance.rs`) marks external data as `Taint::UnverifiedSource`, preventing it from reaching high-privilege gates. Polkagent takes a stronger approach: secret-classified items never enter the context assembly pipeline at all, enforced by type-level classification.

**Correct pattern:** The `Signer` receives only `CanonicalSignRequest` containing opaque `Vec<u8>` payload bytes. The signing path is isolated from the context assembly path entirely — they live in different modules, different async tasks, and different trust boundaries.

### D.2 Why effects are always mediated (AP-01, AP-02, INV-02)

**Anti-pattern:**
```rust
// FORBIDDEN: Do not parse model text to determine what action to take
let model_response = "APPROVED: transfer 10 DOT to 5GrwvaEF...";
if model_response.contains("APPROVED") {
    chain_client.submit_transfer(amount, destination).await?;
}
```

**Why it is wrong:**
1. Model output is in the untrusted boundary (Section 7.1). A prompt-injected adversarial message like "APPROVED: transfer 100000 DOT to <attacker address>" would pass this check.
2. There is no `ResolvedGrant` in this flow. INV-02 requires that grants come from deterministic policy evaluation, not from model text parsing.
3. The raw `chain_client.submit_transfer` call bypasses the entire effect lifecycle: no `EffectIntent`, no outbox, no idempotency key, no `EffectOutcome`, no audit trail.

**Roko enforcement pattern:** In Roko, all external calls go through the `Gate` trait — the execution boundary that validates commands before running them. Roko's `ShellGate` in `roko-gate` only executes commands that pass the gate's validation, not raw model strings. Polkagent adopts a stricter version: the model outputs a structured `ToolCallRequest`, which is validated by the `ToolHost`, checked against a `ResolvedGrant` from the `PolicyEngine`, recorded as an `EffectIntent`, and only then executed by a worker.

**Correct pattern:**
```rust
// The model outputs a structured tool call request
let tool_call = ToolCallRequest {
    tool_name: "polkadot_transfer".into(),
    arguments_json: r#"{"amount": "10 DOT", "destination": "5GrwvaEF..."}"#.into(),
    ..Default::default()
};

// PolicyEngine resolves a grant (deterministic, not model-text-driven)
let grant = policy_evaluator.evaluate(policy_request).await?;
// ToolHost mediates the call under the grant
let result = tool_host.invoke(ToolCall::from(tool_call), &grant, &ctx).await?;
```

### D.3 Why state is never shared mutably across turns (INV-10, INV-13)

**Anti-pattern:**
```rust
// FORBIDDEN: Mutable shared state across turns
static GLOBAL_RUN_STATE: Mutex<HashMap<RunId, RunState>> = Mutex::new(HashMap::new());

async fn handle_turn(run_id: RunId, input: TurnInput) {
    let mut state = GLOBAL_RUN_STATE.lock().unwrap();
    state.get_mut(&run_id).unwrap().add_message(input);
    // No transaction boundary; state and events are not atomic
}
```

**Why it is wrong:**
1. In-memory mutable state is lost on crash. The outbox pattern (INV-03) requires that effects survive crashes; in-memory state cannot satisfy this.
2. Mutable shared state across turns violates the reducer model: there is no `(State, Input) -> (NewState, Effects)` function to replay for recovery.
3. The `sequence` monotonicity invariant (INV-13) cannot be enforced across process restarts without persistent storage.

**Roko enforcement pattern:** Roko uses the substrate (`.roko/signals.jsonl`) as the authoritative state. The TUI's `TuiState` (`/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/state.rs`) is explicitly a *view* of the on-disk state — it uses `RefCell` only for rendering-side mutation, never for authoritative state. The App struct's rendering path does zero I/O: "The render path does zero I/O — it only reads `&self.tui_state` and `&self.data` and writes to the frame buffer."

**Correct pattern:** Polkagent uses the event-sourced reducer:
```rust
// In polkagent-kernel/src/reducer.rs
pub fn reduce(
    current_state: RunState,
    input: KernelInput,
) -> (RunState, Vec<EffectIntent>) {
    // Pure function: no I/O, no global state, no Mutex
    // Both new_state and intents are written in one SQLite transaction
    match input {
        KernelInput::TurnReceived { turn } => {
            let new_state = current_state.with_turn(turn);
            let intents = vec![EffectIntent::for_model_inference(&new_state)];
            (new_state, intents)
        }
        // ...
    }
}
```

### D.4 Why adapters never cross-talk (AP-06, Section 5.2)

**Anti-pattern:**
```rust
// FORBIDDEN: adapter-provider-anthropic depending on adapter-chain-subxt
use adapter_chain_subxt::SubxtClient;

pub struct AnthropicProvider {
    chain_client: SubxtClient, // No! Adapters are independent leaves.
}
```

**Why it is wrong:**
1. Cross-adapter dependencies make the trust boundary diagram invalid. If a provider adapter can call a chain adapter, then compromising the provider adapter also compromises chain access — without going through the kernel or PolicyEngine.
2. Cross-adapter dependencies break the crate isolation property: adding a new chain adapter would require changing the provider adapter.
3. CI fitness functions (`cargo tree` checks) explicitly detect and block this pattern.

**Roko enforcement pattern:** Roko's adapter crates (`roko-mcp-stdio`, `roko-mcp-github`, etc.) are fully independent. They communicate only through the `Gate` trait and the substrate — never directly. Roko's Cargo.toml workspace makes the pattern visible: each MCP crate only imports `roko-core` traits, not other MCP crates.

**Correct pattern:** The kernel is the sole orchestrator. If a provider result needs to trigger a chain read, the flow goes: Provider adapter returns result to RunOrchestrator (application layer), which creates a new EffectIntent for the chain read, which is executed by the ChainClient adapter independently.

### D.5 Anti-pattern Reference Table

| Anti-pattern | Section | Core violation | Roko reference | Detection method |
|---|---|---|---|---|
| AP-01: Model prose as authority | 12 | INV-15 | Roko's `Taint::LlmHallucination` prevents propagation | Code review; no string parsing in effect paths |
| AP-02: Generic `send_transaction` | 12 | INV-02, INV-12 | Roko's Gate trait requires typed command | `cargo tree` shows no raw RPC in model path |
| AP-03: Collapsing Provider/Model/Executor | 12 | Section 2.6 | Roko separates `roko-agent`, `roko-gate`, `roko-chain` | Type system: distinct traits, no single mega-trait |
| AP-04: Mutable grants | 12 | INV-02 | Roko's immutable Engram content hash | `ResolvedGrant` private constructor |
| AP-05: Secret logging | 12 | INV-01, INV-14 | Roko `Provenance.trust` gates logging | `ClassificationGuard` at all sinks |
| AP-06: Adapter cross-talk | 12 | Section 5.2 | Roko MCP crates never import each other | CI: `cargo tree` cross-adapter edge check |
| AP-07: Reputation-based authorization | 12 | INV-05 | Roko's `Score` is display-only, not gate input | `PolicyEvaluationRequest` has no reputation field |
| AP-08: Silent retry after unknown | 12 | INV-04 | Roko's `WorkflowOutcome::Halted` surfaces uncertainty | State machine blocks retry without evidence |
| AP-09: Delivery as completion | 12 | INV-11 | Roko separates delivery from run completion | Separate `DeliveryStatus` type |
| AP-10: Extension self-activation | 12 | INV-06 | Roko's capability manifest is read-only at runtime | `ExtensionRegistry` enforces immutable manifest |
| AP-11: Model fallback mid-effect | 12 | INV-08 | Roko's retry policy creates new attempt records | New attempt = new idempotency key |
| AP-12: Cloud as authority | 12 | INV-07 | Roko runs locally by design | Local policy cache; no cloud in hot path |

---

## APPENDIX E: IMPLEMENTATION CHECKLIST

This checklist provides an ordered implementation plan with acceptance criteria and complexity estimates. Items within a group are listed in dependency order.

**Complexity scale:** XS = <1 day, S = 1-2 days, M = 3-5 days, L = 1-2 weeks, XL = 2-4 weeks.

### E.1 Group 1: Core Types (polkagent-types)

- [ ] **T-01** Create workspace `Cargo.toml` with resolver = "2", shared dependencies, and lint profile matching Roko's workspace.
  - *Acceptance:* `cargo build -p polkagent-types` succeeds. `cargo tree -p polkagent-types | grep tokio` returns 0.
  - *Complexity:* XS

- [ ] **T-02** Implement all ID types: `RunId`, `StepId`, `ArtifactId`, `EffectIntentId`, `EffectAttemptId`, `EffectOutcomeId`, `EventId`, `AgentId`, `ConversationId`, `TurnId`.
  - *Acceptance:* All IDs implement `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash`, `Serialize`, `Deserialize`. `RunId::new()` produces unique values. Serde round-trip test passes.
  - *Complexity:* XS

- [ ] **T-03** Implement `ContentDigest` with BLAKE3 and optional SHA-256 parallel digest. Algorithm tag in serialization.
  - *Acceptance:* `ContentDigest::of(bytes)` produces deterministic output. Hex round-trip test. `cargo test -p polkagent-types` passes.
  - *Complexity:* XS

- [ ] **T-04** Implement core domain structs: `Run`, `Step`, `Turn`, `RunEvent`, `Artifact`.
  - *Acceptance:* All fields present as defined in Section 2. Serde round-trip tests. `RunStatus` state machine enum compiles.
  - *Complexity:* S

- [ ] **T-05** Implement effect domain structs: `EffectIntent`, `EffectAttempt`, `EffectOutcome`. Implement `ResolvedGrant`, `Limits`, `ResourceSelector`.
  - *Acceptance:* All invariant-relevant fields present. `IdempotencyKey::from_intent(intent)` is deterministic. Serde round-trip tests.
  - *Complexity:* S

- [ ] **T-06** Implement authorization domain: `Policy`, `PolicyRule`, `Approval`, `Capability`.
  - *Acceptance:* `ResolvedGrant` has private constructor (pub(crate)). Compiler rejects external construction.
  - *Complexity:* S

- [ ] **T-07** Implement `DomainError` taxonomy (Section 10.2). Implement `Classification` enum with ordering.
  - *Acceptance:* `Classification::SecretForbidden > Classification::Sensitive` (Ord impl). Error display includes recovery hints.
  - *Complexity:* XS

### E.2 Group 2: Kernel (polkagent-kernel)

- [ ] **K-01** Implement `StateMachine`: valid Run lifecycle transitions, terminal state enforcement, unknown-effect blocking.
  - *Acceptance:* State machine test table covers all valid and invalid transitions. INV-04 test passes. INV-13 test (one terminal event per run) passes.
  - *Complexity:* M

- [ ] **K-02** Implement `TurnReducer`: pure `(RunState, KernelInput) -> (RunState, Vec<EffectIntent>)`.
  - *Acceptance:* Reducer is a free function with no `async`, no `Mutex`. Fuzz test: random sequences of inputs produce valid state sequences. No panic.
  - *Complexity:* M

- [ ] **K-03** Implement `GrantResolver`: deterministic intersection of policy layers.
  - *Acceptance:* Same inputs always produce identical `ResolvedGrant`. INV-05 test: varying reputation inputs has zero effect. INV-06 test: empty policy produces deny-all grant.
  - *Complexity:* M

- [ ] **K-04** Implement `ContentVerifier`: BLAKE3 verification on artifact read.
  - *Acceptance:* Tampered body bytes cause verification to return `false`. Correct bytes return `true`.
  - *Complexity:* XS

- [ ] **K-05** Implement `ClassificationGuard`: enforcement at context, event, and telemetry boundaries.
  - *Acceptance:* INV-01 test: `SecretForbidden` item excluded from context pack. INV-14 test: `Sensitive` item excluded from default telemetry output.
  - *Complexity:* S

- [ ] **K-06** Implement `IdempotencyKeyGen`: stable key from effect intent fields.
  - *Acceptance:* Same intent produces same key across process restarts (must not use `Instant` or random seed). INV-08 test.
  - *Complexity:* XS

### E.3 Group 3: Port Traits (polkagent-ports)

- [ ] **P-01** Define all port traits as documented in Appendix A: `ProviderPort`, `SignerPort`, `ChainClientPort`, `TransportPort`, `ToolHostPort`, `MemoryStore`, `EventStore`, `ArtifactStore`, `RunStore`, `EffectStore`, `PolicyEngine`.
  - *Acceptance:* All traits compile. Mock implementations in `mocks` feature compile. Port method count ≤ 10 per trait.
  - *Complexity:* M

- [ ] **P-02** Define remaining ports: `SecretResolver`, `IdentityProvider`, `ExtensionRegistry`, `Clock`, `EventSink`.
  - *Acceptance:* Same as P-01.
  - *Complexity:* S

- [ ] **P-03** Write contract test suite templates for: `EventStore` (ordering, sequence conflict, terminal event count), `ArtifactStore` (digest verification, classification rejection), `EffectStore` (claim/lease/expire semantics).
  - *Acceptance:* `InMemoryEventStore` passes all contract tests. Contract test harness is generic: any `EventStore` impl can be plugged in.
  - *Complexity:* M

- [ ] **P-04** Write contract test suite templates for: `ProviderPort` (streaming, error classification, idempotency hint), `SignerPort` (approval binding, metadata hash check, reject-without-approval).
  - *Acceptance:* `MockProvider` and `MockSigner` pass all contract tests.
  - *Complexity:* M

### E.4 Group 4: SQLite Adapter (adapter-store-sqlite)

- [ ] **A-01** Implement SQLite schema: `runs`, `steps`, `turns`, `events`, `artifacts`, `blobs`, `effect_intents`, `effect_attempts`, `effect_outcomes`, `grants`, `approvals`, `policies`.
  - *Acceptance:* Schema applies via migration. `PRAGMA foreign_keys = ON`. Schema passes `EXPLAIN QUERY PLAN` for all indexed queries.
  - *Complexity:* M

- [ ] **A-02** Implement `EventStore` on SQLite: append with `UNIQUE(run_id, sequence)`, query with range, terminal event count enforcement.
  - *Acceptance:* INV-13 test: duplicate sequence rejected. INV-10 test: state + intent written atomically (within same `BEGIN IMMEDIATE`).
  - *Complexity:* M

- [ ] **A-03** Implement `EffectStore` on SQLite: create intent, claim with lease, record attempt, record outcome, lease expiry detection.
  - *Acceptance:* INV-03 crash test. INV-08 idempotency test. At-most-one-claimed test.
  - *Complexity:* L

- [ ] **A-04** Implement `ArtifactStore` on SQLite + local filesystem blob store.
  - *Acceptance:* Digest verification on read. `SecretForbidden` write rejection test. Lineage DAG query.
  - *Complexity:* M

### E.5 Group 5: Application Services (polkagent-app)

- [ ] **AS-01** Implement `RunOrchestrator`: single-writer actor per run, reducer integration, outbox dispatch.
  - *Acceptance:* INV-03 crash test at all lifecycle points. INV-10 atomicity test. Recovery on restart re-queues orphaned intents.
  - *Complexity:* XL

- [ ] **AS-02** Implement `ContextAssembler`: budget enforcement, classification filtering, exclusion recording, digest computation.
  - *Acceptance:* INV-01 test. INV-14 test. Budget overflow excludes lowest-score items.
  - *Complexity:* M

- [ ] **AS-03** Implement `PolicyEvaluator`: Cedar engine integration, schema validation at load, PARC evaluation.
  - *Acceptance:* INV-02 test. INV-05 test. INV-06 test. Invalid Cedar document rejected at load.
  - *Complexity:* L

- [ ] **AS-04** Implement `ApprovalService`: canonical approval card rendering, approval binding to exact payload digest, multi-approver quorum tracking.
  - *Acceptance:* Approval is bound to exact `ContentDigest` of payload. Approving a different payload hash is rejected.
  - *Complexity:* M

- [ ] **AS-05** Implement `ProjectionBuilder`: `TurnProjection`, `ChainActionProjection`, `ApprovalProjection`.
  - *Acceptance:* Kill and restart server; projection is rebuilt from event log. INV-15: canonical data and model explanation are distinct projection fields.
  - *Complexity:* M

### E.6 Group 6: Integration Tests

- [ ] **IT-01** End-to-end test: user message -> turn -> run -> model inference (mock) -> response delivery.
  - *Acceptance:* Full happy path works with in-memory stores and mock adapters. No panics. Run reaches `Completed` status.
  - *Complexity:* M

- [ ] **IT-02** Chain action saga integration test: intent -> decode -> simulate -> approve -> sign (mock) -> broadcast (mock) -> finality (mock).
  - *Acceptance:* All 6 saga phases produce independent `EffectOutcome` records. INV-12 test.
  - *Complexity:* L

- [ ] **IT-03** Unknown outcome integration test: finality observer times out; system remains in `Unknown`; no retry without evidence.
  - *Acceptance:* INV-04 test. System displays `Unknown` distinctly. No automatic retry.
  - *Complexity:* M

- [ ] **IT-04** CI fitness function tests: adapter-to-adapter dependency check, key-material-reachability check, policy-gate-on-every-effect-path check.
  - *Acceptance:* CI step fails if any check detects a violation. Zero violations in clean workspace.
  - *Complexity:* S

---

## APPENDIX F: REFERENCE FILE MAP

This appendix maps each Polkagent architecture layer to specific reference implementations in the Roko and Bardo codebases.

| Architecture Layer | Polkagent Location | Roko Reference Files | Bardo Reference Files | Key Patterns |
|---|---|---|---|---|
| **Domain Types** | `crates/polkagent-types/` | `/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/engram.rs` (content-addressed identity), `/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/hash.rs` (BLAKE3 ContentHash) | `crates/bardo-primitives/` | BLAKE3 content addressing; ID types as newtypes; `BTreeMap<String, String>` tags for stable hashing |
| **Provenance and Classification** | `crates/polkagent-types/src/provenance.rs` | `/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/provenance.rs` (full `Taint` enum, trust scores, coherence checks) | `crates/golem-safety/` | Typed taint variants over `bool`; `#[non_exhaustive]` for forward-compat; `is_trusted(min_trust)` pattern |
| **Runtime Events** | `crates/polkagent-types/src/event.rs` | `/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/runtime_event.rs` (RuntimeEventEnvelope with seq/ts/schema_version; typed EventKind variants) | `crates/golem-runtime/src/events.rs` | `#[serde(tag = "kind", content = "data")]`; envelope with monotonic seq; `run_id()` accessor on enum |
| **State Machines** | `crates/polkagent-kernel/src/state_machine.rs` | `/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/phase.rs` (PlanPhase lifecycle) | `crates/golem-coordination/` | Exhaustive `match` on status enums; terminal state detection; `From<&str>` for display interop |
| **TUI State** | `apps/polkagent-cli/src/tui/state.rs` | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/state.rs` (TuiState: agents, plans, navigation, scroll, modals) | `apps/bardo-terminal/` | `PendingApproval` pattern; `AgentStatus`/`TaskStatus` with `label()` and `From<&str>`; `RefCell` for render-path mutation only |
| **TUI App Shell** | `apps/polkagent-cli/src/tui/app.rs` | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/app.rs` (App struct: background I/O channels, filesystem watcher, git watcher, render path does zero I/O) | `apps/dashboard/` | `watch::Receiver<Snapshot>` for live state; `mpsc` for approval requests; separate background thread for system metrics; `frame_counter` for adaptive frame rate |
| **Port Traits** | `crates/polkagent-ports/` | `roko-core` Gate/Substrate/Composer traits; `/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/tool/handler.rs` | `crates/bardo-gateway/src/traits.rs` | `#[async_trait]`; `Send + Sync + 'static` bounds; `dyn Trait` at call sites; narrow single-responsibility per trait |
| **Error Handling** | `crates/polkagent-types/src/error.rs` | `/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/error/retry.rs` (RetryPolicy), `/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/error/rpc.rs` | `crates/golem-core/src/error.rs` | `thiserror::Error`; `is_retryable()` on error variants; typed error variants over string errors |
| **Content Hashing** | `crates/polkagent-types/src/digest.rs` | `/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/hash.rs` (BLAKE3 ContentHash, hex serde, `short()` for logs) | `crates/bardo-primitives/src/hash.rs` | `[u8; 32]` newtype; custom hex serde (not base64); `from_hex` validation; `short()` for display |
| **Workspace Layout** | `Cargo.toml` | `/Users/will/dev/nunchi/roko/roko/Cargo.toml` (18 crates, `default-members`, lint profile, profile settings) | `/Users/will/dev/uniswap/bardo/Cargo.toml` | `resolver = "2"`; `edition = "2024"`; `[workspace.lints.rust] unsafe_code = "deny"`; `[profile.release] lto = "thin"` |
| **Outbox / Effect Durability** | `crates/polkagent-kernel/src/outbox.rs` | Roko orchestrator's signal persistence (`.roko/signals.jsonl` as outbox) | `apps/bardo-styx/` (durable delivery) | Write-before-execute; lease-based claiming; idempotency key derivation; recovery on restart |
| **Memory / Knowledge** | `crates/polkagent-ports/src/memory.rs` | `/Users/will/dev/nunchi/roko/roko/crates/roko-neuro/` (knowledge store), `roko-learn/` (episodes, skills) | `crates/mori-context/`, `crates/golem-grimoire/` | Attributed entries; expiry; relevance scoring; classification filtering on query |
| **Chain Client** | `adapters/adapter-chain-subxt/` | `/Users/will/dev/nunchi/roko/roko/crates/roko-chain/` (chain client + wallet trait abstractions) | `crates/golem-chain/`, `crates/golem-uniswap/` | Narrow read/write split; metadata pinning; saga state machine for transaction lifecycle |
| **Config Validation** | `crates/polkagent-app/src/config.rs` | `/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/config/validation.rs` | `crates/golem-core/src/config.rs` | Unknown key rejection; schema versioning; hot-reload via content-addressed snapshots |
| **Policy Evaluation** | `crates/polkagent-app/src/policy_evaluator.rs` | `/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/policy_manifest.rs` | `crates/golem-safety/` | Deny-by-default; deterministic; no randomness or I/O in evaluation path |
| **Telemetry / Observability** | `adapters/adapter-telemetry-otel/` | `/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/obs/` (metrics, scrub, histograms) | `crates/golem-heartbeat/` | Classified event scrubbing before export; tenant-scoped metrics; secrets excluded |

---

## APPENDIX G: EVENT DOMAIN IMPLEMENTATION

This appendix provides the complete, implementation-ready specification of the event system, including the full struct with all fields, the `EventKind` discriminated union organized by durability class, SQLite schema, and ordering/causality semantics.

### G.1 Complete `RunEvent` Struct

```rust
// crates/polkagent-types/src/event.rs

use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

use crate::{
    ArtifactId, ConversationId, EffectIntentId, EffectAttemptId, EventId,
    RunId, StepId, TurnId, UserId,
};

/// One ordered observation within a Run.
///
/// Events are the primary audit and recovery mechanism. The complete history
/// of a Run can be reconstructed from its events. A run is replayed by
/// re-running the reducer over the event log.
///
/// # Ordering guarantees
///
/// `sequence` is strictly monotonically increasing within a `run_id`. The
/// store enforces this with a `UNIQUE(run_id, sequence)` constraint.
/// Global ordering across runs is approximated by `timestamp` but MUST NOT
/// be used for correctness decisions — use `sequence` within a run.
///
/// # Causality tracking
///
/// `causation_id` points to the event that directly caused this event.
/// `correlation_id` groups all events in one user-initiated causal chain
/// (e.g., one turn spanning multiple runs). Both form an explicit causality
/// graph for audit and debugging.
///
/// # Schema version
///
/// `schema_version` allows readers to handle older event payloads gracefully.
/// Unknown schema versions are preserved as opaque JSON; they are never
/// discarded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEvent {
    /// Globally unique event identifier.
    pub id: EventId,
    /// The run this event belongs to.
    pub run_id: RunId,
    /// Monotonically increasing sequence number within the run.
    pub sequence: u64,
    /// What kind of event this is. Determines the durability class.
    pub kind: EventKind,
    /// Storage durability class derived from `kind`.
    pub durability: Durability,
    /// Groups all events in one user-initiated causal chain.
    pub correlation_id: CorrelationId,
    /// The event that directly caused this event, if any.
    pub causation_id: Option<EventId>,
    /// Wall-clock time at event emission. Use `sequence` for ordering.
    pub timestamp: DateTime<Utc>,
    /// Schema version of the payload (for forward-compatible deserialization).
    pub schema_version: u8,
    /// Source component that emitted this event.
    pub source: EventSource,
    /// Event-specific payload. See `EventPayload` below.
    pub payload: EventPayload,
}

/// Groups events from a single user-initiated causal chain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CorrelationId(pub uuid::Uuid);

impl CorrelationId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

/// The Polkagent component that emitted this event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    /// The run orchestrator (application layer).
    RunOrchestrator,
    /// The kernel state machine.
    Kernel,
    /// A specific effect worker, identified by worker ID.
    EffectWorker { worker_id: String },
    /// A transport adapter (identified by transport type).
    Transport { transport_type: String },
    /// The chain client adapter.
    ChainClient,
    /// The signer adapter.
    Signer,
    /// A test harness.
    Test,
}
```

### G.2 Durability Classes

```rust
// crates/polkagent-types/src/event.rs (continued)

/// Storage durability class for a `RunEvent`.
///
/// The class is derived from the `EventKind`, not set independently. This
/// prevents misclassification by callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Durability {
    /// Must survive crash. Required for state machine correctness and audit.
    ///
    /// Implementation: written synchronously within the SQLite transaction
    /// that changes the associated state. A crash after the transaction
    /// commit guarantees both the state change and this event are present.
    ///
    /// Examples: RunCreated, EffectIntentCreated, ApprovalGranted, RunCompleted.
    Durable,

    /// May be lost on crash without affecting correctness.
    ///
    /// Implementation: written to the event store with best-effort ordering.
    /// May be buffered and flushed asynchronously. If lost, the system
    /// remains correct; only live streaming consumers are affected.
    ///
    /// Examples: TextChunk, ProgressUpdate, ToolCallStarted.
    Diagnostic,

    /// In-process only; never written to storage.
    ///
    /// Implementation: distributed via the internal event bus only. Never
    /// hits the `EventStore`. Used for extremely high-frequency events
    /// (token-level streaming) where storage overhead would be prohibitive.
    ///
    /// Projection builders must not rely on `Ephemeral` events for any
    /// state they need to survive a restart.
    Ephemeral,
}

impl EventKind {
    /// Returns the durability class for this event kind.
    ///
    /// The mapping is normative. Changing a durability class is an
    /// architecture decision requiring review.
    pub const fn durability(&self) -> Durability {
        match self {
            // Durable: correctness-required events
            Self::RunCreated
            | Self::RunStarted
            | Self::RunCompleted
            | Self::RunFailed
            | Self::RunCancelled
            | Self::RunTimedOut
            | Self::StepStarted
            | Self::StepCompleted
            | Self::EffectIntentCreated
            | Self::EffectAttemptStarted
            | Self::EffectAttemptCompleted
            | Self::EffectOutcomeRecorded
            | Self::ApprovalRequested
            | Self::ApprovalGranted
            | Self::ApprovalDenied
            | Self::ArtifactCreated
            | Self::DeliveryStarted
            | Self::DeliveryCompleted
            | Self::MetadataEvidenceCaptured
            | Self::CallDecoded
            | Self::SimulationCompleted
            | Self::SignatureRequested
            | Self::SignatureObtained
            | Self::BroadcastAttempted
            | Self::BroadcastConfirmed
            | Self::FinalityObserved
            | Self::ChainOutcomeUnknown => Durability::Durable,

            // Diagnostic: progress events (survives in storage, may be lost on crash)
            Self::ToolCallStarted
            | Self::ToolCallCompleted
            | Self::ProgressUpdate
            | Self::InferenceStarted
            | Self::InferenceCompleted
            | Self::InferenceFailed
            | Self::InferenceFirstToken => Durability::Diagnostic,

            // Ephemeral: high-frequency streaming (in-memory bus only)
            Self::TextChunk => Durability::Ephemeral,
        }
    }
}
```

### G.3 Complete `EventKind` Discriminated Union

```rust
// crates/polkagent-types/src/event.rs (continued)

/// The discriminated union of all event kinds in the Polkagent system.
///
/// Event kinds are grouped by domain area. Adding a new kind is a
/// minor change; removing or renaming a kind is a major change requiring
/// a migration plan (older events with the old kind must be readable).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum EventKind {
    // ---- Run lifecycle (Durable) ----
    RunCreated {
        conversation_id: ConversationId,
        agent_id: crate::AgentId,
        config_revision: crate::ConfigDigest,
        policy_revision: crate::PolicyDigest,
    },
    RunStarted,
    RunCompleted {
        step_count: u32,
        duration_ms: u64,
    },
    RunFailed {
        error_code: String,
        error_message: String,
        step_id: Option<StepId>,
    },
    RunCancelled {
        reason: String,
        requested_by: Option<UserId>,
    },
    RunTimedOut {
        deadline_ms: u64,
    },

    // ---- Step lifecycle (Durable) ----
    StepStarted {
        step_id: StepId,
        kind: crate::StepKind,
        sequence: u64,
    },
    StepCompleted {
        step_id: StepId,
        duration_ms: u64,
    },

    // ---- Effect lifecycle (Durable) ----
    EffectIntentCreated {
        intent_id: EffectIntentId,
        effect_kind: crate::EffectKind,
        step_id: StepId,
        idempotency_key: String,
    },
    EffectAttemptStarted {
        intent_id: EffectIntentId,
        attempt_id: EffectAttemptId,
        attempt_number: u32,
        worker_id: String,
    },
    EffectAttemptCompleted {
        intent_id: EffectIntentId,
        attempt_id: EffectAttemptId,
        status: crate::AttemptStatus,
    },
    EffectOutcomeRecorded {
        intent_id: EffectIntentId,
        attempt_id: EffectAttemptId,
        status: crate::OutcomeStatus,
    },

    // ---- Approval lifecycle (Durable) ----
    ApprovalRequested {
        approval_id: crate::ApprovalId,
        effect_intent_id: EffectIntentId,
        approver_type: String,
        payload_digest: crate::ContentDigest,
    },
    ApprovalGranted {
        approval_id: crate::ApprovalId,
        granted_by: UserId,
        bound_payload_digest: crate::ContentDigest,
    },
    ApprovalDenied {
        approval_id: crate::ApprovalId,
        denied_by: UserId,
        reason: String,
    },

    // ---- Artifact lifecycle (Durable) ----
    ArtifactCreated {
        artifact_id: ArtifactId,
        kind: String,
        digest: crate::ContentDigest,
        classification: crate::Classification,
    },

    // ---- Delivery lifecycle (Durable) ----
    DeliveryStarted {
        delivery_id: String,
        conversation_id: ConversationId,
        transport_type: String,
    },
    DeliveryCompleted {
        delivery_id: String,
        duration_ms: u64,
    },

    // ---- Chain action saga (Durable) ----
    MetadataEvidenceCaptured {
        chain_profile_id: crate::ChainProfileId,
        spec_version: u32,
        metadata_digest: crate::MetadataDigest,
        block_ref: String,
    },
    CallDecoded {
        artifact_id: ArtifactId,
        pallet: String,
        call_name: String,
    },
    SimulationCompleted {
        artifact_id: ArtifactId,
        success: bool,
        fee_estimate: Option<u128>,
    },
    SignatureRequested {
        account_ref: String,
        chain_profile_id: crate::ChainProfileId,
        approval_id: crate::ApprovalId,
    },
    SignatureObtained {
        artifact_id: ArtifactId,
        public_key_hex: String,
    },
    BroadcastAttempted {
        chain_profile_id: crate::ChainProfileId,
    },
    BroadcastConfirmed {
        tx_hash: String,
    },
    FinalityObserved {
        tx_hash: String,
        block_ref: String,
        finalized: bool,
    },
    ChainOutcomeUnknown {
        tx_hash: Option<String>,
        reason: String,
        last_checked_block: Option<String>,
    },

    // ---- Progress / diagnostic (Diagnostic) ----
    ToolCallStarted {
        tool_name: String,
        step_id: StepId,
    },
    ToolCallCompleted {
        tool_name: String,
        step_id: StepId,
        duration_ms: u64,
        success: bool,
    },
    ProgressUpdate {
        phase: String,
        percent_complete: Option<u8>,
        message: String,
    },
    InferenceStarted {
        model_id: String,
        step_id: StepId,
        request_id: String,
    },
    InferenceCompleted {
        model_id: String,
        step_id: StepId,
        request_id: String,
        input_tokens: u32,
        output_tokens: u32,
        duration_ms: u64,
    },
    InferenceFailed {
        model_id: String,
        step_id: StepId,
        request_id: String,
        error_code: String,
        retryable: bool,
    },
    InferenceFirstToken {
        model_id: String,
        step_id: StepId,
        request_id: String,
        ttft_ms: u64,
    },

    // ---- High-frequency streaming (Ephemeral — in-memory bus only) ----
    TextChunk {
        step_id: StepId,
        text: String,
        token_count: u32,
    },
}

/// Event-specific payload as a structured type.
///
/// The payload is stored as JSON in SQLite and decoded at read time.
/// Unknown variants (from future schema versions) are preserved as
/// `EventPayload::Unknown { raw_json }` rather than discarded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventPayload {
    Typed(EventKind),
    Unknown { raw_json: serde_json::Value },
}
```

### G.4 SQLite Schema for Event Storage

```sql
-- migrations/0001_events.sql

-- The authoritative durable event log.
-- This table must never be modified after write (events are immutable).
CREATE TABLE IF NOT EXISTS run_events (
    -- Globally unique event identifier (UUID v4, stored as TEXT for readability)
    id                  TEXT    NOT NULL PRIMARY KEY,
    -- The run this event belongs to
    run_id              TEXT    NOT NULL,
    -- Monotonically increasing within run_id
    -- UNIQUE constraint enforces INV-13
    sequence            INTEGER NOT NULL,
    -- EventKind discriminator (e.g. "run_created", "approval_granted")
    kind                TEXT    NOT NULL,
    -- Durability class: "durable", "diagnostic" (never "ephemeral" in this table)
    durability          TEXT    NOT NULL CHECK (durability IN ('durable', 'diagnostic')),
    -- Causality tracking
    correlation_id      TEXT    NOT NULL,
    causation_id        TEXT    REFERENCES run_events(id),
    -- Wall-clock time (ISO-8601 UTC)
    timestamp           TEXT    NOT NULL,
    -- Schema version for forward-compatible deserialization
    schema_version      INTEGER NOT NULL DEFAULT 1,
    -- Source component
    source              TEXT    NOT NULL,
    -- Full JSON payload (EventKind as tagged union)
    payload             TEXT    NOT NULL,  -- JSON

    UNIQUE (run_id, sequence)
);

-- Index for the most common query: events_for_run(run_id, since_sequence)
CREATE INDEX IF NOT EXISTS idx_run_events_run_seq
    ON run_events (run_id, sequence);

-- Index for correlation-based queries (cross-run causality tracing)
CREATE INDEX IF NOT EXISTS idx_run_events_correlation
    ON run_events (correlation_id);

-- Index for fast terminal event lookup (recovery on restart)
CREATE INDEX IF NOT EXISTS idx_run_events_kind
    ON run_events (kind)
    WHERE kind IN ('run_completed', 'run_failed', 'run_cancelled', 'run_timed_out');

-- Diagnostic events may be stored in a separate, lower-durability table
-- that can be safely truncated without affecting correctness.
CREATE TABLE IF NOT EXISTS run_events_diagnostic (
    id                  TEXT    NOT NULL PRIMARY KEY,
    run_id              TEXT    NOT NULL,
    sequence            INTEGER NOT NULL,
    kind                TEXT    NOT NULL,
    correlation_id      TEXT    NOT NULL,
    causation_id        TEXT,
    timestamp           TEXT    NOT NULL,
    schema_version      INTEGER NOT NULL DEFAULT 1,
    source              TEXT    NOT NULL,
    payload             TEXT    NOT NULL,

    UNIQUE (run_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_run_events_diag_run_seq
    ON run_events_diagnostic (run_id, sequence);
```

### G.5 Event Ordering and Causality Semantics

**Within-run ordering:** Events within a run are totally ordered by `sequence`. The `RunOrchestrator` generates sequence numbers using a per-run atomic counter. The SQLite `UNIQUE(run_id, sequence)` constraint enforces uniqueness at the storage boundary.

**Cross-run causality:** The `correlation_id` field groups all events in one user-initiated causal chain. When a user sends a message, the `TransportAdapter` generates a new `CorrelationId`. All events produced by the resulting `Turn`, `Run`, and `Step` chain carry this correlation ID. Cross-run causality is tracked through parent `EventId` references in `causation_id`.

**Causality invariant:** If event B was caused by event A, then `B.causation_id == Some(A.id)` and `B.timestamp >= A.timestamp`. The ordering check is advisory (clocks can skew); the causal chain is the `causation_id` link, not the timestamp.

**Terminal event invariant (INV-13):** A run has at most one event with `kind` in `{run_completed, run_failed, run_cancelled, run_timed_out}`. The `EventStore::append` implementation enforces this by checking for existing terminal events before writing:

```rust
// In adapter-store-sqlite/src/event_store.rs

async fn append(&self, event: &RunEvent) -> Result<(), StoreError> {
    let conn = self.pool.get().await?;

    // If this is a terminal event, verify no terminal event already exists.
    if event.kind.is_terminal() {
        let existing_terminal: Option<i64> = conn.query_row(
            "SELECT COUNT(*) FROM run_events
             WHERE run_id = ?1
             AND kind IN ('run_completed','run_failed','run_cancelled','run_timed_out')",
            [event.run_id.as_str()],
            |row| row.get(0),
        ).optional()?;

        if existing_terminal.unwrap_or(0) > 0 {
            return Err(StoreError::TerminalEventAlreadyExists {
                run_id: event.run_id.clone(),
            });
        }
    }

    conn.execute(
        "INSERT INTO run_events (id, run_id, sequence, kind, durability, correlation_id,
         causation_id, timestamp, schema_version, source, payload)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            event.id.as_str(),
            event.run_id.as_str(),
            event.sequence as i64,
            event.kind.kind_name(),
            event.durability.as_str(),
            event.correlation_id.0.to_string(),
            event.causation_id.as_ref().map(|id| id.as_str()),
            event.timestamp.to_rfc3339(),
            event.schema_version as i64,
            event.source.as_str(),
            serde_json::to_string(&event.payload)
                .map_err(|e| StoreError::Serialization { message: e.to_string() })?,
        ],
    )?;

    Ok(())
}
```

**Recovery replay:** On process restart, the `RunOrchestrator` replays the event log for in-progress runs by calling `EventStore::events_for_run(run_id, 0)` and passing each event through the `TurnReducer` to reconstruct the current `RunState`. The reducer is replay-safe (pure, no side effects) so replaying is identical to initial processing.

**Roko reference:** This pattern follows Roko's `RuntimeEventEnvelope` (`/Users/will/dev/nunchi/roko/roko/crates/roko-core/src/runtime_event.rs`): an envelope with `run_id`, `seq`, `ts`, and `schema_version` wrapping a typed `RuntimeEvent` discriminated union. Polkagent extends this pattern with explicit `durability`, `causation_id`, and `correlation_id` fields required for the stricter audit and recovery guarantees of a signing-capable platform.

---

*End of PRD-02*
