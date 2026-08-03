# CLI Reference

## Global Flags

These flags are available on all commands.

| Flag | Description |
|------|-------------|
| `--no-color` | Disable colours and TUI effects |
| `-c, --config <PATH>` | Override config file path |
| `-v, --verbose` | Increase verbosity (`-v` = debug, `-vv` = trace) |
| `--format <FORMAT>` | Output format: `human`, `json`, `json-pretty`, `table` (default: `human`) |
| `--dry-run` | No writes |
| `--yes` | Skip approval prompts |
| `--log-file <PATH>` | Override log file; use `-` for stderr only |

---

## Commands

### `init [DIR]`

Initialize a `.polkagent/` project directory.

```
polkagent init [DIR]
```

| Flag | Description |
|------|-------------|
| `--force` | Overwrite existing project directory |

**Examples:**

```bash
polkagent init
polkagent init ./my-project
```

---

### `run`

Execute a run against an agent.

```
polkagent run -a <AGENT> -p <TEXT> [FLAGS]
```

| Flag | Description |
|------|-------------|
| `-a, --agent-id <AGENT>` | Agent name or UUID (required) |
| `-p, --prompt <TEXT>` | Prompt text (required) |
| `--json` | JSON output |
| `--wait / --no-wait` | Wait for completion (default: wait) |
| `--provider <PROVIDER>` | Override provider |
| `-m, --model <MODEL>` | Override model (e.g. `anthropic/claude-opus-4-6`) |
| `--harness <HARNESS>` | Override harness |
| `--stream / --no-stream` | Streaming mode (default: stream) |
| `--timeout <SECS>` | Cancel after N seconds (default: 300) |

**Examples:**

```bash
polkagent run -a my-agent -p "Summarize referendum 1234"
polkagent run -a my-agent -p "Query balance" --json --no-stream
polkagent run -a my-agent -p "Research" --model gpt-4o --timeout 120
```

---

### `agent`

Agent CRUD and lifecycle management.

#### `agent create <NAME>`

Create a new agent.

```
polkagent agent create <NAME> [FLAGS]
```

| Flag | Description |
|------|-------------|
| `-m, --model <MODEL>` | Model ID (default: `anthropic/claude-sonnet-4-6`) |
| `-d, --description <TEXT>` | Agent description |
| `--capability <CAP>` | Capability to assign (repeatable) |
| `--max-turns <N>` | Maximum turns per run |
| `--preferred-model <MODEL_ID>` | Preferred model ID |
| `--timeout <SECS>` | Default timeout in seconds |
| `--max-tokens-per-turn <N>` | Maximum tokens per turn |
| `--json` | JSON output |

**Example:**

```bash
polkagent agent create researcher --model anthropic/claude-opus-4-6 -d "Governance researcher"
```

#### `agent list`

List all agents.

```bash
polkagent agent list
```

#### `agent show <ID>`

Show details for an agent.

```bash
polkagent agent show <ID>
```

#### `agent delete <ID>`

Delete an agent.

```bash
polkagent agent delete <ID>
```

#### `agent start <ID>`

Start an agent.

```bash
polkagent agent start <ID>
```

#### `agent stop <ID>`

Stop a running agent.

```bash
polkagent agent stop <ID>
```

#### `agent pause <ID>`

Pause an agent.

```bash
polkagent agent pause <ID>
```

#### `agent resume <ID>`

Resume a paused agent.

```bash
polkagent agent resume <ID>
```

---

### `skill`

Skill management.

#### `skill list`

List installed skills.

```bash
polkagent skill list
```

#### `skill install <PATH>`

Install a skill from a path.

```bash
polkagent skill install <PATH>
```

#### `skill update <NAME>`

Update a skill.

```bash
polkagent skill update <NAME>
```

#### `skill remove <NAME>`

Remove a skill.

```bash
polkagent skill remove <NAME>
```

#### `skill show <NAME>`

Show details for a skill.

```bash
polkagent skill show <NAME>
```

---

### `tui`

Launch the interactive ROSEDUST terminal UI.

```bash
polkagent tui
```

---

### `serve`

Start the HTTP API and WebSocket server.

```
polkagent serve [FLAGS]
```

| Flag | Description |
|------|-------------|
| `-p, --port <PORT>` | Port to listen on (default: `8080`) |
| `--host <HOST>` | Host to bind to (default: `0.0.0.0`) |
| `--cors-origin <ORIGIN>` | Allowed CORS origin (repeatable) |
| `--read-only` | Reject all mutating requests |

**Example:**

```bash
polkagent serve --port 9090 --host 127.0.0.1
```

---

### `config`

Configuration management.

#### `config show`

Show the resolved configuration.

```
polkagent config show [--toml | --json]
```

