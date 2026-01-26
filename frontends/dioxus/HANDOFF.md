# Dioxus Desktop Frontend — Handoff

## Current State (2026-06-18 — ported forward to the ViewModel architecture)

The Dioxus Desktop frontend was bit-rotted against `holon-frontend` (it still used the
retired `RenderInterpreter` / `BuilderArgs` / `RenderContext.session()` path). It has been
**ported forward to the modern `ViewModel` / `node_dispatch` render architecture** — the same
one `frontends/gpui` and `frontends/dioxus-web` use.

`cargo check` is **clean (0 errors, 0 warnings)**. See "Build & verify" for the recipe (the
crate is excluded from the default workspace, so `-p` needs a temporary member swap).

### Architecture (current)

Render pipeline:

```
ReactiveEngine::watch(&uri)  ->  Stream<ReactiveViewModel>
  -> rvm.snapshot()          ->  ViewModel              (serializable static tree)
  -> RenderNode { node }     ->  render_node(&ViewModel, &DioxusRenderContext)
  -> match node.widget_name() ->  <widget>::render(node, ctx) -> Element  (rsx! HTML/CSS)
```

- The macro `holon_macros::builder_registry!(… node_dispatch: Element, node_type: ViewModel,
  empty: rsx!{} …)` in `render/builders/mod.rs` generates `render_node`, dispatching on
  `node.widget_name()` to one builder file per widget.
- Every builder is `pub fn render(node: &ViewModel, ctx: &DioxusRenderContext) -> Element`.
  It destructures its own `ViewKind` variant from `&node.kind` (e.g.
  `let ViewKind::Text { content, bold, color, .. } = &node.kind else { return rsx!{} };`)
  and emits `rsx!` HTML/CSS. The webview (wry) renders the HTML.
- `DioxusRenderContext` is intentionally empty. Interactive builders pull the session/engine
  from Dioxus context (`use_context`), injected at launch via `LaunchBuilder::with_context`.

Bootstrap (`main.rs`):
- `DioxusModule` composes `CoreInfraModule` + `holon_app::HolonFrontendModule`, then wires the
  shared render interpreter via `set_render_interpreter(make_interpret_fn(slot))` — this is
  what makes `ReactiveEngine` produce non-empty `ViewModel`s. `on_start` populates the
  `BuilderServicesSlot` with the live engine (mirrors `GpuiModule`).
- `main()` resolves `FrontendSession` + `ReactiveEngine`, injects both (+ the tokio
  `Handle`) into Dioxus context, then launches the desktop webview window.
- `App()` bridges the tokio `engine.watch` stream (Send) into a dioxus `Signal<ViewModel>`
  (!Send) through a `tokio::sync::watch` channel, and renders `RenderNode { node }`.
  Cmd+Z / Cmd+Shift+Z undo/redo are preserved.

### Interactive builders (in-process, no wasm bridge)
- `render/builders/live_block.rs` — `LiveBlockNode` owns its own in-process
  `ReactiveEngine::watch(&uri)` subscription and provides `EntityContext` to descendants.
- `render/builders/editable_text.rs` + `src/editor.rs` — `EditorCell` is a native `<input>`
  that commits on blur (`onchange`) by dispatching `block.update` via
  `FrontendSession::execute_operation` on the injected tokio `Handle`.
- `render/builders/dispatch.rs` — shared helper. `dispatch_intent(rt, session, intent)`
  fires an `OperationIntent` onto the tokio runtime (mirrors gpui's
  `BuilderServices::dispatch_intent`); `click_modifiers(Modifiers)` maps a dioxus
  modifier set to Holon's `ClickModifiers` (cmd == META). Skipped by the builder macro.
- `render/builders/selectable.rs` — `SelectableNode` pre-resolves every
  modifier-bound intent on the node into a `HashMap<ClickModifiers, OperationIntent>`
  and dispatches the match on `onmousedown` (modifier clicks `stop_propagation`).
  Mirrors gpui `selectable.rs`. Passthrough when the node has no bound operations.
