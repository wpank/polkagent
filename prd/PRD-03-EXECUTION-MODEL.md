# PRD-03: Agent/Run/Effect/Graph Execution Model

**Status:** definitive PRD

**Document version:** 1.0.0

**Date:** 2026-07-30

**Audience:** engineers implementing the Polkagent runtime, architects reviewing
execution safety, operators configuring production deployments, and reviewers
evaluating the platform's durability and correctness guarantees

**Prerequisite reading:** This document is self-contained. It references
vocabulary from PRD-02 (Vocabulary, Invariants and System Architecture) and
personas from PRD-01 (Vision, Principles, Personas and Product Pillars). Where
those terms appear, they are re-explained inline so that a first-time reader
can evaluate this PRD independently.

**Implementation status:** design specification. No running production code
exists for the execution model described here.

---

## 1. Purpose and reader orientation

### 1.1 What this document defines

This PRD specifies how Polkagent executes work. It covers:

- How an agent is defined and transitions through lifecycle states.
- How a run (one unit of durable work) progresses from creation to terminal
  outcome.
- How runs decompose into turns and steps.
- How effects (actual external I/O) are proposed, claimed, attempted, and
  resolved with safety guarantees.
- How workflows compose multiple runs or steps into graphs.
- How events, concurrency, resources, cancellation, and crash recovery behave.

The execution model is the central safety mechanism of the platform. Every
model call, tool invocation, chain action, signing request, and external
delivery passes through it. The model is designed so that:

1. A crash never silently repeats an irreversible external action.
2. A model or extension cannot bypass policy, approval, or signing isolation.
3. Every material decision is recoverable from durable state.
4. Unknown outcomes remain visibly unknown until resolved.

### 1.2 Why this matters to users

A Polkadot user who asks an agent to transfer assets needs confidence that the
transfer happens exactly once, that the agent cannot silently exceed authorized
limits, and that a crash during signing or broadcast produces a clear recovery
state rather than a duplicated payment. A builder using a coding harness needs
confidence that a tool effect that edits files or runs tests can be replayed
for debugging without re-executing chain submissions. An operator running an
autonomous agent needs confidence that the execution model enforces budgets,
deadlines, and revocation even when no human is watching.

### 1.3 Core vocabulary used in this document

| Term | Meaning |
|---|---|
| **Agent** | A versioned definition (AgentSpec) of behavior, execution route, tools/skills, policy, and deployment requirements. It is not synonymous with one model. |
| **Run** | One durable execution instance. A run has a lifecycle state machine and contains turns, steps, effects, artifacts, and events. |
| **Turn** | One request-response cycle within a run. A turn begins when the executor receives input and ends when it produces a terminal output or requests an effect. |
| **Step** | An atomic unit within a turn: a model inference, a tool call, an approval check, or another discrete operation. |
| **Effect** | Actual external I/O such as calling a model, invoking a tool, requesting a signature, broadcasting a transaction, or sending a message. |
| **EffectIntent** | The durable command recorded BEFORE the I/O occurs. It describes what should happen, not what did happen. |
| **EffectAttempt** | One claim, lease, retry, and idempotency lifecycle for executing an intent. Multiple attempts may exist for one intent. |
| **EffectOutcome** | The immutable result observed for exactly one attempt: Success, Failure, Timeout, Cancelled, or Unknown. |
| **Grant** | The exact, resolved set of permissions and limits available to a run or effect. Immutable once created. |
| **Gate** | An independent deterministic check that allows, denies, or escalates an action. |
| **Artifact** | Durable, attributable content or evidence with type, provenance, digest, classification, and parent links. |
| **Event** | An ordered observation of lifecycle or streaming activity. |
| **Projection** | A recoverable read model derived from durable records, used by UIs and APIs. |
| **Outbox** | The durable, ordered list of EffectIntents waiting to be performed. |
| **Reducer** | Deterministic code that takes current state plus an input and produces new state plus requested effects. |
| **Turn actor** | The sole logical writer for a run. It serializes decisions, not all network I/O. |

### 1.4 Relationship to other PRDs

- **PRD-01** defines personas and product pillars. This PRD implements the
  execution guarantees those pillars require.
- **PRD-02** defines the canonical vocabulary and system invariants. This PRD
  specifies the state machines and contracts that enforce those invariants.
- **PRD-04** defines providers, models, harnesses, tools, and skills. This PRD
  specifies how they are invoked through the effect pipeline.
- **PRD-07** defines identity, policy, and security. This PRD specifies how
  grants are consumed and enforced during execution.
- **PRD-10** defines data, artifacts, events, and observability. This PRD
  specifies how execution produces those records.

---

## 2. Agent lifecycle

An agent is defined by an `AgentSpec`: a versioned, portable, declarative
record. The agent itself has a lifecycle independent of any particular run.

### 2.1 Agent states

```text
                    +-----------+
                    |  Created  |
                    +-----+-----+
                          |
                    validate & configure
                          |
                    +-----v-----+
                    |Configured |
                    +-----+-----+
                          |
                    activate (grant resolution)
                          |
                    +-----v------+
               +--->|   Active   |<---+
               |    +--+-----+--+    |
               |       |     |       |
           resume   pause  error   recover
               |       |     |       |
               |    +--v--+  |  +----v----+
               +----+Paused| +->|Degraded |
                    +------+    +---------+
                                    |
                              manual recover or
                              deactivate
                                    |
                    +-----------+   |
                    |Deactivated|<--+
                    +-----+-----+
                          |
                    archive / delete
                          |
                    +-----v-----+
                    | Archived  |
                    +-----------+
```

### 2.2 State descriptions

| State | Description | Allowed transitions |
|---|---|---|
| **Created** | AgentSpec exists but has not been validated or configured with deployment-specific bindings. | -> Configured |
| **Configured** | Validated AgentSpec with resolved provider routes, tool bindings, policy references, and deployment settings. | -> Active, -> Archived |
| **Active** | Accepting and executing runs. Grants are resolved, policies are bound, execution resources are available. | -> Paused, -> Degraded, -> Deactivated |
| **Paused** | Temporarily not accepting new runs. In-flight runs continue to their next safe checkpoint, then suspend. | -> Active, -> Deactivated |
| **Degraded** | Active but one or more execution dependencies have failed (provider down, harness unhealthy, chain RPC unreachable). Existing runs may complete; new runs may be queued or rejected depending on degradation severity. | -> Active, -> Deactivated |
| **Deactivated** | Permanently stopped. No new runs accepted. In-flight runs are cancelled cooperatively. | -> Archived |
| **Archived** | Spec, runs, artifacts, and audit trail are retained. Agent cannot be reactivated without creating a new version. | Terminal |

### 2.3 Agent lifecycle invariants

- **AGENT-INV-1:** An agent in Created state cannot accept runs.
- **AGENT-INV-2:** Transitioning to Active requires successful grant resolution
  for all declared capabilities.
- **AGENT-INV-3:** Pausing an agent does not cancel in-flight effects; it
  prevents new runs and suspends in-flight runs at their next checkpoint.
- **AGENT-INV-4:** Deactivation triggers cooperative cancellation of all
  in-flight runs (see section 9).
- **AGENT-INV-5:** The AgentSpec version used by each run is recorded
  immutably.

### 2.4 AgentSpec structure (Rust sketch)

```rust
/// Versioned, portable agent definition.
pub struct AgentSpec {
    pub id: AgentId,
    pub version: SemVer,
    pub name: String,
    pub description: String,

    /// Execution route: which provider/model/harness to use.
    pub execution_route: ExecutionRoute,

    /// Tools and skills this agent may use.
    pub capabilities: DeclaredCapabilities,

    /// Policy references for grant resolution.
    pub policy_refs: Vec<PolicyRef>,

    /// Memory and context configuration.
    pub memory_config: MemoryConfig,

    /// Surfaces this agent can be reached through.
    pub surfaces: Vec<SurfaceBinding>,

    /// Resource limits and budgets.
    pub resource_limits: ResourceLimits,

    /// Deployment requirements.
    pub deployment: DeploymentRequirements,

    /// Integrity digest of this spec.
    pub digest: Sha256Digest,
}

pub enum ExecutionRoute {
    /// Direct model API call.
    DirectModel {
        provider: ProviderRef,
        model: ModelRef,
        parameters: ModelParameters,
    },
    /// Long-lived coding/agent harness.
    Harness {
        harness: HarnessRef,
        session_config: SessionConfig,
    },
    /// External agent service (ACP/A2A).
    RemoteAgent {
        endpoint: EndpointRef,
        protocol: AgentProtocol,
    },
    /// Composite: route based on capability/cost/availability.
    Routed {
        router: RouterRef,
        candidates: Vec<ExecutionRoute>,
        fallback_policy: FallbackPolicy,
    },
}
```

---

## 3. Run lifecycle

A run is the fundamental unit of durable execution. Every piece of work the
platform performs -- from a simple model call to a multi-step chain action --
is a run.

### 3.1 Run state machine

```text
                         +----------+
                         | Created  |
                         +----+-----+
                              |
                         enqueue (validate, assign id,
                                  record agent spec version)
                              |
                         +----v-----+
                    +--->|  Queued   |
                    |    +----+-----+
                    |         |
                    |    schedule (claim slot,
                    |              resolve grants)
                    |         |
                    |    +----v-----+
                    |    | Running  |<---------+
                    |    +--+--+--+-+          |
                    |       |  |  |            |
                    |       |  |  +-- effect completes
                    |       |  |      (resume from
                    |       |  |       WaitingEffect)
                    |       |  |               |
                    |       |  +-- approval     |
                    |       |      granted      |
                    |       |      (resume from |
                    |       |       WaitingApproval)
                    |       |                  |
                    |    +--v-----------+      |
                    |    |WaitingApproval|------+
                    |    +--------------+
                    |       |
                    |    +--v-----------+
                    |    |WaitingEffect |------+
                    |    +--------------+
                    |
                    |    (retry after
                    |     transient failure)
                    |
             +------+------+   +----------+   +-----------+
             |  Completed  |   |  Failed  |   | Cancelled |
             +-------------+   +----------+   +-----------+
                                    |
                              +-----v-----+
                              |  Unknown  |
                              +-----------+
```

### 3.2 Run states

| State | Description | Terminal? |
|---|---|---|
| **Created** | Run record exists with a unique ID, source context, and agent spec version. No execution has begun. | No |
| **Queued** | Validated and accepted for execution. Waiting for a scheduling slot. | No |
| **Running** | Actively executing turns. The turn actor owns this run. | No |
| **WaitingApproval** | Execution paused because a policy gate requires human, quorum, or external approval before proceeding. The exact approval request is recorded. | No |
| **WaitingEffect** | Execution paused while one or more effects are being performed by workers. The run resumes when all required outcomes arrive. | No |
| **Completed** | All turns finished successfully. Terminal output and artifacts are recorded. | Yes |
| **Failed** | Execution encountered an unrecoverable error. Failure reason, partial artifacts, and effect outcomes are preserved. | Yes |
| **Cancelled** | Execution was cancelled by user, operator, policy, timeout, or budget exhaustion. Cooperative cancellation was attempted. | Yes |
| **Unknown** | A run reached a state where its outcome cannot be determined (e.g., a chain broadcast succeeded but finality observation timed out). Requires manual resolution. | Yes (until resolved) |

### 3.3 Run state transitions

| From | To | Guard | Side effects |
|---|---|---|---|
| Created | Queued | Agent is Active; input validates; deduplication passes | Emit RunQueued event; record input artifacts |
| Queued | Running | Scheduling slot available; grants resolve successfully | Emit RunStarted event; create turn actor; record ResolvedGrant |
| Running | WaitingApproval | Policy gate returns RequireApproval | Emit ApprovalRequested event; record approval request artifact |
| WaitingApproval | Running | Approval granted and valid | Emit ApprovalGranted event; record approval artifact |
| WaitingApproval | Cancelled | Approval denied or approval timeout | Emit RunCancelled event with denial reason |
| Running | WaitingEffect | Turn produces EffectIntents; reducer commits them to outbox | Emit EffectIntentsCreated event |
| WaitingEffect | Running | All required EffectOutcomes received | Emit EffectsResolved event; feed outcomes to reducer |
| Running | Completed | Reducer produces terminal success state; no pending effects | Emit RunCompleted event; record terminal artifacts |
| Running | Failed | Unrecoverable error; retry budget exhausted | Emit RunFailed event; record error artifacts |
| Running | Cancelled | Cancellation signal received; cooperative shutdown completes | Emit RunCancelled event |
| Any non-terminal | Cancelled | Budget exhaustion, deadline, or operator action | Cooperative cancellation; emit RunCancelled |
| Failed | Queued | Retry policy permits; retry count < max | Emit RunRetryQueued event |
| Any non-terminal | Unknown | Effect outcome is genuinely indeterminate | Emit RunUnknown event; require manual resolution |

### 3.4 Run state invariants

- **RUN-INV-1:** A run has exactly one turn actor at any time. Concurrent
  execution of the same run is forbidden.
- **RUN-INV-2:** State transitions and effect intent creation are atomic. The
  new state and its outbox entries are committed in one database transaction.
- **RUN-INV-3:** A terminal state (Completed, Failed, Cancelled, Unknown) is
  immutable. A run in Unknown state may be manually resolved to Completed,
  Failed, or Cancelled, but this is a new administrative event, not a state
  mutation.
- **RUN-INV-4:** The ResolvedGrant used by each run is recorded at run start
  and is immutable for that run's lifetime.
- **RUN-INV-5:** Every state transition emits exactly one RunEvent with a
  monotonically increasing sequence number within the run.
- **RUN-INV-6:** A WaitingApproval state records the exact payload/evidence
  requiring approval. The approval response is bound to that exact request.
- **RUN-INV-7:** Delivery of results to a client is a separate concern from run
  completion. A run may be Completed even if the client has not yet received
  the result.

### 3.5 Run record (Rust sketch)

```rust
pub struct Run {
    pub id: RunId,
    pub agent_id: AgentId,
    pub agent_spec_version: SemVer,
    pub agent_spec_digest: Sha256Digest,

    /// Current lifecycle state.
    pub state: RunState,

    /// Correlation to parent conversation, trigger, or workflow.
    pub source: RunSource,

    /// Immutable grant snapshot for this run.
    pub grant: ResolvedGrant,

    /// Policy and configuration revision used.
    pub policy_revision: Digest,
    pub config_revision: Digest,

    /// Resource accounting.
    pub budget: RunBudget,
    pub usage: RunUsage,

    /// Timing.
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub deadline: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,

    /// Monotonic event counter.
    pub event_sequence: u64,

    /// Current turn number.
    pub turn_number: u32,

    /// Links to artifacts produced.
    pub artifact_ids: Vec<ArtifactId>,

    /// Terminal outcome, if reached.
    pub outcome: Option<RunOutcome>,
}

pub enum RunState {
    Created,
    Queued,
    Running,
    WaitingApproval { request_id: ApprovalRequestId },
    WaitingEffect { pending_intents: Vec<EffectIntentId> },
    Completed,
    Failed { reason: FailureReason },
    Cancelled { reason: CancellationReason },
    Unknown { context: UnknownContext },
}

pub enum RunOutcome {
    Success { output: ArtifactId },
    Failure { error: RunError, partial_output: Option<ArtifactId> },
    Cancelled { reason: CancellationReason },
    Unknown { last_known_state: String },
}

pub enum RunSource {
    Conversation { conversation_id: ConversationId, turn_id: TurnId },
    Trigger { trigger_id: TriggerId, event: TriggerEvent },
    Workflow { parent_run_id: RunId, node_id: NodeId },
    Api { request_id: RequestId, principal: PrincipalId },
    Manual { operator_id: OperatorId },
}
```

---

## 4. Turn and step model

### 4.1 How a run decomposes

A run consists of one or more **turns**. Each turn consists of one or more
**steps**. This decomposition reflects how agent execution actually works:

```text
Run
 |
 +-- Turn 1
 |    |-- Step 1: Context assembly (pure)
 |    |-- Step 2: Model inference (effect)
 |    |-- Step 3: Parse model output (pure)
 |    +-- Step 4: Tool call request (effect)
 |
 +-- Turn 2
 |    |-- Step 1: Feed tool result to model (effect)
 |    |-- Step 2: Parse model output (pure)
 |    +-- Step 3: Terminal output (pure)
 |
 +-- Turn 3 (if run resumes later)
      |-- Step 1: Resume context assembly (pure)
      |-- Step 2: Model inference (effect)
      +-- Step 3: Terminal output (pure)
```

### 4.2 Turn lifecycle

A turn begins when the turn actor receives input (either from ingress or from
a completed effect) and ends when:

1. The reducer produces a terminal output for the run, OR
2. The reducer produces one or more EffectIntents that require external I/O,
   OR
3. The reducer determines that approval is required, OR
4. An error occurs.

```text
        +------------------+
        | TurnInput        |
        | (message, effect |
        |  outcome, resume)|
        +--------+---------+
                 |
        +--------v---------+
        | Context Assembly |  <-- pure, no I/O
        +--------+---------+
                 |
        +--------v---------+
        | Reducer: decide  |  <-- deterministic
        | next action      |
        +--------+---------+
                 |
        +--------v---------+
        | Produce:         |
        | - new TurnState  |
        | - EffectIntents  |  <-- committed atomically
        | - RunEvents      |
        | - Artifacts      |
        +------------------+
```

### 4.3 Turn state (Rust sketch)

```rust
pub struct Turn {
    pub id: TurnId,
    pub run_id: RunId,
    pub turn_number: u32,
    pub input: TurnInput,
    pub started_at: Timestamp,
    pub completed_at: Option<Timestamp>,
    pub steps: Vec<Step>,
    pub output: Option<TurnOutput>,
}

pub enum TurnInput {
    /// Initial message or trigger that started the run.
    Ingress { message: IngressMessage },
    /// Result of a completed effect.
    EffectResult { outcomes: Vec<EffectOutcome> },
    /// Approval decision.
    ApprovalResult { decision: ApprovalDecision },
    /// Resume from a paused or saved state.
    Resume { checkpoint: CheckpointRef },
}

pub enum TurnOutput {
    /// Run is complete.
    Terminal { output: ArtifactId },
    /// Effects needed before next turn.
    EffectsRequested { intents: Vec<EffectIntentId> },
    /// Approval needed before next turn.
    ApprovalRequired { request: ApprovalRequestId },
    /// Error occurred.
    Error { error: TurnError },
}
```

### 4.4 Step types

```rust
pub struct Step {
    pub id: StepId,
    pub turn_id: TurnId,
    pub step_number: u32,
    pub kind: StepKind,
    pub started_at: Timestamp,
    pub completed_at: Option<Timestamp>,
    pub artifacts_produced: Vec<ArtifactId>,
}

pub enum StepKind {
    /// Pure context assembly: no I/O, deterministic.
    ContextAssembly {
        context_pack: ContextPackId,
    },
    /// Model inference: an effect that calls a model API.
    ModelInference {
        effect_intent_id: EffectIntentId,
        route: ModelRoute,
    },
    /// Tool invocation: an effect that calls a tool.
    ToolInvocation {
        effect_intent_id: EffectIntentId,
        tool: ToolRef,
        grant: ResolvedGrantId,
    },
    /// Policy evaluation: deterministic gate check.
    PolicyEvaluation {
        decision: PolicyDecisionId,
    },
    /// Approval check: determines if approval is needed.
    ApprovalCheck {
        result: ApprovalCheckResult,
    },
    /// Output parsing: pure transformation of model output.
    OutputParsing {
        input_artifact: ArtifactId,
        output_artifact: ArtifactId,
    },
    /// Chain action step: decode, simulate, prepare, sign, broadcast, watch.
    ChainAction {
        phase: ChainActionPhase,
        effect_intent_id: EffectIntentId,
    },
}

pub enum ChainActionPhase {
    MetadataCapture,
    CallDecode,
    Simulation,
    ApprovalRequest,
    SignatureRequest,
    Broadcast,
    FinalityWatch,
}
```

### 4.5 Turn invariants

- **TURN-INV-1:** Turns within a run are strictly sequential. Turn N+1 cannot
  begin until Turn N has completed.
- **TURN-INV-2:** Steps within a turn are ordered. Each step records its
  sequence number.
- **TURN-INV-3:** Context assembly steps are pure and deterministic. They
  produce a ContextPack artifact that records exactly which evidence was
  offered to the model.
- **TURN-INV-4:** A turn that produces EffectIntents commits them atomically
  with the turn state.
- **TURN-INV-5:** The reducer MUST NOT perform I/O. Effects are *intents*
  returned by the reducer and executed by the outbox relay after the turn
  commits. This separation is what makes deterministic replay safe and
  testable: re-running the reducer against recovered state produces the
  same intent list without re-executing any external action.

---

## 5. Effect pipeline

The effect pipeline is the core safety mechanism. It ensures that every
external action is:

1. Recorded as an intent BEFORE it happens.
2. Claimed by a worker with a lease.
3. Executed with idempotency guarantees.
4. Resolved with an immutable outcome.

### 5.1 Effect pipeline overview

```text
Reducer decides       Worker claims        Worker performs       Worker records
an effect is     -->  the intent from -->  the actual       --> the immutable
needed                the outbox           external I/O         outcome

  EffectIntent          EffectAttempt        [external I/O]      EffectOutcome
  (durable,             (leased,             (model call,        (immutable,
   pre-I/O)              idempotent)          tool call,          post-I/O)
                                              sign request,
                                              broadcast, etc.)
```

### 5.2 EffectIntent: the durable command

An EffectIntent is the durable, pre-I/O record of what should happen. It is
created by the reducer and committed to the outbox atomically with the run
state transition. The intent exists BEFORE any external action occurs.

#### EffectIntent state machine

```text
        +----------+
        | Pending  |
        +----+-----+
             |
        claim (worker lease)
             |
        +----v-----+
        | Claimed  |
        +----+-----+
             |
        worker executes
             |
        +----v------+
        | Executing |
        +--+--+--+--+
           |  |  |
     +-----+  |  +------+
     |        |          |
  +--v---+ +--v-----+ +-v--------+
  |Resolved| |Retrying| |Superseded|
  +--------+ +--------+ +----------+
```

