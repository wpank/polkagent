# Research — Polkadot, Polkadot SDK, JAM, and product landscape

**Purpose.** Source-backed input to the definitive Polkagent PRD. It maps
current Polkadot/Parity/W3F products and interfaces that could matter for a
Rust-first Polkadot agent platform that can run locally, self-hosted, or in a
managed multi-tenant environment through the same portable contracts.

**Research/access date:** 2026-07-29. **Source policy:** official Polkadot
Developer Docs/Wiki, Parity, W3F JAM site, and first-party crate documentation
only. Product status is fast-moving; every production decision must re-check
the linked source and target-network runtime metadata.

## How to read this document

This is deliberately self-contained. It describes the ecosystem first, then
states what Polkagent could integrate, what is safe to build now, and what must
remain an experiment. It uses the following labels consistently:

| Label | Meaning | May it enter the MVP without more proof? |
|---|---|---|
| **Verified fact** | Directly supported by the linked first-party source as of the access date | Yes, subject to normal adapter/security testing |
| **Inference** | A reasoned conclusion from verified facts | No; validate in a spike or user study |
| **Proposal** | A Polkagent product/architecture choice | Only after it is accepted in the definitive PRD or architecture decision record (ADR) |
| **Experimental / unknown** | Preview, roadmap, reverse-engineered, or insufficiently documented surface | No; isolate behind a research feature and explicit go/no-go gate |

The owner has established a broad long-term direction beyond the first
release: product engineering, on-chain action/payments, and PCA/mobile reach
are equal pillars; custody is configurable; any supported action—including
governance—may run fully autonomously when deliberately configured; and
marketplace publication is permissionless. Conservative statements below are
default or release-phase recommendations, not permanent platform
prohibitions.

**PCA** means `polkadot-chat-agents`, the existing reference product for
private Polkadot-oriented mobile/chat access to coding agents and framework
harnesses. Polkagent must preserve and improve its user-visible behavior while
replacing internal coupling.

### Plain-language glossary

| Term | Meaning in this document |
|---|---|
| **Polkadot** | A multi-chain network. Independent chains can use shared Polkadot security and communicate using a common message format. |
| **Polkadot Hub** | The Polkadot system chain/product surface for common application capabilities such as assets, identity-adjacent services, smart contracts, governance interaction and cross-chain operations. Older material may call relevant components Asset Hub; always use the target chain's live metadata rather than a name alone. |
| **Polkadot SDK** | Rust framework/tooling used to build Polkadot-compatible chains, runtimes and parachains (application-specific chains). It includes FRAME, the modular runtime-development framework, and Cumulus-related parachain support. |
| **Runtime** | The on-chain state-transition logic. Runtime metadata describes the currently available pallets, storage, events and callable extrinsics. A runtime upgrade can change the API an agent sees. |
| **Pallet** | A modular runtime component, for example balances, assets, governance, identity or proxy management. |
| **Extrinsic** | A signed or otherwise authorized request submitted to a Polkadot SDK chain, analogous to a transaction but broader than a simple value transfer. |
| **Subxt** | Rust library for querying Polkadot SDK chains and building/signing/submitting transactions using metadata-derived types. |
| **PAPI** | Polkadot API, a TypeScript client toolkit built around metadata-generated types and light-client-oriented operation. |
| **XCM** | Cross-Consensus Messaging: a standardized *format* for expressing instructions between consensus systems. It is not itself the transport, route, fee policy or success guarantee. |
| **EVM / REVM** | Ethereum Virtual Machine compatibility. REVM is the Rust EVM implementation used by Polkadot Hub's compatible contract surface. |
| **PVM / PolkaVM** | Polkadot Virtual Machine: a RISC-V-based deterministic execution environment. It appears both in Hub's PVM contract direction and in the prospective JAM architecture; these are related technology directions, not a reason to assume identical APIs. |
| **JAM** | Join-Accumulate Machine, a proposed future evolution of Polkadot's base architecture. It describes services and PVM execution but is not the production substrate on which Polkagent v1 should depend. |
| **People Chain** | A Polkadot system parachain that hosts the current on-chain identity pallet and registrar judgments. |
| **Bulletin Chain** | A specialized Polkadot storage-chain surface documented for CID/IPFS-compatible data storage; its documented testnet retention/availability constraints matter. |
| **OpenGov** | Polkadot's on-chain governance system of referenda, origins, tracks, conviction voting and delegation. |
| **Signer** | The boundary that authorizes a transaction payload. A signer can be a wallet, hardware device, Vault/air-gapped flow, proxy/multisig process or test double. It is not an LLM and must not expose its seed to an agent. |
| **Intent** | Polkagent's proposed immutable description of an action before execution. An intent is reviewable, constrained, expiring and evidence-linked; it is not a natural-language request. |
| **Finality** | Strong confirmation that a submitted chain action has become part of the finalized chain. Submitted, included and finalized are different states. |
| **ED** | Existential deposit: a target runtime’s minimum-balance/account-survival rule. It is profile-specific and must be queried or derived from current chain evidence. |

## Executive recommendation

Polkagent should begin with an **off-chain, portable agent runtime** that can
read Polkadot data and construct reviewable intents. The same runtime should
support user-owned local operation and managed/cloud operation. Its secure
default is:

```text
read/query → explain/simulate → create intent + evidence → independent approval/signer
→ submit → wait for receipt/finality → report durable outcome
```

