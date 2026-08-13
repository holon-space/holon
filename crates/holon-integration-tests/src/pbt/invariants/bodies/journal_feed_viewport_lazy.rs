//! `inv-journal-feed-viewport-lazy`.
//!
//! @pbt oracle correspondence
//! @pbt covers viewport-bounded expansion of journals-feed day pages — a day
//!   outside the rendered window must not be expanded, and a day inside it must
//!   be
//! @pbt slips-if-removed every day page in the feed stays expanded regardless
//!   of the window, so each one materialises a `live_query` shell, a watched
//!   matview and a CDC subscription and render cost scales with history
//!
//! Windowed only — headless slices supply no geometry and deselect.
//!
//! # What "on screen" is MEASURED to be
//!
//! Registration is not the signal: the outline registers bounds for every row
//! it lays out, visible or not. On the journals rung a 60-day feed registers
//! all 60 while the panel shows 30, so an oracle reading presence in the
//! registry would call the whole feed on screen and convict nothing.
//!
//! Two signals separate the visible rows from the rest, and both were dumped
//! from a real frame (panel box `108..1080`, `lane-logs/a1-verify.md` DEFECT
//! 2):
//!
//! ```text
//! block:jday-000 y=136  h=26   …  block:jday-029 y=1064 h=16   ← on screen
//! block:jday-030 y=1096 h=0.0  …  block:jday-059 y=2024 h=0.0  ← off screen
//! ```
//!
//! POSITION — where the row was laid out relative to the panel's box — and
//! CLIP — the renderer collapsing an invisible row to zero height. They are
//! produced by different parts of the frontend and they agreed on all 60 rows
//! in that frame. Neither is taken alone: a day counts as on screen only when
//! BOTH say so, and off screen only when both say so. A row the two disagree
//! about is UNDECIDED, and one such row ends the evaluation: the invariant
//! returns [`InvariantResult::Skipped`] naming the rows rather than judging the
//! remainder. Judging around them would make the off-screen arm strictly weaker
//! than either signal alone — a row below the fold that the renderer did not
//! clip is off screen by position, on screen by clip, and would be convicted by
//! neither.
//!
//! An earlier revision of this module claimed box intersection as the signal
//! and named clipping nowhere. That was untrue: with every off-screen row
//! already at `h == 0`, the intersection test decided nothing, and stretching
//! the panel's bottom edge to `+1e6` left the classification unchanged.
//!
//! # Why reading the viewport from the SUT does not blind the oracle
//!
//! The window comes from the SUT, which normally lets a SUT defect hide behind
//! an oracle that shares it. It does not here, for two independent reasons.
//!
//! Both signals are produced by LAYOUT — where rows land, how tall the panel
//! is, what the renderer clips — while the property under test is the expansion
//! policy, which decides what to MATERIALISE. Nothing that changes the second
//! can move the first, so the input cannot be tuned to make this invariant
//! pass.
//!
//! And the reported window is not taken on trust: [`Self::check_viewport_laws`]
//! holds it to three properties a wrong window fails — it fits the panel's
//! measured capacity ([`onscreen_capacity`]), it contains the focused day, and
//! it is contiguous in feed order. Where capacity cannot refute a SUT that
//! reports its whole history as on screen, the invariant says so
//! ([`InvariantResult::Skipped`]) instead of passing for free.

use std::collections::BTreeSet;

use holon_api::EntityUri;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefJournalFeed;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::RenderedElement;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::WidgetSnapshot;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

/// Floor on a feed day row's painted height: a row carries at least one line of
/// text, measured at 16px on the windowed rung (`block:jday-029 h=16`, the row
/// straddling the fold). Rows there are normally 26-32px, so this is a floor
/// and not an average — it makes [`onscreen_capacity`] an over-estimate of how
/// many rows can fit, which is the safe direction for a ceiling.
const MIN_FEED_ROW_HEIGHT: f32 = 16.0;

/// The most day rows that can be VISIBLE in a panel `panel_height` px tall.
///
/// Derived per frame from the panel's own measured box rather than fixed as a
/// constant, because a constant drifts: the previous `MAX_ONSCREEN_FEED_DAYS =
/// 64` sat above the rung's 60-day fixture, so a SUT claiming all 60 rows were
/// on screen fitted under the ceiling and blinded the whole oracle.
///
/// Deliberately counts only what fits ON screen. Rows a renderer keeps warm
/// just outside the viewport are not visible, whatever it prefetches — an
/// overscan allowance belongs to the expansion policy A2 lands, not to the
/// meaning of "on screen".
pub fn onscreen_capacity(panel_height: f32) -> usize {
    (panel_height / MIN_FEED_ROW_HEIGHT).ceil().max(1.0) as usize
}

pub struct InvJournalFeedViewportLazy;

