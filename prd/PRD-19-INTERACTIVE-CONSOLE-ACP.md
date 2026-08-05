# PRD-19 — Interactive Polkagent: TUI, Terminal Sessions, Orchestration, and ACP

**Status:** active architecture and implementation plan; initial ACP server and
actionable TUI slices implemented

**Prepared:** 2026-08-05

**Scope:** current Polkagent checkout at `/Users/will/dev/par/polkagent`, compared with Roko at `/Users/will/dev/nunchi/roko/roko` and the current ACP v1/Zed documentation

**Implementation status:** `polkagent-surface-acp` and protocol-safe
`polkagent acp` dispatch implement an ACP v1 initialize/new/prompt/cancel slice,
slash-command discovery, agent selection, and `AppService`-backed prompts. An
official-SDK subprocess test proves that wire path. The F9 Console now supports
selecting an active agent, composing a prompt, starting a durable run through
the same bootstrap as `polkagent run`, viewing live output/progress/usage, and
cancelling it. Both remain bounded adapters: durable sessions, a shared
interaction/runtime composition, structured tools and permissions, and manual
Zed validation remain open.

**Supersedes:** the implementation role of archived PRD-18; unresolved work is
tracked in `IMPLEMENTATION-BACKLOG.md`

## Executive decision

### 2026-08-05 implementation checkpoint

The first useful vertical TUI slice is now present:

- `p` opens a dedicated prompt composer for the selected or first active agent;
- F9/`9` opens the Console workspace;
- Enter starts a real `AppService` run and `x` requests cancellation;
- live text, lifecycle, progress, tool names, errors, and final usage are
  projected into the Console without blocking the terminal loop;
- the durable run is selected and refreshed into the existing Runs, Run Detail,
  and Timeline views;
- the one-shot command and TUI share `start_run_inner`, so this slice does not
  shell out or manufacture database rows;
- reducer, key mapping, TestBackend rendering, and fake-executor durable-run
  tests cover the new seam.

Deliberate gaps remain: the controller composes one `AppService` per prompt
rather than receiving a long-lived `PolkagentRuntime`; it retains only the most
recent in-memory transcript; input is single-line and has no history or slash
commands; direct approval/database actions remain; restart/resume,
conversation-turn correlation, simultaneous runs, group orchestration,
attachments, and shared command/session parity with ACP are not implemented.
The architecture below remains the target rather than retroactively treating
this slice as TUI-01 completion.

Polkagent now has a useful single-run interactive TUI slice, but it is not yet
the durable, multi-turn agent product described by this PRD.

It has a substantial monitoring TUI, a runnable one-shot `polkagent run`, an
in-process execution service, streaming run events, conversation storage, an
ACP **client** used to drive other coding agents, and a separate bounded ACP
**agent server** for editors. These are valuable building blocks. The TUI and
ACP server each reach a real `AppService` run, but neither is backed by the
shared durable interaction/runtime target.

The delivered slices establish two distinct product surfaces that still need
to be completed:

1. The bounded TUI Console must become a durable interactive Polkagent session
   usable both as focused terminal chat and inside the existing TUI.
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
| Prompt Polkagent interactively | Partial | F9 Console has a single-line composer; focused `polkagent chat`, history, slash commands, and durable sessions remain |
| Prompt from inside the TUI | Implemented for one run at a time | `p` selects an active agent and opens the Console composer |
| Start or cancel a run from the TUI | Implemented for the bounded slice | Shared run bootstrap starts through `AppService`; `x` requests service cancellation |
| Approve/deny in the TUI | Partial and unsafe architecturally | It writes outcome rows directly through `TuiDb`, bypassing `AppService` |
| See live run output in the TUI | Partial | Controller projects the owned run's live event receiver; lag/restart recovery and durable transcript projection remain |
| Persist/resume human conversations | Building blocks exist, not integrated | SQLite conversation schema/store exists; prompt/run path does not use it as a session |
| Orchestrate agent groups from a user surface | Domain building blocks only | `polkagent-group` exists, but there is no CLI/TUI/service surface for it |
| Use Cursor/Goose/Kiro/OpenCode *from* Polkagent | ACP client exists and is tested | `polkagent-harness-acp` plus harness adapter crates |
| Use Polkagent *from* Zed | Protocol slice implemented; manual Zed proof pending | `polkagent acp` uses the official SDK and an executable client fixture; rich editor acceptance is still open |
| ACP slash commands/config selectors | Commands partially implemented | `/help`, `/status`, `/agents`, and `/agent` are advertised; shared registry and config options remain missing |
| REST API as a production control plane | Not yet reliable for this purpose | `serve` constructs in-memory agent/run stores and does not attach conversation storage |

