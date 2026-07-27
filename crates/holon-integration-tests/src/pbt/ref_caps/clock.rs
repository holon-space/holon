//! `RefClock` / `RefClockMut` (ADR 0024 §6 AdvanceDay).
//!
//! @pbt kind ref
//! @pbt covers journal-count — thin delegate to `ClockState`: current day +
//!   `visited_days` cardinality = predicted journal-block count (one per
//!   distinct day). Boot day agrees with the SUT by construction (shared
//!   `KEYSTONE_CLOCK_BOOT_MS`), no read-back side channel.

use holon_pbt_core::capabilities::RefClock;
use holon_pbt_core::capabilities::RefClockMut;

use super::super::reference_state::ReferenceState;

impl RefClock for ReferenceState {
    fn today(&self) -> String {
        self.clock.today.clone()
    }
    fn expected_journal_day_count(&self) -> usize {
        self.clock.visited_days.len()
    }
    fn visited_days(&self) -> std::collections::BTreeSet<String> {
        self.clock.visited_days.clone()
    }
}

impl RefClockMut for ReferenceState {
    fn advance_day(&mut self, days: i64) {
        self.clock.advance_day(days);
        // Model the journal day-block the production `journals_auto_create` rule
        // fires on the day-rollover CDC (ADR 0024 §6), mirroring the boot
        // journal, so the composed per-tick reconcile + block-tree invariants
        // account for the SUT's rule-minted block. Only reached when `AdvanceDay`
        // is applied, i.e. a frontend+Turso wiring that fires the rule live
        // (`SutClockAdvance` gated by `HOLON_PBT_ADVANCE_DAY`). `seed_journal_day`
        // is idempotent, so a same-day re-tick (`days == 0`) is a no-op. Sequence
        // strictly increases per distinct day so the ref sibling order matches
        // the SUT's created-last fractional append.
        use holon_orgmode::models::OrgBlockExt;
        let date = self.clock.today.clone();
        // Sibling-order choice (a), empirically matched to the SUT: the
        // production journal action APPENDS the new day-block LAST among
        // `block:journals`' children (its Loro fractional index sorts after all
        // existing siblings — heading, rule source blocks, boot journal, and any
        // earlier rollover). Reproduce that by stamping a `sequence` strictly
        // greater than every current child's, so the reference's `(sequence, id)`
        // sort lands it last. `seed_journal_day` is idempotent, so a same-day
        // re-tick (`days == 0`) re-computes but inserts nothing.
        let journals = holon_pbt_core::capabilities::EntityUri::block("journals");
        let next_seq = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == journals)
            .map(|b| b.sequence())
            .max()
            .unwrap_or(0)
            + 1;
        crate::pbt::composed::wide_e2e::seed_journal_day(self, &date, next_seq);
    }
}
