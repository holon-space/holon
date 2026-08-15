//! Holon Dioxus Web — worker-bridge frontend.
//!
//! Architecture: `holon-frontend` + `BackendEngine` run inside a dedicated
//! `wasm32-wasip1-threads` Web Worker (the `holon-worker` crate). This
//! frontend receives serialized `ViewModel` snapshots via `postMessage` and
//! renders them as Dioxus elements. No holon crates are imported here —
//! the only coupling is the JSON wire format.

mod bridge;
mod dnd;
mod editor;
mod render;

use std::cell::RefCell;

use bridge::WorkerBridge;
use dioxus::prelude::*;
use holon_frontend::view_model::ViewModel;
use holon_frontend::view_model::WatchEnvelope;
use js_sys::Reflect;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::*;

/// The read-side block table name, mirroring `holon_turso::BLOCK_READ_TABLE`.
/// That const lives in the native-only `holon-turso` crate, which the wasm
/// frontend can't depend on; the worker (which owns the DB) uses the same
/// value, so this readiness-probe string must stay in sync with it.
const BLOCK_READ_TABLE: &str = "block";

/// URL of the worker entry module, relative to the serving root.
const WORKER_URL: &str = "/web/worker-entry.mjs";
/// OPFS-backed database file. The worker's OPFS bridge requires every file
/// Turso will open to be `registerFile`d ahead of `engineInit` (sync access
/// handles must be created in advance) — see the boot sequence below.
const DB_PATH: &str = "holon.db";

/// How long to wait for the first root-layout projection envelope before
/// declaring the boot failed (B3). Cold start incl. WASM instantiation + seed
/// is a few seconds; this is a generous ceiling, not a latency target.
const WATCH_READY_TIMEOUT_MS: u32 = 10_000;

// WorkerBridge wraps Rc<_> so it is !Send. We keep it alive in a thread-local
// so Dioxus signals (which require Send) never need to hold it directly.
thread_local! {
    static BRIDGE: RefCell<Option<WorkerBridge>> = const { RefCell::new(None) };
    /// Active MCP relay WebSocket. Replaced on reconnect; None when hub is down.
    static MCP_WS: RefCell<Option<web_sys::WebSocket>> = const { RefCell::new(None) };
}

fn main() {
    console_error_panic_hook::set_once();
    init_tracing();
    tracing::info!("[holon-dioxus-web] booting");
    dioxus::launch(App);
}

/// Install the wasm tracing layer at INFO by default (B4). dioxus-core emits a
/// `tracing::trace!("Marking task … as dirty")` on every scheduler tick — at
/// the 60Hz tick-pump cadence that floods the console and drowns real errors.
/// Gate the layer at INFO so those never reach the console; opt into verbose
/// with `?log=trace` (or `?log=debug`) in the page URL.
fn init_tracing() {
    let level = url_log_level().unwrap_or(tracing::Level::INFO);
    let config = tracing_wasm::WASMLayerConfigBuilder::new()
        .set_max_level(level)
        .build();
    tracing_wasm::set_as_global_default_with_config(config);
}

/// Read a `log=<level>` query parameter from the page URL, if present.
fn url_log_level() -> Option<tracing::Level> {
    let search = web_sys::window()?.location().search().ok()?; // ALLOW(ok): JsValue error has no Display; None = no override
    // `search` looks like "?log=trace&foo=bar"; scan for the log= pair.
    let raw = search.trim_start_matches('?');
    for pair in raw.split('&') {
        if let Some(val) = pair.strip_prefix("log=") {
            return match val.to_ascii_lowercase().as_str() {
                "trace" => Some(tracing::Level::TRACE),
                "debug" => Some(tracing::Level::DEBUG),
                "info" => Some(tracing::Level::INFO),
                "warn" => Some(tracing::Level::WARN),
                "error" => Some(tracing::Level::ERROR),
                _ => None,
            };
        }
    }
    None
}

#[derive(Clone, PartialEq)]
enum BootState {
    Booting,
    /// The root-layout projection actually rendered at least once (B3): "ready"
    /// is only ever shown once a real ViewModel envelope arrived, never over a
    /// failed/empty projection.
    Ready {
        cold_start_ms: u64,
    },
    Failed(String),
    /// The browser denies a precondition the worker needs. Distinct from
    /// `Failed` because no amount of clearing local data can grant it, so the
    /// card must not offer the reset remedy.
    PlatformUnsupported(String),
    /// Engine came up but `block:root-layout` is absent from the projection.
    /// With the IVM-reopen fix this should be unreachable, but if the local DB
    /// is genuinely corrupt we say so loudly and offer a recoverable reset (B2)
    /// rather than sitting on a green lie.
    NoRootLayout,
}

