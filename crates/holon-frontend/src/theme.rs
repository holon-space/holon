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

/// Fill of the collapsed-parent disclosure halo in the tree/sidebar.
///
/// A non-text state indicator: it must clear a 3:1 contrast ratio against the
/// surface it sits on ([`ThemeColors::sidebar_background`]) in EVERY theme, or
/// it silently stops communicating "this row hides something" (dogfood F1,
/// 2026-07-30: the previous fill measured 1.05:1 and was invisible).
/// The GPUI builder reaches this same colour as `theme.muted_foreground`, which
/// `apply_holon_theme` maps from `text_secondary`.
pub fn collapsed_halo_fill(c: &ThemeColors) -> Rgba8 {
    c.text_secondary
}

/// The disclosure glyph knocked out of the halo. Sits ON the fill, so it must
/// clear the same floor against [`collapsed_halo_fill`], not against the page.
pub fn collapsed_halo_glyph(c: &ThemeColors) -> Rgba8 {
    c.background
}

/// WCAG 2.x relative-luminance contrast ratio between two colours,
/// `1.0..=21.0`.
///
/// Alpha is ignored: every colour here is composited over an opaque surface
/// before it reaches a pixel, and the ratio is judged on that result.
pub fn contrast_ratio(a: Rgba8, b: Rgba8) -> f32 {
    fn channel(c: u8) -> f32 {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    fn luminance(c: Rgba8) -> f32 {
        0.2126 * channel(c[0]) + 0.7152 * channel(c[1]) + 0.0722 * channel(c[2])
    }
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[derive(Clone, Debug)]
pub struct ThemeDef {
    pub name: String,
    pub is_dark: bool,
    pub colors: ThemeColors,
}

#[derive(Clone)]
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
            parse_theme_yaml(yaml, &mut themes).expect("builtin theme asset must be valid");
        }

        if let Some(dir) = user_themes_dir {
            if dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
                            if let Ok(content) = std::fs::read_to_string(&path) {
                                if let Err(e) = parse_theme_yaml(&content, &mut themes) {
                                    tracing::error!(
                                        "Skipping user theme file {}: {e}",
                                        path.display()
                                    );
                                }
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

fn parse_theme_yaml(yaml: &str, out: &mut HashMap<String, ThemeDef>) -> Result<(), String> {
    let file: ThemeFile =
        serde_yaml::from_str(yaml).map_err(|e| format!("failed to parse theme YAML: {e}"))?;

    for (key, entry) in file.themes {
        let hex = |name: &str, value: &str| -> Result<Rgba8, String> {
            parse_hex(value)
                .map_err(|e| format!("theme '{key}': invalid color {name}: '{value}': {e}"))
        };
        let c = &entry.colors;
        let colors = ThemeColors {
            primary: hex("primary", &c.primary)?,
            primary_dark: hex("primaryDark", &c.primary_dark)?,
            primary_light: hex("primaryLight", &c.primary_light)?,
            text_primary: hex("textPrimary", &c.text_primary)?,
            text_secondary: hex("textSecondary", &c.text_secondary)?,
            text_tertiary: hex("textTertiary", &c.text_tertiary)?,
            background: hex("background", &c.background)?,
            background_secondary: hex("backgroundSecondary", &c.background_secondary)?,
            sidebar_background: hex("sidebarBackground", &c.sidebar_background)?,
            border: hex("border", &c.border)?,
            border_focus: hex("borderFocus", &c.border_focus)?,
            success: hex("success", &c.success)?,
            error: hex("error", &c.error)?,
            warning: hex("warning", &c.warning)?,
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
    Ok(())
}

/// Parse a `#RGB` / `#RGBA` / `#RRGGBB` / `#RRGGBBAA` hex color.
fn parse_hex(s: &str) -> Result<Rgba8, String> {
    let s = s.trim_start_matches('#');
    if !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("non-hex character".into());
    }
    let nibble = |i: usize| u8::from_str_radix(&s[i..i + 1], 16).expect("validated hex digit");
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).expect("validated hex digits");
    match s.len() {
        3 => Ok([nibble(0) * 17, nibble(1) * 17, nibble(2) * 17, 255]),
        4 => Ok([
            nibble(0) * 17,
            nibble(1) * 17,
            nibble(2) * 17,
            nibble(3) * 17,
        ]),
        6 => Ok([byte(0), byte(2), byte(4), 255]),
        8 => Ok([byte(0), byte(2), byte(4), byte(6)]),
        n => Err(format!("expected 3, 4, 6 or 8 hex digits, got {n}")),
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
        assert_eq!(parse_hex("#FF0000"), Ok([255, 0, 0, 255]));
        assert_eq!(parse_hex("#00FF00"), Ok([0, 255, 0, 255]));
    }

    #[test]
    fn test_parse_hex_rgba() {
        assert_eq!(parse_hex("#FF0000E6"), Ok([255, 0, 0, 230]));
    }

    #[test]
    fn test_parse_hex_shorthand() {
        assert_eq!(parse_hex("#abc"), Ok([0xAA, 0xBB, 0xCC, 255]));
        assert_eq!(parse_hex("#abcd"), Ok([0xAA, 0xBB, 0xCC, 0xDD]));
    }

    #[test]
    fn test_parse_hex_invalid_is_err() {
        assert!(parse_hex("#zzzzzz").is_err()); // non-hex
        assert!(parse_hex("#abcde").is_err()); // bad length
        assert!(parse_hex("#ééé").is_err()); // multibyte, no panic
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
