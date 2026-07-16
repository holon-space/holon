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

### Configuration

Test runner configuration is the SINGLE file `.config/nextest.toml` (the only
path cargo-nextest reads — a root `Nextest.toml` is inert and was deleted, F12).
It defines one `default` profile plus per-binary `slow-timeout` overrides so the
long-running E2E / PBT suites (`general_e2e_composed_pbt`, `turso_storage_pbt`,
…) get a larger hard cap than ordinary tests.

**PBT tests must NEVER carry nextest `retries`.** A property-based test that only
passes on retry is hiding non-determinism — a real flaky-divergence bug the
retry would silently paper over. The live config sets no `retries` anywhere;
keep it that way. (The deleted root `Nextest.toml` carried an inert
`retries = 1/2`; "fixing" a flaky PBT by re-adding retries is the wrong fix —
root-cause the non-determinism instead.)

### Combining with Code Coverage

Nextest works well with `cargo-llvm-cov` for coverage reporting:

```bash
cargo llvm-cov nextest --html --output-dir target/coverage-report
```

### Feature-gated test suites (and the empty-binary trap)

A test file whose first line is `#![cfg(feature = "X")]` compiles to an
**empty** binary — `0 tests, 0 benchmarks` — whenever `X` is off. An empty test
binary is a *green* test binary, so a gated suite silently stops running the
moment nothing enables its feature. BugFunnel row 78: `holon-orgmode`'s
`sync_controller_mutation_pbt.rs` is `#![cfg(feature = "di")]`-gated, `di` is
non-default, and the default `cargo test --workspace` compiled it to 0 tests —
so it rotted for weeks with a real round-trip data-loss bug hidden inside it.

Two mechanisms keep this from recurring:

1. **Coverage** — every gated suite must actually run *somewhere*:
   - `pbt`-gated suites live in `holon-integration-tests`, where `pbt` is a
     **default** feature, so `cargo test --workspace` (CI `rust-checks`) runs them.
   - `di`-gated suites (`holon-orgmode`) are run explicitly, because `di` is
     **non-default**:
     ```bash
     cargo nextest run -p holon-orgmode --features di
     ```
     CI does this in the `gated-suites` job (`.github/workflows/ci.yml`).

2. **Anti-rot guard** — `scripts/check-gated-test-suites.sh` auto-discovers every
   `#![cfg(feature=...)]`-gated file under a `tests/` dir and classifies each by
   whether its gating feature is in the crate's **default** feature set:
   - Feature IS default (e.g. `pbt` in holon-integration-tests) → the default
     `cargo test --workspace` already compiles+runs it. Not at risk; reported only.
   - Feature is **non-default** (e.g. `di`) → at risk. The guard **fails loud**
     unless BOTH hold: (a) some `.github/workflows/` step runs that crate with
     that feature (catches the unwired-suite gap — row 78 itself), and (b) the
     binary lists >0 tests with the feature on (catches an emptied / renamed /
     moved-out suite). It also fails on any `cfg` form it can't parse, rather
     than silently skipping it.

   Only the non-default-gated suites are compiled, so it stays cheap. It is
   self-maintaining — a newly-added gated suite is picked up automatically. Run:
   ```bash
   bash scripts/check-gated-test-suites.sh
   ```
   CI runs it in the same `gated-suites` job.

**When you add a `#![cfg(feature=...)]`-gated test file with a non-default
feature:** add a CI step that runs it with that feature — the guard will fail
until you do. The guard proves the binary is non-empty and wired; only the
actual run (step 1) proves the tests pass.

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
| `boot_parse`   | `on_file_changed` (per file, cold boot) | `blocks`, `path`, `ms` (parse+diff) |
| `boot_write`   | `on_file_changed` (per file)            | `blocks`, `path`, `ms` (block_raw ops apply) |
| `boot_feed_wait` | `on_file_changed` feed barrier (A/C)  | `caught_up`, `skipped`, `site`, `ms` |
| `boot_place_wait` | `on_file_changed` ordering replay    | `path`, `ms` (`ordering.children` + `place`) |
| `boot_file`    | `run_file_sync_controller` (per file)   | `path`, `ms` (whole `on_file_changed`) |
| `boot_ingest_total` | `run_file_sync_controller` (once)  | `files`, `ms` (whole initial scan) |
| `boot_feed_converge` | `finish_initial_scan` (once)      | `blocks`, `caught_up`, `ms` (one end-of-scan wait) |

