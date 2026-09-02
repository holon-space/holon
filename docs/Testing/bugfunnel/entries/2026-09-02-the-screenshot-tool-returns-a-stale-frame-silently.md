---
id: 2026-09-02-the-screenshot-tool-returns-a-stale-frame-silently
date: 2026-09-02
gap: PERCEPTION
secondary: ENVIRONMENT
status: NOTED
summary: >-
  The MCP `screenshot` tool returned a byte-identical frame across two
  navigations, so the dogfood channel's only pixel oracle reports stale state
  as current with nothing to say so.
---

## Bug

Found while dogfooding the kitchen feature (lane `kitchen-dogfood`). After
navigating the main region to a recipe — `execute_operation navigation/focus`,
confirmed by `describe_ui` reporting
`live_block(block:8a802b12-…)` in `block:default-main-panel` — the screenshot
still showed the Journals view, window title included. Three screenshots taken
across the boot state and two post-navigation states were byte-identical:

```
9d6b7ea56b693b7d4c6ecd5ca877b74c  01-boot.png
9d6b7ea56b693b7d4c6ecd5ca877b74c  02-recipe.png
9d6b7ea56b693b7d4c6ecd5ca877b74c  03-recipe-retry.png
```

The geometry block `describe_ui` returns agreed it was stale: the same 853
measured elements each time, with the recipe's own text at `y=2510.0 h=0.0
NO-VISIBLE-AREA`.

Screenshots in the lane scratchpad
(`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad/kd-shots/`).

## Root cause

Not established, and the entry is filed as NOTED for that reason. Two
candidates, and this run cannot separate them:

1. The app was launched via `nohup` and its window was never fronted, because
   the `osascript` step the dogfood skill's launch protocol prescribes was
   refused: `Not authorized to send Apple events to System Events (-1743)`.
   A background GPUI window may simply not produce new frames.
2. The screenshot path serves the last presented frame rather than forcing a
   present.

Either way the reportable defect is the same and is independent of which holds:
the tool answered with a stale image and said nothing about it. A pixel oracle
that can be silently wrong is worse than one that refuses, because every visual
finding drawn through it — including the absence of a finding — is unfalsifiable.

## Missing piece

`screenshot` carries no frame identity. Nothing in its response says when the
frame was produced, what the window's visibility state was, or whether a new
frame was presented for this call, so a caller cannot tell a stale answer from
a fresh one. The dogfood skill's launch protocol also has no fallback for the
case where the Accessibility permission its un-minimise step needs is not
granted, which is how this machine is configured today.

## Remedy

NOTED, not fixed. Two cheap improvements, in order:

1. `screenshot` returns a frame counter or timestamp plus the window's
   visible/occluded state, so a caller can detect staleness. This is worth
   doing whichever root cause holds.
2. The dogfood skill records the Accessibility-denial failure mode and what to
   do about it, since its current step is the only path it offers and that path
   is closed here.

Until then, treat pixel evidence from a `nohup`-launched instance on this
machine as unreliable, and prefer `describe_ui`, which read the live widget
tree correctly throughout this session.
