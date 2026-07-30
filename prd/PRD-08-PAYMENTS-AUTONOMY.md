# PRD-08: Payments, Autonomous Agents and Economic Controls

**Status:** definitive PRD
**Owner:** unassigned
**Last updated:** 2026-07-30
**Authority:** `01-ESTABLISHED-BASELINE.md` is authoritative where this document
does not explicitly supersede it.
**Implementation status:** design direction only. No production payment rail,
funded agent account, or autonomous mandate has been validated.

---

## 1. Purpose and orientation

### 1.1 What this document covers

This PRD defines how Polkagent handles money: how value is represented, how
payments are proposed and executed, how agents can operate with increasing
degrees of autonomy over funds, and what controls protect users at every level.

It is written for readers who may be unfamiliar with Polkadot's account model,
asset system, or cross-chain messaging. Every specialized concept is explained
before it is used in a requirement.

### 1.2 Why payments matter to an agent platform

Polkagent's Act pillar -- safely performing on-chain work -- necessarily involves
value. Even a read-only governance scout must understand balances, fees, and
account survival rules. A transfer assistant must decode, simulate, and present
a payment before an external signer authorizes it. A policy-autonomous agent
must operate within budgets that the platform enforces independently of the
model.

The payment system is therefore not an add-on feature. It is a foundational
domain that intersects custody, policy, identity, chain integration, UX, and
the effect/artifact lifecycle defined in other PRDs.

### 1.3 Relationship to other PRDs

| PRD | Interface |
|---|---|
| PRD-02 (Vocabulary/Architecture) | Payment types, effect lifecycle, grant model |
| PRD-03 (Execution Model) | `EffectIntent`/`EffectAttempt`/`EffectOutcome` for payment effects |
| PRD-04 (Providers/Tools) | Payment tools and signer ports |
| PRD-05 (Polkadot Integration) | Chain client, metadata, XCM, runtime APIs |
| PRD-07 (Identity/Signers/Policy) | Signer isolation, policy evaluation, account binding |
| PRD-10 (Data/Artifacts) | Receipts, reconciliation artifacts, audit trail |
| PRD-13 (UX) | Action cards, approval flows, budget dashboards |

### 1.4 Core terms

| Term | Meaning in this PRD |
|---|---|
| **Payment** | Any operation that moves, locks, reserves, approves, or otherwise changes the disposition of value on a blockchain or between agents. |
| **Asset** | A fungible or non-fungible unit of value on a specific chain, identified by chain genesis hash, runtime version, and asset location/ID -- never by ticker alone. |
| **Intent** | A typed, expiring, network-bound proposal for an action. It is not evidence that the action occurred. |
| **Action card** | The canonical, metadata-derived, trusted UI rendering of an intent. Model prose is explanatory and visibly separate. |
| **Signer** | An isolated component that receives a canonical payload and returns a signature or refusal. It never exposes key material to the model, harness, or tool context. |
| **Mandate** | A configured policy that authorizes an agent to execute actions within defined boundaries without per-action human approval. |
| **Receipt** | A durable artifact that joins intent, approval/mandate evidence, submission, and finality observation into an exportable, verifiable record. |
| **Profile** | A `ChainProfile` that pins genesis hash, metadata hash, runtime version, asset registry, and fee parameters at a recorded block. |
| **ED** | Existential deposit: the target runtime's minimum-balance/account-survival rule. It is profile-specific. |

### 1.5 Guiding principles

1. **Keys never enter model context.** Signing keys live in external wallets,
   hardware devices, threshold services, KMS/HSM, or isolated agent keystores.
   The model, harness, tool, and marketplace extension never receive seed
   phrases, private keys, or signing sessions.

2. **Canonical data, not model prose, is authority.** The action card is
   rendered from typed, metadata-decoded fields. The model may explain; it
   cannot forge card fields or authorize payment.

3. **Conservative defaults, configurable ceilings.** New agents start with zero
   payment authority. Each expansion requires explicit owner configuration with
   visible consequences.

4. **Every outcome is durable.** Success, failure, timeout, cancellation, and
   unknown are distinct recorded states. The system never collapses "unknown"
   into "failed" or "succeeded."

5. **Profile-bound, not ticker-bound.** An asset's decimals, sufficiency,
   fee eligibility, and existential-deposit consequence are facts about a
   specific chain at a specific block, not portable constants.

---

## 2. Payment domain model

### 2.1 Asset identity

An asset in Polkagent is not a ticker string. It is a composite identity bound
to a specific chain and runtime.

#### 2.1.1 Canonical asset identifier

```rust
/// A fully qualified asset identity bound to a chain profile.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct AssetId {
    /// Genesis hash of the chain where this asset lives.
    pub chain_genesis: H256,
    /// Runtime spec version at which this identity was resolved.
    pub spec_version: u32,
    /// The asset's on-chain location or identifier.
    /// For native balance: `AssetLocation::Native`.
    /// For Assets pallet items: `AssetLocation::PalletAsset { pallet_index, asset_id }`.
    /// For foreign assets: `AssetLocation::Foreign { xcm_location }`.
    pub location: AssetLocation,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum AssetLocation {
    /// The chain's native balance (e.g., DOT on Polkadot Hub).
    Native,
    /// An asset managed by an Assets pallet instance.
    PalletAsset {
        pallet_index: u8,
        asset_id: u128,
    },
    /// A foreign asset keyed by its XCM multilocation.
    Foreign {
        xcm_location: VersionedLocation,
    },
}
```

#### 2.1.2 Asset metadata profile

```rust
/// Resolved, block-pinned metadata for a known asset.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetProfile {
    pub id: AssetId,
    /// Human-readable name from chain metadata. Untrusted for security decisions.
    pub name: Option<String>,
    /// Ticker/symbol from chain metadata. Untrusted for security decisions.
    pub symbol: Option<String>,
    /// Decimal places for human-readable display.
    pub decimals: u8,
    /// Whether this asset is "sufficient" -- can an account exist with only
    /// this asset and no native balance?
    pub is_sufficient: bool,
    /// Minimum balance / existential deposit for this asset.
    pub existential_deposit: u128,
    /// Whether this asset can pay transaction fees on the target chain.
    pub fee_eligible: bool,
    /// Operator-curated trust level for display and payment use.
    pub trust_level: AssetTrustLevel,
    /// Block hash at which this profile was resolved.
    pub resolved_at_block: H256,
    /// Timestamp of resolution.
    pub resolved_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AssetTrustLevel {
    /// Operator has explicitly verified this asset for use in payments.
    Verified,
    /// Known from chain metadata but not operator-verified.
    Recognized,
    /// Unknown or unverified asset. Display with warnings.
    Unknown,
}
```

**Requirement PAY-ASSET-001:** An asset ID, decimal scale, sufficiency,
fee eligibility, and ED must be queried from the target chain at a recorded
block and retained with genesis hash, metadata hash, and runtime version.
A drifted or unknown profile must refuse to render a confident amount or
fee claim.

**Requirement PAY-ASSET-002:** Asset metadata fields (name, symbol, ticker)
are untrusted display data. They must never be the sole basis for asset
identity in payment operations. Maintain an operator-curated asset allowlist
keyed by `AssetId`.

**Requirement PAY-ASSET-003:** Display branding (icons, names, tickers) is
separate from the canonical `AssetId`. An ERC-20-shaped precompile does not
prove that a display token is the intended issuer asset.

### 2.2 Supported asset classes

| Asset class | Polkadot mechanism | Polkagent support | Phase |
|---|---|---|---|
| Native balance (DOT) | `pallet_balances` | Core | Phase 1 |
| System stablecoins (USDC, USDT on Hub) | `pallet_assets` on Hub | Core | Phase 1 |
| Foreign assets | `pallet_assets` with XCM multilocation key | Supported | Phase 2 |
| Bridged assets | Bridge Hub + foreign asset registration | Supported | Phase 2+ |
| EVM/Revive tokens | ERC-20 precompile mapping to Assets pallet | Optional adapter | Phase 3 |
| NFTs | `pallet_nfts` or similar | Research | Phase 4+ |
| Coretime | Broker pallet | Research | Phase 4+ |

**v1 rail confirmation:** Research has confirmed the concrete asset IDs for Phase 1 stablecoins on Asset Hub: USDT is asset ID 1984, USDC is asset ID 1337. These are identified by their `PalletAsset { pallet_index, asset_id }` composite key in the canonical `AssetId` structure -- never by ticker string alone. Both are registered on Polkadot Hub mainnet and are valid Phase 1 transfer targets.

### 2.3 Network and chain binding

Every payment operation is bound to a specific chain profile. There is no
"generic DOT transfer" -- there is a transfer on a specific chain with a
specific genesis hash and runtime version.

```rust
/// A chain profile pins the execution context for payment operations.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChainProfile {
    /// Human label (e.g., "Polkadot Hub", "Paseo Testnet").
    pub label: String,
    /// Genesis hash -- the immutable chain identity.
    pub genesis_hash: H256,
    /// Current runtime spec version.
    pub spec_version: u32,
    /// Metadata hash for integrity checking.
    pub metadata_hash: H256,
    /// SS58 address prefix.
    pub ss58_prefix: u16,
    /// Known asset profiles for this chain.
    pub assets: Vec<AssetProfile>,
    /// RPC endpoint(s).
    pub endpoints: Vec<String>,
    /// Whether this is a testnet (value-free).
    pub is_testnet: bool,
    /// Block at which this profile was last verified.
    pub verified_at_block: H256,
    pub verified_at: DateTime<Utc>,
}
```

**Requirement PAY-CHAIN-001:** Every payment intent must reference a
`ChainProfile` by genesis hash. A runtime upgrade that changes the metadata
hash must invalidate outstanding intents and trigger re-verification.

**Requirement PAY-CHAIN-002:** Testnet and mainnet profiles must be visually
and programmatically distinguishable. No operation may silently cross the
testnet/mainnet boundary.

### 2.4 Fee assets and fee estimation

Transaction fees on Polkadot Hub can be paid in the native token (DOT) or,
where the runtime supports it, in alternative fee assets via
`pallet_asset_conversion`.

**USDC fee-sufficiency status:** As of 2026-07-30, USDC (asset 1337) is NOT
a fee-sufficient asset on Polkadot Hub. Referendum 174 requests fee-sufficiency
for USDC but has not yet passed. Polkagent must not assume USDC can pay fees.
For Phase 1, plan for DOT as the fee asset on all Asset Hub transactions, with
`pallet_asset_conversion` as the fallback for USDT/USDC-denominated flows.
This section will be updated if and when referendum 174 passes and the
sufficiency flag is reflected in the runtime profile for asset 1337.

```rust
/// A fee estimate bound to a specific block and profile.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeeEstimate {
    /// The asset used to pay fees.
    pub fee_asset: AssetId,
    /// Estimated fee amount in the fee asset's smallest unit.
    pub estimated_amount: u128,
    /// Maximum fee the user is willing to pay.
    pub fee_cap: Option<u128>,
    /// Block at which this estimate was computed.
    pub estimated_at_block: H256,
    /// Method used to produce the estimate.
    pub estimation_method: FeeEstimationMethod,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FeeEstimationMethod {
    /// Used the runtime's `TransactionPaymentApi`.
    RuntimeApi,
    /// Used `DryRunApi` simulation.
    DryRun,
    /// Used `XcmPaymentApi` for cross-chain fee estimation.
    XcmPaymentApi,
    /// Historical average from recent blocks.
    Historical,
    /// Operator-configured static estimate.
    Static,
}
```

**Requirement PAY-FEE-001:** Fee estimates are inputs to an intent, not
guarantees of execution. The system must re-check immediately before
signing/submitting when the estimate is older than a configurable threshold
(default: 2 blocks).

**Requirement PAY-FEE-002:** When alternative fee assets are used, the system
must verify fee eligibility from the current runtime profile, not from a
cached or assumed capability.

**Requirement PAY-FEE-003:** Every payment must be pre-flighted with the
three-API sequence: `DryRunApi` (simulate dispatch and events), `XcmPaymentApi`
(for any cross-chain weight component), and `TransactionPaymentApi`
(convert weight to fee amount). No payment may be submitted as a blind extrinsic
without completing this sequence. If any API is unavailable on the target
runtime, the pre-flight must surface that as an uncertainty block, not a
silent skip.

### 2.5 Existential deposits and account survival

Polkadot SDK chains enforce existential deposits (ED): accounts whose free
balance falls below the ED are reaped (deleted). Different assets may have
different EDs, and "sufficient" assets can keep an account alive without
native balance.

**Requirement PAY-ED-001:** Every transfer intent must compute and display
the sender's post-transfer balance relative to the applicable ED. If the
transfer would cause account reaping, the action card must display an
explicit warning with the exact ED, current balance, and projected
post-transfer balance.

**Requirement PAY-ED-002:** The system must distinguish between
`transfer_allow_death` (permits reaping) and `transfer_keep_alive` (fails if
it would reap). The default for user-facing transfers should be
`transfer_keep_alive` unless the user explicitly acknowledges the reaping
consequence.

**Requirement PAY-ED-003:** For funded agent accounts, the system must
maintain a configurable keep-alive buffer above the ED. The buffer amount
is a policy parameter, not a hardcoded constant.

**Requirement PAY-ED-004:** Every transaction submission must correctly manage
the nonce lifecycle: fetch the current account nonce at a pinned block, use it
exactly once in the signed payload, and handle nonce-collision errors
(e.g., `InvalidTransaction::Stale`) as distinct from other dispatch failures.
A nonce gap caused by a dropped transaction must be detected during
reconciliation and surfaced as an explicit Unknown state rather than a silent
retry loop. Metadata-hash inclusion in the signed payload (where the runtime
and signer support it) is required for all mainnet transactions.

### 2.6 Balance types

Polkadot accounts have multiple balance categories:

| Balance type | Meaning | Payment relevance |
|---|---|---|
| **Free** | Available for transfer or fee payment | The primary spendable balance |
| **Reserved** | Locked by runtime for deposits, bonds, or governance | Cannot be transferred; must be unreserved first |
| **Frozen** | Locked by governance voting, vesting, or staking | Cannot be transferred while frozen |
| **Transferable** | `free - max(frozen, reserved)` or runtime-specific formula | The actual amount available for payment |

**Requirement PAY-BAL-001:** The action card must display the transferable
balance, not the free balance, when showing available funds. The distinction
between free, reserved, and frozen must be visible in account research views.

**Requirement PAY-BAL-002:** Balance queries must be pinned to a specific
block. The system must not display a balance without its block reference
and must warn when the reference is older than a configurable staleness
threshold.

---

## 3. Payment operations

### 3.1 One-shot transfers

The simplest payment operation: send a specific amount of a specific asset
from one account to another on the same chain.

#### 3.1.1 User flow

```text
User request: "Send 10 DOT to Alice"
  |
  v
1. Intent construction
   - Resolve sender account from profile
   - Resolve recipient (SS58 address, verify format and network)
   - Resolve asset (DOT = native balance on target chain)
   - Query current balance, nonce, ED, fees at a pinned block
   |
  v
2. Pre-flight checks (see section 3.1.2)
   |
  v
3. Action card rendering
   - Canonical fields from metadata-decoded intent
   - Model explanation is visually separate
   |
  v
4. Approval
   - Per-action: user reviews and approves in UI
   - Mandate: system verifies intent falls within configured policy
   |
  v
5. Signing
   - Exact canonical payload sent to isolated signer
   - Signer returns signature or refusal
   |
  v
6. Submission
   - Signed extrinsic submitted to RPC endpoint
   - Idempotency key prevents duplicate submission
   |
  v
7. Finality observation
   - Watch for inclusion, then finality
   - Record outcome: success/failure/unknown
   |
  v
8. Receipt generation
   - Durable artifact linking intent -> approval -> submission -> outcome
```

#### 3.1.2 Pre-flight checks

Before rendering the action card, the system performs these checks:

| Check | API / mechanism | Failure behavior |
|---|---|---|
| Sender balance >= amount + estimated fee + keep-alive buffer | `system_account` state query | Refuse with explanation |
| Recipient address is valid SS58 for the target network | Local validation | Refuse with explanation |
| Recipient is not the sender (unless explicitly allowed) | Local check | Warn, allow override |
| Asset is in the operator-curated allowlist | Local policy | Refuse for unknown assets |
| Runtime metadata matches the profile | Metadata hash comparison | Refuse with stale-metadata error |
| Dispatch simulation succeeds | `DryRunApi` | Refuse with simulated error |
| Fee estimate is fresh and accurate | `TransactionPaymentApi` + `DryRunApi` | Re-estimate or refuse |
| XCM weight component estimated (if applicable) | `XcmPaymentApi` | Refuse if unavailable and route is cross-chain |
| Amount decimal encoding matches asset profile | Local validation | Refuse if ambiguous |
| No near-duplicate/homoglyph recipient addresses in recent history | Local check | Warn |

The pre-flight sequence must run `DryRunApi` before computing the fee via
`TransactionPaymentApi` so that the simulated weight is used for the fee
estimate rather than a static approximation. This three-API sequence
(`DryRunApi` -> `XcmPaymentApi` if cross-chain -> `TransactionPaymentApi`)
is mandatory; no blind submits.

**Requirement PAY-XFER-001:** A one-shot transfer must complete all
pre-flight checks before rendering the action card. No check may be skipped
by model instruction or tool-result injection.

### 3.2 Payment requests and invoices

A payment request is a structured message from a payee to a payer, asking
for a specific payment. An invoice adds terms, due dates, and reference data.

```rust
/// A payment request from a payee.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaymentRequest {
    pub id: PaymentRequestId,
    /// Who is requesting payment.
    pub payee: AccountId32,
    /// Requested asset and amount.
    pub asset: AssetId,
    pub amount: u128,
    /// Human-readable description/memo.
    pub memo: Option<String>,
    /// Optional reference for reconciliation.
    pub reference: Option<String>,
    /// Expiry time for the request.
    pub expires_at: Option<DateTime<Utc>>,
    /// Chain profile the payment should occur on.
    pub chain_profile: H256, // genesis hash
    /// Request status.
    pub status: PaymentRequestStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PaymentRequestStatus {
    Pending,
    Accepted { intent_id: IntentId },
    Paid { receipt_id: ReceiptId },
    Declined { reason: Option<String> },
    Expired,
    Cancelled,
}
```

**Requirement PAY-REQ-001:** Payment requests from external sources are
untrusted input. The system must re-verify all fields (recipient, asset,
amount, chain) against the current profile before constructing an intent.
A request cannot bypass pre-flight checks or policy evaluation.

**Requirement PAY-REQ-002:** Payment requests must be durable artifacts
with full lifecycle tracking. A request that has been paid must link to
its receipt; a request that expired must be visibly expired.

### 3.3 Allowances via proxy and delegated transfers

Polkadot SDK provides proxy and delegated-transfer mechanisms that can
limit what an agent account may do.

#### 3.3.1 Proxy-based allowances

A proxy grants one account the ability to act on behalf of another, filtered
by proxy type. Polkagent uses this for agent accounts (section 8).

```text
Controller (human/multisig)
  |
  +-- Pure Proxy (holds funds, no direct key)
        |
        +-- Agent Sub-Proxy (narrowly filtered)
              - Allowed: transfer_keep_alive to allowlisted recipients
              - Denied: proxy management, bonding, governance
              - Time-delayed or announcement-required
```

**Requirement PAY-PROXY-001:** Proxy type filters are runtime-defined code,
not portable labels. Polkagent must inspect the target runtime's source or
metadata to determine which calls a given proxy type permits. A label like
`NonTransfer` or `Staking` on one chain does not guarantee the same
semantics on another.

**Requirement PAY-PROXY-002:** Proxy addition and removal are high-risk
operations. They must be treated as sensitive payment-adjacent actions with
their own pre-flight checks, action cards, and approval requirements.

#### 3.3.2 Delegated asset transfers

The Assets pallet supports delegated transfers via `pallet_assets::approve_transfer`:
an account may approve another account to transfer up to a specified amount of
a specific asset. This is the on-chain allowance pattern for stablecoin flows
(e.g., USDT asset 1984, USDC asset 1337 on Asset Hub).

