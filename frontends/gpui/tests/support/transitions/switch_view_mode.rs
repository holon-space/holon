//! `SwitchViewMode` GPUI binding: clicking the canonical VMS button
//! id flips the VMS to the requested mode through the production
//! click handler chain.

use holon_frontend::vms_button_id_for;
use holon_pbt_core::{SwitchViewMode, TransitionImpl};
use validated::Validated::{self, Good};

use super::super::GpuiInteractionSession;

impl<'a> TransitionImpl<(), GpuiInteractionSession<'a>> for SwitchViewMode {
    type Reason = ();
    fn preconditions(&self, _: &()) -> Validated<(), Self::Reason> {
        Good(())
    }
    fn apply_to_ref(&self, _: &mut ()) {}
    async fn apply_to_sut(&self, _: &(), sut: &mut GpuiInteractionSession<'a>) {
        let button_id = vms_button_id_for(&self.block_id, &self.target_mode);
        sut.click_at_element(&button_id);
    }
}
