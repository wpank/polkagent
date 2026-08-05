# PRD-07 — Identity, Accounts, Signers, Policy and Security

> **Implementation note (audited 2026-08-05):** This PRD remains normative,
> but its embedded implementation statements and checklists are not current
> status evidence. Use [STATUS.md](STATUS.md) and
> [IMPLEMENTATION-BACKLOG.md](IMPLEMENTATION-BACKLOG.md) for verified state and
> the dependency-ordered execution queue.

**Status:** definitive PRD
**Owner:** unassigned
**Date:** 2026-07-30
**Audience:** engineers, product designers, security reviewers, and operators
with no prior Polkagent, Roko, or `polkadot-chat-agents` context

## 1. Purpose and orientation

This document specifies how Polkagent identifies people, agents, organizations,
and services; how it manages Polkadot accounts and signing keys; how policy and
capability grants control what agents may do; and how the security model
protects users, funds, and data across local, self-hosted, and managed-cloud
deployments.

Identity, custody, and policy are the most security-sensitive areas of the
platform. A mistake here can lose funds, leak private keys, allow unauthorized
actions, or enable social-engineering attacks. The design therefore follows
three guiding principles:

1. **Keys never reach models.** No language model, prompt, harness, tool,
   extension, marketplace package, or log may receive raw signing key material.
   Signing happens through an isolated port that receives only canonical,
   policy-approved payloads.

2. **Policy is resolved outside model control.** An LLM may propose an action.
   Deterministic, auditable policy code decides whether it is permitted. A
   model cannot widen its own grants through persuasive text.

3. **Identity is evidence, not authority.** A People Chain registration, a
   display name, a reputation score, or an agent card provides contextual
   information. None of these automatically grants capabilities, funds access,
   or trust.

### 1.1 Relationship to other PRDs

This PRD is self-contained but references concepts defined elsewhere:

| Concept | Primary PRD | What this PRD uses |
|---|---|---|
| Run, effect, artifact, event | PRD-03 | Effect lifecycle that policy gates authorize or deny |
| Provider, harness, tool, skill | PRD-04 | Extension boundaries that grants constrain |
| Chain client, profile, metadata | PRD-05 | Chain identity and metadata pinning for signer payloads |
| Transport, PCA compatibility | PRD-06 | Transport authentication and message admission |
| Payment, settlement, autonomy tiers | PRD-08 | Economic effects that signer and policy control |
| Memory, groups | PRD-09 | Data classification and tenant boundaries |
| Cloud tenancy, control plane | PRD-11 | Multi-tenant identity and authorization |
| Marketplace, extensions | PRD-12 | Extension capability declarations and sandbox boundaries |

Where this PRD needs a concept from another, the necessary context is
summarized inline. A reader should not need a second PRD to understand this one.

### 1.2 Definitions

| Term | Meaning in this PRD |
|---|---|
| **Principal** | Any entity that can be authenticated and authorized: a person, agent, service, organization, or device. |
| **Identity** | A set of claims, keys, and attributes associated with a principal. |
| **Account** | A Polkadot on-chain account identified by a public key and represented in SS58 format. |
| **Signer** | An isolated component that can authorize a canonical payload. It may be a wallet, hardware device, KMS, proxy, multisig, or programmatic account. |
| **Grant** | A resolved, immutable, time-bounded authorization for a specific operation. |
| **Policy** | Versioned rules that determine what is permitted, denied, or requires approval. |
| **Gate** | An independent, deterministic check that allows, denies, or escalates a proposed action. |
| **Agent card** | A signed, portable descriptor that binds an agent identity to its capabilities, accounts, and operator. |
| **Classification** | A data sensitivity label: `Public`, `Private`, `Sensitive`, or `SecretForbidden`. |
| **Capability** | A declared permission that an extension, tool, or agent requests from its host. |
| **Mandate** | An owner-configured authorization for autonomous operation, with explicit scope, budget, and revocation. |

### 1.3 Evidence and maturity labels

| Label | Meaning |
|---|---|
| **Established** | Owner-confirmed direction; definitive PRDs must preserve it. |
| **Verified fact** | Supported by a primary source as of the access date. |
| **Proposal** | A Polkagent design choice, not existing behavior. |
| **Experimental** | Requires research, spike, and explicit maturity labeling. |
| **Invariant** | A property that must hold regardless of configuration. |

---

## 2. Identity taxonomy

Polkagent recognizes four categories of principal. Each has different
authentication methods, lifecycle, and trust properties.

### 2.1 Agent identity

An agent is a versioned product/runtime definition that combines behavior,
execution routes, tools, skills, context, memory, surfaces, policy references,
and deployment requirements. It is not synonymous with one model.

**Established:** agents may exist at any point on an identity spectrum:

| Mode | Description | Authentication | On-chain presence |
|---|---|---|---|
| **Local-only** | Runs on one machine, no external identity | Process-level OS authentication | None |
| **Pseudonymous** | Has a stable key pair but no verified real-world identity | Cryptographic key | Optional; unverified |
| **Registered** | Bound to a Polkadot account with optional People Chain identity | Account signature + optional identity fields | Account exists; identity optional |
| **Organizational** | Operated under an organization's identity and policy | Organization SSO/delegation + account | Organization-linked account |
| **Public service** | Published agent with discoverable capabilities and pricing | Signed agent card + account + optional identity | Account + optional marketplace listing |

**Invariant:** no identity mode is mandatory. A private local agent needs no
account, no registration, and no People Chain entry to function within its
local grants.

**Agent identity data model:**

```rust
/// Stable identity for an agent across restarts and deployments.
struct AgentIdentity {
    /// Locally unique, stable identifier.
    agent_id: AgentId,
    /// Human-readable label chosen by the operator.
    display_name: String,
    /// Cryptographic key pair for signing agent cards and transport messages.
    /// The private key is held in the signer port, never in agent memory.
    transport_key: PublicKey,
    /// Optional binding to a Polkadot on-chain account.
    chain_account: Option<AccountBinding>,
    /// Optional People Chain identity reference.
    identity_ref: Option<IdentityRef>,
    /// Operator who controls this agent's policy and lifecycle.
    operator: OperatorRef,
    /// Version of the AgentSpec that defines this agent's behavior.
    spec_version: SpecVersion,
}

/// Binding between an agent and a Polkadot account.
struct AccountBinding {
    /// The on-chain account in SS58 format for the target network.
    ss58_address: SS58Address,
    /// Raw 32-byte public key (AccountId32 for Substrate accounts).
    account_id: AccountId32,
    /// Network genesis hash that scopes this binding.
    genesis_hash: GenesisHash,
    /// Signature proving the account holder authorized this binding.
    binding_proof: Signature,
    /// When this binding was created.
    bound_at: Timestamp,
    /// Optional expiry.
    expires_at: Option<Timestamp>,
}
```

### 2.2 Person identity

A person interacts with Polkagent as a user, operator, approver, or
administrator. Person identity connects the human to their accounts, agents,
and organizations.

| Attribute | Source | Trust level |
|---|---|---|
| Local username | OS/process authentication | Trusted within the local deployment |
| Polkadot account | Cryptographic key ownership | Verified by signature |
| People Chain display name | On-chain bonded field | Informational; not authorization |
| Registrar judgment | People Chain registrar attestation | Qualified signal; registrar-specific |
| Verifiable credential | W3C VC standard, off-chain | Issuer-dependent |
| OAuth/OIDC identity | External identity provider | Provider-dependent |
| Passkey/WebAuthn | Device-bound credential | Strong device authentication |
| Organization membership | Organization identity system | Organization-dependent |

**Invariant:** a person's display name, registrar judgment, or reputation score
is never sufficient to authorize an action. Authorization requires a valid
grant derived from explicit policy.

### 2.3 Organization identity

Organizations operate agents on behalf of teams, departments, or legal
entities. Organization identity enables:

- **Tenancy:** isolation of data, secrets, policies, and audit between
  organizations in a managed deployment.
- **Delegation:** controlled transfer of authority from organization to team to
  individual to agent.
- **Hierarchy:** nested teams with inherited and overridden policies.
- **Compliance:** audit, retention, and access controls scoped to organizational
  boundaries.

```rust
struct OrganizationIdentity {
    org_id: OrgId,
    display_name: String,
    /// Root accounts that can manage this organization.
    admin_accounts: Vec<AccountId32>,
    /// Authentication method for organization members.
    auth_method: OrgAuthMethod,
    /// Policy that governs all agents and users within this organization.
    org_policy: PolicyRef,
    /// Optional on-chain identity.
    chain_identity: Option<IdentityRef>,
}

enum OrgAuthMethod {
    /// Members authenticate with Polkadot account signatures.
    PolkadotAccount,
    /// Members authenticate through an external identity provider.
    ExternalIdP { provider_url: Url, client_id: String },
    /// Members use organization-managed API keys.
    ApiKey,
    /// Combination of methods.
    Multi(Vec<OrgAuthMethod>),
}
```

### 2.4 Service identity

Services are non-human principals that interact with Polkagent programmatically:
API clients, webhook sources, external tools, harness processes, and
agent-to-agent connections.

| Service type | Authentication | Typical grants |
|---|---|---|
| API client | API key or OAuth2 client credentials | Scoped to specific agent operations |
| Webhook source | HMAC signature or shared secret | Trigger-only; no direct effect authority |
| Harness process | Process-local token issued at start | Tool invocation within run grants |
| Agent-to-agent | Mutual key authentication | Negotiated per-interaction grants |
| Control-plane service | mTLS or service account token | Administrative operations within tenant |

```rust
struct ServiceIdentity {
    service_id: ServiceId,
    service_type: ServiceType,
    /// Credentials are handles, not raw values.
    credential_ref: SecretRef,
    /// Maximum grants this service may receive.
    max_grants: GrantConstraints,
    /// When this identity expires and must be rotated.
    expires_at: Option<Timestamp>,
    /// Who created and manages this service identity.
    owner: PrincipalRef,
}
```

---

## 3. Polkadot account model

### 3.1 SS58 address format

Polkadot uses the SS58 address format, a base-58 encoding that includes:

- A **network prefix** (0 for Polkadot, 2 for Kusama, 42 for generic Substrate).
- A **32-byte public key** (AccountId32 for standard Substrate accounts).
- A **checksum** for error detection.

The same underlying key pair can produce different SS58 addresses on different
networks. Polkagent must:

- Always display the SS58 address appropriate for the target network.
- Store and compare the raw 32-byte AccountId32 internally.
- Validate SS58 checksums before any operation.
- Display both the SS58 address and a truncated raw key for disambiguation.

### 3.2 Key types and derivation

Polkadot supports multiple cryptographic key types:

| Key type | Algorithm | Use | Polkagent relevance |
|---|---|---|---|
| **Sr25519** | Schnorr over Ristretto255 | Default account keys, staking | Primary account type |
| **Ed25519** | EdDSA over Curve25519 | Alternative account keys, some validators | Supported alternative |
| **ECDSA/secp256k1** | Elliptic curve DSA | Ethereum-compatible accounts, EVM layer | Hub EVM compatibility |

**Key derivation** uses a hierarchical scheme:

- **Hard derivation** (`//path`): produces a new key pair that cannot be linked
  to the parent without the seed. Used for account isolation.
- **Soft derivation** (`/path`): produces a derived key that can be verified
  against the parent public key. Used for sub-accounts.

**Polkagent requirements:**

- REQ-ACCT-01: Support Sr25519, Ed25519, and ECDSA account types.
- REQ-ACCT-02: Never perform key derivation within the agent runtime. Derivation
  happens exclusively within signer implementations.
- REQ-ACCT-03: Store only public keys and account references in the agent
  database. Private keys exist only within signer boundaries.

### 3.3 Account lifecycle

```text
create key pair (in signer) -> derive address -> fund (existential deposit)
  -> use for transactions -> optionally set identity -> optionally create proxy
  -> optionally add to multisig -> rotate/revoke -> archive
```

**Existential deposit (ED):** every Polkadot account must maintain a minimum
balance (the ED) or it will be reaped (removed from chain state). Polkagent must:

- REQ-ACCT-04: Display the ED for the target chain before any transfer.
- REQ-ACCT-05: Warn when a transfer would reduce an account below ED.
- REQ-ACCT-06: Never silently allow account reaping through agent-initiated
  transfers.

### 3.4 Hub EVM account mapping

Polkadot Hub can map native 32-byte Substrate accounts to 20-byte
Ethereum-compatible addresses for the EVM layer. This mapping is
profile-specific and must be verified against the target chain's current
runtime metadata.

- REQ-ACCT-07: When operating on Hub EVM, display both the native SS58 address
  and the mapped Ethereum address.
- REQ-ACCT-08: Verify account mapping against current chain evidence before
  constructing EVM-related intents.

---

## 4. People Chain and identity integration

### 4.1 What People Chain provides

**Verified fact (2026-07-29):** People Chain is the current specialized
identity parachain in the Polkadot ecosystem. It provides:

- **Identity fields:** bonded on-chain fields including display name, legal
  name, email, web, Twitter/X handle, and custom fields. Setting identity
  requires a bond (deposit).
- **Registrar judgments:** independent registrars can attest to the accuracy of
  identity fields. Judgment levels include Unknown, Reasonable, KnownGood, and
  Erroneous.
- **Sub-identities:** an account can create sub-identities linked to a parent,
  useful for organizational hierarchies.
- **Identity clearing:** an account holder can clear their identity and recover
  their bond.

