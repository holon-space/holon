//! Holon Dioxus Web — worker-bridge frontend.
//!
//! Architecture: `holon-frontend` + `BackendEngine` run inside a dedicated
//! `wasm32-wasip1-threads` Web Worker (the `holon-worker` crate). This
//! frontend receives serialized `ViewModel` snapshots via `postMessage` and
//! renders them as Dioxus elements. No holon crates are imported here —
//! the only coupling is the JSON wire format.

mod bridge;
mod editor;
mod render;

use std::cell::RefCell;

use bridge::WorkerBridge;
use dioxus::prelude::*;
use holon_frontend::view_model::ViewModel;
use js_sys::Reflect;
use serde_json::Value;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;

/// URL of the worker entry module, relative to the serving root.
const WORKER_URL: &str = "/web/worker-entry.mjs";
const DB_PATH: &str = ":memory:";

// WorkerBridge wraps Rc<_> so it is !Send. We keep it alive in a thread-local
// so Dioxus signals (which require Send) never need to hold it directly.
thread_local! {
    static BRIDGE: RefCell<Option<WorkerBridge>> = const { RefCell::new(None) };
    /// Active MCP relay WebSocket. Replaced on reconnect; None when hub is down.
    static MCP_WS: RefCell<Option<web_sys::WebSocket>> = const { RefCell::new(None) };
}

fn main() {
    console_error_panic_hook::set_once();
    tracing_wasm::set_as_global_default();
    tracing::info!("[holon-dioxus-web] booting");
    dioxus::launch(App);
}

#[derive(Clone, PartialEq)]
enum BootState {
    Booting,
    Ready { cold_start_ms: u64 },
    Failed(String),
}

#[component]
fn App() -> Element {
    let mut boot_state = use_signal(|| BootState::Booting);
    let mut view_model: Signal<Option<ViewModel>> = use_signal(|| None);

    use_future(move || async move {
        let t0 = now_ms();

        let bridge = match WorkerBridge::spawn(WORKER_URL).await {
            Ok(b) => b,
            Err(e) => {
                boot_state.set(BootState::Failed(format!("worker spawn: {e}")));
                return;
            }
        };

        if let Err(e) = bridge.call("engineInit", [DB_PATH.into()]).await {
            boot_state.set(BootState::Failed(format!("engineInit: {e}")));
            return;
        }

        // Connect the MCP relay bridge (best-effort; reconnects automatically on close).
        connect_mcp_relay(bridge.clone());

        // The root layout block has a well-known id set by seed_default_layout.
        let root_val = match bridge
            .call(
                "engineExecuteQuery",
                ["SELECT id FROM block WHERE id='block:root-layout' LIMIT 1".into()],
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
                // No root block — show degraded ready UI. Set BRIDGE
                // before flipping boot state so a synchronous re-render
                // sees it populated.
                BRIDGE.with(|b| *b.borrow_mut() = Some(bridge));
                let cold_start_ms = now_ms().saturating_sub(t0);
                boot_state.set(BootState::Ready { cold_start_ms });
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

        bridge.on_snapshot(handle, move |json| {
            match serde_json::from_str::<ViewModel>(&json) {
                Ok(v) => {
                    if let Some(saved) = editor::cursor::save() {
                        editor::cursor::enqueue_restore(saved);
                    }
                    view_model.set(Some(v));
                }
                Err(e) => tracing::error!("[snapshot] deserialize failed: {e}"),
            }
        });

        // Store bridge in thread-local BEFORE marking ready, so any render
        // path triggered by BootState::Ready sees a live BRIDGE.
        BRIDGE.with(|b| *b.borrow_mut() = Some(bridge));

        let cold_start_ms = now_ms().saturating_sub(t0);
        boot_state.set(BootState::Ready { cold_start_ms });
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
                        span {
                            style: "color: #555; font-size: 0.75em;",
                            "· [layout: degraded — AvailableSpace=None in worker]"
                        }
                    },
                    BootState::Failed(err) => rsx! {
                        span { style: "color: #ff5252; font-size: 0.8em;", "⚠ {err}" }
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
                        div { style: "color: #ff5252; padding: 32px;",
                            h2 { "Boot failed" }
                            pre {
                                style: "white-space: pre-wrap; font-size: 0.85em;",
                                "{err}"
                            }
                        }
                    },
                    (BootState::Ready { .. }, Some(vm)) => rsx! {
                        render::RenderNode { node: vm.clone() }
                    },
                    (BootState::Ready { .. }, None) => rsx! {
                        div {
                            style: "color: #888; font-style: italic; padding: 32px; text-align: center;",
                            "No root layout found. Expected block with id='block:root-layout'."
                        }
                    },
                }
            }
        }
    }
}

/// Connect to the MCP relay hub as `role=browser`. All incoming tool calls
/// are forwarded to the worker via `engineMcpTool` and the results are sent back.
/// Reconnects automatically after 1 second when the hub closes (handles
/// `trunk --watch` restarts without requiring a page reload).
fn connect_mcp_relay(bridge: WorkerBridge) {
    let host = web_sys::window()
        .and_then(|w| w.location().host().ok())
        .unwrap_or_else(|| "localhost:8765".to_string());
    let url = format!("ws://{host}/mcp-hub?role=browser");

    let ws = match web_sys::WebSocket::new(&url) {
        Ok(ws) => ws,
        Err(e) => {
            tracing::warn!("[mcp-relay] connect failed: {e:?} — will retry in 1s");
            let bridge_clone = bridge.clone();
            wasm_bindgen_futures::spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(1000).await;
                connect_mcp_relay(bridge_clone);
            });
            return;
        }
    };

    MCP_WS.with(|slot| *slot.borrow_mut() = Some(ws.clone()));
    tracing::debug!("[mcp-relay] connecting to {url}");

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
                            .ok()
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

    // onclose: clear slot and reconnect after 1 second.
    let onclose: Closure<dyn Fn(web_sys::CloseEvent)> =
        Closure::wrap(Box::new(move |_: web_sys::CloseEvent| {
            tracing::debug!("[mcp-relay] disconnected — reconnecting in 1 s");
            MCP_WS.with(|slot| *slot.borrow_mut() = None);
            let bridge = bridge.clone();
            wasm_bindgen_futures::spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(1000).await;
                connect_mcp_relay(bridge);
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
        .ok()?
        .as_f64()
        .unwrap_or(0.0) as u32;
    if len == 0 {
        return None;
    }
    let item = Reflect::get(val, &JsValue::from_str("0")).ok()?;
    if item.is_undefined() || item.is_null() {
        return None;
    }
    Reflect::get(&item, &"id".into()).ok()?.as_string()
}

fn now_ms() -> u64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now() as u64)
        .unwrap_or(0)
}
