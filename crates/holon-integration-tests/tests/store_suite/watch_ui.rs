//! Tests the UiEvent stream lifecycle:
//! - Happy path: Structure + Data events flow correctly
//! - Error recovery: render failures → error RenderExpr → fix → valid
//!   RenderExpr
//! - Structural hot-swap: editing query source triggers new Structure event
//! - Trigger pipeline: slash command → ViewEventHandler → CommandMenu →
//!   operation
//!
//! @pbt kind harness
//! @pbt covers uievent-stream-lifecycle — UiEvent stream happy-path +
//! error-recovery

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_api::UiEvent;
use holon_api::Value;
use holon_frontend::reactive::BuilderServices;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_integration_tests::test_tracing::ProblemKind;
use holon_integration_tests::test_tracing::SpanCollector;
use holon_integration_tests::test_tracing::attach_scope_to_runtime;
use holon_integration_tests::test_tracing::begin_test_scope;

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
                    "#+begin_src holon_prql\n",
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

        let (render_expr, _watch) = env
            .watch_ui_first_structure(&EntityUri::block("query-heading"))
            .await
            .expect("watch_ui should succeed for block with query source");

        // The `source` view mode is the query-source-specific part of the render: it
        // carries this block's own query text and language. A fixture language that is
        // not a query language attaches no query source, and the block renders as a
        // leaf instead.
        let holon_api::render_types::RenderExpr::FunctionCall { name, args } = &render_expr else {
            panic!("expected a query-source render, got {render_expr:?}");
        };
        assert_eq!(
            name, "view_mode_switcher",
            "a query-source child must render the view-mode switcher"
        );
        let source_mode = args
            .iter()
            .find(|a| a.name.as_deref() == Some("mode_source"))
            .unwrap_or_else(|| panic!("switcher carries no `source` view mode: {render_expr:?}"));
        assert_eq!(
            source_mode.value,
            holon_api::render_types::RenderExpr::FunctionCall {
                name: "source_editor".to_string(),
                args: vec![
                    holon_api::render_types::Arg {
                        name: Some("language".to_string()),
                        value: holon_api::render_types::RenderExpr::Literal {
                            value: Value::String("holon_prql".to_string()),
                        },
                    },
                    holon_api::render_types::Arg {
                        name: Some("content".to_string()),
                        value: holon_api::render_types::RenderExpr::Literal {
                            value: Value::String(
                                "from block | select {id, content} | take 5".to_string()
                            ),
                        },
                    },
                ],
            },
            "the `source` view mode must carry this block's query text and language"
        );
    });
}

