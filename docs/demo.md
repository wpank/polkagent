# Demos

These demos are ordered from zero-risk local exploration to real provider and
HTTP integrations. Each one states what it changes and what a successful run
proves.

## Before you begin

You need:

- Rust 1.89 or newer;
- a Unix-like shell for the commands as written (PowerShell equivalents work,
  but use its environment-variable syntax);
- the Polkagent repository for fixture- and script-based demos;
- optional `jq`, `curl`, `sqlite3`, Docker, and `websocat` where called out.

Install the current workspace binary:

```bash
cargo install --path crates/polkagent-cli
polkagent version
```

> [!TIP]
> If you are changing the source, use `cargo run -p polkagent-cli -- …` instead
> of reinstalling after every edit. Replace `polkagent` in the examples with
> that command prefix.

## Demo 1: offline CLI tour

**Time:** about five minutes  
**Needs an API key:** no  
**External I/O:** none  
**Writes:** an isolated directory under `/tmp`

This demo proves installation, configuration discovery, migrations, agent
creation, a durable run, JSON output, and inspection. Without a provider key,
Polkagent deliberately returns a deterministic simulated response.

### 1. Create an isolated workspace

```bash
demo_dir="$(mktemp -d /tmp/polkagent-demo.XXXXXX)"
cd "$demo_dir"
polkagent init .
export POLKAGENT_DATABASE_SQLITE_PATH="$demo_dir/polkagent.db"
```

The database override matters: the generated config otherwise points to the
normal user data directory, so it may show agents from other projects.

### 2. Validate configuration

```bash
polkagent config path
polkagent config validate
polkagent doctor
```

`doctor` may report missing provider credentials. That is expected in this
offline demo.

### 3. Create an agent

```bash
polkagent agent create demo-researcher \
  --model anthropic/claude-sonnet-4-6 \
  --description "Isolated documentation demo" \
  --max-turns 4 \
  --timeout 30

polkagent agent list
polkagent agent show demo-researcher
```

### 4. Execute a simulated run

```bash
polkagent run \
  --agent-id demo-researcher \
  --prompt "Reply with a one-line hello" \
  --no-harness \
  --timeout 15 \
  --json \
  --wait
```

Expected behavior includes this diagnostic:

```text
No API key or local model configured. Using simulated responses.
```

The response content is intentionally generic. The useful evidence is the
durable `run_id`, agent link, terminal result, and token accounting.

### 5. Inspect what was stored

```bash
polkagent status
polkagent inspect db
polkagent inspect agent demo-researcher
```

Use `polkagent inspect --help` to explore the run/effect/artifact inspection
commands available in your checkout.

### What this demo does not prove

- model-provider connectivity or response quality;
- live Polkadot RPC behavior;
- tool execution;
- approval or signing safety;
- production deployment readiness.

The directory is disposable. Polkagent does not automatically delete it, so
you can inspect the SQLite database and exported JSON after the demo.

## Demo 2: talk to a real provider

**Time:** about ten minutes  
**Needs an API key:** yes  
**External I/O:** HTTPS to your selected model provider  
**May incur cost:** yes

Continue from Demo 1 or initialize a normal project.

### 1. Set one provider credential

Choose one:

```bash
export ANTHROPIC_API_KEY="<YOUR_KEY>"
# export OPENAI_API_KEY="<YOUR_KEY>"
# export GEMINI_API_KEY="<YOUR_KEY>"
# export OPENROUTER_API_KEY="<YOUR_KEY>"
```

Do not put the key in `polkagent.toml`, prompts, screenshots, shell scripts, or
commits. Shell environment variables can still be visible to child processes;
use your platform's secret manager for serious deployments.

### 2. Confirm provider resolution

```bash
polkagent config validate
polkagent doctor
```

### 3. Run a bounded prompt

```bash
polkagent run \
  --agent-id demo-researcher \
  --provider anthropic \
  --model anthropic/claude-sonnet-4-6 \
  --no-harness \
  --prompt "Explain what a Polkadot OpenGov track controls in three bullets. Mark anything you are unsure about." \
  --timeout 60
```

If you chose a different provider, change both provider and model. Provider
IDs, model IDs, and configured provider-entry IDs are distinct; consult
[Providers](providers.md) when resolution fails.

### 4. Continue as a durable conversation

```bash
polkagent chat --agent demo-researcher --title "Provider demo"
```

Try:

```text
/help
Give me a concrete example.
Which parts of your answer should I verify against chain state?
/runs
```

The chat prints a durable conversation ID. Exit with Ctrl-D and resume it:

```bash
polkagent chat --agent demo-researcher --resume <CONVERSATION_ID>
```

### What this demo proves

- provider selection and direct model execution;
- streaming/final output behavior;
- durable transcript and run correlation;
- restart-resumable conversation state.

It still does not prove that every model claim or chain fact is correct.

## Demo 3: explore the TUI with sample data

**Time:** about five minutes  
**Needs an API key:** no  
**External I/O:** no provider call during seeding  
**Writes:** `~/.polkagent-demo/polkagent.db`

The repository includes `scripts/demo.sh`, which creates synthetic agents,
runs, events, and effects for visual exploration.

Requirements:

```bash
command -v polkagent
command -v sqlite3
```

Seed and launch:

```bash
./scripts/demo.sh
```

