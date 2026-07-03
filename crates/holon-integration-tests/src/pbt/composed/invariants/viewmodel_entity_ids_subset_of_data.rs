//! `inv-viewmodel-entity-ids-subset-of-data` — the entity ids referenced by the
//! rendered tree are a subset of the data rows. `Needs SutRenderer + RefViewSelection +
//! RefLayout`. The ref side is the production `ReferenceState`; selection ANDs the
//! SUT and ref cap sets, so it only fires where a real renderer slice is wired
//! (the frontend slice).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::{RefLayout, RefViewSelection, SutRenderer};
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::viewmodel_entity_ids_subset_of_data::InvViewmodelEntityIdsSubsetOfData;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvViewmodelEntityIdsSubsetOfData,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutRenderer>()],
            sut_absent: Vec::new(),
            ref_present: vec![
                CapId::of::<dyn RefViewSelection>(),
                CapId::of::<dyn RefLayout>(),
            ],
        },
    ))
}