Sources: [People Chain reference](https://docs.polkadot.com/reference/polkadot-hub/people-and-identity/),
[identity guide](https://wiki.polkadot.com/learn/learn-identity/)

### 4.2 What People Chain does NOT provide

People Chain identity is NOT:

- **Authorization.** A registrar judgment does not grant capabilities.
- **Universal personhood.** It does not prove a person is unique or human.
- **Message authentication.** It does not prove a chat message came from the
  identity holder.
- **Signer consent.** It does not prove a signer reviewed and approved a
  payload.
- **Privacy-preserving.** On-chain identity fields are public. Private data
  must not be placed in identity fields.

### 4.3 Polkagent identity integration requirements

**Verified fact:** Registrar judgements can be queried programmatically via the
People Chain identity pallet's storage (`IdentityOf`, `SuperOf`, `SubsOf`).
Polkagent uses this for counterparty and agent identity attestation — e.g.,
checking whether an agent's operator account has a `KnownGood` or `Reasonable`
judgement before granting elevated defaults in policy. Sub-identities
(`SubsOf`) are the natural structure for agent fleets operated under a single
organizational account: each agent gets a sub-identity linked to the parent,
making fleet membership verifiable on-chain without a separate registry.

- REQ-IDENT-01: Treat People Chain identity as a **read-only signal**. Display
  registrar status as contextual information, never as authorization.
- REQ-IDENT-02: Resolve identity for display purposes through a typed
  `IdentityResolver` that returns qualified, timestamped claims:

```rust
struct IdentityResolution {
    /// The account whose identity was resolved.
    account: AccountId32,
    /// Network where identity was queried.
    network: NetworkId,
    /// Block at which identity was read.
    at_block: BlockRef,
    /// Resolved fields, each with source and confidence.
    fields: Vec<IdentityField>,
    /// Registrar judgments, each with registrar identity and level.
    judgments: Vec<RegistrarJudgment>,
    /// Whether this resolution is stale (block age > threshold).
    is_stale: bool,
}

struct IdentityField {
    field_name: String,
    field_value: String,
    source: IdentitySource,
}

enum IdentitySource {
    /// Directly from on-chain identity pallet.
    OnChain { block: BlockRef },
    /// From a cached resolution.
    Cached { resolved_at: Timestamp, cache_ttl: Duration },
}

struct RegistrarJudgment {
    registrar_index: u32,
    registrar_account: AccountId32,
    judgment: JudgmentLevel,
    at_block: BlockRef,
}

enum JudgmentLevel {
    Unknown,
    Reasonable,
    KnownGood,
    Erroneous,
    OutOfDate,
    LowQuality,
    FeePaid(Balance),
}
```

- REQ-IDENT-03: Always display the block reference and staleness indicator
  alongside identity information.
- REQ-IDENT-04: Never use identity fields as the sole basis for recipient
  verification in transfers. The canonical recipient is the AccountId32, not a
  display name.
- REQ-IDENT-05: Support sub-identity resolution for organizational account
  structures. Agent fleets should use People Chain sub-identities to make
  fleet membership verifiable without a separate off-chain registry.
- REQ-IDENT-06: Do not store personal identity information from People Chain in
  Polkagent's local database beyond a time-limited cache with explicit retention
  policy.

### 4.4 Personhood (experimental)

Parity's 2025 roundup reports initial deployment work on an individuality and
proof-of-personhood direction using Bandersnatch RingVRF. This is explicitly
experimental for Polkagent:

- REQ-IDENT-07: Do not gate baseline Polkagent functionality on personhood
  verification.
- REQ-IDENT-08: If personhood becomes a stable product API, it may serve as one
  policy input (e.g., anti-Sybil rate limiting for public agents), never as
  sole authorization.

---

## 5. Agent cards

### 5.1 Purpose

An agent card is a signed, portable descriptor that allows external parties to
discover, verify, and interact with an agent. It binds an agent's identity to
its capabilities, operator, accounts, and contact information.

### 5.2 Agent card structure

```rust
struct AgentCard {
    /// Unique identifier for this agent.
    agent_id: AgentId,
    /// Human-readable name.
    display_name: String,
    /// Version of the agent's spec.
    spec_version: SpecVersion,
    /// Operator identity.
    operator: OperatorRef,
    /// Capabilities this agent offers.
    offered_capabilities: Vec<CapabilityDescriptor>,
    /// Polkadot accounts this agent is bound to.
    bound_accounts: Vec<AccountBinding>,
    /// Supported interaction protocols.
    protocols: Vec<ProtocolDescriptor>,
    /// Contact and discovery information.
    endpoints: Vec<EndpointDescriptor>,
    /// When this card was issued.
    issued_at: Timestamp,
    /// When this card expires.
    expires_at: Timestamp,
    /// Content hash of the card body.
    digest: Sha256Digest,
    /// Signature over the digest by the agent's transport key.
    signature: Signature,
}
```

### 5.3 Agent card requirements

- REQ-CARD-01: An agent card is optional. Local-only agents need no card.
- REQ-CARD-02: A card must be signed by the agent's transport key. Verification
  requires only the public key.
- REQ-CARD-03: A card has an explicit expiry. Expired cards must not be accepted
  for interaction initiation.
- REQ-CARD-04: A card's `bound_accounts` must each carry a proof that the
  account holder authorized the binding.
- REQ-CARD-05: A card's `offered_capabilities` are descriptive. They do not
  grant the agent any authority; a consumer must still establish grants through
  policy.
- REQ-CARD-06: Display data from a card (name, capabilities, operator) must
  never be treated as authorization. UI must visually separate card-sourced
  information from policy-derived authority.

---

## 6. Verifiable credentials

### 6.1 Standards

Polkagent may issue and verify credentials following the W3C Verifiable
Credentials Data Model (VCDM) where interoperability with external systems
is needed. This is a phased capability.

### 6.2 Credential types relevant to Polkagent

| Credential type | Issuer | Subject | Use |
|---|---|---|---|
| Agent capability attestation | Operator or evaluator | Agent | Attests that an agent passed specific evaluations |
| Operator authorization | Organization | Operator | Delegates operational authority within org boundaries |
| Account ownership | Account holder | Agent/Service | Proves account binding without exposing keys |
| Evaluation result | Evaluation service | Agent/Skill | Records evaluation outcomes |
| Service agreement | Buyer and seller | Agent service | Captures terms of a service engagement |

### 6.3 Requirements

- REQ-CRED-01: Verifiable credentials are optional and phased. Core
  functionality must not depend on VC infrastructure.
- REQ-CRED-02: Credential verification must check issuer identity, expiry,
  revocation status, and schema conformance.
- REQ-CRED-03: Credentials are evidence artifacts, not authorization sources.
  They may inform policy decisions but cannot override grant constraints.
- REQ-CRED-04: Private credential data must follow the same classification
  rules as all other Polkagent data (Public, Private, Sensitive,
  SecretForbidden).

---

## 7. Signer architecture

The signer is the most critical security boundary in Polkagent. It is the
component that authorizes Polkadot transactions by signing canonical payloads.
The design must ensure that signing keys never leave the signer boundary and
that the signer only acts on properly authorized, fully verified payloads.

### 7.1 The key isolation invariant

**Invariant INV-SIGN-01: Models NEVER see raw keys.**

No language model, prompt template, agent memory, harness process, tool
implementation, marketplace extension, log output, artifact body, event
stream, projection, API response, debug dump, error message, or configuration
file may contain raw private key material, seed phrases, or key derivation
paths that could reconstruct a private key.

This invariant holds regardless of:

- Autonomy level (including fully autonomous operation).
- Custody mode (including local encrypted keystore).
- Deployment topology (including single-user local).
- Extension trust tier (including built-in tools).
- Error or crash conditions.

**Enforcement mechanisms:**

- The `Signer` trait accepts only canonical payload references, never raw keys.
- Secret material uses `SecretRef` handles resolved by a dedicated secret
  store; the agent runtime never holds the resolved value. Key material is
  referenced exclusively through opaque handles — the model and the agent
  runtime see a `SignerId` or `SecretRef`, never a byte slice of key material.
- In Rust, any short-lived buffer that transiently holds decrypted key material
  must be wrapped in a `zeroize`-on-drop type (e.g., `secrecy::Secret<[u8; 32]>`
  or a custom `Zeroize` impl) so that decrypted key bytes are overwritten
  immediately after use and never linger in freed heap memory.
- Data classification marks all key material as `SecretForbidden`; any
  component that attempts to log, store, or transmit `SecretForbidden` data
  triggers an immediate security event.
- Artifact admission rejects content classified as `SecretForbidden`.
- Build-time and test-time checks verify that `Signer` implementations do not
  expose key material through their public API surface.

### 7.2 Signer port trait

All signer implementations conform to a single narrow trait. The trait receives
a fully prepared, canonical payload and returns a signature or refusal. It
does not participate in payload construction, policy evaluation, or intent
creation.

```rust
/// The isolated signing boundary.
///
/// A signer receives a fully canonical payload that has already passed:
/// - Intent creation with metadata pinning
/// - Call decoding and evidence capture
/// - Simulation (where supported)
/// - Policy evaluation producing a ResolvedGrant
/// - Authorization decision (human approval or valid mandate)
///
/// The signer's only job is to cryptographically sign the exact bytes
/// or refuse.
#[async_trait]
trait Signer: Send + Sync {
    /// Unique identifier for this signer instance.
    fn signer_id(&self) -> &SignerId;

    /// The account(s) this signer can sign for.
    fn accounts(&self) -> &[AccountId32];

    /// The key type(s) this signer supports.
    fn supported_key_types(&self) -> &[KeyType];

    /// Request a signature on a canonical payload.
    ///
    /// The signer receives:
    /// - The exact bytes to sign (SCALE-encoded extrinsic payload).
    /// - The authorization decision that approved this signing.
    /// - A display-ready summary for signers that show UI (wallets, hardware).
    ///
    /// The signer MUST NOT:
    /// - Modify the payload.
    /// - Sign a different payload.
    /// - Expose key material in its return value or side effects.
    /// - Cache or log the payload bytes.
    async fn sign(
        &self,
        request: SigningRequest,
    ) -> SigningResult;

    /// Check whether this signer is available and healthy.
    async fn health(&self) -> SignerHealth;

    /// Report the signer's capabilities and constraints.
    fn capabilities(&self) -> SignerCapabilities;
}

/// A request to sign a canonical payload.
struct SigningRequest {
    /// Unique, non-reusable identifier for this signing request.
    request_id: SigningRequestId,
    /// The exact canonical bytes to sign.
    payload: CanonicalPayload,
    /// The account that should sign.
    signing_account: AccountId32,
    /// The key type to use.
    key_type: KeyType,
    /// The network this signature targets.
    network: NetworkId,
    /// The authorization decision that approved this signing.
    authorization: AuthorizationDecision,
    /// Human-readable summary for signer UIs.
    display_summary: DisplaySummary,
    /// When this request expires. The signer must refuse after expiry.
    expires_at: Timestamp,
    /// Policy revision that was active when this request was created.
    policy_revision: Digest,
}

/// The result of a signing attempt.
enum SigningResult {
    /// Signature was produced.
    Signed {
        signature: Signature,
        signer_account: AccountId32,
        signed_at: Timestamp,
    },
    /// Signer refused the request.
    Refused {
        reason: SignerRefusalReason,
        signer_id: SignerId,
    },
    /// Signer is unavailable (hardware disconnected, service down).
    Unavailable {
        reason: String,
        retry_after: Option<Duration>,
    },
    /// Request expired before signing completed.
    Expired {
        request_id: SigningRequestId,
    },
}

enum SignerRefusalReason {
    /// User explicitly declined in the wallet UI.
    UserDeclined,
    /// The payload failed the signer's own validation.
    PayloadRejected { details: String },
    /// The account is not available in this signer.
    AccountNotFound,
    /// The key type is not supported.
    KeyTypeNotSupported,
    /// The signer's internal policy denied the request.
    PolicyDenied { details: String },
    /// Hardware/security module error.
    HardwareError { details: String },
}
```

### 7.3 Signer implementations

Polkagent supports multiple signer modes behind the single `Signer` trait.
Each mode has different security properties, user experience, and operational
requirements.

#### 7.3.1 Watch-only signer

**Phase: v1 (default)**

The watch-only signer cannot sign. It exists to support read-only operation
where an agent monitors accounts and prepares intents without any signing
capability.

```rust
struct WatchOnlySigner {
    watched_accounts: Vec<AccountId32>,
}
// sign() always returns Refused { reason: WatchOnly }
```

- REQ-SIGN-01: The default signer for new agents is watch-only.
- REQ-SIGN-02: Watch-only mode is always available, even when other signers are
  configured, as a deliberate safety choice.

#### 7.3.2 External wallet signer

**Phase: v1**

The user's existing wallet application performs signing. Polkagent exports the
canonical payload and the wallet returns a signature.

```text
Polkagent prepares intent -> exports canonical payload
  -> wallet displays payload details -> user reviews and approves
  -> wallet signs -> signature returned to Polkagent
  -> Polkagent verifies signature matches request
```

Supported wallet protocols (verified against current ecosystem):

- **Polkadot.js/browser extension:** WalletConnect or injected provider.
- **Substrate Connect:** light-client-based wallet integration.
- **QR code / Polkadot Vault:** air-gapped signing flow.
- **Ledger hardware:** USB/Bluetooth connection.
- **WalletConnect v2:** cross-device wallet connection.

- REQ-SIGN-03: The external wallet signer must verify that the returned
  signature corresponds to the exact payload sent for signing.
- REQ-SIGN-04: The signer must correlate the signature to the `SigningRequestId`
  and reject signatures for any other request.
- REQ-SIGN-05: Payload export must include the genesis hash, metadata hash,
  runtime version, call data, and Polkagent intent ID.

#### 7.3.3 Hardware wallet (Ledger, Polkadot Vault)

**Phase: v1**

Hardware wallets provide the strongest key isolation because keys never leave
the hardware device.

**Ledger:**
- Keys generated and stored on the device.
- Signing requires physical button confirmation.
- Limited display for payload review.
- USB or Bluetooth connection.

**Polkadot Vault (formerly Parity Signer):**
- Air-gapped mobile device as signing device.
- Payload transferred via QR code.
- Signature returned via QR code.
- No network connection on the signing device.

**Verified fact:** The Ledger Common/Generic App leverages on-chain metadata
together with RFC-0078 metadata-hash shortening (`CheckMetadataHash` signed
extension). This makes the app runtime-upgrade resilient: the short metadata
hash allows Ledger to verify that the call it is about to sign matches the
metadata the runtime currently carries, without storing the full metadata on
the device. Decoded extrinsic fields are displayed on the device screen
offline. Polkagent must supply the correct metadata hash in the signing payload
so that Ledger can perform this verification.

**Verified fact:** Polkadot Vault (air-gapped, QR transport) is the
cold-storage default for high-value agent accounts. It represents the
strongest instantiation of the "keys outside the model" invariant: the signing
key never touches any networked device. Policy-bounded hot keys (referenced by
opaque handle, held in `zeroize`d secure memory) handle low-value bounded
autonomy; Vault handles anything requiring higher assurance.

- REQ-SIGN-06: Hardware signer adapters must handle device disconnection,
  timeout, and user cancellation gracefully.
- REQ-SIGN-07: For Polkadot Vault, the QR code flow must encode the complete
  payload context including network and intent binding.
- REQ-SIGN-07a: For Ledger, the signing payload must include the RFC-0078
  metadata hash so the device can verify the call against current runtime
  metadata without requiring a full metadata download.

#### 7.3.4 Proxy and multisig accounts

**Phase: v2**

Polkadot's runtime supports proxy and multisig account structures that provide
additional authorization layers.

**Proxy accounts:**
- A proxy can execute calls on behalf of another account.
- Proxy types restrict which calls are allowed (e.g., `Staking`, `Governance`,
  `NonTransfer`, `Any`).
- Time-delayed proxies add a waiting period before execution.
- Proxies can be added and removed by the proxied account.

**Multisig accounts:**
- Require M-of-N signers to authorize a call.
- Each signer independently reviews and signs.
- The final signer submits the call.

**Polkadot staking-operator proxy pattern (verified fact):** the documented
best practice for staking operators uses a controller account with a narrowly
filtered proxy that excludes balance-moving, bonding, and proxy-management
calls. Delays and revocation reduce blast radius.

**Agent account template:** Polkagent adopts this Staking-Operator pattern as
the standard template for all agent accounts. The preferred structure is a
**pure proxy** (keyless, non-deterministic address, no independent spending
power) with a **narrow proxy type** that excludes balance transfers and
proxy-management calls. Pure proxies prevent agents from adding their own
proxies (no privilege escalation path) and migrate cleanly to Asset Hub. For
high-value agent accounts, a **time-delay proxy** adds a mandatory waiting
period before execution, giving owners a window to cancel. The invariant
enforced by this structure: no model code path can reach raw key material or
submit an unbounded proxy call.

**Avoid:** `Any`-type pure proxies on Asset Hub (the target chain may not
support them, making the proxy inaccessible); always specify the narrowest
applicable proxy type.

```rust
struct ProxySignerConfig {
    /// The account being proxied.
    proxied_account: AccountId32,
    /// The proxy account that will sign.
    proxy_account: AccountId32,
    /// The proxy type restricting allowed calls.
    proxy_type: ProxyType,
    /// Optional time delay.
    delay: Option<BlockNumber>,
    /// The signer implementation for the proxy account.
    proxy_signer: Box<dyn Signer>,
}

struct MultisigSignerConfig {
    /// The multisig account address.
    multisig_account: AccountId32,
    /// Threshold for approval.
    threshold: u16,
    /// All signatories.
    signatories: Vec<AccountId32>,
    /// The signer for this node's signatory.
    local_signer: Box<dyn Signer>,
}
```

- REQ-SIGN-08: Proxy and multisig are operator-configured bounds, never an
  excuse to skip payload review.
- REQ-SIGN-09: Polkagent must verify the proxy type's call filter against the
  intended call before attempting a proxied signature.
- REQ-SIGN-10: For multisig, Polkagent must track approval state and present
  each signatory with the full decoded call data.
- REQ-SIGN-11: Proxy addition and removal operations require explicit
  operator approval and are flagged as high-risk in action cards.

#### 7.3.5 Local encrypted keystore

**Phase: v2 (gated)**

A local encrypted keystore stores key material on disk, encrypted with a
user-provided passphrase or hardware-backed key.

```text
User provides passphrase -> derive encryption key (Argon2id)
  -> decrypt signing key in memory -> sign -> zeroize signing key
```

- REQ-SIGN-12: Local keystore keys are encrypted at rest with Argon2id-derived
  keys.
- REQ-SIGN-13: Decrypted key material is zeroized immediately after signing.
  In Rust, use the `zeroize` crate (via `Zeroize` / `ZeroizeOnDrop` derive or
  `secrecy::Secret<T>`) on any stack or heap buffer holding decrypted key bytes.
  This prevents stale key material lingering in freed memory.
- REQ-SIGN-14: The keystore file must not be readable by the agent process
  during normal operation. A separate short-lived signer process handles
  decryption and signing.
- REQ-SIGN-15: Local keystore is explicitly labeled in UI as self-custody with
  clear recovery, backup, and rotation guidance.

#### 7.3.6 Organizational / delegated signing

**Phase: v2**

Organizations may operate a signing service that enforces organizational
policy before signing.

```text
Agent requests signature -> organizational signer service
  -> service evaluates org policy -> service signs if approved
  -> signature returned with org policy evidence
```

- REQ-SIGN-16: Organizational signers must return evidence of which
  organizational policy was evaluated.
- REQ-SIGN-17: The organizational signer is a separate service from the
  Polkagent runtime; it is not embedded in the agent process.

#### 7.3.7 Managed KMS/HSM

**Phase: v2 (gated)**

Cloud-managed key management services (AWS KMS, GCP Cloud KMS, Azure Key
Vault) or hardware security modules provide custody with operational controls.

- REQ-SIGN-18: KMS/HSM signers must log every signing operation to the KMS
  audit trail independently of Polkagent's own audit.
- REQ-SIGN-19: KMS key policies must be documented as part of the deployment
  configuration, not silently inherited.
- REQ-SIGN-20: Polkagent must not assume that KMS custody removes the need
  for its own policy evaluation. KMS signs after Polkagent policy approves.

#### 7.3.8 MPC signing

**Phase: v3 (experimental)**

Multi-party computation signing distributes key shares across multiple
parties so that no single party holds the complete key. MPC-as-a-service
(MPCaaS) providers offer hosted threshold signing for sr25519 and other
key types, removing the need to operate share distribution infrastructure.
W3C Verifiable Credentials / DIDs for agent capability attestation are a
related direction: an agent's capability set could be attested by a VC issued
by an evaluator, enabling cross-organizational trust without a shared registry.
Both MPC for sr25519 and W3C VC/DID attestation are promising directions but
are not v1; they require additional ecosystem maturity and independent security
review before handling real value.

- REQ-SIGN-21: MPC signing is experimental. It requires independent security
  review before handling real value.
- REQ-SIGN-22: MPC key generation, share distribution, and signing ceremonies
  must be documented and auditable.

#### 7.3.9 Programmable / smart accounts

**Phase: v3 (experimental)**

Smart contract-based accounts can enforce arbitrary on-chain logic before
authorizing transactions.

- REQ-SIGN-23: Smart account integration requires verified contract audit and
  tested interaction patterns.
- REQ-SIGN-24: The smart account's on-chain logic is an additional enforcement
  layer; it does not replace Polkagent's off-chain policy evaluation.

#### 7.3.10 Funded agent accounts

**Phase: v3 (gated)**

For fully autonomous operation, an agent may have a funded account dedicated
to its operations. This is the highest-autonomy signing mode.

```rust
struct FundedAgentAccount {
    /// The agent's own funded account.
    agent_account: AccountId32,
    /// The signer for this account (may be local keystore, KMS, or proxy).
    account_signer: Box<dyn Signer>,
    /// Mandatory: the mandate that authorizes autonomous operation.
    mandate: AutonomousMandate,
    /// Mandatory: circuit breaker configuration.
    circuit_breaker: CircuitBreakerConfig,
    /// Mandatory: notification targets for autonomous actions.
    notifications: Vec<NotificationTarget>,
    /// Mandatory: emergency pause/revoke mechanism.
    emergency_controls: EmergencyControlConfig,
}
```

- REQ-SIGN-25: Funded agent accounts require explicit, understandable mandate
  configuration.
- REQ-SIGN-26: Funded accounts must have circuit breakers with per-action,
  rolling, and lifetime budget limits.
- REQ-SIGN-27: The mandate owner must be able to pause or revoke the agent's
  signing authority at any time through an out-of-band mechanism.
- REQ-SIGN-28: Every autonomous signing operation must produce a receipt with
  mandate reference, policy evidence, and action details.

### 7.4 Signer selection and lifecycle

```text
Agent configuration specifies signer mode
  -> runtime resolves signer at startup
  -> signer reports health and capabilities
  -> per-action: policy selects eligible signer
  -> signing request flows through Signer trait
  -> result recorded as EffectOutcome
```

- REQ-SIGN-29: An agent may have multiple signers configured for different
  accounts, networks, or action families.
- REQ-SIGN-30: Signer selection is part of policy evaluation, not model choice.
  A model cannot select which signer to use.
- REQ-SIGN-31: Signer health is monitored. If a signer becomes unavailable,
  pending requests fail gracefully rather than falling back to a different
  signer without policy authorization.

---

## 8. Policy and capability model

### 8.1 Design principles

Policy is the mechanism by which Polkagent determines what any principal may
do. The design follows these principles:

1. **Deny by default.** A new agent has no effects until explicit grants exist.
2. **Intersection, not union.** The effective permission is the intersection of
   all governing policies. A broad flag cannot override a specific restriction.
3. **Outside model control.** Policy evaluation is deterministic code. An LLM
   cannot modify, bypass, or influence policy evaluation.
4. **Immutable resolution.** Once a grant is resolved for an operation, it is
   hashed and bound to the effect attempt. It cannot be retroactively changed.
5. **Time-bounded.** Every grant has an expiry. There are no perpetual grants.
6. **Auditable.** Every policy decision is recorded with the policy revision,
   input facts, and resulting grant or denial.

**Policy engine: Cedar (verified fact).** Polkagent uses Cedar as its
policy-as-code engine. Cedar is written in Rust (embeddable as a crate with no
sidecar), formally verified in Lean, deny-by-default, and sub-millisecond in
evaluation. Its PARC model (Principal / Action / Resource / Context) maps
directly to Polkagent's grant structure. Cedar's schema validator catches
policy errors before deployment, which is critical for a safety gate. The
alternative (OPA/Rego) requires a sidecar or CGo and is more expressive than
needed; Cedar's Rust-native embedding and Lean proofs win for this use.

**Capability-based access with WASI handles.** For marketplace plugins hosted
in Wasmtime, capability access follows the WASI capability model: each plugin
receives only the capability handles explicitly granted to it at load time. A
plugin cannot acquire ambient authority or import capabilities beyond those
listed in its WIT world. This maps naturally onto Cedar's PARC grants —
the Cedar policy determines which handles are handed to a plugin, and the
Wasmtime host enforces that no other ambient access is possible.

### 8.2 Grant structure

A grant is the resolved, immutable authorization for a specific operation:

```rust
/// The resolved authorization for one operation.
/// Created by policy evaluation; immutable once produced.
struct ResolvedGrant {
    /// Unique identifier for this grant.
    grant_id: GrantId,
    /// Who is authorized (agent, user, service).
    subject: SubjectId,
    /// What resource or operation is authorized.
    resource: ResourceSelector,
    /// Which effects are permitted.
    allowed_effects: EffectSet,
    /// Quantitative limits.
    limits: GrantLimits,
    /// Authorization decisions that contributed to this grant.
    approvals: Vec<ApprovalRef>,
    /// When this grant expires.
    expires_at: Timestamp,
    /// Digest of the policy revision used to produce this grant.
    policy_revision: Digest,
    /// Digest of the entire resolved grant for binding to effects.
    digest: Digest,
}

/// Quantitative limits on a grant.
struct GrantLimits {
    /// Maximum number of requests allowed.
    max_requests: Option<u64>,
    /// Maximum total bytes.
    max_bytes: Option<u64>,
    /// Maximum spend in the grant's asset.
    max_spend: Option<Balance>,
    /// Rolling spend limit (e.g., per hour/day).
    rolling_spend: Option<RollingLimit>,
    /// Deadline for completing the granted operation.
    deadline: Option<Timestamp>,
    /// Maximum concurrent operations.
    max_concurrency: Option<u32>,
}

/// Selects which resources a grant applies to.
enum ResourceSelector {
    /// A specific tool by ID and version.
    Tool { tool_id: ToolId, version: VersionReq },
    /// A filesystem path pattern.
    FileSystem { roots: Vec<PathBuf>, access: FileAccess },
    /// A network host pattern.
    Network { hosts: Vec<HostPattern>, access: NetworkAccess },
    /// A chain/account/call pattern.
    Chain {
        network: NetworkId,
        accounts: Vec<AccountId32>,
        call_filter: CallFilter,
    },
    /// A model/provider route.
    Model { route: ModelRoute },
    /// Multiple resources.
    Composite(Vec<ResourceSelector>),
}
```

### 8.3 Capability resolution

Capabilities flow from declaration through policy to resolved grants:

```text
Extension/tool/agent declares required capabilities in manifest
  -> Operator reviews and accepts at installation
  -> Policy evaluation intersects:
       platform policy
     ∩ workspace/environment policy
     ∩ organization/tenant policy
     ∩ sender/conversation policy
     ∩ declared extension capability
     ∩ account/signer policy
     ∩ chain-specific risk policy
     ∩ current approval or mandate
  -> Produces ResolvedGrant or Denial
  -> Grant is immutable, hashed, bound to effect
```

**Invariant INV-POLICY-01:** The effective permission is always the
intersection of all applicable policies. No single policy layer can
grant capabilities that another layer denies.

**Invariant INV-POLICY-02:** Policy evaluation is a pure function of its
inputs. Given the same policy revision, the same facts, and the same
request, it must produce the same decision.

### 8.4 Policy evaluation pipeline

```rust
/// The policy evaluation pipeline.
/// Each stage is independent and short-circuits on denial.
struct PolicyPipeline {
    stages: Vec<Box<dyn PolicyStage>>,
}

#[async_trait]
trait PolicyStage: Send + Sync {
    /// Evaluate this stage.
    /// Returns Continue with accumulated constraints, or Deny.
    async fn evaluate(
        &self,
        request: &PolicyRequest,
        accumulated: &AccumulatedConstraints,
    ) -> PolicyStageResult;
}

enum PolicyStageResult {
    /// Continue to next stage with tightened constraints.
    Continue(AccumulatedConstraints),
    /// Deny the request with explanation.
    Deny(PolicyDenial),
    /// Require explicit approval before proceeding.
    RequireApproval(ApprovalRequirement),
}

struct PolicyDenial {
    stage: String,
    reason: String,
    policy_revision: Digest,
    /// The specific constraint that caused the denial.
    constraint: String,
}
```

The standard pipeline stages are:

| Stage | Purpose | Short-circuit on |
|---|---|---|
| 1. Platform defaults | Enforce platform-wide invariants | Invariant violation |
| 2. Organization policy | Apply tenant/org constraints | Org-level denial |
| 3. Workspace/environment | Apply deployment-specific rules | Environment restriction |
| 4. Agent/product policy | Apply agent-specific grants | Agent not authorized |
| 5. Sender/session policy | Apply per-user/conversation limits | Sender not permitted |
| 6. Extension capability | Intersect with declared capabilities | Capability not declared |
| 7. Account/signer policy | Verify signer availability and account constraints | Account/signer restriction |
| 8. Chain risk policy | Apply chain-specific risk rules | Risk threshold exceeded |
| 9. Approval/mandate | Verify human approval or valid mandate | No valid authorization |
| 10. Budget/rate | Check remaining budget and rate limits | Budget exhausted |

### 8.5 Least privilege default

**Invariant INV-POLICY-03:** A new agent starts with no granted effects.
Read-only observation of configured chain profiles and response to its
operator are the only default capabilities. Every additional capability
requires explicit configuration.

Default grant profile for a new agent:

```yaml
default_grants:
  # Can respond to its operator through configured transport.
  - resource: transport/operator
    effects: [respond]
  # Can read configured chain profiles (no write).
  - resource: chain/configured_profiles
    effects: [query, subscribe]
    limits:
      max_requests: 1000/hour
  # Can use configured model for conversation (no tools).
  - resource: model/configured_default
    effects: [inference]
    limits:
      max_requests: 100/hour
      max_bytes: 10MB/hour
```

### 8.6 Time-bounded grants

Every grant has an explicit expiry:

- **Short-lived grants** (minutes to hours): per-action approvals, session
  authorizations, tool invocations.
- **Medium-lived grants** (hours to days): batch approvals, workspace sessions,
  deployment windows.
- **Long-lived grants** (days to months): autonomous mandates, service
  agreements, organizational delegations.

- REQ-POLICY-01: No grant may be created without an expiry.
- REQ-POLICY-02: Expired grants must be denied immediately; no grace period.
- REQ-POLICY-03: Grant renewal requires re-evaluation through the full policy
  pipeline.

### 8.7 Scope hierarchies

Resources are organized in a hierarchy that supports both broad and narrow
grants:

```text
platform
  └─ organization
       └─ workspace
            └─ agent
                 └─ run
                      └─ effect
```

A grant at a higher level provides an upper bound. A grant at a lower level
can only narrow, never widen, the parent grant.

```text
Example:
  Org policy allows: chain/polkadot/transfers up to 100 DOT/day
  Agent policy allows: chain/polkadot/transfers up to 10 DOT/day
  Run grant resolves: chain/polkadot/transfers up to 10 DOT/day (narrower wins)
```

---

## 9. Authentication flows

### 9.1 CLI authentication

```text
User starts CLI -> reads local config file with identity
  -> for operations requiring signing: prompts for signer access
  -> for cloud operations: OAuth2/PKCE flow or API key
```

- REQ-AUTH-01: CLI identity is derived from the OS user account and local
  configuration. No network call is required for local-only operation.
- REQ-AUTH-02: CLI API keys must not appear in command history. Use environment
  variables or secure credential storage.

### 9.2 Web authentication

```text
User opens browser -> OAuth2/OIDC login or Polkadot wallet connection
  -> session token with explicit scope and expiry
  -> for chain actions: wallet signing through browser extension or WalletConnect
```

- REQ-AUTH-03: Web sessions have explicit scope and expiry, not indefinite
  duration.
- REQ-AUTH-04: Wallet connection for signing is separate from web session
  authentication.

### 9.3 Mobile authentication

```text
User opens mobile app -> biometric/PIN authentication
  -> session established with configured agent
  -> for chain actions: wallet handoff or Polkadot Vault QR flow
```

- REQ-AUTH-05: Mobile sessions must support biometric and PIN fallback
  authentication.
- REQ-AUTH-06: Mobile signing flows must work with air-gapped devices
  (QR code paths).

### 9.4 API authentication

```text
Client sends request with API key or OAuth2 bearer token
  -> server validates token, resolves principal
  -> request processed within principal's grants
```

- REQ-AUTH-07: API keys are scoped to specific operations and have explicit
  expiry.
- REQ-AUTH-08: API key creation, rotation, and revocation must be auditable.
- REQ-AUTH-09: Rate limiting is applied per API key, not only per IP address.

### 9.5 Agent-to-agent authentication

```text
Agent A sends request with signed envelope
  -> Agent B verifies signature against known public key
  -> Capability negotiation establishes mutual grants
  -> Interaction proceeds within negotiated grants
```

- REQ-AUTH-10: Agent-to-agent authentication uses mutual cryptographic
  verification, not display names or reputation.
- REQ-AUTH-11: Each agent-to-agent interaction establishes fresh, scoped,
  time-bounded grants through capability negotiation.

---

## 10. Delegation model

### 10.1 Proxy chains

Polkadot supports nested proxy chains where account A delegates to B, and B
delegates to C. Polkagent must:

- REQ-DELEG-01: Track the full delegation chain for any proxied operation.
- REQ-DELEG-02: Display the delegation chain in approval UI so the user
  understands the authority path.
- REQ-DELEG-03: Verify that each link in the proxy chain is valid and not
  expired or revoked.
- REQ-DELEG-04: Enforce the most restrictive proxy type in the chain.

### 10.2 Authority limits

Delegation in Polkagent is subject to a fundamental constraint:

**Invariant INV-DELEG-01:** A delegate cannot have more authority than its
delegator. Authority can only narrow through delegation, never widen.

This applies at every level:

| Delegation type | Authority limit |
|---|---|
| Organization to team | Team cannot exceed org policy |
| Operator to agent | Agent cannot exceed operator grants |
| Agent to sub-agent | Sub-agent cannot exceed parent grants |
| Account to proxy | Proxy is bounded by proxy type filter |
| Multisig | Each signatory signs independently; threshold enforced on-chain |

### 10.3 Delegation records

```rust
struct DelegationRecord {
    /// Who delegated authority.
    delegator: PrincipalRef,
    /// Who received authority.
    delegate: PrincipalRef,
    /// What authority was delegated.
    granted: GrantConstraints,
    /// When the delegation was created.
    created_at: Timestamp,
    /// When the delegation expires.
    expires_at: Timestamp,
    /// Whether the delegation can be further sub-delegated.
    sub_delegatable: bool,
    /// Maximum depth of sub-delegation.
    max_depth: Option<u32>,
    /// The delegator's signature authorizing this delegation.
    delegator_signature: Signature,
}
```

---

## 11. Reputation

### 11.1 Reputation as evidence, not authorization

**Invariant INV-REP-01: Reputation NEVER authorizes.**

Reputation signals may influence:
- Discovery ranking in marketplaces.
- Default sort order in search results.
- Informational display alongside agent cards.
- Policy inputs where explicitly configured by an operator.

Reputation signals must NOT:
- Grant capabilities.
- Widen grants.
- Substitute for policy evaluation.
- Override explicit denials.
- Create implicit trust.

### 11.2 Reputation signals

| Signal | Source | Reliability | Use |
|---|---|---|---|
| Evaluation results | Deterministic test suites | High (reproducible) | Capability evidence |
| Usage statistics | Platform telemetry | Medium (gameable) | Popularity indicator |
| User ratings | User feedback | Low (subjective, gameable) | Sentiment indicator |
| Registrar judgments | People Chain registrars | Medium (registrar-dependent) | Identity confidence |
| Operator identity | Account + optional identity | Verifiable | Accountability |
| Publication history | Registry records | Verifiable | Track record |
| Security audit reports | Independent auditors | High (auditor-dependent) | Security confidence |

### 11.3 Anti-Sybil measures

For public-facing agent services and marketplaces, Sybil attacks (creating
many fake identities) are a concern:

- REQ-REP-01: Reputation systems must account for Sybil risk. Raw counts
  (number of ratings, number of users) are weak signals.
- REQ-REP-02: Polkadot account creation cost (existential deposit) provides
  minimal economic Sybil resistance.
- REQ-REP-03: When personhood verification becomes a stable product API,
  it may serve as an anti-Sybil signal for rate limiting and reputation
  weighting. It is never authorization.
- REQ-REP-04: Reputation manipulation (fake reviews, coordinated rating) must
  be detectable and reportable through abuse response mechanisms.

---

## 12. Privacy

### 12.1 Data minimization

- REQ-PRIV-01: Collect and retain only the data necessary for the configured
  operation.
- REQ-PRIV-02: Identity resolution caches must have explicit TTL and be
  purgeable.
- REQ-PRIV-03: On-chain identity fields are public; Polkagent must not mislead
  users about the visibility of data they place on-chain.

### 12.2 Data classification

All data within Polkagent is classified:

| Classification | Description | Retention | Access | Model context |
|---|---|---|---|---|
| **Public** | Intended for public visibility | Configurable | Open | Allowed |
| **Private** | User/tenant-scoped data | Configurable | Authenticated | Allowed with consent |
| **Sensitive** | PII, financial details, health | Minimized | Authorized + logged | Restricted |
| **SecretForbidden** | Keys, seeds, passwords, tokens | Never retained | Never in runtime | Forbidden |

- REQ-PRIV-04: Data classification is inherited: output inherits the most
  restrictive classification of its inputs unless an audited declassification
  policy applies.
- REQ-PRIV-05: `SecretForbidden` data must never appear in logs, artifacts,
  model context, API responses, error messages, or projections.

### 12.3 Consent and user control

- REQ-PRIV-06: Users must be able to inspect what data Polkagent holds about
  them.
- REQ-PRIV-07: Users must be able to delete their data, including derived
  indexes and caches.
- REQ-PRIV-08: Data export must include all user-attributed data in a portable
  format.
- REQ-PRIV-09: Sharing data with external services (model providers, chain
  nodes, analytics) requires explicit configuration, not silent default.

### 12.4 Tenant isolation

In managed deployments:

- REQ-PRIV-10: Tenant data must be isolated at every layer: database, storage,
  queue, cache, encryption key, telemetry, and support tooling.
- REQ-PRIV-11: Cross-tenant data access must be technically prevented, not
  merely policy-prohibited.
- REQ-PRIV-12: Managed service operators must not have default access to tenant
  conversation content, workspace files, or signing material.

---

## 13. Revocation and recovery

### 13.1 Key rotation

- REQ-REVOKE-01: Every credential type (API keys, agent transport keys, signer
  bindings, session tokens) must support rotation without service interruption.
- REQ-REVOKE-02: Key rotation must invalidate the old key immediately or within
  a configurable grace period.
- REQ-REVOKE-03: Rotation events are recorded in the audit trail with the old
  and new key identifiers (never the key material itself).

### 13.2 Emergency controls

```rust
struct EmergencyControlConfig {
    /// Accounts that can trigger emergency pause.
    emergency_accounts: Vec<AccountId32>,
    /// Out-of-band pause mechanism (e.g., API endpoint, kill switch).
    pause_mechanism: PauseMechanism,
    /// What happens when emergency pause is triggered.
    pause_behavior: PauseBehavior,
    /// Notification targets for emergency events.
    emergency_notifications: Vec<NotificationTarget>,
}

enum PauseBehavior {
    /// Stop all new operations; complete in-flight ones.
    GracefulStop,
    /// Stop all operations immediately, including in-flight.
    ImmediateStop,
    /// Stop only signing operations; continue read-only.
    SigningOnly,
}
```

- REQ-REVOKE-04: Every signing-capable agent must have a configured emergency
  pause mechanism.
- REQ-REVOKE-05: Emergency pause must work even if the agent's normal
  communication channels are compromised.
- REQ-REVOKE-06: Pause must be achievable through at least one out-of-band
  mechanism (direct API call, process signal, on-chain proxy removal).

### 13.3 Social recovery

For self-custody scenarios, Polkagent should support social recovery
mechanisms:

- REQ-REVOKE-07: Support configuration of recovery contacts who can
  collectively authorize key rotation or account recovery.
- REQ-REVOKE-08: Social recovery requires a configurable threshold (M-of-N
  contacts must approve).
- REQ-REVOKE-09: Recovery contacts are identified by Polkadot accounts with
  verified signatures, not by display names or email addresses.

### 13.4 Revocation propagation

When a credential, grant, or delegation is revoked:

- REQ-REVOKE-10: Revocation takes effect immediately for new operations.
- REQ-REVOKE-11: In-flight operations using the revoked credential must
  complete or fail within a configurable grace period.
- REQ-REVOKE-12: Dependent grants (grants derived from the revoked grant)
  must be transitively revoked.
- REQ-REVOKE-13: Revocation must propagate across deployment topologies
  (local cached policy must be updated or operations paused).

---

## 14. Threat model

### 14.1 Trust boundaries

```text
                                    UNTRUSTED
   ┌──────────────────────────────────────────────────────────────┐
   │  Public internet                                             │
   │  ┌─────────────┐  ┌──────────────┐  ┌────────────────────┐  │
   │  │ Model       │  │ RPC endpoint │  │ Marketplace        │  │
   │  │ providers   │  │ / indexer    │  │ extensions         │  │
   │  └─────────────┘  └──────────────┘  └────────────────────┘  │
   │  ┌─────────────┐  ┌──────────────┐  ┌────────────────────┐  │
   │  │ Chat        │  │ Webhook      │  │ Agent-to-agent     │  │
   │  │ transport   │  │ sources      │  │ connections        │  │
   │  └─────────────┘  └──────────────┘  └────────────────────┘  │
   └──────────────────────────────────────────────────────────────┘
                              │
                    ┌─────────┴─────────┐
                    │   ADMISSION       │  Rate limits, validation,
                    │   BOUNDARY        │  authentication, classification
                    └─────────┬─────────┘
                              │
   ┌──────────────────────────┴───────────────────────────────────┐
   │  POLKAGENT RUNTIME (trusted kernel)                          │
   │  ┌──────────┐  ┌────────────┐  ┌──────────────────────────┐  │
   │  │ Policy   │  │ Turn       │  │ Effect outbox            │  │
   │  │ engine   │  │ reducer    │  │ & workers                │  │
   │  └──────────┘  └────────────┘  └──────────────────────────┘  │
   │  ┌──────────┐  ┌────────────┐  ┌──────────────────────────┐  │
   │  │ Artifact │  │ Run event  │  │ Projection               │  │
   │  │ store    │  │ log        │  │ service                  │  │
   │  └──────────┘  └────────────┘  └──────────────────────────┘  │
   └──────────────────────────┬───────────────────────────────────┘
                              │
                    ┌─────────┴─────────┐
                    │   SIGNER          │  Key isolation boundary
                    │   BOUNDARY        │  Keys never cross this line
                    └─────────┬─────────┘
                              │
   ┌──────────────────────────┴───────────────────────────────────┐
   │  SIGNER (isolated)                                           │
   │  Hardware wallet │ External wallet │ KMS │ Local keystore    │
   └──────────────────────────────────────────────────────────────┘
```

### 14.2 Attack vectors and mitigations

| Attack vector | Description | Mitigation |
|---|---|---|
| **Prompt injection** | Malicious content in retrieved web pages, chat messages, code, or memory manipulates model behavior | Content classification, input validation, policy enforcement outside model control, model output is never authorization. See §14.4 for quantitative risk data and playbook. |
| **Confused deputy** | An agent is tricked into performing an action that looks legitimate but targets the wrong recipient or performs the wrong operation | Canonical payload binding, independent decode verification, metadata pinning, explicit approval of exact bytes. The policy gate + human approval step is the confused-deputy firewall: even a fully injected model cannot move value without a deterministic policy allow and (for high-value) explicit human sign-off on the exact decoded bytes. See §14.4 for emergency response. |
| **Key theft** | An attacker gains access to signing keys | Key isolation invariant, separate signer process, encrypted storage, hardware signers, key rotation |
| **Stale metadata** | Runtime upgrades change the meaning of a call after intent creation | Metadata hash pinning, stale-metadata gate, re-validation on drift |
| **Replay attack** | A previously valid signing request is resubmitted | Unique request IDs, nonces, expiry timestamps, on-chain nonce verification |
| **Homoglyph/lookalike recipient** | An attacker substitutes a visually similar address | Address comparison gates, known-address warnings, full raw key display |
| **Batch call hiding** | A dangerous inner call (e.g., `proxy.addProxy`) is hidden inside a batch | Recursive batch flattening, call-type flagging in approval cards |
| **Proxy escalation** | An agent adds a proxy to an account, gaining broader access | Proxy operations flagged as high-risk, require elevated approval |
| **Extension supply chain** | A malicious marketplace extension exfiltrates data or escalates privilege | Capability intersection, sandbox enforcement, signed manifests, revocation |
| **Transport impersonation** | An attacker sends messages pretending to be the operator | Transport authentication, signed envelopes, challenge-response |
| **Cloud tenant escape** | One tenant's agent accesses another tenant's data | Database isolation, encryption key separation, storage namespace enforcement |
| **Signer substitution** | The signer is replaced with a malicious one | Signer identity verification, configuration pinning, startup health checks |
| **Fee manipulation** | An attacker manipulates fee estimates to cause overpayment or transaction failure | Independent fee estimation, fee bounds in policy, simulation verification |
| **Reorg/finality confusion** | An agent treats an unfinalized transaction as final | Explicit finality states, no conflation of inclusion with finality, unknown state handling |

### 14.3 Trust assumptions

The Polkagent security model assumes:

| Component | Assumed trustworthy | Assumed potentially adversarial |
|---|---|---|
| Polkagent core runtime | Yes (after review) | — |
| Policy engine | Yes (deterministic, tested) | — |
| Signer implementation | Yes (isolated, reviewed) | — |
| Local filesystem (single user) | Yes | — |
| Model provider responses | — | Yes (may hallucinate, leak, or manipulate) |
| Chat/web user input | — | Yes (may contain injection attempts) |
| RPC endpoint data | — | Yes (may be stale, forked, or compromised) |
| Marketplace extensions | — | Yes (may be malicious or compromised) |
| Retrieved web content | — | Yes (may contain prompt injection) |
| Cloud co-tenants | — | Yes (must be isolated) |
| Memory/knowledge store entries | — | Yes (may be poisoned or stale) |

### 14.4 Prompt injection and confused-deputy playbook

**Quantitative risk context (verified fact):** The Gray Swan indirect
prompt-injection benchmark, reported in Anthropic's Claude Opus 4.5 system
card (November 2025), records the following attack-success rates in agentic
settings:

| Attempts per run | Opus 4.5 (most resistant tested) | Gemini 3 Pro / GPT-5.1 |
|---|---|---|
| 1 | 4.7% | up to ~92% |
| 10 | 33.6% | up to ~92% |
| 100 | 63.0% | up to ~92% |

Even the most resistant tested model fails on ~5% of single attempts and on
nearly two-thirds of 100-attempt campaigns. This means **model-level resistance
alone is not a viable defense**. The architecture must assume injection
succeeds and contain the blast radius through controls external to the model.

**Defense-in-depth posture:**

1. **Treat all tool output and retrieved content as untrusted data, never
   commands.** Tool results, web-retrieved pages, memory store entries, and
   agent-to-agent messages are inputs to reasoning; they cannot authorize,
   widen, or modify grants.
2. **The policy gate is the confused-deputy firewall.** An injected model
   that produces a malicious proposed action still cannot execute it: the
   deterministic Cedar policy evaluates the proposed action against the current
   grant, and human approval is required for value-moving effects. A model
   cannot generate text that bypasses the policy gate.
3. **Approval cards show canonical decoded data, not model-narrated text.**
   The human approval step presents the exact decoded call bytes from the
   signer payload, not a model-authored description of what will happen. This
   breaks the injection path where the model narrates a benign description for
   a malicious payload.
4. **Injection detection as a policy input.** A dedicated injection-detection
   stage (heuristic or model-based, running outside the main harness) may flag
   suspicious patterns in tool output. A flag raises the policy threshold
   (e.g., requires human approval even for actions that would otherwise be
   policy-autonomous).

**Emergency response sequence (in priority order):**

1. **Revoke proxy.** A single on-chain `proxy.removeProxy` transaction
   (requiring only the proxied account's signature) immediately removes the
   agent's ability to submit any on-chain action. This is the fastest
   containment action for funded agent accounts.
2. **Rotate hot keys.** Invalidate and rotate any API keys, session tokens,
   or local keystore credentials associated with the compromised agent.
3. **Freeze budgets.** Set the agent's mandate budget to zero or revoke the
   mandate entirely, blocking autonomous operation even if key rotation is
   delayed.
4. **Preserve audit evidence.** Do not delete logs or event streams; preserve
   the full run event log for post-incident analysis.
5. **Notify operator.** Emergency notification targets (configured in
   `EmergencyControlConfig`) receive immediate alerts on proxy revocation,
   key rotation, and budget freeze events.

---

## 15. Security invariants

These properties must hold at all times, regardless of configuration,
deployment topology, autonomy level, or extension installation:

### 15.1 Key isolation invariants

| ID | Invariant | Verification method |
|---|---|---|
| INV-SIGN-01 | Models never see raw keys | Static analysis of Signer trait; runtime classification enforcement; red-team testing |
| INV-SIGN-02 | SecretForbidden data never reaches logs, artifacts, or projections | Classification gate on every write path; audit sampling |
| INV-SIGN-03 | Signer only acts on authorized, canonical payloads | Signing request validation; policy evidence binding; expiry enforcement |

### 15.2 Policy invariants

| ID | Invariant | Verification method |
|---|---|---|
| INV-POLICY-01 | Effective permission is the intersection of all policies | Property-based testing with random policy combinations |
| INV-POLICY-02 | Policy evaluation is deterministic | Same-input-same-output tests across restarts |
| INV-POLICY-03 | New agents start with no effect grants | Default configuration tests; no-grant-no-effect verification |
| INV-POLICY-04 | Model output cannot modify policy | Boundary tests proving model responses are data, not commands |
| INV-POLICY-05 | Expired grants are denied immediately | Time-manipulation tests with expired grants |

### 15.3 Integrity invariants

| ID | Invariant | Verification method |
|---|---|---|
| INV-INT-01 | A restart never duplicates an external effect | Kill/restart tests during signing, broadcast, and finality |
| INV-INT-02 | An effect records authorization evidence before execution | Database transaction tests; crash recovery verification |
| INV-INT-03 | Approval cards show canonical data, not model-authored content | UI rendering tests with adversarial model output |
| INV-INT-04 | Unknown outcome is never silently treated as success or failure | State machine tests for timeout, disconnect, and ambiguous responses |

### 15.4 Isolation invariants

| ID | Invariant | Verification method |
|---|---|---|
| INV-ISO-01 | Tenant data is isolated at every layer | Cross-tenant access tests; storage namespace verification |
| INV-ISO-02 | Extensions cannot exceed their declared capabilities | Capability intersection tests with adversarial extensions |
| INV-ISO-03 | A delegate cannot exceed delegator authority | Delegation chain tests with nested narrowing |
| INV-ISO-04 | Reputation cannot widen grants | Grant resolution tests with maximum reputation and minimum policy |

### 15.5 Availability invariants

| ID | Invariant | Verification method |
|---|---|---|
| INV-AVAIL-01 | Emergency pause stops signing within configured time | Pause drill under load |
| INV-AVAIL-02 | Control plane unavailability does not widen local grants | Disconnect tests with cached policy |
| INV-AVAIL-03 | Signer unavailability fails gracefully, never falls back silently | Health check and failover tests |

---

## 16. Autonomy levels and mandate configuration

Autonomy is a policy mode, not a hard-coded product ceiling. This section
specifies the complete autonomy ladder and the requirements for each level.

### 16.1 Autonomy ladder

| Level | Behavior | Signing | Default |
|---|---|---|---|
| **Observe** | Read, monitor, explain, alert | None (watch-only) | Yes, for new agents |
| **Prepare** | Produce plans, intents, simulations | None | After explicit enablement |
| **Per-action approval** | Each action requires human/quorum approval | External or hardware | After explicit enablement |
| **Session approval** | Pre-authorize a bounded set of actions for a time window | External or hardware | After explicit enablement |
| **Policy-autonomous** | Execute without per-action approval while deterministic policy permits | Any configured signer | After explicit mandate |
| **Fully autonomous** | Continuous operation with no routine human approval | Funded account or delegated | After explicit mandate + security review |

### 16.2 Mandate structure

For policy-autonomous and fully autonomous levels, a mandate must be
explicitly configured:

```rust
struct AutonomousMandate {
    /// Unique identifier.
    mandate_id: MandateId,
    /// Who created this mandate.
    issuer: PrincipalRef,
    /// Which agent this mandate authorizes.
    agent: AgentId,
    /// Which accounts/networks are in scope.
    scope: MandateScope,
    /// Budget constraints.
    budget: MandateBudget,
    /// Time bounds.
    valid_from: Timestamp,
    valid_until: Timestamp,
    /// Required evidence for each autonomous action.
    evidence_requirements: EvidenceRequirements,
    /// Circuit breaker configuration.
    circuit_breaker: CircuitBreakerConfig,
    /// Notification and audit requirements.
    notifications: MandateNotifications,
    /// Emergency control configuration.
    emergency_controls: EmergencyControlConfig,
    /// Who may modify or revoke this mandate.
    revocation_authority: Vec<PrincipalRef>,
    /// Issuer's signature over the mandate.
    signature: Signature,
}

struct MandateScope {
    /// Allowed networks.
    networks: Vec<NetworkId>,
    /// Allowed accounts.
    accounts: Vec<AccountId32>,
    /// Allowed action families (e.g., transfers, staking, governance).
    action_families: Vec<ActionFamily>,
    /// Allowed call filters.
    call_filters: Vec<CallFilter>,
    /// Allowed recipients.
    allowed_recipients: Option<Vec<AccountId32>>,
    /// Disallowed calls (always denied even if family matches).
    denied_calls: Vec<CallFilter>,
}

struct MandateBudget {
    /// Per-action limit.
    per_action: Option<Balance>,
    /// Rolling limit with window.
    rolling: Option<RollingLimit>,
    /// Lifetime limit.
    lifetime: Option<Balance>,
    /// Fee limit per action.
    max_fee: Option<Balance>,
}

struct CircuitBreakerConfig {
    /// Maximum consecutive failures before pause.
    max_consecutive_failures: u32,
    /// Maximum unknown-outcome actions before pause.
    max_unknown_outcomes: u32,
    /// Rate limit for actions.
    rate_limit: Option<RateLimit>,
    /// Anomaly detection rules.
    anomaly_rules: Vec<AnomalyRule>,
}
```

- REQ-MANDATE-01: No UI may represent "autonomous" as a vague toggle. Each
  mandate must make its scope, budget, time bounds, evidence requirements,
  circuit breakers, notifications, and revocation authority explicit and
  understandable.
- REQ-MANDATE-02: Creating a mandate requires at least the same authentication
  strength as the most sensitive operation it authorizes.
- REQ-MANDATE-03: Mandate modification requires the same or higher
  authentication strength as mandate creation.
- REQ-MANDATE-04: Every autonomous action must record the mandate ID and the
  specific policy evaluation that authorized it.

---

## 17. Acceptance criteria and verification checklist

### 17.1 Key isolation

| Test | Pass criteria |
|---|---|
| Static analysis of `Signer` trait implementations | No method returns or logs raw key material |
| `SecretForbidden` classification gate | Artifact store, log sink, projection, and model context reject SecretForbidden data |
| Crash during signing | Restart never exposes key material in recovery state |
| Extension boundary test | No extension API surface can request or receive key material |
| Red-team: model requests keys | Policy denial; no key material in any response path |

### 17.2 Policy enforcement

| Test | Pass criteria |
|---|---|
| Default new agent | Zero effect grants; only read and respond |
| Policy intersection | Random policy combination produces tightest constraint |
| Expired grant | Denial is immediate; no grace period effect |
| Model attempts policy modification | Policy unchanged; attempt recorded as security event |
| Reputation-maximum + policy-minimum | Grant is minimum (reputation adds nothing) |
| Delegation chain | Each level narrows; no widening at any depth |

### 17.3 Signer operation

| Test | Pass criteria |
|---|---|
| External wallet flow | Signature matches exact payload; wrong-payload signature rejected |
| Hardware wallet disconnect | Graceful failure; no fallback to software signer |
| Proxy call filter | Disallowed call type rejected before reaching signer |
| Multisig threshold | Action proceeds only when threshold reached |
| Funded account circuit breaker | Consecutive failure or budget exhaustion pauses signing |
| Emergency pause | Signing stops within configured time; in-flight completes or fails |

### 17.4 Authentication

| Test | Pass criteria |
|---|---|
| API key rotation | Old key rejected; new key accepted; no service interruption |
| Session expiry | Expired session denied; no stale-session attacks |
| Agent-to-agent authentication | Unsigned or wrong-key messages rejected |
| Transport impersonation | Forged sender identity detected and rejected |

### 17.5 Privacy and isolation

| Test | Pass criteria |
|---|---|
| Data deletion | All user data, caches, and derived indexes removed |
| Tenant isolation | Cross-tenant database, storage, and cache access impossible |
| Identity cache TTL | Stale identity data purged on schedule |
| SecretForbidden in error path | Exception/error messages never contain key material |

### 17.6 Threat scenario tests

| Scenario | Expected outcome |
|---|---|
| Prompt injection in retrieved content | Policy enforcement prevents unauthorized action; injection flagged |
| Batch call hiding proxy.addProxy | Recursive call flattening surfaces hidden call; flagged as high-risk |
| Stale metadata after runtime upgrade | Metadata hash gate denies signing; fresh evidence required |
| Homoglyph recipient address | Address comparison gate warns; full raw key shown |
| Signer substitution attempt | Startup verification detects mismatch; operation denied |
| Control plane disconnection | Local cached policy enforced; no grant widening |
| Extension data exfiltration | Capability intersection blocks unauthorized network access |
| Replay of expired signing request | Request ID and expiry check reject replay |

### 17.7 Mandate and autonomy tests

| Test | Pass criteria |
|---|---|
| Mandate creation | Requires authentication at least as strong as authorized operations |
| Mandate scope enforcement | Actions outside mandate scope denied |
| Mandate budget enforcement | Over-budget actions denied |
| Mandate expiry | Expired mandate denied; no grace period |
| Circuit breaker trigger | Consecutive failures pause autonomous operation |
| Emergency revocation | Mandate revoked; all pending operations cancelled or completed |
| Mandate audit trail | Every autonomous action traceable to specific mandate and policy evaluation |

---

## 18. Phasing and maturity

| Component | Phase | Prerequisites |
|---|---|---|
| Watch-only signer | v1 | Core runtime |
| External wallet signer | v1 | Payload export, wallet protocol test vectors |
| Hardware wallet (Ledger, Vault) | v1 | Device interaction testing, QR flow testing |
| People Chain identity (read-only) | v1 | Chain client, identity resolution cache |
| Agent card (local) | v1 | Transport key, card signing |
| Policy engine (core stages) | v1 | Grant types, resource selectors |
| CLI/API authentication | v1 | Local config, API key management |
| Proxy/multisig signer | v2 | Proxy type verification, multisig state tracking |
| Local encrypted keystore | v2 | Encryption/zeroization testing, security review |
| Organizational signer | v2 | Org identity, org policy service |
| Web/mobile authentication | v2 | OAuth2/OIDC integration, biometric support |
| Agent-to-agent authentication | v2 | Mutual key verification, capability negotiation |
| KMS/HSM signer | v2 | Cloud provider integration, audit trail integration |
| Agent card (published) | v2 | Registry, discovery, verification |
| Verifiable credentials | v2 | VC issuance/verification, schema registry |
| Social recovery | v2 | Recovery contact management, threshold signing |
| Delegation hierarchy | v2 | Org structure, delegation records |
| MPC signing | v3 | Independent security review, ceremony tooling |
| Programmable/smart accounts | v3 | Contract audit, interaction pattern testing |
| Funded agent accounts | v3 | Mandate system, circuit breakers, emergency controls, security review |
| Personhood-gated policies | v3 | Stable personhood API, privacy review |
| Autonomous mandate system | v3 | Full policy engine, budget tracking, notification system |

### 18.1 Staged implementation recommendations

The following staging is derived from the research synthesis for this domain:

**Do now (v1 blockers):**
- Keys-outside-model: enforce the opaque handle + `zeroize` pattern throughout;
  no key bytes reachable from agent or model code paths.
- Pure-proxy agent accounts with the Staking-Operator narrow proxy type as the
  standard template; time-delay proxies for accounts with higher value exposure.
- Cedar as the policy engine: embeddable, deny-by-default, schema-validated
  before deploy.
- Prompt injection and confused-deputy playbook (§14.4): detection stage,
  canonical approval cards, proxy-revocation as first emergency response.

**Validate next (v2, requires research or spike):**
- Programmatic People Chain judgement query for counterparty/agent attestation;
  sub-identity enrollment for agent fleets.
- Passkey/OIDC bridge for web and mobile authentication.

**Defer (v3 or later):**
- MPC signing for sr25519 / MPCaaS: requires independent security review.
- W3C VC/DID for agent capability attestation: awaiting ecosystem maturity.
- ZK-based reputation: no stable substrate primitive yet.

**Avoid (permanent constraints):**
- Any model code path that can reach raw key bytes or seed phrases.
- `Any`-type or unbounded proxy for agent accounts.
- Treating model output as authorization for on-chain actions.

---

## 19. Cross-document interface summary

This section records how this PRD connects to others so a reader does not
need to locate the other document to understand the interface.

### 19.1 Effect lifecycle (PRD-03)

The signer operates within the effect lifecycle. The relevant states are:

```text
EffectIntent::RequestSignature
  -> EffectAttempt (worker claims the intent, leases the signer)
  -> sign() called on Signer trait
  -> EffectOutcome::Signed | Refused | Unavailable | Expired
```

The signing request is an effect like any other. It has an idempotency key,
a lease, a retry policy (limited: most signing refusals should not be retried
without human intervention), and a terminal outcome.

### 19.2 Chain evidence (PRD-05)

The signer receives a `CanonicalPayload` that was constructed from:

1. An intent (PRD-03) created from user request or trigger.
2. Chain profile resolution (PRD-05) binding to network, genesis, and metadata.
3. Call decode against pinned metadata (PRD-05).
4. Simulation evidence where supported (PRD-05).
5. Policy evaluation (this PRD, section 8).
6. Authorization decision (this PRD, section 16).

### 19.3 Payment effects (PRD-08)

Payments are a specific category of chain effects. This PRD provides the
signer and policy infrastructure; PRD-08 defines the payment-specific intent
types, reconciliation, and accounting.

### 19.4 Tenant identity (PRD-11)

Multi-tenant deployments use the identity model from this PRD. Organization
identity, service accounts, and tenant-scoped policy are the foundation for
the isolation described in PRD-11.

### 19.5 Extension capabilities (PRD-12)

Marketplace extensions declare capabilities in their manifests. The capability
intersection mechanism in this PRD (section 8.3) determines what an installed
extension may actually do within a specific deployment, workspace, and run.

---

## Appendix A. Requirement index

All requirements in this PRD use the format `REQ-{AREA}-{NN}`.

| Area | Count | Range |
|---|---|---|
| ACCT (accounts) | 8 | REQ-ACCT-01 through REQ-ACCT-08 |
| IDENT (identity) | 8 | REQ-IDENT-01 through REQ-IDENT-08 |
| CARD (agent cards) | 6 | REQ-CARD-01 through REQ-CARD-06 |
| CRED (credentials) | 4 | REQ-CRED-01 through REQ-CRED-04 |
| SIGN (signers) | 31 | REQ-SIGN-01 through REQ-SIGN-31 |
| POLICY (policy) | 3 | REQ-POLICY-01 through REQ-POLICY-03 |
| AUTH (authentication) | 11 | REQ-AUTH-01 through REQ-AUTH-11 |
| DELEG (delegation) | 4 | REQ-DELEG-01 through REQ-DELEG-04 |
| REP (reputation) | 4 | REQ-REP-01 through REQ-REP-04 |
| PRIV (privacy) | 12 | REQ-PRIV-01 through REQ-PRIV-12 |
| REVOKE (revocation) | 13 | REQ-REVOKE-01 through REQ-REVOKE-13 |
| MANDATE (mandates) | 4 | REQ-MANDATE-01 through REQ-MANDATE-04 |

## Appendix B. Invariant index

| ID | Summary | Section |
|---|---|---|
| INV-SIGN-01 | Models never see raw keys | 7.1 |
| INV-SIGN-02 | SecretForbidden never reaches logs/artifacts/projections | 15.1 |
| INV-SIGN-03 | Signer only acts on authorized canonical payloads | 15.1 |
| INV-POLICY-01 | Effective permission is intersection of all policies | 8.3 |
| INV-POLICY-02 | Policy evaluation is deterministic | 8.3 |
| INV-POLICY-03 | New agents start with no effect grants | 8.5 |
| INV-POLICY-04 | Model output cannot modify policy | 15.2 |
| INV-POLICY-05 | Expired grants denied immediately | 15.2 |
| INV-INT-01 | Restart never duplicates external effect | 15.3 |
| INV-INT-02 | Effect records authorization before execution | 15.3 |
| INV-INT-03 | Approval cards show canonical data | 15.3 |
| INV-INT-04 | Unknown outcome never silently resolved | 15.3 |
| INV-ISO-01 | Tenant data isolated at every layer | 15.4 |
| INV-ISO-02 | Extensions cannot exceed declared capabilities | 15.4 |
| INV-ISO-03 | Delegate cannot exceed delegator authority | 15.4 |
| INV-ISO-04 | Reputation cannot widen grants | 15.4 |
| INV-AVAIL-01 | Emergency pause stops signing within configured time | 15.5 |
| INV-AVAIL-02 | Control plane unavailability does not widen grants | 15.5 |
| INV-AVAIL-03 | Signer unavailability fails gracefully | 15.5 |
| INV-DELEG-01 | Delegate cannot have more authority than delegator | 10.2 |
| INV-REP-01 | Reputation never authorizes | 11.1 |

## Appendix C. Glossary of Polkadot-specific terms

| Term | Definition |
|---|---|
| **SS58** | Substrate's base-58 address encoding format, including a network prefix and checksum. |
| **AccountId32** | A 32-byte public key used as the canonical account identifier in Substrate chains. |
| **Sr25519** | Schnorr signature scheme over Ristretto255, the default key type for Polkadot accounts. |
| **Ed25519** | Edwards-curve Digital Signature Algorithm over Curve25519. |
| **ECDSA/secp256k1** | Elliptic Curve DSA used for Ethereum-compatible accounts. |
| **Existential deposit (ED)** | The minimum balance an account must maintain to avoid being reaped (deleted from chain state). |
| **People Chain** | A Polkadot system parachain hosting the identity pallet and registrar judgments. |
| **Registrar** | An on-chain entity that provides identity judgments (attestations) on People Chain. |
| **Proxy** | An on-chain delegation mechanism allowing one account to act on behalf of another within a type-restricted filter. |
| **Multisig** | An on-chain mechanism requiring M-of-N account signatures to authorize a call. |
| **Pure proxy** | A proxy account created without a pre-existing keypair; controlled entirely through its proxy relationships. |
| **Extrinsic** | A Polkadot transaction: a signed (or unsigned) call to a runtime pallet function. |
| **SCALE** | Simple Concatenated Aggregate Little-Endian: the binary encoding used by Polkadot SDK runtimes. |
| **Genesis hash** | The hash of a chain's genesis block, used as an immutable network identifier. |
| **Metadata** | The runtime's typed description of its callable pallets, arguments, events, and storage. Changes on runtime upgrade. |
| **Polkadot Vault** | An air-gapped mobile signing application (formerly Parity Signer) that uses QR codes for payload transfer. |
| **WalletConnect** | A cross-device wallet connection protocol. |
| **CheckMetadataHash** | A signed extension that can bind signing flows to metadata. |

## Appendix D. Evidence sources

| Source | Access date | Use in this PRD |
|---|---|---|
| [People Chain reference](https://docs.polkadot.com/reference/polkadot-hub/people-and-identity/) | 2026-07-29 | Identity pallet, registrar judgments |
| [Identity guide](https://wiki.polkadot.com/learn/learn-identity/) | 2026-07-29 | Identity fields, sub-identities |
| [Wallet landscape](https://docs.polkadot.com/develop/toolkit/integrations/storage/) | 2026-07-29 | Signer ecosystem |
| [Hub account mapping](https://docs.polkadot.com/smart-contracts/connect) | 2026-07-29 | EVM account mapping |
| [Staking operator proxy](https://docs.polkadot.com/node-infrastructure/run-a-validator/operational-tasks/staking-operator-proxy/) | 2026-07-29 | Proxy best practices |
| [Parity 2025 roundup](https://www.parity.io/blog/polkadot-roundup-2025) | 2026-07-29 | Personhood/individuality |
| [Subxt API](https://docs.rs/subxt/latest/subxt/) | 2026-07-29 | Rust chain client |
| `01-ESTABLISHED-BASELINE.md` sections 8, 11 | 2026-07-30 | Custody, identity baseline |

---

## APPENDIX A: GRANT RESOLUTION ALGORITHM

### A.1 Policy Evaluation Engine

Polkagent uses **Cedar** as its policy-as-code engine (see section 8.1). The
policy language is TOML for human-editable configuration and Cedar's own schema
language for machine-enforced validation. The evaluation model is ABAC
(Attribute-Based Access Control) with a strict deny-override semantic: any
deny in any applicable policy layer terminates evaluation immediately.

#### Policy File Format

Agent-level and organization-level policies are authored in TOML, then
compiled to Cedar policy sets at deploy time. The Cedar schema validator
rejects syntactically invalid or schema-mismatched policies before they
reach the runtime.

```toml
# ~/.config/polkagent/policies/agent-defi-bot.toml
# Policy revision is embedded in the file and hashed on load.
[meta]
revision = "2026-07-30T09:00:00Z"
agent_id = "defi-bot-v2"
policy_engine = "cedar"

# Each [[rule]] block maps to one Cedar policy statement.
[[rule]]
id = "allow-transfers-under-threshold"
effect = "permit"
principal = { type = "Agent", id = "defi-bot-v2" }
action = ["chain:Transfer", "chain:TransferKeepAlive"]
resource = { network = "polkadot", account_pattern = "*" }

[rule.conditions]
max_amount_dot = 10
allowed_recipients = ["5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"]
require_approval_above_dot = 5

[[rule]]
id = "deny-proxy-management"
effect = "forbid"
principal = { type = "Agent", id = "defi-bot-v2" }
action = ["chain:AddProxy", "chain:RemoveProxy", "chain:ProxyAnnounced"]
resource = "*"
# No conditions: unconditional deny.
```

#### Rule Evaluation Order

Cedar uses **deny-overrides** semantics, which is the safe default for
security gates:

1. All applicable `forbid` rules are evaluated first.
2. If any `forbid` matches, the decision is `Deny` regardless of any `permit`
   rules.
3. If no `forbid` matches, all applicable `permit` rules are evaluated.
4. If at least one `permit` matches, the decision is `Allow`.
5. If neither matches, the decision is `Deny` (deny-by-default).

This ordering means a narrow deny rule is never defeated by a broader permit
rule at the same or higher scope.

#### ABAC Model

Each policy evaluation receives a structured **context** object that carries
the attributes used in rule conditions:

```rust
/// Attributes available to Cedar policy rules during evaluation.
struct PolicyContext {
    /// The principal making the request.
    principal: PrincipalAttributes,
    /// The action being requested.
    action: ActionAttributes,
    /// The resource being acted upon.
    resource: ResourceAttributes,
    /// Environmental context.
    env: EnvironmentAttributes,
}

struct PrincipalAttributes {
    principal_id: String,
    principal_type: String,         // "Agent" | "User" | "Service"
    org_id: Option<String>,
    autonomy_level: String,
    reputation_score: Option<f64>,  // informational only; never authorization
    mandate_id: Option<String>,
    delegation_depth: u32,
}

struct ActionAttributes {
    action_id: String,              // e.g. "chain:Transfer"
    action_family: String,          // e.g. "transfer" | "staking" | "governance"
    estimated_value: Option<u128>,  // in planck
    network: String,
    call_hash: Option<String>,
}

struct ResourceAttributes {
    resource_type: String,          // "Account" | "Tool" | "Network" | "File"
    resource_id: String,
    is_watched: bool,
    classification: String,         // "Public" | "Private" | "Sensitive" | "SecretForbidden"
}

struct EnvironmentAttributes {
    current_timestamp: i64,
    active_grants: Vec<String>,     // grant IDs in scope
    session_id: Option<String>,
    injection_risk_score: Option<f64>,
    metadata_fresh: bool,
}
```

#### Policy Composition

Policy sets are composed in layers. Each layer is a Cedar policy set loaded
from a distinct source, and all sets are evaluated together against the unified
Cedar engine. The effective decision is always the intersection (deny-overrides
across layers):

```text
GlobalPolicySet        (platform invariants; never overrideable)
    + OrgPolicySet     (organization-level rules; override workspace)
    + WorkspacePolicySet (deployment/environment rules)
    + AgentPolicySet   (agent-specific grants)
    + MandatePolicySet (active mandate constraints; loaded per-request)
= ComposedPolicySet    (evaluated as one Cedar request)
```

Each policy set carries a `revision` digest. The composed digest is the hash
of all constituent revisions in layer order. This digest is bound to every
`ResolvedGrant` (see section 8.2).

#### Cache Invalidation on Policy Changes

Policy sets are loaded on startup and re-loaded on change signals:

- File-backed policies: `inotify`/`kqueue` watch on the policy directory.
  Any modification triggers an async reload and schema validation. If
  validation fails, the previous revision remains active and an error is
  logged.
- Remote-backed policies (org/cloud): a short-lived TTL cache (default: 60
  seconds) with forced invalidation on the `policy.updated` event from the
  control plane.
- In-flight evaluations that started before the reload complete against the
  old revision. The new revision applies to all evaluations started after the
  reload commit.
- `ResolvedGrant` objects carry their policy revision digest. If a grant's
  revision no longer matches the current composed revision, the grant is
  treated as expired and must be re-resolved.

---

### A.2 Grant Resolution Pseudocode

The grant resolution function takes a principal, action, resource, and context
and returns a `GrantDecision`. The full algorithm below includes all decision
paths, conflict resolution between overlapping grants, delegation chain
traversal, and time-based expiry.

```rust
/// The output of grant resolution.
enum GrantDecision {
    /// The request is permitted with the given resolved grant.
    Permit(ResolvedGrant),
    /// The request is denied.
    Deny(PolicyDenial),
    /// The request requires human or quorum approval before proceeding.
    RequireApproval(ApprovalRequirement),
    /// The request is deferred pending additional context (e.g. fresh metadata).
    Defer(DeferReason),
}

/// Resolve whether a principal may perform an action on a resource.
///
/// Called by the policy pipeline after all context is assembled.
/// This function is synchronous and deterministic: same inputs → same output.
fn resolve_grant(
    principal: &PrincipalRef,
    action: &ActionDescriptor,
    resource: &ResourceDescriptor,
    context: &PolicyContext,
    policy_store: &ComposedPolicySet,
    grant_store: &ActiveGrantStore,
    delegation_registry: &DelegationRegistry,
    clock: &dyn Clock,
) -> GrantDecision {

    // Step 1: reject expired context immediately.
    if context.env.current_timestamp > action.requested_at + MAX_CONTEXT_AGE_SECS {
        return GrantDecision::Deny(PolicyDenial {
            stage: "context-expiry".into(),
            reason: "request context is too old".into(),
            policy_revision: policy_store.revision(),
            constraint: format!("max_context_age={}s", MAX_CONTEXT_AGE_SECS),
        });
    }

    // Step 2: traverse delegation chain; collect all effective principals.
    // INV-DELEG-01: authority can only narrow through the chain.
    let effective_principals = match expand_delegation_chain(
        principal,
        delegation_registry,
        clock.now(),
    ) {
        Ok(chain) => chain,
        Err(ChainError::Expired(link)) => {
            return GrantDecision::Deny(PolicyDenial {
                stage: "delegation-chain".into(),
                reason: format!("delegation link expired: {:?}", link),
                policy_revision: policy_store.revision(),
                constraint: "delegation must be within validity period".into(),
            });
        }
        Err(ChainError::DepthExceeded) => {
            return GrantDecision::Deny(PolicyDenial {
                stage: "delegation-chain".into(),
                reason: "delegation depth limit exceeded".into(),
                policy_revision: policy_store.revision(),
                constraint: format!("max_depth={}", MAX_DELEGATION_DEPTH),
            });
        }
        Err(ChainError::AuthorityExceeded { link, detail }) => {
            // Invariant violation: a delegate claims more authority than delegator.
            // Record as a security event and deny.
            emit_security_event(SecurityEvent::DelegationAuthorityExceeded {
                link,
                detail: detail.clone(),
            });
            return GrantDecision::Deny(PolicyDenial {
                stage: "delegation-chain".into(),
                reason: format!("delegate exceeds delegator authority: {}", detail),
                policy_revision: policy_store.revision(),
                constraint: "INV-DELEG-01 violated".into(),
            });
        }
    };

    // Step 3: evaluate the composed Cedar policy set.
    // Cedar evaluation is deny-overrides; any forbid in any layer denies.
    let cedar_request = CedarRequest {
        principal: principal_to_cedar(&effective_principals),
        action: action_to_cedar(action),
        resource: resource_to_cedar(resource),
        context: context_to_cedar(context),
    };

    let cedar_response = policy_store.is_authorized(&cedar_request);

    match cedar_response.decision {
        CedarDecision::Deny => {
            return GrantDecision::Deny(PolicyDenial {
                stage: "cedar-evaluation".into(),
                reason: cedar_response.reasons.join("; "),
                policy_revision: policy_store.revision(),
                constraint: cedar_response.errors.join("; "),
            });
        }
        CedarDecision::Allow => {
            // Continue to quantitative limit checks.
        }
    }

    // Step 4: check for active, non-expired grants from the grant store.
    // A pre-issued grant (from a prior approval or mandate) may already cover
    // this request. Use the most specific matching grant.
    let active_grant = grant_store.find_best_match(principal, action, resource, clock.now());

    // Step 5: if no active grant, determine approval requirement.
    let approval_requirement = if active_grant.is_none() {
        compute_approval_requirement(action, context, &cedar_response.annotations)
    } else {
        None
    };

    if let Some(req) = approval_requirement {
        return GrantDecision::RequireApproval(req);
    }

    // Step 6: for active grants, verify time bounds.
    if let Some(ref grant) = active_grant {
        if grant.expires_at <= clock.now() {
            // REQ-POLICY-02: expired grants denied immediately.
            grant_store.mark_expired(grant.grant_id);
            return GrantDecision::Deny(PolicyDenial {
                stage: "grant-expiry".into(),
                reason: format!("grant {} has expired", grant.grant_id),
                policy_revision: policy_store.revision(),
                constraint: format!("expired_at={}", grant.expires_at),
            });
        }

        // Step 7: verify the grant's policy revision matches current policy.
        if grant.policy_revision != policy_store.revision() {
            // Policy changed since grant was issued; must re-resolve.
            return GrantDecision::Defer(DeferReason::PolicyChanged {
                grant_revision: grant.policy_revision,
                current_revision: policy_store.revision(),
            });
        }
    }

    // Step 8: resolve quantitative limits.
    // Intersection: take the minimum of grant limits and active mandate limits.
    let limits = resolve_limit_intersection(
        active_grant.as_ref().map(|g| &g.limits),
        context.active_mandate_limits(),
        action,
    );

    // Step 9: check budget and rate limits against current usage.
    match check_budget_and_rate(principal, action, &limits, grant_store) {
        BudgetCheck::Ok => {}
        BudgetCheck::Exceeded { detail } => {
            return GrantDecision::Deny(PolicyDenial {
                stage: "budget-check".into(),
                reason: detail,
                policy_revision: policy_store.revision(),
                constraint: "budget or rate limit exceeded".into(),
            });
        }
    }

    // Step 10: produce the resolved grant.
    let resolved = ResolvedGrant {
        grant_id: GrantId::new(),
        subject: principal.into(),
        resource: resource.to_selector(),
        allowed_effects: cedar_response.allowed_effects(),
        limits,
        approvals: active_grant
            .map(|g| g.approvals.clone())
            .unwrap_or_default(),
        expires_at: compute_grant_expiry(action, &limits, clock.now()),
        policy_revision: policy_store.revision(),
        digest: Digest::default(), // filled below
    };

    let resolved = resolved.with_digest(); // compute and seal the digest
    grant_store.record(resolved.clone());

    GrantDecision::Permit(resolved)
}
```

#### Conflict Resolution Between Overlapping Grants

When multiple active grants match a request, `find_best_match` applies the
following resolution order:

1. **Scope specificity:** the grant with the narrowest matching resource
   selector wins (e.g., a grant for `polkadot/account/5Grw…` beats one for
   `polkadot/*`).
2. **Recency:** among grants of equal specificity, the most recently issued
   grant applies.
3. **Quantity intersection:** effective limits are always the minimum across
   all applicable grants (INV-POLICY-01).
4. **Denials override:** if any applicable grant is a denial record (e.g., an
   emergency freeze), the denial wins regardless of specificity or recency.

#### Delegation Chain Traversal

`expand_delegation_chain` walks the delegation graph from the immediate
principal upward to the root authority, collecting every `DelegationRecord`
in order. For each link:

- The link's `expires_at` must be in the future.
- The link's `granted` constraints must be a subset of the delegator's own
  active grants at the time the delegation was issued.
- The `delegation_depth` of the resulting chain must not exceed
  `max_depth` from the root record.
- If `sub_delegatable` is `false` on any link, traversal stops there; further
  sub-delegation from that point is not allowed.

The result is a list of `EffectivePrincipal` entries, each annotated with the
constraints that apply at that delegation level. The Cedar evaluation uses the
intersection of all constraints across the chain.

#### Time-Based Grant Expiry

Grant expiry is computed deterministically at resolution time:

```rust
fn compute_grant_expiry(
    action: &ActionDescriptor,
    limits: &GrantLimits,
    now: Timestamp,
) -> Timestamp {
    // Start from the minimum of: action deadline, mandate validity,
    // and configured maximum grant lifetime.
    let candidates = [
        limits.deadline,
        action.mandate_valid_until,
        Some(now + MAX_GRANT_LIFETIME),
    ];

    candidates
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(now + DEFAULT_GRANT_LIFETIME)
}

const MAX_GRANT_LIFETIME: Duration = Duration::from_secs(30 * 24 * 3600); // 30 days
const DEFAULT_GRANT_LIFETIME: Duration = Duration::from_secs(3600);        // 1 hour
```

REQ-POLICY-01 (no grant without expiry) is enforced by this function: it
always produces a finite timestamp. There is no code path that produces
`expires_at = None`.

---

### A.3 Gate Implementation

Gates are independent, stateless checks that run synchronously in the policy
pipeline. They receive a `PolicyRequest` and return `GateResult`. A gate is
NOT a substitute for Cedar policy evaluation; it is an additional enforcement
layer for checks that are easier to express imperatively than in Cedar's
declarative language.

#### Gate Trait

```rust
/// An independent, deterministic enforcement check.
///
/// Gates run after Cedar evaluation and before grant production.
/// Each gate is a pure function of its inputs; it does not mutate state.
#[async_trait]
pub trait Gate: Send + Sync + 'static {
    /// Unique identifier for this gate.
    fn gate_id(&self) -> &str;

    /// Human-readable description for audit records.
    fn description(&self) -> &str;

    /// Evaluate this gate against the request.
    async fn check(&self, request: &GateRequest) -> GateResult;
}

pub struct GateRequest {
    pub principal: PrincipalRef,
    pub action: ActionDescriptor,
    pub resource: ResourceDescriptor,
    pub context: PolicyContext,
    /// Accumulated Cedar evaluation annotations (e.g. allowed effects, risk flags).
    pub cedar_annotations: CedarAnnotations,
}

pub enum GateResult {
    /// Gate passes; include optional annotations for downstream gates.
    Allow { annotations: Vec<GateAnnotation> },
    /// Gate denies; evaluation stops immediately.
    Deny { reason: String, detail: String },
    /// Gate requires human approval before proceeding.
    Escalate { requirement: ApprovalRequirement },
}
```

#### Built-In Gates

**ApprovalGate** — enforces human or quorum approval for actions above a
configured risk threshold:

```rust
pub struct ApprovalGate {
    /// Actions that always require approval regardless of policy.
    always_require: Vec<ActionPattern>,
    /// Value threshold above which approval is required (in planck).
    value_threshold: Option<u128>,
    /// Risk score threshold from the injection-detection stage.
    injection_risk_threshold: Option<f64>,
    /// Approval timeout: how long to wait for a response.
    timeout: Duration,
    /// IPC channel to the TUI approval modal.
    /// Mirrors Roko's ApprovalChannel (crates/roko-cli/src/tui/approval_ipc.rs).
    approval_tx: mpsc::Sender<ApprovalRequest>,
}

impl ApprovalGate {
    async fn check(&self, request: &GateRequest) -> GateResult {
        let needs_approval =
            self.always_require.iter().any(|p| p.matches(&request.action))
            || request.action.estimated_value
                .zip(self.value_threshold)
                .map(|(v, t)| v > t)
                .unwrap_or(false)
            || request.context.env.injection_risk_score
                .zip(self.injection_risk_threshold)
                .map(|(s, t)| s > t)
                .unwrap_or(false);

        if !needs_approval {
            return GateResult::Allow { annotations: vec![] };
        }

        GateResult::Escalate {
            requirement: ApprovalRequirement {
                gate_id: self.gate_id().into(),
                reason: "action requires explicit human approval".into(),
                display_summary: request.action.display_summary.clone(),
                timeout: self.timeout,
                quorum: ApprovalQuorum::Single, // configurable
            },
        }
    }
}
```

**BudgetGate** — checks real-time spend against rolling and lifetime limits:

```rust
pub struct BudgetGate {
    /// Budget tracking store (reads committed spends from the event log).
    budget_store: Arc<dyn BudgetStore>,
}

impl BudgetGate {
    async fn check(&self, request: &GateRequest) -> GateResult {
        let Some(value) = request.action.estimated_value else {
            return GateResult::Allow { annotations: vec![] };
        };

        let usage = self.budget_store
            .current_usage(&request.principal, &request.context)
            .await;

        if let Some(rolling) = usage.rolling_limit {
            if usage.rolling_spent + value > rolling {
                return GateResult::Deny {
                    reason: "rolling budget exceeded".into(),
                    detail: format!(
                        "spent={} limit={} window={}",
                        usage.rolling_spent, rolling, usage.rolling_window
                    ),
                };
            }
        }

        if let Some(lifetime) = usage.lifetime_limit {
            if usage.lifetime_spent + value > lifetime {
                return GateResult::Deny {
                    reason: "lifetime budget exceeded".into(),
                    detail: format!(
                        "spent={} limit={}",
                        usage.lifetime_spent, lifetime
                    ),
                };
            }
        }

        GateResult::Allow { annotations: vec![] }
    }
}
```

**AllowlistGate** — checks recipient accounts against a configured allowlist:

```rust
pub struct AllowlistGate {
    /// Set of allowed recipient AccountId32 values.
    allowed: HashSet<AccountId32>,
    /// Whether to deny unknown recipients or only warn.
    mode: AllowlistMode,
}

pub enum AllowlistMode {
    /// Unknown recipient is denied.
    Strict,
    /// Unknown recipient escalates to approval.
    Escalate,
    /// Unknown recipient is annotated but allowed.
    Warn,
}
```

**RateLimitGate** — enforces per-principal, per-action rate limits using a
token bucket algorithm:

```rust
pub struct RateLimitGate {
    /// Token bucket store (keyed by principal + action family).
    buckets: Arc<dyn TokenBucketStore>,
    /// Default rate limit applied when no specific limit matches.
    default_rate: RateLimit,
    /// Per-action-family overrides.
    overrides: HashMap<ActionFamily, RateLimit>,
}

pub struct RateLimit {
    /// Maximum tokens (requests) in the bucket.
    capacity: u64,
    /// Tokens replenished per second.
    refill_rate: f64,
}
```

#### Gate Composition

Gates are composed using three combinators. Composition is evaluated at
startup and the resulting tree is immutable for the lifetime of the pipeline
stage:

```rust
pub enum ComposedGate {
    /// Single gate.
    Leaf(Box<dyn Gate>),
    /// All child gates must allow (short-circuits on first deny).
    And(Vec<ComposedGate>),
    /// At least one child gate must allow (short-circuits on first allow).
    Or(Vec<ComposedGate>),
    /// Gates run in declared order; result of each is passed as context to the next.
    Sequential(Vec<ComposedGate>),
}

impl ComposedGate {
    pub async fn check(&self, request: &GateRequest) -> GateResult {
        match self {
            ComposedGate::Leaf(g) => g.check(request).await,

            ComposedGate::And(gates) => {
                let mut annotations = vec![];
                for gate in gates {
                    match gate.check(request).await {
                        GateResult::Allow { annotations: a } => annotations.extend(a),
                        deny_or_escalate => return deny_or_escalate,
                    }
                }
                GateResult::Allow { annotations }
            }

            ComposedGate::Or(gates) => {
                let mut last = GateResult::Deny {
                    reason: "no gate allowed".into(),
                    detail: String::new(),
                };
                for gate in gates {
                    match gate.check(request).await {
                        GateResult::Allow { annotations } => {
                            return GateResult::Allow { annotations }
                        }
                        other => last = other,
                    }
                }
                last
            }

            ComposedGate::Sequential(gates) => {
                let mut accumulated = vec![];
                for gate in gates {
                    let mut req = request.clone();
                    req.cedar_annotations.extend_with(&accumulated);
                    match gate.check(&req).await {
                        GateResult::Allow { annotations: a } => accumulated.extend(a),
                        other => return other,
                    }
                }
                GateResult::Allow { annotations: accumulated }
            }
        }
    }
}
```

#### Gate Result Caching and Invalidation

Gate results for identical `(principal, action, resource, context)` tuples
may be cached for a short TTL to avoid redundant computation within a single
run. Cache entries are keyed by the deterministic hash of the `GateRequest`.
Cache invalidation rules:

- Expiry: maximum cache TTL is 5 seconds for rate-sensitive gates; 0 seconds
  (no caching) for `ApprovalGate` (every approval must be fresh).
- Policy revision change: any policy reload flushes the entire gate cache.
- Budget event: any committed spend event flushes `BudgetGate` cache entries
  for the affected principal.
- Emergency pause: flush entire gate cache immediately.

---

## APPENDIX B: SECRET MANAGEMENT

### B.1 Secret Store Interface

All secrets are accessed through the `SecretStore` trait. The agent runtime
holds only `SecretRef` handles. Raw secret material is never held in agent
memory; it is resolved transiently inside the store implementation and
zeroized after use.

```rust
/// Opaque handle to a stored secret. Safe to log, clone, and pass across
/// process boundaries. Never contains raw secret material.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretRef {
    /// Globally unique identifier for this secret.
    pub secret_id: SecretId,
    /// Classification of the secret (always SecretForbidden for key material).
    pub classification: DataClassification,
    /// Human-readable label (displayed in audit logs, never the value).
    pub label: String,
}

/// Classification of secret material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataClassification {
    Public,
    Private,
    Sensitive,
    /// Key material, seed phrases, passwords. Must never reach model, logs,
    /// artifacts, or projections.
    SecretForbidden,
}

/// The isolated secret storage boundary.
///
/// Implementations may back this with the OS keychain, an encrypted file,
/// an HSM, or a cloud KMS. The interface is identical; the agent runtime
/// sees only this trait.
#[async_trait]
pub trait SecretStore: Send + Sync + 'static {
    /// Retrieve and use a secret value within a closure.
    ///
    /// The closure receives the raw bytes transiently. The store
    /// implementation zeroizes the buffer after the closure returns.
    /// The agent runtime never holds the raw value outside the closure.
    async fn with_secret<F, R>(
        &self,
        secret_ref: &SecretRef,
        f: F,
    ) -> Result<R, SecretStoreError>
    where
        F: FnOnce(&[u8]) -> R + Send,
        R: Send;

    /// Store a new secret and return its handle.
    ///
    /// The caller provides raw material exactly once. After this call,
    /// the raw material must be zeroized at the call site.
    async fn store(
        &self,
        label: &str,
        classification: DataClassification,
        material: &mut [u8], // zeroized by the store on return
    ) -> Result<SecretRef, SecretStoreError>;

    /// Rotate a secret: replace the material and return the same handle.
    async fn rotate(
        &self,
        secret_ref: &SecretRef,
        new_material: &mut [u8],
    ) -> Result<(), SecretStoreError>;

    /// Revoke a secret. After this call, `with_secret` returns an error.
    async fn revoke(&self, secret_ref: &SecretRef) -> Result<(), SecretStoreError>;

    /// List secrets accessible to the caller (labels and handles only; no material).
    async fn list(&self) -> Result<Vec<SecretRef>, SecretStoreError>;

    /// Check store health and availability.
    async fn health(&self) -> SecretStoreHealth;
}
```

#### Implementations

**OsKeychainStore** — delegates to the operating system's secure credential
storage. On macOS: Keychain Services. On Linux: libsecret / Secret Service API.
On Windows: Credential Manager. This is the default for local deployments.

```rust
pub struct OsKeychainStore {
    /// Service name used to namespace keys in the OS keychain.
    service: String,
    /// Audit log sink for every access.
    audit: Arc<dyn AuditLog>,
}
```

**EncryptedFileStore** — stores secrets in an Argon2id-encrypted file on disk.
The decryption key is derived from a user-provided passphrase or a
hardware-backed key. Used when OS keychain is unavailable.

```rust
pub struct EncryptedFileStore {
    /// Path to the encrypted store file.
    path: PathBuf,
    /// KDF parameters for the Argon2id derivation.
    kdf_params: Argon2Params,
    /// Audit log sink.
    audit: Arc<dyn AuditLog>,
}
```

File path: `~/.local/share/polkagent/secrets.enc` (XDG base dir on Linux) or
`~/Library/Application Support/Polkagent/secrets.enc` (macOS).

**HsmStore** — delegates signing and secret derivation to a hardware security
module (PKCS#11 interface). Used in enterprise and validator deployments.

```rust
pub struct HsmStore {
    /// PKCS#11 slot identifier.
    slot: u64,
    /// Library path for the PKCS#11 provider.
    library_path: PathBuf,
    /// PIN for the HSM slot (itself stored as a SecretRef from another store).
    pin_ref: SecretRef,
    pub audit: Arc<dyn AuditLog>,
}
```

**CloudKmsStore** — delegates to a cloud KMS (AWS KMS, GCP Cloud KMS, Azure
Key Vault). The cloud KMS never exposes raw key material; it performs signing
operations internally. The store uses the KMS API to wrap/unwrap data keys.

```rust
pub struct CloudKmsStore {
    /// Cloud provider configuration (region, key ARN/name, credentials).
    config: CloudKmsConfig,
    /// HTTP client with retry and timeout.
    client: Arc<dyn HttpClient>,
    /// Audit log sink (supplements the cloud provider's own audit trail).
    audit: Arc<dyn AuditLog>,
}
```

#### Secret Classification and Access Control

Every `SecretRef` carries a `DataClassification`. Access is controlled by:

1. **Classification gate:** any attempt to pass a `SecretForbidden` value into
   model context, logs, artifacts, or projections is rejected at the data
   boundary.
2. **Principal access control:** each secret has an access control list (ACL)
   stored alongside the handle in the `SecretStore`. The `with_secret` method
   checks the caller's principal against the ACL before resolving.
3. **Scope pinning:** a secret created for use with a specific agent or signer
   is pinned to that scope. Cross-scope access is denied.

#### Audit Logging for Secret Access

Every call to `with_secret`, `store`, `rotate`, and `revoke` produces an audit
record:

```rust
pub struct SecretAccessAuditRecord {
    pub timestamp: Timestamp,
    pub principal: PrincipalRef,
    pub secret_id: SecretId,
    pub secret_label: String,          // never the value
    pub operation: SecretOperation,    // Access | Store | Rotate | Revoke
    pub outcome: AuditOutcome,         // Success | Denied | Error
    pub caller_context: String,        // e.g. "signer:local-keystore"
}
```

The audit record is written to the audit log before the operation is performed.
If the write fails, the operation is aborted. This ensures that every secret
access has a pre-operation audit entry.

#### Secret Rotation Procedures

1. **Generate new material** inside the store boundary (or import from an
   external source via a zeroized buffer).
2. **Call `rotate`** on the `SecretStore` with the new material. The store
   atomically replaces the old material and records the rotation in the audit
   log.
3. **Update all dependent references:** any `SecretRef` pointing to the old
   label continues to work because `rotate` updates the stored material in
   place. No handle changes are required for in-place rotation.
4. **Verify:** the caller must confirm the new material is accessible via
   `with_secret` before declaring rotation complete.
5. **Revoke the old path** (for key rollover scenarios where a new key pair
   is being substituted): after all dependent systems are updated, call
   `revoke` on the old handle.

REQ-REVOKE-01 and REQ-REVOKE-03 apply: rotation must not cause service
interruption, and the rotation event must appear in the audit trail with
old and new identifiers (never raw material).

---

### B.2 Signer Isolation

The local encrypted keystore signer (section 7.3.5) and any signer that
holds raw key material must run in a **separate OS process** from the agent
runtime. This is the process-level isolation layer that enforces
INV-SIGN-01 (models never see raw keys).

#### Process-Level Isolation

```text
┌─────────────────────────────────────────┐
│  Polkagent agent process                │
│  - Model harness                        │
│  - Policy engine                        │
│  - Effect outbox                        │
│  - Holds: SecretRef (opaque handle)     │
│  - Does NOT hold: raw key bytes         │
└──────────────┬──────────────────────────┘
               │  Unix domain socket (IPC)
               │  Protocol: length-prefixed CBOR
               │  Auth: process-local nonce issued at signer startup
               ▼
┌─────────────────────────────────────────┐
│  Polkagent signer process (isolated)    │
│  - LocalKeystoreSigner                  │
│  - Holds: encrypted keystore on disk    │
│  - Decrypts: in zeroize-on-drop buffer  │
│  - Accepts: SigningRequest via IPC      │
│  - Returns: Signature | Refused         │
└─────────────────────────────────────────┘
```

The signer process is spawned by the agent process at startup with a
restricted environment: no network access, no filesystem access outside its
keystore path, reduced system call surface via seccomp-bpf (Linux) or
sandbox-exec (macOS).

#### IPC Protocol

The IPC protocol between the agent process and the signer process mirrors
the pattern used in Roko's approval IPC
(`/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/approval_ipc.rs`):
a bounded channel with typed request/response structs and a oneshot reply
channel per request.

```rust
/// Sent from agent process to signer process over the Unix domain socket.
///
/// Mirrors the structure of Roko's ApprovalRequest: a typed payload with
/// an embedded reply-channel identifier so the signer can correlate responses.
pub struct IpcSigningRequest {
    /// Unique request identifier. The signer echoes this in the response.
    pub request_id: SigningRequestId,
    /// The canonical payload bytes to sign.
    pub payload: CanonicalPayload,
    /// The account that should sign.
    pub signing_account: AccountId32,
    /// The key type to use.
    pub key_type: KeyType,
    /// The network this signature targets.
    pub network: NetworkId,
    /// The authorization decision that approved this signing.
    pub authorization: AuthorizationDecision,
    /// Human-readable summary for the signer's own logging.
    pub display_summary: DisplaySummary,
    /// When this request expires.
    pub expires_at: Timestamp,
    /// Policy revision at time of issuance.
    pub policy_revision: Digest,
}

/// Sent from signer process back to the agent process.
pub struct IpcSigningResponse {
    /// Echoed request identifier.
    pub request_id: SigningRequestId,
    /// The result.
    pub result: SigningResult,
}

/// Channel management: each side holds one end of a bounded mpsc.
pub struct SignerIpcChannel {
    pub request_tx: mpsc::Sender<IpcSigningRequest>,
    pub response_rx: mpsc::Receiver<IpcSigningResponse>,
    /// Buffer size matches Roko's ApprovalChannel convention.
    pub buffer: usize,
}

impl SignerIpcChannel {
    pub fn new(buffer: usize) -> (Self, SignerIpcServerSide) {
        let (req_tx, req_rx) = mpsc::channel(buffer);
        let (resp_tx, resp_rx) = mpsc::channel(buffer);
        (
            SignerIpcChannel { request_tx: req_tx, response_rx: resp_rx, buffer },
            SignerIpcServerSide { request_rx: req_rx, response_tx: resp_tx },
        )
    }
}
```

#### Payload Verification Before Signing

The signer process performs its own independent verification before signing.
It does not trust that the policy engine has already validated the payload:

```rust
/// Signer-side verification before any key material is touched.
fn verify_payload_before_signing(
    request: &IpcSigningRequest,
    local_policy: &SignerPolicy,
    clock: &dyn Clock,
) -> Result<(), SignerRefusalReason> {
    // 1. Check request has not expired.
    if request.expires_at <= clock.now() {
        return Err(SignerRefusalReason::PayloadRejected {
            details: "signing request expired".into(),
        });
    }

    // 2. Verify authorization evidence is present and well-formed.
    if !request.authorization.is_valid() {
        return Err(SignerRefusalReason::PolicyDenied {
            details: "authorization evidence missing or malformed".into(),
        });
    }

    // 3. Verify the signing account is known to this signer.
    if !local_policy.known_accounts.contains(&request.signing_account) {
        return Err(SignerRefusalReason::AccountNotFound);
    }

    // 4. Verify the key type is supported.
    if !local_policy.supported_key_types.contains(&request.key_type) {
        return Err(SignerRefusalReason::KeyTypeNotSupported);
    }

    // 5. Check the signer's own call filter (independent of agent-level policy).
    if let Some(filter) = &local_policy.call_filter {
        let call_type = decode_call_type(&request.payload)?;
        if filter.denies(&call_type) {
            return Err(SignerRefusalReason::PolicyDenied {
                details: format!("call type {:?} denied by signer policy", call_type),
            });
        }
    }

    Ok(())
}
```

#### Signing Request Logging and Auditing

Every signing attempt produces an audit record, regardless of outcome:

```rust
pub struct SigningAuditRecord {
    pub timestamp: Timestamp,
    pub request_id: SigningRequestId,
    pub principal: PrincipalRef,
    pub signing_account: AccountId32,
    pub key_type: KeyType,
    pub network: NetworkId,
    pub payload_hash: Sha256Digest,     // hash of payload bytes; not the raw bytes
    pub authorization_digest: Digest,   // digest of the authorization decision
    pub policy_revision: Digest,
    pub outcome: SigningAuditOutcome,
    pub signer_id: SignerId,
}

pub enum SigningAuditOutcome {
    Signed { signature_hash: Sha256Digest },
    Refused { reason: SignerRefusalReason },
    Expired,
    Error { detail: String },
}
```

The audit record is written to the audit log by the signer process (not the
agent process) before any key material is accessed. This ensures auditability
even if the agent process crashes or is compromised after the signing response
is returned.

---

## APPENDIX C: IMPLEMENTATION CHECKLIST

Tasks are grouped by domain area. Each task includes an acceptance criterion
and a phase label. Phase labels match section 18 of the main PRD.

### Identity Types (v1)

- [ ] **IMPL-IDENT-01** Define `AgentIdentity`, `AccountBinding`, `PersonIdentity`,
  `OrganizationIdentity`, `ServiceIdentity` structs in `polkagent-identity` crate.
  **Acceptance:** all structs serialize/deserialize via SCALE and JSON; unit tests
  cover round-trip and field validation.

- [ ] **IMPL-IDENT-02** Implement `IdentityResolver` trait with `OnChainResolver`
  backed by a subxt client against People Chain.
  **Acceptance:** resolver returns a timestamped `IdentityResolution` with stale
  indicator; integration test against a live People Chain node.

- [ ] **IMPL-IDENT-03** Implement People Chain identity cache with configurable TTL
  (default: 5 minutes). Cache must be purgeable per user request (REQ-PRIV-07).
  **Acceptance:** cache hit avoids a chain query; cache miss or stale entry triggers
  a fresh query; explicit purge removes all entries for the given account.

- [ ] **IMPL-IDENT-04** Implement SS58 address formatting and validation for all
  supported networks. Display logic must show network-appropriate prefix.
  **Acceptance:** fuzz test with random bytes; no panics; invalid checksums are
  always rejected.

- [ ] **IMPL-IDENT-05** Implement `AgentCard` signing and verification.
  **Acceptance:** a card signed by key A is accepted; a card with a tampered field
  is rejected; an expired card is rejected.

### Policy Engine (v1)

- [ ] **IMPL-POLICY-01** Integrate the `cedar-policy` crate. Define the Cedar schema
  for Polkagent's PARC model (principal, action, resource, context types).
  **Acceptance:** schema validation rejects a policy with a missing field or wrong
  type; CI gate runs schema check on every policy file change.

- [ ] **IMPL-POLICY-02** Implement policy file loading from TOML with Argon2id-derived
  content hash and schema validation. File changes trigger async reload via
  `inotify`/`kqueue`.
  **Acceptance:** a valid policy file loads in < 100 ms; an invalid policy file is
  rejected without affecting the active policy; reload test verifies the new policy
  is active for subsequent requests.

- [ ] **IMPL-POLICY-03** Implement the composed policy set (global + org + workspace +
  agent + mandate layers). Each layer's revision is hashed; the composite digest
  is stored on `ResolvedGrant`.
  **Acceptance:** property-based test generates random policy combinations and
  verifies the effective decision is always the intersection (deny-overrides).

- [ ] **IMPL-POLICY-04** Implement `PolicyContext` construction from `AgentRun`,
  `PrincipalRef`, and `ActionDescriptor`. Injection risk score populated from
  detection stage output.
  **Acceptance:** unit test with a known injection risk score verifies the context
  field is set correctly; zero risk score when detection stage is absent.

- [ ] **IMPL-POLICY-05** Implement the ten-stage `PolicyPipeline` (table in section 8.4).
  Each stage is independently testable; the pipeline short-circuits on denial.
  **Acceptance:** isolation test for each stage; integration test running all stages
  in sequence.

- [ ] **IMPL-POLICY-06** Implement default grant profile for new agents (section 8.5).
  **Acceptance:** a freshly created agent with no explicit policy has exactly the
  three default grants and no others; any effect outside those three is denied.

### Grant Resolution (v1–v2)

- [ ] **IMPL-GRANT-01** Implement `resolve_grant` function (Appendix A.2) with all
  decision paths: expiry, delegation chain, Cedar evaluation, approval escalation,
  budget/rate checks, grant production.
  **Acceptance:** unit tests for each decision path; property-based test for
  delegation depth and authority narrowing.

- [ ] **IMPL-GRANT-02** Implement `ActiveGrantStore` with SQLite backend (local) and
  Postgres backend (cloud). Grants are append-only; expiry is enforced on read.
  **Acceptance:** expired grant denied on read; concurrent write test for race-free
  grant insertion.

- [ ] **IMPL-GRANT-03** Implement delegation chain traversal and `DelegationRegistry`.
  **Acceptance:** chain with valid links resolves correctly; expired link denies;
  authority-exceeded link emits a security event and denies.

- [ ] **IMPL-GRANT-04** Implement time-based grant expiry (`compute_grant_expiry`).
  REQ-POLICY-01: no grant without expiry; REQ-POLICY-02: expired grants denied
  immediately.
  **Acceptance:** a grant 1 ms past its expiry is denied; no code path produces
  `expires_at = None`.

### Gates (v1)

- [ ] **IMPL-GATE-01** Implement `ApprovalGate` with IPC channel to TUI modal (see
  Appendix B.2 and Appendix E).
  **Acceptance:** action above value threshold suspends and awaits TUI response;
  timeout produces `Deny`; approval produces `Allow`.

- [ ] **IMPL-GATE-02** Implement `BudgetGate` with `BudgetStore` backed by the run
  event log. Rolling and lifetime limits enforced.
  **Acceptance:** spend exactly at limit is allowed; spend one planck over limit
  is denied; limit reset after rolling window.

- [ ] **IMPL-GATE-03** Implement `AllowlistGate` with `Strict`, `Escalate`, and `Warn`
  modes. Allowlist loaded from policy config.
  **Acceptance:** address in allowlist is allowed; unknown address behaves per mode.

- [ ] **IMPL-GATE-04** Implement `RateLimitGate` with token bucket algorithm.
  **Acceptance:** burst up to capacity is allowed; sustained rate above refill
  rate produces denials; bucket state persists across requests within a run.

- [ ] **IMPL-GATE-05** Implement gate composition (`And`, `Or`, `Sequential`) and
  gate result caching with 5-second TTL.
  **Acceptance:** `And` short-circuits on first deny; `Or` short-circuits on first
  allow; cache flush on policy reload verified.

### Secret Management (v1–v2)

- [ ] **IMPL-SECRET-01** Implement `SecretStore` trait and `OsKeychainStore` for macOS
  and Linux.
  **Acceptance:** store, retrieve, rotate, revoke round-trip test; raw bytes are
  not observable outside the `with_secret` closure.

- [ ] **IMPL-SECRET-02** Implement `EncryptedFileStore` with Argon2id KDF.
  File at `~/.local/share/polkagent/secrets.enc` (Linux) /
  `~/Library/Application Support/Polkagent/secrets.enc` (macOS).
  **Acceptance:** encrypted file is not readable without the correct passphrase;
  Argon2id parameters meet OWASP 2024 guidance; zeroize test confirms buffer is
  cleared after use.

- [ ] **IMPL-SECRET-03** Implement audit logging for all `SecretStore` operations.
  Audit record written before operation; failure to write audit aborts the operation.
  **Acceptance:** audit log contains one record per operation; no secret values in
  any log record.

- [ ] **IMPL-SECRET-04** Implement `CloudKmsStore` for AWS KMS (v2).
  **Acceptance:** integration test with a localstack-backed KMS endpoint; KMS
  audit trail contains signing events.

### Signer Isolation (v1–v2)

- [ ] **IMPL-SIGNER-01** Implement `WatchOnlySigner`. Default for new agents.
  **Acceptance:** `sign()` always returns `Refused { reason: WatchOnly }`;
  `accounts()` returns the watched list.

- [ ] **IMPL-SIGNER-02** Implement the signer IPC channel (`SignerIpcChannel`) and the
  isolated signer process launcher.
  **Acceptance:** IPC round-trip test with a mock signer process; process
  isolation verified (agent process cannot read signer process memory).

- [ ] **IMPL-SIGNER-03** Implement `verify_payload_before_signing` in the signer
  process.
  **Acceptance:** expired request is refused; unknown account is refused; call
  type matching signer call filter deny list is refused.

- [ ] **IMPL-SIGNER-04** Implement signing audit records in the signer process.
  Audit record written before key material is accessed.
  **Acceptance:** audit record present for every signing attempt; record contains
  payload hash (not raw bytes); record written even when signing fails.

- [ ] **IMPL-SIGNER-05** Implement `LocalKeystoreSigner` with Argon2id encryption and
  zeroize-on-drop for decrypted buffers (v2).
  **Acceptance:** decrypted key bytes are not observable in memory after signing;
  zeroize-on-drop verified with a custom test allocator.

- [ ] **IMPL-SIGNER-06** Implement `ExternalWalletSigner` with Polkadot.js and
  WalletConnect v2 protocols.
  **Acceptance:** returned signature is verified against the exact sent payload;
  wrong-payload signature is rejected.

- [ ] **IMPL-SIGNER-07** Implement `HardwareSigner` for Ledger (USB) and Polkadot Vault
  (QR code), including RFC-0078 metadata hash in the payload.
  **Acceptance:** device disconnect is handled gracefully; no fallback to software
  signer; metadata hash is present in every hardware signing payload.

---

## APPENDIX D: REFERENCE FILE MAP

| Component | Roko Files | Bardo Files | Key Patterns |
|---|---|---|---|
| Approval modal (TUI) | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/modals/approval.rs` | `/Users/will/dev/uniswap/bardo/apps/mori/src/tui/modals/confirm.rs` | `render_approval`: centered popup, danger border style, agent role + command display, `[y]` approve / `[n]` reject keybindings; Bardo: title + description fields, styled `[y] confirm [n] cancel` line |
| Approval IPC channel | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/approval_ipc.rs` | — | `ApprovalRequest { role, command, approval_id, response_tx: oneshot::Sender<bool> }`; `ApprovalChannel::new(buffer)` produces bounded `mpsc` pair; orchestrator owns `tx`, TUI owns `rx` |
| Approval request types | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/approval_ipc.rs` | — | Typed request struct; opaque `approval_id` string; oneshot boolean reply; mirrors `IpcSigningRequest.request_id` pattern from Appendix B.2 |
| Styled confirmation dialog | — | `/Users/will/dev/uniswap/bardo/apps/mori/src/tui/modals/confirm.rs` | `render()` dispatches on `ConfirmAction` variant; `render_merge_to_main`: flow graphic (`──⬆──▶`), plan count badge, warning line, confirm/cancel keybindings; drop-shadow via `postfx::drop_shadow`; ember-colored border |
| Theme / color system | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/modals/approval.rs` (uses `theme.danger()`, `theme.accent_bold()`, `theme.warning()`, `theme.success()`, `theme.muted()`, `theme.text()`) | `/Users/will/dev/uniswap/bardo/apps/mori/src/tui/modals/confirm.rs` (uses `Theme::STATUS_WARN`, `Theme::FG`, `Theme::STATUS_ERROR`, `Theme::STATUS_OK`, `Theme::EMBER`) | Roko: method-based theme on `&Theme`; Bardo: associated constants on `Theme` struct |
| Centered rect utility | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/modals/approval.rs` (`centered_rect(percent_x, percent_y, area)`) | `/Users/will/dev/uniswap/bardo/apps/mori/src/tui/modals/confirm.rs` (`centered_rect(50, 25, area)` / `centered_rect(60, 40, area)`) | Both use `Layout::default()` with `Constraint::Percentage` split in vertical then horizontal direction; popup sizes differ by use case (40% height for approval, 25% for simple confirm) |
| IPC channel buffer | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/approval_ipc.rs` (`ApprovalChannel::new(buffer: usize)`) | — | Buffer size parameterized at construction; typical value: 8–16 for approval queues |
| Key isolation (no raw keys in IPC) | `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/approval_ipc.rs` (command is a `String`; no raw bytes of sensitive data) | — | `IpcSigningRequest` follows same pattern: `payload` is `CanonicalPayload` (opaque wrapper), not a raw `[u8]` exposed to the TUI layer |

---

## APPENDIX E: TUI SURFACE FOR IDENTITY/SECURITY

This appendix specifies the terminal UI surfaces that expose identity and
security operations to the operator. All surfaces are implemented with
`ratatui`. The approval modal follows Roko's pattern; the confirmation dialog
follows Bardo's pattern.

### Approval Modal

The approval modal is displayed whenever the `ApprovalGate` escalates a
request. It receives an `ApprovalRequest` from the `ApprovalChannel` (see
`/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/approval_ipc.rs`)
and sends a boolean response via the embedded `oneshot::Sender<bool>`.

The Polkagent approval modal extends Roko's `render_approval` function
(`/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/modals/approval.rs`)
with additional context fields for chain-specific data.

```rust
pub struct PolkagentApprovalRequest {
    /// Agent label (maps to Roko's `role` field).
    pub agent_label: String,
    /// Human-readable description of the action.
    pub action_summary: String,
    /// Decoded call type (e.g. "transfer(dest=5Grw…, value=1.5 DOT)").
    pub decoded_call: String,
    /// Network this action targets.
    pub network: String,
    /// Estimated fee in human-readable format.
    pub estimated_fee: Option<String>,
    /// Risk flags from the gate pipeline.
    pub risk_flags: Vec<String>,
    /// Opaque approval identifier (maps to Roko's `approval_id`).
    pub approval_id: String,
    /// Reply channel.
    pub response_tx: oneshot::Sender<bool>,
}
```

ASCII wireframe (60 columns x 20 rows, rendered inside `centered_rect(60, 40, area)`):

```
┌──────────────── Approval Required ─────────────────┐
│                                                     │
│  Agent:   defi-bot-v2                               │
│                                                     │
│  Action:  chain:Transfer                            │
│  Network: Polkadot                                  │
│                                                     │
│  Call:                                              │
│    transfer(                                        │
│      dest=5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNeh…,   │
│      value=1.500000000000 DOT                       │
│    )                                                │
│                                                     │
│  Fee:     ~0.002 DOT (estimated)                    │
│                                                     │
│  Risks:   [!] First transfer to this recipient      │
│                                                     │
│  [y] approve          [n] reject          [?] help  │
└─────────────────────────────────────────────────────┘
```

Implementation notes:
- Border style: `theme.danger()` (red), matching Roko's approval modal.
- `decoded_call` is rendered in `theme.warning()` (yellow), never
  model-authored text.
- `risk_flags` are rendered in `theme.danger()` if present.
- Pressing `y` sends `true` on `response_tx`; pressing `n` sends `false`.
- Pressing `?` opens the help overlay with a link to the operator's policy
  documentation.
- Timeout countdown displayed in the footer if `ApprovalRequirement.timeout`
  is set.

---

### Confirmation Dialog

Used for destructive configuration operations (mandate creation, key rotation,
emergency pause). Follows Bardo's `render` pattern
(`/Users/will/dev/uniswap/bardo/apps/mori/src/tui/modals/confirm.rs`) with
a drop-shadow and ember-colored border.

ASCII wireframe (50 columns x 12 rows, rendered inside `centered_rect(50, 25, area)`):

```
┌──────────────── Confirm Action ─────────────────┐
│                                                 │
│   Revoke Mandate: defi-bot-mandate-001          │
│                                                 │
│   This will immediately stop all autonomous     │
│   operations for agent defi-bot-v2.             │
│                                                 │
│                                                 │
│   [y] confirm         [n] cancel                │
└─────────────────────────────────────────────────┘
```

Implementation notes:
- Drop-shadow rendered via `postfx::drop_shadow` (as in Bardo).
- Border color: `Theme::EMBER` for destructive actions.
- Title: "Confirm Action".
- `[y]` in `Theme::STATUS_ERROR` (red/danger); `[n]` in `Theme::STATUS_OK`
  (green).

---

### Identity Management Panel

Displays the current agent's identity, bound accounts, key status, and
delegation tree. Rendered as a full-width panel in the main TUI layout.

ASCII wireframe (80 columns):

```
┌─────────────────────── Identity ─────────────────────────────────────────────┐
│  Agent:     defi-bot-v2              Operator: 5GrwvaEF…                     │
│  Agent ID:  agent_01JZ…             Spec:     v1.4.2                         │
│                                                                               │
│  ── Bound Accounts ────────────────────────────────────────────────────────  │
│  [*] 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY  (Polkadot)          │
│       Signer: external-wallet   Key: Sr25519   Proxy: NonTransfer/7-day       │
│       Identity: "Acme Corp Agent" [KnownGood @14,923,001] (fresh)            │
│                                                                               │
│  ── Delegation Tree ──────────────────────────────────────────────────────── │
│  org:acme-corp                                                                │
│    └─ operator:alice                                                          │
│         └─ agent:defi-bot-v2  (expires: 2026-08-29, sub-delegatable: false)  │
│                                                                               │
│  ── Key Status ───────────────────────────────────────────────────────────── │
│  Transport key: pk_01JZ…   Rotated: 2026-07-15   Status: ACTIVE              │
│  Signer:        external    Health: HEALTHY        Last check: 00:12 ago      │
│                                                                               │
│  [r] rotate key  [d] delegation details  [i] identity details  [q] back      │
└───────────────────────────────────────────────────────────────────────────────┘
```

---

### Policy Editor

Allows operators to view, edit, and test Cedar policy rules. Includes a test
harness that evaluates a sample request against the current policy before save.

ASCII wireframe (80 columns):

```
┌───────────────────────── Policy Editor ──────────────────────────────────────┐
│  Agent: defi-bot-v2    Revision: 2026-07-30T09:00:00Z    Status: VALID       │
│                                                                               │
│  ── Rules ─────────────────────────────────────────────────────────────────  │
│  [1] allow-transfers-under-threshold   PERMIT   chain:Transfer, chain:Tran…  │
│  [2] deny-proxy-management             FORBID   chain:AddProxy, chain:Remo…  │
│  [3] require-approval-above-5-dot      ESCALATE chain:Transfer (value>5DOT)  │
│                                                                               │
│  ── Selected Rule ──────────────────────────────────────────────────────────  │
│  effect = "permit"                                                            │
│  action = ["chain:Transfer", "chain:TransferKeepAlive"]                      │
│  conditions.max_amount_dot = 10                                               │
│  conditions.require_approval_above_dot = 5                                    │
│                                                                               │
│  ── Test Request ───────────────────────────────────────────────────────────  │
│  Action: chain:Transfer  Value: 3 DOT  Recipient: 5Grw…                      │
│  Result: PERMIT (matched rule 1)                                              │
│                                                                               │
│  ── Audit Log ──────────────────────────────────────────────────────────────  │
│  2026-07-30 09:12:01  PERMIT  chain:Transfer  defi-bot-v2  1.5 DOT          │
│  2026-07-30 09:11:44  DENY    chain:AddProxy  defi-bot-v2  (rule 2)          │
│                                                                               │
│  [e] edit rule  [+] add rule  [t] run test  [s] save & validate  [q] back   │
└───────────────────────────────────────────────────────────────────────────────┘
```

---

### Approval Queue

Displays pending approval requests sorted by priority (value descending, then
time-to-expiry ascending). The operator navigates the queue and approves or
rejects each item.

ASCII wireframe (80 columns):

```
┌──────────────────────── Approval Queue ──────────────────────────────────────┐
│  Pending: 3    Approved today: 12    Denied today: 2                         │
│                                                                               │
│  ── Queue (sorted by priority) ─────────────────────────────────────────────  │
│  [1] !! defi-bot-v2   chain:Transfer   8.5 DOT → 5Hq…   expires in 4m 12s  │
│  [2]    defi-bot-v2   chain:Transfer   2.0 DOT → 5Grw…  expires in 9m 01s  │
│  [3]    yield-bot     chain:Nominate   (no value)        expires in 14m 59s  │
│                                                                               │
│  ── Selected Item [1] ──────────────────────────────────────────────────────  │
│  Agent:    defi-bot-v2                                                        │
│  Action:   chain:Transfer                                                     │
│  Decoded:  transfer(dest=5Hq3…, value=8.5 DOT)                               │
│  Network:  Polkadot     Fee: ~0.002 DOT                                       │
│  Risks:    [!] Value above 5 DOT threshold                                    │
│            [!] Recipient not in allowlist                                     │
│                                                                               │
│  [y] approve  [n] reject  [j/k] next/prev  [a] approve all below 5DOT       │
└───────────────────────────────────────────────────────────────────────────────┘
```

---

## APPENDIX F: CONFIGURATION GUIDE

This appendix specifies the exact file formats and locations for all
operator-controlled configuration.

### Policy File Format and Location

Agent-level policy files are TOML. The file name matches the agent ID.
The directory is configurable via `POLKAGENT_POLICY_DIR` (default:
`~/.config/polkagent/policies/`).

```
~/.config/polkagent/policies/
├── agent-defi-bot-v2.toml       # agent-level policy
├── agent-yield-bot.toml
└── org-acme-corp.toml           # org-level policy (loaded as a layer)
```

Full TOML schema for an agent policy file:

```toml
[meta]
# Semantic version of the policy schema.
schema_version = "1.0"
# ISO-8601 timestamp; used as the policy revision for grant binding.
revision = "2026-07-30T09:00:00Z"
# Must match the agent_id in the agent configuration.
agent_id = "defi-bot-v2"
# Policy engine; must be "cedar".
policy_engine = "cedar"
# Optional human-readable description.
description = "Policy for the DeFi trading bot"

# Each [[rule]] block defines one Cedar policy statement.
[[rule]]
id = "allow-transfers-under-threshold"
# "permit" or "forbid"
effect = "permit"
# Actions covered by this rule.
action = ["chain:Transfer", "chain:TransferKeepAlive"]
# Resource pattern ("*" for any).
resource = "*"

[rule.conditions]
# Maximum transfer amount in DOT (human-readable; converted to planck internally).
max_amount_dot = 10
# Require human approval for transfers above this amount.
require_approval_above_dot = 5
# Allowlist of recipient accounts (SS58). If absent: all recipients permitted.
# allowed_recipients = ["5GrwvaEF…"]

[[rule]]
id = "deny-proxy-management"
effect = "forbid"
action = ["chain:AddProxy", "chain:RemoveProxy", "chain:ProxyAnnounced"]
resource = "*"
# No [rule.conditions] block: unconditional deny.

[gates]
# Budget gate configuration.
[gates.budget]
rolling_limit_dot = 50
rolling_window_hours = 24
lifetime_limit_dot = 10000

# Rate limit gate configuration.
[gates.rate_limit]
capacity = 20              # max requests in bucket
refill_rate_per_second = 0.1  # 6 requests per minute sustained

# Allowlist gate configuration.
[gates.allowlist]
mode = "Escalate"          # "Strict" | "Escalate" | "Warn"
# recipients = ["5Grw…"]  # if absent: no allowlist enforced
```

### Grant Configuration

Grants are not configured directly by operators. They are produced by the
grant resolution algorithm as a result of policy evaluation and approval
decisions. Operators configure grants indirectly through:

1. **Policy rules** (section above): determine what the Cedar engine permits.
2. **Mandate configuration** (section below): define the scope and budget for
   autonomous grants.
3. **Approval decisions**: human approvals produce time-bounded grants that are
   stored in the `ActiveGrantStore`.

### Secret Store Backend Selection

The secret store backend is selected in the agent configuration file:

```toml
# ~/.config/polkagent/agents/defi-bot-v2.toml

[secrets]
# Backend: "os_keychain" | "encrypted_file" | "hsm" | "cloud_kms"
backend = "os_keychain"

# For encrypted_file backend:
# [secrets.encrypted_file]
# path = "~/.local/share/polkagent/secrets.enc"
# argon2_memory_kb = 65536
# argon2_iterations = 3
# argon2_parallelism = 4

# For cloud_kms backend (v2):
# [secrets.cloud_kms]
# provider = "aws"           # "aws" | "gcp" | "azure"
# region = "us-east-1"
# key_id = "arn:aws:kms:us-east-1:123456789012:key/…"
# credentials_profile = "polkagent-prod"
```

### Signer Configuration

```toml
# ~/.config/polkagent/agents/defi-bot-v2.toml

[[signers]]
# Signer mode: "watch_only" | "external_wallet" | "hardware_ledger" |
#              "hardware_vault" | "proxy" | "multisig" | "local_keystore" |
#              "cloud_kms" | "organizational"
mode = "external_wallet"
# The account this signer handles.
account = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
# Wallet protocol for external_wallet mode.
wallet_protocol = "walletconnect_v2"

# For proxy mode (v2):
# [[signers]]
# mode = "proxy"
# proxied_account = "5HpG9w…"
# proxy_account = "5GrwvaEF…"
# proxy_type = "NonTransfer"
# delay_blocks = 50400  # ~7 days at 6s/block
# proxy_signer = { mode = "external_wallet", wallet_protocol = "polkadot_js" }
```

### Audit Log Settings

```toml
# ~/.config/polkagent/config.toml

[audit]
# Audit log backend: "file" | "syslog" | "otlp"
backend = "file"
# Minimum log level for audit events.
min_level = "info"

[audit.file]
path = "~/.local/share/polkagent/audit.jsonl"
# Maximum file size before rotation.
max_size_mb = 100
# Number of rotated files to retain.
max_files = 10
# Retention period for audit records.
retention_days = 90

# For OpenTelemetry export (v2):
# [audit.otlp]
# endpoint = "https://otel-collector.example.com:4317"
# headers = { "x-api-key" = "${OTEL_API_KEY}" }
```

---

## APPENDIX G: SECURITY TESTING

This appendix specifies targeted security test cases. Each test is a concrete
scenario with a setup, action, and expected outcome. Tests are organized
by the invariant they verify.

### Policy Bypass Test Cases

**TEST-SEC-01: Cedar forbid overrides permit (INV-POLICY-01)**
- Setup: load a policy with a `permit` for `chain:Transfer` and a `forbid`
  for `chain:Transfer` at a different scope.
- Action: request `chain:Transfer` by the principal that matches both rules.
- Expected: `Deny`. The forbid rule overrides the permit regardless of scope.

**TEST-SEC-02: Expired policy revision rejects existing grant (INV-POLICY-05)**
- Setup: create a `ResolvedGrant` with revision `R1`. Reload policy to revision `R2`.
- Action: attempt to use the `R1` grant for a new request.
- Expected: `Defer(DeferReason::PolicyChanged)` and re-resolution under `R2`.

**TEST-SEC-03: Model output cannot produce a grant (INV-POLICY-04)**
- Setup: inject model output that contains text resembling a `ResolvedGrant`
  JSON structure into the artifact store.
- Action: policy pipeline evaluates the next request for the same principal.
- Expected: the model-authored text is not parsed as a grant; the request is
  evaluated from scratch against the Cedar engine.

**TEST-SEC-04: Default new agent has no effect grants (INV-POLICY-03)**
- Setup: create a new agent with no explicit policy file.
- Action: request `chain:Transfer` for the new agent.
- Expected: `Deny`. Only the three default read/respond/inference grants exist.

**TEST-SEC-05: Injection risk score raises approval threshold**
- Setup: configure `ApprovalGate.injection_risk_threshold = 0.5`. Set
  `context.env.injection_risk_score = 0.7`.
- Action: request an action that would normally be policy-autonomous.
- Expected: `RequireApproval` rather than `Permit`.

---

### Grant Escalation Tests

**TEST-SEC-06: Delegation authority cannot exceed delegator (INV-DELEG-01)**
- Setup: create a delegation chain where the delegate's `granted` constraints
  claim a higher `max_amount_dot` than the delegator's active grants.
- Action: attempt to resolve a grant for the delegate.
- Expected: `Deny` with a `SecurityEvent::DelegationAuthorityExceeded` emitted.

**TEST-SEC-07: Scope hierarchy cannot widen (section 8.7)**
- Setup: org policy allows `chain:Transfer` up to 100 DOT/day. Agent policy
  attempts to configure 200 DOT/day.
- Action: grant resolution for the agent's transfer request.
- Expected: effective limit is 100 DOT/day (org policy wins). The agent's
  wider limit is silently capped.

**TEST-SEC-08: Reputation maximum does not widen minimum policy (INV-ISO-04)**
- Setup: set `principal.reputation_score = 1.0` (maximum). Set agent policy
  to deny `chain:Transfer`.
- Action: request `chain:Transfer`.
- Expected: `Deny`. Reputation has no effect on the Cedar evaluation.

**TEST-SEC-09: Sub-delegation beyond max_depth is denied**
- Setup: create a delegation chain of depth 4. Set `max_depth = 3` on the root
  delegation record.
- Action: attempt to resolve a grant using the depth-4 delegate.
- Expected: `Deny(ChainError::DepthExceeded)`.

---

### Signer Isolation Verification

**TEST-SEC-10: Signer process cannot be accessed from agent process (INV-SIGN-01)**
- Setup: start the agent and signer as separate OS processes.
- Action: from the agent process, attempt to read the signer process's memory
  space (e.g., via `/proc/<pid>/mem` on Linux or `vm_read` on macOS).
- Expected: access is denied by the OS. The test passes if the read attempt
  returns a permission error.

**TEST-SEC-11: Payload verification before signing rejects expired request**
- Setup: create an `IpcSigningRequest` with `expires_at` one second in the
  past.
- Action: send the request to the signer process.
- Expected: `SigningResult::Refused { reason: PayloadRejected { details: "signing request expired" } }`.

**TEST-SEC-12: Signer refuses unknown account**
- Setup: configure the signer with account `A`. Send a signing request for
  account `B`.
- Expected: `SigningResult::Refused { reason: AccountNotFound }`.

**TEST-SEC-13: Signer audit record written before key access**
- Setup: instrument the signer process to record the order of: (1) audit log
  write, (2) key decryption.
- Action: send a valid signing request.
- Expected: audit log write occurs strictly before key decryption begins.

**TEST-SEC-14: Hardware signer disconnect fails gracefully (INV-AVAIL-03)**
- Setup: connect a Ledger device. Begin a signing request. Disconnect the
  device mid-request.
- Expected: `SigningResult::Unavailable { reason: "device disconnected" }`.
  The agent does NOT fall back to any software signer.

---

### Secret Leakage Detection Tests

**TEST-SEC-15: SecretForbidden data rejected by artifact store (INV-SIGN-02)**
- Setup: classify a byte buffer as `SecretForbidden`. Attempt to write it to
  the artifact store.
- Expected: the store returns `ClassificationError::SecretForbidden`. No data
  is written. A `SecurityEvent::ClassificationViolation` is emitted.

**TEST-SEC-16: SecretForbidden data rejected by log sink**
- Setup: attempt to log a string containing simulated key material. Tag the
  log record with `DataClassification::SecretForbidden`.
- Expected: the log sink drops the record and emits a
  `SecurityEvent::ClassificationViolation`. No key material appears in any log
  file.

**TEST-SEC-17: Key material zeroized after signing**
- Setup: use a custom test allocator that tracks all allocations. Perform a
  signing operation that decrypts a key into a heap buffer.
- Action: after signing, inspect the heap region that held the key material.
- Expected: the region contains all zeros (zeroize-on-drop verified).

**TEST-SEC-18: Crash during signing does not expose key material in recovery**
- Setup: simulate a process crash (SIGKILL) at the moment of signing.
- Action: restart the signer process. Inspect the signer's state file or
  core dump if present.
- Expected: no raw key material in any recovery artifact. The restart
  re-prompts for the keystore passphrase.

**TEST-SEC-19: API key not exposed in CLI command history**
- Setup: configure `POLKAGENT_API_KEY` as an environment variable. Invoke a
  CLI command that uses the API key.
- Action: inspect shell history files (`~/.bash_history`, `~/.zsh_history`).
- Expected: the API key value does not appear in any history file. REQ-AUTH-02.

**TEST-SEC-20: Secret audit log does not contain raw secret values**
- Setup: perform a sequence of `store`, `with_secret`, `rotate`, and `revoke`
  operations on the `SecretStore`.
- Action: inspect the audit log file.
- Expected: every audit record contains only `secret_id`, `secret_label`,
  `operation`, `outcome`, and `timestamp`. No raw bytes, no base64-encoded
  values, no hex strings that match the stored secret material.
| `research-polkadot-jam-products.md` sections 5, 6 | 2026-07-30 | Identity, signer integration |
| `research-roko-definitive.md` patterns 7, 8 | 2026-07-30 | Gates, capability intersection |
| `reserach/research1.md` section I | 2026-07-30 | Identity candidates I1-I4 |
| `reserach/research2.md` prompts 3, 13, 14 | 2026-07-30 | Signer, proxy, security |
