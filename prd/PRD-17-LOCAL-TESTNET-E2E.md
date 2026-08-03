# PRD-17: Local Testnet Infrastructure, End-to-End Testing, and Agent-Driven Development

**Status:** draft
**Owner:** unassigned
**Last updated:** 2026-08-03
**Audience:** engineers, QA, operators, and AI coding agents (Claude Code, etc.)
who need to run, test, debug, and diagnose polkagent against production-realistic
local Polkadot networks

---

## 1. Purpose and orientation

### 1.1 What this document covers

This PRD defines how polkagent provisions, configures, and runs against a
**local multi-chain Polkadot testnet** that mirrors production topology. The
goal is full end-to-end exercising of every polkagent capability — governance
research, treasury monitoring, balance queries, payment lifecycle, XCM
cross-chain transfers, staking operations, identity management, multisig/proxy
workflows, asset operations, and agent autonomy controls — against chains
whose state, parameters, and runtime behavior closely match Polkadot mainnet.

**Critically**, this PRD also defines how an **AI coding agent** (such as
Claude Code) can use this infrastructure as an interactive development and
debugging environment — spawning testnets, running individual tests, inspecting
chain state, diagnosing failures, and iterating on code with rapid feedback —
all through CLI commands and structured JSON output that an agent can invoke
and parse directly.

**The headline experience:**

```
polkagent testnet go        # One command. Everything works.
polkagent testnet demo      # See a governance proposal fly through in 4 minutes.
polkagent testnet try E2E-GOV-01   # Run one test, see every step live.
```

A reader who completes this document should understand:

1. **How to go from zero to a working test in one command** (§ 2).
2. What local testnet infrastructure is provisioned and how (§ 3-4).
3. How an AI agent or developer interactively runs, inspects, debugs, and
   diagnoses the entire stack from the CLI (§ 5).
4. What end-to-end test scenarios exercise every polkagent tool (§ 6).
5. How test execution, assertion, and reporting are structured (§ 7).
6. How this integrates with CI/CD and the TUI (§ 9-10).

### 1.2 Why local testnet testing matters

Polkagent's existing test pyramid (PRD-15 § 2) covers unit, integration,
contract, and property tests using `polkagent-chain-fake` and
`polkagent-signer-fake`. These tests validate internal correctness but do not
exercise:

- **Real SCALE encoding/decoding** against actual runtime metadata.
- **Real RPC interactions** with substrate nodes (HTTP JSON-RPC via subxt).
- **Real block production** with finality, session rotation, and era transitions.
- **Real cross-chain messaging** via HRMP channels between parachains.
- **Real governance lifecycles** with track parameters, deposits, and enactment.
- **Real treasury spends** with bounded periods and burn mechanics.
- **Real staking mechanics** with nomination, validator election, and payouts.

Without these, the gap between "tests pass" and "works on mainnet" remains
unacceptably large. This PRD closes that gap.

### 1.3 Why agent-driven development support matters

Polkagent is developed iteratively with AI coding agents (Claude Code) driving
implementation, testing, and debugging. The current workflow — writing code,
running `cargo test`, reading fake-chain test output — works for unit and
integration tests but cannot validate real chain behavior. Agents need to:

- **Spawn a testnet** with a single command and get structured connection info.
- **Run specific E2E scenarios** individually and see pass/fail with diagnostics.
- **Query live chain state** (balances, referenda, staking, XCM) to debug issues.
- **Submit test extrinsics** and observe results without leaving the CLI.
- **Inspect logs and events** when something fails, with structured output.
- **Iterate rapidly** — fix code, re-run one test, check chain state, repeat.

Without explicit support for this workflow, agents waste time parsing
unstructured output, guessing at chain state, and re-running entire test
suites when only one scenario needs attention.

### 1.4 Relationship to other PRDs

| PRD | What this document adds |
|---|---|
| PRD-03 (Execution) | Live effect lifecycle testing — real extrinsic submission, inclusion, finality |
| PRD-05 (Polkadot) | Concrete Zombienet/Chopsticks configurations for every chain integration |
| PRD-07 (Security) | Signer integration tests with real sr25519 keys and proxy/multisig flows |
| PRD-08 (Payments) | Payment lifecycle E2E — draft → simulate → sign → submit → confirm → receipt |
| PRD-14 (APIs) | Schema conformance against real runtime metadata |
| PRD-15 (Testing) | E2E test layer sitting atop the existing test pyramid |
| PRD-16 (TUI) | TUI exercised against live chain data for dashboard accuracy |

### 1.5 Labels

| Label | Meaning |
|---|---|
| **MUST** | Non-negotiable requirement (RFC 2119) |
| **SHOULD** | Strongly recommended; deviation requires documented rationale |
| **MAY** | Optional; included for completeness |
| **Verified** | Confirmed against upstream Polkadot SDK / Zombienet documentation |
| **Proposed** | Polkagent design choice requiring validation |

---

## 2. One-command experiences and visual design

This section defines the headline UX: what the user or agent sees and feels
when they interact with the local testnet infrastructure. Every command is
designed to be **one thing you type, one clear result you see**.

### 2.1 The three one-liners

These three commands represent the entire happy path. A developer or agent
should be able to go from nothing to a fully validated E2E test run with
just these:

```
polkagent testnet go              # Spawn + verify + run all Phase 1 tests
polkagent testnet demo            # Showcase: run a governance proposal end-to-end
polkagent testnet try E2E-GOV-01  # Run one scenario, see every step live
```

#### `polkagent testnet go`

One command to rule them all. Checks prerequisites, spawns the network,
waits for readiness, runs the Phase 1 test suite, and reports results.
If the network is already running, skips spawn and runs tests immediately.

```
$ polkagent testnet go

  ┌──────────────────────────────────────────────────────────────────┐
  │                                                                  │
  │   ◈  P O L K A G E N T   T E S T N E T                         │
  │                                                                  │
  └──────────────────────────────────────────────────────────────────┘

  Checking prerequisites...

    ✓  polkadot v1.16.0             /usr/local/bin/polkadot
    ✓  polkadot-parachain v1.16.0   /usr/local/bin/polkadot-parachain
    ✓  zombienet v2.1.0             /usr/local/bin/zombienet
    ✓  ports 9933-9994              all available
    ✓  disk space                   42.3 GB free

  Spawning network...

    ◈ relay chain        ████████████████████████████████████  ready
      alice              ws://127.0.0.1:9944   ▪ validator
      bob                ws://127.0.0.1:9954   ▪ validator

    ◈ asset hub          ████████████████████████████████████  ready
      collator-01        ws://127.0.0.1:9964   ▪ para 1000

    ◈ collectives        ████████████████████████████████████  ready
      collator-01        ws://127.0.0.1:9974   ▪ para 1001

    ◈ bridge hub         ████████████████████████████████████  ready
      collator-01        ws://127.0.0.1:9984   ▪ para 1002

    ◈ coretime           ████████████████████████████████████  ready
      collator-01        ws://127.0.0.1:9994   ▪ para 1005

    ✓  All 5 chains producing blocks            elapsed 38s

  Verifying genesis state...

    ✓  Dev accounts funded           6 accounts, 4M DOT total
    ✓  Validators active             alice, bob
    ✓  Staking configured            era 1, session 1
    ✓  Treasury funded               10,000,000 DOT
    ✓  Governance tracks             8 tracks, shortened periods
    ✓  Identity registrar            dave (index 0)
    ✓  Asset Hub: TUSD               asset 1337, 5 holders
    ✓  Collectives: Fellowship       dave at rank 1
    ✓  HRMP channels                 6 channels open

  Running E2E tests...

    E2E-TRES-01  Balance query & transfer     ✓  passed    2.1s
    E2E-PAY-01   Payment lifecycle (10 states) ✓  passed    8.4s

  ──────────────────────────────────────────────────────────
    2 passed · 0 failed · 10.5s total
  ──────────────────────────────────────────────────────────

  Network is still running. Use `polkagent testnet down` to stop.
```

#### `polkagent testnet demo`

A visual showcase that walks through an impressive end-to-end governance
proposal — submitting, voting, deciding, confirming, and enacting — with
a live progress display. Designed to demonstrate polkagent's capabilities
to stakeholders or to verify the full stack works.

```
$ polkagent testnet demo

  ┌──────────────────────────────────────────────────────────────────┐
  │                                                                  │
  │   ◈  GOVERNANCE DEMO — Referendum Lifecycle                     │
  │                                                                  │
  │   Submitting a Root-origin referendum, voting it through,        │
  │   and watching it enact — all in under 5 minutes.               │
  │                                                                  │
  └──────────────────────────────────────────────────────────────────┘

  ① Submit preimage                                          ✓ done
    ▸ System.remark("polkagent-demo-2026-08-03")
    ▸ hash: 0xa1b2c3d4...e5f6
    ▸ block #12 · fee 0.003 DOT

  ② Submit referendum on Root track                          ✓ done
    ▸ referendum #1 · track: root
    ▸ block #13 · fee 0.004 DOT

  ③ Place decision deposit                                   ✓ done
    ▸ 100 DOT deposited
    ▸ block #14

  ④ Preparing                                                ✓ done
    ▸ waiting 2 blocks...
    ▸ referendum #1 now in Preparing state

  ⑤ Cast votes                                               ✓ done
    ▸ alice: Aye · 500,000 DOT · conviction ×1
    ▸ bob:   Aye · 500,000 DOT · conviction ×1

  ⑥ Deciding ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━   ✓ done
    ▸ approval: 100.0%  ▸ support: 50.0%
    ▸ entered Confirming at block #18

  ⑦ Confirming ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━   ✓ done
    ▸ 10 confirm blocks elapsed
    ▸ referendum APPROVED at block #28

  ⑧ Enacting                                                 ✓ done
    ▸ 5 blocks enactment delay
    ▸ System.remark executed at block #33

  ⑨ Polkagent verification                                   ✓ done
    ▸ referendum_lookup:  Approved ✓
    ▸ track_info:         root track params match ✓
    ▸ voter_history:      alice vote recorded ✓
    ▸ treasury_overview:  treasury balance correct ✓

  ──────────────────────────────────────────────────────────
    Demo complete · 9 steps · all passed · 4m 12s
  ──────────────────────────────────────────────────────────

    Other demos:   polkagent testnet demo --scenario payment
                   polkagent testnet demo --scenario xcm
                   polkagent testnet demo --scenario staking
                   polkagent testnet demo --scenario multisig
```

#### `polkagent testnet try E2E-GOV-01`

Run a single scenario with live step-by-step output. Each step shows what's
happening, its status, and key data — in real time as it executes.

```
$ polkagent testnet try E2E-GOV-01

  E2E-GOV-01 · Referendum lifecycle — submit to enactment

    1 │ Submit preimage                              ✓   120ms
      │ hash: 0xa1b2...c3d4

    2 │ Submit referendum (Root track)               ✓   6.2s
      │ referendum #1 · block #14

    3 │ Place decision deposit                       ✓   6.1s
      │ 100 DOT · block #15

    4 │ Query referendum_lookup                      ✓   85ms
      │ status: Preparing

    5 │ Query track_info (root)                      ✓   62ms
      │ decision_period: 50 blocks

    6 │ Wait for prepare period (2 blocks)           ✓   12s
      │ block #14 → #16

    7 │ Alice votes Aye (500k DOT, ×1)              ✓   6.0s
      │ block #17

    8 │ Bob votes Aye (500k DOT, ×1)                ✓   6.1s
      │ block #18

    9 │ Query referendum_lookup                      ✓   78ms
      │ status: Deciding · approval: 100% · support: 50%

   10 │ Query voter_history (alice)                  ✓   95ms
      │ 1 vote: ref #1 Aye 500k DOT

   11 │ Wait for confirm period                      ✓   72s
      │ 12 blocks elapsed

   12 │ Query referendum_lookup                      ✓   80ms
      │ status: Approved

   13 │ Wait for enactment (5 blocks)                ✓   30s

   14 │ Verify System.remark enacted                 ✓   110ms
      │ event found in block #33

  ──────────────────────────────────────────────────────────
    ✓  14/14 steps passed · 2m 19s
    ✓  Invariants: INV-01 INV-02 INV-03 INV-04
  ──────────────────────────────────────────────────────────
```

When a step fails, the display is immediately diagnostic:

```
    9 │ Query referendum_lookup                      ✗   78ms
      │
      │   expected: status = Deciding
      │   actual:   status = Preparing
      │
      │   ℹ Referendum still in prepare period.
      │     submitted at block #14, current block #15,
      │     prepare_period = 2 blocks, need 1 more block.
      │
      │   💡 Try: polkagent testnet wait --blocks 1
      │
      │   Chain state at failure:
      │     relay best=#15  finalized=#13  era=1  session=1
      │
      │   Stopping (--stop-on-failure).
```

### 2.2 Visual status dashboard

```
$ polkagent testnet status

  ┌──────────────────────────────────────────────────────────────────┐
  │  ◈  LOCAL TESTNET                           uptime 12m 34s      │
  └──────────────────────────────────────────────────────────────────┘

  Chain             Endpoint              Best    Final   Status
  ─────────────────────────────────────────────────────────────────
  relay chain       ws://127.0.0.1:9944    #124    #121   ● producing
  asset hub         ws://127.0.0.1:9964    #118    #115   ● producing
  collectives       ws://127.0.0.1:9974    #116    #113   ● producing
  bridge hub        ws://127.0.0.1:9984    #117    #114   ● producing
  coretime          ws://127.0.0.1:9994    #115    #112   ● producing

  Staking            era 6 · session 12 · 2 validators
  Treasury           9,998,500 DOT · next spend in 26 blocks
  Governance         1 active referendum (root track, Deciding)
  HRMP               6 channels open · 0 pending messages
```

### 2.3 Visual balance table

```
$ polkagent testnet balances

  ┌──────────────────────────────────────────────────────────────────┐
  │  ◈  DEV ACCOUNT BALANCES                       at block #124    │
  └──────────────────────────────────────────────────────────────────┘

  Account    Free             Reserved     Transferable     Nonce
  ─────────────────────────────────────────────────────────────────
  alice      999,899.995 DOT  1.010 DOT    499,899.995 DOT  5
  bob        1,000,000.0 DOT  0.000 DOT    500,000.000 DOT  2
  charlie    499,950.0   DOT  0.100 DOT    399,950.000 DOT  3
  dave       500,100.0   DOT  1.000 DOT    500,100.000 DOT  0
  eve        499,999.0   DOT  0.500 DOT    499,999.000 DOT  1
  ferdie     500,000.0   DOT  0.000 DOT    500,000.000 DOT  0

  Asset Hub (TUSD):
  alice      100,000 TUSD     bob  90,000 TUSD     charlie  100,000 TUSD
  dave       100,000 TUSD     eve  610,000 TUSD
```

### 2.4 Visual query output

Every `query` subcommand has a beautiful default human output and a
`--format json` machine output. The human output is the default.

```
$ polkagent testnet query referendum 1

  ┌──────────────────────────────────────────────────────────────────┐
  │  ◈  REFERENDUM #1                               at block #124   │
  └──────────────────────────────────────────────────────────────────┘

  Track         root (id 0)
  Status        Deciding
  Submitted     block #14 (10m 42s ago)

  Tally
    Ayes        1,000,000 DOT                    ████████████████ 100%
    Nays        0 DOT                                              0%
    Support     50.0%

  Timeline
    Preparing   block #14 → #16                  ✓ done
    Deciding    block #16 → #66  (50 blocks)     ━━━━━━━━━━━━━━━  68%
    Confirming  needs 10 blocks                  ◌ pending
    Enactment   5 block delay                    ◌ pending

  Estimated completion     ~3m 12s from now

  Votes (2)
    alice       Aye · 500,000 DOT · conviction ×1
    bob         Aye · 500,000 DOT · conviction ×1
```

```
$ polkagent testnet query staking

  ┌──────────────────────────────────────────────────────────────────┐
  │  ◈  STAKING                                     at block #124   │
  └──────────────────────────────────────────────────────────────────┘

  Era 6 · Session 12 · 2 validators

  Validators
  ─────────────────────────────────────────────────────────────────
  alice        self: 100,000 DOT    nominators: 1    total: 101,000 DOT
  bob          self: 100,000 DOT    nominators: 1    total: 101,000 DOT

  Nominators
  ─────────────────────────────────────────────────────────────────
  charlie      bonded: 1,000 DOT    target: alice    status: active
  dave         bonded: 1,000 DOT    target: bob      status: active

  Nomination Pools
  ─────────────────────────────────────────────────────────────────
  Pool #1      depositor: bob       total: 150 DOT   members: 2

  Next era in 8 blocks (~48s)   Bonding period: 4 eras (~8 min)
```

### 2.5 Visual exec output

Extrinsic submissions show a live progress trail:

```
$ polkagent testnet exec transfer --from alice --to dave --amount 100

  ◈ Transfer 100 DOT · alice → dave

    Building extrinsic...                        ✓
    Signing (sr25519)...                         ✓
    Submitting to relay chain...                 ✓  tx 0xa1b2...c3d4
    Waiting for inclusion...                     ✓  block #125
    Waiting for finality...                      ✓  block #125 finalized

  ──────────────────────────────────────────────────────────
  ✓  Transfer complete

    Fee             0.0124 DOT
    Alice balance   999,799.983 DOT  (was 999,899.995)
    Dave balance    500,200.000 DOT  (was 500,100.000)
  ──────────────────────────────────────────────────────────
```

### 2.6 TUI integration (F9 — Testnet tab)

The polkagent TUI SHOULD add a **Testnet** tab (F9) that provides a live
dashboard of the local testnet when running. This integrates with the
existing ROSEDUST design system.