## 1. What Polkagent actually has today

### 1.1 The TUI is now an actionable monitor for one active run

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

- is synchronous;
- polls keyboard input;
- polls SQLite every five seconds;
- optionally polls a chain endpoint;
- renders at an effective 10 fps (5 fps when idle);
- drains a typed controller channel for run output and task completion.

`App` owns a `SqlitePool` and `RunController`, but not a long-lived
`PolkagentRuntime`, command registry, or conversation service. The controller
legitimately starts/cancels through `AppService` and subscribes before run
creation, but it rebuilds the one-shot composition for each prompt.

The TUI is not completely read-only: approvals, denials, and memory deletion
write directly through `TuiDb`. That is a problem rather than a pattern to
extend. Approval should flow through `AppService::approve_effect` so policy,
audit, notifications, and waiting tasks observe the same transition. Starting
runs by inserting database rows would be even more dangerous and must not be
done.

### 1.2 Input now has a bounded prompt mode

`crates/polkagent-cli/src/tui/input.rs` now defines a distinct `Prompt` mode in
addition to `Normal`, memory-search `Insert`, and the still-inert `Command`
mode. It supports character entry, backspace, submit, and cancel. Remaining
editor gaps are:

- `/` invokes search, not a general slash-command prompt;
- Command mode handles only Escape;
- the prompt is single-line and append/backspace only;
- there is no history, completion, slash-command parsing, paste model, or
  grapheme-aware cursor movement;
- target selection is the highlighted or first active agent, not a modal or
  durable conversation configuration.

This is enough to initiate useful work, but not yet an IDE-quality editor.

### 1.3 The one-shot run path contains the production wiring we need

`polkagent run -a <agent> -p <prompt>` does real work. The command currently:

- resolves configuration, provider, model, and harness;
- creates the event bus before starting the run;
- constructs the chain client and tool registry;
- builds `AppService` with SQLite stores and an executor;
- rehydrates/registers an agent spec from SQLite;
- starts the run;
- streams events and supports cancellation/timeout.

That bootstrap has now been extracted to `start_run_inner` inside the run
command and is shared by the TUI's interim controller. This prevents immediate
behavior drift, but it still needs to become a production `RuntimeFactory`
used by every long-lived surface.

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

- A conversation is not the same thing as an executing turn. There is no
  application service that appends the user message, starts one or more runs,
  associates them with a conversation/turn, aggregates output, appends the
  assistant result, and exposes cancellation.
- `start_run` accepts only `(agent_id, prompt)` and does not accept a
  conversation/interaction ID or per-session execution overrides.
- `RunProgressEvent::ToolUse` contains only tool name and status. ACP needs a
  stable tool-call ID and benefits from title, kind, input, output, locations,
  and raw-content/diff information.
- The approval mapping currently manufactures a new `ApprovalId` instead of
  carrying the underlying request/effect identity through the event model.
- Text/progress/tool events are ephemeral. A receiver that lags or attaches
  late cannot reconstruct a complete transcript unless deltas are also
  accumulated into a durable turn result.

### 1.5 The HTTP server is not yet the shared runtime

