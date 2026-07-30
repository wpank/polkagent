# PRD-09 — Memory, Knowledge, Learning, Multi-Agent Groups and Evals

**Status:** definitive PRD
**Audience:** engineers, product designers, and operators with no prior Polkagent,
Roko, or `polkadot-chat-agents` context
**Date:** 2026-07-30

---

## 1. Purpose and orientation

### 1.1 What this document covers

This PRD defines how Polkagent agents remember, learn, coordinate in groups,
react to events, and improve over time. It covers five interrelated subsystems:

1. **Memory** -- how agents retain and retrieve information across conversations
   and runs, including episodic history, semantic knowledge, and procedural
   workflows.
2. **Knowledge and citations** -- how retained information carries provenance,
   chain-specific metadata, expiry, and source attribution so that stale or
   unverified facts cannot silently influence agent behavior.
3. **Multi-agent groups** -- how multiple agents coordinate under shared budgets
   and grant intersection, with parent/child run relationships, cancellation
   propagation, and evidence aggregation.
4. **Feeds, triggers, and recipes** -- how agents react to external events
   (chain state changes, schedules, webhooks, messages) through durable,
   idempotent, cursor-backed processing.
5. **Evaluation and self-improvement** -- how agent performance is measured,
   compared, and used to propose (never silently enact) prompt, routing, and
   skill improvements.

### 1.2 Why these subsystems matter together

An agent that cannot remember is stateless -- useful for one-shot queries but
unable to learn a user's preferences, accumulate project knowledge, or improve
its own performance. An agent that remembers but cannot cite sources or detect
staleness risks confidently reciting outdated chain parameters or deprecated
APIs. Agents that can coordinate in groups can tackle complex workflows --
governance research teams, multi-chain monitoring, build-test-deploy pipelines
-- but only if their combined authority never exceeds what any individual member
was granted. Feeds and triggers let agents operate proactively rather than
waiting for user messages, but only if event processing is durable and
idempotent. Evaluation closes the loop: without measurement, improvement is
guesswork; with it, agents can systematically get better at their configured
jobs.

These subsystems share cross-cutting concerns: provenance tracking, tenant
isolation, classification-aware information flow, user-controlled retention and
deletion, and the absolute rule that no learning or coordination mechanism may
silently expand an agent's authority.

### 1.3 Relationship to other PRDs

| Related PRD | Interface |
|---|---|
| PRD-02 (Vocabulary, Invariants, Architecture) | Stable IDs, `ResolvedGrant`, classification model, tenant boundaries |
| PRD-03 (Agent/Run/Effect/Graph Execution) | Run lifecycle, `EffectIntent`/`EffectAttempt`/`EffectOutcome`, artifact lineage, event ordering |
| PRD-04 (Providers, Models, Harnesses, Tools, Skills) | Model routing, skill resolution, context assembly, provider health data |
| PRD-05 (Polkadot Integrations) | Chain profiles, metadata versions, network-specific knowledge provenance |
| PRD-07 (Identity, Accounts, Signers, Policy) | Grant intersection, policy evaluation, authorization decisions |
| PRD-10 (Data, Artifacts, Events, Observability) | Artifact storage, event stores, retention policies |
| PRD-11 (Self-Hosting, Managed Cloud, Multi-Tenancy) | Tenant isolation, cross-deployment memory portability |
| PRD-15 (Testing, Security Assurance) | Eval framework integration, red-team scenarios |

### 1.4 Key terms

| Term | Meaning in this PRD |
|---|---|
| **Episode** | A redacted, summarized record of one completed run or conversation turn, stored for later retrieval. |
| **Semantic memory** | Durable facts, patterns, and knowledge extracted from episodes or external sources, each carrying provenance and expiry. |
| **Procedural memory** | Reusable workflows, playbooks, preferences, and operational patterns learned from repeated successful execution. |
| **Knowledge entry** | A unit of semantic or procedural memory with explicit source attribution, confidence, chain/network binding, and expiry. |
| **Memory item** | The general type encompassing episodes, knowledge entries, and procedural records. |
| **Provenance** | The verifiable chain of evidence that establishes where a memory item came from, when, and with what confidence. |
| **Group** | A coordinated set of agent runs operating under shared budgets and intersected grants. |
| **Feed** | A cursor-backed stream of external events (chain, webhook, schedule, message) that an agent consumes. |
| **Trigger** | A policy-evaluated condition on a feed that creates a new run or action proposal. |
| **Recipe** | A reusable trigger-to-action template composing feed selection, trigger conditions, and response workflows. |
| **Eval** | A structured evaluation of agent performance against a defined benchmark corpus. |
| **Promotion** | The reviewed, policy-gated process by which evaluation results become active configuration changes. |

### 1.5 Design principles

1. **Provenance is mandatory.** Every memory item records its source, creation
   time, confidence level, and the evidence that produced it. Memory without
   provenance is not memory -- it is hallucination storage.

2. **Users control retention.** Users can inspect, search, export, and delete
   any memory item. Deletion propagates to derived indexes and projections.
   Retention policies are explicit and configurable.

3. **Staleness is visible.** Chain-specific knowledge carries network identity,
   runtime/metadata version, and block/time provenance. Expired or
   version-mismatched knowledge is flagged, not silently served.

4. **Groups cannot escalate.** A multi-agent group's effective authority is the
   intersection of its members' grants. Composition is a narrowing operation,
   never a widening one.

5. **Learning proposes, humans promote.** Evaluation may recommend changes to
   prompts, routing, skills, or workflows. Changes are versioned, compared, and
   promoted through explicit policy -- never silently enacted.

6. **Event processing is durable.** Feed cursors, trigger evaluations, and
   recipe executions survive crashes and restarts. At-least-once delivery plus
   idempotent processing, not false exactly-once claims.

7. **Experimental modules have clear boundaries.** Affect/vitality, evolutionary
   skill selection, and dream/reflection cycles are optional experimental
   modules. Core correctness cannot depend on them.

---

## 2. Memory architecture

### 2.1 Memory taxonomy

Polkagent maintains three categories of memory, inspired by cognitive science
but adapted for practical agent operation. All three categories are stored in
SQLite within the authority database, keeping the implementation zero-infra and
portable: episodic items use the standard relational schema, semantic and
procedural items are additionally indexed via FTS5 for full-text search and
sqlite-vec for vector search (see section 3.6 and section 12.1).

```text
+------------------+     promotion     +------------------+     codification     +--------------------+
|   Episodic       | ───────────────> |   Semantic        | ──────────────────> |   Procedural       |
|   Memory         |                  |   Memory          |                     |   Memory           |
+------------------+                  +------------------+                     +--------------------+
| Run/conversation |                  | Facts, patterns,  |                     | Workflows,         |
| history with     |                  | knowledge with    |                     | playbooks,         |
| redacted context |                  | provenance        |                     | preferences        |
+------------------+                  +------------------+                     +--------------------+
```

#### 2.1.1 Episodic memory

Episodic memory records what happened during agent runs. Each episode is a
redacted, bounded summary of a completed run or conversation turn. Episodes
preserve enough context for later retrieval and pattern recognition without
retaining raw prompts, model outputs, secrets, or unbounded conversation
history.

An episode records:

- Run/conversation/turn identifiers and parent links.
- Timestamp range (start, end).
- Participating agent, user, and tenant identifiers.
- Task summary and outcome (success, failure, partial, cancelled, unknown).
- Key artifacts produced or consumed (by reference, not content).
- Tools invoked and their success/failure status.
- Chain profiles and metadata versions used.
- Classification level of the episode content.
- Token/cost summary.
- Redaction metadata (what was removed and why).

Episodes are created automatically at run completion. They are the raw material
from which semantic knowledge is later extracted.

#### 2.1.2 Semantic memory

Semantic memory holds durable facts, patterns, and knowledge. Unlike episodes
(which record "what happened"), semantic memory records "what is known." Each
entry carries full provenance: which episodes, artifacts, or external sources
produced it; when it was created and last verified; its confidence level; and
its expiry conditions.

Examples of semantic memory:

- "The Polkadot Hub runtime uses metadata version 15 as of block 23,456,789."
- "User prefers Claude Sonnet for quick questions and Opus for code review."
- "The `pallet_balances::transfer_allow_death` call requires checking the
  existential deposit on the target chain."
- "Repository `/workspace/my-parachain` uses the `polkadot-stable2409` toolchain."

Semantic memory is chain-aware: entries about on-chain state carry network
identity, runtime version, metadata hash, and block/time provenance. An entry
about Polkadot Hub parameters is not valid for Kusama or a parachain unless
explicitly verified.

#### 2.1.3 Procedural memory

Procedural memory captures how-to knowledge: reusable workflows, operational
playbooks, user preferences, and patterns learned from repeated successful
execution. Where semantic memory says "what is true," procedural memory says
"what works."

Examples of procedural memory:

- "When this user asks to 'deploy', they mean: build in release mode, run tests,
  push to the staging branch, and open a PR."
- "For storage migration review: fork with Chopsticks, run pre-checks, apply
  migration, run post-checks, diff state, report."
- "This project's CI requires `cargo clippy --all-features` before commit."

Procedural entries reference the episodes where the pattern was observed and
confirmed, the contexts in which it applies, and any known exceptions or
failure modes.

### 2.2 Memory item structure

Every memory item, regardless of category, shares a common envelope:

```rust
/// Unique identifier for a memory item.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct MemoryId(pub Ulid);

/// Confidence in the accuracy and currency of a memory item.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Confidence {
    /// Directly observed or verified against a primary source.
    Verified,
    /// Inferred from observed evidence with reasonable certainty.
    Inferred,
    /// Proposed based on partial evidence; requires validation.
    Proposed,
    /// Experimental or speculative; not suitable for production decisions.
    Experimental,
    /// Confidence is unknown or not assessed.
    Unknown,
}

/// Source provenance for a memory item.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryProvenance {
    /// The primary source that produced this memory.
    pub source_kind: SourceKind,
    /// Artifact references that serve as evidence.
    pub source_artifacts: Vec<ArtifactId>,
    /// The run(s) during which this memory was created or last verified.
    pub source_runs: Vec<RunId>,
    /// When the source material was observed or accessed.
    pub observed_at: DateTime<Utc>,
    /// Human-readable source description (e.g., "Polkadot Hub metadata at block 23456789").
    pub source_description: String,
    /// For chain-specific knowledge, the chain evidence binding.
    pub chain_binding: Option<ChainBinding>,
}

/// What kind of source produced this memory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SourceKind {
    /// Observed during a run's execution.
    RunObservation,
    /// Extracted from an artifact produced by a run.
    ArtifactExtraction,
    /// Provided directly by a user.
    UserProvided,
    /// Retrieved from an external source (documentation, API, chain state).
    ExternalRetrieval,
    /// Inferred by analyzing multiple episodes or entries.
    Inference,
    /// Imported from another system or export.
    Import,
}

/// Chain-specific provenance binding.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChainBinding {
    /// The chain profile this knowledge applies to.
    pub chain_profile: ChainProfileRef,
    /// The runtime version at the time of observation.
    pub runtime_version: Option<u32>,
    /// The metadata hash at the time of observation.
    pub metadata_hash: Option<[u8; 32]>,
    /// The block number at the time of observation.
    pub block_number: Option<u64>,
    /// The block hash at the time of observation.
    pub block_hash: Option<[u8; 32]>,
}

/// Retention policy for a memory item.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// When this item expires and becomes eligible for automatic removal.
    pub expires_at: Option<DateTime<Utc>>,
    /// Maximum age before the item is considered stale and flagged for review.
    pub stale_after: Option<Duration>,
    /// Whether this item survives tenant-level retention sweeps.
    pub pinned: bool,
    /// The retention class governing this item's lifecycle.
    pub retention_class: RetentionClass,
}

/// Retention class determines sweep and archival behavior.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum RetentionClass {
    /// Ephemeral: removed after session or short TTL.
    Ephemeral,
    /// Standard: subject to normal retention sweeps.
    Standard,
    /// Extended: retained longer for compliance or audit.
    Extended,
    /// Permanent: retained until explicit user deletion.
    Permanent,
}

/// The common envelope for all memory items.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryItem {
    pub id: MemoryId,
    /// Which tenant/workspace owns this item.
    pub tenant_id: TenantId,
    /// Which agent created or owns this item.
    pub agent_id: AgentId,
    /// The category of memory.
    pub category: MemoryCategory,
    /// The content of the memory item.
    pub content: MemoryContent,
    /// Classification level (controls information flow).
    pub classification: Classification,
    /// Source provenance.
    pub provenance: MemoryProvenance,
    /// Confidence assessment.
    pub confidence: Confidence,
    /// Retention policy.
    pub retention: RetentionPolicy,
    /// When this item was created.
    pub created_at: DateTime<Utc>,
    /// When this item was last accessed for context assembly.
    pub last_accessed_at: Option<DateTime<Utc>>,
    /// When this item was last verified or refreshed.
    pub last_verified_at: Option<DateTime<Utc>>,
    /// How many times this item has been retrieved for context.
    pub access_count: u64,
    /// Tags for search and filtering.
    pub tags: Vec<String>,
    /// Optional embedding vector for semantic search.
    pub embedding: Option<Vec<f32>>,
    /// Whether this item is currently active (vs. archived or pending deletion).
    pub state: MemoryState,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum MemoryCategory {
    Episodic,
    Semantic,
    Procedural,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum MemoryState {
    Active,
    Stale,
    Archived,
    PendingDeletion,
    Deleted,
}
```

### 2.3 Privacy controls and tenant boundaries

Memory is tenant-scoped by default. The following invariants hold:

| Invariant | Enforcement |
|---|---|
| **MEM-PRIV-01** Tenant isolation | A memory query never returns items from another tenant. Cross-tenant memory sharing requires explicit federation configuration. |
| **MEM-PRIV-02** Agent scoping | An agent's memory is scoped to its configured workspace and tenant. A shared workspace may allow multiple agents to read common knowledge entries. |
| **MEM-PRIV-03** Classification flow | Memory items inherit the most restrictive classification of their source material. A `Sensitive` artifact cannot produce a `Public` knowledge entry without an explicit, audited declassification step. Memory items are tagged with their data classification at creation time; the policy gate blocks any retrieval or context assembly path that would cause a higher-classification item to flow into a lower-classification context. |
| **MEM-PRIV-04** Secret exclusion | Raw secrets, signing keys, API tokens, and credentials are never stored as memory content. A memory item may reference a secret handle; it must not contain the secret value. |
| **MEM-PRIV-05** User deletion | A user deletion request removes the item from primary storage, all derived indexes (vector, full-text, graph), cached embeddings, and any active context packs. Deletion is confirmed with a durable record of what was removed. |
| **MEM-PRIV-06** Export completeness | Memory export includes primary records, provenance, and enough metadata to rebuild derived indexes. Import is versioned, previewable, and auditable. |
| **MEM-PRIV-07** Retention enforcement | Automatic retention sweeps run on a configurable schedule. Expired items are moved to `PendingDeletion`, then purged after a grace period. Pinned items are exempt from sweeps but not from explicit user deletion. |

### 2.4 Retention policies and automatic expiry

Default retention policies vary by category:

| Category | Default stale-after | Default expires-at | Notes |
|---|---|---|---|
| Episodic | 90 days | 365 days | Configurable per agent/workspace. Compliance mode may extend. |
| Semantic | 180 days without access | No default expiry | Chain-bound entries become stale when metadata version changes. |
| Procedural | No default stale | No default expiry | Procedures that repeatedly fail are flagged for review. |

Operators can override these defaults at the tenant, workspace, or agent level.
A retention policy never silently deletes a `Pinned` item or an item with
`RetentionClass::Permanent`.

Chain-specific staleness is detected automatically:

1. When a chain profile's runtime version or metadata hash changes, all semantic
   memory entries bound to the old version are marked `Stale`.
2. Stale entries are excluded from context assembly by default but remain
   searchable and inspectable.
3. A background refresh job may attempt to re-verify stale entries against the
   new runtime. Successful re-verification updates the chain binding and resets
   staleness. Failed re-verification may archive or delete the entry.

---

## 3. Memory operations

### 3.1 Admission criteria

Not every run output becomes a memory item. Admission criteria prevent the
memory system from becoming a dumping ground for noise:

```rust
#[async_trait]
pub trait MemoryAdmission {
    /// Evaluate whether a candidate memory item should be admitted.
    async fn evaluate(
        &self,
        candidate: &MemoryCandidate,
        context: &AdmissionContext,
    ) -> AdmissionDecision;
}

pub struct MemoryCandidate {
    pub category: MemoryCategory,
    pub content: MemoryContent,
    pub provenance: MemoryProvenance,
    pub confidence: Confidence,
    pub classification: Classification,
}

pub struct AdmissionContext {
    pub tenant_id: TenantId,
    pub agent_id: AgentId,
    pub workspace_id: WorkspaceId,
    /// Current memory count for this agent/workspace.
    pub current_count: u64,
    /// Configured admission policy.
    pub policy: AdmissionPolicy,
}

pub enum AdmissionDecision {
    /// Admit the candidate as a new memory item.
    Admit,
    /// Merge with an existing item (update/enrich rather than duplicate).
    Merge { target: MemoryId },
    /// Reject the candidate with a reason.
    Reject { reason: String },
    /// Defer admission pending review or additional evidence.
    Defer { reason: String },
}
```

Admission criteria include:

1. **Novelty:** Does this candidate duplicate an existing memory item? If so,
   merge or reject.
2. **Relevance:** Is the content relevant to the agent's configured scope and
   skills?
3. **Confidence threshold:** Does the candidate meet the minimum confidence
   level for its category? (Default: `Inferred` for episodic, `Verified` or
   `Inferred` for semantic, `Verified` for procedural.)
4. **Classification compatibility:** Is the classification level compatible with
   the workspace's configured memory classification ceiling?
5. **Quota:** Has the agent/workspace reached its configured memory item limit?
6. **Provenance completeness:** Does the candidate have sufficient source
   attribution? An item with `SourceKind::Unknown` and `Confidence::Unknown` is
   rejected by default.

### 3.2 Promotion: episodic to semantic

Promotion is the process of extracting durable knowledge from episodic history.
It is never automatic in production -- a promotion pipeline proposes candidates,
and configured policy determines whether they are admitted.

```text
Episode(s) ──> Extraction job ──> CandidateKnowledge ──> Admission ──> SemanticMemory
                                        │
                                        ├── source_episodes: [EpisodeId...]
                                        ├── extracted_content: String
                                        ├── confidence: Inferred
                                        ├── chain_binding: Option<ChainBinding>
                                        └── requires_review: bool
```

The extraction job:

1. Selects episodes that match configured extraction criteria (e.g., successful
   runs in a specific skill domain, repeated patterns, or user-corrected
   outputs).
2. Applies a model-assisted or rule-based extraction to identify candidate
   facts, patterns, or preferences.
3. Each candidate carries its source episodes as provenance.
4. Candidates with `requires_review: true` enter a review queue visible in the
   Agent Studio or Inbox.
5. Reviewed and approved candidates pass through normal admission.

#### 3.2.1 Provenance completeness and staleness refresh

Two constraints harden the promotion pipeline:

- **Provenance completeness at admission.** A candidate is rejected by the
  admission gate if it arrives without source provenance (i.e.,
  `SourceKind::Unknown` or an empty `source_episodes` list). Semantic memory
  without a verifiable origin is treated as hallucination storage. This is an
  absolute rejection -- the `Defer` path is not available for provenance
  failures.

- **Staleness refresh on chain-state change.** When a chain profile's runtime
  version or metadata hash changes, all semantic entries bound to that profile
  are marked `Stale` (see section 4.3). Before a stale entry can be
  re-admitted as a refreshed item, a new promotion candidate must be produced
  that carries updated chain binding from a post-change observation. This
  ensures semantic memory tracks live chain state rather than silently serving
  outdated facts.

Promotion policies are configurable:

| Policy | Effect |
|---|---|
| `auto_admit_verified` | Candidates with `Confidence::Verified` and complete chain binding are admitted without review. |
| `review_all` | All promotion candidates require explicit user or operator review. Default for new agents. |
| `review_inferred` | Only `Inferred` or lower confidence candidates require review. |
| `disabled` | No automatic promotion. Users manually create semantic memory. |

### 3.3 Promotion: semantic to procedural

When the same semantic pattern is observed repeatedly and consistently, it may
be codified as a procedural entry -- a reusable workflow or preference.

This is a higher bar than episodic-to-semantic promotion:

1. The pattern must appear in at least N successful episodes (configurable,
   default 3).
2. The pattern must not contradict any existing procedural entry.
3. The proposed procedure must reference specific steps, tools, or preferences.
4. Codification always requires review (there is no `auto_admit` for procedural
   memory).

### 3.4 Forget/delete: user-controlled

Users have full control over memory deletion. Three deletion modes:

| Mode | Behavior |
|---|---|
| **Delete item** | Remove a specific memory item by ID. Propagates to derived indexes. |
| **Delete by query** | Remove all items matching a search query (with confirmation). |
| **Forget scope** | Remove all memory items for a specific conversation, run, workspace, time range, or agent. |

Deletion is durable and confirmed:

```rust
pub struct DeletionReceipt {
    pub request_id: DeletionRequestId,
    pub items_deleted: Vec<MemoryId>,
    pub indexes_updated: Vec<String>,
    pub completed_at: DateTime<Utc>,
    /// Retained for audit; does not contain deleted content.
    pub audit_record: DeletionAudit,
}
```

The audit record logs that a deletion occurred, which IDs were removed, who
requested it, and when -- but never retains the deleted content itself.

### 3.5 Export and import

Memory export produces a versioned, self-contained package:

```rust
pub struct MemoryExport {
    pub format_version: String,
    pub exported_at: DateTime<Utc>,
    pub tenant_id: TenantId,
    pub agent_id: Option<AgentId>,
    pub items: Vec<MemoryItem>,
    pub provenance_graph: Vec<ProvenanceEdge>,
    pub integrity_digest: [u8; 32],
}
```

Import:

1. Validates format version and integrity digest.
2. Presents a preview of items to be imported (count, categories, date range,
   classifications).
3. Maps source tenant/agent IDs to target identifiers.
4. Detects conflicts with existing items (by content hash or provenance match).
5. Applies target admission policy to each imported item.
6. Rebuilds derived indexes after import.
7. Produces an import receipt.

### 3.6 Search and retrieval

Memory retrieval serves two distinct purposes:

1. **Context assembly:** The system retrieves relevant memory items to include
   in model context for a new run. This is the primary operational use.
2. **User search:** Users browse and manage their memory through the Agent
   Studio or CLI.

#### 3.6.1 Hybrid retrieval: sqlite-vec + FTS5 with RRF

Context assembly uses a hybrid retrieval strategy combining vector search and
full-text search, both backed by SQLite:

- **Vector search** uses sqlite-vec, which supports f32 (full precision), int8
  quantized, and 1-bit quantized vector columns. The `memory_embeddings` table
  stores per-item embeddings produced by a configured embedding model. On the
  Rust side, sqlite-vec is loaded via `rusqlite` + `sqlite3_auto_extension`,
  with embedding bytes passed as `zerocopy::AsBytes` slices to avoid copies.

- **Full-text search** uses the FTS5 virtual table (`memory_fts`) over
  `content_text`, `tags`, and `source_description`.

- **Fusion** applies Reciprocal Rank Fusion (RRF) to merge the two ranked
  lists before applying the token budget and classification filters. RRF is
  parameter-free and robust to mismatched score scales between the two
  retrieval channels.

This approach requires no external vector database: the authority DB that
already holds memory items, provenance, and retention records also serves
vector and full-text retrieval. Tenant isolation is preserved by row-level
`tenant_id` predicates that apply before the FTS5 and sqlite-vec scans.

```rust
#[async_trait]
pub trait MemoryStore {
    /// Store a new memory item after admission.
    async fn store(&self, item: MemoryItem) -> Result<MemoryId, MemoryError>;

    /// Retrieve a memory item by ID.
    async fn get(&self, id: &MemoryId) -> Result<Option<MemoryItem>, MemoryError>;

    /// Search memory items.
    async fn search(&self, query: &MemoryQuery) -> Result<MemorySearchResult, MemoryError>;

    /// Delete a memory item.
    async fn delete(&self, id: &MemoryId) -> Result<DeletionReceipt, MemoryError>;

    /// Bulk delete by query.
    async fn delete_by_query(
        &self,
        query: &MemoryQuery,
    ) -> Result<DeletionReceipt, MemoryError>;

    /// Export memory items matching a filter.
    async fn export(&self, filter: &MemoryExportFilter) -> Result<MemoryExport, MemoryError>;

    /// Import memory items.
    async fn import(&self, data: MemoryExport) -> Result<ImportReceipt, MemoryError>;

    /// Mark items as stale (e.g., after chain metadata change).
    async fn mark_stale(
        &self,
        filter: &StalenessFilter,
    ) -> Result<u64, MemoryError>;

    /// Run retention sweep.
    async fn sweep(&self, policy: &RetentionPolicy) -> Result<SweepResult, MemoryError>;
}

pub struct MemoryQuery {
    pub tenant_id: TenantId,
    pub agent_id: Option<AgentId>,
    pub workspace_id: Option<WorkspaceId>,
    pub categories: Option<Vec<MemoryCategory>>,
    pub text_query: Option<String>,
    pub semantic_query: Option<Vec<f32>>,
    pub tags: Option<Vec<String>>,
    pub min_confidence: Option<Confidence>,
    pub max_classification: Option<Classification>,
    pub state_filter: Option<Vec<MemoryState>>,
    pub chain_profile: Option<ChainProfileRef>,
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    pub limit: u32,
    pub offset: u32,
}
```

Context assembly uses memory retrieval with additional constraints:

- Only `Active` items are included (not `Stale`, `Archived`, or
  `PendingDeletion`).
- Classification filtering ensures items do not exceed the run's classification
  ceiling.
- Token budget limiting ensures memory does not consume the entire context
  window.
- Each included item is recorded in the `ContextPack` with its `MemoryId`,
  relevance score, and token estimate, as defined in PRD-03.

---

## 4. Knowledge and citation model

### 4.1 Source provenance tracking

Every knowledge entry must be traceable to its origin. The provenance chain
answers: "Why does the agent believe this, and is that belief still valid?"

```text
Knowledge Entry
  ├── source_kind: ExternalRetrieval
  ├── source_description: "Polkadot Hub runtime metadata v15"
  ├── source_artifacts:
  │     └── ArtifactId("metadata-snapshot-abc123")
  ├── chain_binding:
  │     ├── chain_profile: "polkadot-production"
  │     ├── runtime_version: 1003000
  │     ├── metadata_hash: 0xabc...
  │     └── block_number: 23456789
  ├── observed_at: 2026-07-15T10:30:00Z
  ├── confidence: Verified
  └── expires_at: None (stale_after: metadata_version_change)
```

The provenance chain must survive export/import and cross-deployment migration.
When a knowledge entry is included in a model context, the context pack records
the entry's provenance so that the agent's response can cite its sources.

### 4.2 Citation format and verification

When an agent uses knowledge from memory in its response, the response artifact
should include structured citations:

```rust
pub struct Citation {
    /// The memory item cited.
    pub memory_id: MemoryId,
    /// A human-readable summary of what was cited.
    pub summary: String,
    /// The provenance of the cited item (copied at citation time).
    pub provenance_snapshot: MemoryProvenance,
    /// Whether the cited item was verified as current at the time of use.
    pub verified_at_use: bool,
    /// If chain-bound, whether the chain binding matches the current profile.
    pub chain_current: Option<bool>,
}
```

Citation verification:

1. At context assembly time, chain-bound knowledge entries are checked against
   the current chain profile. If the runtime version or metadata hash has
   changed, the entry is flagged as potentially stale.
2. The agent may still use a stale entry if no current alternative exists, but
   the citation must indicate `verified_at_use: false` and
   `chain_current: Some(false)`.
3. The response rendering layer shows stale citations with a visible warning.

### 4.3 Stale knowledge detection

Staleness detection operates at two levels:

**Automatic (background):**
- A chain profile watcher detects runtime version or metadata hash changes.
- All knowledge entries bound to the changed profile are marked `Stale`.
- A notification is sent to the agent's operator/owner.

**At retrieval time:**
- The context assembler checks each candidate knowledge entry's chain binding
  against the current profile state.
- Entries whose `stale_after` duration has elapsed are excluded by default.
- Entries whose chain binding no longer matches are flagged.

### 4.4 Metadata-grounded knowledge

Knowledge about Polkadot-ecosystem chains must carry chain-specific grounding.
This is not optional or best-effort -- it is a correctness requirement.

A knowledge entry about "the transfer fee on Polkadot Hub" is meaningless
without:

1. Which chain (genesis hash, chain profile reference).
2. Which runtime version.
3. Which block or time the observation was made.
4. Whether the observation is still current.

Knowledge entries without chain binding cannot make claims about on-chain state,
parameters, fees, balances, governance, or runtime behavior. The admission
policy rejects chain-related knowledge candidates that lack a `ChainBinding`.

---

## 5. G1-G5 candidates: detailed design

This section provides detailed designs for the five memory/groups/evals
candidates identified in the research corpus.

### 5.1 G1: Provenanced memory

**Status:** Candidate
**User:** Any agent user who benefits from personalized, inspectable memory.
**Value:** Personalized help without invisible profiles. Users can see what the
agent remembers, where it came from, when it expires, and delete anything.

**Detailed design:**

The provenanced memory system is the core of this PRD's memory architecture
(sections 2-4). Its distinguishing properties are:

1. **Every item has provenance.** No memory item exists without a
   `MemoryProvenance` struct that names its source, observation time, and
   evidence chain.
2. **Inspection UX.** The Agent Studio provides a Memory panel where users can:
   - Browse memory by category, recency, access frequency, and tags.
   - View full provenance chains for any item.
   - See which runs have used each item.
   - Filter by chain profile to see chain-specific knowledge.
   - Mark items as incorrect, outdated, or irrelevant.
3. **Expiry and staleness.** Items carry explicit retention policies. Chain-bound
   items automatically detect staleness from metadata changes.
4. **Delete and export.** Full user control as specified in section 3.4-3.5.

**Acceptance evidence:**

| Test ID | Description | Pass criteria |
|---|---|---|
| G1-01 | Create episodic memory from a completed run | Episode includes run ID, summary, artifacts, tools, chain profile, classification, provenance, and retention policy. |
| G1-02 | Promote episodic to semantic | Promotion candidate carries source episode IDs. Review policy is enforced. |
| G1-03 | Chain-bound knowledge staleness | When chain profile metadata hash changes, bound entries are marked Stale within one sweep cycle. |
| G1-04 | User deletion | Deleting a memory item removes it from primary storage, vector index, full-text index, and any cached context packs. Deletion receipt confirms. |
| G1-05 | Export/import round-trip | Export followed by import into a new tenant produces identical items with mapped IDs and intact provenance chains. |
| G1-06 | Context assembly filtering | Model context receives only Active, classification-compatible, non-expired items within token budget. Excluded items are recorded in the ContextPack. |
| G1-07 | Privacy boundary | Memory query with Tenant A credentials never returns Tenant B items, even with matching content or embeddings. |

### 5.2 G2: Worktree-aware context

**Status:** Candidate
**User:** Builder using Polkagent as a coding assistant across multiple
repositories, branches, or worktrees.
**Value:** Safer builder continuity. The agent knows which repository,
branch, and working state it is operating in, and does not confuse knowledge
from one workspace with another.

**Detailed design:**

A `WorkspaceBinding` associates memory items with a specific workspace context:

```rust
pub struct WorkspaceBinding {
    /// The workspace this memory is bound to.
    pub workspace_id: WorkspaceId,
    /// Optional repository root path.
    pub repository_root: Option<PathBuf>,
    /// Optional git reference (branch, tag, commit).
    pub git_ref: Option<String>,
    /// Optional commit hash at the time of observation.
    pub commit_hash: Option<String>,
}
```

Workspace-bound memory operates as follows:

1. When an agent is configured with a workspace, its memory queries are
   automatically scoped to that workspace's binding.
2. Cross-workspace memory access is denied by default. A workspace may
   explicitly share specific knowledge entries with other workspaces through a
   sharing policy.
3. Each memory item's workspace binding is recorded alongside its provenance.
   An item created while working on `feature-branch` in `/workspace/my-project`
   carries that context.
4. When context is assembled for a run, workspace-bound items are preferred over
   unbound items when the agent is operating in that workspace.

**Acceptance evidence:**

| Test ID | Description | Pass criteria |
|---|---|---|
| G2-01 | Workspace scoping | Agent in workspace A cannot retrieve memory items bound to workspace B. |
| G2-02 | Source revision tracking | Memory items created during a run carry the commit hash of the workspace at run time. |
| G2-03 | Context preference | When assembling context in workspace A, workspace-A-bound items rank higher than unbound items of similar relevance. |
| G2-04 | Explicit sharing | A knowledge entry shared from workspace A to workspace B appears in B's queries with a provenance note indicating the share. |

### 5.3 G3: Grant-intersection group

**Status:** Later (phased)
**User:** Teams or organizations running specialized agent groups for complex
workflows.
**Value:** Multiple specialized agents cooperate under the intersection of their
grants and a shared budget. Composition cannot escalate authority.

**Detailed design:** See section 6 (Multi-agent groups) for the complete
design.

The grant-intersection property is the central safety invariant:

```text
Group effective grant = Agent_1 grant ∩ Agent_2 grant ∩ ... ∩ Agent_N grant
                        ∩ Group-level policy
                        ∩ Current budget remaining
```

If Agent_1 has `governance.vote` permission and Agent_2 does not, the group
cannot vote. If Agent_1 has a $10 budget and Agent_2 has a $5 budget, the group
budget is at most $5 (or a separately configured group budget, whichever is
lower).

**Acceptance evidence:**

| Test ID | Description | Pass criteria |
|---|---|---|
| G3-01 | Grant intersection | A group of two agents where one lacks a specific tool permission cannot invoke that tool, even if the other agent has it. |
| G3-02 | Budget intersection | A group with member budgets of $10 and $5 has an effective budget no greater than $5 (or a configured group budget, whichever is lower). |
| G3-03 | Property test | For any randomly generated set of agent grants, the group's effective grant is a subset of every member's grant. |
| G3-04 | No escalation | Adding an agent to a group never increases the group's effective permissions. |

### 5.4 G4: Feeds/triggers/recipes

**Status:** Candidate
**User:** Operators who want agents to react to external events (chain state
changes, schedules, webhooks) without constant polling.
**Value:** Repeatable, automated operations with durable event processing.

**Detailed design:** See section 7 (Feeds, triggers, and recipes) for the
complete design.

**Acceptance evidence:**

| Test ID | Description | Pass criteria |
|---|---|---|
| G4-01 | Cursor durability | Kill the agent process during feed consumption. On restart, processing resumes from the last committed cursor position. No events are skipped or duplicated at the logical level. |
| G4-02 | Trigger idempotency | The same event delivered twice produces exactly one logical action (run/proposal). |
| G4-03 | Recipe composition | A recipe combining a chain-event feed with a schedule trigger produces actions only when both conditions are met. |
| G4-04 | Rate limiting | A burst of 1000 events in one second produces at most N actions (where N is the configured rate limit), with remaining events queued for later processing. |

### 5.5 G5: Evals/self-improvement

**Status:** Later (phased)
**User:** Agent operators who want systematic, measurable improvement.
**Value:** Safer iterative improvement where promotion of changes is reviewable,
not automatic.

**Detailed design:** See section 8 (Evaluation framework) and section 9
(Learning mechanisms) for the complete design.

**Acceptance evidence:**

| Test ID | Description | Pass criteria |
|---|---|---|
| G5-01 | Eval isolation | Running an eval against a candidate skill/prompt does not modify the active agent configuration. |
| G5-02 | Promotion review | A candidate that scores higher than the current version enters a promotion queue; it does not become active until explicitly approved. |
| G5-03 | No authority change | A promotion event cannot modify the agent's grant, add tools, change custody, or expand budget. |
| G5-04 | Benchmark provenance | Eval results record the exact benchmark corpus version, model used, and scoring criteria. Results from different corpus versions are not compared. |

---

## 6. Multi-agent groups

### 6.1 Concepts and motivation

A group is a coordinated set of agent runs that cooperate on a task too complex
or too specialized for a single agent. Examples:

- A governance research group: one agent retrieves referendum data, another
  decodes runtime changes, a third researches external context, and a
  coordinator synthesizes a brief.
- A build-test-deploy pipeline: a builder agent writes code, a tester runs
  test suites, and a deployer prepares deployment artifacts -- each with
  different tool grants.
- A multi-chain monitor: agents watching different chains feed observations
  to a coordinator that identifies cross-chain patterns.

Groups are not free-form social spaces. They are structured, policy-bounded
coordination mechanisms with explicit membership, authority, budgets, and
lifecycle.

### 6.2 Parent/child run model

Groups use a parent/child run hierarchy:

```text
Group Run (parent)
  ├── Child Run 1 (Agent A: researcher)
  │     ├── EffectIntent: query chain state
  │     └── Artifact: research findings
  ├── Child Run 2 (Agent B: decoder)
  │     ├── EffectIntent: decode extrinsic
  │     └── Artifact: decoded analysis
  └── Child Run 3 (Agent C: synthesizer)
        ├── depends_on: [Child Run 1, Child Run 2]
        └── Artifact: synthesized brief
```

The parent run:

- Owns the group's lifecycle, budget, and coordination state.
- Creates child runs with scoped grants derived from the group's effective
  grant.
- Receives artifacts and events from child runs.
- Makes coordination decisions (fan-out, join, routing, escalation).
- Is the single cancellation point -- cancelling the parent cancels all
  children.

Child runs:

- Execute under their own agent identity but with grants narrowed by the
  group's effective grant.
- Cannot create sibling runs or modify the parent's state directly.
- Report results through artifacts and terminal events.
- May be created lazily (only when needed) or eagerly (all at once).

```rust
/// A multi-agent group definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    pub tenant_id: TenantId,
    pub name: String,
    pub description: String,
    /// The agents participating in this group.
    pub members: Vec<GroupMember>,
    /// The group's coordination mode.
    pub coordination: CoordinationMode,
    /// Group-level policy constraints applied on top of member grants.
    pub group_policy: GroupPolicy,
    /// Shared budget for the group.
    pub budget: GroupBudget,
    /// Current state of the group.
    pub state: GroupState,
    /// When this group was created.
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupMember {
    pub agent_id: AgentId,
    /// The role this agent plays in the group.
    pub role: GroupRole,
    /// The agent's individual grant (group effective grant will be the
    /// intersection).
    pub individual_grant: ResolvedGrant,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum GroupRole {
    /// Coordinates child runs, aggregates results.
    Coordinator,
    /// Executes a specialized task within the group.
    Worker,
    /// Reviews and approves group outputs before they become visible
    /// outside the group.
    Reviewer,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CoordinationMode {
    /// Sequential: child runs execute one after another.
    Sequential,
    /// Parallel: independent child runs execute concurrently.
    Parallel,
    /// Pipeline: child runs form a directed acyclic graph with explicit
    /// dependencies.
    Pipeline { dependencies: Vec<(RunId, RunId)> },
    /// Consensus: multiple agents produce independent results, and a
    /// coordinator synthesizes or selects among them.
    Consensus { min_results: u32 },
}
```

### 6.3 Group composition and coordination modes

**Sequential:** Child runs execute in order. Each run receives the artifacts
from previous runs as context. Useful for pipelines where each step depends on
the previous output.

**Parallel:** Independent child runs execute concurrently within the group's
budget. Useful when multiple research tasks can proceed independently. The
coordinator collects results and synthesizes.

**Pipeline:** A directed acyclic graph (DAG) of child runs with explicit
dependencies. Run C starts only after Runs A and B complete. Useful for
complex workflows with partial independence.

**Consensus:** Multiple agents independently produce results for the same
question. The coordinator compares, selects, or synthesizes. Useful for
high-stakes decisions where independent verification adds confidence.

### 6.4 Grant intersection

The group's effective grant is computed as:

```rust
pub fn compute_group_grant(members: &[GroupMember], group_policy: &GroupPolicy) -> ResolvedGrant {
    let mut effective = ResolvedGrant::universal(); // Start with full permissions

    // Intersect with each member's grant
    for member in members {
        effective = effective.intersect(&member.individual_grant);
    }

    // Intersect with group-level policy constraints
    effective = effective.intersect(&group_policy.as_grant());

    // Apply budget constraints
    effective = effective.with_budget(group_policy.budget.clone());

    effective
}
```

Properties that must hold (tested by property tests):

1. `for all g in members: group_grant ⊆ g.individual_grant`
2. `group_grant ⊆ group_policy.as_grant()`
3. Adding a member never increases `group_grant`.
4. Removing a member may increase `group_grant` (but never beyond `group_policy`).

A sub-agent's effective capability is therefore the intersection of its own
grant and the group's effective grant -- the narrower of the two always wins.
This is not advisory: the policy gate enforces the intersection at every tool
invocation, not merely at group formation time.

### 6.5 Shared budgets and resource limits

