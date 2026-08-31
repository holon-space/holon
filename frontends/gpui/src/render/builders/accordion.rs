use holon_frontend::ReactiveViewModel;

use super::prelude::*;

const DEFAULT_MAX_HEIGHT_FRACTION: f32 = 0.4;

/// Whether the accordion is currently expanded (live state on the node's
/// `expanded` Mutable, seeded from the `collapsed` prop — R6: never the
/// `ctx.local` title-keyed cache). Absent handle ⇒ treated as expanded.
fn is_expanded(node: &ReactiveViewModel) -> bool {
    node.expanded.as_ref().map(|m| m.get()).unwrap_or(true)
}

/// Header row: chevron (reflects expanded state) + optional icon + title.
///
/// When `collapsible` (default), the header is a click target that flips the
/// node's `expanded` Mutable and refreshes the window (the established
/// expand_toggle pattern — R6: the live state is the node Mutable, never the
/// `ctx.local` title-keyed cache). The next render reads `is_expanded` and
/// renders or drops the body.
fn header_row(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let title = node.prop_str("title").unwrap_or_default();
    let icon = node.prop_str("icon").unwrap_or_default();
    let glyph = if is_expanded(node) { "▾" } else { "▸" };

    let mut header = div()
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .text_color(tc(ctx, |t| t.foreground))
        .child(div().text_sm().child(glyph));
    if !icon.is_empty() {
        // Through the icon builder, like every other header: it owns the glyph
        // lookup and the Android substitution table.
        header = header.child(super::tag_with_entity_id(
            ctx,
            "icon",
            Some(&format!("accordion-icon:{title}")),
            super::icon::render_icon(&icon, 0.0, ctx),
        ));
    }
    if !title.is_empty() {
        header = header.child(
            div()
                .flex_1()
                .font_weight(gpui::FontWeight::BOLD)
                .child(title.clone()),
        );
    }

    // Collapsible header is a click target: flip the node's `expanded` Mutable
    // and refresh the window (the established expand_toggle pattern). The next
    // render reads `is_expanded` and renders or drops the body.
    let collapsible = node.prop_bool("collapsible").unwrap_or(true);
    match (collapsible, node.expanded.clone()) {
        (true, Some(expanded)) => header
            .id(hashed_id(&format!("accordion-toggle:{title}")))
            .cursor_pointer()
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, _cx| {
                expanded.set(!expanded.get());
                window.refresh();
            })
            .into_any_element(),
        _ => header.into_any_element(),
    }
}

/// Rows the accordion's content currently holds: the collections rendered
/// directly beneath it, plus the row sets of the `live_query` shells it
/// contains. A shell no frame has created yet counts zero.
///
/// `None` when nothing beneath the accordion produces rows at all — static
/// content, for which "empty" has no meaning.
fn content_rows(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Option<usize> {
    let own = node
        .collection
        .as_ref()
        .map(|v| v.children_snapshot().len());
    let streamed = (node.widget_name().as_deref() == Some("live_query")).then(|| {
        super::live_query::cached_shell(node, ctx)
            .map(|shell| ctx.with_gpui(|_, cx| shell.read(cx).row_count()))
            .unwrap_or(0)
    });
    node.children
        .iter()
        .filter_map(|c| content_rows(c, ctx))
        .chain(own)
        .chain(streamed)
        .reduce(|a, b| a + b)
}

/// What this frame does with the accordion.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Visibility {
    Paint,
    /// A `hide_when_empty` accordion whose content currently has no rows.
    Hide,
    /// `hide_when_empty` on content that can never produce a row count. Hiding
    /// it would be permanent and silent, so the frame says so instead.
    NoRowSource,
}

