# Polkagent definitive-PRD deep-research brief

**Status:** research input for the authoritative PRD suite

**Purpose:** drive a final research pass before replacing the iterative planning
corpus with a cohesive, implementation-ready set of PRDs. This brief reflects
the owner’s clarified product intent and incorporates the findings of
`deep-research-report (19).md` without reducing the long-term platform to that
report’s narrow MVP recommendation.

## 0. Reader orientation

### 0.1 What exists today

Polkagent is currently a product and architecture design effort. The repository
contains research, reconnaissance, early PRDs, architecture decisions, and
delivery plans; it should not be mistaken for a production implementation.
This research brief is intended to close evidence gaps before a definitive PRD
suite and Rust workspace are created.

Two existing codebases provide important context:

- **`polkadot-chat-agents` (PCA)** is the reference product Polkagent must
  preserve and improve. It lets users interact with coding agents and other
  agent runtimes through Polkadot-oriented private/mobile chat. It already
  contains important behavior around bot identity, message polling,
  durable-before-acknowledgement processing, direct coding-agent processes,
  framework bridges, project files, live replies, configuration, and
  deployment. Polkagent should reproduce compatible user-visible behavior
  through cleaner versioned abstractions rather than copying PCA’s internal
  implementation.
- **Roko** is a separate, broad agent-platform design and implementation
  project. It contains useful patterns for durable artifacts and events,
  effectful execution, narrow adapter interfaces, policy/capability resolution,
  context and memory, graphs, groups, feeds/triggers, extensions, evaluation,
  telemetry, and platform operations. Polkagent should select the strongest
  patterns and simplify or reject abstractions that do not serve concrete
  Polkadot product workflows.

Three external deep-research reports informed this brief. Their most important
contribution was to distinguish a credible first proving slice from the
long-term product. Report 19 recommended **Explain Before Sign**—decode and
explain a proposed transaction before a user signs it—as an early validation
slice. That recommendation is retained as a useful architecture and trust test,
not as a replacement for the complete Polkagent vision.

The relevant local source locations are:

```text
/Users/will/dev/par/polkagent
  New Polkagent repository and planning corpus.

/Users/will/dev/par/polkadot-chat-agents
  Existing/reference chat-agent product and compatibility source.

/Users/will/dev/nunchi/roko/roko
  Roko source, docs, and iterative architecture material.

/Users/will/dev/par/deep-research-report (17).md
/Users/will/dev/par/deep-research-report (18).md
/Users/will/dev/par/deep-research-report (19).md
  External research inputs. Their relevant conclusions must be summarized
  inline in final research; readers must not be required to open them.
```

Paths are provenance pointers. A research deliverable is not self-contained if
it merely tells the reader to inspect one of them.

### 0.2 Core terms and acronyms

| Term | Meaning in this brief |
|---|---|
| **PRD** | Product requirements document: the normative description of what the product must do, why, for whom, and how success is accepted. |
| **Polkagent** | The proposed Rust-first, Polkadot-native product-engineering and agent platform. |
| **PCA** | `polkadot-chat-agents`, the existing/reference chat-agent product. |
| **Polkadot SDK** | The Rust framework and libraries used to build Polkadot-compatible runtimes, chains, nodes, and related infrastructure. |
| **JAM** | Join-Accumulate Machine, a developing protocol/computation architecture whose tooling and production maturity must be verified rather than assumed. |
| **PVM / PolkaVM** | Polkadot Virtual Machine and its implementation/tooling, used for deterministic program execution and emerging contract/product paths. |
| **XCM** | Cross-Consensus Messaging, the format and execution model for interactions across Polkadot consensus systems. It is not simply a generic token-send API. |
| **Subxt** | A Rust client library for submitting extrinsics and reading state/events from Substrate/Polkadot SDK-based chains. |
| **PAPI** | Polkadot API, a TypeScript client approach used in browser and application integrations. |
| **Product SDK** | Polkadot application/product integration tooling. Exact supported mobile, chat, identity, storage, and signing capabilities require current verification. |
| **W3F** | Web3 Foundation. |
| **Provider** | A connection/service that exposes one or more AI models. |
| **Model** | A particular model identity and capability set, distinct from the provider connection. |
| **Executor** | A component that performs one run/turn and emits normalized events. |
| **Harness** | A long-lived coding-agent or agent-framework process/service with sessions, health, cancellation, tools, and its own lifecycle. |
| **Tool** | A typed operation an agent may invoke under an explicit capability grant. |
| **Skill** | A versioned package of instructions, schemas, examples, tests, context, and declared tool/capability requirements. |
| **Artifact** | Durable, attributable content or evidence such as a file, diff, plan, decoded call, simulation, receipt, or test result. |
| **Event** | An ordered observation that describes lifecycle or streaming activity. |
| **Effect / `EffectIntent` / `EffectAttempt` / `EffectOutcome`** | An effect is actual external work such as calling a model, running a tool, sending, signing, or submitting. `EffectIntent` is the durable command recorded before that I/O. `EffectAttempt` records one claim, lease, retry, and idempotency lifecycle. `EffectOutcome` is the immutable success, failure, timeout, cancellation, or unknown result observed for one attempt. |
| **Grant** | The exact, resolved set of permissions and limits allowed for a run or effect. |
| **MCP** | Model Context Protocol, one possible tool/integration protocol. |
| **ACP / A2A** | Agent Client Protocol / agent-to-agent protocols; interoperability options that require capability negotiation rather than assumed semantic parity. |
| **RPC** | Remote procedure call. |
| **WASM** | WebAssembly, a possible sandboxed extension format. |
| **DTO** | Data transfer object: a versioned wire-level data shape. |
| **JTBD** | Job to be done: the concrete user outcome and situation a product supports. |
| **SLO** | Service-level objective for measurable reliability or performance. |

