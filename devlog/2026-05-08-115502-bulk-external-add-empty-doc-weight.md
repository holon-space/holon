# BulkExternalAdd: dynamic weight when most docs are empty

**Date**: 2026-05-08
**Continues**: `devlog/2026-05-08-014542-gpui-pbt-edit-starvation-handoff.md`

## What changed

`BulkExternalAdd::weighted_generator` (crates/holon-integration-tests/src/pbt/transitions/bulk_external_add.rs) now scales its returned weight on the fraction of empty documents:

- A doc is "empty" when no block has `parent_id == doc_uri && content_type == Text && !is_page() && !layout_blocks.contains(...)`.
- If at least half the docs are empty, weight = 100 (dominates the strategy union).
- Otherwise weight = 1 (base, lets the rest of the suite explore).

This is dynamic and self-balancing: bulk-add fires hard until docs hold Text blocks, then steps aside.

## Why

Per the prior handoff, every edit-path generator (`SplitBlock`, `Indent`, `Outdent`, `EditViaViewModel`, `ClickBlock`@Main, …) filters on `state.main_editable_descendants()` — which returns empty in the default seed because Main is pinned to the layout block `block:default-main-panel`. Without seeded content under any document, the PBT never reached editor code.

## Verification

Run: `cargo test -p holon-gpui --test gpui_ui_pbt --features pbt 2>&1 | tee /tmp/gpui_ui_pbt_v2.log`

Edit-path transition counts (compared to pre-change run from the prior handoff):

| Transition | Before | After |
|---|---|---|
| BulkExternalAdd | 0 | 10 |
| FocusEditableText | 0 | 4 |
| Blur | 0 | 3 |
| NavigateFocus | 0 | 4 |

The PBT does now reach editor code paths.

## Surfaced bug — `Blur` escape not consumed

At step 15 the run panics:

```
thread '<unnamed>' panicked at crates/holon-integration-tests/src/pbt/sut.rs:2345:14:
Blur: escape failed: GPUI keystroke not consumed: keystroke="escape" modifiers=[]
```

This is exactly the class of bug the prior handoff hoped to expose. `Blur::apply_to_sut` sends Escape via GPUI and asserts the keystroke was handled; the panic means the active editor (or its container) didn't claim the Escape. Real bug, not an artifact of the weighting fix.

Next investigation will start there. Likely candidates: focus chain not pointing at the editor when Blur fires; editor's keymap missing an `escape` binding; the active editor being an ephemeral cell that no longer exists by the time the keystroke reaches the focus tree.

## Drive-by

`frontends/mcp/src/tools.rs:1610` had a pre-existing macro error blocking workspace builds: `serde_json::json!({...})` does not accept inline method-chains. Extracted `let warnings: Vec<String> = ...collect()` before the macro. This was WIP in user's tree; left the change in place since I needed it to build the test crate.

## Files touched

- `crates/holon-integration-tests/src/pbt/transitions/bulk_external_add.rs` — added empty-doc count + dynamic weight
- `frontends/mcp/src/tools.rs` — extracted `warnings` vec before the `json!` macro to fix the macro-parse error

## Logs

- `/tmp/gpui_ui_pbt_v2.log` — full run; counts above derived from `grep -oE 'pbt_step.*Step [0-9]+/50: [A-Za-z_]+'` and family-grep
