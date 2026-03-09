---
title: Holon browser demo — Phase 2 resume handoff
date: 2026-04-14
status: ready-to-execute
predecessor: devlog/handoff-2026-04-14-holon-wasm-browser-demo.md
---

# Mission

Resume Phase 2 of the Holon browser demo. The previous session got Dioxus
Web mounting in Chromium, Turso `:memory:` opening, the schema DDL
completing, and the first matview being created — then a single
`RuntimeError: unreachable` immediately after `Marked 1 resources as
available`. Goal of this session: drive that `unreachable` to a real
panic message, fix it, and reach the first end-to-end render of any
holon block in the browser.

Read these first, in order:

- `devlog/handoff-2026-04-14-holon-wasm-browser-demo.md` — original Phase 1+2 plan, still the source of truth for scope (in-memory only, no org sync, no MCP, no iroh, no file watchers).
- `devlog/2026-04-14-phase2-progress.md` — what got done and which fixes are where.
- The two most recent `feat(wasm):` commits on the working branch (Phase 1 holon-crate fixes + Phase 2 dioxus-web boot).

# What is already proven (do not re-litigate)

`cd /Users/martin/Workspaces/pkm/holon && cargo check --target wasm32-unknown-unknown -p holon-dioxus-web` is **green**. So is native `cargo check -p holon`.

`dx build --platform web --release` produces a working bundle at
`target/dx/holon-dioxus-web/release/web/public/`. Serve it with
`python3 -m http.server 8766 -d <that path>` and load in any browser.

Confirmed in Chromium (Playwright) — runtime, not just compile-time:

- Dioxus mounts the `App` component, the `use_future` bootstrap runs.
- `holon::di::lifecycle::create_backend_engine(":memory:".into(), |_| Ok(()))` enters the BackendEngine resolution.
- Turso `:memory:` opens via the wasm32 `MemoryIO` branch in
  `crates/holon/src/storage/turso.rs:open_database`.
- WAL initializes, `read_header_page` runs, the schema migration phase
  fires (`DB schema is outdated: 52 < 53` → `txn_finish_result: Ok(Done(()))`).
- The actor logs `[TursoBackend::Actor] DDL completed successfully` and
  `[TursoBackend::Actor] Marked 1 resources as available:
  ["watch_view_d77ac41ba85c1706"]`.
- Then `RuntimeError: unreachable`. No captured Rust panic message.

Do **not** re-litigate the Turso `open_database` wasm branch, the
`spawn_actor` helper, or the `web-time` migration. They work.

# Out of scope (still)

Same as the predecessor handoff. In particular: no OPFS, no org sync,
no MCP, no iroh. The demo is in-memory only.

# Environment / state to carry forward

These edits exist locally and must be preserved. Some are NOT tracked
by `jj` because they live in `~/.cargo/git/checkouts/` and
`/Users/martin/Workspaces/rust/fluxdi`.

## Holon working copy (jj-tracked)

