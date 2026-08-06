# Approval pause-and-resume design

**Status:** active execution design; APR-00/APR-01 SQLite foundation, APR-02
policy/composition, and the APR-04 ACP protocol harness implemented; runtime
and surface integration incomplete

**Prepared:** 2026-08-06

**Scope:** durable permission gating for registered tool effects through the
shared runtime, `InteractionService`, terminal chat, TUI Console, HTTP control
plane, and ACP editor surface. This packet refines the open approval work in
[`PRD-19`](PRD-19-INTERACTIVE-CONSOLE-ACP.md).

## Outcome

A grant-bearing tool call can be proposed, displayed, approved or denied, and
resumed without external I/O occurring before authorization. The decision and
execution continuation survive client disconnect and process restart. Every
surface uses the same approval identity and application operation.

This document does not claim that the end-to-end capability exists today.

## 1. Status quo and evidence

| Area | Present | Blocking gap |
|---|---|---|
| Interaction contract | `ApprovalView`, `ApprovalRequested`, `ApprovalResolved`, `approve`, `deny`, and typed `/approve` and `/deny` exist in `polkagent-interaction` | Production `runtime/src/interaction.rs` returns `Unavailable`; pending lookup is empty in chat and TUI adapters |
| Tool execution | Grantless registered tools persist intent, claim, attempt, outcome, and correlated events before/after I/O | `run/src/orchestrator.rs` withholds every tool with `required_grant`; its `GrantResolver` is unused and `ToolContext.grants` is empty |
| Policy | Default-deny evaluation, explicit deny/permit/`RequireApproval` effects, strict named-file loading, gate escalation, and exact resolver injection/readiness are implemented | The orchestrator does not yet consume the resolver for grant-bearing tool continuation; durable approval authority is not composed into the runtime execution path |
| Effect persistence | SQLite V18 adds durable approval/checkpoint records, makes `effect_intents.state` authoritative, clears legacy `claimed_by` sentinels, and enforces approval/lease invariants | The orchestrator and application services do not yet call the coordinator; non-SQLite adapters do not advertise this capability |
| Application service | Approval methods and a broadcast channel exist | Methods only verify an effect and send a process-local boolean; no decision is persisted |
| API | Approve/deny routes exist | They expect unreachable states, create an ephemeral record, and emit bus-only events |
| Run lifecycle | `AwaitingApproval` and `WaitingEffect` states exist; the store now exposes exact-state run CAS and the SQLite coordinator commits run/effect/approval/checkpoint/event transitions together | Existing orchestration/service paths are not wired to the coordinator, denial still follows legacy behavior there, and the orchestrator never pauses |
| Recovery | Durable interaction replay, abandoned-run recovery, and a versioned SQLite checkpoint store with version/lease CAS exist | Startup does not yet rehydrate approval checkpoints or reconcile retry-class outcomes; the live model loop still keeps its continuation in memory |
| ACP | Durable sessions, prompts, cancellation, tool updates, slash discovery, safe permission-request projection, and an official-SDK protocol harness exist | The production backend does not issue native permission requests or bind decisions to the durable coordinator/effect path |
| Terminal/TUI | Approval rendering, commands, and approval views exist | They do not resolve the shared durable approval operation end to end |

The missing center is not another widget. It is a durable transaction boundary
that binds policy, the exact effect subject, approval identity, run state, and
a resumable continuation.

## 2. Blocking ADR decisions

No implementation packet that mutates effect, run, or approval state starts
until APR-ADR-01 through APR-ADR-05 are accepted in an ADR or recorded as
accepted amendments here.

