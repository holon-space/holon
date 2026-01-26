//! In-process [`ScreenshotBackend`] that paints r3bl_tui
//! [`OffscreenBuffer`] cells into RGBA8 pixels.
//!
//! Pairs with [`crate::test_harness::CapturingApp`]: the harness composes
//! each render pass into a shared `Arc<RwLock<Option<OffscreenBuffer>>>`
//! via the public `compose_render_ops_into_ofs_buf` API, and this
//! backend reads that buffer on `capture()`.
//!
//! Painter policy: each cell becomes a `(cell_w × cell_h)` block of
//! pixels. For [`PixelChar::PlainText`], the block uses the cell's
//! resolved foreground color (or white when no `color_fg` is set), so
//! every glyph contributes "bright" pixels that
//! [`holon_integration_tests::ui_driver::analyze_screenshot_emptiness`]
//! counts as content. Spacer / Void cells get a near-black pixel so the
//! background stays below the brightness threshold (45 per channel).
//! Reusing the existing analyzer means inv14 fires identically across
//! GPUI and TUI without changing trait surfaces.
//!
//! Cell-to-pixel ratio is fixed by `(cell_w, cell_h)` constants
//! supplied at construction. Tests typically pass [`crate::geometry::CELL_W`] /
//! [`crate::geometry::CELL_H`] so the painter dimensions stay aligned
//! with the geometry registry.

use std::sync::{Arc, RwLock};

use holon_integration_tests::ui_driver::{CapturedScreenshot, ScreenshotBackend};
use r3bl_tui::{OffscreenBuffer, PixelChar, TuiColor};

/// In-process screenshot backend over a shared [`OffscreenBuffer`].
pub struct OffscreenBufferBackend {
    buffer: Arc<RwLock<Option<OffscreenBuffer>>>,
    cell_w: f32,
    cell_h: f32,
    title: String,
}

impl OffscreenBufferBackend {
    pub fn new(buffer: Arc<RwLock<Option<OffscreenBuffer>>>, cell_w: f32, cell_h: f32) -> Self {
        Self {
            buffer,
            cell_w,
            cell_h,
            title: "Holon TUI PBT".into(),
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }
}

impl ScreenshotBackend for OffscreenBufferBackend {
    fn capture(&self) -> Option<CapturedScreenshot> {
        let guard = self.buffer.read().unwrap();
        let buf = guard.as_ref()?;
        Some(paint_offscreen_buffer(buf, self.cell_w, self.cell_h))
    }

    fn window_title(&self) -> String {
        self.title.clone()
    }
}

/// Render `buffer` as RGBA8 with each cell expanded to a
/// `(cell_w × cell_h)` block. Public for unit tests.
pub fn paint_offscreen_buffer(
    buffer: &OffscreenBuffer,
    cell_w: f32,
    cell_h: f32,
) -> CapturedScreenshot {
    let cw = cell_w.max(1.0) as u32;
    let ch = cell_h.max(1.0) as u32;
    let cols = u32::from(*buffer.window_size.col_width);
    let rows = u32::from(*buffer.window_size.row_height);
    let width = cols * cw;
    let height = rows * ch;
    let mut data = vec![0u8; (width * height * 4) as usize];

    let bg = (16u8, 16u8, 16u8);

    for row in 0..rows {
        for col in 0..cols {
            let pixel = buffer.buffer[row as usize][col as usize];
            let (r, g, b) = pixel_color(pixel, bg);
            for cy in 0..ch {
                for cx in 0..cw {
                    let px = col * cw + cx;
                    let py = row * ch + cy;
                    let idx = ((py * width + px) * 4) as usize;
                    data[idx] = r;
                    data[idx + 1] = g;
                    data[idx + 2] = b;
                    data[idx + 3] = 255;
                }
            }
        }
    }

    CapturedScreenshot {
        data,
        width,
        height,
    }
}

fn pixel_color(cell: PixelChar, bg: (u8, u8, u8)) -> (u8, u8, u8) {
    match cell {
        PixelChar::PlainText {
            display_char,
            style,
        } if !is_blank(display_char) => style
            .color_fg
            .map(tui_color_to_rgb)
            .unwrap_or((255, 255, 255)),
        // Blank glyphs (' ', SPACER_GLYPH, etc.) stay on the background so the
        // whitespace surrounding text doesn't trip the brightness threshold.
        _ => bg,
    }
}

fn is_blank(c: char) -> bool {
    c.is_whitespace() || c == r3bl_tui::SPACER_GLYPH_CHAR
}

fn tui_color_to_rgb(color: TuiColor) -> (u8, u8, u8) {
    let rgb: r3bl_tui::RgbValue = color.into();
    (rgb.red, rgb.green, rgb.blue)
}
