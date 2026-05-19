//! Bridge between `holon-integration-tests`' rich PBT state/SUT and
//! the shared `holon_layout_testing` capability traits.
//!
//! - `impl LayoutRefState for ReferenceState`: lets the shared
//!   `weighted_generator` / `preconditions` bodies in
//!   `holon_layout_testing::transitions::*` consume our reference state
//!   directly (via `LayoutRef::new(&state)`).
//! - `SutClickAdapter`: wraps `&mut dyn SutHandle` so it implements the
//!   capability traits (`Clickable`, `LiveBlockSink`) the shared
//!   `apply_to_sut` bodies require. Delegates to two new `SutHandle`
//!   methods (`apply_click_at_element` and `apply_deliver_block_content_loaded`).
//!
//! ## Why a bridge instead of impls on `SutHandle` directly?
//!
//! `SutHandle` is `dyn`-safe and already big. The capability impls live
//! on a wrapper so the trait surface of `SutHandle` doesn't grow a
//! second axis of "things drivers must implement". New shared
//! capabilities are added by extending this adapter, not by widening
//! the SUT trait.

use std::sync::OnceLock;

use holon_layout_testing::blueprint::{BlockHandle, DrawerHandle};
use holon_layout_testing::{Clickable, LayoutRefState, LiveBlockSink};

use super::reference_state::ReferenceState;
use super::transition_dispatch::SutHandle;

/// Default-layout drawer handles. The integration-tests PBT always boots the
/// default layout, which renders two sidebars (`block:default-left-sidebar`
/// and `block:default-right-sidebar`) as toggleable drawers. We surface them
/// once via `OnceLock` so `LayoutRefState::drawer_handles` can return a
/// stable `&[DrawerHandle]`.
fn default_drawer_handles() -> &'static [DrawerHandle] {
    static HANDLES: OnceLock<Vec<DrawerHandle>> = OnceLock::new();
    HANDLES
        .get_or_init(|| {
            vec![
                DrawerHandle {
                    block_id: "block:default-left-sidebar".to_string(),
                },
                DrawerHandle {
                    block_id: "block:default-right-sidebar".to_string(),
                },
            ]
        })
        .as_slice()
}

impl LayoutRefState for ReferenceState {
    /// VMS handles aren't modelled in `ReferenceState` today — the rich
    /// PBT doesn't currently exercise `SwitchViewMode`. Returning empty
    /// means the shared `SwitchViewMode::weighted_generator` will
    /// rightly `Fail(NoSwitchableHandles)`.
    ///
    /// TODO: walk the rendered VM tree for `widget_name == "view_mode_switcher"`
    /// nodes and surface them here.
    fn switchable_handles(&self) -> &[BlockHandle] {
        &[]
    }

    /// Default layout's left/right sidebars are real drawers; surface them
    /// once the app has started. Before `apply_start_app` they don't render
    /// yet, so return empty so `ToggleDrawer` skips them.
    fn drawer_handles(&self) -> &[DrawerHandle] {
        if self.app_started {
            default_drawer_handles()
        } else {
            &[]
        }
    }

    /// Deferred `live_block` placeholders are a fast-UI test concern
    /// (the GPUI layout PBT's `arb_deferred_live_block_scenario`). The
    /// integration-tests PBT runs a real backend that always returns
    /// real data — no deferred placeholders to deliver.
    fn deferred_block_ids(&self) -> Vec<String> {
        Vec::new()
    }

    /// Surface every currently-collapsible target the ref state knows
    /// about. `ReferenceState::expanded_toggles` already tracks the
    /// SET of expanded ones; the complete set of toggleable targets is
    /// derivable from the rendered tree. For now we return the
    /// already-expanded ones (which `ToggleCollapse` would collapse).
    /// Extending to "also include collapsed ones available to expand"
    /// needs a rendered-tree walk — follow-up.
    fn collapsible_target_ids(&self) -> Vec<String> {
        self.expanded_toggles
            .iter()
            .map(|uri| uri.to_string())
            .collect()
    }

    fn current_view_mode(&self, _: &str) -> Option<&str> {
        None
    }

    fn drawer_is_open(&self, block_id: &str) -> bool {
        // Default to open: production default layout boots both sidebars open.
        // `ToggleDrawer::apply_to_ref` flips the recorded state.
        self.drawer_open.get(block_id).copied().unwrap_or(true)
    }
}

/// Wrap `&mut S` (any `S: SutHandle`) so it satisfies the capability
/// traits the shared `apply_to_sut` bodies require. Construct at the
/// dispatch site: `let mut sut = SutClickAdapter(handle); let mut
/// layout_sut = LayoutSut::new(&mut sut);`. Generic over `S` since
/// `SutHandle` is no longer `dyn`-dispatched (concrete-`S` dispatch).
pub struct SutClickAdapter<'a, S: SutHandle>(pub &'a mut S);

impl<S: SutHandle> Clickable for SutClickAdapter<'_, S> {
    fn click_at_element(&mut self, element_id: &str) {
        // The shared `apply_to_sut` bodies are async; the SutHandle method is
        // too. We're already inside a tokio runtime (the integration-tests
        // PBT runner uses `runtime.block_on`), so `futures::executor::block_on`
        // would `EnterError`. `block_in_place` releases the worker so we can
        // call `Handle::current().block_on` synchronously without deadlock.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.0.apply_click_at_element(element_id))
        });
    }
}

impl<S: SutHandle> LiveBlockSink for SutClickAdapter<'_, S> {
    fn deliver_block_content_loaded(&mut self, block_id: &str) {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(self.0.apply_deliver_block_content_loaded(block_id))
        });
    }
}
