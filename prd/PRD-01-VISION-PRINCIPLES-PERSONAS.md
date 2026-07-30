# PRD-01: Vision, Principles, Personas and Product Pillars

**Status:** definitive
**Version:** 1.0
**Date:** 2026-07-30
**Authority:** owner-confirmed; supersedes iterative planning documents
**Audience:** engineers, designers, product managers, investors, and partners with no prior Polkagent context
**Implementation status:** normative product requirements; not a claim of running production code

---

## Table of contents

1. [Purpose and scope](#1-purpose-and-scope)
2. [Product vision](#2-product-vision)
3. [Three equal pillars](#3-three-equal-pillars)
4. [Core principles](#4-core-principles)
5. [Design philosophy](#5-design-philosophy)
6. [Confirmed owner decisions](#6-confirmed-owner-decisions)
7. [Target personas](#7-target-personas)
8. [Jobs to be done (JTBD)](#8-jobs-to-be-done)
9. [Product opportunity portfolio](#9-product-opportunity-portfolio)
10. [Non-goals and exclusions](#10-non-goals-and-exclusions)
11. [Maturity labels](#11-maturity-labels)
12. [Success metrics and KPIs](#12-success-metrics-and-kpis)
13. [Complete glossary](#13-complete-glossary)
14. [Cross-document references](#14-cross-document-references)
15. [Acceptance criteria checklist](#15-acceptance-criteria-checklist)

---

## 1. Purpose and scope

### 1.1 What this document is

PRD-01 is the normative foundation of the Polkagent product. It defines:

- The long-term product vision and the three equal pillars that compose it.
- The principles and design philosophy that govern every subsequent PRD.
- The personas who use the platform and the jobs they need done.
- The opportunity portfolio that connects user jobs to platform capabilities.
- The maturity framework that distinguishes shipped behavior from experimental research.
- The vocabulary and definitions that all PRDs share.

Every subsequent PRD (02 through 15) must be consistent with this document. When a later PRD introduces a capability, persona extension, or vocabulary refinement, it must trace back to a section in PRD-01 or explicitly propose an amendment.

### 1.2 What this document is not

- It is not an implementation specification. Rust traits, schemas, wire formats, and deployment topologies appear in PRDs 02-15.
- It is not a schedule. Release sequence, phase gates, and delivery milestones appear in PRD-15.
- It is not a claim that the platform exists. Polkagent is currently a researched design corpus. The repository contains planning material, research dossiers, and architecture proposals.

### 1.3 How to read this document

Sections are designed to be read in order for a newcomer, or individually by reference for a domain expert. Each section ends with acceptance criteria that must be satisfied before the PRD is considered complete. Terms are defined on first use and collected in the glossary (Section 13). Maturity labels from Section 11 apply throughout.

### 1.4 Source authority

This PRD synthesizes and supersedes the following research inputs:

| Source | Role |
|---|---|
| `00-DEEP-RESEARCH-BRIEF.md` | Research brief with owner decisions, vocabulary, and quality rules |
| `01-ESTABLISHED-BASELINE.md` | Confirmed baseline with all established directions |
| `02-SOURCE-COVERAGE-CHECKLIST.md` | Source coverage and traceability checklist |
| `research-polkadot-jam-products.md` | Polkadot/JAM/PVM ecosystem research |
| `research-pca-cloud-compat.md` | PCA compatibility, cloud, and successor architecture |
| `research-roko-definitive.md` | Roko pattern synthesis |
| `reserach/research1.md` | Opportunity catalog across Build, Act, Reach |
| `reserach/research2.md` | Consolidated decision-grade research across 20 follow-on prompts |
| `reserach/research3.md` | Targeted research on first workflow, safety architecture, autonomy dial, local-first state, persona focus, and staged recommendations |

Where these sources conflict, `01-ESTABLISHED-BASELINE.md` is authoritative for product direction, and this PRD is the definitive normative expression of that direction.

### 1.5 Polkadot context for a newcomer

Polkadot is an ecosystem for interoperable blockchains, shared security, applications, assets, governance, and emerging general-purpose computation. Polkagent must understand several related but distinct surfaces:

- **Polkadot SDK** is the Rust framework and library collection used to build compatible runtimes, nodes, chains, and system logic.
- A **runtime** defines a chain's state-transition logic. A **pallet** is a modular runtime component.
- **Polkadot Hub and system-chain functionality** expose assets, applications, governance, identity, messaging, and other ecosystem capabilities.
- **XCM** (Cross-Consensus Messaging) describes interactions across consensus systems. It involves locations, versions, fees, execution, and outcome uncertainty; it is not merely a generic token-send call.
- **Subxt** is a Rust library for reading state, submitting extrinsics, and following events against Polkadot SDK-based chains.
- **PAPI** (Polkadot API) is a TypeScript-oriented client option useful in browser and application integrations.
- **PVM/PolkaVM** is a RISC-V-based virtual machine with emerging product and contract uses.
- **JAM** (Join-Accumulate Machine) is a developing computation protocol and service model. Polkagent treats it as strategic and experimental until APIs, tooling, networks, and operational guarantees are verified.
- **Finality** is durable chain agreement that an included action will not be reverted under the protocol's normal model. Signing, broadcasting, and block inclusion are not the same as finality.

Polkagent's purpose is not to make these distinctions disappear. It should translate them into understandable user choices while retaining precise typed evidence internally.

### 1.6 Existing codebases

Two existing projects provide important context:

- **`polkadot-chat-agents` (PCA)** is the reference product Polkagent must preserve and improve. It lets users interact with coding agents and other agent runtimes through Polkadot-oriented private/mobile chat. It already contains important behavior around bot identity, message polling, durable-before-acknowledgement processing, direct coding-agent processes, framework bridges, project files, live replies, configuration, and deployment. Polkagent should reproduce compatible user-visible behavior through cleaner versioned abstractions rather than copying PCA's internal implementation.
- **Roko** is a separate, broad agent-platform design and implementation project. It contains useful patterns for durable artifacts and events, effectful execution, narrow adapter interfaces, policy/capability resolution, context and memory, graphs, groups, feeds/triggers, extensions, evaluation, telemetry, and platform operations. Polkagent should select the strongest patterns and simplify or reject abstractions that do not serve concrete Polkadot product workflows.

Neither is a drop-in dependency or evidence that the proposed platform is already implemented.

**Section 1 acceptance criteria:**
- [ ] A newcomer can understand what Polkagent is, what this document covers, and where to find deeper material after reading Section 1 alone.
- [ ] Source authority is explicit and traceable.
- [ ] No claim of existing implementation is made.

---

## 2. Product vision

### 2.1 Vision statement

> **Polkagent is a Rust-first, Polkadot-native product-engineering and agent platform for building, using, publishing, operating, and paying agents that understand Polkadot and can safely perform Polkadot work.**

### 2.2 Long-term vision narrative

Polkagent envisions a world where every meaningful Polkadot workflow -- from scaffolding a parachain runtime to executing a cross-chain payment to governing a treasury -- can be performed, reviewed, and automated through intelligent agents that are safe, transparent, and accountable.

The platform exists at the intersection of three capabilities that no existing tool combines:

1. **Deep Polkadot understanding.** Agents that can decode runtime metadata, interpret XCM instructions, simulate storage migrations, analyze governance proposals, and construct typed chain interactions -- not generic chatbots with blockchain keywords appended.

2. **Evidence-bearing safety.** Every material action produces durable, attributable evidence: decoded call data, policy evaluations, simulation results, signing records, finality observations, and receipts. Agent prose is explanatory; canonical data is authoritative. A model's text that "the transfer succeeded" is never treated as proof.

3. **Configurable autonomy.** A spectrum from read-only research through per-action human approval to fully autonomous operation under policy-bounded mandates. The owner controls where on this spectrum each action family sits. Raw signing keys never enter model or tool contexts. Audit, revocation, and recovery invariants remain active at every autonomy level.

### 2.2a First workflow: the treasury/governance operator path

Research confirms the highest-value, highest-safety-leverage first workflow is the treasury/governance operator sequence:

```text
research → explain → simulate (DryRunApi) → intent + evidence
  → approve → sign (Vault/Ledger) → submit → finality → report
```

This workflow is the primary proof of value for the platform because it exercises every safety primitive end-to-end: DryRunApi simulation, calldata decode, evidence card generation, human approval gate, isolated signing, finality observation, and durable receipt. Getting this workflow right for the treasury/governance operator persona (P5) is the product's first concrete goal.

**Time-to-first-value target:** A new user should be able to connect a wallet in read-only mode, see their delegations and treasury exposure, and receive one safe suggested action within minutes. This "connect wallet → observe → one suggested action" path is the entry point before any write authority is established.

**Onboarding risk and mitigation:** The primary onboarding risk is friction for non-technical governance operators. Mitigation: a first-run wizard and a read-only "shadow mode" that demonstrates the full workflow against real chain data without any signing or submission capability, allowing operators to verify the platform's behavior before granting write authority.

### 2.3 What makes Polkagent different

Polkagent is not a generic AI agent framework that happens to connect to a blockchain. It is not a wallet with a chatbot. It is not a DevOps pipeline with AI sprinkled on top. Its differentiator is the combination of:

- **Polkadot-native domain types.** Chain profiles, runtime metadata, XCM locations, pallet calls, governance tracks, asset identities, proxy relationships, and finality states are first-class typed concepts, not string-labeled generic parameters.
- **Three equal pillars.** Building Polkadot products, performing on-chain actions, and reaching users through mobile/chat are all first-class. The platform is not defined by one use case.
- **Reusable safety primitives.** Evidence-bearing effects, canonical action cards, metadata-pinned chain interpretation, grants outside model control, durable outboxes, and portable product kits serve all three pillars through the same architecture.
- **Portable operation.** The same agent definition, configuration, and state run locally, self-hosted, or in managed cloud without forced lock-in.
- **Permissionless extensibility.** Skills, tools, models, harnesses, feeds, agent services, and product kits can be published, discovered, and installed without a central gate -- with safe verified/curated default discovery views.

### 2.4 The reusable product thesis

The platform's core value proposition is a small set of reusable primitives:

```text
request/trigger -> durable run -> evidence + policy -> proposed intent/effect
  -> approval or configured mandate -> isolated signer/worker -> receipt/outcome
  -> truthful projection in CLI, web, and optional PCA-compatible chat
```

An action card is authoritative only when it is rendered from canonical, metadata-bound data. Model prose is explanatory and must be visibly separate.

**Evidence-based safety is the product**, not a feature bolted onto it. Every action ships with a chain-derived evidence card: DryRunApi simulation output, decoded calldata bound to the relevant runtime metadata, policy evaluation record, and finality observation. The enabling primitives are DryRunApi (for preflight simulation) and calldata decode (for canonical human-readable interpretation). A user who can read an evidence card and verify it against chain data has the same information as a sophisticated on-chain operator -- that is the product's core promise.

### 2.5 Product outcome -- the long-term user journey

```text
idea or recurring job
  -> choose a safe starter or describe the desired outcome
  -> create/version an AgentSpec or ProductSpec
  -> attach models, harnesses, tools, skills, data, chain profiles, and policy
  -> test with fixtures, simulations, evals, and a preview environment
  -> inspect permissions, custody, costs, and expected effects
  -> publish locally, to a team, to managed cloud, or to a registry/marketplace
  -> use it through chat, mobile, web, CLI, API, feeds, or triggers
  -> approve actions or enable policy-driven autonomy
  -> observe evidence, receipts, costs, finality, failures, and recovery
  -> export, migrate, update, pause, revoke, or delete it
```

The simple path should feel like configuring a polished product, not assembling an agent framework. Advanced users must still be able to replace every major adapter and inspect every meaningful decision.

### 2.6 Three illustrative end-to-end experiences

#### Builder Copilot

1. A developer opens an existing Polkadot SDK repository in the local CLI or Studio.
2. Polkagent detects the workspace type, toolchain, network profiles, and available product kits; the user confirms a constrained workspace grant.
3. A selected coding harness analyzes the repository and emits a plan artifact.
4. Tool effects create a worktree, edit files, compile, test, benchmark, and capture logs as evidence.
5. The user reviews code, test evidence, migration/metadata changes, and a deployment preview.
6. Depending on policy, deployment is prepared for approval or performed autonomously in an allowed environment.
7. The entire run can be resumed through CLI, web, or mobile Inbox without losing project or effect state.

#### Mobile Polkadot assistant

1. A user messages a private agent through the best supported Polkadot mobile/app surface.
2. The transport authenticates the sender, durably admits the message, and acknowledges it only after recoverable state exists.
3. The agent reads permitted chain data and returns live progress.
4. For an action, the user receives a canonical approval sheet in a surface capable of showing complete evidence.
5. The selected wallet/signer confirms the exact payload.
6. Polkagent reports separately that it was signed, submitted, included, and finalized -- or that the outcome is unknown.
7. Web remains available for richer evidence, configuration, and recovery.

#### Fully autonomous paid service

1. A publisher packages a monitored service as an agent/product kit and publishes it through one or more permissionless registries.
2. A customer or organization installs a pinned version, reviews permissions, selects providers and compute, configures payment terms, and chooses a signer/custody mode.
3. The owner grants the deployed agent continuous authority over specified accounts/action families. The mandate may be narrow or deliberately broad.
4. Feeds or chain events trigger runs. The agent quotes or charges, performs work, settles payments, and issues receipts without routine human approval.
5. Deterministic policy, isolated signing, idempotent effects, budgets if configured, monitoring, circuit breakers, revocation, reconciliation, and audit remain active.
6. The owner can export or move the agent between managed and self-hosted operation.

**Section 2 acceptance criteria:**
- [ ] The vision statement can be quoted in a single sentence.
- [ ] The narrative distinguishes Polkagent from generic agent frameworks, wallets, and DevOps tools.
- [ ] Three illustrative experiences demonstrate each pillar.
- [ ] The reusable product thesis is stated and explained.
- [ ] No implementation detail appears; those belong in PRDs 02-15.

---

## 3. Three equal pillars

### 3.1 Pillar overview

Polkagent's product surface is organized into three pillars of equal importance. "Equal" means that no pillar is a secondary add-on or a future-only aspiration. Each pillar has its own user base, value proposition, architecture requirements, and release gates. The platform kernel serves all three.

```text
+------------------------------------------------------------------+
|                        POLKAGENT PLATFORM                         |
+------------------------------------------------------------------+
|                                                                    |
|   +------------------+  +------------------+  +------------------+ |
|   |      BUILD       |  |       ACT        |  |      REACH       | |
|   |                  |  |                  |  |                  | |
|   | Create, test,    |  | Research, prep,  |  | Mobile/app chat, | |
|   | deploy, operate  |  | simulate, approve|  | encrypted        | |
|   | Polkadot products|  | and execute on-  |  | transport, coding| |
|   | contracts, runtim|  | chain work and   |  | agents, projects,| |
|   | chains, agents   |  | payments         |  | deployment, UX   | |
|   +------------------+  +------------------+  +------------------+ |
|                                                                    |
|   +----------------------------------------------------------+    |
|   |              SHARED PLATFORM KERNEL                        |   |
|   | Durable runs, effects, artifacts, grants, policy, events, |   |
|   | identity, storage, observability, extensions, marketplace  |   |
|   +----------------------------------------------------------+    |
+------------------------------------------------------------------+
```

### 3.2 Pillar 1: Build

**Purpose:** Help individuals and teams create, test, deploy, and operate Polkadot products.

**Scope of "products":**

| Product type | Examples | Maturity |
|---|---|---|
| Polkadot SDK runtimes and pallets | Custom pallets, runtime configurations, storage migrations | Established |
| Smart contracts | ink! contracts, Solidity/Revive contracts, PVM programs | Established (ink!), Experimental (PVM) |
| Parachains and system chains | Chain specifications, coretime, genesis, upgrades | Established |
| Applications and frontends | dApps, wallets, dashboards, indexers | Established |
| XCM integrations | Cross-chain messaging, asset transfers, remote execution | Established |
| JAM services | Refine/Accumulate services, PVM programs | Experimental |
| Agents and product kits | Polkagent agents, skills, tools, product bundles | Established |
| Infrastructure | Validators, collators, RPC nodes, monitoring | Phased |

**What Build enables:**

- Explore and understand Polkadot SDK, runtime, pallet, XCM, contract, PVM, JAM, governance, wallet, indexer, and application code.
- Create project plans, architecture, code, tests, migrations, deployment artifacts, documentation, threat models, and operational runbooks.
- Run direct model APIs or coding harnesses such as Codex-, Claude Code-, OpenCode-, ACP-, A2A-, or custom-compatible executors.
- Maintain durable projects and workspaces with explicit roots, branches, sessions, files, artifacts, and tool grants.
- Compile, test, lint, simulate, benchmark, launch local networks, deploy to development environments, and prepare reviewed production changes.
- Inspect runtime metadata and upgrades, storage changes, contract behavior, chain state, governance activity, fees, and cross-chain effects.
- Turn repeated workflows into versioned skills, agents, product kits, feeds, monitors, and services.
- Use the same runtime for a local coding assistant, a team automation, a public service, or a policy-bounded autonomous operator.

**Build safety rule:** Generated outputs are attributable artifacts. Agent prose is not treated as proof that code compiled, a test passed, a deployment occurred, or chain state changed.

### 3.3 Pillar 2: Act

**Purpose:** Safely research, prepare, simulate, approve, and execute Polkadot payments and on-chain work.

**Complete action lifecycle:**

```text
request or trigger
  -> typed intent
  -> chain/network/account resolution
  -> canonical decode and metadata evidence
  -> read-only evidence
  -> fee/preflight/simulation evidence where available
  -> policy evaluation
  -> optional human, quorum, or external-system approval
  -> exact-payload signing
  -> submission/broadcast
  -> inclusion/finality or explicit unknown state
  -> reconciliation, receipt, and durable audit trail
```

**Supported action families** (each requires its own decoder, evidence policy, simulation capabilities, fixtures, risk model, and acceptance gates):

| Action family | Description | Maturity |
|---|---|---|
| Native DOT transfers | Simple balance transfers on relay chain and system chains | Established |
| Asset transfers | Stablecoins, fungible and non-fungible assets on Asset Hub | Established |
| XCM operations | Cross-chain asset transfers, remote execution, reserve operations | Phased |
| Contract/PVM calls | Smart contract interactions and PVM program execution | Phased |
| Identity operations | Setting identity, requesting judgments, managing sub-identities | Phased |
| Proxy/multisig | Adding/removing proxies, creating multisig calls, approving | Phased |
| Staking/nomination | Nominating validators, bonding, unbonding, claiming rewards | Phased |
| Governance | Voting on referenda, delegating, submitting proposals | Phased |
| Treasury | Submitting treasury proposals, tips, bounties | Phased |
| Recurring/metered payments | Scheduled transfers, streaming payments, subscriptions | Gated |
| Agent-to-agent commerce | Quoting, settling, receipting between autonomous agents | Research |
| Escrow/dispute | Contract-mediated conditional settlement | Research |

**Act safety rule:** A generic `send_transaction` tool is not an adequate safety boundary. Each action family must have its own decoder, evidence policy, simulation, fixtures, risk model, and acceptance gates. A model sentence such as "the transfer succeeded" is never authoritative.

**Autonomy model:**

| Level | Description | Default |
|---|---|---|
| Read-only | Agent can query chain state and produce reports; no write effects | Yes (new agents) |
| Propose-only | Agent can draft intents; all require explicit human approval | Yes (new action families) |
| Per-action approval | Each action requires human review of the canonical card | Configurable |
| Policy-bounded autonomy | Actions within defined budgets, allowlists, and limits execute automatically | Configurable |
| Full autonomy | All supported actions execute under the owner's configured mandate | Configurable (requires explicit setup) |

Any supported action family may run without a human in the loop when deliberately configured. Raw signing keys still remain outside model/tool contexts, and audit/recovery invariants remain active at every autonomy level.

### 3.4 Pillar 3: Reach

**Purpose:** Support everything `polkadot-chat-agents` does or intends to do -- mobile/app chat, encrypted transport, direct coding-agent brains, framework harnesses, files, projects, deployment, and rich conversation UX -- with a cleaner, more modular Rust-first architecture.

**PCA capabilities that Polkagent must preserve or improve:**

| Capability | PCA behavior | Polkagent successor |
|---|---|---|
| Bot identity and registration | Products Devnet/Paseo profiles, account binding | Versioned agent identity with optional on-chain binding |
| Encrypted transport | Statement Store + app encryption/sessions/devices | Transport port with PCA-compatible adapter and native hardening path |
| Message admission | Durable-before-acknowledgement, ACK/dedup/owed-reply rules | Durable inbound with ordered outbound lanes |
| Direct coding agents | Claude, Codex, OpenCode process runners | Executor/harness port with normalized events |
| Framework bridges | HTTP/API bridges to external agent services | Harness adapter port |
| Files and media | Project files, HOP/Bulletin attachment delivery | Artifact/file store with transport-specific delivery |
| Live replies and progress | Streaming partial responses, status updates | Event/projection port with surface-specific rendering |
| Projects and worktrees | Repository-aware sessions with branch management | Workspace/project model with explicit roots and grants |
| Model/harness selection | Per-conversation model choice | Agent/run-level execution route configuration |
| Configuration | Bot config files, environment, deployment profiles | Versioned declarative configuration with safe defaults |
| Deployment | SSH, Docker, remote management | Deployment port with local/cloud/container targets |
| T3ams | Team collaboration through chat | Group/organization model (Phased) |

**Surface strategy:**

Polkagent must provide the best UX through the best available surface for each job:

| Surface | Primary use | Maturity |
|---|---|---|
| CLI/TUI | Builders, operators, automation | Established |
| Web/PWA (Agent Studio) | Rich configuration, timelines, artifacts, marketplace | Established |
| Agent Inbox (web) | Approvals, receipts, run status, notifications | Established |
| Polkadot mobile/app chat | PCA-compatible mobile assistant | Phased (dependent on app API maturity) |
| API/SDK | Programmatic access, integrations, CI/CD | Established |
| Desktop shell | Enhanced local development experience | Phased |
| Native mobile client | Dedicated Polkagent mobile app | Phased (if app integration insufficient) |

**Reach safety rule:** Do not make the entire product wait on one experimental mobile or messaging API. A user must have a high-quality fallback surface at every phase.

### 3.5 Pillar interaction

The three pillars are not isolated products. They share:

- **The same kernel.** Durable runs, effects, artifacts, grants, policy, events, and projections.
- **The same safety model.** Evidence-bearing effects, canonical action cards, isolated signing, and grant-controlled tools.
- **The same extension system.** Skills, tools, models, harnesses, and product kits work across pillars.
- **The same deployment model.** Local, self-hosted, and managed cloud with portable state.
- **The same surfaces.** CLI, web, mobile, and API serve all three pillars.

A builder who writes a runtime upgrade also needs Act to deploy it and Reach to coordinate with a team. A DOT holder who uses Act for payments also benefits from Build's governance research tools and Reach's mobile inbox. The architecture must not create pillar-specific silos.

**Section 3 acceptance criteria:**
- [ ] Each pillar has a clear purpose statement and scope.
- [ ] Supported capabilities are listed with maturity labels.
- [ ] Safety rules are stated for each pillar.
- [ ] The autonomy model for Act is described with levels and defaults.
- [ ] PCA compatibility requirements for Reach are enumerated.
- [ ] Pillar interaction and shared infrastructure are explained.

---

## 4. Core principles

### 4.1 Principle index

| ID | Principle | Status |
|---|---|---|
| P-01 | Rust-first, not Rust-only | Established |
| P-02 | Polkadot-native depth before generic breadth | Established |
| P-03 | Local and cloud are equal product modes | Established |
| P-04 | Evidence over assertion | Established |
| P-05 | Configurable autonomy with invariant safety | Established |
| P-06 | Extensibility without kernel bloat | Established |
| P-07 | Progressive disclosure and excellent UX | Established |
| P-08 | Portable state and no forced lock-in | Established |
| P-09 | Permissionless extension with safe defaults | Established |
| P-10 | Separation of narrative from authority | Established |
| P-11 | Explicit maturity and honest uncertainty | Established |

### 4.2 P-01: Rust-first, not Rust-only

**Status:** Established

Rust is the primary language for the runtime, domain model, execution engine, policy, security, chain and payment logic, CLI, services, storage adapters, extension host, and native SDKs. TypeScript is appropriate for browser UI, Polkadot Product SDK/PAPI integration, generated clients, and ecosystems where it materially improves compatibility or user experience. Language boundaries use versioned protocols and schemas rather than shared internal state.

**Rationale:** Rust provides memory safety, performance, strong typing, and ecosystem alignment with Polkadot SDK and the broader Substrate/Polkadot toolchain. TypeScript provides reach into browser and existing Polkadot Product SDK surfaces.

**Constraints:**
- Security-critical logic (policy evaluation, grant resolution, signing orchestration, effect lifecycle) must be in Rust.
- A TypeScript companion is a bounded leaf adapter; it owns no kernel state, policy, grants, custody, or signing secrets.
- PCA compatibility may require a bounded TypeScript/JS codec bridge as a temporary leaf transport adapter.

### 4.3 P-02: Polkadot-native depth before generic breadth

**Status:** Established

Polkagent is primarily for Polkadot, Polkadot SDK, Polkadot Hub and system-chain functionality, JAM/PVM, and related Parity/Web3 Foundation product surfaces. Polkadot concepts are first-class domain types, not thin labels over a generic multichain wallet agent.

**Constraints:**
- External protocols may be integrated when useful, especially model, harness, tool, identity, payment, and interoperability standards.
- The public product model should not be generalized into a lowest-common-denominator multichain abstraction before Polkadot depth is proven.
- Chain profiles, runtime metadata, XCM semantics, governance parameters, asset identities, and finality models are typed domain objects with Polkadot-specific behavior.

### 4.4 P-03: Local and cloud are equal product modes

**Status:** Established

Local/self-hosted and managed multi-tenant cloud are equally intentional platform modes. They must implement the same public domain contracts, specs, policy semantics, export format, and compatibility tests.

**Constraints:**
- State, identities, agent definitions, artifacts, policies, and audit history must be portable.
- Managed services may add fleet management, collaboration, hosted execution, billing, policy distribution, observability, backups, and support without making the local runtime a second-class client.
- No forced cloud dependency or silent control-plane access to private conversations, wallet material, or workspace contents.
- When the control plane is unavailable, the data plane follows locally cached policy and pauses actions outside it. No grant is widened or silently executed.

### 4.5 P-04: Evidence over assertion

**Status:** Established

Every material action produces durable, attributable evidence. The platform distinguishes between:

| Evidence type | Description |
|---|---|
| Canonical data | Decoded call data, metadata bindings, simulation results, finality observations |
| Policy records | Grant evaluations, approval decisions, mandate references |
| Effect records | Intent, attempt, outcome with timestamps and causal links |
| Artifacts | Files, diffs, test results, plans, receipts, reports |
| Model narrative | Explanatory text from AI models -- labeled as such, never authoritative |

**Constraints:**
- A model's assertion that something happened is not evidence that it happened.
- Canonical action cards are rendered from typed, metadata-bound data, not model prose.
- Receipts must correctly represent failed, unknown, and successful cases.
- Evidence supports replay/audit without replaying external side effects.

### 4.6 P-05: Configurable autonomy with invariant safety

**Status:** Established

The platform supports a configurable ladder from read-only and per-action approval through independently funded, fully autonomous agents. Any supported action family may run without a human in the loop when deliberately configured.

**Autonomy as a dial, not a switch.** The four positions on the dial are:

| Dial position | Description |
|---|---|
| Observe-only | Agent reads chain state and produces reports; zero write grants. |
| Propose | Agent drafts intents; every action requires explicit human approval before execution. |
| Bounded-auto within policy + budget | Actions within defined budgets, allowlists, time boundaries, and counterparty lists execute automatically; all others escalate. |
| Time-delayed | Autonomous execution proceeds after a configured delay, giving the owner a window to cancel before the action reaches the chain. |

**Enabling primitives for the autonomy dial:** Cedar policy evaluation (declarative, verifiable rules) + proxy time-delay (on-chain revocation window) + per-period budgets (value caps enforced before signing). These three primitives together create a bounded autonomous authority that an owner can meaningfully configure and understand.

**Explicit avoid:** "Any"-proxy autonomy (an agent granted a Polkadot `Any`-type proxy can perform any extrinsic, which defeats all bounded-auto safety properties) and unbounded auto-execution (no budget, no allowlist, no delay) are explicitly not supported as safe defaults and must require extreme explicit configuration with clear UX warnings.

**Invariants that hold at every autonomy level:**
- Raw signing keys remain outside model/tool contexts.
- Audit trails are immutable and complete.
- Revocation is immediate and effective.
- Recovery procedures are defined and tested.
- Unknown outcomes are never silently treated as success or failure.
- Grant resolution is deterministic and outside model control.
- Idempotent effects never duplicate an irreversible action.

**Constraints:**
- Default flows are read-only or explicit approval.
- Broad or unlimited owner-selected policies must be accompanied by clear UX explanation of consequences.
- Emergency controls (pause, revoke, circuit-breaker) must be available at every autonomy level.

### 4.7 P-06: Extensibility without kernel bloat

**Status:** Established

The platform provides distinct extension axes:

| Extension axis | Examples |
|---|---|
| Providers | Model API connections (OpenAI, Anthropic, local, gateway) |
| Models | Specific model identities and capability sets |
| Executors | Run adapters for different execution modes |
| Harnesses | Long-lived coding-agent or framework processes |
| Tools | Typed operations under capability grants |
| Skills | Versioned instruction/context/test bundles |
| Transports | Chat, web, API, mobile, webhook channels |
| Chain clients | Polkadot, Asset Hub, parachains, custom chains |
| Signers | External wallets, hardware, proxy, managed KMS |
| Stores | SQLite, Postgres, object storage, cloud services |
| Memory systems | Episodic, semantic, procedural memory backends |
| Triggers | Feeds, schedules, chain events, webhooks |
| Surfaces | CLI, web, mobile, desktop presentation |
| Control-plane services | Billing, fleet management, policy distribution |

**Constraints:**
- Stable traits and protocols should be narrow and capability-oriented.
- Optional integrations remain leaf dependencies; the core must not import provider, harness, transport, wallet, or chain-specific application logic.
- Extensions declare identity, version, compatibility, permissions, inputs, outputs, effects, costs, and health behavior.
- Adding an integration should not require modifying unrelated core crates.

### 4.8 P-07: Progressive disclosure and excellent UX

**Status:** Established

- Common jobs must work with sensible presets and minimal mandatory configuration.
- Complexity should appear only when a user needs it.
- Every powerful choice must have a plain-language explanation, consequence preview, and safe default.
- A beginner can start from a product kit; an expert can edit the underlying versioned specs, policies, routes, and manifests.
- Configuration should be possible through a polished UI, declarative files, CLI, and API without creating divergent semantics.
- Long-running work must expose progress, waiting states, requested approvals, costs, failures, and a definitive final outcome.
- Errors must describe what happened, what remains safe, and the next recovery action.

### 4.9 P-08: Portable state and no forced lock-in

**Status:** Established

- Agent definitions, configurations, artifacts, policies, audit history, and identity can be exported and imported.
- Moving between local, self-hosted, and managed cloud does not require rebuilding an agent from scratch.
- No single registry, provider, or cloud service is a mandatory dependency for core operation.
- Export formats are documented, versioned, and tested.

**Local-first / portable-state design:** One SQLite file is the user's entire agent -- conversations, runs, artifacts, effects, audit trail, and configuration. This file is exportable, inspectable with standard tools, and migratable between local, self-hosted, and managed deployments without a proprietary export step. A user who copies their SQLite file to another machine has their complete agent. A user who wants to inspect their agent's history needs no special tooling beyond sqlite3. This is a product commitment, not an implementation detail.

### 4.10 P-09: Permissionless extension with safe defaults

**Status:** Established

- Skills, tools, models, compute, feeds, agent services, and product kits can be published and discovered through permissionless, multi-registry, federated marketplaces.
- Verified and curated views may be excellent defaults but must not create a mandatory central publishing gate.
- Discovery is separate from activation and authorization.
- Self-hosted users can consume, mirror, federate, sideload, or run registries without managed-control-plane lock-in.
- Extensions use sandboxing, capability disclosure, operator approval, and policy-controlled activation.

### 4.11 P-10: Separation of narrative from authority

**Status:** Established

- Model-generated text is explanatory. It is never the source of truth for whether an action occurred, whether a grant exists, what a transaction contains, or what the outcome was.
- Canonical action cards, policy decisions, grant resolutions, effect outcomes, and finality observations are authoritative data structures.
- Rendering must visually distinguish model narrative from canonical data.
- A model cannot forge card fields, widen grants, or override policy decisions.

### 4.12 P-11: Explicit maturity and honest uncertainty

**Status:** Established

- Every feature, integration, and capability carries a maturity label (Section 11).
- Experimental features are isolated behind research flags and explicit UI labeling.
- Core correctness and ordinary product flows cannot depend on experimental features.
- Unknown outcomes are represented as unknown, not collapsed into success or failure.
- Time-sensitive facts carry access dates and source references.

**Section 4 acceptance criteria:**
- [ ] Every principle has a unique ID, name, status, rationale, and constraints.
- [ ] Principles cover language, ecosystem, deployment, evidence, autonomy, extensibility, UX, portability, marketplace, narrative/authority, and maturity.
- [ ] No principle contradicts another; interactions are acknowledged.
- [ ] Principles are testable: each has constraints that can be verified.

---

## 5. Design philosophy

### 5.1 Minimal kernel

The durable kernel owns state transitions, identity, sequencing, policy evaluation, grant resolution, effect lifecycle, and audit evidence. It is deliberately small. Everything that can be a leaf adapter is a leaf adapter.

**The kernel owns:**
- Stable IDs and versioned specifications.
- Authoritative aggregate state transitions.
- Run, turn, event, artifact, effect, approval, and grant lifecycles.
- Deterministic policy evaluation.
- Idempotency, causality, sequencing, deadlines, cancellation, and recovery.
- Configuration and policy revision binding.
- Audit evidence and projection inputs.

**The kernel does not own:**
- Provider-specific HTTP clients.
- Harness-specific session formats.
- Transport-specific polling or UI.
- Wallet UI or raw signing keys.
- Polkadot product presentation logic.
- Marketplace ranking policy.
- A universal workflow language for every future use case.

### 5.2 Bounded leaf adapters

Each integration point -- providers, chain clients, signers, transports, stores -- implements a narrow, stable trait. An adapter:

- Declares its identity, version, capabilities, permissions, costs, and health behavior.
- Receives only the data it needs to perform its function.
- Returns normalized events and outcomes to the kernel.
- Cannot access other adapters' state directly.
- Cannot modify kernel state except through the kernel's public interface.
- Can be replaced, upgraded, or removed without modifying unrelated adapters.

### 5.3 Permissionless extension

Extensions are units of capability that can be created, published, discovered, installed, and used without a central authority's permission:

```text
Extension lifecycle:
  create -> declare capabilities and requirements
  -> test with conformance fixtures
  -> sign and publish to one or more registries
  -> discover through search, recommendation, or direct reference
  -> review capabilities, permissions, and trust signals
  -> install with operator approval
  -> activate under policy and sandbox constraints
  -> monitor health, usage, and cost
  -> update, revoke, or uninstall
```

The marketplace is a discovery and distribution layer, not a trust oracle. Activation decisions are made by operators, not by the marketplace.

### 5.4 Evidence-bearing effects

Every material external interaction -- calling a model, running a tool, sending a message, signing a payload, submitting a transaction -- follows the effect lifecycle:

```text
Effect lifecycle:
  EffectIntent   -- durable command recorded before I/O
  EffectAttempt   -- one claim, lease, retry, and idempotency lifecycle
  EffectOutcome   -- immutable success, failure, timeout, cancellation, or unknown
```

**Key properties:**
- An effect intent is recorded before the I/O happens.
- An attempt can be leased, retried, and tracked for idempotency.
- An outcome is immutable once recorded.
- Unknown is a valid and preserved outcome; it is never collapsed.
- Replay/debugging can reconstruct a result from recorded intents and outcomes without resending effects.
- Crash/restart never duplicates an irreversible effect.

### 5.5 Configurable autonomy

Autonomy is not a single global switch. It is configured per:

- **Action family:** transfers, governance votes, contract deployments, etc.
- **Account/asset scope:** which accounts and assets an agent may touch.
- **Value limits:** per-action, per-period, and total budgets.
- **Counterparty allowlists/denylists:** permitted recipients and contracts.
- **Time boundaries:** mandate duration, active hours, expiration.
- **Network scope:** which chains and profiles the mandate covers.
- **Approval requirements:** none, single-user, quorum, external system.

The configuration is explicit, versioned, and reviewable. The platform must render the consequences of a broad or unlimited mandate clearly in the UX.

### 5.6 Layered system view

```text
User and product surfaces
  Polkadot/mobile chat | Web/PWA | Studio | Inbox | CLI/TUI | API/SDK
                              |
Transport and presentation adapters
  authentication | messages | files | live updates | action sheets
                              |
Durable application/runtime services
  conversations | runs | orchestration | context | policy | projections
                              |
Minimal domain kernel
  stable IDs | state machines | artifacts | events | effects | grants
                              |
Execution and product ports
  models | harnesses | tools | memory | chain | signer | payment | registry
                              |
Infrastructure adapters
  SQLite/Postgres | object store | queues | RPCs | wallets | KMS | telemetry
```

Only downward-facing contracts connect the layers. A mobile transport cannot write a "completed" turn directly; a model provider cannot submit a transaction; a marketplace package cannot grant itself network or signer access; and a control plane cannot silently reinterpret a local agent's policy.

### 5.7 One request through the architecture

For a request that eventually transfers an asset:

1. `Transport` authenticates and durably admits a message.
2. The conversation service creates a `Turn` and `Run`.
3. The context assembler records which messages, artifacts, memories, and chain evidence enter the model context.
4. `TurnExecutor` emits a structured proposed action.
5. The application service validates it into a typed `ChainIntent`.
6. `ChainClient` effects decode metadata and collect read-only/preflight evidence.
7. `PolicyEvaluator` creates a `PolicyDecision` and `ResolvedGrant`.
8. The approval service either records an exact approval, finds an applicable autonomous mandate, or denies the action.
9. `Signer` receives only the canonical payload and binding data.
10. Submission and finality watchers run as separate idempotent effects.
11. Projections render the same truthful state in chat, web, CLI, audit, and operator views.

At no point is a model sentence such as "the transfer succeeded" authoritative.

**Section 5 acceptance criteria:**
- [ ] The kernel boundary is explicitly defined (what it owns vs. does not own).
- [ ] Leaf adapter constraints are stated.
- [ ] Extension lifecycle is described.
- [ ] The effect lifecycle (Intent/Attempt/Outcome) is defined with properties.
- [ ] Autonomy configuration dimensions are enumerated.
- [ ] The layered system view shows clear separation of concerns.
- [ ] A concrete request-through-architecture walkthrough is provided.

---

## 6. Confirmed owner decisions

The following decisions are owner-confirmed and normative for all PRDs. They represent the definitive product direction and supersede any conflicting language in research documents or earlier planning material.

| # | Topic | Definitive direction | Status |
|---|---|---|---|
| D-01 | Scope | Complete long-term platform with implementable phases and an intentionally narrow first proof. | Established |
| D-02 | Central pillars | Product building, on-chain action/payments, and PCA/mobile assistant functionality are equally central. | Established |
| D-03 | Language | Rust for runtime, chain/payment/security logic, CLI, services, and SDKs; TypeScript where browser UI or Product SDK integration requires it. | Established |
| D-04 | Ecosystem | Primarily Polkadot, Polkadot SDK, JAM/PVM, and Parity/Web3 Foundation product surfaces. Do not generalize the public model into a generic multichain framework prematurely. | Established |
| D-05 | PCA compatibility | Preserve all relevant behavior, bridge/plugin compatibility, identity/state/config migration, and deployment workflows while replacing internal coupling with better abstractions. | Established |
| D-06 | Autonomy | Support a configurable ladder from read-only and per-action approval through independently funded, fully autonomous agents. Any supported action family may run without a human in the loop when deliberately configured; raw signing keys still remain outside model/tool contexts and audit/recovery invariants remain active. | Established |
| D-07 | Deployment | First-class local/self-hosted and managed multi-tenant cloud/control plane; portable state/config/identity with no forced lock-in. | Established |
| D-08 | Advanced systems | Incorporate Roko-style memory, learning, graphs, groups, feeds/triggers, extensions, marketplace, identity/reputation, and evals where they add real value. | Established |
| D-09 | Marketplace | Permissionless and plural publication/discovery for skills, tools, models, compute, feeds, agent services, and product kits. Use safe verified/curated default views, sandboxing, and policy-controlled activation without imposing a single central publishing gate. Support configurable platform/protocol/registry/creator fees. | Established |
| D-10 | Identity | Polkadot-native agent identity/account and optional personhood/reputation/registry integration, while allowing local-only or pseudonymous agents. | Established |
| D-11 | Custody | Configurable external wallet, hardware, proxy/multisig, local encrypted, organizational, managed KMS/HSM, MPC, programmable-account, and funded-agent-account modes. Default to user/external signing for valuable actions while keeping all modes portable and explicit. | Established |
| D-12 | User surfaces | Preserve and improve PCA-style mobile/chat behavior. Select among Polkadot App/Product integration, responsive web/PWA, desktop, and a dedicated native client according to the best achievable UX; do not depend on one experimental host API. | Established |
| D-13 | Experimental work | Define ambitious JAM/PVM, autonomous learning, and agent-economy targets, but label maturity, dependencies, validation spikes, and release gates. | Established |
| D-14 | Deliverable detail | Include Rust traits, schemas, state machines, database/API/config examples, UX flows, cloud tenancy, deployment, acceptance, and migration plans. | Established |
| D-15 | Funding strategy | Fund via OpenGov treasury/bounties and W3F targeted strategic grants. Decentralized Futures is closed and not a funding path. W3F has pivoted to product-centric, targeted strategic funding, which requires demonstrating concrete value to the Polkadot ecosystem before and during applications. | Established |
| D-16 | Primary target | Treasury/governance operator (P5) and parachain-builder (P1) are the primary target personas for initial development and funding. Retail/mobile-first (P4 as a passive DOT holder with no technical background) is explicitly deferred. | Established |

### 6.1 Decision interpretation rules

- A decision labeled "Established" must not be contradicted by any PRD.
- A decision applies to the long-term platform, not only to the first release.
- A decision that mentions phased delivery does not defer the design; it defers the implementation.
- "Configurable" means both the safe default and the advanced option must be designed and tested.
- "Permissionless" means absence of a mandatory central gate, not absence of safety controls.

**Section 6 acceptance criteria:**
- [ ] Every decision from the research brief is reproduced with its definitive direction.
- [ ] Each decision has a unique ID for cross-reference.
- [ ] Interpretation rules prevent misreading decisions as deferral or exclusion.

---

## 7. Target personas

### 7.1 Persona framework

Each persona is described with:

- **Identity:** who they are and their organizational context.
- **Existing tools and workflows:** what they use today.
- **Pain points:** what is hard or broken in current workflows.
- **Polkagent value:** what the platform provides for them.
- **Primary pillar(s):** which pillar(s) they primarily engage with.
- **Autonomy posture:** their typical position on the autonomy ladder.
- **Risk profile:** what can go wrong and what they need to trust.

### 7.2 Persona P1: Runtime/parachain engineer

**Identity:** A Rust developer who works on Polkadot SDK runtimes, pallets, chain specifications, storage migrations, and runtime upgrades. They may work at a parachain team, a system-chain contributor, or as an independent developer.

**Existing tools and workflows:**
- Polkadot SDK templates and `cargo` toolchain.
- Zombienet for local multi-chain test networks.
- Chopsticks for fork-testing and simulation.
- Polkadot-JS Apps for manual chain interaction.
- Git, CI/CD, and code review tools.
- Documentation scattered across wiki, docs, and repository READMEs.

**Pain points:**
- Storage migrations are high-risk, hard to test, and easy to get wrong.
- Runtime upgrades can break clients, frontends, and cross-chain integrations silently.
- Metadata changes are not automatically detected or communicated to downstream consumers.
- Local test networks require significant setup and maintenance.
- Benchmarking and weight calculation are tedious and error-prone.
- Cross-chain testing with XCM requires understanding multiple runtime versions simultaneously.

**Polkagent value:**
- AI-assisted code generation with deep Polkadot SDK context.
- Automated storage migration rehearsal with fork-based pre/post checks.
- Runtime upgrade impact analysis with metadata diff and compatibility reports.
- Metadata-drift detection and automated client regeneration proposals.
- Integrated Zombienet and Chopsticks orchestration.
- Benchmark analysis and weight explanation.
- Durable project/worktree management with artifact trail.

**Primary pillar(s):** Build (primary), Act (for deployment and testing)

**Autonomy posture:** Read-only to propose-only for chain interactions; per-action approval for test deployments; policy-bounded for CI/CD automation in non-production environments.

**Risk profile:** A bad migration can brick a chain. A broken upgrade can disable governance. A wrong XCM configuration can lose funds. They need the agent to be a rigorous reviewer, not an unchecked code generator.

### 7.3 Persona P2: Smart contract / PVM developer

**Identity:** A developer building smart contracts (ink!, Solidity/Revive) or PVM programs for deployment on Polkadot SDK chains. May range from a Web3 newcomer to an experienced Solidity developer exploring Polkadot.

**Existing tools and workflows:**
- ink! toolchain (`cargo-contract`, `drink` testing framework).
- Solidity/Revive toolchain for EVM-compatible contracts.
- Contract explorers and interaction tools.
- Local development nodes.
- Unit testing frameworks and limited property testing.

**Pain points:**
- PVM/PolkaVM tooling is early and documentation is sparse.
- Contract testing is limited compared to traditional software.
- Deployment and upgrade paths are not well documented.
- Cross-contract and cross-chain interactions are complex.
- Security audit tooling is immature compared to Ethereum/Solidity.
- Debugging deployed contracts is difficult.

**Polkagent value:**
- Contract scaffolding with ink!/PVM templates and patterns.
- Integrated build, test, static analysis, and property/fuzz testing workflows.
- Contract evidence workbench with versioned artifact retention.
- Deployment with simulation and review gates.
- Contract monitoring and interaction tools.
- PVM development assistance as the ecosystem matures.

**Primary pillar(s):** Build (primary), Act (for deployment and interaction)

**Autonomy posture:** Propose-only for deployments; per-action approval for contract calls with value; read-only for exploration.

**Risk profile:** Contract bugs can lose user funds permanently. Deployment is irreversible on many chains. They need strong testing and review gates, not faster deployment.

### 7.4 Persona P3: dApp / frontend developer

**Identity:** A developer building user-facing applications that interact with Polkadot chains. Often works in TypeScript/JavaScript with React or similar frameworks. May not be deeply familiar with Polkadot SDK internals.

**Existing tools and workflows:**
- PAPI or Polkadot-JS API for chain interaction.
- Web frameworks (React, Next.js, etc.).
- Wallet libraries and WalletConnect.
- Block explorers and indexers (Subscan, SubQuery).
- Product SDK for Polkadot App integration.

**Pain points:**
- Runtime upgrades break client code without warning.
- API type generation is fragile and version-specific.
- Understanding XCM from a frontend perspective is daunting.
- Wallet integration varies across providers.
- Testing against real chain state is slow and unreliable.
- Polkadot-specific patterns (metadata, SCALE encoding, existential deposits) require specialized knowledge.

**Polkagent value:**
- Product kits with starter templates, typed APIs, and deployment defaults.
- Metadata-aware API generation and drift detection.
- Chain interaction helpers with proper error handling and fee estimation.
- Wallet integration guidance and testing.
- Application monitoring and user experience tools.
- Embedding Polkagent agents as application features (SDK/API).

**Primary pillar(s):** Build (primary), Reach (for distribution)

**Autonomy posture:** Read-only for development queries; fully local for build/test cycles.

**Risk profile:** Application bugs confuse users; integration breakage after runtime upgrades damages trust. They need stable APIs and clear migration paths more than AI-generated code.

### 7.5 Persona P4: DOT holder and Polkadot user

**Identity:** An individual who holds DOT or other Polkadot ecosystem assets. Ranges from passive holders to active stakers, nominators, and governance participants. May use mobile primarily and have limited technical knowledge.

**Existing tools and workflows:**
- Polkadot-JS Apps or alternative wallets (Nova, SubWallet, Talisman).
- Block explorers for transaction tracking.
- Governance dashboards (Polkassembly, SubSquare).
- Staking dashboards.
- Mobile wallet apps.

**Pain points:**
- Transaction details are opaque -- encoded bytes with no clear explanation.
- Cross-chain operations (XCM) are confusing and risky.
- Governance proposals are complex and hard to evaluate.
- Staking requires understanding complex parameters (era, sessions, slashing).
- Proxy and multisig operations are error-prone.
- Transaction outcomes are ambiguous (included vs. finalized vs. failed).

**Polkagent value:**
- Explain Before Sign: clear, canonical explanation of what a transaction does before signing.
- Plain-language governance research with cited sources.
- Staking and nomination guidance with risk assessment.
- Account monitoring and balance tracking across chains.
- Cross-chain operation planning with route and fee explanation.
- Receipt and audit trail for all transactions.
- Mobile-friendly assistant through chat or Inbox.

**Primary pillar(s):** Act (primary), Reach (for mobile/chat access)

**Autonomy posture:** Read-only by default; per-action approval for transactions; possible policy-bounded autonomy for recurring staking operations after trust is established.

**Risk profile:** Losing DOT to a confusing transaction is devastating. Being tricked by a malicious batch or proxy addition is a real threat. They need the agent to be a trustworthy translator and guardian, not just a faster transaction submission tool.

### 7.6 Persona P5: Treasury / governance participant

**Strategic priority:** P5 (combined with P1 for parachain-builders) is the primary target persona for Polkagent's first workflow and initial funding strategy. Research confirms that W3F has pivoted to product-centric, targeted strategic funding, and the OpenGov treasury/bounties path requires demonstrating concrete value to treasury operators and governance participants. Retail/consumer personas (P4 as a mobile-first DOT holder) are explicitly deferred.

**Identity:** A delegate, council member, treasury curator, bounty administrator, or active governance voter. May manage significant collective funds through multisig or proxy arrangements.

**Existing tools and workflows:**
- Polkassembly and SubSquare for proposal discussion.
- Polkadot-JS Apps for governance operations.
- Multisig coordination tools (Polkadot Multisig, manual coordination).
- Spreadsheets and documents for treasury accounting.
- Communication channels (Element, Discord, forums).

**Pain points:**
- Referendum analysis requires reading complex preimages and understanding track mechanics.
- Treasury proposals lack standardized impact assessment.
- Multisig coordination is manual and error-prone.
- No integrated audit trail connecting votes, delegations, and treasury disbursements.
- Proxy/multisig confusion causes operational errors and potential fund loss.
- Governance participation requires significant time investment to evaluate proposals.

**Polkagent value:**
- Governance research copilot with cited, profile-backed analysis of referenda.
- Full treasury/governance operator workflow: research → explain → simulate (DryRunApi) → intent + evidence card → approve → sign (Vault/Ledger) → submit → finality → report.
- Multisig/proxy coordination assistant with state tracking and approval workflows.
- Treasury accounting with receipt and reconciliation tools.
- Delegation management and voting history.
- Automated monitoring of governance activity relevant to interests.
- Evidence-based risk flags for proposals with large treasury impact.
- Read-only "shadow mode" entry: connect wallet, see delegations and treasury exposure, get one safe suggested action -- without any signing authority required to begin.

**Primary pillar(s):** Act (primary), Build (for governance tooling)

**Autonomy posture:** Read-only for research; per-action approval for votes and treasury operations; possible policy-bounded autonomy for delegation management.

**Risk profile:** Governance mistakes can direct millions in treasury funds. Proxy confusion can compromise organizational accounts. They need careful, evidence-based decision support with strong operational safety.

### 7.7 Persona P6: Agent operator (individual)

**Identity:** A technically competent individual who deploys and operates one or more Polkagent agents. May run agents for personal use, for a small team, or as a service for others. Comfortable with CLI, configuration, and deployment but not necessarily a platform contributor.

**Existing tools and workflows:**
- Docker, systemd, or cloud VMs for process management.
- Configuration management (dotfiles, environment variables).
- Monitoring tools (logs, metrics, alerts).
- SSH and remote management.

**Pain points:**
- Agent setup requires understanding many moving parts (models, tools, permissions, chains).
- No standardized agent definition or deployment format.
- Monitoring agent behavior requires custom tooling.
- Cost management (model API costs, compute, chain fees) is ad hoc.
- Security configuration is complex and easy to get wrong.
- No easy way to share or replicate agent configurations.

**Polkagent value:**
- Declarative `AgentSpec` and `ProductSpec` for portable agent definitions.
- CLI and web-based configuration with progressive disclosure.
- Integrated monitoring, cost tracking, and health checks.
- Policy and permission management with safe defaults.
- Deployment targets (local, Docker, cloud) with the same agent definition.
- Marketplace for pre-built skills, tools, and product kits.

**Primary pillar(s):** All three (as operator)

**Autonomy posture:** Configures autonomy per agent and per action family. May run some agents in full autonomy mode for routine operations.

**Risk profile:** A misconfigured agent can act outside its intended scope. A compromised agent can access sensitive data or funds. They need clear configuration UX, strong defaults, and operational visibility.

### 7.8 Persona P7: Agent operator (organizational)

**Identity:** A team or organization that operates a fleet of agents for internal use or as a service. Has dedicated infrastructure, security requirements, compliance obligations, and multiple users with different roles.

**Existing tools and workflows:**
- Kubernetes or cloud container orchestration.
- Enterprise identity and access management.
- Compliance and audit frameworks.
- Cost allocation and billing systems.
- Incident response procedures.

**Pain points (in addition to individual operator pain points):**
- Multi-tenancy and isolation requirements.
- Role-based access control for agent configuration and operation.
- Audit and compliance reporting.
- Fleet management across environments and regions.
- Cost allocation across teams and projects.
- Incident response and recovery procedures for agent failures.

**Polkagent value:**
- Multi-tenant control plane with organization/team/user boundaries.
- Role-based policy and permission management.
- Fleet deployment, monitoring, and health management.
- Audit trail and compliance reporting.
- Cost allocation and billing integration.
- Regional deployment options.
- Incident response tooling and recovery procedures.

**Primary pillar(s):** All three (as organizational operator)

**Autonomy posture:** Organization-wide policies constrain individual agent autonomy. May require quorum approval for high-value actions.

**Risk profile:** Organizational reputation, compliance obligations, and aggregated fund exposure multiply the stakes. They need enterprise-grade security, audit, and operational controls.

### 7.9 Persona P8: Platform operator (self-hosted)

**Identity:** A team or individual who runs the Polkagent platform infrastructure itself, either for internal use or as a hosting service. Responsible for availability, updates, backups, and security of the platform.

**Existing tools and workflows:**
- Infrastructure as code (Terraform, Ansible, NixOS).
- Database administration (SQLite, Postgres).
- Network security and firewall management.
- Backup and disaster recovery.
- Performance monitoring and capacity planning.

**Pain points:**
- No existing Polkadot-native agent platform to operate.
- Complex dependency management across Rust runtime, TypeScript companions, chain clients, and model providers.
- Security hardening for a system that handles signing and funds.
- Upgrade procedures that maintain state and configuration.
- Monitoring agent health across different execution modes.

**Polkagent value:**
- Well-defined deployment topologies with documented requirements.
- Database migration and upgrade procedures.
- Health check and monitoring integration points.
- Backup and restore procedures.
- Security hardening guides and conformance tests.
- Clear separation of data plane and optional control plane.

**Primary pillar(s):** Infrastructure supporting all three pillars

**Autonomy posture:** Controls the autonomy boundaries available to tenant agents.

**Risk profile:** Platform compromise affects all tenants. Data loss affects all users. They need operational excellence documentation and tooling.

### 7.10 Persona P9: Marketplace publisher

**Identity:** A developer or team that creates and publishes skills, tools, model routes, harnesses, feeds, agent services, or product kits for use by others. May be an individual contributor, a specialized team, or the platform team itself.

**Existing tools and workflows:**
- Package management (npm, crates.io, Docker Hub).
- CI/CD for building and testing.
- Documentation and marketing.
- Version management and changelogs.
- License and pricing management.

**Pain points:**
- No established marketplace for Polkadot agent capabilities.
- No standard packaging format for agent skills, tools, or product kits.
- Discovery and distribution require building custom channels.
- Trust and reputation are hard to establish.
- Monetization paths are unclear.

**Polkagent value:**
- Standardized manifest and packaging format.
- Permissionless publication to one or more registries.
- Conformance testing and compatibility verification.
- Trust signals (signatures, evaluations, usage metrics).
- Configurable pricing and fee models.
- Update, revocation, and vulnerability response mechanisms.
- Federation and multi-registry publication.

**Primary pillar(s):** Build (creating extensions), Reach (distribution)

**Autonomy posture:** N/A for publishing; published extensions must declare their required autonomy.

**Risk profile:** A malicious or buggy extension can compromise users. They need clear capability disclosure, sandboxing, and incident response procedures. Their reputation depends on reliability and security.

### 7.11 Persona P10: Autonomous agent owner

**Identity:** An individual or organization that funds and authorizes an agent to act continuously and autonomously. This persona deliberately grants broad authority to achieve operational goals without routine human intervention.

**Existing tools and workflows:**
- Manual execution of recurring operations.
- Simple automation scripts and cron jobs.
- Proxy/multisig setups for limited delegation.
- Manual monitoring and intervention.

**Pain points:**
- Recurring operations (staking, governance, treasury management) require constant human attention.
- Delegation mechanisms are crude (proxy types, limited call filters).
- No way to define nuanced operational policies (budget limits, counterparty restrictions, time windows).
- No integrated monitoring, receipting, and emergency control for autonomous operations.
- Setting up continuous automation requires significant technical expertise.

**Polkagent value:**
- Configurable mandates with fine-grained scope, budget, time, and counterparty controls.
- Policy-bounded autonomous execution with isolated signing.
- Continuous monitoring with circuit breakers and emergency pause/revoke.
- Complete receipt and audit trail for all autonomous actions.
- Budget tracking and cost management.
- Clear UX for understanding and configuring broad authority.
- Recovery procedures for unexpected states.

**Primary pillar(s):** Act (primary)

**Autonomy posture:** Policy-bounded or full autonomy for configured action families. This is the persona that exercises the highest levels of autonomy.

**Risk profile:** The highest risk persona. Autonomous operation with real funds requires the strongest safety guarantees. Misconfiguration can lead to rapid fund depletion. Compromised agents can drain accounts. They need the invariant safety guarantees (isolated signing, audit, revocation, recovery) to hold under all conditions.

### 7.12 Persona summary matrix

| ID | Persona | Primary pillar(s) | Default autonomy | Technical depth | Fund exposure |
|---|---|---|---|---|---|
| P1 | Runtime/parachain engineer | Build | Read-only / Propose | Expert | Low (test environments) |
| P2 | Contract/PVM developer | Build | Read-only / Propose | Advanced | Low-Medium |
| P3 | dApp/frontend developer | Build, Reach | Read-only | Intermediate | None |
| P4 | DOT holder/user | Act, Reach | Read-only / Per-action | Basic-Intermediate | Medium-High |
| P5 | Treasury/governance participant | Act, Build | Read-only / Per-action | Intermediate-Advanced | High (collective) |
| P6 | Agent operator (individual) | All | Configurable | Advanced | Variable |
| P7 | Agent operator (organization) | All | Org-policy constrained | Advanced | High (aggregated) |
| P8 | Platform operator (self-hosted) | Infrastructure | N/A | Expert | System-wide |
| P9 | Marketplace publisher | Build, Reach | N/A | Advanced | Reputation |
| P10 | Autonomous agent owner | Act | Policy-bounded / Full | Advanced | High |

**Section 7 acceptance criteria:**
- [ ] Every persona from the research brief is defined with identity, tools, pain points, value, pillar, autonomy, and risk.
- [ ] The persona matrix provides a scannable summary.
- [ ] Personas span all three pillars.
- [ ] Autonomy postures are realistic and differentiated.
- [ ] Risk profiles acknowledge what can go wrong for each persona.

---

## 8. Jobs to be done

### 8.1 JTBD framework

Each JTBD entry captures:
- **Persona:** which persona(s) perform this job.
- **Situation:** when and why the job arises.
- **Motivation:** what the persona is trying to achieve.
- **Expected outcome:** the successful end state.
- **Current alternatives:** how the job is done today without Polkagent.
- **Pillar:** which pillar(s) the job maps to.

### 8.2 Build pillar JTBD

#### JTBD-B01: Scaffold a new Polkadot SDK runtime or pallet

| Field | Value |
|---|---|
| Persona | P1 (Runtime engineer) |
| Situation | Starting a new parachain, adding a pallet to an existing runtime, or creating a solo chain. |
| Motivation | Get a correct, well-structured starting point with proper configuration, tests, and toolchain setup. |
| Expected outcome | A compilable, testable project with appropriate pallet templates, configuration, genesis, benchmarks, and CI setup -- all matching the target Polkadot SDK version. |
| Current alternatives | Polkadot SDK templates, Pop CLI, manual cargo init with copypaste from examples. |
| Pillar | Build |

#### JTBD-B02: Rehearse a storage migration

| Field | Value |
|---|---|
| Persona | P1 (Runtime engineer) |
| Situation | Preparing a runtime upgrade that includes storage migrations. The engineer needs to verify that migrations transform state correctly without data loss or corruption. |
| Motivation | Avoid bricking the chain or losing user data during a runtime upgrade. |
| Expected outcome | A reproducible rehearsal against a fork of the target chain state, with pre/post state comparison, error detection for seeded bad migrations, and evidence artifacts. No alteration of live state. |
| Current alternatives | try-runtime, Chopsticks fork testing, manual review, hope. |
| Pillar | Build |

#### JTBD-B03: Analyze a runtime upgrade impact

| Field | Value |
|---|---|
| Persona | P1 (Runtime engineer), P3 (dApp developer) |
| Situation | A new runtime version is proposed or enacted. Downstream consumers need to know what changed and whether their code still works. |
| Motivation | Prevent silent breakage of clients, frontends, and integrations after a runtime upgrade. |
| Expected outcome | A structured report showing changed calls, types, constants, events, storage items, and migration cues -- attributed to specific metadata versions and runtime artifacts. |
| Current alternatives | Manual metadata diffing, reading changelogs (if they exist), trial and error. |
| Pillar | Build |

#### JTBD-B04: Build and test a smart contract

| Field | Value |
|---|---|
| Persona | P2 (Contract developer) |
| Situation | Developing an ink! or Solidity/Revive smart contract for deployment on a Polkadot SDK chain. |
| Motivation | Build a correct, secure contract with comprehensive testing before deployment. |
| Expected outcome | A contract with build artifacts, unit tests, property/fuzz tests, static analysis results, deployment simulation evidence, and versioned artifact retention. |
| Current alternatives | cargo-contract, drink testing framework, limited fuzzing, manual review. |
| Pillar | Build |

#### JTBD-B05: Set up and run a local test network

| Field | Value |
|---|---|
| Persona | P1 (Runtime engineer), P2 (Contract developer) |
| Situation | Need to test cross-chain interactions, governance flows, or multi-node scenarios locally. |
| Motivation | Validate behavior in a realistic multi-chain environment without using public testnets. |
| Expected outcome | An ephemeral, reproducible test network with configured chains, assets, accounts, and scenarios. Test results captured as evidence. |
| Current alternatives | Zombienet with manual TOML configuration, docker-compose setups. |
| Pillar | Build |

#### JTBD-B06: Create and maintain a coding harness session

| Field | Value |
|---|---|
| Persona | P1, P2, P3 (any developer) |
| Situation | Working on a codebase that benefits from AI-assisted coding with persistent context, file access, and tool usage. |
| Motivation | Get intelligent coding assistance that understands the project, remembers decisions, and can execute build/test commands within defined boundaries. |
| Expected outcome | A durable coding session with repository awareness, branch management, file artifacts, tool grants, and resumable state across CLI, web, or mobile surfaces. |
| Current alternatives | Claude Code, Codex, Cursor, GitHub Copilot -- none with Polkadot-specific context or chain-aware safety. |
| Pillar | Build, Reach |

#### JTBD-B07: Monitor metadata drift and regenerate clients

| Field | Value |
|---|---|
| Persona | P1, P3 (runtime engineer, dApp developer) |
| Situation | A target chain has been upgraded, and the developer's client code may be stale. |
| Motivation | Detect changes early and regenerate type bindings before users encounter errors. |
| Expected outcome | Automated detection of metadata drift, a proposed code update (branch/PR), passing tests, and human review before merge. Never a direct merge or deploy. |
| Current alternatives | Manual checks, breaking in production, periodic regeneration scripts. |
| Pillar | Build |

#### JTBD-B08: Explain an extrinsic or SCALE-encoded call

| Field | Value |
|---|---|
| Persona | P1, P4, P5 (engineer, user, governance participant) |
| Situation | Encountering an opaque encoded extrinsic in a block explorer, governance proposal, or transaction history. |
| Motivation | Understand what the call does, what accounts and assets are affected, and what the parameters mean. |
| Expected outcome | A human-readable decode with metadata/genesis binding, uncertainty flags for unsupported or ambiguous elements, and distinct handling for valid, stale, wrong-network, and malformed inputs. |
| Current alternatives | Polkadot-JS Apps decode, Subscan, manual SCALE decoding. |
| Pillar | Build (tooling), Act (safety) |

### 8.3 Act pillar JTBD

#### JTBD-A01: Explain Before Sign

| Field | Value |
|---|---|
| Persona | P4 (DOT holder), P5 (governance participant) |
| Situation | About to sign a transaction and wants to understand exactly what it does before authorizing it. |
| Motivation | Avoid signing malicious, confusing, or unintended transactions. |
| Expected outcome | A canonical action card showing decoded call, affected accounts, assets, fees, simulation results (where available), risk flags, and uncertainty -- rendered from metadata-bound data, not model prose. Exact bytes handed to an external signer. |
| Current alternatives | Polkadot Vault decoded display, wallet transaction preview (varies by wallet), reading hex in Polkadot-JS Apps. |
| Pillar | Act |

#### JTBD-A01a: Execute the end-to-end treasury/governance operator workflow

| Field | Value |
|---|---|
| Persona | P5 (treasury/governance participant) |
| Situation | A treasury curator or governance delegate needs to evaluate, prepare, and execute a treasury spend or governance vote, with full evidence before signing. |
| Motivation | Complete the operation with the same confidence as an on-chain expert, without becoming one -- and produce an auditable trail that justifies the decision to other council members or token holders. |
| Expected outcome | A complete run through: research (sourced from chain data) → plain-language explanation → DryRunApi simulation result → typed intent with evidence card → explicit approval step → signing via Vault/Ledger (keys never enter the agent) → submission → finality observation → structured report with full audit trail. First entry point is read-only: connect wallet, see delegations and treasury exposure, get one safe suggested action -- no signing authority needed to begin. |
| Current alternatives | Manual research across Polkassembly/SubSquare, manual preimage reading, Polkadot-JS Apps for execution, spreadsheets for accounting. No integrated evidence trail. |
| Pillar | Act |

#### JTBD-A02: Research a governance proposal

| Field | Value |
|---|---|
| Persona | P5 (governance participant) |
| Situation | A referendum is open and the participant needs to understand the proposal, its track, timing, source call/preimage, and implications. |
| Motivation | Make an informed governance decision without spending hours reading preimages and track mechanics. |
| Expected outcome | A research brief with track, source call/preimage decoded, timing, risk analysis, and uncertainty -- every factual field backed by block/hash evidence. Clear separation between analysis and advocacy. |
| Current alternatives | Polkassembly/SubSquare discussions, manual preimage reading, community summaries. |
| Pillar | Act |

#### JTBD-A03: Execute a cross-chain transfer

| Field | Value |
|---|---|
| Persona | P4 (DOT holder) |
| Situation | Wants to move assets between Polkadot relay chain, Asset Hub, a parachain, or another ecosystem chain. |
| Motivation | Complete the transfer safely with clear understanding of route, fees, and potential failure modes. |
| Expected outcome | Route planning with identified source/destination/asset/fee, simulation where available, clear display of uncertainty and failure conditions, canonical approval, signing, and truthful finality observation. |
| Current alternatives | XCM UIs in wallets (limited), manual XCM construction, bridge UIs. |
| Pillar | Act |

#### JTBD-A04: Set up recurring staking operations

| Field | Value |
|---|---|
| Persona | P4 (DOT holder), P10 (autonomous agent owner) |
| Situation | Wants to automate recurring staking operations: nominating, claiming rewards, rebonding, or adjusting nominations based on performance. |
| Motivation | Maintain optimal staking returns without daily manual intervention. |
| Expected outcome | A configured mandate with scope (staking operations only), budget (fee limits), time boundaries, monitoring, and receipts. Emergency pause/revoke available. |
| Current alternatives | Manual daily/weekly operations, limited third-party staking services. |
| Pillar | Act |

#### JTBD-A05: Coordinate a multisig operation

| Field | Value |
|---|---|
| Persona | P5 (governance participant), P7 (organizational operator) |
| Situation | Multiple signatories need to approve a transaction (treasury disbursement, governance action, fund transfer). |
| Motivation | Coordinate approval across signatories safely, track progress, and execute when threshold is met. |
| Expected outcome | Drafted call with canonical explanation, approval tracking per signatory, deadline awareness, execution upon threshold, and receipt for all parties. No signer exposes keys to the agent. |
| Current alternatives | Manual coordination via chat/email, Polkadot Multisig app, Polkadot-JS Apps multisig UI. |
| Pillar | Act |

#### JTBD-A06: Monitor and respond to chain events

| Field | Value |
|---|---|
| Persona | P6 (individual operator), P10 (autonomous agent owner) |
| Situation | An operator wants to be notified of or automatically respond to specific chain events (governance deadlines, balance changes, slashing events, contract events). |
| Motivation | Timely awareness and response without constant manual monitoring. |
| Expected outcome | Configured triggers with notification or bounded action response, duplicate/reconnect handling, budget controls, and pause/revoke capability. |
| Current alternatives | Custom scripts, third-party monitoring services, manual checking. |
| Pillar | Act |

### 8.4 Reach pillar JTBD

#### JTBD-R01: Interact with an agent through mobile chat

| Field | Value |
|---|---|
| Persona | P4 (DOT holder) |
| Situation | Away from desktop, wants to check account status, review a pending approval, or ask a question about a governance proposal. |
| Motivation | Get useful, trustworthy agent assistance on mobile without a complex desktop setup. |
| Expected outcome | Private, encrypted conversation through the best available mobile surface. Durable message delivery, live progress, canonical action cards for any required approvals, and consistent state with web/CLI views. |
| Current alternatives | PCA chat through Polkadot app, general-purpose chat with API keys. |
| Pillar | Reach |

#### JTBD-R02: Resume a coding session across surfaces

| Field | Value |
|---|---|
| Persona | P1, P2 (developers) |
| Situation | Started a coding session on desktop CLI, wants to check progress or provide input from mobile or web. |
| Motivation | Continuity across surfaces without losing project state, conversation context, or in-progress work. |
| Expected outcome | Same run state, conversation history, artifacts, and pending decisions available across CLI, web, and mobile surfaces. No data loss on surface switch. |
| Current alternatives | PCA supports some cross-surface conversation; most coding tools are single-surface. |
| Pillar | Reach |

#### JTBD-R03: Review and approve pending actions from Inbox

| Field | Value |
|---|---|
| Persona | P4 (DOT holder), P5 (governance participant), P7 (organizational operator) |
| Situation | One or more agents have pending actions requiring approval: transactions, deployments, configuration changes. |
| Motivation | Review all pending approvals in one place with full evidence, make informed decisions, and track outcomes. |
| Expected outcome | An Agent Inbox showing pending approvals, recent activity, receipts, and recovery states -- consistent across web and mobile, with canonical action cards for each pending item. |
| Current alternatives | Email notifications, manual checking of each agent, custom dashboards. |
| Pillar | Reach |

#### JTBD-R04: Deploy and manage an agent

| Field | Value |
|---|---|
| Persona | P6 (individual operator), P7 (organizational operator) |
| Situation | Has configured an agent and wants to deploy it to a target environment (local daemon, Docker, cloud). |
| Motivation | Reliable deployment with health monitoring, cost tracking, and the ability to update, pause, or remove. |
| Expected outcome | Deployed agent with health checks, monitoring, cost dashboard, log access, and operational controls. Same agent definition works across deployment targets. |
| Current alternatives | PCA deployment via SSH/Docker, manual process management. |
| Pillar | Reach |

#### JTBD-R05: Migrate from PCA to Polkagent

| Field | Value |
|---|---|
| Persona | P6 (individual operator), existing PCA users |
| Situation | Currently running PCA bots and wants to migrate to Polkagent without losing state, identity, or breaking existing consumers. |
| Motivation | Better architecture and capabilities without disrupting existing workflows. |
| Expected outcome | State/config/identity import, compatibility routes for existing bridge consumers, behavior contract verification, and rollback capability. Migration is tested and reversible. |
| Current alternatives | N/A (this is a Polkagent-specific migration). |
| Pillar | Reach |

### 8.5 Cross-pillar JTBD

#### JTBD-X01: Install and use a product kit

| Field | Value |
|---|---|
| Persona | P3 (dApp developer), P6 (individual operator) |
| Situation | Wants to quickly start using a specific Polkagent capability (governance research, staking management, contract deployment) without building from scratch. |
| Motivation | Fast time to value with a tested, maintained bundle of capabilities. |
| Expected outcome | Install a versioned product kit, review its capabilities and permissions, configure it for the target environment, and start using it. Kit includes skills, tools, policies, fixtures, and UX assets. |
| Current alternatives | Manual assembly of tools, scripts, and configurations. |
| Pillar | All |

#### JTBD-X02: Publish a skill or tool to the marketplace

| Field | Value |
|---|---|
| Persona | P9 (marketplace publisher) |
| Situation | Has created a useful skill, tool, or product kit and wants to make it available to others. |
| Motivation | Share or monetize a capability through standardized distribution. |
| Expected outcome | Package, sign, test, and publish to one or more registries. Discoverable through marketplace with trust signals, capability disclosure, and pricing. Updatable and revocable. |
| Current alternatives | GitHub repositories, npm packages, blog posts. |
| Pillar | All |

#### JTBD-X03: Configure and understand agent permissions

| Field | Value |
|---|---|
| Persona | All personas |
| Situation | Setting up a new agent or reviewing the permissions of an existing one. |
| Motivation | Understand exactly what the agent can do and ensure it matches intended scope. |
| Expected outcome | Clear display of all grants, policies, action family permissions, account/asset scope, budget limits, and time boundaries. Plain-language explanation of consequences. Safe defaults that require explicit expansion. |
| Current alternatives | Manual configuration file review, no standardized permission model. |
| Pillar | All |

**Section 8 acceptance criteria:**
- [ ] Every persona has at least one JTBD.
- [ ] JTBDs span all three pillars plus cross-pillar scenarios.
- [ ] Each JTBD has situation, motivation, outcome, and current alternatives.
- [ ] JTBDs are concrete enough to derive acceptance tests from.

---

## 9. Product opportunity portfolio

### 9.1 Portfolio organization

Opportunities are organized by pillar and assigned readiness levels. An opportunity is not a ship commitment; it requires a definitive PRD or accepted ADR to become a requirement. The readiness levels are:

| Readiness | What it permits | Evidence requirement |
|---|---|---|
| **Kernel / invariant** | Carry the constraint into PRDs and ADRs; prototype with shared kernel | Cross-cutting obligation; catalog does not prove implementation |
| **Primitive-ready** | Build an isolated, value-free prototype with fake/local adapters | Cannot be product-ready until family-specific gates pass |
| **Prototype only** | Run a bounded spike or testnet/local fixture | No production write authority, real-value movement, or availability promise |
| **Candidate** | Research, interview users, create evidence-gated proposal | No ship commitment until a PRD accepts it |
| **First candidate slice** | Give early validation priority without making it the whole product | Still passes same evidence gates; cannot demote Build or Reach |
| **Read-only first** | Query, analyze, draft with zero write grant | Write path receives independent gate |
| **Gated** | Design the contract now; implement after named gates pass | Long-term scope does not imply default enablement |
| **Experimental** | Isolate behind research flags and explicit maturity UI | Core correctness cannot depend on it |
| **Research** | Investigate a hypothesis; mock if useful | No product commitment or market claim |
| **Later** | Preserve as phased scope after prerequisite evidence | Sequence is dependency-driven |

### 9.2 Build opportunities

| ID | Opportunity | Readiness | User | Value |
|---|---|---|---|---|
| A1 | Storage-migration rehearsal | Primitive-ready | P1 | Turns risky, expert-only review into inspectable evidence |
| A2 | Upgrade impact brief | Primitive-ready | P1, P3 | Makes upgrade drift legible |
| A3 | XCM planner | Prototype only | P1, P4 | Explains route/fee uncertainty before value moves |
| A4 | Zombienet test orchestrator | Primitive-ready | P1 | Faster cross-chain regression investigation |
| A5 | Contract evidence workbench | Experimental | P2 | Evidence retained with release candidates |
| A6 | Explain an extrinsic | Primitive-ready | P1, P4, P5 | Inspect opaque calls with uncertainty display |
| A7 | Metadata-drift watcher | Candidate | P1, P3 | Reduces client breakage after upgrades |
| A8 | Chain-spec/coretime coach | Candidate | P1 | Guided planning for complex deployment |
| A9 | Metadata-grounded RAG | Primitive-ready | P1, P2, P3 | Fewer stale API hallucinations |
| A10 | Benchmark/weights copilot | Candidate | P1 | Makes performance review easier to audit |

### 9.3 Act opportunities

| ID | Opportunity | Readiness | User | Value |
|---|---|---|---|---|
| B1 | Explain Before Sign | First candidate slice | P4, P5 | Trustworthy bridge from intent to signature |
| B2 | OpenGov research copilot | Read-only first | P5 | Makes governance research less opaque |
| B3 | Intent lifecycle | Primitive-ready | All Act users | Reusable safety flow for many action families |
| B4 | Pre-sign risk gates | Candidate | P4, P5 | Visible defense against deceptive payloads |
| B5 | User-facing XCM preflight | Prototype only | P4 | Better cross-chain decision support |
| B6 | Multisig/proxy coordinator | Candidate | P5, P7 | Coordination without custody |
| B7 | Scheduled or metered payments | Gated | P10 | Useful recurring operations |
| B8 | Receipts and export | Primitive-ready | All | Better reconciliation and audit |
| B9 | Treasury/portfolio research | Read-only first | P5 | Reduces operational search burden |

### 9.4 Reach opportunities

| ID | Opportunity | Readiness | User | Value |
|---|---|---|---|---|
| F1 | Agent Inbox | Candidate | All | One honest view of agent activity |
| F2 | Structured action card | Kernel | P4, P5 | Keeps narrative separate from authority |
| F3 | Live run timeline | Primitive-ready | All | Reduces ambiguous "thinking" states |
| F4 | Time-travel simulation | Later | P6, P8 | Better debugging/what-if analysis |
| F5 | Pre-flight failure card | Candidate | P4 | Helps users avoid surprise |
| F6 | First-run account brief | Read-only first | P4 | Immediate value without custody |
| F7 | Error/recovery explainer | Primitive-ready | All | Less opaque operations |

### 9.5 Autonomy and agent-economy opportunities

| ID | Opportunity | Readiness | User | Value |
|---|---|---|---|---|
| C1 | Funded policy-bounded account | Gated | P10 | Configured autonomy without exposing keys |
| C2 | Agent earns/spends for services | Research | P10 | Future machine-commerce interoperability |
| C3 | Agent-to-agent escrow | Research | P10 | Potential market for verifiable work |
| C4 | Always-on watcher | Candidate | P6, P10 | Timely operations without attention |

### 9.6 Trust, safety, and evidence opportunities

| ID | Opportunity | Readiness | User | Value |
|---|---|---|---|---|
| E1 | Evidence-bearing effects | Kernel | All | Turns "trust the agent" into inspectable evidence |
| E2 | Optional Bulletin/IPFS anchoring | Experimental | P7, P9 | Portable public proof where appropriate |
| E3 | Prompt-injection-visible UX | Candidate | All | Makes a critical safety boundary visible |
| E4 | Replay/debugging | Primitive-ready | P6, P8 | Better incident analysis |
| E5 | Least privilege | Kernel | All | Safe understandable default |

**Prompt injection threat benchmark:** Gray Swan research finds that indirect prompt injection succeeds 4.7% of the time on a single attempt and rises to 63% success over 100 attempts against Opus 4.5. This is not a theoretical threat. Defense-in-depth is mandatory, not optional:

- Evidence cards are rendered from typed canonical data, not from model-generated text that could be poisoned.
- Tool grants are resolved deterministically from policy, not from model-asserted intent.
- Model narrative is visually separated from authoritative data in every surface.
- E3 (prompt-injection-visible UX) exposes injection attempts to the user rather than silently suppressing them.
- No write grant is activated based on model text alone.

These defenses are tracked as kernel invariants, not as a separate "security layer."

### 9.7 Memory, groups, and learning opportunities

| ID | Opportunity | Readiness | User | Value |
|---|---|---|---|---|
| G1 | Provenanced memory | Candidate | P6, P10 | Personalized help without invisible profiles |
| G2 | Worktree-aware context | Candidate | P1, P2 | Safer builder continuity |
| G3 | Grant-intersection group | Later | P7 | Composition cannot escalate authority |
| G4 | Feeds/triggers/recipes | Candidate | P6, P10 | Repeatable operations |
| G5 | Evals/self-improvement | Later | P9 | Safer iterative improvement |

### 9.8 Marketplace and extension opportunities

| ID | Opportunity | Readiness | User | Value |
|---|---|---|---|---|
| H1 | Product kits | Candidate | P3, P6 | Concrete route from platform to outcome |
| H2 | Capability-disclosed units | Later | P9 | Permissionless with safe defaults |
| H3 | Self-hosted registries | Candidate | P7, P8 | No forced cloud lock-in |
| H4 | Public agent-service listings | Research | P9 | Future discoverable agent economy |

### 9.9 Identity and reputation opportunities

| ID | Opportunity | Readiness | User | Value |
|---|---|---|---|---|
| I1 | Agent card | Candidate | P9 | Discoverability and provenance |
| I2 | Identity signal | Read-only | P4, P5 | Better counterparty context |
| I3 | Personhood gating | Experimental | P10 | Potential anti-abuse control |
| I4 | Reputation never authorizes | Invariant | All | Avoids "trusted-looking" escalation |

### 9.10 Deployment and operation opportunities

| ID | Opportunity | Readiness | User | Value |
|---|---|---|---|---|
| J1 | Portable local/managed deployment | Architecture requirement | All | User choice without forced lock-in |
| J2 | Fleet workers | Later | P7 | Scalable operations |
| J3 | Offline-safe degradation | Invariant | All | No silent cloud dependency |
| J4 | Regional isolation | Later | P7 | Enterprise deployment control |

### 9.11 Visionary bets

| ID | Opportunity | Readiness | Prerequisite |
|---|---|---|---|
| K1 | Evidence-bearing autonomous organization | Research | Mature C1/G3, governance acceptance, independent security review |
| K2 | Public-good maintenance agent | Research | Transparent funding mandate, evaluation, accountable operator |
| K3 | Public governance research service | Research | Neutrality, anti-manipulation, public review |
| K4 | Coretime operations agent | Research | Reliable cost inputs, strict budget/approval bounds |
| K5 | Attested evaluations | Research | Evaluation integrity, privacy, anti-Sybil design |
| K6 | JAM-native agent service | Research | Stable JAM toolchain/network/API, concrete user workflow |

### 9.12 JAM and PVM opportunities

| ID | Opportunity | Readiness | Description |
|---|---|---|---|
| D1 | Long-running service actors | Experimental | Future service could hold resumable workflow state |
| D2 | Accord-like agreements | Experimental | Future enforceable interaction agreements |
| D3 | Refine/accumulate work receipts | Experimental | Verifiable-work research |
| D4 | PVM escrow/workflow components | Experimental | Bounded market workflow enforcement |

### 9.13 Candidate sequencing

This is dependency order, not calendar commitment:

1. **Safety kernel:** Durable runs/effects/outbox, grants, artifact lineage, fake transport/executor/signer, restart and duplicate-delivery tests. Gate: fault injection shows no duplicate visible or irreversible effect.
2. **One useful Build + Act + Reach proof:** A6/B1 plus one builder fixture and one local CLI/web projection; PCA chat is a separately gated adapter. Gate: pinned metadata, canonical card, external/fake signer, comprehension and recovery tests meet predefined thresholds.
3. **Read-only value expansion:** A1/A2/A9, B2/B9, F3/F7, I2, and PCA C0 compatibility work. Gate: real bounded jobs show repeat use; no source or privacy regressions.
4. **Controlled write expansion:** B4/B6, selected XCM planning, watchers, and product kits. Gate: family-specific schema, simulation, failure, approval, and support evidence exists.
5. **Managed, public, and value-moving tracks:** C1/C2/C3, H2/H4, J2/J4. Gate: security review, custody/reconciliation, abuse/support, legal review, tenant isolation, measured demand, and pause/revoke/exit drills.
6. **Experimental frontier:** D*, I3, K*. Gate: current primary implementation evidence and a discrete spike; no dependency from core correctness.

### 9.14 Portfolio selection rubric

Promote a candidate only when a definitive PRD records:
- User/job hypothesis
- Source/maturity evidence
- Owner
- Architecture and custody boundary
- Failure/unknown states
- Acceptance artifact
- Measurable product threshold
- Stop/narrow condition

A compelling demo without those elements is an experiment, not a product commitment.

### 9.15 Staged recommendations from research synthesis

Research synthesis (research3.md) yields the following staged recommendations. These inform sequencing and resource allocation without creating new calendar commitments.

**Do now:**
- Treasury/governance operator workflow: research → explain → simulate (DryRunApi) → intent + evidence → approve → sign (Vault/Ledger) → submit → finality → report.
- Read-only shadow mode: connect wallet, observe delegations and treasury exposure, get one safe suggested action -- no signing authority required.

**Validate next (build and test before committing):**
- Autonomy dial UX: test whether governance operators can correctly configure and understand bounded-auto within policy + budget, and whether the time-delayed position reduces anxiety sufficiently to enable practical delegation.
- On-chain agent identity via People Chain: validate whether a verifiable on-chain agent identity materially improves trust for treasury operators reviewing agent-proposed actions.

**Defer (do not build until do-now and validate-next produce evidence):**
- Retail/mobile-first persona: passive DOT holders who primarily interact on mobile with no prior Polkadot technical knowledge. The evidence for product-market fit here is thinner and the safety requirements are harder to meet at scale.

**Avoid:**
- "Any"-proxy autonomy: granting an agent an on-chain `Any`-type proxy removes all action-family filtering and defeats bounded-auto safety. Not a supported autonomous configuration.
- Unbounded auto-execution: no budget ceiling, no allowlist, no time delay, no per-action confirmation. May be technically configurable but must be presented with extreme explicit warnings and is not a supported product default.

**Section 9 acceptance criteria:**
- [ ] Every opportunity from the research corpus is cataloged.
- [ ] Readiness levels are assigned and defined.
- [ ] Sequencing is dependency-driven with explicit gates.
- [ ] The promotion rubric prevents premature commitment.

---

## 10. Non-goals and exclusions

### 10.1 What Polkagent is NOT

| Non-goal | Rationale |
|---|---|
| A generic multichain agent framework | Polkadot-native depth before generic breadth (P-02). Support other chains only through properly typed adapters, not as a primary product identity. |
| A wallet | Polkagent orchestrates signing but does not hold keys or replace dedicated wallet software. The signer is always an external, isolated component or service. |
| A block explorer or indexer | Polkagent consumes chain data but does not replace Subscan, SubQuery, or similar data infrastructure. It is a consumer, not a replacement. |
| A governance platform | Polkagent provides governance research tools, not a voting platform. It does not replace Polkassembly or SubSquare. |
| A financial advisor | Polkagent provides information and tooling, not personalized financial, investment, or legal advice. |
| A custodial service | Polkagent never takes custody of user funds. All custody modes keep signing authority external to the agent runtime. |
| A centralized exchange | Polkagent is not a trading platform and does not facilitate speculative trading. |
| An oracle or price feed | Polkagent does not provide authoritative price data or act as an oracle for on-chain smart contracts. |
| A compliance or regulatory solution | Polkagent provides tools that may assist with compliance but is not a compliance product and does not give legal advice. |
| A replacement for existing good tools | Where tools like Zombienet, Chopsticks, Pop CLI, or cargo-contract work well, Polkagent wraps them as typed adapters rather than reimplementing them. |
| A model vendor | Polkagent integrates with model providers but does not train, host, or sell models. |

### 10.2 Explicit exclusions

| Exclusion | Explanation |
|---|---|
| Raw signing keys in model/tool context | Signing keys never enter the model's prompt, tool parameters, or any component that handles model I/O. |
| Automatic grant widening | No mechanism (model output, reputation, marketplace signal, control plane directive) can widen a grant without explicit owner action. |
| Evaluation results as authorization | Evaluation scores may influence discovery or display but never capability resolution. |
| Implicit custody | The platform never silently becomes custodian of funds. |
| Silent cloud dependency | Local agents must function without a cloud control plane. |
| Mandatory central publishing gate | Publication must be permissionless; curation and verification are discovery features, not publication requirements. |
| Calendar-based schedule commitments | Sequencing is dependency-driven, not date-driven. |
| Legal advice | The platform does not provide legal, tax, or regulatory advice. |

### 10.3 Decision posture

- Local/self-hosted and managed operation are equal modes. Managed services may add fleet/backup/observability but must not silently become custodian or read private data-plane content.
- Identity, a Polkadot account, and personhood are optional integrations; local or pseudonymous agents remain valid.
- A marketplace may be permissionless and plural. Safe/verified discovery is a user-experience default, not a central publication veto.
- Fully autonomous configured actions remain in long-term scope, but each action family needs its own authority, signer, limit, recovery, and release gate. Default flows are read-only or explicit approval.
- Do not make claims about competitor absence, payment volumes, incidents, release timelines, or unverified protocol milestones.

**Section 10 acceptance criteria:**
- [ ] Non-goals are explicit and rationale is provided.
- [ ] Exclusions are enumerated with explanations.
- [ ] Decision posture statements prevent common misinterpretations.

---

## 11. Maturity labels

### 11.1 Label definitions

Every feature, integration, capability, and component in the Polkagent platform carries one of the following maturity labels:

| Label | Definition | Evidence requirement | UX treatment |
|---|---|---|---|
| **Established** | An owner-confirmed product or architecture direction that the definitive PRDs must preserve. Design and implementation proceed. | Owner confirmation and PRD acceptance. | Standard product feature; no special labeling needed. |
| **Default** | The recommended out-of-box behavior. A user or operator may select another supported mode. | Same as established, plus usability evidence for the default choice. | Presented as the standard path; alternatives are discoverable but not forced. |
| **Configurable** | A deliberate choice exposed through a safe, understandable UX and a versioned configuration contract. | Both the default and non-default paths have acceptance tests. | Clear UX for the choice, consequence preview, and ability to change. |
| **Phased** | A committed part of the long-term platform, delivered after its dependencies and release gates. | Design is complete; implementation blocked on named prerequisites. | May appear in documentation and roadmap; not available in current release. |
| **Experimental** | A valuable direction that requires research, an implementation spike, and explicit maturity labeling. | A spike or prototype exists; production readiness is not claimed. | Isolated behind feature flags; explicitly labeled as experimental in UI. |
| **Open** | Evidence or a lower-level implementation decision is still required. | Named evidence gap documented. | Not visible to users until resolved. |

### 11.2 Maturity transition rules

- A feature may move from Experimental to Phased when a successful spike, user evidence, and security review exist.
- A feature may move from Phased to Established when all named dependencies and gates are satisfied.
- A feature may move from Established to Default when usability evidence supports the default choice.
- A feature may be rejected (removed from scope) at any maturity level if evidence shows it is not valuable, safe, or feasible.
- Core correctness and ordinary product flows must not depend on Experimental features.
- An Experimental feature that becomes a dependency for an Established feature must first be promoted.

### 11.3 Maturity label assignment

| Component | Current maturity |
|---|---|
| Three-pillar product vision | Established |
| Durable run/effect/artifact kernel | Established |
| Evidence-bearing effects | Established |
| Grant-controlled tools | Established |
| Policy evaluation | Established |
| Rust-first runtime | Established |
| CLI surface | Established |
| Web/PWA surface | Established |
| API/SDK surface | Established |
| Explain Before Sign (B1) | Established (first candidate slice) |
| PCA chat compatibility | Phased |
| Polkadot App/Product SDK integration | Phased |
| Desktop shell | Phased |
| Native mobile client | Phased |
| Marketplace | Phased |
| Multi-tenant cloud control plane | Phased |
| Fleet management | Phased |
| Funded autonomous agent accounts | Phased (gated) |
| Agent-to-agent commerce | Experimental |
| JAM/PVM service integration | Experimental |
| Personhood gating | Experimental |
| Bulletin/IPFS anchoring | Experimental |
| Affect/vitality/evolutionary modules | Open (requires demonstrated product need) |

**Section 11 acceptance criteria:**
- [ ] Every maturity label is defined with evidence requirements and UX treatment.
- [ ] Transition rules between labels are explicit.
- [ ] Current maturity assignments cover major platform components.
- [ ] Core correctness cannot depend on Experimental features.

---

## 12. Success metrics and KPIs

### 12.1 Metric framework

Success metrics are organized by pillar and measured at three levels:

1. **Adoption:** usage, retention, and growth.
2. **Quality:** correctness, safety, and reliability.
3. **Outcome:** value delivered to users.

All metrics require a measurement method and a baseline. Targets are set after the first measurement cycle, not before.

### 12.2 Platform-wide metrics

| ID | Metric | Measurement method | Pillar |
|---|---|---|---|
| M-00 | Active agents deployed | Count of agents with at least one run in trailing 30 days | All |
| M-01 | Runs per day | Count of completed runs across all agents | All |
| M-02 | Effect safety rate | Percentage of effects with complete Intent/Attempt/Outcome chain and no orphaned or duplicated irreversible effects | All |
| M-03 | Unknown outcome preservation | Percentage of unknown outcomes correctly represented (not collapsed to success/failure) | All |
| M-04 | Grant containment | Zero instances of grant widening without explicit owner action (target: 100%) | All |
| M-05 | Surface consistency | Percentage of run states that render identically across CLI, web, and mobile | All |
| M-06 | Deployment portability | Successful export/import/migration between local, self-hosted, and managed environments | All |

### 12.3 Build pillar metrics

| ID | Metric | Measurement method |
|---|---|---|
| M-B01 | Builder session completion rate | Percentage of coding sessions that produce at least one artifact |
| M-B02 | Migration rehearsal coverage | Number of runtime upgrades rehearsed before production deployment |
| M-B03 | Metadata drift detection time | Median time between runtime upgrade and automated drift detection |
| M-B04 | Build tool integration count | Number of distinct Polkadot build tools (Zombienet, Chopsticks, cargo-contract, etc.) available as typed adapters |
| M-B05 | Product kit installations | Count of product kit installations across all users |
| M-B06 | Extrinsic decode accuracy | Percentage of supported extrinsics correctly decoded against known-good test vectors |

### 12.4 Act pillar metrics

| ID | Metric | Measurement method |
|---|---|---|
| M-A01 | Action card comprehension rate | Pre-registered comprehension study pass rate (percentage of users who correctly identify action, recipient, asset, fee, and risk in test scenarios) |
| M-A02 | Pre-sign risk detection rate | Percentage of seeded dangerous transactions (malicious batch, proxy addition, address poisoning) that are flagged |
| M-A03 | False positive rate | Percentage of benign transactions incorrectly flagged as risky |
| M-A04 | Transaction finality tracking | Percentage of submitted transactions with complete finality observation (finalized, failed, or explicitly unknown) |
| M-A05 | Autonomous action safety rate | Zero unauthorized actions outside configured mandates (target: 100%) |
| M-A06 | Receipt completeness | Percentage of actions with complete receipt (intent, evidence, approval, submission, outcome) |
| M-A07 | Recovery success rate | Percentage of unknown/failed states with successful recovery or correct user guidance |

### 12.5 Reach pillar metrics

| ID | Metric | Measurement method |
|---|---|---|
| M-R01 | PCA compatibility pass rate | Percentage of C0/C1 behavior contract tests passing |
| M-R02 | Cross-surface state consistency | Percentage of state views that are identical across CLI, web, and mobile within 5 seconds of update |
| M-R03 | Message delivery reliability | Percentage of messages durably admitted and acknowledged without loss |
| M-R04 | Session resume success rate | Percentage of interrupted sessions that resume correctly with full state |
| M-R05 | Agent Inbox engagement | Percentage of pending approvals reviewed within user-configured SLA |
| M-R06 | Migration success rate | Percentage of PCA-to-Polkagent migrations completed without data loss or behavior regression |

### 12.6 Marketplace and extension metrics

| ID | Metric | Measurement method |
|---|---|---|
| M-K01 | Published extensions | Count of published skills, tools, and product kits |
| M-K02 | Extension installation rate | Count of extension installations per period |
| M-K03 | Extension safety incidents | Count of security or capability violations by marketplace extensions |
| M-K04 | Registry federation count | Number of self-hosted or federated registries active |

### 12.7 Metric governance

- Metrics are reviewed quarterly.
- Targets are set after the first measurement cycle provides a baseline.
- No metric is gamed by reclassifying outcomes (e.g., unknown as failure to improve failure-rate metrics).
- Quality metrics (M-02 through M-04) have zero-tolerance targets for safety-critical invariants.
- Adoption metrics are context-dependent and do not justify shipping unsafe features.

**Section 12 acceptance criteria:**
- [ ] Metrics span all three pillars plus platform-wide concerns.
- [ ] Each metric has a measurement method.
- [ ] Safety metrics have zero-tolerance targets.
- [ ] Metric governance prevents gaming and premature optimization.

---

## 13. Complete glossary

### 13.1 Core platform terms

| Term | Definition |
|---|---|
| **Polkagent** | The proposed Rust-first, Polkadot-native product-engineering and agent platform. |
| **Agent** | A versioned product/runtime definition that combines behavior, execution route, tools/skills, context/memory, surfaces, policy, and deployment requirements. It is not synonymous with one model. |
| **`AgentSpec`** | The portable, versioned declarative record that defines an agent's behavior, routes, capabilities, memory, surfaces, policy references, and deployment requirements. |
| **`ProductSpec`** | A portable, versioned composition of one or more agents plus product-kit assets, workflows, UX, tests, policies, and deployment defaults for a user outcome. |
| **Run** | One durable execution instance, potentially containing model, tool, approval, delivery, and chain effects. |
| **Turn** | A single request-response cycle within a run, typically corresponding to one user message and agent response. |
| **Artifact** | Durable, attributable content or evidence such as a file, diff, plan, decoded call, test result, simulation, receipt, or report. |
| **Event** | An ordered observation of lifecycle or streaming activity. |
| **Effect** | Actual external work such as calling a model, running a tool, sending a message, signing a payload, or submitting a transaction. |
| **`EffectIntent`** | The durable command recorded before the I/O happens. |
| **`EffectAttempt`** | One claim, lease, retry, and idempotency lifecycle for an effect. |
| **`EffectOutcome`** | The immutable success, failure, timeout, cancellation, or unknown result observed for one attempt. |
| **Policy** | Versioned rules that decide what is permitted, denied, approval-gated, routed, or limited. |
| **Grant** | The exact resolved capabilities and limits available to a run or effect. Determined by policy evaluation, not by model text. |
| **Intent** | A typed proposal for a chain action, payment, deployment, message, or other effect. It is not evidence that the effect occurred. |
| **Approval** | An explicit authorization decision for a proposed intent, either human, quorum, external system, or autonomous mandate. |
| **Mandate** | A configured, time-bounded, scope-limited autonomous authority that permits actions without per-action human approval. |

### 13.2 Execution terms

| Term | Definition |
|---|---|
| **Provider** | A connection/service that exposes one or more AI models (e.g., OpenAI, Anthropic, local inference, routing gateway). |
| **Model** | A particular model identity and capability set, distinct from the provider connection. |
| **Executor** | An adapter component that performs one run/turn and emits normalized events. |
| **Harness** | A long-lived coding-agent or agent-framework process/service with sessions, health, cancellation, tools, and its own lifecycle (e.g., Claude Code, Codex, OpenCode). |
| **Tool** | A typed operation an agent may invoke only through an effective grant. |
| **Skill** | A versioned package of instructions, schemas, examples, tests, context, and declared tool/capability requirements. |
| **Product kit** | A reviewed starter bundle for a user outcome that composes agents, skills, policies, UI assets, fixtures, and deployment defaults. |
| **Context** | The assembled input to a model including messages, artifacts, memories, chain evidence, and tool results. |

### 13.3 Transport and surface terms

| Term | Definition |
|---|---|
| **Transport** | A channel through which authenticated input, output, files, edits, and acknowledgements move (e.g., web, API, Polkadot chat). |
| **Surface** | A user-facing presentation medium (CLI, web/PWA, mobile, desktop). |
| **Agent Studio** | The web-based rich configuration, development, and monitoring interface for agents. |
| **Agent Inbox** | The unified view of pending approvals, recent activity, receipts, and agent status across surfaces. |
| **Action card** | A structured, canonical UI element that presents a proposed action with all evidence, separated from model narrative. |
| **Projection** | A read-only view of authoritative kernel state, rendered for a specific surface or consumer. |

### 13.4 Chain and Polkadot terms

| Term | Definition |
|---|---|
| **PCA** | `polkadot-chat-agents`, the existing/reference chat-agent product. |
| **Polkadot SDK** | The Rust framework and libraries used to build Polkadot-compatible runtimes, chains, nodes, and related infrastructure. |
| **JAM** | Join-Accumulate Machine, a developing protocol/computation architecture. |
| **PVM / PolkaVM** | Polkadot Virtual Machine and its implementation/tooling, used for deterministic program execution and emerging contract/product paths. |
| **XCM** | Cross-Consensus Messaging, the format and execution model for interactions across Polkadot consensus systems. Not simply a generic token-send API. |
| **Subxt** | A Rust client library for submitting extrinsics and reading state/events from Substrate/Polkadot SDK-based chains. |
| **PAPI** | Polkadot API, a TypeScript client approach used in browser and application integrations. |
| **Product SDK** | Polkadot application/product integration tooling for building within the Polkadot app ecosystem. |
| **Runtime** | The state-transition logic of a chain, compiled to WASM and executed by nodes. |
| **Pallet** | A modular component of a runtime that encapsulates specific functionality. |
| **Metadata** | The runtime's typed description of callable pallets, arguments, events, and storage. A runtime upgrade can change it. |
| **Profile** | A bound collection of genesis hash, runtime/spec version, metadata hash, block evidence, supported APIs, assets, constants, and verification time for a particular chain state. |
| **Extrinsic** | A signed or unsigned submission to a chain runtime (transaction, inherent). |
| **SCALE** | The binary encoding used by Polkadot SDK runtimes; must be decoded against the relevant runtime metadata. |
| **Finality** | Durable chain agreement that an included block/extrinsic will not be reverted under the protocol's normal model. |
| **Existential deposit (ED)** | The target runtime's minimum balance rule. Must not be hard-coded globally. |
| **OpenGov** | Polkadot's referendum, origins/tracks, delegation, conviction, and enactment governance system. Parameters are runtime-specific. |
| **DryRunApi** | A Polkadot runtime API that allows callers to simulate extrinsic execution against current chain state and retrieve the expected outcome (events, storage changes, fees, errors) without submitting the extrinsic on-chain. A key enabling primitive for evidence-based safety. |
| **Calldata decode** | The process of interpreting SCALE-encoded extrinsic bytes against runtime metadata to produce a typed, human-readable representation of a call, its pallet, its method, and its parameters. A key enabling primitive for evidence-based safety alongside DryRunApi. |
| **Shadow mode** | A read-only operating mode in which the agent connects to the chain, observes state, generates evidence cards, and proposes actions, but holds no write authority. Used for onboarding and validation before signing rights are granted. |

### 13.5 Identity and custody terms

| Term | Definition |
|---|---|
| **Signer** | An isolated component or external wallet that signs an exact canonical payload under account policy. |
| **Custody** | The arrangement for controlling signing keys: external wallet, hardware, proxy/multisig, local encrypted, managed KMS, MPC, or programmable account. |
| **Proxy** | On-chain delegated call authority with type-specific filters. |
| **Multisig** | On-chain multi-party signing arrangement requiring a threshold of signatures. |
| **Agent card** | An optional signed descriptor binding an agent identity to an on-chain account. |
| **Personhood** | An optional external proof of human identity; an input signal, not authorization. |
| **Reputation** | A signal that may influence discovery or display but never capability resolution. |

### 13.6 Deployment and operations terms

| Term | Definition |
|---|---|
| **Data plane** | The runtime that owns conversations, runs, workspaces, effects, artifacts, and execution. |
| **Control plane** | Optional management services for tenancy, fleets, policy distribution, observability, billing, and deployment coordination. |
| **Tenant** | An isolated organizational unit within a managed deployment. |
| **Fleet** | A managed collection of agent workers across environments. |
| **Worker** | A runtime process that executes agent runs. |

### 13.7 Marketplace terms

| Term | Definition |
|---|---|
| **Registry** | A service or repository that stores and indexes published extensions. |
| **Manifest** | A signed declaration of an extension's identity, version, capabilities, permissions, dependencies, and compatibility. |
| **Federation** | The ability for registries to interoperate, mirror, and cross-reference. |
| **Sideload** | Installing an extension directly from a local source without a registry. |

### 13.8 Protocol and standard terms

| Term | Definition |
|---|---|
| **MCP** | Model Context Protocol, one possible tool/integration protocol. |
| **ACP / A2A** | Agent Client Protocol / agent-to-agent protocols; interoperability options that require capability negotiation rather than assumed semantic parity. |
| **RPC** | Remote procedure call. |
| **WASM** | WebAssembly, a possible sandboxed extension format. |
| **DTO** | Data transfer object: a versioned wire-level data shape. |
| **SLO** | Service-level objective for measurable reliability or performance. |
| **W3F** | Web3 Foundation. |

### 13.9 Process terms

| Term | Definition |
|---|---|
| **PRD** | Product requirements document: the normative description of what the product must do, why, for whom, and how success is accepted. |
| **ADR** | Architecture decision record: a documented architecture decision with context, options, decision, and consequences. |
| **JTBD** | Job to be done: the concrete user outcome and situation a product supports. |
| **Spike** | A time-boxed investigation to resolve a specific technical uncertainty. |
| **Gate** | A named set of criteria that must be satisfied before a feature progresses to the next maturity level. |
| **Fixture** | A reproducible test input/output pair used to verify behavior. |

**Section 13 acceptance criteria:**
- [ ] All terms from the research brief Section 0.2 are included.
- [ ] Additional terms introduced in this PRD are defined.
- [ ] Terms are organized by domain for easy reference.
- [ ] Each term has a concise, unambiguous definition.

---

## 14. Cross-document references

### 14.1 PRD dependency map

PRD-01 provides the foundation for all subsequent PRDs. The following table shows how each PRD relates to PRD-01 content:

| PRD | Title | PRD-01 dependencies | What it adds |
|---|---|---|---|
| PRD-02 | Vocabulary, Invariants, System Architecture | Section 5 (design philosophy), Section 13 (glossary) | Rust traits, crate structure, invariant formalization, architecture decision records |
| PRD-03 | Agent/Run/Effect/Graph Execution Model | Section 5.4 (effects), Section 5.7 (request walkthrough) | State machines, lifecycle specifications, graph/workflow model, concurrency |
| PRD-04 | Providers, Models, Harnesses, Tools, Skills | Section 4.6 (P-06 extensibility), Section 13 (execution terms) | Provider traits, model capabilities, harness lifecycle, tool contracts, skill format |
| PRD-05 | Polkadot Chain, SDK, JAM/PVM Integrations | Section 1.5 (Polkadot context), Section 3.2 (Build pillar) | Chain client traits, metadata handling, XCM, contract, JAM adapters |
| PRD-06 | PCA Compatibility, Chat/Mobile, Messaging | Section 1.6 (PCA), Section 3.4 (Reach pillar) | Transport port, PCA behavior contracts, migration schemas, compatibility fixtures |
| PRD-07 | Identity, Accounts, Signers, Policy, Security | Section 4.5 (P-05 autonomy), Section 13 (identity/custody terms) | Identity domain model, signer traits, policy engine, security model |
| PRD-08 | Payments, Autonomous Agents, Economic Controls | Section 3.3 (Act pillar), Section 5.5 (configurable autonomy) | Payment state machine, mandate model, accounting, settlement |
| PRD-09 | Memory, Knowledge, Learning, Multi-Agent, Evals | Section 9.7 (memory opportunities) | Memory schema, knowledge provenance, group model, eval framework |
| PRD-10 | Data, Artifacts, Events, Observability, Recovery | Section 4.4 (P-04 evidence), Section 5.4 (effects) | Storage contracts, event bus, telemetry, backup/restore, SLOs |
| PRD-11 | Self-Hosting, Managed Cloud, Multi-Tenancy | Section 4.3 (P-03 local/cloud equality) | Deployment topologies, tenant model, control plane, portability |
| PRD-12 | Marketplace, Registry, Extension SDK, Product Kits | Section 4.10 (P-09 permissionless extension) | Package format, registry protocol, trust model, sandboxing, fees |
| PRD-13 | UX, CLI, Studio, Inbox, Mobile, Operator Surfaces | Section 4.8 (P-07 progressive disclosure), Section 3.4 (Reach) | Wireframes, information architecture, interaction patterns, accessibility |
| PRD-14 | APIs, Schemas, Configuration, Migration | Section 4.9 (P-08 portability), Section 13 (glossary) | API specifications, schema versions, config format, migration tooling |
| PRD-15 | Testing, Security Assurance, Roadmap, Acceptance | Section 11 (maturity labels), Section 12 (metrics) | Test pyramid, threat model, CI/CD gates, release sequence, acceptance criteria |

### 14.2 Cross-reference rules

- Every PRD must reference the PRD-01 principle(s) it implements.
- When a later PRD extends a persona, it must reference the PRD-01 persona definition.
- When a later PRD adds a JTBD, it must trace to a PRD-01 persona and opportunity.
- When a later PRD introduces a new term, it must be consistent with or explicitly extend the PRD-01 glossary.
- When a later PRD changes a maturity label, it must document the gate that was satisfied.

### 14.3 Research-to-PRD traceability

| Research package | Primary PRD destination(s) |
|---|---|
| A -- Polkadot/product matrix | PRD-05, PRD-07 |
| B -- JAM/PVM | PRD-05 |
| C -- PCA compatibility | PRD-06 |
| D -- Roko synthesis | PRD-02, PRD-03, PRD-09 |
| E -- Execution/providers/harnesses | PRD-03, PRD-04 |
| F -- Product engineering | PRD-05 |
| G -- Payments/autonomy | PRD-08 |
| H -- Identity/reputation | PRD-07 |
| I -- Memory/multi-agent | PRD-09 |
| J -- Cloud | PRD-11 |
| K -- Marketplace/extensions | PRD-12 |
| L -- Product UX | PRD-13 |
| M -- Assurance | PRD-15 |

**Section 14 acceptance criteria:**
- [ ] Every PRD has a documented relationship to PRD-01.
- [ ] Cross-reference rules prevent inconsistency.
- [ ] Research-to-PRD traceability is complete.

---

## 15. Acceptance criteria checklist

### 15.1 PRD-01 completeness verification

This checklist must be satisfied before PRD-01 is considered complete:

**Section 1: Purpose and scope**
- [ ] 1.1 A newcomer can understand what Polkagent is after reading Section 1 alone.
- [ ] 1.2 Source authority and traceability are explicit.
- [ ] 1.3 No claim of existing implementation is made.
- [ ] 1.4 Polkadot context is explained for newcomers.
- [ ] 1.5 Existing codebases (PCA, Roko) are described with their role.

**Section 2: Product vision**
- [ ] 2.1 Vision statement is quotable in a single sentence.
- [ ] 2.2 Long-term narrative distinguishes Polkagent from alternatives.
- [ ] 2.3 Three illustrative experiences cover all pillars.
- [ ] 2.4 The reusable product thesis is stated.
- [ ] 2.5 No implementation detail appears in the vision.

**Section 3: Three equal pillars**
- [ ] 3.1 Each pillar has a purpose statement and detailed scope.
- [ ] 3.2 Build pillar covers all product types with maturity labels.
- [ ] 3.3 Act pillar defines the complete action lifecycle.
- [ ] 3.4 Act pillar enumerates action families with maturity labels.
- [ ] 3.5 Act pillar defines the autonomy model with levels and defaults.
- [ ] 3.6 Reach pillar enumerates PCA compatibility requirements.
- [ ] 3.7 Reach pillar defines surface strategy with maturity labels.
- [ ] 3.8 Pillar interaction and shared infrastructure are explained.
- [ ] 3.9 Safety rules are stated for each pillar.

**Section 4: Core principles**
- [ ] 4.1 Every principle has a unique ID, name, status, and constraints.
- [ ] 4.2 Principles are testable through stated constraints.
- [ ] 4.3 No principle contradicts another.
- [ ] 4.4 Principles cover all stated domains (language, ecosystem, deployment, evidence, autonomy, extensibility, UX, portability, marketplace, narrative/authority, maturity).

**Section 5: Design philosophy**
- [ ] 5.1 Kernel boundary is explicitly defined.
- [ ] 5.2 Leaf adapter constraints are stated.
- [ ] 5.3 Extension lifecycle is described.
- [ ] 5.4 Effect lifecycle is defined with invariant properties.
- [ ] 5.5 Autonomy configuration dimensions are enumerated.
- [ ] 5.6 Layered system view shows clear separation.
- [ ] 5.7 Request walkthrough demonstrates the architecture.

**Section 6: Confirmed owner decisions**
- [ ] 6.1 Every decision from the research brief is reproduced.
- [ ] 6.2 Each decision has a unique ID.
- [ ] 6.3 Interpretation rules prevent misreading.

**Section 7: Target personas**
- [ ] 7.1 At least 9 distinct personas are defined.
- [ ] 7.2 Each persona has identity, tools, pain points, value, pillar, autonomy, and risk.
- [ ] 7.3 Personas span all three pillars.
- [ ] 7.4 The persona summary matrix is scannable.
- [ ] 7.5 Autonomy postures are differentiated and realistic.

**Section 8: Jobs to be done**
- [ ] 8.1 Every persona has at least one JTBD.
- [ ] 8.2 JTBDs span all three pillars.
- [ ] 8.3 Each JTBD has situation, motivation, outcome, and alternatives.
- [ ] 8.4 JTBDs are concrete enough for acceptance test derivation.

**Section 9: Product opportunity portfolio**
- [ ] 9.1 Every opportunity from the research corpus is cataloged.
- [ ] 9.2 Readiness levels are assigned and defined.
- [ ] 9.3 Sequencing is dependency-driven with explicit gates.
- [ ] 9.4 The promotion rubric is stated.

**Section 10: Non-goals and exclusions**
- [ ] 10.1 Non-goals are explicit with rationale.
- [ ] 10.2 Exclusions are enumerated with explanations.
- [ ] 10.3 Decision posture statements prevent misinterpretation.

**Section 11: Maturity labels**
- [ ] 11.1 Every label has a definition, evidence requirement, and UX treatment.
- [ ] 11.2 Transition rules between labels are explicit.
- [ ] 11.3 Current assignments cover major components.

**Section 12: Success metrics**
- [ ] 12.1 Metrics span all pillars and platform-wide concerns.
- [ ] 12.2 Each metric has a measurement method.
- [ ] 12.3 Safety metrics have zero-tolerance targets.
- [ ] 12.4 Metric governance prevents gaming.

**Section 13: Glossary**
- [ ] 13.1 All terms from research brief Section 0.2 are included.
- [ ] 13.2 Additional PRD-01 terms are defined.
- [ ] 13.3 Terms are organized by domain.
- [ ] 13.4 Definitions are unambiguous.

**Section 14: Cross-document references**
- [ ] 14.1 Every PRD (02-15) has a documented relationship.
- [ ] 14.2 Research-to-PRD traceability is complete.
- [ ] 14.3 Cross-reference rules are stated.

**Section 15: Acceptance criteria**
- [ ] 15.1 This checklist exists and covers all sections.
- [ ] 15.2 Each checkable item is binary (pass/fail).
- [ ] 15.3 No item requires subjective judgment without a stated standard.

### 15.2 External verification

Before this PRD is accepted as definitive:

1. **Owner review:** The product owner confirms that the vision, pillars, decisions, and persona priorities match their intent.
2. **Consistency check:** No section contradicts another section within this PRD.
3. **Baseline alignment:** This PRD is consistent with `01-ESTABLISHED-BASELINE.md` on all owner-confirmed decisions.
4. **Glossary completeness:** Every term used in a specialized sense throughout this PRD appears in the glossary.
5. **Forward compatibility:** PRDs 02-15 can cite this document by section and find the referenced content.

---

*End of PRD-01: Vision, Principles, Personas and Product Pillars*

---

## APPENDIX A: DETAILED PERSONA JOURNEY MAPS

This appendix expands the five highest-priority personas (P1, P2, P3, P4, P5) with granular step-by-step journey maps, screen-level TUI touchpoints, before/after pain point analysis, specific measurable success KPIs, and example session transcripts. These journeys are normative inputs for PRD-13 (UX) and PRD-15 (acceptance criteria).

---

### A.1 Persona P1: Runtime/Parachain Engineer Journey

**Scenario:** A parachain engineer needs to write a new pallet, rehearse a storage migration, and prepare a runtime upgrade for the Paseo testnet.

#### Step-by-step workflow (15 steps)

| Step | Action | TUI Screen | Agent Behavior |
|------|--------|------------|----------------|
| 1 | `polkagent init` in existing Polkadot SDK workspace | Welcome / workspace detection view | Agent reads `Cargo.toml`, detects SDK version, workspace layout, and active chain profiles |
| 2 | Agent presents workspace grant request | Grant approval modal | Shows exactly which paths will receive read/write/exec grants; engineer approves scoped grant |
| 3 | Engineer types: "scaffold a nomination-pools tracking pallet" | Run view / streaming output | Agent consults Polkadot SDK metadata, emits plan artifact with proposed pallet structure |
| 4 | Plan artifact reviewed in TUI | Plan tree widget (prd-13, analogous to Roko's `plan_tree.rs`) | Agent shows file list, types, storage items, and estimated weights; engineer can edit before proceeding |
| 5 | Agent creates worktree and writes pallet scaffolding | File diff panel (analogous to Roko's `diff_panel.rs`) | Every file write is an `EffectIntent` → `EffectAttempt` → `EffectOutcome`; diff shown inline |
| 6 | `cargo build --features runtime-benchmarks` via harness | Terminal output panel with streaming lines | Build errors shown with SCALE context and SDK version annotations |
| 7 | Engineer types: "explain this compile error in pallet context" | Conversation panel | Agent decodes error with pallet, storage, and weight context from workspace metadata |
| 8 | Fixes applied; `cargo test -p pallet-nomination-tracker` | Task progress widget (`task_progress.rs` pattern) | Shows test run phases: compile → unit → integration → docs; each task shows `✓`/`✗`/`►` status |
| 9 | Engineer types: "rehearse the storage migration for this pallet" | Evidence workbench screen | Agent forks Paseo state via Chopsticks, runs `try-runtime`, captures pre/post state diff |
| 10 | Migration rehearsal result rendered | Evidence card panel | Canonical decode of storage changes; any data-loss conditions shown in `amber`/`crimson` highlight |
| 11 | Engineer reviews metadata diff | Metadata diff view | Agent shows which call indices, types, constants changed; which downstream clients need regeneration |
| 12 | Engineer prepares upgrade call | Action intent panel | Agent constructs typed `ChainIntent` for `system.setCode`; DryRunApi simulation runs automatically |
| 13 | DryRunApi result shown | Evidence card with simulation badge | Events, expected weight consumption, and storage changes shown; model narrative is visually separated |
| 14 | Engineer approves action | Approval modal (`approval.rs` pattern, border styled with `theme.warning()`) | Exact extrinsic bytes displayed; engineer reviews payload before handoff to external signer |
| 15 | Vault/Ledger signs; submission and finality tracked | Run timeline / finality tracker | Distinct status for: signed → submitted → included → finalized; unknown states never collapsed |

#### Pain points and solutions

| Pain point (today) | Polkagent solution |
|--------------------|--------------------|
| Storage migrations are reviewed manually with no fork-based testing | Automated Chopsticks fork + `try-runtime` rehearsal; pre/post state diff as durable evidence artifact |
| Compile errors require reading SDK internals without Polkadot context | Agent annotates errors with pallet, metadata, and SDK-version-specific context |
| Runtime upgrade impact is discovered after deployment | Metadata diff report generated before any submission; downstream client impact listed |
| Benchmark/weight analysis requires reading FRAME benchmark output manually | Agent parses weights, explains them in terms of extrinsic cost, and flags anomalies |
| Worktree state is lost between CLI sessions | Durable project/worktree model; one SQLite file preserves full session state |

#### Success KPIs for P1

| KPI | Target value | Measurement method |
|-----|-------------|--------------------|
| Time to scaffold a compilable pallet from description | < 8 minutes (p50) | Instrument session start to `cargo build` success in telemetry |
| Storage migration rehearsal pass rate (correct pre/post diff) | > 95% on supported SDK versions | Seeded migration fixtures with known correct diffs |
| Metadata drift detection latency | < 5 minutes after runtime upgrade enactment | Monitor upgrade events on Paseo/Kusama; measure detection timestamp |
| Build tool round-trip latency (cargo → result in TUI) | < 2s overhead above native cargo | Compare wall-clock vs. native cargo in benchmarks |
| Session resume success rate (worktree state intact) | > 99% | Chaos tests: kill process mid-run; measure full resume without data loss |

#### Example session transcript

```
$ polkagent start --workspace . --profile paseo

Polkagent  workspace: /home/eng/parachains/nova-chain
           detected: polkadot-sdk 1.14.0, 4 pallets, target: paseo
           requesting grant: read /home/eng/parachains/nova-chain, write worktrees/, exec cargo

[Grant request — review before approving]
  read   /home/eng/parachains/nova-chain/**
  write  /home/eng/parachains/nova-chain/.roko/worktrees/**
  exec   cargo, zombienet (version-pinned)
[y] approve  [n] reject  [e] edit scope

> y

eng> scaffold a nomination-pools tracking pallet

agent  Planning...
       • pallet-nomination-tracker/src/lib.rs
       • pallet-nomination-tracker/src/benchmarking.rs
       • pallet-nomination-tracker/src/weights.rs
       • pallet-nomination-tracker/src/mock.rs
       • pallet-nomination-tracker/src/tests.rs

       3 storage items proposed: TrackedPools (BTreeMap), LastSynced (BlockNumber), TotalDelegations (u128)
       2 events: PoolTracked, DelegationRecorded
       2 extrinsics: track_pool(pool_id), force_sync(pool_id) [root origin]

       Review plan? [y to proceed / e to edit]

> y

agent  [EffectIntent] write pallet-nomination-tracker/src/lib.rs
       [EffectOutcome] written — 284 lines
       [EffectIntent] cargo check -p pallet-nomination-tracker
       [EffectOutcome] success (0 errors, 3 warnings — unused storage read in force_sync)
       [Artifact] plan-001.json stored

eng> rehearse the storage migration if I add a new field `last_reward_era: EraIndex` to TrackedPools

agent  Forking Paseo at block #4,821,004 via Chopsticks...
       [EffectIntent] chopsticks fork paseo --block 4821004
       [EffectOutcome] fork ready at ws://127.0.0.1:8000

       Running try-runtime on forked state...
       [EffectIntent] cargo run --features try-runtime -- try-runtime --runtime-id nova_runtime
       [EffectOutcome] success

       Migration result:
         StorageVersion: 0 -> 1
         TrackedPools: 42 entries migrated
         Added field `last_reward_era`: initialized to EraIndex(0) for all entries
         Data integrity: OK — no entries lost, no decode failures

       [Artifact] migration-rehearsal-001.json (pre/post state diff, 42 entries)

       NOTE (model narrative): The migration logic looks straightforward. No data loss detected.
       EVIDENCE (canonical): try-runtime exit 0, 42/42 entries migrated, StorageVersion bumped.
```

---

### A.2 Persona P2: Smart Contract / PVM Developer Journey

**Scenario:** A developer is building an ink! contract for a DAO treasury multisig and needs to test, audit for common vulnerabilities, and prepare a deploy to Contracts parachain.

#### Step-by-step workflow (12 steps)

| Step | Action | TUI Screen | Agent Behavior |
|------|--------|------------|----------------|
| 1 | `polkagent start --workspace . --profile contracts-paseo` | Workspace detection | Agent reads `Cargo.toml` for `ink!` dependency, detects `cargo-contract` version |
| 2 | Engineer types: "scaffold a multisig treasury DAO contract in ink!" | Conversation / plan view | Agent generates ink! scaffolding with storage, messages, events, and error types |
| 3 | Agent writes contract files | File diff panel | Each write tracked as durable effect; full diff shown in `diff_panel.rs`-style widget |
| 4 | `cargo contract build --release` | Task progress widget | Build phases shown; any ABI/storage layout warnings annotated with ink! context |
| 5 | Engineer types: "run drink tests for the multisig approve flow" | Terminal output / test results panel | Agent invokes `drink` test suite; each test result shown with `✓`/`✗`/`►` glyph |
| 6 | Engineer types: "check for common ink! vulnerabilities" | Static analysis panel | Agent runs ink!-specific checks: reentrancy patterns, unchecked arithmetic, cross-contract call assumptions |
| 7 | Static analysis results shown | Evidence card | Findings listed with severity (`amber` for warning, `crimson` for critical), exact code location, and recommended fix |
| 8 | Fixes applied; re-run tests | Task progress | All tests pass; agent captures artifact bundle (WASM, ABI, test results, static analysis report) |
| 9 | Engineer types: "simulate deployment to contracts-paseo" | Simulation / evidence workbench | Agent runs `cargo contract instantiate --dry-run`; shows storage deposit, gas estimate, and constructor outcome |
| 10 | Deployment simulation result shown | Evidence card with simulation badge | Exact storage deposit, endowment required, estimated gas; model narrative visually separated |
| 11 | Engineer approves deployment intent | Approval modal | Exact payload bytes shown; approval routes to external signer (Vault or `subkey`-based dev signer) |
| 12 | Contract deployed; address and hash recorded | Run timeline + receipt | Contract address, code hash, block hash, and finality status stored as durable receipt artifact |

#### Pain points and solutions

| Pain point (today) | Polkagent solution |
|--------------------|-------------------|
| No integrated security analysis for ink! contracts | Agent-driven static analysis pass with ink!-specific vulnerability patterns |
| Storage deposit calculation is trial-and-error | Dry-run simulation gives exact deposit before any real funds move |
| Versioned artifact retention is manual | Automatic artifact bundle (WASM, ABI, test results) stored per run |
| PVM tooling documentation is sparse | Agent provides PVM-specific context as tooling matures (Experimental maturity path) |

#### Success KPIs for P2

| KPI | Target value | Measurement method |
|-----|-------------|--------------------|
| Contract build round-trip latency overhead | < 3s above native `cargo contract build` | Benchmark comparison |
| Static analysis false negative rate (on seeded vulnerable contracts) | < 5% | Seeded test corpus with known vulnerability types |
| Deployment simulation accuracy (predicted vs. actual storage deposit) | Within 5% of on-chain value | Compare dry-run estimate to actual contract instantiation |
| Artifact bundle completeness (all required artifacts present) | 100% | Artifact presence check in CI/CD integration tests |

#### Example session transcript

```
eng> check for common ink! vulnerabilities in lib.rs

agent  Analyzing pallet-dao-treasury/lib.rs against ink! vulnerability patterns...

       [EffectIntent] static-analysis: ink! vulnerability scan
       [EffectOutcome] scan complete — 3 findings

       Findings (canonical):
       ┌─────────────────────────────────────────────────┐
       │ FINDING-001  SEVERITY: WARNING                  │
       │ Pattern: Unchecked arithmetic in approve()      │
       │ Location: lib.rs:142  self.approvals += 1       │
       │ Risk: Overflow in u32 approval counter          │
       │ Fix: use saturating_add or checked_add          │
       ├─────────────────────────────────────────────────┤
       │ FINDING-002  SEVERITY: INFO                     │
       │ Pattern: Cross-contract call without gas limit  │
       │ Location: lib.rs:198  treasury_ref.withdraw()   │
       │ Risk: OOG on callee may propagate unexpectedly  │
       │ Fix: Forward explicit gas_limit parameter       │
       └─────────────────────────────────────────────────┘

       NOTE (model narrative): These are common issues in multisig contracts.
       EVIDENCE (canonical): 2 findings from pattern scan; 0 critical vulnerabilities detected.
```

---

### A.3 Persona P3: dApp / Frontend Developer Journey

**Scenario:** A frontend developer needs to integrate Polkadot balance queries and a transfer UI into a Next.js app, and detect when a runtime upgrade breaks their client.

#### Step-by-step workflow (10 steps)

| Step | Action | TUI Screen | Agent Behavior |
|------|--------|------------|----------------|
| 1 | `polkagent start --workspace . --profile polkadot-hub` | Workspace detection | Agent reads `package.json`, detects PAPI version, TypeScript config |
| 2 | Developer types: "generate typed balance query hooks for Polkadot Hub" | Conversation / plan view | Agent fetches Hub metadata, generates PAPI-typed hooks for `system.account` and asset balances |
| 3 | Generated code shown as diff | File diff panel | Each file tracked as artifact; no write until developer approves |
| 4 | Developer approves file writes | Simple approval (no signing needed — code-only effect) | Files written to workspace; agent registers metadata version in artifact record |
| 5 | Developer types: "set up metadata drift monitoring for polkadot-hub" | Monitor configuration view | Agent creates a feed trigger: watch Hub runtime version, compare to recorded metadata hash |
| 6 | Agent detects runtime upgrade on Hub (simulated or real) | Notification badge in status bar | `amber` status badge: "Metadata drift detected — polkadot-hub spec_version 1200014 → 1200015" |
| 7 | Developer reviews metadata diff | Metadata diff view | Changed call indices, type changes, new/removed storage items; downstream impact on generated code |
| 8 | Agent proposes regenerated client code | File diff panel | New generated types shown as diff against current; developer reviews before accepting |
| 9 | Developer approves regeneration | Simple approval | Updated files written; test suite re-run automatically via configured harness |
| 10 | Test results shown; PR description generated | Evidence card + artifact | Test pass/fail, affected modules, and suggested PR description as durable artifact |

#### Pain points and solutions

| Pain point (today) | Polkagent solution |
|--------------------|-------------------|
| Runtime upgrades silently break generated types | Metadata drift monitor with real-time detection and diff |
| API type generation is fragile and requires manual re-runs | Automated regeneration proposal on drift detection |
| Polkadot-specific patterns (SCALE, ED) require specialist knowledge | Agent provides context-aware explanations inline |

#### Success KPIs for P3

| KPI | Target value | Measurement method |
|-----|-------------|--------------------|
| Metadata drift detection latency | < 5 minutes from upgrade enactment | Monitor real upgrade events on test networks |
| Generated client code compatibility with detected metadata version | 100% compile success on first generation | Integration tests against pinned metadata versions |
| Time to first working balance query from a blank workspace | < 15 minutes | User study with 5 frontend developers |

---

### A.4 Persona P4: DOT Holder and Polkadot User Journey

**Scenario:** A DOT holder wants to understand their staking position, review a pending referendum that would affect validator rewards, and execute a cross-chain transfer from Polkadot Hub to Asset Hub.

#### Step-by-step workflow (14 steps)

| Step | Action | TUI Screen | Agent Behavior |
|------|--------|------------|----------------|
| 1 | `polkagent start --read-only --account 12abc...` | Welcome / shadow mode banner | Agent connects read-only; no signing keys entered; clear banner: "Shadow mode — read only" |
| 2 | Agent displays account brief | Account overview panel | Balances (free, reserved, staked), active nominations, pending rewards — all from chain state |
| 3 | User types: "what are my validators doing right now?" | Conversation + data panel | Agent fetches active era exposure, recent points, and slashing risk for each nomination |
| 4 | User types: "explain referendum 847 about validator reward reduction" | Governance research panel | Agent decodes preimage, fetches track parameters, and produces sourced research brief |
| 5 | Research brief shown | Evidence card | Every factual field (track, origin, enactment period) backed by block hash; model analysis clearly separated from canonical data |
| 6 | User types: "should I vote yes or no?" | Conversation (model narrative section) | Agent explicitly declines to advocate; presents tradeoffs with cited sources; model narrative label shown |
| 7 | User types: "vote yes on referendum 847 for me" | Intent panel | Agent drafts `convictionVoting.vote` intent; shows decoded call, conviction, and locked amount |
| 8 | DryRunApi simulation runs | Evidence card with simulation badge | Expected events, storage changes, and fee shown; "simulation succeeded" in jade; any anomalies in amber/crimson |
| 9 | User reviews and approves | Approval modal (centered, `danger`-border for signing actions) | Exact extrinsic bytes shown; user hands payload to wallet |
| 10 | Nova/SubWallet signs transaction | External signer handoff | Agent receives signed payload; never held keys |
| 11 | Submission and finality tracked | Run timeline | Signed → submitted → included → finalized; each step timestamped |
| 12 | User types: "move 50 DOT from my relay chain account to Asset Hub" | XCM intent panel | Agent plans XCM route, estimates fees, identifies destination account format |
| 13 | XCM preflight evidence shown | Evidence card | Route: Polkadot → Asset Hub; fee estimate; failure modes explained; uncertainty flags |
| 14 | User approves; transfer executed | Run timeline + receipt | Full receipt: intent, evidence, approval timestamp, submission hash, finality block |

#### Pain points and solutions

| Pain point (today) | Polkagent solution |
|--------------------|-------------------|
| Transaction details are opaque hex bytes | Canonical decode with metadata binding; plain-language explanation separated from raw data |
| Governance proposals require hours of research | Research brief with sourced factual fields and explicit model/canonical separation |
| Cross-chain transfers are confusing and easy to get wrong | XCM route planner with fee estimate and failure mode explanation before any signing |
| Transaction outcome is ambiguous (included vs. finalized) | Distinct finality tracking: signed, submitted, included, finalized, or explicitly unknown |

#### Success KPIs for P4

| KPI | Target value | Measurement method |
|-----|-------------|--------------------|
| Action card comprehension rate | > 85% (pre-registered study: users identify action, recipient, asset, fee, risk correctly) | User comprehension study with seeded scenarios |
| Dangerous transaction detection rate (malicious batch, proxy addition) | > 99% | Seeded dangerous transaction corpus |
| False positive rate (benign transactions flagged as risky) | < 2% | Seeded benign transaction corpus |
| Shadow-mode to first read-only value | < 3 minutes from account connection | Instrument time to first account brief render |

#### Example session transcript

```
user> what does referendum 847 actually do?

agent  Fetching preimage for referendum 847...

       [EffectIntent] chain read: polkadot referenda preimage for ref 847
       [EffectOutcome] preimage hash 0xabc123... retrieved, size 284 bytes
       [EffectIntent] calldata decode: 0xabc123... against polkadot spec_version 1200014
       [EffectOutcome] decoded

       ── CANONICAL EVIDENCE ───────────────────────────────────────
       Track:      15 (Root)
       Origin:     system.Root
       Call:       staking.setMinNominatorBond { bond: 5_000_000_000_000 }
       Block hash: 0xdef456... (block #21,004,400)
       Current:    2_000_000_000_000 planck (2 DOT)
       Proposed:   5_000_000_000_000 planck (5 DOT)
       ──────────────────────────────────────────────────────────────

       NOTE (model narrative): This proposal raises the minimum nominator bond from 2 DOT
       to 5 DOT. Nominators with less than 5 DOT bonded would become inactive if this passes.
       This affects approximately 12,000 accounts based on on-chain state as of block #21,004,350.
       The analysis above is explanatory — the canonical data above is authoritative.

user> vote yes on it

agent  Preparing vote on referendum 847...

       Action card (canonical):
       ┌─────────────────────────────────────────────────────┐
       │ PROPOSED ACTION                                     │
       │ Pallet:    convictionVoting                         │
       │ Call:      vote                                     │
       │ Ref:       847                                      │
       │ Vote:      Aye, conviction x1                       │
       │ Lock:      12.4 DOT for 28 days                     │
       │ Account:   12abc...xyz (your account)               │
       │ Fee est:   ~0.001 DOT                               │
       │ Simulation: OK (DryRunApi, block #21,004,512)       │
       └─────────────────────────────────────────────────────┘

       NOTE (model narrative): Voting aye with conviction 1x will lock 12.4 DOT for 28 days.

       Review evidence before approving. [y] approve  [n] cancel  [?] explain more
```

---

### A.5 Persona P5: Treasury / Governance Operator Journey

**Scenario (primary first workflow):** A treasury curator needs to evaluate a proposal for a 50,000 DOT developer grant, simulate the treasury spend extrinsic, prepare the approval call through a 3-of-5 multisig, and produce a final audit report for the council.

#### Step-by-step workflow (18 steps)

| Step | Action | TUI Screen | Agent Behavior |
|------|--------|------------|----------------|
| 1 | `polkagent start --shadow --profile polkadot` | Welcome / shadow mode | Agent connects wallet read-only; no signing authority established |
| 2 | Agent shows treasury exposure brief | Treasury overview panel | Open proposals, pending spends, treasury balance, and curator's delegation position |
| 3 | Agent suggests: "review treasury proposal #142 (50k DOT)" | Suggestion card | Single safe suggested action shown; derived from open proposals in chain state |
| 4 | Operator accepts suggestion | Conversation | Agent begins research workflow |
| 5 | Agent researches proposal #142 | Governance research panel | Fetches preimage, proposal submitter history, beneficiary address history, and referenced milestones |
| 6 | Research brief shown | Evidence card | Every claim backed by block hash; submitter identity chain from People Chain (if available) |
| 7 | Operator types: "flag any risk factors for this proposal" | Evidence card (risk section) | Agent checks: beneficiary address age, prior treasury interactions, preimage consistency, spending track alignment |
| 8 | Risk flags shown | Evidence card (amber/crimson annotations) | Specific risks labeled with severity; model analysis clearly separated from canonical data |
| 9 | Operator types: "simulate the treasury approval extrinsic" | Simulation / evidence workbench | Agent constructs `treasury.approveProposal` call and runs DryRunApi simulation |
| 10 | DryRunApi result shown | Evidence card with simulation badge | Expected spend, treasury balance change, events, and finality path shown |
| 11 | Operator reviews and approves the intent | Approval modal | Exact extrinsic bytes shown; operator approves routing to multisig workflow |
| 12 | Agent prepares multisig call data | Multisig coordinator panel | Shows multisig address, threshold (3 of 5), current approvals (0), call hash |
| 13 | Operator shares call hash with co-signatories via external channel | (External) | Agent produces formatted message with call hash and deadline for distribution |
| 14 | Co-signatories submit approvals (tracked in agent state) | Multisig status panel | Real-time tracking: "1/3 approved", "2/3 approved"; each signatory shown with block timestamp |
| 15 | Threshold met; agent prepares execution call | Execution intent panel | Agent constructs `multisig.asMulti` to execute; DryRunApi run again at current block |
| 16 | Final approval and signing | Approval modal + external signer | Operator approves; Vault/Ledger signs the exact payload; keys never enter agent |
| 17 | Submission and finality tracked | Run timeline | Signed → submitted → included → finalized; each step shown with block hash |
| 18 | Audit report generated | Report artifact panel | Complete trail: proposal research, risk flags, simulation results, approval timestamps, finality observation |

#### Pain points and solutions

| Pain point (today) | Polkagent solution |
|--------------------|-------------------|
| Referendum/proposal research is manual across multiple sites | Single integrated research workflow with sourced evidence cards |
| Multisig coordination is done over chat/email with no tracking | Integrated multisig coordinator with real-time approval tracking |
| No integrated audit trail connecting research to signing decision | Durable audit trail from research through finality, all in one run |
| Treasury proposals lack standardized risk assessment | Structured risk flag analysis with canonical backing |
| Proxy/multisig confusion causes operational errors | Clear multisig state visualization; threshold tracking; exact payload verification |

#### Success KPIs for P5

| KPI | Target value | Measurement method |
|-----|-------------|--------------------|
| Time from proposal selection to research brief | < 5 minutes for supported proposal types | Instrument workflow from step 4 to step 6 |
| Risk flag detection rate (seeded proposals with known issues) | > 90% | Seeded corpus with beneficiary address anomalies, inconsistent milestones |
| Multisig coordination time reduction vs. manual | > 50% reduction in median time to threshold | Comparative user study with current manual process |
| Audit report completeness (all required fields present) | 100% | Checklist-based artifact validation |
| Shadow-mode to first useful insight | < 3 minutes from wallet connection | Instrument time to first treasury brief render |

#### Example session transcript

```
$ polkagent start --shadow --profile polkadot

Polkagent  shadow mode — read only
           wallet: 12abc...xyz (connected, no signing authority)
           treasury: 38.4M DOT  open proposals: 7  your delegations: 3 active

           Suggested action: review treasury proposal #142 (50,000 DOT)
           [Enter] begin review  [Tab] see all open proposals  [?] help

> [Enter]

agent  Researching treasury proposal #142...

       [EffectIntent] chain read: treasury proposal #142 preimage + metadata
       [EffectOutcome] preimage 0xaaa... decoded, submitter 14xyz... history retrieved

       ── EVIDENCE: PROPOSAL #142 ──────────────────────────────────
       Proposer:    14xyz...abc (account age: 847 days, 2 prior treasury proposals)
       Beneficiary: 16def...ghi (account age: 12 days — NEW ACCOUNT)
       Amount:      50,000 DOT
       Milestone:   "Protocol development — 6 months" (no milestone breakdown provided)
       Preimage:    treasury.spend { amount: 50000000000000000, beneficiary: 16def...ghi }
       Block hash:  0xbbb... (block #21,010,200)
       ──────────────────────────────────────────────────────────────

       ── RISK FLAGS ───────────────────────────────────────────────
       !! AMBER: Beneficiary account is 12 days old with no prior on-chain activity
       !! AMBER: No milestone breakdown provided in preimage; single lump sum
       ✓  OK:    Proposer has history of 2 completed treasury proposals
       ✓  OK:    Amount is within track spending limits for Treasury track
       ──────────────────────────────────────────────────────────────

       NOTE (model narrative): The new beneficiary account and lump-sum structure are
       risk factors common in proposals that require additional due diligence.
       The risk flags above are derived from canonical on-chain data, not speculation.

> simulate the approval extrinsic

agent  Simulating treasury.approveProposal { proposal_id: 142 }...

       [EffectIntent] DryRunApi: treasury.approveProposal(142) at block #21,010,350
       [EffectOutcome] simulation success

       Simulation result (canonical):
       ┌──────────────────────────────────────────────────────┐
       │ SIMULATION — DryRunApi (block #21,010,350)          │
       │ Call:     treasury.approveProposal { id: 142 }      │
       │ Events:   Treasury.Approved { proposal_id: 142 }    │
       │ Fee:      ~0.002 DOT                                │
       │ State:    proposal status: Pending → Approved        │
       │ Spend:    will execute at next spend period (7 days) │
       │ Result:   Success                                   │
       └──────────────────────────────────────────────────────┘
```

---

## APPENDIX B: SUCCESS METRICS FRAMEWORK

This appendix defines exact KPIs with target values for all metric categories. Targets marked `[BASELINE]` are set after the first measurement cycle. Targets marked with specific values are invariants or pre-registered thresholds that must hold regardless of baseline.

---

### B.1 User Acquisition and Retention Metrics

| ID | Metric | Target value | Measurement method | Priority |
|----|--------|-------------|-------------------|----------|
| UA-01 | Weekly active users (WAU) | [BASELINE + growth trajectory] | Count distinct users with at least one run in trailing 7 days | High |
| UA-02 | Monthly active users (MAU) | [BASELINE] | Count distinct users with at least one run in trailing 30 days | High |
| UA-03 | Day-30 retention rate | > 40% | Cohort analysis: users returning within 30 days of first run | High |
| UA-04 | Day-7 retention rate | > 60% | Cohort analysis: users returning within 7 days of first run | High |
| UA-05 | Time to first value (P5/P1 primary path) | < 5 minutes | Instrument: wallet connect → first meaningful output rendered | Critical |
| UA-06 | Shadow mode conversion rate | > 30% | % of shadow-mode sessions that proceed to first approved action within 7 days | High |
| UA-07 | PCA migration completion rate | > 90% | % of initiated migrations that complete without data loss or regression | High |
| UA-08 | Developer onboarding completion (P1/P2) | > 70% | % of developers who complete first build + test cycle within 30 minutes | Medium |
| UA-09 | Net Promoter Score (NPS) | > 40 | Quarterly in-product survey | Medium |

---

### B.2 Agent Execution Metrics

| ID | Metric | Target value | Measurement method | Priority |
|----|--------|-------------|-------------------|----------|
| AE-01 | Run completion rate | > 95% | % of started runs that reach a terminal state (success, failure, cancelled — not stuck) | Critical |
| AE-02 | Run latency p50 (governance research) | < 30 seconds | Instrument from user request to research brief rendered | High |
| AE-03 | Run latency p50 (evidence card generation) | < 10 seconds | Instrument from intent creation to evidence card rendered | High |
| AE-04 | Run latency p95 (any run type) | < 120 seconds | Instrument across run types; p95 must not exceed 2 minutes | High |
| AE-05 | Model call success rate | > 99% | % of model API calls that return a valid response (excluding user-caused errors) | High |
| AE-06 | Tool call success rate | > 98% | % of granted tool calls that complete without error | High |
| AE-07 | Harness session reliability | > 99.5% uptime per session | Measure harness process availability during active sessions | High |
| AE-08 | Effect lifecycle completeness | 100% | Every effect has EffectIntent + at least one EffectAttempt + EffectOutcome | Critical |
| AE-09 | Unknown outcome preservation | 100% | Zero instances of unknown outcomes collapsed to success/failure | Critical |
| AE-10 | Crash recovery success rate | > 99% | % of process crashes that resume without data loss or duplicate effects | Critical |
| AE-11 | Idempotency invariant | 0 violations | Count of irreversible effects duplicated across crash/retry cycles | Critical |

---

### B.3 Safety Metrics

| ID | Metric | Target value | Measurement method | Priority |
|----|--------|-------------|-------------------|----------|
| SF-01 | Grant containment | 0 violations | Count of grants widened without explicit owner action | Critical — zero tolerance |
| SF-02 | Signing key exposure | 0 incidents | Count of signing keys appearing in model context, tool parameters, or logs | Critical — zero tolerance |
| SF-03 | Dangerous transaction detection rate | > 99% | Seeded corpus: malicious batch, proxy addition, address-poisoning transfers | Critical |
| SF-04 | False positive rate (dangerous tx flagging) | < 2% | Seeded corpus of benign transactions that should not be flagged | High |
| SF-05 | Autonomous action containment | 0 violations | Count of actions outside configured mandate executed without approval | Critical — zero tolerance |
| SF-06 | DryRunApi simulation coverage | > 95% of supported action families | % of action families where simulation is attempted before signing | High |
| SF-07 | Evidence card canonical accuracy | 100% match to chain state | Compare decoded fields to direct RPC queries on seeded test vectors | Critical |
| SF-08 | Model narrative / canonical separation | 100% of renders | Every surface must visually distinguish model text from canonical data | Critical |
| SF-09 | Policy evaluation determinism | 100% | Same policy + same input always produces same grant resolution | Critical |
| SF-10 | Prompt injection detection | Visible to user in > 80% of seeded cases | E3 (prompt-injection-visible UX) surfaces injection attempts in test scenarios | High |
| SF-11 | Approval gate bypass rate | 0 violations | Count of actions that bypassed required human approval gate | Critical — zero tolerance |
| SF-12 | Revocation effectiveness | < 1 minute from revoke to effect | Instrument time from revocation command to last autonomous action halted | High |

---

### B.4 Ecosystem Metrics

| ID | Metric | Target value | Measurement method | Priority |
|----|--------|-------------|-------------------|----------|
| EC-01 | Published agents (distinct AgentSpecs) | [BASELINE] | Count in marketplace and self-hosted registries | Medium |
| EC-02 | Published skills | [BASELINE] | Count of distinct versioned skills published | Medium |
| EC-03 | Published tools | [BASELINE] | Count of distinct versioned tools published | Medium |
| EC-04 | Published product kits | [BASELINE] | Count of distinct product kit installations | Medium |
| EC-05 | Marketplace extension installation rate | [BASELINE + growth] | Count of extension installations per 30 days | Medium |
| EC-06 | Registry federation instances | [BASELINE] | Count of self-hosted registries active and federated | Low |
| EC-07 | Extension security incidents | 0 | Count of capability violations or malicious behavior from marketplace extensions | Critical |
| EC-08 | Treasury/governance operator adoption | > 10 active operators within 90 days of first workflow release | Count of P5 personas with at least 3 runs in trailing 30 days | High |
| EC-09 | OpenGov proposals researched via Polkagent | [BASELINE] | Count of referenda for which agent research was performed | Medium |
| EC-10 | Multisig operations coordinated | [BASELINE] | Count of completed multisig workflows | Medium |

---

### B.5 Developer Experience Metrics

| ID | Metric | Target value | Measurement method | Priority |
|----|--------|-------------|-------------------|----------|
| DX-01 | Time to first agent (P1/P2 builder path) | < 30 minutes from blank workspace | User study: time from `polkagent init` to first successful build cycle | High |
| DX-02 | Time to first governance research brief (P5) | < 5 minutes from shadow-mode start | Instrument: wallet connect to first research brief artifact | Critical |
| DX-03 | CLI command success rate (first attempt) | > 90% | % of CLI commands that produce expected output on first invocation | High |
| DX-04 | API call success rate | > 99% | % of valid API calls that return expected response | High |
| DX-05 | Documentation discoverability (users find answer in < 2 queries) | > 80% | In-product help sessions; measure search-to-answer paths | Medium |
| DX-06 | Error message clarity (users successfully self-recover) | > 70% | Support ticket deflection rate; measure sessions with error followed by successful recovery | High |
| DX-07 | PCA compatibility pass rate | > 95% of C0/C1 behavior contract tests | Automated compatibility test suite | High |
| DX-08 | Configuration round-trip accuracy | 100% | Export AgentSpec → import → verify all settings preserved | Critical |
| DX-09 | Cross-surface state consistency | > 99% within 5 seconds | % of run states that render identically across CLI, web, and mobile within 5 seconds of update | High |
| DX-10 | Skill/tool conformance test pass rate | > 98% for published extensions | % of published extensions passing conformance fixture suite | High |

---

## APPENDIX C: COMPETITIVE POSITIONING

This appendix defines Polkagent's precise differentiated position relative to three competitive categories. The goal is not to criticize alternatives but to be explicit about where Polkagent occupies ground no existing tool covers and where it deliberately chooses not to compete.

---

### C.1 General AI Agent Frameworks

**Competitors in scope:** LangChain, CrewAI, AutoGPT, Microsoft AutoGen, LlamaIndex Workflows

#### What they do well

- Rapidly composable multi-agent pipelines with Python-first ergonomics.
- Broad model provider support (OpenAI, Anthropic, Ollama, etc.) with community tooling.
- Large ecosystems of community-built tools, retrievers, and agent patterns.
- Lower barrier to prototype: minimal configuration for a working agent loop.

#### What they lack for Polkadot use cases

| Gap | Why it matters |
|-----|---------------|
| No Polkadot-native domain types | Chain profiles, runtime metadata, XCM locations, governance tracks, and finality states are not first-class concepts. They are treated as generic string/dict parameters. |
| No evidence-bearing safety model | Model assertion that "the transaction succeeded" is treated identically to a chain finality observation. There is no canonical data / model narrative separation. |
| No typed action lifecycle | `send_transaction` as a generic tool with no per-family decoder, simulation, risk model, or acceptance gate. |
| No configurable autonomy with invariant safety | Autonomy is binary or ad-hoc. No structured mandate model, no per-period budget enforcement, no emergency revocation path. |
| Python runtime | For Polkadot-critical logic (signing orchestration, policy evaluation, chain interaction), Python's type system and memory model introduce risks that Rust eliminates. |
| No durable-before-acknowledgement message semantics | Mobile and chat integrations are bolted on; no equivalent to PCA's admission guarantees. |
| No portable state / SQLite-first design | Agent state is typically in-memory or provider-specific. No SQLite-exportable audit trail. |

#### Polkagent's differentiated position vs. general frameworks

Polkagent is not a Python-first prototyping tool. It trades rapid prototype assembly for Polkadot-native correctness, evidence-bearing safety, and Rust-first reliability. A user who wants to chain three LLM calls together with a web search tool should use LangChain. A user who wants an agent that correctly simulates a governance vote extrinsic, separates canonical evidence from model narrative, and routes signing through Ledger should use Polkagent.

---

### C.2 Blockchain-Specific Agent Tools

**Competitors in scope:** Brian (EVM), Coinbase AgentKit, Olas/Autonolas, Fetch.ai, ElizaOS blockchain plugins

#### What they do well

- Purpose-built for on-chain action with native wallet integration.
- Some support configurable autonomy patterns (Olas specifically).
- Coinbase AgentKit has clean separation of signer from agent runtime.
- ElizaOS has broad social/chat surface reach.

#### What they lack for Polkadot use cases

| Gap | Why it matters |
|-----|---------------|
| EVM-centric type models | Polkadot's runtime metadata, SCALE encoding, XCM, governance tracks, proxy types, and pallet call structure are not modeled. Generic ABI/calldata tools do not compose with Polkadot domain concepts. |
| No Polkadot SDK build tooling | None integrate with cargo, Zombienet, Chopsticks, try-runtime, or cargo-contract. |
| No governance research tooling | None have integrated preimage decode, referendum track analysis, or treasury proposal risk flags. |
| No PCA/mobile compatibility | None preserve PCA's mobile chat, durable admission, or encrypted transport semantics. |
| TypeScript/JavaScript runtimes | Policy evaluation, signing orchestration, and chain interaction in JavaScript introduce risks for high-value Polkadot operations. |

#### Polkagent's differentiated position vs. blockchain agent tools

Existing blockchain agent tools assume EVM semantics. Polkadot's type system (SCALE, metadata-bound decoding, XCM versioning, proxy filters, OpenGov tracks) is fundamentally different and requires purpose-built tooling. Polkagent is the first tool designed from the ground up for Polkadot's specific constraints, safety requirements, and developer workflows.

---

### C.3 Polkadot-Specific Tooling

**Competitors in scope:** Polkassembly, SubSquare, Nova Wallet, SubWallet, Talisman, Polkadot-JS Apps, Pop CLI

#### What they do well

- Polkassembly and SubSquare: best-in-class governance discussion and proposal browsing.
- Nova/SubWallet/Talisman: excellent wallet UX with growing governance features.
- Polkadot-JS Apps: comprehensive low-level chain interaction for experts.
- Pop CLI: excellent Polkadot SDK project scaffolding with modern DX.

#### What they lack

| Gap | Why it matters |
|-----|---------------|
| No integrated AI research layer | Governance platforms present raw proposals; analysis is manual. No integrated preimage decode with LLM-assisted plain-language explanation. |
| Wallet-only; no builder surface | Wallets do not integrate with coding harnesses, project management, or storage migration tooling. |
| No evidence-bearing effect lifecycle | Wallet operations produce receipts, but there is no integrated policy evaluation, DryRunApi simulation, or durable audit trail tied to agent reasoning. |
| No configurable autonomy model | No structured mandate/budget/policy system for delegating recurring operations to an autonomous agent. |
| No cross-surface agent state | A governance action in SubSquare has no connection to a builder session in Pop CLI or a mobile notification. |
| No extensible marketplace | Skills, tools, and custom workflows cannot be packaged and distributed through a permissionless registry. |

#### Polkagent's differentiated position vs. Polkadot-specific tooling

Polkagent is not a wallet, not a governance forum, and not a block explorer. It is the layer that connects them: a programmable, auditable, evidence-bearing AI agent layer that can research proposals (using Polkassembly data as a source), prepare extrinsics (better than manual Polkadot-JS Apps), sign through external wallets (Nova, SubWallet, Ledger, Vault), and produce a permanent audit trail. It makes the existing ecosystem programmable rather than replacing it.

#### Summary differentiation matrix

| Capability | General frameworks | Blockchain agents | Polkadot tools | Polkagent |
|-----------|-------------------|-------------------|----------------|-----------|
| Polkadot-native domain types | No | No | Partial | Yes |
| Evidence-bearing safety (canonical vs. narrative separation) | No | Partial | No | Yes |
| Configurable autonomy with invariant safety | No | Partial | No | Yes |
| Build pillar (SDK, pallets, migrations, contracts) | No | No | Partial (Pop CLI) | Yes |
| Governance research + preimage decode | No | No | Partial (SubSquare) | Yes |
| PCA/mobile compatibility | No | No | No | Yes |
| Rust-first runtime | No | No | Partial | Yes |
| Portable state (SQLite, export/import) | No | No | No | Yes |
| Permissionless extensible marketplace | Partial | No | No | Yes |

---

## APPENDIX D: IMPLEMENTATION CHECKLIST

This checklist organizes the concrete engineering tasks needed to implement the vision layer described in PRD-01. Each task has acceptance criteria, a responsible PRD reference, and a dependency group. Groups are ordered by the candidate sequencing from Section 9.13.

Tasks marked `[KERNEL]` are blocking prerequisites. Tasks marked `[PERSONA]` directly unlock a specific persona's primary workflow. Tasks marked `[METRIC]` establish measurement infrastructure.

---

### D.1 Group 1: Core Identity and Kernel Foundation

These tasks implement the durable kernel described in Section 5 and must be complete before any persona workflow can be demonstrated.

- [ ] **D1-01** Define and implement `AgentSpec` schema (versioned, portable, declarative)
  - Acceptance: A spec can be written to disk, read back, and produce identical behavior. Export/import round-trip passes for all required fields.
  - PRD reference: PRD-02, PRD-14
  - Dependency: None

- [ ] **D1-02** Implement `EffectIntent` / `EffectAttempt` / `EffectOutcome` lifecycle in Rust
  - Acceptance: Process crash during an effect never duplicates an irreversible effect. Unknown outcomes are preserved as `Unknown`, not collapsed. Fault-injection tests pass.
  - PRD reference: PRD-03
  - Dependency: D1-01

- [ ] **D1-03** Implement durable run/turn/artifact state machine in SQLite
  - Acceptance: A run survives process restart at any point in its lifecycle. Session resume produces identical state. One SQLite file is the complete agent.
  - PRD reference: PRD-03, PRD-10
  - Dependency: D1-01, D1-02

- [ ] **D1-04** Implement grant resolution and policy evaluation (Cedar or equivalent)
  - Acceptance: Same policy + same input always produces same grant. No model output can modify a grant. Grant widening without explicit owner action is impossible. Policy tests pass.
  - PRD reference: PRD-07
  - Dependency: D1-01

- [ ] **D1-05** Implement calldata decode against pinned runtime metadata (Subxt-based)
  - Acceptance: Decode matches known-good outputs for all supported pallet call families. Unsupported/malformed input produces explicit uncertainty, not a best-guess decode. Test vectors cover at least 20 extrinsic types.
  - PRD reference: PRD-05
  - Dependency: D1-01

- [ ] **D1-06** Implement DryRunApi simulation integration
  - Acceptance: Simulation results match on-chain execution for all supported action families within seeded test scenarios. Simulation failure is surfaced as evidence, not hidden.
  - PRD reference: PRD-05
  - Dependency: D1-05

- [ ] **D1-07** Implement chain profile definition and pinning (genesis hash, spec version, metadata hash)
  - Acceptance: A profile binds a chain name to a specific genesis hash, spec version, and metadata hash. Decode operations refuse to proceed without a valid profile match.
  - PRD reference: PRD-05
  - Dependency: D1-05

- [ ] **D1-08** Implement isolated signer trait with no key material in model/tool context
  - Acceptance: Static analysis confirms no signing key flows into model context or tool parameters. Signer receives only canonical payload bytes.
  - PRD reference: PRD-07
  - Dependency: D1-04

- [ ] **D1-09** Implement evidence card data structure (canonical action card)
  - Acceptance: Card fields are populated only from typed chain data, never from model text. Model narrative is stored separately and labeled distinctly in all renders.
  - PRD reference: PRD-13
  - Dependency: D1-05, D1-06

- [ ] **D1-10** Implement finality observer (track: signed → submitted → included → finalized)
  - Acceptance: Each state transition is recorded with block hash and timestamp. Unknown finality is represented as `Unknown`, never assumed to be success.
  - PRD reference: PRD-05
  - Dependency: D1-02

- [ ] **D1-11** Implement revocation and emergency pause
  - Acceptance: Revoke command halts all autonomous actions for the affected agent within < 1 minute. No action executes after revoke during the test window.
  - PRD reference: PRD-07, PRD-08
  - Dependency: D1-04

- [ ] **D1-12** Implement audit trail persistence and export
  - Acceptance: All grant evaluations, approval decisions, effect outcomes, and finality observations are stored in SQLite and exportable as structured JSON. No field is omitted or summarized.
  - PRD reference: PRD-10
  - Dependency: D1-02, D1-04

---

### D.2 Group 2: Persona Support — P5 and P1 Primary Workflows

These tasks implement the first concrete persona workflows (treasury/governance operator and runtime engineer) on top of the kernel.

- [ ] **D2-01** Implement shadow mode (read-only, no signing authority, account brief on connect)
  - Acceptance: Shadow mode cannot submit any extrinsic. Account brief renders within 3 minutes of wallet connection. Wallet connection does not require granting write authority.
  - PRD reference: PRD-05, PRD-13
  - Dependency: D1-07, D1-09
  - Persona: P5

- [ ] **D2-02** Implement governance research workflow (preimage fetch → decode → research brief)
  - Acceptance: For a given referendum ID, agent produces a research brief with all canonical fields backed by block hash within 5 minutes. Model narrative is labeled and separated.
  - PRD reference: PRD-05, PRD-13
  - Dependency: D1-05, D1-09
  - Persona: P5

- [ ] **D2-03** Implement treasury proposal risk flag analysis
  - Acceptance: Risk flags are derived only from canonical on-chain data (account age, prior interactions, preimage consistency). Model-generated risk assessments are labeled as narrative, not as canonical flags.
  - PRD reference: PRD-05
  - Dependency: D2-02
  - Persona: P5

- [ ] **D2-04** Implement approval modal TUI widget
  - Acceptance: Modal displays agent role, exact command/payload, and keybinding hints. User can approve or reject. No action proceeds without explicit approval. Analogous to `roko-cli/src/tui/modals/approval.rs`.
  - PRD reference: PRD-13
  - Dependency: D1-09
  - Persona: P4, P5

- [ ] **D2-05** Implement multisig coordinator (state tracking, approval count, threshold detection)
  - Acceptance: Agent tracks signatory approvals in real time. Threshold detection triggers execution intent correctly. No signatory's key enters the agent. Co-signatory status shown with block timestamps.
  - PRD reference: PRD-07, PRD-08
  - Dependency: D1-04, D1-08, D1-10
  - Persona: P5, P7

- [ ] **D2-06** Implement full P5 treasury/governance operator workflow end-to-end
  - Acceptance: A new user can complete the full path (shadow → research → simulate → intent → approve → sign → submit → finality → report) for a supported action family on Paseo within a 30-minute session. Each step produces a durable artifact.
  - PRD reference: PRD-01 Section 2.2a, PRD-05
  - Dependency: D2-01 through D2-05
  - Persona: P5

- [ ] **D2-07** Implement workspace detection and grant request modal for P1
  - Acceptance: Agent correctly identifies Polkadot SDK workspace type, SDK version, and available chain profiles. Grant request shows exact file paths and exec permissions before any write.
  - PRD reference: PRD-04, PRD-07
  - Dependency: D1-04
  - Persona: P1

- [ ] **D2-08** Implement Chopsticks/try-runtime integration for storage migration rehearsal
  - Acceptance: Agent forks a target chain state, runs migration, and produces pre/post state diff as a durable artifact. Migration rehearsal never touches live state.
  - PRD reference: PRD-05
  - Dependency: D2-07
  - Persona: P1

- [ ] **D2-09** Implement metadata diff report (runtime upgrade impact analysis)
  - Acceptance: For any two spec versions of the same chain, agent produces a structured diff of changed call indices, types, constants, and storage items. Report is bound to specific metadata hashes.
  - PRD reference: PRD-05
  - Dependency: D1-07
  - Persona: P1, P3

- [ ] **D2-10** Implement task progress widget (analogous to Roko `task_progress.rs`)
  - Acceptance: Widget shows task list with `✓`/`✗`/`►`/`·` status glyphs, phase labels, ETA estimate, and scroll for long lists. ROSEDUST semantic colors applied per status.
  - PRD reference: PRD-13
  - Dependency: D1-03
  - Persona: P1, P2

---

### D.3 Group 3: First Workflow MVP — Evidence, Safety, and CLI Surface

These tasks complete the minimum viable first workflow and establish the CLI/TUI surface.

- [ ] **D3-01** Implement ROSEDUST TUI theme (analogous to `roko-cli/src/tui/theme.rs`)
  - Acceptance: `RosedustTheme` struct with canonical palette, semantic color helpers, progress gradient, and `NO_COLOR` support. All TUI widgets use theme tokens; no hardcoded colors.
  - PRD reference: PRD-13, Appendix F
  - Dependency: None

- [ ] **D3-02** Implement status badge widget (analogous to `status_badge.rs`)
  - Acceptance: `Active`, `Done`, `Error`, `Warning`, `Pending`, `Unknown` badges render with ROSEDUST colors, icon prefixes, and SLOW_BLINK modifier for active/error states.
  - PRD reference: PRD-13
  - Dependency: D3-01

- [ ] **D3-03** Implement wave/progress bar with semantic gradient (analogous to `wave_progress.rs` and `gauge.rs`)
  - Acceptance: Progress bar interpolates from `danger` → `warning` → `success` using OKLab gradient. Confidence gauge maps value to color threshold (same `confidence_color` pattern as Bardo).
  - PRD reference: PRD-13
  - Dependency: D3-01

- [ ] **D3-04** Implement run timeline widget (effect lifecycle visualization)
  - Acceptance: Timeline shows EffectIntent → EffectAttempt → EffectOutcome for each effect in the current run. Event markers use Unicode glyphs (analogous to Bardo `timeline.rs` `RibbonEventType` pattern).
  - PRD reference: PRD-13
  - Dependency: D1-02, D3-01

- [ ] **D3-05** Implement evidence card render (canonical fields + model narrative separation)
  - Acceptance: Canonical fields render in `frost`/`white` text. Model narrative section renders in `mist`/`ghost` with an explicit "NOTE (model narrative)" header. Visual separation is never absent.
  - PRD reference: PRD-13, Appendix F
  - Dependency: D1-09, D3-01

- [ ] **D3-06** Implement CLI `polkagent start`, `polkagent run`, `polkagent status`, `polkagent approve`, `polkagent revoke`
  - Acceptance: Each command produces expected output on first invocation with no prior configuration. Help text describes all required and optional parameters. Error messages include recovery action.
  - PRD reference: PRD-14
  - Dependency: D1-01, D1-03

- [ ] **D3-07** Implement message delivery reliability (durable-before-acknowledgement for inbound requests)
  - Acceptance: Agent acknowledges only after durable state exists. Duplicate delivery on retry does not produce duplicate effects. Test: kill process mid-admit; verify idempotent resume.
  - PRD reference: PRD-06
  - Dependency: D1-02

- [ ] **D3-08** Implement receipt generation (full artifact: intent + evidence + approval + submission + finality)
  - Acceptance: Receipt is generated for every completed action. Receipt contains all required fields (see Section 12.4 M-A06). Receipt is exportable as JSON.
  - PRD reference: PRD-10
  - Dependency: D1-10, D1-12

---

### D.4 Group 4: Metrics Infrastructure

These tasks establish the measurement layer needed to validate all success metrics in Appendix B.

- [ ] **D4-01** Implement telemetry event schema (structured, privacy-respecting run events)
  - Acceptance: Events emitted for: run start, turn start, effect intent, effect outcome, approval, finality observation. Events contain metric-relevant fields only; no PII, no message content.
  - PRD reference: PRD-10
  - Dependency: D1-02

- [ ] **D4-02** Implement safety metric instrumentation (grant containment, key exposure, bypass detection)
  - Acceptance: Grant widening events are counted and alertable. Key material presence in model context is detectable by static analysis. Bypass events are impossible by construction and verifiable.
  - PRD reference: PRD-15
  - Dependency: D1-04, D1-08

- [ ] **D4-03** Implement action card comprehension test fixture suite
  - Acceptance: Fixture suite contains at least 20 seeded scenarios (correct, malicious batch, proxy addition, address-poisoning). Test harness reports detection rate and false positive rate.
  - PRD reference: PRD-15
  - Dependency: D1-09, D3-05

- [ ] **D4-04** Implement DryRunApi simulation accuracy test suite
  - Acceptance: For each supported action family, simulation result is compared to on-chain execution result on a controlled test network. Accuracy measured as field-level match rate.
  - PRD reference: PRD-15
  - Dependency: D1-06

- [ ] **D4-05** Implement session timing instrumentation (time to first value metrics)
  - Acceptance: `time_to_first_account_brief`, `time_to_first_research_brief`, `time_to_first_build_success` events emitted per session. These feed UA-05, DX-02, DX-01 metrics.
  - PRD reference: PRD-15
  - Dependency: D4-01

---

## APPENDIX E: REFERENCE FILE MAP

This appendix maps Polkagent vision concepts to specific implementation files in the Roko and Bardo reference projects. The "Pattern to adopt" column describes what Polkagent should take from each reference, and any adaptations needed for the Polkadot-native context.

---

### E.1 Core Theme and Color System

| Component | Roko file | Bardo file | Pattern to adopt |
|-----------|-----------|------------|-----------------|
| Theme struct (canonical palette) | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/theme.rs` (`RosedustTheme` / `Theme` struct, ROSEDUST constants) | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/palette.rs` | Adopt the `Theme` struct pattern with typed semantic fields (`success`, `warning`, `danger`, `info`). Polkagent extends with Polkadot-specific semantic colors: `finalized` (jade), `unknown` (amber), `rejected` (crimson), `pending` (muted rose). |
| Color math utilities | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/tui/color.rs` (OKLab gradient, `darken`, `lighten`, HSV conversion) | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/design/palette.rs` | Adopt OKLab gradient interpolation for evidence card progress bars. The `gradient_oklab` function from ROSEDUST spec maps directly to `confidence_color` thresholds in Bardo's `gauge.rs`. |
| Design tokens | N/A | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/design/tokens.rs` | Adopt design token pattern for Polkagent: named constants for each semantic state (finality states, autonomy levels, action family types). |
| APCA contrast | `/Users/will/dev/nunchi/roko/roko/docs/v1/12-interfaces/07-rosedust-design-language.md` (APCA reference implementation) | N/A | Adopt APCA contrast verification for all new TUI text/background pairings. Target `|Lc| >= 75` for body text, `>= 45` for non-text. |

---

### E.2 Approval and Safety Modals

| Component | Roko file | Bardo file | Pattern to adopt |
|-----------|-----------|------------|-----------------|
| Approval modal | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/modals/approval.rs` (`render_approval`: centered popup, danger border, agent role + command display, `[y]`/`[n]` keybinds) | N/A | Direct pattern for Polkagent's action approval modal. Adapt: replace "Command" field with structured evidence card fields (pallet, call, recipient, asset, fee, simulation result). Keep `danger` border style for signing actions. |
| Confirm modal | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/modals/confirm.rs` (`ConfirmAction` enum, `render_confirm`, warning border for non-destructive confirmations) | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/navigation/modal.rs` | Adopt `ConfirmAction` enum pattern for Polkagent's `ApprovalAction` enum: `GovVote { ref_id, aye, conviction }`, `TreasuryApprove { proposal_id }`, `Transfer { amount, dest }`, `XcmSend { route, asset }`. |
| Batch review modal | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/modals/batch_review.rs` | N/A | Adapt for Polkagent's multisig coordinator: batch display of co-signatory approvals with status per signatory (pending/approved/rejected). |

---

### E.3 Progress and Status Display

| Component | Roko file | Bardo file | Pattern to adopt |
|-----------|-----------|------------|-----------------|
| Task progress widget | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/task_progress.rs` (`render_task_progress`: status counts, semantic progress bar, scrollable list with `✓`/`✗`/`►`/`·` glyphs) | N/A | Adopt directly for Polkagent's build workflow task tracking (P1/P2). Extend status set with `Simulating`, `AwaitingApproval`, `Finalizing` states. |
| Status badge | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/status_badge.rs` (`StatusBadge` enum, icon prefixes, SLOW_BLINK for active/error) | N/A | Adopt pattern. Polkagent extends with: `Finalized` (jade, `◉`), `Unknown` (amber, `?`), `Rejected` (crimson, `✗`), `AwaitingApproval` (rose, `..`, pulsing). |
| Wave/phase progress bar | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/wave_progress.rs` (proportional wave segments, animated ocean gradient) | N/A | Adopt for Polkagent's run timeline progress: phases are `research`, `simulate`, `approve`, `sign`, `submit`, `finalize`. Each phase gets a proportional segment. |
| Confidence/vitality gauge | N/A | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/widgets/gauge.rs` (`ConfidenceGauge`, `VitalityGauge`, `confidence_color` threshold function, `render_gauge` pattern) | Adopt `confidence_color` threshold pattern for Polkagent's simulation confidence indicator on evidence cards: `>= 0.75` jade, `>= 0.50` info, `>= 0.25` amber, `< 0.25` crimson. |
| Timeline ribbon | N/A | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/widgets/timeline.rs` (`RibbonEventType` with glyph/color, `TimelineEvent`) | Adopt for Polkagent's effect lifecycle timeline. Map event types: `EffectIntent` (`◆`, muted), `EffectAttempt` (`►`, rose), `EffectOutcome::Success` (`▲`, jade), `EffectOutcome::Failed` (`!`, amber), `EffectOutcome::Unknown` (`?`, crimson). |

---

### E.4 Information Architecture and Layout

| Component | Roko file | Bardo file | Pattern to adopt |
|-----------|-----------|------------|-----------------|
| Layered layout | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/layout.rs` | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/layout.rs` | Adopt three-zone layout pattern: header bar (agent identity + status), main content area (context-dependent view), status/footer bar (keybinds + cost + latency). |
| Tab bar | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/tab_bar.rs` | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/widgets/tabs.rs` | Adopt tab pattern for Polkagent's primary navigation: `[Run]` `[Evidence]` `[Approvals]` `[History]` `[Config]`. Active tab uses `rose-bright` accent. |
| Header bar | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/header_bar.rs` | N/A | Adopt pattern: agent name (rose-bright), run status badge, chain profile (muted), autonomy level indicator, and cost counter (ghost text). |
| Diff panel | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/diff_panel.rs` | N/A | Adopt for Polkagent's file diff display in build workflows (P1/P2). Additions in jade, deletions in crimson, context in mist. |
| Scrollable list | N/A | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/widgets/scrolllist.rs` | Adopt scroll pattern for Polkagent's approval queue, history view, and evidence item lists. |

---

### E.5 Polkadot-Specific Components (Novel — No Direct Reference)

These components have no direct analog in Roko or Bardo and must be designed from scratch. The reference files inform their styling but not their content.

| Component | Description | Roko/Bardo design influence |
|-----------|-------------|----------------------------|
| Evidence card | Canonical display of chain action evidence (pallet, call, decoded fields, DryRunApi result, policy decision) | ROSEDUST panel styling from `theme.rs`; clear zone separation (canonical vs. narrative) inspired by Roko's two-type text hierarchy (accent for data, muted for labels) |
| Chain profile indicator | Shows active chain, genesis hash prefix, spec version, and metadata hash fingerprint | Status badge pattern from `status_badge.rs`; badge color reflects chain health (jade = confirmed, amber = stale, crimson = mismatch) |
| Autonomy dial widget | Shows current autonomy level (Observe / Propose / Bounded-auto / Time-delayed) as a segmented selector | Confidence gauge threshold coloring from Bardo's `gauge.rs`; level colors: observe (muted), propose (info), bounded (amber), time-delayed (crimson) |
| Finality tracker | Step-by-step finality progress (signed → submitted → included → finalized / unknown) | Timeline ribbon from Bardo's `timeline.rs`; each step has a distinct glyph and semantic color |
| Multisig status panel | Co-signatory approval matrix with block timestamps | Phase-compact widget pattern from `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/phase_compact.rs` |
| XCM route visualizer | Shows source chain → relay → destination chain with fee at each hop | Wave progress ribbon from `wave_progress.rs` adapted as a hop-by-hop route display |

---

## APPENDIX F: DESIGN PRINCIPLES TO TUI MAPPING

This appendix maps each core principle from Section 4 to specific TUI patterns from the ROSEDUST design language (Roko and Bardo reference implementations). For each principle, the mapping shows: the concrete UI expression, the specific widget or component it manifests as, and the reference file that informs the implementation.

---

### F.1 P-04: Evidence Over Assertion → Evidence Card Widget

**Principle statement (Section 4.5):** Every material action produces durable, attributable evidence. Model narrative is explanatory, never authoritative. Canonical action cards are rendered from typed, metadata-bound data, not model prose.

**TUI manifestation: Evidence card with two-zone layout**

```
┌─ ACTION EVIDENCE ─────────────────────── block #21,004,512 ┐
│ CANONICAL ─────────────────────────────────────────────────│
│ Pallet:     convictionVoting                               │
│ Call:       vote                                           │
│ Ref:        847                                            │
│ Vote:       Aye  conviction x1                             │
│ Lock:       12.4 DOT  28 days                              │
│ Fee est:    ~0.001 DOT                                     │
│ DryRunApi:  OK  (events: Voted, FeeCharged)                │
│ Metadata:   polkadot spec_version 1200014 (pinned)         │
├ NOTE (model narrative) ────────────────────────────────────│
│ Voting aye with conviction 1x will lock 12.4 DOT for 28   │
│ days. The referendum closes in approximately 3 days.       │
└────────────────────────────────────────────────────────────┘
```

**Design rules:**
- Canonical zone: `frost` text (`#e5e7eb` / Roko `theme.fg`) on `twilight` background. Field labels in `ghost` text. No model-generated text in this zone.
- Model narrative zone: `mist` text (`#9ca3af` / Roko `theme.fg_muted`). "NOTE (model narrative)" header in `rose-dim` to make the label unmissable.
- Zone separator: single `─` line in `rose-dim` (`border_style(theme.rose_muted())`).
- Panel border: `rose-dim` standard; `amber` if simulation returned warnings; `crimson` if simulation failed.

**Reference files:**
- Panel border style: `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/modals/approval.rs` (`Block::default().border_style(theme.danger())`)
- Text hierarchy: `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/theme.rs` (`theme.text()` vs `theme.muted()` style helpers)
- Color tokens: `/Users/will/dev/nunchi/roko/roko/docs/v1/12-interfaces/07-rosedust-design-language.md` (frost, mist, ghost, rose-dim definitions)

---

### F.2 P-05: Configurable Autonomy with Invariant Safety → Autonomy Dial + Approval Modal

**Principle statement (Section 4.6):** Autonomy is a dial with four positions: Observe-only, Propose, Bounded-auto within policy + budget, Time-delayed. Approval gate is a visible, blocking UI interaction for per-action approval level.

**TUI manifestation 1: Autonomy dial (segmented status bar element)**

```
Autonomy: [OBSERVE] [PROPOSE] [BOUNDED-AUTO] [TIME-DELAYED]
                    ^^^^^^^^^^^
                    current: PROPOSE
```

**Design rules:**
- Segments: inactive segments in `ghost` text. Active segment in background color of its level:
  - `OBSERVE`: `info` (sapphire — reading/analysis)
  - `PROPOSE`: `mist` (neutral — drafting only)
  - `BOUNDED-AUTO`: `amber` (warning — automated but limited)
  - `TIME-DELAYED`: `crimson` (danger — execute-after-window; most autonomous)
- Rationale: brightness encodes danger level. Observe is calm; time-delayed is bright and alarming. This directly implements "P-07 progressive disclosure" — the user can see the risk level at a glance.

**TUI manifestation 2: Approval modal for per-action gate**

Directly implements `render_approval` from `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/modals/approval.rs`, extended with evidence card fields:

```
┌──────────────── Approval Required ─────────────────────────┐
│                                                            │
│  Agent:    polkagent/governance-operator v1.2              │
│                                                            │
│  Action:                                                   │
│    convictionVoting.vote { ref: 847, aye: true, conv: 1 } │
│    Lock: 12.4 DOT for 28 days                             │
│    Simulation: OK  (DryRunApi block #21,004,512)           │
│                                                            │
│  [y] approve     [n] reject     [?] see full evidence      │
│                                                            │
└────────────────────────────────────────────────────────────┘
```

**Design rules:**
- Border: always `theme.danger()` (crimson) for any action that modifies on-chain state. This is non-negotiable — even low-risk actions use danger border when signing is required.
- Agent line: `theme.muted()` label + `theme.accent_bold()` value (matches Roko `approval.rs` line 36-37 exactly).
- Action lines: `theme.warning()` (amber) — same as Roko `approval.rs` line 48. Amber is "attention required" without the "stop" signal of crimson.
- Keybinds: `[y]` in `theme.success()`, `[n]` in `theme.danger()` (matches Roko pattern exactly).

**Reference files:**
- `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/modals/approval.rs` (direct pattern)
- `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/modals/confirm.rs` (ConfirmAction enum for action-type dispatch)

---

### F.3 P-10: Separation of Narrative from Authority → Visual Zone Enforcement

**Principle statement (Section 4.11):** Rendering must visually distinguish model narrative from canonical data. A model cannot forge card fields. Evidence is authoritative; model prose is explanatory and labeled as such.

**TUI manifestation: Mandatory zone headers and color protocol**

Every evidence panel, research brief, and simulation result must implement the following zone protocol:

| Zone | Header text | Text color | Background | Border |
|------|-------------|-----------|-----------|--------|
| Canonical | `── EVIDENCE ──` or `── CANONICAL ──` | `frost` (`#e5e7eb`) | `twilight` (`#12101a`) | `rose-dim` |
| Simulation | `── SIMULATION ──` + DryRunApi badge | `frost` | `twilight` | `jade` if OK, `amber` if warning, `crimson` if failed |
| Model narrative | `NOTE (model narrative)` | `mist` (`#9ca3af`) | `dusk` (`#1a1726`, slightly elevated) | `ghost` (subtle separator only) |
| Risk flags | `── RISK FLAGS ──` | `amber` for warnings, `crimson` for critical | `twilight` | `amber` |

**Bardo influence:** Bardo's "Law 2: Color is mortality taxonomy" maps directly — every color answers a question about the element's authority level. Rose/frost = alive/authoritative. Mist/ghost = secondary/explanatory. This is not a style choice; it is a safety invariant.

**Reference files:**
- `/Users/will/dev/uniswap/bardo/prd/18-interfaces/rendering/00-design-system.md` Section 3, Law 2 (color as taxonomy)
- `/Users/will/dev/nunchi/roko/roko/docs/v1/12-interfaces/07-rosedust-design-language.md` (text hierarchy: frost/mist/ghost definitions)
- `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/theme.rs` (semantic style helpers: `theme.text()`, `theme.muted()`)

---

### F.4 P-07: Progressive Disclosure → Status Badge and Phase Indicators

**Principle statement (Section 4.8):** Common jobs work with sensible presets. Complexity appears only when needed. Long-running work exposes progress, waiting states, costs, failures, and a definitive final outcome.

**TUI manifestation 1: Status badge on every run**

Directly adopts `StatusBadge` from `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/status_badge.rs`.

Polkagent extensions to the `StatusBadge` enum:

```rust
pub enum AgentStatusBadge {
    // Adopted from Roko
    Active,      // ">> ACTIVE"  rose, SLOW_BLINK
    Done,        // "OK DONE"    jade
    Error,       // "!! ERROR"   crimson, SLOW_BLINK
    Warning,     // "?! WARN"    amber
    Pending,     // ".. PENDING" ghost
    Paused,      // "|| PAUSED"  amber
    // Polkagent-specific
    AwaitApproval, // ">> APPROVE" rose-bright, SLOW_BLINK — blocking on user
    Simulating,    // "~~ SIM"    info
    Finalizing,    // "◉ FINAL"   jade, subtle pulse
    Unknown,       // "?? UNKNWN" amber — finality unknown, never collapsed
    ShadowMode,    // "-- SHADOW" ghost — read-only, no write authority
}
```

**TUI manifestation 2: Finality step display (progressive disclosure of chain events)**

The run timeline shows increasing detail as finality progresses:

```
Step 1 (minimal):  ◆ signed        [ghost]     — not shown until complete
Step 2 (visible):  ► submitted     [muted]     — tx hash available
Step 3 (visible):  ▲ included      [info]      — block hash available
Step 4 (visible):  ◉ finalized     [jade]      — finality confirmed
   or  (visible):  ? unknown       [amber]     — finality cannot be confirmed
   or  (visible):  ✗ failed        [crimson]   — extrinsic failed in block
```

- Step 1 is ghost until signing completes; then promoted to `muted`. Progressive disclosure means users see fewer distracting states early.
- Finality is the moment that gets full jade emphasis — it is the canonical completion signal.

**Reference files:**
- `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/status_badge.rs` (StatusBadge, icon/color/pulse logic)
- `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/widgets/timeline.rs` (`RibbonEventType` glyph/color pattern)

---

### F.5 P-11: Explicit Maturity and Honest Uncertainty → Unknown State Rendering

**Principle statement (Section 4.12):** Unknown outcomes are represented as unknown, not collapsed into success or failure. Experimental features are isolated behind feature flags and explicit UI labeling.

**TUI manifestation 1: Unknown outcome badge**

When finality cannot be confirmed (network issue, timeout, equivocation risk), the status must render as:

```
? UNKNOWN  [amber text, never jade or crimson]
  Finality observation timed out at block #21,004,612.
  Transaction hash: 0xabc...xyz
  Recovery: check Subscan for tx hash, or retry observation.
```

**Design rules:**
- `Unknown` is amber (`theme.warning`), not crimson (`theme.danger`). Crimson implies confirmed failure. Unknown means "we cannot tell" — different semantics.
- Recovery action is always shown. The principle "Errors must describe what happened, what remains safe, and the next recovery action" (P-07) applies equally to unknown states.
- The transaction hash is always included so the user can verify externally. This is a safety invariant.

**TUI manifestation 2: Experimental feature label**

Any TUI screen or capability behind an experimental feature flag renders a persistent label:

```
┌─ [EXPERIMENTAL] XCM Route Planner ─────────────────────────┐
│  Maturity: Experimental                                     │
│  This feature may produce incorrect route calculations.    │
│  Do not rely on fee estimates for high-value transfers.    │
│  Report issues: github.com/polkagent/issues                │
└────────────────────────────────────────────────────────────┘
```

**Design rules:**
- `[EXPERIMENTAL]` prefix in `amber` bold; never hidden or abbreviated.
- Warning text in `mist` (secondary emphasis; not as alarming as crimson, but clearly cautionary).
- The border of experimental screens uses `amber` (warning) rather than `rose-dim` (standard).

**Reference files:**
- `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/status_badge.rs` (StatusBadge::Warning, StatusBadge::Pending as closest analogs)
- `/Users/will/dev/nunchi/roko/roko/docs/v1/12-interfaces/07-rosedust-design-language.md` (amber semantic definition: "Warnings. Approaching thresholds.")

---

### F.6 P-06: Extensibility Without Kernel Bloat → Marketplace View

**Principle statement (Section 4.7):** Extensions declare identity, version, capabilities, permissions, inputs, outputs, effects, costs, and health behavior. Adding an integration should not require modifying unrelated core crates.

**TUI manifestation: Marketplace view (analogous to Roko `marketplace_view.rs`)**

Reference file: `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/views/marketplace_view.rs`

Each extension item in the marketplace list renders:

```
◆ polkadot-governance-skill  v1.2.3  [VERIFIED]
  author: polkagent-team    downloads: 1,204    rating: 4.8
  requires: chain.read, governance.read
  effects: none (read-only skill)
  cost: free
  [Enter] install   [i] details   [/] search
```

**Design rules:**
- `[VERIFIED]` badge in jade. `[UNVERIFIED]` in amber. `[REVOKED]` in crimson. Color encodes trust level.
- `requires:` capability list in `mist`. Critical: `chain.write`, `signer.access`, `exec` capabilities render in `amber` to highlight elevated permissions.
- `effects: none` in ghost (low-emphasis). Any `write` or `exec` effects render in amber for attention.
- Install action requires an approval modal showing full capability declaration before any installation completes.

---

### F.7 P-03: Local and Cloud as Equal Modes → Deployment Mode Indicator

**Principle statement (Section 4.4):** Local/self-hosted and managed cloud are equally intentional. No forced cloud dependency. Data plane follows locally cached policy when control plane is unavailable.

**TUI manifestation: Deployment mode badge in header bar**

```
polkagent v0.1.0  [LOCAL]  polkadot-hub  PROPOSE  cost: $0.02
```

or:

```
polkagent v0.1.0  [CLOUD: eu-west-1]  polkadot-hub  PROPOSE  cost: $0.02
```

or when control plane is unavailable:

```
polkagent v0.1.0  [LOCAL ⚠ cloud-offline]  polkadot-hub  PROPOSE  cost: $0.02
```

**Design rules:**
- `[LOCAL]` badge in `info` (sapphire — information, not warning).
- `[CLOUD: region]` badge in `info`.
- `⚠ cloud-offline` suffix in `amber`. This is the state where "data plane follows locally cached policy." The warning signals degraded capability, not failure.
- Critically: the `[LOCAL ⚠ cloud-offline]` state must never silently widen grants. If a cloud-distributed policy cannot be fetched, the last locally cached policy applies. This must be visible.

**Reference files:**
- `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/header_bar.rs` (header bar layout and badge placement pattern)
- `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/status_badge.rs` (badge style patterns)
