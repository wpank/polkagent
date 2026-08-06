# Troubleshooting

Start with the shortest diagnostic loop:

```bash
polkagent version
rustc --version
polkagent config path
polkagent config validate
polkagent doctor
polkagent status
```

Add `-v` for debug logs or `-vv` for trace logs. Logs may contain identifiers,
paths, provider diagnostics, and tool metadata, so review and redact them before
sharing.

## Installation and build problems

### Rust is too old

Symptom:

```text
package requires rustc 1.89 or newer
```

Check and update:

```bash
rustc --version
rustup update stable
```

The declared minimum supported Rust version is 1.89. A newer stable release is
recommended for local development unless you are specifically testing MSRV.

### `polkagent` is not found

Confirm Cargo's bin directory is on `PATH`:

```bash
cargo install --path crates/polkagent-cli
command -v polkagent
```

If you are in the repository and do not want to install globally:

```bash
cargo run -p polkagent-cli -- --help
```

### The binary does not match the source checkout

An installed or previously built binary can be older than the Markdown you are
reading. Compare:

```bash
polkagent version
git rev-parse --short HEAD
cargo run -p polkagent-cli -- --help
```

Use the source-built command while developing, or reinstall after rebuilding.

## Configuration problems

### Polkagent is using an unexpected config file

Show discovery paths and the resolved value:

```bash
polkagent config path
polkagent config show
polkagent config get database.sqlite.path
```

Configuration merges built-in defaults, user config, project config, and
environment overrides. An exported `POLKAGENT_*` variable wins over files.

### Agents from another project appear

The generated project config uses a user-level SQLite path by default. That is
useful for sharing agents across working directories, but surprising in an
isolated demo.

Set an explicit project database:

```bash
export POLKAGENT_DATABASE_SQLITE_PATH="$(pwd)/.polkagent/polkagent.db"
polkagent agent list
```

Do this before creating agents. Pointing at a different path intentionally
selects a different database.

### Environment override has no effect

Check the exact variable name in [Configuration](configuration.md), then start
a new process from the shell where it is exported:

```bash
env | rg '^POLKAGENT_'
polkagent config show
```

Arrays replace rather than append. A list-valued environment override may
replace all file-configured values.

### Config parses but runtime startup fails

Validation checks schema and values; startup can additionally fail when a
referenced policy directory, provider secret, database, or network dependency
is unavailable.

```bash
polkagent -vv doctor
polkagent -vv status
```

For policy errors, verify that opt-in policy loading points to one safe,
existing TOML file and that unknown fields are not present.

## Provider and model problems

### “No API key or local model configured”

This is informational when you intended to use simulated responses. For a real
model, export the matching provider key:

```bash
export ANTHROPIC_API_KEY="<YOUR_KEY>"
# or OPENAI_API_KEY, GEMINI_API_KEY, OPENROUTER_API_KEY, …
```

Then run:

```bash
polkagent doctor
polkagent run --agent-id <AGENT> --no-harness \
  --prompt "Reply with a short connectivity check" --timeout 60
```

### Provider works, but the model is rejected

Use the canonical provider/model form expected by the agent and executor:

```text
anthropic/claude-sonnet-4-6
openai/gpt-4o
gemini/gemini-2.5-flash
```

Provider catalogs change. The definitive local answer is the configured model
list and `polkagent --help`/subcommand help for your checkout, not an old copied
example.

### The wrong harness is selected

Harness auto-detection can choose an installed external agent. To isolate the
model executor, use:

```bash
polkagent run --agent-id <AGENT> --no-harness --prompt "…"
```

To test a specific harness, pass `--harness <NAME>` and confirm the binary is
on `PATH`. See [Harnesses](harnesses.md).

### A request hangs or times out

Bound it and increase diagnostics:

```bash
polkagent -vv run --agent-id <AGENT> --no-harness \
  --prompt "…" --timeout 60
```

Check provider reachability, DNS, proxy settings, provider rate limits, and the
configured connect/first-token/request timeouts. A timeout is not proof that an
external side effect did not happen; effect recovery preserves indeterminate
outcomes where applicable.

## Database problems

### “database is locked” or slow SQLite writes

SQLite permits concurrent readers but serializes writes. Check whether several
Polkagent processes point at the same file:

```bash
polkagent config get database.sqlite.path
polkagent status
```

Let the other writer finish or use separate databases for unrelated demos.
Increase `database.sqlite.busy_timeout_ms` only after understanding the
contention; a longer wait does not remove it.

