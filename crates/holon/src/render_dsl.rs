//! Render DSL: Rhai-based language for defining render expressions.
//!
//! Render source blocks contain Rhai expressions that produce a render tree.
//! Each widget function (columns, list, tree, text, icon, etc.) is registered
//! as a Rhai function that returns a Dynamic map describing a RenderExpr node.
//!
//! ## Syntax examples
//!
//! ```rhai
//! columns(#{gap: 4, item_template: block_ref()})
//!
//! list(#{sortkey: "name", item_template: render_block()})
//!
//! tree(#{parent_id: col("parent_id"), sortkey: col("id"), item_template: render_block()})
//!
//! list(#{item_template: selectable(
//!     row(icon("folder"), spacer(6), text(col("name"))),
//!     #{action: navigation_focus(#{region: "main", block_id: col("doc_uri")})}
//! )})
//! ```
//!
//! ## Special functions
//!
//! - `col("name")` — column reference (resolved per-row at render time)
//! - All other functions map to RenderExpr::FunctionCall nodes

use std::collections::HashMap;

use anyhow::{Context, Result};
use holon_api::Value;
use holon_api::render_types::{Arg, RenderExpr};
use rhai::{Dynamic, Engine as RhaiEngine, Map as RhaiMap};

/// Parse a render DSL string into a RenderExpr.
pub fn parse_render_dsl(source: &str) -> Result<RenderExpr> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Ok(default_table());
    }

    // Try JSON first (backwards compat)
    if let Ok(expr) = serde_json::from_str::<RenderExpr>(trimmed) {
        return Ok(expr);
    }

    let engine = create_render_engine();
    let result = engine
        .eval_expression::<Dynamic>(trimmed)
        .map_err(|e| anyhow::anyhow!("Failed to parse render DSL '{}': {e}", trimmed))?;

    dynamic_to_render_expr(&result)
        .with_context(|| format!("Failed to convert Rhai result to RenderExpr: {:?}", &result))
}

fn default_table() -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "table".to_string(),
        args: Vec::new(),
        operations: Vec::new(),
    }
}

/// Create a Rhai engine with all render functions registered.
fn create_render_engine() -> RhaiEngine {
    let mut engine = RhaiEngine::new();

    // col("name") → ColumnRef marker
    engine.register_fn("col", |name: &str| -> Dynamic {
        let mut map = RhaiMap::new();
        map.insert("_type".into(), "col".into());
        map.insert("name".into(), Dynamic::from(name.to_string()));
        Dynamic::from(map)
    });

    // Register all widget functions with 0-arg and multi-arg overloads.
    // Each returns a Dynamic map: #{_type: "fn", _name: "...", _args: [...]}
    let widget_names = [
        // Layout
        "columns",
        "list",
        "tree",
        "outline",
        "row",
        "block",
        "section",
        "stack",
        "grid",
        "scroll",
        "flexible",
        "table",
        // Leaf
        "spacer",
        "badge",
        "icon",
        "bullet",
        "text",
        "checkbox",
        "collapse_button",
        "date_header",
        "progress",
        "count_badge",
        "status_indicator",
        // Interactive
        "selectable",
        "drop_zone",
        "block_operations",
        "editable_text",
        "pie_menu",
        "focusable",
        "draggable",
        "source_block",
        "source_editor",
        "query_result",
        "live_query",
        "block_ref",
        "render_block",
        "state_toggle",
        // Animation
        "animated",
        "hover_row",
        "staggered",
        "pulse",
        // Navigation / actions (dotted names registered separately)
    ];

    for name in widget_names {
        register_widget_fn(&mut engine, name);
    }

    // Dotted names: Rhai doesn't allow dots in function names,
    // so users write underscores and we map to the dotted name.
    register_widget_fn_aliased(&mut engine, "navigation_focus", "navigation.focus");

    engine
}

/// Register a widget function where the Rhai name differs from the output name.
/// E.g., `navigation_focus` in Rhai → `navigation.focus` in RenderExpr.
fn register_widget_fn_aliased(engine: &mut RhaiEngine, rhai_name: &str, output_name: &str) {
    let n = output_name.to_string();

    let n0 = n.clone();
    engine.register_fn(rhai_name, move || -> Dynamic { make_fn_node(&n0, vec![]) });
    let n1 = n.clone();
    engine.register_fn(rhai_name, move |a: Dynamic| -> Dynamic {
        make_fn_node(&n1, vec![a])
    });
    let n2 = n.clone();
    engine.register_fn(rhai_name, move |a: Dynamic, b: Dynamic| -> Dynamic {
        make_fn_node(&n2, vec![a, b])
    });
    let n3 = n.clone();
    engine.register_fn(
        rhai_name,
        move |a: Dynamic, b: Dynamic, c: Dynamic| -> Dynamic { make_fn_node(&n3, vec![a, b, c]) },
    );
}

