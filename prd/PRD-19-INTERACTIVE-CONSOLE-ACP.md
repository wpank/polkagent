# PRD-19 — Interactive Polkagent: TUI, Terminal Sessions, Orchestration, and ACP

**Status:** active architecture and implementation plan; shared runtime,
durable interaction service, terminal chat, TUI Console, HTTP adapter, and ACP
server slices implemented

**Prepared:** 2026-08-05
**Updated:** 2026-08-06

**Scope:** current Polkagent checkout at `/Users/will/dev/par/polkagent`, compared with Roko at `/Users/will/dev/nunchi/roko/roko` and the current ACP v1/Zed documentation

**Implementation status:** `polkagent-surface-acp` and protocol-safe
`polkagent acp` dispatch implement an ACP v1 initialize/new/load/resume/prompt/
cancel slice over the durable `InteractionService`, state-aware registry-backed
slash-command discovery, and persisted session agent/model selection. An
official-SDK subprocess suite proves prompt/run, shared-registry command
discovery, inactive/active/inactive catalog refresh across success, cancellation,
and backend error, truthful refusal, active-request cancellation, durable
terminal state, exact conversation/turn/run identity, same-turn retry without
duplication, restart/load/follow-up/resume, secret/panic containment, and
protocol-stdout safety on that wire path.
The F9 Console supports selecting an active agent, Unicode-safe multiline
editing, registry-derived slash discovery/completion, durable prompts and
follow-ups through `InteractionService`, typed output/progress/usage,
cancellation, transcript/history reload after restart, and bounded simultaneous
turns across distinct agent/conversation targets. `[`/`]` switches retained
activities without losing per-conversation transcript, prompt draft, or history;
`x` cancels the selected exact activity. Its terminal lifecycle is proven in a
real Unix PTY. `polkagent chat` now provides a focused
interactive/non-TTY adapter with explicit resume, multiline input, shared
help/status/agents/agent/runs/inspect/cancel/new/resume/model handlers, checkpoint
resubscribe, and SIGINT cancellation. The TUI now executes the truthful
help/status/agents/agent/new/resume/runs/inspect/model subset
through the same registry and service executor, renders structured results, and
switches/reloads exact durable conversations without creating model turns. ACP
now maps its session ID exactly to the durable conversation UUID and uses the
same prompt, cancel, config, replay, and restart lifecycle. Effect-backed tool
updates and immutable origin-cwd verification now cross restart. Session list/
import, permissions, raw tool-data redaction, MCP passthrough, and manual Zed
validation remain open.

One deterministic SQLite conformance fixture now carries the same exact
conversation through HTTP creation/config, terminal-chat prompting, ACP
restart/load/follow-up, TUI load/prompt/render, and final HTTP projection. The
ordered transcript, turn/run links, persisted model, usage, lifecycle, and ACP
checkpoint agree, while commands and refusal create no work. Active-turn
cancel remains covered by separate adapter-specific durable tests so the
linear three-turn fixture can continue; approval and rich plan/redaction-safe
structured-tool projection remain open.

FND-02 now includes shared IDs/config/requests/handles, structured events and
projections, the service/store traits, a bounded durable/live replay hub, typed
MVP slash-command parsing plus handlers, and a runtime-composed durable
`InteractionService`. The service prepares caller-identified correlated runs
without events, atomically commits user transcript plus turn/run links,
subscribes, then activates execution. Assistant transcript and terminal state
commit together exactly once; retry, cancel, replay, lag, pre-activation crash,
and paginated restart recovery have headless tests. Only target configuration
and execution-scoped model selection are executable today. Model precedence is
prompt override, persisted interaction, then agent default; validation happens
before activation and changes only the cloned prepared-run spec. TUI, terminal
chat, HTTP, and ACP bind the service. Model-executor prompts include
bounded typed prior completed turns; string-only harness history fails role-
safely. Effect-backed tool lifecycle projection is implemented with stable IDs
and safe status, while approval and provider/harness/autonomy/max-turn/budget
overrides remain open, so the full headless exit criterion is not closed.

**Supersedes:** the implementation role of archived PRD-18; unresolved work is
tracked in `IMPLEMENTATION-BACKLOG.md`

## Executive decision

### 2026-08-05 implementation checkpoint

The actionable TUI slice now includes:

- `p` opens a dedicated prompt composer for the selected or first active agent;
- F9/`9` opens the Console workspace;
- Enter starts a durable interaction turn and `x` requests exact turn
  cancellation;
- live text, lifecycle, progress, tool names, errors, and final usage are
  projected into the Console without blocking the terminal loop;
- the durable run is selected and refreshed into the existing Runs, Run Detail,
  and Timeline views;
- the TUI retains one factory-built runtime and starts through its exact
  `InteractionService`, so it does not shell out or manufacture database rows;
- reducer, key mapping, TestBackend rendering, and fake-executor durable-run
  tests cover the new seam;
- extended-grapheme-safe cursor/edit/delete behavior, multiline submission,
  bounded history, bracketed paste, draft restoration, and a scrolling wide-
  character-aware composer;
- shared-registry slash discovery/help with aliases, argument hints, keyboard
  selection, truthful conversation availability, and executable `/help`,
  `/status`, `/agents`, `/agent`, `/new`, `/resume`, `/runs`, `/inspect`, and
  `/model` results;
- drop-backed best-effort terminal restoration, with real Unix PTY evidence for
  normal exit, ordinary error, and caught-panic unwind.

The Console now has a bounded asynchronous same-agent session selector in
addition to `/new` and `/resume`; it loads the exact selected durable session
while unrelated work continues and serializes overlapping control requests.
The controller admits at most eight simultaneous agent/conversation turns and
retains 32 compact activity summaries and bounded per-conversation viewports.
Same-conversation duplicate work is rejected without clearing the draft, and
capacity/retention/backpressure behavior is deterministic. Deliberate gaps remain:
model executors receive the
newest 32 completed pairs among the latest 1,000 prior turn records; harness-
backed follow-up is explicitly unsupported because its string ingress cannot
preserve roles. Direct legacy
approval/database actions remain outside the Console; group plans/child-run
orchestration, attachments, word navigation, and approval/configuration command
parity are not implemented.
The architecture below remains the target rather than retroactively treating
this slice as TUI-01 completion.

Polkagent now has useful durable terminal and TUI session slices with bounded
model-executor context, but it is not yet the approval-capable multi-agent
orchestration product described by this PRD.

`polkagent-runtime` now supplies the accepted production `RuntimeFactory`,
structured readiness, durable SQLite composition, startup recovery, and agent
rehydration. One-shot `polkagent run`, the TUI, ACP, and `serve` have migrated
to it; focused tests prove TUI reuse across sequential prompts, ACP startup
recovery, and HTTP-created state across a runtime restart.

Polkagent has a substantial monitoring TUI, a runnable one-shot `polkagent
run`, an in-process execution service, streaming run events, conversation
storage, an ACP **client** used to drive other coding agents, and a separate
bounded ACP **agent server** for editors. These are valuable building blocks.
The TUI, terminal chat, HTTP API, and ACP now use the durable
`InteractionService` prompt/transcript lifecycle. ACP restores exact sessions
through load/resume after process restart rather than owning a parallel run
bootstrap path.

The delivered slices establish two distinct product surfaces that still need
to be completed:

1. The delivered terminal chat/TUI session must gain role-safe harness
   follow-up, rich redaction-safe tool detail, approvals, broader command
   coverage, and multi-agent orchestration.
2. The ACP **agent server** (`polkagent acp`) must grow from its protocol MVP
   into full Zed and other ACP-client support.

The important architectural decision is to build both as adapters over one
shared `InteractionService`, command registry, production runtime, and event
stream. The TUI, compact terminal chat, REST/WebSocket API, and ACP server must
not each implement their own run bootstrap, slash-command parsing, persistence,
or approval behavior.

The archived `archive/2026-08-05/superseded-plans/PRD-18-INTERACTIVE-TUI.md`
correctly identifies much of the TUI
problem and proposes a prompt bar. The new Console closes its smallest
single-run gap, but that archived plan still does not address ACP, durable
interactive sessions, multi-run/multi-agent turns, or the production runtime.
It remains design input rather than the completion contract.

## Bottom line: status matrix