#[test]
fn watch_ui_emits_data_events_after_structure() {
    let rt = runtime();
    rt.block_on(async {
        let env = TestEnvironmentBuilder::new()
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

        let (_render_expr, mut watch) = env
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
                    UiEvent::Structure { .. } => {
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
        let env = TestEnvironmentBuilder::new()
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
        let (error_render_expr, mut watch) = env
            .watch_ui_first_structure(&EntityUri::block("missing-block"))
            .await
            .expect("watch_ui should return stream even for missing block");

        // The error render_expr should have an "error" function call
        match &error_render_expr {
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
        let recovered_render_expr =
            TestEnvironment::wait_for_next_structure(&mut watch, Duration::from_secs(10))
                .await
                .expect("Should receive recovered Structure event");

        // Any non-error expression is fine.
        if let holon_api::render_types::RenderExpr::FunctionCall { name, .. } =
            &recovered_render_expr
        {
            assert_ne!(
                name, "error",
                "After recovery, render_expr should not be an error"
            );
        }
    });
}

/// The common case: `create_block` is awaited, so the newborn's row is in the
/// store before the watcher's first render, and that render must not be the
/// missing-row error widget. This pins the WARN-tier path staying out of the
/// ordinary case, NOT that the create race is impossible — on a row that had
/// not landed, the same widget would paint.
#[test]
fn a_just_created_blocks_first_render_is_not_the_missing_row_widget() {
    let rt = runtime();
    rt.block_on(async {
        let env = TestEnvironmentBuilder::new()
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

        env.create_block("newborn", "placeholder", "just typed")
            .await
            .expect("create newborn");

        // Watch with NO wait for the row: the point is the render that races it.
        let (first_render_expr, _watch) = env
            .watch_ui_first_structure(&EntityUri::block("newborn"))
            .await
            .expect("watch_ui should return a stream for the newborn");

        if let holon_api::render_types::RenderExpr::FunctionCall { name, .. } = &first_render_expr {
            assert_ne!(
                name, "error",
                "a just-created block rendered the missing-row error widget"
            );
        }
    });
}

/// A runtime whose worker threads carry this case's observability scope: the
/// `ui_watcher` logs its render failures from inside a spawned task, so a
/// thread-local capture would see nothing.
fn scoped_runtime() -> Arc<tokio::runtime::Runtime> {
    SpanCollector::global();
    let scope = begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    attach_scope_to_runtime(&mut builder, scope);
    Arc::new(builder.build().expect("Failed to create runtime"))
}

/// The ERROR tier of `render_and_forward` needs a positive pin: the
/// `api::ui_watcher` unit tests exercise the disclosure helpers, never the
/// dispatch. A render failure that is NOT a missing row — here a query source
/// reading a table no schema provider registers, which the integration
/// attribution cannot explain away — is a real failure, so it must reach the
/// error-capture oracles as well as paint the visible error widget.
#[test]
fn a_render_failure_that_is_not_a_missing_row_stays_at_the_error_tier() {
    let rt = scoped_runtime();
    rt.block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file(
                "test.org",
                concat!(
                    "* Broken Query\n",
                    ":PROPERTIES:\n",
                    ":ID: broken-query\n",
                    ":END:\n",
                    "#+begin_src holon_prql\n",
                    "from missing_table_zzz | select {id}\n",
                    "#+end_src\n",
                ),
            )
            .build(rt.clone())
            .await
            .expect("Failed to build environment");

        assert!(
            env.wait_for_block("broken-query", SYNC_TIMEOUT).await,
            "broken-query should sync"
        );

        let (render_expr, _watch) = env
            .watch_ui_first_structure(&EntityUri::block("broken-query"))
            .await
            .expect("watch_ui should return a stream for a block whose render fails");

        match &render_expr {
            holon_api::render_types::RenderExpr::FunctionCall { name, .. } => {
                assert_eq!(
                    name, "error",
                    "a failed render must still paint the error widget"
                );
            }
            other => panic!("Expected FunctionCall(error), got {:?}", other),
        }

        let problems = SpanCollector::global().captured_problems();
        assert!(
            problems.iter().any(|p| {
                p.kind == ProblemKind::ErrorLog
                    && p.target.contains("ui_watcher")
                    && p.message.contains("render_entity")
                    && p.message.contains("broken-query")
            }),
            "the failing render must be logged at ERROR by the ui_watcher — that tier is what \
             `inv-no-observed-errors` reads. Captured: {problems:#?}"
        );

        let problems = SpanCollector::global().captured_problems();
        assert!(
            problems.iter().any(|p| {
                p.kind == ProblemKind::ErrorLog
                    && p.target.contains("ui_watcher")
                    && p.message.contains("render_entity")
                    && p.message.contains("broken-query")
            }),
            "the failing render must be logged at ERROR by the ui_watcher — that tier is what \
             `inv-no-observed-errors` reads. Captured: {problems:#?}"
        );
    });
}

// =============================================================================
// Structural Hot-Swap
// =============================================================================

