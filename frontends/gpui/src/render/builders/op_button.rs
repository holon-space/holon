use std::collections::HashMap;
use std::sync::Arc;

use holon_api::Value;
use holon_frontend::param_collection::CollectStep;
use holon_frontend::param_collection::ParamChoice;
use holon_frontend::param_collection::ParamCollector;
use holon_frontend::reactive::BuilderServices;

use super::prelude::*;
use crate::entity_view_registry::CacheKey;
use crate::geometry::TransparentTracker;

/// Transient, per-button param-collection state. `None` = closed; `Some` = the
/// popup is open on the collector's current step. Held in the ephemeral entity
/// cache, so it survives a `window.refresh()` re-render but is wiped on a
/// structural rebuild (the row going away).
struct OpParamPopup {
    collector: Option<ParamCollector>,
    /// Created and focused when the popup opens so the wrapper receives key
    /// events — Escape closes it. `None` while closed.
    focus: Option<gpui::FocusHandle>,
}

/// Render a tappable op affordance: icon above an accessible label.
///
/// Tap resolves the operation and either dispatches it (all params satisfied by
/// the row context) or opens a param-collection popup anchored at the button
/// (params still missing — e.g. `integration.set_field` needs `field` and
/// `value`). The popup collects `Bool` and `OneOf` params by pointer; a param
/// kind with no pointer affordance surfaces a visible error rather than a
/// silent no-op.
// ALLOW(fallback): names the default-branch label, not error swallowing
/// The short label under the icon is what a sighted user reads when the op has
/// no glyph; GPUI has no accessibility surface yet (see V2 in the mobile-bar
/// plan — `Android TalkBack` / `iOS VoiceOver` need upstream GPUI work).
pub fn render(node: &holon_frontend::ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let op_name = node.prop_str("op_name").unwrap_or_default();
    let target_id = node.prop_str("target_id").unwrap_or_default();
    let display_name = node.prop_str("display_name").unwrap_or_default();
    let icon_char = op_icon_char(&op_name);
    let icon_label = if icon_char.is_empty() {
        fallback_short_label(&display_name)
    } else {
        icon_char.to_string()
    };

    let services = ctx.services.clone();
    let op_name_owned = op_name.clone();
    let target_id_owned = target_id.clone();
    let element_id = format!("op-button-{op_name}-{target_id}");

    let icon_size = ctx.style().icon_size;
    let box_padding = ctx.style().icon_box_padding;

    // Transient popup state for this button, keyed on the op + target so two
    // rows (or two ops on one row) never share a collection in progress.
    let popup = ctx.local.get_or_create_typed(
        CacheKey::Ephemeral(format!("op-param-popup:{op_name}:{target_id}")),
        || {
            ctx.with_gpui(|_window, cx| {
                cx.new(|_cx| OpParamPopup {
                    collector: None,
                    focus: None,
                })
            })
        },
    );
    let open_step: Option<CollectStep> =
        ctx.with_gpui(|_window, cx| popup.read(cx).collector.as_ref().map(|c| c.current()));

    let tracked_id = element_id.clone();
    let tracked_label = display_name.clone();
    let popup_click = popup.clone();
    let inner = div()
        .id(hashed_id(&element_id))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(2.0))
        .px(px(box_padding))
        .py(px(4.0))
        .cursor_pointer()
        .child(
            div()
                .text_size(px(icon_size))
                .line_height(px(icon_size))
                .text_color(tc(ctx, |t| t.foreground))
                .child(icon_label),
        )
        .child(
            div()
                .text_size(px(10.0))
                .line_height(px(12.0))
                .text_color(tc(ctx, |t| t.muted_foreground))
                .child(display_name.clone()),
        )
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            open_or_dispatch(
                &services,
                &popup_click,
                &op_name_owned,
                &target_id_owned,
                window,
                cx,
            );
        })
        .into_any_element();

    // Registered under `op-button-{op}-{target}` as well as the registry's
    // positional tag: a caller asking "does THIS row offer THAT op" has no
    // other way to name it.
    let trigger =
        TransparentTracker::new(tracked_id, "op_button", ctx.bounds_registry.clone(), inner)
            .with_displayed_text(tracked_label);

    let Some(step) = open_step else {
        return trigger.into_any_element();
    };

    // The menu renders INLINE beneath the button, not as a floating overlay: a
    // row list is tightly packed, and an absolutely-positioned menu lands on the
    // next row, where a deferred layer failed to capture the click and it fell
    // through to that row's button. An in-flow menu is laid out where it paints,
    // so its choices hit-test exactly where they are drawn.
    let menu = build_param_popup(&popup, &op_name, &target_id, step, ctx);

    // Dismissal lives on the WRAPPER (trigger + menu), not the menu alone: an
    // own-trigger click would otherwise close in the capture phase and reopen in
    // the bubble phase. `on_mouse_down_out` closes the popup on any click outside
    // the wrapper — the Settings modal's own dismissal idiom (a capture-phase
    // window-wide listener; no backdrop, no z-order). Opening another row's
    // button is itself such an outside click, so this is the single-open
    // mechanism the code relies on. Escape needs the focus handle (created on
    // open) tracked here so key dispatch, which walks root→focused, reaches it.
    // Neither path dispatches an operation — closing is not a mutation.
    let focus_handle = ctx.with_gpui(|_window, cx| popup.read(cx).focus.clone());
    let escape_popup = popup.clone();
    let outside_popup = popup.clone();
    let mut wrapper = div()
        .id(hashed_id(&format!(
            "op-param-wrapper-{op_name}-{target_id}"
        )))
        .flex()
        .flex_col();
    if let Some(handle) = &focus_handle {
        wrapper = wrapper.track_focus(handle);
    }
    wrapper
        .on_key_down(move |ev, window, cx| {
            if ev.keystroke.key.as_str() == "escape" {
                escape_popup.update(cx, |p, _cx| {
                    p.collector = None;
                    p.focus = None;
                });
                window.refresh();
                cx.stop_propagation();
            }
        })
        .on_mouse_down_out(move |_, window, cx| {
            outside_popup.update(cx, |p, _cx| {
                p.collector = None;
                p.focus = None;
            });
            window.refresh();
        })
        .child(trigger)
        .child(menu)
        .into_any_element()
}

