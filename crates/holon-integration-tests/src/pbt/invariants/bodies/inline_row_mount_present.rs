//! `inv-inline-row-mount-present` — the TUI-flavored sibling of
//! `inv-live-block-shell-present`.
//!
//! @pbt oracle internal-consistency
//! @pbt covers mount-faithfulness for frontends that resolve doc-block rows
//!   INLINE — the windowed render actually routed rows through the production
//!   `tree`/`table`/`outline` collection boundary that registers each row as
//!   the rendering of its block entity, rather than painting text nobody can
//!   locate by URI
//! @pbt slips-if-removed a renderer that paints rows without registering them
//!   looks identical in a screenshot yet leaves `BoundsRegistry` free of any
//!   block-bearing row, so every by-URI region query (focus, caret, click
//!   routing, the whole `UserDriver` surface) silently addresses nothing
//!
//! The distinguishing observable: the TUI's `render_collection_vertical`
//! (frontends/tui/src/render/mod.rs) registers each `tree`/`table`/`outline`
//! row as `widget_type = "render_entity"` keyed by the row's block id —
//! deliberately NOT `live_block`, because in this frontend no per-block shell
//! wrapper survives (`render_live_block` skips registration for the three
//! `block:default-*` layout containers). So a `render_entity`-tagged `block:*`
//! element is present **iff** the production inline-row mount path ran.
//!
//! Why this is a separate invariant rather than a relaxation of
//! `inv-live-block-shell-present`: the two frontends have genuinely different
//! production mount architectures, and the caps
//! (`SutPerBlockShellMount` / `SutInlineRowMount`) name which one is under
//! test. Checking a frontend against the other's marker is what made the
//! shell invariant a permanent red on the TUI.
//!
//! `Strict`. `Skipped` while the window has committed no frame yet (empty
//! snapshot — root still loading); otherwise it asserts an inline-mounted row
//! is present.

use holon_api::EntityUri;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

/// The inline-row mount observable: a geometry element that IS the rendering
/// of a particular block entity. Shared with the windowed harness's readiness
/// gate, which must not admit a frame that has painted nothing addressable.
pub fn is_inline_row_tag(widget_type: &str, entity_id: Option<&str>) -> bool {
    widget_type == "render_entity" && entity_id.is_some_and(|eid| eid.starts_with("block:"))
}

pub struct InvInlineRowMountPresent;

impl InvInlineRowMountPresent {
    pub const ID: InvariantId = InvariantId("inv-inline-row-mount-present");
    const LABEL: &'static str = "inv-inline-row-mount-present";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvInlineRowMountPresent
where
    S: SutLayout,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let elements = sut.rendered_elements().await;

        if elements.is_empty() {
            return InvariantResult::Skipped(format!(
                "[{}] BoundsRegistry empty — window has committed no frame yet",
                Self::LABEL
            ));
        }

        let inline_mounted = elements.iter().any(|el| {
            is_inline_row_tag(
                &el.widget_type,
                el.entity_id.as_ref().map(EntityUri::as_str),
            )
        });

        if inline_mounted {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(format!(
                "[{}] rendered {} element(s) but NONE is a `render_entity`-tagged `block:*` row — \
                 the window painted no block through the production inline-row mount path (the \
                 `tree`/`table`/`outline` collection boundary that registers each row by its \
                 block id). Nothing in this frame is addressable by URI, so every by-URI region \
                 query resolves to nothing.",
                Self::LABEL,
                elements.len(),
            ))
        }
    }
}
