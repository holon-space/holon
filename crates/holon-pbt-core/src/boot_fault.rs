//! The keystone's "fault at boot" axis: a per-case draw beside the wiring,
//! shrinking toward [`BootFault::None`]. Each variant is a fault the app must
//! survive and disclose; a later increment adds its own.

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BootFault {
    None,
    /// The previous run panicked: its panic record is in the config dir
    /// before the app starts.
    PriorCrash(PriorPanic),
    /// A task spawned after the app started panics.
    TaskPanic,
}

/// The panic record a [`BootFault::PriorCrash`] leaves behind.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PriorPanic {
    pub message: String,
    /// `file:line:column`, as a panic location reads.
    pub location: String,
    pub thread: String,
}

fn any_prior_panic() -> BoxedStrategy<PriorPanic> {
    (
        "[a-z][a-z ]{0,30}",
        "[a-z_]{1,10}",
        1u32..5000,
        1u32..120,
        "[a-z-]{1,12}",
    )
        .prop_map(|(message, file, line, column, thread)| PriorPanic {
            message,
            location: format!("crates/holon/src/{file}.rs:{line}:{column}"),
            thread,
        })
        .boxed()
}

pub fn any_boot_fault() -> BoxedStrategy<BootFault> {
    prop_oneof![
        6 => Just(BootFault::None),
        1 => any_prior_panic().prop_map(BootFault::PriorCrash),
        1 => Just(BootFault::TaskPanic),
    ]
    .boxed()
}
