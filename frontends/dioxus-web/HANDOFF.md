# Handoff — getting `holon-dioxus-web` (wasm) running in a browser

**Audience:** whoever picks up the browser frontend next.

> **Stale references below:** `frontends/dioxus/` (the desktop port this
> document repeatedly cites as "the working reference" for the render layer)
> has been deleted. Those passages are kept as a record of how the browser
> render layer was derived; `frontends/dioxus-web/src/render/builders/` is now
> the only copy.

## ✅ RUNS IN A BROWSER (2026-06-18, W4 follow-on, end-to-end)
Both wasm crates compile, `node serve.mjs` serves them, and the app **boots in a
real browser (verified via Playwright): "ready (~2 s)", the wasm worker engine
initializes + seeds, and the UI renders `data-block-id` elements.** COOP/COEP +
SharedArrayBuffer + wasi-threads all work.

### Getting it running (exact steps that worked)
1. `rustup target add wasm32-unknown-unknown wasm32-wasip1-threads`
2. Worker npm deps (more than README listed): in `frontends/holon-worker`,
   `npm install` then `npm install --save-dev @emnapi/core @napi-rs/wasm-runtime @tybys/wasm-util`
   (the last three were missing; the browser importmap needs them or the worker
   module graph 404s and the Worker dies with an opaque `onerror`).
3. **Build the worker with an absolute `RUSTC`** (critical — see gotcha below):
   ```
   cd frontends/holon-worker
   RUSTC="$(rustup which rustc)" \
   EMNAPI_LINK_DIR="$(pwd)/node_modules/emnapi/lib/wasm32-wasi-threads" \
     ./node_modules/.bin/napi build --features browser --profile dev --platform \
     --target wasm32-wasip1-threads --no-js --manifest-path ./Cargo.toml --output-dir .
   ```
4. `cd frontends/dioxus-web && npm install`
5. Serve the smaller napi-optimized worker wasm (faster instantiation than the
   737 MB raw debug one):
   `HOLON_WORKER_WASM="$(cd ../holon-worker && pwd)/holon_worker.wasm32-wasi.wasm" node serve.mjs --build`
   Open <http://127.0.0.1:8765/>.

