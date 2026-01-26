use std::path::PathBuf;
use std::sync::OnceLock;

use gpui::{img, ImageSource, Resource, StyledImage};

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
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("assets/icons")))
            .unwrap_or_else(|| PathBuf::from("assets/icons"))
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
        _ => return None,
    })
}

/// Unicode fallback for icons without an SVG file.
fn icon_char(name: &str) -> &'static str {
    match name {
        "orgmode" => "◉",
        "circle" => "●",
        "chevron_right" => "›",
        "chevron_down" => "⌄",
        "chevron_left" => "‹",
        "chevron_up" => "⌃",
        "plus" | "add" => "+",
        "minus" | "remove" => "−",
        "info" => "ℹ",
        "code" | "source" => "⟨⟩",
        "list" | "menu" | "hamburger" => "☰",
        "table" => "▦",
        "eye_off" | "hidden" => "◌",
        "lock" => "🔒",
        "unlock" => "🔓",
        "home" => "⌂",
        "drag" | "grip" => "⠿",
        "edit" | "pencil" => "✎",
        "trash" | "delete" => "🗑",
        _ => "•",
    }
}

pub fn render(name: &String, size: &f32, ctx: &GpuiRenderContext) -> Div {
    let icon_size = if *size > 0.0 { *size } else { 16.0 };

    // Try SVG icon first, fall back to Unicode character
    if let Some(svg_name) = icon_svg_name(name.as_str()) {
        let path = icons_dir().join(format!("{svg_name}.svg"));
        if path.exists() {
            let source = ImageSource::Resource(Resource::Path(path.into()));
            return div()
                .flex_shrink_0()
                .w(px(icon_size + 4.0))
                .h(px(icon_size))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    img(source)
                        .w(px(icon_size))
                        .h(px(icon_size))
                        .grayscale(ctx.bounds_registry.icon_greyscale()),
                );
        }
    }

    // Unicode fallback
    let color = tc(ctx, |t| t.muted_foreground);
    div()
        .flex_shrink_0()
        .w(px(icon_size + 4.0))
        .h(px(icon_size))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(icon_size))
        .line_height(px(icon_size))
        .text_color(color)
        .child(icon_char(name.as_str()).to_string())
}
