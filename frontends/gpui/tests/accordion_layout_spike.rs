//! Inc 0 spike (backlinks-bounded-region plan §5) — a PERMANENT executable
//! spec of the accordion bounded-footer layout idiom, built as a HAND-ROLLED
//! div tree (no product code) so the layout question is answered before any
//! widget work.
//!
//! Target shape (plan §4):
//!
//! ```text
//! div(W×H)                         // definite viewport
//!   flex_col
//!   ├─ div.flex_1.min_h_0.overflow_y_scroll   // MAIN region (scrolls)
//!   └─ div.flex_shrink_0.max_h(relative(f))   // ACCORDION region (pinned)
//!        ├─ header
//!        └─ div.flex_1.min_h_0.overflow_y_scroll  // capped body (scrolls)
//! ```
//!
//! Asserts, via `BoundsRegistry`:
//!   (a) many rows ⇒ region height ≤ f × viewport + ε (the CAP holds)
//!   (b) few rows  ⇒ region height ≈ content, below the cap (shrink-to-content)
//!   (c) a wheel over the capped body reveals a below-cap row (internal scroll)
//!
//! VERDICT (recorded 2026-07-23): `max_h(gpui::relative(f))` caps correctly
//! under Taffy in this shape — the percentage max-height resolves against the
//! definite-height absolute-free flex parent. The `viewport_size()` px fallback
//! (editor_view.rs:988) was NOT needed. See `cap_style()` below — flip it to
//! the px branch if a future gpui bump regresses relative max-height.
//!
//! Run: `cargo test -p holon-gpui --test accordion_layout_spike`

#[path = "support/mod.rs"]
mod support;

use gpui::Div;
use gpui::TestAppContext;
use gpui::Window;
use gpui::div;
use gpui::point;
use gpui::prelude::*;
use gpui::px;
use gpui::relative;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::geometry::TransparentTracker;
use support::simulate_wheel_at;

const VIEWPORT_W: f32 = 600.0;
const VIEWPORT_H: f32 = 400.0;
const FRACTION: f32 = 0.33;
const ROW_H: f32 = 24.0;
const HEADER_H: f32 = 20.0;
/// Cap in pixels for the current viewport (`0.33 × 400 = 132`).
const CAP_PX: f32 = FRACTION * VIEWPORT_H;
const EPS: f32 = 1.5;

const MANY_ROWS: usize = 40;
const FEW_ROWS: usize = 2;
/// A body row far below the cap fold — clipped to 0 until a wheel scrolls it
/// in.
const FAR_ACC_IX: usize = 37;

const REGION_WIDGET: &str = "accordion-spike-region";

fn acc_row_id(ix: usize) -> String {
    format!("acc-row-{ix}")
}

/// The accordion region cap. Relative (percentage) max-height is the primary
/// idiom; the commented px branch is the `window.viewport_size()` fallback
/// (editor_view.rs:988 precedent) kept ready for a Taffy regression.
fn apply_cap(d: Div, _viewport_h: f32) -> Div {
    d.max_h(relative(FRACTION))
    // Fallback if relative max-height regresses:
    // d.max_h(px(FRACTION * _viewport_h))
}

/// Hand-rolled view of the §4 target shape. No product widgets — this is the
/// pure layout idiom under test.
struct AccordionSpikeView {
    acc_rows: usize,
    bounds: BoundsRegistry,
}

impl gpui::Render for AccordionSpikeView {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        self.bounds.begin_pass();

        // MAIN region: enough rows to overflow (proves it is its own viewport,
        // independent of the accordion). Not asserted on here — the main-scroll
        // firewall lives in main_panel_scroll.rs.
        let mut main = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .id("spike-main")
            .overflow_y_scroll();
        for i in 0..30 {
            main = main.child(div().h(px(ROW_H)).w_full().child(format!("main {i}")));
        }

        // ACCORDION body: `acc_rows` fixed-height rows, each tracked by
        // entity_id so the wheel-reveal assertion can read visible heights.
        let mut body = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .id("spike-acc-body")
            .overflow_y_scroll();
        for i in 0..self.acc_rows {
            let row = div().h(px(ROW_H)).w_full().child(format!("acc {i}"));
            let tracked = TransparentTracker::new(
                acc_row_id(i),
                "acc-row",
                self.bounds.clone(),
                row.into_any_element(),
            )
            .with_entity_id(acc_row_id(i));
            body = body.child(tracked);
        }

