# PRD-10: Data, Artifacts, Events, Observability and Recovery

> **Implementation note (audited 2026-08-05):** This PRD remains normative,
> but its embedded implementation statements and checklists are not current
> status evidence. Use [STATUS.md](STATUS.md) and
> [IMPLEMENTATION-BACKLOG.md](IMPLEMENTATION-BACKLOG.md) for verified state and
> the dependency-ordered execution queue.

**Status:** definitive PRD
**Audience:** engineers, product designers, operators, and security reviewers with
no prior Polkagent, Roko, or `polkadot-chat-agents` context
**Last updated:** 2026-07-30
**Authority:** `01-ESTABLISHED-BASELINE.md` is authoritative if this document
conflicts with it.

---

## Table of contents

1. [Purpose and orientation](#1-purpose-and-orientation)
2. [Scope, non-goals, and cross-PRD interfaces](#2-scope-non-goals-and-cross-prd-interfaces)
3. [Terms and definitions](#3-terms-and-definitions)
4. [Personas and jobs affected](#4-personas-and-jobs-affected)
5. [Artifact taxonomy](#5-artifact-taxonomy)
6. [Artifact lifecycle](#6-artifact-lifecycle)
7. [Artifact storage](#7-artifact-storage)
8. [Event system](#8-event-system)
9. [Event stream architecture](#9-event-stream-architecture)
10. [Observability stack](#10-observability-stack)
11. [Evidence-bearing effects (E1)](#11-evidence-bearing-effects-e1)
12. [Replay and debugging (E4)](#12-replay-and-debugging-e4)
13. [Optional Bulletin/IPFS anchoring (E2)](#13-optional-bulletinipfs-anchoring-e2)
14. [Live run timeline (F3)](#14-live-run-timeline-f3)
15. [Recovery mechanisms](#15-recovery-mechanisms)
16. [Data classification](#16-data-classification)
17. [Database schema sketches](#17-database-schema-sketches)
18. [Wire formats](#18-wire-formats)
19. [Performance requirements](#19-performance-requirements)
20. [Acceptance criteria and verification checklist](#20-acceptance-criteria-and-verification-checklist)

---

## 1. Purpose and orientation

### 1.1 What this document covers

This PRD specifies how Polkagent stores, classifies, streams, observes, and
recovers all data produced by agents, runs, effects, and operator actions. It
is the authoritative reference for:

- **Artifacts** -- the durable, attributable evidence and output that Polkagent
  produces (decoded calls, simulation reports, signed receipts, plans, diffs,
  test results, memory items).
- **Events** -- the ordered observations that describe lifecycle transitions,
  streaming activity, errors, approvals, and outcome states.
- **Observability** -- the metrics, traces, logs, and projections that let
  operators, users, and support engineers understand system behavior.
- **Recovery** -- the procedures that restore correct state after crashes,
  corruption, operator error, or disaster.

### 1.2 Why data management matters to Polkagent

Polkagent occupies a position of asymmetric risk. It prepares, explains,
simulates, approves, signs, and submits chain actions that may move real value.
A user must be able to answer, after the fact:

- Which runtime metadata was used when a call was decoded?
- Did a simulation precede the signature request?
- What did the signer see, and does it match what was submitted?
- Did the same intent ever produce two broadcast attempts?
- What policy revision authorized the effect?

These questions cannot be answered from a live token stream or from in-memory
state that vanishes on crash. They require durable, linked, classified,
integrity-protected records and deterministic projections that recover from
those records.

At the same time, retaining every streaming chunk, every model prompt, or every
raw provider response creates storage cost, privacy liability, and compliance
exposure. The data architecture must therefore distinguish:

- **Material evidence** that must be retained for correctness, audit, and
  recovery.
- **Operational diagnostics** that are useful for debugging but may be
  bounded, redacted, and expired.
- **Ephemeral activity** such as typing indicators and progress percentages
  that are never persisted.

### 1.3 Relationship to the research corpus

This PRD synthesizes and codifies findings from:

- `00-DEEP-RESEARCH-BRIEF.md` -- artifact/event definitions, research quality
  rules, and finding format.
- `01-ESTABLISHED-BASELINE.md` -- durable records (section 6.2), core
  invariants (section 6.3), stable ports (section 6.4), and deployment topology
  (section 12).
- `research-roko-definitive.md` -- Pattern 1 (artifact/event split), Pattern 2
  (run ledger/outbox), Pattern 11 (projections/lenses), and the adoption
  matrix.
- `reserach/research1.md` -- E1 evidence-bearing effects, E4 replay/debugging,
  E2 Bulletin anchoring, F3 live run timeline candidates.
- `reserach/research2.md` -- Prompt 11 (SQLite authority store), Prompt 17
  (evidence packages/Bulletin anchoring).
- `reserach/research3.md` -- Domain 10 cross-domain synthesis: content
  addressing (BLAKE3/SHA-256), event sourcing with schema evolution,
  OpenTelemetry + `tracing` spine, structured logging with secret redaction,
  SQLite write-throughput tuning, coordinated DB+blob backup, and Bulletin
  anchoring caveats. Staged recommendations (do-now/validate-next/defer/avoid)
  from that synthesis are reflected throughout this document.

Where this PRD specifies a concrete schema, wire format, or behavioral
contract, it supersedes the research documents' proposals on the same topic.

### 1.4 Staged implementation guidance (from research synthesis)

The following phasing guidance is drawn from research3.md Domain 10 synthesis
and applies to the engineering roadmap for this PRD.

| Phase | Items |
|---|---|
| **do-now** | Tracing + OpenTelemetry spine with token/cost tracking; secret redaction enforced at the tracing-layer boundary; content-addressed artifact store (SHA-256 external, optional BLAKE3 internal). |
| **validate-next** | Versioned event schema evolution with migration functions; coordinated DB+blob backup using the SQLite backup API with a `global_sequence` watermark. |
| **defer** | PostgreSQL table partitioning for large multi-tenant scale; CQRS projection rebuild orchestration tooling at scale (lens rebuild is already specified but tooling automation is deferred). |
| **avoid** | Logging secret material or raw sensitive calldata at any log level; treating Bulletin Chain as a durable long-term store (it is a supplementary time-limited anchor, not an archive). |

### 1.5 Design principles

| Principle | Implication |
|---|---|
| **Durable facts differ from live activity** | Material evidence is retained and linked; streaming progress is lossy and coalescible. |
| **Evidence before action** | Every consequential effect records its inputs, policy, and authorization before the I/O occurs. |
| **Immutability at evidence boundaries** | Artifacts, effect outcomes, and resolved grants are append-only. Mutation is a new version with explicit lineage. |
| **Content-addressed integrity** | Artifact digests enable tamper detection and deduplication without global coordination. |
| **Classification by default** | Every record carries a sensitivity level. Secret material is excluded from logs, projections, and model context by default. |
| **Projections, not mutable views** | UIs, CLIs, APIs, and operator dashboards read versioned projections derived from durable records. |
| **Local-first, cloud-portable** | SQLite is the default authority store. PostgreSQL is the managed-cloud equivalent. The schema contract is portable. |
| **Recovery from durable state** | A restart, reconnect, or migration must recover truthful status from persisted records, not from in-memory caches or live streams. |

---

## 2. Scope, non-goals, and cross-PRD interfaces

### 2.1 In scope

- Artifact types, lifecycle, storage, lineage, and provenance.
- Event types, ordering, delivery, persistence, and replay.
- In-process and cross-process event streaming.
- Observability: metrics, traces, structured logs, and projections.
- Evidence-bearing effects (E1).
- Deterministic replay and debugging (E4).
- Optional Bulletin/IPFS anchoring (E2).
- Live run timeline projection (F3).
- Crash recovery, backup/restore, point-in-time recovery, disaster recovery.
- Data classification and sensitivity handling.
- Database schema sketches for core tables.
- Wire formats for events and artifact metadata.
- Performance requirements: latency, throughput, and storage budgets.

### 2.2 Non-goals

- Policy evaluation logic (PRD-07).
- Model/provider execution semantics (PRD-04).
- Chain client and signer contracts (PRD-05, PRD-07).
- UX rendering of timelines and approval cards (PRD-13).
- Multi-tenancy isolation enforcement (PRD-11).
- Marketplace manifest schemas (PRD-12).
- Memory and knowledge store admission/retrieval (PRD-09).
- API versioning and migration tooling (PRD-14).
- Security threat model and red-team scenarios (PRD-15).

### 2.3 Cross-PRD interfaces

| Interface | Consuming PRD | Direction |
|---|---|---|
| `ArtifactStore` trait | PRD-03 (execution), PRD-09 (memory) | This PRD defines; others consume |
| `EventStore` / `EventSink` trait | PRD-03, PRD-13 (UX) | This PRD defines; others consume |
| `EffectStore` trait | PRD-03 | This PRD defines; PRD-03 specifies effect state machines |
| Data classification levels | PRD-07 (security), PRD-16 (classification) | This PRD defines levels; PRD-07 enforces them |
| Projection contracts | PRD-13 (UX surfaces), PRD-11 (cloud) | This PRD defines derivation; others consume |
| Database schemas | PRD-14 (migration) | This PRD sketches; PRD-14 versions and migrates |
| Backup/restore contracts | PRD-11 (cloud), PRD-15 (assurance) | This PRD defines; others implement topology |

---

## 3. Terms and definitions

| Term | Definition |
|---|---|
| **Artifact** | A durable, attributable piece of content or evidence with a content digest, type, classification, provenance, and parent references. Examples: a decoded call, a simulation report, a signed receipt, a metadata snapshot. |
| **Artifact lineage** | The directed acyclic graph (DAG) of parent-child relationships between artifacts. Each artifact names the artifacts it was derived from. |
| **Provenance** | The record of who or what produced an artifact: source system, software version, timestamp, operator, and input references. |
| **Content digest** | A cryptographic hash (SHA-256) of an artifact's canonical byte content, computed at creation time. Used for integrity verification and deduplication. |
| **Blob** | The raw byte content of an artifact body, stored separately from its metadata. |
| **Run event** | An ordered observation of lifecycle or streaming activity within a run. Events have a per-run monotonic sequence number. |
| **Durability class** | Whether an event is `Durable` (must survive crash), `Diagnostic` (retained with bounded lifetime), or `Ephemeral` (never persisted). |
| **Effect** | Actual external I/O: invoking a model, running a tool, requesting a signature, broadcasting a transaction, or observing finality. |
| **EffectIntent** | The durable command recorded before the I/O occurs. |
| **EffectAttempt** | One claim, lease, retry number, idempotency key, and worker lifecycle for a single attempt at an intent. |
| **EffectOutcome** | The immutable observed result of exactly one attempt: success, failure, timeout, cancellation, or unknown. |
| **Projection** | A versioned, recoverable read model derived from durable records for consumption by UIs, CLIs, APIs, or operator tools. |
| **Lens** | A named, read-only projection with a defined schema, refresh policy, and access-control scope. Inspired by Roko's Lens concept. |
| **Classification** | A data sensitivity level: `Public`, `Internal`, `Private`, `Sensitive`, or `SecretForbidden`. |
| **Correlation ID** | A stable identifier that links related events, artifacts, and effects across run boundaries. |
| **Causation ID** | The identifier of the specific event or effect that directly caused a subsequent event. |
| **Cursor** | A position marker in an event stream, enabling resume-from-position semantics. |
| **WAL** | Write-ahead log. SQLite uses WAL mode for concurrent read/write access. |
| **SSE** | Server-Sent Events, a unidirectional HTTP streaming protocol. |
| **CQRS** | Command Query Responsibility Segregation: separating write (command) and read (query) models. |

---

## 4. Personas and jobs affected

| Persona | Job | Data/observability need |
|---|---|---|
| **Polkadot user** | Understand what an agent did with their account | Artifact lineage from intent through signature to finality; truthful outcome projection |
| **Application developer** | Debug agent behavior during development | Structured logs, traces, event replay, artifact inspection |
| **Runtime/chain engineer** | Verify metadata used for decode/simulation | Pinned metadata artifacts with digest, provenance, and expiry |
| **Product team** | Monitor agent fleet health and costs | Metrics dashboards, usage projections, cost aggregation |
| **Organization/operator** | Audit actions, recover from failures, comply with data policies | Classified event streams, backup/restore, retention enforcement, export |
| **Autonomous-agent owner** | Verify that autonomous actions matched policy | Evidence-bearing effect chains, resolved grant snapshots, receipt artifacts |
| **Support engineer** | Diagnose user-reported issues | Correlation ID lookup, deterministic replay, structured error context |
| **Security reviewer** | Detect unauthorized actions or data exposure | Classification enforcement logs, grant audit trail, secret-access records |

---

## 5. Artifact taxonomy

### 5.1 Artifact types

Each artifact has a `kind` field drawn from a typed enumeration. New kinds may
be added through versioned schema evolution; existing kind semantics must not
change without a migration.

| Kind | Description | Typical producer | Example content |
|---|---|---|---|
| `File` | A user-visible file or document | Harness, tool, user upload | Source code file, configuration, image |
| `Diff` | A change set between two states | Coding harness, migration tool | Git diff, storage diff, metadata diff |
| `Plan` | A proposed sequence of actions | Model, workflow engine | Step-by-step action plan with estimated effects |
| `DecodedCall` | A human-readable representation of a chain call | Chain client, metadata decoder | Decoded extrinsic with pallet, call, and arguments |
| `MetadataSnapshot` | A pinned runtime metadata record | Chain client | Runtime metadata with genesis hash, spec version, and metadata hash |
| `SimulationResult` | Output of a dry-run or fork simulation | Chain client, Chopsticks adapter | State changes, events, fees, XCM effects |
| `Receipt` | A composite evidence record for a completed action | Effect/saga coordinator | Links to intent, approval, signature, broadcast, and finality artifacts |
| `TestResult` | Output of a test, benchmark, or evaluation run | Tool, harness, evaluation engine | Pass/fail, coverage, timing, resource usage |
| `MemoryItem` | A durable knowledge or episode record | Memory subsystem | Attributed fact, playbook rule, or episode summary |
| `ContextPack` | The assembled context sent to a model | Context assembler | Included/excluded items, token budget, digest |
| `ModelResponse` | The raw or normalized response from a model | Executor | Completion text, tool calls, usage, stop reason |
| `ApprovalRecord` | A captured authorization decision | Approval service | Approver identity, timestamp, bound payload hash, policy revision |
| `SignedPayload` | The exact bytes signed by a signer | Signer adapter | SCALE-encoded extrinsic with signature |
| `BroadcastReceipt` | Evidence of transaction submission | Broadcaster | Transaction hash, RPC endpoint, submission timestamp |
| `FinalityObservation` | Evidence of inclusion/finality state | Finality watcher | Block hash, block number, finality status, observation time |
| `PolicySnapshot` | The resolved policy/grant at decision time | Policy evaluator | Serialized `ResolvedGrant` with policy revision digest |
| `ErrorReport` | Structured error with context | Any component | Error code, message, component, correlation ID, recovery hint |
| `DiagnosticBundle` | Bounded operational diagnostics | Runtime, adapters | Redacted logs, timing, resource usage for one run/effect |
| `ExportPackage` | A portable data export | Export service | Versioned archive of artifacts, events, and metadata |

### 5.2 Artifact type rules

**REQ-ART-001.** Every artifact kind must declare:
- Whether its content is typically `Immutable` or `Versioned`.
- Its default classification level.
- Its default retention tier (see section 7.5).
- Whether it may contain secret material (and if so, the required redaction
  strategy).

**REQ-ART-002.** The artifact kind enumeration is extensible through schema
evolution. Adding a new kind must not require changes to artifact storage,
lineage tracking, or projection infrastructure.

**REQ-ART-003.** `File` and `Diff` artifacts may reference workspace-scoped
content. Their blob storage respects workspace isolation boundaries.

### 5.3 Artifact structure

Every artifact shares a common metadata envelope:

```rust
/// Core artifact metadata. The body content is stored separately as a blob.
pub struct ArtifactMeta {
    /// Globally unique artifact identifier.
    pub id: ArtifactId,

    /// The type of content this artifact represents.
    pub kind: ArtifactKind,

    /// SHA-256 digest of the canonical body bytes.
    pub content_digest: Sha256Digest,

    /// Size of the body in bytes.
    pub content_size: u64,

    /// MIME type of the body content, when applicable.
    pub content_type: Option<String>,

    /// Artifacts this one was derived from (evidence lineage).
    pub parents: Vec<ArtifactId>,

    /// Who or what produced this artifact.
    pub provenance: Provenance,

    /// Data sensitivity level.
    pub classification: Classification,

    /// The run that produced this artifact, if any.
    pub run_id: Option<RunId>,

    /// The effect attempt that produced this artifact, if any.
    pub effect_attempt_id: Option<EffectAttemptId>,

    /// The conversation this artifact belongs to, if any.
    pub conversation_id: Option<ConversationId>,

    /// The workspace/tenant scope.
    pub scope: ScopeId,

    /// When the artifact was created.
    pub created_at: Timestamp,

    /// When the artifact expires (if retention policy applies).
    pub expires_at: Option<Timestamp>,

    /// Application-specific labels for filtering and indexing.
    pub labels: BTreeMap<String, String>,

    /// Schema version for this metadata structure.
    pub schema_version: u32,
}

/// Where an artifact came from.
pub struct Provenance {
    /// The system or component that produced this artifact.
    pub source: ProvenanceSource,

    /// Software version that produced it.
    pub software_version: String,

    /// Timestamp of production.
    pub produced_at: Timestamp,

    /// The operator or principal, if attributable.
    pub principal: Option<PrincipalId>,

    /// Input artifact references that contributed to production.
    pub input_refs: Vec<ArtifactId>,

    /// Chain/network evidence, if chain-related.
    pub chain_evidence: Option<ChainEvidence>,
}

/// Chain-specific provenance fields.
pub struct ChainEvidence {
    /// Genesis hash of the target chain.
    pub genesis_hash: H256,

    /// Runtime spec version at the time of evidence capture.
    pub spec_version: u32,

    /// Block hash at which evidence was captured.
    pub at_block_hash: H256,

    /// Block number at which evidence was captured.
    pub at_block_number: u64,

    /// Metadata hash, if metadata-hash checking is supported.
    pub metadata_hash: Option<H256>,
}
```

### 5.4 Artifact identifiers

**REQ-ART-010.** Artifact IDs are locally generated ULIDs (Universally Unique
Lexicographically Sortable Identifiers). ULIDs provide:
- Monotonic ordering within a millisecond.
- No coordination required for generation.
- Sortable by creation time.
- 128-bit collision resistance.

**REQ-ART-011.** Content digests use **SHA-256** as the canonical algorithm,
computed over the canonical byte representation of the artifact body. SHA-256
is required for interoperability with external ecosystems (Sigstore, IPFS
CIDv0, on-chain anchoring). Internally, the implementation **may** also
maintain a BLAKE3 digest for fast deduplication and integrity checks where
external interop is not required — BLAKE3 is faster and parallelizable for
high-throughput artifact ingestion. When both digests are present, SHA-256
is the authoritative external reference and BLAKE3 is a performance aid. The
digest algorithm(s) in use are recorded in the metadata to support future
algorithm migration. (Research basis: research3.md finding 23 [V/I].)

**REQ-ART-012.** Artifact IDs are internal identifiers. When an artifact is
exported, referenced externally, or anchored on-chain, the SHA-256 content
digest serves as the canonical external reference.

---

## 6. Artifact lifecycle

### 6.1 Lifecycle states

```text
Created -> Classified -> Stored -> Referenced -> [Archived | Deleted]
```

| State | Description | Invariants |
|---|---|---|
| `Created` | Metadata and body bytes exist in memory | Content digest has been computed; kind and provenance are set |
| `Classified` | Classification level has been assigned | Classification follows the rules in section 16; inherited classification from inputs is applied |
| `Stored` | Metadata and body are durably persisted | Metadata is in the artifact table; body is in blob storage; both are within the same transactional boundary or use two-phase commit |
| `Referenced` | Other artifacts or events reference this artifact by ID | Parent links are immutable; reference integrity is maintained |
| `Archived` | Body is moved to cold storage; metadata retained | Metadata remains queryable; body retrieval may have higher latency |
| `Deleted` | Metadata is marked deleted; body is removed | Soft-delete with configurable hard-delete after retention period; referencing artifacts retain the ID but body is unavailable |

### 6.2 Lifecycle rules

**REQ-ART-020.** An artifact must not transition to `Stored` until its content
digest has been verified against the body bytes.

**REQ-ART-021.** Once an artifact is in `Stored` state, its metadata fields
`id`, `kind`, `content_digest`, `content_size`, `parents`, `provenance`,
`classification`, and `created_at` are immutable. Only `labels`,
`expires_at`, and lifecycle state may change.

**REQ-ART-022.** Artifact deletion is a soft-delete. The metadata row is
retained with a `deleted_at` timestamp. The body blob is eligible for
removal after the configured retention period.

**REQ-ART-023.** Deleting an artifact does not cascade to its children or
parents. Referencing artifacts retain the parent ID; attempts to retrieve
the deleted body return a `NotFound` result with the original metadata
available for audit.

**REQ-ART-024.** Archival moves the body blob from hot storage to cold
storage. The metadata row is updated with a `storage_tier` field. Retrieval
from cold storage is asynchronous and may have multi-second latency.

### 6.3 Lineage and provenance tracking

**REQ-ART-030.** Every artifact records zero or more parent artifact IDs in
its `parents` field. This forms a directed acyclic graph (DAG).

**REQ-ART-031.** The lineage DAG must support the following queries:
- **Ancestors:** Given an artifact, find all artifacts in its lineage chain.
- **Descendants:** Given an artifact, find all artifacts derived from it.
- **Lineage chain:** Given a receipt artifact, reconstruct the full evidence
  chain from intent through metadata, decode, simulation, approval, signature,
  broadcast, and finality.
- **Common ancestors:** Given two artifacts, find shared lineage.

**REQ-ART-032.** Provenance must distinguish:
- **System provenance:** produced by a Polkagent component (decoder, simulator,
  policy evaluator).
- **Model provenance:** produced by an LLM or coding harness.
- **User provenance:** uploaded or authored by a human user.
- **External provenance:** received from an external system (chain RPC,
  webhook, feed).
- **Derived provenance:** computed from other artifacts (e.g., a diff between
  two metadata snapshots).

**REQ-ART-033.** Chain-related artifacts must include `ChainEvidence` in their
provenance. A chain artifact without `ChainEvidence` is invalid and must be
rejected at the store boundary.

### 6.4 Immutability guarantees

**REQ-ART-040.** Artifacts in `Stored` state are append-only. To represent a
changed version of an artifact:
1. Create a new artifact with a new ID and content digest.
2. Set the original artifact as a parent of the new artifact.
3. Optionally add a `supersedes` label pointing to the original.

**REQ-ART-041.** Content digests must be recomputable. Given an artifact's
body bytes and the stated digest algorithm, any component must be able to
verify integrity independently.

**REQ-ART-042.** If integrity verification fails (content does not match
digest), the artifact must be quarantined: marked with a
`integrity_violation` label, excluded from projections, and an error event
emitted.

### 6.5 Content-addressed storage

**REQ-ART-050.** When two artifacts have identical content digests and the
same `kind`, the storage layer may deduplicate the body blob. Metadata
records remain separate.

**REQ-ART-051.** Deduplication must not cross classification boundaries. A
`Private` blob and a `Public` blob with identical content are stored
separately and subject to separate retention and access policies.

**REQ-ART-052.** Content-addressed lookup is an optional index. The primary
key is the artifact ID (ULID). Content-digest lookup is used for
deduplication, integrity verification, and external reference resolution.

---

## 7. Artifact storage

### 7.1 Local filesystem layout

The default local installation stores artifacts under a configurable root
directory:

```text
$POLKAGENT_DATA_DIR/
  config/                          # Configuration files
  db/
    polkagent.db                   # SQLite authority database
    polkagent.db-wal               # WAL file
    polkagent.db-shm               # Shared memory file
  blobs/
    <prefix>/<artifact-id>         # Blob bodies, sharded by ID prefix
  exports/
    <export-id>/                   # Exported packages
  backups/
    <timestamp>/                   # Point-in-time backup snapshots
  logs/
    polkagent.log                  # Structured log output
    polkagent.log.<N>              # Rotated log files
  tmp/
    uploads/                       # Temporary upload staging
    scratch/                       # Temporary processing
```

**REQ-STORE-001.** `$POLKAGENT_DATA_DIR` defaults to:
- macOS: `~/Library/Application Support/polkagent/`
- Linux: `~/.local/share/polkagent/`
- When `$XDG_DATA_HOME` is set: `$XDG_DATA_HOME/polkagent/`

**REQ-STORE-002.** Blob files are sharded by the first two characters of the
artifact ID to avoid directory inode exhaustion. Example:
`blobs/01/01J5ABCDEF1234567890.blob`.

**REQ-STORE-003.** Temporary files in `tmp/` are cleaned on startup. No
component may rely on `tmp/` contents surviving a restart.

### 7.2 Database storage

#### 7.2.1 SQLite for local deployments

**REQ-STORE-010.** SQLite is the default authority store for local and
self-hosted deployments.

**REQ-STORE-011.** SQLite configuration:
- WAL journal mode for concurrent reads during writes.
- `synchronous = NORMAL` for durability with acceptable performance.
- `foreign_keys = ON`.
- `journal_size_limit` configured to prevent unbounded WAL growth.
- Busy timeout of 5 seconds (`busy_timeout = 5000`).
- Page size of 4096 bytes (default).
- `wal_autocheckpoint` tuned for write throughput: the default (1000 pages)
  is appropriate for moderate load; high-throughput deployments may increase
  this value (e.g., 4000–8000 pages) to reduce checkpoint frequency at the
  cost of slightly larger WAL files. Checkpoint mode should be set to
  `PASSIVE` for background checkpointing and `TRUNCATE` on clean shutdown.
  (Research basis: research3.md finding [I] — SQLite event-store schema tuned
  for write throughput.)
- A **single-writer task** architecture in Tokio: one dedicated task owns the
  write connection and receives write commands via an `mpsc` channel; a pool
  of read connections serves concurrent reads. This avoids `SQLITE_BUSY`
  contention without requiring `SKIP LOCKED`. All write transactions use
  `BEGIN IMMEDIATE` to take the write lock at transaction start and avoid
  upgrade deadlocks.

**REQ-STORE-012.** All state changes for a single run/turn are committed in
a single SQLite transaction. Artifact metadata and effect intents created by
a reducer are committed atomically.

**REQ-STORE-013.** The SQLite database file must not exceed 1 TB. Retention
policies (section 7.5) and archival (section 6.2) prevent unbounded growth.

#### 7.2.2 PostgreSQL for managed deployments

**REQ-STORE-020.** Managed cloud deployments use PostgreSQL as the authority
store.

**REQ-STORE-021.** The PostgreSQL schema must be semantically equivalent to
the SQLite schema. A portable schema definition generates both dialects.

**REQ-STORE-022.** PostgreSQL deployments use:
- Connection pooling (e.g., PgBouncer or built-in pool).
- Row-level security for tenant isolation (see PRD-11).
- Advisory locks for single-writer enforcement where needed.
- Partitioning by tenant ID for large multi-tenant deployments.

**REQ-STORE-023.** Schema migrations use a versioned, forward-only migration
system. Each migration is idempotent and includes a rollback path.

### 7.3 Blob storage for large artifacts

**REQ-STORE-030.** Artifact bodies larger than 256 KB are stored in blob
storage rather than inline in the database.

**REQ-STORE-031.** Artifact bodies at or below 256 KB may be stored inline in the
database `artifact_body` column for reduced I/O and simpler transactions.

**REQ-STORE-032.** Local blob storage uses the filesystem layout in section 7.1.
Managed cloud blob storage uses an object store (S3-compatible) with:
- Server-side encryption.
- Tenant-scoped bucket prefixes or separate buckets.
- Lifecycle policies for archival and deletion.

**REQ-STORE-033.** Blob storage is write-once. A blob, once written, is never
modified. Overwrite attempts are rejected.

**REQ-STORE-034.** Blob retrieval returns the raw bytes and the stored content
digest. The caller must verify the digest.

### 7.4 Encryption at rest

**REQ-STORE-040.** Local deployments use filesystem-level encryption
(FileVault, LUKS, BitLocker) as the default encryption-at-rest mechanism.
Application-level encryption is optional and configurable.

**REQ-STORE-041.** When application-level encryption is enabled:
- Artifacts classified as `Sensitive` or `SecretForbidden` are encrypted
  before storage using AES-256-GCM.
- The encryption key is derived from a master key stored in the platform's
  secret resolver (keychain, KMS, HSM, or configured secret store).
- Each artifact uses a unique nonce. The nonce is stored alongside the
  encrypted blob.
- Key rotation creates new keys for future writes; existing blobs are
  re-encrypted during a background migration.

**REQ-STORE-042.** Managed cloud deployments use server-side encryption for
all blob storage and transparent data encryption (TDE) or equivalent for
database storage, plus optional application-level encryption for
`Sensitive` and `SecretForbidden` classifications.

**REQ-STORE-043.** Encryption key identifiers are stored in artifact metadata.
The actual key material is never stored in the artifact table, event log,
or projection tables.

### 7.5 Retention policies

**REQ-STORE-050.** Retention is configured per artifact kind, classification
level, and scope (tenant/workspace/agent).

**REQ-STORE-051.** Default retention tiers:

| Tier | Default duration | Applicable kinds | Behavior at expiry |
|---|---|---|---|
| `Evidence` | 2 years | Receipt, SignedPayload, BroadcastReceipt, FinalityObservation, ApprovalRecord, PolicySnapshot | Archive to cold storage |
| `Operational` | 90 days | ContextPack, ModelResponse, DiagnosticBundle, ErrorReport | Soft-delete, then hard-delete after 30 days |
| `Workspace` | Lifetime of workspace | File, Diff, Plan, TestResult | Deleted with workspace, or after configured duration |
| `Ephemeral` | 7 days | Streaming chunks, progress snapshots | Hard-delete |
| `Custom` | Configurable | Any | Operator-defined |

**REQ-STORE-052.** A retention enforcement job runs periodically (default:
daily) and processes expired artifacts according to their tier behavior.

**REQ-STORE-053.** Retention policies must not delete artifacts that are
referenced as parents by non-expired artifacts, unless the entire lineage
chain is eligible for deletion.

**REQ-STORE-054.** Operators may override retention per artifact, per kind,
or per scope. The effective retention is the longest of all applicable
policies.

**REQ-STORE-055.** When a user exercises a data deletion right (e.g., GDPR
erasure), the retention policy is overridden. The deletion must cascade to
blob storage, derived indexes, and memory stores. A deletion audit record
is retained.

---

## 8. Event system

### 8.1 Event types catalog

Events are the ordered observations that describe what happened during a run,
an effect, or a system lifecycle transition. Each event has a type drawn from
a typed enumeration.

#### Schema evolution

**REQ-EVT-SCHEMA-001.** Event types are **versioned**. Each event payload
carries a `schema_version` field. When the payload schema for an event type
changes, the version is incremented and both the old and new schema versions
are supported for a defined migration window.

**REQ-EVT-SCHEMA-002.** New optional fields may be added to an event payload
in a minor schema update (backward-compatible). Removing or renaming fields,
or changing field types, requires a new schema version.

**REQ-EVT-SCHEMA-003.** The event store must be able to deserialize events
written with any supported schema version. A migration function upgrades
older-version payloads to the current schema on read, or during an explicit
migration sweep. Deterministic replay (section 12) must use the
**original schema version** of each event to ensure replay accuracy — events
are not upconverted before replay unless the replay target explicitly opts in
to a schema migration. (Research basis: research3.md finding [I] — event
sourcing with schema evolution.)

#### 8.1.1 Lifecycle events

These events mark transitions in the run, conversation, or system lifecycle.

| Event type | Description | Durability class |
|---|---|---|
| `RunCreated` | A new run has been created | Durable |
| `RunStarted` | A run has begun execution | Durable |
| `RunCompleted` | A run has finished successfully | Durable |
| `RunFailed` | A run has terminated with an error | Durable |
| `RunCancelled` | A run has been cancelled by user or policy | Durable |
| `RunTimedOut` | A run exceeded its deadline | Durable |
| `RunResumed` | A run has resumed from a checkpoint | Durable |
| `TurnStarted` | A turn within a run has begun | Durable |
| `TurnCompleted` | A turn has finished | Durable |
| `ConversationCreated` | A new conversation has been created | Durable |
| `ConversationClosed` | A conversation has been closed | Durable |
| `WorkspaceCreated` | A new workspace has been created | Durable |
| `WorkspaceDeleted` | A workspace has been deleted | Durable |

#### 8.1.2 Streaming events

These events carry incremental model output and are typically lossy.

| Event type | Description | Durability class |
|---|---|---|
| `TextDelta` | Incremental text from a model response | Ephemeral |
| `ToolCallStart` | A tool invocation has begun | Diagnostic |
| `ToolCallEnd` | A tool invocation has completed | Diagnostic |
| `ThinkingStart` | Model reasoning/thinking has begun | Ephemeral |
| `ThinkingDelta` | Incremental reasoning text | Ephemeral |
| `ThinkingEnd` | Model reasoning has completed | Ephemeral |
| `ProgressUpdate` | Percentage or stage progress | Ephemeral |

#### 8.1.3 Effect events

These events record the lifecycle of external effects.

| Event type | Description | Durability class |
|---|---|---|
| `EffectIntentCreated` | A new effect intent has been recorded | Durable |
| `EffectAttemptStarted` | A worker has claimed an effect intent | Durable |
| `EffectAttemptCompleted` | An attempt has finished (any outcome) | Durable |
| `EffectOutcomeRecorded` | The immutable result has been stored | Durable |
| `EffectRetryScheduled` | A retry has been scheduled | Durable |
| `EffectCancelled` | An effect has been cancelled | Durable |
| `EffectLeaseExpired` | A worker's claim lease has expired | Durable |

#### 8.1.4 Approval events

| Event type | Description | Durability class |
|---|---|---|
| `ApprovalRequested` | An action requires human/quorum approval | Durable |
| `ApprovalGranted` | Approval has been given | Durable |
| `ApprovalDenied` | Approval has been denied | Durable |
| `ApprovalExpired` | An approval request has timed out | Durable |
| `MandateApplied` | A configured autonomous mandate authorized the action | Durable |

#### 8.1.5 Error events

| Event type | Description | Durability class |
|---|---|---|
| `ErrorOccurred` | A recoverable error has occurred | Durable |
| `PanicRecovered` | A panic was caught and the run was preserved | Durable |
| `IntegrityViolation` | An artifact or event failed integrity check | Durable |
| `PolicyDenial` | A policy evaluation denied an action | Durable |
| `ClassificationEscalation` | Data exceeded its expected classification | Durable |

#### 8.1.6 System events

| Event type | Description | Durability class |
|---|---|---|
| `SystemStarted` | The Polkagent runtime has started | Durable |
| `SystemShutdown` | The runtime is shutting down gracefully | Durable |
| `BackupCompleted` | A backup has finished successfully | Durable |
| `RetentionEnforced` | A retention sweep has completed | Durable |
| `SchemasMigrated` | Database schemas have been migrated | Durable |
| `ConfigurationChanged` | Runtime configuration has been reloaded | Durable |

### 8.2 Event structure

```rust
/// A single event in the Polkagent event stream.
pub struct RunEvent {
    /// Globally unique event identifier (ULID).
    pub id: EventId,

    /// The type of event.
    pub event_type: EventType,

    /// Per-run monotonic sequence number. Unique within a run.
    pub sequence: u64,

    /// Global monotonic sequence number across all runs.
    /// Used for cross-run ordering and cursor-based streaming.
    pub global_sequence: u64,

    /// The run this event belongs to.
    pub run_id: RunId,

    /// The conversation this event belongs to, if any.
    pub conversation_id: Option<ConversationId>,

    /// Correlation ID for linking related events across runs.
    pub correlation_id: CorrelationId,

    /// The event that directly caused this event.
    pub causation_id: Option<EventId>,

    /// Workspace/tenant scope.
    pub scope: ScopeId,

    /// When the event occurred.
    pub timestamp: Timestamp,

    /// Durability class determines persistence behavior.
    pub durability: DurabilityClass,

    /// Typed event payload.
    pub payload: EventPayload,

    /// References to artifacts created or referenced by this event.
    pub artifact_refs: Vec<ArtifactId>,

    /// Schema version for this event structure.
    pub schema_version: u32,
}

pub enum DurabilityClass {
    /// Must survive crash. Persisted synchronously.
    Durable,
    /// Useful for debugging. Persisted asynchronously with bounded lifetime.
    Diagnostic,
    /// Never persisted. Streamed only.
    Ephemeral,
}
```

### 8.3 Event ordering guarantees

**REQ-EVT-001.** Events within a single run are totally ordered by their
per-run `sequence` number. No two events in the same run share a sequence
number.

**REQ-EVT-002.** The `global_sequence` provides a total order across all
runs within a scope (tenant/instance). It is generated by the authority
store (auto-incrementing integer in SQLite/PostgreSQL).

**REQ-EVT-003.** Durable events are visible to readers only after the
transaction that created them has committed. There is no speculative
read of uncommitted events.

**REQ-EVT-004.** A run's terminal event (`RunCompleted`, `RunFailed`,
`RunCancelled`, `RunTimedOut`) is the last durable event in its sequence.
At most one terminal event exists per run.

**REQ-EVT-005.** Ephemeral events are delivered best-effort and may be lost
on crash, network interruption, or backpressure. Consumers must not depend
on receiving every ephemeral event.

### 8.4 Event delivery semantics

**REQ-EVT-010.** Durable events use **at-least-once** delivery with
**idempotent consumers**.

**REQ-EVT-011.** Each consumer maintains a cursor (the `global_sequence` of
the last processed event). On reconnect, the consumer resumes from its
cursor position. Events with sequence numbers at or below the cursor are
skipped by idempotent processing.

**REQ-EVT-012.** Consumers must be idempotent: processing the same event
twice must produce the same observable effect. Projections use
upsert-or-skip semantics keyed by event ID.

**REQ-EVT-013.** Ephemeral events use **at-most-once** delivery. If a
consumer is not connected when an ephemeral event is produced, the event
is lost.

### 8.5 Event subscription model

**REQ-EVT-020.** Consumers subscribe to events using filters:
- By run ID (events for a specific run).
- By conversation ID (events for a specific conversation).
- By scope (events for a specific tenant/workspace).
- By event type (events matching specific types).
- By durability class (durable only, diagnostic, or all).
- By global sequence range (cursor-based pagination).

**REQ-EVT-021.** Subscriptions may be:
- **Pull-based:** Consumer polls with a cursor. Returns a batch of events
  since the cursor, up to a configurable batch size.
- **Push-based:** Consumer receives events via SSE, WebSocket, or
  in-process channel as they are produced.
- **Mixed:** Consumer uses push for real-time updates and pull for
  catch-up after reconnect.

**REQ-EVT-022.** Push-based subscriptions must include the consumer's cursor
in the subscription request. The server sends all events since that cursor
before switching to live streaming.

### 8.6 Event persistence and replay

**REQ-EVT-030.** Durable events are persisted in the authority store's
`run_events` table (see section 17).

**REQ-EVT-031.** Diagnostic events are persisted in a separate
`diagnostic_events` table with a shorter retention period.

**REQ-EVT-032.** Ephemeral events are never persisted. They exist only in
in-process channels and active WebSocket/SSE connections.

**REQ-EVT-033.** Event replay is the process of re-reading persisted events
from the authority store. Replay uses the same cursor-based subscription
model as live streaming.

**REQ-EVT-034.** Event replay must reproduce the exact sequence of durable
events for a given run. The replayed sequence must be identical to the
original, including event IDs, sequence numbers, timestamps, and payloads.

**REQ-EVT-035.** Diagnostic events may be absent during replay if they have
been expired by retention enforcement. Consumers must tolerate gaps in
diagnostic events.

**Implementation evidence (2026-08-06).** SQLite migration V19 preserves the
complete `StoredEvent` envelope and the canonical `RunEvent` turn, step,
effect-intent, effect-attempt, and causation IDs. It leaves legacy rowids (the
implemented global cursor) unchanged and truthfully defaults fields that did
not exist. Diagnostic storage prefixes are backfilled to diagnostic durability
and normalized away on reads. Canonical recorder/store/API replay tests cover
null and populated metadata; malformed JSON, timestamps, typed IDs,
durability, and event-type/payload pairs fail closed. V20 normalizes legacy
approval coordinator event IDs to stable UUIDs without changing rowid cursors
and catalogs `effects_resolved`. PostgreSQL has matching
schema/adapter fields, but live migration/conformance evidence remains pending.
This is metadata fidelity, not authorization: conversation/scope filters do
not enforce a tenant or principal boundary. OBS-01 remains open for trace and
request-context injection, retention, operator recovery, and best-effort event
decisions.

---

## 9. Event stream architecture

### 9.1 In-process channels

**REQ-STREAM-001.** Within a single Polkagent process, events flow through
bounded async channels (e.g., `tokio::sync::broadcast` or
`tokio::sync::mpsc`).

**REQ-STREAM-002.** The in-process event bus has a configurable channel
capacity (default: 1024 events). When the channel is full, backpressure
behavior depends on the durability class:
- Durable events: the producer blocks until space is available. Durable
  events must never be dropped.
- Diagnostic events: the producer drops the oldest undelivered diagnostic
  event (ring buffer semantics).
- Ephemeral events: the producer drops the event immediately.

**REQ-STREAM-003.** The in-process bus supports multiple concurrent
subscribers. Each subscriber receives its own copy of each event.

### 9.2 Cross-process event bus

**REQ-STREAM-010.** When Polkagent runs as multiple processes (e.g., a
daemon plus a CLI tool, or a worker plus a coordinator), cross-process
event delivery uses one of:
- **Shared database polling:** The secondary process polls the `run_events`
  table using a cursor. This is the default and simplest option.
- **Unix domain socket:** A local IPC channel for low-latency delivery
  between co-located processes.
- **Message queue:** For managed cloud deployments, a tenant-scoped message
  queue (e.g., NATS, Redis Streams) distributes events to workers and
  projections.

**REQ-STREAM-011.** The cross-process event bus preserves the same ordering
guarantees as the in-process bus: total order within a run, global order
within a scope.

**REQ-STREAM-012.** Cross-process consumers use the same cursor and
idempotency model as in-process consumers.

### 9.3 WebSocket streaming to surfaces

**REQ-STREAM-020.** Browser, mobile, and desktop surfaces receive events
via WebSocket connections to the Polkagent HTTP API.

**REQ-STREAM-021.** WebSocket event streaming supports:
- Authentication via token in the initial HTTP upgrade request.
- Subscription filters specified in the first message after connection.
- Cursor-based catch-up: the client sends its last known cursor; the server
  replays missed durable events before switching to live mode.
- Heartbeat/ping-pong with a 30-second interval. Three missed pongs trigger
  server-side connection close.
- Graceful shutdown: the server sends a `StreamEnd` control message before
  closing.

**REQ-STREAM-022.** SSE (Server-Sent Events) is supported as a fallback for
environments where WebSocket is unavailable. SSE provides the same
cursor-based catch-up and event filtering, but is unidirectional (server to
client only).

**REQ-STREAM-023.** Both WebSocket and SSE streams encode events as JSON
using the wire format defined in section 18.

**Implementation status (2026-08-06).** The global run-event endpoint
`GET /api/v1alpha1/events/stream` implements the WebSocket portion with
upgrade-time authentication; query-based `after_sequence`, `run_id`, and
`kinds`; live-attach-before-replay; bounded 256-record store pages; global
checkpoint dedupe; 30-second pings; and durable recovery after broadcast lag.
It fails a missing store at the upgrade and sends a sanitized 1011 close if
replay/recovery fails. This does not complete the aspirational first-message
subscription, three-missed-pong, graceful `StreamEnd`, SSE fallback, or
per-class buffer/metrics requirements below. Diagnostic and ephemeral frames
remain best-effort.

The separate command endpoint `GET /ws/v1alpha1` preserves its existing
bidirectional `msg_type` envelope and single-channel subscribe/unsubscribe
commands. It now attaches live before upgrade completion, keeps a per-session
global checkpoint and recovers later durable lag from the same store in
256-record pages without duplicates. Durable event envelopes add an optional
opaque `v1:<global_sequence>` cursor. Reconnect requires a valid query token,
installs all initial subscriptions, and sends an additive `ready` barrier before
replay can advance. `v1:0` starts at retained history, while a nonzero token
must identify an exact retained event; unauthenticated probes perform no store
read, and malformed, stale, and future tokens fail closed. Run and agent
subscriptions work and each connection is capped at 256 distinct channels.
Omitting the cursor preserves legacy live-only behavior. The producer-less
`system` channel is rejected, and diagnostic/ephemeral frames remain
best-effort. This is not completion of per-class buffers, metrics, coalescing,
graceful stream-end, or the broader channel requirements below.

### 9.4 Backpressure handling

**REQ-STREAM-030.** When a WebSocket or SSE client cannot consume events
fast enough:
1. The server buffers up to 256 events per connection.
2. If the buffer fills, ephemeral events are dropped first.
3. If the buffer remains full, diagnostic events are dropped.
4. If the buffer remains full after dropping non-durable events, the
   connection is closed with a `BackpressureExceeded` error. The client
   must reconnect with its last cursor.

**REQ-STREAM-031.** Durable events are never dropped from the authority
store. A slow consumer reconnects and catches up via cursor-based replay.

**REQ-STREAM-032.** Backpressure metrics (dropped events per connection,
reconnection rate, buffer utilization) are exposed in the observability
stack.

### 9.5 Event coalescing

**REQ-STREAM-040.** Ephemeral events of the same type within the same run
may be coalesced during streaming. For example, consecutive `TextDelta`
events may be merged into a single delta covering the combined text.

**REQ-STREAM-041.** Coalescing must preserve the semantic meaning of the
merged events. The coalesced event's timestamp is the timestamp of the
last constituent event.

**REQ-STREAM-042.** Coalescing is a streaming optimization only. It does
not affect persisted events (durable and diagnostic events are stored
individually).

---

## 10. Observability stack

### 10.1 Overview

Polkagent's observability stack provides four complementary signal types:

```text
Metrics   -- aggregated numerical measurements over time
Traces    -- distributed request/effect flows across components
Logs      -- structured diagnostic records with context
Lenses    -- versioned read-only projections for UIs and operators
```

These signals serve different audiences and latency requirements. Metrics are
for dashboards and alerts; traces are for request-level debugging; logs are for
detailed diagnosis; lenses are for user-facing state.

### 10.2 Metrics

#### 10.2.1 What to measure

**REQ-OBS-001.** The following metric families are required:

| Metric family | Example metrics | Type |
|---|---|---|
| **Run lifecycle** | `runs_created_total`, `runs_completed_total`, `runs_failed_total`, `runs_active` | Counter, Gauge |
| **Run duration** | `run_duration_seconds` | Histogram |
| **Effect lifecycle** | `effects_created_total`, `effects_completed_total`, `effects_failed_total`, `effects_retried_total` | Counter |
| **Effect duration** | `effect_duration_seconds` (by effect type) | Histogram |
| **Model usage** | `model_requests_total`, `model_tokens_input_total`, `model_tokens_output_total`, `model_cost_total` | Counter |
| **Model latency** | `model_time_to_first_token_seconds`, `model_request_duration_seconds` | Histogram |
| **Tool usage** | `tool_invocations_total`, `tool_duration_seconds` | Counter, Histogram |
| **Artifact storage** | `artifacts_created_total`, `artifacts_stored_bytes`, `blobs_stored_bytes` | Counter, Gauge |
| **Event throughput** | `events_produced_total`, `events_consumed_total`, `events_dropped_total` | Counter |
| **Stream connections** | `websocket_connections_active`, `sse_connections_active` | Gauge |
| **Database** | `db_queries_total`, `db_query_duration_seconds`, `db_transactions_total`, `db_transaction_duration_seconds` | Counter, Histogram |
| **Approval** | `approvals_requested_total`, `approvals_granted_total`, `approvals_denied_total`, `approval_wait_seconds` | Counter, Histogram |
| **Chain** | `chain_requests_total`, `chain_request_duration_seconds`, `chain_submissions_total`, `finality_wait_seconds` | Counter, Histogram |
| **Policy** | `policy_evaluations_total`, `policy_denials_total` | Counter |
| **Errors** | `errors_total` (by component, severity) | Counter |
| **Retention** | `retention_artifacts_archived_total`, `retention_artifacts_deleted_total`, `retention_bytes_freed_total` | Counter |

#### 10.2.2 Metric types and aggregation

**REQ-OBS-002.** Metrics use the following types:
- **Counter:** monotonically increasing value (e.g., total requests).
- **Gauge:** point-in-time value that can increase or decrease (e.g., active
  connections).
- **Histogram:** distribution of values with configurable bucket boundaries
  (e.g., latency percentiles).

**REQ-OBS-003.** All metrics carry labels for:
- `scope` (tenant/workspace ID for multi-tenant deployments).
- `component` (the subsystem that produced the metric).
- Additional type-specific labels (e.g., `effect_type`, `model_id`,
  `tool_id`, `error_code`).

**REQ-OBS-004.** Label cardinality must be bounded. Metrics must not use
unbounded values (such as user IDs or artifact IDs) as label values.

#### 10.2.3 Metric export

**REQ-OBS-005.** Metrics are exposed via:
- **Prometheus-compatible `/metrics` endpoint:** for scraping by external
  monitoring systems.
- **In-process registry:** for internal projections and health checks.
- **Optional push gateway:** for environments where scraping is not feasible.
- **Optional OTLP (OpenTelemetry Protocol) export:** for managed
  observability platforms.

### 10.3 Traces

#### 10.3.1 Distributed tracing across runs and effects

**REQ-OBS-010.** Polkagent uses distributed tracing to track request flow
across components and effect boundaries. Each trace represents a logical
unit of work (e.g., a run, a chain action saga, or a multi-run workflow).

**REQ-OBS-011.** Trace structure:
- A **trace** has a unique trace ID (128-bit, W3C Trace Context compatible).
- A trace contains one or more **spans**.
- A **span** represents a unit of work within a trace: a run, a turn, an
  effect attempt, a tool invocation, a chain RPC call, or a database query.
- Spans have parent-child relationships forming a tree.

**REQ-OBS-012.** Mandatory spans:
- `run` (root span for a run).
- `turn` (child of run).
- `effect_attempt` (child of turn or run).
- `model_request` (child of turn).
- `tool_invocation` (child of turn).
- `chain_rpc` (child of effect_attempt).
- `signer_request` (child of effect_attempt).

**REQ-OBS-013.** Span attributes include:
- `run_id`, `conversation_id`, `scope`.
- `effect_type`, `effect_intent_id`, `attempt_number`.
- `model_id`, `provider_id`.
- `tool_id`, `grant_digest`.
- `chain_profile`, `rpc_method`.
- `status` (ok, error), `error_code`, `error_message` (redacted).
- `duration_ms`.

**REQ-OBS-014.** Traces must not include:
- Secret material (API keys, signing keys, session tokens).
- Full model prompts or responses (these are artifacts with their own
  classification).
- Raw user content classified as `Sensitive` or `SecretForbidden`.

**REQ-OBS-015.** Trace export uses OpenTelemetry SDK with configurable
exporters: Jaeger, Zipkin, OTLP, or console (for local development). The
implementation uses the Rust `tracing` crate as the instrumentation layer,
with `tracing-opentelemetry` bridging to the OpenTelemetry SDK. One trace
is created per turn; all LLM, tool, chain-submit, and finality spans are
children of the turn span. (Research basis: research3.md finding 22 [I].)

**REQ-OBS-015a.** **Token and cost tracking.** Every `model_request` span
must record:
- `tokens.input`: number of input tokens consumed.
- `tokens.output`: number of output tokens produced.
- `tokens.total`: sum of input and output tokens.
- `cost.estimated_usd`: estimated cost in USD (may be null if pricing is
  unknown for the provider/model combination).
- `model.id` and `provider.id` as span attributes.

These values are also propagated to the `UsageLens` (section 10.5) for
aggregated cost/usage dashboards.

**REQ-OBS-015b.** **Multi-agent trace propagation.** When a Polkagent
instance spawns or delegates to another agent (sub-agent, remote tool, or
A2A-delegated agent), the W3C Trace Context (`traceparent`/`tracestate`
headers or equivalent) is propagated so that the full multi-agent action
appears as a single distributed trace. Incoming trace context from parent
agents is accepted and used as the parent span, subject to the same
redaction rules that apply to all spans.

#### 10.3.2 Trace-event correlation

**REQ-OBS-016.** Every durable event includes the active trace ID and span
ID at the time of emission. This allows navigating from an event to its
containing trace and vice versa.

**REQ-OBS-017.** The correlation ID in events and the trace ID in spans
use the same value when a run maps 1:1 to a trace. When a trace spans
multiple runs (e.g., a workflow), the correlation ID links related runs
within the trace.

### 10.4 Logs

#### 10.4.1 Structured logging

**REQ-OBS-020.** All log output uses structured JSON format with the
following required fields:

```json
{
  "timestamp": "2026-07-30T12:34:56.789Z",
  "level": "info",
  "target": "polkagent::runtime::effect_driver",
  "message": "Effect attempt completed",
  "run_id": "01J5ABC...",
  "correlation_id": "01J5XYZ...",
  "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
  "span_id": "00f067aa0ba902b7",
  "scope": "tenant-123",
  "component": "effect_driver",
  "fields": {
    "effect_type": "broadcast",
    "attempt_number": 1,
    "outcome": "success",
    "duration_ms": 342
  }
}
```

#### 10.4.2 Log levels

| Level | When to use | Retention |
|---|---|---|
| `error` | Unrecoverable failures, integrity violations, security events | 90 days minimum |
| `warn` | Recoverable problems, policy denials, retries, degraded operation | 30 days |
| `info` | Significant lifecycle transitions, completed effects, configuration changes | 14 days |
| `debug` | Detailed execution flow, adapter-level communication | 7 days |
| `trace` | Per-token, per-byte, per-query detail | 1 day |

**REQ-OBS-021.** The default log level is `info`. It is configurable at
runtime per component.

**REQ-OBS-022.** Log output destinations:
- **Local:** structured JSON to file with rotation (default: 100 MB per file,
  10 files retained).
- **Console:** human-readable format for development and CLI usage.
- **Managed:** forwarded to the configured log aggregation service (e.g.,
  Loki, CloudWatch, Datadog).

#### 10.4.3 Correlation IDs and context propagation

**REQ-OBS-023.** Every log entry within a run includes the `run_id` and
`correlation_id`. Every log entry within an effect attempt additionally
includes the `effect_intent_id` and `attempt_number`.

**REQ-OBS-024.** Context propagation uses `tracing` crate spans in Rust.
Entering a span automatically attaches its fields to all log entries,
metrics, and child spans within it.

#### 10.4.4 Log redaction

**REQ-OBS-025.** Logs must never contain:
- Secret material (API keys, signing keys, seed phrases, session tokens).
- Full model prompts or responses.
- User content classified as `Sensitive` or `SecretForbidden`.
- Raw chain account private keys or mnemonic phrases.
- Raw calldata parameters that may contain sensitive values (amounts, addresses
  of sensitive accounts, or governance payloads above a configured sensitivity
  threshold).

**REQ-OBS-025a.** Redaction is applied **at the tracing-layer boundary** —
that is, before any field enters the `tracing` span or log record, not as a
post-processing step on output. Adapter code that handles secrets must pass
opaque handles or redacted placeholders to the tracing layer; it must never
pass raw key material or raw sensitive calldata even if the log level is
below the current threshold. This ensures that no configuration change (e.g.,
enabling `trace`-level logging) can inadvertently expose secrets. (Research
basis: research3.md finding [V] — structured logging with secret redaction at
the layer boundary.)

**REQ-OBS-026.** When a log entry references sensitive data, it must use:
- An artifact ID (the reader can look up the artifact with appropriate
  authorization).
- A redacted placeholder (e.g., `"api_key": "[REDACTED]"`).
- A hash or digest of the value (for correlation without exposure).

### 10.5 Lenses (projections)

#### 10.5.1 Concept

A **Lens** is a named, versioned, read-only projection of durable state
designed for a specific consumer. Lenses are the primary interface between the
durable data layer and UIs, CLIs, APIs, and operator tools. They are inspired
by Roko's Lens/StateHub pattern but adapted for Polkagent's domain.

```text
Durable events + artifacts
    |
    v
Projection engine (processes events via cursor)
    |
    v
Lens tables (denormalized, queryable, recoverable)
    |
    v
API / WebSocket / CLI / Dashboard
```

#### 10.5.2 Required lenses

**REQ-OBS-030.** The following lenses are required:

| Lens | Schema | Primary consumer | Refresh trigger |
|---|---|---|---|
| `RunStatusLens` | Run ID, status, progress, duration, outcome, error summary | CLI, Web UI, Inbox | Every durable run event |
| `ConversationLens` | Conversation ID, last activity, message count, active runs | Chat UI, Inbox | Turn/message events |
| `EffectStatusLens` | Effect intent ID, status, attempts, last outcome, next retry | Operator dashboard, approval UI | Every effect event |
| `ApprovalLens` | Approval request ID, status, requestor, action summary, deadline | Inbox, mobile, approval UI | Approval events |
| `ChainActionLens` | Intent ID, saga stage, chain/network, decoded call summary, finality status | Action card, timeline | Chain-related events |
| `ArtifactIndexLens` | Kind, run ID, classification, created at, size, labels | Artifact browser, search | Every artifact creation |
| `UsageLens` | Model/provider, tokens in/out, cost, request count, period | Usage dashboard, billing | Model effect outcomes |
| `HealthLens` | Component, status, last heartbeat, error count, latency p99 | Operator dashboard, alerting | System events, heartbeats |
| `PolicyAuditLens` | Run ID, policy revision, grant digest, decision, reason | Audit log, compliance | Policy/approval events |

#### 10.5.3 Lens contracts

**REQ-OBS-031.** Every lens specifies:
- A name and version.
- The set of event types it consumes.
- Its cursor position (the `global_sequence` of the last processed event).
- Its computed timestamp (when the lens was last refreshed).
- Its freshness guarantee (maximum acceptable lag behind the event stream).
- Its recovery procedure (how to rebuild from scratch).

**REQ-OBS-032.** A lens is **recoverable**: it can be entirely rebuilt by
replaying all relevant durable events from the beginning. This is used
during:
- Schema migration (drop and rebuild the lens).
- Corruption recovery.
- New lens deployment (backfill from existing events).

**REQ-OBS-033.** Lenses are eventually consistent with the durable event
stream. The maximum acceptable lag is configurable per lens (default:
1 second for interactive lenses, 60 seconds for analytics lenses).

**REQ-OBS-034.** Lens tables are stored in the same database as the
authority store but in a separate schema/prefix. They may be dropped and
rebuilt without affecting the authority store.

**REQ-OBS-035.** Lenses must not be the source of truth for any decision.
They are read-only derivations. If a lens and the authority store disagree,
the authority store is correct and the lens must be rebuilt.

---

## 11. Evidence-bearing effects (E1)

### 11.1 Design intent

Evidence-bearing effects are a kernel-level architectural pattern: every
material effect records its canonical inputs, policy authorization, attempts,
and outcome as linked artifacts. The purpose is to turn "trust the agent"
into inspectable, auditable, reproducible evidence.

This is the E1 candidate from the opportunity catalog (`reserach/research1.md`):

> Each material effect records canonical inputs, policy, attempts, and
> outcome links. Turns "trust the agent" into inspectable evidence.

### 11.2 Evidence chain structure

For a chain action, the evidence chain looks like:

```text
UserRequest (artifact)
    |
    v
ActionIntent (artifact)
    |-> parents: [UserRequest]
    |
    v
MetadataSnapshot (artifact)
    |-> parents: [ActionIntent]
    |-> chain_evidence: { genesis_hash, spec_version, block_hash, metadata_hash }
    |
    v
DecodedCall (artifact)
    |-> parents: [ActionIntent, MetadataSnapshot]
    |
    v
SimulationResult (artifact)
    |-> parents: [DecodedCall, MetadataSnapshot]
    |
    v
PolicySnapshot (artifact)
    |-> parents: [DecodedCall, SimulationResult]
    |-> content: serialized ResolvedGrant with policy revision digest
    |
    v
ApprovalRecord (artifact)
    |-> parents: [PolicySnapshot, DecodedCall]
    |-> content: approver, timestamp, bound payload hash
    |
    v
SignedPayload (artifact)
    |-> parents: [ApprovalRecord, DecodedCall, MetadataSnapshot]
    |
    v
BroadcastReceipt (artifact)
    |-> parents: [SignedPayload]
    |
    v
FinalityObservation (artifact)
    |-> parents: [BroadcastReceipt]
    |
    v
Receipt (artifact)
    |-> parents: [all of the above]
    |-> content: composite summary linking the full chain
```

### 11.3 Evidence requirements

**REQ-E1-001.** Every effect intent must be stored as a durable record
before the corresponding I/O is attempted. This is the "evidence before
action" invariant.

**REQ-E1-002.** Every effect attempt must record:
- The intent it claims.
- A stable idempotency key.
- A lease expiry (after which the attempt is considered abandoned).
- The worker ID that claimed it.
- The attempt number (monotonically increasing per intent).

**REQ-E1-003.** Every effect outcome must record:
- The attempt it resolves.
- The outcome status: `Success`, `Failure`, `Timeout`, `Cancelled`, or
  `Unknown`.
- Typed result or error evidence as an artifact reference.
- External references (e.g., transaction hash, block hash).
- The wall-clock duration of the attempt.

**REQ-E1-004.** An effect outcome of `Unknown` is a legitimate terminal
state. It means the I/O was performed but the result could not be
determined (e.g., an RPC call that timed out after submission). The system
must not silently convert `Unknown` to `Failure` or `Success`.

**REQ-E1-005.** For chain actions, the evidence chain in section 11.2 is
mandatory. Skipping a step (e.g., signing without a metadata snapshot) is
a policy violation and must be rejected by the saga coordinator.

**REQ-E1-006.** Evidence artifacts must be queryable by:
- Run ID (all evidence for a run).
- Effect intent ID (all evidence for an effect).
- Lineage traversal (ancestors and descendants of any artifact).
- Chain evidence fields (genesis hash, spec version, block hash).

### 11.4 Evidence reconstruction

**REQ-E1-010.** Given a receipt artifact, an operator must be able to
reconstruct the complete evidence chain without access to in-memory state,
live streams, or external services. The reconstruction uses only the
authority store.

**REQ-E1-011.** An evidence reconstruction report includes:
- Each artifact in the chain with its digest, provenance, and classification.
- Each event in the run's durable event stream.
- Each effect intent, attempt, and outcome.
- The resolved grant and policy revision that authorized the action.
- Integrity verification status for each artifact.

---

## 12. Replay and debugging (E4)

### 12.1 Design intent

Replay is the ability to re-execute the deterministic parts of a run against
pinned inputs, without re-performing external side effects. It is the E4
candidate from the opportunity catalog:

> Operator replays deterministic reductions against pinned inputs, not
> external side effects. Better incident analysis.

### 12.2 What is replayable

**REQ-E4-001.** The `TurnReducer` function is deterministic:

```rust
fn reduce(state: TurnState, input: TurnInput) -> (TurnState, Vec<EffectIntent>);
```

Given the same `state` and `input`, it must produce the same output. This
function is replayable.

**REQ-E4-002.** External effects (model calls, tool invocations, chain RPCs,
signer requests) are NOT replayable. Their outputs are non-deterministic.
During replay, their recorded `EffectOutcome` artifacts are substituted.

### 12.3 Replay procedure

**REQ-E4-010.** To replay a run:
1. Load the initial `TurnState` from the run's creation event.
2. For each `TurnInput` in the run's durable event stream:
   a. Apply `reduce(state, input)` to produce `(new_state, intents)`.
   b. Compare the produced `intents` with the recorded `EffectIntent`s.
   c. Substitute the recorded `EffectOutcome` for each intent.
   d. Feed the outcome as the next input to the reducer.
3. Compare the final state with the recorded terminal state.

**REQ-E4-011.** Replay declares nondeterminism explicitly. When a replayed
reduction produces different `EffectIntent`s than the recorded ones, the
replay report flags the divergence with:
- The input that caused the divergence.
- The expected and actual intents.
- The state before and after the divergent reduction.

**REQ-E4-012.** Replay must never send an effect to an external system. It
operates entirely against recorded data.

**REQ-E4-013.** Replay must never modify the authority store. Replay output
is a separate report artifact.

### 12.4 Replay modes

| Mode | Purpose | Inputs | Output |
|---|---|---|---|
| **Verification** | Confirm that the recorded run is consistent | Recorded events, artifacts, and outcomes | Pass/fail with divergence report |
| **Debugging** | Step through a run interactively | Recorded data + breakpoint conditions | Step-by-step state transitions |
| **What-if** | Explore alternative paths | Recorded data with modified inputs/outcomes | Alternative state and intent sequences |

### 12.5 Replay limitations

**REQ-E4-020.** Replay cannot guarantee determinism when:
- The reducer depends on wall-clock time (use the recorded timestamps).
- The reducer depends on external state not captured in artifacts.
- Model responses differ between the original and replay (this is expected;
  model responses are substituted, not re-executed).

**REQ-E4-021.** The replay report must state which nondeterministic inputs
were substituted and which deterministic computations were verified.

---

## 13. Optional Bulletin/IPFS anchoring (E2)

### 13.1 Design intent

Bulletin/IPFS anchoring is an experimental adapter that allows users to
optionally anchor artifact digests on Polkadot's Bulletin storage or IPFS.
It is the E2 candidate from the opportunity catalog:

> User chooses to anchor a classified artifact digest/CID after local
> storage. Portable public proof where appropriate.

### 13.2 Anchoring model

**REQ-E2-001.** Anchoring is opt-in per artifact. Users explicitly choose
which artifacts to anchor. Anchoring is never automatic by default.

**REQ-E2-002.** Only the content digest (SHA-256 hash, or an IPFS CID derived
from it for IPFS/Bulletin targets) of an artifact is anchored, not the
artifact content. The content remains in local/managed storage under its
classification and retention policies. When an anchor is placed on the
Bulletin Chain, the event that records the anchoring must include the CID
as a structured field in its payload so that the anchor can be independently
verified from the event store. (Research basis: research3.md finding [V] —
Bulletin/IPFS anchoring for evidence chains, CID in event.)

**REQ-E2-003.** Anchoring produces an `AnchorRecord` artifact:

```rust
pub struct AnchorRecord {
    /// The artifact whose digest was anchored.
    pub artifact_id: ArtifactId,

    /// The content digest that was anchored.
    pub content_digest: Sha256Digest,

    /// Where the anchor was placed.
    pub anchor_target: AnchorTarget,

    /// The anchor's reference (e.g., statement hash, IPFS CID).
    pub anchor_ref: String,

    /// Timestamp of anchoring.
    pub anchored_at: Timestamp,

    /// The account that performed the anchoring, if on-chain.
    pub anchor_account: Option<AccountId>,

    /// Cost of the anchoring operation.
    pub cost: Option<Cost>,
}

pub enum AnchorTarget {
    /// Polkadot Bulletin/Statement Store.
    Bulletin { chain: ChainProfileRef },
    /// IPFS content addressing.
    Ipfs { gateway: String },
    /// Custom anchoring target.
    Custom { target_id: String },
}
```

### 13.3 Safety constraints

**REQ-E2-010.** Artifacts classified as `Sensitive` or `SecretForbidden`
must not be anchored. Attempting to anchor such an artifact is a policy
violation.

**REQ-E2-011.** Anchoring a digest does not make the content public. The
digest alone does not reveal the content. However, if the content is
independently known, the digest confirms its existence and integrity.

**REQ-E2-012.** Local evidence remains authoritative. The anchored digest
is supplementary proof. If the local artifact is deleted, the anchor
remains but cannot reconstruct the content.

**REQ-E2-013.** Bulletin anchoring requires an on-chain account with
sufficient balance. The anchoring effect uses the standard effect intent/
attempt/outcome lifecycle with policy evaluation and optional approval.

### 13.4 Maturity

**REQ-E2-020.** Bulletin/IPFS anchoring is an **experimental** feature.
It must be:
- Behind an explicit feature flag.
- Labeled as experimental in all UIs.
- Not depended upon by core correctness or recovery.
- Subject to Bulletin availability, retention, and cost constraints
  verified per target network profile.

**REQ-E2-021.** The system must communicate clearly to users that the Bulletin
Chain currently operates with approximately **two-week retention** for stored
data (as of July 2026; testnet parameters, subject to change). Anchoring on
the Bulletin Chain is therefore suitable as a **supplementary, time-limited
proof** — not as a long-term archive. The local artifact store remains the
authoritative durable record. Any UX that surfaces Bulletin-anchored evidence
must include a disclosure that Bulletin retention is limited and the anchor
may no longer be resolvable after the retention window. (Research basis:
research3.md finding 19 [V] — Bulletin Chain testnet, ~2-week retention;
avoid section: "avoid: relying on Bulletin durability.")

---

## 14. Live run timeline (F3)

### 14.1 Design intent

The live run timeline is a real-time status projection that shows users
the current state of a run as it progresses. It is the F3 candidate from
the opportunity catalog:

> User sees received -> queued -> working -> approval -> outcome, with
> durable status separate from animation.

### 14.2 Timeline states

The timeline presents a run as a sequence of **phases**, each with a
status indicator:

```text
Received -> Queued -> Working -> [Approval] -> Completing -> Done
                                                              |
                                                    [Failed | Cancelled | TimedOut]
```

| Phase | When entered | When exited | Visual indicator |
|---|---|---|---|
| `Received` | Message/trigger admitted | Run created | Checkmark |
| `Queued` | Run created, waiting for executor | Execution begins | Clock/hourglass |
| `Working` | Executor/harness is active | Terminal or approval event | Spinner with progress |
| `Approval` | An action requires authorization | Approval granted/denied/expired | Attention indicator |
| `Completing` | Post-approval effects in progress | All effects resolved | Spinner |
| `Done` | All effects resolved successfully | N/A (terminal) | Success indicator |
| `Failed` | A terminal failure occurred | N/A (terminal) | Error indicator |
| `Cancelled` | User or policy cancelled the run | N/A (terminal) | Cancel indicator |
| `TimedOut` | Run exceeded its deadline | N/A (terminal) | Timeout indicator |

### 14.3 Timeline data model

```rust
/// The timeline projection for a single run.
pub struct RunTimeline {
    pub run_id: RunId,
    pub conversation_id: Option<ConversationId>,
    pub phases: Vec<TimelinePhase>,
    pub current_phase: TimelinePhaseType,
    pub started_at: Timestamp,
    pub completed_at: Option<Timestamp>,
    pub duration: Option<Duration>,
    pub progress: Option<Progress>,
    pub sub_timelines: Vec<EffectTimeline>,
}

pub struct TimelinePhase {
    pub phase_type: TimelinePhaseType,
    pub status: PhaseStatus,
    pub entered_at: Timestamp,
    pub exited_at: Option<Timestamp>,
    pub detail: Option<String>,
}

pub enum PhaseStatus {
    Pending,
    Active,
    Completed,
    Failed { error_summary: String },
    Skipped,
}

pub struct Progress {
    pub percent: Option<f32>,
    pub stage: Option<String>,
    pub sub_stage: Option<String>,
}

/// Timeline for a single effect within a run.
pub struct EffectTimeline {
    pub effect_intent_id: EffectIntentId,
    pub effect_type: String,
    pub status: EffectTimelineStatus,
    pub attempts: Vec<AttemptSummary>,
}
```

### 14.4 Timeline requirements

**REQ-F3-001.** The timeline is derived from the `RunStatusLens` and
`EffectStatusLens` projections, not from in-memory state. A restart or
reconnect recovers the truthful timeline from durable records.

**REQ-F3-002.** Timeline updates are streamed to connected surfaces via
the event streaming infrastructure (section 9). The timeline is a
projection of durable events, not a separate mutable state.

**REQ-F3-003.** Durable status (the actual phase and outcome) is always
separate from animation (spinners, progress bars, typing indicators).
A surface that loses its WebSocket connection must display the last known
durable status, not a stale animation.

**REQ-F3-004.** The timeline must truthfully represent `Unknown` outcomes.
When an effect's outcome is unknown (e.g., a broadcast was attempted but
the result is not yet determined), the timeline must show "Pending" or
"Unknown" rather than "Success" or "Failed".

**REQ-F3-005.** Sub-timelines for individual effects within a run provide
detailed progress for chain action sagas, showing each stage (decode,
simulate, approve, sign, broadcast, finality) independently.

---

## 15. Recovery mechanisms

### 15.1 Crash recovery

**REQ-REC-001.** After a crash, Polkagent recovers by:
1. Opening the SQLite database. WAL replay recovers uncommitted but written
   pages automatically.
2. Scanning for in-progress runs (`RunStarted` without a terminal event).
3. For each in-progress run, scanning for in-flight effect attempts
   (`EffectAttemptStarted` without `EffectAttemptCompleted`).
4. Expired leases are marked as `LeaseExpired`. The run's reducer receives
   a timeout/lease-expired input and decides the next action.
5. Unclaimed effect intents are eligible for re-claiming by workers.
6. Projection lenses are refreshed from their last cursor position.

**REQ-REC-002.** Crash recovery must never duplicate an external effect.
The idempotency key and lease mechanism ensure that:
- An effect whose outcome is recorded is not re-attempted.
- An effect whose lease has expired may be re-claimed with a new attempt
  (and new attempt number), but uses the same idempotency key for
  providers that support it.
- An effect whose lease has not expired is not re-claimed (another worker
  may still be processing it).

This crash-safety guarantee depends on the **transactional outbox pattern**:
the domain event and the outbox row for each effect intent are written in
the same SQLite transaction as the state change that produced them. A crash
between writing the outbox row and executing the external I/O leaves the
outbox row unconsumed, which is detected on recovery and triggers a retry
with the same idempotency key. A crash after external I/O but before outcome
recording results in a re-attempt that must be deduplicated by the idempotency
key at the external system. (Research basis: research3.md finding [I] —
crash recovery via outbox + idempotency.)

**REQ-REC-003.** Crash recovery completes within 30 seconds for databases
up to 10 GB. Recovery time is proportional to the number of in-progress
runs, not the total database size.

**REQ-REC-004.** A recovery report is logged at `info` level, summarizing:
- Number of in-progress runs found.
- Number of expired leases.
- Number of effect intents re-queued.
- Time taken for recovery.

### 15.2 Backup and restore

#### 15.2.1 Backup

**REQ-REC-010.** Local deployments support two backup methods:
- **SQLite online backup:** Uses SQLite's backup API (`sqlite3_backup_init` /
  `sqlite3_backup_step` / `sqlite3_backup_finish`) to create a consistent
  snapshot of the database while the system is running. The backup API
  copies pages under a shared lock, allowing concurrent reads and writes
  to continue. Blob files must be copied in a **coordinated** manner: the
  blob copy begins after the database snapshot completes, and the backup
  manifest records the `global_sequence` of the last committed event at
  snapshot time. Any blobs referenced by events with sequence numbers up
  to that watermark are included; blobs written after the watermark are
  not required for consistency of the snapshot. (Research basis:
  research3.md finding [I] — coordinated DB+blob backup.)
- **Filesystem snapshot:** Stop the daemon, snapshot the entire
  `$POLKAGENT_DATA_DIR`, restart. This is the simplest method for
  environments with filesystem snapshot support (ZFS, LVM, cloud volumes).

**REQ-REC-011.** Managed deployments use:
- PostgreSQL logical or physical backups managed by the cloud platform.
- Object store versioning and replication for blob storage.
- Backup schedules and retention are operator-configurable.

**REQ-REC-012.** Backup artifacts include:
- The authority database (SQLite file or PostgreSQL dump).
- All blob files referenced by non-expired artifacts.
- The configuration file (excluding secrets, which are in the secret store).
- A manifest file listing the backup contents, timestamp, software version,
  schema version, and integrity digests.

**REQ-REC-013.** Backup does not include:
- Ephemeral events (never persisted).
- Projection/lens tables (they are rebuilt from events).
- Temporary files.
- Secret material (backed up separately through the secret store).

#### 15.2.2 Restore

**REQ-REC-020.** Restore from backup:
1. Verify the backup manifest's integrity digest.
2. Stop the running daemon.
3. Replace the database and blob files with the backup contents.
4. Start the daemon. It runs crash recovery (section 15.1).
5. Lenses are rebuilt from events (this may take time for large databases).
6. A `BackupRestored` system event is logged.

**REQ-REC-021.** Restore must handle version mismatches:
- If the backup is from an older schema version, run forward migrations
  before starting.
- If the backup is from a newer schema version, reject the restore with a
  clear error.

**REQ-REC-022.** Restore does not replay external effects. Runs that were
in-progress at the time of backup are recovered per section 15.1.

### 15.3 State export and import

**REQ-REC-030.** Export produces a portable `ExportPackage` artifact
containing:
- A versioned manifest with format version, timestamp, scope, and filters.
- Selected artifact metadata and bodies.
- Selected durable events.
- Selected effect intents, attempts, and outcomes.
- Configuration (excluding secrets).

**REQ-REC-031.** Export supports filters:
- By time range.
- By run ID or conversation ID.
- By artifact kind.
- By classification level (exports may exclude sensitive data).

**REQ-REC-032.** Import reads an `ExportPackage` and:
1. Validates the manifest and verifies artifact digests.
2. Creates new artifact IDs (preserving original IDs as provenance metadata).
3. Inserts artifacts, events, and effects into the target store.
4. Rebuilds affected lenses.
5. Produces an import report artifact.

**REQ-REC-033.** Import is idempotent: importing the same package twice does
not create duplicate records. Deduplication uses content digests.

### 15.4 Point-in-time recovery

**REQ-REC-040.** Point-in-time recovery allows restoring the system state
to a specific moment by:
1. Restoring from the most recent backup before the target time.
2. Replaying durable events from the backup's last `global_sequence` up to
   the target `global_sequence`.
3. Rebuilding lenses to the target position.

**REQ-REC-041.** Point-in-time recovery requires that the WAL or a
continuous event archive is available between the backup and the target
time. Managed deployments achieve this through PostgreSQL point-in-time
recovery (WAL archiving). Local deployments achieve this through periodic
backups with bounded recovery point objective.

**REQ-REC-042.** The recovery point objective (RPO) for local deployments
is configurable. Default: the interval between automated backups (e.g.,
daily). Managed deployments target continuous RPO (zero data loss for
committed transactions).

### 15.5 Disaster recovery

**REQ-REC-050.** Disaster recovery addresses complete loss of the primary
data store (hardware failure, data center loss, accidental deletion).

**REQ-REC-051.** Disaster recovery procedure:
1. Provision new infrastructure.
2. Restore from the most recent off-site backup.
3. Run point-in-time recovery if continuous WAL/event archives are available.
4. Verify integrity of restored artifacts.
5. Resume operation. In-progress runs recover per section 15.1.

**REQ-REC-052.** Recovery time objective (RTO):
- Local deployments: depends on backup availability and restoration speed.
  No SLO is imposed; operators manage their own disaster recovery.
- Managed deployments: target RTO of 1 hour. This includes infrastructure
  provisioning, backup restoration, and lens rebuilding.

**REQ-REC-053.** Disaster recovery must be tested quarterly in managed
deployments with documented results.

---

## 16. Data classification

### 16.1 Classification levels

**REQ-CLASS-001.** Every artifact, event, and log entry carries a
classification level:

| Level | Description | Example content | Handling rules |
|---|---|---|---|
| `Public` | Intended for public visibility | Published agent card, open-source skill manifest | No access restriction; may be cached, indexed, and exported freely |
| `Internal` | Visible within the Polkagent instance | Run status, effect metadata, usage metrics | Accessible to authenticated principals within the scope; not exposed externally without explicit export |
| `Private` | Visible only to the owning user/tenant | Conversation content, model responses, decoded calls, personal artifacts | Encrypted at rest when application-level encryption is enabled; tenant-isolated; requires authorization for access |
| `Sensitive` | Requires heightened protection | Approval records with financial details, signed payloads, account bindings, PII | Always encrypted at rest; access-logged; excluded from model context by default; export requires explicit consent |
| `SecretForbidden` | Must never be stored as artifact content | Signing keys, seed phrases, API tokens, session secrets | Never stored in artifacts, events, logs, or projections. References use handles from the secret store. If detected in an artifact body, the artifact is quarantined and an `IntegrityViolation` event is emitted. |

### 16.2 Classification rules

**REQ-CLASS-010.** Default classification is assigned based on artifact kind
(see section 5.1). The producer may escalate (increase restriction) but
never relax below the kind's default.

**REQ-CLASS-011.** Inherited classification: when an artifact is derived from
parents, its classification is at least as restrictive as the most
restrictive parent. This is computed automatically during artifact creation.

**REQ-CLASS-012.** Classification is immutable after storage. To change an
artifact's classification, create a new version with the updated level.

**REQ-CLASS-013.** Classification enforcement:
- The `ArtifactStore` rejects writes that violate kind-default classification.
- The `ContextAssembler` excludes artifacts above the model's clearance level.
- The `EventSink` redacts sensitive fields from events before streaming to
  unauthorized consumers.
- The log subsystem applies redaction rules based on classification.
- Projections/lenses inherit the most restrictive classification of their
  source events and artifacts.

### 16.3 Secret detection

**REQ-CLASS-020.** A configurable secret-detection scanner runs on artifact
bodies at creation time. It checks for patterns indicative of:
- Private keys (hex-encoded 32/64 byte sequences with key prefixes).
- Mnemonic phrases (BIP-39 word sequences).
- API keys and tokens (known provider patterns).
- Connection strings with embedded credentials.

**REQ-CLASS-021.** If secret material is detected in an artifact body:
1. The artifact is quarantined (stored but marked `integrity_violation`).
2. An `IntegrityViolation` event is emitted.
3. The artifact is excluded from projections and model context.
4. An operator notification is sent.

**REQ-CLASS-022.** Secret detection is best-effort. It cannot guarantee
detection of all secret formats. Classification enforcement
(section 16.2) and input validation at adapter boundaries provide
defense in depth.

---

## 17. Database schema sketches

### 17.1 Artifact tables

```sql
-- Core artifact metadata
CREATE TABLE artifacts (
    id              TEXT PRIMARY KEY,       -- ULID
    kind            TEXT NOT NULL,          -- ArtifactKind enum
    content_digest  TEXT NOT NULL,          -- SHA-256 hex
    content_size    INTEGER NOT NULL,       -- bytes
    content_type    TEXT,                   -- MIME type
    classification  TEXT NOT NULL,          -- Classification enum
    run_id          TEXT,                   -- FK to runs
    effect_attempt_id TEXT,                -- FK to effect_attempts
    conversation_id TEXT,                  -- FK to conversations
    scope_id        TEXT NOT NULL,          -- tenant/workspace
    created_at      TEXT NOT NULL,          -- ISO 8601
    expires_at      TEXT,                   -- ISO 8601
    deleted_at      TEXT,                   -- soft-delete timestamp
    storage_tier    TEXT NOT NULL DEFAULT 'hot',  -- hot | cold
    schema_version  INTEGER NOT NULL DEFAULT 1,
    FOREIGN KEY (run_id) REFERENCES runs(id),
    FOREIGN KEY (conversation_id) REFERENCES conversations(id)
);

CREATE INDEX idx_artifacts_run ON artifacts(run_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_artifacts_kind ON artifacts(kind, created_at) WHERE deleted_at IS NULL;
CREATE INDEX idx_artifacts_scope ON artifacts(scope_id, created_at) WHERE deleted_at IS NULL;
CREATE INDEX idx_artifacts_digest ON artifacts(content_digest);
CREATE INDEX idx_artifacts_expires ON artifacts(expires_at) WHERE expires_at IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX idx_artifacts_conversation ON artifacts(conversation_id) WHERE deleted_at IS NULL;

-- Artifact lineage (parent-child edges)
CREATE TABLE artifact_parents (
    child_id   TEXT NOT NULL,              -- FK to artifacts
    parent_id  TEXT NOT NULL,              -- FK to artifacts
    PRIMARY KEY (child_id, parent_id),
    FOREIGN KEY (child_id) REFERENCES artifacts(id),
    FOREIGN KEY (parent_id) REFERENCES artifacts(id)
);

CREATE INDEX idx_artifact_parents_parent ON artifact_parents(parent_id);

-- Artifact provenance
CREATE TABLE artifact_provenance (
    artifact_id       TEXT PRIMARY KEY,     -- FK to artifacts
    source_type       TEXT NOT NULL,        -- system | model | user | external | derived
    software_version  TEXT NOT NULL,
    produced_at       TEXT NOT NULL,        -- ISO 8601
    principal_id      TEXT,
    genesis_hash      TEXT,                -- chain evidence (nullable)
    spec_version      INTEGER,
    at_block_hash     TEXT,
    at_block_number   INTEGER,
    metadata_hash     TEXT,
    FOREIGN KEY (artifact_id) REFERENCES artifacts(id)
);

-- Artifact labels (key-value pairs for filtering)
CREATE TABLE artifact_labels (
    artifact_id  TEXT NOT NULL,
    key          TEXT NOT NULL,
    value        TEXT NOT NULL,
    PRIMARY KEY (artifact_id, key),
    FOREIGN KEY (artifact_id) REFERENCES artifacts(id)
);

-- Inline artifact bodies (for artifacts <= 256 KB)
CREATE TABLE artifact_bodies (
    artifact_id  TEXT PRIMARY KEY,
    body         BLOB NOT NULL,
    FOREIGN KEY (artifact_id) REFERENCES artifacts(id)
);
```

### 17.2 Event tables

```sql
-- Durable run events
CREATE TABLE run_events (
    id               TEXT PRIMARY KEY,      -- ULID
    event_type       TEXT NOT NULL,
    sequence         INTEGER NOT NULL,      -- per-run monotonic
    global_sequence  INTEGER NOT NULL,      -- auto-increment across all runs
    run_id           TEXT NOT NULL,
    turn_id          TEXT,
    step_id          TEXT,
    effect_intent_id TEXT,
    effect_attempt_id TEXT,
    conversation_id  TEXT,
    correlation_id   TEXT NOT NULL,
    causation_id     TEXT,                  -- FK to run_events
    scope_id         TEXT NOT NULL,
    timestamp        TEXT NOT NULL,         -- ISO 8601
    durability       TEXT NOT NULL,         -- Durable | Diagnostic
    payload          TEXT NOT NULL,         -- JSON-encoded EventPayload
    trace_id         TEXT,                  -- W3C trace ID
    span_id          TEXT,                  -- W3C span ID
    schema_version   INTEGER NOT NULL DEFAULT 1,
    FOREIGN KEY (run_id) REFERENCES runs(id),
    UNIQUE (run_id, sequence)
);

CREATE INDEX idx_events_run ON run_events(run_id, sequence);
CREATE INDEX idx_events_global ON run_events(global_sequence);
CREATE INDEX idx_events_correlation ON run_events(correlation_id);
CREATE INDEX idx_events_scope_type ON run_events(scope_id, event_type, global_sequence);
CREATE INDEX idx_events_conversation ON run_events(conversation_id, global_sequence)
    WHERE conversation_id IS NOT NULL;

-- Event-artifact references
CREATE TABLE event_artifact_refs (
    event_id     TEXT NOT NULL,
    artifact_id  TEXT NOT NULL,
    PRIMARY KEY (event_id, artifact_id),
    FOREIGN KEY (event_id) REFERENCES run_events(id),
    FOREIGN KEY (artifact_id) REFERENCES artifacts(id)
);

-- Diagnostic events (separate table, shorter retention)
CREATE TABLE diagnostic_events (
    id               TEXT PRIMARY KEY,
    event_type       TEXT NOT NULL,
    run_id           TEXT NOT NULL,
    scope_id         TEXT NOT NULL,
    timestamp        TEXT NOT NULL,
    payload          TEXT NOT NULL,
    expires_at       TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES runs(id)
);

CREATE INDEX idx_diag_events_expires ON diagnostic_events(expires_at);
CREATE INDEX idx_diag_events_run ON diagnostic_events(run_id);
```

### 17.3 Effect tables

```sql
-- Effect intents (durable commands for external work)
CREATE TABLE effect_intents (
    id              TEXT PRIMARY KEY,       -- ULID
    run_id          TEXT NOT NULL,
    effect_type     TEXT NOT NULL,          -- model | tool | sign | broadcast | finality | ...
    idempotency_key TEXT NOT NULL,          -- stable key for deduplication
    payload         TEXT NOT NULL,          -- JSON-encoded intent details
    grant_digest    TEXT NOT NULL,          -- digest of the ResolvedGrant
    status          TEXT NOT NULL DEFAULT 'pending',  -- pending | claimed | completed | cancelled
    created_at      TEXT NOT NULL,
    scope_id        TEXT NOT NULL,
    FOREIGN KEY (run_id) REFERENCES runs(id),
    UNIQUE (idempotency_key)
);

CREATE INDEX idx_effect_intents_run ON effect_intents(run_id);
CREATE INDEX idx_effect_intents_status ON effect_intents(status) WHERE status = 'pending';

-- Effect attempts (one per try of an intent)
CREATE TABLE effect_attempts (
    id                TEXT PRIMARY KEY,     -- ULID
    intent_id         TEXT NOT NULL,        -- FK to effect_intents
    attempt_number    INTEGER NOT NULL,
    worker_id         TEXT NOT NULL,
    lease_expires_at  TEXT NOT NULL,        -- ISO 8601
    started_at        TEXT NOT NULL,
    completed_at      TEXT,
    status            TEXT NOT NULL DEFAULT 'running',  -- running | completed | expired
    FOREIGN KEY (intent_id) REFERENCES effect_intents(id),
    UNIQUE (intent_id, attempt_number)
);

CREATE INDEX idx_effect_attempts_intent ON effect_attempts(intent_id);
CREATE INDEX idx_effect_attempts_lease ON effect_attempts(lease_expires_at)
    WHERE status = 'running';

-- Effect outcomes (immutable results for exactly one attempt)
CREATE TABLE effect_outcomes (
    id              TEXT PRIMARY KEY,       -- ULID
    attempt_id      TEXT NOT NULL UNIQUE,   -- FK to effect_attempts
    outcome_status  TEXT NOT NULL,          -- success | failure | timeout | cancelled | unknown
    result_artifact_id TEXT,               -- FK to artifacts (result evidence)
    error_code      TEXT,
    error_message   TEXT,                  -- redacted
    external_ref    TEXT,                  -- e.g., transaction hash
    duration_ms     INTEGER,
    recorded_at     TEXT NOT NULL,
    FOREIGN KEY (attempt_id) REFERENCES effect_attempts(id),
    FOREIGN KEY (result_artifact_id) REFERENCES artifacts(id)
);
```

### 17.4 Lens tables (examples)

```sql
-- Run status lens (denormalized, recoverable)
CREATE TABLE lens_run_status (
    run_id          TEXT PRIMARY KEY,
    conversation_id TEXT,
    scope_id        TEXT NOT NULL,
    status          TEXT NOT NULL,          -- queued | working | approval | completing | done | failed | cancelled | timed_out
    phase           TEXT NOT NULL,
    started_at      TEXT NOT NULL,
    completed_at    TEXT,
    duration_ms     INTEGER,
    progress_pct    REAL,
    progress_stage  TEXT,
    outcome_summary TEXT,
    error_summary   TEXT,
    effect_count    INTEGER NOT NULL DEFAULT 0,
    artifact_count  INTEGER NOT NULL DEFAULT 0,
    updated_at      TEXT NOT NULL,
    cursor          INTEGER NOT NULL        -- global_sequence of last processed event
);

CREATE INDEX idx_lens_run_scope ON lens_run_status(scope_id, status);
CREATE INDEX idx_lens_run_conversation ON lens_run_status(conversation_id);

-- Usage lens (aggregated model/provider usage)
CREATE TABLE lens_usage (
    id              TEXT PRIMARY KEY,
    scope_id        TEXT NOT NULL,
    period_start    TEXT NOT NULL,          -- ISO 8601 (hourly bucket)
    period_end      TEXT NOT NULL,
    model_id        TEXT NOT NULL,
    provider_id     TEXT NOT NULL,
    request_count   INTEGER NOT NULL DEFAULT 0,
    tokens_input    INTEGER NOT NULL DEFAULT 0,
    tokens_output   INTEGER NOT NULL DEFAULT 0,
    cost_units      INTEGER NOT NULL DEFAULT 0,  -- smallest currency unit
    error_count     INTEGER NOT NULL DEFAULT 0,
    p50_latency_ms  INTEGER,
    p99_latency_ms  INTEGER,
    cursor          INTEGER NOT NULL
);

CREATE INDEX idx_lens_usage_scope ON lens_usage(scope_id, period_start);
CREATE INDEX idx_lens_usage_model ON lens_usage(model_id, period_start);

-- Lens cursor tracking (one row per lens)
CREATE TABLE lens_cursors (
    lens_name       TEXT PRIMARY KEY,
    cursor          INTEGER NOT NULL,       -- global_sequence
    computed_at     TEXT NOT NULL,          -- ISO 8601
    schema_version  INTEGER NOT NULL DEFAULT 1
);
```

### 17.5 Core supporting tables

```sql
-- Runs (minimal; execution semantics owned by PRD-03)
CREATE TABLE runs (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT,
    scope_id        TEXT NOT NULL,
    status          TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

-- Conversations (minimal; owned by PRD-06)
CREATE TABLE conversations (
    id              TEXT PRIMARY KEY,
    scope_id        TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    closed_at       TEXT
);

-- Schema version tracking
CREATE TABLE schema_migrations (
    version         INTEGER PRIMARY KEY,
    description     TEXT NOT NULL,
    applied_at      TEXT NOT NULL,
    checksum        TEXT NOT NULL
);
```

---

## 18. Wire formats

### 18.1 JSON event schema

Events are encoded as JSON for WebSocket/SSE streaming and API responses.

```json
{
  "$schema": "https://polkagent.dev/schemas/event/v1.json",
  "id": "01J5ABCDEF1234567890",
  "event_type": "EffectOutcomeRecorded",
  "sequence": 42,
  "global_sequence": 1000042,
  "run_id": "01J5RUNID123456789",
  "conversation_id": "01J5CONV1234567890",
  "correlation_id": "01J5CORR1234567890",
  "causation_id": "01J5CAUSE123456789",
  "scope": "tenant-abc",
  "timestamp": "2026-07-30T12:34:56.789Z",
  "durability": "durable",
  "payload": {
    "effect_intent_id": "01J5EFF1234567890",
    "attempt_id": "01J5ATT1234567890",
    "outcome_status": "success",
    "result_artifact_id": "01J5ART1234567890",
    "external_ref": "0xabcd1234...",
    "duration_ms": 342
  },
  "artifact_refs": ["01J5ART1234567890"],
  "schema_version": 1
}
```

### 18.2 JSON artifact metadata schema

```json
{
  "$schema": "https://polkagent.dev/schemas/artifact-meta/v1.json",
  "id": "01J5ART1234567890",
  "kind": "DecodedCall",
  "content_digest": "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
  "content_size": 4096,
  "content_type": "application/json",
  "parents": ["01J5PARENT1234567", "01J5PARENT2345678"],
  "provenance": {
    "source_type": "system",
    "software_version": "polkagent/0.1.0",
    "produced_at": "2026-07-30T12:34:56.789Z",
    "principal_id": null,
    "chain_evidence": {
      "genesis_hash": "0x91b171bb158e2d3848fa23a9f1c25182fb8e20313b2c1eb49219da7a70ce90c3",
      "spec_version": 1003000,
      "at_block_hash": "0xabcdef...",
      "at_block_number": 22500000,
      "metadata_hash": "0x123456..."
    }
  },
  "classification": "Private",
  "run_id": "01J5RUNID123456789",
  "scope": "tenant-abc",
  "created_at": "2026-07-30T12:34:56.789Z",
  "expires_at": "2028-07-30T12:34:56.789Z",
  "labels": {
    "chain": "polkadot",
    "pallet": "Balances",
    "call": "transfer_allow_death"
  },
  "schema_version": 1
}
```

### 18.3 WebSocket protocol

**REQ-WIRE-001.** WebSocket messages use JSON-RPC 2.0 framing:

```json
// Client -> Server: Subscribe
{
  "jsonrpc": "2.0",
  "method": "events.subscribe",
  "params": {
    "filters": {
      "run_id": "01J5RUNID123456789",
      "event_types": ["RunStarted", "RunCompleted", "EffectOutcomeRecorded"],
      "durability": ["durable", "diagnostic"]
    },
    "cursor": 999800
  },
  "id": 1
}

// Server -> Client: Subscription confirmed
{
  "jsonrpc": "2.0",
  "result": {
    "subscription_id": "sub-abc-123",
    "catchup_count": 42
  },
  "id": 1
}

// Server -> Client: Event notification
{
  "jsonrpc": "2.0",
  "method": "events.notification",
  "params": {
    "subscription_id": "sub-abc-123",
    "event": { /* event JSON as defined in 18.1 */ }
  }
}

// Server -> Client: Heartbeat
{
  "jsonrpc": "2.0",
  "method": "events.heartbeat",
  "params": {
    "server_time": "2026-07-30T12:35:00.000Z",
    "global_sequence": 1000100
  }
}
```

### 18.4 SSE format

**REQ-WIRE-010.** SSE events use the `event` and `data` fields:

```text
event: RunStarted
data: {"id":"01J5EVT123","run_id":"01J5RUN123","sequence":1,"global_sequence":1000001,...}

event: TextDelta
data: {"id":"01J5EVT124","run_id":"01J5RUN123","payload":{"text":"The transfer"}}

event: heartbeat
data: {"server_time":"2026-07-30T12:35:00.000Z","global_sequence":1000100}
```

**REQ-WIRE-011.** SSE connections use the `Last-Event-ID` header for
cursor-based resume. The `id` field in each SSE event is the
`global_sequence` as a string.

### 18.5 Export package format

**REQ-WIRE-020.** Export packages use a TAR archive with the following
structure:

```text
export-<id>.tar.gz
  manifest.json              # Package metadata, version, filters, integrity
  artifacts/
    <artifact-id>.meta.json  # Artifact metadata (section 18.2 format)
    <artifact-id>.body       # Artifact body bytes
  events/
    events.ndjson            # Newline-delimited JSON events (section 18.1 format)
  effects/
    intents.ndjson           # Effect intents
    attempts.ndjson          # Effect attempts
    outcomes.ndjson          # Effect outcomes
```

**REQ-WIRE-021.** The manifest includes:
- Format version.
- Export timestamp.
- Scope (tenant/workspace).
- Filters applied.
- Total artifact count, event count, and byte size.
- SHA-256 digest of each included file.

---

## 19. Performance requirements

### 19.1 Latency

| Operation | Target (p50) | Target (p99) | Measurement |
|---|---|---|---|
| Artifact store (inline body) | < 5 ms | < 20 ms | Time from `create` call to durable commit |
| Artifact store (blob body) | < 10 ms | < 50 ms | Time from `create` call to durable commit (body write + metadata commit) |
| Artifact retrieve (inline body) | < 2 ms | < 10 ms | Time from `get` call to body returned |
| Artifact retrieve (blob body) | < 5 ms | < 25 ms | Time from `get` call to body returned |
| Event persist (durable) | < 2 ms | < 10 ms | Time from event creation to committed in authority store |
| Event stream delivery | < 10 ms | < 50 ms | Time from event commit to delivery to a connected WebSocket client |
| Lineage query (5-level chain) | < 10 ms | < 50 ms | Time for a recursive ancestor query |
| Lens refresh | < 100 ms | < 500 ms | Time to process a batch of events and update a lens |
| Crash recovery | < 5 s | < 30 s | Time from process start to accepting new work |

### 19.2 Throughput

| Operation | Target (sustained) | Measurement |
|---|---|---|
| Artifact creation | 100 artifacts/second | Concurrent runs creating artifacts |
| Event production | 1000 events/second | Mixed durable and diagnostic events |
| WebSocket streaming | 500 events/second per connection | Sustained delivery to a single client |
| Concurrent runs | 50 active runs | Simultaneous runs in a single instance |
| Concurrent WebSocket connections | 100 connections | Active event stream consumers |

### 19.3 Storage budgets

| Data category | Budget (per active workspace) | Notes |
|---|---|---|
| Authority database | 10 GB | SQLite file size after 1 year of typical use |
| Blob storage (hot) | 50 GB | Active artifact bodies |
| Blob storage (cold/archive) | 500 GB | Archived evidence |
| Diagnostic events | 1 GB | Retained for configured period before expiry |
| Lens tables | 2 GB | Derived projections |
| Logs | 1 GB | Rotated structured log files |

**REQ-PERF-001.** Storage usage is monitored. When any category exceeds 80%
of its budget, a warning is emitted. When any category exceeds 95%, the
system:
- Triggers an immediate retention enforcement sweep.
- Emits an operator alert.
- Continues accepting new work (does not stop).

**REQ-PERF-002.** Operators may adjust storage budgets through configuration.
The budgets above are defaults for a single-user local deployment.
Managed multi-tenant deployments configure per-tenant quotas.

---

## 20. Acceptance criteria and verification checklist

### 20.1 Artifact system

| ID | Criterion | Verification method |
|---|---|---|
| AC-ART-01 | Artifact creation assigns a ULID, computes SHA-256 digest, and persists metadata + body atomically | Unit test: create artifact, verify ID format, digest match, and transactional persistence |
| AC-ART-02 | Artifact lineage DAG supports ancestor and descendant queries | Integration test: create a 7-level evidence chain (intent -> metadata -> decode -> simulation -> approval -> signature -> receipt), query ancestors of receipt returns all 7 |
| AC-ART-03 | Artifact immutability: modifying a stored artifact's core metadata fails | Unit test: attempt to update `kind`, `content_digest`, or `parents` of a stored artifact; verify rejection |
| AC-ART-04 | Content-addressed deduplication stores one body for two artifacts with the same digest and kind | Integration test: create two artifacts with identical content; verify one blob, two metadata rows |
| AC-ART-05 | Deduplication does not cross classification boundaries | Integration test: create two artifacts with identical content but different classifications; verify two blobs |
| AC-ART-06 | Soft-delete marks artifact as deleted; body is removed after retention period | Integration test: delete artifact, verify metadata retained with `deleted_at`, body accessible until retention expires, then removed |
| AC-ART-07 | Archival moves body to cold storage; metadata remains queryable | Integration test: archive artifact, verify metadata query succeeds, body retrieval returns with `cold` tier indicator |
| AC-ART-08 | Retention enforcement deletes expired artifacts and frees storage | Integration test: create artifacts with short expiry, run retention job, verify artifacts deleted and bytes freed |
| AC-ART-09 | Retention does not delete artifacts referenced as parents by non-expired children | Integration test: create parent/child pair where parent is expired but child is not; verify parent is retained |
| AC-ART-10 | Integrity violation detected when content does not match stored digest | Integration test: corrupt a blob file; retrieve artifact; verify `IntegrityViolation` event emitted and artifact quarantined |

### 20.2 Event system

| ID | Criterion | Verification method |
|---|---|---|
| AC-EVT-01 | Durable events survive process crash | Fault test: kill process during run; restart; verify all durable events are present and correctly sequenced |
| AC-EVT-02 | Per-run event sequence is monotonic with no gaps | Integration test: concurrent runs produce interleaved events; verify each run's sequence is monotonic |
| AC-EVT-03 | Global sequence is monotonic across all runs | Integration test: verify global sequence across concurrent runs is strictly increasing |
| AC-EVT-04 | At most one terminal event per run | Integration test: attempt to emit two terminal events for the same run; verify second is rejected |
| AC-EVT-05 | Cursor-based replay reproduces exact event sequence | Integration test: complete a run; replay events from cursor 0; verify sequence matches original |
| AC-EVT-06 | Diagnostic events may be absent during replay after retention expiry | Integration test: create diagnostic events with short retention; run retention enforcement; replay; verify graceful handling of missing diagnostics |
| AC-EVT-07 | WebSocket client receives catch-up events on reconnect | Integration test: client connects, receives events, disconnects, events occur, client reconnects with cursor; verify missed events delivered |
| AC-EVT-08 | Backpressure drops ephemeral events first, then diagnostic, never durable | Load test: produce events faster than consumer processes; verify ephemeral dropped first, durable always delivered |

### 20.3 Evidence-bearing effects (E1)

| ID | Criterion | Verification method |
|---|---|---|
| AC-E1-01 | Effect intent is stored before I/O | Integration test: trace a chain action; verify `EffectIntentCreated` event timestamp precedes any external call |
| AC-E1-02 | Idempotency key prevents duplicate effects | Fault test: simulate a crash after effect execution but before outcome recording; on recovery, verify the re-claimed attempt uses the same idempotency key and the external system reports deduplication |
| AC-E1-03 | Unknown outcome is preserved as Unknown, not converted | Integration test: simulate a timeout after broadcast; verify `EffectOutcome` status is `Unknown`; verify UX shows "Unknown" |
| AC-E1-04 | Evidence chain for a chain action links all required artifacts | Integration test: complete a chain action end-to-end; verify receipt artifact has lineage to intent, metadata, decode, simulation, approval, signature, broadcast, and finality artifacts |
| AC-E1-05 | Evidence reconstruction from authority store succeeds without external state | Integration test: complete a chain action; clear in-memory state; reconstruct evidence chain from database only; verify all artifacts and their digests |

### 20.4 Replay and debugging (E4)

| ID | Criterion | Verification method |
|---|---|---|
| AC-E4-01 | Replay reproduces same state transitions for a deterministic reducer | Unit test: record a run; replay it; verify state sequence matches |
| AC-E4-02 | Replay substitutes recorded outcomes, never sends external effects | Integration test: replay a run that included model calls and chain broadcasts; verify zero external calls made |
| AC-E4-03 | Replay detects and reports divergence | Unit test: modify a reducer; replay a previously recorded run; verify divergence report identifies the differing reduction |
| AC-E4-04 | Replay does not modify the authority store | Integration test: replay a run; verify all authority store tables are unchanged (row counts and digests) |

### 20.5 Recovery

| ID | Criterion | Verification method |
|---|---|---|
| AC-REC-01 | Crash recovery completes within 30 seconds for 10 GB database | Performance test: populate 10 GB database with in-progress runs; kill process; measure recovery time |
| AC-REC-02 | Crash recovery does not duplicate external effects | Fault test: kill process during effect execution; on recovery, verify expired leases re-queued, completed effects not re-attempted |
| AC-REC-03 | Backup and restore produces identical authority store | Integration test: populate database; backup; restore to new location; compare all tables row-by-row |
| AC-REC-04 | Lens rebuild from events produces identical lens state | Integration test: build lenses from events; drop lens tables; rebuild from events; compare row-by-row |
| AC-REC-05 | Export and import round-trip preserves artifact integrity | Integration test: create artifacts; export; import to new instance; verify content digests match |
| AC-REC-06 | Import is idempotent | Integration test: import same package twice; verify no duplicate rows |

### 20.6 Data classification

| ID | Criterion | Verification method |
|---|---|---|
| AC-CLS-01 | Artifact classification defaults match kind defaults | Unit test: create each artifact kind without explicit classification; verify default is applied |
| AC-CLS-02 | Inherited classification is at least as restrictive as most restrictive parent | Integration test: create artifact with parents of different classifications; verify child inherits the most restrictive |
| AC-CLS-03 | SecretForbidden content is detected and quarantined | Integration test: attempt to store an artifact body containing a known private key pattern; verify quarantine and IntegrityViolation event |
| AC-CLS-04 | Logs do not contain secret material | Integration test: run a complete chain action with API keys configured; grep all log output for key patterns; verify zero matches |
| AC-CLS-05 | Model context excludes Sensitive and SecretForbidden artifacts | Integration test: create artifacts at each classification level; assemble context; verify Sensitive and SecretForbidden artifacts are excluded |

### 20.7 Observability

| ID | Criterion | Verification method |
|---|---|---|
| AC-OBS-01 | Prometheus /metrics endpoint exposes all required metric families | Integration test: exercise all code paths; scrape /metrics; verify every metric family from section 10.2 is present |
| AC-OBS-02 | Traces include mandatory spans for run, turn, effect, model, tool, chain | Integration test: complete a chain action; export traces; verify span tree matches expected structure |
| AC-OBS-03 | Trace-event correlation: every durable event includes trace_id and span_id | Integration test: complete a run; verify all durable events have non-null trace_id and span_id |
| AC-OBS-04 | Log redaction: no secret material in log output at any level | Security test: configure trace-level logging; run actions with secrets; verify no secrets in output |

### 20.8 Performance

| ID | Criterion | Verification method |
|---|---|---|
| AC-PERF-01 | Artifact store latency meets p99 targets | Benchmark: 1000 sequential artifact creates; measure p50 and p99 |
| AC-PERF-02 | Event production throughput meets target | Benchmark: sustained event production at 1000/s for 60 seconds; measure drop rate |
| AC-PERF-03 | 50 concurrent runs complete without contention failures | Load test: 50 simultaneous runs with mixed effects; verify zero contention errors |
| AC-PERF-04 | WebSocket streaming delivers 500 events/s per connection | Benchmark: stream events to a single client at target rate; measure delivery latency and drop rate |
| AC-PERF-05 | Storage stays within budget after 30 days of simulated typical use | Simulation test: generate 30 days of typical workload; verify storage categories are within budgets |

---

## Appendix A: Requirement index

All requirements in this document are prefixed by their section:

| Prefix | Section |
|---|---|
| `REQ-ART-*` | Artifact taxonomy and lifecycle |
| `REQ-STORE-*` | Artifact storage |
| `REQ-EVT-*` | Event system |
| `REQ-STREAM-*` | Event stream architecture |
| `REQ-OBS-*` | Observability stack |
| `REQ-E1-*` | Evidence-bearing effects |
| `REQ-E4-*` | Replay and debugging |
| `REQ-E2-*` | Bulletin/IPFS anchoring |
| `REQ-F3-*` | Live run timeline |
| `REQ-REC-*` | Recovery mechanisms |
| `REQ-CLASS-*` | Data classification |
| `REQ-WIRE-*` | Wire formats |
| `REQ-PERF-*` | Performance |

Requirements use stable IDs. New requirements are appended; existing IDs are
never reused.

---

## Appendix B: Glossary cross-reference

| This PRD term | Roko equivalent | Baseline equivalent |
|---|---|---|
| Artifact | Signal (durable subset) | Artifact |
| Run event | Pulse (lifecycle subset) | RunEvent |
| EffectIntent | Part of effect driver model | EffectIntent |
| EffectAttempt | Part of effect driver model | EffectAttempt |
| EffectOutcome | Part of effect driver model | EffectOutcome |
| Lens | Lens / StateHub projection | Projection |
| Classification | CaMeL classification (simplified) | Classification |
| Content digest | Signal attestation/hash | Content digest |
| Durability class | Signal vs Pulse | (new in this PRD) |
| Correlation ID | (ad hoc) | (new in this PRD) |
| Causation ID | (ad hoc) | (new in this PRD) |

---

## Appendix C: Decision log

| Decision | Rationale | Alternatives considered |
|---|---|---|
| ULID for artifact and event IDs | Sortable, no coordination, 128-bit collision resistance | UUIDv7 (similar properties; ULID has broader Rust ecosystem support), auto-increment (not portable across instances) |
| SHA-256 as canonical external digest; BLAKE3 as optional internal fast digest | SHA-256 is required for Sigstore, IPFS CIDv0, and on-chain anchoring interop. BLAKE3 is faster and parallelizable and may be maintained internally for deduplication and integrity checks where external interop is not needed. Research3.md finding 23 [V/I] confirmed this dual-algorithm approach. | SHA-256 only (simpler, less throughput); BLAKE3 only (breaks external ecosystem interop); SHA-3 (no clear advantage for this use case) |
| SQLite as default authority store | Embedded, zero-ops, sufficient for single-user and small-team deployments, well-tested WAL mode | PostgreSQL only (over-provisioned for local use), DuckDB (less mature for OLTP), custom LSM (unnecessary complexity) |
| Separate blob storage for large artifacts | Keeps SQLite database compact; avoids BLOB I/O overhead in transactional queries | All-inline (limits practical database size), all-external (unnecessary I/O for small artifacts) |
| Three durability classes | Balances retention cost against diagnostic value and recovery needs | Two classes (insufficient granularity), per-event TTL (too complex for producers) |
| Lens/projection pattern for UIs | Avoids mutable shared state; recoverable from durable events; separates read and write concerns | Direct database queries (no denormalization; poor read performance for complex views), in-memory caches (not crash-recoverable) |
| At-least-once event delivery with idempotent consumers | Simpler than exactly-once; proven pattern; consumers control deduplication | Exactly-once (requires distributed transactions; false promise over unreliable networks), at-most-once (unacceptable for durable events) |
| Classification levels (5 levels) | Matches common enterprise data handling requirements; simpler than Roko's CaMeL scheme | Three levels (insufficient for secret/PII distinction), seven levels (over-segmented for v1) |

---

## Appendix D: Open questions

| Question | Impact | Resolution path |
|---|---|---|
| Optimal SQLite page size for mixed workloads (metadata + small blobs) | Performance | Benchmark with realistic workload during implementation spike |
| PostgreSQL partitioning strategy for multi-tenant event tables | Managed cloud performance | Evaluate range partitioning by scope + time vs. hash partitioning by scope during cloud architecture spike |
| BLAKE3 vs SHA-256 for content digests | Performance of artifact creation | Benchmark both; consider BLAKE3 if hardware SHA-256 is not available on target platforms |
| Exact coalescing semantics for TextDelta events | Streaming UX quality | User study during UX spike; define maximum coalescing window |
| Bulletin retention and cost on target networks | E2 feasibility | Live network measurement during Bulletin spike |
| Point-in-time recovery granularity for local SQLite deployments | Recovery completeness | Evaluate WAL archiving approaches during recovery spike |

---

## APPENDIX A: DATABASE SCHEMA IMPLEMENTATION

### A.1 Complete SQLite Schema

The sketches in section 17 give the outline. This appendix provides the
complete, deployable DDL with all columns, constraints, indexes, triggers,
and supporting tables. Execute this in a single transaction guarded by
`schema_migrations` version 1.

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
PRAGMA wal_autocheckpoint = 1000;

-- ── schema migrations ──────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS schema_migrations (
    version         INTEGER PRIMARY KEY,
    description     TEXT    NOT NULL,
    applied_at      TEXT    NOT NULL,   -- ISO 8601 UTC
    checksum        TEXT    NOT NULL    -- SHA-256 of migration SQL
);

-- ── scope / tenant ─────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS scopes (
    id              TEXT    PRIMARY KEY,   -- ULID
    kind            TEXT    NOT NULL,      -- user | workspace | tenant
    owner_id        TEXT,
    created_at      TEXT    NOT NULL,
    closed_at       TEXT
);

-- ── conversations ──────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS conversations (
    id              TEXT    PRIMARY KEY,   -- ULID
    scope_id        TEXT    NOT NULL,
    created_at      TEXT    NOT NULL,
    closed_at       TEXT,
    FOREIGN KEY (scope_id) REFERENCES scopes(id)
);

CREATE INDEX IF NOT EXISTS idx_conv_scope ON conversations(scope_id);

-- ── runs ───────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS runs (
    id              TEXT    PRIMARY KEY,   -- ULID
    conversation_id TEXT,
    scope_id        TEXT    NOT NULL,
    -- terminal states: completed | failed | cancelled | timed_out
    status          TEXT    NOT NULL DEFAULT 'queued',
    created_at      TEXT    NOT NULL,
    updated_at      TEXT    NOT NULL,
    FOREIGN KEY (scope_id)        REFERENCES scopes(id),
    FOREIGN KEY (conversation_id) REFERENCES conversations(id)
);

CREATE INDEX IF NOT EXISTS idx_runs_scope_status   ON runs(scope_id, status);
CREATE INDEX IF NOT EXISTS idx_runs_conversation   ON runs(conversation_id)
    WHERE conversation_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_runs_created        ON runs(created_at);

-- ── turns ──────────────────────────────────────────────────────────────────
-- One turn = one request/response cycle within a run.

CREATE TABLE IF NOT EXISTS turns (
    id              TEXT    PRIMARY KEY,   -- ULID
    run_id          TEXT    NOT NULL,
    turn_number     INTEGER NOT NULL,      -- monotonic within run (1-based)
    status          TEXT    NOT NULL DEFAULT 'active',
    started_at      TEXT    NOT NULL,
    completed_at    TEXT,
    FOREIGN KEY (run_id) REFERENCES runs(id),
    UNIQUE (run_id, turn_number)
);

CREATE INDEX IF NOT EXISTS idx_turns_run ON turns(run_id);

-- ── steps ──────────────────────────────────────────────────────────────────
-- One step = one tool invocation or sub-action within a turn.

CREATE TABLE IF NOT EXISTS steps (
    id              TEXT    PRIMARY KEY,   -- ULID
    turn_id         TEXT    NOT NULL,
    run_id          TEXT    NOT NULL,
    step_number     INTEGER NOT NULL,      -- monotonic within turn (1-based)
    step_type       TEXT    NOT NULL,      -- model | tool | chain_rpc | signer | ...
    status          TEXT    NOT NULL DEFAULT 'pending',
    started_at      TEXT    NOT NULL,
    completed_at    TEXT,
    FOREIGN KEY (turn_id) REFERENCES turns(id),
    FOREIGN KEY (run_id)  REFERENCES runs(id),
    UNIQUE (turn_id, step_number)
);

CREATE INDEX IF NOT EXISTS idx_steps_run  ON steps(run_id);
CREATE INDEX IF NOT EXISTS idx_steps_turn ON steps(turn_id);

-- ── artifacts ──────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS artifacts (
    id                  TEXT    PRIMARY KEY,   -- ULID
    kind                TEXT    NOT NULL,      -- ArtifactKind enum value
    content_digest      TEXT    NOT NULL,      -- "sha256:<hex>"
    blake3_digest       TEXT,                  -- "blake3:<hex>" optional fast digest
    content_size        INTEGER NOT NULL,      -- bytes
    content_type        TEXT,                  -- MIME type, nullable
    classification      TEXT    NOT NULL,      -- Public|Internal|Private|Sensitive|SecretForbidden
    run_id              TEXT,
    turn_id             TEXT,
    step_id             TEXT,
    effect_attempt_id   TEXT,
    conversation_id     TEXT,
    scope_id            TEXT    NOT NULL,
    created_at          TEXT    NOT NULL,      -- ISO 8601 UTC
    expires_at          TEXT,                  -- ISO 8601 UTC, nullable
    deleted_at          TEXT,                  -- soft-delete timestamp, nullable
    storage_tier        TEXT    NOT NULL DEFAULT 'hot', -- hot | cold
    integrity_ok        INTEGER NOT NULL DEFAULT 1,     -- 0 = quarantined
    schema_version      INTEGER NOT NULL DEFAULT 1,
    FOREIGN KEY (run_id)            REFERENCES runs(id),
    FOREIGN KEY (conversation_id)   REFERENCES conversations(id),
    FOREIGN KEY (turn_id)           REFERENCES turns(id),
    FOREIGN KEY (step_id)           REFERENCES steps(id)
);

CREATE INDEX IF NOT EXISTS idx_artifacts_run        ON artifacts(run_id)
    WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_artifacts_kind       ON artifacts(kind, created_at)
    WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_artifacts_scope      ON artifacts(scope_id, created_at)
    WHERE deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_artifacts_digest     ON artifacts(content_digest);
CREATE INDEX IF NOT EXISTS idx_artifacts_blake3     ON artifacts(blake3_digest)
    WHERE blake3_digest IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_artifacts_expires    ON artifacts(expires_at)
    WHERE expires_at IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_artifacts_conv       ON artifacts(conversation_id)
    WHERE deleted_at IS NULL AND conversation_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_artifacts_quarantine ON artifacts(integrity_ok)
    WHERE integrity_ok = 0;

-- Lineage DAG (parent -> child edges)
CREATE TABLE IF NOT EXISTS artifact_parents (
    child_id    TEXT NOT NULL,
    parent_id   TEXT NOT NULL,
    PRIMARY KEY (child_id, parent_id),
    FOREIGN KEY (child_id)  REFERENCES artifacts(id),
    FOREIGN KEY (parent_id) REFERENCES artifacts(id)
);

CREATE INDEX IF NOT EXISTS idx_artifact_parents_parent ON artifact_parents(parent_id);

-- Provenance (1:1 with artifact)
CREATE TABLE IF NOT EXISTS artifact_provenance (
    artifact_id       TEXT    PRIMARY KEY,
    source_type       TEXT    NOT NULL,   -- system|model|user|external|derived
    software_version  TEXT    NOT NULL,
    produced_at       TEXT    NOT NULL,
    principal_id      TEXT,
    genesis_hash      TEXT,               -- chain evidence fields, all nullable
    spec_version      INTEGER,
    at_block_hash     TEXT,
    at_block_number   INTEGER,
    metadata_hash     TEXT,
    FOREIGN KEY (artifact_id) REFERENCES artifacts(id)
);

-- Labels (key/value, arbitrary)
CREATE TABLE IF NOT EXISTS artifact_labels (
    artifact_id TEXT NOT NULL,
    key         TEXT NOT NULL,
    value       TEXT NOT NULL,
    PRIMARY KEY (artifact_id, key),
    FOREIGN KEY (artifact_id) REFERENCES artifacts(id)
);

CREATE INDEX IF NOT EXISTS idx_artifact_labels_kv ON artifact_labels(key, value);

-- Inline bodies (<= 256 KB stored here; larger stored on filesystem)
CREATE TABLE IF NOT EXISTS artifact_bodies (
    artifact_id TEXT    PRIMARY KEY,
    body        BLOB    NOT NULL,
    FOREIGN KEY (artifact_id) REFERENCES artifacts(id)
);

-- Trigger: prevent mutation of immutable fields after initial insert.
-- Any UPDATE that touches these columns is rejected.
CREATE TRIGGER IF NOT EXISTS trg_artifact_immutable
BEFORE UPDATE ON artifacts
FOR EACH ROW
WHEN OLD.id IS NOT NULL
BEGIN
    SELECT CASE
        WHEN NEW.id              != OLD.id              THEN RAISE(ABORT, 'artifact.id is immutable')
        WHEN NEW.kind            != OLD.kind            THEN RAISE(ABORT, 'artifact.kind is immutable')
        WHEN NEW.content_digest  != OLD.content_digest  THEN RAISE(ABORT, 'artifact.content_digest is immutable')
        WHEN NEW.content_size    != OLD.content_size    THEN RAISE(ABORT, 'artifact.content_size is immutable')
        WHEN NEW.classification  != OLD.classification  THEN RAISE(ABORT, 'artifact.classification is immutable')
        WHEN NEW.scope_id        != OLD.scope_id        THEN RAISE(ABORT, 'artifact.scope_id is immutable')
        WHEN NEW.created_at      != OLD.created_at      THEN RAISE(ABORT, 'artifact.created_at is immutable')
    END;
END;

-- ── run events ─────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS run_events (
    id              TEXT    PRIMARY KEY,   -- ULID
    event_type      TEXT    NOT NULL,
    sequence        INTEGER NOT NULL,      -- per-run monotonic
    global_sequence INTEGER NOT NULL,      -- auto-assigned, strictly increasing
    run_id          TEXT    NOT NULL,
    turn_id         TEXT,
    step_id         TEXT,
    conversation_id TEXT,
    correlation_id  TEXT    NOT NULL,
    causation_id    TEXT,                  -- FK to run_events
    scope_id        TEXT    NOT NULL,
    timestamp       TEXT    NOT NULL,      -- ISO 8601 UTC
    durability      TEXT    NOT NULL,      -- Durable | Diagnostic
    payload         TEXT    NOT NULL,      -- JSON-encoded EventPayload
    trace_id        TEXT,                  -- W3C 32-hex-char trace ID
    span_id         TEXT,                  -- W3C 16-hex-char span ID
    schema_version  INTEGER NOT NULL DEFAULT 1,
    FOREIGN KEY (run_id)  REFERENCES runs(id),
    FOREIGN KEY (turn_id) REFERENCES turns(id),
    FOREIGN KEY (step_id) REFERENCES steps(id),
    UNIQUE (run_id, sequence)
);

-- global_sequence must be unique and increasing; enforced by the
-- single-writer task that inserts events.
CREATE UNIQUE INDEX IF NOT EXISTS idx_events_global      ON run_events(global_sequence);
CREATE        INDEX IF NOT EXISTS idx_events_run         ON run_events(run_id, sequence);
CREATE        INDEX IF NOT EXISTS idx_events_correlation ON run_events(correlation_id);
CREATE        INDEX IF NOT EXISTS idx_events_scope_type  ON run_events(scope_id, event_type, global_sequence);
CREATE        INDEX IF NOT EXISTS idx_events_conv        ON run_events(conversation_id, global_sequence)
    WHERE conversation_id IS NOT NULL;
CREATE        INDEX IF NOT EXISTS idx_events_turn        ON run_events(turn_id)
    WHERE turn_id IS NOT NULL;

-- Trigger: prevent more than one terminal event per run.
-- Terminal types: RunCompleted, RunFailed, RunCancelled, RunTimedOut.
CREATE TRIGGER IF NOT EXISTS trg_run_single_terminal
BEFORE INSERT ON run_events
FOR EACH ROW
WHEN NEW.event_type IN ('RunCompleted','RunFailed','RunCancelled','RunTimedOut')
BEGIN
    SELECT CASE
        WHEN (
            SELECT COUNT(*) FROM run_events
            WHERE run_id = NEW.run_id
              AND event_type IN ('RunCompleted','RunFailed','RunCancelled','RunTimedOut')
        ) > 0
        THEN RAISE(ABORT, 'only one terminal event is permitted per run')
    END;
END;

-- Event-to-artifact cross-reference
CREATE TABLE IF NOT EXISTS event_artifact_refs (
    event_id    TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    PRIMARY KEY (event_id, artifact_id),
    FOREIGN KEY (event_id)    REFERENCES run_events(id),
    FOREIGN KEY (artifact_id) REFERENCES artifacts(id)
);

-- Diagnostic events (shorter retention, queried separately)
CREATE TABLE IF NOT EXISTS diagnostic_events (
    id          TEXT    PRIMARY KEY,   -- ULID
    event_type  TEXT    NOT NULL,
    run_id      TEXT    NOT NULL,
    turn_id     TEXT,
    scope_id    TEXT    NOT NULL,
    timestamp   TEXT    NOT NULL,
    payload     TEXT    NOT NULL,      -- JSON
    expires_at  TEXT    NOT NULL,
    FOREIGN KEY (run_id) REFERENCES runs(id)
);

CREATE INDEX IF NOT EXISTS idx_diag_expires ON diagnostic_events(expires_at);
CREATE INDEX IF NOT EXISTS idx_diag_run     ON diagnostic_events(run_id);

-- ── effect tables ──────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS effect_intents (
    id              TEXT    PRIMARY KEY,   -- ULID
    run_id          TEXT    NOT NULL,
    turn_id         TEXT,
    step_id         TEXT,
    effect_type     TEXT    NOT NULL,      -- model|tool|sign|broadcast|finality|anchor|...
    idempotency_key TEXT    NOT NULL,
    payload         TEXT    NOT NULL,      -- JSON intent details (no secret material)
    grant_digest    TEXT    NOT NULL,      -- SHA-256 of the ResolvedGrant JSON
    status          TEXT    NOT NULL DEFAULT 'pending', -- pending|claimed|completed|cancelled
    created_at      TEXT    NOT NULL,
    scope_id        TEXT    NOT NULL,
    FOREIGN KEY (run_id)  REFERENCES runs(id),
    FOREIGN KEY (turn_id) REFERENCES turns(id),
    FOREIGN KEY (step_id) REFERENCES steps(id),
    UNIQUE (idempotency_key)
);

CREATE INDEX IF NOT EXISTS idx_effect_intents_run     ON effect_intents(run_id);
CREATE INDEX IF NOT EXISTS idx_effect_intents_pending ON effect_intents(status)
    WHERE status = 'pending';
CREATE INDEX IF NOT EXISTS idx_effect_intents_scope   ON effect_intents(scope_id, created_at);

CREATE TABLE IF NOT EXISTS effect_attempts (
    id               TEXT    PRIMARY KEY,   -- ULID
    intent_id        TEXT    NOT NULL,
    attempt_number   INTEGER NOT NULL,
    worker_id        TEXT    NOT NULL,
    lease_expires_at TEXT    NOT NULL,      -- ISO 8601 UTC
    started_at       TEXT    NOT NULL,
    completed_at     TEXT,
    status           TEXT    NOT NULL DEFAULT 'running', -- running|completed|expired
    FOREIGN KEY (intent_id) REFERENCES effect_intents(id),
    UNIQUE (intent_id, attempt_number)
);

CREATE INDEX IF NOT EXISTS idx_attempts_intent ON effect_attempts(intent_id);
CREATE INDEX IF NOT EXISTS idx_attempts_lease  ON effect_attempts(lease_expires_at)
    WHERE status = 'running';

CREATE TABLE IF NOT EXISTS effect_outcomes (
    id                  TEXT    PRIMARY KEY,   -- ULID
    attempt_id          TEXT    NOT NULL UNIQUE,
    outcome_status      TEXT    NOT NULL,      -- success|failure|timeout|cancelled|unknown
    result_artifact_id  TEXT,
    error_code          TEXT,
    error_message       TEXT,                  -- redacted; no secret material
    external_ref        TEXT,                  -- e.g., transaction hash
    duration_ms         INTEGER,
    recorded_at         TEXT    NOT NULL,
    FOREIGN KEY (attempt_id)         REFERENCES effect_attempts(id),
    FOREIGN KEY (result_artifact_id) REFERENCES artifacts(id)
);

-- ── projections / lenses ───────────────────────────────────────────────────

-- Cursor tracking for all lenses (one row per named lens)
CREATE TABLE IF NOT EXISTS lens_cursors (
    lens_name       TEXT    PRIMARY KEY,
    cursor          INTEGER NOT NULL DEFAULT 0,  -- global_sequence of last processed event
    computed_at     TEXT    NOT NULL,
    schema_version  INTEGER NOT NULL DEFAULT 1
);

-- Run status lens
CREATE TABLE IF NOT EXISTS lens_run_status (
    run_id          TEXT    PRIMARY KEY,
    conversation_id TEXT,
    scope_id        TEXT    NOT NULL,
    status          TEXT    NOT NULL,
    phase           TEXT    NOT NULL,
    started_at      TEXT    NOT NULL,
    completed_at    TEXT,
    duration_ms     INTEGER,
    progress_pct    REAL,
    progress_stage  TEXT,
    outcome_summary TEXT,
    error_summary   TEXT,
    effect_count    INTEGER NOT NULL DEFAULT 0,
    artifact_count  INTEGER NOT NULL DEFAULT 0,
    turn_count      INTEGER NOT NULL DEFAULT 0,
    updated_at      TEXT    NOT NULL,
    cursor          INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_lens_run_scope ON lens_run_status(scope_id, status);
CREATE INDEX IF NOT EXISTS idx_lens_run_conv  ON lens_run_status(conversation_id)
    WHERE conversation_id IS NOT NULL;

-- Effect status lens
CREATE TABLE IF NOT EXISTS lens_effect_status (
    intent_id       TEXT    PRIMARY KEY,
    run_id          TEXT    NOT NULL,
    scope_id        TEXT    NOT NULL,
    effect_type     TEXT    NOT NULL,
    status          TEXT    NOT NULL,
    attempt_count   INTEGER NOT NULL DEFAULT 0,
    last_outcome    TEXT,
    next_retry_at   TEXT,
    last_error      TEXT,
    created_at      TEXT    NOT NULL,
    updated_at      TEXT    NOT NULL,
    cursor          INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_lens_eff_run    ON lens_effect_status(run_id);
CREATE INDEX IF NOT EXISTS idx_lens_eff_scope  ON lens_effect_status(scope_id, status);

-- Approval lens
CREATE TABLE IF NOT EXISTS lens_approval (
    approval_id     TEXT    PRIMARY KEY,   -- from ApprovalRequested payload
    run_id          TEXT    NOT NULL,
    scope_id        TEXT    NOT NULL,
    status          TEXT    NOT NULL,      -- pending|granted|denied|expired
    requestor_id    TEXT,
    action_summary  TEXT,
    deadline        TEXT,
    decided_at      TEXT,
    cursor          INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_lens_approval_scope  ON lens_approval(scope_id, status);
CREATE INDEX IF NOT EXISTS idx_lens_approval_run    ON lens_approval(run_id);

-- Usage lens (hourly buckets)
CREATE TABLE IF NOT EXISTS lens_usage (
    id              TEXT    PRIMARY KEY,   -- ULID
    scope_id        TEXT    NOT NULL,
    period_start    TEXT    NOT NULL,      -- ISO 8601, truncated to hour
    period_end      TEXT    NOT NULL,
    model_id        TEXT    NOT NULL,
    provider_id     TEXT    NOT NULL,
    request_count   INTEGER NOT NULL DEFAULT 0,
    tokens_input    INTEGER NOT NULL DEFAULT 0,
    tokens_output   INTEGER NOT NULL DEFAULT 0,
    cost_units      INTEGER NOT NULL DEFAULT 0,  -- smallest currency unit (e.g., micro-USD)
    error_count     INTEGER NOT NULL DEFAULT 0,
    p50_latency_ms  INTEGER,
    p99_latency_ms  INTEGER,
    cursor          INTEGER NOT NULL DEFAULT 0,
    UNIQUE (scope_id, period_start, model_id, provider_id)
);

CREATE INDEX IF NOT EXISTS idx_lens_usage_scope  ON lens_usage(scope_id, period_start);
CREATE INDEX IF NOT EXISTS idx_lens_usage_model  ON lens_usage(model_id, period_start);

-- Health lens (per-component)
CREATE TABLE IF NOT EXISTS lens_health (
    component       TEXT    PRIMARY KEY,
    scope_id        TEXT    NOT NULL,
    status          TEXT    NOT NULL DEFAULT 'unknown', -- ok|degraded|error|unknown
    last_heartbeat  TEXT,
    error_count_1h  INTEGER NOT NULL DEFAULT 0,
    p99_latency_ms  INTEGER,
    detail          TEXT,
    cursor          INTEGER NOT NULL DEFAULT 0
);

-- Policy audit lens
CREATE TABLE IF NOT EXISTS lens_policy_audit (
    id              TEXT    PRIMARY KEY,   -- ULID
    run_id          TEXT    NOT NULL,
    scope_id        TEXT    NOT NULL,
    policy_revision TEXT    NOT NULL,
    grant_digest    TEXT    NOT NULL,
    decision        TEXT    NOT NULL,      -- granted|denied
    reason          TEXT,
    decided_at      TEXT    NOT NULL,
    cursor          INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_lens_audit_scope ON lens_policy_audit(scope_id, decided_at);
CREATE INDEX IF NOT EXISTS idx_lens_audit_run   ON lens_policy_audit(run_id);

-- Artifact index lens
CREATE TABLE IF NOT EXISTS lens_artifact_index (
    artifact_id     TEXT    PRIMARY KEY,
    kind            TEXT    NOT NULL,
    run_id          TEXT,
    scope_id        TEXT    NOT NULL,
    classification  TEXT    NOT NULL,
    content_size    INTEGER NOT NULL,
    created_at      TEXT    NOT NULL,
    labels_json     TEXT    NOT NULL DEFAULT '{}',   -- denormalized JSON object
    cursor          INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_lens_art_scope ON lens_artifact_index(scope_id, kind, created_at);
CREATE INDEX IF NOT EXISTS idx_lens_art_run   ON lens_artifact_index(run_id)
    WHERE run_id IS NOT NULL;
```

### A.2 Event Versioning Strategy

#### Schema evolution rules

Every event payload type carries a `schema_version: u32` field (also stored
as a column on `run_events`). The rules below govern how versions increment
and how older records are handled.

| Change type | Schema version impact | Migration required |
|---|---|---|
| Add optional field with a default value | Minor — no increment required; old readers ignore unknown fields | No |
| Add required field | New version — increment `schema_version` | Migration function required |
| Rename field | New version | Migration function required |
| Remove field | New version | Migration function required |
| Change field type or semantics | New version | Migration function required |

**Migration functions.** Each event type maintains a migration table keyed
by `(event_type, from_version)` → `to_version` transformation:

```rust
/// Upgrade an event payload from schema_version N to the current version.
/// Returns the upgraded payload and the new schema_version.
pub trait EventMigration {
    fn migrate(
        event_type: &EventType,
        from_version: u32,
        payload: serde_json::Value,
    ) -> Result<(serde_json::Value, u32), MigrationError>;
}

/// Registry of migration functions keyed by (event_type, from_version).
pub struct EventMigrationRegistry {
    /// (event_type_str, from_version) -> migration fn
    migrations: HashMap<(String, u32), Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value, MigrationError>>>,
}

impl EventMigrationRegistry {
    /// Upgrade a payload through all intermediate versions to reach current.
    /// If no migration path exists, returns MigrationError::NoPath.
    pub fn upgrade_to_current(
        &self,
        event_type: &str,
        from_version: u32,
        current_version: u32,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, MigrationError> {
        let mut value = payload;
        let mut v = from_version;
        while v < current_version {
            let key = (event_type.to_string(), v);
            let f = self.migrations.get(&key)
                .ok_or(MigrationError::NoPath { event_type: event_type.to_string(), from: v, to: current_version })?;
            value = f(value)?;
            v += 1;
        }
        Ok(value)
    }
}
```

**Read-time vs sweep migration.** Two strategies are supported:

- **Read-time (lazy):** When an event is read from the store, the migration
  registry upgrades its payload on the fly if `schema_version` < current.
  Results are not written back; the store retains the original version.
  Suitable for most events during the migration window.

- **Sweep migration:** An explicit migration job reads all events of a given
  type below the target version, upgrades them, and writes the upgraded
  payload back with the new `schema_version`. Used after the migration
  window closes, to avoid paying the migration cost on every read.

**Replay accuracy.** Deterministic replay (section 12) must use the
**original** stored `schema_version` and payload. It must not upgrade
event payloads before replaying them, because the reducer that originally
ran may have expected the old schema. The replay harness receives the raw
stored payload; the upgrade path is applied only when the operator
explicitly passes `--migrate-on-replay`.

#### Dead letter handling

When the event store encounters an event with an unknown `event_type` (i.e.,
a type not registered in the current codebase), it applies the following
policy:

```rust
pub enum UnknownEventPolicy {
    /// Log a warning and skip the event during projection.
    /// Used for lenses that process a subset of event types.
    Skip,
    /// Move to the dead-letter table and continue.
    DeadLetter,
    /// Halt processing and require operator intervention.
    Halt,
}
```

Dead-lettered events are stored in:

```sql
CREATE TABLE IF NOT EXISTS dead_letter_events (
    id              TEXT    PRIMARY KEY,
    event_type      TEXT    NOT NULL,   -- unknown type string
    schema_version  INTEGER NOT NULL,
    raw_payload     TEXT    NOT NULL,   -- original JSON, unmodified
    global_sequence INTEGER NOT NULL,
    run_id          TEXT,
    received_at     TEXT    NOT NULL,
    reason          TEXT    NOT NULL    -- 'unknown_type' | 'migration_failed' | 'parse_error'
);

CREATE INDEX IF NOT EXISTS idx_dlq_received ON dead_letter_events(received_at);
```

Operators can inspect dead-letter events, provide a migration function, and
re-process them with a recovery job. They are not deleted automatically.

### A.3 Projection System

#### Projection rebuild algorithm

A projection (lens) is a pure function of the durable event stream. The
rebuild algorithm is identical for initial build and corruption recovery:

```
1. BEGIN EXCLUSIVE TRANSACTION on the lens tables.
2. DELETE all rows from the target lens table.
3. UPDATE lens_cursors SET cursor = 0 WHERE lens_name = '<lens>';
4. COMMIT.

5. Open a read cursor on run_events at global_sequence = 1.
6. Read events in batches of BATCH_SIZE (default: 500).
7. For each event:
   a. Look up the lens's event handler for event.event_type.
   b. If no handler, skip.
   c. Apply the handler: UPSERT into the lens table.
8. After each batch:
   a. BEGIN TRANSACTION.
   b. Write all UPSERT statements from the batch.
   c. UPDATE lens_cursors SET cursor = <last_global_sequence>,
                               computed_at = now()
      WHERE lens_name = '<lens>';
   d. COMMIT.
9. Repeat from step 6 until no more events.
10. Switch to incremental mode: process live events as they arrive.
```

Rebuild is idempotent. Running it twice produces the same result because
each step begins with a full truncation.

#### Incremental vs full rebuild decision

| Trigger | Action |
|---|---|
| New lens deployed (no existing cursor) | Full rebuild from global_sequence = 1 |
| Lens schema version incremented | Full rebuild (drop + rebuild) |
| Corruption detected (lens row count != expected) | Full rebuild |
| Process restart with existing cursor | Incremental from stored cursor |
| Operator `lens rebuild --lens <name>` command | Full rebuild |
| Operator `lens resume --lens <name>` command | Incremental from stored cursor |

#### Consistency checking

After an incremental update, the lens engine runs a lightweight consistency
check on a configurable subset of lens rows:

```rust
/// Spot-check that lens_run_status is consistent with run_events.
/// Samples `sample_size` run IDs and verifies their status matches
/// what full replay would produce.
pub async fn spot_check_run_status_lens(
    db: &Pool,
    sample_size: usize,
) -> ConsistencyReport {
    // 1. Sample random run_ids from lens_run_status.
    // 2. For each sampled run, replay its events (from run_events) and
    //    compute expected status.
    // 3. Compare computed status with lens row.
    // 4. Return ConsistencyReport { checked, mismatches, duration }.
}
```

Full consistency verification is triggered automatically after a crash
recovery or backup restore. It compares row counts between the event log
and the lens tables and spot-checks a sample of records.

#### Lenses as materialized views

Conceptually, each lens table is a SQLite materialized view — a denormalized
query result that is refreshed incrementally as new events arrive. Unlike a
SQL `CREATE VIEW`, a lens table:

- Is stored on disk and survives restarts.
- Is rebuilt from events when dropped or corrupted.
- Has an explicit cursor tracking how far it has consumed the event stream.
- Is owned by one projection task; no other writer touches its rows.

The lens projection task runs in a dedicated `tokio` task per lens and
receives events from the in-process event bus. It processes events in order,
committing batches to the database. Backpressure from a slow lens does not
block event production; the lens simply falls behind and catches up from
the event store.

---

## APPENDIX B: OBSERVABILITY STACK

### B.1 Metrics

#### OpenTelemetry integration

Polkagent uses the `opentelemetry` crate family for metrics, with
`opentelemetry-prometheus` exposing a Prometheus-compatible scrape endpoint.
The `metrics` facade crate is used for ergonomic instrument registration.

```rust
use opentelemetry::metrics::{Counter, Histogram, ObservableGauge};
use opentelemetry_sdk::metrics::SdkMeterProvider;

/// Initialise the global meter provider.
/// Call once at process start before any instrument is registered.
pub fn init_metrics(config: &MetricsConfig) -> SdkMeterProvider {
    let exporter = opentelemetry_prometheus::exporter()
        .with_registry(prometheus::default_registry().clone())
        .build()
        .expect("prometheus exporter");

    SdkMeterProvider::builder()
        .with_reader(exporter)
        .with_resource(Resource::new(vec![
            KeyValue::new("service.name", "polkagent"),
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
        ]))
        .build()
}
```

#### Key metric definitions

The table below lists every required metric with its full name, type,
label set, and histogram bucket configuration where applicable.

```rust
/// All Polkagent metric instruments. Constructed once and cloned cheaply.
pub struct PolkagentMetrics {
    // ── run lifecycle ───────────────────────────────────────────────────
    pub runs_created_total:    Counter<u64>,
    pub runs_completed_total:  Counter<u64>,
    pub runs_failed_total:     Counter<u64>,
    pub runs_cancelled_total:  Counter<u64>,
    pub runs_active:           ObservableGauge<u64>,
    /// Labels: scope, outcome ("completed"|"failed"|"cancelled"|"timed_out")
    pub run_duration_seconds:  Histogram<f64>,

    // ── effect lifecycle ────────────────────────────────────────────────
    pub effects_created_total:   Counter<u64>,
    pub effects_completed_total: Counter<u64>,
    pub effects_failed_total:    Counter<u64>,
    pub effects_retried_total:   Counter<u64>,
    /// Labels: scope, effect_type
    pub effect_duration_seconds: Histogram<f64>,

    // ── model usage ─────────────────────────────────────────────────────
    /// Labels: scope, model_id, provider_id
    pub model_requests_total:       Counter<u64>,
    pub model_tokens_input_total:   Counter<u64>,
    pub model_tokens_output_total:  Counter<u64>,
    /// Cost in micro-USD (integer to avoid float issues in counters)
    pub model_cost_micro_usd_total: Counter<u64>,
    /// Labels: scope, model_id, provider_id
    pub model_time_to_first_token_seconds: Histogram<f64>,
    pub model_request_duration_seconds:    Histogram<f64>,

    // ── tool / step ─────────────────────────────────────────────────────
    /// Labels: scope, tool_id
    pub tool_invocations_total:  Counter<u64>,
    pub tool_duration_seconds:   Histogram<f64>,
    pub tool_errors_total:       Counter<u64>,

    // ── artifact storage ────────────────────────────────────────────────
    /// Labels: scope, kind
    pub artifacts_created_total: Counter<u64>,
    /// Gauge: total bytes in artifact_bodies + filesystem blobs
    pub artifacts_stored_bytes:  ObservableGauge<u64>,
    pub blobs_stored_bytes:      ObservableGauge<u64>,

    // ── event throughput ────────────────────────────────────────────────
    /// Labels: durability ("durable"|"diagnostic"|"ephemeral")
    pub events_produced_total: Counter<u64>,
    pub events_consumed_total: Counter<u64>,
    pub events_dropped_total:  Counter<u64>,

    // ── stream connections ──────────────────────────────────────────────
    pub websocket_connections_active: ObservableGauge<u64>,
    pub sse_connections_active:       ObservableGauge<u64>,

    // ── database ────────────────────────────────────────────────────────
    /// Labels: operation ("read"|"write"), table
    pub db_queries_total:             Counter<u64>,
    pub db_query_duration_seconds:    Histogram<f64>,
    pub db_transactions_total:        Counter<u64>,
    pub db_transaction_duration_seconds: Histogram<f64>,

    // ── approval ────────────────────────────────────────────────────────
    pub approvals_requested_total: Counter<u64>,
    pub approvals_granted_total:   Counter<u64>,
    pub approvals_denied_total:    Counter<u64>,
    pub approval_wait_seconds:     Histogram<f64>,

    // ── chain ────────────────────────────────────────────────────────────
    /// Labels: scope, chain_profile, rpc_method
    pub chain_requests_total:        Counter<u64>,
    pub chain_request_duration_secs: Histogram<f64>,
    pub chain_submissions_total:     Counter<u64>,
    pub finality_wait_seconds:       Histogram<f64>,

    // ── policy ──────────────────────────────────────────────────────────
    pub policy_evaluations_total: Counter<u64>,
    pub policy_denials_total:     Counter<u64>,

    // ── errors ──────────────────────────────────────────────────────────
    /// Labels: scope, component, severity ("error"|"panic")
    pub errors_total: Counter<u64>,

    // ── retention ───────────────────────────────────────────────────────
    pub retention_artifacts_archived_total: Counter<u64>,
    pub retention_artifacts_deleted_total:  Counter<u64>,
    pub retention_bytes_freed_total:        Counter<u64>,
}
```

**Histogram bucket boundaries.** Default bucket sets for common histogram
instruments:

```rust
/// Sub-10 ms operations (artifact inline store, event persist).
const FAST_BUCKETS: &[f64] = &[0.001, 0.002, 0.005, 0.010, 0.020, 0.050, 0.100, 0.250];

/// Human-scale latencies (model TTFT, approval wait, finality wait).
const HUMAN_BUCKETS: &[f64] = &[0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0];

/// Run duration (seconds; spans seconds to hours).
const RUN_DURATION_BUCKETS: &[f64] = &[1.0, 5.0, 15.0, 30.0, 60.0, 180.0, 600.0, 1800.0, 3600.0];
```

#### Prometheus exposition format

The `/metrics` endpoint returns standard Prometheus text format. Example
excerpt:

```text
# HELP polkagent_runs_created_total Total number of runs created.
# TYPE polkagent_runs_created_total counter
polkagent_runs_created_total{scope="workspace-abc"} 142

# HELP polkagent_run_duration_seconds Run duration distribution.
# TYPE polkagent_run_duration_seconds histogram
polkagent_run_duration_seconds_bucket{scope="workspace-abc",outcome="completed",le="1.0"} 18
polkagent_run_duration_seconds_bucket{scope="workspace-abc",outcome="completed",le="5.0"} 71
polkagent_run_duration_seconds_bucket{scope="workspace-abc",outcome="completed",le="+Inf"} 89
polkagent_run_duration_seconds_sum{scope="workspace-abc",outcome="completed"} 312.4
polkagent_run_duration_seconds_count{scope="workspace-abc",outcome="completed"} 89

# HELP polkagent_model_tokens_input_total Total input tokens consumed.
# TYPE polkagent_model_tokens_input_total counter
polkagent_model_tokens_input_total{scope="workspace-abc",model_id="claude-opus-4-6",provider_id="anthropic"} 4200000

# HELP polkagent_effects_active Active (in-flight) effect attempts.
# TYPE polkagent_effects_active gauge
polkagent_effects_active{scope="workspace-abc",effect_type="model"} 3
```

### B.2 Structured Logging

#### Log format (JSON)

All log entries use the JSON structure defined in section 10.4.1. The Rust
implementation uses the `tracing` crate with `tracing-subscriber` configured
for JSON output via `tracing_subscriber::fmt::format::Json`.

```rust
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

pub fn init_logging(config: &LogConfig) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.default_level));

    let json_layer = fmt::layer()
        .json()
        .with_current_span(true)
        .with_span_list(false)   // avoid per-line span list noise
        .with_file(false)        // omit source file from production logs
        .with_line_number(false)
        .flatten_event(true);

    // File writer with rotation
    let file_appender = tracing_appender::rolling::RollingFileAppender::new(
        tracing_appender::rolling::Rotation::DAILY,
        &config.log_dir,
        "polkagent.log",
    );
    let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(filter)
        .with(json_layer.with_writer(file_writer))
        .init();
}
```

#### Log levels and filtering

Per-component log level filtering uses the `EnvFilter` syntax:

```text
# Default info globally, debug for the effect driver, trace for SQLite
RUST_LOG=info,polkagent::runtime::effect_driver=debug,polkagent::db=trace
```

Levels map to syslog severity:

| `tracing` level | Syslog severity | When to use |
|---|---|---|
| `error` | ERROR (3) | Unrecoverable failure; integrity violation; security event |
| `warn`  | WARNING (4) | Recoverable problem; policy denial; retry; degraded state |
| `info`  | INFO (6) | Significant lifecycle event; completed effect; config reload |
| `debug` | DEBUG (7) | Adapter communication; per-request detail |
| `trace` | DEBUG (7) | Per-token stream; per-byte I/O; per-SQLite-query |

#### Context propagation

The `tracing` span hierarchy carries context fields automatically:

```rust
// At run start — all child spans and log entries inherit run_id, scope, etc.
let run_span = info_span!(
    "run",
    run_id          = %run.id,
    correlation_id  = %run.correlation_id,
    scope           = %run.scope_id,
    conversation_id = run.conversation_id.as_deref().unwrap_or(""),
);

// At turn start (child of run span)
let turn_span = info_span!(
    parent: &run_span,
    "turn",
    turn_id     = %turn.id,
    turn_number = turn.turn_number,
);

// At effect attempt start (child of turn span)
let effect_span = info_span!(
    parent: &turn_span,
    "effect_attempt",
    intent_id      = %attempt.intent_id,
    attempt_number = attempt.attempt_number,
    effect_type    = %attempt.effect_type,
    idempotency_key = %attempt.idempotency_key,
);
```

#### Secret redaction

Redaction is enforced at the tracing-layer boundary (REQ-OBS-025a). The
following pattern illustrates the correct approach:

```rust
// CORRECT: pass only the artifact ID, not the key material.
tracing::debug!(
    artifact_id = %signing_key_artifact_id,
    "signing key resolved from secret store"
);

// CORRECT: redacted placeholder for external references.
tracing::info!(
    api_key  = "[REDACTED]",
    provider = %provider_id,
    "provider client initialised"
);

// WRONG — never do this:
// tracing::debug!(api_key = %raw_api_key, "provider client initialised");
```

A compile-time lint (custom `clippy` lint or `deny` macro) flags any field
named `key`, `secret`, `mnemonic`, `seed`, `token`, or `password` that is
not wrapped in a `Redacted<T>` newtype.

```rust
/// Newtype that redacts its value in all Display/Debug/tracing output.
pub struct Redacted<T>(T);

impl<T> std::fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl<T> std::fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Redacted(..)")
    }
}
```

### B.3 Tracing

#### Distributed tracing with OpenTelemetry

Polkagent uses the `tracing-opentelemetry` bridge to connect `tracing`
instrumentation to the OpenTelemetry SDK. This produces W3C Trace Context
compatible traces exported to any OTLP-compatible backend.

```rust
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_opentelemetry::OpenTelemetryLayer;

pub fn init_tracing(config: &TracingConfig) -> SdkTracerProvider {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&config.otlp_endpoint)  // e.g., "http://localhost:4317"
        .build()
        .expect("OTLP span exporter");

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(Resource::new(vec![
            KeyValue::new("service.name", "polkagent"),
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
        ]))
        .with_sampler(opentelemetry_sdk::trace::Sampler::TraceIdRatioBased(
            config.sample_rate,  // 0.0–1.0; default 1.0 (sample all)
        ))
        .build();

    tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(
            provider.tracer("polkagent"),
        ))
        .init();

    provider
}
```

#### Span hierarchy

```text
run (trace root)
├── turn
│   ├── model_request
│   │   └── [streaming chunks — not separate spans; added as events]
│   ├── tool_invocation
│   │   └── [tool-specific child spans if the tool itself instruments]
│   └── effect_attempt            ← one per external I/O
│       ├── chain_rpc             ← if effect is a chain call
│       │   └── rpc_decode
│       ├── signer_request        ← if effect requires signing
│       └── broadcast             ← if effect is transaction broadcast
└── turn (next turn in same run)
    └── ...
```

Span attributes follow `opentelemetry-semantic-conventions` where applicable:

```rust
// On the `run` root span:
span.set_attribute(Key::new("polkagent.run_id").string(run_id.to_string()));
span.set_attribute(Key::new("polkagent.scope_id").string(scope_id.to_string()));
span.set_attribute(Key::new("polkagent.correlation_id").string(correlation_id.to_string()));

// On the `model_request` span:
span.set_attribute(Key::new("gen_ai.system").string(provider_id.to_string()));
span.set_attribute(Key::new("gen_ai.request.model").string(model_id.to_string()));
span.set_attribute(Key::new("gen_ai.usage.input_tokens").i64(tokens_input as i64));
span.set_attribute(Key::new("gen_ai.usage.output_tokens").i64(tokens_output as i64));
span.set_attribute(Key::new("polkagent.cost.estimated_micro_usd").i64(cost_micro_usd));

// On the `effect_attempt` span:
span.set_attribute(Key::new("polkagent.effect_type").string(effect_type.to_string()));
span.set_attribute(Key::new("polkagent.intent_id").string(intent_id.to_string()));
span.set_attribute(Key::new("polkagent.attempt_number").i64(attempt_number as i64));
```

#### Trace sampling strategy

| Environment | Default sample rate | Rationale |
|---|---|---|
| Local development | 1.0 (100%) | All traces useful; low volume |
| Staging | 1.0 | Full coverage for pre-production verification |
| Production (low-volume) | 1.0 | Complete audit trail; few concurrent users |
| Production (high-volume managed) | 0.1 (10%) + always-sample on error | Cost control; errors always captured |

Head-based sampling is the default. Tail-based sampling (Jaeger adaptive,
OpenTelemetry Collector tail-sampling processor) is recommended for
high-volume managed deployments where cost is a constraint.

#### Trace export backends

| Backend | Config key | Protocol | Use case |
|---|---|---|---|
| Jaeger (all-in-one) | `tracing.backend = "jaeger"` | OTLP/gRPC | Local development |
| OTLP collector | `tracing.backend = "otlp"` | OTLP/gRPC or HTTP | Production; any backend |
| Console (stdout) | `tracing.backend = "console"` | Text | Development; CI |
| Disabled | `tracing.backend = "none"` | — | Minimal deployments |

---

## APPENDIX C: FILE WATCHING & STREAMING

### C.1 Pattern origin

Roko's TUI uses two complementary modules for real-time data from the
`.roko/` data directory:

- `jsonl_tailer.rs` — Typed, accumulating incremental reader for
  append-only JSONL files. Maintains a byte offset cursor so only
  newly-appended lines are deserialized on each tick.

- `fs_watch.rs` — Filesystem event watcher that coalesces bursts of
  kernel notifications into a single `FsRefresh::Coalesced` signal,
  with automatic fallback to a 1-second poll thread when `notify`
  cannot initialize.

Polkagent adopts both patterns for watching its `.polkagent/` data
directory and streaming durable events to the TUI.

### C.2 JSONL tailer for incremental reading

Roko's `IncrementalTailer<T>` pattern
(`/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/jsonl_tailer.rs`):

```rust
/// Accumulating, incremental reader for typed JSONL files.
/// Wraps a JsonlCursor and deserializes each new line into T.
pub struct IncrementalTailer<T> {
    cursor: JsonlCursor,    // tracks byte offset into the file
    items: Vec<T>,          // all successfully parsed items so far
    pub parse_errors: usize,
}

impl<T: serde::de::DeserializeOwned> IncrementalTailer<T> {
    /// Read only newly appended lines since the last tick.
    /// Returns count of newly parsed items.
    /// On file truncation: clears accumulator and resyncs from start.
    pub fn tick(&mut self) -> std::io::Result<usize> { ... }
}
```

Key properties inherited by the Polkagent adaptation:
- O(new bytes) per tick instead of O(file size) — critical for long-running
  runs that accumulate thousands of events.
- Truncation detection: if the file is rotated or truncated, the cursor
  resets and the accumulator is cleared; the TUI resyncs from the new start.
- Malformed lines are skipped and counted (`parse_errors`) rather than
  causing a hard failure.

**Polkagent adaptation for `.polkagent/events/`:**

```rust
use polkagent_tui::tailer::IncrementalTailer;

// One tailer per JSONL shard file in .polkagent/events/<run_id>.jsonl
pub struct RunEventTailer {
    tailer: IncrementalTailer<RunEvent>,
    run_id: RunId,
}

impl RunEventTailer {
    pub fn tick(&mut self) -> std::io::Result<Vec<RunEvent>> {
        let n = self.tailer.tick()?;
        if n == 0 {
            return Ok(vec![]);
        }
        // Return only the newly appended slice.
        let total = self.tailer.len();
        Ok(self.tailer.items()[total - n..].to_vec())
    }
}
```

**JSONL shard naming convention:**

```text
.polkagent/
  events/
    <run_id>.jsonl          -- durable events for a single run
    _system.jsonl           -- system-level events (startup, shutdown, backup)
  artifacts/
    index.jsonl             -- artifact metadata index (tail for ArtifactIndexLens)
  diagnostics/
    <run_id>.jsonl          -- diagnostic events (shorter retention)
```

### C.3 Filesystem watcher for `.polkagent/`

Roko's `fs_watch.rs`
(`/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/fs_watch.rs`)
provides:

- **`watch_roko_dir(workdir)`:** Uses `notify` + `notify-debouncer-full`
  to watch the directory recursively and coalesce bursts of filesystem
  events into a single `FsRefresh::Coalesced` signal over a bounded
  `mpsc::sync_channel(4)`. The 200 ms debounce window prevents flooding
  the TUI event loop during rapid writes.

- **`watch_roko_dir_with_fallback(workdir)`:** Falls back to a dedicated
  poll thread (1-second interval, thread name `tui-fs-poll-fallback`)
  when `notify` cannot initialize. The poll thread computes a fingerprint
  hash of the directory tree (modification times + sizes + path hashes)
  and emits a refresh signal on change.

**Polkagent adaptation:**

```rust
/// Watch .polkagent/ recursively for any filesystem change.
/// Returns an FsWatchHandle whose rx channel yields FsRefresh::Coalesced.
pub fn watch_polkagent_dir(data_dir: &Path) -> FsWatchHandle {
    // Identical contract to Roko's watch_roko_dir_with_fallback,
    // watching $POLKAGENT_DATA_DIR instead of .roko/.
    watch_dir_with_fallback(data_dir, Duration::from_millis(200))
}
```

The TUI event loop integrates the watcher and tailers as follows:

```rust
// In the TUI tick handler:
pub fn on_tick(&mut self) {
    // 1. Check for FS refresh signal (non-blocking).
    while let Ok(FsRefresh::Coalesced) = self.fs_watch.try_recv() {
        // 2. For each active run, tick its event tailer.
        for tailer in &mut self.run_tailers {
            if let Ok(new_events) = tailer.tick() {
                self.state.apply_events(new_events);
            }
        }
        // 3. Tick the artifact index tailer.
        if let Ok(new_metas) = self.artifact_tailer.tick() {
            self.state.apply_artifact_meta(new_metas);
        }
    }
    // 4. Render frame.
    self.render();
}
```

### C.4 Real-time event streaming to TUI

The streaming pipeline from the Polkagent daemon to the TUI has two modes:

**Direct-file mode (local, single-process):**

```text
Daemon writes event to run_events table
    └── Daemon also appends JSON line to .polkagent/events/<run_id>.jsonl
            └── notify kernel event → FsRefresh::Coalesced → TUI tick
                    └── IncrementalTailer::tick() reads new lines → state update → render
```

**WebSocket mode (daemon + separate TUI process):**

```text
Daemon produces durable event
    └── WebSocket push to TUI client
            └── TUI receives JSON event → state update → render
        (fallback: cursor-based pull on reconnect)
```

The JSONL sidecar files in `.polkagent/events/` are written alongside the
SQLite database write in the same daemon task, so they are always consistent
with committed events. They are not the authoritative store (that is the
SQLite database) but serve as a low-overhead streaming channel to local TUI
consumers.

### C.5 Backpressure when consumer is slow

When the TUI tick rate (typically 50–100 ms intervals) cannot keep pace with
the event production rate:

1. **FS watch channel saturation.** The `mpsc::sync_channel(4)` used by the
   watcher bounds the queue to 4 coalesced signals. Additional signals are
   dropped via `try_send`. This is safe: the next tick will read all
   accumulated new lines from the JSONL file, so no events are lost.
   The JSONL file is the buffer.

2. **IncrementalTailer batching.** A single `tick()` call reads all
   newly-appended lines since the last call. If 500 events were written
   between ticks, all 500 are read in one batch. The TUI applies them all
   to state and renders once, avoiding per-event render overhead.

3. **WebSocket buffer overflow.** For WebSocket consumers, the server-side
   per-connection buffer (256 events, section 9.4) absorbs bursts. On
   overflow, ephemeral events are dropped first; durable events are
   never dropped. A slow client disconnected by backpressure reconnects
   with its last `global_sequence` cursor and catches up from the store.

4. **TUI render throttle.** The render loop is decoupled from the event
   ingestion loop. Events are applied to state on every tick; the frame
   is rendered at a fixed rate (configurable, default 50 ms). Multiple
   state updates between renders are collapsed into one frame.

---

## APPENDIX D: IMPLEMENTATION CHECKLIST

Tasks are ordered by dependency (earlier tasks unblock later ones). Each
task lists its acceptance criterion from section 20 where applicable.

### D.1 SQLite schema and database layer

- [ ] **DB-01** Create `schema_migrations` table and migration runner.
  Run all DDL from Appendix A.1 as migration version 1. Verify
  `PRAGMA foreign_keys = ON` and `PRAGMA journal_mode = WAL` are set
  on every connection.
  _Acceptance: migration applies idempotently; second run is a no-op._

- [ ] **DB-02** Implement the single-writer task architecture: one
  dedicated Tokio task owns the write connection; read connections are
  served from a pool (default size: 4). All write transactions use
  `BEGIN IMMEDIATE`.
  _Acceptance: 50 concurrent read tasks + 1 write task produce zero
  `SQLITE_BUSY` errors under sustained load._

- [ ] **DB-03** Implement `ArtifactStore`: `create`, `get_meta`,
  `get_body`, `list`, `soft_delete`, `update_labels`, `update_tier`.
  Enforce immutability trigger (Appendix A.1).
  _Acceptance: AC-ART-01, AC-ART-03._

- [ ] **DB-04** Implement artifact lineage: `add_parents`, `ancestors`,
  `descendants`, `lineage_chain`.
  _Acceptance: AC-ART-02 (7-level chain query < 50 ms p99)._

- [ ] **DB-05** Implement blob storage: filesystem sharding for bodies
  > 256 KB; inline storage for bodies ≤ 256 KB.
  _Acceptance: AC-ART-04, AC-ART-05 (deduplication within/across
  classification boundaries)._

- [ ] **DB-06** Implement content-addressed deduplication (same digest +
  same kind → shared blob, separate metadata rows).
  _Acceptance: AC-ART-04, AC-ART-05._

- [ ] **DB-07** Implement retention enforcement job: scan `artifacts`
  for expired rows; soft-delete; schedule hard-delete of blobs.
  _Acceptance: AC-ART-08, AC-ART-09._

- [ ] **DB-08** Implement integrity verification: on blob retrieval,
  recompute SHA-256 and compare. On mismatch emit `IntegrityViolation`
  event and quarantine.
  _Acceptance: AC-ART-10._

### D.2 Event system

- [ ] **EVT-01** Implement `EventStore`: `append_durable`, `append_diagnostic`,
  `read_from_cursor`, `read_run_events`. Enforce single-terminal-event
  trigger and unique `(run_id, sequence)` constraint.
  _Acceptance: AC-EVT-01, AC-EVT-02, AC-EVT-03, AC-EVT-04._
  _Partial evidence (2026-08-06): SQLite V19 and the PostgreSQL event adapter
  retain the full stored envelope and component correlation IDs. SQLite has
  exact legacy-migration, null/populated round-trip, diagnostic normalization,
  canonical API replay, and fail-closed corruption tests. Live PostgreSQL
  migration/conformance, diagnostic retention, and the broader acceptance
  matrix remain open._

- [ ] **EVT-02** Implement `global_sequence` assignment in the
  single-writer task using SQLite `ROWID` auto-increment or an explicit
  sequence counter table.
  _Acceptance: AC-EVT-03 (global sequence strictly increasing)._

- [ ] **EVT-03** Implement in-process event bus with
  `tokio::sync::broadcast`. Backpressure policy: durable blocks, diagnostic
  uses ring buffer, ephemeral drops immediately (section 9.1).
  _Acceptance: AC-EVT-08._

- [ ] **EVT-04** Implement JSONL sidecar writer: for every committed
  durable event, append a JSON line to `.polkagent/events/<run_id>.jsonl`
  within the same writer task (after the DB commit).
  _No separate acceptance criterion; dependency for C.4._

- [ ] **EVT-05** Implement event migration registry (Appendix A.2):
  read-time upgrade, dead-letter table, `UnknownEventPolicy::DeadLetter`.
  _Acceptance: replay across schema version boundaries without hard failure._

- [ ] **EVT-06** Implement cursor-based event subscription: pull API
  (`read_from_cursor`) and push via WebSocket (section 9.3).
  _Acceptance: AC-EVT-05, AC-EVT-07._
  _Partial evidence (2026-08-06): the global API WebSocket passes bounded
  replay, reconnect, filter, concurrent follow, forced-lag, dedupe, auth, and
  sanitized failure fixtures, including canonical recorder-to-SQLite payload
  projection and rowid checkpoint proof. The command WebSocket separately
  passes real-TCP run/agent multi-subscription, unsubscribe, subscription-cap,
  forced-lag recovery/dedupe, pre-checkpoint fail-closed, invalid-projection,
  missing-store, and sanitized-backend fixtures. It now also passes initial
  replay, versioned reconnect, filtering across pages, dedupe, lag/reconnect
  interaction, and malformed/stale/future cursor fixtures. The remaining
  section 9.3/9.4 requirements above keep this item open._

### D.3 Artifacts

- [ ] **ART-01** Implement secret-detection scanner using regex patterns
  for private keys, mnemonics, API tokens (section 16.3).
  _Acceptance: AC-CLS-03._

- [ ] **ART-02** Implement inherited classification: at `create` time,
  compute `max(kind_default, max(parent_classifications))`.
  _Acceptance: AC-CLS-01, AC-CLS-02._

- [ ] **ART-03** Implement `ProviderOracle` for artifact provenance
  (section 6.3): `ChainEvidence` required for chain-related kinds.
  _Acceptance: chain artifact without ChainEvidence is rejected at store
  boundary._

- [ ] **ART-04** Implement archival: move blob from hot filesystem tier
  to cold tier; update `storage_tier` column in `artifacts`.
  _Acceptance: AC-ART-07._

### D.4 Projections (lenses)

- [ ] **LENS-01** Implement projection engine: per-lens Tokio task,
  batch event processing, `lens_cursors` table, UPSERT pattern.
  _Acceptance: AC-REC-04 (drop + rebuild produces identical state)._

- [ ] **LENS-02** Implement `RunStatusLens`: handles `RunCreated`,
  `RunStarted`, `RunCompleted`, `RunFailed`, `RunCancelled`, `RunTimedOut`,
  `TurnStarted`, `TurnCompleted`, `EffectIntentCreated`,
  `EffectOutcomeRecorded`.

- [ ] **LENS-03** Implement `EffectStatusLens`: handles all `Effect*`
  events.

- [ ] **LENS-04** Implement `ApprovalLens`: handles `Approval*` events.

- [ ] **LENS-05** Implement `UsageLens`: handles `EffectOutcomeRecorded`
  events for model effects; aggregates into hourly buckets.

- [ ] **LENS-06** Implement `ArtifactIndexLens`: handles
  `EffectOutcomeRecorded` + artifact creation events.

- [ ] **LENS-07** Implement `HealthLens` and `PolicyAuditLens`.

- [ ] **LENS-08** Implement full-rebuild and incremental-resume paths.
  Consistency spot-check after crash recovery (Appendix A.3).
  _Acceptance: AC-REC-04._

### D.5 Metrics

- [ ] **MET-01** Initialize `SdkMeterProvider` with Prometheus exporter.
  Expose `/metrics` endpoint on configurable port (default: 9090).
  _Acceptance: AC-OBS-01._

- [ ] **MET-02** Instrument all code paths to produce the metrics defined
  in Appendix B.1. Wire `PolkagentMetrics` into each subsystem.
  _Acceptance: AC-OBS-01 (all families present in scrape output)._

- [ ] **MET-03** Add OTLP metrics export path (optional, configured).

### D.6 Logging

- [ ] **LOG-01** Initialize JSON structured logging via `tracing-subscriber`
  with file rotation. Configure per-component level filtering via
  `EnvFilter`.

- [ ] **LOG-02** Implement `Redacted<T>` newtype. Enforce redaction
  at call sites using the compile-time lint pattern.
  _Acceptance: AC-OBS-04, AC-CLS-04._

- [ ] **LOG-03** Verify that context propagation (`tracing` span
  hierarchy) attaches `run_id`, `correlation_id`, `trace_id`, `span_id`
  to every log entry within a run.
  _Acceptance: AC-OBS-03._

### D.7 Tracing

- [ ] **TRC-01** Initialize `SdkTracerProvider` with OTLP exporter.
  Wire `tracing-opentelemetry` layer into `tracing_subscriber::registry`.
  _Acceptance: AC-OBS-02._

- [ ] **TRC-02** Instrument `run`, `turn`, `model_request`,
  `tool_invocation`, `effect_attempt`, `chain_rpc`, `signer_request`
  spans with mandatory attributes (section 10.3 + Appendix B.3).
  _Acceptance: AC-OBS-02 (expected span tree exported to backend)._

- [ ] **TRC-03** Implement W3C Trace Context propagation for multi-agent
  calls (REQ-OBS-015b): inject `traceparent`/`tracestate` on outbound;
  extract and use as parent on inbound.

- [ ] **TRC-04** Verify that every durable event records the active
  `trace_id` and `span_id` from the current `tracing` span.
  _Acceptance: AC-OBS-03._

---

## APPENDIX E: REFERENCE FILE MAP

| Component | Roko reference files | Bardo reference files | Key patterns adopted |
|---|---|---|---|
| Incremental JSONL reader | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/jsonl_tailer.rs` — `IncrementalTailer<T>`: byte-offset cursor, truncation detection, per-tick O(new-bytes) read | — | `RunEventTailer`, artifact index tailer in Polkagent TUI |
| Filesystem watcher | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/fs_watch.rs` — `FsWatchHandle`, `watch_roko_dir_with_fallback`, debounce 200 ms, poll fallback 1 s | — | `watch_polkagent_dir` watching `$POLKAGENT_DATA_DIR` |
| Health dashboard | — | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/soma/health.rs` — `HealthScreen`, labeled key/value pairs, tick counter, ratatui `Block`/`Paragraph` layout | Polkagent health view (run count, active effects, error rate) |
| Log viewer | — | `/Users/will/dev/uniswap/bardo/apps/mori/src/tui/views/logs.rs` — `LogLevel` color mapping, timestamp prefix, event icon detection, source accent by role, scrollbar integration | Polkagent log viewer with level filtering and event type icons |
| System metrics | — | `/Users/will/dev/uniswap/bardo/apps/mori/src/tui/widgets/sys_metrics.rs` — CPU/MEM gauge + braille sparkline, NET/DSK rate rows, FPS row, gateway status row, top-procs table | Polkagent system panel: CPU, MEM, DB size, events/s |
| Event sourcing | — | — | Section 8 + Appendix A.2: versioned events, migration functions, dead-letter table |
| Projection engine | — | — | Appendix A.3: cursor-based rebuild, incremental update, consistency checking |
| Metrics | — | — | Appendix B.1: `PolkagentMetrics`, `opentelemetry-prometheus`, histogram buckets |
| Tracing | — | — | Appendix B.3: `tracing-opentelemetry`, OTLP export, span hierarchy |
| Secret redaction | — | — | Appendix B.2: `Redacted<T>`, tracing-boundary enforcement |

---

## APPENDIX F: TUI SURFACE FOR OBSERVABILITY

All TUI views use `ratatui` (the same library as Roko and Bardo). Each view
is a distinct screen or panel, toggled by keyboard shortcuts. The wireframes
below use 80-column ASCII.

### F.1 Health dashboard

Inspired by Bardo's `health.rs` (`/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/soma/health.rs`),
which renders labeled key/value rows inside a bordered block with tick and
cycle counters. The Polkagent health dashboard extends this to show agent
system vitals, uptime, active run counts, resource usage, and component
status derived from the `HealthLens`.

```
┌─ Polkagent / Health ─────────────────────────────────────────────────────┐
│                                                                           │
│  Uptime       3h 42m 18s                                                 │
│  Runs         Active: 3  Completed: 142  Failed: 2                       │
│  Effects      In-flight: 7  Completed today: 891  Retries: 14           │
│  Events       Produced: 12,400/s  Queue depth: 0                         │
│  Approvals    Pending: 1  ⚠ awaiting decision                            │
│                                                                           │
│  ── components ──────────────────────────────────────────────────────── │
│  effect-driver      ● ok      p99: 4ms    errors/h: 0                   │
│  artifact-store     ● ok      p99: 8ms    errors/h: 0                   │
│  event-bus          ● ok      p99: 1ms    errors/h: 0                   │
│  chain-client       ● ok      p99: 310ms  errors/h: 0                   │
│  lens-engine        ● ok      lag: 12ms   errors/h: 0                   │
│  retention-job      ● ok      last: 14m ago                              │
│                                                                           │
│  ── storage ─────────────────────────────────────────────────────────── │
│  DB             1.2 GB / 10 GB  ▓▓░░░░░░░░  12%                        │
│  Blobs (hot)    8.4 GB / 50 GB  ▓▓░░░░░░░░  17%                        │
│  Logs           420 MB / 1 GB   ▓▓▓▓░░░░░░  42%                        │
│                                                                           │
└──────────────────────────────── [h]ealth [l]ogs [m]etrics [r]uns ───────┘
```

**State source:** `HealthLens` table + Prometheus metrics endpoint.
**Refresh:** every TUI tick (50 ms) or on `FsRefresh::Coalesced`.
**Key bindings:** `Tab`/`BackTab` to navigate screens; `q` to quit.

### F.2 Log viewer with filtering

Inspired by Bardo's `logs.rs` (`/Users/will/dev/uniswap/bardo/apps/mori/src/tui/views/logs.rs`),
which renders a scrollable, color-coded log list with timestamp prefix,
level icon, source label (colored by component role), and event icon
detection. The Polkagent log viewer adds level filtering, run_id scoping,
and component filtering.

```
┌─ Logs (2,341) ──────────────────── filter: info+ │ run: 01J5RUN123 ──────┐
│ 14:23:01 · [effect-driver ] Effect attempt completed                 ✓   │
│ 14:23:01 · [artifact-store] Artifact stored  kind=DecodedCall id=01J… ·  │
│ 14:23:00 · [chain-client  ] RPC call completed  method=state_call   ·    │
│ 14:22:59 ‥ [effect-driver ] Lease renewed  intent=01J5EFF… ttl=30s  ⟲   │
│ 14:22:58 · [turn-engine   ] Turn 2 started                          →    │
│ 14:22:55 · [model-request ] TTFT 820ms  model=claude-opus-4-6       ·    │
│ 14:22:54 · [effect-driver ] Effect intent created  type=model       ·    │
│ 14:22:53 · [turn-engine   ] Turn 1 completed  duration=12.4s        ✓    │
│ 14:22:41 ⚠ [chain-client  ] RPC retry 1/3  method=state_call       ⟲    │
│ 14:22:40 · [run-engine    ] Run started  run_id=01J5RUN123          →    │
│ ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░│ ▓ │
│                                                                       │   │
│                                                                       │   │
│                                                                       │   │
│                                                                       │ ░ │
└──── [↑↓] scroll  [f] filter level  [r] filter run  [c] clear ────────────┘
```

**Level icons** (matching Bardo's `logs.rs` pattern):

| Level | Icon | Color |
|---|---|---|
| `error` | `✗` | Red (ember) |
| `warn`  | `⚠` | Yellow (warning) |
| `info`  | `·` | Foreground dim |
| `debug` | `‥` | Text dim |

**Event detection icons** (appended to log line):

| Pattern in message | Icon |
|---|---|
| `completed`, `ok`, `APPROVE` | ` ✓` |
| `entering`, `started`, `phase:` | ` →` |
| `retry`, `iteration`, `renewal` | ` ⟲` |
| `gate`, `policy`, `grant` | ` ⚙` |

**State source:** `IncrementalTailer<LogEntry>` watching
`.polkagent/logs/polkagent.log`.
**Filter state:** held in TUI state; filters applied before rendering.

### F.3 System metrics

Inspired by Bardo's `sys_metrics.rs` (`/Users/will/dev/uniswap/bardo/apps/mori/src/tui/widgets/sys_metrics.rs`),
which renders CPU, MEM, NET, DSK rows with animated gauge fills and braille
sparklines, plus FPS counter and gateway status. The Polkagent system panel
replaces the gateway row with DB metrics.

```
┌─ System ──────────────────────────────────────────────────────────────────┐
│ CPU   23.1%  ▓▓▓░░░░░░░  ⠁⠃⠇⡇⣇⣿⡇⣇⡇⠁                                  │
│ MEM   4.2GB  ▓▓▓▓░░░░░░  ⡇⡇⣇⣇⣿⣿⣿⣿⣿⣇                                  │
│ NET   ↑12K               ⠁⠁⠃⠁⠁⠁⠃⠁⠁⠁                                  │
│ DSK   R180K              ⠁⠁⠁⠁⠃⣿⠃⠁⠁⠁                                  │
│ FPS    49.8                                                               │
│ DB    1.2GB  WAL:0.2MB   ⠁⠁⠃⠁⠁⠁⠁⠃⠁⠁                                  │
│ EVT   8,420/s  drop:0    ⡇⡇⣇⣿⡇⣿⡇⡇⣇⣿                                  │
│── active runs ────────────────────────────────────────────────────────── │
│ 01J5RUN1…  working    turn 3/~   12.4s  model=claude-opus-4-6           │
│ 01J5RUN2…  approval   awaiting   45.2s  ⚠ waiting for user              │
│ 01J5RUN3…  working    turn 1/~    2.1s  model=claude-opus-4-6           │
└───────────────────────────────────────────────────────────────────────────┘
```

**Polkagent-specific rows:**
- `DB`: SQLite database file size + WAL file size with sparkline of write
  rate (transactions/s).
- `EVT`: Events produced per second + dropped count, with sparkline.

**State source:** `sysinfo` crate for CPU/MEM/NET/DSK; Prometheus metrics
for DB and EVT rows; `RunStatusLens` for active runs table.

### F.4 Run timeline visualization

Events for a single run plotted on a time axis. Phase transitions are
marked with icons; effect sub-timelines are indented below each turn.

```
┌─ Run Timeline: 01J5RUN123 ────────────────────────── 14:22:40 → 14:23:12 ┐
│                                                                           │
│  14:22:40  ●  Run started                                                │
│  14:22:40  │  ├─ Turn 1 ─────────────────────────── 12.4s ────────────  │
│  14:22:40  │  │  · model_request  claude-opus-4-6   820ms TTFT           │
│  14:22:55  │  │  · effect: chain_rpc  state_call    310ms ok             │
│  14:22:55  │  │  ⟲ effect: chain_rpc  state_call    retry (RPC error)   │
│  14:22:58  │  │  · effect: chain_rpc  state_call    280ms ok             │
│  14:23:01  │  │  · artifact: DecodedCall stored                          │
│  14:23:01  │  └─ Turn 1 completed                                        │
│  14:23:01  │  ├─ Turn 2 ─────────────────────────── in progress ───────  │
│  14:23:01  │  │  · model_request  claude-opus-4-6   … streaming          │
│  14:23:12  │  │  ⚠ effect: sign  approval required                      │
│  14:23:12  │  └─ [awaiting approval]                                     │
│                                                                           │
│  Duration: 32s  │  Turns: 2  │  Effects: 4 (1 pending)                  │
│  [↑↓] scroll  [e] expand effect  [a] artifact list  [q] back            │
└───────────────────────────────────────────────────────────────────────────┘
```

**State source:** `RunStatusLens` + `EffectStatusLens` + JSONL event tailer
for live updates.
**Live update:** events from `IncrementalTailer<RunEvent>` append to the
timeline in real time. Completed phases show elapsed time; in-progress
phases show a spinner.

### F.5 Artifact browser

Tree view of artifacts associated with a run or conversation, with content
preview on selection.

```
┌─ Artifacts: 01J5RUN123 (14 artifacts) ────────────────────────────────────┐
│                                                                            │
│  ▼ Evidence chain (chain action)                                          │
│    · DecodedCall          4.1 KB  Private   14:22:58  ✓ verified          │
│    · MetadataSnapshot     2.8 KB  Internal  14:22:55  ✓ verified          │
│    · SimulationResult    12.3 KB  Private   14:22:59  ✓ verified          │
│    · PolicySnapshot       1.2 KB  Sensitive 14:23:00  ✓ verified          │
│    · ApprovalRecord       0.8 KB  Sensitive 14:23:02  ✓ verified          │
│    · SignedPayload         0.5 KB  Sensitive 14:23:03  ✓ verified          │
│    · BroadcastReceipt     0.3 KB  Internal  14:23:10  ✓ verified          │
│    · FinalityObservation  0.4 KB  Internal  14:23:12  ✓ verified          │
│  ▶ Receipt [composite]    2.1 KB  Private   14:23:12  ✓ verified          │
│  ▶ ModelResponse (2)                                                      │
│  ▶ ContextPack (2)                                                        │
│  ▶ ErrorReport (0)                                                        │
│                                                                            │
│  ┌─ Preview: DecodedCall ────────────────────────────────────────────┐    │
│  │  pallet: Balances                                                 │    │
│  │  call:   transfer_allow_death                                     │    │
│  │  dest:   5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY      │    │
│  │  value:  1,000,000,000,000 (1 DOT)                               │    │
│  └───────────────────────────────────────────────────────────────────┘    │
│  [↑↓] navigate  [Enter] expand/preview  [v] verify digest  [x] export    │
└────────────────────────────────────────────────────────────────────────────┘
```

**State source:** `ArtifactIndexLens` for the tree; artifact body fetched
on selection from the artifact store.
**Digest verification:** pressing `v` re-reads the blob and computes SHA-256;
result shown inline.

### F.6 Event stream viewer

Real-time event feed with type filtering, showing the raw event stream for
a run or the global scope.

```
┌─ Events: live │ run: 01J5RUN123 │ filter: Durable ──────── seq: 1,000,042 ┐
│                                                                            │
│  seq:1000042  14:23:12  EffectOutcomeRecorded   attempt=01J5ATT… ok ✓    │
│  seq:1000041  14:23:10  BroadcastReceipt stored artifact=01J5ART…        │
│  seq:1000040  14:23:10  EffectAttemptCompleted  attempt=01J5ATT… 2.1s    │
│  seq:1000039  14:23:08  EffectAttemptStarted    worker=w-1 lease=30s     │
│  seq:1000038  14:23:03  EffectOutcomeRecorded   attempt=01J5ATT… ok ✓    │
│  seq:1000037  14:23:03  SignedPayload stored    artifact=01J5ART…        │
│  seq:1000036  14:23:02  ApprovalGranted         approval=01J5APR…        │
│  seq:1000035  14:23:00  ApprovalRequested       approval=01J5APR… ⚠      │
│  seq:1000034  14:22:59  EffectOutcomeRecorded   attempt=01J5ATT… ok ✓    │
│  seq:1000033  14:22:58  SimulationResult stored artifact=01J5ART…        │
│  ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░│ ▓  │
│                                                                       │   │
│                                                                       │ ░  │
│                                                                       │   │
└──── [↑↓] scroll  [f] filter type  [d] durability  [p] pause  [/] search ─┘
```

**State source:** `IncrementalTailer<RunEvent>` watching
`.polkagent/events/<run_id>.jsonl` for local; WebSocket subscription for
remote.
**Pause mode:** pressing `p` halts live updates and allows scrolling into
history. Resuming sends the accumulated events since pause as a batch.

---

## APPENDIX G: CONFIGURATION GUIDE

All configuration is loaded from `$POLKAGENT_DATA_DIR/config/polkagent.toml`
(TOML format). Environment variables prefixed `POLKAGENT_` override any
config file value. Secrets are never stored in this file; they are resolved
from the platform's secret store at runtime.

### G.1 Database settings

```toml
[database]
# Path to the SQLite database file.
# Default: $POLKAGENT_DATA_DIR/db/polkagent.db
path = "/home/user/.local/share/polkagent/db/polkagent.db"

# SQLite journal mode.
# Always WAL for production; allowed values: WAL (default), DELETE (testing only).
journal_mode = "WAL"

# SQLite synchronous mode.
# NORMAL = fsync on WAL checkpoint only; FULL = fsync on every write.
synchronous = "NORMAL"

# Maximum WAL file size before an automatic checkpoint (in pages; default 1000).
# Increase for write-heavy workloads; decrease to bound WAL file size.
wal_autocheckpoint = 1000

# Read connection pool size (for concurrent reads; default 4).
read_pool_size = 4

# Busy timeout in milliseconds (default 5000).
busy_timeout_ms = 5000

# Inline body threshold: artifact bodies <= this size are stored in the
# artifact_bodies table; larger bodies use the filesystem blob store.
# Default: 262144 (256 KB).
inline_body_threshold_bytes = 262144
```

### G.2 Event retention policies

```toml
[retention]
# How often the retention enforcement job runs (cron expression or interval).
# Default: "0 3 * * *" (3 AM daily).
schedule = "0 3 * * *"

# Default retention durations by tier (ISO 8601 duration strings).
[retention.tiers]
evidence    = "P2Y"     # 2 years  (Receipt, SignedPayload, ApprovalRecord, ...)
operational = "P90D"    # 90 days  (ContextPack, ModelResponse, DiagnosticBundle, ...)
workspace   = "P0Y"     # lifetime of workspace (deleted with workspace)
ephemeral   = "P7D"     # 7 days   (streaming chunks, progress snapshots)

# Per-artifact-kind overrides (kind name → duration).
[retention.kind_overrides]
ModelResponse    = "P30D"   # Override operational default to 30 days
ContextPack      = "P14D"
DiagnosticBundle = "P7D"

# Diagnostic events retention (separate from durable events).
diagnostic_events_ttl = "P7D"

# Minimum retention regardless of other settings (e.g., for compliance).
minimum_retention = "P30D"
```

### G.3 Metrics export configuration

```toml
[metrics]
# Enable the Prometheus /metrics HTTP endpoint.
enabled = true

# Listen address for the metrics server.
listen_addr = "127.0.0.1:9090"

# Enable OTLP metrics export in addition to (or instead of) Prometheus.
otlp_enabled = false
otlp_endpoint = "http://localhost:4317"

# Scrape interval hint for Prometheus (informational; actual scrape
# interval is set on the Prometheus server).
scrape_interval_seconds = 15
```

### G.4 Log level and output settings

```toml
[logging]
# Default log level. Override per component with RUST_LOG.
# Allowed: trace | debug | info | warn | error
default_level = "info"

# Per-component level overrides.
[logging.component_levels]
"polkagent::runtime::effect_driver" = "debug"
"polkagent::db"                     = "info"
"polkagent::chain_client"           = "debug"

[logging.file]
# Directory for log files (default: $POLKAGENT_DATA_DIR/logs/).
dir = "/home/user/.local/share/polkagent/logs"
# Maximum size of a single log file before rotation (bytes).
max_file_size_bytes = 104857600  # 100 MB
# Number of rotated files to retain.
max_files = 10

[logging.console]
# Enable human-readable console output (for development/CLI).
enabled = true
# Format: "pretty" (human-readable) or "json" (structured JSON).
format = "pretty"
```

### G.5 Tracing configuration

```toml
[tracing]
# Tracing backend. Options: "otlp" | "jaeger" | "console" | "none"
backend = "console"

# OTLP gRPC endpoint (used when backend = "otlp" or "jaeger").
otlp_endpoint = "http://localhost:4317"

# Sampling rate (0.0–1.0). 1.0 = sample all traces.
sample_rate = 1.0

# Always sample traces that contain an error span, regardless of sample_rate.
always_sample_on_error = true

# Service name reported to the tracing backend.
service_name = "polkagent"
```

### G.6 `.polkagent/` directory layout

The complete layout of the data directory for a local deployment:

```text
$POLKAGENT_DATA_DIR/           # Default: ~/.local/share/polkagent/
  config/
    polkagent.toml             # Main configuration file
    polkagent.toml.example     # Documented example (never loaded)

  db/
    polkagent.db               # SQLite authority database (WAL mode)
    polkagent.db-wal           # WAL journal (auto-managed by SQLite)
    polkagent.db-shm           # Shared memory file (auto-managed)

  blobs/
    01/                        # Sharded by first 2 chars of artifact ULID
      01J5ABCDEF1234567890.blob
    02/
      02J5...blob
    ...

  events/                      # JSONL sidecar files for TUI streaming
    <run_id>.jsonl             # Durable events for one run
    _system.jsonl              # System-level events

  diagnostics/
    <run_id>.jsonl             # Diagnostic events for one run

  exports/
    <export_id>/
      manifest.json
      artifacts/
      events/
      effects/

  backups/
    2026-07-30T030000Z/
      manifest.json
      polkagent.db             # SQLite snapshot
      blobs/                   # Blob files up to global_sequence watermark

  logs/
    polkagent.log              # Current structured JSON log
    polkagent.log.2026-07-29   # Rotated files (date-stamped)
    polkagent.log.2026-07-28

  tmp/
    uploads/                   # Staging area for in-progress uploads
    scratch/                   # Ephemeral processing scratch space
    # Cleared on daemon startup; never relied upon across restarts.
```

**Directory permissions:**

| Path | Mode | Rationale |
|---|---|---|
| `$POLKAGENT_DATA_DIR/` | `0700` | Only the owning user may read/write |
| `db/` | `0700` | Database files contain all durable state |
| `blobs/` | `0700` | Artifact bodies may contain private/sensitive data |
| `exports/` | `0700` | Exports may contain classified artifacts |
| `backups/` | `0700` | Backup snapshots contain full database |
| `logs/` | `0700` | Logs may reference run/artifact IDs |
| `config/polkagent.toml` | `0600` | Config may reference secret store paths |
