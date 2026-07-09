//! Mobile entry points for iOS and Android.
//!
//! These are activated only with `--features mobile` and compile only on their
//! respective target OS.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::*;
use holon_frontend::{FrontendSession, HolonConfig, SessionConfig};

use crate::geometry::BoundsRegistry;

fn open_holon_window(cx: &mut App, db_path: Option<PathBuf>, orgmode_root: Option<PathBuf>) {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    let rt_handle = rt.handle().clone();

    let (session, engine, debug, app) = rt.block_on(async {
        let widgets = crate::render_supported_widgets();
        let ui_info = holon_api::UiInfo {
            available_widgets: widgets,
            screen_size: None,
        };
        let mut holon_config = HolonConfig {
            db_path,
            vault: holon_frontend::config::VaultConfig { root: orgmode_root },
            ..Default::default()
        };
        // Mobile builds don't read `~/.config/holon/holon.toml`, so the
        // desktop-style opt-in (`[crdt] enabled = true`) doesn't apply here.
        // Without this, `LoroModule` is never configured → `Arc<LoroShareBackend>`
        // is never registered → share/accept ops fail with
        // "No provider registered for entity: tree". Mobile is a first-class
        // target for sharing, so enable the CRDT substrate unconditionally.
        holon_config.crdt.enabled = Some(true);
        let config_dir = holon_frontend::config::resolve_config_dir(None);
        let session_config = SessionConfig::new(ui_info);

        // Bootstrap through `GpuiModule` (same path as desktop `main.rs`)
        // instead of `holon_app::new_from_config`. This starts the embedded
        // MCP server in `GpuiModule::on_start`, making mobile a first-class
        // debuggable / automatable target. The iOS simulator shares the
        // host's loopback, so the MCP HTTP server (default `:8520`, override
        // with `MCP_SERVER_PORT`) is reachable from the host — set a distinct
        // port when a desktop Holon already holds 8520.
        let mut app = fluxdi::Application::new(crate::di::GpuiModule {
            holon_config,
            session_config,
            config_dir,
            locked_keys: std::collections::HashSet::new(),
        });
        app.bootstrap().await.expect("GpuiModule bootstrap failed");

        let injector = app.injector();
        let session = injector.resolve::<FrontendSession>();
        let engine = injector.resolve::<holon_frontend::reactive::ReactiveEngine>();
        let debug = injector.resolve::<holon_mcp::server::DebugServices>();

        // Populate the reset-safe `live_debug` cell so the debug PBT tools
        // (`await_quiescence`, `debug_pbt_snapshot`) observe the boot session's
        // convergence/mirror handles. These are `root_async` factories — awaited
        // here so they are live (not raced) before any tool call; a later
        // `reset_vault` swaps this cell for the fresh session's handles.
        {
            let loro_sync_handle = injector
                .try_resolve_async::<holon::sync::LoroSyncControllerHandle>()
                .await
                .ok();
            // `BlockQuerySource` is not a DI key — the FrontendSession factory
            // builds it inline; the session accessor is the only handle.
            let block_query_source = Some(session.block_query().clone());
            let org_idle_signal = injector
                .try_resolve::<holon_orgmode::OrgSyncIdleSignal>()
                .ok();
            let loro_doc_store = injector
                .try_resolve::<holon::sync::LoroBlockOperations>()
                .ok()
                .map(|ops| ops.shared_doc_store());
            *debug.live_debug.write().expect("live_debug cell poisoned") =
                holon_mcp::server::DebugHandlesCell {
                    loro_sync_handle,
                    org_idle_signal,
                    block_query_source,
                    loro_doc_store,
                    reactive_engine: Some(engine.clone()),
                };
        }

        (session, engine, debug, app)
    });

    // Keep the runtime AND the DI application alive for the process lifetime:
    // the spawned MCP server task and background factories hold services
    // resolved from `app`'s injector, and the `NavigationState` built below
    // shares `debug.input_router` so MCP-injected input reaches the window.
    std::thread::spawn(move || {
        let _app = app;
        rt.block_on(std::future::pending::<()>());
    });

    // Phase 1 Option A: open a REBINDABLE window so a per-case `reset_vault`
    // MCP call can swap the engine+session in place (keeping this one window,
    // one MCP server). Mirror `launch_holon_window_with_engine`'s nav wiring so
    // MCP-injected input still reaches the window.
    let mut nav =
        crate::navigation_state::NavigationState::with_input_router(debug.input_router.clone());
    nav.set_navigation_debug(debug.navigation_state.clone());
    let bounds_registry = BoundsRegistry::new();
    let handle = crate::launch_holon_window_rebindable(
        session,
        engine,
        rt_handle,
        nav,
        bounds_registry,
        Some(debug.clone()),
        "Holon",
        cx,
    )
    .expect("rebindable Holon window failed to open");

    // Install the gpui-side reset builder so the (tokio) `reset_vault` tool can
    // boot a fresh SUT without a second MCP server. It runs on whatever runtime
    // the tool awaits it on — the MCP server runs on `rt`, so this lands there.
    let reset_builder: holon_mcp::server::ResetBuilderFn = Arc::new(|files| {
        Box::pin(crate::reset::build_fresh_sut_from_files(files))
            as futures::future::BoxFuture<
                'static,
                anyhow::Result<holon_mcp::server::ResetBuildOutput>,
            >
    });
    debug.reset_builder.set(reset_builder).ok();

    // Main-thread reset pump: owns the `!Send` `RebindHandle` and re-points the
    // live window when a `ResetRequest` arrives. Mirrors `setup_interaction_pump`.
    let (reset_tx, mut reset_rx) =
        futures::channel::mpsc::channel::<holon_mcp::server::ResetRequest>(4);
    debug.reset_tx.set(reset_tx).ok();
    cx.spawn(async move |cx| {
        use futures::StreamExt;
        while let Some(req) = reset_rx.next().await {
            let holon_mcp::server::ResetRequest {
                session,
                engine,
                ack,
            } = req;
            // `AsyncApp::update` is infallible on the gpui-mobile fork (returns
            // the closure result directly), so the rebind always runs on the
            // main thread here; report success once it has.
            cx.update(|cx| handle.rebind(session, engine, cx));
            ack.send(Ok(())).ok();
        }
    })
    .detach();
}

