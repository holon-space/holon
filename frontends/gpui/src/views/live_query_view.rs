use std::sync::Arc;

use gpui::prelude::*;
use gpui::*;
use holon_frontend::view_model::ViewModel;
use tokio::sync::watch;

use crate::geometry::BoundsRegistry;
use crate::render::builders::{self, GpuiRenderContext};

/// A persistent GPUI view for a live query result.
///
/// Subscribes to `BlockWatchRegistry::watch_query_view_model()` and re-renders
/// only when the query result changes.
pub struct LiveQueryView {
    current: ViewModel,
    pipeline: Arc<holon_frontend::RenderPipeline>,
    bounds_registry: BoundsRegistry,
}

impl LiveQueryView {
    pub fn new(
        pipeline: Arc<holon_frontend::RenderPipeline>,
        vm_rx: watch::Receiver<ViewModel>,
        bounds_registry: BoundsRegistry,
        cx: &mut Context<Self>,
    ) -> Self {
        let current = vm_rx.borrow().clone();

        // Bridge tokio watch::Receiver → GPUI via smol channel.
        let (notify_tx, notify_rx) = smol::channel::bounded::<ViewModel>(1);
        let rt = pipeline.runtime_handle.clone();
        rt.spawn(async move {
            let mut rx = vm_rx;
            while rx.changed().await.is_ok() {
                let vm = rx.borrow().clone();
                let _ = notify_tx.try_send(vm);
            }
        });

        cx.spawn(async move |this, cx| {
            while let Ok(vm) = notify_rx.recv().await {
                let result = this.update(cx, |view, cx| {
                    view.current = vm;
                    cx.notify();
                });
                if result.is_err() {
                    break;
                }
            }
        })
        .detach();

        Self {
            current,
            pipeline,
            bounds_registry,
        }
    }
}

impl Render for LiveQueryView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let gpui_ctx = GpuiRenderContext {
            ctx: holon_frontend::RenderContext::from_pipeline(self.pipeline.clone()),
            bounds_registry: self.bounds_registry.clone(),
        };
        builders::render(&self.current, &gpui_ctx)
    }
}
