//! Translucent overlays for PBT screenshots: action banner, pass/fail badge,
//! optional assertion text. Designed to be composited onto an `RgbaImage`
//! captured via `xcap`, regardless of the underlying frontend.
//!
//! Pacing: one `Pre` capture before each transition + one `Post` capture
//! after invariants ≈ 2 fps in practice; the recorder doesn't impose its
//! own throttle.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::{Rgba, RgbaImage};

/// Step phase — encoded into the screenshot filename so a per-step pair
/// (pre/post) sorts lexicographically in capture order.
#[derive(Debug, Clone, Copy)]
pub enum Phase {
    Pre,
    Post,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Pre => "pre",
            Phase::Post => "post",
        }
    }
}

/// Outcome of a transition's invariant checks.
#[derive(Debug, Clone)]
pub enum Verdict {
    Pass,
    Fail { assertion: String },
}

/// What to draw on top of the captured frame. All overlays render with a
/// single global `alpha` so the composite stays readable but doesn't hide
/// the UI underneath.
#[derive(Debug, Clone)]
pub struct Overlay {
    /// Action label drawn in the top banner, e.g. `"NavigateFocus(Down)"`.
    pub banner: Option<String>,
    /// Pass/Fail badge drawn in the top-right corner. Absent on `Pre`.
    pub verdict: Option<Verdict>,
    /// Per-overlay alpha in `[0.0, 1.0]`. Easy to tweak from one place.
    pub alpha: f32,
}

/// Default banner/badge alpha. ~0.8 keeps a hint of the UI behind it but
/// makes the action label and assertion text comfortably readable.
pub const DEFAULT_OVERLAY_ALPHA: f32 = 0.8;

impl Overlay {
    pub fn action(label: impl Into<String>) -> Self {
        Self {
            banner: Some(label.into()),
            verdict: None,
            alpha: DEFAULT_OVERLAY_ALPHA,
        }
    }

    pub fn pass(label: impl Into<String>) -> Self {
        Self {
            banner: Some(label.into()),
            verdict: Some(Verdict::Pass),
            alpha: DEFAULT_OVERLAY_ALPHA,
        }
    }

    pub fn fail(label: impl Into<String>, assertion: impl Into<String>) -> Self {
        Self {
            banner: Some(label.into()),
            verdict: Some(Verdict::Fail {
                assertion: assertion.into(),
            }),
            alpha: DEFAULT_OVERLAY_ALPHA,
        }
    }
}

const FONT_BYTES: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

fn font() -> FontRef<'static> {
    FontRef::try_from_slice(FONT_BYTES).expect("bundled Inter-Regular.ttf failed to parse")
}

/// Composite the overlay onto a captured frame. Coordinates are in physical
/// pixels (xcap returns retina-scaled images on macOS); banner/badge sizes
/// adapt to the image dimensions so retina vs non-retina both look right.
///
/// Layout is bottom-anchored: badge + banner sit at the very bottom of the
/// frame; the assertion block (Fail only) stacks just above them.
pub fn paint_overlay(img: &mut RgbaImage, overlay: &Overlay) {
    let (img_w, img_h) = (img.width(), img.height());
    let scale = if img_w >= 2000 { 2.0_f32 } else { 1.0 };
    let font = font();

    let banner_h = (40.0 * scale) as u32;
    let banner_bottom = img_h;
    let banner_top = banner_bottom.saturating_sub(banner_h);
    let badge_size = (40.0 * scale) as u32;
    let margin = (8.0 * scale) as u32;

    let bg = Rgba([0, 0, 0, 255]);
    let white = Rgba([255, 255, 255, 255]);

    if let Some(banner) = &overlay.banner {
        let pad = (12.0 * scale) as u32;
        fill_rect_blended(img, 0, banner_top, img_w, banner_h, bg, overlay.alpha);

        // Auto-shrink so the full action label stays within the banner width
        // (minus the badge column when a verdict is present).
        let badge_reserve = if overlay.verdict.is_some() {
            badge_size + 2 * margin
        } else {
            0
        };
        let max_text_w = img_w.saturating_sub(2 * pad + badge_reserve) as f32;
        let mut text_px = 22.0 * scale;
        let min_px = 12.0 * scale;
        while text_width(&font, banner, text_px) > max_text_w && text_px > min_px {
            text_px -= 1.0;
        }
        let baseline = banner_top as i32 + ((banner_h as f32 - text_px) / 2.0).max(0.0) as i32;
        draw_text(
            img,
            &font,
            banner,
            pad as i32,
            baseline,
            text_px,
            white,
            overlay.alpha,
        );
    }

    if let Some(verdict) = &overlay.verdict {
        let badge_x = img_w.saturating_sub(badge_size + margin);
        let badge_y = banner_top;

        match verdict {
            Verdict::Pass => {
                let green = Rgba([40, 180, 90, 255]);
                fill_rect_blended(
                    img,
                    badge_x,
                    badge_y,
                    badge_size,
                    badge_size,
                    green,
                    overlay.alpha,
                );
                draw_check(img, badge_x, badge_y, badge_size, white, overlay.alpha);
            }
            Verdict::Fail { assertion } => {
                let red = Rgba([220, 50, 50, 255]);
                fill_rect_blended(
                    img,
                    badge_x,
                    badge_y,
                    badge_size,
                    badge_size,
                    red,
                    overlay.alpha,
                );
                draw_cross(img, badge_x, badge_y, badge_size, white, overlay.alpha);

                // Assertion block sits ABOVE the banner so the bottom row stays
                // visually pinned to the banner+badge.
                let text_px = 20.0 * scale;
                let block_x = (16.0 * scale) as i32;
                let block_pad_y = (8.0 * scale) as u32;
                let block_w = img_w.saturating_sub(2 * (16.0 * scale) as u32);
                let line_h = (text_px * 1.25) as u32;

                let lines = wrap_text(&font, assertion, text_px, block_w);
                let block_h = line_h * (lines.len() as u32) + block_pad_y * 2;
                let block_y = banner_top.saturating_sub(block_h);

                fill_rect_blended(img, 0, block_y, img_w, block_h, bg, overlay.alpha);

                for (i, line) in lines.iter().enumerate() {
                    draw_text(
                        img,
                        &font,
                        line,
                        block_x,
                        block_y as i32 + block_pad_y as i32 + (i as i32 * line_h as i32),
                        text_px,
                        white,
                        overlay.alpha,
                    );
                }
            }
        }
    }
}