// ─── iOS ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "ios")]
const DEFAULT_INDEX_ORG: &str = include_str!("../../../assets/default/index.org");
#[cfg(target_os = "ios")]
const DEFAULT_JOURNALS_ORG: &str = include_str!("../../../assets/default/Journals.org");

#[cfg(target_os = "ios")]
fn ios_data_paths() -> (Option<PathBuf>, Option<PathBuf>) {
    // On iOS the app sandbox exposes a writable home directory; HOME points
    // at `…/data/Containers/Data/Application/<UUID>`. Put the DB inside
    // Library/ (not backed up to the cloud but persistent) and the org-mode
    // working copy inside Documents/ so the user sees it from the Files app.
    let home = std::env::var("HOME").ok().map(PathBuf::from);
    let db_path = home.as_ref().map(|h| h.join("Library").join("holon.db"));
    let orgmode_root = home.as_ref().map(|h| h.join("Documents").join("holon-pkm"));
    if let Some(db) = db_path.as_ref() {
        if let Some(parent) = db.parent() {
            std::fs::create_dir_all(parent).expect("create Library dir for holon.db");
        }
    }
    if let Some(org) = orgmode_root.as_ref() {
        std::fs::create_dir_all(org).expect("create orgmode root dir");
        let is_empty = std::fs::read_dir(org)
            .expect("read orgmode root dir")
            .next()
            .is_none();
        if is_empty {
            let seed = org.join("index.org");
            std::fs::write(&seed, DEFAULT_INDEX_ORG).expect("write seed index.org");
            eprintln!("GPUI iOS: seeded {}", seed.display());
        }
        // Seed notes.org whenever it doesn't exist — independent of is_empty so
        // existing installs that only have index.org also get a visible document.
        // "index.org" is filtered from the sidebar (name == "index"), so without
        // this file the sidebar is always empty on a fresh install.
        let journals_path = org.join("Journals.org");
        if !journals_path.exists() {
            std::fs::write(&journals_path, DEFAULT_JOURNALS_ORG).expect("write seed Journals.org");
            eprintln!("GPUI iOS: seeded {}", journals_path.display());
        }
    }
    (db_path, orgmode_root)
}

