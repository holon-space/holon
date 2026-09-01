//! Frontend-agnostic view model for the integrations settings surface.
//!
//! The rendering layer must not name [`IntegrationConfigStore`] or the
//! credential types behind [`Configuration`]: a settings list needs the
//! provider, whether it is on, and whether its one-time setup has happened —
//! never where a secret lives. This module is that projection, plus the two
//! writes the surface performs, so the same list can back a second frontend
//! without moving any logic.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use futures_signals::signal::Mutable;
use futures_signals::signal::ReadOnlyMutable;
use holon_api::icon_name::IconName;
use holon_mcp_client::CredentialRoot;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::IntegrationState;
use holon_mcp_client::oauth_bootstrap::BrowserOpener;
use holon_mcp_client::rest_oauth2::RestOAuth2Config;

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
    /// Whether [`IntegrationsSettingsVm::configure`] has a consent flow to run
    /// for this provider — true iff its sidecar declares an OAuth2 arm with an
    /// authorization endpoint.
    ///
    /// A surface that offered the setup on every unconfigured row would put a
    /// dead-end button on integrations that authenticate with a static token or
    /// no token at all.
    pub configurable: bool,
    /// What the row calls this provider: the sidecar's `display_name`, else
    /// [`humanize_provider_name`].
    pub display_name: String,
    /// The glyph the row shows: the sidecar's `icon`, else [`DEFAULT_ICON`].
    pub icon: IconName,
    /// The BARE id of the block `integration.open_default_view` focuses, or
    /// `None` when this provider has no view page yet.
    pub default_view: Option<String>,
}

/// The sidecar-sourced half of an [`IntegrationRow`], with the derivations
/// already applied.
struct Presentation {
    display_name: String,
    icon: IconName,
    default_view: Option<String>,
}

/// The glyph a provider that names none gets.
///
/// `link` is the only name in the renderer's table that means "a connection to
/// something outside" — there is no plug, cloud or database glyph to prefer.
pub const DEFAULT_ICON: &str = "link";

/// The provider id as a title: `claude-history` → `Claude History`.
///
/// Splits on `-`, `_` and camelCase humps, so `logseqDb` → `Logseq Db`. This is
/// a DERIVATION, not a translation — a provider whose real name it gets wrong
/// says so with `display_name` in its sidecar.
pub fn humanize_provider_name(provider: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut prev_lower_or_digit = false;
    for ch in provider.chars() {
        if ch == '-' || ch == '_' {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            prev_lower_or_digit = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower_or_digit && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        prev_lower_or_digit = ch.is_lowercase() || ch.is_numeric();
        word.push(ch);
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
        .iter()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>()
                        + chars.as_str().to_lowercase().as_str()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where a provider's one-time consent flow has got to.
///
/// The rendering layer shows this verbatim, so every arm has to be a sentence a
/// user can act on: a flow that fails mid-browser has no other way to explain
/// itself, and a silent return to "Unconfigured" is the degradation this
/// codebase refuses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConfigureProgress {
    /// No flow has run in this session.
    #[default]
    Idle,
    /// The browser has been opened and the loopback is waiting.
    AwaitingConsent,
    Succeeded,
    Failed(String),
}

impl ConfigureProgress {
    /// The line the settings row prints, or `None` while there is nothing to
    /// say.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Idle => None,
            Self::AwaitingConsent => Some(
                "Waiting for you to finish in the browser — Holon is listening for the redirect."
                    .to_string(),
            ),
            Self::Succeeded => {
                Some("Configured. This takes effect at the next launch.".to_string())
            }
            Self::Failed(why) => Some(format!("Configuration failed: {why}")),
        }
    }
}

/// The integrations settings list, over the store that owns enablement.
pub struct IntegrationsSettingsVm {
    store: Arc<IntegrationConfigStore>,
    /// The directory the sidecars and state files live in — what the consent
    /// flow needs to read a provider's OAuth endpoints.
    dir: PathBuf,
    /// The active profile's config directory. The consent flow WRITES a refresh
    /// token, so it must land in the profile that asked for it and nowhere
    /// else.
    root: CredentialRoot,
    /// Per-provider consent-flow progress, keyed by provider id.
    progress: Mutex<HashMap<String, Mutable<ConfigureProgress>>>,
}

impl IntegrationsSettingsVm {
    pub fn new(store: Arc<IntegrationConfigStore>, root: CredentialRoot) -> Self {
        let dir = store.dir().to_path_buf();
        Self {
            store,
            dir,
            root,
            progress: Mutex::new(HashMap::new()),
        }
    }