fn fill_rect_blended(
    img: &mut RgbaImage,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: Rgba<u8>,
    alpha: f32,
) {
    let (img_w, img_h) = (img.width(), img.height());
    let x_end = (x + w).min(img_w);
    let y_end = (y + h).min(img_h);
    let a = alpha.clamp(0.0, 1.0);
    for py in y..y_end {
        for px in x..x_end {
            blend_pixel(img, px, py, color, a);
        }
    }
}

fn blend_pixel(img: &mut RgbaImage, x: u32, y: u32, color: Rgba<u8>, alpha: f32) {
    let dst = img.get_pixel_mut(x, y);
    for c in 0..3 {
        let s = dst.0[c] as f32;
        let o = color.0[c] as f32;
        dst.0[c] = (s * (1.0 - alpha) + o * alpha) as u8;
    }
    dst.0[3] = 255;
}

fn draw_text(
    img: &mut RgbaImage,
    font: &FontRef<'_>,
    text: &str,
    x: i32,
    y: i32,
    px: f32,
    color: Rgba<u8>,
    alpha: f32,
) {
    let scale = PxScale::from(px);
    let scaled = font.as_scaled(scale);
    let mut caret_x = x as f32;
    let baseline_y = y as f32 + scaled.ascent();
    let mut prev_glyph: Option<ab_glyph::GlyphId> = None;

    for ch in text.chars() {
        let glyph_id = font.glyph_id(ch);
        if let Some(prev) = prev_glyph {
            caret_x += scaled.kern(prev, glyph_id);
        }
        let glyph = glyph_id.with_scale_and_position(scale, ab_glyph::point(caret_x, baseline_y));
        let advance = scaled.h_advance(glyph_id);
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bb = outlined.px_bounds();
            outlined.draw(|gx, gy, coverage| {
                let px = bb.min.x as i32 + gx as i32;
                let py = bb.min.y as i32 + gy as i32;
                if px < 0 || py < 0 {
                    return;
                }
                let (px, py) = (px as u32, py as u32);
                if px >= img.width() || py >= img.height() {
                    return;
                }
                blend_pixel(img, px, py, color, alpha * coverage);
            });
        }
        caret_x += advance;
        prev_glyph = Some(glyph_id);
    }
}

fn text_width(font: &FontRef<'_>, text: &str, px: f32) -> f32 {
    let scaled = font.as_scaled(PxScale::from(px));
    let mut w = 0.0;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for ch in text.chars() {
        let g = font.glyph_id(ch);
        if let Some(p) = prev {
            w += scaled.kern(p, g);
        }
        w += scaled.h_advance(g);
        prev = Some(g);
    }
    w
}

