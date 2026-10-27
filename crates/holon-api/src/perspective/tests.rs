use super::*;
use crate::block::Block;

fn panel_block(id: &str, parent: &str) -> Block {
    Block::new_text(EntityUri::block(id), EntityUri::block(parent), id)
}

fn source_child(id: &str, parent: &str, language: QueryLanguage, query: &str) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), EntityUri::block(parent), query);
    b.source_language = Some(SourceLanguage::Query(language));
    b
}

fn render_child(id: &str, parent: &str, dsl: &str) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), EntityUri::block(parent), dsl);
    b.source_language = Some(SourceLanguage::Render);
    b
}

/// A perspective with two panels, each with its own query, parses into a
/// typed spec whose panels carry the right languages and queries.
#[test]
fn parses_panels_with_queries() {
    let mut persp = panel_block("tasks-view", "root-layout");
    persp.set_property(P_NAME, "Tasks".to_string());
    persp.parent_id = EntityUri::no_parent();

    let mut left = panel_block("tv-left", "tasks-view");
    left.set_property("region", "left_sidebar".to_string());
    left.set_property("sequence", 0i64);
    let mut main = panel_block("tv-main", "tasks-view");
    main.set_property("region", "main".to_string());
    main.set_property("sequence", 1i64);

    let blocks = vec![
        persp,
        left,
        source_child(
            "tv-left::src",
            "tv-left",
            QueryLanguage::HolonSql,
            "SELECT * FROM block",
        ),
        main,
        source_child(
            "tv-main::src",
            "tv-main",
            QueryLanguage::HolonGql,
            "MATCH (b:block) RETURN b",
        ),
        render_child("tv-main::render", "tv-main", "tree(#{})"),
    ];

    let spec = PerspectiveSpec::parse(&EntityUri::block("tasks-view"), &blocks).unwrap();
    assert_eq!(spec.name, "Tasks");
    assert_eq!(spec.panels.len(), 2);
    assert_eq!(spec.panels[0].id, EntityUri::block("tv-left"));
    assert_eq!(spec.panels[0].region, Some(Region::LeftSidebar));
    assert_eq!(
        spec.panels[0].source,
        Some(PanelSource {
            language: QueryLanguage::HolonSql,
            query: "SELECT * FROM block".to_string()
        })
    );
    assert_eq!(spec.panels[1].id, EntityUri::block("tv-main"));
    assert_eq!(
        spec.panels[1].source.as_ref().unwrap().language,
        QueryLanguage::HolonGql
    );
    assert_eq!(spec.panels[1].render.as_deref(), Some("tree(#{})"));
}

/// Parse-don't-validate: an unrecognized `perspective_*` field is a loud
/// error, not a silently-dropped property.
#[test]
fn unknown_perspective_field_fails_loud() {
    let mut persp = panel_block("p", "root-layout");
    persp.set_property("perspective_bogus", "x".to_string());
    let blocks = vec![persp];
    let err = PerspectiveSpec::parse(&EntityUri::block("p"), &blocks).unwrap_err();
    assert!(
        err.to_string().contains("unknown perspective field"),
        "got: {err}"
    );
}

/// A malformed recognized field also fails loud.
#[test]
fn malformed_conceal_completed_fails_loud() {
    let mut persp = panel_block("p", "root-layout");
    persp.set_property(P_CONCEAL_COMPLETED, "maybe".to_string());
    let blocks = vec![persp];
    let err = PerspectiveSpec::parse(&EntityUri::block("p"), &blocks).unwrap_err();
    assert!(err.to_string().contains("must be a boolean"), "got: {err}");
}

/// A perspective declares `perspective_action_bar: false` to lay out its
/// panels without the narrow-screen action bar, whose global tier needs the
/// `navigation` entity.
#[test]
fn action_bar_off_drops_the_bar_from_the_layout() {
    let mut persp = panel_block("p", "root-layout");
    persp.set_property("perspective_action_bar", false);
    let panel = panel_block("p-main", "p");
    let render = render_child("p-main::render", "p-main", "text(\"hi\")");
    let blocks = vec![persp, panel, render];
    let spec = PerspectiveSpec::parse(&EntityUri::block("p"), &blocks).unwrap();
    let dsl = spec.layout_dsl().unwrap();
    assert!(
        !dsl.contains("navigation:main") && !dsl.contains("bottom_dock"),
        "the layout must not carry the action bar: {dsl}"
    );
    spec.layout_expr().unwrap();

    let mut on = panel_block("q", "root-layout");
    on.set_property("perspective_action_bar", true);
    let blocks = vec![
        on,
        panel_block("q-main", "q"),
        render_child("q-main::render", "q-main", "text(\"hi\")"),
    ];
    let spec = PerspectiveSpec::parse(&EntityUri::block("q"), &blocks).unwrap();
    assert!(spec.layout_dsl().unwrap().contains("navigation:main"));
}

