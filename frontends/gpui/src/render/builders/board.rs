use std::collections::HashMap;

use gpui::Axis;
use gpui::ElementId;
use gpui::Entity;
use gpui::IntoElement;
use gpui::SharedString;
use gpui_component::ActiveTheme as _;
use gpui_component::sortable::Sortable;
use gpui_component::sortable::SortableState;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::theme_token::ThemeToken;
use holon_frontend::OperationIntent;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;

use super::prelude::*;

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
    /// Persisted row id. `None` when the source row had no `id` column —
    /// drag/drop in that case is in-memory only. A row whose id forms no URI
    /// never reaches here: `extract_card` refuses it into a banner.
    row_id: Option<EntityUri>,
    /// Parent block id of the source row. Used as the `move_block` target
    /// when this card becomes the positional anchor of a reorder. `None`
    /// when the row carries no `parent_id` column (non-block entities) —
    /// reorders anchored on such a card are in-memory only.
    parent_id: Option<String>,
    /// Whether the card's lane lets it be dragged. `Sortable` enables drag per
    /// lane, so one card refused a drag holds its whole lane still.
    draggable: bool,
    accent: Option<gpui::Hsla>,
    accent_hex: u32,
    lines: Vec<CardLine>,
}

/// The `move_block` and `set_field` a persisted card's row is offered — the
/// operations a drag persists with. Read from the CURRENT render, never from a
/// lane's cached `SortableState` items, which keep the cards of an earlier one.
#[derive(Clone)]
struct CardOffer {
    move_block: Option<holon_api::render_types::OperationDescriptor>,
    set_field: Option<holon_api::render_types::OperationDescriptor>,
}

impl CardOffer {
    fn of(card_vm: &ReactiveViewModel) -> Self {
        let bound = |name: &str| {
            card_vm
                .operations
                .iter()
                .find(|w| w.descriptor.name == name)
                .map(|w| w.descriptor.clone())
        };
        Self {
            move_block: bound("move_block"),
            set_field: bound("set_field"),
        }
    }

    fn offers_drag(&self) -> bool {
        self.move_block.is_some() && self.set_field.is_some()
    }
}

/// The offers of the persisted cards this render shows, by row id.
type BoardOffers = std::sync::Arc<HashMap<EntityUri, CardOffer>>;

/// The accent a card names, as a theme colour and as its packed form.
///
/// A card with no accent takes the theme's muted foreground, so the border and
/// the tint still follow the active theme rather than a hardcoded constant.
fn card_accent(ctx: &GpuiRenderContext, accent: Option<&str>) -> (Option<gpui::Hsla>, u32) {
    match super::theme::optional_colour_prop(ctx, accent) {
        Some(colour) => (Some(colour), super::theme::packed(colour)),
        None => (None, super::theme::packed(tc(ctx, |t| t.muted_foreground))),
    }
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
                // `muted` reads as secondary text, and exactly two of the
                // vocabulary's names mean secondary text. The name goes through
                // the same seam every other colour site uses, so a name outside
                // the vocabulary is reported here rather than treated as
                // primary text in silence.
                muted: match child.prop_str("color").as_deref().filter(|c| !c.is_empty()) {
                    None => false,
                    Some(name) => match holon_frontend::theme_arg::resolve_colour_arg(name) {
                        Ok(ThemeToken::Muted) | Ok(ThemeToken::Secondary) => true,
                        Ok(_) => false,
                        Err(e) => {
                            tracing::error!(target: "holon.board", "{e}");
                            false
                        }
                    },
                },
            })
        })
        .collect()
}