fn wrap_text(font: &FontRef<'_>, text: &str, px: f32, max_w: u32) -> Vec<String> {
    let max = max_w as f32;
    let mut out: Vec<String> = Vec::new();
    for paragraph in text.lines() {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if text_width(font, &candidate, px) <= max {
                current = candidate;
            } else {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                if text_width(font, word, px) > max {
                    out.extend(break_long_word(font, word, px, max));
                } else {
                    current = word.to_string();
                }
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
        if paragraph.is_empty() {
            out.push(String::new());
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn break_long_word(font: &FontRef<'_>, word: &str, px: f32, max: f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in word.chars() {
        let candidate: String = current.chars().chain(std::iter::once(ch)).collect();
        if text_width(font, &candidate, px) > max && !current.is_empty() {
            out.push(std::mem::take(&mut current));
            current.push(ch);
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn draw_check(img: &mut RgbaImage, x: u32, y: u32, size: u32, color: Rgba<u8>, alpha: f32) {
    let s = size as f32;
    let p1 = (x as f32 + 0.20 * s, y as f32 + 0.55 * s);
    let p2 = (x as f32 + 0.42 * s, y as f32 + 0.75 * s);
    let p3 = (x as f32 + 0.80 * s, y as f32 + 0.30 * s);
    let thickness = (size / 8).max(2);
    draw_thick_line(img, p1, p2, thickness, color, alpha);
    draw_thick_line(img, p2, p3, thickness, color, alpha);
}

fn draw_cross(img: &mut RgbaImage, x: u32, y: u32, size: u32, color: Rgba<u8>, alpha: f32) {
    let s = size as f32;
    let inset = 0.25 * s;
    let p1 = (x as f32 + inset, y as f32 + inset);
    let p2 = (x as f32 + s - inset, y as f32 + s - inset);
    let p3 = (x as f32 + s - inset, y as f32 + inset);
    let p4 = (x as f32 + inset, y as f32 + s - inset);
    let thickness = (size / 8).max(2);
    draw_thick_line(img, p1, p2, thickness, color, alpha);
    draw_thick_line(img, p3, p4, thickness, color, alpha);
}

fn draw_thick_line(
    img: &mut RgbaImage,
    a: (f32, f32),
    b: (f32, f32),
    thickness: u32,
    color: Rgba<u8>,
    alpha: f32,
) {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len = (dx * dx + dy * dy).sqrt().max(1.0);
    let steps = len.ceil() as i32;
    let r = thickness as i32 / 2;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let cx = (a.0 + dx * t) as i32;
        let cy = (a.1 + dy * t) as i32;
        for oy in -r..=r {
            for ox in -r..=r {
                if ox * ox + oy * oy <= r * r {
                    let px = cx + ox;
                    let py = cy + oy;
                    if px < 0 || py < 0 {
                        continue;
                    }
                    let (px, py) = (px as u32, py as u32);
                    if px >= img.width() || py >= img.height() {
                        continue;
                    }
                    blend_pixel(img, px, py, color, alpha);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};
    use std::path::PathBuf;

    /// Render the three overlay variants onto a synthetic dark frame and save
    /// PNGs to `target/overlay-smoke/`. Run with
    /// `cargo test -p holon-integration-tests --features pbt -- --nocapture
    ///  screenshot_overlay::tests::smoke` and inspect the output by eye.
    #[test]
    fn smoke() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("overlay-smoke");
        std::fs::create_dir_all(&dir).unwrap();

        let make_canvas = || -> image::RgbaImage {
            let (w, h) = (3024u32, 1890u32);
            ImageBuffer::from_fn(w, h, |x, y| {
                let g = ((x + y) / 12) as u8 % 64 + 30;
                Rgba([g, g, g.saturating_add(8), 255])
            })
        };

        let mut a = make_canvas();
        paint_overlay(
            &mut a,
            &Overlay::action("ClickBlock  •  op_dispatch.click_block(block:abc123)"),
        );
        a.save(dir.join("01-pre-action.png")).unwrap();

        let mut b = make_canvas();
        paint_overlay(
            &mut b,
            &Overlay::pass("NavigateFocus(Down)  •  navigation.navigate_focus(Down)"),
        );
        b.save(dir.join("02-post-pass.png")).unwrap();

        let mut c = make_canvas();
        paint_overlay(
            &mut c,
            &Overlay::fail(
                "ToggleState  •  block.cycle_task_state(block:7a44bf6f)",
                "inv12: block:7a44bf6f task_state expected `DONE` after toggle, but reference \
                 model has `TODO`. Reference state computed by ReferenceState::apply, observed \
                 state read from sut.snapshot().",
            ),
        );
        c.save(dir.join("03-post-fail.png")).unwrap();

        eprintln!("[overlay smoke] wrote 3 images to {}", dir.display());
    }
}
