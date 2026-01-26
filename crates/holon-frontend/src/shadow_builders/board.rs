use super::prelude::*;
use holon_api::ReactiveRowProvider;
use std::collections::HashMap;

// Kanban-style board: a horizontal arrangement of vertical lanes.
//
// Three call shapes (selected in order of precedence):
//
// 1. Inline rows: `board(item_template: card(...), lane_field: "status",
//    rows: [...])`. Used by the design gallery and tests where rows are a
//    DSL literal. Snapshot-only — no live updates.
//
// 2. Streaming: `board(item_template: card(...), lane_field: "status")` with
//    `ctx.data_source` set. Each lane gets its own `streaming_collection`
//    with a `LaneFilteredProvider` over the upstream data source, so cards
//    update reactively when the row set changes (CDC, peer sync, drag-drop
//    persistence). Lane KEYS are seeded from a snapshot at interpretation
//    time; new lanes that appear later require a board re-interpretation
//    (e.g. mode toggle / nav). Most kanban use cases pin lanes via
//    `lane_order` so this is fine.
//
// 3. Static fallback: `board(item_template: card(...), lane_field: "status")`
//    with only `ctx.data_rows` (no streaming source). Same grouping as the
//    streaming path but the cards are materialized once.
//
// Optional named args (apply to all three shapes):
// - `lane_order: ["To Do", "In Progress", "Done"]` — explicit lane sequence.
//   Lanes not in the list are appended in lexicographic order. When absent,
//   all lanes sort lexicographically (deterministic across reloads).
// - `lane_label_default: "No status"` — title used for the lane that
//   collects rows whose `lane_field` value is missing or the empty string.
// - `lane_width` — pixel width override for each lane (default: GPUI side).
//
// The "static positional" shape (`board(board_lane(...), board_lane(...))`)
// is also accepted as a fallback when no `item_template` is supplied; the
// positional children are interpreted as pre-built lanes.
holon_macros::widget_builder! {
    raw fn board(ba: BA<'_>) -> ViewModel {
        tracing::info!(
            "[BOARD_INTERP] enter row_count={} positional_exprs={} named_keys={:?} data_source={}",
            ba.ctx.data_rows.len(),
            ba.args.positional_exprs.len(),
            ba.args.named.keys().collect::<Vec<_>>(),
            ba.ctx.data_source.is_some(),
        );
        let template = ba.args.get_template("item_template").cloned();
        let lane_field = ba
            .args
            .get_string("lane_field")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "task_state".to_string());
        let lane_label_default = ba
            .args
            .get_string("lane_label_default")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "No status".to_string());
        let lane_width = ba.args.get_f64("lane_width");
        let lane_order_pref: Vec<String> = match ba.args.named.get("lane_order") {
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_string().map(|s| s.to_string()))
                .collect(),
            _ => Vec::new(),
        };

        let mut board_props: HashMap<String, Value> = HashMap::new();
        board_props.insert("lane_field".to_string(), Value::String(lane_field.clone()));
        if let Some(w) = lane_width {
            board_props.insert("lane_width".to_string(), Value::Float(w));
        }

        // Helper: order lane keys with `lane_order` first, then remaining
        // keys in lex order. Same algorithm for all three shapes.
        let order_lanes = |present: &HashMap<String, ()>| -> Vec<String> {
            let mut ordered: Vec<String> = Vec::new();
            for key in &lane_order_pref {
                if present.contains_key(key) {
                    ordered.push(key.clone());
                }
            }
            let mut remaining: Vec<String> = present
                .keys()
                .filter(|k| !ordered.contains(k))
                .cloned()
                .collect();
            remaining.sort();
            ordered.extend(remaining);
            ordered
        };

        let lane_value_of = |row: &HashMap<String, Value>| -> String {
            let raw = row.get(&lane_field).and_then(|v| v.as_string()).unwrap_or("");
            if raw.is_empty() {
                lane_label_default.clone()
            } else {
                raw.to_string()
            }
        };

        if let Some(tmpl) = template {
            // ── Shape 1: inline rows literal ────────────────────────────
            if let Some(Value::Array(arr)) = ba.args.named.get("rows") {
                let rows: Vec<Arc<HashMap<String, Value>>> = arr
                    .iter()
                    .filter_map(|v| match v {
                        Value::Object(obj) => Some(Arc::new(obj.clone())),
                        _ => None,
                    })
                    .collect();
                return interpret_static(
                    &tmpl,
                    &rows,
                    &ba,
                    &lane_field,
                    &lane_value_of,
                    &order_lanes,
                    board_props,
                );
            }

            // ── Shape 2: streaming via data_source ──────────────────────
            //
            // ONE driver owns the partitioning. `ReactiveView::new_grouped`
            // sets up a `Grouped` variant whose driver subscribes to the
            // upstream once, buckets rows by `lane_field`, and atomically
            // replaces the lane list on every event. This eliminates the
            // race that the previous "N independent `LaneFilteredProvider`
            // subscribers" design suffered from (a row in transit could be
            // observed in BOTH source and target lanes for one frame).
            if let Some(ds) = ba.ctx.data_source.as_ref() {
                let upstream: Arc<dyn ReactiveRowProvider> = ds.clone();
                let view = crate::reactive_view::ReactiveView::new_grouped(
                    crate::reactive_view_model::CollectionVariant::from_name("board", 0.0)
                        .expect("`board` layout is registered as a builtin"),
                    upstream,
                    tmpl.clone(),
                    lane_field.clone(),
                    lane_label_default.clone(),
                    lane_order_pref.clone(),
                    Some("sort_key".to_string()),
                    ba.ctx.available_space,
                );
                tracing::info!("[BOARD_INTERP] grouped view created (single-driver path)");
                return ViewModel {
                    collection: Some(Arc::new(view)),
                    ..ViewModel::from_widget("board", board_props)
                };
            }

            // ── Shape 3: static fallback over ctx.data_rows ─────────────
            return interpret_static(
                &tmpl,
                &ba.ctx.data_rows,
                &ba,
                &lane_field,
                &lane_value_of,
                &order_lanes,
                board_props,
            );
        }

        // No item_template: treat positional args as pre-built lanes.
        let children: Vec<ViewModel> = ba
            .args
            .positional_exprs
            .iter()
            .map(|expr| (ba.interpret)(expr, ba.ctx))
            .collect();
        ViewModel {
            children: children.into_iter().map(Arc::new).collect(),
            ..ViewModel::from_widget("board", board_props)
        }
    }
}

