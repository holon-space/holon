//! Inc C — the sticky/in-flow accordion overlay: shared constants + the
//! observational spec (pure checkers) that BOTH the windowed dedicated test and
//! the shared-catalog PBT invariant bodies evaluate.
//!
//! The renderer tags three roles into the geometry registry; the checkers find
//! them by `widget_type` (no per-instance key needed), so one spec covers a
//! single stack and (Inc E) an N-section stack.
//!
//! Why the geometry lives here and not in the widget: the sticky footer is an
//! ABSOLUTE `.occlude()` overlay whose top is computed from OBSERVED bounds
//! (the previous frame's committed section/container geometry — the same
//! prepaint-lag seam the spike drove off a `ScrollHandle`). Encoding the
//! position law once, here, means the widget, the test, and the PBT invariant
//! all agree by construction.

/// `widget_type` of the section-stack scroll container (definite-height
/// parent).
pub const SECTION_STACK_CONTAINER_WIDGET: &str = "section_stack_container";
/// `widget_type` of one tracked section inside the stack.
pub const SECTION_WIDGET: &str = "section_stack_section";
/// `widget_type` of a painted sticky-accordion footer overlay.
pub const STICKY_FOOTER_WIDGET: &str = "accordion_sticky_footer";

/// Pixel tolerance for geometry equality (sub-pixel rounding across frames).
pub const TOL: f32 = 1.0;