### Critical gotcha: `RUSTC` must be absolute (wasi-threads reactor init)
`napi-build` derives `crt1-reactor.o` from `<RUSTC>/../../lib/rustlib/<target>/lib/self-contained/`.
If `RUSTC` is the bare string `rustc` (cargo's default in some shells), the parent
chain collapses to an empty path, `crt1-reactor.o` is **not** linked, `_initialize`
isn't exported, and the worker **hangs/crashes during instantiation** (symptom:
`worker did not emit 'ready'` / opaque `worker onerror {isTrusted:true}`). Setting
`RUSTC="$(rustup which rustc)"` fixes it. The build warning to watch for:
`crt1-reactor.o not found at , the multi-threaded runtime may not be initialized`.
Re-run after `cargo clean -p holon-worker --target wasm32-wasip1-threads` if the
build script is cached (it won't re-run on an env-only change).

### Debugging the worker (it has no visible console from the page)
The worker + its emnapi thread-pool sub-workers log to their own consoles, which
Playwright/the page can't see, and worker errors arrive as opaque
`{isTrusted:true}`. To surface them, `web/worker-entry.mjs` and
`web/wasi-worker-with-opfs-stubs.mjs` now forward `unhandledrejection`/`error` to
the parent as `{kind:'fatal'}` / `{__holon_fatal}`, and `web/index.html` (the
standalone worker harness at `/web/index.html`) prints them. Use that harness to
diagnose worker-side failures.

### Remaining functional gaps (app runs; content is sparse)
1. **`block` has no `name` column.** `crates/holon/sql/schema/block_matview.sql`
   projects block_raw's 17 columns but does **not** extract `name` from the
   properties JSON. The seed's sidebar/journals PRQL (`from block | filter name …`)
   and any `col("name")` therefore reference a missing column → the generated
   `watch_view_*` matviews fail (`⚠ Failed to query matview … watch_view_…`) →
   sidebar/doc lists don't populate. `block_raw` no longer has a top-level `name`
   column either (the seed's old `UPDATE block_raw SET name` was removed — it errored
   `no such column: block_raw.name`; name is now only in `properties`). e2e tests
   still use `block.name`, so a doc-type dynamic-schema view normally provides it —
   that projection isn't materializing for the worker's seeded layout. **Next step:**
   decide how the current schema exposes a doc's display name (project
   `json_extract(properties,'$.name') AS name` in the block matview, or register the
   doc type's name field so its view is created) and update `seed.rs` + the matview
   accordingly.
2. **Layout always degraded** (`[layout: degraded — AvailableSpace=None in worker]`)
   — known limitation: no viewport size is piped to the worker. Documented below.

## ✅ COMPILE FIXED (2026-06-18, W4 follow-on)
`cargo check --target wasm32-unknown-unknown` is now **0 errors / 0 warnings** for
`holon-dioxus-web`. Native (`holon-frontend/api/core/expr`) still compiles — verified.
What it took beyond the builder rot the original handoff predicted:

1. **Builder rot (as predicted):** copied the ~33 pure display builders verbatim from
   `frontends/dioxus/src/render/builders/`, renamed `col.rs` → `column.rs`, kept the two
   worker-bridge builders (`live_block.rs`, `editable_text.rs`) but converted their
   `render()` to the node-based signature, and switched `mod.rs` from `kind_type:` to
   `empty: rsx! {}`.
2. **`workspace-hack` leaked native deps into wasm (the "second wave"):** since 2026-04 the
   workspace adopted cargo-hakari. `workspace-hack` lists `tokio (full → mio)`, `reqwest`,
   `rmcp` as **unconditional** deps, and `dioxus-web` pulls it transitively via the
   `holon-frontend → holon-core → holon-api → holon-expr` path deps → `mio` fails to build
   for wasm. Fix: gated `workspace-hack` behind `[target.'cfg(not(target_arch = "wasm32"))'`
   in those four crates' `Cargo.toml`. (Adding the wasm triple to `.config/hakari.toml`
   does **not** work — hakari simulates all workspace members incl. native-only ones, so
   tokio stays "universal".) ⚠️ `cargo hakari manage-deps` will try to move these lines back
   under `[dependencies]`; keep them gated (comments in each file say so).
3. **Latent `ready_signal` wasm bug in `holon-frontend/src/lib.rs`:** the field is declared
   unconditionally but its two initializers were `#[cfg(not(target_arch = "wasm32"))]`-gated
   → E0063 on wasm. `tokio`'s `sync` feature is present on wasm, so the fix was to drop both
   cfg gates (native behavior unchanged).
4. **Stale API ref in `main.rs`:** `holon_frontend::BLOCK_READ_TABLE_PUB` was removed; the
   const now lives only in native-only `holon-turso`. Replaced the import with a local
   `const BLOCK_READ_TABLE = "block"` (the readiness-probe SQL is sent to the worker, which
   owns the DB) with a sync-warning comment.

**Remaining = the browser bring-up below** (build worker wasm, `node serve.mjs`, smoke test).
That path is untouched by the above and per the original notes "already works", but has not
been re-run since this fix.

---

## Original handoff (build/run infra still accurate)

The build/run *infrastructure* (worker, `serve.mjs`, trunk,
MCP relay) described in `README.md` is still accurate, but the **UI crate no longer
compiles** — it has bit-rotted against `holon-frontend`'s render macro exactly the way the
Dioxus *desktop* frontend had, and which was just fixed under W4. Until the rot below is
cleared, `cargo build --target wasm32-unknown-unknown` fails and there is nothing for trunk
to serve. **Fixing the compile is the whole job; everything else already works.**

Read `README.md` first for the toolchain, the Web-Worker architecture, `serve.mjs`, and the
MCP relay. This handoff only covers what `README.md` is now *wrong* about and how to fix it.

---

## What broke (and why it's the same problem W4 just solved)

