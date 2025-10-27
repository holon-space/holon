//! Tests the UiEvent stream lifecycle:
//! - Happy path: Structure + Data events flow correctly
//! - Error recovery: render failures → error WidgetSpec → fix → valid WidgetSpec
//! - Structural hot-swap: editing query source triggers new Structure event

use std::sync::Arc;
use std::time::Duration;

use holon_api::UiEvent;
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
            .watch_ui_first_structure("query-heading")
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
            .watch_ui_first_structure("data-heading")
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
                    UiEvent::Structure { .. } => {
                        // Structural re-renders can happen too; keep waiting for Data
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
            .watch_ui_first_structure("missing-block")
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
            .watch_ui_first_structure("evolving-heading")
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
