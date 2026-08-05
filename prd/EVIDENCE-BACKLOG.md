# Polkagent evidence and decision backlog

**Updated:** 2026-08-05

This queue holds product, safety, interoperability, and operational evidence
that cannot be closed by adding types or unit tests. It consolidates the still
useful obligations from archived research, validation plans, UX audits, and E2E
reports.

| ID | Evidence required | Blocks | Completion artifact |
|---|---|---|---|
| EVD-01 | Interview/observe representative builder, operator, and Polkadot action users on the first useful workflows. | Scope ordering and persona confidence | Findings with participant profile, tasks, observed pain, decision, and PRD changes |
| EVD-02 | Test explain-before-sign/action-card comprehension, canonical-vs-model text separation, fee/network/finality understanding, and refusal. | CHAIN-01, PAY-01 production UX | Script, anonymized results, comprehension/error rates, revised card acceptance criteria |
| EVD-03 | Decide and validate signer custody/handoff for local, external wallet, KMS/Vault, proxy, and testnet developer modes. | SEC-01, CHAIN-01, PAY-01 | Threat model, selected backends, byte-level handoff transcript, failure/revocation tests |
| EVD-04 | Prove metadata pinning, stale metadata, wrong genesis/network, runtime upgrade, decode drift, and exact signed bytes on a live local network. | CHAIN-01 release gate | Machine-readable run bundle with metadata hashes, bytes, events, finality, and negative cases |
| EVD-05 | Exercise crash boundaries before/after intent persistence, external I/O, unknown outcome, approval, outbox delivery, and restart. | EXE-01, OBS-01 | Fault matrix and durable records showing no duplicate external effect |
| EVD-06 | Interoperate cross-process with the PCA reference product for messages, attachments, dedupe, restart, and reply semantics. | PCA-01 | Versioned compatibility fixture/transcript and documented deviations |
| EVD-07 | Validate Polkagent as a Zed custom ACP agent: init, prompt streaming, tools, permission, cancel, restart/load, commands, config options, cwd/MCP behavior. | ACP-01 | ACP transcript fixtures, Zed screenshots/log review, setup and troubleshooting guide |
| EVD-08 | Run container/deployment smoke with persistent data, health, shutdown, restart, backup/restore, upgrade/rollback, auth, and resource pressure. | OPS-01 | CI artifact, runbook, recovery timings, known limits |
| EVD-09 | Attempt extension escape/capability abuse, malicious package/update, dependency confusion, and rollback. | EXT-01 | Red-team corpus, sandbox/trust results, signed package provenance |
| EVD-10 | Prove tenant/principal isolation through API, stores, events, memory, artifacts, groups, payments, logs, and metrics. | SEC-01, OPS-01 | Cross-tenant matrix with deny/audit evidence |
| EVD-11 | Compare TUI, terminal chat, API, and ACP views of the same active/restarted interaction. | FND-02, TUI-01, ACP-01, API-01 | Cross-surface conformance test using stable interaction/run/event IDs |
| EVD-12 | Validate real provider/harness readiness, streaming, tool protocol, cancellation, retry, billing/usage, context limits, and redaction. | PRD-04a maturity claims | Per-adapter conformance report; unsupported features explicitly labeled |

## Partial evidence captured

- **EVD-08 / OPS-01 container lifecycle slices (2026-08-05):**
  `scripts/container-smoke.sh`, invoked by the `container-smoke` CI job,
  validates the Compose model, builds the canonical image with `Cargo.lock`,
  waits for Docker readiness, probes live/ready/startup through the published
  port, and verifies a configured non-root user and process UID. It also proves
  that a read-only bind-mounted config reaches `serve`, SIGTERM drains the HTTP
  server with exit 0, a newly created container retains the same named volume,
  and a SQLite-backed CLI agent marker survives replacement. CI uploads a
  summary, selected state, and container logs on success or failure. EVD-08
  remains open for durable API run/agent recovery, worker/effect draining,
  Postgres, backup/restore, upgrade/rollback, auth, tenant isolation, crash
  boundaries, and resource-pressure evidence.

## Evidence quality rules

- Record the commit/worktree state, configuration, real/fake adapters, command,
  environment, expected outcome, actual result, and durable artifact paths.
- A passing unit test is not a substitute for a client, network, process, or
  restart boundary named by the evidence item.
- Preserve failed evidence. Move raw captures to a dated `tmp/archive/`, and
  summarize the decision or open gap here.
- Close an item only when its artifact is reproducible by another agent or
  operator from documented commands.
- Link closed evidence from `STATUS.md` before upgrading a maturity level.