/// Concealment params parse from namespaced fields.
#[test]
fn parses_concealment() {
    let mut persp = panel_block("p", "root-layout");
    persp.set_property(P_CONCEAL_COMPLETED, true);
    persp.set_property(P_CONCEAL_TAGS, "archived, spam".to_string());
    let blocks = vec![persp];
    let spec = PerspectiveSpec::parse(&EntityUri::block("p"), &blocks).unwrap();
    assert!(spec.concealment.hide_completed);
    assert_eq!(spec.concealment.hide_tags, vec!["archived", "spam"]);
}

/// The default: with no pointer set, the root-layout block is itself the
/// active perspective.
#[test]
fn resolve_defaults_to_root_layout() {
    let mut root = panel_block("root-layout", "root-layout");
    root.parent_id = EntityUri::no_parent();
    root.content = "Default".to_string();
    let blocks = vec![root];
    assert_eq!(
        active_perspective_id(&EntityUri::block("root-layout"), &blocks).unwrap(),
        EntityUri::block("root-layout")
    );
    let spec = resolve_active_perspective(&EntityUri::block("root-layout"), &blocks).unwrap();
    assert_eq!(spec.id, EntityUri::block("root-layout"));
}

/// Setting the pointer switches which perspective resolves — the queries
/// the active perspective exposes change accordingly.
#[test]
fn set_and_resolve_switches_active_perspective() {
    let mut root = panel_block("root-layout", "root-layout");
    root.parent_id = EntityUri::no_parent();

    // Default perspective (root-layout's own panel).
    let root_panel = panel_block("root-main", "root-layout");
    let root_src = source_child(
        "root-main::src",
        "root-main",
        QueryLanguage::HolonSql,
        "SELECT 'default'",
    );

    // A second, named perspective with a different query.
    let mut tasks = panel_block("tasks", "root-layout");
    tasks.parent_id = EntityUri::no_parent();
    tasks.set_property(P_NAME, "Tasks".to_string());
    let tasks_panel = panel_block("tasks-main", "tasks");
    let tasks_src = source_child(
        "tasks-main::src",
        "tasks-main",
        QueryLanguage::HolonSql,
        "SELECT 'tasks'",
    );

    set_active_perspective(&mut root, &EntityUri::block("tasks"));

    let blocks = vec![root, root_panel, root_src, tasks, tasks_panel, tasks_src];

    let spec = resolve_active_perspective(&EntityUri::block("root-layout"), &blocks).unwrap();
    assert_eq!(spec.id, EntityUri::block("tasks"));
    assert_eq!(spec.name, "Tasks");
    assert_eq!(spec.panels.len(), 1);
    assert_eq!(
        spec.panels[0].source.as_ref().unwrap().query,
        "SELECT 'tasks'"
    );
}

