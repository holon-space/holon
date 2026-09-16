//! An unknown colour name in a render source block is refused when the block is
//! PARSED.
//!
//! The refusal cannot live only in the widget builders. For the props-only
//! widgets (`text`, `icon`, ...) a collection's `item_template` takes the
//! `resolve_props` fast path, which re-derives props from the same expression
//! and never runs the builder — so a bad colour inside an item template would
//! reach the frame loop, where the only options are a panic or a silent
//! substitution. Validating the parsed expression covers both paths, at the
//! same moment a bad `live_query(source: ...)` is refused.
//!
//! These cases pin the two shapes that matter: the item-template shape the
//! builder cannot see, and the per-row shape that must NOT be refused, because
//! `color: col("kind")` names no colour until a row exists.

use holon_api::render_dsl::parse_render_dsl;

#[test]
fn an_unknown_colour_literal_is_refused() {
    let err = parse_render_dsl(r#"text("x", #{color: "corrigendum"})"#)
        .expect_err("a colour no theme defines must be refused when the block is parsed");
    let msg = err.to_string();
    assert!(msg.contains("corrigendum"), "{msg}");
    assert!(msg.contains("THEME_TOKENS"), "{msg}");
}

/// The shape the builder-level refusal cannot reach: an item template whose
/// first call is `text`, so every row's props come from the fast path.
#[test]
fn an_unknown_colour_inside_an_item_template_is_refused() {
    let err =
        parse_render_dsl(r#"list(#{item_template: text(col("content"), #{color: "chartreuse"})})"#)
            .expect_err(
                "an item template's colour must be refused too, or the fast path never sees it",
            );
    let msg = err.to_string();
    assert!(msg.contains("chartreuse"), "{msg}");
}

#[test]
fn an_unknown_accent_is_refused() {
    let err = parse_render_dsl(r#"card(#{accent: "threshold"}, text("x"))"#)
        .expect_err("`accent` is a colour argument and gets the same refusal");
    assert!(err.to_string().contains("threshold"), "{err}");
}

/// A colour nested below the widget that carries it is still the layout's
/// colour, so the walk must reach it.
#[test]
fn a_nested_colour_is_refused() {
    let err = parse_render_dsl(r#"column(row(icon("sync"), text("x", #{color: "not_a_colour"})))"#)
        .expect_err("a nested colour must be refused as well as a top-level one");
    assert!(err.to_string().contains("not_a_colour"), "{err}");

    let err = parse_render_dsl(r#"list(#{item_template: row(text("x", #{color: "nope"}))})"#)
        .expect_err("a colour inside a nested collection template must be refused");
    assert!(err.to_string().contains("nope"), "{err}");
}

#[test]
fn a_known_colour_parses() {
    for source in [
        r#"text("x", #{color: "muted"})"#,
        r#"text("x", #{color: "primary"})"#,
        r#"icon("sync", #{color: "success"})"#,
        r#"card(#{accent: "accent"}, text("x"))"#,
    ] {
        parse_render_dsl(source).unwrap_or_else(|e| panic!("{source} must parse, got {e}"));
    }
}

/// A literal hex is refused, not carried: a layout that wants `#3B82F6` wants a
/// colour that ignores the active theme, and both a light and a dark ship.
#[test]
fn a_hex_literal_is_refused() {
    let err = parse_render_dsl(r##"text("x", #{color: "#3B82F6"})"##)
        .expect_err("a hex colour must be refused rather than carried to the renderer");
    assert!(err.to_string().contains("#3B82F6"), "{err}");
}

/// A per-row colour names no colour yet, so there is nothing to refuse here.
/// Refusing it would break the shape the generic lever is built on.
#[test]
fn a_per_row_colour_is_not_refused() {
    parse_render_dsl(r#"text(col("content"), #{color: col("kind")})"#)
        .expect("a column-bound colour is resolved per row and must parse");
}
