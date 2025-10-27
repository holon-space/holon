//! Tests the UiEvent stream lifecycle:
//! - Happy path: Structure + Data events flow correctly
//! - Error recovery: render failures → error WidgetSpec → fix → valid WidgetSpec
//! - Structural hot-swap: editing query source triggers new Structure event
//! - Trigger pipeline: slash command → ViewEventHandler → CommandMenu → operation

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::{EntityUri, UiEvent, Value};
use holon_integration_tests::{TestEnvironment, TestEnvironmentBuilder};

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

// =============================================================================
// Happy Path
// =============================================================================

#[test]
fn watch_ui_emits_structure_event_for_block_with_query_source() {
    let rt = runtime();
    rt.block_on(async {
        // Create a heading with a PRQL query source child
        let env = TestEnvironmentBuilder::new()
            .with_org_file(
                "test.org",
                concat!(
                    "* My Query Block\n",
                    ":PROPERTIES:\n",
                    ":ID: query-heading\n",
                    ":END:\n",
                    "#+begin_src prql\n",
                    "from block | select {id, content} | take 5\n",
                    "#+end_src\n",
                ),
            )
            .build(rt.clone())
            .await
            .expect("Failed to build environment");

        assert!(
            env.wait_for_block("query-heading", SYNC_TIMEOUT).await,
            "query-heading block should sync"
        );

        let (widget_spec, _watch) = env
            .watch_ui_first_structure(&EntityUri::block("query-heading"))
            .await
            .expect("watch_ui should succeed for block with query source");

        // The WidgetSpec should have a render expression
        assert!(
            !matches!(
                widget_spec.render_expr,
                holon_api::render_types::RenderExpr::Literal { .. }
            ),
            "render_expr should not be a bare literal — expected a function call (table, list, etc.)"
        );
    });
}

#[test]
fn watch_ui_emits_data_events_after_structure() {
    let rt = runtime();
    rt.block_on(async {
        let mut env = TestEnvironmentBuilder::new()
            .with_org_file(
                "test.org",
                concat!(
                    "* Data Watcher\n",
                    ":PROPERTIES:\n",
                    ":ID: data-heading\n",
                    ":END:\n",
                    "#+begin_src prql\n",
                    "from block | select {id, content} | take 10\n",
                    "#+end_src\n",
                ),
            )
            .build(rt.clone())
            .await
            .expect("Failed to build environment");

        assert!(
            env.wait_for_block("data-heading", SYNC_TIMEOUT).await,
            "data-heading block should sync"
        );

        let (_widget_spec, mut watch) = env
            .watch_ui_first_structure(&EntityUri::block("data-heading"))
            .await
            .expect("watch_ui should succeed");

        // Trigger a data change by adding a new block
        env.write_org_file(
            "extra.org",
            concat!(
                "* Extra Block\n",
                ":PROPERTIES:\n",
                ":ID: extra-1\n",
                ":END:\n",
            ),
        )
        .await
        .expect("write extra.org");

        assert!(
            env.wait_for_block("extra-1", SYNC_TIMEOUT).await,
            "extra-1 block should sync"
        );

        // Wait for a Data event (the new block should appear in the query results)
        let deadline = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let event = watch.recv().await.expect("stream should stay open");
                match event {
                    UiEvent::Data { batch, generation } => {
                        assert!(generation > 0, "generation should be positive");
                        assert!(
                            !batch.inner.items.is_empty(),
                            "Data event should contain changes"
                        );
                        return;
                    }
                    UiEvent::Structure { .. } | UiEvent::CollectionUpdate { .. } => {
                        continue;
                    }
                }
            }
        })
        .await;

        assert!(
            deadline.is_ok(),
            "Should receive a Data event within timeout"
        );
    });
}

// =============================================================================
// Error Recovery
// =============================================================================

#[test]
fn watch_ui_error_recovery_on_nonexistent_block() {
    let rt = runtime();
    rt.block_on(async {
        let mut env = TestEnvironmentBuilder::new()
            .with_org_file(
                "test.org",
                "* Placeholder\n:PROPERTIES:\n:ID: placeholder\n:END:\n",
            )
            .build(rt.clone())
            .await
            .expect("Failed to build environment");

        assert!(
            env.wait_for_block("placeholder", SYNC_TIMEOUT).await,
            "placeholder should sync"
        );

        // Watch a block that doesn't exist yet — should get an error Structure event
        let (error_spec, mut watch) = env
            .watch_ui_first_structure(&EntityUri::block("missing-block"))
            .await
            .expect("watch_ui should return stream even for missing block");

        // The error spec should have an "error" function call
        match &error_spec.render_expr {
            holon_api::render_types::RenderExpr::FunctionCall { name, .. } => {
                assert_eq!(name, "error", "Expected error widget for missing block");
            }
            other => panic!("Expected FunctionCall(error), got {:?}", other),
        }

        // Now create the block with a query source
        env.write_org_file(
            "missing.org",
            concat!(
                "* Now Exists\n",
                ":PROPERTIES:\n",
                ":ID: missing-block\n",
                ":END:\n",
                "#+begin_src prql\n",
                "from block | select {id, content} | take 3\n",
                "#+end_src\n",
            ),
        )
        .await
        .expect("write missing.org");

        assert!(
            env.wait_for_block("missing-block", SYNC_TIMEOUT).await,
            "missing-block should sync"
        );

        // The watcher should emit a new Structure event once the block appears
        let recovered_spec =
            TestEnvironment::wait_for_next_structure(&mut watch, Duration::from_secs(10))
                .await
                .expect("Should receive recovered Structure event");

        match &recovered_spec.render_expr {
            holon_api::render_types::RenderExpr::FunctionCall { name, .. } => {
                assert_ne!(
                    name, "error",
                    "After recovery, render_expr should not be an error"
                );
            }
            _ => {} // Any non-error expression is fine
        }
    });
}

