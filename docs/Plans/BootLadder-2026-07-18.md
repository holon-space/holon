# Holon Boot Ladder + Recovery Surface — Architecture Plan

Status: increments approved 2026-07-18 (rulings below); increment 1 in progress

*Authored by Plan agent 2026-07-18, senior-reviewed.*

## First-principles goals

1. User data is never hostage to derived-state failure (Turso is ephemeral by
   contract; org + Loro are replicas).
2. Every failure is visible; degraded operation only when disclosed.
3. Repair is self-service on-device (Android incident: stale-DB reconcile panic
   needed `pm clear`).
4. There is always a rung that cannot fail (disclosure + repair need somewhere
   to render).

## Findings (verified 2026-07-18)

- **Desktop boot**: `frontends/gpui/src/main.rs` → `cli::build_session` →
  fluxdi `Application::new(GpuiModule).bootstrap()` (180s timeout) → `Err` =
  process exit before any window.
- **Mobile boot**: `frontends/gpui/src/mobile.rs` `android_main` / `ios_main` →
  `open_holon_window` → `bootstrap().await.expect(...)` — a PANIC (the
  incident). Mobile bypasses `holon.toml` entirely (inline config,
  `crdt.enabled` hardcoded `Some(true)`).
- **Panic inventory on the spine**: `open_and_register_core` expects
  (`crates/holon/src/di/lifecycle.rs` L79/L86); `preload_startup_views` `panic!`
  (L41); matview reconcile chain (`crates/holon-turso/src/schema_modules.rs`
  L358–L875); `FrontendSession` factory expects
  (`crates/holon-app/src/wiring.rs`: `seed_default_layout`,
  `start_action_watchers`, `watch_default`, `create_dir_all`); iOS path helpers.
- **Existing pluggability**: `SessionParts` capability model
  (`Option<QueryEngine>`, `Option<OperationEngine>`; accessors degrade visibly);
  test-only no-Turso wiring end-to-end (`build_no_turso_container`,
  `lifecycle.rs` L157 + `no_turso.rs`, ADR 0004 Phase 9); Loro/org/MCP
  conditionals real in `add_frontend`; `DegradedSignalBus` → `DegradedToast`
  banner substrate (post-boot only, transient); `reset_vault` MCP tool +
  `FreshSut` + `ResetRequest`→`RebindHandle` main-thread pump = in-process
  whole-app swap; `guard_consolidator_epoch` / `wipe_durable_state`
  per-component durable-state descriptors; `PreferenceDef`-driven config UI;
  `HOLON_LOG` file destination (desktop only). No crash-loop / safe-mode
  machinery.

## Target architecture — Option B chosen: boot supervisor ABOVE DI

(Option A ladder-in-DI rejected: fluxdi providers panic, half-initialized
containers, degradation smeared across factories = de-facto catch-all.)

`BootSupervisor` attempts complete wiring plans (rungs); each attempt = fresh DI
container; component failures caught at module boundary as typed
`BootError{component, stage, source}`; surviving container registers
`BootReport` for UI disclosure. Optional components (org sync, MCP integrations,
share) degrade in place within a rung; the ladder is only for load-bearing
storage and the terminal shell.

Rungs:

- **3 Full**
- **2 Degraded-in-place** (non-storage `BootError`; formalizes today)
- **1a No-Turso** (Turso `BootError`; productionize `build_no_turso_container`
  + Loro read stack + `register_block_query_frontend`; banner)
- **1b Loro failure** lands on rung 0 (NO auto-SqlOnly — invariant 10 forbids
  consolidator flip; epoch guard exists to refuse it)
- **0 Recovery shell** (infallible: `FrontendSession` over static in-memory
  `BlockQuerySource` `from_sync` + bundled recovery layout; UI + config editor +
  repair actions + log export; no disk reads to construct).

Boot stages: `config-load` → `epoch-guard` → `container-configure` →
`engine-resolve` (schema + matview + preload) → `session-resolve` → `post-ready`
(already degrades via `DegradedSignalBus`).

Crash-loop sentinel: boot-attempt marker in config dir at entry, cleared on
first paint; 2 consecutive un-cleared → rung 0 directly. Follows
`consolidator_epoch` `write_marker` precedent.

Disclosure: `BootReport { rung, components: Vec<(BootComponent,
Ok|Degraded(reason)|Excluded(BootError))> }` in DI, on `FrontendSession`;
persistent banner variant beside `DegradedToast`; tap → diagnostics page.

## Recovery surface

Entries: rung 0 auto; settings "Diagnostics & Repair"; MCP debug tools (mobile
runs embedded MCP server → remote agent repair).

Wiring: dedicated `RecoveryActions` service constructed from PATHS ONLY
(`db_path`, `loro_dir`, `config_dir`, log path) — NOT
`available_operations` / `OperationEngine` (must work when the op pipeline is
down). Each action carries `CostDisclosure{what_is_lost, duration_class}`
rendered before confirmation.

