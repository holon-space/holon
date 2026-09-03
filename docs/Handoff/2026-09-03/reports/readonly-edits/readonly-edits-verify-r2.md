# Verify `readonly-edits` rev 2 — CONFIRMED (with two reported gaps, no remedies)

Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/readonly-edits` (`pwd` confirmed).
Step 0: `rg -n "editable_field_any|ReadOnlyTextCellBacking" crates/` non-empty — right tree
(`crates/holon-frontend/src/editor_view_model.rs`, `crates/holon-loro/src/block_cell_registry.rs`,
`crates/holon-core/src/write_tier_gate.rs`, `crates/holon-core/src/cell_registry.rs`,
`crates/holon-core/src/cell.rs`).

## 1. Seam audit

`rg -n "live_field_any|editable_field_any|apply_text_op|write_field|set_field"` over `crates/`
and `frontends/` (`lane-logs/verify-r2-seams.log`, 1201 lines). Every call site classified:

- **Guarded (`editable_field_any` / `editable_field`)** — the only production caller is
  `ReactiveEngine::editable_text` (`crates/holon-frontend/src/reactive.rs:4365-4383`), which the
  GPUI editor (`frontends/gpui/src/views/editor_view.rs:163`) and the TUI
  (`frontends/tui/src/app_main.rs:999`) both call for the `content` field only. This is the
  keystroke path.
- **`live_field_any` / `live_field`, origin judged upstream at the dispatcher** —
  `BlockCellRegistry::write_field` (`crates/holon-loro/src/block_cell_registry.rs:818` for
  content, `:946` for other scalars), reached from
  `crates/holon/src/core/sql_block_operations.rs:1075`, which the `OperationDispatcher` calls
  only after `enforce_write_tier` (`crates/holon/src/api/operation_dispatcher.rs:527-549`) has
  already resolved `id`/`parent_id` against the same `WriteTierAuthority` and let the op through
  (any origin except `Ingest`/`Sync` is refused there before the provider runs). Same story for
  `write_position` (`block_cell_registry.rs:382`, called from `sql_block_operations.rs:473` and
  `traits.rs:933`, both post-dispatch-gate) and for `read_content_via_cells`
  (`crates/holon-core/src/traits.rs:888`, a **read**, not a write).
- **Neither guarded nor origin-judged** — none found. `rg -n "live_field"` restricted to
  `frontends/` and `crates/holon-frontend/src/` returns exactly one hit, a doc comment
  (`reactive.rs:2383`); no frontend code resolves `live_field`/`live_field_any` directly.
- **User-initiated non-content writes** (checkbox / task-state) go through
  `state_toggle.rs:106` → `services.dispatch_intent(intent)` → the dispatcher →
  `enforce_write_tier`, not through any cell seam at all.

No bypass found among reachable production callers.

## 2. Live probe (throwaway, appended to `cook_vault_ingest.rs`, removed after; sha256 restored — see §3)

Ran `verify_r2_scratch_probe` against `Pancakes.cook` (`lane-logs/verify-r2-probe.log`):

| probe | call | result |
|---|---|---|
| A | `editable_field::<Value>(step, "completed").set(true)` | **succeeded** (unguarded — see gap 2 below) |
| B | `editable_field::<String>(step, "block_type").set("hacked_type")` | **succeeded** (unguarded — same gap) |
| C | `live_field::<String>(step, "content").apply_text_op(Insert "HACKED ")` | **succeeded** — mints `"HACKED Crack the eggs..."` in `get_block_authoritative` (see gap 1 below) |
| D | `editable_field::<String>(step, "content").apply_text_op(Insert "ZZZ ")` | refused: `cooklang is a read-only format: …/Pancakes.cook is authoritative input…` |
| D | `editable_field::<String>(step, "content").set("WHOLESALE")` | refused, same message |

Disclosure fired once: `[("edit-refused-read-only-format", ".../Pancakes.cook")]`.

Probe D is the production keystroke path end-to-end (same seam `editor_view.rs`/`app_main.rs`
use) and is refused. Drag/drop reorder, paste, and the `[[` picker insert all reduce to either
`write_position`/`enforce_write_tier` (reorder) or `TextOp` through the content cell (paste,
`[[` insert), both already exercised above; I traced these structurally rather than driving them
through GPUI given the time budget — flagged, not asserted green.
`OpOrigin::Sync` re-checked at `operation_dispatcher.rs:536`: unchanged, passes by design (a
peer's merged history), matching the lane report and rev-1 verdict.

## 3. Teeth

Baseline sha256 of `crates/holon-loro/src/block_cell_registry.rs`:
`cd4db75f8896e27cfb68709a0e7ac3b51ebcde050b2b023131d8518ed1978eca`. Disabled the wrapping hunk
(`if field != "content" { return Ok(cell_any); }` → unconditional early return). Reran
`cook_vault_ingest`: `a_keystroke_on_a_recipe_block_is_refused_at_the_cell` went **red**
(`lane-logs/verify-r2-teeth.log`, "a keystroke on a read-only-format block must be refused at the
cell: ()"), 8/9 otherwise green. Restored via cp-aside; sha256 matches the baseline again.

## 4. Rev-1 bypass shape, repeated against rev 2

The rev-1 verdict's counterexample was `EditorViewModel::apply_local_edit` → `apply_local` →
`cell.apply_text_op` on the cell GPUI attaches via `editable_text`/`editable_field_any`. That is
exactly probe D above (the promoted test drives the identical seam with the production registry)
— now refused. **Fixed for the path the rev-1 verdict actually exercised.**

## 5. Disclosure

Rendered, not just logged: `frontends/gpui/src/share_ui.rs:332` maps
`ShareDegradedReason::EditRefusedReadOnlyFormat` to `DegradedKind::EditRefusedReadOnlyFormat`,
and `share_ui.rs:1749-1752` renders the banner text `"Edit refused — read-only file"`. The
caret-enters-a-recipe-row limitation is documented as open PARTIAL in
`docs/Testing/bugfunnel/entries/2026-09-03-read-only-format-blocks-accept-edits-that-are-discarded.md:116-120`
— confirmed, matches the lane report's own "still deferred" item 1.

## 6. Gate rerun

- `cargo nextest -p holon-frontend -p holon-loro -p holon-core -E 'test(read_only) + test(readonly) + test(write_tier) + test(cell)'` → **52 passed, 0 failed**, 1039 filtered out (`lane-logs/verify-r2-gate.log`).
- `just keystone-smoke` → **FAILED**: `general_e2e_composed_pbt` panicked at
  `harness.rs:1206` on an `inv-sql-budget` violation (`OpenTabViaModifierClick.sql_reads: 24
  exceeds expected 23 + tolerance 0`), in a run wired `authority: block-CRUD=Sql(SqlOperationProvider);
  projection-sinks=Sql(block_raw,matview)` — **CRDT/Loro layer disabled for this composed run**,
  i.e. an SQL-only read-count budget assertion, structurally unrelated to this lane's
  `holon-loro`/`holon-core`/`holon-frontend` cell-registry diff. A `p95 SLO … within the
  machine-load slack` warning immediately precedes it, and the task brief notes the machine is
  shared with 7 other lanes. I did not bisect against a clean `main` to positively rule this out
  — flagging it as a probable environmental/pre-existing red rather than asserting it either way.

## Reported gaps (no remedies — routed back for the orchestrator to decide)

1. **`live_field_any`/`live_field` on `content` is not a public safe API — nothing in the type
   system stops a future caller from getting an ungated write.** Probe C called
   `live_field::<String>(uri, "content").apply_text_op(...)` directly (bypassing
   `editable_field`) and it minted an unguarded op in the authoritative store, identical in shape
   to the rev-1 HACKED probe. No current production code does this (confirmed by the seam audit
   in §1), so this is not a live bypass — but the guarantee rev 2 provides is "every caller who
   goes through `editable_field` for content, or through the dispatcher for everything else, is
   safe," not "the registry itself refuses an ungated write." Any new caller (a plugin, an MCP
   write tool, a test utility, a future frontend) that resolves `BlockCellRegistry` and calls
   `live_field`/`live_field_any` for `content` directly reproduces the rev-1 defect exactly.
2. **`editable_field_any`'s guard is hardcoded to `field == "content"`.** Probes A and B show
   `editable_field::<Value>(uri, "completed")` and `editable_field::<String>(uri, "block_type")`
   both succeed unguarded on a read-only-homed block. No production caller currently calls
   `editable_field` for a non-`content` field — checkbox/state-toggle writes go through
   `dispatch_intent` → the dispatcher instead (§1) — so this is the same class of gap as (1), not
   a live bypass, but it means "editable_field protects you" is true only for one specific field
   by convention, not by contract.
3. `just keystone-smoke`'s `general_e2e_composed_pbt` failure (§6) — not chased to a root cause;
   reported so the orchestrator can decide whether to treat it as pre-existing/environmental or
   investigate before landing.

## Verdict

**CONFIRMED**: rev 2 closes the specific defect the rev-1 verdict found (the editor's keystroke
path on `content`, the only cell seam any production frontend code reaches), and no other
production-reachable write path bypasses `enforce_write_tier`/the cell-level guard. The claim
"closes EVERY write path" holds for every path this session could enumerate as reachable from
running code. It does **not** hold as a structural/type-level guarantee — gaps 1 and 2 above are
real, latent, currently-unreachable-in-prod holes in the same shape as the rev-1 defect, worth a
one-line note in the lane report's "deferred" section even though nothing exploits them today.