impl BootState {
    /// Stable slug for the `data-boot-state` attribute the web-arm driver
    /// awaits.
    fn marker(&self) -> &'static str {
        match self {
            BootState::Booting => "booting",
            BootState::Ready { .. } => "ready",
            BootState::Failed(_) => "failed",
            BootState::PlatformUnsupported(_) => "platform-unsupported",
            BootState::NoRootLayout => "no-root-layout",
        }
    }
}

/// `None` when the page can host the worker. Otherwise a user-facing reason
/// naming the missing precondition — the worker is `wasm32-wasip1-threads` and
/// needs `SharedArrayBuffer`, which the browser only exposes to a
/// cross-origin-isolated page.
fn missing_platform_precondition() -> Option<String> {
    // Read both off the global rather than via typed web-sys accessors: the
    // `crossOriginIsolated` binding needs a web-sys feature this crate does not
    // enable, and `SharedArrayBuffer` has no typed accessor at all.
    let global = js_sys::global();
    let prop = |name: &str| js_sys::Reflect::get(&global, &name.into()).ok(); // ALLOW(ok): a failed Reflect::get means the property is unreachable — exactly the "missing" condition being reported

    if prop("crossOriginIsolated").and_then(|v| v.as_bool()) != Some(true) {
        return Some(
            "This page is not cross-origin isolated, so the database engine cannot start. On \
             holon.space that isolation is installed by a service worker — if your browser blocks \
             service workers (private/incognito restrictions, an extension, or a site setting for \
             this domain), reload once with them allowed."
                .to_string(),
        );
    }
    if prop("SharedArrayBuffer").is_none_or(|v| v.is_undefined()) {
        return Some(
            "This browser does not expose SharedArrayBuffer, which the database engine requires. \
             It is usually disabled by a privacy setting or an enterprise policy."
                .to_string(),
        );
    }
    None
}

