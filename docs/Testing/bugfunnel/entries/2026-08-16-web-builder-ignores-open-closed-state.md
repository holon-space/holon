---
id: 2026-08-16-web-builder-ignores-open-closed-state
date: 2026-08-16
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  The web `drawer` builder ignores open/closed state entirely
source_line: 691
---

## Bug

(D20 shared-moves lane; found by ADVERSARIAL VERIFICATION of this lane's own
Move 1 — `lane-logs/shared-moves-verify.md` §1a — not by any running test)
**The web `drawer` builder ignores open/closed state entirely**: it renders
its child and nothing else, where GPUI reads
`services.drawer_open(&block_id, mode)` and collapses a closed shrink drawer
to its toggle width. The state is not on the snapshot — `ViewKind::Drawer`
carries only `block_id`, `mode`, `width`, `child`, while `drawer_open`
resolves through `widget_state_explicit`, a live `FrontendSession` lookup
with no serialized counterpart — so the browser always paints every drawer
open and reserves its full `Fixed{px: width}`. SURFACED BY Move 1, not
caused by it: the pre-Move heuristic gave any `*right*`-matching panel
`flex: 0 1 auto` (content-sized), so a fresh vault collapsed the right
sidebar by accident — which also meant an open right sidebar was never
really 260px.

## Root cause

D20 shared-moves lane, found by ADVERSARIAL VERIFICATION of this lane's own
Move 1 (lane-logs/shared-moves-verify.md §1a), not by any running test: the
web `drawer` builder ignores the drawer's open/closed state entirely —
`frontends/dioxus-web/src/render/builders/drawer.rs` renders its child and
nothing else, where GPUI reads `services.drawer_open(&block_id, mode)`
(`crates/holon-frontend/src/reactive.rs:225`) and collapses a closed shrink
drawer to its toggle width. The state is NOT on the snapshot:
`ViewKind::Drawer` (`crates/holon-frontend/src/view_model.rs:369-376`)
carries only `block_id`, `mode`, `width`, `child`, while `drawer_open`
resolves through `widget_state_explicit`, a live `FrontendSession`
view-store lookup with no serialized counterpart. So the browser always
paints every drawer open and always reserves its full `Fixed{px: width}`
allocation. SURFACED BY Move 1 rather than caused by it: the pre-Move web
builder classified panels by block-id substring and gave anything matching
`*right*` a `.holon-col-rail { flex: 0 1 auto }`, i.e. CONTENT-SIZED, so a
fresh vault (no `focus_roots` row for `region = 'right_sidebar'`) collapsed
the right sidebar to nothing. That collapse was an accident of the heuristic
and was itself a divergence — it also meant an OPEN right sidebar was never
actually 260px wide on the web. Honouring `layout_hint` removed the accident
and left the real gap visible. ENVIRONMENT primary: the divergent code path
exists only in the dioxus-web builder set, which no gate on this lane
compiles (`just precommit`'s browser leg is `cargo check -p holon-frontend`;
gate-hygiene's `check-dioxus-web-wasm` lands separately), and no arm drives
a browser. COVERAGE secondary: no transition opens or closes a drawer and
asserts the resulting allocation on any arm. NOT FIXED — deliberately
escalated rather than approximated. Approximating it needs information that
does not exist: emptiness is not derivable either, because a panel's drawer
child is a `LiveBlock` whose content arrives asynchronously over
`engineWatchView`, so a snapshot-time emptiness test would collapse a
populated panel that is merely still loading. The rung that closes it is a
snapshot field: add `open: bool` to `ViewKind::Drawer`, populated by the
shadow builder from the same view-store the GPUI builder reads. That fixes
the web AND makes it agree with GPUI, which the old heuristic never did.
MITIGATED meanwhile: panel chrome (surface, padding, scroll container,
borders) moved off the column wrapper onto `[data-role="drawer"]`, so a
zero-width column paints nothing at all, and the column wrapper is now
purely structural — see the two sibling changes in this lane's report.)

## Missing piece

The divergent path exists only in the dioxus-web builder set, which no gate
on this lane compiles (`just precommit`'s browser leg is `cargo check -p
holon-frontend`), and no arm drives a browser. COVERAGE secondary: no
transition opens or closes a drawer and asserts the resulting allocation on
any arm.

## Remedy

FIXED (D26.a) by the escalated rung, not by a proxy — emptiness was never
usable here (a panel's drawer child is a `LiveBlock` whose content arrives
asynchronously over `engineWatchView`, so a snapshot-time emptiness test
would collapse a populated panel that is merely still loading).
`ViewKind::Drawer` now carries a typed `open: bool` (no `Option`, no
defensive default), stamped by the SHARED shadow builder from
`BuilderServices::drawer_open` — the same view-store read GPUI performs live
— so the two arms agree for the first time. The mode-aware meaning is
encoded once: an explicit stored setting wins, else
`DrawerMode::default_open` (Shrink open, Overlay closed). Three supporting
pieces: (1) `set_widget_open` now bumps `viewport_generation`, so a store
flip actually re-interprets and the web receives a fresh snapshot — without
it the stamp would freeze at boot; (2) `DRAWER_TOGGLE_WIDTH` moved to
`holon_frontend` and a CLOSED shrink drawer's `layout_hint` now reserves the
toggle strip instead of the full width, which is the collapsed geometry
GPUI's `columns` already computed for itself — GPUI is unaffected (its
shrink branch overrides `layout_hint`), web and TUI now match it; (3) the
web builder renders `data-drawer-open` and clips a closed shrink drawer to
that strip, a closed overlay drawer to nothing. PROVEN
RED-FOR-THE-RIGHT-REASON then green by a NEW keystone invariant
`inv-drawer-open-matches-ref`
(`crates/holon-integration-tests/src/pbt/invariants/bodies/drawer_open_matches_ref.rs`,
wired via `RefNavHistory` newly capmap-hosted and registered on the ref
`CapMap`): red `lane-logs/drawer-open-red.log` — "drawer
block:default-left-sidebar carries no 'open' prop — the snapshot has no
open/closed state" attributed to layer `viewmodel`; green
`lane-logs/drawer-open-green.log` with the invariant fully engaged
(`inv-drawer-open-matches-ref=5/5`, `3/3`) and the classifier reporting
PASS-WITH-NOTE, 0 novel. Consumers checked: `cargo check -p holon-gpui
--all-targets --features pbt` and `cargo check --manifest-path
frontends/dioxus-web/Cargo.toml --target wasm32-unknown-unknown`, both
green. RESIDUAL, disclosed not faked — the ENVIRONMENT half is NOT closed:
the keystone asserts the SNAPSHOT carries the right open state, which is the
layer the bug lived at, but no arm renders a real browser, so the PIXEL
claim (a closed sidebar actually paints 12px wide in Chrome) remains
unverified by any gate. Second residual: dioxus-web builders have no write
path back to the view store (the web `expand_toggle` is a local signal
only), so the browser still cannot TOGGLE a drawer — it now faithfully
renders whatever the store says, and a drawer closed elsewhere stays closed
until the web gains a store-write path. NAMED COST of that same gap: where
GPUI's closed branch renders the TOGGLE ALONE and clips the panel away, the
web's closed drawer still renders its full child subtree and merely clips it
to the 12px strip — so the drawer's `live_block` children stay mounted with
their `engineWatchView` subscriptions live, and no toggle affordance is
painted in that strip. Both follow from the missing store-write path (a real
toggle needs somewhere to write), and both are cheap to close once it
exists. Neither residual regresses the default web experience: shrink
sidebars default open.
