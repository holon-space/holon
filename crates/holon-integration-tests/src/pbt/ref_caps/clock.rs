//! `RefClock` / `RefClockMut` (ADR 0024 §6 AdvanceDay).

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
}

impl RefClockMut for ReferenceState {
    fn advance_day(&mut self, days: i64) {
        self.clock.advance_day(days);
    }
}
