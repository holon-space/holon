//! `inv-read-only-home-refuses-writes` — a read-only home is a write BOUNDARY.
//!
//! @pbt oracle internal-consistency — the store's read-only-homed blocks still
//!   hold what the ingest wrote, and every write aimed at one was refused and
//!   disclosed
//! @pbt covers read-only-home-write-boundary — a store-origin write against a
//!   block whose document is homed in a `WriteTier::ReadOnly` file
//! @pbt slips-if-removed a write to a `.cook`-homed block lands in `block_raw`
//!   with no writer able to put it on disk, so the store says one thing and the
//!   authoritative file another, and the user is never told
//!
//! Holon ships no writer for a read-only format, so its file is INPUT: the
//! store may project it and never author it. An accepted edit is therefore not
//! a small loss — it is a permanent, invisible divergence between the store and
//! the file the user still edits by hand.
//!
//! The read-only set is not re-derived here. The SUT cap asks the production
//! [`holon_core::WriteTierAuthority`] — the object the dispatcher itself
//! consults — so a gate that stopped classifying anything shows up as a
//! declared block that production no longer refuses, not as a quiet pass.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::RefReadOnlyHomes;
use holon_pbt_core::capabilities::SutReadOnlyHomes;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvReadOnlyHomeRefusesWrites;

impl InvReadOnlyHomeRefusesWrites {
    pub const ID: InvariantId = InvariantId("inv-read-only-home-refuses-writes");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvReadOnlyHomeRefusesWrites
where
    R: RefReadOnlyHomes,
    S: SutReadOnlyHomes,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, reference: &R, sut: &S) -> InvariantResult {
        let declared = reference.read_only_homed_blocks();
        if declared.is_empty() {
            return InvariantResult::Ok;
        }

        let at_ingest = sut.read_only_blocks_at_ingest().await;
        let classified: BTreeSet<&str> = at_ingest.iter().map(|(id, _)| id.as_str()).collect();
        let unclassified: Vec<String> = declared
            .iter()
            .filter(|id| !classified.contains(id.as_str()))
            .map(|id| id.to_string())
            .collect();
        if !unclassified.is_empty() {
            return InvariantResult::Fail(format!(
                "{} block(s) the fixture homed in a read-only format are NOT classified read-only \
                 by the production WriteTierAuthority: {:?}. Every write to them would be accepted \
                 — the gate is disarmed, not merely untested. Classified: {:?}",
                unclassified.len(),
                unclassified,
                classified,
            ));
        }

        let now = sut.read_only_blocks_now().await;
        if let Some((id, ingested, stored)) = first_divergence(&at_ingest, &now) {
            return InvariantResult::Fail(format!(
                "block `{id}` is homed in a read-only file, yet `block_raw` no longer holds what \
                 the ingest wrote: ingested {ingested:?}, stored {stored:?}. A write reached the \
                 store that the disk can never take."
            ));
        }

        let (attempts, refusals) = sut.read_only_write_attempts().await;
        if attempts == 0 {
            return InvariantResult::Ok;
        }
        if refusals != attempts {
            return InvariantResult::Fail(format!(
                "{attempts} write(s) were aimed at a read-only-homed block but only {refusals} \
                 were refused. An accepted write left the store holding text no writer can put on \
                 disk."
            ));
        }
        let raised = sut.raised_degraded_conditions().await;
        let kind = holon_loro::ShareDegradedReason::EDIT_REFUSED_READ_ONLY_FORMAT;
        if !raised.iter().any(|c| c == kind) {
            return InvariantResult::Fail(format!(
                "{attempts} write(s) were refused for a read-only home, but `{kind}` was never \
                 raised on the degraded bus — the refusal is silent, so the user's edit vanishes \
                 with no disclosure. Raised: {raised:?}"
            ));
        }
        InvariantResult::Ok
    }
}

/// The first block whose stored content left its ingested value, or a block
/// that vanished from the read-only set entirely.
fn first_divergence<'a>(
    at_ingest: &'a [(String, String)],
    now: &'a [(String, String)],
) -> Option<(&'a str, &'a str, String)> {
    at_ingest.iter().find_map(|(id, ingested)| {
        let stored = now.iter().find(|(other, _)| other == id).map(|(_, c)| c);
        match stored {
            Some(stored) if stored == ingested => None,
            Some(stored) => Some((id.as_str(), ingested.as_str(), stored.clone())),
            None => Some((
                id.as_str(),
                ingested.as_str(),
                "<gone from the read-only set>".to_string(),
            )),
        }
    })
}
