use holon_api::Value;
use holon_api::render_types::RenderExpr;

use super::prelude::*;
use crate::reactive_view_model::CollectionData;

// `table` is a collection widget: it delivers a data row set through an item
// template, streaming when a live data source is present and snapshotting
// otherwise. That is its whole behaviour when called bare — `live_query`'s
// default item template is `table()`.
//
// `columns:` layers a COLUMNAR presentation on top without changing that. The
// spec is a list of column maps `#{header: "…", cell: <render-expr>, width:
// flex(w)|fixed(px)}`. Two things come out of it:
//
//   * the per-row item template, synthesized as `row(cell0, …, cellC)` — so
//     rows keep going through the collection pipeline and each one keeps its
//     reactive row handle. A cell that re-reads its row (an `ops_of` list whose
//     guard reads a mirror column) therefore re-evaluates when the row changes.
//   * the column geometry, as props on the collection NODE (`col_count`,
//     `col{k}_header`, `col{k}_width`). One vector, applied by the renderer to
//     the header and to every row alike — which is what makes the columns line
//     up rather than each row negotiating its own widths.
//
// The columnar form takes the `table_columnar` layout so a bare `table` keeps
// the default `table` layout and its render path untouched.

enum ColumnWidth {
    Flex(f32),
    Fixed(f32),
}

struct ColumnDef {
    header: String,
    cell: RenderExpr,
    width: ColumnWidth,
    /// Whether this column's cell breaks its own content across lines. Parsed
    /// once here rather than re-derived where the floors are computed.
    wraps: bool,
}

/// Whether `cell` contains a collection that wraps — `list(#{…, wrap: "wrap"})`
/// at any depth.
///
/// A column like that has no smallest width it can be read at: give it less
/// room and its items take another line, which is the shape `wrap` exists to
/// produce. A column of text has one, because a word cannot break.
fn cell_wraps(cell: &RenderExpr) -> bool {
    match cell {
        RenderExpr::FunctionCall { args, .. } => {
            let wraps_here = args.iter().any(|a| {
                a.name.as_deref() == Some("wrap")
                    && matches!(&a.value, RenderExpr::Literal { value } if value.as_string() == Some("wrap"))
            });
            wraps_here || args.iter().any(|a| cell_wraps(&a.value))
        }
        RenderExpr::Array { items } => items.iter().any(cell_wraps),
        RenderExpr::Object { fields } => fields.values().any(cell_wraps),
        RenderExpr::BinaryOp { left, right, .. } => cell_wraps(left) || cell_wraps(right),
        RenderExpr::Literal { .. }
        | RenderExpr::ColumnRef { .. }
        | RenderExpr::LiveBlock { .. } => false,
    }
}

fn parse_width(expr: Option<&RenderExpr>) -> ColumnWidth {
    match expr {
        None => ColumnWidth::Flex(1.0),
        Some(RenderExpr::FunctionCall { name, args }) => {
            let n = args.first().and_then(|a| match &a.value {
                RenderExpr::Literal { value } => value.as_f64(),
                _ => None,
            });
            match name.as_str() {
                "flex" => ColumnWidth::Flex(n.unwrap_or(1.0) as f32),
                "fixed" => ColumnWidth::Fixed(n.unwrap_or_else(|| {
                    panic!("table column `width: fixed(px)` needs a numeric px, got {expr:?}")
                }) as f32),
                other => panic!(
                    "table column `width` must be flex(w) or fixed(px), got `{other}(…)` — \
                     content-max/auto width is unsupported (needs a cross-row measurement pass)"
                ),
            }
        }
        Some(other) => panic!("table column `width` must be flex(w) or fixed(px), got {other:?}"),
    }
}