| ID | Decision to freeze | Recommended decision |
|---|---|---|
| APR-ADR-01 | Durable source of truth and transaction boundary | Add an `ApprovalCoordinatorStore` unit-of-work port. The approval row and exact effect digest are authoritative; broadcasts only wake readers. Approval/effect/run/outbox transitions commit in one transaction with expected-state CAS. |
| APR-ADR-02 | Policy versus human authority | Explicit or default policy deny never prompts and cannot be overridden. Permit executes. Only an explicit `RequireApproval`/escalation result creates a request. Load the configured policy/resolver through the runtime builder. |
| APR-ADR-03 | User rejection semantics | A user `RejectOnce` produces a typed permission-denied tool result and resumes the model. Run cancellation, approval expiry at the run deadline, and ACP `Cancelled` cancel or time out the run instead. Add a reject-effect transition; do not reuse the current cancel-on-deny transition. |
| APR-ADR-04 | Crash-resume boundary | Persist a versioned typed executor checkpoint at every model response that can yield effects. Reconstructing from interaction text alone is forbidden because it loses tool grouping and typed messages. |
| APR-ADR-05 | Remembered decisions | Ship `AllowOnce` and `RejectOnce` only. `AllowAlways`/`RejectAlways` require a separate durable mandate model, scope UI, revocation, expiry, and audit design. |

If APR-ADR-03 is rejected for a smaller first delivery, cancellation-on-denial
must be named as temporary product behavior in every surface and test. It must
not be described as an IDE-quality agent loop.

## 3. Required durable ports

### 3.1 Approval coordinator

Add a narrow store-neutral port rather than composing independent
`EffectStore::update_intent_state`, `RunStore::update_state`, and event calls:

```rust
trait ApprovalCoordinatorStore {
    async fn pause_for_approval(
        &self,
        request: PauseForApproval,
    ) -> Result<StoredApproval, ApprovalStoreError>;

    async fn resolve_approval(
        &self,
        request: ResolveApproval,
    ) -> Result<ResolveApprovalResult, ApprovalStoreError>;

    async fn get_approval(&self, id: ApprovalId)
        -> Result<StoredApproval, ApprovalStoreError>;

    async fn list_pending(&self, scope: ApprovalScope, page: Page)
        -> Result<Vec<StoredApproval>, ApprovalStoreError>;

    async fn claim_approved_effect(
        &self,
        request: ClaimApprovedEffect,
    ) -> Result<ClaimedEffect, ApprovalStoreError>;
}
```

`pause_for_approval` atomically:

1. checks the run is `Running` and the turn/effect lineage is exact;
2. inserts the unclaimable effect and its canonical subject digest;
3. inserts one pending approval, unique by effect ID;
4. stores the paused execution checkpoint;
5. transitions the run to `AwaitingApproval { approval_id }`; and
6. appends a durable approval-requested outbox/run event.

`resolve_approval` validates the conversation, turn, run, effect, digest,
deadline, and authorized principal. It then performs a single pending-to-
terminal CAS, transitions the effect and run according to the tables below,
and appends a durable decision event in the same transaction. Retrying the
same decision returns the original record; a different or late decision is a
conflict.

`claim_approved_effect` is the only operation that may move an approved effect
to a leased execution state. Generic pending workers must not claim it.

### 3.2 Execution checkpoint

Add a versioned `ExecutionCheckpointStore`, composed by the coordinator for
the pause transaction and by the orchestrator for recovery:

```rust
trait ExecutionCheckpointStore {
    async fn lease_resumable(
        &self,
        worker: WorkerId,
        lease: Duration,
        page: Page,
    ) -> Result<Vec<LeasedCheckpoint>, CheckpointError>;

    async fn commit_progress(
        &self,
        expected_version: u64,
        next: CheckpointProgress,
    ) -> Result<u64, CheckpointError>;
}
```

The checkpoint contains or safely references:

- schema version, run/turn/conversation/agent identity, model and executor;
- typed inference messages needed to resume, including assistant text and the
  complete parallel tool-call group;
- exact tool-call IDs, tool names, validated arguments, required grant,
  canonical subject digest, and effect IDs;