The previous session's last `jj describe` is `feat(wasm): dioxus-web
crate boots holon backend in browser`. It contains all in-tree changes.
Verify with `jj log -r '@-..@'`.

## Out-of-tree, will be erased by `cargo clean`

1. `~/.cargo/git/checkouts/turso-6395fa21babebd65/c4dc3fc/workspace-hack/Cargo.toml`
   — stubbed to empty `[package] name = "workspace-hack"`. Replaces a
   hakari-managed file that unifies tokio full / mio / hyper-server /
   pyo3 / rusqlite-bundled. Not on the active resolution path anymore
   (we moved to cdd46b1c) but keep it stubbed for safety.

2. `~/.cargo/git/checkouts/turso-6395fa21babebd65/cdd46b1/workspace-hack/Cargo.toml`
   — same stub. **CRITICAL**: `name = "workspace-hack"` (not
   `turso-workspace-hack-stub`). An earlier session had it renamed and
   that broke `cargo update -p turso_core` with "no matching package
   named workspace-hack found". If you see that error, this is the
   first thing to check.

3. `/Users/martin/Workspaces/rust/fluxdi/fluxdi/Cargo.toml`: tokio
   features dropped `rt-multi-thread`. Must be present — the holon
   workspace deps `path` it directly.

If any of the above is missing after a `cargo clean`, restoring them
is a 30-second job — see the actual current contents in each file.

## Cargo.toml deps that matter for wasm

- `crates/holon/Cargo.toml` `[target.'cfg(target_arch = "wasm32")'.dependencies]` adds `wasm-bindgen-futures = "0.4"` and `web-time = "1"`.
- `frontends/dioxus-web/Cargo.toml` declares the dioxus features `["web", "macro", "html", "hooks", "signals", "launch"]` (without these the macros + `dioxus::launch` aren't available).
- `Cargo.toml` (workspace root) lists `frontends/dioxus-web` as a member.
- Workspace `fluxdi` is now `path = "/Users/martin/Workspaces/rust/fluxdi/fluxdi"` (not the git URL).

# The actual unreachable

This is your job. Strategy in priority order:

## Strategy A — Make the panic message reach the console (1 hour)

The unreachable at `wasm-function[35639]:0xe47c0c` after `Marked 1
resources as available: ["watch_view_d77ac41ba85c1706"]` is almost
certainly a Rust panic that didn't make it through
`console_error_panic_hook` because the panic message was elided by
release-mode optimization, or because it's actually a wasm
`unreachable` instruction (not a Rust panic).

1. Rebuild with debug symbols + names preserved:
   `dx build --platform web --release` flips on LTO and strips. Try
   `dx build --platform web` (debug profile) first. The wasm will be
   even bigger but stack traces will name functions.

2. If debug build still doesn't print a Rust panic, the unreachable is
   from an actual wasm unreachable instruction — usually emitted for
   `panic!` with `panic = "abort"` or for `unreachable!()`. Search for
   `unreachable!()` and `panic!("` in `crates/holon/src/sync/` and
   `crates/holon/src/api/` matview/preload code paths.

3. Add explicit `tracing::info!` markers in
   `crates/holon/src/sync/matview_manager.rs` around `ensure_view` and
   in any post-DDL hook (look for `MatviewHook` callsites). The last
   captured log line is `Marked 1 resources as available`, so the next
   `tracing::info!` you place will tell you exactly which function
   never returned.

4. Once you have a Rust panic message, fix it and re-test.

## Strategy B — Localize via `watch_view_d77ac41ba85c1706`

The hash `d77ac41ba85c1706` is `hash(sql)` of whatever query was
preloaded. Search the holon source for what gets preloaded during
`create_backend_engine` boot:

```
grep -rn "preload_views\|preload_startup_views\|preload(" crates/holon/src
```

Likely it's the root layout query or a default startup view. Trace
which one and reason about its post-creation hook.

## Strategy C — Bypass entirely

If the matview hook turns out to be deeply wasm-unfriendly, the
nuclear option is to skip view preload on wasm. `preload_startup_views`
and friends already have configuration; gate the call from the
dioxus-web crate's bootstrap to skip them:

```rust
// in frontends/dioxus-web/src/main.rs
create_backend_engine(":memory:".into(), |_inj| {
    // Intentionally skip preload_views on wasm — see Phase 2 handoff.
    Ok(())
}).await
```

That's already what the bootstrap does. The preload may be happening
inside `create_backend_engine` itself via the DI graph, not via the
setup_fn. Check `crates/holon/src/di/lifecycle.rs:create_backend_engine`
and `crates/holon/src/di/registration.rs` for any
`Provider::root_async` that auto-preloads.

# After the unreachable is fixed

Phase 2 still has:

1. **Seed content.** Once boot completes cleanly, insert a few blocks
   via direct SQL on first launch so there is something to render.
   Cheapest: a single document block + 2-3 child blocks. The schema
   constraint to remember: `created_at` and `updated_at` MUST be
   `Value::Integer(millis)`, NOT TEXT — see `crates/holon/CLAUDE.md`
   "Blocks Table Schema Mismatch".

2. **Wire `watch_ui`.** The dioxus-web `App` component currently shows
   a "Booting…" placeholder and then a "Backend ready" placeholder
   once the backend resolves. Replace the ready branch with a call to
   `holon::api::ui_watcher::watch_ui(engine, EntityUri::block(ROOT_LAYOUT_BLOCK_ID))`
   and feed the resulting `WatchHandle` events into a `Signal<UiEvent>`.
   Reference: `frontends/dioxus/src/main.rs` does this exact bridging
   for the desktop crate (read its `App` component).

3. **Render at least one widget.** Port the minimum builder set from
   `frontends/dioxus/src/render/builders/` — `text`, `row`, `col`,
   `section`, `spacer`, `block_ref` is enough to render a static tree.
   Do NOT pull in `holon-frontend` (still wasm-incompatible). The
   render context can be a thin `RenderContext` defined in
   `frontends/dioxus-web/src/render/context.rs`.

4. **One interactive op.** After something renders, hook a click
   handler on a single block to call
   `engine.execute_operation(...)` (e.g. toggle task state). This
   proves the CDC loop is alive end to end.

# Reality checks before claiming done

1. After every milestone, load the page via Playwright MCP
   (`mcp__playwright__browser_navigate` + `browser_console_messages`)
   and verify zero `RuntimeError: unreachable` and zero `panicked at`
   in the console. The previous session learned the hard way that
   `cargo check` does not catch wasm runtime panics.

2. Do not claim "renders" from a `<p>` tag. The screen must show
   actual content sourced from the in-memory Turso DB.

3. Before merging, run `cargo check -p holon` natively. The Phase 1
   commit and the Phase 2 boot commit both kept native green; do not
   regress that.

# Known follow-ups OUTSIDE Phase 2 scope

Capture these in a TODO but don't try to do them this session:

- ~50 other `tokio::spawn` sites in `crates/holon/src/` will panic
  the moment their code paths run. The migrated set is just `TursoBackend::new`,
  `TursoBackend::row_changes`, and `MatviewManager::spawn_demux`.
  A Python script to swap them all is the right move once the demo
  is ergonomically usable.

- Several `std::time::SystemTime::now()` sites still in `turso_core`
  itself (`functions/datetime.rs:set_to_current`,
  `translate/plan.rs:2898/2931/3155` ChaCha8Rng seeds, `vdbe/sorter.rs`
  + `vdbe/rowset.rs` `get_seed`). Will fire when SQL evaluates
  `datetime('now')` or the planner picks a hash seed for a non-trivial
  query. Push an upstream turso follow-up similar to the `cdd46b1c`
  `clock.rs` patch — extend `web-time` import to those modules.

- 87 MB unstripped wasm. `wasm-opt` crashes on the DWARF emitter, so
  `dx` falls back to the un-opted file. Phase 3 work: try
  `dx build --release` with `[profile.release] strip = true`,
  `panic = "abort"`, `lto = "fat"`, `codegen-units = 1`,
  `opt-level = "z"` at the workspace root, then run `wasm-opt`
  manually. Target is 5 MB gzipped. This is bonus, not a Phase 2
  blocker.

- `/Users/martin/Workspaces/rust/fluxdi` has the rt-multi-thread fix.
  It needs `jj describe` + branch + push when ready. That's a separate
  repo; do not forget it when shipping.

# Files most likely to need touching this session

Sorted by likelihood:

1. `crates/holon/src/sync/matview_manager.rs` — `ensure_view`, hook
   wiring, `MatviewHook` callbacks. Likely panic site.
2. `crates/holon/src/api/backend_engine.rs` — `BackendEngine::new`
   tail end (matview manager init), `preload_views`.
3. `crates/holon/src/di/lifecycle.rs` — `create_backend_engine_with_extras`
   resolves `BackendEngine` then `extras`, then logs
   `Bootstrap completed`. We never see that log → the unreachable is
   inside one of those resolutions or in their post-resolve hooks.
4. `crates/holon/src/di/registration.rs` — `register_core_services_with_backend`,
   in particular any `Provider::root_async` that does post-init work.
5. `frontends/dioxus-web/src/main.rs` — once you have a fix, the
   `App` component is where you'll wire `watch_ui` and the renderer.

# Definition of done for this session

Minimum bar:

1. The `RuntimeError: unreachable` is gone. Console clean during boot.
2. The placeholder "Backend ready" message is reached and visible.

Stretch:

3. At least one block (any block) renders from the in-memory DB via
   `watch_ui`.
4. Clicking a block dispatches an operation and the UI updates.

Anything reaching #3 unblocks a real demo and should be `jj
describe`d immediately.
