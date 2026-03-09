//! Phase 0.2 spike: bare-GPUI rich-text input feasibility.
//!
//! Demonstrates that we can build the Phase 3 editor on GPUI primitives:
//!   - `WindowTextSystem::shape_text` with per-run styling (Bold/Italic via TextRun.font + color)
//!   - `WrappedLine::position_for_index` (caret offset → pixel point)
//!   - `WrappedLine::closest_index_for_position` (mouse hit-testing → caret offset)
//!   - `EntityInputHandler` (IME hook trait)
//!   - `window.handle_input(focus_handle, ElementInputHandler::new(...))`
//!   - `paint_quad(fill(...))` for the caret
//!   - Action dispatch (Cmd+B etc.) — covered by existing GPUI infrastructure
//!
//! Run: `cargo run --example rich_input_spike -p holon-gpui --features=desktop`
//!
//! This is *not* a polished editor. It's the smallest end-to-end demonstration
//! that the architecture compiles and the primitives compose. Phase 3 builds
//! the full editor on this foundation.

use std::ops::Range;

use gpui::prelude::*;
use gpui::{
    actions, div, fill, px, rgb, size, white, App, Application, Bounds, Context,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, Hsla, MouseDownEvent,
    ParentElement, Pixels, Point, SharedString, TextAlign, TextRun, UTF16Selection, Window,
    WindowBounds, WindowOptions, WrappedLine,
};

/// The minimal mark vocabulary the spike exercises (a subset of the planned full set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    Bold,
    Italic,
    Code,
}

#[derive(Debug, Clone)]
struct MarkSpan {
    range: Range<usize>, // byte offsets in the spike's plain text
    mark: Mark,
}

/// State for the spike. In Phase 3 this lives behind a `LoroText` + Loro cursor.
struct RichTextSpike {
    text: SharedString,
    marks: Vec<MarkSpan>,
    caret: usize, // byte offset
    focus: FocusHandle,
}

impl RichTextSpike {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _ = window;
        Self {
            text: "Hello, rich world! Click anywhere to move the caret.".into(),
            marks: vec![
                MarkSpan {
                    range: 0..5,
                    mark: Mark::Bold,
                }, // "Hello"
                MarkSpan {
                    range: 7..11,
                    mark: Mark::Italic,
                }, // "rich"
                MarkSpan {
                    range: 12..17,
                    mark: Mark::Code,
                }, // "world"
            ],
            caret: 0,
            focus: cx.focus_handle(),
        }
    }

    /// The mapping that does the real work in Phase 3: marks → Vec<TextRun>.
    /// This is the single hot path the rich-text renderer takes — proves marks
    /// translate to GPUI's native styling without lossy encoding.
    fn build_runs(&self, font: gpui::Font) -> Vec<TextRun> {
        let len = self.text.len();
        let mut runs: Vec<TextRun> = Vec::new();
        let mut i = 0;
        while i < len {
            let active_at_i = self.active_marks_at(i);
            let mut j = i + 1;
            while j < len && self.active_marks_at(j) == active_at_i {
                j += 1;
            }
            let mut run_font = font.clone();
            let mut color: Hsla = rgb(0xeeeeee).into();
            let mut background = None;
            for mark in &active_at_i {
                match mark {
                    Mark::Bold => run_font.weight = gpui::FontWeight::BOLD,
                    Mark::Italic => run_font.style = gpui::FontStyle::Italic,
                    Mark::Code => {
                        background = Some(rgb(0x333333).into());
                        color = rgb(0xa6e3a1).into();
                    }
                }
            }
            runs.push(TextRun {
                len: j - i,
                font: run_font,
                color,
                background_color: background,
                underline: None,
                strikethrough: None,
            });
            i = j;
        }
        runs
    }

    fn active_marks_at(&self, byte_offset: usize) -> Vec<Mark> {
        self.marks
            .iter()
            .filter(|m| m.range.contains(&byte_offset))
            .map(|m| m.mark)
            .collect()
    }
}

