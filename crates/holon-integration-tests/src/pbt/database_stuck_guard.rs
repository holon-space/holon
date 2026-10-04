//! `inv-no-database-stuck` — no SQL command keeps the actor busy past the hang
//! bound (`HOLON_ACTOR_HANG_MS`), in any session a PBT boots, and the watch
//! that reports it keeps running.
//!
//! @pbt oracle internal-consistency — the session's `ConditionBus` never
//!   carries `DatabaseStuck` or `DatabaseWatchFailed` (no ref)
//! @pbt covers stuck SQL actor — a command that spins or blocks inside the
//!   engine, which freezes every later read and write of the session, and a
//!   failed watch that can no longer report one
//! @pbt slips-if-removed a transition whose write spins the IVM commit hangs
//!   the run with no output until the outer test timeout
//!
//! A stuck command never returns, so the step that sent it never reaches an
//! invariant check, and a failed watch can no longer report one. The guard is a
//! thread of its own that ends the process with the report.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;

use holon_api::ConditionBus;
use holon_api::ConditionChange;
use holon_api::ConditionKind;
use holon_pbt_core::invariant::InvariantId;
use tokio::sync::broadcast::error::RecvError;

pub const ID: InvariantId = InvariantId("inv-no-database-stuck");

static GUARDED: Mutex<Vec<Weak<ConditionBus>>> = Mutex::new(Vec::new());

/// Ends the process with exit code 101 when `bus` raises `DatabaseStuck` or
/// `DatabaseWatchFailed`.
pub fn forbid_database_stuck(bus: &Arc<ConditionBus>) {
    GUARDED
        .lock()
        .expect("guarded buses poisoned")
        .push(Arc::downgrade(bus));
    let mut changes = bus.subscribe().changes;
    let bus = Arc::downgrade(bus);
    std::thread::Builder::new()
        .name("database-stuck-guard".into())
        .spawn(move || {
            loop {
                match changes.blocking_recv() {
                    Ok(ConditionChange::Raised(c)) => fail_if_database_broken(&c.reason),
                    Ok(ConditionChange::Cleared(_)) => {}
                    Err(RecvError::Lagged(_)) => check_current(&bus),
                    Err(RecvError::Closed) => return,
                }
            }
        })
        .expect("spawn the database-stuck guard thread");
}

/// Guards the container's bus. Called from the first DI closure, so the boot's
/// own commands are guarded too.
pub fn forbid_database_stuck_in(injector: &fluxdi::Injector) {
    forbid_database_stuck(&bus_of(injector));
}

/// Panics unless a guard watches the container's bus.
pub fn assert_guarded_in(injector: &fluxdi::Injector) {
    assert_guarded(&bus_of(injector));
}

fn bus_of(injector: &fluxdi::Injector) -> Arc<ConditionBus> {
    (*injector
        .try_resolve::<Arc<ConditionBus>>()
        .expect("every container provides a ConditionBus"))
    .clone()
}

fn assert_guarded(bus: &Arc<ConditionBus>) {
    let guarded = GUARDED
        .lock()
        .expect("guarded buses poisoned")
        .iter()
        .any(|g| std::ptr::eq(g.as_ptr(), Arc::as_ptr(bus)));
    assert!(
        guarded,
        "[{}] no guard watches the session's ConditionBus: a stuck SQL command would hang the \
         run with no output",
        ID.0
    );
}

fn check_current(bus: &Weak<ConditionBus>) {
    if let Some(bus) = bus.upgrade() {
        for c in bus.current() {
            fail_if_database_broken(&c.reason);
        }
    }
}

fn fail_if_database_broken(reason: &ConditionKind) {
    match reason {
        ConditionKind::DatabaseStuck {
            command,
            running_secs,
            report,
        } => {
            eprintln!(
                "[{}] a `{command}` SQL command has kept the actor busy for {running_secs} s, \
                 past the hang bound. The step that sent it cannot return.\n{report}",
                ID.0
            );
            std::process::exit(101);
        }
        ConditionKind::DatabaseWatchFailed { cause } => {
            eprintln!("[{}] the SQL actor watch failed: {cause}", ID.0);
            std::process::exit(101);
        }
        _ => {}
    }
}