fn register_widget_fn(engine: &mut RhaiEngine, name: &str) {
    let n = name.to_string();

    // 0-arg: block_ref()
    let n0 = n.clone();
    engine.register_fn(name, move || -> Dynamic { make_fn_node(&n0, vec![]) });

    // 1-arg: columns(#{gap: 4}) or text("hello") or text(col("name"))
    let n1 = n.clone();
    engine.register_fn(name, move |arg: Dynamic| -> Dynamic {
        make_fn_node(&n1, vec![arg])
    });

    // 2-arg: selectable(child, #{action: ...})
    let n2 = n.clone();
    engine.register_fn(name, move |a: Dynamic, b: Dynamic| -> Dynamic {
        make_fn_node(&n2, vec![a, b])
    });

    // 3-arg: row(a, b, c)
    let n3 = n.clone();
    engine.register_fn(name, move |a: Dynamic, b: Dynamic, c: Dynamic| -> Dynamic {
        make_fn_node(&n3, vec![a, b, c])
    });

    // 4-arg
    let n4 = n.clone();
    engine.register_fn(
        name,
        move |a: Dynamic, b: Dynamic, c: Dynamic, d: Dynamic| -> Dynamic {
            make_fn_node(&n4, vec![a, b, c, d])
        },
    );

    // 5-arg
    let n5 = n.clone();
    engine.register_fn(
        name,
        move |a: Dynamic, b: Dynamic, c: Dynamic, d: Dynamic, e: Dynamic| -> Dynamic {
            make_fn_node(&n5, vec![a, b, c, d, e])
        },
    );

    // 6-arg
    let n6 = n.clone();
    engine.register_fn(
        name,
        move |a: Dynamic, b: Dynamic, c: Dynamic, d: Dynamic, e: Dynamic, f: Dynamic| -> Dynamic {
            make_fn_node(&n6, vec![a, b, c, d, e, f])
        },
    );
}

/// Build a function node Dynamic.
///
/// Convention: the LAST Map argument is treated as named args.
/// All other arguments are positional.
fn make_fn_node(name: &str, args: Vec<Dynamic>) -> Dynamic {
    let mut map = RhaiMap::new();
    map.insert("_type".into(), "fn".into());
    map.insert("_name".into(), Dynamic::from(name.to_string()));

    let mut positional = Vec::new();
    let mut named = RhaiMap::new();

    for arg in args.into_iter() {
        if arg.is_map() {
            // Check if it's a col() marker or a fn node — those are positional
            let m = arg.clone().cast::<RhaiMap>();
            if m.contains_key("_type") {
                positional.push(arg);
            } else {
                // Plain map = named args (merge into named)
                for (k, v) in m {
                    named.insert(k, v);
                }
            }
        } else {
            positional.push(arg);
        }
    }

    map.insert("_positional".into(), Dynamic::from(positional));
    map.insert("_named".into(), Dynamic::from(named));
    Dynamic::from(map)
}

// ---------------------------------------------------------------------------
// Dynamic → RenderExpr conversion
// ---------------------------------------------------------------------------

fn dynamic_to_render_expr(d: &Dynamic) -> Result<RenderExpr> {
    if d.is_map() {
        let map = d.clone().cast::<RhaiMap>();

        let typ = map
            .get("_type")
            .and_then(|v| v.clone().into_string().ok())
            .unwrap_or_default();

        match typ.as_str() {
            "fn" => map_to_function_call(&map),
            "col" => {
                let name = map
                    .get("name")
                    .and_then(|v| v.clone().into_string().ok())
                    .unwrap_or_default();
                Ok(RenderExpr::ColumnRef { name })
            }
            _ => {
                // Plain map without _type — treat as Object
                let mut fields = HashMap::new();
                for (k, v) in &map {
                    fields.insert(k.to_string(), dynamic_to_render_expr(v)?);
                }
                Ok(RenderExpr::Object { fields })
            }
        }
    } else if d.is_array() {
        let arr = d.clone().cast::<Vec<Dynamic>>();
        let items: Result<Vec<_>> = arr.iter().map(dynamic_to_render_expr).collect();
        Ok(RenderExpr::Array { items: items? })
    } else {
        // Primitive → Literal
        Ok(RenderExpr::Literal {
            value: dynamic_to_value(d),
        })
    }
}

