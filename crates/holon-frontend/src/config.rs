//! Layered configuration with source tracking.
//!
//! Uses `premortem` for declarative source merging and `clap` for CLI parsing.
//! Both derive from the same struct (`HolonConfig`), so adding a new config field
//! automatically gets CLI + env var + TOML support.
//!
//! **Precedence** (higher number wins):
//! 1. Defaults (compiled into the binary)
//! 2. `holon.toml` file (in config dir)
//! 3. CLI arguments / environment variables (via clap → `ClapSource`)
//!
//! Values from layer 3 are "locked" — the settings UI shows them as read-only.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use premortem::error::SourceErrorKind;
use premortem::source::{ConfigValues, Source};
use premortem::trace::TracedConfig;
use premortem::value::ConfigValue;
use premortem::{Config, ConfigEnv, ConfigErrors};
use serde::{Deserialize, Serialize};

#[cfg(not(target_arch = "wasm32"))]
use holon_todoist::di::TodoistConfig;

use crate::preferences::PrefKey;

// ---------------------------------------------------------------------------
// HolonConfig — the single source of truth
// ---------------------------------------------------------------------------

/// Top-level configuration. Derives both `clap::Args` (CLI) and
/// `serde::Deserialize` (TOML / premortem). Adding a field here automatically
/// gives it CLI + env + file support.
/// On wasm32 targets, the clap derive and todoist field are omitted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(clap::Parser))]
#[cfg_attr(
    not(target_arch = "wasm32"),
    command(author, version, about = "Holon personal knowledge manager")
)]
pub struct HolonConfig {
    /// Config directory (determines holon.toml location and default db_path).
    /// Not stored in holon.toml itself.
    #[cfg_attr(not(target_arch = "wasm32"), arg(long, env = "HOLON_CONFIG_DIR"))]
    #[serde(skip)]
    pub config_dir: Option<PathBuf>,

    /// Database file path (default: `{config_dir}/holon.db`)
    #[cfg_attr(not(target_arch = "wasm32"), arg(long, env = "HOLON_DB_PATH"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_path: Option<PathBuf>,

    #[cfg_attr(not(target_arch = "wasm32"), command(flatten))]
    #[serde(default)]
    pub orgmode: OrgmodeConfig,

    #[cfg_attr(not(target_arch = "wasm32"), command(flatten))]
    #[serde(default)]
    pub loro: LoroPreferences,

    /// Todoist integration config. Native-only (requires network stack not available on wasm32).
    #[cfg(not(target_arch = "wasm32"))]
    #[cfg_attr(not(target_arch = "wasm32"), arg(skip))]
    #[serde(default)]
    pub todoist: TodoistConfig,

    #[cfg_attr(not(target_arch = "wasm32"), command(flatten))]
    #[serde(default)]
    pub ui: UiConfig,

    #[cfg_attr(not(target_arch = "wasm32"), command(flatten))]
    #[serde(default)]
    pub hooks: HooksConfig,

    /// MCP integrations directory (default: `{config_dir}/integrations`)
    #[cfg_attr(
        not(target_arch = "wasm32"),
        arg(long, env = "HOLON_MCP_INTEGRATIONS_DIR")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_integrations_dir: Option<PathBuf>,

    /// Flat key-value store for all preferences. Keys use dotted notation ("ui.theme").
    /// This is the canonical store that the settings UI reads/writes.
    #[cfg_attr(not(target_arch = "wasm32"), arg(skip))]
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub preferences: HashMap<PrefKey, toml::Value>,
}

impl premortem::validate::Validate for HolonConfig {
    fn validate(&self) -> premortem::error::ConfigValidation<()> {
        premortem::Validation::Success(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(clap::Args))]
pub struct OrgmodeConfig {
    /// Root directory containing .org files (scanned recursively)
    #[cfg_attr(
        not(target_arch = "wasm32"),
        arg(long = "orgmode-root-directory", env = "HOLON_ORGMODE_ROOT_DIRECTORY")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_directory: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(clap::Args))]
pub struct LoroPreferences {
    /// Enable the Loro CRDT layer
    #[cfg_attr(
        not(target_arch = "wasm32"),
        arg(long = "loro-enabled", env = "HOLON_LORO_ENABLED")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,

    /// Loro storage directory (default: `{orgmode_root}/.loro` or `{config_dir}/.loro`)
    #[cfg_attr(
        not(target_arch = "wasm32"),
        arg(long = "loro-storage-dir", env = "HOLON_LORO_STORAGE_DIR")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(clap::Args))]
pub struct UiConfig {
    /// Color theme name
    #[cfg_attr(
        not(target_arch = "wasm32"),
        arg(long = "ui-theme", env = "HOLON_UI_THEME")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,

