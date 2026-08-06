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
Zed smoke test and session listing/import are still open. APR-07's native
permission path is implementation-gated and is not considered shipped until
its code gate merges; its operator contract is frozen below so it can be
reviewed and exercised consistently.

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

## Durable approval opt-in (APR-07 implementation-gated)

APR-07 defines one explicit approval authority for an entire local ACP stdio
process. The code gate is still pending merge: if `polkagent acp --help` does
not show these flags, that binary does not contain the opt-in.

The three flags are an all-or-none tuple:

```text
--approval-tenant <ID>
--approval-workspace <ID>
--approval-principal <UUID>
```

`--approval-principal` must be a non-nil UUID. Tenant and workspace IDs must be
non-empty bounded identifiers. A partial tuple, malformed UUID, nil UUID, or
invalid identifier fails startup before any protocol output is written to
stdout. Omitting all three flags leaves approval authority unbound and keeps
approval-required effects fail-closed.

After the code gate lands, an approval-enabled Zed entry has this shape:

```json
{
  "agent_servers": {
    "polkagent": {
      "type": "custom",
      "command": "/absolute/path/to/polkagent",
      "args": [
        "acp", "--agent", "editor-agent",
        "--approval-tenant", "local-tenant",
        "--approval-workspace", "zed-workspace",
        "--approval-principal", "018f4d6b-1a2b-7c3d-8e4f-0123456789ab"
      ],
      "env": {}
    }
  }
}
```

These values are stable authorization identifiers, not credentials. Do not put
API keys, tokens, passwords, private keys, seeds, mnemonics, or other secrets in
Zed JSON arguments or `env`; keep secrets outside version-controlled editor
settings and supply them through an appropriate protected local mechanism.

The approval surface is fixed internally to `acp-stdio`; it is not a user
option. Polkagent does not infer or replace any part of the authority from the
agent, working directory, configuration, database, ACP request/client metadata,
session, or a default. A single process cannot switch principals between
requests or sessions.

This is a local-process trust boundary: the OS user who owns the stdio process
and its launch configuration is trusted to assert the tuple. It is not
multi-user or multi-principal authentication and must not be treated as an
authorization layer for a shared or remotely exposed ACP service.

Approval-enabled ACP additionally requires every `session/new`, `session/load`,
and `session/resume` cwd to equal the runtime workdir exactly. That runtime
workdir is the ACP process's current directory at launch. Comparison is lexical:
there is no canonicalization or symlink resolution. Consequently, launch
Polkagent from the workspace Zed will send as its cwd; a mismatch fails before
session attachment or work begins. Without approval opt-in, the existing
immutable editor-origin cwd contract below remains unchanged.

The runtime binds the same authority to the durable interaction service and to
the approval executor. The same SQLite pool backs the coordinator and the
checkpoint/effect stores. Restart recovery therefore requires the same
database, exact authority tuple, and exact runtime workdir; it must not invent
identity from a recovered row.

An approval is presented as a native ACP permission request with exactly two
choices: `polkagent.allow_once` and `polkagent.reject_once`. There is no
remember/always grant. Protocol cancellation, disconnect, or error uses the
service cancellation path and is not a third permission choice. The request is
bound to the exact pending approval/run/tool-call identities; only bounded,
redacted metadata crosses the editor boundary, never raw tool arguments,
output, diffs, or locations. `/approve` and `/deny` remain unadvertised because
the native permission request owns this interaction.

Automated official-client coverage in the APR-07 code gate exercises
allow-once, reject-once, cancellation, exact identity checks, and pending-
approval restart/load without duplicate effects. This is not a completion
claim: the code must merge, and the manual Zed permission/restart smoke remains
unverified.

The editor-provided `session/new` working directory becomes the interaction's
immutable durable origin; there is no separate `--workdir` flag. It must be an
absolute, UTF-8, lexically normalized path with no `.` or `..` components.
Polkagent compares the exact stored spelling without filesystem
canonicalization, symlink resolution, or dependence on the subprocess launch
directory. Runtime config discovery and relative config/database paths are
still rooted at the subprocess launch directory because the runtime must be
ready before the editor sends `session/new`. You may omit `--agent`; the runtime resolves the new durable
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
| `/runs` | List up to 20 newest runs linked to this durable editor session. |
| `/inspect <run-id>` | Inspect a session-owned run through a bounded, redaction-safe projection. |
| `/model [id]` | Show or select the session model; use `default` or `inherit` to return to the selected agent's model. |
| `/cancel` | Cancel the active editor prompt (`/stop`); advertised only while a prompt is active. |

The catalog is state-aware. New, loaded, and resumed sessions first publish the
inactive subset without `/cancel`. Immediately before normal prompt execution,
the server publishes the active subset with `/cancel`; after success,
cancellation, or a backend error it republishes the inactive subset. `/help`
uses the same registry availability context, while `/help cancel` remains
available as detailed documentation and explains when the command is inactive.
The official-client subprocess suite freezes all three terminal paths so an
editor cannot retain a stale active-only command.