### 0.3 What “self-contained research” means

Each research package must be understandable by a competent engineer or
product designer who has never read the repositories or earlier reports. It
must:

- Explain the relevant system or protocol before evaluating it.
- Summarize source behavior inline and use links/paths as evidence, not as
  substitutes for explanation.
- Define specialized terms on first use.
- Separate existing behavior from proposed Polkagent behavior.
- State why a finding matters to users and architecture.
- Include examples, diagrams, data shapes, or state transitions when prose
  alone is ambiguous.
- State uncertainty and maturity explicitly.

## 1. Product intent that research must preserve

Polkagent is intended to become a complete, long-term **Polkadot-native agent
product-engineering platform**, not only an Explain Before Sign application.
The platform must make three pillars equally first-class:

1. **Build:** create, test, deploy and operate Polkadot, Polkadot SDK, JAM and
   PVM products, contracts, runtimes, chains/parachains, integrations and agents.
2. **Act:** safely research, prepare, simulate, approve and execute Polkadot
   payments and on-chain work, including long-term policy-bounded autonomous
   agent accounts.
3. **Reach users:** support everything `polkadot-chat-agents` does or intends to
   do—mobile/app chat, encrypted transport, direct coding-agent brains,
   framework harnesses, files, projects, deployment and rich conversation UX—
   with a cleaner, more modular Rust-first architecture.

Explain Before Sign remains a useful early vertical slice and safety proving
ground. It is not the platform’s complete identity. Governance Scout, Builder
Copilot, private assistants, public paid agents, product kits, multi-agent
systems, marketplaces and cloud control-plane capabilities belong in the
long-term design with explicit phasing and release gates.

## 2. Confirmed owner decisions

| Topic | Definitive direction |
|---|---|
| Scope | Complete long-term platform with implementable phases and an intentionally narrow first proof. |
| Central pillars | Product building, on-chain action/payments and PCA/mobile assistant functionality are equally central. |
| Language | Rust for runtime, chain/payment/security logic, CLI, services and SDKs; TypeScript where browser UI or Product SDK integration requires it. |
| Ecosystem | Primarily Polkadot, Polkadot SDK, JAM/PVM and Parity/Web3 Foundation product surfaces. Do not generalize the public model into a generic multichain framework prematurely. |
| PCA compatibility | Preserve all relevant behavior, bridge/plugin compatibility, identity/state/config migration and deployment workflows while replacing internal coupling with better abstractions. |
| Autonomy | Support a configurable ladder from read-only and per-action approval through independently funded, fully autonomous agents. Any supported action family may run without a human in the loop when deliberately configured; raw signing keys still remain outside model/tool contexts and audit/recovery invariants remain active. |
| Deployment | First-class local/self-hosted and managed multi-tenant cloud/control plane; portable state/config/identity with no forced lock-in. |
| Advanced systems | Incorporate Roko-style memory, learning, graphs, groups, feeds/triggers, extensions, marketplace, identity/reputation and evals where they add real value. |
| Marketplace | Permissionless and plural publication/discovery for skills, tools, models, compute, feeds, agent services and product kits. Use safe verified/curated default views, sandboxing and policy-controlled activation without imposing a single central publishing gate. Support configurable platform/protocol/registry/creator fees. |
| Identity | Polkadot-native agent identity/account and optional personhood/reputation/registry integration, while allowing local-only or pseudonymous agents. |
| Custody | Configurable external wallet, hardware, proxy/multisig, local encrypted, organizational, managed KMS/HSM, MPC, programmable-account and funded-agent-account modes. Default to user/external signing for valuable actions while keeping all modes portable and explicit. |
| User surfaces | Preserve and improve PCA-style mobile/chat behavior. Select among Polkadot App/Product integration, responsive web/PWA, desktop and a dedicated native client according to the best achievable UX; do not depend on one experimental host API. |
| Experimental work | Define ambitious JAM/PVM, autonomous learning and agent-economy targets, but label maturity, dependencies, validation spikes and release gates. |
| Deliverable detail | Include Rust traits, schemas, state machines, database/API/config examples, UX flows, cloud tenancy, deployment, acceptance and migration plans. |

