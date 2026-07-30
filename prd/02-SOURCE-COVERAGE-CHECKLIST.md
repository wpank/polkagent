# Polkagent definitive PRDs — source coverage checklist

**Status:** active synthesis checklist

**Scope:** every document directly under `polkagent/tmp`, excluding
`polkagent/tmp/PRDs`

**Inventory date:** 2026-07-29

**Inventory result:** 35 readable Markdown documents, 6,353 lines, 445,599
bytes; no unreadable or non-text source files

## 1. Purpose

This checklist prevents the definitive PRD suite from losing useful details
while replacing an iterative and sometimes contradictory planning corpus.

It serves four purposes:

1. Prove that every source document was considered.
2. Preserve unique requirements from older or superseded documents.
3. Resolve contradictions using the owner’s current direction.
4. Trace final requirements to implementation contracts, UX, tests, evidence,
   or an explicit defer/reject decision.

An item is not complete merely because similar prose appears in a PRD. It is
complete only when the definitive suite gives it a clear decision, target
release or maturity, and—when normative—an acceptance artifact.

### 1.1 Polkagent in one page

Polkagent is a proposed Rust-first, Polkadot-native platform with three equal
product pillars:

1. **Build:** help people and teams create, test, deploy, and operate agents,
   applications, contracts/PVM programs, Polkadot SDK runtimes/chains, and
   emerging JAM services.
2. **Act and pay:** explain, prepare, simulate, approve, sign, submit, watch,
   and—when deliberately configured—fully automate Polkadot payments and
   on-chain actions.
3. **Reach users:** preserve and improve the mobile/private-chat, coding-agent,
   projects/files, framework bridge, and deployment experience of
   `polkadot-chat-agents` (PCA) through Polkadot app/mobile, web, CLI, desktop,
   API, and embedded surfaces.

The platform must support local/self-hosted and managed multi-tenant cloud as
equal modes. Providers, models, coding harnesses, tools, skills, transports,
signers, chain clients, stores, memory systems, and user surfaces remain
independent adapters around a durable kernel. Publication and discovery use a
permissionless, plural marketplace/registry model with safe default trust
views. Custody and approval are configurable; a user may configure fully
autonomous operation, but models still do not receive raw signing keys and
durability/audit/recovery rules remain active.

This context is repeated here so a checklist reviewer does not need to read the
baseline before recognizing when an older source conflicts with current intent.

### 1.2 Why there are 35 sources

The `tmp` corpus evolved in layers:

```text
Repository reconnaissance
  00–03: map Roko, PCA, and adjacent code/workspaces
       ↓
First normative platform decomposition
  10–19: product, architecture, domain, providers, harnesses, transport,
         security, data, extensions, and delivery
       ↓
Architecture correction and implementation preparation
  20–25: effect/outbox refinements, PCA compatibility, Rust workspace,
         ADRs, conformance spikes, and concise normative invariants
       ↓
External product/ecosystem research
  30–36: Polkadot surfaces, payments, agent UX, product-engineering vision,
         landscape, technical integrations, business/marketplace operations
       ↓
Narrow first-release/evidence program
  37–40: Explain Before Sign, user validation, 90-day charter, evidence backlog
       ↓
Cross-corpus synthesis and future research
  99–102: master dossier and reusable broad/specific research prompts
```

Later documents often refine earlier implementation details, but later does not
always mean broader or more authoritative. Documents 37–40 intentionally
narrowed the first validation release. The owner subsequently confirmed a
complete long-term platform, configurable full autonomy, permissionless
marketplaces, equal cloud/self-host modes, and PCA/mobile parity. The checklist
must therefore preserve the evidence discipline of 37–40 without treating
their first-release exclusions as permanent product limits.

### 1.3 Terms used by this checklist

| Term | Meaning |
|---|---|
| **PCA** | `polkadot-chat-agents`, the reference implementation/product whose external behavior and migration paths Polkagent should preserve and improve. |
| **Roko** | A separate broad agent-platform code/design corpus used as a source of patterns, not as an API Polkagent must clone. |
| **Artifact** | Durable attributable content/evidence, such as a file, decoded action, test result, simulation, or receipt. |
| **Event** | An ordered observation of lifecycle or streaming activity. |
| **Effect / `EffectIntent` / `EffectAttempt` / `EffectOutcome`** | An effect is actual external work. `EffectIntent` is the durable command recorded before an adapter performs it. `EffectAttempt` records one claim, lease, retry, and idempotency lifecycle. `EffectOutcome` is the immutable success, failure, timeout, cancellation, or unknown result observed for one attempt. |
| **Outbox** | Transactional storage of effects/messages so state changes and eventual external delivery survive crashes. |
| **Grant / `ResolvedGrant`** | The immutable exact permissions and limits authorized for a run/effect. |
| **Provider / model** | The service connection and the particular AI model; they are separate concepts. |
| **Executor / harness** | A per-run execution adapter and a potentially long-lived coding/framework service with sessions and health; they are separate concepts. |
| **Transport** | An input/output channel such as web, API, or Polkadot chat. |
| **Signer** | An isolated component or external wallet that signs an exact canonical payload. |
| **PVM / PolkaVM** | Polkadot Virtual Machine execution and tooling. |
| **JAM** | Join-Accumulate Machine, an emerging computation/service architecture that requires explicit maturity labels. |
| **Normative** | Required behavior that implementation and acceptance tests must satisfy. |
| **Evidence gate** | A named test, spike, user study, or operational proof required before scope or risk increases. |

## 2. Checklist semantics

### 2.1 Item states

- `[ ]` — not yet demonstrably covered by the definitive suite.
- `[x]` — covered and verified against its source.
- **Accepted** — selected product or architecture behavior.
- **Default** — out-of-box behavior that remains configurable.
- **Deferred** — intentionally later, with dependency or reopening trigger.
- **Experimental** — requires a spike/evidence gate before product commitment.
- **Rejected** — deliberately excluded, with rationale.
- **Superseded** — replaced by a newer decision; unique rationale still
  preserved.

### 2.2 Requirement record required in the final traceability matrix

Every normative item should eventually have:

```yaml
id: RUNTIME-EFFECT-001
requirement: "Authoritative state change and effect enqueue are atomic."
classification: normative
priority: P0
maturity: accepted
source_documents:
  - 20-roko-design-review.md
  - 25-normative-prd-addendum.md
target_prds:
  - 03-runtime-and-execution.md
decision_or_adr: ADR-002
acceptance_artifact: crash-recovery contract test
implementation_status: not_started
evidence_level: 1
owner: unassigned
review_date: null
conflicting_sources: []
```

`Accepted` means the design decision is made. `Verified` must require a test,
prototype, user study, observed production evidence, or another named
artifact.

### 2.3 Evidence levels

| Level | Meaning |
|---|---|
| E0 | Assertion or idea only. |
| E1 | Documented by a credible source or existing code inspection. |
| E2 | Reproduced in a fixture, spike, or controlled test. |
| E3 | Observed with representative users or a live integration. |
| E4 | Sustained in production with measured reliability. |

### 2.4 How an author uses this checklist

For each definitive PRD:

1. Select its row in the destination table in section 4.
2. Read the source summaries in sections 5 and 6 for the listed inputs.
3. Copy the relevant cross-cutting items from section 7 into a working
   traceability table.
4. Resolve conflict against section 3 and the checked owner decisions in
   section 10.
5. Write the PRD so its user outcome, domain terms, dependencies, defaults,
   configuration, failure states, security model, and acceptance are
   understandable inline.
6. Mark an item covered only after recording the destination requirement ID.
7. Mark it verified only after a second pass compares the PRD against the
   source summary and, for normative behavior, names an acceptance artifact.

Example:

```text
Source obligation:
  21-pca-compatibility-prd → durable-before-ACK admission

Definitive requirement:
  PCA-INGRESS-003 → A transport may acknowledge a delivery only after a
  deduplicated accepted-message record and owed-work state are durable.

Acceptance artifact:
  Kill the runtime at each admission transaction boundary; after restart,
  prove the message is neither lost nor executed as two semantic turns.
```

## 3. Source authority and reconciliation rules

Use this order when sources conflict:

1. Explicit owner decisions, including the current
   `01-ESTABLISHED-BASELINE.md`.
2. The future definitive PRDs and newly accepted ADRs.
3. Existing normative invariants in documents 20, 21, 23, and 25 when they do
   not conflict with current owner direction.
4. Externally observable `polkadot-chat-agents` behavior for compatibility
   claims.
5. Current primary-source ecosystem evidence and implementation spikes.
6. Older PRDs, research syntheses, and dossiers.

Important reconciliation rules:

- The complete long-term platform supersedes documents that redefine Polkagent
  as only an Explain Before Sign MVP.
- Explain Before Sign remains an early chain-safety proving slice.
- Product building, chain actions/payments, and PCA/mobile user reach remain
  equally central.
- Local/self-hosted and managed multi-tenant cloud are equally first-class.
- Custody and approval are configurable with safe defaults.
- Every supported action class may be fully autonomous if explicitly
  configured.