#[component]
fn App() -> Element {
    let mut boot_state = use_signal(|| BootState::Booting);
    let mut view_model: Signal<Option<ViewModel>> = use_signal(|| None);

    use_future(move || async move {
        let t0 = now_ms();

        // Preflight the two platform preconditions the worker cannot run
        // without. Both come from cross-origin isolation, which on a static
        // host is faked by the coi-serviceworker; if a browser blocks service
        // workers (policy, extension, or site setting) the worker fails deep
        // inside wasm instantiation with an opaque message. Name the real
        // cause here instead.
        if let Some(reason) = missing_platform_precondition() {
            boot_state.set(BootState::PlatformUnsupported(reason));
            return;
        }

        let bridge = match WorkerBridge::spawn(WORKER_URL).await {
            Ok(b) => b,
            Err(e) => {
                boot_state.set(BootState::Failed(format!("worker spawn: {e}")));
                return;
            }
        };

        // Publish the bridge to the thread-local immediately (B2): the
        // "Reset local data" action calls `engineResetStorage` through it, and
        // it must be reachable from EVERY subsequent failure state — not only
        // the ready path. Booting renders no interactive surface, so an
        // early-set bridge cannot be misused before the projection mounts.
        BRIDGE.with(|b| *b.borrow_mut() = Some(bridge.clone()));

        // Pre-register the OPFS files (db + WAL) so the worker's OPFS shim
        // can hand Turso sync access handles for them. On a page reload the
        // PREVIOUS worker's sync handles may not have been released yet
        // (worker teardown is async), which surfaces as
        // NoModificationAllowedError — retry with backoff instead of failing
        // the boot on a race the browser resolves itself moments later.
        'files: for file in [DB_PATH.to_string(), format!("{DB_PATH}-wal")] {
            const ATTEMPTS: u32 = 10;
            let mut last_err = String::new();
            for attempt in 0..ATTEMPTS {
                match bridge.call("registerFile", [file.clone().into()]).await {
                    Ok(_) => continue 'files,
                    Err(e) => {
                        last_err = format!("{e}");
                        tracing::warn!(
                            "[boot] registerFile {file} attempt {}/{ATTEMPTS} failed: {last_err}",
                            attempt + 1
                        );
                        gloo_timers::future::TimeoutFuture::new(300).await;
                    }
                }
            }
            boot_state.set(BootState::Failed(format!(
                "registerFile {file} after {ATTEMPTS} attempts: {last_err}"
            )));
            return;
        }

        if let Err(e) = bridge.call("engineInit", [DB_PATH.into()]).await {
            boot_state.set(BootState::Failed(format!("engineInit: {e}")));
            return;
        }

        // Connect the MCP relay bridge (best-effort; reconnects automatically on
        // close).
        connect_mcp_relay(bridge.clone(), 0);

        // Seed the viewport BEFORE the first watch so the root
        // `if_space(...)` picks the real breakpoint on the first paint,
        // then keep it live on window resize.
        send_viewport(&bridge).await;
        install_resize_listener(bridge.clone());

        // The root layout block has a well-known id set by seed_default_layout.
        let root_val = match bridge
            .call(
                "engineExecuteQuery",
                [format!(
                    // ALLOW(sql): startup readiness probe before BackendEngine is wired
                    "SELECT id FROM {BLOCK_READ_TABLE} WHERE id='block:root-layout' LIMIT 1"
                )
                .into()],
            )
            .await
        {
            Ok(v) => v,
            Err(e) => {
                boot_state.set(BootState::Failed(format!("root block query: {e}")));
                return;
            }
        };

        let root_id = match extract_first_id(&root_val) {
            Some(id) => id,
            None => {
                // Engine came up but the root-layout row is absent from the
                // projection. Do NOT show green "ready" over an empty page
                // (B3) — surface a loud, recoverable NoRootLayout state that
                // offers "Reset local data" (B2). BRIDGE was already published
                // right after spawn, so the reset action can reach the worker.
                boot_state.set(BootState::NoRootLayout);
                return;
            }
        };

        // Subscribe to ViewModel snapshots for the root block.
        let handle_val = match bridge
            .call("engineWatchView", [root_id.clone().into()])
            .await
        {
            Ok(v) => v,
            Err(e) => {
                boot_state.set(BootState::Failed(format!("engineWatchView: {e}")));
                return;
            }
        };

        // Worker's subscription counter starts at 1; 0 is a sentinel.
        // Fail loudly rather than binding a listener that can never fire.
        let handle = match handle_val.as_f64() {
            Some(h) if h >= 1.0 => h as u32,
            other => {
                boot_state.set(BootState::Failed(format!(
                    "engineWatchView returned bogus handle: {other:?}"
                )));
                return;
            }
        };

        // B3: "ready" is only truthful once a real projection actually
        // rendered. Flip BootState::Ready on the FIRST watch envelope, not
        // eagerly after subscribing — a subscription that never delivers
        // (failed projection) must not read as green.
        let ready_marked = std::rc::Rc::new(std::cell::Cell::new(false));
        let ready_on_snapshot = ready_marked.clone();
        bridge.on_snapshot(handle, move |json| {
            match serde_json::from_str::<WatchEnvelope>(&json) {
                Ok(env) => {
                    // Focus unchanged → preserve the local caret across the
                    // re-render (structural remounts blur the old element).
                    // Focus CHANGED → the worker's word wins (ADR 0010):
                    // worker_focus moves DOM focus instead, and no stale
                    // restore may snap it back.
                    let dom_focus = editor::cursor::save();
                    if env.focused_block == dom_focus.as_ref().map(|s| s.entity_id.clone()) {
                        if let Some(saved) = dom_focus {
                            editor::cursor::enqueue_restore(saved);
                        }
                    }
                    view_model.set(Some(env.view_model));
                    editor::worker_focus::apply(env.focused_block, env.caret_offset);
                    if !ready_on_snapshot.replace(true) {
                        let cold_start_ms = now_ms().saturating_sub(t0);
                        boot_state.set(BootState::Ready { cold_start_ms });
                    }
                }
                Err(e) => tracing::error!("[snapshot] deserialize failed: {e}"),
            }
        });

        // B3 watchdog: if no projection arrives, don't sit forever on
        // "booting…" (an equally dishonest not-ready). Fail loud with a
        // recoverable error after a generous grace period.
        let ready_watchdog = ready_marked.clone();
        wasm_bindgen_futures::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(WATCH_READY_TIMEOUT_MS).await;
            if !ready_watchdog.get() {
                boot_state.set(BootState::Failed(format!(
                    "root-layout watch produced no projection within {WATCH_READY_TIMEOUT_MS}ms — \
                     the projection failed to render"
                )));
            }
        });
    });

    // Continuous runtime pump. Without this, the worker's current-thread
    // runtime only advances during user-initiated RPCs, so file-watcher /
    // external / delayed events never reach the frontend. ~16ms cadence
    // matches 60fps; the tick itself awaits a 10ms sleep inside the
    // runtime so the cost is bounded.
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(16).await;
            let Some(bridge) = BRIDGE.with(|b| b.borrow().clone()) else {
                continue;
            };
            if let Err(e) = bridge.call("engineTick", [JsValue::from_f64(10.0)]).await {
                tracing::error!("[tick pump] engineTick failed: {e}");
                // Brief backoff on error so we don't hot-spin on a dead worker.
                gloo_timers::future::TimeoutFuture::new(250).await;
            }
        }
    });

    let s = boot_state.read().clone();
    let vm = view_model.read().clone();

    rsx! {
        div {
            style: "display: flex; flex-direction: column; height: 100vh; background: #121212; color: #e0e0e0; font-family: system-ui; overflow: hidden;",

            // ── Title bar ───────────────────────────────────────────────────
            div {
                style: "display: flex; align-items: center; gap: 8px; padding: 6px 12px; background: #1a1a2e; border-bottom: 1px solid #2a2a3a; flex-shrink: 0;",
                // Machine-readable boot state. The web-arm driver awaits this
                // instead of scraping the badge text, so a failed boot fails
                // the test loudly instead of timing out.
                "data-boot-state": "{s.marker()}",
                span { style: "font-weight: bold; color: #e0e0e0;", "Holon" }
                match &s {
                    BootState::Booting => rsx! {
                        span { style: "color: #888; font-size: 0.8em;", "booting…" }
                    },
                    BootState::Ready { cold_start_ms } => rsx! {
                        span {
                            style: "color: #7fdf7f; font-size: 0.75em;",
                            "ready ({cold_start_ms}ms)"
                        }
                    },
                    BootState::Failed(err) => rsx! {
                        span { style: "color: #ff5252; font-size: 0.8em;", "⚠ {err}" }
                    },
                    BootState::PlatformUnsupported(_) => rsx! {
                        span { style: "color: #ff5252; font-size: 0.8em;", "⚠ browser cannot host the engine" }
                    },
                    BootState::NoRootLayout => rsx! {
                        span { style: "color: #ffb020; font-size: 0.8em;", "⚠ local data corrupt" }
                    },
                }
            }

            // ── Main content ─────────────────────────────────────────────────
            div {
                style: "flex: 1; overflow: auto; padding: 12px;",
                match (&s, &vm) {
                    (BootState::Booting, _) => rsx! {
                        div {
                            style: "color: #888; font-style: italic; padding: 32px; text-align: center;",
                            "Starting backend…"
                        }
                    },
                    (BootState::Failed(err), _) => rsx! {
                        RecoveryCard {
                            accent: "#ff5252",
                            title: "Boot failed",
                            message: "The backend could not start. The error detail is below. \
                                      Resetting local data clears this browser's stored vault (OPFS) \
                                      and re-seeds a fresh one — try that if the failure looks like \
                                      corrupt local state.",
                            detail: Some(err.clone()),
                            reset: true,
                        }
                    },
                    (BootState::PlatformUnsupported(reason), _) => rsx! {
                        RecoveryCard {
                            accent: "#ff5252",
                            title: "This browser cannot run Holon",
                            message: "Holon's database engine needs a capability your browser is not \
                                      granting this page. Clearing local data cannot help — the \
                                      missing precondition is described below.",
                            detail: Some(reason.clone()),
                            reset: false,
                        }
                    },
                    (BootState::NoRootLayout, _) => rsx! {
                        RecoveryCard {
                            accent: "#ffb020",
                            title: "Local data unavailable",
                            message: "The engine started but the root layout is missing from the \
                                      projection — the database stored in this browser is corrupt or \
                                      incomplete. Reset local data to rebuild a fresh vault. This only \
                                      clears data stored in THIS browser (OPFS).",
                            detail: None,
                            reset: true,
                        }
                    },
                    (BootState::Ready { .. }, Some(vm)) => rsx! {
                        render::RenderNode { node: vm.clone() }
                    },
                    (BootState::Ready { .. }, None) => rsx! {
                        div {
                            style: "color: #888; font-style: italic; padding: 32px; text-align: center;",
                            "Rendering…"
                        }
                    },
                }
            }
        }
    }
}

