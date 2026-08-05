# PRD-11 — Self-Hosting, Managed Cloud and Multi-Tenancy

> **Implementation note (audited 2026-08-05):** This PRD remains normative,
> but its embedded implementation statements and checklists are not current
> status evidence. Use [STATUS.md](STATUS.md) and
> [IMPLEMENTATION-BACKLOG.md](IMPLEMENTATION-BACKLOG.md) for verified state and
> the dependency-ordered execution queue. Bounded OPS-01 slices now verify the
> canonical single-instance image/Compose boot, non-root user, HTTP health,
> read-only bind-mounted configuration, clean SIGTERM HTTP drain, container
> replacement on the same named volume, and a restart-safe SQLite CLI marker
> in CI. This is not durable API/run recovery: durable API composition,
> worker/effect draining, Postgres, tenant isolation, backup/restore,
> upgrade/rollback, release, and managed-cloud claims remain open.

**Status:** definitive PRD
**Owner:** unassigned
**Last updated:** 2026-07-30
**Depends on:** PRD-02 (vocabulary/invariants), PRD-03 (execution model),
PRD-07 (identity/policy/security), PRD-10 (data/artifacts/events/observability)
**Consumed by:** PRD-12 (marketplace/extensions), PRD-13 (UX/operator surfaces),
PRD-14 (APIs/schemas/configuration), PRD-15 (testing/security assurance)

---

## 1. Purpose and orientation

### 1.1 What this document covers

This PRD defines how Polkagent is deployed, operated, and scaled. It specifies
the deployment architectures for local personal use, self-hosted team or
organization servers, and a managed multi-tenant cloud service. It establishes
the contracts between data plane, execution plane, and control plane so that
an agent behaves identically regardless of where it runs.

A reader unfamiliar with Polkagent should understand:

- **Polkagent** is a Rust-first, Polkadot-native agent platform with three
  equal pillars: Build (help teams create Polkadot products), Act (safely
  prepare and execute on-chain work), and Reach (preserve the private
  mobile/chat experience of the existing `polkadot-chat-agents` product).
- The platform is currently a design corpus, not a production system. This PRD
  establishes the deployment architecture that implementation must follow.
- "Self-hosted" means the user runs Polkagent on hardware they control.
  "Managed cloud" means Polkagent runs as a hosted service with tenant
  isolation. Both are equally intentional product modes.

### 1.2 Why deployment architecture matters early

Deployment decisions shape trust, privacy, cost, and operational complexity.
A cloud-only design forces users to trust a third party with secrets and chat
content. A local-only design prevents teams from collaborating without
infrastructure expertise. Polkagent must avoid both traps by separating
concerns into well-defined planes with portable contracts.

### 1.3 Reader prerequisites

This PRD assumes familiarity with:

- Container images and orchestration (Docker, Kubernetes).
- Basic networking concepts (TLS, DNS, load balancing).
- Database concepts (SQLite, PostgreSQL, migrations).
- Secret management (OS keychains, KMS, HSM).

Domain-specific terms are defined in section 2.

### 1.4 Relationship to other PRDs

| PRD | Interface with this document |
|---|---|
| PRD-02 | Provides canonical vocabulary: agent, run, effect, artifact, grant |
| PRD-03 | Defines execution model that runs inside the execution plane |
| PRD-07 | Defines identity, policy, and security that the control plane distributes |
| PRD-10 | Defines data, artifacts, events that the data plane owns |
| PRD-12 | Marketplace registries may be self-hosted or managed |
| PRD-13 | UX surfaces connect to whichever deployment mode is active |
| PRD-14 | Configuration schemas must work across all deployment modes |

---

## 2. Definitions

| Term | Meaning in this PRD |
|---|---|
| **Data plane** | The runtime layer that owns agent state, conversations, runs, effects, artifacts, secrets, transport sessions, and policy evaluation. Always local to a deployment. |
| **Execution plane** | The layer that schedules and runs agent work: workers, harness processes, tool execution, resource management. Sits between data plane and control plane. |
| **Control plane** | Optional management services for tenancy, fleet coordination, policy distribution, observability aggregation, billing, and deployment lifecycle. Not required for local operation. |
| **Tenant** | An isolated organizational unit in managed or self-hosted multi-tenant deployments. Owns one or more projects, environments, users, and agents. |
| **Worker** | A process or container that executes agent runs. May be local, self-hosted, or managed. |
| **Fleet** | A set of workers and agents managed as a unit by an operator. |
| **Enrollment** | The process by which a data plane registers with a control plane using public-key and mutual TLS authentication. |
| **Desired state** | A signed, versioned configuration revision that the control plane distributes and the data plane may accept or reject. |
| **Portability** | The ability to export all agent state, configuration, and identity from one deployment mode and import it into another. |
| **Degraded mode** | Operation when the control plane is unavailable; the data plane continues using locally cached policy. |
| **Deployment manifest** | An immutable record of image digest, config revision, required secrets, volumes, network policy, and execution profile for a deployment. |

---

## 3. Deployment principle: local and cloud are equal

### 3.1 The equality invariant

**Established:** local/self-hosted and managed multi-tenant cloud are equally
first-class deployment modes. Neither is a fallback, demo, or development-only
configuration. The same `AgentSpec`, policies, package lockfile, run semantics,
effect contracts, and export format apply everywhere.

This principle has concrete consequences:

1. Every feature that works in managed cloud must also work when self-hosted.
   The control plane may add convenience (fleet dashboards, billing, managed
   backups) but must not add correctness that local deployments lack.
2. Every feature that works locally must produce the same observable agent
   behavior when running in managed cloud. A managed worker is an isolated
   data plane, not a different runtime.
3. No silent cloud dependency. If the control plane is unavailable, a data
   plane continues operating with its locally cached policy and reports a
   clear degraded-management state.
4. No forced lock-in. A user can export from managed cloud and import to
   self-hosted (or vice versa) without losing agent identity, history,
   artifacts, or policy.

### 3.2 What equality does not mean

- Feature parity does not require identical operational convenience. Managed
  cloud may offer auto-scaling, managed backups, and fleet dashboards that
  a local install does not replicate automatically.
- Self-hosted does not mean unsupported. Documentation, tooling, and
  community resources must make self-hosting viable.
- Cloud does not mean less secure. Tenant isolation, encryption, and access
  controls may exceed what a casual local deployment achieves.

#### 3.2.1 One binary, config-gated features

The self-hosted binary and the managed cloud binary must be the same
compiled artifact. Features exclusive to cloud (fleet dashboards, managed
backups, billing) are unlocked by configuration and license, not by a
separate code path. A divergent OSS/cloud codebase -- as seen with projects
like GitLab CE/EE or early Supabase -- creates permanent maintenance burden,
breaks the portability guarantee, and erodes trust in the self-hosted
offering. Polkagent avoids this pattern absolutely: there is one binary, one
schema, one protocol. The managed cloud upsell is operational convenience and
shared infrastructure, not proprietary runtime logic.

### 3.3 Decision authority

The data plane always has final authority over what it executes. A control
plane distributes desired configuration; the data plane evaluates, accepts, or
rejects it. A control plane cannot:

- Silently widen an active turn's grant.
- Access data-plane secrets by default.
- Override locally cached policy during an active run.
- Read chat plaintext, bot seeds, session keys, or provider credentials.

---

## 4. Three-plane architecture

### 4.1 Overview

```text
+------------------------------------------------------------------+
|                        USER SURFACES                              |
|  CLI / TUI  |  Agent Studio  |  Agent Inbox  |  Mobile/Chat      |
+------+-------+-------+--------+------+--------+------+-----------+
       |               |               |               |
       v               v               v               v
+------------------------------------------------------------------+
|                      CONTROL PLANE (optional)                     |
|                                                                   |
|  Tenant admin  |  Fleet mgmt  |  Policy dist  |  Billing/metering|
|  Observability |  Marketplace  |  Backup svc   |  Incident mgmt  |
+--------+---------+----------+---------+----------+---------------+
         |                    |                    |
         | desired-state      | health/metrics     | audit events
         | (signed, versioned)|  (redacted)        | (hashed)
         v                    ^                    ^
+------------------------------------------------------------------+
|                      EXECUTION PLANE                              |
|                                                                   |
|  Worker registry  |  Job scheduler  |  Resource mgmt  |  Health  |
|  Auto-scaling     |  Queue mgmt     |  Harness lifecycle         |
+--------+---------+----------+---------+--------------------------+
         |                    |
         v                    v
+------------------------------------------------------------------+
|                       DATA PLANE (required)                       |
|                                                                   |
|  State DB (SQLite/Postgres)  |  Artifact store  |  Secret vault  |
|  Conversations / runs        |  Effects / events |  Transport     |
|  Policy evaluation / grants  |  Chain reads/signer coordination  |
|  Provider/harness execution  |  Local workspaces                 |
+------------------------------------------------------------------+
```

### 4.2 Data plane (required)

The data plane is the only mandatory layer. It is the runtime that owns all
agent state and executes all agent behavior. Every deployment mode -- from a
single binary on a laptop to an isolated managed worker in a cloud cluster --
runs a data plane.

#### 4.2.1 Responsibilities

| Responsibility | Description |
|---|---|
| State persistence | SQLite (local/single-node) or PostgreSQL (multi-node/managed) for conversations, runs, turns, effects, grants, approvals, receipts, and projections. |
| Artifact storage | Local filesystem, object store, or encrypted managed storage for files, diffs, plans, decoded calls, simulations, test results, and generated content. |
| Secret management | Resolve secrets through a configured backend: encrypted file, OS keychain, environment variable, KMS, HSM, or MPC service. Secrets never leave the data plane by default. |
| Transport sessions | Maintain encrypted chat sessions, device channels, Statement Store connections, and bridge/harness communication. |
| Policy evaluation | Evaluate grants from locally cached policy. The data plane is the enforcement point; it does not delegate grant decisions to external services. |
| Provider/harness execution | Connect to AI model providers, manage harness processes, execute tools under resolved grants. |
| Chain interaction | Read chain state, coordinate with signers, submit transactions, watch for finality/inclusion events. |
| Effect lifecycle | Record `EffectIntent` before I/O, track `EffectAttempt` with lease/retry/idempotency, record immutable `EffectOutcome`. |

#### 4.2.2 State ownership boundaries

```text
DATA PLANE OWNS                           DATA PLANE DOES NOT OWN
-----------------------------------------+------------------------------------------
Decrypted messages                        | Other tenants' data
Scoped session keys                       | Control-plane root credentials
State database (SQLite/Postgres)          | Unscoped support access
Artifact plaintext                        | Cross-tenant encryption keys
Provider connection credentials           | Billing/payment processing
Bot identity material                     | Fleet-wide health aggregation
Local workspace files                     | Marketplace catalog (read-only cache OK)
Resolved grants and policy cache          | Tenant membership administration
```

#### 4.2.3 Database schema principles

The data plane database must support:

- **Transactional acceptance**: a message is durably persisted before its
  source transport is acknowledged. Crash between persist and ACK causes
  at-most-once re-delivery, not data loss.
- **WAL mode**: SQLite uses WAL for concurrent read access during writes.
- **Migration versioning**: schema changes are versioned, reversible, and
  applied at startup with a hold-and-check pattern.
- **Tenant column**: in multi-tenant PostgreSQL deployments, every table
  includes a `tenant_id` column with row-level security policies.
- **Export projection**: every table can be projected to a portable export
  format without exposing internal IDs or indexes.

#### 4.2.4 Data plane contract (Rust sketch)

```rust
/// The portable data-plane interface.
/// Every deployment mode implements this trait identically.
pub trait DataPlane: Send + Sync + 'static {
    /// Persist a new run and return its durable ID.
    async fn create_run(&self, spec: &RunSpec) -> Result<RunId>;

    /// Record an effect intent before any I/O.
    async fn record_intent(&self, run: RunId, intent: &EffectIntent) -> Result<IntentId>;

    /// Record the outcome of an effect attempt.
    async fn record_outcome(
        &self,
        intent: IntentId,
        attempt: u32,
        outcome: &EffectOutcome,
    ) -> Result<()>;

    /// Retrieve the current resolved grant for a run.
    async fn resolved_grant(&self, run: RunId) -> Result<Grant>;

    /// Export all state for portability.
    async fn export(&self, filter: &ExportFilter) -> Result<ExportBundle>;

    /// Import state from another deployment.
    async fn import(&self, bundle: &ExportBundle, opts: &ImportOptions) -> Result<ImportReport>;

    /// Health check for the data plane.
    async fn health(&self) -> DataPlaneHealth;
}
```

### 4.3 Execution plane

The execution plane manages how agent work is scheduled, dispatched, and
monitored. In a local deployment it may be as simple as a single-threaded
Tokio runtime. In a managed cloud it becomes a distributed job scheduler with
auto-scaling and resource quotas.

#### 4.3.1 Responsibilities

| Responsibility | Description |
|---|---|
| Worker management | Register, discover, health-check, and decommission workers. |
| Job scheduling | Assign runs to available workers based on resource requirements, affinity, and priority. |
| Queue management | Durable job queues with at-least-once delivery, visibility timeouts, and dead-letter handling. |
| Harness lifecycle | Start, monitor, restart, and stop harness processes (coding agents, framework bridges). |
| Resource management | Track and enforce CPU, memory, disk, and GPU quotas per tenant/agent/run. |
| Auto-scaling | Scale workers up/down based on queue depth, latency, and configured policies. |
| Health monitoring | Heartbeat-based worker health with configurable failure detection and recovery. |

#### 4.3.2 Execution plane modes

| Deployment | Execution plane implementation |
|---|---|
| Local personal | In-process Tokio runtime; single worker; no queue infrastructure. |
| Self-hosted single server | Local daemon with systemd supervision; optional SQLite job queue. |
| Self-hosted cluster | Distributed workers with PostgreSQL-backed job queue or Redis/NATS. |
| Managed cloud | Kubernetes-orchestrated workers with managed queue service. |

#### 4.3.3 Worker contract (Rust sketch)

```rust
/// A worker that can execute agent runs.
pub trait Worker: Send + Sync + 'static {
    /// Unique worker identifier.
    fn id(&self) -> WorkerId;

    /// Worker capabilities and resource limits.
    fn capabilities(&self) -> WorkerCapabilities;

    /// Execute a job and return the result.
    async fn execute(&self, job: Job) -> Result<JobResult>;

    /// Report current health and load.
    async fn health(&self) -> WorkerHealth;

    /// Gracefully drain: finish current work, accept no new jobs.
    async fn drain(&self) -> Result<()>;
}

/// Worker capabilities for scheduling decisions.
pub struct WorkerCapabilities {
    /// Maximum concurrent runs.
    pub max_concurrent: u32,
    /// Available memory in bytes.
    pub available_memory: u64,
    /// Available disk in bytes.
    pub available_disk: u64,
    /// GPU availability.
    pub gpu: Option<GpuInfo>,
    /// Supported harness types.
    pub harness_types: Vec<HarnessType>,
    /// Network access level.
    pub network_access: NetworkAccessLevel,
    /// Region/zone for data locality.
    pub region: Option<Region>,
}
```

#### 4.3.4 Job lifecycle

```text
                  +----------+
                  | CREATED  |
                  +----+-----+
                       |
                       v
                  +----------+
              +-->| QUEUED   |
              |   +----+-----+
              |        |
              |        v
              |   +----------+
              |   | CLAIMED  |  <-- worker claims with lease
              |   +----+-----+
              |        |
              |        v
              |   +----------+
              |   | RUNNING  |  <-- worker renews lease periodically
              |   +----+-----+
              |        |
              |   +----+----+----+
              |   |         |    |
              |   v         v    v
         +----+---+  +------+ +--------+
         |RETRY   |  |DONE  | |FAILED  |
         |(requeue|  +------+ +---+----+
         | with   |              |
         | backoff|              v
         +--------+         +--------+
                             |DEAD    | <-- after max retries
                             |LETTER  |
                             +--------+
```

### 4.4 Control plane (optional)

The control plane provides management services for multi-agent, multi-user,
and multi-tenant deployments. It is entirely optional: a local deployment
operates without it, and a self-hosted team may choose never to run one.

#### 4.4.1 Responsibilities

| Responsibility | Description |
|---|---|
| Tenant administration | Create, configure, suspend, and delete tenants. Manage organizations, projects, environments, users, and service accounts. |
| Fleet management | Register data planes, distribute configuration, coordinate rolling upgrades, health dashboards. |
| Policy distribution | Sign, version, and distribute policy revisions. Track which data planes have applied which revision. |
| Observability aggregation | Collect redacted metrics, health summaries, and audit hashes. No plaintext content. |
| Billing and metering | Track usage dimensions, enforce quotas, generate invoices. |
| Marketplace hosting | Optional: host or federate skill/tool/model registries. |
| Backup coordination | Optional: coordinate encrypted backup schedules and verify integrity. |
| Incident management | Alert routing, escalation, and response coordination. |

#### 4.4.2 What the control plane receives

The control plane receives only what it needs for management. The boundary
is strict:

```text
CONTROL PLANE MAY RECEIVE                 CONTROL PLANE MUST NOT RECEIVE (default)
-----------------------------------------+------------------------------------------
Tenant/org/member metadata                | Chat plaintext
Agent public identity                     | Bot seed / private keys
Desired config/policy revisions           | Session keys
Extension/package digests                 | Provider OAuth refresh tokens
Rollout/health summaries                  | Raw vault/media artifacts
Redacted metrics and audit hashes         | Decrypted messages
Support tickets (user-initiated)          | Tool execution details
Billing/usage counters                    | Artifact content
Agent capability declarations             | Grant resolution internals
```

#### 4.4.3 Control plane trust model

1. **Pull, not push.** The data plane pulls desired state from the control
   plane on a configurable interval. The control plane cannot push arbitrary
   commands to a data plane.
2. **mTLS enrollment.** The data plane initiates enrollment by presenting its
   agent public key over a mutually authenticated TLS connection. The control
   plane issues a short-lived enrollment certificate; subsequent pulls and
   health reports use this certificate for authentication. No enrollment token
   is ever stored in plaintext configuration.
3. **Signed revisions.** Every configuration revision is signed with the
   tenant's management key. The data plane verifies the signature before
   evaluating the revision.
4. **Accept or reject.** The data plane may reject a revision that conflicts
   with its current state, active grants, or local overrides. It reports the
   rejection reason to the control plane.
5. **Graceful degradation.** If the control plane becomes unreachable, the
   data plane continues operating under locally cached policy (see section 8).
   This is not a special mode; it is the default behavior whenever connectivity
   is absent, including during initial deployment before enrollment.
6. **No remote shell.** Fleet controls are limited to: pause, rollout, drain,
   rotate credentials, revoke extensions, disable signer/payout, and request
   encrypted backup. No interactive shell, file access, or process inspection.
7. **Audit trail.** Every control-plane action is logged with actor, tenant,
   timestamp, action, and outcome.

#### 4.4.4 Control plane contract (Rust sketch)

```rust
/// Control plane client interface used by data planes.
pub trait ControlPlaneClient: Send + Sync + 'static {
    /// Enroll this data plane with the control plane.
    async fn enroll(&self, enrollment: &EnrollmentRequest) -> Result<EnrollmentResponse>;

    /// Pull the latest desired configuration revision.
    async fn pull_config(&self, since: ConfigRevision) -> Result<Option<SignedConfigRevision>>;

    /// Report health status.
    async fn report_health(&self, health: &DataPlaneHealth) -> Result<()>;

    /// Report redacted metrics.
    async fn report_metrics(&self, metrics: &RedactedMetrics) -> Result<()>;

    /// Report a configuration acceptance or rejection.
    async fn report_config_status(
        &self,
        revision: ConfigRevision,
        status: ConfigApplyStatus,
    ) -> Result<()>;
}

/// Status of a configuration revision application.
pub enum ConfigApplyStatus {
    /// Revision accepted and applied.
    Applied { at: Timestamp },
    /// Revision rejected with reason.
    Rejected { reason: String, at: Timestamp },
    /// Revision is pending evaluation.
    Pending,
}
```

### 4.5 Plane contracts and boundaries

#### 4.5.1 Data plane <-> execution plane boundary

```text
Data plane provides to execution plane:
  - Run specifications and resolved grants
  - Artifact read/write handles (scoped to run)
  - Secret references (resolved at execution time, not passed as values)
  - Effect recording endpoints
  - Health reporting channel

Execution plane provides to data plane:
  - Job completion/failure notifications
  - Resource utilization metrics
  - Worker health status
  - Harness lifecycle events
```

#### 4.5.2 Data plane <-> control plane boundary

```text
Data plane provides to control plane:
  - Enrollment request (public key, agent identity, capabilities)
  - Health heartbeat (redacted: no content, no secrets)
  - Configuration acceptance/rejection reports
  - Redacted usage metrics
  - Audit event hashes

Control plane provides to data plane:
  - Signed configuration revisions
  - Extension/package manifests and digests
  - Fleet-wide announcements (pause, drain, upgrade)
  - Billing/quota status
```

#### 4.5.3 Cross-plane invariants

These hold regardless of deployment mode:

| ID | Invariant | Verification |
|---|---|---|
| CP-01 | A control plane cannot widen an active run's grant. | Property test: inject a grant-widening revision during an active run; verify it is rejected. |
| CP-02 | A control plane cannot access data-plane secrets by default. | Penetration test: attempt secret retrieval through all control-plane APIs; verify denial. |
| CP-03 | Control-plane unavailability does not prevent data-plane operation. | Disconnect drill: sever control-plane connectivity; verify agent continues with cached policy. |
| CP-04 | Every cross-plane message is authenticated and tenant-scoped. | Protocol test: inject cross-tenant messages; verify rejection and audit logging. |
| CP-05 | Data-plane export produces a complete, importable bundle. | Round-trip test: export, import to a fresh deployment, verify behavioral equivalence. |
| CP-06 | No duplicate visible or irreversible effect after crash/restart. | Fault injection: crash at every effect lifecycle point; verify idempotency. |

---

## 5. Self-hosted deployment

### 5.1 Overview

Self-hosted deployment means the user controls the hardware and network where
Polkagent runs. This ranges from a developer laptop to an organization's
Kubernetes cluster. The common trait is that the user manages the
infrastructure and retains full control over secrets, data, and network access.

### 5.2 Single-binary local agent (CLI + daemon)

The simplest deployment is a single binary that combines the CLI interface and
a background daemon. The single binary is the default and primary deployment
artifact. Managed cloud is the upsell, not the primary target. This ordering
means the self-hosted experience must be polished enough to stand alone, and
every managed-cloud feature must be reachable without it.

#### 5.2.1 Architecture

```text
+-----------------------------------------------------+
|  User workstation / laptop                           |
|                                                      |
|  +------------------+    +-----------------------+   |
|  |  polkagent CLI   |    |  polkagent daemon     |   |
|  |                  |    |                       |   |
|  |  - Commands      +--->|  - Data plane         |   |
|  |  - Configuration |    |  - Execution plane    |   |
|  |  - Status/logs   |<---+  - SQLite state       |   |
|  |  - Doctor        |    |  - Local secrets      |   |
|  +------------------+    |  - Transport sessions  |   |
|                          |  - Worker (in-process) |   |
|                          +-----------------------+   |
|                                    |                 |
|                          +---------+---------+       |
|                          |  ~/.polkagent/    |       |
|                          |  state.db         |       |
|                          |  config.toml      |       |
|                          |  secrets.enc      |       |
|                          |  artifacts/       |       |
|                          |  logs/            |       |
|                          +-------------------+       |
+-----------------------------------------------------+
```

#### 5.2.2 Installation

```bash
# From official release
curl -fsSL https://install.polkagent.dev | sh

# From crates.io
cargo install polkagent

# From source
git clone https://github.com/nicosiatechnologies/polkagent
cd polkagent && cargo build --release

# Verify installation
polkagent --version
polkagent doctor
```

#### 5.2.3 First-run setup

