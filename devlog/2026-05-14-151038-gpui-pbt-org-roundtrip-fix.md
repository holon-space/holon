---
date: "2026-05-14 15:10"
project: "holon"
---

# GPUI PBT seed=42 handoff — org-roundtrip fix landed; failure B is next

Continues `devlog/2026-05-14-134651-gpui-pbt-seed42-handoff.md`.

Reproducer: `PROPTEST_SEED=42 cargo test -p holon-gpui --test gpui_ui_pbt --features pbt`.

## TL;DR for the next session

1. **Org-roundtrip parent_id mismatch (steps 5/6) is FIXED deterministically.** It had become the dominant blocker post-handoff (2/2 reruns hit it before any other failure).
2. **The current blocker is failure B from the prior handoff**: `wait_for_entity_bounds` 5s timeout for `block:c2f12z-s` at step 4 SplitBlock. It now surfaces *deterministically* on seed=42 instead of 2-of-5-flake.
3. New diagnostic data captured: Main panel renders only 2 of ref-doc-0's children. See "Failure B current state" below for the panel tree.
4. Recommended path forward is the same the prior handoff already named: `PBT_PAUSE_SECONDS=20` + holon-live MCP on port 8528, plus the specific SQL/view-model queries listed below.

## Status table

| Failure | Pre-fix (seed=42) | Post-fix (seed=42) |
|---|---|---|
| org-roundtrip mismatch on step 5/6 WriteOrgFile (`block:-9` parent `block:ref-doc-1` vs `file:index.org`) | deterministic | **gone** |
| `apply_bulk_external_add` panic: `Document not found for BulkExternalAdd: block:ref-doc-1` | exposed by first half of fix | **gone** (second half of fix) |
| **Failure B**: `wait_for_entity_bounds` timeout 5s for `block:c2f12z-s` at SplitBlock step 4 | 2/5 flake | **deterministic** (now the gating failure) |

## What landed this session

### Root cause of the org-roundtrip mismatch

Two related missing-update sites for documents added **post-startup** via `WriteOrgFile`:

1. `sut.rs:3826` `synthetic_to_parent` mapping read `self.doc_uri_map` directly, but `self.doc_uri_map` is only populated in `apply_start_app` (sut.rs:728–756). For docs created later by post-startup `WriteOrgFile`, the lookup missed and fell back to `EntityUri::file(filename)` — while production's parser had already resolved the doc to `block:ref-doc-1` via the `#+ID:` injection that `WriteOrgFile::apply_to_sut` performs. Result: every block under that doc compared unequal between actual and reference.
2. `apply_write_org_file` (sut.rs:498) wrote the file but never re-keyed `self.ctx.documents` from `file:<filename>` → the resolved doc URI. So later transitions calling `resolve_uri(doc) → ctx.documents.get(resolved)` (e.g. `apply_bulk_external_add` at sut.rs:1404) panicked with `Document not found`.

`apply_start_app` already did both at startup; the post-startup path just never replicated them.

### Fixes (both in `crates/holon-integration-tests/src/pbt/sut.rs`)

1. **`synthetic_to_parent` now uses `lazy_doc_uri_map`** — the local map that `check_invariants_async` was already building via `ctx.resolve_doc_uri_by_name` for any unresolved doc. Just a one-line rebind; no extra resolution work.
2. **`apply_write_org_file` mirrors `apply_start_app`'s resolve loop** when the app is running (`is_running()` gate). After writing, it polls `ctx.resolve_doc_uri_by_name(filename)` for up to 5s; on success, re-keys `ctx.documents` from `file:<filename>` → resolved, and seeds `doc_uri_map.insert(resolved, resolved)` (the synthetic equals the resolved because `apply_to_sut` injects `#+ID: <synthetic>`).

`check_invariants_async` is `&self`, so a write-through into `self.doc_uri_map` was not possible without an invasive signature refactor; the duplicated resolve in `apply_write_org_file` is the lighter fix and matches the start-app pattern.

## Failure B current state — what to investigate next

After NavigateFocus(ref-doc-0) at step 3, the Main panel renders only 3 entries (`/tmp/run5.log` lines 313–347):