fn parse_columns(expr: &RenderExpr) -> Vec<ColumnDef> {
    let items = match expr {
        RenderExpr::Array { items } => items,
        other => panic!("table `columns:` must be a list of column maps, got {other:?}"),
    };
    assert!(
        !items.is_empty(),
        "table `columns:` must list at least one column"
    );
    items
        .iter()
        .map(|item| {
            let fields = match item {
                RenderExpr::Object { fields } => fields,
                other => {
                    panic!(
                        "each table column must be a map #{{header, cell, width}}, got {other:?}"
                    )
                }
            };
            let header = match fields.get("header") {
                Some(RenderExpr::Literal {
                    value: Value::String(s),
                }) => s.clone(),
                Some(other) => {
                    panic!("table column `header` must be a string literal, got {other:?}")
                }
                None => panic!("table column is missing `header`: {fields:?}"),
            };
            let cell = fields
                .get("cell")
                .cloned()
                .unwrap_or_else(|| panic!("table column `{header}` is missing `cell`"));
            let width = parse_width(fields.get("width"));
            let wraps = cell_wraps(&cell);
            ColumnDef {
                header,
                cell,
                width,
                wraps,
            }
        })
        .collect()
}

fn width_prop(w: &ColumnWidth) -> String {
    match w {
        ColumnWidth::Flex(weight) => format!("flex:{weight}"),
        ColumnWidth::Fixed(px) => format!("fixed:{px}"),
    }
}

/// Each column's FLOOR, in px: the width it would have in a container
/// `authored_w` wide.
///
/// Flex shares are proportional all the way down, so in a narrow container
/// every column ends up narrower than its own words and every cell breaks
/// mid-word. The floor turns the weights into what their author meant — a
/// budget for a container of a known size — and the renderer lets the row wrap
/// once the floors no longer fit side by side.
///
/// A fixed column's floor is its own px. A flex column's is its share of what
/// is left after the fixed columns and the gaps, which is the same arithmetic
/// the renderer does at `authored_w` — so at that width and above, nothing
/// about the layout changes.
///
/// A column that wraps its own content has NO floor. The floor exists because a
/// column cannot break its words; a column that breaks them is the case it was
/// never for, and giving it one both over-provisions it in a narrow container
/// and makes its own wrapping unreachable. Its share is still taken out of the
/// budget, so the columns that do have floors keep the widths they were
/// authored with.
fn column_minimums(columns: &[ColumnDef], authored_w: f32, gap: f32) -> Vec<Option<f32>> {
    let fixed_total: f32 = columns
        .iter()
        .filter_map(|c| match c.width {
            ColumnWidth::Fixed(px) => Some(px),
            ColumnWidth::Flex(_) => None,
        })
        .sum();
    let gaps = gap * columns.len().saturating_sub(1) as f32;
    let weight_total: f32 = columns
        .iter()
        .filter_map(|c| match c.width {
            ColumnWidth::Flex(w) => Some(w),
            ColumnWidth::Fixed(_) => None,
        })
        .sum();
    let flex_room = (authored_w - fixed_total - gaps).max(0.0);
    columns
        .iter()
        .map(|c| {
            if c.wraps {
                return None;
            }
            Some(match c.width {
                ColumnWidth::Fixed(px) => px,
                ColumnWidth::Flex(w) if weight_total > 0.0 => flex_room * w / weight_total,
                ColumnWidth::Flex(_) => 0.0,
            })
        })
        .collect()
}

/// The container width a `min_width:` names, refused rather than guessed.
///
/// Absent is a legal answer and means no floor — a table that never declared
/// the width it was sized for has no budget to hold its columns to.
fn parse_min_width(value: Option<&Value>) -> Option<f32> {
    let value = value?;
    let n = value
        .as_f64()
        .unwrap_or_else(|| panic!("table `min_width` must be a number of px, got {value:?}"));
    assert!(
        n > 0.0,
        "table `min_width` is the container width the column weights were authored against, so it \
         must be positive; got {n}"
    );
    Some(n as f32)
}

/// The gap BETWEEN columns. Shipped to the renderer as a prop rather than
/// hard-coded at both ends, because the column floors are computed from it here
/// and applied there — two copies of the number would put the floors and the
/// layout they are floors for quietly out of step.
const COLUMN_GAP: f32 = 8.0;