#[cfg(target_os = "ios")]
#[no_mangle]
pub extern "C" fn gpui_ios_register_app() {
    // Route `tracing` to stderr. iOS installed no subscriber, so every
    // `tracing::{error,warn,info,debug}!` — including the `dispatch_intent_chain`
    // failure logs that explain why an operation didn't commit — was silently
    // dropped, leaving the app undebuggable on device (a fail-loud violation).
    // Captured via `xcrun simctl launch --console-pty`. `RUST_LOG` overrides the
    // default `info` filter (pass through `SIMCTL_CHILD_RUST_LOG`).
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    std::panic::set_hook(Box::new(|info| {
        eprintln!("GPUI PANIC: {info}");
    }));

    gpui_mobile::ios::ffi::set_app_callback(Box::new(|cx: &mut App| {
        let (db_path, orgmode_root) = ios_data_paths();
        eprintln!("GPUI iOS: db_path={db_path:?} orgmode_root={orgmode_root:?}");
        open_holon_window(cx, db_path, orgmode_root);
    }));
}

#[cfg(target_os = "ios")]
pub fn ios_main() {
    gpui_ios_register_app();
    gpui_mobile::ios::ffi::run_app();
}

// ─── Android ─────────────────────────────────────────────────────────────

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: android_activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("holon-gpui"),
    );

    gpui_mobile::android::jni::install_panic_hook();
    log::info!("android_main: entered");

    let internal = app.internal_data_path();
    let external = app.external_data_path();
    log::info!("android_main: internal_data_path={internal:?}, external_data_path={external:?}");

    let db_path = internal.map(|p| p.join("holon.db"));
    let orgmode_root = external.map(|p| p.join("holon-pkm"));
    log::info!("android_main: db_path={db_path:?}, orgmode_root={orgmode_root:?}");

    let _platform = gpui_mobile::android::jni::init_platform(&app);
    log::info!("android_main: platform initialised");

    let shared = gpui_mobile::android::jni::shared_platform()
        .expect("shared_platform() returned None after init_platform");

    let gpui_app = Application::with_platform(std::rc::Rc::new(shared));
    gpui_app.run(|cx| {
        open_holon_window(cx, db_path, orgmode_root);
    });
}

pub fn safe_area_top_px() -> f32 {
    #[cfg(target_os = "android")]
    {
        let from_platform = gpui_mobile::android::jni::platform()
            .and_then(|p| p.primary_window())
            .map(|w| w.safe_area_insets_logical().top)
            .unwrap_or(0.0);
        return from_platform.max(32.0);
    }
    #[cfg(target_os = "ios")]
    {
        return gpui_mobile::safe_area_insets().0.max(20.0);
    }
    #[allow(unreachable_code)]
    0.0
}

pub fn safe_area_bottom_px() -> f32 {
    #[cfg(target_os = "android")]
    {
        return gpui_mobile::android::jni::platform()
            .and_then(|p| p.primary_window())
            .map(|w| w.safe_area_insets_logical().bottom)
            .unwrap_or(0.0);
    }
    #[cfg(target_os = "ios")]
    {
        let safe = gpui_mobile::safe_area_insets().1;
        let kb = gpui_mobile::keyboard_height();
        return safe.max(kb);
    }
    #[allow(unreachable_code)]
    0.0
}

