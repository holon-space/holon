//! `DeliverBlockContent` GPUI binding: pushes deferred content through
//! the registry's structural-changes stream. Not a user action — a
//! test-only stimulus simulating async data arrival from the backend.

use holon_pbt_core::{DeliverBlockContent, TransitionImpl};
use validated::Validated::{self, Good};

use super::super::GpuiInteractionSession;

impl<'a> TransitionImpl<(), GpuiInteractionSession<'a>> for DeliverBlockContent {
    type Reason = ();
    fn preconditions(&self, _: &()) -> Validated<(), Self::Reason> {
        Good(())
    }
    fn apply_to_ref(&self, _: &mut ()) {}
    async fn apply_to_sut(&self, _: &(), sut: &mut GpuiInteractionSession<'a>) {
        sut.deliver_block_content_loaded(&self.block_id);
    }
}
