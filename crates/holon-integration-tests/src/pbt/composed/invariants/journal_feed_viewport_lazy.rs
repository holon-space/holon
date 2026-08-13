//! `inv-journal-feed-viewport-lazy` wired into the composed catalog.
//! Needs `RefJournalFeed + RefLayout + RefViewSelection + RefFocus` (ref side)
//! + `SutRenderer + SutLayout` (SUT). `SutLayout` is the windowed geometry cap,
//! so headless slices deselect honestly rather than judging a viewport they
//! cannot see.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefJournalFeed;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::journal_feed_viewport_lazy::InvJournalFeedViewportLazy;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvJournalFeedViewportLazy,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>(), CapId::of::<dyn SutLayout>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefJournalFeed>(),
                CapId::of::<dyn RefLayout>(),
                CapId::of::<dyn RefViewSelection>(),
                CapId::of::<dyn RefFocus>(),
            ],
        },
        Attribution::at(Layer::ViewModel, file!()),
    ))
}
