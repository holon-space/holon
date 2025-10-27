use std::collections::HashMap;
use std::path::Path;

pub type Rgba8 = [u8; 4];

#[derive(Clone, Debug)]
pub struct ThemeColors {
    pub primary: Rgba8,
    pub primary_dark: Rgba8,
    pub primary_light: Rgba8,
    pub text_primary: Rgba8,
    pub text_secondary: Rgba8,
    pub text_tertiary: Rgba8,
    pub background: Rgba8,
    pub background_secondary: Rgba8,
    pub sidebar_background: Rgba8,
    pub border: Rgba8,
    pub border_focus: Rgba8,
    pub success: Rgba8,
    pub error: Rgba8,
    pub warning: Rgba8,
}

#[derive(Clone, Debug)]
pub struct ThemeDef {
    pub name: String,
    pub is_dark: bool,
    pub colors: ThemeColors,
}

pub struct ThemeRegistry {
    themes: HashMap<String, ThemeDef>,
}

impl ThemeRegistry {
    pub fn load(user_themes_dir: Option<&Path>) -> Self {
        let mut themes = HashMap::new();

        let builtins: &[&str] = &[
            include_str!("../../../assets/themes/holon.yaml"),
            include_str!("../../../assets/themes/catppuccin.yaml"),
            include_str!("../../../assets/themes/dracula.yaml"),
            include_str!("../../../assets/themes/github.yaml"),
            include_str!("../../../assets/themes/gruvbox.yaml"),
            include_str!("../../../assets/themes/monokai.yaml"),
            include_str!("../../../assets/themes/nord.yaml"),
            include_str!("../../../assets/themes/onedark.yaml"),
            include_str!("../../../assets/themes/solarized.yaml"),
            include_str!("../../../assets/themes/tomorrow.yaml"),
            include_str!("../../../assets/themes/default.yaml"),
        ];

        for yaml in builtins {
            parse_theme_yaml(yaml, &mut themes);
        }

        if let Some(dir) = user_themes_dir {
            if dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path
                            .extension()
                            .map_or(false, |e| e == "yaml" || e == "yml")
                        {
                            if let Ok(content) = std::fs::read_to_string(&path) {
                                parse_theme_yaml(&content, &mut themes);
                            }
                        }
                    }
                }
            }
        }

        Self { themes }
    }

    pub fn get(&self, name: &str) -> Option<&ThemeDef> {
        self.themes.get(name)
    }

    pub fn available(&self) -> Vec<(&str, bool)> {
        let mut result: Vec<_> = self
            .themes
            .iter()
            .map(|(k, v)| (k.as_str(), v.is_dark))
            .collect();
        result.sort_by_key(|(name, _)| *name);
        result
    }
}

#[derive(serde::Deserialize)]
struct ThemeFile {
    themes: HashMap<String, ThemeEntry>,
}

#[derive(serde::Deserialize)]
struct ThemeEntry {
    name: String,
    #[serde(rename = "isDark")]
    is_dark: bool,
    colors: ColorEntries,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ColorEntries {
    primary: String,
    primary_dark: String,
    primary_light: String,
    text_primary: String,
    text_secondary: String,
    text_tertiary: String,
    background: String,
    background_secondary: String,
    sidebar_background: String,
    border: String,
    border_focus: String,
    success: String,
    error: String,
    warning: String,
}

fn parse_theme_yaml(yaml: &str, out: &mut HashMap<String, ThemeDef>) {
    let file: ThemeFile = match serde_yaml::from_str(yaml) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("Failed to parse theme YAML: {e}");
            return;
        }
    };

    for (key, entry) in file.themes {
        let colors = ThemeColors {
            primary: parse_hex(&entry.colors.primary),
            primary_dark: parse_hex(&entry.colors.primary_dark),
            primary_light: parse_hex(&entry.colors.primary_light),
            text_primary: parse_hex(&entry.colors.text_primary),
            text_secondary: parse_hex(&entry.colors.text_secondary),
            text_tertiary: parse_hex(&entry.colors.text_tertiary),
            background: parse_hex(&entry.colors.background),
            background_secondary: parse_hex(&entry.colors.background_secondary),
            sidebar_background: parse_hex(&entry.colors.sidebar_background),
            border: parse_hex(&entry.colors.border),
            border_focus: parse_hex(&entry.colors.border_focus),
            success: parse_hex(&entry.colors.success),
            error: parse_hex(&entry.colors.error),
            warning: parse_hex(&entry.colors.warning),
        };

        out.insert(
            key,
            ThemeDef {
                name: entry.name,
                is_dark: entry.is_dark,
                colors,
            },
        );
    }
}