/// Centered recovery card for the unrecoverable boot states (B2/B3): a muted
/// danger-palette card on the app's dark background, with a clear message, an
/// optional raw-error detail block, and the Reset action. Kept to simple inline
/// CSS; final palette harmonization happens at merge with the styling stream.
#[component]
fn RecoveryCard(
    accent: String,
    title: String,
    message: String,
    detail: Option<String>,
    /// False for failures a fresh vault cannot fix, where offering the reset
    /// would send the user down a remedy that provably does not apply.
    reset: bool,
) -> Element {
    rsx! {
        div {
            style: "height: 100%; display: flex; align-items: center; justify-content: center; padding: 24px;",
            div {
                style: format!(
                    "max-width: 460px; width: 100%; background: #1a1a2e; border: 1px solid {accent}; \
                     border-top: 3px solid {accent}; border-radius: 10px; padding: 28px 32px; \
                     box-shadow: 0 8px 32px rgba(0,0,0,0.45);"
                ),
                h2 {
                    style: format!("margin: 0 0 12px; color: {accent}; font-size: 1.15em;"),
                    "{title}"
                }
                p {
                    style: "margin: 0; color: #c0c0cc; line-height: 1.55; font-size: 0.92em;",
                    "{message}"
                }
                if let Some(detail) = detail {
                    pre {
                        style: "margin: 16px 0 0; padding: 10px 12px; background: #12121e; \
                                border-radius: 6px; color: #ff8a8a; font-size: 0.8em; \
                                white-space: pre-wrap; word-break: break-word; max-height: 180px; \
                                overflow: auto;",
                        "{detail}"
                    }
                }
                if reset {
                    ResetDataButton {}
                }
            }
        }
    }
}