/// The synthesized default layout is byte-identical (modulo whitespace
/// normalization by the parser) to the previously-hardcoded YAML
/// `root_layout` variant when applied to the bundled root-layout shape:
/// left drawer / plain main / right drawer, plus a machinery-only advice
/// child that must NOT become a column.
#[test]
fn layout_dsl_reproduces_bundled_default_shape() {
    let mut root = panel_block("root-layout", "root-layout");
    root.parent_id = EntityUri::no_parent();

    let mut left = panel_block("default-left-sidebar", "root-layout");
    left.set_property("collapse_to", "drawer".to_string());
    let main = panel_block("default-main-panel", "root-layout");
    let mut right = panel_block("default-right-sidebar", "root-layout");
    right.set_property("collapse_to", "drawer".to_string());
    let advice = panel_block("default-advice-rules", "root-layout");

    let blocks = vec![
        root,
        left,
        source_child(
            "left::src",
            "default-left-sidebar",
            QueryLanguage::HolonSql,
            "SELECT 1",
        ),
        main,
        source_child(
            "main::src",
            "default-main-panel",
            QueryLanguage::HolonGql,
            "MATCH (b:block) RETURN b",
        ),
        right,
        source_child(
            "right::src",
            "default-right-sidebar",
            QueryLanguage::HolonGql,
            "MATCH (b:block) RETURN b",
        ),
        advice, // no query/render child → machinery-only, not displayable
    ];

    let spec = resolve_active_perspective(&EntityUri::block("root-layout"), &blocks).unwrap();
    let dsl = spec.layout_dsl().unwrap();

    // The bar's two tiers come from the same constants the builder uses:
    // this test pins the LAYOUT around them, not their internal spelling.
    let expected = format!(
        "{}{}{}{}{}{}{}",
        concat!(
            "if_space(600,\n  ",
            "bottom_dock(\n    ",
            "columns(drawer(\"block:default-left-sidebar\", \
                 live_block(\"block:default-left-sidebar\"), #{mode: \"overlay\"}), ",
            "live_block(\"block:default-main-panel\"), ",
            "drawer(\"block:default-right-sidebar\", \
                 live_block(\"block:default-right-sidebar\"), #{mode: \"overlay\"})),\n    ",
            "row(#{gap: 8}, ",
        ),
        ACTION_BAR_ENTITY_OPS,
        ", ",
        ACTION_BAR_GLOBAL_OPS,
        ")),\n  ",
        concat!(
            "if_space(1000,\n    ",
            "columns(drawer(\"block:default-left-sidebar\", \
                 live_block(\"block:default-left-sidebar\"), #{mode: \"shrink\"}), ",
            "live_block(\"block:default-main-panel\"), ",
            "drawer(\"block:default-right-sidebar\", \
                 live_block(\"block:default-right-sidebar\"), #{mode: \"overlay\"})),\n    ",
        ),
        concat!(
            "columns(drawer(\"block:default-left-sidebar\", \
                 live_block(\"block:default-left-sidebar\"), #{mode: \"shrink\"}), ",
            "live_block(\"block:default-main-panel\"), ",
            "drawer(\"block:default-right-sidebar\", \
                 live_block(\"block:default-right-sidebar\"), #{mode: \"shrink\"}))))",
        ),
    );
    assert_eq!(dsl, expected);

    // And it parses into a real RenderExpr.
    spec.layout_expr().unwrap();
}

/// A perspective whose panels are all machinery-only fails loud rather
/// than rendering an empty columns() shell.
#[test]
fn layout_expr_no_displayable_panels_fails_loud() {
    let mut persp = panel_block("p", "root-layout");
    persp.parent_id = EntityUri::no_parent();
    let bare = panel_block("p-child", "p");
    let blocks = vec![persp, bare];
    let spec = PerspectiveSpec::parse(&EntityUri::block("p"), &blocks).unwrap();
    let err = spec.layout_expr().unwrap_err();
    assert!(
        err.to_string().contains("no displayable panels"),
        "got: {err}"
    );
}

/// `perspective_profile` parses into the profile-cache key (EntityName);
/// a malformed value fails loud.
#[test]
fn profile_override_parses_to_profile_name() {
    let mut persp = panel_block("p", "root-layout");
    persp.set_property(P_PROFILE, "kanban_collection".to_string());
    let panel = panel_block("p-main", "p");
    let src = source_child("p-main::src", "p-main", QueryLanguage::HolonSql, "SELECT 1");
    let blocks = vec![persp, panel, src];
    let spec = PerspectiveSpec::parse(&EntityUri::block("p"), &blocks).unwrap();
    assert_eq!(
        spec.profile_override,
        Some(EntityName::new("kanban_collection"))
    );

    let mut bad = panel_block("q", "root-layout");
    bad.set_property(P_PROFILE, "9 not a name".to_string());
    let err = PerspectiveSpec::parse(&EntityUri::block("q"), &[bad]).unwrap_err();
    assert!(
        err.to_string().contains("is not a valid entity name"),
        "got: {err}"
    );
}

/// Persistence shape: the pointer is a plain block property, so it
/// round-trips through the same `properties` map Loro persists (like
/// collapse state). A property survives a Block clone/serialize the
/// same way — assert the property is present and drives resolution
/// after reconstructing the block set from its wire form.
#[test]
fn pointer_persists_as_block_property() {
    let mut root = panel_block("root-layout", "root-layout");
    root.parent_id = EntityUri::no_parent();
    set_active_perspective(&mut root, &EntityUri::block("tasks"));
    assert_eq!(
        root.get_property_str(ACTIVE_PERSPECTIVE_PROPERTY)
            .as_deref(),
        Some("block:tasks")
    );
    // Reconstruct from a round-tripped clone (models a reload from storage).
    let reloaded = root.clone();
    assert_eq!(
        active_perspective_id(&EntityUri::block("root-layout"), &[reloaded]).unwrap(),
        EntityUri::block("tasks")
    );
}
