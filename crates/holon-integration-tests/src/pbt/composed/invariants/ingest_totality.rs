//! `inv-ingest-totality` wired into the composed catalog — the SUBSET (ref ⊆
//! SUT) twin of `inv-block-ids-match-ref`. `Needs SutSqlProjection +
//! RefBlockTree`, so it selects wherever a SQL projection is wired (the
//! frontend / Turso draws) and deselects on a Loro-only draw (ingest totality
//! is a storage-projection property). Modelled on the sibling `no_orphan`
//! bridge; asserts every non-seed reference block landed in the projection,
//! firing LOUDLY as an ingest-data-loss violation.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::ingest_totality::InvIngestTotality;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvIngestTotality,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutSqlProjection>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBlockTree>()],
        },
    ))
}