/// "Reset local data" button (B2): clears this browser's OPFS vault and
/// reloads. Available in every unrecoverable boot state.
#[component]
fn ResetDataButton() -> Element {
    rsx! {
        button {
            style: "margin-top: 20px; padding: 9px 18px; background: #2a2a3a; color: #ff8a8a; \
                    border: 1px solid #ff5252; border-radius: 6px; cursor: pointer; font-size: 0.9em;",
            onclick: move |_| reset_local_data(),
            "Reset local data"
        }
    }
}

/// Clear all local (OPFS) data and reload (B2). The delete runs WORKER-side via
/// `engineResetStorage`: the worker tears the engine down, closes its Turso
/// OPFS sync-access handles, and removes the db/wal files — steps the page
/// cannot do while the worker holds those handles (they fail with
/// `NoModificationAllowedError`). Then reload for a clean re-seed.
fn reset_local_data() {
    wasm_bindgen_futures::spawn_local(async move {
        match BRIDGE.with(|b| b.borrow().clone()) {
            Some(bridge) => {
                if let Err(e) = bridge.call("engineResetStorage", [DB_PATH.into()]).await {
                    // Don't hide the failure, but still reload — the user asked
                    // to reset and a fresh boot is the recovery path.
                    tracing::error!("[reset] engineResetStorage failed: {e}");
                }
            }
            None => tracing::error!("[reset] no worker bridge available — reloading only"),
        }
        if let Some(win) = web_sys::window() {
            if let Err(e) = win.location().reload() {
                tracing::error!("[reset] page reload failed: {e:?}");
            }
        }
    });
}

/// Push the current window viewport (CSS px + devicePixelRatio) into the
/// worker's `UiState` so `if_space(...)` container queries evaluate against
/// real dimensions. Errors are loud: a failed viewport push means the
/// layout silently renders the desktop-first branch.
async fn send_viewport(bridge: &WorkerBridge) {
    let Some(win) = web_sys::window() else {
        tracing::error!("[viewport] no window object — viewport not sent");
        return;
    };
    let width = win.inner_width().ok().and_then(|v| v.as_f64()); // ALLOW(ok): JS reflection — non-numeric is handled below
    let height = win.inner_height().ok().and_then(|v| v.as_f64()); // ALLOW(ok): JS reflection — non-numeric is handled below
    let (Some(width), Some(height)) = (width, height) else {
        tracing::error!("[viewport] window.innerWidth/Height not numeric — viewport not sent");
        return;
    };
    let scale = win.device_pixel_ratio();
    if let Err(e) = bridge
        .call(
            "engineSetViewport",
            [width.into(), height.into(), scale.into()],
        )
        .await
    {
        tracing::error!("[viewport] engineSetViewport failed: {e}");
    }
}

