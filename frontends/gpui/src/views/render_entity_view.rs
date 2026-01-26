use std::sync::Arc;

use gpui::*;
use holon_api::EntityUri;
use holon_frontend::RenderContext;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;

use crate::entity_view_registry::EntityCache;
use crate::entity_view_registry::LiveBlockAncestors;
use crate::entity_view_registry::LocalEntityScope;
use crate::geometry::BoundsRegistry;
use crate::navigation_state::NavigationState;
use crate::render::builders::GpuiRenderContext;
use crate::render::builders::prelude::click_to_focus;
use crate::render::builders::{self};

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
    /// Shell-level cache one level up. `LiveQuery` lookups route here so
    /// data-semantic queries (same SQL → same result) share state across
    /// rows in the same shell. All other kinds stay row-scoped.
    parent_cache: EntityCache,
    /// Ancestor `live_block` ids leading down to this row's parent shell,
    /// captured at creation time. Re-emitted into each render frame's
    /// `GpuiRenderContext` so the lazy `live_block` builder can refuse
    /// cyclic creation across the row's entity boundary (mirrors the
    /// equivalent field on `ReactiveShell`).
    live_block_ancestors: LiveBlockAncestors,
    /// Latched when this row's block held focus; armed → the next unfocused
    /// render evicts the cached `EditorView` (see `render`). A latch rather
    /// than an every-render sweep so templates that render `editable_text`
    /// unconditionally don't thrash create/drop each frame.
    editor_pending_evict: bool,
}

impl RenderEntityView {
    pub fn new(
        current: Arc<ReactiveViewModel>,
        ctx: RenderContext,
        services: Arc<dyn BuilderServices>,
        nav: NavigationState,
        bounds_registry: BoundsRegistry,
        parent_cache: EntityCache,
        live_block_ancestors: LiveBlockAncestors,
        _: &mut Context<Self>,
    ) -> Self {
        Self {
            current,
            ctx,
            services,
            nav,
            bounds_registry,
            entity_cache: Default::default(),
            parent_cache,
            live_block_ancestors,
            editor_pending_evict: false,
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
        let local = LocalEntityScope::new()
            .with_cache(self.entity_cache.clone())
            .with_parent(self.parent_cache.clone());
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
            .map(EntityUri::from_raw);

        let Some(ref id) = block_id else {
            return child_el;
        };

        let is_focused = gpui_ctx.services().focused_block().as_ref() == Some(id);
        if is_focused {
            self.editor_pending_evict = true;
            return child_el;
        }

        let eviction_enabled = std::env::var("HOLON_EDITOR_EVICT")
            .map(|v| v != "off")
            .unwrap_or(true);
        if eviction_enabled && self.editor_pending_evict {
            // Defocused: drop the row's cached editor so InputState + undo
            // history + line layouts don't accumulate one-per-ever-focused
            // block. Keep an editor whose input still holds window focus —
            // on an A→B move this row can re-render before B's editor has
            // mounted; evicting then would blur the window. The latch stays
            // armed and retries next render.
            let all_gone = crate::entity_view_registry::evict_ephemeral_with_prefix(
                &self.entity_cache,
                "editable-text-",
                |any| {
                    any.clone()
                        .downcast::<crate::views::EditorView>()
                        .map(|editor| {
                            use gpui::Focusable;
                            let input = editor.read(cx).input_entity();
                            input.focus_handle(cx).is_focused(window)
                        })
                        .unwrap_or(true)
                },
            );
            if all_gone {
                self.editor_pending_evict = false;
            }
        }

        let el_id = format!("render-entity-{}", id);
        let services = gpui_ctx.services.clone();
        click_to_focus(&el_id, child_el, id.clone(), services).into_any_element()
    }
}