/// A card's visual snapshot, or the refusal when its source row's `id` names
/// no entity. A refused row never becomes a draggable card: every persistence
/// leg the board has is addressed by that id, so a card built on unusable text
/// could only drag in a lie. The static path refuses the same rows one layer
/// up (`shadow_builders::board::build_static_card`).
fn extract_card(
    ctx: &GpuiRenderContext,
    lane_index: usize,
    card_index: usize,
    card_vm: &ReactiveViewModel,
) -> Result<BoardCard, holon_api::RowIdUnusable> {
    let (accent, accent_hex) = card_accent(ctx, card_vm.prop_str("accent").as_deref());
    // Static path attaches `row_id` / `parent_id` as card-level props.
    // Streaming path attaches the source row to `card_vm.data` (via
    // `flat_driver::interpret_and_attach`) — read both, props take
    // precedence so static-path tests stay deterministic.
    let row_id = match card_vm.prop_str("row_id") {
        Some(stamped) => holon_api::row_id_of_str(&stamped),
        None => card_vm.row_identity(),
    };
    if let Some(refusal) = row_id.unusable() {
        return Err(refusal.clone());
    }
    let parent_id = card_vm.prop_str("parent_id").or_else(|| {
        card_vm
            .data
            .get_cloned()
            .get("parent_id")
            .and_then(|v| v.as_string().map(|s| s.to_string()))
    });
    Ok(BoardCard {
        id: ((lane_index as u64) << 32) | (card_index as u64),
        row_id: row_id.entity(),
        parent_id,
        draggable: true,
        accent,
        accent_hex,
        lines: extract_lines(card_vm),
    })
}

/// Extract the cards that should populate a single lane.
///
/// Both paths put cards into `lane.children`:
/// - Static path (gallery / inline rows): shadow board interprets cards eagerly
///   and stores them as `lane.children`.
/// - Streaming path: `ReactiveView::create_grouped_driver` rebuilds lane VMs
///   (each with `children = cards`) atomically per upstream event.
fn extract_lane_cards(lane: &ReactiveViewModel) -> Vec<std::sync::Arc<ReactiveViewModel>> {
    lane.children.to_vec()
}

impl BoardCard {
    /// A card with no persisted row drags in memory only and dispatches
    /// nothing; a persisted card drags only when its row is offered both
    /// operations a drag dispatches.
    fn offers_drag(&self, offers: &BoardOffers) -> bool {
        self.row_id
            .as_ref()
            .is_none_or(|row_id| offers.get(row_id).is_some_and(CardOffer::offers_drag))
    }
}

/// The operation named by `pick` that the dragged card's row is offered in
/// this render. A card that is no longer offered it (it left the board, or the
/// write tier now refuses it) is disclosed and its drop not persisted.
fn offered<'a>(
    services: &std::sync::Arc<dyn BuilderServices>,
    offers: &'a BoardOffers,
    row_id: &EntityUri,
    op_name: &str,
    pick: impl Fn(&CardOffer) -> Option<&holon_api::render_types::OperationDescriptor>,
    context: &str,
) -> Option<&'a holon_api::render_types::OperationDescriptor> {
    let operation = offers.get(row_id).and_then(|offer| pick(offer));
    if operation.is_none() {
        services.surface_op_failure(
            holon_core::WRITE_TIER_ENTITY,
            op_name,
            &anyhow::anyhow!(
                "board {context}: row_id={row_id} is not offered {op_name} in the current \
                 render; the drop is not persisted"
            ),
        );
    }
    operation
}

/// Dispatch the card's offered `set_field(id=row_id, field=field,
/// value=value)`.
fn dispatch_set_field(
    services: &std::sync::Arc<dyn BuilderServices>,
    offers: &BoardOffers,
    row_id: &EntityUri,
    field: &str,
    value: String,
    context: &str,
) {
    let Some(operation) = offered(
        services,
        offers,
        row_id,
        "set_field",
        |o| o.set_field.as_ref(),
        context,
    ) else {
        return;
    };
    match OperationIntent::set_field(
        &operation.entity_name,
        &operation.name,
        &row_id.to_string(),
        field,
        Value::String(value),
    ) {
        Ok(intent) => services.dispatch_intent(intent),
        Err(e) => services.surface_op_failure(
            operation.entity_name.as_str(),
            &operation.name,
            &anyhow::anyhow!("board {context}: not dispatched for row_id={row_id}: {e}"),
        ),
    }
}