```
┌─ F1 Dashboard ─ F2 Agents ─ F3 Runs ─ F4 System ─ ··· ─ F9 Testnet ─┐
│                                                                        │
│  ┌─ Chains ─────────────────────────┐ ┌─ Active Referendum ──────────┐ │
│  │                                  │ │                              │ │
│  │  ● relay       #124  fin #121   │ │  #1 root — Deciding          │ │
│  │  ● asset hub   #118  fin #115   │ │  ████████████████░░░░  68%   │ │
│  │  ● collectives #116  fin #113   │ │  ayes 1M DOT  nays 0        │ │
│  │  ● bridge hub  #117  fin #114   │ │  ~3m 12s remaining           │ │
│  │  ● coretime    #115  fin #112   │ │                              │ │
│  │                                  │ └──────────────────────────────┘ │
│  │  era 6  session 12  uptime 12m  │                                  │
│  └──────────────────────────────────┘ ┌─ Treasury ───────────────────┐ │
│                                       │                              │ │
│  ┌─ Recent Events ──────────────────┐ │  Balance   9,998,500 DOT    │ │
│  │                                  │ │  Next spend  26 blocks      │ │
│  │  #124  Balances.Transfer         │ │  Burn rate   1%             │ │
│  │        alice → dave  100 DOT     │ │  Bounties    1 active       │ │
│  │  #122  Staking.EraPaid           │ │                              │ │
│  │        era 5  reward 100 DOT     │ └──────────────────────────────┘ │
│  │  #120  Session.NewSession        │                                  │
│  │        session 12                │ ┌─ Dev Balances ───────────────┐ │
│  │  #118  XCM.Sent                  │ │                              │ │
│  │        relay → asset hub         │ │  alice    999,899 DOT       │ │
│  │                                  │ │  bob    1,000,000 DOT       │ │
│  └──────────────────────────────────┘ │  charlie  499,950 DOT       │ │
│                                       │  dave     500,100 DOT       │ │
│  ┌─ HRMP ───────────────────────────┐ │  eve      499,999 DOT       │ │
│  │  1000 ⇄ 1001  ● open            │ │  ferdie   500,000 DOT       │ │
│  │  1000 ⇄ 1002  ● open            │ │                              │ │
│  │  1000 ⇄ 1005  ● open            │ └──────────────────────────────┘ │
│  └──────────────────────────────────┘                                  │
│                                                                        │
├────────────────────────────────────────────────────────────────────────┤
│ F9 Testnet │ ▪ 5 chains ● │ era 6 │ ref #1 Deciding 68% │ 12m 34s │
└────────────────────────────────────────────────────────────────────────┘
```

The Testnet tab refreshes every 5 seconds (matching existing TUI refresh
cycle) and uses the ROSEDUST palette:

- `dot_pink` for chain names and Polkadot-branded elements.
- `finalized_teal` for finalized block heights and "producing" status.
- `pending_amber` for pending referenda, unfinalized blocks.
- `success` (jade) for completed/approved items.
- `danger` (crimson) for failed transactions or slashing events.
- `parachain_violet` for parachain and XCM indicators.
- `rose` family for selection, focus, and interactive elements.

### 2.7 Demo scenarios

The `demo` command supports multiple themed showcases:

| Demo | What it shows | Duration |
|---|---|---|
| `governance` (default) | Submit referendum → vote → decide → confirm → enact | ~4 min |
| `payment` | Draft → simulate → approve → sign → submit → finalize → receipt | ~30s |
| `xcm` | Teleport DOT relay → Asset Hub → back, showing cross-chain balances | ~1 min |
| `staking` | Bond → nominate → era transition → payout → claim rewards | ~3 min |
| `multisig` | Create 2-of-3 multisig → fund → initiate → approve → execute | ~1 min |
| `assets` | Create token → mint → transfer → freeze → unfreeze → DEX swap | ~2 min |
| `identity` | Set identity → request judgement → receive judgement → sub-accounts | ~1 min |
| `kitchen-sink` | Run all demos sequentially — the full showcase | ~12 min |

```bash
polkagent testnet demo                         # Default: governance
polkagent testnet demo --scenario payment      # Payment lifecycle
polkagent testnet demo --scenario kitchen-sink # Everything
```

### 2.8 Alias commands for common operations

Frequently used operations get short aliases:

```bash
# These are equivalent:
polkagent testnet go                    # Check + spawn + test
polkagent testnet balances              # query balance --all-dev
polkagent testnet try E2E-GOV-01        # scenario run E2E-GOV-01 --verbose
polkagent testnet watch                 # status --follow (auto-refresh)
polkagent testnet send alice dave 100   # exec transfer --from alice --to dave --amount 100
polkagent testnet vote alice 1 aye      # exec vote --signer alice --referendum 1 --aye
```

### 2.9 Design principles for CLI output

All testnet CLI output follows these rules:

1. **Human-first.** The default output is beautiful, readable, and needs no
   flags. `--format json` is for machines and agents.
2. **Progressive disclosure.** Default output shows the essential info. Add
   `--verbose` for step-level detail. Add `--format json` for full data.
3. **Live progress.** Long operations (spawn, wait, scenario run) show a
   live progress display — not silence until completion.
4. **Color with meaning.** Colors from the ROSEDUST palette convey status:
   jade=success, amber=pending, crimson=error, teal=finalized, pink=Polkadot.
   Respects `NO_COLOR` and `--no-color`.
5. **One thing per command.** Each command does one thing and names it clearly.
   No flags-of-flags. If it's complex, it gets its own subcommand.
6. **Error context, not stack traces.** Failures show what went wrong, what
   was expected, what was actual, and what to try next — not raw error types.
7. **Consistent layout.** Every output uses the same box-drawing header style,
   the same alignment, the same table formatting.

### 2.10 Cinematic rendering — the Bardo-inspired visual system

> **Reference:** The visual philosophy below is adapted from the Bardo terminal
> specification (`/Users/will/dev/uniswap/bardo/prd/18-interfaces/`). Bardo
> treats the terminal as a consciousness medium, not a display medium. We apply
> that same philosophy to testnet infrastructure: the testnet is not a dashboard
> showing you numbers — it is a living network that has those numbers as its
> vital signs. You watch it breathe. You watch it finalize. You watch it govern.

#### 2.10.1 The perpetual motion principle (adapted from Bardo)

> **Source:** `bardo/prd/18-interfaces/26-bardo-terminal-foundation.md` §
> "The Perpetual Motion Principle"

Nothing on the testnet dashboard is ever at rest. Every element is driven by
at least one continuously changing variable. A chain name that hasn't changed
still sits next to a block height that ticks. A finalized height that hasn't
moved still has a phosphor afterimage of its last update fading. A balance
that's static still lives in a row where the nonce incremented. Static pixels
are bugs.

For every element on the testnet display, three questions:

1. **What makes it move?** Block production, finality, events, balances.
2. **What makes it change?** State transitions: referendum advancing, era
   rotating, transfer completing, test passing/failing.
3. **What makes it decay?** Phosphor persistence — bright moments leave ghosts,
   ghosts fade, fades are replaced by the next moment's ghost.

#### 2.10.2 Rendering techniques

The testnet TUI MUST use ratatui's full rendering vocabulary to achieve
cinematic visual density. These are the techniques, ordered by visual impact:

**Half-block double resolution** (`▀▄█`)
- An 80×24 terminal becomes 80×48 effective vertical pixels.
- Used for: network topology visualization, block production waveforms,
  approval/support gradient bars, XCM message flow arrows.
- Each cell carries two independent colors (foreground = upper, background =
  lower), enabling smooth color gradients impossible with single-character
  rendering.
- **Reference:** `bardo/prd/18-interfaces/rendering/01-demoscene.md` §
  "Half-Block Double Resolution"

**Braille sub-pixel rendering** (`⠀`–`⣿`, U+2800–U+28FF)
- 2×4 dot grid per cell = 8 sub-pixels per character.
- An 80×24 terminal becomes 160×96 effective resolution.
- Used for: block production sparklines (already in `token_sparkline.rs`),
  network throughput waveforms, finality lag oscilloscope traces, XCM
  message density fields.
- Dot count as brightness proxy: more dots = brighter apparent value.
- **Existing implementation:** `crates/polkagent-cli/src/tui/widgets/token_sparkline.rs`
  already uses `Marker::Braille` — the testnet tab extends this pattern.

**Block element density ramp** (`░▒▓█`)
- Used for: progress bars with gradient fills, heatmaps showing validator
  performance, approval/support thermometer gauges.
- The phosphor decay chain from Bardo: `█→▓→▒→░→·→ ` applied to recently-
  changed values. A balance that just changed glows `█`, then fades through
  the chain over 2 seconds.
- **Reference:** `bardo/prd/18-interfaces/rendering/00-design-system.md` §
  "Character vocabulary"

**Box-drawing and connector characters** (`┌─┐│└─┘├┤┬┴┼`)
- Used for: chain topology diagrams showing relay ↔ parachain connections,
  HRMP channel maps, multisig approval flow diagrams, referendum lifecycle
  timelines.

**Waveform characters** (`▁▂▃▄▅▆▇█`)
- Used for: block time oscilloscope (rolling 60-block history), finality lag
  trace, transaction throughput waveform, staking reward per-era bar chart.
- Phosphor decay applied: recent values at full brightness, older values
  dim through the `phosphor_res` color.
- **Reference:** `bardo/prd/18-interfaces/rendering/02-visualization-primitives.md` §
  "WaveformDisplay"

**Special Unicode characters for status and icons**

| Character | Meaning |
|---|---|
| `◈` | Polkagent brand mark (already used in headers) |
| `●` | Chain producing / node healthy |
| `◌` | Pending / waiting |
| `◆` | Active validator |
| `◇` | Inactive validator |
| `⇄` | HRMP bidirectional channel |
| `→` | Unidirectional message / transfer direction |
| `✓` | Step passed / assertion met |
| `✗` | Step failed / assertion violated |
| `▪` | Role indicator (validator, collator) |
| `⠶` | XCM message in transit (braille motion) |
| `♦` | Treasury / financial element |
| `⌘` | Governance / referendum |
| `⚡` | Fast finality / instant |

#### 2.10.3 CRT atmosphere and post-processing

> **Source:** `bardo/prd/18-interfaces/rendering/00-design-system.md` § "CRT
> materiality" and `crates/polkagent-cli/src/tui/theme.rs` lines 101-109

The polkagent ROSEDUST palette already defines CRT atmosphere colors that are
**not yet used in rendering**:

```rust
// theme.rs — defined but not yet rendered
pub scanline_dark: Color,   // Rgb(5, 5, 7)   #050507
pub phosphor_res: Color,    // Rgb(26, 16, 24) #1A1018
pub noise_warm: Color,      // Rgb(42, 24, 32) #2A1820
pub noise_cool: Color,      // Rgb(32, 24, 40) #201828
```

The testnet tab SHOULD activate these atmospheric layers:

**Scanline effect** — Every other row gets a subtle darkening using
`scanline_dark` as background. This creates the CRT monitor feel without
sacrificing readability. Intensity scales with testnet health:
- Healthy network: 12% scanline strength (barely visible, ambient).
- Degraded (missed blocks): 25% (the monitor is stressed).
- Chain stalled: 40% (the screen is dying with the network).

**Phosphor persistence** — When a value changes (balance, block height,
referendum status), the old value leaves a ghost rendered in `phosphor_res`.
The ghost fades over 4 frames (at 5-second refresh = 20 seconds total):
`rose_bright → rose → rose_dim → phosphor_res → gone`. This makes every
state change visible even after it happens.

**Noise floor** — A configurable fraction of background cells show dim noise
characters (`·`, `∙`, `⋅`) in `noise_warm` or `noise_cool`. Default 0.5%
of cells. Creates the impression of a live CRT receiving signal. During
network stress, noise increases to 2%.

#### 2.10.4 Animation with TachyonFX

> **Dependency:** `tachyonfx` crate — ratatui's official animation library
> (50+ shader-like effects).

The testnet TUI SHOULD use TachyonFX for transitions and state-change
animations:

| Event | TachyonFX effect | Duration |
|---|---|---|
| Test step passes | `sweep` (left→right, jade) | 300ms |
| Test step fails | `coalesce` (crimson border flash) | 500ms |
| Chain starts producing | `fade_in` on chain row | 800ms |
| Chain stalls | `dissolve` on chain status indicator | 1200ms |
| Referendum advances stage | `slide_in` new stage label | 400ms |
| Block finalized | `ping` pulse on finalized height | 200ms |
| Transfer completes | `sweep` along the transfer row | 350ms |
| XCM message arrives | `coalesce` at destination chain | 400ms |
| Network spawn complete | `parallel(fade_in, sweep)` on all chains | 1500ms |
| Era transition | `dissolve` old era → `fade_in` new era | 600ms |

These animations fire during the TUI's 5-second refresh cycle. When a
refresh detects a state change, the appropriate animation is queued and
rendered over subsequent frames. The TUI should target 15-30fps during
animations (ratatui crossterm backend can sustain this).

#### 2.10.5 Network topology visualization

> **Inspiration:** `bardo/prd/18-interfaces/rendering/02-visualization-primitives.md`
> § "ForceGraph" and ratatui `Canvas` widget with `Marker::HalfBlock`

The testnet tab MUST include a visual network topology map showing chains
and their connections. This is the centrepiece visual — the thing that makes
someone say "wow" when they see the testnet running.

```
  ┌─ Network Topology ──────────────────────────────────────────────┐
  │                                                                  │
  │                          ◆ relay chain                           │
  │                         ╱  │  │  ╲                               │
  │                        ╱   │  │   ╲                              │
  │                 ◈ asset   ◈ collectives                          │
  │                   hub    ╱       ╲                                │
  │                   │     ╱         ╲                               │
  │                   │  ◈ bridge    ◈ coretime                      │
  │                   │    hub                                       │
  │                   │                                              │
  │              ⇄────┴────⇄────────⇄                               │
  │              HRMP channels (6 open)                              │
  │                                                                  │
  │  Legend: ◆ relay  ◈ parachain  ⇄ HRMP  ─── finalized            │
  │          ● producing  ○ syncing  ✗ stalled                       │
  └──────────────────────────────────────────────────────────────────┘
```

Implementation with ratatui `Canvas`:
- Use `Canvas::default().marker(Marker::HalfBlock)` for 2× vertical resolution.
- Chains are nodes rendered as colored circles (relay = `dot_pink`,
  parachains = `parachain_violet`).
- HRMP channels are edges rendered as lines with directional arrows.
- Active message flow shown as animated dots (`⠶`) moving along edges.
- Node size/brightness scales with block production rate.
- Stalled chains dim and their edges turn `rose_dim`.

**Complementary crate:** `ratatui-plt` provides `NetworkGraph` for
force-directed graph layout. Use this for automatic node positioning when
the network has more than 5 chains.

#### 2.10.6 Visualization widget catalog for testnet

Each widget below maps to a ratatui primitive or composition:

**Block production waveform** (per chain)
```
relay  ▁▂▃▅▇█▇▅▃▂▁▁▂▃▅▇█▇▅▃▂▁▁▂▃▅▇█   6.02s avg
```
- ratatui `Sparkline` with `Marker::Braille` for sub-pixel smoothness.
- Rolling 30-block window showing block time in seconds.
- Color: `finalized_teal` for finalized blocks, `pending_amber` for pending.
- Phosphor decay: older values fade through `rose_dim → rose_deep → phosphor_res`.
- **Existing pattern:** `token_sparkline.rs` — extend with per-chain tracking.

**Finality lag oscilloscope**
```
finality  ▂▂▃▂▂▃▂▂▁▁▂▃▅▇▅▃▂▁▁▂▂▂▃▂   lag 1-3 blk
```
- Dual-trace waveform: best block height vs finalized height.
- Gap = finality lag. Color shifts `finalized_teal → pending_amber → danger`
  as lag grows.
- **Reference:** `bardo/prd/18-interfaces/rendering/02-visualization-primitives.md` §
  "WaveformDisplay" dual-trace mode.

**Referendum approval thermometer**
```
  Approval  ████████████████████████████████████░░░░  87%
  Support   ████████████████░░░░░░░░░░░░░░░░░░░░░░░░  38%
  Threshold ──────────────────────────▏               63%
```
- ratatui `Gauge` with gradient fill: `rose_deep → rose → rose_bright`.
- Threshold marker rendered as a vertical line using box-drawing `▏`.
- Approval and support update live as votes are cast.

**Treasury balance waterfall**
```
  Treasury ♦ 9,998,500 DOT
  ─────────────────────────────────────────
  ████████████████████████████████████████  burn ░░
  ├─── spends ────┤├── bounties ──┤├ burn ┤
```
- Stacked bar using block elements showing allocation breakdown.
- Animated: spend segments shrink when treasury spends execute,
  burn segment grows each spend period.

**XCM message flow**
```
  relay ────⠶──→ asset hub     teleport 100 DOT     ✓ delivered
  relay ────⠶──→ collectives   system remark         ⠶ in flight
  asset hub ←──⠶── bridge hub  reserve transfer      ✓ delivered
```
- Animated braille dots (`⠶`) move left→right along the arrow.
- Color: `parachain_violet` for XCM elements, `finalized_teal` on delivery.

**Validator unit array** (NERV-inspired)
```
  ┌──────┬──────┐
  │ ◆██▌ │ ◆██▌ │
  │alice  │ bob  │
  │ 101k  │ 101k │
  └──────┴──────┘
```
- Small repeating units showing each validator's status, self-stake, and
  total backing.
- **Inspiration:** `bardo/prd/18-interfaces/rendering/04-nerv-aesthetic.md` §
  "B1. Data as Architecture: The Repeating Unit Array"
- Active validators glow in `finalized_teal`. Slashed validators flash
  `danger` and their gauge empties.
- Scales to 50+ validators by wrapping into a grid wall.

**Event stream with phosphor decay**
```
  #128  Balances.Transfer     alice → dave  100 DOT   █ now
  #126  Staking.EraPaid       era 5  reward 100 DOT   ▓ 10s ago
  #124  Session.NewSession    session 12               ▒ 20s ago
  #122  XCM.Sent              relay → asset hub        ░ 30s ago
  #120  Referenda.Submitted   ref #1 root track        · 40s ago
```
- Each event fades through the phosphor decay chain: `█→▓→▒→░→·→ `.
- Color encodes event type: transfers in `bone`, staking in `rose`,
  governance in `dot_pink`, XCM in `parachain_violet`.
- Scroll buffer holds 100 events; visible window shows most recent 8-12.

**Test execution progress array**
```
  ┌────────┬────────┬────────┬────────┬────────┬────────┐
  │ ██████ │ ██████ │ ████░░ │ ░░░░░░ │ ░░░░░░ │ ░░░░░░ │
  │GOV-01  │PAY-01  │XCM-01  │STK-01  │MUL-01  │IDN-01  │
  │ ✓ pass │ ✓ pass │  ⠶ run │ ◌ wait │ ◌ wait │ ◌ wait │
  └────────┴────────┴────────┴────────┴────────┴────────┘
```
- NERV-inspired unit array, one cell per test scenario.
- Gauge fills as steps complete. Color: `success` for pass, `danger` for
  fail, `pending_amber` for running, `border` for waiting.
- Running test's gauge animates with a sweep effect.

#### 2.10.7 The TUI testnet experience — full-screen cinematic mode

When running `polkagent testnet watch` or the F9 Testnet tab, the full-screen
layout SHOULD look like this at 120+ column width:

```
┌──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  ⌈ Ｐ Ｏ Ｌ Ｋ Ａ Ｇ Ｅ Ｎ Ｔ   Ｔ Ｅ Ｓ Ｔ Ｎ Ｅ Ｔ ⌋                          era 6 · session 12 · 12m 34s │
├──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                                                      │
│  ┌─ Network Topology ──────────────────────┐  ┌─ Block Production ──────────────────────────────────────────────┐   │
│  │              ◆ relay                     │  │  relay       ▁▂▃▅▇█▇▅▃▂▁▁▂▃▅▇█▇▅▃▂▁▁▂▃▅▇█  #128  fin #125    │   │
│  │             ╱│╲                          │  │  asset hub   ▁▂▃▄▅▆▇█▇▆▅▄▃▂▁▁▂▃▄▅▆▇█▇▆▅▄  #122  fin #119    │   │
│  │            ╱ │ ╲                         │  │  collectives ▁▂▃▃▅▆▇▇▆▅▃▃▂▁▁▂▃▃▅▆▇▇▆▅▃▃▂  #120  fin #117    │   │
│  │     ◈ AH  ◈CO ◈BH                       │  │  bridge hub  ▁▂▃▅▆▇█▇▆▅▃▂▁▁▂▃▅▆▇█▇▆▅▃▂▁  #121  fin #118    │   │
│  │        ╲  │  ╱                           │  │  coretime    ▁▂▃▄▅▇█▇▅▄▃▂▁▁▂▃▄▅▇█▇▅▄▃▂▁  #119  fin #116    │   │
│  │         ◈ CT                             │  │                                                                │   │
│  │    ⇄──────⇄──────⇄                      │  │  finality    ▂▂▃▂▂▃▂▂▁▁▂▃▅▇▅▃▂▁▁▂▂▂▃▂▂▂  lag 1-3 blocks     │   │
│  │    6 HRMP channels                       │  └────────────────────────────────────────────────────────────────┘   │
│  └──────────────────────────────────────────┘                                                                       │
│                                                                                                                      │
│  ┌─ Governance ⌘ ──────────────────────────┐  ┌─ Events (phosphor decay) ──────────────────────────────────────┐   │
│  │  Referendum #1 · root · Deciding         │  │  #128 Balances.Transfer    alice → dave  100 DOT          █   │   │
│  │  Approval ████████████████████░░░  87%   │  │  #126 Staking.EraPaid      era 5  reward 100 DOT          ▓   │   │
│  │  Support  ████████░░░░░░░░░░░░░░░  38%   │  │  #124 Session.NewSession   session 12                    ▒   │   │
│  │  Threshold ───────────────▏        63%   │  │  #122 XCM.Sent             relay → asset hub              ░   │   │
│  │  ~3m 12s to confirmation                 │  │  #120 Referenda.Submitted  ref #1 root track              ·   │   │
│  └──────────────────────────────────────────┘  └────────────────────────────────────────────────────────────────┘   │
│                                                                                                                      │
│  ┌─ Dev Balances ♦ ────────────────────────┐  ┌─ Test Execution ───────────────────────────────────────────────┐   │
│  │  alice    999,899.995 DOT   nonce 5     │  │  ┌──────┬──────┬──────┬──────┬──────┬──────┐                   │   │
│  │  bob    1,000,000.000 DOT   nonce 2     │  │  │██████│██████│████░░│░░░░░░│░░░░░░│░░░░░░│                   │   │
│  │  charlie  499,950.000 DOT   nonce 3     │  │  │GOV-01│PAY-01│XCM-01│STK-01│MUL-01│IDN-01│                   │   │
│  │  dave     500,100.000 DOT   nonce 0     │  │  │✓ pass│✓ pass│ ⠶ run│◌ wait│◌ wait│◌ wait│                   │   │
│  │  eve      499,999.000 DOT   nonce 1     │  │  └──────┴──────┴──────┴──────┴──────┴──────┘                   │   │
│  │  ferdie   500,000.000 DOT   nonce 0     │  │  2/6 passed · 1 running · 3 pending · 0 failed               │   │
│  └──────────────────────────────────────────┘  └────────────────────────────────────────────────────────────────┘   │
│                                                                                                                      │
│  ┌─ Validators ◆ ─┬─ Treasury ♦ ──────────┐  ┌─ XCM ⠶ ─────────────────────────────────────────────────────────┐ │
│  │ ◆██▌  ◆██▌     │  9,998,500 DOT        │  │  relay ──⠶──→ asset hub     teleport 100 DOT          ✓        │ │
│  │alice   bob      │  next spend: 26 blk   │  │  relay ──⠶──→ collectives   system remark             ⠶        │ │
│  │ 101k   101k     │  burn: 1%             │  │  asset hub ←──⠶── bridge   reserve transfer           ✓        │ │
│  └─────────────────┴───────────────────────┘  └─────────────────────────────────────────────────────────────────┘ │
│                                                                                                                      │
├──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ F9 Testnet │ ● 5 chains │ ◆ 2 validators │ ⌘ ref #1 Deciding 87% │ ♦ 9.99M DOT │ ⠶ 2 XCM │ era 6 │ 12m 34s    │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

Key visual elements adapted from Bardo:

- **Fullwidth Unicode header** (`Ｐ Ｏ Ｌ Ｋ Ａ Ｇ Ｅ Ｎ Ｔ`) — institutional
  register from `bardo/prd/18-interfaces/rendering/04-nerv-aesthetic.md`. The
  fullwidth characters create a typeface effect that reads as "control room"
  rather than "text terminal".
- **Phosphor decay event stream** — adapted from Bardo's phosphor persistence
  chain (`bardo/prd/18-interfaces/rendering/00-design-system.md` § "Character
  vocabulary"). Events fade `█→▓→▒→░→·→ ` over time.
- **Waveform traces** — per-chain block production sparklines adapted from
  Bardo's `WaveformDisplay` primitive with phosphor decay on old samples.
- **Unit arrays** — validator and test execution grids adapted from Bardo's
  NERV institutional register (`04-nerv-aesthetic.md` § "B1").
- **Network topology** — force-directed graph with animated message flow,
  the centrepiece visual.
- **Dense information panels** — every pixel carries meaning. No whitespace
  for whitespace's sake. The density itself communicates "operational
  control room."

#### 2.10.8 Responsive breakpoints (matching existing TUI)

The testnet dashboard adapts to three terminal widths, matching the existing
TUI breakpoint system in `dashboard.rs`:

| Breakpoint | Width | Layout |
|---|---|---|
| **Compact** | < 80 cols | Single column, stacked panels. Topology hidden. Waveforms compressed to 20 chars. |
| **Standard** | 80-119 cols | Two columns. Topology in text mode (no canvas). Waveforms 40 chars. |
| **Wide** | ≥ 120 cols | Full cinematic layout (§ 2.10.7). Canvas topology. Waveforms 60 chars. |

At compact width, the status bar condenses to essential metrics only:
```
F9 │ ● 5 │ ref #1 87% │ 12m
```

#### 2.10.9 Color semantics for testnet visualization

All testnet rendering uses the ROSEDUST palette from `theme.rs`. No new
colors are introduced. The semantic mapping:

| Palette token | Testnet meaning |
|---|---|
| `dot_pink` | Chain identity, Polkadot-branded headers, referendum icons |
| `finalized_teal` | Finalized blocks, healthy status, delivered XCM |
| `pending_amber` | Pending blocks, in-flight XCM, running tests |
| `success` (jade) | Passed tests, completed transfers, approved referenda |
| `danger` (crimson) | Failed tests, stalled chains, rejected referenda, slashing |
| `rose` | Active elements, selection, interactive focus |
| `rose_bright` | Alerts, threshold crossings, just-changed values |
| `rose_dim` | Inactive elements, older events, secondary data |
| `bone` | The single most important number on screen (e.g., treasury balance) |
| `parachain_violet` | Parachain identifiers, XCM indicators, cross-chain elements |
| `dream` | Memory/knowledge elements, info indicators |
| `phosphor_res` | Ghost of recently-changed values (phosphor persistence) |
| `scanline_dark` | CRT scanline alternating rows |
| `noise_warm` | Background noise cells during network stress |

#### 2.10.10 Agent (Claude) visual output compatibility

The cinematic TUI is designed for human operators. For AI agents (Claude Code),
all the same information is available via `--format json`:

```bash
# Agent sees structured data, not pretty pictures
polkagent testnet status --format json
polkagent testnet query referendum 1 --format json
polkagent testnet try E2E-GOV-01 --format json
```

The `--format json` output MUST include all data that the TUI renders, so an
agent can:

1. **Verify visual claims.** If the TUI shows "Referendum #1: Deciding, 87%
   approval", the JSON contains `{"status": "Deciding", "approval": 0.87}`.
2. **Detect state changes.** The JSON includes timestamps and block heights
   for every piece of state, so agents can diff between queries.
3. **Diagnose failures.** When a test fails, the JSON includes the full
   assertion context (expected, actual, chain state snapshot).
4. **Drive automated workflows.** An agent can spawn a testnet, run tests,
   query state, fix code, and re-run — all through JSON-returning commands.

The human output and JSON output are produced from the same underlying data
structures, ensuring they never diverge.

### 2.11 `polkagent testnet watch` — the live cinematic mode

```bash
polkagent testnet watch                    # Full TUI mode, all panels
polkagent testnet watch --panel topology   # Just the network graph
polkagent testnet watch --panel events     # Just the event stream
polkagent testnet watch --panel tests      # Just the test execution grid
```

`watch` opens a full-screen ratatui application (not the main polkagent TUI,
but a standalone testnet monitor). It renders the cinematic layout from
§ 2.10.7 and refreshes continuously:

- **Block production:** updates on every new block (subscription via
  `chain_subscribeNewHeads` RPC).
- **Events:** updates on every new finalized block event.
- **Balances:** polls every 5 seconds.
- **Governance:** polls every 10 seconds.
- **Test progress:** updates in real-time when tests are running concurrently.

Keyboard controls:
- `q` — quit
- `Tab` — cycle focus between panels
- `1`-`8` — jump to specific panel
- `f` — toggle fullscreen for focused panel
- `r` — force refresh all panels
- `t` — open test runner (start `polkagent testnet go` inline)

When running inside an AI agent session (detected via `CLAUDE_CODE=1` env var
or `--non-interactive` flag), `watch` degrades gracefully to periodic text
snapshots every 5 seconds, printed to stdout, parseable by the agent.

---

## 3. Local testnet architecture

> **For the impatient:** You don't need to read this section. Just run
> `polkagent testnet go` and everything below is handled for you.

### 3.1 Tooling selection

**Primary tool: Zombienet v2 (with Rust SDK)**

Zombienet is Parity's official tool for spawning ephemeral Polkadot test
networks. It supports relay chains and parachains, configurable genesis state,
a test DSL (`.zndsl`) for assertions, and a **Rust SDK (`zombienet-sdk`)**
for programmatic network control from within polkagent's Rust codebase.

| Capability | Zombienet CLI | Zombienet Rust SDK | Chopsticks |
|---|---|---|---|
| Multi-chain topology | Yes | Yes | Yes (multi-fork) |
| Production-like consensus | Yes (BABE/GRANDPA) | Yes | No (mock blocks) |
| Real block production | Yes | Yes | No (`dev_newBlock`) |
| Live chain state fork | No | No | Yes |
| HRMP/XCM channels | Yes | Yes | Yes (XCM simulation) |
| Programmatic node access | Via parsed output | Via `NetworkNode` API | Via RPC |
| Test DSL | `.zndsl` | Rust assertions | None |
| Startup time | 30-60s | 30-60s | 5-10s |
| CI integration | Excellent | Excellent | Good |
| Agent workflow | Good (CLI) | Best (typed API) | Good (fast) |

**Decision:** Use the Zombienet **Rust SDK** as the primary programmatic
interface for E2E tests and the `polkagent testnet` CLI subcommand. Use the
Zombienet **CLI** for standalone interactive sessions. Use Chopsticks as a
complementary tool for fast iteration and state forking.

**Secondary tool: Chopsticks**

Chopsticks forks live chain state locally and provides `dev_setStorage`,
`dev_newBlock`, and `dev_setHead` RPCs for deterministic testing. It is
valuable for:

- Rapid prototyping of governance/treasury scenarios against real mainnet state.
- Testing polkagent against exact production storage layouts.
- XCM dry-run simulation without full relay chain overhead.

### 3.2 Network topology

The local testnet MUST mirror the production Polkadot relay chain plus all
system parachains that polkagent interacts with:

```
┌──────────────────────────────────────────────────────────────────────┐
│                        RELAY CHAIN                                   │
│                     polkadot-local                                   │
│            (2 validators: alice, bob)                                │
│                                                                      │
│  Pallets: System, Balances, Staking, Session, Babe, Grandpa,        │
│           Treasury, ConvictionVoting, Referenda, Preimage,           │
│           Bounties, ChildBounties, Tips, Scheduler, Utility,         │
│           Multisig, Proxy, Identity, Registrar, Slots, Auctions,     │
│           NominationPools, ElectionProviderMultiPhase, Vesting       │
└────────┬───────────┬──────────────┬───────────────┬─────────────────┘
         │           │              │               │
    HRMP ↕      HRMP ↕         HRMP ↕          HRMP ↕
         │           │              │               │
┌────────▼──┐ ┌──────▼────┐ ┌──────▼──────┐ ┌──────▼──────┐
│ Asset Hub  │ │Collectives│ │ Bridge Hub  │ │ Coretime    │
│ Para 1000  │ │ Para 1001 │ │ Para 1002   │ │ Para 1005   │
│            │ │           │ │             │ │             │
│ Assets     │ │ Fellowship│ │ Snowbridge  │ │ Broker      │
│ NFTs       │ │ Alliance  │ │ BridgeRelay │ │ OnDemand    │
│ AssetConv  │ │ Salary    │ │             │ │             │
│ ForeignAst │ │ Treasury  │ │             │ │             │
│ Contracts  │ │           │ │             │ │             │
└────────────┘ └───────────┘ └─────────────┘ └─────────────┘
```

**Note on People Chain (Para 1004):** The People Chain for identity management
is included in production Polkadot but MAY be omitted from initial testnet
setup if polkagent's identity tools are not yet targeting it. When included,
it adds the Identity, IdentityMigrator, and People pallets.

### 3.3 Zombienet configuration

The master Zombienet TOML configuration lives at:
```
polkagent/testnet/zombienet.toml
```

```toml
[settings]
timeout = 120
provider = "native"

[relaychain]
chain = "polkadot-local"
default_command = "polkadot"
default_args = ["-lparachain=debug,xcm=trace"]

  [[relaychain.nodes]]
  name = "alice"
  validator = true
  args = ["--alice"]
  ws_port = 9944
  rpc_port = 9933
  prometheus_port = 9615

  [[relaychain.nodes]]
  name = "bob"
  validator = true
  args = ["--bob"]
  ws_port = 9954
  rpc_port = 9943
  prometheus_port = 9625

[[parachains]]
id = 1000
chain = "asset-hub-polkadot-local"
cumulus_based = true

  [[parachains.collators]]
  name = "asset-hub-collator-01"
  command = "polkadot-parachain"
  args = ["--alice"]
  ws_port = 9964
  rpc_port = 9953

[[parachains]]
id = 1001
chain = "collectives-polkadot-local"
cumulus_based = true

  [[parachains.collators]]
  name = "collectives-collator-01"
  command = "polkadot-parachain"
  args = ["--alice"]
  ws_port = 9974
  rpc_port = 9963

[[parachains]]
id = 1002
chain = "bridge-hub-polkadot-local"
cumulus_based = true

  [[parachains.collators]]
  name = "bridge-hub-collator-01"
  command = "polkadot-parachain"
  args = ["--alice"]
  ws_port = 9984
  rpc_port = 9973

[[parachains]]
id = 1005
chain = "coretime-polkadot-local"
cumulus_based = true

  [[parachains.collators]]
  name = "coretime-collator-01"
  command = "polkadot-parachain"
  args = ["--alice"]
  ws_port = 9994
  rpc_port = 9983

[[hrmp_channels]]
sender = 1000
recipient = 1001
max_capacity = 8
max_message_size = 8192

[[hrmp_channels]]
sender = 1001
recipient = 1000
max_capacity = 8
max_message_size = 8192

[[hrmp_channels]]
sender = 1000
recipient = 1002
max_capacity = 8
max_message_size = 8192

[[hrmp_channels]]
sender = 1002
recipient = 1000
max_capacity = 8
max_message_size = 8192

[[hrmp_channels]]
sender = 1000
recipient = 1005
max_capacity = 8
max_message_size = 8192

[[hrmp_channels]]
sender = 1005
recipient = 1000
max_capacity = 8
max_message_size = 8192
```

Port numbers are **pinned** (not dynamically assigned) to enable deterministic
connection from polkagent and diagnostic scripts without parsing spawn output.

### 3.4 Binary requirements

| Binary | Source | Purpose |
|---|---|---|
| `polkadot` | `polkadot-sdk` release or nix | Relay chain validator node |
| `polkadot-parachain` | `polkadot-sdk` release or nix | System parachain collator |
| `chain-spec-builder` | `polkadot-sdk` release or nix | Custom chain spec generation |
| `zombienet` | npm / GitHub releases | Network orchestration (CLI) |
| `chopsticks` | npm (`@acala-network/chopsticks`) | State fork testing |
| `subxt` | `cargo install subxt-cli` | Runtime exploration (`subxt explore`) |
| `subkey` | `polkadot-sdk` or cargo install | Key inspection and signing |
| `polkadot-js-api` | npm (`@polkadot/api-cli`) | Quick chain queries from CLI |

Binaries SHOULD be pinned to a specific polkadot-sdk release (e.g.,
`polkadot-stable2407` or later) in `testnet/POLKADOT_VERSION` to ensure
reproducible genesis state and runtime behavior.

### 3.5 Provider configuration

With pinned ports, the polkagent `ChainProfile` configuration for the local
testnet is static and checked into the repository:

```toml
# testnet/polkagent-local.toml
# Auto-generated chain config for local testnet — genesis hashes filled at spawn

[chains.polkadot-local]
network_type = "local"
rpc_endpoints = ["http://127.0.0.1:9933"]

[chains.asset-hub-local]
network_type = "local"
rpc_endpoints = ["http://127.0.0.1:9953"]

[chains.collectives-local]
network_type = "local"
rpc_endpoints = ["http://127.0.0.1:9963"]

[chains.bridge-hub-local]
network_type = "local"
rpc_endpoints = ["http://127.0.0.1:9973"]

