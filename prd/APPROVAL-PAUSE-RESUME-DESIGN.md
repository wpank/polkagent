# Approval pause-and-resume design

**Status:** active integration design; APR-00/APR-01 SQLite foundation, APR-02
policy composition, the bounded APR-03 executor/store continuation, the
APR-04 ACP protocol harness, and the bounded APR-05 interaction/service/HTTP
contract are implemented. APR-06 terminal chat and the TUI F6 adapter are
implemented over the shared service boundary. APR-07 implements one explicit
process-bound local-stdio ACP authority and durable native permission path.
Integrated commit `399db83` extends that explicit local-process authority
tuple to terminal chat and the explicit TUI command. APR-08 commit `20dbcb1`
proves one bounded cross-surface happy-path/restart seam. Shared/remote
multi-principal authentication, manual Zed validation, the broader crash
matrix, and operational closure remain incomplete.

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

This document claims the bounded explicit local-process authority slice and
one cross-surface happy-path/restart fixture, not shared/remote production
authority or full end-to-end completion.

## 1. Status quo and evidence

| Area | Present | Blocking gap |
|---|---|---|
| Interaction contract | `ApprovalView`, stable `ApprovalRequested`/`ApprovalResolved` replay, a bounded pending query, and real `approve`/`deny` results exist in `polkagent-interaction` and the durable runtime adapter. APR-07 explicitly binds one authority to ACP; terminal chat and TUI consume the same boundary when explicitly composed. | Authority remains unbound by default. Shared/remote production authentication and pagination beyond the hard 100-row bound remain open. |
| Tool execution | Grantless tools retain the generic durable pipeline. With an explicit authority, `RuntimeFactory` composes the APR-03 coordinator/checkpoint path for tri-state grant resolution, atomic pause, one-shot exact claim, immediate revalidation, one handler call, durable outcome, and typed reject continuation. | It remains disabled without authority. The bounded slice accepts exactly one approval-gated call in a model tool group and one approval checkpoint per run. |
| Policy | Default-deny evaluation, explicit deny/permit/`RequireApproval` effects, strict named-file loading, gate escalation, exact resolver injection, stable policy snapshots, and immediate pre-I/O revalidation are implemented | Activation remains fail-closed by default; broader grant kinds and authenticated shared/remote principal authorization beyond explicit local-process chat/TUI/ACP remain open |
| Effect persistence | SQLite V18 adds durable approval/checkpoint records, makes `effect_intents.state` authoritative, clears legacy `claimed_by` sentinels, enforces approval/lease invariants, and supplies the APR-03 atomic reducer used by the injected orchestrator path | Non-SQLite adapters do not advertise this capability; retention, operator reconciliation, and the broader crash matrix remain open |
| Application service | `AppService` retains the injected coordinator and exposes scoped list/get/resolve operations. Process-local approval broadcasts are removed; effect-ID-only compatibility methods fail explicitly because they lack authority scope. Decided checkpoints can recover after agents rehydrate. | APR-07 binds a local-process tuple; shared services still need authenticated tenant/workspace/principal derivation before enabling the resolver or grant-bearing executor. |
| API | Scoped `GET /interactions/{id}/approvals` and `POST .../{approval_id}/approve|deny` route through the shared service. Auth, explicit principal-bound service composition, read-only refusal, OpenAPI parity, idempotent retry, and wrong-conversation denial are tested. Legacy `/effects/{id}/approve|deny` are deprecated 501 migration stubs. | `app_state_from_runtime` intentionally does not install the authenticated approval service, so the production server returns unavailable. API keys currently authenticate a deployment credential but do not supply a stable per-key principal mapping. |
| Run lifecycle | The APR-03 path consumes `AwaitingApproval` and `WaitingEffect`, uses exact-state run CAS, and commits run/effect/approval/checkpoint/event transitions through the SQLite coordinator. APR-05 decisions trigger recovery. Explicit local-process chat, TUI, and ACP authority compose this path through the same `RuntimeFactory`. | Default/no-subcommand TUI, surfaces without the all-or-none authority tuple, and the production API remain authority-unbound. Shared/remote authentication is not supplied by the local tuple. |
| Recovery | Resumable decided checkpoints lease by exact version/worker and reduce before another model request. An approved `Executing`/`Resolved` effect without one durable outcome returns typed manual reconciliation and is never rerun. Startup calls approval recovery before the generic reaper, which no longer destroys approval-owned states. APR-05 wakes recovery after a durable decision and replays deterministic approval events after restart. APR-08 proves exact identity and one immutable outcome across one cross-surface restart fixture, restart from `Approved`/`Resumable` before any effect/checkpoint lease, safe reclaim of one expired pre-I/O claim, and the adjacent real-file `Executing`/no-outcome boundary through two fail-closed service recoveries. | Operator resolution remains unimplemented, and the other pause/decision/claim/pre-I/O/post-I/O crash points remain APR-08 work. |
| ACP | The bounded local-stdio path issues native permission requests and binds allow/reject/cancel to the durable coordinator with exact identities, redaction, one-I/O allow, zero-I/O reject/cancel, and restart/load recovery | Manual Zed validation and shared/remote multi-principal authentication remain open; authority is explicit and unbound by default |
| Terminal chat | `/status` projects at most 100 exact pending approval IDs; `/approve` and `/deny` route only through `InteractionService`, remain usable during an active turn, bound denial reasons to 4,096 Unicode characters, and preserve coordinator scope/idempotency/conflict behavior across restart. Explicit `polkagent chat` accepts the all-or-none authority tuple and fixes the surface to `terminal-chat`; omission stays fail-closed. Stable event output excludes untrusted approval metadata and denial text. | The tuple is a process-owner assertion, not shared/remote authentication. Cancellation of a turn pending approval now coordinator-cancels the approval/run with zero tool I/O; cancellation during handler I/O remains open. |
| TUI F6 | The selected durable Console conversation scopes an asynchronous pending queue and exact approve/deny CAS through `InteractionService`. Explicit `polkagent tui` accepts the all-or-none local authority tuple and fixes the surface to `tui`. The view shows bounded/redacted service title, description, policy reason, expiry, and full confirmation identities. Legacy direct approval queries/writes are removed. | The no-subcommand TUI and explicit TUI without the tuple remain authority-unbound. The first 100 rows are shown with an explicit total/reveal-remainder message; pagination and shared/remote authentication remain open. Cancel/shutdown coordinator-cancels a pending approval/run with zero tool I/O; cancellation during handler I/O remains open. |