```bash
# Interactive first-run wizard
polkagent init

# Non-interactive with explicit options
polkagent init \
  --mode local-private \
  --provider anthropic \
  --chain polkadot \
  --data-dir ~/.polkagent
```

The first-run wizard guides through:

1. **Mode selection:** local private bot, local builder agent, T3ams workspace
   agent, import PCA, or connect to a managed tenant.
2. **Identity creation:** generate or import agent identity; configure
   account binding if desired.
3. **Provider setup:** configure AI model provider with credential validation.
4. **Transport setup:** configure chat transport if Reach mode is selected.
5. **Health check:** run `polkagent doctor` to validate all connections.

#### 5.2.4 Directory structure

```text
~/.polkagent/
  config.toml                  # Main configuration
  state.db                     # SQLite database (WAL mode)
  state.db-wal                 # WAL file
  state.db-shm                 # Shared memory file
  secrets.enc                  # Encrypted secrets (age/AEAD)
  identity/
    agent.pub                  # Agent public key
    agent.enc                  # Encrypted agent private key
  artifacts/
    <run-id>/                  # Per-run artifact directories
      <artifact-id>.<ext>
  logs/
    polkagent.log              # Structured JSON log
    polkagent.log.1            # Rotated logs
  cache/
    metadata/                  # Chain metadata cache
    extensions/                # Extension/package cache
  workspaces/                  # Managed workspace roots
```

#### 5.2.5 Configuration file

```toml
# ~/.polkagent/config.toml

[agent]
name = "my-builder-agent"
mode = "local-private"              # local-private | local-builder | teams | managed

[data]
dir = "~/.polkagent"
db = "sqlite"                       # sqlite | postgres
max_artifact_size = "100MB"
artifact_retention_days = 90
log_retention_days = 30

[provider.anthropic]
model = "claude-opus-4-6"
api_key_ref = "secrets:anthropic_api_key"  # Reference, not value
max_tokens = 8192
timeout_seconds = 300

[transport.polkadot_chat]
enabled = true
network = "polkadot"
endpoint = "wss://rpc.polkadot.io"
poll_interval_seconds = 5

[chain.polkadot]
endpoint = "wss://rpc.polkadot.io"
metadata_cache = true
profile_refresh_interval = "1h"

[execution]
max_concurrent_runs = 4
max_run_duration = "30m"
harness_timeout = "10m"

[secrets]
backend = "encrypted-file"          # encrypted-file | os-keychain | env
path = "~/.polkagent/secrets.enc"

[policy]
default_grant = "read-only"
require_approval = ["chain-write", "payment", "deploy"]

[observability]
log_level = "info"
metrics_enabled = false
structured_logs = true
```

#### 5.2.6 CLI commands

```text
polkagent init                    # First-run setup wizard
polkagent start                   # Start daemon in foreground
polkagent start --daemon          # Start as background daemon
polkagent stop                    # Gracefully stop daemon
polkagent status                  # Show daemon and agent status
polkagent doctor                  # Validate configuration and connectivity
polkagent config show             # Display resolved configuration
polkagent config set <key> <val>  # Set a configuration value
polkagent agent list              # List configured agents
polkagent agent create <spec>     # Create a new agent from spec
polkagent run list                # List recent runs
polkagent run inspect <id>        # Show run details and timeline
polkagent artifact list <run-id>  # List artifacts for a run
polkagent export                  # Export all state for portability
polkagent import <bundle>         # Import state from export bundle
polkagent upgrade                 # Check and apply updates
polkagent logs                    # Tail structured logs
polkagent logs --run <id>         # Logs for a specific run
```

### 5.3 Docker / Docker Compose

#### 5.3.1 Dockerfile

The production image targets a minimal attack surface. The preferred base for
the final stage is `gcr.io/distroless/cc-debian12` (or a scratch-based image
if the binary is statically linked with musl). The `debian:bookworm-slim`
variant below is acceptable for development builds where shell access is
needed; production releases must use distroless.

```dockerfile
# Multi-stage build for minimal image
FROM rust:1.82-bookworm AS builder

WORKDIR /build
COPY . .
RUN cargo build --release --bin polkagent

# Production: use distroless for minimal attack surface.
# Switch FROM line to gcr.io/distroless/cc-debian12 for release builds.
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates tls-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Run as non-root
RUN groupadd -r polkagent && useradd -r -g polkagent -d /data polkagent

COPY --from=builder /build/target/release/polkagent /usr/local/bin/polkagent

USER polkagent
WORKDIR /data

VOLUME ["/data"]
EXPOSE 9400/tcp

HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD polkagent health || exit 1

ENTRYPOINT ["polkagent"]
CMD ["start", "--data-dir", "/data"]
```

#### 5.3.2 Docker Compose

```yaml
# docker-compose.yml
version: "3.9"

services:
  polkagent:
    image: ghcr.io/nicosiatechnologies/polkagent:latest
    container_name: polkagent
    restart: unless-stopped
    user: "1000:1000"
    volumes:
      - polkagent-data:/data
      - ./config.toml:/data/config.toml:ro
    ports:
      - "127.0.0.1:9400:9400"      # API (localhost only)
    environment:
      - POLKAGENT_LOG_LEVEL=info
      - POLKAGENT_SECRETS_BACKEND=env
    env_file:
      - .env                        # Provider API keys
    healthcheck:
      test: ["CMD", "polkagent", "health"]
      interval: 30s
      timeout: 5s
      retries: 3
    deploy:
      resources:
        limits:
          memory: 2G
          cpus: "2.0"
    security_opt:
      - no-new-privileges:true
    read_only: true
    tmpfs:
      - /tmp:size=100M

volumes:
  polkagent-data:
    driver: local
```

#### 5.3.3 Docker Compose with coding harness

```yaml
# docker-compose.harness.yml
version: "3.9"

services:
  polkagent:
    image: ghcr.io/nicosiatechnologies/polkagent:latest
    container_name: polkagent
    restart: unless-stopped
    volumes:
      - polkagent-data:/data
      - ./config.toml:/data/config.toml:ro
      - workspace:/workspace
    ports:
      - "127.0.0.1:9400:9400"
    environment:
      - POLKAGENT_LOG_LEVEL=info
    networks:
      - agent-net

  harness:
    image: ghcr.io/nicosiatechnologies/polkagent-harness:latest
    container_name: polkagent-harness
    restart: unless-stopped
    volumes:
      - workspace:/workspace
    environment:
      - POLKAGENT_HARNESS_SOCKET=/data/harness.sock
    networks:
      - agent-net
    security_opt:
      - no-new-privileges:true
    cap_drop:
      - ALL

volumes:
  polkagent-data:
  workspace:

networks:
  agent-net:
    driver: bridge
    internal: true                  # No external network access for harness
```

### 5.4 systemd service

The systemd unit is the recommended production deployment on bare-metal and
VM hosts. The unit uses `Type=notify` so the daemon signals readiness after
startup checks complete, and `WatchdogSec` so systemd will restart the process
if it stops sending `sd_notify(WATCHDOG=1)` pings. The binary must call
`sd_notify` from its health-check loop. The security hardening directives
below are mandatory for production; they should not be removed without a
documented risk-acceptance decision.

#### 5.4.1 Unit file

```ini
# /etc/systemd/system/polkagent.service
[Unit]
Description=Polkagent daemon
Documentation=https://docs.polkagent.dev
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
ExecStart=/usr/local/bin/polkagent start --data-dir /var/lib/polkagent
ExecReload=/bin/kill -HUP $MAINPID
ExecStop=/usr/local/bin/polkagent stop --graceful --timeout 30

# Security hardening
User=polkagent
Group=polkagent
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
RestrictNamespaces=yes
RestrictRealtime=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
ReadWritePaths=/var/lib/polkagent /var/log/polkagent
SystemCallFilter=@system-service
SystemCallErrorNumber=EPERM

# Resource limits
MemoryMax=2G
TasksMax=256
LimitNOFILE=65536

# Restart policy
Restart=on-failure
RestartSec=5s
WatchdogSec=60s

[Install]
WantedBy=multi-user.target
```

#### 5.4.2 Setup script

```bash
#!/usr/bin/env bash
set -euo pipefail

# Create user and directories
useradd --system --home-dir /var/lib/polkagent --shell /usr/sbin/nologin polkagent
mkdir -p /var/lib/polkagent /var/log/polkagent
chown -R polkagent:polkagent /var/lib/polkagent /var/log/polkagent
chmod 700 /var/lib/polkagent

# Install binary
install -m 755 polkagent /usr/local/bin/polkagent

# Install systemd unit
install -m 644 polkagent.service /etc/systemd/system/polkagent.service
systemctl daemon-reload
systemctl enable polkagent

# Initialize (as polkagent user)
sudo -u polkagent polkagent init --mode local-private --data-dir /var/lib/polkagent

# Start
systemctl start polkagent
systemctl status polkagent
```

### 5.5 Kubernetes (Helm charts)

#### 5.5.1 Architecture

```text
+------------------------------------------------------------------+
| Kubernetes cluster                                                |
|                                                                   |
|  +--------------------+    +-------------------------------+      |
|  |  Namespace:         |    |  Namespace:                   |      |
|  |  polkagent-system   |    |  polkagent-workers            |      |
|  |                     |    |                               |      |
|  |  +---------------+  |    |  +----------+  +----------+  |      |
|  |  | Control plane |  |    |  | Worker-0 |  | Worker-1 |  |      |
|  |  | (optional)    |  |    |  | Pod      |  | Pod      |  |      |
|  |  +-------+-------+  |    |  +----+-----+  +----+-----+  |      |
|  |          |           |    |       |             |         |      |
|  |  +-------+-------+  |    |  +----+-------------+-----+  |      |
|  |  | PostgreSQL    |  |    |  | Shared PVC or object    |  |      |
|  |  | (state)       |  |    |  | store for artifacts     |  |      |
|  |  +---------------+  |    |  +-------------------------+  |      |
|  +--------------------+    +-------------------------------+      |
|                                                                   |
|  +--------------------+                                           |
|  |  Secrets:           |                                          |
|  |  polkagent-secrets  |                                          |
|  |  (Kubernetes Secret |                                          |
|  |   or External       |                                          |
|  |   Secrets Operator) |                                          |
|  +--------------------+                                           |
+------------------------------------------------------------------+
```

#### 5.5.2 Helm values (excerpt)

```yaml
# values.yaml
replicaCount: 2

image:
  repository: ghcr.io/nicosiatechnologies/polkagent
  tag: "latest"
  pullPolicy: IfNotPresent

serviceAccount:
  create: true
  name: polkagent

config:
  agent:
    mode: "self-hosted"
  data:
    db: "postgres"
    dbUrl: "postgresql://polkagent:$(DB_PASSWORD)@polkagent-db:5432/polkagent"
  execution:
    maxConcurrentRuns: 8
  secrets:
    backend: "kubernetes"

database:
  enabled: true
  type: postgresql
  persistence:
    enabled: true
    size: 10Gi
    storageClass: "standard"

artifacts:
  storage:
    type: "pvc"                     # pvc | s3 | gcs
    size: 50Gi
    storageClass: "standard"

workers:
  replicas: 2
  resources:
    requests:
      cpu: "500m"
      memory: "512Mi"
    limits:
      cpu: "2000m"
      memory: "2Gi"
  autoscaling:
    enabled: true
    minReplicas: 1
    maxReplicas: 10
    targetCPUUtilizationPercentage: 70

controlPlane:
  enabled: false                    # Optional for self-hosted
  replicas: 1

ingress:
  enabled: true
  className: "nginx"
  annotations:
    cert-manager.io/cluster-issuer: "letsencrypt-prod"
  hosts:
    - host: polkagent.example.com
      paths:
        - path: /
          pathType: Prefix
  tls:
    - secretName: polkagent-tls
      hosts:
        - polkagent.example.com

secrets:
  provider: "kubernetes"            # kubernetes | vault | aws-sm | gcp-sm
  existingSecret: "polkagent-secrets"

networkPolicy:
  enabled: true
  # Workers cannot access internet directly; egress through proxy
  workerEgress:
    - to:
      - namespaceSelector:
          matchLabels:
            name: polkagent-system
    - to:
      - ipBlock:
          cidr: 0.0.0.0/0
      ports:
        - protocol: TCP
          port: 443                 # HTTPS only for provider APIs

podSecurityContext:
  runAsNonRoot: true
  runAsUser: 1000
  fsGroup: 1000
  seccompProfile:
    type: RuntimeDefault
```

#### 5.5.3 Helm chart structure

```text
polkagent-helm/
  Chart.yaml
  values.yaml
  templates/
    _helpers.tpl
    deployment.yaml                 # Main daemon deployment
    statefulset-workers.yaml        # Worker statefulset
    service.yaml                    # ClusterIP service
    ingress.yaml                    # Optional ingress
    configmap.yaml                  # Configuration
    secret.yaml                     # Secret references
    serviceaccount.yaml
    networkpolicy.yaml              # Network isolation
    pdb.yaml                        # Pod disruption budget
    hpa.yaml                        # Horizontal pod autoscaler
    rbac.yaml                       # RBAC rules
    tests/
      test-connection.yaml
      test-health.yaml
```

### 5.6 Desktop app wrapper

A future desktop application may wrap the CLI + daemon in a platform-native
shell (macOS menubar, Windows system tray, Linux desktop entry) for users
who prefer a graphical interface over terminal commands.

#### 5.6.1 Architecture

```text
+----------------------------------------------+
|  Desktop app (Tauri or Electron)              |
|                                               |
|  +------------------+  +-----------------+    |
|  | System tray /    |  | Web view        |    |
|  | Menubar          |  | (Agent Studio)  |    |
|  |                  |  |                 |    |
|  | - Start/stop     |  | - Conversations |    |
|  | - Status         |  | - Runs          |    |
|  | - Quick actions   |  | - Approvals     |    |
|  +--------+---------+  +--------+--------+    |
|           |                     |             |
|           v                     v             |
|  +-------------------------------------+     |
|  | polkagent daemon (embedded)          |     |
|  | Same binary, same data plane         |     |
|  +-------------------------------------+     |
+----------------------------------------------+
```

#### 5.6.2 Desktop-specific considerations

- The daemon is the same binary; the desktop wrapper manages its lifecycle.
- Auto-start on login is opt-in, not default.
- Notifications use OS-native notification APIs.
- The web view connects to the daemon's local API on localhost.
- No additional network listeners beyond what the daemon configuration specifies.

### 5.7 Prerequisites

| Deployment mode | Minimum requirements |
|---|---|
| Local CLI | Rust-supported OS (Linux, macOS, Windows); 512 MB RAM; 100 MB disk; internet for provider APIs |
| Docker | Docker 20.10+; 1 GB RAM; 500 MB disk |
| Docker Compose | Docker Compose v2+; same as Docker |
| systemd | Linux with systemd; same as Docker |
| Kubernetes | Kubernetes 1.28+; Helm 3.12+; persistent storage; ingress controller |
| Desktop | macOS 12+ / Windows 10+ / Linux with display server; same as local CLI |

### 5.8 Configuration

Configuration is resolved in this precedence order (highest to lowest):

1. CLI flags (`--data-dir /path`)
2. Environment variables (`POLKAGENT_DATA_DIR=/path`)
3. Configuration file (`config.toml`)
4. Defaults

Environment variable names follow the pattern `POLKAGENT_<SECTION>_<KEY>` in
uppercase with underscores. Nested keys use double underscores:
`POLKAGENT_PROVIDER__ANTHROPIC__MODEL=claude-opus-4-6`.

### 5.9 Upgrading

#### 5.9.1 Upgrade process

```bash
# Check for available updates
polkagent upgrade --check

# Apply update (stops daemon, upgrades binary, migrates DB, restarts)
polkagent upgrade --apply

# Rollback if needed
polkagent upgrade --rollback
```

#### 5.9.2 Upgrade invariants

- Database migrations are applied automatically at startup.
- Every migration has a corresponding rollback migration.
- The upgrade process creates a pre-upgrade export bundle as a safety net.
- Active runs are drained before the upgrade proceeds.
- The upgrade is atomic: it either fully succeeds or fully rolls back.

---

## 6. Managed cloud deployment

### 6.1 Multi-tenant architecture

The managed cloud hosts multiple tenants on shared infrastructure with strong
isolation guarantees. Each tenant is a separate organizational unit with its
own data, configuration, secrets, and billing.

#### 6.1.1 Tenant hierarchy

```text
Platform (Polkagent Cloud)
  |
  +-- Tenant: Acme Corp (tenant_id: acme-corp)
  |     |
  |     +-- Organization: Acme Engineering
  |     |     |
  |     |     +-- Project: DeFi Bot
  |     |     |     |
  |     |     |     +-- Environment: production
  |     |     |     |     +-- Agent: yield-monitor
  |     |     |     |     +-- Agent: trade-executor
  |     |     |     |
  |     |     |     +-- Environment: staging
  |     |     |           +-- Agent: yield-monitor-staging
  |     |     |
  |     |     +-- Project: Governance Research
  |     |           |
  |     |           +-- Environment: production
  |     |                 +-- Agent: gov-analyst
  |     |
  |     +-- Organization: Acme Operations
  |           |
  |           +-- Project: Infrastructure
  |                 +-- Environment: production
  |                       +-- Agent: chain-monitor
  |
  +-- Tenant: Bob's Bots (tenant_id: bobs-bots)
        |
        +-- (single default organization)
              +-- Project: Personal
                    +-- Environment: default
                          +-- Agent: my-assistant
```

#### 6.1.2 Entity model (Rust sketch)

```rust
/// A tenant is the top-level isolation boundary.
pub struct Tenant {
    pub id: TenantId,
    pub name: String,
    pub plan: Plan,
    pub status: TenantStatus,
    pub created_at: Timestamp,
    pub settings: TenantSettings,
}

/// An organization within a tenant.
pub struct Organization {
    pub id: OrgId,
    pub tenant_id: TenantId,
    pub name: String,
    pub settings: OrgSettings,
}

/// A project groups related agents and environments.
pub struct Project {
    pub id: ProjectId,
    pub org_id: OrgId,
    pub tenant_id: TenantId,      // Denormalized for query efficiency
    pub name: String,
    pub settings: ProjectSettings,
}

/// An environment within a project (e.g., production, staging).
pub struct Environment {
    pub id: EnvironmentId,
    pub project_id: ProjectId,
    pub tenant_id: TenantId,
    pub name: String,
    pub settings: EnvironmentSettings,
}

/// Membership and roles.
pub struct Membership {
    pub user_id: UserId,
    pub scope: MembershipScope,     // Tenant, Org, Project, or Environment
    pub scope_id: String,
    pub role: Role,
}

/// Roles with well-defined permission sets.
pub enum Role {
    Owner,              // Full administrative control
    Operator,           // Manage agents, deployments, and configuration
    Approver,           // Approve agent actions and policy changes
    Auditor,            // Read-only access to logs, metrics, and audit trails
    Developer,          // Create and test agents; no production deployment
    SupportLimited,     // Limited access for support; no secrets or plaintext
}
```

### 6.2 Tenant / org / user / agent boundaries

#### 6.2.1 Isolation matrix

| Boundary | Enforcement mechanism | Violation response |
|---|---|---|
| Tenant-to-tenant | Separate encryption key hierarchy; database row-level security; object store prefix policy; network namespace isolation; queue topic isolation | Deny, audit, alert |
| Org-to-org (within tenant) | Database row-level security; role-based access; audit logging | Deny, audit |
| Project-to-project | Database scoping; artifact namespace isolation | Deny, audit |
| Environment-to-environment | Separate config/secret sets; optional network isolation | Deny, audit |
| Agent-to-agent | Separate run contexts; grant intersection for groups; artifact scoping | Deny, audit |
| User-to-user | Authentication; role-based access per scope; audit | Deny, audit |

#### 6.2.2 Data isolation guarantees

1. **Encryption at rest.** All tenant data is encrypted with tenant-specific
   keys. The platform operator cannot read tenant data without the tenant's
   key hierarchy.
2. **Encryption in transit.** All inter-service communication uses TLS 1.3.
   Intra-cluster communication uses mutual TLS.
3. **Logical isolation.** Database queries always include `tenant_id` in WHERE
   clauses, enforced by row-level security policies that cannot be bypassed
   by application code. Every table in the shared PostgreSQL cluster carries a
   `tenant_id` column with a `ROW LEVEL SECURITY` policy; the application
   connection sets `app.current_tenant` and the RLS policy filters all reads
   and writes automatically.
4. **Policy isolation.** Each tenant's Cedar authorization policies are stored
   and evaluated in the context of that tenant's `tenant_id`. A Cedar policy
   cannot reference or affect resources belonging to a different tenant. The
   policy gate (see PRD-07) receives the tenant scope as a mandatory principal
   attribute on every authorization request.
5. **Compute isolation.** Workers are assigned to tenant tiers: shared-tier
   workers process one tenant's jobs at a time and are recycled between
   tenants; dedicated-tier workers are pinned to a single tenant and never
   process jobs from another tenant. No tenant's data persists in worker
   memory after job completion.
6. **Network isolation.** Tenant workers run in separate network namespaces
   with controlled egress policies.
7. **Artifact isolation.** Object store paths include tenant ID as a prefix.
   Bucket policies prevent cross-tenant access. Signed URLs are tenant-scoped
   and time-limited.

#### 6.2.3 Tenant lifecycle

```text
                +----------+
                | CREATED  |
                +----+-----+
                     |
                     v
                +-----------+
                | ACTIVE    |<------+
                +----+------+       |
                     |              |
              +------+------+      |
              |             |      |
              v             v      |
         +--------+   +---------+  |
         |SUSPENDED|  |UPGRADING+--+
         +----+----+  +---------+
              |
              v
         +---------+
         |OFFBOARDING|
         +----+------+
              |
              v
         +---------+
         | DELETED |  (data purged after retention period)
         +---------+
```

### 6.3 Shared infrastructure vs dedicated

| Tier | Infrastructure | Use case |
|---|---|---|
| Shared (default) | Shared compute pool, shared database cluster (row-level isolation), shared object store (prefix isolation) | Individual users, small teams, development/staging |
| Dedicated compute | Dedicated worker nodes for the tenant; shared database and storage | Teams with compute-intensive workloads or compliance requirements |
| Dedicated infrastructure | Dedicated database, dedicated object store, dedicated network; optional dedicated hardware | Enterprise with strict isolation, regulatory, or audit requirements |

### 6.4 Regional deployment options

| Region | Data residency | Availability |
|---|---|---|
| EU (Frankfurt) | EU data residency for GDPR compliance | Phase 3+ |
| US (Virginia) | US data residency | Phase 3+ |
| APAC (Singapore/Sydney) | APAC data residency | Phase 4+ |

Regional deployment means:

- All tenant data (state, artifacts, secrets, backups) stays within the
  selected region.
- Control plane metadata may be replicated globally for management, but
  content and secrets remain regional.
- A tenant may choose to restrict their deployment to one region.
- Cross-region agent communication is possible but requires explicit
  configuration and data-transfer consent.

---

## 7. Worker architecture

### 7.1 Worker registration and discovery

#### 7.1.1 Registration flow

```text
Worker                          Control Plane / Scheduler
  |                                    |
  |  1. RegisterWorker(capabilities)   |
  +------------------------------------>
  |                                    |
  |  2. WorkerRegistered(id, config)   |
  <------------------------------------+
  |                                    |
  |  3. Heartbeat(health, load)        |
  +------------------------------------>  (every 15s)
  |                                    |
  |  4. HeartbeatAck(config_update?)   |
  <------------------------------------+
  |                                    |
```

#### 7.1.2 Worker identity

Each worker has a unique identity derived from:

- A cryptographic key pair generated at first start.
- A worker ID assigned by the scheduler on registration.
- A tenant scope (for managed deployments).

The worker proves its identity on every heartbeat and job claim using its
private key. The scheduler verifies using the registered public key.

#### 7.1.3 Discovery in self-hosted clusters

Self-hosted clusters may use:

- **Static configuration:** workers listed in a configuration file.
- **DNS-based:** workers register via DNS SRV records.
- **Kubernetes-native:** workers discovered via Kubernetes service endpoints.

### 7.2 Job scheduling and assignment

#### 7.2.1 Scheduling algorithm

```text
1. Job arrives in queue with requirements:
   - Required harness types
   - Minimum memory/CPU
   - Region/zone affinity (if applicable)
   - Tenant scope
   - Priority level

2. Scheduler selects candidate workers:
   - Filter by tenant scope
   - Filter by required capabilities
   - Filter by available resources
   - Filter by region affinity

3. Among candidates, select by:
   - Least-loaded (weighted by current jobs / max concurrent)
   - Data locality (prefer workers with cached artifacts)
   - Sticky affinity (prefer the worker that ran the previous turn)

4. Assign job with lease:
   - Worker claims job with a time-bounded lease
   - Worker renews lease every 30 seconds while working
   - If lease expires, job returns to queue for reassignment
```

#### 7.2.2 Priority levels

| Priority | Use case | Scheduling behavior |
|---|---|---|
| CRITICAL | Security-related, incident response | Preempts lower priorities; separate capacity reservation |
| HIGH | User-initiated interactive requests | Scheduled before NORMAL; bounded wait time |
| NORMAL | Standard agent runs, scheduled tasks | FIFO within priority level |
| LOW | Background processing, batch jobs, evals | Scheduled only when capacity is available |
| BULK | Large batch imports, data migration | Rate-limited; may be deferred during peak |

### 7.3 Health monitoring

#### 7.3.1 Health check protocol

Workers report health via periodic heartbeats:

```rust
pub struct WorkerHealth {
    pub worker_id: WorkerId,
    pub status: WorkerStatus,
    pub uptime: Duration,
    pub current_jobs: u32,
    pub max_jobs: u32,
    pub cpu_usage_percent: f32,
    pub memory_usage_bytes: u64,
    pub memory_limit_bytes: u64,
    pub disk_usage_bytes: u64,
    pub disk_limit_bytes: u64,
    pub last_job_completed_at: Option<Timestamp>,
    pub error_count_last_hour: u32,
    pub version: String,
}

pub enum WorkerStatus {
    Healthy,
    Degraded { reason: String },
    Draining,           // Finishing current work, accepting no new jobs
    Unhealthy { reason: String },
}
```

#### 7.3.2 Failure detection

| Signal | Detection | Response |
|---|---|---|
| Missed heartbeats | 3 consecutive missed heartbeats (45s) | Mark worker unhealthy; reassign its jobs |
| High error rate | >50% job failure in 10-minute window | Mark worker degraded; reduce scheduling weight |
| Resource exhaustion | Memory >90% or disk >95% | Mark worker degraded; do not assign new jobs |
| Process crash | Container exit with non-zero code | Kubernetes/systemd restarts; jobs reassigned after lease expiry |
| Network partition | Worker cannot reach scheduler | Worker pauses new work; completes in-flight jobs; reconnects with state reconciliation |

### 7.4 Auto-scaling rules

#### 7.4.1 Scale-up triggers

| Metric | Threshold | Action |
|---|---|---|
| Queue depth | >10 pending jobs for >2 minutes | Add 1 worker per 10 pending jobs |
| Queue wait time | p95 >30 seconds | Add 1 worker |
| Worker CPU utilization | Average >80% for >5 minutes | Add 1 worker |
| Worker memory utilization | Average >85% for >5 minutes | Add 1 worker |

#### 7.4.2 Scale-down triggers

| Metric | Threshold | Action |
|---|---|---|
| Queue depth | 0 pending jobs for >10 minutes | Remove 1 worker (respecting min replicas) |
| Worker utilization | Average <20% for >15 minutes | Remove 1 worker |
| Time of day | Off-peak hours (configurable) | Scale to min replicas |

#### 7.4.3 Scaling constraints

- **Minimum replicas:** at least 1 worker per tenant with active agents.
- **Maximum replicas:** configurable per tenant (plan-dependent).
- **Cooldown:** 5 minutes between scale-up events; 10 minutes between
  scale-down events.
- **Draining:** workers are drained (finish current work) before removal.
- **Budget:** scaling respects tenant compute budget limits.

### 7.5 Resource quotas per tenant

| Resource | Free tier | Team tier | Enterprise tier |
|---|---|---|---|
| Max concurrent runs | 2 | 10 | Configurable (default 50) |
| Max workers | 1 | 5 | Configurable (default 20) |
| Max CPU per run | 1 core | 4 cores | Configurable |
| Max memory per run | 512 MB | 2 GB | Configurable |
| Max run duration | 10 minutes | 30 minutes | Configurable (default 2h) |
| Max artifact storage | 1 GB | 50 GB | Configurable |
| Max state DB size | 500 MB | 10 GB | Configurable |
| API rate limit | 60 req/min | 600 req/min | Configurable |

Quota enforcement is at the execution plane. When a quota is reached:

1. New runs are queued (not rejected) until capacity is available.
2. The user receives a clear "quota exceeded" notification with current usage.
3. The control plane records the quota event for billing/planning.
4. Emergency overrides exist for critical operations (owner-only).

---

## 8. Offline-safe degradation (J3 invariant)

### 8.1 The J3 invariant

**Invariant J3:** when the control plane is unavailable, the data plane
follows locally cached policy and pauses actions outside it. No grant is
widened and no effect is silently executed when the authority source is
unreachable.

This is not a degraded-UX concern; it is a safety invariant. An agent that
silently continues with stale or absent policy could execute unauthorized
effects.

### 8.2 Behavior when control plane is unavailable

#### 8.2.1 What continues

| Capability | Behavior during disconnection |
|---|---|
| Active runs | Continue to completion under their already-resolved grants. |
| Policy evaluation | Use locally cached policy. No new grants are issued that require control-plane validation. |
| Transport sessions | Continue operating; messages are sent and received normally. |
| Chain reads | Continue via configured RPC endpoints. |
| Artifact storage | Local/data-plane storage continues; managed backup pauses. |
| Health monitoring | Local health checks continue; control-plane reporting pauses. |

#### 8.2.2 What pauses

| Capability | Behavior during disconnection |
|---|---|
| New policy revisions | Cannot pull; use cached. Display staleness warning. |
| Fleet commands | Cannot receive pause/drain/rollout commands. |
| Billing/metering | Usage is recorded locally; will be synced on reconnection. |
| Extension updates | Cannot check for updates; use cached versions. |
| Cross-tenant features | Any feature requiring control-plane mediation pauses. |
| New enrollments | Cannot enroll new data planes. |

#### 8.2.3 Action pausing rules

Actions are paused (not denied) when all of these are true:

1. The action requires a grant that references a policy revision the data
   plane has never seen (policy freshness check fails).
2. The action is in an action family that is configured to require control-
   plane validation (e.g., payment above a threshold).
3. The cached policy does not contain a pre-authorized fallback for this
   action family.

Paused actions enter a `PENDING_CONNECTIVITY` state. When the control plane
reconnects, they are re-evaluated against the current policy revision. The
user is notified of paused actions and may cancel them.

### 8.3 Locally cached policy

#### 8.3.1 Cache structure

```rust
pub struct PolicyCache {
    /// The latest applied policy revision.
    pub revision: PolicyRevision,
    /// When this revision was received from the control plane.
    pub received_at: Timestamp,
    /// The maximum age before the cache is considered stale.
    pub max_age: Duration,
    /// The resolved policy document.
    pub policy: Policy,
    /// Signature from the tenant's management key.
    pub signature: Signature,
    /// Hash of the policy for integrity verification.
    pub hash: PolicyHash,
}
```

#### 8.3.2 Staleness behavior

| Cache age | Behavior |
|---|---|
| Fresh (< max_age) | Normal operation; all cached grants are valid. |
| Stale (> max_age, < 2x max_age) | Operation continues; UI shows "policy may be stale" warning; operator is notified. |
| Very stale (> 2x max_age) | Read-only actions continue; write/payment actions are paused; operator alert escalates. |
| Expired (> 4x max_age or configured hard limit) | All non-emergency actions pause; only emergency-stop and export continue. |

Default `max_age` is 24 hours. Operators may configure shorter or longer
periods. A local-only deployment without a control plane has no staleness
concern; its policy is authoritative.

### 8.4 Reconnection and sync

#### 8.4.1 Reconnection protocol

```text
Data Plane                         Control Plane
  |                                    |
  |  1. Reconnect(last_revision,       |
  |     local_metrics_since)           |
  +------------------------------------>
  |                                    |
  |  2. ReconnectResponse(             |
  |     new_revisions[],               |
  |     missed_commands[],             |
  |     billing_sync_request)          |
  <------------------------------------+
  |                                    |
  |  3. For each new revision:         |
  |     Evaluate, apply or reject      |
  |                                    |
  |  4. SyncComplete(                  |
  |     applied_revisions[],           |
  |     rejected_revisions[],          |
  |     local_metrics_batch,           |
  |     paused_actions_resolved[])     |
  +------------------------------------>
  |                                    |
  |  5. SyncAck(billing_update)        |
  <------------------------------------+
```

#### 8.4.2 Conflict resolution

When the data plane reconnects and receives policy revisions that conflict
with actions taken during disconnection:

1. **Already-completed effects are immutable.** The control plane cannot
   retroactively invalidate a completed effect. It may flag it for review.
2. **Paused actions are re-evaluated.** If the new policy permits them,
   they proceed. If it denies them, they are cancelled with a reason.
3. **Locally widened grants are rejected.** If the data plane issued a
   broader grant during disconnection than the current policy permits,
   a compliance event is logged and the operator is notified.
4. **Usage reconciliation.** Locally recorded usage is synced to billing.
   Discrepancies are flagged for operator review.

---

## 9. State portability

### 9.1 Export format specification

All Polkagent state is exportable to a documented, versioned format. The
export is a self-contained bundle that another Polkagent instance can import.

#### 9.1.1 Export bundle structure

```text
polkagent-export-<timestamp>.tar.gz
  manifest.json                      # Bundle metadata and version
  agents/
    <agent-id>.json                  # AgentSpec for each agent
  config/
    config.toml                      # Resolved configuration
    policies/
      <policy-id>.json               # Policy documents
    extensions.lock                  # Extension lockfile
  identity/
    agent-identity.json              # Public identity (never private keys)
    account-bindings.json            # Chain account bindings
  state/
    conversations.jsonl              # Conversations (JSONL for streaming)
    runs.jsonl                       # Run records
    effects.jsonl                    # Effect lifecycle records
    approvals.jsonl                  # Approval decisions
    receipts.jsonl                   # Receipts and evidence
  artifacts/
    <run-id>/
      <artifact-id>.<ext>           # Artifact files
  memory/
    episodes.jsonl                   # Memory records
    semantic.jsonl                   # Semantic memory
  metadata/
    export-info.json                 # Export timestamp, source version, etc.
    checksums.sha256                 # Integrity checksums for all files
```

#### 9.1.2 Manifest schema

```json
{
  "format_version": "1.0.0",
  "polkagent_version": "0.5.0",
  "exported_at": "2026-07-30T12:00:00Z",
  "source_deployment": {
    "mode": "managed",
    "tenant_id": "acme-corp",
    "region": "eu-frankfurt"
  },
  "contents": {
    "agents": 3,
    "conversations": 142,
    "runs": 891,
    "effects": 2340,
    "artifacts": 567,
    "memory_records": 1203
  },
  "secrets_excluded": true,
  "secret_rebinding_required": [
    "anthropic_api_key",
    "polkadot_rpc_endpoint"
  ]
}
```

### 9.2 Import validation

#### 9.2.1 Import process

```text
1. VALIDATE
   - Verify bundle integrity (checksums)
   - Check format version compatibility
   - Parse and validate all schemas

2. PREVIEW
   - Show mapping report: what will be imported, what conflicts exist
   - Identify secrets that need rebinding
   - Show unsupported or deprecated configuration
   - Estimate storage requirements

3. CONFIRM
   - User reviews and confirms the import plan
   - User provides required secret bindings

4. IMPORT
   - Apply state in dependency order
   - Record import provenance on every imported record
   - Generate an import report

5. VERIFY
   - Run doctor checks on imported state
   - Validate transport connectivity
   - Validate provider connectivity
   - Run health checks

6. ACTIVATE
   - Enable imported agents (manually or automatically per config)
```

#### 9.2.2 Import safety rules

- **No concurrent identity.** Import refuses if the imported agent identity
  is already active in another deployment.
- **No automatic effect replay.** Pending or unknown-state effects are
  imported as review items, not automatically retried.
- **Rollback support.** The import creates a rollback point. If any step
  fails, the entire import is rolled back.
- **Audit trail.** Every imported record carries an `imported_from` field
  with the source deployment and timestamp.

### 9.3 Migration between deployment modes

#### 9.3.1 Supported migration paths

```text
Local CLI  <-->  Docker  <-->  Kubernetes  <-->  Managed Cloud
    |                                                |
    +------------------------------------------------+
                    (direct migration)
```

All migration paths use the same export/import mechanism. Migration-specific
concerns:

| From | To | Additional steps |
|---|---|---|
| Local CLI | Docker | Export; configure Docker volumes; import |
| Local CLI | Managed Cloud | Export; create tenant; import; rebind secrets |
| Docker | Kubernetes | Export; configure Helm values; import via init job |
| Managed Cloud | Local CLI | Export from cloud console; import locally; rebind secrets |
| Managed Cloud | Self-hosted K8s | Export; deploy Helm chart; import; rebind secrets |

#### 9.3.2 Database migration

| Source DB | Target DB | Migration method |
|---|---|---|
| SQLite | SQLite | File copy (same schema) |
| SQLite | PostgreSQL | Schema-compatible migration tool; row-by-row import |
| PostgreSQL | SQLite | Schema-compatible export; works only for single-tenant |
| PostgreSQL | PostgreSQL | pg_dump/pg_restore or logical replication |

The portable schema is the enabler for both SQLite-to-PostgreSQL migration and
GDPR data subject rights. Because every entity (conversation, run, effect,
artifact, memory record) references the owning agent and tenant, the export
pipeline can produce a complete, verifiable dump of all data for a given
subject and the delete pipeline can cascade-delete all rows scoped to that
subject without affecting other tenants or agents. These operations must be
exposed as first-class CLI and API commands, not ad-hoc SQL:

```text
polkagent gdpr export --subject <user-or-agent-id>   # produces signed bundle
polkagent gdpr delete --subject <user-or-agent-id>   # hard-deletes; audit logged
```

The delete command must be reversible only by restoring from a pre-deletion
backup, not by any application-level undo. It must complete within the
statutory response deadline (72 hours under GDPR Art. 17).

### 9.4 No vendor lock-in guarantees

| Guarantee | How it is enforced |
|---|---|
| All data is exportable | Export covers all state; nothing is silently excluded |
| Export format is documented | Public specification with versioning |
| Secrets are rebindable | Export identifies required secrets without containing them |
| Identity is portable | Agent identity is a key pair; account bindings are metadata |
| Configuration is standard | TOML/JSON configuration with documented schema |
| No proprietary protocols | Control-plane API is documented; data-plane is the product |

---

## 10. Secrets management

### 10.1 Local: encrypted file or OS keychain

#### 10.1.1 Encrypted file backend

The default local secret backend uses age-compatible encryption:

```text
~/.polkagent/secrets.enc
  |
  +-- Encrypted with:
  |     - User passphrase (default)
  |     - Hardware key (YubiKey, etc.) (optional)
  |     - Both (optional)
  |
  +-- Contains:
        - Provider API keys
        - Agent private key material
        - Transport session keys
        - Chain RPC authentication tokens
        - Custom user secrets
```

Secrets are decrypted into memory at daemon start and never written to disk
in plaintext. The daemon clears secret memory on shutdown.

#### 10.1.2 OS keychain backend

On supported platforms, secrets may be stored in the OS keychain:

| Platform | Keychain | Service name |
|---|---|---|
| macOS | Keychain Access / Security framework | `dev.polkagent.<agent-id>` |
| Linux | libsecret / GNOME Keyring / KDE Wallet | `polkagent/<agent-id>` |
| Windows | Windows Credential Manager | `Polkagent/<agent-id>` |

#### 10.1.3 Environment variable backend

For container deployments, secrets may be injected via environment variables:

```bash
POLKAGENT_SECRET_ANTHROPIC_API_KEY=sk-ant-...
POLKAGENT_SECRET_RPC_TOKEN=...
```

Environment variables are read once at startup and cleared from the process
environment.

### 10.2 Cloud: KMS/HSM integration

#### 10.2.1 Architecture

```text
+--------------------+     +------------------+     +---------------+
| Data plane worker  |     | KMS / HSM        |     | Secret store  |
|                    |     |                  |     |               |
| Needs secret X     +---->| Decrypt envelope +---->| Return secret |
|                    |     | key for tenant   |     | plaintext     |
| Uses secret X      |<---+ (never exports   |<---+               |
| (in memory only)   |     |  master key)     |     |               |
+--------------------+     +------------------+     +---------------+
```

#### 10.2.2 Supported KMS providers

| Provider | Integration | Key type |
|---|---|---|
| AWS KMS | AWS SDK; IAM role per tenant | AES-256-GCM envelope encryption |
| GCP Cloud KMS | GCP SDK; service account per tenant | AES-256-GCM envelope encryption |
| Azure Key Vault | Azure SDK; managed identity per tenant | AES-256-GCM envelope encryption |
| HashiCorp Vault | Vault API; AppRole or Kubernetes auth | Transit secrets engine |
| Self-hosted HSM | PKCS#11 interface | Hardware-backed keys |

#### 10.2.3 Envelope encryption

Each tenant's secrets are encrypted with a data encryption key (DEK). The DEK
is itself encrypted with a key encryption key (KEK) managed by the KMS:

```text
Secret value --> Encrypt with DEK --> Encrypted secret (stored)
DEK          --> Encrypt with KEK --> Encrypted DEK (stored alongside)
KEK          --> Managed by KMS    --> Never leaves KMS boundary
```

### 10.3 Secret rotation

#### 10.3.1 Rotation types

| Secret type | Rotation trigger | Rotation method |
|---|---|---|
| Provider API keys | Manual or scheduled (90 days) | Generate new key in provider; update secret store; verify new key works; revoke old key |
| Agent identity key | Manual (rare) | Generate new key pair; update enrollment; re-establish transport sessions |
| Transport session keys | Automatic (per session) | Renegotiated by transport protocol |
| Database encryption DEK | Scheduled (annually) or on compromise | Re-encrypt with new DEK; update encrypted DEK in store |
| KMS KEK | Per KMS provider rotation policy | Transparent to data plane; KMS handles rotation |

#### 10.3.2 Rotation invariants

- Active sessions using the old secret continue until natural expiry.
- The new secret is validated (connectivity test) before the old one is
  revoked.
- Rotation events are logged in the audit trail.
- Failed rotations alert the operator and do not revoke the old secret.

### 10.4 Access audit

Every secret access is logged:

```rust
pub struct SecretAccessEvent {
    pub timestamp: Timestamp,
    pub secret_id: SecretId,
    pub accessor: Accessor,         // Worker, CLI, API, support
    pub action: SecretAction,       // Read, Rotate, Delete
    pub tenant_id: TenantId,
    pub run_id: Option<RunId>,      // If accessed during a run
    pub result: AccessResult,       // Allowed, Denied, Error
    pub source_ip: Option<IpAddr>,
}
```

Audit logs for secrets are:

- Stored separately from application logs.
- Retained for at least 1 year (configurable per tenant).
- Immutable (append-only, tamper-evident).
- Accessible only to `Owner` and `Auditor` roles.

---

## 11. Billing and metering (managed mode)

### 11.1 Usage dimensions

| Dimension | Unit | Measurement point |
|---|---|---|
| Compute time | CPU-seconds | Per run, measured by worker |
| Memory usage | GB-hours | Peak per run, measured by worker |
| Storage | GB-months | Artifact + state DB, measured daily |
| API calls | Requests | Per API call, measured by API gateway |
| Model tokens (pass-through) | Tokens (input + output) | Per provider call, measured by data plane |
| Transport messages | Messages | Per message sent/received |
| Bandwidth | GB | Egress measured at network boundary |

### 11.2 Billing periods and invoicing

| Cycle | Description |
|---|---|
| Real-time metering | Usage counters updated every 60 seconds |
| Daily aggregation | Daily usage summaries computed at 00:00 UTC |
| Monthly billing period | Calendar month; invoices generated on 1st of following month |
| Prepaid credits | Optional prepaid credit balance; usage deducted in real-time |

### 11.3 Metering architecture

```text
Worker --> Usage event --> Metering queue --> Aggregator --> Billing DB
                              |
                              v
                         Rate limiter  -->  Quota enforcement
                              |
                              v
                         Alert engine  -->  Notifications
```

#### 11.3.1 Metering contract (Rust sketch)

```rust
pub struct UsageEvent {
    pub tenant_id: TenantId,
    pub timestamp: Timestamp,
    pub dimension: UsageDimension,
    pub quantity: f64,
    pub metadata: UsageMetadata,
}

pub enum UsageDimension {
    ComputeSeconds { cpu_cores: f32 },
    MemoryGbHours,
    StorageGbMonths,
    ApiCalls,
    ModelTokens { provider: ProviderId, model: ModelId },
    TransportMessages,
    BandwidthGb,
}

pub struct UsageMetadata {
    pub run_id: Option<RunId>,
    pub agent_id: Option<AgentId>,
    pub environment_id: Option<EnvironmentId>,
    pub worker_id: WorkerId,
}
```

### 11.4 Quota management

#### 11.4.0 Enforcement at the policy gate

Usage counters (token spend, effect count, compute seconds) are incremented
inside the policy gate -- the same component that evaluates Cedar grants.
This means quota enforcement is not a separate middleware that can be bypassed
by internal callers; it is a property of the authorization decision. A run
that would exceed a tenant's token quota fails the grant check at the policy
gate before the AI provider call is made, the same way an unauthorized effect
would. Counters are persisted to the data plane state DB so they survive
restarts and are available during control-plane disconnection.

This approach also provides the audit signal needed for billing: every
token/effect counted at the gate produces an `UsageEvent` that flows to the
metering queue described in section 11.3.

Quotas are enforced at multiple levels:

```text
Platform limits
  |
  +-- Tenant plan limits
        |
        +-- Organization budget limits
              |
              +-- Project budget limits
                    |
                    +-- Agent resource limits
```

When a quota is approached:

| Usage level | Action |
|---|---|
| 80% of quota | Warning notification to tenant owner and operators |
| 90% of quota | Alert notification; usage dashboard highlights |
| 100% of quota | New runs queued (not rejected); owner notified |
| 110% of quota (grace) | New runs rejected; active runs complete; escalation |

### 11.5 Usage dashboards

The billing dashboard shows:

- Current period usage by dimension, broken down by organization, project,
  environment, and agent.
- Historical usage trends (daily, weekly, monthly).
- Cost projection for the current billing period.
- Quota utilization as percentage of plan limits.
- Top consumers (agents, runs) by each dimension.

---

## 12. SLOs and incident management

### 12.1 Availability targets

| Component | SLO | Measurement | Exclusions |
|---|---|---|---|
| Data plane (managed) | 99.9% monthly | Successful health checks / total checks | Scheduled maintenance (with 72h notice) |
| Control plane | 99.5% monthly | Successful API responses / total requests | Data plane operates independently during downtime |
| API gateway | 99.9% monthly | Successful responses / total requests (excluding 429s) | Rate-limited requests |
| Job scheduler | 99.9% monthly | Jobs assigned within SLA / total jobs | Priority BULK jobs |
| External AI provider | tracked but not owned | Provider error rate reported per provider | Provider outages excluded from Polkagent SLO |

#### 12.1.1 External AI provider as a failure domain

External AI providers (Anthropic, OpenAI, etc.) are a distinct failure domain.
Their availability and latency directly affect agent run success rates but are
not under Polkagent's control. SLO/error-budget calculations must therefore:

