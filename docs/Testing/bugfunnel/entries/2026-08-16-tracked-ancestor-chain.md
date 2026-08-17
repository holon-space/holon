---
id: 2026-08-16-tracked-ancestor-chain
date: 2026-08-16
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  `frontends/dioxus-web/src/render/builders/live_block.rs` tracked no ancestor
  chain
source_line: 695
---

## Bug

(D20 shared-moves lane; same CODE AUDIT, finding 6 — the mirror, back on the
web) **`frontends/dioxus-web/src/render/builders/live_block.rs` tracked no
ancestor chain**, so an `A -> B -> A` embed had nothing to stop it. GPUI
refuses to construct or even cache-look-up a child already on
`ctx.live_block_ancestors`. The web's failure mode is strictly worse: each
level is its own `engineWatchView` RPC plus a live worker-side watch, so the
chain is unbounded round trips and server-side subscriptions, not merely
unbounded local widgets.

## Root cause

D20 shared-moves lane, same CODE AUDIT, finding 6 — the mirror of the row
above, back on the web: `frontends/dioxus-web/.../live_block.rs` tracked no
ancestor chain, so an `A -> B -> A` embed had nothing to stop it. GPUI
refuses to construct (or even cache-look-up) a child already on
`ctx.live_block_ancestors`. The web's failure mode is strictly worse than
GPUI's would be: each level of the web's chain is its own `engineWatchView`
RPC plus a live worker-side watch, so an unbounded chain is an unbounded
number of round trips and server-side subscriptions, not merely unbounded
local widgets. COVERAGE primary: nothing on EITHER arm generates a block
embedded inside itself — GPUI's guard is equally untested, which is why the
asymmetry survived. ENVIRONMENT secondary: the per-node RPC subscription
mechanism has no headless or GPUI counterpart, so even a generated cycle
would exhibit the unbounded-subscription failure only in a browser. FIXED by
making the rule shared rather than mirrored: `LiveBlockAncestors` moved from
`frontends/gpui/src/entity_view_registry.rs` into
`holon_frontend::live_block_ancestors` (GPUI re-exports it, so its call
sites are unchanged) and gained a named `would_cycle`; the web threads the
chain down a Dioxus context provider and refuses with the same `warn!`.
Covered at the shared layer by `would_cycle_detects_reentry_at_any_depth`
and `pushed_is_an_immutable_copy`. RESIDUAL GAP, disclosed: no generator
nests a block inside itself, so the guard is proven as a pure function and
NOT end-to-end on either arm. The rung: a keystone generator arm that
renders `live_block(X)` inside X's own subtree.)

## Missing piece

Nothing on EITHER arm generates a block embedded inside itself — GPUI's
guard is equally untested, which is why the asymmetry survived. ENVIRONMENT
secondary: the per-node RPC subscription mechanism has no headless or GPUI
counterpart, so even a generated cycle shows the unbounded-subscription
failure only in a browser.

## Remedy

FIXED by making the rule shared rather than mirrored: `LiveBlockAncestors`
moved from `frontends/gpui/src/entity_view_registry.rs` to
`holon_frontend::live_block_ancestors` (GPUI re-exports it, call sites
unchanged) and gained a named `would_cycle`; the web threads the chain down
a Dioxus context provider and refuses with the same `warn!`. Covered by
`would_cycle_detects_reentry_at_any_depth` + `pushed_is_an_immutable_copy`.
RESIDUAL, disclosed: no generator nests a block inside itself, so the guard
is proven as a pure function and NOT end-to-end on either arm. Rung: a
keystone arm rendering `live_block(X)` inside X's own subtree.