fn geometry_props(
    columns: &[ColumnDef],
    min_width: Option<f32>,
) -> std::collections::HashMap<String, Value> {
    let mut props = std::collections::HashMap::new();
    props.insert(
        "col_count".to_string(),
        Value::String(columns.len().to_string()),
    );
    props.insert("col_gap".to_string(), Value::Float(COLUMN_GAP as f64));
    let minimums = min_width.map(|w| column_minimums(columns, w, COLUMN_GAP));
    for (k, col) in columns.iter().enumerate() {
        props.insert(format!("col{k}_header"), Value::String(col.header.clone()));
        props.insert(
            format!("col{k}_width"),
            Value::String(width_prop(&col.width)),
        );
        // Three answers, not two. A floored column ships its px. A wrapping
        // column ships `col{k}_min_content`, which asks the renderer for the
        // flex default — as narrow as the column's own header word, and no
        // narrower, because a header is text and text does not wrap mid-word.
        // A table that declared no budget at all ships neither.
        match minimums.as_ref().map(|mins| mins[k]) {
            Some(Some(min)) => {
                props.insert(format!("col{k}_min"), Value::Float(min as f64));
            }
            Some(None) => {
                props.insert(format!("col{k}_min_content"), Value::Boolean(true));
            }
            None => {}
        }
    }
    props
}

/// The per-row item template: `row(cell0, …, cellC)`.
///
/// Positional children only. Named props on a collection's TOP-LEVEL item
/// template are dropped by the streaming interpret path
/// (`docs/Testing/bugfunnel/entries/
/// 2026-08-20-streaming-list-drops-named-props-on-item-column.md`), so the row'
/// s geometry deliberately lives on the table node instead of here.
fn row_template(columns: &[ColumnDef]) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "row".to_string(),
        args: columns
            .iter()
            .map(|col| holon_api::render_types::Arg {
                name: None,
                value: col.cell.clone(),
            })
            .collect(),
    }
}