/// Resolve the operation for this click and route it: dispatch directly when
/// the row context already satisfies every param, else open the collection
/// popup. `window.refresh()` re-renders so the just-opened popup paints (the
/// same idiom `expand_toggle` uses for ephemeral state).
fn open_or_dispatch(
    services: &Arc<dyn BuilderServices>,
    popup: &gpui::Entity<OpParamPopup>,
    op_name: &str,
    target_id: &str,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    let mut probe: HashMap<String, Value> = HashMap::new();
    probe.insert("id".into(), Value::String(target_id.to_string()));
    let Some(profile) = services.resolve_profile(&probe) else {
        tracing::warn!("op_button tap: resolve_profile returned None for target_id={target_id}");
        return;
    };
    let Some(op) = profile.operations.into_iter().find(|o| o.name == op_name) else {
        tracing::warn!(
            "op_button tap: op '{op_name}' not found on profile for target_id={target_id}"
        );
        return;
    };
    let mut ctx_params: HashMap<String, Value> = HashMap::new();
    ctx_params.insert("id".into(), Value::String(target_id.to_string()));

    let collector = ParamCollector::for_op(&op, &ctx_params);
    if collector.needs_collection() {
        // Focus the popup on open so Escape reaches it (key dispatch walks
        // root→focused). The wrapper tracks this same handle next render.
        let handle = cx.focus_handle();
        handle.focus(window, cx);
        popup.update(cx, |p, _cx| {
            p.collector = Some(collector);
            p.focus = Some(handle);
        });
        window.refresh();
    } else {
        services.present_op(op, ctx_params);
    }
}

