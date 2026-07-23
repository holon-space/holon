//! `inv-live-block-shell-present`.
//!
//! @pbt oracle internal-consistency
//! @pbt covers shell-faithfulness — the windowed render actually routed panel
//!   blocks through the per-block `live_block` → `ReactiveShell` wrapper, the
//!   production mount path, rather than a bare static-VM mount
//! @pbt slips-if-removed a fixture that mounts blocks bare (no `ReactiveShell`
//!   wrapper) renders "fine" in a screenshot yet skips the reactive
//!   subscription / caret / bounds-registry plumbing the shell owns — exactly
//!   the class of bug that hid twice this week behind shell-less dedicated
//!   fixtures
//!
//! The distinguishing observable: `live_block::render` (frontends/gpui) tags
//! each PANEL container (`block:default-*`) into the geometry registry via
//! `tag_with_entity_id(ctx, "live_block", Some(bid), …)` — and it only does so
//! after `get_or_create_live_block` has spun up the block's `ReactiveShell`
//! entity. So a `live_block`-typed element whose `entity_id` is a
//! `block:default-*` panel is present in the snapshot **iff** the production
//! per-block shell wrapping ran. A bare static-VM mount (e.g. the
//! `seeded_accordion_panel_smoke` fixture) never emits such an element.
//!
//! `Strict`. `Skipped` while the window has committed no frame yet (empty
//! snapshot — root still loading); otherwise it asserts the shell tag is
//! present. Engagement is gated (in the wiring) on `SutLayout +
//! SutFrontendEngine`, i.e. only the live windowed slice.

use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvLiveBlockShellPresent;

impl InvLiveBlockShellPresent {
    pub const ID: InvariantId = InvariantId("inv-live-block-shell-present");
    const LABEL: &'static str = "inv-live-block-shell-present";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvLiveBlockShellPresent
where
    S: SutLayout,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let elements = sut.rendered_elements().await;

        // No committed frame yet — the window is still loading. Treat as Skip
        // (matches the `inv-frontend-bounds-rendered` empty-snapshot policy);
        // the settle hook drives the tree non-empty before the real checks.
        if elements.is_empty() {
            return InvariantResult::Skipped(format!(
                "[{}] BoundsRegistry empty — window has committed no frame yet",
                Self::LABEL
            ));
        }

        let shell_tagged = elements.iter().any(|el| {
            el.widget_type == "live_block"
                && el
                    .entity_id
                    .as_ref()
                    .is_some_and(|eid| eid.as_str().starts_with("block:default-"))
        });

        if shell_tagged {
            InvariantResult::Ok
        } else {
            InvariantResult::Fail(format!(
                "[{}] rendered {} element(s) but NONE is a `live_block`-tagged \
                 `block:default-*` panel — the window did not route blocks through the \
                 production per-block `ReactiveShell` wrapper (`live_block::render`). This is \
                 the bare-mount masking signature: blocks laid out without the shell that owns \
                 their reactive subscription / caret / bounds plumbing.",
                Self::LABEL,
                elements.len(),
            ))
        }
    }
}
