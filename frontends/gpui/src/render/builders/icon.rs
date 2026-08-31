use std::path::PathBuf;
use std::sync::OnceLock;

use gpui::ImageSource;
use gpui::Resource;
use gpui::StyledImage;
use gpui::img;

use super::prelude::*;

/// Icons directory resolved once at startup.
/// Dev: CARGO_MANIFEST_DIR/assets/icons (symlinked to workspace root).
/// Release: next to binary, or HOLON_WORKSPACE_ROOT override.
fn icons_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Ok(root) = std::env::var("HOLON_WORKSPACE_ROOT") {
            return PathBuf::from(root).join("assets/icons");
        }
        if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
            return PathBuf::from(manifest).join("assets/icons");
        }
        match std::env::current_exe() {
            Ok(p) => p
                .parent()
                .map(|d| d.join("assets/icons"))
                .unwrap_or_else(|| {
                    tracing::warn!(
                        target: "holon.icons",
                        "current_exe has no parent directory; falling back to ./assets/icons \
                         — icons may render as unicode glyphs"
                    );
                    PathBuf::from("assets/icons")
                }),
            Err(e) => {
                tracing::warn!(
                    target: "holon.icons",
                    error = %e,
                    "std::env::current_exe() failed; falling back to ./assets/icons \
                     — icons may render as unicode glyphs"
                );
                PathBuf::from("assets/icons")
            }
        }
    })
}

/// Map icon name to the SVG filename in assets/icons/ (from manifest.toml).
/// Returns None for icons that don't have an SVG counterpart.
fn icon_svg_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "folder" | "directory" => "folder",
        "folder_open" | "directory_open" => "folder-open",
        "file" | "document" | "file_text" | "document_text" => "page-facing-up",
        "check" | "checkbox" => "check-mark",
        "close" | "x" | "error" => "cross-mark",
        "search" => "magnifying-glass",
        "settings" | "gear" => "gear",
        "star" => "star",
        "tag" | "label" => "label",
        "clock" | "time" => "hourglass",
        "calendar" => "calendar",
        "warning" | "alert" => "warning",
        "link" => "link",
        "eye" | "visible" => "eye",
        "bell" | "notification" => "bell",
        "bookmark" => "bookmark",
        "clipboard" => "clipboard",
        "memo" | "note" => "memo",
        "scroll" => "scroll",
        "inbox" => "inbox",
        "outbox" => "outbox",
        "bar_chart" | "chart" => "bar-chart",
        "light_bulb" | "idea" => "light-bulb",
        "fire" | "hot" => "fire",
        "speech" | "comment" => "speech-bubble",
        "thought" => "thought-bubble",
        "robot" | "ai" => "robot",
        "refresh" | "sync" | "cycle" => "cycle",
        "sparkles" | "magic" => "sparkles",
        "pushpin" | "pin" => "pushpin",
        "arrow_right" | "right" => "right-arrow",
        "notebook" => "notebook",
        "tree" | "list_tree" | "outliner" | "books" => "books",
        "table" | "table_2" | "grid" | "abacus" => "abacus",
        _ => return None,
    })
}

/// Icon-name → Unicode glyph, used when no SVG file exists for the name. The
/// single source of truth swept by the icon-font coverage test so every glyph
/// is asserted Android-renderable. Glyphs the embedded DejaVu font can't render
/// (🗑 trash/delete) are substituted on Android via `crate::ICON_SUBSTITUTES`;
/// 🔒/🔓 have no DejaVu substitute and are documented in
/// `crate::KNOWN_ANDROID_GLYPH_GAPS` (unreached — no layout names them today).
pub(crate) const ICON_CHARS: &[(&str, &str)] = &[
    ("orgmode", "\u{25C9}"),       // ◉
    ("circle", "\u{25CF}"),        // ●
    ("chevron_right", "\u{203A}"), // ›
    ("chevron_down", "\u{2304}"),  // ⌄
    ("chevron_left", "\u{2039}"),  // ‹
    ("chevron_up", "\u{2303}"),    // ⌃
    ("plus", "+"),
    ("add", "+"),
    ("minus", "\u{2212}"),          // −
    ("remove", "\u{2212}"),         // −
    ("info", "\u{2139}"),           // ℹ
    ("code", "\u{27E8}\u{27E9}"),   // ⟨⟩
    ("source", "\u{27E8}\u{27E9}"), // ⟨⟩
    ("list", "\u{2630}"),           // ☰
    ("menu", "\u{2630}"),           // ☰
    ("hamburger", "\u{2630}"),      // ☰
    ("table", "\u{25A6}"),          // ▦
    ("eye_off", "\u{25CC}"),        // ◌
    ("hidden", "\u{25CC}"),         // ◌
    ("lock", "\u{1F512}"),          // 🔒 (KNOWN_ANDROID_GLYPH_GAPS)
    ("unlock", "\u{1F513}"),        // 🔓 (KNOWN_ANDROID_GLYPH_GAPS)
    ("home", "\u{2302}"),           // ⌂
    ("drag", "\u{283F}"),           // ⠿
    ("grip", "\u{283F}"),           // ⠿
    ("edit", "\u{270E}"),           // ✎
    ("pencil", "\u{270E}"),         // ✎
    ("trash", "\u{1F5D1}"),         // 🗑 → ⌦ on Android
    ("delete", "\u{1F5D1}"),        // 🗑 → ⌦ on Android
];

/// Glyph for an unknown icon name (bullet). // ALLOW(fallback): names the
/// default-branch glyph, not error swallowing — an unknown name is a valid
/// input.
pub(crate) const ICON_CHAR_DEFAULT: &str = "\u{2022}"; // •

