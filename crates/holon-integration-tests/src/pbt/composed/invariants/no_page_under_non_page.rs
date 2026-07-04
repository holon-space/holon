//! `inv-no-page-under-non-page` wired into the composed catalog — the
//! ref-boundary tripwire for the no-pages-under-non-pages ruling (Fork B B1,
//! R8). A non-seed page's structural ancestors up to a root must all be pages.
//!
//! `Needs RefBlockTree` only (no SUT cap): it asserts the *generator/seed*
//! guarantee, not a SUT⇄ref differential. The ref always provides
//! `RefBlockTree`, so it selects on every composed draw and RED-catches any
//! future generator/seed change that reintroduces the prohibited topology —
//! before `DocumentManager::name_chain` `bail!`s deep in writeback. Vacuously
//! green on a ref modeling no non-seed page (the default keystone).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::no_page_under_non_page::InvNoPageUnderNonPage;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvNoPageUnderNonPage,
        RunMode::Strict,
        Needs {
            sut_present: Vec::new(),
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBlockTree>()],
        },
        Attribution::at(Layer::StoreCrdt, file!()),
    ))
}