```rust
/// An asset allowance granted via the Assets pallet.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetAllowance {
    /// The asset being allowed.
    pub asset: AssetId,
    /// The account granting the allowance.
    pub owner: AccountId32,
    /// The account receiving the allowance.
    pub delegate: AccountId32,
    /// Maximum amount the delegate may transfer.
    pub amount: u128,
    /// Block at which this allowance was queried.
    pub queried_at_block: H256,
}
```

**Requirement PAY-ALLOW-001:** Delegated asset approvals bound amount and
revocation but do not automatically express Polkagent's time, purpose,
recipient, action-family, or cumulative-budget policy. Polkagent's policy
layer must enforce its own constraints independently of on-chain allowances.

**Requirement PAY-ALLOW-002:** When `pallet_assets::approve_transfer` is used
for agent allowances, the on-chain approved amount must be treated as a ceiling,
not as authorization. Polkagent's per-period budget caps (section 8.2) must
enforce a tighter bound that the policy layer checks independently before
constructing any transfer_approved extrinsic. The on-chain amount and
Polkagent's budget state must be reconciled on each pre-flight.

### 3.4 XCM cross-chain settlement

XCM (Cross-Consensus Messaging) enables asset transfers between Polkadot
chains. It is a message format, not a delivery guarantee.

#### 3.4.1 Cross-chain intent

```rust
/// A cross-chain payment intent.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CrossChainPaymentIntent {
    pub id: IntentId,
    /// Source chain profile.
    pub source: ChainProfile,
    /// Destination chain and location.
    pub destination: VersionedLocation,
    /// Beneficiary on the destination chain.
    pub beneficiary: VersionedLocation,
    /// Asset to transfer, identified by XCM location.
    pub asset: VersionedAsset,
    /// Amount in the asset's smallest unit.
    pub amount: u128,
    /// Fee asset on the source chain.
    pub source_fee_asset: AssetId,
    /// Estimated delivery/execution fee on the destination.
    pub destination_fee_estimate: Option<FeeEstimate>,
    /// XCM version negotiated between source and destination.
    pub xcm_version: u32,
    /// Reserve/teleport transfer type.
    pub transfer_type: XcmTransferType,
    /// Route evidence: channel status, version, runtime support.
    pub route_evidence: RouteEvidence,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum XcmTransferType {
    /// Asset is teleported (burned on source, minted on destination).
    Teleport,
    /// Asset is reserved on source, derivative issued on destination.
    ReserveDeposit,
    /// Asset is withdrawn from reserve on behalf of sender.
    ReserveWithdraw,
}
```

#### 3.4.2 Cross-chain safety requirements

**Requirement PAY-XCM-001:** Default to read/plan/simulate for cross-chain
operations. A real XCM message is at least as consequential as a transfer
and can fail partly or asynchronously across chains.

**Requirement PAY-XCM-002:** Every supported route must have a test path
in Chopsticks/Zombienet plus target-testnet evidence. The system must not
infer that a route works because either chain individually supports the
asset.

**Requirement PAY-XCM-003:** The action card for cross-chain payments must
display: source chain, destination chain, asset identity on both chains,
fees on both chains, transfer type (teleport/reserve), XCM version,
and explicit unknowns about destination execution.

**Requirement PAY-XCM-004:** Cross-chain payment finality requires
observing both source inclusion and, where possible, destination
execution. The receipt must distinguish "source finalized" from
"destination confirmed" and surface partial failure explicitly.

**Requirement PAY-XCM-005:** `DryRunApi` and `XcmPaymentApi` availability
must be probed per profile. Their absence does not block intent construction
but must be surfaced as increased uncertainty in the action card.

### 3.5 Escrow patterns

Escrow holds funds in a controlled state pending a condition. Polkagent
supports escrow through off-chain durable workflow and, in the future,
on-chain contract primitives.

#### 3.5.1 Off-chain escrow workflow

```text
1. Buyer and seller agree on terms (off-chain or via payment request)
2. Buyer funds a controlled account (pure proxy or dedicated account)
3. Escrow policy defines release conditions:
   - Seller delivers evidence (artifact, on-chain state change)
   - Timeout with automatic refund
   - Dispute triggers human/quorum review
4. On condition met: transfer from controlled account to seller
5. On dispute: freeze and escalate
6. Receipt records the full escrow lifecycle
```

**Requirement PAY-ESCROW-001:** Escrow is a phased capability requiring
independent security review. Phase 1 supports only the off-chain workflow
pattern. On-chain contract escrow (PVM/EVM) is Phase 3+ with separate
audit gates.

**Requirement PAY-ESCROW-002:** Escrow timeout behavior must be
deterministic and configured before funds are committed. The system must
not leave funds in an escrow state indefinitely without a recovery path.

#### 3.5.2 Escrow via multisig + time-delay (validate-next)

Without smart contracts, escrow can be approximated on Polkadot using a
multisig account (requiring n-of-m signatories to release funds) combined
with an announcement-delay proxy. The pattern is:

1. Buyer and seller are both signatories; a neutral or time-delay account
   holds funds.
2. Release requires both parties (or a threshold) to sign `multisig.approveAsMulti`.
3. A proxy announcement delay gives both parties a window to abort.
4. Timeout refund is a separate multisig call that becomes valid after a
   configured block height.

This approach eliminates smart-contract risk at the cost of requiring
co-operation for release. It is validated for Phase 2 as the escrow mechanism
for high-value agent-to-agent transactions. Smart-contract-based escrow
(PVM/EVM) remains Phase 3+. Streaming/metered payments that depend on
continuous escrow settlement are deferred past Phase 3 (see section 3.7).

### 3.6 Subscriptions and recurring payments

Recurring payments execute at intervals under a configured mandate.

```rust
/// A recurring payment schedule.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecurringPayment {
    pub id: RecurringPaymentId,
    /// The mandate that authorizes this schedule.
    pub mandate_id: MandateId,
    /// Recipient account.
    pub recipient: AccountId32,
    /// Asset and amount per interval.
    pub asset: AssetId,
    pub amount_per_interval: u128,
    /// Payment interval.
    pub interval: Duration,
    /// Maximum total payments (None = unlimited within mandate budget).
    pub max_occurrences: Option<u32>,
    /// Completed payment count.
    pub completed_count: u32,
    /// Next scheduled execution.
    pub next_execution: DateTime<Utc>,
    /// Whether the schedule is active.
    pub status: RecurringStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RecurringStatus {
    Active,
    Paused { reason: String },
    Completed,
    Cancelled { reason: String },
    Failed { last_error: String },
}
```

**Requirement PAY-RECUR-001:** Recurring payments require a Tier 2 or Tier 3
autonomy configuration (section 7) with explicit budget, recipient allowlist,
and interval bounds.

**Requirement PAY-RECUR-002:** Each recurring payment execution produces an
independent receipt. Failed executions must pause the schedule and notify
the owner rather than silently retrying.

**Requirement PAY-RECUR-003:** The owner must be able to pause, resume,
modify, or cancel a recurring payment at any time. Modification takes
effect before the next scheduled execution.

### 3.7 Streaming and metered payments

Streaming payments release value continuously or in fine-grained increments,
useful for compute billing, service metering, or time-based compensation.

```rust
/// A streaming payment configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamingPayment {
    pub id: StreamingPaymentId,
    pub mandate_id: MandateId,
    pub recipient: AccountId32,
    pub asset: AssetId,
    /// Total budget for the stream.
    pub total_budget: u128,
    /// Rate per second in the asset's smallest unit.
    pub rate_per_second: u128,
    /// Amount already disbursed.
    pub disbursed: u128,
    /// Minimum disbursement batch size (to amortize fees).
    pub min_disbursement: u128,
    /// Stream lifecycle.
    pub status: StreamStatus,
    pub started_at: DateTime<Utc>,
    pub last_disbursement_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StreamStatus {
    Active,
    Paused,
    Completed,
    Cancelled { disbursed_to_recipient: u128, returned_to_sender: u128 },
}
```

**Requirement PAY-STREAM-001:** Streaming payments are deferred past Phase 3.
Research has confirmed that on-chain streaming without smart contracts requires
either high transaction volume (amortized fees become prohibitive) or a trusted
batching layer, both of which need independent security review and settlement
reconciliation tests. This capability is moved to the defer queue: it must not
be designed into Phase 1-3 APIs in ways that pre-commit an approach. Revisit
when PVM contract support matures or a native Polkadot streaming pallet exists.

**Requirement PAY-STREAM-002:** The system must batch on-chain disbursements
to amortize transaction fees. The batch size and interval are configurable
policy parameters. Accrued but undisbursed amounts must be tracked
accurately and displayed to both parties.

**Requirement PAY-STREAM-003:** Stream cancellation must settle the accrued
amount to the recipient and return the remainder to the sender. The
settlement must be atomic or use a two-phase commit with explicit
intermediate states.

### 3.8 Batch operations

Multiple payment operations may be batched into a single extrinsic using
`utility.batch`, `utility.batchAll`, or `utility.forceBatch`.

**Requirement PAY-BATCH-001:** The action card for a batch must flatten and
display every inner operation individually. A batch must not hide operations
from the user or approval system.

**Requirement PAY-BATCH-002:** Batch risk assessment must flag:
- Hidden proxy additions or removals within a batch
- Mixed high-risk and low-risk operations
- Operations that individually pass policy but collectively exceed budget
- Inner calls that the system cannot fully decode

**Requirement PAY-BATCH-003:** When using `batchAll` (atomic), the action
card must explain that all operations succeed or all fail. When using
`batch` (best-effort), partial failure behavior must be displayed.

---

## 4. Explain Before Sign (B1)

Explain Before Sign is the critical first-slice action that proves the
payment architecture. It is the bridge between user intent and external
signer authorization.

### 4.1 Concept

The user selects a supported action. The system binds a chain profile and
metadata, performs pre-flight checks, renders a canonical action card,
then hands the exact payload bytes to an external signer. The model
explains; the card authorizes; the signer signs.

### 4.2 User flow

```text
                          User
                           |
                    "Send 10 DOT to 5F...9q"
                           |
                           v
                  +------------------+
                  | Intent Builder   |
                  | - resolve chain  |
                  | - resolve asset  |
                  | - resolve recip. |
                  | - query state    |
                  +------------------+
                           |
                           v
                  +------------------+
                  | Pre-flight       |
                  | - balance check  |
                  | - address check  |
                  | - fee estimate   |
                  | - ED check       |
                  | - risk gates     |
                  +------------------+
                           |
                    pass   |   fail
                  +--------+--------+
                  |                  |
                  v                  v
         +----------------+   +-----------+
         | Action Card    |   | Refusal   |
         | (canonical)    |   | card with |
         |                |   | reason    |
         | Network: ...   |   +-----------+
         | Action: ...    |
         | Recipient: ... |
         | Amount: ...    |
         | Fee cap: ...   |
         | ED impact: ... |
         | Evidence: ...  |
         | Expiry: ...    |
         +----------------+
         | Model explains |
         | (visually sep) |
         +----------------+
                  |
           user approves
                  |
                  v
         +----------------+
         | Signer Port    |
         | (external)     |
         | - receives     |
         |   canonical    |
         |   payload only |
         | - returns sig  |
         |   or refusal   |
         +----------------+
                  |
                  v
         +----------------+
         | Submitter      |
         | - RPC submit   |
         | - idempotency  |
         +----------------+
                  |
                  v
         +----------------+
         | Finality       |
         | Watcher        |
         | - inclusion    |
         | - finality     |
         | - outcome      |
         +----------------+
                  |
                  v
         +----------------+
         | Receipt        |
         | - intent hash  |
         | - approval ref |
         | - tx hash      |
         | - block        |
         | - outcome      |
         | - evidence     |
         +----------------+
```

### 4.3 Canonical action card

The action card is the single trusted rendering of a payment intent. It is
derived entirely from metadata-decoded, profile-bound data.

```text
+------------------------------------------------------------------+
| PAYMENT INTENT                                    [Polkagent]    |
|------------------------------------------------------------------|
| Network:       Polkadot Hub (genesis 0x91b1...)                  |
| Action:        balances.transferKeepAlive                        |
| Recipient:     5F3s...9q (SS58) / 0x8e...a2 (raw AccountId32)   |
| Asset:         DOT / native balance                              |
| Amount:        10.0000000000 DOT                                 |
| Fee cap:       <= 0.0150 DOT                                    |
| Sender balance: 125.3400 DOT (after: 115.3250 DOT, ED: 1.0 DOT)|
| Metadata:      spec 1002006 / hash 0xab...cd                    |
| Evidence:      balance at block 0x7f...e1, fee at block 0x7f...e1|
| Signer:        external wallet "owner-1"                         |
| Expiry:        10 minutes / 1 submission attempt                 |
+------------------------------------------------------------------+
| Agent explanation:                                               |
| "This sends 10 DOT to Alice's account. Your remaining balance   |
|  of ~115.33 DOT is well above the 1 DOT existential deposit."   |
+------------------------------------------------------------------+
|                    [ Approve ]  [ Reject ]                       |
+------------------------------------------------------------------+
```

**Requirement PAY-CARD-001:** The action card must separate canonical fields
(top section) from model explanation (bottom section) with a visible boundary.
Model text must never appear inside the canonical field area.

**Requirement PAY-CARD-002:** If any canonical field cannot be decoded with
confidence from the current metadata profile, the card must display "unknown"
for that field rather than a model-generated guess.

**Requirement PAY-CARD-003:** The action card must display both SS58 and raw
AccountId32 formats for recipient addresses to help users verify against
multiple sources.

**Requirement PAY-CARD-004:** The action card must show the sender's
post-transfer balance and its relationship to the ED. If the transfer would
bring the balance below ED + keep-alive buffer, display a prominent warning.

### 4.4 Metadata-pinned decoding

Every action card field is derived from the chain's runtime metadata at
a specific version and block.

**Requirement PAY-META-001:** The system must pin metadata by spec version
and metadata hash. A mismatch between the pinned metadata and the
current runtime must block intent construction with a clear error.

**Requirement PAY-META-002:** Call decoding must use the pinned metadata's
type registry. The system must not decode a call using metadata from a
different spec version or chain.

**Requirement PAY-META-003:** For the initial release, support a curated
set of well-known calls (balances.transferKeepAlive,
balances.transferAllowDeath, assets.transfer, assets.transferKeepAlive).
Return an explicit "unsupported call" response for unrecognized calls
rather than attempting best-effort decoding.

### 4.5 Pre-flight checks

See section 3.1.2 for the general pre-flight check table. Additional B1
checks:

**Requirement PAY-PRE-001:** Pre-flight checks must execute deterministically
from profile-bound data. A model instruction, tool result, or chat message
cannot disable or weaken a pre-flight check.

**Requirement PAY-PRE-002:** Pre-flight failures produce a refusal card
(not an action card) that explains the specific failure, the evidence
that caused it, and what the user could do differently.

### 4.6 External signer handoff

The signer receives only what it needs to sign and nothing more.

```rust
/// The payload sent to an external signer.
#[derive(Clone, Debug)]
pub struct SigningRequest {
    /// Unique intent identifier for correlation.
    pub intent_id: IntentId,
    /// The canonical SCALE-encoded extrinsic payload.
    pub payload: Vec<u8>,
    /// Genesis hash for network binding.
    pub genesis_hash: H256,
    /// Block hash for era/mortality binding.
    pub block_hash: H256,
    /// Metadata hash for metadata-aware signers.
    pub metadata_hash: Option<H256>,
    /// Human-readable summary for wallet display.
    pub display_summary: String,
    /// Expiry for this signing request.
    pub expires_at: DateTime<Utc>,
}

/// The response from an external signer.
#[derive(Clone, Debug)]
pub enum SigningResponse {
    Signed {
        /// The signature bytes.
        signature: Vec<u8>,
        /// The signing scheme used.
        scheme: SigningScheme,
        /// The signer's public key.
        signer_public_key: Vec<u8>,
    },
    Refused {
        reason: Option<String>,
    },
    Timeout,
}
```

**Requirement PAY-SIGN-001:** The signing request must correlate to an exact
`IntentId`. The system must reject a signature returned for any other intent.

**Requirement PAY-SIGN-002:** The signing request must include genesis hash
and block hash for network/era binding. Metadata hash is included where the
signer and target runtime support metadata-hash checking.

**Requirement PAY-SIGN-003:** Signing requests must expire. A signature
returned after expiry must be rejected even if cryptographically valid.

### 4.7 Finality observation

After submission, the system watches for inclusion and finality.

```rust
/// Observed outcome states for a submitted extrinsic.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TransactionOutcome {
    /// Submitted to RPC but not yet seen in a block.
    Pending {
        submitted_at: DateTime<Utc>,
        tx_hash: H256,
    },
    /// Included in a block but not yet finalized.
    Included {
        block_hash: H256,
        block_number: u64,
        extrinsic_index: u32,
        dispatch_result: DispatchResult,
        events: Vec<DecodedEvent>,
    },
    /// Finalized -- the block containing this extrinsic is final.
    Finalized {
        block_hash: H256,
        block_number: u64,
        extrinsic_index: u32,
        dispatch_result: DispatchResult,
        events: Vec<DecodedEvent>,
    },
    /// The extrinsic was dropped or replaced without inclusion.
    Dropped {
        reason: Option<String>,
    },
    /// The system cannot determine the outcome.
    Unknown {
        last_check: DateTime<Utc>,
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DispatchResult {
    Success,
    Failed { error: String },
}
```

**Requirement PAY-FIN-001:** The system must distinguish Pending, Included,
Finalized, Dropped, and Unknown as separate states. It must never collapse
Unknown into Failed or Succeeded.

**Requirement PAY-FIN-002:** Finality observation must survive process
restarts. The finality watcher is a durable effect with its own
`EffectAttempt`/`EffectOutcome` lifecycle.

**Requirement PAY-FIN-003:** A payment is not "complete" until finality
is observed or the system explicitly records Unknown with an explanation.
UI must not display a confirmed-looking state during the Pending or
Included phases.

---

## 5. Intent lifecycle (B3)

The intent lifecycle converts a natural-language request into a typed,
policy-evaluated, signer-ready payload.

### 5.1 Lifecycle states

```text
                     +----------+
                     | Drafting |
                     +----+-----+
                          |
                  resolve chain/asset/recipient
                          |
                     +----v-----+
                     | Proposed |
                     +----+-----+
                          |
                   pre-flight checks
                          |
                +----+----+----+----+
                |                   |
           pass |              fail |
                |                   |
           +----v-----+      +-----v----+
           | Verified |      | Refused  |
           +----+-----+      +----------+
                |
         policy evaluation
                |
        +---+---+---+---+
        |       |       |
   allow|  deny |  need |
        |       |  appr |
   +----v---+ +-v----+ +v--------+
   |Ready to| |Denied| |Awaiting |
   | sign   | +------+ |approval |
   +----+---+          +----+----+
        |                   |
        |            approved/denied
        |                   |
        +---<----+----->----+
                 |
          +------v------+
          | Signing     |
          +------+------+
                 |
          signed/refused/timeout
                 |
        +---+----+----+---+
        |        |        |
   +----v---+ +--v---+ +-v------+
   |Submitted| |Sign  | |Sign   |
   +----+----+ |Refuse| |Timeout|
        |      +------+ +-------+
        |
   inclusion/finality
        |
   +----v------+
   | Finalized |  (or Dropped / Unknown)
   +----+------+
        |
   +----v------+
   | Receipted |
   +-----------+
```

### 5.2 Intent data structure

```rust
/// A payment intent through its full lifecycle.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaymentIntent {
    pub id: IntentId,
    /// The chain profile this intent is bound to.
    pub chain_profile: ChainProfile,
    /// The specific action being proposed.
    pub action: PaymentAction,
    /// Sender account.
    pub sender: AccountId32,
    /// Pre-flight evidence collected during verification.
    pub preflight_evidence: PreflightEvidence,
    /// Fee estimate and cap.
    pub fee: FeeEstimate,
    /// The resolved grant that authorizes this intent.
    pub grant: Option<ResolvedGrant>,
    /// Current lifecycle state.
    pub status: IntentStatus,
    /// Idempotency key to prevent duplicate effects.
    pub idempotency_key: IdempotencyKey,
    /// When this intent expires.
    pub expires_at: DateTime<Utc>,
    /// Audit trail of state transitions.
    pub history: Vec<IntentEvent>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PaymentAction {
    NativeTransfer {
        recipient: AccountId32,
        amount: u128,
        keep_alive: bool,
    },
    AssetTransfer {
        asset: AssetId,
        recipient: AccountId32,
        amount: u128,
        keep_alive: bool,
    },
    CrossChainTransfer(CrossChainPaymentIntent),
    Batch {
        operations: Vec<PaymentAction>,
        batch_type: BatchType,
    },
    ProxyCall {
        real_account: AccountId32,
        proxy_type: String,
        inner_action: Box<PaymentAction>,
    },
    // Future action types added here.
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BatchType {
    /// All succeed or all fail.
    Atomic,
    /// Best effort; partial failure possible.
    BestEffort,
    /// Force execute all; ignores individual failures.
    Force,
}
```