holon_macros::widget_builder! {
    fn table(#[default = 4.0] gap: f32, children: Collection) {
        let __parent_space = ba.ctx.available_space;
        let virtual_child = match virtual_child_slot_from_arg(&ba) {
                    Ok(slot) => slot,
                    Err(msg) => return ViewModel::error("table", msg),
                };

        let Some(spec) = ba.args.get_template("columns") else {
            // Bare `table` — the collection widget it has always been.
            return match children {
                CollectionData::Streaming { item_template, data_source, sort_key, rules } => {
                    ViewModel::streaming_collection("table", item_template, data_source, gap, ItemFlow::Stacked, sort_key, __parent_space, None, virtual_child, rules, None, Default::default())
                }
                CollectionData::Static { mut items } => {
                    if let Some(tmpl) = ba.args.get_template("item_template").or(ba.args.get_template("item")) {
                        let vc = match interpret_virtual_child(&ba, tmpl) {
                            Ok(vc) => vc,
                            Err(msg) => return ViewModel::error("table", msg),
                        };
                        if let Some(vc) = vc {
                            items.push(vc);
                        }
                    }
                    let items = weave_advice_into_items(&ba, items);
                    ViewModel::static_collection("table", items, gap, ItemFlow::Stacked, Default::default())
                }
            };
        };

        // Columnar. `children` was resolved without an `item_template:`, so it
        // holds one empty `row` element per data row — discarded here, the row
        // template comes from `columns:` instead. Keeping the macro's
        // `Collection` param anyway is what makes the bare arm above provably
        // the original widget.
        let columns = parse_columns(spec);
        let props = geometry_props(&columns, parse_min_width(ba.args.named.get("min_width")));
        let item_template = row_template(&columns);
        let sort_key = holon_api::render_eval::sort_key_column(ba.args).map(|s| s.to_string());
        let rules = crate::row_pipeline::parse_rules_arg(ba.args.named.get("rules"));

        // Same data-source precedence the macro's `Collection` param applies:
        // an explicit `collection:` wins over the inherited `ctx.data_source`.
        let data_source = ba.args.get_rows("collection").or_else(|| {
            ba.ctx.data_source.clone().map(|r| r as std::sync::Arc<dyn holon_api::ReactiveRowProvider>)
        });

        match data_source {
            Some(ds) => ViewModel::streaming_collection(
                "table_columnar", item_template, ds, gap, ItemFlow::Stacked, sort_key,
                __parent_space, None, virtual_child, rules, None, props,
            ),
            None => {
                let sorted = holon_api::render_eval::sorted_rows(&ba.ctx.data_rows, sort_key.as_deref());
                let count = sorted.len();
                let items = sorted
                    .into_iter()
                    .enumerate()
                    .map(|(i, row)| {
                        let positional = std::collections::HashMap::from([
                            ("position".to_string(), Value::Integer(i as i64)),
                            ("count".to_string(), Value::Integer(count as i64)),
                            ("is_first".to_string(), Value::Boolean(i == 0)),
                            ("is_last".to_string(), Value::Boolean(i + 1 == count)),
                            ("is_empty_collection".to_string(), Value::Boolean(count == 0)),
                        ]);
                        let (node, _) = crate::row_pipeline::apply_full_row_pipeline(
                            ba.services,
                            ba.ctx,
                            &item_template,
                            &rules,
                            &row,
                            positional,
                            |expr, c| (ba.interpret)(expr, c),
                        );
                        node
                    })
                    .collect();
                ViewModel::static_collection("table_columnar", items, gap, ItemFlow::Stacked, props)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use holon_api::render_types::Arg;

    use super::*;

    fn lit(s: &str) -> RenderExpr {
        RenderExpr::Literal {
            value: Value::String(s.to_string()),
        }
    }

    fn num_fn(name: &str, n: f64) -> RenderExpr {
        RenderExpr::FunctionCall {
            name: name.to_string(),
            args: vec![Arg {
                name: None,
                value: RenderExpr::Literal {
                    value: Value::Float(n),
                },
            }],
        }
    }

    fn column(fields: Vec<(&str, RenderExpr)>) -> RenderExpr {
        RenderExpr::Object {
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect::<HashMap<_, _>>(),
        }
    }

    #[test]
    fn parses_flex_and_fixed_columns() {
        let spec = RenderExpr::Array {
            items: vec![
                column(vec![
                    ("header", lit("Provider")),
                    (
                        "cell",
                        RenderExpr::ColumnRef {
                            name: "provider_name".to_string(),
                        },
                    ),
                    ("width", num_fn("flex", 2.0)),
                ]),
                column(vec![
                    ("header", lit("Status")),
                    (
                        "cell",
                        RenderExpr::ColumnRef {
                            name: "status".to_string(),
                        },
                    ),
                    ("width", num_fn("fixed", 120.0)),
                ]),
            ],
        };
        let cols = parse_columns(&spec);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].header, "Provider");
        assert!(matches!(cols[0].width, ColumnWidth::Flex(w) if (w - 2.0).abs() < f32::EPSILON));
        assert_eq!(cols[1].header, "Status");
        assert!(
            matches!(cols[1].width, ColumnWidth::Fixed(px) if (px - 120.0).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn defaults_missing_width_to_flex_one() {
        let spec = RenderExpr::Array {
            items: vec![column(vec![("header", lit("A")), ("cell", lit("x"))])],
        };
        let cols = parse_columns(&spec);
        assert!(matches!(cols[0].width, ColumnWidth::Flex(w) if (w - 1.0).abs() < f32::EPSILON));
    }

    #[test]
    #[should_panic(expected = "missing `header`")]
    fn rejects_column_without_header() {
        let spec = RenderExpr::Array {
            items: vec![column(vec![("cell", lit("x"))])],
        };
        parse_columns(&spec);
    }

    #[test]
    #[should_panic(expected = "content-max/auto width is unsupported")]
    fn rejects_content_max_width() {
        let spec = RenderExpr::Array {
            items: vec![column(vec![
                ("header", lit("A")),
                ("cell", lit("x")),
                (
                    "width",
                    RenderExpr::FunctionCall {
                        name: "auto".to_string(),
                        args: vec![],
                    },
                ),
            ])],
        };
        parse_columns(&spec);
    }

    #[test]
    #[should_panic(expected = "must be a list of column maps")]
    fn rejects_non_array_spec() {
        parse_columns(&lit("not a list"));
    }
}
