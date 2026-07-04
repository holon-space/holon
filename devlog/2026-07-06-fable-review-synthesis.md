# Fable Deep-Review Synthesis — 2026-07-06

Seven parallel Fable-5 agents reviewed Holon (crate coupling, Model.md invariants,
arch-docs consistency, PBT endgame, ADR audit, Petri-engine, adversarial redesign).
Full reports: `devlog/2026-07-06-fable-*.md`. This is the deduplicated, ranked action list.

## Cross-validated framing correction (3 agents independently)
The "Petri-net engine refactor" = the **WSJF task-ranking engine** (`holon-engine`/
`holon-petri`, MCP `rank_tasks`). It is NOT a sync/frontend replacement. The removed
`AppState`/`BlockWatchRegistry`/`ReactiveViewKind` were a *separate* frontend rewrite to
`ReactiveEngine` (futures-signals, alive in `holon-frontend/src/reactive.rs`).
`UI.md`/`RenderPipeline.md` remain accurate. No ADR or arch-doc names a removed type as live.

## P0 — prod-correctness / build-integrity
- **A0 fallback survives** (`holon-loro/src/loro_backend.rs:884,913`, `.unwrap_or_else(default_sort_key)`):
  non-total fractional-index projection silently faked — the exact historical bug shape.
  ADR 0005's "default_sort_key() removed" is FALSE. → kill the fallback, fail loud.
- **Rhai injection in holon-petri** (`lib.rs:959-1119`, `build_objective_expr:1121-1139`):
  user text `format!`-ed into guard source; a `"` in a name = injection / fire-time failure.
  Reachable from live MCP `rank_tasks`. → typed `AttrInit::Literal(Value)`.
- **MCP-path panics** (`holon-petri/src/block_domain.rs:505`): library `panic!` on bad org
  data reachable from `rank_tasks` = process abort not tool error. → `thiserror` (declared, unused).
- **workspace-hack drags GPUI+proptest into every native prod build**
  (`workspace-hack/Cargo.toml:48` pins gpui `test-support` as normal dep). Root cause of the
  "proptest in prod" audit item — per-crate gating is actually correct. → `[final-excludes]` gpui
  in `.config/hakari.toml` + cargo-tree regression test. **Biggest single win (~10 lines).**
- **Non-reproducible build**: root `Cargo.toml:265` `[patch]` → local `/Users/martin/...` path.

## P1 — architecture / coupling (cross-confirmed by coupling + redesign agents)
- **`holon` is a shadow composition root** (12 files name `holon_turso`/`holon_loro`; `holon/src/di/`
  below holon-app) — falsifies CrateMap's "holon-app owns every concrete wiring". archlint bans
  Loro/Turso naming only in orgmode/frontend, so `holon` is exempt by omission.
  → move `holon/src/di`→holon-app; add archlint Cargo.toml-edge rule.
- **Hexagon violation: `holon-mcp-client` → `holon_turso`** (`mcp_integration.rs:12,580`,
  imports `DbHandle` + `matview_manager::reconcile_named_view`) — adapter→adapter. → kernel port.
- **`frontends/mcp` is a library, not a frontend** (4 normal dependents incl. integration-tests
  upward). → split `crates/holon-mcp-server` + thin bin.
- **`holon-markdown` = dead crate**, zero dependents. → delete.
- **holon-api mixes data kernel + ~3.8k LOC UI render DSL** → storage adapters rebuild on widget
  changes. → extract render-spec crate. (Deferred: api+core→kernel merge, `holon`→`holon-sync` rename.)

## P1 — invariant enforcement gaps (Model.md agent)
- **inv 3 unenforced** (`board.rs:261` mints sort_key in the frontend, dispatches `set_field("sort_key")`;
  update-op vocab accepts+discards a `sort_key` key). → closed `BlockWriteField` enum at intent boundary.
- **inv 8 (commit points) missing in TUI** (`app_main.rs:1061,1088` split/join with no flush);
  composed PBT structurally can't catch it.
- **inv 9 (tombstones outlive bases) vacuous** — no GC / replica-base registry to enforce against.

## P2 — doc drift (mechanical, high-ROI)
- `Sync.md` PRQL examples say table `operations` — real table is `operation` (singular): copy-paste breaks.
- `Replication.md:486`/`Sync.md:13,180` call `LoroMetaCellBacking`/`LwwScalarBacking` unimplemented —
  both exist + wired (reader would rebuild existing code).
- `Principles.md:394` misclassifies `focused_block` as per-instance widget state — it's window-global
  (UI.md/ADR 0010) — dangerous for ADR 0015's `(id, occurrence)` re-keying.
- c4 diagrams + `baseline/crates/architecture.json` one refactor behind (miss holon-loro/petri/profiles)
  + CrateMap omits same three (no `@c4` annotation). → re-run `just arch-docs`.
- Root `Architecture.md` index still shows old reactive dataflow + nonexistent `ReactiveRenderedRows`.
- ADR status sweep: 0002 STALE→supersede; 0001 "Superseded By: None" wrong; 0004/0005/0006/0010 still
  "Proposed" though shipped. Missing ADRs: Petri engine, ReactiveEngine rewrite, correspondence registry,
  CapMap DI, Turso chained-matview bet.

## PBT endgame (verification agent)
- "Fully retired" CONFIRMED; keystone = 32-line shell over `ComposedSut<WideE2E>`. Residue cosmetic
  (~40 stale doc-comments, stray `models.rs.orig`).
- **Red (a) toggle-state category ALREADY FIXED** (verified by running the test, passes 2.8s) — strike it.
- Red (b) TUI drawer + red (c) ghost-row = harness bugs, not prod (`layout_bridge.rs:34` GPUI-blind;
  empty ref-universe oracle). Phase-4 wiring ≈ 5-7 days (tasks 4.1-4.6); 6-row alphabet = separate ~1wk.

## Rule-violation flag
- `frontends/waterui/src/lib.rs:73,138` still calls DELETED `cdc::spawn_ui_listener` / old
  `RenderContext::new` — hidden from CI by workspace `exclude`. Old+new coexist (CLAUDE.md violation).

## Suggested sequencing
1. **~1 day, do now**: A0 fallback, Rhai injection + MCP panics, hakari gpui exclude, local `[patch]` fix.
2. **~2.5 days, structural**: 5 coupling surgeries (di→app, mcp-client port, delete markdown, mcp→crates, archlint edge-rule).
3. **~half day, docs**: Sync.md table name, backing-status + focused_block bugs, `just arch-docs`, ADR status sweep.
4. **~1 sprint**: PBT Phase-4 wiring (4.1-4.6) + two harness triages.
5. **Backlog**: api+core→kernel merge, `holon`→`holon-sync` rename, missing ADRs, inv 8/9 enforcement, waterui.
