//! Sticky-per-section-footer spike for Holon's Journals view — proves TWO
//! candidate mechanisms in a HAND-ROLLED div tree (no product widgets) so the
//! layout question is answered before any Journals feature work.
//!
//! ── FORK VERDICT (recorded 2026-07-23, gpui fork rev 44506e1) ──────────────
//!
//! There is NO "sticky" code in gpui core. Zed's `sticky_items`
//! (crates/ui/src/components/sticky_items.rs) is in a crate Holon does NOT
//! vendor. It is built on the `UniformListDecoration` trait — present on
//! `uniform_list` ONLY, never on the variable-height `list.rs` — and hard-codes
//! a FIXED `item_height` with TOP-only geometry:
//!   `drifting_y_offset = (anchor_top - scroll_top - sticky_area_h).min(ZERO)`,
//!   base_origin = bounds.origin.
//! => NOT reusable for variable-height journal sections. A bottom-edge sticky
//! footer over variable heights is a geometry rewrite, not a flag flip.
//!
//! The EAGER path (`gpui::ScrollHandle` on a plain `div().overflow_y_scroll()`)
//! is the real candidate. It exposes — and these WORK with variable-height
//! children — `.offset()`, `.bounds()` (viewport), `.bottom_item()`,
//! `.top_item()`, `.bounds_for_item(ix)` (laid-out child bounds in WINDOW
//! coords, refreshed after scroll), `.max_offset()`. M2 below builds the
//! footer as a hand-rolled absolute overlay whose top is COMPUTED each render
//! from that handle.
//!
//! ── M1 (native-seam plumbing proof + its limit) ────────────────────────────
//! `uniform_list(...).with_decoration(impl UniformListDecoration)` paints ONE
//! element pinned at the list-top viewport edge. GREEN test proves the
//! decoration seam is reachable from OUR tree and the pin holds under scroll.
//! FINDING (not a failure): the seam is uniform-height + top-only, so it does
//! NOT apply to variable-height journal FOOTERS. Cost is documented at
//! `TopPinDecoration` — counter-scrolling the base origin by `-scroll_offset`
//! is the whole mechanism; there is no bottom-edge or variable-height
//! affordance in the trait's `compute(range, bounds, scroll_offset,
//! item_height, ...)` contract (item_height is a single scalar).
//!
//! ── M2 (hand-rolled overlay on the eager path — the real candidate) ─────────
//! N=4 VARIABLE-height sections in one `div().relative().w(W).h(H)` definite
//! viewport. A `.flex_1().min_h_0().overflow_y_scroll().track_scroll(&h)`
//! scroller holds the sections; a SIBLING `.absolute()` footer overlay is
//! positioned from the `ScrollHandle` every render (see `compute_footer_top`).
//! The overlay's accordion cap is `max_h(px(f * H))` where H is the definite
//! `.relative()` container height. FINDING: `max_h(relative(f))` does NOT cap
//! an ABSOLUTELY-positioned overlay in this fork (Taffy leaves it at content
//! height), unlike the in-flow accordion child — so the fraction-of-container
//! cap is computed in Rust. Wheel isolation needs `.occlude()` on the overlay
//! (without it the wheel scrolls both the footer and the outer scroller).
//!
//! Run: `cargo test -p holon-gpui --test sticky_footer_spike`

#[path = "support/mod.rs"]
mod support;

use std::ops::Range;

use gpui::AnyElement;
use gpui::App;
use gpui::Bounds;
use gpui::Div;
use gpui::Entity;
use gpui::Pixels;
use gpui::Point;
use gpui::ScrollHandle;
use gpui::TestAppContext;
use gpui::UniformListScrollHandle;
use gpui::VisualTestContext;
use gpui::Window;
use gpui::div;
use gpui::point;
use gpui::prelude::*;
use gpui::px;
use gpui::relative;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::geometry::TransparentTracker;
use support::simulate_wheel_at;

// ── Shared geometry ────────────────────────────────────────────────────────
const W: f32 = 600.0;
const H: f32 = 400.0;
const ROW_H: f32 = 24.0;
const SEC_HEADER_H: f32 = 24.0;
const EPS: f32 = 2.5;

// M1
const M1_ITEMS: usize = 40;

