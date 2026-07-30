# PRD-05: Polkadot Chain, SDK, JAM/PVM and Product-Building Integrations

**Status:** definitive PRD
**Owner:** unassigned
**Last updated:** 2026-07-30
**Audience:** engineers, product designers, and decision-makers who may or may not have prior Polkadot experience

## 0. Purpose and user outcome

This PRD defines how Polkagent connects to the Polkadot ecosystem: which chains, protocols, APIs, tools, and product surfaces it integrates with, and how those integrations are built, tested, promoted, and maintained. It covers the entire integration surface from low-level SCALE encoding through high-level product-engineering workflows, and separates what is buildable now from what requires further research.

A reader who completes this document should understand:

1. What Polkadot is and how its components relate to each other.
2. Which ecosystem surfaces Polkagent integrates with, at what maturity level.
3. How Polkagent's Rust-first architecture connects to chain clients, signers, wallets, and developer tools.
4. How the product-engineering workbench helps builders create, test, deploy, and operate Polkadot products.
5. Where JAM and PVM fit in the long-term strategy and what must be true before they enter the product.

This PRD does not define the agent execution model (PRD-03), provider/harness architecture (PRD-04), PCA compatibility (PRD-06), identity/signer policy (PRD-07), or payment flows (PRD-08). It references those documents at interface boundaries and summarizes enough context for independent reading.

### 0.1 Scope

**In scope:**
- Polkadot ecosystem primer for non-Polkadot readers
- Integration capability matrix for every relevant chain, API, and tool
- Rust vs TypeScript integration decisions
- Chain/network profile model
- Metadata strategy and drift detection
- Signer/wallet compatibility overview (detailed policy in PRD-07)
- Product-engineering workbench design
- Builder JTBDs and workflows (A1-A10)
- Tool integrations (Zombienet, Chopsticks, Pop CLI, contract toolchains)
- Local/testnet/production promotion pipeline
- JAM/PVM research section
- Acceptance criteria and verification checklist

**Out of scope:**
- Agent execution lifecycle (PRD-03)
- Provider/model/harness contracts (PRD-04)
- PCA transport protocol and chat compatibility (PRD-06)
- Signer policy, custody tiers, and key management (PRD-07)
- Payment rails, autonomous accounts, and economic controls (PRD-08)
- Memory, knowledge, and multi-agent coordination (PRD-09)

### 0.2 Labels used in this document

| Label | Meaning |
|---|---|
| **Verified fact** | Directly supported by the linked first-party source as of the access date. |
| **Inference** | A reasoned conclusion from verified facts; must be validated in a spike or user study. |
| **Proposal** | A Polkagent product or architecture choice; requires acceptance in a PRD or ADR. |
| **Experimental** | Preview, evolving, reverse-engineered, or insufficiently documented surface. |
| **Production baseline** | Documented/current route suitable for an adapter after normal security testing. |
| **Supported with qualification** | Usable but requires pinned network/runtime/API and conformance fixtures. |
| **Preview / evolving** | Do not make v1 correctness depend on it. |
| **Research only** | No product commitment until concrete implementation/testnet/API is verified. |

### 0.3 Key terms defined in this document

| Term | Meaning |
|---|---|
| **Chain profile** | A named, versioned, pinned description of a target network including genesis hash, metadata hash, RPC endpoints, call allowlists, asset registry, and signer support. |
| **Metadata** | The runtime's typed self-description of available pallets, storage, events, constants, types, and callable extrinsics. |
| **SCALE** | Simple Concatenated Aggregate Little-Endian encoding, the binary format used by Polkadot SDK runtimes. |
| **Extrinsic** | A signed or otherwise authorized request submitted to a chain, broader than a simple value transfer. |
| **Chain intent** | Polkagent's typed, immutable, expiring proposal for a chain action before signing and submission. |
| **Drift** | A mismatch between a pinned chain profile and the live chain's current runtime metadata. |

---

## 1. Polkadot ecosystem primer

This section is for readers who are not Polkadot developers. It explains the ecosystem components that Polkagent must understand to operate safely.

### 1.1 What is Polkadot

Polkadot is a multi-chain network designed for interoperability, shared security, and application-specific blockchains. Unlike a single monolithic chain, Polkadot allows independent chains to connect to a shared security layer and communicate with each other using a common message format.

The key architectural insight is **shared security without shared state**: each connected chain maintains its own state and logic while inheriting security guarantees from the network's validators. This allows specialized chains optimized for specific use cases (assets, identity, governance, smart contracts) rather than one general-purpose chain that does everything.

For Polkagent, this means there is no single "Polkadot API." Every chain in the ecosystem has its own runtime, its own metadata, its own callable operations, and potentially its own upgrade schedule. An agent that works with Polkadot must understand which chain it is talking to, what that chain's current capabilities are, and how those capabilities might change.

### 1.2 Relay chain, parachains, and system chains

**The relay chain** is Polkadot's central coordination layer. It provides shared security through its validator set and manages the consensus across connected chains. Application developers generally do not interact with the relay chain directly for product logic.

**Parachains** (parallel chains) are independent blockchains that connect to the relay chain for shared security. Each parachain has its own runtime logic, state, and block production. A parachain can be purpose-built for any use case: DeFi, identity, gaming, supply chain, or anything else. Parachains acquire execution time on the relay chain through a mechanism called **coretime**.

**System chains** are parachains operated by the network itself rather than by independent teams. They provide core ecosystem services:

- **Polkadot Hub** (historically known as Asset Hub): the primary application entry point. It hosts assets, smart contracts (EVM and PVM), governance interaction, and cross-chain operations. **Verified fact:** Polkadot Hub documentation describes it as the application layer for the ecosystem. [Source: Polkadot Hub reference, accessed 2026-07-29](https://docs.polkadot.com/reference/polkadot-hub/)

- **People Chain**: hosts on-chain identity records, registrar judgments, and sub-identities. Identity here is an attestation model, not universal authorization. **Verified fact:** [Source: People Chain reference, accessed 2026-07-29](https://docs.polkadot.com/reference/polkadot-hub/people-and-identity/)

- **Bulletin Chain**: a specialized storage surface for CID/IPFS-compatible data with documented testnet retention constraints (~2 weeks on testnet). **Verified fact:** [Source: Bulletin data storage, accessed 2026-07-29](https://docs.polkadot.com/reference/polkadot-hub/data-storage/)

### 1.3 What is Polkadot SDK (formerly Substrate)

Polkadot SDK is the Rust framework and library collection used to build Polkadot-compatible chains, runtimes, nodes, and system logic. It encompasses several major components:

- **FRAME** (Framework for Runtime Aggregation of Modularized Entities): the modular runtime development framework. Developers compose runtimes from reusable modules called **pallets**.

- **Cumulus**: libraries and tools for building parachains that connect to the Polkadot relay chain.

- **Node template and parachain templates**: starter projects for building new chains.

Polkadot SDK is a Rust-native development environment. Building runtimes, pallets, and chains requires Rust toolchains, and the compiled runtime is a WebAssembly (Wasm) blob that executes deterministically on validators.

**Verified fact:** Official documentation covers Rust-native parachain and runtime development, storage migrations, runtime upgrades, Zombienet test networks, and Chopsticks fork/testing workflows. [Source: Parachain development guide, accessed 2026-07-29](https://docs.polkadot.com/develop/parachains/)

### 1.4 Runtimes, pallets, extrinsics, events, and storage

**A runtime** defines a chain's state-transition logic. It determines what the chain can do: which operations are available, how state is stored, what events are emitted, and how fees are calculated. A runtime is compiled to Wasm and can be upgraded on a live chain through governance without a hard fork (a "forkless upgrade").

**Pallets** are modular runtime components. Examples include:
- `pallet_balances`: native token balances and transfers
- `pallet_assets`: fungible asset management
- `pallet_identity`: on-chain identity records
- `pallet_proxy`: delegated account authority
- `pallet_multisig`: multi-signature account operations
- `pallet_referenda`: governance referendum logic
- `pallet_contracts` / `pallet_revive`: smart contract execution (EVM/PVM)

Each pallet exposes:
- **Callable extrinsics** (also called "calls" or "dispatchables"): operations that external users can submit. For example, `balances.transfer_allow_death(dest, value)` moves native tokens.
- **Storage items**: queryable on-chain state such as account balances, asset metadata, or governance referendum details.
- **Events**: notifications emitted when state changes occur. For example, a successful transfer emits a `Transfer` event with sender, recipient, and amount.
- **Constants**: fixed runtime configuration values such as existential deposit amounts.

**An extrinsic** is a signed (or otherwise authorized) request submitted to a chain. It is broader than a simple value transfer: it can be a governance vote, an identity registration, a proxy delegation, a contract call, a batch of operations, or any other runtime-defined action. The term "transaction" is sometimes used informally, but "extrinsic" is the precise Polkadot SDK term.

### 1.5 What is SCALE encoding and metadata

**SCALE** (Simple Concatenated Aggregate Little-Endian) is the binary encoding format used by all Polkadot SDK runtimes. Every extrinsic, storage query, and event is encoded and decoded using SCALE. It is compact, deterministic, and type-aware, but it is not self-describing: you cannot decode SCALE bytes without knowing the types involved.

This is where **metadata** becomes critical. Every Polkadot SDK runtime publishes metadata that describes:
- Every pallet, its index, and its callable extrinsics with full type information
- Every storage item, its key and value types
- Every event and error type
- Constants, their types, and their values
- The runtime's type registry (all types used in calls, storage, events)

**Verified fact:** Subxt's documented workflow downloads metadata and generates a static Rust interface; PAPI similarly generates TypeScript descriptors from metadata. [Source: Subxt docs, accessed 2026-07-29](https://docs.polkadot.com/reference/tools/subxt/)

**Why metadata matters for agents:** An agent cannot safely hard-code "transfer means these bytes" across all networks or forever on one network. A runtime upgrade can change the call shape, types, fees, or the behavior a call invokes. The metadata is the runtime's API contract, and it can change at any block.

### 1.6 What is XCM

**XCM** (Cross-Consensus Messaging) is a standardized message format for expressing instructions between consensus systems. It is not itself a transport protocol, delivery guarantee, fee calculator, or success assurance.

An XCM message describes what should happen (withdraw assets, deposit assets, execute a call, report an error, etc.) using a location-based addressing model. Locations identify chains, accounts, assets, and other addressable entities using a hierarchical path (e.g., "the asset at pallet index 50, asset ID 1984, on the chain two hops away from here").

Key concepts:
- **Multilocation / Location**: an address that can refer to a chain, an account, an asset class, or a contract within the cross-consensus universe.
- **Reserve transfers vs teleports**: two fundamentally different mechanisms for moving assets between chains, with different trust and availability properties.
- **Weight and fees**: XCM execution on the destination chain costs weight; the message must carry or arrange for fee payment on the destination.
- **Version negotiation**: different chains may support different XCM versions; messages must be version-compatible.

**Verified fact:** XCM is documented as a format that separates message expression from delivery. [Source: XCM overview, accessed 2026-07-29](https://docs.polkadot.com/parachains/interoperability/get-started/)

**For Polkagent:** XCM operations are high-risk cross-chain effects. A failed XCM transfer can leave assets trapped on a remote chain. Polkagent must default to read/plan/simulate and require explicit evidence-gated approval before any XCM dispatch.

### 1.7 What are JAM and PVM

**PVM (PolkaVM / Polkadot Virtual Machine)** is a RISC-V-based deterministic execution environment. It appears in two contexts:

1. **Hub PVM contracts**: Polkadot Hub supports PVM as a native high-performance smart contract execution target alongside the EVM-compatible REVM surface. PVM contracts are compiled from Solidity (via `resolc`) or potentially other languages targeting RISC-V. **Verified fact:** The PVM contract documentation explicitly labels this path as early-stage/preview with potentially unstable or incomplete tooling. [Source: PVM design/status, accessed 2026-07-29](https://docs.polkadot.com/polkadot-protocol/smart-contract-basics/polkavm-design/)

2. **JAM services**: PVM is the execution engine for services in the prospective JAM architecture.

**JAM (Join-Accumulate Machine)** is a proposed future evolution of Polkadot's base architecture. It describes a permissionless service model where:
- Services are deployed with `refine`, `accumulate`, and `onTransfer` entry points.
- The base protocol is transactionless; application functionality lives in services.
- Services run PVM code deterministically across validators.

**Verified fact:** The official Wiki describes JAM as a proposed successor design with permissionless services. The Graypaper provides the formal specification. W3F runs a prize/conformance initiative. [Source: JAM Chain, accessed 2026-07-29](https://wiki.polkadot.com/learn/learn-jam-chain/)

**For Polkagent:** JAM is strategically important but too prospective for a v1 dependency. No Polkagent v1 correctness, identity, payment, or workflow claim should depend on a JAM service API. PVM contracts on Hub are a preview surface that requires its own maturity gates. Both are tracked as experimental/research integration targets.

### 1.8 Ecosystem topology summary

```text
Human / organization
  +-- controls wallet, signer, policy, workspace, provider accounts
  +-- talks to Polkagent through CLI/web and later chat transport

Polkagent (off-chain Rust runtime)
  +-- durable run/effect/evidence store
  +-- policy + approval + external signer handoff
  +-- model/harness/tool adapters
  +-- chain adapters: Subxt first; optional EVM and XCM planners
  +-- transport adapters: local first; chat after proof

Polkadot ecosystem
  +-- Polkadot Hub: assets, contracts (EVM+PVM), cross-chain surface
  +-- People Chain: on-chain identity and registrar judgments
  +-- Relay/system/parachains: target-specific runtimes and governance
  +-- Bulletin: optional CID-addressed artifact storage
  +-- Future JAM: prospective services/PVM architecture
```

---

## 2. Integration capability matrix

**Access date:** 2026-07-29. All status assessments must be re-verified against target-network metadata and linked sources before implementation decisions.

### 2.1 Polkadot Hub

| Attribute | Detail |
|---|---|
| **Description** | Primary application entry point: assets, smart contracts, governance interaction, XCM, and ecosystem services |
| **Maturity** | Production baseline |
| **APIs / repos** | Subxt against Hub metadata; Hub EVM JSON-RPC for contract surface; runtime metadata v14/v15 |
| **Primary source** | [Polkadot Hub reference](https://docs.polkadot.com/reference/polkadot-hub/) |
| **Signing model** | Standard Polkadot extrinsic signing (SR25519/ED25519); EVM-compatible signing for EVM surface with account mapping |
| **Permission boundary** | Profile-pinned call allowlist; asset allowlist; fee/amount limits |
| **Test environment** | Paseo testnet (public); Zombienet/Chopsticks local fork |
| **Rate / quota / cost** | RPC endpoint dependent; no protocol-level rate limit but provider-level limits apply; existential deposit rules |
| **License** | Apache 2.0 / GPL 3.0 (Polkadot SDK) |
| **Integration risk** | Runtime upgrades change metadata; asset metadata/ticker is untrusted display data; EVM/PVM dual-VM complicates account identity |
| **Polkagent role** | Read-only research and evidence (Phase 1); intent builder and controlled submission (Phase 2+); EVM adapter (Phase 2+); PVM research spike |

### 2.2 Asset Hub / Assets functionality

| Attribute | Detail |
|---|---|
| **Description** | Native and foreign asset management, asset metadata/roles, delegated transfers, fee payment in supported assets |
| **Maturity** | Production baseline |
| **APIs / repos** | `pallet_assets`, `pallet_foreign_assets`; Subxt storage/call queries; ERC-20 precompile for EVM-mapped assets |
| **Primary source** | [Hub assets](https://docs.polkadot.com/reference/polkadot-hub/assets/), [ERC-20 precompile](https://docs.polkadot.com/smart-contracts/precompiles/erc20/) |
| **Signing model** | Standard extrinsic signing; delegated transfers use asset-specific approval patterns |
| **Permission boundary** | Asset allowlist keyed by genesis hash + runtime version + full location/ID; operator-curated trust view |
| **Test environment** | Paseo testnet; local Zombienet/Chopsticks with asset creation fixtures |
| **Rate / quota / cost** | Existential deposit per asset; creation deposit; sufficient vs non-sufficient assets affect account survival |
| **License** | Apache 2.0 / GPL 3.0 |
| **Integration risk** | Asset impersonation via display metadata; foreign asset location confusion; decimal scale mismatch; ED consequences |
| **Polkagent role** | Portfolio/treasury read model (Phase 1); policy-bound transfer intent (Phase 2); stablecoin/payment workflows (Phase 3+) |

**Research findings:**
- **Canonical stablecoin asset IDs:** USDC is asset ID 1337 and USDT is asset ID 1984 on Asset Hub. These are "sufficient" assets (they can exist in an account without a DOT existential deposit and can establish accounts on their own). **Always verify asset IDs on-chain** against the live `pallet_assets` storage — anyone can register an asset with a display name of "USDC." Look-alike assets are a real attack vector. **Verified fact** (research3.md, Domain 05).
- **Existential deposit:** The existential deposit for accounts holding non-sufficient assets on Asset Hub is 0.1 DOT. Accounts holding only sufficient assets (USDC 1337 or USDT 1984) are not subject to this ED, but any account that also holds non-sufficient assets must maintain the 0.1 DOT minimum to avoid account reaping. **Verified fact** (research3.md, Domain 05).
- **Foreign assets:** USDC and USDT arrive on Asset Hub via Snowbridge (Ethereum bridge) and the Kusama bridge respectively. Their presence as `pallet_foreign_assets` entries means their canonical location path must be resolved via asset registry, not assumed from the display name. **Verified fact** (research3.md, Domain 05).
- **USDC fee-sufficiency is NOT enacted:** Referendum 174 requested that USDC (asset 1337) become fee-sufficient on the relay chain, but as of the research access date this referendum is a proposal, not yet passed. Do not assume USDC can pay transaction fees; budget for DOT fee payment. Use `XcmPaymentApi::query_acceptable_payment_assets` to determine what fees a destination chain will accept. **Verified fact** (research3.md, Domain 05 and Domain 08).

### 2.3 People Chain

| Attribute | Detail |
|---|---|
| **Description** | Specialized identity parachain hosting on-chain identity records, registrar judgments, and sub-identities |
| **Maturity** | Production baseline for read-only signals |
| **APIs / repos** | `pallet_identity` on People Chain; Subxt storage queries |
| **Primary source** | [People Chain reference](https://docs.polkadot.com/reference/polkadot-hub/people-and-identity/), [identity guide](https://wiki.polkadot.com/learn/learn-identity/) |
| **Signing model** | Standard extrinsic signing for identity operations (set identity, request judgment) |
| **Permission boundary** | Read-only in Polkagent; identity data is a contextual signal, never authorization |
| **Test environment** | Paseo People Chain testnet |
| **Rate / quota / cost** | Identity bond (refundable); registrar fees (variable per registrar) |
| **License** | Apache 2.0 / GPL 3.0 |
| **Integration risk** | Over-trust of identity/registrar status; privacy implications of on-chain identity data; stale registrar judgments |
| **Polkagent role** | Optional read-only policy input and display signal; verified-agent/operator profile context |

### 2.4 OpenGov (referenda, tracks, preimages)

| Attribute | Detail |
|---|---|
| **Description** | On-chain governance: concurrent referenda, origin/track system with different thresholds/deposits/timing, conviction voting, track-specific delegation |
| **Maturity** | Production baseline for read/analysis; submission and autonomous voting independently gated |
| **APIs / repos** | `pallet_referenda`, `pallet_conviction_voting`, `pallet_preimage`; Subxt storage/event queries |
| **Primary source** | [OpenGov overview](https://docs.polkadot.com/polkadot-protocol/onchain-governance/overview/), [origins/tracks](https://docs.polkadot.com/polkadot-protocol/onchain-governance/origins-tracks/) |
| **Signing model** | Standard extrinsic signing for votes, delegation, proposals |
| **Permission boundary** | Read-only default; write operations require per-action-family gates; autonomous governance requires explicit mandate |
| **Test environment** | Paseo testnet with governance pallets; Chopsticks time-travel for referendum lifecycle |
| **Rate / quota / cost** | Decision/submission deposits (variable by track); conviction lock periods |
| **License** | Apache 2.0 / GPL 3.0 |
| **Integration risk** | High financial/governance impact; changing track parameters; covert persuasion risk; stale referendum data |
| **Polkagent role** | Governance research copilot (Phase 1, read-only); vote/delegation intent builder (Phase 3+); configured autonomous voting (gated) |

**Research finding — model the full lifecycle explicitly for the treasury-operator workflow:** The OpenGov lifecycle that Polkagent must understand and surface for the treasury/governance operator persona includes: (1) preimage submission (`pallet_preimage::note_preimage`), (2) referendum submission to the appropriate track origin, (3) decision period with conviction-weighted votes (1x–6x lock multiplier), (4) track-level delegation (`pallet_conviction_voting::delegate`) and undelegation, (5) approval/rejection thresholds by track (Small Tipper through Root, each with distinct deposit, min-enactment, and approval curves), and (6) execution after enactment delay. The track/origin system means a proposal's required approval threshold, decision deposit, and timelock vary significantly by track — the agent must resolve these values from live chain storage, not from hardcoded constants. **Verified fact** (research3.md, Domain 05).

### 2.5 Identity and registrars

| Attribute | Detail |
|---|---|
| **Description** | Bonded identity information, registrar judgment attestations, sub-identity linking |
| **Maturity** | Production baseline for read; evolving for personhood/Individuality |
| **APIs / repos** | `pallet_identity` on People Chain |
| **Primary source** | [People Chain reference](https://docs.polkadot.com/reference/polkadot-hub/people-and-identity/) |
| **Signing model** | Standard extrinsic signing |
| **Permission boundary** | Identity is a signal, not authorization; reputation never widens a grant |
| **Test environment** | Paseo People Chain |
| **Rate / quota / cost** | Identity bond; registrar fees |
| **License** | Apache 2.0 / GPL 3.0 |
| **Integration risk** | Registrar judgment is not universal personhood; Individuality/RingVRF is preview |
| **Polkagent role** | Read-only context; optional agent card identity binding; future personhood-gated rate policy (experimental) |

### 2.6 XCM messaging

| Attribute | Detail |
|---|---|
| **Description** | Cross-consensus message format for asset transfers, remote calls, and inter-chain coordination |
| **Maturity** | Supported with qualification; high-risk effect requiring per-route evidence |
| **APIs / repos** | XCM pallet/runtime APIs; `DryRunApi`, `XcmPaymentApi` where supported; Subxt for construction/submission |
| **Primary source** | [XCM overview](https://docs.polkadot.com/parachains/interoperability/get-started/) |
| **Signing model** | Standard extrinsic signing on source chain; execution on destination is protocol-mediated |
| **Permission boundary** | Per-route evidence gates; no autonomous dispatch until route/action passes simulation, custody, recovery, and mandate gates |
| **Test environment** | Chopsticks XCM simulation; Zombienet relay+parachain setups; Paseo cross-chain testnet |
| **Rate / quota / cost** | Weight/fee on both source and destination chains; refund/trap behavior varies |
| **License** | Apache 2.0 / GPL 3.0 |
| **Integration risk** | Location/fee/route/version mistakes; partial failure across chains; trapped assets; stale simulation |
| **Polkagent role** | XCM planner and simulator (Phase 2, read-only); controlled dispatch (Phase 3+, per-route gated) |

**Research findings:**
- **XCM versions in use:** Target XCM v4 and v5. Version negotiation is required; check the `SupportedVersion` storage on both source and destination chains before constructing a message.
- **DryRunApi as mandatory pre-flight:** `DryRunApi::dry_run_call` returns the execution result, emitted events, and any forwarded XCMs. Combined with `XcmPaymentApi::query_delivery_fee`, `query_weight_to_asset_fee`, and `query_acceptable_payment_assets`, this gives a chain-computed "what will happen and what it costs" card before signing. Both APIs are live on Polkadot/Kusama system parachains (Polkadot SDK v1.12.0+; polkadot-fellows runtimes 1.4.0+ added `XcmRecorder` so the `local_xcm` field is populated). No XCM dispatch should proceed without a successful dry-run pass. **Verified fact** (research3.md, Domain 05).
- **Test in Chopsticks before mainnet:** All XCM routes must be validated with Chopsticks before testnet promotion. See Section 9.2 for Chopsticks XCM limitations (no Ethereum JSON-RPC in the fork). **Verified fact** (research3.md, Domain 05).

### 2.7 Smart contracts: EVM (REVM)

| Attribute | Detail |
|---|---|
| **Description** | Ethereum-compatible contract execution on Hub via REVM; standard JSON-RPC, Solidity, Hardhat, Foundry, MetaMask compatibility |
| **Maturity** | Production baseline for EVM adapter |
| **APIs / repos** | Ethereum JSON-RPC; Hub EVM endpoints; standard Solidity/EVM tooling |
| **Primary source** | [Smart contracts overview](https://docs.polkadot.com/smart-contracts/overview), [JSON-RPC APIs](https://docs.polkadot.com/smart-contracts/for-eth-devs/json-rpc-apis/), [dual VM stack](https://docs.polkadot.com/smart-contracts/for-eth-devs/dual-vm-stack) |
| **Signing model** | Ethereum-compatible signing (ECDSA/secp256k1); account mapping required between native and EVM accounts |
| **Permission boundary** | Contract address allowlist; ABI provenance verification; calldata decode before signing |
| **Test environment** | Hub EVM testnet; standard Ethereum dev tools (Hardhat, Foundry) |
| **Rate / quota / cost** | Gas fees; EVM-compatible metering |
| **License** | Apache 2.0 / GPL 3.0 |
| **Integration risk** | Account mapping complexity; non-EVM runtime semantics not visible through JSON-RPC; ABI/source provenance |
| **Polkagent role** | EVM read/ABI decode (Phase 2); contract interaction intent builder (Phase 2+); contract evidence workbench (Phase 3+) |

### 2.8 Smart contracts: PVM (PolkaVM)

| Attribute | Detail |
|---|---|
| **Description** | RISC-V-based native smart contract execution; Solidity via resolc compiler; multi-dimensional metering |
| **Maturity** | Preview / evolving |
| **APIs / repos** | `pallet_revive`; resolc compiler; PVM-specific metering (ref_time, proof_size, storage deposit) |
| **Primary source** | [PVM design/status](https://docs.polkadot.com/polkadot-protocol/smart-contract-basics/polkavm-design/), [EVM vs PVM](https://docs.polkadot.com/smart-contracts/for-eth-devs/evm-vs-pvm/) |
| **Signing model** | Standard Polkadot extrinsic signing |
| **Permission boundary** | Research spike only; no v1 production dependency |
| **Test environment** | Hub PVM testnet (when available); local Zombienet |
| **Rate / quota / cost** | Three-dimensional metering model |
| **License** | Apache 2.0 / GPL 3.0 |
| **Integration risk** | Documentation explicitly marks early-stage/preview; tooling limited; metering model differs from EVM |
| **Polkagent role** | Research spike; contract evidence workbench candidate; future high-performance agent escrow/workflow component |

**Research findings — known PVM/resolc constraints (validate-next gate):**
- **resolc is not compatible with standard solc bytecode.** resolc (the Revive compiler, forked from zksolc/era-compiler) compiles Solidity source to PolkaVM (RISC-V). EVM bytecode produced by standard `solc` cannot be deployed to `pallet_revive`. Use the `--resolc` flag in Hardhat/Foundry polkadot forks or Remix with the resolc backend.
- **Contract bytecode size limits:** Builders (including Montaq Labs) have documented that real contracts frequently exceed PVM bytecode size limits, requiring contract splitting or workarounds. Any PVM contract spike must measure compiled size and plan for splitting.
- **CREATE2 address divergence:** CREATE2 address derivation on PVM uses the PolkaVM code hash, not the EVM init-code hash. The same Solidity source produces a different deployment address on PVM than on Ethereum. Do not assume CREATE2 address parity when migrating EVM contracts.
- **REVM for EVM-bytecode migration:** REVM on Hub provides full EVM-bytecode compatibility for contracts that do not require PVM-specific features. If an EVM contract works as-is under REVM, that is the lower-risk migration path compared to a resolc recompile. Consider REVM (Section 2.7) first, resolc second. **Verified fact** (research3.md, Domain 05).

### 2.9 Wallets (Polkadot.js, Nova, SubWallet, Talisman, etc.)

| Attribute | Detail |
|---|---|
| **Description** | Non-custodial hot wallets (browser extensions, mobile apps) and cold-signing options (Ledger, Polkadot Vault) |
| **Maturity** | Supported with qualification |
| **APIs / repos** | Wallet Connect; browser extension injection APIs; QR signing (Vault); Ledger apps |
| **Primary source** | [Wallet landscape](https://docs.polkadot.com/develop/toolkit/integrations/storage/) |
| **Signing model** | External signer; each wallet has its own supported signing methods and UX |
| **Permission boundary** | Polkagent hands off canonical payload; wallet displays and signs independently |
| **Test environment** | Paseo testnet with wallet integrations |
| **Rate / quota / cost** | No direct protocol cost; wallet-specific UX constraints |
| **License** | Varies by wallet |
| **Integration risk** | Wallet UX may not display all critical intent fields; unsigned metadata risks; varied signing support |
| **Polkagent role** | External signer handoff (Phase 2); wallet-specific adapter testing; mobile consent surface |

### 2.10 Multisig and proxy accounts

| Attribute | Detail |
|---|---|
| **Description** | Multi-signature accounts requiring N-of-M approval; proxy accounts with call-filtered delegated authority; pure proxies for anonymous account creation |
| **Maturity** | Production baseline |
| **APIs / repos** | `pallet_multisig`, `pallet_proxy`; Subxt queries and call construction |
| **Primary source** | [Staking operator proxy](https://docs.polkadot.com/node-infrastructure/run-a-validator/operational-tasks/staking-operator-proxy/) |
| **Signing model** | Multisig: each signer signs independently; Proxy: proxy account signs on behalf of proxied account within call filter |
| **Permission boundary** | Proxy call filters enforce which operations are delegated; multisig threshold enforces quorum |
| **Test environment** | Paseo testnet; Chopsticks state manipulation for edge cases |
| **Rate / quota / cost** | Multisig deposit; proxy creation deposit |
| **License** | Apache 2.0 / GPL 3.0 |
| **Integration risk** | Pure proxy/multisig edge cases; social engineering of approval; proxy semantics misunderstanding |
| **Polkagent role** | Multisig/proxy coordinator (candidate); proxy-limited agent operations; organizational workflow support |

### 2.11 Subxt (Rust client)

| Attribute | Detail |
|---|---|
| **Description** | Official Rust client library for Polkadot SDK chains: metadata-generated types, storage/event queries, transaction construction/signing/submission/watch |
| **Maturity** | Production baseline |
| **APIs / repos** | `subxt` crate, `subxt-signer` crate; [docs.rs/subxt](https://docs.rs/subxt/latest/subxt/) |
| **Primary source** | [Subxt reference](https://docs.polkadot.com/reference/tools/subxt/) |
| **Signing model** | `subxt-signer` provides key/signature types; actual signing is separated to an external signer port |
| **Permission boundary** | Metadata pinning; call allowlists; signer isolation |
| **Test environment** | Offline metadata fixtures; Paseo testnet; local Zombienet |
| **Rate / quota / cost** | RPC endpoint dependent |
| **License** | Apache 2.0 / GPL 3.0 |
| **Integration risk** | Metadata/runtime drift; dynamic call construction from model text; RPC endpoint availability |
| **Polkagent role** | Primary Rust chain adapter (v1 core); read-only research; intent builder; policy-gated submission |

**Research findings:**
- **Version:** Current documented version is subxt v0.44.0. Pin this version in `Cargo.toml` and track the changelog for V16 metadata support (subxt issue #1901) and any breaking changes. **Verified fact** (research3.md, Domain 05).
- **Static + dynamic dual mode is the canonical decode strategy:** Use static codegen (compile-time generated types from pinned metadata) for the hot path on allowlisted calls. Use the dynamic client for runtime-upgrade backfill, explain/inspect paths, and reads against non-allowlisted calls. This gives compile-time type safety where it matters most while preserving flexibility for exploratory queries. **Verified fact** (research3.md, Domain 05).
- **CheckMetadataHash (RFC-0078):** subxt supports the `CheckMetadataHash` signed extension, which embeds a Merkleized hash of the runtime metadata in each signed extrinsic. This is the same hash that Ledger and Polkadot Vault use to verify what they are signing offline. Enabling this extension links the signer's trust to the chain profile's pinned metadata, closing the gap between what Polkagent constructs and what the hardware wallet displays. Refuse to sign if the metadata hash in the extrinsic does not match the profile's pinned hash. **Verified fact** (research3.md, Domain 05).

### 2.12 PAPI (TypeScript client)

| Attribute | Detail |
|---|---|
| **Description** | Polkadot API: light-client-first TypeScript toolkit with metadata-generated types and descriptor compatibility checks |
| **Maturity** | Supported with qualification |
| **APIs / repos** | PAPI npm packages; [papi.io](https://papi.io/) |
| **Primary source** | [PAPI reference](https://docs.polkadot.com/reference/tools/papi/) |
| **Signing model** | TypeScript signing via polkadot-signer packages |
| **Permission boundary** | Browser/frontend integration; no Rust core dependency |
| **Test environment** | Standard Node.js/browser testing |
| **Rate / quota / cost** | Light client or RPC endpoint dependent |
| **License** | Open source (MIT/Apache) |
| **Integration risk** | Avoid Node.js dependency in Rust core; maintenance risk if ecosystem shifts |
| **Polkagent role** | TypeScript companion/bridge tooling; fixture generation; front-end/wallet bridges; cross-checking Rust adapter; not a core dependency |

**Research finding:** The **polkadot.js API** (`@polkadot/api`) is no longer actively developed; it is in maintenance mode only. New TypeScript integrations should use **PAPI** or **Dedot** instead. Polkagent's TypeScript companions (fixture generation, front-end bridges, wallet adapters) should not introduce new polkadot.js dependencies. **Verified fact** (research3.md, Domain 05).

### 2.13 Polkadot App / Product SDK

| Attribute | Detail |
|---|---|
| **Description** | Official Host and Statement Store documentation; Products are host-mediated web applications |
| **Maturity** | Research / fixture-gated |
| **APIs / repos** | Host API (Desktop/mobile); Statement Store; Product messaging architecture |
| **Primary source** | [Host Statement Store interface](https://docs.polkadot.com/reference/apps/hosts/polkadot-desktop/statement-store/), [Statement Store reference](https://docs.polkadot.com/reference/apps/infrastructure/statement-store/), [Product messaging architecture](https://docs.polkadotcommunity.foundation/architecture/messaging/) |
| **Signing model** | Host-mediated; product-specific account/topic model |
| **Permission boundary** | Host controls Product access; Products do not specify PCA encryption, session, ACK, or recovery semantics |
| **Test environment** | Desktop Host development mode |
| **Rate / quota / cost** | Statement Store allowance/authorization model |
| **License** | Varies |
| **Integration risk** | No stable standalone external-agent compatibility contract established; host-mediated model limits direct agent access |
| **Polkagent role** | Research spike for mobile/chat companion; separately gated transport adapter; not core runtime dependency |

### 2.14 Statement Store / encrypted messaging

| Attribute | Detail |
|---|---|
| **Description** | Short-lived signed, allowance-gated, best-effort gossip on People Chain; ephemeral signal layer |
| **Maturity** | Research / fixture-gated |
| **APIs / repos** | Statement Store via Host API; channels/lifecycle documentation |
| **Primary source** | [Statement Store reference](https://docs.polkadot.com/reference/apps/infrastructure/statement-store/), [lifecycle](https://docs.polkadot.com/reference/apps/infrastructure/statement-store/lifecycle/), [channels](https://docs.polkadot.com/reference/apps/infrastructure/statement-store/channels/) |
| **Signing model** | Signed statements with account-based authorization |
| **Permission boundary** | Application-level delivery/ACK/ordering; no guaranteed persistence |
| **Test environment** | Desktop Host development mode |
| **Rate / quota / cost** | Allowance-based statement submission |
| **License** | Varies |
| **Integration risk** | Does not specify PCA's encryption, session, ACK, ordering, or recovery semantics; best-effort delivery |
| **Polkagent role** | Transport adapter research spike (PRD-06 scope); not core dependency; PCA compatibility through separate fixture-gated adapter |

### 2.15 Bulletin storage

| Attribute | Detail |
|---|---|
| **Description** | CID/IPFS-compatible on-chain storage surface; authorization/allowance model; chunking support |
| **Maturity** | Preview / testnet-qualified |
| **APIs / repos** | Chain RPC; P2P/Helia or gateway retrieval |
| **Primary source** | [Bulletin data storage](https://docs.polkadot.com/reference/polkadot-hub/data-storage/), [store/retrieve guide](https://docs.polkadot.com/chain-interactions/store-data/bulletin-chain/) |
| **Signing model** | Standard extrinsic signing for storage submission |
| **Permission boundary** | Artifact classification; no secrets uploaded; local evidence remains authoritative |
| **Test environment** | Bulletin testnet (~2 week retention) |
| **Rate / quota / cost** | Storage deposit/allowance; ~2 week testnet retention; gateway retrieval not automatically trustless |
| **License** | Apache 2.0 / GPL 3.0 |
| **Integration risk** | Limited retention; availability assumptions; gateway trust; allowance/authorization model |
| **Polkagent role** | Optional artifact anchoring/proof experiment; CID reference supplements local evidence; never replaces local storage |

---

## 3. Rust vs TypeScript integration decisions

### 3.1 Decision framework

Polkagent is Rust-first. The question for each integration is not "can we use TypeScript?" but "does TypeScript provide a material advantage that justifies maintaining a separate language boundary?"

| Decision criterion | Rust | TypeScript |
|---|---|---|
| Runtime, domain model, execution engine | Required | Not appropriate |
| Chain client (reads, intents, submission) | Primary (Subxt) | Companion (PAPI for fixture generation and front-end bridges) |
| Policy, security, grant evaluation | Required | Not appropriate |
| CLI, services, storage adapters | Primary | Not appropriate |
| Browser UI and web application | Generated API client | Primary (React/framework) |
| Wallet/front-end interoperability | Generated types/WASM module | Primary (wallet injection APIs) |
| Polkadot Product SDK / Host integration | Not directly applicable | Required (Products are web apps) |
| Mobile chat transport adapter | Preferred (after protocol proof) | Compatibility bridge (interim, from PCA) |
| Contract development tooling | Primary for Polkadot SDK pallets/runtime | Primary for Solidity/EVM tooling |

### 3.2 Concrete decisions

**Rust kernel and adapters:**
- `polkagent-chain-types`: stable domain types (ChainProfileRef, ChainIntent, ChainEvidence, SubmissionReceipt)
- `polkagent-chain`: ChainClient, Signer, Simulation port traits
- `polkagent-chain-subxt`: Subxt-based adapter implementing ChainClient
- `polkagent-chain-evm`: Optional EVM JSON-RPC adapter behind ChainClient domain mapping
- `polkagent-chain-fixtures`: per-network .scale metadata, encoded-call/event test vectors
- `polkagent-chain-policy`: call allowlists, destination allowlists, asset allowlists, fee/cost rules

**TypeScript companions:**
- Browser Agent Studio / Inbox UI
- Wallet bridge components
- PAPI-based fixture generation and cross-verification scripts
- Polkadot Product SDK integration adapter (if/when Product Host API proves viable)

**Language boundary contract:** All cross-language boundaries use versioned JSON/SCALE schemas. No shared mutable state between Rust and TypeScript processes. The Rust runtime is the source of truth for policy, evidence, and effect state.

### 3.3 Library selection rule

No chain library's dynamic API may be exposed directly to a model or plug-in. The adapter translates external types into the small stable domain types (`AccountRef`, `AssetRef`, `ChainIntent`, `ChainEvidence`, `SubmissionReceipt`). This makes a runtime metadata update, a library upgrade, or a future JAM adapter a contained integration change rather than an event-schema or policy rewrite.

---

## 4. Chain/network profile model

### 4.1 What is a chain profile

Every Polkadot-ecosystem network that Polkagent interacts with is represented as a **chain profile**: a named, versioned, pinned description of the target network's identity, capabilities, and Polkagent's configured relationship to it.

A chain profile is not just a network name. It binds a specific runtime version, metadata snapshot, RPC configuration, call allowlists, asset registry, and signer support into a single testable, versionable unit.

### 4.2 Profile schema

```rust
/// A chain profile identifies a specific network and its current
/// verified integration state. All high-risk operations are bound
/// to a profile reference.
struct ChainProfile {
    /// Unique identifier for this profile configuration
    id: ProfileId,
    /// Human-readable name (e.g., "polkadot-hub-mainnet-v1")
    name: String,
    /// Network classification
    network_class: NetworkClass,
    /// Immutable network identity
    genesis_hash: [u8; 32],
    /// Accepted runtime interface snapshot
    metadata_hash: [u8; 32],
    /// Current compatibility facts
    runtime_spec_version: u32,
    transaction_version: u32,
    /// SS58 address prefix for display
    ss58_prefix: u16,
    /// Configured/failover RPC endpoints
    rpc_endpoints: Vec<RpcEndpoint>,
    /// Exact pallet/call combinations permitted for write operations
    call_allowlist: CallAllowlist,
    /// Canonical asset identifiers, decimal scales, sufficiency, ED
    asset_registry: AssetRegistry,
    /// Tested wallet/hardware/proxy signing methods
    signer_support: Vec<SignerCapability>,
    /// Last validated metadata/fixture date
    last_validation: Timestamp,
    /// Fixture corpus reference
    fixture_corpus: CorpusRef,
}

enum NetworkClass {
    /// Live network with real value
    Production,
    /// Public test network (e.g., Paseo)
    Testnet,
    /// Local ephemeral network (Zombienet/Chopsticks)
    Local,
    /// Developer simulation
    Development,
}

struct RpcEndpoint {
    url: String,
    provider: Option<String>,
    priority: u8,
    health_check_interval: Duration,
}
```

### 4.3 Profile lifecycle

1. **Creation**: An operator or product kit creates a profile by downloading metadata from a target network and configuring call/asset/signer allowlists.

2. **Validation**: The profile's fixture corpus is run against the target network. Metadata hash, genesis hash, and spec version are verified. Call allowlists are tested against actual runtime capabilities.

3. **Active use**: All chain operations reference a `ChainProfileRef` (id + genesis hash + metadata hash + spec version). Evidence, intents, and receipts are permanently bound to the profile that produced them.

4. **Drift detection**: Polkagent periodically checks the live chain's metadata against the profile's pinned snapshot. When a mismatch is detected, the profile enters a `Drifted` state. High-risk write operations are blocked until the profile is re-validated.

5. **Re-validation**: After a runtime upgrade, the operator downloads new metadata, updates allowlists as needed, re-runs the fixture corpus, and promotes the profile to a new version.

6. **Archival**: Old profile versions are retained for audit and receipt verification but not used for new operations.

### 4.4 Profile drift behavior

| Drift scenario | Polkagent behavior |
|---|---|
| Metadata hash mismatch (runtime upgrade detected) | Mark profile as `Drifted`; block high-risk writes; allow reads with drift warning; notify operator |
| Spec version change without metadata change | Allow continued operation with drift advisory |
| Genesis hash mismatch | Fatal: this is a different network; refuse all operations |
| RPC endpoint unreachable (all endpoints) | Mark profile as `Unavailable`; block all chain operations; use cached state with staleness warning |
| Call allowlist entry no longer valid in new metadata | Remove entry from effective allowlist; log removal; continue with reduced capabilities |
| Asset registry entry changed (decimals, sufficiency, ED) | Flag affected assets as `RequiresRevalidation`; block amount display for flagged assets |

### 4.5 Default profiles

Polkagent ships with curated profile templates for:
- Polkadot Hub (mainnet)
- Polkadot Hub (Paseo testnet)
- People Chain (mainnet)
- People Chain (Paseo testnet)
- Polkadot relay chain (read-only governance/staking queries)

Each template includes known genesis hashes, recommended RPC endpoints, standard call/asset allowlists, and an initial fixture corpus. Templates must be activated and validated by the operator before use.

---

## 5. Metadata strategy

### 5.1 Why metadata management is critical

Polkadot SDK runtimes are upgradeable. A governance-approved runtime upgrade can change any pallet's call signatures, storage schemas, event types, constants, or fee structures without a hard fork. This means the "API" an agent relies on to decode, construct, and verify chain operations can change at any block.

An agent that ignores this reality risks:
- Constructing invalid extrinsics that fail or produce unintended effects
- Displaying incorrect amounts, assets, or recipients
- Missing new security-relevant parameters
- Approving operations against a stale understanding of the runtime

### 5.2 Metadata handling architecture

```text
Metadata lifecycle:

1. Download     <-- Subxt downloads metadata from RPC or loads from file
                    Records: genesis hash, spec version, metadata hash, block number
2. Pin          <-- Store metadata snapshot as a verified fixture
                    Generate static Rust types for allowlisted calls
3. Validate     <-- Run fixture corpus against pinned metadata
                    Verify known calls, storage, events, constants
4. Use          <-- All chain operations reference pinned metadata
                    Dynamic fallback only for read/explain paths with size limits
5. Watch        <-- Background drift detector compares live vs pinned
                    Alerts operator on spec version or metadata hash change
6. Refresh      <-- Operator downloads new metadata, re-generates types
                    Re-runs fixture corpus, promotes new profile version
```

### 5.3 Static vs dynamic metadata

| Use case | Metadata mode | Safety level |
|---|---|---|
| Known allowlisted calls (transfers, governance) | Static: compile-time generated Rust types from pinned metadata | Highest: type errors are compilation errors |
| Explain/inspect arbitrary extrinsics | Dynamic: runtime metadata lookup with type/size/recursion limits | Medium: schema-validated but not compile-time checked |
| Construct new call types suggested by model | Blocked | N/A: model output is untrusted; cannot dynamically construct write calls |

### 5.4 Metadata versioning

Polkadot SDK supports multiple metadata versions. The current standard is V14 (with V15 adding improved type information). Polkagent should:

1. Target V14 as the minimum supported metadata version.
2. Support V15 features where available for improved type resolution.
3. Record the metadata version alongside the hash in every profile reference.
4. Refuse to operate against a chain that provides metadata below V14.

**Research finding (V16):** V16 metadata support is in progress in subxt (tracked in subxt issue #1901). V16 is not yet required for production, but the static+dynamic dual-mode decode path in subxt is designed to absorb this upgrade without breaking the adapter layer. Monitor the subxt changelog and plan a profile-fixture refresh when V16 becomes available on target networks. **Verified fact** (research3.md, Domain 05).

### 5.5 Drift detection implementation

```rust
/// Drift detection runs as a background task per active profile.
struct DriftDetector {
    profile_id: ProfileId,
    check_interval: Duration,
    last_check: Timestamp,
    last_live_spec: u32,
    last_live_metadata_hash: [u8; 32],
}

enum DriftStatus {
    /// Profile matches live chain
    Current,
    /// Spec version changed; metadata may or may not differ
    SpecVersionDrifted {
        pinned: u32,
        live: u32,
    },
    /// Metadata hash differs; calls/types may have changed
    MetadataDrifted {
        pinned_hash: [u8; 32],
        live_hash: [u8; 32],
        live_spec: u32,
    },
    /// Could not reach any configured RPC endpoint
    Unreachable {
        last_reachable: Timestamp,
    },
}
```

---

## 6. Signer and wallet compatibility

This section summarizes the signer/wallet integration relevant to chain operations. Detailed custody policy, key management tiers, and autonomy levels are defined in PRD-07.

### 6.1 Signer architecture principle

The signer is an isolated component or external device that authorizes an exact canonical payload. It is not an LLM, not a model prompt, and does not receive or expose raw key material to the Polkagent runtime, executor, harness, or any tool.

```text
Polkagent runtime                     External signer
  |                                      |
  | 1. Construct ChainIntent             |
  | 2. Render evidence card              |
  | 3. Apply policy/approval             |
  |                                      |
  | 4. Hand off canonical payload ------->|
  |    (genesis, metadata, call, era,     |
  |     nonce, fee cap, expiry)           |
  |                                      |
  |                     5. Display/sign <-|
  |                        or refuse      |
  |                                      |
  |<------- 6. Return signature ---------|
  |             or refusal               |
  |                                      |
  | 7. Submit, watch, record receipt     |
```

### 6.2 Signer types and compatibility

| Signer type | Description | Polkagent phase | Notes |
|---|---|---|---|
| `WatchOnlySigner` | No signing; intent export only | Phase 1 | First integration; proof of evidence/intent pipeline |
| `FakeSigner` | Deterministic test signatures | Phase 1-2 | Fixture testing only; never on real networks |
| `ExternalWalletSigner` | Browser extension wallets (Talisman, SubWallet, etc.) | Phase 2 | Via wallet injection or WalletConnect; wallet displays and confirms |
| `LedgerSigner` | Hardware wallet signing | Phase 2+ | QR or USB; Ledger Common/Generic App leverages on-chain metadata + RFC-0078 `CheckMetadataHash` to display decoded extrinsics offline and remain runtime-upgrade resilient; requires `CheckMetadataHash` signed extension enabled |
| `VaultSigner` | Air-gapped QR signing via Polkadot Vault | Phase 2+ | Strongest key isolation (air-gapped, QR transport); cold-storage default for high-value agent accounts; UX overhead acceptable for treasury-operator persona |
| `ProxySigner` | Proxy account with call-filtered authority | Phase 3+ | Operator-configured bounds; second enforcement layer |
| `MultisigSigner` | Multi-signature threshold signing | Phase 3+ | Coordination across multiple signers |
| `LocalEncryptedSigner` | Locally encrypted key with passphrase | Phase 3+ | Self-hosted convenience; still isolated from runtime |
| `KmsSigner` | Cloud KMS / HSM integration | Phase 4+ | Managed deployment; key never leaves HSM |
| `MpcSigner` | Multi-party computation signing | Phase 4+ | Distributed key generation and signing |
| `AgentAccountSigner` | Funded agent-owned proxy/limited account | Gated | Autonomous operations; strict policy/budget/revocation |

**Research finding — safest signer combination:** Polkadot Vault (air-gapped QR) and Ledger with the Common/Generic App are the safest signers for high-value agent accounts. Both leverage RFC-0078 `CheckMetadataHash` to verify the exact runtime metadata at signing time, making them resilient to runtime upgrades and to man-in-the-middle attacks on RPC data. These are the "keys outside the model" anchor for treasury operations. Hot paths for low-value bounded-autonomy operations may use policy-bounded hot keys held in `zeroize`-secured memory referenced by opaque handle — never exposed to the model. **Verified fact** (research3.md, Domain 05). See PRD-07 for full custody tier policy.

### 6.3 Wallet compatibility matrix

| Wallet / signer | Native Polkadot (SR25519) | EVM (ECDSA/secp256k1) | Hardware support | Mobile support | Polkagent integration status |
|---|---|---|---|---|---|
| Talisman | Yes | Yes | Ledger | No | Candidate Phase 2 |
| SubWallet | Yes | Yes | Ledger | iOS/Android | Candidate Phase 2 |
| Nova Wallet | Yes | Limited | Ledger | iOS/Android | Candidate Phase 2-3 |
| Polkadot.js Extension | Yes | No | No | No | Maintenance; not recommended for new integration |
| Polkadot Vault | Yes | No | Air-gapped QR | iOS/Android | Candidate Phase 2+ |
| Ledger | Yes | Yes (Ethereum app) | Hardware | Via wallet apps | Candidate Phase 2+ |
| WalletConnect | Protocol bridge | Protocol bridge | Via connected wallet | Via connected wallet | Research |

### 6.4 Account mapping

Polkadot Hub's dual-VM architecture requires mapping between native 32-byte Substrate accounts and 20-byte Ethereum-compatible accounts when operating on the EVM surface.

**Verified fact:** Hub documentation describes account mapping for connecting native accounts to the EVM layer. [Source: Hub account mapping, accessed 2026-07-29](https://docs.polkadot.com/smart-contracts/connect)

Polkagent must:
- Track which account format is relevant for each operation
- Display both native and mapped addresses when operating on the EVM surface
- Never assume a native address and an EVM address refer to the same account without verified mapping

---

## 7. Product-engineering workbench

### 7.1 Purpose

The product-engineering workbench is Polkagent's "Build" pillar: a set of tools, workflows, and product kits that help Polkadot developers create, test, deploy, monitor, and upgrade their products.

The workbench does not replace existing tools. It wraps them as typed adapters and product kits, adds agent-assisted analysis and evidence collection, and provides policy-bounded automation. A developer who already uses Zombienet, Chopsticks, or Pop CLI should find that Polkagent makes their workflow faster and more observable, not different.

### 7.2 Design principles

1. **Wrap, do not replace.** Existing Polkadot SDK tools are mature and trusted. Polkagent adapts them through typed tool interfaces rather than reimplementing their functionality.

2. **Evidence over trust.** Every workbench operation produces an attributable artifact. Agent prose is not proof that code compiled, a test passed, a deployment occurred, or chain state changed.

3. **Policy-bounded effects.** Workspace, filesystem, network, and chain operations are subject to the same capability/grant model as all other Polkagent effects. A coding harness gets a scoped directory, not root access.

4. **Reviewable proposals.** The workbench proposes changes (file edits, config modifications, deployment parameters) as reviewable artifacts. Destructive or irreversible operations require explicit approval.

5. **Product kits as composition.** Repetitive workflows are packaged as versioned product kits: bundles of skills, tool configurations, fixtures, policies, and UX for a specific developer outcome.

### 7.3 Workbench capabilities

| Capability | Description | Dependencies | Phase |
|---|---|---|---|
| **Scaffold** | Generate project structure from templates (runtime, pallet, contract, application) | Pop CLI adapter; project templates | Phase 2 |
| **Edit** | Agent-assisted code editing within a policy-bounded workspace | Coding harness (PRD-04); filesystem tool grants | Phase 2 |
| **Build** | Compile Rust/Solidity/ink! projects with toolchain detection | Local toolchain; resource limits | Phase 2 |
| **Test** | Run unit tests, integration tests, property tests with evidence capture | Project test framework; artifact model | Phase 2 |
| **Simulate** | Fork/replay chain state for migration, upgrade, or XCM testing | Chopsticks adapter; Zombienet adapter | Phase 2-3 |
| **Benchmark** | Run Polkadot SDK benchmarks with output analysis | FRAME benchmarking framework; isolated environment | Phase 3 |
| **Deploy** | Prepare and execute deployment to testnet or production | Deployment tool adapter; approval gates; signer handoff | Phase 3+ |
| **Monitor** | Watch deployed runtime/contract/service for events, errors, drift | Chain subscription adapter; alerting | Phase 3+ |
| **Upgrade** | Prepare runtime upgrade artifacts with migration analysis | Metadata diff; migration rehearsal; governance coordination | Phase 3+ |

### 7.4 Product kits per workflow

| Product kit | Target developer | Included capabilities | Key tools |
|---|---|---|---|
| **Runtime Development Kit** | Pallet/runtime engineers | Scaffold, edit, build, test, benchmark, simulate migration, prepare upgrade | Polkadot SDK templates, Zombienet, Chopsticks, FRAME benchmarking |
| **Contract Development Kit** | Smart contract developers (EVM/ink!/PVM) | Scaffold, edit, build, test, deploy to testnet, verify, monitor | Hardhat/Foundry (EVM), cargo-contract (ink!), resolc (PVM) |
| **Parachain Launch Kit** | Teams launching a new parachain | Scaffold, configure chain spec, test with Zombienet, deploy to testnet, coretime guidance | Pop CLI, Zombienet, chain spec templates |
| **XCM Integration Kit** | Cross-chain developers | Plan routes, simulate transfers, test with multi-chain Zombienet/Chopsticks | Chopsticks XCM simulation, Zombienet relay+parachain |
| **Governance Operations Kit** | Governance participants and proposal authors | Research referenda, analyze proposals, draft votes/delegations, track outcomes | OpenGov reader, preimage analyzer |
| **Application Development Kit** | dApp developers | Scaffold frontend, connect wallet, interact with Hub, test against testnet | PAPI/wallet libraries, Hub APIs, Paseo |

---

## 8. Builder JTBDs and workflows (A1-A10)

### 8.1 A1: Storage-migration rehearsal

**User:** Runtime engineer preparing a storage migration for a production upgrade.

**Situation:** The engineer has written a migration function and needs to verify it against realistic chain state before proposing a governance referendum.

**Workflow:**
1. Engineer specifies a migration module and selects a chain profile for fork source.
2. Polkagent launches Chopsticks with a fork of the target chain at a specified block.
3. The migration is applied to the forked state.
4. Polkagent runs pre/post storage consistency checks and produces a diff artifact.
5. Results include: migrated storage keys, value changes, new/removed keys, migration weight, and any panics or errors.
6. The diff artifact is retained with fork block hash, metadata version, and migration code reference.

**Value:** Turns a risky, expert-only review into inspectable evidence.

**Dependencies / risk:** Fork fidelity, resource isolation, no production credentials.

**Acceptance evidence:** Reproducible fixture detects seeded bad migration and never alters live state.

**Status:** Primitive-ready.

**Required tool integrations:**
- Chopsticks adapter: fork at block, apply runtime upgrade, compare state
- Filesystem tool: access migration code in workspace
- Artifact model: structured diff with provenance

### 8.2 A2: Upgrade impact brief

**User:** Runtime engineer or technical governance participant assessing a proposed runtime upgrade.

**Situation:** A new runtime Wasm blob has been proposed. The engineer needs to understand what changed before voting or deploying.

**Workflow:**
1. Engineer provides or selects old and new metadata/runtime artifacts.
2. Polkagent performs a structural diff: new/removed/changed pallets, calls, storage items, events, constants, and types.
3. The agent generates a plain-language brief highlighting: API-breaking changes, migration requirements, new capabilities, removed features, and affected downstream clients.
4. Test results from the project's CI (if available) are incorporated as evidence.

**Value:** Makes upgrade drift legible to non-experts and governance voters.

**Dependencies / risk:** Metadata alone cannot prove semantic compatibility; the brief is an analysis, not a safety guarantee.

**Acceptance evidence:** Known changed/unchanged fixtures produce correct, attributed reports.

**Status:** Primitive-ready.

**Required tool integrations:**
- Metadata download and comparison utility
- Structured diff artifact generator
- Optional: CI/test result ingestion

### 8.3 A3: XCM planner

**User:** Developer or user planning a cross-chain asset operation.

**Situation:** The user wants to move an asset from one chain to another and needs to understand route, fees, risks, and failure modes before committing.

**Workflow:**
1. User selects source chain, destination chain, asset, amount, and beneficiary.
2. Polkagent resolves chain profiles for both source and destination.
3. Agent queries available XCM routes, versions, reserve/teleport configuration.
4. Where available, `DryRunApi` and `XcmPaymentApi` provide fee and weight estimates.
5. Agent produces a plan artifact showing: route, asset identity on both chains, fee requirements, weight estimates, refund/trap behavior, known failure modes, and version compatibility.
6. The plan explicitly refuses unsupported routes rather than guessing.

**Value:** Explains route/fee uncertainty before value moves.

**Dependencies / risk:** Target-specific XCM versions, fees, reserve rules, partial failure across chains.

**Acceptance evidence:** Local + testnet proof per supported route; wrong asset/location/fee fixtures refuse.

**Status:** Prototype only.

**Required tool integrations:**
- Subxt chain queries on both source and destination
- XCM route/version resolution
- DryRunApi/XcmPaymentApi where available
- Chopsticks/Zombienet XCM simulation

### 8.4 A4: Zombienet test orchestrator

**User:** Development team running cross-chain integration tests.

**Situation:** The team has a checked-in Zombienet scenario specification and wants automated orchestration of ephemeral test networks with agent-assisted failure analysis.

**Workflow:**
1. Team supplies a Zombienet scenario file in their repository.
2. Polkagent validates the scenario specification and checks host prerequisites (Docker/Podman/native provider availability).
3. Agent starts an ephemeral relay+parachain network per the scenario.
4. Scenario tests are executed and results are captured as structured artifacts.
5. On failure, agent analyzes logs and proposes (but does not apply) a patch or investigation path.
6. Network is torn down after test completion.

**Value:** Faster cross-chain regression investigation.

**Dependencies / risk:** Host/container quotas; untrusted repository code must be sandboxed.

**Acceptance evidence:** Scenario is reproducible; denied host/network access is demonstrably blocked.

**Status:** Primitive-ready.

**Required tool integrations:**
- Zombienet CLI adapter (start, wait, test, teardown)
- Log analysis tool
- Host prerequisite checker (Docker, Podman)
- Resource isolation policy

### 8.5 A5: Contract evidence workbench

**User:** Smart contract developer preparing a release candidate.

**Situation:** The developer has a contract (Solidity/ink!/PVM) and needs verifiable evidence of build, analysis, and test results for the release.

**Workflow:**
1. Developer specifies contract project and target toolchain.
2. Polkagent detects project type and available tooling.
3. Steps run explicitly in sequence: build, static analysis, unit tests, property/fuzz tests, deployment to local testnet, integration tests.
4. Each step produces a versioned artifact with tool version, inputs, outputs, and pass/fail status.
5. Unsupported toolchains are surfaced with an explicit message rather than guessed.

**Value:** Evidence is retained alongside the release candidate.

**Dependencies / risk:** PVM/contract toolchain maturity varies; toolchain detection must be conservative.

**Acceptance evidence:** Deliberately vulnerable fixture is caught; unsupported toolchain is surfaced, not guessed.

**Status:** Experimental (tied to toolchain maturity).

**Required tool integrations:**
- Solidity build tools (Hardhat, Foundry)
- ink! build tools (cargo-contract)
- PVM build tools (resolc) -- when stable
- Static analysis tools where available
- Local testnet deployment (Zombienet/Chopsticks)

### 8.6 A6: Explain an extrinsic

**User:** Developer, auditor, or advanced user inspecting an opaque chain operation.

**Situation:** The user has SCALE-encoded extrinsic bytes or a call from a governance proposal and wants to understand exactly what it does.

**Workflow:**
1. User provides encoded extrinsic bytes, a call data hex, or a referendum reference.
2. User specifies (or Polkagent detects) the target chain profile.
3. Polkagent decodes the call against the profile's pinned metadata.
4. The decoded result shows: pallet, call name, every argument with its type and value, nested calls (for batch/proxy/multisig), and any embedded XCM.
5. Where arguments reference on-chain entities (accounts, assets, proposals), additional context is looked up.
6. The agent provides a plain-language explanation alongside the canonical decoded view.
7. Uncertainty is explicit: unknown assets, unresolvable accounts, or unfamiliar call patterns are labeled.

**Value:** Builder/auditor can inspect opaque calls with chain-grounded evidence.

**Dependencies / risk:** Exact metadata/genesis binding; no model-only decoding; nested/complex calls may exceed decode coverage.

**Acceptance evidence:** Valid, stale, wrong-network, malformed fixtures yield distinct outcomes.

**Status:** Primitive-ready.

**Required tool integrations:**
- Subxt dynamic metadata decoder
- Profile-pinned metadata
- On-chain entity resolver (account, asset, proposal lookups)

### 8.7 A7: Metadata-drift watcher

**User:** Library/tool maintainer or team with a client that depends on specific chain metadata.

**Situation:** When a target chain undergoes a runtime upgrade, the team's client code may need updates. They want to be notified and have a draft update prepared automatically.

**Workflow:**
1. Team configures a metadata-drift watch on a chain profile with a linked repository.
2. Polkagent periodically checks the live chain's metadata against the profile's pinned version.
3. On drift detection: downloads new metadata, generates a metadata diff, identifies affected code/types in the repository.
4. Agent creates a proposal branch with regenerated types, updated fixtures, and a draft PR description.
5. The proposal waits for human review; it is never merged or deployed automatically.

**Value:** Reduces client breakage after runtime upgrades.

**Dependencies / risk:** Repository write scope (branch creation); false positives on benign metadata changes.

**Acceptance evidence:** Simulated upgrade triggers one attributed proposal, never a direct merge/deploy.

**Status:** Candidate.

**Required tool integrations:**
- Drift detector (from Section 5.5)
- Metadata diff tool
- Repository/worktree tool (branch creation, file writes)
- PR/proposal artifact generator

### 8.8 A8: Chain-spec and coretime coach

**User:** New team planning to launch a parachain.

**Situation:** The team needs guidance on chain specification configuration, coretime acquisition, and deployment prerequisites.

**Workflow:**
1. Team describes their parachain's purpose and requirements.
2. Polkagent provides a guided, read-only planning session covering: chain spec parameters, runtime configuration, collator requirements, coretime options, relay chain registration, and testnet deployment steps.
3. The agent identifies unknown inputs and prompts for decisions (consensus mechanism, token economics, governance model).
4. Output is a planning artifact with decisions made, remaining unknowns, and next steps.
5. No purchasing, registration, or deployment is performed.

**Value:** Helps new teams understand a complex path without costly mistakes.

**Dependencies / risk:** Costs/market parameters are volatile; coretime pricing is market-driven; no purchase path in initial release.

**Acceptance evidence:** A current profile-backed plan identifies unknown inputs; no purchase path in initial release.

**Status:** Candidate.

**Required tool integrations:**
- Chain spec documentation/template reference
- Coretime pricing reader (read-only)
- Pop CLI guidance

### 8.9 A9: Metadata-grounded RAG

**User:** Developer asking questions about a specific chain's capabilities.

**Situation:** The developer wants to know "Does this chain support asset X?" or "What events does pallet Y emit?" and needs an answer grounded in actual chain metadata, not the model's training data.

**Workflow:**
1. Developer asks a chain-specific question and selects a chain profile.
2. Polkagent retrieves the chain's pinned metadata and relevant documentation sources.
3. The answer is generated with citations: metadata hash, block/profile reference, and source revision.
4. Stale metadata answers are explicitly rejected or flagged with a drift warning.

**Value:** Fewer stale API hallucinations; answers are attributable to specific chain state.

**Dependencies / risk:** Retrieval quality; confidential repository context; metadata staleness.

**Acceptance evidence:** Evaluation corpus measures citation coverage and rejects stale metadata answers.

**Status:** Primitive-ready.

**Required tool integrations:**
- Metadata query interface (pallet/call/storage/event/type lookup)
- Documentation source retrieval
- Citation model (metadata hash, block reference, source revision)

### 8.10 A10: Benchmark and weights copilot

**User:** Runtime engineer running FRAME benchmarks to determine extrinsic weights.

**Situation:** The engineer has added or modified a pallet and needs to run benchmarks, understand the output, and integrate weight information into the runtime.

**Workflow:**
1. Engineer specifies the pallet and benchmark configuration.
2. Polkagent validates the benchmark environment (hardware, binary, chain spec).
3. Benchmarks are run through the standard FRAME benchmarking framework.
4. Agent analyzes output: weight function parameters, linear/quadratic components, R-squared values, outliers.
5. Results are presented as a structured artifact with environment metadata and validity assessment.
6. Invalid or unreliable benchmark setups are explicitly flagged.

**Value:** Makes performance review easier to audit.

**Dependencies / risk:** Benchmark environment validity (results are hardware-dependent); misleading summaries of weight functions.

**Acceptance evidence:** Fixture distinguishes valid from invalid benchmark setup.

**Status:** Candidate.

**Required tool integrations:**
- FRAME benchmarking CLI adapter
- Benchmark output parser
- Environment validator (hardware, binary, configuration checks)

---

## 9. Tool integrations

### 9.1 Zombienet

**What it is:** A testing framework for spinning up ephemeral Polkadot SDK networks (relay chains + parachains) for integration testing. Supports Kubernetes, Podman, and native provider backends.

**Verified fact:** Zombienet is documented as the recommended tool for ephemeral multi-chain test networks. [Source: Zombienet docs, accessed 2026-07-29](https://docs.polkadot.com/develop/toolkit/)

**Polkagent integration:**

```rust
trait ZombienetAdapter {
    /// Validate a scenario specification
    fn validate_scenario(&self, spec: &ScenarioSpec) -> Result<ValidationReport>;

    /// Launch an ephemeral network from a scenario
    fn launch(&self, spec: &ScenarioSpec, config: &LaunchConfig) -> Result<NetworkHandle>;

    /// Run tests against a running network
    fn run_tests(&self, handle: &NetworkHandle) -> Result<TestReport>;

    /// Tear down the network and clean up resources
    fn teardown(&self, handle: NetworkHandle) -> Result<()>;

    /// Check host prerequisites (Docker, Podman, binaries)
    fn check_prerequisites(&self) -> Result<PrerequisiteReport>;
}
```

**Use cases in Polkagent:**
- A4: Zombienet test orchestrator
- Integration testing for chain adapters
- XCM route testing (multi-chain scenarios)
- Runtime upgrade migration testing
- Product kit end-to-end tests

**Not a Polkagent dependency:** Zombienet is a test tool. It is not required for normal agent operation. An operator who only uses Polkagent for chat/research never needs Zombienet installed.

### 9.2 Chopsticks

**What it is:** A local chain fork/replay tool that creates an in-memory copy of a chain's state at a specific block, allowing state manipulation, time travel, and XCM simulation without affecting the real network.

**Verified fact:** Chopsticks can fork/replay chains and simulate XCM but does not provide Ethereum JSON-RPC in its Smoldot-based fork. Current documentation references Chopsticks v1.3.1. [Source: Chopsticks docs, accessed 2026-07-29](https://docs.polkadot.com/develop/toolkit/parachains/fork-chains/chopsticks/)

**Polkagent integration:**

```rust
trait ChopsticksAdapter {
    /// Create a fork of a chain at a specific block
    fn fork(&self, config: &ForkConfig) -> Result<ForkHandle>;

    /// Apply a runtime upgrade to the fork
    fn apply_upgrade(&self, handle: &ForkHandle, wasm: &[u8]) -> Result<UpgradeReport>;

    /// Manipulate storage in the fork
    fn set_storage(&self, handle: &ForkHandle, changes: &[StorageChange]) -> Result<()>;

    /// Advance the fork by N blocks
    fn advance_blocks(&self, handle: &ForkHandle, count: u32) -> Result<Vec<BlockReport>>;

    /// Simulate XCM between forked chains
    fn simulate_xcm(
        &self,
        source: &ForkHandle,
        dest: &ForkHandle,
        message: &XcmMessage,
    ) -> Result<XcmSimulationReport>;

    /// Compare storage state before and after an operation
    fn diff_storage(
        &self,
        handle: &ForkHandle,
        before: BlockHash,
        after: BlockHash,
    ) -> Result<StorageDiff>;
}
```

**Use cases in Polkagent:**
- A1: Storage migration rehearsal
- A3: XCM planner (simulation component)
- Pre-sign simulation (fee estimation, effect preview)
- Runtime upgrade impact analysis

**Limitation:** Chopsticks does not support EVM JSON-RPC. Do not use it as proof of EVM integration.

### 9.3 Pop CLI

**What it is:** A developer-experience CLI that wraps common Polkadot SDK workflows: generating chains/contracts from templates, building, and launching local test networks via Zombienet SDK.

**Verified fact:** Pop CLI is documented as a developer tool for common build/network workflows. [Source: Pop CLI docs, accessed 2026-07-29](https://docs.polkadot.com/reference/tools/pop-cli/)

**Polkagent integration:**

```rust
trait PopCliAdapter {
    /// Generate a new project from a template
    fn generate(
        &self,
        template: &TemplateSpec,
        output_dir: &Path,
    ) -> Result<GenerationReport>;

    /// Build a project
    fn build(&self, project_dir: &Path, config: &BuildConfig) -> Result<BuildReport>;

    /// Launch a local network
    fn launch_network(
        &self,
        config: &NetworkConfig,
    ) -> Result<NetworkHandle>;
}
```

**Use cases in Polkagent:**
- Scaffold workflow in product-engineering workbench
- Project template selection and generation
- Developer onboarding and guidance
- Local network launch for testing

### 9.4 Contract toolchains

| Toolchain | Target | Polkagent adapter | Phase |
|---|---|---|---|
| **Hardhat** | EVM/Solidity on Hub REVM | Build, test, deploy adapter | Phase 2+ |
| **Foundry** | EVM/Solidity on Hub REVM | Build, test, deploy, fuzz adapter | Phase 2+ |
| **cargo-contract** | ink! contracts | Build, test, deploy adapter | Phase 3+ (when ink! maturity allows) |
| **resolc** | Solidity to PVM | Build adapter | Research (PVM is preview); validate bytecode size limits and CREATE2 address divergence before testnet promotion |

Each toolchain adapter follows the same pattern: detect project type, validate toolchain installation, execute build/test/deploy steps, capture output as structured artifacts, and report results with tool versions and environment metadata.

---

## 10. Local/testnet/production promotion pipeline

### 10.1 Pipeline overview

Every Polkagent chain integration follows a promotion pipeline from local development through testnet validation to production operation. Each stage has explicit entry criteria, activities, and exit gates.

```text
+-------------+     +------------+     +-------------+
|    Local     | --> |  Testnet   | --> | Production  |
| Development  |     | Validation |     | Operation   |
+-------------+     +------------+     +-------------+
    |                     |                   |
    | Zombienet/          | Paseo/            | Mainnet/
    | Chopsticks/         | testnet           | production
    | in-memory           | endpoints         | endpoints
    |                     |                   |
    | Fake signers,       | Test accounts,    | Real signers,
    | fixture data        | zero-value ops    | real value
    |                     |                   |
    | No real chain       | Real chain,       | Real chain,
    | effects             | no real value     | real value
```

### 10.2 Stage definitions

#### Local development

**Environment:** Zombienet ephemeral networks, Chopsticks forks, in-memory chain simulators, local Node for JS tooling.

**Activities:**
- Develop and test chain adapters against pinned metadata fixtures
- Run unit tests and integration tests with deterministic outcomes
- Validate call encoding/decoding, event parsing, storage queries
- Test error handling for malformed, unsupported, and edge-case inputs
- Simulate drift detection and profile refresh workflows

**Entry criteria:** None (developer workstation).

**Exit gate:** All fixture corpus tests pass; known edge cases have deterministic outcomes; no flaky tests.

#### Testnet validation

**Environment:** Paseo testnet, Hub EVM testnet, People Chain testnet, Bulletin testnet.

**Activities:**
- Validate chain profile against live testnet metadata
- Execute read-only queries against real chain state
- Test intent construction, fee estimation, and simulation against real endpoints
- Test external signer handoff with test accounts and zero-value operations
- Validate XCM routes against real cross-chain testnet topology
- Run comprehension tests with real evidence cards

**Entry criteria:** All local fixture tests pass; chain profile matches testnet metadata; test accounts funded.

**Exit gate:** Profile validation passes; read/write operations produce expected results; signer handoff works end-to-end; comprehension tests meet registered thresholds.

#### Production operation

**Environment:** Polkadot mainnet, Hub mainnet, People Chain mainnet.

**Activities:**
- Activate production chain profiles with mainnet genesis hash and metadata
- Enable read-only operations first
- Promote to write operations per action-family gates
- Monitor for drift, errors, and anomalies
- Maintain incident response procedures

**Entry criteria:** Testnet validation complete; security review for write operations; operator approval; all family-specific gates pass.

**Exit gate:** Continuous: drift detection active; error rates within SLO; incident response tested.

### 10.3 Action-family promotion gates

Each class of chain operation (transfer, governance vote, XCM message, contract call, etc.) has its own promotion gate independent of other families. A transfer adapter reaching production does not automatically promote governance voting.

| Gate requirement | Description |
|---|---|
| **Fixture corpus** | Deterministic test vectors covering success, failure, edge cases, and adversarial inputs |
| **Call schema** | Documented pallet/call/argument structure with metadata binding |
| **Evidence rules** | What canonical evidence the action card must display |
| **Approval card** | Trusted UI rendering of critical fields from typed data |
| **Failure/recovery corpus** | Tests for rejected, failed, timed-out, and unknown-outcome scenarios |
| **Signer compatibility** | Tested with at least one external signer on testnet |
| **Comprehension test** | Users correctly identify critical fields and reject dangerous fixtures |
| **Security review** | For write operations: independent review of call construction, signing, and submission |

---

## 11. JAM/PVM section: experimental and research integrations

### 11.1 Current maturity assessment

**Access date:** 2026-07-29.

| Surface | Status | Evidence |
|---|---|---|
| JAM specification (Graypaper) | Gray Paper v0.7.x (v0.7.0 June 2025, v0.7.1 July 2025); formal specification exists | [Graypaper](https://graypaper.com/) |
| JAM conformance / prize program | Active; Milestone 1 (correct block import) is current Fellowship evaluation stage | [W3F JAM program](https://jam.web3.foundation/) |
| JAM implementations (PolkaJAM, etc.) | Multiple implementations in progress | [Parity 2025 roundup](https://www.parity.io/blog/polkadot-roundup-2025) |
| JAM testnet / production network | No production deployment; core developers cite 12–20 month delivery horizon | Research only |
| JAM service SDK / developer tools | Emerging; not stable | Research only |
| PVM (PolkaVM) implementation | Active development | [PolkaVM repo](https://github.com/nickelate/polkavm) |
| PVM contracts on Hub | Early-stage preview | [PVM design/status](https://docs.polkadot.com/polkadot-protocol/smart-contract-basics/polkavm-design/) |
| PVM contract tooling (resolc, etc.) | Limited; evolving | Preview |
| CoreVM / low-resource nodes | Experimental | Research only |

### 11.2 What is buildable now vs later

**Buildable now (as research/prototype):**
- PVM contract compilation from Solidity via resolc (with preview caveats — see Section 2.8 for bytecode size limits and CREATE2 divergence)
- PVM contract deployment to local testnet
- Basic PVM contract interaction through standard pallet_revive calls
- Study of JAM Graypaper for architectural inspiration (durable outbox, saga patterns)

**Buildable when tooling stabilizes (validate-next gate):**
- PVM contract development kit (A5 variant), including resolc deploy to testnet and interaction via pallet_revive
- OpenGov preimage submission and lifecycle management workflow (A: governance operations)
- PVM-specific metering analysis (three-dimensional model)
- PVM contract evidence workbench

**Buildable only after production APIs exist (defer):**
- JAM service deployment and management
- JAM service-based agent verification
- JAM service-based payment/escrow
- CoreVM-based embedded agent verification

**JAM/CorePlay horizon (research3.md):** JAM Gray Paper is at v0.7.x; Milestone 1 (correct block import) is the current Fellowship evaluation stage. Core developers cite a 12–20 month delivery horizon for JAM mainnet and 12–24 months for CorePlay. There is no production service model to build against and no stable service ABI. JAM inter-service messaging is still asynchronous-only. **Trigger to revisit:** public multi-client testnet stability plus a stable service ABI. Until that trigger fires, PVM appears in Polkagent only via `pallet_revive`/resolc smart contracts as described above. **Verified fact** (research3.md, Domain 05).

#### Staged recommendations (research synthesis)

| Stage | Actions |
|---|---|
| **do-now** | subxt static+dynamic path with CheckMetadataHash; DryRunApi+XcmPaymentApi pre-flight for all XCM; Chopsticks XCM simulation before any testnet dispatch; Vault and Ledger signer adapters |
| **validate-next** | resolc contract deploy to local testnet (with bytecode-size and CREATE2 validation fixtures); OpenGov preimage submission flow end-to-end |
| **defer** | JAM services; CorePlay; USDC fee-sufficiency assumption (pending referendum 174) |
| **avoid** | Signing any extrinsic without metadata-hash check; hardcoding gas or weight constants across runtime versions; treating polkadot.js API as actively maintained for new integrations |

### 11.3 JAM/PVM candidate opportunities (D1-D4)

#### D1: Long-running service actors

**Concept:** A JAM service could hold resumable workflow state, enabling decentralized long-running agent processes.

**Current status:** Research only. No stable service API, deployment toolchain, or production network exists.

**Kill criteria:** If JAM service APIs remain unstable or the cost/performance model is prohibitive for agent workloads after 12 months from first testnet, downgrade to monitoring-only.

**Validation gate:** Stable testable service path with reproducible deploy/invoke/state-query cycle.

#### D2: Accord-like agreements

**Concept:** Future agents could enter enforceable interaction agreements mediated by JAM services.

**Current status:** Research only. No specification or product API for cross-service agreements exists.

**Kill criteria:** If no concrete specification emerges within 18 months or if the trust model requires unacceptable centralization, abandon in favor of off-chain agreement protocols.

**Validation gate:** Formal model, adversarial test suite, and demonstrated advantage over off-chain alternatives.

#### D3: Refine/accumulate work receipts

**Concept:** JAM's refine/accumulate model could provide verifiable work receipts for agent computations.

**Current status:** Research only. The economic model, proof system, and cost structure are unknown.

**Kill criteria:** If the cost per receipt exceeds practical thresholds or if off-chain evidence provides equivalent assurance, abandon on-chain receipt path.

**Validation gate:** Off-chain evidence prototype first; demonstrated value of on-chain receipt over local evidence; cost model acceptable.

#### D4: PVM escrow/workflow components

**Concept:** A PVM contract could enforce bounded market workflows (escrow, milestone releases, dispute resolution).

**Current status:** Experimental. PVM contracts are preview; contract audit and network readiness are prerequisites.

**Kill criteria:** If PVM contract tooling does not reach production stability within 12 months or if off-chain workflow enforcement provides equivalent safety, defer to off-chain approach.

**Validation gate:** Reproducible build/deploy/simulation plus independent security audit of the contract.

### 11.4 Architectural hedge

Keep core types (`Artifact`, `Intent`, `Receipt`, `PolicyDecision`, `ChainClient`, `Signer`) network-agnostic and versioned. If a future JAM adapter proves useful, it can introduce `JamServiceIntent`, `JamServiceReceipt`, and a `JamChainClient` without rewriting run orchestration or replacing the local evidence store.

```rust
// The ChainClient trait is network-agnostic
trait ChainClient {
    type Intent: ChainIntentLike;
    type Receipt: ChainReceiptLike;
    type Evidence: ChainEvidenceLike;

    fn query(&self, query: ChainQuery) -> Result<Self::Evidence>;
    fn construct_intent(&self, params: IntentParams) -> Result<Self::Intent>;
    fn submit(&self, signed: SignedPayload) -> Result<SubmissionHandle>;
    fn watch_finality(&self, handle: SubmissionHandle) -> Result<Self::Receipt>;
}

// Future JAM adapter implements the same trait family
// struct JamChainClient implements ChainClient<
//     Intent = JamServiceIntent,
//     Receipt = JamServiceReceipt,
//     Evidence = JamServiceEvidence,
// >
```

### 11.5 JAM/PVM integration principles

1. **No v1 dependency.** Core Polkagent correctness, identity, payment, or workflow claims never depend on JAM or PVM availability.

2. **Leaf adapter only.** JAM/PVM integrations are leaf crates that implement existing chain adapter traits. They do not introduce new kernel concepts.

3. **Explicit maturity labeling.** Any JAM/PVM feature in the product UI is marked with its maturity level. Users see "Experimental" or "Research" labels.

4. **Independent gating.** JAM/PVM integrations have their own promotion gates independent of stable chain integrations. A stable Subxt adapter does not validate a JAM adapter.

5. **Graceful absence.** The platform functions fully without JAM/PVM crates compiled in. They are optional cargo features, not default dependencies.

---

## 12. Acceptance criteria and verification checklist

### 12.1 Integration acceptance criteria

Each integration listed in this PRD must meet the following criteria before entering its declared phase:

#### Phase 1 (read-only) criteria

| ID | Criterion | Verification method |
|---|---|---|
| INT-P1-01 | Chain profile creation, validation, and drift detection work for at least one testnet | Automated test: create profile, validate against Paseo, simulate drift |
| INT-P1-02 | Metadata download, pinning, and hash verification produce deterministic results | Fixture test: same metadata produces same hash across runs |
| INT-P1-03 | At least one Subxt-based read operation (balance query) returns correctly formatted, profile-bound evidence | Integration test against Paseo |
| INT-P1-04 | Drift detection correctly identifies spec version and metadata hash changes | Simulation test: inject metadata change, verify drift alert |
| INT-P1-05 | Extrinsic decoding (A6) correctly decodes valid, malformed, wrong-network, and unsupported call bytes | Fixture corpus with known expected outcomes |
| INT-P1-06 | OpenGov reader returns referendum state with block/hash evidence | Integration test against Paseo governance |
| INT-P1-07 | Identity reader returns People Chain data with qualified uncertainty labels | Integration test against Paseo People Chain |
| INT-P1-08 | Asset registry queries return canonical decimals, sufficiency, and ED with profile binding | Integration test against Paseo Asset Hub |

#### Phase 2 (controlled write) criteria

| ID | Criterion | Verification method |
|---|---|---|
| INT-P2-01 | ChainIntent construction produces correct SCALE-encoded call bytes verified against metadata | Round-trip test: construct, encode, decode, compare |
| INT-P2-02 | External signer handoff delivers exact canonical payload with genesis/metadata/intent binding | Integration test with at least one wallet on testnet |
| INT-P2-03 | Submission and finality watching correctly distinguish all ChainActionStatus states | Fault injection: rejected, included, finalized, failed, timeout, unknown |
| INT-P2-04 | Approval, payload, signer request, and signature are bound to the same IntentId | Correlation test: mismatched IntentId causes rejection |
| INT-P2-05 | Expired or cancelled intents cannot produce a signed submission | Timing test: expired intent rejected by signer handoff |
| INT-P2-06 | Evidence card displays correct canonical fields for supported action classes | Comprehension test: pre-registered cohort, registered thresholds |
| INT-P2-07 | EVM adapter correctly constructs and decodes Hub EVM calls with account mapping | Integration test against Hub EVM testnet |

#### Phase 3+ (expanded capabilities) criteria

| ID | Criterion | Verification method |
|---|---|---|
| INT-P3-01 | XCM planner correctly identifies supported/unsupported routes | Route test corpus with known supported and unsupported configurations |
| INT-P3-02 | Migration rehearsal (A1) detects seeded bad migration without altering live state | Chopsticks fork test with intentionally failing migration |
| INT-P3-03 | Metadata-drift watcher (A7) creates proposal without direct merge/deploy | End-to-end test: simulate upgrade, verify branch creation, verify no auto-merge |
| INT-P3-04 | Product kits install, validate, and provide declared capabilities | Kit conformance test per product kit |
| INT-P3-05 | Zombienet orchestrator (A4) starts, tests, and tears down networks reliably | Automated scenario test with resource cleanup verification |

### 12.2 Cross-cutting verification requirements

| ID | Requirement | Verification |
|---|---|---|
| INT-CC-01 | No chain library API is exposed directly to models or plugins | Architecture review: all chain operations go through domain type adapters |
| INT-CC-02 | Model output never becomes `call_scale` without canonical metadata decoding, policy evaluation, and explicit signer handoff | Adversarial test: model suggests call bytes, verify they are rejected without proper pipeline |
| INT-CC-03 | All chain evidence includes genesis hash, metadata hash, and block reference | Field presence validation in all evidence artifacts |
| INT-CC-04 | Profile drift blocks high-risk write operations | Drift injection test: verify write rejection with drift active |
| INT-CC-05 | Asset display never presents unknown decimals, ED, or sufficiency as known | Fixture test: unknown/drifted asset data shows explicit uncertainty |
| INT-CC-06 | Signer port never receives or returns raw key material to/from the runtime | Architecture review and integration test: verify key isolation |
| INT-CC-07 | All tool adapters (Zombienet, Chopsticks, Pop CLI, contract toolchains) follow capability/grant model | Permission test: denied tool access is enforced |
| INT-CC-08 | Local and testnet fixtures are not accidentally used against production networks | Genesis hash verification in all operations |
| INT-CC-09 | TypeScript companions use versioned JSON/SCALE schemas; no shared mutable state with Rust runtime | Schema compatibility test |
| INT-CC-10 | JAM/PVM features compile as optional cargo features and the platform functions fully without them | Build test: compile without JAM/PVM features, run core test suite |

### 12.3 Ongoing verification

| Activity | Frequency | Owner |
|---|---|---|
| Metadata drift check against all active profiles | Every 15 minutes per profile | Automated |
| Fixture corpus re-run against testnet | Daily | CI |
| Profile re-validation after detected drift | On drift detection | Operator + CI |
| Comprehension test (for new action families) | Before each family promotion | Product team |
| Security review (for write operations) | Before production write enablement | Security team |
| Tool adapter version compatibility check | Weekly | CI |
| XCM route re-verification | After any participating chain upgrade | CI + operator |

---

## 13. Requirements index

### 13.1 Functional requirements

| ID | Requirement | Section | Phase |
|---|---|---|---|
| PINT-F01 | Polkagent must support named, versioned chain profiles with pinned metadata | 4 | 1 |
| PINT-F02 | Chain profiles must include genesis hash, metadata hash, spec version, RPC endpoints, call allowlists, and asset registries | 4.2 | 1 |
| PINT-F03 | Drift detection must identify spec version and metadata hash changes and block high-risk writes | 4.4, 5.5 | 1 |
| PINT-F04 | Subxt-based chain adapter must support read operations (balance, asset, governance, identity queries) with profile binding | 2.11 | 1 |
| PINT-F05 | Extrinsic decoding must work against pinned metadata with explicit uncertainty for unsupported calls | 8.6 | 1 |
| PINT-F06 | ChainIntent construction must produce correct SCALE-encoded call bytes bound to a profile reference | 4, 10.2 | 2 |
| PINT-F07 | External signer handoff must deliver canonical payload with genesis/metadata/intent binding | 6.1, 6.2 | 2 |
| PINT-F08 | Submission and finality watching must distinguish drafted, authorized, signed, submitted, included, finalized, failed, and unknown states | 10.2 | 2 |
| PINT-F09 | EVM adapter must support Hub REVM read/call operations with account mapping | 2.7 | 2 |
| PINT-F10 | XCM planner must resolve routes, fees, and versions with explicit refusal for unsupported routes | 2.6, 8.3 | 2-3 |
| PINT-F11 | Product-engineering workbench must support scaffold, edit, build, test, simulate, deploy, monitor, and upgrade workflows | 7 | 2-3 |
| PINT-F12 | Builder workflows A1-A10 must produce attributable artifacts with provenance | 8 | 2-3 |
| PINT-F13 | Tool adapters (Zombienet, Chopsticks, Pop CLI, contract toolchains) must follow capability/grant model | 9 | 2-3 |
| PINT-F14 | Action-family promotion must follow independent per-family gates | 10.3 | All |

### 13.2 Non-functional requirements

| ID | Requirement | Section | Phase |
|---|---|---|---|
| PINT-N01 | All chain operations must reference a ChainProfileRef with genesis hash and metadata hash | 4.2 | 1 |
| PINT-N02 | No chain library API may be exposed directly to models or plugins | 3.3 | 1 |
| PINT-N03 | Model output must never become call_scale without canonical decoding, policy, and signer handoff | 3.3, 12.2 | 2 |
| PINT-N04 | Signer port must never expose raw key material to the runtime, executor, harness, or tools | 6.1 | 2 |
| PINT-N05 | Asset display must never present unknown decimals, ED, or sufficiency as known facts | 2.2, 12.2 | 1 |
| PINT-N06 | JAM/PVM features must be optional cargo features; platform functions fully without them | 11.5 | All |
| PINT-N07 | All evidence artifacts must include genesis hash, metadata hash, and block reference | 12.2 | 1 |
| PINT-N08 | TypeScript companions must use versioned schemas with no shared mutable state with Rust runtime | 3.2 | All |
| PINT-N09 | Chain profile drift detection must run at least every 15 minutes per active profile | 12.3 | 1 |
| PINT-N10 | Fixture corpus must be re-run daily against testnet | 12.3 | 1 |

---

## 14. Cross-document interfaces

### 14.1 Interfaces to PRD-03 (Execution model)

- Chain operations are effects within the run/effect lifecycle. `ChainIntent` becomes an `EffectIntent`; `ChainActionStatus` maps to `EffectAttempt` and `EffectOutcome` states.
- Signing, submission, and finality watching are separate effects with their own attempt/outcome records.
- Tool adapters (Zombienet, Chopsticks, etc.) are typed tool effects within the workspace grant model.

### 14.2 Interfaces to PRD-04 (Providers, harnesses, tools)

- Chain adapters are a tool category. The `ChainClient` is exposed to harnesses through a typed tool interface, not as raw RPC access.
- Coding harnesses interact with the chain through the same policy-bounded tool grants as any other tool.
- Product kits compose tool grants, skills, and chain profile references.

### 14.3 Interfaces to PRD-06 (PCA compatibility)

- Statement Store and Product Host API are transport adapters, not chain integration surfaces.
- Chat transport binds to the same chain profile model for any chain-related operations initiated through chat.

### 14.4 Interfaces to PRD-07 (Identity, signers, policy)

- Signer types and wallet compatibility defined here are the chain-facing view. PRD-07 defines the full custody tier model, key management policies, and autonomy levels.
- People Chain identity integration is read-only in this PRD; PRD-07 defines how identity signals feed into policy decisions.

### 14.5 Interfaces to PRD-08 (Payments)

- Asset Hub assets integration defined here provides the chain adapter for payment flows defined in PRD-08.
- Fee estimation, existential deposit handling, and asset identity resolution are chain adapter responsibilities.
- Payment intents use the same `ChainIntent` and submission pipeline defined here.

---

## 15. Appendix: primary source inventory

All sources accessed 2026-07-29 unless otherwise noted.

| Source | URL | Used for |
|---|---|---|
| Polkadot Hub reference | https://docs.polkadot.com/reference/polkadot-hub/ | Hub capabilities, assets, contracts, governance |
| Hub assets | https://docs.polkadot.com/reference/polkadot-hub/assets/ | Asset management, foreign assets, fee payment |
| Smart contracts overview | https://docs.polkadot.com/smart-contracts/overview | EVM/PVM dual VM, contract deployment |
| JSON-RPC APIs | https://docs.polkadot.com/smart-contracts/for-eth-devs/json-rpc-apis/ | Hub EVM endpoint compatibility |
| PVM design/status | https://docs.polkadot.com/polkadot-protocol/smart-contract-basics/polkavm-design/ | PVM maturity, preview status |
| EVM vs PVM | https://docs.polkadot.com/smart-contracts/for-eth-devs/evm-vs-pvm/ | Dual VM differences, metering model |
| ERC-20 precompile | https://docs.polkadot.com/smart-contracts/precompiles/erc20/ | Asset/ERC-20 mapping |
| Parachain development | https://docs.polkadot.com/develop/parachains/ | Polkadot SDK, FRAME, runtime development |
| Subxt reference | https://docs.polkadot.com/reference/tools/subxt/ | Rust chain client |
| PAPI reference | https://docs.polkadot.com/reference/tools/papi/ | TypeScript client |
| Polkadot.js API status | https://docs.polkadot.com/reference/tools/polkadot-js-api | Maintenance-only status |
| Dedot reference | https://docs.polkadot.com/reference/tools/dedot/ | Alternative TypeScript client |
| Sidecar reference | https://docs.polkadot.com/reference/tools/sidecar/ | REST API facade |
| XCM overview | https://docs.polkadot.com/parachains/interoperability/get-started/ | Cross-chain messaging |
| OpenGov overview | https://docs.polkadot.com/polkadot-protocol/onchain-governance/overview/ | Governance system |
| Origins/tracks | https://docs.polkadot.com/polkadot-protocol/onchain-governance/origins-tracks/ | Governance tracks |
| People Chain reference | https://docs.polkadot.com/reference/polkadot-hub/people-and-identity/ | Identity, registrars |
| Identity guide | https://wiki.polkadot.com/learn/learn-identity/ | Identity system details |
| Bulletin data storage | https://docs.polkadot.com/reference/polkadot-hub/data-storage/ | CID storage surface |
| Store/retrieve guide | https://docs.polkadot.com/chain-interactions/store-data/bulletin-chain/ | Bulletin usage |
| Statement Store via Desktop Host | https://docs.polkadot.com/reference/apps/hosts/polkadot-desktop/statement-store/ | Host API for Statement Store |
| Statement Store reference | https://docs.polkadot.com/reference/apps/infrastructure/statement-store/ | Statement Store architecture |
| Statement Store lifecycle | https://docs.polkadot.com/reference/apps/infrastructure/statement-store/lifecycle/ | Statement lifecycle |
| Statement Store channels | https://docs.polkadot.com/reference/apps/infrastructure/statement-store/channels/ | Channel model |
| Product messaging architecture | https://docs.polkadotcommunity.foundation/architecture/messaging/ | Product messaging |
| Wallet landscape | https://docs.polkadot.com/develop/toolkit/integrations/storage/ | Wallet options |
| Hub account mapping | https://docs.polkadot.com/smart-contracts/connect | EVM account mapping |
| Staking operator proxy | https://docs.polkadot.com/node-infrastructure/run-a-validator/operational-tasks/staking-operator-proxy/ | Proxy security pattern |
| JAM Chain | https://wiki.polkadot.com/learn/learn-jam-chain/ | JAM architecture |
| JAM FAQ | https://wiki.polkadot.com/learn/learn-jam-faq/ | JAM details |
| W3F JAM program | https://jam.web3.foundation/ | JAM conformance/prize |
| Parity 2025 roundup | https://www.parity.io/blog/polkadot-roundup-2025 | Ecosystem status |
| Zombienet docs | https://docs.polkadot.com/develop/toolkit/ | Test network tool |
| Chopsticks docs | https://docs.polkadot.com/develop/toolkit/parachains/fork-chains/chopsticks/ | Fork/replay tool |
| Pop CLI docs | https://docs.polkadot.com/reference/tools/pop-cli/ | Developer CLI |
| Alternative fee tutorial | https://docs.polkadot.com/tutorials/polkadot-sdk/system-chains/asset-hub/send-a-tx-paying-fee-different-token/ | Fee payment in non-native assets |
| Deployment to testnet | https://docs.polkadot.com/develop/parachains/deployment/ | Testnet deployment |

---

*End of PRD-05: Polkadot Chain, SDK, JAM/PVM and Product-Building Integrations*

---

## APPENDIX A: CHAIN INTEGRATION IMPLEMENTATION BLUEPRINT

### A.1 Metadata Strategy Implementation

#### Download and caching system

Metadata is fetched once from the target chain at profile creation time and stored as an immutable fixture. The cache key is `(genesis_hash, spec_version, metadata_version)`. All subsequent operations load from the fixture; live fetches only occur during drift detection polling and explicit operator-initiated refreshes.

```rust
/// Metadata cache entry. Stored on disk as `{genesis_hex}_{spec_version}_v{meta_version}.scale`.
#[derive(Debug, Clone)]
pub struct MetadataEntry {
    /// 32-byte genesis hash that uniquely identifies the network.
    pub genesis_hash: [u8; 32],
    /// Runtime spec version at time of download.
    pub spec_version: u32,
    /// Metadata encoding version (14 or 15; 16 when available).
    pub metadata_version: u8,
    /// Raw SCALE-encoded metadata bytes.
    pub raw_bytes: Vec<u8>,
    /// Blake2-256 hash of `raw_bytes`, used for drift comparison.
    pub metadata_hash: [u8; 32],
    /// Block number at which metadata was fetched.
    pub fetched_at_block: u64,
    /// Wall-clock timestamp of the fetch.
    pub fetched_at: Timestamp,
}

/// Metadata cache: a content-addressed store keyed by genesis + spec + meta version.
pub struct MetadataCache {
    store_dir: PathBuf,
    entries: HashMap<MetadataCacheKey, MetadataEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MetadataCacheKey {
    pub genesis_hash: [u8; 32],
    pub spec_version: u32,
    pub metadata_version: u8,
}

impl MetadataCache {
    /// Load all cached entries from disk on startup.
    pub fn load(store_dir: PathBuf) -> Result<Self> { /* ... */ }

    /// Fetch metadata from a live endpoint and store it. Returns the new entry.
    pub async fn fetch_and_cache(
        &mut self,
        endpoint: &str,
        genesis_hash: [u8; 32],
    ) -> Result<MetadataEntry> { /* ... */ }

    /// Retrieve a pinned entry by key. Returns None if not cached.
    pub fn get(&self, key: &MetadataCacheKey) -> Option<&MetadataEntry> { /* ... */ }

    /// Persist a new entry to the store directory.
    fn persist(&self, entry: &MetadataEntry) -> Result<()> { /* ... */ }
}
```

The cache is stored in the operator's Polkagent data directory under `metadata_cache/`. Each entry is written as a single SCALE file with a sidecar JSON manifest. This layout makes entries portable as fixtures across test environments.

#### Metadata drift detection algorithm

Drift detection compares the live chain's metadata hash against the profile's pinned hash. The algorithm runs as a background task per active profile.

```rust
/// Full drift detection algorithm for one profile check cycle.
pub async fn check_drift(
    profile: &ChainProfile,
    client: &dyn ChainClient,
) -> Result<DriftStatus> {
    // Step 1: Query live runtime version from any healthy endpoint.
    let live_version = client.runtime_version().await?;

    // Step 2: Fast path — if spec_version matches, the metadata is almost
    // certainly unchanged. Skip the heavier metadata hash check.
    if live_version.spec_version == profile.runtime_spec_version
        && live_version.transaction_version == profile.transaction_version
    {
        return Ok(DriftStatus::Current);
    }

    // Step 3: Spec version changed. Fetch the live metadata and compute its hash.
    let live_metadata_bytes = client.metadata_bytes().await?;
    let live_hash = blake2_256(&live_metadata_bytes);

    // Step 4: Hash comparison.
    if live_hash == profile.metadata_hash {
        // Spec bumped but metadata bytes are identical (rare but valid).
        return Ok(DriftStatus::SpecVersionDrifted {
            pinned: profile.runtime_spec_version,
            live: live_version.spec_version,
        });
    }

    // Step 5: Full drift — both spec version and metadata hash differ.
    // Perform field-level diff for the operator notification.
    let pinned_meta = profile.load_pinned_metadata()?;
    let diff = metadata_field_diff(&pinned_meta, &live_metadata_bytes)?;

    Ok(DriftStatus::MetadataDrifted {
        pinned_hash: profile.metadata_hash,
        live_hash,
        live_spec: live_version.spec_version,
        diff,
    })
}

/// Field-level diff between two raw metadata blobs.
/// Returns structured information about added/removed/changed pallets,
/// calls, storage items, events, and types.
pub fn metadata_field_diff(
    pinned: &[u8],
    live: &[u8],
) -> Result<MetadataDiff> {
    let pinned_decoded = decode_metadata(pinned)?;
    let live_decoded = decode_metadata(live)?;

    MetadataDiff {
        added_pallets: /* ... */,
        removed_pallets: /* ... */,
        changed_calls: /* ... */,     // signature or index changes
        changed_storage: /* ... */,   // key/value type changes
        changed_events: /* ... */,    // type changes
        changed_constants: /* ... */, // value changes
        type_registry_diff: /* ... */, // added/removed/renamed types
    }
}
```

#### Automatic chain profile update workflow

When drift is detected, the operator is notified via the Polkagent event system. The update workflow is:

1. Polkagent emits a `ProfileDrifted` event with the diff summary.
2. The operator reviews the diff (via CLI or TUI metadata browser — see Appendix F).
3. Operator invokes `polkagent chain update-profile <profile-id>`.
4. Polkagent downloads new metadata, regenerates static types for the call allowlist, and re-runs the fixture corpus.
5. If all fixtures pass, the profile is promoted to a new version with an incremented `profile_version`.
6. The old version is archived. All existing receipts and evidence retain their original `ChainProfileRef`.

The update workflow is never automatic. An operator action is always required to promote a drifted profile. High-risk write operations remain blocked until the promotion completes.

#### Metadata version pinning and compatibility checking

```rust
/// Enforce minimum metadata version. Refuse operation if below V14.
pub fn assert_compatible_metadata_version(version: u8) -> Result<()> {
    match version {
        14 => Ok(()),
        15 => Ok(()), // V15 preferred; improved type info
        16 => Ok(()), // V16 when available; monitor subxt issue #1901
        v if v < 14 => Err(PolkagentError::IncompatibleMetadataVersion {
            found: v,
            minimum: 14,
        }),
        _ => Ok(()), // forward-compatible: accept future versions with warning
    }
}
```

#### Runtime upgrade event handling

The drift detector subscribes to finalized-head events via the RPC subscription layer. When a new finalized block arrives, the detector compares the runtime version announced in the block header against the profile's pinned version. This provides near-real-time upgrade detection without polling.

```rust
/// Subscription-driven upgrade watcher.
pub struct UpgradeWatcher {
    profile_id: ProfileId,
    subscription: FinalizedHeadSubscription,
    last_known_spec: u32,
}

impl UpgradeWatcher {
    pub async fn run(mut self, event_tx: mpsc::Sender<PolkagentEvent>) {
        while let Some(header) = self.subscription.next().await {
            if header.runtime_version.spec_version != self.last_known_spec {
                let _ = event_tx.send(PolkagentEvent::RuntimeUpgradeDetected {
                    profile_id: self.profile_id.clone(),
                    old_spec: self.last_known_spec,
                    new_spec: header.runtime_version.spec_version,
                    at_block: header.number,
                }).await;
                self.last_known_spec = header.runtime_version.spec_version;
            }
        }
    }
}
```

---

### A.2 Chain Profile System

#### Complete ChainProfile struct definition

```rust
use std::collections::HashMap;
use std::time::Duration;

/// A chain profile identifies a specific network and its current
/// verified integration state. All high-risk operations are bound
/// to a profile reference.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChainProfile {
    /// Schema version for forward-compatible deserialization.
    pub schema_version: u32,

    // --- Identity ---
    /// Unique identifier for this profile configuration.
    pub id: ProfileId,
    /// Monotonically increasing version for this profile (increments on re-validation).
    pub profile_version: u32,
    /// Human-readable name (e.g. "polkadot-hub-mainnet-v1").
    pub name: String,
    /// Short description for operator display.
    pub description: String,
    /// Network classification.
    pub network_class: NetworkClass,

    // --- Network identity (immutable) ---
    /// Hex-encoded genesis hash. Never changes for a given network.
    pub genesis_hash: [u8; 32],
    /// SS58 address prefix for display (0 = Polkadot, 2 = Kusama, 42 = generic).
    pub ss58_prefix: u16,
    /// Human-readable chain name from genesis (e.g. "Polkadot Asset Hub").
    pub chain_name: String,

    // --- Runtime snapshot (changes on upgrade) ---
    /// Spec version at last validation.
    pub runtime_spec_version: u32,
    /// Transaction version at last validation.
    pub transaction_version: u32,
    /// Metadata encoding version (14 or 15).
    pub metadata_version: u8,
    /// Blake2-256 hash of pinned metadata bytes.
    pub metadata_hash: [u8; 32],
    /// Block number at which metadata was pinned.
    pub metadata_pinned_at_block: u64,

    // --- RPC configuration ---
    /// Ordered list of RPC endpoints (primary first, then failovers).
    pub rpc_endpoints: Vec<RpcEndpoint>,
    /// Timeout for individual RPC calls.
    pub rpc_timeout: Duration,
    /// Maximum concurrent RPC connections.
    pub rpc_max_connections: usize,

    // --- Access control ---
    /// Exact pallet/call combinations permitted for write operations.
    pub call_allowlist: CallAllowlist,
    /// Canonical asset identifiers, decimal scales, sufficiency, ED rules.
    pub asset_registry: AssetRegistry,
    /// Destination address allowlist for transfers (None = unrestricted within policy).
    pub destination_allowlist: Option<DestinationAllowlist>,
    /// Maximum fee in planck that any single operation may spend.
    pub max_fee_planck: Option<u128>,

    // --- Signer support ---
    /// Tested wallet/hardware/proxy signing methods for this profile.
    pub signer_support: Vec<SignerCapability>,
    /// Whether CheckMetadataHash signed extension is required (recommended: true).
    pub require_metadata_hash_check: bool,

    // --- Validation state ---
    /// Current operational state of this profile.
    pub state: ProfileState,
    /// Timestamp of last successful validation.
    pub last_validation: Timestamp,
    /// Reference to the fixture corpus used for validation.
    pub fixture_corpus: CorpusRef,
    /// Human-readable validation notes (e.g. known limitations).
    pub validation_notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NetworkClass {
    /// Live network with real economic value.
    Production,
    /// Public test network (e.g. Paseo).
    Testnet,
    /// Local ephemeral network (Zombienet / Chopsticks).
    Local,
    /// Developer simulation (in-memory, deterministic).
    Development,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProfileState {
    /// Profile has been validated and is operational.
    Active,
    /// Metadata hash or spec version mismatch detected. High-risk writes blocked.
    Drifted { detected_at: Timestamp },
    /// No endpoint is reachable. All chain operations blocked.
    Unavailable { last_reachable: Timestamp },
    /// Profile created but not yet validated. No operations permitted.
    Pending,
    /// Archived. Not used for new operations; available for receipt verification.
    Archived { archived_at: Timestamp },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RpcEndpoint {
    /// WebSocket or HTTPS URL.
    pub url: String,
    /// Human-readable provider name (e.g. "OnFinality", "Dwellir").
    pub provider: Option<String>,
    /// Priority: lower number = higher priority.
    pub priority: u8,
    /// Whether this endpoint supports light-client connections.
    pub supports_light_client: bool,
    /// Health check interval. None = no periodic checking.
    pub health_check_interval: Option<Duration>,
}

/// A call allowlist entry identifies one callable extrinsic by pallet + call name.
/// Indices are not used because they can change across runtime upgrades.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct AllowedCall {
    /// Pallet name as it appears in metadata (e.g. "Balances").
    pub pallet: String,
    /// Call name (e.g. "transfer_keep_alive").
    pub call: String,
    /// Optional human annotation explaining why this call is permitted.
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CallAllowlist {
    pub entries: Vec<AllowedCall>,
}

impl CallAllowlist {
    /// Returns true if the given pallet/call pair is permitted.
    pub fn is_permitted(&self, pallet: &str, call: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.pallet == pallet && e.call == call)
    }
}

/// Immutable reference to a profile version, embedded in all chain operations.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChainProfileRef {
    pub profile_id: ProfileId,
    pub profile_version: u32,
    pub genesis_hash: [u8; 32],
    pub metadata_hash: [u8; 32],
    pub spec_version: u32,
}
```

#### Profile versioning and migration

Every time a profile is re-validated after a drift event, `profile_version` is incremented. The previous version is serialized to `profiles/archive/<id>_v<version>.toml`. This ensures that every historical `ChainProfileRef` embedded in receipts and evidence remains resolvable.

```rust
impl ChainProfile {
    /// Promote this profile to a new version after re-validation.
    pub fn promote(&self, new_metadata: MetadataEntry) -> Self {
        Self {
            profile_version: self.profile_version + 1,
            runtime_spec_version: new_metadata.spec_version,
            metadata_version: new_metadata.metadata_version,
            metadata_hash: new_metadata.metadata_hash,
            metadata_pinned_at_block: new_metadata.fetched_at_block,
            state: ProfileState::Active,
            last_validation: Timestamp::now(),
            // All other fields carry forward unchanged.
            ..self.clone()
        }
    }
}
```

#### Profile discovery (well-known chains vs custom)

Polkagent ships a built-in registry of well-known chain profiles as a TOML bundle in the binary. Well-known profiles include genesis hashes for Polkadot Hub (mainnet and Paseo), People Chain (mainnet and Paseo), and the relay chain. Custom profiles are loaded from the operator's `~/.polkagent/profiles/` directory.

```rust
pub enum ProfileSource {
    /// Bundled at compile time. Operator activates before use.
    WellKnown { registry_key: &'static str },
    /// Loaded from the operator's profile directory.
    Custom { path: PathBuf },
}

pub struct ProfileRegistry {
    well_known: HashMap<String, ChainProfile>,
    custom: HashMap<ProfileId, ChainProfile>,
}

impl ProfileRegistry {
    /// List all available profiles (well-known + custom).
    pub fn list(&self) -> Vec<&ChainProfile> { /* ... */ }

    /// Resolve a profile by id, checking custom first then well-known.
    pub fn resolve(&self, id: &ProfileId) -> Option<&ChainProfile> { /* ... */ }

    /// Activate a well-known profile by running first-time validation.
    pub async fn activate_well_known(
        &mut self,
        key: &str,
        client: &dyn ChainClient,
    ) -> Result<ProfileId> { /* ... */ }
}
```

#### Profile validation and health checking

A profile health check verifies three things independently:

1. **Metadata binding**: The live chain's metadata hash matches the profile's pinned hash.
2. **Endpoint reachability**: At least one configured RPC endpoint is responding.
3. **Fixture corpus**: The compiled fixture tests pass against the current live state.

```rust
pub struct ProfileHealthReport {
    pub profile_id: ProfileId,
    pub profile_version: u32,
    pub checked_at: Timestamp,
    pub metadata_binding: HealthCheck,
    pub endpoint_status: Vec<(String, HealthCheck)>,
    pub fixture_corpus: HealthCheck,
    pub overall: ProfileState,
}

#[derive(Debug, Clone)]
pub enum HealthCheck {
    Pass,
    Warn { message: String },
    Fail { message: String },
}
```

#### Call allowlist enforcement

Every intent construction request passes through allowlist enforcement before any SCALE encoding occurs. This is a synchronous gate in the chain adapter, not an RPC call.

```rust
impl ChainAdapter {
    pub fn enforce_allowlist(
        &self,
        profile: &ChainProfile,
        pallet: &str,
        call: &str,
    ) -> Result<()> {
        if !profile.call_allowlist.is_permitted(pallet, call) {
            return Err(PolkagentError::CallNotAllowed {
                profile_id: profile.id.clone(),
                pallet: pallet.to_string(),
                call: call.to_string(),
                suggestion: "Add this call to the profile's call_allowlist after review.",
            });
        }
        Ok(())
    }
}
```

---

### A.3 RPC Client Layer

The Polkadot RPC layer uses WebSocket connections for subscriptions and HTTP/WS for request-response queries. Polkagent's adapter wraps the underlying Subxt client with connection pool management, endpoint failover, and typed subscription handling.

#### Connection pool management

```rust
/// Connection pool: maintains one live connection per endpoint,
/// with automatic reconnection on failure.
pub struct RpcConnectionPool {
    /// Ordered endpoint list (primary first).
    endpoints: Vec<RpcEndpoint>,
    /// Active connections keyed by endpoint URL.
    connections: HashMap<String, Arc<SubxtClient>>,
    /// Current primary endpoint index.
    primary_idx: AtomicUsize,
    /// Connection establishment timeout.
    connect_timeout: Duration,
    /// Reconnect backoff settings.
    backoff: ExponentialBackoff,
}

impl RpcConnectionPool {
    /// Get a healthy client, attempting endpoints in priority order.
    pub async fn healthy_client(&self) -> Result<Arc<SubxtClient>> {
        for endpoint in self.endpoints_by_priority() {
            if let Some(client) = self.connections.get(&endpoint.url) {
                if self.is_healthy(client).await {
                    return Ok(Arc::clone(client));
                }
            }
            // Attempt to (re)connect.
            match self.connect(&endpoint.url).await {
                Ok(client) => return Ok(client),
                Err(e) => {
                    tracing::warn!(url = %endpoint.url, error = %e, "endpoint unavailable");
                    continue;
                }
            }
        }
        Err(RpcError::AllEndpointsUnavailable)
    }

    async fn is_healthy(&self, client: &SubxtClient) -> bool {
        // Send a lightweight system_health RPC call with a short timeout.
        client.rpc().system_health().await.is_ok()
    }
}
```

#### Endpoint failover and load balancing

Failover is priority-order: Polkagent always tries the highest-priority endpoint first. Load balancing across equal-priority endpoints is round-robin with health gating. A failed endpoint is placed in a cooldown window (`ENDPOINT_COOLDOWN_SECS`, default 30) before being retried.

```rust
const ENDPOINT_COOLDOWN_SECS: u64 = 30;

struct EndpointState {
    url: String,
    priority: u8,
    last_failure: Option<Instant>,
    failure_count: u32,
}

impl EndpointState {
    fn is_in_cooldown(&self) -> bool {
        self.last_failure
            .map(|t| t.elapsed().as_secs() < ENDPOINT_COOLDOWN_SECS)
            .unwrap_or(false)
    }
}
```

#### Subscription management (finalized heads, events)

Subscriptions are long-lived WebSocket streams. Polkagent maintains two standard subscriptions per active profile: finalized-head notifications (for drift detection) and chain storage change events (for operation monitoring).

```rust
pub struct ProfileSubscriptions {
    profile_id: ProfileId,
    /// Stream of finalized block headers.
    finalized_heads: FinalizedHeadStream,
    /// Stream of storage change sets for watched keys.
    storage_changes: StorageChangeStream,
    /// Cancellation token to shut down subscriptions cleanly.
    cancel: CancellationToken,
}

impl ProfileSubscriptions {
    pub async fn spawn(
        profile: &ChainProfile,
        client: Arc<SubxtClient>,
        watched_keys: Vec<StorageKey>,
        event_tx: mpsc::Sender<PolkagentEvent>,
    ) -> Result<Self> {
        let finalized_heads = client
            .blocks()
            .subscribe_finalized()
            .await?;

        let storage_changes = client
            .rpc()
            .state_subscribe_storage(watched_keys)
            .await?;

        // Spawn background tasks that forward events to the event bus.
        // ...
        Ok(Self { /* ... */ })
    }
}
```

#### Request/response type mapping

All RPC responses are mapped to Polkagent's stable domain types before being returned to callers. This isolates the rest of the system from Subxt's evolving API surface.

```rust
/// Stable domain types returned by the chain adapter.
pub struct AccountInfo {
    pub address: AccountRef,
    pub free_balance: u128,
    pub reserved_balance: u128,
    pub nonce: u32,
    pub profile_ref: ChainProfileRef,
    pub at_block: BlockRef,
}

pub struct AssetBalance {
    pub asset_ref: AssetRef,
    pub balance: u128,
    pub is_frozen: bool,
    pub is_sufficient: bool,
    pub profile_ref: ChainProfileRef,
    pub at_block: BlockRef,
}

pub struct BlockRef {
    pub number: u64,
    pub hash: [u8; 32],
}
```

#### SCALE codec integration

SCALE encoding and decoding for intent construction uses Subxt's static codegen path for allowlisted calls and the dynamic path for decode/explain workflows.

```rust
/// Construct a transfer_keep_alive call using static codegen types.
/// Static path: compile-time type safety, no dynamic dispatch.
pub fn build_transfer_keep_alive_call(
    dest: &AccountRef,
    value: u128,
    profile: &ChainProfile,
) -> Result<EncodedCall> {
    // Generated by subxt-codegen from pinned metadata.
    use polkadot_hub_static::runtime::balances::calls::types::TransferKeepAlive;

    let call = TransferKeepAlive {
        dest: dest.to_multi_address()?,
        value,
    };

    let call_scale = call.encode();
    Ok(EncodedCall {
        pallet: "Balances".to_string(),
        call_name: "transfer_keep_alive".to_string(),
        scale_bytes: call_scale,
        profile_ref: profile.as_ref(),
    })
}

/// Decode arbitrary extrinsic bytes using the dynamic path.
/// Returns a structured decode result with per-argument values and types.
pub async fn decode_extrinsic(
    bytes: &[u8],
    profile: &ChainProfile,
) -> Result<DecodedExtrinsic> {
    let metadata = profile.load_pinned_metadata()?;
    // Use subxt dynamic decode API.
    let decoded = subxt::dynamic::decode_extrinsic(bytes, &metadata)?;
    Ok(DecodedExtrinsic::from_subxt_dynamic(decoded, profile.as_ref()))
}
```

---

### A.4 XCM Integration

#### XCM message construction helpers

XCM message construction is always source-chain-aware. The helper produces a typed `XcmPlan` rather than raw bytes; bytes are only produced at the signer handoff stage.

```rust
/// A typed XCM plan produced by the planner. Not yet signed or submitted.
#[derive(Debug, Clone)]
pub struct XcmPlan {
    /// Source chain profile reference.
    pub source: ChainProfileRef,
    /// Destination chain profile reference.
    pub destination: ChainProfileRef,
    /// Transfer mechanism chosen by the planner.
    pub mechanism: XcmMechanism,
    /// Asset being transferred (profile-bound identity, not display name).
    pub asset: AssetRef,
    /// Amount in the asset's smallest denomination.
    pub amount: u128,
    /// Beneficiary on the destination chain.
    pub beneficiary: AccountRef,
    /// Fee estimate (may be a range if DryRunApi is unavailable).
    pub fee_estimate: FeeEstimate,
    /// XCM version negotiated for this route.
    pub xcm_version: u8,
    /// Whether this plan has been validated by DryRunApi.
    pub dry_run_validated: bool,
    /// Expiry: plan is invalid after this block.
    pub expires_at_block: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XcmMechanism {
    /// Reserve-backed transfer (asset lives on a reserve chain).
    ReserveTransfer,
    /// Teleport (both chains trust each other to mint/burn).
    Teleport,
}

#[derive(Debug, Clone)]
pub struct FeeEstimate {
    /// Source chain fee in planck.
    pub source_fee: FeeRange,
    /// Destination chain execution weight cost.
    pub dest_weight_fee: Option<FeeRange>,
    /// Delivery fee from source to destination.
    pub delivery_fee: Option<FeeRange>,
    /// Asset used to pay fees on each chain.
    pub source_fee_asset: AssetRef,
    pub dest_fee_asset: Option<AssetRef>,
}

#[derive(Debug, Clone)]
pub struct FeeRange {
    /// Best estimate.
    pub estimate: u128,
    /// Upper bound with safety margin (use for user display).
    pub max: u128,
}
```

#### Fee estimation for cross-chain transfers

Fee estimation uses the `XcmPaymentApi` and `DryRunApi` where available. When the destination chain does not expose these APIs, a conservative heuristic based on weight constants is used and explicitly labeled as an estimate.

```rust
pub async fn estimate_xcm_fees(
    plan: &XcmPlan,
    source_client: &dyn ChainClient,
    dest_client: &dyn ChainClient,
) -> Result<FeeEstimate> {
    // 1. Check fee-paying asset options on destination.
    let acceptable_assets = dest_client
        .xcm_payment_api_query_acceptable_payment_assets(plan.xcm_version)
        .await
        .ok(); // None if API not available

    // 2. Run DryRunApi on source chain.
    let dry_run_result = source_client
        .dry_run_api_dry_run_call(&plan.as_extrinsic())
        .await
        .ok();

    // 3. Query delivery fee from source.
    let delivery_fee = source_client
        .xcm_payment_api_query_delivery_fee(
            &plan.destination.genesis_hash,
            &plan.as_xcm_message(),
        )
        .await
        .ok();

    // 4. Compose the estimate.
    Ok(FeeEstimate {
        source_fee: source_client.estimate_extrinsic_fee(&plan.as_extrinsic()).await?,
        dest_weight_fee: dry_run_result.as_ref().and_then(|r| r.dest_weight_fee.clone()),
        delivery_fee,
        // If DryRunApi unavailable, mark as heuristic.
        // ...
    })
}
```

#### Reserve transfer vs teleport decision logic

The mechanism choice is not user-configurable. It is determined by querying the runtime's XCM configuration for the route.

```rust
/// Determine the appropriate XCM mechanism for a given route.
/// Returns Err if the route is not supported.
pub async fn resolve_xcm_mechanism(
    source: &ChainProfileRef,
    destination: &ChainProfileRef,
    asset: &AssetRef,
    source_client: &dyn ChainClient,
) -> Result<XcmMechanism> {
    // Query the source chain's XCM router and trusted teleporter config.
    let teleport_trusted = source_client
        .is_trusted_teleporter(destination, asset)
        .await?;

    if teleport_trusted {
        return Ok(XcmMechanism::Teleport);
    }

    // Check if reserve transfer is configured for this route.
    let reserve_configured = source_client
        .is_reserve_transfer_supported(destination, asset)
        .await?;

    if reserve_configured {
        return Ok(XcmMechanism::ReserveTransfer);
    }

    Err(XcmError::RouteNotSupported {
        source: source.profile_id.clone(),
        destination: destination.profile_id.clone(),
        asset: asset.clone(),
        suggestion: "Verify XCM channel configuration on both chains with Chopsticks simulation.",
    })
}
```

#### Multi-hop routing

Most routes in the Polkadot ecosystem are direct (hub-to-parachain or parachain-to-hub). Multi-hop routes (parachain-to-parachain via relay or hub) require explicit hop resolution.

```rust
/// A resolved XCM route with one or more hops.
#[derive(Debug, Clone)]
pub struct XcmRoute {
    /// Ordered list of chains the message passes through.
    pub hops: Vec<ChainProfileRef>,
    /// Mechanism for each hop.
    pub mechanisms: Vec<XcmMechanism>,
    /// Whether this is a direct route (single hop).
    pub is_direct: bool,
}

/// Resolve a route between two chains. Multi-hop routes are routed via Hub.
pub async fn resolve_route(
    source: &ChainProfileRef,
    destination: &ChainProfileRef,
    asset: &AssetRef,
    client: &dyn ChainClient,
) -> Result<XcmRoute> {
    // Try direct first.
    if let Ok(mechanism) = resolve_xcm_mechanism(source, destination, asset, client).await {
        return Ok(XcmRoute {
            hops: vec![source.clone(), destination.clone()],
            mechanisms: vec![mechanism],
            is_direct: true,
        });
    }
    // Fall back to hub-mediated routing.
    // Hub is resolved from the well-known profile registry.
    // ...
    Err(XcmError::NoRouteFound {
        source: source.profile_id.clone(),
        destination: destination.profile_id.clone(),
    })
}
```

#### XCM dry-run validation

No XCM plan proceeds to the signer handoff stage without a successful dry-run. The dry-run gate is enforced in the intent construction pipeline.

```rust
/// Gate: run DryRunApi before allowing XCM intent to proceed to signing.
pub async fn xcm_dry_run_gate(
    plan: &mut XcmPlan,
    source_client: &dyn ChainClient,
) -> Result<DryRunResult> {
    let result = source_client
        .dry_run_api_dry_run_call(&plan.as_extrinsic())
        .await
        .map_err(|e| XcmError::DryRunFailed {
            reason: e.to_string(),
            mitigation: "Validate XCM route with Chopsticks before retrying.",
        })?;

    if !result.execution_result.is_ok() {
        return Err(XcmError::DryRunExecutionFailed {
            result: result.execution_result,
        });
    }

    plan.dry_run_validated = true;
    Ok(result)
}
```

---

## APPENDIX B: PRODUCT ENGINEERING WORKBENCH

### B.1 Zombienet Integration

#### Test network provisioning

The Zombienet adapter wraps the `zombienet-sdk` crate (or the CLI binary as a subprocess, depending on feature flags). The adapter manages the full network lifecycle: validation, launch, readiness polling, test execution, and teardown.

```rust
pub struct ZombienetAdapter {
    /// Path to the zombienet binary (if using CLI subprocess mode).
    binary: Option<PathBuf>,
    /// Provider backend.
    provider: ZombienetProvider,
    /// Working directory for network artifacts.
    work_dir: PathBuf,
    /// Resource limits enforced on launched processes.
    resource_limits: ResourceLimits,
}

#[derive(Debug, Clone)]
pub enum ZombienetProvider {
    /// Docker containers per node.
    Docker,
    /// Podman containers per node.
    Podman,
    /// Native OS processes (requires node binaries on PATH).
    Native,
    /// Kubernetes (advanced; not default).
    Kubernetes { kubeconfig: PathBuf },
}

impl ZombienetAdapter {
    /// Validate a scenario specification before attempting launch.
    /// Returns a structured report of validation issues.
    pub fn validate_scenario(&self, spec: &ScenarioSpec) -> Result<ValidationReport> {
        // Check: all referenced binaries exist.
        // Check: provider requirements are met.
        // Check: chain spec files exist and are valid JSON.
        // Check: port/resource conflicts.
        // ...
    }

    /// Launch an ephemeral network. Returns a handle for interaction.
    pub async fn launch(
        &self,
        spec: &ScenarioSpec,
        config: &LaunchConfig,
    ) -> Result<NetworkHandle> {
        // 1. Check prerequisites.
        self.check_prerequisites()?;
        // 2. Write working files.
        self.prepare_work_dir(spec)?;
        // 3. Spawn nodes.
        let network = self.spawn_network(spec).await?;
        // 4. Poll readiness (timeout from config).
        self.wait_for_readiness(&network, config.readiness_timeout).await?;
        Ok(NetworkHandle { network, work_dir: self.work_dir.clone() })
    }
}
```

#### Chain spec generation

Polkagent can generate chain specs from templates for common network topologies (solo relay, relay+parachain, relay+two-parachains). Templates are filled with the operator-supplied parameters.

```rust
pub struct ChainSpecTemplate {
    /// Template identifier (e.g. "relay-plus-asset-hub").
    pub id: &'static str,
    /// Number of relay chain nodes.
    pub relay_nodes: usize,
    /// Parachain descriptors included in the template.
    pub parachains: Vec<ParachainDescriptor>,
}

pub struct ParachainDescriptor {
    pub para_id: u32,
    pub collator_count: usize,
    /// Binary name expected on PATH.
    pub binary: String,
    /// Whether this parachain uses a genesis wasm override.
    pub genesis_wasm: Option<PathBuf>,
}

pub fn generate_chain_spec(
    template: &ChainSpecTemplate,
    overrides: &ChainSpecOverrides,
) -> Result<serde_json::Value> {
    // Merge template with overrides. Apply genesis account funding, sudo key, etc.
    // ...
}
```

#### Node lifecycle management

The `NetworkHandle` owns the spawned process tree. Dropping the handle triggers cleanup.

```rust
pub struct NetworkHandle {
    /// Zombienet internal network object.
    network: zombienet_sdk::Network,
    /// Working directory; cleaned up on drop.
    work_dir: PathBuf,
}

impl NetworkHandle {
    /// Return the WebSocket RPC URL for the named node.
    pub fn node_rpc_url(&self, node_name: &str) -> Result<String> { /* ... */ }

    /// Return the WS URL for the named parachain's collator.
    pub fn parachain_rpc_url(&self, para_id: u32) -> Result<String> { /* ... */ }

    /// Block until all nodes report peers and a minimum block number.
    pub async fn wait_for_block(&self, min_block: u64, timeout: Duration) -> Result<()> { /* ... */ }
}

impl Drop for NetworkHandle {
    fn drop(&mut self) {
        // Terminate all node processes and clean up work_dir.
    }
}
```

#### Log aggregation from test nodes

Each node writes logs to a file in the working directory. The adapter aggregates logs into a structured `NetworkLogs` artifact for the agent to analyze on test failure.

```rust
pub struct NetworkLogs {
    pub network_id: String,
    pub collected_at: Timestamp,
    /// Per-node log entries, truncated to the last N lines if very large.
    pub node_logs: HashMap<String, Vec<LogEntry>>,
    /// Detected errors and warnings, extracted by pattern matching.
    pub anomalies: Vec<LogAnomaly>,
}

pub struct LogAnomaly {
    pub node: String,
    pub level: LogLevel,
    pub message: String,
    pub line_number: usize,
    pub timestamp: Option<String>,
}
```

---

### B.2 Chopsticks Integration

#### Fork-at-block simulation

Chopsticks forks a chain at a specific block by downloading state from a live endpoint and serving it locally. The Polkagent adapter starts Chopsticks as a subprocess and connects to its local WebSocket port.

```rust
pub struct ChopsticksAdapter {
    /// Path to the chopsticks binary.
    binary: PathBuf,
    /// HTTP port for the forked chain's RPC.
    port: u16,
    /// Working directory for fork state.
    work_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ForkConfig {
    /// Live endpoint to fork from.
    pub endpoint: String,
    /// Block number to fork at. None = fork at latest.
    pub at_block: Option<u64>,
    /// Whether to allow state overrides on the fork.
    pub allow_overrides: bool,
    /// Whether to start the Chopsticks database in-memory (no disk persistence).
    pub in_memory: bool,
}

impl ChopsticksAdapter {
    pub async fn fork(&self, config: &ForkConfig) -> Result<ForkHandle> {
        let port = self.find_free_port()?;
        let child = self.spawn_chopsticks(config, port).await?;
        self.wait_for_ready(port, Duration::from_secs(30)).await?;
        Ok(ForkHandle { child, port, config: config.clone() })
    }
}

pub struct ForkHandle {
    child: tokio::process::Child,
    pub port: u16,
    pub config: ForkConfig,
}

impl ForkHandle {
    /// WebSocket URL for this fork's RPC.
    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }
}

impl Drop for ForkHandle {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}
```

#### State manipulation for testing

State overrides allow tests to preset balances, storage values, or governance state without replaying real transactions.

```rust
/// A storage override for a forked chain.
#[derive(Debug, Clone)]
pub struct StorageOverride {
    /// Pallet name.
    pub pallet: String,
    /// Storage item name.
    pub item: String,
    /// SCALE-encoded key suffix (for maps) or empty for plain storage.
    pub key: Vec<u8>,
    /// SCALE-encoded value to set.
    pub value: Vec<u8>,
}

impl ChopsticksAdapter {
    /// Apply storage overrides to a running fork via Chopsticks' `dev_setStorage` RPC.
    pub async fn apply_overrides(
        &self,
        handle: &ForkHandle,
        overrides: &[StorageOverride],
    ) -> Result<()> {
        let client = self.connect_to_fork(handle).await?;
        let changes: serde_json::Value = overrides
            .iter()
            .map(|o| {
                // Chopsticks `dev_setStorage` accepts pallet-keyed maps.
                json!({ o.pallet.as_str(): { o.item.as_str(): hex::encode(&o.value) } })
            })
            .collect();
        client.rpc().request("dev_setStorage", rpc_params![changes]).await?;
        Ok(())
    }
}
```

#### Deterministic replay

For storage migration testing (A1), Polkagent applies the migration to the fork and compares pre/post state with a deterministic diff.

```rust
/// Apply a runtime upgrade WASM to a fork and capture the storage diff.
pub async fn rehearse_migration(
    adapter: &ChopsticksAdapter,
    fork: &ForkHandle,
    wasm_path: &Path,
) -> Result<MigrationReport> {
    let pre_hash = fork.state_root().await?;
    // Upload WASM and trigger upgrade via `dev_setWasmCode`.
    adapter.set_wasm_code(fork, wasm_path).await?;
    // Advance one block to execute the migration on_runtime_upgrade hook.
    adapter.advance_blocks(fork, 1).await?;
    let post_hash = fork.state_root().await?;

    let diff = adapter.diff_storage(fork, pre_hash, post_hash).await?;

    Ok(MigrationReport {
        fork_block: fork.config.at_block,
        pre_state_root: pre_hash,
        post_state_root: post_hash,
        diff,
        migration_wasm: wasm_path.to_path_buf(),
        captured_at: Timestamp::now(),
    })
}
```

---

### B.3 Contract Toolchain

#### ink! contract compilation pipeline

```rust
pub struct CargoContractAdapter {
    /// Path to the `cargo-contract` binary.
    binary: PathBuf,
    /// Workspace root for the contract project.
    workspace: PathBuf,
    /// Resource limits on the compilation process.
    resource_limits: ResourceLimits,
}

impl CargoContractAdapter {
    /// Compile an ink! contract. Returns a build artifact with the `.contract`
    /// bundle path, WASM size, and compiler version.
    pub async fn build(&self, manifest: &Path) -> Result<ContractBuildArtifact> {
        let output = self.run_subprocess(
            &["contract", "build", "--manifest-path", &manifest.display().to_string()],
        ).await?;

        let wasm_path = self.locate_wasm_output(&output)?;
        let contract_path = wasm_path.with_extension("contract");

        Ok(ContractBuildArtifact {
            wasm_path,
            contract_bundle: contract_path,
            wasm_size_bytes: std::fs::metadata(&wasm_path)?.len(),
            compiler_version: self.get_version().await?,
            build_output: output.stdout,
        })
    }
}
```

#### Contract deployment automation

Deployment is gated: the adapter returns a `DeploymentPlan` for operator review before any live submission. The actual submission uses the standard `ChainIntent` / signer handoff pipeline.

```rust
pub struct DeploymentPlan {
    /// Chain profile for the target network.
    pub profile_ref: ChainProfileRef,
    /// Contract bundle (WASM + ABI).
    pub contract_bundle: PathBuf,
    /// Constructor selector and arguments.
    pub constructor: ConstructorCall,
    /// Estimated storage deposit.
    pub storage_deposit_estimate: u128,
    /// Whether this is a dry-run preview only.
    pub is_preview: bool,
}

impl CargoContractAdapter {
    /// Estimate deployment cost without submitting.
    pub async fn estimate_deployment(
        &self,
        plan: &DeploymentPlan,
        client: &dyn ChainClient,
    ) -> Result<DeploymentEstimate> {
        // Use `contracts_call` dry-run RPC to estimate without submission.
        // ...
    }

    /// Produce a ChainIntent for deploying the contract. Requires operator approval.
    pub fn deployment_intent(&self, plan: &DeploymentPlan) -> Result<ChainIntent> {
        // Construct `contracts.instantiate_with_code` call intent.
        // ...
    }
}
```

#### ABI management and type generation

ink! contract ABIs are extracted from the `.contract` bundle (a JSON file containing the ABI alongside the WASM). Polkagent parses the ABI to generate typed call builders and event decoders.

```rust
pub struct InkAbi {
    pub contract_name: String,
    pub ink_version: String,
    pub constructors: Vec<InkConstructor>,
    pub messages: Vec<InkMessage>,
    pub events: Vec<InkEvent>,
    pub source_hash: [u8; 32],
}

impl InkAbi {
    /// Parse from a `.contract` bundle file.
    pub fn from_bundle(path: &Path) -> Result<Self> { /* ... */ }

    /// Encode a message call into SCALE bytes for submission.
    pub fn encode_message(&self, selector: &str, args: &[serde_json::Value]) -> Result<Vec<u8>> { /* ... */ }

    /// Decode an event from raw SCALE bytes.
    pub fn decode_event(&self, bytes: &[u8]) -> Result<DecodedEvent> { /* ... */ }
}
```

---

## APPENDIX C: JAM/PVM RESEARCH ROADMAP

### Validation gates for JAM adoption decisions

Before any Polkagent feature may depend on JAM, the following gates must all pass:

| Gate | Description | Status |
|---|---|---|
| **JAM-G1: Multi-client testnet stability** | At least two independent JAM implementations have run a public multi-client testnet for 30+ days without halting. | Not yet open. Trigger: W3F announces multi-client testnet launch. |
| **JAM-G2: Stable service ABI** | A versioned, stable API for deploying and invoking JAM services exists with backward-compatibility guarantees. | Not yet open. Trigger: Parity publishes service ABI v1.0 with stability commitment. |
| **JAM-G3: Developer toolchain** | A supported SDK for building JAM services in Rust exists with documentation, examples, and a working deploy/invoke/state-query cycle on testnet. | Not yet open. Trigger: SDK tagged v1.0 with worked examples. |
| **JAM-G4: Cost model** | The computational cost per service invocation and state-storage cost are published and stable enough to model agent workload budgets. | Not yet open. Trigger: Published fee schedule on testnet. |
| **JAM-G5: Security review** | An independent security audit of the JAM service model covers the threat model relevant to agent-controlled services. | Deferred until G1–G4 pass. |

Until JAM-G1 and JAM-G2 are both open, no Polkagent product feature may reference JAM service APIs.

### Minimum viable JAM integration spec

When gates open, the minimum viable JAM integration is:

1. A `JamChainClient` implementing the existing `ChainClient` trait with `JamServiceIntent` and `JamServiceReceipt` as associated types.
2. A `JamServiceDeployer` tool adapter for deploying a pre-audited service bundle to a JAM testnet.
3. A profile-level `JamServiceProfile` extending `ChainProfile` with service-specific fields (service_id, refine_selector, accumulate_selector, on_transfer_selector).
4. Integration tests against a local JAM node (when a stable embedded node exists).
5. The feature gated behind `#[cfg(feature = "jam")]` with no effect on the default build.

### PVM execution model compatibility assessment

PVM (PolkaVM) is a RISC-V-based deterministic execution environment. Key compatibility questions for Polkagent:

| Question | Assessment |
|---|---|
| Can Polkagent interact with PVM contracts via `pallet_revive` calls? | Yes. PVM contracts are invoked through standard Subxt extrinsics targeting `pallet_revive::bare_call`. |
| Can Polkagent deploy PVM contracts? | Yes, for research spikes. The `resolc` compiler produces PVM bytecode from Solidity. Bytecode size limits and CREATE2 divergence (see Section 2.8) must be validated first. |
| Can PVM replace the EVM surface for agent escrow? | Not yet. PVM contracts are preview; tooling and network stability requirements are unmet. Revisit when `pallet_revive` is production-stable on Hub. |
| Does PVM's three-dimensional metering model affect agent fee estimation? | Yes. PVM charges `ref_time`, `proof_size`, and `storage_deposit` separately. Fee estimation must account for all three dimensions; the EVM gas model does not apply. |
| Can JAM's PVM differ from Hub's PVM? | Potentially. JAM PVM may have different constraints, entry points (refine/accumulate/onTransfer), and state model. Treat them as separate execution environments until JAM-G2 is open. |

### Timeline alignment with JAM mainnet

Based on public statements from core developers (12–20 month horizon from 2026-07-30):

```text
2026 Q3-Q4   Milestone 1 evaluations by W3F Fellowship (correct block import)
             PVM contract tooling (resolc) continues stabilization on Hub
             Polkagent: validate-next PVM research spike

2027 Q1-Q2   Expected JAM multi-client testnet (trigger for JAM-G1)
             Polkagent: monitor; begin JAM-G2 evaluation if testnet stable

2027 Q3-Q4   Potential JAM mainnet (highly uncertain)
             Polkagent: begin MVP JAM integration if G1-G4 all pass

2028+        Stable JAM service SDK; potential D1-D4 product features
```

This timeline is informational only and will be superseded by actual ecosystem events. The trigger-based gates (JAM-G1 through JAM-G5) are the controlling mechanism, not calendar dates.

---

## APPENDIX D: IMPLEMENTATION CHECKLIST

The following ordered task list covers the full implementation surface described in this PRD. Each task includes an acceptance criterion. Tasks are grouped by system and ordered within each group from foundational to advanced.

### D.1 Metadata system

- [ ] **META-01**: Implement `MetadataCache` with disk-backed SCALE storage and sidecar JSON manifests.
  - Acceptance: `fetch_and_cache` round-trips correctly; reload from disk yields identical `metadata_hash`.
- [ ] **META-02**: Implement `check_drift` with fast spec-version path and full hash-comparison path.
  - Acceptance: fixture test with injected spec bump produces `SpecVersionDrifted`; fixture with hash change produces `MetadataDrifted` with non-empty `diff`.
- [ ] **META-03**: Implement `metadata_field_diff` producing structured diff of pallets/calls/storage/events.
  - Acceptance: known metadata pair (e.g. before/after a pallet addition) produces correct added/removed entries.
- [ ] **META-04**: Implement `UpgradeWatcher` subscription-driven drift detection.
  - Acceptance: simulated spec-version change in Chopsticks fork triggers `RuntimeUpgradeDetected` event within one finalization cycle.
- [ ] **META-05**: Integrate `assert_compatible_metadata_version`; refuse operation on sub-V14 metadata.
  - Acceptance: connecting to a V13 metadata endpoint returns `IncompatibleMetadataVersion` error.
- [ ] **META-06**: Implement metadata version pinning in `ChainProfile`; track V16 readiness per subxt issue #1901.
  - Acceptance: profile serializes `metadata_version` field; upgrade path from V14 to V15 is covered by a migration test.

### D.2 Chain profiles

- [ ] **PROF-01**: Implement `ChainProfile` struct with full field set; TOML serialization/deserialization round-trips losslessly.
  - Acceptance: `serde_roundtrip` test passes for all sample profiles.
- [ ] **PROF-02**: Implement `ProfileRegistry` with well-known and custom profile loading.
  - Acceptance: well-known Hub (mainnet and Paseo) profiles load from bundled TOML; custom profile loaded from `~/.polkagent/profiles/` directory.
- [ ] **PROF-03**: Implement `ProfileState` transitions: `Pending -> Active -> Drifted -> Active` (after re-validation).
  - Acceptance: state machine test covers all transitions including `Unavailable` and `Archived`.
- [ ] **PROF-04**: Implement `ProfileHealthReport` with independent metadata/endpoint/fixture checks.
  - Acceptance: mocked endpoint failure produces `Fail` for endpoint check without affecting metadata or fixture checks.
- [ ] **PROF-05**: Implement `CallAllowlist.is_permitted` enforcement in intent construction pipeline.
  - Acceptance: attempt to construct intent for non-allowlisted call returns `CallNotAllowed`.
- [ ] **PROF-06**: Implement `ChainProfile::promote` for post-drift re-validation; archive old version.
  - Acceptance: promotion increments `profile_version`; old version serialized to archive directory.
- [ ] **PROF-07**: Ship curated well-known profiles: Hub mainnet, Hub Paseo, People Chain mainnet, People Chain Paseo, Relay (read-only).
  - Acceptance: all five profiles load, parse, and pass genesis hash validation against live endpoints.

### D.3 RPC layer

- [ ] **RPC-01**: Implement `RpcConnectionPool` with priority-order failover and per-endpoint cooldown.
  - Acceptance: with primary endpoint mocked as down, `healthy_client` returns a connection on the secondary endpoint.
- [ ] **RPC-02**: Implement endpoint health checking via `system_health` RPC with configurable timeout.
  - Acceptance: health check returns `Fail` within 2 seconds for an unreachable endpoint.
- [ ] **RPC-03**: Implement `ProfileSubscriptions` for finalized-head and storage-change subscriptions.
  - Acceptance: finalized-head subscription emits events for each new finalized block; cancellation token stops the subscription cleanly.
- [ ] **RPC-04**: Implement stable domain type mapping: `AccountInfo`, `AssetBalance`, `BlockRef`.
  - Acceptance: Subxt response types map to domain types without leaking Subxt types to callers.
- [ ] **RPC-05**: Implement static codegen path for allowlisted calls (transfer_keep_alive, governance votes).
  - Acceptance: `build_transfer_keep_alive_call` produces SCALE bytes that round-trip through the dynamic decode path.
- [ ] **RPC-06**: Implement dynamic decode path for explain/inspect with recursion and size limits.
  - Acceptance: deeply nested batch call decodes correctly; exceeding recursion limit returns `DecodeLimitExceeded` rather than stack overflow.

### D.4 XCM

- [ ] **XCM-01**: Implement `resolve_xcm_mechanism` for reserve-transfer vs teleport selection.
  - Acceptance: Hub-to-relay route resolves to teleport; Hub-to-external-parachain resolves to reserve-transfer; unsupported route returns `RouteNotSupported`.
- [ ] **XCM-02**: Implement `estimate_xcm_fees` using `XcmPaymentApi` and `DryRunApi`; fall back to heuristic with explicit label when APIs unavailable.
  - Acceptance: fee estimate fixture matches documented Hub-to-relay fee range; heuristic path is labeled `"estimate_only"` in the output.
- [ ] **XCM-03**: Implement `xcm_dry_run_gate` as a mandatory pre-signing check.
  - Acceptance: XCM intent with a known-bad route fails the dry-run gate before reaching the signer port.
- [ ] **XCM-04**: Implement XCM version negotiation; check `SupportedVersion` storage on both chains.
  - Acceptance: version negotiation fixture correctly selects V4 when both chains support V4 and V5 but destination only supports V4.
- [ ] **XCM-05**: Implement multi-hop route resolution via Hub for parachain-to-parachain routes.
  - Acceptance: parachain-A to parachain-B route resolves through Hub when direct route is unavailable.
- [ ] **XCM-06**: Validate all XCM routes with Chopsticks simulation before testnet promotion.
  - Acceptance: Chopsticks XCM simulation fixture passes for Hub-relay and Hub-parachain routes.

### D.5 Workbench tools

- [ ] **WORK-01**: Implement `ZombienetAdapter` with validate/launch/run-tests/teardown lifecycle.
  - Acceptance: relay+parachain scenario launches, produces at least one finalized block, and tears down cleanly.
- [ ] **WORK-02**: Implement `NetworkHandle` with per-node RPC URL resolution and block-readiness polling.
  - Acceptance: `wait_for_block(5, 60s)` succeeds in a local Zombienet scenario.
- [ ] **WORK-03**: Implement `NetworkLogs` aggregation with anomaly extraction.
  - Acceptance: known error pattern in node log is detected and surfaced in `NetworkLogs.anomalies`.
- [ ] **WORK-04**: Implement `ChopsticksAdapter` with fork/apply-upgrade/set-storage/advance-blocks/diff.
  - Acceptance: `rehearse_migration` detects a seeded storage migration bug and produces a non-empty diff.
- [ ] **WORK-05**: Implement `StorageOverride` application via Chopsticks `dev_setStorage` RPC.
  - Acceptance: balance override is reflected in subsequent account query on the fork.
- [ ] **WORK-06**: Implement `CargoContractAdapter` build/estimate/deploy pipeline for ink! contracts.
  - Acceptance: sample ink! flipper contract compiles and produces a `.contract` bundle with correct WASM hash.
- [ ] **WORK-07**: Implement `InkAbi` parser and message encoder/event decoder.
  - Acceptance: flipper contract `flip` message encodes to correct SCALE selector bytes; `Flipped` event decodes correctly.
- [ ] **WORK-08**: Implement `PopCliAdapter` generate/build/launch-network wrappers.
  - Acceptance: Pop CLI generates a parachain template project and the generated project compiles.

### D.6 JAM research

- [ ] **JAM-01**: Open JAM-G1 tracking ticket. Monitor W3F multi-client testnet announcements.
  - Acceptance: ticket exists; reviewed monthly.
- [ ] **JAM-02**: Open JAM-G2 tracking ticket. Monitor Parity service ABI publications.
  - Acceptance: ticket exists; reviewed monthly.
- [ ] **JAM-03**: Implement PVM research spike: resolc compile + `pallet_revive` deploy on local testnet.
  - Acceptance: sample Solidity contract compiles with resolc, deploys to local Zombienet Hub node, and a `bare_call` returns expected output. Bytecode size and CREATE2 address recorded in the spike artifact.
- [ ] **JAM-04**: Implement `#[cfg(feature = "jam")]` feature gate; confirm default build compiles without it.
  - Acceptance: `cargo build` (no feature flags) and `cargo build --features jam` both succeed with zero errors.

---

## APPENDIX E: REFERENCE FILE MAP

The table below maps each Polkagent integration component to its most relevant reference implementations in the Roko and Bardo codebases. These are design references, not dependencies.

| Component | Roko Files | Bardo Files | Key Patterns |
|---|---|---|---|
| **Chain client trait** | `/Users/will/dev/nunchi/roko/roko/crates/roko-chain/src/wallet.rs` — `ChainWallet` trait: read/write split, address/balance/nonce/sign-submit/receipt | — | Read-write separation; `async_trait`; opaque handle types (`TxHash`, `Receipt`); name() for logging |
| **RPC client (request/response)** | `/Users/will/dev/nunchi/roko/roko/apps/roko-chain-watcher/src/rpc_client.rs` — `MirageRpcClient`: typed JSON-RPC envelope, atomic request ID, `send_rpc<R>` generic helper | — | `AtomicU64` request ID; strongly-typed response structs; `serde_json::Value` only at boundary; structured error variants |
| **Subscription / polling loop** | `/Users/will/dev/nunchi/roko/roko/apps/roko-chain-watcher/src/watcher.rs` — `Watcher`: poll-react loop with rate limiting, dry-run mode, event counter, graceful exit | — | `parking_lot::Mutex` for reset window; `AtomicU32/U64` for counters; `tracing` for observability; `tokio::time::sleep` interruptible by `max_events` gate |
| **Chain watcher config** | `/Users/will/dev/nunchi/roko/roko/apps/roko-chain-watcher/src/config.rs` — `WatcherCli`: clap-derived flags with env fallbacks | — | `clap::Parser`; `with_defaults()` for test/embed usage; `env=` attribute for 12-factor compatibility |
| **TUI chain explorer** | — | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/world/chain.rs` — `ChainScreen`: block feed with head display and tx/gas columns | `ChainScreenState::from_app_state`; `Block`+`Borders`+`Paragraph`; placeholder "awaiting" text; `take(10)` for recent entries |
| **TUI protocol status** | — | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/world/protocols.rs` — `ProtocolsScreen`: tabular protocol list with selection highlight | `format!("{:<42} {:<12} {:>14.2}")` column layout; selected-row `Color::Yellow` highlight; empty-state placeholder |
| **TUI bridge/XCM monitor** | — | `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/widgets/protocol/bridge_status.rs` — `BridgeStatusWidget`: route/amount/fee/status badge + in-flight `Gauge` | `status_badge` color mapping; `flight_progress_pct` saturating math; `write_row` height-guard pattern; `BridgeStatusWidgetConfig` for feature toggling |

### Specific pattern references

**Read-write trait split (from Roko `wallet.rs`):** Polkagent's `ChainClient` (reads) and `ChainSigner` (writes) should maintain the same separation as Roko's `ChainClient` + `ChainWallet`. This allows mock implementations in tests that cover only the read or write path.

**Typed RPC envelope (from Roko `rpc_client.rs`):** The `RpcRequest`/`RpcResponse` envelope pattern with a single generic `send_rpc<R: DeserializeOwned>` helper is the correct structure for the Subxt RPC extension layer. Never expose `serde_json::Value` to business logic; always deserialize at the boundary.

**Rate-limit window reset (from Roko `watcher.rs`):** The `maybe_reset_window` + `acquire_reaction_slot` pattern using `parking_lot::Mutex<Instant>` and `AtomicU32` is applicable to Polkagent's per-profile RPC rate limiting.

**Clap config with `with_defaults()` (from Roko `config.rs`):** All Polkagent chain adapter configurations should provide a `with_defaults()` constructor for use in tests and embedded contexts, alongside the clap-derived CLI parser.

**TUI awaiting placeholder (from Bardo `chain.rs` and `protocols.rs`):** The pattern of checking `is_empty()` and rendering a `DarkGray` "awaiting..." placeholder before the main content is the standard Polkagent TUI pattern for subscription-driven data that may not yet be populated.

**Bridge status badge (from Bardo `bridge_status.rs`):** The `status_badge(status) -> (&'static str, Color)` pattern and the `flight_progress_pct` saturating percentage calculation are directly reusable for XCM operation status display.

---

## APPENDIX F: TUI SURFACE FOR POLKADOT

The Polkagent TUI (built on `ratatui`, following Bardo's screen/widget architecture) exposes five Polkadot-specific views. Each view follows the `Screen` trait pattern from Bardo: a state struct populated from `AppState`, a `render` method, and key handlers.

### F.1 Chain Explorer View

**Reference:** `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/world/chain.rs`

The chain explorer shows live block feed, finalized head, and active profile status for the current chain.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ POLKADOT / Chain Explorer                                  [q]uit  [Tab]next │
├─────────────────────────────────────────────────────────────────────────────┤
│ Profile   polkadot-hub-mainnet-v3         State  ● ACTIVE                   │
│ Network   Polkadot Asset Hub             Head   #22,481,334                 │
│ Genesis   0x68d5...4e20                  Spec   1003004                     │
│ Meta Hash 0xf3a1...c8b2 ✓               Drift  none detected               │
│                                                                             │
│ Recent Finalized Blocks                                                     │
│ ──────────────────────────────────────────────────────────────────────────  │
│ #22481334   47 extrinsics   weight 398,241,000   12s ago                   │
│ #22481333   31 extrinsics   weight 271,000,000   24s ago                   │
│ #22481332   52 extrinsics   weight 441,873,200   36s ago                   │
│ #22481331   18 extrinsics   weight 155,990,100   48s ago                   │
│ #22481330   39 extrinsics   weight 334,020,500   60s ago                   │
│                                                                             │
│ RPC Endpoints                                                               │
│ ● wss://polkadot-asset-hub-rpc.polkadot.io    [primary]   latency  42ms    │
│ ○ wss://rpc.ibp.network/asset-hub-polkadot    [failover]  latency  88ms    │
│ ○ wss://sys.ibp.network/asset-hub-polkadot    [failover]  latency 114ms    │
└─────────────────────────────────────────────────────────────────────────────┘
```

```rust
#[derive(Debug, Clone, Default)]
pub struct ChainExplorerState {
    pub profile_name: String,
    pub profile_state: ProfileState,
    pub genesis_hash_hex: String,
    pub spec_version: u32,
    pub metadata_hash_hex: String,
    pub drift_status: DriftStatus,
    pub head_block: u64,
    pub recent_blocks: Vec<BlockSummary>,
    pub endpoint_status: Vec<EndpointDisplay>,
    pub tick: u64,
}

#[derive(Debug, Clone, Default)]
pub struct BlockSummary {
    pub number: u64,
    pub extrinsic_count: usize,
    pub weight: u64,
    pub seconds_ago: u64,
}

#[derive(Debug, Clone)]
pub struct EndpointDisplay {
    pub url: String,
    pub role: &'static str,   // "primary" | "failover"
    pub healthy: bool,
    pub latency_ms: Option<u64>,
}
```

### F.2 Metadata Browser and Diff Viewer

The metadata browser renders the pinned metadata as a navigable tree: pallets > calls / storage / events / constants. The diff viewer highlights changes between the pinned and live versions when drift is detected.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ POLKADOT / Metadata Browser              Profile: polkadot-hub-mainnet-v3   │
├──────────────────────────────┬──────────────────────────────────────────────┤
│ Pallets                      │ Balances > Calls                             │
│ ──────────────────────────── │ ──────────────────────────────────────────── │
│ > Balances              [12] │ transfer_allow_death(dest, value)            │
│   Assets                [24] │ force_transfer(source, dest, value)          │
│   ForeignAssets         [18] │ transfer_keep_alive(dest, value)     ✓ ALW  │
│   Identity               [8] │ transfer_all(dest, keep_alive)               │
│   Referenda             [15] │ force_unreserve(who, amount)                 │
│   ConvictionVoting       [9] │ upgrade_accounts(who)                        │
│   Proxy                 [11] │ force_set_balance(who, new_free)             │
│   Multisig              [10] │                                              │
│   Contracts             [22] │ ✓ ALW  = in call allowlist                  │
│   Revive                [14] │ [Enter] = inspect call signature             │
│   System                [16] │ [d]    = show diff vs live                   │
│   ...                        │ [/]    = search pallets                      │
└──────────────────────────────┴──────────────────────────────────────────────┘

[DRIFT MODE] — Pinned: spec 1003004  Live: spec 1003005
┌─────────────────────────────────────────────────────────────────────────────┐
│ Metadata Diff                                                  [Esc] close   │
├─────────────────────────────────────────────────────────────────────────────┤
│ + Added pallet: PolkadotXcm v3.2                                           │
│ ~ Changed: Balances.transfer_keep_alive — arg 2 type: Compact<u128>→u128   │
│ ~ Changed: Assets.create — added arg: is_sufficient: bool                  │
│ - Removed: OldPallet.deprecated_call                                        │
│                                                                             │
│ Allowlist impact:                                                           │
│   Balances.transfer_keep_alive  — SIGNATURE CHANGED, re-validate required   │
│                                                                             │
│ [u] Update profile  [r] Re-run fixtures  [Esc] dismiss                     │
└─────────────────────────────────────────────────────────────────────────────┘
```

```rust
pub struct MetadataBrowserState {
    pub profile_ref: ChainProfileRef,
    pub pallets: Vec<PalletSummary>,
    pub selected_pallet: usize,
    pub selected_item: usize,
    pub view_mode: MetadataBrowserMode,
    pub drift_diff: Option<MetadataDiff>,
    pub search_query: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataBrowserMode {
    Browse,
    InspectCall { pallet: String, call: String },
    DiffView,
    Search,
}
```

### F.3 Protocol Status Dashboard

**Reference:** `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/screens/world/protocols.rs`

The protocol status dashboard shows active chain profiles, their operational state, recent operation history, and allowlist summary.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ POLKADOT / Protocol Status                                 [q]uit  [Tab]next │
├─────────────────────────────────────────────────────────────────────────────┤
│ Profile                            State       Spec    Last Check           │
│ ──────────────────────────────────────────────────────────────────────────  │
│ polkadot-hub-mainnet-v3            ● ACTIVE    1003004  2s ago              │
│ polkadot-hub-paseo-v2              ● ACTIVE     900034  14s ago             │
│ people-chain-mainnet-v1            ● ACTIVE    1002001  8s ago              │
│ people-chain-paseo-v1            ◌ DRIFTED     900010  31s ago  !          │
│ polkadot-relay-readonly-v1         ● ACTIVE    1003000  5s ago              │
│                                                                             │
│ Recent Operations                                                           │
│ ──────────────────────────────────────────────────────────────────────────  │
│ 14:32:07  balance_query   polkadot-hub-mainnet   ok     12ms               │
│ 14:31:55  asset_query     polkadot-hub-mainnet   ok      9ms               │
│ 14:31:22  referenda_read  polkadot-relay-ro       ok     44ms              │
│ 14:30:58  identity_read   people-chain-mainnet   ok     27ms               │
│                                                                             │
│ [Enter] profile detail  [u] update drifted  [r] re-validate  [a] add      │
└─────────────────────────────────────────────────────────────────────────────┘
```

```rust
#[derive(Debug, Clone, Default)]
pub struct ProtocolStatusState {
    pub profiles: Vec<ProfileStatusEntry>,
    pub selected: usize,
    pub recent_ops: Vec<OperationLogEntry>,
    pub tick: u64,
}

#[derive(Debug, Clone)]
pub struct ProfileStatusEntry {
    pub name: String,
    pub state: ProfileState,
    pub spec_version: u32,
    pub last_check_secs_ago: u64,
    pub has_alert: bool,
}

#[derive(Debug, Clone)]
pub struct OperationLogEntry {
    pub timestamp: String,
    pub operation: String,
    pub profile: String,
    pub outcome: OperationOutcome,
    pub latency_ms: u64,
}
```

### F.4 Bridge Monitoring (XCM Monitor)

**Reference:** `/Users/will/dev/uniswap/bardo/apps/bardo-terminal/src/widgets/protocol/bridge_status.rs`

The XCM monitor shows in-flight cross-chain operations with progress indicators, fee summaries, and status badges. It adapts Bardo's `BridgeStatusWidget` pattern directly.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ POLKADOT / XCM Monitor                                     [q]uit  [Tab]next │
├─────────────────────────────────────────────────────────────────────────────┤
│ ┌─ Hub ↔ Relay · ReserveTransfer ───────────────────────────────────────┐   │
│ │ Route: Polkadot Asset Hub ──→ Polkadot Relay                         │   │
│ │ Amount: 10.0000 DOT                                                   │   │
│ │ Fee: ~0.0032 DOT  ETA: 24s                                           │   │
│ │ ◈ IN FLIGHT                                                           │   │
│ │ ██████████████████░░░░░░░░░░░░░░  67%                                │   │
│ └──────────────────────────────────────────────────────────────────────┘   │
│                                                                             │
│ ┌─ Hub ↔ Acala · ReserveTransfer ───────────────────────────────────────┐  │
│ │ Route: Polkadot Asset Hub ──→ Acala                                   │  │
│ │ Amount: 500.00 USDT (asset 1984)                                      │  │
│ │ Fee: ~0.0041 DOT (source) + ~0.12 ACA (dest)  ETA: 60s              │  │
│ │ ● COMPLETE                                                            │  │
│ └──────────────────────────────────────────────────────────────────────┘  │
│                                                                             │
│ ┌─ Hub ↔ ParaX · Teleport ──────────────────────────────────────────────┐  │
│ │ Route: Polkadot Asset Hub ──→ ParaX (para 2030)                      │  │
│ │ Amount: 2.5 DOT                                                       │  │
│ │ Fee: ~0.0028 DOT  ETA: 18s                                           │  │
│ │ ✗ FAILED  — trapped assets: verify on destination                    │  │
│ └──────────────────────────────────────────────────────────────────────┘  │
│ [Enter] details  [r] retry plan  [c] copy intent id                        │
└─────────────────────────────────────────────────────────────────────────────┘
```

```rust
/// Adapter of Bardo's BridgeStatusWidget for Polkagent XCM operations.
#[derive(Debug, Clone)]
pub struct XcmOperationEntry {
    pub intent_id: String,
    /// Source chain display name.
    pub from_chain: String,
    /// Destination chain display name.
    pub to_chain: String,
    /// XCM mechanism label.
    pub mechanism: String,
    /// Asset display (amount + symbol).
    pub amount_display: String,
    /// Fee summary string.
    pub fee_summary: String,
    /// Estimated time remaining in seconds.
    pub estimated_time_secs: u64,
    /// Unix timestamp of submission.
    pub submitted_at_secs: u64,
    /// Current operation status.
    pub status: XcmOpStatus,
    /// Failure reason if status is Failed.
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XcmOpStatus {
    Planned,
    AwaitingSignature,
    Submitted,
    InFlight,
    Complete,
    Failed,
}

fn xcm_status_badge(status: &XcmOpStatus) -> (&'static str, ratatui::style::Color) {
    use ratatui::style::Color;
    match status {
        XcmOpStatus::Planned          => ("◌ PLANNED",   Color::DarkGray),
        XcmOpStatus::AwaitingSignature => ("◌ SIGNING",   Color::Yellow),
        XcmOpStatus::Submitted        => ("◌ SUBMITTED", Color::Yellow),
        XcmOpStatus::InFlight         => ("◈ IN FLIGHT", Color::Magenta),
        XcmOpStatus::Complete         => ("● COMPLETE",  Color::Green),
        XcmOpStatus::Failed           => ("✗ FAILED",    Color::Red),
    }
}
```

### F.5 XCM Message Builder Visualization

The XCM builder visualizes a pending `XcmPlan` before the operator approves it for signing. It shows all plan fields, the dry-run result, and the fee estimate in a structured evidence card layout.

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ POLKADOT / XCM Builder — Intent Review                      [Esc] cancel    │
├─────────────────────────────────────────────────────────────────────────────┤
│ Intent ID     intent:8f3a2c..d4e1                                           │
│ Created       2026-07-30 14:31:02 UTC                                       │
│ Expires       block #22,481,394  (~10 minutes)                              │
│                                                                             │
│ Route                                                                       │
│   Source      Polkadot Asset Hub (hub-mainnet-v3, spec 1003004)            │
│   Destination Polkadot Relay Chain (relay-mainnet-v1, spec 1003000)        │
│   Mechanism   Teleport                                                      │
│   XCM Version V4                                                           │
│                                                                             │
│ Transfer                                                                    │
│   Asset       DOT (native, 10 decimals)                                    │
│   Amount      10.000000000000 DOT  (10000000000000 planck)                │
│   Beneficiary 15oF4...xQ3p (relay SS58)                                   │
│                                                                             │
│ Fees                                                                        │
│   Source fee  0.003200000000 DOT (estimate)                                │
│   Delivery    0.000400000000 DOT (from XcmPaymentApi)                     │
│   Dest weight 100,000,000 ref_time (from DryRunApi)                       │
│                                                                             │
│ Validation                                                                  │
│   DryRunApi   ✓ PASSED  — execution ok, no trapped assets                 │
│   Allowlist   ✓ route permitted by profile polkadot-hub-mainnet-v3        │
│   Expiry      ✓ plan valid until block #22,481,394                        │
│                                                                             │
│ [s] sign & submit    [e] export intent    [Esc] cancel                     │
└─────────────────────────────────────────────────────────────────────────────┘
```

```rust
pub struct XcmBuilderState {
    pub plan: XcmPlan,
    pub dry_run_result: Option<DryRunResult>,
    pub fee_estimate: FeeEstimate,
    pub validation_summary: Vec<ValidationItem>,
    pub current_focus: XcmBuilderFocus,
}

#[derive(Debug, Clone)]
pub struct ValidationItem {
    pub label: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XcmBuilderFocus {
    Review,
    SignAndSubmit,
    Export,
}
```

---

## APPENDIX G: CONFIGURATION GUIDE

### G.1 Chain profile TOML format

Chain profiles are stored as TOML files in `~/.polkagent/profiles/`. Well-known profiles are bundled at compile time but can be overridden by placing a file with the same `id` in the profiles directory.

```toml
# ~/.polkagent/profiles/polkadot-hub-mainnet-v3.toml
schema_version = 1
id = "polkadot-hub-mainnet-v3"
profile_version = 3
name = "polkadot-hub-mainnet-v3"
description = "Polkadot Asset Hub (mainnet) — validated 2026-07-30"
network_class = "Production"

# Immutable network identity
genesis_hash = "0x68d56f15f85d3136970ec16946040bc1752654e906147f7e43e9d539d7c3de2f"
ss58_prefix = 0
chain_name = "Polkadot Asset Hub"

# Runtime snapshot (update after each runtime upgrade)
runtime_spec_version = 1003004
transaction_version = 26
metadata_version = 15
metadata_hash = "0xf3a1c8b24e20..."  # full 64-char hex
metadata_pinned_at_block = 22_481_000

# RPC endpoints (primary first)
[[rpc_endpoints]]
url = "wss://polkadot-asset-hub-rpc.polkadot.io"
provider = "Parity"
priority = 1
supports_light_client = false
health_check_interval = "30s"

[[rpc_endpoints]]
url = "wss://rpc.ibp.network/asset-hub-polkadot"
provider = "IBP"
priority = 2
supports_light_client = false
health_check_interval = "30s"

[[rpc_endpoints]]
url = "wss://sys.ibp.network/asset-hub-polkadot"
provider = "IBP"
priority = 3
supports_light_client = false
health_check_interval = "60s"

# RPC settings
rpc_timeout = "10s"
rpc_max_connections = 4

# Signer requirements
require_metadata_hash_check = true

[[signer_support]]
type = "WatchOnly"
phase = 1

[[signer_support]]
type = "ExternalWallet"
wallets = ["Talisman", "SubWallet", "Nova"]
phase = 2

[[signer_support]]
type = "Ledger"
phase = 2

[[signer_support]]
type = "PolkadotVault"
phase = 2

# Call allowlist: only these pallet/call pairs may be used in write operations.
[[call_allowlist.entries]]
pallet = "Balances"
call = "transfer_keep_alive"
rationale = "Standard DOT transfer; safe destination requires approval"

[[call_allowlist.entries]]
pallet = "Assets"
call = "transfer"
rationale = "Fungible asset transfer (USDT 1984, USDC 1337)"

[[call_allowlist.entries]]
pallet = "ConvictionVoting"
call = "vote"
rationale = "OpenGov vote; gated by governance mandate"

# Asset registry
[[asset_registry.assets]]
id = "dot-native"
label = "DOT"
pallet = "native"
asset_id = 0
decimals = 10
is_sufficient = true
existential_deposit = 100_000_000  # 0.01 DOT in planck
verified_at_block = 22_481_000

[[asset_registry.assets]]
id = "usdt-1984"
label = "USDT"
pallet = "Assets"
asset_id = 1984
decimals = 6
is_sufficient = true
existential_deposit = 0
verified_at_block = 22_481_000
canonical_location = "{ parents: 0, interior: { X2: [PalletInstance(50), GeneralIndex(1984)] } }"

[[asset_registry.assets]]
id = "usdc-1337"
label = "USDC"
pallet = "Assets"
asset_id = 1337
decimals = 6
is_sufficient = true
existential_deposit = 0
verified_at_block = 22_481_000
canonical_location = "{ parents: 0, interior: { X2: [PalletInstance(50), GeneralIndex(1337)] } }"

# Validation state
[profile_state]
type = "Active"

last_validation = "2026-07-30T14:00:00Z"
fixture_corpus = "polkadot-hub-mainnet-v3-corpus"
validation_notes = [
    "USDC fee-sufficiency (referendum 174) not yet enacted; budget DOT for fees.",
    "EVM surface requires separate EVM profile configuration.",
]
```

### G.2 RPC endpoint configuration

Additional RPC endpoint options beyond what is in the profile TOML:

```toml
# ~/.polkagent/rpc.toml — global RPC settings, applied to all profiles unless overridden.

# Maximum number of simultaneous WebSocket connections across all profiles.
global_max_connections = 16

# Default timeout for unresponsive endpoints (overrides per-profile setting).
default_rpc_timeout = "10s"

# Endpoint cooldown after failure before retry.
endpoint_cooldown = "30s"

# Light client configuration (optional; enables smoldot-based connections).
[light_client]
enabled = false
bootnodes = []  # Override per-profile if needed.
```

### G.3 Metadata cache settings

```toml
# ~/.polkagent/metadata_cache.toml

# Directory for cached metadata SCALE files and sidecar manifests.
# Defaults to ~/.polkagent/metadata_cache/
store_dir = "~/.polkagent/metadata_cache/"

# Maximum number of cached metadata entries before LRU eviction.
max_entries = 50

# Minimum age before an entry is eligible for eviction.
min_age = "7d"

# Whether to keep all pinned-profile entries regardless of age (recommended: true).
keep_active_profile_entries = true
```

### G.4 XCM routing configuration

```toml
# ~/.polkagent/xcm.toml — XCM routing policy for all profiles.

# Whether DryRunApi validation is mandatory before any XCM submission.
# Setting this to false is strongly discouraged.
require_dry_run = true

# Maximum number of hops in a resolved XCM route.
max_hops = 2

# XCM version preference (negotiated down if destination does not support).
preferred_xcm_version = 5
minimum_xcm_version = 3

# Fee safety margin: multiply estimated fees by this factor for the maximum cap.
fee_safety_margin = 1.5

# Chains considered safe for direct teleport (hub-to-relay only by default).
# Additional chains require explicit addition after validation.
trusted_teleport_routes = [
    { source = "polkadot-hub-mainnet-v*", destination = "polkadot-relay-readonly-v*" },
]

# Reserve transfer routes allowed for supported assets.
allowed_reserve_transfer_routes = [
    { source = "polkadot-hub-mainnet-v*", destination = "*", assets = ["usdt-1984", "usdc-1337", "dot-native"] },
]

# Default expiry for XCM plans in blocks.
plan_expiry_blocks = 50

# Whether to require operator confirmation for multi-hop routes.
confirm_multihop = true
```

---

*Appendices A–G appended 2026-07-30.*