impl InvJournalFeedViewportLazy {
    pub const ID: InvariantId = InvariantId("inv-journal-feed-viewport-lazy");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvJournalFeedViewportLazy
where
    R: RefJournalFeed + RefLayout + RefViewSelection + RefFocus,
    S: SutRenderer + SutLayout,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        if !sut.root_render_ready().await {
            return InvariantResult::Skipped(
                "root render not ready (loading / spacer / not watchable / interpret panic)".into(),
            );
        }

        let feed_days = ref_.feed_day_pages();
        if feed_days.is_empty() {
            return InvariantResult::Skipped(
                "journals feed is not the rendered Main root, so it draws no day pages".into(),
            );
        }

        let elements = sut.rendered_elements().await;
        if elements.is_empty() {
            return InvariantResult::Skipped(
                "no geometry registered — this slice installs no viewport provider".into(),
            );
        }

        Self::evaluate(
            ref_,
            &feed_days,
            &elements,
            &sut.widget_tree_snapshot().await,
        )
    }
}

impl InvJournalFeedViewportLazy {
    /// Snapshot-in / result-out evaluation, so the laws and the expansion rule
    /// are judged against ONE frame.
    fn evaluate<R>(
        ref_: &R,
        feed_days: &[EntityUri],
        elements: &[RenderedElement],
        root: &WidgetSnapshot,
    ) -> InvariantResult
    where
        R: RefJournalFeed + RefLayout + RefViewSelection + RefFocus,
    {
        // Not `Ok`: without the main panel there is no viewport and no verdict.
        // Passing here would be indistinguishable from a verified pass at the
        // call site, which is exactly how a ref that quietly loses its panel
        // turns a green rung into a no-op.
        let Some(main_panel_id) = ref_.main_panel_block_id() else {
            return InvariantResult::Skipped(
                "the reference knows no main panel, so it models no viewport for the feed to be \
                 judged against"
                    .into(),
            );
        };
        let Some(panel) = root
            .walk()
            .find(|n| n.entity_id.as_deref() == Some(main_panel_id.as_str()))
        else {
            return InvariantResult::Skipped(format!(
                "main-panel node (entity_id '{}') not in this snapshot tick",
                main_panel_id.as_str(),
            ));
        };

        // The panel's own box is the feed's viewport.
        let Some(viewport) = elements.iter().find(|e| {
            e.entity_id.as_ref().map(EntityUri::as_str) == Some(main_panel_id.as_str())
                && e.height > 1.0
        }) else {
            return InvariantResult::Skipped(format!(
                "main panel '{}' registered no box in this frame, so there is no viewport to \
                 judge against",
                main_panel_id.as_str(),
            ));
        };
        let (top, bottom) = (viewport.y, viewport.y + viewport.height);

        // POSITION and CLIP, read separately (see the module docs). A day is on
        // screen only where both agree, off screen only where both agree.
        let by_position: BTreeSet<&str> = elements
            .iter()
            .filter(|e| e.y >= top && e.y < bottom)
            .filter_map(|e| e.entity_id.as_ref())
            .map(EntityUri::as_str)
            .collect();
        let by_clip: BTreeSet<&str> = elements
            .iter()
            .filter(|e| e.height > 0.0)
            .filter_map(|e| e.entity_id.as_ref())
            .map(EntityUri::as_str)
            .collect();

        let onscreen: Vec<&EntityUri> = feed_days
            .iter()
            .filter(|d| by_position.contains(d.as_str()) && by_clip.contains(d.as_str()))
            .collect();
        let offscreen: Vec<&EntityUri> = feed_days
            .iter()
            .filter(|d| !by_position.contains(d.as_str()) && !by_clip.contains(d.as_str()))
            .collect();
        let undecided: Vec<&str> = feed_days
            .iter()
            .filter(|d| by_position.contains(d.as_str()) != by_clip.contains(d.as_str()))
            .map(EntityUri::as_str)
            .collect();
        let disclosure = format!(
            "panel box {top:.0}..{bottom:.0}; {on} day(s) on screen, {off} off, {u} undecided \
             (position and clip disagree){detail} of {total} in the feed",
            on = onscreen.len(),
            off = offscreen.len(),
            u = undecided.len(),
            detail = if undecided.is_empty() {
                String::new()
            } else {
                format!(" {undecided:?}")
            },
            total = feed_days.len(),
        );

        // A day the two signals disagree about is not a day this oracle may
        // judge around. Convicting only the rows they agree on would make the
        // off-screen arm STRICTLY WEAKER than a single signal — a row below the
        // fold that the renderer did not clip (`y=1096, h=26`) is off screen by
        // position, on screen by clip, and would walk free between the two arms.
        // A disagreement means the viewport model no longer matches the
        // frontend, so the whole frame is undecidable, not partially decidable.
        if !undecided.is_empty() {
            return InvariantResult::Skipped(format!(
                "POSITION and CLIP disagree about {n} feed day page(s), so this oracle's model of \
                 the viewport no longer matches the frontend's and no expansion verdict drawn \
                 from it would mean anything — {disclosure}",
                n = undecided.len(),
            ));
        }

        if onscreen.is_empty() && offscreen.is_empty() {
            return InvariantResult::Skipped(format!(
                "no feed day page could be placed on or off screen — {disclosure}",
            ));
        }

        match Self::check_viewport_laws(ref_, feed_days, &onscreen, bottom - top, &disclosure) {
            InvariantResult::Ok => {}
            other => return other,
        }

        let toggles = panel.collect_by_kind("expand_toggle");
        let expanded = |day: &EntityUri| -> bool {
            toggles
                .iter()
                .find(|t| t.props.get("target_id").map(String::as_str) == Some(day.as_str()))
                .and_then(|t| t.props.get("expanded"))
                .map(String::as_str)
                == Some("true")
        };

        let leaked: Vec<&str> = offscreen
            .iter()
            .filter(|d| expanded(d))
            .map(|d| d.as_str())
            .collect();
        if !leaked.is_empty() {
            return InvariantResult::Fail(format!(
                "[inv-journal-feed-viewport-lazy] {n} feed day page(s) are EXPANDED while OFF \
                 SCREEN: {leaked:?}. Expected collapsed (off-screen); got expanded. Each one \
                 materialises a live_query shell, a watched matview and a CDC subscription for \
                 content nobody can see, so feed cost scales with history instead of with the \
                 window. {disclosure}.",
                n = leaked.len(),
            ));
        }

        let dark: Vec<&str> = onscreen
            .iter()
            .filter(|d| !expanded(d))
            .map(|d| d.as_str())
            .collect();
        if !dark.is_empty() {
            return InvariantResult::Fail(format!(
                "[inv-journal-feed-viewport-lazy] {n} feed day page(s) are ON SCREEN but \
                 COLLAPSED: {dark:?}. Expected expanded (on-screen); got collapsed. A day the \
                 user is looking at must show its content. {disclosure}.",
                n = dark.len(),
            ));
        }

        InvariantResult::Ok
    }

