//! Typed, component-attributed boot failures (boot-ladder increment 1).
//!
//! The boot spine used to `expect`/`panic!` at every failable step, so a
//! stale DB, an unwritable path, or a corrupt store all surfaced as an opaque
//! panic (the Android incident). [`BootError`] carries *which component* failed
//! at *which stage*, wrapping the underlying error so the full source chain is
//! preserved for disclosure — never swallowed.
//!
//! Increment 1 introduces the type and routes the boot-spine panic inventory
//! into it (error PROPAGATION, not handling — no fallbacks, no silent
//! recovery). The recovery shell / ladder that *acts* on a `BootError` is
//! increment 2+.

use std::fmt;

/// Which load-bearing boot component produced a failure.
///
/// Attribution answers "what do I repair?" for the recovery surface. The set
/// is deliberately coarse — one variant per component the boot supervisor
/// treats as an independently-failable unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootComponent {
    /// Config load / path resolution (`holon.toml`, data dirs).
    Config,
    /// Consolidator epoch guard (refuses an unsafe consolidator-mode flip).
    EpochGuard,
    /// Turso storage engine (open, backend construction, schema, matviews).
    Turso,
    /// Loro CRDT store.
    Loro,
    /// OrgMode file sync.
    OrgSync,
    /// External MCP integrations (`integrations/*.yaml`).
    McpIntegrations,
    /// `FrontendSession` assembly (the DI factory that wires the ViewModel).
    Session,
    /// Host platform init (mobile windowing, tokio runtime).
    Platform,
}

impl fmt::Display for BootComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            BootComponent::Config => "config",
            BootComponent::EpochGuard => "epoch-guard",
            BootComponent::Turso => "turso",
            BootComponent::Loro => "loro",
            BootComponent::OrgSync => "org-sync",
            BootComponent::McpIntegrations => "mcp-integrations",
            BootComponent::Session => "session",
            BootComponent::Platform => "platform",
        };
        f.write_str(s)
    }
}

/// Which boot stage was executing when the failure occurred.
///
/// Stages are ordered as the spine runs them: config-load → epoch-guard →
/// container-configure → engine-resolve → session-resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootStage {
    ConfigLoad,
    EpochGuard,
    ContainerConfigure,
    EngineResolve,
    SessionResolve,
}

impl fmt::Display for BootStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            BootStage::ConfigLoad => "config-load",
            BootStage::EpochGuard => "epoch-guard",
            BootStage::ContainerConfigure => "container-configure",
            BootStage::EngineResolve => "engine-resolve",
            BootStage::SessionResolve => "session-resolve",
        };
        f.write_str(s)
    }
}

/// A boot failure, attributed to a component and stage, wrapping its cause.
///
/// The `source` is retained (not flattened to a string) so callers can walk
/// the full chain. `Display` renders the attribution plus the whole chain.
#[derive(Debug)]
pub struct BootError {
    pub component: BootComponent,
    pub stage: BootStage,
    pub source: anyhow::Error,
}

impl BootError {
    pub fn new(
        component: BootComponent,
        stage: BootStage,
        source: impl Into<anyhow::Error>,
    ) -> Self {
        Self {
            component,
            stage,
            source: source.into(),
        }
    }

