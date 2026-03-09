---
title: Holon browser demo — wasm32 port handoff
date: 2026-04-14
status: ready-to-execute
---

# Mission

Get Holon to run as a pure-browser demo — no backend server, no MCP, no
filesystem org sync — so anyone can try it from a static page without
installing anything. Target: **Dioxus Web** (`wasm32-unknown-unknown`),
in-memory Turso, Loro snapshot seeded from `include_bytes!`.

# What is already proven (do not re-litigate)

A throwaway spike at `/tmp/holon-wasm-spike/` compiles `turso` + `loro` to
`wasm32-unknown-unknown`, links to a 12 MB release wasm, and runs successfully
in a real browser. Verified output from the spike:

```
loro snapshot bytes: 278
turso rows: 1:alice, 2:bob
[init ok]
```

That proves:

- `turso::Builder::new_local(":memory:").build().await` works in-browser.
- `CREATE TABLE`, `INSERT`, `SELECT`, async row iteration all work.
- `LoroDoc::new()`, text insert, `export(ExportMode::Snapshot)` all work.
- The `wasm32-unknown-unknown` target is usable — we do **not** need the
  napi-based `wasm32-wasip1-threads` path that upstream Turso uses for
  its `@tursodatabase/database-wasm` npm package.

The spike crate and its working `Cargo.toml` are still on disk — consult
them as ground truth when the holon port runs into the same errors.

# Out of scope for this handoff

- **OPFS persistence.** In-memory Turso only. Turso's OPFS backend lives in
  the napi binding (`bindings/javascript/src/browser.rs`), depends on
  `wasm32-wasip1-threads` + raw `extern "C"` imports, and is not reusable
  from a `wasm32-unknown-unknown` blob. Fresh DB per page load is acceptable
  for a demo. If you want pre-seeded content, bake a Loro snapshot into the
  wasm with `include_bytes!` and import on first boot.
- **Org-mode sync.** Do not try to compile `holon-orgmode` or
  `holon-filesystem`. Exclude them from the wasm build entirely.
- **MCP server.** Skip `holon-mcp`, `holon-mcp-client`.
- **Todoist sync, enigo input driver.** Skip.

# Known patches required (upstream status)

1. **`turso_core/io/clock.rs` — use `web-time` on wasm targets.**
   UPSTREAMED (user confirmed done 2026-04-14). If a fresh
   `cargo update -p turso_core` pulls it, you are good. If not, re-apply
   locally: swap `std::time::{Instant, SystemTime}` for `web-time::{Instant,
   SystemTime}` under `#[cfg(target_arch = "wasm32")]` in
   `core/io/clock.rs`, and add `web-time = "1"` under
   `[target.'cfg(target_family = "wasm")'.dependencies]` in
   `core/Cargo.toml`.

2. **Turso's `workspace-hack` crate force-enables features that don't
   build on wasm.** NOT YET UPSTREAMED. The hakari-managed file at
   `turso/workspace-hack/Cargo.toml` unifies `tokio = ["full"]`,
   `hyper = ["server"]`, `pyo3 = ["extension-module"]`,
   `rusqlite = ["bundled"]`, forcing every consumer of any turso crate to
   pull `mio`, native sockets, and a Python extension build. None of those
   compile on `wasm32-unknown-unknown`.

   Workaround used by the spike: stub out
   `/Users/martin/Workspaces/bigdata/turso/workspace-hack/Cargo.toml` to an
   empty package declaration, and patch both `turso_core` and
   `workspace-hack` to the local path in the holon root Cargo.toml:

   ```toml
   [patch."https://github.com/nightscape/turso.git"]
   turso_core = { path = "/Users/martin/Workspaces/bigdata/turso/core" }
   workspace-hack = { path = "/Users/martin/Workspaces/bigdata/turso/workspace-hack" }
   ```

   The original hakari-generated workspace-hack is backed up at
   `/tmp/workspace-hack-original.toml`. **Restore it before running
   `cargo hakari generate`** or you will drift the whole workspace. Long
   term this needs an upstream fix: either split `workspace-hack` into a
   native-only crate and a wasm-safe subset, or teach hakari to generate
   per-platform sections. Not your problem for the demo — just keep the
   path patches until Turso upstream grows wasm CI.

3. **`uuid` needs the `js` feature.** `turso_core` transitively pulls uuid
   without forcing randomness source selection. The top-level crate (here,
   the dioxus-web frontend) must declare uuid directly with `features =
   ["v4", "v5", "v7", "js"]` to unify.

