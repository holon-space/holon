//! `inv-viewmodel-shows-source-when-no-query` — the degraded ("shows source")
//! render twin. The first real negative-selection (`sut_absent`) consumer.
//!
//! `Needs { sut_present: [SutRenderer], sut_absent: [SutQueryResults], ref_present: [] }`
//! — selected ONLY where a renderer is wired WITHOUT a query engine (the
//! `block_query_degraded` builder over a no-Turso block-query frontend), and
//! deselected (disclosed) wherever the full-mode `SutQueryResults` is present. Its
//! body reads no ref caps (see §5.2 soundness in the body module): the degradation
//! is a property of the SUT's wiring, not the cap-blind reference.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{SutQueryResults, SutRenderer};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::viewmodel_shows_source_when_no_query::InvViewmodelShowsSourceWhenNoQuery;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewmodelShowsSourceWhenNoQuery,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: vec![CapId::of::<dyn SutQueryResults>()],
            ref_present: Vec::new(),
        },
    ))
}
