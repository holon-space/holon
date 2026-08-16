//! Platform capabilities and boot-step disclosure.
//!
//! Two boot paths assemble a Holon session: the shared DI wiring
//! (`holon_app::wiring::add_frontend`, every native frontend) and the
//! hand-mirrored browser-worker assembly (`holon-worker`'s
//! `build_engine_state`). The worker exists because `holon-app` is not
//! wasm-buildable; until that dependency split lands, the two paths drift, and
//! every drift so far degraded SILENTLY — an omitted step announces itself only
//! as a runtime failure someone eventually debugs.
//!
//! This module makes the divergence a log line. A boot path records each step
//! it performs on a [`BootDisclosure`]; [`BootDisclosure::finish`] then reports
//! every step that never ran, separating the three reasons one can be missing:
//!
//! - the platform cannot host it ([`SkipReason::MissingCapability`], `warn!`) —
//!   a disclosed degradation, which is what CLAUDE.md's error-handling policy
//!   asks for;
//! - this session is not configured for it ([`SkipReason::ConfigAbsent`],
//!   `info!`) — expected and quiet at warn level, but never invisible;
//! - nothing stopped it ([`SkipReason::NoPlatformReason`], `warn!`) — drift
//!   between the two paths, i.e. a bug waiting to be found.
//!
//! **`finish()` cannot be forgotten.** It CONSUMES the disclosure and returns a
//! [`BootReport`] that `SessionParts` requires, so a boot path that skips the
//! disclosure does not compile. An earlier design returned the report through a
//! query method that answered "empty" both when nothing was skipped and when
//! the terminator was never called — deleting the `finish()` call left every
//! gate green while disarming the whole ledger.
//!
//! `finish` both logs and returns the list, so tests assert on the returned
//! value and need no tracing subscriber (the wasm log sink routes every level
//! to `console.error`, so a level-based assertion would be meaningless anyway).

use std::sync::Arc;
use std::sync::Mutex;

/// A platform facility a boot step can need.
///
/// Only facilities some boot step actually gates on appear here. Notably absent
/// is a "background tasks" capability: the worker's current-thread runtime IS a
/// real constraint, but no boot step turned out to require detached execution —
/// the one candidate (`PostReady`) has an inline `await` arm and runs fine
/// without it. An unused capability is a fake excuse waiting to be handed out.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PlatformCapability {
    /// A host filesystem reachable through `tokio::fs` and `notify` watches.
    /// OPFS does not count: the org layer cannot watch or traverse it.
    Filesystem,
    /// Spawning child processes (MCP integration servers, post-write hooks).
    ProcessSpawn,
}

impl PlatformCapability {
    pub fn label(self) -> &'static str {
        match self {
            Self::Filesystem => "filesystem",
            Self::ProcessSpawn => "process-spawn",
        }
    }
}

impl std::fmt::Display for PlatformCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// What a platform offers. Constructed only from the named shapes below, so a
/// caller cannot invent a set that no real target has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlatformCapabilities(&'static [PlatformCapability]);

impl PlatformCapabilities {
    /// Desktop / CLI / test hosts: everything.
    pub const NATIVE: Self = Self(&[
        PlatformCapability::Filesystem,
        PlatformCapability::ProcessSpawn,
    ]);

    /// `holon-worker` on `wasm32-wasip1-threads`: neither. Storage is an OPFS
    /// shim the org layer cannot use, and there are no processes.
    pub const BROWSER_WORKER: Self = Self(&[]);

    /// The capabilities of the target this code is compiled for.
    ///
    /// A boot path that IS the browser worker names [`Self::BROWSER_WORKER`]
    /// directly instead — the worker keeps the same reduced shape when it is
    /// compiled natively for a `cargo check`.
    pub const fn current() -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            Self::BROWSER_WORKER
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::NATIVE
        }
    }

    pub fn has(self, capability: PlatformCapability) -> bool {
        self.0.contains(&capability)
    }
}

