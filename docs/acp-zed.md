# ACP and Zed integration

Polkagent has an ACP v1 stdio server backed by the same durable
`InteractionService` as its other interactive surfaces. Each `session/new`
creates exactly one durable interaction and returns its conversation UUID
unchanged as the ACP `sessionId`. Prompts become correlated durable turns and
runs; editor restart does not replace their identity or transcript.

This is an executable protocol slice, not a claim of complete Zed support. The
repository tests launch the real binary through the official ACP Rust client,
including cancellation while a provider request is active, official
`session/load`/`session/resume` across subprocess restarts, idempotent turn
retry, and durable agent/model isolation across concurrent sessions. Real typed
interaction events are forwarded before the terminal prompt response. A manual
Zed smoke test, session listing/import, and permission updates are still open.

## Prerequisites

Create and activate at least one agent:

```bash
polkagent agent create editor-agent --model anthropic/claude-sonnet-4-6
polkagent agent start editor-agent
```

Configure the provider in `polkagent.toml` or set its API-key environment
variable. With no ready provider, the current runtime deliberately falls back
to the simulated executor and reports that fact on stderr.

Build the binary and resolve its absolute path:

```bash
cargo build --release -p polkagent-cli
realpath target/release/polkagent
```

## Zed custom-agent configuration

Add a custom external agent in Zed using the UI, or add an equivalent entry to
your Zed settings. Replace the command with the absolute path printed above:

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

The editor-provided absolute `session/new` working directory is authoritative
for ACP session metadata; there is no separate `--workdir` flag. Runtime config
discovery and relative config/database paths are rooted at the subprocess launch
directory because the runtime must be ready before the editor sends
`session/new`. You may omit `--agent`; the runtime resolves the new durable
interaction to an active agent, and clients can later change it through their
native selector or `/agent <name-or-id>`. `--model` is an optional initial
persisted override; it must be valid for the selected agent's executor and
provider.

ACP file diagnostics are disabled by default. To opt in, add the global
`--log-file` option before `acp` and use a path dedicated to one Polkagent ACP
process:

```json
"args": [
  "--log-file", "/absolute/private/path/polkagent-acp.jsonl",
  "acp", "--agent", "editor-agent"
]
```

The file is structured JSONL, rotates before an append would take it past 1
MiB, and retains three generations total (the active file plus `.1` and `.2`).
On Unix, Polkagent sets the file to `0600` and a newly-created immediate parent
directory to `0700`; it does not change permissions on an existing parent.
Symlink and other non-regular destinations are rejected. Use `--log-file -` or
omit the option to keep ACP file diagnostics disabled.

## Available commands

The server publishes these through ACP `available_commands_update`:

| Command | Effect |
|---|---|
| `/help [command]` | Show registry-backed ACP command help (`/commands`). |
| `/status` | Show the session ID, workspace, selected agent, and prompt activity (`/st`). |
| `/agents` | List active agents from the configured SQLite database. |
| `/agent <name-or-id>` | Select the agent used for subsequent prompts (`/use`). |
| `/model [id]` | Show or select the session model; use `default` or `inherit` to return to the selected agent's model. |
| `/cancel` | Cancel the active editor prompt (`/stop`). |

Normal text prompts call the durable `InteractionService`, creating a stable
turn and linked run before execution. Typed `AgentMessageDelta` events become
progressive ACP agent-message updates before the terminal prompt response.
The backend-to-surface channel is bounded at 32 updates, so a slow client
applies backpressure rather than allowing unbounded buffering. The terminal
text is reconciled against the streamed prefix and only a previously unstreamed
suffix is sent, preventing duplicate output. A prefix mismatch fails closed as
a protocol error.

These updates ultimately reflect real runtime `StreamingToken` events projected
through the durable interaction stream. If the bounded stream lags, the ACP
backend reattaches after its last durable sequence checkpoint rather than
silently skipping events. The current runtime orchestrator may emit one event
containing a provider's complete response, so this does not claim HTTP/SSE
token-level streaming from every provider adapter.

