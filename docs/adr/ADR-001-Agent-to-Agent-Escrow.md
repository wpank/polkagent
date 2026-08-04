# ADR-001: Agent-to-Agent Escrow via Multisig + Time-Delay Proxy

**Status:** proposed
**Date:** 2026-08-04
**Deciders:** polkagent core team
**PRD references:** PRD-08 §3.5 (escrow patterns), §5.3 (deliverable), §8.1 (pure proxy)

## Context

Polkagent agents need to transact with each other — Agent A pays Agent B to
perform work (data analysis, tool invocations, sub-task execution). Without an
escrow mechanism, the paying agent must either trust the counterparty fully
(pay-before-work risk) or the performing agent must trust it will be paid
(work-before-pay risk).

PRD-08 §3.5 specifies that Phase 1 supports off-chain escrow only, Phase 2
introduces on-chain escrow via Polkadot primitives, and Phase 3+ adds
smart-contract escrow (PVM/EVM). This ADR covers the **Phase 2** mechanism.

### Polkadot primitives available

| Primitive | Pallet | Use in escrow |
|-----------|--------|---------------|
| **Multisig** | `pallet_multisig` | 2-of-2 (or 2-of-3 with arbiter) co-signing for fund release |
| **Pure proxy** | `pallet_proxy` | Keyless account that holds escrowed funds; controlled entirely by proxy relationships |
| **Time-delay proxy** | `pallet_proxy` (announcement delay) | Announce-then-execute pattern provides an abort window before release |
| **Proxy filter** | `pallet_proxy` (`ProxyType`) | Restricts the call types a proxy can make (e.g. `Balances` only) |

### Constraints

- No PVM/EVM smart contracts in Phase 2 — must use native pallets only.
- Timeout/refund must be deterministic (PAY-ESCROW-002).
- The mechanism must be auditable — full lifecycle recorded as receipts.
- Must integrate with the existing `IntentStateMachine` and `BudgetChecker`.

## Decision

Implement agent-to-agent escrow using a **2-of-2 multisig + time-delay proxy**
pattern on Asset Hub, modeled as an in-process state machine. The off-chain
state machine drives on-chain extrinsic construction; the chain is the source
of truth for fund custody.

### Escrow lifecycle

```text
Proposed → Funded → Active → AwaitingRelease → Released
                  ↘ Expired                  ↘ Disputed → Resolved
         ↘ Cancelled                                    ↘ Refunded
```

1. **Proposed** — Buyer and seller agree on terms (amount, asset, timeout,
   completion criteria). An `EscrowAgreement` struct is created.

2. **Funded** — Buyer transfers funds to a **pure-proxy account**. The pure
   proxy has two proxy relationships:
   - A multisig proxy (2-of-2: buyer + seller) for release.
   - A time-delay proxy (buyer only) for timeout refund.

3. **Active** — Seller performs work. The funds are locked in the pure proxy.

4. **AwaitingRelease** — Seller signals completion and provides evidence
   (artifact hash, on-chain state reference, or signed attestation).

5. **Released** (terminal) — Both parties co-sign `multisig.as_multi` to
   transfer from the pure proxy to the seller. Receipt generated.

6. **Expired** (terminal) — Timeout block height reached without release.
   Buyer's time-delay proxy announcement matures; funds return to buyer.

7. **Disputed** — Either party flags a dispute before release. Escrow
   freezes; resolution requires arbiter intervention or mutual agreement.

8. **Resolved** (terminal) — Dispute resolved; funds sent to the determined
   party.