[chains.coretime-local]
network_type = "local"
rpc_endpoints = ["http://127.0.0.1:9983"]
```

A helper script fills in actual genesis hashes after spawn (§ 6.3).

---

## 4. Genesis state configuration

The testnet MUST be pre-configured at genesis to provide a realistic starting
state that enables immediate test execution without lengthy setup sequences.

### 4.1 Development accounts

Polkadot development accounts are derived from the well-known seed:
`bottom drive obey lake curtain smoke basket hold race lonely fit walk`

| Account | SS58 Address | Role in tests | Initial balance |
|---|---|---|---|
| Alice | `5GrwvaEF...` | Validator, sudo, governance proposer, treasury curator | 1,000,000 DOT |
| Bob | `5FHneW46...` | Validator, voter, nominator, multisig co-signer | 1,000,000 DOT |
| Charlie | `5FLSigC9...` | Delegator, bounty proposer, proxy controller | 500,000 DOT |
| Dave | `5DAAnrj7...` | Payment recipient, identity registrar, fellowship member | 500,000 DOT |
| Eve | `5HGjWAeF...` | Tipper, NFT creator, asset issuer | 500,000 DOT |
| Ferdie | `5CiPPseX...` | Multisig co-signer, pure proxy creator | 500,000 DOT |

### 5.2 Shortened governance parameters

Production Polkadot governance tracks have decision periods of 14-28 days,
which are impractical for testing. The genesis config MUST override track
parameters to enable governance lifecycles to complete within minutes:

```json
{
  "referenda": {
    "tracks": [
      {
        "id": 0,
        "name": "root",
        "max_deciding": 1,
        "decision_deposit": "1000000000000",
        "prepare_period": 2,
        "decision_period": 50,
        "confirm_period": 10,
        "min_enactment_period": 5,
        "min_approval": { "linearDecreasing": { "length": 1000000000, "floor": 500000000, "ceil": 1000000000 } },
        "min_support": { "linearDecreasing": { "length": 1000000000, "floor": 0, "ceil": 500000000 } }
      },
      {
        "id": 1,
        "name": "whitelisted_caller",
        "max_deciding": 10,
        "decision_deposit": "100000000000",
        "prepare_period": 1,
        "decision_period": 30,
        "confirm_period": 5,
        "min_enactment_period": 2
      },
      {
        "id": 10,
        "name": "treasurer",
        "max_deciding": 5,
        "decision_deposit": "10000000000",
        "prepare_period": 1,
        "decision_period": 30,
        "confirm_period": 5,
        "min_enactment_period": 2
      },
      {
        "id": 30, "name": "small_tipper",
        "max_deciding": 15, "decision_deposit": "1000000000",
        "prepare_period": 1, "decision_period": 10,
        "confirm_period": 3, "min_enactment_period": 1
      },
      {
        "id": 31, "name": "big_tipper",
        "max_deciding": 10, "decision_deposit": "5000000000",
        "prepare_period": 1, "decision_period": 15,
        "confirm_period": 5, "min_enactment_period": 1
      },
      {
        "id": 32, "name": "small_spender",
        "max_deciding": 10, "decision_deposit": "10000000000",
        "prepare_period": 1, "decision_period": 20,
        "confirm_period": 5, "min_enactment_period": 2
      },
      {
        "id": 33, "name": "medium_spender",
        "max_deciding": 5, "decision_deposit": "50000000000",
        "prepare_period": 1, "decision_period": 25,
        "confirm_period": 5, "min_enactment_period": 2
      },
      {
        "id": 34, "name": "big_spender",
        "max_deciding": 5, "decision_deposit": "100000000000",
        "prepare_period": 2, "decision_period": 30,
        "confirm_period": 5, "min_enactment_period": 3
      }
    ]
  }
}
```

All periods are specified in **blocks**. With a 6-second block time, a
50-block decision period completes in 5 minutes.

### 5.3 Shortened staking parameters

| Parameter | Production | Local testnet |
|---|---|---|
| Era duration | 24 hours (14400 blocks) | 2 minutes (20 blocks) |
| Session length | 4 hours (2400 blocks) | 1 minute (10 blocks) |
| Sessions per era | 6 | 2 |
| Bonding duration | 28 eras (28 days) | 4 eras (8 minutes) |
| Slash defer duration | 27 eras | 2 eras |
| History depth | 84 eras | 10 eras |
| Max nominations | 16 | 16 |
| Min nominator bond | 250 DOT | 1 DOT |

### 5.4 Shortened treasury parameters

| Parameter | Production | Local testnet |
|---|---|---|
| Spend period | 24 days (345600 blocks) | 5 minutes (50 blocks) |
| Burn rate | 1% of remainder | 1% of remainder |
| Bounty deposit base | 1 DOT | 0.1 DOT |
| Bounty update period | 90 days | 5 minutes (50 blocks) |
| Tip countdown | 1 day | 1 minute (10 blocks) |

### 5.5 Pre-seeded state

The genesis config MUST pre-seed the following state for immediate test
readiness:

#### 3.5.1 Staking

- Alice and Bob registered as validators with session keys.
- Charlie and Dave nominated to Alice and Bob respectively.
- One nomination pool created (pool ID 1) with Bob as depositor, 100 DOT.

#### 3.5.2 Treasury

- Treasury pre-funded with 10,000,000 DOT via genesis balance.
- One active bounty (bounty ID 0): "Agent Testing Bounty", 1000 DOT, curator = Dave.
- One child bounty under bounty 0: 100 DOT, pending assignment.

#### 3.5.3 Identity (relay chain)

- Dave registered as identity registrar (registrar index 0), fee = 0.1 DOT.
- Alice has full identity set (display, legal, web, email, twitter) with
  Dave's "Reasonable" judgement.
- Bob has identity set without judgement (pending).

#### 3.5.4 Governance

- One preimage uploaded: `System.remark("polkagent-test-genesis")`.
- One referendum on Root track in Preparing state, using the above preimage.

#### 3.5.5 Multisig / Proxy

- A 2-of-3 multisig composed of (Alice, Bob, Charlie) with threshold 2.
  Deterministic address computed from sorted account IDs.
- A pure proxy owned by Ferdie with proxy type `Any`.
- A time-delayed proxy: Eve delegates `Governance` proxy to Alice with
  delay = 5 blocks.

#### 3.5.6 Asset Hub (Para 1000)

- Sufficient asset created: asset ID 1337, name "TestUSD", symbol "TUSD",
  decimals 6, admin = Eve, min balance = 1000 (0.001 TUSD).
- Eve mints 1,000,000 TUSD to herself, transfers 100,000 TUSD each to
  Alice, Bob, Charlie, Dave.
- NFT collection created: collection ID 1, owner = Eve, settings open.
- NFTs minted: IDs 1-5 in collection 1, owned by Eve.

#### 3.5.7 Collectives (Para 1001)

- Dave inducted into Technical Fellowship at rank 1 (I Dan).
- Fellowship salary period configured (shortened to 5 minutes).

#### 3.5.8 Vesting

- Charlie has a vesting schedule: 100,000 DOT over 200 blocks
  (locked = 100,000 DOT, per_block = 500 DOT, starting_block = 0).

---

## 5. Agent-driven development workflow

This section defines the interactive development experience for AI coding
agents (Claude Code) and human developers. The goal: **a developer or agent
can go from zero to running a test against a live chain in under 2 minutes,
and from a test failure to understanding the root cause in under 30 seconds.**

### 5.1 The `polkagent testnet` CLI subcommand

A new `testnet` subcommand group is added to the polkagent CLI (extending the
existing `network` command group or as a sibling). All commands support the
global `--format json` flag for structured output that agents can parse.

#### 4.1.1 Command reference

```
polkagent testnet
  up              Spawn local testnet (idempotent — reuses if already running)
  down            Tear down running testnet
  status          Show testnet status, RPC endpoints, block heights, peer counts
  check           Verify prerequisites (binaries, ports, disk space)
  reset           Tear down and re-spawn with fresh genesis state
  logs            Tail logs from one or all nodes
  config          Generate polkagent chain config from running testnet
  exec            Submit a raw extrinsic to a testnet chain
  query           Query chain state (balances, staking, governance, identity, etc.)
  wait            Wait for a condition (block height, finality, era, referendum state)
  scenario        Run a specific E2E scenario by ID
  seed            Execute post-genesis seeding (for state that cannot be set at genesis)
  snapshot        Save/restore chain state for reproducible debugging
```

#### 4.1.2 `polkagent testnet up`

Spawns the local testnet. Idempotent — if a testnet is already running
(detected via pidfile + port probe), prints connection info and exits.

```bash
# Interactive (human)
$ polkagent testnet up
Spawning local testnet...
  Relay chain:   ws://127.0.0.1:9944  (alice, bob)
  Asset Hub:     ws://127.0.0.1:9964  (para 1000)
  Collectives:   ws://127.0.0.1:9974  (para 1001)
  Bridge Hub:    ws://127.0.0.1:9984  (para 1002)
  Coretime:      ws://127.0.0.1:9994  (para 1005)
  Genesis hashes written to testnet/genesis-hashes.json
  Polkagent config written to testnet/polkagent-local.toml
Testnet ready. All chains producing blocks.

# Structured (for agents)
$ polkagent testnet up --format json
{
  "status": "ready",
  "chains": {
    "relay": {
      "name": "polkadot-local",
      "ws_endpoint": "ws://127.0.0.1:9944",
      "rpc_endpoint": "http://127.0.0.1:9933",
      "prometheus": "http://127.0.0.1:9615",
      "genesis_hash": "0xabc123...",
      "block_height": 4,
      "validators": ["alice", "bob"]
    },
    "asset_hub": {
      "name": "asset-hub-polkadot-local",
      "para_id": 1000,
      "ws_endpoint": "ws://127.0.0.1:9964",
      "rpc_endpoint": "http://127.0.0.1:9953",
      "genesis_hash": "0xdef456...",
      "block_height": 2
    },
    "collectives": { ... },
    "bridge_hub": { ... },
    "coretime": { ... }
  },
  "config_path": "testnet/polkagent-local.toml",
  "pid_file": "testnet/.zombienet.pid",
  "log_dir": "testnet/logs/"
}
```

**Flags:**
- `--relay-only` — Spawn only the relay chain (fastest startup, ~15s).
- `--with-parachains 1000,1001` — Select specific parachains.
- `--dir <path>` — Persistent directory (chain state preserved across restarts).
- `--background` — Daemonize (write PID file, return immediately).
- `--wait-blocks <n>` — Wait until all chains have produced N blocks before
  returning (default: 2).
- `--timeout <secs>` — Spawn timeout (default: 120).

#### 4.1.3 `polkagent testnet status`

Quick health check of the running testnet. Designed for an agent to call
before running tests to verify the network is healthy.

```bash
$ polkagent testnet status --format json
{
  "running": true,
  "uptime_secs": 342,
  "chains": {
    "relay": {
      "best_block": 57,
      "finalized_block": 54,
      "peers": 1,
      "producing_blocks": true,
      "era": 2,
      "session": 5,
      "epoch": 5
    },
    "asset_hub": {
      "best_block": 48,
      "finalized_block": 45,
      "peers": 0,
      "producing_blocks": true,
      "para_included": true
    },
    ...
  },
  "hrmp_channels": [
    { "sender": 1000, "recipient": 1001, "open": true },
    { "sender": 1001, "recipient": 1000, "open": true },
    ...
  ]
}
```

#### 4.1.4 `polkagent testnet check`

Pre-flight check before spawning. Verifies all prerequisites are met.

```bash
$ polkagent testnet check --format json
{
  "ready": false,
  "checks": [
    { "name": "polkadot binary", "status": "ok", "path": "/usr/local/bin/polkadot", "version": "1.16.0" },
    { "name": "polkadot-parachain binary", "status": "ok", "path": "/usr/local/bin/polkadot-parachain", "version": "1.16.0" },
    { "name": "zombienet binary", "status": "missing", "install": "npm i -g @nickliu/zombienet" },
    { "name": "port 9944", "status": "ok", "available": true },
    { "name": "port 9964", "status": "conflict", "pid": 12345, "process": "polkadot" },
    { "name": "disk space", "status": "ok", "available_gb": 42.3, "required_gb": 10 },
    { "name": "polkadot-sdk version", "status": "ok", "pinned": "stable2407", "actual": "stable2407" }
  ]
}
```

### 5.2 Chain state inspection commands

These commands let an agent or developer query live chain state for debugging,
without writing Rust code or opening a browser.

#### 4.2.1 `polkagent testnet query`

General-purpose chain state query with subcommands for each domain:

```bash
# Balance query
$ polkagent testnet query balance --chain relay --account alice --format json
{
  "account": "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
  "free": "999999950000000000",
  "free_dot": "99999.995",
  "reserved": "1010000000000",
  "reserved_dot": "0.101",
  "frozen": "500000000000000000",
  "frozen_dot": "50000.0",
  "transferable": "499999950000000000",
  "transferable_dot": "49999.995",
  "nonce": 3,
  "exists": true,
  "at_block": 57
}

# All dev account balances at once
$ polkagent testnet query balance --chain relay --all-dev --format json
{
  "at_block": 57,
  "accounts": {
    "alice": { "free_dot": "99999.995", "nonce": 3 },
    "bob":   { "free_dot": "100000.0",  "nonce": 0 },
    "charlie": { "free_dot": "50000.0", "nonce": 1 },
    ...
  }
}

# Governance: list referenda
$ polkagent testnet query referenda --chain relay --format json
{
  "at_block": 57,
  "referenda": [
    {
      "index": 0,
      "track": "root",
      "status": "Preparing",
      "proposal_hash": "0xabc...",
      "submitted_at": 1,
      "decision_deposit": { "who": null, "amount": "1000000000000" },
      "tally": { "ayes": "0", "nays": "0", "support": "0" }
    }
  ]
}

# Governance: specific referendum detail
$ polkagent testnet query referendum --chain relay --index 0 --format json
{
  "index": 0,
  "track": { "id": 0, "name": "root", "decision_period": 50, "confirm_period": 10 },
  "status": "Deciding",
  "tally": { "ayes": "1000000000000000000", "nays": "0", "support": "0.5" },
  "deciding_since": 3,
  "blocks_remaining": 42,
  "time_remaining_secs": 252,
  "confirming": false
}

# Staking overview
$ polkagent testnet query staking --chain relay --format json
{
  "era": 3,
  "session": 7,
  "validators": ["alice", "bob"],
  "nominators": {
    "charlie": { "targets": ["alice"], "bonded_dot": "1000.0" },
    "dave": { "targets": ["bob"], "bonded_dot": "1000.0" }
  },
  "pools": [
    { "id": 1, "depositor": "bob", "total_dot": "150.0", "members": 2 }
  ],
  "era_payout_dot": "100.0"
}

# Treasury
$ polkagent testnet query treasury --chain relay --format json
{
  "balance_dot": "9998500.0",
  "next_spend_at_block": 100,
  "blocks_until_spend": 43,
  "active_bounties": 1,
  "pending_proposals": 0,
  "burn_rate": 0.01
}

# Identity
$ polkagent testnet query identity --chain relay --account alice --format json
{
  "display": "Alice Validator",
  "legal": "Alice Corp",
  "web": "https://alice.example.com",
  "email": "alice@example.com",
  "twitter": "@alice_validator",
  "judgements": [{ "registrar": 0, "judgement": "Reasonable" }],
  "deposit_dot": "1.0"
}

# Asset Hub assets
$ polkagent testnet query assets --chain asset-hub --format json
{
  "assets": [
    {
      "id": 1337,
      "name": "TestUSD",
      "symbol": "TUSD",
      "decimals": 6,
      "admin": "eve",
      "total_supply": "1000000000000",
      "holders": 5,
      "is_sufficient": true
    }
  ]
}

# XCM: HRMP channel status
$ polkagent testnet query hrmp --chain relay --format json
{
  "channels": [
    { "sender": 1000, "recipient": 1001, "capacity": 8, "used": 0, "msg_size": 8192 },
    { "sender": 1001, "recipient": 1000, "capacity": 8, "used": 0, "msg_size": 8192 },
    ...
  ]
}

# Raw storage query (for anything not covered by named queries)
$ polkagent testnet query storage --chain relay \
    --pallet System --entry Account \
    --key "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY" \
    --format json
{
  "pallet": "System",
  "entry": "Account",
  "value": { "nonce": 3, "consumers": 2, "providers": 1, "sufficients": 0, "data": { ... } },
  "at_block": 57
}
```

#### 4.2.2 `polkagent testnet query` implementation

Under the hood, `testnet query` commands connect to the local node via
`polkagent-chain-subxt` (the same client used in production) and use the
dynamic subxt API to query storage without compile-time codegen. This ensures
the query path exercises the same code polkagent uses at runtime.

For named queries (balance, referenda, staking, etc.), the command knows the
storage key structure. For `storage` (raw), the command uses
`subxt explore`-style metadata introspection to locate the storage entry and
decode the value.

All outputs include the `at_block` field so the agent can correlate state
with specific events or test steps.

### 5.3 Extrinsic submission for debugging

#### 4.3.1 `polkagent testnet exec`

Submit extrinsics directly from the CLI for setup, debugging, or manual
test steps. Uses dev account signing (sr25519 via `subxt-signer`).

```bash
# Transfer
$ polkagent testnet exec --chain relay --signer alice \
    transfer --to dave --amount 100.0 --format json
{
  "extrinsic": "Balances.transfer_allow_death",
  "tx_hash": "0xabc123...",
  "status": "finalized",
  "block_hash": "0xdef456...",
  "block_number": 58,
  "extrinsic_index": 1,
  "fee_dot": "0.0123",
  "events": [
    { "pallet": "Balances", "event": "Transfer", "from": "alice", "to": "dave", "amount_dot": "100.0" },
    { "pallet": "Balances", "event": "Deposit", "who": "alice_treasury", "amount_dot": "0.0098" },
    { "pallet": "System", "event": "ExtrinsicSuccess" }
  ]
}

# Vote on a referendum
$ polkagent testnet exec --chain relay --signer alice \
    vote --referendum 0 --aye --conviction 1 --balance 500000 --format json

# Submit preimage
$ polkagent testnet exec --chain relay --signer alice \
    submit-preimage --call "System.remark" --args '"hello"' --format json

# Submit referendum
$ polkagent testnet exec --chain relay --signer alice \
    submit-referendum --track root --preimage-hash 0xabc... --format json

# Place decision deposit
$ polkagent testnet exec --chain relay --signer alice \
    decision-deposit --referendum 1 --format json

# Delegate voting
$ polkagent testnet exec --chain relay --signer charlie \
    delegate --track root --to alice --conviction 1 --balance 100000 --format json

# XCM teleport
$ polkagent testnet exec --chain relay --signer alice \
    teleport --dest asset-hub --amount 100.0 --format json

# Bond and nominate
$ polkagent testnet exec --chain relay --signer dave \
    bond --amount 1000.0 --format json
$ polkagent testnet exec --chain relay --signer dave \
    nominate --targets alice --format json

# Set identity
$ polkagent testnet exec --chain relay --signer bob \
    set-identity --display "Bob Validator" --email "bob@example.com" --format json

# Multisig approve
$ polkagent testnet exec --chain relay --signer bob \
    multisig-approve --call-hash 0xabc... --other-signatories alice,charlie \
    --threshold 2 --format json