## 3. Research quality rules

Every research package must:

1. Prefer current primary sources: official docs/specs, maintained repositories,
   release notes, source code, test vectors and public network evidence.
2. State access date for time-sensitive facts and distinguish:
   **verified**, **inference**, **proposal**, **experimental**, **unknown**.
3. Inspect implementation as well as docs. A forum/RFC/roadmap establishes
   intent, not production support.
4. Identify exact languages, APIs, versions, feature maturity, licenses,
   security assumptions, operational dependencies and testing options.
5. Produce architecture implications and explicit do-now / phase-later / reject
   decisions rather than another broad ecosystem summary.
6. Preserve portability and least privilege. Availability of a protocol does
   not justify placing it in the trusted kernel.
7. Explain enough source context inline that a first-time reader can evaluate
   the recommendation without opening a referenced repository.
8. Identify contradictions between documentation, code, network behavior, and
   roadmap statements.
9. Give every recommendation a user consequence, architecture consequence,
   security consequence, and delivery/maturity consequence.
10. Avoid false equivalence. Similar-looking APIs may have different
    guarantees for streaming, tool calls, signing, finality, privacy, or
    recovery.

### 3.1 Required finding format

Use a consistent record for material findings:

```yaml
finding_id: POLKADOT-TRANSPORT-001
claim: "Short statement that can be accepted or rejected."
status: verified | inference | proposal | experimental | unknown
as_of: 2026-07-29
sources:
  - title: "Primary source title"
    url_or_path: "Exact URL or repository path"
    source_type: code | specification | official_docs | network_observation
source_context: >
  Inline explanation of what the source actually establishes.
user_consequence: "What changes for a user."
architecture_consequence: "Kernel, adapter, UX, or deployment implication."
security_consequence: "New boundary, threat, or required control."
recommendation: do_now | phase_later | spike | reject
acceptance_artifact: "Test, fixture, prototype, study, or decision required."
```

### 3.2 Common context to prepend when a package is researched alone

The following context block makes any package prompt independently usable:

> Polkagent is a proposed Rust-first, Polkadot-native product-engineering and
> agent platform. It has three equal goals: help users build Polkadot SDK,
> contracts/PVM, chain, JAM and application products; safely perform and,
> when explicitly configured, fully automate Polkadot payments and on-chain
> actions; and preserve/improve the private mobile/chat, coding-agent,
> project/file and deployment experience of `polkadot-chat-agents`. It must
> support both local/self-hosted and managed multi-tenant cloud operation
> through portable contracts. Providers, models, executors, harnesses, tools,
> transports, signers, stores and surfaces must remain independently
> extensible. Publication and discovery should support permissionless,
> federated marketplaces with safe default trust views. Use primary sources,
> define terms, distinguish current fact from proposal, and make the report
> self-contained.

## 4. Required research packages

### Package A — Polkadot, Hub and product integration matrix

#### Prompt

> Build a current, source-verified integration map for a Rust-first Polkagent
> platform across Polkadot Hub, Asset Hub/system functionality, OpenGov,
> identity/People, XCM, contracts, EVM compatibility, PVM/PolkaVM, wallets,
> multisig/proxy, Subxt, PAPI, Product SDK, Polkadot App/Products, encrypted
> messaging/Statement Store, Bulletin storage and relevant Parity/W3F product
> surfaces. For each, document production/devnet/experimental maturity, exact
> APIs and repositories, version strategy, signing model, permission boundary,
> test environment, rate/quota/cost, license and integration risk. Recommend
> which belong in the Rust kernel, leaf adapters, TypeScript companion apps,
> product kits or future research. Explicitly verify current App/mobile and
> Statement Store support instead of inheriting PCA assumptions.

#### Required output

- Dated capability/support matrix.
- Rust versus TypeScript integration decisions.
- Chain/network/profile identity model.
- Metadata/runtime-upgrade strategy.
- Signer/wallet compatibility matrix.
- Product/App/mobile go/no-go spikes.

### Package B — JAM/PVM-native agent opportunities

#### Prompt

