# Observability report — "how well is Holon?"

Status: **proposal** (not yet ratified). Author: agent (2026-08-12). Scope: a unified,
data-backed answer to "is Holon getting better or worse", including visual evidence from
the GPUI test runs, stored in declarative files, rendered as a static report, hostable on
GitHub Pages.

---

## 1. What this is for

Martin's asks, restated as requirements:

| Ask | Requirement |
| --- | --- |
| "how many tests are failing" | R1 — unit/integration test counts + pass/fail, per suite and per run |
| "how many issues the keystone PBTs detect" | R2 — keystone tier results, known-red vs novel classification, registry drift |
| "what the SLOs are" | R3 — the p95 < 200ms SLO + the per-rung latency ratchet, shown as target vs measured |
| "dominators for runtime of certain operations" | R4 — per-action latency breakdown (projection / dispatch / CDC rows) + per-transition apply/check timing |
| "screenshots / video of the GPUI test run" | R5 — frame capture during `gpui_gherkin_replay` and the windowed keystone loop |
| "one report ... how 'well' Holon is" | R6 — a single HTML report with a health summary |
| "historical values / charts, direction of travel" | R7 — time-series over immutable run records |
| "declarative files (YAML) for custom analysis" | R8 — YAML run records as the source of truth |
| "HTML generated from data, or JS loads data" | R9 — static report, JS loads the data (works offline, works on Pages) |
| "host on GitHub Pages" | R10 — a Pages deploy path that needs no server |

## 2. What already exists (build on it — do not duplicate)

Holon is already heavily instrumented. The observability report is a **unification layer**
over these existing pieces, not a new measurement system:

| Existing piece | Produces | Consumed today by |
| --- | --- | --- |
| `just pbt general\|petri\|orgmode\|loro` | proptest logs (pass/fail, shrunk case) | humans, `tee` to `/tmp` |
| `just keystone-smoke / keystone-full / keystone-nightly` | tiered keystone runs | justfile, orchestrator |
| `scripts/keystone-known-reds.sh` | per-signature classification vs the registry; exit 0 pass / 1 novel / 3 no verdict | keystone-nightly |
| `docs/Testing/KeystoneKnownReds.md` | known-red / fixed-pending-soak rows | the classifier |
| `scripts/keystone-known-reds-fixture.sh` | registry drift + outcome-classification check | `just known-reds-fixture` |
| `just measure-latency` → `scripts/measure_latency.py` | per-action p50/p95/max + per-stage cost + **dominator line** | console |
| `just latency-gate` → `measure_latency.py --ratchet docs/Testing/latency-ceilings.txt` | per-rung p50 vs ceiling, pass/fail | build gate |
| `just soak` | vault-scale latency + RSS, written to `docs/Testing/soak/soak-*.txt` | committed text files |
| `scripts/pbt_inventory.py` | `docs/Testing/pbt-inventory.yaml` (+ `.md`) | docs |
| `docs/Testing/BugFunnel.md` | running distribution ENVIRONMENT/COVERAGE/PERCEPTION/ORACLE | docs |
| `crates/holon-api/src/latency_e2e.rs` | `holon_latency` tracing events (prod correlator, SLO endpoint) | logs |
| `crates/.../pbt/stepper.rs` `StepTimingAgg` | per-transition `apply_ms` / `check_ms` | eprintln |
| GPUI harness `pbt_harness/` | windowed `ComposedSut<WideE2E>` + `SimUserDriver` + `replay_fixture_windowed` | windowed PBT/gherkin |
| `window.render_to_image()` | `image::RgbaImage` of the current frame (TestPlatform + HeadlessRenderer) | MCP screenshot path only |
| `cargo nextest --message-format libtest-json-plus` | machine-readable per-test results | (unused) |
| `cargo llvm-cov report --summary-only` | coverage percentages | docs |

The genuinely new code is: **(1)** a screen-capture hook in the GPUI harness, **(2)** a
machine-readable (JSON) emission mode for `measure_latency.py`, **(3)** a collector that
assembles a run record, **(4)** the static report, **(5)** the Pages workflow. Everything
else is re-pointing existing producers at a common output.

## 3. Architecture