/// Icon-name → glyph, routed through `crate::icon` so glyphs the embedded
/// coverage font can't render are substituted on Android. Unknown names get the
/// bullet default.
fn icon_char(name: &str) -> &'static str {
    let glyph = ICON_CHARS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, g)| *g)
        .unwrap_or(ICON_CHAR_DEFAULT);
    crate::icon(glyph)
}

/// Map an optional semantic color name to a theme token. Empty/unknown names
/// fall back to `muted_foreground` (the default icon tint).
fn icon_color(ctx: &GpuiRenderContext, name: &str) -> Hsla {
    match name {
        "primary" => tc(ctx, |t| t.primary),
        "accent" => tc(ctx, |t| t.accent_foreground),
        "muted" => tc(ctx, |t| t.muted_foreground),
        "warning" => tc(ctx, |t| t.warning),
        "info" => tc(ctx, |t| t.accent),
        "success" => tc(ctx, |t| t.success),
        "foreground" => tc(ctx, |t| t.foreground),
        _ => tc(ctx, |t| t.muted_foreground),
    }
}

pub fn render(node: &holon_frontend::ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let name = node
        .prop_str("name")
        .unwrap_or_else(|| "circle".to_string());
    let size = node.prop_f64("size").unwrap_or(16.0) as f32;
    let color = node
        .prop_str("color")
        .map(|c| icon_color(ctx, &c))
        .unwrap_or_else(|| tc(ctx, |t| t.muted_foreground));
    let mt = node.prop_f64("mt").unwrap_or(0.0) as f32;
    render_icon_styled(&name, size, color, mt, ctx)
}

/// Internal helper for rendering an icon by name and size, used by other
/// builders. Uses the default `muted_foreground` tint and no top offset.
pub(crate) fn render_icon(name: &str, size: f32, ctx: &GpuiRenderContext) -> Div {
    let color = tc(ctx, |t| t.muted_foreground);
    render_icon_styled(name, size, color, 0.0, ctx)
}

/// Render an icon glyph with an explicit tint and top offset. `mt` shifts the
/// glyph box down so it can align to a text line's vertical center when the
/// containing row is top-aligned (block-outline bullets, task toggles).
fn render_icon_styled(name: &str, size: f32, color: Hsla, mt: f32, ctx: &GpuiRenderContext) -> Div {
    let s = ctx.style();
    let default_icon_size = s.icon_size;
    let box_padding = s.icon_box_padding;
    drop(s);

    let icon_size = if size > 0.0 { size } else { default_icon_size };

    // Try SVG icon first, fall back to Unicode character
    if let Some(svg_name) = icon_svg_name(name) {
        let path = icons_dir().join(format!("{svg_name}.svg"));
        if path.exists() {
            let source = ImageSource::Resource(Resource::Path(path.into()));
            return div()
                .flex_shrink_0()
                .mt(px(mt))
                .w(px(icon_size + box_padding))
                .h(px(icon_size))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img(source)
                        .w(px(icon_size))
                        .h(px(icon_size))
                        .grayscale(false),
                );
        }
    }

    // Unicode fallback // ALLOW(fallback): describes default-branch path, not error
    // swallowing
    div()
        .flex_shrink_0()
        .mt(px(mt))
        .w(px(icon_size + box_padding))
        .h(px(icon_size))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(icon_size))
        .line_height(px(icon_size))
        .text_color(color)
        .child(icon_char(name).to_string())
}

#[cfg(test)]
mod icon_char_coverage {
    use super::ICON_CHAR_DEFAULT;
    use super::ICON_CHARS;

    /// Every named-icon glyph (and the unknown-name default) must render on
    /// Android: DejaVu-covered, substituted via `crate::ICON_SUBSTITUTES`, or a
    /// documented `crate::KNOWN_ANDROID_GLYPH_GAPS` entry. Sweeps the table so
    /// a newly-added icon glyph cannot silently tofu.
    #[test]
    fn every_named_icon_glyph_renders_on_android() {
        for (name, glyph) in ICON_CHARS {
            crate::assert_icon_renderable_on_android(glyph, &format!("icon::{name}"));
        }
        crate::assert_icon_renderable_on_android(ICON_CHAR_DEFAULT, "icon::<default>");
    }
}

#[cfg(test)]
mod shared_name_table_agreement {
    use holon_api::icon_name::ICON_NAMES;

    use super::ICON_CHARS;
    use super::icon_svg_name;

    /// `ICON_NAMES` is what a config file — an integration sidecar — is allowed
    /// to ask for, and asking for a name this renderer cannot draw would put a
    /// bullet where a glyph belongs with nobody watching. Sweeping it here is
    /// what keeps the two tables one table.
    #[test]
    fn every_shared_name_resolves_to_an_svg_or_a_glyph() {
        for name in ICON_NAMES {
            assert!(
                icon_svg_name(name).is_some() || ICON_CHARS.iter().any(|(n, _)| n == name),
                "holon_api::icon_name::ICON_NAMES lists {name:?}, which this renderer draws as \
                 the unknown-name bullet"
            );
        }
    }

    /// The other direction, for the half of the renderer that can be
    /// enumerated. `icon_svg_name` is a `match` and has no iterator, so an SVG
    /// name added there without being listed in `ICON_NAMES` stays out of a
    /// sidecar's reach until somebody notices.
    #[test]
    fn every_glyph_name_is_shared() {
        for (name, _) in ICON_CHARS {
            assert!(
                ICON_NAMES.contains(name),
                "icon.rs draws {name:?} but holon_api::icon_name::ICON_NAMES omits it, so a \
                 sidecar cannot ask for it"
            );
        }
    }
}