### 5.3 Idempotency

**Requirement PAY-IDEMP-001:** Every intent has a stable idempotency key
generated at creation time. A restart, retry, or reconnection must not
create a second intent for the same logical request.

**Requirement PAY-IDEMP-002:** The submission effect uses the idempotency
key to prevent duplicate RPC submissions. If the system crashes between
signing and submission confirmation, recovery must check for an existing
on-chain inclusion before re-submitting.

**Requirement PAY-IDEMP-003:** A crashed process resuming must scan for
intents in Signing or Submitted states and attempt to determine their
outcome before creating new intents from the same conversation.

### 5.4 Cancellation

**Requirement PAY-CANCEL-001:** An intent may be cancelled at any state
before Submitted. After submission, the intent cannot be cancelled
on-chain (Polkadot does not support transaction cancellation in the
general case).

**Requirement PAY-CANCEL-002:** Cancellation of an intent in AwaitingApproval
state must release any held resources and update projections immediately.

**Requirement PAY-CANCEL-003:** If an intent expires before signing, the
system must automatically transition it to Refused with an expiry reason
rather than leaving it in a limbo state.

---

## 6. Pre-sign risk gates (B4)

Risk gates are deterministic checks that flag dangerous patterns in
proposed actions before they reach the signer.

### 6.1 Risk patterns

| Pattern | Detection | Action |
|---|---|---|
| **Batch hiding** | `batchAll` contains a `proxy.addProxy` or `proxy.removeProxy` alongside innocuous transfers | Flatten batch; highlight proxy operations prominently |
| **Near-duplicate recipient** | Recipient address differs by 1-2 characters from a known/recent address | Warn with visual diff |
| **Homoglyph address** | Recipient SS58 contains visually similar characters to a known address | Warn with character-level comparison |
| **Unusual fee** | Estimated fee exceeds 10x historical median for this call type | Warn with fee comparison |
| **Stale metadata** | Runtime spec version has changed since profile was pinned | Block intent; require profile refresh |
| **Account reaping** | Transfer would bring sender below ED | Block (keep-alive) or warn with explicit acknowledgment (allow-death) |
| **Self-transfer** | Sender and recipient are the same account | Warn; allow override with acknowledgment |
| **Large value** | Transfer exceeds operator-configured threshold | Require additional approval or cooling period |
| **First-time recipient** | Recipient has never received funds from this sender | Inform; not block |
| **Proxy nesting** | Nested proxy/batch/multisig calls create deep dispatch chains | Flatten and display full call graph |
| **Unknown call** | Inner call cannot be decoded against current metadata | Block; refuse to construct intent |

### 6.2 Risk gate requirements

**Requirement PAY-RISK-001:** Risk gates execute on the decoded, typed
intent -- not on raw bytes or model text. They are deterministic functions
of profile-bound data.

**Requirement PAY-RISK-002:** Risk gates cannot be disabled by model
instruction, tool output, or chat message. Operator-configured policy
may adjust thresholds but cannot remove structural gates (batch flattening,
metadata freshness, decode completeness).

**Requirement PAY-RISK-003:** Each risk gate produces a typed finding that
is included in the action card. Findings are categorized as Block (refuse
intent), Warn (display prominently, allow override), or Inform
(display without requiring action).

**Requirement PAY-RISK-004:** The risk gate corpus must include adversarial
test fixtures: batch hiding a `proxy.addProxy`, near-duplicate addresses,
stale metadata, malicious token/NFT names in memo fields, and call-index
drift after runtime upgrade.

---

## 7. Autonomy tiers

Autonomy tiers define how much independent authority an agent has over
payment operations. The tiers form a ladder from zero authority to full
autonomous operation.

### 7.1 Tier definitions

#### Tier 0: Read-only (propose only)

The agent can read chain state, explain transactions, produce research
and analysis, and draft intents. It has zero write authority. It cannot
sign, submit, or authorize any on-chain effect.

| Property | Value |
|---|---|
| Chain reads | Allowed |
| Intent drafting | Allowed (produces action cards for review) |
| Signing | Denied |
| Submission | Denied |
| Budget | None (no spend authority) |
| Default for | New agents, unknown integrations |

**Configuration UX:** This is the default. No configuration needed.

#### Tier 1: Per-action approval

The agent can construct verified intents and present action cards. Each
action requires explicit human approval before the signer is invoked.

| Property | Value |
|---|---|
| Chain reads | Allowed |
| Intent construction | Allowed with full pre-flight checks |
| Action card | Presented to user for each action |
| Signing | Only after user approves specific action card |
| Submission | Only after signing succeeds |
| Budget | Optional per-action and cumulative limits |
| Default for | First-time payment-enabled agents |

**Configuration UX:** Enable payment capability for specific action
families (e.g., native transfers). Configure signer. Each action shows
an action card and waits for approval.

```toml
[agent.capabilities.payments]
enabled = true
tier = "per_action"
allowed_actions = ["native_transfer", "asset_transfer"]
signer = { type = "external_wallet", label = "owner-1" }
```

#### Tier 2: Policy-bounded autonomous

The agent can execute actions autonomously within a configured mandate.
Actions within policy proceed without per-action approval. Actions outside
policy are either denied or escalated to human approval.

| Property | Value |
|---|---|
| Chain reads | Allowed |
| Intent construction | Allowed |
| Policy evaluation | Automatic; mandate defines boundaries |
| Signing | Automatic for within-policy actions |
| Submission | Automatic for within-policy actions |
| Budget | Required: per-action, rolling, and lifetime limits |
| Recipient allowlist | Required |
| Rate limiting | Required |
| Emergency controls | Required: pause, revoke, circuit breaker |
| Default for | Configured operational agents |

**Configuration UX:** Explicit mandate configuration with visible
consequence summary.

```toml
[agent.capabilities.payments]
enabled = true
tier = "policy_bounded"

[agent.capabilities.payments.mandate]
id = "ops-mandate-001"

# Action scope
allowed_actions = ["native_transfer", "asset_transfer"]
allowed_chains = ["0x91b1..."]  # genesis hashes
allowed_assets = ["native", "asset:1984"]  # USDT on Hub
allowed_recipients = [
    "5F3s...9q",  # treasury
    "5Gx7...2a",  # payroll
]

# Budget limits
per_action_limit = { amount = "100_000_000_000", asset = "native" }  # 10 DOT
rolling_limit = { amount = "1_000_000_000_000", asset = "native", window = "24h" }
lifetime_limit = { amount = "10_000_000_000_000", asset = "native" }

# Rate limits
max_actions_per_hour = 10
max_actions_per_day = 50
cooldown_between_actions = "60s"

# Escalation
out_of_policy = "deny"  # or "escalate_to_human"

# Signer
signer = { type = "proxy", real = "5Abc...xy", proxy_type = "NonTransfer" }

# Emergency
circuit_breaker = { trigger = "3_failures_in_1h", action = "pause_and_notify" }
emergency_contacts = ["operator@example.com"]
```

#### Tier 3: Fully autonomous

The agent operates continuously with a funded account and broad mandate.
No routine human approval is required. Emergency controls, audit, and
signer isolation remain active.

| Property | Value |
|---|---|
| Chain reads | Allowed |
| Intent construction | Allowed |
| Policy evaluation | Broad mandate; minimal restrictions |
| Signing | Automatic via funded agent account |
| Submission | Automatic |
| Budget | Required (may be deliberately large) |
| Recipient allowlist | Optional (may be open) |
| Rate limiting | Required |
| Emergency controls | Required: pause, revoke, circuit breaker, recovery |
| Audit | Required: full receipt trail, real-time notifications |
| Default for | Never (requires explicit, informed configuration) |

**Configuration UX:** Multi-step configuration with consequence review,
confirmation prompt, and cooling period.

```toml
[agent.capabilities.payments]
enabled = true
tier = "fully_autonomous"

[agent.capabilities.payments.mandate]
id = "service-mandate-001"

# Broad action scope
allowed_actions = ["native_transfer", "asset_transfer", "cross_chain_transfer"]
allowed_chains = ["0x91b1...", "0xfc2c..."]
allowed_assets = ["native", "asset:1984", "asset:1337"]
# No recipient allowlist -- open sending

# Budget (deliberately large)
per_action_limit = { amount = "10_000_000_000_000", asset = "native" }  # 1000 DOT
rolling_limit = { amount = "100_000_000_000_000", asset = "native", window = "24h" }
lifetime_limit = { amount = "none" }  # explicit unlimited

# Rate limits (still required)
max_actions_per_hour = 100
max_actions_per_day = 1000
cooldown_between_actions = "10s"

# Funded account
signer = { type = "funded_agent_account", account_id = "agent-ops-1" }

# Auto top-up
[agent.capabilities.payments.auto_topup]
enabled = true
source = "5Abc...xy"  # controller account
threshold = "50_000_000_000"  # 5 DOT
topup_amount = "500_000_000_000"  # 50 DOT
requires_approval = true  # controller must approve top-up

# Emergency controls
circuit_breaker = { trigger = "5_failures_in_1h", action = "pause_and_notify" }
emergency_revoke_key = "5Emr...gk"  # can instantly revoke all mandates
emergency_contacts = ["ops-team@example.com", "ceo@example.com"]
audit_webhook = "https://audit.example.com/polkagent"
```

### 7.2 Tier comparison matrix

| Capability | Tier 0 | Tier 1 | Tier 2 | Tier 3 |
|---|---|---|---|---|
| Read chain state | Yes | Yes | Yes | Yes |
| Draft intents | Yes | Yes | Yes | Yes |
| Action cards | Display only | Approve/reject | Auto within policy | Auto |
| Sign transactions | No | After approval | Auto within policy | Auto |
| Submit transactions | No | After signing | Auto within policy | Auto |
| Budget required | No | Optional | Required | Required |
| Recipient allowlist | N/A | Optional | Required | Optional |
| Rate limits | N/A | Optional | Required | Required |
| Circuit breaker | N/A | Optional | Required | Required |
| Emergency revoke | N/A | Optional | Required | Required |
| Audit trail | Basic | Standard | Full | Full + real-time |
| Signer isolation | N/A | Required | Required | Required |
| Keys outside model | N/A | Required | Required | Required |

### 7.3 Emergency controls

Emergency controls are available at every tier and are mandatory at Tier 2
and above.

#### 7.3.1 Pause

Immediately suspends all autonomous payment activity. Pending approvals are
held. Active submissions continue to finality observation but no new
submissions are made.

**Requirement PAY-EMRG-001:** Pause must take effect within one second of
invocation. It must not wait for in-progress policy evaluations or model
responses.

#### 7.3.2 Revoke

Permanently invalidates a mandate. All pending intents under that mandate
are cancelled. The agent returns to Tier 0 for the revoked action families.

**Requirement PAY-EMRG-002:** Revocation must be available through multiple
channels: CLI, web UI, API, and (for Tier 3) the emergency revoke key
account via an on-chain proxy removal.

#### 7.3.3 Circuit breaker

Automatic pause triggered by anomalous activity patterns.

**Requirement PAY-EMRG-003:** Circuit breaker conditions must be
configurable: consecutive failures, budget consumption rate, unusual
recipient patterns, or operator-defined rules. The default is
`3 failures in 1 hour -> pause and notify`.

#### 7.3.4 Recovery

After an emergency pause or revocation, the owner reviews:
- All pending intents and their states
- All submitted but unfinalized transactions
- The agent's current balance and account state
- The mandate configuration that was active

**Requirement PAY-EMRG-004:** Recovery produces a reconciliation report
linking every intent to its outcome (including Unknown). The owner may
then resume, re-configure, or permanently disable the mandate.

### 7.4 Tier transition requirements

| Transition | Requirements |
|---|---|
| 0 -> 1 | Enable payment capability; configure signer |
| 1 -> 2 | Configure mandate with budget, allowlist, rate limits, circuit breaker; acknowledge consequences |
| 2 -> 3 | Configure funded agent account; review broad mandate consequences; multi-step confirmation with cooling period |
| Any -> 0 | Immediate; revoke all mandates |
| 3 -> 2 | Narrow mandate; disable auto top-up |
| 2 -> 1 | Remove mandate; require per-action approval |

**Requirement PAY-TIER-001:** Tier escalation (toward more autonomy) must
require explicit, informed configuration. The system must display a
consequence summary before each escalation.

**Requirement PAY-TIER-002:** Tier de-escalation (toward less autonomy)
must be immediate and unconditional. An owner can always reduce an agent's
authority instantly.

**Requirement PAY-TIER-003:** No UI may represent "autonomous" as a vague
toggle. Every autonomy configuration must display the specific accounts,
networks, assets, recipients, budgets, and emergency controls that apply.

---

## 8. Funded agent accounts

### 8.1 Account structure

A funded agent account is a Polkadot account that the agent can use for
autonomous operations. It is structured to limit blast radius while
enabling operational independence.

#### 8.1.1 Recommended topology

```text
Controller (human or multisig)
  |
  |-- owns/controls -->  Pure Proxy
  |                         |
  |                         |-- funds flow in -->
  |                         |-- agent operates via sub-proxy -->
  |                         |
  |                    Agent Sub-Proxy
  |                    (narrowly filtered)
  |                         |
  |                         |-- allowed: transfer to allowlisted recipients
  |                         |-- allowed: specific pallet calls per filter
  |                         |-- denied: proxy management
  |                         |-- denied: bonding / unbonding
  |                         |-- denied: governance (unless configured)
  |                         |
  |                         |-- time delay: configurable announcement period
  |                         |-- keep-alive: enforced
  |
  |-- emergency revoke --> removes agent sub-proxy instantly
```

**Why pure proxy?** A pure proxy has no private key of its own. Only its
controller(s) can manage it. This means the agent can operate through
the proxy while the controller retains ultimate authority. Even if the
agent's operational key is compromised, the attacker cannot change the
proxy's controller or drain funds beyond the sub-proxy's filter.

**Custody recommendation (research-validated):** The recommended custody
stack is: pure-proxy (holds funds) + narrowly filtered sub-proxy (agent
operational key) + per-period budget caps enforced by the policy layer.
For high-value operations (operator-defined threshold), the sub-proxy
filter must require co-signing via a multisig account rather than
allowing unilateral agent signing. This provides a backstop independent
of the software policy layer: even a fully compromised agent process
cannot sign high-value transfers without the second multisig signatory.
The `proxy_filter` field in `PureProxyWithSubProxy` (section 8.1.2) must
be set to a runtime-verified type that excludes proxy management, bonding,
and governance from the agent's permitted call set.

#### 8.1.2 Account structure types

```rust
/// Supported agent account structures.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentAccount {
    pub id: AgentAccountId,
    pub label: String,
    /// The account structure.
    pub structure: AgentAccountStructure,
    /// Current known balance (block-pinned).
    pub balance: Option<BalanceSnapshot>,
    /// Budget configuration.
    pub budget: BudgetConfig,
    /// Auto top-up rules.
    pub auto_topup: Option<AutoTopupConfig>,
    /// Status.
    pub status: AgentAccountStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentAccountStructure {
    /// Pure proxy controlled by a human/multisig with an agent sub-proxy.
    PureProxyWithSubProxy {
        /// The pure proxy account (holds funds).
        pure_proxy: AccountId32,
        /// The controller account(s).
        controllers: Vec<AccountId32>,
        /// The agent's operational sub-proxy.
        agent_proxy: AccountId32,
        /// The proxy type filter applied to the agent.
        proxy_filter: String,
        /// Announcement delay in blocks.
        announcement_delay: u32,
    },
    /// Multisig-controlled account with agent as one signatory.
    MultisigControlled {
        /// The multisig account.
        multisig: AccountId32,
        /// All signatories.
        signatories: Vec<AccountId32>,
        /// Required threshold.
        threshold: u16,
        /// Which signatory is the agent.
        agent_signatory: AccountId32,
    },
    /// Direct funded account (highest risk, simplest structure).
    /// The agent holds operational keys for this account.
    DirectFunded {
        account: AccountId32,
        /// Recovery/backup account.
        recovery_account: Option<AccountId32>,
    },
}
```

### 8.2 Budget limits and rate limiting

```rust
/// Budget configuration for a funded agent account.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Maximum spend per individual action.
    pub per_action: Option<BudgetLimit>,
    /// Maximum spend in a rolling time window.
    pub rolling: Option<RollingBudget>,
    /// Maximum lifetime spend.
    pub lifetime: Option<BudgetLimit>,
    /// Maximum number of actions per time window.
    pub rate_limit: Option<RateLimit>,
    /// Minimum time between actions.
    pub cooldown: Option<Duration>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BudgetLimit {
    /// Maximum amount in the asset's smallest unit.
    pub amount: u128,
    /// The asset this limit applies to.
    pub asset: AssetId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RollingBudget {
    pub limit: BudgetLimit,
    /// Time window for the rolling limit.
    pub window: Duration,
    /// Current spend in the window.
    pub current_spend: u128,
    /// Window start time.
    pub window_start: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RateLimit {
    pub max_actions: u32,
    pub window: Duration,
    pub current_count: u32,
    pub window_start: DateTime<Utc>,
}
```

**Requirement PAY-BUDGET-001:** Budget checks must occur at intent
construction time (optimistic) and again immediately before signing
(authoritative). A budget exceeded between construction and signing
must block the signing request.

**Requirement PAY-BUDGET-002:** Budget state must be durable and survive
process restarts. After a crash, the system must reconstruct budget
consumption from the receipt/intent store before permitting new actions.

**Requirement PAY-BUDGET-003:** When a rolling budget window expires, the
system resets the window counter. It must not accumulate unspent budget
across windows.

### 8.3 Automatic top-up

```rust
/// Automatic top-up configuration for agent accounts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AutoTopupConfig {
    /// Source account for top-up funds.
    pub source: AccountId32,
    /// When balance falls below this threshold, trigger top-up.
    pub threshold: u128,
    /// Amount to transfer during top-up.
    pub topup_amount: u128,
    /// Whether top-up requires controller approval.
    pub requires_approval: bool,
    /// Maximum number of top-ups per day.
    pub max_daily_topups: u32,
    /// Asset for top-up (typically native balance).
    pub asset: AssetId,
}
```

**Requirement PAY-TOPUP-001:** Auto top-up is a separate payment operation
with its own intent, approval (if required), and receipt. It follows the
same lifecycle as any other payment.

**Requirement PAY-TOPUP-002:** Auto top-up requires explicit configuration.
It is never enabled by default. The source account owner must authorize
the top-up arrangement.

**Requirement PAY-TOPUP-003:** Auto top-up has its own rate limit
(`max_daily_topups`) independent of the agent's operational rate limits.
This prevents a runaway agent from draining the source account through
rapid top-up requests.

### 8.4 Recovery and revocation

**Requirement PAY-RECOVER-001:** The controller must be able to revoke the
agent's proxy access at any time. On-chain proxy removal is immediate and
does not require the agent's cooperation.

**Requirement PAY-RECOVER-002:** After revocation, the controller can
recover all funds from the pure proxy through their own remaining proxy
relationship.

**Requirement PAY-RECOVER-003:** The system must detect proxy revocation
(via chain event subscription) and immediately transition the agent to
Tier 0 with an appropriate notification.

**Requirement PAY-RECOVER-004:** Key rotation for agent operational keys
must be supported without losing the proxy relationship. The procedure
is: add new proxy, verify new proxy works, remove old proxy.

---

## 9. Agent-to-agent commerce

### 9.1 x402

x402 is an HTTP-based payment protocol designed for machine-to-machine
transactions. It uses HTTP 402 responses to request payment before serving
content.