```
                          ┌────────────────────────────────────────────┐
  producers              │              collectors                      │
                          │                                             │
 nextest --libtest-json  ─┤                                            │
 pbt logs ────────────────┤  scripts/holon_obs.py  collect  ──►  run record
 keystone-known-reds.sh ──┤        (one run_id)                 (YAML)  │
 measure_latency.py --json┤                                            │
 llvm-cov summary ────────┤                                            │
 BugFunnel.md parse ──────┤                                            │
 gpui capture manifest ───┤                                            │
                          └────────────────────────────────────────────┘
                                          │
                                          ▼
                    observability/
                    ├── runs/<run_id>.yaml        (source of truth, committed)
                    ├── runs.json                  (derived mirror for the JS)
                    ├── assets/<run_id>/<seq>.png  (frames, committed)
                    ├── assets/<run_id>/run.webm   (optional, size-gated)
                    └── report/
                        ├── index.html  app.js  app.css
                        └── vendor/                 (committed chart lib, offline)
                                          │
                                          ▼
                              GitHub Pages (gh-pages branch)
```

**Principle: YAML is canonical, JSON is derived.** The report JS cannot fetch YAML
natively, so the collector writes a `runs.json` mirror (one `yaml.safe_load` →
`json.dump`) of the same content. Custom analysis reads the YAML; the report reads the
JSON. They are generated by the same code path, so they cannot drift.

## 4. The run record (the declarative heart)

One file per observed run, immutable once written, append-only index. Schema
(`observability/runs/<run_id>.yaml`):

```yaml
run_id: 20260812T215900Z-keystone-nightly   # sortable, unique
schema: 1
created_at: 2026-08-12T21:59:00Z
git_rev: 394e6b14fb
git_branch: main
host: martins-macbook                    # hostname; "" for CI
kind: keystone-nightly                   # one of the kind vocabulary below
duration_sec: 8123.4

# ── R1: test counts ──────────────────────────────────────────────
tests:
  unit_integration:          # from nextest --message-format libtest-json-plus
    passed: 1667
    failed: 0
    skipped: 2
    suites: {holon: {passed: 219, failed: 0}, ...}   # optional per-crate

# ── R2: keystone PBT ─────────────────────────────────────────────
keystone:
  tier: nightly                 # smoke | full | nightly | scale | mcp
  cases: 64
  runs: 2
  verdict: pass-with-note       # green | red | pass-with-note
  green_runs: 1
  red_runs: 1
  known_red_hits: {org-render-echo-loop: 3, split-id-no-pairing: 1}
  novel_signatures: 0

# ── R3: SLOs ─────────────────────────────────────────────────────
slo:
  target_p95_ms: 200            # interaction -> projection-visible
  source: latency-ceilings.txt + CLAUDE.md
  # measured against the p95 SLO (soak) and/or the per-rung ratchet (gate)
  ratchet:                      # one entry per GATED rung in the ceilings file
    - {rung: total.p50.NavigateFocus, value_ms: 233.5, ceiling_ms: 374, n: 24, ok: true}
    - {rung: total.p50.TypeChars,     value_ms: 136.5, ceiling_ms: 219, n: 24, ok: true}
    - {rung: e2e.p50.set_field,       value_ms:  25.0, ceiling_ms:  40, n: 20, ok: true}
  ratchet_verdict: passed

# ── R4: runtime dominators ────────────────────────────────────────
latency:                       # from measure_latency.py --json
  actions:
    - {action: SplitBlock, n: 127, p50_ms: 131.0, p95_ms: 191.3, max_ms: 208.0, mean_ms: 136.7}
    - {action: NavigateFocus, n: 6, p50_ms: 224.5, p95_ms: 259.2, max_ms: 268.0, mean_ms: 231.5}
  stages:                      # pipeline stage cost, all actions
    - {stage: projection, n: 44, p50_ms: 118.2, p95_ms: 172.0, mean_ms: 121.0}
    - {stage: rows,       n: 40, p50_ms: 8.1,  p95_ms: 15.0,  mean_ms: 9.0}
  dominator:
    projection_pct_of_action_wall: 95
    note: "full-document DFS projection snapshot per commit"
  step_timing:                 # from StepTimingAgg (per-transition apply/check)
    - {transition: SplitBlock, apply_ms: 120, check_ms: 11}
    - {transition: TypeChars,  apply_ms: 130, check_ms: 5}

# ── R5: visual evidence ───────────────────────────────────────────
media:
  capture_dir: assets/20260812T215900Z-keystone-nightly
  frames: 42
  video: assets/20260812T215900Z-keystone-nightly/run.webm   # optional
  events: assets/20260812T215900Z-keystone-nightly/events.jsonl  # step annotations

# ── quality signals (time-series friendly) ────────────────────────
known_reds: {open: 6, fixed_pending_soak: 4}
bugfunnel: {ENVIRONMENT: 171, COVERAGE: 119, PERCEPTION: 70, ORACLE: 66}
coverage: {lines_pct: 61.3, regions_pct: 55.1}     # optional (llvm-cov)
rss: {start_mb: 240, peak_mb: 890, growth_mb: 650} # soak only
```