Boot ingest is a per-file serial pipeline; the `boot_*` stages measure a cold
boot (empty Turso + existing org vault). Under Option 1 the per-file
`boot_feed_wait` is deferred (`skipped=true`, `ms=0`) and replaced by one
`boot_feed_converge` at end of scan; `caught_up=false` on any feed stage means
the barrier hit its ceiling. `measure_latency.py` prints a `BOOT INGEST` table
for these and flags any ceiling hits.

To run against a custom log (e.g. the live app with `RUST_LOG=holon_latency=debug`):

```bash
python3 scripts/measure_latency.py /path/to/log
some-command | python3 scripts/measure_latency.py -
```

## Scale Soak

`just measure-latency` runs the keystone against a 3-block focus doc, so vault-scale
behaviour never manifests — the projection/CDC/consolidator latency cliff
(`pass_ms ≈ 11.3 + 0.221×blocks`) and RSS growth at 5–10k blocks are found by hand. The
soak reproduces that regime automatically: it boots the SAME headless composed keystone
(the real `dispatch → Loro commit → projection → Turso/matview CDC → reactive rows`
pipeline, **CRDT on** — `full_headless` forces `crdt.enabled = Some(true)`) against a
seeded synthetic vault, then drives a few hundred mixed actions and grades each action
type against the **p95 < 200ms SLO**.

```bash
just soak                 # 5000 blocks, ~320 mixed actions, 30s settle budget
just soak 10000 480       # 10k blocks, ~480 actions
just soak 5000 320 30000 200   # size, actions, settle_ms, blocks-per-doc
```

What it does:

- **Seeds** a deterministic synthetic vault of `size` extra blocks (`scripts` →
  `crates/holon-integration-tests/src/pbt/composed/soak_seed.rs`): many pages, deep
  trees, `TODO`/`DONE`/`DOING` tasks, intra-vault links, and unicode (CJK / RTL / emoji /
  math). Same bytes every run. The extra blocks are seeded as separate org **docs** the
  SUT boots but the oracle folds into its scaffold seed-set, so the invariant catalog
  stays green while every action still pays the whole-vault projection/CDC cost.
- **Raises the settle budget** to `settle_ms` (default 30000). The keystone's 150ms
  `converge_projections` cap is far below a multi-second vault-scale drain; too small a
  budget would silently cap `action_total` below the true latency and hide the cliff.
- **Drives** ~`actions` mixed actions (edit / indent / outdent / split / toggle task
  state / navigate) via the production `E2ETransition` alphabet.
- **Measures** per-action-type p50/p95/max + per-stage cost + dominator
  (`measure_latency.py --fail-over-p95 200`) and samples process RSS over time
  (`scripts/soak_rss_sampler.sh` — the OS RSS, since the headless run emits no
  `MemoryMonitor` lines).

Results (per-action latency table, SLO gate verdict, RSS start→peak→end) are written to
`docs/Testing/soak/soak-<size>-blocks-<stamp>.txt` and echoed to the console. Runtime is
minutes: the vault is re-seeded once per proptest case (~20 actions each), so boot
overhead dominates wall time — boot is excluded from `action_total`, but the
`stage=boot_*` events (above) time boot ingest directly under the same
`holon_latency` target. For a cold-boot many-file benchmark:

```bash
HOLON_SOAK_SEED_FILES=200 HOLON_SOAK_BLOCKS_PER_FILE=10 \
  RUST_LOG=holon_latency=debug \
  cargo run --release --example diag_harness -p holon-integration-tests \
  --features boot-bench 2>&1 | tee /tmp/boot.log
python3 scripts/measure_latency.py /tmp/boot.log   # BOOT INGEST table
```

`TestEnvironmentBuilder` builds a fresh empty Turso per run, so this is cold by
construction (the warm-boot `file.content_hash` fast-path cannot engage).

**Nightly:** run `just soak` (5k) or `just soak 10000 480` and commit the result file
under `docs/Testing/soak/`. No CI/cron wiring — it is a single reliable command; diff
the newest result against the prior committed one to spot regressions.

