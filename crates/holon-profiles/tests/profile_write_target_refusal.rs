//! A profile whose widget would write a private field or an order key through
//! `set_field` is refused when the profile loads, naming the widget argument,
//! the field and the structural op that owns it.

use holon_profiles::create_default_registry;
use holon_profiles::parse_profile_yaml;

fn block_profile_rendering(render: &str) -> String {
    format!(
        "entity_name: block\nvariants:\n  - name: grouped\n    priority: 5\n    render: '{render}'\n"
    )
}

fn load(yaml: &str) -> anyhow::Result<()> {
    let registry = create_default_registry().expect("default registry loads");
    registry.apply_parsed_profile(parse_profile_yaml(yaml).expect("profile parses"))
}

#[test]
fn a_board_lane_field_naming_a_private_field_is_refused() {
    let err = load(&block_profile_rendering(
        r#"board(#{item_template: render_entity(), lane_field: "parent_id"})"#,
    ))
    .expect_err("a cross-lane drag would set_field(parent_id)");
    let msg = format!("{err:#}");
    for needle in [
        "profile 'block'",
        "variant 'grouped'",
        "board",
        "lane_field",
        "'parent_id' is a private field",
        "move_block",
    ] {
        assert!(msg.contains(needle), "missing {needle:?} in: {msg}");
    }
}

#[test]
fn every_author_supplied_write_target_is_checked() {
    for (render, arg, field) in [
        (r#"state_toggle(col("parent_id"))"#, "field", "parent_id"),
        (r#"state_toggle("sort_key")"#, "field", "sort_key"),
        (
            r#"state_toggle(#{field: "parent_id"})"#,
            "field",
            "parent_id",
        ),
        (
            r#"row(text(col("content")), editable_text(col("content"), #{field: "sort_key"}))"#,
            "field",
            "sort_key",
        ),
    ] {
        let err = load(&block_profile_rendering(render))
            .expect_err(&format!("`{render}` writes `{field}` through set_field"));
        let msg = format!("{err:#}");
        for needle in [arg, field] {
            assert!(
                msg.contains(needle),
                "`{render}`: missing {needle:?} in: {msg}"
            );
        }
    }
}

#[test]
fn writable_targets_load() {
    for render in [
        r#"board(#{item_template: render_entity(), lane_field: "task_state"})"#,
        r#"state_toggle(col("task_state"))"#,
        r#"state_toggle(#{field: "enabled", binding: "bool"})"#,
        r#"editable_text(col("content"))"#,
        r#"text(col("parent_id"))"#,
    ] {
        load(&block_profile_rendering(render))
            .unwrap_or_else(|e| panic!("`{render}` must load: {e:#}"));
    }
}

#[test]
fn an_org_embedded_profile_is_checked_for_write_targets() {
    let check = create_default_registry()
        .expect("default registry loads")
        .profile_load_check();
    let refused = parse_profile_yaml(&block_profile_rendering(
        r#"board(#{item_template: render_entity(), lane_field: "parent_id"})"#,
    ))
    .expect("profile parses");
    let msg = format!(
        "{:#}",
        check("block-profile-id", &refused).expect_err("lane_field parent_id refused")
    );
    assert!(msg.contains("lane_field"), "{msg}");
}
