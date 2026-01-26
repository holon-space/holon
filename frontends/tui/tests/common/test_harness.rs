//! `CapturingApp` — a thin r3bl_tui [`App`] wrapper around
//! [`holon_tui::app_main::AppMain`] that intercepts each `app_render`
//! pass and composes the resulting `RenderPipeline` into a shared
//! [`OffscreenBuffer`] (sidesteps `GlobalData::maybe_saved_ofs_buf`
//! privacy without forking r3bl).
//!
//! Used by `tui_ui_pbt`: the harness wires the same
//! `Arc<RwLock<Option<OffscreenBuffer>>>` into both this wrapper (writer)
//! and `OffscreenBufferBackend` (reader), so screenshot capture is
//! synchronous with respect to whatever frame r3bl just painted.

use std::sync::{Arc, RwLock};

use holon_tui::app_main::{AppMain, AppSignal, TuiState};
use r3bl_tui::{
    App, CommonResult, ComponentRegistryMap, EventPropagation, GlobalData, HasFocus, InputEvent,
    MemoizedLenMap, OffscreenBuffer, RenderPipeline,
};

/// `App` impl that delegates to [`AppMain`] and snapshots each frame's
/// composed `OffscreenBuffer` into a shared slot.
pub struct CapturingApp {
    inner: AppMain,
    captured: Arc<RwLock<Option<OffscreenBuffer>>>,
}

impl CapturingApp {
    pub fn new(captured: Arc<RwLock<Option<OffscreenBuffer>>>) -> Self {
        Self {
            inner: AppMain::default(),
            captured,
        }
    }
}

impl App for CapturingApp {
    type S = TuiState;
    type AS = AppSignal;

    fn app_init(
        &mut self,
        registry: &mut ComponentRegistryMap<TuiState, AppSignal>,
        focus: &mut HasFocus,
    ) {
        self.inner.app_init(registry, focus);
    }

    fn app_handle_input_event(
        &mut self,
        event: InputEvent,
        global: &mut GlobalData<TuiState, AppSignal>,
        registry: &mut ComponentRegistryMap<TuiState, AppSignal>,
        focus: &mut HasFocus,
    ) -> CommonResult<EventPropagation> {
        self.inner
            .app_handle_input_event(event, global, registry, focus)
    }

    fn app_handle_signal(
        &mut self,
        signal: &AppSignal,
        global: &mut GlobalData<TuiState, AppSignal>,
        registry: &mut ComponentRegistryMap<TuiState, AppSignal>,
        focus: &mut HasFocus,
    ) -> CommonResult<EventPropagation> {
        self.inner
            .app_handle_signal(signal, global, registry, focus)
    }

    fn app_render(
        &mut self,
        global: &mut GlobalData<TuiState, AppSignal>,
        registry: &mut ComponentRegistryMap<TuiState, AppSignal>,
        focus: &mut HasFocus,
    ) -> CommonResult<RenderPipeline> {
        let pipeline = self.inner.app_render(global, registry, focus)?;
        let mut buf = OffscreenBuffer::new_empty(global.window_size);
        let mut memo = MemoizedLenMap::default();
        pipeline.compose_render_ops_into_ofs_buf(global.window_size, &mut buf, &mut memo);
        *self.captured.write().unwrap() = Some(buf);
        Ok(pipeline)
    }
}
