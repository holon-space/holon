use super::prelude::*;
use crate::reactive_view::ChildSpaceFn;
use crate::reactive_view_model::CollectionVariant;
use crate::render_context::{AvailableSpace, LayoutHint};

/// Horizontal list layout — the primary layout of the application.
///
/// **Partitioning container**: `columns` is the first (and in v1, only)
/// builder that refines `available_space` as it cascades from a parent to
/// its children. Each child receives an equal slice of the parent's
/// `width_px` (minus the gaps between children), so profile variants
/// gating on `available_width_px` select correctly per child slot —
/// e.g. a card inside a narrow sidebar picks its phone variant even on a
/// wide desktop, because the sidebar columns only gave it ~300 px to work
/// with. This is the container-query primitive.
///
/// Three call shapes are supported:
///
/// 1. **Positional children** `columns(a, b, c)` — each child expression is
///    interpreted lazily with a partitioned `RenderContext` so that
///    `if_space` / `pick_active_variant` inside a child sees the slot width,
///    not the parent width.
///
/// 2. **Streaming** `columns(item_template: expr, from: live_query)` — the
///    existing flat-driver path; partitioning is handled by the driver via
///    `child_space_fn` (unchanged).
///
/// 3. **Snapshot** `columns(item_template: expr)` over `ctx.data_rows` —
///    same partitioning as (1), applied per-row at interpretation time.
holon_macros::widget_builder! {
    raw fn columns(ba: BA<'_>) -> ViewModel {
        let gap = ba.args.get_f64("gap").map(|v| v as f32).unwrap_or(16.0);

        // Branch A — positional children.
        //
        // Two-phase layout to handle heterogeneous child widths:
        //
        // Phase 1: Interpret all children against the parent context. Fixed
        //   children (drawers, spacers) set their own child space during
        //   interpretation and declare their width via layout_hint. The Phase 1
        //   result for Fixed children is already correct — no re-interpretation.
        //
        // Partition: sum Fixed widths + gaps, distribute remainder among Flex
        //   children by weight.
        //
        // Phase 2: Re-interpret only Flex children with their computed slot so
        //   if_space() inside them evaluates against the correct width.
        //   Cost: ~µs per child (builds ReactiveViewModel wrapper structs only;
        //   no streams or watchers are set up until watch_live() fires later).
        if !ba.args.positional_exprs.is_empty() {
            let exprs = &ba.args.positional_exprs;
            match ba.ctx.available_space {
                Some(parent) => {
                    // Phase 1 — interpret all children to discover layout hints.
                    let phase1: Vec<ViewModel> =
                        exprs.iter().map(|e| (ba.interpret)(e, ba.ctx)).collect();

                    // Collect (expr, hint) config for the PartitionedStatic driver.
                    let children_config: Vec<(holon_api::render_types::RenderExpr, LayoutHint)> =
                        exprs
                            .iter()
                            .zip(phase1.iter())
                            .map(|(expr, vm)| (expr.clone(), vm.layout_hint))
                            .collect();

                    let hints: Vec<LayoutHint> =
                        phase1.iter().map(|vm| vm.layout_hint).collect();

                    // Overlay drawers (Fixed { px: 0 }) don't consume flow space.
                    let flow_count = hints
                        .iter()
                        .filter(|h| !matches!(h, LayoutHint::Fixed { px } if *px == 0.0))
                        .count();
                    let gap_total = gap * flow_count.saturating_sub(1) as f32;

                    let fixed_total: f32 = hints
                        .iter()
                        .filter_map(|h| match h {
                            LayoutHint::Fixed { px } => Some(*px),
                            _ => None,
                        })
                        .sum();
                    let flex_weight_total: f32 = hints
                        .iter()
                        .filter_map(|h| match h {
                            LayoutHint::Flex { weight } => Some(*weight),
                            _ => None,
                        })
                        .sum();

                    let remaining = (parent.width_px - fixed_total - gap_total).max(0.0);

                    if flex_weight_total == 0.0 && remaining > 1.0 {
                        tracing::debug!(
                            "columns: all children are Fixed, {remaining:.0}px unused"
                        );
                    }

                    // Phase 2 — re-interpret only Flex children with correct slot.
                    let items: Vec<ViewModel> = phase1
                        .into_iter()
                        .zip(exprs.iter())
                        .zip(hints.iter())
                        .map(|((vm, expr), hint)| match hint {
                            LayoutHint::Fixed { .. } => vm,
                            LayoutHint::Flex { weight } => {
                                let w =
                                    remaining * weight / flex_weight_total.max(f32::EPSILON);
                                let slot = AvailableSpace {
                                    width_px: w,
                                    width_physical_px: w * parent.scale_factor,
                                    ..parent
                                };
                                (ba.interpret)(expr, &ba.ctx.with_available_space(slot))
                            }
                        })
                        .collect();

                    // PartitionedStatic: the driver will re-interpret children
                    // when parent space changes (viewport resize), pushing
                    // correct slot widths to each child reactively.
                    let view = crate::reactive_view::ReactiveView::new_partitioned_static(
                        items,
                        children_config,
                        gap,
                        Some(parent),
                        CollectionVariant::Columns { gap },
                    );
                    return ViewModel {
                        collection: Some(std::sync::Arc::new(view)),
                        ..ViewModel::from_widget("columns", std::collections::HashMap::new())
                    };
                }
                // No parent space known — static fallback for headless/snapshot.
                None => {
                    let items: Vec<ViewModel> =
                        exprs.iter().map(|e| (ba.interpret)(e, ba.ctx)).collect();
                    return ViewModel::static_collection("columns", items, gap);
                }
            }
        }

        // Branch B — data-driven. Streaming and snapshot forks.
        let template = ba
            .args
            .get_template("item_template")
            .or(ba.args.get_template("item"))
            .cloned();
        let sort_key = holon_api::render_eval::sort_key_column(ba.args).map(|s| s.to_string());

        // Data-source precedence mirrors the macro's `Collection` extraction:
        // explicit `collection:` named arg (populated by `resolve_args_with`
        // when a value-fn returns `InterpValue::Rows`) wins over the
        // inherited `ctx.data_source`. `columns` uses `raw fn` so the macro
        // path doesn't apply — handle it inline.
        let data_source: Option<std::sync::Arc<dyn holon_api::ReactiveRowProvider>> = ba
            .args
            .get_rows("collection")
            .or_else(|| {
                ba.ctx
                    .data_source
                    .clone()
                    .map(|r| r as std::sync::Arc<dyn holon_api::ReactiveRowProvider>)
            });

        match (template, data_source) {
            (Some(tmpl), Some(ds)) => {
                // Streaming: hand the partition fn + parent_space to the
                // streaming_collection path. The flat driver re-fires
                // partitioning through row_render_context on space change.
                let parent_space = ba.ctx.available_space;
                let child_space_fn: Option<Arc<ChildSpaceFn>> =
                    Some(Arc::new(move |p, c| partition(p, c, gap)));
                ViewModel::streaming_collection(
                    "columns",
                    tmpl,
                    ds,
                    gap,
                    sort_key,
                    parent_space,
                    child_space_fn,
                    None,
                )
            }
            (Some(tmpl), None) => {
                // Snapshot over ctx.data_rows. Partition per-row so that
                // if_space inside the template sees the slot width.
                let sorted =
                    holon_api::render_eval::sorted_rows(&ba.ctx.data_rows, sort_key.as_deref());
                let count = sorted.len();
                let items: Vec<ViewModel> = sorted
                    .into_iter()
                    .map(|row| {
                        let row_ctx = ba.ctx.with_row(row);
                        let row_ctx = match row_ctx.available_space {
                            Some(p) => row_ctx.with_available_space(partition(p, count, gap)),
                            None => row_ctx,
                        };
                        let ops: Vec<holon_api::render_types::OperationWiring> = ba
                            .services
                            .resolve_profile(row_ctx.row())
                            .map(|p| {
                                p.operations
                                    .into_iter()
                                    .map(|d| d.to_default_wiring())
                                    .collect()
                            })
                            .unwrap_or_default();
                        let row_ctx = if ops.is_empty() {
                            row_ctx
                        } else {
                            row_ctx.with_operations(ops, ba.services)
                        };
                        (ba.interpret)(&tmpl, &row_ctx)
                    })
                    .collect();
                ViewModel::static_collection("columns", items, gap)
            }
            (None, _) => {
                // No template, no positional children — fail loud rather than
                // silently producing an empty or wrong result.
                panic!("columns: no positional children and no item_template — nothing to render");
            }
        }
    }
}

fn partition(parent: AvailableSpace, count: usize, gap: f32) -> AvailableSpace {
    let count_f = count.max(1) as f32;
    let effective_px = (parent.width_px - gap * (count_f - 1.0)).max(0.0);
    let child_px = effective_px / count_f;
    AvailableSpace {
        width_px: child_px,
        height_px: parent.height_px,
        width_physical_px: child_px * parent.scale_factor,
        height_physical_px: parent.height_physical_px,
        scale_factor: parent.scale_factor,
    }
}
