# ACP and Zed integration

Polkagent now has an initial ACP v1 stdio server. It can be launched by an ACP
client, create an editor session, advertise slash commands, and route a normal
prompt through Polkagent's existing `AppService` run orchestration.

This is an executable protocol slice, not a claim of complete Zed support. The
repository tests launch the real binary through the official ACP Rust client,
including cancellation while a provider request is active. A manual Zed smoke
test, durable thread import/resume, structured tool and permission updates, and
session configuration are still open.

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

The editor-provided absolute `session/new` working directory is authoritative;
there is no separate `--workdir` flag. You may omit `--agent` and select one in
the conversation with `/agents` followed by `/agent <name-or-id>`.

## Available commands

The server publishes these through ACP `available_commands_update`:

| Command | Effect |
|---|---|
| `/help` | Show the ACP command summary. |
| `/status` | Show the session ID, workspace, and selected agent. |
| `/agents` | List active agents from the configured SQLite database. |
| `/agent <name-or-id>` | Select the agent used for subsequent prompts. |

Normal text prompts start a real Polkagent run and return its accumulated text
as an ACP agent-message update before the terminal prompt response. Only one
normal prompt may be active per ACP session. `session/cancel` is mapped to the
active `AppService` run. Token-by-token ACP forwarding is still open.

## Current protocol boundary

Implemented and covered by executable protocol evidence:

- official `agent-client-protocol` v2.0 SDK using its stable ACP v1 schema;
- stdio initialize, new-session, prompt, session-update, and cancel handlers;
- absolute-cwd validation and protocol errors for unknown/busy sessions;
- text and resource-link prompts;
- slash-command discovery and handling;
- CLI early dispatch before telemetry so stdout belongs to ACP;
- subprocess stdout-line assertions that reject anything other than JSON-RPC;
- fail-closed startup for a missing explicit config: exit code 4, an empty
  stdout channel, and the diagnostic on stderr;
- an official-client subprocess test that performs initialization, command
  discovery, `/help`, and a real `AppService` run through the fake executor;
- an official-client subprocess test that holds a real provider request open,
  cancels it, receives `Cancelled`, and verifies the durable run state and
  terminal timestamp in SQLite.

Not implemented yet:

- `session/list`, `session/load`, thread persistence/import, or restart resume;
- dynamic model/provider/target/autonomy configuration options;
- structured tool calls, plans, usage, and permission request/response;
- client filesystem/terminal support and MCP-server passthrough;
- additional workspace roots (rejected explicitly) and use of cwd as model or
  filesystem context beyond session metadata;
- durable multi-turn conversation history and a shared `RuntimeFactory`;
- ACP-safe file logging plus panic and secret-redaction evidence;
- manual Zed validation, including approval, cancellation, restart, and logs.

Supplied MCP servers and additional workspace roots are rejected instead of
being silently ignored.

## Troubleshooting and verification

ACP owns stdout. Provider-selection notices go to stderr, and the tested
missing-explicit-config failure exits before writing stdout. Do not wrap the
command in a script that prints banners to stdout. In Zed, use `dev: open acp
logs` to inspect the subprocess exchange. Polkagent does not yet implement a
separate ACP file log.

Run the executable conformance slice locally:

```bash
cargo test -p polkagent-cli --test acp_stdio_e2e -- --nocapture
```

For a manual smoke, verify in order: agent appears, session opens, slash-command
completion is visible, `/status` responds, a normal prompt returns a message,
cancellation stops active work, and Zed's ACP log contains only JSON-RPC frames
on the server's stdout channel. This manual matrix remains unverified in the
repository status.
