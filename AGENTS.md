# Repository Guidelines

## Project Overview

Holon is a Personal Knowledge & Task Management system built in Rust. It maintains live bidirectional sync with external systems (Todoist, org-mode files, the filesystem) and enables unified queries across all sources. The workspace is split into library crates, frontend crates, and experiments.

## Project Structure

```
holon/
├── crates/                  # Core library crates
│   ├── holon/               # Top-level integration crate
│   ├── holon-api/           # Public API types and traits
│   ├── holon-core/          # Domain model and core logic
│   ├── holon-engine/        # Petri-net based execution engine
│   ├── holon-frontend/      # Shared frontend abstractions
│   ├── holon-macros/        # Procedural macros
│   ├── holon-orgmode/       # Org-mode parser/sync
│   ├── holon-filesystem/    # Filesystem sync provider
│   ├── holon-todoist/       # Todoist sync provider
│   ├── holon-mcp-client/    # MCP client integration
│   └── holon-integration-tests/  # End-to-end & PBT suites
├── frontends/               # UI frontends (each is a separate workspace member)
│   ├── gpui/                # GPUI desktop frontend
│   ├── tui/                 # Terminal UI frontend
│   ├── ply/                 # Ply frontend
│   ├── mcp/                 # MCP server frontend
│   └── flutter/             # Flutter/Dart + flutter_rust_bridge
├── experiments/             # Spikes and prototypes (not production)
├── tools/                   # Internal build/dev tooling
├── assets/                  # Icons and static assets
├── scripts/                 # Shell scripts for coverage and CI
├── Cargo.toml               # Workspace manifest with shared dependencies
├── justfile                 # Task runner (see below)
└── deny.toml                # cargo-deny license and advisory config
```

All shared dependency versions are declared once under `[workspace.dependencies]` in the root `Cargo.toml`. Individual crates reference them with `.workspace = true`.

## Build, Test, and Development Commands

The project uses [`just`](https://github.com/casey/just) as a task runner. Run `just` with no arguments to list all available recipes.

| Command | What it does |
|---|---|
| `just build` | Builds the full workspace |
| `just test` | Runs all workspace unit/integration tests |
| `just clippy` | Runs Clippy across all targets |
| `just fmt-check` | Checks formatting without modifying files |
| `just lint` | Runs all quality checks (fmt, clippy, deny, machete, jscpd) |
| `just deny` | Audits dependencies for vulnerabilities and license issues |
| `just machete` | Finds unused dependencies |
| `just pbt <name>` | Runs a property-based test suite (`general`, `petri`, `orgmode`, `loro`) |
| `just pbt-all` | Runs all PBT suites sequentially |
| `just watch <ui>` | Hot-reloads a frontend (`gpui`, `tui`, `ply`) |
| `just coverage` | Collects runtime code coverage |

For faster test runs, prefer `cargo nextest` (configured in `Nextest.toml`):

```sh
cargo nextest run                   # all tests
cargo nextest run -p holon-core     # single crate
cargo nextest run --profile ci      # CI profile (sequential, retries enabled)
```

The Rust toolchain is pinned to **nightly** via `rust-toolchain.toml`.

## Coding Style & Naming Conventions

- **Formatting**: `rustfmt` with the config in `rustfmt.toml` (imports and modules are reordered alphabetically). Run `cargo fmt` before committing.
- **Linting**: Clippy is enforced with `-D warnings` in CI. Fix all warnings before opening a PR.
- **Error handling**: Use `anyhow` for application-level errors and `thiserror` for library error types. Avoid `.unwrap()` outside of tests.
- **Naming**: Follow standard Rust conventions — `snake_case` for functions/variables, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- **Async**: Use `tokio` for async runtimes. Prefer `async-trait` for trait objects that need async methods.
- **Dependencies**: Always use `workspace = true` when referencing a dependency already declared in the root `Cargo.toml`. Never duplicate version pins in individual crates.

## Testing Guidelines

The project uses three complementary testing strategies:

1. **Unit & integration tests** — standard `#[test]` and `#[tokio::test]` in each crate, plus integration tests in `crates/holon-integration-tests/`.
2. **Property-based tests (PBTs)** — via `proptest` and `proptest-state-machine`. PBT suites live in `tests/*_pbt.rs` files and are run with `just pbt <name>`.
3. **BDD / acceptance tests** — via `cucumber`. See `CUCUMBER_SETUP.md` for setup details.

**Conventions:**
- Name test functions descriptively: `test_<behaviour>_when_<condition>`.
- Use `serial_test` for tests that cannot run concurrently.
- Use `tempfile` for any test that touches the filesystem.
- PBTs are slow; do not run them in the default `just test` invocation — use `just pbt` explicitly.

## Commit & Pull Request Guidelines

Commits follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>: <short description>

# Common types used in this repo:
feat:     new feature or capability
fix:      bug fix
refactor: code restructuring without behaviour change
chore:    tooling, dependencies, CI changes
docs:     documentation only
test:     adding or updating tests
```

**Pull requests:**
- Keep PRs focused — one logical change per PR.
- Run `just lint` locally before pushing; CI will reject PRs that fail any check.
- Reference related issues or design documents (e.g. `ARCHITECTURE.md`, `VISION.md`) in the PR description where applicable.
- New public API surface should include doc comments (`///`).

## Security & Dependency Management

- `cargo deny check` (via `just deny`) enforces allowed SPDX licenses and checks for known advisories. The allowed set is defined in `deny.toml`.
- Do **not** add dependencies with licenses outside the allow-list without updating `deny.toml` and justifying the addition.
- Some frontends (`waterui`, `dioxus`, `blinc`) are excluded from the workspace due to known upstream compatibility issues — see the comments in the root `Cargo.toml` before attempting to re-enable them.

## Agent-Specific Instructions

- Read `ARCHITECTURE.md` and `ARCHITECTURE_PRINCIPLES.md` before making structural changes.
- The `experiments/` directory is intentionally unstable — do not depend on it from production crates.
- `workspace-hack` is managed by `cargo hakari`; do not edit it manually.
- When adding a new crate, register it in the root `Cargo.toml` `[workspace]` members list and add an entry under `[workspace.dependencies]` for any new external dependency it introduces.