    /// Hold the SUT-reported window to properties a wrong window fails, so the
    /// oracle cannot inherit a viewport defect from the input it reads.
    fn check_viewport_laws<R>(
        ref_: &R,
        feed_days: &[EntityUri],
        onscreen: &[&EntityUri],
        panel_height: f32,
        disclosure: &str,
    ) -> InvariantResult
    where
        R: RefJournalFeed + RefFocus,
    {
        let capacity = onscreen_capacity(panel_height);
        if onscreen.len() > capacity {
            return InvariantResult::Fail(format!(
                "[inv-journal-feed-viewport-lazy] viewport law (bounded) FAILS: {n} feed day \
                 pages report as on screen, above the {capacity} a {panel_height:.0}px panel can \
                 show at {MIN_FEED_ROW_HEIGHT:.0}px a row. The reported window is not a window, \
                 so no expansion verdict drawn from it would mean anything. {disclosure}.",
                n = onscreen.len(),
            ));
        }

        // A SUT that reports its whole history as on screen is the failure this
        // law exists to catch, and capacity is the only one of the three that
        // can catch it. Where the feed is small enough to fit, the law cannot —
        // and then neither can this invariant, because every day being on
        // screen also empties the off-screen arm. Saying so is the difference
        // between an unexercised law and a passed one.
        if onscreen.len() == feed_days.len() && capacity >= feed_days.len() {
            return InvariantResult::Skipped(format!(
                "the whole {total}-day feed reports as on screen and a {panel_height:.0}px panel \
                 could hold up to {capacity}, so the bounded law cannot refute the claim and no \
                 day is left off screen to judge. Give the rung more days than {capacity} to make \
                 this frame decidable. {disclosure}.",
                total = feed_days.len(),
            ));
        }

        if let Some(focused) = ref_.current_focus(CapRegion::Main)
            && feed_days.contains(&focused)
            && !onscreen.iter().any(|d| **d == focused)
        {
            return InvariantResult::Fail(format!(
                "[inv-journal-feed-viewport-lazy] viewport law (contains focus) FAILS: feed day \
                 {focused} holds Main focus but is not in the reported window.",
            ));
        }

        // Feed order is the render order, so the painted rows are one unbroken
        // run of it. A hole means rows were dropped from the middle of the
        // scroll rather than trimmed at its edges.
        let positions: Vec<usize> = feed_days
            .iter()
            .enumerate()
            .filter(|(_, d)| onscreen.iter().any(|o| *o == *d))
            .map(|(i, _)| i)
            .collect();
        if let (Some(first), Some(last)) = (positions.first(), positions.last())
            && last - first + 1 != positions.len()
        {
            let missing: Vec<&str> = feed_days[*first..=*last]
                .iter()
                .filter(|d| !onscreen.iter().any(|o| *o == *d))
                .map(EntityUri::as_str)
                .collect();
            return InvariantResult::Fail(format!(
                "[inv-journal-feed-viewport-lazy] viewport law (contiguous) FAILS: the reported \
                 window spans feed positions {first}..={last} but omits {missing:?} inside that \
                 span — a gap in the middle of the scroll.",
            ));
        }

        InvariantResult::Ok
    }
}