/// A step of the session boot sequence that at least one boot path skips.
///
/// Steps both paths always perform are deliberately absent — this enum exists
/// to name divergences, not to mirror the wiring.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BootStep {
    /// Model.md invariant 10: reject a vault consolidated under another mode.
    ConsolidatorEpochGuard,
    /// The one channel through which a frontend learns something degraded.
    DegradedSignalBus,
    /// Tells the frontend that edits stopped reaching disk. Needs no
    /// filesystem itself — it is a plain provider that emits on the bus.
    WritebackDisclosure,
    /// `ThemeRegistry` + `PreferenceDef`s — pure data, no platform need.
    ThemeAndPreferences,
    /// `UiInfo` (window/viewport description) into DI.
    UiInfo,
    /// The org vault: parser, file-sync controller, watcher.
    OrgModeIngest,
    /// External MCP providers configured from `{config_dir}/integrations`.
    McpIntegrations,
    /// Registering MCP-backed FDW tables + the matview hook on the engine.
    McpFdwTables,
    /// `PublishErrorTracker`, the session's publish-failure record.
    PublishErrorTracker,
    /// The org initial-scan readiness signal handed to the session.
    FileWatcherReadySignal,
    /// `DbHandle::transition_to_ready` — the actor stays in boot mode without
    /// it.
    TransitionDbToReady,
    /// Seeding the bundled default layout so the shell has something to render.
    SeedDefaultLayout,
    /// Rule/action discovery. Without it seeded rules never fire.
    StartActionWatchers,
    /// A registry-backed `LinkTargetClassifier`; its absence leaves built-in
    /// schemes only, so links to registered entity types stop resolving.
    RegistryLinkClassifier,
    /// Post-readiness work: await the org scan, open the `SyncGate`, resolve
    /// the Loro sync controller. Its `wait_for_ready` arm awaits inline, so
    /// it needs no detached execution; on the worker its body would be
    /// largely vacuous (no scan signal, no Loro controller), but "vacuous"
    /// is not "impossible", so skipping it reads as drift rather than as a
    /// platform excuse.
    PostReady,
}

impl BootStep {
    pub const ALL: [BootStep; 15] = [
        Self::ConsolidatorEpochGuard,
        Self::DegradedSignalBus,
        Self::WritebackDisclosure,
        Self::ThemeAndPreferences,
        Self::UiInfo,
        Self::OrgModeIngest,
        Self::McpIntegrations,
        Self::McpFdwTables,
        Self::PublishErrorTracker,
        Self::FileWatcherReadySignal,
        Self::TransitionDbToReady,
        Self::SeedDefaultLayout,
        Self::StartActionWatchers,
        Self::RegistryLinkClassifier,
        Self::PostReady,
    ];

    /// The capability without which this step cannot run ANYWHERE. `None` means
    /// the step is portable — so skipping it is drift, never degradation.
    ///
    /// The bar is "cannot run", not "would not do anything useful". A step
    /// whose body is merely empty on some platform still returns `None`:
    /// handing it a capability excuse would silence a real divergence on
    /// content grounds.
    pub fn requires(self) -> Option<PlatformCapability> {
        match self {
            Self::ConsolidatorEpochGuard | Self::OrgModeIngest | Self::FileWatcherReadySignal => {
                Some(PlatformCapability::Filesystem)
            }
            Self::McpIntegrations | Self::McpFdwTables => Some(PlatformCapability::ProcessSpawn),
            Self::DegradedSignalBus
            | Self::WritebackDisclosure
            | Self::ThemeAndPreferences
            | Self::UiInfo
            | Self::PublishErrorTracker
            | Self::TransitionDbToReady
            | Self::SeedDefaultLayout
            | Self::StartActionWatchers
            | Self::RegistryLinkClassifier
            | Self::PostReady => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ConsolidatorEpochGuard => "consolidator-epoch-guard",
            Self::DegradedSignalBus => "degraded-signal-bus",
            Self::WritebackDisclosure => "writeback-disclosure",
            Self::ThemeAndPreferences => "theme-and-preferences",
            Self::UiInfo => "ui-info",
            Self::OrgModeIngest => "orgmode-ingest",
            Self::McpIntegrations => "mcp-integrations",
            Self::McpFdwTables => "mcp-fdw-tables",
            Self::PublishErrorTracker => "publish-error-tracker",
            Self::FileWatcherReadySignal => "file-watcher-ready-signal",
            Self::TransitionDbToReady => "transition-db-to-ready",
            Self::SeedDefaultLayout => "seed-default-layout",
            Self::StartActionWatchers => "start-action-watchers",
            Self::RegistryLinkClassifier => "registry-link-classifier",
            Self::PostReady => "post-ready",
        }
    }
}