The durable center and its bounded interaction/HTTP/ACP adapters now exist.
The same explicit local-process authority tuple is available to chat, TUI, and
ACP. The remaining production blocker is authenticated
tenant/workspace/principal derivation for shared/remote surfaces, plus the
broader APR-08 crash/operations matrix and manual-editor closure.

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

APR-05 intentionally exposes one bounded first page: at most 100 pending
approvals in stable store order, offset zero, with no cursor or pagination in
the `InteractionService`/HTTP contract. The OpenAPI response records
`maxItems: 100`. This is truthful for the current single-active-turn execution
limit, but pagination must be added before relaxing that invariant or claiming
an unbounded operator queue.

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
| APR-05 — bounded complete | Durable interaction projection, pending query, real approve/deny service, HTTP adapter | APR-01, APR-03 event contract | Replay, restart, retry/conflict, auth/read-only, OpenAPI parity, and wrong-scope tests pass; production authority remains unbound |
| APR-06 — bounded complete | Terminal chat and TUI F6 list/resolve exact scoped approvals through `InteractionService`; TUI direct approval SQL is removed. | APR-05 | Chat plus TUI scope/retry/conflict/restart/concurrency/cancel/fail-closed, reducer, redaction, and render tests pass. Later `399db83` adds explicit local authority; defaults stay unbound. |
| APR-07 — bounded complete | Local-stdio ACP backend update and explicit process-authority coordinator binding | APR-03, APR-04, APR-05 | Official client allow/reject/cancel and SIGKILL/restart/load tests pass; manual Zed remains open |
| APR-08 — bounded fixture complete; packet active | Cross-surface fixture, crash matrix, security and observability closure | APR-03, APR-05, APR-06, APR-07 | One user-path happy-path/restart fixture passes; crash, editor, auth, security, observability, and workspace-gate closure remain |

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