    /// The list over the integrations directory at `dir`, with a store of its
    /// own. The composition root uses [`Self::new`] instead, because there the
    /// boot loader must read the SAME store — two stores over one directory
    /// share the files but not the signals.
    pub fn over_dir(dir: &Path, config_dir: &Path) -> anyhow::Result<Self> {
        Ok(Self::new(
            Arc::new(IntegrationConfigStore::load(dir)?),
            CredentialRoot::new(config_dir),
        ))
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
                let presentation = self.presentation(provider);
                IntegrationRow {
                    provider,
                    enabled: state.enabled,
                    status: ConfigStatus::of(&state.configuration),
                    configurable: self.oauth2_config(provider).is_ok(),
                    display_name: presentation.display_name,
                    icon: presentation.icon,
                    default_view: presentation.default_view,
                }
            })
            .collect()
    }

    /// `provider`'s presentation triple, resolved against its sidecar.
    ///
    /// A sidecar that will not read leaves the row on the derivations and says
    /// so in the log; it is not silent, because the startup loader reports the
    /// same file as ignored and this is the second voice on one fact, not the
    /// only one. Refusing to produce a row here instead would take the whole
    /// Integrations section down over one unparseable installed file.
    fn presentation(&self, provider: &'static str) -> Presentation {
        let derived = || Presentation {
            display_name: humanize_provider_name(provider),
            icon: IconName::parse(DEFAULT_ICON)
                .unwrap_or_else(|e| panic!("DEFAULT_ICON must be a name the renderer draws: {e}")),
            default_view: None,
        };
        match holon_mcp_client::integration_config::provider_content(&self.dir, provider) {
            Ok(content) => Presentation {
                display_name: content
                    .config
                    .display_name
                    .clone()
                    .unwrap_or_else(|| humanize_provider_name(provider)),
                icon: content.config.icon.clone().unwrap_or_else(|| {
                    IconName::parse(DEFAULT_ICON).expect("DEFAULT_ICON is drawable")
                }),
                default_view: content.config.default_view.clone(),
            },
            Err(e) => {
                tracing::warn!(
                    provider,
                    "[IntegrationsSettingsVm] '{provider}' sidecar did not read ({e:#}); its row \
                     falls back to the derived name and the default icon, and it has no default \
                     view"
                );
                derived()
            }
        }
    }

    /// `provider`'s OAuth2 block, or the reason there is no consent flow to
    /// run.
    ///
    /// One lookup behind both the row's `configurable` flag and the flow
    /// itself, so the button cannot appear on a row whose flow would
    /// immediately refuse.
    fn oauth2_config(&self, provider: &str) -> anyhow::Result<RestOAuth2Config> {
        Ok(self.provider_oauth2(provider)?.0)
    }

    /// `provider`'s OAuth2 block plus any supersession the caller must
    /// disclose.
    fn provider_oauth2(
        &self,
        provider: &str,
    ) -> anyhow::Result<(RestOAuth2Config, Option<String>)> {
        let content = holon_mcp_client::integration_config::provider_content(&self.dir, provider)?;
        let config = content.config;
        let oauth2 = config
            .transport
            .rest
            .as_ref()
            .and_then(|rest| rest.auth.as_ref())
            .and_then(|auth| auth.oauth2.as_ref())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "'{provider}' does not authenticate with OAuth2, so there is no consent flow \
                     to run"
                )
            })?;
        anyhow::ensure!(
            oauth2.auth_url.is_some(),
            "'{provider}' declares no OAuth2 `auth_url`, so there is no authorization endpoint to \
             send the browser to"
        );
        Ok((oauth2.clone(), content.superseded))
    }

    /// Switch `provider` on or off, leaving its configuration axis untouched.
    ///
    /// Read-modify-write rather than a fresh state: enablement and
    /// configuration are independent, and writing a default configuration here
    /// would discard a consent the user would have to sit through again — and
    /// which some providers will not grant a second time without a manual
    /// revoke.
    pub fn set_enabled(&self, provider: &str, enabled: bool) -> anyhow::Result<()> {
        let mut state = self.store.get(provider)?;
        state.enabled = enabled;
        self.store.set(provider, state)
    }

    /// The consent-flow progress cell for `provider`, created on first use.
    pub fn configure_progress(&self, provider: &str) -> ReadOnlyMutable<ConfigureProgress> {
        self.progress
            .lock()
            .expect("the progress map is only held across map operations")
            .entry(provider.to_string())
            .or_default()
            .read_only()
    }

    /// Claim `provider`'s consent flow, or refuse because one is already
    /// running.
    ///
    /// The check and the claim happen under ONE lock: two clicks landing at the
    /// same moment must not both read `Idle` and both proceed.
    fn begin_or_refuse(&self, provider: &str) -> anyhow::Result<()> {
        let mut flows = self
            .progress
            .lock()
            .expect("the progress map is only held across map operations");
        let cell = flows.entry(provider.to_string()).or_default();
        anyhow::ensure!(
            cell.get_cloned() != ConfigureProgress::AwaitingConsent,
            "a setup for '{provider}' is already waiting on your browser. Finish or cancel it \
             before starting another."
        );
        cell.set(ConfigureProgress::AwaitingConsent);
        Ok(())
    }

    fn set_progress(&self, provider: &str, progress: ConfigureProgress) {
        self.progress
            .lock()
            .expect("the progress map is only held across map operations")
            .entry(provider.to_string())
            .or_default()
            .set(progress);
    }

    /// Run `provider`'s consent flow the way a desktop frontend wants it: the
    /// user's own browser, the standard consent timeout.
    ///
    /// The [`BrowserOpener`] seam stays on [`Self::configure`] for tests; a
    /// rendering layer has no business naming it, and this keeps the OAuth
    /// types out of the frontend entirely.
    pub async fn configure_with_system_browser(&self, provider: &str) -> anyhow::Result<()> {
        self.configure(
            provider,
            &holon_mcp_client::oauth_bootstrap::SystemBrowser,
            holon_mcp_client::oauth_bootstrap::DEFAULT_CONSENT_TIMEOUT,
        )
        .await
    }

    /// Run `provider`'s one-time consent flow, publishing its progress.
    ///
    /// The failure is reported through [`ConfigureProgress::Failed`] AND
    /// returned: the caller may want a toast as well, and a flow whose only
    /// trace is a progress cell nobody rendered would be an invisible failure.
    pub async fn configure(
        &self,
        provider: &str,
        browser: &dyn BrowserOpener,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        // Claim the provider before doing anything. The button keeps painting
        // until the flow finishes, so without this a second click starts a
        // second browser hop, a second loopback listener and a second write to
        // the same token file — with one shared progress cell, so whichever
        // finishes last decides what the user is told.
        self.begin_or_refuse(provider)?;
        let outcome = self.run_configure(provider, browser, timeout).await;
        match &outcome {
            Ok(()) => self.set_progress(provider, ConfigureProgress::Succeeded),
            Err(e) => self.set_progress(provider, ConfigureProgress::Failed(format!("{e:#}"))),
        }
        outcome
    }

    async fn run_configure(
        &self,
        provider: &str,
        browser: &dyn BrowserOpener,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let (oauth2, superseded) = self.provider_oauth2(provider)?;
        if let Some(reason) = superseded {
            // The user may well be configuring BECAUSE they edited that file.
            // Letting the flow quietly use the bundled copy instead would make
            // their edit look ineffective with nothing to explain it.
            tracing::warn!(
                provider,
                "an installed sidecar for '{provider}' was passed over for the bundled copy, so \
                 the consent flow is using the bundled endpoints and credential paths: {reason}"
            );
        }

        holon_mcp_client::oauth_bootstrap::configure_integration(
            provider,
            &oauth2,
            &self.store,
            &holon_mcp_client::integration_config::env_var_lookup,
            browser,
            timeout,
            &self.root,
        )
        .await
    }

    /// Each bundled provider with the signal behind its row, for a caller that
    /// wants to react when a state changes outside its own toggle (another
    /// window, an OAuth bootstrap, a hand-edited state file).
    pub fn signals(&self) -> Vec<(&'static str, ReadOnlyMutable<IntegrationState>)> {
        self.store
            .providers()
            .into_iter()
            .map(|provider| {
                let state = self.store.state(provider).unwrap_or_else(|e| {
                    panic!("Bundled provider '{provider}' has no state cell: {e:#}")
                });
                (provider, state)
            })
            .collect()
    }
}

#[cfg(test)]
mod presentation_derivations {
    use super::*;

    /// The derivation a sidecar that says nothing falls back to. The three
    /// cases are the three shapes a provider id takes in this bundle: hyphens,
    /// one bare word, and a camelCase hump.
    #[test]
    fn a_provider_id_derives_a_title() {
        assert_eq!(humanize_provider_name("claude-history"), "Claude History");
        assert_eq!(humanize_provider_name("todoist"), "Todoist");
        assert_eq!(humanize_provider_name("logseqDb"), "Logseq Db");
        assert_eq!(
            humanize_provider_name("json_placeholder"),
            "Json Placeholder"
        );
    }

    /// Separators do not become empty words, and a leading capital is not a
    /// hump — `GCal` is one word, not `G Cal`.
    #[test]
    fn separators_and_leading_capitals_do_not_split_into_empties() {
        assert_eq!(humanize_provider_name("a--b"), "A B");
        assert_eq!(humanize_provider_name("GCal"), "Gcal");
    }

    #[test]
    fn the_default_icon_is_one_the_renderer_draws() {
        IconName::parse(DEFAULT_ICON).expect("DEFAULT_ICON must be in the shared table");
    }
}
