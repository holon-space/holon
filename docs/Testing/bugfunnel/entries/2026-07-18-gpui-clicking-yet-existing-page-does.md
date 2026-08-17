---
id: 2026-07-18-gpui-clicking-yet-existing-page-does
date: 2026-07-18
gap: COVERAGE
secondary: PERCEPTION
status: FIXED
summary: >-
  GPUI: clicking a `[[link]]` to a not-yet-existing page does NOTHING (user
  report). The dangling-link render arm (`EntityRef::Name`,
  `frontends/gpui/src/render/builders/rendered_text.rs`) only
  `tracing::warn!("dangling link click: …")` and stopped — a leftover "C3
  seam" stub — while the resolved-link arm (`EntityRef::Internal`) correctly
  dispatches `navigation.focus`. The core op `create_page_from_link`
  (deterministic-ish page-chain create + dangling-link healing) EXISTED and
  was unit-tested (`crates/holon/tests/create_page_from_link.rs`), but nothing
  in the UI ever invoked it. Violates the 2026-07-10 links ruling (lazy
  page-create on click of dangling links)
source_line: 809
---

## Bug

GPUI: clicking a `[[link]]` to a not-yet-existing page does NOTHING (user
report). The dangling-link render arm (`EntityRef::Name`,
`frontends/gpui/src/render/builders/rendered_text.rs`) only
`tracing::warn!("dangling link click: …")` and stopped — a leftover "C3
seam" stub — while the resolved-link arm (`EntityRef::Internal`) correctly
dispatches `navigation.focus`. The core op `create_page_from_link`
(deterministic-ish page-chain create + dangling-link healing) EXISTED and
was unit-tested (`crates/holon/tests/create_page_from_link.rs`), but nothing
in the UI ever invoked it. Violates the 2026-07-10 links ruling (lazy
page-create on click of dangling links)

## Missing piece

No PBT/test drives a CLICK on a dangling-link mark through the GPUI
`rendered_text` builder; the op was tested in isolation but its UI trigger
(the mouse-down closure) was never exercised, so the stubbed no-op arm
shipped. Secondary PERCEPTION: the failure was a silent no-op (warn
invisible to the user)

## Remedy

FIXED 2026-07-18. New `BuilderServices::follow_dangling_link(target,
region)` seam (`crates/holon-frontend/src/reactive.rs`): default fires only
the create; `ReactiveEngine` override spawns a task that runs
`create_page_from_link`, reads the fresh leaf-page id from the op RESPONSE
(id is a random UUID — the scouted "deterministic id via link_parser"
premise was WRONG, so pre-computing a nav target is impossible; must chain
on the response), then mirrors + persists `navigation.focus` to the new leaf
so the click feels identical to a resolved link. Second click re-resolves
the healed junction → arm becomes `Internal`. Dangling arm in
`rendered_text.rs` now calls `follow_dangling_link(name, "main")`.
Last-writer race GUARDED (found in verifier review): focus is captured at
CLICK time and the spawned task only mirrors+persists `navigation.focus` if
focus is unchanged when the create completes; if the user navigated
elsewhere during the async create window, the stale task skips the focus
move with a disclosed `tracing::info!("dangling-link navigation superseded
by newer navigation")` (the page CREATE still stands — the healed link makes
the next click resolve). Unit-tested the new decision logic at the seam:
`dangling_link_nav_target` (response→(leaf,reset_scroll), fail-loud on
non-String response) + `dangling_nav_superseded` (stale captured focus →
skip; fresh → apply) — 5 tests green in `reactive::tests`. GPUI closure
itself compile-green + manual-verification pending on a live desktop
instance. NOTE: `frontends/dioxus-web` has NO link-aware rendering at all
(no `InlineMark`/`EntityRef` handling) — same bug latent there, deferred.
Gates: `cargo check -p holon-frontend` + `-p holon-gpui` clean; `nextest -p
holon-frontend dangling_link` 3/3.