- next model-turn, step, and effect sequence numbers;
- accumulated usage, cost, deadline, and retry classification;
- current pending/approved/denied effects and a monotonic checkpoint version;
- data classification, retention expiry, and integrity digest.

Checkpoint leases and `commit_progress(expected_version, ...)` prevent two
recovery workers from resuming the same model loop. A terminal run removes or
tombstones the resumable checkpoint according to retention policy.

### 3.3 Projection boundary

The transaction writes a durable outbox/run event. The event bus publishes
only after commit. `InteractionService` projects that record into stable,
idempotent `ApprovalRequested` and `ApprovalResolved` envelopes. Restart and
lag recovery backfill from the durable record; they never depend on a process-
local receiver.

Stable interaction event IDs derive from the approval ID plus an event-kind
discriminator. The pending approval query reads the approval store, not a live
event stream.

## 4. State machines

### 4.1 Approval

| Current | Operation | Next | Notes |
|---|---|---|---|
| absent | pause | `Pending` | One row per exact effect; stable caller-supplied ID |
| `Pending` | allow once | `Approved` | Records principal, surface, rationale, policy/digest snapshot, and time |
| `Pending` | reject once | `Denied` | Produces a no-I/O tool rejection for the continuation |
| `Pending` | run deadline | `Expired` | Run becomes `TimedOut`; effect never executes |
| `Pending` | run/session cancel | `Cancelled` | Run becomes `Cancelled`; effect never executes |
| terminal | repeat identical operation | unchanged | Idempotent; return original record |
| terminal | different/late operation | conflict | Never change the recorded decision |

### 4.2 Effect

| Current | Operation | Next | I/O allowed? |
|---|---|---|---|
| absent | pause transaction | `AwaitingApproval` | No |
| `AwaitingApproval` | approval allowed | `Approved` | No |
| `AwaitingApproval` | approval rejected | `Denied` | No; terminal no-I/O effect |
| `AwaitingApproval` | approval expired | `Expired` | No; terminal |
| `AwaitingApproval` | approval cancelled | `Cancelled` | No; terminal |
| `Approved` | exact continuation claims | `Claimed` | No, until attempt start is durable |
| `Claimed` | attempt start committed | `Executing` | Yes |
| `Executing` | immutable outcome committed | `Resolved` | I/O finished or reconciled |
| expired lease | retry policy permits | `Approved` or reconciliation | Never auto-retry `NoAutoRetry` |

The real `effect_intents.state` column becomes authoritative. `claimed_by`
stores only a worker identity and must no longer encode terminal states.

### 4.3 Run

| Current | Operation | Next |
|---|---|---|
| `Running` | pause transaction | `AwaitingApproval { approval_id }` |
| `AwaitingApproval` | allow/reject decision | `WaitingEffect { effect_id }` |
| `AwaitingApproval` | cancel | `Cancelled` |
| `AwaitingApproval` | deadline | `TimedOut` |
| `WaitingEffect` | approved effect resolves and result is reduced | `Running` |
| `WaitingEffect` | denied effect becomes typed tool result and is reduced | `Running` |
| `WaitingEffect` | unrecoverable or indeterminate effect | `Failed` or explicit manual-reconciliation state |

All transitions compare both expected state and embedded approval/effect ID.
The current unconditional run update is not sufficient.

## 5. Safety and reliability invariants

### Authorization and default deny

- Default deny or explicit deny creates no approval and performs no I/O.
- Human approval is one-shot authority for the exact policy-escalated subject;
  it does not grant unrelated actions or bypass path, custody, tenant, budget,
  or sandbox enforcement.
- The `ResolvedGrant` produced after approval is passed into `ToolContext` and
  is revalidated immediately before dispatch.
- Missing policy, approval store, checkpoint version, security context, or
  principal identity fails closed and prevents advertising the tool.

### Identity and idempotency

- Approval ID, effect ID, tool-call ID, run, turn, conversation, agent, and
  principal are stored and checked as one lineage.