/// Re-send the viewport on every window resize (leaked closure — lives for
/// the page lifetime, like the tick pump).
fn install_resize_listener(bridge: WorkerBridge) {
    let Some(win) = web_sys::window() else {
        tracing::error!("[viewport] no window object — resize listener not installed");
        return;
    };
    let closure: Closure<dyn Fn()> = Closure::wrap(Box::new(move || {
        let bridge = bridge.clone();
        wasm_bindgen_futures::spawn_local(async move {
            send_viewport(&bridge).await;
        });
    }));
    if let Err(e) = win.add_event_listener_with_callback("resize", closure.as_ref().unchecked_ref())
    {
        tracing::error!("[viewport] resize listener install failed: {e:?}");
    }
    closure.forget();
}

/// How many consecutive failed relay connects before giving up. A static host
/// (GitHub Pages) has no hub to reach, so an uncapped retry is a permanent 1 Hz
/// failure loop for the life of the page.
const MCP_RELAY_MAX_ATTEMPTS: u32 = 5;

/// Connect to the MCP relay hub as `role=browser`. All incoming tool calls
/// are forwarded to the worker via `engineMcpTool` and the results are sent
/// back.
///
/// An unreachable hub does NOT fail in `WebSocket::new` — construction only
/// rejects a malformed or scheme-forbidden URL. The failure arrives later as
/// `onclose` without a preceding `onopen`, so that is what counts a consecutive
/// attempt; after `MCP_RELAY_MAX_ATTEMPTS` of them the retry stops with a
/// disclosed warning. An `onclose` that DID open is a hub restart
/// (`trunk --watch`), so it reconnects with the count reset.
fn connect_mcp_relay(bridge: WorkerBridge, attempt: u32) {
    let location = web_sys::window().map(|w| w.location());
    let host = location
        .as_ref()
        .and_then(|l| l.host().ok()) // ALLOW(ok): web-sys JsValue error has no Display; default below
        .unwrap_or_else(|| "localhost:8765".to_string());
    // A ws:// socket from an https:// page is refused by the browser before any
    // request leaves, so the scheme must follow the page's.
    let scheme = match location.as_ref().and_then(|l| l.protocol().ok()) // ALLOW(ok): web-sys JsValue error has no Display; ws default below
    {
        Some(p) if p == "https:" => "wss",
        _ => "ws",
    };
    let url = format!("{scheme}://{host}/mcp-hub?role=browser");

    let ws = match web_sys::WebSocket::new(&url) {
        Ok(ws) => ws,
        Err(e) => {
            if attempt + 1 >= MCP_RELAY_MAX_ATTEMPTS {
                tracing::warn!(
                    "[mcp-relay] connect to {url} failed {} times ({e:?}) — giving up; MCP \
                     tooling is unavailable for this page",
                    attempt + 1
                );
                return;
            }
            tracing::warn!("[mcp-relay] connect to {url} failed: {e:?} — will retry in 1s");
            let bridge_clone = bridge.clone();
            wasm_bindgen_futures::spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(1000).await;
                connect_mcp_relay(bridge_clone, attempt + 1);
            });
            return;
        }
    };

    MCP_WS.with(|slot| *slot.borrow_mut() = Some(ws.clone()));
    tracing::debug!("[mcp-relay] connecting to {url}");

    // Distinguishes "the hub was never there" from "the hub restarted": only
    // onclose WITHOUT a preceding onopen counts toward the attempt cap.
    let opened = std::rc::Rc::new(std::cell::Cell::new(false));
    let opened_on_open = opened.clone();
    let onopen: Closure<dyn Fn(web_sys::Event)> =
        Closure::wrap(Box::new(move |_: web_sys::Event| {
            opened_on_open.set(true);
        }));
    ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    onopen.forget();

    // onmessage: receive tool call requests from the native relay.
    let bridge_msg = bridge.clone();
    let ws_msg = ws.clone();
    let onmessage: Closure<dyn Fn(web_sys::MessageEvent)> =
        Closure::wrap(Box::new(move |e: web_sys::MessageEvent| {
            let data = match e.data().as_string() {
                Some(s) => s,
                None => return,
            };
            let msg: serde_json::Value = match serde_json::from_str(&data) {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("[mcp-relay] parse error: {e}");
                    return;
                }
            };
            let id = match msg.get("id").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return,
            };
            let tool = match msg.get("tool").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return,
            };
            let arguments = msg
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            let args_json = serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string());

            let bridge = bridge_msg.clone();
            let ws = ws_msg.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let response = match bridge
                    .call(
                        "engineMcpTool",
                        [JsValue::from_str(&tool), JsValue::from_str(&args_json)],
                    )
                    .await
                {
                    Ok(val) => {
                        // Worker parsed the result JSON; stringify back to text.
                        let text = js_sys::JSON::stringify(&val)
                            .ok() // ALLOW(ok): JsValue error has no Display; None handled below
                            .and_then(|s| s.as_string())
                            .unwrap_or_else(|| "null".to_string());
                        let content = serde_json::to_string(
                            &serde_json::json!([{"type": "text", "text": text}]),
                        )
                        .unwrap_or_default();
                        serde_json::json!({"id": id, "content": content})
                    }
                    Err(e) => {
                        let content = serde_json::to_string(&serde_json::json!([
                            {"type": "text", "text": format!("error: {e}")}
                        ]))
                        .unwrap_or_default();
                        serde_json::json!({"id": id, "is_error": true, "content": content})
                    }
                };
                if let Ok(s) = serde_json::to_string(&response) {
                    if ws.ready_state() == web_sys::WebSocket::OPEN {
                        let _ = ws.send_with_str(&s);
                    }
                }
            });
        }));
    ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();

    // onclose: clear the slot, then either reconnect or give up. This is where
    // an unreachable hub surfaces — see this function's doc comment.
    let url_on_close = url.clone();
    let onclose: Closure<dyn Fn(web_sys::CloseEvent)> =
        Closure::wrap(Box::new(move |_: web_sys::CloseEvent| {
            MCP_WS.with(|slot| *slot.borrow_mut() = None);
            // A socket that opened proves the hub exists, so its close is a
            // restart and the count starts over; one that never opened is
            // another consecutive failure to reach it.
            let next = if opened.get() { 0 } else { attempt + 1 };
            if next >= MCP_RELAY_MAX_ATTEMPTS {
                tracing::warn!(
                    "[mcp-relay] could not reach {url_on_close} in {next} attempts — giving up; \
                     MCP tooling is unavailable for this page"
                );
                return;
            }
            tracing::debug!("[mcp-relay] disconnected — reconnecting in 1 s (attempt {next})");
            let bridge = bridge.clone();
            wasm_bindgen_futures::spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(1000).await;
                connect_mcp_relay(bridge, next);
            });
        }));
    ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
    onclose.forget();
}

