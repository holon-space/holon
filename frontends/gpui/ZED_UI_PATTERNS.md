# GPUI Rendering Patterns (from Zed)

Reference guide for applying GPUI patterns to the Holon GPUI frontend.
All file links are relative to the Zed codebase root (i.e., `crates/gpui/...`).

---

## Table of Contents

1. [Element Lifecycle](#1-element-lifecycle)
2. [Core Abstractions](#2-core-abstractions)
3. [State Management](#3-state-management)
4. [Invalidation & Re-rendering](#4-invalidation--re-rendering)
5. [View Caching](#5-view-caching)
6. [Lists & Virtual Scrolling](#6-lists--virtual-scrolling)
7. [Deferred Drawing & Overlays](#7-deferred-drawing--overlays)
8. [Async & Spawning](#8-async--spawning)
9. [Element Composition Patterns](#9-element-composition-patterns)
10. [Actions & Keybindings](#10-actions--keybindings)
11. [Scene & GPU Backend](#11-scene--gpu-backend)
12. [Anti-Patterns](#12-anti-patterns)
13. [Holon-Specific Mapping](#13-holon-specific-mapping)
14. [Testing GPUI UI (Zed patterns)](#14-testing-gpui-ui-zed-patterns)

---

## 1. Element Lifecycle

Every element goes through three phases per frame:

```
request_layout  →  prepaint  →  paint
  (Taffy)         (hitboxes)    (scene primitives)
```

**Source**: `crates/gpui/src/element.rs:51-100`

```rust
pub trait Element: 'static + IntoElement {
    type RequestLayoutState;    // state from layout → prepaint
    type PrepaintState;         // state from prepaint → paint

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState);

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState;

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    );
}
```

**What happens in each phase:**

| Phase | Work | Thread safety |
|-------|------|---------------|
| `request_layout` | Calls Taffy layout engine, returns `LayoutId`. No painting. | Main thread |
| `prepaint` | Bounds are resolved. Register hitboxes, build dispatch tree nodes. Cache state for paint. | Main thread |
| `paint` | Emit primitives (quads, shadows, paths, sprites) to the `Scene`. Register mouse/key listeners. | Main thread |

**Key insight**: State computed in `request_layout` is passed to `prepaint`, which passes state to `paint`. This avoids recomputation and lets GPUI cache entire phases.

---

## 2. Core Abstractions

### Render (stateful views)

`crates/gpui/src/element.rs:131`

```rust
pub trait Render: 'static + Sized {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement;
}
```

Views implement `Render`. They have **identity across frames** (via `Entity<T>`), and their output is cached.

### RenderOnce (stateless components)

`crates/gpui/src/element.rs:147`

```rust
pub trait RenderOnce: 'static {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement;
}
```

Components are consumed on render (take `self`). No identity, no caching. Use for pure layout recipes.

### Entity<T> (handle to model/view)

`crates/gpui/src/app/entity_map.rs:413`

Entities are stored in a `SlotMap` keyed by `EntityId`. Single-threaded borrowing via `RefCell<T>`.

```rust
// Read
let value = entity.read(cx);           // → &T

// Write (triggers potential re-render)
entity.update(cx, |state, cx| {
    state.some_field = new_value;
    cx.notify();  // mark dirty
});
```

### AnyView (type-erased view with caching)

`crates/gpui/src/view.rs:22-30`

`AnyView` wraps `Entity<V>` and implements `Element` with **caching logic**. It can reuse the previous frame's layout/paint if:
- Bounds, content mask, and text style haven't changed
- `cx.notify()` was NOT called on the view
- `window.refresh()` is NOT active

---

## 3. State Management

### Entity (model) + notify

`crates/gpui/src/app/context.rs:229`

```rust
// Inside a Context<T>:
cx.notify();  // marks THIS entity dirty, queues re-render
```

### observe (react to entity changes)

`crates/gpui/src/app/context.rs:63`

```rust
cx.observe(&other_entity, |this, observed, cx| {
    // `observed` changed — react
    cx.notify();  // propagate to our view
});
```

Auto-downgrades to `WeakEntity` internally to prevent reference cycles.

### observe_global (react to global state)

`crates/gpui/src/app/context.rs:176`

```rust
cx.observe_global::<FileIcons>(|this, cx| {
    cx.notify();  // re-render when icons change
});
```

**Real usage** (`crates/outline_panel/src/outline_panel.rs:775`):
```rust
cx.observe_global::<FileIcons>(|_, cx| { cx.notify(); });
```

### subscribe (typed events)

`crates/gpui/src/app/context.rs:98` / `crates/gpui/src/gpui.rs:237`

```rust
// Declare emitter:
impl EventEmitter<DismissEvent> for ContextMenu {}

// Subscribe:
let subscription = cx.subscribe(&context_menu, |this, _, _: &DismissEvent, cx| {
    this.context_menu.take();
    cx.notify();
});
// MUST store the subscription — dropped sub = unsubscribed
self.context_menu = Some((context_menu, position, subscription));
```

**With window context** (`crates/gpui/src/app/context.rs:355`):
```rust
cx.subscribe_in(&pane, window, |this, _, event, window, cx| {
    // has access to `window` too
});
```

### Global state

`crates/gpui/src/app.rs:1640,1680` / `crates/gpui/src/global.rs:22`

```rust
impl Global for MyGlobal {}
cx.set_global(MyGlobal { ... });
let g = cx.global::<MyGlobal>();
```

---

## 4. Invalidation & Re-rendering

### How GPUI decides what to re-render

`crates/gpui/src/window.rs:107,927,1489-1496`

1. `cx.notify()` on an entity → inserts its `EntityId` into `dirty_views: FxHashSet<EntityId>`
2. **Ancestor propagation**: all ancestor views in the element tree are also marked dirty
3. On next frame, `Window::draw()` (`window.rs:2170`) runs the full element lifecycle
4. During `AnyView::prepaint`, if the view is NOT in `dirty_views` AND bounds/mask/style match, **skip** and replay from cache

### notify vs refresh

```rust
cx.notify();       // Mark ONE entity dirty. Cheapest. Use this.
window.refresh();  // Force full window repaint. Use sparingly.
```

`window.refresh()` (`window.rs:1549`) is a sledgehammer — it redraws everything. Prefer `cx.notify()` on specific entities.

### on_next_frame

`crates/gpui/src/window.rs:1839`

```rust
window.on_next_frame(|window, cx| {
    // runs once at the start of the next frame
});
```

Useful for coalescing multiple state changes into a single render pass.

---

## 5. View Caching

`crates/gpui/src/view.rs:22-30`

```rust
struct ViewCacheKey {
    bounds: Bounds<Pixels>,
    content_mask: ContentMask<Pixels>,
    text_style: TextStyle,
}
```

When `AnyView` is rendered:
1. Compare current `ViewCacheKey` to previous frame's key
2. If key matches AND view not in `dirty_views`:
   - Call `reuse_prepaint(range)` — replay hitboxes, tooltips, dispatch tree from previous frame
   - Call `reuse_paint(range)` — replay scene operations, mouse listeners, input handlers
3. If key differs OR view is dirty → full `render()` → `request_layout()` → `prepaint()` → `paint()`

**`PrepaintStateIndex`** (`window.rs:761`) and **`PaintIndex`** (`window.rs:771`) are range-based snapshots into the frame's state vectors, enabling efficient replay.

**Implication for Holon**: Structure the element tree so that independent UI regions are separate `Entity<View>` instances. When data changes in one region, only that view re-renders.

### Empirical findings from Holon (2026-04-06)

Validated via `frontends/gpui/examples/refresh_cascade.rs` Panels A-M.

#### `size_full()` vs `min_h_0()` for height propagation

`min_h_0()` does **NOT** propagate definite height through a flex chain. Only `size_full()` (= `width: 100%; height: 100%`) works. This was confirmed by Panel L (broken with `min_h_0`) vs Panel G (working with `size_full`).

**Rule**: Every intermediate `div` between the root flex container and `uniform_list` must have BOTH `flex_1()` AND `size_full()`.

#### `.cached(style)` requires `height: 100%` in the style

When wrapping an `Entity<View>` with `AnyView::from(entity).cached(style)`, the `style` must include `size.height = Some(relative(1.0).into())` for the cached view to participate in flex height distribution. Using `min_size.height = Some(px(0.0).into())` causes the cached view to render with zero height.

**Panel F** (working): `cached(style)` with `flex_grow=1, width=100%, height=100%`
**Panel M** (broken): `cached(style)` with `flex_grow=1, width=100%, min_height=0`

#### `w_full()` needed on flex_col children for editor width

When elements are rendered inside a `flex_col` container (e.g., `CollectionView::wrap_items()`), child rows need explicit `w_full()` for their descendant editors to get definite width. Without it, editor `TextInput` elements get squeezed to content width (~24px). This was confirmed via the GPUI inspector showing `BOUNDS size: 24×36, CONTENT size: 0×20` on input elements.

Affected builders that needed `w_full()`:
- `collection_view.rs` — `wrap_items()` container divs (Table, Tree, Outline, List variants)
- `tree_item.rs` — tree item row div
- `row.rs` — row builder div

#### `overflow_y_scroll` kills `uniform_list`

`overflow_y_scroll` on ANY ancestor gives `uniform_list` unconstrained height, causing it to render zero items. The scroll must be removed from the ancestor and handled by the collection itself (either via `uniform_list`'s built-in scroll or a dedicated scroll container for the non-virtual path).

#### `.cached()` works fine with async content — earlier blank was a layout bug

Initially `.cached()` on `BlockRefView` appeared to prevent re-rendering after async content arrived. This was misdiagnosed as a cache invalidation bug. The actual cause was the layout chain: intermediate divs used `min_h_0()` instead of `size_full()`, giving the cached view zero height. With the correct layout chain (`size_full()` + `flex_1()` on every intermediate div), `.cached()` works correctly — `cx.notify()` from async spawns properly marks the entity dirty and the cache wrapper re-renders.

**Result**: Idle render cascade dropped from ~6000 to 0 EditorView/5s with `.cached()` on BlockRefView.

---

## 6. Lists & Virtual Scrolling

### uniform_list (same-height items — fast path)

`crates/gpui/src/elements/uniform_list.rs:22`

Measures ONE item, computes all positions linearly. Only renders visible range.

```rust
uniform_list(
    "entries",                    // element ID
    items_len,                    // total item count
    cx.processor(move |this, range: Range<usize>, window, cx| {
        // Called with the visible range only
        this.entries[range.clone()]
            .iter()
            .map(|entry| this.render_entry(entry, window, cx))
            .collect()
    }),
)
.with_sizing_behavior(ListSizingBehavior::Infer)
.with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
.with_width_from_item(self.max_width_item_index)
.track_scroll(&self.scroll_handle)
```

**Real usage**: `crates/outline_panel/src/outline_panel.rs:4697-4763`

**Key points**:
- `cx.processor()` (`app/context.rs:264`) wraps a closure that borrows `&mut self` — GPUI handles the borrow lifetime
- `.track_scroll()` syncs with a `ScrollHandle` for programmatic scrolling
- `.with_width_from_item()` sets horizontal extent from a specific item (useful for variable-width content)

### list (variable-height items)

`crates/gpui/src/elements/list.rs:54`

Uses a `SumTree` for O(log n) height lookups. Only renders visible items + overdraw buffer.

```rust
let list_state = ListState::new(
    item_count,
    ListAlignment::Top,  // or ::Bottom for chat-style
    px(overdraw),
    move |ix, window, cx| render_item(ix, window, cx),
);
list(list_state).into_any_element()
```

### When to use which

| Scenario | Use |
|----------|-----|
| Block outline (same-height rows) | `uniform_list` |
| Table with variable row heights | `list` |
| Small fixed list (<50 items) | Just `.children(items)` |

---

## 7. Deferred Drawing & Overlays

`crates/gpui/src/elements/deferred.rs:7-16`

```rust
deferred(
    anchored()
        .position(menu_position)
        .anchor(gpui::Corner::TopLeft)
        .child(context_menu.clone()),
)
.with_priority(1)
```

**Real usage**: `crates/outline_panel/src/outline_panel.rs:4827-4834`

**How it works** (`crates/gpui/src/window.rs:723-750,2303`):
1. During normal paint, `deferred()` captures the element + rendering context (view, element ID stack, text style, content mask) into a `DeferredDraw`
2. After all normal elements paint, deferred draws are sorted by `priority` (higher = later = on top)
3. Each deferred draw replays its context and paints the element

**`anchored()`** (`crates/gpui/src/elements/anchored.rs:16-27`): positions an element absolutely at a given point, optionally anchored from a corner.

**Use in Holon for**: command palette, pie menus, context menus, tooltips.

---

## 8. Async & Spawning

### cx.spawn (foreground async)

`crates/gpui/src/app/context.rs:237`

```rust
cx.spawn(async move |this, cx| {
    // `this` is WeakEntity — auto-weak to prevent cycles
    let data = fetch_data().await;
    this.update(cx, |state, cx| {
        state.data = data;
        cx.notify();
    }).log_err();
})
```

### cx.spawn_in (foreground with window access)

`crates/gpui/src/app/context.rs:661`

```rust
cx.spawn_in(window, async move |this, cx| {
    loop {
        cx.background_executor().timer(Duration::from_millis(16)).await;
        this.update(cx, |state, cx| {
            state.scroll_offset += adjustment;
            cx.notify();
        }).ok();
    }
})
```

**Real usage** — debounced update (`crates/project_panel/src/project_panel.rs:711-720`):
```rust
this.diagnostic_summary_update = cx.spawn(async move |this, cx| {
    cx.background_executor().timer(Duration::from_millis(30)).await;
    this.update(cx, |this, cx| {
        this.update_diagnostics(cx);
        cx.notify();
    }).log_err();
});
```

### Critical rules

1. **Store or detach**: `Task<T>` cancels on drop. Either store it in a field or call `.detach()`
2. **Use WeakEntity in closures**: `cx.spawn()` gives you a `WeakEntity` — safe for long-lived tasks
3. **Don't block**: Never call `.block_on()` inside spawn — use `.await`
4. **Background work**: Use `cx.background_executor().spawn(|| heavy_computation)` for CPU-bound work, then `.await` the result in a foreground task

### cx.listener and cx.processor

`crates/gpui/src/app/context.rs:252,264`

```rust
// cx.listener: for event handlers that need &mut Self
.on_click(cx.listener(|this, event, window, cx| {
    this.handle_click(event, window, cx);
}))

// cx.processor: for callbacks that return values (list item renderers)
uniform_list("items", count, cx.processor(|this, range, window, cx| {
    this.render_items(range, window, cx)
}))
```

Both handle the `Entity<Self>` borrow internally — you get `&mut self` in the closure.

---

## 9. Element Composition Patterns

### Fluent builder API

`crates/gpui/src/util.rs:11` (FluentBuilder), `crates/gpui/src/styled.rs:22` (Styled)

Every element uses a Tailwind-like fluent API:

```rust
div()
    .id("my-element")          // element identity (required for stateful interactions)
    .flex()                     // display: flex
    .flex_col()                 // flex-direction: column
    .gap_2()                    // gap: 8px (spacing scale)
    .p_4()                      // padding: 16px
    .bg(cx.theme().colors().background)
    .rounded_lg()
    .border_1()
    .border_color(border_color)
    .shadow_md()
    .overflow_hidden()
    .child(header)
    .child(content)
    .children(footer_items)
```

### Conditional rendering

```rust
// .when() — conditional modifier
div()
    .when(is_selected, |this| this.bg(selected_color))
    .when(!is_selected, |this| this.hover(|s| s.bg(hover_color)))

// .when_some() — Option-based conditional
div()
    .when_some(icon_path, |this, path| {
        this.child(Icon::from_path(path))
    })

// .map() — arbitrary transformation
div()
    .map(|this| {
        if count > 1 {
            this.child(Label::new(format!("{count} items")))
        } else {
            this.child(Label::new(name))
        }
    })
```

### Layout helpers

```rust
v_flex()    // div().flex().flex_col()
h_flex()    // div().flex().flex_row()
```

### Interactive elements

`crates/gpui/src/elements/div.rs:642` (InteractiveElement), `:1123` (StatefulInteractiveElement)

```rust
div()
    .id("clickable")           // REQUIRED for stateful interactions
    .cursor_pointer()
    .on_click(cx.listener(|this, event, window, cx| { ... }))
    .on_mouse_down(MouseButton::Left, |event, window, cx| { ... })
    .on_mouse_move(|event, window, cx| {
        if event.dragging() { window.start_window_move(); }
    })
    .hover(|style| style.bg(hover_bg))
    .active(|style| style.bg(active_bg))
    .on_drag_move::<DragPayload>(cx.listener(|this, event, window, cx| { ... }))
    .on_drop(cx.listener(|this, payload: &DragPayload, window, cx| { ... }))
    .track_focus(&self.focus_handle)
    .key_context(self.dispatch_context(window, cx))
    .on_action(cx.listener(Self::open_entry))
```

### ParentElement

`crates/gpui/src/element.rs:156`

```rust
pub trait ParentElement {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>);

    fn child(mut self, child: impl IntoElement) -> Self { ... }
    fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self { ... }
}
```

### Extracting render methods

Zed consistently extracts complex subtrees into methods:

```rust
impl Render for OutlinePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("outline-panel")
            .size_full()
            .child(self.render_header(cx))
            .child(self.render_main_contents(window, cx))
            .child(self.render_footer(cx))
    }
}

impl OutlinePanel {
    fn render_header(&self, cx: &Context<Self>) -> impl IntoElement { ... }
    fn render_main_contents(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement { ... }
    fn render_footer(&self, cx: &Context<Self>) -> impl IntoElement { ... }
}
```

---

## 10. Actions & Keybindings

`crates/gpui/src/action.rs:24,117`

```rust
// Define actions (zero-cost type-safe commands):
actions!(outline_panel, [ExpandEntry, CollapseEntry, SelectNext, SelectPrev]);

// Register handlers:
div()
    .on_action(cx.listener(Self::expand_entry))
    .on_action(cx.listener(Self::collapse_entry))

// Dispatch from code:
window.dispatch_action(ExpandEntry.boxed_clone(), cx);
```

Actions propagate through the focus tree (dispatch tree). Keybindings map key sequences → actions via JSON keymap files. This is relevant for Holon's operation system.

---

## 11. Scene & GPU Backend

### Scene structure

`crates/gpui/src/scene.rs:27`

```rust
pub struct Scene {
    pub paint_operations: Vec<PaintOperation>,
    pub shadows: Vec<Shadow>,
    pub quads: Vec<Quad>,
    pub paths: Vec<Path<ScaledPixels>>,
    pub underlines: Vec<Underline>,
    pub monochrome_sprites: Vec<MonochromeSprite>,
    pub subpixel_sprites: Vec<SubpixelSprite>,
    pub polychrome_sprites: Vec<PolychromeSprite>,
    pub surfaces: Vec<PaintSurface>,
}
```

### Batching

`crates/gpui/src/scene.rs:158`

`Scene::batches()` produces an iterator of `PrimitiveBatch` — consecutive same-type primitives sorted by `DrawOrder`. The GPU backend issues one draw call per batch.

### Paint operation flow

1. Elements call `window.paint_quad(...)`, `window.paint_shadow(...)`, etc. during `paint()`
2. These push primitives into the current `Scene`
3. After all painting, `Scene::finish()` sorts by draw order
4. Backend iterates `batches()`, issues GPU draw calls

### Layers

```rust
window.push_layer(bounds);
// ... paint children ...
window.pop_layer();
```

Layers establish clipping regions and Z-ordering. Each layer gets a new `DrawOrder` scope.

---

## 12. Anti-Patterns

### Things that hurt GPUI performance

1. **Calling `window.refresh()` instead of `cx.notify()`** — forces full repaint of every view
2. **One giant Render impl** — nothing can be cached independently. Split into sub-views with their own `Entity<T>`
3. **Allocating in `render()`** — render is called every frame for dirty views. Pre-compute expensive data in model updates, not in render
4. **Not storing Task handles** — dropped tasks get cancelled silently
5. **Using `Arc<AtomicBool>` for cross-entity communication** — use `cx.observe()` / `cx.subscribe()` instead to get automatic dirty tracking
6. **Blocking the main thread** — all GPUI rendering is single-threaded. Long computations must go to `cx.background_executor()`
7. **Not using `.id()` on interactive elements** — required for hover states, focus, drag, scroll tracking. Missing IDs cause GPUI to lose state between frames.
8. **Rebuilding entire lists** — use `uniform_list` or `list` for >50 items

---

## 13. Holon-Specific Mapping

### Current architecture

Holon's GPUI frontend (`frontends/gpui/src/lib.rs`) currently has:
- **One `HolonApp` view** that owns everything
- **`spec_dirty` flag** + `cached_display` for coarse-grained caching
- **`AtomicBool` bridge** between tokio CDC and GPUI's main thread
- Shadow interpreter produces a `ViewModel` tree, then `render::builders::render()` maps it to GPUI elements

### What to improve (applying Zed patterns)

| Current | Better (Zed pattern) | Why |
|---------|---------------------|-----|
| Single `HolonApp` renders everything | Split into sub-views per panel/region (`Entity<SidebarView>`, `Entity<MainView>`) | Enables per-view caching; sidebar doesn't re-render when main content changes |
| `Arc<AtomicBool>` for spec_dirty | `cx.observe()` on a shared `Model<AppState>` | Automatic dirty propagation, no manual flag management |
| `window.refresh()` on every CDC change | `cx.notify()` only on the affected view entity | Avoids full-window repaint |
| Flat list of children in outline/tree | `uniform_list` with range-based rendering | Only renders visible items; critical for large documents |
| Settings overlay as conditional child | `deferred(anchored(...))` | Proper Z-ordering, doesn't interfere with main layout |
| `tokio::spawn` + `AtomicBool` bridge | `cx.spawn()` directly for foreground async | GPUI-native, no AtomicBool polling loop needed |
| Everything in one `render()` method | Extract `render_titlebar()`, `render_sidebar()`, `render_content()` | Readability + each can become its own cached view later |

### Recommended sub-view split

```
HolonApp (root)
├── TitleBar (Entity<TitleBar>)          — rarely changes
├── ContentArea
│   ├── LeftSidebar (Entity<SidebarView>) — changes on navigation
│   ├── MainPanel (Entity<MainPanel>)     — changes on block edits
│   └── RightSidebar (Entity<SidebarView>)
└── Overlays (deferred)
    ├── CommandPalette
    ├── PieMenu
    └── SettingsPanel
```

Each `Entity<View>` gets its own caching. `cx.observe()` wires them to shared models. Only dirty views re-render.

### BlockRef → Entity pattern

Currently `block_ref` creates inline elements. The Zed pattern would be:

```rust
struct BlockView {
    block_id: EntityUri,
    content: ViewModel,
}

impl Render for BlockView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render just this block's ViewModel subtree
        render::builders::render(&self.content, &self.gpui_ctx)
    }
}
```

When block data changes, only that `BlockView` is notified and re-rendered. Parent layout stays cached.

### List virtualization for outlines

```rust
// In OutlineView::render():
uniform_list(
    "outline-items",
    self.visible_block_count,
    cx.processor(|this, range, window, cx| {
        this.blocks[range.clone()]
            .iter()
            .map(|block| this.render_block_item(block, window, cx))
            .collect()
    }),
)
.track_scroll(&self.scroll_handle)
```

### CDC → GPUI notification (recommended pattern)

Replace the current `AtomicBool` polling loop with:

```rust
// In Model<AppState>:
impl AppState {
    fn apply_cdc_update(&mut self, update: CdcUpdate, cx: &mut Context<Self>) {
        self.widget_spec = update.spec;
        cx.notify();  // propagates to all observers
    }
}

// In HolonApp::new():
cx.observe(&app_state_model, |this, state, cx| {
    // AppState changed — figure out which sub-view to notify
    this.sidebar.update(cx, |sidebar, cx| {
        sidebar.on_data_change(state.read(cx));
        cx.notify();
    });
});
```

---

## 14. Testing GPUI UI (Zed patterns)

Zed has a rich set of in-process UI test helpers in the `gpui` crate. None of these
require a real display server — they run the full element lifecycle
(`request_layout` → `prepaint` → `paint`) against a mocked `TestPlatform`, which
makes them millisecond-fast and headless-safe.

### 14.1 Entry points

| Helper | Source | What it buys you |
|---|---|---|
| `#[gpui::test]` macro | `crates/gpui/src/test.rs:70` (`run_test`) | Deterministic async runner, seeded scheduler, forbids real wall-clock sleeps |
| `TestAppContext` | `crates/gpui/src/app/test_context.rs:20` | `App`-level context with `TestDispatcher`; `add_window()` / `add_window_view()`, `run_until_parked()` |
| `VisualTestContext` | `crates/gpui/src/app/visual_test_context.rs` | Window-scoped context; acts as both `Window` and `App`; exposes `simulate_*`, `dispatch_action`, `draw()` |
| `HeadlessAppContext` | `crates/gpui/src/app/headless_app_context.rs:38` | Real `PlatformTextSystem` (accurate glyph bounds) + `TestDispatcher` — use when layout depends on real text shaping |
| `TestPlatform` | `crates/gpui/src/platform/test/platform.rs:20` | Mocked `Platform` / `TestDisplay` / `TestWindow`; no GPU; what all of the above use under the hood |

**Upgrade pattern** used in every complex Zed test:

```rust
#[gpui::test]
async fn test_outline(cx: &mut gpui::TestAppContext) {
    init_test(cx);
    let window = cx.add_window(|w, cx| MyView::new(w, cx));
    let view = window.read_with(cx, |v, _| v.clone());
    let cx = &mut VisualTestContext::from_window(window.into(), cx); // upgrade
    cx.run_until_parked();
    // ... assertions
}
```

`add_window_view()` renders off-screen but runs the **real** `Element` pipeline,
so layout and paint code paths are exercised.

### 14.2 Bounds assertions via `debug_selector`

`crates/gpui/src/elements/div.rs:800-816` — any `Div` can be tagged with a
debug selector, which registers its final bounds in the window's
`debug_bounds: FxHashMap<SharedString, Bounds<Pixels>>` (`window.rs:2029`)
during paint:

```rust
div()
    .debug_selector(|| format!("outline-item-{}", index))
    .child(...)
```

In tests:

```rust
let bounds = cx.debug_bounds("outline-item-3").expect("not rendered");
assert!(bounds.size.height > px(0.));
assert!(bounds.size.width  > px(0.));
cx.simulate_click(bounds.center(), Modifiers::default());
```

Zed uses this for drag-and-drop tests in `crates/workspace/src/pane.rs:6600-6700`
(tabs) and layout sanity in `project_panel` tests. It is the closest thing gpui
has to a built-in element-tree dump — walk all your selectors post-draw to
inspect what laid out where.

**Mapping to Holon**: we already have `BoundsRegistry` (populated on every
render in `render::builders`). That is our `debug_bounds` equivalent and is
*always on* — no opt-in `.debug_selector()` needed. Every rendered widget is
queryable out of the box, keyed by widget type / index. This is a strict
superset of Zed's facility and is the seam our fast test layer will use.

### 14.3 Input simulation

`crates/gpui/src/app/test_context.rs:726-809` and
`crates/gpui/src/app/visual_test_context.rs:222-340`:

```rust
cx.simulate_keystrokes(window, "cmd-p escape");
cx.simulate_input(window, "hello");
cx.simulate_click(position, Modifiers::default());
cx.dispatch_action(MyAction);   // bypasses keymap, dispatches into focus tree
```

These go through the **real** dispatch tree — focus handles, key contexts,
actions, `on_click` / `on_mouse_down` listeners. So you catch bugs that live in
event routing, not just handler logic. `run_until_parked()` after each event
flushes effects.

### 14.4 Virtual list testing (`uniform_list` / `list`)

From `crates/picker/src/picker.rs:963-1029` and
`crates/project_panel/src/project_panel_tests.rs:18-167`:

- Do **not** try to scroll virtual lists pixel-wise in unit tests. Instead,
  dispatch the **action** (`SelectNext`, `SelectPrevious`) and assert on the
  delegate's `selected_index` / `visible_entries`.
- For visual verification, query bounds of sentinel items via `debug_selector`
  after dispatching.
- `picker.rs:1008-1029` verifies `SelectNext` skips non-selectable items by
  dispatching in a loop and reading `selected_index()`.

### 14.5 Single-element `draw()` for isolated layout tests

`crates/gpui/src/app/test_context.rs:838-867`:

```rust
cx.draw(origin, available_space, |window, cx| {
    MyElement::new().into_any_element()
});
```

Runs `request_layout` → `prepaint` → `paint` for one element tree in a detached
window, returning resolved layout state. The narrowest possible fixture — use
it when you want to pinpoint *which container* causes a 0-height collapse.

### 14.6 Visual regression (when pixels are the spec)

`VisualTestAppContext` keeps a real macOS renderer but positions windows at
`(-10000, -10000)` so they render invisibly, then exposes
`cx.capture_screenshot(window) -> RgbaImage`. Slow; use for goldens of icons or
custom `paint()` code. For 95% of tests prefer bounds assertions or
layout-tree snapshots.

### 14.7 What Zed does *not* have

- **No built-in element/scene tree dump.** You build one by iterating every
  registered selector. Our `BoundsRegistry` already gives us
  `Vec<(WidgetId, BoundsInfo { widget_type, .. })>` for free.
- **No property-based UI tests.** Zed leans on hand-written fixtures. This is
  the layer we can add cheaply.
- **No Taffy-level layout tests.** Zed trusts Taffy.

---

## Key File Index

| File | What it defines |
|------|----------------|
| `crates/gpui/src/element.rs` | `Element`, `Render`, `RenderOnce`, `ParentElement` traits |
| `crates/gpui/src/view.rs` | `AnyView`, `ViewCacheKey`, cache reuse logic |
| `crates/gpui/src/window.rs` | `Window`, `DeferredDraw`, `PrepaintStateIndex`, `PaintIndex`, dirty_views, `draw()`, `refresh()` |
| `crates/gpui/src/app/context.rs` | `Context<T>`: `notify()`, `observe()`, `subscribe()`, `spawn()`, `listener()`, `processor()` |
| `crates/gpui/src/app/entity_map.rs` | `Entity<T>`, `WeakEntity<T>`, entity storage |
| `crates/gpui/src/app.rs` | `App`, `global()`, `set_global()` |
| `crates/gpui/src/scene.rs` | `Scene`, primitives, `batches()` |
| `crates/gpui/src/styled.rs` | `Styled` trait (Tailwind-like CSS methods) |
| `crates/gpui/src/elements/div.rs` | `Div`, `Interactivity`, `InteractiveElement`, `StatefulInteractiveElement` |
| `crates/gpui/src/elements/uniform_list.rs` | `uniform_list()` — fast virtual scrolling for same-height items |
| `crates/gpui/src/elements/list.rs` | `ListState`, `list()` — variable-height virtual scrolling |
| `crates/gpui/src/elements/deferred.rs` | `deferred()` — paint-after-everything overlays |
| `crates/gpui/src/elements/anchored.rs` | `anchored()` — absolute positioning for popovers |
| `crates/gpui/src/elements/canvas.rs` | `canvas()` — custom paint callback |
| `crates/gpui/src/action.rs` | `actions!` macro, `Action` trait |
| `crates/gpui/src/global.rs` | `Global` trait |
| `crates/gpui/src/util.rs` | `FluentBuilder` trait (`.when()`, `.when_some()`, `.map()`) |
| **Real-world examples** | |
| `crates/project_panel/src/project_panel.rs` | File tree: `Render`, events, drag & drop, context menus |
| `crates/outline_panel/src/outline_panel.rs` | Outline: `uniform_list`, `deferred`, `observe_global`, search |
| `crates/terminal_panel/src/terminal_panel.rs` | Terminal: `subscribe`, `observe`, async spawn |