Registered grantless tools use the same durable effect-backed interaction
projection as terminal chat and the Console. ACP receives a native `tool_call`
with `in_progress`, followed by `tool_call_update` with `completed` or `failed`.
The ACP tool-call ID is the exact persisted effect-intent UUID and is unchanged
across live delivery, checkpoint replay, and process restart. Arguments and raw
output are deliberately absent; only canonical registry metadata and a safe
policy/outcome summary cross the editor boundary. Refused, malformed,
unallowlisted, and approval-required calls have no durable attempt and therefore
cannot fabricate a tool update.

The server negotiates the SDK's stable ACP v1 schema. That schema has no
cancelled or unknown tool status, so those two interaction states use ACP's
non-success terminal `failed` status while safe content retains the distinction.
Polkagent does not invent a text-only pseudo-tool protocol.

When the runtime terminal event reports nonzero real token counts and the
effective model has a known configured or built-in context window, Polkagent
also sends the stable ACP `usage_update`: `used` is input plus output tokens and
`size` is that context window. It emits no usage update when either fact is
unknown. Only one normal prompt may be active per ACP session. Native
`session/cancel` and `/cancel` call `InteractionService::cancel_turn` for the
exact active durable turn. Run-ID and all-session cancellation are not
advertised by this ACP adapter.

Successful runtime-backed prompt responses include a namespaced `_meta.polkagent`
object containing the exact `conversationId`, `turnId`, linked `runIds`, and
last durable event `checkpoint`. A client that needs request-level idempotency
may supply a UUID string at `_meta["polkagent.turnId"]`; retrying the same turn
ID with identical prompt and effective configuration replays the existing turn
without creating another run, while conflicting reuse is rejected.

## Session configuration

Each `session/new` response advertises exactly two select options supported by
the pinned stable ACP v1 SDK:

| Configuration ID | Category | Values | Effect |
|---|---|---|---|
| `polkagent.agent` | `_polkagent_agent` | Active agent IDs | Persists the single-agent target for later prompts. |
| `model` | ACP `model` | Agent default, plus configured model IDs | Overrides the selected agent's model for later prompts. |

`session/set_config_option` validates values against current active agents and
configured models, rejects unknown values and changes during an active prompt,
and returns the refreshed option list. Both choices are persisted in the
durable interaction and survive ACP subprocess restart. Model selection uses
the interaction-service precedence and same-provider validation; a model that
is merely discoverable but cannot be applied by the selected backend is
rejected. Execution applies the effective model only to the cloned prepared-run
agent spec, so concurrent ACP sessions do not mutate or race on the shared
agent registry.

`/agent` and `/model` call the same durable interaction configuration methods as
native ACP configuration. ACP v1 has no server-to-client configuration-change
notification, so a slash-command change cannot proactively refresh an editor's
native selector; a later load/resume and the next prompt use the persisted
value.

Provider, group/automatic-target, autonomy, tool, and permission controls are
intentionally not advertised. Only the single-agent target and model option are
wired through the durable interaction service.

## Restart, load, and resume

Initialization advertises stable ACP v1 `loadSession` and
`sessionCapabilities.resume` support. Pass the conversation UUID returned by
`session/new` back as the session ID:

- `session/load` validates and loads that durable interaction, restores its
  persisted agent/model configuration, and replays each correlated durable
  user and assistant message through ACP message-chunk updates before returning.
- `session/resume` restores the same interaction and configuration without
  replaying earlier messages, as required by the ACP distinction.

Both requests use the absolute cwd supplied by the reconnecting client for
subsequent prompt context. Polkagent's current interaction schema does not
persist the original client cwd, so the server cannot independently compare the
new cwd with the value used at creation; ACP clients are responsible for the
protocol requirement that it match. MCP servers and additional directories are
still rejected. ACP v1 exposes no session-import method in the pinned SDK, and
Polkagent does not advertise `session/list`.

## Current protocol boundary

Implemented and covered by executable protocol evidence:

- official `agent-client-protocol` v2.0 SDK using its stable ACP v1 schema;
- stdio initialize, new/load/resume-session, prompt, session-update, and cancel handlers;
- exact conversation/turn/run identity mapping and namespaced prompt-response
  correlation metadata, including idempotent retry with a supplied turn UUID;
- durable transcript replay on load and no replay on resume;
- bounded forwarding of real runtime text deltas, exact terminal-text
  reconciliation, and conditional truthful ACP usage updates;
- native ACP `tool_call` and replacement `tool_call_update` projections with
  exact durable IDs, safe summaries, and no raw input/output fields;