# Batch call
$ polkagent testnet exec --chain relay --signer alice \
    batch --calls '["Balances.transfer(bob,10)","Balances.transfer(charlie,10)"]' \
    --format json

# Raw call (for anything not covered by named extrinsics)
$ polkagent testnet exec --chain relay --signer alice \
    raw --pallet Balances --call transfer_allow_death \
    --args '{"dest": "5FHneW46...", "value": 10000000000000}' --format json
```

**Every `exec` response includes:**
- `tx_hash` — For correlation with on-chain records.
- `status` — `"finalized"`, `"in_block"`, `"dropped"`, or `"invalid"`.
- `block_number` and `block_hash` — Where the extrinsic landed.
- `fee_dot` — Actual fee charged.
- `events` — All events emitted by the extrinsic, decoded.
- `error` (if failed) — Decoded dispatch error with module name and error name.

#### 4.3.2 Error output for failed extrinsics

When an extrinsic fails, the output MUST be diagnostic-rich:

```json
{
  "extrinsic": "Balances.transfer_allow_death",
  "tx_hash": "0xabc123...",
  "status": "finalized",
  "block_number": 58,
  "success": false,
  "dispatch_error": {
    "module": "Balances",
    "error": "InsufficientBalance",
    "docs": "Balance too low to send value.",
    "context": {
      "free_balance_dot": "0.5",
      "transfer_amount_dot": "100.0",
      "existential_deposit_dot": "1.0"
    }
  },
  "events": [
    { "pallet": "System", "event": "ExtrinsicFailed", "error": "Balances.InsufficientBalance" }
  ]
}
```

### 5.4 Wait conditions

#### 4.4.1 `polkagent testnet wait`

Block until a chain condition is met. Essential for test scripts and agent
workflows where you need to wait for async chain operations.

```bash
# Wait for N blocks
$ polkagent testnet wait --chain relay --blocks 10 --format json
{ "waited_blocks": 10, "final_block": 67, "elapsed_secs": 60 }

# Wait for finality of a specific block
$ polkagent testnet wait --chain relay --finalized 58 --format json
{ "finalized": true, "block": 58, "elapsed_secs": 12 }

# Wait for an era transition
$ polkagent testnet wait --chain relay --next-era --format json
{ "previous_era": 2, "current_era": 3, "elapsed_secs": 45 }

# Wait for a referendum to reach a specific state
$ polkagent testnet wait --chain relay --referendum 0 --state Approved --timeout 600 --format json
{ "referendum": 0, "state": "Approved", "elapsed_secs": 312 }

# Wait for a treasury spend period
$ polkagent testnet wait --chain relay --next-spend-period --format json
{ "spend_period_at_block": 100, "elapsed_secs": 180 }

# Wait for XCM delivery (monitor destination chain for a specific event)
$ polkagent testnet wait --chain asset-hub --event "Balances.Deposit" \
    --from-block 45 --timeout 60 --format json
{ "found": true, "block": 47, "event": { "pallet": "Balances", "event": "Deposit", ... } }

# Wait for block height
$ polkagent testnet wait --chain relay --block-height 100 --format json
{ "block_height": 100, "elapsed_secs": 180 }
```

### 5.5 Log and event inspection

#### 4.5.1 `polkagent testnet logs`

Tail or search node logs. Logs are captured from Zombienet node processes.

```bash
# Tail all relay chain logs (human-readable, for interactive debugging)
$ polkagent testnet logs --chain relay --follow

# Tail with filtering (grep-like)
$ polkagent testnet logs --chain relay --follow --filter "xcm"

# Search historical logs for errors
$ polkagent testnet logs --chain relay --level error --format json
{
  "entries": [
    {
      "timestamp": "2026-08-03T12:34:56Z",
      "level": "ERROR",
      "target": "xcm::executor",
      "message": "XCM execution failed: TooExpensive",
      "node": "alice",
      "block": 42
    }
  ]
}

# Dump events from a specific block range
$ polkagent testnet logs --chain relay --events --blocks 50-60 --format json
{
  "blocks": [
    {
      "number": 50,
      "events": [
        { "pallet": "Staking", "event": "EraPaid", "era": 2, "reward_dot": "100.0" },
        { "pallet": "Session", "event": "NewSession", "session": 5 }
      ]
    },
    ...
  ]
}
```

#### 4.5.2 Polkagent outbox event correlation

When running polkagent tools against the testnet, the polkagent event log
(`~/.polkagent/logs/events.jsonl`) can be correlated with chain events:

```bash
# Show polkagent effect lifecycle events from the last run
$ polkagent logs --last-run --format json
{
  "run_id": "run_abc123",
  "effects": [
    {
      "type": "EffectIntent",
      "timestamp": "2026-08-03T12:34:50Z",
      "intent_id": "eff_001",
      "call": "Balances.transfer_allow_death",
      "chain": "polkadot-local"
    },
    {
      "type": "EffectAttempt",
      "timestamp": "2026-08-03T12:34:51Z",
      "intent_id": "eff_001",
      "tx_hash": "0xabc..."
    },
    {
      "type": "EffectOutcome",
      "timestamp": "2026-08-03T12:34:57Z",
      "intent_id": "eff_001",
      "tx_hash": "0xabc...",
      "finalized": true,
      "block": 58
    }
  ]
}
```

### 5.6 Scenario runner with granular control

#### 4.6.1 `polkagent testnet scenario`

Run individual E2E scenarios or scenario groups, with detailed step-by-step
output for debugging.

```bash
# List all available scenarios
$ polkagent testnet scenario list --format json
{
  "scenarios": [
    { "id": "E2E-GOV-01", "name": "Referendum lifecycle", "domain": "governance", "phase": 3 },
    { "id": "E2E-TRES-01", "name": "Balance query and transfer", "domain": "treasury", "phase": 1 },
    ...
  ]
}

# Run a single scenario with verbose step output
$ polkagent testnet scenario run E2E-TRES-01 --verbose --format json
{
  "scenario": "E2E-TRES-01",
  "name": "Balance query and transfer history",
  "status": "passed",
  "duration_secs": 18.5,
  "steps": [
    {
      "step": 1,
      "description": "Query Alice balance via balance_query tool",
      "status": "passed",
      "duration_ms": 120,
      "result": {
        "free_dot": "1000000.0",
        "reserved_dot": "0.0"
      }
    },
    {
      "step": 2,
      "description": "Alice transfers 100 DOT to Dave",
      "status": "passed",
      "duration_ms": 6200,
      "result": {
        "tx_hash": "0xabc...",
        "block": 58,
        "fee_dot": "0.0123"
      }
    },
    {
      "step": 3,
      "description": "Verify Alice balance decreased",
      "status": "passed",
      "duration_ms": 95,
      "assertion": "alice.free == 999899.9877 DOT",
      "actual": "999899.9877"
    },
    {
      "step": 4,
      "description": "Verify Dave balance increased",
      "status": "passed",
      "duration_ms": 88,
      "assertion": "dave.free == 500100.0 DOT",
      "actual": "500100.0"
    },
    {
      "step": 5,
      "description": "Query transfer_history for Alice",
      "status": "passed",
      "duration_ms": 150,
      "result": {
        "transfers": [
          { "block": 58, "to": "dave", "amount_dot": "100.0", "fee_dot": "0.0123" }
        ]
      }
    }
  ],
  "invariants_checked": ["INV-01", "INV-02", "INV-03", "INV-04"],
  "invariant_violations": []
}

# Run a domain group
$ polkagent testnet scenario run --domain governance --format json

# Run all scenarios for a specific phase
$ polkagent testnet scenario run --phase 1 --format json

# Run a scenario and stop at first failure (for debugging)
$ polkagent testnet scenario run E2E-GOV-01 --stop-on-failure --verbose --format json
{
  "scenario": "E2E-GOV-01",
  "status": "failed",
  "failed_at_step": 9,
  "steps": [
    ...
    {
      "step": 9,
      "description": "Verify referendum is in Deciding state",
      "status": "failed",
      "duration_ms": 105,
      "assertion": "referendum.status == Deciding",
      "actual": "Preparing",
      "error": "Referendum 1 is still in Preparing state. Prepare period is 2 blocks but only 1 block has passed since submission.",
      "diagnostic": {
        "referendum_submitted_at_block": 57,
        "current_block": 58,
        "prepare_period_blocks": 2,
        "blocks_remaining": 1
      },
      "chain_state_at_failure": {
        "relay_best_block": 58,
        "relay_finalized_block": 55,
        "referendum_0": { "status": "Preparing", "submitted_at": 57 }
      }
    }
  ]
}
```

#### 4.6.2 Scenario failure diagnostics

When a scenario step fails, the output MUST include:

1. **What was expected** — The assertion that failed.
2. **What was actual** — The value that was observed.
3. **Why it likely failed** — A human-readable diagnostic message.
4. **Relevant chain state** — Snapshot of related state at the failure point.
5. **Timing info** — Block numbers and timestamps for correlation with logs.
6. **Suggested fix** — If the failure pattern is known (e.g., "need to wait
   for N more blocks", "insufficient balance", "wrong track").

This diagnostic richness is critical for agent workflows: the agent reads the
JSON failure output and immediately knows what to investigate or fix.

### 5.7 Snapshot and restore

#### 4.7.1 `polkagent testnet snapshot`

Save and restore chain state for reproducible debugging sessions.

```bash
# Save current state
$ polkagent testnet snapshot save --name "pre-governance-test" --format json
{
  "name": "pre-governance-test",
  "path": "testnet/snapshots/pre-governance-test/",
  "relay_block": 57,
  "asset_hub_block": 48,
  "size_mb": 125.4,
  "created_at": "2026-08-03T12:35:00Z"
}

# List snapshots
$ polkagent testnet snapshot list --format json

# Restore a snapshot (tears down current network, restarts with saved state)
$ polkagent testnet snapshot restore --name "pre-governance-test" --format json
```

This enables the agent workflow: "I found a bug, let me snapshot the state,
fix the code, restore the snapshot, and re-run the failing scenario."

### 5.8 Interactive debugging session workflow

This is the expected end-to-end workflow when an agent (Claude Code) is
developing and testing polkagent code:

```
┌─────────────────────────────────────────────────────────────────┐
│  AGENT DEVELOPMENT SESSION                                      │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  1. polkagent testnet check                                     │
│     → Verify prerequisites are met                              │
│                                                                 │
│  2. polkagent testnet up --background                           │
│     → Spawn testnet, get connection info (JSON)                 │
│                                                                 │
│  3. polkagent testnet status --format json                      │
│     → Verify all chains producing blocks                        │
│                                                                 │
│  4. polkagent testnet query balance --all-dev --format json     │
│     → Confirm genesis state is correct                          │
│                                                                 │
│  5. [Agent writes/modifies polkagent code]                      │
│                                                                 │
│  6. cargo build -p polkagent-tool-governance                    │
│     → Compile the changed crate                                 │
│                                                                 │
│  7. polkagent testnet scenario run E2E-GOV-01 --verbose         │
│     --format json                                               │
│     → Run the specific scenario being developed                 │
│                                                                 │
│  8a. IF PASSED:                                                 │
│      → Move to next scenario or wider test suite                │
│                                                                 │
│  8b. IF FAILED:                                                 │
│      polkagent testnet query referendum --index 0 --format json │
│      → Inspect the specific state that caused failure           │
│                                                                 │
│      polkagent testnet logs --chain relay --level error          │
│      --format json                                              │
│      → Check for node-level errors                              │
│                                                                 │
│      polkagent logs --last-run --format json                    │
│      → Check polkagent's own effect/tool logs                   │
│                                                                 │
│      [Agent reads error context, modifies code]                 │
│                                                                 │
│      polkagent testnet scenario run E2E-GOV-01 --verbose        │
│      --format json                                              │
│      → Re-run the scenario                                      │
│                                                                 │
│  9. polkagent testnet scenario run --phase 1 --format json      │
│     → Run all Phase 1 scenarios for regression check            │
│                                                                 │
│  10. polkagent testnet down                                     │
│      → Clean shutdown when done                                 │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 5.9 The `polkagent doctor` integration

The existing `polkagent doctor` command MUST be extended to check local
testnet connectivity:

```bash
$ polkagent doctor --format json
{
  "checks": [
    { "name": "config valid", "status": "ok" },
    { "name": "database", "status": "ok" },
    { "name": "anthropic API key", "status": "ok" },
    { "name": "local testnet", "status": "ok",
      "details": {
        "relay": { "connected": true, "block": 57 },
        "asset_hub": { "connected": true, "block": 48 },
        "collectives": { "connected": true, "block": 45 }
      }
    },
    { "name": "chain metadata sync", "status": "ok",
      "details": { "spec_version": 1016000, "metadata_hash": "0x..." }
    }
  ]
}
```

### 5.10 External diagnostic tools integration

For situations where polkagent's built-in queries are insufficient, the
following external tools SHOULD be available and documented:

#### 5.10.1 `subxt explore` for runtime introspection

```bash
# Browse all pallets on local relay chain
$ subxt explore --url ws://127.0.0.1:9944

# List all calls in Referenda pallet
$ subxt explore --url ws://127.0.0.1:9944 Referenda calls

# List all storage entries in Staking pallet
$ subxt explore --url ws://127.0.0.1:9944 Staking storage

# List all constants in Balances pallet
$ subxt explore --url ws://127.0.0.1:9944 Balances constants
```

This is invaluable for debugging metadata mismatches — an agent can explore
what's actually available on the running chain.

#### 5.10.2 `polkadot-js-api` for quick ad-hoc queries

```bash
# Quick balance check
$ polkadot-js-api --ws ws://127.0.0.1:9944 \
    query.system.account 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY

# Check referendum info
$ polkadot-js-api --ws ws://127.0.0.1:9944 \
    query.referenda.referendumInfoFor 0

# Quick transfer from dev account
$ polkadot-js-api --ws ws://127.0.0.1:9944 --seed "//Alice" \
    tx.balances.transferAllowDeath 5FHneW46... 10000000000000

# Get call info/docs
$ polkadot-js-api --info tx.referenda.submit
```

#### 5.10.3 `subkey` for key inspection

```bash
# Inspect Alice's key details
$ subkey inspect "//Alice"

# Verify a specific address belongs to a dev account
$ subkey inspect "//Alice" --network polkadot
```

#### 5.10.4 Direct JSON-RPC for low-level debugging

```bash
# Check node health
$ curl -s -H "Content-Type: application/json" \
    -d '{"id":1,"jsonrpc":"2.0","method":"system_health"}' \
    http://127.0.0.1:9933 | jq

# Get runtime version
$ curl -s -H "Content-Type: application/json" \
    -d '{"id":1,"jsonrpc":"2.0","method":"state_getRuntimeVersion"}' \
    http://127.0.0.1:9933 | jq

# Dry-run an extrinsic
$ curl -s -H "Content-Type: application/json" \
    -d '{"id":1,"jsonrpc":"2.0","method":"system_dryRun","params":["0x..."]}' \
    http://127.0.0.1:9933 | jq
```

---

## 6. End-to-end test scenarios

Each scenario exercises a specific polkagent tool or workflow against the
live local testnet. Scenarios are grouped by domain and reference the
polkagent crate that provides the capability.

### 6.1 Governance scenarios (`polkagent-tool-governance`)

These scenarios exercise the 5 governance tools: `referendum_lookup`,
`track_info`, `voter_history`, `delegation_info`, `treasury_overview`.

#### E2E-GOV-01: Referendum lifecycle — submit to enactment

**Preconditions:** Network running, Alice has sufficient balance.

**Steps:**
1. Alice submits a preimage: `System.remark("e2e-governance-test")`.
2. Alice submits a referendum on the Root track referencing the preimage hash.
3. Alice places the decision deposit.
4. Polkagent queries `referendum_lookup` — verifies referendum is in `Preparing` state.
5. Polkagent queries `track_info` for Root track — verifies shortened parameters.
6. Wait for prepare period (2 blocks).
7. Alice votes `Aye` with full conviction and 500,000 DOT.
8. Bob votes `Aye` with full conviction and 500,000 DOT.
9. Polkagent queries `referendum_lookup` — verifies referendum is in `Deciding` state.
10. Polkagent queries `voter_history` for Alice — verifies the vote is recorded.
11. Wait for decision + confirm period (~60 blocks / 6 minutes).
12. Polkagent queries `referendum_lookup` — verifies referendum is `Approved`.
13. Wait for enactment period (5 blocks).
14. Verify the `System.remark` was enacted by checking events.

**Agent debug workflow if this fails:**
```bash
# Check referendum state
polkagent testnet query referendum --index 1 --format json
# Check if votes are recorded
polkagent testnet query storage --pallet ConvictionVoting --entry VotingFor \
    --key "alice,0" --format json
# Check track parameters
polkagent testnet query storage --pallet Referenda --entry TrackFor \
    --key 0 --format json
# Check block timing
polkagent testnet status --format json  # see current block height
```

**Assertions:**
- All polkagent governance tool responses accurately reflect on-chain state.
- Referendum progresses through all lifecycle states.
- Timing aligns with shortened track parameters.

#### E2E-GOV-02: Delegation and delegated voting

**Steps:**
1. Charlie delegates voting power to Alice on Root track with 1x conviction.
2. Polkagent queries `delegation_info` for Charlie — verifies delegation target, track, conviction.
3. Alice submits and votes on a referendum.
4. Polkagent queries `voter_history` for Charlie — verifies delegated vote is attributed.
5. Charlie undelegates.
6. Polkagent queries `delegation_info` — verifies delegation removed.

#### E2E-GOV-03: Multi-track governance query

**Steps:**
1. Submit referenda on 3 different tracks simultaneously (Root, Small Tipper, Treasurer).
2. Polkagent queries `track_info` for each track — verifies correct parameters per track.
3. Polkagent queries `referendum_lookup` for each — verifies track assignment is correct.
4. Verify `max_deciding` limits are respected.

#### E2E-GOV-04: Treasury overview during active spend period

**Steps:**
1. Polkagent queries `treasury_overview` — captures initial treasury balance.
2. Submit a treasury spend proposal via Treasurer track referendum.
3. Vote and approve the spend.
4. Wait for spend period expiry (50 blocks).
5. Polkagent queries `treasury_overview` — verifies balance decreased by spend + burn.
6. Verify the 1% burn occurred.

### 6.2 Treasury and portfolio scenarios (`polkagent-tool-treasury`)

#### E2E-TRES-01: Balance query and transfer history

**Steps:**
1. Polkagent queries `balance_query` for Alice — verifies free, reserved, frozen balances.
2. Alice transfers 100 DOT to Dave via `Balances.transfer_allow_death`.
3. Wait for block inclusion and finality.
4. Polkagent queries `balance_query` for Alice and Dave — verifies updated balances.
5. Polkagent queries `transfer_history` for Alice — verifies the transfer is recorded.
6. Verify fee deduction from Alice's balance.

