# Handoff: Enrich BoundsRegistry for GPUI PBT Invariants

## Context

The GPUI PBT test (`frontends/gpui/tests/gpui_ui_pbt.rs`) now has inv14 which checks
the frontend's ViewModel for Error widgets and queries BoundsRegistry for element existence.
However, BoundsRegistry currently stores **only pixel bounds** per element — no widget type,
no entity URI, no content info. This limits what we can assert about what GPUI actually rendered.

The goal is to enrich BoundsRegistry so that PBT invariants can verify not just "an element
exists with bounds" but also "the element is the right type, has the right entity, and has
non-trivial content."

## Current State

### BoundsRegistry (GPUI-specific)

**File**: `frontends/gpui/src/geometry.rs`

```rust
pub struct BoundsRegistry {
    inner: Arc<RwLock<HashMap<String, Bounds<Pixels>>>>,
}
```

Stores: `HashMap<element_id_string, Bounds<Pixels>>`.
Element IDs follow pattern: `"render-block-{EntityUri}"`.

### GeometryProvider trait (shared, in holon-frontend)

**File**: `crates/holon-frontend/src/geometry.rs`

```rust
pub trait GeometryProvider: Send + Sync {
    fn element_bounds(&self, id: &str) -> Option<ElementRect>;
    fn all_element_ids(&self) -> Vec<String>;
}
```

`ElementRect` has: `x, y, width, height` (f32) + `center()` method.

### Where `tracked()` is called (element registration sites)

| Builder file | Element type | Available data at call site |
|---|---|---|
| `render/builders/render_entity.rs:67` | render_entity | `block_id: EntityUri`, is_focused, click handler |
| `render/builders/editable_text.rs:19,53` | editable_text | `entity_id` from row data, text content |
| `render/builders/selectable.rs:57` | selectable | `entity_id`, selection action, label |

Each call site constructs `el_id = format!("render-block-{}", id)` and calls
`tracked(el_id, inner_element, &ctx.bounds_registry)`.

### Current inv14 bounds assertions (B1, B3, B4)

**File**: `crates/holon-integration-tests/src/pbt/sut.rs` (inv14 block)

- **B1**: `all_element_ids()` is non-empty
- **B3**: No element has zero-area bounds
- **B4**: Entity IDs from ViewModel have corresponding `render-block-{id}` in bounds (warning only)

## Proposed Enhancement

### Step 1: Introduce `ElementInfo` in `holon-frontend/src/geometry.rs`

Replace the bare `ElementRect` storage with a richer struct:

```rust
#[derive(Debug, Clone)]
pub struct ElementInfo {
    pub bounds: ElementRect,
    /// Widget type name: "render_entity", "editable_text", "selectable", "tree_item", etc.
    pub widget_type: String,
    /// The entity URI this element represents (if data-bound).
    pub entity_id: Option<String>,
    /// Whether this element has visible content (false for empty containers, error placeholders).
    pub has_content: bool,
}
```

Update `GeometryProvider` trait:

```rust
pub trait GeometryProvider: Send + Sync {
    fn element_bounds(&self, id: &str) -> Option<ElementRect>;
    fn element_info(&self, id: &str) -> Option<ElementInfo>;
    fn all_element_ids(&self) -> Vec<String>;
    fn all_elements(&self) -> Vec<(String, ElementInfo)>;
}
```

Keep `element_bounds` for backward compat (used by GeometryDriver for click simulation).
Add `element_info` and `all_elements` for richer assertions.

### Step 2: Update `BoundsRegistry` in `frontends/gpui/src/geometry.rs`

Change inner storage from `HashMap<String, Bounds<Pixels>>` to `HashMap<String, ElementInfo>`.
Update `tracked()` to accept metadata:

```rust
pub fn tracked(
    el_id: impl Into<String>,
    child: AnyElement,
    registry: &BoundsRegistry,
    widget_type: &str,
    entity_id: Option<&str>,
    has_content: bool,
) -> BoundsTracker { ... }
```

The `BoundsTracker::prepaint()` method records the full `ElementInfo` with bounds.

### Step 3: Update all `tracked()` call sites

| File | widget_type | entity_id | has_content |
|---|---|---|---|
| `render/builders/render_entity.rs:67` | `"render_entity"` | `Some(&id.to_string())` | `true` (has child) |
| `render/builders/editable_text.rs:19` | `"editable_text"` | entity from row data | `!text.is_empty()` |
| `render/builders/editable_text.rs:53` | `"editable_text"` | entity from row data | `!text.is_empty()` |
| `render/builders/selectable.rs:57` | `"selectable"` | entity from row data | `true` |

### Step 4: New invariants in inv14

With enriched BoundsRegistry, add these checks after the existing B1/B3/B4:

```
B5 (no error elements):
    For all elements in BoundsRegistry, assert widget_type != "error".
    This catches error widgets that GPUI rendered but the ViewModel didn't report
    (e.g., builder-level error handling that wraps content in error divs).

B6 (widget type consistency):
    For each entity_id in the ViewModel that has a corresponding BoundsRegistry entry,
    verify the widget_type is consistent. E.g., if ViewModel says it's an "editable_text",
    the BoundsRegistry entry should have widget_type="editable_text", not "render_entity".

B7 (content presence):
    When the reference model says blocks have content (non-empty content field),
    the corresponding BoundsRegistry entry should have has_content=true.
    Catches rendering bugs where content is lost during the builder pipeline.

B8 (entity coverage):
    Upgrade B4 from Warning to Error: every entity ID in the ViewModel's visible
    tree MUST have a BoundsRegistry entry (unless virtual scrolling is active).
    This proves GPUI actually rendered the block, not just that the engine produced it.
```

### Step 5: Also consider tracking these elements

Currently only `render_entity`, `editable_text`, and `selectable` are tracked.
Consider adding `tracked()` to:

- `tree_item` builder (important for tree layout assertions)
- `columns` builder root (to verify the three-column layout rendered)
- `block_ref` view (to verify nested block references rendered)

Each additional tracking point provides more assertion coverage.

## Key Files to Modify

| File | Change |
|---|---|
| `crates/holon-frontend/src/geometry.rs` | Add `ElementInfo`, extend `GeometryProvider` trait |
| `frontends/gpui/src/geometry.rs` | Store `ElementInfo`, update `tracked()` signature |
| `frontends/gpui/src/render/builders/render_entity.rs` | Pass metadata to `tracked()` |
| `frontends/gpui/src/render/builders/editable_text.rs` | Pass metadata to `tracked()` |
| `frontends/gpui/src/render/builders/selectable.rs` | Pass metadata to `tracked()` |
| `frontends/blinc/src/geometry.rs` | Update `BlincGeometry` to implement new trait methods |
| `crates/holon-integration-tests/src/pbt/sut.rs` | Add B5-B8 assertions in inv14 |
| `crates/holon-integration-tests/src/ui_driver.rs` | Update `GeometryDriver` if trait changes |

## Verification

1. Run headless PBT: `cargo nextest run -p holon-integration-tests general_e2e_pbt`
   - Should pass unchanged (no frontend_geometry set)

2. Run GPUI PBT: `HOLON_PERF_BUDGET=0 cargo test -p holon-gpui --test gpui_ui_pbt --features pbt -- --nocapture`
   - inv14 should print element info: widget types, entity IDs, content status
   - B5-B8 assertions should fire if there are rendering bugs

3. With MCP inspection: `PBT_MCP_PORT=8521` — compare `describe_ui` output against BoundsRegistry entries
