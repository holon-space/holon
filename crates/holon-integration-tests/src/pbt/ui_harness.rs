//! Frontend-neutral scaffolding for PBT harnesses
//! (`gpui_ui_pbt`, `tui_ui_pbt`, …).
//!
//! These helpers orchestrate the cross-thread / cross-runtime dance that
//! surrounds [`run_pbt_with_driver_sync_callback`](super::phased::run_pbt_with_driver_sync_callback):
//!
//! - bumping `PBT_MEMORY_MULTIPLIER` to budget for an extra frontend
//! - building a per-frontend `target/pbt-screenshots/<subdir>` path
//! - optionally booting an embedded MCP server for live inspection
//! - polling the frontend's `GeometryProvider` until real content lands
//! - watching the PBT thread and signalling quit when it finishes
//!
//! The actual frontend event loop and the `Box<dyn UserDriver>`
//! construction stay in each frontend's PBT test file — those parts are
//! genuinely platform-specific.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};

/// Default `PBT_MEMORY_MULTIPLIER` for a frontend PBT.
///
/// A live frontend (GPUI window or TUI render task) adds ~40 MB of
/// transient RSS per transition for entity caches and render trees on
/// top of the headless baseline; 15× keeps the memory invariants well
/// above what we observe in practice.
pub const DEFAULT_FRONTEND_MEMORY_MULTIPLIER: &str = "15";

/// Set `PBT_MEMORY_MULTIPLIER` to `default` if it isn't already set.
///
/// Call this before the PBT thread spawns so all child invariants see
/// the same budget.
pub fn set_memory_multiplier_if_unset(default: &str) {
    if std::env::var("PBT_MEMORY_MULTIPLIER").is_err() {
        // SAFETY: called before any other thread reads or writes env vars
        // (frontend PBTs invoke this from the synchronous prologue of `main`,
        // before the PBT thread or runtime spawn).
        unsafe {
            std::env::set_var("PBT_MEMORY_MULTIPLIER", default);
        }
    }
}

/// Pin the primary's Loro peer_id (`HOLON_LORO_PEER_ID=1`) if it isn't
/// already set, so the PBT reference model's `loro_merge_text`
/// (`crates/holon-integration-tests/src/pbt/state_machine.rs:58`, which
/// hardcodes peer_a=1, peer_b=2 when simulating the merge) predicts the
/// same RGA tiebreak ordering production produces.
///
/// Production's `LoroDocument::new` defaults to `rand::random::<u64>()`
/// for global P2P uniqueness; this override is test-only. Loro RGA
/// tiebreaks concurrent inserts by lower peer_id, so a large random
/// primary id can flip merge order relative to the reference's
/// peer_a=1 < peer_b=2 (= peer_idx+100 in the SUT) assumption — see
/// `crates/holon/examples/loro_text_update_vs_delete_insert.rs` for
/// the worked-through repro.
///
/// Call before the PBT thread spawns; the env var is read by
/// `LoroDocument::new` / `load_from_file` at primary doc creation
/// time.
pub fn set_loro_peer_id_if_unset(default: &str) {
    if std::env::var("HOLON_LORO_PEER_ID").is_err() {
        // SAFETY: same prologue invariant as `set_memory_multiplier_if_unset`
        // — called before any other thread is alive.
        unsafe {
            std::env::set_var("HOLON_LORO_PEER_ID", default);
        }
    }
}

/// Enable `PBT_ATOMIC_EDITOR=1` if it isn't already set. The atomic
/// editor primitives (`FocusEditableText`, `TypeChars`,
/// `DeleteBackward`, `MoveCursor`, `PressKey`) are the
/// keyboard-driven UI path; with this on, the bypass-style
/// `EditViaViewModel` / `EditViaDisplayTree` transitions are gated
/// off. See `frontends/tui/TODO.md` items A6 and the May 2026
/// "atomic editor PBT primitives" memory note.
///
/// Call before the PBT thread spawns; the flag is read by
/// `ReferenceState::atomic_editor_enabled()` at strategy-construction
/// time.
pub fn enable_atomic_editor_if_unset() {
    if std::env::var("PBT_ATOMIC_EDITOR").is_err() {
        // SAFETY: same prologue invariant as the helpers above.
        unsafe {
            std::env::set_var("PBT_ATOMIC_EDITOR", "1");
        }
    }
}