> Research current JAM and PVM/PolkaVM implementations, specifications,
> developer tools, service model, testing infrastructure, deployment paths and
> public maturity. Determine where an agent product can add genuine value:
> service scaffolding, Refine/Accumulate development, formal/evidence-driven
> testing, PVM contract development, code analysis, simulation, deployment,
> monitoring, payment/compute settlement or agent execution. Compare these
> workflows with conventional Polkadot SDK runtime/contract development.
> Propose clean leaf abstractions that do not force experimental concepts into
> the stable runtime. Identify concrete product kits and strict validation gates.

#### Required output

- Maturity and tooling matrix.
- JAM/PVM product-kit proposals.
- Adapter/trait boundary and artifact types.
- Research-only versus buildable-now decisions.
- Validation and kill criteria.

### Package C — complete PCA compatibility and successor design

#### Prompt

> Audit the complete `polkadot-chat-agents` repository and document every
> behavior and intended capability Polkagent must retain: bot identity and
> registration, Products Devnet/Paseo profiles, encrypted opener/session/device
> transport, ACK/dedup/owed-reply rules, outbound lanes, files/media/storage,
> direct Claude/Codex/OpenCode/custom brains, sessions/resume, model switching,
> reasoning, projects/worktrees, portable tool policy, rich replies, commands,
> T3ams, bridge leases and proactive authority, Hermes/OpenClaw integrations,
> deployment/SSH/Docker, configuration and testing. Design a replacement in
> which transport, conversation, execution, harness lifecycle, files,
> capabilities and deployment are independent versioned ports. Include exact
> state/config/identity import, compatibility routes, behavior contract suites,
> rolling migration and rollback. Propose UX improvements without breaking
> existing consumers.

For a reader unfamiliar with PCA, begin by reconstructing its user journey and
runtime topology in plain language. Explain how a message travels from a mobile
or Polkadot product surface to a local bot process and back; distinguish a
direct coding-agent “brain” from an HTTP/framework bridge; and define why ACK,
deduplication, leases, owed replies, and ordered outbound lanes matter under
crashes and retries.

#### Required output

- Feature-by-feature compatibility matrix.
- Legacy-to-canonical domain mapping.
- Import/export/migration schemas.
- Bridge/OpenAPI compatibility plan.
- Native and compatibility test corpora.
- Direct-runner and framework-harness SDK design.

### Package D — Roko architectural synthesis

#### Prompt

> Inspect the entire Roko repository, docs and tmp corpus. Extract its most
> elegant implemented and specified patterns across Signal/Pulse, Store/Bus,
> Cell/Graph/Flow/Rack, protocol traits, providers/harnesses/tools, gateway,
> memory/learning/dreams, groups, feeds/recipes/triggers, extensions/hooks,
> security/taint/capabilities, telemetry/lenses, auth/payments, marketplace,
> registries, arenas/evals, deployment and orchestration. Distinguish proven
> implementation from speculative design. For each pattern, decide whether
> Polkagent should adapt it directly, simplify it, make it optional or reject
> it. Design a coherent vocabulary that avoids Roko terminology drift while
> retaining fractal composition and rich extensibility.

For a reader unfamiliar with Roko, first explain what problem Roko is trying to
solve and which parts are implemented versus aspirational. Translate Roko terms
such as Signal, Pulse, Cell, Graph, Flow, Rack, Lens, Gateway, Feed, Recipe,
Arena, and Dream into ordinary architecture concepts before deciding whether
Polkagent should use them.

#### Required output

- Exact source-path map.
- Adopt/adapt/defer/reject table.
- Minimal stable kernel vocabulary.
- Optional advanced subsystem architecture.
- Dependency layering and extension-safety rules.
- Novel Roko × Polkadot combinations.

### Package E — agent execution, providers, models and harnesses

#### Prompt

> Design a future-proof execution architecture that supports direct model APIs,
> OpenAI-compatible endpoints, Anthropic-style APIs, local models, routing
> gateways, Codex/Claude/OpenCode-style coding CLIs, ACP/A2A/framework services,
> MCP tools, browser/task executors and custom process/RPC/WASM adapters without
> collapsing their semantics. Research current capability negotiation,
> streaming, structured outputs, tool calls, context limits, reasoning controls,
> sessions/resume, usage/cost, cancellation, retries, fallback and health
> patterns. Define precise Rust contracts and conformance suites. Preserve raw
> capabilities explicitly instead of promising false provider parity.

Show at least three end-to-end examples: a single direct model call, a resumed
coding-harness session operating in a workspace, and a remote/framework agent
that requests a policy-gated tool. Demonstrate which identifiers, events,
artifacts, grants, usage records, and cancellation semantics remain common and
which are adapter-specific.

#### Required output

- Provider/model/executor/harness/tool taxonomy.
- Rust trait and DTO sketches.
- Capability descriptor and negotiation scheme.
- Model catalog/routing/fallback design.
- Harness service lifecycle and sandbox boundary.
- Contract tests and adapter onboarding guide.

