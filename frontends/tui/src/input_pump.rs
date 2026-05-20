//! Channel-driven interaction pump for the TUI, mirroring
//! `holon_gpui::setup_interaction_pump` in shape.
//!
//! Two distinct paths exist now:
//!
//! 1. **PBT path (real input pipeline)** — `TuiUserDriver` owns a
//!    `tokio::sync::mpsc::Sender<InputEvent>` plumbed into the renderer
//!    task. Every PBT verb translates into a sequence of `InputEvent`s
//!    pushed through that channel; the renderer's third `tokio::select!`
//!    arm calls the real `AppMain::app_handle_input_event` for each
//!    event. **This bypasses the pump entirely** — the pump's
//!    `dispatch_interaction` is never called for PBT-emitted events.
//!
//! 2. **MCP path (engine fast-path)** — external MCP clients still send
//!    `InteractionCommand`s carrying a oneshot response. The pump task
//!    drains them and dispatches via `dispatch_interaction`. Today this
//!    only handles `MouseClick` (resolved to a click via
//!    `navigation::editor_focus` engine dispatch); other event kinds
//!    return `handled: false` with a `detail` explaining the v1 limit.
//!    MCP clients aren't on the input pipeline because (a) the message
//!    shape carries a oneshot the renderer arm has no place to wire up,
//!    and (b) MCP semantics are coarser-grained than per-keystroke
//!    events. Folding both paths into the renderer would force the MCP
//!    pump to learn the `InputEvent` shape and lose the response_tx;
//!    keep them separate.
//!
//! When r3bl_tui exposes a runtime input stream, the MCP-side
//! `MouseClick` could also flip to producing real `InputEvent`s and
//! routing through `input_tx`. Out-of-scope today.

use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
use holon_mcp::server::{DebugServices, InteractionCommand, InteractionEvent, InteractionResponse};
use r3bl_tui::InputEvent;

use crate::app_main::EditState;
use crate::render::RenderRegistry;
use crate::user_driver::TuiUserDriver;

/// Install the TUI's interaction pump and register the PBT-facing
/// `UserDriver`.
///
/// Mirrors `holon_gpui::setup_interaction_pump`:
///
/// 1. Build the [`InteractionCommand`] mpsc channel for MCP clients.
/// 2. Set [`DebugServices::interaction_tx`] (so MCP tools can send
///    events) and [`DebugServices::user_driver`] (so the PBT harness can
///    fetch a [`crate::user_driver::TuiUserDriver`] via the same shared
///    `Arc<DebugServices>`).
/// 3. Spawn a tokio task on `runtime` that drains the MCP channel and
///    dispatches each event via [`dispatch_interaction`] (engine
///    fast-path).
///
/// `input_tx` and the renderer-shared Arcs are forwarded into
/// `TuiUserDriver::new` so the PBT-driven driver can push real
/// `InputEvent`s into the renderer's select loop.
#[allow(clippy::too_many_arguments)]
pub fn setup_interaction_pump(
    debug: &Arc<DebugServices>,
    geometry: Arc<dyn GeometryProvider>,
    engine: Arc<ReactiveEngine>,
    runtime: tokio::runtime::Handle,
    input_tx: tokio::sync::mpsc::Sender<InputEvent>,
    last_registry: Arc<Mutex<RenderRegistry>>,
    focus_index: Arc<AtomicUsize>,
    edit_state: Arc<Mutex<Option<EditState>>>,
    render_seq: Arc<AtomicU64>,
    render_notify: Arc<tokio::sync::Notify>,
) {
    let (tx, mut rx) = futures::channel::mpsc::channel::<InteractionCommand>(16);
    debug.interaction_tx.set(tx.clone()).ok();

    let driver: Arc<dyn holon_frontend::user_driver::UserDriver> = Arc::new(TuiUserDriver::new(
        engine.clone(),
        geometry.clone(),
        input_tx,
        last_registry,
        focus_index,
        edit_state,
        render_seq,
        render_notify,
        tx,
    ));
    debug.user_driver.set(driver).ok();

    let pump_geometry = geometry;
    let pump_engine = engine;
    runtime.spawn(async move {
        while let Some(cmd) = rx.next().await {
            let response = dispatch_interaction(&cmd.event, &pump_geometry, &pump_engine).await;
            // Drop is fine here — caller may have abandoned the oneshot.
            let _ = cmd.response_tx.send(response);
        }
    });
}

/// MCP path event dispatcher. Stays on the engine fast-path (see module
/// docs); the PBT path bypasses this function entirely.
pub async fn dispatch_interaction(
    event: &InteractionEvent,
    geometry: &Arc<dyn GeometryProvider>,
    engine: &Arc<ReactiveEngine>,
) -> InteractionResponse {
    match event {
        InteractionEvent::MouseClick { position, .. } => {
            let Some(entity_id) = entity_at(geometry.as_ref(), *position) else {
                return InteractionResponse {
                    handled: false,
                    detail: Some(format!(
                        "no entity at position {:?} in TUI geometry",
                        position
                    )),
                };
            };
            // Focus the clicked block by setting the in-memory authority
            // directly (ADR 0010) — no `editor_cursor` write/CDC round-trip.
            // ALLOW(entity_uri_from_raw): boundary — entity_id from TUI geometry hit-test (a String)
            let block_uri = holon_api::EntityUri::from_raw(&entity_id);
            engine.set_focus(Some(block_uri));
            InteractionResponse {
                handled: true,
                detail: None,
            }
        }
        _ => InteractionResponse {
            handled: false,
            detail: Some(
                "TUI MCP-path interaction pump only handles MouseClick — \
                 see crate::input_pump module docs for the policy"
                    .into(),
            ),
        },
    }
}

/// Find the entity covering `position` (pixel-space). Picks the smallest
/// element that contains the point so a deeply-nested clickable wins
/// over its enclosing container.
fn entity_at(geometry: &dyn GeometryProvider, (px, py): (f32, f32)) -> Option<String> {
    geometry
        .all_elements()
        .into_iter()
        .filter(|(_, info)| {
            info.entity_id.is_some()
                && px >= info.x
                && px <= info.x + info.width
                && py >= info.y
                && py <= info.y + info.height
        })
        .min_by(|(_, a), (_, b)| {
            a.area()
                .partial_cmp(&b.area())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .and_then(|(_, info)| info.entity_id.map(|s| s.to_string()))
}