- native `polkagent.agent` and standard `model` select-option discovery and
  `session/set_config_option`, with validated durable changes and same-provider
  model refusal before execution;
- absolute-cwd validation and protocol errors for unknown/busy sessions;
- text and resource-link prompts;
- shared-registry slash-command discovery, aliases, detailed help, agent
  selection, status, and active-prompt cancellation;
- CLI early dispatch before telemetry so stdout belongs to ACP;
- one shared `RuntimeFactory` composition for ACP, including file-backed SQLite
  migration, abandoned-run recovery, active-agent rehydration, provider/model
  overrides, the `AppService`, and its event bus;
- opt-in JSONL file diagnostics with bounded rotation, restrictive Unix file
  permissions, non-regular-path refusal, and known-pattern secret redaction;
- controlled diagnostic categories that omit the exercised prompt and response
  bodies, plus executable redaction and rotation probes;
- subprocess stdout-line assertions that reject anything other than JSON-RPC;
- fail-closed startup for a missing explicit config: exit code 4, an empty
  stdout channel, and the diagnostic on stderr;
- fail-closed startup for an unavailable explicitly selected provider before
  any protocol stdout, even though an unconfigured local session may explicitly
  use the reported simulated-executor fallback;
- an official-client subprocess test that performs initialization, command
  discovery, `/help`, and a real durable interaction turn through the fake executor;
- an official-client restart test that seeds an abandoned durable run, starts
  ACP, and verifies that the shared runtime factory recovered it to a terminal
  state through the same database used by the editor surface;
- an official-client subprocess test that changes two sessions through native
  configuration and `/model`, overlaps their real prompts, and proves that each
  provider request receives its own model while both durable runs use the
  selected agent;
- an official-client delayed-provider test that proves a real runtime text
  event and truthful usage update arrive before the terminal ACP response,
  verifies exact non-duplicated final text, and rejects non-JSON stdout;
- an official-client subprocess test that holds a real provider request open,
  cancels it, receives `Cancelled`, and verifies the durable run state and
  terminal timestamp in SQLite.
- an official-client new/prompt/retry/restart/load/follow-up/resume test that
  proves one conversation identity, exact durable turn/run correlations, no
  duplicate retry run, transcript replay on load, no replay on resume, and
  persisted model configuration.

Not implemented yet:

- `session/list` and thread import (the pinned stable ACP v1 SDK has no import request);
- dynamic provider, target, or autonomy configuration options;
- structured plans and permission request/response;
- provider HTTP/SSE token-level streaming where the runtime currently emits a
  complete response as one `StreamingToken` event;
- client filesystem/terminal support and MCP-server passthrough;
- additional workspace roots (rejected explicitly) and use of cwd as model or
  filesystem context beyond session metadata;
- persistence and server-side comparison of the original session cwd during load/resume;
- manual Zed validation, including approval, cancellation, restart, and logs.

Supplied MCP servers and additional workspace roots are rejected instead of
being silently ignored.

## Troubleshooting and verification

ACP owns stdout. Provider-selection notices go to stderr, and the tested
missing-explicit-config failure exits before writing stdout. Do not wrap the
command in a script that prints banners to stdout. In Zed, use `dev: open acp
logs` to inspect the subprocess exchange; when `--log-file` is configured,
inspect Polkagent's separate JSONL file for controlled lifecycle categories.

The file sink applies Polkagent's known-pattern redactor for API-key-like
values, 64-byte hex seeds, and 12/24-word lowercase mnemonic-shaped strings.
It is not an arbitrary sensitive-data classifier. Current call sites therefore
record fixed lifecycle descriptions rather than raw prompts, responses,
provider bodies, workspace paths, agent names, or command arguments. Future
diagnostic fields must preserve that boundary.

Run the executable conformance slice locally:

```bash
cargo test -p polkagent-cli --test acp_stdio_e2e -- --nocapture
```

For a manual smoke, verify in order: agent appears, session opens, the native
agent/model selectors and slash-command completion are visible, changing each
selector affects the next prompt, `/model` affects a later prompt, `/status`
responds, response chunks appear before the terminal turn response, known-model
usage appears without duplicating text, cancellation stops active work, and
Zed's ACP log contains only JSON-RPC frames on the server's stdout channel.
This manual matrix remains unverified in the repository status.
