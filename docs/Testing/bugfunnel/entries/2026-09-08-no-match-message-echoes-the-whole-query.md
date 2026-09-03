---
id: 2026-09-08-no-match-message-echoes-the-whole-query
date: 2026-09-08
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  Quick-open's empty state echoes the query verbatim and unbounded, so a long
  query turns "No matches for X" into a wall of text that fills the overlay to
  the bottom of the window and is itself clipped without an ellipsis.
---

## Bug

Found by the `dogfood-explorer` gate re-run of the cmd-K search fix (lane
`search-fix`, port 8710, seeded throwaway vault).

Typing an 8 000-character query (`"Ab"` repeated 4 000 times) into quick-open
produces the correct verdict — there is no match — but renders it as:

    No matches for
    "AbAbAbAbAb… (the entire query, wrapped over ~20 lines)

The overlay grows from its normal ~200 px to fill nearly the whole window
height, hides the page behind it, and *still* cannot show the whole query: the
echo is cut off flush at the overlay's bottom edge with no ellipsis and no
scroll affordance, so the message is simultaneously enormous and truncated.

Evidence: `lane-logs/dogfood-r2/19-overlong.png` (full frame),
`lane-logs/dogfood-r2/19-crop.png` (overlay detail).

This is not the over-limit case. 8 000 ASCII characters fold to roughly 32 000
GLOB pattern bytes, comfortably under the 50 000-byte ceiling
(`MAX_GLOB_PATTERN_BYTES`, `crates/holon/src/api/query_engine.rs:426`), so the
search ran normally and legitimately found nothing. The defect is purely in how
the empty state presents the query.

## Root cause

The empty-state string interpolates the raw query with no length cap and no
`text-overflow` treatment, and the overlay's height is driven by its content, so
the message's size is a direct function of what the user typed. Not localised
further — the overlay has no `describe_ui` node (entry
`2026-09-03-titlebar-toolbar-is-invisible-to-describe-ui`), so it was observed
only through screenshots.

## Missing piece

Nothing bounds, or asserts anything about, the rendered size of the quick-open
overlay. The headless keystone PBT has no pixels and no overlay; the windowed
lane never drives quick-open at all; and no generator produces a query long
enough for the difference to show — every search query the suites type is a
handful of characters.

## Remedy

Open. Truncate the echoed query in the empty state to a fixed budget (roughly
one line) with an ellipsis, and cap the overlay's height so no query can grow
it past its normal bounds. Pair the fix with a windowed assertion that the
overlay's rendered height is independent of query length — a single long-query
case is enough to pin it.
