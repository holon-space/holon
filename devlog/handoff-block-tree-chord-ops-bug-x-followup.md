# Handoff: Bug X closed and chord-op PBT green at PROPTEST_CASES=100

**Status (2026-04-27):** Bug X from `handoff-block-tree-chord-ops-followup.md` is closed. Chord-op-weighted PBT passes:
- **20/20 cases in 726s** (initial verification)
- **100/100 cases in 1860s** (~31min, deeper verification)

Three additional shrinker-vs-generator gaps were fixed along the way. Chord-op layer can now be declared settled.

## What this pass closed

The shrinker only re-checks `preconditions(state, transition)`; it does **not** re-run the strategy generator that originally proposed the transition. Anything the generator gates on but the precondition doesn't can be silently violated by shrinking. Three transitions had this gap.

### Bug X (handoff): `ToggleState` precondition tightened (3 layers)

Original precondition was just `state.app_started`. The generator at `state_machine.rs:916/935` filtered candidates by `main_focus_roots`, layout headlines, and `state_toggle_block_ids()` — none of that survived shrinking. ToggleState's `wait_for_entity_in_resolved_view_model` at `sut.rs:1727` timed out on shrunk-into-invalid sequences.

Now (in `crates/holon-integration-tests/src/pbt/state_machine.rs:1646-1668`):

```rust
E2ETransition::ToggleState { block_id, .. } => {
    let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
    state.app_started
        && state.current_focus(holon_api::Region::Main).is_some()
        && state.active_layout_renders_region(holon_api::Region::Main)
        && focus_roots.contains(block_id)
        && !state.layout_blocks.contains(block_id)
}
```

Three structural prerequisites mirrored:
1. **Main navigation focus exists** + **block is a direct child of focus** — the generator's `main_focus_roots.contains(*id)` filter (line 935). Without a Main nav focus the panel renders empty rows; even with focus, the block must be one of those rows.
2. **`active_layout_renders_region(Main)`** — see "Layer 3" below: production must actually render entity-bound content in the panel for `wait_for_entity_in_resolved_view_model` to find anything.
3. **Block is not a layout headline** — layout headlines define their own render expression that can omit `state_toggle` entirely. `EditViaViewModel`/`Indent`/`Outdent`/`MoveUp`/`MoveDown`/`TriggerSlashCommand` already enforce this; ToggleState now matches.

### Layer 3: `active_layout_renders_region` distinguishes "no layout" from "unparseable layout"

`reference_state.rs:662` previously conflated:
- **No root layout headline** (truly default `assets/default/index.org`) → return `true` (default renders all three regions)
- **Custom root layout headline whose render expression we can't parse** → also returned `true` (wrong)

The PBT generators in `pbt/generators.rs:232-242` (`index_file_gql_varlen`) produce expressions like `list(item_template: row(text("varlen")))` that aren't in `valid_render_expressions()`. The reference parser returns `None`, so `render_expressions` ends up empty for that render source, and `root_render_expr()` returns `None` — which used to mean "default layout in effect". The ref state then thought clicks/toggles would resolve normally; production rendered the custom layout's static `text("varlen")` rows that have **no entity binding**, so `wait_for_entity_in_resolved_view_model` timed out.

Fix: split the two cases.

```rust
let Some(_root_id) = self.root_layout_block_id() else {
    return true; // no custom layout
};
match self.root_render_expr() {
    Some(expr) => /* check live_block_targets */,
    None => false, // unparseable custom layout — predict no entity rendering
}
```

This unblocks `focusable_rendered_block_ids(Main)` / ClickBlock and the new ToggleState gate to refuse Main-region transitions whenever the test wrote an unparseable layout.

### Loro peer-sync ordering: recanonicalize after merge

Both `MergeFromPeer` and `SyncWithPeer` apply branches called `merge_peer_blocks_into_primary` but never recanonicalized sequences afterwards. New peer blocks landed at default `sequence=0` (from `Block::default()` inside `from_block_content`), colliding with existing children's sequences and producing a child order the org renderer would never output.

Concrete shrunk repro:
```
WriteOrgFile (custom index.org with render+gql layout)
StartApp + AddPeer
PeerEdit Create { parent_stable_id: "812qj9-2u1" }
SyncWithPeer
```

