---
id: 2026-08-31-tab-count-paints-zero-when-the-read-fails
date: 2026-08-31
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  Collapsing the tab strip into a count button dropped the strip's error branch,
  so a failed tab read painted a plausible "▤ 0" instead of saying it failed.
---

## Bug
Found by the adversarial verifier reviewing the one-row chrome
(`2026-08-31-three-row-chrome-eats-a-phone-viewport`), before the work landed.

The deleted `render_tab_strip` had a `"Tabs unavailable: {err}"` branch. Neither
replacement — the title row's count button nor the tab list — read
`TabStripState::error`, though the resolver still wrote it. Any resolution
failure (a no-Turso wiring hitting the fail-loud `region_open_tabs` default, a
schema skew, an unparseable `block_id`) therefore left `tabs` empty and painted
`▤ 0`: a number that reads as "you have no tabs open" when the truth is "I could
not find out". CLAUDE.md priority 4, on the one element that, after the strip
was gone, is all the user has.

## Root cause
`render_tab_count_button` rendered `format!("▤ {}", tabs.len())` unconditionally
and `render_tab_list` early-returned only on `!list_open`; `error` had no reader
anywhere in `frontends/gpui/src/tab_strip.rs`. The module doc still promised the
opposite ("rendered as a visible message, never a silently-empty bar"), which is
how the gap survived review — the prose asserted the behaviour that had been
deleted.

## Missing piece
No rung ever drove a FAILING tab read. Every windowed test exercised the happy
path, so the error branch was unreachable by the suite both before and after the
rewrite, and deleting it changed no test result. Two further oracle holes rode
along: the close control was asserted to exist but never clicked, and the count's
`displayed_text` was read once at boot and never after a mutation.

## Remedy
FIXED. The button paints `▤ !` in the error colour and the list carries the
message (`tab-list-error`); the module doc now describes what the code does.

Pinned by `frontends/gpui/tests/chrome_one_row_windowed.rs`:
`the_count_button_shows_the_failure_not_a_plausible_zero` injects a real failure
at the seam's own parse boundary — an open row whose `block_id` is not a URI —
and asserts the title row says so. Red before the fix (`button after=Some("▤ 2")`
while the list showed the error), green after (`after=Some("▤ !")`,
`state_tabs=Some(0)`).

Closing the count-staleness hole then exposed a REAL second defect the rungs now
also cover: the re-read is requested when an op is DISPATCHED, so it could beat
the write to the database and hand back the pre-op world, leaving the count one
behind after a close or a new tab.

The first attempt at that — comparing the read against a pre-op snapshot and
retrying while they matched — was itself refuted in review, and the reason is
worth keeping: snapshot equality cannot tell "the write has not landed" from
"the write was a no-op". It mis-scored the ordinary gesture (closing the ACTIVE
tab moved the snapshot's cursor before it was taken, so a genuinely stale read
passed as fresh) and it burned 22 reads on a keypress that changed nothing.

The mechanism now is the write's own completion: a tab op dispatched from the
chrome goes through `dispatch_tab_op`, which awaits
`dispatch_intent_awaitable` and asks for exactly ONE re-read when the write
reports back, while reads taken with a write outstanding are dropped rather than
shown. No snapshot comparison, no retry budget, and no give-up branch, because
the signal is the write finishing rather than a guess about whether it has.

Three disclosure holes in that lifecycle were closed after review, all of them
the same failure to say what the count means:

- A REFUSED write is a different fact from a failed read, and has a different
  lifetime — the read that follows a refusal SUCCEEDS, and would erase the
  refusal a frame after it appeared. `TabError::{Read, Write}` separates them:
  a good read answers a read error only, and the refusal stands until a write
  lands. It is also `tracing::error!`-ed.
- A write that never reports back would freeze the count at its pre-op value
  silently. Past `SLOW_WRITE_DISCLOSE_AFTER` (2s) the button shows
  `TAB_COUNT_WAITING_LABEL` (`▤ …`) and a WARN names the op — the number is
  disclosed as predating the write rather than being replaced by a guess or a
  faked timeout.
- `BuilderServices::dispatch_intent_awaitable` is now a REQUIRED method. Its old
  default did dispatch, but reported `Proven` immediately instead of awaiting the
  write — so an impl that forgot to override it made the whole mechanism inert:
  the re-read fired before the write landed and the pre-op world simply stayed on
  screen.