4. **`getrandom` needs the `js` / `wasm_js` feature.** Force via direct
   dependency in the top-level crate:

   ```toml
   [target.'cfg(target_arch = "wasm32")'.dependencies]
   getrandom = { version = "0.2", features = ["js"] }
   getrandom_03 = { package = "getrandom", version = "0.3", features = ["wasm_js"] }
   ```

# The working spike Cargo.toml (copy-paste starting point)

From `/tmp/holon-wasm-spike/Cargo.toml` — this is what a minimal crate that
links turso+loro on wasm32 looks like. Use it as a template when configuring
the dioxus-web frontend:

```toml
[dependencies]
loro = "1.0"
uuid = { version = "1", features = ["v4", "v5", "v7", "js"] }
turso = { git = "https://github.com/nightscape/turso.git", branch = "holon", default-features = false }
turso_core = { git = "https://github.com/nightscape/turso.git", branch = "holon", default-features = false, features = ["json"] }

[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio = { version = "1", features = ["rt", "macros"] }

[target.'cfg(target_arch = "wasm32")'.dependencies]
wasm-bindgen = "0.2"
wasm-bindgen-futures = "0.4"
web-sys = { version = "0.3", features = ["console"] }
getrandom = { version = "0.2", features = ["js"] }
getrandom_03 = { package = "getrandom", version = "0.3", features = ["wasm_js"] }

[patch."https://github.com/nightscape/turso.git"]
turso_core = { path = "/Users/martin/Workspaces/bigdata/turso/core" }
workspace-hack = { path = "/Users/martin/Workspaces/bigdata/turso/workspace-hack" }
```

# Order of attack

## Phase 1 — Get `holon` to cargo-check on wasm32

Currently `crates/holon/Cargo.toml` uses `tokio = { workspace = true, features = ["full"] }`. `full` pulls `net` → `mio` → breaks on wasm32.

1. Narrow tokio features per-target in the workspace root Cargo.toml:

   ```toml
   tokio = { version = "1", default-features = false, features = ["sync", "macros", "rt", "time"] }
   ```

   And where crates explicitly opt into `"full"`, gate them with
   `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`.

2. Gate all filesystem modules in `crates/holon` with
   `#[cfg(not(target_arch = "wasm32"))]`. The grep hit list from the spike
   investigation:

   ```
   crates/holon/src/sync/loro_block_operations.rs
   crates/holon/src/core/datasource.rs
   crates/holon/src/api/loro_backend.rs
   crates/holon/src/sync/iroh_sync_adapter.rs
   crates/holon/src/api/operation_dispatcher.rs
   crates/holon/src/core/operation_log.rs
   crates/holon/src/navigation/provider.rs
   ```

   Each references `std::fs` / `tokio::fs` / walkdir / notify. Some are
   whole modules that can be `#[cfg]`-excluded at `mod` declaration;
   others need narrower gates around specific functions. Do NOT try to
   refactor these into traits yet — a blunt `#[cfg]` exclusion is fine for
   the demo; refactoring comes later if someone wants persistence.

3. Exclude entire crates from the wasm build by gating their inclusion in
   the workspace dependency graph: `holon-orgmode`, `holon-filesystem`,
   `holon-mcp-client`, `holon-todoist`, `holon-frontend` (contains
   `enigo` — an OS input driver). The cleanest way is
   `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` for whatever
   crate currently pulls them in.

4. `holon` likely imports filesystem-bound things at the crate root. You
   may need to split `crates/holon/src/lib.rs` so the wasm build re-exports
   only the `api` + `core` + `engine` surface. Expect this to be the
   messiest part of the phase.

5. Success criterion: `cd /Users/martin/Workspaces/pkm/holon && cargo
   check --target wasm32-unknown-unknown -p holon 2>&1 | tee /tmp/holon-wasm-check.log`
   finishes with zero errors. Do this WITHOUT any dioxus involvement —
   prove the backend crate graph is wasm-clean first.

6. Add the `[patch]` block from above to the workspace root Cargo.toml so
   turso builds. You will also need to hoist the `uuid = {..., "js"}` and
   `getrandom = {..., "js"}` workarounds into whatever crate ends up being
   the wasm entry point (the dioxus-web crate).

## Phase 2 — Get `frontends/dioxus` to build for web

The `frontends/dioxus` bookmark is currently desktop-webview (`wry`). The
RSX, builder layer, operations, CDC bridge, and render interpreter are all
portable — they emit HTML and already use CSS custom properties. The
non-portable parts are `main.rs` (launcher) and the tokio runtime setup.