| User capability | Current status | Evidence / implication |
|---|---|---|
| Run `polkagent` with no arguments | Launches the monitoring TUI when stdout is a TTY | `polkagent-cli/src/main.rs` dispatches `None` to `launch_tui` |
| Prompt Polkagent interactively | Implemented for single-agent turns with per-conversation model selection | `polkagent chat` and the F9 Console call the durable interaction service; model executors receive bounded typed completed history, while harness follow-up fails explicitly |
| Prompt from inside the TUI | Implemented for bounded simultaneous agent/conversation turns | `p` opens the selected Console conversation; distinct targets continue in the background, `[`/`]` switches retained activities, and same-conversation duplicates fail before draft loss |
| Start or cancel a run from the TUI | Implemented for the bounded activity slice | `InteractionService::prompt` creates correlated conversation/turn/run state; the controller admits eight turns and `x` cancels only the selected exact activity |
| Execute commands in the TUI | Truthful durable subset implemented | `/help`, `/status`, `/agents`, `/agent`, `/new`, `/resume`, `/runs`, `/inspect`, and `/model` use the shared command executor, render structured results, persist target/model per conversation, and never become model turns; run reads share ACP/chat scope, bounds, and redaction while unavailable capabilities fail explicitly |
| Approve/deny in the TUI | Not safely implemented | Legacy Approvals-tab writes mutate display-state rows but cannot durably resolve and resume the exact paused effect; coordinator-backed interaction approval remains required |
| See live run output in the TUI | Implemented for correlated foreground/background activity | Controller projects bounded per-activity interaction events and resubscribes from a durable checkpoint after lag; a 32-entry redaction-safe strip exposes identity/status without prompt/output/error detail, while approval/rich plan projection remains absent |
| Persist/resume human conversations | Implemented in TUI, chat, HTTP, and ACP | TUI reloads/switches sessions, chat resumes a conversation ID, HTTP exposes session/turn/event reads, ACP maps session IDs to conversation UUIDs and supports restart load/resume, and completed pairs feed the next model-executor call |
| Orchestrate agent groups from a user surface | Domain building blocks only | `polkagent-group` exists, but there is no CLI/TUI/service surface for it |
| Use Cursor/Goose/Kiro/OpenCode *from* Polkagent | ACP client exists and is tested | `polkagent-harness-acp` plus harness adapter crates |
| Use Polkagent *from* Zed | Protocol/tool slice implemented; manual Zed proof pending | `polkagent acp` uses the official SDK, durable cwd isolation, native tool updates, state-aware command discovery, and executable client fixtures; editor permission acceptance remains APR-07 |
| ACP slash commands/config selectors | Durable state-aware shared-registry subset plus persisted agent/model selection implemented | `/help`, `/status`, `/agents`, `/agent`, `/runs`, `/inspect`, and `/model` are advertised while idle; current-prompt `/cancel` plus `/stop` is added only while busy and withdrawn after success/cancel/error. `/new` and `/resume` map truthfully to native ACP lifecycle operations; run reads are bounded, redaction-safe, and conversation-scoped, while native selectors write the same durable interaction config and provider/autonomy settings remain unavailable |
| REST API as a production control plane | Durable interaction/core slice implemented | Versioned lifecycle, strict persisted target/model config, finite replay, checkpointed SSE, immutable skill reads, and durable memory query/lookup/stats/deletion use exact runtime-owned components; 9 optional skill-mutation/audit/registry routes remain unavailable |

## 1. What Polkagent actually has today

### 1.1 The TUI is now an actionable monitor for bounded concurrent runs

The current Ratatui application has nine top-level tabs plus run detail:

- Dashboard
- Agents
- Runs
- System
- Timeline
- Approvals
- Memory
- Audit
- Console

This is a meaningful operational dashboard. The Console's reducer, input
mapping, TestBackend render, and durable fake-executor run path now have focused
coverage; the broader baseline remains recorded in `STATUS.md`.

The event loop in `crates/polkagent-cli/src/tui/app.rs`:

- asynchronously selects bounded terminal input, runtime/controller events,
  monitoring completions, resize updates, and coalesced ticks;
- applies backpressure to key and paste input while retaining only the newest
  resize and avoiding accumulated idle ticks;
- schedules the five-second SQLite aggregate projection on a tracked blocking
  job, with one in flight and one newest-generation follow-up;
- schedules an optional chain poll on a separate tracked, single-flight
  blocking job, with bounded HTTP exchanges;
- carries monitoring snapshots through a four-slot bounded result channel and
  rejects stale run-selection generations before they can update live state;
- renders at an effective 10 fps (5 fps when idle);
- awaits a bounded typed controller channel for run output and task completion;
- joins input/controller/monitoring producers on ordinary shutdown, with an
  explicit error if a blocking monitoring job exceeds the finite shutdown
  ceiling rather than silently claiming it was joined.

`App` owns a long-lived `PolkagentRuntime`, its `SqlitePool`, and a
`RunController`. The Console reads the shared typed command registry for
discovery/help and routes prompt/cancel through the runtime's exact durable
`InteractionService`. Sequential prompts reuse one durable per-agent
interaction. Distinct selected agent/conversation targets may also execute
concurrently: activity UUIDs correlate every update, the selected viewport
restores its transcript/draft/history, and exact cancellation does not affect
background turns. `s` lists and selects same-agent interactions through the
service even while unrelated turns run; typed events and conversation/turn/run
IDs project without blocking the UI, and bounded transcript/composer history
reloads after restart. Deliberately
blocked/failing refresh fixtures prove cancel, prompt entry, resize, and
controller completion stay responsive, and that only the latest selected-run
projection can apply.

The TUI is not completely read-only: legacy approvals, denials, and memory
deletion write directly through `TuiDb`. Those approval writes are unsafe
display-state mutations, not a usable permission workflow. Replace them only
with coordinator-backed `InteractionService::approve`/`deny` in APR-05; the
APR-01 SQLite coordinator now exists, but current `AppService` approval methods
remain bus-only and the orchestrator does not invoke the durable boundary.
Starting runs by inserting database rows would be even more dangerous and must
not be done.

### 1.2 Input now has a bounded prompt mode

`crates/polkagent-cli/src/tui/input.rs` now defines a distinct `Prompt` mode in
addition to `Normal`, memory-search `Insert`, and the still-inert `Command`
mode. It supports extended-grapheme insertion/deletion/cursor/line navigation,
multiline submission, bounded history with draft restoration, bracketed paste,
and cancel/reset. Paste is one non-submitting action; CRLF/lone CR normalize to
LF, tabs become four spaces, and other control characters are dropped. The
128-KiB composer truncates only at whole grapheme boundaries. Remaining editor
gaps are:

- `/` in normal mode invokes search, while `/` typed in the Console composer
  opens the shared-registry picker;
- Command mode handles only Escape;
- registry-derived completion/help and the truthful help/status/agents/agent/
  new/resume/runs/inspect/model subset execute, but word navigation and
  approval/configuration command parity remain open;
- composer history and transcript reload durably, and `s` opens a bounded
  asynchronous same-agent session picker with stale-result guards;
- target selection can begin from the highlighted/first active agent and then
  persist per conversation through `/agent`; create/edit still lacks a modal.

This is enough to initiate useful work, but not yet an IDE-quality editor.

### 1.3 One-shot run now consumes the shared production runtime

`polkagent run --agent-id <agent> --prompt <prompt>` now builds
`polkagent-runtime::RuntimeFactory` and consumes its shared `AppService`, event
bus, SQLite pool, configuration, readiness, rehydration, and recovery. A
subprocess test proves a real run and restart-safe durable records through this
entry point.

The TUI and terminal chat now retain one `PolkagentRuntime` and use its exact
`InteractionService`; `serve` injects that same runtime-owned service into the
versioned HTTP interaction routes. ACP also retains one factory-built runtime
and routes exact durable editor sessions through its interaction service;
official-client fixtures prove abandoned-run recovery and prompt/retry/load/
follow-up/resume across process restart.

### 1.4 Useful session and streaming primitives exist but are incomplete

Useful components already present:

- `polkagent-conversation` defines conversations and typed messages.
- SQLite migration v5 and `conversation_store_impl.rs` provide durable storage.
- `AppService::start_run`, `cancel_run`, `approve_effect`, and `deny_effect`
  provide application operations.
- `EventBus`, `RunEvent`, and `RunProgressStream` provide live progress.
- The orchestrator publishes streaming text, tool status, approvals, and
  terminal events.

