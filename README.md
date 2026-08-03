# Polkagent

A Rust-first, Polkadot-native platform for building, using, and publishing AI agents.

Polkagent enables AI agents to operate within the Polkadot ecosystem — governance research, treasury operations, cross-chain transfers — with evidence-bearing safety, configurable autonomy, and crash-safe execution.

## Features

- Multi-provider LLM support (Anthropic, OpenAI, Google Gemini, OpenRouter, Perplexity, Cerebras, local/Ollama) with zero-config auto-detection from environment variables
- Polkadot-native governance tools (referendum lookup, track info, voter history, delegation info, treasury overview)
- Polkadot-native treasury tools (balance query, staking info, portfolio summary, transfer history, vesting schedule)
- Crash-safe effect pipeline with four invariants (signer isolation, intent-before-I/O, no silent duplicates, unknown stays unknown)
- Grant-based policy engine for configurable agent autonomy
- Multi-harness support (Claude Code, Codex, Cursor, Copilot, Goose, Kiro) via ACP
- ROSEDUST terminal UI (ratatui)
- REST + WebSocket API (`/api/v1alpha1`)
- Skill system with TOML manifests for extensibility
- Episodic agent memory with SQLite FTS5
- Budget enforcement (per-run and per-day USD limits)
- 74-crate hexagonal architecture with strict dependency rules

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

## Configuration

Configuration is loaded from two locations:

- `~/.config/polkagent/polkagent.toml` — global defaults
- `.polkagent/polkagent.toml` — project-local overrides (takes precedence)

Environment variables prefixed with `POLKAGENT_*` override both. See [docs/configuration.md](docs/configuration.md) for the full reference.

## Architecture at a Glance

Polkagent is organized as a Cargo workspace following a hexagonal (ports and adapters) architecture. Domain logic lives in pure crates with no I/O dependencies. External systems are accessed through narrow trait-based ports, with concrete adapters provided separately.

```
                      +-----------+
                      |    User   |
                      +-----+-----+
                            |
                +-----------+-----------+
                |  Surfaces (CLI / API) |
                +-----------+-----------+
                            |
          +-----------------+-----------------+
          |          Application Core         |
          |  +-----------+  +-------------+   |
          |  |    Run    |  |    Grant     |   |
          |  |  Manager  |  |   Resolver   |   |
          |  +-----+-----+  +------+------+   |
          |        |               |           |
          |  +-----+-----+  +-----+-----+     |
          |  |  Effect   |  |   Event    |     |
          |  |  Pipeline |  |    Bus     |     |
          |  +-----+-----+  +-----+-----+     |
          |        |               |           |
          |  +-----+-----+  +-----+-----+     |
          |  |  Artifact |  |   Outbox   |     |
          |  |  Store    |  |            |     |
          |  +-----------+  +-----------+     |
          +-----------------+-----------------+
                            |
          +-----------------+-----------------+
          |              Ports                |
          |  (executor, signer, store,        |
          |   chain, transport traits)        |
          +-----------------+-----------------+
                            |
          +-----------------+-----------------+
          |            Adapters               |
          |  SQLite  Anthropic  OpenAI  Fake  |
          |  Ollama  External-Signer  ...     |
          +-----------------+-----------------+
                            |
          +-----------------+-----------------+
          |         External Systems          |
          |  LLM APIs  Polkadot  Filesystem   |
          +-----------------------------------+
```

See [docs/architecture.md](docs/architecture.md) for a full description of the layered design, crate dependency diagram, and key invariants.

## Documentation

- [Getting Started](docs/getting-started.md)
- [Configuration](docs/configuration.md)
- [CLI Reference](docs/cli.md)
- [Architecture](docs/architecture.md)
- [Providers](docs/providers.md)
- [Harnesses](docs/harnesses.md)
- [Tools & Skills](docs/tools-and-skills.md)
- [Safety](docs/safety.md)
- [API Reference](docs/api.md)
- [Deployment](docs/deployment.md)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, coding standards, and contribution guidelines.

## Security

See [SECURITY.md](SECURITY.md) for reporting vulnerabilities.

## License

Licensed under Apache-2.0.