`frontends/dioxus-web` and `frontends/dioxus` (desktop) share the **same** render layer:
the `holon_macros::builder_registry!` macro in `node_dispatch` mode + one builder file per
`ViewKind` widget, all emitting `dioxus::prelude::Element`. Since this crate was written
(2026-04), two things changed in shared code and this crate (being workspace-`exclude`d and
wasm-only) was never recompiled, so it silently rotted:

1. **The macro now calls every builder as `render(node, ctx)`** — it passes the whole
   `&ViewModel` plus context, and each builder destructures its own `ViewKind` variant.
   This crate's builders still use the **old destructured-field signature**
   `render(field_a: &T, field_b: &T, …, ctx)` (see `README.md` "Render pipeline", now
   outdated). Result: ~33 builders fail with E0061/E0308 "this function takes N args but 2
   were supplied / expected `&String`, found `&ViewModel`".

2. **The macro's empty/None arm was hardcoded to `gpui::div().into_any_element()`** — a
   gpui-only expression. In a wasm build that's `error: cannot find module `gpui``. W4 made
   that arm configurable via a new optional `empty:` macro parameter (default preserves
   gpui); this crate must pass `empty: rsx! {}`.

Both fixes are already proven: the desktop port in `frontends/dioxus/` did exactly this and
compiles 0 errors / 0 warnings.

> **Dependency:** the `empty:` macro parameter lives in
> `crates/holon-macros/src/builder_registry.rs` and ships with the **W4 branch**
> (`refactor(dioxus): port desktop frontend forward …`). Land or rebase onto that change
> before starting, or the `empty: rsx!{}` line won't parse.

---

## Fix recipe

### 1. Add `empty:` to the macro invocation
In `src/render/builders/mod.rs`, the `builder_registry!` call needs `empty: rsx! {}` (and
the now-unused `kind_type:` arg can be dropped). Mirror what `frontends/dioxus/src/render/builders/mod.rs`
does:

```rust
holon_macros::builder_registry!(
    "src/render/builders",
    skip: [prelude, util],
    node_dispatch: Element,
    context: DioxusRenderContext,
    node_type: holon_frontend::view_model::ViewModel,
    empty: rsx! {},
);
```

