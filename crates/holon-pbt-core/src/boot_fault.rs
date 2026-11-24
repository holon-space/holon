//! The keystone's "fault at boot" axis: a per-case draw beside the wiring,
//! shrinking toward [`BootFault::None`]. Each variant is a fault the app must
//! survive and disclose; a later increment adds its own.

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BootFault {
    None,
    /// The previous run panicked: its panic record is in the config dir
    /// before the app starts.
    PriorCrash(PriorPanic),
    /// A task spawned after the app started panics.
    TaskPanic,
    /// The vault directory does not exist when the app starts, as on a fresh
    /// mobile install: the platform hands over a data dir, not a vault.
    MissingVaultDir,
}

/// The panic record a [`BootFault::PriorCrash`] leaves behind.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
        1 => Just(BootFault::MissingVaultDir),
    ]
    .boxed()
}

/// A boot step that fails from data or the environment. The app must boot
/// without that part and name the step and its error in the boot report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BootStepFault {
    /// The integration-state mirror's first projection fails.
    IntegrationProjection,
    /// Subscribing the rule and action discovery fails.
    ActionWatchers,
    /// Seeding the default layout fails.
    LayoutSeed,
    /// The Loro sync controller's factory returns an error.
    SyncController,
    /// The MCP integration supervisor's factory returns an error.
    IntegrationSupervisor,
    /// The OS refuses a file watcher, as at its watcher limit.
    VaultWatcher,
    /// The OS builds the file watcher but refuses to watch the vault.
    VaultWatchRegistration,
}

impl BootStepFault {
    pub const ALL: [BootStepFault; 7] = [
        Self::IntegrationProjection,
        Self::ActionWatchers,
        Self::LayoutSeed,
        Self::SyncController,
        Self::IntegrationSupervisor,
        Self::VaultWatcher,
        Self::VaultWatchRegistration,
    ];

    /// The fault fires in the OS file watcher, which a session reaches only
    /// over the production change source.
    pub fn fires_in_os_watcher(self) -> bool {
        matches!(self, Self::VaultWatcher | Self::VaultWatchRegistration)
    }
}

pub fn any_boot_step_fault() -> BoxedStrategy<BootStepFault> {
    proptest::sample::select(BootStepFault::ALL.to_vec()).boxed()
}