    /// Frosted glass window effect (macOS Big Sur+ / Windows 11+)
    #[cfg_attr(
        not(target_arch = "wasm32"),
        arg(long = "ui-glass-background", env = "HOLON_UI_GLASS_BACKGROUND")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glass_background: Option<bool>,

    /// Per-widget UI state (open/closed, dimensions). Not settable via CLI.
    #[cfg_attr(not(target_arch = "wasm32"), arg(skip))]
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub widgets: HashMap<String, WidgetState>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(not(target_arch = "wasm32"), derive(clap::Args))]
pub struct HooksConfig {
    /// Shell command to run after writing an org file
    #[cfg_attr(
        not(target_arch = "wasm32"),
        arg(long = "hooks-post-org-write", env = "HOLON_HOOKS_POST_ORG_WRITE")
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_org_write: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct WidgetState {
    #[serde(default = "default_true")]
    pub open: bool,
    #[serde(default)]
    pub width: Option<f32>,
    #[serde(default)]
    pub height: Option<f32>,
}

fn default_true() -> bool {
    true
}

impl Default for WidgetState {
    fn default() -> Self {
        Self {
            open: true,
            width: None,
            height: None,
        }
    }
}

// ---------------------------------------------------------------------------
// SessionConfig — programmatic, per-frontend
// ---------------------------------------------------------------------------

/// Configuration that varies per frontend and is NOT user-facing.
/// Not stored in holon.toml, not settable via CLI.
#[derive(Clone)]
pub struct SessionConfig {
    /// Which widgets this frontend supports + screen dimensions.
    pub ui_info: holon_api::UiInfo,
    /// Wait for file watcher readiness before returning from `FrontendSession::new`.
    /// Mostly used by tests that assert on final state.
    pub wait_for_ready: bool,
    /// Use fake Todoist client (testing only). Registers the same DI path
    /// as production but with an in-memory fake instead of real API calls.
    pub todoist_fake: bool,
}

impl SessionConfig {
    pub fn new(ui_info: holon_api::UiInfo) -> Self {
        Self {
            ui_info,
            wait_for_ready: true,
            todoist_fake: false,
        }
    }

    pub fn without_wait(mut self) -> Self {
        self.wait_for_ready = false;
        self
    }

    pub fn with_todoist_fake(mut self) -> Self {
        self.todoist_fake = true;
        self
    }
}

// ---------------------------------------------------------------------------
// ClapSource — feeds clap-parsed values into premortem
// ---------------------------------------------------------------------------

/// A premortem `Source` that injects values parsed by clap (CLI args + env vars).
/// Only non-None fields are emitted, so unspecified args don't override the TOML file.
pub struct ClapSource(HolonConfig);

impl ClapSource {
    pub fn new(parsed: HolonConfig) -> Self {
        Self(parsed)
    }
}

impl Source for ClapSource {
    fn name(&self) -> &str {
        "cli"
    }

