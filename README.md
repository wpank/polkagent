# Polkagent

Rust-first, Polkadot-native platform for building, using, and publishing AI agents.

## Overview

Polkagent enables AI agents to deeply understand and operate within the Polkadot ecosystem — from governance research and treasury operations to runtime engineering and cross-chain transfers. It provides evidence-bearing safety, configurable autonomy, and a permissionless extension marketplace.

### Three Pillars

- **Build** — Create agents with deep Polkadot domain knowledge, typed chain profiles, and metadata-aware tooling
- **Act** — Execute agent workflows with approval gates, budget enforcement, signer isolation, and durable effect tracking
- **Reach** — Publish and discover agents, skills, tools, and context packs through a federated marketplace

## Project Structure

```
polkagent/
├── prd/                    # Product Requirements Documents
│   ├── PRD-00-MASTER-INDEX.md
│   ├── PRD-01-VISION-PRINCIPLES-PERSONAS.md
│   ├── PRD-02-VOCABULARY-ARCHITECTURE.md
│   ├── PRD-03-EXECUTION-MODEL.md
│   ├── PRD-04-PROVIDERS-MODELS-TOOLS.md
│   ├── PRD-05-POLKADOT-INTEGRATIONS.md
│   ├── PRD-06-PCA-COMPATIBILITY.md
│   ├── PRD-07-IDENTITY-SECURITY.md
│   ├── PRD-08-PAYMENTS-AUTONOMY.md
│   ├── PRD-09-MEMORY-GROUPS-EVALS.md
│   ├── PRD-10-DATA-OBSERVABILITY.md
│   ├── PRD-11-DEPLOYMENT-CLOUD.md
│   ├── PRD-12-MARKETPLACE-EXTENSIONS.md
│   ├── PRD-13-UX-SURFACES.md
│   ├── PRD-14-APIs-SCHEMAS-CONFIG.md
│   ├── PRD-15-TESTING-ROADMAP.md
│   └── research-*.md          # Research documents
└── README.md
```

## PRD Suite

The project is specified across 16 PRDs with comprehensive implementation appendices:

| PRD | Title | Scope |
|-----|-------|-------|
| 00 | Master Index | Workspace layout, build sequence, cross-PRD dependencies, phased roadmap |
| 01 | Vision & Personas | Product pillars, persona journey maps, success metrics, competitive positioning |
| 02 | Vocabulary & Architecture | Canonical types, Rust trait definitions, layered architecture, invariant enforcement |
| 03 | Execution Model | Agent/Run state machines, effect pipeline, graph engine, crash recovery |
| 04 | Providers & Tools | Multi-provider support, cascade routing, tool system, skill packages, harnesses |
| 05 | Polkadot Integrations | Chain profiles, metadata strategy, XCM, Zombienet/Chopsticks, JAM roadmap |
| 06 | PCA Compatibility | ACK protocol, device channels, outbound lane, state import, rolling migration |
| 07 | Identity & Security | Grant resolution, Cedar policy engine, secret management, signer isolation |
| 08 | Payments & Autonomy | Asset registry, fee estimation, mandate DSL, budget enforcement |
| 09 | Memory & Evals | SQLite+FTS5+sqlite-vec storage, episode management, feeds, eval framework |
| 10 | Data & Observability | Database schemas, event system, OpenTelemetry, JSONL tailer, projections |
| 11 | Deployment & Cloud | Data/execution/control planes, Docker, Kubernetes/Helm, HA/DR |
| 12 | Marketplace | Package manifests, PubGrub resolver, sandbox isolation, trust tiers, extension SDK |
| 13 | UX Surfaces | 25-screen TUI (ratatui), ROSEDUST design system, 20 widgets, CLI, web API |
| 14 | APIs & Schemas | REST/WebSocket/SSE API, SQLite+PostgreSQL DDL, TOML config, Rust+TS SDKs |
| 15 | Testing & Roadmap | Testing pyramid, 25 security test cases, TUI tests, CI/CD, phase gates |

Each PRD includes implementation blueprints with Rust code sketches, agent-ready checklists, reference file maps to proven patterns, configuration guides, and ASCII wireframes for TUI components.

## Tech Stack

- **Language:** Rust
- **TUI:** ratatui + crossterm (ROSEDUST design system)
- **Database:** SQLite (local) / PostgreSQL (cloud)
- **AI Providers:** Anthropic, OpenAI, Google, OpenAI-compatible, local (Ollama)
- **Blockchain:** Polkadot SDK, subxt, SCALE codec
- **Policy:** Cedar (ABAC)

## License

TBD