/// Emptiness is the content's own row count, so no second query decides it.
fn visibility(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Visibility {
    if !node.prop_bool("hide_when_empty").unwrap_or(false) {
        return Visibility::Paint;
    }
    match content_rows(node, ctx) {
        None => Visibility::NoRowSource,
        Some(0) => Visibility::Hide,
        Some(_) => Visibility::Paint,
    }
}

/// Whether the split must treat this accordion as absent — it paints nothing,
/// so the chrome that introduces it has nothing to introduce.
pub(crate) fn is_hidden(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> bool {
    visibility(node, ctx) == Visibility::Hide
}

/// The standard NAMED error widget, so the no-error-widget oracles see this
/// the way they see every other invalid accordion.
fn no_row_source_error(ctx: &GpuiRenderContext) -> Div {
    let node = ReactiveViewModel::error(
        "accordion",
        "accordion hide_when_empty needs content that produces rows",
    );
    div().w_full().child(super::render(&node, ctx))
}

/// Zero-height clip holding the content that a hidden accordion does not show.
/// Rendering it is what creates and keeps the shell whose rows decide the next
/// frame's [`is_hidden`], so the section reappears when a row arrives.
fn hidden_carrier(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    div()
        .flex_shrink_0()
        .w_full()
        .h(px(0.0))
        .overflow_hidden()
        .child(super::column::render_children_content_height(
            &node.children,
            ctx,
        ))
}

/// The bounded-footer accordion element (plan §4). Called by the flow-panel
/// split in `columns.rs`/`column.rs` for a correctly-placed accordion (direct
/// child of a flow column). Its `max_h(relative(f))` resolves against the
/// split's definite-height absolute wrapper (verified by the Inc 0 spike), so
/// with overflowing content the region caps at `f × panel_height` and its body
/// is its own scroll viewport; with little content — or none, or collapsed —
/// it shrinks to exactly what it renders.
pub(crate) fn render_bounded(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    match visibility(node, ctx) {
        Visibility::Hide => return hidden_carrier(node, ctx),
        Visibility::NoRowSource => return no_row_source_error(ctx),
        Visibility::Paint => {}
    }

    let fraction = node
        .prop_f64("max_height_fraction")
        .unwrap_or(DEFAULT_MAX_HEIGHT_FRACTION as f64) as f32;

    let expanded = is_expanded(node);

    let mut region = div()
        .flex_shrink_0()
        .w_full()
        .flex()
        .flex_col()
        .max_h(gpui::relative(fraction))
        .child(header_row(node, ctx));

    if expanded {
        let title = node.prop_str("title").unwrap_or_default();
        let body_id = hashed_id(&format!("accordion-body:{title}"));
        // Body is its own `min_h_0 + overflow_y_scroll` viewport (the April-2026
        // cascade lesson: without `min_h_0` the content height becomes the min
        // and the internal scroll freezes). Children render at content height
        // (`render_children_content_height`), so the region above shrinks to
        // them until the cap clamps it and this viewport takes over scrolling.
        region = region.child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .id(body_id)
                .overflow_y_scroll()
                .child(super::column::render_children_content_height(
                    &node.children,
                    ctx,
                )),
        );
    }
    region
}

/// Generic dispatch path — reached ONLY when an accordion is rendered somewhere
/// other than the flow-panel split (it is not a direct child of a flow column).
/// The split intercepts every correctly-placed accordion before this, so
/// reaching here means the accordion is misplaced: render the standard error
/// widget (the render-time half of the §3 fail-loud placement guard) rather
/// than a silently-unbounded region.
pub fn render(_: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    div()
        .p_2()
        .rounded(px(4.0))
        .bg(tc(ctx, |t| t.secondary))
        .text_color(tc(ctx, |t| t.danger))
        .text_sm()
        .child("accordion must be a direct child of a main-panel column")
}

/// In-flow (`pinned:false`) accordion: a capped, collapsible region rendered at
/// its NATURAL flow position inside a section stack — no split, no pinning. It
/// scrolls with its siblings. Visually identical to the bounded region; the
/// difference is placement (inline vs. the flow-panel footer split).
pub(crate) fn render_in_flow(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    render_bounded(node, ctx)
}

/// Sticky (`sticky:true`) accordion: an ABSOLUTE `.occlude()` footer overlay
/// pinned by the spike position law. `.occlude()` (wheel containment — v1
/// hard-contains a wheel at the footer, no fall-through) and the px-computed
/// cap are baked in HERE, not at call sites.
///
/// The cap and the overlay top are BOTH computed from the container's definite
/// height, read from the previous frame's committed bounds (the disclosed
/// prepaint-lag seam the spike drove off a `ScrollHandle`; it converges over
/// the settle frames):
///   - `px_cap = max_height_fraction × container_height`;
///   - `footer_top = min(viewport_bottom − footer_h, next_section_top)`.
///
/// Fail-loud on a non-definite container (never assume a height): while the
/// container has not yet committed bounds (first frame) we paint an invisible
/// zero-size placeholder so the NEXT frame has geometry to read; once the
/// container IS committed but its height is degenerate (≤ 1px — a genuinely
/// indefinite parent) we paint the standard error widget, never a fabricated
/// cap.
pub(crate) fn render_sticky(
    node: &ReactiveViewModel,
    ctx: &GpuiRenderContext,
    key: &str,
) -> AnyElement {
    use holon_frontend::geometry::GeometryProvider;
    use holon_frontend::sticky_accordion as sa;

    match visibility(node, ctx) {
        Visibility::Hide => return hidden_carrier(node, ctx).into_any_element(),
        Visibility::NoRowSource => return no_row_source_error(ctx).into_any_element(),
        Visibility::Paint => {}
    }

    let fraction = node
        .prop_f64("max_height_fraction")
        .unwrap_or(DEFAULT_MAX_HEIGHT_FRACTION as f64) as f32;
    let container_id = format!("section-stack:{key}");
    let footer_id = format!("sticky-footer:{key}");

    let container = ctx.bounds_registry.find_by_entity_id(&container_id);
    let (container_top, container_h) = match container {
        // Definite container: proceed.
        Some(info) if info.height.is_finite() && info.height > 1.0 => (info.y, info.height),
        // Committed but degenerate ⇒ genuinely indefinite parent: FAIL LOUD.
        Some(_) => {
            return div()
                .p_2()
                .rounded(px(4.0))
                .bg(tc(ctx, |t| t.secondary))
                .text_color(tc(ctx, |t| t.danger))
                .text_sm()
                .child("sticky accordion: container height is not definite")
                .into_any_element();
        }
        // Not committed yet (first frame): invisible placeholder; settle re-reads.
        None => return div().into_any_element(),
    };

    let px_cap = fraction * container_h;
    let expanded = is_expanded(node);

    // Footer height: the overlay's own committed height (previous frame),
    // capped; on the first footer frame fall back to the cap (expanded) or the
    // header height (collapsed). Converges over settle.
    let committed_h = ctx
        .bounds_registry
        .find_by_entity_id(&footer_id)
        .map(|i| i.height)
        .filter(|h| h.is_finite() && *h > 1.0);
    // Committed height was already px-capped last frame, so it IS the footer
    // height; the first footer frame falls back to the cap (expanded) or 0
    // (collapsed header sizes itself). Converges over the settle frames.
    let footer_h = match (expanded, committed_h) {
        (_, Some(h)) => h,
        (true, None) => px_cap,
        (false, None) => 0.0,
    };

    let viewport_bottom = container_top + container_h;
    let pinned = viewport_bottom - footer_h;
    // Next-section handoff: the topmost tracked section that starts BELOW the
    // pinned line pushes the outgoing footer up (spike `compute_footer_top`).
    let next_top = ctx
        .bounds_registry
        .all_elements()
        .into_iter()
        .filter(|(_, i)| i.widget_type.as_ref() == sa::SECTION_WIDGET)
        .map(|(_, i)| i.y)
        .filter(|y| *y > pinned + sa::TOL)
        .fold(f32::INFINITY, f32::min);
    let footer_top = if next_top.is_finite() {
        pinned.min(next_top)
    } else {
        pinned
    };

    // The footer itself (header + capped, internally-scrolling body).
    let mut footer = div()
        .w_full()
        .flex()
        .flex_col()
        .child(header_row(node, ctx));
    if expanded {
        // px-cap baked in: `max_h(relative(f))` does NOT clamp an absolute
        // overlay (spike finding), so cap in px against the definite container.
        footer = footer.max_h(px(px_cap));
        let title = node.prop_str("title").unwrap_or_default();
        let body_id = hashed_id(&format!("sticky-accordion-body:{title}"));
        footer = footer.child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .id(body_id)
                .overflow_y_scroll()
                .child(super::column::render_children_content_height(
                    &node.children,
                    ctx,
                )),
        );
    }

    // Absolute, occluding overlay at the computed top, full container width.
    let overlay = div()
        .absolute()
        .top(px(footer_top))
        .left(px(0.0))
        .w(px(container_below_width(ctx, &container_id)))
        // `.occlude()`: a wheel over the footer is hard-contained (v1) — it does
        // NOT fall through to the scroller behind it (spike finding).
        .occlude()
        .child(footer);

    crate::geometry::TransparentTracker::new(
        footer_id.clone(),
        sa::STICKY_FOOTER_WIDGET,
        ctx.bounds_registry.clone(),
        overlay.into_any_element(),
    )
    .with_entity_id(footer_id)
    .into_any_element()
}

/// The container's committed width (full-bleed overlay), read from the previous
/// frame; falls back to a large width the flex parent will clamp on frame 1.
fn container_below_width(ctx: &GpuiRenderContext, container_id: &str) -> f32 {
    use holon_frontend::geometry::GeometryProvider;
    ctx.bounds_registry
        .find_by_entity_id(container_id)
        .map(|i| i.width)
        .filter(|w| w.is_finite() && *w > 1.0)
        .unwrap_or(0.0)
}