**Not covered** (disclosed casualties): final GPU paint (headless — no window), real
file-watcher churn (the vault is seeded once, not edited on disk mid-run), multi-peer
CRDT sync/merge latency (single in-process peer), and platform differences (measured on
the dev host only). The soak stresses **block count**; it does not vary editor buffer
size or query complexity.
## Memory & Async-Stall Profiling

Two profilers sit alongside the latency tooling above. Together the four tools
answer four different "why is it slow / heavy?" questions:

| tool                | question it answers                          | how to run              |
|---------------------|----------------------------------------------|-------------------------|
| **latency** (above) | which *stage* of a UI action is slow?        | `just measure-latency`  |
| **chrome-trace**    | wall-clock timeline of spans (flamechart)    | `--features chrome-trace` (see `memory_monitor::chrome_trace`) |
| **heap (dhat)**     | *what allocates* the memory / where it grows | `just heap-profile`     |
| **stall (tokio-console)** | which async *task* stalls / never yields | `just tokio-console-*`   |

Both are feature-gated and zero-cost in normal builds.

### Heap profiling — dhat

Answers "where did the 4 GB go?". dhat installs a `#[global_allocator]` that
records every allocation's size and call stack, and writes `dhat-heap.json` on
a clean exit or Ctrl+C.

The allocator + profiler live in `holon-frontend`
(`memory_monitor::heap_profile`) behind the `heap-profile` feature, and are
already wired into the **real GPUI app** (`frontends/gpui/src/main.rs` calls
`heap_profile::start()`). For a scriptable, headless run there is a dedicated
harness that boots the same engine (org parse -> Loro -> CDC -> Turso, incl. the
async `DatabaseActor`) and ingests a synthetic vault:

```bash
just heap-profile            # seeds 2000 blocks, writes + summarizes dhat-heap.json
just heap-profile 8000       # bigger workload

# equivalent raw invocation:
HOLON_SOAK_SEED_BLOCKS=2000 \
  cargo run --release --example diag_harness -p holon-integration-tests \
  --features heap-profile
```

To profile the **live desktop app** instead, build it with the feature and use
the UI, then Ctrl+C:

```bash
cargo run -p holon-gpui --features heap-profile   # exercise the UI, then Ctrl+C
```

`HOLON_RSS_ABORT_MB` (default 1024) makes the `MemoryMonitor` self-abort — and
flush dhat — if RSS blows past the threshold, so a runaway leak still produces a
profile.

**Reading the output** — the web viewer at
<https://nnethercote.github.io/dh_view/dh_view.html> is the richest view, but
offline you can summarize with:

```bash
bash scripts/analyze_dhat.sh dhat-heap.json      # total bytes + top 15 sites
bash scripts/analyze_dhat.sh dhat-heap.json 40   # top 40
```

It prints lifetime total bytes/allocations and the top allocation sites by total
bytes at their leaf frame. A *true negative* (flat total, allocations dominated
by expected sites) is a useful result — it rules memory out.

### Async-stall profiling — tokio-console

Answers "which task starved the runtime?" (e.g. the `DatabaseActor` starvation
deadlock). `console_subscriber` exposes per-task poll/idle/busy times over a gRPC
port; the `tokio-console` TUI attaches to a live process. It only records real
data when built with `--cfg tokio_unstable`.

It is wired into `holon-frontend::logging::init()` behind the `tokio-console`
feature, so it attaches to the **live GPUI app**:

```bash
cargo install tokio-console                       # one-time (the CLI)

# 1) run the real app with the console runtime enabled:
just tokio-console-app
#    (= RUSTFLAGS="--cfg tokio_unstable" cargo run -p holon-gpui --features tokio-console)

# 2) in another shell, attach:
tokio-console http://127.0.0.1:6669
```

For a headless, scriptable run against the same engine boot (no window), use the
harness — it holds the process open so you can attach:

```bash
just tokio-console-harness 2000 120               # 2000 blocks, hold 120s
tokio-console http://127.0.0.1:6669               # attach from another shell
```

The task list shows total polls, busy time, and last-poll — a task that is
`RUNNING` with a large busy time and few polls is monopolizing a worker thread
(the starvation signature). Bind address overridable via `TOKIO_CONSOLE_BIND`.

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

