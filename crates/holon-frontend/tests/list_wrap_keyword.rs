//! `list(#{horizontal: …, wrap: …})` — the two layout keywords, at the DSL
//! boundary.
//!
//! Together they decide whether a collection stacks its items, lays them side
//! by side, or lays them side by side and continues on a second line when it
//! runs short of width (the Settings Setup column's operation buttons,
//! `settings_setup_column_wraps_windowed`). Nothing downstream can tell a
//! mistyped keyword from an absent one, so a keyword that is not understood
//! must be refused HERE — quietly taking the default puts a button outside its
//! column and says nothing.
//!
//! The refusals are the point of this file, and the TYPE is what used to be
//! dropped. A wrong VALUE (`wrap: "maybe"`) was always loud; `wrap: true` and
//! `horizontal: "true"` were read as "keyword absent" and the collection laid
//! itself out the other way in silence — the `text(#{style: …})` shape the
//! boundary exists to prevent. Every collection builder and the view-mode
//! click share this parse: `every_builder_survives_authored_args.rs` holds
//! that property.
//!
//! @pbt kind harness
//! @pbt covers list-layout-keywords — a collection's stacking and wrapping mode
//! is parsed at the DSL boundary and every unrecognised `horizontal:` / `wrap:`
//! is refused loudly
//! @pbt slips-if-removed a mistyped layout keyword lays the collection out the
//! other way with no warning, and the only symptom is a button painted outside
//! its column

use std::sync::Arc;

use holon_api::render_types::RenderExpr;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ItemFlow;
use holon_frontend::reactive_view_model::ReactiveViewModel;

/// Parsed from real DSL source rather than hand-built, so the test exercises
/// the same arg shape a layout author produces.
fn expr(wrap_arg: &str) -> RenderExpr {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let src = format!(
        r#"list(#{{gap: 8, horizontal: true{wrap_arg}, item_template: text("x")}}, text("a"), text("b"))"#
    );
    holon_api::render_dsl::parse_render_dsl(&src)
        .unwrap_or_else(|e| panic!("the list source must parse ({src}): {e}"))
}

/// The same, with the whole named-arg set under the caller's control — the
/// `horizontal:` and `gap:` cases need sources that [`expr`] hardcodes.
fn expr_body(named_args: &str) -> RenderExpr {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let src = format!(r#"list(#{{{named_args}}}, text("a"), text("b"))"#);
    holon_api::render_dsl::parse_render_dsl(&src)
        .unwrap_or_else(|e| panic!("the list source must parse ({src}): {e}"))
}

/// Interpret through the prod shadow-builder path.
fn build(e: &RenderExpr) -> ReactiveViewModel {
    let services = StubBuilderServices::new();
    services.interpret(e, &RenderContext::default())
}

/// The builder draws an error node whose message contains `needle`.
fn builder_refuses(e: &RenderExpr, needle: &str) {
    let json = serde_json::to_value(build(e).snapshot()).expect("a ViewModel serializes");
    assert_eq!(
        json["widget"], "error",
        "the list must draw an error node: {json}"
    );
    let message = json["message"]
        .as_str()
        .expect("an error node carries a message");
    assert!(
        message.contains(needle),
        "the error must name {needle:?}: {message:?}"
    );
}

fn flow_of(vm: &ReactiveViewModel) -> ItemFlow {
    vm.collection
        .as_ref()
        .and_then(|v: &Arc<holon_frontend::reactive_view::ReactiveView>| v.layout())
        .unwrap_or_else(|| panic!("a `list` must build a collection view with a layout"))
        .flow
}

// ── The keyword is understood ──────────────────────────────────────────────

#[test]
fn wrap_reaches_the_collection_layout() {
    let e = expr(r#", wrap: "wrap""#);
    assert_eq!(
        flow_of(&build(&e)),
        ItemFlow::WrappingRow,
        "the shadow builder must carry the wrapping mode into the collection's layout"
    );
}

#[test]
fn nowrap_is_the_same_as_naming_no_wrap_at_all() {
    assert_eq!(flow_of(&build(&expr(r#", wrap: "nowrap""#))), ItemFlow::Row);
    assert_eq!(flow_of(&build(&expr(""))), ItemFlow::Row);
}

// ── Every unrecognised `wrap:` is refused ──────────────────────────────────

#[test]
fn a_bool_where_a_keyword_belongs_is_refused_by_the_builder() {
    builder_refuses(&expr(", wrap: true"), "Boolean(true)");
}

#[test]
fn a_number_where_a_keyword_belongs_is_refused() {
    builder_refuses(&expr(", wrap: 1"), "Integer(1)");
}

#[test]
fn an_unknown_keyword_is_refused_by_the_builder() {
    builder_refuses(&expr(r#", wrap: "maybe""#), "names no wrapping mode");
}

#[test]
fn wrapping_a_stacked_collection_names_nothing() {
    let src = r#"list(#{gap: 8, wrap: "wrap"}, text("a"))"#;
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let e = holon_api::render_dsl::parse_render_dsl(src).expect("source parses");
    builder_refuses(&e, "without `horizontal: true`");
}

// ── `horizontal:` is judged by the same parse ──────────────────────────────

#[test]
fn naming_no_horizontal_stacks_the_items() {
    let e = expr_body("gap: 8");
    assert_eq!(flow_of(&build(&e)), ItemFlow::Stacked);
}

#[test]
fn a_string_where_a_boolean_belongs_is_refused_by_the_builder() {
    builder_refuses(
        &expr_body(r#"gap: 8, horizontal: "true""#),
        "String(\"true\")",
    );
}

#[test]
fn a_number_where_a_boolean_belongs_is_refused() {
    builder_refuses(&expr_body("gap: 8, horizontal: 1"), "Integer(1)");
}

// ── `gap:` is judged by the same parse ─────────────────────────────────────
// Same shape, one line above the two keywords: a `gap:` of the wrong type was
// dropped and the layout's declared default silently took its place.

#[test]
fn a_named_gap_reaches_the_collection_layout() {
    let e = expr_body("gap: 12");
    assert_eq!(
        build(&e)
            .collection
            .as_ref()
            .and_then(|v| v.layout())
            .expect("a `list` builds a collection view")
            .gap,
        12.0,
        "the shadow builder must read the authored gap, not the layout default"
    );
}

#[test]
fn a_string_where_a_gap_belongs_is_refused_by_the_builder() {
    builder_refuses(&expr_body(r#"gap: "8", horizontal: true"#), "String(\"8\")");
}
