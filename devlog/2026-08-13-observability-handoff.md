# Handoff — Holon observability + web hosting (2026-08-13)

Owner: agent session (observability workstream). Status: **landed on `main`**, with
one uncommitted `web.yml` tweak and a `web-pages` bookmark awaiting the ply removal.

---

## 1. What this is

Two related features, built end-to-end in one session:

1. **Observability** — every keystone/GPUI PBT run emits a machine-readable result
   as a side effect of the run itself (green AND red); a collector turns those into
   canonical YAML; a static report renders them (including a frame flipbook of GPUI
   pixel screenshots); a publisher uploads them to a GitHub Release.
2. **Web hosting** — `holon-dioxus-web` + the wasm32-wasip1-threads worker, hosted
   statically on `https://holon.space/` (apex), with the observability report at
   `https://holon.space/obs/`.

## 2. The three design decisions that shape everything

These are load-bearing; don't "simplify" them away without re-deriving:

- **Data store is a GitHub Release (`observability`), not the repo and not Actions
  artifacts.** Chosen for indefinite retention + the ability to upload directly from
  Martin's Mac (`gh release upload` works locally; `actions/upload-artifact` does not).
- **Release assets are NOT browser-fetchable.** Verified empirically: the blob host
  (`release-assets.githubusercontent.com` → Azure) sends no `Access-Control-Allow-Origin`
  header, so a Pages page cannot `fetch()` them cross-origin. This is why the deploy
  workflows download release data **server-side** and co-locate it with the report.
- **Pixel capture is macOS-only.** GPUI ships exactly one headless renderer
  (`MetalHeadlessRenderer`, in `gpui_macos`). On Linux `current_headless_renderer()`
  returns `None` and capture is a no-op. There is no software renderer upstream.

## 3. File inventory (my additions)

```text
crates/holon-integration-tests/src/pbt/run_result.rs   # emission: RunResultGuard + panic hook
crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs  # keystone transcribed + guards
frontends/gpui/tests/pbt_harness/capture.rs            # FrameSink (pixel capture)
frontends/gpui/tests/pbt_harness/windowed_wide.rs      # TestApp -> HeadlessAppContext migration
frontends/gpui/tests/pbt_harness/sim_windowed_replay.rs# same migration (SimUserDriver)
frontends/gpui/tests/gpui_compose_sut_windowed.rs      # inline boot paths migrated
frontends/gpui/tests/gpui_composed_windowed_loop.rs    # windowed loop: capture + guard
frontends/gpui/tests/gpui_gherkin_replay.rs            # gherkin replay: capture + guard
frontends/gpui/Cargo.toml                              # +gpui_platform/test-support, +image dev-dep
scripts/holon_obs.py                                   # collector + publisher + server
observability/report/{index.html,app.js,app.css,config.js}  # static report (no deps)
observability/README.md                                # usage + layout + caveats
docs/Proposals/observability-report.md                 # full design (why, options, build order)
frontends/dioxus-web/build-pages.mjs                   # static assembly (bare imports + coi)
.github/workflows/_build-obs.yml                       # REUSABLE obs builder
.github/workflows/pages.yml                            # obs-only deploy (calls _build-obs)
.github/workflows/web.yml                              # full-site deploy (app + /obs/), tags-only
```

## 4. How to use it

```bash
just obs-collect                 # .result.json -> .yaml + runs.json + captures.json
just obs-serve                   # serve repo root; report at :8000/observability/report/
just obs-push                    # upload runs.json (clobber) + new YAMLs to the release
just gherkin-replay-capture      # GPUI gherkin replay WITH pixel capture, then serve
```

Env vars the harness reads:

| Var | Effect |
| --- | --- |
| `HOLON_CAPTURE_DIR=<dir>` | enable pixel capture, write frames + `events.jsonl` there |
| `HOLON_RESULT_DIR=<dir>` | redirect run-record output (default `<repo>/.observability/runs/`) |
| `HOLON_RESULT_DISABLE=1` | disable emission entirely |
| `HOLON_RESULT_GIT_REV=<rev>` | stamp a git rev (else `holon_obs.py` fills from `git rev-parse`) |
| `GHERKIN_FEATURE=<path>` | override the gherkin feature file |

