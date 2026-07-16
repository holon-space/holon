//! Directed test for the C8 ruling: the ROOT display slot resolves via an
//! ordinary query over perspective DATA (the `active_perspective` pointer
//! property is the degenerate slot query), and switching is an ordinary
//! `set_field` on that pointer — no `activate_perspective` op.
//!
//! Asserts, against the Turso arm (`BlockDomain::render_root_slot`):
//! 1. default: the root slot synthesizes the bundled 3-panel layout from the
//!    root-layout block's own panels;
//! 2. after `set_field(active_perspective)`: the main-panel columns switch to
//!    the seeded second perspective's panels;
//! 3. the perspective's `profile_override` drives the collection panel's
//!    resolved variants (`resolve_collection_variants_named`), so the panel's
//!    default view mode switches too.
//!
//! @pbt kind harness
//! @pbt covers perspective-slot-resolution — C8 root display slot resolves via
//! perspective-data query

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_api::Value;
use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

/// Org file seeding a second perspective (with a `profile_override`) and the
/// override profile itself. IDs are bare per ORG_SYNTAX.md.
const PERSPECTIVE_ORG: &str = concat!(
    "* Tasks Perspective\n",
    ":PROPERTIES:\n",
    ":ID: tasks-perspective\n",
    ":perspective_name: Tasks\n",
    ":perspective_profile: kanban_collection\n",
    ":END:\n",
    "** Tasks Main\n",
    ":PROPERTIES:\n",
    ":ID: tasks-main-panel\n",
    ":END:\n",
    "#+begin_src holon_sql\n",
    "SELECT * FROM block WHERE parent_id = 'block:journals'\n",
    "#+end_src\n",
    "* Kanban Collection Profile\n",
    ":PROPERTIES:\n",
    ":ID: kanban-profile-heading\n",
    ":END:\n",
    "#+begin_src holon_entity_profile_yaml\n",
    "entity_name: kanban_collection\n",
    "computed: {}\n",
    "variants:\n",
    "  - name: board_view\n",
    "    priority: 0\n",
    "    render: 'board(#{item_template: render_entity(), lane_field: \"task_state\"})'\n",
    "#+end_src\n",
);

#[test]
fn root_slot_switches_via_set_field_and_override_drives_panel_variants() {
    let rt = runtime();
    rt.block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("perspectives.org", PERSPECTIVE_ORG)
            .build(rt.clone())
            .await
            .expect("build environment");

        assert!(
            env.wait_for_block("tasks-perspective", SYNC_TIMEOUT).await,
            "tasks-perspective block should sync"
        );
        assert!(
            env.wait_for_block("tasks-main-panel", SYNC_TIMEOUT).await,
            "tasks-main-panel block should sync"
        );

        // 1. Default: root slot synthesizes the bundled layout from the root-layout
        //    block's own panels.
        let default_expr = env.initial_widget().await.expect("default root render");
        let default_dbg = format!("{default_expr:?}");
        assert!(
            default_dbg.contains("if_space") && default_dbg.contains("block:default-main-panel"),
            "default root slot must synthesize the bundled panels, got: {default_dbg}"
        );
        assert!(
            !default_dbg.contains("block:tasks-main-panel"),
            "seeded perspective must not render before the pointer is set: {default_dbg}"
        );

        // Baseline: the collection panel resolves the DEFAULT collection
        // variants (multi-mode switcher with tree default) before the switch.
        let (before_expr, _s) = env
            .engine()
            .blocks()
            .render_entity(&EntityUri::block("tasks-main-panel"), &None)
            .await
            .expect("panel render before switch");
        let before_dbg = format!("{before_expr:?}");
        assert!(
            before_dbg.contains("tree"),
            "before the switch the panel must offer the default collection variants (tree \
             default), got: {before_dbg}"
        );

        // 2. Switch = ordinary set_field on the pointer data. No op.
        let mut params: HashMap<String, Value> = HashMap::new();
        params.insert(
            "id".to_string(),
            Value::String("block:root-layout".to_string()),
        );
        params.insert(
            "field".to_string(),
            Value::String("active_perspective".to_string()),
        );
        params.insert(
            "value".to_string(),
            Value::String("block:tasks-perspective".to_string()),
        );
        env.execute_operation("block", "set_field", params)
            .await
            .expect("set_field(active_perspective)");

        // Poll until the projection is visible to the render path.
        let deadline = std::time::Instant::now() + SYNC_TIMEOUT;
        let switched_dbg = loop {
            let expr = env
                .initial_widget()
                .await
                .expect("root render after switch");
            let dbg = format!("{expr:?}");
            if dbg.contains("block:tasks-main-panel") {
                break dbg;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "root slot did not switch to the pointed-to perspective within {SYNC_TIMEOUT:?}; \
                 last render: {dbg}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert!(
            !switched_dbg.contains("block:default-main-panel"),
            "after the switch the root slot must show ONLY the active perspective's panels, got: \
             {switched_dbg}"
        );

        // 3. The perspective's profile_override drives the panel's resolved collection
        //    variants: the kanban_collection profile's single unconditional board
        //    variant becomes the panel's render.
        // Poll: the override profile block is ingested into the profile cache
        // asynchronously (PROFILE_SQL matview), so give it the same window.
        let deadline = std::time::Instant::now() + SYNC_TIMEOUT;
        loop {
            let (after_expr, _s) = env
                .engine()
                .blocks()
                .render_entity(&EntityUri::block("tasks-main-panel"), &None)
                .await
                .expect("panel render after switch");
            let after_dbg = format!("{after_expr:?}");
            if after_dbg.contains("board") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the active perspective's profile_override (kanban_collection) must drive the \
                 panel's collection variants to board within {SYNC_TIMEOUT:?}, got: {after_dbg}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
}