**Current status (as of 2026-07-30):**
- x402 is designed to be extensible by `(scheme, network)` pair.
- Existing implementations use EVM `exact` flow (ERC-20 on Ethereum-compatible chains).
- No shipped native-Polkadot (sr25519) x402 implementation was verified.
- A Revive/EVM account path on Polkadot Hub is a plausible integration.
- A native Polkadot scheme would require a separate specification, verifier,
  facilitator, replay model, wallet support, and conformance suite.

**Non-EVM scheme over Asset Hub stablecoins (validate-next):** Research has
identified x402 as a candidate machine-to-machine payment protocol for
tool-consumption billing on Polkagent, operating over Asset Hub stablecoins
(USDT 1984 / USDC 1337) rather than EVM tokens. This would define a new
`(scheme="polkadot-asset-hub", network="polkadot")` x402 extension with:
- An sr25519-signed payment attestation replacing the EVM signature.
- Settlement via `pallet_assets::transfer_keep_alive` on Asset Hub.
- A verifier that checks the attestation and confirms on-chain inclusion
  before releasing the resource.
This path is placed in the validate-next queue: it requires a written scheme
specification, a facilitator design, replay/nonce handling for sr25519, and
conformance tests before any production integration.

**Requirement PAY-X402-001:** Polkagent must represent x402 through a
generic payment-protocol adapter, not as a core dependency. The adapter
must refuse unrecognized or stale profiles.

**Requirement PAY-X402-002:** x402 integration requires explicit
replay/expiry/facilitator-trust/resource-binding/settlement-finality
testing against the selected scheme's official conformance vectors plus
adversarial race conditions.

### 9.2 ACP (Agent Commerce Protocol)

**Current status:** Stability and changelog not verified as of 2026-07-29.
Polkagent should monitor ACP development but must not claim compatibility
until schemas, signatures, revocation, replay, and settlement conformance
are pinned.

### 9.3 AP2

**Current status (as of 2026-07-30):** AP2 defines a mandate chain: the
human owner issues an **Intent** (what they want done), the agent constructs
a **Cart** (the specific actions and costs), and the owner issues a **Payment**
authorization bound to that Cart. This three-step mandate chain is a useful
design model for Polkagent's own authorization flow, independent of whether
AP2 itself becomes an interoperable standard.

Mapping to Polkagent concepts:
- AP2 Intent -> Polkagent `EffectIntent` / user request
- AP2 Cart -> Polkagent `PaymentIntent` + action card (the canonical, priced
  representation that the user reviews)
- AP2 Payment -> Polkagent approval evidence (`HumanApproval` or
  `MandateAuthorized` in `AuthorizationEvidence`)

No stable interoperable AP2 profile or Polkadot implementation was verified.
The mandate chain model is adopted as a design reference; actual AP2 wire
compatibility is tracked as research.

**Requirement PAY-A2A-001:** Agent-to-agent commerce standards (x402, ACP,
AP2) are interoperability research inputs, not Polkadot standards. Each
requires its own adapter with independent conformance testing.

**Requirement PAY-A2A-002:** Native Polkadot payment mandates (using
proxy/delegation mechanisms) remain a separate, primary design path.
External standards complement but do not replace native settlement.

### 9.4 Agent commerce lifecycle

When agent-to-agent commerce matures, the lifecycle is:

```text
1. Service discovery
   - Agent publishes capabilities, pricing, and payment terms
   - Buyer discovers and evaluates service

2. Quote and authorization
   - Buyer requests quote
   - Seller responds with price, payment rail, and terms
   - Buyer's policy evaluates quote against budget and mandate

3. Payment
   - Buyer constructs payment intent per agreed terms
   - Payment executes through the selected rail
   - Receipt generated

4. Service delivery
   - Seller verifies payment receipt
   - Seller delivers service/artifact
   - Buyer verifies delivery

5. Settlement and reconciliation
   - Both parties record the exchange
   - Dispute resolution if needed
```

**Requirement PAY-COMMERCE-001:** Agent-to-agent commerce is a Phase 4+
research capability. It requires legal review, settlement reconciliation,
abuse response, audit trail, and exit plan before any real-value pilot.

---

## 10. Receipts and reconciliation (B8)

### 10.1 Receipt structure

A receipt is the durable, exportable artifact that proves what happened
with a payment.

```rust
/// A complete payment receipt.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaymentReceipt {
    pub id: ReceiptId,
    /// The intent this receipt documents.
    pub intent_id: IntentId,
    /// Summary of the payment action.
    pub action_summary: PaymentActionSummary,

    // --- Authorization evidence ---
    /// How this payment was authorized.
    pub authorization: AuthorizationEvidence,

    // --- Execution evidence ---
    /// The signed extrinsic hash.
    pub tx_hash: Option<H256>,
    /// Submission timestamp.
    pub submitted_at: Option<DateTime<Utc>>,
    /// Final outcome.
    pub outcome: TransactionOutcome,

    // --- Chain evidence ---
    /// Block hash where the extrinsic was finalized.
    pub finalized_block: Option<H256>,
    /// Block number.
    pub finalized_block_number: Option<u64>,
    /// Relevant decoded events from the block.
    pub events: Vec<DecodedEvent>,
    /// Actual fee paid (from events).
    pub actual_fee: Option<u128>,

    // --- Metadata ---
    /// Chain profile at time of execution.
    pub chain_profile_snapshot: ChainProfile,
    /// Agent that executed this payment.
    pub agent_id: AgentId,
    /// Run/conversation context.
    pub run_id: Option<RunId>,
    /// Tags for organization/search.
    pub tags: Vec<String>,
    /// Custom metadata for reconciliation.
    pub custom_fields: HashMap<String, String>,

    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AuthorizationEvidence {
    /// Human approved via action card.
    HumanApproval {
        approved_at: DateTime<Utc>,
        approval_id: ApprovalId,
    },
    /// Mandate-authorized (autonomous).
    MandateAuthorized {
        mandate_id: MandateId,
        policy_snapshot: ResolvedGrant,
    },
    /// Quorum/multisig approval.
    QuorumApproval {
        approvers: Vec<AccountId32>,
        threshold: u16,
        approval_id: ApprovalId,
    },
}
```

### 10.2 Receipt requirements

**Requirement PAY-RCPT-001:** Every payment effect -- successful, failed,
or unknown -- must produce a receipt. There is no "silent" payment.

**Requirement PAY-RCPT-002:** Receipts must be immutable after creation.
Corrections are recorded as separate linked artifacts, not as mutations
to existing receipts.

**Requirement PAY-RCPT-003:** Receipts must correctly represent failed and
unknown cases. A receipt for a failed payment must include the error. A
receipt for an unknown outcome must state what is unknown and what the
system last observed.

**Requirement PAY-RCPT-004:** Receipts must be exportable in a structured
format (JSON, CSV) for external accounting and reconciliation systems.

### 10.3 Reconciliation

Reconciliation is the process of matching intent records against on-chain
evidence to ensure consistency.

```text
Reconciliation workflow:

1. Periodic scan
   - Query all intents in Submitted or Unknown state
   - Check on-chain state for each

2. Match
   - For each submitted intent, search for the tx hash on-chain
   - Verify block, events, and dispatch result

3. Resolve
   - Submitted + found finalized = update to Finalized, generate receipt
   - Submitted + found failed = update to Failed, generate receipt
   - Submitted + not found + timeout exceeded = mark Unknown
   - Unknown + found on rescan = resolve to actual outcome

4. Report
   - Generate reconciliation report
   - Flag discrepancies (intent says success, chain says failure)
   - Alert on unresolved Unknown states
```

**Requirement PAY-RECON-001:** Reconciliation must run automatically on a
configurable schedule (default: every 10 minutes for active agents).

**Requirement PAY-RECON-002:** Reconciliation must detect and flag
discrepancies between Polkagent's recorded state and on-chain evidence.
A discrepancy is a potential security or correctness issue and must be
surfaced to the operator.

**Requirement PAY-RECON-003:** The reconciliation process must be
idempotent. Running it multiple times on the same data must produce the
same result.

---

## 11. Accounting architecture

### 11.1 Per-agent accounting

Every agent maintains its own accounting ledger that tracks:

- Intents created and their outcomes
- Receipts and their linked intents
- Budget consumption (per-action, rolling, lifetime)
- Fees paid
- Top-up events

```rust
/// Per-agent accounting summary.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentAccounting {
    pub agent_id: AgentId,
    /// Total number of payment intents created.
    pub total_intents: u64,
    /// Intents by final status.
    pub intents_by_status: HashMap<String, u64>,
    /// Total value transferred (by asset).
    pub total_transferred: HashMap<AssetId, u128>,
    /// Total fees paid (by asset).
    pub total_fees: HashMap<AssetId, u128>,
    /// Current budget consumption.
    pub budget_consumption: BudgetConsumption,
    /// Last reconciliation timestamp.
    pub last_reconciliation: Option<DateTime<Utc>>,
}
```

### 11.2 Per-run accounting

Each agent run (conversation, trigger, scheduled task) tracks its own
payment activity for attribution and debugging.

**Requirement PAY-ACCT-001:** Every payment intent must be attributed to
a specific run. Cross-run budget consumption must be aggregated at the
agent level.

### 11.3 Per-tenant accounting

In multi-tenant deployments, accounting is isolated per tenant.

**Requirement PAY-ACCT-002:** Tenant accounting boundaries must be
enforced in the data layer. An agent in one tenant must not be able
to read or affect payment state in another tenant.

**Requirement PAY-ACCT-003:** Accounting data must support export in
standard formats for external financial systems. The export must include
sufficient detail for audit and reconciliation but must exclude signing
keys, raw payloads, and other sensitive material.

---

## 12. Payment state machine

The payment state machine governs the lifecycle of every payment operation.

### 12.1 State diagram

```text
                    +----------+
              +---->| DRAFTING |
              |     +----+-----+
              |          |
              |    chain/asset resolved
              |          |
              |     +----v-----+
              |     | PROPOSED |
              |     +----+-----+
              |          |
              |    pre-flight checks
              |          |
              +--+  +----+----+  +--+
              |  |  |         |  |  |
         fail |  |  | pass    |  |  | pre-flight
              |  |  |         |  |  | error
              v  |  v         v  |  v
        +------+ | +--------+ | +--------+
        |REFUSED| | |VERIFIED| | |ERROR   |
        +------+ | +----+---+ | +--------+
                 |      |      |
                 | policy eval |
                 |      |      |
            +----+----+-+--+---+----+
            |         |            |
       allow|    deny |     need   |
            |         |     approval|
       +----v---+ +---v--+ +------v------+
       |READY   | |DENIED| |AWAITING     |
       |TO_SIGN | +------+ |APPROVAL     |
       +----+---+          +------+------+
            |                     |
            |              approved|denied
            |                     |
            +-----<----+---->-----+
                       |
                +------v------+
                | SIGNING     |
                +------+------+
                       |
              +--------+--------+--------+
              |                 |        |
         signed            refused   timeout
              |                 |        |
        +-----v-----+    +-----v-+  +---v-----+
        | SUBMITTED  |    |SIGN   |  |SIGN     |
        +-----+------+    |REFUSED|  |TIMEOUT  |
              |            +-------+  +---------+
              |
     +--------+--------+--------+
     |                  |        |
  included          dropped   timeout
     |                  |        |
+----v-----+      +-----v-+  +--v------+
| INCLUDED |      |DROPPED|  |TX       |
+----+-----+      +-------+  |TIMEOUT  |
     |                        +--+------+
     |                           |
  finalized                  reconciliation
     |                           |
+----v------+             +------v------+
| FINALIZED |             | UNKNOWN     |
+----+------+             +------+------+
     |                           |
+----v------+             later resolution
| RECEIPTED |                    |
+-----------+             +------v------+
                          | RECEIPTED   |
                          +-------------+
```

### 12.2 State transition rules

| From | To | Trigger | Conditions |
|---|---|---|---|
| DRAFTING | PROPOSED | Chain/asset/recipient resolved | All fields populated |
| PROPOSED | VERIFIED | Pre-flight checks pass | All checks green |
| PROPOSED | REFUSED | Pre-flight checks fail | Any blocking check fails |
| PROPOSED | ERROR | System error during pre-flight | Infrastructure failure |
| VERIFIED | READY_TO_SIGN | Policy allows | Grant resolved, no approval needed |
| VERIFIED | DENIED | Policy denies | Action outside all grants |
| VERIFIED | AWAITING_APPROVAL | Policy requires approval | Human/quorum approval needed |
| AWAITING_APPROVAL | READY_TO_SIGN | Approval received | Valid approval for this intent |
| AWAITING_APPROVAL | DENIED | Approval denied | Human/quorum rejects |
| AWAITING_APPROVAL | REFUSED | Approval timeout | Approval expired |
| READY_TO_SIGN | SIGNING | Signing request sent | Signer invoked |
| SIGNING | SUBMITTED | Signature received and submitted | Valid signature, RPC accepted |
| SIGNING | SIGN_REFUSED | Signer refused | Signer returned refusal |
| SIGNING | SIGN_TIMEOUT | Signing timeout | Signer did not respond in time |
| SUBMITTED | INCLUDED | Extrinsic in block | Block inclusion observed |
| SUBMITTED | DROPPED | Extrinsic dropped | RPC reports drop/replacement |
| SUBMITTED | TX_TIMEOUT | Submission timeout | No inclusion after threshold |
| TX_TIMEOUT | UNKNOWN | Reconciliation inconclusive | Cannot determine outcome |
| INCLUDED | FINALIZED | Block finalized | Finality observed |
| FINALIZED | RECEIPTED | Receipt generated | Immutable receipt created |
| UNKNOWN | RECEIPTED | Later resolution | Reconciliation finds outcome |

### 12.3 Terminal states

Terminal states are: REFUSED, DENIED, ERROR, SIGN_REFUSED, SIGN_TIMEOUT,
DROPPED, RECEIPTED.

UNKNOWN is quasi-terminal: it persists until reconciliation resolves it or
the operator manually closes it.

**Requirement PAY-SM-001:** State transitions must be atomic with durable
storage. A crash must not leave the state machine in an inconsistent state.

**Requirement PAY-SM-002:** Every state transition must be recorded in the
intent's history with a timestamp, reason, and relevant evidence.

**Requirement PAY-SM-003:** No state transition may skip intermediate states.
For example, an intent cannot go from PROPOSED directly to SUBMITTED without
passing through VERIFIED and READY_TO_SIGN.

---

## 13. Rust traits and types

### 13.1 Core payment traits

```rust
/// Constructs payment intents from user requests.
#[async_trait]
pub trait IntentBuilder: Send + Sync {
    /// Build a payment intent from a typed action request.
    async fn build(
        &self,
        action: PaymentAction,
        sender: AccountId32,
        profile: &ChainProfile,
        options: IntentOptions,
    ) -> Result<PaymentIntent, IntentError>;
}

/// Evaluates pre-flight checks on a proposed intent.
#[async_trait]
pub trait PreflightChecker: Send + Sync {
    /// Run all pre-flight checks and return findings.
    async fn check(
        &self,
        intent: &PaymentIntent,
    ) -> Result<PreflightResult, PreflightError>;
}

/// Evaluates risk patterns on a verified intent.
pub trait RiskGate: Send + Sync {
    /// Evaluate risk patterns and return findings.
    fn evaluate(
        &self,
        intent: &PaymentIntent,
    ) -> Vec<RiskFinding>;
}

/// Manages the payment intent lifecycle.
#[async_trait]
pub trait PaymentService: Send + Sync {
    /// Create a new payment intent.
    async fn create_intent(
        &self,
        action: PaymentAction,
        sender: AccountId32,
        profile: &ChainProfile,
    ) -> Result<PaymentIntent, PaymentError>;

    /// Advance an intent through the state machine.
    async fn advance(
        &self,
        intent_id: &IntentId,
    ) -> Result<PaymentIntent, PaymentError>;

    /// Cancel an intent (if in a cancellable state).
    async fn cancel(
        &self,
        intent_id: &IntentId,
        reason: String,
    ) -> Result<(), PaymentError>;

    /// Get the current state of an intent.
    async fn get_intent(
        &self,
        intent_id: &IntentId,
    ) -> Result<PaymentIntent, PaymentError>;

    /// List intents matching a filter.
    async fn list_intents(
        &self,
        filter: IntentFilter,
    ) -> Result<Vec<PaymentIntent>, PaymentError>;
}
```

### 13.2 Signer trait

```rust
/// An isolated signer that never exposes key material.
#[async_trait]
pub trait Signer: Send + Sync {
    /// Request a signature for a canonical payload.
    async fn sign(
        &self,
        request: SigningRequest,
    ) -> Result<SigningResponse, SignerError>;

    /// Get the signer's public key without exposing private material.
    async fn public_key(&self) -> Result<Vec<u8>, SignerError>;

    /// Get the signer's account ID.
    async fn account_id(&self) -> Result<AccountId32, SignerError>;

    /// Get signer capabilities (scheme, metadata-hash support, etc.).
    fn capabilities(&self) -> SignerCapabilities;
}

/// Signer implementations.
pub enum SignerImpl {
    /// Watch-only: can draft and simulate but cannot sign.
    WatchOnly,
    /// External wallet via browser extension or mobile.
    ExternalWallet(ExternalWalletSigner),
    /// Intent export: produces a signable payload for offline signing.
    IntentExporter(IntentExporterSigner),
    /// Proxy signer: signs proxy dispatch calls via an agent-held key.
    ProxySigner(ProxySignerConfig),
    /// Hardware wallet (Ledger, Polkadot Vault).
    HardwareWallet(HardwareWalletConfig),
    /// Managed KMS/HSM.
    ManagedKms(KmsConfig),
    /// MPC/threshold.
    Threshold(ThresholdConfig),
}
```

### 13.3 Budget and mandate traits

```rust
/// Evaluates whether an action is within a configured mandate.
#[async_trait]
pub trait MandateEvaluator: Send + Sync {
    /// Check if the given intent is authorized by any active mandate.
    async fn evaluate(
        &self,
        intent: &PaymentIntent,
        agent_id: &AgentId,
    ) -> Result<MandateDecision, MandateError>;
}

#[derive(Clone, Debug)]
pub enum MandateDecision {
    /// Action is within mandate; proceed without human approval.
    Authorized {
        mandate_id: MandateId,
        grant: ResolvedGrant,
    },
    /// Action is outside all mandates; deny.
    Denied {
        reason: String,
    },
    /// Action is outside mandate but escalation is configured.
    EscalateToHuman {
        reason: String,
    },
}

/// Tracks budget consumption.
#[async_trait]
pub trait BudgetTracker: Send + Sync {
    /// Record a spend against the budget.
    async fn record_spend(
        &self,
        agent_id: &AgentId,
        mandate_id: &MandateId,
        asset: &AssetId,
        amount: u128,
    ) -> Result<(), BudgetError>;

    /// Check if a proposed spend is within budget.
    async fn check_budget(
        &self,
        agent_id: &AgentId,
        mandate_id: &MandateId,
        asset: &AssetId,
        amount: u128,
    ) -> Result<BudgetCheckResult, BudgetError>;

    /// Get current budget consumption.
    async fn get_consumption(
        &self,
        agent_id: &AgentId,
        mandate_id: &MandateId,
    ) -> Result<BudgetConsumption, BudgetError>;
}
```

### 13.4 Receipt and reconciliation traits

```rust
/// Manages payment receipts.
#[async_trait]
pub trait ReceiptStore: Send + Sync {
    /// Create an immutable receipt.
    async fn create(
        &self,
        receipt: PaymentReceipt,
    ) -> Result<ReceiptId, ReceiptError>;

    /// Get a receipt by ID.
    async fn get(
        &self,
        id: &ReceiptId,
    ) -> Result<PaymentReceipt, ReceiptError>;

    /// List receipts matching a filter.
    async fn list(
        &self,
        filter: ReceiptFilter,
    ) -> Result<Vec<PaymentReceipt>, ReceiptError>;

    /// Export receipts in a structured format.
    async fn export(
        &self,
        filter: ReceiptFilter,
        format: ExportFormat,
    ) -> Result<Vec<u8>, ReceiptError>;
}

/// Reconciles intent records against on-chain evidence.
#[async_trait]
pub trait Reconciler: Send + Sync {
    /// Run reconciliation for all unresolved intents.
    async fn reconcile(
        &self,
        agent_id: &AgentId,
    ) -> Result<ReconciliationReport, ReconciliationError>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReconciliationReport {
    pub agent_id: AgentId,
    pub run_at: DateTime<Utc>,
    /// Intents that were resolved during this reconciliation.
    pub resolved: Vec<ResolvedIntent>,
    /// Intents still in Unknown state.
    pub still_unknown: Vec<IntentId>,
    /// Discrepancies found (recorded state vs. chain evidence).
    pub discrepancies: Vec<Discrepancy>,
}
```

