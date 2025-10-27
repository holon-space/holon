//! Container-query partitioning tests for `columns`.
//!
//! Each test builds a `columns(if_space(T, text("narrow"), text("wide")), ...)`
//! expression, interprets it with a parent `available_width_px`, and asserts
//! that every child resolved to the expected branch.
//!
//! `if_space` reads `ctx.available_space.width_px` at interpret time — the
//! same field that `pick_active_variant` reads for EntityProfile conditions.
//! So these tests validate the invariant that `columns` must interpret its
//! children with a narrowed (`slot_width`) context, not the parent's context.
//!
//! Tests are written fail-first: all fail against the current implementation
//! (positional children are interpreted with the parent context before the
//! rewrite) and pass after `columns` becomes a `raw fn` that partitions space
//! before interpreting children.

use std::sync::Arc;

use holon_api::render_types::{Arg, RenderExpr};
use holon_api::Value;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::{AvailableSpace, ReactiveViewModel, RenderContext, StubBuilderServices};

// ── DSL helpers ─────────────────────────────────────────────────────────────

fn text_expr(s: &str) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "text".to_string(),
        args: vec![Arg {
            name: None,
            value: RenderExpr::Literal {
                value: Value::String(s.to_string()),
            },
        }],
    }
}

fn if_space_expr(threshold: f64, narrow: RenderExpr, wide: RenderExpr) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "if_space".to_string(),
        args: vec![
            Arg {
                name: None,
                value: RenderExpr::Literal {
                    value: Value::Float(threshold),
                },
            },
            Arg {
                name: None,
                value: narrow,
            },
            Arg {
                name: None,
                value: wide,
            },
        ],
    }
}

/// `columns(children...)` with default gap (16).
fn columns_expr(children: Vec<RenderExpr>) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "columns".to_string(),
        args: children
            .into_iter()
            .map(|e| Arg {
                name: None,
                value: e,
            })
            .collect(),
    }
}

/// `columns(gap: G, children...)`.
fn columns_expr_with_gap(gap: f64, children: Vec<RenderExpr>) -> RenderExpr {
    let mut args = vec![Arg {
        name: Some("gap".to_string()),
        value: RenderExpr::Literal {
            value: Value::Float(gap),
        },
    }];
    args.extend(children.into_iter().map(|e| Arg {
        name: None,
        value: e,
    }));
    RenderExpr::FunctionCall {
        name: "columns".to_string(),
        args,
    }
}

// ── Interpretation helper ────────────────────────────────────────────────────

fn space(width_px: f32) -> AvailableSpace {
    AvailableSpace {
        width_px,
        height_px: 900.0,
        width_physical_px: width_px,
        height_physical_px: 900.0,
        scale_factor: 1.0,
    }
}

/// Interpret `expr` with a context whose `available_width_px` is `width`.
fn interpret_at(expr: &RenderExpr, width: f32) -> ReactiveViewModel {
    let services = StubBuilderServices::new();
    let ctx = RenderContext {
        available_space: Some(space(width)),
        ..Default::default()
    };
    services.interpret(expr, &ctx)
}

/// Interpret `expr` with no available_space (None → desktop-first fallback).
fn interpret_no_space(expr: &RenderExpr) -> ReactiveViewModel {
    let services = StubBuilderServices::new();
    let ctx = RenderContext {
        available_space: None,
        ..Default::default()
    };
    services.interpret(expr, &ctx)
}

// ── Tree walker ──────────────────────────────────────────────────────────────