/// Print the per-transition rejection histogram on every panic. Without this
/// the `proptest_state_machine!` path swallows the `print_rejection_histogram()`
/// hook that the phased-PBT path runs in `pbt_teardown`, so the histogram is
/// never surfaced when a headless case fails. Idempotent — installs once per
/// process. Shared by the native-mode slices (`proptest_config:`); the
/// cases-based macro arm wires its own.
pub fn install_rejection_histogram_panic_hook() {
    use std::sync::Once;
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            crate::pbt::validation::print_rejection_histogram();
            prev(info);
        }));
    });
}

/// The standard `ProptestConfig` for a native-mode (`proptest_config:`) slice:
/// activates the atomic editor, installs the rejection-histogram panic hook, and
/// pins the failure-persistence file to `tests/<slice_name>.proptest-regressions`
/// (the default `SourceParallel` mode mislocates `lib.rs`/`main.rs` for
/// macro-generated tests and warns). `cases` defaults to 8 — `PROPTEST_CASES`
/// overrides at runtime; `PROPTEST_MAX_SHRINK_ITERS` overrides the shrink budget
/// (default 50). Slices sharing one state machine pass the *same* `slice_name`
/// to share one regressions file (e.g. `general_e2e_pbt` + its `_sql_only` peer).
pub fn standard_pbt_config(slice_name: &str) -> proptest::test_runner::Config {
    use proptest::test_runner::{Config, FileFailurePersistence};

    enable_atomic_editor_if_unset();
    install_rejection_histogram_panic_hook();
    let max_shrink = std::env::var("PROPTEST_MAX_SHRINK_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let regressions = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(format!("{slice_name}.proptest-regressions"));
    Config {
        cases: 8,
        max_shrink_iters: max_shrink,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            // proptest wants a 'static str, so leak the resolved path.
            Box::leak(regressions.to_string_lossy().into_owned().into_boxed_str()),
        ))),
        ..Config::default()
    }
}

/// Build `<cwd>/target/pbt-screenshots/<subdir>`.
///
/// The directory itself is created lazily by `GeometryDriver::with_screenshots`,
/// not by this helper.
pub fn screenshot_dir(subdir: &str) -> PathBuf {
    std::env::current_dir()
        .expect("current_dir failed")
        .join("target")
        .join("pbt-screenshots")
        .join(subdir)
}

/// Boot the embedded MCP server on `env_var`'s port if the env var is set.
///
/// Mirrors the inline block at the top of each frontend PBT test:
/// reads the port, enters the runtime, calls
/// `holon_mcp::di::start_embedded_mcp_server`, and sleeps two seconds so
/// the listener is bound before the renderer takes the main thread.
///
/// `label` is used for the eprintln messages so logs disambiguate
/// between simultaneous PBTs.
pub fn try_start_embedded_mcp(
    runtime: &tokio::runtime::Handle,
    engine: &Arc<holon::api::BackendEngine>,
    reactive_engine: &Arc<ReactiveEngine>,
    debug: Arc<holon_mcp::server::DebugServices>,
    env_var: &str,
    label: &str,
) {
    // PBT_PAUSE_SECONDS forces the MCP server up on a known port
    // (MCP_PAUSE_PORT) so external tools can attach during a panic
    // pause — overrides whatever `env_var` is configured to.
    let port: Option<u16> = if crate::debug_pause::pause_enabled() {
        Some(crate::debug_pause::MCP_PAUSE_PORT)
    } else {
        std::env::var(env_var).ok().map(|s| {
            s.parse()
                .unwrap_or_else(|e| panic!("{env_var} must be a u16: {e}"))
        })
    };
    let Some(port) = port else {
        return;
    };
    let engine = Some(engine.clone());
    let services: Arc<dyn BuilderServices> = reactive_engine.clone();
    let _guard = runtime.enter();
    holon_mcp::di::start_embedded_mcp_server_with_debug(engine, Some(services), port, debug);
    eprintln!("[{label}] MCP server starting on port {port}");
    std::thread::sleep(Duration::from_secs(2));
    eprintln!("[{label}] MCP server should be ready on port {port}");
}