---

## 14. Marketplace settlement and fee model

### 14.1 Marketplace payment operations

When Polkagent's marketplace (PRD-12) involves paid offerings -- skills,
tools, compute, agent services, product kits -- the payment system
provides settlement.

```text
Buyer -----> Marketplace -----> Seller
       quote/accept             deliver
       |                        ^
       v                        |
  PaymentIntent            service/artifact
       |
       v
  settlement on selected rail
       |
       v
  PaymentReceipt (both parties)
```

### 14.2 Configurable fee model

```rust
/// Fee configuration for a marketplace transaction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarketplaceFeeConfig {
    /// Platform fee (percentage, basis points).
    pub platform_fee_bps: u32,
    /// Protocol fee (percentage, basis points).
    pub protocol_fee_bps: u32,
    /// Registry fee (percentage, basis points).
    pub registry_fee_bps: u32,
    /// Creator/seller receives the remainder.
    pub creator_share: CreatorShare,
    /// Referral fee (optional).
    pub referral_fee_bps: Option<u32>,
    /// Minimum transaction amount.
    pub minimum_amount: Option<u128>,
    /// Settlement asset.
    pub settlement_asset: AssetId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CreatorShare {
    /// Creator gets 100% minus fees.
    Remainder,
    /// Creator gets a fixed percentage.
    Fixed { bps: u32 },
}
```

**Requirement PAY-MKT-001:** Marketplace fees must be transparent. The
buyer must see the full fee breakdown before committing to a purchase.

**Requirement PAY-MKT-002:** Fee configuration must be set per registry
or marketplace. Different registries may have different fee structures.

**Requirement PAY-MKT-003:** Free offerings must remain supported.
The marketplace must not require payment for every listing.

---

## 15. Compliance considerations

This section identifies compliance questions that require qualified
legal counsel. Polkagent does not provide legal advice.

### 15.1 Questions for counsel

1. **Custody classification.** Does the chosen custody topology,
   jurisdiction, operator role, asset, fee model, and ability to
   initiate or halt transfers create licensing, custody, or
   consumer-protection obligations?

2. **Autonomy tiers.** Does the answer change between submit-only
   (Tier 1), user-approved, delegated proxy (Tier 2), and fully
   autonomous funded-agent (Tier 3) modes?

3. **Money transmission.** Does operating a marketplace with payment
   settlement constitute money transmission in relevant jurisdictions?

4. **Record retention.** What receipt/audit data must be retained,
   for how long, and under what privacy constraints?

5. **Tax reporting.** What reporting obligations arise from automated
   payment activity, especially for recurring/streaming payments?

6. **Sanctions screening.** Must recipients be screened against
   sanctions lists? If so, at what tier and for which jurisdictions?

7. **Consumer protection.** What dispute resolution, refund, and
   cooling-off obligations apply to marketplace transactions?

### 15.2 Design principles for compliance readiness

**FLAG FOR COUNSEL:** Money transmission licensing, sanctions compliance, and
tax reporting obligations for autonomous on-chain payment agents are
unresolved legal questions. The design below builds the technical hooks needed
to satisfy likely requirements; it does not resolve the legal questions.
Counsel review is required before any real-value production deployment.

**Requirement PAY-COMPL-001:** The payment system must produce a complete
audit trail regardless of compliance requirements. It is easier to
satisfy future requirements with existing data than to reconstruct
historical activity.

**Requirement PAY-COMPL-002:** Custody mode, autonomy tier, and
marketplace participation must be independently configurable so that
operators can comply with jurisdiction-specific requirements.

**Requirement PAY-COMPL-003:** The system must support operator-configured
recipient allowlists and denylists. Whether these are used for sanctions
compliance, organizational policy, or other purposes is the operator's
decision.

**Requirement PAY-COMPL-004:** Real-value payment capabilities (beyond
testnet) must not be enabled until the operator has acknowledged
compliance responsibilities through an explicit configuration step.

**Requirement PAY-COMPL-005:** The payment system must expose a sanctions
screening hook at intent construction time. This hook receives the
recipient account and asset before the action card is rendered. By default
the hook is a no-op pass-through. Operators can wire it to an external
screening API. If the hook returns a deny result, intent construction must
refuse with a compliance block that is not overrideable by model
instruction. This hook must be implemented in Phase 1 even if no operator
uses it yet.

**Requirement PAY-COMPL-006:** The receipt export format (PAY-RCPT-004)
must include all fields necessary for tax reporting: sender, recipient,
asset, amount in asset-native units, amount in a reference fiat value
(if configured), timestamp, block reference, fee paid, and the
authorization evidence type. The export must be available in CSV and
structured JSON. This is the audit-export hook that enables future
reporting compliance without retroactive data reconstruction.

---

## 15a. Staged implementation recommendations

This section distills the research findings from the 2026-07-30 review into
an actionable priority queue for the implementation team. It supplements the
phase gates in section 16 with a do/validate/defer/avoid framing.

### Do now (Phase 1 blockers)

| Item | Where |
|---|---|
| DOT and USDT (1984) / USDC (1337) transfers on Asset Hub | §2.2, §3.1 |
| All fees in DOT; do not assume USDC fee-sufficiency (ref 174 not enacted) | §2.4 |
| Three-API pre-flight: DryRunApi -> XcmPaymentApi -> TransactionPaymentApi | §3.1.2, §2.4 PAY-FEE-003 |
| Pure-proxy + proxy filter for all funded agent accounts | §8.1 |
| Per-period budget caps enforced in policy layer independently of on-chain allowances | §8.2 |
| Respect ED + nonce lifecycle + metadata-hash on every mainnet transaction | §2.5 PAY-ED-004, §4.6 |
| Sanctions screening hook (no-op by default) wired at intent construction | §15.2 PAY-COMPL-005 |
| Audit-export receipt fields for tax reporting | §15.2 PAY-COMPL-006 |

### Validate next (Phase 2 candidates)

| Item | Where |
|---|---|
| x402 non-EVM scheme over Asset Hub stablecoins for tool payments | §9.1 |
| Multisig + announcement-delay escrow (no smart contracts) | §3.5.2 |
| Multisig co-signing requirement for high-value operations | §8.1 |
| pallet_assets approve_transfer allowance patterns with policy-layer reconciliation | §3.3.2 PAY-ALLOW-002 |

### Defer

| Item | Reason |
|---|---|
| Streaming / metered payments | No native Polkadot streaming primitive; fee economics unresolved |
| USDC fee-sufficiency assumption | Referendum 174 not yet passed; monitor chain state |
| Smart-contract escrow (PVM/EVM) | Phase 3+ with separate audit gates |
| AP2 wire compatibility | No stable interoperable profile verified |

### Avoid

| Anti-pattern | Risk |
|---|---|
| Blind extrinsic submission without DryRunApi pre-flight | Silent dispatch failures; wasted fees |
| Treating ticker strings as asset identity | Token confusion / asset substitution attacks |
| Ignoring existential deposit on transfers | Account reaping |
| Unbounded agent spending budgets | Runaway agent or compromised key drains funds |
| Assuming USDC pays fees because it is "a stablecoin" | Transaction failure; referendum 174 not enacted |

---

## 16. Phase gates for real-value enablement

Real-value payment operations are gated behind evidence-based milestones.

### 16.1 Phase 1: Read-only and testnet

| Gate | Evidence required | Owner |
|---|---|---|
| Chain profile management | Pinned metadata fixtures for target testnets | Chain adapter team |
| Balance/asset reading | Correct balance display with ED, frozen, reserved | Chain adapter team |
| Action card rendering | Canonical card matches metadata-decoded intent | UX team |
| Intent lifecycle (testnet) | Full state machine on Paseo with fake signer | Core team |
| Pre-flight checks | All check types pass against seeded test fixtures | Core team |

### 16.2 Phase 2: Single-chain mainnet with per-action approval

| Gate | Evidence required | Owner |
|---|---|---|
| External signer integration | At least one wallet (Polkadot.js, Talisman, or equivalent) | Signer team |
| Metadata-hash binding | Signer and runtime metadata-hash check verified | Signer team |
| Pre-flight + risk gates | Adversarial test corpus (batch hiding, homoglyphs, stale metadata) | Security team |
| Comprehension study | Users correctly identify critical fields in blinded action cards | UX team |
| Finality observation | Correct state tracking through inclusion -> finality on mainnet | Core team |
| Receipt generation | Complete receipt for success, failure, and unknown outcomes | Core team |
| Reconciliation | Automated reconciliation detects seeded discrepancies | Core team |
| Security review | Independent security review of payment lifecycle | Security team |

### 16.3 Phase 3: Policy-autonomous operations

| Gate | Evidence required | Owner |
|---|---|---|
| Mandate evaluation | Policy correctly allows/denies/escalates against test corpus | Core team |
| Budget enforcement | Budget checks block over-budget actions; survive restarts | Core team |
| Rate limiting | Rate limits enforced under concurrent load | Core team |
| Circuit breaker | Automatic pause on configured trigger conditions | Core team |
| Proxy/multisig integration | Correct proxy dispatch and call-filter enforcement on target runtime | Chain adapter team |
| Emergency controls | Pause, revoke, and recovery drills succeed | Operations team |
| Recurring payments | Schedule execution, pause, resume, cancel lifecycle | Core team |
| Independent security review | Payment + autonomy architecture reviewed by external party | Security team |
| Legal review | Counsel review of custody, compliance, and operational obligations | Legal/compliance |

### 16.4 Phase 4: Fully autonomous and agent commerce

| Gate | Evidence required | Owner |
|---|---|---|
| Funded agent accounts | Proxy topology verified on target runtime | Chain adapter team |
| Auto top-up | Top-up lifecycle with approval and rate limiting | Core team |
| Cross-chain payments | XCM route evidence on testnet and mainnet | Chain adapter team |
| Streaming payments | Stream lifecycle with settlement and cancellation | Core team |
| Agent-to-agent commerce | Protocol adapter conformance (x402 or equivalent) | Integration team |
| Marketplace settlement | Fee model, dispute handling, reconciliation | Marketplace team |
| Red-team exercise | Adversarial prompt/tool tests cannot widen authority | Security team |
| Operational readiness | Monitoring, alerting, incident response procedures | Operations team |
| Legal and compliance | Full compliance review for autonomous and commerce modes | Legal/compliance |

---

## 17. Acceptance criteria and verification checklist

### 17.1 Core payment lifecycle

- [ ] PAY-CORE-01: A native transfer intent can be created, verified,
  approved, signed (via external signer), submitted, observed to finality,
  and receipted on Paseo testnet.
- [ ] PAY-CORE-02: The same lifecycle works for an Assets pallet transfer
  (e.g., USDT on Hub testnet).
- [ ] PAY-CORE-03: Pre-flight checks correctly block: insufficient balance,
  invalid recipient, stale metadata, and ED violation.
- [ ] PAY-CORE-04: The action card displays all canonical fields from
  metadata-decoded data. Model text is visually separate.
- [ ] PAY-CORE-05: A signing request correlates to exactly one intent.
  A signature for a different intent is rejected.
- [ ] PAY-CORE-06: Process crash between signing and submission does not
  cause duplicate submission on restart.
- [ ] PAY-CORE-07: Finality observation survives process restart and
  correctly resolves to Finalized, Dropped, or Unknown.
- [ ] PAY-CORE-08: Receipts for success, failure, and unknown cases contain
  complete evidence chains.

### 17.2 Risk gates

- [ ] PAY-RISK-01: A `batchAll` containing a hidden `proxy.addProxy`
  is flagged with a blocking risk finding.
- [ ] PAY-RISK-02: Near-duplicate and homoglyph recipient addresses
  trigger warnings.
- [ ] PAY-RISK-03: Stale metadata blocks intent construction.
- [ ] PAY-RISK-04: A model instruction cannot disable risk gates.
- [ ] PAY-RISK-05: Risk findings appear in the action card.

### 17.3 Autonomy

- [ ] PAY-AUTO-01: A new agent starts at Tier 0 with zero payment authority.
- [ ] PAY-AUTO-02: Tier escalation requires explicit configuration with
  consequence display.
- [ ] PAY-AUTO-03: Tier de-escalation is immediate and unconditional.
- [ ] PAY-AUTO-04: A Tier 2 agent with a mandate can execute within-policy
  actions without human approval.
- [ ] PAY-AUTO-05: A Tier 2 agent cannot execute actions outside its mandate.
- [ ] PAY-AUTO-06: Budget limits are enforced and survive process restart.
- [ ] PAY-AUTO-07: Rate limits are enforced under concurrent load.
- [ ] PAY-AUTO-08: Circuit breaker triggers pause on configured conditions.
- [ ] PAY-AUTO-09: Emergency pause takes effect within 1 second.
- [ ] PAY-AUTO-10: Emergency revocation returns agent to Tier 0.

### 17.4 Funded agent accounts

- [ ] PAY-FUND-01: Pure proxy + sub-proxy topology correctly limits
  agent operations to the configured filter on the target runtime.
- [ ] PAY-FUND-02: Controller can revoke agent proxy access at any time.
- [ ] PAY-FUND-03: Controller can recover all funds after revocation.
- [ ] PAY-FUND-04: Agent detects proxy revocation via chain events.
- [ ] PAY-FUND-05: Auto top-up executes with correct approval and rate limiting.
- [ ] PAY-FUND-06: Key rotation works without losing proxy relationship.

### 17.5 Reconciliation and accounting

- [ ] PAY-ACCT-01: Reconciliation detects and flags discrepancies between
  recorded state and on-chain evidence.
- [ ] PAY-ACCT-02: Budget consumption is accurately tracked across restarts.
- [ ] PAY-ACCT-03: Receipts are exportable in JSON and CSV formats.
- [ ] PAY-ACCT-04: Per-agent and per-run accounting correctly attributes
  all payment activity.
- [ ] PAY-ACCT-05: Multi-tenant deployments enforce accounting isolation.

### 17.6 Safety invariants (must hold at every tier)

- [ ] PAY-SAFE-01: Signing keys never appear in model context, logs,
  events, tool results, or marketplace extension APIs.
- [ ] PAY-SAFE-02: A model instruction cannot create, widen, or bypass
  a payment mandate.
- [ ] PAY-SAFE-03: A crashed/restarted process never duplicates an
  irreversible payment effect.
- [ ] PAY-SAFE-04: Unknown transaction outcome is never silently collapsed
  into success or failure.
- [ ] PAY-SAFE-05: Testnet and mainnet profiles cannot be confused or
  silently crossed.
- [ ] PAY-SAFE-06: An adversarial prompt/tool injection test suite cannot
  cause unauthorized payment activity.
- [ ] PAY-SAFE-07: All payment operations produce durable audit evidence.

---

## Appendix A: Requirement index

| ID | Section | Summary |
|---|---|---|
| PAY-ASSET-001 | 2.1 | Asset identity queried from chain at recorded block |
| PAY-ASSET-002 | 2.1 | Asset metadata is untrusted display data |
| PAY-ASSET-003 | 2.1 | Display branding separate from canonical AssetId |
| PAY-CHAIN-001 | 2.3 | Intent references ChainProfile by genesis hash |
| PAY-CHAIN-002 | 2.3 | Testnet/mainnet visually distinguishable |
| PAY-FEE-001 | 2.4 | Fee estimates re-checked before signing |
| PAY-FEE-002 | 2.4 | Alternative fee asset eligibility verified per profile |
| PAY-ED-001 | 2.5 | Post-transfer balance vs. ED displayed |
| PAY-ED-002 | 2.5 | Default to transfer_keep_alive |
| PAY-ED-003 | 2.5 | Agent accounts maintain configurable keep-alive buffer |
| PAY-BAL-001 | 2.6 | Display transferable, not free, balance |
| PAY-BAL-002 | 2.6 | Balance queries pinned to specific block |
| PAY-XFER-001 | 3.1 | Pre-flight checks required before action card |
| PAY-REQ-001 | 3.2 | Payment requests are untrusted input |
| PAY-REQ-002 | 3.2 | Payment requests are durable artifacts |
| PAY-PROXY-001 | 3.3 | Proxy filters are runtime-defined code |
| PAY-PROXY-002 | 3.3 | Proxy add/remove are high-risk operations |
| PAY-ALLOW-001 | 3.3 | On-chain allowances don't replace Polkagent policy |
| PAY-XCM-001 | 3.4 | Default to read/plan/simulate for XCM |
| PAY-XCM-002 | 3.4 | Test path required for every supported route |
| PAY-XCM-003 | 3.4 | XCM action card shows both-chain details |
| PAY-XCM-004 | 3.4 | XCM receipt distinguishes source/destination finality |
| PAY-XCM-005 | 3.4 | DryRunApi/XcmPaymentApi probed per profile |
| PAY-ESCROW-001 | 3.5 | Escrow phased; off-chain first |
| PAY-ESCROW-002 | 3.5 | Escrow timeout must have recovery path |
| PAY-RECUR-001 | 3.6 | Recurring requires Tier 2+ with budget/allowlist |
| PAY-RECUR-002 | 3.6 | Each recurring execution has independent receipt |
| PAY-RECUR-003 | 3.6 | Owner can pause/resume/modify/cancel at any time |
| PAY-STREAM-001 | 3.7 | Streaming is Phase 3+ with security review |
| PAY-STREAM-002 | 3.7 | Batch disbursement to amortize fees |
| PAY-STREAM-003 | 3.7 | Stream cancellation settles accrued amount |
| PAY-BATCH-001 | 3.8 | Batch flattens and displays all inner operations |
| PAY-BATCH-002 | 3.8 | Batch risk assessment flags hidden operations |
| PAY-BATCH-003 | 3.8 | Batch atomicity behavior displayed |
| PAY-CARD-001 | 4.3 | Canonical fields separate from model text |
| PAY-CARD-002 | 4.3 | Unknown fields displayed, not guessed |
| PAY-CARD-003 | 4.3 | SS58 and raw AccountId32 both shown |
| PAY-CARD-004 | 4.3 | Post-transfer balance and ED shown |
| PAY-META-001 | 4.4 | Metadata pinned by spec version and hash |
| PAY-META-002 | 4.4 | Decoding uses pinned metadata type registry |
| PAY-META-003 | 4.4 | Initial release supports curated call set |
| PAY-PRE-001 | 4.5 | Pre-flight deterministic from profile data |
| PAY-PRE-002 | 4.5 | Pre-flight failure produces refusal card |
| PAY-SIGN-001 | 4.6 | Signing request correlates to exact IntentId |
| PAY-SIGN-002 | 4.6 | Signing request includes genesis/block hash |
| PAY-SIGN-003 | 4.6 | Signing requests expire |
| PAY-FIN-001 | 4.7 | Five distinct outcome states |
| PAY-FIN-002 | 4.7 | Finality watcher survives restart |
| PAY-FIN-003 | 4.7 | Payment not complete until finality observed |
| PAY-IDEMP-001 | 5.3 | Stable idempotency key per intent |
| PAY-IDEMP-002 | 5.3 | Submission uses idempotency to prevent duplicates |
| PAY-IDEMP-003 | 5.3 | Crash recovery checks existing intents |
| PAY-CANCEL-001 | 5.4 | Cancellable before Submitted |
| PAY-CANCEL-002 | 5.4 | Cancel releases resources immediately |
| PAY-CANCEL-003 | 5.4 | Expired intents auto-refuse |
| PAY-RISK-001 | 6.2 | Risk gates operate on typed intent |
| PAY-RISK-002 | 6.2 | Risk gates not disableable by model |
| PAY-RISK-003 | 6.2 | Risk findings included in action card |
| PAY-RISK-004 | 6.2 | Adversarial risk gate test corpus |
| PAY-EMRG-001 | 7.3 | Pause within 1 second |
| PAY-EMRG-002 | 7.3 | Revocation via multiple channels |
| PAY-EMRG-003 | 7.3 | Configurable circuit breaker |
| PAY-EMRG-004 | 7.3 | Recovery produces reconciliation report |
| PAY-TIER-001 | 7.4 | Tier escalation requires informed configuration |
| PAY-TIER-002 | 7.4 | Tier de-escalation immediate |
| PAY-TIER-003 | 7.4 | No vague autonomy toggle |
| PAY-BUDGET-001 | 8.2 | Budget checked at construction and signing |
| PAY-BUDGET-002 | 8.2 | Budget state durable across restarts |
| PAY-BUDGET-003 | 8.2 | Rolling budget does not accumulate |
| PAY-TOPUP-001 | 8.3 | Auto top-up is a separate payment operation |
| PAY-TOPUP-002 | 8.3 | Auto top-up requires explicit configuration |
| PAY-TOPUP-003 | 8.3 | Auto top-up has independent rate limit |
| PAY-RECOVER-001 | 8.4 | Controller can revoke proxy at any time |
| PAY-RECOVER-002 | 8.4 | Controller can recover funds after revocation |
| PAY-RECOVER-003 | 8.4 | System detects proxy revocation |
| PAY-RECOVER-004 | 8.4 | Key rotation without losing proxy |
| PAY-X402-001 | 9.1 | x402 via generic adapter, not core |
| PAY-X402-002 | 9.1 | x402 conformance testing required |
| PAY-A2A-001 | 9.3 | Agent commerce standards as adapters |
| PAY-A2A-002 | 9.3 | Native mandates are primary path |
| PAY-COMMERCE-001 | 9.4 | Agent commerce Phase 4+ with legal review |
| PAY-RCPT-001 | 10.2 | Every effect produces a receipt |
| PAY-RCPT-002 | 10.2 | Receipts immutable |
| PAY-RCPT-003 | 10.2 | Failed/unknown receipts accurate |
| PAY-RCPT-004 | 10.2 | Receipts exportable |
| PAY-RECON-001 | 10.3 | Automatic reconciliation on schedule |
| PAY-RECON-002 | 10.3 | Discrepancy detection and flagging |
| PAY-RECON-003 | 10.3 | Reconciliation idempotent |
| PAY-ACCT-001 | 11.2 | Intents attributed to specific runs |
| PAY-ACCT-002 | 11.3 | Tenant accounting isolation |
| PAY-ACCT-003 | 11.3 | Accounting export in standard formats |
| PAY-SM-001 | 12.3 | Atomic state transitions with durable storage |
| PAY-SM-002 | 12.3 | State transitions recorded in history |
| PAY-SM-003 | 12.3 | No skipping intermediate states |
| PAY-MKT-001 | 14.2 | Transparent marketplace fees |
| PAY-MKT-002 | 14.2 | Per-registry fee configuration |
| PAY-MKT-003 | 14.2 | Free offerings supported |
| PAY-COMPL-001 | 15.2 | Complete audit trail regardless of requirements |
| PAY-COMPL-002 | 15.2 | Custody/tier/marketplace independently configurable |
| PAY-COMPL-003 | 15.2 | Operator-configured allow/deny lists |
| PAY-COMPL-004 | 15.2 | Real-value enablement requires acknowledgment |
| PAY-COMPL-005 | 15.2 | Sanctions screening hook at intent construction |
| PAY-COMPL-006 | 15.2 | Receipt export includes tax-reporting fields |
| PAY-FEE-003 | 2.4 | Three-API pre-flight sequence mandatory; no blind submits |
| PAY-ED-004 | 2.5 | Nonce lifecycle + metadata-hash on every mainnet tx |
| PAY-ALLOW-002 | 3.3 | On-chain approve_transfer ceiling vs. policy-layer budget |