/// Extract the first `id` string from an `engineExecuteQuery` response
/// array. `holon_api::Value` is `#[serde(untagged)]`, so string columns
/// arrive as plain JS strings — NOT as `{Text: {value: "..."}}`. See
/// `value_serde_wire_format_is_untagged` in holon-api.
///
/// Uses `Reflect` everywhere instead of `dyn_ref::<js_sys::Array>()` —
/// the latter relies on `instanceof Array`, which returns false for
/// arrays that cross a postMessage structured-clone boundary (different
/// Array constructor in the cloned realm). `val.length` and `val[0]`
/// work regardless of realm.
fn extract_first_id(val: &JsValue) -> Option<String> {
    let len = Reflect::get(val, &"length".into())
        .ok()? // ALLOW(ok): JS reflection — absent property is a normal None
        .as_f64()
        .unwrap_or(0.0) as u32;
    if len == 0 {
        return None;
    }
    let item = Reflect::get(val, &JsValue::from_str("0")).ok()?; // ALLOW(ok): same as above
    if item.is_undefined() || item.is_null() {
        return None;
    }
    Reflect::get(&item, &"id".into()).ok()?.as_string() // ALLOW(ok): same as above
}

fn now_ms() -> u64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now() as u64)
        .unwrap_or(0)
}