- The approval subject digest covers a versioned canonical encoding of tool
  name, validated arguments, required action/resource, working-directory and
  security scope, agent/run identity, and tool-spec fingerprint.
- Changed arguments, policy snapshot, security scope, or tool registration
  require a new approval. A stale approval cannot authorize changed code.
- Exactly one effect outcome and exactly one continuation reduction may win.

### Crash and retry

- Crash after pause commit leaves a replayable pending request.
- Crash after approval but before claim leaves an approved, unclaimable-by-
  generic-workers effect and a resumable checkpoint.
- Crash after claim but before I/O releases or re-leases according to the
  checkpoint and retry class.
- Crash after possible I/O but before outcome requires independent
  reconciliation for `CheckBeforeRetry`; `NoAutoRetry` never repeats and must
  surface an indeterminate/manual-reconciliation result.
- Startup rehydrates pending and decided checkpoints before accepting work.
  It stops blanket-failing `AwaitingApproval`/`WaitingEffect` only after this
  recovery path is enabled and tested.

### Disconnect, cancellation, and timeout

- Client disconnect never implies approval. Pending work remains pending until
  its deadline or configured disconnect policy; it never executes by default.
- ACP sends native `session/request_permission` with only `AllowOnce` and
  `RejectOnce`. `Selected` maps to the shared coordinator. Unknown outcomes
  are rejection, never approval.
- ACP `Cancelled`, `session/cancel`, terminal-chat/TUI cancellation, run
  deadline, and shutdown race through the same CAS. The winner is durable;
  late approval conflicts.
- A client without a usable permission surface must not receive/advertise a
  grant-bearing tool path unless another authorized durable approval surface
  is configured.

## 6. Smallest honest vertical slice

Deliver one end-to-end path before expanding commands or remembered policy:

1. One registered counting tool declares one required grant.
2. One loaded policy explicitly escalates that exact action; default deny is
   separately proven not to prompt.
3. A scripted model produces one tool call. The runtime atomically stores its
   checkpoint, effect, approval request, run state, and durable event.
4. Chat and TUI can list and resolve the same approval through
   `InteractionService`; no surface writes the database directly.
5. Allow executes the tool exactly once with a real `ResolvedGrant`, records
   the outcome, resumes the model, and completes the turn.
6. Reject produces a typed tool error, performs zero tool I/O, resumes the
   model, and completes the turn.
7. ACP presents the same request through official
   `session/request_permission`, then follows the same coordinator path.
8. Restart while pending and restart after allow-before-claim both recover and
   complete without duplicate I/O.

A live-process wait around a broadcast channel is not this vertical slice and
must be labeled live-process-only if shipped separately.

## 7. Parallel work packets

```text
APR-00 -> {APR-01 store/coordinator, APR-02 policy/composition,
           APR-04 ACP protocol harness}
{APR-01, APR-02} -> APR-03 orchestrator/recovery
{APR-01, APR-03 event contract} -> APR-05 interaction projection
APR-05 -> APR-06 chat/TUI
{APR-03, APR-04, APR-05} -> APR-07 ACP integration
{APR-03, APR-05, APR-06, APR-07} -> APR-08 cross-surface E2E
```