---

## Appendix B: Glossary

| Term | Definition |
|---|---|
| **AccountId32** | A 32-byte Polkadot account identifier. |
| **ACP** | Agent Commerce Protocol -- an emerging standard for agent-to-agent payments. |
| **AP2** | A proposed mandate standard for agent payment authorization. |
| **batchAll** | A Polkadot utility call that executes multiple calls atomically. |
| **Circuit breaker** | An automatic safety mechanism that pauses agent operations when anomalous patterns are detected. |
| **Controller** | The human or multisig account that maintains ultimate authority over an agent's funded account. |
| **ED** | Existential deposit -- the minimum balance to keep an account alive. |
| **Grant** | The resolved set of permissions and limits for a specific effect. |
| **Homoglyph** | Visually similar characters that could be used to deceive (e.g., 0/O, 1/l). |
| **Idempotency key** | A unique identifier ensuring that retrying an operation does not duplicate its effect. |
| **Mandate** | A configured policy authorizing autonomous action within defined boundaries. |
| **Metadata hash** | A hash of the runtime's type/call/event descriptions, used for integrity. |
| **PVM** | PolkaVM -- Polkadot's RISC-V-based virtual machine for smart contracts. |
| **Pure proxy** | A Polkadot proxy account with no private key, controlled entirely by its proxy relationships. |
| **Reconciliation** | Matching recorded intent state against on-chain evidence. |
| **Reaping** | Deletion of an account whose balance falls below the existential deposit. |
| **SS58** | The address encoding format used by Polkadot and Substrate chains. |
| **Subxt** | A Rust client library for interacting with Substrate/Polkadot chains. |
| **x402** | An HTTP payment protocol using 402 status codes for machine-to-machine transactions. |
| **XCM** | Cross-Consensus Messaging -- a message format for cross-chain interactions. |

---

## APPENDIX A: PAYMENT SYSTEM IMPLEMENTATION BLUEPRINT

### A.1 Asset Registry

The asset registry is the authoritative runtime source of truth for every asset
Polkagent can send, receive, or quote. It is never populated from ticker strings,
configuration guesses, or model-generated metadata.

#### A.1.1 Asset identification scheme

Every asset is keyed by `(chain_genesis: H256, spec_version: u32, location:
AssetLocation)`. The triple is stable across node restarts but is invalidated by
a runtime upgrade that changes the spec version. Two assets with the same display
symbol but different `AssetLocation` values are distinct and must never be
conflated.

```rust
/// Canonical asset registry entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub id: AssetId,
    pub profile: AssetProfile,
    /// Source that introduced this entry.
    pub source: RegistrySource,
    /// Wall-clock time of last successful refresh.
    pub last_refreshed: DateTime<Utc>,
    /// Block hash at which the profile was last verified.
    pub last_verified_block: H256,
    /// Number of consecutive refresh failures.
    pub refresh_failures: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum RegistrySource {
    /// Explicitly added by an operator.
    AdminAllowlist,
    /// Discovered via `pallet_assets` enumeration and accepted by auto-discovery policy.
    AutoDiscovered { accepted_at: DateTime<Utc> },
    /// Built-in Phase 1 asset (DOT native, USDT 1984, USDC 1337).
    BuiltIn,
}
```

**Resolution order for a payment operation:**

1. Check `BuiltIn` entries (always trusted for Phase 1 assets).
2. Check `AdminAllowlist` entries for the target genesis hash.
3. If auto-discovery is enabled, check `AutoDiscovered` entries that have been
   explicitly accepted.
4. Otherwise, refuse with an `AssetNotInRegistry` error -- never construct
   an intent for an unknown asset.

#### A.1.2 Asset metadata caching and refresh

Asset profiles are block-pinned snapshots. They are valid until the runtime
spec version changes or the configured staleness threshold is exceeded.

```rust
pub struct AssetRegistryConfig {
    /// Maximum age before a profile is re-fetched (default: 10 minutes).
    pub staleness_threshold: Duration,
    /// Maximum age before a stale profile causes a hard block (default: 60 minutes).
    pub hard_expiry: Duration,
    /// Number of allowed consecutive refresh failures before blocking (default: 3).
    pub max_refresh_failures: u32,
    /// Whether to re-fetch proactively in the background before staleness.
    pub background_refresh: bool,
}
```

Refresh procedure for a single entry:

1. Query the chain's current spec version. If it differs from the stored
   `spec_version`, invalidate the cached profile and re-fetch from scratch.
2. Call `pallet_assets::metadata(asset_id)` and `pallet_assets::asset(asset_id)`
   at the chain tip, recording the block hash.
3. Recompute `is_sufficient`, `existential_deposit`, and `fee_eligible` from
   the runtime response.
4. Update `last_refreshed`, `last_verified_block`, and reset `refresh_failures`
   to zero.
5. If the query fails, increment `refresh_failures`. If `refresh_failures` >=
   `max_refresh_failures`, mark the entry `Unresolvable` and surface this as
   an uncertainty block in any dependent intent.

#### A.1.3 Allowlist management

**Admin allowlist (operator-curated):** Entries are added via the Polkagent
configuration file or admin API. Each entry specifies the full `AssetId` plus
an optional label and trust level. Only `Verified` or `Recognized` assets in
the admin allowlist are usable in payment operations.

**Auto-discovery:** When enabled, Polkagent enumerates `pallet_assets` on the
target chain and presents newly found assets to the operator for acceptance.
Auto-discovered assets are held in a `Pending` state and cannot be used in
payment operations until an operator explicitly accepts them. Auto-discovery
is disabled by default.

**Phase 1 built-ins:** The following assets are pre-registered and
`Verified` without operator action:

| Asset | Chain | `AssetLocation` | `AssetId` |
|---|---|---|---|
| DOT (native) | Polkadot Hub | `AssetLocation::Native` | Genesis + `Native` |
| USDT | Polkadot Hub | `AssetLocation::PalletAsset { pallet_index, asset_id: 1984 }` | Genesis + 1984 |
| USDC | Polkadot Hub | `AssetLocation::PalletAsset { pallet_index, asset_id: 1337 }` | Genesis + 1337 |

`pallet_index` is resolved at runtime from the chain's metadata and must not
be hardcoded in source.

#### A.1.4 Existential deposit (ED) handling per asset

Each asset profile carries its own `existential_deposit` expressed in the
asset's smallest unit. For assets where `is_sufficient = true`, the account
can survive on that asset alone. For non-sufficient assets, the native balance
ED is still required.

```rust
/// Compute whether a proposed spend would leave the sender below the effective ED.
pub fn ed_impact(
    current_balance: u128,
    asset_profile: &AssetProfile,
    spend_amount: u128,
    keep_alive_buffer: u128, // operator-configured extra margin
) -> EdImpact {
    let post_transfer = current_balance.saturating_sub(spend_amount);
    let effective_floor = asset_profile.existential_deposit + keep_alive_buffer;
    if post_transfer < effective_floor {
        EdImpact::WouldReap {
            post_transfer,
            effective_floor,
            ed: asset_profile.existential_deposit,
        }
    } else {
        EdImpact::Safe { post_transfer, margin: post_transfer - effective_floor }
    }
}

pub enum EdImpact {
    Safe { post_transfer: u128, margin: u128 },
    WouldReap { post_transfer: u128, effective_floor: u128, ed: u128 },
}
```

The `keep_alive_buffer` is a per-agent policy parameter (default: 0, i.e. exact
ED). Operators of funded agent accounts should set this to at least 0.1 DOT
(1_000_000_000 planck) to absorb fee fluctuations.

---

### A.2 Fee Estimation Engine

The fee engine executes the mandatory three-API pre-flight sequence before every
payment action. No call may skip any step; failure of any step surfaces as an
uncertainty block in the action card.

#### A.2.1 Weight-based fee calculation

Polkadot fees are computed from the extrinsic's dispatch weight via:

```
fee = base_fee + (weight * per_weight_fee) + length_fee + tip
```

All four components are queried from the runtime, not estimated from static tables.

```rust
/// Input to the fee estimation pipeline.
pub struct FeeEstimationInput {
    /// The encoded call bytes (SCALE).
    pub call_bytes: Vec<u8>,
    /// The sender account (for nonce / account info).
    pub sender: AccountId32,
    /// Chain profile pinning the runtime to use.
    pub profile: ChainProfile,
    /// Desired tip strategy.
    pub tip_strategy: TipStrategy,
}

/// Result of the three-API pre-flight.
pub struct FeeEstimationOutput {
    /// Simulated dispatch result from DryRunApi.
    pub dry_run_result: DryRunResult,
    /// XCM weight contribution (None if not cross-chain).
    pub xcm_weight: Option<Weight>,
    /// Final fee estimate from TransactionPaymentApi.
    pub fee: FeeEstimate,
    /// Blocks at which each API was called (for staleness tracking).
    pub evidence_blocks: FeeEvidenceBlocks,
}
```

**Execution sequence:**

1. **DryRunApi (`system_dryRun`):** Submit the encoded call for dispatch
   simulation. This returns the simulated dispatch result, emitted events, and
   the consumed weight. A simulated failure must block intent construction --
   no blind submits.
2. **XcmPaymentApi (`xcm_payment_queryWeightToAssetFee`):** Only invoked when
   the call contains an XCM instruction. Converts the XCM-specific weight
   component to a fee in the target asset. If unavailable, surface as increased
   uncertainty; do not skip silently.
3. **TransactionPaymentApi (`payment_queryFeeDetails`):** Convert the total
   dispatch weight (from step 1 + step 2) to a fee amount in the fee asset.
   This is the authoritative estimate used in the action card.

#### A.2.2 Tip strategy

```rust
pub enum TipStrategy {
    /// No tip. Suitable for non-urgent operations.
    None,
    /// Fixed tip amount in the fee asset's smallest unit.
    Fixed(u128),
    /// Percentage of the base fee as a tip.
    Percentage(f64),
    /// Operator-configured default for the chain profile.
    ProfileDefault,
}
```

Tip is added to the fee estimate shown in the action card. The fee cap
(`FeeEstimate::fee_cap`) must include the tip. Users see:
`estimated fee: X + tip: Y <= cap: Z`.

#### A.2.3 XCM fee estimation for cross-chain

Cross-chain payments require fee estimation on both the source and destination
chains. The source fee covers submitting the XCM message; the destination fee
covers its execution.

```rust
pub struct XcmFeeEstimate {
    /// Source chain fee (DOT or configured fee asset).
    pub source_fee: FeeEstimate,
    /// Destination chain execution fee (in the XCM fee asset for that chain).
    pub destination_fee: Option<FeeEstimate>,
    /// Whether the destination fee is included in the XCM message or paid separately.
    pub destination_fee_mode: DestinationFeeMode,
}

pub enum DestinationFeeMode {
    /// Fee is deducted from the transferred asset amount.
    DeductedFromTransfer,
    /// Fee is paid from the sender's reserve on the source chain.
    PaidFromReserve,
    /// Destination fee mechanism is unknown (surface as uncertainty).
    Unknown,
}
```

#### A.2.4 Fee display and user confirmation flow

The action card presents fees in human-readable form:

```
Fee breakdown:
  Base fee:    0.0010 DOT
  Weight fee:  0.0035 DOT
  Length fee:  0.0002 DOT
  Tip:         0.0000 DOT
  ─────────────────────────
  Total:     ≤ 0.0050 DOT    (cap: 0.0150 DOT)
  Evidence:    block #22104821 (age: 2s)
```

If the fee estimate is older than the configured staleness threshold (default:
2 blocks) at the moment the user clicks Approve, the system re-runs the three-API
sequence before invoking the signer. If re-estimation produces a fee above the
cap, the action card refreshes with the new estimate and requires re-confirmation.

---

### A.3 Payment Intent Pipeline

The pipeline converts a raw user request into a receipted on-chain outcome through
a series of typed, durable stages. Each stage produces evidence that is carried
forward into the receipt.

#### A.3.1 PaymentIntent -> PaymentAction -> PaymentReceipt

```
User request (natural language or structured)
    |
    v
[IntentBuilder]
    - Resolve chain profile (genesis hash)
    - Resolve asset (registry lookup, not ticker)
    - Resolve recipient (SS58 validation, network prefix check)
    - Fetch block-pinned state: balance, nonce, ED
    |
    v
PaymentIntent { status: Proposed, ... }
    |
    v
[PreflightChecker] -- all checks from §3.1.2
    |
   pass/fail
    |
PaymentIntent { status: Verified | Refused, preflight_evidence: ... }
    |
    v
[FeeEstimationEngine] -- three-API sequence (§A.2.1)
    |
    v
[RiskGates] -- typed findings, not string matching
    |
    v
[PolicyEvaluator / MandateEvaluator]
    - Tier 1: await human approval -> action card rendered
    - Tier 2/3: mandate check -> auto-authorize or escalate
    |
    v
PaymentAction {
    - SCALE-encoded extrinsic payload
    - genesis_hash, block_hash, nonce, mortality era
    - metadata_hash (where supported)
}
    |
    v
[Signer port] -- receives only the canonical payload
    |
    v
[Submitter] -- idempotency key, RPC submit
    |
    v
[FinalityWatcher] -- Pending -> Included -> Finalized | Dropped | Unknown
    |
    v
PaymentReceipt {
    intent_id, action_summary, authorization,
    tx_hash, outcome, finalized_block, events,
    actual_fee, chain_profile_snapshot, ...
}
```

#### A.3.2 Dry-run simulation before execution

The `DryRunApi` simulation must occur before the action card is rendered, not
after. If the simulation fails with a dispatch error (e.g.,
`BadOrigin`, `InsufficientBalance`, `CallFiltered`), the system must display
a refusal card with the decoded error, not proceed to the approval step.

```rust
pub struct DryRunResult {
    /// Whether the simulated dispatch succeeded.
    pub success: bool,
    /// Decoded dispatch error, if any.
    pub error: Option<String>,
    /// Consumed weight from simulation.
    pub weight: Weight,
    /// Events emitted during simulation.
    pub simulated_events: Vec<DecodedEvent>,
    /// Block at which the simulation ran.
    pub simulated_at_block: H256,
}
```

Simulation results are included in `PreflightEvidence` and displayed in the
action card's evidence section. A user can see that the payment was simulated
and what the simulation produced.

#### A.3.3 Action card generation with exact calldata

The action card is generated from the typed `PaymentIntent` using
metadata-decoded field names and values. The process:

1. SCALE-decode the call bytes against the pinned metadata type registry.
2. Map each decoded field to a display label from the metadata.
3. Apply asset profile formatting: divide raw planck amounts by `10^decimals`
   and suffix with the asset symbol.
4. Validate the decoded output matches the `PaymentAction` fields
   (guard against call-index drift after a runtime upgrade between
   construction and card rendering).
5. Render the canonical section; attach model explanation beneath the
   visible separator.

If any field cannot be decoded with full confidence, display `[unresolvable]`
for that field rather than a fallback guess.

#### A.3.4 Receipt storage and querying

Receipts are immutable once created. The store is append-only: corrections are
linked amendments, not mutations.

```rust
pub struct ReceiptQuery {
    pub agent_id: Option<AgentId>,
    pub run_id: Option<RunId>,
    pub mandate_id: Option<MandateId>,
    pub asset: Option<AssetId>,
    pub chain_genesis: Option<H256>,
    pub outcome: Option<OutcomeFilter>,
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
    pub limit: usize,
    pub offset: usize,
}

pub enum OutcomeFilter {
    AnyFinalized,
    Failed,
    Unknown,
    Dropped,
}
```

The storage backend must support:
- Point lookup by `ReceiptId` in O(1).
- Range query by `(agent_id, created_at)` for dashboard views.
- Full-text search by tag and custom fields for reconciliation.
- Export to JSON and CSV (PAY-RCPT-004).

Receipts reference their parent `PaymentIntent` by ID. The intent store is
similarly append-only with a transition history log.

---

### A.4 Mandate Authorization DSL

The mandate DSL is the configuration language for autonomous payment authority.
It is evaluated deterministically against the typed `PaymentIntent` -- not
against model text.

#### A.4.1 Complete mandate specification