APR-03 now constructs and recomputes domain-separated BLAKE3 subject and
checkpoint integrity digests at the executor/store trust boundaries. The
SQLite foundation alone still must not be treated as proof for another adapter;
each adapter must opt into the exact recovery capability and pass conformance.

APR-02 completed on 2026-08-06. Policy loading is explicitly enabled and
otherwise composes an empty default-deny resolver. Enabled loading selects one
safe named TOML file, rejects unknown fields, duplicates, malformed rules,
relative directory escape, policy-name traversal, and unsupported `~user`
forms, and fails runtime startup on missing or invalid input. Matching deny
beats approval and allow; approval beats allow. The exact resolver `Arc` is
retained by `AppService` and injected into its orchestrator, with truthful
ready/disabled startup state. APR-03 composes the bounded durable executor when
the coordinator and checkpoint ports are explicitly injected. APR-05 supplies
the real scoped decision operation, but production grant-bearing tools remain
withheld until `RuntimeFactory` can bind it to authenticated stable authority.

APR-04 completed on 2026-08-06 at the protocol-only boundary. The ACP surface
constructs `session/request_permission` from the safe tool projection with the
exact tool-call identity, exactly `AllowOnce` and `RejectOnce`, and no raw input
or output. Its response decoder maps those two selections and `Cancelled`, and
fails closed for unadvertised options. The official-SDK duplex harness covers
allow, reject, a pending request resolved after a matching `session/cancel`, an
unknown option, a client protocol error, and disconnect while pending. This
does not itself connect ACP to a production backend, persist a decision, or
authorize an effect; APR-07 now supplies that bounded integration, while APR-08
retains the cross-surface end-to-end work.

APR-03 delivered its bounded executor/store continuation on 2026-08-06.
Canonical domain-separated BLAKE3 digests now bind approval subjects,
checkpoints, tool specs, and policy snapshots. Non-capable trait adapters
declare themselves false and cannot make executor readiness truthful. The
orchestrator evaluates default/explicit deny as a typed no-effect/no-I/O tool
error; resource-specific denial is intentionally checked after the model call
because the exact tool arguments do not exist at advertisement time. An
explicit `RequireApproval` commits effect, request, checkpoint, event, and run
pause atomically, then treats polling only as a wake-up and reloads the
decision from the store. `AllowOnce` leases the exact checkpoint/effect,
records the attempt boundary, revalidates unchanged policy/tool/security
scope, performs at most one handler call, persists one outcome, and atomically
resumes the run with a stable `EffectsResolved` event. `RejectOnce` follows
the same reducer with zero attempts and zero outcomes. Expiry and cancellation
are terminal coordinator decisions.

The service can recover decided checkpoints after persisted agents are
registered and before generic abandoned-run recovery. The generic reaper now
excludes `AwaitingApproval` and `WaitingEffect`. A durable attempt without a
durable outcome is a manual-reconciliation error and is never retried. The
production runtime reports approval storage, executor, and surface readiness
separately. It keeps them disabled by default and enables them only when an
explicit stable authority is bound; `399db83` does so for explicit local chat,
TUI, and ACP commands. The no-subcommand TUI, the three commands without the
tuple, and the API still do not advertise grant-bearing tools. A real SQLite
counting-tool fixture now proves AllowOnce executes exactly once, receives one
exact grant, feeds the result back to the model, and completes. Remaining
APR-03 closure evidence is the equivalent reject/default-deny and restart
crash matrix; the SQLite coordinator tests already prove the atomic approved/
rejected reducer and idempotent stable-event boundary.