`polkagent serve` currently opens the production SQLite database for effects,
but constructs `InMemoryAgentStore` and `InMemoryRunManager`. It also does not
attach the SQLite conversation store to `AppState`. The WebSocket endpoint is
primarily a subscription envelope, not a complete session command channel.

The interactive feature should not be built as “TUI calls the current REST
server.” First create a real shared runtime. Local surfaces can call it
in-process; remote surfaces can later call the same service through HTTP/WS.

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
move onto the future shared interaction/runtime contract.

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

The ACP `session/new` working directory is authoritative; Polkagent has no
separate `--workdir` flag. Zed documents custom agents and
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

ACP uses the absolute working directory supplied by `session/new`; there is no
`--workdir` flag. Although `--log-file` is a global CLI option, ACP early
dispatch currently bypasses telemetry/file-log initialization, so ACP file
logging must not be documented as supported yet.

From Zed, a user should be able to:

- start a Polkagent thread;
- select a configured agent or group;
- select a model/provider/harness where valid;
- send ordinary prompts;
- see streamed text, tool calls, plans, usage, and approvals;
- cancel a turn;
- invoke useful slash commands;
- load/import and resume previous sessions;
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

Extract the run command's bootstrap into a new runtime module/crate. Suggested
shape:

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

`RuntimeFactory` must consistently:

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

Migrate `run`, `tui`, `chat`, `serve`, and `acp` to this factory. Until that is
done, parity claims across surfaces will be false.

### 5.3 Interaction service

Add a surface-neutral service. “Session” is overloaded by provider and harness
sessions, so use `InteractionService` while mapping its durable identity to
`ConversationId`.

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

Replace direct `TuiDb::approve_effect`/`deny_effect` writes with runtime calls.
An approval modal must show:

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

Remaining work is durable `InteractionService` mapping, structured events and
permissions, session persistence/config options, and protocol-safe file
logging. It may depend on `polkagent-service`, the new interaction/command
crate, and ACP SDK. Core/service crates must not depend on ACP types.

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

- [ ] Add an architecture decision record for shared interaction semantics.
- [ ] Extract production `RuntimeFactory` from `commands/run.rs`.
- [ ] Migrate one-shot `run` to the factory with no behavior regression.
- [ ] Make `serve` use production durable stores and attach conversation store.
- [ ] Define structured `InteractionEvent`, tool-call identity, and real approval identity.
- [ ] Add runtime startup/readiness integration tests.

**Exit:** one-shot CLI and API can use the same production service composition.

### Phase 1 — durable single-agent interaction service (P0)

- [ ] Add interaction turn/link migrations.
- [ ] Add `InteractionService` and config/target types.
- [ ] Link run creation to conversation/turn at creation time.
- [ ] Persist user and assistant messages and terminal turn state.
- [ ] Add live subscription with lag/recovery behavior.
- [ ] Implement typed command registry and MVP commands.
- [ ] Add cancellation and approval service tests.

**Exit:** a headless test can create, prompt, stream, cancel, persist, load, and
resume a conversation without any TUI or ACP types.

### Phase 2 — focused terminal chat (P0/P1)

- [ ] Add `polkagent chat` CLI surface.
- [ ] Implement editable composer, history, streaming transcript, Ctrl-C cancel.
- [ ] Render shared slash-command help/completion.
- [ ] Add resume and target/model selectors.
- [ ] Test non-TTY behavior and terminal restoration after panic/error.

**Exit:** Polkagent has a useful Roko-like interactive terminal session.

### Phase 3 — actionable TUI (P0/P1)

- [ ] Convert TUI event loop to async channel-driven architecture.
- [ ] Pass `PolkagentRuntime`, not only `SqlitePool`, into `App`.
- [x] Add the first single-run Console workspace and composer.
- [x] Preserve root explicit config selection through the TUI run bootstrap.
- [ ] Upgrade the composer for Unicode cursor movement, multiline input,
  history, completion, and durable conversation selection.
