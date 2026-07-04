use super::prelude::*;
use crate::views::EditorView;

pub fn render(node: &holon_frontend::ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let content = node.prop_str("content").unwrap_or_default();
    let field = node.prop_str("field").unwrap_or_else(|| "content".to_string());

    let Some(row_id) = node.row_id() else {
        return static_fallback(&content, ctx);
    };

    // Suffix the occurrence coordinate (ADR 0016 §3) so a display-placed second
    // occurrence of a block gets its own editor identity. `Canonical` → empty
    // suffix → byte-identical key to before for every real row.
    let occ = node.occurrence().key_suffix();
    let el_id = format!("editable-text-{row_id}-{field}{occ}");
    let has_content = !content.is_empty();

    // The EditorView entity is parent-owned via `LocalEntityScope`'s
    // `EntityCache`: each `RenderEntityView` / `ReactiveShell` keeps its
    // own cache, so an editor lives exactly as long as the row that owns
    // it. When the row is removed (collection driver `RemoveAt`) the
    // cache drops with the parent and the editor's `Task<()>`s
    // (`_data_subscription`, `_cursor_subscription`) cancel naturally.
    //
    // Render never touches `InputState::set_value` for sync — see
    // `EditorView::new`'s `_data_subscription` for backend → InputState
    // propagation (gated on focus to avoid clobbering live typing).
    let operations = node.operations.clone();
    let triggers = node.triggers.clone();
    let services = ctx.services.clone();
    let nav = ctx.nav.clone();
    let data_handle = Some(node.data.clone());
    let el_id_for_create = el_id.clone();
    let row_id_for_create = row_id.clone();
    let content_for_create = content.clone();
    let field_for_create = field.clone();
    let bounds_registry_for_create = ctx.bounds_registry.clone();

    let key = crate::entity_view_registry::CacheKey::Ephemeral(el_id.clone());
    let any = ctx.local.get_or_create(key, || {
        ctx.with_gpui(|window, cx| {
            cx.new(|cx| {
                EditorView::new(
                    el_id_for_create,
                    content_for_create,
                    field_for_create,
                    row_id_for_create,
                    operations,
                    triggers,
                    services,
                    nav,
                    data_handle,
                    bounds_registry_for_create,
                    window,
                    cx,
                )
            })
            .into_any()
        })
    });
    let entity: gpui::Entity<EditorView> = any.downcast().expect("editable_text cache type mismatch");

    // Snapshot the live `InputState` value so PBT invariants can detect UI
    // staleness, and reconcile a stale editor against the live row content.
    //
    // The `EditorView`'s data-sync subscription (see `EditorView::new`) is
    // bound to the per-row `Mutable` cell that existed when the editor was
    // first cached. A structural change (split/join) or a navigation rebuilds
    // the `ReactiveRowSet` with *fresh* per-row cells, orphaning that
    // subscription: later external writes (peer edits, file reloads, the
    // post-split projection) land on the new cell and never reach the cached
    // editor's `InputState`. The shell still re-renders this builder on the
    // new cell's data signal, so `content` here is always the live value.
    // When the editor is NOT window-focused the user cannot be mid-typing, so
    // pushing the live content into a stale `InputState` is always safe — it
    // is the backstop that keeps the displayed/edited text converged with the
    // backend even after the event-driven subscription has been orphaned.
    let (displayed_text, is_window_focused): (std::sync::Arc<str>, bool) = ctx.with_gpui(|window, cx| {
        use gpui::Focusable;
        // Read phase: gather focus state, then DROP the `entity.read` borrow
        // before mutating the entity. `converge_input` takes `&mut self` (→
        // `entity.update`), which panics on a still-live `entity.read` borrow.
        let (input, is_focused, just_focused) = {
            let view = entity.read(cx);
            let input = view.input_entity().clone();
            let is_focused = input.focus_handle(cx).is_focused(window);
            // `just_focused` is the false→true window-focus edge (e.g. click-to-edit);
            // `just_blurred` is the true→false edge.
            let (just_focused, just_blurred) = view.focus_transition(is_focused);
            // On iOS/Android the platform focus-change events never reach the
            // editor's `InputEvent::Focus`/`Blur` subscription (confirmed via
            // MCP-driven clicks: the field focuses — caret renders — but no
            // `InputEvent` fires), so the soft keyboard was never raised on
            // focus. Drive it from the render-path focus edge, which is the
            // reliable mobile focus signal. `editor_focus_gained/lost` are
            // no-ops off `feature = "mobile"`.
            #[cfg(feature = "mobile")]
            {
                if just_focused {
                    crate::mobile::editor_focus_gained();
                } else if just_blurred {
                    crate::mobile::editor_focus_lost(cx);
                }
            }
            #[cfg(not(feature = "mobile"))]
            let _ = just_blurred;
            (input, is_focused, just_focused)
        };
        // Reconcile a stale `InputState` to the authority when the user cannot
        // be mid-typing: either the editor is unfocused, or focus *just*
        // arrived this frame (no keystroke yet). A continuously-focused editor
        // is left alone so in-flight typing is never yanked. `converge_input`
        // prefers the Loro cell authority over the SQL-lagged `content`
        // (curing the projection lag) and keeps `previous_text` in lockstep.
        if !is_focused || just_focused {
            entity.update(cx, |this, cx| {
                this.converge_input(&content, window, cx);
            });
        }
        // Snapshot the post-convergence value for the PBT staleness invariants.
        let displayed = input.read(cx).value().to_string();
        (std::sync::Arc::from(displayed.as_str()), is_focused)
    });
    let inner = entity.into_any_element();

    // Grey placeholder hint for empty editors — helps users discover that
    // typing into an empty block creates content. Rendered as an
    // absolutely-positioned BEHIND the real Input so it doesn't intercept
    // clicks or typing (GPUI hit-tests children in reverse paint order).
    let element = if !has_content {
        div()
            .relative()
            .child(
                div()
                    .absolute()
                    .top(px(4.0))
                    .left(px(0.0))
                    .text_color(gpui::Hsla {
                        h: 0.0,
                        s: 0.0,
                        l: 0.5,
                        a: 0.5,
                    })
                    .text_size(px(15.0))
                    .line_height(px(22.0))
                    .child("Type here to add a new block"),
            )
            .child(inner)
            .into_any_element()
    } else {
        inner
    };

    crate::geometry::tracked(
        el_id,
        element,
        &ctx.bounds_registry,
        "editable_text",
        Some(&row_id),
        has_content,
        Some(displayed_text),
    )
    .with_focused(is_window_focused)
    .into_any_element()
}

fn static_fallback(content: &str, ctx: &GpuiRenderContext) -> AnyElement {
    let text_color = tc(ctx, |t| t.foreground);
    let display_text = if content.is_empty() {
        "(empty)".to_string()
    } else {
        content.to_string()
    };

    div()
        .w_full()
        .min_h(px(26.0))
        .py(px(1.0))
        .text_color(text_color)
        .text_size(px(15.0))
        .line_height(px(22.0))
        .child(display_text)
        .into_any_element()
}