```toml
[mandate]
# Unique, stable identifier for this mandate.
id = "daily-treasury-ops"
# Human label (display only; not used in evaluation).
name = "Daily treasury operations"
# Which autonomy tier this mandate enables.
tier = "policy_bounded"  # or "fully_autonomous"

# ── Action scope ─────────────────────────────────────────────────────
# Allowed call families. Values match PaymentAction variant names.
allowed_actions = ["native_transfer", "asset_transfer"]

# Allowed chains by genesis hash (mainnet + testnet may not share a mandate).
allowed_chains = [
    "0x91b171bb158e2d3848fa23a9f1c25182fb8e20313b2c1eb49219da7a70ce90c3"  # Polkadot Hub
]

# Allowed assets by registry key ("native" or "asset:<id>").
allowed_assets = ["native", "asset:1984"]

# Allowed recipient accounts. Supports exact SS58 and glob patterns.
# The glob "bounty-*" matches any address tagged as a bounty recipient
# in the operator-configured recipient registry.
allowed_destinations = ["5F3sABC...9q", "bounty-*"]

# Whether to require DryRunApi simulation for every action.
# true is always safer and is the recommended default.
requires_dry_run = true

# ── Budget limits ─────────────────────────────────────────────────────
# Maximum spend per individual action (in asset's smallest unit).
max_per_transaction = "100_000_000_000"       # 10 DOT
max_per_transaction_asset = "native"

# Maximum spend in a rolling 24-hour window.
max_per_day = "500_000_000_000"               # 50 DOT
max_per_day_asset = "native"

# Maximum cumulative lifetime spend.
max_lifetime = "10_000_000_000_000"           # 1000 DOT
max_lifetime_asset = "native"

# Actions below this amount are auto-approved (no escalation).
# Actions at or above this amount go through the normal mandate evaluation.
# Set to "0" to require evaluation for all amounts.
auto_approve_below = "10_000_000_000"         # 1 DOT
auto_approve_below_asset = "native"

# ── Rate limits ───────────────────────────────────────────────────────
max_actions_per_hour = 10
max_actions_per_day = 50
# Minimum wall-clock time between consecutive actions.
cooldown_between_actions = "60s"

# ── Escalation ────────────────────────────────────────────────────────
# What to do when an action falls outside mandate scope.
out_of_policy = "deny"  # or "escalate_to_human"

# ── Circuit breaker ───────────────────────────────────────────────────
[mandate.circuit_breaker]
# Condition: N consecutive failures within T duration.
consecutive_failures = 3
within_duration = "1h"
# Action on trigger.
action = "pause_and_notify"  # or "revoke"

# ── Lifecycle ─────────────────────────────────────────────────────────
# Optional: mandate automatically expires at this time.
expires_at = "2027-01-01T00:00:00Z"

# Optional: mandate activates only within these UTC hours (24h format).
active_hours = { start = "09:00", end = "17:00", timezone = "UTC" }
```

#### A.4.2 Mandate evaluation algorithm

```
fn evaluate_mandate(intent: &PaymentIntent, mandate: &Mandate) -> MandateDecision:

  1. Check mandate is Active (not Suspended, Revoked, or Expired).
     -> If not Active: Denied { reason: "mandate not active" }

  2. Check chain binding: intent.chain_profile.genesis_hash in mandate.allowed_chains.
     -> If not: Denied { reason: "chain not in mandate" }

  3. Check action family: intent.action variant in mandate.allowed_actions.
     -> If not: out_of_policy handler (Denied or EscalateToHuman)

  4. Check asset: intent action's asset in mandate.allowed_assets.
     -> If not: out_of_policy handler

  5. Check recipient: intent action's recipient matches an entry in
     mandate.allowed_destinations (exact match or glob expansion via
     operator recipient registry).
     -> If not: out_of_policy handler

  6. Check per-action amount <= mandate.max_per_transaction.
     -> If over: Denied { reason: "exceeds per-transaction limit" }

  7. Check per-day rolling budget: current_day_spend + amount <= mandate.max_per_day.
     -> If would exceed: Denied { reason: "would exceed daily budget" }

  8. Check lifetime budget: lifetime_spend + amount <= mandate.max_lifetime.
     -> If would exceed: Denied { reason: "would exceed lifetime budget" }

  9. Check rate limit: actions in last hour <= mandate.max_actions_per_hour.
     -> If over: Denied { reason: "rate limit exceeded" }

  10. Check cooldown: now - last_action_time >= mandate.cooldown_between_actions.
      -> If too soon: Denied { reason: "cooldown period active" }

  11. Check active_hours: current UTC time in mandate.active_hours range.
      -> If outside: Denied { reason: "outside mandate active hours" }

  12. Check auto_approve_below: if amount < auto_approve_below, skip further
      evaluation and return Authorized without escalation.

  13. All checks pass -> Authorized { mandate_id, grant: ResolvedGrant { ... } }
```

All checks are evaluated against durable budget state, not in-memory counters.
Budget state is persisted before returning `Authorized` so a crash does not
allow replay of the same spend.

#### A.4.3 Mandate lifecycle

```rust
pub enum MandateStatus {
    /// Created but not yet active (e.g., awaiting owner confirmation).
    Draft,
    /// Active: will be evaluated for matching intents.
    Active { activated_at: DateTime<Utc> },
    /// Suspended: no new actions authorized; existing in-flight actions complete.
    Suspended { reason: String, suspended_at: DateTime<Utc> },
    /// Revoked: permanently disabled; cannot be re-activated.
    Revoked { reason: String, revoked_at: DateTime<Utc> },
    /// Expired by its configured `expires_at` time.
    Expired { expired_at: DateTime<Utc> },
}
```

Lifecycle transitions:

| From | To | Trigger | Who |
|---|---|---|---|
| Draft | Active | Owner confirmation | Owner |
| Active | Suspended | Circuit breaker or owner action | System or Owner |
| Suspended | Active | Owner resume | Owner |
| Active / Suspended | Revoked | Owner revoke or emergency | Owner |
| Active | Expired | `expires_at` time reached | System (cron) |

Revocation is permanent. To re-grant authority, the owner must create a new
mandate with a new `id`. This prevents accidental re-activation of a
revoked policy.

#### A.4.4 Audit trail for mandate usage

Every mandate evaluation is recorded regardless of outcome:

```rust
pub struct MandateAuditEntry {
    pub id: AuditEntryId,
    pub mandate_id: MandateId,
    pub intent_id: IntentId,
    pub agent_id: AgentId,
    pub evaluated_at: DateTime<Utc>,
    pub decision: MandateDecision,
    /// Snapshot of the budget state at evaluation time.
    pub budget_snapshot: BudgetConsumption,
    /// Which check failed first (for Denied decisions).
    pub first_failing_check: Option<String>,
}
```

The audit log is append-only, indexed by `(mandate_id, evaluated_at)`, and
included in the export output for compliance review (PAY-COMPL-001).

---

## APPENDIX B: BUDGET ENFORCEMENT

Budget enforcement is a distinct layer from mandate policy evaluation. Mandates
define what an agent is permitted to do; budgets define how much. Budget state
is persisted to durable storage before every spend is authorized.

### B.1 Per-agent budget tracking

```rust
/// Live budget state for a single agent account.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentBudgetState {
    pub agent_id: AgentId,
    /// Per-mandate budget consumption.
    pub mandate_budgets: HashMap<MandateId, MandateBudgetState>,
    /// Aggregate across all mandates for this agent.
    pub aggregate: AggregateBudget,
    /// Last persistence timestamp (for crash recovery).
    pub persisted_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MandateBudgetState {
    pub mandate_id: MandateId,
    /// Total spend this rolling window, per asset.
    pub rolling_spend: HashMap<AssetId, RollingWindow>,
    /// Total lifetime spend, per asset.
    pub lifetime_spend: HashMap<AssetId, u128>,
    /// Actions executed this rolling window.
    pub rolling_action_count: RollingWindow<u32>,
    /// Timestamp of last action (for cooldown enforcement).
    pub last_action_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RollingWindow<T> {
    pub value: T,
    pub window_start: DateTime<Utc>,
    pub window_duration: Duration,
}
```

**Persistence contract:** Before returning `Authorized` from mandate evaluation,
the budget tracker must:
1. Increment the rolling and lifetime spend optimistically.
2. Persist the updated `AgentBudgetState` to durable storage.
3. Only then return `Authorized`.

If step 2 fails, the spend must not be counted and the evaluation must return
a transient error (not `Authorized`). This prevents the "optimistic but
crash-before-persist" scenario from allowing extra spends.

### B.2 Per-run budget limits

Each agent run (conversation, trigger, scheduled task) can have an independent
budget cap that is enforced in addition to the mandate budget.

```rust
pub struct RunBudgetConfig {
    /// Maximum total spend for this run, per asset.
    pub max_spend: HashMap<AssetId, u128>,
    /// Maximum number of payment actions in this run.
    pub max_actions: Option<u32>,
}
```

Per-run limits are provided at run creation time and enforced by the payment
service before consulting the mandate evaluator. If a run budget is exhausted,
the agent's payment actions are refused for the remainder of the run even if
the mandate budget would permit them.

### B.3 Aggregate budget across all agents

Operators can configure an organization-level aggregate budget that applies
across all agents:

```rust
pub struct OrganizationBudget {
    pub org_id: OrgId,
    /// Maximum total spend per rolling window across all agents.
    pub max_aggregate_rolling: HashMap<AssetId, RollingBudget>,
    /// Maximum total lifetime spend across all agents.
    pub max_aggregate_lifetime: HashMap<AssetId, u128>,
}
```

Aggregate budget checks occur after per-agent mandate checks but before the
signer is invoked. A spend that passes per-agent checks but would exceed the
org aggregate is denied with a clear error message.

### B.4 Budget alert thresholds

```rust
pub struct BudgetAlerts {
    /// Send alert when rolling spend reaches this fraction of the limit (e.g., 0.80).
    pub rolling_warning_threshold: f64,
    /// Send alert when lifetime spend reaches this fraction of the limit.
    pub lifetime_warning_threshold: f64,
    /// Alert channel (webhook, email, on-chain notification).
    pub channels: Vec<AlertChannel>,
}

pub enum AlertChannel {
    Webhook { url: String, secret: String },
    Email { address: String },
    OnChain { account: AccountId32 },
}
```

Alerts are informational and do not block payment operations. They give operators
advance notice before hard limits are reached.

### B.5 Over-budget handling

When a budget limit is reached, the system applies the configured
`OverBudgetPolicy`:

```rust
pub enum OverBudgetPolicy {
    /// Deny the action; do not pause the agent.
    Deny,
    /// Pause the agent and notify the operator.
    PauseAndNotify,
    /// Escalate the specific action to human approval
    /// even if the mandate would normally auto-authorize.
    EscalateToHuman,
}
```

The default policy is `Deny`. `PauseAndNotify` is appropriate for agents where
hitting a budget ceiling is unexpected and warrants investigation. `EscalateToHuman`
is appropriate for agents that should continue operating but require human oversight
for high-spend periods.

### B.6 Budget reset schedules

Rolling budgets reset at the start of each window. Windows are calendar-aligned
where possible (daily windows reset at UTC midnight; weekly windows reset on
Monday UTC midnight) to match operator expectations.

```rust
pub enum BudgetResetSchedule {
    /// Rolling: window advances continuously. No hard reset.
    Rolling { window: Duration },
    /// Calendar daily: resets at UTC midnight.
    Daily,
    /// Calendar weekly: resets Monday UTC midnight.
    Weekly,
    /// Calendar monthly: resets on the 1st of each month UTC midnight.
    Monthly,
    /// Manual: operator must explicitly reset via API or CLI.
    Manual,
}
```

**Unspent budget does not carry over.** A daily budget of 50 DOT that saw only
20 DOT spent does not give 80 DOT the next day. This prevents budget
accumulation that could enable large one-day spikes.

---

## APPENDIX C: IMPLEMENTATION CHECKLIST

Tasks are grouped by subsystem. Each task includes a brief acceptance criterion.
Tasks within a group are roughly ordered by dependency.

### C.1 Asset Registry

- [ ] **IMPL-REG-01** Define `RegistryEntry`, `RegistrySource`, and `AssetRegistryConfig`
  structs in `polkagent-payments` crate.
  *Criterion: Compiles; unit tests for source discrimination pass.*

- [ ] **IMPL-REG-02** Implement `BuiltIn` entries for DOT native, USDT 1984, and USDC 1337
  on Polkadot Hub genesis hash. Resolve `pallet_index` from chain metadata at startup,
  not from a hardcoded constant.
  *Criterion: Integration test against Paseo testnet resolves all three entries correctly.*

- [ ] **IMPL-REG-03** Implement `AdminAllowlist` CRUD: add, remove, update trust level.
  Entries stored in the Polkagent database with genesis hash as a shard key.
  *Criterion: Added entry survives process restart; removed entry cannot be used in intents.*

- [ ] **IMPL-REG-04** Implement background refresh task with staleness threshold and
  `max_refresh_failures` circuit. Refresh failures increment a counter and are logged.
  *Criterion: Simulated RPC failure for 3 consecutive polls marks the entry `Unresolvable`
  and blocks new intents referencing it.*

- [ ] **IMPL-REG-05** Implement ED impact computation (`ed_impact` fn in §A.1.4).
  *Criterion: Unit tests for `Safe`, `WouldReap`, and exact-ED edge cases.*

- [ ] **IMPL-REG-06** Auto-discovery enumeration (disabled by default). When enabled,
  enumerate `pallet_assets` and place new entries in `Pending` state. Provide operator
  CLI command `polkagent asset accept <asset-id>` to promote to `AutoDiscovered`.
  *Criterion: Discovered assets do not appear in payment operations until accepted.*

### C.2 Fee Estimation Engine

- [ ] **IMPL-FEE-01** Implement `FeeEstimationInput`, `FeeEstimationOutput`, and
  `FeeEvidenceBlocks` structs.
  *Criterion: Compiles; fields match §A.2.1 spec.*

- [ ] **IMPL-FEE-02** Implement `DryRunApi` integration via `system_dryRun` RPC call.
  Decode the `DispatchResultWithPostInfo` from the response; extract weight and events.
  *Criterion: Integration test on Paseo correctly simulates a `balances.transferKeepAlive`
  call and returns consumed weight.*

- [ ] **IMPL-FEE-03** Implement `TransactionPaymentApi` (`payment_queryFeeDetails`) using
  the weight from step IMPL-FEE-02.
  *Criterion: Fee estimate matches on-chain actual fee to within 10% on Paseo testnet.*

- [ ] **IMPL-FEE-04** Implement `XcmPaymentApi` (`xcm_payment_queryWeightToAssetFee`).
  Gate on cross-chain calls only; treat API unavailability as an uncertainty block,
  not a fatal error.
  *Criterion: Non-XCM calls skip this step; XCM calls include an XCM weight component
  in the output.*

- [ ] **IMPL-FEE-05** Implement tip strategy evaluation and fee cap inclusion.
  *Criterion: `TipStrategy::Percentage(0.1)` adds 10% of base fee as tip; total
  (fee + tip) appears in the action card and is <= the configured cap.*

- [ ] **IMPL-FEE-06** Implement staleness re-check before signing invocation.
  If estimate is older than 2 blocks, re-run the three-API sequence.
  *Criterion: In a test with a simulated 3-block delay between card rendering and
  approval, the fee is re-fetched before signing.*

### C.3 Payment Intent Pipeline

- [ ] **IMPL-PIPE-01** Implement `IntentBuilder` trait and its default implementation.
  Wire chain profile resolution, asset registry lookup, and recipient SS58 validation.
  *Criterion: `build()` returns an error for unknown assets and for malformed
  SS58 addresses.*

- [ ] **IMPL-PIPE-02** Implement `PreflightChecker` trait covering all checks in §3.1.2.
  Each check is a separate function; the result is a `Vec<PreflightFinding>`.
  *Criterion: Each check can be unit-tested independently with mock chain state.*

- [ ] **IMPL-PIPE-03** Implement `RiskGate` trait with the full pattern corpus from §6.1.
  *Criterion: Adversarial fixtures (batch-hidden proxy, homoglyph address, stale metadata)
  all produce the expected finding category.*

- [ ] **IMPL-PIPE-04** Implement action card renderer: metadata-decode call bytes,
  apply asset formatting, separate canonical fields from model text.
  *Criterion: A serialized `PaymentIntent` renders to a card that matches the
  ASCII template in §4.3 field-for-field.*

- [ ] **IMPL-PIPE-05** Implement `DryRunResult` storage in `PreflightEvidence`.
  Display simulated events in the action card evidence section.
  *Criterion: A simulated `ExtrinsicFailed` event causes a refusal card, not an
  action card.*

- [ ] **IMPL-PIPE-06** Implement `ReceiptStore` with SQLite backend for local deployments
  and a pluggable interface for external databases.
  *Criterion: Create, get, list, and export (JSON, CSV) all pass integration tests.*

- [ ] **IMPL-PIPE-07** Implement idempotency key generation (UUID v7, includes timestamp
  for ordering) and duplicate-submission guard.
  *Criterion: Restarting the process with the same pending intent does not create a
  second RPC submission.*

- [ ] **IMPL-PIPE-08** Implement `FinalityWatcher` as a durable task: persists state
  between restarts, handles Pending -> Included -> Finalized | Dropped | Unknown.
  *Criterion: Process killed between Included and Finalized resumes watching on restart
  and correctly resolves to Finalized.*

### C.4 Mandate System

- [ ] **IMPL-MAN-01** Implement the mandate TOML schema and its Rust deserialization
  into the `Mandate` struct. Validate on load: no empty `allowed_chains`,
  `max_per_transaction <= max_per_day`.
  *Criterion: Invalid mandates produce descriptive validation errors on startup.*

- [ ] **IMPL-MAN-02** Implement `MandateEvaluator` trait with the full 13-step algorithm
  from §A.4.2.
  *Criterion: Each of the 13 check types can be exercised by a unit test with
  a synthetic intent.*

- [ ] **IMPL-MAN-03** Implement recipient glob expansion against the operator recipient
  registry. Support prefix globs (`bounty-*`) and exact matches.
  *Criterion: `bounty-*` matches `bounty-7` and `bounty-ops` but not `old-bounty-7`.*

- [ ] **IMPL-MAN-04** Implement `MandateAuditEntry` persistence. Every evaluation (pass
  or fail) is written to an append-only audit log.
  *Criterion: After 100 evaluations, all 100 entries are queryable by mandate ID.*

- [ ] **IMPL-MAN-05** Implement mandate lifecycle state machine (Draft -> Active ->
  Suspended -> Revoked; Active -> Expired on `expires_at`).
  *Criterion: A mandate with `expires_at` in the past refuses new evaluations.*

- [ ] **IMPL-MAN-06** Implement mandate `active_hours` enforcement using UTC-aware
  time comparison.
  *Criterion: An evaluation submitted at 03:00 UTC when `active_hours = 09:00-17:00`
  is Denied.*

### C.5 Budget Enforcement

- [ ] **IMPL-BUD-01** Implement `AgentBudgetState`, `MandateBudgetState`, and `RollingWindow`
  structs with serialization.
  *Criterion: A budget state round-trips through JSON without loss.*

- [ ] **IMPL-BUD-02** Implement `BudgetTracker` trait with durable persistence contract
  (persist before returning Authorized).
  *Criterion: A simulated crash between increment and persist does not allow double-spend
  in the test fixture.*

- [ ] **IMPL-BUD-03** Implement rolling window advance: when `now > window_start + window_duration`,
  reset the window value and update `window_start`.
  *Criterion: A budget window that expires mid-test resets correctly and allows new spend.*

- [ ] **IMPL-BUD-04** Implement per-run budget limits and their enforcement before
  mandate evaluation.
  *Criterion: A run with `max_spend = 5 DOT` refuses a 6 DOT action even if the mandate
  would allow it.*

- [ ] **IMPL-BUD-05** Implement organization aggregate budget and its check position in
  the evaluation pipeline.
  *Criterion: A per-agent-permitted spend that would exceed the org aggregate is denied.*

- [ ] **IMPL-BUD-06** Implement `BudgetAlerts` with webhook delivery. Send alert at
  `rolling_warning_threshold` and `lifetime_warning_threshold` fractions.
  *Criterion: Reaching 80% of daily budget triggers a webhook POST to the configured URL.*

- [ ] **IMPL-BUD-07** Implement `OverBudgetPolicy` (Deny, PauseAndNotify, EscalateToHuman).
  *Criterion: Each policy produces the correct agent state change and notification.*

- [ ] **IMPL-BUD-08** Implement `BudgetResetSchedule` variants (Rolling, Daily, Weekly,
  Monthly, Manual) with calendar alignment.
  *Criterion: A Daily schedule resets at UTC midnight regardless of when within the day
  the first spend occurred.*

---

## APPENDIX D: REFERENCE FILE MAP