APR-05 delivered its bounded projection/service/HTTP slice on 2026-08-06.
`InteractionApprovalAuthority` freezes tenant, workspace, principal, and
surface. `AppService` retains and queries the same `ApprovalCoordinatorStore`
used by APR-03; list/get/resolve validate exact authority and conversation
scope, identical retries return the persisted decision, and conflicting or
late decisions fail closed. The runtime projects real approval/effect/run IDs
into deterministic requested/resolved interaction event IDs, backfills them
from SQLite after restart, and wakes the durable checkpoint recovery path after
resolution. The HTTP adapter requires authentication to be enabled plus an
explicitly composed principal-bound approval service, honors read-only mode,
and routes through `InteractionService`; it performs no database writes.
Legacy effect-only mutations now always return 501 because possession of an
effect UUID cannot establish approval authority.

APR-05 remains a contract and test-composition claim outside explicitly bound
surfaces. `app_state_from_runtime` does not populate
`authenticated_approval_service`, and default runtime readiness reports the
approval surface unavailable and executor disabled. APR-06's terminal-chat and
TUI adapters are complete at their bounded service seams. Commit `399db83`
binds explicit local-process authority for chat, TUI, and ACP; default and API
composition stay fail-closed. APR-08 owns shared/remote production authority
plus full crash/security/observability closure.

APR-07 delivered its bounded local-stdio binding on 2026-08-06. The CLI accepts
`--approval-tenant`, `--approval-workspace`, and a non-nil
`--approval-principal` only as an all-or-none tuple, fixes the surface to
`acp-stdio`, and emits no protocol stdout on invalid startup. `RuntimeFactory`
binds that authority to the interaction service and the APR-03 executor using
one SQLite pool, derives a stable versioned service principal, recovers approval
checkpoints before generic runs, and reports readiness from the actual bound
capability. Native ACP permission requests contain bounded/redacted metadata
and exactly allow-once/reject-once; cancellation, disconnect, protocol error,
and timeout use the coordinator cancel path and never grant. Pending recovery
is hard-bounded to 100 and fails closed on overflow. Official-client tests prove
allow executes one attempt/outcome, reject and cancel execute none, racing
decisions have one winner, and SIGKILL followed by `session/load` re-presents
one pending request and completes one effect. Manual Zed validation remains an
APR-08/editor evidence item.

APR-08 delivered one bounded cross-surface happy-path/restart fixture in
`20dbcb1`, built on `399db83`; `4b4f11b` fixes the integrated
`RuntimeFactory` future-size boundary. The real file-backed SQLite
[`approval_cross_surface_e2e.rs`](../crates/polkagent-cli/tests/approval_cross_surface_e2e.rs)
fixture creates its approval through a strict policy and registered governance
tool rather than seeding rows. TUI `RunController` lists and approves the exact
request, while terminal chat's shared `ServiceCommandExecutor` proves
wrong-conversation refusal, restart idempotency, and opposite-decision
conflict. A restarted TUI preserves conversation/approval/effect/run/tool-call
IDs; a wrong principal is refused. One handler attempt, one immutable outcome,
a completed run, zero pending approvals, and zero lineage orphans are verified
directly in SQLite. Public projection bounds and sentinel redaction are also
proved. This does not close the broader crash-boundary matrix, possible-I/O
operator reconciliation, manual Zed, shared/remote authentication,
observability/security, or cancellation during handler I/O.