        let header = div().h(px(HEADER_H)).w_full().child("Linked references");

        let region = apply_cap(div().flex_shrink_0().w_full().flex().flex_col(), VIEWPORT_H)
            .child(header)
            .child(body);

        // Track the region so the test can read its laid-out (== visible, it is
        // never ancestor-clipped) height.
        let tracked_region = TransparentTracker::new(
            "spike-region".to_string(),
            REGION_WIDGET,
            self.bounds.clone(),
            region.into_any_element(),
        );

        div()
            .w(px(VIEWPORT_W))
            .h(px(VIEWPORT_H))
            .flex()
            .flex_col()
            .child(main)
            .child(tracked_region)
    }
}

fn region_height(bounds: &BoundsRegistry) -> Option<f32> {
    bounds
        .all_elements()
        .iter()
        .find(|(_, info)| info.widget_type.as_ref() == REGION_WIDGET)
        .map(|(_, info)| info.height)
}

fn visible_height(bounds: &BoundsRegistry, entity_id: &str) -> Option<f32> {
    bounds
        .all_elements()
        .iter()
        .find(|(_, info)| info.entity_id.as_deref() == Some(entity_id))
        .map(|(_, info)| info.height)
}

#[gpui::test]
fn cap_holds_with_many_rows(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let bounds = BoundsRegistry::new();
    let (_e, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| AccordionSpikeView {
            acc_rows: MANY_ROWS,
            bounds,
        }
    });
    vcx.run_until_parked();
    bounds.flush();

    let h = region_height(&bounds).expect("region tracked");
    assert!(
        h <= CAP_PX + EPS,
        "many rows: accordion region height {h} must be capped at f×viewport \
         ({CAP_PX}); relative max-height is NOT holding under Taffy — switch \
         `apply_cap` to the px fallback"
    );
    assert!(
        h > CAP_PX - EPS,
        "region should reach the cap with many rows (got {h})"
    );
}

#[gpui::test]
fn shrinks_to_content_with_few_rows(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let bounds = BoundsRegistry::new();
    let (_e, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| AccordionSpikeView {
            acc_rows: FEW_ROWS,
            bounds,
        }
    });
    vcx.run_until_parked();
    bounds.flush();

    let h = region_height(&bounds).expect("region tracked");
    let content = HEADER_H + FEW_ROWS as f32 * ROW_H;
    assert!(
        h <= content + EPS,
        "few rows: region height {h} should shrink to content {content}, not the cap"
    );
    assert!(
        h < CAP_PX - 10.0,
        "few rows: region {h} must be clearly below cap {CAP_PX}"
    );
}

#[gpui::test]
fn wheel_reveals_below_cap_row(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let bounds = BoundsRegistry::new();
    let (_e, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| AccordionSpikeView {
            acc_rows: MANY_ROWS,
            bounds,
        }
    });
    vcx.run_until_parked();
    bounds.flush();

    let far_before = visible_height(&bounds, &acc_row_id(FAR_ACC_IX)).unwrap_or(0.0);
    assert!(
        far_before <= 0.0,
        "row {FAR_ACC_IX} should start clipped below the cap fold (got {far_before})"
    );

    // Wheel over the accordion body centre. The region sits at the panel
    // bottom: main takes `H - cap`, so the body centre is ~ H - cap/2.
    let acc_centre_y = VIEWPORT_H - CAP_PX / 2.0;
    simulate_wheel_at(
        vcx,
        point(px(VIEWPORT_W / 2.0), px(acc_centre_y)),
        px(-2000.0),
    );
    vcx.run_until_parked();
    bounds.flush();

    let far_after = visible_height(&bounds, &acc_row_id(FAR_ACC_IX)).unwrap_or(0.0);
    assert!(
        far_after > 0.0,
        "after a wheel over the capped accordion body, row {FAR_ACC_IX} should \
         have scrolled into view (nonzero visible height) but was still clipped \
         ({far_after}). The capped body's `min_h_0 + overflow_y_scroll` internal \
         viewport is not scrolling."
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