- Marketplace publication/discovery is permissionless and plural by design,
  with safe default views and execution controls.
- Rust-first remains established; TypeScript is allowed at browser/Product SDK
  boundaries.
- A generalized graph is not required by the minimal runtime API, but advanced
  workflows, groups, feeds, triggers, memory, and learning remain committed
  platform areas.

## 4. Proposed definitive PRD destinations

The final filenames may change, but every checklist item needs one primary
home.

| ID | Definitive document area |
|---|---|
| D00 | Suite index, reading order, authority, traceability, glossary map |
| D01 | Vision, personas, jobs, principles, product pillars, scope |
| D02 | Vocabulary, invariants, architecture, crate and dependency model |
| D03 | Agent, conversation, run, event, artifact, effect, workflow execution |
| D04 | Providers, models, executors, harnesses, tools, skills, workspaces |
| D05 | Polkadot chain, SDK, contracts, PVM, JAM, and builder integrations |
| D06 | PCA compatibility, chat, transport, mobile, Polkadot App/Product |
| D07 | Identity, policy, capabilities, security, accounts, custody, signers |
| D08 | Payments, budgets, economic controls, and autonomous actions |
| D09 | Context, memory, knowledge, learning, evals, groups, feeds, triggers |
| D10 | Persistence, artifacts, events, projections, observability, operations |
| D11 | Local/self-hosted, managed cloud, tenancy, fleets, deployment |
| D12 | Extensions, registries, permissionless marketplace, product kits |
| D13 | Studio, Inbox, approval/action sheets, CLI, API, operator UX |
| D14 | Traits, APIs, schemas, configuration, import/export, migration |
| D15 | Testing, conformance, security assurance, validation, roadmap, gates |

### 4.1 What each destination must answer

| ID | Central question | Principal outputs |
|---|---|---|
| D00 | How does a reader navigate and trust the suite? | Reading paths, authority, terminology links, decision/evidence status, source-to-requirement matrix |
| D01 | What product is being built, for whom, and why? | North star, personas, jobs, product pillars, principles, outcomes, non-goals, maturity |
| D02 | What remains stable as every integration changes? | Layering, kernel, dependency rules, invariants, crate boundaries, trust boundaries |
| D03 | How does durable work start, progress, branch, fail, recover, and finish? | Aggregates, state machines, events, artifacts, effects, workflows, concurrency, cancellation |
| D04 | How does Polkagent use different AI and execution systems safely? | Provider/model routing, executor/harness lifecycles, tools, skills, browser/workspace isolation |
| D05 | How does it build and act across Polkadot products? | Chain profiles, Subxt/PAPI/Product roles, SDK/runtime/contracts/PVM/JAM workflows, action families |
| D06 | How does a PCA/mobile/chat user interact and migrate? | C0–C3 behavior, transports, files/live replies, mobile surfaces, bridge/native paths, fixtures |
| D07 | Who may do what with which account and why? | Identity, authentication, policy, grants, approvals, custody, signer isolation, threat model |
| D08 | How does value move and how can action become autonomous? | Payment/action intents, mandates, budgets, settlement, receipts, autonomy levels, emergency controls |
| D09 | What context persists and how do agents coordinate or improve? | Memory, provenance, learning/evals, groups, graphs, feeds, triggers, authority limits |
| D10 | How is truth stored, observed, repaired, retained, and deleted? | Database/object schemas, outbox, projections, telemetry, recovery, backup, lifecycle |
| D11 | How do equivalent local and managed deployments operate? | Data/control planes, tenancy, fleets, regions, secrets, SLOs, portability, billing |
| D12 | How can third parties publish and users safely compose capabilities? | Manifests, extension tiers, registries, permissionless marketplace, trust views, product kits |
| D13 | How does complexity become a top-tier user experience? | Studio, Inbox, action sheets, CLI, operator console, onboarding, error/recovery, accessibility |
| D14 | What exact contracts let implementations and clients interoperate? | Rust traits, DTOs, OpenAPI/events, config, schemas, import/export, migrations, examples |
| D15 | What evidence permits release and later expansion? | Test pyramid, conformance corpora, red team, validation, phases, release/stop/reopening gates |

## 5. Complete source inventory

The inventory is complete. The final two columns remain unchecked until the
definitive suite exists and is audited.

| Source | Inventoried | Unique contribution that must survive | Likely destinations | Absorbed | Verified |
|---|---:|---|---|---:|---:|
| `00-INDEX.md` | [x] | Authority, reading order, initial decisions/non-goals, deliberately deferred questions | D00, D01, D15 | [ ] | [ ] |
| `01-roko-recon.md` | [x] | Artifact/event split, narrow ports, graphs, middleware, capability intersection, projections, versioned config | D02, D03, D07, D09, D10 | [ ] | [ ] |
| `02-polkadot-chat-agents-recon.md` | [x] | PCA code map, durable message lifecycle, direct runners, bridges, lanes, files, strengths/gaps | D04, D06, D10, D14 | [ ] | [ ] |
| `03-workspace-ecosystem-recon.md` | [x] | Repository ownership map, protocol constraints, Rust migration cautions, test/delivery conventions | D02, D06, D14, D15 | [ ] | [ ] |
| `10-product-prd.md` | [x] | Original personas/jobs, product principles, v1 capabilities, success metrics and NFRs | D01, D13, D15 | [ ] | [ ] |
| `11-system-architecture-prd.md` | [x] | Layering, workspace/crates, ports, actor/runtime pattern, turn pipeline | D02, D03, D14 | [ ] | [ ] |
| `12-domain-execution-prd.md` | [x] | Stable IDs, aggregates, turn states, event envelope, context/memory, retry taxonomy | D03, D09, D10, D14 | [ ] | [ ] |
| `13-provider-model-prd.md` | [x] | Provider/model/executor/harness separation, catalogs, routing, adapter onboarding | D04, D14, D15 | [ ] | [ ] |
| `14-harness-tool-prd.md` | [x] | Harness contract, tools, grants, workspaces, artifacts, compatibility bridge, plugin lifecycle | D04, D07, D12, D14 | [ ] | [ ] |
| `15-transport-prd.md` | [x] | Transport trait, Polkadot adapter, ingress/egress/attachment contract, capability model, tests | D06, D14, D15 | [ ] | [ ] |
| `16-security-prd.md` | [x] | Threats, trust boundaries, policy, secrets, isolation tiers, safety gates | D07, D11, D15 | [ ] | [ ] |
| `17-data-operations-prd.md` | [x] | SQLite authority, recovery, configuration precedence, read models, observability, retention/deletion | D10, D11, D14, D15 | [ ] | [ ] |
| `18-extension-sdk-prd.md` | [x] | Stability layers, manifests, SDK/RPC contracts, compatibility, PCA plugin migration | D12, D14, D15 | [ ] | [ ] |
| `19-delivery-prd.md` | [x] | Phases, test pyramid, implementation guardrails, production hardening | D15 | [ ] | [ ] |
| `20-roko-design-review.md` | [x] | EffectIntent, chain saga, terminality split, stream rules, HarnessService, ResolvedGrant, evidence/admission | D02, D03, D04, D07, D10 | [ ] | [ ] |
| `21-pca-compatibility-prd.md` | [x] | C0/C1 semantics, ACK/dedupe/leases, egress lanes, bridge fencing, files, live replies, migration fixtures | D06, D10, D14, D15 | [ ] | [ ] |
| `22-rust-workspace-blueprint.md` | [x] | Concrete workspace layout, crate dependencies, Cargo policy, CI/MSRV, initial milestone | D02, D14, D15 | [ ] | [ ] |
| `23-architecture-decision-records.md` | [x] | Nine initial ADRs for workspace, outbox, actors, adapter taxonomy, grants, bridge, SQLite, transport proof, graphs | D00, D02, D03, D06, D07, D15 | [ ] | [ ] |
| `24-conformance-and-spike-plan.md` | [x] | Polkadot interop, stream normalization, delivery-recovery spikes and shared contract tests | D06, D10, D15 | [ ] | [ ] |
| `25-normative-prd-addendum.md` | [x] | Concise effect, terminality, grant, stream, compatibility, evidence, chain-action, admission, harness rules | D02, D03, D04, D06, D07, D10 | [ ] | [ ] |
| `30-polkadot-product-surface-research.md` | [x] | Governance Scout, Explain Before Sign, metadata monitor, App companion, JAM opportunity/maturity | D01, D05, D06, D13, D15 | [ ] | [ ] |
| `31-agent-payments-research.md` | [x] | Payment intent, checkout/settlement split, micropayments, rail negotiation, USDC/Polkadot context, compliance/tests | D07, D08, D14, D15 | [ ] | [ ] |
| `32-agent-product-paradigms-research.md` | [x] | Takopi/local UX, coding-agent progress, browser sessions, skills/context distinction, provenance | D01, D04, D09, D12, D13 | [ ] | [ ] |
| `33-product-engineering-vision-prd.md` | [x] | Studio, Inbox, product kits, payment sheet, use-case portfolio, make/publish/charge phases | D01, D08, D12, D13 | [ ] | [ ] |
| `34-polkadot-agent-landscape-research.md` | [x] | Ecosystem/competitor map, governance signal, safe action-composition gap, positioning claims to validate | D01, D05, D15 | [ ] | [ ] |
| `35-technical-integration-research.md` | [x] | Subxt-first Rust path, SDK/PAPI/Product boundaries, signer port, Hub/contracts/assets, testnets and spikes | D05, D06, D07, D14, D15 | [ ] | [ ] |
| `36-agent-business-product-research.md` | [x] | Catalog/store operations, cost envelopes, hosting economics, abuse/fraud, approval UX, business stages | D08, D11, D12, D13, D15 | [ ] | [ ] |
| `37-external-deep-research-synthesis.md` | [x] | Narrow validation wedge, evidence-seeking hypotheses, comprehension gate, decision gates | D01, D13, D15 | [ ] | [ ] |
| `38-product-validation-program.md` | [x] | Cohorts/interviews, trust prototypes, fault scenarios, comprehension testing, alpha metrics/privacy | D13, D15 | [ ] | [ ] |
| `39-report-18-mvp-execution-charter.md` | [x] | Advanced-user/action hypothesis, 90-day evidence plan, MVP metrics and stop/narrow triggers | D01, D13, D15 | [ ] | [ ] |
| `40-report-18-evidence-backlog.md` | [x] | E1–E9 evidence blockers, quality rubric, research artifact/update protocol | D00, D15 | [ ] | [ ] |
| `99-deep-research-dossier.md` | [x] | Broad integrated vision, layered architecture, compatibility, providers, security, chain, payments, roadmap | All | [ ] | [ ] |
| `100-follow-on-deep-research-dossier.md` | [x] | Fourteen research prompts and quality/cadence requirements | D00, D15 | [ ] | [ ] |
| `101-mvp-deep-research-prompt.md` | [x] | Reusable focused research request and expected decision artifacts | D00, D15 | [ ] | [ ] |
| `102-follow-on-deep-research-prompts.md` | [x] | Twenty decision-grade prompts for transport, Subxt, signers, user demand, competition, trust UX, mobile, payments, governance, XCM, storage, isolation, autonomy, safety, product kits, evidence, migrations, metadata drift, PVM and JAM | D00, D05, D06, D07, D08, D10, D12, D13, D15 | [ ] | [ ] |

