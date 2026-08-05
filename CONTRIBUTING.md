# Contributing to Polkagent

Thank you for your interest in contributing to Polkagent. This document
covers the development setup, workflow, and conventions used in the project.

## Development Setup

### Prerequisites

- **Rust 1.89+** (the workspace MSRV). Install via [rustup](https://rustup.rs/).
- **SQLite 3** development headers (usually bundled; the `rusqlite` dependency
  uses the `bundled` feature).
- **Git** for version control.

Optional tooling:

```bash
# Linting and formatting (included with rustup)
rustup component add clippy rustfmt

# Security auditing
cargo install cargo-audit cargo-deny

# Fuzz testing (requires nightly)
cargo install cargo-fuzz
```

### Building

```bash
# Check the entire workspace compiles
cargo check --workspace

# Build all crates in debug mode
cargo build --workspace

# Build the CLI binary in release mode
cargo build --release -p polkagent-cli
```

### Testing

```bash
# Run the full test suite
cargo test --workspace

# Run tests for a specific crate
cargo test -p polkagent-core

# Run slow / ignored tests
cargo test --workspace -- --ignored

# Run property-based tests
cargo test -p polkagent-core --test proptests

# Run store contract tests
cargo test -p polkagent-store-sqlite --test store_contracts
```

### Linting

```bash
# Clippy with workspace lints
cargo clippy --workspace -- -D warnings

# Format check
cargo fmt --all -- --check
```

## Making Changes

### Branch Workflow

1. Create a feature branch from `main`:
   ```bash
   git checkout -b feat/short-description
   ```
2. Make your changes, keeping commits focused and atomic.
3. Ensure all checks pass locally before pushing.
4. Open a pull request against `main`.

### Pull Request Guidelines

- **One feature or fix per PR.** Keep PRs focused and reviewable.
- **Tests required.** Every new feature or bug fix must include tests. The CI
  pipeline will reject PRs with failing tests.
- **Describe the "why."** The PR description should explain the motivation, not
  just list changed files.
- **Link issues.** If the PR addresses an issue, reference it with
  `Closes #123` or `Fixes #123`.

### Code Style

- Follow existing patterns in the codebase. When in doubt, look at how
  neighboring code handles a similar concern.
- All code must be **clippy-clean** at the workspace lint level defined in
  `Cargo.toml`.
- Format with `cargo fmt` before committing.
- `unsafe` code is denied workspace-wide. Do not use it.

### Commit Messages

Follow this convention:

```
<type>(<scope>): <short summary>

<optional body explaining why, not what>
```

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `ci`

Scope is typically the crate name without the `polkagent-` prefix, e.g.:

```
feat(core): add RunId display formatting
fix(store-sqlite): handle concurrent WAL checkpoint
test(effect): add proptest for idempotency key collisions
```

### Adding a New Crate

1. Create the crate directory under `crates/`.
2. Add it to `[workspace.members]` in the root `Cargo.toml`.
3. Use `workspace = true` for `edition`, `rust-version`, `license`, and
   `repository` in the crate's `Cargo.toml`.
4. Add `[lints] workspace = true` to inherit workspace lint settings.
5. Use workspace dependencies wherever possible.

## Architecture Notes

Polkagent follows a hexagonal (ports and adapters) architecture. Traits in
`-trait` crates define ports; concrete implementations live in adapter crates
(e.g., `polkagent-store-sqlite`, `polkagent-executor-anthropic`). See
`ARCHITECTURE.md` for a detailed overview.

Key invariants that must be preserved in all contributions:

- **INV-01:** The signer never sees model-modified data. Only user-approved
  bytes reach the signer.
- **INV-02:** An EffectIntent is persisted BEFORE any I/O is attempted.
- **INV-03:** A crash never silently repeats an irreversible external action.
- **INV-04:** Unknown outcomes remain visibly unknown until explicitly resolved.

## Getting Help

- Open a GitHub issue for bug reports and feature requests.
- Use GitHub Discussions for questions and design conversations.
