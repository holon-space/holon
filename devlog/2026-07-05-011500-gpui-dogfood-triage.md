---
date: "2026-07-05 01:15"
session: "67fed000-184a-47ac-bb0e-a658ab78b66d"
project: "holon"
---

## GPUI dogfooding triage — 5 rough edges root-caused (live app + 3 code investigations)

Drove the live GPUI app via the `holon-live` MCP (click, type, key chords, arrow nav — all worked, DB-verified), then triaged five user-reported rough edges. Every finding below is grounded in live-app evidence (raw SQL against the running Turso, `/tmp/holon.log` traces, screenshots) or file:line citations.

### 1. Multi-second interaction latency

The reactive half is push-driven and fast — a `navigation.focus` op fanned a 744-row watch batch out in ~1.5 ms. No multi-second timer exists in the local edit path.

**Prime suspect:** `LoroProjection::project()` runs a **full-document DFS snapshot + full diff on every Loro commit** (every keystroke), serialized under a global `project_lock` (`crates/holon-loro/src/loro_sync_controller.rs:349-388`). The UI renders from SQL/CDC, so nothing appears until this O(N) pass + Turso write + IVM completes. This is the prod twin of the "widget snapshot resample = 83% of keystone wall" finding.

**Amplifiers:**
- `TursoBlockQuerySource::snapshot()` settle gate: 50 ms of CDC *silence* required, 5 s ceiling (`crates/holon/src/sync/turso_block_query_source.rs:72-73`, wait loop `crates/holon-api/src/live_data.rs:132-177`). Per-keystroke projection churn keeps CDC noisy → consumers block the full 5 s.
- Org sync loop: every UI edit triggers org re-render + disk write; 100 ms poll re-stats it; 2 s discovery tick (`crates/holon-orgmode/src/di.rs:710-723`).
- `deliver_to_subscribers`: up to 5 s head-of-line block per slow CDC subscriber (`crates/holon-loro/src/event_ring.rs:96-125`).

**Confirm:** rerun with `HOLON_LOG` debug and read `snapshot_ms` spans (`loro_sync_controller.rs:386,470-471`) + `live_data.wait_for_quiescent` `timed_out=true`.

Also observed: RSS +426 MB in ~15 min of light use, one idle +45 MB jump (`[MemoryMonitor]` lines) — possible leak.

### 2. Left sidebar stale after page deletion

Sidebar = `SELECT b.* FROM block b JOIN block_tags bt … WHERE bt.tag='Page'` (`assets/default/index.org:5-15`) → a `watch_view_*` matview **chained on the `block` matview** ⋈ base `block_tags`.

Live evidence of insert-only behavior: at boot the sidebar rendered pages **flat in pre-reparenting positions** while `block_raw` parent links were already correct; it only fixed itself on a later full rebuild.

Ranked causes (not mutually exclusive):
- **H1** Turso IVM chained-matview stale rows on delete — in-repo repro: `crates/holon/examples/turso_ivm_chained_matview_stale_rows.rs`; matches the intermittency.
- **H2** `block_tags` rows never deleted: schema declares `ON DELETE CASCADE` but Turso FK enforcement is off by default and holon never issues `PRAGMA foreign_keys=ON`; `prepare_delete` (`crates/holon/src/core/sql_operation_provider.rs:725-790`) only deletes from `block_raw`.
- **H3** rowid-keyed CDC deletes silently no-op in the GPUI row store: `ReactiveRenderedRows::apply` (`crates/holon-frontend/src/reactive.rs:491-495`) lacks the rowid→key fallback that `LiveData` got (`crates/holon-api/src/live_data.rs:274-287`).

