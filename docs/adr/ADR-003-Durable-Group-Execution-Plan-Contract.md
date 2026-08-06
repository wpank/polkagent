# ADR-003: Durable Group Execution Plan Contract

**Status:** accepted for the canonical v1 codec; ledger and group-revision design proposed

**Date:** 2026-08-06

**Owners:** group, runtime, interaction, and SQLite maintainers

## Context

ORC-01 needs a durable ledger between a validated group definition and child
runtime launches. A complete plan must exist before the first child can start;
every task must retain its exact agent and child conversation/turn/run identity;
terminal results and cancellation targets must survive restart; and identical
retries must be idempotent while conflicting retries fail closed.

The current in-memory execution types were designed for synchronous tests, not
as a persistence protocol. `ExecutionPlan` derives serde directly over:

- a `HashMap<TaskId, Vec<TaskId>>` dependency graph, whose JSON object ordering
  is not canonical;
- unversioned `TaskId(u64)` values encoded as JSON numbers, which are unsafe for
  consumers that cannot exactly represent integers above 2^53;
- raw JSON task input without a canonical-byte or size contract;
- `GrantSpec.max_budget: f64`, without a finite-value or canonical-decimal
  persistence rule; and
- a graph that accepts duplicate task IDs, duplicate/dangling/self edges, and
  cycles until an executor happens to inspect it.

`TaskResult` is also not a safe durable terminal contract: it carries a Rust
`Duration` and floating-point budget, has no result version, and does not bind
the result to exact child identities.

The diagnostic
[`execution_plan_current_serde.json`](../../crates/polkagent-group/tests/fixtures/execution_plan_current_serde.json)
fixture records the present serde shape. The audit test proves that serde accepts
duplicate dangling edges and emits no version envelope. This fixture is
evidence, not a storage format.

## Decision

The canonical v1 plan contract is accepted and implemented independently of
storage. Do not add group-execution tables or a `GroupExecutionStore` until the
separate group-definition revision prerequisite below is implemented. The
earlier design shape remains in
[`group_execution_plan_v1_proposed.json`](../../crates/polkagent-group/tests/fixtures/group_execution_plan_v1_proposed.json);
the accepted exact bytes and digest are
[`group_execution_plan_v1.canonical.json`](../../crates/polkagent-group/tests/fixtures/group_execution_plan_v1.canonical.json)
and
[`group_execution_plan_v1.blake3`](../../crates/polkagent-group/tests/fixtures/group_execution_plan_v1.blake3).

### Stable identities

The later execution ledger, not the plan codec, must introduce:

- `GroupExecutionId`: globally unique UUID v7 for one prepared plan;
- `TaskId`: scoped to one `GroupExecutionId`, retaining the current full `u64`
  range but encoded as a canonical base-10 string without sign or leading zero;
- exact parent `ConversationId`, `InteractionTurnId`, and `RunId`; and
- exact child `ConversationId`, `InteractionTurnId`, and `RunId` reserved before
  launch and attached to one task by expected-state compare-and-swap.

The parent identities and `GroupId` are immutable once prepared. Child identity
tuples cannot be reused by another task or replaced after attachment.

### Canonical v1 plan

[`execution_plan_codec`](../../crates/polkagent-group/src/execution_plan_codec.rs)
implements a separate durable DTO rather than persisting the derived serde
output of `ExecutionPlan`:

```text
contract = "polkagent.group-execution-plan"
schema_version = 1
mode = sequential | parallel | pipeline | consensus
tasks = declaration-ordered task records with explicit ordinal
dependency_edges = numerically sorted [{ blocked_by, task_id }]
```

The v1 limits are protocol constants, not deployment configuration:

- at most 1,024 tasks;
- at most 16,384 dependency edges;
- at most 262,144 canonical UTF-8 bytes in one task input;
- at most 64 task-input JSON container levels, where a scalar has depth zero;
  and
- at most 1,048,576 canonical UTF-8 bytes in the complete plan.

The encoder determines canonical lengths and rejects over-limit input before
allocating canonical output. The decoder rejects an over-limit complete byte
slice before JSON parsing. Normalization and structural validation happen
before storage:

1. Require at least one task and unique task IDs and ordinals.
2. At the later prepare-service boundary, require every task agent to be a
   current member of the exact group loaded through `GroupService`; the
   owner/leader invariants therefore remain shared. This membership check is
   deliberately not part of the group-agnostic plan codec and must bind the
   group revision described below.
3. Require every dependency endpoint to exist; reject duplicate and self edges.
4. Prove the graph is acyclic. Preserve declaration order independently of edge
   order.
5. Apply mode constraints, including at least two tasks for consensus.
6. Recursively sort JSON object keys by their raw UTF-8 byte sequence, retain
   array order, reject duplicate keys at decode, and emit UTF-8 with no BOM,
   insignificant whitespace, or trailing newline. Strings emit Unicode as raw
   UTF-8 and escape only quotation mark, reverse solidus, and required control
   characters using lowercase `\u00xx` where a short JSON escape is unavailable.
   Task input numbers are accepted only when exactly representable as `i64` or
   `u64`, then emitted as the shortest base-10 integer (`0`, never `-0`, and no
   leading zero). Fractional/exponent input numbers are rejected until Polkagent
   adopts an explicit decimal type. The byte limit measures the exact canonical
   input representation; depth is rejected before canonical writing.
7. Normalize capability and pallet sets as sorted unique non-empty strings.
8. Reject non-finite or negative grant budgets, normalize both `0.0` and `-0.0`
   to `"0"`, and encode every other accepted IEEE-754 value using direct Ryū
   shortest round-trip decimal text inside a JSON string. Terminal and group
   spend use unsigned smallest-unit integers encoded as canonical decimal
   strings; they never reuse `TaskResult.budget_spent: f64`. This preserves the
   normalized grant value without treating binary float as money. The policy
   gate must still narrow it against the hard `u64` group budget, and a later
   money-type migration requires a new contract version.
9. Compute `blake3-v1:<64 lowercase hex>` over the exact byte concatenation
   `b"polkagent.group-execution-plan.v1" || [0x00] || canonical_v1_json_utf8`.
   The JSON bytes contain no BOM or trailing newline. Prepare idempotency
   compares this digest plus immutable parent/group identity; tests must pin the
   full canonical bytes and digest, not only semantic JSON equality.

The normalized DTO is the migration fixture. Derived serde for
`ExecutionPlan` remains an internal convenience and may evolve without changing
durable rows.

### Compatibility and upcasting

Every durable plan row retains its original `schema_version`, canonical bytes,
and digest. Decoding dispatches on the exact version:

- version 1 verifies its digest before JSON decoding, rejects duplicate keys
  before constructing `serde_json::Value`, decodes into the v1 DTO, revalidates
  its invariants, requires byte-for-byte canonical re-encoding, and then
  upcasts into the current in-memory launch model without losing accepted v1
  data;
- an unknown future version fails closed rather than being partially decoded;
- the unversioned current-serde diagnostic fixture is never auto-imported or
  guessed as v1;
- a future v2 reader may upcast v1 in memory, but it must not rewrite v1 bytes or
  digest during an ordinary read or retry; and
- new writes use the active accepted version. There is no implicit downcast.

Public decode errors classify duplicate keys, invalid structure, invalid task
IDs, unsupported contracts, and syntax location without echoing
attacker-controlled key names or values into logs or surfaces.

Any intentional rewrite is a separate forward migration that records old/new
version and digest evidence. Idempotency always compares the originally stored
version and digest, so a software upgrade cannot turn a conflicting retry into
an identical one.

The upcast is lossless after accepted normalization. It does not reconstruct
duplicate capability/pallet entries or the sign bit of `-0.0`, because those
details are intentionally absent from the v1 DTO.

### Proposed ledger records and states

After the codec is accepted, the port should expose normalized records rather
than raw SQL or surface types.

`GroupExecutionStatus`:

```text
Prepared -> Active -> Succeeded | Failed | Cancelled
    |          |
    +----------+-> CancelRequested -> Cancelled | Failed
```

`GroupTaskStatus`:

```text
Prepared -> ChildAttributed -> Succeeded | Failed | Cancelled
```

`Prepared` means the execution row, every task, and every dependency edge
committed in one transaction. `ChildAttributed` means the exact child identity
tuple is durable; it does not claim that a child process or run has started.
Terminal task records use a separate versioned result DTO with canonical output,
bounded summary, integer duration units, integer budget units, and the attached
child identity.

### Proposed `GroupExecutionStore` operations

The port should contain only atomic persistence operations:

- `prepare_execution`: insert the execution, all tasks, and all edges in one
  transaction. An identical immutable request returns the stored record; a
  reused execution ID with a different digest or parent/group identity returns
  a typed conflict. No partial plan may be observable.
- `attach_child_identity`: expected-state CAS from `Prepared` to
  `ChildAttributed`. The same task/identity retry succeeds; a different tuple,
  missing task, cancel-requested parent, or wrong state returns a typed error.
- `record_task_terminal`: expected-state CAS from `ChildAttributed` to one
  terminal state. An identical result digest retry succeeds; a different result
  or terminal state conflicts.
- `request_cancellation`: atomically set the parent cancellation request and
  snapshot the exact nonterminal attributed child run IDs into cancellation
  targets. Every retry returns that same snapshot, even if child terminal rows
  arrive later. It never fabricates IDs for tasks that were not attributed.
- read operations for one execution, its ordered tasks/edges, ready prepared
  tasks, and the stored cancellation target snapshot.

SQLite will store task IDs as canonical decimal `TEXT`, not signed `INTEGER`, so
the complete `u64` range survives. Foreign keys and unique constraints protect
execution/task/child identity relationships, while transactions and conditional
updates enforce atomicity. Migration registration remains a separate
`RuntimeFactory` integration-owner change.

### Group-definition concurrency prerequisite

Validation through `GroupService` currently has no monotonic group-definition
revision: member mutations do not provide a token that a later prepare
transaction can compare. The accepted implementation must first add a durable
group revision (incremented for every member or policy mutation) or an
equivalent canonical snapshot token. `prepare_execution` carries that token and
fails if the definition changed between service validation and atomic prepare.
It must not rely on process-local locking.

### Launcher and canceller handoff

The next service layer needs two narrow, surface-neutral runtime capabilities:

```text
GroupChildLauncher.launch(PreparedChildLaunch) -> exact ChildIdentity
GroupChildCanceller.cancel(ChildRunId) -> acknowledgement
```

`PreparedChildLaunch` includes the already-durable execution/task/group/agent,
parent identity, normalized input/grant, and pre-reserved child identity. The
launcher must accept caller-supplied IDs and treat the child `RunId` as its
idempotency key. The service attaches those IDs before calling the launcher, so
a crash can safely retry the same launch without an unattributed child.

Cancellation is also outside the database transaction. The service reads the
durable cancellation-target snapshot and calls the canceller once per exact run
ID; acknowledgement/result recording is retryable. Neither handoff permits the
ledger adapter to execute or cancel children itself.

## Consequences

- The accepted codec packet deliberately adds no schema, store port, group
  revision, launcher, runtime execution, or cancellation implementation.
- A subsequent revision/ledger packet can add the dedicated SQLite ledger only
  after the durable group-definition revision exists, with real
  restart/concurrency/CAS tests.
- TUI, ACP, CLI, HTTP, central migration registration, and `RuntimeFactory`
  composition remain out of scope until the headless ledger and launcher path
  are proven.
- Existing synchronous executors remain usable for unit tests but are not
  evidence of durable child orchestration.

## Rejected alternatives

- **Persist `serde_json::to_string(ExecutionPlan)` directly:** unversioned and
  noncanonical; cannot support stable digests or conflict detection.
- **Normalize only at read time:** cannot distinguish an identical retry from a
  conflicting write and leaves stored evidence ambiguous.
- **Hold a SQLite transaction across child launch/cancel:** couples database
  locks to external work and still cannot make the external effect atomic.
- **Attach child IDs after launch:** a crash can create an unattributed child.
- **Use process-local locks for group membership validation:** does not protect
  restart or multiple processes.
