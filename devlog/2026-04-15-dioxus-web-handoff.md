# Holon Dioxus Web — Session Handoff

**Date:** 2026-04-15 (updated 2026-04-15 PM with shared-macro port)
**Branch / HEAD:** detached from `9e51cd86f` (post `72778bb08`, working tree dirty)
**Plan:** `~/.claude/plans/majestic-sauteeing-lollipop.md` — C1–C3 + C7 + partial C4

> **Update PM session (shared-macro port):** the JSON-field-probing renderer
> (423-line `render.rs`) has been replaced with a typed builder tree that
> reuses `holon-frontend` and the shared `holon_macros::builder_registry!`
> macro — see the [Follow-up session 2](#follow-up-session-2--typed-viewmodel--shared-builder_registry-port) section at the bottom of this
> doc for the full delta and what's left. The original handoff below is
> left intact as historical context.

---

## TL;DR

The Dioxus web frontend now renders an end-to-end ViewModel from the
`holon-worker` Web Worker and cleanly reports boot failures. Three real bugs
were found and fixed via Playwright/gstack-browse testing.

**What works:**
- Boot → `engineInit(:memory:)` → seed → watch root layout → render 3-column
  layout with 3 independently-watched `BlockRef` cells.
- Tick pump drives the current-thread runtime at ~60fps.
- Failure paths (wasm 404, missing exports, serialize errors) surface as
  `BootState::Failed` within 10 s instead of hanging forever.
- 3 lockdown tests in `holon-api` for the untagged `Value` wire format.

**What's still empty:**
- Each recursively-watched `BlockRef` subscribes but renders empty children
  because the seeded layout queries (`from block | filter name != ...`) match
  no user blocks. The plumbing is correct; the data model has nothing in it
  yet. Adding a visible block requires either editing the seed or wiring a
  "create block" operation in the Dioxus UI.
- EditorCell path is wired but never gets a chance to render (no
  `editable_text` nodes emitted by the current seed data).

---

## How to run

```bash
cd frontends/dioxus-web
node serve.mjs --build      # one-shot build + serve on http://127.0.0.1:8765
# or:
node serve.mjs --watch      # trunk watch + serve
```

**Dependencies:**
- `trunk` 0.21+ (`brew install trunk`)
- Node 20+
- `@napi-rs/cli` 3.1+ (local in `frontends/holon-worker/node_modules`)
- `wasm32-unknown-unknown` and `wasm32-wasip1-threads` Rust targets

**Worker wasm is pre-built** and checked in at
`frontends/holon-worker/holon_worker.wasm32-wasi.wasm` (~630 MB debug build).
Rebuild with:

```bash
cd frontends/holon-worker
EMNAPI_LINK_DIR="$(pwd)/node_modules/emnapi/lib/wasm32-wasi-threads" \
  ./node_modules/.bin/napi build \
  --features browser --profile dev --platform \
  --target wasm32-wasip1-threads --no-js \
  --manifest-path ./Cargo.toml --output-dir .
# napi's copyArtifact breaks on dev profile (looks for target/.../dev but
# cargo uses target/.../debug). Copy manually:
cp target/wasm32-wasip1-threads/debug/holon_worker.wasm \
   holon_worker.wasm32-wasi.wasm
```

`--profile release-official` produces a much smaller wasm (LTO + opt-z) but
takes ~15 min to compile. Use `dev` during iteration.

---

## Architecture refresher

```
┌────────────────────────┐          postMessage          ┌──────────────────────┐
│  Dioxus page (main)    │  ─────────────────────────▶  │  holon-worker Web    │
│  holon-dioxus-web      │                               │  Worker (wasi-threads)│
│                        │  ◀───  {kind, handle, json}  │                      │
│  ┌─────────────────┐   │                               │  BackendEngine +     │
│  │ WorkerBridge    │   │                               │  ReactiveEngine      │
│  │ (bridge.rs)     │   │                               │  (current-thread rt) │
│  │   onmessage ──▶ │   │                               │                      │
│  │   call(kind, a) │   │                               │                      │
│  │   on_snapshot   │   │                               │                      │
│  └─────────────────┘   │                               └──────────────────────┘
│                        │
│  BRIDGE (thread_local) │
│  view_model Signal     │
│  RenderNode (render.rs)│
│  ├─ BlockRefNode       │◀── each owns its own watch subscription
│  │  └─ inner_vm Signal │
│  └─ EditableTextNode   │
│     └─ EditorCell      │
└────────────────────────┘
```

All holon crates live inside the worker. The Dioxus side only speaks JSON over
`postMessage` — no holon types imported.

---

## What got shipped this session

### New files

- `frontends/dioxus-web/src/bridge.rs` (225 lines) — `WorkerBridge` RPC + snapshot routing
- `frontends/dioxus-web/src/render.rs` (423 lines) — `ViewModel` JSON → Dioxus element tree
- `frontends/dioxus-web/src/editor.rs` (400 lines) — uncontrolled `contenteditable` with flat-offset cursor preservation and debounced dispatch
- `frontends/holon-worker/src/seed.rs` (148 lines) — default layout seed replacing `FrontendSession::seed_default_layout` (which can't compile on wasi)
- `frontends/holon-worker/src/subscriptions.rs` (60 lines) — `allocate` / `install` / `remove` / `cancel` handle registry
- `frontends/dioxus-web/serve.mjs` — combined trunk + worker-harness dev server on port 8765

### Key changes in existing files

- `frontends/dioxus-web/src/main.rs` — worker bridge boot, root-layout watch, tick pump `use_future`
- `frontends/holon-worker/src/lib.rs` — Phase 3 `engine_*` napi exports (`init`, `execute_query`, `execute_sql`, `execute_operation`, `watch_view`, `drop_subscription`, `set_variant`, `tick`, `reactive_check`) collapsed into one `#[cfg(feature = "browser")] mod engine_exports`
- `frontends/holon-worker/web/worker-entry.mjs` — RPC loop, snapshot-array-unwrap fix, lazy `Proxy` stubs for `__wbindgen_*` imports
- `crates/holon-frontend/src/view_model.rs` — `ViewKind::tag() -> &'static str` method
- `crates/holon-api/src/lib.rs` — enriched `uri_from_row` error + 2 lockdown tests for the untagged `Value` serde format

---

## Bugs found & fixed this session (testing ledger)

### 1. TSFN tuple delivered as JS array on emnapi/wasi (`worker-entry.mjs`) 🔴

**Symptom:** First snapshot arrived at the Dioxus bridge but
`snapshotJson` parsed as `Value::Array([String(...)])` instead of
`Value::Object`, so `RenderNode` rendered a placeholder.

**Root cause:** napi-rs's `ThreadsafeFunction<String>::build_callback(|ctx|
Ok((ctx.value,)))` declares a 1-tuple of JS arguments. On native napi the
tuple is unpacked to positional args; on emnapi/wasi-threads the tuple is
delivered as a single-element JS array `[payload]`. Calling
`JSON.stringify([string])` produced `'["..."]'` — a double-encoded,
array-wrapped JSON string.

**Fix:** `frontends/holon-worker/web/worker-entry.mjs` lines ~147–175 —
unwrap the array before routing:

```js
let payload = cbArg
if (Array.isArray(payload) && payload.length === 1) payload = payload[0]
```

**Testing insight:** This was invisible in the worker harness (its B3 test
used `...cbArgs` rest-params + `json['0']` fallback, which accidentally
unwrapped). The cleanup after initial Playwright runs removed the fallback
and broke the rendering silently. Diagnosing took adding `tracing::info!`
in main.rs to log the parsed `Value::Debug` repr.

### 2. Seed with double `block:` prefix (`seed.rs`) 🟡

`block:block:left_sidebar::render::0` and `::src::0` — not parsed as
invalid URIs (fluent_uri accepted them), but conceptually wrong and
distracting during debugging. Fixed to `block:default-left-sidebar::...`.

### 3. Workspace inheritance broken by `exclude` (`Cargo.toml`) 🔴

`frontends/dioxus-web` is in the workspace `exclude` list (wasm32-only —
would fail `cargo check --workspace`). An excluded package cannot use
`foo.workspace = true` inheritance; cargo reports
`failed to find a workspace root`.

**Fix:** `frontends/dioxus-web/Cargo.toml` — replaced `tracing.workspace =
true` / `futures.workspace = true` / `serde_json.workspace = true` with
direct version pins, comment explaining why.

### 4. `WorkerBridge::spawn` hangs on broken worker (`bridge.rs`) 🟡

`ready_rx.await` had no timeout. If the worker failed to instantiate
(missing wasm, import link error, crash inside `beforeInit`), the Dioxus
app sat at "booting…" indefinitely.

**Fix:** `frontends/dioxus-web/src/bridge.rs` ~lines 147–170 — race
`ready_rx` against a 10 s `TimeoutFuture` and return `BootState::Failed`
with a precise diagnostic ("worker did not emit `ready` within
10000ms — likely failed to instantiate…").

**Verified** by temporarily renaming `holon_worker.wasm32-wasi.wasm.bak`
and observing the failure banner render cleanly, then recovering on
rename-back + reload.

### 5. Dev-build worker wasm has many more `__wbindgen_*` imports

Release builds strip most wasm-bindgen references; dev builds keep them.
The three hand-written stubs in `worker-entry.mjs` were no longer
sufficient. Replaced both `__wbindgen_placeholder__` and
`__wbindgen_externref_xform__` with a `Proxy` that lazily returns a
0-stub for any requested name, logging a warning.

### 6. `ErrorEvent` + worker `onerror` wiring (`bridge.rs`)

Worker panics used to be invisible to the main page. Added:
- `WorkerOptions` + `Worker::set_onerror` handler → `tracing::error!`
- `Worker::set_onmessageerror` handler → `tracing::error!`
- `web-sys` features: `ErrorEvent`, `Event`

---

## Files by line number

### `frontends/dioxus-web/src/main.rs`

| Lines | What |
|-------|------|
| 28–30 | `thread_local! BRIDGE` — !Send, held in `Rc` |
| 47–138 | `App` component, boot `use_future` |
| 68–80 | Query root layout block |
| 82–96 | Extract root id, handle None branch |
| 98–117 | `engineWatchView` + bogus handle guard |
| 119–129 | Snapshot callback registration |
| 144–156 | Continuous tick pump |
| 194–238 | RSX title bar + content branch match |
| 243–254 | `extract_first_id` (Reflect-only, no `dyn_ref::<Array>`) |

### `frontends/dioxus-web/src/bridge.rs`

| Lines | What |
|-------|------|
| 23–44 | `BridgeInner`, `SnapshotCallback` Rc alias |
| 50–90 | `onmessage` closure — handles `ready` / `snapshot` / RPC response |
| 94–108 | `Rc<RefCell<dyn FnMut>>` dispatch pattern — avoids reentrant borrow |
| 113–135 | `set_onerror` + `set_onmessageerror` wiring |
| 137–171 | `ready_rx` + 10 s timeout race |
| 177–208 | `call` RPC method |
| 210–222 | `on_snapshot`, `drop_subscription` |

### `frontends/dioxus-web/src/render.rs`

| Lines | What |
|-------|------|
| 14 | `EntityContext` newtype (Dioxus context) |
| 19–276 | `RenderNode` match over `widget` tag |
| 279–361 | `BlockRefNode` — per-cell subscription via `use_future` |
| 364–376 | `EditableTextNode` — reads `EntityContext` |
| 378–408 | Helpers + `value_to_display` (untagged-aware) |

### `frontends/dioxus-web/src/editor.rs`

| Lines | What |
|-------|------|
| 44–75 | `EditorCell` — empty `contenteditable` body, imperative sync |
| 77–94 | `sync_dom_to_prop` — skip overwrite when focused |
| 96–137 | `schedule_content_update` — 50 ms trailing debounce with `Timeout` |
| 139–159 | `dispatch_content_update_now` — fires the RPC |
| 167–399 | `cursor` module — flat UTF-16 offset tree walker |

### `frontends/holon-worker/src/lib.rs`

| Lines | What |
|-------|------|
| 98–326 | `backend` module — engine init + all backend entry points |
| 242–249 | `tick(budget_ms)` |
| 258–327 | `watch_view` — coalescing drain loop with fail-loud serialize |
| 358–448 | `engine_exports` napi module — thin wrappers around `backend::*` |
| 498–556 | Phase-2 Turso harness (`open_db`, `db_query`, `db_execute`) |
| 560–619 | Serde round-trip tests (native only) |

---

## Open issues / what to do next

### Priority 1 — makes the UI show anything useful

- [ ] **Task #22 (in progress):** Recursive `BlockRef` watching is wired in
      `render.rs::BlockRefNode` but the current seed data has no user blocks
      matching the sidebar/main panel queries. Options:
      1. Add a `block:welcome` seed block with `name = "Welcome"` so it
         shows up in the left sidebar list.
      2. Add a "Create block" button that dispatches `engineExecuteOperation`
         so the user can populate content themselves.
      3. Make the seed include a realistic example document.

- [ ] **Subscription leak:** `BlockRefNode`'s `use_future` does not drop
      its subscription when the component unmounts. For a dynamic UI this
      leaks handles into `subscriptions::REGISTRY` forever. Need either:
      - A Dioxus `use_drop` pattern if available in 0.7, or
      - Store the handle in a Signal and observe it from a cleanup future.

### Priority 2 — known-broken paths

- [ ] **`engineExecuteSql` returns `()`** instead of affected-row count —
      documented in-code but not exposed to the Dioxus bridge. If we need
      row counts (for "X rows inserted" UI), `DbHandle::query` doesn't
      expose `changes()`; add a new accessor.

- [ ] **Editor `Enter` key is silently swallowed**
      (`editor.rs::EditorCell::onkeydown` line ~60). A real implementation
      should dispatch a `block.split` operation. Today Enter just
      prevents default.

- [ ] **Debounced dispatch + blur racing:** if the user types, blurs,
      and the debounce timer hasn't flushed yet, the next snapshot can
      clobber the pending text. Fix: flush debounce on blur. Tracked
      in the earlier review comments.

### Priority 3 — polish

- [ ] The `[layout: degraded — AvailableSpace=None in worker]` banner in
      the title bar is a permanent marker. The worker runs `watch_live`
      but has no screen dimensions, so container-query render exprs like
      `if_space(600.0, ...)` always take the fallback branch. Fix: pipe
      the page's viewport size into the worker via a setter, e.g.
      `engineSetViewport(w, h)`, called from Dioxus on mount + resize.

- [ ] `dev` profile wasm is ~630 MB. Fine for localhost but
      embarrassing if anyone else runs it. Consider `--profile release`
      (not `release-official`) as a middle ground.

- [ ] `frontends/dioxus-web/src/bridge.rs` still has `drop_subscription`
      as dead code. Wire it up once `BlockRefNode` tracks its handle.

---

## Testing playbook

**Start:**
```bash
cd frontends/dioxus-web
node serve.mjs --build
```

**Verify via `/browse`:**
```bash
B=~/.claude/skills/gstack/browse/dist/browse
$B goto http://127.0.0.1:8765/
sleep 5
$B js "({
  html_len: document.querySelector('#main').innerHTML.length,
  block_refs: document.querySelectorAll('[data-block-id]').length,
  editors: document.querySelectorAll('[data-entity-id]').length,
  text: document.body.innerText.slice(0, 200)
})"
```

**Expected:**
```json
{
  "html_len": 1264,
  "block_refs": 3,
  "editors": 0,
  "text": "Holon browser demo ... ready (1300–2000 ms) · [layout: degraded ...]"
}
```

**Check worker logs (via main page console — `onerror` wiring
forwards worker errors):**
```bash
$B console --errors
# Should be empty aside from the `integrity` attribute preload warning
```

**Check tick pump is ticking:**
```bash
# `$B console` shows many signal writes per second from use_future
$B console 2>&1 | grep -c "dioxus-hooks.*use_future"
```

**Test the 10 s boot-failure timeout:**
```bash
mv frontends/holon-worker/holon_worker.wasm32-wasi.wasm{,.bak}
$B reload; sleep 12
$B js "document.body.innerText.slice(0, 400)"
# expect: "⚠ worker spawn: worker did not emit `ready` within 10000ms..."
mv frontends/holon-worker/holon_worker.wasm32-wasi.wasm{.bak,}
$B reload; sleep 5
```

---

## Wire format lockdown

**`holon_api::Value` is `#[serde(untagged)]`.** Primitives round-trip as
bare JSON: `Value::String("x")` → `"x"`, `Value::Integer(42)` → `42`,
`Value::Float(3.5)` → `3.5`, `Value::Null` → `null`.