## 6. Source-specific obligations

This section captures details that can disappear if synthesis relies only on
document titles or executive summaries.

### 6.1 `00-INDEX.md`

- [ ] Preserve a clear reading order and authority hierarchy.
- [ ] Reclassify old v1 non-goals as current default, phased, experimental, or
  rejected instead of copying them blindly.
- [ ] Resolve each deliberately deferred question or preserve it with a named
  evidence gate.
- [ ] Explain which old documents are descriptive, normative, implementation
  companions, or research.

### 6.2 `01-roko-recon.md`

- [ ] Define artifact versus event versus effect.
- [ ] Preserve narrow protocol traits and dependency inversion.
- [ ] Support declarative graphs without requiring them for simple runs.
- [ ] Separate model API adapters from agent/coding harness adapters.
- [ ] Specify ordered middleware/interceptor behavior and short-circuit rules.
- [ ] Resolve permissions through capability intersection/policy composition.
- [ ] Keep orchestration durable and inspectable.
- [ ] Treat observability as read-only projections, not domain mutation.
- [ ] Make configuration versioned, composable, validated, and exportable.
- [ ] Explicitly list Roko ideas not adopted wholesale.

### 6.3 `02-polkadot-chat-agents-recon.md`

- [ ] Preserve the exact external message lifecycle and durability points.
- [ ] Preserve durable-before-ACK behavior.
- [ ] Preserve per-device/session behavior, retries, dedupe, leases, and owed
  work.
- [ ] Preserve direct provider/coding-runner behavior.
- [ ] Preserve portable tool policy and project/workspace expectations.
- [ ] Preserve framework/harness bridge behavior without retaining internal
  coupling.
- [ ] Record the implementation paths used to derive compatibility fixtures.
- [ ] Turn PCA strengths and limitations into explicit successor requirements.

### 6.4 `03-workspace-ecosystem-recon.md`

- [ ] Retain a source/code ownership map for compatibility implementation.
- [ ] Treat existing protocol facts as external constraints, not preferences.
- [ ] Preserve reliability, ordering, persistence, and security findings.
- [ ] Incorporate existing test and delivery conventions where they remain
  useful.
- [ ] Address Rust-specific porting risks instead of assuming a transliteration.
- [ ] Keep unresolved ecosystem questions visible until proven.

### 6.5 `10-product-prd.md`

- [ ] Reconcile all original personas and jobs with the complete-platform
  personas.
- [ ] Preserve useful initial capabilities as product slices or product kits.
- [ ] Retain product principles that improve simplicity, inspectability, and
  ownership.
- [ ] Convert success metrics into measurable product/evidence gates.
- [ ] Preserve performance, reliability, portability, and usability NFRs.
- [ ] Resolve or assign every implementation-spike question.

### 6.6 `11-system-architecture-prd.md`

- [ ] Publish the definitive layered architecture.
- [ ] Publish a concrete Rust workspace/crate proposal.
- [ ] Define every stable port and its owner.
- [ ] Define actor/partition concurrency and backpressure.
- [ ] Define the end-to-end turn/run pipeline.
- [ ] Enforce dependency direction through CI or architecture tests.

### 6.7 `12-domain-execution-prd.md`

- [ ] Define stable identifier types and generation/serialization rules.
- [ ] Define agent, conversation, turn, run, artifact, event, and deployment
  aggregates.
- [ ] Define state machines and terminal states.
- [ ] Define event envelope, ordering, correlation, causation, and durability.
- [ ] Define bounded context and memory semantics.
- [ ] Define retryable, terminal, policy, user, provider, transport, and unknown
  errors.

### 6.8 `13-provider-model-prd.md`

- [ ] Keep provider, connection, model, route, executor, and harness distinct.
- [ ] Define model/provider catalogs and discovery.
- [ ] Define routing, fallback, budget, residency, privacy, and capability
  decisions.
- [ ] Record the selected route and rationale per run.
- [ ] Define normalized direct-provider streaming/tool behavior.
- [ ] Create an adapter onboarding and conformance checklist.
- [ ] Avoid promising arbitrary model strings without validation.

### 6.9 `14-harness-tool-prd.md`

- [ ] Define the per-run executor contract.
- [ ] Define the long-lived harness-service lifecycle separately.
- [ ] Define tool input/output/effect schemas.
- [ ] Define exact grants, quotas, roots, host allowlists, and secret access.
- [ ] Define workspace isolation, staging, artifact capture, and cleanup.
- [ ] Preserve a compatibility bridge for existing harnesses.
- [ ] Define plugin install, start, health, update, disable, revoke, and removal.

### 6.10 `15-transport-prd.md`

- [ ] Define transport identity, ingress cursor, poll/receive, ACK, send, edit,
  delete/cancel where supported, and attachment operations.
- [ ] Define transport-neutral admission and egress semantics.
- [ ] Define the native/bridge Polkadot transport scope.
- [ ] Preserve protocol-specific invariants without leaking them into core.
- [ ] Define capability negotiation for transports lacking edits, files,
  presence, or notifications.
- [ ] Provide fake and conformance transports.

### 6.11 `16-security-prd.md`

- [ ] Maintain a threat model for users, senders, models, remote content, tools,
  extensions, providers, cloud staff, wallets, and chains.
- [ ] Draw trust boundaries for local and managed deployment.
- [ ] Define deterministic policy evaluation and denial/approval reasons.
- [ ] Define secret storage, resolution, rotation, redaction, and incident
  response.
- [ ] Define isolation tiers and their enforceable guarantees.
- [ ] Define gates for tools, egress, signing, broadcasting, payments,
  publishing, and policy changes.
- [ ] Turn security acceptance criteria into tests and operational controls.

### 6.12 `17-data-operations-prd.md`

- [ ] Define authoritative storage and transactional boundaries.
- [ ] Preserve SQLite/WAL as an initial profile without making the public
  contract SQLite-specific.
- [ ] Define recovery for crashes at every effect/delivery boundary.
- [ ] Define configuration precedence, revisions, reload, and invalid-state
  behavior.
- [ ] Define structured logs, traces, metrics, audit records, and read models.
- [ ] Define backup, restore, export, retention, deletion, legal hold, and
  derived-index cleanup.

### 6.13 `18-extension-sdk-prd.md`

- [ ] Publish stability layers for in-process APIs, process/RPC, WASM/component,
  and remote services.
- [ ] Define a versioned manifest and compatibility negotiation.
- [ ] Define SDKs without exposing unstable core internals.
- [ ] Define extension permissions, effects, health, quotas, updates, and
  revocation.
- [ ] Define migration for existing PCA plugins and bridge integrations.
- [ ] Provide conformance tests and example extensions.

### 6.14 `19-delivery-prd.md`

- [ ] Preserve seam-first delivery.
- [ ] Reconcile old phases with the complete long-term roadmap.
- [ ] Preserve the unit, integration, contract, fault, security, end-to-end,
  and user-validation test pyramid.