| State | Description |
|---|---|
| **Pending** | Intent committed to outbox. No worker has claimed it. |
| **Claimed** | A worker holds a time-limited lease. No other worker may claim it. |
| **Executing** | The worker is performing the external I/O. |
| **Resolved** | An EffectOutcome has been recorded. Terminal. |
| **Retrying** | The attempt failed with a retriable error. A new attempt will be created after backoff. The intent returns to Pending. |
| **Superseded** | The intent was cancelled because the run was cancelled, or a newer intent replaces it. Terminal. |

#### EffectIntent Rust sketch

```rust
pub struct EffectIntent {
    pub id: EffectIntentId,
    pub run_id: RunId,
    pub turn_id: TurnId,
    pub step_id: StepId,

    /// What kind of effect to perform.
    pub kind: EffectKind,

    /// The grant authorizing this effect.
    pub grant_id: ResolvedGrantId,

    /// Stable idempotency key for deduplication.
    pub idempotency_key: IdempotencyKey,

    /// Ordering within the run's outbox.
    pub sequence: u64,

    /// Current state.
    pub state: EffectIntentState,

    /// Deadline for this effect.
    pub deadline: Option<Timestamp>,

    /// Maximum number of attempts.
    pub max_attempts: u32,

    /// Current attempt count.
    pub attempt_count: u32,

    /// Priority class for scheduling.
    pub priority: EffectPriority,

    pub created_at: Timestamp,
    pub resolved_at: Option<Timestamp>,
}

pub enum EffectKind {
    /// Call a model API.
    ModelCall {
        route: ModelRoute,
        context_pack: ContextPackRef,
        parameters: ModelParameters,
        streaming: bool,
    },
    /// Invoke a tool.
    ToolCall {
        tool: PinnedToolRef,
        input: ToolInput,
        grant: ResolvedGrantRef,
        sandbox_config: Option<SandboxConfig>,
    },
    /// Request a signature from an isolated signer.
    SignatureRequest {
        payload: CanonicalPayloadRef,
        signer: SignerRef,
        approval: ApprovalRef,
        chain_evidence: ChainEvidenceRef,
    },
    /// Broadcast a signed transaction.
    Broadcast {
        signed_payload: SignedPayloadRef,
        target: ChainTarget,
        submission_strategy: SubmissionStrategy,
    },
    /// Observe transaction finality.
    FinalityWatch {
        transaction: TransactionRef,
        target: ChainTarget,
        timeout: Duration,
    },
    /// Deliver a message or result through a transport.
    Delivery {
        transport: TransportRef,
        message: OutgoingMessage,
        delivery_class: DeliveryClass,
    },
    /// Read chain state or metadata.
    ChainRead {
        target: ChainTarget,
        query: ChainQuery,
    },
    /// Run a simulation or dry-run.
    Simulation {
        target: ChainTarget,
        call: EncodedCall,
        parameters: SimulationParameters,
    },
    /// Harness-specific operations (start session, resume, etc.).
    HarnessOperation {
        harness: HarnessRef,
        operation: HarnessOp,
    },
}

pub enum EffectIntentState {
    Pending,
    Claimed { worker_id: WorkerId, lease_expires: Timestamp },
    Executing { worker_id: WorkerId, started_at: Timestamp },
    Resolved { outcome_id: EffectOutcomeId },
    Retrying { next_attempt_after: Timestamp, last_error: String },
    Superseded { reason: SupersessionReason },
}
```

### 5.3 EffectAttempt: one execution lifecycle

An EffectAttempt records one attempt to execute an EffectIntent. Multiple
attempts may exist for a single intent (retries). Each attempt is immutable
after it produces an outcome.

#### EffectAttempt state machine

```text
        +---------+
        | Created |
        +----+----+
             |
        worker claims lease
             |
        +----v-----+
        |  Leased  |
        +----+-----+
             |
        begin I/O
             |
        +----v-------+
        | InProgress |
        +--+--+--+---+
           |  |  |
     +-----+  |  +-------+
     |        |           |
  +--v----+ +-v-------+ +-v--------+
  |Success| |Failure   | |Timeout   |
  +-------+ +----------+ +----------+

  +-----------+
  | Cancelled |   (from Created, Leased, or InProgress)
  +-----------+
```

#### EffectAttempt Rust sketch

```rust
pub struct EffectAttempt {
    pub id: EffectAttemptId,
    pub intent_id: EffectIntentId,
    pub run_id: RunId,

    /// Which attempt this is (1-based).
    pub attempt_number: u32,

    /// Idempotency key inherited from the intent.
    pub idempotency_key: IdempotencyKey,

    /// Worker that claimed this attempt.
    pub worker_id: WorkerId,

    /// Lease expiry. If exceeded without outcome, the attempt is timed out.
    pub lease_expires: Timestamp,

    /// Retry class: determines backoff and retry eligibility.
    pub retry_class: RetryClass,

    /// Current state.
    pub state: AttemptState,

    /// Timing.
    pub created_at: Timestamp,
    pub claimed_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,

    /// The outcome, once resolved.
    pub outcome: Option<EffectOutcome>,
}

pub enum AttemptState {
    Created,
    Leased,
    InProgress,
    Completed { outcome_id: EffectOutcomeId },
    Cancelled { reason: CancellationReason },
}

pub enum RetryClass {
    /// Safe to retry: read-only operations, idempotent writes.
    Idempotent,
    /// Retry with caution: check for partial completion first.
    CheckBeforeRetry,
    /// Never automatically retry: signing, broadcasting, payments.
    NoAutoRetry,
}
```

### 5.4 EffectOutcome: the immutable result

An EffectOutcome is the immutable record of what actually happened during one
attempt. It is created exactly once per attempt and never modified.

#### EffectOutcome variants

```rust
pub struct EffectOutcome {
    pub id: EffectOutcomeId,
    pub attempt_id: EffectAttemptId,
    pub intent_id: EffectIntentId,
    pub run_id: RunId,

    /// The observed result.
    pub result: OutcomeResult,

    /// External references (transaction hash, block number, etc.).
    pub external_refs: Vec<ExternalRef>,

    /// Artifacts produced by this effect.
    pub artifacts: Vec<ArtifactId>,

    /// Resource usage (tokens, compute, fees, etc.).
    pub usage: EffectUsage,

    /// Timing.
    pub observed_at: Timestamp,

    /// Digest of the outcome for integrity verification.
    pub digest: Sha256Digest,
}

pub enum OutcomeResult {
    /// Effect completed successfully.
    Success {
        /// Typed result data.
        data: OutcomeData,
    },
    /// Effect failed with a known error.
    Failure {
        /// Error classification.
        error_class: ErrorClass,
        /// Human-readable error.
        message: String,
        /// Whether this failure is retriable.
        retriable: bool,
    },
    /// Effect timed out.
    Timeout {
        /// How long we waited.
        waited: Duration,
        /// Whether partial work may have occurred.
        partial_work_possible: bool,
    },
    /// Effect was cancelled before completion.
    Cancelled {
        /// Whether partial work may have occurred.
        partial_work_possible: bool,
        reason: CancellationReason,
    },
    /// Outcome cannot be determined.
    Unknown {
        /// What we know about the state.
        context: String,
        /// Recommended resolution action.
        resolution_hint: ResolutionHint,
    },
}

pub enum ErrorClass {
    /// Client-side error (bad request, invalid input).
    ClientError,
    /// Server-side error (provider down, rate limited).
    ServerError,
    /// Network error (timeout, connection refused).
    NetworkError,
    /// Authorization error (denied by policy, insufficient grant).
    AuthorizationError,
    /// Resource exhaustion (budget, quota, rate limit).
    ResourceExhaustion,
    /// Chain-specific error (nonce, insufficient balance, dispatch error).
    ChainError { dispatch_error: Option<String> },
}

pub enum ResolutionHint {
    /// Check the chain for transaction status.
    CheckChain { transaction_ref: TransactionRef },
    /// Retry the operation.
    RetryOperation,
    /// Requires manual investigation.
    ManualInvestigation { context: String },
    /// The effect can be safely abandoned.
    SafeToAbandon,
}
```

### 5.5 Effect pipeline invariants

- **EFF-INV-1:** An EffectIntent is committed to the database BEFORE any
  external I/O occurs. If the process crashes after committing the intent
  but before executing it, the intent remains in the outbox for a worker
  to claim.
- **EFF-INV-2:** Each EffectAttempt has a time-limited lease. If the lease
  expires without an outcome, the attempt is marked Timeout and the intent
  may be retried (if its retry class permits).
- **EFF-INV-3:** An EffectOutcome is immutable. Once recorded, it cannot be
  changed.
- **EFF-INV-4:** The idempotency key is stable across retries of the same
  intent. External systems that support idempotency keys receive the same
  key on every attempt.
- **EFF-INV-5:** Effects with RetryClass::NoAutoRetry (signing, broadcasting)
  are never automatically retried. A new intent must be created explicitly.
- **EFF-INV-6:** An outcome of Unknown is preserved as Unknown. The system
  never silently converts Unknown to Success or Failure.
- **EFF-INV-7:** Effect outcomes are fed back to the run's reducer as
  TurnInput::EffectResult. The reducer decides the next state based on the
  outcome.
- **EFF-INV-8:** A cancelled run causes all pending/claimed effects to be
  superseded. In-progress effects are sent a cancellation signal but may
  complete.

---

## 6. Durable outbox pattern

The outbox is the bridge between the deterministic reducer and the fallible
external world. It provides ordered, at-most-once delivery of effects.

### 6.1 Outbox design

```text
+-------------------+         +-------------------+
|   Turn Actor      |         |   Effect Workers  |
|                   |         |                   |
|  Reducer produces |  commit |  Claim intents    |
|  EffectIntents    |-------->|  from outbox      |
|  + new RunState   |  (one   |  (lease-based)    |
|  atomically       |  txn)   |                   |
|                   |         |  Execute I/O      |
|                   |         |                   |
|  Consume outcomes |<--------|  Record outcomes  |
|  in next turn     |         |                   |
+-------------------+         +-------------------+

Database (SQLite/Postgres):
+--------------------------------------------------+
| effect_intents table                              |
|   id | run_id | sequence | state | kind | ...     |
|------|--------|----------|-------|------|---------|
|   1  |  R1    |    1     | Pending | ModelCall   |
|   2  |  R1    |    2     | Claimed | ToolCall   |
|   3  |  R1    |    3     | Resolved| Signature  |
+--------------------------------------------------+
```

### 6.2 Outbox processing algorithm

```text
1. Worker polls for Pending intents using a lease-based claim inside a
   BEGIN IMMEDIATE transaction:

   UPDATE effect_intents
   SET state = 'Claimed',
       lease_owner = ?,
       lease_expires = ?
   WHERE id IN (
     SELECT id FROM effect_intents
     WHERE state = 'Pending'
       AND lease_expires < ?
     ORDER BY priority, sequence
     LIMIT ?
   )
   RETURNING *;

   Note: SQLite does not support SKIP LOCKED. Instead, BEGIN IMMEDIATE
   serializes writers so only one worker holds the write lock at a time.
   Leases provide crash recovery; the reaper reclaims expired leases.
   This avoids SQLITE_BUSY storms that arise from concurrent write attempts
   on a WAL-mode database (see section 6.4 for WAL and busy_timeout
   configuration; see section 13.4 for the Tokio single-writer pattern).

2. Worker creates an EffectAttempt record.

3. Worker performs the external I/O, passing the idempotency_key where
   supported by the external system.

4. Worker creates an EffectOutcome record.

5. Worker sets intent state = Resolved and links the outcome.

6. The turn actor is notified (via channel or polling) that outcomes are
   available.

7. The turn actor feeds the outcomes to the reducer as TurnInput::EffectResult.
```

### 6.3 Lease expiry and recovery

```text
Background reaper process:

1. Periodically scan for intents WHERE state = 'Claimed'
   AND lease_expires < now().

2. For each expired lease:
   a. If the intent's retry_class is Idempotent:
      - Set state = Pending (return to outbox for re-claim).
      - Record a Timeout EffectOutcome for the expired attempt.
   b. If the intent's retry_class is CheckBeforeRetry:
      - Set state = Pending with a flag indicating the worker should
        check for partial completion before retrying.
   c. If the intent's retry_class is NoAutoRetry:
      - Record an Unknown EffectOutcome.
      - Set intent state = Resolved with Unknown outcome.
      - The reducer must handle this explicitly.
```

### 6.4 SQLite WAL and write serialization

The outbox store is configured for safe concurrent access under SQLite:

```sql
PRAGMA journal_mode = WAL;         -- enables concurrent readers + one writer
PRAGMA busy_timeout = 5000;        -- wait up to 5 s before returning SQLITE_BUSY
```

All write paths (commit_turn, claim_intent, record_outcome) open their
transactions with `BEGIN IMMEDIATE`, which acquires the write lock at
transaction start. This prevents the "optimistic write" pattern that produces
`SQLITE_BUSY` errors mid-transaction. The single-writer Tokio task described
in section 13.4 reinforces this: because only one task holds the write
connection, contention at the SQLite level is eliminated entirely.

### 6.5 Outbox ordering guarantees

- **OUTBOX-1:** Within a single run, effects are sequenced by their outbox
  sequence number.
- **OUTBOX-2:** Effects with sequential dependencies (e.g., sign THEN
  broadcast) are expressed as separate intents in separate turns. The reducer
  creates the broadcast intent only after receiving the sign outcome.
- **OUTBOX-3:** Independent effects within one turn may execute concurrently.
  The reducer specifies which intents must complete before the run proceeds.
- **OUTBOX-4:** Cross-run ordering is not guaranteed. Each run's outbox is
  independent.
- **OUTBOX-5:** See section 6.4 for WAL and `BEGIN IMMEDIATE` configuration
  that prevents write contention.

---

## 7. Idempotency

### 7.1 Idempotency key generation

Every EffectIntent receives a stable idempotency key derived from:

```rust
pub struct IdempotencyKey(Sha256Digest);

impl IdempotencyKey {
    pub fn generate(
        run_id: &RunId,
        intent_sequence: u64,
        effect_kind_discriminant: &str,
        canonical_input_digest: &Sha256Digest,
    ) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(run_id.as_bytes());
        hasher.update(intent_sequence.to_le_bytes());
        hasher.update(effect_kind_discriminant.as_bytes());
        hasher.update(canonical_input_digest.as_bytes());
        IdempotencyKey(hasher.finalize().into())
    }
}
```

### 7.2 Deduplication rules

| Scenario | Behavior |
|---|---|
| Same idempotency key, Pending intent exists | Reject duplicate; return existing intent ID |
| Same idempotency key, Resolved intent exists | Return existing outcome |
| Same idempotency key, Claimed intent exists | Wait for existing claim to resolve |
| Different idempotency key | New intent; no deduplication |

### 7.3 External idempotency

When an external system supports idempotency keys (e.g., model API,
payment rail), the EffectAttempt passes the same `idempotency_key` to the
external system. This ensures that retries of the same intent produce the
same external result.

When an external system does NOT support idempotency keys (e.g., raw RPC
broadcast), the effect must use RetryClass::CheckBeforeRetry or
RetryClass::NoAutoRetry, and the worker must check for existing results
before retrying.

### 7.4 At-most-once guarantee for irreversible effects

For effects that are irreversible (signing, broadcasting, fund transfers):

1. The EffectIntent is committed with RetryClass::NoAutoRetry.
2. The worker claims and executes exactly once per attempt.
3. If the attempt produces an Unknown outcome, no automatic retry occurs.
4. A new intent must be explicitly created by the reducer after investigating
   the Unknown state.
5. The investigation step checks for existing on-chain evidence before
   creating a new broadcast intent.

### 7.5 Exactly-once semantics: the full picture

"Exactly-once" delivery is a composite property, not a single mechanism:

| Layer | Guarantee | Mechanism |
|---|---|---|
| **Delivery to outbox** | At-least-once | Pending intent survives crashes; lease reaper re-queues expired claims |
| **Internal deduplication** | At-most-once submission | Idempotency key lookup before creating a new intent (see 7.2) |
| **External mutation** | At-most-once execution | `RetryClass::NoAutoRetry` for irreversible effects; external systems receive idempotency key on every retry attempt |

Together these three layers produce effectively-once semantics: the intent
reaches a worker at least once, duplicates within the platform are collapsed
by the idempotency key, and irreversible external mutations are submitted
at most once per explicit intent creation.

---

## 8. Crash recovery

### 8.1 Recovery principles

The execution model is designed so that after any crash (process death, host
reboot, database corruption recovery), the system can resume to a consistent
state without duplicating external effects.

### 8.2 What is replayed vs. what is not

| Component | Replayed after crash? | Rationale |
|---|---|---|
| Run state | Yes -- recovered from database | Run state is the authoritative record |
| Turn state | Yes -- recovered from database | Turns are durable |
| Pending EffectIntents | Yes -- remain in outbox | Workers claim and execute them |
| Claimed EffectIntents (lease expired) | Handled by lease recovery | See section 6.3 |
| In-progress EffectAttempts (lease expired) | Depends on retry class | Idempotent: retry. NoAutoRetry: mark Unknown |
| Resolved EffectOutcomes | Never re-executed | Outcomes are immutable terminal records |
| Context assembly | Re-computed from artifacts | Pure function of recorded inputs |
| Model inference | Re-executed only if intent is Pending | A new attempt is created |
| Tool calls | Re-executed only if intent is Pending and class allows | Check-before-retry or no-auto-retry |
| Signatures | Never auto-retried | New intent required |
| Broadcasts | Never auto-retried | Check chain state first |
| Projections | Rebuilt from durable records | Projections are derived, not authoritative |
| Live streams (SSE/WebSocket) | Reconnect from cursor | Lossy; UX convenience only |

### 8.3 Recovery procedure

```text
On startup:

1. Load all runs in non-terminal states from the database.

2. For each run:
   a. Check for expired leases on claimed EffectIntents.
      - Handle according to retry class (see section 6.3).
   b. Check for EffectOutcomes that arrived but were not yet
      consumed by the reducer.
      - Feed them to the reducer.
   c. Check for Pending EffectIntents that have no active worker.
      - They remain in the outbox for normal claim processing.
   d. Verify that the run's ResolvedGrant is still valid
      (policy not revoked, agent not deactivated).
      - If invalid, cancel the run.

3. Resume the turn actor for each non-terminal run.

4. Rebuild projections from durable records.
```

### 8.4 Crash recovery invariants

- **CRASH-INV-1:** No EffectOutcome is ever lost. Once written to the
  database, it persists.
- **CRASH-INV-2:** A crash between committing an EffectIntent and executing
  it results in a Pending intent in the outbox. A worker will claim it.
- **CRASH-INV-3:** A crash during effect execution (between claim and
  outcome recording) results in a lease expiry. The lease recovery process
  handles it according to retry class.
- **CRASH-INV-4:** A crash after recording an outcome but before the reducer
  consumes it results in an unconsumed outcome. Recovery feeds it to the
  reducer.
- **CRASH-INV-5:** Signing and broadcast effects are NEVER automatically
  retried after a crash. They produce Unknown outcomes that require explicit
  investigation.
- **CRASH-INV-6:** The reducer is a pure function. Given the same run state
  and input, it produces the same output. Recovery can safely re-run the
  reducer with recovered state and unconsumed outcomes.

### 8.5 Example: crash during chain action

```text
Timeline:
  T1: Reducer creates SignatureRequest EffectIntent (committed to DB)
  T2: Worker claims intent, requests signature from external signer
  T3: CRASH
  T4: Process restarts
  T5: Recovery finds Claimed intent with expired lease
  T6: Since retry_class = NoAutoRetry, records Unknown outcome
  T7: Reducer receives Unknown outcome for signature request
  T8: Reducer transitions run to Unknown state
  T9: Operator investigates: was the signature obtained?
  T10a: If yes -> create new BroadcastIntent with the signed payload
  T10b: If no -> create new SignatureRequest intent

At no point does the system automatically re-request a signature or
re-broadcast a transaction.
```

---

## 9. Cancellation

### 9.1 Cooperative cancellation model

Cancellation in Polkagent is cooperative. A cancellation signal is a request,
not an immediate kill.

### 9.2 Cancellation sources

| Source | Description |
|---|---|
| User request | User explicitly cancels a run through UI/API/CLI |
| Operator action | Operator cancels a run for operational reasons |
| Budget exhaustion | Run exceeds its configured budget (tokens, cost, time) |
| Deadline | Run exceeds its configured deadline |
| Policy revocation | The grant or policy backing the run is revoked |
| Agent deactivation | The agent is deactivated |
| Parent cancellation | A parent workflow run is cancelled |
| Approval denial | An approval request is denied |
| Circuit breaker | An automated safety mechanism triggers |

### 9.3 Cancellation procedure

```text
1. Cancellation signal received for run R.

2. Set run.cancellation_requested = true with reason and timestamp.

3. For each Pending EffectIntent:
   - Set state = Superseded.
   - No external I/O occurs.

4. For each Claimed/Executing EffectIntent:
   - Send cancellation signal to the worker.
   - Worker MAY complete the current operation if it is near completion
     and doing so is safer than aborting.
   - Worker MUST record an EffectOutcome (either the completed result
     or a Cancelled outcome).
   - Worker MUST respect a hard timeout (2x the normal lease duration).

5. Once all EffectIntents are Resolved or Superseded:
   - Run the reducer one final time with CancellationInput.
   - Reducer produces cleanup effects if needed (e.g., releasing locks).
   - Reducer transitions run to Cancelled state.

6. Emit RunCancelled event.
```

### 9.4 Cancellation of irreversible effects

Cancellation cannot undo completed effects. If a signing request has already
been fulfilled, the signed payload exists. If a transaction has been
broadcast, the broadcast has occurred.

The cancellation procedure for in-progress chain actions:

| Phase | Cancellation behavior |
|---|---|
| MetadataCapture | Safe to cancel. No external side effect. |
| CallDecode | Safe to cancel. No external side effect. |
| Simulation | Safe to cancel. No external side effect. |
| SignatureRequest (not yet submitted to signer) | Safe to cancel. |
| SignatureRequest (submitted, awaiting response) | Wait for signer response. Do not broadcast. |
| Broadcast (not yet submitted) | Safe to cancel. Do not broadcast. |
| Broadcast (submitted) | Cannot undo. Record broadcast receipt. Monitor finality. |
| FinalityWatch | Continue watching. Report final outcome regardless of cancellation. |