- [x] Start/cancel a run through the shared one-shot `AppService` bootstrap as
  an interim vertical slice.
- [ ] Start/cancel runs through `InteractionService`.
- [x] Render live text, lifecycle/tool-name progress, usage, and errors for the
  Console-owned run.
- [ ] Render structured tools/plans/approvals and recover after lag/restart.
- [ ] Replace direct approval/denial database writes.
- [ ] Add create/select agent modal; defer full agent-spec editor.
- [x] Add reducer, key mapping, TestBackend rendering, and durable fake-run
  bootstrap tests.
- [ ] Add full event-loop, resize, simultaneous-run, restart, and resume tests.

**Exit:** a user can enter the TUI, select/create an agent, prompt it, observe
work, approve/deny, cancel, and prompt again without leaving.

### Phase 4 — ACP server and Zed MVP (P0/P1)

- [x] Add official ACP Rust SDK and `polkagent-surface-acp` crate.
- [x] Add protocol-safe early `polkagent acp` dispatch.
- [x] Implement the bounded initialize/new/prompt/cancel slice.
- [ ] Implement durable session list/load/import/resume.
- [ ] Map live interaction events to session updates.
- [x] Advertise the initial MVP slash commands.
- [ ] Move commands onto the shared registry.
- [ ] Expose target/model/autonomy config options.
- [ ] Implement session list for thread import.
- [ ] Implement tool permission round-trip.
- [x] Write the Zed custom-agent setup guide.
- [x] Add an official-SDK subprocess protocol fixture.
- [ ] Add official-client cancellation/stop-reason coverage.
- [ ] Add protocol-safe file diagnostics and startup/panic/redaction proof.
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

- Protocol stdout contains only JSON-RPC frames.
- Initialize negotiates current v1 capabilities correctly.
- New session honors absolute cwd and supplied MCP servers.
- Prompt streams before its terminal response.
- Tool calls have stable IDs and legal status transitions.
- Permission allow/deny/timeout/disconnect are all tested; default is deny.
- Cancel terminates linked runs and returns a correct stop reason.
- List/load survive server restart and work with Zed thread import.
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

- **Runtime duplication:** copying the run bootstrap into TUI/ACP will create
  inconsistent providers, tools, recovery, and policy. Phase 0 prevents this.
- **Event loss:** current text/tool events are ephemeral. Durable turn
  accumulation and lag recovery are mandatory.
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
| Workspace | Keep `polkagent-surface-acp`; add `polkagent-interaction` and `polkagent-runtime` behind clean APIs |
| `polkagent-service` | Accept linked run requests; expose all UI mutations; integrate group service |
| `polkagent-run` | Enrich progress/tool/approval identity; add interaction correlation |
| `polkagent-conversation` | Treat as durable transcript store under interaction service |
| `polkagent-store-sqlite` | Add turn/run link migrations and projections |
| `polkagent-cli/src/main.rs` | Early ACP dispatch exists; route run/TUI/chat/ACP through the runtime factory and add ACP-safe diagnostics |
| `polkagent-cli/src/tui/` | Async loop, composer, conversation projection, shared command completion |
| `polkagent-cli/src/commands/serve.rs` | Replace in-memory stores with production runtime |
| `polkagent-harness-acp` | Keep as downstream ACP client; do not turn it into the server crate |
| Docs | ACP/Zed and bounded TUI setup exist; add terminal-chat guidance and attach manual Zed acceptance evidence |

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
- `crates/polkagent-run/src/orchestrator.rs`
- `crates/polkagent-run/src/progress.rs`
- `crates/polkagent-conversation/`
- `crates/polkagent-store-sqlite/src/conversation_store_impl.rs`
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

The interim TUI and ACP slices are shipped and intentionally bounded. Next
extract `RuntimeFactory` and `InteractionService`, migrate run/TUI/ACP onto
them, then add chat, durable sessions, richer commands, tools/permissions, and
orchestration. Do not deepen the separate bootstrap seams.
