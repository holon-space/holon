//! Tests for the reactive ViewModel architecture.
//!
//! Uses `harness = false` + `gpui::run_test_once` to avoid the
//! `#[gpui::test]` recursion limit issue.
//!
//! Run: cargo test -p holon-gpui --test reactive_vm_test

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use futures_signals::signal::{Mutable, SignalExt};
use gpui::*;

use holon_api::render_types::RenderExpr;
use holon_gpui::reactive_vm_poc::*;

// ── ToggleView — bridges a Mutable<bool> signal to GPUI ────────────────

struct ToggleView {
    expanded: Mutable<bool>,
    signal_fires: Arc<Mutex<usize>>,
}

impl ToggleView {
    fn new(expanded: Mutable<bool>, cx: &mut Context<Self>) -> Self {
        let fires = Arc::new(Mutex::new(0usize));
        let signal = expanded.signal();
        let fires_clone = fires.clone();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            let mut stream = signal.to_stream();
            stream.next().await;
            while stream.next().await.is_some() {
                *fires_clone.lock().unwrap() += 1;
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
        Self {
            expanded,
            signal_fires: fires,
        }
    }

    fn fires(&self) -> usize {
        *self.signal_fires.lock().unwrap()
    }
}

impl Render for ToggleView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(if self.expanded.get() {
            "open"
        } else {
            "closed"
        })
    }
}

// ── Test runner ────────────────────────────────────────────────────────

fn run(name: &str, f: impl FnOnce(&mut TestAppContext) + std::panic::UnwindSafe + 'static) {
    eprint!("  test {name} ... ");
    gpui::run_test_once(
        0,
        Box::new(move |dispatcher| {
            let cx = &mut TestAppContext::build(dispatcher, None);
            f(cx);
        }),
    );
    eprintln!("ok");
}

fn interp() -> Arc<dyn Interpreter> {
    default_interpreter()
}

