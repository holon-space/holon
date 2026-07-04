//! `inv-journal-one-per-day` — the production journal auto-create rule yields
//! exactly one journal date-page per calendar day, and one for every day the
//! clock has visited. `Needs SutSqlProjection + RefClock`. Selection ANDs the
//! SUT and ref cap sets, so it fires only where a real SQL projection AND the
//! reference clock model are both present — the frontend+Turso slices that host
//! `SutClockAdvance` and can actually drive `AdvanceDay`.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefClock;
use holon_pbt_core::capabilities::SutClockAdvance;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::journal_one_per_day::InvJournalOnePerDay;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvJournalOnePerDay,
        RunMode::Strict,
        Needs {
            // `SutClockAdvance` gates selection to the frontend+Turso arm that
            // hosts the controllable clock and fires the journal auto-create rule
            // live (env `HOLON_PBT_ADVANCE_DAY`); without it a Turso-storage slice
            // that has `SutSqlProjection` but never fires the rule would false-RED
            // P2 (boot day expected, no journal page). `SutSqlProjection` supplies
            // the journal date-page read the body performs.
            sut_present: vec![
                CapId::of::<dyn SutSqlProjection>(),
                CapId::of::<dyn SutClockAdvance>(),
            ],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefClock>()],
        },
        Attribution::at(Layer::Projection, file!()),
    ))
}