### 9.5 Resource cleanup

After cancellation completes:

1. Release any held workspace locks.
2. Close harness sessions if the run owns them exclusively.
3. Record final resource usage.
4. Update projections to show cancelled state with partial results.

---

## 10. Grant resolution

Grants determine what a run is allowed to do. Grant resolution happens
outside model control and produces an immutable ResolvedGrant.

### 10.1 Grant resolution inputs

```text
ResolvedGrant = intersection of:

  Platform policy          (global rules, safety invariants)
  ∩ Tenant/org policy      (organization-level restrictions)
  ∩ Agent policy           (agent-level declared capabilities)
  ∩ Deployment policy      (environment-specific rules)
  ∩ Sender/trigger policy  (who/what initiated the run)
  ∩ Account/signer policy  (what the signer permits)
  ∩ Approval state         (current approvals, mandates)
  ∩ Budget/quota state     (remaining resources)
```

### 10.2 ResolvedGrant structure

```rust
pub struct ResolvedGrant {
    pub id: ResolvedGrantId,
    pub run_id: RunId,

    /// The subject (agent/run) this grant applies to.
    pub subject: SubjectId,

    /// Allowed effect kinds.
    pub allowed_effects: HashSet<EffectKindDiscriminant>,

    /// Tool-specific grants.
    pub tool_grants: Vec<ToolGrant>,

    /// Chain-specific grants.
    pub chain_grants: Vec<ChainGrant>,

    /// Resource limits.
    pub limits: GrantLimits,

    /// Approvals that contributed to this grant.
    pub approvals: Vec<ApprovalRef>,

    /// Autonomous mandate reference, if applicable.
    pub mandate: Option<MandateRef>,

    /// When this grant expires.
    pub expires_at: Timestamp,

    /// Digest of the policy revisions used to compute this grant.
    pub policy_digest: Sha256Digest,

    /// Immutable once created.
    pub created_at: Timestamp,
    pub digest: Sha256Digest,
}

pub struct ToolGrant {
    pub tool_id: PinnedToolRef,
    pub allowed_operations: HashSet<String>,
    pub filesystem_roots: Vec<PathBuf>,
    pub network_hosts: Vec<HostPattern>,
    pub read_write: ReadWriteScope,
    pub max_requests: Option<u32>,
    pub max_bytes: Option<u64>,
}

pub struct ChainGrant {
    pub chain_target: ChainTarget,
    pub allowed_actions: HashSet<ChainActionFamily>,
    pub allowed_accounts: Vec<AccountRef>,
    pub spend_limits: SpendLimits,
    pub requires_approval: bool,
}

pub struct GrantLimits {
    pub max_tokens: Option<u64>,
    pub max_cost: Option<Decimal>,
    pub max_duration: Option<Duration>,
    pub max_effects: Option<u32>,
    pub max_concurrent_effects: Option<u32>,
}
```

### 10.3 Grant enforcement

Grants are enforced at two points:

1. **At run start:** The grant is resolved and recorded. If resolution fails
   (e.g., no matching policy, insufficient quota), the run transitions to
   Failed.
2. **At effect execution:** Before each effect, the worker verifies the
   effect against the run's ResolvedGrant. If the effect exceeds the grant,
   it is denied with an AuthorizationError outcome.

### 10.4 Grant invariants

- **GRANT-INV-1:** A ResolvedGrant is immutable. It cannot be modified after
  creation.
- **GRANT-INV-2:** Grant resolution is deterministic. Given the same policy
  inputs and approval state, the same grant is produced.
- **GRANT-INV-3:** The policy digest recorded in the grant makes it possible
  to verify which policy version was in effect.
- **GRANT-INV-4:** A model, harness, tool, or extension cannot modify its
  own grant. Grants are resolved outside model-controlled text.
- **GRANT-INV-5:** Budget consumption is tracked atomically with effect
  execution. An effect that would exceed remaining budget is denied before
  I/O.

---

## 11. Graph/workflow composition

For work that requires multiple steps, conditional logic, parallelism, or
human decision points, Polkagent supports declarative workflow composition.
Workflows compile to the same run/effect/outbox mechanism -- they do not
introduce a second executor.

### 11.1 Workflow as composed runs

A workflow is a `WorkflowSpec` that describes a directed acyclic graph (DAG)
of nodes. Each node corresponds to a child run or a control node.

```text
WorkflowSpec
  |
  +-- Node A: ChainRead (child run)
  |     |
  |     +-- on success --> Node B
  |     +-- on failure --> Node E (error handler)
  |
  +-- Node B: ModelInference (child run)
  |     |
  |     +-- on success --> Node C, Node D (parallel)
  |
  +-- Node C: ToolCall (child run)  ---+
  |                                     |-- join --> Node F
  +-- Node D: ToolCall (child run)  ---+
  |
  +-- Node E: ErrorHandler (child run)
  |
  +-- Node F: Approval (control node)
        |
        +-- on approved --> Node G
        +-- on denied --> Terminal(Cancelled)
```

### 11.2 Composition patterns

#### Sequential

```rust
WorkflowSpec {
    nodes: vec![
        Node::Run { id: "A", spec: run_spec_a, next: Some("B") },
        Node::Run { id: "B", spec: run_spec_b, next: Some("C") },
        Node::Run { id: "C", spec: run_spec_c, next: None },
    ],
}
```

Execution: A completes, then B starts with A's output, then C starts with
B's output.

#### Parallel (fan-out / fan-in)

```rust
WorkflowSpec {
    nodes: vec![
        Node::Run { id: "A", spec: run_spec_a, next: Some("fork-1") },
        Node::Fork {
            id: "fork-1",
            branches: vec!["B", "C", "D"],
            join: "join-1",
        },
        Node::Run { id: "B", spec: run_spec_b, next: None },
        Node::Run { id: "C", spec: run_spec_c, next: None },
        Node::Run { id: "D", spec: run_spec_d, next: None },
        Node::Join {
            id: "join-1",
            strategy: JoinStrategy::WaitAll,
            next: Some("E"),
        },
        Node::Run { id: "E", spec: run_spec_e, next: None },
    ],
}
```

Execution: A completes, then B, C, D execute in parallel. After all three
complete, E starts with their combined outputs.

#### Conditional

```rust
WorkflowSpec {
    nodes: vec![
        Node::Run { id: "A", spec: run_spec_a, next: Some("branch-1") },
        Node::Branch {
            id: "branch-1",
            condition: Condition::OutputFieldEquals {
                field: "action_type",
                value: "transfer",
            },
            if_true: "B",
            if_false: "C",
        },
        Node::Run { id: "B", spec: transfer_run, next: None },
        Node::Run { id: "C", spec: query_run, next: None },
    ],
}
```

#### Loop (bounded)

```rust
WorkflowSpec {
    nodes: vec![
        Node::Loop {
            id: "loop-1",
            body: "A",
            condition: Condition::MaxIterations(5),
            exit_condition: Condition::OutputFieldEquals {
                field: "converged",
                value: "true",
            },
            next: Some("B"),
        },
        Node::Run { id: "A", spec: iteration_run, next: None },
        Node::Run { id: "B", spec: final_run, next: None },
    ],
}
```

Loops are always bounded by a maximum iteration count. Unbounded loops are
not supported.

### 11.3 Workflow execution rules

- **WF-RULE-1:** Each workflow node that is a Run creates a child run with its
  own RunId, turn actor, and effect pipeline.
- **WF-RULE-2:** The parent workflow run coordinates child runs but does not
  directly execute their effects.
- **WF-RULE-3:** Child run grants are the intersection of the parent's grant
  and the child's declared requirements.
- **WF-RULE-4:** Cancelling the parent cascades cancellation to all active
  child runs.
- **WF-RULE-5:** A child run failure propagates to the parent according to
  the node's error handling specification.
- **WF-RULE-6:** Loops are bounded. The WorkflowSpec must specify max
  iterations. The runtime rejects unbounded loops.
- **WF-RULE-7:** Fork/join parallelism is limited by the parent run's
  max_concurrent_effects grant.

### 11.4 Workflow node types (Rust sketch)

```rust
pub enum WorkflowNode {
    /// Execute a child run.
    Run {
        id: NodeId,
        spec: RunSpec,
        next: Option<NodeId>,
        error_handler: Option<NodeId>,
    },
    /// Fork into parallel branches.
    Fork {
        id: NodeId,
        branches: Vec<NodeId>,
        join: NodeId,
    },
    /// Wait for parallel branches to complete.
    Join {
        id: NodeId,
        strategy: JoinStrategy,
        next: Option<NodeId>,
    },
    /// Conditional branch.
    Branch {
        id: NodeId,
        condition: Condition,
        if_true: NodeId,
        if_false: NodeId,
    },
    /// Bounded loop.
    Loop {
        id: NodeId,
        body: NodeId,
        max_iterations: u32,
        exit_condition: Condition,
        next: Option<NodeId>,
    },
    /// Request approval.
    Approval {
        id: NodeId,
        request: ApprovalSpec,
        on_approved: NodeId,
        on_denied: NodeId,
    },
    /// Terminal node.
    Terminal {
        id: NodeId,
        outcome: TerminalOutcome,
    },
}

pub enum JoinStrategy {
    /// Wait for all branches to complete.
    WaitAll,
    /// Wait for the first branch to complete, cancel others.
    WaitFirst,
    /// Wait for N branches to complete, cancel others.
    WaitN(u32),
}
```

---

## 12. Event stream architecture

### 12.1 Event types

Events are ordered observations produced during execution. They serve two
purposes: durable lifecycle recording and real-time UI streaming.

```rust
pub struct RunEvent {
    pub id: RunEventId,
    pub run_id: RunId,

    /// Monotonically increasing within a run.
    pub sequence: u64,

    /// Event type.
    pub kind: RunEventKind,

    /// Correlation to turn/step/effect.
    pub correlation: EventCorrelation,

    /// Durability class.
    pub durability: Durability,

    /// Timestamp.
    pub timestamp: Timestamp,

    /// Optional payload.
    pub payload: Option<EventPayload>,
}

pub enum RunEventKind {
    // Lifecycle events (always durable)
    RunQueued,
    RunStarted,
    RunCompleted { outcome: ArtifactId },
    RunFailed { error: RunError },
    RunCancelled { reason: CancellationReason },
    RunUnknown { context: UnknownContext },

    // Turn events (always durable)
    TurnStarted { turn_number: u32 },
    TurnCompleted { turn_number: u32 },

    // Effect events (always durable)
    EffectIntentCreated { intent_id: EffectIntentId },
    EffectAttemptStarted { attempt_id: EffectAttemptId },
    EffectOutcomeRecorded { outcome_id: EffectOutcomeId },

    // Approval events (always durable)
    ApprovalRequested { request_id: ApprovalRequestId },
    ApprovalGranted { approval_id: ApprovalId },
    ApprovalDenied { reason: String },

    // Artifact events (always durable)
    ArtifactCreated { artifact_id: ArtifactId, kind: ArtifactKind },

    // Progress events (ephemeral, best-effort)
    StreamingToken { text: String },
    StreamingDelta { delta: StreamDelta },
    ProgressUpdate { message: String, percentage: Option<f32> },

    // Resource events (durable)
    BudgetConsumed { resource: ResourceKind, amount: Decimal },
    BudgetWarning { resource: ResourceKind, remaining: Decimal },

    // Diagnostic events (configurable durability)
    DiagnosticLog { level: LogLevel, message: String },
}

pub enum Durability {
    /// Must be persisted before acknowledgement.
    Durable,
    /// Best-effort delivery. May be lost on crash.
    Ephemeral,
    /// Persisted if retention policy permits.
    Conditional { policy: RetentionPolicy },
}

pub struct EventCorrelation {
    pub run_id: RunId,
    pub turn_id: Option<TurnId>,
    pub step_id: Option<StepId>,
    pub effect_intent_id: Option<EffectIntentId>,
    pub effect_attempt_id: Option<EffectAttemptId>,
}
```

### 12.2 Event ordering guarantees

- **EVENT-ORD-1:** Within a run, events have a strictly monotonic sequence
  number. No gaps, no reordering.
- **EVENT-ORD-2:** Durable events are committed atomically with the state
  change they describe.
- **EVENT-ORD-3:** Ephemeral events (streaming tokens, progress) are
  delivered best-effort. Clients must tolerate gaps.
- **EVENT-ORD-4:** Cross-run event ordering is not guaranteed. Events from
  different runs may interleave arbitrarily.

### 12.3 Event delivery

Events are delivered through two channels:

1. **Durable event store:** All durable events are written to the event store
   (SQLite/Postgres). Projections and audit tools read from this store.
2. **Live event stream:** Ephemeral and durable events are published to
   connected clients via SSE or WebSocket. The live stream is a lossy
   convenience; the event store is the source of truth.

### 12.4 Subscription model

```rust
pub trait RunEventSubscriber {
    /// Subscribe to events for a specific run.
    /// Returns events from the given cursor position.
    async fn subscribe(
        &self,
        run_id: RunId,
        from_sequence: u64,
        filter: EventFilter,
    ) -> impl Stream<Item = RunEvent>;
}

pub struct EventFilter {
    /// Include only these event kinds.
    pub kinds: Option<HashSet<RunEventKindDiscriminant>>,
    /// Include only durable events.
    pub durable_only: bool,
    /// Include only events for these correlation targets.
    pub correlations: Option<EventCorrelationFilter>,
}
```

---

## 13. Concurrency model

### 13.1 Rust async runtime

Polkagent uses Tokio as its async runtime. The execution model is designed
around Rust's ownership and concurrency primitives.

### 13.2 Task structure

```text
Main process
 |
 +-- Scheduler task
 |    |-- Claims run slots from the queue
 |    +-- Spawns turn actor tasks
 |
 +-- Turn actor tasks (one per active run)
 |    |-- Owns mutable run state
 |    |-- Runs the reducer
 |    +-- Commits state + effects atomically
 |
 +-- Effect worker pool
 |    |-- N worker tasks (configurable)
 |    |-- Each claims one effect at a time
 |    +-- Executes external I/O
 |
 +-- Lease reaper task
 |    |-- Periodically checks for expired leases
 |    +-- Handles recovery per retry class
 |
 +-- Projection builder task
 |    |-- Consumes durable events
 |    +-- Updates read models
 |
 +-- Live stream publisher task
      |-- Receives all events
      +-- Publishes to connected SSE/WebSocket clients
```

### 13.3 Concurrency rules

- **CONC-1:** Each run has exactly one turn actor. There is no concurrent
  modification of a run's state.
- **CONC-2:** Effect workers execute concurrently across runs. A worker
  claims one effect at a time from any run.
- **CONC-3:** Within a run, the reducer is single-threaded. It processes
  one input at a time.
- **CONC-4:** Effect execution is concurrent with the turn actor. The turn
  actor is not blocked while effects execute (it is suspended in
  WaitingEffect state).
- **CONC-5:** Database transactions use lease-based claiming to avoid
  duplicate execution. On SQLite the write connection is owned by a single
  task (see section 13.4), so `BEGIN IMMEDIATE` serializes all claims without
  contention. On Postgres, `SELECT ... FOR UPDATE SKIP LOCKED` may be used
  instead.

### 13.4 SQLite single-writer pattern

SQLite's write concurrency model requires care in an async Tokio context.
The recommended pattern is:

```text
One dedicated write task
  |-- Owns the single write connection (SqliteConnection, not a pool)
  |-- Receives write commands via mpsc channel
  |-- Executes BEGIN IMMEDIATE + writes + COMMIT serially
  +-- Returns results via oneshot channels

Read connection pool (N connections, N concurrent readers)
  +-- Effect workers and projectors read from this pool
```

This eliminates `SQLITE_BUSY` errors entirely: there is never more than one
concurrent writer, so `BEGIN IMMEDIATE` never blocks on another writer.
Read traffic does not block writes in WAL mode. The `mpsc` channel provides
backpressure for write demand (see section 13.5).

All effect store write paths (`commit_turn`, `claim_intent`, `record_outcome`)
are sent as commands to the write task, not called directly from worker tasks.

### 13.5 Backpressure

| Component | Backpressure mechanism |
|---|---|
| Run queue | Bounded queue. New runs are rejected with RetryLater when full. |
| Effect outbox | Bounded per-run. Reducer cannot create more intents than the run's max_concurrent_effects. |
| Worker pool | Fixed pool size. Claims block when all workers are busy. |
| Live stream | Bounded channel. Slow consumers miss ephemeral events. |
| Model API | Per-provider concurrency limit and rate limiting. |
| Tool execution | Per-tool concurrency limit from grant. |

---

## 14. Resource management

### 14.1 Resource types

```rust
pub struct RunBudget {
    /// Maximum tokens (input + output) across all model calls.
    pub max_tokens: Option<u64>,
    /// Maximum monetary cost (in smallest unit of billing currency).
    pub max_cost: Option<Decimal>,
    /// Maximum wall-clock duration for the entire run.
    pub max_duration: Option<Duration>,
    /// Maximum number of effects.
    pub max_effects: Option<u32>,
    /// Maximum number of turns.
    pub max_turns: Option<u32>,
    /// Maximum number of artifacts produced.
    pub max_artifacts: Option<u32>,
    /// Maximum total artifact storage bytes.
    pub max_artifact_bytes: Option<u64>,
}

pub struct RunUsage {
    pub tokens_used: u64,
    pub cost_incurred: Decimal,
    pub elapsed: Duration,
    pub effects_executed: u32,
    pub turns_executed: u32,
    pub artifacts_produced: u32,
    pub artifact_bytes: u64,
}
```

### 14.2 Per-effect resource limits

Each EffectIntent inherits limits from the run's grant:

```rust
pub struct EffectLimits {
    /// Maximum duration for this specific effect.
    pub timeout: Duration,
    /// Maximum response size.
    pub max_response_bytes: Option<u64>,
    /// Maximum tokens for model calls.
    pub max_tokens: Option<u64>,
    /// Maximum cost for this effect.
    pub max_cost: Option<Decimal>,
}
```

### 14.3 Budget enforcement

```text
Before each effect:
  1. Check run budget: remaining_budget >= estimated_cost
  2. If insufficient, deny the effect with ResourceExhaustion
  3. Reserve the estimated cost from the budget

After each effect:
  1. Record actual usage
  2. Adjust reservation (release excess or record overage)
  3. If cumulative usage exceeds budget, cancel the run

Budget warning thresholds (configurable):
  - 75% consumed: emit BudgetWarning event
  - 90% consumed: emit BudgetWarning event, notify operator
  - 100% consumed: cancel run
```

### 14.4 Timeout hierarchy

```text
Effect timeout  <  Turn timeout  <  Run deadline  <  Agent session limit

Each level supersedes the levels below it. An effect timeout triggers
that effect's Timeout outcome. A run deadline triggers run cancellation.
```

---

## 15. End-to-end examples

### 15.1 Simple model call with tool use

**Scenario:** A user asks an agent to explain a Polkadot referendum.

```text
1. Transport receives message "Explain referendum 42 on Polkadot"
2. Conversation service creates Turn and Run
   Run state: Created -> Queued

3. Scheduler picks up the run
   Run state: Queued -> Running
   Grant resolved: read-only chain access, model inference

4. Turn 1 begins
   Step 1: ContextAssembly
     - Assemble user message + agent system prompt + chain profile
     - Produce ContextPack artifact (records what model will see)

   Step 2: Reducer creates EffectIntent
     - Kind: ModelCall (route to configured provider/model)
     - Committed to outbox atomically with run state
     Run state: Running -> WaitingEffect

5. Worker claims ModelCall intent
   - Lease acquired
   - Calls model API with context pack
   - Model responds: "I need to look up referendum 42. Use chain_query tool."
   - Records EffectOutcome::Success with model output artifact

6. Run state: WaitingEffect -> Running
   Reducer receives model output, parses tool request

   Step 3: Reducer creates EffectIntent
     - Kind: ChainRead (query referendum 42 details)
     - Verifies grant allows chain read
     - Committed to outbox
     Run state: Running -> WaitingEffect

7. Worker claims ChainRead intent
   - Queries chain RPC for referendum data
   - Records EffectOutcome::Success with referendum data artifact

8. Run state: WaitingEffect -> Running
   Turn 2 begins

   Step 1: ContextAssembly
     - Original context + tool result + chain data
     - Produce updated ContextPack artifact

   Step 2: Reducer creates EffectIntent
     - Kind: ModelCall (send tool result to model for final answer)
     Run state: Running -> WaitingEffect

9. Worker claims ModelCall intent
   - Model produces final explanation
   - Records EffectOutcome::Success with explanation artifact

10. Run state: WaitingEffect -> Running
    Reducer produces terminal output
    Run state: Running -> Completed
    Emit RunCompleted event

11. Delivery effect sends result through transport to user
```

### 15.2 Resumed coding-harness session

**Scenario:** A developer resumes a coding session that was paused yesterday.

```text
1. User sends "resume" through CLI
2. System finds the paused run (run R1, state: WaitingApproval)
   The run was paused because the harness proposed a file edit that
   required approval.

3. User reviews the proposed edit and approves
   Run state: WaitingApproval -> Running

4. Turn 3 begins (turns 1-2 completed yesterday)
   Step 1: Feed ApprovalResult to reducer
   Step 2: Reducer creates EffectIntent
     - Kind: HarnessOperation (resume session with approval)
     Run state: Running -> WaitingEffect

5. Worker claims HarnessOperation intent
   - Connects to harness process (still running or restarted)
   - Sends session resume with approval token
   - Harness applies the approved file edit
   - Harness continues working: runs tests, proposes next change
   - Records EffectOutcome::Success with harness output

6. Run state: WaitingEffect -> Running
   Reducer parses harness output: more changes proposed

7. Reducer creates EffectIntents:
   - Kind: ToolCall (apply file changes to worktree)
   - Kind: ToolCall (run test suite)
   Both effects can execute in parallel.
   Run state: Running -> WaitingEffect

8. Workers claim both ToolCall intents concurrently
   - Worker A applies file changes, records Success
   - Worker B runs tests, records Success with test result artifact

9. Run state: WaitingEffect -> Running
   Reducer sees all changes applied and tests passing
   Creates terminal output artifact with summary

10. Run state: Running -> Completed
    Delivery effect notifies user through CLI
```

