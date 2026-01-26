//! Unit tests for `OffscreenBufferBackend`'s painter, isolated from the
//! `tui_ui_pbt` `harness=false` integration so they actually run under
//! `cargo test`.

use holon_integration_tests::ui_driver::analyze_screenshot_emptiness;
use r3bl_tui::{height, new_style, tui_color, width, OffscreenBuffer, PixelChar, TuiStyle};

mod common;
use common::screenshot::paint_offscreen_buffer;

fn pixel_text(c: char, style: TuiStyle) -> PixelChar {
    PixelChar::PlainText {
        display_char: c,
        style,
    }
}

#[test]
fn paint_emits_fg_color_for_plain_text() {
    let mut buf = OffscreenBuffer::new_empty(width(4) + height(2));
    buf.buffer[0][0] = pixel_text('X', new_style!(color_fg: {tui_color!(hex "#FFAA00")}));

    let captured = paint_offscreen_buffer(&buf, 8.0, 16.0);
    assert_eq!(captured.width, 4 * 8);
    assert_eq!(captured.height, 2 * 16);

    let idx = 0;
    assert_eq!(captured.data[idx], 255);
    assert_eq!(captured.data[idx + 1], 170);
    assert_eq!(captured.data[idx + 2], 0);
    assert_eq!(captured.data[idx + 3], 255);

    let idx_bg = (3 * 8) * 4;
    assert!(captured.data[idx_bg] < 45);
    assert!(captured.data[idx_bg + 1] < 45);
    assert!(captured.data[idx_bg + 2] < 45);
}

/// Catches architecture decision 5 (encode fg, not bg) regression.
#[test]
fn analyze_emptiness_sees_painted_text_as_content() {
    // 8 rows × cell_h=16 = 128 px; the analyzer skips the top 80 px so
    // rows 5..7 (y=80..127) contribute to the brightness fraction.
    let mut buf = OffscreenBuffer::new_empty(width(80) + height(8));
    let style = new_style!(color_fg: {tui_color!(hex "#FFFFFF")});
    for r in 5..8 {
        for c in 0..40 {
            buf.buffer[r][c] = pixel_text('A', style);
        }
    }

    let captured = paint_offscreen_buffer(&buf, 8.0, 16.0);
    let emptiness = analyze_screenshot_emptiness(&captured);
    assert!(
        emptiness.content_fraction > 0.0,
        "expected non-empty content fraction, got {emptiness:?}",
    );
}

#[test]
fn analyze_emptiness_blank_buffer_is_empty() {
    let buf = OffscreenBuffer::new_empty(width(80) + height(8));
    let captured = paint_offscreen_buffer(&buf, 8.0, 16.0);
    let emptiness = analyze_screenshot_emptiness(&captured);
    assert_eq!(
        emptiness.content_fraction, 0.0,
        "blank Spacer buffer must analyze as empty",
    );
}