    /// Classify an opaque fluxdi `bootstrap()` failure into an attributed
    /// [`BootError`].
    ///
    /// fluxdi surfaces module lifecycle failures as
    /// `module_lifecycle_failed(module_name, phase, details)` whose message
    /// embeds `module=<Name>`. We can't get a Result back from a provider
    /// closure without forking fluxdi (open question for increment 4), so the
    /// bootstrap call surface is the boundary where we recover attribution —
    /// by matching the module name the spine wrote into the message. This is
    /// the sanctioned boundary-wrap, not a fork.
    pub fn from_bootstrap_error(err: fluxdi::Error) -> Self {
        let msg = err.message.as_str();
        let (component, stage) = if msg.contains("CoreInfraModule") {
            (BootComponent::Turso, BootStage::EngineResolve)
        } else if msg.contains("LoroModule") {
            (BootComponent::Loro, BootStage::EngineResolve)
        } else if msg.contains("OrgModeModule") {
            (BootComponent::OrgSync, BootStage::ContainerConfigure)
        } else if msg.contains("McpIntegrationsModule") {
            (
                BootComponent::McpIntegrations,
                BootStage::ContainerConfigure,
            )
        } else {
            // GpuiModule/HolonFrontendModule wrap `on_start` (FrontendSession
            // resolution) and anything not attributable to a named storage
            // module. Session/SessionResolve is the honest default: the
            // failure surfaced while assembling the session.
            (BootComponent::Session, BootStage::SessionResolve)
        };
        Self::new(component, stage, anyhow::anyhow!("{msg}"))
    }

    /// A multi-line, component-attributed report for a terminal / log sink:
    /// the attribution line followed by the full source chain.
    pub fn structured_report(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "boot failed: component={} stage={}",
            self.component, self.stage
        );
        let _ = writeln!(out, "  cause: {}", self.source);
        for (depth, cause) in self.source.chain().skip(1).enumerate() {
            let _ = writeln!(out, "  [{}] {}", depth + 1, cause);
        }
        out
    }
}

impl fmt::Display for BootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "boot failed [component={} stage={}]: {:#}",
            self.component, self.stage, self.source
        )
    }
}

impl std::error::Error for BootError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use fluxdi::Injector;
    use fluxdi::Module;
    use holon::di::CoreInfraModule;

    use super::*;

    /// A failing Turso open must surface as an attributed
    /// `BootError{Turso, EngineResolve}` — NOT a panic.
    ///
    /// This exercises the real spine: `CoreInfraModule::configure` runs
    /// `open_and_register_core`, whose Turso-open step now propagates an error
    /// (previously `.expect`, which panicked — the pre-fix RED state: this test
    /// would panic instead of producing an `Err` to classify). fluxdi wraps it
    /// as `module=CoreInfraModule`, and `from_bootstrap_error` recovers the
    /// attribution at the boundary.
    #[test]
    fn failing_turso_open_attributes_turso_engine_resolve() {
        // A directory is not a valid sqlite file; opening it as the DB fails
        // at `open_database` (before any actor spawn, so no runtime needed).
        let bad_db_path = std::env::temp_dir();
        assert!(bad_db_path.is_dir(), "temp dir must exist for this test");

        let injector = Injector::root();
        let flux_err = CoreInfraModule {
            db_path: bad_db_path,
        }
        .configure(&injector)
        .expect_err("opening a directory as the Turso DB must fail, not succeed");

        let boot_err = BootError::from_bootstrap_error(flux_err);
        assert_eq!(boot_err.component, BootComponent::Turso);
        assert_eq!(boot_err.stage, BootStage::EngineResolve);
    }

    /// A second named storage module (Loro) classifies to its own component.
    #[test]
    fn loro_module_failure_attributes_loro_engine_resolve() {
        let flux_err =
            fluxdi::Error::module_lifecycle_failed("LoroModule", "configure", "loro open failed");
        let boot_err = BootError::from_bootstrap_error(flux_err);
        assert_eq!(boot_err.component, BootComponent::Loro);
        assert_eq!(boot_err.stage, BootStage::EngineResolve);
    }

    /// Anything not attributable to a named storage module falls back to
    /// Session/SessionResolve (the honest default — the failure surfaced while
    /// assembling the session).
    #[test]
    fn unattributed_failure_falls_back_to_session_resolve() {
        let flux_err = fluxdi::Error::module_lifecycle_failed(
            "SomeUnrelatedModule",
            "on_start",
            "an arbitrary failure with no known storage-module name",
        );
        let boot_err = BootError::from_bootstrap_error(flux_err);
        assert_eq!(boot_err.component, BootComponent::Session);
        assert_eq!(boot_err.stage, BootStage::SessionResolve);
    }
}