// M2 sections — DIFFERENT body row counts so section heights differ.
// Heights (header + rows*24): s0=120 s1=264 s2=168 s3=360, total=912 > H.
const SECTION_ROWS: [usize; 4] = [4, 10, 6, 14];

// M2 footer accordion. The cap is a FRACTION of the definite container height
// H, so the overlay's max-height has a definite base to resolve against.
const FOOTER_HEADER_H: f32 = 24.0;
const FOOTER_BODY_ROWS: usize = 10; // content 240 > cap ⇒ body scrolls
const FOOTER_CAP_FRACTION: f32 = 0.4; // 0.4 * 400 = 160
const FOOTER_CAP_PX: f32 = FOOTER_CAP_FRACTION * H; // 160
/// Expanded overlay height once the cap bites (content exceeds the cap).
const FOOTER_H_EXPANDED: f32 = FOOTER_CAP_PX; // 160
/// Collapsed overlay = header only.
const FOOTER_H_COLLAPSED: f32 = FOOTER_HEADER_H; // 24
/// A footer body row far below the cap fold — clipped until an internal wheel.
const FAR_FOOT_ROW: usize = 9;
/// A row deep in the last section, below the outer fold at rest.
const FAR_SECTION: usize = 3;
const FAR_SECTION_ROW: usize = 12;

fn m1_row_id(ix: usize) -> String {
    format!("m1-row-{ix}")
}
fn sec_row_id(s: usize, r: usize) -> String {
    format!("sec-{s}-row-{r}")
}
fn section_id(s: usize) -> String {
    format!("section-{s}")
}
fn foot_row_id(r: usize) -> String {
    format!("foot-row-{r}")
}

fn info(bounds: &BoundsRegistry, entity: &str) -> Option<ElementInfo> {
    bounds
        .all_elements()
        .into_iter()
        .find(|(_, i)| i.entity_id.as_deref() == Some(entity))
        .map(|(_, i)| i)
}

fn visible_height(bounds: &BoundsRegistry, entity: &str) -> f32 {
    info(bounds, entity).map(|i| i.height).unwrap_or(0.0)
}

// ── M1: native uniform_list decoration seam ────────────────────────────────

/// The whole M1 mechanism: the decoration is prepainted at
/// `padded_bounds.origin + scroll_offset` (uniform_list.rs:501-518), so to pin
/// an element at the list-top VIEWPORT edge we counter-scroll it by
/// `-scroll_offset.y`. That is the entire cost — but note the trait hands us a
/// single scalar `item_height` and TOP-anchored `bounds.origin`; there is no
/// bottom edge and no per-item height, which is exactly why this seam cannot
/// carry a variable-height journal footer.
struct TopPinDecoration {
    bounds: BoundsRegistry,
}

impl gpui::UniformListDecoration for TopPinDecoration {
    fn compute(
        &self,
        _visible_range: Range<usize>,
        _bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
        _item_height: Pixels,
        _item_count: usize,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        let pin = div()
            .absolute()
            .top(px(-f32::from(scroll_offset.y)))
            .left(px(0.0))
            .w(px(W))
            .h(px(ROW_H))
            .child("PINNED");
        let tracked = TransparentTracker::new(
            "m1-pin".to_string(),
            "m1-pin",
            self.bounds.clone(),
            pin.into_any_element(),
        )
        .with_entity_id("m1-pin");
        div()
            .relative()
            .size_full()
            .child(tracked)
            .into_any_element()
    }
}

struct M1DecorView {
    list_handle: UniformListScrollHandle,
    bounds: BoundsRegistry,
}

impl gpui::Render for M1DecorView {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        self.bounds.begin_pass();
        let row_bounds = self.bounds.clone();
        let handle = self.list_handle.clone();

