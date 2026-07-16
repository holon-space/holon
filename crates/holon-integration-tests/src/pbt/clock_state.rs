//! Reference-side calendar-clock model (ADR 0024 §6). Extracted from
//! `reference_state.rs`.
//!
//! @pbt kind ref
//! @pbt covers journal-count — `visited_days` cardinality predicts the
//!   journal-rule output (one block per distinct day; same-day re-tick
//!   idempotent). Boot day agrees with the SUT `TestClock` by construction via
//!   the shared `KEYSTONE_CLOCK_BOOT_MS` constant — no read-back side channel.

/// The fixed wall-clock instant the composed frontend boot injects (local noon
/// on 2026-01-15, timezone-robust). Both the SUT's `TestClock` and the
/// reference model derive the boot day from this, so they agree by construction
/// without a side channel.
pub const KEYSTONE_CLOCK_BOOT_MS: i64 = 1_768_478_400_000; // 2026-01-15T12:00:00Z

/// Reference-side calendar-clock model (ADR 0024 §6). Tracks the model's
/// current day and the set of distinct days the clock has visited, from which
/// it predicts the journal-rule output: exactly one journal block per distinct
/// day.
#[derive(Debug, Clone)]
pub struct ClockState {
    /// The model's current local calendar day, `YYYY-MM-DD`.
    pub today: String,
    /// Every distinct day the clock has been at (the boot day plus each
    /// advanced-to day). Its cardinality is the predicted journal count;
    /// same-day re-ticks are idempotent (set insert is a no-op).
    pub visited_days: std::collections::BTreeSet<String>,
}

impl ClockState {
    /// Seed from the fixed boot instant, so the boot day is already visited
    /// (the rule fires once at boot).
    pub fn new() -> Self {
        let boot =
            holon_api::CalendarDate::from_clock(&holon_api::TestClock::new(KEYSTONE_CLOCK_BOOT_MS))
                .ymd();
        let mut visited_days = std::collections::BTreeSet::new();
        visited_days.insert(boot.clone());
        Self {
            today: boot,
            visited_days,
        }
    }

    /// Advance the model day by `days` calendar days and record the new day.
    /// `days == 0` re-ticks the same day (idempotent).
    pub fn advance_day(&mut self, days: i64) {
        let next = holon_api::CalendarDate::parse(&self.today)
            .expect("clock today is a valid YYYY-MM-DD")
            .add_days(days);
        self.today = next.ymd();
        self.visited_days.insert(self.today.clone());
    }
}

impl Default for ClockState {
    fn default() -> Self {
        Self::new()
    }
}
