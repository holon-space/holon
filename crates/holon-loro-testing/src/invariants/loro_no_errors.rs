//! `inv-loro-no-errors` — `SutLoroLog` only, ignores the reference, so it runs
//! whenever a Loro store is wired. Asserts the LoroSyncController logged no
//! error since startup. Inert-but-honest in the pure-Loro slice (a standalone
//! CRDT has no sync controller, so `loro_had_errors` is structurally `false`);
//! its real teeth run in the ONE PBT (full mode), where `compose_sut` backs
//! `SutLoroLog` with the live `LoroSyncControllerHandle` error counter.
//!
//! @pbt oracle internal-consistency
//! @pbt covers loro-sync-error — LoroSyncController dropped/failed-to-apply
//!   events (a gap in the SQL→Loro mirror), checked on the SUT alone
//! @pbt slips-if-removed an outdent/indent/split whose new parent is not yet a
//!   TreeID makes the controller log `Cannot resolve parent URI` and silently
//!   drop the event; the Loro tree diverges from SQL with no failing check to
//!   surface it

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutLoroLog;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvLoroNoErrors;

impl InvLoroNoErrors {
    pub const ID: InvariantId = InvariantId("inv-loro-no-errors");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvLoroNoErrors
where
    S: SutLoroLog,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        if sut.loro_had_errors().await {
            InvariantResult::Fail(
                "[inv-loro-no-errors] LoroSyncController logged error(s). Search captured logs \
                 for `[LoroSyncController] Failed to apply` to find which event(s) the SQL→Loro \
                 mirror dropped (e.g. `Cannot resolve parent URI to TreeID: block:UUID` for \
                 outdent/indent/split where the new parent isn't yet a TreeID in the Loro tree)."
                    .to_string(),
            )
        } else {
            InvariantResult::Ok
        }
    }
}

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvLoroNoErrors,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutLoroLog>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}
