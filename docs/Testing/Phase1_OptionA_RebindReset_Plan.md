# Phase 1 Option A — in-process per-case reset via `RebindHandle` + a `reset_vault` MCP tool

Goal: give the `McpUserDriver` rung (and any MCP client) a **cheap, in-process
per-case reset** of the running Holon GPUI app — keep the GPUI window, swap the
engine + session inside it — instead of the heavier `simctl terminate`/relaunch
(the already-landed Option B' script, kept as a cold-boot fallback:
`crates/holon-integration-tests/scripts/ios_reset_sut.sh`).

This reuses the **existing** rebind machinery:
`launch_holon_window_rebindable` → `RebindHandle::rebind(session, engine, cx)`
(`frontends/gpui/src/lib.rs:973,998`).

> **[Fable] Correction:** `RebindHandle::rebind` currently has **NO in-tree
> caller** — the windowed PBT rung uses `launch_holon_window_rebindable`
> (`frontends/gpui/tests/pbt_harness/windowed_wide.rs:132`) but never rebinds
> mid-run; the rebind-exercising `windowed_replay` minimizer was deleted
> (see the "moved here from the deleted `windowed_replay` rebind service"
> comment at `windowed_wide.rs:234`). The mechanism is real and designed for
> this (the window-side driver deliberately reads the *live* engine per
> command — `lib.rs:1562,1580` — exactly so a rebind re-points it), but it is
> **unproven in the current tree**. Sequencing step 0 below adds a cheap
> desktop `TestApp` smoke that rebinds twice before any iOS work.

## Decision: build a NEW engine per reset (rebind-to-self REJECTED) **[Fable]**

The "even cheaper reset" (wipe + reseed the LIVE engine, no new engine) was
investigated and is **not viable**:

- `full_sync` (`crates/holon/src/api/operation_dispatcher.rs:318-401`) clears
  sync **tokens**, provider **caches**, and stale `watch_view_*` matviews
  (`backend_engine.rs:125` `drop_stale_matviews`), then re-runs provider
  `sync`. It is a *refresh* path for FDW/external-sync providers. It does NOT
  delete `block_raw` rows, does NOT reset the Loro doc store (mobile runs with
  `crdt.enabled = Some(true)`, `mobile.rs:35`), does NOT reset undo stacks,
  focus/nav state, or the org file-sync controller, and does NOT re-ingest an
  org root.
- No "reload vault" path exists anywhere (searched `holon`, `holon-frontend`,
  `holon-orgmode`). Building one would mean hand-rolling: Loro store wipe +
  consolidator epoch reset + SQL DELETE-all + matview drop + entity-cache
  clear + org rewrite + forced re-ingest + session nav/undo reset — each a new
  bespoke seam that can silently drift from the production boot composition.
- Building a new engine is already cheap: the headless harness boots one per
  PBT case via the same seam (see C below); DI bootstrap logs completion in
  ms (`crates/holon/src/di/lifecycle.rs:214-217`).

**Verdict: construct a fresh engine+session per reset. Production-faithful
(it IS the production boot path), deterministic by construction, and isolates
the old engine's background tasks (see F).**

## What `rebind` already does (confirmed)

`RebindHandle::rebind(session, engine, cx: &mut App)` (lib.rs:973):
- writes the new engine into `LiveEngine` (the cell the interaction pump reads),
- clears the `EntityCache`,
- `app_model.update(cx, |m| m.rebind(session, engine, vp, cx))` (drops stale panel
  shells, re-seeds viewport),
- re-points the root-layout signal pump at the new engine (`spawn_root_layout_signal`).

It must run on the **GPUI main thread** (holds `cx: &mut App`).

## Current boot (what changes)

`frontends/gpui/src/mobile.rs::open_holon_window` (mobile.rs:14):
1. `rt.block_on` bootstraps a `GpuiModule` via `fluxdi::Application` — **this also
   starts the embedded MCP server in `GpuiModule::on_start`**,
