use std::sync::Arc;

use gpui::*;
use holon_frontend::view_model::ViewModel;
use holon_frontend::RenderPipeline;
use tokio::sync::watch;

use crate::geometry::BoundsRegistry;
use crate::render::builders::{self, GpuiRenderContext};

/// A persistent GPUI view for a single block.
///
/// Subscribes to `BlockWatchRegistry::watch_view_model()` and re-renders
/// only when this block's ViewModel changes — not when the root tree rebuilds.
pub struct BlockRefView {
    block_id: String,
    current: ViewModel,
    pipeline: Arc<RenderPipeline>,
    bounds_registry: BoundsRegistry,
}

impl BlockRefView {
    pub fn new(
        block_id: String,
        pipeline: Arc<RenderPipeline>,
        vm_rx: watch::Receiver<ViewModel>,
        bounds_registry: BoundsRegistry,
        cx: &mut Context<Self>,
    ) -> Self {
        let current = vm_rx.borrow().clone();

        // Bridge tokio watch::Receiver → GPUI cx.notify().
        // The watch::Receiver uses tokio — we spawn a tokio task to await
        // changes and bridge them to the GPUI entity via a smol channel.
        let (notify_tx, notify_rx) = smol::channel::bounded::<ViewModel>(1);
        let rt = pipeline.runtime_handle.clone();
        rt.spawn(async move {
            let mut rx = vm_rx;
            while rx.changed().await.is_ok() {
                let vm = rx.borrow().clone();
                // bounded(1): drops if GPUI side hasn't consumed yet (natural coalescing)
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
            block_id,
            current,
            pipeline,
            bounds_registry,
        }
    }

    pub fn block_id(&self) -> &str {
        &self.block_id
    }
}

impl Render for BlockRefView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let gpui_ctx = GpuiRenderContext {
            ctx: holon_frontend::RenderContext::from_pipeline(self.pipeline.clone()),
            bounds_registry: self.bounds_registry.clone(),
        };
        builders::render(&self.current, &gpui_ctx)
    }
}
