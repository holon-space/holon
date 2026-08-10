use super::prelude::*;
use crate::views::EditorView;

pub fn render(node: &holon_frontend::ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let content = node.prop_str("content").unwrap_or_default();
    let field = node
        .prop_str("field")
        .unwrap_or_else(|| "content".to_string());

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
    let reseed_services = ctx.services.clone();
    // ALLOW(entity_uri_from_raw): render-spec row_id, schemed to match the
    // key the undo/redo dispatch arms its re-seed under.
    let reseed_row = holon_api::EntityUri::from_raw(&row_id);
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
    let entity: gpui::Entity<EditorView> =
        any.downcast().expect("editable_text cache type mismatch");

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
    // Increment G: this backstop runs ONLY for no-cell (unwired / headless)
    // editors — a cell-attached editor's `_remote_delta_subscription` over the
    // un-orphaned entity `Cell` makes it unnecessary, so it is gated off below.
    let (displayed_text, is_window_focused): (std::sync::Arc<str>, bool) =
        ctx.with_gpui(|window, cx| {
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
                    // Re-borrow via short-lived `entity.read(cx)` temporaries rather
                    // than the outer `view` binding: `editor_focus_lost` needs
                    // `&mut cx`, which cannot coexist with a live `entity.read(cx)`
                    // borrow (`view` is such a borrow).
                    if just_focused {
                        entity.read(cx).note_focus_gained_mobile();
                    } else if just_blurred {
                        let my_gen = entity.read(cx).focus_gen();
                        crate::mobile::editor_focus_lost(cx, my_gen);
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
            // (curing the projection lag) and keeps the VM buffer in lockstep.
            // Increment G — in the steady state a cell-attached editor converges
            // solely via its `_remote_delta_subscription` (the entity `Cell` is the
            // single external content source), so the render backstop stays OFF
            // there: `displayed_text` below reports the editor's actual live
            // `InputState` with no render-path patch-up, giving `inv-displayed-text/
            // widget` real teeth over the cell path. No-cell (unwired / headless)
            // editors keep the full backstop that cures their orphaned
            // `_data_subscription`.
            //
            // 2026-07-10 — but a cell-attached editor STILL re-reads the cell
            // authority on the focus-GAIN edge. A cached editor reused across a
            // split/join rowset rebuild can hold an `InputState` that never received
            // the structural `set_field` delta (the entity `Cell`'s broadcast
            // subscription can be starved / miss the write across the rebuild).
            // Trusting that stale buffer silently corrupts data: the next keystroke
            // commits the pre-join text — resurrecting merged-away content — and
            // `Enter` at its end splits past the canonical length ("Split position 18
            // exceeds content length 17"). Focus-gain is a safe convergence point
            // (no keystroke has landed → nothing to yank), and `converge_input`
            // reads ONLY the cell authority (`current_text()`) for a cell editor, so
            // this does NOT reintroduce the retired SQL `content` backstop. It is
            // idempotent — a no-op when the cell already delivered.
            let cell_attached = entity.read(cx).has_cell();
            // An undo/redo restored the store under a FOCUSED editor, which
            // every other convergence channel skips. Applied here and cleared
            // only once the restored text has actually arrived in `content`,
            // so a render that beats the projection leaves it armed for the
            // next one.
            let reseed = reseed_services.authority_reseed_armed(&reseed_row);
            let reseed_pending = reseed && input.read(cx).value() != content;
            if converge_on_render(cell_attached, is_focused, just_focused) || reseed_pending {
                let source = if reseed_pending {
                    "undo_reseed"
                } else if cell_attached {
                    "focus_reload"
                } else {
                    "render_backstop"
                };
                entity.update(cx, |this, cx| {
                    this.converge_input(source, &content, window, cx);
                });
            }
            if reseed_pending {
                reseed_services.consume_authority_reseed(&reseed_row);
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
    //
    // Visibility keys off the LIVE editor text (`displayed_text`), NOT the
    // committed `content` prop: the first keystroke into an empty block updates
    // the `InputState` immediately but does not commit to the projection until
    // later, so `has_content` (derived from `content`) would still be false and
    // the grey "Type here" hint would draw UNDER the freshly-typed glyph until
    // commit (dogfood 2026-07-19 PERCEPTION bug). `displayed_text` reflects the
    // keystroke this same frame, so the hint disappears the instant text lands.
    let show_placeholder = displayed_text.is_empty();
    let element = if show_placeholder {
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
                    .child("Type here"),
            )
            .child(inner)
            .into_any_element()
    } else {
        inner
    };

    // The editor spans its row. Under the tracked-widget contract (see
    // `crate::geometry`) that is the widget's own job, not the tracker's —
    // `flex_col` puts width on the cross axis so the child stretches to the
    // full row, matching `rendered_text`'s `w_full` read-mode counterpart.
    let element = div().w_full().flex_col().child(element).into_any_element();

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

/// Whether the render path should converge this editor's `InputState` to its
/// content authority this frame. See the call site for the full rationale.
///
/// - No-cell editor: converge whenever the user cannot be mid-typing — either
///   unfocused, or focus just arrived (no keystroke yet). Cures the orphaned
///   `_data_subscription`.
/// - Cell-attached editor: converge ONLY on the focus-gain edge (re-read the
///   cell authority), never in the focused/unfocused steady state — the entity
///   `Cell` remote-delta subscription owns the steady state, and gating the
///   backstop off there keeps `inv-displayed-text` teeth.
fn converge_on_render(cell_attached: bool, is_focused: bool, just_focused: bool) -> bool {
    if cell_attached {
        just_focused
    } else {
        !is_focused || just_focused
    }
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

#[cfg(test)]
mod tests {
    use super::converge_on_render;

    #[test]
    fn cell_editor_converges_only_on_focus_gain() {
        // Focus-gain edge: re-read the cell authority. This is the fix for the
        // stale-buffer data corruption after a split/join rowset rebuild
        // (2026-07-10): the cached editor's `InputState` may have missed the
        // structural `set_field` delta, so focus-gain must reload from the cell.
        assert!(converge_on_render(true, true, true));
        // Steady state (focused or unfocused, no edge): the entity `Cell`
        // remote-delta path owns it — the backstop stays off to keep
        // `inv-displayed-text` real teeth over the cell path.
        assert!(!converge_on_render(true, true, false));
        assert!(!converge_on_render(true, false, false));
    }

    #[test]
    fn no_cell_editor_keeps_full_backstop() {
        // Unfocused steady state and focus gain both converge (cure the orphaned
        // `_data_subscription`); a continuously-focused editor is left alone so
        // in-flight typing is never yanked.
        assert!(converge_on_render(false, false, false));
        assert!(converge_on_render(false, true, true));
        assert!(!converge_on_render(false, true, false));
    }
}
