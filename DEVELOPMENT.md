# Development Guide

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

Test runner configuration is in `.config/nextest.toml` in the workspace root. Key settings:

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

## UI Action Latency

Measure end-to-end latency of a UI action (indent / outdent / cycle task state /
split / ...) — from the moment the action is dispatched until its result becomes
VISIBLE (the reactive row batch is applied). Instrumentation lives under the
`holon_latency` tracing target and is zero-cost unless that target is enabled.

```bash
just measure-latency            # 16 random cases (default)
just measure-latency 40         # more cases -> tighter p95/max
```

This drives the REAL pipeline through the headless composed keystone
(`general_e2e_composed_pbt`): `dispatch -> Loro commit -> LoroProjection resample
-> Turso/matview CDC -> reactive rows`. It measures everything EXCEPT final GPU
paint (headless — no window). Output is a per-action `count / p50 / p95 / max /
mean` table plus per-stage cost (dispatch, projection, CDC rows) and a dominator
line. Raw log: `/tmp/holon-latency.log`.

Each stage emits one greppable line under `target="holon_latency"`:

| stage          | emitted at                              | key fields                    |
|----------------|-----------------------------------------|-------------------------------|
| `dispatch`     | `ReactiveEngine::dispatch_intent_sync`  | `action`, `block`, `ms`       |
| `projection`   | `LoroProjection::project` (per commit)  | `ops`, `blocks`, `snapshot_ms`, `ms` |
| `rows`         | `LiveData::subscribe` (CDC batch apply) | `source`, `rows`, `seq`, `ms` |
| `action_total` | composed harness `apply` (per action)   | `action`, `total_ms`          |

To run against a custom log (e.g. the live app with `RUST_LOG=holon_latency=debug`):

```bash
python3 scripts/measure_latency.py /path/to/log
some-command | python3 scripts/measure_latency.py -
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

**Coarse GROUP BY** (`normalize-log.py`) — strips timestamps, ANSI colour codes, UUIDs/ULIDs, `block:` URIs, paths, large integers and inline JSON/SQL param blobs, then groups and counts. Collapses ~16k raw lines into ~1k unique normalized lines, so the top of the output is almost always the bottleneck:

```bash
python3 scripts/normalize-log.py /tmp/holon.log | head -80
```

Reach for this first when investigating "why is startup slow" or "what is the app actually doing". Pure stdlib Python, no deps.

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