/// Static board: snapshot rows, group, interpret each card eagerly.
///
/// Used by:
/// - Inline `rows: [...]` literal (gallery, tests).
/// - The fallback path when no `data_source` is set.
fn interpret_static(
    tmpl: &holon_api::render_types::RenderExpr,
    rows: &[Arc<HashMap<String, Value>>],
    ba: &BA<'_>,
    lane_field: &str,
    lane_value_of: &dyn Fn(&HashMap<String, Value>) -> String,
    order_lanes: &dyn Fn(&HashMap<String, ()>) -> Vec<String>,
    board_props: HashMap<String, Value>,
) -> ViewModel {
    let mut lane_rows: HashMap<String, Vec<Arc<HashMap<String, Value>>>> = HashMap::new();
    for row in rows {
        lane_rows
            .entry(lane_value_of(row))
            .or_default()
            .push(row.clone());
    }
    let presence: HashMap<String, ()> = lane_rows.keys().map(|k| (k.clone(), ())).collect();
    let ordered = order_lanes(&presence);

    let lanes: Vec<ViewModel> = ordered
        .into_iter()
        .map(|lane| {
            let rows_in_lane = lane_rows.remove(&lane).unwrap_or_default();
            let cards: Vec<ViewModel> = rows_in_lane
                .into_iter()
                .map(|row| build_static_card(tmpl, row, ba, lane_field))
                .collect();
            let mut lane_props = HashMap::new();
            lane_props.insert("title".to_string(), Value::String(lane));
            ViewModel {
                children: cards.into_iter().map(Arc::new).collect(),
                ..ViewModel::from_widget("board_lane", lane_props)
            }
        })
        .collect();

    ViewModel {
        children: lanes.into_iter().map(Arc::new).collect(),
        ..ViewModel::from_widget("board", board_props)
    }
}

/// Interpret a single static card and attach `row_id` / `sort_key` props
/// so the GPUI renderer can dispatch persistence ops on drag/drop. The
/// streaming path doesn't need this — it gets row metadata via
/// `card_vm.data` (set by `flat_driver::interpret_and_attach`).
fn build_static_card(
    tmpl: &holon_api::render_types::RenderExpr,
    row: Arc<HashMap<String, Value>>,
    ba: &BA<'_>,
    _lane_field: &str,
) -> ViewModel {
    let row_ctx = ba.ctx.with_row(row.clone());
    let card_vm = (ba.interpret)(tmpl, &row_ctx);
    let row_id_val = row
        .get("id")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string());
    let sort_key_val = row
        .get("sort_key")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string());
    if row_id_val.is_some() || sort_key_val.is_some() {
        let mut extended = card_vm.props.get_cloned();
        if let Some(id) = row_id_val {
            extended.insert("row_id".to_string(), Value::String(id));
        }
        if let Some(sk) = sort_key_val {
            extended.insert("sort_key".to_string(), Value::String(sk));
        }
        card_vm.props.set(extended);
    }
    card_vm
}