Gaps that matter to an IDE-quality session:

- A conversation is not the same thing as an executing turn. The new
  `InteractionService` now appends user/assistant transcript, creates and
  associates one target run, projects output, and exposes cancellation; group
  runs, role-safe harness context, approvals, and rich tool transcript remain
  absent.
- `start_run` accepts only `(agent_id, prompt)` and does not accept a
  conversation/interaction ID or per-session execution overrides.
- The legacy `RunProgressEvent::ToolUse` adapter still contains only tool name
  and status. The interaction service no longer relies on it for ACP: it
  projects stable effect-derived tool-call identity plus safe title/kind/status.
  Raw input, output, locations, and diff content remain withheld pending a
  redaction contract.
- The legacy approval mapping still manufactures a new `ApprovalId` instead of
  carrying the underlying request/effect identity. The interaction contract has
  the right identity shape, but no durable coordinator currently produces it.
- The runtime interaction service now maps run lifecycle/text into durable
  envelopes, accumulates assistant text, and commits transcript plus terminal
  state atomically. Completed-run restart recovery uses a durable output
  artifact when present and otherwise fails closed instead of fabricating an
  empty successful transcript. Model-executor calls receive typed bounded prior
  completed pairs; failed/cancelled/timed-out partial pairs are excluded and
  harness history is refused rather than flattened. Effect-backed tool status
  is projected with stable IDs; approvals remain absent and raw tool data is
  withheld pending redaction policy.

### 1.5 The HTTP server now shares the durable core runtime

`polkagent serve` now builds one strict `PolkagentRuntime` and injects durable
agent, run, effect, event, artifact, payment, and conversation stores plus its
event bus. Black-box restart tests create an agent/run over HTTP, preserve
verified artifact content/classification/lineage, and reload them from the same
database. Tool list/detail/grant routes project the exact runtime registry with
deterministic ordering and truthful empty behavior when registration is
disabled. Immutable configured-skill reads and durable memory query/exact
lookup/non-mutating stats/atomic deletion use exact runtime-owned components.
Nine remaining skill-mutation/audit/registry routes publish and test an
explicit 501 boundary rather than falling back to in-memory implementations.
The exact runtime `InteractionService` also backs versioned create/list/load/
archive/turn/prompt/cancel and strict target/model config routes. A finite JSON
event route provides
turn-filtered durable checkpoints across restart. A separate interaction SSE
route replays then follows typed envelopes, uses durable sequence IDs and
`Last-Event-ID`, and recovers live-receiver lag after the last event actually
delivered. The older WebSocket endpoint remains a distinct run-event protocol.

The TUI does not call REST: local surfaces use the same runtime service
in-process, while remote clients use finite replay or checkpoint-aware SSE.
The service attaches bounded live delivery first, then demand-loads at most one
1,000-event durable replay page, so old checkpoints neither miss concurrent
publication nor materialize the full backlog.

### 1.6 The two ACP directions are separate adapters

```text
Downstream harness path:
Polkagent (ACP client) ──spawns/prompts──> Cursor / Goose / Kiro / OpenCode

Editor path:
Zed (ACP client) ──spawns/prompts──> polkagent acp (ACP agent server)
```

`polkagent-harness-acp` remains the downstream client. The separate
`polkagent-surface-acp` server adapter now implements the bounded ACP v1 editor
slice. Keep these responsibilities and trust boundaries separate while both
converge on the shared interaction/runtime contract.

## 2. What Roko demonstrates

Roko has two relevant user experiences.

### 2.1 Terminal interaction

With no subcommand and TTY stdin, Roko launches a unified inline chat. It has a
persistent scrollback-style terminal UI, editable prompt input, history,
streaming output, interruption, session state, and slash commands. A separate
dashboard exists, but chat is a first-class interaction rather than a monitor
that happens to contain a tiny command field.

Useful patterns to carry over:

- no-argument discoverability;
- one continuous conversation rather than repeated one-shot invocations;
- stream text/tool progress while keeping the input/session alive;
- Ctrl-C cancels the active turn without automatically destroying the session;
- command discovery from `/help`;
- model, effort, tools, MCP, context, and reset controls live at session scope.

Polkagent should support both:

- `polkagent chat` for a focused terminal conversation;
- a composer embedded in `polkagent tui` for operators who also need the
  dashboard and orchestration views.

Changing the no-argument default immediately is not necessary. Keep the TUI
default and make it actionable; provide `chat` explicitly. A later usability
test can decide whether no arguments should open Chat or the full Console.

### 2.2 ACP agent server

Roko has a dedicated `roko-acp` crate and `roko acp` command. It implements:

- an early CLI dispatch path so telemetry cannot corrupt protocol stdout;
- newline-delimited JSON-RPC over stdio;
- initialize/capability negotiation;
- new/load/list/resume/close session behavior;
- prompt streaming through `session/update`;
- cancellation;
- session config options and modes;
- dynamic available slash commands;
- tool-call updates and permission requests;
- persisted sessions;
- work-directory and config handling;
- file-only protocol logging.

It is a strong behavioral reference, but not code to copy wholesale.

Lessons from Roko's current implementation:

- Its ACP dispatch/event bridge is extremely large and mixes transport,
  provider routing, tool execution, workflow orchestration, knowledge,
  persistence, command parsing, and CLI subprocess bridging.
- Many slash commands are translated back into `roko` CLI subprocesses. That
  gives fast parity but duplicates the command catalog and weakens typed error,
  cancellation, telemetry, and transaction handling.
- Roko has separate slash-command catalogs for terminal chat and ACP, which can
  drift.
- Its own plan backlog records ACP streaming, permission, MCP, model UX, and
  slash-command wiring gaps. “Roko has ACP” should not be interpreted as “copy
  every Roko implementation choice.”

The local source audit makes that trade-off concrete: Roko's ACP adapter is a
large surface (`bridge_events.rs` is about 5.7k lines, `session.rs` about 2.4k,
and `runner.rs` about 2.2k) and directly spawns CLI/shell subprocesses in its
event bridge. Its active local plans still report 0/5 tasks complete for ACP
slash-command streaming (`P21`), 1/5 for tool permissions (`P22`), and 0/4 for
MCP passthrough (`P25`). Roko remains the UX benchmark for a persistent inline
conversation and editor discoverability; these source facts reinforce using
Polkagent's shared typed interaction/runtime boundary instead of reproducing
the adapter's internal coupling.

Polkagent should reproduce the product capability through a smaller adapter
over shared application services.

## 3. Current external contract: ACP v1 and Zed

