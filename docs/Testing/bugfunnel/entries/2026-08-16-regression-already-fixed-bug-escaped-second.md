---
id: 2026-08-16-regression-already-fixed-bug-escaped-second
date: 2026-08-16
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  A REGRESSION of an already-fixed bug, re-escaped on a second frontend: the
  web gated styled rendering on LINK marks only
source_line: 693
---

## Bug

(D20 shared-moves lane; same CODE AUDIT, finding 13b) **A REGRESSION of an
already-fixed bug, re-escaped on a second frontend: the web gated styled
rendering on LINK marks only**, so a block whose marks were
Bold/Italic/Underline fell to the plain-text branch and its formatting
silently vanished in the browser. That is dogfood 2026-07-22 bug 1, which
GPUI fixed by gating on any mark kind and recorded in a comment at the
predicate — a comment the web copy predates and was never linked to.

## Root cause

D20 shared-moves lane, same CODE AUDIT, finding 13b — a REGRESSION of an
already-fixed bug, re-escaped on a second frontend:
`frontends/dioxus-web/.../rendered_text.rs` gated styled rendering on LINK
marks only, so a block whose marks were Bold/Italic/Underline fell through
to the plain-text branch and its formatting silently vanished in the
browser. That is precisely dogfood 2026-07-22 bug 1, which GPUI fixed by
gating on ANY mark kind and recorded in a comment at the predicate — a
comment the web copy's author never saw, because the predicate was copied
before the fix and the two copies were never linked. ENVIRONMENT primary for
the same structural reason as the row above (out-of-workspace, out-of-CI
builder set). ORACLE secondary and the sharper half:
`inv-paint-text-styling` catches exactly this defect, but it reads GPUI's
PAINTED styled runs off the geometry tracker, and the web arm exposes no
equivalent observable — so even a keystone case that produced a bold-only
block would have gone green while the browser rendered it plain. FIXED
beyond the audit's line item: sharing the one-line predicate alone would NOT
have fixed anything, because the web had no styled painting at all to route
into. `wants_styled_render` moved to `holon_frontend::link_segments`, and a
new shared `styled_link_segments` merges the link partition with
`holon_api::style_fingerprint`'s style partition into one ordered contiguous
run list, which the web now paints as per-run CSS. Red-first proof at the
shared layer: `styled_link_segments_carries_non_link_marks` asserts a
bold-only block yields a run with `flags.bold`, which the old link-only gate
could not produce. RESIDUAL GAP, disclosed: the web arm still has no
painted-output observable, so the ORACLE half is NOT closed — the shared
partition is tested, the browser's rendering of it is not.)

## Missing piece

Same out-of-workspace / out-of-CI builder set as the row above. ORACLE
secondary and sharper: `inv-paint-text-styling` catches this defect but
reads GPUI's PAINTED styled runs off the geometry tracker, and the web arm
exposes no equivalent observable — a keystone case producing a bold-only
block would have gone green while the browser rendered it plain.

## Remedy

FIXED, and beyond the audit's line item: sharing the predicate alone fixes
nothing, because the web had no styled painting to route into.
`wants_styled_render` moved to `link_segments`, and a new shared
`styled_link_segments` merges the link partition with
`holon_api::style_fingerprint`'s style partition into one ordered contiguous
run list the web paints as per-run CSS. Proof:
`styled_link_segments_carries_non_link_marks` asserts a bold-only block
yields a `flags.bold` run, which the old link-only gate could not produce.
RESIDUAL: the web still has no painted-output observable, so the ORACLE half
is NOT closed.