2. resolves `session`, `engine`, `debug (DebugServices)` from the injector,
3. keeps the DI `app` + tokio runtime alive on a detached thread,
4. `launch_holon_window_with_engine(session, engine, debug, rt_handle, cx)`.

## Design

### A. Make the mobile launch rebindable
- Swap step 4 for `launch_holon_window_rebindable(...)` → obtain a `RebindHandle`.
- The `RebindHandle` holds GPUI-main-thread types (`AnyWindowHandle`,
  `Entity<AppModel>`, `EntityCache`, `LiveEngine`) — **it is NOT `Send`**, so it
  can NOT be stashed on `DebugServices` (which is `Send + Sync`). It must stay on
  the main thread.

### B. A main-thread reset pump owning the `RebindHandle`
- After launch, `cx.spawn` a main-thread task that OWNS the `RebindHandle` and
  awaits `ResetRequest { session, engine, ack }` from a channel; on receipt it does
  `cx.update(|cx| handle.rebind(session, engine, cx))` then signals `ack`.
- Mirrors the existing interaction-pump pattern (`setup_interaction_pump`,
  lib.rs:~1548) which already awaits `InteractionCommand`s on a `futures::mpsc` and
  drives the window via `cx.update_window`.
- The channel `Sender<ResetRequest>` goes on `DebugServices` as a new
  `OnceLock<...>` slot (next to `interaction_tx`, server.rs:119). `ResetRequest`
  carries `Arc<FrontendSession>` + `Arc<ReactiveEngine>` (both `Send + Sync` — the
  interaction pump already moves `Arc<ReactiveEngine>` across threads) + a
  `oneshot::Sender<Result<()>>` ack.
- **[Fable] Send-pitfall audit: clean.** `FrontendSession`/`ReactiveEngine`
  Arcs already cross the tokio↔GPUI boundary in `open_holon_window` (resolved
  under `rt.block_on`, used on the main thread). The pump body only needs
  `AsyncApp::update`, same as the interaction pump. One real pitfall: do NOT
  build the fresh engine on the GPUI main thread (blocking DI boot there
  freezes the UI and `block_on` inside the pump can deadlock) — the tool side
  (tokio) builds it, the pump only rebinds.

### C. Build a fresh seeded engine+session WITHOUT restarting MCP — **RESOLVED [Fable]**
- Hard constraint confirmed: re-bootstrapping `GpuiModule` would start a 2nd MCP
  server — the MCP server registration lives in `GpuiModule::configure`
  (`injector.add_mcp_server(mcp_port)`, `frontends/gpui/src/di.rs:70`) and the
  start in `GpuiModule::on_start` (`mcp.start()`, di.rs:90).
- **The seam is `holon_app::new_from_config_with_di`**
  (`crates/holon-app/src/session.rs:47`) — the exact builder the headless PBT
  harness uses per case (`HeadlessFrontendComponent::new_with_loro`,
  `crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:127-197`).
  It runs the full production DI composition (`create_backend_engine_with_extras`
  → `add_frontend`, `lifecycle.rs:186`) and **never touches MCP** — MCP is a
  `GpuiModule`-only addition. No module variant or skip-flag needed.
- Signature:
  `new_from_config_with_di(holon_config, session_config, config_dir, locked_keys, extra_setup, extra_resolve) -> Result<(Arc<FrontendSession>, Arc<BackendEngine>, T)>`.
  In `extra_setup`, replicate what `GpuiModule::configure` does minus MCP:
  resolve `BuilderServicesSlot`, `injector.set_render_interpreter(crate::make_interpret_fn(slot.0.clone()))`
  (di.rs:62-63). In `extra_resolve`, resolve + return the `ReactiveEngine` and
  fill the slot with `engine.clone()` as `BuilderServices` (mirror di.rs:83-87
  / components.rs:174-184).