The focused real-file SQLite
[`approval_possible_io_recovery.rs`](../crates/polkagent-service/tests/approval_possible_io_recovery.rs)
fixture now closes three adjacent boundaries around effect claim and attempt
start. First, the production coordinator pauses and durably approves, then all
first-process handles stop while the effect is still `Approved`, the
checkpoint is `Resumable`, both effect/checkpoint lease pairs are null, and no
attempt or outcome exists. A newly composed `AppService` leases and claims the
exact continuation, invokes the handler once, persists one linked consumed
success outcome, and completes/terminalizes the exact lineage; a second
restart recovers zero work and leaves the full queried lineage unchanged.

For the next boundary, the original process stops after claim with zero
attempts. Once the old effect/checkpoint leases expire, a restarted service
reclaims under a new worker and produces the same exact-once terminal result;
another restart is inert and preserves lineage. If an attempt start was
durable but no outcome exists, two expired-lease recoveries instead return the
same typed manual-reconciliation run/effect identity, invoke the handler zero
times, create no outcome, and preserve exact lineage. This proves only these
adjacent approval-before-claim, pre-I/O, and possible-I/O boundaries. It does
not provide an operator reconciliation workflow, prove the `Resolved`
corruption case, or close the remaining crash-point matrix.

The APR-00 contract-freeze remainder is now executable. Static
[`approval_state_v1.json`](../crates/polkagent-store-trait/tests/fixtures/approval_state_v1.json)
and
[`approval_error_v1.json`](../crates/polkagent-store-trait/tests/fixtures/approval_error_v1.json)
snapshots pin the serialized approval/checkpoint lifecycle values, retry
classes, and every current `ApprovalStoreError` representation. No-wildcard
matches make additions to those bounded enums a compile-time test update, and
unknown serialized tags fail closed. Static
[`approval_outbox_events_v1.json`](../crates/polkagent-core/tests/fixtures/approval_outbox_events_v1.json)
pins the exact `EventKind` payload set emitted by the approval coordinator; it
does not claim to enumerate unrelated `EventKind` variants.

`ServiceError::ManualReconciliation` remains an internal typed process error,
not a persistence or wire type. Its actual serialized surface boundary is the
runtime's `InteractionError` projection, now pinned to non-retryable
`unavailable`, the generic durable-approval failure message, no details, and
no run ID, effect ID, or reconciliation-reason leakage. The
`fixture_schema_version` fields in these snapshots version only the test
corpus; they are not new production envelopes or production schema versions.
No production representation or migration changed for this freeze.

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
- `cargo test -p polkagent-store-trait --test approval_contract_freeze`
  - exact state and store-error snapshots round-trip;
  - compiler-checked variant matches require fixture review for a new bounded
    enum variant;
  - unknown tags and unsupported test-fixture versions fail closed.
- `cargo test -p polkagent-core --test approval_outbox_contract`
  - the exact coordinator-emitted approval event payload set round-trips;
  - unknown event tags and unsupported test-fixture versions fail closed.
- `cargo test -p polkagent-service manual_reconciliation_preserves_typed_run_error_identity`
  - the internal run-to-service conversion preserves exact typed identity and
    stable diagnostic text.
- `cargo test -p polkagent-runtime manual_reconciliation_projects_as_bounded_surface_error`
  - the serialized interaction projection is exact, round-trips, and excludes
    internal reconciliation identifiers and reason text.
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
- `cargo test -p polkagent-runtime approval_projection_scope_retry_conflict_and_restart_are_durable`
  - pending lookup returns exact scoped durable identity;
  - wrong conversation/principal fail closed;
  - identical retry is idempotent and an opposite decision conflicts;
  - requested/resolved projection IDs remain stable and unique after reopen.
- `cargo test -p polkagent-api --test approval_http --test openapi_interactions --test openapi_route_parity`
  - missing/invalid authentication fails, explicit composition succeeds, and
    the ordinary runtime remains unavailable;
  - read-only mode blocks decisions and denial length counts the OpenAPI-defined
    4,096 Unicode characters;
  - source, embedded, served OpenAPI, and router operations remain in parity.