impl Focusable for RichTextSpike {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// IME hook impl. Bodies are stubs — Phase 3 wires these through the
/// EditorController. The trait *existing* on our type is the proof that
/// macOS IME plugs in cleanly without re-implementing platform plumbing.
impl EntityInputHandler for RichTextSpike {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        self.text.get(range).map(|s| s.to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.caret..self.caret,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let r = range.unwrap_or(self.caret..self.caret);
        let mut s = self.text.to_string();
        s.replace_range(r.clone(), new_text);
        self.text = s.into();
        self.caret = r.start + new_text.len();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // Stub: Phase 3 wires this to LoroText with an `ime_marked_range`
        // analog. The trait existing on our type is the proof.
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.caret)
    }
}

actions!(rich_input_spike, [ToggleBold]);

impl Render for RichTextSpike {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let entity = SPIKE_ENTITY.with(|c| c.borrow().clone().expect("entity registered"));
        div()
            .key_context("RichInputSpike")
            .track_focus(&self.focus)
            .on_action(|_: &ToggleBold, _, _| {
                eprintln!("[spike] ToggleBold action fired — Cmd+B is wired");
            })
            .size_full()
            .bg(rgb(0x1e1e2e))
            .p_8()
            .child(TextElement { state: entity })
    }
}

thread_local! {
    static SPIKE_ENTITY: std::cell::RefCell<Option<Entity<RichTextSpike>>> =
        const { std::cell::RefCell::new(None) };
}

/// The Element that paints the rich text + caret and handles mouse-clicks.
struct TextElement {
    state: Entity<RichTextSpike>,
}

struct PrepaintData {
    line: WrappedLine,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintData;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspect_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let style = gpui::Style {
            size: size(
                gpui::relative(1.0).into(),
                gpui::DefiniteLength::from(px(28.0)).into(),
            ),
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspect_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let state = self.state.read(cx);
        let font = window.text_style().font();
        let runs = state.build_runs(font);
        let text = state.text.clone();
        let mut wrapped = window
            .text_system()
            .shape_text(text, px(16.0), &runs, None, None)
            .expect("shape_text failed");
        let line = wrapped.remove(0);
        PrepaintData { line }
    }

    fn paint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspect_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_height = px(28.0);
        let (focus, caret) = {
            let state = self.state.read(cx);
            (state.focus.clone(), state.caret)
        };

        // Register as IME target while focused.
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.state.clone()),
            cx,
        );

        // Paint the text. paint() returns Result<()> — surface failures.
        prepaint
            .line
            .paint(
                bounds.origin,
                line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            )
            .expect("paint line");

        // Paint caret as a 2-px tall quad at position_for_index(caret).
        if let Some(caret_pos) = prepaint.line.position_for_index(caret, line_height) {
            let caret_origin = bounds.origin + caret_pos;
            let caret_bounds = Bounds::new(caret_origin, size(px(2.0), line_height));
            window.paint_quad(fill(caret_bounds, white()));
        }

        // Mouse: click → set caret to byte-offset under cursor.
        // Clone what we need into the listener so it can outlive `state`.
        let state_handle = self.state.clone();
        let bounds_for_mouse = bounds;
        let line_for_mouse = prepaint.line.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, _phase, window, cx| {
            if !bounds_for_mouse.contains(&event.position) {
                return;
            }
            let local = event.position - bounds_for_mouse.origin;
            // closest_index_for_position returns Ok(idx) for an exact hit
            // and Err(idx) for the nearest fallback — both are usable.
            let idx = line_for_mouse
                .closest_index_for_position(local, line_height)
                .unwrap_or_else(|i| i);
            let focus = state_handle.read(cx).focus.clone();
            state_handle.update(cx, |s, cx| {
                s.caret = idx;
                cx.notify();
            });
            window.focus(&focus, cx);
            eprintln!("[spike] click → caret={idx}");
        });
    }
}

fn main() {
    eprintln!("rich_input_spike (Phase 0.2): bare-GPUI rich text architecture demo");
    let app = Application::with_platform(gpui_platform::current_platform(false));

    app.run(|cx: &mut App| {
        cx.bind_keys([gpui::KeyBinding::new(
            "cmd-b",
            ToggleBold,
            Some("RichInputSpike"),
        )]);

        let bounds = Bounds::centered(None, size(px(800.0), px(200.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                let entity = cx.new(|cx| RichTextSpike::new(window, cx));
                SPIKE_ENTITY.with(|c| {
                    *c.borrow_mut() = Some(entity.clone());
                });
                eprintln!("[spike] window opened — try clicking the text or pressing Cmd+B");
                entity
            },
        )
        .unwrap();

        cx.activate(true);
    });
}