- [ ] Keep architectural guardrails measurable in CI.
- [ ] Add cloud, mobile, marketplace, payments, autonomy, product engineering,
  PVM, and JAM tracks.

### 6.15 `20-roko-design-review.md`

- [ ] Make `EffectIntent` first-class.
- [ ] Split preparation, approval, signing, submission, and finality.
- [ ] Separate execution terminality from delivery terminality.
- [ ] Require sequence, durability class, and one terminal stream event.
- [ ] Separate `HarnessService` lifecycle from `TurnExecutor`.
- [ ] Record safe model-routing decisions without premature self-optimization.
- [ ] Make `ResolvedGrant` a durable hashed artifact.
- [ ] Prefer manifest plus process/RPC boundaries before unsafe in-process
  plugins.
- [ ] Enforce admission limits for messages, files, artifacts, streams, queues,
  and public endpoints.
- [ ] Preserve raw evidence with classification, redaction, retention, and
  access boundaries.

### 6.16 `21-pca-compatibility-prd.md`

- [ ] Define C0, C1, C2, and improved native compatibility levels.
- [ ] Map PCA identity, user, device, conversation, message, project, and
  session IDs.
- [ ] Preserve exact admission ordering and ACK behavior.
- [ ] Preserve dedupe under at-least-once delivery.
- [ ] Preserve backpressure without data loss or false ACK.
- [ ] Preserve opener/session protocol details.
- [ ] Classify all inbound content.
- [ ] Model owed replies, worker claims, leases, retries, and outbox state.
- [ ] Preserve ordered Statement Store/outbound lanes and fencing.
- [ ] Version and authenticate the compatibility bridge.
- [ ] Preserve required route behavior and row contracts.
- [ ] Scope files/media and prevent path/tenant leakage.
- [ ] Preserve chunks, edits, progress, live replies, finals, and notifications.
- [ ] Define fixture capture, shadow mode, bridge-first migration, import, and
  rollback.
- [ ] Pass mandatory C0/C1 contract tests before compatibility claims.

### 6.17 `22-rust-workspace-blueprint.md`

- [ ] Reconcile the proposed crate tree with the definitive architecture.
- [ ] State allowed dependency direction and optional feature boundaries.
- [ ] Pin workspace-level compiler, edition, lint, formatting, audit, and
  dependency policies.
- [ ] Choose an MSRV policy explicitly.
- [ ] Define generated-schema/client ownership.
- [ ] Provide CI matrices for platforms, features, adapters, migrations, and
  contract suites.
- [ ] Convert the Phase 0 tasks into implementable milestones.
- [ ] Link every implementation-dependent choice to an ADR.

### 6.18 `23-architecture-decision-records.md`

- [ ] Revalidate ADR-001 Rust workspace/dependency direction.
- [ ] Revalidate ADR-002 durable ingress and transactional outbox.
- [ ] Revalidate ADR-003 per-conversation/partition single writer.
- [ ] Revalidate ADR-004 provider/model/executor/harness separation.
- [ ] Revalidate ADR-005 runtime-owned capability grants.
- [ ] Revalidate ADR-006 compatibility bridge as a projection.
- [ ] Revalidate ADR-007 SQLite as the initial authority.
- [ ] Resolve ADR-008 native Polkadot transport through evidence.
- [ ] Preserve ADR-009’s simple-run API while allowing phased workflow graphs.
- [ ] Add ADRs for cloud tenancy, custody modes, permissionless registries,
  autonomy, extension isolation, memory, and JAM/PVM boundaries.

### 6.19 `24-conformance-and-spike-plan.md`

- [ ] Execute or re-specify the Polkadot App/transport interoperability spike.
- [ ] Execute or re-specify executor streaming normalization.
- [ ] Execute crash-safe delivery recovery/fault injection.
- [ ] Publish shared adapter contract suites.
- [ ] Define “ready for implementation” with artifacts, not prose confidence.
- [ ] Extend the plan to signers, chain profiles, payments, cloud isolation,
  marketplace packages, and autonomous policy.

### 6.20 `25-normative-prd-addendum.md`

- [ ] Preserve the effect-intent rule.
- [ ] Preserve the terminality rule.
- [ ] Preserve the resolved-grant rule.
- [ ] Preserve the stream rule.
- [ ] Preserve the compatibility rule.
- [ ] Preserve the evidence rule.
- [ ] Preserve the chain-action rule.
- [ ] Preserve the admission rule.
- [ ] Preserve the harness-service rule.
- [ ] Reconcile old scope guardrails with the established long-term platform.

### 6.21 `30-polkadot-product-surface-research.md`

- [ ] Preserve OpenGov analyst/vote-preparation opportunities.
- [ ] Preserve Explain Before Sign as an early safety slice.
- [ ] Preserve metadata/runtime-upgrade monitoring.
- [ ] Validate Polkadot App/Product companion integration.
- [ ] Preserve JAM service-development work with experimental maturity labels.
- [ ] Carry each ecosystem uncertainty into a spike or evidence gate.
- [ ] Date and reverify claims before relying on them.

### 6.22 `31-agent-payments-research.md`

- [ ] Model payment as a typed constrained intent, not a prose/tool side effect.
- [ ] Separate checkout/authorization from settlement.
- [ ] Treat API micropayments and commerce payments as different products.
- [ ] Negotiate and pin rail/protocol versions and capabilities.
- [ ] Reverify Polkadot asset/stablecoin/payment-rail availability.
- [ ] Define quote, authorization, reservation, execution, settlement,
  reconciliation, refund, dispute, and unknown states.
- [ ] Support real payments and fully autonomous configured modes in later
  phases rather than retaining an old permanent no-money boundary.
- [ ] Define technical, fraud, accounting, tax/compliance, support, and
  operational boundaries.
- [ ] Require real-money acceptance and incident tests.

### 6.23 `32-agent-product-paradigms-research.md`

- [ ] Preserve the low-friction local-agent plus remote-chat pattern.
- [ ] Preserve asynchronous coding-agent progress and final-result UX.
- [ ] Treat browser sessions as credential-bearing, observable resources.
- [ ] Separate high-level browser tasks from raw browser control.
- [ ] Preserve skills as narrow contextual/capability packages.
- [ ] Make external actions configurable, permissioned, confirmed as selected,
  and attributable.
- [ ] Turn prioritized uses and commands into product-kit candidates.
- [ ] Reassess older non-scope against the complete-platform roadmap.

### 6.24 `33-product-engineering-vision-prd.md`

- [ ] Preserve the complete Agent Studio experience.
- [ ] Preserve the cross-surface Agent Inbox.
- [ ] Preserve skills and product kits.
- [ ] Preserve canonical approval and payment sheets.
- [ ] Preserve and expand the use-case portfolio.
- [ ] Define idea-to-build-to-test-to-publish-to-operate workflow.
- [ ] Expand “make, publish, charge” into self-hosted/cloud, marketplace, and
  autonomous phases.
- [ ] Replace overly restrictive non-goals with maturity and safety gates.

### 6.25 `34-polkadot-agent-landscape-research.md`

- [ ] Maintain a dated ecosystem/competitor map.
- [ ] Preserve the evidence that governance is an important use-case family
  without assuming it is the only wedge.
- [ ] Treat safe action composition as a core product boundary.
- [ ] Document the gap Polkagent intends to own.
- [ ] Validate differentiation claims before marketing them.
- [ ] Track changing standards and adjacent projects.

### 6.26 `35-technical-integration-research.md`

- [ ] Preserve a Subxt-first Rust integration profile where evidence still
  supports it.
- [ ] Define when full Polkadot SDK embedding is justified.
- [ ] Define PAPI and Product SDK/browser responsibilities.
- [ ] Define a narrow signer port and multiple custody adapters.
- [ ] Enforce exact payload, chain, account, metadata, and approval binding.
- [ ] Cover Hub, native assets, contracts/PVM, and EVM compatibility where
  relevant.
- [ ] Provide local, development, test, forked, and public-network test paths.
- [ ] Execute ordered Subxt, transport, signer, asset/payment, and contract
  spikes.
- [ ] Convert decisions to current ADRs.

### 6.27 `36-agent-business-product-research.md`

- [ ] Preserve operator control and trust filters without making publication
  centrally permissioned.
- [ ] Show extension listing identity, version, permissions, effects, data
  access, cost, risk, and support.
- [ ] Model provider, tool, browser, RPC, chain fee, storage, and support costs.
- [ ] Treat unknown cost as unknown, not zero.
- [ ] Define who pays under private, team, public, sponsored, and agent-funded
  modes.
- [ ] Cover marketplace/hosting abuse, fraud, spam, denial of wallet,
  chargebacks, disputes, and support.
- [ ] Preserve high-quality approval UX patterns.
- [ ] Treat embedded builders and deployment as strategic product capability.
- [ ] Reconcile old conservative business stages with permissionless
  registries and configurable autonomy.

### 6.28 `37-external-deep-research-synthesis.md`