- `cargo test -p polkagent-cli --test chat_e2e approval`
  - ordinary production composition hides approval commands and explicit
    `/approve` fails with no turn, run, or approval mutation.
- `cargo test -p polkagent-cli commands::chat::tests::approval`
  - the focused real-SQLite chat fixture lists exact scoped IDs and routes
    allow/reject only through the principal-bound durable interaction service;
  - wrong conversation, identical retry after service restart, opposite-
    decision conflict, coordinator-backed pending cancellation, no-pending
    cancellation, Unicode denial bounds, and redaction-safe event rendering
    pass.
- `cargo test -p polkagent-cli --lib approval`
  - TUI controller list/decision preserves exact conversation and approval
    identity across wrong scope, identical retry, opposite-decision conflict,
    controller restart, and an unrelated active Console turn;
  - authority-unbound composition reports unavailable, stale correlations are
    rejected, display metadata is bounded/redacted, and cancel/shutdown
    durably cancels a pending approval/run without tool I/O.
- `cargo test -p polkagent-cli --test tui_tests approvals`
  - empty/loaded/unavailable queue states, exact detail fields, full-ID
    confirmations, and key actions render without fabricated operation/risk
    metadata.
- `cargo test -p polkagent-surface-acp permission`
  - official SDK client sees exact tool-call identity and only once options;
  - allow/reject map correctly; `session/cancel` resolves pending requests as
    cancelled; disconnect never executes the effect.
- `cargo test -p polkagent-cli --test approval_cross_surface_e2e`
  - one real policy-required tool path is observed and controlled through TUI
    plus terminal chat's shared command executor before and after restart;
  - exact durable identities, wrong scope/principal refusal, retry/conflict,
    one attempt/outcome, zero pending/orphans, projection bounds, and sentinel
    redaction pass without seeded approval rows.
- `cargo test -p polkagent-service --test approval_possible_io_recovery`
  - one production SQLite seam stops after durable approval with an
    `Approved` effect, `Resumable` checkpoint, null effect/checkpoint leases,
    and zero attempts/outcomes; a fresh `AppService` recovers it into one
    handler call, attempt, consumed success outcome, and terminal reduction;
  - one production SQLite seam restarts from an expired `Claimed` effect with
    no attempt, reclaims under a new worker, executes once, and persists one
    exactly linked consumed success outcome plus completed/terminal lineage;
  - a second restart after either successful recovery recovers zero work and
    leaves handler, attempt, outcome, approval, effect, run, turn, step, and
    checkpoint identity unchanged;
  - the paired case restarts twice from `Executing` with one attempt and no
    outcome;
  - typed manual reconciliation remains stable, the registered handler stays
    at zero invocations, no outcome is synthesized, and exact lineage is
    unchanged after both recovery attempts.
- Full APR-08 closure still requires the HTTP/ACP-inclusive user path and the
  remaining pause/decision/claim/pre-I/O/post-I/O crash points without
  duplicate I/O, plus an operator reconciliation workflow.

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
- [x] Freeze serialized approval, checkpoint, state, error, and the exact
  approval-coordinator outbox payload set. Approval/checkpoint envelopes and
  duration encoding were already frozen; dedicated state/error snapshots use
  compiler-checked no-wildcard variant guards, and the outbox snapshot is
  deliberately limited to coordinator-emitted events. Test-fixture wrapper
  versions are test metadata, not production envelopes. Internal
  `ServiceError::ManualReconciliation` is not serialized; its real
  `InteractionError` surface projection is frozen and redaction-tested.
- [ ] Assign one owner for each hot file and packet.

### Durable foundation

- [x] Add approval and checkpoint domain records without surface types.
- [x] Add coordinator/checkpoint ports and expected-state run CAS.
- [x] Add and test the forward migration and legacy effect-state backfill.
- [x] Implement SQLite transactions, indexes, pagination, leases, and
  idempotent conflict semantics.
