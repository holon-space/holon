use std::pin::Pin;
use std::sync::Arc;

use futures_signals::signal::{Signal, SignalExt};
use gpui::prelude::*;
use gpui::*;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::RenderContext;

use crate::entity_view_registry::LocalEntityScope;
use crate::geometry::BoundsRegistry;
use crate::navigation_state::NavigationState;
use crate::render::builders::{self, GpuiRenderContext};

/// A persistent GPUI view for a live query result.
///
/// Polls a `Signal<ReactiveViewModel>` directly from the GPUI executor.
pub struct LiveQueryView {
    current: ReactiveViewModel,
    ctx: RenderContext,
    services: Arc<dyn BuilderServices>,
    nav: NavigationState,
    bounds_registry: BoundsRegistry,
}

impl LiveQueryView {
    pub fn new(
        ctx: RenderContext,
        services: Arc<dyn BuilderServices>,
        signal: Pin<Box<dyn Signal<Item = ReactiveViewModel> + Send>>,
        nav: NavigationState,
        bounds_registry: BoundsRegistry,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn(async move |this, cx| {
            signal
                .for_each(|rvm| {
                    let _ = this.update(cx, |view, cx| {
                        view.current = rvm;
                        cx.notify();
                    });
                    async {}
                })
                .await;
        })
        .detach();

        Self {
            current: ReactiveViewModel::empty(),
            ctx,
            services,
            nav,
            bounds_registry,
        }
    }
}

impl Render for LiveQueryView {
    #[tracing::instrument(
        level = "trace",
        skip_all,
        name = "frontend.render",
        fields(component = "live_query")
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let gpui_ctx = GpuiRenderContext::new(
            self.ctx.clone(),
            self.services.clone(),
            self.bounds_registry.clone(),
            LocalEntityScope::new(),
            self.nav.clone(),
            window,
            cx,
        );
        builders::render(&self.current, &gpui_ctx)
    }
}
