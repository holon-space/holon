//! `inv-live-children-match-ref` — the live children projection (SQL + Loro)
//! agrees with the reference's block tree. `Needs SutSqlProjection + SutLoroLog
//! + RefBlockTree`. Selection ANDs the SUT and ref cap sets, so it fires where
//! both the SQL and Loro read caps are present — the combined SQL+Loro /
//! frontend slices.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutLoroLog;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::live_children_match_ref::InvLiveChildrenMatchRef;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvLiveChildrenMatchRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![
                CapId::of::<dyn SutSqlProjection>(),
                CapId::of::<dyn SutLoroLog>(),
            ],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBlockTree>()],
        },
    ))
}
