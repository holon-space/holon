---
id: 2026-08-31-three-row-chrome-eats-a-phone-viewport
date: 2026-08-31
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  The top chrome stacked three separate rows — title bar, tab strip, breadcrumb
  bar — spending 96px above the content before a phone had drawn a single row.
---

## Bug
With two tabs open, the window drew three stacked chrome rows: the 38px title
bar, the 30px open-tabs strip, and the 28px breadcrumb bar. On a phone viewport
that is 96px of chrome above the first line of content, and every extra tab
widened the strip rather than the chrome paying for itself. Reported by Martin
(D48.d, 2026-08-30): the tabs should work "like browsers with a button
containing the tab count and on click the tab list opens, with options to create
a new tab".

## Root cause
Not a defect in any one component — each bar was correct on its own and simply
claimed its own row in the page column. `frontends/gpui/src/lib.rs` built
`title_bar` (`h(px(38.0))`), then appended `tab_strip::render_tab_strip`
(`h(px(30.0))`) and `breadcrumb::render_breadcrumb_bar` (`h(px(28.0))`) as
siblings above the content wrapper. Measured by the rung below at both a 390px
and a 1440px viewport: `content_top=96.0` — the first painted content element
began 96px down.

The tab strip also grew with the tab count, so the cost was unbounded in the
one direction a phone can least afford.

## Missing piece
No test asserted a BUDGET on the chrome. Every windowed test measured what a
component drew, never what the composition COST the content beneath it, so
three individually-correct bars could stack without anything going red. The
observable that was missing is "how far down the content starts" — cheap to
measure from `BoundsRegistry`, and absent until now.

## Remedy
FIXED. The chrome is one row: the breadcrumb moved into the title row as the
page title (`breadcrumb::render_breadcrumb_inline`, `flex_1` + `min_w_0` so the
toolbar is never pushed off a narrow screen), and the strip was replaced by a
`chrome-tab-count` button that opens a tab list popup with per-tab switch and
close plus a new-tab action (`tab_strip::render_tab_count_button` /
`render_tab_list`, `navigation.new_tab`). `render_tab_strip` and
`render_breadcrumb_bar` were deleted rather than left dormant.

Pinned by `frontends/gpui/tests/chrome_one_row_windowed.rs`, which measures
`content_top` at a narrow AND a wide viewport with two tabs open, so a future
bar that claims a row of its own goes red on the budget rather than on its own
appearance. Red before the fix: `content_top=96.0` at both widths with
`tab_count_button=None`; green after: `content_top=38.0`, button text `▤ 2`.
