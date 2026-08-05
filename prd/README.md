# Polkagent product and implementation planning

This directory is the canonical planning surface for Polkagent as of
2026-08-05. Start here instead of treating an old checklist, audit report, or
crate count as the current roadmap.

## Reading order

1. [`PRD-00-MASTER-INDEX.md`](PRD-00-MASTER-INDEX.md) — document authority,
   active scope, and how the suite fits together.
2. [`STATUS.md`](STATUS.md) — evidence-backed implementation status for the
   current worktree.
3. [`IMPLEMENTATION-BACKLOG.md`](IMPLEMENTATION-BACKLOG.md) — dependency-ordered
   work packets, acceptance gates, and parallel-agent lanes.
   [`QA-01-CLIPPY-INVENTORY.json`](QA-01-CLIPPY-INVENTORY.json) is the
   machine-readable closure record for the mandatory and extended lint gates,
   with the original remediation baseline preserved.
4. The relevant normative PRD from PRD-01 through PRD-15.
5. The active delivery PRDs:
   [`PRD-17-LOCAL-TESTNET-E2E.md`](PRD-17-LOCAL-TESTNET-E2E.md) and
   [`PRD-19-INTERACTIVE-CONSOLE-ACP.md`](PRD-19-INTERACTIVE-CONSOLE-ACP.md).
6. [`EVIDENCE-BACKLOG.md`](EVIDENCE-BACKLOG.md) for research, validation, and
   operational proof that code-only work cannot close.
7. [`ADR-002`](../docs/adr/ADR-002-Shared-Runtime-Interaction-Semantics.md) for
   the accepted shared runtime, interaction, persistence, event, and command
   boundaries used by current implementation work.

## Authority rules

- PRD-01 through PRD-15 define intended product and architecture.
- PRD-17 and PRD-19 are current delivery designs for live-chain E2E and
  interactive/IDE surfaces.
- `STATUS.md` is the implementation-truth snapshot. It overrides stale
  implementation claims and unchecked appendices embedded in older PRDs.
- `IMPLEMENTATION-BACKLOG.md` is the current execution queue. An agent should
  not create a second backlog in `tmp/`.
- Tests and crate presence are evidence of component maturity, not proof of a
  user-visible end-to-end capability.
- Archived material is provenance, not current instruction.

## Completion and archive policy

A capability may be marked delivered only when all of the following are true:

- its production composition path uses the intended durable adapters;
- its primary user surface can initiate, inspect, cancel, and recover it;
- success, refusal, timeout, restart, and partial-failure paths are tested;
- an end-to-end test exercises the same path a user or external client uses;
- configuration, security, observability, and operator documentation exist;
- no fake, no-op, placeholder, in-memory-only, or `501 Not Implemented` fallback
  remains on the claimed production path.

A planning document may be archived when it is superseded, is point-in-time
evidence whose open items have been transferred, or describes a capability
that meets the completion rule above. Archiving a research input does not mean
the product capability is delivered.

## Scratch-work policy

`tmp/` is ignored and is no longer a roadmap. Use it only for disposable
captures and short-lived working notes. Promote decisions or verified gaps to
this directory, then move the source evidence into a dated `tmp/archive/`.