### Package F — Polkadot product-engineering platform

#### Prompt

> Research the real workflows required to build Polkadot products, contracts,
> runtimes, chains/parachains, JAM services and PVM applications. Audit current
> tools such as Polkadot SDK templates, Subxt, Pop CLI/MCP, Chopsticks,
> Zombienet, ecosystem tests, runtime upgrade tools, contract toolchains,
> frontends/wallet libraries, deployment and documentation/AI resources. Design
> a product-engineering workbench in which agents can scaffold, edit, test,
> simulate, deploy, monitor and upgrade projects through reviewable artifacts
> and policy-bounded effects. Avoid duplicating good existing tools; make them
> typed adapters/product kits.

Explain each selected tool and workflow for readers who are not already
Polkadot runtime engineers. Include the developer’s starting state, expected
output, local prerequisites, chain/network dependencies, destructive effects,
review points, and recovery path.

#### Required output

- Builder JTBD and workflow map.
- Product-kit catalog and dependency graph.
- Workspace/worktree/artifact/review model.
- Local/testnet/production promotion pipeline.
- Contract/runtime/JAM-specific gates.
- Agent Studio and CLI UX proposal.

### Package G — payments, agent accounts and economic autonomy

#### Prompt

> Research a Polkadot-native payment and autonomous-agent account architecture.
> Cover DOT and Asset Hub stablecoins/assets, fee assets, payment requests,
> invoices, one-shot transfers, allowances/proxies/multisig, smart-account or
> policy-account patterns, XCM settlement, escrow, subscriptions, streaming/
> metered payments, x402, ACP, AP2 concepts, agent-to-agent commerce, receipts,
> reconciliation, refunds/disputes, accounting and finality. Design configurable
> autonomy tiers from propose-only through per-action approval to funded,
> continuously and fully autonomous agent accounts. Defaults should be
> conservative, but an owner must be able to configure any supported action
> family for no-human-in-the-loop execution. Keep keys outside model/harness
> contexts and preserve audit, revocation, recovery, idempotency, and truthful
> finality states at every autonomy level.
> Include compliance questions and operational controls without giving legal
> advice.

#### Required output

- Payment domain/state model and Rust traits.
- Asset/network canonical identity scheme.
- Autonomy/policy tier matrix.
- Signer/key-custody options and threat model.
- Explicit high-autonomy configuration and emergency-control UX.
- Accounting/receipt/reconciliation architecture.
- Marketplace settlement and configurable fee model.
- Phase gates for real-value enablement.

### Package H — identity, personhood, reputation and registries

#### Prompt

> Research how Polkagent agents, people, teams and services should identify and
> authenticate across local, cloud and on-chain contexts. Cover Polkadot accounts,
> People/identity/personhood products, Web3 naming, agent cards, verifiable
> credentials, hardware/user-controlled signing, service identities, API keys,
> OAuth/passkeys, organization tenancy, public registries, reputation and
> attestations. Design opt-in modes from local/pseudonymous to registered and
> reputation-bearing agents. Prevent reputation, display-name and registry
> entries from becoming implicit authorization.

#### Required output

- Identity/credential taxonomy.
- Agent/person/org domain model.
- Authentication and delegation flows.
- On-chain/off-chain registry boundary.
- Reputation evidence and anti-Sybil strategy.
- Privacy, revocation and recovery model.

### Package I — memory, learning, knowledge and multi-agent systems

#### Prompt

> Adapt useful Roko memory, learning, graph, group and evaluation concepts for
> Polkagent. Design durable episodic/semantic/procedural memory with provenance,
> retention, privacy, user controls and tenant/conversation boundaries. Define
> parent/child runs, multi-agent groups and coordination modes with grant
> intersection, budgets, cancellation and evidence. Research evaluation,
> routing feedback and self-improvement mechanisms that cannot modify immutable
> safety gates or silently expand authority. Treat affect/vitality/evolutionary
> ideas as optional experimental modules unless a product need is demonstrated.

#### Required output

- Memory schema, admission/forget/export controls.
- Knowledge/source provenance and citation model.
- Multi-agent/group/run model.
- Learning/evaluation feedback loops.
- Safety invariants and user UX.
- Optional experimental module boundaries.

### Package J — self-hosted plus managed cloud architecture

#### Prompt

> Design equal first-class self-hosted and managed multi-tenant Polkagent
> deployments. Separate data plane, execution plane and control plane; support
> local agents, remote workers, autoscaling, schedules/triggers, queues,
> workspaces, secrets, artifacts, billing, policy, audit, observability,
> deployment updates and regional isolation. Define tenant/org/user/agent
> boundaries, portability/export/import, hybrid operation, end-to-end encryption
> options and failure behavior when the control plane is unavailable. Avoid a
> cloud dependency for correctness of local agents while allowing excellent
> managed UX.

