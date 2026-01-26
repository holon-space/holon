use super::prelude::*;

use std::collections::HashMap;

use gpui::{Axis, ElementId, Entity, IntoElement, SharedString};
use gpui_component::sortable::{Sortable, SortableState};
use gpui_component::ActiveTheme as _;

use holon_api::Value;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::OperationIntent;

const LANE_WIDTH_PX: f32 = 240.0;
const LANE_GAP_PX: f32 = 16.0;
const CARD_GAP_PX: f32 = 6.0;
const LANE_HEADER_GAP_PX: f32 = 8.0;
const LANE_PADDING_PX: f32 = 10.0;
const LANE_BORDER_RADIUS_PX: f32 = 8.0;
const LANE_HEADER_TEXT_SIZE_PX: f32 = 12.0;
const CARD_BORDER_RADIUS_PX: f32 = 6.0;
const CARD_PAD_X_PX: f32 = 12.0;
const CARD_PAD_Y_PX: f32 = 8.0;
const CARD_GAP_INNER_PX: f32 = 4.0;
const CARD_BG: u32 = 0x2A2A27FF;

/// Visual snapshot of a single text child of a `card(...)` view model.
#[derive(Clone)]
struct CardLine {
    content: String,
    bold: bool,
    size: f32,
    muted: bool,
}

/// Sortable item, fully self-contained: holds the visual snapshot extracted
/// from the `card(...)` view model (accent, text lines). We can't store an
/// `Entity<Render>` here because Sortable invokes `render_item` twice per
/// frame for a dragging item (once for the in-list ghost, once for the
/// floating drag preview, see `SortableDragData::render` in gpui-component).
/// Sharing one Entity between both placements double-renders it and the
/// element tree gets confused (visible flicker / disappearing cards). Plain
/// data + inline div construction sidesteps that.
#[derive(Clone)]
struct BoardCard {
    id: u64,
    /// Persisted row id (e.g. `block:UUID`). `None` when the source row had
    /// no `id` column — drag/drop in that case is in-memory only.
    row_id: Option<String>,
    /// Fractional-index sort_key. `None` when the row has no `sort_key`
    /// column (rare today; non-block entities). Within-lane reorder updates
    /// only fire when this is `Some`.
    sort_key: Option<String>,
    accent: Option<gpui::Hsla>,
    accent_hex: u32,
    lines: Vec<CardLine>,
}