Data flow: `.result.json` (harness) → `.yaml` (canonical, PyYAML) → `runs.json`
(cumulative index, the report's single fetch). `.observability/` is gitignored.

## 5. Deployment topology

```text
holon.space/
├── index.html + dioxus wasm/js       ← app (web.yml)
├── coi-serviceworker.js              ← patches COOP/COEP client-side
├── holon_worker.wasm32-wasi.wasm     ← worker (absolute root URL)
├── web/ + node_modules/              ← worker harness + ESM deps
└── obs/                              ← observability report (runs.json + report assets)
```

- **DNS** (already done): apex `A` records → `185.199.108–111.153`; `www` CNAME →
  `holon-space.github.io`. GitHub cert for `holon.space` is automatic; "Enforce
  HTTPS" was still `false` at last check — toggle it once GitHub's DNS check passes.
- **The `/obs/` split**: the report fetches `runs.json` *relative to itself*, so on the
  deployed site it resolves to `/obs/runs.json` with no config change. `config.js`'s
  `localDataPath`/`capturesIndexPath` are local-serving-only.
- **coi-serviceworker**: `wasm32-wasip1-threads` needs `SharedArrayBuffer` →
  cross-origin isolation, which GitHub Pages can't set via headers. The service worker
  patches them client-side; first visit reloads once.

## 6. Workflows and their relationship

| Workflow | Trigger | Deploys |
| --- | --- | --- |
| `web.yml` | `tags: ['v*.*.*']` + manual | **full site** (app at `/` + `/obs/`), atomically |
| `pages.yml` | `push: [main]` (report/workflow paths) + manual | **obs-only** — fast path |
| `_build-obs.yml` | (reusable, `workflow_call`) | the `/obs/` artifact, shared by both |

**Critical footgun**: GitHub Pages deploys atomically (one artifact = the whole site).
Running `pages.yml` *after* `web.yml` **overwrites the app with an obs-only site**.
Once `web.yml` has deployed once, **retire `pages.yml`** (both workflows document this).

## 7. Current state — what's landed vs pending

- **Landed on `main`**: the observability feature (commit `uuqkvzzr`
  "feat(observability): run-record emission, static report, GPUI pixel capture, Pages
  deploy"), including the `/obs/` subpath change.
- **`web-pages` bookmark** (`qwrlkmuu` "feat(web): deploy holon-dioxus-web + worker…"):
  the web deploy + `build-pages.mjs` + reusable obs builder + `web.yml` + refactored
  `pages.yml`. **Not merged** — blocked on the ply removal.
- **`sw/lane-obs-fixes`** (`yupxolwx`, task #11 D8.a): a *separate* lane already
  iterated on `run_result.rs` (run-result Drop now discloses failed writes; flamegraph
  files unique per write). Coordinate with that lane before touching `run_result.rs`.
- **Uncommitted (working copy)**: my last edits — `web.yml` tags-only trigger
  (`v*.*.*`) + comment cleanup. These are the only changes I did NOT commit (at the
  user's request). The working copy also carries the *other session's* ply-removal
  edits across many files — do NOT mix them.

## 8. The `web.yml` CI failure (why web-pages hasn't landed)

First `web.yml` run failed at `check-out-of-workspace-patch-revs.sh`:
`DRIFT: frontends/ply/Cargo.lock disagrees with the root lockfile`. ply is being
removed in a **separate session** (do not touch it from here). Once ply is gone from
the workspace, that guard stops checking it (the guard lists members explicitly).

## 9. How to continue

1. **Finish the ply removal** (other session) → the patch-rev guard no longer trips.
2. **Commit the uncommitted `web.yml`** (tags-only + comment cleanup), scoped:
   `jj commit .github/workflows/web.yml -m "ci(web): build+deploy only on release tags"`.
3. **Merge `web-pages`** → then push a `v*.*.*` tag to trigger the first full deploy
   (~20 min: `release-official` worker build is ~15 min).
4. **Retire `pages.yml`** after the first successful `web.yml` deploy.
5. **Toggle "Enforce HTTPS"** in Settings → Pages once GitHub's DNS check goes green.
6. **Verify** `https://holon.space/` (one-time reload = coi-serviceworker registering)
   and `https://holon.space/obs/`.

### Known unverified-in-CI risks

- The `napi build --profile release-official` command in `web.yml` is copied from
  `devex-gates.yml`'s `web-worker-smoke`, which the repo itself marks "UNVERIFIED in
  CI". If the first tag-triggered run fails at the worker build, the fix is in that
  napi command's environment (EMNAPI_LINK_DIR / target dir), not the app code.
- `build-pages.mjs` was validated locally end-to-end (bare-import rewrite against the
  real `worker-entry.mjs`, coi injection, correct tree). It was NOT run against a real
  `release-official` wasm (only a stub), so the wasm-path hand-off in CI is the one
  untested seam.

## 10. What I deliberately did NOT do

- No `CNAME` file — workflow-deployed Pages sites don't need one.
- No video — frames + a JS flipbook instead (no ffmpeg dependency).
- No Linux pixel capture — impossible without a software renderer (see §2).
- No ply changes — that's another session's work; reverted my one stray edit to
  `check-out-of-workspace-patch-revs.sh`.