fn map_to_function_call(map: &RhaiMap) -> Result<RenderExpr> {
    let name = map
        .get("_name")
        .and_then(|v| v.clone().into_string().ok())
        .unwrap_or_else(|| "unknown".to_string());

    let mut args = Vec::new();

    // Named args
    if let Some(named_dyn) = map.get("_named") {
        if named_dyn.is_map() {
            let named = named_dyn.clone().cast::<RhaiMap>();
            for (k, v) in &named {
                args.push(Arg {
                    name: Some(k.to_string()),
                    value: dynamic_to_render_expr(v)?,
                });
            }
        }
    }

    // Positional args
    if let Some(pos_dyn) = map.get("_positional") {
        if pos_dyn.is_array() {
            let positional = pos_dyn.clone().cast::<Vec<Dynamic>>();
            for v in &positional {
                args.push(Arg {
                    name: None,
                    value: dynamic_to_render_expr(v)?,
                });
            }
        }
    }

    Ok(RenderExpr::FunctionCall {
        name,
        args,
        operations: Vec::new(),
    })
}

fn dynamic_to_value(d: &Dynamic) -> Value {
    if d.is_int() {
        Value::Integer(d.as_int().unwrap())
    } else if d.is_float() {
        Value::Float(d.as_float().unwrap())
    } else if d.is_bool() {
        Value::Boolean(d.as_bool().unwrap())
    } else if d.is_string() {
        Value::String(d.clone().into_string().unwrap())
    } else if d.is_unit() {
        Value::Null
    } else {
        Value::String(format!("{d:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_function() {
        let expr = parse_render_dsl("table()").unwrap();
        assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "table"));
    }

    #[test]
    fn test_block_ref() {
        let expr = parse_render_dsl("block_ref()").unwrap();
        assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "block_ref"));
    }

    #[test]
    fn test_columns_with_named_args() {
        let expr = parse_render_dsl("columns(#{gap: 4, item_template: block_ref()})").unwrap();
        if let RenderExpr::FunctionCall { name, args, .. } = &expr {
            assert_eq!(name, "columns");
            assert!(args.iter().any(|a| a.name.as_deref() == Some("gap")));
            assert!(
                args.iter()
                    .any(|a| a.name.as_deref() == Some("item_template"))
            );
        } else {
            panic!("Expected FunctionCall");
        }
    }

    #[test]
    fn test_col_reference() {
        let expr = parse_render_dsl("text(col(\"name\"))").unwrap();
        if let RenderExpr::FunctionCall { args, .. } = &expr {
            assert!(matches!(&args[0].value, RenderExpr::ColumnRef { name } if name == "name"));
        } else {
            panic!("Expected FunctionCall");
        }
    }

    #[test]
    fn test_nested_functions() {
        let expr =
            parse_render_dsl(r#"row(icon("folder"), spacer(6), text(col("name")))"#).unwrap();
        if let RenderExpr::FunctionCall { name, args, .. } = &expr {
            assert_eq!(name, "row");
            assert_eq!(args.len(), 3);
        } else {
            panic!("Expected FunctionCall");
        }
    }

    #[test]
    fn test_tree_with_col_refs() {
        let expr = parse_render_dsl(r#"tree(#{parent_id: col("parent_id"), sortkey: col("id"), item_template: render_block()})"#).unwrap();
        if let RenderExpr::FunctionCall { name, args, .. } = &expr {
            assert_eq!(name, "tree");
            let parent_arg = args
                .iter()
                .find(|a| a.name.as_deref() == Some("parent_id"))
                .unwrap();
            assert!(
                matches!(&parent_arg.value, RenderExpr::ColumnRef { name } if name == "parent_id")
            );
        } else {
            panic!("Expected FunctionCall");
        }
    }

    #[test]
    fn test_json_backwards_compat() {
        let json = r#"{"FunctionCall":{"name":"table","args":[],"operations":[]}}"#;
        let expr = parse_render_dsl(json).unwrap();
        assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "table"));
    }

    #[test]
    fn test_empty_defaults_to_table() {
        let expr = parse_render_dsl("").unwrap();
        assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "table"));
    }
}