Do not delete `-wal` or `-shm` files from a live database. Follow the offline
backup procedure in [Deployment](deployment.md#offline-sqlite-backuprestore-runbook).

### Migration fails

Preserve the database and logs. Do not manually edit schema version rows.

```bash
polkagent -vv doctor
sqlite3 /exact/path/to/polkagent.db 'PRAGMA integrity_check;'
sqlite3 /exact/path/to/polkagent.db 'PRAGMA foreign_key_check;'
```

Run SQLite commands only against an exact, verified path and preferably an
offline copy. Report the originating version, target version, and full sanitized
error.

### Data appears missing after restart

Most often, the restarted process resolved a different database or config.
Compare before and after:

```bash
polkagent config path
polkagent config get database.sqlite.path
polkagent inspect db
```

For Docker, confirm the named volume and Compose project are the same. A new
anonymous volume is a new database.

## CLI and terminal problems

### A flag from an example is rejected

The generated help for your binary wins:

```bash
polkagent --help
polkagent run --help
polkagent agent create --help
```

Global flags may be accepted both globally and within subcommands by the
current CLI, but placing them before the subcommand is the least ambiguous:

```bash
polkagent --format json agent list
```

### TUI display is corrupted

Try a simpler rendering mode:

```bash
NO_COLOR=1 polkagent tui
polkagent --no-color tui
```

Confirm `TERM` describes your terminal and avoid piping the TUI. If the process
was force-killed while in raw mode, restore the terminal with:

```bash
stty sane
reset
```

### TUI approval view says authority is unavailable

This is the expected fail-closed default. Ordinary startup does not invent a
principal. Approval-enabled local surfaces require the full documented
tenant/workspace/principal tuple and an applicable strict policy. See
[CLI: TUI](cli.md#tui) and [Safety](safety.md).

### Durable chat resume fails

Check all three identity boundaries:

- the conversation UUID is complete;
- the selected agent exists and is active;
- the resumed process points at the same SQLite database.

```bash
polkagent agent list
polkagent config get database.sqlite.path
polkagent chat --agent <AGENT> --resume <CONVERSATION_ID>
```

An interaction is scoped to its durable target and provenance; Polkagent fails
closed instead of silently attaching an incompatible session.

## API problems

### Server starts on the wrong address

`polkagent serve` defaults to `0.0.0.0:8080`. CLI `--host` and `--port` override
file settings for that invocation:

```bash
polkagent serve --host 127.0.0.1 --port 9090
```

Confirm with:

```bash
curl --fail http://127.0.0.1:9090/health/live
```

### `401 Unauthorized`

The deployment has authentication enabled. Add the configured API key or
bearer token to protected requests:

```bash
curl --header 'X-API-Key: <KEY>' http://127.0.0.1:9090/api/v1alpha1/agents
```

Never put credentials in query strings. Public health/discovery endpoints and
protected resource endpoints intentionally have different policies.

### `405 Method Not Allowed` in read-only mode

`--read-only` rejects mutating methods. Use GET projections, restart without
read-only mode in an authorized environment, or move the mutation to an
appropriate process. Do not work around it with direct database edits.

### `501 Not Implemented`

This is often an intentional composition boundary, not a routing bug. The
OpenAPI contract can describe an optional route while the ordinary runtime
declines to install the required durable store or approval authority.

Check [API reference](api.md), [runtime/API composition](runtime-api-composition.md),
and [implementation status](../prd/STATUS.md).

### API create-run returns agent not found

The path requires the agent UUID returned by `POST /agents`, not its name:

```bash
agent_id="$(curl --silent http://127.0.0.1:9090/api/v1alpha1/agents | jq -r '.data[0].id')"
```

Use that UUID in `/agents/$agent_id/runs`.

### WebSocket or SSE reconnect misses events

Use the checkpoint/cursor emitted by the protocol you are consuming. The run
event socket, command socket, and interaction SSE stream use distinct cursor
contracts; do not pass a cursor from one to another.

Diagnostic and ephemeral frames are best-effort. Durable replay covers durable
frames only. See [Outbox and events](outbox-events.md).

## ACP and Zed problems

### Zed reports invalid JSON-RPC

ACP stdout must contain protocol JSON only. Do not redirect logs to stdout.
Start ACP with diagnostics disabled (default) or with an explicit log file:

```bash
polkagent --log-file /tmp/polkagent-acp.jsonl acp --agent <AGENT>
```

### Session load works in one directory but fails in another

ACP persists an immutable lexical working-directory origin and verifies it on
load/resume. Launch the adapter from the exact same workspace path. Symlinked,
relative, or traversal-equivalent paths are not silently accepted.

### Approval controls are missing

Native ACP permission choices appear only when the process has explicit local
approval authority and a prompt reaches an approval-required effect. `/approve`
and `/deny` are intentionally not ACP slash commands; the editor's native
permission request carries the decision.

See [ACP and Zed](acp-zed.md) for the full trust boundary.

## Chain and tool problems

### Chain status or metadata request fails

Check the endpoint, network, TLS, and runtime compatibility:

```bash
polkagent chain status
polkagent chain metadata
```

Runtime upgrades can invalidate cached assumptions. Preserve the endpoint,
genesis hash, runtime version, metadata hash/version, and block context in a
bug report.

### Tool is listed but not executed

Registration, model advertisement, policy authorization, and product-surface
composition are separate gates. Check:

1. Is the tool in the runtime registry?
2. Does the selected model support the required tool-call format?
3. Does the agent declare the needed capability?
4. Does policy allow it or require approval?
5. Does this surface compose the handler and authority?

See [Tools and skills](tools-and-skills.md#current-registered-tool-execution-boundary).

## What to include in a useful bug report

Provide:

- the exact command with secrets and sensitive paths removed;
- `polkagent version`, `rustc --version`, OS, and architecture;
- the short Git revision for source builds;
- resolved config paths and relevant non-secret values;
- the exact surface, agent/run/turn/effect IDs where safe;
- expected behavior, actual behavior, and whether it reproduces;
- sanitized logs from `-v` or `-vv`;
- whether the provider, harness, chain, database, and signer were real or fake.

For vulnerabilities, do not open a public issue. Follow
[`SECURITY.md`](../SECURITY.md).
