//! Channel-driven interaction pump for the TUI, mirroring
//! `holon_gpui::setup_interaction_pump` in shape.
//!
//! Populates [`DebugServices::interaction_tx`] and
//! [`DebugServices::user_driver`] so MCP tools and the PBT harness can
//! find the same channel and driver the GPUI flavour does. The pump task
//! drains [`InteractionCommand`]s and resolves each [`InteractionEvent`]
//! through the engine (v1 — see module-level note below).
//!
//! v1 dispatch policy: r3bl_tui's input device is a fixed-vector mock
//! (`MockInputDevice` consumes a `gen_input_stream` `InlineVec` until
//! exhausted, then `run_main_event_loop` breaks). There is no public way
//! to push a `crossterm::Event` into the live event loop today, so the
//! pump can't drive the real terminal input pipeline the way GPUI's
//! pump drives `dispatch_event`/`dispatch_keystroke`. Instead:
//!
//! - [`InteractionEvent::MouseClick`] → look up the entity at the click
//!   position via the [`GeometryProvider`], dispatch the bound click
//!   intent (or a `navigation::editor_focus` fallback) on the
//!   [`ReactiveEngine`].
//! - [`InteractionEvent::KeyDown`] / `KeyUp` / `Type` / `Mouse{Down,Up,Move}` /
//!   `ScrollWheel` → not yet wired; the pump returns
//!   `handled: false` with a `detail` explaining the v1 limit. The PBT
//!   doesn't depend on this path because [`crate::user_driver::TuiUserDriver`]
//!   takes the engine fast-path through `ReactiveEngineDriver`; MCP
//!   clients that send these events get a clear error instead of a
//!   silent no-op.
//!
//! When r3bl_tui exposes a runtime input stream
//! (see "Open follow-up tasks" in the plan), this module flips to
//! producing real `crossterm::Event`s.

use std::sync::Arc;

use futures::StreamExt;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
use holon_frontend::user_driver::UserDriver;
use holon_mcp::server::{DebugServices, InteractionCommand, InteractionEvent, InteractionResponse};

use crate::user_driver::TuiUserDriver;

/// Install the TUI's interaction pump.
///
/// Mirrors `holon_gpui::setup_interaction_pump`:
///
/// 1. Build the [`InteractionCommand`] mpsc channel.
/// 2. Set [`DebugServices::interaction_tx`] (so MCP tools can send
///    events) and [`DebugServices::user_driver`] (so the PBT harness can
///    fetch a [`UserDriver`] via the same shared `Arc<DebugServices>`).
/// 3. Spawn a tokio task on `runtime` that drains the channel and
///    dispatches each event via [`dispatch_interaction`].
pub fn setup_interaction_pump(
    debug: &Arc<DebugServices>,
    geometry: Arc<dyn GeometryProvider>,
    engine: Arc<ReactiveEngine>,
    runtime: tokio::runtime::Handle,
) {
    let (tx, mut rx) = futures::channel::mpsc::channel::<InteractionCommand>(16);
    debug.interaction_tx.set(tx.clone()).ok();

    let driver: Arc<dyn UserDriver> =
        Arc::new(TuiUserDriver::new(tx, geometry.clone(), engine.clone()));
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

/// v1 event dispatcher (see module-level doc for policy).
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
            let mut params = std::collections::HashMap::new();
            params.insert(
                "block_id".into(),
                holon_api::Value::String(entity_id.clone()),
            );
            params.insert("region".into(), holon_api::Value::String("main".into()));
            params.insert("cursor_offset".into(), holon_api::Value::Integer(0));
            let intent = OperationIntent::new("navigation".into(), "editor_focus".into(), params);
            match engine.dispatch_intent_sync(intent).await {
                Ok(()) => InteractionResponse {
                    handled: true,
                    detail: None,
                },
                Err(e) => InteractionResponse {
                    handled: false,
                    detail: Some(format!("editor_focus dispatch failed: {e}")),
                },
            }
        }
        _ => InteractionResponse {
            handled: false,
            detail: Some(
                "TUI v1 interaction pump only handles MouseClick — \
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
        .and_then(|(_, info)| info.entity_id)
}
