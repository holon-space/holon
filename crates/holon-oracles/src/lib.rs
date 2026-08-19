//! @c4 component
//! @c4 layer Core
//! Pattern: Monitor
//! @c4 uses holon-api "shared value & operation types" "Rust"
//!
//! Live oracles — keystone PBT invariants shipped into debug builds.
//!
//! The composed keystone PBT carries ~37 invariants, but they only run in
//! tests, while most escaped bugs are ENVIRONMENT bugs whose home field is
//! prod. This crate ships the *ref-less* subset (checkable against live app
//! state alone, no PBT reference model) as background assertions in debug
//! builds, so every manual dogfood session is an oracle-carrying session.
//!
//! Architecture (fail loud, never fake — see CLAUDE.md):
//! - [`checks`] — pure check functions, the single shared implementation. The
//!   keystone PBT bodies delegate to these; the live runner feeds them from SQL
//!   snapshots. One implementation, no drift.
//! - [`status`] — process-global violation ledger + change notification. Global
//!   because the tracing subscriber (the latency oracle's source) is
//!   process-global; the PBT harness never touches it.
//! - [`runner`] — background tokio task running the cheap tier on a fixed
//!   cadence, off the UI hot path.
//! - [`latency`] — a `tracing_subscriber` Layer that turns the existing
//!   `holon_latency` stage events (dispatch/rows/projection) into SLO
//!   violations (any stage >200ms).
//!
//! Gating: debug builds only (the frontends wire it under
//! `cfg(debug_assertions)`), `HOLON_ORACLES=off` opts out, heavier checks are
//! reserved for `HOLON_ORACLES=full`.

pub mod checks;
pub mod latency;
pub mod runner;
pub mod status;

/// Which oracle tier runs, from `HOLON_ORACLES`.
///
/// Unset / `on` / `cheap` → `Cheap` (default ON in debug builds).
/// `off` → `Off`. `full` → `Full` (cheap tier + heavy checks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OracleMode {
    Off,
    Cheap,
    Full,
}

impl OracleMode {
    pub fn from_env() -> Self {
        match std::env::var("HOLON_ORACLES").as_deref() {
            Ok("off") | Ok("0") => OracleMode::Off,
            Ok("full") => OracleMode::Full,
            Ok("") | Ok("on") | Ok("cheap") | Err(_) => OracleMode::Cheap,
            Ok(other) => panic!("HOLON_ORACLES must be one of off|cheap|on|full, got '{other}'"),
        }
    }

    pub fn enabled(self) -> bool {
        self != OracleMode::Off
    }
}