#### Required output

- Deployment topology diagrams.
- Control/data/execution plane contracts.
- Multi-tenancy and authorization model.
- Worker registration/scheduling/recovery protocol.
- State/artifact portability specification.
- Managed billing/SLO/incident model.
- Docker/systemd/Kubernetes/desktop deployment paths.

### Package K — marketplace, extensions and product kits

#### Prompt

> Design a secure marketplace and registry for skills/context packs, tools,
> models/providers, harnesses, compute, feeds, agent services and complete
> product kits. Research manifest/signing/provenance, semantic compatibility,
> dependency resolution, capabilities/data-access disclosure, installation,
> operator approval, sandboxing, updates, revocation, vulnerability response,
> ratings/evaluations and configurable platform fees. Use a permissionless,
> multi-registry publication model: verified and curated views may be excellent
> defaults but must not create a mandatory central publishing gate. Keep
> discovery separate from activation and authorization. Ensure self-hosted
> users can consume, mirror, federate, sideload, or run registries without
> managed-control-plane lock-in.

#### Required output

- Unified package/manifest taxonomy.
- Registry and trust architecture.
- Resolution/install/update/revoke state machines.
- Publisher/operator/user UX.
- Fee, licensing and entitlement model.
- Security/abuse/incident operations.

### Package L — product UX and information architecture

#### Prompt

> Design a top-tier UX spanning first-run setup, local CLI/daemon, browser Agent
> Studio, Agent Inbox, Polkadot mobile chat, approvals, payments, run timelines,
> projects/worktrees, product kits, deployment, cloud organization management,
> marketplace and developer SDKs. Define progressive disclosure so simple bots
> are easy while advanced operators can inspect grants, evidence, costs and
> recovery. Use trusted structured action cards for irreversible effects and
> separate model narrative from canonical state. Include accessibility, mobile
> constraints, notifications, errors, onboarding and migration from PCA.

Compare Polkadot App/Product-hosted mobile UX, responsive web/PWA, desktop
shells, and a dedicated native mobile client. Recommend the smallest set of
first-party surfaces that provides the best user outcome and a robust fallback;
do not assume either that Polkagent must build a native app or that an external
host exposes every required capability.

#### Required output

- Persona/journey maps.
- Information architecture.
- Key wireframes and interaction states.
- Configuration and permission UX.
- Failure/recovery/support UX.
- Local/cloud/mobile consistency rules.

### Package M — evaluation, security and production assurance

#### Prompt

> Build a complete assurance model for Polkagent: deterministic state-machine
> tests, port contract suites, model/provider evals, tool/harness sandbox tests,
> prompt-injection and confused-deputy attacks, signer/payment red-teaming,
> metadata/runtime drift, supply-chain security, reproducible releases,
> penetration testing, fault injection, backups/restores, tenancy isolation,
> availability/SLOs and incident response. Incorporate Roko-style arenas where
> useful, but keep evaluation outputs from directly granting authority.

#### Required output

- Threat model and trust-boundary diagrams.
- Test/eval pyramid and fixture corpora.
- CI/release gates.
- Red-team scenarios.
- Cloud/self-hosted SLO and runbook requirements.
- Security maturity roadmap.

## 5. Cross-package synthesis questions

The final research synthesis must resolve:

1. What belongs in the minimal stable kernel versus optional subsystem or
   adapter?
2. What is the canonical vocabulary for agent, run, event, artifact, effect,
   graph/workflow, capability, policy, approval, signer, payment and identity?
3. Which Roko abstractions improve composability and which create unnecessary
   indirection for common product flows?
4. Which PCA compatibility behaviors are immutable external contracts and
   which legacy implementation choices should disappear?
5. How can local/self-hosted and managed cloud use identical public contracts
   without compromising portability or tenant isolation?
6. How can funded, fully autonomous agents operate without granting models raw
   keys or discarding signer isolation, audit, revocation, recovery, and
   truthful outcome semantics? How should the product explain deliberately
   broad or unlimited owner-selected policies?
7. How should Polkadot SDK/JAM/PVM product building reuse existing tools instead
   of reimplementing them?
8. What is the smallest elegant extension system that can grow into a safe
   marketplace without destabilizing the kernel?
9. Which product capabilities are available now, which require spikes and
   which are long-term research?
10. What sequence proves the architecture while keeping all three product
    pillars equally represented in the long-term platform?
11. Which integrity properties remain invariant even when custody, approval,
    publication, deployment, and autonomy are configured flexibly?
12. How should a permissionless marketplace provide safe default discovery,
    previews, isolation, provenance, updates, and incident response without
    becoming a central gatekeeper?