**Proposal:** make Rust
[`subxt`](https://docs.polkadot.com/reference/tools/subxt/) against explicitly
pinned Polkadot/Hub/People metadata, plus local test fixtures, the first chain
integration baseline. **Verified facts:** official documentation exposes Hub
EVM/REVM, Assets/XCM, and PVM contract surfaces, while the PVM documentation
labels that path early-stage/preview. **Inference:** JAM is strategically
important but too prospective for a v1 dependency; no Polkagent v1
correctness, identity, payment, or workflow claim should depend on a JAM
service.

The existing `polkadot-chat-agents` protocol is the most direct route to a
Polkadot-native conversation transport, but the public documentation surveyed
here does **not** establish a stable public App Chat/Statement Store/Bulletin
messaging SDK. Treat that transport as a separately versioned, fixture-gated
adapter, not the core runtime's identity or execution substrate.

## Ecosystem primer: where Polkagent fits

### The product landscape in one picture

```text
Human / organisation
  ├─ controls wallet, signer, policy, workspace and provider accounts
  └─ talks to Polkagent through local CLI/web and, later, a chat transport

Polkagent (off-chain Rust runtime)
  ├─ durable run/effect/evidence store
  ├─ policy + approval + external signer handoff
  ├─ model/harness/tool adapters (optional)
  ├─ chain adapters: Subxt first; optional EVM and XCM planners
  └─ transport adapters: local test transport first; chat only after proof

Polkadot ecosystem
  ├─ Polkadot Hub: assets, contracts, cross-chain application surface
  ├─ People Chain: on-chain identity records and registrar judgments
  ├─ Relay/system/parachains: target-specific runtimes and governance
  ├─ Bulletin: optional CID-addressed artifact storage surface
  └─ future JAM: prospective services/PVM architecture
```

**Verified fact:** Polkadot Hub documentation presents the Hub as an application
entry point with assets, smart contracts, governance, identity-related features
and XCM interoperability. [Polkadot Hub reference](https://docs.polkadot.com/reference/polkadot-hub/)

**Proposal:** Polkagent is not itself a blockchain, RPC protocol, or contract
execution layer. It is an off-chain orchestrator that converts user requests
and triggers into durable, policy-constrained, reviewable operations across
those systems. It may productize wallet/signer custody adapters, managed
operation, and a permissionless marketplace, but those remain separable
services rather than powers granted to a model.

### Why an agent needs a different design from a normal dApp

A normal dApp often asks a wallet to sign one known transaction. An agent may
read many sources, call tools, retain a model session, retry after crashes, and
propose an external effect later. That multiplication of steps creates four
failure classes Polkagent must make explicit:

1. **Interpretation risk:** the model might misunderstand a requested action,
   asset, recipient, contract or governance proposal.
2. **Runtime drift:** a Polkadot runtime upgrade can change the metadata/schema
   used to encode calls or interpret events.
3. **Effect/retry risk:** a process might crash after an external action was
   sent but before its outcome was recorded.
4. **Authority risk:** an agent or plug-in might get more access than the human
   intended if secrets, signer authority or tool permissions are inherited.

**Proposal:** the runtime must therefore use a durable `EffectIntent` and
outbox, pin high-risk chain interpretation to a chain profile and metadata
version, and separate preparing an action from approval, signing, submission
and finality observation.

### Concrete default flow: Explain Before Sign

The first credible Polkagent flow is intentionally narrow. For example, an
advanced user asks, “What will this asset transfer do?” and provides a proposed
payload or selects a supported action in the local UI.

```text
1. Identify profile     → target chain genesis hash + RPC + metadata revision
2. Decode canonically   → pallet/call/arguments from metadata, not model guess
3. Read/preflight       → balances, nonce, asset details, fee/ED where supported
4. Apply policy         → deny / require approval / authorize by mandate, with a ResolvedGrant
5. Render evidence card → recipient, asset, decimal amount, fees, risks, source hashes
6. Authorize            → user/quorum approves, or an eligible configured mandate authorizes exact IntentId
7. Isolated signer      → wallet/hardware/KMS/MPC/agent signer receives canonical expiring payload
8. Submit/watch         → submitted → included → finalized / failed / unknown
9. Persist/report       → durable receipt and redacted timeline
```

The first release uses the human-approved branch. A policy-autonomous or fully
autonomous deployment follows the same evidence, grant, payload-binding,
submission, and finality stages; it replaces per-action human approval with an
unexpired `AuthorizationDecision` tied to a configured mandate and policy
revision. The model does not become the authorizer.

Illustrative core types:

```rust
struct ChainProfileRef {
    id: String,
    genesis_hash: [u8; 32],
    metadata_hash: [u8; 32],
    runtime_spec_version: u32,
}

struct ChainIntent {
    id: IntentId,
    profile: ChainProfileRef,
    call_scale: Vec<u8>,
    decoded_call: DecodedCallSummary,
    signer_policy: SignerPolicyRef,
    max_fee: Option<AssetAmount>,
    expires_at: Timestamp,
    evidence: Vec<ArtifactRef>,
}

enum ChainActionStatus {
    Drafted, Authorized, Signed, Submitted { hash: TxHash },
    Included { block: BlockHash }, Finalized { block: BlockHash },
    Failed { reason: ChainFailure }, Unknown,
}
```

These types are a **proposal**, not an assertion about current source code. The
important design rule is that a model-generated sentence never becomes
`call_scale` without canonical chain decoding, policy and explicit signer
handoff. `ChainActionStatus` is the overall saga lifecycle. It is distinct
from an `EffectOutcome`, which is the immutable observed result of one
`EffectAttempt`; signing, submission, and finality watching each have their
own attempts/outcomes even though they advance one chain action.

## Maturity and support matrix

Status labels: **Production baseline** = documented/current route suitable for
an adapter after normal security testing. **Supported with qualification** =
usable, but pin network/runtime/API and add conformance fixtures. **Preview /
evolving** = do not make v1 correctness depend on it. **Research only** = no
product commitment until a concrete implementation/testnet/API is verified.

| Area | Current landscape / integration | Status for Polkagent | Product opportunity | Principal risk | Primary source(s), accessed 2026-07-29 |
|---|---|---|---|---|---|
| Polkadot SDK/FRAME/Cumulus parachains | Rust SDK, runtime/pallet development, chain specs, forkless upgrades; local Zombienet/Chopsticks | Production baseline for **testing against**, not needed to run agent v1 | Agent-assisted pallet/runtime/chain maintenance workflows | Runtime-specific API changes; do not treat every SDK chain as identical | [Parachain development](https://docs.polkadot.com/develop/parachains/), [Zombienet](https://docs.polkadot.com/develop/toolkit/), [Chopsticks](https://docs.polkadot.com/develop/toolkit/parachains/fork-chains/chopsticks/) |
| Rust chain client | `subxt` + `subxt-signer`; metadata-generated types, reads, subscriptions, transaction watch/finality | Production baseline | Read-only research; intent builder; policy-gated submission adapter | Metadata/runtime drift; signer must stay separate from executor | [Subxt Rust API](https://docs.polkadot.com/reference/tools/subxt/) |
| TypeScript clients | PAPI light-client-first; Dedot recommended; Polkadot.js in maintenance mode | Supported with qualification / bridge tooling | Compatibility sidecar or generated fixture tooling, not core | Avoid Node client dependency in Rust core | [PAPI](https://docs.polkadot.com/reference/tools/papi/), [Dedot](https://docs.polkadot.com/reference/tools/dedot/), [Polkadot.js API status](https://docs.polkadot.com/reference/tools/polkadot-js-api) |
| Polkadot Hub assets / foreign assets | Native Assets pallets, asset metadata/roles, foreign assets keyed by XCM location, delegated transfers | Production baseline after target metadata tests | Treasury/payment assistant, asset discovery, controlled transfer intents | Asset impersonation/location confusion; permissions and fees | [Hub assets](https://docs.polkadot.com/reference/polkadot-hub/assets/) |
| Hub EVM (REVM) | Ethereum JSON-RPC, Solidity/tool compatibility, precompiles | Production baseline for EVM adapter | Contract read/verify/intent workflows | Account mapping, RPC endpoints, non-EVM runtime semantics | [Smart contracts overview](https://docs.polkadot.com/smart-contracts/overview), [JSON-RPC APIs](https://docs.polkadot.com/smart-contracts/for-eth-devs/json-rpc-apis/) |
| Hub PVM / PolkaVM contracts | RISC-V backend, Solidity/resolc path, multi-dimensional metering | Preview/evolving | Future high-performance agent escrow/workflow components | Documentation says early-stage / tooling limited; no v1 dependency | [PVM design/status](https://docs.polkadot.com/polkadot-protocol/smart-contract-basics/polkavm-design/) |
| XCM / XCMP | XCM is a message format; Hub assets and contracts can use it | Supported with qualification | Cross-chain asset/intent planner and simulator | XCM location/fee/route/version mistakes; no autonomous dispatch until a route/action family passes evidence, simulation, custody, recovery and mandate gates | [XCM overview](https://docs.polkadot.com/parachains/interoperability/get-started/) |
| People Chain identity | on-chain identity, registrars/judgments, sub-identities, bonds | Production baseline for **read-only signals** | Verified-agent/operator profile and policy inputs | Identity is not universal personhood/trust; privacy and registrar semantics | [People Chain](https://docs.polkadot.com/reference/polkadot-hub/people-and-identity/) |
| Proof of personhood / Individuality | announced/developing Bandersnatch/RingVRF direction; product specifics still shifting | Preview/research | Anti-spam or human-gated access later | Do not gate basic use or assume availability / policy semantics | [Parity 2025 roundup](https://www.parity.io/blog/polkadot-roundup-2025) |
| OpenGov | referenda, origins/tracks, delegation, conviction | Production baseline for read/analysis; submission and autonomy independently gated | Governance brief, proposal evidence, vote/delegation intent review and later configured autonomy | High financial/governance impact; changing track parameters; no autonomous vote in the initial integration phase | [OpenGov overview](https://docs.polkadot.com/polkadot-protocol/onchain-governance/overview/), [origins/tracks](https://docs.polkadot.com/polkadot-protocol/onchain-governance/origins-tracks/) |
| Wallets, proxies, multisig, hardware | ecosystem wallets, Ledger/Vault, account mapping, narrowly scoped proxy patterns | Supported with qualification | External signer handoff; proxy-limited operations | Key custody, signed-payload deception, unsupported wallet UX | [Wallet landscape](https://docs.polkadot.com/develop/toolkit/integrations/storage/), [Hub account mapping](https://docs.polkadot.com/smart-contracts/connect), [staking-operator proxy](https://docs.polkadot.com/node-infrastructure/run-a-validator/operational-tasks/staking-operator-proxy/) |
| Bulletin storage | CID/IPFS-compatible storage on testnet docs; authorization, retention, chunking | Preview/testnet-qualified | Artifact anchoring/proof package experiment | ~2 week testnet retention, allowance/authorization and availability assumptions | [Bulletin data storage](https://docs.polkadot.com/reference/polkadot-hub/data-storage/) |
| Polkadot App Chat / Statement Store | official Host and Statement Store documentation exists, but no stable standalone external-agent/PCA compatibility contract was established | Research/fixture-gated | Private outbound-only agent conversation transport and optional Product/mobile companion | Public primitive/Host behavior does not specify PCA encryption, session, ACK, ordering, registration, or recovery semantics | PCA commit `2adddcc8cfd732804cd9bbcbcd26974b44b47f66`; [official Host Statement Store interface](https://docs.polkadot.com/reference/apps/hosts/polkadot-desktop/statement-store/); [Product messaging architecture](https://docs.polkadotcommunity.foundation/architecture/messaging/) |
| JAM / PVM services | future successor design; Graypaper, prize/conformance efforts, services/refine/accumulate/onTransfer | Research only | Long-term decentralized agent service / verifiable workflow experiments | Not deployed product surface; transactionless model and tooling maturity | [JAM Chain](https://wiki.polkadot.com/learn/learn-jam-chain/), [W3F JAM program](https://jam.web3.foundation/), [Parity JAM context](https://www.parity.io/blog/polkadot-roundup-2025) |
| Local network tools | Zombienet, Chopsticks, Pop CLI, Paseo | Production baseline for integration testing | Reproducible chain + agent E2E test kit | Fork/tool limitations (e.g. Chopsticks EVM RPC) | [Zombienet](https://docs.polkadot.com/develop/toolkit/), [Chopsticks](https://docs.polkadot.com/develop/toolkit/parachains/fork-chains/chopsticks/), [Pop CLI](https://docs.polkadot.com/reference/tools/pop-cli/) |

## Actors, authority and product surfaces

The same account can technically be used for many purposes, but Polkagent
should not model it that way. The following separation is central to user trust.

| Actor/surface | What it owns | What it must never implicitly receive |
|---|---|---|
| User / operator | The policy, approved workspace, provider account, chain profile and choice of signer | A surprise spending commitment, background authority grant or hidden hosted dependency |
| Polkagent core | Durable state machine, evidence references, policy evaluation, outbox and adapter coordination | Wallet mnemonic, unrestricted filesystem access, a provider's raw credential in event logs |
| Executor/harness | A scoped model/CLI/process session for one turn | Transport identity seed, authoritative database, unbounded host files, unrestricted signer ability |
| Tool adapter | Narrow operation such as chain read, filesystem access, browser session or HTTP query | The ability to choose its own authorization or widen its own network/path limits |
| Chain client | Queries, subscriptions, call construction, preflight and submission observation | A private key unless it is explicitly a signer implementation; authority to bypass policy |
| External signer | Canonical reviewed payload authorization | Model prompt, arbitrary shell arguments, transport/session credentials |
| Wallet/mobile/hardware device | User-visible confirmation and signature | A claim that a signature was final chain execution; signature completion is not finality |
| Polkadot App/Chat transport, if added | Delivery of encrypted conversation content | Authority to execute tools or sign merely because a message arrived |
| Remote RPC/provider | A service endpoint selected by configuration | Authority over local truth; results must be classified and retried/verified as appropriate |

**Inference:** product language should consistently say “Polkagent proposes,
checks and hands off”; it should reserve “executed” for a recorded adapter
receipt and “finalized” for a verified chain observation. This prevents a
natural-language agent response from being mistaken for a wallet or chain fact.

### Mobile and wallet role

**Verified fact:** Polkadot documentation lists non-custodial wallet options,
including browser/mobile wallets and cold-signing options such as Ledger and
Polkadot Vault. Hub documentation describes account mapping needed when a
native 32-byte Substrate account is used with the Ethereum-compatible contract
layer. [Wallets](https://docs.polkadot.com/develop/toolkit/integrations/storage/), [Hub account mapping](https://docs.polkadot.com/smart-contracts/connect)

**Proposal:** Polkagent should make the mobile/wallet layer a *remote-control
and consent surface*, not a secret-storage dependency. A future mobile/App
experience may receive a run notification, inspect an evidence card, approve a
bound intent, and invoke an external signer; the local runtime remains the
authoritative owner of recovery/outbox state.

### Product surfaces by time horizon

| Surface | What a newcomer sees | Earliest phase | Why it exists |
|---|---|---|---|
| Local CLI / minimal web view | `status`, `doctor`, `turns show`, approval card, intent export | MVP | Operator control and trustworthy diagnostics without transport complexity |
| External wallet / hardware handoff | Exact network/call/value/fee/expiry payload | MVP gate, after fake signer proof | Independent consent and key custody |
| Chain reader | Read-only account, asset, governance and transaction evidence | MVP | The agent can be useful without moving value |
| Polkadot chat / App companion | Private message starts a bounded run and receives final status | Later, only after compatibility proof | A differentiated conversation channel, not MVP foundation |
| Browser / external web evidence tool | Isolated named browser session with snapshots and approval for effects | Later | Evidence gathering/verification, not generic uncontrolled browsing |
| Agent Studio / kits / plug-in catalog | Configured agent profiles, skills/context packs and approved adapters | Later | Reusable product engineering, only after core policy boundaries work |
| Public paid agent | Metered, abuse-resistant public access with explicit rails | Horizon | Requires legal, fraud, economics and operational proof |

## Chain mechanics primer: what an agent must know before it acts

### Metadata is the API contract

Polkadot SDK chains publish runtime metadata describing available pallets,
storage entries, events, constants, types and callable extrinsics. **Verified
fact:** Subxt's documented workflow downloads metadata and generates a static
Rust interface; PAPI similarly generates TypeScript descriptors from metadata.
[Subxt docs](https://docs.polkadot.com/reference/tools/subxt/), [PAPI docs](https://docs.polkadot.com/reference/tools/papi/)

This means an agent cannot safely hard-code “transfer means these bytes” across
all networks or forever on one network. A runtime upgrade may change call shape,
types, fees or the behavior a call invokes.

**Proposal: Chain Profile.** Every supported Polkagent network is a named,
versioned profile:

```text
profile_id       polkadot-hub-paseo-v1
genesis_hash     immutable network identity
RPC set          explicitly configured/failover endpoints
metadata hash    accepted runtime interface snapshot
spec/tx version  current compatibility facts
call allowlist   exact pallet/call combinations and argument constraints
asset registry   canonical locations/IDs, decimal/issuer provenance
signer support   tested wallet/hardware/proxy methods
test evidence    fixture corpus and last revalidation date
```

When the live chain's profile does not match a high-risk expected metadata
snapshot, Polkagent must produce `UnsupportedOrDrifted`, not guess, dynamically
construct a write call, or silently “upgrade” a pending approval.

### From a user request to a final chain result

| Stage | What is known | What may still go wrong | Required user-visible wording |
|---|---|---|---|
| Draft | A user/model has requested an action | Request may be ambiguous or malicious | “Proposed; not signed or sent” |
| Canonical decode | Metadata decoded the call and arguments | Wrong profile, stale metadata, unknown call/asset, misleading display data | “Decoded for `<network>` at metadata `<hash>`” |
| Preflight | Current reads/estimate/simulation were observed | State, fee, nonce, route or contract state can change | “Estimate/preflight; not a guarantee” |
| Approved | User accepted exact bounded intent | Approval may expire or be superseded | “Approved for this exact payload until `<time>`” |
| Signed | External signer authorized payload | Submission can fail; signature can become stale | “Signed; not yet sent/finalized” |
| Submitted | RPC accepted/broadcast request | It may be dropped/replaced/rejected later | “Submitted; awaiting inclusion” |
| Included | Chain block included the extrinsic | It may not be final; event outcome can fail | “Included in block; awaiting finality” |
| Finalized | Finality watcher observed final chain result | Downstream XCM/contract effects can still have their own lifecycle | “Finalized” with receipt/event evidence |

### An illustrative transfer intent

```text
Network:       Paseo Polkadot Hub (genesis 0x…)
Action:        balances.transfer_allow_death
Recipient:     5F…9q (SS58 + raw AccountId32 shown)
Asset:         DOT / native balance (canonical identifier: …)
Amount:        12.5000000000 DOT
Fee cap:       <= 0.015 DOT
Account impact: sender balance after action must remain >= configured minimum
Metadata:      spec 1234 / hash 0x…
Evidence:      account state at block 0x…, fee quote at block 0x…
Expiry:        10 minutes; one submission attempt
Signer:        external wallet `owner-1`, no seed supplied to Polkagent
```

The agent may explain the card in prose, but the trusted UI is rendered from
typed fields. If an amount cannot be decoded with a canonical decimal scale, the
product should refuse rather than display a confident number.

## 1. Polkadot Hub: the practical dApp and asset surface

Polkadot Hub is documented as the primary entry point for applications: it
supports assets, identity, governance, staking, XCM, and contract deployment
without operating a parachain. [Polkadot Hub reference](https://docs.polkadot.com/reference/polkadot-hub/)

### Assets and payments

Hub's Assets functionality covers native and foreign assets, asset roles and
metadata, and delegated transfers. Foreign assets use an XCM multilocation as
their identifier. The docs also describe transaction fee payment in supported
assets rather than DOT only. [Hub assets](https://docs.polkadot.com/reference/polkadot-hub/assets/), [alternative-fee tutorial](https://docs.polkadot.com/tutorials/polkadot-sdk/system-chains/asset-hub/send-a-tx-paying-fee-different-token/)

**Profile rule:** an asset ID, decimal scale, sufficiency/keep-alive property,
fee eligibility, origin location, and existential-deposit consequence are not
portable facts about a ticker. They must be queried from the selected chain at
a recorded block and retained with genesis hash, metadata hash, and runtime
version. An unknown or drifted profile must refuse to render a confident amount
or fee claim.

**Polkagent opportunities**

- Portfolio/treasury read model: balances, asset origin, metadata, fee asset,
  known location, transfer history/evidence.
- Policy-bound transfer intent: recipient, amount, asset location/ID,
  existential-deposit consequences, fee asset, runtime call bytes and
  expected/maximum fee all rendered before independent signing.
- Controlled delegated-transfer operations for an explicitly configured proxy
  account, never based only on a chat claim.
- Stablecoin/product workflows should use *specific target-network asset IDs
  and origin verification*. An ERC-20-shaped precompile does not prove that a
  display token is the intended issuer asset. Hub maps Assets pallet instances
  to ERC-20 precompiles. [ERC-20 precompile](https://docs.polkadot.com/smart-contracts/precompiles/erc20/)

**Risks / requirements**

- Asset metadata/name/ticker are not sufficient identity. Maintain an
  operator-curated asset allowlist keyed by chain genesis hash + runtime version
  + full location/asset identifier; display branding is untrusted.
- A fee-estimation, simulation, and nonce snapshot are inputs to an intent,
  not guarantees of execution. Re-check immediately before signing/submitting.
- Do not use agent-owned wallet seeds for user assets. The signer is an external
  port with a human-visible payload and an explicit network/genesis binding.

### EVM/REVM and PVM contracts

Polkadot Hub's documented dual VM design combines REVM for unmodified Ethereum
bytecode and PVM (PolkaVM) for RISC-V-based execution. Hub docs position REVM
as familiar EVM JSON-RPC/Hardhat/Foundry/MetaMask compatibility, whereas PVM is
the native performance path. [Smart-contract overview](https://docs.polkadot.com/smart-contracts/overview), [dual VM stack](https://docs.polkadot.com/smart-contracts/for-eth-devs/dual-vm-stack)

The PVM contract page explicitly calls Ethereum-compatible PVM smart contracts
an early-stage preview and potentially unstable/incomplete; the EVM-vs-PVM page
also documents three-dimensional `ref_time`/`proof_size`/storage-deposit
metering and material compatibility differences. [PVM design/status](https://docs.polkadot.com/polkadot-protocol/smart-contract-basics/polkavm-design/), [EVM vs PVM](https://docs.polkadot.com/smart-contracts/for-eth-devs/evm-vs-pvm/)

| Contract-facing feature | v1 decision | Why |
|---|---|---|
| EVM read calls / logs / ABI decoding | Optional supported adapter | Standard Ethereum JSON-RPC and mature existing Rust/EVM libraries make this tractable, provided chain allowlist and ABI provenance are explicit. |
| EVM transaction intent | Gated Phase 2+ | Build/calldata-decode/simulate/render/sign separately; no blind contract call from model text. |
| PVM read/deploy/call | Research spike | Better native Rust alignment but documented preview/tooling limitations make it unsuitable as a product dependency. |
| On-chain agent contracts | Not v1 | Off-chain durable workflow/policy is more adaptable and avoids irreversible contract surface before product fit. |

**High-value use case:** an agent explains a contract's ABI-call effect and
produces a transaction-intent package, while a connected wallet displays and
signs independently. It must never claim a call is safe merely because it
simulated once or because it uses an EVM ABI.

## 2. SDKs, APIs and runtime compatibility

### Rust first: Subxt

Subxt is the official documented Rust client for Polkadot SDK chains. The
workflow downloads target metadata, generates static type-safe interfaces,
reads storage/events/runtime APIs, and can sign/submit/watch transactions with
`subxt-signer`. [Subxt Rust API](https://docs.polkadot.com/reference/tools/subxt/)

**Recommended adapter architecture**

```text
polkagent-chain-types       stable domain intent / receipt / policy types
polkagent-chain             ChainClient + Signer + simulation ports
polkagent-chain-subxt       target metadata, RPC, subscriptions, dynamic fallback
polkagent-chain-fixtures    per-network .scale metadata, encoded-call/event vectors
polkagent-chain-policy      allowlisted calls, destinations, assets, fee/cost rules
```

- Pin metadata per supported network/profile and check the runtime/metadata hash
  at startup. Do not deserialize arbitrary dynamic calls supplied by the model.
- Generate static interfaces for high-value known calls; use dynamic metadata
  only for inspect/explain paths, guarded by schema/type/size limits.
- Maintain a `CallIntent` containing genesis hash, metadata hash/version,
  pallet/call, SCALE arguments, human-decoded summary, origin/signer policy,
  nonce/era, fee estimate, simulation evidence, expiry and idempotency key.
- `Signer` never returns a seed to the runtime/executor. It signs a canonical,
  reviewed payload through a hardware/mobile/wallet/proxy interface and returns
  a signature or refusal.

### TypeScript and REST companions

PAPI is a light-client-first TypeScript toolkit with metadata-generated types,
multi-chain support and descriptor compatibility checks. Polkadot.js API is
documented as maintenance-only; new projects are directed to PAPI or Dedot.
[PAPI](https://docs.polkadot.com/reference/tools/papi/), [Polkadot.js API status](https://docs.polkadot.com/reference/tools/polkadot-js-api)

These are valuable for fixture generation, front-end/wallet bridges, and
cross-checking a native Rust adapter. They do not justify embedding Node in
Polkagent's main runtime. Sidecar is a REST facade option but adds a separate
Node service; use only where an external integration demonstrably requires it.
[Sidecar](https://docs.polkadot.com/reference/tools/sidecar/)

## 3. XCM: cross-chain intent, not a generic send primitive

XCM defines a standardized **message format**; it is not itself a delivery
protocol. The receiving/sending consensus systems and channel configuration
matter. [XCM overview](https://docs.polkadot.com/parachains/interoperability/get-started/)

**Implications**

- Model XCM as a typed `CrossChainIntent` with source chain, destination
  location, asset identity/location, beneficiary, route/channel version,
  execution/weight/fee assets, refund behavior and final expected events.
- Default to read/plan/simulate. A real message is at least as consequential as
  a transfer and can fail partly/asynchronously across chains.
- Require a test path in Chopsticks/Zombienet plus target-testnet evidence for
  every supported route. Do not infer that an XCM route works because either
  chain individually supports the asset.
- Probe the selected source and destination runtime APIs and record their
  response shape/version in the route fixture. `DryRunApi`/`XcmPaymentApi` and
  emitted/forwarded XCM details are useful where present, but are
  runtime/profile capabilities rather than a universal cross-chain contract.
- Treat remote execution as a high-risk effect. There is no agent shortcut
  around destination chain policy, message version, assets, reserve/teleport
  rules, fee availability and runtime upgrade compatibility.

**Near-term opportunity:** XCM-aware governance/treasury research that explains
where an asset lives, how it may move, and what dependencies a proposed
operation has—without submitting the operation.

## 4. Governance: explain and prepare first; autonomous voting when configured

**Verified fact:** OpenGov supports concurrent referenda, origins/tracks with different
thresholds/deposits/timing, conviction voting and track-specific delegation.
[OpenGov overview](https://docs.polkadot.com/polkadot-protocol/onchain-governance/overview/), [origins/tracks](https://docs.polkadot.com/polkadot-protocol/onchain-governance/origins-tracks/)

| Feature | Safe initial/default product role | Default restriction |
|---|---|---|
| Referendum discovery | Monitor, explain current state/track/timeline, link source/external discussion, maintain watchlist | Claim a proposal's content/effect without a content hash/preimage + metadata evidence |
| Proposal analysis | Produce structured brief: origin, call, target runtime, deposits, enactment, affected pallets/assets, uncertainty | “Vote yes/no” as personalized financial/governance advice or unattended action |
| Delegation | Explain existing delegations and generate a reviewed delegation intent | Set/replace delegation automatically |
| Vote/submission | Construct an intent only after explicit requested action | Agent uses a stored seed, proxy, or prior approval to cast a vote |

OpenGov track parameters are runtime governance data, not constants. Re-query
and record block/hash at the time of each explanation and signing request.
This includes track/origin availability, decision/deposit/enactment timing,
conviction and lock consequences, delegation state, and the referenced
preimage/call. A governance brief must describe an unavailable or stale field
as unknown rather than importing values from another network or prior runtime.

These are safe defaults and early release boundaries. The complete platform
must also support policy-autonomous and fully autonomous governance when an
authorized owner deliberately configures an eligible signer/account, tracks,
calls, budgets, evidence rules, failure behavior, and revocation. The signer
still receives an exact metadata-bound payload; an LLM never turns its own
recommendation into authority.

## 5. Identity, personhood and agent identity

**Verified fact:** People Chain is the current specialized identity parachain. It supports bonded
identity information, registrar judgments and linked sub-identities; registrar
judgment confidence is an attestation model, not a universal personhood or
authorization guarantee. [People Chain reference](https://docs.polkadot.com/reference/polkadot-hub/people-and-identity/), [identity guide](https://wiki.polkadot.com/learn/learn-identity/)

**Product design**

- Maintain three identities independently: `operator` (human/policy owner),
  `agent transport identity` (messageable account/keys), and `chain signer`
  (wallet/proxy/multisig used for intent authorization). They may be linked but
  must never be conflated.
- Use People Chain only as an optional **read-only signal**: verified display,
  linked account, registrar status, or policy allowlist input. It cannot by
  itself prove that an incoming chat message is safe or that a signer consented.
- Avoid putting personal data, private prompts, raw agent memory, or secrets in
  identity fields. On-chain data/bonds/registrar processes have different
  privacy and retention properties from local runtime state.

Parity's 2025 roundup reports initial deployment work on an Individuality,
proof-of-personhood direction using Bandersnatch RingVRF, but product APIs and
policy semantics need separate confirmation before use. [Parity 2025 roundup](https://www.parity.io/blog/polkadot-roundup-2025)

**Potential later opportunity:** optional personhood-backed rate limits for a
public agent, using proof verification as a policy input. This is explicitly
not v1: no current PRD should promise sybil resistance or impose identity/KYC.

## 6. Signers, wallets, proxies and account mapping

The documented ecosystem includes non-custodial hot wallets, Ledger/Polkadot
Vault cold signing, and wallet integration libraries. Hub can require mapping
native 32-byte Substrate accounts to an Ethereum-compatible account for the EVM
layer. [wallet landscape](https://docs.polkadot.com/develop/toolkit/integrations/storage/), [Hub connection/account mapping](https://docs.polkadot.com/smart-contracts/connect)

Polkadot's staking-operator proxy illustrates the correct security pattern:
delegated operations are limited to necessary calls and exclude balance-moving,
bonding and proxy-management actions; delays and revocation reduce blast radius.
[staking operator proxy](https://docs.polkadot.com/node-infrastructure/run-a-validator/operational-tasks/staking-operator-proxy/)

**Signer policy**

1. The first external-signer release has `WatchOnlySigner`,
   `ExternalWalletSigner`, and `IntentExporter`; no seed phrase in
   `polkagent.toml`, agent prompt, plugin environment, log or bridge request.
   Later local encrypted, KMS/HSM, MPC, proxy, multisig, programmable, and
   funded agent-account signers still use the same isolated signer port.
2. The agent may request **draft**, **simulate**, then **sign**. Each stage has
   a distinct event/approval. Sign and submit may be separate confirmations.
3. Proxies/multisig are operator-configured bounds, never an excuse to skip
   payload review. A proxy's call filter is a second enforcement layer.
4. Wallet response must be correlated to exact genesis/payload/call/metadata
   hashes and an expiring `IntentId`; reject a signature for any other intent.
   A wallet/runtime metadata-hash mechanism, where supported by the selected
   signer and target runtime, is a useful additional check; it does not by
   itself bind Polkagent's approval, policy revision, recipient, amount, or
   `IntentId`.

## 7. Bulletin, App Chat and decentralized conversation transport

Bulletin Chain documentation describes IPFS-compatible CID-addressed storage,
authorization/allowance concepts, testnet endpoint/gateway, chunking and an
approximately two-week testnet retention period. Retrieval through a gateway is
not automatically trustless; docs recommend direct P2P/Helia for production and
note future light-client retrieval. [Bulletin data storage](https://docs.polkadot.com/reference/polkadot-hub/data-storage/), [store/retrieve guide](https://docs.polkadot.com/chain-interactions/store-data/bulletin-chain/)

**Polkagent use:** attachments or evidence packages can be staged locally,
hashed, classified and optionally uploaded through a specifically configured
storage adapter. They remain user-owned local artifacts first; a CID is a
reference, not a confidentiality guarantee or durable archive.

**Statement Store and Product-host context:** the official documentation now
describes Statement Store as short-lived signed, allowance-gated, best-effort
gossip on People Chain; a Product reaches it through a Host API rather than by
direct node access. Desktop documents per-Product account/topic mediation, and
explicitly leaves delivery acknowledgement, retry, ordering, secondary topics,
channels and TTL as application-level concerns. It also documents the normal
composition: ephemeral signal in Statement Store, durable CID-addressed content
in Bulletin. [Statement Store via Desktop Host API](https://docs.polkadot.com/reference/apps/hosts/polkadot-desktop/statement-store/), [Statement Store reference](https://docs.polkadot.com/reference/apps/infrastructure/statement-store/), [lifecycle](https://docs.polkadot.com/reference/apps/infrastructure/statement-store/lifecycle/), [channels](https://docs.polkadot.com/reference/apps/infrastructure/statement-store/channels/)

This does **not** establish a stable, standalone daemon or external-agent
protocol. Products are host-mediated web applications; it does not specify
PCA's encrypted session envelope, device discovery, bot identity lifecycle,
ACK protocol, HOP ticket format, or a general external signing API.
`polkadot-chat-agents` remains the compatibility evidence for those behaviors.
Therefore:

- Preserve its discovered protocol invariants as compatibility fixtures, not
  assumptions that public docs guarantee.
- Build a Rust `transport-polkadot-chat` only after byte-level key/topic/codec
  vectors, device-session tests and message lifecycle tests exist. Until then,
  a JS bridge is an experimental compatibility option, not a kernel dependency.
- Keep chat registration/personhood/bandwidth/allowance flows out of the core
  runtime and expose them as explicit local operator CLIs.
- Do not store conversation content or sessions on-chain; encrypted transport
  state and durable local session recovery remain critical.

## 8. JAM and PVM services: strategic horizon, not product substrate

**Verified documentation claim:** JAM (Join-Accumulate Machine) is presented as a prospective successor design
for the relay chain. The official Wiki describes permissionless services with
`refine`, `accumulate`, and `onTransfer` entry points; it is transactionless at
the base protocol and intended services hold application functionality. It also
describes PVM/RISC-V, asynchronous service interactions, and an eventual
compatibility path for parachains. [JAM Chain](https://wiki.polkadot.com/learn/learn-jam-chain/), [JAM FAQ](https://wiki.polkadot.com/learn/learn-jam-faq/)

Parity's 2025 recap describes the Graypaper/conformance work, PolkaJAM,
PolkaVM, CoreVM and experimentation infrastructure, while W3F's JAM site is a
prize/conformance initiative including low-resource-node work. These are strong
signals of an active ecosystem, not evidence of a production app-service API.
[Parity JAM roundup](https://www.parity.io/blog/polkadot-roundup-2025), [W3F JAM program](https://jam.web3.foundation/)

| JAM idea | Polkagent interpretation | Status |
|---|---|---|
| Service as permissionless code/state | Future agent-verification/evidence or market primitive | Research; no v1 architecture dependency |
| PVM/RISC-V deterministic execution | Potential portable verifier or policy/receipt program | Research; use native Rust/Wasm tests now |
| Coretime | Potential paid computation allocation | Not a substitute for normal provider billing or local resource limits |
| Asynchronous `onTransfer` / service interaction | A useful conceptual analogue for durable outbox/saga design | Architectural inspiration only |
| Low-resource node/tooling | Potential embedded verifier/light-client path | Monitor; do not commit implementation until APIs/artifacts mature |

**Design hedge:** keep `Artifact`, `Intent`, `Receipt`, `PolicyDecision`,
`ChainClient`, and `Signer` network-agnostic and versioned. If a future JAM
adapter proves useful, it can introduce a `JamServiceIntent`/receipt without
rewriting run orchestration or replacing local evidence storage.

## 9. Build, test, deploy and self-hosting tooling

Polkadot SDK development is Rust-native. The documented path includes a
parachain template, FRAME/pallet work, chain specifications, Cumulus/relay
integration, runtime upgrade/migration practices, and test deployment to Paseo.
[parachain development](https://docs.polkadot.com/develop/parachains/), [deployment to testnet](https://docs.polkadot.com/develop/parachains/deployment/)

| Tool | Use in Polkagent delivery | Caveat |
|---|---|---|
| Zombienet | Ephemeral local relay/parachain test networks; XCM/governance/runtime scenario tests; Kubernetes/Podman/native providers | A test harness, not a Polkagent production dependency. [Docs](https://docs.polkadot.com/develop/toolkit/) |
| Chopsticks | Local fork/replay/storage manipulation/time travel/XCM experiments | Docs warn its Smoldot-based fork lacks Ethereum JSON-RPC; do not use it as EVM integration proof. [Docs](https://docs.polkadot.com/develop/toolkit/parachains/fork-chains/chopsticks/) |
| Pop CLI | Generate/build chains and launch local ephemeral networks via Zombienet SDK | Good developer-experience integration target for future coding harness skills; no runtime dependency. [Docs](https://docs.polkadot.com/reference/tools/pop-cli/) |
| Paseo testnet | Production-runtime-adjacent public test stage | Network metadata and endpoints still change; use named profile + fixtures. |
| Hub EVM testnet | Standard EVM tooling/JSON-RPC test stage | Test account mapping and true eth-RPC endpoint, not only WSS. |

**Self-hosting opportunity:** ship a single Rust binary/compose profile for the
agent runtime, then an optional `polkagent lab` test profile that launches
memory adapters by default and invokes Zombienet/Chopsticks only in developer
integration suites. Do not require an operator of a normal agent to operate a
node, collator, validator, or cloud account.

## Integration API inventory

| Boundary | Candidate API/protocol | Adapter contract | Decision |
|---|---|---|---|
| Chain reads/events/runtime calls | Subxt WebSocket/JSON-RPC + metadata | `ChainClient::query/subscribe/runtime_call` | v1 core adapter |
| Chain intent/sign/submit/watch | Subxt + external signers / wallet protocol | `IntentBuilder`, `Signer`, `Submitter`, `FinalityWatcher` | Phase 1 read + Phase 2 gated submit |
| EVM reads/calls/logs | Hub eth JSON-RPC, ABI/ethers-style provider | `EvmClient` leaf behind `ChainClient` domain mapping | Optional Phase 2 |
| XCM | Runtime calls and target metadata/channel configuration | `CrossChainPlanner` then explicit intent | Research/Phase 2+ |
| Assets/fees | Hub runtime metadata, `pallet_assets`, asset-conversion/fee extension runtime facts | `AssetRegistry` / `FeeQuote` | Read-only Phase 1 |
| OpenGov | runtime storage/events/preimages | `GovernanceReader` / `GovernanceIntentBuilder` | Read Phase 1; submit Phase 3 only |
| Identity | People Chain storage | `IdentityResolver` returning untrusted/qualified claims | Read-only optional Phase 1 |
| Bulletin | PAPI/chain RPC + P2P/Helia or gateway | `ArtifactRemoteStore` | Testnet research only |
| App Chat | encrypted Statement Store/HOP codec from sibling reference | `Transport` adapter + protocol vectors | Research spike |
| JAM | future service/node/light-client APIs | future leaf crate | No scheduled v1 integration |

## Product opportunities ranked

| Priority | Opportunity | Why it fits Polkagent | Gate to start |
|---|---|---|---|
| P0 | Private Polkadot coding/operations agent with read-only chain research | Rust-first, local workspace + Polkadot context; avoids custody | Subxt read adapter + deterministic fixtures + artifact/evidence model |
| P0 | Governance and treasury evidence assistant | High user value without autonomous value movement | Metadata-pinned OpenGov/asset readers, source hash/preimage verifier, disclaimer/policy UX |
| P1 | Intent-review companion for transfers, staking, governance and EVM calls | Clean separation between helpful LLM explanation and human signing | External signer, call decoder, simulation/fee quotes, approval/outbox/finality tests |
| P1 | Polkadot Chat private agent transport | Differentiated UX/outbound-only encrypted conversation | Full compatibility test vectors and registration/session recovery spike |
| P2 | XCM planning/simulation assistant | Genuine ecosystem complexity where a planner adds value | Route/fee/version test matrix across local fork + testnet; strict high-risk policy |
| P2 | Bulletin-backed evidence/artifacts | CID reference can complement local immutable evidence | Retention/allowance/privacy model, P2P integrity and production service confirmation |
| P3 | Personhood-aware access/rate policy | Could improve public agent anti-spam posture | Verified product API/spec, privacy review, non-personhood fallback |
| P3 | PVM/JAM verifiable agent services | Long-term technical differentiation | Production/testnet APIs, toolchain stability, economic model and independent audit |

## Concrete use cases and the required safety boundary

The following are examples of what “Polkadot agents” can mean. They are not all
committed scope. Their purpose is to make the difference clear between helpful
analysis, bounded intent preparation, configured autonomous authority, and
unsafe unbounded or self-authorized authority.

### A. Account and asset research

**User goal:** “Explain this account's balances and whether I can make a
transfer without losing access to the account.”

1. The chain reader obtains balance/asset state for a configured profile and a
   recorded block reference.
2. It resolves canonical decimals and known asset identities from an
   operator-configured, verifiable asset-trust view. This is distinct from the
   permissionless extension marketplace: it selects which asset metadata the
   deployment will trust for safety-sensitive display.
3. The model may summarize the facts, clearly separating chain evidence from
   explanatory inference.
4. No signer, egress-capable tool, or transfer intent is created unless the
   user starts a separate action.

**Why this is valuable:** it delivers immediate utility with no wallet custody
or automated financial effect. **MVP suitability:** high.

### B. Explain Before Sign for a native transfer or asset action

**User goal:** “I have this action/payload. Tell me what it will do before I
sign it.”

The safe flow is the transfer-intent flow above. A native/asset transfer is the
first candidate because its critical facts can be rendered in a compact trusted
card: account, recipient, asset, amount, decimals, fee cap, account survival
conditions, network and expiry.

**MVP suitability:** high only for one selected call class and one profile.
The product must return an explicit unsupported response for any wrapped batch,
proxy, multisig, XCM, contract call or asset it has not been tested to decode.

### C. Governance Scout

**User goal:** “What is referendum N, what can it change, and what evidence
should I read before I decide?”

The agent reads referendum state, origin/track/timing, linked preimage/call and
relevant runtime facts, then produces a cited brief. It may generate a **draft
vote or delegation intent** but must not vote, set delegation or submit a
referendum by default. OpenGov permits simultaneous referenda and uses
track-specific timing/thresholds, so a stale dashboard explanation is not
sufficient. [OpenGov overview](https://docs.polkadot.com/polkadot-protocol/onchain-governance/overview/)

**MVP suitability:** medium. It is a strong second product kit after canonical
action evidence works, but governance data provenance and user research must
show a repeated job worth solving.

### D. XCM planner and cross-chain preflight

**User goal:** “Can asset X move from chain A to account B on chain C, what
will it cost, and what could fail?”

XCM is not an ordinary transfer API. An XCM plan needs source/destination
locations, canonical asset identity, route/channel support, beneficiary,
execution/weight/fee asset, version, refund/trap behavior and a chain-specific
success observation plan. **Verified fact:** XCM is a format that separates
message expression from delivery. [XCM overview](https://docs.polkadot.com/parachains/interoperability/get-started/)

**Proposal:** v1 only explains a supported route or refuses. Real dispatch is a
later high-risk effect requiring local-fork/testnet fixtures and a separate
approval/signing phase.

### E. Contract interaction firewall

**User goal:** “This dApp asks my wallet to call a contract—what is it asking
for?”

For the EVM-compatible Hub surface, Polkagent can eventually parse ABI/calldata,
contract address/code/verified-interface provenance, value, allowance-like
effects, gas/fee constraints and simulation result. A contract call remains
untrusted when ABI/source is absent; calldata may be displayed as opaque bytes
with an explicit refusal to make semantic claims.

**Verified fact:** Hub's REVM path supports standard Ethereum tooling and
Ethereum JSON-RPC; PVM differs in execution/resource behavior and is documented
as preview/limited tooling. [Hub contract overview](https://docs.polkadot.com/smart-contracts/overview), [PVM status](https://docs.polkadot.com/polkadot-protocol/smart-contract-basics/polkavm-design/)

**MVP suitability:** low. It is a good fixture/security test category, not the
initial action class.

### F. Builder/operator copilot

**User goal:** “Help me build, test or inspect a Polkadot SDK project without
giving you my wallet or all of my machine.”

This is the local coding-agent path: a harness gets a private staged directory
and policy-limited workspace/tools; it can use Zombienet, Chopsticks or Pop CLI
in a development profile. **Verified fact:** Zombienet supports ephemeral
Polkadot SDK network testing; Chopsticks can fork/replay chains and simulate
XCM but does not provide Ethereum JSON-RPC in its Smoldot-based fork; Pop CLI
wraps common build/network workflows. [Zombienet](https://docs.polkadot.com/develop/toolkit/), [Chopsticks](https://docs.polkadot.com/develop/toolkit/parachains/fork-chains/chopsticks/), [Pop CLI](https://docs.polkadot.com/reference/tools/pop-cli/)

**MVP suitability:** a later product surface. It shares the runtime's
capability/evidence architecture but has broader filesystem/process risk than a
read-only chain evidence flow.

### G. Private Polkadot chat agent

**User goal:** “Talk to my locally operated agent from a Polkadot-native chat
surface without exposing a public webhook.”

The existing sibling implementation shows this can be valuable: encrypted
store-and-forward messaging, outbound-only polling, persistent sessions and
bridge harnesses. But stable public protocol/API support has not been verified
in this research. **Experimental / unknown:** treat it as a dedicated adapter
spike and compatibility program, never as proof that the first product must be
a chat bot.

## API and library division of responsibility

One common failure mode is to choose a library and allow it to determine the
whole product architecture. Polkagent should instead own stable domain ports and
place ecosystem libraries behind leaf adapters.

| Need | Candidate integration | What it is responsible for | What Polkagent still owns |
|---|---|---|---|
| Rust Substrate/Polkadot reads | `subxt` | RPC, SCALE encoding/decoding, metadata-derived types, block/event subscriptions | profile pinning, allowlists, retries, intent/evidence model, policy and user UX |
| Rust transaction signing primitives | `subxt-signer` where appropriate | Key/signature support for an explicit signer implementation | whether a key exists at all, external signer flow, approval binding, secret isolation |
| TypeScript/web/client validation | PAPI or Dedot | metadata descriptors, light-client/web integrations, wallet/front-end interoperability | Rust core correctness, fixture provenance, no Node dependency in core |
| EVM contract access | Ethereum JSON-RPC + a Rust EVM client library chosen later | calls/logs/transactions against Hub EVM endpoint | contract provenance, ABI policy, simulation/evidence, signing boundary |
| Polkadot SDK chain creation | FRAME/Cumulus/SDK | building a new runtime/parachain | Polkagent does not need this to operate; use for test/developer use cases only |
| Local network tests | Zombienet / Chopsticks / Pop CLI | launch/fork/test chain environments | agent test scenarios, fixture assertions, no production control-plane dependence |
| Bulletin data | PAPI/RPC + CID/P2P/gateway clients | storage submission/retrieval mechanics | artifact classification, retention/privacy, allowance policy, hash/evidence binding |
| Chat | separately proven Statement Store/HOP codec adapter | encrypted message protocol mechanics | run admission, policy, outbox, retry/ordering and harness isolation |

### Library-selection rule

**Proposal:** no chain library's dynamic API may be exposed directly to a model
or plug-in. The adapter translates external types into the small stable domain
types (`AccountRef`, `AssetRef`, `ChainIntent`, `ChainEvidence`,
`SubmissionReceipt`). This makes a runtime metadata update, a library upgrade or
a future JAM adapter a contained integration change rather than an event-schema
or policy rewrite.

## Cross-cutting risks and non-negotiable controls

1. **Runtime drift:** metadata is code-level API. Pin it; test regeneration;
   fail closed on incompatible high-risk calls.
2. **Model output is hostile/untrusted input:** it cannot create an allowed
   extrinsic/tool/network destination; typed policy and allowlists decide.
3. **Signer custody:** no `mnemonic`, root seed, session key, chat key or bridge
   token crosses into prompts/logs/plugins. External signer boundaries remain
   visible even for owner-operated bots.
4. **Finality and truth:** distinguish `drafted`, `approved`, `signed`,
   `submitted`, `included`, `finalized`, `failed`, and `unknown`. Never turn an
   RPC submission response into “completed.”
5. **Cross-chain/contract risk:** simulation can be stale/incomplete; require
   expiry, quoted limits, decoded payload, destination genesis/location, and
   explicit signing.
6. **Chat protocol risk:** one serving process per identity, durable sessions,
   ACK-or-retry, device-channel behavior and replacement-safe outbound queues
   are transport invariants, not optional reliability polish.
7. **Privacy:** People Chain identity, Bulletin CIDs, on-chain remarks/events,
   and third-party RPCs are not appropriate stores for private memory/prompts.
8. **Provider/cloud coupling:** local-first means no required hosted control
   plane. External RPC/provider failures are managed adapters with a durable
   retry/outbox policy, not silent state loss.

## Phased implementation plan and go/no-go gates

The phases below are **proposals** for converting this landscape into a
responsible product sequence. A later phase is not automatically authorized by
completion of an earlier one: it must meet its own technical and user-trust
evidence gate.

### Phase 0 — prove the local safety kernel

Build no real chain action, chat transport, payment rail, browser tool or
plug-in marketplace. Implement typed IDs/events, deterministic turn/effect
transitions, memory + SQLite recovery, `ResolvedGrant`, an in-memory transport
and echo/fake executor.

**Gate P0:** fault injection proves duplicate inbound work and a crash around an
outbox effect cannot produce two semantic visible results. The runtime can
display `drafted`, `approved`, `cancelled`, `failed` and `unknown` accurately.

### Phase 1 — one read-only Polkadot evidence profile

Implement one `subxt` profile for one target testnet or explicitly selected
chain. The scope is read-only: account/asset facts and one supported canonical
call decode/preflight fixture corpus. Create `doctor`, profile drift detection,
redacted `turns show`, and a static approval card.

**Gate P1 (technical):**

- Metadata fixture download/generation is reproducible and identifies genesis,
  metadata hash and runtime version.
- Valid, stale, wrong-network, malformed and unsupported action fixtures have
  deterministic outcomes.
- The card never presents unknown scale/asset/recipient/network data as known.
- Runtime drift invalidates the profile or triggers a reviewed refresh; it does
  not silently reuse a prior approval.

**Gate P1 (product):** target users can inspect a realistic card and correctly
describe its network, action, recipient/affected state, asset/amount/fee and
what remains uncertain.

The validation PRD must pre-register the cohort and threshold before testing.
A concrete starting proposal is at least 12 representative advanced users,
100% correct identification of network, action class, signer, recipient and
deliberately dangerous mismatches, at least 90% correct identification of
amount/asset/fee/uncertainty fields, and zero approvals of seeded
wrong-network or wrong-recipient critical fixtures. These numbers are proposed
decision thresholds, not evidence that the gate has passed.

### Phase 2 — controlled external signer handoff

Add a fake signer first, then one independently controlled external signer
path. No Polkagent-held seed. Supported action remains one narrow call class.

**Gate P2:**

- Approval, canonical payload, signer request and signature are bound to the
  same `IntentId`, genesis hash, metadata hash and expiry.
- A cancellation/expiry/stale worker cannot send against an old approval.
- A real signer UX displays the same critical facts that Polkagent's trusted
  card displays, or the integration is deferred.
- Submission and finality tests distinguish rejected, submitted, included,
  finalized, failure and timeout/unknown outcomes.
- Moderated comprehension tests catch deliberate wrong-network, wrong-recipient
  and fee-breach fixtures before user signoff.

For P2, “catch” means the participant refuses the dangerous request and can
name the conflicting canonical field without moderator prompting. Record
critical misses separately from ordinary usability errors; a critical miss
blocks real-signing release even if average task completion is high.

The provisional P2 cohort is at least 12 representative advanced users, at
least six of whom did not participate in P1, each completing a zero-value or
testnet flow through the selected real signer. Every participant sees at least
three pre-registered critical fixtures and five ordinary tasks. The release
threshold is zero signatures/approvals of a critical fixture, 100% unprompted
identification of its conflicting network/recipient/fee field, at least 90%
ordinary-task completion, and no more than a 10% ordinary critical-field error
rate. Any critical miss stops the real-signing release; it cannot be averaged
away or waived by one owner. After a fix, the affected gate is rerun with a
fresh cohort. Named product, security, and UX owners must all sign the gate
record, and any one may stop release when the evidence is incomplete.

### Phase 3 — durable product surface and bounded action expansion

Add a private local alpha, chain profile monitor, event/evidence export,
approved workspace/harness path or a Governance Scout **read-only** kit. Only
add an action type after its call schema, evidence rules, approval card and
failure/recovery corpus exist.

**Gate P3:** repeated target users use it on a real bounded job; evidence is
opened/understood before action; recovery works across restart; operator cost,
queue and diagnostics are credible. Absence of repeat usage is a signal to
narrow or stop, not to add chat/marketplace features.

The validation plan must replace “repeated” and “credible” with registered
numbers. A provisional private-alpha bar is: at least 8 qualified participants,
at least 5 complete a second real bounded job within four weeks, 100% of
injected restart/outbox recovery drills preserve one logical effect, every
unknown chain outcome remains visibly unknown, and cost/queue/diagnostic data
is complete for at least 95% of runs. These are planning thresholds to test and
revise, not claims of achieved product fit.

### Phase 4 — optional transports, XCM, contracts and artifacts

Each is an independent program:

| Program | Minimum prerequisite | Expansion gate |
|---|---|---|
| Polkadot App Chat/PCA | byte-level interop vectors, device-session/restart/ACK/lane tests, demand evidence | shadow mode interoperates with reference and no message loss/duplicate reply under fault tests |
| XCM | route registry, local fork + testnet proof per route, failure/refund semantics | user can understand route/fee/beneficiary; no unsafe automatic retry |
| EVM contracts | address/ABI provenance, calldata/simulation corpus, wallet handoff | malformed/unknown ABI causes refusal; no raw model-generated call reaches signer |
| Bulletin artifacts | retention/allowance/P2P integrity/privacy model | artifact CIDs never replace local source/evidence; secrets cannot be uploaded |
| Builder harness/tools | sandbox/resource policy, path/network test suite, real job demand | no transport seed/state leak; denied tools cannot be bypassed by prompt or plug-in |

### Separate gated tracks — payments, public agents, autonomy, PVM and JAM

Payments, public agents, permissionless marketplaces, and fully autonomous
accounts are established long-term platform tracks, not ideas that need to be
re-authorized as product scope. Enabling each implementation still requires
its own evidence: settlement/reconciliation, legal/operating-model review,
abuse/fraud response, custody and recovery, independent security review,
measured demand, and pause/revoke/exit plans.

PVM contract tooling can advance when its selected official toolchain,
deployment API, simulation/evidence path, and network profile are
reproducible. JAM remains a more distant experimental track until a stable
service toolchain, network/API, conformance story, and concrete user workflow
exist.

## Newcomer decision tree

```text
Do you need to move money, vote, send XCM, or call a contract?
  └─ yes → start with Explain Before Sign, one profile and external signer
           Do not make autonomous custody the first unproven integration.
           Add configured autonomous signer modes after the action family and
           signer/recovery contracts pass their gates.

Do you need a private assistant for a repository or operation?
  └─ yes → start local, with workspace/tool policy and a harness test kit.
           Add Polkadot chat only if its protocol spike passes.

Do you need a new on-chain application?
  └─ EVM-compatible app now → evaluate Hub REVM path and standard EVM tooling.
  └─ native high-performance contract → research PVM; treat it as preview.
  └─ custom chain/runtime → use Polkadot SDK + Pop/Zombienet/Chopsticks;
                             this is a different product from operating Polkagent.

Do you need a decentralized agent service on JAM?
  └─ not for v1. Track the official JAM/Graypaper/conformance path and keep
     Polkagent's intent/evidence abstractions portable in the meantime.
```

## Deep-research questions / required spikes

### P0: before a native Polkadot Chat transport commitment

1. Obtain authoritative or reproducible byte vectors for App Chat key
derivation, request/session/device topics, encryption envelopes, message IDs,
ACKs, statement replacement, HOP claims and registration proof.
2. Identify current owner/support/upgrade path of Statement Store, identity
backend, People resources, Bulletin/HOP and the Polkadot App client. Is there a
public stability/compatibility contract?
3. Prove Rust interoperability against Node reference and actual app for
opener/follow-up/device sessions, restart, duplicate delivery, rejection,
attachment and send queue behavior.
4. Clarify mainnet vs named testnet onboarding, personhood/attestation,
bandwidth and storage allowance requirements without relying on privileged
faucets or app-only services.

### P0: before any signer/submission feature

1. Define supported call allowlist per genesis+metadata: balances/assets,
OpenGov, proxy/multisig, XCM and EVM are separate policies.
2. Evaluate external signer integrations (wallet extension/mobile/hardware/Vault
or QR) for payload binding, errors, cancellation and user-readable decoding.
3. Capture test vectors for fee/nonce/mortality/metadata updates and finality;
test stale-signature and replay rejection.
4. Define legal/product stance for governance/financial explanations,
recommendations and automated monitoring.

### P1: PVM and Hub contract routes

1. Re-check actual mainnet/testnet PVM contract availability, resolc toolchain,
Rust/RISC-V pipeline, debug/source verification, ABI/log conventions and
cross-VM semantics.
2. Compare EVM eth-RPC output, WSS/runtime events and account mapping to avoid
two inconsistent agent state views.
3. Build a malicious-contract test suite: reentrancy-like effects, approvals,
proxy calls, large return data, misleading metadata, events and simulation
versus execution divergence.

### P1: XCM and assets

1. Establish a supported-route registry with origins, destination locations,
asset canonical identifiers, fee assets, delivery channels, execution limits and
test evidence.
2. Test failure/refund/trap/retry semantics and user-facing recovery narratives;
there must be no “retry automatically” default for value movement.
3. Decide whether any stablecoin or payment workflow requires issuer attestation,
compliance review, or fiat/on/off-ramp partner integrations.

### P2: JAM horizon scanning

1. Track the final Graypaper/audit/conformance milestones and any official
testnet/service SDK rather than extrapolating from prize materials.
2. Determine actual service deployment/economic APIs, execution limits, data
availability, user intent/transaction ingress and migration guarantees.
3. Prototype a tiny deterministic receipt/policy verifier only when the PVM
toolchain and reproducible deployment/test environment are available.

## Source index (accessed 2026-07-29)

- [Polkadot Hub reference](https://docs.polkadot.com/reference/polkadot-hub/)
- [Hub assets](https://docs.polkadot.com/reference/polkadot-hub/assets/)
- [Smart contracts overview](https://docs.polkadot.com/smart-contracts/overview)
- [Hub JSON-RPC APIs](https://docs.polkadot.com/smart-contracts/for-eth-devs/json-rpc-apis/)
- [PVM design/status](https://docs.polkadot.com/polkadot-protocol/smart-contract-basics/polkavm-design/)
- [Subxt Rust API](https://docs.polkadot.com/reference/tools/subxt/)
- [PAPI](https://docs.polkadot.com/reference/tools/papi/)
- [Dedot](https://docs.polkadot.com/reference/tools/dedot/)
- [Polkadot.js API status](https://docs.polkadot.com/reference/tools/polkadot-js-api)
- [XCM overview](https://docs.polkadot.com/parachains/interoperability/get-started/)
- [OpenGov overview](https://docs.polkadot.com/polkadot-protocol/onchain-governance/overview/)
- [People Chain](https://docs.polkadot.com/reference/polkadot-hub/people-and-identity/)
- [Bulletin storage](https://docs.polkadot.com/reference/polkadot-hub/data-storage/)
- [Statement Store reference](https://docs.polkadot.com/reference/apps/infrastructure/statement-store/)
- [Statement Store lifecycle](https://docs.polkadot.com/reference/apps/infrastructure/statement-store/lifecycle/)
- [Statement Store channels](https://docs.polkadot.com/reference/apps/infrastructure/statement-store/channels/)
- [Statement Store through Polkadot Desktop](https://docs.polkadot.com/reference/apps/hosts/polkadot-desktop/statement-store/)
- [Wallet landscape](https://docs.polkadot.com/develop/toolkit/integrations/storage/)
- [Hub account mapping](https://docs.polkadot.com/smart-contracts/connect)
- [Parachain development](https://docs.polkadot.com/develop/parachains/)
- [Zombienet](https://docs.polkadot.com/develop/toolkit/)
- [Chopsticks](https://docs.polkadot.com/develop/toolkit/parachains/fork-chains/chopsticks/)
- [Pop CLI](https://docs.polkadot.com/reference/tools/pop-cli/)
- [JAM Chain Wiki](https://wiki.polkadot.com/learn/learn-jam-chain/)
- [W3F JAM program](https://jam.web3.foundation/)
- [Parity 2025 Polkadot/JAM roundup](https://www.parity.io/blog/polkadot-roundup-2025)