        let list = gpui::uniform_list("m1-list", M1_ITEMS, move |range: Range<usize>, _w, _cx| {
            range
                .map(|i| {
                    let row = div().h(px(ROW_H)).w_full().child(format!("item {i}"));
                    TransparentTracker::new(
                        m1_row_id(i),
                        "m1-row",
                        row_bounds.clone(),
                        row.into_any_element(),
                    )
                    .with_entity_id(m1_row_id(i))
                    .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&handle)
        .with_decoration(TopPinDecoration {
            bounds: self.bounds.clone(),
        })
        .w_full()
        .h_full();

        div().w(px(W)).h(px(H)).child(list)
    }
}

#[gpui::test]
fn m1_uniform_list_decoration_pins_at_top(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let bounds = BoundsRegistry::new();
    let handle = UniformListScrollHandle::new();
    let (entity, vcx) = cx.add_window_view({
        let (b, h) = (bounds.clone(), handle.clone());
        move |_, _| M1DecorView {
            list_handle: h,
            bounds: b,
        }
    });
    vcx.run_until_parked();
    bounds.flush();

    let pin_before = info(&bounds, "m1-pin").expect("pin element tracked").y;
    let row0_before = info(&bounds, &m1_row_id(0)).expect("row 0 tracked").y;

    // Scroll the uniform_list down. Wheel over the list body centre.
    simulate_wheel_at(vcx, point(px(W / 2.0), px(H / 2.0)), px(-300.0));
    // The decoration recomputes on the next paint; force a render.
    entity.update(&mut vcx.cx.clone(), |_, cx| cx.notify());
    vcx.run_until_parked();
    bounds.flush();

    let pin_after = info(&bounds, "m1-pin").expect("pin element tracked").y;
    let row0_after = visible_height(&bounds, &m1_row_id(0));

    // Non-vacuous: content actually moved (row 0 scrolled up out of view).
    assert!(
        row0_before >= 0.0 && row0_after <= 0.0,
        "list did not scroll (row0 y before={row0_before}, \
         visible-height after={row0_after}); M1 assertion would be vacuous"
    );
    // The seam holds: the pinned element stayed at the list-top edge.
    assert!(
        (pin_after - pin_before).abs() <= EPS,
        "pinned decoration drifted from the top under scroll: \
         before={pin_before} after={pin_after}"
    );
}

// ── M2: hand-rolled overlay on the eager ScrollHandle path ──────────────────

/// LOAD-BEARING machinery — the production cost proxy. Given the eager
/// `ScrollHandle` and the current footer height, compute the overlay's top so
/// the footer of the bottom-most in-view section pins to the viewport bottom
/// and hands off to the incoming section: the next section's top pushes the
/// outgoing footer up. (7 lines of geometry.)
fn compute_footer_top(sh: &ScrollHandle, footer_h: f32) -> f32 {
    let vp = sh.bounds();
    let pinned = f32::from(vp.bottom()) - footer_h;
    let bottom_ix = sh.bottom_item();
    match sh.bounds_for_item(bottom_ix + 1) {
        Some(next) => pinned.min(f32::from(next.top())),
        None => pinned,
    }
}

struct M2StickyView {
    scroll: ScrollHandle,
    bounds: BoundsRegistry,
    collapsed: bool,
    /// When set, a greedy `relative(1.0)` child (R4 class) is placed inside the
    /// capped footer body to prove the cap still holds.
    greedy: bool,
}

impl M2StickyView {
    fn footer_h(&self) -> f32 {
        if self.collapsed {
            FOOTER_H_COLLAPSED
        } else {
            FOOTER_H_EXPANDED
        }
    }
}

fn build_section(s: usize, nrows: usize, bounds: &BoundsRegistry) -> Div {
    let mut sec = div().w_full().flex().flex_col();
    sec = sec.child(div().h(px(SEC_HEADER_H)).w_full().child(format!("§{s}")));
    for r in 0..nrows {
        let row = div().h(px(ROW_H)).w_full().child(format!("s{s} r{r}"));
        let tracked = TransparentTracker::new(
            sec_row_id(s, r),
            "m2-row",
            bounds.clone(),
            row.into_any_element(),
        )
        .with_entity_id(sec_row_id(s, r));
        sec = sec.child(tracked);
    }
    sec
}

impl gpui::Render for M2StickyView {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        self.bounds.begin_pass();

        // Scroller: definite viewport comes from the `.relative().h(H)` root;
        // `flex_1 + min_h_0 + overflow_y_scroll` makes it its own scroll region.
        // Each section is a DIRECT child (wrapped transparently), so the
        // ScrollHandle's `child_bounds` are per-section.
        let mut scroller = div()
            .id("m2-scroller")
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        for (s, &nrows) in SECTION_ROWS.iter().enumerate() {
            let sec = build_section(s, nrows, &self.bounds);
            let tracked = TransparentTracker::new(
                section_id(s),
                "m2-section",
                self.bounds.clone(),
                sec.into_any_element(),
            )
            .with_entity_id(section_id(s));
            scroller = scroller.child(tracked);
        }

        // Footer overlay: absolutely positioned, top COMPUTED from the handle.
        let footer_h = self.footer_h();
        let footer_top = compute_footer_top(&self.scroll, footer_h);
        let bottom_ix = self.scroll.bottom_item();

        let mut footer = div().w(px(W)).flex().flex_col().child(
            div()
                .h(px(FOOTER_HEADER_H))
                .w_full()
                .child(format!("footer §{bottom_ix}")),
        );
        if !self.collapsed {
            // Accordion cap. FINDING: `max_h(relative(f))` does NOT cap an
            // ABSOLUTELY-positioned overlay (Taffy leaves it at content
            // height, verified at 264px). Unlike the in-flow flex child in
            // `accordion_layout_spike.rs`, an absolute element's percentage
            // max-height does not resolve against its containing block here.
            // So the definite height still ORIGINATES from the container H,
            // but we compute it in Rust: px(FOOTER_CAP_FRACTION * H).
            footer = footer.max_h(px(FOOTER_CAP_PX));
            let mut body = div()
                .id("m2-foot-body")
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_y_scroll();
            for r in 0..FOOTER_BODY_ROWS {
                let row = div().h(px(ROW_H)).w_full().child(format!("foot r{r}"));
                let tracked = TransparentTracker::new(
                    foot_row_id(r),
                    "m2-foot-row",
                    self.bounds.clone(),
                    row.into_any_element(),
                )
                .with_entity_id(foot_row_id(r));
                body = body.child(tracked);
            }
            if self.greedy {
                // R4-class greedy child: wants 100% of the parent height.
                // The overlay cap must still clamp the whole footer.
                body = body.child(div().w_full().h(relative(1.0)).child("greedy"));
            }
            footer = footer.child(body);
        }

        let overlay = div()
            .absolute()
            .top(px(footer_top))
            .left(px(0.0))
            // `.occlude()` makes the overlay opaque to hit-testing so a wheel
            // over the footer does NOT also reach the outer scroller behind
            // it. Without this, gpui propagates the wheel to BOTH scrollers
            // (see `m2_wheel_over_footer_does_not_scroll_outer`).
            .occlude()
            .child(footer);
        let tracked_footer = TransparentTracker::new(
            "m2-footer".to_string(),
            "m2-footer",
            self.bounds.clone(),
            overlay.into_any_element(),
        )
        .with_entity_id("m2-footer");

        div()
            .relative()
            .flex()
            .flex_col()
            .w(px(W))
            .h(px(H))
            .child(scroller)
            .child(tracked_footer)
    }
}

/// Open the M2 view, returning the entity, visual cx, its scroll handle, and
/// the bounds registry. The overlay's top depends on the handle populated by
/// the PREVIOUS frame's prepaint, so a fresh window needs a couple of settle
/// frames before its position is correct.
fn open_m2(
    cx: &mut TestAppContext,
    collapsed: bool,
    greedy: bool,
) -> (
    Entity<M2StickyView>,
    &mut VisualTestContext,
    ScrollHandle,
    BoundsRegistry,
) {
    cx.update(|cx| gpui_component::init(cx));
    let bounds = BoundsRegistry::new();
    let scroll = ScrollHandle::new();
    let (entity, vcx) = cx.add_window_view({
        let (b, s) = (bounds.clone(), scroll.clone());
        move |_, _| M2StickyView {
            scroll: s,
            bounds: b,
            collapsed,
            greedy,
        }
    });
    vcx.run_until_parked();
    settle(&entity, vcx, &bounds);
    (entity, vcx, scroll, bounds)
}

/// Force re-renders so the eager overlay reads the freshly-populated handle
/// and converges (the overlay is out-of-flow, so section geometry is stable
/// and two frames are enough).
fn settle(entity: &Entity<M2StickyView>, vcx: &mut VisualTestContext, bounds: &BoundsRegistry) {
    for _ in 0..3 {
        entity.update(&mut vcx.cx.clone(), |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    bounds.flush();
}

#[gpui::test]
fn m2_footer_pinned_at_viewport_bottom(cx: &mut TestAppContext) {
    let (_e, _vcx, scroll, bounds) = open_m2(cx, false, false);

    let footer = info(&bounds, "m2-footer").expect("footer overlay tracked");
    // Trap: overlay must commit NONZERO bounds — invisible overlays hide bugs.
    assert!(
        footer.height > 0.0 && footer.width > 0.0,
        "footer overlay committed zero size {}x{}",
        footer.width,
        footer.height
    );
    let vp_bottom = f32::from(scroll.bounds().bottom());
    assert!(
        (footer.y + footer.height - vp_bottom).abs() <= EPS,
        "footer bottom {} not pinned to viewport bottom {vp_bottom}",
        footer.y + footer.height
    );
}

#[gpui::test]
fn m2_footer_overlay_bounds_registered(cx: &mut TestAppContext) {
    // The overlay MUST commit correct bounds to BoundsRegistry while sticky.
    let (_e, _vcx, scroll, bounds) = open_m2(cx, false, false);
    let footer = info(&bounds, "m2-footer").expect("overlay must register");
    let expect_top = compute_footer_top(&scroll, FOOTER_H_EXPANDED);
    assert!(
        (footer.y - expect_top).abs() <= EPS,
        "registered footer top {} disagrees with computed {expect_top}",
        footer.y
    );
    // Left-aligned to the viewport, full width.
    assert!(
        footer.x.abs() <= EPS && (footer.width - W).abs() <= EPS,
        "overlay not aligned/sized: x={} w={}",
        footer.x,
        footer.width
    );
}

#[gpui::test]
fn m2_sections_and_footer_nonzero_height(cx: &mut TestAppContext) {
    // Trap: `size_full` inside an indefinite-height context collapses to 0.
    // Every section and the footer must have nonzero height.
    // Sections 0 and 1 span the viewport at rest and must paint nonzero
    // height. (Sections below the fold are legitimately clipped to 0 VISIBLE
    // height — their reachability is proven by the below-fold reveal test,
    // not by a collapse-to-0 that would signal the size_full trap.)
    let (_e, _vcx, _scroll, bounds) = open_m2(cx, false, false);
    for s in 0..2 {
        let h = visible_height(&bounds, &section_id(s));
        assert!(h > 0.0, "in-view section {s} collapsed to 0 height");
    }
    assert!(
        visible_height(&bounds, "m2-footer") > 0.0,
        "footer collapsed to 0 height"
    );
}

#[gpui::test]
fn m2_handoff_pushes_footer_up(cx: &mut TestAppContext) {
    let (entity, vcx, scroll, bounds) = open_m2(cx, false, false);

    let top_pinned = info(&bounds, "m2-footer").expect("footer tracked").y;

    // Scroll so a section boundary crosses the footer line. Wheel over the
    // upper scroller region (footer covers the lower ~160px, so hit y=80).
    simulate_wheel_at(vcx, point(px(W / 2.0), px(80.0)), px(-140.0));
    settle(&entity, vcx, &bounds);

    let bottom_ix = scroll.bottom_item();
    let footer_top = info(&bounds, "m2-footer").expect("footer tracked").y;

    // HANDOFF: the incoming section's top pushes the outgoing footer up, so
    // the footer sits at or above the pinned line.
    assert!(
        footer_top <= top_pinned + EPS,
        "footer did not get pushed up on handoff: pinned={top_pinned} \
         after-scroll={footer_top}"
    );
    // Non-overlap guarantee of the spec formula: the footer top is never
    // rendered BELOW the incoming section's top (min(pinned, next.top)).
    if let Some(next) = scroll.bounds_for_item(bottom_ix + 1) {
        assert!(
            footer_top <= f32::from(next.top()) + EPS,
            "footer top {footer_top} rendered below incoming section top {}",
            f32::from(next.top())
        );
    }
}

#[gpui::test]
fn m2_footer_cap_holds(cx: &mut TestAppContext) {
    // Accordion cap: `max_h(relative(f))` on the overlay resolves against the
    // definite `.relative()` container and clamps the expanded footer.
    let (_e, _vcx, _scroll, bounds) = open_m2(cx, false, false);
    let h = visible_height(&bounds, "m2-footer");
    assert!(
        h <= FOOTER_CAP_PX + EPS,
        "footer height {h} exceeds cap {FOOTER_CAP_PX}; the px cap derived from \
         the definite container height is not clamping the overlay"
    );
    assert!(
        h > FOOTER_CAP_PX - EPS,
        "footer {h} should reach the cap {FOOTER_CAP_PX} (content exceeds it)"
    );
}

#[gpui::test]
fn m2_greedy_child_does_not_break_cap(cx: &mut TestAppContext) {
    // R4-class trap: a greedy `relative(1.0)` child inside the capped footer
    // must not blow the cap.
    let (_e, _vcx, _scroll, bounds) = open_m2(cx, false, true);
    let h = visible_height(&bounds, "m2-footer");
    assert!(
        h <= FOOTER_CAP_PX + EPS,
        "greedy relative(1.0) child broke the footer cap: height {h} > \
         {FOOTER_CAP_PX}"
    );
}

#[gpui::test]
fn m2_collapse_expand_frees_height(cx: &mut TestAppContext) {
    // Live collapse/expand via an entity field frees the height.
    let (entity, vcx, _scroll, bounds) = open_m2(cx, true, false);
    let collapsed_h = visible_height(&bounds, "m2-footer");

    entity.update(&mut vcx.cx.clone(), |v, cx| {
        v.collapsed = false;
        cx.notify();
    });
    settle(&entity, vcx, &bounds);
    let expanded_h = visible_height(&bounds, "m2-footer");

    assert!(
        (collapsed_h - FOOTER_H_COLLAPSED).abs() <= EPS,
        "collapsed footer height {collapsed_h} != {FOOTER_H_COLLAPSED}"
    );
    assert!(
        expanded_h > collapsed_h + 10.0,
        "expand did not free height: collapsed={collapsed_h} \
         expanded={expanded_h}"
    );
}

#[gpui::test]
fn m2_footer_internal_scroll_reveals_row(cx: &mut TestAppContext) {
    // Trap: missing `min_h_0` freezes internal scroll. A below-cap footer row
    // must be reachable by a wheel OVER the footer body.
    let (entity, vcx, _scroll, bounds) = open_m2(cx, false, false);

    let before = visible_height(&bounds, &foot_row_id(FAR_FOOT_ROW));
    assert!(
        before <= 0.0,
        "footer row {FAR_FOOT_ROW} should start clipped below the cap fold \
         (got {before})"
    );

    // Footer covers the lower ~160px of the viewport; wheel at its centre.
    let footer_centre_y = H - FOOTER_CAP_PX / 2.0;
    simulate_wheel_at(vcx, point(px(W / 2.0), px(footer_centre_y)), px(-500.0));
    settle(&entity, vcx, &bounds);

    let after = visible_height(&bounds, &foot_row_id(FAR_FOOT_ROW));
    assert!(
        after > 0.0,
        "wheel over the capped footer body did not reveal row \
         {FAR_FOOT_ROW}; `min_h_0 + overflow_y_scroll` internal viewport is \
         not scrolling (still {after})"
    );
}

#[gpui::test]
fn m2_below_fold_section_row_reachable(cx: &mut TestAppContext) {
    // N=4 variable heights: a below-fold section row is reachable by wheeling
    // the OUTER scroller (pointer over the upper region, clear of the footer).
    let (entity, vcx, _scroll, bounds) = open_m2(cx, false, false);

    let target = sec_row_id(FAR_SECTION, FAR_SECTION_ROW);
    let before = visible_height(&bounds, &target);
    assert!(
        before <= 0.0,
        "deep row {target} should start below the outer fold (got {before})"
    );

    simulate_wheel_at(vcx, point(px(W / 2.0), px(60.0)), px(-2000.0));
    settle(&entity, vcx, &bounds);

    let after = visible_height(&bounds, &target);
    assert!(
        after > 0.0,
        "wheel over the outer scroller did not reveal {target} (still {after})"
    );
}

#[gpui::test]
fn m2_footers_correct_at_extremes(cx: &mut TestAppContext) {
    // At rest (top) the footer belongs to whichever section spans the viewport
    // bottom; after a full scroll down it belongs to the LAST section.
    let (entity, vcx, scroll, bounds) = open_m2(cx, false, false);

    // At rest the registered footer top must equal the machinery output.
    let rest_top = info(&bounds, "m2-footer").expect("footer tracked").y;
    let rest_expect = compute_footer_top(&scroll, FOOTER_H_EXPANDED);
    assert!(
        (rest_top - rest_expect).abs() <= EPS,
        "at rest footer top {rest_top} != computed {rest_expect}"
    );

    simulate_wheel_at(vcx, point(px(W / 2.0), px(60.0)), px(-5000.0));
    settle(&entity, vcx, &bounds);

    assert_eq!(
        scroll.bottom_item(),
        SECTION_ROWS.len() - 1,
        "fully scrolled: bottom section should be the last one"
    );
    let bottom_top = info(&bounds, "m2-footer").expect("footer tracked").y;
    let bottom_expect = compute_footer_top(&scroll, FOOTER_H_EXPANDED);
    assert!(
        (bottom_top - bottom_expect).abs() <= EPS,
        "fully scrolled footer top {bottom_top} != computed {bottom_expect}"
    );
}

// ── Wheel routing (documented; asserted where feasible) ─────────────────────
//
// gpui routes a `ScrollWheelEvent` to the hitbox under the pointer
// (`Window::mouse_hit_test`). FINDING: painting the overlay AFTER the scroller
// is NOT enough — without `.occlude()` on the overlay, a wheel over the footer
// scrolls BOTH the footer's internal `overflow_y_scroll` body AND the outer
// scroller behind it (verified: outer offset moved -500). `.occlude()` makes
// the overlay opaque to hit-testing so the wheel is contained. With it, a
// wheel over the footer moves only footer content; a wheel over the upper
// (footer-free) region moves only the outer scroller. The reveal tests above
// are that discrimination in action:
//   - `m2_footer_internal_scroll_reveals_row` wheels at y = H - cap/2 (over the
//     footer) and moves ONLY footer content.
//   - `m2_below_fold_section_row_reachable` wheels at y = 60 (clear of the
//     footer) and moves ONLY the outer scroller.
// This next test asserts the cross-isolation directly: a wheel over the footer
// must NOT scroll the outer sections.

#[gpui::test]
fn m2_wheel_over_footer_does_not_scroll_outer(cx: &mut TestAppContext) {
    let (entity, vcx, scroll, bounds) = open_m2(cx, false, false);
    let outer_offset_before = f32::from(scroll.offset().y);

    // Wheel squarely over the footer overlay.
    let footer_centre_y = H - FOOTER_CAP_PX / 2.0;
    simulate_wheel_at(vcx, point(px(W / 2.0), px(footer_centre_y)), px(-500.0));
    settle(&entity, vcx, &bounds);

    let outer_offset_after = f32::from(scroll.offset().y);
    assert!(
        (outer_offset_after - outer_offset_before).abs() <= EPS,
        "wheel over the footer leaked into the outer scroller: \
         before={outer_offset_before} after={outer_offset_after}"
    );
}

// ── Nested-shell interplay (documented probe) ───────────────────────────────
//
// Question: if a section were wrapped in a `size_full + overflow_y_scroll`
// shell (as production `scrollable_list_wrapper` does for collection-backed
// nodes), would the absolute footer overlay still stick? The overlay is a
// SIBLING of the outer scroller and anchors to the `.relative()` root — a
// section's inner shell is a DESCENDANT of the scroller and cannot become the
// overlay's containing block, so overlay placement is unaffected. What a
// `size_full` inner shell DOES risk is the section itself collapsing to 0
// height inside the content-sized flex column (the BugFunnel 230 / main-panel
// class) — which is exactly what `m2_sections_and_footer_nonzero_height`
// guards against here. A full nested-collection reproduction belongs in
// `panel_scroll_spike.rs` (real `ReactiveShell` path), not this pure-layout
// spike, so it is documented rather than re-implemented.
