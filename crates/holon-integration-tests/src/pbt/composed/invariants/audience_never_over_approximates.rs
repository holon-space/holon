//! `inv-audience-never-over-approximates` wired into the composed catalog — the
//! ADR 0028 C2/H3 directional-alignment ref-boundary tripwire.
//!
//! `Needs RefAudience` only (no SUT cap): it asserts the share-migration
//! guarantee, not a SUT⇄ref differential (no share machinery exists in prod
//! yet). The ref always provides `RefAudience`, so it selects on every composed
//! draw and RED-catches any migration model that leaves a block observably in a
//! container whose policy no longer covers it. Vacuously green on a ref
//! modeling no crossings (`shared_block_ids()` empty — the default keystone),
//! so it adds no false RED.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefAudience;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::audience_never_over_approximates::InvAudienceNeverOverApproximates;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvAudienceNeverOverApproximates,
        RunMode::Strict,
        Needs {
            sut_present: Vec::new(),
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefAudience>()],
        },
    ))
}