## Quality gates (two-tier)

This is a colocated jj+git repo: **git hooks do not fire for jj commits**, so the
gates are `just` recipes you run yourself. Plain-git contributors can wire them
into hooks with `scripts/install-git-hooks.sh` (bypass a run with `--no-verify`).

| Tier | Command | When | What runs |
|------|---------|------|-----------|
| 1 | `just precommit` | every commit | defensive-code ratchet + `cargo check --workspace` |
| 2 | `just prepush` | every push | full keystone (`PROPTEST_CASES=16`, incl. persisted regression seeds) |

Notes:

- **Defensive-code ratchet** (`scripts/defensive-ratchet.sh`): runs
  `scripts/check-defensive-code.sh` and compares against the committed baseline
  `scripts/defensive-baseline.txt` (the pre-existing stock of violations).
  Only NEW violations fail the gate. Fix them or annotate with
  `// ALLOW(<reason>)`; after a reviewed intentional change run
  `scripts/defensive-ratchet.sh --update` and commit the baseline.
- **Why no keystone smoke in Tier 1**: measured 2026-07-07, even
  `PROPTEST_CASES=2` on the keystone takes ~4.5 min — proptest unconditionally
  replays the persisted regression seeds
  (`tests/general_e2e_composed_pbt.proptest-regressions`, 11 seeds) and every
  case pays full composed-SUT boot. A "smoke" is barely cheaper than the full
  16-case run, so the keystone lives entirely in Tier 2.
- Timings assume a **warm build cache**; the first run after a rebase that
  touches many crates pays the compile cost once.

## iOS live-MCP E2E gate (`general_e2e_composed_pbt_live_mcp`)

The out-of-process twin of the headless keystone: the SAME transitions +
invariant catalog, driven against a **live Holon app on the iOS simulator** over
its embedded MCP server. This is how iOS joins the E2E gate — it exercises the
real platform input/render/store wiring the headless keystone cannot see.

**Recipe** (wrapped by [`scripts/run_ios_live_mcp_e2e.sh`](scripts/run_ios_live_mcp_e2e.sh)):

1. The app must be built + installed on the sim already (this is a *launch*, not
   a build/install).
2. Relaunch it with reset + MCP enabled. `simctl launch` forwards `SIMCTL_CHILD_*`
   env into the app process:
   ```sh
   xcrun simctl terminate <SIM_UDID> space.holon.gpui
   SIMCTL_CHILD_MCP_SERVER_PORT=8521 \
   SIMCTL_CHILD_HOLON_MCP_ALLOW_RESET=1 \
     xcrun simctl launch <SIM_UDID> space.holon.gpui
   ```
   `HOLON_MCP_ALLOW_RESET=1` is REQUIRED — the keystone does a per-case
   `reset_vault`, which the app refuses without it (fails with
   "reset_vault is disabled — set HOLON_MCP_ALLOW_RESET=1").
3. Run the test (note: **NOT** `--ignored` — `general_e2e_composed_pbt_live_mcp`
   is a plain `#[test]` that self-skips unless `HOLON_PBT_LIVE_MCP` is set, so
   `--ignored` filters it OUT):
   ```sh
   HOLON_PBT_LIVE_MCP=1 MCP_SERVER_PORT=8521 PROPTEST_CASES=3 \
     cargo test -p holon-integration-tests \
       --test general_e2e_composed_pbt general_e2e_composed_pbt_live_mcp \
       -- --nocapture --test-threads=1
   ```
   The server's reset budget is 20/process, so keep `PROPTEST_CASES` small and
   never shrink live (`max_shrink_iters: 0` in the test).

**Status (2026-07-09): NOT yet green.** The keystone connects, resets, and drives
transitions for ~50s, then panics in the `SplitBlock` transition
(`crates/holon-integration-tests/src/pbt/composed/live_mcp.rs` `focus_editor`):
the driver geometry-clicks the split target by its `BoundsRegistry` bounds, but
if that block lives under a page other than the focused `main` root it is never
rendered → `click_entity` returns "no bounds recorded" and the 10s budget
expires. To reach green the live driver must navigate the target block's page
into `main` (or focus it) before geometry-driving it. Re-run the script to
reproduce and to re-check once that harness gap closes.
