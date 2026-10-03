//! `inv-no-database-stuck` — no SQL command keeps the actor busy past the hang
//! bound (`HOLON_ACTOR_HANG_MS`), in any session a PBT boots.
//!
//! @pbt oracle internal-consistency — the session's `ConditionBus` never
//!   carries `DatabaseStuck` (no ref)
//! @pbt covers stuck SQL actor — a command that spins or blocks inside the
//!   engine, which freezes every later read and write of the session
//! @pbt slips-if-removed a transition whose write spins the IVM commit hangs
//!   the run with no output until the outer test timeout
//!
//! A stuck command never returns, so the step that sent it never reaches an
//! invariant check. The guard is a thread of its own that ends the process with
//! the report.

use std::sync::Arc;
use std::sync::Weak;

use holon_api::ConditionBus;
use holon_api::ConditionChange;
use holon_api::ConditionKind;
use holon_pbt_core::invariant::InvariantId;
use tokio::sync::broadcast::error::RecvError;

pub const ID: InvariantId = InvariantId("inv-no-database-stuck");

/// Ends the process with exit code 101 when `bus` raises `DatabaseStuck`.
pub fn forbid_database_stuck(bus: &Arc<ConditionBus>) {
    let mut changes = bus.subscribe().changes;
    let bus = Arc::downgrade(bus);
    std::thread::Builder::new()
        .name("database-stuck-guard".into())
        .spawn(move || {
            loop {
                match changes.blocking_recv() {
                    Ok(ConditionChange::Raised(c)) => fail_if_stuck(&c.reason),
                    Ok(ConditionChange::Cleared(_)) => {}
                    Err(RecvError::Lagged(_)) => check_current(&bus),
                    Err(RecvError::Closed) => return,
                }
            }
        })
        .expect("spawn the database-stuck guard thread");
}

fn check_current(bus: &Weak<ConditionBus>) {
    if let Some(bus) = bus.upgrade() {
        for c in bus.current() {
            fail_if_stuck(&c.reason);
        }
    }
}

fn fail_if_stuck(reason: &ConditionKind) {
    if let ConditionKind::DatabaseStuck {
        command,
        running_secs,
        report,
    } = reason
    {
        eprintln!(
            "[{}] a `{command}` SQL command has kept the actor busy for {running_secs} s, past \
             the hang bound. The step that sent it cannot return.\n{report}",
            ID.0
        );
        std::process::exit(101);
    }
}