```
column#40 -> rendered_text -> block:-q--2b-9--g39c5-e06u1565-5    (read-only)
column#49 -> rendered_text -> block:nvhz--r75-0sz-7-n37s9o5x7j     (read-only)
column#58 -> editable_text -> block:ref-doc-0                      (the page itself)
```

All other ref-doc-0 children (including `c2f12z-s`) are absent from the frontend tree, not just from `BoundsRegistry`. The "tried the registry under alternate ids" theory from the prior handoff is now ruled out: the render list itself is short.

### Hypotheses (in likely order)

1. **Main render binding filters on a property `c2f12z-s` has** — `c2f12z-s` has `task_state: WAITING` in its properties (visible in the prior org-roundtrip diff). Suspect the layout's render query excludes blocks with a non-empty `task_state` (intentional: tasks render via a tasklist view, not as plain text rows), or excludes `WAITING` specifically.
2. **Live blocks matview lag** — `c2f12z-s` was inserted in setup step 1; its propagation through the matview that backs Main's render binding may not have completed by step 4. Note: `block_raw` / `live_blocks` have a CDC-delivery gate elsewhere in the PBT, but the Main panel's render binding is a different matview chain (likely query-block → render-block → list of children) and isn't gated.
3. **Virtualized list cutoff** — least likely, the panel is 936×1072 and only ~3 rows are rendered; lots of vertical space below.

### Concrete next steps

```
PBT_PAUSE_SECONDS=20 PROPTEST_SEED=42 cargo test -p holon-gpui --test gpui_ui_pbt --features pbt
```

Then attach via the holon-live MCP on port 8528 and run:

- `SELECT * FROM block_raw WHERE id = 'block:c2f12z-s'` — confirms write side. Should show `task_state = "WAITING"` in properties.
- `SELECT * FROM block WHERE id = 'block:c2f12z-s'` — confirms the matview has it.
- `mcp__holon-live__holon_pbt__describe_ui` and `…__inspect_loro_blocks` for the Main panel's view model — see if `c2f12z-s` is in the view model but absent from the frontend, vs absent from the view model entirely.
- Print the Main panel's render binding source (look at the `block:-9::render::0` and `block:-9::src::0` source bodies for index.org; alternative if the test wrote a different layout file, inspect the source attached to the focused page's render block via `mcp__holon-live__holon_pbt__execute_source_block` or `list_loro_documents`).

Resolution paths:

- If hypothesis 1: either make the SplitBlock generator pick only blocks the panel actually renders (read the view-model rendered children list), or make `c2f12z-s` not have `task_state` set in the test fixture.
- If hypothesis 2: add a pre-SplitBlock wait gated on the panel's view-model rendered children list rather than `block_raw`/`live_blocks`.

The prior handoff also flagged that *don't* reach for the editor_focus / watch_editor_cursor structural fix until bounds is sorted — same fragility may bite the new EditorView's mount path.

## Files touched (uncommitted in working copy `@`)

- `crates/holon-integration-tests/src/pbt/sut.rs`:
  - `apply_write_org_file` (≈ line 498) — added post-write doc URI resolve + re-key gated on `ctx.is_running()`.
  - `check_invariants_async`'s `synthetic_to_parent` construction (≈ line 3826) — read from `lazy_doc_uri_map` instead of `self.doc_uri_map`.

No other files changed this session. The rest of the `jj st` working-copy modifications are inherited from prior sessions (see `devlog/2026-05-14-134651-gpui-pbt-seed42-handoff.md` "Files touched").

## Repro log artifacts (transient)

- `/tmp/run1.log`, `/tmp/run2.log` — pre-fix runs showing the deterministic org-roundtrip mismatch on step 5/6 with the `block:-9` parent diff.
- `/tmp/run4.log` — after first half of fix, panic moved to `apply_bulk_external_add` `Document not found`.
- `/tmp/run5.log` — after full fix, panic moved to step 4 SplitBlock `block:c2f12z-s` bounds timeout. Contains the Main-panel render tree dump that establishes c2f12z-s is absent from the view, not from BoundsRegistry alone.

These are in `/tmp` and won't survive; re-run if you need them.
