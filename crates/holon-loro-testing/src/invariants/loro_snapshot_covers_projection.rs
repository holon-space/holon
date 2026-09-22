//! `inv-loro-snapshot-covers-projection` — the `.loro` snapshot on disk holds
//! every Loro change the projection has already written to SQL.
//!
//! @pbt oracle internal-consistency
//! @pbt covers loro-durability — a Loro write that reached SQL but not the
//!   on-disk snapshot, so a crash or quit rolls Loro back behind SQL
//! @pbt slips-if-removed an org-ingest write lands in Loro and SQL and is never
//!   saved; the next boot reloads the older snapshot and drops the block from
//!   the authority while SQL and the org file still carry it

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutLoroDurability;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvLoroSnapshotCoversProjection;

impl InvLoroSnapshotCoversProjection {
    pub const ID: InvariantId = InvariantId("inv-loro-snapshot-covers-projection");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvLoroSnapshotCoversProjection
where
    S: SutLoroDurability,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        match sut.loro_snapshot_lag().await {
            None => InvariantResult::Ok,
            Some(lag) => InvariantResult::Fail(format!(
                "[inv-loro-snapshot-covers-projection] SQL reflects Loro changes the on-disk \
                 snapshot does not hold, so a crash or quit now loses them from the authority: \
                 {lag}"
            )),
        }
    }
}

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvLoroSnapshotCoversProjection,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutLoroDurability>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::at(Layer::StoreCrdt, file!()),
    ))
}
