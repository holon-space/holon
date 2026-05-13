//! `ToggleCollapse` GPUI binding: clicks the production chevron at the
//! canonical `expand_toggle_id_for(target_id)` element id, flipping the
//! row's `expanded` `Mutable<bool>` through the same `on_mouse_down`
//! chain a real user's chevron tap would.

use holon_frontend::expand_toggle_id_for;
use holon_pbt_core::{ToggleCollapse, TransitionImpl};
use validated::Validated::{self, Good};

use super::super::GpuiInteractionSession;

impl<'a> TransitionImpl<(), GpuiInteractionSession<'a>> for ToggleCollapse {
    type Reason = ();
    fn preconditions(&self, _: &()) -> Validated<(), Self::Reason> {
        Good(())
    }
    fn apply_to_ref(&self, _: &mut ()) {}
    async fn apply_to_sut(&self, _: &(), sut: &mut GpuiInteractionSession<'a>) {
        let toggle_id = expand_toggle_id_for(&self.target_id);
        sut.click_at_element(&toggle_id);
    }
}
