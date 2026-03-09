//! Experiment: Can tokio channels be awaited from GPUI's executor?
//!
//! Run: cargo test -p holon-gpui --test executor_bridge_test

use std::pin::pin;
use std::time::Duration;

use gpui::Application;

fn main() {
    let app = Application::with_platform(gpui_platform::current_platform(false));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let tokio_handle = rt.handle().clone();

    app.run(move |cx| {
        // Run all experiments sequentially in one cx.spawn so they don't block each other
        cx.spawn(async move |_cx| {
            let mut results = Vec::new();

            // === H1: Direct tokio oneshot await from GPUI executor ===
            {
                let (tx, rx) = tokio::sync::oneshot::channel::<String>();

                tokio_handle.spawn(async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    tx.send("h1_ok".to_string()).ok();
                });

                match rx.await {
                    Ok(val) => {
                        eprintln!("[H1] Direct oneshot await from GPUI: SUCCESS ({})", val);
                        results.push(format!("H1: OK ({})", val));
                    }
                    Err(e) => {
                        eprintln!("[H1] Direct oneshot await from GPUI: FAILED ({})", e);
                        results.push(format!("H1: FAILED ({})", e));
                    }
                }
            }

            // === H2: Spawn on tokio, await JoinHandle from GPUI ===
            {
                let join_handle = tokio_handle.spawn(async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    "h2_ok".to_string()
                });

                match join_handle.await {
                    Ok(val) => {
                        eprintln!("[H2] Spawn+await JoinHandle from GPUI: SUCCESS ({})", val);
                        results.push(format!("H2: OK ({})", val));
                    }
                    Err(e) => {
                        eprintln!("[H2] Spawn+await JoinHandle from GPUI: FAILED ({})", e);
                        results.push(format!("H2: FAILED ({})", e));
                    }
                }
            }

            // === H3: futures-signals map_future with tokio bridge ===
            {
                use futures_signals::signal::{Mutable, SignalExt};

                let filter = Mutable::new("test_query".to_string());
                let handle = tokio_handle.clone();

                let signal = filter.signal_cloned().map_future(move |f| {
                    let handle = handle.clone();
                    async move {
                        let join = handle.spawn(async move {
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            format!("results_for_{}", f)
                        });
                        join.await.unwrap_or_else(|e| format!("error: {}", e))
                    }
                });

                // map_future returns Signal<Option<T>>: None while pending, Some when done
                use futures::stream::StreamExt;
                let mut stream = pin!(signal.to_stream());
                let mut found = false;
                while let Some(val) = stream.next().await {
                    if let Some(v) = val {
                        eprintln!("[H3] map_future + tokio bridge: SUCCESS ({})", v);
                        results.push(format!("H3: OK ({})", v));
                        found = true;
                        break;
                    }
                }
                if !found {
                    eprintln!(
                        "[H3] map_future + tokio bridge: FAILED (stream ended without value)"
                    );
                    results.push("H3: FAILED".to_string());
                }
            }

            // === Summary ===
            eprintln!("\n=== RESULTS ===");
            for line in &results {
                eprintln!("  {}", line);
            }
            let all_ok = results.len() == 3 && results.iter().all(|s| s.contains("OK"));
            eprintln!("ALL PASSED: {}", all_ok);
            eprintln!("===============\n");

            std::process::exit(if all_ok { 0 } else { 1 });
        })
        .detach();
    });
}