| Component | Roko Files | Bardo Files | Key Patterns |
|---|---|---|---|
| Wallet / signer port | `/Users/will/dev/nunchi/roko/roko/crates/roko-chain/src/wallet.rs` | -- | `ChainWallet` trait: address, balance, nonce, sign-and-submit, receipt polling. Maps to Polkagent `Signer` port (§13.2). Split reads/writes so tests can mock one side. |
| State machine transitions | `/Users/will/dev/nunchi/roko/roko/crates/roko-conductor/src/state_machine.rs` | -- | `PhaseTransition` with `(from, to, at_ms, reason)`. Maps to `IntentEvent` in the payment state machine history (§12). Timeout-by-complexity-band pattern applies to intent signing timeouts. |
| Position/balance dashboard | -- | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/fate/positions.rs` | `PositionsScreenState` with `positions`, `total_nav_usd`, `total_unrealized_pnl_usd`. Maps to Polkagent balance dashboard showing asset breakdown, total NAV, unrealized surplus. `PositionRow` with in-range, fees-accrued, health-factor maps to Polkagent's `AssetBalance` row with ED margin and budget utilization. |
| Transaction approval card | -- | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/fate/trading.rs` | `TradingScreen`: permit queue (`PermitSummary`), warden delay queue (`WardenEntry`), gate result (`GateResultSummary`). Maps to Polkagent approval queue: pending intents, mandate-held actions, risk-gate findings. `approved/risk_score` maps to `MandateDecision::Authorized` / `RiskFinding`. |
| Risk dashboard | -- | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/fate/risk.rs` | `RiskScreenState` with `risk_score`, `drawdown_current_pct`, `drawdown_hwm`, `risk_metrics`. `risk_score_color` (green/yellow/red thresholds) and `drawdown_color` apply directly to Polkagent budget utilization coloring. `ViolationDisplay` maps to `RiskFinding`. `LayerResultDisplay` maps to pre-flight check layer results. |
| Budget sparkline | -- | Bardo braille widget: `/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/braille.rs` | Braille-encoded sparkline for time-series data. Apply to budget spend-over-time display in the budget tracker screen. |
| Rate limiting | `/Users/will/dev/nunchi/roko/roko/apps/mirage-rs/src/rate_limit.rs` | -- | Token-bucket or sliding-window rate limit. Maps to `RateLimit` in `BudgetConfig` (§8.2). |
| Finality watching | `/Users/will/dev/nunchi/roko/roko/apps/roko-chain-watcher/src/watcher.rs` | -- | `ChainWatcher`: subscribes to new blocks, matches known tx hashes, emits receipt events. Maps to Polkagent `FinalityWatcher` task (§4.7, §IMPL-PIPE-08). |
| RPC client | `/Users/will/dev/nunchi/roko/roko/apps/roko-chain-watcher/src/rpc_client.rs` | -- | RPC retry, backoff, and connection management. Adapts to `subxt` client for Polkadot. |
| Tool audit / receipt store | `/Users/will/dev/nunchi/roko/roko/crates/roko-fs/src/tool_audit.rs` | -- | Append-only audit log with query by (agent_id, timestamp). Direct model for `MandateAuditEntry` and `ReceiptStore`. |

---

## APPENDIX E: TUI SURFACE FOR PAYMENTS

The payment TUI uses `ratatui` (as in Bardo) with a screen-per-domain layout.
Keyboard navigation follows Bardo's j/k or Up/Down row selection, Tab/BackTab
for screen cycling, and `q` to quit.

### E.1 Position/Balance Dashboard

Inspired by Bardo's `PositionsScreen` (`positions.rs`), the balance dashboard
shows each asset held by the agent with margin-to-ED, budget utilization, and
block reference.

```
+------------------------------------------------------------------+
|  POLKAGENT / Balances                    block #22104821  [2s]  |
|------------------------------------------------------------------|
|  Asset          Balance       Transferable    ED margin  Budget  |
| ─────────────── ──────────── ─────────────── ─────────── ─────── |
|  DOT (native)   125.34       124.32          +123.32 DOT  18%   |
|  USDT (1984)      0.00         0.00            n/a         0%   |
|  USDC (1337)    500.00       500.00            n/a         0%   |
| ─────────────── ──────────── ─────────────── ─────────── ─────── |
|  Total NAV (DOT equiv):  ~627.45 DOT                            |
|  Daily budget:    10.40 / 50.00 DOT  ████░░░░░░░░  20.8%       |
|  Lifetime budget: 120.00 / 1000.00 DOT  ████████░░  12.0%      |
|------------------------------------------------------------------|
|  Last refresh: 2026-07-30T14:22:01Z   [r] refresh  [?] help    |
+------------------------------------------------------------------+
```

Key bindings: `j`/`k` -- select asset row; `Enter` -- drill into asset
transaction history; `r` -- force refresh; `Tab` -- next screen.

Colors mirror Bardo's `il_color` thresholds adapted to ED margin:
- ED margin > 10 DOT: green
- ED margin 1-10 DOT: yellow
- ED margin < 1 DOT: red (risk of reaping)

### E.2 Transaction Approval Card

Inspired by Bardo's `TradingScreen` permit queue and gate result display
(`trading.rs`).

```
+------------------------------------------------------------------+
|  PAYMENT APPROVAL REQUEST                         [Polkagent]   |
|================================================================= |
|  CANONICAL FIELDS (metadata-decoded)                            |
| ─────────────────────────────────────────────────────────────── |
|  Network:      Polkadot Hub  (genesis 0x91b1...)                |
|  Action:       balances.transferKeepAlive                       |
|  Recipient:    5F3sABC...9q                                     |
|                0x8eaf...a2  (raw AccountId32)                   |
|  Asset:        DOT / native balance                             |
|  Amount:       10.0000000000 DOT                                |
|  Fee cap:      <= 0.0150 DOT                                    |
|  Post-balance: 115.3250 DOT  (ED: 1.0 DOT  margin: +114.3)    |
|  Metadata:     spec 1002006 / hash 0xab...cd                    |
|  Dry-run:      PASS  (events: Transfer, Deposit)               |
|  Expiry:       10 min / 1 attempt                               |
| ─────────────────────────────────────────────────────────────── |
|  RISK FINDINGS                                                  |
|  [INFO] First payment to this recipient.                        |
| ─────────────────────────────────────────────────────────────── |
|  AGENT EXPLANATION (model-generated, not authoritative)         |
|  "Sends 10 DOT to Alice's treasury account. Balance stays      |
|   well above the 1 DOT existential deposit."                   |
|=================================================================|
|   Pending: 0   Warden queue: 0   Last gate: APPROVED risk=0.04 |
|================================================================= |
|  [ A ] Approve    [ R ] Reject    [ D ] Details    [ Q ] Quit  |
+------------------------------------------------------------------+
```

Bardo pattern applied: the gate result summary (`GateResultSummary`) appears
as a status bar line with color-coded approval state and risk score. The
canonical section above the separator is rendered from typed fields only;
the model explanation is below.

### E.3 Risk Dashboard

Inspired by Bardo's `RiskScreen` (`risk.rs`) with `RiskScreenState` providing
composite risk score, drawdown, and exposure breakdown.

```
+------------------------------------------------------------------+
|  POLKAGENT / Payment Risk Dashboard                             |
|------------------------------------------------------------------|
|  Risk score:   0.12  ████░░░░░░░░░░░░░░░░  LOW                 |
|  Budget util:  20.8%  ████░░░░░░░░░░░░░░░  GREEN               |
|  Rate limit:   3/10 actions/hr  ████░░░░░  30%                  |
|------------------------------------------------------------------|
|  VIOLATIONS                                                      |
|  (none)                                                          |
|------------------------------------------------------------------|
|  LAYER RESULTS                                                   |
|  Layer 0 (balance):    PASS                                     |
|  Layer 1 (recipient):  PASS                                     |
|  Layer 2 (metadata):   PASS                                     |
|  Layer 3 (dry-run):    PASS                                     |
|  Layer 4 (risk gates): PASS                                     |
|  Layer 5 (mandate):    PASS  (mandate: daily-treasury-ops)      |
|------------------------------------------------------------------|
|  EXPOSURE BY ASSET                                               |
|  DOT:   10.40 DOT spent today  (20.8% of daily 50 DOT)         |
|  USDT:   0.00 spent today                                       |
|------------------------------------------------------------------|
|  Circuit breaker: NOMINAL  (0 failures in last 1h)              |
|  Last assessment: 2026-07-30T14:22:03Z                          |
+------------------------------------------------------------------+
```

Color thresholds adapted from `risk_score_color` in `risk.rs`:
- Risk score < 0.3: green
- Risk score 0.3-0.7: yellow
- Risk score > 0.7: red

Budget utilization adapted from `drawdown_color`:
- Utilization < 50%: green
- Utilization 50-80%: yellow
- Utilization > 80%: red

### E.4 Budget Tracker (Sparkline)

```
+------------------------------------------------------------------+
|  POLKAGENT / Budget Tracker            24h rolling window       |
|------------------------------------------------------------------|
|  DOT spend (last 24h):  10.40 / 50.00 DOT                      |
|                                                                  |
|  00h  02h  04h  06h  08h  10h  12h  14h  16h  18h  20h  22h   |
|  ⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⡄⠀⠀⠀⠀⠀⠀⠀⠀⠀⡠⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀  (braille sparkline)  |
|  ─────────────────────────────────────────────────────────────  |
|  Tx count:  3 actions  (max 50/day; 10/hour)                    |
|  Last tx:   2026-07-30T14:20:11Z                                |
|  Next reset: 2026-07-31T00:00:00Z  (in 9h 38m)                 |
|------------------------------------------------------------------|
|  ALERTS                                                          |
|  (none -- below 80% threshold)                                  |
|------------------------------------------------------------------|
|  Lifetime:  120.00 / 1000.00 DOT  ████████░░░░░░░░  12.0%      |
|  Per-run:   10.40 / unlimited                                   |
+------------------------------------------------------------------+
```

The braille sparkline draws on the pattern from Roko's braille widget
(`/Users/will/dev/nunchi/roko/roko/crates/roko-cli/src/tui/widgets/braille.rs`).
Each braille cell encodes 2x4 pixels, giving an 8-row resolution spend chart
in a single text line. The Y-axis is scaled to the daily budget limit.

### E.5 Mandate Configuration Editor

```
+------------------------------------------------------------------+
|  POLKAGENT / Mandate Editor           mandate: daily-treasury   |
|------------------------------------------------------------------|
|  Status: ACTIVE  activated: 2026-07-01T09:00:00Z               |
|  Tier:   policy_bounded                                          |
|------------------------------------------------------------------|
|  SCOPE                                                           |
|  Actions:       native_transfer, asset_transfer                 |
|  Chains:        Polkadot Hub (0x91b1...)                        |
|  Assets:        DOT (native), USDT (1984)                       |
|  Recipients:    5F3s...9q  bounty-*                             |
|  Requires dry-run: YES                                           |
|------------------------------------------------------------------|
|  LIMITS                                                          |
|  Per-transaction:  10.0000 DOT                                  |
|  Per-day:          50.0000 DOT  (used: 10.40  remaining: 39.60)|
|  Lifetime:      1000.0000 DOT  (used: 120.00 remaining: 880.00)|
|  Auto-approve <: 1.0000 DOT                                     |
|------------------------------------------------------------------|
|  RATE LIMITS                                                     |
|  Max/hour:  10  (used: 3)     Max/day:  50  (used: 3)          |
|  Cooldown:  60s               Last action: 2026-07-30T14:20:11Z|
|------------------------------------------------------------------|
|  CIRCUIT BREAKER                                                 |
|  Trigger:   3 failures / 1h  Status: NOMINAL (0 failures)      |
|  Action:    pause_and_notify                                     |
|------------------------------------------------------------------|
|  EXPIRES:   2027-01-01T00:00:00Z  (in 155 days)                |
|  ACTIVE HRS: 09:00-17:00 UTC                                    |
|------------------------------------------------------------------|
|  [ E ] Edit field   [ S ] Suspend   [ R ] Revoke   [ Q ] Quit  |
+------------------------------------------------------------------+
```

### E.6 Transaction History and Receipt Viewer

```
+------------------------------------------------------------------+
|  POLKAGENT / Transaction History             agent: ops-agent-1 |
|------------------------------------------------------------------|
|  Date/Time (UTC)    Asset    Amount      Status      Recipient   |
| ─────────────────── ──────── ──────────── ─────────── ─────────── |
|  2026-07-30 14:20   DOT      5.0000      FINALIZED   5F3s...9q  |
|  2026-07-30 12:05   DOT      3.0000      FINALIZED   bounty-7   |
|  2026-07-30 09:33   DOT      2.4000      FINALIZED   5F3s...9q  |
|  2026-07-29 16:44   USDT    50.0000      FINALIZED   5Gx7...2a  |
|  2026-07-29 09:01   DOT      1.0000      FAILED      5F3s...9q  |
| ─────────────────── ──────── ──────────── ─────────── ─────────── |
|  Showing 5 of 47  [n] next page  [p] prev page  [/] filter     |
|------------------------------------------------------------------|
|  SELECTED RECEIPT: 2026-07-30 14:20 / 5.0000 DOT               |
|  Intent:     pay_intent_01J...                                  |
|  Auth:       MandateAuthorized (daily-treasury-ops)             |
|  Tx hash:    0xfa22...b9c1                                      |
|  Block:      #22104800  finalized                               |
|  Fee paid:   0.0031 DOT                                         |
|  Events:     Transfer, Deposit, TransactionFeePaid              |
|  [ X ] Export JSON   [ C ] Export CSV   [ Enter ] Full detail  |
+------------------------------------------------------------------+
```

---

## APPENDIX F: CONFIGURATION GUIDE

### F.1 Asset allowlist configuration

Assets are configured in `polkagent.toml` under `[payments.assets]`. Built-in
Phase 1 assets (DOT, USDT 1984, USDC 1337) require no configuration.

```toml
[payments.assets]
# Allow auto-discovery of new assets on enumerated chains.
# Default: false. When true, newly found assets appear in the admin UI
# for explicit acceptance before they become usable.
auto_discovery = false

# Staleness threshold: re-fetch asset metadata after this duration.
staleness_threshold = "10m"

# Hard expiry: block new intents if asset profile is older than this.
hard_expiry = "60m"

# Maximum consecutive refresh failures before blocking the asset.
max_refresh_failures = 3

# Additional operator-curated assets (beyond built-ins).
[[payments.assets.allowlist]]
chain_genesis = "0x91b171bb158e2d3848fa23a9f1c25182fb8e20313b2c1eb49219da7a70ce90c3"
location = { type = "PalletAsset", asset_id = 1984 }
label = "USDT on Asset Hub"
trust_level = "Verified"

[[payments.assets.allowlist]]
chain_genesis = "0x91b171bb158e2d3848fa23a9f1c25182fb8e20313b2c1eb49219da7a70ce90c3"
location = { type = "PalletAsset", asset_id = 1337 }
label = "USDC on Asset Hub"
trust_level = "Verified"
```

### F.2 Budget settings per agent/org

Budget settings live under `[payments.budget]` for the default agent and under
`[agents.<agent-id>.payments.budget]` for per-agent overrides.

```toml
[payments.budget]
# Per-run limits apply to a single conversation/trigger/task run.
per_run_max_dot = "5_000_000_000_000"   # 500 DOT per run
per_run_max_actions = 20

# Organization-level aggregate (across all agents).
[payments.budget.aggregate]
max_daily_dot = "1_000_000_000_000_000"  # 100,000 DOT/day
max_lifetime_dot = "none"                # unlimited

# Alert thresholds (fraction of the limit, 0.0-1.0).
[payments.budget.alerts]
rolling_warning_threshold = 0.80
lifetime_warning_threshold = 0.90
channels = [
    { type = "webhook", url = "https://ops.example.com/budget-alert", secret = "..." },
    { type = "email",   address = "ops@example.com" },
]

# Over-budget policy.
over_budget_policy = "deny"  # or "pause_and_notify" or "escalate_to_human"

# Reset schedule for the organization aggregate.
reset_schedule = "daily"  # "rolling", "daily", "weekly", "monthly", "manual"
```

### F.3 Mandate template library

Polkagent ships three mandate templates that operators can use as starting points.
Templates are applied with `polkagent mandate apply --template <name> [overrides]`.

**Template: `read-only`** (Tier 0, no payment authority)
```toml
[mandate]
id = "read-only"
tier = "read_only"
allowed_actions = []
```

**Template: `treasury-ops`** (Tier 2, daily treasury operations)
```toml
[mandate]
id = "treasury-ops"
tier = "policy_bounded"
allowed_actions = ["native_transfer", "asset_transfer"]
allowed_chains   = []          # Must be filled in by operator.
allowed_assets   = ["native"]
allowed_destinations = []      # Must be filled in by operator.
requires_dry_run = true
max_per_transaction        = "10_000_000_000"    # 1 DOT -- adjust to taste.
max_per_transaction_asset  = "native"
max_per_day                = "500_000_000_000"   # 50 DOT
max_per_day_asset          = "native"
max_lifetime               = "10_000_000_000_000" # 1000 DOT
max_lifetime_asset         = "native"
auto_approve_below         = "1_000_000_000"     # 0.1 DOT
auto_approve_below_asset   = "native"
max_actions_per_hour = 10
max_actions_per_day  = 50
cooldown_between_actions = "60s"
out_of_policy = "deny"
[mandate.circuit_breaker]
consecutive_failures = 3
within_duration = "1h"
action = "pause_and_notify"
```

**Template: `service-agent`** (Tier 3, fully autonomous service)
```toml
[mandate]
id = "service-agent"
tier = "fully_autonomous"
allowed_actions = ["native_transfer", "asset_transfer", "cross_chain_transfer"]
allowed_chains  = []           # Must be filled in by operator.
allowed_assets  = ["native", "asset:1984", "asset:1337"]
# No allowed_destinations restriction -- open sending.
requires_dry_run = true
max_per_transaction       = "100_000_000_000"       # 10 DOT
max_per_transaction_asset = "native"
max_per_day               = "1_000_000_000_000"     # 100 DOT
max_per_day_asset         = "native"
max_lifetime              = "none"                  # Must be set explicitly.
auto_approve_below        = "10_000_000_000"        # 1 DOT
auto_approve_below_asset  = "native"
max_actions_per_hour = 100
max_actions_per_day  = 1000
cooldown_between_actions = "10s"
out_of_policy = "deny"
[mandate.circuit_breaker]
consecutive_failures = 5
within_duration = "1h"
action = "pause_and_notify"
expires_at = ""  # Must be set explicitly by operator.
```

### F.4 Fee estimation tuning

```toml
[payments.fees]
# Maximum age of a fee estimate before re-fetching (in blocks).
max_estimate_age_blocks = 2

# Default tip strategy.
tip_strategy = "none"  # or "fixed:<planck>", "percentage:<0.0-1.0>"

# Fee cap multiplier: the fee cap shown in the action card is
# (estimated_fee * fee_cap_multiplier). Must be >= 1.0.
fee_cap_multiplier = 3.0

# Whether to treat XcmPaymentApi unavailability as a hard error.
# false (default): surface as uncertainty, allow action card with warning.
# true: block intent construction if XcmPaymentApi is unavailable for XCM calls.
xcm_payment_api_required = false
```

### F.5 Receipt storage settings

```toml
[payments.receipts]
# Storage backend.
backend = "sqlite"  # or "postgres", "sled"

# SQLite path (for local deployments).
sqlite_path = "/var/lib/polkagent/receipts.db"

# PostgreSQL connection string (for hosted deployments).
# postgres_url = "postgres://polkagent:secret@localhost/polkagent_receipts"

# Maximum age of receipts to retain in hot storage.
# Older receipts are archived to cold storage (if configured).
hot_retention = "365d"

# Export format for `polkagent receipt export` command.
default_export_format = "json"  # or "csv"

# Fields included in CSV export (must cover PAY-COMPL-006 requirements).
csv_fields = [
    "id", "intent_id", "agent_id", "run_id", "mandate_id",
    "sender", "recipient", "asset_id", "amount_raw", "amount_human",
    "fee_asset_id", "fee_raw", "fee_human",
    "authorization_type", "mandate_id",
    "tx_hash", "block_hash", "block_number", "finalized_at",
    "outcome", "created_at",
    # Reference fiat value (if price oracle configured):
    "amount_usd_approx", "fee_usd_approx",
]

# Optional: price oracle for fiat-equivalent reporting (PAY-COMPL-006).
[payments.receipts.price_oracle]
enabled = false
# provider = "coingecko"
# api_key = ""
# base_currency = "USD"
# cache_ttl = "5m"
```
