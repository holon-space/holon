use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::theme::ThemeRegistry;

/// Dotted preference key, e.g. "ui.theme". Validated at construction.
///
/// Invariant: non-empty, only alphanumeric + dots + underscores, no leading/trailing dots.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PrefKey(String);

impl PrefKey {
    pub fn new(raw: &str) -> Self {
        assert!(
            !raw.is_empty()
                && raw
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '.' || c == '_')
                && !raw.starts_with('.')
                && !raw.ends_with('.'),
            "Invalid preference key: {raw:?}"
        );
        Self(raw.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PrefKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for PrefKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PrefKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(Self::new(&s))
    }
}

/// A named section that groups preferences in the UI. Validated at construction.
///
/// Invariant: non-empty, human-readable label.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PrefSection(String);

impl PrefSection {
    pub fn new(label: &str) -> Self {
        assert!(
            !label.is_empty(),
            "Preference section label must not be empty"
        );
        Self(label.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PrefSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone)]
pub struct ChoiceOption {
    pub value: String,
    pub label: String,
}

#[derive(Clone)]
pub enum PrefType {
    /// Dropdown with fixed set of choices
    Choice(Vec<ChoiceOption>),
    /// Obscured text input (API keys, passwords)
    Secret,
    /// Plain text input
    Text,
    /// Boolean toggle
    Toggle,
    /// Path selected by platform file picker
    DirectoryPath,
}

/// A single preference definition.
///
/// The schema is built once at session startup from `define_preferences()`.
/// Frontends use it to generate settings UI via the render interpreter.
#[derive(Clone)]
pub struct PreferenceDef {
    pub key: PrefKey,
    pub label: String,
    pub description: String,
    pub section: PrefSection,
    pub pref_type: PrefType,
    pub default: toml::Value,
    pub requires_restart: bool,
}

/// Build the complete preference schema.
///
/// Theme choices are populated dynamically from the `ThemeRegistry`.
pub fn define_preferences(theme_registry: &ThemeRegistry) -> Vec<PreferenceDef> {
    let appearance = PrefSection::new("Appearance");
    let integrations = PrefSection::new("Integrations");
    let data = PrefSection::new("Data");

    let theme_options: Vec<ChoiceOption> = theme_registry
        .available()
        .into_iter()
        .map(|(name, is_dark)| {
            let suffix = if is_dark { " (Dark)" } else { " (Light)" };
            ChoiceOption {
                value: name.to_string(),
                label: format!("{name}{suffix}"),
            }
        })
        .collect();

    vec![
        PreferenceDef {
            key: PrefKey::new("ui.theme"),
            label: "Theme".into(),
            description: "Choose your preferred color theme.".into(),
            section: appearance.clone(),
            pref_type: PrefType::Choice(theme_options),
            default: toml::Value::String("holonLight".into()),
            requires_restart: false,
        },
        PreferenceDef {
            key: PrefKey::new("ui.glass_background"),
            label: "Glass Background".into(),
            description: "Frosted glass window effect — blurs the desktop behind the app.".into(),
            section: appearance,
            pref_type: PrefType::Toggle,
            default: toml::Value::Boolean(false),
            requires_restart: true,
        },
        PreferenceDef {
            key: PrefKey::new("todoist.api_key"),
            label: "Todoist API Key".into(),
            description: "Enter your Todoist API key to sync tasks. Find it in Todoist Settings > Integrations.".into(),
            section: integrations,
            pref_type: PrefType::Secret,
            default: toml::Value::String(String::new()),
            requires_restart: true,
        },
        PreferenceDef {
            key: PrefKey::new("orgmode.root_directory"),
            label: "OrgMode Directory".into(),
            description: "Select the root directory containing your .org files. The directory will be scanned recursively.".into(),
            section: data,
            pref_type: PrefType::DirectoryPath,
            default: toml::Value::String(String::new()),
            requires_restart: true,
        },
    ]
}

/// Convert a `toml::Value` to `holon_api::Value` for use in render data rows.
pub fn value_to_toml(v: &holon_api::Value) -> toml::Value {
    match v {
        holon_api::Value::String(s) => toml::Value::String(s.clone()),
        holon_api::Value::Integer(i) => toml::Value::Integer(*i),
        holon_api::Value::Float(f) => toml::Value::Float(*f),
        holon_api::Value::Boolean(b) => toml::Value::Boolean(*b),
        other => toml::Value::String(format!("{other:?}")),
    }
}

pub fn toml_to_api_value(v: &toml::Value) -> holon_api::Value {
    match v {
        toml::Value::String(s) => holon_api::Value::String(s.clone()),
        toml::Value::Integer(i) => holon_api::Value::Integer(*i),
        toml::Value::Float(f) => holon_api::Value::Float(*f),
        toml::Value::Boolean(b) => holon_api::Value::Boolean(*b),
        _ => holon_api::Value::String(v.to_string()),
    }
}

/// Generate render data rows from preference definitions and current values.
///
/// Each row represents one preference field with columns:
/// - `key`: the dotted key string
/// - `value`: current value (or default)
/// - `label`: display name
/// - `description`: help text
/// - `section`: section label
/// - `pref_type`: type discriminant ("choice", "secret", "text", "toggle", "directory_path")
/// - `requires_restart`: boolean
/// - `options`: JSON array of `{value, label}` for Choice type (empty array otherwise)
pub fn preferences_to_rows(
    defs: &[PreferenceDef],
    current: &HashMap<PrefKey, toml::Value>,
    locked: &HashSet<PrefKey>,
) -> Vec<HashMap<String, holon_api::Value>> {
    defs.iter()
        .map(|def| {
            let value = current.get(&def.key).unwrap_or(&def.default);

            let type_str = match &def.pref_type {
                PrefType::Choice(_) => "choice",
                PrefType::Secret => "secret",
                PrefType::Text => "text",
                PrefType::Toggle => "toggle",
                PrefType::DirectoryPath => "directory_path",
            };

            let options = match &def.pref_type {
                PrefType::Choice(opts) => holon_api::Value::Array(
                    opts.iter()
                        .map(|o| {
                            holon_api::Value::Object(HashMap::from([
                                ("value".into(), holon_api::Value::String(o.value.clone())),
                                ("label".into(), holon_api::Value::String(o.label.clone())),
                            ]))
                        })
                        .collect(),
                ),
                _ => holon_api::Value::Array(vec![]),
            };

            HashMap::from([
                (
                    "key".into(),
                    holon_api::Value::String(def.key.as_str().into()),
                ),
                ("value".into(), toml_to_api_value(value)),
                ("label".into(), holon_api::Value::String(def.label.clone())),
                (
                    "description".into(),
                    holon_api::Value::String(def.description.clone()),
                ),
                (
                    "section".into(),
                    holon_api::Value::String(def.section.as_str().into()),
                ),
                (
                    "pref_type".into(),
                    holon_api::Value::String(type_str.into()),
                ),
                (
                    "requires_restart".into(),
                    holon_api::Value::Boolean(def.requires_restart),
                ),
                ("options".into(), options),
                (
                    "locked".into(),
                    holon_api::Value::Boolean(locked.contains(&def.key)),
                ),
            ])
        })
        .collect()
}

/// Generate a `RenderExpr` tree for the preferences UI.
///
/// Groups preferences by section, produces:
/// ```text
/// col(children: [
///     section(#{title: "Appearance"}, children: [pref_field(...), ...]),
///     section(#{title: "Integrations"}, children: [pref_field(...), ...]),
///     ...
/// ])
/// ```
pub fn preferences_render_expr(defs: &[PreferenceDef]) -> holon_api::render_types::RenderExpr {
    use holon_api::render_types::{Arg, RenderExpr};

    // Group defs by section (preserving order of first appearance)
    let mut section_order: Vec<&PrefSection> = Vec::new();
    let mut by_section: HashMap<&PrefSection, Vec<&PreferenceDef>> = HashMap::new();
    for def in defs {
        if !by_section.contains_key(&def.section) {
            section_order.push(&def.section);
        }
        by_section.entry(&def.section).or_default().push(def);
    }

    let section_exprs: Vec<RenderExpr> = section_order
        .into_iter()
        .map(|section| {
            let pref_fields: Vec<RenderExpr> = by_section[section]
                .iter()
                .map(|def| {
                    let type_str = match &def.pref_type {
                        PrefType::Choice(_) => "choice",
                        PrefType::Secret => "secret",
                        PrefType::Text => "text",
                        PrefType::Toggle => "toggle",
                        PrefType::DirectoryPath => "directory_path",
                    };

                    RenderExpr::FunctionCall {
                        name: "pref_field".into(),
                        args: vec![
                            Arg {
                                name: Some("key".into()),
                                value: RenderExpr::Literal {
                                    value: holon_api::Value::String(def.key.as_str().into()),
                                },
                            },
                            Arg {
                                name: Some("pref_type".into()),
                                value: RenderExpr::Literal {
                                    value: holon_api::Value::String(type_str.into()),
                                },
                            },
                            Arg {
                                name: Some("requires_restart".into()),
                                value: RenderExpr::Literal {
                                    value: holon_api::Value::Boolean(def.requires_restart),
                                },
                            },
                        ],
                    }
                })
                .collect();

            {
                let mut section_args = vec![Arg {
                    name: Some("title".into()),
                    value: RenderExpr::Literal {
                        value: holon_api::Value::String(section.as_str().into()),
                    },
                }];
                // Each pref_field as an individual positional arg (not wrapped in Array)
                for pf in pref_fields {
                    section_args.push(Arg {
                        name: None,
                        value: pf,
                    });
                }
                RenderExpr::FunctionCall {
                    name: "section".into(),
                    args: section_args,
                }
            }
        })
        .collect();

    // Each section as an individual positional arg (not wrapped in Array)
    let column_args: Vec<Arg> = section_exprs
        .into_iter()
        .map(|s| Arg {
            name: None,
            value: s,
        })
        .collect();
    RenderExpr::FunctionCall {
        name: "column".into(),
        args: column_args,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pref_key_valid() {
        let k = PrefKey::new("ui.theme");
        assert_eq!(k.as_str(), "ui.theme");
        assert_eq!(k.to_string(), "ui.theme");
    }

    #[test]
    fn pref_key_single_segment() {
        let k = PrefKey::new("theme");
        assert_eq!(k.as_str(), "theme");
    }

    #[test]
    #[should_panic(expected = "Invalid preference key")]
    fn pref_key_empty() {
        PrefKey::new("");
    }

    #[test]
    #[should_panic(expected = "Invalid preference key")]
    fn pref_key_leading_dot() {
        PrefKey::new(".ui.theme");
    }

    #[test]
    #[should_panic(expected = "Invalid preference key")]
    fn pref_key_trailing_dot() {
        PrefKey::new("ui.theme.");
    }

    #[test]
    #[should_panic(expected = "Invalid preference key")]
    fn pref_key_spaces() {
        PrefKey::new("ui theme");
    }

    #[test]
    fn pref_section_valid() {
        let s = PrefSection::new("Appearance");
        assert_eq!(s.as_str(), "Appearance");
    }

    #[test]
    #[should_panic(expected = "must not be empty")]
    fn pref_section_empty() {
        PrefSection::new("");
    }

    #[test]
    fn pref_key_serde_roundtrip() {
        let key = PrefKey::new("ui.theme");
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, "\"ui.theme\"");
        let back: PrefKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, key);
    }

