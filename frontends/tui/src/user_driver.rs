//! TUI [`UserDriver`] mirroring [`holon_gpui::user_driver::GpuiUserDriver`].
//!
//! Field-for-field identical to GPUI's driver
//! (`tx` / `geometry` / `engine`) for structural symmetry between the two
//! frontend PBT harnesses — the side-by-side diff between
//! `gpui_ui_pbt.rs` and `tui_ui_pbt.rs` collapses to "what does the pump
//! do with each `InteractionEvent`", not "are the drivers shaped
//! differently".
//!
//! v1 dispatch path: r3bl_tui's `MockInputDevice` is a closed stream
//! (`gen_input_stream` consumes a fixed `InlineVec` and `run_main_event_loop`
//! breaks once it's empty), so we can't push `crossterm::Event`s into the
//! live event loop the way GPUI's `dispatch_event` pushes into the window
//! input pipeline. Instead, the driver delegates to an internal
//! [`ReactiveEngineDriver`] which dispatches operation intents directly on
//! the [`ReactiveEngine`] — same end-state as the GPUI path
//! (operation dispatch on the same engine), without exercising real
//! terminal input.
//!
//! When r3bl_tui grows a runtime input stream
//! (see `Open follow-up tasks` in the plan), this driver's body switches
//! to translating `InteractionEvent` → `crossterm::Event` and pushing it
//! through `tx` so the pump can drive the real input pipeline. The struct
//! shape doesn't change.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use futures::channel::mpsc::Sender;
use holon_api::{KeyChord, Value};
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::user_driver::{ReactiveEngineDriver, UserDriver};
use holon_mcp::server::InteractionCommand;

/// Channel-based [`UserDriver`] for the TUI. The struct fields mirror
/// `GpuiUserDriver` field-for-field; the `inner` [`ReactiveEngineDriver`]
/// holds the v1 dispatch implementation.
///
/// `tx` and `geometry` are populated for parity with GPUI and so future
/// work can swap the v1 engine path for a real `InteractionEvent` →
/// `crossterm::Event` pipeline without touching the harness or the test.
pub struct TuiUserDriver {
    pub tx: Sender<InteractionCommand>,
    pub geometry: Arc<dyn GeometryProvider>,
    pub engine: Arc<ReactiveEngine>,
    inner: ReactiveEngineDriver,
}

impl TuiUserDriver {
    pub fn new(
        tx: Sender<InteractionCommand>,
        geometry: Arc<dyn GeometryProvider>,
        engine: Arc<ReactiveEngine>,
    ) -> Self {
        let inner = ReactiveEngineDriver::new(engine.clone());
        Self {
            tx,
            geometry,
            engine,
            inner,
        }
    }
}

#[async_trait]
impl UserDriver for TuiUserDriver {
    async fn synthetic_dispatch(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()> {
        let intent = OperationIntent::new(entity.into(), op.into(), params);
        self.engine.dispatch_intent_sync(intent).await
    }

    async fn send_key_chord(
        &self,
        root_block_id: &str,
        root_tree: &ReactiveViewModel,
        entity_id: &str,
        chord: &KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool> {
        self.inner
            .send_key_chord(root_block_id, root_tree, entity_id, chord, extra_params)
            .await
    }

    async fn drop_entity(
        &self,
        root_block_id: &str,
        source_id: &str,
        target_id: &str,
    ) -> Result<bool> {
        self.inner
            .drop_entity(root_block_id, source_id, target_id)
            .await
    }
}
