---
id: 2026-09-08-degraded-banner-covers-the-titlebar-and-first-row
date: 2026-09-08
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  The deferred-reimport banner is an absolute overlay at the top of the window,
  so it hides the titlebar, the toolbar buttons and the top of the first content
  row — permanently, because it is undismissable and lifts only on a successful
  retry.
---

## Bug

Found by the `dogfood-explorer` gate for D94.a, port 8720, fixture
`/tmp/dogfood-w10-degraded`. Screenshot `lane-logs/dogfood-w10/03-banner-s.png`
(degraded) against `lane-logs/dogfood-w10/06-after-retry-s.png` (same window
after the retry succeeded and the banner lifted).

While the banner is up:

- the window title ("Journals") and the traffic-light row sit behind it;
- the whole top-right toolbar strip is behind it;
- the page's first content row is behind its lower edge — the day heading
  `2026-09-08` shows only as ghosting through the banner's background.

The after-retry frame shows all of it back, which is what identifies the banner
as the occluder rather than a paint fault.

This matters more than a normal overlay because D94.a made the banner
undismissable on purpose: it stands until a retry succeeds, and (see entry
`2026-09-08-a-pre-pair-top-level-page-can-never-be-reimported`) a retry
currently cannot succeed for a pre-pair page. The occlusion is then permanent,
and it lands on the first row — where a caret placed in the first block of a
page would be.

## Root cause

`render_deferred_reimport_banner` (`frontends/gpui/src/share_ui.rs:1269`) draws
`.absolute().top(px(16.0)).left(px(16.0)).right(px(16.0))` into the overlay
stack. Overlays paint over the whole window, including the titlebar chrome, and
nothing insets the layout beneath by the banner's height, so the banner's
~54 px band overlaps content whose first row starts at y=50 in main-panel
coordinates.

## Missing piece

The D94.a windowed surface is the `BoundsRegistry` id `deferred-reimport-banner`
— a test can ask whether the banner is present, not whether it sits on top of
something. No assertion anywhere compares an overlay's bounds against the
bounds of the content or the chrome beneath it, so nothing could have gone red.

## Remedy

Fixed in chain commit `f2fa918a` ("fix(pairing): a top-level page written before
a pair is re-imported, and the degraded bar no longer covers the chrome"). The
bar left the overlay stack: `share_ui::render_deferred_reimport_bar` takes a
reserved band in the page's flow directly under the title row, still
undismissable and still lifted only by the bus.

Covered by the bar-geometry assertions in
`the_window_paints_the_deferred_reimport_banner_and_its_retry_completes_the_pair`
(`frontends/gpui/tests/pairing_deferred_reimport_windowed.rs:222`), which
compare the bar's painted bounds against the tracked `TITLE_ROW_ID` row
(line 329) and against every content row (`content_rows`), failing by name on
any vertical overlap.