/// The overlay: the current step's choices (or a visible error for a param kind
/// with no pointer affordance), wrapped so the windowed driver and PBTs can
/// find it (`op_param_popup`) and click its items
/// (`op-param-item-{param}-{slug}`).
///
/// This overlay is the only driver of the `ParamCollector` state machine; the
/// editor slash menu (`command_provider` via `EditorView`) still runs its own
/// async EntityId param collection. When those unify, another driver appears,
/// or a flow needs a non-anchored / app-level popup, promote to one
/// frontend-session-level popup surface observed from `present_op` (Option B in
/// ~/.claude/plans/holon-opbtn-param-popup-2026-08-20.md).
fn build_param_popup(
    popup: &gpui::Entity<OpParamPopup>,
    op_name: &str,
    target_id: &str,
    step: CollectStep,
    ctx: &GpuiRenderContext,
) -> TransparentTracker {
    let bg = tc(ctx, |t| t.popover);
    let border = tc(ctx, |t| t.border);
    let fg = tc(ctx, |t| t.foreground);
    let muted = tc(ctx, |t| t.muted_foreground);
    let accent = tc(ctx, |t| t.accent);

    let mut menu = div()
        .mt(px(2.0))
        .min_w(px(120.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .p(px(4.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .rounded(px(6.0))
        .shadow_md();

    match step {
        CollectStep::Collect {
            param_name,
            choices,
        } => {
            menu = menu.child(
                div()
                    .px(px(8.0))
                    .py(px(2.0))
                    .text_size(px(10.0))
                    .text_color(muted)
                    .child(param_name.clone()),
            );
            for choice in choices {
                menu = menu.child(choice_row(popup, &param_name, choice, fg, accent, ctx));
            }
        }
        CollectStep::Unsupported { reason, .. } => {
            // Fail loud, visible: no pointer affordance for this param kind, so
            // say so rather than silently doing nothing.
            menu = menu.child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .max_w(px(280.0))
                    .text_size(px(12.0))
                    .text_color(tc(ctx, |t| t.danger))
                    .child(reason),
            );
        }
        CollectStep::Ready(_) => {
            // A ready collector dispatches and closes on the final pick, so the
            // popup is never rendered in this state. Reaching here means the
            // open/close bookkeeping drifted.
            menu = menu.child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .text_size(px(12.0))
                    .text_color(tc(ctx, |t| t.danger))
                    .child("internal error: popup open with all params resolved"),
            );
        }
    }

    TransparentTracker::new(
        format!("op-param-popup-{op_name}-{target_id}"),
        "op_param_popup",
        ctx.bounds_registry.clone(),
        menu.into_any_element(),
    )
    .with_vm_node(None)
}

/// One pickable choice. Clicking resolves the current param and either advances
/// to the next step or, once every param is resolved, dispatches the operation
/// through the SAME path a satisfied click uses (`dispatch_intent` — journal,
/// latency, failure toast) and closes the popup.
fn choice_row(
    popup: &gpui::Entity<OpParamPopup>,
    param_name: &str,
    choice: ParamChoice,
    fg: gpui::Hsla,
    accent: gpui::Hsla,
    ctx: &GpuiRenderContext,
) -> TransparentTracker {
    let item_id = format!("op-param-item-{param_name}-{}", choice.slug);
    let services = ctx.services.clone();
    let popup_click = popup.clone();
    let param_owned = param_name.to_string();
    let value = choice.value.clone();

    let row = div()
        .id(hashed_id(&item_id))
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .text_size(px(13.0))
        .text_color(fg)
        .hover(|s| s.bg(accent).text_color(gpui::rgb(0xffffff)))
        .child(choice.label)
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            let intent = popup_click.update(cx, |p, _cx| {
                let step = {
                    let collector = p
                        .collector
                        .as_mut()
                        .expect("a choice was clicked while the popup was closed");
                    collector.pick(&param_owned, value.clone());
                    collector.current()
                };
                match step {
                    CollectStep::Ready(intent) => {
                        p.collector = None;
                        Some(intent)
                    }
                    CollectStep::Collect { .. } | CollectStep::Unsupported { .. } => None,
                }
            });
            if let Some(intent) = intent {
                services.dispatch_intent(intent);
            }
            window.refresh();
        });

    TransparentTracker::new(
        item_id,
        "op_param_item",
        ctx.bounds_registry.clone(),
        row.into_any_element(),
    )
}

/// Hardcoded op-name → single-char icon glyph map. The single source of truth
/// for op glyphs — [`op_icon_char`] looks up here and the icon-font coverage
/// test sweeps it, so every op glyph is asserted Android-renderable. Glyphs the
/// embedded DejaVu font can't render (🗑 delete, ⧉ embed) are substituted on
/// Android by `crate::icon` via `crate::ICON_SUBSTITUTES`.
pub(crate) const OP_ICONS: &[(&str, &str)] = &[
    ("cycle_task_state", "\u{27F3}"), // ⟳
    ("delete", "\u{1F5D1}"),          // 🗑
    ("dismiss_advice", "\u{2715}"),   // ✕ (dismiss a woven advice suggestion, ADR 0022)
    ("create", "+"),
    ("update", "\u{270E}"),       // ✎
    ("set_field", "\u{270E}"),    // ✎
    ("embed_entity", "\u{29C9}"), // ⧉
    ("embed", "\u{29C9}"),        // ⧉
    ("indent", "\u{21E5}"),       // ⇥
    ("outdent", "\u{21E4}"),      // ⇤
    ("move_up", "\u{2191}"),      // ↑
    ("move_down", "\u{2193}"),    // ↓
];

/// Op-name → icon glyph, already routed through `crate::icon` so glyphs the
/// embedded coverage font can't render are substituted on Android. Unknowns
/// return empty and the caller falls back to the first two letters of
/// `display_name`.
fn op_icon_char(op_name: &str) -> &'static str {
    OP_ICONS
        .iter()
        .find(|(name, _)| *name == op_name)
        .map(|(_, glyph)| crate::icon(glyph))
        .unwrap_or("")
}

fn fallback_short_label(display_name: &str) -> String {
    display_name
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

#[cfg(test)]
mod op_icon_coverage {
    use super::OP_ICONS;

    /// Every op glyph must render on Android — DejaVu-covered directly, or
    /// routed through `crate::ICON_SUBSTITUTES` (🗑, ⧉). Sweeps the table so a
    /// newly-added op glyph cannot silently tofu the way `delete`/`embed` did.
    #[test]
    fn every_op_glyph_renders_on_android() {
        for (op_name, glyph) in OP_ICONS {
            crate::assert_icon_renderable_on_android(glyph, &format!("op_button::{op_name}"));
        }
    }
}