/// Cross-lane drag handler. Updates `lane_field` to the destination lane's
/// title; the drop position is persisted separately by
/// `dispatch_move_for_position` after the optimistic state update.
fn dispatch_lane_change(
    services: &std::sync::Arc<dyn BuilderServices>,
    offers: &BoardOffers,
    row_id: &EntityUri,
    lane_field: &str,
    new_value: &str,
) {
    dispatch_set_field(
        services,
        offers,
        row_id,
        lane_field,
        new_value.to_string(),
        "on_insert (lane_field)",
    );
}

/// Persist the card at `position` by dispatching a positional `move_block`
/// intent anchored on its new neighbors. The ordering authority
/// (`BlockOrdering::place` behind the `move_block` op) mints the resulting
/// order key — intent never carries one (Model.md invariant 3), so this
/// path works identically in SqlOnly and Loro mode.
///
/// Anchor choice:
/// - a previous card exists → "place after prev" under prev's parent (the exact
///   positional meaning of the drop);
/// - dropped at the top of the lane → "first child" of the next card's parent.
///   When the lane is a filtered subset of a larger sibling set this can
///   overshoot to the very front of that sibling set (still before `next`, so
///   the lane order is correct).
/// - no neighbors (single-card lane) → nothing to persist.
///
/// No-op with a `warn!` when the anchor card lacks `row_id`/`parent_id`
/// (non-block entities, rows whose query doesn't select `parent_id`) —
/// the drag stays in-memory only, disclosed in the log.
fn dispatch_move_for_position(
    services: &std::sync::Arc<dyn BuilderServices>,
    offers: &BoardOffers,
    items: &[BoardCard],
    position: usize,
    context: &str,
) {
    let Some(card) = items.get(position) else {
        return;
    };
    let Some(row_id) = card.row_id.as_ref() else {
        return;
    };
    let prev = position.checked_sub(1).and_then(|i| items.get(i));
    let next = items.get(position + 1);
    let (parent_id, after_block_id) = match (prev, next) {
        (Some(prev), _) => {
            let (Some(parent), Some(prev_id)) = (prev.parent_id.as_deref(), prev.row_id.as_ref())
            else {
                tracing::warn!(
                    "board {context}: reorder anchor lacks parent_id/row_id — not persisting \
                     position of row_id={row_id}"
                );
                return;
            };
            (parent.to_string(), Some(prev_id.to_string()))
        }
        (None, Some(next)) => {
            let Some(parent) = next.parent_id.as_deref() else {
                tracing::warn!(
                    "board {context}: top-of-lane anchor lacks parent_id — not persisting \
                     position of row_id={row_id}"
                );
                return;
            };
            (parent.to_string(), None)
        }
        (None, None) => return,
    };
    let Some(operation) = offered(
        services,
        offers,
        row_id,
        "move_block",
        |o| o.move_block.as_ref(),
        context,
    ) else {
        return;
    };
    let mut params: HashMap<String, Value> = HashMap::new();
    params.insert("id".into(), Value::String(row_id.to_string()));
    params.insert("parent_id".into(), Value::String(parent_id));
    if let Some(after) = after_block_id {
        params.insert("after_block_id".into(), Value::String(after));
    }
    services.dispatch_intent(OperationIntent::new(
        operation.entity_name.clone(),
        operation.name.clone(),
        params,
    ));
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

fn render_card(item: &BoardCard, _: usize, _: &gpui::Window, cx: &gpui::App) -> gpui::AnyElement {
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
        .when(item.draggable, |card| card.cursor_grab())
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
    //   - Streaming path: `node.collection.items` (the `Grouped` driver atomically
    //     replaces the lane list on every upstream event).
    let lanes: Vec<std::sync::Arc<ReactiveViewModel>> = if let Some(ref view) = node.collection {
        view.items.lock_ref().iter().cloned().collect()
    } else {
        node.children.to_vec()
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
    //   - `current_row.id` when the board renders inside a collection profile (each
    //     collection entity's id distinguishes its board), and
    //   - the lane structure (titles + child counts) as a structural fingerprint
    //     for the gallery / standalone path where there is no per-instance row.
    let board_key_seed = {
        use std::hash::Hash;
        use std::hash::Hasher;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        "board".hash(&mut h);
        if let Some(row) = ctx.ctx.current_row.as_ref() {
            // ALLOW(raw_row_id_column): key — hashed into the board's element id
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
        .gap(px(node
            .prop_f64("gap")
            .expect("the board builder stamps its `gap`")
            as f32))
        .p(px(LANE_GAP_PX));

    let mut offers: HashMap<EntityUri, CardOffer> = HashMap::new();
    let mut lane_cards: Vec<(Vec<BoardCard>, Vec<AnyElement>)> = Vec::with_capacity(lanes.len());
    for (lane_index, lane) in lanes.iter().enumerate() {
        let card_vms = extract_lane_cards(lane);
        let mut cards: Vec<BoardCard> = Vec::with_capacity(card_vms.len());
        let mut refusals: Vec<AnyElement> = Vec::new();
        for (card_index, card_vm) in card_vms.iter().enumerate() {
            match extract_card(ctx, lane_index, card_index, card_vm) {
                Ok(card) => {
                    if let Some(row_id) = &card.row_id {
                        offers.insert(row_id.clone(), CardOffer::of(card_vm));
                    }
                    cards.push(card);
                }
                Err(refusal) => refusals.push(error_banner(&refusal.to_string(), ctx)),
            }
        }
        lane_cards.push((cards, refusals));
    }
    let offers: BoardOffers = std::sync::Arc::new(offers);

    for (lane_index, (lane, (mut cards, refusals))) in lanes.iter().zip(lane_cards).enumerate() {
        let title = lane.prop_str("title").unwrap_or_else(|| "Lane".to_string());

        // `Sortable` enables drag per lane, so one card not offered a move
        // holds its whole lane still; the lane says so.
        let held = cards.iter().filter(|c| !c.offers_drag(&offers)).count();
        let lane_drags = held == 0;
        for card in &mut cards {
            card.draggable = lane_drags;
        }

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
            let offers_for_insert = offers.clone();
            let offers_for_reorder = offers.clone();
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
            .disabled(!lane_drags)
            .on_reorder(move |_from, to, _w, cx| {
                // Within-lane reorder: state already reflects the new order.
                // Persist by dispatching a positional move anchored on the
                // card's new neighbors (the ordering authority mints the key).
                let items = state_for_reorder.read(cx).items().to_vec();
                dispatch_move_for_position(
                    &services_for_reorder,
                    &offers_for_reorder,
                    &items,
                    to,
                    "on_reorder",
                );
            })
            .on_insert(move |item, insert_idx, _src_state, _w, cx| {
                let Some(row_id) = item.row_id.as_ref() else {
                    // Inline / synthetic cards (e.g. gallery demo) have no
                    // persisted id — drop is in-memory only.
                    return;
                };
                dispatch_lane_change(
                    &services_for_insert,
                    &offers_for_insert,
                    row_id,
                    &lane_field_owned,
                    &target_lane_value,
                );
                // Also persist the drop position so the card stays put on
                // reload (otherwise the dropped row keeps its old order key
                // from the source lane).
                let items = state_for_insert.read(cx).items().to_vec();
                dispatch_move_for_position(
                    &services_for_insert,
                    &offers_for_insert,
                    &items,
                    insert_idx,
                    "on_insert (position)",
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
            .when(!lane_drags, |lane| {
                lane.child(
                    div()
                        .text_size(px(LANE_HEADER_TEXT_SIZE_PX))
                        .text_color(tc(ctx, |t| t.muted_foreground))
                        .child(format!(
                            "{held} card(s) here are not offered a move, so this lane cannot be dragged"
                        )),
                )
            })
            .child(sortable)
            .children(refusals);

        row = row.child(lane_view);
    }

    row.into_any_element()
}
