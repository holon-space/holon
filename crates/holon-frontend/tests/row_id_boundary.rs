//! A query row's `id` column is authored outside Holon — a vault's own SQL
//! chooses it. A value that forms no URI must paint ONE error node naming it,
//! never unwind inside the URI constructor: the column is read again by the
//! item template, by every value fn and by navigation, so an unrefused row
//! takes the whole render with it.

use std::sync::Arc;

use futures_signals::signal::Mutable;
use holon_api::Value;
use holon_api::widget_spec::DataRow;
use holon_frontend::ReactiveViewModel;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::interpret_pure;

/// The row a `live_query` returns for `SELECT 'my task' AS id`.
fn bad_row() -> Arc<DataRow> {
    let mut row = DataRow::new();
    row.insert("id".to_string(), Value::String("my task".to_string()));
    row.insert("content".to_string(), Value::String("hello".to_string()));
    Arc::new(row)
}

/// A row with exactly the columns given.
fn row_with(pairs: &[(&str, &str)]) -> Arc<DataRow> {
    Arc::new(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect(),
    )
}

fn good_row(id: &str) -> Arc<DataRow> {
    let mut row = DataRow::new();
    row.insert("id".to_string(), Value::String(id.to_string()));
    row.insert("content".to_string(), Value::String("hello".to_string()));
    Arc::new(row)
}

fn interpret(dsl: &str, rows: &[Arc<DataRow>]) -> ReactiveViewModel {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr = holon_api::render_dsl::parse_render_dsl(dsl).expect("dsl should parse");
    interpret_pure(&expr, rows, &StubBuilderServices::new())
}

/// The per-row nodes a collection materialized.
fn items(vm: &ReactiveViewModel) -> Vec<Arc<ReactiveViewModel>> {
    let collection = vm
        .collection
        .as_ref()
        .unwrap_or_else(|| panic!("expected a collection, got {:?}", vm.widget_name()));
    collection.items.lock_ref().to_vec()
}

/// The message of the first error node anywhere under `vm` — a wrapper such as
/// `tree_item` keeps the refusal as a child rather than replacing itself.
fn error_under(vm: &ReactiveViewModel) -> Option<String> {
    if vm.widget_name().as_deref() == Some("error") {
        return Some(vm.prop_str("message").unwrap_or_default());
    }
    vm.children
        .iter()
        .find_map(|child| error_under(child))
        .or_else(|| {
            vm.collection
                .as_ref()
                .and_then(|c| c.items.lock_ref().iter().find_map(|i| error_under(i)))
        })
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

/// `live_block()` reads the surrounding row's `id` column to name the block it
/// renders.
#[test]
fn a_row_whose_id_forms_no_uri_renders_one_error_node_naming_it() {
    let tree = interpret(
        "columns(#{item_template: live_block()})",
        &[bad_row(), good_row("block:ok")],
    );
    let items = items(&tree);

    assert_eq!(items.len(), 2, "one node per row, refused or not");
    let msg = message(&items[0]);
    assert!(msg.contains("my task"), "{msg}");
    assert_eq!(
        items[1].widget_name().as_deref(),
        Some("live_block"),
        "the well-formed row renders its template"
    );
}

/// A leaf builder reaches the row id through `entity_id()`, which the row-set
/// pipelines and `user_driver` also call.
#[test]
fn a_refused_row_paints_one_error_node_instead_of_one_per_leaf() {
    let tree = interpret(
        "columns(#{item_template: row(#{children: [badge(col(\"content\")), text(col(\"content\"))]})})",
        &[bad_row()],
    );
    let items = items(&tree);

    assert_eq!(items.len(), 1, "the refusal replaces the whole row");
    assert!(message(&items[0]).contains("my task"));
}

#[test]
fn navigating_onto_a_refused_row_resolves_no_entity_without_panicking() {
    let tree = interpret(
        "columns(#{item_template: live_block()})",
        &[bad_row(), good_row("block:ok")],
    );

    for item in items(&tree) {
        let resolved = holon_frontend::focus_path::resolve_entity_id(&item);
        assert_eq!(item.entity_id(), resolved);
    }
    assert_eq!(
        holon_frontend::focus_path::resolve_entity_id(&items(&tree)[1]),
        Some(holon_api::EntityUri::block("ok"))
    );
}

/// A tree/outline binds its rows through the same seam as a flat collection.
#[test]
fn a_tree_row_whose_id_forms_no_uri_renders_an_error_node_naming_it() {
    let tree = interpret(
        "tree(#{item_template: text(col(\"content\"))})",
        &[bad_row()],
    );
    let items = items(&tree);

    assert_eq!(items.len(), 1);
    let msg = error_under(&items[0]).expect("the row renders an error node");
    assert!(msg.contains("my task"), "{msg}");
}

/// A node built outside a collection — a single `live_query` reading the
/// surrounding row — reaches the same seam.
#[test]
fn a_bare_node_over_a_refused_row_renders_an_error_node() {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr = holon_api::render_dsl::parse_render_dsl("live_block()").expect("dsl should parse");
    let ctx = RenderContext::default().with_row_mutable(Mutable::new(bad_row()).read_only());

    let vm = StubBuilderServices::new().interpret(&expr, &ctx);

    assert!(message(&vm).contains("my task"));
}

/// The row store keys a row by the id text it was given, and the CDC arms that
/// carry an id without a row derive the same key — otherwise a `Deleted` for
/// such a row would never find it.
#[test]
fn the_row_store_keys_an_unusable_id_by_its_text() {
    let of_row = holon_api::RowIdentity::of_row(&*bad_row());
    let of_id = holon_api::RowIdentity::of_id_str("my task");

    assert_eq!(of_row, of_id);
    assert!(of_row.is_value(), "an id that names no entity is not one");
}

/// A row with no `id` column is an ordinary value row, not a fault.
#[test]
fn a_row_without_an_id_column_is_not_refused() {
    let mut bare = DataRow::new();
    bare.insert("content".to_string(), Value::String("hello".to_string()));

    let tree = interpret(
        "columns(#{item_template: text(col(\"content\"))})",
        &[Arc::new(bare)],
    );
    let items = items(&tree);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].widget_name().as_deref(), Some("text"));
}

