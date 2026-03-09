use std::path::PathBuf;

use gpui::{img, ClipboardItem, Image, ImageFormat, ImageSource, Resource};
use gpui_component::menu::{ContextMenuExt, PopupMenuItem};

use holon_frontend::ReactiveViewModel;

use super::prelude::*;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let path = node.prop_str("path").unwrap_or_default();
    let alt = node.prop_str("alt").unwrap_or_default();
    let width = node.prop_f64("width").map(|v| v as f32);
    let height = node.prop_f64("height").map(|v| v as f32);
    let resolved = resolve_image_path(&path);

    if !image_exists_cached(&resolved) {
        let label = if alt.is_empty() {
            format!("[missing image: {path}]")
        } else {
            format!("[missing image: {alt}]")
        };
        return div()
            .px(px(8.0))
            .py(px(4.0))
            .text_color(tc(ctx, |t| t.muted_foreground))
            .child(label)
            .into_any_element();
    }

    let source = ImageSource::Resource(Resource::Path(resolved.clone().into()));
    let mut image_el = img(source).rounded(px(4.0));

    match (width, height) {
        (Some(w), Some(h)) => {
            image_el = image_el.w(px(w)).h(px(h));
        }
        (Some(w), None) => {
            image_el = image_el.w(px(w));
        }
        (None, Some(h)) => {
            image_el = image_el.h(px(h));
        }
        (None, None) => {
            image_el = image_el.max_w(px(600.0));
        }
    }

    let el_id = hashed_id(&format!("img:{path}"));
    let resolved_for_menu = resolved.clone();

    div()
        .id(el_id)
        .py(px(4.0))
        .child(image_el)
        .context_menu(move |menu, _window, _cx| {
            let path_for_click = resolved_for_menu.clone();
            menu.item(
                PopupMenuItem::new("Copy image").on_click(move |_, _window, cx| {
                    copy_image_to_clipboard(&path_for_click, cx);
                }),
            )
        })
        .into_any_element()
}

fn copy_image_to_clipboard(path: &PathBuf, cx: &mut gpui::App) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("Failed to read image for clipboard: {e}");
            return;
        }
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png");
    let format = match ext {
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "webp" => ImageFormat::Webp,
        "svg" => ImageFormat::Svg,
        "bmp" => ImageFormat::Bmp,
        "tiff" | "tif" => ImageFormat::Tiff,
        "ico" => ImageFormat::Ico,
        _ => ImageFormat::Png,
    };
    let image = Image::from_bytes(format, bytes);
    cx.write_to_clipboard(ClipboardItem::new_image(&image));
}

fn org_root() -> Option<PathBuf> {
    std::env::var("HOLON_VAULT_ROOT")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOLON_WORKSPACE_ROOT").ok().map(PathBuf::from))
        .or_else(|| {
            std::env::var("CARGO_MANIFEST_DIR")
                .ok()
                .map(PathBuf::from)
        })
}

/// Per-frame existence check with a positive cache: `render` runs every
/// frame, and an uncached `Path::exists()` is a filesystem stat per image
/// per frame. Only positive results are cached — a missing image keeps
/// re-statting and appears as soon as the file lands. A file deleted
/// after being cached skips the "[missing image]" label and surfaces as
/// gpui's own failed-to-load rendering instead (disclosed, not faked).
fn image_exists_cached(resolved: &PathBuf) -> bool {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static KNOWN_GOOD: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    let known = KNOWN_GOOD.get_or_init(|| Mutex::new(HashSet::new()));
    if known.lock().unwrap().contains(resolved) {
        return true;
    }
    let exists = resolved.exists();
    if exists {
        known.lock().unwrap().insert(resolved.clone());
    }
    exists
}

fn resolve_image_path(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        return p;
    }
    if let Some(root) = org_root() {
        let candidate = root.join(path);
        if let Ok(canonical) = candidate.canonicalize() {
            if let Ok(root_canonical) = root.canonicalize() {
                if canonical.starts_with(&root_canonical) {
                    return canonical;
                }
                tracing::error!(
                    "Path jail: {path:?} resolves to {} which escapes org root {}",
                    canonical.display(),
                    root_canonical.display()
                );
                return PathBuf::from("/dev/null");
            }
        }
        return candidate;
    }
    p
}
