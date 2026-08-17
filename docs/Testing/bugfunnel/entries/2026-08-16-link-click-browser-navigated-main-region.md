---
id: 2026-08-16-link-click-browser-navigated-main-region
date: 2026-08-16
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  A link click in the BROWSER navigated the main region on any
  `EntityRef::entity_uri()` being `Some`
source_line: 692
---

## Bug

(D20 shared-moves lane; found by CODE AUDIT of the GPUI vs dioxus-web
builder sets — `lane-logs/frontend-dup-audit.md` finding 13a — not by any
running test) **A link click in the BROWSER navigated the main region on any
`EntityRef::entity_uri()` being `Some`**, with no registry check and no
scheme check (`frontends/dioxus-web/src/render/builders/rendered_text.rs`
`follow_internal_link`). GPUI's `link_click_action` had carried both gates
since the 2026-08-08 row and its comment names this class exactly: a
registered-but-viewless scheme (`tag:`, `person:`) or an unregistered one
drives the panel to a focus root that `focus_roots JOIN block` cannot show —
a blank main region, undisclosed.

## Root cause

D20 shared-moves lane, found by CODE AUDIT of the two builder sets
(lane-logs/frontend-dup-audit.md finding 13a), not by any running test:
clicking an inline link in the BROWSER navigated the main region on ANY
`EntityRef::entity_uri()` being `Some` — no registry check, no scheme check
(`frontends/dioxus-web/.../rendered_text.rs` `follow_internal_link`). GPUI's
`link_click_action` had carried BOTH gates since the BugFunnel 2026-08-08
row, and its own comment names this exact class as the reason: a
registered-but-viewless scheme (`tag:`, `person:`) or an outright
unregistered one drives the panel to a focus root that `focus_roots JOIN
block` cannot show, i.e. a blank main region with no disclosure. ENVIRONMENT
primary and structural rather than incidental: the failing function exists
ONLY in the dioxus-web builder set, which sits outside the cargo workspace
(`Cargo.toml` members) and outside CI, so no keystone case, no GPUI window
test and no `cargo check --workspace` ever compiled it, let alone drove it.
COVERAGE secondary: no transition in the catalog clicks a link whose target
is scheme-shaped but not `block:` — the link generators mint block targets
and dangling names. FIXED by deleting the web's private routing and adopting
the shared `holon_frontend::link_segments::link_click_action` + `nav_focus`;
GPUI's six routing unit tests moved to
`crates/holon-frontend/src/link_segments.rs` and now cover BOTH arms
(`link_click_action_routes_each_kind_to_its_verb`,
`link_click_action_opens_mailto_rather_than_navigating`). The web classifier
is `LinkTargetClassifier::default()` (built-ins only, since the profile
registry lives in the worker) — behaviourally identical here because
navigation additionally requires the `block` scheme, which is built in.
RESIDUAL GAP, disclosed not faked: no automated arm drives a real browser
click on a link, so the fix is pinned by the shared unit tests plus the type
system, not by an end-to-end case; the web-arm PBT rungs landed at ede2ed57
are the place that gap closes.)

## Missing piece

The failing function exists ONLY in the dioxus-web builder set, which is
outside the cargo workspace members and outside CI, so no keystone case, no
GPUI window test and no `cargo check --workspace` ever compiled it. COVERAGE
secondary: no transition clicks a link whose target is scheme-shaped but not
`block:`.

## Remedy

FIXED — web routing deleted, shared
`holon_frontend::link_segments::link_click_action` + `nav_focus` adopted;
GPUI's six routing unit tests moved to
`crates/holon-frontend/src/link_segments.rs` and now cover both arms. Web
classifier is `LinkTargetClassifier::default()` (built-ins only; identical
here because navigation also requires the built-in `block` scheme).
RESIDUAL, disclosed not faked: no arm drives a real browser link click, so
the fix is pinned by shared unit tests and the type system, not end-to-end.