Actions:

- drop matviews + re-reconcile (cheap, no loss; `reconcile_named_view` /
  `derived_reconciler`)
- delete Turso DB + in-process reboot + reseed (MINUTES; no replica loss;
  progress via `ready_signal`; `TursoDurableState::wipe` + `ResetRequest` pump)
- reset Loro store (DESTROYS op history not in org files; strong warning;
  `LoroDurableState` wipe)
- reset config (writes `holon.toml.bak`)
- export logs (mobile ring-buffer `HOLON_LOG` destination + share / copy)

## Increments (risk-first, independently landable)

1. De-panic boot spine + `BootError` (**THIS INCREMENT**).
2. Crash-loop sentinel + rung-0 recovery shell (converts the Android incident
   from `pm clear` to self-service on its own).
3. `RecoveryActions` + cost disclosure (delete-Turso-DB + in-process reboot,
   drop-matviews, reset-config) + MCP tools.
4. `BootSupervisor` ladder (no-Turso plan productionized; `BootReport` in DI).
5. Persistent degraded banner from `BootReport`.
6. Mobile config parity (`holon.toml` on device; `PreferenceDef` UI).
7. Log ring buffer + export on mobile.
8. **(design first) Lazy priority-driven reprojection** — after a Turso wipe (or
   any reprojection), the app STAYS OPERATIONAL and reprojects on demand: pages
   the user opens are projected / cached immediately; everything else is indexed
   slowly in the background, throttled so there is no device heat / battery
   drain. This is a bigger architectural call than recovery alone (it may
   improve both UX and steady-state architecture) and needs its own options doc
   before implementation — not just a recovery action.

Out of scope: the specific matview-reconcile bug (parallel lane);
consolidator-epoch migration (ladder never auto-flips consolidator mode);
offline command log; P2P failures; other frontends beyond compiling; any
automatic silent repair.

## Risk register

- Failed-attempt partial state → `FreshSut` retirement (leak-but-inert); never
  reuse a failed container.
- fluxdi provider-cache poisoning → fresh injector per attempt.
- `catch_unwind` poisoned locks → rung 0 uses nothing from the caught state.
- No-Turso plan only test-proven → exercise `loro_seams` under real vault sizes.
- Sentinel false positives → clear-on-first-paint, threshold 2, rung 0 offers
  "boot normally anyway".
- Reseed progress → org-scan `ready_signal` suffices v1.

## Staleness probes (run at each increment start)

```
rg -n "expect\(|panic!" crates/holon/src/di/lifecycle.rs crates/holon-app/src/wiring.rs frontends/gpui/src/mobile.rs
ast-outline outline crates/holon-app/src/wiring.rs; ast-outline outline crates/holon-app/src/no_turso.rs
rg -n "reconcile_named_view|preload_startup_views" crates/holon-turso/src/schema_modules.rs crates/holon/src/di/lifecycle.rs
rg -n "DegradedSignalBus|ShareDegraded|DegradedToast" -g '!target' crates frontends
rg -n "reset_vault|ResetRequest|RebindHandle|FreshSut" frontends/gpui/src frontends/mcp/src
rg -n "guard_consolidator_epoch|wipe_durable_state|DurableState" crates/holon-app/src crates/holon-turso/src crates/holon-loro/src
```

## Rulings 2026-07-18 (Martin)

1. **Mobile last-resort `catch_unwind`** — SANCTIONED, but *only* at the
   outermost mobile shell, as a disclosed exception (an `ALLOW`-comment) that
   displays the panic verbatim in the recovery UI. Not a general catch-all.
   Implemented in increment 2 — this does NOT widen increment-1 scope
   (increment 1 stays Results-only, no `catch_unwind`).
2. **After delete-Turso-DB** — the app STAYS OPERATIONAL; reseed is
   LAZY / INCREMENTAL: pages the user opens are projected / cached immediately
   on demand, everything else is indexed slowly in the background, throttled to
   avoid device heat / battery drain. Captured as **Increment 8 (design first)**
   above; it is a bigger architectural call needing its own options doc, and may
   improve both UX and architecture beyond recovery.
3. **Loro-store failure** — recovery shell for now. PLUS a recorded follow-up
   rung idea, **rung 1b-org**: continue working *read-WRITE* directly from the
   org files while Loro is down; a later repaired Loro (e.g. files copied from
   another device) picks up the interim edits as external edits through the
   existing file-sync path. Needs design + PBT validation before implementation.
   Also: recovery behaviour is to be PBT-tested with new keystone transitions
   `CorruptLoro` / `CorruptTurso` — applied both mid-run and before `StartApp` —
   with RED guards landed BEFORE the fixes so the failure modes are visible
   immediately. A separate lane owns this.
4. **Desktop parity** — the SAME recovery window as mobile; the structured CLI
   error stays only as the fallback when running headless.
5. **Log export** — copy-into-`Documents` now; platform share sheet later.