/// The `:__virtual:` marker is Holon's own creation-slot encoding, but the id
/// column is the vault's: a value that merely *contains* the marker is not a
/// creation slot, and the placeholder branch must not resolve its "parent" with
/// a conversion that assumes one. The refusal, not the marker reader, answers
/// this row.
#[test]
fn a_row_whose_id_contains_the_virtual_marker_but_is_not_one_renders_an_error_node() {
    let tree = interpret(
        "columns(#{item_template: text(col(\"content\"))})",
        &[row_with(&[("id", "my task:__virtual:x")])],
    );
    let items = items(&tree);

    assert_eq!(items.len(), 1);
    let msg = error_under(&items[0]).expect("the row renders an error node");
    assert!(msg.contains("my task:__virtual:x"), "{msg}");
}

/// The marker reader is also reached outside a collection, over a raw context
/// row.
#[test]
fn the_origin_of_a_row_whose_id_contains_the_marker_does_not_unwind() {
    let row = row_with(&[("id", "my task:__virtual:x")]);

    assert_eq!(
        holon_frontend::row_origin::RowOrigin::from_row(&row),
        holon_frontend::row_origin::RowOrigin::Canonical
    );
}

/// `transclude()` names its block with a positional argument, else the row's
/// `target_uri` column — a column the vault's own SQL chooses.
#[test]
fn a_transclude_target_that_forms_no_uri_renders_an_error_node() {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr = holon_api::render_dsl::parse_render_dsl("transclude()").expect("dsl should parse");
    let ctx = RenderContext::default().with_row_mutable(
        Mutable::new(row_with(&[
            ("id", "block:ok"),
            ("target_uri", "block:my task"),
        ]))
        .read_only(),
    );

    let vm = StubBuilderServices::new().interpret(&expr, &ctx);

    assert!(message(&vm).contains("block:my task"), "{}", message(&vm));
}

/// `drawer()` names the block whose collapse state it controls with a
/// positional argument, else the surrounding row's `id` column. Both reach the
/// `block_id` prop, which `entity_id()` resolves without re-checking it.
#[test]
fn a_drawer_over_a_refused_row_renders_an_error_node() {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr = holon_api::render_dsl::parse_render_dsl("drawer()").expect("dsl should parse");
    let ctx = RenderContext::default().with_row_mutable(Mutable::new(bad_row()).read_only());

    let vm = StubBuilderServices::new().interpret(&expr, &ctx);

    assert!(message(&vm).contains("my task"), "{}", message(&vm));
}

#[test]
fn a_drawer_whose_positional_id_forms_no_uri_renders_an_error_node() {
    let tree = interpret(
        "drawer(\"block:my task\", text(col(\"content\")))",
        &[good_row("block:ok")],
    );

    assert!(
        message(&tree).contains("block:my task"),
        "{}",
        message(&tree)
    );
}

/// A row with no `id` column is an ordinary value row, so a drawer over it
/// names no block — it must not claim the empty string as one.
#[test]
fn a_drawer_over_a_row_without_an_id_column_names_no_entity() {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr = holon_api::render_dsl::parse_render_dsl("drawer()").expect("dsl should parse");
    let row = row_with(&[("content", "hello")]);
    let ctx = RenderContext::default().with_row_mutable(Mutable::new(row).read_only());

    let vm = StubBuilderServices::new().interpret(&expr, &ctx);

    assert_eq!(vm.widget_name().as_deref(), Some("drawer"));
    assert_eq!(
        vm.prop_str("block_id"),
        None,
        "no id column, no block named"
    );
    assert_eq!(vm.entity_id(), None);
}