**Kind vocabulary** (a run record is always *about one thing*; a full "health sweep"
emits a `sweep` record that references the others):

`unit` · `keystone-smoke` · `keystone-full` · `keystone-nightly` · `keystone-scale` ·
`soak` · `latency-gate` · `gherkin-replay` · `sweep` (aggregate) · `ci`

### Index

`observability/index.yaml` — one line per run, nothing else (so it stays tiny):

```yaml
schema: 1
runs:
  - {run_id: 20260812T215900Z-keystone-nightly, kind: keystone-nightly, git_rev: 394e6b14fb,
     created_at: 2026-08-12T21:59:00Z, file: runs/20260812T215900Z-keystone-nightly.yaml}
```

The report JS loads the index, then lazy-loads individual run YAMLs as the user picks a
point on a chart. A `sweep` run record additionally carries a `members: [run_id, ...]`
list so the "health at a glance" view can render one aggregate card from several runs.

## 5. Component A — capture layer (screenshots / video)

### Where it hooks

The GPUI windowed harness already exposes the exact seams needed
(`crates/holon-integration-tests/src/pbt/fixtures/mod.rs::replay_steps`):

- `after_start_app(&mut S)` — fires once after `StartApp`.
- `per_tick(&mut S, &M::State)` — fires after **every** post-StartApp transition's
  invariants pass, with SUT + reference state in scope.

`gpui_gherkin_replay.rs` and `gpui_composed_windowed_loop.rs` already thread these hooks;
they currently pass no-ops. The capture layer is a new module
`frontends/gpui/tests/pbt_harness/capture.rs` that implements the hooks.

### Capture primitive

`window.render_to_image()` already exists on GPUI's `Window`
(`gpui/src/window.rs:1962`), returns `image::RgbaImage`, and works on `TestPlatform` when
a `HeadlessRenderer` is configured — the same path the MCP screenshot uses
(`frontends/gpui/src/lib.rs:2762`). So the capture hook needs **no new rendering
dependency**; it only needs the `AnyWindowHandle` (already held by `SimUserDriver`).

```rust
// sketch — pbt_harness/capture.rs
pub struct FrameSink {
    dir: PathBuf,            // HOLON_CAPTURE_DIR
    seq: AtomicUsize,
    events: Mutex<File>,     // events.jsonl
    window: AnyWindowHandle,
    enabled: bool,           // false when HOLON_CAPTURE_DIR is unset -> zero cost
}

impl FrameSink {
    /// Call from per_tick / after_start_app. Renders the current frame, writes
    /// `NNNN.png`, appends one JSONL line: {seq, step, transition, phase,
    /// invariants_ok, ref_summary}.
    pub fn capture(&self, step: usize, transition: &str, phase: Phase, ok: bool) {
        if !self.enabled { return; }
        let img = /* app.update_window(... render_to_image ...) */;
        img.save(self.dir.join(format!("{:04}.png", seq)))?;
        // events.jsonl: {seq, step, transition, phase, ok}
    }
}
```

- **PNG frames** are the primary artifact (small, self-contained, already supported by
  `image`). Frame rate: one per `per_tick` is the natural cadence (one per transition +
  invariant check) — this is what a human wants to scrub.
- **Video** is a *derived* convenience: a `webm` assembled from the PNG sequence with
  `ffmpeg` at collect time (`ffmpeg -framerate 2 -i %04d.png run.webm`), **only** if
  ffmpeg is present and the frame count × size is under a budget. No new Rust dependency;
  video is optional and never blocks a run. (Fallback if ffmpeg is unwanted: an
  HTML/JS "flipbook" that scrubs the PNG frames directly — actually the *default* in the
  report, with webm as an optional extra.)

### Enabling it

The harness binary `gpui_gherkin_replay` and the windowed loop read `HOLON_CAPTURE_DIR`;
if unset, `FrameSink` is disabled (no capture, no I/O). A `just` recipe wraps it:

```make
gherkin-replay-capture feature='tests/features/ordinary_block_interaction.feature':
    mkdir -p observability/assets
    dir="observability/assets/$(date -u +%Y%m%dT%H%M%SZ)-gherkin"
    HOLON_CAPTURE_DIR="$dir" GHERKIN_FEATURE={{feature}} \
        cargo test -p holon-gpui --features pbt --test gpui_gherkin_replay -- --test-threads=1
    echo "frames: $dir"
```