    #[test]
    fn define_preferences_produces_all_sections() {
        let registry = ThemeRegistry::load(None);
        let defs = define_preferences(&registry);

        assert!(defs.len() >= 3);

        let sections: Vec<&str> = defs.iter().map(|d| d.section.as_str()).collect();
        assert!(sections.contains(&"Appearance"));
        assert!(sections.contains(&"Integrations"));
        assert!(sections.contains(&"Data"));
    }

    #[test]
    fn theme_choice_has_options() {
        let registry = ThemeRegistry::load(None);
        let defs = define_preferences(&registry);

        let theme_def = defs.iter().find(|d| d.key.as_str() == "ui.theme").unwrap();
        match &theme_def.pref_type {
            PrefType::Choice(options) => {
                assert!(!options.is_empty());
                assert!(options.iter().any(|o| o.value == "holonLight"));
            }
            _ => panic!("Expected Choice type for ui.theme"),
        }
    }

    #[test]
    fn preferences_to_rows_uses_defaults() {
        let registry = ThemeRegistry::load(None);
        let defs = define_preferences(&registry);
        let empty: HashMap<PrefKey, toml::Value> = HashMap::new();
        let rows = preferences_to_rows(&defs, &empty, &HashSet::new());

        assert_eq!(rows.len(), defs.len());

        let theme_row = rows
            .iter()
            .find(|r| matches!(r.get("key"), Some(holon_api::Value::String(k)) if k == "ui.theme"))
            .unwrap();
        assert_eq!(
            theme_row.get("value"),
            Some(&holon_api::Value::String("holonLight".into()))
        );
        assert_eq!(
            theme_row.get("pref_type"),
            Some(&holon_api::Value::String("choice".into()))
        );
    }

    #[test]
    fn preferences_to_rows_uses_overrides() {
        let registry = ThemeRegistry::load(None);
        let defs = define_preferences(&registry);
        let overrides = HashMap::from([(
            PrefKey::new("ui.theme"),
            toml::Value::String("dracula".into()),
        )]);
        let rows = preferences_to_rows(&defs, &overrides, &HashSet::new());

        let theme_row = rows
            .iter()
            .find(|r| matches!(r.get("key"), Some(holon_api::Value::String(k)) if k == "ui.theme"))
            .unwrap();
        assert_eq!(
            theme_row.get("value"),
            Some(&holon_api::Value::String("dracula".into()))
        );
    }

    #[test]
    fn preferences_render_expr_structure() {
        let registry = ThemeRegistry::load(None);
        let defs = define_preferences(&registry);
        let expr = preferences_render_expr(&defs);

        // Top-level is column() with section children
        match &expr {
            holon_api::render_types::RenderExpr::FunctionCall { name, args, .. } => {
                assert_eq!(name, "column");
                assert!(!args.is_empty());
            }
            _ => panic!("Expected FunctionCall at root"),
        }
    }
}