1. Track provider error rates and latency separately from platform errors.
2. Exclude provider-caused failures from the Polkagent error budget.
3. Surface provider degradation in the operator dashboard so operators can
   direct users to a fallback provider or explain the disruption.
4. Define a `ProviderUnavailable` error category in the effect lifecycle so
   runs paused due to provider outage are not counted as Polkagent failures
   in SLO reports.

Provider health is monitored by the data plane via synthetic probe runs
(lightweight no-effect turns) on a configurable interval. The result is
exposed on the `/health` endpoint and in the managed observability stack.

### 12.2 Latency targets

| Operation | Target (p95) | Target (p99) |
|---|---|---|
| API response (non-streaming) | 200ms | 500ms |
| Job assignment (CRITICAL priority) | 5s | 15s |
| Job assignment (HIGH priority) | 15s | 60s |
| Job assignment (NORMAL priority) | 60s | 5m |
| Config revision propagation | 5m | 15m |
| Health status update | 30s | 60s |

### 12.3 Incident classification and response

| Severity | Definition | Response time | Resolution target |
|---|---|---|---|
| SEV-1 | Platform-wide outage; data loss; security breach | 15 minutes | 4 hours |
| SEV-2 | Multiple tenants affected; degraded performance | 30 minutes | 8 hours |
| SEV-3 | Single tenant affected; workaround available | 2 hours | 24 hours |
| SEV-4 | Minor issue; cosmetic; no user impact | 1 business day | 5 business days |

#### 12.3.1 Incident response protocol

```text
1. DETECT
   - Automated monitoring alerts (PagerDuty/Opsgenie)
   - User report (support ticket)
   - Synthetic monitoring failure

2. TRIAGE
   - On-call engineer assesses severity
   - Opens incident channel
   - Notifies affected tenants (SEV-1/2)

3. MITIGATE
   - Apply immediate mitigation (failover, rollback, scale-up)
   - Communicate status to affected tenants
   - Update status page

4. RESOLVE
   - Root cause identified and fixed
   - Fix deployed and verified
   - Affected tenants notified of resolution

5. REVIEW
   - Post-incident review within 5 business days
   - Blameless root cause analysis
   - Action items for prevention
   - SLO impact assessment
```

### 12.4 Runbook requirements

Every managed service component must have a runbook covering:

| Section | Contents |
|---|---|
| Service description | What the service does; dependencies; architecture |
| Health checks | How to verify the service is healthy |
| Common failures | Symptoms, diagnosis steps, and remediation for known failure modes |
| Scaling | How to scale up/down; capacity planning guidelines |
| Deployment | How to deploy, rollback, and verify |
| Backup/restore | How to backup and restore data; RTO/RPO targets |
| Secret rotation | How to rotate each secret the service depends on |
| Escalation | Who to contact; when to escalate |

---

## 13. Deployment topology diagrams

### 13.1 Local personal deployment

```text
+----------------------------------------------------------+
|  User workstation                                         |
|                                                           |
|  +-------------------+    +---------------------------+   |
|  |  polkagent CLI    |    |  polkagent daemon         |   |
|  |                   +--->|                           |   |
|  |  Terminal / TUI   |<---+  Data plane:              |   |
|  +-------------------+    |    SQLite state           |   |
|                           |    Local artifact store   |   |
|                           |    Encrypted secrets      |   |
|                           |                           |   |
|                           |  Execution plane:         |   |
|                           |    In-process Tokio       |   |
|                           |    Single worker          |   |
|                           |                           |   |
|                           |  No control plane         |   |
|                           +-------------+-------------+   |
|                                         |                 |
|                              +----------+----------+      |
|                              |  External services  |      |
|                              |  - AI provider API  |      |
|                              |  - Chain RPC        |      |
|                              |  - Statement Store  |      |
|                              +---------------------+      |
+----------------------------------------------------------+
```

### 13.2 Self-hosted server deployment

```text
+------------------------------------------------------------------+
|  Organization server                                              |
|                                                                   |
|  +-----------------------+    +------------------------------+    |
|  |  polkagent daemon     |    |  polkagent daemon            |    |
|  |  (agent-1)            |    |  (agent-2)                   |    |
|  |                       |    |                              |    |
|  |  Data plane:          |    |  Data plane:                 |    |
|  |    SQLite state       |    |    SQLite state              |    |
|  |    Local artifacts    |    |    Local artifacts           |    |
|  |    OS keychain        |    |    OS keychain               |    |
|  |                       |    |                              |    |
|  |  Execution plane:     |    |  Execution plane:            |    |
|  |    Daemon worker      |    |    Daemon worker             |    |
|  +-----------+-----------+    +--------------+---------------+    |
|              |                               |                    |
|              +---------------+---------------+                    |
|                              |                                    |
|              +---------------+---------------+                    |
|              |  Reverse proxy (nginx/caddy)  |                    |
|              |  TLS termination              |                    |
|              +---------------+---------------+                    |
|                              |                                    |
+------------------------------+------------------------------------+
                               |
                    Internet (provider APIs, chain RPCs)
```

### 13.3 Self-hosted Kubernetes deployment

```text
+------------------------------------------------------------------+
| Kubernetes cluster                                                |
|                                                                   |
| +---------------------+  +------------------------------------+  |
| | polkagent-system NS  |  | polkagent-workers NS               |  |
| |                      |  |                                    |  |
| | +------------------+ |  | +------------+  +------------+    |  |
| | | PostgreSQL       | |  | | Worker pod |  | Worker pod |    |  |
| | | StatefulSet      | |  | | (agent-1)  |  | (agent-2)  |    |  |
| | | (state DB)       | |  | |            |  |            |    |  |
| | +------------------+ |  | | Data plane |  | Data plane |    |  |
| |                      |  | | (connects  |  | (connects  |    |  |
| | +------------------+ |  | |  to PG)    |  |  to PG)    |    |  |
| | | Object store     | |  | +------+-----+  +------+-----+    |  |
| | | (MinIO/S3)       | |  |        |               |          |  |
| | | (artifacts)      | |  +--------+---------------+----------+  |
| | +------------------+ |           |               |             |
| |                      |           |               |             |
| | +------------------+ |  +--------+---------------+----------+  |
| | | Optional         | |  | Shared services                   |  |
| | | control plane    | |  |                                    |  |
| | | Deployment       | |  | +------------------+              |  |
| | +------------------+ |  | | Job queue        |              |  |
| +---------------------+  | | (Redis/NATS)     |              |  |
|                           | +------------------+              |  |
|                           +------------------------------------+  |
|                                                                   |
| +------------------------------+                                  |
| | Ingress controller           |                                  |
| | (nginx/traefik)              |                                  |
| +------------------------------+                                  |
+------------------------------------------------------------------+
```

### 13.4 Managed cloud deployment

```text
+=====================================================================+
| Polkagent Managed Cloud                                              |
|                                                                      |
| +---------------------------+   +-------------------------------+    |
| | Control Plane             |   | Tenant: Acme Corp             |    |
| |                           |   |                               |    |
| | +---------------------+  |   | +---------------------------+ |    |
| | | API Gateway         |  |   | | Isolated data plane       | |    |
| | | (auth, rate limit)  |  |   | |                           | |    |
| | +---------------------+  |   | | +----------+ +---------+  | |    |
| |                           |   | | | Worker-0 | | Worker-1|  | |    |
| | +---------------------+  |   | | | Pod      | | Pod     |  | |    |
| | | Tenant admin        |  |   | | +----------+ +---------+  | |    |
| | | service             |  |   | |                           | |    |
| | +---------------------+  |   | | +----------+             | |    |
| |                           |   | | | Tenant   |             | |    |
| | +---------------------+  |   | | | PostgreSQL|             | |    |
| | | Fleet manager       |  |   | | | (RLS)    |             | |    |
| | | service             |  |   | | +----------+             | |    |
| | +---------------------+  |   | |                           | |    |
| |                           |   | | +----------+             | |    |
| | +---------------------+  |   | | | Tenant   |             | |    |
| | | Policy distributor  |  |   | | | artifact |             | |    |
| | | service             |  |   | | | store    |             | |    |
| | +---------------------+  |   | | +----------+             | |    |
| |                           |   | |                           | |    |
| | +---------------------+  |   | | +----------+             | |    |
| | | Billing/metering    |  |   | | | KMS      |             | |    |
| | | service             |  |   | | | (tenant  |             | |    |
| | +---------------------+  |   | | |  keys)   |             | |    |
| |                           |   | | +----------+             | |    |
| | +---------------------+  |   | +---------------------------+ |    |
| | | Observability       |  |   +-------------------------------+    |
| | | (Prometheus/Grafana) |  |                                        |
| | +---------------------+  |   +-------------------------------+    |
| |                           |   | Tenant: Bob's Bots            |    |
| | +---------------------+  |   |                               |    |
| | | Incident/alerting   |  |   | +---------------------------+ |    |
| | | (PagerDuty)         |  |   | | Isolated data plane       | |    |
| | +---------------------+  |   | | (same structure as above) | |    |
| +---------------------------+   | +---------------------------+ |    |
|                                  +-------------------------------+    |
+=====================================================================+
```

### 13.5 Hybrid deployment

```text
+---------------------------+         +---------------------------+
| Organization premises     |         | Polkagent Cloud           |
|                           |         |                           |
| +---------------------+  |  mTLS   | +---------------------+   |
| | Local workers        |  +-------->| | Control plane       |   |
| | (sensitive data)     |  |  pull   | |                     |   |
| |                      |<-+--------+| | - Fleet dashboard   |   |
| | Data plane:          |  | desired | | - Policy dist       |   |
| |   Full state         |  | state   | | - Observability     |   |
| |   Secrets (local)    |  |         | | - Billing           |   |
| |   No cloud access    |  | push    | +---------------------+   |
| |   to secrets         |  | health  |                           |
| +---------------------+  |  metrics | +---------------------+   |
|                           |  (redacted) | Cloud workers     |   |
| +---------------------+  |         | | (non-sensitive)     |   |
| | HSM / hardware      |  |         | |                     |   |
| | signer              |  |         | | Data plane:         |   |
| +---------------------+  |         | |   Isolated state    |   |
+---------------------------+         | |   Cloud KMS         |   |
                                      | +---------------------+   |
                                      +---------------------------+
```

---

## 14. Configuration examples

### 14.1 Local personal agent

```toml
# Local private assistant configuration
[agent]
name = "my-assistant"
mode = "local-private"

[data]
dir = "~/.polkagent"
db = "sqlite"

[provider.anthropic]
model = "claude-opus-4-6"
api_key_ref = "secrets:anthropic_api_key"

[chain.polkadot]
endpoint = "wss://rpc.polkadot.io"

[secrets]
backend = "os-keychain"

[policy]
default_grant = "read-only"
require_approval = ["chain-write", "payment"]

[execution]
max_concurrent_runs = 2
```

### 14.2 Self-hosted team server

```toml
# Team server configuration
[agent]
name = "team-builder"
mode = "local-builder"

[data]
dir = "/var/lib/polkagent"
db = "sqlite"
max_artifact_size = "500MB"
artifact_retention_days = 365

[provider.anthropic]
model = "claude-opus-4-6"
api_key_ref = "secrets:anthropic_api_key"
max_tokens = 16384
timeout_seconds = 600

[provider.anthropic-fast]
model = "claude-sonnet-4-6"
api_key_ref = "secrets:anthropic_api_key"

[chain.polkadot]
endpoint = "wss://rpc.polkadot.io"
metadata_cache = true

[chain.polkadot-testnet]
endpoint = "wss://westend-rpc.polkadot.io"

[transport.polkadot_chat]
enabled = true
network = "polkadot"
poll_interval_seconds = 3

[execution]
max_concurrent_runs = 8
max_run_duration = "60m"
harness_timeout = "30m"

[secrets]
backend = "encrypted-file"
path = "/var/lib/polkagent/secrets.enc"

[policy]
default_grant = "read-only"
require_approval = ["chain-write", "payment", "deploy", "file-write"]

[observability]
log_level = "info"
metrics_enabled = true
metrics_endpoint = "http://prometheus:9090"
structured_logs = true

[api]
enabled = true
bind = "127.0.0.1:9400"
auth = "bearer-token"
auth_token_ref = "secrets:api_token"
```

### 14.3 Kubernetes cluster

```toml
# Kubernetes deployment configuration
[agent]
name = "org-agent-pool"
mode = "self-hosted"

[data]
db = "postgres"
db_url_ref = "secrets:database_url"
max_artifact_size = "1GB"
artifact_retention_days = 730

[data.artifacts]
backend = "s3"
bucket = "polkagent-artifacts"
region = "eu-central-1"
endpoint_ref = "secrets:s3_endpoint"

[provider.anthropic]
model = "claude-opus-4-6"
api_key_ref = "secrets:anthropic_api_key"

[execution]
max_concurrent_runs = 20
max_run_duration = "120m"
worker_count = 4

[secrets]
backend = "kubernetes"

[policy]
default_grant = "read-only"
require_approval = ["chain-write", "payment", "deploy"]

[observability]
log_level = "info"
metrics_enabled = true
metrics_format = "prometheus"
traces_enabled = true
traces_endpoint = "http://jaeger:14268/api/traces"

[api]
enabled = true
bind = "0.0.0.0:9400"
auth = "mtls"
```

### 14.4 Managed cloud (tenant configuration)

```toml
# Tenant configuration (managed cloud)
# Most infrastructure settings are managed by the platform

[agent]
name = "acme-defi-monitor"
mode = "managed"

[tenant]
id = "acme-corp"
organization = "acme-engineering"
project = "defi-bot"
environment = "production"

[provider.anthropic]
model = "claude-opus-4-6"
api_key_ref = "managed:anthropic_api_key"

[chain.polkadot]
endpoint = "managed:polkadot-rpc"       # Platform-managed RPC

[policy]
default_grant = "read-only"
require_approval = ["chain-write", "payment"]
max_run_budget_usd = 10.0

[execution]
max_concurrent_runs = 5
max_run_duration = "30m"

[notifications]
email = ["ops@acme.example"]
webhook = "https://acme.example/polkagent-webhook"
events = ["run-failed", "approval-required", "quota-warning"]
```

### 14.5 Hybrid deployment

```toml
# Hybrid: local workers + managed control plane
[agent]
name = "hybrid-agent"
mode = "hybrid"

[data]
dir = "/var/lib/polkagent"
db = "sqlite"

[control_plane]
enabled = true
endpoint = "https://cloud.polkagent.dev"
tenant_id = "acme-corp"
enrollment_key_ref = "secrets:enrollment_key"
pull_interval = "5m"
health_report_interval = "1m"

[provider.anthropic]
model = "claude-opus-4-6"
api_key_ref = "secrets:anthropic_api_key"    # Local secret, not uploaded

[chain.polkadot]
endpoint = "wss://rpc.polkadot.io"           # Direct, not proxied

[secrets]
backend = "encrypted-file"                   # Local, not cloud
path = "/var/lib/polkagent/secrets.enc"

[policy]
source = "control-plane"                     # Pull from control plane
local_override = true                        # Can override locally
cache_max_age = "24h"

[execution]
max_concurrent_runs = 4
```

---

## 15. Acceptance criteria and verification checklist

### 15.1 Self-hosted deployment acceptance

| ID | Criterion | Verification method |
|---|---|---|
| SH-01 | Single binary installs and runs on Linux x86_64, Linux aarch64, macOS x86_64, macOS aarch64 | CI builds and smoke tests on each platform |
| SH-02 | `polkagent init` completes first-run setup without errors | Automated test with mock provider |
| SH-03 | `polkagent doctor` validates all configured connections | Test with valid and invalid configurations |
| SH-04 | SQLite state survives daemon restart without data loss | Kill -9 during active run; verify state integrity on restart |
| SH-05 | Docker image runs as non-root user | Container scanning; security audit |
| SH-06 | Docker Compose starts complete stack with one command | Automated test in CI |
| SH-07 | systemd service starts, stops, and restarts cleanly | Integration test on Linux |
| SH-08 | Helm chart deploys to Kubernetes with default values | Test on kind/k3s in CI |
| SH-09 | Upgrade path preserves all data and configuration | Upgrade test from N-1 to N version |
| SH-10 | Export/import round-trip produces identical agent behavior | Behavioral equivalence test |

### 15.2 Managed cloud acceptance

| ID | Criterion | Verification method |
|---|---|---|
| MC-01 | Tenant creation and provisioning completes in <60 seconds | Load test with concurrent tenant creation |
| MC-02 | Cross-tenant data access is impossible through all API paths | Penetration test; attempt to read another tenant's data through every endpoint |
| MC-03 | Tenant-scoped encryption prevents platform operator data access | Key hierarchy audit; attempt plaintext access without tenant key |
| MC-04 | Worker auto-scaling responds to load within configured thresholds | Load test with variable job injection rates |
| MC-05 | Quota enforcement prevents resource exhaustion | Inject jobs exceeding quota; verify queuing and notification |
| MC-06 | Billing accurately reflects actual usage | Reconcile metered usage against known synthetic workloads |
| MC-07 | Regional data residency is enforced | Attempt cross-region data access; verify denial |
| MC-08 | Tenant offboarding purges all data after retention period | Verify no data remains after offboarding and retention expiry |

### 15.3 Worker architecture acceptance

| ID | Criterion | Verification method |
|---|---|---|
| WK-01 | Workers register and receive jobs within 30 seconds | Timing test in load scenario |
| WK-02 | Failed workers' jobs are reassigned after lease expiry | Kill worker during active job; verify reassignment |
| WK-03 | Auto-scaling adds workers when queue depth exceeds threshold | Inject burst of jobs; verify scaling event |
| WK-04 | Workers drain gracefully before shutdown | Send drain signal; verify current jobs complete and no new jobs accepted |
| WK-05 | Job priority is respected under load | Inject mixed-priority jobs; verify CRITICAL jobs scheduled first |

### 15.4 Offline-safe degradation acceptance (J3)

| ID | Criterion | Verification method |
|---|---|---|
| J3-01 | Disconnecting control plane does not stop active runs | Sever control-plane connectivity during active run; verify completion |
| J3-02 | Cached policy is used when control plane is unavailable | Disconnect; trigger new run; verify grant from cached policy |
| J3-03 | No grant is widened during disconnection | Disconnect; attempt action outside cached grant; verify denial |
| J3-04 | Paused actions resume correctly on reconnection | Disconnect; trigger action requiring fresh policy; reconnect; verify execution |
| J3-05 | Usage is accurately synced after reconnection | Disconnect; run workloads; reconnect; verify usage metrics match |
| J3-06 | Stale policy triggers visible warning | Set short max_age; disconnect beyond max_age; verify UI warning |

### 15.5 State portability acceptance

| ID | Criterion | Verification method |
|---|---|---|
| SP-01 | Export produces a valid, complete bundle | Export; validate against schema; verify checksums |
| SP-02 | Import restores all exported state | Export from source; import to fresh target; compare state |
| SP-03 | Import refuses concurrent identity conflict | Attempt import while source is still running; verify rejection |
| SP-04 | Import does not replay pending effects | Export with pending effects; import; verify effects in review queue, not executed |
| SP-05 | Migration from local to managed works end-to-end | Full migration drill; verify agent operates in managed mode |
| SP-06 | Migration from managed to local works end-to-end | Full migration drill; verify agent operates locally |
| SP-07 | Database migration SQLite to PostgreSQL succeeds | Automated migration test; verify row counts and data integrity |

### 15.6 Secrets management acceptance

| ID | Criterion | Verification method |
|---|---|---|
| SM-01 | Secrets are never written to disk in plaintext | File system audit during and after operation |
| SM-02 | Secrets are cleared from memory on shutdown | Memory inspection after graceful and ungraceful shutdown |
| SM-03 | Secret rotation does not interrupt active sessions | Rotate secret during active run; verify completion |
| SM-04 | Secret access is audited with actor and timestamp | Retrieve secret; verify audit log entry |
| SM-05 | KMS envelope encryption protects tenant secrets | Verify encrypted DEK without KEK is useless |
| SM-06 | OS keychain integration works on macOS and Linux | Automated test on both platforms |

### 15.7 Billing acceptance (managed mode)

| ID | Criterion | Verification method |
|---|---|---|
| BL-01 | Usage metering is accurate to within 1% | Compare metered usage against synthetic known workloads |
| BL-02 | Quota warnings fire at 80% and 90% thresholds | Inject usage approaching quota; verify notification timing |
| BL-03 | Quota enforcement prevents exceeding configured limits | Inject usage exceeding quota; verify enforcement behavior |
| BL-04 | Invoices are generated on the first of each month | Automated test with synthetic billing period |
| BL-05 | Usage dashboard shows real-time data within 2 minutes | Inject usage; verify dashboard update latency |

### 15.8 Cross-cutting acceptance

| ID | Criterion | Verification method |
|---|---|---|
| CC-01 | Same AgentSpec runs identically across all deployment modes | Deploy same spec locally, Docker, K8s, managed; compare behavior |
| CC-02 | Configuration precedence (CLI > env > file > defaults) works | Set conflicting values at each level; verify resolution |
| CC-03 | Structured logs include correlation IDs across planes | Trace a request from API to worker; verify correlation IDs |
| CC-04 | Health endpoint returns correct status under all conditions | Test healthy, degraded, and unhealthy states |
| CC-05 | No deployment mode introduces a correctness difference | Property tests comparing run outputs across modes |

---

## 16. Functional requirements index

| ID | Requirement | Priority | Plane | Phase |
|---|---|---|---|---|
| DEPLOY-001 | Single-binary local installation | P0 | Data + Execution | 0 |
| DEPLOY-002 | SQLite data plane with WAL mode | P0 | Data | 0 |
| DEPLOY-003 | Encrypted local secret backend | P0 | Data | 0 |
| DEPLOY-004 | CLI lifecycle commands (init, start, stop, status, doctor) | P0 | All | 0 |
| DEPLOY-005 | Configuration file with documented schema | P0 | All | 0 |
| DEPLOY-006 | Environment variable configuration override | P0 | All | 0 |
| DEPLOY-007 | Docker image and Dockerfile | P1 | All | 1 |
| DEPLOY-008 | Docker Compose with harness support | P1 | All | 1 |
| DEPLOY-009 | systemd service unit | P1 | All | 1 |
| DEPLOY-010 | Export all state to portable bundle | P0 | Data | 1 |
| DEPLOY-011 | Import state from portable bundle | P0 | Data | 1 |
| DEPLOY-012 | OS keychain secret backend | P1 | Data | 1 |
| DEPLOY-013 | PostgreSQL data plane backend | P1 | Data | 2 |
| DEPLOY-014 | Kubernetes Helm chart | P1 | All | 2 |
| DEPLOY-015 | Worker registration and discovery | P1 | Execution | 2 |
| DEPLOY-016 | Job scheduling with priorities | P1 | Execution | 2 |
| DEPLOY-017 | Health monitoring and failure detection | P1 | Execution | 2 |
| DEPLOY-018 | Control plane enrollment (optional) | P2 | Control | 2 |
| DEPLOY-019 | Signed configuration distribution | P2 | Control | 2 |
| DEPLOY-020 | Offline-safe degradation (J3 invariant) | P0 | Data | 2 |
| DEPLOY-021 | Multi-tenant data isolation | P1 | Data + Control | 3 |
| DEPLOY-022 | Tenant hierarchy (org/project/env) | P2 | Control | 3 |
| DEPLOY-023 | Role-based access control | P1 | Control | 3 |
| DEPLOY-024 | KMS/HSM secret backend | P2 | Data | 3 |
| DEPLOY-025 | Auto-scaling workers | P2 | Execution | 3 |
| DEPLOY-026 | Resource quotas per tenant | P2 | Execution | 3 |
| DEPLOY-027 | Usage metering and billing | P2 | Control | 3 |
| DEPLOY-028 | SLO monitoring and alerting | P2 | Control | 3 |
| DEPLOY-029 | Incident management integration | P2 | Control | 3 |
| DEPLOY-030 | Regional deployment | P3 | All | 4 |
| DEPLOY-031 | Desktop app wrapper | P3 | All | 4 |
| DEPLOY-032 | Dedicated infrastructure tier | P3 | All | 4 |

