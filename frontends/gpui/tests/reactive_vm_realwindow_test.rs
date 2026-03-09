//! Real-window test for reactive ViewModel rendering.
//!
//! Uses `Application::run()` on the main thread (real macOS event loop),
//! NOT `TestAppContext::run_until_parked()` which masks frame-timing issues.
//!
//! A background thread mutates data via `Mutable::set()` (thread-safe) and
//! verifies that the GPUI event loop fires the signal, re-renders children,
//! and updates their `display` Mutable — all through natural frame cycles.
//!
//! Run: cargo test -p holon-gpui --test reactive_vm_realwindow_test

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_signals::signal::Mutable;
use gpui::prelude::*;
use gpui::*;

use holon_gpui::reactive_vm_poc::*;

struct Handles {
    data_m: Mutable<Arc<DataRow>>,
    child_render_counts: Vec<Arc<AtomicUsize>>,
    child_displays: Vec<Mutable<String>>,
}

unsafe impl Send for Handles {}

struct OpaqueWrapper {
    root: Entity<ReactiveNode>,
}
impl Render for OpaqueWrapper {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.root.clone())
    }
}

fn main() {
    eprintln!("\nrunning reactive_vm_realwindow_test\n");

    let (handles_tx, handles_rx) = sync_channel::<Handles>(1);
    let (quit_tx, quit_rx) = sync_channel::<()>(1);

    let test_thread = std::thread::spawn(move || {
        eprint!("  waiting for GPUI window ... ");
        let h = handles_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("timed out waiting for GPUI window to create entities");
        eprintln!("ok");

        // Wait for initial render (poll render_count until > 0)
        eprint!("  waiting for initial render ... ");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if h.child_render_counts
                .iter()
                .all(|c| c.load(Ordering::Relaxed) >= 1)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for initial render"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!("ok");

        let before: Vec<usize> = h
            .child_render_counts
            .iter()
            .map(|c| c.load(Ordering::Relaxed))
            .collect();

        // Change data via Mutable (thread-safe, no GPUI context needed)
        eprint!("  changing data via Mutable::set ... ");
        h.data_m
            .set(Arc::new(make_row("demo", "RealWindow Updated!", "low")));
        eprintln!("ok");

        // Wait for signal → display update → cx.notify() → render
        // In the real event loop this goes through natural frame cycles,
        // NOT run_until_parked which drains everything synchronously.
        eprint!("  waiting for signal + re-render ... ");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let all_re_rendered = h
                .child_render_counts
                .iter()
                .zip(before.iter())
                .all(|(c, b)| c.load(Ordering::Relaxed) > *b);
            if all_re_rendered {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for re-render after data change — \
                 signal→cx.notify()→render path broken in real event loop. \
                 Before: {before:?}, Current: {:?}",
                h.child_render_counts
                    .iter()
                    .map(|c| c.load(Ordering::Relaxed))
                    .collect::<Vec<_>>()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!("ok");

        // Verify display Mutables have correct value (signal fired and updated them)
        eprint!("  verifying display Mutable values ... ");
        let d0 = h.child_displays[0].get_cloned();
        assert!(
            d0.contains("RealWindow Updated!"),
            "child[0] display not updated by signal: '{d0}'"
        );
        let d1 = h.child_displays[1].get_cloned();
        assert!(
            d1.contains("low"),
            "child[1] display not updated by signal: '{d1}'"
        );
        eprintln!("ok");

        eprintln!("\n  all real-window assertions passed\n");
        let _ = quit_tx.send(());
    });

    let app = Application::with_platform(gpui_platform::current_platform(false));
    app.run(move |cx| {
        let data = Arc::new(make_row("demo", "Hello World", "high"));
        let root_expr = expr(
            "row",
            vec![
                positional_arg(expr("text", vec![positional_arg(col_ref("content"))])),
                positional_arg(expr("badge", vec![positional_arg(col_ref("priority"))])),
            ],
        );

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(100.0), px(100.0)),
                    size: size(px(400.0), px(200.0)),
                })),
                ..Default::default()
            },
            |_, cx| {
                let root =
                    cx.new(|cx| ReactiveNode::new(root_expr, data, default_interpreter(), cx));

                let child_render_counts: Vec<Arc<AtomicUsize>> = root
                    .read(cx)
                    .children
                    .iter()
                    .map(|c| c.read(cx).render_count.clone())
                    .collect();
                let child_displays: Vec<Mutable<String>> = root
                    .read(cx)
                    .children
                    .iter()
                    .map(|c| c.read(cx).display.clone())
                    .collect();

                handles_tx
                    .send(Handles {
                        data_m: root.read(cx).data.clone(),
                        child_render_counts,
                        child_displays,
                    })
                    .expect("failed to send handles");

                cx.new(|_cx| OpaqueWrapper { root })
            },
        )
        .expect("failed to open window");

        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                if quit_rx.try_recv().is_ok() {
                    let _ = cx.update(|cx| cx.quit());
                    break;
                }
            }
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });

    test_thread.join().expect("test thread panicked");
}