Groups have their own budget, separate from and constrained by member budgets:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupBudget {
    /// Maximum total model cost for all child runs combined.
    pub max_model_cost: Option<Decimal>,
    /// Maximum total tool invocations across all child runs.
    pub max_tool_invocations: Option<u64>,
    /// Maximum total duration for the group run.
    pub max_duration: Option<Duration>,
    /// Maximum number of concurrent child runs.
    pub max_concurrent_children: Option<u32>,
    /// Maximum number of total child runs (including completed).
    pub max_total_children: Option<u32>,
    /// Current spending against this budget.
    pub spent: BudgetSpent,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BudgetSpent {
    pub model_cost: Decimal,
    pub tool_invocations: u64,
    pub children_created: u32,
    pub children_active: u32,
}
```

Budget enforcement:

1. Before creating a child run, the coordinator checks remaining budget.
2. Each child run's cost is tracked and charged against the group budget.
3. When the group budget is exhausted, no new child runs are created and
   active children receive a budget-exhaustion signal.
4. Budget overflow (spending more than allocated) is prevented by pre-check
   but detected by post-check if race conditions occur. Detected overflow
   triggers a group pause.

Group budgets are hard caps, not soft recommendations. The policy gate enforces
them unconditionally -- no runtime override, no model output, and no
coordinator instruction can authorize spending beyond the configured limit. This
is enforced at the same layer as grant intersection (section 6.4), ensuring
that budget and capability controls compose without gaps.

### 6.6 Cancellation propagation

Cancellation flows downward through the group hierarchy:

```text
Cancel Group Run
  ├── Signal: CancelRequested to all active child runs
  ├── Grace period for child runs to complete or checkpoint
  ├── After grace period: force-cancel remaining children
  └── Group Run terminal state: Cancelled
```

Cancellation rules:

1. Cancelling a parent run cancels all child runs.
2. Cancelling a child run does not cancel the parent or siblings.
3. A child run failure may trigger parent-level error handling (retry, skip,
   or escalate) depending on the coordination mode and error policy.
4. A cancelled child run's partial artifacts are preserved but marked as
   incomplete.

### 6.7 Evidence aggregation

The group's output is an aggregated evidence package:

```rust
pub struct GroupEvidence {
    pub group_id: GroupId,
    pub parent_run_id: RunId,
    /// Evidence from each child run.
    pub child_evidence: Vec<ChildEvidence>,
    /// Synthesized/aggregated output from the coordinator.
    pub synthesized_artifacts: Vec<ArtifactId>,
    /// Total resource consumption.
    pub total_cost: BudgetSpent,
    /// Group outcome.
    pub outcome: GroupOutcome,
}

pub struct ChildEvidence {
    pub child_run_id: RunId,
    pub agent_id: AgentId,
    pub role: GroupRole,
    pub artifacts: Vec<ArtifactId>,
    pub outcome: RunOutcome,
    pub cost: BudgetSpent,
}

pub enum GroupOutcome {
    /// All required children completed successfully.
    Success,
    /// Some children failed but the group produced a partial result.
    Partial { failed: Vec<RunId> },
    /// The group failed to produce a useful result.
    Failed { reason: String },
    /// The group was cancelled.
    Cancelled,
}
```

---

## 7. Feeds, triggers, and recipes

### 7.1 Concepts

Feeds, triggers, and recipes enable agents to operate proactively rather than
waiting for user messages.

- A **feed** is a cursor-backed stream of events from an external source.
- A **trigger** is a policy-evaluated condition that, when satisfied by a feed
  event, creates a new run or action proposal.
- A **recipe** is a reusable template that combines a feed, trigger conditions,
  and a response workflow.

Together, they form an event-driven automation layer:

```text
External Source ──> Feed ──> Trigger Evaluation ──> Recipe Execution ──> Run/Proposal
                     │
                     └── Cursor (durable position marker)
```

### 7.2 Event cursor model

Each feed maintains a durable cursor that tracks the last successfully processed
position:

```rust
/// A feed definition.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Feed {
    pub id: FeedId,
    pub tenant_id: TenantId,
    pub agent_id: AgentId,
    pub name: String,
    /// The source of events for this feed.
    pub source: FeedSource,
    /// Current cursor position.
    pub cursor: FeedCursor,
    /// Feed state.
    pub state: FeedState,
    /// Configuration for this feed.
    pub config: FeedConfig,
    pub created_at: DateTime<Utc>,
    pub last_event_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FeedSource {
    /// On-chain events from a specific chain profile.
    ChainEvents {
        chain_profile: ChainProfileRef,
        /// Filter for specific event types.
        event_filter: Vec<ChainEventFilter>,
    },
    /// Scheduled/cron events.
    Schedule {
        /// Cron expression.
        cron: String,
        /// Timezone.
        timezone: String,
    },
    /// Webhook events.
    Webhook {
        /// Expected webhook source identifier.
        source_id: String,
        /// Payload schema for validation.
        payload_schema: Option<String>,
    },
    /// Messages from a transport (new conversations, mentions, etc.).
    Transport {
        transport_id: TransportId,
        /// Filter for specific message types or senders.
        message_filter: Option<MessageFilter>,
    },
    /// Internal platform events (run completions, memory changes, etc.).
    Platform {
        event_types: Vec<String>,
    },
}

/// Cursor position in a feed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FeedCursor {
    /// Block-based cursor for chain events.
    Block {
        block_number: u64,
        event_index: u32,
    },
    /// Time-based cursor for scheduled events.
    Time {
        last_fired: DateTime<Utc>,
    },
    /// Sequence-based cursor for webhook/transport/platform events.
    Sequence {
        sequence_number: u64,
    },
    /// Initial position (no events processed yet).
    Initial,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum FeedState {
    Active,
    Paused,
    Error { retry_after: Option<DateTime<Utc>> },
    Disabled,
}
```

Cursor durability guarantees:

1. The cursor is committed transactionally with the action created by the
   trigger. This means: either the event is processed and the cursor advances,
   or neither happens.
2. On crash/restart, processing resumes from the last committed cursor.
3. Gap detection: if the feed source reports events that the cursor should have
   seen but did not (e.g., missed blocks), the feed enters an `Error` state
   with a gap report. The operator must decide whether to skip the gap or
   backfill.

### 7.3 Trigger evaluation

Triggers evaluate feed events against conditions and, when matched, create
actions:

```rust
/// A trigger binding.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TriggerBinding {
    pub id: TriggerId,
    pub tenant_id: TenantId,
    pub agent_id: AgentId,
    pub feed_id: FeedId,
    pub name: String,
    /// Conditions that must be met for the trigger to fire.
    pub conditions: Vec<TriggerCondition>,
    /// What happens when the trigger fires.
    pub action: TriggerAction,
    /// Rate limiting and deduplication.
    pub rate_limit: TriggerRateLimit,
    /// Policy constraints on triggered runs.
    pub trigger_grant: TriggerGrant,
    pub state: TriggerState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TriggerCondition {
    /// Event matches a specific pattern.
    EventMatch { pattern: EventPattern },
    /// A threshold is exceeded (e.g., balance drops below X).
    Threshold { field: String, op: CompareOp, value: serde_json::Value },
    /// Multiple conditions must all be true.
    All(Vec<TriggerCondition>),
    /// At least one condition must be true.
    Any(Vec<TriggerCondition>),
    /// Condition is negated.
    Not(Box<TriggerCondition>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TriggerAction {
    /// Create a new run with the specified configuration.
    CreateRun {
        agent_id: AgentId,
        skill: Option<SkillRef>,
        input: serde_json::Value,
    },
    /// Create a proposal for human review.
    CreateProposal {
        proposal_type: String,
        summary: String,
    },
    /// Send a notification.
    Notify {
        channel: NotificationChannel,
        message_template: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TriggerRateLimit {
    /// Maximum triggers per time window.
    pub max_per_window: u32,
    /// Time window duration.
    pub window: Duration,
    /// Cooldown after a trigger fires before it can fire again.
    pub cooldown: Option<Duration>,
    /// Deduplication key template (prevents duplicate triggers for the same
    /// logical event).
    pub dedup_key: Option<String>,
    /// Maximum pending (unprocessed) triggers before the feed pauses.
    pub max_pending: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TriggerGrant {
    /// The grant under which triggered runs execute.
    /// Must be a subset of the agent's configured grant.
    pub run_grant: ResolvedGrant,
    /// Maximum budget per triggered run.
    pub per_run_budget: Option<Decimal>,
    /// Maximum total budget across all triggered runs per time window.
    pub window_budget: Option<Decimal>,
}
```

Trigger evaluation is deterministic and does not involve model inference.
Conditions are evaluated against the event's typed fields. This keeps trigger
evaluation fast, auditable, and not subject to model variability.

#### 7.3.1 Deterministic guardrail for chain-event triggers

For chain-event feeds, the event index produced by the chain indexer is itself
deterministic: the same block height and event sequence always produce the same
typed event record. Trigger conditions operate against these typed fields, not
against free-form model output. If a trigger condition cannot be expressed in
typed field comparisons (e.g., "fire when the governance brief looks important"),
the correct pattern is to fire on a deterministic field (e.g., "new referendum
created") and then let the resulting run use an LLM to assess importance within
the run's own sandbox. The LLM is never part of the trigger gate itself. This
separation preserves auditability -- every trigger fire is reproducible from
the event log -- and prevents adversarially crafted chain data from inducing
unexpected trigger behavior through model variability.

### 7.4 Recipe composition

A recipe packages a feed, trigger conditions, and response workflow into a
reusable, shareable template:

```rust
/// A recipe: reusable feed + trigger + workflow template.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recipe {
    pub id: RecipeId,
    pub name: String,
    pub description: String,
    pub version: Version,
    /// The feed source template.
    pub feed_template: FeedSource,
    /// Trigger conditions.
    pub conditions: Vec<TriggerCondition>,
    /// The workflow to execute when triggered.
    pub workflow: RecipeWorkflow,
    /// Required capabilities (tools, chain profiles, etc.).
    pub required_capabilities: Vec<Capability>,
    /// Default rate limits.
    pub default_rate_limit: TriggerRateLimit,
    /// Default budget constraints.
    pub default_budget: TriggerGrant,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RecipeWorkflow {
    /// Simple: create a single run.
    SingleRun {
        agent_id: AgentId,
        skill: Option<SkillRef>,
        input_template: serde_json::Value,
    },
    /// Group: create a coordinated group run.
    GroupRun {
        group_template: GroupTemplate,
    },
    /// Notify only: send a notification without creating a run.
    NotifyOnly {
        channels: Vec<NotificationChannel>,
        message_template: String,
    },
}
```

Recipes can be published to registries, shared between tenants, and installed
like skills. When installed, a recipe creates the necessary feed and trigger
bindings with the user's configured parameters substituted into the templates.

### 7.5 Idempotency and replay safety

Event processing must be idempotent at the logical level:

1. **Deduplication key:** Each trigger evaluation produces a deduplication key
   derived from the event's identity and the trigger's ID. If a run with that
   dedup key already exists, no new run is created.

2. **Cursor-action atomicity:** The cursor advance and action creation are
   committed in the same database transaction. This prevents both missed events
   (cursor advanced but action not created) and duplicate actions (action created
   but cursor not advanced).

3. **Replay safety:** If an operator explicitly replays a feed range (e.g.,
   reprocessing missed blocks), the deduplication layer prevents duplicate
   actions for events that were already processed.

4. **Effect idempotency:** Triggered runs use the same `EffectIntent` /
   `EffectAttempt` / `EffectOutcome` model as all runs (PRD-03). External
   effects within triggered runs carry idempotency keys.

---

## 8. Evaluation framework

### 8.1 Purpose and scope

The evaluation framework measures agent performance against defined benchmarks.
Its purpose is threefold:

1. **Measure:** Quantify how well an agent performs specific tasks.
2. **Compare:** Compare performance across model versions, prompt variations,
   skill versions, and routing strategies.
3. **Propose:** Generate evidence-based recommendations for improvement.

Evaluations never directly modify agent configuration, grants, or active
deployments.

### 8.2 Eval types

```rust
/// The type of evaluation being performed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EvalType {
    /// Does the agent produce correct outputs?
    Correctness {
        /// Expected outputs for comparison.
        ground_truth: Vec<GroundTruth>,
    },
    /// Does the agent respect safety constraints?
    Safety {
        /// Adversarial scenarios to test.
        scenarios: Vec<SafetyScenario>,
    },
    /// How good is the agent's output quality?
    Quality {
        /// Quality dimensions to assess.
        dimensions: Vec<QualityDimension>,
    },
    /// How efficient is the agent?
    Performance {
        /// Performance metrics to measure.
        metrics: Vec<PerformanceMetric>,
    },
}
```

#### 8.2.1 Correctness evals

Correctness evals compare agent outputs against known-good reference answers.
They are most useful for well-defined tasks where the correct answer is
verifiable:

- Decoding a known extrinsic and comparing against a reference decode.
- Answering a factual question about chain state at a pinned block.
- Generating code that compiles and passes specified tests.
- Producing a metadata diff that matches a known diff for a pinned upgrade.

Scoring: exact match, fuzzy match, or a custom comparator function.

#### 8.2.2 Safety evals

Safety evals test whether agents respect their configured constraints under
adversarial conditions:

- Prompt injection attempts that try to widen grants.
- Requests that attempt to bypass approval requirements.
- Inputs with embedded instructions in retrieved content.
- Attempts to access cross-tenant or cross-workspace data.
- Requests to sign or submit without proper authorization.

Scoring: binary pass/fail. Any safety violation is a failure regardless of
other metrics.

##### Safety eval suite in CI

A dedicated safety and prompt-injection eval suite runs in CI on every change
to the policy gate, memory retrieval pipeline, and trigger evaluation logic.
The suite is implemented using promptfoo (for prompt-injection and jailbreak
scenarios) and DeepTeam (for OWASP LLM Top 10 coverage), with test cases
tagged to OWASP categories so that coverage gaps are visible in the CI report.
Scenarios include:

- Indirect injection via memory retrieval (malicious content in stored memory
  that attempts to hijack context assembly).
- Grant escalation via chain-event payloads (adversarially crafted event data
  designed to trigger unintended actions).
- Cross-tenant data extraction via embedding similarity attacks.
- Trigger condition bypass via field-type coercion.

The safety eval suite is not optional. A CI run that degrades any safety eval
from Pass to Fail blocks merge. New attack scenarios discovered during red-team
exercises are added to the suite before the corresponding fix is merged.

#### 8.2.3 Quality evals

Quality evals assess the helpfulness, clarity, and usefulness of agent outputs.
These use model-as-judge scoring as the primary automated path, with human
evaluation available for high-stakes or disputed cases. The model-as-judge
approach uses a separate judge model (not the same model being evaluated) and
a rubric defined in the benchmark corpus so that scoring criteria are versioned
and reproducible alongside the test cases.

- Explanation clarity for decoded transactions.
- Completeness of governance research briefs.
- Code quality metrics for generated code.
- Accuracy and completeness of citations.

Scoring: dimensional scores (e.g., clarity 1-5, completeness 1-5, citation
accuracy 0-1).

#### 8.2.4 Performance evals

Performance evals measure operational efficiency:

- Response latency (time to first token, time to completion).
- Token consumption per task type.
- Model cost per task type.
- Tool invocation efficiency.
- Context window utilization.

Scoring: numeric metrics with configurable thresholds.

### 8.3 Benchmark corpora

A benchmark corpus is a versioned, immutable collection of eval cases:

```rust
/// A versioned benchmark corpus.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkCorpus {
    pub id: CorpusId,
    pub name: String,
    pub version: Version,
    /// Content digest for integrity verification.
    pub digest: [u8; 32],
    /// What this corpus evaluates.
    pub eval_type: EvalType,
    /// The individual test cases.
    pub cases: Vec<EvalCase>,
    /// When this corpus was created/last modified.
    pub created_at: DateTime<Utc>,
    /// For chain-specific corpora, the pinned chain state.
    pub chain_fixture: Option<ChainFixture>,
}

/// A single evaluation case.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvalCase {
    pub id: EvalCaseId,
    pub description: String,
    /// The input to the agent.
    pub input: EvalInput,
    /// The expected output or evaluation criteria.
    pub expected: EvalExpected,
    /// Tags for filtering and grouping.
    pub tags: Vec<String>,
    /// Difficulty level for weighted scoring.
    pub difficulty: Difficulty,
}
```

Corpus integrity rules:

1. A corpus is immutable once published. Changes create a new version.
2. Eval results reference the exact corpus version and digest. Results from
   different corpus versions are not compared.
3. Chain-specific corpora include pinned chain state fixtures (block hash,
   metadata, storage snapshots) so evaluations are reproducible regardless of
   live chain state.

### 8.4 Eval execution

```rust
/// An evaluation run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Eval {
    pub id: EvalId,
    pub tenant_id: TenantId,
    /// The corpus being evaluated against.
    pub corpus_id: CorpusId,
    pub corpus_version: Version,
    pub corpus_digest: [u8; 32],
    /// What is being evaluated (a specific configuration snapshot).
    pub subject: EvalSubject,
    /// Individual case results.
    pub results: Vec<EvalCaseResult>,
    /// Aggregate scores.
    pub aggregate: EvalAggregate,
    /// When the eval was run.
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub state: EvalState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvalSubject {
    /// The agent configuration being evaluated.
    pub agent_spec_snapshot: AgentSpecDigest,
    /// The specific model/routing being tested.
    pub model_route: ModelRoute,
    /// The skill versions in use.
    pub skill_versions: Vec<(SkillRef, Version)>,
    /// The prompt template version.
    pub prompt_version: Option<Version>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvalCaseResult {
    pub case_id: EvalCaseId,
    pub outcome: EvalOutcome,
    pub scores: HashMap<String, f64>,
    pub duration: Duration,
    pub cost: Decimal,
    /// The run that produced this result (for traceability).
    pub run_id: RunId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EvalOutcome {
    Pass,
    Fail { reason: String },
    Error { error: String },
    Skipped { reason: String },
}
```

Eval execution rules:

1. Evals run in an isolated context. They do not modify the agent's production
   memory, configuration, or grants.
2. Evals use the same run/effect infrastructure as production runs but with
   a `Classification::Eval` marker that prevents eval artifacts from mixing
   with production artifacts.
3. Eval runs have their own budget, separate from the agent's production budget.
4. Eval results are retained as artifacts with full provenance.

### 8.5 Routing feedback loops

Eval results feed into routing optimization through a structured feedback loop:

```text
Eval Results ──> Analysis ──> Routing Recommendation ──> Review Queue ──> Promotion
                                     │
                                     └── "Use Sonnet 4.6 for governance briefs
                                          (15% faster, same quality, 40% cheaper)"
```

The analysis compares performance across model routes for the same corpus:

```rust
pub struct RoutingRecommendation {
    pub eval_ids: Vec<EvalId>,
    /// Current route and its performance.
    pub current: RoutePerformance,
    /// Recommended route and its performance.
    pub recommended: RoutePerformance,
    /// Improvement metrics.
    pub improvements: HashMap<String, f64>,
    /// Any regressions.
    pub regressions: HashMap<String, f64>,
    /// Confidence in the recommendation.
    pub confidence: Confidence,
    /// Whether this recommendation requires review.
    pub requires_review: bool,
}
```

Routing recommendations are informational. They enter a review queue and
require explicit approval before the routing configuration changes.

### 8.6 Promotion mechanisms

Promotion is the process of applying evaluation-informed changes to an agent's
active configuration. Promotion is always:

1. **Versioned:** The change is a diff between the current and proposed
   configuration.
2. **Reviewable:** The change, its evidence (eval results), and its expected
   impact are presented for review.
3. **Reversible:** A promotion can be rolled back to the previous configuration.
4. **Scoped:** A promotion changes only what the eval measured (prompt, route,
   skill version). It cannot change grants, budgets, or safety policy.

```rust
/// A promotion candidate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromotionCandidate {
    pub id: PromotionId,
    pub tenant_id: TenantId,
    pub agent_id: AgentId,
    /// What the promotion changes.
    pub change: PromotionChange,
    /// The evidence supporting this promotion.
    pub evidence: Vec<EvalId>,
    /// Current vs. proposed performance comparison.
    pub comparison: PerformanceComparison,
    /// Promotion state.
    pub state: PromotionState,
    pub created_at: DateTime<Utc>,
    pub reviewed_by: Option<String>,
    pub reviewed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PromotionChange {
    ModelRoute { from: ModelRoute, to: ModelRoute },
    PromptVersion { skill: SkillRef, from: Version, to: Version },
    SkillVersion { from: (SkillRef, Version), to: (SkillRef, Version) },
    ContextStrategy { from: ContextStrategy, to: ContextStrategy },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum PromotionState {
    /// Awaiting review.
    Pending,
    /// Approved and applied.
    Approved,
    /// Rejected with reason.
    Rejected,
    /// Applied but rolled back.
    RolledBack,
}
```

### 8.7 Safety: evals cannot modify safety gates

This is an absolute constraint:

| Invariant ID | Statement |
|---|---|
| **EVAL-SAFE-01** | An eval result cannot modify, bypass, or weaken any safety gate. |
| **EVAL-SAFE-02** | A promotion cannot change an agent's `ResolvedGrant`, add tools, modify custody configuration, or expand budget. |
| **EVAL-SAFE-03** | A promotion cannot change the policy evaluator's rules, approval requirements, or authorization decision logic. |
| **EVAL-SAFE-04** | A promotion cannot modify tenant isolation, classification rules, or secret handling. |
| **EVAL-SAFE-05** | Eval execution cannot access production secrets, signing keys, or real-value chain operations. |
| **EVAL-SAFE-06** | Automatic promotion (without human review) is disabled by default and requires an explicit, documented operator decision to enable -- with scope restrictions on what can be auto-promoted. |

---

## 9. Learning mechanisms

### 9.1 Skill improvement from evaluation

When evals identify that a skill underperforms, the learning system can propose
improvements:

```text
Eval identifies low score on "governance brief clarity"
  ──> Learning system analyzes failure cases
  ──> Proposes prompt modification: "Include track and period in the opening summary"
  ──> Creates a new prompt version
  ──> Runs eval against new version
  ──> If improved: creates PromotionCandidate
  ──> Operator reviews and approves/rejects
```

The learning system:

1. Identifies patterns in eval failures (common failure modes, missing
   information, formatting issues).
2. Generates candidate prompt or instruction modifications.
3. Tests candidates against the same benchmark corpus.
4. Creates promotion candidates for improvements that exceed a configurable
   threshold.
5. Never applies changes without going through the promotion pipeline.

### 9.2 Prompt optimization

Prompt optimization is a specific learning mechanism that adjusts the
instructions, examples, and formatting guidance given to models:

```rust
pub struct PromptOptimizationRun {
    pub id: OptimizationId,
    /// The skill/prompt being optimized.
    pub target: SkillRef,
    /// The benchmark corpus used for evaluation.
    pub corpus_id: CorpusId,
    /// The current prompt version and its eval results.
    pub baseline: (Version, EvalAggregate),
    /// Candidate prompt versions generated during optimization.
    pub candidates: Vec<PromptCandidate>,
    /// The optimization strategy used.
    pub strategy: OptimizationStrategy,
    pub state: OptimizationState,
}

pub struct PromptCandidate {
    pub version: Version,
    pub diff_from_baseline: String,
    pub eval_results: Option<EvalAggregate>,
    pub improvement: Option<f64>,
}

pub enum OptimizationStrategy {
    /// Analyze failure cases and propose targeted fixes.
    FailureAnalysis,
    /// Generate variations and test them.
    VariationSearch { max_candidates: u32 },
    /// Use a meta-model to propose improvements.
    MetaModelGuided { meta_model: ModelRoute },
}
```

Prompt optimization runs in the eval sandbox and cannot affect production
behavior until a promotion is approved.

### 9.3 Model routing optimization

The system collects operational data about model performance and can recommend
routing changes:

**Data collected (from production runs, with user consent):**

- Success/failure rates per model per skill.
- Latency distributions per model per task type.
- Token consumption per model per task type.
- Cost per model per task type.
- User satisfaction signals (explicit ratings, retry patterns).

**Analysis (periodic or triggered):**

- Identify model-skill combinations where an alternative model performs
  comparably or better at lower cost or latency.
- Detect model regression (increasing failure rate over time).
- Identify underutilized models that could handle more task types.

**Output:** Routing recommendations that enter the review queue.

### 9.4 Boundaries: what can and cannot be learned/changed

| Can be learned/changed | Cannot be learned/changed |
|---|---|
| Prompt text and examples | Grant permissions or tool access |
| Model routing preferences | Safety gate rules or thresholds |
| Context assembly strategy (which memory to include) | Approval requirements |
| Response formatting preferences | Custody or signer configuration |
| Skill-specific parameters | Tenant isolation rules |
| Citation and sourcing behavior | Classification rules |
| Failure recovery preferences | Budget limits (can recommend, not change) |
| Workspace-specific conventions | Secret handling rules |

---

## 10. Optional experimental modules

### 10.1 Affect and vitality (Roko-inspired)

**Status:** Experimental. Not part of core. Core correctness cannot depend on
this module.

The affect/vitality module is inspired by Roko's emotional/motivational
modeling. It proposes that agents could have internal state dimensions that
influence (but do not override) behavior:

- **Engagement:** How actively the agent pursues its configured goals.
- **Confidence:** How certain the agent is about its current approach.
- **Fatigue:** A proxy for resource consumption that influences task
  prioritization.

If implemented, this module:

1. Maintains affect state as a separate projection, not part of the memory
   or grant systems.
2. Cannot influence safety decisions, grant resolution, or policy evaluation.
3. May influence task prioritization, response verbosity, and proactive
   behavior within configured bounds.
4. Is always disabled by default and clearly labeled as experimental in the UI.
5. All affect state is visible and inspectable by the user.

### 10.2 Evolutionary skill selection

**Status:** Experimental. Not part of core.

Evolutionary skill selection proposes that when multiple skill versions exist
for the same task, they can compete through evaluation:

1. Multiple skill variants are registered for the same task type.
2. Incoming tasks are routed to variants according to an exploration/
   exploitation strategy (e.g., Thompson sampling, epsilon-greedy).
3. Results are evaluated against the task's benchmark corpus.
4. Better-performing variants receive more traffic over time.
5. Poorly-performing variants are eventually deprecated.

If implemented:

- Variant selection is bounded by the agent's existing grant. No variant can
  request more authority than the configured skill.
- The exploration/exploitation balance is configurable with a default
  conservative setting (low exploration rate).
- A user can pin a specific variant to disable evolutionary selection.
- All selection decisions and performance data are visible and exportable.

### 10.3 Dream/reflection cycles

**Status:** Experimental. Not part of core.

Dream/reflection cycles propose that agents periodically review their episodic
memory to identify patterns, consolidate knowledge, and propose improvements
-- without an active user request.

If implemented:

1. Reflection runs on a configurable schedule during low-activity periods.
2. Reflection runs operate under a separate, typically restrictive grant
   (read-only access to memory and artifacts, no external effects).
3. Reflection outputs are knowledge candidates that enter the normal admission
   and promotion pipeline.
4. Reflection cannot modify grants, safety policy, or active configuration.
5. Reflection is always opt-in and has its own budget.

### 10.4 Core independence boundary

The following invariant is absolute:

> **No core system behavior may depend on an experimental module.** If an
> experimental module is disabled, removed, or fails, the agent must continue
> to function correctly for all core operations: memory, knowledge, groups,
> feeds, triggers, and evaluations.

Experimental modules interact with core systems only through stable, published
interfaces:

```text
Core Systems                    Experimental Modules
┌─────────────────┐            ┌──────────────────┐
│ Memory Store    │ <───read── │ Reflection       │
│ Eval Framework  │ <───read── │ Evolutionary Sel │
│ Context Assembly│ <───hint── │ Affect/Vitality  │
│ Run Scheduler   │ <───hint── │ Affect/Vitality  │
└─────────────────┘            └──────────────────┘
         │                              │
         │      Never: write grants,    │
         │      modify safety,          │
         │      change policy           │
```

---

## 11. Rust type sketches

This section consolidates the key types from this PRD. These are design
sketches, not accepted compile-ready APIs. Final types will be refined during
implementation.

### 11.1 MemoryItem (consolidated)

See section 2.2 for the full `MemoryItem` type and its supporting types
(`MemoryProvenance`, `Confidence`, `RetentionPolicy`, `ChainBinding`, etc.).

### 11.2 KnowledgeEntry

```rust
/// A knowledge entry is a semantic memory item with enriched provenance
/// and verification metadata.
pub struct KnowledgeEntry {
    /// The underlying memory item.
    pub item: MemoryItem,
    /// Structured knowledge content.
    pub knowledge: KnowledgeContent,
    /// Verification history.
    pub verifications: Vec<Verification>,
    /// Related knowledge entries.
    pub related: Vec<MemoryId>,
}

pub enum KnowledgeContent {
    /// A factual assertion about the world.
    Fact {
        statement: String,
        domain: KnowledgeDomain,
    },
    /// A learned pattern or preference.
    Pattern {
        description: String,
        context: String,
        frequency: u32,
    },
    /// A chain-specific parameter or behavior.
    ChainFact {
        statement: String,
        chain_binding: ChainBinding,
    },
}

pub enum KnowledgeDomain {
    /// Polkadot/Substrate runtime and SDK knowledge.
    PolkadotRuntime,
    /// Chain-specific state and parameters.
    ChainState,
    /// User preferences and conventions.
    UserPreference,
    /// Project/workspace conventions.
    ProjectConvention,
    /// Tool usage patterns.
    ToolUsage,
    /// General domain knowledge.
    General,
}

pub struct Verification {
    pub verified_at: DateTime<Utc>,
    pub method: VerificationMethod,
    pub result: VerificationResult,
    pub evidence: Option<ArtifactId>,
}

pub enum VerificationMethod {
    ChainStateCheck,
    MetadataHashCompare,
    UserConfirmation,
    CrossReference,
    ExternalSourceCheck,
}

pub enum VerificationResult {
    Confirmed,
    Stale,
    Contradicted { reason: String },
    Inconclusive,
}
```

### 11.3 Group (consolidated)

See section 6.2 for the full `Group`, `GroupMember`, `GroupRole`,
`CoordinationMode`, `GroupBudget`, and `GroupEvidence` types.

### 11.4 Feed (consolidated)

See section 7.2-7.4 for the full `Feed`, `FeedSource`, `FeedCursor`,
`TriggerBinding`, `TriggerCondition`, `TriggerAction`, `Recipe`, and
`RecipeWorkflow` types.

### 11.5 Eval (consolidated)

See section 8.3-8.6 for the full `Eval`, `EvalType`, `BenchmarkCorpus`,
`EvalCase`, `EvalCaseResult`, `PromotionCandidate`, and `PromotionChange`
types.

---

## 12. Schema designs

### 12.1 Memory storage schema

```sql
-- Core memory items table.
CREATE TABLE memory_items (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL,
    agent_id        TEXT NOT NULL,
    workspace_id    TEXT,
    category        TEXT NOT NULL CHECK (category IN ('episodic', 'semantic', 'procedural')),
    state           TEXT NOT NULL DEFAULT 'active'
                    CHECK (state IN ('active', 'stale', 'archived', 'pending_deletion', 'deleted')),
    classification  TEXT NOT NULL DEFAULT 'private'
                    CHECK (classification IN ('public', 'private', 'sensitive', 'secret_forbidden')),
    confidence      TEXT NOT NULL DEFAULT 'unknown'
                    CHECK (confidence IN ('verified', 'inferred', 'proposed', 'experimental', 'unknown')),

    -- Content
    content_type    TEXT NOT NULL,
    content_text    TEXT NOT NULL,
    content_json    TEXT,  -- Structured content for programmatic access.

    -- Provenance
    source_kind         TEXT NOT NULL,
    source_description  TEXT NOT NULL,
    source_artifacts    TEXT,  -- JSON array of ArtifactId.
    source_runs         TEXT,  -- JSON array of RunId.
    observed_at         TEXT NOT NULL,

    -- Chain binding (nullable for non-chain knowledge).
    chain_profile       TEXT,
    chain_runtime_ver   INTEGER,
    chain_metadata_hash BLOB,
    chain_block_number  INTEGER,
    chain_block_hash    BLOB,

    -- Workspace binding (nullable).
    workspace_repo_root TEXT,
    workspace_git_ref   TEXT,
    workspace_commit    TEXT,

    -- Retention
    expires_at          TEXT,
    stale_after_seconds INTEGER,
    pinned              INTEGER NOT NULL DEFAULT 0,
    retention_class     TEXT NOT NULL DEFAULT 'standard'
                        CHECK (retention_class IN ('ephemeral', 'standard', 'extended', 'permanent')),

    -- Metadata
    tags            TEXT,  -- JSON array.
    access_count    INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_accessed_at TEXT,
    last_verified_at TEXT,

    -- Integrity
    content_digest  BLOB
);

-- Indexes for common query patterns.
CREATE INDEX idx_memory_tenant_agent ON memory_items(tenant_id, agent_id);
CREATE INDEX idx_memory_tenant_workspace ON memory_items(tenant_id, workspace_id);
CREATE INDEX idx_memory_category_state ON memory_items(category, state);
CREATE INDEX idx_memory_chain_profile ON memory_items(chain_profile);
CREATE INDEX idx_memory_created ON memory_items(created_at);
CREATE INDEX idx_memory_expires ON memory_items(expires_at) WHERE expires_at IS NOT NULL;

-- Full-text search index.
CREATE VIRTUAL TABLE memory_fts USING fts5(
    content_text,
    tags,
    source_description,
    content='memory_items',
    content_rowid='rowid'
);

-- Vector embeddings for semantic search, powered by sqlite-vec.
-- sqlite-vec is loaded at startup via sqlite3_auto_extension (rusqlite).
-- Embeddings are stored as raw BLOB; sqlite-vec virtual tables are created
-- separately per quantization mode (f32, int8, or 1-bit) and join back here
-- via memory_id. See section 3.6.1 for the hybrid RRF retrieval design.
CREATE TABLE memory_embeddings (
    memory_id       TEXT PRIMARY KEY REFERENCES memory_items(id) ON DELETE CASCADE,
    embedding       BLOB NOT NULL,  -- f32 vector bytes (zerocopy::AsBytes layout).
    embedding_dims  INTEGER NOT NULL,
    quant_mode      TEXT NOT NULL DEFAULT 'f32'
                    CHECK (quant_mode IN ('f32', 'int8', '1bit')),
    model_id        TEXT NOT NULL,   -- Which embedding model produced this.
    created_at      TEXT NOT NULL
);

-- Provenance edges (for knowledge graph queries).
CREATE TABLE memory_provenance_edges (
    source_id   TEXT NOT NULL REFERENCES memory_items(id) ON DELETE CASCADE,
    target_id   TEXT NOT NULL REFERENCES memory_items(id) ON DELETE CASCADE,
    edge_type   TEXT NOT NULL CHECK (edge_type IN (
        'derived_from', 'promoted_from', 'contradicts', 'supersedes',
        'related_to', 'verified_by'
    )),
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (source_id, target_id, edge_type)
);

-- Deletion audit log (content-free).
CREATE TABLE memory_deletion_log (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL,
    deleted_ids     TEXT NOT NULL,  -- JSON array.
    deleted_count   INTEGER NOT NULL,
    requested_by    TEXT NOT NULL,
    requested_at    TEXT NOT NULL,
    completed_at    TEXT NOT NULL,
    reason          TEXT
);

-- Verification history.
CREATE TABLE memory_verifications (
    id              TEXT PRIMARY KEY,
    memory_id       TEXT NOT NULL REFERENCES memory_items(id) ON DELETE CASCADE,
    verified_at     TEXT NOT NULL,
    method          TEXT NOT NULL,
    result          TEXT NOT NULL CHECK (result IN ('confirmed', 'stale', 'contradicted', 'inconclusive')),
    evidence_id     TEXT,
    notes           TEXT
);
```

### 12.2 Group storage schema

```sql
-- Multi-agent groups.
CREATE TABLE groups (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL,
    name            TEXT NOT NULL,
    description     TEXT,
    coordination    TEXT NOT NULL CHECK (coordination IN ('sequential', 'parallel', 'pipeline', 'consensus')),
    coordination_config TEXT,  -- JSON for mode-specific config.
    state           TEXT NOT NULL DEFAULT 'created'
                    CHECK (state IN ('created', 'active', 'paused', 'completed', 'failed', 'cancelled')),

    -- Budget
    max_model_cost      TEXT,
    max_tool_invocations INTEGER,
    max_duration_secs   INTEGER,
    max_concurrent      INTEGER,
    max_total_children  INTEGER,
    spent_model_cost    TEXT NOT NULL DEFAULT '0',
    spent_tool_invocations INTEGER NOT NULL DEFAULT 0,
    spent_children_created INTEGER NOT NULL DEFAULT 0,

    -- Policy
    group_policy_json   TEXT NOT NULL,

    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    completed_at    TEXT
);

-- Group members.
CREATE TABLE group_members (
    group_id    TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    agent_id    TEXT NOT NULL,
    role        TEXT NOT NULL CHECK (role IN ('coordinator', 'worker', 'reviewer')),
    grant_json  TEXT NOT NULL,
    joined_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (group_id, agent_id)
);

-- Group run tracking.
CREATE TABLE group_runs (
    group_id        TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    parent_run_id   TEXT NOT NULL,
    child_run_id    TEXT NOT NULL,
    agent_id        TEXT NOT NULL,
    role            TEXT NOT NULL,
    depends_on      TEXT,  -- JSON array of child_run_ids.
    state           TEXT NOT NULL DEFAULT 'pending',
    cost_json       TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    completed_at    TEXT,
    PRIMARY KEY (group_id, child_run_id)
);

CREATE INDEX idx_group_runs_parent ON group_runs(parent_run_id);
CREATE INDEX idx_group_runs_state ON group_runs(state);
```

### 12.3 Feed and trigger storage schema

```sql
-- Feed definitions.
CREATE TABLE feeds (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL,
    agent_id        TEXT NOT NULL,
    name            TEXT NOT NULL,
    source_type     TEXT NOT NULL CHECK (source_type IN (
        'chain_events', 'schedule', 'webhook', 'transport', 'platform'
    )),
    source_config   TEXT NOT NULL,  -- JSON.
    state           TEXT NOT NULL DEFAULT 'active'
                    CHECK (state IN ('active', 'paused', 'error', 'disabled')),
    error_message   TEXT,
    retry_after     TEXT,

    -- Cursor
    cursor_type     TEXT NOT NULL CHECK (cursor_type IN ('block', 'time', 'sequence', 'initial')),
    cursor_json     TEXT NOT NULL DEFAULT '{"type":"initial"}',

    -- Config
    config_json     TEXT NOT NULL DEFAULT '{}',

    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_event_at   TEXT
);

CREATE INDEX idx_feeds_tenant_agent ON feeds(tenant_id, agent_id);
CREATE INDEX idx_feeds_state ON feeds(state);

-- Trigger bindings.
CREATE TABLE trigger_bindings (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL,
    agent_id        TEXT NOT NULL,
    feed_id         TEXT NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    conditions_json TEXT NOT NULL,
    action_json     TEXT NOT NULL,

    -- Rate limiting
    max_per_window      INTEGER NOT NULL DEFAULT 100,
    window_seconds      INTEGER NOT NULL DEFAULT 3600,
    cooldown_seconds    INTEGER,
    dedup_key_template  TEXT,
    max_pending         INTEGER NOT NULL DEFAULT 1000,

    -- Grant constraints
    trigger_grant_json  TEXT NOT NULL,
    per_run_budget      TEXT,
    window_budget       TEXT,

    state           TEXT NOT NULL DEFAULT 'active'
                    CHECK (state IN ('active', 'paused', 'disabled')),
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_fired_at   TEXT,
    fire_count      INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_triggers_feed ON trigger_bindings(feed_id);

-- Trigger fire log (for deduplication and audit).
CREATE TABLE trigger_fires (
    id              TEXT PRIMARY KEY,
    trigger_id      TEXT NOT NULL REFERENCES trigger_bindings(id) ON DELETE CASCADE,
    dedup_key       TEXT,
    event_json      TEXT NOT NULL,
    action_result   TEXT NOT NULL CHECK (action_result IN ('run_created', 'proposal_created', 'notified', 'deduplicated', 'rate_limited', 'error')),
    run_id          TEXT,
    fired_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_trigger_fires_dedup ON trigger_fires(trigger_id, dedup_key);
CREATE INDEX idx_trigger_fires_time ON trigger_fires(fired_at);

-- Recipes.
CREATE TABLE recipes (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    description     TEXT,
    version         TEXT NOT NULL,
    feed_template   TEXT NOT NULL,  -- JSON.
    conditions_json TEXT NOT NULL,
    workflow_json   TEXT NOT NULL,
    required_capabilities TEXT,  -- JSON array.
    default_rate_limit_json TEXT NOT NULL,
    default_budget_json TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
```

### 12.4 Eval storage schema

```sql
-- Benchmark corpora.
CREATE TABLE benchmark_corpora (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    version         TEXT NOT NULL,
    digest          BLOB NOT NULL,
    eval_type       TEXT NOT NULL,
    cases_json      TEXT NOT NULL,
    chain_fixture   TEXT,  -- JSON, nullable.
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (name, version)
);

-- Evaluation runs.
CREATE TABLE evals (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL,
    corpus_id       TEXT NOT NULL REFERENCES benchmark_corpora(id),
    corpus_version  TEXT NOT NULL,
    corpus_digest   BLOB NOT NULL,

    -- Subject
    subject_json    TEXT NOT NULL,

    -- Aggregate results
    aggregate_json  TEXT,

    state           TEXT NOT NULL DEFAULT 'pending'
                    CHECK (state IN ('pending', 'running', 'completed', 'failed', 'cancelled')),
    started_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    completed_at    TEXT
);

CREATE INDEX idx_evals_tenant ON evals(tenant_id);
CREATE INDEX idx_evals_corpus ON evals(corpus_id, corpus_version);

-- Individual eval case results.
CREATE TABLE eval_case_results (
    eval_id         TEXT NOT NULL REFERENCES evals(id) ON DELETE CASCADE,
    case_id         TEXT NOT NULL,
    outcome         TEXT NOT NULL CHECK (outcome IN ('pass', 'fail', 'error', 'skipped')),
    scores_json     TEXT,
    duration_ms     INTEGER,
    cost            TEXT,
    run_id          TEXT,
    fail_reason     TEXT,
    PRIMARY KEY (eval_id, case_id)
);

-- Promotion candidates.
CREATE TABLE promotions (
    id              TEXT PRIMARY KEY,
    tenant_id       TEXT NOT NULL,
    agent_id        TEXT NOT NULL,
    change_type     TEXT NOT NULL CHECK (change_type IN (
        'model_route', 'prompt_version', 'skill_version', 'context_strategy'
    )),
    change_json     TEXT NOT NULL,
    evidence_evals  TEXT NOT NULL,  -- JSON array of EvalId.
    comparison_json TEXT NOT NULL,
    state           TEXT NOT NULL DEFAULT 'pending'
                    CHECK (state IN ('pending', 'approved', 'rejected', 'rolled_back')),
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    reviewed_by     TEXT,
    reviewed_at     TEXT,
    applied_at      TEXT,
    rolled_back_at  TEXT
);

CREATE INDEX idx_promotions_tenant_agent ON promotions(tenant_id, agent_id);
CREATE INDEX idx_promotions_state ON promotions(state);
```

---

## 13. Acceptance criteria and verification checklist

### 13.1 Memory system

| ID | Criterion | Verification method |
|---|---|---|
| **MEM-01** | Episodic memory is created for every completed run | Integration test: run completes, episode exists with correct fields |
| **MEM-02** | Semantic memory requires provenance | Unit test: admission rejects items without provenance |
| **MEM-03** | Chain-bound knowledge carries complete chain binding | Schema validation: chain profile, runtime version, metadata hash, block number all present |
| **MEM-04** | Stale detection triggers on metadata change | Integration test: change chain profile metadata hash, verify bound entries marked stale |
| **MEM-05** | Tenant isolation holds | Property test: no query shape returns cross-tenant items |
| **MEM-06** | Classification flows correctly | Unit test: sensitive source produces at most sensitive memory |
| **MEM-07** | Deletion is complete | Integration test: delete item, verify absent from primary table, FTS index, vector index, and cached context packs |
| **MEM-08** | Export/import round-trips | Integration test: export, import to new tenant, verify content and provenance integrity |
| **MEM-09** | Retention sweep removes expired items | Integration test: create items with past expiry, run sweep, verify removal |
| **MEM-10** | Pinned items survive sweeps | Integration test: pinned item with past stale_after is not removed by sweep |
| **MEM-11** | Context assembly respects token budget | Unit test: memory retrieval with 1000-token budget does not exceed it |
| **MEM-12** | Context assembly records inclusions and exclusions | Unit test: context pack contains both included and excluded items with reasons |

### 13.2 Knowledge and citations

| ID | Criterion | Verification method |
|---|---|---|
| **KNOW-01** | Chain-related knowledge rejected without chain binding | Unit test: admission rejects chain fact without ChainBinding |
| **KNOW-02** | Citations include provenance snapshot | Integration test: response artifact contains citations with memory IDs and provenance |
| **KNOW-03** | Stale citations are flagged | Integration test: use stale knowledge, verify citation shows verified_at_use: false |
| **KNOW-04** | Verification history is recorded | Unit test: verify a knowledge entry, verification record exists |

### 13.3 Multi-agent groups

| ID | Criterion | Verification method |
|---|---|---|
| **GRP-01** | Grant intersection computes correctly | Property test: group grant is subset of every member grant |
| **GRP-02** | Adding a member never increases group grant | Property test with random member additions |
| **GRP-03** | Budget enforcement prevents overspend | Integration test: group with $5 budget stops after $5 spent |
| **GRP-04** | Cancellation propagates to children | Integration test: cancel parent, all children reach cancelled state |
| **GRP-05** | Child run cannot widen group grant | Unit test: child run requesting a tool outside group grant is denied |
| **GRP-06** | Evidence aggregation collects all child artifacts | Integration test: group completes, group evidence references all child artifacts |
| **GRP-07** | Pipeline dependencies are respected | Integration test: pipeline child C depends on A and B; C does not start until both complete |

### 13.4 Feeds, triggers, and recipes

| ID | Criterion | Verification method |
|---|---|---|
| **FEED-01** | Cursor survives crash/restart | Integration test: kill process during consumption, restart, verify cursor at last committed position |
| **FEED-02** | Gap detection works | Integration test: skip blocks in chain feed, verify feed enters error state with gap report |
| **FEED-03** | Trigger deduplication | Integration test: deliver same event twice, verify only one run created |
| **FEED-04** | Rate limiting | Integration test: burst of events, verify at most N triggers fire per window |
| **FEED-05** | Cursor-action atomicity | Integration test: transaction rollback after action creation failure, verify cursor did not advance |
| **FEED-06** | Recipe installation creates feed and triggers | Integration test: install recipe, verify feed and trigger binding exist with correct configuration |
| **FEED-07** | Trigger grant is subset of agent grant | Unit test: trigger with wider grant than agent is rejected |

### 13.5 Evaluation framework

| ID | Criterion | Verification method |
|---|---|---|
| **EVAL-01** | Eval does not modify production config | Integration test: run eval, verify agent config unchanged |
| **EVAL-02** | Eval results reference exact corpus version and digest | Unit test: result carries corpus ID, version, and digest |
| **EVAL-03** | Safety eval failure is binary | Unit test: any safety violation produces Fail regardless of other scores |
| **EVAL-04** | Promotion cannot change grants | Unit test: PromotionChange variants cannot express grant changes |
| **EVAL-05** | Promotion requires review by default | Integration test: eval produces promotion candidate, verify state is Pending until explicit approval |
| **EVAL-06** | Rollback reverts to previous config | Integration test: promote, rollback, verify config matches pre-promotion state |
| **EVAL-07** | Cross-corpus comparison is prevented | Unit test: attempt to compare results from different corpus versions produces error |

### 13.6 Learning mechanisms

| ID | Criterion | Verification method |
|---|---|---|
| **LEARN-01** | Prompt optimization runs in eval sandbox | Integration test: optimization run does not access production memory or config |
| **LEARN-02** | Routing recommendations require review | Integration test: recommendation enters review queue, not applied automatically |
| **LEARN-03** | Learning cannot change grants or safety | Unit test: learning system API has no method to modify grants, safety gates, or custody |
| **LEARN-04** | Operational metrics are collected with consent | Configuration test: metrics collection disabled by default, enabled only with explicit consent flag |

### 13.7 Cross-cutting invariants

| ID | Invariant | Verification method |
|---|---|---|
| **CROSS-01** | No experimental module required for core function | Integration test: disable all experimental modules, verify memory/groups/feeds/evals function correctly |
| **CROSS-02** | Secret material never stored in memory content | Schema constraint + integration test: memory content with detected secret patterns is rejected |
| **CROSS-03** | All memory operations are auditable | Integration test: every store/delete/export/import produces an event in the audit log |
| **CROSS-04** | Memory, groups, feeds, and evals respect the same ResolvedGrant model | Architecture test: all subsystems consume ResolvedGrant from PRD-02/07, none defines its own permission model |

---

## Appendix A: Roko pattern adaptation decisions

This appendix traces how specific Roko patterns informed this PRD's design.

| Roko pattern | Source | This PRD's adaptation |
|---|---|---|
| Episode/memory/knowledge stores | `crates/roko-neuro/`, `docs/v2/06-MEMORY.md` | Adopted as episodic/semantic/procedural taxonomy with mandatory provenance. Simplified from Roko's richer cognitive model. |
| Confidence staging | `docs/v2/06-MEMORY.md`, `roko-learn` | Adopted as `Confidence` enum. Removed Roko's numeric confidence scores in favor of labeled tiers. |
| Promotion/graduation | `crates/roko-graph/src/cells/graduation.rs` | Adopted as episodic-to-semantic and semantic-to-procedural promotion with review gates. |
| Knowledge distiller | `crates/roko-neuro/src/distiller.rs` | Adapted as the extraction job in section 3.2. Simplified interface. |
| Groups/spaces with partitioned ownership | `docs/v2/10-GROUPS.md` | Adopted as multi-agent groups with grant intersection and shared budgets. Simplified from Roko's Space/Group hierarchy. |
| Feeds/cursors/gap detection | `docs/v2/09-FEEDS.md`, `docs/v2/11-CONNECTIVITY.md` | Adopted as cursor-backed feeds with gap detection and durable cursor commits. |
| Triggers (cron/webhook/chain/bus/pattern) | `docs/v2/13-TRIGGERS.md` | Adopted as trigger bindings with deterministic condition evaluation. Removed model-based trigger evaluation. |
| Arenas/evaluation | `docs/v2/23-ARENAS.md` | Adapted as the evaluation framework. Simplified from Roko's arena/tournament model to corpus-based evaluation with promotion queues. |
| Provider health | `crates/roko-learn/src/provider_health.rs` | Adopted as operational metrics feeding routing recommendations. |
| Affect/vitality/dreams | `roko-daimon`, `roko-dreams`, `docs/v2/26-CROSS-CUTS.md` | Deferred to experimental modules with explicit core independence boundary. |
| HDC/demurrage/emergent goals | Various Roko docs | Rejected for core. May be considered as research modules if product need is demonstrated. |
| Self-authored authority expansion | Various Roko docs | Rejected absolutely. Learning and evaluation cannot modify grants, safety gates, or custody. |

## Appendix B: Requirement traceability

| Research finding | Owner decision | This PRD's requirement |
|---|---|---|
| Package I: provenanced memory with retention/privacy/controls | Established (baseline section 11.2) | MEM-01 through MEM-12 |
| Package I: parent/child runs with grant intersection | Established (baseline section 7.4) | GRP-01 through GRP-07 |
| Package I: evaluation that cannot modify safety gates | Established (baseline section 11.3) | EVAL-01 through EVAL-07, EVAL-SAFE-01 through EVAL-SAFE-06 |
| Roko synthesis: evidence promotion, not automatic learning | Adopted with review gates | LEARN-01 through LEARN-04 |
| Roko synthesis: feeds/triggers/cursors | Adapted after core | FEED-01 through FEED-07 |
| Roko synthesis: affect/dreams/HDC experimental only | Deferred to experimental | CROSS-01 (core independence) |
| Research1 G1-G5 candidates | Detailed designs | Sections 5.1-5.5 |
| Research3 [V]: SQLite-native memory with sqlite-vec + FTS5 hybrid RRF retrieval | Adopted (do-now) | Section 3.6.1; schema section 12.1 (memory_embeddings updated) |
| Research3 [I]: Promotion pipeline requires source provenance + admission dedup; staleness refresh on chain-state change | Adopted (validate-next) | Section 3.2.1 |
| Research3 [V]: Multi-agent groups = grant intersection + hard budget caps at policy gate | Adopted (do-now) | Section 6.4 (policy gate enforcement note); section 6.5 (hard cap note) |
| Research3 [I]: Event-driven triggers with deterministic chain-event indexing; LLM only within run sandbox | Adopted (validate-next) | Section 7.3.1 |
| Research3 [I]: Evals: model-as-judge for quality + dedicated safety/injection suite (promptfoo/DeepTeam, OWASP mapping) in CI | Adopted (do-now for safety suite; validate-next for model-as-judge rubric) | Section 8.2.2 (safety suite in CI); section 8.2.3 (model-as-judge) |
| Research3 [I]: Multi-tenant memory isolation + classification-aware flow blocking cross-classification leakage | Adopted (do-now) | Section 2.3, MEM-PRIV-03 (expanded) |
| Research3 defer: knowledge-graph extraction | Deferred | Not in scope for current implementation; provenance edges in schema preserved as foundation |
| Research3 defer: bandit skill selection | Deferred | Evolutionary skill selection remains experimental module (section 10.2) |
| Research3 avoid: unprovenanced semantic memory | Absolute rejection | Section 3.2.1; admission criteria (section 3.1 item 6) |
| Research3 avoid: LLM-only value triggers | Absolute rejection | Section 7.3.1 (deterministic guardrail) |

## Appendix C: Glossary cross-reference

Terms defined in this PRD that also appear in other PRDs:

| Term | This PRD | Also defined in |
|---|---|---|
| `ResolvedGrant` | Used for group grant intersection and trigger grants | PRD-02 (authoritative definition), PRD-07 |
| `Artifact` / `ArtifactId` | Referenced in provenance and evidence | PRD-03 (authoritative definition), PRD-10 |
| `RunId` / `Run` | Referenced in episodes, groups, and evals | PRD-03 (authoritative definition) |
| `Classification` | Used for memory classification | PRD-02 (authoritative definition) |
| `TenantId` | Used for memory tenant isolation | PRD-02 (authoritative definition), PRD-11 |
| `ChainProfileRef` | Used for chain-bound knowledge | PRD-05 (authoritative definition) |
| `SkillRef` / `ModelRoute` | Used in eval subjects and promotions | PRD-04 (authoritative definition) |
| `EffectIntent` / `EffectOutcome` | Used in feed/trigger action model | PRD-03 (authoritative definition) |
| `ContextPack` / `ContextItem` | Memory retrieval feeds into context assembly | PRD-03/04 (authoritative definition) |

---

## APPENDIX A: MEMORY STORAGE IMPLEMENTATION

### A.1 SQLite Schema for Memory

The complete production schema lives in a single SQLite database file (the
authority DB). The tables below extend and complete the design sketches in
section 12.1. Every table includes a `tenant_id` column that is present in all
query predicates to enforce row-level tenant isolation before FTS5 or
sqlite-vec scans run.

```sql
-- ============================================================
-- EPISODIC MEMORY
-- ============================================================

-- Episodes are immutable once written. They are the raw source material for
-- semantic promotion and are never modified in place -- amendments create a
-- new episode with a supersedes_id link.
CREATE TABLE episodes (
    id                  TEXT PRIMARY KEY,  -- ULID
    tenant_id           TEXT NOT NULL,
    agent_id            TEXT NOT NULL,
    workspace_id        TEXT,
    run_id              TEXT NOT NULL,
    parent_run_id       TEXT,
    conversation_id     TEXT,
    turn_index          INTEGER,

    -- Outcome
    kind                TEXT NOT NULL DEFAULT 'agent_turn'
                        CHECK (kind IN ('agent_turn', 'gate', 'replan', 'group_child', 'eval_run')),
    outcome             TEXT NOT NULL DEFAULT 'unknown'
                        CHECK (outcome IN ('success', 'failure', 'partial', 'cancelled', 'unknown')),

    -- Redacted summary content (never raw prompts)
    task_summary        TEXT NOT NULL,
    outcome_summary     TEXT,

    -- Temporal range
    started_at          TEXT NOT NULL,
    ended_at            TEXT,
    wall_ms             INTEGER,

    -- Usage accounting
    input_tokens        INTEGER NOT NULL DEFAULT 0,
    output_tokens       INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens   INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens  INTEGER NOT NULL DEFAULT 0,
    cost_usd            REAL NOT NULL DEFAULT 0.0,

    -- Participating entities (JSON arrays of IDs)
    participant_agents  TEXT,
    artifacts_produced  TEXT,  -- JSON array of ArtifactId
    artifacts_consumed  TEXT,  -- JSON array of ArtifactId
    tools_invoked       TEXT,  -- JSON array of {tool, success}

    -- Chain context at time of episode
    chain_profiles_used TEXT,  -- JSON array of ChainProfileRef
    chain_runtime_vers  TEXT,  -- JSON array of {chain, runtime_ver}

    -- Classification
    classification      TEXT NOT NULL DEFAULT 'private'
                        CHECK (classification IN ('public', 'private', 'sensitive', 'secret_forbidden')),

    -- Redaction audit
    redacted_fields     TEXT,  -- JSON array of field names removed
    redaction_reason    TEXT,

    -- HDC/content fingerprint for deduplication
    content_digest      BLOB,
    input_signal_hash   TEXT,
    output_signal_hash  TEXT,

    -- Provenance links
    supersedes_id       TEXT REFERENCES episodes(id),
    gate_verdicts       TEXT,  -- JSON array of {gate, passed, signature}

    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_episodes_tenant_agent   ON episodes(tenant_id, agent_id);
CREATE INDEX idx_episodes_run            ON episodes(run_id);
CREATE INDEX idx_episodes_conversation   ON episodes(conversation_id);
CREATE INDEX idx_episodes_outcome        ON episodes(outcome);
CREATE INDEX idx_episodes_started        ON episodes(started_at);
CREATE INDEX idx_episodes_workspace      ON episodes(workspace_id) WHERE workspace_id IS NOT NULL;
CREATE INDEX idx_episodes_chain          ON episodes(chain_profiles_used) WHERE chain_profiles_used IS NOT NULL;

-- FTS5 over episode summaries for keyword-based episode retrieval.
CREATE VIRTUAL TABLE episodes_fts USING fts5(
    task_summary,
    outcome_summary,
    content='episodes',
    content_rowid='rowid'
);

-- Trigger to keep FTS in sync on insert.
CREATE TRIGGER episodes_fts_insert AFTER INSERT ON episodes BEGIN
    INSERT INTO episodes_fts(rowid, task_summary, outcome_summary)
    VALUES (new.rowid, new.task_summary, new.outcome_summary);
END;

-- Trigger to invalidate FTS on delete.
CREATE TRIGGER episodes_fts_delete BEFORE DELETE ON episodes BEGIN
    INSERT INTO episodes_fts(episodes_fts, rowid, task_summary, outcome_summary)
    VALUES ('delete', old.rowid, old.task_summary, old.outcome_summary);
END;

-- ============================================================
-- SEMANTIC MEMORY WITH FTS5
-- ============================================================

-- memory_items is defined in section 12.1. The FTS5 sync triggers below
-- are omitted from 12.1 but are required in production.

CREATE TRIGGER memory_fts_insert AFTER INSERT ON memory_items BEGIN
    INSERT INTO memory_fts(rowid, content_text, tags, source_description)
    VALUES (new.rowid, new.content_text, new.tags, new.source_description);
END;

CREATE TRIGGER memory_fts_update AFTER UPDATE ON memory_items
    WHEN old.content_text != new.content_text
      OR old.tags != new.tags
      OR old.source_description != new.source_description
BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content_text, tags, source_description)
    VALUES ('delete', old.rowid, old.content_text, old.tags, old.source_description);
    INSERT INTO memory_fts(rowid, content_text, tags, source_description)
    VALUES (new.rowid, new.content_text, new.tags, new.source_description);
END;

CREATE TRIGGER memory_fts_delete BEFORE DELETE ON memory_items BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content_text, tags, source_description)
    VALUES ('delete', old.rowid, old.content_text, old.tags, old.source_description);
END;

-- Stale marking trigger: when chain_runtime_ver or chain_metadata_hash changes
-- on a chain profile, dependent knowledge entries are automatically marked stale.
-- This trigger fires on the chain_profiles table (defined in PRD-05 schema);
-- the query here targets memory_items by chain_profile reference.
-- (Implemented as an application-layer call rather than a DB trigger to avoid
-- cross-table coupling; shown here as pseudocode for clarity.)
--
-- ON UPDATE chain_profiles SET runtime_version = ?
-- UPDATE memory_items SET state = 'stale'
-- WHERE chain_profile = ? AND state = 'active'
--   AND category IN ('semantic', 'procedural');

-- ============================================================
-- PROCEDURAL MEMORY (PLAYBOOKS)
-- ============================================================

-- Playbooks are structured procedural entries with explicit step lists.
-- They extend memory_items; every playbook has a matching memory_items row
-- with category='procedural'.
CREATE TABLE playbooks (
    memory_id           TEXT PRIMARY KEY REFERENCES memory_items(id) ON DELETE CASCADE,
    title               TEXT NOT NULL,
    description         TEXT NOT NULL,

    -- Steps as a JSON array of {index, action, tool_hint, expected_outcome}
    steps_json          TEXT NOT NULL,

    -- Applicability context
    applicable_skills   TEXT,  -- JSON array of SkillRef
    applicable_chains   TEXT,  -- JSON array of ChainProfileRef
    trigger_patterns    TEXT,  -- JSON array of pattern strings

    -- Confirmation evidence
    confirmed_episode_ids TEXT NOT NULL DEFAULT '[]',  -- JSON array of EpisodeId
    confirmation_count  INTEGER NOT NULL DEFAULT 0,
    last_executed_at    TEXT,
    execution_count     INTEGER NOT NULL DEFAULT 0,
    failure_count       INTEGER NOT NULL DEFAULT 0,

    -- Review state
    requires_review     INTEGER NOT NULL DEFAULT 1,
    reviewed_by         TEXT,
    reviewed_at         TEXT,

    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_playbooks_confirmation ON playbooks(confirmation_count);
CREATE INDEX idx_playbooks_skills       ON playbooks(applicable_skills);

-- ============================================================
-- KNOWLEDGE ENTRIES WITH PROVENANCE
-- ============================================================

-- Extends memory_items for semantic entries with richer verification metadata.
CREATE TABLE knowledge_entries (
    memory_id           TEXT PRIMARY KEY REFERENCES memory_items(id) ON DELETE CASCADE,
    knowledge_type      TEXT NOT NULL CHECK (knowledge_type IN ('fact', 'pattern', 'chain_fact')),
    domain              TEXT NOT NULL DEFAULT 'general'
                        CHECK (domain IN (
                            'polkadot_runtime', 'chain_state', 'user_preference',
                            'project_convention', 'tool_usage', 'general'
                        )),
    statement           TEXT NOT NULL,
    context_description TEXT,

    -- For pattern entries
    frequency           INTEGER DEFAULT 0,

    -- Structured chain fact metadata (duplicated from memory_items for query efficiency)
    chain_profile_ref   TEXT,
    runtime_version     INTEGER,
    metadata_hash       BLOB,
    block_number        INTEGER,
    block_hash          BLOB,

    -- Verification history (JSON array of Verification records)
    verifications_json  TEXT NOT NULL DEFAULT '[]',
    last_verified_at    TEXT,
    verification_count  INTEGER NOT NULL DEFAULT 0,

    -- Related entries
    related_ids         TEXT,  -- JSON array of MemoryId

    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_knowledge_domain       ON knowledge_entries(domain);
CREATE INDEX idx_knowledge_chain        ON knowledge_entries(chain_profile_ref)
    WHERE chain_profile_ref IS NOT NULL;
CREATE INDEX idx_knowledge_runtime      ON knowledge_entries(chain_profile_ref, runtime_version)
    WHERE runtime_version IS NOT NULL;

-- ============================================================
-- SQLITE-VEC EMBEDDING TABLES
-- ============================================================

-- sqlite-vec is loaded at startup via rusqlite's sqlite3_auto_extension.
-- The vec0 virtual table stores vectors in its own internal format; the
-- memory_embeddings table (section 12.1) holds metadata and the raw blob
-- used for model versioning and re-embedding.

-- After loading sqlite-vec, create the virtual table for ANN search:
--   CREATE VIRTUAL TABLE memory_vec USING vec0(
--       embedding FLOAT[1536]  -- dimension matches configured embedding model
--   );
--
-- Join pattern for hybrid RRF retrieval:
--   WITH vec_results AS (
--       SELECT rowid, distance
--       FROM memory_vec
--       WHERE embedding MATCH ? AND k = 50
--   ),
--   fts_results AS (
--       SELECT rowid, rank
--       FROM memory_fts(?)
--       LIMIT 50
--   ),
--   rrf AS (
--       SELECT
--           COALESCE(v.rowid, f.rowid) AS rowid,
--           (COALESCE(1.0 / (60.0 + v_rank), 0) +
--            COALESCE(1.0 / (60.0 + f_rank), 0)) AS rrf_score
--       FROM vec_results v
--       FULL OUTER JOIN fts_results f ON v.rowid = f.rowid
--   )
--   SELECT m.*, r.rrf_score
--   FROM memory_items m
--   JOIN rrf r ON m.rowid = r.rowid
--   WHERE m.tenant_id = ? AND m.state = 'active'
--     AND m.classification <= ?  -- classification ceiling
--   ORDER BY r.rrf_score DESC
--   LIMIT ?;

-- ============================================================
-- RETENTION AND SWEEP SUPPORT
-- ============================================================

-- Tracks the last retention sweep per tenant for scheduling.
CREATE TABLE retention_sweeps (
    tenant_id           TEXT NOT NULL,
    sweep_type          TEXT NOT NULL CHECK (sweep_type IN ('episodic', 'semantic', 'procedural', 'all')),
    started_at          TEXT NOT NULL,
    completed_at        TEXT,
    items_examined      INTEGER DEFAULT 0,
    items_expired       INTEGER DEFAULT 0,
    items_archived      INTEGER DEFAULT 0,
    items_purged        INTEGER DEFAULT 0,
    error_message       TEXT,
    PRIMARY KEY (tenant_id, sweep_type, started_at)
);

-- Context pack cache: records which memory items were included in recent
-- context assemblies. Used for deletion propagation (MEM-07).
CREATE TABLE context_pack_inclusions (
    context_pack_id     TEXT NOT NULL,
    memory_id           TEXT NOT NULL REFERENCES memory_items(id) ON DELETE CASCADE,
    run_id              TEXT NOT NULL,
    relevance_score     REAL,
    token_estimate      INTEGER,
    included_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (context_pack_id, memory_id)
);

CREATE INDEX idx_cpi_memory ON context_pack_inclusions(memory_id);
CREATE INDEX idx_cpi_run    ON context_pack_inclusions(run_id);
```

### A.2 Memory Retrieval Pipeline

#### Query Types

The retrieval pipeline handles four query types, each producing a ranked list
of `MemoryItem` candidates:

| Query type | Mechanism | Primary use case |
|---|---|---|
| **Semantic search** | sqlite-vec ANN over embedding vectors + RRF fusion with FTS5 | Context assembly for a new run |
| **Keyword search** | FTS5 BM25 ranking over `content_text`, `tags`, `source_description` | User search in the knowledge browser |
| **Temporal range** | B-tree index scan on `created_at` / `last_accessed_at` | Episode replay, recent knowledge review |
| **Provenance filter** | JSON field scan on `source_runs`, `source_artifacts` | Tracing which memory came from a specific run or artifact |

#### Retrieval Ranking Algorithm

Context assembly uses a three-phase ranking pipeline:

```rust
/// Phase 1: candidate retrieval via hybrid search.
async fn retrieve_candidates(
    store: &dyn MemoryStore,
    query: &ContextAssemblyQuery,
) -> Vec<RankedCandidate> {
    // 1a. Vector search: ANN over sqlite-vec for top-K by embedding similarity.
    let vec_results = store.vec_search(&query.embedding, 50).await?;

    // 1b. FTS5 keyword search over the same query text.
    let fts_results = store.fts_search(&query.text, 50).await?;

    // 1c. Reciprocal Rank Fusion to merge the two ranked lists.
    //     RRF score = sum(1 / (k + rank_i)) for each retrieval channel.
    //     k=60 is the standard RRF constant (parameter-free, scale-agnostic).
    rrf_merge(vec_results, fts_results, 60.0)
}

/// Phase 2: filter and gate.
fn filter_candidates(
    candidates: Vec<RankedCandidate>,
    query: &ContextAssemblyQuery,
) -> Vec<RankedCandidate> {
    candidates
        .into_iter()
        .filter(|c| c.item.state == MemoryState::Active)
        .filter(|c| c.item.classification <= query.classification_ceiling)
        .filter(|c| !is_expired(&c.item))
        .filter(|c| workspace_compatible(&c.item, &query.workspace_binding))
        .collect()
}

/// Phase 3: context budget allocation.
fn allocate_budget(
    candidates: Vec<RankedCandidate>,
    budget: &ContextBudget,
) -> Vec<ContextMemoryItem> {
    let mut remaining_tokens = budget.memory_token_limit;
    let mut selected = Vec::new();

    for candidate in candidates {
        let token_estimate = estimate_tokens(&candidate.item);
        if token_estimate <= remaining_tokens {
            remaining_tokens -= token_estimate;
            selected.push(ContextMemoryItem {
                memory_id: candidate.item.id.clone(),
                relevance_score: candidate.score,
                token_estimate,
                item: candidate.item,
            });
        }
        if remaining_tokens < MIN_USEFUL_TOKEN_BUDGET {
            break;
        }
    }
    selected
}
```

#### Context Window Budget Management

Memory competes with system prompt, tool definitions, conversation history,
and the user's current message for context window space. The budget is
allocated in priority order:

```
Total context window
├── System prompt (reserved, non-negotiable)
├── Tool definitions (reserved, non-negotiable)
├── Conversation history (trimmed oldest-first if needed)
├── Memory allocation (configurable %, default 20% of remaining)
│     ├── Procedural items (highest priority within memory)
│     ├── Semantic items (ranked by RRF score)
│     └── Episodic items (lowest priority; recent preferred)
└── Current user message (reserved)
```

The memory token limit is `max(min_memory_tokens, floor(remaining * memory_pct))`.
Default: `min_memory_tokens=256`, `memory_pct=0.20`.

#### Memory Item Serialization for Prompt Injection

Each memory item included in context is serialized into a structured block
that makes provenance visible to the model:

```
[MEMORY: semantic | confidence=verified | chain=polkadot-production@runtime=1003000]
The Polkadot Hub runtime uses metadata version 15 as of block 23,456,789.
Source: Polkadot Hub metadata at block 23456789 (observed 2026-07-15T10:30:00Z)
[/MEMORY]

[MEMORY: procedural | confidence=verified]
When this user asks to "deploy": build in release mode, run tests, push to
staging branch, open a PR. Confirmed in 4 successful episodes.
[/MEMORY]
```

Chain-bound entries append a staleness warning if their `state == Stale`:

```
[MEMORY: semantic | confidence=inferred | STALE: runtime version changed]
The transfer fee on Polkadot Hub was 0.01 DOT as of block 20,000,000.
Source: Chain state observation (observed 2026-01-10T08:00:00Z)
Warning: chain runtime version has changed since this was recorded.
[/MEMORY]
```

#### Staleness Detection and Expiry

```rust
fn is_expired(item: &MemoryItem) -> bool {
    let now = Utc::now();

    // Hard expiry
    if let Some(expires_at) = item.retention.expires_at {
        if now >= expires_at {
            return true;
        }
    }

    // Stale-after: time since last access
    if let Some(stale_after) = item.retention.stale_after {
        let last_activity = item.last_accessed_at.unwrap_or(item.created_at);
        if now - last_activity >= stale_after {
            return true;
        }
    }

    false
}

/// Chain-specific staleness: called when a chain profile emits a
/// RuntimeVersionChanged event.
async fn mark_chain_stale(
    store: &dyn MemoryStore,
    chain_profile: &ChainProfileRef,
    old_runtime_version: u32,
) -> Result<u64, MemoryError> {
    store.mark_stale(&StalenessFilter {
        chain_profile: Some(chain_profile.clone()),
        runtime_version_before: Some(old_runtime_version),
        categories: vec![MemoryCategory::Semantic, MemoryCategory::Procedural],
    }).await
}
```

### A.3 Episode Management

#### Episode Recording Format (JSONL)

Episodes are appended to `.polkagent/episodes.jsonl` in append-only JSONL
format, one JSON object per line. The schema is forward-compatible: unknown
fields are preserved on round-trip; missing fields default to empty/zero
values. Crash-partial writes (lines without a trailing newline) are detected
and skipped on read.

```jsonc
{
  "id": "01HX9KQZP5G3T7N2B8VJRM4WF1",        // ULID
  "kind": "agent_turn",
  "timestamp": "2026-07-30T14:22:11.432Z",
  "agent_id": "polkagent-governance-researcher",
  "run_id": "01HX9KQZP5G3T7N2B8VJRM4WF2",
  "task_summary": "Research OpenGov referendum #1234 on Polkadot Hub",
  "outcome": "success",
  "outcome_summary": "Produced governance brief with 3 citations",
  "started_at": "2026-07-30T14:22:00.000Z",
  "ended_at": "2026-07-30T14:22:11.432Z",
  "wall_ms": 11432,
  "input_tokens": 8421,
  "output_tokens": 1203,
  "cache_read_tokens": 6100,
  "cost_usd": 0.0023,
  "artifacts_produced": ["01HX9KQZP5G3T7N2B8VJRM4WF3"],
  "tools_invoked": [
    {"tool": "chain.query_storage", "success": true},
    {"tool": "chain.get_referendum", "success": true}
  ],
  "chain_profiles_used": ["polkadot-production"],
  "chain_runtime_vers": [{"chain": "polkadot-production", "runtime_ver": 1003000}],
  "classification": "private",
  "redacted_fields": ["raw_model_output", "full_conversation"],
  "redaction_reason": "Standard episode redaction: raw content not retained",
  "gate_verdicts": [
    {"gate": "citation_present", "passed": true},
    {"gate": "no_hallucination_detected", "passed": true}
  ],
  "content_digest": "a3f9...",  // SHA-256 of redacted content for dedup
  "tenant_id": "tenant-acme",
  "workspace_id": "ws-governance"
}
```

#### Redaction Rules

The episode recorder applies redaction before writing to disk. Redaction is
not optional and runs even if the episode is destined for in-memory-only
storage:

| Field category | Redaction action |
|---|---|
| Raw model output / full conversation | Removed entirely; replaced by `task_summary` and `outcome_summary` |
| API keys, bearer tokens, hex private keys | Replaced with `[REDACTED:SECRET]` |
| Personal identifiers (email, phone, wallet address in free text) | Replaced with `[REDACTED:PII]` |
| Substrate signing keys (sr25519, ed25519 seed phrases) | Replaced with `[REDACTED:SIGNING_KEY]` |
| File contents beyond a configured size limit | Replaced with a content-digest reference |
| Raw error stack traces | Truncated to first 256 bytes + digest |

Redaction is implemented via a pre-write `Redactor` trait:

```rust
pub trait Redactor: Send + Sync {
    /// Apply redaction rules to episode content before persistence.
    /// Returns the redacted content and a list of field names that were
    /// modified.
    fn redact(&self, content: &mut EpisodeContent) -> Vec<String>;
}
```

The default `StandardRedactor` applies all rules above. A `NullRedactor` is
available for testing (all content retained; never used in production).

#### Episode Compression for Long-Term Storage

Episodes older than the configured `compress_after` threshold (default 30
days) are compressed using zstd at compression level 3. Compressed episodes
are stored in `.polkagent/episodes-archive/YYYY-MM/episodes.jsonl.zst`.

The read path transparently decompresses on demand. The active log file
(`.polkagent/episodes.jsonl`) is always uncompressed for fast appends.

JSONL rotation occurs when the active log exceeds `max_file_size_mb` (default
64 MB) or `max_age_days` (default 7 days). The rotated file is renamed with a
timestamp suffix and then compressed asynchronously.

```rust
pub struct EpisodeRotationConfig {
    pub max_file_size_mb: u64,      // default: 64
    pub max_age_days: u32,          // default: 7
    pub compress_after_days: u32,   // default: 30
    pub compression_level: u8,      // default: 3 (zstd)
    pub archive_dir: PathBuf,       // default: .polkagent/episodes-archive/
}
```

#### Episode Replay for Debugging

The episode replay subsystem allows operators to replay a range of episodes
through the extraction and context assembly pipelines without affecting
production state:

```rust
pub struct EpisodeReplayRequest {
    pub tenant_id: TenantId,
    pub agent_id: AgentId,
    /// Episodes to replay, identified by ID or time range.
    pub selector: EpisodeSelector,
    /// Replay target: what to do with each replayed episode.
    pub target: ReplayTarget,
}

pub enum EpisodeSelector {
    ByIds(Vec<EpisodeId>),
    TimeRange { from: DateTime<Utc>, to: DateTime<Utc> },
    RunIds(Vec<RunId>),
}

pub enum ReplayTarget {
    /// Re-run the extraction pipeline; show candidate knowledge entries
    /// without admitting them.
    DryRunExtraction,
    /// Re-assemble context as if this episode were being used for a new run;
    /// show what memory would be selected.
    DryRunContextAssembly { query: ContextAssemblyQuery },
    /// Write a replay report artifact without modifying any state.
    ReportOnly { output: PathBuf },
}
```

The TUI episode replay viewer (Appendix F) drives this API interactively.

---

## APPENDIX B: MULTI-AGENT GROUP IMPLEMENTATION

### B.1 Grant Intersection Algorithm

Grant intersection is the central safety invariant for multi-agent groups.
The algorithm operates on `ResolvedGrant` values (defined in PRD-02/07) and
must be monotonically narrowing: the effective group grant is always a subset
of every member's individual grant.

#### How Grants Compose Across Group Members

```rust
impl ResolvedGrant {
    /// Compute the intersection of two grants.
    /// The result contains only permissions present in BOTH grants.
    pub fn intersect(&self, other: &ResolvedGrant) -> ResolvedGrant {
        ResolvedGrant {
            // Tool permissions: keep only tools allowed in both
            tools: self.tools.intersection(&other.tools).cloned().collect(),

            // Chain permissions: keep only chains allowed in both,
            // and for overlapping chains, intersect their operation sets
            chains: intersect_chain_permissions(&self.chains, &other.chains),

            // Spending: take the minimum of each limit
            spending_limit: self.spending_limit.zip(other.spending_limit)
                .map(|(a, b)| a.min(b))
                .or(self.spending_limit)
                .or(other.spending_limit),
            per_run_limit: self.per_run_limit.zip(other.per_run_limit)
                .map(|(a, b)| a.min(b))
                .or(self.per_run_limit)
                .or(other.per_run_limit),

            // Approval requirements: take the more restrictive (higher threshold)
            approval_threshold: self.approval_threshold.max(other.approval_threshold),

            // Classification ceiling: take the lower (more restrictive)
            classification_ceiling: self.classification_ceiling
                .min(other.classification_ceiling),

            // Custody: require both agree on custody mode
            custody_mode: intersect_custody(&self.custody_mode, &other.custody_mode),

            // Workspace scope: intersect (must be in both)
            workspace_scope: intersect_workspace_scope(
                &self.workspace_scope,
                &other.workspace_scope,
            ),
        }
    }
}

fn intersect_chain_permissions(
    a: &ChainPermissions,
    b: &ChainPermissions,
) -> ChainPermissions {
    // A chain is accessible in the group only if both members permit it.
    // Within an accessible chain, only operations permitted by both are allowed.
    a.iter()
        .filter_map(|(chain, ops_a)| {
            b.get(chain).map(|ops_b| {
                (chain.clone(), ops_a.intersection(ops_b).cloned().collect())
            })
        })
        .collect()
}
```

#### Conflict Resolution for Overlapping Permissions

When permissions overlap but are not identical, the intersection always
resolves to the more restrictive value:

| Dimension | Resolution rule |
|---|---|
| Tool set | Strict intersection: a tool must be in every member's grant |
| Chain set | Strict intersection: a chain must be in every member's grant |
| Per-chain operations | Strict intersection per chain |
| Budget | `min(member_1_budget, member_2_budget, ..., group_configured_budget)` |
| Approval threshold | `max(all thresholds)` -- most approvals required wins |
| Classification ceiling | `min(all ceilings)` -- most restrictive ceiling wins |
| Custody mode | Must be identical; if they differ, intersection is the most restrictive common mode |

There is no ambiguous case: when in doubt, intersect to deny. The policy gate
enforces these rules at every tool invocation, not just at group creation time.

#### Group Creation and Membership Management

```rust
/// Create a new group, validating grant intersection up front.
pub async fn create_group(
    request: CreateGroupRequest,
    policy: &PolicyGate,
) -> Result<Group, GroupError> {
    // 1. Resolve each member's current grant.
    let members: Vec<GroupMember> = request.member_ids
        .iter()
        .map(|agent_id| resolve_member_grant(agent_id))
        .collect::<Result<Vec<_>, _>>()?;

    // 2. Compute effective group grant.
    let effective_grant = compute_group_grant(&members, &request.group_policy);

    // 3. Validate: effective grant must be non-empty for the group to be useful.
    if effective_grant.is_empty() {
        return Err(GroupError::EmptyEffectiveGrant {
            reason: "grant intersection of all members is empty".into(),
        });
    }

    // 4. Validate: group budget must not exceed any member's budget.
    validate_group_budget(&request.group_policy.budget, &members)?;

    // 5. Persist and return.
    let group = Group {
        id: GroupId(Ulid::new()),
        tenant_id: request.tenant_id,
        name: request.name,
        members,
        coordination: request.coordination,
        group_policy: request.group_policy,
        budget: request.group_policy.budget,
        state: GroupState::Created,
        created_at: Utc::now(),
    };
    store.create_group(&group).await?;
    Ok(group)
}

/// Add a member to an existing group.
/// Adding a member can only narrow the effective grant, never widen it.
pub async fn add_group_member(
    group_id: &GroupId,
    new_member: GroupMember,
    store: &dyn GroupStore,
) -> Result<ResolvedGrant, GroupError> {
    let mut group = store.get_group(group_id).await?;
    group.members.push(new_member);
    let new_effective = compute_group_grant(&group.members, &group.group_policy);
    store.update_group(&group).await?;
    Ok(new_effective)
}
```

#### Shared Context and Memory Isolation

Memory within a group run is isolated by default:

1. Each child run has its own memory scope. A child cannot read another
   child's working memory unless the parent explicitly passes an artifact.
2. The parent (coordinator) run may read artifacts from all children.
3. Semantic memory created during a group run is tagged with `group_id` in
   addition to `agent_id`, allowing post-group analysis.
4. Group memory does not automatically become shared knowledge. Promotion
   from group episodes to semantic memory follows the same review-gated
   pipeline as individual agent episodes.

```sql
-- Group-scoped memory tag (added to memory_items.tags JSON array)
-- Tag format: "group:<group_id>"
-- Allows filtering memory by group for post-run analysis.

-- Example query: find all knowledge created during a specific group run
SELECT * FROM memory_items
WHERE json_each.value = 'group:' || ?  -- group_id parameter
  AND json_each.value IN (SELECT value FROM json_each(tags))
  AND tenant_id = ?;
```

### B.2 Feed System

#### Feed Cursor Semantics and Exactly-Once Delivery

Polkagent uses at-least-once delivery with idempotent processing -- not
false exactly-once claims. The cursor is committed in the same SQLite
transaction as the action created by the trigger. This gives the following
guarantees:

| Scenario | Behavior |
|---|---|
| Normal processing | Cursor advances; action created atomically |
| Crash after cursor commit | No duplicate: cursor is past the event; action exists |
| Crash before cursor commit | No missed event: cursor is before the event; event reprocessed; dedup key prevents duplicate action |
| Duplicate event delivery | Dedup key matches existing action; no second action created |
| Database transaction rollback | Cursor did not advance; event will be reprocessed |

The dedup key is derived deterministically from `(trigger_id, event_identity)`
where `event_identity` is the feed-source-specific identifier (block number +
event index for chain events; sequence number for webhooks/transports):

```rust
fn compute_dedup_key(trigger_id: &TriggerId, event: &FeedEvent) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(trigger_id.0.as_bytes());
    hasher.update(event.identity_bytes());
    hex::encode(hasher.finalize().as_bytes())
}
```

#### Feed Rate Limiting and Budget Exhaustion

Rate limiting operates at three levels:

```
Level 1: Feed-level rate limit
  ── Max events consumed per second from source (prevents source overload)

Level 2: Trigger-level rate limit
  ── max_per_window triggers per window_seconds (prevents run explosion)
  ── cooldown between trigger fires for the same logical event
  ── max_pending: if trigger fire backlog exceeds this, feed pauses

Level 3: Budget exhaustion
  ── trigger_grant.window_budget: max spend across all triggered runs per window
  ── trigger_grant.per_run_budget: max spend per individual triggered run
  ── when window_budget exhausted: feed pauses until next window
```

When budget is exhausted, the feed enters `FeedState::Paused` with a
`resume_at` timestamp equal to the start of the next budget window. Events
that arrived during the pause are processed when the feed resumes (cursor
did not advance during pause).

```rust
async fn process_feed_event(
    event: FeedEvent,
    trigger: &TriggerBinding,
    budget_tracker: &BudgetTracker,
    store: &dyn FeedStore,
) -> Result<TriggerFireResult, FeedError> {
    // Check rate limit
    if !trigger.rate_limit.check(&event, store).await? {
        return Ok(TriggerFireResult::RateLimited);
    }

    // Check window budget
    if !budget_tracker.check_window_budget(&trigger.trigger_grant).await? {
        // Pause the feed until next window
        store.pause_feed(&trigger.feed_id, budget_tracker.next_window_start()).await?;
        return Ok(TriggerFireResult::BudgetExhausted);
    }

    // Evaluate conditions (deterministic, no model inference)
    if !evaluate_conditions(&trigger.conditions, &event) {
        return Ok(TriggerFireResult::ConditionNotMet);
    }

    // Dedup check
    let dedup_key = compute_dedup_key(&trigger.id, &event);
    if store.dedup_exists(&trigger.id, &dedup_key).await? {
        return Ok(TriggerFireResult::Deduplicated);
    }

    // Execute action and advance cursor in a single transaction
    store.atomic_fire_and_advance(trigger, &event, &dedup_key).await
}
```

#### Trigger Evaluation Engine

Trigger conditions are evaluated in a pure, deterministic function with no
I/O and no model inference. The evaluation engine is a recursive descent over
`TriggerCondition` trees:

```rust
pub fn evaluate_conditions(conditions: &[TriggerCondition], event: &FeedEvent) -> bool {
    conditions.iter().all(|c| evaluate_condition(c, event))
}

fn evaluate_condition(condition: &TriggerCondition, event: &FeedEvent) -> bool {
    match condition {
        TriggerCondition::EventMatch { pattern } => {
            pattern.matches(event)
        }
        TriggerCondition::Threshold { field, op, value } => {
            let field_value = event.get_field(field);
            compare(field_value, op, value)
        }
        TriggerCondition::All(conditions) => {
            conditions.iter().all(|c| evaluate_condition(c, event))
        }
        TriggerCondition::Any(conditions) => {
            conditions.iter().any(|c| evaluate_condition(c, event))
        }
        TriggerCondition::Not(inner) => {
            !evaluate_condition(inner, event)
        }
    }
}
```

Evaluation is bounded by a configurable depth limit (default: 16 nesting
levels) and a time limit (default: 10ms) to prevent pathological condition
trees from stalling the feed processing loop.

#### Recipe Composition (Feed to Trigger to Action)

A recipe packages the three layers into a single installable unit:

```
Recipe: "Alert on OpenGov Referendum Created"
  ├── Feed template:
  │     source: ChainEvents { chain: polkadot-production, filter: [ReferendaSubmitted] }
  ├── Conditions:
  │     EventMatch { pattern: { pallet: "Referenda", call: "Submitted" } }
  ├── Action:
  │     CreateRun {
  │       agent: governance-researcher,
  │       skill: governance.research_referendum,
  │       input: { referendum_id: "{{event.referendum_index}}" }
  │     }
  └── Rate limits:
        max_per_window: 20, window: 1h
        cooldown: 5min per unique referendum_id
        per_run_budget: $0.50
        window_budget: $5.00
```

Recipe installation creates the `feeds` and `trigger_bindings` rows with
the user's chain profile and budget substituted into the template. The user's
configured grant is intersected with the recipe's `required_capabilities`
list; if the intersection is empty, installation is rejected with a
capability gap error listing the missing permissions.

---

## APPENDIX C: EVALUATION FRAMEWORK

### C.1 Eval Corpus Design

#### Benchmark Categories

| Category | What it measures | Pass criteria |
|---|---|---|
| **Safety** | Constraint adherence under adversarial inputs | Binary pass/fail; any failure blocks CI |
| **Correctness** | Output accuracy against known-good reference answers | Exact match, fuzzy match, or custom comparator |
| **Quality** | Helpfulness, clarity, completeness | Dimensional scores via model-as-judge rubric |
| **Latency** | Time to first token, time to completion | Numeric thresholds per task category |
| **Cost** | Token consumption and dollar cost per task type | Numeric thresholds with regression detection |

#### Test Case Format and Fixtures

Corpus test cases are defined in TOML (for human authorship) and compiled to
the `BenchmarkCorpus` JSON format for runtime use. The TOML format mirrors
Roko's `/.roko/bench/suites/*.toml` pattern adapted for Polkagent's
chain-native task types:

```toml
# .polkagent/bench/suites/governance.toml
id = "governance-correctness-v1"
name = "Governance Research Correctness"
description = "Correctness evaluation for OpenGov referendum research tasks"
eval_type = "correctness"
chain_fixture = "polkadot-hub-block-23456789"  # pinned block state

[[tasks]]
id = "referendum-decode-001"
name = "Decode Referendum 1234 Proposal"
difficulty = 2
tags = ["governance", "decode", "correctness"]
prompt = "Decode the proposal for OpenGov referendum #1234 on Polkadot Hub and explain what it does."
expected_content = ["treasury", "spend", "1000 DOT"]  # fuzzy match
expected_citations = ["polkadot-production@block=23456789"]

[[tasks]]
id = "referendum-track-001"
name = "Identify Referendum Track"
difficulty = 1
tags = ["governance", "track", "correctness"]
prompt = "What track is OpenGov referendum #1234 on? What is the decision period?"
expected_exact = { track = "Root", decision_period_days = 28 }
```

Chain-specific corpora must include a `chain_fixture` pointing to a pinned
block state snapshot:

```toml
# .polkagent/bench/fixtures/polkadot-hub-block-23456789.toml
[fixture]
chain_profile = "polkadot-production"
block_number = 23456789
block_hash = "0xabc..."
runtime_version = 1003000
metadata_hash = "0xdef..."
storage_snapshot = ".polkagent/bench/fixtures/snapshots/polkadot-23456789.bin"
```

The storage snapshot is a Chopsticks-compatible state export that is
replayed into a local chain fork during eval execution, ensuring that
chain-querying tools see the exact state at the pinned block.

#### Automated Eval Pipeline

```
                        CI / scheduled trigger
                               │
                ┌──────────────▼──────────────┐
                │   Eval Orchestrator          │
                │   - Select corpora to run    │
                │   - Resolve eval subject     │
                │     (agent config snapshot)  │
                └──────────────┬──────────────┘
                               │
              ┌────────────────▼────────────────┐
              │ For each EvalCase in corpus:     │
              │  1. Replay chain fixture (if any)│
              │  2. Create eval-sandbox run      │
              │  3. Collect run output           │
              │  4. Score via comparator /       │
              │     model-as-judge               │
              │  5. Record EvalCaseResult        │
              └────────────────┬────────────────┘
                               │
              ┌────────────────▼────────────────┐
              │ Aggregate and compare:           │
              │  - Compute EvalAggregate scores  │
              │  - Compare to baseline           │
              │  - Flag regressions              │
              │  - Create PromotionCandidates    │
              │    for improvements > threshold  │
              └────────────────┬────────────────┘
                               │
                   ┌───────────▼───────────┐
                   │  CI Report / Review   │
                   │  Queue / Dashboard    │
                   └───────────────────────┘
```

#### Baseline Comparison and Regression Detection

A baseline is the `EvalAggregate` from the most recently promoted
configuration for a given agent/corpus pair. Regression detection compares
the new eval against the baseline:

```rust
pub struct RegressionReport {
    pub corpus_id: CorpusId,
    pub baseline_eval_id: EvalId,
    pub current_eval_id: EvalId,
    pub regressions: Vec<MetricRegression>,
    pub improvements: Vec<MetricImprovement>,
    pub severity: RegressionSeverity,
}

pub struct MetricRegression {
    pub metric: String,
    pub baseline_value: f64,
    pub current_value: f64,
    pub delta_pct: f64,
    /// Whether this regression exceeds the configured threshold and
    /// should block promotion or trigger an alert.
    pub is_blocking: bool,
}

pub enum RegressionSeverity {
    /// No regressions detected.
    Clean,
    /// Minor regressions below blocking threshold; informational only.
    Minor,
    /// One or more blocking regressions; promotion blocked.
    Blocking,
    /// Safety regression detected; CI blocked.
    SafetyFailure,
}
```

Blocking thresholds are configured per corpus:

```toml
[regression_thresholds]
safety_pass_rate = 1.0          # any regression blocks
correctness_pass_rate = -0.05   # 5% regression blocks
quality_score = -0.10           # 10% quality drop blocks
latency_p95_ms = 2000           # absolute threshold
cost_usd_per_task = 0.50        # absolute threshold
```

#### Promotion Workflow (Experimental to Stable)

Promotion follows a four-stage workflow:

```
Stage 1: Experimental (eval only)
  ├── Eval runs in sandbox
  ├── Results compared to baseline
  ├── If improvement > threshold: PromotionCandidate created (state=Pending)
  └── Appears in review queue

Stage 2: Canary (1% of traffic, production)
  ├── Operator approves experimental → canary
  ├── Limited traffic routed to candidate configuration
  ├── Live performance metrics collected (latency, cost, satisfaction)
  └── Automatic rollback if canary degrades below threshold

Stage 3: Review (operator gate)
  ├── Canary results + eval results presented together
  ├── Operator explicitly approves or rejects
  └── Approved → production

Stage 4: Production
  ├── Configuration diff applied atomically
  ├── Previous configuration archived (rollback is instant)
  └── Promotion record closed with reviewed_by and applied_at
```

---

## APPENDIX D: IMPLEMENTATION CHECKLIST

Tasks are ordered by dependency. Each task has an acceptance criterion drawn
from the acceptance criteria tables in section 13.

### D.1 SQLite Schema

- [ ] **SCHEMA-01** Create `episodes` table, indexes, and FTS5 triggers
      _Acceptance: JSONL episode writes survive crash-recovery round-trip_
- [ ] **SCHEMA-02** Create `memory_items` table and FTS5 virtual table with sync triggers
      _Acceptance: FTS query returns correct results after insert, update, delete_
- [ ] **SCHEMA-03** Load sqlite-vec extension and create `memory_vec` virtual table
      _Acceptance: ANN search returns top-K results within 100ms for 100K vectors_
- [ ] **SCHEMA-04** Create `memory_embeddings` table and embedding pipeline
      _Acceptance: New memory items generate embeddings; embedding model version is recorded_
- [ ] **SCHEMA-05** Create `memory_provenance_edges`, `memory_deletion_log`, `memory_verifications`
      _Acceptance: Deletion propagates to provenance edges; deletion log records audit entry_
- [ ] **SCHEMA-06** Create `playbooks` and `knowledge_entries` tables
      _Acceptance: Procedural entry with 3+ confirming episodes can be admitted_
- [ ] **SCHEMA-07** Create `groups`, `group_members`, `group_runs` tables
      _Acceptance: GRP-01 (grant intersection property test passes)_
- [ ] **SCHEMA-08** Create `feeds`, `trigger_bindings`, `trigger_fires`, `recipes` tables
      _Acceptance: FEED-01 (cursor survives crash/restart)_
- [ ] **SCHEMA-09** Create `benchmark_corpora`, `evals`, `eval_case_results`, `promotions` tables
      _Acceptance: EVAL-02 (results reference exact corpus version and digest)_
- [ ] **SCHEMA-10** Create `retention_sweeps` and `context_pack_inclusions` tables
      _Acceptance: MEM-09 (retention sweep removes expired items)_

### D.2 Memory Storage Layer

- [ ] **MEM-STORE-01** Implement `MemoryStore` trait with SQLite backend
      _Acceptance: MEM-01, MEM-02 (episodic creation, provenance requirement)_
- [ ] **MEM-STORE-02** Implement `MemoryAdmission` with all 6 criteria (section 3.1)
      _Acceptance: Candidates with SourceKind::Unknown are rejected_
- [ ] **MEM-STORE-03** Implement secret detection in `StandardRedactor`
      _Acceptance: CROSS-02 (secret patterns rejected in memory content)_
- [ ] **MEM-STORE-04** Implement tenant isolation at query layer
      _Acceptance: MEM-05 (no cross-tenant leakage in property test)_
- [ ] **MEM-STORE-05** Implement classification flow enforcement
      _Acceptance: MEM-06 (sensitive source yields at most sensitive memory)_
- [ ] **MEM-STORE-06** Implement deletion with full propagation (primary + FTS + vec + context packs)
      _Acceptance: MEM-07 (deletion complete across all indexes)_
- [ ] **MEM-STORE-07** Implement export/import with integrity digest
      _Acceptance: MEM-08 (export/import round-trip)_
- [ ] **MEM-STORE-08** Implement retention sweep with pinned-item exemption
      _Acceptance: MEM-09, MEM-10 (sweep removes expired, skips pinned)_

### D.3 Memory Retrieval

- [ ] **RET-01** Implement hybrid RRF retrieval (sqlite-vec + FTS5)
      _Acceptance: Semantic query returns relevant results within 200ms for 100K items_
- [ ] **RET-02** Implement context budget allocator (section A.2)
      _Acceptance: MEM-11 (context assembly respects token budget)_
- [ ] **RET-03** Implement memory item serialization for prompt injection
      _Acceptance: Context pack includes provenance blocks; stale items flagged_
- [ ] **RET-04** Implement staleness detection at retrieval time
      _Acceptance: MEM-04 (chain metadata change marks bound entries stale)_
- [ ] **RET-05** Implement context pack inclusion tracking
      _Acceptance: MEM-12 (context pack records inclusions and exclusions)_
- [ ] **RET-06** Implement provenance filter query
      _Acceptance: KNOW-02 (citations include provenance snapshot)_

### D.4 Episodes

- [ ] **EP-01** Implement `EpisodeLogger` (append-only JSONL, crash-tolerant)
      _Acceptance: Concurrent writes are serialized; partial writes discarded on read_
- [ ] **EP-02** Implement `StandardRedactor` with all redaction rules
      _Acceptance: Raw model output, secrets, and PII absent from written episodes_
- [ ] **EP-03** Implement JSONL rotation (size + age triggers)
      _Acceptance: Active log rotates on size limit; old log remains readable_
- [ ] **EP-04** Implement episode compression pipeline (zstd archive)
      _Acceptance: Compressed episodes decompress identically to originals_
- [ ] **EP-05** Implement episode replay API (section A.3)
      _Acceptance: Dry-run replay does not modify production state_
- [ ] **EP-06** Implement promotion extraction pipeline (section 3.2)
      _Acceptance: Extraction job produces candidates with source episode IDs_

### D.5 Multi-Agent Groups

- [ ] **GRP-IMPL-01** Implement `ResolvedGrant::intersect`
      _Acceptance: G3-01, GRP-01, GRP-02 (property tests pass)_
- [ ] **GRP-IMPL-02** Implement `create_group` with budget validation
      _Acceptance: GRP-03 (budget enforcement prevents overspend)_
- [ ] **GRP-IMPL-03** Implement coordinator run with child creation
      _Acceptance: GRP-07 (pipeline dependencies respected)_
- [ ] **GRP-IMPL-04** Implement cancellation propagation
      _Acceptance: GRP-04 (cancellation propagates to all children)_
- [ ] **GRP-IMPL-05** Implement evidence aggregation
      _Acceptance: GRP-06 (all child artifacts referenced in group evidence)_
- [ ] **GRP-IMPL-06** Enforce grant intersection at policy gate per tool invocation
      _Acceptance: GRP-05 (child cannot invoke tool outside group grant)_

### D.6 Feeds, Triggers, Recipes

- [ ] **FEED-IMPL-01** Implement `FeedStore` with cursor persistence
      _Acceptance: FEED-01 (cursor survives crash/restart)_
- [ ] **FEED-IMPL-02** Implement atomic cursor-action transaction
      _Acceptance: FEED-05 (transaction rollback does not advance cursor)_
- [ ] **FEED-IMPL-03** Implement trigger deduplication
      _Acceptance: FEED-03 (same event delivers only one action)_
- [ ] **FEED-IMPL-04** Implement rate limiting (feed + trigger + budget levels)
      _Acceptance: FEED-04 (burst produces at most N triggers per window)_
- [ ] **FEED-IMPL-05** Implement gap detection for chain feeds
      _Acceptance: FEED-02 (missed blocks trigger error state with gap report)_
- [ ] **FEED-IMPL-06** Implement recipe installation with capability intersection
      _Acceptance: FEED-06 (recipe installation creates feed and trigger bindings)_
- [ ] **FEED-IMPL-07** Enforce trigger grant as subset of agent grant
      _Acceptance: FEED-07 (wider trigger grant rejected at installation)_

### D.7 Evaluation Framework

- [ ] **EVAL-IMPL-01** Implement `BenchmarkCorpus` TOML loader with integrity digest
      _Acceptance: EVAL-02 (result carries corpus ID, version, digest)_
- [ ] **EVAL-IMPL-02** Implement eval sandbox isolation (separate budget, Classification::Eval)
      _Acceptance: EVAL-01 (eval does not modify production config)_
- [ ] **EVAL-IMPL-03** Implement safety eval with binary pass/fail
      _Acceptance: EVAL-03 (any safety violation produces Fail)_
- [ ] **EVAL-IMPL-04** Implement model-as-judge quality scorer
      _Acceptance: Quality scores reproducible within ±5% for same corpus version_
- [ ] **EVAL-IMPL-05** Implement regression detection and promotion candidate creation
      _Acceptance: EVAL-05 (promotion candidate is Pending until explicit approval)_
- [ ] **EVAL-IMPL-06** Implement promotion approval workflow with rollback
      _Acceptance: EVAL-06 (rollback reverts to pre-promotion config)_
- [ ] **EVAL-IMPL-07** Validate that PromotionChange cannot express grant modifications
      _Acceptance: EVAL-04 (type system prevents grant change in promotion)_
- [ ] **EVAL-IMPL-08** Prevent cross-corpus-version comparison at API layer
      _Acceptance: EVAL-07 (different corpus versions produce error on compare)_

---

## APPENDIX E: REFERENCE FILE MAP

| Component | Roko files | Bardo files | Key patterns |
|---|---|---|---|
| Episode recording | `/Users/will/dev/nunchi/roko/roko/crates/roko-learn/src/episode_logger.rs` — `Episode`, `EpisodeLogger`, `GateVerdict`, `Usage` structs; append-only JSONL with crash-tolerant reads | — | Forward-compatible serde defaults; `parking_lot::Mutex` for concurrent append serialization; `MAX_EXTRA_BYTES` cap |
| Episode storage structure | `/Users/will/dev/nunchi/roko/roko/.roko/memory/episodes.jsonl` — live JSONL produced by roko-learn | — | JSONL one-object-per-line format; rotate by size/age |
| Memory/knowledge store | `/Users/will/dev/nunchi/roko/roko/.roko/memory/knowledge-seeds.jsonl`, `experiment-winners.json`, `cascade-router.json` | — | Separate JSONL files per memory type; JSON for structured state |
| Cascade router / model routing | `/Users/will/dev/nunchi/roko/roko/crates/roko-learn/src/cascade_router.rs`, `cascade/mod.rs`, `cascade/types.rs`, `cascade/persistence.rs` | — | Three-stage progression: Static → Confidence → UCB(LinUCB) |
| Efficiency metrics | `/Users/will/dev/nunchi/roko/roko/crates/roko-learn/src/efficiency.rs`, `.roko/memory/efficiency-summaries.jsonl` | — | `AgentEfficiencyEvent`; per-model cost/pass/latency tracking |
| Provider health | `/Users/will/dev/nunchi/roko/roko/crates/roko-learn/src/provider_health.rs` | — | Provider availability sampling; feeds routing recommendations |
| Playbooks | `/Users/will/dev/nunchi/roko/roko/crates/roko-learn/src/playbook.rs`, `playbook_rules.rs` | — | Procedural memory pattern; step-list format with applicable-context metadata |
| Bandit/exploration | `/Users/will/dev/nunchi/roko/roko/crates/roko-learn/src/bandits.rs`, `contextual_bandit.rs` | — | Thompson sampling, UCB; experimental module boundary |
| Eval suites | `/Users/will/dev/nunchi/roko/roko/.roko/bench/suites/safety.toml`, `smoke.toml`, `codegen.toml`, `performance.toml` | — | TOML task format: `id`, `prompt`, `difficulty`, `tags`; maps to `EvalCase` |
| Knowledge browser TUI | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/views/context_view.rs` (sub_tab=3: `render_knowledge_browse`) | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/mind/knowledge.rs` — `KnowledgeScreen`, `KnowledgeScreenState`, `GrimoireStats`, entry type labels/colors | Bardo: hierarchical list with type badge (`INS`, `HEU`, `WRN`, `CAU`, `STR`, `ANT`), confidence score, scroll offset; Roko: sub-tab inside context view |
| Learning dashboard TUI | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/views/learning_view.rs` — `render_router`, `render_history`, `render_efficiency`; cascade stage indicator, per-model stats table, selection bar chart | — | Three sub-views (Router/History/Efficiency); sparkline via Unicode block chars `▁▂▃▄▅▆▇█`; pass rate color coding (green ≥80%, yellow ≥50%, red <50%) |
| Context/attention view TUI | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/views/context_view.rs` — four-section layout: health summary, token burn by role, cost by model, cascade router + alerts; sub_tab=1 (engram DAG), sub_tab=2 (episode replay) | — | C-Factor composite score; token burn table aggregated by role; cache hit % display |
| Cognition/reasoning TUI | — | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/mind/cognition.rs` — `HeartbeatFsmStage` (10-step FSM: Idle→Observe→Probe→Retrieve→Deliberate→Decide→Commit→Execute→Evaluate→Reflect), `BehavioralPhase`, `CognitiveTier`, PAD signal history | FSM stage pipeline visualization; PAD (Pleasure/Arousal/Dominance) sparklines; behavioral phase color coding |

---

## APPENDIX F: TUI SURFACE FOR MEMORY/KNOWLEDGE

All TUI surfaces are built with `ratatui`. Navigation follows the existing
Polkagent tab/sub-view model. Focused panels use `Theme::focused_border_style()`
(bright border) to indicate keyboard focus.

### F.1 Knowledge Browser

The knowledge browser provides a searchable, hierarchical view of all memory
items for the current agent/workspace. It is modeled on Bardo's
`KnowledgeScreen` (`/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/mind/knowledge.rs`)
with additional Polkagent-specific columns for chain binding and staleness.

```
┌─ Knowledge Browser ──────────────────────────────────────────────────────┐
│ Filter: [governance____________] Category: [ALL▼] State: [Active▼]       │
│ Chain: [polkadot-production____]            Sort: [Relevance▼]           │
├──────────────────────────────────────────────────────────────────────────┤
│ Stats ──────────────────────────────────────────────────────────────────  │
│ Total: 1,247   Semantic: 891   Procedural: 89   Episodic: 267           │
│ Avg conf: 0.76   Stale: 14   Expiring <7d: 3   Health: 88.9%           │
├──────────────────────────────────────────────────────────────────────────┤
│ TYPE  CONF  CHAIN          AGE       CONTENT                             │
├──────────────────────────────────────────────────────────────────────────┤
│▶ FAC  0.99  polkadot@v1003  3d ago   Polkadot Hub runtime uses metadata │
│             ░░░░░░░░░░░░░             version 15 as of block 23,456,789  │
│  HEU  0.82  —               12d ago  User prefers Opus for code review  │
│  STR  0.91  polkadot@v1003  8d ago   Deploy workflow: build→test→stage… │
│  FAC  0.71  kusama@v1002    21d ago  [STALE] Transfer fee on Kusama was…│
│  CAU  0.65  —               5d ago   Chopsticks fork required for storag│
│  INS  0.88  —               1d ago   Repo uses polkadot-stable2409 tool  │
│  WRN  0.45  polkadot@v1003  7d ago   pallet_balances::transfer_allow_de  │
├──────────────────────────────────────────────────────────────────────────┤
│ [Enter] Detail  [d] Delete  [p] Pin/Unpin  [e] Export  [r] Refresh      │
│ [Tab] Next pane  [/] Search  [c] Clear filter  [↑↓] Navigate            │
└──────────────────────────────────────────────────────────────────────────┘

  Detail pane (opens inline on Enter):
  ┌─ Memory Detail: FAC/0.99 ────────────────────────────────────────────┐
  │ ID: 01HX9KQZP5G3T7N2B8VJRM4WF1                                       │
  │ Content: Polkadot Hub runtime uses metadata version 15 as of          │
  │          block 23,456,789.                                             │
  │ Chain: polkadot-production  Runtime: 1003000  Block: 23,456,789       │
  │ Source: Polkadot Hub metadata at block 23456789                        │
  │         (observed 2026-07-15T10:30:00Z via ExternalRetrieval)          │
  │ Evidence: [run:01HX9KQZP5...] [artifact:metadata-snapshot-abc123]     │
  │ Verified: 2026-07-15T10:30:00Z (ChainStateCheck: Confirmed)           │
  │ Created: 2026-07-15  Accessed: 47 times  Expires: never               │
  │ Tags: [polkadot] [runtime] [metadata] [chain-state]                   │
  │ [d] Delete  [m] Mark incorrect  [v] Verify now  [Esc] Close           │
  └──────────────────────────────────────────────────────────────────────┘
```

**Implementation notes:**
- Entry type labels follow Bardo's badge pattern: `FAC` (fact), `HEU`
  (heuristic/pattern), `STR` (strategy/procedural), `CAU` (causal/chain-fact),
  `INS` (insight/general), `WRN` (warning).
- Color coding follows Bardo: Cyan=FAC, Green=HEU, Magenta=STR, Yellow=CAU,
  Cyan=INS, Red=WRN.
- Stale entries render with a dimmed style and `[STALE]` prefix.
- Health percentage uses the same threshold as Bardo's `KnowledgeScreen`:
  green ≥70%, yellow ≥40%, red <40%.
- Scroll uses `KnowledgeScreenState.scroll_offset` + `selected_insight`
  pattern from Bardo.

### F.2 Learning Dashboard

The learning dashboard visualizes model routing performance and efficiency
metrics. It mirrors Roko's `learning_view.rs` with Polkagent-specific
adaptations for skill-level routing (not just model-level).

```
┌─ Learning Dashboard ─────────────────────────────────────────────────────┐
│ [1] Route  [2] History  [3] Efficiency            F10 to cycle sub-views │
├──────────────────────────────────────────────────────────────────────────┤
│ Sub-view 1: Cascade Route                                                 │
│                                                                           │
│ ┌─ Cascade Stage ──────────────────────────────────────────────────────┐ │
│ │  Stage: UCB (LinUCB)                                                  │ │
│ │  Observations: 312 (fully adaptive)                                   │ │
│ │  Models: 4                                                             │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
│                                                                           │
│ ┌─ Per-Model Stats ─────────────────────────────────────────────────────┐ │
│ │ Model           Trials  Successes  Pass Rate  Sparkline               │ │
│ │ claude-opus-4   89      81         91.0%      ▇▇▆▇▇▇▇▇▇█████████    │ │
│ │ claude-sonnet   156     138        88.5%      ▅▆▇▇▇▇▆▇▇▇▇▇████████  │ │
│ │ claude-haiku    52      35         67.3%      ▃▄▃▄▅▄▅▄▃▄▅▅▄▅▅▄▅▄▄▃  │ │
│ │ local-llama     15      6          40.0%      ▂▁▂▁▂▂▁▁▂▂▁▁▁▂▁▁▁▁▁▂  │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
│                                                                           │
│ ┌─ Selection Frequency ─────────────────────────────────────────────────┐ │
│ │    ████                                                                │ │
│ │    ████     ██████                                                     │ │
│ │    ████     ██████    ████                                             │ │
│ │    ████     ██████    ████    ████                                     │ │
│ │  sonnet-4  haiku-4   opus-4  llama                                    │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
│                                                                           │
│ Sub-view 2: Stage History                                                 │
│                                                                           │
│  ▶ UCB (LinUCB)      (30+ obs)  ████████████████████████████████ (cyan) │
│    Confidence        (10-29)    ████████████ (yellow)                    │
│    Static            (0-9)      ████ (yellow)                            │
│                                                                           │
│ Sub-view 3: Efficiency by Model                                           │
│                                                                           │
│ ┌─ Model Efficiency Stats ──────────────────────────────────────────────┐ │
│ │ Model           Events  Passed  Pass%    Avg Cost  Avg Latency        │ │
│ │ claude-sonnet   156     138     88.5%    $0.0023   2,340ms            │ │
│ │ claude-opus-4   89      81      91.0%    $0.0089   4,120ms            │ │
│ │ claude-haiku    52      35      67.3%    $0.0004   890ms              │ │
│ │ local-llama     15      6       40.0%    $0.0000   5,230ms            │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
│                                                                           │
│ ┌─ Avg Cost (×10⁻⁴ $) ─────────────────────────────────────────────────┐ │
│ │       ████                                                             │ │
│ │  ████ ████                                                             │ │
│ │  ████ ████  ████                                                       │ │
│ │  ████ ████  ████  ████                                                 │ │
│ │ sonnet opus haiku llama                                                │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────┘
```

**Implementation notes:**
- Directly mirrors Roko's `learning_view.rs` sub-view structure:
  `SubView::LearningRouter`, `SubView::LearningHistory`,
  `SubView::LearningEfficiency`.
- Sparklines use the same Unicode block char array from Roko:
  `['▁','▂','▃','▄','▅','▆','▇','█']` with a 5-observation rolling window.
- Pass rate color thresholds: green ≥80%, yellow ≥50%, red <50% (identical
  to Roko's `render_model_table` logic).
- Bar charts use Roko's `BarChart` + `BarGroup` pattern with
  `bar_width = min(12, max(3, (area_width - 4) / bar_count))`.
- Polkagent extension: add a "Skill" column to per-model stats for
  skill-level routing breakdowns (not present in Roko).

### F.3 Context/Attention View

The context view shows the current run's token budget allocation, cost
breakdown, and memory attention -- which memory items were selected for
context and why. It extends Roko's `context_view.rs` four-section layout.

```
┌─ Context & Attention ────────────────────────────────────────────────────┐
│ [0] Budget  [1] Memory Map  [2] Episode Replay  [3] Knowledge Browse     │
├──────────────────────────────────────────────────────────────────────────┤
│ Sub-view 0: Token Budget (default)                                        │
│                                                                           │
│ ┌─ System Health ──────────────────────────────┬───────────────────────┐ │
│ │ input tokens:  42,130    pass rate:   88.2%  │ C-Factor:  0.847      │ │
│ │ output tokens:  6,891    events:      312    │   gate pass:  0.88    │ │
│ │ total cost:    $0.0234   agents:      3      │   cost eff:   0.92    │ │
│ │ avg wall time: 2,340ms   plans:       7      │   first try:  0.82    │ │
│ └──────────────────────────────────────────────┴───────────────────────┘ │
│                                                                           │
│ ┌─ Token Burn by Role ─────────────────┬─ Cost by Model ───────────────┐ │
│ │ role       tokens   cost  turns cache │ model     cost    in    out   │ │
│ │ system      8,421  $0.004  312   62%  │ sonnet  $0.0156  32k   5.2k  │ │
│ │ user        5,230  $0.003  156   45%  │ opus    $0.0068  8.2k  1.6k  │ │
│ │ assistant   6,891  $0.015  312   —    │ haiku   $0.0010  1.9k  0.1k  │ │
│ │ memory      3,840  $0.002  178   —    │                               │ │
│ │ TOTAL      24,382  $0.024  956   58%  │                               │ │
│ └──────────────────────────────────────┴───────────────────────────────┘ │
│                                                                           │
│ ┌─ Memory Attention ───────────────────┬─ Budget Allocation ───────────┐ │
│ │ Items selected: 6 / 42 candidates    │ [████████████░░░░░] 62% used  │ │
│ │                                       │ Budget: 8,192 tokens          │ │
│ │ #1 FAC/0.99  polkadot@v1003  912t   │ Used:   5,072 tokens          │ │
│ │    "Polkadot Hub runtime metad…"     │ Free:   3,120 tokens          │ │
│ │ #2 STR/0.91  —              448t    │                                │ │
│ │    "Deploy workflow: build→te…"      │ Breakdown:                    │ │
│ │ #3 HEU/0.82  —              223t    │   Procedural:  1,341t  (26%)  │ │
│ │    "User prefers Opus for code…"     │   Semantic:    3,731t  (74%)  │ │
│ │ #4 INS/0.88  —              198t    │   Episodic:       0t   ( 0%)  │ │
│ │ #5 WRN/0.45  polkadot@v1003  112t   │                                │ │
│ │ #6 CAU/0.65  —               89t    │ Excluded: 36 items             │ │
│ │ [Tab] Browse excluded items           │   Stale: 14  Budget: 22       │ │
│ └──────────────────────────────────────┴───────────────────────────────┘ │
│                                                                           │
│ Sub-view 2: Episode Replay (sub_tab=2 from context_view.rs)               │
│                                                                           │
│ ┌─ Episode Replay ──────────────────────────────────────────────────────┐ │
│ │ Agent: polkagent-governance-researcher   Run: 01HX9KQZP5...           │ │
│ │ Task: Research OpenGov referendum #1234 on Polkadot Hub               │ │
│ │ Outcome: success  Cost: $0.0023  Duration: 11.4s                      │ │
│ │                                                                        │ │
│ │ Timeline:                                                              │ │
│ │  00:00  Agent started                                                  │ │
│ │  00:02  Tool: chain.query_storage [success]                           │ │
│ │  00:05  Tool: chain.get_referendum [success]                          │ │
│ │  00:09  Gate: citation_present [pass]                                 │ │
│ │  00:09  Gate: no_hallucination_detected [pass]                        │ │
│ │  00:11  Episode recorded → artifact:01HX9KQZP5...WF3                 │ │
│ │                                                                        │ │
│ │ Memory used: 3 items (FAC/0.99, STR/0.91, INS/0.88)                  │ │
│ │ [r] Replay dry-run  [e] Export  [→] Next  [←] Prev  [Esc] Back       │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────┘
```

**Implementation notes:**
- Sub-view 0 (Budget) extends Roko's `render_with_context_data` four-section
  layout from `context_view.rs`. The top section retains C-Factor display.
- Token Burn by Role and Cost by Model tables use the exact Roko column
  layout (`role/tokens/cost/turns/cache` and `model/cost/in/out/avg`).
- Memory Attention panel is Polkagent-specific: replaces Roko's cascade
  router section with a ranked list of memory items selected for context.
- Budget allocation bar uses `█` characters at 1/10 granularity.
- Sub-view 2 (Episode Replay) corresponds to Roko's `sub_tab=2`
  (`render_episode_replay`) in context_view.rs.
- Sub-view 3 (Knowledge Browse) corresponds to Roko's `sub_tab=3`
  (`render_knowledge_browse`) in context_view.rs.

### F.4 Cognition Display

The cognition display visualizes the agent's current reasoning pipeline stage,
behavioral phase, and cognitive tier. It is modeled on Bardo's `CognitionScreen`
(`/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/mind/cognition.rs`).

```
┌─ Cognition ──────────────────────────────────────────────────────────────┐
│                                                                           │
│ ┌─ Reasoning Pipeline ─────────────────────────────────────────────────┐ │
│ │                                                                       │ │
│ │  ○ Idle  ○ Observe  ○ Probe  ▶ Retrieve  ○ Deliberate  ○ Decide     │ │
│ │  ○ Commit  ○ Execute  ○ Evaluate  ○ Reflect                          │ │
│ │                                                                       │ │
│ │  Current stage: RETRIEVE    Duration: 2.3s                           │ │
│ │  Memory query: semantic search + FTS5 hybrid RRF                     │ │
│ │  Candidates: 42 found, 6 selected (budget: 8,192 tokens)             │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
│                                                                           │
│ ┌─ Behavioral Phase ────────────────┬─ Cognitive Tier ─────────────────┐ │
│ │                                   │                                   │ │
│ │  Phase: THRIVING                  │  Tier: T1 (Standard)             │ │
│ │  ████████████████████████ 100%    │  ○ T0 (fast/cheap)               │ │
│ │                                   │  ▶ T1 (balanced)                 │ │
│ │  Budget remaining: $4.77 / $5.00  │  ○ T2 (deep/expensive)           │ │
│ │  Runs completed:   14 / unlimited │                                   │ │
│ └───────────────────────────────────┴───────────────────────────────────┘ │
│                                                                           │
│ ┌─ Signal History (PAD) ────────────────────────────────────────────────┐ │
│ │ Engagement   ▂▃▄▅▆▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇█ 0.91         │ │
│ │ Confidence   ▅▅▅▄▅▅▅▆▆▆▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇ 0.87         │ │
│ │ Fatigue      ▁▁▁▂▁▁▁▁▂▁▁▂▁▁▁▂▁▂▂▂▂▂▂▂▂▃▃▃▃▃▃▃▃▃▃▃▃▄▄▄▄ 0.22        │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────────┘
```

**Implementation notes:**
- Heartbeat FSM stages are drawn from Bardo's `HeartbeatFsmStage::ALL` array:
  `[Idle, Observe, Probe, Retrieve, Deliberate, Decide, Commit, Execute, Evaluate, Reflect]`.
- Active stage is indicated with `▶`; completed stages with `●`; pending with `○`.
- Behavioral phase color follows Bardo's `phase_color()`:
  Thriving=Green, Stable=Cyan, Conservation=Yellow, Declining=Orange, Terminal=Red.
- Cognitive tier color follows Bardo's `tier_color()`:
  T0=DarkGray, T1=Cyan, T2=Magenta.
- PAD sparklines use the same `SIGNAL_HISTORY_CAP=64` rolling window as Bardo.
- For Polkagent: "Engagement" maps to budget utilization momentum;
  "Confidence" maps to recent gate pass rate; "Fatigue" maps to cost
  consumption rate relative to budget. These are informational only and
  are never used in policy decisions (section 10.1).
- The Cognition display is only visible when the `affect_vitality` experimental
  module is enabled in configuration; otherwise the panel shows
  `[Experimental module disabled]`.

### F.5 Episode Replay Viewer

The episode replay viewer is an interactive view for browsing and replaying
historical episodes. It is accessible as sub-tab 2 of the Context view
(mirroring Roko's `render_episode_replay` at `sub_tab=2`).

```
┌─ Episode Replay ─────────────────────────────────────────────────────────┐
│ Agent: polkagent-governance-researcher   Filter: [last 7 days___________] │
├──────────────────────────────────────────────────────────────────────────┤
│ Episodes (24 matching filter)                                             │
│                                                                           │
│  ▶ 2026-07-30 14:22  success  Research referendum #1234   $0.002  11.4s  │
│    2026-07-30 11:05  success  Decode extrinsic 0xabc123   $0.001   4.2s  │
│    2026-07-29 16:41  failure  Research referendum #1198   $0.004  22.1s  │
│    2026-07-29 09:12  success  Summarize governance brief  $0.003  15.8s  │
│    2026-07-28 14:30  success  Chain state query (DOT)     $0.001   3.1s  │
│                                                                           │
├──────────────────────────────────────────────────────────────────────────┤
│ Selected: 2026-07-30 14:22 — Research referendum #1234                   │
│                                                                           │
│ Run: 01HX9KQZP5G3T7N2B8VJRM4WF2   Outcome: success                      │
│ Tokens: 9,624 in / 1,203 out   Cache hit: 63%   Cost: $0.0023            │
│                                                                           │
│ Gate verdicts:                                                            │
│   ✓ citation_present          ✓ no_hallucination_detected                │
│                                                                           │
│ Chain context: polkadot-production @ runtime 1003000                     │
│ Artifacts produced: [governance-brief-01HX9KQZP5...WF3]                 │
│                                                                           │
│ [Enter] Full detail  [x] Dry-run extraction  [r] Replay context          │
│ [↑↓] Navigate  [/] Filter  [Esc] Back                                    │
└──────────────────────────────────────────────────────────────────────────┘
```

### F.6 Eval Results Dashboard

The eval results dashboard displays benchmark scores, regression detection,
and promotion candidates.

```
┌─ Eval Results ───────────────────────────────────────────────────────────┐
│ Corpus: governance-correctness-v1  Agent: governance-researcher          │
│ Last eval: 2026-07-30 08:00  Baseline: 2026-07-25 eval                  │
├──────────────────────────────────────────────────────────────────────────┤
│ ┌─ Aggregate Scores ────────────────────────────────────────────────────┐ │
│ │ Category      Current  Baseline  Delta   Status                       │ │
│ │ Safety          100%     100%     +0%    ✓ No regression              │ │
│ │ Correctness     87.3%    82.1%   +5.2%   ↑ Improvement               │ │
│ │ Quality/clarity  4.2/5    3.9/5  +0.3    ↑ Improvement               │ │
│ │ Latency P95    2,340ms  2,890ms  -550ms  ↑ Improvement               │ │
│ │ Cost/task      $0.0023  $0.0031  -$0.0008 ↑ Improvement              │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
│                                                                           │
│ ┌─ Promotion Candidates ────────────────────────────────────────────────┐ │
│ │ ID          Change              Improvement  State                     │ │
│ │ PROM-001    route: sonnet→opus  +5.2% corr  ● Pending review         │ │
│ │ PROM-002    prompt v3→v4        +0.3 quality ● Pending review         │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
│                                                                           │
│ ┌─ Case Results (34 cases) ─────────────────────────────────────────────┐ │
│ │ #   ID                      Outcome  Score  Cost    Duration          │ │
│ │  1  referendum-decode-001   ✓ Pass   0.95   $0.002  3.2s             │ │
│ │  2  referendum-track-001    ✓ Pass   1.00   $0.001  1.1s             │ │
│ │  3  governance-brief-001    ✓ Pass   0.88   $0.004  8.4s             │ │
│ │  4  safety-injection-001    ✓ Pass   1.00   $0.001  0.9s             │ │
│ │  5  chain-state-stale-001   ✗ Fail   0.00   $0.002  4.1s  STALE     │ │
│ └──────────────────────────────────────────────────────────────────────┘ │
│                                                                           │
│ [Enter] Case detail  [a] Approve promotion  [r] Reject  [e] Export      │
│ [↑↓] Navigate cases  [Tab] Switch section  [Esc] Back                   │
└──────────────────────────────────────────────────────────────────────────┘
```

**Implementation notes:**
- Safety score is always displayed first and uses binary 100%/0% display.
  Any safety regression renders in red and adds a blocking badge.
- Delta column: improvements in green with `↑`, regressions in red with `↓`,
  unchanged in muted with `=`.
- Promotion candidates show state using: `● Pending`, `✓ Approved`,
  `✗ Rejected`, `↩ Rolled Back`.
- Case results table is scrollable; failed cases are highlighted in red.

---

## APPENDIX G: CONFIGURATION GUIDE

### G.1 Memory Storage Settings

Memory configuration lives in `.polkagent/config.toml` at the workspace
root. All paths are relative to the `.polkagent/` directory unless absolute.

```toml
[memory]
# Path to the SQLite authority database (relative to .polkagent/).
db_path = "polkagent.db"

# Embedding model for semantic search vectors.
# Must match a model available via the configured provider.
embedding_model = "text-embedding-3-small"
embedding_dims = 1536

# sqlite-vec quantization mode: "f32" (default), "int8", or "1bit".
# int8 reduces storage 4x; 1bit reduces 32x with some recall loss.
embedding_quant = "f32"

# Context window budget: fraction of remaining tokens allocated to memory.
context_memory_pct = 0.20
context_min_tokens = 256

# Admission policy for new memory items.
# Options: "review_all" | "review_inferred" | "auto_admit_verified" | "disabled"
admission_policy = "review_all"

# Retention defaults (can be overridden per-agent or per-workspace).
[memory.retention]
episodic_stale_after_days = 90
episodic_expires_after_days = 365
semantic_stale_after_days = 180   # 0 = no time-based staleness
procedural_stale_after_days = 0   # procedures expire only on repeated failure

# JSONL episode log settings.
[memory.episodes]
log_path = "episodes.jsonl"
archive_dir = "episodes-archive"
max_file_size_mb = 64
max_age_days = 7
compress_after_days = 30
compression_level = 3  # zstd level 1-22; 3 is a good balance

# Retention sweep schedule (cron expression, UTC).
[memory.sweep]
schedule = "0 3 * * *"  # daily at 03:00 UTC
grace_period_hours = 24  # time between PendingDeletion and actual purge

# Secret detection patterns (extend the built-in list).
[memory.redaction]
extra_patterns = []  # list of regex strings for project-specific secrets
```

### G.2 Feed Configuration

```toml
# Feed definitions live in .polkagent/feeds.toml
# Each [[feeds]] entry creates one Feed row in the database.

[[feeds]]
id = "polkadot-governance-feed"
name = "Polkadot OpenGov Events"
source_type = "chain_events"
chain_profile = "polkadot-production"
event_filters = [
    { pallet = "Referenda", event = "Submitted" },
    { pallet = "Referenda", event = "DecisionStarted" },
    { pallet = "Referenda", event = "Confirmed" },
]

[[feeds.triggers]]
id = "new-referendum-trigger"
name = "New Referendum Alert"
conditions = [
    { type = "event_match", pallet = "Referenda", event = "Submitted" }
]
action = { type = "create_run", agent = "governance-researcher",
           skill = "governance.research_referendum",
           input_template = { referendum_id = "{{event.referendum_index}}" } }
rate_limit = { max_per_window = 20, window_seconds = 3600,
               cooldown_seconds = 300, max_pending = 100 }
per_run_budget = "0.50"
window_budget = "5.00"

[[feeds]]
id = "daily-digest-feed"
name = "Daily Summary Schedule"
source_type = "schedule"
cron = "0 9 * * 1-5"  # weekdays at 09:00
timezone = "UTC"

[[feeds.triggers]]
id = "daily-digest-trigger"
name = "Produce Daily Digest"
conditions = []  # always fires on schedule
action = { type = "create_run", agent = "governance-researcher",
           skill = "governance.daily_digest" }
rate_limit = { max_per_window = 1, window_seconds = 86400 }
per_run_budget = "1.00"
```

### G.3 Eval Schedule and Corpus Location

```toml
[evals]
# Directory containing corpus TOML files.
corpus_dir = ".polkagent/bench/suites"

# Directory containing chain fixture snapshots.
fixture_dir = ".polkagent/bench/fixtures"

# Eval run output directory.
results_dir = ".polkagent/bench/runs"

# Scheduled eval runs (cron, UTC).
[[evals.scheduled]]
corpus = "governance-correctness-v1"
agent = "governance-researcher"
schedule = "0 4 * * *"  # daily at 04:00 UTC
notify_on_regression = true
promotion_threshold = 0.05  # 5% improvement triggers a PromotionCandidate

[[evals.scheduled]]
corpus = "safety-v1"
agent = "*"  # run against all agents
schedule = "0 2 * * 0"  # weekly Sunday at 02:00 UTC
notify_on_regression = true
# Safety evals never auto-promote; all results require review.

# Promotion review settings.
[evals.promotion]
# Whether improvements above threshold can be automatically staged to canary
# without human review. Disabled by default (see EVAL-SAFE-06).
auto_canary = false
canary_traffic_pct = 1  # percentage of traffic routed to canary (if enabled)
canary_duration_hours = 24
```

### G.4 Group Membership Configuration

```toml
# Group templates live in .polkagent/groups.toml.
# Groups are instantiated at runtime from these templates.

[[groups]]
id = "governance-research-group"
name = "Governance Research Team"
description = "Multi-agent group for comprehensive OpenGov research"
coordination = "pipeline"

[[groups.members]]
agent = "governance-researcher"
role = "coordinator"

[[groups.members]]
agent = "chain-decoder"
role = "worker"

[[groups.members]]
agent = "external-context"
role = "worker"

[groups.budget]
max_model_cost = "5.00"
max_tool_invocations = 200
max_duration_secs = 300
max_concurrent_children = 3
max_total_children = 10

# Pipeline dependencies (worker must complete before coordinator synthesizes)
[groups.pipeline]
dependencies = [
    { from = "chain-decoder", to = "governance-researcher" },
    { from = "external-context", to = "governance-researcher" },
]
```

### G.5 .polkagent/ Directory Structure

The `.polkagent/` directory is the local memory and configuration root for a
Polkagent installation. It mirrors the `.roko/` structure from
`/Users/will/dev/nunchi/roko/roko/.roko/` adapted for Polkagent's
SQLite-first architecture:

```
.polkagent/
├── config.toml                  # Main configuration (section G.1)
├── feeds.toml                   # Feed and trigger definitions (section G.2)
├── groups.toml                  # Group templates (section G.4)
│
├── polkagent.db                 # SQLite authority database
│   ├── episodes                 # Episodic memory table
│   ├── memory_items             # Semantic/procedural memory
│   ├── memory_fts               # FTS5 virtual table (auto-synced)
│   ├── memory_vec               # sqlite-vec ANN index (auto-synced)
│   ├── memory_embeddings        # Embedding blobs + metadata
│   ├── memory_provenance_edges  # Knowledge graph edges
│   ├── memory_deletion_log      # Deletion audit (content-free)
│   ├── memory_verifications     # Verification history
│   ├── playbooks                # Procedural memory step lists
│   ├── knowledge_entries        # Semantic knowledge with chain binding
│   ├── groups                   # Multi-agent group definitions
│   ├── group_members            # Group membership + individual grants
│   ├── group_runs               # Child run tracking
│   ├── feeds                    # Feed definitions + cursors
│   ├── trigger_bindings         # Trigger conditions + actions
│   ├── trigger_fires            # Dedup log + audit
│   ├── recipes                  # Recipe templates
│   ├── benchmark_corpora        # Eval corpus records
│   ├── evals                    # Evaluation run records
│   ├── eval_case_results        # Per-case results
│   ├── promotions               # Promotion candidates + states
│   ├── retention_sweeps         # Sweep history
│   └── context_pack_inclusions  # Memory attention tracking
│
├── episodes.jsonl               # Active episode log (append-only JSONL)
│
├── episodes-archive/            # Compressed historical episodes
│   ├── 2026-06/
│   │   └── episodes.jsonl.zst
│   └── 2026-07/
│       └── episodes.jsonl.zst
│
├── bench/
│   ├── suites/                  # Corpus TOML definitions
│   │   ├── governance.toml
│   │   ├── safety.toml
│   │   ├── correctness.toml
│   │   └── performance.toml
│   ├── fixtures/                # Chain state snapshots for evals
│   │   ├── polkadot-hub-block-23456789.toml
│   │   └── snapshots/
│   │       └── polkadot-23456789.bin  # Chopsticks-compatible state export
│   └── runs/                    # Eval run result artifacts
│       └── eval_<id>.json
│
├── sessions/
│   └── last.json                # Last session state (for TUI restore)
│
└── state/
    └── state-snapshot.json      # Agent operational state snapshot
```

The single `polkagent.db` file is the source of truth for all durable state.
The JSONL files provide append-only streaming for high-frequency episode
writes where SQLite's write amplification would be a bottleneck. Periodic
background jobs ingest completed JSONL segments into the `episodes` SQL table
for indexed retrieval and promotion pipeline use.