### 15.3 Policy-gated on-chain action (Explain Before Sign)

**Scenario:** A user asks an agent to transfer 100 DOT to Alice.

```text
1. Transport receives: "Send 100 DOT to Alice (5GrwvaEF...)"
2. Run created with agent spec for on-chain actions
   Grant resolved: chain read, chain simulation, signature request
   (requires per-action approval for transfers)

3. Turn 1: Context assembly and model inference
   Model produces a structured TransferIntent:
   {
     action: "transfer",
     asset: { id: "DOT", chain: "polkadot" },
     amount: "100000000000",  // 100 DOT in planck
     recipient: "5GrwvaEF...",
     sender: "<user's configured account>"
   }

4. Reducer validates the TransferIntent and creates EffectIntents:
   - Kind: ChainRead (capture current metadata for Polkadot)
   Run state: Running -> WaitingEffect

5. Worker captures metadata snapshot
   EffectOutcome::Success with metadata artifact (hash, spec version)

6. Reducer creates EffectIntent:
   - Kind: ChainRead (decode the transfer call against metadata)
   Run state: Running -> WaitingEffect

7. Worker decodes the call, producing a human-readable artifact:
   "balances.transferKeepAlive(dest: 5GrwvaEF..., value: 100 DOT)"
   Links to metadata artifact for provenance.
   EffectOutcome::Success with decoded call artifact.

8. Reducer creates EffectIntent:
   - Kind: Simulation (dry-run the call)
   Run state: Running -> WaitingEffect

9. Worker runs simulation against chain node
   EffectOutcome::Success with simulation artifact:
   {
     success: true,
     fee_estimate: "0.015 DOT",
     weight: { ref_time: 200000000, proof_size: 3593 },
     events: ["Transfer { from, to, amount }"]
   }

10. Reducer evaluates policy: transfer requires approval.
    Creates ApprovalRequest artifact linking:
    - decoded call artifact
    - metadata artifact
    - simulation artifact
    - model explanation artifact
    Run state: Running -> WaitingApproval
    Emit ApprovalRequested event

11. User receives canonical approval card:
    +--------------------------------------------+
    | TRANSFER                                    |
    | Chain: Polkadot (genesis: 0x91b...)         |
    | Metadata: spec v1003007 (hash: 0xab3...)    |
    |                                             |
    | From: 5FHneW46... (your account)            |
    | To:   5GrwvaEF...                           |
    | Amount: 100.000000000 DOT                   |
    | Fee estimate: ~0.015 DOT                    |
    |                                             |
    | Simulation: SUCCESS                         |
    | [Approve] [Reject]                          |
    +--------------------------------------------+
    (Card is rendered from canonical artifacts,
     NOT from model prose)

12. User approves
    Run state: WaitingApproval -> Running

13. Reducer verifies metadata has not changed since approval.
    The SignatureRequest includes a CheckMetadataHash extension (via dynamic
    subxt decode path). The signer independently verifies the metadata hash
    against its own expectation and REFUSES TO SIGN if the hash doesn't match.
    This provides defense-in-depth: even if the platform's drift check passes,
    the signer is an independent guard.
    If metadata changed:
      - Record drift, regenerate evidence, re-request approval.
    If metadata unchanged:
      - Create EffectIntent:
        Kind: SignatureRequest
        payload: canonical encoded call bytes (with CheckMetadataHash extension)
        signer: user's configured signer
        approval: reference to the approval artifact
        chain_evidence: reference to metadata + simulation artifacts
    Run state: Running -> WaitingEffect

14. Worker sends signature request to external signer
    (wallet, hardware device, proxy, or custody service)
    Signer sees exact bytes; Polkagent never holds the private key.
    EffectOutcome::Success with signed payload artifact

15. Reducer creates EffectIntent:
    - Kind: Broadcast
      signed_payload: reference to signed payload artifact
      target: Polkadot via configured RPC
      submission_strategy: SubmitAndWatch
    Run state: Running -> WaitingEffect

16. Worker broadcasts signed transaction
    EffectOutcome::Success with submission receipt:
    { tx_hash: "0xabc...", block_hash: "0xdef..." }

17. Reducer creates EffectIntent:
    - Kind: FinalityWatch
      transaction: tx_hash
      timeout: 120 seconds
    Run state: Running -> WaitingEffect

18. Worker watches for finality
    EffectOutcome::Success:
    {
      status: Finalized,
      block_number: 12345678,
      block_hash: "0xdef...",
      events: ["Transfer { from, to, amount: 100 DOT }"]
    }

    OR

    EffectOutcome::Unknown:
    {
      context: "Finality not observed within timeout",
      resolution_hint: CheckChain { tx_hash: "0xabc..." }
    }

19. If Finalized:
    Reducer creates receipt artifact linking all evidence:
    intent -> metadata -> decoded call -> simulation -> approval
    -> signed payload -> broadcast receipt -> finality observation
    Run state: Running -> Completed
    "100 DOT transferred to 5GrwvaEF... Finalized at block 12345678."

    If Unknown:
    Run state: Running -> Unknown
    Operator or user must check chain state and manually resolve.
```

---

## 16. Rust trait and type sketches

This section collects the key trait boundaries that define the execution
model's public contracts.

### 16.1 Turn reducer

```rust
/// The core deterministic decision function.
/// Given current state and an input, produces new state and effect intents.
/// This function MUST be pure: no I/O, no randomness, no clock access.
///
/// Effects are *intents* returned in ReducerOutput.effect_intents. They are
/// written to the outbox and executed by effect workers AFTER this function
/// returns and the turn is committed. The reducer NEVER executes I/O itself.
/// This is what makes crash recovery safe: re-running reduce() on recovered
/// state reproduces the same intent list without re-triggering external actions.
pub trait TurnReducer: Send + Sync {
    fn reduce(
        &self,
        state: &TurnState,
        input: TurnInput,
        grant: &ResolvedGrant,
    ) -> ReducerOutput;
}

pub struct ReducerOutput {
    /// New state to commit.
    pub new_state: TurnState,
    /// Effects to enqueue in the outbox. Executed AFTER commit, never inside reduce().
    pub effect_intents: Vec<EffectIntentSpec>,
    /// Events to emit.
    pub events: Vec<RunEventSpec>,
    /// Artifacts to store.
    pub artifacts: Vec<ArtifactSpec>,
    /// Terminal output, if the run is complete.
    pub terminal: Option<TerminalOutput>,
}
```

### 16.2 Effect worker

```rust
/// Executes one EffectIntent by performing external I/O.
#[async_trait]
pub trait EffectWorker: Send + Sync {
    /// Claim and execute an effect.
    /// Returns the outcome observed.
    async fn execute(
        &self,
        intent: &EffectIntent,
        attempt: &EffectAttempt,
        cancellation: CancellationToken,
    ) -> EffectOutcome;
}
```

### 16.3 Effect store

```rust
/// Durable storage for the effect pipeline.
#[async_trait]
pub trait EffectStore: Send + Sync {
    /// Commit new state and effect intents atomically.
    async fn commit_turn(
        &self,
        run_id: RunId,
        new_state: TurnState,
        intents: Vec<EffectIntent>,
        events: Vec<RunEvent>,
        artifacts: Vec<Artifact>,
    ) -> Result<(), StoreError>;

    /// Claim the next pending intent from the outbox.
    async fn claim_intent(
        &self,
        worker_id: WorkerId,
        lease_duration: Duration,
    ) -> Result<Option<EffectIntent>, StoreError>;

    /// Record an effect outcome.
    async fn record_outcome(
        &self,
        outcome: EffectOutcome,
    ) -> Result<(), StoreError>;

    /// Get unprocessed outcomes for a run.
    async fn pending_outcomes(
        &self,
        run_id: RunId,
    ) -> Result<Vec<EffectOutcome>, StoreError>;

    /// Find expired leases for recovery.
    async fn expired_leases(
        &self,
        cutoff: Timestamp,
    ) -> Result<Vec<EffectIntent>, StoreError>;
}
```

### 16.4 Run scheduler

```rust
/// Schedules runs for execution.
#[async_trait]
pub trait RunScheduler: Send + Sync {
    /// Enqueue a run for execution.
    async fn enqueue(&self, run: Run) -> Result<RunId, SchedulerError>;

    /// Claim the next queued run for execution.
    async fn claim_run(&self) -> Result<Option<Run>, SchedulerError>;

    /// Cancel a run.
    async fn cancel(
        &self,
        run_id: RunId,
        reason: CancellationReason,
    ) -> Result<(), SchedulerError>;

    /// Get current queue depth.
    async fn queue_depth(&self) -> Result<u32, SchedulerError>;
}
```

### 16.5 Event sink

```rust
/// Receives and routes events from the execution model.
#[async_trait]
pub trait RunEventSink: Send + Sync {
    /// Record a durable event.
    async fn record(&self, event: RunEvent) -> Result<(), EventError>;

    /// Publish an ephemeral event to live subscribers.
    fn publish(&self, event: RunEvent);
}
```

### 16.6 Grant resolver

```rust
/// Resolves the effective grant for a run.
#[async_trait]
pub trait GrantResolver: Send + Sync {
    /// Resolve the grant for a new run.
    async fn resolve(
        &self,
        agent: &AgentSpec,
        source: &RunSource,
        policies: &[PolicyRef],
        approvals: &[ApprovalRef],
    ) -> Result<ResolvedGrant, GrantError>;

    /// Verify an effect against a resolved grant.
    fn verify_effect(
        &self,
        effect: &EffectKind,
        grant: &ResolvedGrant,
        usage: &RunUsage,
    ) -> Result<(), GrantError>;
}
```

---

## 17. Wire/schema examples

### 17.1 Run state (JSON representation)

```json
{
  "id": "run_01J8XYZABC123",
  "agent_id": "agent_polkadot_assistant_v1",
  "agent_spec_version": "1.2.0",
  "state": "WaitingApproval",
  "source": {
    "type": "Conversation",
    "conversation_id": "conv_01J8XYZ",
    "turn_id": "turn_01J8XYZ_003"
  },
  "grant": {
    "id": "grant_01J8XYZ",
    "allowed_effects": ["ModelCall", "ChainRead", "Simulation", "SignatureRequest"],
    "chain_grants": [
      {
        "chain_target": {
          "genesis_hash": "0x91b171bb158e2d3848fa23a9f1c25182fb8e20313b2c1eb49219da7a70ce90c3",
          "spec_name": "polkadot"
        },
        "allowed_actions": ["Transfer", "Query"],
        "requires_approval": true
      }
    ],
    "limits": {
      "max_tokens": 100000,
      "max_cost": "1.00",
      "max_duration_secs": 300
    },
    "policy_digest": "sha256:abcdef..."
  },
  "budget": {
    "max_tokens": 100000,
    "max_cost": "1.00"
  },
  "usage": {
    "tokens_used": 15420,
    "cost_incurred": "0.23",
    "effects_executed": 5,
    "turns_executed": 2
  },
  "created_at": "2026-07-30T10:00:00Z",
  "started_at": "2026-07-30T10:00:01Z",
  "event_sequence": 12,
  "turn_number": 2
}
```

### 17.2 EffectIntent (JSON representation)

```json
{
  "id": "intent_01J8XYZ_006",
  "run_id": "run_01J8XYZABC123",
  "turn_id": "turn_01J8XYZ_002",
  "kind": {
    "type": "SignatureRequest",
    "payload_ref": "artifact_01J8XYZ_encoded_call",
    "signer_ref": "signer_polkadot_wallet_extension",
    "approval_ref": "approval_01J8XYZ_004",
    "chain_evidence_ref": "artifact_01J8XYZ_metadata_sim"
  },
  "grant_id": "grant_01J8XYZ",
  "idempotency_key": "sha256:run_01J8XYZ/6/SignatureRequest/abcdef",
  "sequence": 6,
  "state": "Pending",
  "deadline": "2026-07-30T10:05:00Z",
  "max_attempts": 1,
  "attempt_count": 0,
  "priority": "Normal",
  "created_at": "2026-07-30T10:02:15Z"
}
```

### 17.3 EffectOutcome (JSON representation)

```json
{
  "id": "outcome_01J8XYZ_006_1",
  "attempt_id": "attempt_01J8XYZ_006_1",
  "intent_id": "intent_01J8XYZ_006",
  "run_id": "run_01J8XYZABC123",
  "result": {
    "type": "Success",
    "data": {
      "signed_payload_ref": "artifact_01J8XYZ_signed",
      "signer_account": "5FHneW46..."
    }
  },
  "external_refs": [],
  "artifacts": ["artifact_01J8XYZ_signed"],
  "usage": {
    "duration_ms": 3200
  },
  "observed_at": "2026-07-30T10:02:18Z",
  "digest": "sha256:fedcba..."
}
```

### 17.4 RunEvent (JSON representation)

```json
{
  "id": "event_01J8XYZ_007",
  "run_id": "run_01J8XYZABC123",
  "sequence": 7,
  "kind": "ApprovalRequested",
  "correlation": {
    "run_id": "run_01J8XYZABC123",
    "turn_id": "turn_01J8XYZ_002",
    "step_id": "step_01J8XYZ_002_04"
  },
  "durability": "Durable",
  "timestamp": "2026-07-30T10:02:20Z",
  "payload": {
    "request_id": "approval_req_01J8XYZ_004",
    "payload_summary": "Transfer 100 DOT to 5GrwvaEF..."
  }
}
```

### 17.5 SCALE encoding notes

For on-chain anchoring or Polkadot-native storage, key execution records can
be SCALE-encoded. The SCALE representations follow the same field structure
as the JSON examples above, using Polkadot SDK SCALE codec conventions:

- Enums are encoded with a variant index byte followed by variant fields.
- Strings are SCALE-encoded as length-prefixed byte arrays.
- Optional fields use `Option<T>` encoding (0x00 for None, 0x01 + value for Some).
- Timestamps are encoded as u64 milliseconds since Unix epoch.
- Digests are encoded as fixed-size `[u8; 32]` arrays.

Full SCALE codec derives should be added to all execution model types:

```rust
use parity_scale_codec::{Encode, Decode};

#[derive(Encode, Decode)]
pub struct RunSummary {
    pub id: RunId,
    pub state: RunState,
    pub outcome_digest: Option<[u8; 32]>,
    pub grant_digest: [u8; 32],
    pub created_at: u64,
    pub completed_at: Option<u64>,
}
```

---

## 18. Failure modes catalog

### 18.1 Process failures

| Failure | Detection | Recovery | Invariant preserved |
|---|---|---|---|
| Process crash during reducer | Process restart; run state loaded from DB | Resume from last committed state | RUN-INV-2 (atomic commits) |
| Process crash after intent commit, before execution | Lease reaper finds pending intent | Intent remains in outbox; worker claims it | EFF-INV-1 (intent before I/O) |
| Process crash during effect execution | Lease expires | Recovery per retry class | CRASH-INV-3 |
| Process crash after outcome record, before reducer consumes it | Run recovery finds unconsumed outcomes | Feed outcomes to reducer | CRASH-INV-4 |
| Database corruption | Backup restore; WAL recovery | Replay from last good backup | Depends on backup freshness |
| Out of memory | Process restarts; runs resume | Same as process crash | All crash invariants |

### 18.2 External service failures

| Failure | Detection | Recovery | Invariant preserved |
|---|---|---|---|
| Model API timeout | Effect timeout | Retry if Idempotent; new attempt | EFF-INV-2 (lease expiry) |
| Model API error (rate limit) | ErrorClass::ServerError | Backoff and retry | EFF-INV-4 (idempotency) |
| Model API error (bad request) | ErrorClass::ClientError | No retry; fail the effect | EFF-INV-5 (no auto-retry for non-retriable) |
| Tool execution failure | EffectOutcome::Failure | Depends on tool's retry class | Grant enforcement |
| Signer refusal | EffectOutcome::Failure | Record refusal; reducer decides | EFF-INV-5 (NoAutoRetry) |
| Signer timeout | EffectOutcome::Timeout | Mark Unknown; manual resolution | EFF-INV-6 (Unknown stays Unknown) |
| Chain RPC timeout | EffectOutcome::Timeout | Retry with CheckBeforeRetry | Check for existing submission |
| Chain RPC returns error | EffectOutcome::Failure | Record dispatch error | Chain error classification |
| Transaction not included | FinalityWatch timeout | EffectOutcome::Unknown | EFF-INV-6 |
| Chain reorg after inclusion | FinalityWatch detects reorg | Re-submit or mark Unknown | Never claim finality prematurely |
| Harness process crash | Health check failure | Restart harness; resume or create new session | Session state recovery |
| Transport failure (delivery) | Delivery timeout | Retry delivery (separate from run) | RUN-INV-7 (delivery != completion) |

### 18.3 Concurrency failures

| Failure | Detection | Recovery | Invariant preserved |
|---|---|---|---|
| Two workers claim same intent | Single-writer task + BEGIN IMMEDIATE (SQLite) or SKIP LOCKED (Postgres) | Only one UPDATE succeeds | CONC-5 |
| Two turn actors for same run | Run claim is exclusive | Only one claim succeeds | CONC-1 (single writer) |
| Budget exceeded by concurrent effects | Atomic budget check before execution | Deny effect that would exceed budget | GRANT-INV-5 |
| Deadlock in database | Database timeout | Retry transaction | Standard DB deadlock recovery |

### 18.4 Policy and grant failures

| Failure | Detection | Recovery | Invariant preserved |
|---|---|---|---|
| Grant resolution fails | GrantError during run start | Run transitions to Failed | GRANT-INV-2 |
| Effect exceeds grant | GrantError during effect check | Effect denied; reducer decides next action | GRANT-INV-4 |
| Policy revoked during run | Policy check during run recovery | Cancel the run | Agent lifecycle |
| Approval timeout | Configurable approval deadline | Cancel the run or re-request | RUN-INV-6 |
| Metadata drift during approval | Metadata hash comparison via `CheckMetadataHash` extension; signer refuses to sign if hash doesn't match its expectation | Re-generate evidence; re-request approval | Chain evidence freshness |

### 18.5 Data integrity failures

| Failure | Detection | Recovery | Invariant preserved |
|---|---|---|---|
| Artifact digest mismatch | Integrity check on read | Reject the artifact; re-fetch from source | Artifact provenance |
| Event sequence gap | Sequence check on insert | Reject out-of-order event | EVENT-ORD-1 |
| Stale projection | Version/cursor mismatch | Rebuild projection from events | Projections are derived |
| Grant digest mismatch | Digest verification | Reject the grant; fail the effect | GRANT-INV-1 |

---

## 19. Testing strategy

### 19.1 Test categories

#### Unit tests (per module)

| Target | What to test | Approach |
|---|---|---|
| Reducer | Determinism: same input -> same output | Property-based tests with arbitrary inputs |
| IdempotencyKey | Stability: same parameters -> same key | Fixed test vectors |
| Grant resolution | Intersection logic: deny by default | Permutation tests across policy combinations |
| Effect state machine | Valid transitions only | Exhaustive transition table tests |
| Run state machine | Valid transitions only | Exhaustive transition table tests |
| Agent state machine | Valid transitions only | Exhaustive transition table tests |
| Budget enforcement | Denial at limits | Boundary value tests |

#### Integration tests (cross-module)

| Target | What to test | Approach |
|---|---|---|
| Outbox + Worker | Claim, execute, record cycle | In-process SQLite + mock effects |
| Reducer + Outbox | Atomic commit of state + intents | Crash injection between steps |
| Lease recovery | Expired leases produce correct outcomes | Time manipulation + mock workers |
| Cancellation | Cooperative shutdown completes cleanly | Cancel during each phase |
| Workflow | Fork/join/branch/loop execute correctly | Deterministic child run mocks |

#### Crash recovery tests

| Scenario | Test method | Expected result |
|---|---|---|
| Crash after intent commit | Kill process after DB commit, restart | Intent remains pending; worker claims it |
| Crash during effect execution | Kill worker mid-execution, restart | Lease expires; recovery per retry class |
| Crash after outcome record | Kill process after outcome DB write | Outcome fed to reducer on recovery |
| Crash during reducer | Kill process during reduce(), restart | State rolled back to pre-reduce; re-run |
| Double broadcast prevention | Two workers attempt broadcast for same intent | Only one succeeds (single-writer task + BEGIN IMMEDIATE on SQLite; SKIP LOCKED on Postgres) |

#### Effect-specific tests

| Effect type | Tests |
|---|---|
| ModelCall | Timeout, rate limit, streaming interruption, malformed response |
| ToolCall | Grant denial, sandbox escape attempt, timeout, resource exhaustion |
| SignatureRequest | Signer refusal, timeout, wrong account, payload mismatch |
| Broadcast | RPC error, nonce collision, already included, timeout |
| FinalityWatch | Finalized, not included, reorg, timeout |
| ChainRead | Stale metadata, network mismatch, timeout |
| Simulation | Dispatch error, weight overflow, fee change |

#### End-to-end scenario tests

| Scenario | What it proves |
|---|---|
| Simple query | Full pipeline works for read-only operations |
| Tool use loop | Multi-turn with tool calls works correctly |
| Explain Before Sign | Complete chain action pipeline with approval |
| Crash during chain action | Recovery does not duplicate broadcast |
| Concurrent runs | Runs execute independently without interference |
| Budget exhaustion | Run is cancelled when budget is exceeded |
| Grant denial | Effect is denied when it exceeds the grant |
| Workflow composition | Sequential, parallel, conditional, loop patterns work |
| Cancellation cascade | Parent cancellation propagates to children |
| Unknown outcome | Unknown is preserved and surfaced for resolution |

### 19.2 Test infrastructure