After merge:
- `render::0` had `sequence=0` (recanon'd by WriteOrgFile)
- `src::0` had `sequence=1`
- `peer-...` had `sequence=0` (default)

Ref state sorted by `(sequence, id)` → `[render(0), peer(0; lex < src), src(1)]`.
Production sorted by `(content_type group, sort_key, id)` → `[render, src, peer]` (sources first).

Fix: add `state.recanon_and_rebuild()` after `merge_peer_blocks_into_primary` in both arms (`state_machine.rs` MergeFromPeer + SyncWithPeer). Required making `recanon_and_rebuild` `pub` in `reference_state.rs`.

## Verified

`PROPTEST_CASES=20` with chord-op weights:

```
Bug X (focus_roots layer)    → ToggleState entity not in ViewModel
  → Bug X (layout_blocks layer) → ToggleState on layout-headline
    → Bug X (active_layout layer) + peer-sync recanon → ClickBlock-on-unparseable-layout
      → ALL FIXES → 20/20 PASS in 726s ✓
```

Final command:
```sh
PROPTEST_CASES=20 PBT_WEIGHT_INDENT=10 PBT_WEIGHT_OUTDENT=10 PBT_WEIGHT_SPLIT_BLOCK=10 \
PBT_WEIGHT_CLICK_BLOCK=10 PBT_WEIGHT_DEFAULT=1 \
cargo nextest run -p holon-integration-tests --test general_e2e_pbt \
  -E 'binary(general_e2e_pbt) and test(=general_e2e_pbt)'
```

PROPTEST_CASES=100 with the same weights also passed: **100/100 in 1860s (~31min)**, log at `/tmp/pbt-deeper-100.log`. No deeper failure modes surfaced.

## Files touched (3, all test-side)

- `crates/holon-integration-tests/src/pbt/state_machine.rs` — ToggleState precondition (3 new layers); SyncWithPeer + MergeFromPeer recanon calls.
- `crates/holon-integration-tests/src/pbt/reference_state.rs` — `recanon_and_rebuild` made `pub`; `active_layout_renders_region` distinguishes "no layout" from "unparseable layout".

No production code touched.

## Logs

- `/tmp/pbt-toggle-after-switchview.log` — Bug X repro (pre-fix).
- `/tmp/pbt-toggle-after-fix.log` — focus_roots fix; layout-headline case still open.
- `/tmp/pbt-toggle-after-fix2.log` — layout_blocks added; Bug X gone, peer-sync ordering surfaces.
- `/tmp/pbt-after-peer-recanon.log` — recanon fix; ClickBlock-on-unparseable-layout surfaces.
- `/tmp/pbt-after-active-layout-fix.log` — all fixes; **PASS 20/20 in 726s** ✓
- `/tmp/pbt-deeper-100.log` — PROPTEST_CASES=100 in progress.

## Key non-obvious findings to remember

1. **Shrinker re-checks preconditions, not generators.** Any structural prerequisite the generator enforces but the precondition doesn't can be silently violated by shrinking. Lesson: anything cheaply re-checkable at precondition time, *do* re-check, even if the generator already gates on it.

2. **`render_expr_from_rhai` returning `None` is meaningful state.** The reference state parser only recognizes a fixed set of `valid_render_expressions()`. When the org-file generator writes content outside that set (e.g. `index_file_gql_varlen` → `list(item_template: row(text("varlen")))`), `render_expressions` stays empty for that source. Code that reads `render_expressions` must distinguish "no layout exists" (`root_layout_block_id() == None`) from "layout exists but unparseable" (`root_layout_block_id() == Some` but `render_expressions` empty for its render child). The first means default rendering; the second means the test can't predict what production renders.

3. **Default `Block::default()` sets `sequence=0`.** Any code path that creates a `Block` via `..Self::default()` (e.g. `from_block_content` used by `merge_peer_blocks_into_primary`) needs to either set a real sequence afterwards or trigger a recanon, otherwise the new block's `sequence=0` will collide with any sibling whose sequence has been recanonicalized to 0.

## What's left

### Documented divergences worth revisiting

- `assertions.rs:108-115` still skips the order check when *all* siblings are sources (pre-existing). Now that single-text-among-source-siblings is correctly verified, the all-source case might also be tightened.
- `region_render_source_customized` only gates LeftSidebar; the new `active_layout_renders_region` change covers the Main-panel customized case more broadly. The two predicates may now overlap; consider unifying.
- Inv11/inv13 use `expected_focus_root_ids` independently of `active_layout_renders_region`. If they ever generate spurious focus-chain rows under unparseable layouts, the same gate may need to be threaded through there too.