fn main() {
    eprintln!("\nrunning reactive_vm tests\n");

    // ── Mutable signal basics ──────────────────────────────────────────

    run("mutable_bool_signal_fires_on_toggle", |cx| {
        let expanded = Mutable::new(false);
        let view = cx.new(|cx| ToggleView::new(expanded.clone(), cx));

        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.fires()), 0);

        expanded.set(true);
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.fires()), 1);

        expanded.set(false);
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.fires()), 2);
    });

    run("mutable_clone_preserves_subscriptions", |cx| {
        let original = Mutable::new(false);
        let view = cx.new(|cx| ToggleView::new(original.clone(), cx));
        cx.run_until_parked();

        let cloned = original.clone();
        cloned.set(true);
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.fires()), 1);

        original.set(false);
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.fires()), 2);
        assert!(!cloned.get());
    });

    run("fresh_mutable_loses_subscriptions", |cx| {
        let original = Mutable::new(false);
        let view = cx.new(|cx| ToggleView::new(original.clone(), cx));
        cx.run_until_parked();

        let fresh = Mutable::new(original.get());
        original.set(true);
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.fires()), 1);

        fresh.set(false);
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |v, _| v.fires()), 1);
    });

    // ── ItemNode push-down ─────────────────────────────────────────────

    run("shared_template_pushes_to_all_items", |cx| {
        let shared_tmpl = Mutable::new(tree_template());
        let item0 = cx.new(|cx| {
            ItemNode::new(
                Arc::new(make_row("1", "Buy milk", "high")),
                shared_tmpl.clone(),
                interp(),
                cx,
            )
        });
        let item1 = cx.new(|cx| {
            ItemNode::new(
                Arc::new(make_row("2", "Write docs", "low")),
                shared_tmpl.clone(),
                interp(),
                cx,
            )
        });
        cx.run_until_parked();

        shared_tmpl.set(table_template());
        cx.run_until_parked();

        let s0 = item0.read_with(cx, |v, _| v.snapshot());
        let s1 = item1.read_with(cx, |v, _| v.snapshot());
        assert!(s0.starts_with("│"), "item0: {s0}");
        assert!(s1.starts_with("│"), "item1: {s1}");
    });

    run("data_push_reinterprets_only_target_item", |cx| {
        let shared_tmpl = Mutable::new(tree_template());
        let item0 = cx.new(|cx| {
            ItemNode::new(
                Arc::new(make_row("1", "Buy milk", "high")),
                shared_tmpl.clone(),
                interp(),
                cx,
            )
        });
        let item1 = cx.new(|cx| {
            ItemNode::new(
                Arc::new(make_row("2", "Write docs", "low")),
                shared_tmpl.clone(),
                interp(),
                cx,
            )
        });
        cx.run_until_parked();
        let before_1 = item1.read_with(cx, |v, _| v.snapshot());

        item0.read_with(cx, |v, _| {
            v.data.set(Arc::new(make_row("1", "Buy oat milk", "high")));
        });
        cx.run_until_parked();

        assert!(item0
            .read_with(cx, |v, _| v.snapshot())
            .contains("Buy oat milk"));
        assert_eq!(item1.read_with(cx, |v, _| v.snapshot()), before_1);
    });

    run("template_then_data_uses_new_template", |cx| {
        let shared_tmpl = Mutable::new(tree_template());
        let item = cx.new(|cx| {
            ItemNode::new(
                Arc::new(make_row("1", "Buy milk", "high")),
                shared_tmpl.clone(),
                interp(),
                cx,
            )
        });
        cx.run_until_parked();

        shared_tmpl.set(table_template());
        cx.run_until_parked();

        item.read_with(cx, |v, _| {
            v.data.set(Arc::new(make_row("1", "Buy oat milk", "high")));
        });
        cx.run_until_parked();

        let snap = item.read_with(cx, |v, _| v.snapshot());
        assert!(
            snap.starts_with("│") && snap.contains("Buy oat milk"),
            "{snap}"
        );
    });

    // ── ReactiveNode structural changes ────────────────────────────────

    run("parent_data_change_propagates_to_children", |cx| {
        let data = Arc::new(make_row("1", "Hello", "high"));
        let root_expr = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        );

        let root = cx.new(|cx| ReactiveNode::new(root_expr, data, interp(), cx));
        cx.run_until_parked();

        let child_displays: Vec<String> = root.read_with(cx, |r, cx| {
            r.children
                .iter()
                .map(|c| c.read(cx).display.get_cloned())
                .collect()
        });
        assert!(
            child_displays[0].contains("Hello"),
            "child 0: {}",
            child_displays[0]
        );
        assert!(
            child_displays[1].contains("high"),
            "child 1: {}",
            child_displays[1]
        );

        root.read_with(cx, |r, _| {
            r.data.set(Arc::new(make_row("1", "World", "low")));
        });
        cx.run_until_parked();

        let after: Vec<String> = root.read_with(cx, |r, cx| {
            r.children
                .iter()
                .map(|c| c.read(cx).display.get_cloned())
                .collect()
        });
        assert!(after[0].contains("World"), "child 0 after: {}", after[0]);
        assert!(after[1].contains("low"), "child 1 after: {}", after[1]);
    });

    run("structural_expr_change_adds_and_removes_children", |cx| {
        let data = Arc::new(make_row("1", "Hello", "high"));
        let expr_2 = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        );
        let root = cx.new(|cx| ReactiveNode::new(expr_2, data, interp(), cx));
        cx.run_until_parked();
        assert_eq!(root.read_with(cx, |r, _| r.children.len()), 2);

        let expr_3 = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
                positional_arg(expr("icon", vec![positional_arg(literal_str("star"))])),
            ],
        );
        root.update(cx, |r, cx| r.apply_expr(expr_3, cx));
        cx.run_until_parked();
        assert_eq!(root.read_with(cx, |r, _| r.children.len()), 3);

        let expr_1 = expr(
            "row",
            vec![positional_arg(expr(
                "text",
                vec![positional_arg(col_ref("content"))],
            ))],
        );
        root.update(cx, |r, cx| r.apply_expr(expr_1, cx));
        cx.run_until_parked();
        assert_eq!(root.read_with(cx, |r, _| r.children.len()), 1);
    });

    run("data_change_propagates_to_newly_added_children", |cx| {
        let data = Arc::new(make_row("1", "Hello", "high"));
        let initial = expr(
            "row",
            vec![positional_arg(expr(
                "text",
                vec![positional_arg(col_ref("content"))],
            ))],
        );
        let root = cx.new(|cx| ReactiveNode::new(initial, data, interp(), cx));
        cx.run_until_parked();

        let expanded = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        );
        root.update(cx, |r, cx| r.apply_expr(expanded, cx));
        cx.run_until_parked();

        root.read_with(cx, |r, _| {
            r.data.set(Arc::new(make_row("1", "Changed", "low")));
        });
        cx.run_until_parked();

        let displays: Vec<String> = root.read_with(cx, |r, cx| {
            r.children
                .iter()
                .map(|c| c.read(cx).display.get_cloned())
                .collect()
        });
        assert!(
            displays[0].contains("Changed"),
            "original child: {}",
            displays[0]
        );
        assert!(displays[1].contains("low"), "new child: {}", displays[1]);
    });

    run("children_share_parent_data_mutable", |cx| {
        let data = Arc::new(make_row("demo", "Hello World", "high"));
        let root_expr = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        );
        let root = cx.new(|cx| ReactiveNode::new(root_expr, data, interp(), cx));
        cx.run_until_parked();

        root.read_with(cx, |r, _| {
            r.data.set(Arc::new(make_row("demo", "Changed!", "low")));
        });

        let child_content = root.read_with(cx, |r, cx| {
            r.children[0]
                .read(cx)
                .data
                .get_cloned()
                .get("content")
                .unwrap()
                .to_display_string()
        });
        assert_eq!(
            child_content, "Changed!",
            "child.data.get() must return parent's new value SYNCHRONOUSLY. \
             Got '{child_content}' — Mutables are independent, not shared."
        );
    });

    run("data_change_updates_gpui_rendered_output", |cx| {
        let data = Arc::new(make_row("demo", "Hello World", "high"));
        let root_expr = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        );

        struct TestWrapper {
            root: Entity<ReactiveNode>,
            rendered_text: Arc<Mutex<String>>,
        }
        impl Render for TestWrapper {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                let texts: Vec<String> = self
                    .root
                    .read(_cx)
                    .children
                    .iter()
                    .map(|c| c.read(_cx).display.get_cloned())
                    .collect();
                let combined = texts.join(" | ");
                *self.rendered_text.lock().unwrap() = combined.clone();
                div()
                    .child(self.root.clone())
                    .child(div().id("debug-output").child(combined))
            }
        }

        let rendered_text = Arc::new(Mutex::new(String::new()));
        let rendered_text_clone = rendered_text.clone();

        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(gpui::Bounds {
                        origin: gpui::Point::default(),
                        size: gpui::size(px(400.0), px(200.0)),
                    })),
                    ..Default::default()
                },
                |_, cx| {
                    let root =
                        cx.new(|cx| ReactiveNode::new(root_expr, data, default_interpreter(), cx));
                    cx.new(|_cx| TestWrapper {
                        root,
                        rendered_text: rendered_text_clone,
                    })
                },
            )
            .expect("open_window")
        });

        cx.run_until_parked();

        let initial = rendered_text.lock().unwrap().clone();
        assert!(initial.contains("Hello World"), "initial render: {initial}");

        window
            .update(cx, |wrapper, _window, cx| {
                wrapper
                    .root
                    .read(cx)
                    .data
                    .set(Arc::new(make_row("demo", "Updated!", "low")));
            })
            .unwrap();

        cx.run_until_parked();

        let after = rendered_text.lock().unwrap().clone();
        assert!(
            after.contains("Updated!"),
            "GPUI rendered output after data change: '{}' (expected 'Updated!'). \
             The display Mutable updates but GPUI doesn't re-render the children.",
            after
        );
    });

    run("child_render_called_after_data_change", |cx| {
        let data = Arc::new(make_row("demo", "Hello World", "high"));
        let root_expr = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        );

        struct OpaqueWrapper {
            root: Entity<ReactiveNode>,
        }
        impl Render for OpaqueWrapper {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                div().child(self.root.clone())
            }
        }

        let data_handle: Arc<Mutex<Option<Mutable<Arc<DataRow>>>>> = Arc::new(Mutex::new(None));
        let child_counters: Arc<Mutex<Vec<Arc<std::sync::atomic::AtomicUsize>>>> =
            Arc::new(Mutex::new(vec![]));

        let dh = data_handle.clone();
        let cc = child_counters.clone();

        let _window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(gpui::Bounds {
                        origin: gpui::Point::default(),
                        size: gpui::size(px(400.0), px(200.0)),
                    })),
                    ..Default::default()
                },
                |_, cx| {
                    let root =
                        cx.new(|cx| ReactiveNode::new(root_expr, data, default_interpreter(), cx));
                    *dh.lock().unwrap() = Some(root.read(cx).data.clone());
                    *cc.lock().unwrap() = root
                        .read(cx)
                        .children
                        .iter()
                        .map(|c| c.read(cx).render_count.clone())
                        .collect();
                    cx.new(|_cx| OpaqueWrapper { root })
                },
            )
            .expect("open_window")
        });

        cx.run_until_parked();

        let counters = child_counters.lock().unwrap().clone();
        let before: Vec<usize> = counters.iter().map(|c| c.load(Ordering::Relaxed)).collect();
        assert!(
            before.iter().all(|&c| c >= 1),
            "children must have rendered at least once: {before:?}"
        );

        let data_m = data_handle.lock().unwrap().clone().unwrap();
        data_m.set(Arc::new(make_row("demo", "Updated via signal!", "low")));

        cx.run_until_parked();

        let after: Vec<usize> = counters.iter().map(|c| c.load(Ordering::Relaxed)).collect();
        for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
            assert!(
                a > b,
                "child[{i}] render_count did not increase after data change \
                 (before={b}, after={a}). GPUI did not re-render the child \
                 even though its cx.notify() was called from the signal."
            );
        }
    });

    // ── Interpreter trait boundary ─────────────────────────────────────

    run("custom_interpreter_used_by_nodes", |cx| {
        struct PrefixInterpreter;
        impl Interpreter for PrefixInterpreter {
            fn interpret(&self, expr: &RenderExpr, row: &DataRow) -> String {
                format!("CUSTOM:{}", interpret_expr(expr, row))
            }
        }

        let custom: Arc<dyn Interpreter> = Arc::new(PrefixInterpreter);
        let data = Arc::new(make_row("1", "Hello", "high"));
        let root_expr = expr(
            "row",
            vec![positional_arg(expr(
                "text",
                vec![positional_arg(col_ref("content"))],
            ))],
        );

        let root = cx.new(|cx| ReactiveNode::new(root_expr, data, custom, cx));
        cx.run_until_parked();

        // Check via tree_snapshot (the synchronous read path — must use trait)
        let snap = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        let leaves = snap.leaf_displays();
        assert!(
            leaves[0].starts_with("CUSTOM:"),
            "tree_snapshot must use custom interpreter: {}",
            leaves[0]
        );

        // Also check display Mutable (signal path — must use trait)
        let child_display = root.read_with(cx, |r, cx| r.children[0].read(cx).snapshot());
        assert!(
            child_display.starts_with("CUSTOM:"),
            "signal path must use custom interpreter: {child_display}"
        );

        root.read_with(cx, |r, _| {
            r.data.set(Arc::new(make_row("1", "World", "low")));
        });
        // Check tree_snapshot BEFORE signal fires — live path must use trait
        let snap2 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        let leaves2 = snap2.leaf_displays();
        assert!(
            leaves2[0].contains("CUSTOM:") && leaves2[0].contains("World"),
            "live tree_snapshot after data change: {}",
            leaves2[0]
        );
    });

    // ── TreeSnapshot — synchronous read for MCP/PBT ─────────────────────

    run("tree_snapshot_reads_full_tree_synchronously", |cx| {
        let data = Arc::new(make_row("1", "Hello", "high"));
        let root_expr = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        );

        let root = cx.new(|cx| ReactiveNode::new(root_expr, data, interp(), cx));
        cx.run_until_parked();

        let snap = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert_eq!(snap.kind, "row");
        assert_eq!(snap.children.len(), 2);
        let leaves = snap.leaf_displays();
        assert!(leaves[0].contains("Hello"), "leaf 0: {}", leaves[0]);
        assert!(leaves[1].contains("high"), "leaf 1: {}", leaves[1]);

        // Change data → snapshot reflects new values synchronously
        root.read_with(cx, |r, _| {
            r.data.set(Arc::new(make_row("1", "World", "low")));
        });
        // NO run_until_parked — snapshot reads live Mutables
        let snap2 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        let leaves2 = snap2.leaf_displays();
        assert!(
            leaves2[0].contains("World"),
            "live snapshot leaf 0: {}",
            leaves2[0]
        );
        assert!(
            leaves2[1].contains("low"),
            "live snapshot leaf 1: {}",
            leaves2[1]
        );
    });

    // ── Nested collections — multi-level signal propagation ──────────────

    run(
        "nested_collections_data_propagates_through_3_levels",
        |cx| {
            // row(bold(text(col("content"))), row(badge(col("priority")), text(col("id"))))
            // Level 0: row  (2 children)
            // Level 1: bold (1 child), row (2 children)
            // Level 2: text (leaf), badge (leaf), text (leaf)
            let nested_expr = expr(
                "row",
                vec![
                    positional_arg(expr(
                        "bold",
                        vec![positional_arg(expr(
                            "text",
                            vec![positional_arg(col_ref("content"))],
                        ))],
                    )),
                    positional_arg(expr(
                        "row",
                        vec![
                            positional_arg(expr(
                                "badge",
                                vec![positional_arg(col_ref("priority"))],
                            )),
                            positional_arg(expr("text", vec![positional_arg(col_ref("id"))])),
                        ],
                    )),
                ],
            );

            let data = Arc::new(make_row("42", "Deep nesting", "critical"));
            let root = cx.new(|cx| ReactiveNode::new(nested_expr, data, interp(), cx));
            cx.run_until_parked();

            let snap = cx.read(|cx| root.read(cx).tree_snapshot(cx));
            assert_eq!(snap.kind, "row");
            assert_eq!(snap.children.len(), 2);
            assert_eq!(snap.children[0].kind, "bold");
            assert_eq!(snap.children[0].children.len(), 1);
            assert_eq!(snap.children[1].kind, "row");
            assert_eq!(snap.children[1].children.len(), 2);

            let leaves = snap.leaf_displays();
            assert_eq!(leaves.len(), 3);
            assert!(leaves[0].contains("Deep nesting"), "leaf 0: {}", leaves[0]);
            assert!(leaves[1].contains("critical"), "leaf 1: {}", leaves[1]);
            assert!(leaves[2].contains("42"), "leaf 2: {}", leaves[2]);

            // Change data at root — all 3 leaves must update
            root.read_with(cx, |r, _| {
                r.data.set(Arc::new(make_row("99", "Updated!", "low")));
            });
            cx.run_until_parked();

            let snap2 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
            let leaves2 = snap2.leaf_displays();
            assert!(
                leaves2[0].contains("Updated!"),
                "after: leaf 0: {}",
                leaves2[0]
            );
            assert!(leaves2[1].contains("low"), "after: leaf 1: {}", leaves2[1]);
            assert!(leaves2[2].contains("99"), "after: leaf 2: {}", leaves2[2]);
        },
    );

    run("structural_change_at_nested_level", |cx| {
        // Start: row(text(col("content")))
        // Then change to: row(row(text(col("content")), badge(col("priority"))))
        let initial = expr(
            "row",
            vec![positional_arg(expr(
                "text",
                vec![positional_arg(col_ref("content"))],
            ))],
        );
        let data = Arc::new(make_row("1", "Hello", "high"));
        let root = cx.new(|cx| ReactiveNode::new(initial, data, interp(), cx));
        cx.run_until_parked();

        let snap1 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert_eq!(snap1.leaf_displays().len(), 1);

        // Expand to nested structure
        let expanded = expr(
            "row",
            vec![positional_arg(expr(
                "row",
                vec![
                    positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                    positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
                ],
            ))],
        );
        root.update(cx, |r, cx| r.apply_expr(expanded, cx));
        cx.run_until_parked();

        let snap2 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert_eq!(snap2.children.len(), 1);
        assert_eq!(snap2.children[0].kind, "row");
        assert_eq!(snap2.children[0].children.len(), 2);

        let leaves = snap2.leaf_displays();
        assert!(leaves[0].contains("Hello"), "nested leaf 0: {}", leaves[0]);
        assert!(leaves[1].contains("high"), "nested leaf 1: {}", leaves[1]);

        // Data change propagates through the new nesting
        root.read_with(cx, |r, _| {
            r.data.set(Arc::new(make_row("1", "Changed", "low")));
        });
        cx.run_until_parked();

        let snap3 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        let leaves3 = snap3.leaf_displays();
        assert!(
            leaves3[0].contains("Changed"),
            "after change: leaf 0: {}",
            leaves3[0]
        );
        assert!(
            leaves3[1].contains("low"),
            "after change: leaf 1: {}",
            leaves3[1]
        );
    });

    // ── Render invariant: fresh output between data.set() and signal ───
    //
    // The gap between data.set() and signal propagation is where the
    // frame-timing bug lives.  After data.set() but BEFORE run_until_parked:
    //   - data.get_cloned()    → NEW value  (Mutable is synchronous)
    //   - display.get_cloned() → OLD value  (signal hasn't fired)
    //
    // render() must produce output from the NEW data, not the stale display.
    // tree_snapshot() uses the same interpreter.interpret(expr, data) path
    // as render(), so if tree_snapshot is fresh, render is fresh.
    //
    // This test catches "render reads stale display Mutable" in the headless
    // harness — no real event loop needed.

    run("render_uses_live_data_not_stale_display", |cx| {
        let data = Arc::new(make_row("1", "Original", "high"));
        let root_expr = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        );

        let root = cx.new(|cx| ReactiveNode::new(root_expr, data, interp(), cx));
        cx.run_until_parked();

        // Set data — signal has NOT fired yet (no run_until_parked)
        root.read_with(cx, |r, _| {
            r.data.set(Arc::new(make_row("1", "Fresh!", "low")));
        });

        // Prove the gap exists: display is stale, data is fresh
        let (display_val, data_content) = root.read_with(cx, |r, cx| {
            let child = r.children[0].read(cx);
            let display = child.display.get_cloned();
            let data = child
                .data
                .get_cloned()
                .get("content")
                .unwrap()
                .to_display_string();
            (display, data)
        });
        assert!(
            display_val.contains("Original"),
            "display should still be stale: {display_val}"
        );
        assert!(
            data_content.contains("Fresh!"),
            "data should already be fresh: {data_content}"
        );

        // tree_snapshot uses interpreter.interpret(expr, data) — same path as
        // render().  If this returns fresh data, render() would too.
        let snap = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        let leaves = snap.leaf_displays();
        assert!(
            leaves[0].contains("Fresh!"),
            "tree_snapshot (= render path) must use live data, not stale display. \
             Got '{}' — render() would show stale content for one frame.",
            leaves[0]
        );
        assert!(
            leaves[1].contains("low"),
            "tree_snapshot leaf 1 must use live data. Got '{}'",
            leaves[1]
        );
    });

    // ── Expand toggle — lazy loading ─────────────────────────────────────

    run("expandable_starts_collapsed_no_children", |cx| {
        let data = Arc::new(make_row("1", "Hello", "high"));
        let root_expr = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        );

        let root = cx.new(|cx| ReactiveNode::new_expandable(root_expr, data, interp(), false, cx));
        cx.run_until_parked();

        let snap = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert_eq!(snap.expanded, Some(false));
        assert!(
            snap.children.is_empty(),
            "collapsed should have no children"
        );
        assert!(snap.display.contains("Hello"), "display: {}", snap.display);
    });

    run("expand_toggle_creates_children_lazily", |cx| {
        let data = Arc::new(make_row("1", "Hello", "high"));
        let root_expr = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        );

        let root = cx.new(|cx| ReactiveNode::new_expandable(root_expr, data, interp(), false, cx));
        cx.run_until_parked();

        // Expand — children should be built
        root.read_with(cx, |r, _| {
            r.expanded.as_ref().unwrap().set(true);
        });
        cx.run_until_parked();

        let snap = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert_eq!(snap.expanded, Some(true));
        assert_eq!(snap.children.len(), 2);
        let leaves = snap.leaf_displays();
        assert!(leaves[0].contains("Hello"), "leaf 0: {}", leaves[0]);
        assert!(leaves[1].contains("high"), "leaf 1: {}", leaves[1]);
    });

    run("collapse_drops_children", |cx| {
        let data = Arc::new(make_row("1", "Hello", "high"));
        let root_expr = expr(
            "row",
            vec![positional_arg(expr(
                "text",
                vec![positional_arg(col_ref("content"))],
            ))],
        );

        let root = cx.new(|cx| ReactiveNode::new_expandable(root_expr, data, interp(), true, cx));
        cx.run_until_parked();

        let snap1 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert_eq!(snap1.children.len(), 1);

        // Collapse
        root.read_with(cx, |r, _| {
            r.expanded.as_ref().unwrap().set(false);
        });
        cx.run_until_parked();

        let snap2 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert!(snap2.children.is_empty());

        // Re-expand
        root.read_with(cx, |r, _| {
            r.expanded.as_ref().unwrap().set(true);
        });
        cx.run_until_parked();

        let snap3 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert_eq!(snap3.children.len(), 1);
    });

    run("data_change_after_expand_propagates", |cx| {
        let data = Arc::new(make_row("1", "Hello", "high"));
        let root_expr = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        );

        let root = cx.new(|cx| ReactiveNode::new_expandable(root_expr, data, interp(), false, cx));
        cx.run_until_parked();

        // Expand
        root.read_with(cx, |r, _| {
            r.expanded.as_ref().unwrap().set(true);
        });
        cx.run_until_parked();

        // Change data — children (created after expand) must see it
        root.read_with(cx, |r, _| {
            r.data.set(Arc::new(make_row("1", "Updated!", "low")));
        });
        cx.run_until_parked();

        let snap = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        let leaves = snap.leaf_displays();
        assert!(
            leaves[0].contains("Updated!"),
            "after data change: {}",
            leaves[0]
        );
        assert!(
            leaves[1].contains("low"),
            "after data change: {}",
            leaves[1]
        );
    });

    // ── Per-item data collections ────────────────────────────────────────

    run("collection_children_have_independent_data", |cx| {
        let template = expr("text", vec![positional_arg(col_ref("content"))]);
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(make_row("1", "Alpha", "high")),
            Arc::new(make_row("2", "Beta", "low")),
            Arc::new(make_row("3", "Gamma", "med")),
        ];

        let root = cx.new(|cx| ReactiveNode::new_collection(template, rows.clone(), interp(), cx));
        cx.run_until_parked();

        // Each child shows its own row's content
        let snap = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        let leaves = snap.leaf_displays();
        assert_eq!(leaves.len(), 3);
        assert!(leaves[0].contains("Alpha"), "child 0: {}", leaves[0]);
        assert!(leaves[1].contains("Beta"), "child 1: {}", leaves[1]);
        assert!(leaves[2].contains("Gamma"), "child 2: {}", leaves[2]);
    });

    run("collection_update_one_child_doesnt_affect_others", |cx| {
        let template = expr("text", vec![positional_arg(col_ref("content"))]);
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(make_row("1", "Alpha", "high")),
            Arc::new(make_row("2", "Beta", "low")),
        ];

        let root = cx.new(|cx| ReactiveNode::new_collection(template, rows.clone(), interp(), cx));
        cx.run_until_parked();

        // Update child 0's data only
        root.read_with(cx, |r, cx| {
            let child0 = r.children[0].read(cx);
            child0
                .data
                .set(Arc::new(make_row("1", "Alpha Updated", "high")));
        });
        cx.run_until_parked();

        let snap = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        let leaves = snap.leaf_displays();
        assert!(
            leaves[0].contains("Alpha Updated"),
            "child 0 should update: {}",
            leaves[0]
        );
        assert!(
            leaves[1].contains("Beta"),
            "child 1 should be unchanged: {}",
            leaves[1]
        );
    });

    // ── Signal task cleanup on entity drop ─────────────────────────────
    //
    // Validates that dropping a GPUI entity cancels its signal poll task.
    // Before the fix (`.detach()`), the task would leak and keep running.
    // After the fix (stored `Task` handle), dropping the entity drops the
    // Task which cancels the future.

    run("dropped_entity_signal_task_stops", |cx| {
        let leaf_expr = expr("text", vec![positional_arg(col_ref("content"))]);
        let node = cx.new(|cx| {
            ReactiveNode::new(
                leaf_expr,
                Arc::new(make_row("1", "Hello", "high")),
                interp(),
                cx,
            )
        });
        cx.run_until_parked();

        // Grab the data Mutable from the node so we can push to it after drop
        let data_handle = node.read_with(cx, |r, _| r.data.clone());
        let display_handle = node.read_with(cx, |r, _| r.display.clone());

        let display_before = display_handle.get_cloned();
        assert!(
            display_before.contains("Hello"),
            "before drop: {display_before}"
        );

        // Drop the entity handle. GPUI defers cleanup to flush_effects(),
        // which isn't triggered by run_until_parked() alone. We create a
        // throwaway entity to trigger flush_effects, which frees the dropped
        // entity and cancels its signal tasks.
        drop(node);
        let _flush = cx.new(|_cx| ());
        cx.run_until_parked();

        // Push new data to the Mutable — if signal task leaked, it would
        // update the display Mutable
        data_handle.set(Arc::new(make_row("1", "LEAKED", "high")));
        cx.run_until_parked();

        // Display should NOT have been updated — the signal task was cancelled
        let display_after = display_handle.get_cloned();
        assert!(
            !display_after.contains("LEAKED"),
            "signal task leaked after entity drop! display={display_after}"
        );
    });

    run("dropped_collection_child_signal_stops", |cx| {
        let template = expr("text", vec![positional_arg(col_ref("content"))]);
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(make_row("1", "Alpha", "high")),
            Arc::new(make_row("2", "Beta", "low")),
        ];

        let root = cx.new(|cx| ReactiveNode::new_collection(template, rows, interp(), cx));
        cx.run_until_parked();

        // Grab child 1's data + display handles
        let (child1_data, child1_display) = root.read_with(cx, |r, cx| {
            let child = r.children[1].read(cx);
            (child.data.clone(), child.display.clone())
        });

        assert!(child1_display.get_cloned().contains("Beta"));

        // Remove child 1 — root.update() triggers flush_effects which
        // frees the dropped child entity. The signal task's next
        // this.update() call returns Err and the task breaks.
        root.update(cx, |r, _| {
            r.children.truncate(1);
        });
        let _flush = cx.new(|_cx| ());
        cx.run_until_parked();

        // Push data to the orphaned child's Mutable
        child1_data.set(Arc::new(make_row("2", "LEAKED", "low")));
        cx.run_until_parked();

        // Signal task should have been cancelled with the entity
        let display_after = child1_display.get_cloned();
        assert!(
            !display_after.contains("LEAKED"),
            "child signal task leaked after removal! display={display_after}"
        );
    });

    run("collection_template_change_propagates_to_all", |cx| {
        let template = Mutable::new(expr("text", vec![positional_arg(col_ref("content"))]));
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(make_row("1", "Alpha", "high")),
            Arc::new(make_row("2", "Beta", "low")),
        ];

        let root = cx.new(|cx| {
            ReactiveNode::new_collection_with_template(template.clone(), rows.clone(), interp(), cx)
        });
        cx.run_until_parked();

        // Switch to badge template — all children should change
        template.set(expr("badge", vec![positional_arg(col_ref("priority"))]));
        cx.run_until_parked();

        let snap = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        let leaves = snap.leaf_displays();
        assert!(leaves[0].contains("[high]"), "child 0: {}", leaves[0]);
        assert!(leaves[1].contains("[low]"), "child 1: {}", leaves[1]);
    });

    // ── VecDiff-driven ReactiveCollection ──────────────────────────────
    //
    // Validates MutableVec-backed persistent collections: insert, remove,
    // update (data-only, no rebuild), and move operations — the VecDiff
    // events that CDC streams produce in production.

    run("reactive_collection_initial_snapshot", |cx| {
        let template = expr("text", vec![positional_arg(col_ref("content"))]);
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(make_row("1", "Alpha", "high")),
            Arc::new(make_row("2", "Beta", "low")),
            Arc::new(make_row("3", "Gamma", "med")),
        ];

        let coll = cx.new(|cx| ReactiveCollection::new(template, rows, interp(), cx));
        cx.run_until_parked();

        let displays = coll.read_with(cx, |c, cx| c.child_displays(cx));
        assert_eq!(displays, vec!["Alpha", "Beta", "Gamma"]);
    });

    run("reactive_collection_insert_at", |cx| {
        let template = expr("text", vec![positional_arg(col_ref("content"))]);
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(make_row("1", "Alpha", "high")),
            Arc::new(make_row("2", "Beta", "low")),
        ];

        let coll = cx.new(|cx| ReactiveCollection::new(template, rows, interp(), cx));
        cx.run_until_parked();

        // Insert at position 1 (between Alpha and Beta)
        coll.update(cx, |c, cx| {
            c.insert(1, Arc::new(make_row("3", "Inserted", "med")), cx);
        });
        cx.run_until_parked();

        let displays = coll.read_with(cx, |c, cx| c.child_displays(cx));
        assert_eq!(displays, vec!["Alpha", "Inserted", "Beta"]);
    });

    run("reactive_collection_remove_at", |cx| {
        let template = expr("text", vec![positional_arg(col_ref("content"))]);
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(make_row("1", "Alpha", "high")),
            Arc::new(make_row("2", "Beta", "low")),
            Arc::new(make_row("3", "Gamma", "med")),
        ];

        let coll = cx.new(|cx| ReactiveCollection::new(template, rows, interp(), cx));
        cx.run_until_parked();

        // Remove middle item
        coll.update(cx, |c, cx| {
            c.remove(1, cx);
        });
        cx.run_until_parked();

        let displays = coll.read_with(cx, |c, cx| c.child_displays(cx));
        assert_eq!(displays, vec!["Alpha", "Gamma"]);
        assert_eq!(coll.read_with(cx, |c, _| c.len()), 2);
    });

    run("reactive_collection_update_at_no_rebuild", |cx| {
        let template = expr("text", vec![positional_arg(col_ref("content"))]);
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(make_row("1", "Alpha", "high")),
            Arc::new(make_row("2", "Beta", "low")),
        ];

        let coll = cx.new(|cx| ReactiveCollection::new(template, rows, interp(), cx));
        cx.run_until_parked();

        // Capture the entity ID of child 0 before update
        let child0_entity_id = coll.read_with(cx, |c, _| c.items.lock_ref()[0].entity_id());

        // Update child 0's data — should just set the Mutable, not rebuild
        cx.read(|cx| {
            coll.read(cx)
                .update_data(0, Arc::new(make_row("1", "Updated", "high")), cx);
        });
        cx.run_until_parked();

        // Same entity — proves no rebuild happened
        let child0_entity_id_after = coll.read_with(cx, |c, _| c.items.lock_ref()[0].entity_id());
        assert_eq!(
            child0_entity_id, child0_entity_id_after,
            "entity should be reused, not rebuilt"
        );

        let displays = coll.read_with(cx, |c, cx| c.child_displays(cx));
        assert_eq!(displays, vec!["Updated", "Beta"]);

        // Signal should have updated the display Mutable too
        let display_mutable = coll.read_with(cx, |c, cx| {
            c.items.lock_ref()[0].read(cx).display.get_cloned()
        });
        assert_eq!(display_mutable, "Updated");
    });

    run("reactive_collection_move_item", |cx| {
        let template = expr("text", vec![positional_arg(col_ref("content"))]);
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(make_row("1", "Alpha", "high")),
            Arc::new(make_row("2", "Beta", "low")),
            Arc::new(make_row("3", "Gamma", "med")),
        ];

        let coll = cx.new(|cx| ReactiveCollection::new(template, rows, interp(), cx));
        cx.run_until_parked();

        // Capture entity IDs before move
        let ids_before: Vec<_> = coll.read_with(cx, |c, _| {
            c.items.lock_ref().iter().map(|e| e.entity_id()).collect()
        });

        // Move item 2 (Gamma) to position 0
        coll.update(cx, |c, cx| {
            c.move_item(2, 0, cx);
        });
        cx.run_until_parked();

        let displays = coll.read_with(cx, |c, cx| c.child_displays(cx));
        assert_eq!(displays, vec!["Gamma", "Alpha", "Beta"]);

        // Same entities, just reordered — no rebuilds
        let ids_after: Vec<_> = coll.read_with(cx, |c, _| {
            c.items.lock_ref().iter().map(|e| e.entity_id()).collect()
        });
        assert_eq!(ids_after[0], ids_before[2], "Gamma entity reused");
        assert_eq!(ids_after[1], ids_before[0], "Alpha entity reused");
        assert_eq!(ids_after[2], ids_before[1], "Beta entity reused");
    });

    run("reactive_collection_remove_cleans_up_signal", |cx| {
        let template = expr("text", vec![positional_arg(col_ref("content"))]);
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(make_row("1", "Alpha", "high")),
            Arc::new(make_row("2", "Beta", "low")),
        ];

        let coll = cx.new(|cx| ReactiveCollection::new(template, rows, interp(), cx));
        cx.run_until_parked();

        // Grab handles to the item we'll remove
        let (removed_data, removed_display) = coll.read_with(cx, |c, cx| {
            let item = c.items.lock_ref()[1].read(cx);
            (item.data.clone(), item.display.clone())
        });

        // Remove it
        coll.update(cx, |c, cx| {
            c.remove(1, cx);
        });
        let _flush = cx.new(|_cx| ());
        cx.run_until_parked();

        // Push data to the removed item — signal should be dead
        removed_data.set(Arc::new(make_row("2", "LEAKED", "low")));
        cx.run_until_parked();

        assert!(
            !removed_display.get_cloned().contains("LEAKED"),
            "removed collection item signal leaked! display={}",
            removed_display.get_cloned()
        );
    });

    run("reactive_collection_full_sequence", |cx| {
        let template = expr("text", vec![positional_arg(col_ref("content"))]);
        let rows: Vec<Arc<DataRow>> = vec![
            Arc::new(make_row("1", "Alpha", "high")),
            Arc::new(make_row("2", "Beta", "low")),
        ];

        let coll = cx.new(|cx| ReactiveCollection::new(template, rows, interp(), cx));
        cx.run_until_parked();

        // 1. Insert
        coll.update(cx, |c, cx| {
            c.insert(2, Arc::new(make_row("3", "Gamma", "med")), cx);
        });
        cx.run_until_parked();
        let d1 = coll.read_with(cx, |c, cx| c.child_displays(cx));
        assert_eq!(d1, vec!["Alpha", "Beta", "Gamma"]);

        // 2. Update (data-only)
        cx.read(|cx| {
            coll.read(cx)
                .update_data(1, Arc::new(make_row("2", "Beta-v2", "low")), cx);
        });
        cx.run_until_parked();
        let d2 = coll.read_with(cx, |c, cx| c.child_displays(cx));
        assert_eq!(d2, vec!["Alpha", "Beta-v2", "Gamma"]);

        // 3. Move Gamma to front
        coll.update(cx, |c, cx| {
            c.move_item(2, 0, cx);
        });
        cx.run_until_parked();
        let d3 = coll.read_with(cx, |c, cx| c.child_displays(cx));
        assert_eq!(d3, vec!["Gamma", "Alpha", "Beta-v2"]);

        // 4. Remove Alpha (now at index 1)
        coll.update(cx, |c, cx| {
            c.remove(1, cx);
        });
        cx.run_until_parked();
        let d4 = coll.read_with(cx, |c, cx| c.child_displays(cx));
        assert_eq!(d4, vec!["Gamma", "Beta-v2"]);

        // 5. Update remaining
        cx.read(|cx| {
            coll.read(cx)
                .update_data(0, Arc::new(make_row("3", "Gamma-v2", "med")), cx);
        });
        cx.run_until_parked();
        let d5 = coll.read_with(cx, |c, cx| c.child_displays(cx));
        assert_eq!(d5, vec!["Gamma-v2", "Beta-v2"]);
    });

    // ── Named arg ordering in apply_expr ──────────────────────────────
    //
    // apply_expr must match children by arg name (not position) when all
    // args are named. Rhai #{} maps have no guaranteed iteration order.

    run("named_args_swapped_order_preserves_children", |cx| {
        // Start: toggle(#{header: text(col("content")), content: badge(col("priority"))})
        let initial = expr(
            "toggle",
            vec![
                named_arg(
                    "header",
                    expr("text", vec![positional_arg(col_ref("content"))]),
                ),
                named_arg(
                    "content",
                    expr("badge", vec![positional_arg(col_ref("priority"))]),
                ),
            ],
        );
        let data = Arc::new(make_row("1", "Hello", "high"));
        let root = cx.new(|cx| ReactiveNode::new(initial, data, interp(), cx));
        cx.run_until_parked();

        // Capture entity IDs of the two children
        let ids_before: Vec<_> = root.read_with(cx, |r, _| {
            r.children.iter().map(|c| c.entity_id()).collect()
        });
        assert_eq!(ids_before.len(), 2);

        // Apply same expression with swapped arg order
        let swapped = expr(
            "toggle",
            vec![
                named_arg(
                    "content",
                    expr("badge", vec![positional_arg(col_ref("priority"))]),
                ),
                named_arg(
                    "header",
                    expr("text", vec![positional_arg(col_ref("content"))]),
                ),
            ],
        );
        root.update(cx, |r, cx| r.apply_expr(swapped, cx));
        cx.run_until_parked();

        // Children should be the SAME entities, just reordered
        let ids_after: Vec<_> = root.read_with(cx, |r, _| {
            r.children.iter().map(|c| c.entity_id()).collect()
        });
        // After swap: new order is [content, header] → entities [old_content, old_header]
        assert_eq!(
            ids_after[0], ids_before[1],
            "content entity should be reused at position 0"
        );
        assert_eq!(
            ids_after[1], ids_before[0],
            "header entity should be reused at position 1"
        );

        // Verify display is still correct — check child nodes (not leaves)
        let snap = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert_eq!(snap.children.len(), 2);
        assert_eq!(
            snap.children[0].kind, "badge",
            "position 0 should be badge (content arg)"
        );
        assert_eq!(
            snap.children[1].kind, "text",
            "position 1 should be text (header arg)"
        );
        assert!(
            snap.children[0].display.contains("[high]"),
            "badge display: {}",
            snap.children[0].display
        );
        assert!(
            snap.children[1].display.contains("Hello"),
            "text display: {}",
            snap.children[1].display
        );
    });

    run("named_args_added_and_removed", |cx| {
        let initial = expr(
            "widget",
            vec![
                named_arg("a", expr("text", vec![positional_arg(col_ref("content"))])),
                named_arg(
                    "b",
                    expr("badge", vec![positional_arg(col_ref("priority"))]),
                ),
            ],
        );
        let data = Arc::new(make_row("1", "Hello", "high"));
        let root = cx.new(|cx| ReactiveNode::new(initial, data, interp(), cx));
        cx.run_until_parked();

        let id_a = root.read_with(cx, |r, _| r.children[0].entity_id());

        // Replace: remove "b", add "c", keep "a"
        let changed = expr(
            "widget",
            vec![
                named_arg("c", expr("icon", vec![positional_arg(col_ref("priority"))])),
                named_arg("a", expr("text", vec![positional_arg(col_ref("content"))])),
            ],
        );
        root.update(cx, |r, cx| r.apply_expr(changed, cx));
        cx.run_until_parked();

        let ids_after: Vec<_> = root.read_with(cx, |r, _| {
            r.children.iter().map(|c| c.entity_id()).collect()
        });
        // "a" entity should be reused (now at position 1)
        assert_eq!(ids_after[1], id_a, "arg 'a' entity should be reused");
        // "c" is new
        assert_ne!(ids_after[0], id_a, "arg 'c' should be a new entity");
        assert_eq!(ids_after.len(), 2);

        let snap = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert_eq!(snap.children[0].kind, "icon");
        assert_eq!(snap.children[1].kind, "text");
        assert!(
            snap.children[0].display.contains("⬡high"),
            "icon display: {}",
            snap.children[0].display
        );
        assert!(
            snap.children[1].display.contains("Hello"),
            "text display: {}",
            snap.children[1].display
        );
    });

    // ── Expand-triggers-interpretation ──────────────────────────────────
    //
    // Validates that expanding a node can trigger interpretation (not just
    // structural decomposition) via the Interpreter trait.

    run("expand_with_interpreted_content", |cx| {
        // Use a custom interpreter that transforms "container" expressions
        // into multiple children — simulating production's interpret() that
        // spawns live queries.
        struct ExpandInterpreter;
        impl Interpreter for ExpandInterpreter {
            fn interpret(&self, expr: &RenderExpr, row: &DataRow) -> String {
                interpret_expr(expr, row)
            }
        }

        let data = Arc::new(make_row("1", "Root", "high"));
        // The expression has 2 positional args — expand should create 2 children
        let root_expr = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        );

        let root = cx.new(|cx| {
            ReactiveNode::new_expandable(
                root_expr,
                data.clone(),
                Arc::new(ExpandInterpreter),
                false,
                cx,
            )
        });
        cx.run_until_parked();

        // Collapsed — no children
        let snap0 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert!(snap0.children.is_empty());

        // Expand
        root.read_with(cx, |r, _| {
            r.expanded.as_ref().unwrap().set(true);
        });
        cx.run_until_parked();

        // Children should exist and have interpreted content
        let snap1 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert_eq!(snap1.children.len(), 2);
        assert!(
            snap1.children[0].display.contains("Root"),
            "child 0: {}",
            snap1.children[0].display
        );
        assert!(
            snap1.children[1].display.contains("[high]"),
            "child 1: {}",
            snap1.children[1].display
        );

        // Change data while expanded — children should update
        root.read_with(cx, |r, _| {
            r.data.set(Arc::new(make_row("1", "Updated", "low")));
        });
        cx.run_until_parked();

        let snap2 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert!(
            snap2.children[0].display.contains("Updated"),
            "after update: {}",
            snap2.children[0].display
        );
        assert!(
            snap2.children[1].display.contains("[low]"),
            "after update: {}",
            snap2.children[1].display
        );
    });

    run("expand_content_lazy_via_interpreter", |cx| {
        // Tracks how many times interpret is called
        use std::sync::atomic::AtomicUsize;
        let call_count = Arc::new(AtomicUsize::new(0));

        struct CountingInterpreter(Arc<AtomicUsize>);
        impl Interpreter for CountingInterpreter {
            fn interpret(&self, expr: &RenderExpr, row: &DataRow) -> String {
                self.0.fetch_add(1, Ordering::Relaxed);
                interpret_expr(expr, row)
            }
        }

        let interp = Arc::new(CountingInterpreter(call_count.clone()));
        let data = Arc::new(make_row("1", "Hello", "high"));
        let root_expr = expr("text", vec![positional_arg(col_ref("content"))]);

        let root = cx.new(|cx| ReactiveNode::new_expandable(root_expr, data, interp, false, cx));
        cx.run_until_parked();

        let count_after_create = call_count.load(Ordering::Relaxed);

        // Expand — should trigger interpretation for children
        root.read_with(cx, |r, _| {
            r.expanded.as_ref().unwrap().set(true);
        });
        cx.run_until_parked();

        let count_after_expand = call_count.load(Ordering::Relaxed);
        assert!(
            count_after_expand > count_after_create,
            "expand should trigger interpretation: before={count_after_create} after={count_after_expand}"
        );
    });

    // ── Concurrent data + template mutation ─────────────────────────────
    //
    // Validates that setting data and expr simultaneously produces correct
    // final state via map_ref! coalescing.

    run("concurrent_data_and_expr_mutation", |cx| {
        let data = Arc::new(make_row("1", "Hello", "high"));
        let root_expr = expr("text", vec![positional_arg(col_ref("content"))]);

        let root = cx.new(|cx| ReactiveNode::new(root_expr, data, interp(), cx));
        cx.run_until_parked();

        let snap0 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert!(
            snap0.display.contains("Hello"),
            "initial: {}",
            snap0.display
        );

        // Set BOTH data and expr without run_until_parked between
        root.read_with(cx, |r, _| {
            r.data.set(Arc::new(make_row("1", "World", "low")));
        });
        root.update(cx, |r, cx| {
            r.apply_expr(expr("badge", vec![positional_arg(col_ref("priority"))]), cx);
        });

        // Now drain — map_ref! should coalesce to (new_data, new_expr)
        cx.run_until_parked();

        let snap1 = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert!(
            snap1.display.contains("[low]"),
            "after concurrent mutation: display should use new expr (badge) + new data (low). Got: {}",
            snap1.display
        );
    });

    run("rapid_data_mutations_no_stale_intermediate", |cx| {
        let data = Arc::new(make_row("1", "v0", "high"));
        let root_expr = expr("text", vec![positional_arg(col_ref("content"))]);

        let root = cx.new(|cx| ReactiveNode::new(root_expr, data, interp(), cx));
        cx.run_until_parked();

        // Fire 10 rapid data changes without draining
        for i in 1..=10 {
            root.read_with(cx, |r, _| {
                r.data
                    .set(Arc::new(make_row("1", &format!("v{i}"), "high")));
            });
        }

        // Drain all at once
        cx.run_until_parked();

        // Final snapshot should have the last value
        let snap = cx.read(|cx| root.read(cx).tree_snapshot(cx));
        assert!(
            snap.display.contains("v10"),
            "after rapid mutations, display should show final value. Got: {}",
            snap.display
        );

        // The display Mutable should also reflect the final value
        let display = root.read_with(cx, |r, _| r.display.get_cloned());
        assert!(
            display.contains("v10"),
            "display Mutable should be v10, got: {display}"
        );
    });

    eprintln!("\nall tests passed\n");
}