/// Block (polling every 500 ms) until `geometry` reports an element with
/// both `has_content` and `entity_id`, or until `timeout` elapses.
///
/// This is the standard "frontend has rendered something the test can
/// interact with" gate. Both GPUI and TUI use the same predicate
/// (`has_content && entity_id.is_some()`) — placeholders that the
/// auto-`tag()` pipeline registers don't carry an entity_id, so this
/// only fires once `tracked()`/`render_entity()` has populated a real
/// row.
///
/// On timeout, dumps a `widget_type` histogram + a sample of entity
/// ids so the test failure includes enough context to diagnose why the
/// frontend never reached "real content".
pub fn wait_for_geometry_ready(
    geometry: &Arc<dyn GeometryProvider>,
    timeout: Duration,
    label: &str,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let elements = geometry.all_elements();
        let has_real_content = elements
            .iter()
            .any(|(_, info)| info.has_content && info.entity_id.is_some());
        if has_real_content {
            let n_content = elements.iter().filter(|(_, i)| i.has_content).count();
            eprintln!(
                "[{label}] Window ready: {} elements ({} with has_content)",
                elements.len(),
                n_content,
            );
            return true;
        }
        if Instant::now() > deadline {
            let mut hist: BTreeMap<String, usize> = BTreeMap::new();
            for (_, info) in &elements {
                *hist.entry(info.widget_type.to_string()).or_default() += 1;
            }
            let sample_ids: Vec<String> = elements
                .iter()
                .filter_map(|(_, i)| i.entity_id.as_deref().map(str::to_string))
                .take(5)
                .collect();
            eprintln!(
                "[{label}] Window ready timeout — {} elements, widget_type hist={:?}, \
                 has_content count={}, sample entity_ids={:?}",
                elements.len(),
                hist,
                elements.iter().filter(|(_, i)| i.has_content).count(),
                sample_ids,
            );
            return false;
        }
    }
}

/// Spawn a watcher thread that joins `pbt_handle` once it finishes,
/// signalling quit through the returned channel (unless `keep_env` is
/// set, in which case the frontend window stays open for inspection).
///
/// The frontend's event loop is responsible for polling `quit_rx` and
/// shutting down. `pbt_failed` is set if the PBT thread panicked.
pub fn spawn_quit_on_pbt_finish(
    pbt_handle: JoinHandle<()>,
    keep_env: &str,
    label: &str,
) -> (Arc<AtomicBool>, Receiver<()>) {
    let pbt_failed = Arc::new(AtomicBool::new(false));
    let pbt_failed_for_watcher = pbt_failed.clone();
    let keep_window = std::env::var(keep_env).is_ok();
    let (quit_tx, quit_rx) = sync_channel::<()>(1);
    let label = label.to_string();
    let keep_env = keep_env.to_string();

    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(500));
            if pbt_handle.is_finished() {
                let thread_result = pbt_handle.join();
                if thread_result.is_err() {
                    pbt_failed_for_watcher.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                if keep_window {
                    eprintln!("[{label}] PBT finished, keeping window open ({keep_env})");
                } else {
                    if let Err(e) = &thread_result {
                        eprintln!("[{label}] PBT thread panicked: {e:?}");
                    }
                    let _ = quit_tx.send(());
                }
                break;
            }
        }
    });

    (pbt_failed, quit_rx)
}