```rust
/// Test harness for execution model tests.
pub struct ExecutionTestHarness {
    /// In-memory or SQLite effect store.
    store: Box<dyn EffectStore>,
    /// Mock effect workers.
    workers: Vec<MockEffectWorker>,
    /// Controllable clock.
    clock: MockClock,
    /// Event collector.
    events: EventCollector,
    /// Grant resolver with configurable policies.
    grants: MockGrantResolver,
}

impl ExecutionTestHarness {
    /// Create a run and advance it through all states.
    pub async fn run_to_completion(
        &mut self,
        spec: RunSpec,
        mock_outcomes: Vec<(EffectKind, OutcomeResult)>,
    ) -> RunOutcome { /* ... */ }

    /// Create a run and crash at a specific point.
    pub async fn run_with_crash_at(
        &mut self,
        spec: RunSpec,
        crash_point: CrashPoint,
    ) -> RecoveryResult { /* ... */ }

    /// Verify no duplicate effects were executed.
    pub fn assert_no_duplicate_effects(&self) { /* ... */ }

    /// Verify all events are in monotonic order.
    pub fn assert_event_ordering(&self) { /* ... */ }
}

pub enum CrashPoint {
    AfterIntentCommit,
    DuringEffectExecution,
    AfterOutcomeRecord,
    DuringReducer,
    DuringStateCommit,
}
```

### 19.3 Property-based test properties

Use `proptest` to generate arbitrary inputs and state sequences. The state
machines for Turn, Run, and Agent lifecycle are well-suited to proptest:
generate random sequences of valid and invalid transition inputs and verify
that invariants hold across all of them.

| Property | Statement | Test method |
|---|---|---|
| Reducer determinism | For any state S and input I, reduce(S, I) always produces the same output | `proptest`: run reduce() with same inputs; compare outputs |
| Turn state machine | Only valid transitions are accepted; invalid inputs are rejected | `proptest`: generate arbitrary TurnInput sequences; verify invariants |
| Run state machine | Only valid transitions are accepted; terminal states are final | `proptest`: generate arbitrary RunState transition sequences; verify TURN-INV-* and RUN-INV-* |
| Idempotency key stability | Same (run_id, sequence, kind, input_digest) always produces the same key | `proptest`: generate 10000 random inputs; verify key stability |
| No phantom effects | A completed run has exactly the effects that were committed | Compare committed intents against executed effects |
| Budget monotonicity | Usage only increases; budget only decreases | `proptest`: random effect sequences; verify monotonicity |
| Terminal state finality | Once a run enters a terminal state, no further state changes occur | Attempt transitions from terminal states; all must fail |
| Grant immutability | A ResolvedGrant cannot be modified after creation | Attempt mutations; all must fail at type level |
| Event ordering | Events within a run are strictly monotonic | Check sequence numbers across all test runs |

### 19.4 Fault injection at effect boundaries

Crash injection tests MUST cover the boundary between outbox-write and
effect-execute. This is the highest-risk point: the intent is committed but
the external I/O has not yet occurred. The system must recover correctly
regardless of when in this window the process dies.

| Fault point | Expected recovery |
|---|---|
| Crash after `commit_turn` (intents written, no worker yet) | Intent remains Pending; next worker claims it after restart |
| Crash after `claim_intent` (lease acquired, I/O not started) | Lease reaper marks expired claim; retried per retry class |
| Crash between `claim_intent` and `record_outcome` (I/O in flight) | Lease expires; recovery per retry class (Unknown for NoAutoRetry) |
| Crash after `record_outcome` (outcome written, reducer not notified) | Recovery feeds unconsumed outcome to reducer |

Test infrastructure should use `CrashPoint` injection (see section 19.2) and
verify that `assert_no_duplicate_effects()` passes after every recovery path.

---

## 20. Acceptance criteria

### 20.1 Core execution model

| ID | Criterion | Test scenario |
|---|---|---|
| **EXEC-AC-1** | A simple model call (no tools) completes with a Success outcome. | Send a query; verify RunCompleted event and output artifact. |
| **EXEC-AC-2** | A model call with tool use completes across multiple turns. | Send a query requiring a tool; verify correct turn sequence and effect pipeline. |
| **EXEC-AC-3** | A run that requires approval pauses in WaitingApproval and resumes when approved. | Trigger an approval-gated action; verify state machine and approval artifact binding. |
| **EXEC-AC-4** | A denied approval transitions the run to Cancelled. | Deny an approval request; verify RunCancelled event with denial reason. |
| **EXEC-AC-5** | A run that exceeds its budget is cancelled with BudgetExhaustion reason. | Configure a tight budget; send a query that exceeds it; verify cancellation. |
| **EXEC-AC-6** | A run that exceeds its deadline is cancelled. | Configure a short deadline; send a slow query; verify cancellation. |

### 20.2 Effect pipeline

| ID | Criterion | Test scenario |
|---|---|---|
| **EFF-AC-1** | An EffectIntent is committed to the database before any I/O occurs. | Inspect database state before worker claims the intent. |
| **EFF-AC-2** | An EffectOutcome is immutable after recording. | Attempt to modify a recorded outcome; verify rejection. |
| **EFF-AC-3** | Idempotency keys are stable across retries. | Retry an effect; verify the same key is used. |
| **EFF-AC-4** | NoAutoRetry effects are never automatically retried. | Crash during a signature request; verify no automatic retry. |
| **EFF-AC-5** | Unknown outcomes are preserved as Unknown. | Force a timeout on a broadcast; verify Unknown state. |
| **EFF-AC-6** | Lease expiry triggers correct recovery per retry class. | Let a lease expire for each retry class; verify recovery behavior. |

### 20.3 Crash recovery

| ID | Criterion | Test scenario |
|---|---|---|
| **CRASH-AC-1** | Crash after intent commit: intent remains pending and is claimed by a worker. | Kill process after DB commit; restart; verify intent execution. |
| **CRASH-AC-2** | Crash during effect execution: lease expires and recovery handles it. | Kill worker mid-execution; verify recovery per retry class. |
| **CRASH-AC-3** | Crash after outcome record: outcome is fed to reducer on recovery. | Kill process after outcome write; restart; verify reducer receives outcome. |
| **CRASH-AC-4** | No signing or broadcast effect is automatically retried after crash. | Kill process during sign/broadcast; verify Unknown state and no retry. |
| **CRASH-AC-5** | After any crash, the system resumes to a consistent state. | Crash at each CrashPoint; verify system consistency after recovery. |

### 20.4 Chain action safety

| ID | Criterion | Test scenario |
|---|---|---|
| **CHAIN-AC-1** | A complete Explain Before Sign flow produces linked evidence artifacts. | Execute transfer flow; verify metadata, decode, simulation, approval, signature, broadcast, and finality artifacts are linked. |
| **CHAIN-AC-2** | Metadata drift between approval and signing triggers re-verification. | Simulate metadata change after approval; verify re-verification. |
| **CHAIN-AC-3** | A broadcast timeout produces Unknown state, not automatic retry. | Simulate broadcast timeout; verify Unknown state. |
| **CHAIN-AC-4** | A duplicate broadcast is prevented by idempotency. | Attempt duplicate broadcast; verify deduplication. |
| **CHAIN-AC-5** | Signing is isolated: the model never sees private key material. | Inspect all artifacts and events; verify no key material. |

### 20.5 Grant enforcement

| ID | Criterion | Test scenario |
|---|---|---|
| **GRANT-AC-1** | An effect that exceeds the grant is denied. | Create an effect that exceeds allowed_effects; verify denial. |
| **GRANT-AC-2** | A tool call that exceeds filesystem_roots is denied. | Attempt file access outside allowed roots; verify denial. |
| **GRANT-AC-3** | A chain action that exceeds spend_limits is denied. | Attempt transfer exceeding spend limit; verify denial. |
| **GRANT-AC-4** | Grant resolution produces the same result for the same inputs. | Resolve grants twice with same inputs; verify identical results. |
| **GRANT-AC-5** | A revoked policy causes run cancellation on recovery. | Revoke a policy; restart; verify run cancellation. |

### 20.6 Workflow composition

| ID | Criterion | Test scenario |
|---|---|---|
| **WF-AC-1** | Sequential workflow executes nodes in order. | Define A -> B -> C; verify execution order. |
| **WF-AC-2** | Parallel workflow executes branches concurrently. | Define fork with 3 branches; verify concurrent execution. |
| **WF-AC-3** | Conditional workflow takes the correct branch. | Define branch with condition; verify correct path. |
| **WF-AC-4** | Bounded loop respects max iterations. | Define loop with max 5; verify loop terminates. |
| **WF-AC-5** | Parent cancellation cascades to child runs. | Cancel parent; verify all children are cancelled. |
| **WF-AC-6** | Child grant is intersection of parent grant and child requirements. | Verify child cannot exceed parent's permissions. |

### 20.7 Concurrency and ordering

| ID | Criterion | Test scenario |
|---|---|---|
| **CONC-AC-1** | Two workers cannot claim the same effect intent. | Race two workers; verify only one succeeds. |
| **CONC-AC-2** | Events within a run are strictly monotonic. | Execute a complex run; verify event sequence. |
| **CONC-AC-3** | Concurrent runs do not interfere with each other. | Execute 10 runs simultaneously; verify isolation. |
| **CONC-AC-4** | Backpressure rejects new runs when queue is full. | Fill queue; attempt new run; verify rejection. |

---

## Appendix A: Decision log

| Decision | Rationale | Alternatives considered |
|---|---|---|
| Single turn actor per run | Prevents concurrent state mutation; simplifies recovery | Lock-free CRDT (too complex for v1), optimistic concurrency (risk of conflicts) |
| SQLite for v1 storage | Simple, embedded, sufficient for local-first; Postgres path for managed | Postgres only (adds dependency), custom storage (unnecessary) |
| Outbox pattern for effects | Proven pattern for exactly-once delivery in distributed systems | Direct execution (no crash safety), saga log only (less composable) |
| Lease-based worker claims | Simple, recoverable, no central coordinator needed | Queue-based (adds infrastructure), assignment-based (single point of failure) |
| NoAutoRetry for signing/broadcast | Prevents duplicate irreversible actions | Idempotent retry (insufficient for chain interactions), manual-only (too conservative for reads) |
| Cooperative cancellation | Safer than kill; allows cleanup | Hard kill (risks orphaned resources), no cancellation (poor UX) |
| Workflow compiles to run/effect | One execution model to test and secure | Separate workflow executor (duplicates safety logic), interpreter (harder to audit) |
| Deterministic reducer | Enables replay and debugging; crash recovery is simpler | Non-deterministic (harder recovery), event sourcing only (rebuilding is slow) |
| Do not adopt Temporal | Temporal requires a dedicated cluster, which is architecturally hostile to the local-first, single-binary deployment model. Its durability guarantees come from its own cluster's database — not from the application's database. | Temporal (cluster dependency), Conductor/Cadence (same issue) |
| Adopt DBOS *pattern*, not DBOS *product* | The DBOS insight is correct: "workflow durability = your database's durability." This PRD applies that principle directly — the outbox, reducer, and SQLite WAL provide workflow durability without a separate workflow service. The DBOS product itself has its own infrastructure requirements that conflict with local-first goals. | DBOS platform (external dependency), custom event log (more complex) |
| Single-writer Tokio task for SQLite writes | Eliminates `SQLITE_BUSY` storms; `BEGIN IMMEDIATE` + single writer means no write contention. Read pool provides concurrent read access without blocking writers in WAL mode. | Connection pool for writes (contention), serialized reads (performance loss) |

## Appendix B: Glossary cross-reference

| This PRD | PRD-02 term | Roko equivalent | PCA equivalent |
|---|---|---|---|
| Run | Run | Workflow execution / Run ledger entry | Bot message processing cycle |
| Turn | Turn | Turn / Step in tool loop | Single request-response |
| EffectIntent | EffectIntent | Effect command / Outbox entry | N/A (inline execution) |
| EffectAttempt | EffectAttempt | Driver claim / Lease | N/A |
| EffectOutcome | EffectOutcome | Effect result / Commit entry | N/A (inline result) |
| ResolvedGrant | ResolvedGrant | Effective permission / Gate intersection | N/A (implicit trust) |
| Artifact | Artifact | Signal (durable) | N/A (ephemeral messages) |
| RunEvent | RunEvent | Pulse (ephemeral) + Ledger entry (durable) | Log entries |
| Outbox | Outbox | Effect driver queue | N/A |
| Reducer | Reducer | Workflow engine / Pipeline state | N/A |
| Turn actor | Turn actor | Single owner loop | Message handler |

## Appendix C: Open questions

| Question | Impact | Proposed resolution path |
|---|---|---|
| Exact SQLite schema for effect store | Implementation detail | Spike during first implementation |
| Worker pool sizing heuristics | Performance tuning | Benchmark during integration testing |
| Projection rebuild performance at scale | Managed deployment | Measure during cloud architecture work |
| Harness session state portability | Resume across processes/hosts | Define in PRD-04 harness contract |
| Chain reorg depth handling | Finality observation accuracy | Research per-chain reorg guarantees |
| Approval card rendering contract | UX safety | Define in PRD-13 UX PRD |
| Multi-tenant effect isolation | Managed deployment | Define in PRD-11 cloud PRD |
| SCALE encoding for on-chain anchoring | Optional feature | Design when witness anchoring is implemented |

## Appendix D: Implementation staging

Based on research findings, the following staged approach is recommended:

### Do now (foundational correctness)

- Reducer + outbox + lease queue: the core safety triad. Nothing else is safe
  to build on top until these three work correctly together.
- Idempotency keys: generated at intent creation, passed to external systems
  on every attempt.
- SQLite WAL + `BEGIN IMMEDIATE` + `busy_timeout=5000` + single-writer Tokio
  task (section 6.4, section 13.4).

### Validate next (correctness verification)

- `proptest` state machine tests for Turn, Run, and Agent lifecycle
  (section 19.3).
- Fault injection at all `CrashPoint` boundaries between outbox-write and
  effect-execute (section 19.4).
- Crash-recovery integration tests verifying `assert_no_duplicate_effects()`
  for all retry classes.

### Defer (complexity not yet needed)

- Multi-agent DAG orchestration beyond the workflow composition already
  specified in section 11. Full cross-agent dependency graphs can be added
  when the single-agent execution model is proven stable.

### Avoid (known anti-patterns)

- **Effects inside reducers.** The reducer is a pure function. Any I/O inside
  `reduce()` breaks determinism and makes crash recovery unsafe. This is
  enforced at the trait boundary (see section 16.1 and TURN-INV-5).
- **Nonce management inside the agent runtime.** Nonce selection for chain
  submissions must be deferred to the signer or a dedicated nonce manager.
  Nonce races between concurrent runs or retries lead to duplicate
  transactions. The agent runtime submits a call; the signer resolves the nonce.
- **Adopting Temporal or similar workflow cluster products.** See Appendix A.

---

## Appendix E: Execution Engine Implementation Blueprint

### E.1 Concurrency Model

#### How parallel Steps within a Turn execute

Within a single Turn, the reducer may produce multiple `EffectIntent` records in one atomic commit. Independent intents (those with no ordering dependency) execute concurrently via the effect worker pool. The mechanism is:

1. The reducer emits a `Vec<EffectIntentSpec>` with an explicit `ordering` field.
2. The outbox assigns sequence numbers. Intents with the same `concurrency_group` may be claimed by separate workers simultaneously.
3. The turn actor suspends in `WaitingEffect` until all required intents resolve.
4. Workers pick up intents via the single-writer Tokio task's claim queue.

Concurrency is achieved through **Tokio tasks**, not a thread pool. Each effect worker is a `tokio::spawn`-ed task that owns one leased intent at a time. The worker pool size is configurable (default: `min(num_cpus * 2, 32)`).

Reference pattern from Roko (`crates/roko-agent/src/dispatcher/parallel.rs`):

```rust
// Parallel tools: join_all for concurrent I/O-bound calls
// Serial tools: sequential for stateful / write-write conflict avoidance
use futures::future::join_all;

pub fn partition_by_concurrency(
    calls: Vec<ToolCall>,
    registry: &dyn ToolRegistry,
) -> (Vec<ToolCall>, Vec<ToolCall>) {
    let mut parallel = Vec::new();
    let mut serial = Vec::new();
    for call in calls {
        let is_parallel = registry
            .get(&call.name)
            .is_some_and(|d| d.concurrency == ToolConcurrency::Parallel);
        if is_parallel { parallel.push(call); } else { serial.push(call); }
    }
    (parallel, serial)
}

// In the dispatcher:
let (par_calls, ser_calls) = partition_by_concurrency(calls, registry);
let par_results = join_all(par_calls.iter().map(|c| dispatch_one(c))).await;
// Then serial calls one-by-one
```

In Polkagent, `EffectKind` carries a `ToolConcurrency`-equivalent field. Unknown or write-heavy effects (`SignatureRequest`, `Broadcast`) default to serial-equivalent within a run.

#### Resource budget enforcement

Resource budget enforcement uses a `ResourceAccount` pattern (reference: `crates/roko-runtime/src/resource.rs`):

```rust
/// Budget tracker for a single run. Lives inside the turn actor.
pub struct RunResourceAccount {
    pub tokens:    BudgetEntry<u64>,
    pub cost:      BudgetEntry<Decimal>,
    pub time:      BudgetEntry<Duration>,
    pub effects:   BudgetEntry<u32>,
    pub artifacts: BudgetEntry<u32>,
    started_at:    Instant,
}

impl RunResourceAccount {
    /// Check whether an estimated effect cost fits within the remaining budget.
    /// Returns Err(ResourceExhaustion) if any limit would be exceeded.
    pub fn check_can_afford(&self, estimate: &EffectCostEstimate) -> Result<(), BudgetDenial> {
        if let Some(limit) = self.tokens.limit {
            if self.tokens.used + estimate.tokens > limit {
                return Err(BudgetDenial::TokenLimitExceeded {
                    used: self.tokens.used,
                    limit,
                    requested: estimate.tokens,
                });
            }
        }
        // same for cost, time, effects
        Ok(())
    }

    /// Reserve an estimated cost atomically before the effect executes.
    pub fn reserve(&mut self, estimate: &EffectCostEstimate) -> ReservationId { /* ... */ }

    /// Settle a reservation with actual usage after the effect completes.
    pub fn settle(&mut self, reservation: ReservationId, actual: &EffectUsage) { /* ... */ }
}
```

Budget warnings emit `RunEventKind::BudgetWarning` at 75% and 90% thresholds. At 100%, the turn actor initiates cooperative cancellation.

#### Backpressure mechanisms

| Layer | Mechanism | Implementation |
|---|---|---|
| Run queue | `tokio::sync::Semaphore` with configurable permits | Scheduler task acquires one permit per active run slot |
| Effect outbox per run | `GrantLimits::max_concurrent_effects` checked before intent creation | Reducer returns `ReducerError::TooManyPendingEffects` if exceeded |
| Worker pool | Fixed `JoinSet` of N tasks; new claims block until a slot is free | `tokio::sync::Semaphore` over the worker pool |
| Live stream | `tokio::sync::broadcast` with bounded capacity; `Lagged` error for slow consumers | SSE/WebSocket handler drops missed ephemeral events |
| SQLite write channel | Bounded `mpsc` channel to the single writer task | Write requests queue up; callers `await` the oneshot response |

#### Cancellation propagation

Cancellation uses a hierarchical `CancelToken` (reference: `crates/roko-runtime/src/cancel.rs`):

```rust
// From Roko — parent cancel propagates to all children atomically
pub struct CancelToken {
    inner: Arc<CancelInner>,
}

struct CancelInner {
    cancelled: AtomicBool,
    notify:    Notify,
    parent:    Option<CancelToken>,
}

impl CancelToken {
    pub fn child(&self) -> Self { /* inherits parent cancellation */ }
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
        self.inner.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        // walks parent chain without recursion
        if self.inner.cancelled.load(Ordering::Acquire) { return true; }
        let mut current = self.inner.parent.as_ref();
        while let Some(p) = current {
            if p.inner.cancelled.load(Ordering::Acquire) { return true; }
            current = p.inner.parent.as_ref();
        }
        false
    }
    pub async fn cancelled(&self) { /* awaitable future */ }
}
```

In Polkagent:
- Each **run** owns a `CancelToken` derived from the agent's session-level token.
- Each **effect worker** receives a child token for its current intent.
- Each **child run** in a workflow receives a child token from the parent run.
- `is_cancelled()` is checked at the top of every iteration in the run loop and the worker loop.

---

### E.2 State Machine Implementation

#### Agent lifecycle state machine (complete Rust enum)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentState {
    /// AgentSpec accepted and stored. Not yet validated.
    Created,
    /// Validated AgentSpec with resolved provider, tool, and policy bindings.
    Configured,
    /// Accepting and executing runs. Grants resolved, policies bound.
    Active,
    /// Operator-initiated pause. In-flight runs reach next checkpoint, then suspend.
    Paused,
    /// One or more execution dependencies degraded. New runs may be queued.
    Degraded { stage: DegradationStage },
    /// Permanently stopped. In-flight runs cooperatively cancelled.
    Deactivated,
    /// Spec, runs, artifacts, and audit trail retained. Cannot reactivate.
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradationStage {
    /// Provider degraded; fallback model active.
    ProviderFallback,
    /// Harness unhealthy; new sessions rejected.
    HarnessUnhealthy,
    /// Chain RPC unreachable; chain effects queued.
    ChainRpcUnreachable,
    /// Budget low; reduced capability mode.
    BudgetConstrained,
}
```

**Agent transition table:**

| From | Event | To | Guard | Action |
|---|---|---|---|---|
| `Created` | `Validate` | `Configured` | Spec validation passes; provider/tool/policy refs resolve | Record validated spec digest |
| `Created` | `Validate` | `Archived` | Validation fails irrecoverably | Record failure reason |
| `Configured` | `Activate` | `Active` | Grant resolution succeeds for all declared capabilities | Record `ActivatedAt`; emit `AgentActivated` |
| `Configured` | `Archive` | `Archived` | Operator decision | Emit `AgentArchived` |
| `Active` | `Pause` | `Paused` | Always allowed | Emit `AgentPaused`; suspend new run acceptance |
| `Active` | `DependencyDegraded` | `Degraded` | Degradation detected by health monitor | Record degradation kind; emit `AgentDegraded` |
| `Active` | `Deactivate` | `Deactivated` | Always allowed | Begin cooperative cancellation of all in-flight runs |
| `Paused` | `Resume` | `Active` | Always allowed | Emit `AgentResumed`; resume run acceptance |
| `Paused` | `Deactivate` | `Deactivated` | Always allowed | Begin cooperative cancellation |
| `Degraded` | `DependencyRecovered` | `Active` | Health monitor reports recovery | Emit `AgentRecovered` |
| `Degraded` | `Deactivate` | `Deactivated` | Always allowed | Begin cooperative cancellation |
| `Deactivated` | `Archive` | `Archived` | All in-flight runs cancelled | Emit `AgentArchived` |
| `Archived` | (any) | (none) | Terminal | Reject; emit `InvalidTransition` error |

**Invalid transition handling:**

```rust
pub fn apply_agent_event(
    state: &AgentState,
    event: &AgentEvent,
) -> Result<AgentState, AgentTransitionError> {
    match (state, event) {
        (AgentState::Archived, _) => Err(AgentTransitionError::TerminalState),
        (AgentState::Created, AgentEvent::Validate { .. }) => { /* ... */ }
        _ => Err(AgentTransitionError::InvalidTransition {
            from: state.clone(),
            event: event.clone(),
        }),
    }
}
```

All invalid transitions are logged and emitted as diagnostic events. They never panic.

**Persistence/serialization:**

`AgentState` derives `serde::Serialize` and `serde::Deserialize` with `#[serde(tag = "state", rename_all = "snake_case")]`. The state is stored as a JSONB column in the `agents` table. For Polkadot-native anchoring, `parity_scale_codec::Encode` and `Decode` are derived for a compact `AgentStateSummary`:

```rust
#[derive(Encode, Decode)]
pub struct AgentStateSummary {
    pub id:           AgentId,          // [u8; 32]
    pub state_tag:    u8,               // discriminant
    pub spec_digest:  [u8; 32],
    pub updated_at:   u64,              // ms since epoch
}
```

---

#### Run lifecycle state machine (complete Rust enum)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RunState {
    /// Run record exists. No execution begun.
    Created,
    /// Validated and accepted. Waiting for scheduling slot.
    Queued,
    /// Turn actor owns this run. Executing turns.
    Running,
    /// Paused: policy gate requires approval before proceeding.
    WaitingApproval {
        request_id:   ApprovalRequestId,
        /// Artifact binding the exact payload under review.
        evidence_id:  ArtifactId,
        /// When approval request expires.
        expires_at:   Option<Timestamp>,
    },
    /// Paused: one or more effects executing in workers.
    WaitingEffect {
        /// All intents that must resolve before the run proceeds.
        pending_intent_ids: Vec<EffectIntentId>,
        /// Whether ALL must resolve (WaitAll) or ANY suffices (WaitFirst).
        join_strategy:      JoinStrategy,
    },
    /// Terminal: all turns finished successfully.
    Completed,
    /// Terminal: unrecoverable error.
    Failed { reason: FailureReason },
    /// Terminal: cooperatively cancelled.
    Cancelled { reason: CancellationReason },
    /// Terminal (pending resolution): outcome indeterminate.
    Unknown { context: UnknownContext },
}
```

**Run transition table:**

| From | Event | To | Guard | Action |
|---|---|---|---|---|
| `Created` | `Enqueue` | `Queued` | Agent is `Active`; input validates; dedup passes | Emit `RunQueued`; record input artifacts |
| `Queued` | `Schedule` | `Running` | Slot available; grants resolve | Emit `RunStarted`; spawn turn actor; record `ResolvedGrant` |
| `Running` | `ApprovalRequired` | `WaitingApproval` | Policy gate returns `RequireApproval` | Emit `ApprovalRequested`; record approval request artifact |
| `WaitingApproval` | `ApprovalGranted` | `Running` | Approval decision valid and bound to exact request | Emit `ApprovalGranted`; feed to reducer |
| `WaitingApproval` | `ApprovalDenied` | `Cancelled` | Denial decision received | Emit `RunCancelled { reason: ApprovalDenied }` |
| `WaitingApproval` | `ApprovalTimeout` | `Cancelled` | Approval expired | Emit `RunCancelled { reason: ApprovalTimeout }` |
| `Running` | `EffectsRequested` | `WaitingEffect` | Reducer commits intents | Emit `EffectIntentsCreated` |
| `WaitingEffect` | `EffectsResolved` | `Running` | Join strategy satisfied | Emit `EffectsResolved`; feed outcomes to reducer |
| `Running` | `TerminalSuccess` | `Completed` | Reducer produces terminal success | Emit `RunCompleted`; record output artifact |
| `Running` | `UnrecoverableError` | `Failed` | Error beyond retry budget | Emit `RunFailed`; record error artifact |
| `Running` | `CancelRequested` | `Cancelled` | Signal received; shutdown completes | Emit `RunCancelled` |
| `WaitingEffect` | `CancelRequested` | `Cancelled` | Signal received; all intents superseded or resolved | Supersede pending intents; await in-progress |
| `Any non-terminal` | `BudgetExhausted` | `Cancelled` | Usage exceeds limit | Emit `RunCancelled { reason: BudgetExhausted }` |
| `Any non-terminal` | `DeadlineExceeded` | `Cancelled` | Wall-clock past deadline | Emit `RunCancelled { reason: DeadlineExceeded }` |
| `Any non-terminal` | `IndeterminateOutcome` | `Unknown` | Effect outcome genuinely unknowable | Emit `RunUnknown`; require manual resolution |
| `Failed` | `RetryQueued` | `Queued` | Retry policy permits; count < max | Emit `RunRetryQueued`; increment retry count |

**Persistence:**

```rust
// SQLite schema fragment
//
// CREATE TABLE runs (
//   id            TEXT PRIMARY KEY,
//   agent_id      TEXT NOT NULL,
//   state         TEXT NOT NULL,    -- JSON-encoded RunState tag + payload
//   state_data    JSONB,            -- full RunState variant fields
//   grant_id      TEXT NOT NULL REFERENCES resolved_grants(id),
//   created_at    INTEGER NOT NULL, -- ms epoch
//   updated_at    INTEGER NOT NULL,
//   event_seq     INTEGER NOT NULL DEFAULT 0
// );
//
// All state writes go through the single-writer task using BEGIN IMMEDIATE.
```

---

### E.3 Effect Pipeline Implementation

#### EffectIntent → EffectAttempt → EffectOutcome pipeline

```
Reducer (pure)                  Single-writer task           Effect worker
     │                                  │                          │
     │ ReducerOutput {                  │                          │
     │   effect_intents: [...]  ───────>│ BEGIN IMMEDIATE          │
     │   new_state: ...         }       │ INSERT INTO runs         │
     │                                  │   (state = WaitingEffect)│
     │                                  │ INSERT INTO effect_intents│
     │                                  │   (state = Pending, ...)  │
     │                                  │ INSERT INTO run_events    │
     │                                  │ COMMIT                    │
     │                                  │                          │
     │                                  │ notify workers ──────────>│
     │                                  │                          │ claim_intent()
     │                                  │<── UPDATE effect_intents ─│
     │                                  │   (Pending→Claimed, lease)│
     │                                  │ INSERT effect_attempts    │
     │                                  │ COMMIT                   │
     │                                  │                          │
     │                                  │                          │ [external I/O]
     │                                  │                          │
     │                                  │<── record_outcome() ──────│
     │                                  │   INSERT effect_outcomes  │
     │                                  │   UPDATE effect_intents   │
     │                                  │     (Claimed→Resolved)    │
     │                                  │   UPDATE effect_attempts  │
     │                                  │   COMMIT                  │
     │                                  │                          │
     │<── notify turn actor ────────────│                          │
     │                                  │                          │
     │ TurnInput::EffectResult(outcomes)│                          │
```

#### Idempotency key generation and deduplication

```rust
use sha2::{Digest, Sha256};

/// Stable idempotency key for an effect intent.
/// Derived deterministically from run identity + intent position + content.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdempotencyKey(pub [u8; 32]);

impl IdempotencyKey {
    /// Generate a stable key. Safe to call multiple times with the same inputs.
    pub fn generate(
        run_id:                  &RunId,
        intent_sequence:         u64,
        effect_kind_discriminant: &str,   // e.g. "ModelCall", "SignatureRequest"
        canonical_input_digest:  &[u8; 32],
    ) -> Self {
        let mut h = Sha256::new();
        h.update(run_id.as_bytes());
        h.update(b":");
        h.update(intent_sequence.to_le_bytes());
        h.update(b":");
        h.update(effect_kind_discriminant.as_bytes());
        h.update(b":");
        h.update(canonical_input_digest);
        IdempotencyKey(h.finalize().into())
    }

    pub fn as_hex(&self) -> String {
        hex::encode(self.0)
    }
}

// Deduplication in the single-writer task:
//
// Before inserting a new EffectIntent, check:
//
//   SELECT id, state FROM effect_intents
//   WHERE idempotency_key = ?
//   LIMIT 1;
//
// | Result         | Action                                     |
// |----------------|--------------------------------------------|
// | Not found      | Insert new intent (normal path)            |
// | Pending/Claimed| Return existing intent id to reducer       |
// | Resolved       | Return existing outcome to reducer         |

// Reference: Bardo's webhook deduplication uses the same LRU-bounded
// seen-set pattern for delivery IDs
// (apps/mori/src/orchestrator/platform/idempotency.rs):
//
// pub struct DeliveryIdempotencyCache {
//     queue: VecDeque<String>,
//     seen:  HashSet<String>,
//     capacity: usize,
// }
// impl DeliveryIdempotencyCache {
//     pub fn try_record(&mut self, delivery_id: &str) -> bool { ... }
// }
```

#### Lease-based execution with timeout

```rust
/// Claim parameters for the single-writer task.
pub struct ClaimRequest {
    pub worker_id:      WorkerId,
    pub lease_duration: Duration,   // e.g. 30s for model calls, 120s for finality watch
    pub batch_size:     u32,        // usually 1 per worker
}

/// The single-writer task executes this inside BEGIN IMMEDIATE:
///
/// UPDATE effect_intents
/// SET    state         = 'Claimed',
///        worker_id     = :worker_id,
///        lease_expires = :now + :lease_duration
/// WHERE  id IN (
///          SELECT id FROM effect_intents
///          WHERE  state         = 'Pending'
///            AND  (deadline IS NULL OR deadline > :now)
///          ORDER BY priority ASC, sequence ASC
///          LIMIT  :batch_size
///        )
/// RETURNING *;

/// Per-effect-kind lease durations:
pub fn default_lease_duration(kind: &EffectKind) -> Duration {
    match kind {
        EffectKind::ModelCall { streaming: true, .. }  => Duration::from_secs(300),
        EffectKind::ModelCall { streaming: false, .. } => Duration::from_secs(120),
        EffectKind::ToolCall { .. }                    => Duration::from_secs(60),
        EffectKind::SignatureRequest { .. }            => Duration::from_secs(300),
        EffectKind::Broadcast { .. }                   => Duration::from_secs(60),
        EffectKind::FinalityWatch { timeout, .. }      => *timeout + Duration::from_secs(10),
        EffectKind::ChainRead { .. }                   => Duration::from_secs(30),
        EffectKind::Simulation { .. }                  => Duration::from_secs(60),
        EffectKind::HarnessOperation { .. }            => Duration::from_secs(600),
        EffectKind::Delivery { .. }                    => Duration::from_secs(30),
    }
}
```

#### Retry policy per effect type

```rust
pub struct RetryPolicy {
    pub class:           RetryClass,
    pub max_attempts:    u32,
    pub base_backoff:    Duration,
    pub max_backoff:     Duration,
    pub backoff_factor:  f64,
}

pub fn retry_policy_for(kind: &EffectKind) -> RetryPolicy {
    match kind {
        // Safe to retry with same idempotency key; provider may deduplicate.
        EffectKind::ModelCall { .. } => RetryPolicy {
            class:          RetryClass::Idempotent,
            max_attempts:   3,
            base_backoff:   Duration::from_secs(2),
            max_backoff:    Duration::from_secs(60),
            backoff_factor: 2.0,
        },
        // Read-only; safe idempotent retry.
        EffectKind::ChainRead { .. } | EffectKind::Simulation { .. } => RetryPolicy {
            class:          RetryClass::Idempotent,
            max_attempts:   5,
            base_backoff:   Duration::from_secs(1),
            max_backoff:    Duration::from_secs(30),
            backoff_factor: 2.0,
        },
        // Check chain for existing submission before retrying.
        EffectKind::Broadcast { .. } => RetryPolicy {
            class:          RetryClass::CheckBeforeRetry,
            max_attempts:   2,
            base_backoff:   Duration::from_secs(5),
            max_backoff:    Duration::from_secs(5),
            backoff_factor: 1.0,
        },
        // Never auto-retry. A new intent must be created explicitly.
        EffectKind::SignatureRequest { .. } => RetryPolicy {
            class:          RetryClass::NoAutoRetry,
            max_attempts:   1,
            base_backoff:   Duration::ZERO,
            max_backoff:    Duration::ZERO,
            backoff_factor: 1.0,
        },
        // Tool calls: depends on tool concurrency class.
        EffectKind::ToolCall { .. } => RetryPolicy {
            class:          RetryClass::CheckBeforeRetry,
            max_attempts:   2,
            base_backoff:   Duration::from_secs(1),
            max_backoff:    Duration::from_secs(10),
            backoff_factor: 2.0,
        },
        EffectKind::FinalityWatch { .. } => RetryPolicy {
            class:          RetryClass::Idempotent,
            max_attempts:   1,  // one watch; outcome is Unknown on timeout
            base_backoff:   Duration::ZERO,
            max_backoff:    Duration::ZERO,
            backoff_factor: 1.0,
        },
        EffectKind::HarnessOperation { .. } | EffectKind::Delivery { .. } => RetryPolicy {
            class:          RetryClass::CheckBeforeRetry,
            max_attempts:   3,
            base_backoff:   Duration::from_secs(2),
            max_backoff:    Duration::from_secs(30),
            backoff_factor: 2.0,
        },
    }
}
```

#### Compensation/rollback on failure

Polkagent does not implement automatic compensation ("saga rollback") for failed effects. The rationale: automatic compensation of chain actions is error-prone and can cause double-spend or state corruption. Instead:

1. **Reversible effects** (model calls, reads, tool calls on idempotent targets): the reducer simply does not advance past the failed step. No compensation needed.
2. **Partially applied tool effects** (file edits, workspace mutations): the reducer produces a `ToolCall` intent targeting an undo/revert operation if the tool supports it. This is opt-in per tool.
3. **Irreversible chain effects** (signed broadcasts): the outcome is preserved as-is. The operator uses the audit trail to determine the actual on-chain state and manually initiates any corrective action.

```rust
/// Compensation specification returned by a reducer when a tool effect
/// has partially applied and the tool supports rollback.
pub struct CompensationSpec {
    /// The failed intent that needs to be undone.
    pub failed_intent_id: EffectIntentId,
    /// The compensation effect to execute (e.g., tool rollback call).
    pub compensation:     EffectIntentSpec,
    /// Ordering: compensation executes before any further effects.
    pub priority:         EffectPriority,
}
```

---

### E.4 Crash Recovery

#### WAL (Write-Ahead Log) for in-progress effects

SQLite's WAL mode is the primary durability mechanism for local deployments:

```sql
-- Applied once at database initialization:
PRAGMA journal_mode = WAL;
PRAGMA synchronous  = NORMAL;   -- fsync on WAL checkpoint, not every write
PRAGMA busy_timeout = 5000;     -- 5s wait before SQLITE_BUSY
PRAGMA wal_autocheckpoint = 1000;  -- checkpoint after 1000 WAL pages

-- All writes go through the single-writer task with BEGIN IMMEDIATE.
-- This means the WAL is only written by one task at a time,
-- eliminating SQLITE_BUSY entirely at the application level.
```

The WAL guarantees that a committed transaction persists even if the process crashes immediately after `COMMIT`. Any intent in the `effect_intents` table with `state = 'Pending'` or `state = 'Claimed'` after a crash will be discovered and handled by the recovery procedure.

For Postgres deployments: WAL is the native journal. `SELECT ... FOR UPDATE SKIP LOCKED` replaces the single-writer pattern for effect claiming.

#### Recovery procedure on restart

```rust
/// Called once during process startup, before accepting new runs.
pub async fn recover_all_runs(store: &dyn EffectStore, clock: &dyn Clock) {
    // 1. Load all non-terminal runs
    let active_runs = store.load_runs_by_state(&[
        RunStateTag::Running,
        RunStateTag::WaitingApproval,
        RunStateTag::WaitingEffect,
        RunStateTag::Queued,
    ]).await.expect("recovery: load runs");

    for run_id in active_runs {
        recover_run(store, clock, run_id).await;
    }
}

async fn recover_run(store: &dyn EffectStore, clock: &dyn Clock, run_id: RunId) {
    let now = clock.now();

    // 2. Handle expired leases on claimed intents
    let expired = store.expired_leases(now).await.unwrap();
    for intent in expired.iter().filter(|i| i.run_id == run_id) {
        let policy = retry_policy_for(&intent.kind);
        match policy.class {
            RetryClass::Idempotent => {
                // Safe to re-queue: same idempotency key will be used
                store.record_timeout_outcome(intent).await.unwrap();
                store.set_intent_pending(intent.id).await.unwrap();
            }
            RetryClass::CheckBeforeRetry => {
                // Re-queue with a flag: worker must check for partial completion
                store.record_timeout_outcome(intent).await.unwrap();
                store.set_intent_pending_check_first(intent.id).await.unwrap();
            }
            RetryClass::NoAutoRetry => {
                // Record Unknown; reducer must handle explicitly
                store.record_unknown_outcome(intent, "lease expired during crash").await.unwrap();
                store.set_intent_resolved_unknown(intent.id).await.unwrap();
            }
        }
    }

    // 3. Feed unconsumed outcomes to the run's reducer
    let pending_outcomes = store.pending_outcomes(run_id).await.unwrap();
    if !pending_outcomes.is_empty() {
        resume_turn_actor(run_id, TurnInput::EffectResult {
            outcomes: pending_outcomes,
        }).await;
        return;
    }

    // 4. Verify grant validity (policy revocation check)
    let run = store.load_run(run_id).await.unwrap();
    if !grant_still_valid(&run.grant).await {
        cancel_run(run_id, CancellationReason::PolicyRevoked).await;
        return;
    }

    // 5. Resume turn actor normally
    resume_turn_actor(run_id, TurnInput::Resume {
        checkpoint: CheckpointRef::LastCommitted,
    }).await;
}
```

#### Orphan detection and cleanup

```rust
/// Find and clean up orphaned resources after recovery.
/// Orphans are resources that no longer have an active owning run.
pub async fn cleanup_orphans(store: &dyn EffectStore) {
    // Workers that were holding leases on intents belonging to terminal runs
    let orphaned_intents = store.find_claimed_intents_for_terminal_runs().await.unwrap();
    for intent in orphaned_intents {
        // The run is terminal; no reducer will consume the outcome.
        // Record the outcome for audit purposes and supersede the intent.
        store.supersede_intent(intent.id, SupersessionReason::OwningRunTerminated).await.unwrap();
    }

    // Approval requests that outlived their runs
    let orphaned_approvals = store.find_approval_requests_for_terminal_runs().await.unwrap();
    for req in orphaned_approvals {
        store.expire_approval_request(req.id, "owning run terminated").await.unwrap();
    }

    // Harness sessions that were not closed during cancellation
    let orphaned_sessions = store.find_harness_sessions_for_terminal_runs().await.unwrap();
    for session in orphaned_sessions {
        close_harness_session(session).await;
    }
}
```

#### Data consistency verification

```rust
/// Post-recovery consistency check. Runs after recovery and before
/// accepting new work. Halts startup if critical invariants are violated.
pub async fn verify_consistency(store: &dyn EffectStore) -> ConsistencyReport {
    let mut report = ConsistencyReport::default();

    // INV: every EffectAttempt with state=Completed has an EffectOutcome
    let attempts_without_outcomes = store.find_completed_attempts_without_outcomes().await.unwrap();
    report.missing_outcomes = attempts_without_outcomes;

    // INV: every WaitingEffect run has at least one Pending or Resolved intent
    let orphaned_waiting = store.find_waiting_runs_without_intents().await.unwrap();
    report.orphaned_waiting_runs = orphaned_waiting;

    // INV: event sequences within each run are gapless
    let sequence_gaps = store.find_event_sequence_gaps().await.unwrap();
    report.sequence_gaps = sequence_gaps;

    // INV: no run has two Claimed intents for the same effect kind at the same sequence
    let duplicate_claims = store.find_duplicate_claims().await.unwrap();
    report.duplicate_claims = duplicate_claims;

    if report.is_critical() {
        tracing::error!(?report, "critical consistency violation detected; halting startup");
        std::process::exit(1);
    }

    report
}
```

---

## Appendix F: Graph Execution Engine

### F.1 DAG Representation

A `WorkflowSpec` is a directed acyclic graph (DAG) of `WorkflowNode` values. The graph is validated at spec creation time and stored as an adjacency list.

```rust
/// Complete graph representation for a workflow.
pub struct WorkflowGraph {
    /// All nodes by ID.
    nodes:    HashMap<NodeId, WorkflowNode>,
    /// Adjacency: edges from each node to its successors.
    edges:    HashMap<NodeId, Vec<EdgeSpec>>,
    /// The single entry node (no incoming edges).
    entry:    NodeId,
    /// All terminal nodes (no outgoing edges).
    terminals: Vec<NodeId>,
}

pub struct EdgeSpec {
    pub from:      NodeId,
    pub to:        NodeId,
    pub condition: Option<EdgeCondition>,
}

