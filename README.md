![CI](https://github.com/nicovince/polkagent/actions/workflows/ci.yml/badge.svg)
![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)
![MSRV](https://img.shields.io/badge/rustc-1.80+-orange.svg)
![Crates](https://img.shields.io/badge/crates-74-brightgreen)
![Tests](https://img.shields.io/badge/tests-1200%2B-brightgreen)
![Providers](https://img.shields.io/badge/providers-9-blue)

# Polkagent

**Build AI agents. Act on Polkadot. Reach across chains.**

Polkagent is a Rust-first, Polkadot-native platform for building, running, and publishing AI agents. It connects large language models to on-chain governance, treasury, and staking operations through evidence-bearing safety, configurable autonomy, and crash-safe execution. Whether you need a governance researcher that tracks referenda or a staking advisor that monitors validator performance, Polkagent gives your agents the tools to act on real chain data.

The platform is organized around three pillars. **Build** -- a 74-crate hexagonal architecture with a skill system, TOML manifests, and a built-in eval framework. **Act** -- crash-safe effect pipelines with four safety invariants, grant-based policy control, and budget enforcement. **Reach** -- multi-harness support for Claude Code, Codex, Cursor, Copilot, Goose, and Kiro via ACP, plus a REST and WebSocket API for programmatic access.

## Features

**Multi-Provider AI** -- Connect to 9 providers (Anthropic, OpenAI, Google Gemini, OpenRouter, Perplexity, Cerebras, AWS Bedrock, Azure, and local via Ollama) with zero-config auto-detection from environment variables and a built-in 13-model catalog.

**Polkadot-Native Tooling** -- Governance tools for referendum lookup, track info, voter history, delegation info, and treasury overview. Treasury tools for balance queries, staking info, portfolio summaries, transfer history, and vesting schedules.

**Safety & Crash Recovery** -- Four invariants enforced at every boundary: signer isolation, intent-before-I/O, no silent duplicates, and unknown stays unknown. The crash-safe effect pipeline guarantees that interrupted runs resume without data loss or duplicate side effects.

**Agent Orchestration** -- Multi-harness support (Claude Code, Codex, Cursor, Copilot, Goose, Kiro via ACP), extensible skills with TOML manifests, episodic memory backed by SQLite FTS5, and budget enforcement with per-run and per-day USD limits.

**Developer Experience** -- ROSEDUST interactive terminal UI built on ratatui, REST + WebSocket API (`/api/v1alpha1`), eval framework for regression testing agent behavior, and a 74-crate hexagonal architecture with strict dependency rules.

## Use Cases

### Governance Researcher

Track referenda, analyze voting patterns, and summarize proposals.

```bash
polkagent agent create researcher --model anthropic/claude-opus-4-6 -d "Governance researcher"
polkagent run -a researcher -p "Summarize referendum 1234 and list the top 10 voters"
polkagent run -a researcher -p "Show voter history for address 5GrwvaEF..."
```

### Treasury Watcher

Monitor treasury balances, track transfers, and generate portfolio reports.

```bash
polkagent agent create treasury-bot --model anthropic/claude-sonnet-4-6 -d "Treasury monitor"
polkagent run -a treasury-bot -p "What is the current treasury balance?"
polkagent run -a treasury-bot -p "Show portfolio summary and recent transfer history"
```

### Staking Advisor

Provide staking info, check vesting schedules, and review delegations.

```bash
polkagent agent create staking-advisor --model anthropic/claude-sonnet-4-6 -d "Staking advisor"
polkagent run -a staking-advisor -p "Show staking info for validator 5FHneW46..."
polkagent run -a staking-advisor -p "Check vesting schedule and delegation status for 5GrwvaEF..."
```

### Developer Agent

Integrate with coding harnesses for development workflows.

```bash
# Use with Claude Code harness
polkagent agent create dev-agent --model anthropic/claude-opus-4-6 -d "Dev assistant" \
  --capability code-review --capability refactor
polkagent run -a dev-agent -p "Review the latest PR for security issues" --harness claude-code
```

## Example Workflows

### Research a governance proposal

```bash
# 1. Initialize and create an agent
polkagent init
export ANTHROPIC_API_KEY="sk-ant-..."
polkagent agent create gov-researcher --model anthropic/claude-opus-4-6 -d "Governance analyst"

# 2. Run a research query
polkagent run -a gov-researcher -p "Summarize referendum 1234: what does it propose and who voted?"

# 3. Follow up with a deeper question
polkagent run -a gov-researcher -p "What track is referendum 1234 on? Show the track thresholds."

# 4. Launch the TUI for interactive exploration
polkagent tui
```

### Multi-provider setup

```bash
# Configure three providers via environment variables
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
export GEMINI_API_KEY="AI..."

# Create agents on different providers
polkagent agent create fast-agent --model openai/gpt-4o -d "Fast responses"
polkagent agent create deep-agent --model anthropic/claude-opus-4-6 -d "Deep analysis"

# Override provider at runtime
polkagent run -a fast-agent -p "Quick balance check for 5GrwvaEF..." --model gemini/gemini-2.5-pro
```

### API-first deployment

```bash
# 1. Start the API server
polkagent serve --port 9090 --host 127.0.0.1

# 2. Create an agent via the API
curl -X POST http://127.0.0.1:9090/api/v1alpha1/agents \
  -H "Content-Type: application/json" \
  -d '{"name": "api-agent", "model": "anthropic/claude-sonnet-4-6"}'

# 3. Start a run
curl -X POST http://127.0.0.1:9090/api/v1alpha1/runs \
  -H "Content-Type: application/json" \
  -d '{"agent_id": "api-agent", "prompt": "Summarize referendum 42"}'

# 4. Stream results via WebSocket
websocat ws://127.0.0.1:9090/api/v1alpha1/runs/<RUN_ID>/stream
```

## Quick Start

```bash
# Install from source
cargo install --path crates/polkagent-cli

# Initialize project directory
polkagent init

# Set your provider API key
export ANTHROPIC_API_KEY="sk-ant-..."

# Create an agent
polkagent agent create my-agent --model anthropic/claude-sonnet-4-6

# Run it
polkagent run --agent-id my-agent --prompt "Summarize referendum 1234"

# Or launch the TUI
polkagent tui
```

## CLI Overview

| Command | Description |
|---------|-------------|
| `init` | Initialize `.polkagent/` project directory |
| `run` | Execute a run against an agent |
| `agent` | Agent CRUD and lifecycle management |
| `skill` | Skill management (install, list, remove) |
| `tui` | Launch interactive ROSEDUST terminal UI |
| `serve` | Start the HTTP API + WebSocket server |
| `config` | Show or validate configuration |
| `doctor` | Run system health checks |
| `status` | Show agent count, active runs, effect queue depth |
| `logs` | Tail the event log |
| `inbox` | Manage pending effects awaiting approval |
| `explain` | Decode and preview a hex extrinsic |
| `chain` | Chain interaction and inspection |
| `memory` | Agent memory management |
| `eval` | Run evaluation suites |
| `auth` | API key and credential management |
| `network` | Network endpoint status |
| `completions` | Generate shell completion scripts |
| `version` | Print version |

See [docs/cli.md](docs/cli.md) for the full reference with all flags and subcommands.

## Configuration

Configuration is loaded from two locations:

- `~/.config/polkagent/polkagent.toml` -- global defaults
- `.polkagent/polkagent.toml` -- project-local overrides (takes precedence)

Environment variables prefixed with `POLKAGENT_*` override both. See [docs/configuration.md](docs/configuration.md) for the full reference.

## Architecture at a Glance

Polkagent is organized as a Cargo workspace following a hexagonal (ports and adapters) architecture. Domain logic lives in pure crates with no I/O dependencies. External systems are accessed through narrow trait-based ports, with concrete adapters provided separately.

```mermaid
graph TB
    User([User])

    subgraph Surfaces["Surfaces"]
        CLI["CLI + TUI<br/>(ROSEDUST)"]
        API["REST + WebSocket<br/>API"]
        Webhook["Webhook<br/>Surface"]
    end

    subgraph Core["Application Core"]
        RM["Run Manager"]
        GR["Grant Resolver"]
        EP["Effect Pipeline"]
        EB["Event Bus"]
        AS["Artifact Store"]
        OB["Outbox"]
    end

    subgraph Ports["Ports (Trait Interfaces)"]
        ET["Executor"]
        ST["Signer"]
        SR["Store"]
        CT["Chain"]
        TT["Transport"]
        HT["Harness"]
    end

    subgraph Adapters["Adapters"]
        direction LR
        A1["Anthropic"]
        A2["OpenAI"]
        A3["Gemini"]
        A4["SQLite"]
        A5["Subxt"]
        A6["Fake"]
    end

    subgraph External["External Systems"]
        direction LR
        LLM["LLM APIs"]
        DOT["Polkadot"]
        FS["Filesystem"]
    end

    User --> Surfaces
    Surfaces --> Core
    Core --> Ports
    Ports --> Adapters
    Adapters --> External
```

### How It Works

```mermaid
sequenceDiagram
    participant U as User
    participant S as Surface (CLI/API)
    participant R as Run Manager
    participant G as Grant Resolver
    participant E as Executor (LLM)
    participant P as Effect Pipeline

    U->>S: polkagent run -p "Summarize referendum 42"
    S->>R: Create Run (state: Created)
    R->>R: Transition to Running
    R->>G: Check grants & budget
    G-->>R: Approved
    R->>E: Send prompt to model
    E-->>R: Response with tool calls
    R->>P: Create EffectIntent (persisted before I/O)
    P->>P: Claim → Execute → Record Outcome
    P-->>R: EffectOutcome (Success)
    R->>R: Transition to Completed
    R-->>S: Final response
    S-->>U: Display results
```

### Run Lifecycle

```mermaid
stateDiagram-v2
    [*] --> Created
    Created --> Queued
    Queued --> Running
    Running --> AwaitingApproval
    AwaitingApproval --> Running
    Running --> WaitingEffect
    WaitingEffect --> Running
    Running --> Completing
    Completing --> Completed
    Running --> Failed
    Running --> Cancelled
    Running --> TimedOut
    Completed --> [*]
    Failed --> [*]
    Cancelled --> [*]
    TimedOut --> [*]
```

### Provider Ecosystem

```mermaid
pie title Provider Ecosystem
    "Anthropic" : 3
    "OpenAI" : 5
    "Google Gemini" : 2
    "Perplexity" : 2
    "OpenRouter" : 1
```

See [docs/architecture.md](docs/architecture.md) for a full description of the layered design, crate dependency diagram, and key invariants.

## Documentation

### Core Guides
- [Getting Started](docs/getting-started.md) — installation, first agent, and initial configuration
- [Configuration](docs/configuration.md) — TOML schema, environment variables, and precedence rules
- [CLI Reference](docs/cli.md) — every command, flag, and subcommand
- [Architecture](docs/architecture.md) — hexagonal design, crate map, and dependency rules

### Platform Features
- [Run Lifecycle](docs/run-lifecycle.md) — execution model, state machine, and budget enforcement
- [Safety](docs/safety.md) — the four invariants, grant policies, and crash recovery
- [Tools & Skills](docs/tools-and-skills.md) — built-in tools, TOML skill manifests, and custom skills
- [Providers](docs/providers.md) — supported LLM providers and auto-detection
- [Harnesses](docs/harnesses.md) — integrating with Claude Code, Codex, Cursor, and others

### Polkadot Integration
- [Chain Integration](docs/chain.md) — blockchain interaction, metadata management, identity types
- [Identity & Security](docs/identity-security.md) — signing stack, signer isolation, policy evaluation
- [Payments & Autonomy](docs/payments.md) — payment intents, signer isolation, autonomy levels

### Data & Observability
- [Storage](docs/storage.md) — store architecture, migrations, content-addressed artifacts
- [Memory](docs/memory.md) — episodic, semantic, and working memory
- [Outbox & Events](docs/outbox-events.md) — event system, outbox delivery, artifact lineage
- [Telemetry](docs/telemetry.md) — observability pipeline, metrics, distributed tracing

### Operations
- [API Reference](docs/api.md) — REST endpoints, WebSocket streaming, and authentication
- [Deployment](docs/deployment.md) — production setup, Docker, and reverse proxy configuration
- [Resilience](docs/resilience.md) — circuit breakers, cache-aside, rate limiting
- [Groups](docs/groups.md) — multi-agent groups, parent/child coordination

### Ecosystem
- [Plugins](docs/plugins.md) — plugin architecture, skill vs plugin, load lifecycle
- [Evals](docs/evals.md) — evaluation framework, suites, and category coverage
- [Examples](docs/examples.md) — practical workflows, policy templates, and API cookbook

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, coding standards, and contribution guidelines.

## Security

See [SECURITY.md](SECURITY.md) for reporting vulnerabilities.

## License

Licensed under Apache-2.0.
