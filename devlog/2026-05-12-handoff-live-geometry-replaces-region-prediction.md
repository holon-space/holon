# Handoff — replace `focusable_rendered_block_ids` with live-geometry source-of-truth (Option B)

**Status:** Option A (drop-in fix in `NavigateFocus`) being landed in the same session.
This handoff covers the larger follow-up: kill the entire ref-state region-prediction
maze and route every "what's rendered in region X" query through `BoundsRegistry`.

## Why

The current `ReferenceState::focusable_rendered_block_ids(region)` predicts what
the GPUI window will render by:

1. Bailing out if `region_predictable(region)` is false. That predicate ANDs
   `active_layout_renders_region` (walks `root_render_expr().live_block_targets()`
   for a matching panel id, with several "case 1/2/3a/3b" fallbacks) and
   `!region_render_source_customized(region)` (checks if a layout mutation
   overwrote the panel's seeded render-source child).
2. For `LeftSidebar`, hardcoding the default sidebar PRQL output as
   "every named text page except `index` and `__default__`". This filter is
   pasted into the ref-state and only valid for the default layout (`reference_state.rs:1087-1101`).
3. For `Main`/`RightSidebar`, projecting `expected_focus_root_ids` + descendant
   walk + `layout_blocks.is_focusable` filter.

This produces brittle false negatives in two directions:

- **Empty when reality is non-empty** — current `gpui_ui_pbt` symptom. Histogram
  shows `NavigateFocus: 50 × SidebarFocusNotRendered` even though the user's
  screenshot shows the LeftSidebar rendering "Journals", "index", "__default__".
  Generator never fires → click-to-focus pipeline never exercised → regressions
  like the `rendered_text` `block_id` URI bug (this session) slip through.
- **Non-empty when reality is empty** — happens when a layout mutation that
  the predicate doesn't recognize silently empties the panel; ref-state happily
  generates `ClickBlock`/`NavigateFocus` against a phantom id, SUT dispatch
  times out at `wait_for_element_bounds`.

`BoundsRegistry` already has authoritative live data. Routing every region
query through it removes the predicate maze entirely and naturally tracks
custom layouts / mutations / dynamic visibility changes.

## What B is (the part Option A doesn't do)

Option A patches the `NavigateFocus` generator at a single call site. The
function `focusable_rendered_block_ids` keeps existing for all other callers.
That leaves two sources of truth.

Option B replaces the implementation of `focusable_rendered_block_ids(region)`
with a live-geometry lookup, headless-fallback to the current prediction logic.
Every existing caller transparently gets correct behaviour under the GPUI PBT.

## Existing infrastructure to reuse

### `crates/holon-frontend/src/geometry.rs`

`ElementInfo` (lines 12-44) already carries everything we need:

- `x, y, width, height` — bounds in window coords.
- `widget_type` — e.g. `"live_block"`, `"render_entity"`, `"selectable"`, `"rendered_text"`.
- `entity_id: Option<String>` — the URI bound to this widget (full scheme, e.g.
  `"block:journals"`).
- `parent_id: Option<String>` — id of the immediate tracked-render-tree parent.
  Populated by GPUI's `TransparentTracker` via a thread-local stack. The
  invariant: walking `parent_id` from any tracked element terminates at a
  root-level `tag("reactive_shell", ...)` or `tag("live_block", ...)` element.

`GeometryProvider::all_elements()` returns `Vec<(String, ElementInfo)>` — the
full snapshot. `find_by_entity_id(entity_id)` walks `all_elements` for a match.

### `crates/holon-integration-tests/src/pbt/live_geometry.rs`

Already wraps the GPUI window's `BoundsRegistry` behind a `OnceLock<Arc<dyn
GeometryProvider>>`. Helpers:

- `rendered_entity_ids() -> Option<HashSet<String>>` — entities currently tracked
  with `has_content == true`. `None` when no provider installed.
- `is_entity_rendered(entity_id) -> bool` — false when no provider.
- `is_entity_rendered_or_no_geometry(entity_id) -> bool` — permissive: true when
  no provider, otherwise checks bounds presence.
- `filter_to_rendered(blocks) -> Vec<EntityUri>` — filter helper, no-op in
  headless.

Installed once at `frontends/gpui/tests/gpui_ui_pbt.rs:86` via `live_geometry::install(...)`.

The PBT framework already trusts these helpers in `focus_editable_text.rs:66-79`
(`live_rendered.contains(id.as_str())`) — so the precedent for "ask geometry,
fall back to permissive in headless" is established.

## B implementation plan

### Step 1 — add a region-scoped lookup helper in `live_geometry.rs`

```rust
/// Entity ids of all elements whose tracked-render-tree ancestor chain
/// includes a `live_block` widget with `entity_id == panel_entity_id`.
/// Returns `None` when no geometry provider is installed (headless mode);
/// callers should fall back to the ref-state prediction.
pub fn rendered_entity_ids_in_panel(panel_entity_id: &str) -> Option<HashSet<String>> {
    let provider = LIVE_GEOMETRY.get()?;
    let elements: HashMap<String, ElementInfo> = provider.all_elements().into_iter().collect();

    // Find the `live_block` element(s) whose `entity_id` matches the panel.
    let panel_ids: HashSet<&String> = elements
        .iter()
        .filter(|(_, info)| {
            info.widget_type == "live_block"
                && info.entity_id.as_deref() == Some(panel_entity_id)
        })
        .map(|(id, _)| id)
        .collect();
    if panel_ids.is_empty() {
        return Some(HashSet::new());
    }

    // Walk parent_id from each element; if it terminates at one of the panel
    // root ids, collect this element's entity_id (if any).
    let mut result = HashSet::new();
    for (id, info) in &elements {
        let Some(entity_id) = info.entity_id.as_deref() else {
            continue;
        };
        if entity_id == panel_entity_id {
            continue; // exclude the panel itself
        }
        let mut cursor: Option<&String> = info.parent_id.as_ref();
        let mut depth = 0;
        while let Some(p) = cursor {
            if panel_ids.contains(p) {
                result.insert(entity_id.to_string());
                break;
            }
            depth += 1;
            if depth > 100 {
                // Defensive: tracked tree shouldn't be this deep.
                break;
            }
            cursor = elements.get(p).and_then(|i| i.parent_id.as_ref());
            // current_id is `id` not `p`; safe because we map by id key.
            let _ = id;
        }
    }
    Some(result)
}
```

Add unit tests with a hand-rolled `Vec<(String, ElementInfo)>` fixture covering:
- Empty registry → empty set.
- Panel rendered but no children → empty set.
- Direct children → all returned.
- Deep descendants → returned via parent chain walk.
- Mixed panels → only matching panel's descendants.

### Step 2 — switch `focusable_rendered_block_ids` to live geometry

Modify `reference_state.rs:1068-1132`:

```rust
pub fn focusable_rendered_block_ids(&self, region: Region) -> Vec<EntityUri> {
    let panel_id = match region {
        Region::LeftSidebar => "block:default-left-sidebar",
        Region::Main => "block:default-main-panel",
        Region::RightSidebar => "block:default-right-sidebar",
    };

    // Prefer live geometry when a GeometryProvider is installed (GPUI PBT runs).
    // Headless PBT runs fall through to the ref-state prediction.
    if let Some(rendered) = super::live_geometry::rendered_entity_ids_in_panel(panel_id) {
        return rendered
            .into_iter()
            .filter_map(|id| EntityUri::parse(&id).ok())
            .filter(|uri| {
                let Some(b) = self.block_state.blocks.get(uri) else { return false; };
                b.content_type == ContentType::Text
                    && self.layout_blocks.is_focusable(uri)
                    && !self.layout_blocks.contains(uri)
            })
            .collect();
    }

    // Headless fallback — original prediction logic (lines 1076-1131).
    if !self.region_predictable(region) {
        return Vec::new();
    }
    if region == Region::LeftSidebar {
        return self.block_state.blocks.values()
            .filter(|b| {
                if b.content_type != ContentType::Text || !b.is_page() { return false; }
                let t = b.title();
                !t.is_empty() && t != "index" && t != "__default__"
            })
            .map(|b| b.id.clone())
            .collect();
    }
    let focus_roots = self.expected_focus_root_ids(region);
    focus_roots.into_iter()
        .filter(|id| {
            let is_text = self.block_state.blocks.get(id)
                .map(|b| b.content_type == ContentType::Text)
                .unwrap_or(false);
            is_text && self.layout_blocks.is_focusable(id) && !self.layout_blocks.contains(id)
        })
        .collect()
}
```

**Important detail**: the LeftSidebar live-geometry branch DROPS the "exclude
`index` / `__default__`" filter because the production sidebar actually renders
those. The current PBT exclusion is a workaround for prediction-vs-reality drift
that the live path makes redundant. Verify by inspecting the user screenshot —
those rows do exist as clickable sidebar items.

### Step 3 — re-evaluate the histogram

Re-run `PROPTEST_SEED=1 cargo test -p holon-gpui --test gpui_ui_pbt --features pbt`.
Expected:

- `NavigateFocus: SidebarFocusNotRendered` drops from 50 → ≪50.
- `FocusEditableText: NoFocusInMain` drops as `NavigateFocus` fires.
- `FocusEditableText: NoFocusableBlocks` drops once a populated doc becomes
  focused.
- The Drop-guard histogram in `phased.rs::run_pbt_with_driver_sync_callback`
  (added this session) makes this observable on panic too.

### Step 4 — clean up dead prediction code

Once the live-geometry branch is the production path and all callers route
through it (see "Callers" below), the prediction helpers become headless-only:

- `region_predictable`
- `active_layout_renders_region`
- `region_render_source_customized`
- The "case 3a/3b" fallback maze around the seeded default `root-layout`.

Either keep them gated behind a `#[cfg(test)]` "headless-only" comment, or
delete and let the headless callers fail loudly when live geometry isn't
installed. The deletion direction is cleaner — every test that exercises UI
should install live geometry anyway.

## Callers to verify

Search hits for `focusable_rendered_block_ids`:

```
$ rg -n "focusable_rendered_block_ids" crates/holon-integration-tests/
```

Last audit (handoff time): `transitions/navigate_focus.rs`, `transitions/click_block.rs`,
`reference_state.rs` (definition + a small number of nav/focus helpers).
Each caller currently treats an empty Vec as "skip generation". With the
live-geometry branch swapping behaviour, all callers will continue to work
correctly without code changes — they just get accurate truth.

## Open questions

1. **Right Sidebar pinning.** `expected_focus_root_ids(RightSidebar)` is grown
   by `PinBlock`. The live-geometry path naturally tracks that, but verify by
   running a PBT seed that includes `PinBlock` and inspecting the sidebar
   widget tree.
2. **Bounds zero / hidden elements.** If `BoundsRegistry` records an element
   with zero bounds (e.g. drawer collapsed to width 0 but still mounted),
   should `rendered_entity_ids_in_panel` include it? Current proposal: yes
   (it's in the registry). Consider adding an `info.area() > 0` filter if a
   PBT step finds a false positive.
3. **Bounds staging vs commit.** The PBT already uses
   `wait_for_element_bounds` to handle staged-vs-committed promotion. If the
   generator runs against a staged-but-not-committed snapshot, it may briefly
   under-report. Verify by inspecting `BoundsRegistry::begin_pass` / `flush`
   semantics at `frontends/gpui/src/geometry.rs:96-122`.
4. **TUI / Flutter parity.** The other frontends don't yet provide live
   geometry. They go through the headless fallback automatically. Track in
   `frontends/tui/TODO.md` once GPUI lands.

## Session breadcrumbs (state of the world right now)

Working copy has (un-jj'd) changes for:
- `rendered_text` widget (this session, GPUI + shadow + view_model).
- `block_profile.yaml` `default` → `rendered_text`.
- `render_entity.rs` is_focused click wrapper deleted.
- PBT invariant filters extended for `rendered_text`.
- `navigate_focus.rs` migrated to `Validated<…, Reason>` API (by user).
- `phased.rs::run_pbt_with_driver_sync_callback` has a Drop-guard print of the
  rejection histogram (this session — survives panic).

Bugs found via histogram (`/tmp/gpui_pbt_reason2.log`):
- `NavigateFocus: 50 × SidebarFocusNotRendered` — the immediate driver of this
  follow-up. Option A (in-progress) bypasses the gate; B kills the underlying
  prediction.
- `FocusEditableText: 50 × NoFocusableBlocks, 31 × NoFocusInMain` — downstream
  consequence, resolved once NavigateFocus fires.

Bug NOT caused by this work, observed on first reason-instrumented run:
- `sut.rs:1534` — `query_and_watch 0 timed out after 12.78s` (Turso scheduler
  `mark_available()` not called for `blocks` table). Hit deterministically on
  some PBT seeds (e.g. seed=1 first run). The Drop guard added this session
  makes the rejection histogram survive this panic so debugging continues.
  File separate.

Click-to-focus bug found in `rendered_text.rs` (this session, fixed):
- Click handler passed full URI `"block:journals"` as `block_id` param to
  `navigation.editor_focus`, but `editor_cursor.block_id` joins `block.id`
  which is stored unprefixed. Fixed by calling `.id().to_string()` on the
  parsed `EntityUri` before passing it. PBT can't yet catch this regression
  for reasons documented above.