13. Which combination of Polkadot mobile/app, responsive web, desktop, CLI, and
    dedicated native client provides the best UX at each maturity phase?
14. How will every local deployment and managed-cloud deployment prove
    portability and semantic compatibility?

## 6. Expected final PRD suite

Research should produce enough evidence for an authoritative suite covering:

1. Vision, principles, personas and product pillars.
2. Vocabulary, invariants and system architecture.
3. Agent/run/effect/graph execution model.
4. Providers, models, harnesses, tools and skills.
5. Polkadot chain, SDK, JAM/PVM and product-building integrations.
6. PCA compatibility, chat/mobile and messaging.
7. Identity, accounts, signers, policy and security.
8. Payments, autonomous agents and economic controls.
9. Memory, knowledge, learning, multi-agent groups and evals.
10. Data, artifacts, events, observability and recovery.
11. Self-hosting, managed cloud and multi-tenancy.
12. Marketplace, registry, extension SDK and product kits.
13. UX, CLI, Studio, Inbox, mobile and operator surfaces.
14. APIs, schemas, configuration and migration.
15. Testing, security assurance, roadmap and acceptance criteria.

### 6.1 Minimum content of every definitive PRD

Every final PRD must include:

1. A plain-language purpose and user outcome.
2. Definitions for domain terms introduced in that document.
3. Personas and jobs affected.
4. Scope, non-goals, defaults, configuration choices, and maturity.
5. User journeys, including success, denial, cancellation, failure, unknown,
   recovery, and migration states.
6. Functional and non-functional requirements with stable IDs.
7. Architecture boundaries and dependencies.
8. Rust traits/types and wire/schema examples where relevant.
9. Security, privacy, tenancy, custody, and abuse implications.
10. Observability and operator/support requirements.
11. Acceptance tests, product evidence, and release gates.
12. Cross-document interfaces summarized inline; a reader must not need a
    second PRD merely to understand the first.

### 6.2 Research synthesis artifact set

The final research pass should produce:

```text
Executive synthesis
  Major findings, decisions changed, and remaining blockers.

Current-capability matrix
  Feature/API → maturity → source → platform implication → next proof.

Architecture decision proposals
  Context → options → decision → consequences → validation.

Product opportunity portfolio
  User/job → solution → required platform capabilities → evidence → phase.

Risk and uncertainty register
  Risk → likelihood/impact → mitigation → owner → evidence deadline.

Traceability map
  Research finding → owner decision → PRD requirement → ADR → test/evidence.
```

## 7. Interpretation of report 19

Report 19 correctly emphasizes that the current repository is a design corpus,
not proof of a live implementation; that durable effects, grants, metadata and
signer separation are strong foundations; and that early implementation needs
an evidence-bearing vertical slice. Its recommendation to start with Explain
Before Sign is retained as a **phase-one validation strategy**.

It does not override the owner’s clarified long-term intent: product building,
payments/on-chain autonomy and complete PCA/mobile functionality are equally
central platform pillars. The definitive PRDs should therefore separate:

- **north-star platform architecture** — complete, modular and extensible;
- **release sequence** — risk-ordered, evidence-gated and implementable; and
- **maturity labels** — current, planned, experimental or research-only.

## 8. Research execution order and dependencies

The packages are intentionally separable, but their results should be
synthesized in this order:

1. **Ground truth:** A (Polkadot), B (JAM/PVM), C (PCA), and D (Roko).
2. **Stable execution model:** E (providers/harnesses) and M (assurance).
3. **User value:** F (product engineering), G (payments/autonomy), H
   (identity), and L (UX).
4. **Advanced platform:** I (memory/groups), J (cloud), and K
   (marketplace/extensions).
5. **Cross-package architecture:** resolve vocabulary, trust boundaries,
   maturity, sequencing, and traceability.

Research can run in parallel, but no package may invent a conflicting
definition of agent, run, effect, grant, account, identity, package, or
deployment. When a conflict is found, record it rather than silently choosing
different meanings.

### 8.1 Package status as of 2026-07-29

“First pass complete” means a self-contained research dossier exists. It does
not mean its implementation spikes, user evidence, security review, or
production gates have passed.