impl std::fmt::Display for BootStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Why a boot step did not run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SkipReason {
    /// Disclosed degradation: the platform cannot host this step.
    MissingCapability(PlatformCapability),
    /// This session is not configured for the step (no vault root, no
    /// integrations directory). Expected, so it discloses at `info!`.
    ConfigAbsent(&'static str),
    /// Drift: the platform could have run it and the boot path did not.
    NoPlatformReason,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SkippedStep {
    pub step: BootStep,
    pub reason: SkipReason,
}

/// Proof that a boot path finished its disclosure, and the result.
///
/// `SessionParts` requires one, which is what stops a boot path from quietly
/// dropping the ledger: there is no way to build a session without having
/// called [`BootDisclosure::finish`].
#[derive(Clone, Debug)]
pub struct BootReport {
    capabilities: PlatformCapabilities,
    skipped: Vec<SkippedStep>,
}

impl BootReport {
    /// Every step that did not run, with the reason it did not.
    pub fn skipped(&self) -> &[SkippedStep] {
        &self.skipped
    }

    /// The steps that were skipped with no platform or config excuse — the
    /// drift set. This is the list that should be empty on every boot path.
    pub fn drift(&self) -> Vec<BootStep> {
        self.skipped
            .iter()
            .filter(|s| s.reason == SkipReason::NoPlatformReason)
            .map(|s| s.step)
            .collect()
    }

    pub fn capabilities(&self) -> PlatformCapabilities {
        self.capabilities
    }
}

/// Records which boot steps a path performed and discloses the rest.
///
/// Cloneable and internally shared so the shared wiring can record its
/// registration-phase steps and then hand the same ledger to the
/// `FrontendSession` factory closure, which records the remaining steps and
/// calls [`Self::finish`].
#[derive(Clone)]
pub struct BootDisclosure {
    capabilities: PlatformCapabilities,
    state: Arc<Mutex<State>>,
}

struct State {
    performed: Vec<BootStep>,
    config_absent: Vec<(BootStep, &'static str)>,
}

impl BootDisclosure {
    pub fn new(capabilities: PlatformCapabilities) -> Self {
        Self {
            capabilities,
            state: Arc::new(Mutex::new(State {
                performed: Vec::new(),
                config_absent: Vec::new(),
            })),
        }
    }

    pub fn capabilities(&self) -> PlatformCapabilities {
        self.capabilities
    }

    /// Record that `step` ran. Call it at the step's site, so deleting the step
    /// deletes its mark and the disclosure goes loud on the next boot.
    pub fn performed(&self, step: BootStep) {
        let mut state = self.lock();
        if !state.performed.contains(&step) {
            state.performed.push(step);
        }
    }

    /// Record that `step` was skipped because this session is not configured
    /// for it — no vault root, no integrations directory. Keeps an expected
    /// absence out of the drift set without making it invisible.
    pub fn absent_by_config(&self, step: BootStep, reason: &'static str) {
        let mut state = self.lock();
        if !state.config_absent.iter().any(|(s, _)| *s == step) {
            state.config_absent.push((step, reason));
        }
    }

    /// Close the ledger: log every step that did not run and return the report.
    ///
    /// Consumes the disclosure, and the returned [`BootReport`] is required to
    /// build a session — that is what makes forgetting this call a compile
    /// error instead of a silently disarmed ledger.
    pub fn finish(self) -> BootReport {
        let state = self.lock();
        let skipped: Vec<SkippedStep> = BootStep::ALL
            .into_iter()
            .filter(|step| !state.performed.contains(step))
            .map(|step| {
                let reason = match state.config_absent.iter().find(|(s, _)| *s == step) {
                    Some((_, why)) => SkipReason::ConfigAbsent(why),
                    None => match step.requires() {
                        Some(capability) if !self.capabilities.has(capability) => {
                            SkipReason::MissingCapability(capability)
                        }
                        _ => SkipReason::NoPlatformReason,
                    },
                };
                SkippedStep { step, reason }
            })
            .collect();
        for skip in &skipped {
            match skip.reason {
                SkipReason::MissingCapability(capability) => tracing::warn!(
                    "boot [component=platform-capabilities]: step `{}` SKIPPED — this platform \
                     has no `{capability}`. The feature it provides is unavailable for this \
                     session.",
                    skip.step
                ),
                SkipReason::ConfigAbsent(why) => tracing::info!(
                    "boot [component=platform-capabilities]: step `{}` skipped — {why}. Expected \
                     for this configuration; the feature is simply not in use.",
                    skip.step
                ),
                SkipReason::NoPlatformReason => tracing::warn!(
                    "boot [component=platform-capabilities]: step `{}` SKIPPED with NO platform \
                     reason — this boot path has drifted from the shared wiring \
                     (crates/holon-app/src/wiring.rs) and the feature is silently missing.",
                    skip.step
                ),
            }
        }
        drop(state);
        BootReport {
            capabilities: self.capabilities,
            skipped,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("BootDisclosure mutex poisoned — a boot step panicked while recording")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps_of(report: &BootReport, reason: SkipReason) -> Vec<BootStep> {
        report
            .skipped()
            .iter()
            .filter(|s| s.reason == reason)
            .map(|s| s.step)
            .collect()
    }

    #[test]
    fn all_lists_every_step_exactly_once() {
        let mut seen = Vec::new();
        for step in BootStep::ALL {
            assert!(
                !seen.contains(&step),
                "{step} listed twice in BootStep::ALL"
            );
            seen.push(step);
        }
    }

    #[test]
    fn native_boot_that_performs_every_step_discloses_nothing() {
        let disclosure = BootDisclosure::new(PlatformCapabilities::NATIVE);
        for step in BootStep::ALL {
            disclosure.performed(step);
        }
        assert_eq!(disclosure.finish().skipped(), &[]);
    }

    /// The worker's shape: it seeds, starts watchers, and gets theme /
    /// preferences / error tracker for free from `SessionParts`. Every skip
    /// must be classified, and the steps with no platform excuse must be
    /// named as drift — that partition is the whole point.
    #[test]
    fn browser_worker_shape_separates_degradation_from_drift() {
        let disclosure = BootDisclosure::new(PlatformCapabilities::BROWSER_WORKER);
        for step in [
            BootStep::SeedDefaultLayout,
            BootStep::StartActionWatchers,
            BootStep::ThemeAndPreferences,
            BootStep::PublishErrorTracker,
        ] {
            disclosure.performed(step);
        }

        let report = disclosure.finish();
        assert_eq!(report.skipped().len(), BootStep::ALL.len() - 4);
        assert_eq!(
            report.drift(),
            vec![
                BootStep::DegradedSignalBus,
                BootStep::WritebackDisclosure,
                BootStep::UiInfo,
                BootStep::TransitionDbToReady,
                BootStep::RegistryLinkClassifier,
                BootStep::PostReady,
            ]
        );
        assert_eq!(
            steps_of(
                &report,
                SkipReason::MissingCapability(PlatformCapability::Filesystem)
            ),
            vec![
                BootStep::ConsolidatorEpochGuard,
                BootStep::OrgModeIngest,
                BootStep::FileWatcherReadySignal,
            ]
        );
        assert_eq!(
            steps_of(
                &report,
                SkipReason::MissingCapability(PlatformCapability::ProcessSpawn)
            ),
            vec![BootStep::McpIntegrations, BootStep::McpFdwTables]
        );
    }

    /// A capability-gated step skipped on a platform that HAS the capability is
    /// drift, not degradation — otherwise a native regression would hide behind
    /// the excuse the wasm target earned.
    #[test]
    fn capability_present_but_step_skipped_reads_as_drift() {
        let disclosure = BootDisclosure::new(PlatformCapabilities::NATIVE);
        for step in BootStep::ALL {
            if step != BootStep::OrgModeIngest {
                disclosure.performed(step);
            }
        }
        assert_eq!(
            disclosure.finish().skipped(),
            &[SkippedStep {
                step: BootStep::OrgModeIngest,
                reason: SkipReason::NoPlatformReason,
            }]
        );
    }

    /// A vault-less native session must not report org ingest as drift, and
    /// must not stay silent about it either.
    #[test]
    fn config_absence_is_disclosed_without_counting_as_drift() {
        let disclosure = BootDisclosure::new(PlatformCapabilities::NATIVE);
        for step in BootStep::ALL {
            if step != BootStep::OrgModeIngest {
                disclosure.performed(step);
            }
        }
        disclosure.absent_by_config(BootStep::OrgModeIngest, "no vault root configured");

        let report = disclosure.finish();
        assert_eq!(report.drift(), Vec::new());
        assert_eq!(
            report.skipped(),
            &[SkippedStep {
                step: BootStep::OrgModeIngest,
                reason: SkipReason::ConfigAbsent("no vault root configured"),
            }]
        );
    }

    /// A missing capability outranks nothing: a step the platform cannot host
    /// stays a disclosed degradation even on a path that never reached the
    /// config check.
    #[test]
    fn missing_capability_outranks_drift_for_the_same_step() {
        let disclosure = BootDisclosure::new(PlatformCapabilities::BROWSER_WORKER);
        let report = disclosure.finish();
        assert_eq!(
            steps_of(
                &report,
                SkipReason::MissingCapability(PlatformCapability::Filesystem)
            ),
            vec![
                BootStep::ConsolidatorEpochGuard,
                BootStep::OrgModeIngest,
                BootStep::FileWatcherReadySignal,
            ]
        );
    }
}
