//! `ToggleDrawer` GPUI binding: clicks the production drawer toggle
//! widget at the canonical `drawer_toggle_id_for(block_id)` element id,
//! travelling through the same `on_mouse_down → set_widget_open` chain
//! a real user's tap would. No test back door.

use holon_frontend::drawer_toggle_id_for;
use holon_pbt_core::{ToggleDrawer, TransitionImpl};
use validated::Validated::{self, Good};

use super::super::GpuiInteractionSession;

impl<'a> TransitionImpl<(), GpuiInteractionSession<'a>> for ToggleDrawer {
    type Reason = ();
    fn preconditions(&self, _: &()) -> Validated<(), Self::Reason> {
        Good(())
    }
    fn apply_to_ref(&self, _: &mut ()) {}
    async fn apply_to_sut(&self, _: &(), sut: &mut GpuiInteractionSession<'a>) {
        let toggle_id = drawer_toggle_id_for(&self.block_id);
        sut.click_at_element(&toggle_id);
    }
}
