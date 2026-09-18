//! Render-spec id arguments are authored text: a `live_query`'s context comes
//! from the `id` column of the row it sits in — a value the vault's own SQL
//! chooses — and `entity_uri:` / `virtual_parent:` are typed by the vault
//! author. A value that forms no URI must paint an error node naming it, never
//! unwind inside the URI constructor and take the window with it.

use std::sync::Arc;

use futures_signals::signal::Mutable;
use holon_api::Value;
use holon_api::widget_spec::DataRow;
use holon_frontend::ReactiveViewModel;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::render_interpreter::render_spec_block_uri;

fn interpret(dsl: &str, ctx: &RenderContext) -> ReactiveViewModel {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let parsed = holon_api::render_dsl::parse_render_dsl(dsl).expect("dsl should parse");
    StubBuilderServices::new().interpret(&parsed, ctx)
}

/// A context row as a collection's per-row render builds it.
fn row_context(id: &str) -> RenderContext {
    let mut row = DataRow::new();
    row.insert("id".to_string(), Value::String(id.to_string()));
    RenderContext::default().with_row_mutable(Mutable::new(Arc::new(row)).read_only())
}

fn message(vm: &ReactiveViewModel) -> String {
    assert_eq!(
        vm.widget_name().as_deref(),
        Some("error"),
        "expected an error node, got {:?}",
        vm.widget_name()
    );
    vm.prop_str("message").unwrap_or_default()
}

/// `live_query(#{sql: "SELECT 1"})` in a context row whose `id` is not a URI —
/// what `live_query(#{sql: "SELECT 'my task' AS id", item_template:
/// list(#{item_template: live_query(#{sql: "SELECT 1"})})})` puts there.
#[test]
fn a_live_query_context_id_that_forms_no_uri_paints_an_error_node() {
    let vm = interpret("live_query(#{sql: \"SELECT 1\"})", &row_context("my task"));

    assert!(message(&vm).contains("my task"), "{}", message(&vm));
}

/// A `view_mode_switcher`'s `entity_uri:` is the entity whose modes it
/// switches.
#[test]
fn a_view_mode_switcher_entity_uri_that_forms_no_uri_paints_an_error_node() {
    let vm = interpret(
        "view_mode_switcher(#{entity_uri: \"my task\"}, list())",
        &RenderContext::default(),
    );

    assert!(message(&vm).contains("my task"), "{}", message(&vm));
}

/// `creation_slot: true` is the opt-in for the trailing "type here to create"
/// row; `virtual_parent:` names the block it is parented under.
#[test]
fn a_virtual_parent_that_forms_no_uri_paints_an_error_node() {
    let vm = interpret(
        "list(#{creation_slot: true, virtual_parent: \"my task\", \
         item_template: text(col(\"content\"))})",
        &RenderContext::default(),
    );

    assert!(message(&vm).contains("my task"), "{}", message(&vm));
}

/// A valid id resolves exactly as before, so only the failing case changed.
#[test]
fn a_context_id_resolves_bare_or_schemed_alike() {
    let uri = |id: &str| render_spec_block_uri("context", id).expect("a valid id");

    assert_eq!(uri("abc").as_str(), "block:abc");
    assert_eq!(uri("block:abc").as_str(), "block:abc");
}

#[test]
fn a_context_id_that_forms_no_uri_is_an_error_not_a_panic() {
    let err = render_spec_block_uri("context", "my task").expect_err("a space forms no URI");

    assert!(err.contains("my task"), "{err}");
}