/// `question_options` names the question each answer button dispatches against
/// with the row's `id` column — minted by an integration (`cc-pending-question:
/// <provider question id>`), so the text is authored outside Holon.
const QUESTION_DSL: &str = "question_options(#{options: col(\"options\"), \
                            action: answer_question()})";

fn question_row(id: Option<&str>) -> Arc<DataRow> {
    let mut row = DataRow::new();
    if let Some(id) = id {
        row.insert("id".to_string(), Value::String(id.to_string()));
    }
    row.insert(
        "options".to_string(),
        Value::String(r#"[{"label":"Yes","description":"d"}]"#.to_string()),
    );
    Arc::new(row)
}

fn interpret_over(dsl: &str, row: Arc<DataRow>) -> ReactiveViewModel {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr = holon_api::render_dsl::parse_render_dsl(dsl).expect("dsl should parse");
    let ctx = RenderContext::default().with_row_mutable(Mutable::new(row).read_only());
    StubBuilderServices::new().interpret(&expr, &ctx)
}

#[test]
fn a_question_whose_id_forms_no_uri_renders_an_error_node() {
    let vm = interpret_over(QUESTION_DSL, question_row(Some("my task")));

    assert!(message(&vm).contains("my task"), "{}", message(&vm));
}

#[test]
fn a_question_row_without_an_id_column_renders_an_error_node() {
    let vm = interpret_over(QUESTION_DSL, question_row(None));

    assert!(message(&vm).contains("`id`"), "{}", message(&vm));
}

/// The control: the shipped mirror's own key shape still renders its buttons.
#[test]
fn a_question_with_a_schemed_id_renders_its_options() {
    let vm = interpret_over(
        QUESTION_DSL,
        question_row(Some("cc-pending-question:job-77:0:1a2b3c4d")),
    );

    assert_eq!(vm.widget_name().as_deref(), Some("question_options"));
}

/// The store's ingest and the CDC arms that carry an id without a row must
/// derive the SAME key, or a `Deleted` never finds its entry. An `INTEGER` id
/// column is a real matview shape, and it is the case where reading the column
/// as a string and classifying it as a row disagreed.
#[test]
fn the_two_classifiers_agree_on_an_integer_id() {
    let mut integer = DataRow::new();
    integer.insert("id".to_string(), Value::Integer(42));
    integer.insert("content".to_string(), Value::String("hello".to_string()));

    assert_eq!(
        holon_api::RowIdentity::of_row(&integer),
        holon_api::RowIdentity::of_id_str("42")
    );
    assert_eq!(
        holon_api::data_row_entity_uri(&integer),
        Some(holon_api::EntityUri::block("42"))
    );
}

/// An empty `id` column names no entity, so the row is value-shaped and keys on
/// its content. That is the one case the two classifiers do not share a key
/// for: a CDC change carrying an empty id has no content to hash, so it keys
/// the text — and no CDC change carries an empty id.
#[test]
fn an_empty_id_column_keys_a_row_on_its_content() {
    let empty = row_with(&[("id", ""), ("content", "hello")]);

    let identity = holon_api::RowIdentity::of_row(&empty);
    assert!(identity.is_value(), "an empty id names no entity");
    assert_eq!(holon_api::data_row_entity_uri(&empty), None);
    assert_eq!(
        identity,
        holon_api::RowIdentity::Value(holon_api::RowContentHash::of_row(&empty)),
        "the row keys on the content of ALL its columns, the empty id included"
    );
}

/// `render_entity` resolves the row's profile and picks a variant from it,
/// reading the row's `id` column as a UI-state key. A stub resolves no profile,
/// so the path needs one to be reachable at all — and a row whose column names
/// no entity must still not unwind.
#[test]
fn a_variant_profile_over_a_row_that_names_no_entity_does_not_unwind() {
    use holon_api::predicate::Predicate;
    use holon_api::render_types::RenderProfile;
    use holon_api::render_types::RenderVariant;

    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let profile = RenderProfile {
        name: "refused-row".to_string(),
        render: holon_api::render_dsl::parse_render_dsl("text(#{content: \"fallback\"})")
            .expect("dsl"),
        operations: Vec::new(),
        variants: vec![RenderVariant {
            name: "always".to_string(),
            render: holon_api::render_dsl::parse_render_dsl("text(#{content: \"variant\"})")
                .expect("dsl"),
            operations: Vec::new(),
            condition: Predicate::Always,
        }],
    };
    let services = StubBuilderServices::new().with_profile(profile);
    let expr = holon_api::render_dsl::parse_render_dsl("render_entity()").expect("dsl");
    let ctx = RenderContext::default().with_row_mutable(Mutable::new(bad_row()).read_only());

    let vm = services.interpret(&expr, &ctx);

    assert_ne!(
        vm.widget_name().as_deref(),
        Some("error"),
        "the row's column is a UI-state key here, not an entity to resolve"
    );
}