- [ ] Preserve a narrow, measurable first validation slice.
- [ ] Record product hypotheses as evidence-seeking rather than facts.
- [ ] Preserve user comprehension as a release gate.
- [ ] Preserve explicit decision gates and stop/narrow criteria.
- [ ] Recast the refined MVP as one proving release, not the platform identity.
- [ ] Carry its documentation requirements into the final suite.

### 6.29 `38-product-validation-program.md`

- [ ] Define representative cohorts for builders, advanced users, teams,
  operators, mobile users, publishers, and autonomous-agent operators.
- [ ] Preserve problem interviews and artifact capture.
- [ ] Test approval, refusal, unknown-state, compromised-content, fee, network,
  and asset-identity scenarios.
- [ ] Measure comprehension and dangerous-fault detection.
- [ ] Define private-alpha posture and instrumentation.
- [ ] Define metrics without collecting unnecessary private content.
- [ ] Use a reusable experiment decision template.
- [ ] Maintain a prioritized research backlog.

### 6.30 `39-report-18-mvp-execution-charter.md`

- [ ] Preserve the advanced-user/action hypothesis as a hypothesis.
- [ ] Preserve the 90-day evidence sequence where still useful.
- [ ] Preserve concrete safety/comprehension thresholds.
- [ ] Preserve initial metric scorecards.
- [ ] Map work packages to the definitive crate/doc architecture.
- [ ] Preserve stop/narrow triggers.
- [ ] Supersede language that excludes central long-term pillars.

### 6.31 `40-report-18-evidence-backlog.md`

- [ ] Map each E1–E9 blocker to a current owner, artifact, and decision.
- [ ] Preserve the evidence-quality rubric.
- [ ] Create or explicitly defer each required research artifact.
- [ ] Define how evidence updates decisions and traceability.
- [ ] Add new blockers for cloud, marketplace, autonomy, custody, product
  building, and JAM/PVM.

### 6.32 `99-deep-research-dossier.md`

- [ ] Reconcile its executive brief with current owner decisions.
- [ ] Preserve the integrated product vision, pillars, users, jobs, and kits.
- [ ] Preserve ecosystem and adjacent-product evidence subject to revalidation.
- [ ] Preserve Studio, Inbox, skills, extension, approval, and payment UX.
- [ ] Preserve layered architecture, ports, durable execution, and persistence.
- [ ] Preserve full PCA compatibility requirements.
- [ ] Preserve provider/model/harness distinctions.
- [ ] Preserve security, grants, signers, chain, payments, economics, and
  deployment requirements.
- [ ] Preserve selective Roko adoption.
- [ ] Replace its old sequence with the complete current roadmap.
- [ ] Resolve every central open question or assign it to research.
- [ ] Retain its local source map as traceability evidence.

### 6.33 `100-follow-on-deep-research-dossier.md`

- [ ] Reconcile all fourteen prompts with research already completed.
- [ ] Retain unanswered customer/MVP questions.
- [ ] Retain chain/signer/approval feasibility questions.
- [ ] Retain Statement/PCA and Product/App/mobile support questions.
- [ ] Retain competitor, governance, payment, extension, distribution,
  storage/SLO, assurance, product-engineering, and abuse questions.
- [ ] Apply its primary-source, dates, maturity, contradiction, and
  decision-changing output standards.
- [ ] Define a research cadence and evidence-refresh policy.

### 6.34 `101-mvp-deep-research-prompt.md`

- [ ] Preserve its reusable query structure.
- [ ] Preserve expected decision artifacts rather than merely a narrative
  report.
- [ ] Mark questions answered by reports 17–19 or newer research.
- [ ] Expand future prompts beyond the first slice without losing focus.

### 6.35 `102-follow-on-deep-research-prompts.md`

This later prompt pack adds 20 concrete research programs. Its referenced
opportunity catalog is now preserved as `PRDs/reserach/research1.md`; the
essential obligations are also repeated here so future authors can use this
checklist without opening that catalog first.

- [ ] Produce byte-level Statement Store/App Chat/HOP protocol documentation,
  deterministic crypto/codec vectors, Rust crate compatibility evidence, and a
  native-transport go/no-go.
- [ ] Prove a current Subxt metadata-pinned construct/decode/preflight/fee/drift
  pipeline and test static versus dynamic APIs across runtime upgrades.
- [ ] Audit current wallet, hardware, mobile, proxy, and multisig signer UX for
  exact payload/metadata/intent binding and adversarial substitution.
- [ ] Validate or reject the advanced transaction-reviewer persona with
  incidents, existing-tool gaps, interviews, workflow evidence, and local
  installation willingness.
- [ ] Audit competitor repositories, licenses, deployments, maintainership,
  users, funding, and actual feature/security behavior rather than relying on
  announcements.
- [ ] Design and test approval-card comprehension against batch-call hiding,
  proxy addition, poisoned addresses, excessive fees/slippage, and model/card
  contradiction.
- [ ] Build a dated production/API matrix for Polkadot mobile/app, Product SDK,
  Statement Store, Bulletin, identity, signing, and bot lifecycle, plus
  fallback surfaces.
- [ ] Test x402, AP2, and ACP technical compatibility with Polkadot accounts,
  Hub assets/EVM, signatures, settlement, threats, and regulatory-role
  questions.
- [ ] Define authoritative OpenGov data sources, freshness, provenance,
  conviction/delegation math, parameter drift, and a quantified governance
  workload.
- [ ] Test XCM v5 route, fee, dry-run, destination, refund/failure, and per-hop
  explanation feasibility across selected networks.
- [ ] Benchmark SQLite/WAL concurrency and disk behavior; execute crash,
  backup, restore, schema migration, blob lifecycle, and scale-path drills.
- [ ] Define honest macOS/Linux/container process-isolation profiles,
  termination/resource controls, and residual risk.
- [ ] Design funded autonomous agent accounts using proxies, multisig,
  programmable filters, budgets, isolated signing, attack analysis, and
  recovery.
- [ ] Create a prompt-injection/confused-deputy threat taxonomy, red-team
  corpus, mitigation map, false-positive study, and approval-fatigue controls.
- [ ] Maintain dated JAM/CorePlay/PVM maturity and explicit engagement trigger
  conditions.
- [ ] Size and prioritize Polkadot product kits using user counts, pain,
  feasibility, value, existing tools, distribution, manifests, and safe
  defaults.
- [ ] Specify evidence-package contents, serialization, hashes, privacy,
  access, retention, cost, deletion implications, and optional Bulletin/IPFS/
  local anchoring.
- [ ] Prove or reject the complete mobile Agent Inbox flow from chat action
  card through on-device signature and finality receipt; select a fallback if
  unavailable.
- [ ] Test an agent-assisted runtime storage-migration author/dry-run workflow
  using Chopsticks/try-runtime or alternatives, with weight/state-diff/decode
  evidence and CI integration.
- [ ] Design a runtime-metadata drift watcher that regenerates Subxt/PAPI/Dedot
  types, tests call-site changes, and opens an evidence-backed pull request.
- [ ] Reconcile prompt wording that assumes permanent human approval,
  local-only operation, or distant autonomy with the owner-confirmed flexible
  custody, equal cloud, and fully autonomous long-term direction.
- [ ] Preserve each prompt’s success criterion and convert completed findings
  into PRD decisions rather than accumulating narrative reports.

## 7. Cross-cutting definitive coverage

The source-specific checklist proves ingestion. This section proves that the
result becomes a cohesive product and architecture.

### 7.1 Vision, users, and jobs

- [ ] State a one-sentence promise for the complete platform.
- [ ] State the three equal pillars: build, act/pay, reach users.
- [ ] Define individuals, Polkadot developers, runtime/contract teams, product
  teams, advanced users, organizations, operators, publishers, customers, and
  autonomous-agent owners.
- [ ] Define local/private, team, hosted, public, paid, and autonomous jobs.
- [ ] Define the relationship between platform, agent, product, product kit,
  deployment, and marketplace listing.
- [ ] Separate north-star scope, committed roadmap, experiments, and current
  release.
- [ ] Define measurable user, business, reliability, trust, and ecosystem
  outcomes.
- [ ] State genuine non-goals without excluding established later phases.
- [ ] Explain why Polkagent is Polkadot-native and what makes it differentiated.
- [ ] Avoid unsupported claims about ecosystem maturity or competitors.

### 7.2 Product UX and surfaces

- [ ] Define first-run onboarding for local and cloud users.
- [ ] Define “describe outcome” and product-kit start paths.
- [ ] Define Agent Studio information architecture.
- [ ] Define Agent Inbox and durable cross-surface conversations.
- [ ] Define project/workspace/session/model/harness controls.
- [ ] Define run cards with progress, waiting, cost, warnings, and final result.
- [ ] Define canonical approval/action/payment sheets.
- [ ] Define files, artifacts, citations, diffs, simulation, and receipts.
- [ ] Define web/PWA, CLI/TUI, API/SDK, operator console, and marketplace UX.
- [ ] Define Polkadot App/mobile integration and fallback UX.
- [ ] Evaluate desktop and dedicated native mobile against user value.
- [ ] Define notification, offline/resume, deep-link, and signer handoff.
- [ ] Define beginner presets and expert configuration.
- [ ] Define accessibility, localization, responsive behavior, and error
  recovery.