| Packet | Exclusive ownership | Depends on | Exit artifact |
|---|---|---|---|
| APR-00 | Domain contract, accepted ADR text, fixtures; no production state mutation | none | Frozen states, port signatures, error taxonomy, serialized fixtures |
| APR-01 | Approval/checkpoint migrations, SQLite coordinator, effect-state cutover, run CAS, store conformance | APR-00 | Reopen/race/conformance suite passes |
| APR-02 | Policy approval effect/gate, config loading, resolver injection, readiness | APR-00 | Default-deny/permit/escalate composition tests pass |
| APR-03 | Orchestrator pause, checkpoint reducer, grant injection, recovery, retry reconciliation, startup ordering | APR-01, APR-02 | Counting-tool allow/reject/restart tests pass |
| APR-04 | Fake ACP backend and official-SDK permission request/response/cancel harness only | APR-00 | Protocol tests pass without production backend |
| APR-05 | Durable interaction projection, pending query, real approve/deny service, HTTP adapter | APR-01, APR-03 event contract | Replay and wrong-scope tests pass |
| APR-06 | Chat and TUI approval queue/detail/actions; remove direct DB approval writes | APR-05 | Scripted chat and TUI reducer/render tests pass |
| APR-07 | Production ACP backend update and coordinator binding | APR-03, APR-04, APR-05 | Official client allow/reject/cancel/reconnect tests pass |
| APR-08 | Cross-surface fixture, crash matrix, security and observability closure | APR-03, APR-05, APR-06, APR-07 | User-path E2E and required workspace gates pass |

APR-00 and the SQLite scope of APR-01 completed on 2026-08-06. The
store-neutral serialized approval and checkpoint contracts include exact
lineage, authorized-principal scope, `u64` effect sequencing, typed errors,
one-shot decisions, and fail-closed run state-and-revision CAS. SQLite V18
supplies deterministic legacy effect-state backfill, approval and checkpoint
tables, state/lease/immutability guards, scoped queries, and one-transaction
pause/resolve/claim/checkpoint operations. Stable-ID pause retries return the
same request only when their immutable subject, checkpoint, and metadata are
identical. Human and quorum principals may allow or reject only when they are
the frozen authorized principal; service principals may expire or cancel
within the exact tenant/workspace/conversation scope, but cannot allow or
reject. Approval-linked effects are isolated from generic claim, release,
expiry, and state-update paths. Exact claims pass through a durable `Claimed`
to `Executing` attempt boundary before outcome recording, while an expired
pre-I/O claim can be re-leased only with zero attempts and an eligible
checkpoint. Canonical run events and the legacy dead-letter state remain
supported, with conformance, race, close/reopen, wrong-scope, recovery, and
migration coverage.

The subject and checkpoint integrity digests are caller-supplied and currently
only cross-checked against duplicated persisted columns. Canonical construction
and recomputation at trust boundaries remain APR-03 work; the SQLite foundation
alone does not establish cryptographic checkpoint integrity.

APR-02 completed on 2026-08-06. Policy loading is explicitly enabled and
otherwise composes an empty default-deny resolver. Enabled loading selects one
safe named TOML file, rejects unknown fields, duplicates, malformed rules,
relative directory escape, policy-name traversal, and unsupported `~user`
forms, and fails runtime startup on missing or invalid input. Matching deny
beats approval and allow; approval beats allow. The exact resolver `Arc` is
retained by `AppService` and injected into its orchestrator, with truthful
ready/disabled startup state. This does not make the approval path operational:
grant-bearing tools remain withheld until APR-03 composes the durable
coordinator with execution and continuation.

APR-04 completed on 2026-08-06 at the protocol-only boundary. The ACP surface
constructs `session/request_permission` from the safe tool projection with the
exact tool-call identity, exactly `AllowOnce` and `RejectOnce`, and no raw input
or output. Its response decoder maps those two selections and `Cancelled`, and
fails closed for unadvertised options. The official-SDK duplex harness covers
allow, reject, a pending request resolved after a matching `session/cancel`, an
unknown option, a client protocol error, and disconnect while pending. This
does not connect ACP to a production backend, persist a decision, or authorize
an effect; those remain APR-07 integration and APR-08 end-to-end work.

Hot files have one integration owner at a time: SQLite migration registration,
`store-trait/src/lib.rs`, `service/src/app.rs`, `run/src/orchestrator.rs`,
`runtime/src/factory.rs`, `runtime/src/interaction.rs`, CLI TUI application
state, and `surface-acp/src/lib.rs`.

## 8. Acceptance tests

Agents add focused targets with these stable names or equivalent documented
names, then APR-08 runs them together:

- `cargo test -p polkagent-store-sqlite approval_coordinator`
  - concurrent allow/reject/expire has one winner;
  - identical retry is idempotent and opposite retry conflicts;
  - wrong conversation/digest/principal fails;
  - unapproved effect cannot be claimed; approved effect claims once;
  - close/reopen preserves request, decision, and outbox event.
- `cargo test -p polkagent-grant approval_policy`
  - default and explicit deny never prompt;
  - escalation is explicit; permit and resolved-grant scope are exact.
- `cargo test -p polkagent-run approval_pause_resume`
  - I/O counter is zero while pending and after reject/cancel/timeout;
  - allow executes exactly once and passes the resolved grant;
  - changed arguments or tool fingerprint require a new approval.
- `cargo test -p polkagent-runtime approval_recovery`
  - recover after pause commit and after approval-before-claim;
  - recover claim-before-I/O;
  - reconcile possible-I/O-before-outcome for every retry class;
  - interaction event IDs and ordering remain stable across replay.
- `cargo test -p polkagent-cli --test chat_e2e approval`
  - prompt, list, allow/reject, tool result, follow-up, and cancellation use the
    shared service.
- `cargo test -p polkagent-surface-acp permission`
  - official SDK client sees exact tool-call identity and only once options;
  - allow/reject map correctly; `session/cancel` resolves pending requests as
    cancelled; disconnect never executes the effect.
- `cargo test -p polkagent-integration-tests --test approval_pause_resume`
  - one durable conversation is observed and controlled across HTTP, chat,
    TUI, and ACP before and after restart, without duplicate I/O.

Final closure also runs the repository's required format, check, test, Clippy,
and documentation gates.

## 9. Migration and compatibility requirements

- Add the next forward-only migration (V18 at preparation time; use the next
  available number at implementation time) for `approval_requests`, execution
  checkpoints, unique/indexed lineage, leases, versions, deadlines, and
  decision audit fields.
- Before making `effect_intents.state` authoritative, backfill it from legacy
  `claimed_by`, outcomes, and leases. Clear sentinel values from `claimed_by`;
  thereafter it contains only a worker ID or `NULL`.
- Validate legal effect states/transitions in the coordinator and database
  where practical. Never reinterpret an unknown state as pending or approved.
- Existing resolved outcomes and terminal runs remain immutable. Old rows with
  insufficient lineage are visible for audit but cannot be approved.
- Route HTTP approve/deny through the coordinator while preserving versioned
  response compatibility where truthful. Remove fabricated approval IDs and
  bus-only success.
- SQLite is the first required adapter. Postgres must pass the same conformance
  suite before its readiness can advertise approval support; otherwise it
  fails closed for this capability.
- Rollback is code rollback plus restored pre-migration backup; migrations are
  not reversed in place. Newer unsupported checkpoint versions fail startup
  readiness without executing work.

## 10. Security requirements

- Persist the authenticated principal, approval type, originating surface,
  tenant/workspace scope, reason, conditions, policy snapshot digest, and
  timestamps. Client-supplied display metadata is not authority.
- Authorize list/get/resolve by conversation and tenant scope. Possession of an
  approval UUID alone is insufficient.
- Treat checkpoint messages and tool arguments as prompt-sensitive data:
  encrypt according to storage policy, apply bounded retention, and never log
  raw arguments, secrets, model messages, grants, or tool output.
- Reapply filesystem, shell, chain, custody, amount, budget, and data-
  classification controls immediately before dispatch. Approval cannot widen
  the stored security scope.
- Bound title/description/reason/condition sizes and render them as untrusted
  text in terminal, HTTP, and ACP clients.
- Record immutable audit evidence for request, decision, expiry/cancel,
  execution claim, reconciliation, and terminal outcome.

## 11. Observability and operations

Expose low-cardinality metrics for:

- pending approvals, oldest pending age, requests, decisions, conflicts,
  expirations, cancellations, and decision latency;