Or separate the phases:

```bash
./scripts/demo.sh --seed
./scripts/demo.sh --tui
```

Reset only the demo database and seed it again:

```bash
./scripts/demo.sh --reset
```

> [!WARNING]
> `--reset` removes `~/.polkagent-demo/polkagent.db`. The script does not target
> the normal Polkagent database, but inspect the path before running it if you
> have repurposed that directory.

### Suggested TUI tour

1. Open F1 Dashboard and note agent/run totals.
2. Open F2 Agents, move with `j`/`k`, and press Enter for detail.
3. Open F3 Runs and inspect completed, running, failed, and cancelled examples.
4. Open F5 Timeline for a run with tool events.
5. Open F6 Approvals and observe its explicit empty/unavailable guidance. The
   synthetic legacy effects are not forged into durable principal-scoped
   approval records.
6. Open F9 Console, select an active agent, press `p`, and inspect the composer.
7. Press `q` to exit and confirm the terminal is restored cleanly.

The seeded data is illustrative. It does not assert that the displayed chain
queries actually happened.

## Demo 4: call Polkagent over HTTP

**Time:** about ten minutes  
**Needs:** `curl` and `jq`  
**External I/O:** the configured provider or selected/auto-detected harness
when a run executes

### 1. Start the server

In terminal A:

```bash
export POLKAGENT_DATABASE_SQLITE_PATH="$(pwd)/.polkagent/api-demo.db"
polkagent serve --host 127.0.0.1 --port 9090
```

The CLI flags take precedence over the bind address in the config file.

### 2. Check discovery and readiness

In terminal B:

```bash
api_base="http://127.0.0.1:9090"
curl --fail "$api_base/health/live" | jq .
curl --fail "$api_base/health/ready" | jq .
curl --fail "$api_base/openapi.json" | jq '.info'
```

### 3. Create an agent and capture its UUID

```bash
agent_id="$(
  curl --fail --silent --request POST \
    "$api_base/api/v1alpha1/agents" \
    --header 'Content-Type: application/json' \
    --data '{
      "name": "api-demo",
      "model": "anthropic/claude-sonnet-4-6",
      "description": "Created by the HTTP demo"
    }' | jq --raw-output '.id'
)"

printf 'agent_id=%s\n' "$agent_id"
```

The create-run path requires the UUID. Do not substitute the friendly name.

### 4. Start an idempotent run

```bash
run_id="$(
  curl --fail --silent --request POST \
    "$api_base/api/v1alpha1/agents/$agent_id/runs" \
    --header 'Content-Type: application/json' \
    --data '{
      "input": "Reply with a short hello",
      "idempotency_key": "docs-demo-run-1"
    }' | jq --raw-output '.id'
)"

curl --fail --silent \
  "$api_base/api/v1alpha1/runs/$run_id" | jq .
```

Repeating the same request with the same idempotency key within the supported
window should return the existing run instead of creating duplicate work.

### 5. List durable evidence

```bash
curl --fail --silent \
  "$api_base/api/v1alpha1/runs/$run_id/events" | jq .

curl --fail --silent \
  "$api_base/api/v1alpha1/runs/$run_id/artifacts" | jq .
```

For multi-turn service integrations, prefer the durable `/interactions`
routes and their checkpointed SSE stream. The exact request and response
schemas live in [`openapi.yaml`](../openapi.yaml).

### Authentication note

The default development config has authentication disabled. If it is enabled,
protected routes require one of:

```bash
--header 'X-API-Key: <KEY>'
--header 'Authorization: Bearer <TOKEN>'
```

Do not add credentials to committed demo scripts or URLs.

## Demo 5: run and compare eval suites

**Time:** varies by suite and provider  
**May incur cost:** yes, when backed by a real provider

```bash
polkagent eval list

polkagent eval run fixtures/evals/safety/suite.json \
  --output /tmp/polkagent-safety-baseline.json \
  --details

polkagent eval report /tmp/polkagent-safety-baseline.json
```

After changing a prompt, policy, provider, or model:

```bash
polkagent eval run fixtures/evals/safety/suite.json \
  --output /tmp/polkagent-safety-current.json \
  --details

polkagent eval compare \
  /tmp/polkagent-safety-baseline.json \
  /tmp/polkagent-safety-current.json
```

Record the model, provider, config, code revision, and fixture revision with any
result you intend to compare later.

## Demo 6: inspect local package history

**Time:** about five minutes once you have a package fixture

The package directory must contain exactly one `plugin.toml` or `kit.toml`.
Unsigned local development packages require an explicit trust-policy opt-in:

```bash
polkagent package --trust-policy development install ./path/to/package
polkagent package list
polkagent package get <PACKAGE_NAME>
```

Update and roll back:

```bash
polkagent package --trust-policy development update ./path/to/newer-package
polkagent package rollback <PACKAGE_NAME>
```

This demonstrates durable local package history, not runtime activation or a
cryptographic trust chain.

## Where to go next

- [Getting started](getting-started.md) for a normal installation.
- [Use cases](use-cases.md) to choose a realistic application.
- [Examples and cookbook](examples.md) for adaptable recipes.
- [Troubleshooting](troubleshooting.md) when output differs from a demo.
- [Implementation status](../prd/STATUS.md) before relying on a feature claim.