- [ ] Test comprehension rather than measuring only clicks or completion.

### 7.3 Domain vocabulary and specifications

- [ ] Define canonical `AgentSpec`, `ProductSpec`, `DeploymentSpec`,
  `WorkflowSpec`, and extension manifests.
- [ ] Define user, identity, principal, agent, service, tenant, organization,
  project, workspace, conversation, turn, run, task, and node.
- [ ] Define artifact, event, effect, attempt, projection, evidence, grant,
  policy, approval, signer, account, intent, payment, and receipt.
- [ ] Define stable IDs and serialization/versioning.
- [ ] Define configuration revision and content digest semantics.
- [ ] Define how schemas evolve and migrate.
- [ ] Provide Rust types and wire-schema examples.
- [ ] Prevent synonymous terms from diverging across PRDs.

### 7.4 Runtime, effects, and recovery

- [ ] Define aggregate ownership and concurrency.
- [ ] Define run/turn/effect/delivery/action/payment state machines.
- [ ] Require durable-before-ACK ingress.
- [ ] Require atomic state change and outbox enqueue.
- [ ] Require idempotency, claim leases, retry policy, and attempt history.
- [ ] Define cancellation, interruption, deadlines, and orphan recovery.
- [ ] Define stream ordering, durability classes, and terminality.
- [ ] Separate execution from delivery completion.
- [ ] Separate sign, submit, include, finalize, revert, fail, and unknown.
- [ ] Define compensations without promising impossible rollback.
- [ ] Define deterministic replay boundaries.
- [ ] Define quotas and backpressure at every admission boundary.
- [ ] Provide crash/fault matrices and property tests.

### 7.5 Providers, models, routing, and harnesses

- [ ] Define hosted, gateway, OpenAI-compatible, Anthropic-style, local, and
  custom provider adapters.
- [ ] Define model metadata, capabilities, context, pricing, residency, and
  lifecycle.
- [ ] Define explicit and automatic routing with recorded reasons.
- [ ] Define fallbacks without repeating unsafe side effects.
- [ ] Define direct provider execution and normalized tool calling.
- [ ] Define coding CLI and external-framework harness execution.
- [ ] Define long-lived harness lifecycle, health, upgrade, and resume.
- [ ] Define ACP/A2A or other interoperability only behind verified adapters.
- [ ] Define normalized events and usage accounting.
- [ ] Define secrets and connection-profile resolution.
- [ ] Publish conformance tests and onboarding examples.

### 7.6 Tools, skills, workspaces, and browser activity

- [ ] Define typed tool manifests and effect classes.
- [ ] Define filesystem roots, path canonicalization, network allowlists,
  subprocess, environment, and secret boundaries.
- [ ] Define worktree/project lifecycle, staging, diffs, commits, and artifacts.
- [ ] Define sandbox/isolation tiers.
- [ ] Define context packs separately from executable skills.
- [ ] Define skill manifests, tests, permissions, costs, risks, and versions.
- [ ] Define browser tasks and credential-bearing browser sessions separately.
- [ ] Define generated code/build/test claims as evidence-backed artifacts.
- [ ] Define tool mediation for direct providers and harnesses consistently.
- [ ] Define cleanup, retention, and recovery of workspaces and sessions.

### 7.7 PCA, transport, chat, and mobile

- [ ] Define C0–C3 compatibility.
- [ ] Preserve encrypted/outbound topology where required.
- [ ] Preserve polling/session, ACK, dedupe, lease, owed-work, and lane
  semantics.
- [ ] Preserve direct brains, bridge harnesses, T3ams, Hermes/OpenClaw, files,
  live replies, and CLI workflows.
- [ ] Define transport-neutral core traits.
- [ ] Define current Polkadot App/Product/Statement/Bulletin capability matrix.
- [ ] Define a browser/PWA fallback for unavailable mobile capabilities.
- [ ] Define cross-surface conversation, attachment, approval, and notification
  behavior.
- [ ] Define identity/state/config/project/session import and rollback.
- [ ] Capture and version compatibility fixtures.
- [ ] Test replacement, duplicate, reorder, crash, and degraded-network cases.

### 7.8 Polkadot product engineering

- [ ] Define supported repository and workspace discovery.
- [ ] Define Polkadot SDK/runtime/pallet context packs and toolchains.
- [ ] Define contract/PVM/PolkaVM create-build-test-deploy flows.
- [ ] Define runtime build, benchmark, migration, upgrade, and metadata flows.
- [ ] Define local node/network and test-environment orchestration.
- [ ] Define code, architecture, test, audit, deployment, and runbook artifacts.
- [ ] Define Product SDK/PAPI/client-product workflows.
- [ ] Define indexer, RPC, explorer, wallet, and infrastructure integrations.
- [ ] Define permissions for code execution and deployment.
- [ ] Define product-kit templates without hiding underlying specs.
- [ ] Define team review, CI, release, and incident workflows.
- [ ] Add JAM service-development tooling behind explicit maturity gates.

### 7.9 Chain integration and actions

- [ ] Define network profiles pinned by genesis/spec identity.
- [ ] Define runtime metadata acquisition, verification, caching, and drift.
- [ ] Define asset identity by canonical location/ID and decimals, not ticker.
- [ ] Define Subxt and generated/dynamic API strategy.
- [ ] Define PAPI/Product SDK division at browser/mobile boundaries.
- [ ] Define reads, decode, preflight, simulation, signing, submission, and
  finality ports.
- [ ] Define fees, existential constraints, balances, locks, holds, and
  insufficient-evidence behavior.
- [ ] Define XCM location, versioning, fees, dry-run, delivery, and uncertain
  outcome handling.
- [ ] Define OpenGov, identity, proxy, multisig, staking, treasury, contract,
  PVM, and runtime action families separately.
- [ ] Define chain-specific fixtures and fork/testnet strategy.
- [ ] Refuse unsupported or ambiguously decoded effects.
- [ ] Preserve evidence used at approval time.

### 7.10 Policy, identity, custody, and security

- [ ] Define principal and identity types.
- [ ] Support local, pseudonymous, on-chain, organizational, device, publisher,
  and agent identities.
- [ ] Treat identity/reputation/personhood as optional policy inputs.
- [ ] Define policy composition and deterministic decisions.
- [ ] Define immutable `ResolvedGrant`.
- [ ] Define approval types: person, quorum, multisig, service, session, batch,
  mandate, and policy-autonomous.
- [ ] Define wallet, hardware, proxy, multisig, local keystore, KMS/HSM, MPC,
  remote signer, programmable account, and agent-account adapters.
- [ ] Keep raw key material out of model and ordinary extension boundaries.
- [ ] Define signing payload/network/metadata/approval binding.
- [ ] Define secret storage, rotation, export limitations, and recovery.
- [ ] Define data classification, taint, declassification, egress, and
  retention.
- [ ] Define threat models for prompt injection, confused deputy, supply chain,
  tenant escape, malicious RPC, compromised provider, and wallet drain.
- [ ] Define pause, revoke, quarantine, circuit-break, and incident workflows.
- [ ] Red-team local, cloud, public, marketplace, and fully autonomous modes.

### 7.11 Payments and fully autonomous action

- [ ] Define `PaymentIntent`, quote, mandate, allowance, reservation,
  settlement, receipt, refund, dispute, reconciliation, and unknown state.
- [ ] Separate service checkout from chain settlement.
- [ ] Separate micropayment, commerce, subscription, escrow, streaming, and
  agent-to-agent modes.
- [ ] Define native Polkadot asset/payment adapters and external rails where
  useful.
- [ ] Define per-action, rolling, and lifetime budgets plus explicit broad
  configurations.
- [ ] Define fee, slippage, destination, contract, rate, and concurrency policy.
- [ ] Define observe, prepare, approve, batch, policy-autonomous, and fully
  autonomous levels.
- [ ] Permit all supported action families to become no-human-in-loop when
  explicitly configured and technically supported.
- [ ] Preserve audit, signer isolation, revocation, and truthful status in fully
  autonomous mode.
- [ ] Define independently funded agent accounts and replenishment.
- [ ] Define accounting, tax/compliance surfaces, fraud, abuse, sanctions/risk
  hooks where applicable, and operator responsibility.
- [ ] Define cost envelopes including model, tool, compute, storage, RPC, chain
  fee, marketplace, support, and unknown cost.
- [ ] Define emergency controls and unknown-finality behavior.
- [ ] Gate real money with reconciliation and incident exercises.

### 7.12 Memory, learning, groups, feeds, and triggers

- [ ] Define working, episodic, semantic, procedural, preference, and policy
  memory.
- [ ] Define provenance, citations, classification, visibility, retention,
  deletion, and export.
- [ ] Define full-text, vector, graph, and external-store projections.
- [ ] Define deterministic context budgets and assembly reports.
- [ ] Define user correction and memory review UX.
- [ ] Define eval datasets, graders, human review, and production monitoring.
- [ ] Define versioned promotion/rollback for learned changes.
- [ ] Prevent learning from self-expanding authority.
- [ ] Define agent groups, roles, delegation, quorum, shared artifacts, and
  escalation.
