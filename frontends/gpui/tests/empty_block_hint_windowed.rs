//! Ruling D5B-8.a (Martin, 2026-08-18): an empty block paints NO placeholder
//! hint. The read path used to draw a grey "Type here" in place of the missing
//! content — text the user cannot select, cannot edit, and has to learn to
//! ignore on every empty row of an outline.
//!
//! Two facts pin the ruling, and they pull against each other — which is why
//! both live here:
//!
//! 1. The row paints nothing. `ElementInfo::displayed_text` reports what the
//!    widget puts on screen, so a stand-in hint shows up here as text a block
//!    with empty content is not supposed to have.
//! 2. The row is still THERE — as tall as a one-line row of real content, so
//!    the click target keeps its size and read → edit does not jump.
//!
//! Run: `cargo nextest run -p holon-gpui --test empty_block_hint_windowed`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::TestAppContext;
use holon_api::Value;
use holon_frontend::ReactiveViewModel;
use holon_frontend::geometry::ElementInfo;
use support::BoundsSnapshot;
use support::render_fixture;

const EMPTY_ROW: &str = "block:empty-row";
const FILLED_ROW: &str = "block:filled-row";
const FILLED_TEXT: &str = "A block that has content";

/// Sub-pixel rounding is not a defect; a missing text line (20px at this
/// metric) is.
const EPS: f32 = 1.5;

/// A read-mode block row as the outline builds one: `rendered_text` bound to
/// the row's own `content` column.
fn rendered_text_row(id: &str, content: &str) -> ReactiveViewModel {
    let mut props = HashMap::new();
    props.insert("content".to_string(), Value::String(content.to_string()));
    props.insert("field".to_string(), Value::String("content".to_string()));
    let mut vm = ReactiveViewModel::from_widget("rendered_text", props);

    let mut data = HashMap::new();
    data.insert("id".to_string(), Value::String(id.to_string()));
    data.insert("content".to_string(), Value::String(content.to_string()));
    vm.data = Mutable::new(Arc::new(data)).read_only();
    vm
}

fn outline() -> Arc<ReactiveViewModel> {
    let mut col = ReactiveViewModel::from_widget("column", HashMap::new());
    col.children = vec![
        Arc::new(rendered_text_row(EMPTY_ROW, "")),
        Arc::new(rendered_text_row(FILLED_ROW, FILLED_TEXT)),
    ];
    Arc::new(col)
}

fn text_element<'a>(snap: &'a BoundsSnapshot, row_id: &str) -> &'a ElementInfo {
    let el_id = format!("rendered-text-{row_id}-content");
    snap.entries
        .iter()
        .find(|(id, _)| *id == el_id)
        .map(|(_, info)| info)
        .unwrap_or_else(|| {
            let ids: Vec<&str> = snap.entries.iter().map(|(id, _)| id.as_str()).collect();
            panic!("no tracked element {el_id}; tracked: {ids:?}")
        })
}

#[gpui::test]
fn an_empty_read_mode_block_paints_no_text(cx: &mut TestAppContext) {
    let snap = render_fixture(cx, outline());

    let empty = text_element(&snap, EMPTY_ROW);
    assert_eq!(
        empty.displayed_text.as_deref(),
        Some(""),
        "an empty block must paint nothing — no hint stands in for content the user has not \
         written yet"
    );

    // ... and a row WITH content still paints it, so the assertion above is not
    // vacuously true of a fixture that renders no text at all.
    let filled = text_element(&snap, FILLED_ROW);
    assert_eq!(
        filled.displayed_text.as_deref(),
        Some(FILLED_TEXT),
        "a block with content must still paint it"
    );
}

#[gpui::test]
fn an_empty_read_mode_block_keeps_a_full_row_box(cx: &mut TestAppContext) {
    let snap = render_fixture(cx, outline());

    let empty = text_element(&snap, EMPTY_ROW);
    let filled = text_element(&snap, FILLED_ROW);

    assert!(
        (empty.height - filled.height).abs() < EPS,
        "an empty block's row must stay as tall as a one-line row of content ({:.1}px), else the \
         click target shrinks and focusing it jumps the outline; got {:.1}px",
        filled.height,
        empty.height,
    );
    assert!(
        empty.width > 0.0 && empty.height > 0.0,
        "the empty row must still be a clickable rect; got {:.1}x{:.1}",
        empty.width,
        empty.height,
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
