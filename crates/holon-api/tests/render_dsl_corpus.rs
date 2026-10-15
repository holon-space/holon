//! A hand-written corpus for `parse_render_dsl`: the source on the left, the
//! root widget and the string literals a reader expects on the right.
//!
//! The parser has no independent oracle anywhere else. The keystone asks it for
//! the expected render of a render-source block, so parser and SUT move
//! together, and its only cargo-mutants mutant is unviable (`RenderExpr` has no
//! `Default`), which reports "0 missed" while measuring nothing. The literals
//! are spelled out here so a parse that drops or reorders an argument, or that
//! answers an unparsable source with a default table, goes red.

use holon_api::render_dsl::parse_render_dsl;
use holon_api::render_types::RenderExpr;

/// Every string literal of `expr` in walk order (arguments left to right,
/// depth first) — the text a frontend would render.
fn text_literals(expr: &RenderExpr) -> Vec<String> {
    let mut out = Vec::new();
    if let RenderExpr::Literal {
        value: holon_api::Value::String(s),
    } = expr
    {
        out.push(s.to_string());
    }
    for child in expr.children() {
        out.extend(text_literals(child));
    }
    out
}

fn root_call(expr: &RenderExpr) -> &str {
    match expr {
        RenderExpr::FunctionCall { name, .. } => name,
        other => panic!("a render source must parse to a widget call, got {other:?}"),
    }
}

#[test]
fn the_corpus_parses_to_its_hand_written_shape() {
    let corpus: &[(&str, &str, &[&str])] = &[
        // An empty source is the one SPECIFIED default: the table widget.
        ("", "table", &[]),
        ("table()", "table", &[]),
        (r#"text("hello")"#, "text", &["hello"]),
        (r#"row(text("a"), text("b"))"#, "row", &["a", "b"]),
        (
            r#"column(row(icon("sync"), text(col("content"))))"#,
            "column",
            &["sync"],
        ),
        (
            r#"text(if col("done") { "yes" } else { "no" })"#,
            "text",
            &["yes", "no"],
        ),
        (
            r#"list(#{item_template: text(col("content"))})"#,
            "list",
            &[],
        ),
        // A map argument becomes NAMED args, and `function_call` puts those
        // before the positional ones — so the prop is walked first.
        (r#"text("x", #{color: "muted"})"#, "text", &["muted", "x"]),
        (
            r#"tree(#{parent_id: col("parent_id"), item_template: text("leaf")})"#,
            "tree",
            &["leaf"],
        ),
    ];

    for (source, root, literals) in corpus {
        let expr =
            parse_render_dsl(source).unwrap_or_else(|e| panic!("`{source}` must parse, got {e:#}"));
        assert_eq!(root_call(&expr), *root, "root widget of `{source}`");
        assert_eq!(
            text_literals(&expr),
            literals.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "text literals of `{source}`"
        );
    }
}

/// A source the parser cannot read is an `Err`, never a default table: the
/// block's author must learn that nothing of their layout survived.
#[test]
fn an_unparsable_source_is_refused() {
    for source in [
        r#"text("unclosed""#,
        "row(,)",
        r#"text("x") text("y")"#,
        "#{",
    ] {
        let err = parse_render_dsl(source)
            .err()
            .unwrap_or_else(|| panic!("`{source}` must be refused, not answered with a default"));
        assert!(
            err.to_string().contains("render DSL") || err.to_string().contains("Failed to parse"),
            "the refusal must name the source: {err:#}"
        );
    }
}