- [x] Add shared store conformance; make unsupported adapters fail closed.
  Shared SQLite conformance exists; default approval-resume and exact-
  checkpoint capability probes are false, while SQLite opts in explicitly.

### Policy and execution

- [x] Make approval escalation explicitly configurable and serializable.
- [x] Inject the configured resolver through `AppServiceBuilder` and runtime.
- [x] Gate grant-bearing advertisement on complete executor/store capability.
  Exact policy/resource denial remains an execution-time typed error because
  the call arguments are unknown at advertisement time. Production exposure
  remains disabled until a stable authenticated authority is bound to the
  implemented APR-05 resolver surface.
- [x] Persist checkpoint/effect/approval before publishing or waiting.
- [x] Wake from store-backed state, claim approved effects, pass an exact
  one-shot grant, and revalidate policy, tool fingerprint, arguments,
  workspace, and security scope immediately before I/O.
- [x] Reduce `RejectOnce` as a typed tool result with no attempts/outcomes;
  coordinator expiry/cancel decisions terminalize without I/O.
- [x] Implement exact checkpoint leasing, decided-checkpoint startup ordering,
  canonical digest construction/verification, pre-I/O reclaim, and explicit
  manual reconciliation for possible-I/O-without-outcome. APR-05 wakes decided
  recovery; operator reconciliation workflows remain APR-08 work.

### Shared interaction and surfaces

- [x] Project durable approval request/resolution with stable identities.
- [x] Implement scoped pending lookup and real `InteractionService::approve`
  and `deny`.
- [x] Route HTTP approve/deny through the same coordinator. The router contract
  is test-composable, but production authority composition remains fail-closed.
- [x] Enable chat `/approve` and `/deny` plus pending identity status through
  the shared service/registry. Explicit `polkagent chat` accepts local-process
  authority; omission stays unavailable and advertises neither command.
- [x] Enable a conversation-scoped asynchronous TUI queue/detail/key-action
  surface through `InteractionService`; remove direct approval SQL. Explicit
  `polkagent tui` accepts local-process authority, while the default TUI remains
  unavailable; truncation and stale-result guidance remain truthful.
- [x] Freeze and test the ACP permission protocol projection with once-only
  options, exact tool identity, withheld payloads, cancellation, disconnect,
  and unknown-option/error handling in the official-SDK harness.
- [x] Bind ACP permission requests and responses to the production durable
  coordinator and effect continuation under an explicit local-stdio authority;
  disconnect and protocol failure cancel durably and never grant.

### Closure

- [ ] Pass all focused store/policy/run/runtime/chat/TUI/ACP tests.
- [x] Pass the bounded APR-08 TUI/chat happy-path and restart fixture with one
  effect attempt/outcome, stable identities, zero pending/orphans, and bounded
  redacted projection (`20dbcb1`).
- [x] Prove the APR-08 real-file possible-I/O boundary for one approved
  `Executing` effect with one attempt and no outcome: two service recoveries
  return typed manual reconciliation, perform zero handler calls, synthesize
  no outcome, and preserve exact durable lineage.
- [x] Prove the real-file approval-before-claim boundary: stop with an
  `Approved` effect, `Resumable` checkpoint, null effect/checkpoint leases, and
  zero attempts/outcomes; a restarted service executes and reduces exactly
  once, and another restart performs no work and preserves exact lineage.
- [x] Prove the paired real-file pre-I/O boundary for one approved `Claimed`
  effect with no attempt: after both leases expire, a restarted service
  reclaims under a new worker and produces exactly one handler call, attempt,
  consumed success outcome, and completed/terminal reduction; another restart
  performs no work and preserves the exact lineage.
- [ ] Pass the cross-surface restart and crash-point matrix.
- [ ] Complete authorization, redaction, retention, backup, readiness,
  metrics, traces, alerts, and operator documentation.
- [ ] Run required workspace gates and attach exact command evidence.
- [ ] Update canonical status/backlog only after end-to-end evidence exists.