`/new` and `/resume` are shared Polkagent command names, but they are not
advertised as ACP slash commands because ACP owns session identity and
lifecycle. Create a new editor thread through native `session/new`; reopen an
existing durable thread through native `session/load` or `session/resume` with
its conversation ID and exact original workspace. `/help new`, `/help resume`,
and direct invocation explain those native mappings instead of pretending a
slash command can replace the current ACP session ID. `/approve` and `/deny`
remain unadvertised: after APR-07 lands, native ACP permission requests—not
slash-command mutations—bind the editor response to the durable coordinator.

`/runs` and `/inspect` use the same registry metadata, runtime read model, and
formatter as terminal chat. Inspection exposes stable run/agent/artifact IDs,
a normalized state, and at most a generic terminal error. It does not expose
prompt parameters, provider error strings, artifact bodies, or metadata. Both
commands are bounded to 20 rows. `/inspect` requires exact ownership by the
selected ACP conversation; foreign and missing run IDs are rejected with the
same not-found result.

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

Provider, group/automatic-target, autonomy, and tool controls are intentionally
not advertised. Permission authority is not a session configuration option;
APR-07 binds one explicit process-level tuple from the three CLI flags above.
Only the single-agent target and model option are wired through ACP session
configuration.

## Restart, load, and resume

Initialization advertises stable ACP v1 `loadSession` and
`sessionCapabilities.resume` support. Pass the conversation UUID returned by
`session/new` back as the session ID:

- `session/load` validates and loads that durable interaction, restores its
  persisted agent/model configuration, and replays each correlated durable
  user and assistant message through ACP message-chunk updates before returning.
- `session/resume` restores the same interaction and configuration without
  replaying earlier messages, as required by the ACP distinction.

Both requests must supply the exact durable origin cwd. The shared interaction
service verifies it before transcript replay, session attachment, turn lookup,
or run creation; every later prompt verifies it again. Mismatch, relative, and
traversal spellings fail closed, and one workspace cannot load a session
created by another. MCP servers and additional directories remain rejected.

Rows created before schema migration v17 retain `NULL` origin provenance: the
migration deliberately does not infer a cwd from the database path or process
launch directory. Those legacy interactions remain available to generic
list/load/archive operations, but ACP `session/load`, `session/resume`, and new
prompts fail closed because equality cannot be proven. There is no automatic
legacy upgrade; a future explicit import/rebinding workflow must establish
provenance under separate user authority. ACP v1 exposes no session-import
method in the pinned SDK, and Polkagent does not advertise `session/list`.

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
- durable immutable origin-cwd persistence, exact verification on prompt,
  load, and resume, lexical path validation, cross-workspace isolation, and a
  fail-closed legacy-row policy;
- text and resource-link prompts;
- shared-registry slash-command discovery, aliases, detailed help, agent
  selection, status, bounded conversation-scoped run listing/inspection, and
  active-prompt cancellation, with inactive/active/inactive catalog refreshes
  around every normal prompt;
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
  terminal timestamp in SQLite plus restoration of the inactive command catalog.
- an official-client provider-failure test that proves the backend error is
  redacted and the active-only command catalog is withdrawn before the
  protocol error returns.
- an official-client new/prompt/retry/restart/load/follow-up/resume test that
  proves one conversation identity, exact durable turn/run correlations, no
  duplicate retry run, transcript replay on load, no replay on resume, and
  persisted model configuration, including truthful inactive discovery on
  load/resume and active/inactive refresh around the restarted follow-up.
- an official-client multi-process cwd-provenance test that rejects
  cross-workspace load/resume and relative/traversal roots before work, accepts
  the exact restart origin, and rejects an unproven legacy row without adding
  events, turns, or runs.

Not implemented yet:

- `session/list` and thread import (the pinned stable ACP v1 SDK has no import request);
- dynamic provider, target, or autonomy configuration options;
- structured plans;
- the APR-07 native permission gate is pending merge, and its manual Zed
  permission/restart path remains unverified; the default authority-unbound
  path remains fail-closed;
- provider HTTP/SSE token-level streaming where the runtime currently emits a
  complete response as one `StreamingToken` event;
- client filesystem/terminal support and MCP-server passthrough;
- additional workspace roots (rejected explicitly) and use of cwd as model or
  filesystem context beyond session metadata;
- an explicit authorized import/rebinding workflow for pre-v17 interactions
  whose original cwd cannot be proven automatically;
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
usage appears without duplicating text, `/cancel` appears only during active
work and disappears after completion/failure/cancellation, cancellation stops
active work, and Zed's ACP log contains only JSON-RPC frames on the server's
stdout channel. Once the APR-07 code gate lands, repeat with the complete
approval tuple: confirm the cwd equals the process launch workdir; a pending
tool shows only allow-once and reject-once; allow executes exactly once; reject,
cancel, and disconnect perform no tool I/O; and killing/restarting the process
against the same database, tuple, and cwd recovers one pending request without
duplicating the effect. Confirm partial/nil authority startup fails with empty
stdout and that `/approve` and `/deny` are never advertised. This manual matrix
remains unverified in the repository status.
