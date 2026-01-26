# Development Guide

## First-time setup

- **Rust toolchain** — pinned via `rust-toolchain.toml`, picked up by `rustup` automatically.
- **Metal toolchain (macOS)** — required for the GPUI frontend (`holon-gpui`) because `gpui_macos` compiles Metal shaders at build time. If you see `cannot execute tool 'metal'` during `cargo build -p holon-gpui`, install it with:

  ```bash
  xcodebuild -downloadComponent MetalToolchain
  ```

  Xcode occasionally evicts this component after an update; re-run the command if the error reappears.
- **Tokio runtime for sync unit tests** — `StubBuilderServices::new()` lazily constructs a process-wide multi-threaded tokio runtime on first use. Non-`#[tokio::main]` unit tests can construct it directly and get a real `runtime_handle()` without any extra scaffolding.

## Testing with Nextest

We use [`cargo-nextest`](https://nexte.st/) as our test runner for faster parallel test execution and better output formatting.

### Installation

cargo-nextest is already installed globally. Verify with:

```bash
cargo nextest --version
```

### Basic Usage

Run all tests in the workspace:

```bash
cargo nextest run
```

Run tests for a specific package:

```bash
cargo nextest run -p holon
```

Run tests matching a pattern:

```bash
cargo nextest run feature_name
```

List all available tests without running them:

```bash
cargo nextest list
```

### Test Profiles

We have configured multiple profiles for different testing scenarios:

**`default`** - Standard development testing with pretty output and parallel execution (default)

```bash
cargo nextest run
```

**`quick`** - Fast sanity checks (60s timeout)

```bash
cargo nextest run --profile quick
```

**`ci`** - Strict CI/CD runs with JSON output, sequential execution, and retries

```bash
cargo nextest run --profile ci
```

**`dev`** - Development with verbose output and fail-fast mode (stops after first failure)

```bash
cargo nextest run --profile dev
```

### Configuration

Test runner configuration is in `Nextest.toml` in the workspace root. Key settings:

- **`test-threads`**: Number of parallel test threads (`auto` = all available CPUs)
- **`timeout`**: Individual test timeout in seconds (default: 300s)
- **`retries`**: Number of retries for flaky tests
- **`fail-fast`**: Stop after first failure
- **`output.format`**: `pretty` (default), `dot` (compact), or `json` (machine-readable)

### Combining with Code Coverage

Nextest works well with `cargo-llvm-cov` for coverage reporting:

```bash
cargo llvm-cov nextest --html --output-dir target/coverage-report
```

## Code Coverage

Code coverage helps identify dead code for elimination. We use `cargo-llvm-cov` to collect coverage data from tests.

### Prerequisites

```bash
cargo install cargo-llvm-cov
```

### Running Tests with Coverage

Run the property-based integration test with coverage:

```bash
cargo llvm-cov --test general_e2e_pbt -p holon-integration-tests --html --output-dir target/coverage-report
```

If tests fail but you still want the coverage report:

```bash
# Run tests (coverage data is collected even if tests fail)
cargo llvm-cov --test general_e2e_pbt -p holon-integration-tests 2>&1 || true

# Generate report from collected data
cargo llvm-cov report --html --output-dir target/coverage-report
```

### Viewing Coverage Reports

**HTML report** (interactive, best for exploration):
```bash
open target/coverage-report/html/index.html
```

**Text summary** (for quick overview):
```bash
cargo llvm-cov report --summary-only
```

**Holon packages only** (filter out dependencies):
```bash
cargo llvm-cov report --summary-only 2>&1 | grep -E "(^Filename|^----|^pkm/holon)" > target/coverage-report/holon-coverage-summary.txt
```

### Interpreting Results

The summary shows coverage by file with columns:
- **Regions/Cover**: Branch coverage
- **Functions/Executed**: Function coverage
- **Lines/Cover**: Line coverage (most useful for dead code detection)

**Dead code candidates**: Files with 0% line coverage are strong candidates for removal. Before removing, verify:
1. The code isn't used conditionally (feature flags, platform-specific)
2. No other tests exercise the code
3. The code isn't part of a planned feature

### Cleaning Coverage Data

```bash
cargo llvm-cov clean --workspace
```

## Log Analysis

The application logs to `/tmp/holon.log` using the `tracing` crate (format: `timestamp LEVEL module: [Component] message`).

### Scripts

**Process mining** (PM4Py) — discovers execution patterns, timing bottlenecks, sync cycle stats:

```bash
uv run scripts/analyze-log-pm4py.py /tmp/holon.log
uv run scripts/analyze-log-pm4py.py /tmp/holon.log --case-strategy sync_cycle
uv run scripts/analyze-log-pm4py.py /tmp/holon.log --min-level TRACE --export-csv /tmp/events.csv
```

Case strategies: `component` (default, groups by `[Component]` tag), `time_window` (2s proximity), `sync_cycle` (MCP sync boundaries).

**Template mining** (Drain3) — clusters log lines into templates, surfaces rare/anomalous patterns:

```bash
uv run scripts/analyze-log-drain3.py /tmp/holon.log --show-rare
uv run scripts/analyze-log-drain3.py /tmp/holon.log --min-level INFO --top 30
```

**Metric sparklines** — extracts numeric time-series (RSS memory, sync durations, tx latencies, event rate) and renders ASCII sparklines with outlier detection:

```bash
uv run scripts/analyze-log-metrics.py /tmp/holon.log
uv run scripts/analyze-log-metrics.py /tmp/holon.log --width 60
```

All scripts are self-contained uv scripts with inline dependencies — no virtualenv setup needed.

### JSON Log Format

Append `:json` to any `HOLON_LOG` destination for structured JSON output (one JSON object per line, includes span context):

```bash
HOLON_LOG=file:///tmp/holon.json:json   # JSON to file
HOLON_LOG=stderr:json                   # JSON to stderr
HOLON_LOG=stderr,file:///tmp/h.json:json  # human stderr + JSON file
```

JSON logs include span fields (`entity`, `provider`, `uri`) from instrumented sync cycles, making `jq` queries straightforward:

```bash
# Sync cycle durations by entity
jq 'select(.spans[]?.name == "sync_entity") | {entity: .spans[0].entity, ts: .timestamp}' /tmp/holon.json

# All warnings/errors
jq 'select(.level == "WARN" or .level == "ERROR")' /tmp/holon.json
```

### Span-Instrumented Operations

The MCP sync pipeline carries span context through the full cycle:
- `mcp_full_sync{provider}` — initial full sync of all entities
- `sync_entity{entity, provider}` — per-entity sync with diff stats
- `resource_fetch{uri}` — individual MCP resource read
- `subscription_resync{uri}` — notification-triggered resync

## VCS via JJ (Jujutsu)
This project uses `jj` as VCS.
A few recommendations:
* When you're doing experiments, just do a `jj new` before. That allows you to use `jj edit @-` to jump back to the previous rev, `jj abandon @` to throw away the experiment, or `jj describe` to keep a successful experiment