- **The builder function must live in the gpui crate** (it needs the
  gpui-specific `make_interpret_fn`). Add
  `holon_gpui::build_fresh_sut(seed: SeedSpec) -> Result<(Arc<FrontendSession>, Arc<ReactiveEngine>, RetiredHold)>`
  (async, tokio). `open_holon_window` installs it on `DebugServices` as a new
  `OnceLock<Arc<dyn Fn(SeedSpec) -> BoxFuture<...> + Send + Sync>>` slot, so
  the MCP tool crate stays decoupled from gpui.
- Config must mirror mobile boot: `crdt.enabled = Some(true)` (mobile.rs:35),
  fresh `db_path` + fresh vault root + **fresh `config_dir`** (temp dirs — see
  F for why fresh paths are mandatory, not optional).
- **[Fable] Loro editor-cell check:** the headless harness additionally wires
  `ReactiveEngine.block_cell_registry` after build (components.rs:205-222)
  because the windowless path bypasses frontend `on_start`. Verify post-reset
  typing works (the smoke types a char); if `MutableText` resolution fails,
  port that registry wiring into `build_fresh_sut`.
- Run `build_fresh_sut` on the **same tokio runtime** the window's `rt_handle`
  points at (the MCP tool already executes there), so the fresh engine's
  background tasks land on a live runtime.

### C2. MCP server staleness after rebind — **[Fable] NEW, plan-breaking if skipped**
- `HolonMcpServer` captures `engine: Option<Arc<BackendEngine>>` and
  `builder_services` **by value at session creation**
  (`frontends/mcp/src/server.rs:174-197`; factory closure in
  `di.rs:291-298` clones Arcs captured at `run_http_server` start). After a
  rebind, `execute_raw_sql`/`execute_operation`/`describe_ui`/… would still hit
  the **OLD** engine — the reset would "succeed" while every subsequent MCP read
  observes stale state. Worse: the MCP client keeps ONE streamable-http session
  across the reset, so even a per-session factory fix is insufficient.
- Fix (mechanical): introduce a shared live cell, e.g.
  `type LiveMcpBackend = Arc<std::sync::RwLock<(Option<Arc<BackendEngine>>, Option<Arc<dyn BuilderServices>>)>>`,
  held by `McpServerHandle`, passed through `run_http_server` into the session
  factory, and stored on `HolonMcpServer` in place of the two fields; tools read
  it through accessors **per call**. Mirrors the window's `LiveEngine` cell
  (lib.rs:947-949). The reset tool swaps it right before sending `ResetRequest`.
- Also stale but Phase-2-deferrable (used only by `inspect_loro_blocks` /
  `read_org_file` / `render_org_from_blocks`): the `DebugServices` OnceLock
  slots `loro_doc_store`, `orgmode_root`, `org_fs` populated once by
  `DebugServicesPopulatorModule` (`frontends/mcp/src/di.rs:73-79`). Either
  convert to `RwLock<Option<...>>` and swap in the reset, or document them as
  known-stale-after-reset in the tool docs — do NOT leave them silently wrong.
- NOT stale (rebind-aware by design): `interaction_tx`, `input_router`,
  `navigation_state`, and the window-installed `user_driver` — the interaction
  pump and driver read `LiveEngine` per command (lib.rs:1562,1580).

### D. Seed / DB freshness — **RESOLVED [Fable]**
- Deterministic wide seed = the three files in
  `crates/holon-integration-tests/scripts/seed_wide/` (`index.org` app shell
  with pinned `#+ID`, date-free `Journals.org`, `structural-page.org` ==
  `WIDE_TREE_ORG` at `wide_e2e.rs:150`).
- **Single source of truth: keep the seed generic on the server, pass content
  from the client.** `reset_vault` takes
  `{ files: [{name, content}] }` — the MCP tool wipes nothing it "knows about",
  it just materializes the given files into a **fresh** org root and boots on a
  **fresh** db path. Then:
  - the shell script keeps copying `scripts/seed_wide/` (already does),
  - the Rust `McpUserDriver` smoke passes
    `include_str!("../../scripts/seed_wide/…")` of the same three files,
  - the server embeds NO seed copy at all — zero drift surface, and the tool is
    reusable for any future seed.
