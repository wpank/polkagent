# Getting Started

## Prerequisites

- Rust 1.80+ (MSRV)
- Optional: Docker (for containerized deployment)

## Installation from Source

```bash
git clone https://github.com/nicovince/polkagent.git
cd polkagent
cargo install --path crates/polkagent-cli
```

Verify the installation:

```bash
polkagent version
```

## Initialize a Project

Run the following in your project directory:

```bash
polkagent init
```

This creates a `.polkagent/` directory with a default configuration file at `.polkagent/polkagent.toml`. To overwrite an existing configuration, pass `--force`.

## Setting Up Provider API Keys

Polkagent auto-detects providers from environment variables. Set one of the following:

```bash
# Anthropic (recommended)
export ANTHROPIC_API_KEY="sk-ant-..."

# OpenAI
export OPENAI_API_KEY="sk-..."

# Google Gemini
export GEMINI_API_KEY="..."

# OpenRouter
export OPENROUTER_API_KEY="..."
```

When polkagent detects an API key environment variable, it automatically creates a provider configuration with sensible defaults. No config file editing is required.

For explicit provider configuration, see [Configuration](configuration.md).

## Creating Your First Agent

```bash
polkagent agent create my-agent \
  --model anthropic/claude-sonnet-4-6 \
  --description "My first polkagent"
```

List all agents:

```bash
polkagent agent list
```

Show details for a specific agent:

```bash
polkagent agent show my-agent
```

## Running Your Agent

```bash
# Interactive run
polkagent run --agent-id my-agent --prompt "What is referendum 1234?"

# JSON output
polkagent run --agent-id my-agent --prompt "Query my balance" --json

# Override provider or model for a single run
polkagent run --agent-id my-agent --prompt "Hello" --provider openai --model gpt-4o

# Set a timeout (in seconds)
polkagent run --agent-id my-agent --prompt "Research topic" --timeout 120
```

## Launching the TUI

```bash
polkagent tui
```

The TUI is built on ratatui using the ROSEDUST design system. Configure the theme and effects in your config file:

```toml
[tui]
theme = "dark"              # dark | no_color | high_contrast
atmospheric_effects = true
```

## Running Health Checks

```bash
polkagent doctor
```

This verifies provider connectivity, database status, and overall system health.

## Next Steps

- [CLI Reference](cli.md) — full command documentation
- [Configuration](configuration.md) — all configuration options
- [Providers](providers.md) — provider setup details
- [Tools & Skills](tools-and-skills.md) — available tools and how to extend them