- [ ] Define feed, schedule, webhook, chain-event, message, and manual triggers.
- [ ] Define duplicate/coalescing/backpressure behavior for triggers.
- [ ] Define workflow graphs as an optional layer over the same effect kernel.
- [ ] Define simulation and replay for workflows and groups.

### 7.13 Permissionless extensions and marketplace

- [ ] Define built-in, process/RPC, WASM/component, and remote extension tiers.
- [ ] Define package identity, content digest, signature, publisher, license,
  compatibility, dependency, and lockfile.
- [ ] Support local, private, community, federated, on-chain, and hosted
  registries.
- [ ] Permit self-publication without a central approval gate.
- [ ] Provide verified/curated default views without preventing other sources.
- [ ] Define install preview, permissions, data access, effects, costs, risks,
  support, and update policy.
- [ ] Define sandboxed preview, conformance, malware/supply-chain checks, and
  reproducible builds.
- [ ] Define ratings, reviews, reputation, provenance, usage, and dispute
  signals.
- [ ] Define free, paid, metered, sponsored, subscription, and negotiated
  listings.
- [ ] Define creator, operator, registry, protocol, and referral fees.
- [ ] Define revocation, vulnerability response, staged update, rollback,
  mirrors, and offline bundles.
- [ ] Define organization policy and allow/deny controls.
- [ ] Define product kits, skills, tools, models, compute, feeds, services, and
  agents as distinct listing types.

### 7.14 Persistence, observability, and operations

- [ ] Define initial SQLite/WAL schema and a storage-port evolution path.
- [ ] Define events, effects, attempts, artifacts, projections, configs,
  policies, approvals, accounts, payments, and tenancy schemas.
- [ ] Define transaction boundaries and migrations.
- [ ] Define artifact object storage and encryption.
- [ ] Define logs, traces, metrics, audit, cost, policy, security, and chain
  finality telemetry.
- [ ] Define operator timelines and lenses without duplicating authority.
- [ ] Define backup, restore, point-in-time recovery, export, import, retention,
  deletion, and legal hold.
- [ ] Define disk/queue/quota pressure behavior.
- [ ] Define health, readiness, doctor, diagnostics, and support bundles.
- [ ] Define SLOs, alerts, runbooks, incidents, and disaster recovery.
- [ ] Define privacy-preserving product analytics.
- [ ] Ensure sensitive content is not emitted by default.

### 7.15 Local, cloud, tenancy, and deployment

- [ ] Define laptop/daemon, container, server, cluster, hybrid, and managed
  deployment profiles.
- [ ] Keep public domain/config/extension contracts equivalent across modes.
- [ ] Define data plane versus control plane.
- [ ] Define tenant/org/project/environment/user/service-account hierarchy.
- [ ] Enforce tenant scoping in database, queues, objects, caches, telemetry,
  keys, support, and billing.
- [ ] Define authentication, authorization, federation/SSO, service identity,
  and break-glass.
- [ ] Define regions, residency, encryption, customer-managed keys, backups,
  and deletion.
- [ ] Define fleet registration, rollout, health, drift, rollback, and offline
  behavior.
- [ ] Define scheduling, worker placement, quotas, noisy-neighbor protection,
  and scaling.
- [ ] Define hosted provider/harness/tool/signer integrations.
- [ ] Define billing, metering, plan enforcement, and cost controls.
- [ ] Define state/config/identity/artifact export and cloud-to-local migration.
- [ ] Define admin/support access and immutable audit.
- [ ] Test tenant isolation and portability.

### 7.16 APIs, SDKs, configuration, and migration

- [ ] Define Rust public APIs and feature stability.
- [ ] Define versioned HTTP/gRPC/WebSocket/event protocols as appropriate.
- [ ] Define TypeScript/browser and other generated SDKs.
- [ ] Define CLI command surface and machine-readable output.
- [ ] Define declarative config, environment/secret references, precedence,
  validation, and reload.
- [ ] Define JSON Schema/OpenAPI/Protobuf or equivalent ownership.
- [ ] Define pagination, filtering, idempotency, concurrency control, and error
  envelopes.
- [ ] Define event subscriptions and resume cursors.
- [ ] Define import/export packages and schema migrations.
- [ ] Define PCA state/config/identity/project/session migration.
- [ ] Define extension and marketplace package migrations.
- [ ] Define rollback and partial-failure recovery.
- [ ] Publish complete examples for local, cloud, approval-based, and autonomous
  setups.

### 7.17 Testing, evidence, security assurance, and release gates

- [ ] Define unit, property, model/state-machine, integration, contract,
  compatibility, fault, security, end-to-end, UX, load, and soak tests.
- [ ] Build provider, harness, tool, transport, signer, chain, payment,
  extension, store, and control-plane contract suites.
- [ ] Preserve PCA compatibility fixture corpora.
- [ ] Build chain metadata/action/fee/simulation/finality fixtures.
- [ ] Test duplicates, reordering, crash points, leases, cancellation, unknown
  outcomes, and recovery.
- [ ] Test compromised content, prompt injection, malicious extensions, signer
  substitution, tenant escape, and supply-chain attacks.
- [ ] Run approval/comprehension studies with dangerous fixtures.
- [ ] Define benchmarks and resource/cost budgets.
- [ ] Define E0–E4 evidence and decision gates.
- [ ] Assign every research question an owner, artifact, and review date.
- [ ] Define release gates separately for local, cloud, mobile, money,
  marketplace, custody, and fully autonomous modes.
- [ ] Define stop, narrow, disable, rollback, and incident criteria.
- [ ] Date and refresh time-sensitive ecosystem facts.

### 7.18 Roadmap and implementation readiness

- [ ] Publish a north-star architecture independent of release timing.
- [ ] Publish phased releases across all three central pillars.
- [ ] Keep Explain Before Sign as a proving slice, not the sole identity.
- [ ] Define a three-pillar demonstrator.
- [ ] Define dependencies and parallelizable workstreams.
- [ ] Define maturity labels for current, planned, experimental, and
  research-only integrations.
- [ ] Define implementation milestones, owners, artifacts, and exit criteria.
- [ ] Define ADRs required before dependent coding.
- [ ] Define cloud, marketplace, payment, autonomy, mobile, PVM, and JAM
  reopening/advancement gates.
- [ ] Define backward-compatibility and migration milestones.
- [ ] Define documentation, examples, SDK, and operator-readiness gates.
- [ ] Avoid calendar certainty unsupported by team size and evidence.

## 8. High-risk details that must not be lost

- [ ] `EffectIntent` exists separately from events and results.
- [ ] Execution terminality differs from delivery terminality.
- [ ] Streams carry sequence, durability class, attempt identity, and at most
  one terminal event.
- [ ] `HarnessService` differs from one `TurnExecutor` call.
- [ ] `ResolvedGrant` is a hashed durable record.
- [ ] Forensic evidence has stricter access/retention than routine telemetry.
- [ ] PCA uses per-device/session polling, durable ACK, owed work, and
  replacement-safe ordered egress.
- [ ] Browser session state is credential-bearing.
- [ ] Unknown cost is not zero cost.
- [ ] Network identity is not a display name.
- [ ] Asset identity is not a ticker.
- [ ] Submission is not finality.
- [ ] Unknown chain outcome is not success or safe retry.
- [ ] Approval UI is generated from typed canonical data.
- [ ] User comprehension and dangerous-fault detection are release gates.
- [ ] Memory/eval/model consensus cannot confer authority.
- [ ] Fully autonomous mode retains signer isolation and audit.
- [ ] Permissionless publication does not imply permissionless execution.
- [ ] Local/cloud portability includes derived-memory rebuild inputs.
- [ ] JAM/PVM claims carry explicit maturity and verification dates.

## 9. External inputs not included in the 35-file count

These must also be traced before the suite is considered definitive. The
summaries below explain why each matters; the path is provenance rather than
required background reading.

### 9.1 Current owner decisions

- [ ] **Input:** `PRDs/01-ESTABLISHED-BASELINE.md`.
- **Inline context:** This records the complete-platform north star and the most
  recent decisions: equal build/action/reach pillars; Rust-first boundaries;
  full PCA/mobile compatibility through the best UX; equal local/cloud
  operation; configurable custody; fully autonomous supported actions when
  configured; permissionless plural marketplaces with safe default views; and
  selective use of Roko’s advanced patterns.
- **Coverage rule:** No older MVP exclusion may override this input.

### 9.2 Deep-research reports 17–19

- [ ] **Input:** `deep-research-report (17).md`.
- **Inline context:** This report evaluated the broad dossier, endorsed the
  trust/evidence architecture, proposed Explain Before Sign as the first wedge,
  Governance Scout second, and Builder Copilot later, and enumerated 20 product,
  ecosystem, signer, payment, operations, extension, and human-factors research
  questions. It explicitly noted that it had received documents rather than a
  live codebase.
- **Coverage rule:** Preserve its questions, primary-source discipline,
  measurement plans, and risk analysis. Treat its narrow sequencing and
  conservative later-feature posture as research advice superseded where the
  owner has made a broader long-term decision.

