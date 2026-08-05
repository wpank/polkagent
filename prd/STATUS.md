# Polkagent implementation status

**Evidence snapshot:** 2026-08-05
**Conclusion:** broad component maturity; incomplete production composition;
no PRD verified complete end-to-end

## Verification baseline

The current worktree passed:

```text
cargo check --workspace
cargo test --workspace --no-fail-fast
```

The workspace test run exited 0 and includes extensive unit, property,
contract, integration, security, TUI rendering, API, and doc tests. This is a
strong component baseline. It does not prove production composition, a real
chain action, a container deployment, an interactive TUI prompt, or an ACP
server session.

The audit intentionally treats tests such as “returns 501 when store is not
configured” as contract coverage and simultaneous evidence that production
startup still has a wiring gap.

## Product-surface status

| Surface/capability | Component state | Product state | Decisive gap |
|---|---|---|---|
| One-shot CLI run | Substantial and tested | Partially usable | Bootstrap is embedded in the command; tools/effects/policy are not a real model loop. |
| Monitoring TUI | Rich views and rendering tests | Not actionable as an agent console | No prompt/session/start/cancel path; direct DB mutations bypass services. |
| REST/WebSocket API | Broad route and middleware coverage | Not a durable production control plane | `serve` uses in-memory agents/runs and omits most optional stores/registries, producing many 501s. |
| Interactive terminal chat | No shared surface | Missing | Requires `InteractionService`, command registry, and runtime factory. |
| ACP from Polkagent to other harnesses | ACP client exists and tests pass | Useful downstream adapter | This is client-side harness support only. |
| Polkagent inside Zed/ACP clients | Design complete in PRD-19 | Missing | No ACP agent server, `polkagent acp`, session mapping, or Zed conformance. |
| Providers/harnesses | Many adapters exist | Partially composed | Each adapter needs shared-runtime conformance and real failure/readiness evidence. |
| Tools/skills | Registries and handlers exist | Not actionable in normal run loop | Orchestrator sends no tool schemas and synthesizes tool success instead of executing. |
| Effects/approvals/policy | Strong domain libraries | Incomplete execution path | Effect pipeline and grant resolver are not used by the central orchestrator. |
| Conversations/memory | Durable stores and migrations exist | Not a shared multi-turn experience | Runs are not durably linked to interaction turns; memory is not assembled into normal context. |
| Groups/feeds/evals | Significant libraries/tests | Mostly unsurfaced | No production caller creates durable child runs or evaluates the real composed runtime. |
| Polkadot reads | RPC/metadata/codec components exist | Partially usable | Pinned live metadata and network behavior need real-path validation. |
| Polkadot writes | Effect/signing/finality components exist | Not end-to-end proven | Real signer, exact bytes, transaction matching, finality, dry-run/XCM, and local-chain tests remain. |
| PCA compatibility | Crypto/queue/sync building blocks exist | Not a real network integration | Current transport E2E manually shuttles messages in process. |
| Security | Grants, tests, redaction, signer abstractions exist | Not production hardened | Plaintext file secrets, shared-key API auth, mock KMS/DID paths, and unused policy runtime. |
| Payments | Intent/store/budget components exist | Not value-moving | Store/runtime integration, real signature/settlement, and failure reconciliation remain. |
| Marketplace/plugins | Durable local plugin/kit lifecycle plus listing components | Install/update/rollback/uninstall library exists; no user surface or execution | Manifest/lock format remains split; no CLI/API runtime activation, actual sandbox engine, or cryptographic trust pipeline. |
| Deployment/cloud | Docker/cloud/Postgres artifacts exist | Not operationally proven | Current container command/port drift; no deployment smoke, HA, or production control/worker path. |

## PRD implementation posture

| PRD | Component maturity | Production wiring | User-visible E2E | Disposition |
|---|---|---|---|---|
| 01 Vision | N/A | N/A | N/A | Active normative direction |
| 02 Architecture | Strong but drifted | Partial | No | Active invariants; reconcile when touched |
| 03 Execution | Strong libraries | Blocked at central loop | No | Active P0 |
| 04/04a Providers/tools/harnesses | Strong adapters | Partial | Partial one-shot only | Active P0/P1 |
| 05 Polkadot | Strong read/action components | Partial | No real write proof | Active P1 + PRD-17 |
| 06 PCA | Strong internal primitives | Missing network composition | No | Active P1 |
| 07 Security | Strong primitives/tests | Partial/unsafe defaults | No production security proof | Active P1 |
| 08 Payments | Domain/store components | Missing from runtime | No | Active P2 after safe action path |
| 09 Memory/groups/evals | Strong components | Mostly missing | No orchestration proof | Active P1 |
| 10 Observability | Strong components | Partial | No recovery/replay proof | Active P1 |
| 11 Deployment/cloud | Scaffolding/components | Missing/broken smoke path | No | Active P2 |
| 12 Marketplace/extensions | Durable local lifecycle/components | Library only; execution missing | No install-to-run proof | Active P2 |
| 13 UX | CLI/TUI exist | Partial | Interactive experience missing | Active P0/P1 + PRD-19 |
| 14 API/config | Broad components/routes | P0 composition gap | No durable control-plane proof | Active P0/P1 |
| 15 Testing | Broad green suite | Production paths under-tested | Live/client/ops gates missing | Active cross-cutting |
| 17 Local testnet | Pinned native fixture, provisioning, CI gate, and live RPC/finality test target | Read-only baseline wired; signed action path missing | No real write proof; CI network artifact pending | Active P1 |
| 19 Interactive/ACP | Architecture specified | Missing | No | Active P0/P1 |

## Decisive implementation evidence

- `crates/polkagent-cli/src/commands/serve.rs` constructs
  `InMemoryAgentStore` and `InMemoryRunManager`.
- `crates/polkagent-api/src/state.rs` models event, artifact, skill, tool,
  memory, payment, audit, conversation, and registry dependencies as optional;
  route handlers return `NotImplemented` when startup does not inject them.
- `crates/polkagent-run/src/orchestrator.rs` sends an empty tool list and does
  not drive the configured effect pipeline/grant resolver through real tool
  calls and approvals.
- `crates/polkagent-marketplace/src/local.rs` now persists immutable plugin/kit
  versions and restart-safe selection/history with integrity checks. It
  explicitly records signature bundles as unverified claims because no
  cryptographic verifier or runtime sandbox is connected yet.
- `crates/polkagent-cli/src/tui/app.rs` owns a database pool and polls it; it
  does not own the application/interaction runtime or a live event receiver.
- `crates/polkagent-harness-acp` is an ACP client for downstream coding-agent
  harnesses, not a Polkagent ACP agent server.
- `crates/polkagent-transport-pca` and its current integration test exercise an
  in-process transport, not real PCA application/peer I/O.
- `crates/polkagent-payment/src/ledger.rs` labels its signature as an
  experimental placeholder.
- PRD-17 now has an honest live-node target that queries relay/Asset Hub RPC and
  requires relay finality to advance. The actual CI network run is still needed
  as evidence, and no bytes are yet signed, submitted, matched, or reconciled.

## Completion gate for future status updates

Before changing any row to end-to-end verified, attach all of these:

1. The exact executable entry point and production composition builder.
2. The persistent records created and restart/recovery behavior.
3. The real external adapter used; name any remaining fake explicitly.
4. Happy-path and critical failure-path test names.
5. A user/client transcript or machine-readable artifact.
6. Security and observability evidence.
7. Documentation/configuration changes.

If one item is absent, keep the row partial and add a backlog item rather than
estimating a completion percentage.
