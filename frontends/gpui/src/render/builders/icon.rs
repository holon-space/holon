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
            .ok() // ALLOW(ok): non-critical, has fallback
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
        "tree" | "list_tree" | "outliner" | "books" => "books",
        "table" | "table_2" | "grid" | "abacus" => "abacus",
        _ => return None,
    })
}

/// Unicode fallback for icons without an SVG file.
fn icon_char(name: &str) -> &'static str {
    match name {
        "orgmode" => "\u{25C9}",
        "circle" => "\u{25CF}",
        "chevron_right" => "\u{203A}",
        "chevron_down" => "\u{2304}",
        "chevron_left" => "\u{2039}",
        "chevron_up" => "\u{2303}",
        "plus" | "add" => "+",
        "minus" | "remove" => "\u{2212}",
        "info" => "\u{2139}",
        "code" | "source" => "\u{27E8}\u{27E9}",
        "list" | "menu" | "hamburger" => "\u{2630}",
        "table" => "\u{25A6}",
        "eye_off" | "hidden" => "\u{25CC}",
        "lock" => "\u{1F512}",
        "unlock" => "\u{1F513}",
        "home" => "\u{2302}",
        "drag" | "grip" => "\u{283F}",
        "edit" | "pencil" => "\u{270E}",
        "trash" | "delete" => "\u{1F5D1}",
        _ => "\u{2022}",
    }
}

pub fn render(name: &String, size: &f32, ctx: &GpuiRenderContext) -> Div {
    let s = ctx.style();
    let default_icon_size = s.icon_size;
    let box_padding = s.icon_box_padding;
    drop(s);

    let icon_size = if *size > 0.0 { *size } else { default_icon_size };

    // Try SVG icon first, fall back to Unicode character
    if let Some(svg_name) = icon_svg_name(name.as_str()) {
        let path = icons_dir().join(format!("{svg_name}.svg"));
        if path.exists() {
            let source = ImageSource::Resource(Resource::Path(path.into()));
            return div()
                .flex_shrink_0()
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

    // Unicode fallback
    let color = tc(ctx, |t| t.muted_foreground);
    div()
        .flex_shrink_0()
        .w(px(icon_size + box_padding))
        .h(px(icon_size))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(icon_size))
        .line_height(px(icon_size))
        .text_color(color)
        .child(icon_char(name.as_str()).to_string())
}