---

## 17. Non-functional requirements

| ID | Requirement | Target |
|---|---|---|
| NFR-01 | Daemon startup time (local, warm) | <2 seconds |
| NFR-02 | Daemon startup time (local, cold with DB migration) | <10 seconds |
| NFR-03 | Memory usage (idle daemon, no active runs) | <50 MB |
| NFR-04 | Memory usage (single active run, local) | <200 MB (excluding harness) |
| NFR-05 | Binary size (release, stripped) | <50 MB |
| NFR-06 | Docker image size | <100 MB |
| NFR-07 | Export time (1000 runs, 100 artifacts) | <30 seconds |
| NFR-08 | Import time (1000 runs, 100 artifacts) | <60 seconds |
| NFR-09 | Config change propagation (control plane to data plane) | <5 minutes (p95) |
| NFR-10 | Worker registration to first job | <30 seconds |

---

## 18. Security considerations

### 18.1 Deployment-specific threats

| Threat | Deployment mode | Mitigation |
|---|---|---|
| Stolen laptop with agent secrets | Local | Encrypted secrets with passphrase; full-disk encryption recommended |
| Compromised container image | Docker/K8s | Signed images; vulnerability scanning; minimal base image |
| Network eavesdropping | All | TLS everywhere; mutual TLS for internal communication |
| Privilege escalation in container | Docker/K8s | Non-root user; dropped capabilities; seccomp profiles; read-only filesystem |
| Cross-tenant data leak | Managed | Row-level security; encryption per tenant; network isolation; regular penetration testing |
| Control plane compromise | Managed/Hybrid | Data plane verifies signed revisions; control plane cannot access secrets; tenant keys separate from platform keys |
| Supply chain attack on binary | All | Reproducible builds; signed releases; SBOM; dependency auditing |
| Insider threat (platform operator) | Managed | Tenant-specific encryption; break-glass audit; no default plaintext access |

### 18.2 Required security controls by deployment mode

| Control | Local | Self-hosted | Managed |
|---|---|---|---|
| TLS for external connections | Required | Required | Required |
| Mutual TLS for internal | N/A | Recommended | Required |
| Secret encryption at rest | Required | Required | Required (KMS) |
| Database encryption | Recommended | Recommended | Required |
| Network isolation | N/A | Recommended | Required |
| Image signing | N/A | Recommended | Required |
| RBAC | N/A | Optional | Required |
| Audit logging | Optional | Recommended | Required |
| Penetration testing | N/A | Recommended | Required (annual) |
| Vulnerability scanning | N/A | Recommended | Required (continuous) |

---

## 19. Observability by deployment mode

| Signal | Local | Self-hosted | Managed |
|---|---|---|---|
| Structured logs | JSON file | JSON file or syslog | Centralized log aggregation |
| Metrics | `polkagent status` CLI | Prometheus endpoint | Managed Prometheus/Grafana |
| Traces | N/A | Optional Jaeger/OTLP | Managed tracing service |
| Health checks | `polkagent doctor` | HTTP /health endpoint | Synthetic monitoring + health endpoint |
| Alerting | N/A | Optional integration | PagerDuty/Opsgenie integration |
| Dashboards | CLI status | Optional Grafana | Managed dashboards |

---

## 20. Migration from PCA deployment

Existing `polkadot-chat-agents` deployments can migrate to Polkagent
self-hosted or managed deployments. The migration path:

1. **Assess:** `polkagent migrate assess --pca-dir /path/to/pca` scans the
   PCA installation and produces a compatibility report.
2. **Plan:** review the report; identify unsupported configuration, required
   secret rebinding, and pending work disposition.
3. **Export:** `polkagent migrate export-pca --pca-dir /path/to/pca` produces
   a Polkagent-compatible import bundle from PCA state.
4. **Import:** `polkagent import` with the bundle; review the import report.
5. **Verify:** `polkagent doctor` validates all connections and state.
6. **Cutover:** stop PCA; start Polkagent; monitor for issues.
7. **Rollback:** if issues arise, stop Polkagent; restart PCA; state is
   unchanged.

The PCA migration tool never modifies PCA files. The PCA installation remains
a viable rollback target until the operator explicitly decommissions it.

---

## 21. Glossary of requirement IDs

All requirement IDs in this document follow the pattern `DEPLOY-NNN` for
functional requirements, `NFR-NN` for non-functional requirements, and
two-letter prefixes for acceptance criteria (SH = self-hosted, MC = managed
cloud, WK = worker, J3 = offline-safe, SP = state portability, SM = secrets,
BL = billing, CC = cross-cutting).

---

## 22. Open questions and future work

| ID | Question | Owner | Target resolution |
|---|---|---|---|
| OQ-01 | Exact object store provider for managed artifact storage | Infrastructure | Phase 2 spike |
| OQ-02 | Queue technology selection (Redis vs NATS vs PostgreSQL SKIP LOCKED) | Infrastructure | Phase 2 spike |
| OQ-03 | Desktop app framework selection (Tauri vs Electron) | UX | Phase 4 |
| OQ-04 | Multi-region replication strategy for control plane metadata | Infrastructure | Phase 4 |
| OQ-05 | Compliance certifications to pursue (SOC 2, ISO 27001) | Legal/Security | Phase 3 |
| OQ-06 | Pricing model for managed tiers | Business | Phase 3 |
| OQ-07 | Self-hosted control plane packaging (binary vs Helm chart) | Infrastructure | Phase 3 |

### 22.1 Staged implementation recommendations

The following sequencing is drawn from research synthesis. It is advisory, not
normative; the functional requirements in section 16 govern priority.

**Do now (Phase 0-1):**
- Ship the single-binary release with systemd unit and full hardening
  (`Type=notify`, `WatchdogSec`, all `Protect*` and `Restrict*` directives).
- Implement pull-based config with signed revisions and graceful degradation
  before adding any control-plane features. The degraded-mode path is safer
  to build first than to retrofit later.
- Publish the distroless Docker image as the canonical release artifact.

**Validate next (Phase 2-3):**
- Tenant row-level scoping and Cedar policy isolation before opening managed
  cloud to external tenants. Cross-tenant data leaks are not recoverable.
- Usage counters at the policy gate (token/effect tracking) before exposing
  usage-based billing. Quota enforcement must be correct before billing is
  real.

**Defer (Phase 3-4):**
- Kubernetes operator (vs Helm chart) until operator adoption is proven.
  Helm is sufficient for Phase 2-3.
- Regional data residency until there is a concrete regulatory or customer
  requirement. Over-engineering multi-region too early creates operational
  complexity that slows the core product.

**Avoid permanently:**
- OSS/cloud codebase divergence. Once the split exists it compounds; do not
  introduce it under schedule pressure. Any feature that "only makes sense
  for cloud" should be re-examined -- it usually means a missing abstraction
  in the config-gating layer.

---

## 23. Document history

| Date | Change |
|---|---|
| 2026-07-30 | Initial definitive PRD created from research corpus |
| 2026-07-30 | Integrated research3.md findings: single-binary positioning, distroless image, systemd watchdog, mTLS enrollment, codebase-divergence prohibition, Cedar tenant policy isolation, RLS details, GDPR export/delete, policy-gate quota enforcement, external-AI-provider SLO domain, staged recommendations |
| 2026-07-30 | Appended implementation blueprints (Appendices A–H): data/execution/control plane Rust interfaces, Kubernetes manifests, Helm chart templates, Dockerfile patterns (referencing Roko docker/), Docker Compose with observability stack, HA/DR procedures, ordered implementation checklist, reference file map, TUI wireframes (referencing Bardo system.rs), configuration guide |

---

## APPENDIX A: DEPLOYMENT IMPLEMENTATION BLUEPRINT

### A.1 Data Plane Implementation

The data plane is the only layer that must exist in every deployment mode. Its
implementation is a set of Rust traits (port interfaces) with two concrete
adapters: SQLite for local/single-node and PostgreSQL for multi-node/managed.
A third cross-cutting concern is artifact storage, which parallels the DB
adapter split. Secret management is a fourth pluggable backend selected at
startup by configuration.

#### A.1.1 Port interfaces

Each responsibility from section 4.2.1 maps to one Rust trait. All traits are
object-safe and `Send + Sync + 'static` so they can be stored in `Arc<dyn T>`
and shared across Tokio tasks.

```rust
// crates/polkagent-data/src/ports/mod.rs

pub mod state;      // StateStore
pub mod artifacts;  // ArtifactStore
pub mod secrets;    // SecretBackend
pub mod policy;     // PolicyCache
pub mod transport;  // TransportSessionStore
```

```rust
// crates/polkagent-data/src/ports/state.rs

use polkagent_core::ids::*;
use polkagent_core::types::*;

/// Primary state persistence port.
/// Implemented by SqliteStateStore and PostgresStateStore.
#[async_trait::async_trait]
pub trait StateStore: Send + Sync + 'static {
    // ---- Runs ----
    async fn create_run(&self, spec: &RunSpec) -> Result<RunId>;
    async fn get_run(&self, id: RunId) -> Result<Option<RunRecord>>;
    async fn list_runs(&self, filter: &RunFilter) -> Result<Vec<RunSummary>>;
    async fn update_run_status(&self, id: RunId, status: RunStatus) -> Result<()>;

    // ---- Effects ----
    async fn record_intent(&self, run: RunId, intent: &EffectIntent) -> Result<IntentId>;
    async fn record_attempt(
        &self,
        intent: IntentId,
        attempt: u32,
        started_at: Timestamp,
    ) -> Result<AttemptId>;
    async fn record_outcome(
        &self,
        intent: IntentId,
        attempt: u32,
        outcome: &EffectOutcome,
    ) -> Result<()>;

    // ---- Conversations ----
    async fn create_conversation(&self, spec: &ConversationSpec) -> Result<ConversationId>;
    async fn append_turn(&self, conv: ConversationId, turn: &Turn) -> Result<TurnId>;
    async fn get_conversation(&self, id: ConversationId) -> Result<Option<Conversation>>;

    // ---- Policy cache ----
    async fn get_policy_cache(&self) -> Result<Option<PolicyCache>>;
    async fn put_policy_cache(&self, cache: &PolicyCache) -> Result<()>;

    // ---- Portability ----
    async fn export(&self, filter: &ExportFilter) -> Result<ExportBundle>;
    async fn import(&self, bundle: &ExportBundle, opts: &ImportOptions) -> Result<ImportReport>;

    // ---- Health ----
    async fn health(&self) -> Result<DataPlaneHealth>;
}
```

```rust
// crates/polkagent-data/src/ports/artifacts.rs

#[async_trait::async_trait]
pub trait ArtifactStore: Send + Sync + 'static {
    /// Write artifact bytes and return a content-addressed handle.
    async fn put(
        &self,
        run: RunId,
        name: &str,
        content_type: &str,
        data: bytes::Bytes,
    ) -> Result<ArtifactHandle>;

    /// Read artifact bytes. Returns None if not found.
    async fn get(&self, handle: &ArtifactHandle) -> Result<Option<bytes::Bytes>>;

    /// List artifact handles for a run.
    async fn list_for_run(&self, run: RunId) -> Result<Vec<ArtifactHandle>>;

    /// Delete all artifacts for a run (used in data purge / GDPR delete).
    async fn delete_for_run(&self, run: RunId) -> Result<u64>;

    /// Total bytes stored for a tenant (quota accounting).
    async fn bytes_used(&self, tenant: TenantId) -> Result<u64>;
}
```

```rust
// crates/polkagent-data/src/ports/secrets.rs

#[async_trait::async_trait]
pub trait SecretBackend: Send + Sync + 'static {
    /// Resolve a secret reference to its plaintext value (in-memory only).
    /// The returned String must be zeroed after use.
    async fn resolve(&self, reference: &SecretRef) -> Result<SecretValue>;

    /// Store a new secret. Returns the reference to use in config.
    async fn put(&self, name: &str, value: &SecretValue) -> Result<SecretRef>;

    /// Delete a secret.
    async fn delete(&self, reference: &SecretRef) -> Result<()>;

    /// Rotate a secret: write new value, verify, then mark old as deprecated.
    async fn rotate(
        &self,
        reference: &SecretRef,
        new_value: &SecretValue,
    ) -> Result<SecretRef>;

    /// List secret names (not values) for operator review.
    async fn list_names(&self, tenant: TenantId) -> Result<Vec<String>>;
}
```

#### A.1.2 SQLite adapter

The SQLite adapter is the default for local and single-node deployments. It
uses `sqlx` with the `sqlite` feature, WAL mode enabled at connection, and
`PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;`
applied at every new connection.

```rust
// crates/polkagent-data/src/adapters/sqlite/mod.rs

use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
use std::path::Path;

pub struct SqliteStateStore {
    pool: SqlitePool,
}

impl SqliteStateStore {
    pub async fn open(path: &Path) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5));

        let pool = SqlitePool::connect_with(options).await?;

        // Apply migrations at startup (hold-and-check pattern).
        sqlx::migrate!("./migrations/sqlite").run(&pool).await?;

        Ok(Self { pool })
    }
}

// Migration file layout:
// crates/polkagent-data/migrations/sqlite/
//   0001_initial_schema.up.sql
//   0001_initial_schema.down.sql
//   0002_add_tenant_id.up.sql
//   0002_add_tenant_id.down.sql
//   ...
```

Key SQLite schema patterns:

```sql
-- migrations/sqlite/0001_initial_schema.up.sql

CREATE TABLE IF NOT EXISTS runs (
    id          TEXT PRIMARY KEY,
    agent_id    TEXT NOT NULL,
    tenant_id   TEXT NOT NULL DEFAULT 'local',
    status      TEXT NOT NULL DEFAULT 'created',
    spec_json   TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS effect_intents (
    id          TEXT PRIMARY KEY,
    run_id      TEXT NOT NULL REFERENCES runs(id),
    tenant_id   TEXT NOT NULL DEFAULT 'local',
    family      TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    recorded_at TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS effect_outcomes (
    intent_id   TEXT NOT NULL REFERENCES effect_intents(id),
    attempt     INTEGER NOT NULL,
    status      TEXT NOT NULL,
    result_json TEXT,
    completed_at TEXT NOT NULL,
    PRIMARY KEY (intent_id, attempt)
) STRICT;

-- Row-level scoping enforced in application code for SQLite
-- (no native RLS in SQLite; enforced by always including tenant_id in WHERE).
```

#### A.1.3 PostgreSQL adapter

The PostgreSQL adapter is used for multi-node self-hosted and managed cloud
deployments. It uses `sqlx` with the `postgres` feature, connection pooling
via `PgPool`, and native PostgreSQL row-level security for tenant isolation.

```rust
// crates/polkagent-data/src/adapters/postgres/mod.rs

use sqlx::{PgPool, postgres::PgConnectOptions};

pub struct PostgresStateStore {
    pool: PgPool,
    tenant_id: TenantId,
}

impl PostgresStateStore {
    pub async fn connect(url: &str, tenant_id: TenantId) -> Result<Self> {
        let pool = PgPool::connect(url).await?;

        // Apply migrations.
        sqlx::migrate!("./migrations/postgres").run(&pool).await?;

        Ok(Self { pool, tenant_id })
    }

    /// Set the current tenant context for this connection so RLS policies fire.
    async fn set_tenant_ctx(&self, conn: &mut sqlx::PgConnection) -> Result<()> {
        sqlx::query("SELECT set_config('app.current_tenant', $1, true)")
            .bind(self.tenant_id.as_str())
            .execute(conn)
            .await?;
        Ok(())
    }
}
```

Key PostgreSQL schema patterns:

```sql
-- migrations/postgres/0001_initial_schema.up.sql

CREATE TABLE IF NOT EXISTS runs (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id    UUID NOT NULL,
    tenant_id   TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'created',
    spec_jsonb  JSONB NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Tenant isolation via RLS.
ALTER TABLE runs ENABLE ROW LEVEL SECURITY;
CREATE POLICY runs_tenant_isolation ON runs
    USING (tenant_id = current_setting('app.current_tenant'));

-- Index to ensure queries always include tenant_id.
CREATE INDEX idx_runs_tenant_id ON runs (tenant_id);
CREATE INDEX idx_runs_agent_id ON runs (tenant_id, agent_id);
```

#### A.1.4 Artifact storage: filesystem vs S3

```rust
// crates/polkagent-data/src/adapters/artifacts/mod.rs

pub struct FilesystemArtifactStore {
    root: std::path::PathBuf,
}

pub struct S3ArtifactStore {
    client: aws_sdk_s3::Client,
    bucket: String,
    prefix: String, // tenant_id/
}

// Path scheme for filesystem:
//   {root}/{tenant_id}/{run_id}/{artifact_id}.{ext}
//
// Key scheme for S3:
//   {prefix}{tenant_id}/{run_id}/{artifact_id}.{ext}
//
// Both produce stable, content-addressable paths.
// The handle encodes: store_type, tenant_id, run_id, artifact_id, content_type.
```

Selection by configuration:

```toml
# Local / single-node:
[data.artifacts]
backend = "filesystem"
root = "~/.polkagent/artifacts"

# Self-hosted cluster or managed cloud:
[data.artifacts]
backend = "s3"
bucket  = "polkagent-artifacts-prod"
region  = "eu-central-1"
prefix  = "tenant-data/"
# credentials resolved via instance role / IRSA / env -- never in config
```

#### A.1.5 Secret management backend selection

Secret backend is selected at daemon startup by reading `[secrets] backend`
from config. The selected backend is stored as `Arc<dyn SecretBackend>` and
injected into the data plane via constructor injection.

```rust
// crates/polkagent-daemon/src/startup.rs

pub async fn build_secret_backend(cfg: &SecretsConfig) -> Result<Arc<dyn SecretBackend>> {
    match cfg.backend.as_str() {
        "encrypted-file" => {
            let store = EncryptedFileBackend::open(&cfg.path, &cfg.passphrase_ref).await?;
            Ok(Arc::new(store))
        }
        "os-keychain" => {
            Ok(Arc::new(OsKeychainBackend::new(&cfg.service_name)?))
        }
        "env" => {
            Ok(Arc::new(EnvBackend::load_and_clear()?))
        }
        "kubernetes" => {
            let store = KubernetesSecretBackend::from_in_cluster_config().await?;
            Ok(Arc::new(store))
        }
        "vault" => {
            let store = VaultBackend::connect(&cfg.vault_addr, &cfg.vault_role).await?;
            Ok(Arc::new(store))
        }
        "aws-kms" => {
            let store = AwsKmsBackend::from_env(cfg.kms_key_id.clone()).await?;
            Ok(Arc::new(store))
        }
        other => Err(ConfigError::UnknownSecretBackend(other.to_string()).into()),
    }
}
```

---

### A.2 Execution Plane Implementation

The execution plane manages worker lifecycle, job scheduling, and resource
enforcement. In-process (local) and distributed (cluster) modes share the
same `Worker` and `Scheduler` traits; only the backing infrastructure differs.

#### A.2.1 Worker registry and lifecycle

```rust
// crates/polkagent-execution/src/registry.rs

use dashmap::DashMap;
use std::sync::Arc;

/// Thread-safe registry of live workers.
pub struct WorkerRegistry {
    workers: DashMap<WorkerId, WorkerEntry>,
}

pub struct WorkerEntry {
    pub worker: Arc<dyn Worker>,
    pub last_heartbeat: Timestamp,
    pub health: WorkerHealth,
    pub status: RegistrationStatus,
}

pub enum RegistrationStatus {
    Active,
    Draining,
    Unhealthy { since: Timestamp, reason: String },
    Deregistered { at: Timestamp },
}

impl WorkerRegistry {
    pub fn register(&self, worker: Arc<dyn Worker>) -> WorkerId {
        let id = worker.id();
        self.workers.insert(id, WorkerEntry {
            worker,
            last_heartbeat: Timestamp::now(),
            health: WorkerHealth::default(),
            status: RegistrationStatus::Active,
        });
        id
    }

    /// Called by the heartbeat loop. Returns false if worker should stop.
    pub fn heartbeat(&self, id: WorkerId, health: WorkerHealth) -> bool {
        if let Some(mut entry) = self.workers.get_mut(&id) {
            entry.last_heartbeat = Timestamp::now();
            entry.health = health;
            matches!(entry.status, RegistrationStatus::Active)
        } else {
            false // unknown worker; should stop
        }
    }

    /// Mark a worker for graceful drain.
    pub fn drain(&self, id: WorkerId) {
        if let Some(mut entry) = self.workers.get_mut(&id) {
            entry.status = RegistrationStatus::Draining;
        }
    }

    /// Returns candidate workers for scheduling (Active, not overloaded).
    pub fn candidates(&self, req: &JobRequirements) -> Vec<Arc<dyn Worker>> {
        self.workers
            .iter()
            .filter(|e| matches!(e.status, RegistrationStatus::Active))
            .filter(|e| e.worker.capabilities().satisfies(req))
            .filter(|e| e.health.current_jobs < e.health.max_jobs)
            .map(|e| Arc::clone(&e.worker))
            .collect()
    }
}
```

#### A.2.2 Job scheduler: FIFO, priority, and fairness

The scheduler implements a multi-level priority queue with tenant-level
fairness to prevent any single tenant from starving others at the NORMAL
priority level.

```rust
// crates/polkagent-execution/src/scheduler.rs

use priority_queue::PriorityQueue;

pub struct Scheduler {
    queues: Arc<PriorityQueues>,
    registry: Arc<WorkerRegistry>,
    state_store: Arc<dyn StateStore>,
    metrics: Arc<SchedulerMetrics>,
}

struct PriorityQueues {
    critical: Mutex<VecDeque<QueuedJob>>,
    high:     Mutex<VecDeque<QueuedJob>>,
    normal:   Mutex<FairQueue>,   // round-robin across tenants
    low:      Mutex<VecDeque<QueuedJob>>,
    bulk:     Mutex<VecDeque<QueuedJob>>,
}

/// Fair queue implements weighted round-robin across tenant sub-queues.
struct FairQueue {
    tenant_queues: BTreeMap<TenantId, VecDeque<QueuedJob>>,
    turn: usize,  // index into sorted tenant list
}

impl FairQueue {
    fn push(&mut self, job: QueuedJob) {
        self.tenant_queues
            .entry(job.tenant_id.clone())
            .or_default()
            .push_back(job);
    }

    fn pop(&mut self) -> Option<QueuedJob> {
        // Round-robin: advance turn index, skip empty tenant queues.
        let tenant_ids: Vec<_> = self.tenant_queues.keys().cloned().collect();
        if tenant_ids.is_empty() { return None; }
        for _ in 0..tenant_ids.len() {
            let i = self.turn % tenant_ids.len();
            self.turn = self.turn.wrapping_add(1);
            if let Some(q) = self.tenant_queues.get_mut(&tenant_ids[i]) {
                if let Some(job) = q.pop_front() {
                    // Remove empty tenant queue to keep the map lean.
                    if q.is_empty() {
                        self.tenant_queues.remove(&tenant_ids[i]);
                    }
                    return Some(job);
                }
            }
        }
        None
    }
}

impl Scheduler {
    /// Main dispatch loop. Runs as a background Tokio task.
    pub async fn run(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        loop {
            interval.tick().await;
            self.dispatch_one().await;
        }
    }

    async fn dispatch_one(&self) {
        // Pull next job in priority order.
        let job = self.queues.pop_next();
        let Some(job) = job else { return };

        // Find a suitable worker.
        let candidates = self.registry.candidates(&job.requirements);
        let worker = self.select_worker(candidates, &job);
        let Some(worker) = worker else {
            // No worker available; re-queue.
            self.queues.push(job);
            return;
        };

        // Assign with lease.
        let lease = Lease::new(worker.id(), Duration::from_secs(90));
        tokio::spawn(async move {
            let result = worker.execute(job.inner).await;
            // Record outcome, renew or release lease.
            let _ = result;
        });
    }

    fn select_worker(
        &self,
        candidates: Vec<Arc<dyn Worker>>,
        job: &QueuedJob,
    ) -> Option<Arc<dyn Worker>> {
        candidates.into_iter().min_by_key(|w| {
            let h = self.registry.health(w.id());
            // Lower score = preferred: fewer current jobs relative to capacity.
            (h.current_jobs as f64 / h.max_jobs.max(1) as f64 * 1000.0) as u64
        })
    }
}
```