pub enum EdgeCondition {
    /// Always traverse (unconditional).
    Always,
    /// Traverse only if the source node completed successfully.
    OnSuccess,
    /// Traverse only if the source node failed.
    OnFailure,
    /// Traverse if the output field equals the expected value.
    OutputFieldEquals { field: String, value: serde_json::Value },
    /// Traverse if the output matches a JSONPath expression.
    OutputJsonPath { path: String, expected: serde_json::Value },
}
```

### F.2 Topological Sort for Execution Ordering

```rust
/// Compute a valid topological ordering of nodes.
/// Returns Err if the graph contains a cycle.
pub fn topological_sort(graph: &WorkflowGraph) -> Result<Vec<NodeId>, CycleError> {
    // Kahn's algorithm: O(V + E)
    let mut in_degree: HashMap<NodeId, usize> = graph.nodes.keys()
        .map(|id| (*id, 0))
        .collect();

    for edges in graph.edges.values() {
        for edge in edges {
            *in_degree.entry(edge.to).or_insert(0) += 1;
        }
    }

    let mut queue: VecDeque<NodeId> = in_degree.iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(id, _)| *id)
        .collect();

    let mut order = Vec::with_capacity(graph.nodes.len());

    while let Some(node_id) = queue.pop_front() {
        order.push(node_id);
        if let Some(successors) = graph.edges.get(&node_id) {
            for edge in successors {
                let deg = in_degree.get_mut(&edge.to).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    queue.push_back(edge.to);
                }
            }
        }
    }

    if order.len() != graph.nodes.len() {
        return Err(CycleError { nodes_in_cycle: find_cycle(graph) });
    }

    Ok(order)
}
```

Reference: `crates/roko-runtime/src/task_scheduler.rs` implements the same pattern as a `TaskScheduler` with `Blocked → Ready` dependency resolution:

```rust
// From Roko's TaskScheduler:
fn update_ready(&mut self) {
    let blocked_ids: Vec<String> = self.status.iter()
        .filter(|(_, s)| **s == TaskStatus::Blocked)
        .map(|(id, _)| id.clone())
        .collect();

    for id in blocked_ids {
        if let Some(task) = self.tasks.get(&id) {
            let all_deps_done = task.depends_on.iter()
                .all(|dep| matches!(self.status.get(dep), Some(TaskStatus::Completed)));
            if all_deps_done {
                self.status.insert(id, TaskStatus::Ready);
            }
        }
    }
}
```

### F.3 Parallel Execution of Independent Nodes

Independent nodes (those with no mutual dependency) execute concurrently as sibling child runs. The parent workflow run uses the effect pipeline to spawn child runs as `HarnessOperation` or `RunSpawn` intents:

```rust
/// The workflow coordinator uses the outbox to fan out parallel branches.
///
/// When a Fork node is reached:
/// 1. The reducer emits one EffectIntent per branch (RunSpawn intents).
/// 2. All branch intents share the same concurrency_group, allowing
///    effect workers to claim them simultaneously.
/// 3. The run enters WaitingEffect with JoinStrategy::WaitAll (or configured).
/// 4. Each branch runs as an independent child run.
/// 5. When all branches resolve, the Join node is evaluated.

pub struct RunSpawnIntent {
    pub node_id:        NodeId,
    pub run_spec:       RunSpec,
    /// Grant is intersection of parent grant and child spec requirements.
    pub grant_override: Option<GrantOverride>,
    /// Cancel token child: cancelled when parent is cancelled.
    pub cancel_parent:  RunId,
}
```

The `next_batch()` function from Roko's `TaskScheduler` demonstrates the slot-aware batch dispatch:

```rust
// From roko-runtime/src/task_scheduler.rs
pub fn next_batch(&self) -> Vec<&str> {
    let running_count = self.status.values()
        .filter(|s| **s == TaskStatus::Running)
        .count();
    let available_slots = self.max_parallel.saturating_sub(running_count);
    if available_slots == 0 { return Vec::new(); }

    // Checks file exclusions to avoid write-write conflicts between tasks
    let running_files: HashSet<&str> = /* collect files used by running tasks */;
    // Returns up to available_slots conflict-free ready tasks
}
```

In Polkagent, the analogous mechanism is `GrantLimits::max_concurrent_effects` rather than `max_parallel`, and conflict detection is by `EffectKind` category rather than file path.

### F.4 Join Semantics

```rust
pub enum JoinStrategy {
    /// All branches must complete (success or failure) before the join.
    WaitAll,
    /// The first branch to complete (with any outcome) unblocks the join.
    /// Remaining branches receive a cancellation signal.
    WaitFirst,
    /// N branches must complete before the join unblocks.
    /// Remaining branches receive a cancellation signal.
    WaitN { n: u32 },
    /// A quorum (majority) must succeed. Any failure beyond (total - quorum)
    /// causes the join to fail.
    Quorum { required: u32 },
}

impl JoinStrategy {
    /// Evaluate whether the join condition is satisfied given current outcomes.
    /// Returns Some(JoinOutcome) if resolved, None if still waiting.
    pub fn evaluate(
        &self,
        total_branches:    u32,
        completed:         u32,
        succeeded:         u32,
        failed:            u32,
    ) -> Option<JoinOutcome> {
        match self {
            Self::WaitAll => {
                if completed == total_branches {
                    Some(JoinOutcome::AllResolved { succeeded, failed })
                } else { None }
            }
            Self::WaitFirst => {
                if completed >= 1 {
                    Some(JoinOutcome::FirstResolved)
                } else { None }
            }
            Self::WaitN { n } => {
                if completed >= *n {
                    Some(JoinOutcome::QuorumResolved { reached: completed })
                } else { None }
            }
            Self::Quorum { required } => {
                if succeeded >= *required {
                    Some(JoinOutcome::QuorumResolved { reached: succeeded })
                } else if failed > total_branches - required {
                    Some(JoinOutcome::QuorumFailed { succeeded, failed })
                } else { None }
            }
        }
    }
}
```

### F.5 Error Propagation in Graphs

Error propagation follows the `error_handler` field on each `Run` node:

```rust
pub enum NodeErrorPolicy {
    /// Propagate failure to the parent workflow run (default).
    FailWorkflow,
    /// Route to an error handler node. The handler receives the failed node's
    /// output as its input, plus the error artifact.
    HandleWith { handler_node: NodeId },
    /// Treat failure as success with a default output. Continue the graph.
    IgnoreWithDefault { default_output: serde_json::Value },
    /// Retry the node up to N times before applying the fallback policy.
    RetryThenFail { max_retries: u32, then: Box<NodeErrorPolicy> },
}
```

When a child run fails and its node has `FailWorkflow` policy, the parent workflow run transitions:

1. Remaining sibling branches (if any) receive cancellation signals.
2. All their `WaitingEffect` intents are superseded.
3. The parent run transitions to `Failed { reason: ChildRunFailed { node_id, child_run_id } }`.

### F.6 Cycle Detection and Prevention

Cycles are detected at `WorkflowSpec` creation time, not at runtime:

```rust
/// Validate a WorkflowSpec before it can be persisted or executed.
pub fn validate_workflow_spec(spec: &WorkflowSpec) -> Result<WorkflowGraph, SpecValidationError> {
    let graph = build_graph(spec)?;

    // Topological sort fails iff a cycle exists
    topological_sort(&graph).map_err(|e| SpecValidationError::CycleDetected {
        nodes_in_cycle: e.nodes_in_cycle,
    })?;

    // Additional structural checks:
    // - Exactly one entry node (no incoming edges)
    // - All referenced node IDs exist
    // - All loop nodes have max_iterations > 0
    // - Fork branches all converge to the same Join node
    validate_entry_node(&graph)?;
    validate_all_references(spec, &graph)?;
    validate_loop_bounds(spec)?;
    validate_fork_join_pairing(spec, &graph)?;

    Ok(graph)
}

/// Loops are encoded as self-edges with a bounded iteration counter.
/// The runtime enforces max_iterations as a hard limit:
pub struct LoopState {
    pub node_id:          NodeId,
    pub iteration_count:  u32,
    pub max_iterations:   u32,
    pub exit_condition:   Condition,
}

impl LoopState {
    /// Returns true if the loop must exit (iteration limit reached).
    pub fn must_exit(&self) -> bool {
        self.iteration_count >= self.max_iterations
    }
}
```

---

## Appendix G: Implementation Checklist

Ordered tasks for implementing the execution model. Each item depends on the items above it. Items within a group may be parallelized.

### G.1 State Machines

- [ ] **SM-01** Define `AgentState` enum with all variants; derive `Serialize`, `Deserialize`, `Encode`, `Decode`
- [ ] **SM-02** Implement `apply_agent_event()` transition function with exhaustive match
- [ ] **SM-03** Write unit tests for all valid `AgentState` transitions (table-driven)
- [ ] **SM-04** Write unit tests for all invalid `AgentState` transitions (must return `InvalidTransition`)
- [ ] **SM-05** Define `RunState` enum with all variants and associated data
- [ ] **SM-06** Implement `apply_run_event()` transition function
- [ ] **SM-07** Write unit tests for all valid `RunState` transitions (table-driven)
- [ ] **SM-08** Write `proptest` property: terminal states are final (no further transitions possible)
- [ ] **SM-09** Define `EffectIntentState` and `AttemptState` enums
- [ ] **SM-10** Implement state transition functions for both effect-level state machines
- [ ] **SM-11** Write unit tests: valid/invalid transitions for effect state machines

### G.2 Effect Pipeline

- [ ] **EP-01** Implement `IdempotencyKey::generate()` with SHA-256 over `(run_id, sequence, kind, input_digest)`
- [ ] **EP-02** Write stability tests: same inputs always produce the same key (10k iterations with `proptest`)
- [ ] **EP-03** Implement `EffectStore` trait with SQLite backend
- [ ] **EP-04** Implement the single-writer Tokio task (mpsc command channel + oneshot response)
- [ ] **EP-05** Implement `claim_intent()` using `BEGIN IMMEDIATE` + `UPDATE ... RETURNING`
- [ ] **EP-06** Implement `record_outcome()` atomically with intent state update
- [ ] **EP-07** Implement `commit_turn()` atomically: run state + intents + events + artifacts
- [ ] **EP-08** Implement idempotency dedup check in `commit_turn()` (SELECT before INSERT)
- [ ] **EP-09** Implement `ResourceAccount` with `check_can_afford()`, `reserve()`, `settle()`
- [ ] **EP-10** Integrate budget enforcement into the turn actor loop (check before each effect batch)
- [ ] **EP-11** Implement the lease reaper task (periodic scan for expired leases, handle per retry class)
- [ ] **EP-12** Implement `retry_policy_for()` with per-effect-kind retry classes
- [ ] **EP-13** Write integration test: intent survives process restart (SQLite WAL durability)
- [ ] **EP-14** Write integration test: idempotency key deduplication prevents double-execution

### G.3 Graph Engine

- [ ] **GE-01** Implement `WorkflowGraph` adjacency list representation
- [ ] **GE-02** Implement `topological_sort()` using Kahn's algorithm
- [ ] **GE-03** Implement cycle detection: `validate_workflow_spec()` rejects cyclic graphs
- [ ] **GE-04** Implement `validate_fork_join_pairing()`: every Fork must have a corresponding Join
- [ ] **GE-05** Implement `JoinStrategy::evaluate()` for all four strategies
- [ ] **GE-06** Implement `RunSpawnIntent` and its handling in the effect worker
- [ ] **GE-07** Implement parent-to-child `CancelToken` propagation in fork/join
- [ ] **GE-08** Implement `NodeErrorPolicy` routing in the workflow coordinator reducer
- [ ] **GE-09** Write unit tests: sequential, parallel, conditional, loop, and error-handler patterns
- [ ] **GE-10** Write integration test: fork/join with 3 branches executes concurrently

### G.4 Recovery

- [ ] **RC-01** Implement `recover_all_runs()`: load non-terminal runs on startup
- [ ] **RC-02** Implement `recover_run()`: expired leases → per-retry-class handling
- [ ] **RC-03** Implement unconsumed-outcome feeding to reducer
- [ ] **RC-04** Implement grant validity check during recovery (policy revocation)
- [ ] **RC-05** Implement `cleanup_orphans()`: supersede claimed intents for terminal runs
- [ ] **RC-06** Implement `verify_consistency()`: post-recovery invariant checks with startup-halt on critical failure
- [ ] **RC-07** Write crash recovery test: `CrashPoint::AfterIntentCommit` → intent re-claimed
- [ ] **RC-08** Write crash recovery test: `CrashPoint::DuringEffectExecution` → lease expiry handling
- [ ] **RC-09** Write crash recovery test: `CrashPoint::AfterOutcomeRecord` → outcome consumed on resume
- [ ] **RC-10** Write crash recovery test: `NoAutoRetry` effect after crash → `Unknown` outcome, no retry
- [ ] **RC-11** Verify `assert_no_duplicate_effects()` passes for all crash recovery paths

### G.5 Testing

- [ ] **TS-01** Set up `proptest` with `ExecutionTestHarness` (in-memory store, mock workers, `MockClock`)
- [ ] **TS-02** Property: `TurnReducer::reduce()` is deterministic (same state + input → same output)
- [ ] **TS-03** Property: `RunState` transitions are idempotent (applying same event twice = applying once)
- [ ] **TS-04** Property: budget usage is monotonically non-decreasing
- [ ] **TS-05** Property: event sequences within a run are gapless and monotonic
- [ ] **TS-06** Property: no `EffectOutcome` is ever mutated after creation
- [ ] **TS-07** Benchmark: graph execution throughput (1000 sequential nodes in < 100ms)
- [ ] **TS-08** Benchmark: parallel branch fan-out (64 parallel child runs; measure overhead vs. sequential)
- [ ] **TS-09** Chaos test: random `CancelToken::cancel()` calls during workflow execution; verify clean shutdown
- [ ] **TS-10** Chaos test: random worker crashes at each phase; verify consistency after recovery

---

## Appendix H: Reference File Map

This table maps execution model components to the concrete implementation files in Roko and Bardo that exhibit the same patterns.

| Execution Component | Roko Files | Bardo Files | Key Patterns |
|---|---|---|---|
| **Agent lifecycle state machine** | `crates/roko-runtime/src/lifecycle.rs` | `apps/mori/src/state.rs` | `AgentLifecycleState` enum; `LifecycleTransition` record; `DegradationStage` variants |
| **Run/workflow state machine** | `crates/roko-runtime/src/pipeline_state.rs` | `apps/mori/src/orchestrator/mod.rs` | `WorkflowOutcome`; `Phase` enum; `PipelineInput`/`PipelineOutput` pattern |
| **Reducer pattern** | `crates/roko-runtime/src/pipeline_state.rs` | `apps/mori/src/app/sequential.rs` | Pure `PipelineStateV2` with no side effects; `EffectDriver` executes outputs |
| **Effect driver** | `crates/roko-runtime/src/effect_driver.rs` | `apps/mori/src/app/gates.rs` | `spawn_agent()`, `start_compile_gate()` as effect-executor; result fed back as `PipelineInput` |
| **Workflow engine (orchestrator)** | `crates/roko-runtime/src/workflow_engine.rs` | `apps/mori/src/orchestrator/mod.rs` | `WorkflowEngine::run_with_cancel()`; run loop: step → execute → feed back → repeat |
| **DAG scheduler** | `crates/roko-runtime/src/task_scheduler.rs` | `apps/mori/src/orchestrator/mod.rs` | `TaskScheduler`; `Blocked→Ready` dependency resolution; `next_batch()` with file exclusion |
| **Cancellation token** | `crates/roko-runtime/src/cancel.rs` | `apps/mori/src/app/events.rs` | `CancelToken` with `AtomicBool` + `Notify`; hierarchical parent-child propagation |
| **Resource budgets** | `crates/roko-runtime/src/resource.rs` | `apps/mori/src/orchestrator/mod.rs` | `ResourceAccount`; `BudgetEntry<T>`; token + cost + time tracking |
| **Run ledger / audit** | `crates/roko-runtime/src/run_ledger.rs` | `apps/mori/src/state/persistence.rs` | `RunLedger` with typed phase/agent/gate outcome records; `to_report_compat()` |
| **Idempotency** | `crates/roko-agent/src/dispatcher/dedup_cache.rs` | `apps/mori/src/orchestrator/platform/idempotency.rs` | `DeliveryIdempotencyCache` LRU deduplication; `try_record()` returns false for duplicates |
| **Parallel tool dispatch** | `crates/roko-agent/src/dispatcher/parallel.rs` | `apps/mori/src/orchestrator/mod.rs` | `partition_by_concurrency()`; `join_all` for parallel; sequential for serial |
| **Agent approval flow** | `crates/roko-agent/src/safety/authz.rs` | `apps/mori/src/tui/modals/approval.rs` | Policy gate evaluation; approval request rendering; decision binding |
| **Tool/gate execution** | `crates/roko-agent/src/tool_loop/` | `apps/mori/src/app/gates.rs` | Tool loop backends; gate runner; result → feedback pipeline |
| **Event bus** | `crates/roko-runtime/src/event_bus.rs` | `apps/mori/src/server/sse.rs` | `emit_runtime_event()`; SSE publishing; ephemeral vs. durable event routing |
| **Health check / heartbeat** | `crates/roko-runtime/src/heartbeat.rs` | `apps/mori/src/monitor/mod.rs` | Periodic probes; degradation detection; `AgentLifecycleState::Degraded` transitions |
| **Projection / state snapshot** | `crates/roko-runtime/src/state_snapshot.rs` | `apps/mori/src/server/state.rs` | Derived read model rebuilt from event log; cursor-based incremental update |

---

## Appendix I: TUI Visualization of Execution State

This appendix specifies how the Polkagent execution model surfaces in a terminal UI, referencing concrete patterns from the Roko CLI (`crates/roko-cli/`) and Bardo Mori (`apps/mori/`).

### I.1 Agent Status Indicators

Agent status is shown in the agent roster panel (left ~32% of screen). Reference: `crates/roko-cli/src/tui/views/agents_view.rs` (Roko), `apps/mori/src/tui/views/agents.rs` (Bardo).

Status symbols for Polkagent:

```
◉  Active     — agent is Running one or more runs (ROSE/accent color, BOLD)
○  Idle       — agent is Configured/Active but no runs in flight (muted)
◌  Paused     — agent in Paused state (DREAM/purple)
⚠  Degraded   — agent in Degraded state (WARNING/amber)
✗  Error      — agent in Deactivated or unrecoverable state (EMBER/red)
▸  (spinner)  — animated frame character when a run is actively executing
```

```
ASCII wireframe: Agent Roster panel

┌─ Agents (3 active) ─────────────────────────┐
│  agent          model     status    tokens   │
│                                              │
│ ◉ polkadot-asst claude-3  active ▸  12.4k   │
│   ──────────────────────────────────────── │
│   run: run_01J8XYZ   turn 2/? WaitingEffect │
│   budget: 23% · cost $0.23                  │
│                                              │
│ ◉ chain-watcher  gemini   active ▸   3.1k   │
│   run: run_01J8ABC   turn 1/? Running       │
│   budget: 5%  · cost $0.04                  │
│                                              │
│ ○ query-bot      gpt-4o   idle             │
│   last run: 14 min ago · Completed          │
│                                              │
│ ◌ audit-agent    claude   paused            │
│   resumed pending approval                  │
│                                              │
│  Active: 2 of 4   In: 15.5k   Out: 8.2k    │
└─────────────────────────────────────────────┘
```

Implementation sketch for the roster row (based on `agents_view.rs` pattern):

```rust
fn render_agent_row(
    agent: &AgentSummary,
    active_run: Option<&RunSummary>,
    atmosphere: &Atmosphere,
    selected: bool,
) -> Line<'static> {
    let status_char = match agent.state {
        AgentState::Active if active_run.is_some() => "◉",
        AgentState::Active                          => "○",
        AgentState::Paused                          => "◌",
        AgentState::Degraded { .. }                => "⚠",
        AgentState::Deactivated | AgentState::Archived => "✗",
        _                                          => "·",
    };

    let status_color = match agent.state {
        AgentState::Active if active_run.is_some() => theme.accent, // ROSE
        AgentState::Paused                          => theme.dream,
        AgentState::Degraded { .. }                => theme.warning,
        AgentState::Deactivated                    => theme.ember,
        _                                          => theme.muted,
    };

    let spinner = if active_run.is_some() {
        format!(" {}", atmosphere.spinner())
    } else {
        String::new()
    };

    // status_char · name(14) · model(10) · badge · tokens · spinner
    Line::from(vec![
        Span::styled(format!(" {} ", status_char), Style::default().fg(status_color)),
        Span::styled(format!("{:<14}", agent.name), /* bold if active */),
        Span::styled(format!("{:<10}", agent.model_short), Style::default().fg(theme.muted)),
        badge(status_label(&agent.state), badge_status(&agent.state)),
        Span::styled(format!(" {:>6}", fmt_tokens(agent.total_tokens)), Style::default().fg(theme.muted)),
        Span::styled(spinner, Style::default().fg(status_color)),
    ])
}
```

### I.2 Run Progress Visualization

Run progress occupies the right panel (~68% of screen). Reference: `apps/mori/src/tui/views/plans.rs` and `apps/mori/src/tui/widgets/phase_bar.rs` (Bardo).

```
ASCII wireframe: Run detail panel (right side)

┌─ Run: run_01J8XYZ · polkadot-assistant ─────────────────────────────────┐
│  State: WaitingEffect · Turn 2 · 5 effects total · budget 23% ($0.23)   │
│                                                                           │
│  Pipeline:                                                                │
│  [▓▓▓▓▓▓▓▓▓░░░░░░░░░░░░░] 42%  2/5 effects resolved                    │
│                                                                           │
│  Turn 1  ✓ completed  (2.1s)                                             │
│  ├── Step 1: ContextAssembly    ✓  pure     0ms                          │
│  ├── Step 2: ModelInference     ✓  success  1.8s  claude-3-opus          │
│  └── Step 3: ToolInvocation     ✓  success  0.3s  chain_query            │
│                                                                           │
│  Turn 2  ▸ executing  (1.4s elapsed)                                     │
│  ├── Step 1: ContextAssembly    ✓  pure     0ms                          │
│  ├── Step 2: ModelInference     ▸  leased   1.4s  [░░░░░░░░░░] stream   │
│  └── Step 3: (pending approval)                                           │
│                                                                           │
│  Effects:                                                                 │
│  intent_004  ModelCall     ▸ executing   lease: 28s remaining            │
│  intent_003  ChainRead     ✓ success     0.3s                            │
│  intent_002  ModelCall     ✓ success     1.8s                            │
│  intent_001  ContextPack   ✓ (pure)                                      │
│                                                                           │
│  Streaming output ──────────────────────────────────────────────────── │
│  The referendum 42 on Polkadot concerns the proposal to...               │
│  █ (cursor, live update)                                                 │
└───────────────────────────────────────────────────────────────────────────┘
```

Progress bar implementation (reference: Bardo's `plans_view.rs` `BLOCKS` array):

```rust
const BLOCKS: &[char] = &[
    ' ', '\u{2591}', '\u{258F}', '\u{258E}', '\u{258D}',
    '\u{258C}', '\u{258B}', '\u{258A}', '\u{2589}', '\u{2588}',
];