### 2. Convert the ~33 display builders to the node-based signature
The desktop port already did this conversion for the identical set of widgets. **The pure
display builders are byte-for-byte reusable** — copy them from `frontends/dioxus/src/render/builders/`.
The mechanical rule (if you'd rather re-derive than copy):

```rust
// OLD (this crate, stale):
pub fn render(content: &String, bold: &bool, _: &f32, color: &Option<String>,
              _: &DioxusRenderContext) -> Element { <body> }

// NEW:
use holon_frontend::view_model::ViewKind;
pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::Text { content, bold, color, .. } = &node.kind else {
        return rsx! {};
    };
    <body — UNCHANGED; bindings are &T with the same names>
}
```

- Variant = PascalCase(filename). **Exception: `col.rs` → `ViewKind::Column`** (widget name
  `"column"`). The macro dispatches on `node.widget_name()` matched against the *filename*,
  so this file must be renamed `column.rs` or `Column` nodes never dispatch. (Desktop already
  renamed it.)
- Drop imports that only existed for old param types (e.g. `RenderExpr` in `live_query.rs`,
  `Value` in `pref_field.rs`, `Arc`/`DataRow` in `table_row.rs`) to keep it warning-clean.
- archlint forbids unused *named* params and `_name`; use a bare `_` for the ctx param when
  the body doesn't use it.

### 3. Keep the two bridge-coupled builders web-specific (don't copy from desktop)
Only `live_block.rs` and `editable_text.rs` differ from desktop. Desktop talks to an
**in-process** `ReactiveEngine`/`FrontendSession`; **web talks to the Web Worker over the
postMessage RPC bridge** (`crate::BRIDGE`, `bridge.call("engineWatchView" | "engineExecuteOperation", …)`).
Keep this crate's worker-bridge versions of `live_block.rs`, `editable_text.rs`, and
`editor.rs` — but still fix their `render()` signatures to the node-based form:

- `live_block.rs`: `let ViewKind::LiveBlock { block_id, content } = &node.kind else {…};`
  then keep the existing `LiveBlockNode` component that opens its own
  `engineWatchView` subscription via `BRIDGE`.
- `editable_text.rs`: `let ViewKind::EditableText { content, .. } = &node.kind else {…};`
  then keep `EditableTextNode` → `EditorCell` (the worker-dispatch editor in `editor.rs`).

### 4. Sanity-check `main.rs` / `bridge.rs` / `render/mod.rs`
These are wasm/worker glue, not render-macro consumers, so they likely still compile — but
verify after the builder fixes. Watch for any `ViewModel`/`ViewKind` field renames since
2026-04 (the destructure in step 2 will flush most of these out).

### 5. Verify the compile
```bash
rustup target add wasm32-unknown-unknown      # if missing
cd frontends/dioxus-web
cargo check --target wasm32-unknown-unknown    # excluded pkg: deps are pinned in its Cargo.toml
```
Goal: 0 errors. (Unlike desktop, this crate is wasm-only and is **not** affected by the
gpui/cocoa workspace conflict, so no member-swap gymnastics are needed — just the wasm target.)

---

## Then: bring it up in the browser (existing, working flow)

Once it compiles, follow `README.md` verbatim — nothing there has changed:

1. **Build the worker wasm** (`frontends/holon-worker`, `napi build … --target wasm32-wasip1-threads`).
   The `copyArtifact` error at the end is harmless; `serve.mjs` reads from `target/` directly.
2. **`cd frontends/dioxus-web && npm install`** (for `serve.mjs`'s `ws` dep).
3. **`node serve.mjs --build`** (or `--watch`), open <http://127.0.0.1:8765/>.
   Expect "ready (≈1.3 s)" and a "Welcome" entry in the sidebar.
4. COOP/COEP headers (for `SharedArrayBuffer`) are already set by `serve.mjs`.
5. Optional: the **browser MCP relay** (terminals 2/3 in `README.md`) to drive the in-browser
   engine from Claude Code.

Smoke test (from `README.md`): `document.querySelectorAll('[data-block-id]').length === 3`,
sidebar contains "Welcome", ready < 3 s, no *new* console errors (the mcp-hub WebSocket
errors are pre-existing).

---

## Risks & unknowns to budget for

- **The builder rot may not be the *only* drift.** It's the certain blocker; once it clears,
  expect a second wave of smaller `holon-frontend`/`holon-api` API changes since 2026-04
  (e.g. `ViewKind` variant fields, `Value` shape, worker RPC payloads). Fix loop:
  `cargo check --target wasm32-unknown-unknown` until green.
- **The worker (`holon-worker`) is a separate wasm crate** with its own `Cargo.lock` and may
  have its own drift. If the page boots but shows nothing, check the worker build and the
  worker console at <http://127.0.0.1:8765/web/index.html> (the page console can't see worker
  logs — `README.md` gotcha #6).
- **Carry the desktop port's lesson:** `dioxus-web`'s builders were the reference the W4
  desktop port started from and are *now behind* it. Treat `frontends/dioxus/` as the source
  of truth for the render layer and diff against it.
- **Known functional gaps** (already documented in `README.md` "Known limitations"): layout
  always degraded (no viewport piped to worker), editor `Enter` swallowed, debounce/blur race,
  640 MB dev wasm. None block "runs in a browser"; address after first paint.

## Pointers
- `frontends/dioxus/` — the W4 desktop port; **the working reference** for the render layer
  and the source of the reusable display builders.
- `frontends/dioxus-web/README.md` — toolchain, worker build, `serve.mjs`, MCP relay (still accurate).
- `crates/holon-macros/src/builder_registry.rs` — the macro + the new `empty:` parameter.
- `devlog/2026-04-15-dioxus-web-handoff.md` — original bring-up history (every bug found first time).
- `frontends/holon-worker/src/seed.rs` — hand-written SQL seed for the in-memory default layout.