| Package | Current status | Inline artifact/result | Important remaining proof | Next owner |
|---|---|---|---|---|
| A — Polkadot/product matrix | First pass complete | `research-polkadot-jam-products.md` contains a dated primer, maturity matrix, API roles, flows, sources, risks, and spikes. | Live network/profile checks, mobile/Product support matrix, signer and XCM fixtures | Unassigned |
| B — JAM/PVM | First pass complete as horizon research | The same dossier distinguishes PVM contract paths from prospective JAM services and gives engagement gates. | Current toolchain/network PoCs, stable service APIs, concrete user workflow | Unassigned |
| C — PCA successor | First pass complete | `research-pca-cloud-compat.md` reconstructs PCA, C0–C3, bridge schemas, state/migration, Rust modules, cloud topology, and acceptance. | Commit-pinned executable fixtures, real device interop, import rehearsal | Unassigned |
| D — Roko synthesis | First pass complete | `research-roko-definitive.md` explains Roko, source provenance, patterns, adaptation, tradeoffs, and requirements. | Validate selected code behaviors during implementation; accepted ADRs | Unassigned |
| E — execution/providers/harnesses | Partial | PCA and Roko dossiers define taxonomy, lifecycles, normalized events, grants, and adapter direction; `reserach/research2.md` adds candidate containment and local-store evidence gates. | Provider/model capability survey, concrete Rust DTOs, load/recovery and sandbox conformance suites | Unassigned |
| F — product engineering | Partial opportunity map | Baseline and `reserach/research1.md` explain builder personas and candidate workflows; `reserach/research2.md` refines A1 migration and A7 metadata-repair proving fixtures. | Builder interviews and reproducible toolchain spikes for runtime, contract/PVM, XCM, metadata, and deployment jobs | Unassigned |
| G — payments/autonomy | Partial architecture direction | Baseline defines custody/autonomy modes; `reserach/research2.md` adds profile-gated proxy, asset, payment-standard, signer, and evidence hypotheses. | Block-pinned rail/asset proof, signer/custody conformance, reconciliation, fraud, legal and operational gates | Unassigned |
| H — identity/reputation | Partial | Baseline and Polkadot dossier define optional local/on-chain identity and non-authorization semantics. | Current product APIs, credential/delegation/recovery flows, privacy and anti-Sybil evidence | Unassigned |
| I — memory/multi-agent | Partial from Roko | Roko dossier defines provenance, promotion, groups, feeds, triggers, and authority limits. | Canonical schemas, UX, eval design, tenant/group isolation tests | Unassigned |
| J — cloud | Partial architecture | PCA/cloud dossier defines portable data plane, optional-per-deployment control plane, tenancy and C3 gates. | Managed-worker protocol, storage/queue/key isolation, SLOs, billing, portability drill | Unassigned |
| K — marketplace/extensions | Direction confirmed; detailed research pending | Baseline establishes permissionless plural publishing with safe default discovery; Roko dossier supplies trust-tier lessons. | Manifest/resolver schema, registry federation, sandboxing, commercial/abuse/incident model | Unassigned |
| L — UX | Partial | Baseline and PCA dossier include personas, journeys, Studio/Inbox/mobile/operator flows and action-sheet requirements; `reserach/research2.md` proposes adversarial approval-card study cases. | Comparative surface study, wireframes, accessibility, pre-registered comprehension study and migration usability tests | Unassigned |
| M — assurance | Partial | All dossiers carry threats, fixtures, gates, and failure semantics; `reserach/research2.md` adds a requirement-level evidence ledger across its 20 prompt areas. | Execute the unified threat model, fixtures, red-team, recovery drills, signer tests, SLO/incident and release gates | Unassigned |

The paths in this table are provenance pointers. The substantive context
needed to understand each package appears in this brief and in the named
self-contained artifact; future synthesis must still copy required interface
and user context into the definitive PRD that owns it.

`reserach/research2.md` is a dated, non-normative desk-research synthesis, not
proof that its 20 prompt success criteria were executed. It contributes a
prompt-to-decision map, primary-source links, maturity corrections, a
three-pillar proving sequence, and a claim-to-fixture evidence ledger. Its
recommendations do not override this brief or
`01-ESTABLISHED-BASELINE.md`. A package remains “partial” until the named
live-chain query, code spike, compatibility run, user study, benchmark,
security exercise, recovery drill, or specialist review exists as a durable
artifact.

## 9. Definition of research complete

The research phase is complete only when:

- Every material time-sensitive claim has a current primary source and access
  date.
- Existing behavior, owner decision, inference, proposal, experiment, and
  unknown are visibly distinct.
- Every repository path is accompanied by an inline behavior summary.
- PCA external behavior and migration requirements are complete enough to
  build fixtures.
- Roko patterns have explicit adopt/adapt/defer/reject decisions.
- Every Polkadot/JAM/PVM integration has a maturity and responsibility
  boundary.
- Provider, model, executor, harness, tool, signer, transport, store, and
  surface contracts do not collapse into one abstraction.
- Local and cloud, approval and autonomy, curated views and permissionless
  publishing, and mobile hosts and fallback clients are coherently reconciled.
- Each recommended feature maps to a user job, architecture dependency,
  security boundary, acceptance artifact, and delivery phase.
- The resulting PRDs can be read independently by a first-time reader.