#[test]
fn watch_ui_structural_change_triggers_new_structure_event() {
    let rt = runtime();
    rt.block_on(async {
        let env = TestEnvironmentBuilder::new()
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

        let (_first_render_expr, mut watch) = env
            .watch_ui_first_structure(&EntityUri::block("evolving-heading"))
            .await
            .expect("watch_ui should succeed");

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
        let new_render_expr =
            TestEnvironment::wait_for_next_structure(&mut watch, Duration::from_secs(10))
                .await
                .expect("Should receive new Structure event after query change");

        // The new render_expr should still be valid (not an error)
        // Any non-error expression is fine.
        if let holon_api::render_types::RenderExpr::FunctionCall { name, .. } = &new_render_expr {
            assert_ne!(
                name, "error",
                "Updated query should produce valid render expression"
            );
        }
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
        let (render_expr, _watch) = env
            .watch_ui_first_structure(&EntityUri::block("heading-1"))
            .await
            .expect("watch_ui should succeed");

        let engine_clone = Arc::clone(env.engine());
        let data_rows: Vec<Arc<HashMap<String, Value>>> = vec![];

        let display_tree = tokio::task::spawn_blocking(move || {
            let services = holon_app::HeadlessBuilderServices::new(engine_clone);
            let ctx = holon_frontend::RenderContext::default().with_data_rows(data_rows);
            services.interpret(&render_expr, &ctx).snapshot()
        })
        .await
        .expect("spawn_blocking panicked");

        // Every EditableText with operations should have triggers
        // (inv-viewmodel-editable-text-triggers)
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

#[test]
fn test_keybinding_join_on_operations() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
    runtime.clone().block_on(async {
        use holon_api::input_types::Key;
        use holon_api::input_types::KeyChord;
        use holon_integration_tests::TestEnvironmentBuilder;

        let env = TestEnvironmentBuilder::new()
            .with_org_file(
                "test.org",
                "* Hello\n:PROPERTIES:\n:ID: test-block\n:END:\n",
            )
            .build(runtime.clone())
            .await
            .expect("Failed to build test environment");

        let reactive = env
            .reactive_engine
            .get()
            .expect("ReactiveEngine should be available");

        // Verify keybinding registry
        let bindings = reactive.key_bindings().lock_ref().clone();
        assert!(
            bindings.contains_key("cycle_task_state"),
            "cycle_task_state missing from key_bindings: {:?}",
            bindings.keys().collect::<Vec<_>>()
        );

        // Directly interpret a block through the profile → render_entity path.
        // This bypasses the root layout query (which needs navigation focus)
        // and tests the keybinding join at the shadow interpreter level.
        let services: Arc<dyn holon_frontend::reactive::BuilderServices> = reactive.clone();
        let row: HashMap<String, Value> = [
            ("id".into(), Value::String("block:test-block".into())),
            ("content".into(), Value::String("Hello".into())),
            ("content_type".into(), Value::String("text".into())),
            ("entity_name".into(), Value::String("block".into())),
        ]
        .into();

        let render_expr = holon_api::RenderExpr::FunctionCall {
            name: "render_entity".to_string(),
            args: vec![],
        };

        let ctx = holon_frontend::RenderContext::default().with_data_rows(vec![Arc::new(row)]);
        let vm = services.interpret(&render_expr, &ctx);

        let entity_ids = holon_frontend::focus_path::collect_all_entity_ids(&vm);
        eprintln!("[test] Entities: {:?}", entity_ids);
        assert!(
            !entity_ids.is_empty(),
            "No entities from render_entity interpretation"
        );

        // Check that operations on the ViewModel have keybindings joined
        let all_ops: Vec<_> = vm
            .operations
            .iter()
            .map(|op| format!("{}(kb={:?})", op.descriptor.name, op.descriptor.key_chord()))
            .collect();
        eprintln!("[test] Top-level ops: {:?}", all_ops);
        let has_keybinding = vm
            .operations
            .iter()
            .any(|op| op.descriptor.key_chord().is_some());
        assert!(
            has_keybinding,
            "No operation on the ViewModel has a keybinding set. The keybinding join in \
             with_operations() is not working. Ops: {all_ops:?}"
        );

        // Try Cmd+Enter on the block
        let chord = KeyChord::new(&[Key::Cmd, Key::Enter]);
        let input = holon_frontend::input::WidgetInput::KeyChord {
            keys: chord.0.clone(),
        };
        let action = holon_frontend::focus_path::bubble_input_oneshot(
            &vm,
            &holon_api::EntityUri::block("test-block"),
            &input,
        );

        match &action {
            Some(holon_frontend::input::InputAction::ExecuteOperation { operation, .. }) => {
                assert_eq!(
                    operation.name, "cycle_task_state",
                    "Expected cycle_task_state but got {}",
                    operation.name
                );
            }
            _ => {
                panic!(
                    "Keychord Cmd+Enter did NOT match on block:test-block. Action: {action:?}, \
                     Entities: {entity_ids:?}, Ops: {all_ops:?}"
                );
            }
        }
    });
}