**PBT gap (why the keystone can't see this):**
- No transition ever deletes a page: `transitions/apply_mutation.rs:78,96,122` filter `!b.is_page()`; `PeerEdit::Delete` disabled (`transitions/peer_edit.rs:101`); no `delete_document` transition exists.
- The seeded `left_sidebar::src::0` watch is never registered as a `RefWatch`, so `inv-watch-rows-match-ref` never covers it.

### 3. GitHub page shows no data

Page query blocks read base tables (`gh_repository`, `gh_issue`, `gh_pull_request`) — which had **0 rows**. `~/.config/holon/integrations/github.yaml` is vtable-only (no sync section) → `mcp_full_sync` legitimately no-ops (10 µs in the log; the "pulling data" impression = connect + FDW registration).

Base tables populate **only via lazy writeback when the `_fdw` table is queried** — proven live: one manual `SELECT … FROM gh_repository_fdw LIMIT 3` (≈5 s network) triggered `[WritebackTarget] Wrote 95 rows to 'gh_repository'`. Nothing in the app ever queries the FDWs, so writeback never fires.

Design decision needed: point page queries at `*_fdw` (network hit at render) vs. declarative poll/materialization in the yaml sidecar (essentially deferred generic-MCP phase 3).

### 4. "Type here to add a new block" above the page title; title not h1

Three compounding defects (all cited in `crates/holon-frontend`):
1. Main panel renders via `collection_profile.yaml` `tree_view`, which **lacks the `rules:` arg** setting `role: "page_title"` — the rule only exists in the right sidebar's inline render (`assets/default/index.org:29`).
2. The streaming tree driver evaluates rules with an **empty positional map** (no `level`/`depth`) and **discards returned overrides** (`src/reactive_view.rs:863-876`, esp. L870) — so `eq("level",0)` can never fire on the GPUI path, and `show_bullet:false` would be lost even if it did. Depth is only injected in the static path (`src/render_interpreter.rs:470-479`).
3. Virtual child row is parented to `block:default-main-panel`, which is never in the rowset → `MutableTree::insert` makes it a **root sibling** of the title (`src/mutable_tree.rs:124-135`). Its `sort_key: Float(f64::MAX)` ("appears last") string-encodes via IEEE bits to `"18442240474082181119"`, which sorts lexicographically **before** FractionalIndex hex keys (`crates/holon-api/src/render_eval.rs:35-51`; comparison `src/mutable_tree.rs:24-46`) → placeholder renders first.

Cheap partial fix: `"\u{10FFFF}"` sentinel string instead of the float + parent the virtual row to the focus-root page. Full fix: thread depth + overrides through the streaming driver.

### 5. Clicking some pages (e.g. Holon) freezes the app

Not confirmed (didn't force a repro on the live app; the page rendered fine once mid-session). Ruled out: data size — the Projects/Holon subtree is 126 blocks, no query blocks, no giant content.

**Strong suspect:** page-open `render_entity` creates `watch_view_*` matviews on the fly (`crates/holon-turso/src/matview_manager.rs:452-455`) chained on the `block` matview — the known Turso matview-on-matview DDL hang (see `turso-chained-matview-hang` skill). Supporting log evidence: `render_entity('block:cc-conversation') failed: Failed to create materialized view` — same family, failed loudly instead of hanging.

**Confirm:** `sample Holon 3` during a freeze.

### Bonus findings

- MCP `type_text` reports `keystrokes_sent` even when nothing has focus and keys land nowhere (fail-silent).
- MCP `list_commands` returns `[]` for blocks whose `describe_ui` shows 14 ops.
- `describe_ui` on a leaf block id returns empty instead of an error.
- Journals doc carries ~50 persisted empty blocks (real tree items, not placeholders).
- Turso ignored `count(*)` over a recursive CTE (returned the raw row set) — possible upstream bug.
- `github.yaml` sidecar holds a plaintext PAT — rotate if transcript exposure matters.

### Fix plan (test-first, PBT-preferred)

Phase 1 — lock bugs in as failing tests:
- (#2) Register the seeded sidebar watch as a `RefWatch` in `start_app.rs` ref-apply (free coverage via `inv-watch-rows-match-ref`) + add a page-delete transition with ref-model cascade. Expect red seeds.
- (#4) Unit PBT: virtual-child sort key sorts after any FractionalIndex key. Keystone invariant: focused page's title row precedes the virtual child; title carries `role=page_title`.
- (#5) Integration test: watch-view DDL must time out + surface an error instead of blocking page open.

Phase 2 — fixes driven green: (#2) rowid fallback in `ReactiveRenderedRows::apply`, `block_tags` cascade in `prepare_delete` (or `PRAGMA foreign_keys=ON`), upstream chained-matview fix via /turso-fix; (#4) yaml `rules:` + streaming-driver depth/overrides + sort sentinel.

Phase 3 — (#1) confirm with `snapshot_ms`, then de-risk incremental projection (diff from Loro event deltas instead of full-doc resample) with an experiment before refactoring; (#3) design decision (FDW-backed queries vs declarative poll), pressure-test per the Fable-agent directive; (#5) stack sample → upstream reproducer.