- `render/builders/block_operations.rs` — `BlockOperationsNode` renders a `[...]`
  affordance that dispatches the first block-mutating op (`find_ops_affecting` over
  `parent_id`/`sort_key`/`depth`/`content`, via `OperationIntent::for_row`) on click.
  Renders nothing when no dispatchable op exists. Mirrors gpui `block_operations.rs`.
- `render/builders/state_toggle.rs` — `StateToggleNode` cycles a task-state badge
  (TODO → DOING → DONE → …) on click via `cycle_state` + `OperationIntent::set_field`
  (`find_set_field_op`). Falls back to a static badge when no `set_field` op is wired.
  Mirrors gpui `state_toggle.rs`.

### Theming
`BASE_CSS` (injected via `with_custom_head`) defines the dark-theme CSS custom properties
(`--bg`, `--surface`, `--accent`, …). Builders reference `var(--token)`. Unchanged from before.

## Build & verify

The crate is in the root `[workspace].exclude` list — `dioxus-desktop` pulls `cocoa 0.26.1`
which conflicts with `gpui`'s pinned `cocoa =0.26.0` during unified workspace resolution. To
check or run it, temporarily resolve it in isolation:

1. In the root `Cargo.toml`, move `"frontends/dioxus"` from `exclude` into `members`, and
   remove `"frontends/gpui"` from `members`.
2. In `workspace-hack/Cargo.toml`, comment out the two `gpui = { git … }` lines (workspace-hack
   otherwise pulls gpui's cocoa transitively).
3. `cargo check -p holon-dioxus`  (0 errors / 0 warnings).
4. Revert steps 1–2 before landing (keep dioxus excluded).

Run (after the same swap):
```sh
HOLON_DB_PATH=/path/to/db HOLON_VAULT_ROOT=/path/to/orgfiles/ cargo run -p holon-dioxus
```

## Shared-crate change

`crates/holon-macros/src/builder_registry.rs` gained an optional `empty:` parameter for
`node_dispatch` mode. The empty/None match arm was previously hardcoded to
`gpui::div().into_any_element()` (only gpui exercised it); that arm is now configurable and
**defaults to the gpui expression**, so gpui is unaffected. Dioxus passes `empty: rsx!{}`.

## Remaining work (not blocking compile)

1. **Runtime validation** — `cargo check` is green but the app has not been launched against a
   real vault in this pass. Smoke-test rendering, live_block updates, and editable_text commits.
2. **Editor cursor preservation** — `EditorCell` is a commit-on-blur `<input>`; it does not
   preserve caret position across external re-renders the way the (wasm) dioxus-web editor does.
3. **Operation dispatch from display builders** — `selectable` and `block_operations` are
   now wired (see "Interactive builders" above): they pull `Arc<FrontendSession>` +
   `tokio::Handle` from Dioxus context — like `editable_text` — and dispatch through the
   shared `dispatch::dispatch_intent`, so no dispatch handle on `DioxusRenderContext` was
   needed. `state_toggle` is wired the same way (cycles task state on click). Still
   passthrough/no-op: `draggable` (needs HTML5 drag events) and `pie_menu` (needs a
   menu-open toggle + positioned popover) — heavier UI; mirror gpui's `draggable.rs` /
   `pie_menu.rs` when picking these up.
4. **Sidebar toggle, icons (SVG), MCP server, light mode** — same low-priority items as before.

## Reference implementations
- **dioxus-web** (`frontends/dioxus-web/`) — same Dioxus `Element` output; the closest sibling.
  NOTE: its builders are currently on the *older* destructured-field `render(field, …)`
  signature and do not compile against the current macro — this desktop port is now ahead of it.
- **gpui** (`frontends/gpui/`) — the newest/most-complete `node_dispatch` consumer; the
  reference for bootstrap (`di.rs` `GpuiModule`) and click→operation dispatch.