// ─── Soft keyboard lifecycle ─────────────────────────────────────────────
//
// The platform keyboard must be up exactly while a text input owns focus.
// gpui delivers Blur/Focus in no guaranteed order on a block→block focus
// move (the zombie-editor blur can arrive AFTER the next editor's focus),
// so a naive hide-on-blur dismisses the keyboard mid-editing. Guard with a
// focus generation counter: every focus bumps it; a blur schedules a
// deferred hide that only fires if no focus arrived in the meantime.

use std::sync::atomic::{AtomicU64, Ordering};

static KEYBOARD_FOCUS_GENERATION: AtomicU64 = AtomicU64::new(0);

/// How long a scheduled hide waits for a successor focus before firing.
/// One frame is enough for the mount→grab pipeline; 150ms adds margin for
/// slow re-renders (variant switch re-mounts the editor) without a user-
/// perceivable keyboard flicker window.
const KEYBOARD_HIDE_GRACE: std::time::Duration = std::time::Duration::from_millis(150);

/// A text input gained focus: claim the next generation (cancelling any
/// pending deferred hide) and raise the platform soft keyboard. Returns the
/// generation this focus claimed; the editor stores it and passes it back to
/// [`editor_focus_lost`] so a *stale* editor's later blur cannot hide the
/// keyboard out from under whoever currently holds focus.
pub fn editor_focus_gained() -> u64 {
    let generation = KEYBOARD_FOCUS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    tracing::debug!(generation, "soft keyboard: show (editor focus)");
    platform_show_keyboard();
    generation
}

/// A text input lost focus: schedule a deferred hide keyed to the generation
/// that focus *claimed* (`my_generation`, from the matching
/// [`editor_focus_gained`]).
///
/// The bare generation counter only guards blur-BEFORE-focus (a successor's
/// focus bumps the counter, so a hide scheduled by the predecessor's earlier
/// blur is skipped). It does NOT guard blur-AFTER-focus: gpui delivers
/// Focus/Blur unordered on a block→block move (and on the iOS render-path the
/// unmounting editor's `is_focused=false` edge can be evaluated *after* the
/// successor's `true` edge in the same frame), so the stale editor's blur
/// reads the already-advanced counter and schedules a hide that nothing
/// cancels — the keyboard drops ~150ms after focus though a block is focused.
///
/// Fix: only the editor still holding the current generation may schedule a
/// hide. A stale editor (`my_generation != current`) has already been
/// superseded by a later focus and its blur is ignored.
pub fn editor_focus_lost(cx: &mut App, my_generation: u64) {
    if KEYBOARD_FOCUS_GENERATION.load(Ordering::SeqCst) != my_generation {
        tracing::debug!(
            my_generation,
            "soft keyboard: hide skipped (stale editor blur; focus already moved on)"
        );
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(KEYBOARD_HIDE_GRACE).await;
        if KEYBOARD_FOCUS_GENERATION.load(Ordering::SeqCst) == my_generation {
            tracing::debug!("soft keyboard: hide (editor blur, no refocus)");
            platform_hide_keyboard();
        } else {
            tracing::debug!("soft keyboard: hide skipped (focus moved to another input)");
        }
    })
    .detach();
}

fn platform_show_keyboard() {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    gpui_mobile::show_keyboard();
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    tracing::warn!(
        "soft keyboard show requested but this platform has no soft-keyboard backend \
         (mobile feature enabled on a desktop OS) — input continues via hardware keyboard"
    );
}

fn platform_hide_keyboard() {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    gpui_mobile::hide_keyboard();
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    tracing::warn!(
        "soft keyboard hide requested but this platform has no soft-keyboard backend \
         (mobile feature enabled on a desktop OS)"
    );
}
