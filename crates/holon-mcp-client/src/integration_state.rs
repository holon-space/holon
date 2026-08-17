//! Per-integration user state: is it ON, and does it have credentials.
//!
//! An integration has three independent axes. PRESENCE — whether this build
//! ships the sidecar at all — is settled at compile time by
//! [`crate::bundled_sidecars`], so it is not stored: a provider that is absent
//! from [`BUNDLED_SIDECARS`] has no state, and asking for it is an error.
//! ENABLEMENT and CONFIGURATION are user decisions that outlive the process,
//! so each provider gets a state file beside its sidecar.
//!
//! Consumers never read that file. They take a
//! [`ReadOnlyMutable<IntegrationState>`] out of [`IntegrationConfigStore`] and
//! react to it, which keeps the storage substrate replaceable — a file watcher
//! or a Turso-backed table can land behind the same signal.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use futures_signals::signal::Mutable;
use futures_signals::signal::ReadOnlyMutable;
use serde::Deserialize;
use serde::Serialize;

use crate::bundled_sidecars::BUNDLED_SIDECARS;

/// Where one credential value lives. The state file records the LOCATION of a
/// secret, never the secret — it is plain-text user config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum CredentialRef {
    Env { var: String },
    File { path: PathBuf },
    Keychain { service: String, account: String },
}

/// The credential set a one-time OAuth bootstrap produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub client_id: CredentialRef,
    pub client_secret: CredentialRef,
    pub refresh_token_file: PathBuf,
}

/// Whether the one-time credential setup has happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Configuration {
    #[default]
    Unconfigured,
    Configured(Credentials),
}

/// The two stored axes of one integration.
///
/// This is the in-memory domain type, deliberately NOT the on-disk one: see
/// [`StateFile`], which is what carries the schema version and the strictness.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IntegrationState {
    pub enabled: bool,
    pub configuration: Configuration,
}

/// Generation of the state-file format this build writes and accepts, matching
/// the [`crate::bundled_sidecars::SIDECAR_SCHEMA_VERSION`] treatment of the
/// sidecars these files sit beside. Bump it whenever a change lands that an
/// older state file does not satisfy.
pub const INTEGRATION_STATE_SCHEMA_VERSION: u32 = 1;

/// The on-disk form. Every field is required and unknown fields are rejected at
/// this level (`deny_unknown_fields` cannot reach inside the internally-tagged
/// enums below), so reading a state means reading a COMPLETE,
/// current-generation file: a truncated, empty, hand-edited or older-format
/// file is a parse error rather than a set of defaults that would silently
/// switch an integration off.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateFile {
    schema_version: u32,
    enabled: bool,
    configuration: Configuration,
}

impl StateFile {
    fn of(state: &IntegrationState) -> Self {
        Self {
            schema_version: INTEGRATION_STATE_SCHEMA_VERSION,
            enabled: state.enabled,
            configuration: state.configuration.clone(),
        }
    }

    fn into_state(self) -> anyhow::Result<IntegrationState> {
        anyhow::ensure!(
            self.schema_version == INTEGRATION_STATE_SCHEMA_VERSION,
            "it declares schema_version {} but this build writes and reads \
             schema_version {INTEGRATION_STATE_SCHEMA_VERSION}",
            self.schema_version
        );
        Ok(IntegrationState {
            enabled: self.enabled,
            configuration: self.configuration,
        })
    }
}

/// State lives beside the sidecar it describes. The `.toml` extension keeps it
/// out of the `*.yaml` sidecar scan in [`crate::integration_config`], which
/// would otherwise read `gcal.state` as a provider of its own.
fn state_path_in(dir: &Path, provider: &str) -> PathBuf {
    dir.join(format!("{provider}.state.toml"))
}

/// The command that writes a complete enabling state file. Every disclosure
/// names it rather than quoting a TOML fragment: the state parser rejects a
/// partial file, so "write `enabled = true`" is advice that produces a broken
/// integration.
pub const ENABLE_COMMAND: &str = "scripts/holon-integration-enable.sh";

/// The smallest complete state file that switches an integration ON, rendered
/// from the types so a disclosure quoting it can never drift from what the
/// loader accepts. Credentials stay on their own axis: this leaves the
/// integration `Unconfigured`.
pub fn enabling_state_file() -> String {
    let state = IntegrationState {
        enabled: true,
        configuration: Configuration::Unconfigured,
    };
    toml::to_string_pretty(&StateFile::of(&state))
        .expect("the enabling state file is built from the types and always serializes")
}

