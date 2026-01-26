use std::sync::Arc;

use crate::render::builders::prelude::hashed_id;
use gpui::*;
use holon_api::EntityUri;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::RenderContext;

use crate::entity_view_registry::{EntityCache, LiveBlockAncestors, LocalEntityScope};
use crate::geometry::BoundsRegistry;
use crate::navigation_state::NavigationState;
use crate::render::builders::{self, GpuiRenderContext};

/// A persistent GPUI view for a single rendered entity (collection row).
///
/// Owns a per-row `entity_cache` so nested `live_block` / `live_query` /
/// `render_entity` entities created lazily by their builders survive
/// across `VecDiff::UpdateAt` calls. When the row's data changes, only
/// this view re-renders.
pub struct RenderEntityView {
    current: Arc<ReactiveViewModel>,
    ctx: RenderContext,
    services: Arc<dyn BuilderServices>,
    nav: NavigationState,
    bounds_registry: BoundsRegistry,
    entity_cache: EntityCache,
    /// Ancestor `live_block` ids leading down to this row's parent shell,
    /// captured at creation time. Re-emitted into each render frame's
    /// `GpuiRenderContext` so the lazy `live_block` builder can refuse
    /// cyclic creation across the row's entity boundary (mirrors the
    /// equivalent field on `ReactiveShell`).
    live_block_ancestors: LiveBlockAncestors,
}

impl RenderEntityView {
    pub fn new(
        current: Arc<ReactiveViewModel>,
        ctx: RenderContext,
        services: Arc<dyn BuilderServices>,
        nav: NavigationState,
        bounds_registry: BoundsRegistry,
        live_block_ancestors: LiveBlockAncestors,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            current,
            ctx,
            services,
            nav,
            bounds_registry,
            entity_cache: Default::default(),
            live_block_ancestors,
        }
    }

    /// Push a fresh row RVM into this view in place. Preserves entity
    /// identity for matching widgets via [`ReactiveViewModel::with_update`]
    /// and triggers a re-render — lazy builders pick up any structural
    /// changes from `entity_cache` (or create new entries for new ids).
    pub fn set_content(&mut self, new: Arc<ReactiveViewModel>, cx: &mut Context<Self>) {
        let updated = self.current.with_update(&new);
        self.current = Arc::new(updated);
        cx.notify();
    }

    pub fn row_id(&self) -> Option<String> {
        self.current.row_id()
    }
}

impl Render for RenderEntityView {
    #[tracing::instrument(
        level = "trace",
        skip_all,
        name = "frontend.render",
        fields(component = "entity")
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let local = LocalEntityScope::new().with_cache(self.entity_cache.clone());
        let gpui_ctx = GpuiRenderContext::new(
            self.ctx.clone(),
            self.services.clone(),
            self.bounds_registry.clone(),
            local,
            self.nav.clone(),
            window,
            cx,
        )
        .with_live_block_ancestors(self.live_block_ancestors.clone());

        let Some(ref slot) = self.current.slot else {
            return builders::render(&self.current, &gpui_ctx);
        };

        let content = slot.content.lock_ref();
        let child_el = builders::render(&content, &gpui_ctx);

        let block_id = self
            .current
            .entity()
            .get("id")
            .and_then(|v| v.as_string())
            .map(|s| EntityUri::from_raw(s));

        let Some(ref id) = block_id else {
            return child_el;
        };

        let is_focused = gpui_ctx.services().focused_block().as_ref() == Some(id);
        if is_focused {
            return child_el;
        }

        let id_for_click = id.clone();
        let el_id = format!("render-entity-{}", id);
        let services = gpui_ctx.services.clone();
        div()
            .id(hashed_id(&el_id))
            .cursor_pointer()
            .child(child_el)
            .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                services.set_focus(Some(id_for_click.clone()));
            })
            .into_any_element()
    }
}