- Close the remaining duplication inside the repo: redefine
  `WIDE_TREE_ORG` as
  `include_str!("../../../scripts/seed_wide/structural-page.org")`
  (`include_str!` is const-compatible) so oracle and script literally share one
  file. Add a `#[test]` asserting `Journals.org` stays date-free.
- **[Fable] Do NOT "wipe + rewrite the vault org root" in place** (the previous
  draft's wording): the OLD engine's file-sync controller still watches the old
  root and *writes back* (`:ID:`-drawer persistence) — in-place reuse lets the
  retired engine corrupt the new seed. Always rotate to fresh temp paths
  (org root, db, config_dir) per reset; this is what makes F's leak inert.

### E. The `reset_vault` MCP tool (`frontends/mcp/src/tools.rs`)
- New tool, tokio side:
  1. materialize `files` into fresh temp org-root; pick fresh db path + config dir,
  2. call the `DebugServices` `build_fresh_sut` slot (C) → `(session, engine)`,
  3. swap the `LiveMcpBackend` cell (C2),
  4. send `ResetRequest` on the reset channel (B), await the `ack`,
  5. push the retired SUT onto the retirement list (F), return the new
     `block_raw` id-set count in the tool result (fail-loud self-check).
- **Gating:** register/enable ONLY in non-prod builds — `#[cfg(debug_assertions)]`
  and/or behind an env flag (e.g. `HOLON_MCP_ALLOW_RESET`), so a shipped release
  can never wipe a user's vault over MCP. Fail loud if called when disabled.
  **[Fable]** Note the tool as specced never deletes the USER vault at all (it
  only creates fresh temp roots and abandons the old one) — the gate is still
  mandatory because it swaps the app away from the user's data.

### F. Lifetime of the OLD engine/session/DI app after rebind — **RESOLVED [Fable]**
- **Facts:** there is NO engine-wide teardown API. `ReactiveEngine` /
  `BackendEngine` / `FrontendSession` expose no `shutdown()`; background tasks
  are a mix of abort-on-drop (`AbortHandle` wrappers:
  `holon-frontend/src/reactive_view_model.rs:369-380`,
  `holon-orgmode/src/di.rs:657,685`) and plain detached `tokio::spawn`s
  (consolidator, CDC pumps, file-sync). Dropping the Arcs would stop *some*
  tasks and orphan others mid-await — worst case a detached task drops a
  runtime-bound resource in a weird context. The windowed harness's precedent
  is explicit: it `std::mem::forget`s the SUT on teardown precisely to avoid
  Drop hazards (`windowed_wide.rs:219-229`).
- **Recommendation: leak deliberately, isolate completely, own explicitly.**
  1. Every reset uses fresh temp `db_path` + org root + `config_dir` (D), so
     the retired engine's watchers/consolidator idle against dead paths — the
     leak is *inert*, not interfering. This is the load-bearing invariant.
  2. Ownership: a `static RETIRED: Mutex<Vec<RetiredSut>>` (tool side, all
     members are `Send`) holding `(Arc<FrontendSession>, Arc<ReactiveEngine>, Arc<BackendEngine>, TempDir(s))`.
     Explicit retirement beats accidental leak: growth is observable, and a
     `tracing::warn!` + hard cap (e.g. refuse reset #20 with a clear error →
     fall back to the Option B' cold relaunch) keeps it fail-loud. Do NOT keep
     the DI `Injector` in `RetiredSut` unless needed — the Arcs above are what
     background tasks reference.
  3. The ORIGINAL boot's DI `app` on the detached thread (mobile.rs:65-68)
     stays alive for the process lifetime **by design** — it owns the MCP
     server + `DebugServices`, which must survive every reset. It is not part
     of the retirement story.
- Cost estimate to verify in step 4: one Turso DB + Loro store + task set per
  retired engine; on-sim memory growth per reset must be measured across a
  ~10-reset loop before declaring done. If it's too heavy for long PBT runs,
  the answer is "cold relaunch every K cases", not a teardown API.

## Verification — DONE (live on iPhone 17 Pro sim, 2026-07-07)

Driver: `crates/holon-integration-tests/scripts/verify_reset_vault.py` — ONE
streamable-http session, `reset_vault(wide seed)` ×2, each followed by a
same-session `execute_raw_sql SELECT id FROM block_raw ORDER BY id`. Run twice
(4 resets total). Results:
- **Deterministic id-set**: identical 15-row `block_raw` id-set on every reset
  (pinned `:ID:`s from the wide seed — `root-layout`, `parent/c1/c2`,
  `structural-page`, `journals`, the sidebars, `15223f86…`).
- **C2 (no staleness)**: the same MCP session's post-reset read returned the new
  engine's 15 ids each time — the swapped `LiveMcpBackend` cell is observed by a
  session opened before the reset.
- **Retirement grows 1/reset**: `retired_engines` = 1,2,3,4 across the 4 resets
  (process-wide `RETIRED` static; persists across MCP sessions).
- **No row growth / no cross-talk**: count stayed 15 (not 30) — retired engines
  don't leak rows into the live one.
- **Single server**: `/health` stayed 200 and `/mcp` 406 throughout; reset #2 on
  the same session proves no second server / no port conflict.
- Gate confirmed: launched with `SIMCTL_CHILD_HOLON_MCP_ALLOW_RESET=1`; the tool
  is `#[cfg(debug_assertions)]` + env-gated.

Remaining (follow-ups, not blockers): (a) type-one-char-commits — `build_fresh_sut`
wires the Loro `BlockCellRegistry` and boot raised no "no MutableText", but a full
type→commit assertion is entangled with the deferred B1 iOS Focus/Blur track;
(b) ~10-reset memory-growth soak (retirement proven correct to 4; a longer leak
profile is deferred).

### Original verification checklist
- The `McpUserDriver` smoke calls `reset_vault` (passing the `include_str!`'d
  seed) then `execute_raw_sql SELECT id FROM block_raw ORDER BY id` and asserts
  the deterministic wide-seed id-set (same probe as the script's step 6).
- Repeat the reset ≥2× in one process and assert:
  - the id-set is identical each time,
  - the SAME MCP session keeps working across the reset (C2 regression check),
  - no port conflict / second server (health endpoint stays singular),
  - `RETIRED.len()` grows by exactly 1 per reset and block_raw row count does
    NOT grow (no cross-talk from retired engines),
  - typing one char into a focused block commits (Loro cell registry check, C).

## Status (implementation)

- **Step 0 (desktop rebind smoke) — DONE, GREEN.**
  `frontends/gpui/tests/gpui_rebind_reset_smoke.rs`. Boots SUT#1 via
  `build_fresh_sut` (default `index.org`/`Journals.org` + an `alphapage.org`
  Page), opens a rebindable window, settles, asserts the sidebar renders
  `alphapage`; then builds SUT#2 (`bravopage.org`), calls `rebind`, and asserts
  the sidebar now renders `bravopage` and `alphapage` is GONE. Empirical finding:
  a Page's sidebar `content` == its source **filename stem**, and content blocks
  get fresh UUIDs per boot — so distinct seed filenames are the deterministic,
  readable rebind sentinel (headline text is NOT shown without main-panel focus).
- **Step 1 (`build_fresh_sut`) — DONE.** `frontends/gpui/src/reset.rs`;
  `cargo check -p holon-gpui` clean. All four unverified appendix paths confirmed
  correct as written (`holon::api::BackendEngine`,
  `holon_frontend::config::VaultConfig`, the two cell-registry paths, and
  `sleep`-settle). The Loro `BlockCellRegistry` wiring compiles and the SUT boots
  + renders in the smoke; deeper typing-commit proof deferred to step 4.

- **Step 2 (`LiveMcpBackend` cell) — DONE.** `frontends/mcp/src/server.rs`:
  `HolonMcpServer`'s `engine`/`service`/`builder_services` fields are replaced by
  one swappable `backend: LiveMcpBackend` (`Arc<std::sync::RwLock<McpBackendCell>>`);
  `engine()`/`service()`/`builder_services()` accessors read it per call.
  `run_http_server` (di.rs) builds ONE shared cell and hands every session's
  server a clone via `with_backend_cell`, so a swap through any `self.backend`
  is visible everywhere. `cargo check -p holon-mcp` clean.
- **Step 3 (reset pump + rebindable mobile) — DONE (host-compiled; iOS-target
  check pending).** `frontends/gpui/src/mobile.rs::open_holon_window` now opens
  via `launch_holon_window_rebindable`, installs a `DebugServices::reset_builder`
  slot (→ `reset::build_fresh_sut_from_files`), and `cx.spawn`s a main-thread
  pump owning the `!Send` `RebindHandle` that awaits `ResetRequest` on a new
  `DebugServices::reset_tx` channel and rebinds. New mcp types: `ResetRequest`,
  `ResetBuildOutput`, `ResetBuilderFn`.
- **Step 4 (`reset_vault` tool) — DONE (compiles; live on-sim verification
  pending).** `frontends/mcp/src/tools.rs::reset_vault`, gated
  `#[cfg(debug_assertions)]` + `HOLON_MCP_ALLOW_RESET`. Flow: build fresh SUT →
  swap `LiveMcpBackend` cell → send `ResetRequest`, await ack → push onto
  `RETIRED` (cap 20, else fail-loud → fall back to `ios_reset_sut.sh`) →
  self-check `SELECT id FROM block_raw` returned in the tool result.
  Client supplies the seed (`{files:[{name,content}]}`); server embeds none.

## Sequencing / de-risking **[Fable — reordered]**
0. **Desktop rebind smoke first** (new): a host-side `TestApp` test that opens
   via `launch_holon_window_rebindable`, boots a second SUT with
   `new_from_config_with_di`, calls `handle.rebind(...)`, and asserts the
   window renders the new seed. `rebind` has no in-tree caller today — prove
   it green on the cheap platform before anything iOS.
1. Prove `build_fresh_sut` (C) in isolation on-sim: build, assert id-set via a
   direct engine query, retire it. (No window interaction yet.)
2. C2: the `LiveMcpBackend` cell + per-call accessors; assert a live MCP
   session sees the swapped engine.
3. The main-thread reset pump + rebind on-sim (A/B), driven by a hard-coded seed.
4. The MCP tool + gating (E), client-supplied seed (D); run the verification
   list including the ≥10-reset leak/memory loop (F).

## Appendix — concrete `build_fresh_sut` recipe (step 1, copy-ready)

Verified seams (this session): mirrors `HeadlessFrontendComponent::new_with_loro`
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:172-221`)
with two deltas for the real app: (a) REAL filesystem — no
`override_org_fs_bindings`/`InMemoryFileSystem`, the caller materializes seed
files onto `org_root` on disk; (b) it lives in the gpui crate for
`render_supported_widgets()` (`lib.rs:1524`) and colocation with `rebind`.

Correction to the plan body: `crate::make_interpret_fn` is a `pub use` of
`holon_frontend::reactive::make_interpret_fn` (`lib.rs:41`) — the SAME fn the
headless harness uses; it is NOT gpui-specific. `build_fresh_sut` still belongs
in gpui, but for `render_supported_widgets` + rebind colocation, not the interpreter.

Caller supplies FRESH paths (plan D): a fresh, empty `org_root` with the seed
files already written into it, a fresh `db_path`, and a fresh `config_dir`
(rotate per reset; mobile/tool own the temp-dir lifetime → the retirement hold, F).

```rust
// frontends/gpui/src/lib.rs (or a new `reset.rs` module in the gpui crate)
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// A freshly-booted, wide-seeded SUT for an in-process rebind reset. Holds the
/// BackendEngine so the retirement list (plan F) can keep it (and its temp dirs)
/// alive/inert after the window rebinds away from it.
pub struct FreshSut {
    pub session: Arc<FrontendSession>,
    pub engine: Arc<ReactiveEngine>,
    pub backend: Arc<holon::api::backend_engine::BackendEngine>, // confirm re-export path
}

/// Build a production-faithful FrontendSession + ReactiveEngine on the given
/// fresh, seeded paths, WITHOUT starting an MCP server (that is GpuiModule-only).
/// `org_root` must already contain the seed .org files (caller writes them).
pub async fn build_fresh_sut(
    db_path: PathBuf,
    org_root: PathBuf,
    config_dir: PathBuf,
    settle: Duration,
) -> anyhow::Result<FreshSut> {
    use holon_frontend::config::{HolonConfig, SessionConfig, VaultConfig};
    use holon_frontend::reactive::{
        BuilderServices, BuilderServicesSlot, ReactiveEngine, RenderInterpreterInjectorExt,
    };

    let mut holon_config = HolonConfig {
        db_path: Some(db_path),
        vault: VaultConfig { root: Some(org_root) },
        ..Default::default()
    };
    holon_config.crdt.enabled = Some(true); // mirror mobile.rs:35

    let ui_info = holon_api::UiInfo {
        available_widgets: crate::render_supported_widgets(),
        screen_size: None,
    };
    let session_config = SessionConfig::new(ui_info);

    let injector_slot: Arc<std::sync::OnceLock<fluxdi::Injector>> =
        Arc::new(std::sync::OnceLock::new());
    let injector_slot_c = injector_slot.clone();

    let (session, backend, reactive) = holon_app::new_from_config_with_di(
        holon_config,
        session_config,
        config_dir,
        std::collections::HashSet::new(),
        move |injector| {
            // GpuiModule::configure minus MCP (di.rs:62-63). NO register_debug_services,
            // NO add_mcp_server — the existing MCP server is reused (C2).
            let slot = injector.resolve::<BuilderServicesSlot>();
            injector.set_render_interpreter(crate::make_interpret_fn(slot.0.clone()));
            Ok(())
        },
        move |injector| {
            // GpuiModule::on_start minus MCP (di.rs:83-87).
            let engine = injector.resolve::<ReactiveEngine>();
            let slot = injector.resolve::<BuilderServicesSlot>();
            let services: Arc<dyn BuilderServices> = engine.clone();
            slot.0.set(services).ok();
            injector_slot_c.set(injector.clone()).ok();
            engine
        },
    )
    .await?;

    // Loro editor-cell registry (crdt on) — else typing errs "no MutableText".
    // components.rs:208-221.
    {
        let injector = injector_slot.get().expect("injector captured in extra_resolve");
        let registry: Arc<holon::sync::block_cell_registry::BlockCellRegistry> = injector
            .resolve_async::<holon::sync::block_cell_registry::BlockCellRegistry>()
            .await;
        let registry_dyn: Arc<dyn holon_frontend::cell::EntityCellRegistry> = registry;
        reactive.block_cell_registry.lock().unwrap().replace(registry_dyn);
    }

    // Boot settle: converge the same signals as the headless boot (components.rs:223+);
    // simplest first cut = tokio::time::sleep(settle) then rely on the window's own
    // settle-to-fixed-point after rebind. Prefer converge_signals if reachable here.
    if settle > Duration::ZERO {
        tokio::time::sleep(settle).await;
    }

    Ok(FreshSut { session, engine: reactive, backend })
}
```

Unverified-at-write-time (next agent confirms via `cargo check -p holon-gpui`):
- `holon::api::backend_engine::BackendEngine` re-export path (components.rs infers it).
- `VaultConfig` field name / import path (`holon_frontend::config::VaultConfig`).
- `holon_frontend::cell::EntityCellRegistry` + `holon::sync::block_cell_registry::BlockCellRegistry` paths (from components.rs:212-215, current).
- Whether a proper converge (not `sleep`) is reachable/needed here vs. deferring to
  the window's post-rebind `settle_to_fixed_point`.
