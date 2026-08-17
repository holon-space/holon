//! Frontend-agnostic view model for the integrations settings surface.
//!
//! The rendering layer must not name [`IntegrationConfigStore`] or the
//! credential types behind [`Configuration`]: a settings list needs the
//! provider, whether it is on, and whether its one-time setup has happened —
//! never where a secret lives. This module is that projection, plus the two
//! writes the surface performs, so the same list can back a second frontend
//! without moving any logic.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use futures_signals::signal::ReadOnlyMutable;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::IntegrationState;

/// Whether an integration's one-time credential setup has happened — the
/// display half of [`Configuration`], with the credential locations dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigStatus {
    Unconfigured,
    Configured,
}

impl ConfigStatus {
    fn of(configuration: &Configuration) -> Self {
        match configuration {
            Configuration::Unconfigured => Self::Unconfigured,
            Configuration::Configured(_) => Self::Configured,
        }
    }

    /// The word the settings list prints for this status.
    pub fn label(self) -> &'static str {
        match self {
            Self::Unconfigured => "Unconfigured",
            Self::Configured => "Configured",
        }
    }
}

/// One row of the integrations settings list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationRow {
    /// The provider id — the same `&'static str` the bundle and the store use,
    /// so a row can be handed straight back to
    /// [`IntegrationsSettingsVm::set_enabled`].
    pub provider: &'static str,
    pub enabled: bool,
    pub status: ConfigStatus,
}

/// The integrations settings list, over the store that owns enablement.
pub struct IntegrationsSettingsVm {
    store: Arc<IntegrationConfigStore>,
}

impl IntegrationsSettingsVm {
    pub fn new(store: Arc<IntegrationConfigStore>) -> Self {
        Self { store }
    }

    /// The list over the integrations directory at `dir`, with a store of its
    /// own. The composition root uses [`Self::new`] instead, because there the
    /// boot loader must read the SAME store — two stores over one directory
    /// share the files but not the signals.
    pub fn over_dir(dir: &Path) -> anyhow::Result<Self> {
        Ok(Self::new(Arc::new(IntegrationConfigStore::load(dir)?)))
    }

    /// Where `provider`'s decision is stored — what a disclosure names when the
    /// user has to look at or repair the file.
    pub fn state_path(&self, provider: &str) -> anyhow::Result<PathBuf> {
        self.store.state_path(provider)
    }

    /// Every bundled integration, in bundle order.
    ///
    /// The list is the PRESENCE axis in full: a provider that is off, or that
    /// the user has never touched, is exactly what the settings surface exists
    /// to show. Every name comes from the store's own bundle, so a lookup here
    /// cannot miss — a miss means the store's two views of the bundle have
    /// diverged, which no caller could recover from.
    pub fn rows(&self) -> Vec<IntegrationRow> {
        self.store
            .providers()
            .into_iter()
            .map(|provider| {
                let state = self.store.get(provider).unwrap_or_else(|e| {
                    panic!("Bundled provider '{provider}' has no state cell: {e:#}")
                });
                IntegrationRow {
                    provider,
                    enabled: state.enabled,
                    status: ConfigStatus::of(&state.configuration),
                }
            })
            .collect()
    }

    /// Switch `provider` on or off, leaving its configuration axis untouched.
    ///
    /// Read-modify-write rather than a fresh state: enablement and
    /// configuration are independent, and writing a default configuration here
    /// would discard an OAuth bootstrap that only a manual, external step can
    /// redo.
    pub fn set_enabled(&self, provider: &str, enabled: bool) -> anyhow::Result<()> {
        let mut state = self.store.get(provider)?;
        state.enabled = enabled;
        self.store.set(provider, state)
    }

    /// The signal behind every row, for a frontend that wants to re-render when
    /// a state changes outside its own toggle (another window, an OAuth
    /// bootstrap, a hand-edited state file).
    pub fn signals(&self) -> Vec<ReadOnlyMutable<IntegrationState>> {
        self.store
            .providers()
            .into_iter()
            .map(|provider| {
                self.store.state(provider).unwrap_or_else(|e| {
                    panic!("Bundled provider '{provider}' has no state cell: {e:#}")
                })
            })
            .collect()
    }
}