Reference: `frontends/dioxus/HANDOFF.md` (the original author's handoff)
documents what is done and what is stubbed. Read it before touching the
crate.

1. Create a new workspace member `frontends/dioxus-web` (or add a
   `web` feature to the existing crate — your call; cleanest is a sibling
   crate so the desktop bookmark remains buildable).

2. Swap the launch path: `dioxus = { features = ["web"] }`, use
   `dioxus_web::launch` in place of `LaunchBuilder::new().desktop(...)`.

3. Replace the tokio runtime setup with `wasm_bindgen_futures::spawn_local`.
   The spike's `src/lib.rs` shows the cfg pattern — mirror it.

4. Gut the `FrontendSession` startup so it only initializes:
   - a Turso `:memory:` connection
   - a `LoroDoc` (optionally imported from `include_bytes!("seed.loro")`)
   - the `UiWatcher` / `watch_ui` stream
   No `OrgSyncController`, no file watchers, no MCP server, no iroh sync.

5. Seed content. Options, in ascending effort:
   a. Hard-code a few blocks via direct SQL `INSERT`s on first boot.
   b. Bake a `LoroDoc` export into the binary via
      `include_bytes!("seed.loro")` and `doc.import(...)` on boot. Keeps
      identity stable if you want the demo to show CRDT merge behavior.
   Start with (a). (b) is polish.

6. The existing dioxus frontend has a known bug: nested `block_ref` /
   `live_query` don't live-update because their sub-CDC streams get
   discarded. For the demo, decide whether to fix it (convert those
   builders into proper components with their own `use_future` +
   `Signal<WidgetSpec>`) or ship with a static sub-tree. See
   `frontends/dioxus/HANDOFF.md` §1 for context.

7. Success criterion:
   ```
   cd frontends/dioxus-web && trunk build --release
   python3 -m http.server 8765 -d dist
   ```
   Navigate to `http://localhost:8765/` in a browser. Console shows no
   errors. You see something that renders blocks — even if minimal. Use
   Playwright MCP (`mcp__playwright__browser_navigate` +
   `browser_console_messages` + `browser_evaluate`) to verify. Do not
   claim success from compilation alone — the spike teaches us that
   `cargo check` does not catch `todo!()` panics on wasm. You must load
   the page and see blocks render.

## Phase 3 — Polish (only if Phase 2 works)

- Shrink the wasm. Current spike is 12 MB release. Add `[profile.release]
  opt-level = "z"`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`
  and run `wasm-opt -Oz`. Target 3-5 MB before gzip.
- Host on GitHub Pages or similar. `trunk build --release` + a CI job that
  pushes `dist/` to `gh-pages`.
- Add a banner explaining the demo is in-memory and refreshing wipes
  state. Fail-loud, never-fake (see `CLAUDE.md`).

# Reality checks you must do, in order

1. Compile-time only proves the crate graph links. The spike hit a runtime
   panic (`std::time::Instant::now` on wasm) that `cargo check` missed. After
   every milestone, load the wasm in a real browser via Playwright and
   confirm no `RuntimeError: unreachable`.
2. `holon` has `crates/holon/src/api/loro_backend.rs` and
   `crates/holon/src/core/datasource.rs` using `std::fs` in code paths that
   may or may not be reachable at runtime. Gate them pessimistically —
   easier to un-gate later than to chase a panic.
3. If you hit a wall on `holon-core` or `holon-engine` where the fs gating
   becomes invasive, stop and re-evaluate. The demo goal is binary: it
   either boots or it doesn't. Don't refactor for elegance mid-port.

# Files worth reading before starting

- `/tmp/holon-wasm-spike/Cargo.toml` — working wasm32 deps + patches
- `/tmp/holon-wasm-spike/src/lib.rs` — working spike code, native+wasm cfg pattern
- `/Users/martin/Workspaces/pkm/holon/frontends/dioxus/HANDOFF.md` — prior dioxus frontend state
- `/Users/martin/Workspaces/pkm/holon/CLAUDE.md` — project rules (fail loud, no defensive programming, parse don't validate)
- `/Users/martin/Workspaces/pkm/holon/devlog/2026-04-14-*.md` — prior session logs if relevant

# Definition of done

A static site, served from any plain HTTP server, that:

1. Loads a `.wasm` ≤ 5 MB (gzipped) without console errors.
2. Instantiates Turso in-memory + Loro.
3. Renders at least one screen of Holon's UI (root layout, some blocks)
   via Dioxus Web.
4. Interactive — clicking a block, toggling state, or typing in an editable
   field dispatches the operation and the UI updates via the CDC stream.
5. Refreshing the page wipes state (expected — in-memory only).

Anything less is not shipping. Anything more is bonus.