**Assertions:**
- Balances match to planck precision (1 DOT = 10^10 planck).
- Transfer history includes block number, extrinsic index, and fee.

**Agent debug workflow if this fails:**
```bash
# Check both balances directly
polkagent testnet query balance --chain relay --account alice --format json
polkagent testnet query balance --chain relay --account dave --format json
# Check if the extrinsic was included
polkagent testnet logs --chain relay --events --blocks 57-60 --format json
# Check polkagent's internal effect log
polkagent logs --last-run --format json
```

#### E2E-TRES-02: Staking info and nomination pool

**Steps:**
1. Polkagent queries `staking_info` for Alice — verifies validator status, bonded amount.
2. Polkagent queries `staking_info` for Charlie — verifies nominator status, nominations.
3. Charlie joins nomination pool 1 with 50 DOT.
4. Wait for era transition (20 blocks).
5. Polkagent queries `staking_info` for Charlie — verifies pool membership.
6. Query pool rewards after era payout.

#### E2E-TRES-03: Portfolio summary across chains

**Steps:**
1. Polkagent queries `portfolio_summary` for Alice — captures relay chain balances.
2. Teleport 10 DOT from relay chain to Asset Hub via XCM.
3. Wait for XCM message processing.
4. Polkagent queries `portfolio_summary` for Alice — verifies balance appears on
   Asset Hub and decreased on relay chain.
5. Verify fee deduction accounting across chains.

**Agent debug workflow if this fails:**
```bash
# Check relay chain balance (should have decreased)
polkagent testnet query balance --chain relay --account alice --format json
# Check Asset Hub balance (should have increased)
polkagent testnet query balance --chain asset-hub --account alice --format json
# Check HRMP channel status (are messages flowing?)
polkagent testnet query hrmp --chain relay --format json
# Check for XCM errors in logs
polkagent testnet logs --chain relay --filter "xcm" --level error --format json
polkagent testnet logs --chain asset-hub --filter "xcm" --level error --format json
```

#### E2E-TRES-04: Vesting schedule query

**Steps:**
1. Polkagent queries `vesting_schedule` for Charlie — verifies schedule parameters.
2. Wait for 50 blocks.
3. Polkagent queries `vesting_schedule` — verifies unlocked amount increased.
4. Charlie calls `Vesting.vest()` to claim unlocked tokens.
5. Polkagent queries `balance_query` for Charlie — verifies free balance increased.

#### E2E-TRES-05: Bounty lifecycle

**Steps:**
1. Polkagent queries `treasury_overview` — verifies genesis bounty visible.
2. Alice proposes a new bounty: "E2E Test Bounty", value = 500 DOT.
3. Approve bounty via governance referendum (Small Spender track).
4. Dave (curator) accepts curator role.
5. Dave awards the bounty to Eve.
6. Wait for bounty payout delay.
7. Eve claims the bounty payout.
8. Polkagent queries `treasury_overview` — verifies bounty completed and balance adjusted.

### 6.3 Payment lifecycle scenarios (`polkagent-payment`)

These scenarios exercise the polkagent payment system's 10-state lifecycle:
`Drafting → ReviewPending → SimulationPending → SimulationComplete →
ApprovalPending → Approved → SubmissionPending → Submitted → Confirmed →
Receipted`.

#### E2E-PAY-01: Simple DOT transfer — full lifecycle

**Steps:**
1. Agent drafts a payment: Alice sends 50 DOT to Dave.
2. Payment enters `ReviewPending` — polkagent renders action card with decoded
   call data (`Balances.transfer_allow_death`), fee estimate, and existential
   deposit check.
3. Payment moves to `SimulationPending` — dry-run against local node.
4. Simulation succeeds → `SimulationComplete` with fee estimate and weight.
5. User approves → `ApprovalPending` → `Approved`.
6. Extrinsic submitted → `SubmissionPending` → `Submitted` (with tx hash).
7. Wait for block inclusion → `Confirmed` (with block hash, extrinsic index).
8. Finality reached → `Receipted` (with finality proof).

**Assertions:**
- Every state transition emits the correct outbox event (PRD-10).
- Action card accurately reflects the decoded call.
- Fee estimate from dry-run matches actual fee (within 10% tolerance).
- Budget enforcement checked at approval gate.
- Safety invariants INV-01 through INV-04 are maintained throughout.

**Agent debug workflow if this fails:**
```bash
# Check the payment state machine
polkagent inspect effects --format json
# Check if the extrinsic was actually submitted
polkagent testnet query balance --chain relay --account alice --format json
polkagent testnet query balance --chain relay --account dave --format json
# Check the action card rendering
polkagent explain --hex 0x<encoded_call> --format json
# Verify safety invariants in logs
polkagent logs --last-run --filter "INV-" --format json
```

#### E2E-PAY-02: XCM teleport — cross-chain payment

**Steps:**
1. Agent drafts an XCM teleport: Alice sends 20 DOT from relay chain to Asset Hub.
2. Payment system resolves XCM route (relay → Asset Hub via
   `XcmPallet.limited_teleport_assets`).
3. Dry-run includes XCM execution weight and delivery fee.
4. User approves and signs.
5. Extrinsic submitted on relay chain.
6. Wait for XCM message delivery and execution on Asset Hub.
7. Polkagent verifies Asset Hub balance increased and relay chain balance decreased.
8. Payment reaches `Receipted` state.

**Assertions:**
- XCM route resolution matches expected path (teleport for system parachains).
- Fee estimation covers both relay execution and parachain delivery.
- The polkagent XCM fee estimation (PRD-05 § XCM) aligns with actual fees.

#### E2E-PAY-03: Payment with budget enforcement

**Steps:**
1. Configure polkagent with a payment budget: max 100 DOT per run.
2. Agent drafts payment of 80 DOT — succeeds, within budget.
3. Agent drafts second payment of 30 DOT — fails at approval gate (would
   exceed 100 DOT budget).
4. Verify budget enforcement error is clear and actionable.

#### E2E-PAY-04: Batch payment

**Steps:**
1. Agent drafts a batch payment: Alice sends 10 DOT to Bob, 10 DOT to Charlie,
   10 DOT to Dave via `Utility.batch_all`.
2. Dry-run validates all three transfers.
3. User approves and signs the single batch extrinsic.
4. Wait for inclusion.
5. All three recipients' balances increase by 10 DOT.
6. Single fee deducted from Alice.

#### E2E-PAY-05: Payment to existential deposit boundary

**Steps:**
1. Create a new account (no balance) using a fresh dev key.
2. Agent drafts payment of exactly existential deposit (1 DOT) to new account.
3. Payment system warns about new account creation.
4. Transfer succeeds — new account exists with 1 DOT.
5. Agent drafts payment that would drain the new account below ED.
6. Payment system warns about account reaping.
7. Verify polkagent correctly identifies `transfer_allow_death` vs
   `transfer_keep_alive` implications.

### 6.4 Cross-chain (XCM) scenarios

#### E2E-XCM-01: Teleport DOT relay ↔ Asset Hub

**Steps:**
1. Alice teleports 100 DOT from relay chain to Asset Hub.
2. Wait for XCM processing (~2 relay blocks).
3. Query Alice's balance on both chains.
4. Alice teleports 50 DOT back from Asset Hub to relay chain.
5. Query balances again.
6. Verify round-trip accounting (original minus fees).

#### E2E-XCM-02: Reserve-backed transfer via Asset Hub

**Steps:**
1. Create a foreign asset on Asset Hub representing a token from Bridge Hub.
2. Execute a reserve-backed transfer of the foreign asset.
3. Verify the asset appears on the destination chain.
4. Verify reserve accounting on Asset Hub.

#### E2E-XCM-03: HRMP channel verification

**Steps:**
1. Verify HRMP channels are open between all configured parachain pairs.
2. Send a `Xcm::Transact` from Asset Hub to Collectives chain.
3. Verify the message was delivered and executed.
4. Check watermark advancement.

### 6.5 Identity scenarios

#### E2E-ID-01: Set and query identity

**Steps:**
1. Polkagent queries identity for Bob — verifies identity set but no judgement.
2. Bob requests judgement from registrar Dave (registrar index 0).
3. Dave provides `Reasonable` judgement for Bob.
4. Polkagent queries identity for Bob — verifies judgement is now `Reasonable`.

#### E2E-ID-02: Sub-identity management

**Steps:**
1. Alice sets a sub-identity: `alice_bot` linked to a new account.
2. Polkagent queries `alice_bot` — resolves to Alice's super identity.
3. Alice renames the sub-identity.
4. Polkagent queries again — verifies updated sub name.

### 6.6 Multisig and proxy scenarios

#### E2E-MS-01: Multisig transaction execution

**Steps:**
1. The 2-of-3 multisig (Alice, Bob, Charlie) threshold account has funds.
2. Alice initiates a multisig transfer: 10 DOT from multisig to Eve.
3. Polkagent displays the pending multisig call data and approvals.
4. Bob approves the multisig call (second approval meets threshold).
5. Transaction executes from the multisig address.
6. Eve's balance increases by 10 DOT.

**Assertions:**
- Polkagent correctly computes the deterministic multisig address.
- Polkagent correctly tracks approval state (1/2, then 2/2).
- Timepoint handling is correct across blocks.

#### E2E-MS-02: Proxy transaction execution

**Steps:**
1. Alice executes a `Proxy.proxy` call on behalf of Eve (Governance proxy type).
2. The proxied call: `ConvictionVoting.vote` on an active referendum.
3. Polkagent verifies the vote was recorded under Eve's account, not Alice's.
4. Alice attempts a `Balances.transfer` via Eve's Governance proxy — fails
   (wrong proxy type).

#### E2E-MS-03: Time-delayed proxy

**Steps:**
1. Eve's governance proxy to Alice has delay = 5 blocks.
2. Alice announces an intent to proxy-vote on Eve's behalf.
3. Wait for 5-block delay.
4. Alice executes the proxied call after delay.
5. Verify the call succeeds.
6. Attempt the same flow but execute before delay expires — verify rejection.

#### E2E-MS-04: Pure proxy operations

**Steps:**
1. Ferdie's pure proxy (type `Any`) exists from genesis.
2. Transfer funds to the pure proxy address.
3. Ferdie executes transactions through the pure proxy.
4. Verify the pure proxy address has no private key (can only be controlled
   via proxy calls from Ferdie).

### 6.7 Asset Hub scenarios

#### E2E-AH-01: Fungible asset operations

**Steps:**
1. Query TUSD (asset 1337) metadata and total supply.
2. Alice transfers 10,000 TUSD to Bob via `Assets.transfer`.
3. Query balances — verify transfer.
4. Eve (admin) freezes Bob's TUSD account via `Assets.freeze`.
5. Bob attempts TUSD transfer — fails due to freeze.
6. Eve unfreezes Bob's account.
7. Bob's transfer succeeds.

#### E2E-AH-02: NFT operations

**Steps:**
1. Query NFT collection 1 metadata: owner, items count.
2. Eve transfers NFT #1 to Alice via `Nfts.transfer`.
3. Query NFT #1 owner — verify Alice.
4. Alice sets NFT #1 price via `Nfts.set_price`.
5. Bob buys NFT #1 via `Nfts.buy_item`.
6. Query NFT #1 owner — verify Bob.

#### E2E-AH-03: Asset conversion (DEX)

**Steps:**
1. Eve creates a DOT/TUSD liquidity pool via `AssetConversion.create_pool`.
2. Eve adds liquidity: 100 DOT + 100,000 TUSD.
3. Alice swaps 1 DOT for TUSD via `AssetConversion.swap_exact_tokens_for_tokens`.
4. Query the received TUSD amount — verify AMM pricing.
5. Query pool reserves after swap — verify Uniswap V2 constant product formula.

### 6.8 Staking scenarios

#### E2E-STAKE-01: Validator and nominator lifecycle

**Steps:**
1. Polkagent queries validator set — verifies Alice and Bob are active.
2. Dave bonds 1000 DOT via `Staking.bond`.
3. Dave nominates Alice via `Staking.nominate`.
4. Wait for era transition (20 blocks).
5. Polkagent queries staking info for Dave — verifies active nomination.
6. Wait for another era — trigger `Staking.payout_stakers` for Alice.
7. Verify Dave received staking rewards proportional to stake.

#### E2E-STAKE-02: Nomination pool lifecycle

**Steps:**
1. Query nomination pool 1 (genesis pool).
2. Charlie joins pool 1 with `NominationPools.join`, 50 DOT.
3. Wait for era transition.
4. Verify pool points and reward distribution.
5. Charlie claims rewards via `NominationPools.claim_payout`.
6. Charlie unbonds via `NominationPools.unbond`.
7. Wait for bonding duration (4 eras = 8 minutes).
8. Charlie withdraws unbonded via `NominationPools.withdraw_unbonded`.
9. Verify full principal returned.

#### E2E-STAKE-03: Slashing observation

**Steps:**
1. Induce an equivocation slash on a validator (requires forking validator
   behavior — MAY use Chopsticks `dev_setStorage` to inject a slash).
2. Polkagent detects the slash event.
3. Polkagent queries affected nominators' balances — verifies slash applied.
4. Wait for slash defer period (2 eras).
5. Verify slash finalized and balances adjusted.

### 6.9 Collectives / Fellowship scenarios

#### E2E-COLL-01: Fellowship referendum

**Steps:**
1. Dave (rank 1 fellow) submits a fellowship RFC proposal on Collectives chain.
2. Dave votes on the fellowship referendum.
3. Wait for fellowship decision period.
4. Polkagent queries fellowship referendum status.
5. Verify referendum outcome.

#### E2E-COLL-02: Fellowship salary claim

**Steps:**
1. Verify Dave's fellowship rank and salary entitlement.
2. Wait for salary period cycle (shortened to 5 minutes).
3. Dave registers for salary payout.
4. Dave claims salary payout.
5. Verify Dave received salary on Asset Hub (paid in DOT or USDT equivalent).

### 6.10 Utility and batch scenarios

#### E2E-UTIL-01: Batch calls

**Steps:**
1. Alice submits `Utility.batch_all` with 3 calls:
   - `Balances.transfer_allow_death` 10 DOT to Bob
   - `Balances.transfer_allow_death` 10 DOT to Charlie
   - `System.remark("batch-test")`
2. Polkagent decodes and displays the batch as 3 individual calls.
3. Verify all 3 calls executed atomically.
4. Test `Utility.batch` (non-atomic) where one call fails — verify partial execution.

#### E2E-UTIL-02: Force batch with mixed success

**Steps:**
1. Alice submits `Utility.force_batch` with calls where the second will fail
   (e.g., transfer more than available from a proxy account).
2. Verify first and third calls succeeded, second call failed.
3. Polkagent correctly reports partial success with per-item status.

---

## 7. Test execution framework

### 7.1 Test harness architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                  polkagent-e2e-tests                             │
│                  (new crate)                                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  TestnetManager (wraps zombienet-sdk)                           │
│  ├── spawn(config) → NetworkHandle                              │
│  │   Uses zombienet-sdk NetworkConfigBuilder                    │
│  │   or NetworkConfig::load_from_toml()                         │
│  ├── wait_for_ready(timeout) → Result<ChainEndpoints>           │
│  │   Calls network.wait_until_is_up() + alice.wait_metric()     │
│  ├── extract_genesis_hashes() → HashMap<Chain, H256>            │
│  ├── get_subxt_client(chain) → subxt::OnlineClient              │
│  │   Via node.client::<PolkadotConfig>().await                  │
│  ├── attach(zombie_json_path) → NetworkHandle                   │
│  │   Via AttachToLiveNetwork::attach_native()                   │
│  └── shutdown()                                                 │
│                                                                 │
│  ChainEndpoints                                                 │
│  ├── relay: ChainConnection { ws_uri, rpc_uri, subxt_client }   │
│  ├── asset_hub: ChainConnection                                 │
│  ├── collectives: ChainConnection                               │
│  ├── bridge_hub: ChainConnection                                │
│  └── coretime: ChainConnection                                  │
│                                                                 │
│  TestContext                                                     │
│  ├── chains: ChainEndpoints                                     │
│  ├── dev_accounts: DevAccountSet (subxt-signer sr25519 keys)    │
│  ├── polkagent_config: Config                                   │
│  │                                                              │
│  │  Chain state queries (via subxt dynamic API)                 │
│  ├── query_balance(chain, account) → BalanceInfo                │
│  ├── query_referendum(chain, index) → ReferendumInfo            │
│  ├── query_staking(chain) → StakingInfo                         │
│  ├── query_identity(chain, account) → IdentityInfo              │
│  ├── query_asset(chain, asset_id) → AssetInfo                   │
│  ├── query_nft(chain, collection, item) → NftInfo               │
│  ├── query_storage_raw(chain, pallet, entry, key) → Value       │
│  │                                                              │
│  │  Extrinsic submission (via subxt + dev signers)              │
│  ├── submit_and_watch(chain, signer, call) → TxResult           │
│  ├── transfer(chain, from, to, amount) → TxResult               │
│  ├── teleport(from_chain, to_chain, account, amount) → TxResult │
│  │                                                              │
│  │  Polkagent tool invocation (via polkagent-service)           │
│  ├── invoke_tool(tool_name, params) → ToolResult                │
│  ├── run_payment_lifecycle(payment) → PaymentResult             │
│  │                                                              │
│  │  Assertions                                                  │
│  ├── assert_balance(chain, account, expected)                   │
│  ├── assert_event(chain, block, event_pattern)                  │
│  ├── assert_referendum_state(chain, index, expected_state)      │
│  ├── assert_invariant(invariant_id) — checks outbox log         │
│  │                                                              │
│  │  Wait helpers                                                │
│  ├── wait_for_blocks(chain, n)                                  │
│  ├── wait_for_finality(chain, block)                            │
│  ├── wait_for_era(chain)                                        │
│  ├── wait_for_referendum_state(chain, index, state)             │
│  └── wait_for_xcm_delivery(dest_chain, event_pattern)           │
│                                                                 │
│  DevAccountSet                                                  │
│  ├── alice: subxt_signer::sr25519::Keypair                      │
│  ├── bob: subxt_signer::sr25519::Keypair                        │
│  ├── charlie, dave, eve, ferdie: ...                            │
│  └── derive_fresh(path) → Keypair                               │
│                                                                 │
│  #[e2e_test] proc macro                                         │
│  ├── Acquires or spawns TestContext (shared per test group)      │
│  ├── Attaches to running network if available                   │
│  ├── Captures logs, events, and polkagent outbox events         │
│  ├── Reports pass/fail with step-level detail (JSON)            │
│  └── On failure: dumps chain state snapshot for diagnosis       │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### 7.2 Zombienet SDK integration

The `polkagent-e2e-tests` crate MUST use the `zombienet-sdk` Rust crate for
programmatic network control. This gives typed access to nodes, metrics, and
subxt clients without parsing CLI output.