#### A.2.3 Resource quota enforcement

Quota enforcement runs inside the policy gate (see section 11.4.0). The
execution plane enforces resource limits at the worker level as a second
check.

```rust
// crates/polkagent-execution/src/quota.rs

pub struct QuotaEnforcer {
    store: Arc<dyn StateStore>,
    limits: TenantLimits,
}

impl QuotaEnforcer {
    /// Returns Ok(()) if the job can proceed, Err(QuotaExceeded) otherwise.
    pub async fn check(&self, job: &Job) -> Result<(), QuotaError> {
        let usage = self.store.get_current_usage(job.tenant_id).await?;

        // Concurrent run limit.
        if usage.active_runs >= self.limits.max_concurrent_runs {
            return Err(QuotaError::ConcurrentRunsExceeded {
                current: usage.active_runs,
                limit: self.limits.max_concurrent_runs,
            });
        }

        // Memory request vs limit.
        if job.resource_request.memory_bytes > self.limits.max_memory_per_run {
            return Err(QuotaError::MemoryRequestExceedsLimit {
                requested: job.resource_request.memory_bytes,
                limit: self.limits.max_memory_per_run,
            });
        }

        // Compute budget (token/effect counters tracked at policy gate).
        // Execution plane checks artifact storage quota.
        let artifact_bytes = self.store.artifact_bytes_used(job.tenant_id).await?;
        if artifact_bytes > self.limits.max_artifact_bytes {
            return Err(QuotaError::ArtifactStorageExceeded {
                current_bytes: artifact_bytes,
                limit_bytes: self.limits.max_artifact_bytes,
            });
        }

        Ok(())
    }
}
```

#### A.2.4 Health checking: liveness, readiness, startup probes

The daemon exposes an HTTP `/health` endpoint with three sub-paths that map
directly to Kubernetes probe semantics. The endpoint is implemented with
`axum` and runs on a dedicated port (`POLKAGENT_HEALTH_PORT`, default 9401)
separate from the main API port so health checks never compete with API
traffic.

```rust
// crates/polkagent-daemon/src/health.rs

use axum::{Router, extract::State, Json};

pub fn health_router(state: Arc<DaemonState>) -> Router {
    Router::new()
        .route("/health/live",    axum::routing::get(liveness))
        .route("/health/ready",   axum::routing::get(readiness))
        .route("/health/startup", axum::routing::get(startup))
        .with_state(state)
}

/// Liveness: is the process alive and not deadlocked?
/// Fails only if the process should be killed and restarted.
async fn liveness(State(s): State<Arc<DaemonState>>) -> StatusCode {
    if s.is_alive() { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE }
}

/// Readiness: can the daemon accept requests?
/// Fails during startup, shutdown drain, or DB unavailability.
async fn readiness(State(s): State<Arc<DaemonState>>) -> (StatusCode, Json<ReadinessReport>) {
    let report = s.readiness_report().await;
    let code = if report.ready { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (code, Json(report))
}

/// Startup: has the daemon finished initial startup (migrations, secret load)?
/// Kubernetes startup probe uses this to avoid killing the process during migration.
async fn startup(State(s): State<Arc<DaemonState>>) -> StatusCode {
    if s.startup_complete() { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE }
}

#[derive(serde::Serialize)]
pub struct ReadinessReport {
    pub ready: bool,
    pub db: ComponentStatus,
    pub secret_backend: ComponentStatus,
    pub transport: ComponentStatus,
    pub workers: ComponentStatus,
    pub control_plane: Option<ComponentStatus>,
}
```

#### A.2.5 Graceful shutdown and drain

Graceful shutdown is initiated by `SIGTERM` (Kubernetes default) or
`polkagent stop --graceful`. The shutdown sequence:

> **Current bounded evidence (2026-08-05):** `polkagent serve` handles SIGTERM
> and SIGINT through Axum graceful shutdown, so the listener stops accepting
> and active HTTP connections drain before a clean process exit. The container
> smoke verifies that boundary. The broader daemon sequence below—run/worker
> admission control, durable checkpoints, effect/transport flush, timeout
> escalation, and `polkagent stop --graceful`—is not implemented; EP-09 and
> HA-03 remain open.

```rust
// crates/polkagent-daemon/src/shutdown.rs

pub async fn graceful_shutdown(state: Arc<DaemonState>, timeout: Duration) {
    tracing::info!("graceful shutdown initiated");

    // 1. Stop accepting new jobs.
    state.scheduler().pause_intake().await;

    // 2. Drain active workers: mark them Draining, wait for jobs to complete.
    let drain_deadline = Instant::now() + timeout;
    let workers = state.registry().all_active();
    for worker in &workers {
        worker.drain().await.ok();
    }

    // 3. Wait for all in-flight jobs to complete or timeout.
    loop {
        if Instant::now() > drain_deadline {
            tracing::warn!("drain timeout; {} jobs still running", state.active_job_count());
            break;
        }
        if state.active_job_count() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // 4. Flush state DB write-ahead log.
    state.data_plane().flush().await.ok();

    // 5. Signal readiness probe as not-ready (Kubernetes will stop routing).
    state.set_draining(true);

    // 6. Close transport sessions cleanly.
    state.transport().shutdown().await.ok();

    // 7. Close DB pool.
    state.data_plane().close().await.ok();

    tracing::info!("graceful shutdown complete");
}
```

---

### A.3 Control Plane Implementation

The control plane is an optional suite of services. In Phase 2-3 it can be
deployed as separate binaries on the same host or as microservices in
Kubernetes.

#### A.3.1 Tenant management API

```rust
// crates/polkagent-control/src/api/tenants.rs

use axum::{Router, extract::{State, Path, Json}};

pub fn tenant_router(state: Arc<ControlState>) -> Router {
    Router::new()
        .route("/tenants",            axum::routing::post(create_tenant))
        .route("/tenants/:id",        axum::routing::get(get_tenant))
        .route("/tenants/:id",        axum::routing::patch(update_tenant))
        .route("/tenants/:id/suspend",axum::routing::post(suspend_tenant))
        .route("/tenants/:id/delete", axum::routing::post(offboard_tenant))
        .with_state(state)
}

async fn create_tenant(
    State(s): State<Arc<ControlState>>,
    Json(req): Json<CreateTenantRequest>,
) -> Result<Json<TenantResponse>, ApiError> {
    // 1. Validate plan and configuration.
    // 2. Provision tenant: create DB schema, S3 prefix, KMS key hierarchy.
    // 3. Create default org, project, environment.
    // 4. Return tenant id and enrollment endpoint.
    let tenant = s.tenant_service().create(req).await?;
    Ok(Json(TenantResponse::from(tenant)))
}
```

#### A.3.2 Fleet management

Fleet management tracks which data planes are enrolled, their current config
revision, and their health status.

```rust
// crates/polkagent-control/src/fleet/manager.rs

pub struct FleetManager {
    enrollments: Arc<dyn EnrollmentStore>,
    config_distributor: Arc<ConfigDistributor>,
    health_aggregator: Arc<HealthAggregator>,
}

impl FleetManager {
    /// Process an enrollment request from a new data plane.
    pub async fn enroll(&self, req: EnrollmentRequest) -> Result<EnrollmentResponse> {
        // Verify tenant membership, agent identity signature.
        self.verify_enrollment(&req)?;

        // Issue a short-lived enrollment certificate (mTLS).
        let cert = self.issue_enrollment_cert(&req).await?;

        // Record enrollment: tenant_id, agent_id, worker_id, public_key, capabilities.
        self.enrollments.record(EnrollmentRecord {
            tenant_id: req.tenant_id.clone(),
            agent_id: req.agent_id.clone(),
            public_key: req.public_key.clone(),
            enrolled_at: Timestamp::now(),
            current_revision: None,
        }).await?;

        Ok(EnrollmentResponse {
            worker_id: req.agent_id.clone(),
            enrollment_cert: cert,
            pull_endpoint: self.pull_endpoint(),
            pull_interval_secs: 300,
        })
    }

    /// Distribute a new desired-state revision to enrolled data planes.
    pub async fn distribute_revision(
        &self,
        tenant_id: &TenantId,
        revision: SignedConfigRevision,
    ) -> Result<DistributionReport> {
        let enrolled = self.enrollments.list_for_tenant(tenant_id).await?;
        self.config_distributor.publish(tenant_id, revision, enrolled).await
    }
}
```

#### A.3.3 Policy distribution

Policy revisions are signed with the tenant's management key before
distribution. The data plane verifies the signature before applying.

```rust
// crates/polkagent-control/src/policy/distributor.rs

pub struct PolicyDistributor {
    signing_key: TenantSigningKey,
    store: Arc<dyn PolicyRevisionStore>,
}

impl PolicyDistributor {
    pub async fn publish(
        &self,
        tenant_id: &TenantId,
        policy: Policy,
        description: &str,
    ) -> Result<SignedConfigRevision> {
        let revision_id = RevisionId::new();
        let payload = serde_json::to_vec(&policy)?;
        let signature = self.signing_key.sign(&payload);

        let signed = SignedConfigRevision {
            id: revision_id,
            tenant_id: tenant_id.clone(),
            payload,
            signature,
            signed_at: Timestamp::now(),
            description: description.to_string(),
        };

        self.store.put(tenant_id, &signed).await?;
        Ok(signed)
    }
}
```

#### A.3.4 Billing and usage tracking

Usage events flow from workers through a metering queue. The aggregator
batches them into the billing database.

```rust
// crates/polkagent-control/src/billing/aggregator.rs

pub struct UsageAggregator {
    queue: Arc<dyn UsageQueue>,
    db: Arc<dyn BillingDb>,
    quota_enforcer: Arc<QuotaService>,
}

impl UsageAggregator {
    pub async fn run(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            self.flush_batch().await;
        }
    }

    async fn flush_batch(&self) {
        let events = self.queue.drain_batch(1000).await;
        if events.is_empty() { return; }

        // Aggregate by (tenant_id, dimension, hour).
        let aggregated = aggregate(events);

        // Persist to billing DB.
        self.db.upsert_usage_batch(&aggregated).await.ok();

        // Check quotas and emit alerts.
        for (tenant_id, usage) in &aggregated {
            self.quota_enforcer.check_and_alert(tenant_id, usage).await.ok();
        }
    }
}
```

#### A.3.5 Admin dashboard API

```rust
// crates/polkagent-control/src/api/admin.rs

pub fn admin_router(state: Arc<ControlState>) -> Router {
    Router::new()
        // Fleet
        .route("/admin/fleet",                  axum::routing::get(list_enrolled))
        .route("/admin/fleet/:id/drain",        axum::routing::post(drain_worker))
        .route("/admin/fleet/:id/rollout",      axum::routing::post(trigger_rollout))
        // Billing
        .route("/admin/tenants/:id/usage",      axum::routing::get(get_usage))
        .route("/admin/tenants/:id/quota",      axum::routing::patch(update_quota))
        // Observability
        .route("/admin/health",                 axum::routing::get(fleet_health_summary))
        .route("/admin/incidents",              axum::routing::get(list_incidents))
        .layer(RequireRole::new(Role::Owner))
        .with_state(state)
}
```

---

## APPENDIX B: KUBERNETES DEPLOYMENT

### B.1 Kubernetes Manifests

All manifests live in `deploy/k8s/` under the repository root. They are
consumed by the Helm chart via template generation; the raw manifests here
show the authoritative shape for reference.

#### B.1.1 Worker Deployment

```yaml
# deploy/k8s/worker-deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: polkagent-worker
  namespace: polkagent-workers
  labels:
    app.kubernetes.io/name: polkagent-worker
    app.kubernetes.io/component: worker
    app.kubernetes.io/version: "{{ .Values.image.tag }}"
spec:
  replicas: {{ .Values.workers.replicas }}
  selector:
    matchLabels:
      app.kubernetes.io/name: polkagent-worker
  strategy:
    type: RollingUpdate
    rollingUpdate:
      maxUnavailable: 1
      maxSurge: 1
  template:
    metadata:
      labels:
        app.kubernetes.io/name: polkagent-worker
        app.kubernetes.io/component: worker
      annotations:
        prometheus.io/scrape: "true"
        prometheus.io/port: "9401"
        prometheus.io/path: "/metrics"
    spec:
      serviceAccountName: polkagent-worker
      securityContext:
        runAsNonRoot: true
        runAsUser: 1000
        fsGroup: 1000
        seccompProfile:
          type: RuntimeDefault
      terminationGracePeriodSeconds: 120
      containers:
        - name: polkagent
          image: "{{ .Values.image.repository }}:{{ .Values.image.tag }}"
          imagePullPolicy: {{ .Values.image.pullPolicy }}
          args: ["start", "--data-dir", "/data"]
          ports:
            - name: api
              containerPort: 9400
              protocol: TCP
            - name: health
              containerPort: 9401
              protocol: TCP
          env:
            - name: POLKAGENT_DATA__DB
              value: "postgres"
            - name: POLKAGENT_DATA__DB_URL
              valueFrom:
                secretKeyRef:
                  name: polkagent-secrets
                  key: database-url
            - name: POLKAGENT_SECRETS__BACKEND
              value: "kubernetes"
            - name: POLKAGENT_EXECUTION__WORKER_COUNT
              value: "{{ .Values.workers.workerCount }}"
            - name: RUST_LOG
              value: "info,polkagent=debug"
          livenessProbe:
            httpGet:
              path: /health/live
              port: health
            initialDelaySeconds: 15
            periodSeconds: 30
            timeoutSeconds: 5
            failureThreshold: 3
          readinessProbe:
            httpGet:
              path: /health/ready
              port: health
            initialDelaySeconds: 10
            periodSeconds: 10
            timeoutSeconds: 5
            failureThreshold: 3
          startupProbe:
            httpGet:
              path: /health/startup
              port: health
            initialDelaySeconds: 5
            periodSeconds: 5
            timeoutSeconds: 5
            failureThreshold: 24   # 2 minutes for DB migration
          resources:
            requests:
              cpu: "{{ .Values.workers.resources.requests.cpu }}"
              memory: "{{ .Values.workers.resources.requests.memory }}"
            limits:
              cpu: "{{ .Values.workers.resources.limits.cpu }}"
              memory: "{{ .Values.workers.resources.limits.memory }}"
          volumeMounts:
            - name: data
              mountPath: /data
            - name: config
              mountPath: /data/config.toml
              subPath: config.toml
              readOnly: true
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop: ["ALL"]
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: polkagent-worker-data
        - name: config
          configMap:
            name: polkagent-config
```

#### B.1.2 PostgreSQL StatefulSet

```yaml
# deploy/k8s/postgres-statefulset.yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: polkagent-db
  namespace: polkagent-system
spec:
  serviceName: polkagent-db
  replicas: 1
  selector:
    matchLabels:
      app.kubernetes.io/name: polkagent-db
  template:
    metadata:
      labels:
        app.kubernetes.io/name: polkagent-db
    spec:
      securityContext:
        runAsUser: 999
        fsGroup: 999
      containers:
        - name: postgres
          image: postgres:16-alpine
          env:
            - name: POSTGRES_DB
              value: polkagent
            - name: POSTGRES_USER
              value: polkagent
            - name: POSTGRES_PASSWORD
              valueFrom:
                secretKeyRef:
                  name: polkagent-secrets
                  key: db-password
            - name: PGDATA
              value: /var/lib/postgresql/data/pgdata
          ports:
            - containerPort: 5432
          livenessProbe:
            exec:
              command: ["pg_isready", "-U", "polkagent"]
            periodSeconds: 10
          readinessProbe:
            exec:
              command: ["pg_isready", "-U", "polkagent"]
            initialDelaySeconds: 5
            periodSeconds: 5
          resources:
            requests:
              cpu: "250m"
              memory: "512Mi"
            limits:
              cpu: "2000m"
              memory: "4Gi"
          volumeMounts:
            - name: postgres-data
              mountPath: /var/lib/postgresql/data
  volumeClaimTemplates:
    - metadata:
        name: postgres-data
      spec:
        accessModes: ["ReadWriteOnce"]
        storageClassName: "{{ .Values.database.persistence.storageClass }}"
        resources:
          requests:
            storage: "{{ .Values.database.persistence.size }}"
```

#### B.1.3 Service and Ingress

```yaml
# deploy/k8s/service.yaml
apiVersion: v1
kind: Service
metadata:
  name: polkagent
  namespace: polkagent-workers
spec:
  selector:
    app.kubernetes.io/name: polkagent-worker
  ports:
    - name: api
      port: 9400
      targetPort: api
    - name: health
      port: 9401
      targetPort: health
  type: ClusterIP
---
# deploy/k8s/ingress.yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: polkagent
  namespace: polkagent-workers
  annotations:
    cert-manager.io/cluster-issuer: "letsencrypt-prod"
    nginx.ingress.kubernetes.io/proxy-read-timeout: "3600"
    nginx.ingress.kubernetes.io/proxy-send-timeout: "3600"
    nginx.ingress.kubernetes.io/proxy-body-size: "100m"
    # WebSocket support for agent transport sessions
    nginx.ingress.kubernetes.io/proxy-http-version: "1.1"
    nginx.ingress.kubernetes.io/configuration-snippet: |
      proxy_set_header Upgrade $http_upgrade;
      proxy_set_header Connection "Upgrade";
spec:
  ingressClassName: nginx
  tls:
    - secretName: polkagent-tls
      hosts:
        - "{{ .Values.ingress.hosts[0].host }}"
  rules:
    - host: "{{ .Values.ingress.hosts[0].host }}"
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: polkagent
                port:
                  name: api
```

#### B.1.4 ConfigMap and Secret management

```yaml
# deploy/k8s/configmap.yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: polkagent-config
  namespace: polkagent-workers
data:
  config.toml: |
    [agent]
    mode = "self-hosted"

    [data]
    db = "postgres"

    [data.artifacts]
    backend = "s3"
    bucket  = "{{ .Values.artifacts.bucket }}"
    region  = "{{ .Values.artifacts.region }}"

    [secrets]
    backend = "kubernetes"

    [execution]
    max_concurrent_runs = {{ .Values.workers.maxConcurrentRuns }}
    max_run_duration    = "{{ .Values.workers.maxRunDuration }}"

    [observability]
    log_level       = "info"
    metrics_enabled = true
    metrics_format  = "prometheus"
---
# deploy/k8s/externalsecret.yaml (requires External Secrets Operator)
apiVersion: external-secrets.io/v1beta1
kind: ExternalSecret
metadata:
  name: polkagent-secrets
  namespace: polkagent-workers
spec:
  refreshInterval: 1h
  secretStoreRef:
    name: aws-secretsmanager
    kind: ClusterSecretStore
  target:
    name: polkagent-secrets
    creationPolicy: Owner
  data:
    - secretKey: database-url
      remoteRef:
        key: polkagent/prod/database-url
    - secretKey: anthropic-api-key
      remoteRef:
        key: polkagent/prod/anthropic-api-key
```

#### B.1.5 PersistentVolumeClaim

```yaml
# deploy/k8s/pvc.yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: polkagent-worker-data
  namespace: polkagent-workers
spec:
  accessModes:
    - ReadWriteOnce
  storageClassName: "{{ .Values.artifacts.storageClass }}"
  resources:
    requests:
      storage: "{{ .Values.artifacts.size }}"
```

#### B.1.6 Horizontal Pod Autoscaler

```yaml
# deploy/k8s/hpa.yaml
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: polkagent-worker
  namespace: polkagent-workers
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: polkagent-worker
  minReplicas: {{ .Values.workers.autoscaling.minReplicas }}
  maxReplicas: {{ .Values.workers.autoscaling.maxReplicas }}
  metrics:
    - type: Resource
      resource:
        name: cpu
        target:
          type: Utilization
          averageUtilization: {{ .Values.workers.autoscaling.targetCPUUtilizationPercentage }}
    - type: Resource
      resource:
        name: memory
        target:
          type: Utilization
          averageUtilization: 80
    # Custom metric: job queue depth (requires Prometheus Adapter)
    - type: External
      external:
        metric:
          name: polkagent_job_queue_depth
          selector:
            matchLabels:
              namespace: polkagent-workers
        target:
          type: Value
          value: "10"
```

#### B.1.7 NetworkPolicy

```yaml
# deploy/k8s/networkpolicy.yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: polkagent-worker-isolation
  namespace: polkagent-workers
spec:
  podSelector:
    matchLabels:
      app.kubernetes.io/name: polkagent-worker
  policyTypes:
    - Ingress
    - Egress
  ingress:
    # Allow health checks from the ingress controller namespace.
    - from:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: ingress-nginx
  egress:
    # Allow DNS.
    - ports:
        - port: 53
          protocol: UDP
    # Allow HTTPS to provider APIs and chain RPCs only.
    - ports:
        - port: 443
          protocol: TCP
    # Allow to PostgreSQL in polkagent-system.
    - to:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: polkagent-system
      ports:
        - port: 5432
          protocol: TCP
    # Allow to control plane (if enabled).
    - to:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: polkagent-system
      ports:
        - port: 9402
          protocol: TCP
```

---

### B.2 Helm Chart Structure

The Helm chart is the canonical packaging for Kubernetes deployment of both
self-hosted and managed (operator-deployed) modes.

#### B.2.1 Chart.yaml

```yaml
# deploy/helm/polkagent/Chart.yaml
apiVersion: v2
name: polkagent
description: Polkagent -- Rust-first, Polkadot-native AI agent platform
type: application
version: 0.1.0
appVersion: "0.1.0"
keywords:
  - polkadot
  - ai-agent
  - rust
sources:
  - https://github.com/nicosiatechnologies/polkagent
maintainers:
  - name: Nicosía Technologies
dependencies:
  - name: postgresql
    version: "~15.5"
    repository: https://charts.bitnami.com/bitnami
    condition: database.embedded.enabled
```

#### B.2.2 values.yaml

```yaml
# deploy/helm/polkagent/values.yaml

# --- Image ---
image:
  repository: ghcr.io/nicosiatechnologies/polkagent
  tag: "latest"
  pullPolicy: IfNotPresent

# --- Workers ---
workers:
  replicas: 2
  workerCount: 4          # in-process worker threads per pod
  maxConcurrentRuns: 8
  maxRunDuration: "30m"
  resources:
    requests:
      cpu: "500m"
      memory: "512Mi"
    limits:
      cpu: "2000m"
      memory: "2Gi"
  autoscaling:
    enabled: true
    minReplicas: 1
    maxReplicas: 10
    targetCPUUtilizationPercentage: 70

# --- Database ---
database:
  embedded:
    enabled: false    # Use embedded Bitnami PostgreSQL chart for dev only
  external:
    url: ""           # Set via --set or secret ref
  persistence:
    size: 10Gi
    storageClass: "standard"

# --- Artifacts ---
artifacts:
  backend: "s3"       # s3 | pvc
  bucket: ""
  region: ""
  size: 50Gi
  storageClass: "standard"

# --- Secrets ---
secrets:
  provider: "kubernetes"   # kubernetes | external-secrets | vault
  existingSecret: "polkagent-secrets"
  externalSecrets:
    enabled: false
    store: ""
    storeKind: ClusterSecretStore

# --- Control plane (optional) ---
controlPlane:
  enabled: false
  replicas: 1
  resources:
    requests:
      cpu: "200m"
      memory: "256Mi"
    limits:
      cpu: "1000m"
      memory: "1Gi"

# --- Ingress ---
ingress:
  enabled: true
  className: "nginx"
  annotations:
    cert-manager.io/cluster-issuer: "letsencrypt-prod"
  hosts:
    - host: polkagent.example.com
      paths:
        - path: /
          pathType: Prefix
  tls:
    - secretName: polkagent-tls
      hosts:
        - polkagent.example.com

# --- Observability ---
observability:
  prometheus:
    enabled: true
    serviceMonitor:
      enabled: false   # requires Prometheus Operator
  grafana:
    enabled: false     # deploy separately if needed

# --- Pod security ---
podSecurityContext:
  runAsNonRoot: true
  runAsUser: 1000
  fsGroup: 1000
  seccompProfile:
    type: RuntimeDefault

# --- Network policy ---
networkPolicy:
  enabled: true
```