fn parse_hex(s: &str) -> Rgba8 {
    let s = s.trim_start_matches('#');
    let bytes: Vec<u8> = (0..s.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect();
    match bytes.len() {
        3 => [bytes[0], bytes[1], bytes[2], 255],
        4 => [bytes[0], bytes[1], bytes[2], bytes[3]],
        _ => [255, 0, 255, 255],
    }
}

impl ThemeColors {
    #[cfg(feature = "blinc")]
    fn to_f32(c: Rgba8) -> [f32; 4] {
        [
            c[0] as f32 / 255.0,
            c[1] as f32 / 255.0,
            c[2] as f32 / 255.0,
            c[3] as f32 / 255.0,
        ]
    }

    #[cfg(feature = "blinc")]
    fn lighten(c: Rgba8, amount: f32) -> Rgba8 {
        [
            (c[0] as f32 + (255.0 - c[0] as f32) * amount) as u8,
            (c[1] as f32 + (255.0 - c[1] as f32) * amount) as u8,
            (c[2] as f32 + (255.0 - c[2] as f32) * amount) as u8,
            c[3],
        ]
    }

    #[cfg(feature = "blinc")]
    fn darken(c: Rgba8, amount: f32) -> Rgba8 {
        [
            (c[0] as f32 * (1.0 - amount)) as u8,
            (c[1] as f32 * (1.0 - amount)) as u8,
            (c[2] as f32 * (1.0 - amount)) as u8,
            c[3],
        ]
    }

    #[cfg(feature = "blinc")]
    fn tint_bg(color: Rgba8, bg: Rgba8) -> Rgba8 {
        let alpha = 0.10;
        [
            (bg[0] as f32 * (1.0 - alpha) + color[0] as f32 * alpha) as u8,
            (bg[1] as f32 * (1.0 - alpha) + color[1] as f32 * alpha) as u8,
            (bg[2] as f32 * (1.0 - alpha) + color[2] as f32 * alpha) as u8,
            255,
        ]
    }
}

#[cfg(feature = "blinc")]
impl ThemeColors {
    pub fn to_blinc_color(rgba: Rgba8) -> blinc_core::Color {
        let f = Self::to_f32(rgba);
        blinc_core::Color::rgba(f[0], f[1], f[2], f[3])
    }

    pub fn to_blinc_color_tokens(&self) -> blinc_theme::ColorTokens {
        let bg = self.background;
        blinc_theme::ColorTokens {
            primary: Self::to_blinc_color(self.primary),
            primary_hover: Self::to_blinc_color(Self::darken(self.primary, 0.1)),
            primary_active: Self::to_blinc_color(Self::darken(self.primary, 0.2)),
            secondary: Self::to_blinc_color(self.primary_dark),
            secondary_hover: Self::to_blinc_color(Self::darken(self.primary_dark, 0.1)),
            secondary_active: Self::to_blinc_color(Self::darken(self.primary_dark, 0.2)),
            success: Self::to_blinc_color(self.success),
            success_bg: Self::to_blinc_color(Self::tint_bg(self.success, bg)),
            warning: Self::to_blinc_color(self.warning),
            warning_bg: Self::to_blinc_color(Self::tint_bg(self.warning, bg)),
            error: Self::to_blinc_color(self.error),
            error_bg: Self::to_blinc_color(Self::tint_bg(self.error, bg)),
            info: Self::to_blinc_color(self.primary_light),
            info_bg: Self::to_blinc_color(Self::tint_bg(self.primary_light, bg)),
            background: Self::to_blinc_color(self.background),
            surface: Self::to_blinc_color(self.sidebar_background),
            surface_elevated: Self::to_blinc_color(Self::lighten(self.background_secondary, 0.05)),
            surface_overlay: Self::to_blinc_color(Self::darken(self.background_secondary, 0.05)),
            text_primary: Self::to_blinc_color(self.text_primary),
            text_secondary: Self::to_blinc_color(self.text_secondary),
            text_tertiary: Self::to_blinc_color(self.text_tertiary),
            text_inverse: Self::to_blinc_color(self.background),
            text_link: Self::to_blinc_color(self.primary),
            border: Self::to_blinc_color(self.border),
            border_secondary: Self::to_blinc_color(Self::lighten(self.border, 0.15)),
            border_hover: Self::to_blinc_color(Self::darken(self.border, 0.1)),
            border_focus: Self::to_blinc_color(self.border_focus),
            border_error: Self::to_blinc_color(self.error),
            input_bg: Self::to_blinc_color(self.background_secondary),
            input_bg_hover: Self::to_blinc_color(Self::lighten(self.background_secondary, 0.05)),
            input_bg_focus: Self::to_blinc_color(self.background),
            input_bg_disabled: Self::to_blinc_color(Self::darken(self.background_secondary, 0.1)),
            selection: Self::to_blinc_color(Self::lighten(self.primary, 0.6)),
            selection_text: Self::to_blinc_color(self.text_primary),
            accent: Self::to_blinc_color(self.primary),
            accent_subtle: Self::to_blinc_color(Self::tint_bg(self.primary, bg)),
            tooltip_bg: Self::to_blinc_color(self.text_primary),
            tooltip_text: Self::to_blinc_color(self.background),
        }
    }
}

impl ThemeColors {
    pub fn default_dark() -> Self {
        ThemeRegistry::load(None)
            .get("holonDark")
            .expect("holonDark builtin missing")
            .colors
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_rgb() {
        assert_eq!(parse_hex("#FF0000"), [255, 0, 0, 255]);
        assert_eq!(parse_hex("#00FF00"), [0, 255, 0, 255]);
    }

    #[test]
    fn test_parse_hex_rgba() {
        assert_eq!(parse_hex("#FF0000E6"), [255, 0, 0, 230]);
    }

    #[test]
    fn test_load_builtin_themes() {
        let registry = ThemeRegistry::load(None);
        assert!(registry.get("holonDark").is_some());
        assert!(registry.get("holonLight").is_some());
        assert!(registry.get("nordDark").is_some());
        let available = registry.available();
        assert!(available.len() >= 10);
    }

    #[test]
    fn test_theme_colors_correct() {
        let registry = ThemeRegistry::load(None);
        let dark = registry.get("holonDark").unwrap();
        assert!(dark.is_dark);
        assert_eq!(dark.colors.primary[0], 0x5D);
        assert_eq!(dark.colors.primary[1], 0xBD);
        assert_eq!(dark.colors.primary[2], 0xBD);
    }
}
