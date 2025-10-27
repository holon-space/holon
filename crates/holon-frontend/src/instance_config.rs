use std::collections::HashMap;
use std::path::Path;

use holon_todoist::di::TodoistConfig;

use crate::preferences::PrefKey;

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct InstanceConfig {
    #[serde(default)]
    pub hooks: HooksConfig,
    #[serde(default)]
    pub todoist: TodoistConfig,
    #[serde(default)]
    pub ui: UiSettings,
    /// Flat key-value store for all preferences. Keys use dotted notation ("ui.theme").
    /// This is the canonical store that the settings UI reads/writes.
    #[serde(default)]
    pub preferences: HashMap<PrefKey, toml::Value>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct HooksConfig {
    pub post_org_write: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct UiSettings {
    pub theme: Option<String>,
    /// When true, the window uses a translucent blurred background (frosted glass effect).
    /// Only effective on macOS Big Sur+ and Windows 11+.
    #[serde(default)]
    pub glass_background: bool,
    /// Per-widget state keyed by block ID. Any collapsible/resizable widget
    /// can store its state here.
    #[serde(default)]
    pub widgets: HashMap<String, WidgetState>,
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

impl InstanceConfig {
    /// Load instance config from `{db_dir}/holon.toml`.
    /// Returns `Default` if the file doesn't exist. Panics on parse errors.
    pub fn load(db_dir: &Path) -> Self {
        let path = db_dir.join("holon.toml");
        match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content)
                .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => panic!("Failed to read {}: {}", path.display(), e),
        }
    }

    /// Save config back to `{dir}/holon.toml`.
    pub fn save(&self, dir: &Path) {
        let path = dir.join("holon.toml");
        let content = toml::to_string_pretty(self)
            .unwrap_or_else(|e| panic!("Failed to serialize config: {}", e));
        std::fs::write(&path, content)
            .unwrap_or_else(|e| panic!("Failed to write {}: {}", path.display(), e));
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
                self.ui.glass_background = matches!(&value, toml::Value::Boolean(true));
            }
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
