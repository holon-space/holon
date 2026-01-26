# live_query streaming: per-row diffs replace full re-interpretation

Date: 2026-06-11. Phase 3 item from the memory-reduction plan
(`~/.claude/plans/shiny-watching-newt.md`). Landed via concurrent-session
absorb into main; this devlog is the durable record of the design.

## What changed

- `ReactiveEngine::watch_query_live(query, lang, render_expr, query_context,
  services) -> (EntityUri, LiveBlock)` mirrors `watch_live`: interprets the
  tree ONCE with `RenderContext.data_source = Some(results)`, so collections
  inside the tree get Streaming wiring (per-row `VecDiff`s through their own
  `ReactiveView` pipelines). `structural_changes` fires only on
  render-expression or ui-generation changes — data-only CDC changes no
  longer re-run `interpret_fn` for the whole query tree.
- The gpui `live_query` builder creates a `ReactiveShell` (block mode) fed
  by that `LiveBlock`, cached under `CacheKey::LiveQuery` as before. The
  shell's `block_id` is the engine's `query:<hash>` watcher key, so
  `ReactiveShell::drop` → `services.unwatch(key)` now releases query
  watchers — previously they were NEVER unwatched (leak).
- `ensure_query_watching` now bumps the watcher refcount on reuse
  (mirroring `ensure_watching`) and returns the watcher key.
- DELETED: `views/live_query_view.rs` (`LiveQueryView`) and
  `watch_query_signal` (trait + engine impl). gpui was the sole consumer;
  TUI renders live_query slots statically and shadow builders don't watch.

## Deliberately deferred

Root layout pump (`spawn_root_layout_signal`, lib.rs) still uses
`watch_signal` (full re-interpret per fire): its per-fire side effects
(`reconcile_root_live_blocks`, `nav.set_root`, `resolved_view_model`) are
load-bearing, root-layout data changes are rare, and the risk/benefit is
poor. Revisit only if profiling shows root fires matter.

## Gates (all run on this change)

313 unit tests; `general_e2e_pbt` 2/2 (sql_only 52.8s, Full 55.5s);
gherkin replay 8 steps; layout_insta 4, layout_scroll 9,
panel_scroll_spike 5; `gpui_capture_replay` shows the SAME pre-existing
"fm4"→"4" TursoProjection signature as bare main (not worsened). Live
smoke: Journals page renders via the new path; the two matview ERRORs
visible there reproduce identically on a pre-change binary (pre-existing
Turso chained-matview issue, not this change).