⚠ `--test-threads=1` is already mandatory for gpui windowed tests; the capture inherits
that constraint, so there is exactly one window per process and the frame sequence is
unambiguous.

## 6. Component B — machine-readable emission

`scripts/measure_latency.py` already computes everything R4 needs but prints it as text.
Add a `--json <path>` flag that writes the same tables (per-action, per-stage, dominator,
ratchet verdict) as the structured blocks shown in §4. No change to the text output or to
the gate logic — the JSON write is additive. Same treatment, minimally:

- a small `scripts/holon_obs.py` that wraps the producers and assembles the record (the
  nextest JSON, the pbt log classification via `keystone-known-reds.sh` exit code + its
  `WARN known-red [key] xN` lines, `measure_latency.py --json`, `BugFunnel.md` header
  counters, `KeystoneKnownReds.md` status counts, optional `llvm-cov` summary).

## 7. Component C — report (static HTML + JS)

`observability/report/` — pure static assets, no build step, no server:

```
report/
├── index.html        # single page, sections below
├── app.js            # fetch index.yaml + run YAMLs, render
├── app.css
└── vendor/           # committed chart lib (e.g. uPlot or Chart.js), offline
```

Sections (R6):

1. **Health at a glance** — big cards: tests failing (R1), keystone verdict (R2),
   SLO/ratchet verdict (R3), worst latency dominator (R4), known-reds open, bug-funnel
   distribution. Each card colored green/amber/red and compared to the previous run.
2. **Tests** — pass/fail per suite, failing-test names.
3. **Keystone PBT** — tier history, known-red vs novel counts, registry drift status.
4. **Latency & SLO** — per-action p50/p95 bars with the 200ms SLO line; per-rung ratchet
   chart with ceiling lines (R3 + R4).
5. **Runtime dominators** — stacked per-action breakdown (projection / dispatch / rows /
   other) and per-transition apply-vs-check (R4).
6. **Visual evidence** — flipbook scrubber over the PNG frames, with step/transition
   annotations from `events.jsonl`; optional `<video>` if `run.webm` exists (R5).
7. **History / direction of travel** — line charts of every numeric metric over `index.yaml`
   (R7). Series are computed client-side from the immutable run records, so "did it get
   better or worse" is literally a slope, and an individual point is drillable to the run
   YAML.

The JS loads `index.yaml` + `runs.json` via `fetch`. The report is **fully static and
offline-capable** (no CDN, no API), so it renders identically from `file://` and from
GitHub Pages (R9, R10).

## 8. Component D — history / charts

History is a *consequence* of the append-only index, not a separate store: every metric in
a run record is a time series with one point per run. No aggregation server, no database.
The report keeps the index in memory and lazy-loads run records. To keep Pages fast as the
index grows, a background job can optionally pre-bucket old runs into
`observability/history/<metric>.json`, but that is an optimization, not a requirement.

Direction-of-travel is made explicit by showing a small Δ vs the previous run of the same
`kind` on every card (e.g. "known-reds −2", "SplitBlock p95 +14ms").

## 9. Component E — GitHub Pages (R10)

Two deployment paths, both static:

1. **Report-only** (cheap, recommended default): a `pages.yml` workflow triggers on
   push to `main` + `workflow_dispatch`, runs `python3 scripts/holon_obs.py render`
   (YAML → `runs.json`), and deploys `observability/report/` + `runs.json` +
   `assets/` to the `gh-pages` branch via `actions/deploy-pages`. It deploys **whatever
   run records are committed** — the heavy runs stay local and their YAML is committed,
   exactly like `docs/Testing/soak/*.txt` today.
2. **CI light-run** (optional): the same workflow runs the *cheap* producers it can
   afford on the runner — `cargo nextest run --workspace` (already the CI test job),
   `llvm-cov` summary, `latency-gate` — and emits a `kind: ci` run record before
   deploying. The keystone/soak tiers are deliberately **not** run in CI (the repo has
   already ruled them out: keystone is hours, CI never reaches it — see the
   `keystone-nightly` comment in `justfile`).

Media weight on Pages: PNG frames are committed but capped (retention policy: keep the
last N runs' frames; older runs keep their `events.jsonl` + a hero frame, drop the full
sequence). If frame volume becomes a problem, `git-lfs` on `observability/assets/` is the
escalation, not a design change.

## 10. Metrics catalog (single source of truth per metric)