/// Recursively collect all Text leaf content from a `ReactiveViewModel` tree.
fn collect_texts(node: &ReactiveViewModel) -> Vec<String> {
    if node.widget_name().as_deref() == Some("text") {
        return vec![node.prop_str("content").unwrap_or_default()];
    }

    let mut result = Vec::new();

    // Static children
    for child in &node.children {
        result.extend(collect_texts(child));
    }

    // Reactive collection children
    if let Some(ref view) = node.collection {
        for child in view.items.lock_ref().iter() {
            result.extend(collect_texts(child));
        }
    }

    // Slot content
    if let Some(ref slot) = node.slot {
        let guard = slot.content.lock_ref();
        result.extend(collect_texts(&guard));
    }

    result
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// Test 1: Two positional children, slot crosses threshold downward.
///
/// parent_width = 800, gap = 16 (default)
/// slot = (800 − 16) / 2 = 392  → < 600
/// Expected: both children pick the narrow branch.
#[test]
fn positional_children_narrow_slot() {
    let child = if_space_expr(600.0, text_expr("narrow"), text_expr("wide"));
    let expr = columns_expr(vec![child.clone(), child]);

    let vm = interpret_at(&expr, 800.0);
    let texts = collect_texts(&vm);

    assert_eq!(
        texts,
        vec!["narrow", "narrow"],
        "slot ≈ 392 px should be below threshold 600 — both children must pick 'narrow'"
    );
}

/// Test 2: Same expression at desktop width — slot stays above threshold.
///
/// parent_width = 2000, gap = 16
/// slot = (2000 − 16) / 2 = 992  → ≥ 600
/// Expected: both children pick the wide branch.
#[test]
fn positional_children_wide_slot() {
    let child = if_space_expr(600.0, text_expr("narrow"), text_expr("wide"));
    let expr = columns_expr(vec![child.clone(), child]);

    let vm = interpret_at(&expr, 2000.0);
    let texts = collect_texts(&vm);

    assert_eq!(
        texts,
        vec!["wide", "wide"],
        "slot ≈ 992 px should be above threshold 600 — both children must pick 'wide'"
    );
}

/// Test 3: Nested columns — partitioning composes recursively.
///
/// parent_width = 1000
/// outer slot = (1000 − 16) / 2 = 492
/// inner slot = (492  − 16) / 2 = 238
/// All three entities (two inner + one outer right) see their slot width.
/// With threshold 600: all three slots are below → all narrow.
#[test]
fn nested_columns_compose() {
    let leaf = if_space_expr(600.0, text_expr("narrow"), text_expr("wide"));
    let inner = columns_expr(vec![leaf.clone(), leaf.clone()]);
    let expr = columns_expr(vec![inner, leaf]);

    let vm = interpret_at(&expr, 1000.0);
    let texts = collect_texts(&vm);

    assert_eq!(
        texts,
        vec!["narrow", "narrow", "narrow"],
        "all three slots (238, 238, 492) are below 600 — all children must pick 'narrow'"
    );
}

/// Test 4a: Gap subtraction — slot exactly at threshold picks wide.
///
/// parent_width = 1280, gap = 40, three children
/// slot = (1280 − 80) / 3 = 400  → threshold 400 is NOT narrow (strict-less)
/// Expected: all three pick wide.
#[test]
fn gap_boundary_at_threshold_picks_wide() {
    let child = if_space_expr(400.0, text_expr("narrow"), text_expr("wide"));
    let expr = columns_expr_with_gap(40.0, vec![child.clone(), child.clone(), child]);

    let vm = interpret_at(&expr, 1280.0);
    let texts = collect_texts(&vm);

    assert_eq!(
        texts,
        vec!["wide", "wide", "wide"],
        "slot = 400 equals threshold 400; strict-less means NOT narrow — all must pick 'wide'"
    );
}

/// Test 4b: Gap subtraction — slot one px below threshold picks narrow.
///
/// parent_width = 1279, gap = 40, three children
/// slot = (1279 − 80) / 3 ≈ 399.67  → < 400  → narrow
#[test]
fn gap_boundary_below_threshold_picks_narrow() {
    let child = if_space_expr(400.0, text_expr("narrow"), text_expr("wide"));
    let expr = columns_expr_with_gap(40.0, vec![child.clone(), child.clone(), child]);

    let vm = interpret_at(&expr, 1279.0);
    let texts = collect_texts(&vm);

    assert_eq!(
        texts,
        vec!["narrow", "narrow", "narrow"],
        "slot ≈ 399.67 is below threshold 400 — all must pick 'narrow'"
    );
}

/// Test 5: No available_space — columns must not panic; children fall back
/// to the desktop-first (wide) branch, matching `if_space`'s documented
/// behaviour when `available_space` is `None`.
#[test]
fn no_parent_space_falls_back_to_wide() {
    let child = if_space_expr(600.0, text_expr("narrow"), text_expr("wide"));
    let expr = columns_expr(vec![child.clone(), child]);

    let vm = interpret_no_space(&expr);
    let texts = collect_texts(&vm);

    assert_eq!(
        texts,
        vec!["wide", "wide"],
        "when available_space is None, if_space falls back to the wide branch (desktop-first)"
    );
}

/// Test 6: Single child — no gap deducted.
///
/// parent_width = 500, one child, gap = 16
/// slot = (500 − 0) / 1 = 500  → < 600  → narrow
/// (gap * (count - 1) = 16 * 0 = 0)
#[test]
fn single_child_no_gap() {
    let child = if_space_expr(600.0, text_expr("narrow"), text_expr("wide"));
    let expr = columns_expr(vec![child]);

    let vm = interpret_at(&expr, 500.0);
    let texts = collect_texts(&vm);

    assert_eq!(
        texts,
        vec!["narrow"],
        "single child gets the full parent width (500) minus zero gap — must pick 'narrow'"
    );
}

/// Test 7 (snapshot / data-row path): `columns` with an `item_template` but
/// no live data source should also partition correctly.
///
/// parent_width = 800, two data rows → slot ≈ 392 → narrow.
#[test]
fn snapshot_data_rows_path_partitions() {
    use holon_api::DataRow;

    let tmpl = if_space_expr(600.0, text_expr("narrow"), text_expr("wide"));
    let expr = RenderExpr::FunctionCall {
        name: "columns".to_string(),
        args: vec![Arg {
            name: Some("item_template".to_string()),
            value: tmpl,
        }],
    };

    let rows: Vec<Arc<DataRow>> = (0..2)
        .map(|i| {
            let mut row = DataRow::new();
            row.insert("id".to_string(), Value::String(format!("block:row{i}")));
            Arc::new(row)
        })
        .collect();

    let services = StubBuilderServices::new();
    let ctx = RenderContext {
        available_space: Some(space(800.0)),
        data_rows: rows,
        ..Default::default()
    };
    let vm = services.interpret(&expr, &ctx);
    let texts = collect_texts(&vm);

    assert_eq!(
        texts,
        vec!["narrow", "narrow"],
        "snapshot path: slot ≈ 392 for 2 rows at width 800 → both must pick 'narrow'"
    );
}