9. **Refunded** (terminal) — Funds returned to buyer (dispute resolved in
   buyer's favor, or cancellation before work begins).

10. **Cancelled** (terminal) — Escrow cancelled before funding completes.

### On-chain account topology

```text
┌─────────────────────────────────────────────────┐
│               Pure Proxy (escrow)               │
│                  (no private key)                │
│                                                 │
│  Proxy relationships:                           │
│  ├─ Multisig(buyer, seller) [2-of-2, Balances]  │
│  └─ Buyer [time-delay=T blocks, Balances]       │
└─────────────────────────────────────────────────┘

Release path:   multisig.as_multi(transfer → seller)   — both sign
Refund path:    proxy.announce(transfer → buyer)        — after T blocks
```

### Timeout behavior (PAY-ESCROW-002)

The time-delay proxy announcement gives the seller a window (T blocks) to
dispute before the buyer can unilaterally reclaim funds. T is configured at
escrow creation and is immutable once funded. The announcement is visible
on-chain, providing transparency.

```
Funded at block N, timeout = T blocks
├─ Block N+T:  buyer can proxy.proxy_announced(transfer → buyer)
├─ Before N+T: seller can reject the announcement via proxy.reject_announcement
└─ If neither acts by N+2T: funds remain in pure proxy (manual recovery)
```

## Options considered

### Option A: 2-of-2 multisig (no proxy)

Buyer sends funds to a 2-of-2 multisig address (buyer + seller).

- Pros: Simple; one pallet; well-understood.
- Cons: No unilateral timeout refund — requires both parties to cooperate for
  any movement of funds. Stuck funds if seller disappears. Violates
  PAY-ESCROW-002.

### Option B: Pure proxy + multisig + time-delay proxy (chosen)

Buyer funds a pure proxy controlled by multisig (release) and time-delay
proxy (refund).

- Pros: Deterministic timeout refund via time-delay proxy. No private key
  for escrow account. Proxy filter limits operations to balance transfers.
  Matches PRD-08 §3.5.2 exactly.
- Cons: More complex setup (3 extrinsics to configure proxy). Requires the
  buyer to pay proxy setup fees. Time-delay is in blocks, not wall-clock
  (acceptable for chain-native operations).

### Option C: Smart contract (PVM/EVM)

Deploy an escrow contract on a Polkadot parachain with EVM/PVM support.

- Pros: Arbitrary logic; multi-party arbitration; composability.
- Cons: Requires smart-contract audit (separate gate). Not available in
  Phase 2. Higher attack surface. Deferred to Phase 3+ per PRD-08.

### Option D: Off-chain escrow service

Trusted third-party holds funds and releases on API callback.

- Pros: Simple integration; flexible completion criteria.
- Cons: Introduces trust assumption and single point of failure.
  Counterparty risk shifts to the escrow service. Not self-sovereign.

## Consequences

### Positive

- Deterministic timeout refund satisfies PAY-ESCROW-002.
- No smart-contract risk — uses battle-tested Polkadot pallets.
- Pure proxy eliminates private key management for escrow accounts.
- Full lifecycle is auditable via on-chain events + off-chain state machine.
- State machine integrates naturally with existing `IntentStateMachine` pattern.

### Negative

- Proxy setup requires 3 extrinsics (pure proxy creation + 2 add_proxy calls),
  adding latency and fees to escrow initialization.
- Time-delay is block-based, not wall-clock — timeout duration varies with
  block production rate (6s target on Polkadot, ~12s on Asset Hub).
- 2-of-2 multisig requires both parties online for release; no automatic
  release on evidence submission (would need an oracle or arbiter in Phase 3).
- Dispute resolution is manual in Phase 2 — no on-chain arbitration logic.

### Neutral

- The off-chain state machine is the source of intent/status; the chain is
  the source of truth for fund custody. Both must be reconciled.
- This pattern is extensible to 2-of-3 (with arbiter) in a future iteration.

## Validation

1. **Unit tests**: `EscrowStateMachine` enforces valid transitions; all
   invalid transitions are rejected.
2. **Integration test**: Full Proposed→Funded→Active→AwaitingRelease→Released
   cycle in `tests/escrow_lifecycle.rs` (future).
3. **Testnet dry-run**: Construct the proxy topology on Westend Asset Hub
   and verify release + timeout paths (Phase 2 milestone).
4. **Security review**: Independent review of proxy configuration before
   mainnet deployment (required by PAY-ESCROW-001).

## Related decisions

- PRD-08 §3.5.1: Off-chain escrow workflow (Phase 1, prerequisite)
- PRD-08 §3.5.2: Multisig + time-delay escrow (this ADR implements)
- PRD-08 §8.1: Pure proxy account management
- Future: ADR-NNN: Smart-contract escrow (Phase 3+)