// =============================================================================
// Structural Hot-Swap
// =============================================================================

#[test]
fn watch_ui_structural_change_triggers_new_structure_event() {
    let rt = runtime();
    rt.block_on(async {
        let mut env = TestEnvironmentBuilder::new()
            .with_org_file(
                "test.org",
                concat!(
                    "* Evolving Query\n",
                    ":PROPERTIES:\n",
                    ":ID: evolving-heading\n",
                    ":END:\n",
                    "#+begin_src prql\n",
                    "from block | select {id, content} | take 5\n",
                    "#+end_src\n",
                ),
            )
            .build(rt.clone())
            .await
            .expect("Failed to build environment");

        assert!(
            env.wait_for_block("evolving-heading", SYNC_TIMEOUT).await,
            "evolving-heading should sync"
        );

        let (first_spec, mut watch) = env
            .watch_ui_first_structure(&EntityUri::block("evolving-heading"))
            .await
            .expect("watch_ui should succeed");

        let first_data_len = first_spec.data.len();

        // Edit the query source to select different columns
        env.write_org_file(
            "test.org",
            concat!(
                "* Evolving Query\n",
                ":PROPERTIES:\n",
                ":ID: evolving-heading\n",
                ":END:\n",
                "#+begin_src prql\n",
                "from block | select {id, content, parent_id} | take 5\n",
                "#+end_src\n",
            ),
        )
        .await
        .expect("write updated org file");

        // Wait for structural re-render
        let new_spec =
            TestEnvironment::wait_for_next_structure(&mut watch, Duration::from_secs(10))
                .await
                .expect("Should receive new Structure event after query change");

        // The new spec should still be valid (not an error)
        match &new_spec.render_expr {
            holon_api::render_types::RenderExpr::FunctionCall { name, .. } => {
                assert_ne!(
                    name, "error",
                    "Updated query should produce valid render expression"
                );
            }
            _ => {} // Any non-error is fine
        }

        // Data should be present in the new spec
        assert!(
            !new_spec.data.is_empty() || first_data_len == 0,
            "New spec should have data (unless original had none too)"
        );
    });
}

// =============================================================================
// Trigger Pipeline (slash command → ViewEventHandler → CommandMenu → operation)
// =============================================================================