```toml
# Cargo.toml for polkagent-e2e-tests
[dependencies]
zombienet-sdk = "0.3"
subxt = { version = "0.50", features = ["native"] }
subxt-signer = { version = "0.50", features = ["sr25519"] }
tokio = { version = "1", features = ["full"] }
```

**Key SDK usage patterns:**

```rust
use zombienet_sdk::{NetworkConfig, NetworkConfigExt};
use subxt_signer::sr25519::dev;

// Spawn from TOML
let network = NetworkConfig::load_from_toml("testnet/zombienet.toml")?
    .spawn_native()
    .await?;

// Access nodes
let alice_node = network.get_node("alice")?;
let ws_url = alice_node.ws_uri(); // "ws://127.0.0.1:9944"

// Get typed subxt client
let client = alice_node.client::<subxt::PolkadotConfig>().await?;

// Wait for blocks to be produced
alice_node.wait_metric(
    "block_height_best",
    |v| v >= 5.0,
    Duration::from_secs(120)
).await?;

// Check parachain is included
let para_node = network.get_node("asset-hub-collator-01")?;
para_node.wait_metric(
    "block_height_best",
    |v| v >= 2.0,
    Duration::from_secs(120)
).await?;

// Submit extrinsic via subxt
let alice = dev::alice();
let tx = subxt::dynamic::transaction(
    "Balances",
    "transfer_allow_death",
    (dev::bob().public_key().to_address::<()>(), 10_000_000_000_000u128),
);
let events = client
    .tx()
    .sign_and_submit_then_watch_default(&tx, &alice)
    .await?
    .wait_for_finalized_success()
    .await?;

// Attach to a running network (for interactive sessions)
use zombienet_sdk::AttachToLiveNetwork;
let network = AttachToLiveNetwork::attach_native(
    PathBuf::from("testnet/.zombienet/zombie.json")
).await?;
```

### 7.3 Test organization

```
polkagent-e2e-tests/
├── Cargo.toml
├── src/
│   ├── lib.rs                        # TestnetManager, TestContext
│   ├── manager.rs                    # Zombienet SDK lifecycle management
│   ├── dev_accounts.rs               # Dev account keys via subxt-signer
│   ├── chain_queries.rs              # Dynamic subxt queries for each domain
│   ├── extrinsic_helpers.rs          # Common extrinsic submission helpers
│   ├── assertions.rs                 # Custom assertion helpers
│   ├── wait.rs                       # Wait-for-condition helpers
│   └── reporting.rs                  # JSON step-level test reporting
├── tests/
│   ├── governance.rs                 # E2E-GOV-01 through E2E-GOV-04
│   ├── treasury.rs                   # E2E-TRES-01 through E2E-TRES-05
│   ├── payments.rs                   # E2E-PAY-01 through E2E-PAY-05
│   ├── xcm.rs                        # E2E-XCM-01 through E2E-XCM-03
│   ├── identity.rs                   # E2E-ID-01 through E2E-ID-02
│   ├── multisig_proxy.rs             # E2E-MS-01 through E2E-MS-04
│   ├── asset_hub.rs                  # E2E-AH-01 through E2E-AH-03
│   ├── staking.rs                    # E2E-STAKE-01 through E2E-STAKE-03
│   ├── collectives.rs               # E2E-COLL-01 through E2E-COLL-02
│   └── utility.rs                    # E2E-UTIL-01 through E2E-UTIL-02
└── testnet/                          # Moved here or symlinked
    ├── zombienet.toml
    ├── POLKADOT_VERSION              # Pinned binary version
    ├── polkagent-local.toml          # Generated chain config
    ├── genesis-hashes.json           # Extracted after spawn
    ├── genesis/
    │   ├── relay-genesis-overrides.json
    │   ├── asset-hub-genesis.json
    │   ├── collectives-genesis.json
    │   └── bridge-hub-genesis.json
    ├── snapshots/                    # Saved chain state snapshots
    ├── logs/                         # Captured node logs
    └── scripts/
        ├── setup-binaries.sh         # Download or build required binaries
        ├── run-e2e.sh                # End-to-end test runner
        └── gen-config.sh             # Generate polkagent config from live network
```

### 7.4 Makefile integration

New Makefile targets for testnet operations:

```makefile
# Testnet management
testnet-check:      polkagent testnet check
testnet-up:         polkagent testnet up --background
testnet-down:       polkagent testnet down
testnet-status:     polkagent testnet status
testnet-reset:      polkagent testnet reset

# E2E tests (requires running testnet)
e2e:                cargo test --package polkagent-e2e-tests
e2e-phase1:         polkagent testnet scenario run --phase 1
e2e-governance:     polkagent testnet scenario run --domain governance
e2e-payments:       polkagent testnet scenario run --domain payments
e2e-xcm:            polkagent testnet scenario run --domain xcm

# Full cycle
e2e-full:           testnet-up e2e testnet-down

# One-command experiences
testnet-go:         polkagent testnet go
testnet-demo:       polkagent testnet demo
testnet-try:        polkagent testnet try $(SCENARIO)

# Diagnostic shortcuts
testnet-balances:   polkagent testnet balances
testnet-referenda:  polkagent testnet query referenda
testnet-staking:    polkagent testnet query staking
testnet-watch:      polkagent testnet watch
```

### 7.5 Test lifecycle

```
1. Binary check     →  polkagent testnet check
2. Network spawn    →  polkagent testnet up (or TestnetManager::spawn via SDK)
3. Readiness wait   →  Poll all nodes via wait_metric("block_height_best", ...)
4. Config gen       →  Extract genesis hashes + write polkagent-local.toml
5. Genesis verify   →  Assert pre-seeded state via testnet query
6. Test execution   →  Run scenarios (cargo test or polkagent testnet scenario)
7. Result capture   →  JSON step-level reports + outbox event log
8. Failure diag     →  On failure: chain state dump, log capture, error context
9. Network teardown →  polkagent testnet down (or Drop on TestnetManager)
10. Report          →  JUnit XML + JSON summary + human-readable table
```

### 7.6 Test concurrency model

E2E tests SHOULD run **sequentially within a domain group** but MAY run
**domain groups in parallel** against the same network, provided:

- Each domain group uses distinct dev accounts (to avoid nonce conflicts).
- Shared state dependencies are documented (e.g., treasury balance is shared).
- A domain group that modifies shared state (e.g., governance, treasury) MUST
  run in isolation or use block-level synchronization.

For CI simplicity, the default runner executes all tests sequentially.

### 7.7 Timeout and retry policy

| Operation | Timeout | Retry |
|---|---|---|
| Network spawn | 120 seconds | 1 retry |
| Block production start | 60 seconds | No retry |
| Block finality | 30 seconds | 2 retries |
| XCM message delivery | 60 seconds (10 relay blocks) | No retry |
| Governance decision period | Track-specific + 20% buffer | No retry |
| Era transition | Era length + 20% buffer | No retry |
| Extrinsic inclusion | 30 seconds | 1 retry |

---

## 8. Chopsticks-based testing (complementary)

### 8.1 Use cases

Chopsticks complements Zombienet for scenarios where:

- **Forking production state** is needed (test against real mainnet storage).
- **Deterministic block production** is required (no consensus variability).
- **Fast iteration** is needed (5-second startup vs 30-60 seconds).
- **Storage manipulation** is needed to set up specific edge cases.

### 8.2 Agent workflow with Chopsticks

```bash
# Fork mainnet at latest block
$ polkagent testnet up --provider chopsticks --fork-from polkadot --format json
{
  "provider": "chopsticks",
  "chains": {
    "relay": {
      "ws_endpoint": "ws://127.0.0.1:8000",
      "forked_from": "wss://rpc.polkadot.io",
      "forked_at_block": 22345678
    }
  }
}

# Inject state for testing
$ polkagent testnet exec --chain relay --provider chopsticks \
    dev-set-storage --pallet System --entry Account \
    --key "5GrwvaEF..." \
    --value '{"data":{"free":"10000000000000000000"}}' \
    --format json

# Advance blocks manually
$ polkagent testnet exec --chain relay --provider chopsticks \
    dev-new-block --count 10 --format json
```

### 8.3 Chopsticks test scenarios

#### CHOP-01: Test polkagent against real mainnet state

**Steps:**
1. Fork Polkadot mainnet at latest finalized block.
2. Inject dev account balances via `dev_setStorage`.
3. Connect polkagent to forked chain.
4. Run governance queries — results match real on-chain data.
5. Submit a test extrinsic — succeeds against forked state.

#### CHOP-02: XCM dry-run simulation

**Steps:**
1. Fork both relay chain and Asset Hub.
2. Use Chopsticks XCM simulation mode to test teleport.
3. Call `dev_newBlock` on both chains to advance XCM processing.
4. Verify balances on both chains.

#### CHOP-03: Edge case storage injection

**Steps:**
1. Use `dev_setStorage` to create a specific edge case:
   - Account at exactly existential deposit.
   - Referendum in `Confirming` state with 1 block remaining.
   - Nomination pool with pending slashes.
2. Connect polkagent and verify it handles each edge case gracefully.

---

## 9. Monitoring and observability

### 9.1 Prometheus metrics

Each Zombienet node exposes Prometheus metrics on its configured port.
The E2E test harness SHOULD scrape and assert on key metrics:

| Metric | Purpose | Agent debug use |
|---|---|---|
| `substrate_block_height{status="best"}` | Verify blocks are being produced | "Is the chain stuck?" |
| `substrate_block_height{status="finalized"}` | Verify finality is progressing | "Why isn't my tx finalizing?" |
| `substrate_sub_txpool_validations_scheduled` | Monitor transaction pool activity | "Did my tx reach the pool?" |
| `polkadot_parachain_candidate_backing_statements` | Verify parachain backing | "Is Asset Hub being backed?" |
| `substrate_tasks_polling_started_total` | Node health | "Is the node healthy?" |

Agent access pattern:
```bash
# Quick Prometheus query
$ curl -s http://127.0.0.1:9615/metrics | grep block_height
substrate_block_height{status="best"} 57
substrate_block_height{status="finalized"} 54

# Via polkagent testnet status (preferred — structured JSON)
$ polkagent testnet status --format json
```

### 9.2 Log capture

The test harness MUST capture and correlate:

1. **Zombienet node logs** — block production, finality, XCM message processing.
2. **Polkagent logs** — tool invocations, effect lifecycle, error handling.
3. **Polkagent outbox events** — every EffectIntent, EffectAttempt, EffectOutcome.

Logs SHOULD be structured (JSON) and indexed by test scenario ID for
post-mortem analysis.

### 9.3 Grafana dashboards (optional)

For developer workflow, a docker-compose overlay MAY provision:

- Prometheus scraping all node metrics.
- Grafana with pre-built dashboards for:
  - Block production rate.
  - Finality lag.
  - Transaction pool depth.
  - Parachain inclusion time.
  - XCM message latency.

This is NOT required for CI but is valuable for local debugging.

---

## 10. CI/CD integration

### 10.1 CI pipeline stages

```
┌─────────────┐    ┌──────────────┐    ┌──────────────┐    ┌─────────────┐
│  Build       │───▶│  Unit tests  │───▶│  Integration │───▶│  E2E tests  │
│  (existing)  │    │  (existing)  │    │  (existing)  │    │  (NEW)      │
└─────────────┘    └──────────────┘    └──────────────┘    └─────────────┘
                                                                  │
                                                           ┌──────▼──────┐
                                                           │  Teardown   │
                                                           │  + Report   │
                                                           └─────────────┘
```

### 10.2 E2E stage requirements

| Requirement | Value |
|---|---|
| Runner | Self-hosted or large CI runner (4+ CPU, 8+ GB RAM) |
| Disk space | 10 GB (binaries + chain data) |
| Runtime | 15-30 minutes (all scenarios) |
| Binary cache | Polkadot binaries cached by release version |
| Artifacts | JUnit XML, JSON reports, logs archive, metrics snapshot |
| Trigger | PR merge to main, nightly schedule, manual dispatch |

### 10.3 Failure handling

- **Network spawn failure:** Retry once; if repeated, fail with infra diagnostic.
- **Test failure:** Capture node logs, polkagent logs, and chain state snapshot
  at failure point. Attach as CI artifacts.
- **Timeout:** Kill network, capture partial logs, report timeout location.
- **Flaky test detection:** Track pass/fail history per scenario; quarantine
  tests that fail > 2x in 10 runs.

### 10.4 Binary provisioning

Polkadot SDK binaries MUST be provisioned in CI via one of:

1. **Pre-built releases** from the polkadot-sdk GitHub releases page (preferred).
2. **Nix flake** for reproducible builds (`nix build .#polkadot`).
3. **Docker images** (`parity/polkadot`, `parity/polkadot-parachain`)
   extracted to local binaries.

The binary version MUST be pinned in `testnet/POLKADOT_VERSION` and updated
deliberately via PR.

---

## 11. Polkagent integration points

### 11.1 ChainClient trait exercised

The E2E tests validate that `polkagent-chain-subxt` (the real chain client
implementation) correctly implements the `ChainClient` trait
(`polkagent-chain-trait/src/lib.rs`) against live nodes. Specifically:

| ChainClient method | Scenarios that exercise it |
|---|---|
| `fetch_metadata` | All scenarios (metadata pinning on first connection) |
| `simulate` / `dry_run_call` | E2E-PAY-01, E2E-PAY-02, E2E-PAY-03 |
| `submit_extrinsic` | E2E-PAY-01, E2E-PAY-02, E2E-PAY-04 |
| `watch_finality` | E2E-PAY-01, E2E-PAY-02 (receipted state) |
| `decode_call` | E2E-PAY-01 (action card rendering) |
| `query_storage` | E2E-GOV-01, E2E-TRES-01, E2E-STAKE-01, etc. |
| `xcm_query_delivery_fee` | E2E-PAY-02, E2E-XCM-01 |
| `is_trusted_teleporter` | E2E-XCM-01, E2E-PAY-02 |
| `health` | `polkagent doctor` / `polkagent testnet status` |

### 11.2 Tool validation matrix

| Polkagent tool | Crate | E2E scenarios |
|---|---|---|
| `referendum_lookup` | `polkagent-tool-governance` | GOV-01, GOV-02, GOV-03 |
| `track_info` | `polkagent-tool-governance` | GOV-01, GOV-03 |
| `voter_history` | `polkagent-tool-governance` | GOV-01, GOV-02 |
| `delegation_info` | `polkagent-tool-governance` | GOV-02 |
| `treasury_overview` | `polkagent-tool-governance` | GOV-04, TRES-05 |
| `balance_query` | `polkagent-tool-treasury` | TRES-01, TRES-04, PAY-05 |
| `staking_info` | `polkagent-tool-treasury` | TRES-02, STAKE-01, STAKE-02 |
| `portfolio_summary` | `polkagent-tool-treasury` | TRES-03 |
| `transfer_history` | `polkagent-tool-treasury` | TRES-01 |
| `vesting_schedule` | `polkagent-tool-treasury` | TRES-04 |

### 11.3 Safety invariant verification

Every E2E test MUST verify that polkagent's safety invariants hold:

- **INV-01 (Signer Isolation):** The signing key never passes through the
  model or LLM provider. In E2E tests, verify that the signer is invoked
  only by the effect executor, not by tool code.
- **INV-02 (Effect Intent Before I/O):** Every extrinsic submission is
  preceded by a recorded `EffectIntent`. Verify in outbox event log.
- **INV-03 (No Silent Duplicate Effects):** No extrinsic is submitted twice
  for the same intent. Verify by checking tx hashes against intent IDs.
- **INV-04 (Unknown Stays Unknown):** If a chain query or extrinsic returns
  an unexpected result, polkagent surfaces it rather than silently proceeding.

### 11.4 Signer configuration for E2E

E2E tests MUST use a real signer (not `polkagent-signer-fake`) to exercise
the full signing path:

**Implementation:** A new `polkagent-signer-dev` crate wrapping `subxt-signer`
with sr25519 dev account keys. This signer:

- Implements the `Signer` trait from `polkagent-signer-trait`.
- Constructs keypairs from dev URIs (`//Alice`, `//Bob`, etc.) via
  `subxt_signer::sr25519::dev::alice()` etc.
- Performs real sr25519 signing (not deterministic fakes).
- Records sign requests for test assertions (like `polkagent-signer-fake` does).
- Verifies `CanonicalSignRequest.expires_at` is not in the past.
- Rejects requests for accounts it doesn't manage.

```rust
use polkagent_signer_dev::DevSigner;

let signer = DevSigner::new(); // Manages all 6 dev accounts
let capabilities = signer.describe().await?;
// capabilities.accounts = [alice, bob, charlie, dave, eve, ferdie]

let signed = signer.sign(CanonicalSignRequest { ... }).await?;
// signed.signature = real sr25519 signature
// signed.public_key = real 32-byte public key

assert_eq!(signer.sign_call_count(), 1);
let last_req = signer.last_request(); // For assertion
```

### 11.5 Explain-before-sign pipeline E2E validation

The E2E tests exercise the full explain-before-sign pipeline
(`polkagent-service/src/explain.rs`) against real chains:

```
fetch_metadata(profile)        →  Real metadata from live node
  ↓ verify genesis hash        →  Real genesis hash comparison
decode_call(call_bytes, meta)  →  Real SCALE decoding with real metadata
  ↓ build action card          →  Human-readable card from decoded data
sign(CanonicalSignRequest)     →  Real sr25519 signature via DevSigner
submit_extrinsic(signed)       →  Real RPC submission to live node
watch_finality(tx_hash)        →  Real finality observation
```

This validates that every link in the chain — metadata fetch, genesis
verification, SCALE decoding, action card rendering, signing, submission,
and finality tracking — works with a real Polkadot node.

---

## 12. Phased delivery

### 12.1 Phase 1: Minimal viable testnet + CLI (MVP)

**Goal:** One command to go from nothing to a working E2E test. Beautiful
visual output that makes the experience feel polished from day one.

**Deliverables:**
- `polkagent testnet go` — the one-command experience (§ 2.1).
- `polkagent testnet up/down/status/check` with visual human output + `--format json`.
- `polkagent testnet balances` — visual balance table (§ 2.3).
- `polkagent testnet try <scenario>` — step-by-step visual test runner (§ 2.1).
- `polkagent testnet send` — short alias for exec transfer (§ 2.8).
- `polkagent testnet query balance` — with visual and JSON output (§ 2.4).
- Zombienet config for relay chain only (2 validators, pinned ports).
- `polkagent-signer-dev` crate with dev account signing.
- `TestnetManager` struct using `zombienet-sdk`.
- `setup-binaries.sh` script.
- E2E-TRES-01 (balance query and transfer) passing.
- E2E-PAY-01 (simple DOT transfer lifecycle) passing.
- ROSEDUST-themed terminal output following § 2.9 design principles.

