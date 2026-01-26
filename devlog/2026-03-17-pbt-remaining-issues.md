# Handoff: PBT remaining issues after CDC/IVM fixes

## Context

The CDC/IVM bugs from `devlog/2026-03-16-pbt-cdc-handoff.md` are fully resolved:
- **Bug 1** (json_set → stale IVM values): Fixed in `prepare_update()` — full JSON replacement
- **Bug 2** (build_event_payload dropped Value::Object properties): Fixed — added Object branch
- **Bug 3** (invariant #1 used CDC accumulator): Fixed — now uses direct SQL like production
- **Bug 4** (org parser didn't know custom TODO keywords): Fixed — `parse_org_file_blocks` accepts keyword header
- **Turso IVM bugs**: Both fixed upstream in Turso (changes=0 for simple and full schema cases)

The PBT Full variant passes. Two new failures discovered in CrossExecutor and SqlOnly — both unrelated to CDC/IVM.

## Issue 1: EditViaViewModel on blocks with custom entity profiles (CrossExecutor)

### Failure
`sut.rs:873` — `[EditViaViewModel] No EditableText with id=block:... in display tree`

### Minimal reproduction (regression seed line 7)
```
WriteOrgFile { filename: "dj__jdz__...org", content: "* JWMi\n...\n#+BEGIN_SRC holon_entity_profile_yaml\nentity_name: block\ncomputed: {}\ndefault:\n  render: 'row(col(\"content\"))'\nvariants: []\n#+END_SRC\n" }
StartApp { enable_loro: true }
EditViaViewModel { block_id: "block:6t2-0r--g--3v9--vogy778-p6-a08y4", new_content: " b r lsd m rw " }
```

### Root cause
The org file writes a custom entity profile with `render: 'row(col("content"))'`. This overrides the default block render expression (which uses `editable_text(col("content"))`). The `col("content")` widget renders read-only text — no `EditableText` node in the display tree.

The `EditViaViewModel` precondition (`state_machine.rs:769-778`) checks:
1. ✅ `app_started`
2. ✅ Block exists
3. ✅ `content_type == Text`
4. ✅ Not a layout block

But it does NOT check whether the active render expression produces an `editable_text()` widget.

### Fix
Add a precondition check in `state_machine.rs:769` that the active render expression (considering profiles) contains `editable_text`. Roughly:

```rust
E2ETransition::EditViaViewModel { block_id, .. } => {
    state.app_started
        && state.block_state.blocks.contains_key(block_id)
        && state.block_state.blocks.get(block_id)
            .map_or(false, |b| b.content_type == ContentType::Text)
        && !state.layout_blocks.contains(block_id)
        // NEW: check that profile render expression includes editable_text
        && !state.active_profiles.values().any(|profile| {
            // If a profile is active and its render doesn't include editable_text,
            // skip this block
            profile.default_render.as_ref()
                .map_or(false, |r| !r.to_rhai().contains("editable_text"))
        })
}
```

The exact implementation depends on how `active_profiles` maps to specific blocks. The key insight is: custom entity profiles can override the render expression for ALL blocks of that entity type, removing `editable_text`.

### Key files
- `crates/holon-integration-tests/src/pbt/state_machine.rs:769` — precondition
- `crates/holon-integration-tests/src/pbt/sut.rs:848-877` — EditViaViewModel execution
- `crates/holon-integration-tests/src/pbt/generators.rs` — `VALID_PROFILE_YAMLS` defines profile templates

## Issue 2: Region displayed blocks mismatch after NavigateFocus + Create (SqlOnly)

### Failure
`sut.rs:1624` — `Region 'left_sidebar' displayed blocks mismatch after navigation`

### Minimal reproduction (regression seed line 8)
```
WriteOrgFile { filename: "ri_gcltea_...org", content: "#+TODO: TODO STARTED | CLOSED\n* CT8eVXvX8FF\n:PROPERTIES:\n:ID: 93hy\n:END:\n* ZplGU0   G 9 G5SmxC\n:PROPERTIES:\n:ID: fv\n:END:\n* TJZNhDOMWDVdJWv\n:PROPERTIES:\n:ID: m-n9-xmi\n:END:\n" }
StartApp { enable_loro: false }
NavigateFocus { region: LeftSidebar, block_id: "block:fv" }
ApplyMutation(Create { entity: "block", id: "block::block-0", parent_id: "block:fv", fields: {"content": "yejMd2", "content_type": "text"} })
```

### Root cause
After navigating to focus on `block:fv` in the left sidebar, a new child block is created under `block:fv`. The region data (CDC-accumulated from the `focus_roots JOIN block` matview) should reflect the focus root. The assertion compares the region CDC accumulator against the reference model's expected focus roots.

The mismatch suggests either:
1. The `focus_roots` matview didn't update after the `NavigateFocus` operation (IVM timing)
2. The reference model's `expected_focus_root_ids()` doesn't account for the navigation correctly
3. The region CDC stream didn't deliver the navigation change within the drain timeout

### Hypotheses (sorted by probability)
**H1: Region CDC drain timeout too short** — The `drain_region_cdc_events` uses 200ms timeout. After `NavigateFocus` + `Create`, the region matview might need more time to propagate through the chained IVM (navigation_cursor → navigation_history → current_focus → focus_roots → JOIN block).

**H2: Reference model focus state wrong** — `expected_focus_root_ids()` might return incorrect IDs when a child block is created under the focused block.

**H3: IVM chaining issue** — The `focus_roots JOIN block` chained matview might not fire CDC after the navigation INSERT + block CREATE happen in quick succession.

### Suggested approach
1. Add `eprintln!` in `drain_region_cdc_events` to see if any events arrive
2. Check if the focus_roots matview has the correct data via direct SQL query
3. If the data is correct in SQL but not in CDC, it's a timing issue — increase drain timeout or add a re-drain

### Key files
- `crates/holon-integration-tests/src/pbt/sut.rs:1600-1636` — region data assertion (invariant 8)
- `crates/holon-integration-tests/src/test_environment.rs:958-982` — `setup_region_watch()` and `drain_region_cdc_events()`
- `crates/holon-integration-tests/src/pbt/reference_state.rs` — `expected_focus_root_ids()`
- `crates/holon/src/navigation/provider.rs` — `NavigationProvider::focus()`

## How to run

```bash
# Run all 3 variants (Full passes, CrossExecutor + SqlOnly fail on regression seeds)
cargo test -p holon-integration-tests --test general_e2e_pbt -- --test-threads=1

# Run individual variants
cargo test -p holon-integration-tests --test general_e2e_pbt general_e2e_pbt_sql_only -- --test-threads=1
cargo test -p holon-integration-tests --test general_e2e_pbt general_e2e_pbt_cross_executor -- --test-threads=1

# Turso IVM reproducers (both should pass now)
cargo test -p holon --lib turso_ivm_cdc_zero_changes_repro -- --nocapture
```