#### B.2.3 Template directory layout

```text
deploy/helm/polkagent/
  Chart.yaml
  values.yaml
  templates/
    _helpers.tpl               # Named template definitions
    NOTES.txt                  # Post-install user notes
    # Core workloads
    worker-deployment.yaml
    worker-statefulset.yaml    # Alternative: StatefulSet for sticky storage
    controlplane-deployment.yaml
    postgres-statefulset.yaml  # Used when database.embedded.enabled = false
    # Networking
    service.yaml
    ingress.yaml
    networkpolicy.yaml
    # Configuration
    configmap.yaml
    secret.yaml
    externalsecret.yaml        # Conditional on secrets.externalSecrets.enabled
    # Autoscaling
    hpa.yaml
    pdb.yaml                   # PodDisruptionBudget: maxUnavailable=1
    # RBAC
    serviceaccount.yaml
    role.yaml
    rolebinding.yaml
    clusterrole.yaml           # Read nodes for resource awareness
    clusterrolebinding.yaml
    # Tests
    tests/
      test-connection.yaml
      test-health.yaml
  # Environment overrides
  values/
    dev.yaml
    staging.yaml
    production.yaml
```

#### B.2.4 Environment-specific overrides

```yaml
# deploy/helm/polkagent/values/production.yaml
image:
  tag: "1.2.3"              # Pin to specific release in production

workers:
  replicas: 3
  autoscaling:
    minReplicas: 2
    maxReplicas: 20

database:
  persistence:
    size: 100Gi
    storageClass: "ssd-retain"

networkPolicy:
  enabled: true

observability:
  prometheus:
    serviceMonitor:
      enabled: true
```

Deployment commands:

```bash
# Development cluster (kind/k3s)
helm install polkagent ./deploy/helm/polkagent \
  -f deploy/helm/polkagent/values/dev.yaml \
  --set database.embedded.enabled=true \
  --namespace polkagent-dev --create-namespace

# Production cluster
helm upgrade --install polkagent ./deploy/helm/polkagent \
  -f deploy/helm/polkagent/values/production.yaml \
  --set image.tag=1.2.3 \
  --set secrets.existingSecret=polkagent-prod-secrets \
  --namespace polkagent-system --create-namespace \
  --atomic --timeout 10m
```

---

## APPENDIX C: DOCKER

### C.1 Multi-stage Dockerfile: builder to runtime

The Roko reference codebase (`/Users/will/dev/nunchi/roko/roko/docker/`) uses
two distinct patterns:

- **CLI and daemon binaries** (`roko.Dockerfile`, `gateway.Dockerfile`): slim
  builder stage with system OpenSSL deps; distroless or bookworm-slim runtime.
- **Worker image** (`worker.Dockerfile`): worker runtime adds language runtimes
  (Node.js, Python) required for tool execution in harness processes.

Polkagent follows the same split.

#### C.1.1 Daemon image (production)

```dockerfile
# docker/polkagent.Dockerfile
# Multi-stage build -- production target uses distroless for minimal attack surface.
# Roko reference: docker/roko.Dockerfile (rust:1.91-slim-bookworm / debian:bookworm-slim)

ARG BUILDPLATFORM=linux/amd64
ARG RUST_VERSION=1.82

FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-slim-bookworm AS builder
WORKDIR /src

# System dependencies required by common crates (openssl-sys, sqlx, etc.)
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Cache dependency compilation separately from source.
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release --bin polkagent && \
    cp target/release/polkagent /polkagent

# Production runtime: distroless for minimal attack surface.
# Use debian:bookworm-slim during development when a shell is needed.
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

LABEL org.opencontainers.image.source="https://github.com/nicosiatechnologies/polkagent"
LABEL org.opencontainers.image.description="Polkagent daemon -- Polkadot-native AI agent platform"
LABEL org.opencontainers.image.licenses="MIT OR Apache-2.0"
LABEL org.opencontainers.image.title="polkagent"
LABEL org.opencontainers.image.vendor="Nicosía Technologies"

COPY --from=builder --chown=nonroot:nonroot /polkagent /usr/local/bin/polkagent

# nonroot user (UID 65532) is baked into distroless:nonroot.
USER nonroot
WORKDIR /data

VOLUME ["/data"]

# 9400: main API; 9401: health/metrics (separate port avoids API contention)
EXPOSE 9400/tcp
EXPOSE 9401/tcp

ENTRYPOINT ["/usr/local/bin/polkagent"]
CMD ["start", "--data-dir", "/data"]
```

#### C.1.2 Worker image with harness support

Workers that run coding harnesses or tool-execution sandboxes need additional
runtimes. This mirrors the approach in
`/Users/will/dev/nunchi/roko/roko/docker/worker.Dockerfile`, which installs
Node.js, npm, and Python for Claude Code and OpenAI tool execution.

```dockerfile
# docker/polkagent-worker.Dockerfile
# Worker image: includes harness runtimes (Node.js, Python).
# Roko reference: docker/worker.Dockerfile

ARG BUILDPLATFORM=linux/amd64
ARG RUST_VERSION=1.82

FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-slim-bookworm AS builder
WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release --bin polkagent && \
    cp target/release/polkagent /polkagent

FROM debian:bookworm-slim AS worker-runtime

LABEL org.opencontainers.image.title="polkagent-worker"

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        nodejs \
        npm \
        python3 \
        python3-pip \
        gosu \
    && npm install -g @anthropic-ai/claude-code \
    && pip3 install --no-cache-dir --break-system-packages openai \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /polkagent /usr/local/bin/polkagent

ENV PORT=9400
ENV RUST_LOG=info
ENV POLKAGENT_WORKER_MODE=1

# Worker environment variables set at deploy time:
# POLKAGENT_DATA__DB_URL       -- PostgreSQL URL
# POLKAGENT_SECRETS__BACKEND   -- secret backend selection
# POLKAGENT_CONTROL_PLANE_URL  -- optional control plane endpoint
# ANTHROPIC_API_KEY            -- provider key (prefer secrets backend)

RUN useradd --create-home --shell /bin/bash polkagent \
    && mkdir -p /data \
    && chown -R polkagent:polkagent /data

COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

EXPOSE 9400

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["polkagent", "start", "--data-dir", "/data"]
```

#### C.1.3 Entrypoint script

Mirrors the pattern in
`/Users/will/dev/nunchi/roko/roko/docker/entrypoint.sh`: fixes volume
ownership when running as root (Railway, other PaaS mount volumes as root),
then drops to the unprivileged user via `gosu`.

```bash
#!/usr/bin/env bash
# docker/entrypoint.sh
# Fixes volume ownership when mounted as root (Railway/PaaS pattern).
# Roko reference: docker/entrypoint.sh
set -euo pipefail

DATA_DIR="${POLKAGENT_DATA_DIR:-/data}"

if [ "$(id -u)" = "0" ]; then
    # Running as root: fix ownership so non-root user can write data.
    mkdir -p "${DATA_DIR}"
    chown -R polkagent:polkagent "${DATA_DIR}" 2>/dev/null || true
    exec gosu polkagent "$@"
fi

# Already non-root (local Docker / docker compose): just exec.
exec "$@"
```

### C.2 Docker Compose for local development

The Docker Compose setup mirrors the full Roko stack pattern
(`/Users/will/dev/nunchi/roko/roko/docker/docker-compose.yml`): named
network, named volumes, Prometheus + Grafana observability, and a demo
profile.

```yaml
# docker/docker-compose.yml
# Full local development stack.
# Roko reference: docker/docker-compose.yml (roko/docker/)
name: polkagent

networks:
  polkagent-net:
    driver: bridge

volumes:
  polkagent-data:
  postgres-data:
  prometheus-data:
  grafana-data:

services:
  # --- Primary daemon ---
  polkagent:
    build:
      context: ..
      dockerfile: docker/polkagent.Dockerfile
      target: runtime      # Use `worker-runtime` for harness support
    image: ghcr.io/nicosiatechnologies/polkagent:dev
    container_name: polkagent
    networks:
      - polkagent-net
    depends_on:
      postgres:
        condition: service_healthy
    environment:
      RUST_LOG: "info,polkagent=debug"
      POLKAGENT_DATA__DB: "postgres"
      POLKAGENT_DATA__DB_URL: "postgresql://polkagent:polkagent@postgres:5432/polkagent"
      POLKAGENT_SECRETS__BACKEND: "env"
    env_file:
      - .env                    # Provider API keys -- never committed
    ports:
      - "127.0.0.1:9400:9400"  # API (localhost only)
      - "127.0.0.1:9401:9401"  # Health/metrics
    volumes:
      - polkagent-data:/data
      - ./config.toml:/data/config.toml:ro
    healthcheck:
      test: ["CMD", "/usr/local/bin/polkagent", "health"]
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 30s
    security_opt:
      - no-new-privileges:true
    restart: unless-stopped

  # --- PostgreSQL state DB ---
  postgres:
    image: postgres:16-alpine
    container_name: polkagent-db
    networks:
      - polkagent-net
    environment:
      POSTGRES_DB: polkagent
      POSTGRES_USER: polkagent
      POSTGRES_PASSWORD: polkagent    # Dev only; use secrets in production
    volumes:
      - postgres-data:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U polkagent"]
      interval: 10s
      timeout: 5s
      retries: 6
      start_period: 30s
    restart: unless-stopped

  # --- Prometheus metrics collection ---
  # Roko reference: docker-compose.yml prometheus service
  prometheus:
    image: prom/prometheus:v2.54.1
    container_name: polkagent-prometheus
    networks:
      - polkagent-net
    ports:
      - "9090:9090"
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml:ro
      - prometheus-data:/prometheus
    command:
      - "--config.file=/etc/prometheus/prometheus.yml"
      - "--storage.tsdb.path=/prometheus"
    restart: unless-stopped

  # --- Grafana dashboards ---
  # Roko reference: docker-compose.yml grafana service
  grafana:
    image: grafana/grafana:11.4.0
    container_name: polkagent-grafana
    networks:
      - polkagent-net
    ports:
      - "3000:3000"
    environment:
      GF_SECURITY_ADMIN_USER: "${GF_SECURITY_ADMIN_USER:-admin}"
      GF_SECURITY_ADMIN_PASSWORD: "${GF_SECURITY_ADMIN_PASSWORD:-admin}"
      GF_USERS_ALLOW_SIGN_UP: "false"
    volumes:
      - grafana-data:/var/lib/grafana
    depends_on:
      - prometheus
    restart: unless-stopped

  # ---- Integration test profile ------------------------------------------
  # Run with: SCENARIO=basic docker compose --profile test up --build \
  #             --exit-code-from polkagent-test
  polkagent-test:
    profiles: ["test"]
    build:
      context: ..
      dockerfile: docker/test.Dockerfile
    container_name: polkagent-test
    depends_on:
      polkagent:
        condition: service_healthy
    networks:
      - polkagent-net
    environment:
      POLKAGENT_API_URL: "http://polkagent:9400"
      SCENARIO: "${SCENARIO:-basic}"
    command: ["test", "${SCENARIO:-basic}"]
```

#### C.2.1 Prometheus scrape configuration

```yaml
# docker/prometheus.yml
# Roko reference: docker/prometheus.yml
global:
  scrape_interval: 15s
  evaluation_interval: 15s
  external_labels:
    cluster: "polkagent-local"
    environment: "dev"

scrape_configs:
  - job_name: "prometheus"
    static_configs:
      - targets: ["localhost:9090"]

  - job_name: "polkagent"
    metrics_path: /metrics
    static_configs:
      - targets: ["polkagent:9401"]
        labels:
          service: "polkagent-daemon"

  - job_name: "postgres"
    static_configs:
      - targets: ["postgres-exporter:9187"]
        labels:
          service: "polkagent-db"
```

### C.3 Container health checks

Health check endpoints and their semantics are defined in Appendix A.2.4.
The key principle: the health port (9401) is separate from the API port (9400)
so Kubernetes probes never contend with application traffic.

| Probe | Path | Frequency | Failure action |
|---|---|---|---|
| Startup | `/health/startup` | Every 5s, max 24 failures (2min) | Kill and restart |
| Liveness | `/health/live` | Every 30s, 3 failures | Kill and restart |
| Readiness | `/health/ready` | Every 10s, 3 failures | Remove from load balancer |

### C.4 Volume mounting strategy

| Volume | Mount path | Purpose | Persistence |
|---|---|---|---|
| `polkagent-data` | `/data` | SQLite DB, encrypted secrets, artifact cache, config | Required (PVC/named volume) |
| Config | `/data/config.toml` | Read-only mounted ConfigMap | ConfigMap |
| Secrets | `/run/secrets/` | Read-only mounted Secret files (alternative to env vars) | Secret |
| Workspace | `/workspace` | Harness working directory (worker image only) | Ephemeral or PVC |
| Temp | `/tmp` | Temporary processing space | `tmpfs` (no disk persistence) |

Railway-style volume mounting (Roko reference: `entrypoint.sh`):
when the platform mounts volumes as root, the entrypoint script fixes
ownership and drops to the unprivileged user before exec-ing the daemon.

---

## APPENDIX D: HIGH AVAILABILITY

### D.1 Database replication (PostgreSQL streaming replication)

For self-hosted clusters requiring HA, PostgreSQL streaming replication with
one primary and one or more standbys is the baseline.

```yaml
# deploy/k8s/postgres-ha.yaml (using Zalando postgres-operator or CNPG)
# Alternative: use CloudNativePG operator for Kubernetes-native HA PostgreSQL.
apiVersion: postgresql.cnpg.io/v1
kind: Cluster
metadata:
  name: polkagent-db-ha
  namespace: polkagent-system
spec:
  instances: 3                # 1 primary + 2 standbys
  primaryUpdateStrategy: unsupervised

  postgresql:
    parameters:
      max_connections: "200"
      shared_buffers: "512MB"
      wal_level: "replica"
      max_wal_senders: "10"
      max_replication_slots: "10"

  bootstrap:
    initdb:
      database: polkagent
      owner: polkagent
      secret:
        name: polkagent-db-credentials

  storage:
    size: 100Gi
    storageClass: ssd-retain

  monitoring:
    enablePodMonitor: true     # Requires Prometheus Operator

  backup:
    retentionPolicy: "30d"
    barmanObjectStore:
      destinationPath: "s3://polkagent-backups/postgres"
      s3Credentials:
        accessKeyId:
          name: polkagent-s3-creds
          key: ACCESS_KEY_ID
        secretAccessKey:
          name: polkagent-s3-creds
          key: SECRET_ACCESS_KEY
```

Polkagent data plane connection string for HA PostgreSQL with automatic
failover via PgBouncer or the CNPG RW service:

```toml
[data]
db = "postgres"
# RW service routes to primary; RO service load-balances standbys.
db_url_ref = "secrets:database_url"
# database_url = "postgresql://polkagent:${PW}@polkagent-db-ha-rw:5432/polkagent"
db_pool_max_size = 20
db_pool_min_idle = 2
db_connect_timeout_secs = 10
db_idle_timeout_secs = 300
```

### D.2 Stateless worker scaling

Workers are stateless with respect to agent state: all durable state lives in
the PostgreSQL data plane. A worker pod can be killed and replaced at any time;
in-flight jobs return to the queue after the lease expires (90 seconds by
default).

Design decisions that enable stateless workers:

- Artifact handles are content-addressed URIs pointing to the S3 store,
  not local file paths.
- Secret references are resolved via the configured backend at job start,
  not cached in worker memory across jobs.
- No sticky sessions for REST API calls; WebSocket sessions use session
  affinity (see D.3).

### D.3 Session affinity for WebSocket connections

Agent transport sessions and TUI WebSocket connections must be routed to the
same worker pod for the duration of the session. Nginx ingress annotation:

```yaml
# In ingress.yaml annotations:
nginx.ingress.kubernetes.io/affinity: "cookie"
nginx.ingress.kubernetes.io/session-cookie-name: "polkagent-session"
nginx.ingress.kubernetes.io/session-cookie-expires: "172800"   # 48h
nginx.ingress.kubernetes.io/session-cookie-max-age: "172800"
nginx.ingress.kubernetes.io/session-cookie-change-on-failure: "true"
```

For cases where cookie affinity is unavailable (e.g., non-browser clients),
the API gateway layer routes by agent ID header:
`X-Polkagent-Agent-Id: <agent-id>`.

### D.4 Disaster recovery procedures

#### D.4.1 RTO and RPO targets

| Deployment mode | RTO | RPO |
|---|---|---|
| Local personal | Operator-defined (typically hours) | Last manual export |
| Self-hosted (SQLite) | Minutes (restart daemon) | Last WAL checkpoint (~seconds) |
| Self-hosted (PostgreSQL HA) | 30-60 seconds (automatic failover) | Near-zero (streaming replication) |
| Managed cloud | < 4 hours (SEV-1 target) | < 5 minutes (continuous backup) |

#### D.4.2 DR runbook (managed cloud)

```text
DISASTER RECOVERY RUNBOOK -- Managed Cloud

1. DECLARE DR
   - On-call declares DR incident (SEV-1).
   - Notify all affected tenants via status page.

2. ASSESS FAILURE DOMAIN
   - Database failure: failover via CNPG automatic primary promotion.
   - Entire region failure: activate standby region (requires Phase 4 multi-region).
   - Object store failure: redirect artifact writes to DR bucket; queue reads.

3. RESTORE DATABASE (if primary lost with no automatic failover)
   a. Identify latest consistent backup in S3 backup store.
   b. Launch new PostgreSQL instance from latest base backup.
   c. Apply WAL segments to bring to latest point-in-time.
   d. Update connection string secret in Kubernetes.
   e. Restart worker pods to pick up new connection string.

4. RESTORE ARTIFACT STORE (if S3 bucket unavailable)
   a. Redirect new artifact writes to DR bucket (config change).
   b. Use S3 cross-region replication or manual sync to restore access.

5. VERIFY
   - Run synthetic agent probe across all tenant tiers.
   - Verify health endpoints across all worker pods.
   - Confirm metering queue is draining normally.

6. COMMUNICATE
   - Post resolution to status page.
   - Send post-incident report to affected tenants within 24h.
   - Conduct blameless RCA within 5 business days.
```

### D.5 Backup and restore

#### D.5.1 Backup strategy

| What | How | Frequency | Retention |
|---|---|---|---|
| PostgreSQL state DB | Continuous WAL archiving to S3 via CNPG barman | Continuous | 30 days |
| SQLite state DB (local) | `polkagent export` to encrypted bundle | Manual or scheduled | Operator-defined |
| Artifact store | S3 versioning + cross-region replication | Continuous (S3 native) | Per bucket lifecycle policy |
| Secrets | KMS: provider-managed key backup | Per KMS provider policy | Provider-defined |
| Config revisions | Control plane DB backup | Continuous | 30 days |

#### D.5.2 Restore procedures

```bash
# Restore SQLite from export bundle (local deployment)
polkagent stop
polkagent import --bundle polkagent-export-20260730T120000Z.tar.gz \
  --mode restore \
  --data-dir /var/lib/polkagent
polkagent start

# Point-in-time restore for PostgreSQL (CNPG)
kubectl apply -f - <<EOF
apiVersion: postgresql.cnpg.io/v1
kind: Cluster
metadata:
  name: polkagent-db-restore
  namespace: polkagent-system
spec:
  instances: 1
  bootstrap:
    recovery:
      source: polkagent-db-ha
      recoveryTarget:
        targetTime: "2026-07-30 08:00:00"
  externalClusters:
    - name: polkagent-db-ha
      barmanObjectStore:
        destinationPath: "s3://polkagent-backups/postgres"
        ...
EOF
```

---

## APPENDIX E: IMPLEMENTATION CHECKLIST

Tasks are ordered by dependency. Each group must be complete before the next
group that depends on it begins. Acceptance criteria reference section 15.

### E.1 Data Plane

- [ ] **DP-01** Define `StateStore`, `ArtifactStore`, `SecretBackend` port traits
  - Criterion: trait objects compile as `Arc<dyn T>` with `Send + Sync + 'static`
- [ ] **DP-02** Implement `SqliteStateStore` with WAL mode and migration runner
  - Criterion: SH-04 passes (state survives kill -9)
- [ ] **DP-03** Write initial SQLite migration set (runs, effects, conversations, policy cache)
  - Criterion: all migrations have up/down; reversibility tested
- [ ] **DP-04** Implement `FilesystemArtifactStore`
  - Criterion: content-addressed put/get/list/delete round-trip passes
- [ ] **DP-05** Implement `EncryptedFileBackend` secret backend (age/AEAD)
  - Criterion: SM-01 passes (no plaintext on disk); SM-02 passes (cleared on shutdown)
- [ ] **DP-06** Implement `EnvBackend` (read-once-and-clear environment variable secrets)
  - Criterion: environment not readable from /proc/self/environ after startup
- [ ] **DP-07** Implement `OsKeychainBackend` (macOS Security framework, Linux libsecret)
  - Criterion: SM-06 passes on both platforms
- [ ] **DP-08** Implement `PolicyCache` (SQLite-backed, signature verification)
  - Criterion: J3-02 passes (cached policy used when control plane unavailable)
- [ ] **DP-09** Implement `PostgresStateStore` with row-level security
  - Criterion: MC-02 passes (cross-tenant isolation through all API paths)
- [ ] **DP-10** Write PostgreSQL migration set (mirrors SQLite schema + RLS policies)
  - Criterion: SP-07 passes (SQLite to PostgreSQL migration succeeds)
- [ ] **DP-11** Implement `S3ArtifactStore`
  - Criterion: artifacts survive worker pod restart; MC-07 passes for artifact locality
- [ ] **DP-12** Implement export bundle writer (all tables + artifacts + manifest)
  - Criterion: SP-01 passes (valid, complete bundle with checksums)
- [ ] **DP-13** Implement import bundle reader with rollback support
  - Criterion: SP-02, SP-03, SP-04 pass
- [ ] **DP-14** Implement GDPR export and delete CLI commands
  - Criterion: `polkagent gdpr export` and `polkagent gdpr delete` complete within 72h deadline

### E.2 Execution Plane

- [ ] **EP-01** Define `Worker` trait and `WorkerCapabilities` struct
  - Criterion: in-process Tokio worker implements trait; local mode starts
- [ ] **EP-02** Implement `WorkerRegistry` (dashmap-based, thread-safe)
  - Criterion: WK-01 passes (workers register and receive jobs within 30s)
- [ ] **EP-03** Implement in-process Tokio worker (local mode)
  - Criterion: DEPLOY-001 passes (single-binary local installation)
- [ ] **EP-04** Implement `PriorityQueues` with FIFO and `FairQueue` (tenant round-robin)
  - Criterion: WK-05 passes (CRITICAL jobs scheduled first under load)
- [ ] **EP-05** Implement `Scheduler` dispatch loop with least-loaded worker selection
  - Criterion: WK-02 passes (failed worker jobs reassigned after lease expiry)
- [ ] **EP-06** Implement lease-based job claiming with lease renewal
  - Criterion: lease expiry causes reassignment; no duplicate execution
- [ ] **EP-07** Implement `QuotaEnforcer` (concurrent runs, memory, artifact storage)
  - Criterion: MC-05 passes (quota enforcement prevents resource exhaustion)
- [ ] **EP-08** Implement heartbeat monitor (3 missed = mark unhealthy, reassign jobs)
  - Criterion: WK-02 passes
