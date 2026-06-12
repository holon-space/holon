//! `inv-viewmodel-decompiled-rows-match-query` — the rendered rows decompile back
//! to the reference's query/render-expr. `Needs SutRenderer + RefRender`. The ref
//! side is the production `ReferenceState`; selection ANDs the SUT and ref cap
//! sets, so it only fires where a real renderer slice is wired (the frontend slice).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefRender, SutRenderer};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::viewmodel_decompiled_rows_match_query::InvViewmodelDecompiledRowsMatchQuery;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewmodelDecompiledRowsMatchQuery,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefRender>()],
        },
    ))
}