fn parse_hex(hex: &str) -> Option<u32> {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 || !hex.is_ascii() {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()? as u32;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()? as u32;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()? as u32;
    Some((r << 24) | (g << 16) | (b << 8) | 0xFF)
}

/// Tint `base` toward `accent` at ~15% blend. Mirrors `card.rs::tint_rgba` so
/// drag/drop and non-drag card visuals stay aligned.
fn tint_rgba(accent: u32, base: u32) -> gpui::Hsla {
    let mix = |shift: u32| -> u32 {
        let ca = ((accent >> shift) & 0xFF) as f32;
        let cb = ((base >> shift) & 0xFF) as f32;
        (ca * 0.15 + cb * 0.85) as u32
    };
    let r = mix(24);
    let g = mix(16);
    let b = mix(8);
    gpui::rgba((r << 24) | (g << 16) | (b << 8) | 0xFF).into()
}

fn extract_lines(card_vm: &ReactiveViewModel) -> Vec<CardLine> {
    card_vm
        .children
        .iter()
        .filter_map(|child| {
            if child.widget_name().as_deref() != Some("text") {
                return None;
            }
            let content = child.prop_str("content").unwrap_or_default();
            if content.is_empty() {
                return None;
            }
            Some(CardLine {
                content,
                bold: child.prop_bool("bold").unwrap_or(false),
                size: child.prop_f64("size").unwrap_or(14.0) as f32,
                muted: matches!(
                    child.prop_str("color").as_deref(),
                    Some("muted") | Some("secondary")
                ),
            })
        })
        .collect()
}

fn extract_card(lane_index: usize, card_index: usize, card_vm: &ReactiveViewModel) -> BoardCard {
    let accent_str = card_vm.prop_str("accent").unwrap_or_default();
    let accent_hex = parse_hex(&accent_str).unwrap_or(CARD_BG);
    let accent = parse_hex(&accent_str).map(|hex| gpui::rgba(hex).into());
    // Static path attaches `row_id` / `sort_key` as card-level props.
    // Streaming path attaches the source row to `card_vm.data` (via
    // `flat_driver::interpret_and_attach`) — read both, props take
    // precedence so static-path tests stay deterministic.
    let row_id = card_vm.prop_str("row_id").or_else(|| {
        card_vm
            .data
            .get_cloned()
            .get("id")
            .and_then(|v| v.as_string().map(|s| s.to_string()))
    });
    let sort_key = card_vm.prop_str("sort_key").or_else(|| {
        card_vm
            .data
            .get_cloned()
            .get("sort_key")
            .and_then(|v| v.as_string().map(|s| s.to_string()))
    });
    BoardCard {
        id: ((lane_index as u64) << 32) | (card_index as u64),
        row_id,
        sort_key,
        accent,
        accent_hex,
        lines: extract_lines(card_vm),
    }
}

/// Extract the cards that should populate a single lane.
///
/// Both paths put cards into `lane.children`:
/// - Static path (gallery / inline rows): shadow board interprets cards
///   eagerly and stores them as `lane.children`.
/// - Streaming path: `ReactiveView::create_grouped_driver` rebuilds lane
///   VMs (each with `children = cards`) atomically per upstream event.
fn extract_lane_cards(lane: &ReactiveViewModel) -> Vec<std::sync::Arc<ReactiveViewModel>> {
    lane.children.iter().cloned().collect()
}

/// Resolve the row's profile and its `set_field` op. Returns the
/// entity-name owning that op so callers can build a typed intent. Logs
/// and returns `None` on missing profile / op (callers skip silently —
/// drag/drop must not crash mid-drag).
fn resolve_set_field_entity(
    services: &std::sync::Arc<dyn BuilderServices>,
    row_id: &str,
    context: &str,
) -> Option<holon_api::EntityName> {
    let mut probe: HashMap<String, Value> = HashMap::new();
    probe.insert("id".into(), Value::String(row_id.to_string()));
    let Some(profile) = services.resolve_profile(&probe) else {
        tracing::warn!("board {context}: resolve_profile None for row_id={row_id}");
        return None;
    };
    let Some(op) = profile.operations.iter().find(|o| o.name == "set_field") else {
        tracing::warn!(
            "board {context}: set_field op not found on profile for row_id={row_id}"
        );
        return None;
    };
    Some(op.entity_name.clone())
}

/// Dispatch `set_field(id=row_id, field=field, value=value)` after
/// resolving the row's entity.
fn dispatch_set_field(
    services: &std::sync::Arc<dyn BuilderServices>,
    row_id: &str,
    field: &str,
    value: String,
    context: &str,
) {
    let Some(entity_name) = resolve_set_field_entity(services, row_id, context) else {
        return;
    };
    let intent = OperationIntent::set_field(
        &entity_name,
        "set_field",
        row_id,
        field,
        Value::String(value),
    );
    services.dispatch_intent(intent);
}

/// Cross-lane drag handler. Updates `lane_field` to the destination lane's
/// title; the sort_key for the new position is updated separately by
/// `dispatch_sort_key_for_position` after the optimistic state update.
fn dispatch_lane_change(
    services: &std::sync::Arc<dyn BuilderServices>,
    row_id: &str,
    lane_field: &str,
    new_value: &str,
) {
    dispatch_set_field(
        services,
        row_id,
        lane_field,
        new_value.to_string(),
        "on_insert (lane_field)",
    );
}

/// Compute a fractional-index key that places `row_id` at `position` in
/// `items` and dispatch a `set_field(sort_key)` intent. No-op when the
/// row has no `sort_key` (non-block entity), or when both neighbors lack
/// one (no anchor to bisect from).
fn dispatch_sort_key_for_position(
    services: &std::sync::Arc<dyn BuilderServices>,
    items: &[BoardCard],
    position: usize,
    context: &str,
) {
    let Some(card) = items.get(position) else {
        return;
    };
    let Some(row_id) = card.row_id.as_deref() else {
        return;
    };
    let prev = position
        .checked_sub(1)
        .and_then(|i| items.get(i))
        .and_then(|c| c.sort_key.as_deref());
    let next = items.get(position + 1).and_then(|c| c.sort_key.as_deref());
    if prev.is_none() && next.is_none() {
        // No anchors — a single-card lane carries whatever sort_key it
        // already had. Nothing to fix.
        return;
    }
    match holon::storage::gen_key_between(prev, next) {
        Ok(new_key) => {
            dispatch_set_field(services, row_id, "sort_key", new_key, context);
        }
        Err(err) => {
            tracing::warn!(
                "board {context}: gen_key_between failed for row_id={row_id}: {err}"
            );
        }
    }
}

fn lane_state(
    ctx: &GpuiRenderContext,
    key: crate::entity_view_registry::CacheKey,
    initial: Vec<BoardCard>,
) -> Entity<SortableState<BoardCard>> {
    ctx.local.get_or_create_typed(key, || {
        ctx.with_gpui(|_window, cx| cx.new(|_| SortableState::new(initial)))
    })
}

fn render_card(
    item: &BoardCard,
    _ix: usize,
    _window: &gpui::Window,
    cx: &gpui::App,
) -> gpui::AnyElement {
    let theme = cx.theme();
    let accent = item.accent.unwrap_or(theme.primary);
    let tinted = tint_rgba(item.accent_hex, CARD_BG);

    let mut container = div()
        .id(("board-card", item.id))
        .w_full()
        .bg(tinted)
        .rounded(px(CARD_BORDER_RADIUS_PX))
        .shadow_sm()
        .border_l_4()
        .border_color(accent)
        .px(px(CARD_PAD_X_PX))
        .py(px(CARD_PAD_Y_PX))
        .flex()
        .flex_col()
        .gap(px(CARD_GAP_INNER_PX))
        .cursor_grab()
        .hover(|s| s.shadow_md());

    for line in &item.lines {
        let color = if line.muted {
            theme.muted_foreground
        } else {
            theme.foreground
        };
        let mut text_el = div()
            .text_size(px(line.size))
            .text_color(color)
            .child(line.content.clone());
        if line.bold {
            text_el = text_el.font_weight(gpui::FontWeight::SEMIBOLD);
        }
        container = container.child(text_el);
    }

    container.into_any_element()
}

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    // Lanes come from one of two places:
    //   - Static path: `node.children` (gallery / inline rows).
    //   - Streaming path: `node.collection.items` (the `Grouped` driver
    //     atomically replaces the lane list on every upstream event).
    let lanes: Vec<std::sync::Arc<ReactiveViewModel>> = if let Some(ref view) = node.collection {
        view.items.lock_ref().iter().cloned().collect()
    } else {
        node.children.iter().cloned().collect()
    };
    tracing::info!(
        "[BOARD_RENDER] enter lane_count={} lane_field={:?} streaming={}",
        lanes.len(),
        node.prop_str("lane_field"),
        node.collection.is_some(),
    );
    // Lane field name (e.g. "task_state", "status"). Driven by the shadow
    // board builder which writes it as a top-level prop. Used by the
    // cross-lane drag handler to dispatch the correct `set_field` op.
    let lane_field = node
        .prop_str("lane_field")
        .unwrap_or_else(|| "task_state".to_string());
    // Optional lane width override. Falls through to the default constant
    // when absent. Resolved once for the whole board (uniform-width lanes).
    let lane_width_px = node.prop_f64("lane_width").unwrap_or(LANE_WIDTH_PX as f64) as f32;

    // Cache key must be stable across frames AND distinct between boards in
    // the same view. Parent re-interprets the ViewModel tree every frame, so
    // a pointer-derived seed would miss the cache each frame and reset
    // `SortableState`, dropping any in-progress reorder. We seed from:
    //   - `current_row.id` when the board renders inside a collection
    //     profile (each collection entity's id distinguishes its board), and
    //   - the lane structure (titles + child counts) as a structural
    //     fingerprint for the gallery / standalone path where there is no
    //     per-instance row.
    let board_key_seed = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        "board".hash(&mut h);
        if let Some(row) = ctx.ctx.current_row.as_ref() {
            if let Some(id) = row.get("id").and_then(|v| v.as_string()) {
                id.hash(&mut h);
            }
        }
        for lane in lanes.iter() {
            if let Some(t) = lane.prop_str("title") {
                t.hash(&mut h);
            }
            h.write_usize(extract_lane_cards(lane).len());
        }
        format!("board:{:016x}", h.finish())
    };
    let board_key_seed = board_key_seed.as_str();

    // Outer row holds the lanes side-by-side. `id` + `overflow_x_scroll`
    // turn it into a horizontally scrollable container so wide boards
    // (many lanes / wide lanes) scroll instead of clipping or squashing.
    // The id seed is the same one we use for SortableState caching, so it
    // stays stable across frames AND distinct between boards in the same
    // view. `w_full` lets the container claim the parent's available
    // width as the visible viewport; lanes' `flex_shrink_0` keeps them at
    // their declared width regardless.
    let mut row = div()
        .id(SharedString::from(format!("{board_key_seed}:scroll")))
        .flex()
        .flex_row()
        .items_start()
        .w_full()
        .overflow_x_scroll()
        .gap(px(LANE_GAP_PX))
        .p(px(LANE_GAP_PX));

    for (lane_index, lane) in lanes.iter().enumerate() {
        let title = lane
            .prop_str("title")
            .unwrap_or_else(|| "Lane".to_string());

        let card_vms = extract_lane_cards(lane);
        let cards: Vec<BoardCard> = card_vms
            .iter()
            .enumerate()
            .map(|(card_index, card_vm)| extract_card(lane_index, card_index, card_vm))
            .collect();

        let state_key = crate::entity_view_registry::CacheKey::Ephemeral(format!(
            "{board_key_seed}:lane-state:{lane_index}"
        ));
        let state = lane_state(ctx, state_key, cards);

        let item_id = move |item: &BoardCard| -> ElementId {
            ElementId::NamedInteger("board-card".into(), item.id)
        };

        let lane_id_str = format!("board-lane-{lane_index}");
        let sortable = {
            // Capture per-lane drag-target context. Both callbacks see the
            // post-update state via the cloned Entity handle.
            let services_for_insert = ctx.services.clone();
            let services_for_reorder = ctx.services.clone();
            let state_for_insert = state.clone();
            let state_for_reorder = state.clone();
            let lane_field_owned = lane_field.clone();
            let target_lane_value = title.clone();
            Sortable::new(
                ElementId::Name(lane_id_str.into()),
                state,
                item_id,
                render_card,
            )
            .axis(Axis::Vertical)
            .gap(px(CARD_GAP_PX))
            .on_reorder(move |_from, to, _w, cx| {
                // Within-lane reorder: state already reflects the new order.
                // Dispatch sort_key update for the moved card based on its
                // new neighbors.
                let items = state_for_reorder.read(cx).items().to_vec();
                dispatch_sort_key_for_position(
                    &services_for_reorder,
                    &items,
                    to,
                    "on_reorder",
                );
            })
            .on_insert(move |item, insert_idx, _src_state, _w, cx| {
                let Some(row_id) = item.row_id.as_deref() else {
                    // Inline / synthetic cards (e.g. gallery demo) have no
                    // persisted id — drop is in-memory only.
                    return;
                };
                dispatch_lane_change(
                    &services_for_insert,
                    row_id,
                    &lane_field_owned,
                    &target_lane_value,
                );
                // Also update sort_key so the card stays at this position
                // on reload (otherwise the dropped row keeps its old
                // sort_key from the source lane).
                let items = state_for_insert.read(cx).items().to_vec();
                dispatch_sort_key_for_position(
                    &services_for_insert,
                    &items,
                    insert_idx,
                    "on_insert (sort_key)",
                );
            })
        };

        let lane_view = div()
            .flex()
            .flex_col()
            .w(px(lane_width_px))
            .flex_shrink_0()
            .gap(px(LANE_HEADER_GAP_PX))
            .p(px(LANE_PADDING_PX))
            .rounded(px(LANE_BORDER_RADIUS_PX))
            .bg(tc(ctx, |t| t.sidebar))
            .border_1()
            .border_color(tc(ctx, |t| t.border))
            .child(
                div()
                    .text_size(px(LANE_HEADER_TEXT_SIZE_PX))
                    .text_color(tc(ctx, |t| t.muted_foreground))
                    .child(title.to_uppercase()),
            )
            .child(sortable);

        row = row.child(lane_view);
    }

    row.into_any_element()
}