#### `config validate [PATH]`

Validate a config file.

```bash
polkagent config validate
polkagent config validate ./my-config.toml
```

---

### `doctor`

Run system health checks. Verifies provider connectivity and database status.

```bash
polkagent doctor
```

---

### `status`

Show agent count, active runs, effect queue depth, and memory usage.

```bash
polkagent status
```

---

### `logs`

Tail the event log.

```bash
polkagent logs
```

---

### `inbox`

Manage pending effects awaiting approval.

#### `inbox list`

List pending effects.

```bash
polkagent inbox list
```

#### `inbox show <EFFECT_ID>`

Show details for a pending effect.

```bash
polkagent inbox show <EFFECT_ID>
```

#### `inbox approve <EFFECT_ID>`

Approve a pending effect.

```bash
polkagent inbox approve <EFFECT_ID>
```

#### `inbox deny <EFFECT_ID>`

Deny a pending effect.

```
polkagent inbox deny <EFFECT_ID> [--reason <TEXT>]
```

#### `inbox history`

Show effect history.

```
polkagent inbox history [--limit N]
```

---

### `explain <EXTRINSIC_HEX>`

Decode and preview a hex-encoded extrinsic.

```bash
polkagent explain <EXTRINSIC_HEX>
```

**Example:**

```bash
polkagent explain 0x2804...
```

---

### `chain`

Chain interaction and inspection.

#### `chain status`

Show chain connection status.

```
polkagent chain status [--chain <CHAIN>]
```

#### `chain metadata`

Show chain metadata.

```
polkagent chain metadata [--chain <CHAIN>]
```

#### `chain decode <HEX>`

Decode hex data.

```
polkagent chain decode <HEX> [--chain <CHAIN>]
```

#### `chain balance <ADDRESS>`

Query balance for an address.

```
polkagent chain balance <ADDRESS> [--chain <CHAIN>]
```

---

### `memory`

Agent memory management.

#### `memory search <QUERY>`

Search memories.

```bash
polkagent memory search <QUERY>
```

#### `memory list`

List memory entries.

```bash
polkagent memory list
```

#### `memory forget <ID>`

Delete a memory entry.

```bash
polkagent memory forget <ID>
```

#### `memory stats`

Show memory statistics.

```bash
polkagent memory stats
```

#### `memory export <AGENT_ID> <OUTPUT_PATH>`

Export memories for an agent to a file.

```bash
polkagent memory export <AGENT_ID> <OUTPUT_PATH>
```

#### `memory import <INPUT_PATH>`

Import memories from a file.

```bash
polkagent memory import <INPUT_PATH>
```

#### `memory sweep <AGENT_ID>`

Clean old memories for an agent.

```
polkagent memory sweep <AGENT_ID> [FLAGS]
```

| Flag | Description |
|------|-------------|
| `--dry-run` | Preview without deleting |
| `--max-age-days <N>` | Delete memories older than N days |
| `--min-relevance <FLOAT>` | Minimum relevance score to retain |
| `--max-entries <N>` | Maximum number of entries to retain |

---

### `eval`

Evaluation suites.

#### `eval run <SUITE_PATH>`

Run an evaluation suite.

```bash
polkagent eval run <SUITE_PATH>
```

#### `eval list [DIR]`

List available evaluation suites.

```bash
polkagent eval list
polkagent eval list ./suites
```

#### `eval report <REPORT_PATH>`

View an evaluation report.

```bash
polkagent eval report <REPORT_PATH>
```

#### `eval compare <BASELINE> <CURRENT>`

Compare two evaluation reports.

```bash
polkagent eval compare <BASELINE> <CURRENT>
```

---

### `auth`

API key and credential management.

#### `auth login`

Log in to a provider.

```
polkagent auth login [--provider <PROVIDER>] [--dry-run]
```

#### `auth logout`

Log out.

```
polkagent auth logout [--dry-run]
```

#### `auth whoami`

Show the current authenticated identity.

```bash
polkagent auth whoami
```

#### `auth status`

Show authentication status.

```bash
polkagent auth status
```

---

### `network`

Network endpoint status and control.

#### `network status`

Show network connection status.

```bash
polkagent network status
```

#### `network metadata`

Show network metadata.

```bash
polkagent network metadata
```

#### `network start`

Start network connections.

```bash
polkagent network start
```

#### `network stop`

Stop network connections.

```bash
polkagent network stop
```

---

### `completions <SHELL>`

Generate shell completion scripts.

```bash
polkagent completions <SHELL>
```

Supported shells: `bash`, `zsh`, `fish`, `powershell`.

**Example:**

```bash
polkagent completions bash > ~/.bash_completion.d/polkagent
```

---

### `version`

Print version information and exit.

```bash
polkagent version
```