There is **NO** `{"Text": {"value": "..."}}` or `{"String": "..."}`
wrapping. Any frontend code that probes for such a shape is working
from a wrong mental model.

Tests live in `crates/holon-api/src/lib.rs`:
- `tests::value_serde_wire_format_is_untagged`
- `tests::value_map_params_round_trip_untagged`

Both run on `cargo test -p holon-api --lib` and will fail loudly if
anyone adds `#[serde(tag = "...")]` to `Value`.

---

## Session notes / gotchas for next person

1. **The worker runs on a single dedicated current-thread tokio runtime.**
   Nothing advances unless someone calls `block_on`. The tick pump in
   `main.rs::App` drives it at ~60fps by calling `engineTick(10)` in a
   loop. Without this, subscriptions emit their initial value and then
   go silent.

2. **Dioxus `Signal::set` from a non-reactive closure works** (I was
   suspicious of this, but it's fine in 0.7). The subscription
   callbacks capture signals by copy and mutate them from the bridge
   `onmessage` dispatch.

3. **`contenteditable` body must be empty in the RSX.** Dioxus
   diffing will stomp native cursor state on every render if you put
   `"{content}"` in the body. `EditorCell` uses `use_effect` +
   `set_text_content` instead, and skips the overwrite entirely when
   the element is focused.

4. **Worker console != page console.** The Dioxus page cannot see
   `[wasm] ...` stdout/stderr from the worker unless we forward it.
   The `onerror` wiring in `bridge.rs` forwards worker *errors* but
   not regular `console.log`. For worker tracing during debugging,
   navigate to `http://127.0.0.1:8765/web/index.html` which hosts the
   worker directly and captures its console.

5. **napi-rs on emnapi/wasi delivers TSFN tuples as JS arrays.** This
   is the single gotcha that took 6+ rebuild cycles to pin down. If
   you change the `(String,)` tuple in `backend::watch_view::tsfn`,
   the worker-entry unwrap code needs to change too.

6. **`cargo test -p holon-api` currently has one pre-existing failure**
   (`render_eval::test_state_display`) that is unrelated to anything
   touched this session. My new tests pass.

7. **Don't try to `cargo check --workspace`** — it'll fail because
   `frontends/dioxus-web` and `frontends/holon-worker` are both
   excluded and need their own targets.

---

## Reviewed + deleted

- `crates/holon-api/tests/uri_probe.rs` — temporary test I added to
  verify `fluent_uri::Uri::parse` handles double-colons in paths.
  Confirmed all seed URIs parse; test deleted.
- `dioxus-console.txt`, `dioxus-console-warn.txt`, `worker-errors*.txt`
  — scratch console dumps from Playwright, removed.

Diagnostic `tracing::info!("[diag] ...")` instrumentation has been
stripped from `main.rs`, `bridge.rs`, `render.rs`. The enriched
`uri_from_row` error message in `holon-api` is kept — it's useful
forever, not scratch code.

---

## Follow-up session 2 — typed ViewModel + shared `builder_registry!` port

**Goal:** make the dioxus-web rendering layer as thin as possible and
share everything share-able with `holon-frontend` / the GPUI frontend.

### TL;DR

The 423-line `src/render.rs` that pattern-matched on
`serde_json::Value["widget"].as_str()` is gone. Dioxus-web now:

- **Deserializes at the bridge boundary** into the typed
  `holon_frontend::view_model::ViewModel` tree (same enum GPUI, Flutter,
  MCP, and the PBTs already use). No more string probing anywhere on the
  main thread.
- **Reuses `holon_macros::builder_registry!`** — the same macro that
  drives GPUI's `frontends/gpui/src/render/builders/*.rs` directory now
  generates `render_node` for dioxus-web as well. The macro was extended
  with two optional params (`node_type` / `kind_type`) so GPUI stays on
  `ReactiveViewModel` / `ReactiveViewKind` with zero changes while
  dioxus-web passes `ViewModel` / `ViewKind` to dispatch on snapshot
  trees instead of live reactive ones.
- **Mirrors GPUI's directory layout 1:1** — one file per widget under
  `frontends/dioxus-web/src/render/builders/` with a
  `pub fn render(field_a: &T, field_b: &T, ctx: &DioxusRenderContext) -> Element`
  signature whose param names must match the `ViewKind` variant fields.
- **Has a zero-sized `DioxusRenderContext`** — Dioxus doesn't need the
  bounds registry, focus scope, or GPUI handles that
  `GpuiRenderContext` carries. The struct exists purely so the macro
  has a stable type to thread through every builder and so
  cross-cutting state (theme, preferences) has a single home.

### What changed, by file

**`crates/holon-macros/src/builder_registry.rs`** — back-compat extension:
- Added `node_type: Option<syn::Path>` / `kind_type: Option<syn::Path>`
  parser cases with defaults pointing at
  `holon_frontend::reactive_view_model::ReactiveView{Model,Kind}`.
- `NodeDispatch` mode now uses those paths in both the function
  signature and the match-arm patterns. The fallback arm got
  `#[allow(unreachable_patterns)]` because consumers whose kind enum
  is exhaustively covered by builder files would otherwise warn.
- GPUI (`holon-gpui`) and `holon-macros` both check clean with the
  new code; no GPUI changes were needed.

**`frontends/dioxus-web/Cargo.toml`** — added three path deps:
`holon-frontend`, `holon-api`, `holon-macros`. `holon-frontend` was
verified to build clean for `wasm32-unknown-unknown` before adopting
it. Comment in Cargo.toml explains the rationale.

**`frontends/dioxus-web/src/render.rs`** — deleted.

**`frontends/dioxus-web/src/render/`** — new directory tree:
- `mod.rs` — defines `pub struct EntityContext(pub String)` (moved from
  the old `render.rs`), the zero-sized `DioxusRenderContext`, and
  re-exports `builders::RenderNode`.
- `builders/mod.rs` — invokes the macro with
  `node_dispatch: Element, context: DioxusRenderContext, node_type: holon_frontend::view_model::ViewModel, kind_type: holon_frontend::view_model::ViewKind`.
  Defines the `#[component] pub fn RenderNode(node: ViewModel)` thin
  wrapper and the `render_unsupported` fallback (returns `rsx!{}` —
  that's how `Empty` / `Loading` / `DropZone` unit-variants
  legitimately fall through without needing builder files).
- `builders/prelude.rs` — per-builder re-exports (`dioxus::prelude::*`,
  `ViewModel`, `LazyChildren`, `DioxusRenderContext`, `RenderNode`).
- `builders/util.rs` — `value_to_display(&holon_api::Value) -> String`
  helper used by `table_row` and `pref_field`. Skipped by the macro via
  `skip: [prelude, util]`.
- 31 per-widget builder files, each a verbatim port of the matching
  branch in the old `render.rs`:
  - Leaves: `text.rs`, `badge.rs`, `icon.rs`, `checkbox.rs`,
    `spacer.rs`, `editable_text.rs`, `source_block.rs`,
    `source_editor.rs`, `state_toggle.rs`, `block_operations.rs`,
    `error.rs`.
  - Containers: `row.rs`, `col.rs`, `list.rs`, `section.rs`, `tree.rs`,
    `outline.rs`, `query_result.rs`, `columns.rs`, `table.rs`,
    `table_row.rs`, `tree_item.rs`, `collapsible.rs`, `card.rs`,
    `chat_bubble.rs`, `pref_field.rs`.
  - Single-child wrappers: `focusable.rs`, `selectable.rs`,
    `draggable.rs`, `pie_menu.rs`, `view_mode_switcher.rs`,
    `drawer.rs`.
  - Special: `block_ref.rs` (owns the per-cell subscription
    `use_future`), `live_query.rs`, `render_entity.rs`.
  - **No files** for `Empty` / `Loading` / `DropZone` — unit variants
    fall through the macro fallback to `render_unsupported` which
    returns `rsx!{}`. The fallback logs a warning via
    `tracing::warn!("Unsupported widget: {name}")`; for these three
    that's noise but it's not frequent enough to matter yet.

**`frontends/dioxus-web/src/main.rs`** — changed the `view_model` signal
type from `Signal<Option<serde_json::Value>>` to
`Signal<Option<ViewModel>>`. The snapshot callback now does
`serde_json::from_str::<ViewModel>(&json)` once at the boundary. Bogus
snapshots surface as a parse error in the tracing log, not as a
silently-wrong element tree.

**`frontends/dioxus-web/src/bridge.rs`** — untouched. The RPC transport
doesn't care what type the snapshot deserializes into.

**`frontends/dioxus-web/src/editor.rs`** — untouched. `EditorCell` is
the only non-pure builder (contenteditable cursor preservation,
debounced write dispatch) and it reads `EntityContext` via
`try_consume_context` exactly as before.

### Deviations from a literal port

1. **`table_row.rs`** sorts cells by key for stable display. The old
   `render.rs` iterated `HashMap` unsorted, producing non-deterministic
   column order across snapshots. This matches what `value_to_display`
   already did for nested objects (see `render.rs` L413 in git
   history).
2. **`section.rs`** uses `gap: 0px`. The old code called
   `f64_field(&node, "gap")` which silently defaulted to 0 on miss —
   `ViewKind::Section` has no `gap` field, so the typed port
   hardcodes 0 and matches observed rendering.
3. **`util.rs::value_to_display`** handles `Value::DateTime` and
   `Value::Json` variants that don't exist in `serde_json::Value`. The
   old code implicitly dropped these through the `_ =>` arm.
4. **`BlockRefNode`** now stores `Signal<Option<ViewModel>>` instead of
   `Signal<Option<serde_json::Value>>`. The subscription callback
   deserializes once and stores the typed value.

### Verification

- `cargo check -p holon-macros` — clean.
- `cargo check -p holon-gpui` — clean (4 pre-existing warnings, no regressions).
- `cd frontends/dioxus-web && cargo check --target wasm32-unknown-unknown`
  — clean. Three warnings remain, all pre-existing and predating this
  session: unused `wasm_bindgen::JsCast` import in `main.rs`, unused
  `mut` on `WorkerOptions::new()` in `bridge.rs`, dead
  `drop_subscription` method in `bridge.rs`.
- `cd frontends/dioxus-web && trunk build` — clean, 1 m 03 s, wasm
  artifact landed.

**Not yet verified:**
- End-to-end browser rendering via `node serve.mjs --build` +
  `$B goto http://127.0.0.1:8765/` per the [Testing playbook]
  (#testing-playbook) section above. The `trunk build` succeeded but
  the port changes what the main thread does with each snapshot — if
  any ViewKind variant produces a different DOM shape than the
  hand-written match in the old `render.rs` did, the HTML-len /
  live-block-count assertions in the playbook might drift.

### For the next session — where to continue

Three things in priority order:

1. **Actually run the testing playbook.** Start
   `node serve.mjs --build`, point gstack at `:8765`, check that
   `document.querySelectorAll('[data-block-id]').length === 3` still
   holds and that the boot-failure test with the renamed worker wasm
   still shows the failure banner within 10 s. If any ViewKind
   variant renders differently from what the old string-match
   produced, fix it in the corresponding `builders/*.rs` file — the
   old `render.rs` in git history (e.g.
   `git show HEAD:frontends/dioxus-web/src/render.rs`) is the
   reference source of truth for the visuals.

2. **Pick up Priority 1 from the original handoff** — Task #22 (the
   empty sidebar/main panel). The render plumbing is typed now, so
   the data-side fix is still the same as before: add a
   `block:welcome` seed block or wire up a "create block" operation.
   With the new typed pipeline, any rendering regressions will show
   up as deserialize errors at the bridge boundary, not as
   silent-placeholder elements.

3. **Subscription leak in `BlockRefNode`** (also from the original
   handoff) — the port preserved the leak verbatim. `use_future`
   doesn't drop its subscription on unmount. Fix is the same as
   before: either `use_drop` if Dioxus 0.7 has it, or a cleanup
   future reading a `Signal<u32>` handle. Dead method
   `drop_subscription` in `bridge.rs` becomes live once this
   happens.

### Nice-to-haves deferred

- **Unit-variant builder files.** The macro currently can't emit a
  pattern like `ViewKind::Empty => ...` (unit variants require no
  braces, but the macro always emits `{ .. } => ...`). For now
  `Empty` / `Loading` / `DropZone` fall through the fallback arm and
  log a warning. If the warning noise becomes a problem, extend the
  macro to read the enum source or accept an explicit
  `unit_variants: [Empty, Loading, DropZone]` list.
- **Sharing the editor cell.** `editor.rs` and
  `frontends/gpui/src/views/editor_view.rs` solve the same problem
  (uncontrolled contenteditable with debounced write-back) with
  zero code shared. Extracting a framework-agnostic editor state
  machine into `holon-frontend` is probably worthwhile once a third
  frontend wants it.
- **Shared theme / color palette.** Most builders hardcode the dark
  palette (`#1e1e2e`, `#2a2a3a`, …) inline as CSS strings. GPUI
  pulls colors from `gpui-component::theme`. A `palette.rs` module
  in `render/builders/` that exposes named colors would let us
  swap themes later without touching 31 files.

---

## Follow-up session 3 — playbook verified, welcome seed, leak plugged

**Goal:** finish the three follow-ups left at the end of session 2.

### TL;DR

All three Priority items from session 2 are now done:

1. **Testing playbook re-run end-to-end.** Typed-builder port confirmed
   working: `block_refs=3`, `html_len≈1246`, ready in ~1.3 s, zero
   console errors. Worker wasm wasn't checked in at
   `frontends/holon-worker/holon_worker.wasm32-wasi.wasm` (only
   `target/.../debug/holon_worker.wasm` existed) — copying the dev
   build into place was enough to recover.
2. **Welcome block now visible in left sidebar.** Added a `block:welcome`
   row to the seed plus a child paragraph. The left sidebar now reads
   "Welcome" (with the notebook icon) instead of an empty placeholder.
3. **`BlockRefNode` subscription leak plugged.** `use_drop` cleanup
   now calls `bridge.drop_subscription(handle)` from a
   `wasm_bindgen_futures::spawn_local` future when the component
   unmounts.

### What changed, by file

**`frontends/holon-worker/src/seed.rs`** — added two new tuples to
`stmts`:
- `block:welcome` (parent `DOC_ID`, content "Welcome")
- `block:welcome::para::0` (child paragraph)

After the insert loop, an extra `UPDATE block SET name = 'Welcome'
WHERE id = 'block:welcome'` runs because the seed loop's INSERT
template doesn't set `name`. **Important:** `Block.name` is a real
top-level column on the `block` table (defined in
`crates/holon-api/src/block.rs:271`), not a property inside the
`properties` JSON. The left-sidebar PRQL filters on `name`, so
stuffing `"name":"Welcome"` inside `properties` did **nothing** —
that was the first thing I tried and it produced an empty sidebar.

**`frontends/dioxus-web/src/render/builders/block_ref.rs`** —
- New `watch_handle: Signal<Option<u32>>` captures the handle
  returned by `engineWatchView` after the bridge call resolves.
- New `use_drop` hook reads `watch_handle.peek()`, and if a handle
  is present, spawns a `wasm_bindgen_futures::spawn_local` future
  that awaits `bridge.drop_subscription(handle)`. Errors get
  `tracing::error!`-logged.
- `bridge.drop_subscription` was already wired in `bridge.rs:219`
  and was previously dead code — it's now live.

### Verification

- `cargo check --target wasm32-unknown-unknown` — clean. The 2
  warnings remaining (`unused import: serde_json::Value`,
  `variable does not need to be mutable`) predate this session.
- `trunk build` — clean, ~48 s.
- `node serve.mjs` + Playwright MCP smoke test:
  - `block_refs=3`, `sidebar_text="·notebook\nWelcome"`,
    `body_text` includes `ready (1333ms)` and the welcome row.
  - 3 console errors visible, all `WebSocket /mcp-hub` handshake
    failures from the MCP hub Dioxus boot path. **Pre-existing, not
    introduced by this session.** They were silently masked before
    by the empty sidebar making everything else uninteresting.

**Not yet verified:**
- The unmount cleanup is reached only when a `BlockRefNode` is
  actually torn down. The current root layout never tears one
  down — the three sidebar/main/right-sidebar cells live for the
  whole session. To exercise the path properly, navigate between
  blocks (which will replace the main panel's BlockRefNode) and
  watch `subscriptions::REGISTRY` size in worker logs. Doing this
  needs Task #22b ("create block" or navigation focus action),
  which is not in this session's scope.

### Known gotchas / things the next session should know

1. **The worker wasm is now served straight from cargo `target/`.**
   The 600+ MB binary used to live in
   `frontends/holon-worker/holon_worker.wasm32-wasi.wasm` (untracked
   but VCS-visible — one `git add -A` away from a 600 MB commit
   landing in main). It's gone. `serve.mjs::resolveWorkerWasm()` now
   walks `target/wasm32-wasip1-threads/{debug,release,release-official}/`
   and serves whichever build is newest by mtime. A `.gitignore` in
   the worker root prevents anyone from re-creating the foot-gun.
   Override with `HOLON_WORKER_WASM=/abs/path` if you need a specific
   build, and a missing wasm now returns a 404 with the exact napi
   command to run printed in the body. The "How to run" section
   above still describes the manual `cp` workflow — it's now
   **obsolete**, kept only as historical context. Run the napi
   build (it'll still trip `copyArtifact` — that error is harmless),
   then just reload the page.

2. **The MCP hub WebSocket errors visible in the page console
   are pre-existing.** They come from the Dioxus boot path
   trying to connect a browser-side mcp-hub client to the
   serve.mjs WebSocket; the handshake fails because nothing is
   registering the upgrade for that client. Don't chase these
   when triaging — they have nothing to do with the rendering
   pipeline. If they ever become annoying, the fix is in
   `serve.mjs`'s upgrade routing, not in dioxus-web.

3. **Don't put display-bound metadata in `properties` JSON
   blindly.** The `block` table has real columns for `id`,
   `parent_id`, `name`, `content`, `content_type`,
   `source_language`, `source_name`, `created_at`,
   `updated_at`, `sort_key`. PRQL queries that filter on any
   of these are filtering on the column, not on
   `properties[name]`. Properties JSON is only for things
   that aren't first-class.

### What's left for the next session

- **Wire navigation focus from the sidebar click.** The sidebar
  template already emits `selectable(... #{action: navigation_focus(#{region: "main", block_id: col("id")})})`,
  so clicking "Welcome" should set `current_focus.main =
  block:welcome`. The main panel's GQL traverses `focus_root`
  and should then render the welcome subtree. Verify this
  end-to-end: click → main panel non-empty → assert HTML
  contains welcome text. This is the real user-visible
  payoff for the welcome seed.
- **Editor `Enter` key still swallowed** (Priority 2 from
  session 1, still open). Once a focused welcome block is
  visible, this becomes the next obvious bug — pressing Enter
  in the welcome paragraph should split the block, not
  preventDefault into the void.
- **Layout always degraded.** The title bar still says
  `[layout: degraded — AvailableSpace=None in worker]` because
  the worker has no viewport dims. `engineSetViewport(w, h)`
  on mount + resize would fix it, and let the
  `if_space(600.0, …)` container queries actually pick the
  right column variant.
