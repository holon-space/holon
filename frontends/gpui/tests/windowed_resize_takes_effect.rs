//! The windowed driver's resize must actually reach the window.
//!
//! Every width-sweep test in this fleet judges geometry it attributes to a
//! named viewport width. On the headless platform `Window::resize` stores new
//! bounds but fires no platform resize callback, so `Window::viewport_size`
//! keeps serving its cached value and no relayout runs — a sweep built on
//! `resize` alone measures ONE layout N times and passes vacuously. This pins
//! the paired call (`resize` + `bounds_changed`) that
//! `pbt_harness::windowed_wide::resize_window` makes, and pins that the bare
//! `resize` really is inert, so the helper cannot be "simplified" back into the
//! silent-pass shape.

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::sync::Arc;
use std::sync::Mutex;

use gpui::AppContext;
use gpui::AssetSource;
use gpui::Context;
use gpui::HeadlessAppContext;
use gpui::IntoElement;
use gpui::ParentElement;
use gpui::Render;
use gpui::Styled;
use gpui::Window;
use gpui::div;
use gpui::px;
use gpui::size;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::resize_window;

const HEIGHT: f32 = 800.0;
/// The same span the perception sweeps cover: the app's own `MIN_WIDTH` floor
/// up to a comfortable desktop width.
const WIDTHS: &[f32] = &[300.0, 360.0, 480.0, 640.0, 900.0, 1200.0];

/// Records the viewport width of every render pass, so the test can tell a
/// window that merely *reports* a new size from one that actually re-laid out
/// its contents at that size.
struct WidthRecorder {
    seen: Arc<Mutex<Vec<f32>>>,
}

impl Render for WidthRecorder {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let w = f32::from(window.viewport_size().width);
        self.seen.lock().expect("recorder mutex").push(w);
        div().w(px(w)).h(px(HEIGHT)).child("x")
    }
}

fn open(app: &mut HeadlessAppContext, seen: Arc<Mutex<Vec<f32>>>) -> gpui::AnyWindowHandle {
    app.open_window(size(px(WIDTHS[0]), px(HEIGHT)), |_, cx| {
        cx.new(|_| WidthRecorder { seen })
    })
    .expect("headless window opens")
    .into()
}

fn new_app() -> HeadlessAppContext {
    let assets: Arc<dyn AssetSource> = Arc::new(());
    HeadlessAppContext::with_platform(real_text_system(), assets, || {
        gpui_platform::current_headless_renderer()
    })
}

#[test]
fn resize_window_reaches_the_window_at_every_swept_width() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut app = new_app();
    let window = open(&mut app, seen.clone());
    app.run_until_parked();

    for &w in WIDTHS {
        let reported = resize_window(&mut app, window, w, HEIGHT);
        app.run_until_parked();
        assert!(
            (reported - w).abs() <= 1.0,
            "after resizing to {w}px the window reports a {reported:.1}px viewport, so every \
             measurement a sweep attributes to {w}px is really about a different window"
        );
    }

    // Reporting the width is not the same as laying out at it: a window that
    // answers `viewport_size` from fresh bounds but never re-renders would pass
    // the loop above while every registered bound stayed at the boot width.
    let rendered = seen.lock().expect("recorder mutex").clone();
    for &w in WIDTHS {
        assert!(
            rendered.iter().any(|r| (r - w).abs() <= 1.0),
            "no render pass ran at {w}px (passes: {rendered:?}), so the resize changed the \
             reported viewport without re-laying out the window's contents"
        );
    }
}

#[test]
fn a_bare_resize_is_inert_which_is_why_the_helper_exists() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut app = new_app();
    let window = open(&mut app, seen.clone());
    app.run_until_parked();

    let wide = *WIDTHS.last().expect("WIDTHS is not empty");
    let reported = app.update(|cx| {
        window
            .update(cx, |_, win, _| {
                win.resize(size(px(wide), px(HEIGHT)));
                f32::from(win.viewport_size().width)
            })
            .expect("window alive")
    });
    app.run_until_parked();

    assert!(
        (reported - WIDTHS[0]).abs() <= 1.0,
        "`Window::resize` alone now updates the viewport ({reported:.1}px after asking for \
         {wide}px, boot width {}px). The platform gained the resize callback this fleet works \
         around — drop the `bounds_changed` pairing in `resize_window` and delete this test.",
        WIDTHS[0]
    );
}

// Installs the windowed capturing tracing subscriber before this binary's first
// line of test code (see tests/test_init/mod.rs).
mod test_init;