/// A normalized rectangle + role, built from either a gpui `ElementInfo`
/// (windowed test) or a PBT `RenderedElement` (catalog body). Keeping the
/// checkers on this shape is what lets one spec serve both.
#[derive(Clone, Debug)]
pub struct ObservedRect {
    pub widget_type: String,
    pub entity_id: Option<String>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl ObservedRect {
    pub fn top(&self) -> f32 {
        self.y
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn left(&self) -> f32 {
        self.x
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    fn overlaps(&self, other: &ObservedRect) -> bool {
        let x_overlap = self.left() < other.right() - TOL && other.left() < self.right() - TOL;
        let y_overlap = self.top() < other.bottom() - TOL && other.top() < self.bottom() - TOL;
        x_overlap && y_overlap
    }
}

fn by_type<'a>(obs: &'a [ObservedRect], wt: &str) -> Vec<&'a ObservedRect> {
    obs.iter().filter(|o| o.widget_type == wt).collect()
}

/// The container's viewport bottom (definite parent bottom edge).
fn viewport_bottom(obs: &[ObservedRect]) -> Option<f32> {
    by_type(obs, SECTION_STACK_CONTAINER_WIDGET)
        .first()
        .map(|c| c.bottom())
}

/// The position law (spike M2): `footer.top == min(viewport_bottom − footer_h,
/// next_section_top)`. `next_section_top` = the top of the first section whose
/// top is BELOW the footer's owning section; absent (single/last section) ⇒
/// the footer simply pins to `viewport_bottom − footer_h`.
///
/// Evaluated from OBSERVED bounds only, so a divergence between the widget's
/// computed top and the law is caught even if both drift together.
pub fn check_position_spec(obs: &[ObservedRect]) -> Result<(), String> {
    let footers = by_type(obs, STICKY_FOOTER_WIDGET);
    let footer = footers
        .first()
        .ok_or_else(|| "[position-spec] no sticky footer overlay painted".to_string())?;
    let vp_bottom = viewport_bottom(obs).ok_or_else(|| {
        "[position-spec] no section-stack container to read viewport bottom from".to_string()
    })?;
    let pinned = vp_bottom - footer.h;
    // Next-section handoff: the topmost section that starts below the pinned
    // line pushes the footer up.
    let next_top = by_type(obs, SECTION_WIDGET)
        .iter()
        .map(|s| s.top())
        .filter(|t| *t > footer.top() + TOL)
        .fold(f32::INFINITY, f32::min);
    let expected = if next_top.is_finite() {
        pinned.min(next_top)
    } else {
        pinned
    };
    if (footer.top() - expected).abs() <= TOL {
        Ok(())
    } else {
        Err(format!(
            "[position-spec] footer.top={} != min(viewport_bottom−h={pinned}, next_section_top={next_top}) = {expected}",
            footer.top(),
        ))
    }
}

/// Exactly one sticky footer is painted (ownership): with N sticky accordions
/// only the bottom-most in-view section owns the active overlay.
pub fn check_exactly_one_footer(obs: &[ObservedRect]) -> Result<(), String> {
    let n = by_type(obs, STICKY_FOOTER_WIDGET).len();
    if n == 1 {
        Ok(())
    } else {
        Err(format!(
            "[exactly-one-footer] expected exactly 1 painted sticky footer, found {n}"
        ))
    }
}

/// No two painted footers overlap, and the active footer stays within the
/// container (never spilling past the viewport it is pinned inside).
pub fn check_no_footer_overlap(obs: &[ObservedRect]) -> Result<(), String> {
    let footers = by_type(obs, STICKY_FOOTER_WIDGET);
    for (i, a) in footers.iter().enumerate() {
        for b in &footers[i + 1..] {
            if a.overlaps(b) {
                return Err(format!(
                    "[no-overlap] two sticky footers overlap: {:?} vs {:?}",
                    a.entity_id, b.entity_id
                ));
            }
        }
    }
    if let (Some(f), Some(vp)) = (footers.first(), viewport_bottom(obs)) {
        if f.bottom() > vp + TOL {
            return Err(format!(
                "[no-overlap] footer bottom {} spills past viewport bottom {vp}",
                f.bottom()
            ));
        }
    }
    Ok(())
}

/// The px-cap holds under sticky: footer height ≤ `fraction × container_height
/// + TOL`. (Shrink-to-content — footer height < cap for small content — is
/// covered by driving the same checker with a short section.)
pub fn check_cap_under_sticky(obs: &[ObservedRect], fraction: f32) -> Result<(), String> {
    let footer = by_type(obs, STICKY_FOOTER_WIDGET)
        .first()
        .copied()
        .ok_or_else(|| "[cap-under-sticky] no sticky footer overlay painted".to_string())?;
    let container_h = by_type(obs, SECTION_STACK_CONTAINER_WIDGET)
        .first()
        .map(|c| c.h)
        .ok_or_else(|| "[cap-under-sticky] no container to size the cap against".to_string())?;
    let cap = fraction * container_h;
    if footer.h <= cap + TOL {
        Ok(())
    } else {
        Err(format!(
            "[cap-under-sticky] footer height {} exceeds cap {cap} (= {fraction} × {container_h})",
            footer.h
        ))
    }
}

/// Settle stability: the footer's bounds are identical across two consecutive
/// post-settle reads (the overlay must reach a fixed point, not oscillate as
/// the prepaint-lagged position converges).
pub fn check_settle_stability(a: &[ObservedRect], b: &[ObservedRect]) -> Result<(), String> {
    let fa = by_type(a, STICKY_FOOTER_WIDGET)
        .first()
        .copied()
        .ok_or_else(|| "[settle-stability] snapshot A has no footer".to_string())?
        .clone();
    let fb = by_type(b, STICKY_FOOTER_WIDGET)
        .first()
        .copied()
        .ok_or_else(|| "[settle-stability] snapshot B has no footer".to_string())?
        .clone();
    if (fa.x - fb.x).abs() <= TOL
        && (fa.y - fb.y).abs() <= TOL
        && (fa.w - fb.w).abs() <= TOL
        && (fa.h - fb.h).abs() <= TOL
    {
        Ok(())
    } else {
        Err(format!(
            "[settle-stability] footer moved between settled reads: {fa:?} -> {fb:?}"
        ))
    }
}

/// Overlay bounds committed (generalized): a painted overlay MUST commit
/// non-degenerate bounds into the registry. Absolute / `.occlude()` elements
/// have historically failed to register — assert the footer is present with
/// `w>0 && h>0`.
pub fn check_overlay_bounds_committed(obs: &[ObservedRect]) -> Result<(), String> {
    let footer = by_type(obs, STICKY_FOOTER_WIDGET)
        .first()
        .copied()
        .ok_or_else(|| {
            "[overlay-bounds-committed] no sticky footer overlay committed bounds".to_string()
        })?;
    if footer.w > TOL && footer.h > TOL {
        Ok(())
    } else {
        Err(format!(
            "[overlay-bounds-committed] footer committed degenerate bounds w={} h={}",
            footer.w, footer.h
        ))
    }
}

/// Run every single-snapshot checker; returns the list of failures (empty ⇒
/// all green). `fraction` is the accordion's `max_height_fraction`.
pub fn check_all_single(obs: &[ObservedRect], fraction: f32) -> Vec<String> {
    let mut fails = Vec::new();
    for r in [
        check_exactly_one_footer(obs),
        check_position_spec(obs),
        check_no_footer_overlap(obs),
        check_cap_under_sticky(obs, fraction),
        check_overlay_bounds_committed(obs),
    ] {
        if let Err(e) = r {
            fails.push(e);
        }
    }
    fails
}