**Acceptance:** `polkagent testnet go` runs to completion and produces visual
output matching the mockup in § 2.1.

### 12.2 Phase 2: System parachains + XCM

**Goal:** Add Asset Hub and Collectives parachains, HRMP channels, XCM tests.

**Deliverables:**
- Zombienet config expanded with Asset Hub (1000) and Collectives (1001).
- HRMP channel configuration.
- Genesis state for Asset Hub (TUSD, NFTs) and Collectives (Fellowship).
- `polkagent testnet query assets/hrmp` commands.
- `polkagent testnet exec teleport` command.
- E2E-XCM-01 (teleport) passing.
- E2E-AH-01 (fungible assets) passing.
- E2E-COLL-01 (fellowship referendum) passing.

### 12.3 Phase 3: Governance + Treasury + Identity

**Goal:** Full governance lifecycle with the `demo` showcase command.

**Deliverables:**
- `polkagent testnet demo` — governance demo with live progress (§ 2.1).
- `polkagent testnet demo --scenario payment/xcm/staking` — themed demos (§ 2.7).
- `polkagent testnet query referendum <n>` — visual referendum detail (§ 2.4).
- `polkagent testnet query staking` — visual staking overview (§ 2.4).
- `polkagent testnet vote` — short alias for voting (§ 2.8).
- `polkagent testnet watch` — auto-refreshing status display (§ 2.8).
- Shortened governance track parameters in genesis.
- Shortened treasury parameters.
- `polkagent testnet query referenda/treasury/identity/staking` commands.
- `polkagent testnet exec vote/submit-referendum/delegate` commands.
- `polkagent testnet wait --referendum/--next-era/--next-spend-period` commands.
- E2E-GOV-01 through GOV-04 passing.
- E2E-TRES-02 through TRES-05 passing.
- E2E-ID-01, E2E-ID-02 passing.

### 12.4 Phase 4: Advanced scenarios + Chopsticks + Snapshots

**Goal:** Multisig, proxy, staking, payments, edge cases, Chopsticks.

**Deliverables:**
- Pre-seeded multisig and proxy accounts.
- `polkagent testnet exec multisig-approve/batch/raw` commands.
- `polkagent testnet snapshot save/restore` commands.
- Chopsticks integration (`--provider chopsticks`).
- `polkagent testnet logs` command with filtering and event dumps.
- E2E-MS-01 through MS-04 passing.
- E2E-PAY-02 through PAY-05 passing.
- E2E-STAKE-01 through STAKE-03 passing.
- CHOP-01 through CHOP-03 passing.

### 12.5 Phase 5: CI/CD, TUI, observability, and polish

**Goal:** Full visual experience, TUI integration, CI automation.

**Deliverables:**
- TUI Testnet tab (F9) with live dashboard (§ 2.6).
- `polkagent testnet demo --scenario kitchen-sink` — full showcase (§ 2.7).
- CI pipeline configuration (GitHub Actions).
- Binary caching and version pinning.
- JUnit XML + JSON report generation.
- Log capture and artifact archival.
- Prometheus metrics assertions.
- Flaky test quarantine system.
- `polkagent doctor` testnet integration.
- `polkagent testnet seed` for post-genesis state setup.
- Complete Makefile targets.
- Documentation in `docs/e2e-testing.md`.

---

## 13. Open questions

| ID | Question | Impact | Owner |
|---|---|---|---|
| OQ-01 | Should we use `polkadot-local` chain spec or build a custom spec with `chain-spec-builder`? Custom gives more control over genesis but adds maintenance. | Genesis state fidelity | Unassigned |
| OQ-02 | Should E2E tests run against each PR or only on merge/nightly? PR testing adds 15-30 min to CI. | CI latency | Unassigned |
| OQ-03 | Should Chopsticks tests fork from a pinned mainnet block or latest? Pinned is reproducible but may drift from current runtime. | Test reproducibility | Unassigned |
| OQ-04 | Should the `polkagent testnet exec` commands use `polkagent-chain-subxt` directly or go through the explain-before-sign pipeline? Direct is simpler for test setup; pipeline exercises more code. | Test coverage vs. convenience | Unassigned |
| OQ-05 | Should Bridge Hub and Coretime Chain be included from Phase 2 or deferred? They add complexity but cover bridge and coretime scenarios. | Scope | Unassigned |
| OQ-06 | Should we include a People Chain parachain for identity testing or keep identity on relay chain? Production Polkadot has migrated identity to People Chain. | Production fidelity | Unassigned |
| OQ-07 | What polkadot-sdk release version should we pin to initially? | Compatibility | Unassigned |
| OQ-08 | Should the `polkagent testnet query` commands use the same `ChainClient` trait implementation as production, or a separate direct-subxt path? Using ChainClient validates the trait; direct subxt is simpler. | Diagnostic accuracy | Unassigned |
| OQ-09 | Should the Zombienet Rust SDK be a required dependency of the main polkagent binary, or only of the `polkagent-e2e-tests` crate? Putting it in the binary enables `polkagent testnet up` via SDK; keeping it in tests only reduces binary size. | Binary size vs. convenience | Unassigned |
| OQ-10 | How should `polkagent testnet snapshot` be implemented — copying node databases, or using Zombienet's persistent directory feature? DB copy is more portable; `-d` is simpler but couples to filesystem layout. | Snapshot portability | Unassigned |

---

## 14. Acceptance criteria

### 14.1 MVP acceptance (Phase 1)

**One-command experience:**
- [ ] `polkagent testnet go` runs the full cycle (check → spawn → verify → test)
      with visual output matching the mockup in § 2.1.
- [ ] `polkagent testnet try E2E-TRES-01` shows live step-by-step progress
      matching the mockup in § 2.1.
- [ ] `polkagent testnet balances` shows the visual balance table (§ 2.3).
- [ ] `polkagent testnet status` shows the visual status dashboard (§ 2.2).
- [ ] `polkagent testnet send alice dave 100` transfers and shows the visual
      exec receipt (§ 2.5).

**Visual quality:**
- [ ] Default human output uses ROSEDUST palette colors for status.
- [ ] Progress bars animate during network spawn and scenario wait steps.
- [ ] Failure output shows expected/actual/diagnostic/suggestion (§ 2.1 failure mockup).
- [ ] All commands support `--format json` for machine consumption.
- [ ] `NO_COLOR` and `--no-color` are respected.

**Functionality:**
- [ ] `polkagent testnet up` spawns a 2-validator relay chain in < 60s.
- [ ] Polkagent connects to local node and executes `balance_query` tool.
- [ ] Polkagent submits a balance transfer and tracks it to finality.
- [ ] Payment lifecycle completes all 10 states against live chain.
- [ ] `polkagent testnet down` cleanly shuts down all nodes, no orphans.
- [ ] An AI agent (Claude Code) can run the full workflow in § 5.8 using
      only CLI commands and JSON output.

### 14.2 Full acceptance (Phase 5)

**Visual experience:**
- [ ] `polkagent testnet demo` runs the governance showcase with live progress
      matching the mockup in § 2.1 (numbered steps, progress bars, tally).
- [ ] `polkagent testnet demo --scenario kitchen-sink` runs all 8 demos
      sequentially (~12 min total).
- [ ] TUI Testnet tab (F9) shows live chain status, events, balances, referenda,
      and HRMP channels matching the mockup in § 2.6.
- [ ] `polkagent testnet query referendum <n>` shows visual detail with timeline
      progress bar, tally bar chart, and vote list (§ 2.4).
- [ ] `polkagent testnet query staking` shows visual table with validators,
      nominators, pools, and era timing (§ 2.4).
- [ ] `polkagent testnet watch` auto-refreshes the status display every 2s.
- [ ] All visual output is consistent: same box-drawing, alignment, and table
      style across every command.

**Functionality:**
- [ ] All 36 E2E scenarios pass against full multi-chain testnet.
- [ ] Every polkagent tool is exercised by at least one E2E scenario.
- [ ] Safety invariants INV-01 through INV-04 verified in every scenario.
- [ ] All `polkagent testnet query` subcommands return valid JSON.
- [ ] All `polkagent testnet exec` subcommands return tx receipts with events.
- [ ] `polkagent testnet wait` conditions work for blocks, eras, referenda.
- [ ] `polkagent testnet snapshot save/restore` enables reproducible debugging.
- [ ] `polkagent testnet logs` captures and filters node logs by level and pattern.
- [ ] `polkagent doctor` includes local testnet health check.
- [ ] Scenario failures include diagnostic context: expected vs actual, chain
      state snapshot, suggested fix, and correlation with polkagent logs.
- [ ] CI runs E2E tests on merge to main with < 30 minute total time.
- [ ] Flaky test rate < 5% over trailing 20 runs.
- [ ] `make e2e-full` runs the complete cycle from spawn to teardown.
- [ ] `make testnet-demo` runs the governance demo end-to-end.

---

## 15. Glossary

| Term | Definition |
|---|---|
| **Zombienet** | Parity's ephemeral network orchestrator for spawning local Polkadot testnets with relay chain and parachains. |
| **zombienet-sdk** | Rust crate providing programmatic control of Zombienet networks — spawn, attach, node access, metrics, subxt clients. |
| **Chopsticks** | Acala's tool for forking live Polkadot chain state locally with deterministic block production. |
| **HRMP** | Horizontal Relay-routed Message Passing — cross-chain messaging between parachains via the relay chain. |
| **XCM** | Cross-Consensus Messaging — the format and language for cross-chain communication in Polkadot. |
| **Teleport** | An XCM transfer mode where assets are burned on the source chain and minted on the destination (used between system parachains and relay). |
| **Reserve transfer** | An XCM transfer mode where assets are held in reserve on one chain and derivative assets are minted on the destination. |
| **Existential deposit (ED)** | Minimum balance required to keep an account alive (1 DOT on Polkadot mainnet). |
| **Era** | A staking period during which validator sets are fixed and rewards accrue (24h on production, shortened for testing). |
| **Session** | A sub-period of an era during which session keys are fixed (4h on production, shortened for testing). |
| **Conviction voting** | Polkadot's governance voting mechanism where voters lock tokens for longer periods to amplify voting power. |
| **OpenGov** | Polkadot's current governance system with multiple parallel tracks, each with distinct parameters and origins. |
| **Chain spec** | A JSON configuration defining a blockchain's genesis state, runtime, and network identity. |
| **Dev accounts** | Well-known test accounts (Alice, Bob, etc.) derived from a shared seed, available in `*-local` chain specs. |
| **Pure proxy** | A proxy account with no private key — can only be controlled by its designated proxy accounts. |
| **Nomination pool** | A mechanism allowing multiple users to pool their stake behind validators without each meeting the minimum bond. |
| **Fellowship** | The Polkadot Technical Fellowship — a ranked body of technical contributors on the Collectives chain. |
| **SCALE** | Simple Concatenated Aggregate Little-Endian — Polkadot's binary encoding format. |
| **Action card** | Polkagent's structured rendering of a decoded extrinsic call for human review (PRD-13). |
| **Effect lifecycle** | The EffectIntent → EffectAttempt → EffectOutcome state machine for every polkagent side effect (PRD-03). |
| **subxt** | Rust library for Polkadot chain interaction — supports both static (codegen) and dynamic APIs. |
| **subxt-signer** | Rust crate for sr25519/ed25519 key management and signing, including dev account keys. |
| **DevSigner** | Polkagent's test signer wrapping subxt-signer with real sr25519 signing for E2E tests. |
| **subxt explore** | CLI tool for browsing a running chain's pallets, calls, storage, and constants via metadata. |
| **polkadot-js-api** | JavaScript CLI for quick ad-hoc chain queries and transactions. |
| **subkey** | Substrate key generation and inspection CLI (works offline). |

---

## Appendix A: Complete scenario index

| ID | Domain | Description | Phase |
|---|---|---|---|
| E2E-GOV-01 | Governance | Referendum lifecycle — submit to enactment | 3 |
| E2E-GOV-02 | Governance | Delegation and delegated voting | 3 |
| E2E-GOV-03 | Governance | Multi-track governance query | 3 |
| E2E-GOV-04 | Governance | Treasury overview during active spend period | 3 |
| E2E-TRES-01 | Treasury | Balance query and transfer history | 1 |
| E2E-TRES-02 | Treasury | Staking info and nomination pool | 3 |
| E2E-TRES-03 | Treasury | Portfolio summary across chains | 2 |
| E2E-TRES-04 | Treasury | Vesting schedule query | 3 |
| E2E-TRES-05 | Treasury | Bounty lifecycle | 3 |
| E2E-PAY-01 | Payments | Simple DOT transfer — full lifecycle | 1 |
| E2E-PAY-02 | Payments | XCM teleport — cross-chain payment | 4 |
| E2E-PAY-03 | Payments | Payment with budget enforcement | 4 |
| E2E-PAY-04 | Payments | Batch payment | 4 |
| E2E-PAY-05 | Payments | Payment to existential deposit boundary | 4 |
| E2E-XCM-01 | XCM | Teleport DOT relay ↔ Asset Hub | 2 |
| E2E-XCM-02 | XCM | Reserve-backed transfer via Asset Hub | 4 |
| E2E-XCM-03 | XCM | HRMP channel verification | 2 |
| E2E-ID-01 | Identity | Set and query identity | 3 |
| E2E-ID-02 | Identity | Sub-identity management | 3 |
| E2E-MS-01 | Multisig | Multisig transaction execution | 4 |
| E2E-MS-02 | Multisig | Proxy transaction execution | 4 |
| E2E-MS-03 | Multisig | Time-delayed proxy | 4 |
| E2E-MS-04 | Multisig | Pure proxy operations | 4 |
| E2E-AH-01 | Asset Hub | Fungible asset operations | 2 |
| E2E-AH-02 | Asset Hub | NFT operations | 4 |
| E2E-AH-03 | Asset Hub | Asset conversion (DEX) | 4 |
| E2E-STAKE-01 | Staking | Validator and nominator lifecycle | 4 |
| E2E-STAKE-02 | Staking | Nomination pool lifecycle | 4 |
| E2E-STAKE-03 | Staking | Slashing observation | 4 |
| E2E-COLL-01 | Collectives | Fellowship referendum | 2 |
| E2E-COLL-02 | Collectives | Fellowship salary claim | 4 |
| E2E-UTIL-01 | Utility | Batch calls | 4 |
| E2E-UTIL-02 | Utility | Force batch with mixed success | 4 |
| CHOP-01 | Chopsticks | Test polkagent against real mainnet state | 4 |
| CHOP-02 | Chopsticks | XCM dry-run simulation | 4 |
| CHOP-03 | Chopsticks | Edge case storage injection | 4 |

## Appendix B: CLI command quick reference for agents

This appendix provides a copy-paste reference for an AI agent working in a
terminal session. Commands are ordered by typical workflow sequence.

### Setup
```bash
polkagent testnet check --format json          # Pre-flight verification
polkagent testnet up --format json             # Spawn testnet (or reuse running)
polkagent testnet up --relay-only --format json # Fast: relay chain only
polkagent testnet status --format json         # Health check
```

### Query state
```bash
polkagent testnet query balance --chain relay --account alice --format json
polkagent testnet query balance --chain relay --all-dev --format json
polkagent testnet query balance --chain asset-hub --account alice --format json
polkagent testnet query referenda --chain relay --format json
polkagent testnet query referendum --chain relay --index 0 --format json
polkagent testnet query staking --chain relay --format json
polkagent testnet query treasury --chain relay --format json
polkagent testnet query identity --chain relay --account alice --format json
polkagent testnet query assets --chain asset-hub --format json
polkagent testnet query hrmp --chain relay --format json
polkagent testnet query storage --chain relay --pallet System --entry Account --key "alice" --format json
```

### Submit extrinsics
```bash
polkagent testnet exec --chain relay --signer alice transfer --to dave --amount 100.0 --format json
polkagent testnet exec --chain relay --signer alice vote --referendum 0 --aye --conviction 1 --balance 500000 --format json
polkagent testnet exec --chain relay --signer alice submit-preimage --call "System.remark" --args '"test"' --format json
polkagent testnet exec --chain relay --signer alice submit-referendum --track root --preimage-hash 0x... --format json
polkagent testnet exec --chain relay --signer alice decision-deposit --referendum 1 --format json
polkagent testnet exec --chain relay --signer alice teleport --dest asset-hub --amount 100.0 --format json
polkagent testnet exec --chain relay --signer alice raw --pallet Balances --call transfer_allow_death --args '...' --format json
```

### Wait for conditions
```bash
polkagent testnet wait --chain relay --blocks 10 --format json
polkagent testnet wait --chain relay --finalized 58 --format json
polkagent testnet wait --chain relay --next-era --format json
polkagent testnet wait --chain relay --referendum 0 --state Approved --timeout 600 --format json
polkagent testnet wait --chain relay --block-height 100 --format json
polkagent testnet wait --chain asset-hub --event "Balances.Deposit" --from-block 45 --timeout 60 --format json
```

### Run scenarios
```bash
polkagent testnet scenario list --format json
polkagent testnet scenario run E2E-TRES-01 --verbose --format json
polkagent testnet scenario run E2E-GOV-01 --stop-on-failure --verbose --format json
polkagent testnet scenario run --domain governance --format json
polkagent testnet scenario run --phase 1 --format json
```

### Debug and diagnose
```bash
polkagent testnet logs --chain relay --follow                                    # Tail logs
polkagent testnet logs --chain relay --level error --format json                 # Error logs
polkagent testnet logs --chain relay --filter "xcm" --format json               # Filtered logs
polkagent testnet logs --chain relay --events --blocks 50-60 --format json      # Block events
polkagent logs --last-run --format json                                          # Polkagent effect log
polkagent inspect effects --format json                                          # Effect queue
polkagent doctor --format json                                                   # Full health check
polkagent testnet snapshot save --name "debug-state" --format json              # Save state
polkagent testnet snapshot restore --name "debug-state" --format json           # Restore state
```

### External tools
```bash
subxt explore --url ws://127.0.0.1:9944                                    # Browse runtime
subxt explore --url ws://127.0.0.1:9944 Referenda storage                 # Specific pallet
polkadot-js-api --ws ws://127.0.0.1:9944 query.system.account <addr>     # Quick query
subkey inspect "//Alice"                                                   # Key info
curl -s http://127.0.0.1:9615/metrics | grep block_height                # Prometheus
curl -s -X POST -H "Content-Type: application/json" \
  -d '{"id":1,"jsonrpc":"2.0","method":"system_health"}' \
  http://127.0.0.1:9933 | jq                                             # RPC health
```

### Teardown
```bash
polkagent testnet down                         # Clean shutdown
polkagent testnet reset                        # Tear down + re-spawn fresh
```