- [ ] **Input:** `deep-research-report (18).md`.
- **Inline context:** This report assessed the documentation-heavy repository
  as a strong but pre-proof architecture proposal. It further narrowed the
  recommended first persona to an advanced Polkadot user and the first action
  to one bounded Explain Before Sign flow. It highlighted Subxt, signer
  isolation, transactional effects/outbox, PCA compatibility, comprehension
  testing, CI/conformance, and the lack of live implementation/user evidence.
- **Coverage rule:** Preserve its evidence gates, architecture strengths,
  implementation cautions, metrics, testing, and 90-day validation structure.
  Do not turn “defer from first release” into “exclude from the platform.”

- [ ] **Input:** `deep-research-report (19).md`.
- **Inline context:** This report reiterated that the repository was
  decision-grade design rather than verified software; favored a minimal
  Subxt-backed Explain Before Sign slice; and emphasized canonical
  metadata/evidence, external signer handoff, durable `EffectIntent`, grants,
  recovery, PAPI/Product SDK companion roles, and explicit release evidence.
- **Coverage rule:** Use its first slice to validate the stable kernel. Preserve
  the owner’s later decision that PCA/mobile, product engineering, payments,
  cloud, marketplace, and autonomy remain equal long-term platform work.

### 9.3 Reference implementations

- [ ] **Input:** full `polkadot-chat-agents` implementation and fixtures.
- **Inline context:** PCA is the compatibility ground truth for its actual
  bot identity, message admission/ACK, session/device, owed-work, ordered
  egress, direct coding agents, bridge integrations, files/media, live replies,
  projects, configuration, CLI, and deployment behavior. Documentation alone
  cannot prove all edge cases.
- **Coverage rule:** Capture versioned fixtures and observable behavior; do not
  preserve internal coupling merely because it exists.

- [ ] **Input:** full Roko repository, `docs`, and `tmp`.
- **Inline context:** Roko is a broad agent platform containing both
  implemented behavior and speculative designs for artifacts/events/effects,
  protocol ports, graph composition, policy/taint, memory/learning,
  groups/feeds/triggers, extensions, evaluation, telemetry, identity,
  marketplace, payments, and deployment.
- **Coverage rule:** Label implementation versus design and give every pattern
  an adopt/adapt/defer/reject decision tied to a Polkagent job.

### 9.4 New self-contained research dossiers

- [ ] **Input:** `PRDs/research-roko-definitive.md`.
- **Inline context:** Provides the Roko primer, exact source provenance,
  pattern-by-pattern rationale, concrete Polkagent adaptations, tradeoffs, and
  adoption matrix.

- [ ] **Input:** `PRDs/research-pca-cloud-compat.md`.
- **Inline context:** Reconstructs PCA’s user/runtime behavior, C0–C3
  compatibility, schemas, state machines, migration, the Rust successor, and
  local/managed control-plane implications.

- [ ] **Input:** `PRDs/research-polkadot-jam-products.md`.
- **Inline context:** Provides a dated ecosystem primer, support/maturity
  matrix, API responsibility map, chain/mobile/contracts/PVM/JAM flows, risks,
  and release spikes.

- [ ] **Input:** `PRDs/reserach/research1.md`.
- **Inline context:** Provides a non-normative Build/Act/Reach opportunity
  catalog covering builder workflows, explain-before-sign and payment
  products, mobile/chat reach, identity, marketplace, cloud, governance, XCM,
  PVM, and JAM. It labels evidence and readiness, states the current owner
  boundaries inline, and makes every proposed sequence conditional on
  technical, security, UX, legal, operational, and support gates. The
  established baseline overrides any conflicting candidate recommendation.

- [ ] **Input:** `PRDs/reserach/research2.md`.
- **Status and scope:** This is a non-normative, dated external desk-research
  synthesis (access window 2026-07-29), not one of the 35 direct documents in
  the `tmp` source inventory above. It consolidates the twenty follow-on
  research prompts: Statement Store/HOP Rust portability; Subxt and metadata
  drift; signers; reviewer demand and competitors; approval comprehension;
  App/Product surfaces; payments; OpenGov; XCM; SQLite/recovery; isolation;
  proxy-funded agent accounts; prompt injection; JAM/CorePlay/PVM maturity;
  product kits; Bulletin evidence; mobile inboxes; storage migrations; and
  typed-client regeneration.
- **Inline context:** Its most useful evidence is the maturity distinction:
  Subxt/metadata/dry-run, signer, local evidence, migration, and test tooling
  are candidate Rust-first adapter work; Statement Store/App wire behavior,
  Bulletin production use, and JAM/CorePlay require explicit compatibility or
  maturity gates. A temporary JS codec bridge may be a bounded transport leaf,
  but cannot own core state, policy, grants, custody, or secret authority.
- **Resolved candidate conflicts:** Its Act-focused first slice, Matrix/Telegram
  fallback, approval-centric examples, and reviewed-registry language are not
  permanent platform limits. Apply the owner baseline: Build/Act/Reach remain
  equal; PCA behavior/migration is preserved and improved even if a native Rust
  transport is phased; local/self-hosted and managed workers use portable
  contracts; identity and custody are optional/configurable; supported action
  families may use a valid autonomous mandate; and marketplace publication is
  permissionless/plural with safe discovery views rather than a central gate.
  Use the canonical lifecycle vocabulary: `EffectIntent` is the command before
  I/O, `EffectAttempt` records claim/lease/retry, `EffectOutcome` is the
  immutable observed result, and a chain saga status is not an effect outcome.
- **Coverage rule:** Treat every external claim as dated evidence, not a
  requirement. Adopt it only through a definitive PRD/ADR with the applicable
  source/version fixture, product hypothesis, security/custody analysis, and
  acceptance gate. Re-check live network, runtime metadata, wallet, protocol,
  Product/App, payment, and JAM/PVM facts at implementation time.

- [ ] **Input:** `PRDs/00-DEEP-RESEARCH-BRIEF.md` and future research results.
- **Inline context:** Defines 13 remaining research packages, a common
  standalone context block, evidence standards, output schemas, cross-package
  questions, and the expected definitive suite.

### 9.5 Time-sensitive primary sources

- [ ] **Input:** current official Polkadot, Polkadot SDK, Product/App, PAPI,
  Subxt, PolkaVM/PVM, and JAM primary sources at implementation time.
- **Inline context:** Network features, SDK APIs, repositories, product-host
  capabilities, runtime metadata, wallets, test environments, and JAM/PVM
  tooling can change after this corpus is written.
- **Coverage rule:** Date every material claim, pin versions/profiles where
  appropriate, and treat a roadmap or proposal as evidence of intent rather
  than production support.

## 10. Conflict-resolution checklist

- [x] Long-term complete platform overrides “single-feature product.”
- [x] Explain Before Sign retained as an early validation/architecture slice.
- [x] Product engineering, on-chain/payments, and PCA/mobile remain equal
  pillars.
- [x] PCA behavior is preserved through better abstractions and compatibility
  levels.
- [x] Surface choice follows best UX; PCA/Polkadot mobile behavior is a minimum
  strategic target, not a mandate to build a redundant native app.
- [x] Custody is flexible; external/user signing is the safe default.
- [x] Fully autonomous execution is allowed when configured, including
  high-impact supported action families.
- [x] Raw keys remain outside model/tool/plugin context even in autonomous
  mode.
- [x] Marketplace/registry architecture is permissionless and plural.
- [x] Verified/curated discovery may be a default UX but not a publication
  gate.
- [x] Local/self-hosted and managed cloud are equally first-class.
- [x] Rust is primary; TypeScript remains valid at browser/Product boundaries.
- [x] Simple runs do not require a graph; advanced workflow/group capabilities
  remain in scope.
- [x] Roko memory, learning, groups, feeds, triggers, extensions, identity,
  reputation, and evals are adopted selectively with authority boundaries.
- [x] JAM/PVM ambition remains explicit and maturity-gated.
- [ ] Propagate every resolution into all affected definitive PRDs.
- [ ] Check that no copied old non-goal silently reverses an owner decision.

## 11. Definition of complete

The definitive PRD suite is not complete until:

- [ ] All 35 source rows are marked **Absorbed**.
- [ ] All normative source rows are marked **Verified** against a target PRD.
- [ ] Every checklist item is accepted, defaulted, deferred, experimental,
  rejected, or superseded; none is silently omitted.
- [ ] Every deferred/experimental item has a dependency or reopening gate.
- [ ] Every rejected/superseded item preserves its useful rationale.
- [ ] Every normative invariant has an acceptance artifact.
- [ ] Every API, trait, schema, state machine, and config example uses the same
  vocabulary.
- [ ] Local, cloud, mobile, compatibility, marketplace, payment, custody, and
  autonomy modes are mutually coherent.
- [ ] Cross-document links and target IDs resolve.
- [ ] The suite is understandable to a first-time reader without the old
  corpus.
- [ ] A final traceability matrix maps requirement → source → decision →
  PRD/ADR → phase → test/evidence.