/// Write `text` to `path` via a temporary sibling, so a reader either sees the
/// previous content or the new content and never a partial file. The temporary
/// lands in the same directory to keep the rename on one filesystem, and its
/// `.tmp` extension keeps it out of the `*.yaml` sidecar scan.
fn write_atomically(path: &Path, text: &str) -> anyhow::Result<()> {
    use std::io::Write;

    let tmp = path.with_extension("toml.tmp");
    let mut file = std::fs::File::create(&tmp)
        .with_context(|| format!("Failed to create '{}'", tmp.display()))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("Failed to write '{}'", tmp.display()))?;
    file.sync_all()
        .with_context(|| format!("Failed to flush '{}' to disk", tmp.display()))?;
    drop(file);
    std::fs::rename(&tmp, path).with_context(|| {
        format!(
            "Failed to move '{}' into place at '{}'",
            tmp.display(),
            path.display()
        )
    })
}

/// Reactive access to every bundled integration's state.
#[derive(Debug)]
pub struct IntegrationConfigStore {
    dir: PathBuf,
    states: HashMap<&'static str, Mutable<IntegrationState>>,
}

impl IntegrationConfigStore {
    /// Read the state of every bundled integration out of `dir`.
    ///
    /// A MISSING state file is the only way to mean "never touched", and it
    /// yields the default state. A file that exists but is not a complete,
    /// current-generation [`StateFile`] is fatal, naming the provider and the
    /// path: falling back to the default there would silently switch a
    /// configured integration off, and an empty file is exactly what a crashed
    /// write used to leave behind.
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let mut states = HashMap::new();
        for sidecar in BUNDLED_SIDECARS {
            let path = state_path_in(dir, sidecar.provider);
            let state = match std::fs::read_to_string(&path) {
                Ok(text) => toml::from_str::<StateFile>(&text)
                    .map_err(anyhow::Error::new)
                    .and_then(StateFile::into_state)
                    .with_context(|| {
                        format!(
                            "Integration '{}' has an unusable state file '{}'",
                            sidecar.provider,
                            path.display()
                        )
                    })?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => IntegrationState::default(),
                Err(e) => {
                    return Err(anyhow::Error::new(e).context(format!(
                        "Failed to read state file '{}' for integration '{}'",
                        path.display(),
                        sidecar.provider
                    )));
                }
            };
            states.insert(sidecar.provider, Mutable::new(state));
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            states,
        })
    }

    /// The directory the state files and sidecars share.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The providers this build ships — the presence axis, in full.
    pub fn providers(&self) -> Vec<&'static str> {
        BUNDLED_SIDECARS.iter().map(|s| s.provider).collect()
    }

    /// The reactive cell for `provider`, for consumers that want a signal.
    pub fn state(&self, provider: &str) -> anyhow::Result<ReadOnlyMutable<IntegrationState>> {
        Ok(self.cell(provider)?.read_only())
    }

    /// The current state of `provider`.
    pub fn get(&self, provider: &str) -> anyhow::Result<IntegrationState> {
        Ok(self.cell(provider)?.get_cloned())
    }

    /// Replace `provider`'s state, updating both the file and the signal.
    ///
    /// The file is written first: a consumer must never see a signal for a
    /// decision the next boot will have forgotten. The write goes to a
    /// temporary sibling and is renamed over the target, so the state file is
    /// never observable half-written — an in-place truncating write would leave
    /// a zero-byte file if it were interrupted, and the recovery from that is
    /// an error at the next boot rather than a working integration.
    pub fn set(&self, provider: &str, state: IntegrationState) -> anyhow::Result<()> {
        let cell = self.cell(provider)?;
        let path = state_path_in(&self.dir, provider);
        std::fs::create_dir_all(&self.dir).with_context(|| {
            format!(
                "Failed to create integrations directory '{}'",
                self.dir.display()
            )
        })?;
        let text = toml::to_string_pretty(&StateFile::of(&state))
            .with_context(|| format!("Integration '{provider}' state does not serialize"))?;
        write_atomically(&path, &text).with_context(|| {
            format!(
                "Failed to write state file '{}' for integration '{provider}'",
                path.display()
            )
        })?;
        cell.set(state);
        Ok(())
    }

    /// Where `provider`'s state is stored.
    pub fn state_path(&self, provider: &str) -> anyhow::Result<PathBuf> {
        self.cell(provider)?;
        Ok(state_path_in(&self.dir, provider))
    }

    fn cell(&self, provider: &str) -> anyhow::Result<&Mutable<IntegrationState>> {
        self.states.get(provider).with_context(|| {
            format!(
                "This build ships no sidecar for integration '{provider}' — it has no state. \
                 Bundled: {}",
                self.providers().join(", ")
            )
        })
    }
}
