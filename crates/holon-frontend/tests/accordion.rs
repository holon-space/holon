//! Headless prop-parsing + placement coverage for the `accordion` widget
//! (parse-don't-validate, §3). Mirrors `divider.rs`: parse the render DSL,
//! interpret through the shadow builder, assert on the resulting
//! `ReactiveViewModel`.

use holon_api::render_types::RenderExpr;
use holon_frontend::LayoutHint;
use holon_frontend::ReactiveViewModel;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;

fn interpret(dsl: &str) -> ReactiveViewModel {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let parsed: RenderExpr =
        holon_api::render_dsl::parse_render_dsl(dsl).expect("dsl should parse");
    let services = StubBuilderServices::new();
    let ctx = RenderContext::default();
    services.interpret(&parsed, &ctx)
}

fn is_error(vm: &ReactiveViewModel) -> bool {
    vm.widget_name().as_deref() == Some("error")
}

#[test]
fn accordion_default_props_as_column_child() {
    let vm = interpret("column(accordion(#{title: \"Linked references\"}))");
    assert_eq!(vm.widget_name().as_deref(), Some("column"));
    let acc = &vm.children[0];
    assert_eq!(acc.widget_name().as_deref(), Some("accordion"));
    assert_eq!(acc.prop_str("title").as_deref(), Some("Linked references"));
    // Defaults per §3.
    assert_eq!(acc.prop_f64("max_height_fraction"), Some(0.4));
    assert_eq!(acc.prop_bool("collapsible"), Some(true));
    assert_eq!(acc.prop_bool("collapsed"), Some(false));
    // Live collapse state seeded from `collapsed` on the node's Mutable.
    assert_eq!(
        acc.expanded.as_ref().map(|m| m.get()),
        Some(true),
        "expanded should be seeded to !collapsed"
    );
}

#[test]
fn accordion_explicit_props_parse() {
    let vm = interpret(
        "column(accordion(#{title: \"Refs\", icon: \"link\", \
         max_height_fraction: 0.33, collapsible: false, collapsed: true}))",
    );
    let acc = &vm.children[0];
    assert_eq!(acc.widget_name().as_deref(), Some("accordion"));
    assert_eq!(acc.prop_str("icon").as_deref(), Some("link"));
    assert_eq!(acc.prop_f64("max_height_fraction"), Some(0.33));
    assert_eq!(acc.prop_bool("collapsible"), Some(false));
    assert_eq!(acc.prop_bool("collapsed"), Some(true));
    assert_eq!(acc.expanded.as_ref().map(|m| m.get()), Some(false));
}

#[test]
fn accordion_zero_fraction_is_error_widget() {
    let vm = interpret("column(accordion(#{max_height_fraction: 0.0}))");
    assert!(
        is_error(&vm.children[0]),
        "fraction 0.0 must render the error widget, not a silently-clamped region"
    );
}

#[test]
fn accordion_over_one_fraction_is_error_widget() {
    let vm = interpret("column(accordion(#{max_height_fraction: 2.0}))");
    assert!(
        is_error(&vm.children[0]),
        "fraction 2.0 must be an error widget"
    );
}

#[test]
fn accordion_negative_fraction_is_error_widget() {
    let vm = interpret("column(accordion(#{max_height_fraction: -1.0}))");
    assert!(
        is_error(&vm.children[0]),
        "negative fraction must be an error widget"
    );
}

#[test]
fn pinned_accordion_declares_the_pin_hint() {
    let vm = interpret("column(accordion(#{title: \"Linked references\"}))");
    assert_eq!(
        vm.children[0].layout_hint,
        LayoutHint::PinnedToEnd,
        "a pinned accordion must DECLARE where it belongs — that hint is what \
         every renderer reads instead of matching on the widget name"
    );
}

#[test]
fn section_stack_accordions_declare_no_pin() {
    for dsl in [
        "section_stack(accordion(#{title: \"in flow\", pinned: false}))",
        "section_stack(accordion(#{title: \"sticky\", sticky: true}))",
    ] {
        let vm = interpret(dsl);
        let acc = &vm.children[0];
        assert_eq!(acc.widget_name().as_deref(), Some("accordion"), "{dsl}");
        assert_eq!(
            acc.layout_hint,
            LayoutHint::default(),
            "scroll-integrated placements are not pinned to the container edge: {dsl}"
        );
    }
}

// ── Placement guard (§3, senior-review amendment): an accordion whose
// container    cannot honour its placement must render the error widget. ──

#[test]
fn pinned_accordion_inside_a_section_stack_is_error_widget() {
    let vm = interpret("section_stack(accordion(#{title: \"pinned\"}))");
    assert!(
        is_error(&vm.children[0]),
        "a section stack offers scroll-integrated sections, not trailing-edge pinning"
    );
}

#[test]
fn in_flow_accordion_inside_a_column_is_error_widget() {
    let vm = interpret("column(accordion(#{title: \"in flow\", pinned: false}))");
    assert!(
        is_error(&vm.children[0]),
        "a column offers trailing-edge pinning, not scroll-integrated sections"
    );
}

#[test]
fn accordion_at_root_is_error_widget() {
    let vm = interpret("accordion(#{title: \"orphan\"})");
    assert!(
        is_error(&vm),
        "a top-level accordion (no column parent) must be the error widget, \
         never a silently-unbounded region"
    );
}

#[test]
fn accordion_inside_row_is_error_widget() {
    // The column offers `PinToEnd` to its DIRECT children only — the offer is
    // stripped one level down, so the accordion inside the `row` sees nothing
    // and errors loudly.
    let vm = interpret("column(row(accordion(#{title: \"nested\"})))");
    let row = &vm.children[0];
    assert_eq!(row.widget_name().as_deref(), Some("row"));
    assert!(
        is_error(&row.children[0]),
        "an accordion nested inside a row is not a direct column child"
    );
}

#[test]
fn accordion_inside_accordion_inner_is_error_widget() {
    let vm = interpret("column(accordion(#{title: \"outer\"}, accordion(#{title: \"inner\"})))");
    let outer = &vm.children[0];
    assert_eq!(outer.widget_name().as_deref(), Some("accordion"));
    assert!(
        is_error(&outer.children[0]),
        "an accordion nested directly inside another accordion is misplaced"
    );
}