| Metric | Source | SLO / target | Section |
| --- | --- | --- | --- |
| failing unit/integration tests | nextest JSON | 0 | Tests |
| keystone verdict (green/red/pass-with-note) | pbt + known-reds classifier | green | Keystone |
| novel keystone signatures | `keystone-known-reds.sh` | 0 (regression) | Keystone |
| keystone logs admitting no verdict | `keystone-known-reds.sh` exit 3 | 0 (broken input, not a pass) | Keystone |
| known-red rows (open / fixed-pending-soak) | `KeystoneKnownReds.md` | decreasing | Keystone |
| per-action e2e latency p50/p95/max | `measure_latency.py` | p95 < 200ms | Latency |
| per-rung ratchet (p50 vs ceiling) | `measure_latency.py --ratchet` | ≤ ceiling | Latency |
| runtime dominator (projection % of action wall) | `measure_latency.py` | decreasing | Dominators |
| per-transition apply/check ms | `StepTimingAgg` | — | Dominators |
| bug-funnel distribution | `BugFunnel.md` header | decreasing | Quality |
| coverage % (optional) | `llvm-cov` | increasing | Quality |
| RSS growth (soak) | `soak_rss_sampler.sh` | bounded | Soak |

The SLO itself (R3) is *presented*, not invented here: **p95 < 200ms**,
interaction→projection-visible (`latency_e2e.rs` "SLO endpoint"), plus the
`latency-ceilings.txt` ratchet (ceilings move only down). The report reads the ceilings
file so the SLO shown is always the authoritative one.

## 11. Build order (incremental, each step lands something useful)

1. **`measure_latency.py --json`** — pure additive; unblocks every later step. (hours)
2. **`holon_obs.py collect`** — assemble a minimal `kind: latency-gate` record from an
   existing run log; write `runs/<id>.yaml` + append `index.yaml`. Proves the schema.
   (half day)
3. **Static report skeleton** — `index.html`/`app.js` that reads `index.yaml` + one run
   YAML and renders the health cards + latency table. No charts yet. (half day)
4. **Capture layer** — `capture.rs` + `HOLON_CAPTURE_DIR` + the `gherkin-replay-capture`
   recipe; produces PNG frames + `events.jsonl`. (day)
5. **Flipbook + charts** — vendor a chart lib, add the visual-evidence section and the
   time-series section. (day)
6. **Collector coverage** — wire the remaining producers (nextest JSON, keystone
   classification, BugFunnel/registry counters, `--json` ratchet) into `holon_obs.py`.
   (day)
7. **Pages workflow** — `pages.yml` (report-only first; CI light-run later). (half day)

Total: roughly a week of focused work, and steps 1–3 already deliver a useful, if thin,
report.

## 12. Risks & decisions

- **Frame capture on `TestPlatform` needs a `HeadlessRenderer`.** Confirmed present
  (`platform/test/window.rs:295`), and the MCP screenshot path already exercises
  `render_to_image`. If a specific harness wiring lacks the renderer, `render_to_image`
  fails loud (`anyhow::bail`) and the capture must fail loud too — never emit a blank
  frame silently (the repo's own rule, see the MCP screenshot comment).
- **Video is optional and size-gated**; the flipbook is the default. Keeps the repo from
  bloating with webm and keeps ffmpeg optional.
- **YAML canonical, JSON derived** — one generator, so they can't drift; custom analysis
  stays on YAML.
- **Do not re-measure what's measured** — the report reads existing producers; any new
  metric must have a named source and a named owner before it enters the schema (matches
  the known-reds "no unowned residual" discipline).
- **History without a database** — the append-only index is the database. It must stay
  small; per-run files are lazy-loaded.
- **Retention** — frames are the only bulky artifact; cap them (last N runs) and escalate
  to LFS if needed. Run YAMLs and `events.jsonl` are small and kept forever.

## 13. Open questions for Martin

1. **Where do run records live?** Proposed `observability/` at repo root, committed. This
   makes history = git history + append-only files. Alternative: a sibling repo / gist to
   keep the main repo lean.
2. **Which runs are "official" health?** A nightly local `just obs-sweep` (keystone-nightly
   - latency-gate + soak) that commits one `kind: sweep` record — is that the cadence, or
   is per-commit/per-weave the expectation?
3. **CI scope for Pages**: report-only deploy of committed data (recommended), or also run
   the cheap CI producers to add a `kind: ci` point per push?
4. **Frame retention N** — how many runs' worth of PNG sequences to keep committed?
5. **Chart library preference** — vendored uPlot (tiny, canvas) vs Chart.js (familiar) vs
   hand-rolled SVG (zero dep, more work)?