#[test]
fn trigger_pipeline_slash_command_delete() {
    let rt = runtime();
    rt.block_on(async {
        // Create a heading with a query + sibling blocks in same file
        let env = TestEnvironmentBuilder::new()
            .with_org_file(
                "test.org",
                concat!(
                    "* Parent Block\n",
                    ":PROPERTIES:\n",
                    ":ID: parent-block\n",
                    ":END:\n",
                    "#+begin_src prql\n",
                    "from block | select {id, content} | take 10\n",
                    "#+end_src\n",
                    "* Target Block\n",
                    ":PROPERTIES:\n",
                    ":ID: target-block\n",
                    ":END:\n",
                    "* Keep Block\n",
                    ":PROPERTIES:\n",
                    ":ID: keep-block\n",
                    ":END:\n",
                ),
            )
            .build(rt.clone())
            .await
            .expect("Failed to build environment");

        assert!(
            env.wait_for_block("target-block", SYNC_TIMEOUT).await,
            "target-block should sync"
        );

        // 1. Render the parent block to get a WidgetSpec with operations
        let (ws, _watch) = env
            .watch_ui_first_structure(&EntityUri::block("parent-block"))
            .await
            .expect("watch_ui should succeed");

        // 2. Shadow interpret to ViewModel
        let engine = env.engine();
        let engine_clone = Arc::clone(&engine);
        let render_expr = ws.render_expr.clone();
        let data_rows = ws.data.clone();

        let display_tree = tokio::task::spawn_blocking(move || {
            let ctx = holon_frontend::RenderContext::headless(engine_clone);
            let ctx = ctx.with_data_rows(data_rows);
            let interp = holon_frontend::create_shadow_interpreter();
            interp.interpret(&render_expr, &ctx)
        })
        .await
        .expect("spawn_blocking panicked");

        // 3. Find EditableText nodes and verify triggers are present
        let editables =
            holon_integration_tests::display_assertions::collect_editable_text_nodes(&display_tree);
        assert!(
            !editables.is_empty(),
            "ViewModel should contain EditableText nodes.\n{}",
            display_tree.pretty_print(0)
        );

        // Find one with operations and triggers
        let editable = editables
            .iter()
            .find(|n| !n.operations.is_empty() && !n.triggers.is_empty())
            .unwrap_or_else(|| {
                panic!(
                    "No EditableText with operations+triggers found.\n{}",
                    display_tree.pretty_print(0)
                )
            });

        // 4. Simulate typing "/" at line start
        let event = holon_frontend::input_trigger::check_triggers(&editable.triggers, "/", 1)
            .expect("check_triggers should match '/' at line start");

        // 5. Feed to ViewEventHandler
        let context_params: HashMap<String, Value> = editable
            .entity
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let (field, content) = match &editable.kind {
            holon_frontend::view_model::NodeKind::EditableText { field, content } => {
                (field.clone(), content.clone())
            }
            _ => unreachable!("collect_editable_text_nodes guarantees EditableText"),
        };

        let mut handler = holon_frontend::view_event_handler::ViewEventHandler::new(
            editable.operations.clone(),
            context_params,
            field,
            content,
        );
        let action = handler.handle(event);
        assert!(
            matches!(action, holon_frontend::command_menu::MenuAction::Updated),
            "Expected MenuAction::Updated after typing '/', got {:?}",
            action
        );

        // 6. Menu should be active with available operations
        let menu_state = handler.command_menu.menu_state().unwrap();
        assert!(
            !menu_state.matches.is_empty(),
            "Command menu should have matching operations"
        );

        // 7. Find and select "delete"
        let delete_idx = menu_state
            .matches
            .iter()
            .position(|m| m.operation_name() == "delete")
            .expect("'delete' should be in the command menu");

        for _ in 0..delete_idx {
            handler.on_key(holon_frontend::command_menu::MenuKey::Down);
        }

        let action = handler.on_key(holon_frontend::command_menu::MenuKey::Enter);
        match action {
            holon_frontend::command_menu::MenuAction::Execute {
                entity_name,
                op_name,
                params,
            } => {
                // 8. Execute the operation
                env.execute_operation(&entity_name, &op_name, params)
                    .await
                    .expect("delete operation should succeed");
            }
            other => panic!("Expected MenuAction::Execute, got {:?}", other),
        }

        // 9. Verify the block was deleted
        let rows = env
            .query_sql("SELECT id FROM block WHERE id = 'block:target-block'")
            .await
            .expect("query should succeed");
        assert!(
            rows.is_empty(),
            "target-block should be deleted after slash command"
        );
    });
}

#[test]
fn trigger_presence_on_editable_text_nodes() {
    let rt = runtime();
    rt.block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file(
                "test.org",
                concat!(
                    "* Heading With Children\n",
                    ":PROPERTIES:\n",
                    ":ID: heading-1\n",
                    ":END:\n",
                    "#+begin_src prql\n",
                    "from children | select {id, content}\n",
                    "#+end_src\n",
                ),
            )
            .with_org_file(
                "child.org",
                concat!("* Child A\n", ":PROPERTIES:\n", ":ID: child-a\n", ":END:\n",),
            )
            .build(rt.clone())
            .await
            .expect("Failed to build environment");

        assert!(
            env.wait_for_block("heading-1", SYNC_TIMEOUT).await,
            "heading-1 should sync"
        );

        // Render and shadow interpret
        let (ws, _watch) = env
            .watch_ui_first_structure(&EntityUri::block("heading-1"))
            .await
            .expect("watch_ui should succeed");

        let engine_clone = Arc::clone(&env.engine());
        let render_expr = ws.render_expr.clone();
        let data_rows = ws.data.clone();

        let display_tree = tokio::task::spawn_blocking(move || {
            let ctx = holon_frontend::RenderContext::headless(engine_clone);
            let ctx = ctx.with_data_rows(data_rows);
            let interp = holon_frontend::create_shadow_interpreter();
            interp.interpret(&render_expr, &ctx)
        })
        .await
        .expect("spawn_blocking panicked");

        // Every EditableText with operations should have triggers (inv10g)
        let (total_with_ops, missing) =
            holon_integration_tests::display_assertions::count_editables_missing_triggers(
                &display_tree,
            );

        if total_with_ops > 0 {
            assert_eq!(
                missing,
                0,
                "{missing}/{total_with_ops} EditableText nodes with ops are missing triggers.\n{}",
                display_tree.pretty_print(0)
            );
        }
    });
}