- approved-but-unclaimed count/age, checkpoint lease age, recovery attempts,
  resume latency, reconciliation results, and indeterminate effects;
- tool I/O starts after approval, default-deny refusals, and surface/transport
  failures without tool names, arguments, approval IDs, or principals as
  metric labels.

Structured traces correlate run, turn, effect, approval, and checkpoint using
safe opaque IDs. Readiness reports whether configured policy, approval store,
checkpoint schema, recovery worker, and surface permission support are
operational. Alert on overdue pending approvals, approved effects not consumed,
expired checkpoint leases, repeated CAS conflicts, and indeterminate effects.

Operator documentation must cover listing pending work, safely rejecting or
cancelling it, restart behavior, reconciliation, backup/restore, retention,
and why approval never overrides policy denial.

## 12. Implementation checklist

### Contract freeze

- [x] Accept APR-ADR-01 through APR-ADR-05 as the implementation decisions in
  this design.
- [ ] Freeze serialized approval, checkpoint, state, error, and outbox fixtures.
  Approval/checkpoint envelopes and duration encoding are frozen; dedicated
  state, error, and outbox fixture snapshots remain.
- [ ] Assign one owner for each hot file and packet.

### Durable foundation

- [x] Add approval and checkpoint domain records without surface types.
- [x] Add coordinator/checkpoint ports and expected-state run CAS.
- [x] Add and test the forward migration and legacy effect-state backfill.
- [x] Implement SQLite transactions, indexes, pagination, leases, and
  idempotent conflict semantics.
- [ ] Add shared store conformance; make unsupported adapters fail closed.
  Shared SQLite conformance exists and the run-CAS default fails closed;
  adapter capability/readiness declarations still need explicit coverage.

### Policy and execution

- [x] Make approval escalation explicitly configurable and serializable.
- [x] Inject the configured resolver through `AppServiceBuilder` and runtime.
- [ ] Advertise grant-bearing tools only when the complete approval path is
  ready and policy permits exposure. APR-02 preserves the existing fail-closed
  withholding; durable-path readiness now depends on APR-03.
- [ ] Persist checkpoint/effect/approval before publishing or waiting.
  The coordinator transaction guarantees this ordering, but the orchestrator
  does not invoke it yet.
- [ ] Wake from store-backed state, claim approved effects, pass exact grants,
  and revalidate security before I/O.
- [ ] Reduce denial as a typed tool result; implement cancel/timeout races.
- [ ] Implement checkpoint leasing, startup rehydration, and retry-class
  reconciliation before changing the startup reaper.
  Store-level leasing/version CAS and expired pre-I/O claim recovery are
  complete; orchestrator startup rehydration, post-I/O reconciliation, and
  canonical digest construction/verification remain APR-03 work.

### Shared interaction and surfaces

- [ ] Project durable approval request/resolution with stable identities.
- [ ] Implement scoped pending lookup and real `InteractionService::approve`
  and `deny`.
- [ ] Route HTTP approve/deny through the same coordinator.
- [ ] Enable chat `/approve` and `/deny` plus pending status/detail.
- [ ] Enable TUI queue/detail/key actions and remove direct approval SQL.
- [x] Freeze and test the ACP permission protocol projection with once-only
  options, exact tool identity, withheld payloads, cancellation, disconnect,
  and unknown-option/error handling in the official-SDK harness.
- [ ] Bind ACP permission requests and responses to the production durable
  coordinator and effect continuation; do not infer approval on disconnect or
  protocol failure.

### Closure

- [ ] Pass all focused store/policy/run/runtime/chat/TUI/ACP tests.
- [ ] Pass the cross-surface restart and crash-point matrix.
- [ ] Complete authorization, redaction, retention, backup, readiness,
  metrics, traces, alerts, and operator documentation.
- [ ] Run required workspace gates and attach exact command evidence.
- [ ] Update canonical status/backlog only after end-to-end evidence exists.