- [ ] **EP-09** Implement graceful shutdown / drain sequence with configurable timeout
  - Criterion: WK-04 passes (drain completes before shutdown)
- [ ] **EP-10** Expose health endpoint (`/health/live`, `/health/ready`, `/health/startup`)
  - Criterion: CC-04 passes (correct status under all conditions)
- [ ] **EP-11** Implement auto-scaling event triggers (queue depth, CPU, wait time)
  - Criterion: WK-03 passes (workers added when queue depth exceeds threshold)

### E.3 Control Plane

- [ ] **CP-01** Implement mTLS enrollment endpoint (agent public key → enrollment cert)
  - Criterion: DEPLOY-018 passes (enrollment works); J3-06 passes (stale policy warning)
- [ ] **CP-02** Implement signed configuration revision distribution
  - Criterion: DEPLOY-019 passes; data plane rejects unsigned revisions
- [ ] **CP-03** Implement pull endpoint (data plane polls for new revisions)
  - Criterion: NFR-09 passes (config change propagated within 5 minutes p95)
- [ ] **CP-04** Implement tenant management API (CRUD + suspend + offboard)
  - Criterion: MC-01 passes (tenant creation < 60s); MC-08 passes (data purged on offboard)
- [ ] **CP-05** Implement fleet management (enrollment store, drain, rollout commands)
  - Criterion: fleet dashboard shows all enrolled data planes with current revision
- [ ] **CP-06** Implement `PolicyDistributor` (sign, version, publish)
  - Criterion: signature verified by data plane before application
- [ ] **CP-07** Implement usage metering (worker → queue → aggregator → billing DB)
  - Criterion: BL-01 passes (usage accurate within 1%)
- [ ] **CP-08** Implement quota management and alert notifications (80% / 90% / 100%)
  - Criterion: BL-02, BL-03 pass
- [ ] **CP-09** Implement billing period aggregation and invoice generation
  - Criterion: BL-04 passes (invoices generated on first of month)
- [ ] **CP-10** Implement admin dashboard API (fleet, billing, health summary)
  - Criterion: BL-05 passes (dashboard shows real-time data within 2 minutes)
- [ ] **CP-11** Implement offline degradation: staleness tracking, policy expiry levels
  - Criterion: J3-01 through J3-06 all pass
- [ ] **CP-12** Implement reconnection protocol with usage sync and missed-command replay
  - Criterion: J3-04, J3-05 pass

### E.4 Docker

- [ ] **DK-01** Write `docker/polkagent.Dockerfile` (distroless runtime stage)
  - Criterion: SH-05 passes (non-root user); NFR-06 passes (image < 100 MB)
- [ ] **DK-02** Write `docker/polkagent-worker.Dockerfile` (harness runtime stage)
  - Criterion: worker image runs as non-root; harness tools available
- [ ] **DK-03** Write `docker/entrypoint.sh` (volume ownership fix + gosu drop)
  - Criterion: container starts as non-root even when volume mounted as root
- [ ] **DK-04** Write `docker/docker-compose.yml` (daemon + PostgreSQL + Prometheus + Grafana)
  - Criterion: SH-06 passes (`docker compose up` starts complete stack)
- [ ] **DK-05** Write `docker/docker-compose.harness.yml` (adds harness sidecar)
  - Criterion: harness container runs in isolated internal network
- [ ] **DK-06** Write `docker/prometheus.yml` scrape configuration
  - Criterion: Prometheus scrapes daemon health and metrics endpoints
- [ ] **DK-07** Add HEALTHCHECK instruction to all Dockerfiles
  - Criterion: `docker inspect` shows health status
- [ ] **DK-08** Publish multi-arch images (linux/amd64, linux/arm64) via GitHub Actions
  - Criterion: image pulls and runs on both architectures

### E.5 Kubernetes

- [ ] **K8-01** Write `deploy/k8s/worker-deployment.yaml` with security context
  - Criterion: SH-08 passes (Helm deploys to kind/k3s in CI)
- [ ] **K8-02** Write `deploy/k8s/postgres-statefulset.yaml`
  - Criterion: PostgreSQL starts, accepts connections, is health-checked
- [ ] **K8-03** Write `deploy/k8s/service.yaml` and `ingress.yaml`
  - Criterion: API reachable via ingress; TLS terminates correctly
- [ ] **K8-04** Write `deploy/k8s/networkpolicy.yaml` (worker egress isolation)
  - Criterion: worker pods cannot reach other namespaces except via allowed ports
- [ ] **K8-05** Write `deploy/k8s/hpa.yaml` (CPU + memory + custom queue-depth metric)
  - Criterion: WK-03 passes (auto-scaling responds to load)
- [ ] **K8-06** Write `deploy/k8s/externalsecret.yaml` (External Secrets Operator)
  - Criterion: secrets sync from AWS Secrets Manager / Vault to Kubernetes Secret
- [ ] **K8-07** Write Helm chart (`deploy/helm/polkagent/`) with Chart.yaml, values.yaml
  - Criterion: `helm install` and `helm upgrade` work with no manual steps
- [ ] **K8-08** Write `values/dev.yaml`, `values/staging.yaml`, `values/production.yaml`
  - Criterion: CC-01 passes (same AgentSpec runs identically across modes)
- [ ] **K8-09** Write Helm test manifests (`templates/tests/`)
  - Criterion: `helm test` passes after deployment
- [ ] **K8-10** Write `deploy/k8s/postgres-ha.yaml` (CNPG cluster with WAL archiving)
  - Criterion: D.4.2 DR runbook can be executed successfully

### E.6 High Availability and Disaster Recovery

- [ ] **HA-01** Deploy CNPG HA PostgreSQL cluster (3 instances) in staging
  - Criterion: automatic primary failover completes within 60 seconds
- [ ] **HA-02** Configure S3 WAL archiving and point-in-time restore
  - Criterion: point-in-time restore drill succeeds within RTO target
- [ ] **HA-03** Implement and test graceful worker pod rolling update
  - Criterion: zero in-flight job loss during `kubectl rollout restart`
- [ ] **HA-04** Configure WebSocket session affinity in nginx ingress
  - Criterion: transport sessions survive pod restarts (route to new pod within 5s)
- [ ] **HA-05** Implement and document DR runbook (D.4.2)
  - Criterion: DR drill completes within 4-hour RTO for SEV-1 scenarios
- [ ] **HA-06** Configure S3 cross-region replication for artifact store
  - Criterion: artifacts accessible from DR region after primary region failure
- [ ] **HA-07** Write and test restore procedures (SQLite export, PostgreSQL CNPG)
  - Criterion: `polkagent import` from export bundle restores all state

---

## APPENDIX F: REFERENCE FILE MAP

The table below maps each Polkagent component to the reference files in Roko
and Bardo that were used to derive the implementation patterns in this document.

| Polkagent Component | Roko Reference File | Bardo Reference File | Key Patterns |
|---|---|---|---|
| Daemon Dockerfile (distroless) | `docker/roko.Dockerfile` | -- | Multi-stage builder (rust:1.91-slim-bookworm); distroless/cc-debian12:nonroot runtime; cargo build --release; LABEL OCI annotations |
| Worker Dockerfile (harness) | `docker/worker.Dockerfile` | -- | Node.js + npm + Python3 in runtime stage; ENV vars for template and control plane URL; non-root useradd; CMD roko worker |
| Gateway Dockerfile | `docker/gateway.Dockerfile` | -- | distroless:nonroot runtime; COPY --chown=nonroot; USER nonroot; placeholder EXPOSE |
| Entrypoint script | `docker/entrypoint.sh` | -- | Root-to-nonroot drop via gosu; volume ownership fix for Railway/PaaS; RAILWAY_VOLUME_MOUNT_PATH detection |
| Docker Compose stack | `docker/docker-compose.yml` | -- | Named network (bridge); named volumes; service dependencies with condition: service_healthy; Prometheus + Grafana services; demo profile |
| Prometheus scrape config | `docker/prometheus.yml` | -- | Per-service job_name; service-name labels; metrics_path /metrics; 15s interval |
| Fly.io deployment | `deploy/mirage/fly.toml`, `deploy/roko-agent/fly.toml` | -- | auto_stop/start_machines; min_machines_running; HTTP health check path; mounts with source/destination |
| Railway deployment | `deploy/roko-agent/railway.json` | -- | DOCKERFILE builder; healthcheckPath; healthcheckTimeout; ON_FAILURE restart policy |
| TUI System screen | -- | `apps/bardo-terminal/src/screens/command/system.rs` | SystemScreenState struct (cpu_pct, mem_pct, uptime_secs, deployment_type, workspace_version, binary_hash, rust_version); ratatui Paragraph with Span::styled color thresholds; Screen trait (id, title, render, handle_key) |
| Deployment type enum | -- | `apps/bardo-terminal/src/screens/command/system.rs` | deployment_type: "Managed" / "SelfDeploy" / "BareMetal" |
| Multi-stage Dockerfile pattern | `docker/roko.Dockerfile` + `docker/mirage.Dockerfile` | -- | ARG BUILDPLATFORM; --platform=$BUILDPLATFORM builder; separate system deps RUN layer before COPY for cache efficiency |
| Volume persistence | `docker/mirage.Dockerfile` (VOLUME /workspace/.roko) | -- | Persist daemon state directory across deploys; separate subdirs for state/learn/neuro/dreams |
| Observability port separation | `docker/prometheus.yml` (roko:9092 vs mirage:9091) | -- | Distinct ports per service type; metrics endpoint separate from API port |

---

## APPENDIX G: TUI SURFACE FOR DEPLOYMENT

The Polkagent TUI is built with `ratatui` following the same pattern as Bardo's
terminal (`/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/command/system.rs`).
The `Screen` trait provides `id()`, `title()`, `render()`, and `handle_key()`.
Each deployment screen is a separate struct implementing `Screen`.

### G.1 System status panel

Directly modeled on Bardo's `SystemScreen`, extended with Polkagent-specific
fields. Bardo's `SystemScreenState` exposes: `cpu_pct`, `mem_pct`,
`workspace_version`, `binary_hash`, `deployment_type`, `uptime_secs`,
`golem_id`, `build_timestamp`, `rust_version`. Polkagent adds daemon status,
worker count, and active run count.

```rust
// crates/polkagent-tui/src/screens/system.rs

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

/// Deployment metadata and runtime diagnostics panel.
/// Mirrors Bardo's SystemScreenState pattern.
/// Bardo ref: apps/bardo-terminal/src/screens/command/system.rs
#[derive(Debug, Clone)]
pub struct SystemScreenState {
    pub tick: u64,
    pub cpu_pct: f32,
    pub mem_pct: f32,
    /// "local" | "self-hosted" | "managed" -- maps to Bardo's deployment_type
    pub deployment_mode: String,
    pub workspace_version: String,
    pub binary_hash: String,
    pub uptime_secs: u64,
    pub build_timestamp: String,
    pub rust_version: String,
    /// Polkagent-specific additions below.
    pub daemon_status: String,     // "running" | "degraded" | "stopped"
    pub active_runs: u32,
    pub active_workers: u32,
    pub policy_cache_age_secs: Option<u64>,
    pub control_plane_connected: Option<bool>,
}

pub(crate) struct SystemScreen;

impl Screen for SystemScreen {
    fn id(&self) -> ScreenId { ScreenId::System }
    fn title(&self) -> &str { "System" }

    fn render(&self, frame: &mut Frame<'_>, area: Rect, state: &AppState) {
        let s = SystemScreenState::from_app_state(state);

        let block = Block::default()
            .title(" POLKAGENT / System ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Color thresholds from Bardo's SystemScreen pattern.
        let cpu_color = if s.cpu_pct > 80.0 { Color::Red }
            else if s.cpu_pct > 50.0 { Color::Yellow }
            else { Color::Green };
        let mem_color = if s.mem_pct > 80.0 { Color::Red }
            else if s.mem_pct > 50.0 { Color::Yellow }
            else { Color::Green };
        let status_color = match s.daemon_status.as_str() {
            "running"  => Color::Green,
            "degraded" => Color::Yellow,
            _          => Color::Red,
        };

        let lines = vec![
            Line::from(vec![
                Span::styled("Version:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(&s.workspace_version, Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("Deploy:   ", Style::default().fg(Color::DarkGray)),
                Span::styled(&s.deployment_mode, Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::styled("Status:   ", Style::default().fg(Color::DarkGray)),
                Span::styled(&s.daemon_status, Style::default().fg(status_color)),
            ]),
            Line::from(vec![
                Span::styled("Uptime:   ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}s", s.uptime_secs),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("CPU: ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{:.1}%", s.cpu_pct), Style::default().fg(cpu_color)),
                Span::raw("  "),
                Span::styled("Mem: ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{:.1}%", s.mem_pct), Style::default().fg(mem_color)),
            ]),
            Line::from(vec![
                Span::styled("Workers:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} active", s.active_workers),
                    Style::default().fg(Color::White),
                ),
                Span::raw("  "),
                Span::styled("Runs: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{}", s.active_runs),
                    Style::default().fg(Color::White),
                ),
            ]),
        ];

        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Left), inner);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<AppAction> {
        match key.code {
            KeyCode::Char('q') => Some(AppAction::Quit),
            KeyCode::Tab       => Some(AppAction::NextScreen),
            KeyCode::BackTab   => Some(AppAction::PrevScreen),
            _ => None,
        }
    }
}
```

ASCII wireframe:

```text
+------------------------------------------------------------------+
| POLKAGENT / System                                               |
|                                                                  |
| Version:  0.1.0                                                  |
| Deploy:   self-hosted                                            |
| Status:   running                                                |
| Uptime:   3600s                                                  |
| CPU: 12.4%   Mem: 38.2%                                          |
| Workers:  2 active   Runs: 3                                     |
|                                                                  |
+------------------------------------------------------------------+
```

### G.2 Worker health dashboard

```rust
// crates/polkagent-tui/src/screens/workers.rs

pub struct WorkerHealthScreen {
    scroll: usize,
}

/// Displays a scrollable list of registered workers with health indicators.
```

ASCII wireframe:

```text
+------------------------------------------------------------------+
| POLKAGENT / Workers               [j/k: scroll] [d: drain]      |
|                                                                  |
| ID                  Status      Jobs  CPU    Mem     Errors/hr   |
| ---------------------------------------------------------------- |
| worker-3a1f         HEALTHY       2/4  23.1%  412MB   0          |
| worker-8c2e         HEALTHY       1/4  11.4%  318MB   0          |
| worker-5b9d         DEGRADED      0/4   --    --      12         |
|   reason: high error rate (18% in last 10 min)                  |
|                                                                  |
| Queue: 0 critical  2 high  5 normal  12 low  0 bulk              |
|                                                                  |
+------------------------------------------------------------------+
```

### G.3 Deployment configuration editor

```rust
// crates/polkagent-tui/src/screens/config.rs

pub struct ConfigEditorScreen {
    mode: ConfigMode,    // View | Edit
    current_key: String,
    current_value: String,
    error: Option<String>,
}
```

ASCII wireframe:

```text
+------------------------------------------------------------------+
| POLKAGENT / Config          [e: edit] [s: save] [r: reset]      |
|                                                                  |
| [agent]                                                          |
|   mode            = self-hosted                                  |
|                                                                  |
| [data]                                                           |
|   db              = postgres                                     |
|   db_url          = *** (secrets:database_url)                  |
|   max_artifact_size = 1GB                                        |
|                                                                  |
| [execution]                                                      |
|   max_concurrent_runs = 8                                        |
|   max_run_duration    = 30m                                      |
|   worker_count        = 4                                        |
|                                                                  |
| [secrets]                                                        |
|   backend         = kubernetes                                   |
|                                                                  |
| [observability]                                                  |
|   log_level       = info                                         |
|   metrics_enabled = true                                         |
|                                                                  |
+------------------------------------------------------------------+
```

### G.4 Tenant management panel (cloud mode)

Shown only when `deployment_mode = "managed"`. Maps to the `Role::Operator`
and above.

```rust
// crates/polkagent-tui/src/screens/tenants.rs

pub struct TenantManagementScreen {
    tenants: Vec<TenantSummary>,
    selected: usize,
    filter: TenantFilter,
}
```

ASCII wireframe:

```text
+------------------------------------------------------------------+
| POLKAGENT / Tenants    [n: new] [s: suspend] [del: offboard]    |
|                                                                  |
| ID              Name              Plan       Status    Workers   |
| ---------------------------------------------------------------- |
| acme-corp       Acme Corp         Team       ACTIVE    3 / 5    |
| bobs-bots       Bob's Bots        Free       ACTIVE    1 / 1    |
| widget-co       Widget Co         Enterprise SUSPENDED 0 / 20   |
|                                                                  |
| Selected: acme-corp                                              |
|   Orgs: 2    Projects: 4    Agents: 7    Runs today: 142         |
|   Storage: 23.4 GB / 50 GB                                       |
|   Policy revision: r-3f8a (applied 2026-07-30T09:14:22Z)        |
|                                                                  |
+------------------------------------------------------------------+
```

---

## APPENDIX H: CONFIGURATION GUIDE

### H.1 Deployment mode selection

Set `[agent] mode` in `config.toml` or via `POLKAGENT_AGENT__MODE` environment
variable. This is the only field that changes the daemon's high-level behavior.

| Mode value | Description | Data plane | Execution plane | Control plane |
|---|---|---|---|---|
| `local-private` | Single-user, no sharing | SQLite, local FS | In-process Tokio | None |
| `local-builder` | Developer with team access | SQLite, local FS | In-process Tokio | None |
| `self-hosted` | Multi-user server deployment | SQLite or PostgreSQL | Daemon workers | Optional |
| `managed` | Polkagent Cloud tenant | PostgreSQL (platform) | Managed workers | Required |
| `hybrid` | Local workers + managed control plane | SQLite or PostgreSQL | Daemon workers | Required (pull only) |

### H.2 Database connection configuration

```toml
# SQLite (default for local modes)
[data]
db  = "sqlite"
dir = "~/.polkagent"    # state.db created at {dir}/state.db

# PostgreSQL (self-hosted cluster / managed)
[data]
db         = "postgres"
db_url_ref = "secrets:database_url"
# database_url format: postgresql://user:password@host:5432/dbname
# For HA: postgresql://user:password@pgbouncer:5432/polkagent?sslmode=require

# Connection pool tuning (PostgreSQL only)
db_pool_max_size       = 20
db_pool_min_idle       = 2
db_connect_timeout_secs = 10
db_idle_timeout_secs   = 300
db_max_lifetime_secs   = 1800
```

### H.3 Worker configuration

```toml
[execution]
# Number of concurrent agent runs across all workers on this daemon.
max_concurrent_runs = 4

# Maximum wall-clock time for a single run.
max_run_duration    = "30m"

# Timeout for harness process startup.
harness_timeout     = "10m"

# Number of in-process worker threads (local mode).
# In Kubernetes: set per-pod, total capacity = replicas * worker_count.
worker_count        = 4

# Job lease duration. Expired leases trigger reassignment.
job_lease_secs      = 90

# Heartbeat interval. 3 missed = mark unhealthy.
heartbeat_interval_secs = 15

# Graceful drain timeout on SIGTERM.
drain_timeout_secs  = 120
```

### H.4 TLS / mTLS setup

```toml
# Self-hosted with TLS termination at reverse proxy (recommended):
[api]
enabled = true
bind    = "127.0.0.1:9400"    # Reverse proxy handles TLS
auth    = "bearer-token"
auth_token_ref = "secrets:api_token"

# Self-hosted with direct TLS:
[api]
enabled  = true
bind     = "0.0.0.0:9400"
tls      = true
cert_ref = "secrets:tls_cert"   # PEM certificate chain
key_ref  = "secrets:tls_key"    # PEM private key

# Mutual TLS for Kubernetes intra-cluster:
[api]
enabled   = true
bind      = "0.0.0.0:9400"
auth      = "mtls"
ca_ref    = "secrets:mtls_ca_cert"
cert_ref  = "secrets:mtls_server_cert"
key_ref   = "secrets:mtls_server_key"

# mTLS enrollment (hybrid mode: data plane -> control plane):
[control_plane]
enabled         = true
endpoint        = "https://cloud.polkagent.dev"
tenant_id       = "acme-corp"
enrollment_key_ref = "secrets:enrollment_key"
# After initial enrollment, daemon stores the enrollment cert locally.
# All subsequent pulls use the cert; the enrollment_key is not used again.
mtls_ca_ref     = "secrets:control_plane_ca"   # Optional: pin CA
pull_interval   = "5m"
health_report_interval = "1m"
```

### H.5 Environment variable reference

All configuration keys are addressable via environment variables using the
pattern `POLKAGENT_<SECTION>__<KEY>` (double underscore for nesting).
Boolean values: `true` / `false`. Duration values: `"30s"`, `"5m"`, `"2h"`.

| Environment variable | Config key | Default | Description |
|---|---|---|---|
| `POLKAGENT_AGENT__MODE` | `[agent] mode` | `local-private` | Deployment mode |
| `POLKAGENT_AGENT__NAME` | `[agent] name` | (required) | Agent display name |
| `POLKAGENT_DATA__DIR` | `[data] dir` | `~/.polkagent` | Data directory |
| `POLKAGENT_DATA__DB` | `[data] db` | `sqlite` | Database backend |
| `POLKAGENT_DATA__DB_URL` | `[data] db_url` | (none) | PostgreSQL connection URL |
| `POLKAGENT_SECRETS__BACKEND` | `[secrets] backend` | `encrypted-file` | Secret backend |
| `POLKAGENT_SECRETS__PATH` | `[secrets] path` | `{data_dir}/secrets.enc` | Encrypted file path |
| `POLKAGENT_EXECUTION__MAX_CONCURRENT_RUNS` | `[execution] max_concurrent_runs` | `4` | Max concurrent runs |
| `POLKAGENT_EXECUTION__MAX_RUN_DURATION` | `[execution] max_run_duration` | `"30m"` | Max run wall-clock time |
| `POLKAGENT_EXECUTION__WORKER_COUNT` | `[execution] worker_count` | `4` | In-process worker threads |
| `POLKAGENT_OBSERVABILITY__LOG_LEVEL` | `[observability] log_level` | `info` | Log level (trace/debug/info/warn/error) |
| `POLKAGENT_OBSERVABILITY__METRICS_ENABLED` | `[observability] metrics_enabled` | `false` | Enable Prometheus metrics |
| `POLKAGENT_API__BIND` | `[api] bind` | `127.0.0.1:9400` | API listen address |
| `POLKAGENT_API__HEALTH_BIND` | `[api] health_bind` | `0.0.0.0:9401` | Health/metrics listen address |
| `POLKAGENT_CONTROL_PLANE__ENABLED` | `[control_plane] enabled` | `false` | Enable control plane connection |
| `POLKAGENT_CONTROL_PLANE__ENDPOINT` | `[control_plane] endpoint` | (none) | Control plane URL |
| `POLKAGENT_CONTROL_PLANE__PULL_INTERVAL` | `[control_plane] pull_interval` | `"5m"` | Config pull interval |
| `POLKAGENT_POLICY__DEFAULT_GRANT` | `[policy] default_grant` | `"read-only"` | Default grant level |
| `POLKAGENT_POLICY__CACHE_MAX_AGE` | `[policy] cache_max_age` | `"24h"` | Max age before policy is stale |
| `RUST_LOG` | (Rust env filter) | `info` | Fine-grained log filter (e.g. `info,polkagent=debug`) |

Secret values for provider API keys follow the convention
`POLKAGENT_SECRET_<NAME>` when using the `env` backend:

```bash
# Required when secrets.backend = "env"
POLKAGENT_SECRET_ANTHROPIC_API_KEY=sk-ant-...
POLKAGENT_SECRET_RPC_TOKEN=...
POLKAGENT_SECRET_DATABASE_URL=postgresql://...
```

These values are read exactly once at daemon startup and cleared from the
process environment (via `std::env::remove_var`) before the daemon accepts
any connections.