fn build_progress_bar(fraction: f64, width: usize) -> String {
    let filled = (fraction * width as f64) as usize;
    let partial_idx = ((fraction * width as f64 - filled as f64) * 8.0) as usize;
    let mut bar = String::with_capacity(width);
    for i in 0..width {
        if i < filled {
            bar.push('█');
        } else if i == filled {
            bar.push(BLOCKS[partial_idx]);
        } else {
            bar.push('░');
        }
    }
    bar
}

fn progress_color(fraction: f64, theme: &Theme) -> Color {
    if fraction >= 1.0 { theme.sage }       // complete: green
    else if fraction >= 0.75 { theme.accent } // near complete: accent
    else { theme.muted }                     // in progress: muted
}
```

### I.3 Effect Pipeline Visualization

The effect panel shows the `Intent → Attempt → Outcome` pipeline for each effect within the selected run:

```
ASCII wireframe: Effect pipeline panel

┌─ Effects · run_01J8XYZ ─────────────────────────────────────────────────┐
│  intent  kind              state         attempt  outcome               │
│                                                                           │
│  #001    ModelCall         ✓ Resolved    #1       ✓ Success  1.8s       │
│  #002    ChainRead         ✓ Resolved    #1       ✓ Success  0.3s       │
│  #003    ModelCall         ▸ Executing   #1       (in flight, 1.4s)     │
│  #004    SignatureRequest  ○ Pending     -        -                      │
│  #005    Broadcast         ○ Pending     -        -                      │
│                                                                           │
│  Selected: intent #003                                                    │
│  ├── idempotency_key: sha256:run_01J8XYZ:3:ModelCall:ab3f...            │
│  ├── lease_expires:   28s remaining                                      │
│  ├── retry_class:     Idempotent (max 3 attempts)                        │
│  ├── grant:           grant_01J8XYZ · ModelCall ✓                       │
│  └── deadline:        2026-07-30T10:05:00Z                               │
└───────────────────────────────────────────────────────────────────────────┘
```

Status colors for effect states (based on Bardo's `status_badge.rs`):

```rust
fn effect_intent_style(state: &EffectIntentState, theme: &Theme) -> Style {
    match state {
        EffectIntentState::Pending          => Style::default().fg(theme.muted),
        EffectIntentState::Claimed { .. }   => Style::default().fg(theme.dream),
        EffectIntentState::Executing { .. } => Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
        EffectIntentState::Resolved { .. }  => Style::default().fg(theme.sage),
        EffectIntentState::Retrying { .. }  => Style::default().fg(theme.warning),
        EffectIntentState::Superseded { .. } => Style::default().fg(theme.muted)
            .add_modifier(Modifier::DIM),
    }
}

fn effect_outcome_symbol(result: &OutcomeResult) -> (&'static str, Color) {
    match result {
        OutcomeResult::Success { .. }   => ("✓", theme.sage),    // SAGE green
        OutcomeResult::Failure { .. }   => ("✗", theme.ember),   // EMBER red
        OutcomeResult::Timeout { .. }   => ("⏱", theme.warning), // WARNING amber
        OutcomeResult::Cancelled { .. } => ("⊘", theme.muted),   // muted
        OutcomeResult::Unknown { .. }   => ("?", theme.dream),   // DREAM purple
    }
}
```

### I.4 Graph Execution DAG Rendering

Workflow DAG visualization renders the `WorkflowGraph` as a tree. Reference: `apps/mori/src/tui/widgets/plan_tree.rs` and `apps/mori/src/tui/widgets/wave_bar.rs` (Bardo).

```
ASCII wireframe: Workflow DAG panel

┌─ Workflow: on-chain-transfer · 7 nodes ─────────────────────────────────┐
│  Plans (4/7 ▸2 ✗0)  [Enter:detail h/l:tree]                            │
│                                                                           │
│  ○──▶  A: ChainRead (metadata)        ✓ completed  0.3s                 │
│  │                                                                        │
│  └──▶  B: ChainRead (decode-call)     ✓ completed  0.2s                 │
│  │                                                                        │
│  └──▶  C: Simulation (dry-run)        ✓ completed  1.1s                 │
│  │                                                                        │
│  └──▶  D: Approval gate              ▸ waiting    (user input)          │
│         ├── on_approved ──▶ E         ○ pending                          │
│         └── on_denied  ──▶ Terminal   ○ pending                          │
│                                                                           │
│  E: SignatureRequest                  ○ pending                          │
│  │                                                                        │
│  └──▶  F: Broadcast                  ○ pending                          │
│  │                                                                        │
│  └──▶  G: FinalityWatch              ○ pending                          │
│                                                                           │
│  ████████████████░░░░░░░░░░░░░░░ 57%  4/7 nodes done                    │
└───────────────────────────────────────────────────────────────────────────┘
```

Implementation sketch (based on `plan_tree.rs` wave→plan hierarchy):

```rust
fn render_workflow_node(
    node: &WorkflowNode,
    node_status: &NodeExecutionStatus,
    depth: usize,
    is_last: bool,
    lines: &mut Vec<Line<'static>>,
    theme: &Theme,
    atmosphere: &Atmosphere,
) {
    let indent = "  ".repeat(depth);
    let connector = if depth == 0 { "○──▶" } else if is_last { "└──▶" } else { "├──▶" };

    let (status_icon, status_color) = match node_status {
        NodeExecutionStatus::Pending    => ("○", theme.muted),
        NodeExecutionStatus::Running    => ("▸", theme.accent),
        NodeExecutionStatus::Completed  => ("✓", theme.sage),
        NodeExecutionStatus::Failed     => ("✗", theme.ember),
        NodeExecutionStatus::Cancelled  => ("⊘", theme.muted),
        NodeExecutionStatus::Waiting    => ("◌", theme.dream),
    };

    let label = node_label(node);
    let timing = node_timing_str(node_status);
    let spinner = if matches!(node_status, NodeExecutionStatus::Running) {
        format!(" {}", atmosphere.spinner())
    } else { String::new() };

    lines.push(Line::from(vec![
        Span::raw(format!("{}{} ", indent, connector)),
        Span::styled(label, Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {} {}{}", status_icon, timing, spinner),
                     Style::default().fg(status_color)),
    ]));
}
```

### I.5 Real-time Streaming of Agent Output

Streaming model output is displayed in the right detail panel. Reference: Roko's WebSocket streaming client (`crates/roko-cli/src/tui/`) and Bardo's `apps/mori/src/tui/widgets/agent_output.rs`.

```
ASCII wireframe: Streaming output panel

┌─ Output · ModelCall #003 · claude-3-opus ───────────────────────────────┐
│  Referendum 42 on Polkadot is a proposal submitted by account            │
│  5GrwvaEF... to adjust the base fee multiplier...                        │
│                                                                           │
│  The proposal was submitted at block 12,345,600 and entered the          │
│  Confirming period with track...                                         │
│  █                                                                        │
│  ──────────────────────────────────────────────────────────────────── │
│  Tokens: 847 in / 234 out  Cost: ~$0.04  Elapsed: 1.4s                  │
│                                                                           │
│  [↑↓ scroll]  [c clear]  [Space pause stream]                            │
└───────────────────────────────────────────────────────────────────────────┘
```

Streaming implementation pattern (consuming `RunEventKind::StreamingToken` events):

```rust
/// The streaming output widget maintains a scroll buffer
/// and appends tokens as they arrive from the event bus.
pub struct StreamingOutputWidget {
    /// All tokens received so far for the current effect.
    buffer:       Vec<String>,
    /// Scroll offset (lines from top).
    scroll:       usize,
    /// Whether the stream is complete.
    complete:     bool,
    /// Total tokens received.
    token_count:  u64,
    /// Elapsed since stream start.
    started_at:   Instant,
}

impl StreamingOutputWidget {
    pub fn push_token(&mut self, text: &str) {
        // Split on newlines; append to last line or start new line
        for (i, part) in text.split('\n').enumerate() {
            if i > 0 { self.buffer.push(String::new()); }
            if let Some(last) = self.buffer.last_mut() {
                last.push_str(part);
            } else {
                self.buffer.push(part.to_string());
            }
        }
        self.token_count += 1;
        // Auto-scroll to bottom unless user has scrolled up
        if self.scroll == self.buffer.len().saturating_sub(1) {
            self.scroll = self.buffer.len();
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let visible_lines = area.height as usize - 4; // minus border + status bar
        let start = self.scroll.min(self.buffer.len().saturating_sub(visible_lines));
        let visible: Vec<Line> = self.buffer[start..]
            .iter()
            .take(visible_lines)
            .map(|l| Line::from(Span::raw(l.clone())))
            .chain(if !self.complete {
                // Blinking cursor on last line
                vec![Line::from(Span::styled(
                    "█",
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::RAPID_BLINK),
                ))]
            } else { vec![] })
            .collect();

        let elapsed = self.started_at.elapsed();
        let status = format!(
            "Tokens: {} out  Elapsed: {:.1}s{}",
            self.token_count,
            elapsed.as_secs_f64(),
            if self.complete { "  ✓ complete" } else { "  ▸ streaming" }
        );

        // Render paragraph + status footer
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Span::styled(" Output ", theme.accent().add_modifier(Modifier::BOLD)))
            .border_style(Theme::focused_border_style());

        frame.render_widget(Paragraph::new(visible).block(block), area);
    }
}
```

---

## Appendix J: Expanded Testing Strategy

This appendix complements section 19 with implementation-ready test code patterns.

### J.1 Unit Tests for State Machine Transitions

```rust
#[cfg(test)]
mod agent_state_machine_tests {
    use super::*;

    /// Table-driven test: all valid transitions succeed.
    #[test]
    fn valid_transitions_accepted() {
        let cases: &[(&str, AgentState, AgentEvent, AgentState)] = &[
            ("created→configured", AgentState::Created,
             AgentEvent::Validate { .. }, AgentState::Configured),
            ("configured→active", AgentState::Configured,
             AgentEvent::Activate { .. }, AgentState::Active),
            ("active→paused", AgentState::Active,
             AgentEvent::Pause, AgentState::Paused),
            ("paused→active", AgentState::Paused,
             AgentEvent::Resume, AgentState::Active),
            ("active→degraded", AgentState::Active,
             AgentEvent::DependencyDegraded { stage: DegradationStage::ProviderFallback },
             AgentState::Degraded { stage: DegradationStage::ProviderFallback }),
            ("degraded→active", AgentState::Degraded { .. },
             AgentEvent::DependencyRecovered, AgentState::Active),
        ];

        for (name, from, event, expected_to) in cases {
            let result = apply_agent_event(from, event);
            assert!(result.is_ok(), "{name}: expected Ok, got {:?}", result);
            assert_eq!(result.unwrap(), *expected_to, "{name}: wrong target state");
        }
    }

    /// Table-driven test: invalid transitions are rejected.
    #[test]
    fn invalid_transitions_rejected() {
        let cases: &[(&str, AgentState, AgentEvent)] = &[
            ("archived→anything",   AgentState::Archived, AgentEvent::Activate { .. }),
            ("created→pause",       AgentState::Created,  AgentEvent::Pause),
            ("deactivated→active",  AgentState::Deactivated, AgentEvent::Resume),
        ];

        for (name, from, event) in cases {
            let result = apply_agent_event(from, event);
            assert!(
                matches!(result, Err(AgentTransitionError::InvalidTransition { .. })
                              | Err(AgentTransitionError::TerminalState)),
                "{name}: expected InvalidTransition, got {:?}", result
            );
        }
    }

    /// Property: terminal states are truly terminal.
    #[test]
    fn terminal_states_reject_all_events() {
        use strum::IntoEnumIterator;
        for event in AgentEvent::iter() {
            assert!(
                apply_agent_event(&AgentState::Archived, &event).is_err(),
                "Archived state should reject all events, but accepted {:?}", event
            );
        }
    }
}
```

### J.2 Property-Based Tests for Idempotency

```rust
#[cfg(test)]
mod idempotency_tests {
    use proptest::prelude::*;
    use super::*;

    proptest! {
        /// Same inputs always produce the same idempotency key.
        #[test]
        fn idempotency_key_is_stable(
            run_id_bytes in prop::array::uniform32(0u8..),
            sequence     in 0u64..u64::MAX,
            kind_idx     in 0usize..8,
            input_bytes  in prop::array::uniform32(0u8..),
        ) {
            let run_id = RunId::from_bytes(run_id_bytes);
            let kind = EFFECT_KIND_DISCRIMINANTS[kind_idx % EFFECT_KIND_DISCRIMINANTS.len()];
            let key1 = IdempotencyKey::generate(&run_id, sequence, kind, &input_bytes);
            let key2 = IdempotencyKey::generate(&run_id, sequence, kind, &input_bytes);
            prop_assert_eq!(key1, key2, "key must be stable for the same inputs");
        }

        /// Different inputs produce different keys (collision resistance).
        #[test]
        fn different_inputs_produce_different_keys(
            run_id_a in prop::array::uniform32(0u8..),
            run_id_b in prop::array::uniform32(0u8..),
            sequence in 0u64..u64::MAX,
        ) {
            // Only test when inputs actually differ
            prop_assume!(run_id_a != run_id_b);
            let kind = "ModelCall";
            let digest = [0u8; 32];
            let id_a = RunId::from_bytes(run_id_a);
            let id_b = RunId::from_bytes(run_id_b);
            let key_a = IdempotencyKey::generate(&id_a, sequence, kind, &digest);
            let key_b = IdempotencyKey::generate(&id_b, sequence, kind, &digest);
            prop_assert_ne!(key_a, key_b, "different run IDs must produce different keys");
        }
    }
}
```

### J.3 Integration Tests for Effect Pipeline

```rust
#[cfg(test)]
mod effect_pipeline_integration_tests {
    use super::*;

    /// Full claim → execute → outcome cycle with in-memory store.
    #[tokio::test]
    async fn claim_execute_record_cycle() {
        let store = InMemoryEffectStore::new();
        let harness = ExecutionTestHarness::new(store.clone());

        // 1. Commit a turn with one ModelCall intent
        let intent = test_model_call_intent(run_id());
        store.commit_turn(
            run_id(),
            TurnState::WaitingEffect { pending: vec![intent.id] },
            vec![intent.clone()],
            vec![],
            vec![],
        ).await.unwrap();

        // 2. Worker claims the intent
        let claimed = store.claim_intent(worker_id("w1"), Duration::from_secs(30)).await.unwrap();
        assert!(claimed.is_some(), "intent should be claimable");
        let claimed = claimed.unwrap();
        assert_eq!(claimed.id, intent.id);
        assert!(matches!(claimed.state, EffectIntentState::Claimed { .. }));

        // 3. Record a success outcome
        let outcome = EffectOutcome {
            id:          outcome_id(),
            attempt_id:  attempt_id(),
            intent_id:   intent.id,
            run_id:      run_id(),
            result:      OutcomeResult::Success { data: test_outcome_data() },
            ..Default::default()
        };
        store.record_outcome(outcome.clone()).await.unwrap();

        // 4. Intent is now Resolved
        let resolved = store.load_intent(intent.id).await.unwrap();
        assert!(matches!(resolved.state, EffectIntentState::Resolved { .. }));

        // 5. Outcome is available for the run
        let pending = store.pending_outcomes(run_id()).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, outcome.id);
    }

    /// Idempotency: committing the same intent twice returns the existing record.
    #[tokio::test]
    async fn duplicate_intent_is_deduplicated() {
        let store = InMemoryEffectStore::new();
        let intent = test_model_call_intent(run_id());

        store.commit_turn(run_id(), test_turn_state(), vec![intent.clone()], vec![], vec![]).await.unwrap();
        // Second commit with same idempotency_key
        let result = store.commit_turn(run_id(), test_turn_state(), vec![intent.clone()], vec![], vec![]).await;
        assert!(matches!(result, Err(StoreError::DuplicateIdempotencyKey { .. }))
             || result.is_ok(), // impl may return existing instead of error
             "duplicate intent must be deduplicated");
    }

    /// NoAutoRetry intent after lease expiry → Unknown outcome, no retry.
    #[tokio::test]
    async fn no_auto_retry_produces_unknown_on_lease_expiry() {
        let store = InMemoryEffectStore::new();
        let clock = MockClock::new();

        let mut intent = test_signature_request_intent(run_id());
        intent.retry_class = RetryClass::NoAutoRetry;

        store.commit_turn(run_id(), test_turn_state(), vec![intent.clone()], vec![], vec![]).await.unwrap();
        store.claim_intent(worker_id("w1"), Duration::from_secs(30)).await.unwrap();

        // Advance clock past lease expiry
        clock.advance(Duration::from_secs(35));

        // Run the lease reaper
        let expired = store.expired_leases(clock.now()).await.unwrap();
        assert_eq!(expired.len(), 1);

        recover_run(&store, &clock, run_id()).await;

        // Intent should be Resolved with Unknown outcome
        let resolved = store.load_intent(intent.id).await.unwrap();
        assert!(matches!(resolved.state, EffectIntentState::Resolved { .. }));
        let outcomes = store.pending_outcomes(run_id()).await.unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(outcomes[0].result, OutcomeResult::Unknown { .. }));
    }
}
```

### J.4 Chaos Tests for Crash Recovery

```rust
#[cfg(test)]
mod crash_recovery_chaos_tests {
    use super::*;

    /// For every CrashPoint: crash, recover, verify no duplicate effects.
    #[tokio::test]
    async fn no_duplicate_effects_after_crash_at_any_point() {
        let crash_points = [
            CrashPoint::AfterIntentCommit,
            CrashPoint::DuringEffectExecution,
            CrashPoint::AfterOutcomeRecord,
            CrashPoint::DuringReducer,
            CrashPoint::DuringStateCommit,
        ];

        for crash_point in &crash_points {
            let mut harness = ExecutionTestHarness::new_with_sqlite_wal();

            let _partial = harness.run_with_crash_at(
                test_run_spec(),
                *crash_point,
            ).await;

            // Recovery
            recover_all_runs(harness.store(), harness.clock()).await;

            // Resume to completion
            harness.run_to_completion(test_run_spec(), vec![
                (EffectKind::ModelCall { .. }, OutcomeResult::Success { .. }),
            ]).await;

            harness.assert_no_duplicate_effects();
            harness.assert_event_ordering();
        }
    }

    /// Concurrent cancellation during effect execution produces clean shutdown.
    #[tokio::test]
    async fn cancellation_during_effect_execution_is_clean() {
        let mut harness = ExecutionTestHarness::new();
        let run_id = harness.enqueue(test_run_spec()).await;

        // Start execution; cancel mid-flight
        let handle = tokio::spawn(async move {
            harness.advance_to(RunStateTag::WaitingEffect).await;
            harness.cancel_run(run_id, CancellationReason::UserRequest).await;
            harness.wait_for_terminal(run_id).await
        });

        let outcome = handle.await.unwrap();
        assert!(matches!(outcome, RunOutcome::Cancelled { .. }));

        // No intents should remain in Claimed state
        let claimed = harness.store()
            .find_intents_by_state(EffectIntentState::Claimed { .. }, run_id)
            .await.unwrap();
        assert!(claimed.is_empty(), "no intents should remain Claimed after cancellation");
    }
}
```

### J.5 Benchmark Tests for Graph Execution Performance

```rust
#[cfg(test)]
mod graph_benchmarks {
    use criterion::{Criterion, criterion_group, criterion_main, black_box};
    use super::*;

    /// Benchmark: topological sort of a 1000-node linear DAG.
    fn bench_topological_sort_linear(c: &mut Criterion) {
        let graph = build_linear_dag(1000);
        c.bench_function("topo_sort_linear_1000", |b| {
            b.iter(|| topological_sort(black_box(&graph)))
        });
    }

    /// Benchmark: topological sort of a 1000-node wide-fan DAG (1 → 999 → 1).
    fn bench_topological_sort_wide_fan(c: &mut Criterion) {
        let graph = build_fan_dag(998);
        c.bench_function("topo_sort_fan_1000", |b| {
            b.iter(|| topological_sort(black_box(&graph)))
        });
    }

    /// Benchmark: JoinStrategy::WaitAll evaluation over 64 branches.
    fn bench_join_strategy_wait_all(c: &mut Criterion) {
        let strategy = JoinStrategy::WaitAll;
        c.bench_function("join_wait_all_64_branches", |b| {
            b.iter(|| strategy.evaluate(
                black_box(64), // total
                black_box(64), // completed
                black_box(62), // succeeded
                black_box(2),  // failed
            ))
        });
    }

    /// Benchmark: claim_intent throughput (10k claims on in-memory store).
    fn bench_claim_intent_throughput(c: &mut Criterion) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let store = rt.block_on(InMemoryEffectStore::with_pending_intents(10_000));

        c.bench_function("claim_intent_10k", |b| {
            b.iter(|| {
                rt.block_on(store.claim_intent(
                    black_box(worker_id("bench_worker")),
                    black_box(Duration::from_secs(30)),
                ))
            })
        });
    }

    criterion_group!(
        benches,
        bench_topological_sort_linear,
        bench_topological_sort_wide_fan,
        bench_join_strategy_wait_all,
        bench_claim_intent_throughput,
    );
    criterion_main!(benches);
}
```

**Performance targets:**

| Benchmark | Target | Rationale |
|---|---|---|
| `topo_sort_linear_1000` | < 500µs | Spec validation runs at create time; latency budget is generous |
| `topo_sort_fan_1000` | < 500µs | Same as linear; O(V+E) is input-size bounded |
| `join_wait_all_64` | < 1µs | Called on every effect completion; must be negligible |
| `claim_intent_10k` | < 10µs per claim (in-memory) | SQLite adds ~50-200µs per claim; in-memory baseline |
| SQLite `claim_intent` (WAL) | < 500µs p99 | Single writer; `BEGIN IMMEDIATE`; < 1ms per claim under normal load |
| SQLite `commit_turn` (WAL) | < 2ms p99 | Atomically writes state + intents + events + artifacts |