    fn load(&self, _env: &dyn ConfigEnv) -> std::result::Result<ConfigValues, ConfigErrors> {
        let table = toml::Value::try_from(&self.0).map_err(|e| {
            ConfigErrors::from(premortem::error::ConfigError::SourceError {
                source_name: "cli".into(),
                kind: SourceErrorKind::Other {
                    message: e.to_string(),
                },
            })
        })?;
        let mut values = ConfigValues::empty();
        flatten_toml("", &table, &mut values);
        Ok(values)
    }
}

/// Recursively flatten a TOML table into dotted-path `ConfigValues`.
/// Skips empty tables (which represent None options after skip_serializing_if).
fn flatten_toml(prefix: &str, value: &toml::Value, out: &mut ConfigValues) {
    match value {
        toml::Value::Table(map) => {
            for (k, v) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_toml(&path, v, out);
            }
        }
        toml::Value::String(s) => {
            out.insert(prefix.to_string(), ConfigValue::anonymous(s.as_str()));
        }
        toml::Value::Boolean(b) => {
            out.insert(prefix.to_string(), ConfigValue::anonymous(*b));
        }
        toml::Value::Integer(i) => {
            out.insert(prefix.to_string(), ConfigValue::anonymous(*i));
        }
        toml::Value::Float(f) => {
            out.insert(prefix.to_string(), ConfigValue::anonymous(*f));
        }
        toml::Value::Array(_) | toml::Value::Datetime(_) => {
            out.insert(
                prefix.to_string(),
                ConfigValue::anonymous(value.to_string()),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Config loading pipeline
// ---------------------------------------------------------------------------

/// Resolve the config directory from CLI/env or fall back to platform default.
pub fn resolve_config_dir(cli_override: Option<&Path>) -> PathBuf {
    if let Some(dir) = cli_override {
        return dir.to_path_buf();
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".config/holon");
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return PathBuf::from(xdg).join("holon");
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".config/holon");
        }
    }

    PathBuf::from(".holon")
}

/// Load configuration from the premortem pipeline:
/// `Defaults → holon.toml → ClapSource(CLI+env)`
///
/// Returns the traced config (for source queries) and the set of locked preference keys.
pub fn load_config(
    config_dir: &Path,
    cli_parsed: HolonConfig,
) -> Result<(TracedConfig<HolonConfig>, HashSet<PrefKey>)> {
    std::fs::create_dir_all(config_dir)?;

    let toml_path = config_dir.join("holon.toml");

    let traced = Config::<HolonConfig>::builder()
        .source(premortem::sources::Defaults::from(HolonConfig::default()))
        .source(premortem::sources::Toml::file(&toml_path))
        .source(ClapSource::new(cli_parsed))
        .build_traced()
        .map_err(|errors| anyhow::anyhow!("Config errors: {errors}"))?;

    let locked = extract_locked_keys(&traced);

    Ok((traced, locked))
}

/// Extract preference keys whose final value came from the "cli" source
/// (CLI args or env vars — both fed through ClapSource).
/// Iterates ALL traced paths so new fields are automatically covered.
fn extract_locked_keys(traced: &TracedConfig<HolonConfig>) -> HashSet<PrefKey> {
    traced
        .traces()
        .filter(|(_, trace)| trace.final_value.source.source == "cli")
        .map(|(path, _)| PrefKey::new(path))
        .collect()
}

// ---------------------------------------------------------------------------
// Config persistence (runtime preference changes)
// ---------------------------------------------------------------------------

/// Save a single preference value to `holon.toml`, preserving all other keys.
pub fn save_preference(config_dir: &Path, dotted_key: &str, value: toml::Value) -> Result<()> {
    let path = config_dir.join("holon.toml");

    let mut table: toml::Table = match std::fs::read_to_string(&path) {
        Ok(content) => content
            .parse::<toml::Table>()
            .unwrap_or_else(|_| toml::Table::new()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(e.into()),
    };

    let segments: Vec<&str> = dotted_key.split('.').collect();
    let (parent_segments, leaf) = segments.split_at(segments.len() - 1);

    let mut current = &mut table;
    for &seg in parent_segments {
        current = current
            .entry(seg)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .unwrap_or_else(|| panic!("Expected table at '{seg}' in holon.toml"));
    }
    current.insert(leaf[0].to_string(), value);

    let content = toml::to_string_pretty(&table)?;
    std::fs::write(&path, content)?;
    Ok(())
}

/// Save the full HolonConfig to `holon.toml`.
pub fn save_config(config_dir: &Path, config: &HolonConfig) -> Result<()> {
    let path = config_dir.join("holon.toml");
    let content = toml::to_string_pretty(config)?;
    std::fs::write(&path, content)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Resolved accessors
// ---------------------------------------------------------------------------

impl HolonConfig {
    pub fn resolve_db_path(&self, config_dir: &Path) -> PathBuf {
        self.db_path
            .clone()
            .unwrap_or_else(|| config_dir.join("holon.db"))
    }

    pub fn resolve_mcp_integrations_dir(&self, config_dir: &Path) -> Option<PathBuf> {
        Some(
            self.mcp_integrations_dir
                .clone()
                .unwrap_or_else(|| config_dir.join("integrations")),
        )
    }

    pub fn resolve_loro_storage_dir(&self, config_dir: &Path) -> PathBuf {
        self.loro
            .storage_dir
            .clone()
            .or_else(|| {
                self.orgmode
                    .root_directory
                    .as_ref()
                    .map(|r| r.join(".loro"))
            })
            .unwrap_or_else(|| config_dir.join(".loro"))
    }

    pub fn loro_enabled(&self) -> bool {
        self.loro.enabled.unwrap_or(false)
    }

    pub fn glass_background(&self) -> bool {
        self.ui.glass_background.unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Runtime persistence (load / save / preferences)
// ---------------------------------------------------------------------------

impl HolonConfig {
    /// Load config from `{config_dir}/holon.toml`.
    /// Returns `Default` if the file doesn't exist. On wasm32, always returns Default.
    pub fn load_runtime(config_dir: &Path) -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = config_dir;
            tracing::warn!("[HolonConfig] config file loading not supported on wasm32 — using built-in defaults");
            return Self::default();
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = config_dir.join("holon.toml");
            match std::fs::read_to_string(&path) {
                Ok(content) => toml::from_str(&content)
                    .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
                Err(e) => panic!("Failed to read {}: {}", path.display(), e),
            }
        }
    }

    /// Save config to `{config_dir}/holon.toml`, preserving any keys not owned by HolonConfig.
    /// On wasm32, logs a visible warning and no-ops — config persistence is not supported.
    pub fn save_runtime(&self, config_dir: &Path) {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = config_dir;
            tracing::warn!(
                "[HolonConfig] config save not supported on wasm32 — using in-memory config"
            );
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let path = config_dir.join("holon.toml");

            let mut table: toml::Table = match std::fs::read_to_string(&path) {
                Ok(content) => content
                    .parse::<toml::Table>()
                    .unwrap_or_else(|_| toml::Table::new()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
                Err(e) => panic!("Failed to read {}: {}", path.display(), e),
            };

            let our_toml = toml::to_string_pretty(self)
                .unwrap_or_else(|e| panic!("Failed to serialize config: {}", e));
            let our_table: toml::Table = our_toml
                .parse::<toml::Table>()
                .unwrap_or_else(|e| panic!("Failed to re-parse serialized config: {}", e));

            for (k, v) in our_table {
                table.insert(k, v);
            }

            let content = toml::to_string_pretty(&table)
                .unwrap_or_else(|e| panic!("Failed to serialize config: {}", e));
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap_or_else(|e| {
                    panic!("Failed to create config dir {}: {}", parent.display(), e)
                });
            }
            std::fs::write(&path, content)
                .unwrap_or_else(|e| panic!("Failed to write {}: {}", path.display(), e));
        }
    }

    /// Read a preference value, returning `None` if not set.
    pub fn get_preference(&self, key: &PrefKey) -> Option<&toml::Value> {
        self.preferences.get(key)
    }

    /// Set a preference value and sync to typed fields for backward compatibility.
    pub fn set_preference(&mut self, key: &PrefKey, value: toml::Value) {
        match key.as_str() {
            "ui.theme" => {
                self.ui.theme = match &value {
                    toml::Value::String(s) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                };
            }
            "ui.glass_background" => {
                self.ui.glass_background = Some(matches!(&value, toml::Value::Boolean(true)));
            }
            #[cfg(not(target_arch = "wasm32"))]
            "todoist.api_key" => {
                self.todoist.api_key = match &value {
                    toml::Value::String(s) if !s.is_empty() => Some(s.clone()),
                    _ => None,
                };
            }
            _ => {}
        }
        self.preferences.insert(key.clone(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_serializes_to_minimal_toml() {
        let config = HolonConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(
            !toml_str.contains("root_directory"),
            "None fields should be skipped: {toml_str}"
        );
    }

    #[test]
    fn flatten_toml_produces_dotted_paths() {
        let config = HolonConfig {
            orgmode: OrgmodeConfig {
                root_directory: Some(PathBuf::from("/org")),
            },
            ui: UiConfig {
                theme: Some("dracula".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let table = toml::Value::try_from(&config).unwrap();
        let mut values = ConfigValues::empty();
        flatten_toml("", &table, &mut values);

        assert!(values.contains("orgmode.root_directory"));
        assert!(values.contains("ui.theme"));
    }

    #[test]
    fn save_preference_creates_nested_tables() {
        let dir = tempfile::tempdir().unwrap();
        save_preference(
            dir.path(),
            "orgmode.root_directory",
            toml::Value::String("/org".into()),
        )
        .unwrap();

        let content = std::fs::read_to_string(dir.path().join("holon.toml")).unwrap();
        let table: toml::Table = content.parse().unwrap();
        let orgmode = table["orgmode"].as_table().unwrap();
        assert_eq!(orgmode["root_directory"].as_str().unwrap(), "/org");
    }

    #[test]
    fn save_preference_preserves_existing_keys() {
        let dir = tempfile::tempdir().unwrap();
        let initial = "[ui]\ntheme = \"light\"\n";
        std::fs::write(dir.path().join("holon.toml"), initial).unwrap();

        save_preference(
            dir.path(),
            "orgmode.root_directory",
            toml::Value::String("/org".into()),
        )
        .unwrap();

        let content = std::fs::read_to_string(dir.path().join("holon.toml")).unwrap();
        let table: toml::Table = content.parse().unwrap();
        assert_eq!(table["ui"]["theme"].as_str().unwrap(), "light");
        assert_eq!(table["orgmode"]["root_directory"].as_str().unwrap(), "/org");
    }

    #[test]
    fn resolve_db_path_defaults_to_config_dir() {
        let config = HolonConfig::default();
        let dir = Path::new("/home/user/.config/holon");
        assert_eq!(config.resolve_db_path(dir), dir.join("holon.db"));
    }

    #[test]
    fn resolve_db_path_uses_explicit() {
        let config = HolonConfig {
            db_path: Some(PathBuf::from("/custom/db.sqlite")),
            ..Default::default()
        };
        let dir = Path::new("/home/user/.config/holon");
        assert_eq!(
            config.resolve_db_path(dir),
            PathBuf::from("/custom/db.sqlite")
        );
    }
}