As of 2026-08-05, the official ACP site labels v1 as latest and v2 as draft.
ACP v1 uses JSON-RPC 2.0 and the normal lifecycle is `initialize`,
`session/new` or `session/load`, `session/prompt`, streamed `session/update`
notifications, optional permission/file/terminal calls, cancellation, and a
terminal prompt result. See the [ACP v1 overview](https://agentclientprotocol.com/protocol/v1/overview).

Important current requirements and recommendations:

- Use the official Rust `agent-client-protocol` crate, which implements both
  sides of ACP and powers Zed's external-agent integration. See the
  [official Rust library page](https://agentclientprotocol.com/libraries/rust).
- All ACP file paths must be absolute.
- Slash commands are advertised with
  `available_commands_update` and invoked as ordinary prompt text such as
  `/status`. See [ACP slash commands](https://agentclientprotocol.com/protocol/v1/slash-commands).
- Session config options are now preferred over the older modes API. Use
  config options for agent/group target, model, provider/harness, reasoning or
  budget profile, and autonomy mode; expose legacy modes during the transition
  only where clients need them. See
  [ACP session config options](https://agentclientprotocol.com/protocol/v1/session-config-options).
- Tool calls are reported through structured session updates, and the agent
  may use client permission, filesystem, and terminal capabilities. See
  [ACP tool calls](https://agentclientprotocol.com/protocol/v1/tool-calls).
- Session listing is worth implementing because current Zed can import external
  agent threads.

Zed now supports custom external agents through `agent_servers`. A development
configuration would look like:

```json
{
  "agent_servers": {
    "polkagent": {
      "type": "custom",
      "command": "/absolute/path/to/polkagent",
      "args": ["acp", "--agent", "editor-agent"],
      "env": {}
    }
  }
}
```

The ACP `session/new` working directory becomes the immutable durable
interaction origin; Polkagent has no separate `--workdir` flag. The path must
be absolute, UTF-8, and lexically normalized without `.`/`..`. Every prompt,
load, and resume compares the exact stored spelling before replay or work; no
filesystem canonicalization or symlink resolution changes identity. Zed
documents custom agents and
ACP debugging at [External Agents](https://zed.dev/docs/ai/external-agents).
Zed's `dev: open acp logs` command should be part of the validation checklist.

## 4. Target product behavior

### 4.1 The product is a Console, not only a dashboard

Rename the mental model (the binary can retain `tui`):

```text
Polkagent Console
├── Conversations: prompt, stream, resume, inspect
├── Orchestration: target agent/group, parallel work, plans, cancellation
├── Operations: runs, approvals, costs, errors, audit
├── Resources: agents, skills, memory, tools, providers
└── Chain: status, governance/treasury actions, signing boundaries
```

The main TUI layout should add a conversation workspace rather than only a
one-row command palette:

```text
┌ Agents / Runs / Groups ─────┬ Active conversation ─────────────────┐
│ dev-helper       working    │ you: investigate the failing tests   │
│ gov-researcher   idle       │                                      │
│ review-team      2/3 busy   │ agent: I will inspect...             │
│                            │  • read Cargo.toml                    │
│                            │  • run cargo test                     │
│                            │  ▸ waiting for approval               │
├────────────────────────────┴───────────────────────────────────────┤
│ target: dev-helper  model: sonnet  mode: supervised  cost: $0.04  │
│ > Type a prompt, or / for commands...                              │
└────────────────────────────────────────────────────────────────────┘
```

Required interaction behavior:

- The composer is always discoverable.
- Enter submits; Shift-Enter inserts a newline where terminal support permits.
- Up/Down navigates prompt history only when appropriate.
- Tab completes commands, agents, groups, run IDs, model IDs, and skill names.
- Ctrl-C cancels the active turn; a second Ctrl-C within a short interval may
  exit only after an explicit status hint.
- The user can switch conversations while runs continue.
- Background run activity is visible without stealing input focus.
- Tool calls, approvals, output, usage, and errors appear in the transcript.
- Run detail remains available for deep operational inspection.

### 4.2 Focused terminal chat

Add:

```bash
polkagent chat
polkagent chat --agent dev-helper
polkagent chat --resume <conversation-id>
polkagent chat --group review-team
polkagent chat --model anthropic/claude-sonnet-4-6
```

The compact chat should use the same interaction/session service and command
registry as the TUI. It may use a simpler inline renderer, but its semantics
must be identical.

### 4.3 ACP usage

```bash
polkagent acp --agent editor-agent
polkagent --config /path/to/polkagent.toml acp --agent editor-agent
polkagent acp --agent editor-agent --provider anthropic --model <model> --timeout 300
```

ACP persists the validated absolute working directory supplied by `session/new`
and requires the exact same origin on prompt/load/resume; there is no
`--workdir` flag. Pre-v17 rows with unknown provenance remain generically
readable/archivable but fail closed for editor attachment. The global
`--log-file` option enables ACP-specific bounded,
rotating JSONL diagnostics with redaction and restrictive/no-follow file
handling; it deliberately excludes raw prompt and response bodies and never
writes diagnostics to protocol stdout.

From Zed, a user should be able to:

- start a Polkagent thread;
- select a configured agent or group;
- select a model/provider/harness where valid;
- send ordinary prompts;
- see streamed text, tool calls, plans, usage, and approvals;
- cancel a turn;
- invoke useful slash commands;
- load and resume previous sessions, with list/import added when the protocol
  surface supports them;
- receive clear configuration/provider readiness errors in the thread.

## 5. Shared architecture

### 5.1 Dependency direction

```text
TUI adapter ──────────────┐
Terminal chat adapter ────┤
ACP agent adapter ────────┼──> InteractionService ──> AppService / GroupService
HTTP/WS adapter ──────────┤           │                       │
One-shot CLI adapter ─────┘           ├── CommandRegistry     ├── Orchestrator
                                      ├── ConversationStore   ├── Tool/Effect pipeline
                                      └── InteractionEvents   └── EventBus / stores
```

Rules:

1. Surface adapters translate input/output; they do not implement execution.
2. Commands call typed services; they do not spawn `polkagent` subprocesses.
3. The database is persistence, not the command bus.
4. Every mutation passes through an application service.
5. Events exposed to users have stable IDs and enough structure for TUI, ACP,
   WebSocket, and audit consumers.

### 5.2 Production runtime

`RuntimeFactory` now lives in `polkagent-runtime`, and `run`, TUI, chat,
`serve`, and ACP all consume a retained factory-built runtime. The reference
shape below remains illustrative rather than an exact public API:

```rust
pub struct PolkagentRuntime {
    pub app: Arc<AppService>,
    pub interactions: Arc<InteractionService>,
    pub commands: Arc<CommandRegistry>,
    pub events: EventBus,
    pub pool: SqlitePool,
}

pub struct RuntimeOptions {
    pub config_path: Option<PathBuf>,
    pub workdir: PathBuf,
    pub provider_override: Option<String>,
    pub harness_override: Option<String>,
    pub read_only: bool,
}

impl RuntimeFactory {
    pub async fn build(options: RuntimeOptions) -> Result<PolkagentRuntime>;
}
```

The factory owns this composition boundary and must consistently:

- discover/validate config;
- open/migrate SQLite;
- build durable run/effect/event/conversation stores;
- build provider registry and executor;
- build harness registry/adapter;
- register tools and skills;
- build chain/signer dependencies;
- construct `AppService` and `InteractionService`;
- rehydrate configured agents/groups;
- recover stuck runs;
- start timeout/background services;
- expose readiness warnings without panicking.

Those surface migrations are complete. Remaining composition gaps are signer/
policy/approval, shutdown/background-worker ownership, optional API stores,
execution-scoped interaction settings, and groups; parity claims for those
capabilities remain intentionally withheld.

### 5.3 Interaction service

The surface-neutral `InteractionService` is implemented and maps its durable
identity to `ConversationId`; “session” remains reserved for overloaded
provider, harness, and editor meanings.

The `polkagent-interaction` contract crate defines this boundary, including an
attached replay-aware event stream returned with a started turn, and the
runtime supplies its durable implementation. The code below remains
illustrative of the implemented boundary plus future group/config extensions.

```rust
pub enum InteractionTarget {
    Agent(AgentId),
    Group(GroupId),
    Auto,
}

pub struct InteractionConfig {
    pub target: InteractionTarget,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub harness: Option<String>,
    pub autonomy: AutonomyMode,
    pub max_turns: Option<u32>,
    pub budget: Option<BudgetSpec>,
}

pub struct PromptRequest {
    pub conversation_id: ConversationId,
    pub content: Vec<InteractionContent>,
    pub config_overrides: InteractionOverrides,
    pub client_context: ClientContext,
}

pub struct TurnHandle {
    pub turn_id: TurnId,
    pub run_ids: Vec<RunId>,
    pub events: InteractionEventReceiver,
}
```

Required operations:

- `new_interaction`
- `list_interactions`
- `load_interaction`
- `delete_interaction`
- `prompt`
- `cancel_turn`
- `set_config_option`
- `approve` / `deny`
- `subscribe`

Prompt transaction:

1. Validate session, target, and config.
2. Persist the user message.
3. Create a durable interaction turn.
4. Resolve agent/group/auto target.
5. Start one or more linked runs.
6. Map run events into interaction events.
7. Accumulate a durable final response and usage record.
8. Persist assistant/tool messages and terminal state.
9. Emit exactly one terminal interaction event.

Do not hold a database transaction open across model execution. Persist each
state transition with idempotent turn/run IDs.

### 5.4 Data model changes

Reuse `conversations` and `conversation_messages`; add an explicit join from a
conversation turn to executions. For example:

```sql
CREATE TABLE interaction_turns (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  state TEXT NOT NULL,
  target_json TEXT NOT NULL,
  config_json TEXT NOT NULL,
  user_message_id TEXT NOT NULL,
  assistant_message_id TEXT,
  started_at TEXT NOT NULL,
  completed_at TEXT,
  error_json TEXT,
  UNIQUE(conversation_id, ordinal)
);

CREATE TABLE interaction_turn_runs (
  turn_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  role TEXT NOT NULL,
  ordinal INTEGER NOT NULL,
  PRIMARY KEY(turn_id, run_id)
);
```

Also make `conversation_id`/`interaction_turn_id` part of run creation rather
than attaching it after the run starts. This preserves correlation for the
earliest events.

Conversation metadata should store only presentation data and safe defaults,
not secrets. Session-specific provider credentials remain in the secret store
or process environment.

### 5.5 Interaction event contract

Introduce a richer stable event enum instead of forcing every surface to infer
semantics from database rows:

```rust
pub enum InteractionEvent {
    TurnStarted { turn_id: TurnId, run_ids: Vec<RunId> },
    AgentMessageDelta { run_id: RunId, text: String },
    ThoughtDelta { run_id: RunId, text: String },
    ToolCallStarted { call: ToolCallView },
    ToolCallUpdated { call: ToolCallView },
    PlanUpdated { entries: Vec<PlanEntryView> },
    ApprovalRequested { request: ApprovalView },
    UsageUpdated { usage: UsageView },
    RunStateChanged { run_id: RunId, state: RunState },
    TurnCompleted { result: TurnResult },
    TurnFailed { error: InteractionError },
    TurnCancelled,
}
```

`ToolCallView` must have a stable call ID, title, kind, status, arguments or
safe summary, content/output, and optional file locations/diff. `ApprovalView`
must carry the real effect/request ID. This change improves the TUI, API, and
ACP simultaneously.

Use a bounded live broadcast plus durable checkpoints. If a receiver reports
lag, the adapter should reload the current turn projection and continue rather
than silently dropping content.

### 5.6 Shared command registry

Build one registry used by TUI completion, terminal `/help`, ACP advertised
commands, and optional HTTP discovery.

```rust
pub struct CommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub input_hint: Option<&'static str>,
    pub category: CommandCategory,
    pub mutability: Mutability,
    pub availability: fn(&CommandContext) -> bool,
    pub handler: Arc<dyn CommandHandler>,
}
```

Commands should return typed output/events, not preformatted ANSI strings.
Each surface chooses its renderer. Commands that change session configuration
should call the same setters exposed as ACP config options.

Initial useful catalog:

| Command | Purpose | MVP? |
|---|---|---|
| `/help` | Discover available commands | Yes |
| `/status` | Session target, active runs, provider/model, budget | Yes |
| `/agents` | List/select configured agents | Yes |
| `/agent <name>` | Set current target | Yes |
| `/new [title]` | Start a conversation | Yes |
| `/resume <id>` | Load a conversation | Yes |
| `/runs` | List recent/active runs | Yes |
| `/inspect <run-id>` | Show run detail/artifacts/errors | Yes |
| `/cancel [run-id|all]` | Cancel current/specified work | Yes |
| `/approve <id>` | Approve a pending effect | Yes |
| `/deny <id> [reason]` | Deny a pending effect | Yes |
| `/model [id]` | Show/set model | Yes, plus ACP config option |
| `/provider [id]` | Show/set provider | Phase 2 |
| `/harness [id|none]` | Show/set downstream harness | Phase 2 |
| `/memory <query>` | Search relevant memory | Phase 2 |
| `/tools` | Show effective tool/capability policy | Phase 2 |
| `/cost` | Current turn/session usage and budget | Phase 2 |
| `/groups` | List orchestration groups | Phase 3 |
| `/group <name>` | Set group target | Phase 3 |
| `/plan <prompt>` | Produce/execute an orchestration plan | Phase 3 |
| `/chain status` | Chain readiness/finality | Phase 3 |

TUI-only controls such as `/quit`, `/clear`, or `/goto` may be registered as
surface-local commands, but domain commands must remain shared. ACP should
advertise only commands that are valid for the current session and runtime.

## 6. TUI implementation design

### 6.1 Event loop

Change `App::run` to async and multiplex:

- terminal events;
- render ticks;
- interaction events;
- background refresh ticks;
- task completions/toasts;
- shutdown signals.

Use `tokio::select!` and either `crossterm::event::EventStream` or a dedicated
blocking input thread feeding a channel. Never execute model/provider calls,
chain RPC, SQLite-heavy queries, or command handlers on the render thread.

### 6.2 State split

Separate state into:

- `UiState`: focus, modal, selected rows, scroll, prompt editor, notifications;
- `InteractionProjection`: conversations, transcript, active turns, tool calls;
- `OperationsProjection`: agents, groups, runs, approvals, health, usage;
- `RuntimeHandle`: shared services and channels.

The renderer reads projections only. Commands run in Tokio tasks and feed
typed results back as messages.

### 6.3 Composer

Implement a real editor widget rather than extending the memory search string:

- UTF-8/grapheme-aware cursor movement;
- multiline input;
- Home/End and word navigation;
- paste handling;
- history stored per workspace;
- completion popup;
- command hints/errors;
- max visible height with scrolling;
- draft preservation per conversation;
- optional attachment/context chips later.

Memory search can reuse the text editor component with a different submit
handler, but it must not share the same semantic action variants.

### 6.4 Approvals

After APR-05, replace direct `TuiDb::approve_effect`/`deny_effect` writes
with coordinator-backed `InteractionService` calls. Do not route the modal to
the current bus-only `AppService` methods. An approval modal must show:

- requesting agent/run/tool;
- exact operation and target;
- chain/network/account for chain actions;
- estimated cost/value/fees when available;
- policy reason and expiry;
- approve once / deny (persistent mandates are later work).

### 6.5 Live rendering and recovery

On opening a running conversation:

1. Load its durable projection.
2. Subscribe to live interaction events.
3. Reconcile event sequence/checkpoint.
4. Render deltas.
5. If lagged, reload projection and display a small recovery notice.

This is superior to tailing raw JSONL or relying only on five-second database
polling.

## 7. ACP server design

### 7.1 Crate boundary

`crates/polkagent-surface-acp` now owns:

- official ACP trait implementation;
- protocol-to-domain type conversion;
- per-client/session handles;
- backend turn and slash-command results to bounded ACP session updates;
- capability negotiation;
- stdio transport startup with protocol-only stdout and stderr diagnostics.

Durable `InteractionService` mapping, exact session IDs, prompt/cancel,
load/resume, typed text/lifecycle/usage/checkpoint/tool events, immutable origin
cwd comparison, persisted native active-agent/model options, and live
registry-backed command availability are implemented. ACP publishes the idle
catalog on new/load/resume, adds current-turn cancel while a prompt is active,
and restores the idle catalog after success, cancellation, or backend failure.
Remaining work is session list/import, permission/rich-plan events, raw tool-data
redaction, MCP/client capabilities, provider/autonomy configuration, and manual
Zed evidence. Production permission binding is explicitly APR-07. Opt-in
protocol-safe file diagnostics are implemented. It
may depend on `polkagent-service`, the interaction/command crate, and ACP SDK.
Core/service crates must not depend on ACP types.

### 7.2 CLI boot safety

Dispatch `polkagent acp` before installing any console tracing subscriber or
printing banners. In ACP mode:

- stdout is protocol only;
- diagnostics go to a configured file and/or stderr only where the client
  tolerates it;
- panic hooks must not write arbitrary text to stdout;
- startup failure should become a valid JSON-RPC error when possible;
- secrets and raw prompts must not appear in logs;
- flush one complete JSON-RPC message per frame/line as required by the SDK.

This is one of the best patterns to take directly from Roko.

### 7.3 Method mapping

| ACP operation | Polkagent operation |
|---|---|
| `initialize` | Report version, prompt capabilities, load/list support, client FS/terminal/MCP negotiation |
| `session/new` | `InteractionService::new_interaction`, honoring absolute cwd and MCP servers |
| `session/load` | Load durable conversation, replay sufficient history/projection |
| `session/list` | Page durable conversations for Zed thread import |
| session delete/close where supported | Archive/delete per retention policy; never silently destroy run/audit evidence |
| `session/prompt` | Parse content/command and call `InteractionService::prompt` |
| `session/cancel` | Cancel the active interaction turn and linked runs |
| `session/set_config_option` | Validate and update target/model/provider/harness/autonomy config |
| `session/set_mode` | Compatibility adapter to autonomy/mode config |
| permission request response | Resume the real pending effect/tool request |

Do not return from `session/prompt` until the turn reaches a terminal stop
reason, but stream updates throughout.

The shared `/new` and `/resume` command names are lifecycle documentation in
ACP, not advertised mutations: editor clients use `session/new`,
`session/load`, and `session/resume` so a prompt cannot silently replace its
current session ID. Approval commands remain unadvertised until APR-07 binds
the durable coordinator to production permission requests.

### 7.4 Event mapping

| Interaction event | ACP session update |
|---|---|
| `AgentMessageDelta` | `agent_message_chunk` |
| `ThoughtDelta` | `agent_thought_chunk` when supported/enabled |
| `ToolCallStarted` | `tool_call` |
| `ToolCallUpdated` | `tool_call_update` |
| `PlanUpdated` | `plan` |
| `UsageUpdated` | usage update/config-appropriate extension |
| `ApprovalRequested` | `session/request_permission` request to client |
| command catalog change | `available_commands_update` |
| config dependency/fallback change | `config_option_update` |
| completion/failure/cancel | final `session/prompt` stop reason or JSON-RPC error as appropriate |

An agent execution failure should normally be visible in the transcript/tool
status and end the turn with a meaningful result. Protocol misuse, invalid
parameters, unknown session, or busy session should be JSON-RPC errors.

### 7.5 Config options

Advertise ordered select options:

1. `target` (agent/group/auto)
2. `model`
3. `provider` where multiple valid providers exist
4. `harness` including executor-only
5. `autonomy` (`observe`, `supervised`, `delegated` or the project's canonical terms)
6. `budget_profile` or reasoning/effort if supported by the selected backend

Changing model/provider may change valid harnesses and vice versa; return the
complete recomputed option list. Never expose a value that is not actually
ready. Include provider readiness warnings at initialization/session creation.

### 7.6 Client capabilities and tools

MVP may execute tools in Polkagent's own runtime while reporting them to Zed.
Then add, in order:

1. `session/request_permission` for all mutating/exec/chain effects;
2. client filesystem reads/writes when negotiated and beneficial;
3. client terminals for commands so output is native to the IDE;
4. MCP server passthrough from `session/new` into Polkagent tool discovery;
5. images/embedded context only after the end-to-end provider path supports them.

Fail closed if a permission response is missing, invalid, timed out, or the
client disconnects.

## 8. Orchestration semantics

“Agent orchestration” needs explicit behavior; it should not just mean a list
of agents on screen.

### 8.1 MVP

- A conversation targets one persisted agent.
- Multiple conversations/runs may execute concurrently subject to runtime
  limits.
- The user can see all active work and switch focus.
- Cancellation and approval apply to exact run/effect identities.

### 8.2 Group target

The existing group domain is primarily in-memory coordination/execution logic.
Before exposing `/group`, add a `GroupService` with durable definitions and
execution linkage.

A group prompt must produce an explicit execution plan:

```rust
pub struct OrchestrationPlan {
    pub tasks: Vec<PlannedTask>,
    pub edges: Vec<TaskDependency>,
    pub synthesis: SynthesisPolicy,
    pub quorum: Option<QuorumPolicy>,
    pub budget: BudgetSpec,
}
```

Support staged modes rather than a magical “multi-agent” flag:

- parallel fan-out + synthesis;
- sequential handoff;
- reviewer quorum;
- leader/delegates;
- explicit DAG.

Each child task gets its own run ID and role; the interaction turn links all
runs. Stream child identity with every update so TUI/Zed can attribute output.
The final assistant message is the synthesis result, with child outputs
available for inspection.

### 8.3 Safety

- Effective grants are the intersection of user/session, group, agent, tool,
  and environment policies.
- Budgets apply at turn and child-run levels.
- One child cannot approve another child's effect unless a policy explicitly
  permits it.
- Cancelling a turn propagates to all children and records partial results.
- Chain signing always preserves explain-before-sign and exact payload review.

## 9. Delivery plan

### Phase 0 — contract and runtime extraction (P0)

- [x] Add an architecture decision record for shared interaction semantics.
- [x] Extract production `RuntimeFactory` into `polkagent-runtime`.
- [x] Migrate one-shot `run` to the factory with no behavior regression.
- [x] Migrate the TUI and ACP to one retained factory-built runtime with
  executable reuse/recovery evidence.
- [x] Make `serve` use the strict production runtime and durable core stores;
  retain a machine-readable/tested 501 boundary for missing optional adapters.
- [x] Define structured `InteractionEvent` and effect-backed tool identity.
- [ ] Carry real durable approval/effect identity end to end through the
  coordinator, checkpoint, service, and surfaces.
- [x] Add runtime startup/readiness, recovery, rehydration, durability, and
  strict/simulated-policy integration tests.

**Exit:** one-shot CLI and API can use the same production service composition.

### Phase 1 — durable single-agent interaction service (P0)

- [x] Add interaction session/turn/run-link/event migrations and SQLite adapter.
- [x] Define surface-neutral `InteractionService` traits and config/target types.
- [x] Implement the single-agent durable `InteractionService` with persisted
  target and model config against the production runtime.
- [x] Prepare caller-identified correlated runs without events, link the turn,
  subscribe, and only then activate the earliest lifecycle event.
- [x] Atomically persist user input before execution and assistant transcript
  plus terminal turn state exactly once.
- [x] Add bounded live subscription with durable replay and explicit
  lag/recovery checkpoints, including durable run-event backfill.
- [x] Define the typed MVP command registry/parser and structured output contracts.
- [x] Implement typed handler entry points for all 12 MVP commands;
  unavailable capabilities fail explicitly until their service ports are
  composed.
- [ ] Wire the applicable handler set through terminal/TUI/ACP; HTTP exposes
  typed resource operations over the same ports rather than slash-command
  text. ACP currently executes a truthful durable help/status/agents/agent/
  runs/inspect/model/current-turn-cancel subset; native ACP operations own
  new/load/resume lifecycle.
- [x] Add cancellation, caller-ID retry, transcript causality, lag/replay,
  multi-page, and restart/pre-activation crash service tests.
- [x] Carry real effect identity through structured tool start/update replay and
  restart tests across terminal/TUI/ACP.
- [ ] Carry durable approval identity through service approve/deny and add the
  coordinator/checkpoint restart matrix.
- [x] Assemble the newest 32 completed pairs among the latest 1,000 prior
  records as typed model-executor input, append the current user once, omit
  partial failed/cancelled/timed-out pairs, and reject string-only harness
  history without role flattening.
- [x] Support persisted and prompt-scoped model selection on cloned prepared
  specs with canonical same-provider validation, strict harness evidence, and
  no cross-session shared-spec races.
- [ ] Support execution-scoped provider/harness/autonomy/max-turn/budget
  configuration without shared-spec races.

**Exit:** the current headless suite proves create, contextual model-executor
prompt, stream, retry, cancel, persist, load, replay, and harness refusal
without TUI or ACP types. Phase 1 remains open for real effect approval/deny and
a role-safe harness/session context contract.

### Phase 2 — focused terminal chat (P0/P1)

- [x] Add the single-agent `polkagent chat` CLI surface over the retained
  runtime and durable interaction service.
- [x] Implement line-oriented multiline composition, durable transcript resume,
  typed streaming, checkpoint resubscribe, and Ctrl-C cancellation.
- [x] Execute the truthful help/status/agents/agent/runs/inspect/cancel/new/
  resume/model subset through shared slash-command handlers.
- [x] Persist `/agent` per conversation with restart/next-run proof, active/
  ambiguous/unknown refusal, and no AgentSpec/turn/run/event mutation.
- [x] Add persisted execution-scoped `/model` selection with same-provider
  validation, restart/isolation proof, and no AgentSpec or transcript mutation.
- [ ] Add richer interactive editing plus provider/harness/autonomy selectors;
  unsupported configuration currently fails explicitly.
- [x] Test non-TTY stdout/stderr behavior, restart resume, refusal, and SIGINT
  cancellation. The surface does not enter raw/alternate-screen mode.

**Exit:** Polkagent has a useful Roko-like interactive terminal session.

### Phase 3 — actionable TUI (P0/P1)

- [x] Convert the TUI event loop to a bounded async channel-driven architecture
  with lossless key/paste backpressure, coalesced resize/tick signals, awaited
  runtime completions, tracked shutdown, and separately single-flight
  background SQLite/chain projections.
- [x] Pass one retained `PolkagentRuntime`, not only `SqlitePool`, into `App`.
- [x] Add the first single-run Console workspace and composer.
- [x] Preserve root explicit config selection through the TUI run bootstrap.
- [x] Upgrade the composer for extended-grapheme cursor/edit/delete behavior,
  multiline input, bounded history/draft restoration, bracketed paste,
  128-KiB whole-grapheme bounds, and scrolling viewport behavior.
- [x] Add registry-derived slash-command discovery/help/completion with aliases,
  input hints, selection, and acceptance.
- [x] Execute `/help`, `/status`, `/agents`, `/agent`, `/new`, `/resume`,
  `/runs`, and `/inspect` through the shared command executor with structured
  pending/completed/failed output, stale-result guards, exact conversation
  switching/scope, and no accidental model turn.
- [x] Consume the runtime-owned run read model and shared formatter without
  adapter SQL or event-loop blocking. Same/foreign/missing, restart,
  redaction/bounds, stale-selection, and concurrent-activity fixtures preserve
  the selected viewport and exact cancellation path.
- [x] Persist exact active-agent selection per conversation in chat and Console;
  reject unknown/inactive/ambiguous/active-work/stale changes, preserve transcript
  and model, survive restart, route the next run to the selected target, and
  create no turn/run/event or AgentSpec mutation.
- [x] Execute `/model [id]` through the same durable service path, render the
  current selection in status/header, and prove conversation isolation,
  restart, typed refusal, and stale-result guards without creating a turn.
- [x] Add an explicit durable conversation selector. `s` asynchronously lists
  a bounded set of same-agent summaries, loads the exact selected transcript,
  excludes foreign-agent sessions, remains usable while unrelated work runs,
  serializes control requests, and has restart coverage.
- [x] Start/cancel correlated turns through `InteractionService`.
- [x] Render live text, lifecycle/tool-name progress, usage, and errors for the
  Console-owned run.
- [x] Recover interaction text/lifecycle/usage after lag and reload transcript
  after restart.
- [x] Render the real effect-backed tool lifecycle with one durable effect-
  derived identity and safe status through lag/restart. Raw arguments/output are
  withheld pending one redaction/classification contract.
- [ ] Render rich plans and durable approvals once the runtime produces them.
- [ ] Replace direct approval/denial database writes.
- [ ] Add create-agent modal; durable list/select is available through
  `/agents` and `/agent`, while full agent-spec editing is deferred.
- [x] Add reducer, key mapping, TestBackend rendering, and durable fake-run
  bootstrap tests.
- [x] Add real Unix PTY proof for terminal restoration after normal exit,
  ordinary error, and caught panic.
- [x] Add deterministic headless event-loop, resize/coalescing, blocked/failing
  projection, stale-generation, and bounded-shutdown tests.
- [x] Add the first simultaneous-run surface slice: eight correlated
  agent/conversation turns, 32 retained redaction-safe activity summaries,
  bounded per-conversation viewports, `[`/`]` switching, selected exact cancel,
  same-conversation duplicate refusal, deterministic terminal eviction,
  backpressure, and shutdown reaping. Controlled two-execution and TestBackend
  fixtures cover the surface; group planning/child orchestration remains Phase 5.
- [x] Add focused real-runtime restart/history/follow-up/cancellation tests plus
  stale-history race and UTF-8 output-bound regressions.

**Exit:** a user can enter the TUI, select/create an agent, prompt it, observe
work, approve/deny, cancel, and prompt again without leaving.

### Phase 4 — ACP server and Zed MVP (P0/P1)

- [x] Add official ACP Rust SDK and `polkagent-surface-acp` crate.
- [x] Add protocol-safe early `polkagent acp` dispatch.
- [x] Implement the bounded initialize/new/prompt/cancel slice.
- [x] Map ACP session IDs exactly to durable conversation UUIDs and implement
  new/load/resume plus restart recovery through `InteractionService`.
- [ ] Implement session list/import if supported by a future SDK/surface; the
  pinned stable ACP v1 SDK exposes neither import nor Polkagent session list.
- [x] Map shared text/lifecycle/usage/checkpoint `InteractionEvent` envelopes
  to ACP session updates with lag replay and exact terminal reconciliation.
- [x] Map effect-backed tool start/update events with stable effect-derived IDs
  to native ACP tool messages, including lag/restart replay and safe terminal
  status mapping.
- [ ] Map durable approval and rich plan events once the runtime produces them.
- [x] Advertise the initial MVP slash commands.
- [x] Move discovery/parsing/help/aliases onto the shared registry and execute
  the truthful durable subset, including current-prompt cancellation.
- [x] Derive ACP availability from the live registry context: publish the idle
  catalog on new/load/resume, add `/cancel` only while a normal prompt is
  active, and restore idle discovery after success, cancellation, and backend
  error. Document `/new`/`/resume` as native ACP lifecycle operations and keep
  `/approve`/`/deny` withheld pending APR-07.
- [x] Expose `/runs` and `/inspect <run-id>` in ACP through the runtime-owned
  conversation-scoped read model and shared bounded formatter; prove durable
  IDs before and after subprocess restart with the official ACP client.
- [x] Expose native active-agent/model config options and route `/agent` and
  `/model` through the same persisted interaction configuration.
- [x] Persist and compare immutable lexical origin cwd before prompt/load/resume;
  reject cross-workspace/relative/traversal/legacy-unknown attachment before
  events, turns, or runs.
- [ ] Expose group/auto targets plus autonomy/provider/harness options only
  after execution-scoped semantics can be guaranteed. Active-agent target
  selection is already persisted and advertised.
- [ ] Implement the APR-07 production tool-permission round-trip; the existing
  protocol-only harness is not a coordinator binding.
- [x] Write the Zed custom-agent setup guide.
- [x] Add an official-SDK subprocess protocol fixture.
- [x] Add official-client active-run cancellation/stop-reason coverage and
  verify the reason-bearing durable terminal run state/timestamp.
- [x] Add official-client new/prompt/same-turn-retry/restart/load/follow-up/
  resume coverage proving exact conversation/turn/run identities, persisted
  model config, replay-on-load, no replay-on-resume, and no duplicate retry run.
- [x] Reuse one `RuntimeFactory` composition and prove abandoned-run recovery
  through an official-client restart fixture.
- [x] Prove successful-session stdout purity and missing-explicit-config
  startup failure with empty stdout, a stderr diagnostic, and exit code 4.
- [x] Prove provider/backend redaction, panic containment and payload
  suppression, unavailable-provider startup failure, and protocol stdout
  purity.
- [x] Add opt-in protocol-safe bounded JSONL diagnostics with known-pattern
  redaction, restrictive Unix permissions, `O_NOFOLLOW`, non-regular-path
  refusal, rotation, and subprocess stdout/content-safety proofs.
- [x] Forward bounded real runtime text before the terminal response, reconcile
  exact final text without duplication, and emit usage only from real terminal
  counts plus a known model context window. Provider HTTP/SSE streaming remains
  a separate adapter capability.
- [ ] Validate manually with Zed ACP logs and the full acceptance matrix.

**Exit:** Polkagent can be added as a Zed custom external agent and complete a
real tool-using, cancellable, permission-gated prompt.

### Phase 5 — orchestration groups and richer IDE support (P1/P2)

- [ ] Add durable `GroupService` and CRUD.
- [ ] Add group-target interaction turns and child-run attribution.
- [ ] Add plans and synthesis/quorum results to event contract.
- [ ] Add TUI group/worktree/parallel views.
- [ ] Add ACP plan updates and dynamic group commands/options.
- [ ] Add client filesystem/terminal and MCP passthrough.
- [ ] Add registry packaging after custom-agent stability.

**Exit:** Polkagent is a real multi-agent orchestration surface in both TUI and
Zed, not merely a single-agent chat wrapper.

## 10. Test and acceptance plan

### Shared service

- A prompt creates exactly one durable interaction turn.
- A single-agent turn links exactly one run; a group turn links all child runs.
- User message precedes execution; assistant message is persisted exactly once.
- Reload after process restart reconstructs transcript and terminal state.
- Cancel is idempotent and propagates to linked runs.
- Duplicate/replayed events do not duplicate transcript text or terminal events.
- Receiver lag triggers projection recovery.

### TUI/chat

- Prompt editor handles Unicode, paste, multiline, history, resize, narrow terminal.
- Submission does not block rendering/input.
- User can switch conversations while work continues.
- Live events are attributed to the correct agent/run.
- Approval calls service and unblocks the exact waiting effect.
- Panic/error always restores terminal state.
- Headless TestBackend snapshots cover input, streaming, tools, approval, failure.

### ACP

- Protocol stdout contains only JSON-RPC frames. The official-client subprocess
  fixtures assert JSON on every observed successful-session stdout line, and
  the missing-explicit-config fixture asserts empty stdout.
- Initialize negotiates current v1 capabilities correctly.
- New session requires an absolute cwd and, until Phase 5 passthrough exists,
  explicitly rejects supplied MCP servers and additional roots.
- Prompt streams before its terminal response.
- Tool calls have stable IDs and legal status transitions.
- Permission allow/deny/timeout/disconnect are all tested; default is deny.
- Cancel terminates linked runs and returns a correct stop reason. The bounded
  official-client fixture proves this for one active run and its durable
  terminal state; propagation across future grouped runs remains open.
- Load/resume survive server restart. List/import remains unsupported until the
  pinned SDK/surface exposes it and then requires Zed proof.
- Dynamic command and config-option updates conform to ACP schema.
- Unknown session, busy session, invalid config, and provider failure are distinct.
- Zed manual smoke: add custom agent, prompt, tool call, approve, cancel, resume,
  inspect `dev: open acp logs`.

### Success metrics

- First successful prompt from TUI in under 30 seconds for an initialized user.
- First successful Zed prompt using only documented setup.
- Time-to-first-token is visible and does not wait for the five-second DB poll.
- Zero duplicated command implementations across TUI/chat/ACP.
- Zero direct database mutations from UI adapters.
- All user-visible active work has cancel and inspect paths.

## 11. Risks and explicit non-goals

### Risks

- **Runtime duplication:** one-shot run, TUI, chat, ACP, and `serve` share the
  production factory; TUI/chat/HTTP/ACP consume the headless interaction
  service. A bounded grantless registered-tool/effect loop is composed;
  approval/policy/resume and crash recovery remain incomplete.
- **Event loss:** TUI/chat/HTTP/ACP use durable interaction replay and stable
  effect-backed tool projection; approval/plan projection remains incomplete.
- **Protocol drift:** ACP v2 is draft. Pin the official SDK, test v1, and isolate
  conversions in the adapter.
- **SQLite concurrency:** Zed may spawn processes while TUI/API is open. Enable
  WAL/busy timeouts, keep transactions short, and test multi-process access.
- **Approval bypass:** current TUI direct DB writes must not be reused.
- **Command drift:** one registry is mandatory; avoid Roko-style duplicated
  terminal/ACP catalogs and CLI subprocess mapping.
- **False orchestration:** group UI before durable group execution would be
  decorative. Ship single-agent interaction first, then explicit group plans.
- **Secrets/logging:** ACP logs and transcripts must use the existing
  classification/redaction rules.

### Not MVP

- ACP v2 implementation before v1 works in Zed.
- A custom Zed extension; custom external-agent configuration is sufficient.
- Remote ACP transport; stdio is the first target.
- Full-screen agent-spec YAML editor.
- Arbitrary agent-generated recursive delegation without explicit budgets.
- Images/audio before provider and storage paths support them end-to-end.
- Replacing every existing operational tab.

## 12. Recommended file/crate changes

| Area | Suggested change |
|---|---|
| Workspace | `polkagent-surface-acp`, `polkagent-interaction`, and `polkagent-runtime` now exist; converge every executable surface on them |
| `polkagent-service` | Replace bus-only approval with coordinator-backed interaction mutations; integrate group service |
| `polkagent-run` | Add approval checkpoint/CAS pause-resume without regressing effect-backed tool identity and interaction correlation |
| `polkagent-conversation` | Treat as durable transcript store under interaction service |
| `polkagent-store-sqlite` | Session/turn/run-link/event persistence and the V18 approval/checkpoint coordinator are composed; keep surface code behind the serialized store operations |
| `polkagent-cli/src/main.rs` | Early ACP dispatch, one-shot/TUI/chat/ACP/serve runtime convergence, and ACP-safe bounded diagnostics exist |
| `polkagent-cli/src/commands/chat.rs` | Single-agent durable line-mode chat with persisted per-conversation agent/model selection exists; add richer editing/config only after execution semantics are truthful |
| `polkagent-cli/src/tui/` | Durable prompt/cancel/history/session/agent/model selection, shared commands, safe tool status, bounded simultaneous activity switching, async input, and background monitoring workers exist; add approvals/rich plans and group/child-run orchestration |
| `polkagent-cli/src/commands/serve.rs` | Shared durable core runtime plus skill reads and all four memory routes exist; compose the remaining published 9-route optional boundary one truthful family at a time |
| `polkagent-harness-acp` | Keep as downstream ACP client; do not turn it into the server crate |
| Docs | ACP/Zed, durable terminal chat, TUI, and HTTP interaction guidance plus successful restarted cross-surface evidence exist; attach manual Zed and active-turn permission/cancel evidence |

## 13. Source trail

Primary local Polkagent evidence:

- `crates/polkagent-cli/src/main.rs`
- `crates/polkagent-cli/src/cli.rs`
- `crates/polkagent-cli/src/commands/run.rs`
- `crates/polkagent-cli/src/commands/serve.rs`
- `crates/polkagent-cli/src/tui/app.rs`
- `crates/polkagent-cli/src/tui/input.rs`
- `crates/polkagent-cli/src/tui/db.rs`
- `crates/polkagent-service/src/app.rs`
- `crates/polkagent-service/src/lifecycle.rs`
- `crates/polkagent-runtime/`
- `crates/polkagent-interaction/`
- `crates/polkagent-run/src/orchestrator.rs`
- `crates/polkagent-run/src/progress.rs`
- `crates/polkagent-conversation/`
- `crates/polkagent-store-sqlite/src/conversation_store_impl.rs`
- `crates/polkagent-store-sqlite/src/interaction_store_impl.rs`
- `crates/polkagent-group/`
- `crates/polkagent-harness-acp/src/lib.rs`
- `crates/polkagent-api/src/routes/ws.rs`
- `prd/archive/2026-08-05/superseded-plans/PRD-18-INTERACTIVE-TUI.md`
- local-only ignored evidence:
  `tmp/archive/2026-08-05/audit-evidence/ux-tui-issues.md`

Primary local Roko comparison:

- `crates/roko-cli/src/main.rs`
- `crates/roko-cli/src/chat_inline.rs`
- `crates/roko-cli/src/chat_session.rs`
- `crates/roko-cli/src/repl.rs`
- `crates/roko-acp/src/handler.rs`
- `crates/roko-acp/src/session.rs`
- `crates/roko-acp/src/bridge_events.rs`
- `crates/roko-acp/src/builtin_tools.rs`
- `docs/v2/ACP-INTEGRATION-GUIDE.md`

External primary references:

- [ACP v1 overview](https://agentclientprotocol.com/protocol/v1/overview)
- [ACP Rust SDK](https://agentclientprotocol.com/libraries/rust)
- [ACP slash commands](https://agentclientprotocol.com/protocol/v1/slash-commands)
- [ACP session config options](https://agentclientprotocol.com/protocol/v1/session-config-options)
- [ACP tool calls](https://agentclientprotocol.com/protocol/v1/tool-calls)
- [Zed external agents](https://zed.dev/docs/ai/external-agents)

## Final recommendation

The terminal, TUI, HTTP, and ACP slices are shipped and intentionally bounded.
`RuntimeFactory` plus the single-agent, model-selectable durable interaction/
store/event/command service now exist. One-shot run, TUI, chat, ACP, and `serve`
share that runtime; TUI/chat/HTTP/ACP share the durable interaction lifecycle.
The TUI now supports bounded simultaneous turns across independently selected
agent/conversation activities, but it does not yet build or execute group plans.
Next implement the durable approval coordinator/checkpoint packet in
[`APPROVAL-PAUSE-RESUME-DESIGN.md`](APPROVAL-PAUSE-RESUME-DESIGN.md), connect
permissions with crash-safe resume, complete manual Zed evidence, define
role-safe harness history, expand truthful command coverage, and compose durable
group/child-run orchestration on top of the proven activity surface.
