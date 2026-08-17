---
id: 2026-08-08-trailing-nested-page-chevron-toggles-records
date: 2026-08-08
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The trailing nested-page chevron toggles, records state, materialises its
  content — and the opened page paints ZERO children. It opens onto nothing.
source_line: 1185
---

## Bug

(dogfood-explorer gate pass on the #16 fix, real GPUI app over its embedded
MCP) **The trailing nested-page chevron toggles, records state, materialises
its content — and the opened page paints ZERO children. It opens onto
nothing.** Reproduced twice with a 4s settle, zero ERROR lines;
`describe_ui` reports the node OPEN while the window is EMPTY, and the
window is the truth. The #16 windowed PBT already clicked the REAL chevron
through the production handler and asserted the gate `Mutable`, the
`materialize_if_gated()` flag and the painted glyph — all GREEN throughout.
Flags about an affordance are not that affordance's result. NO SECONDARY GAP
— the harness-masking story this row first carried was REFUTED by a
fresh-context verifier's single-variable experiment: swapping ONLY the
collection shape to the shared harness's pre-populated
`new_static_with_layout` still fails on the broken tree with the identical
signature and passes on the fixed tree
(`lane-logs/verify-16c-maskprobe{,-control}.log`). Driver-vs-static never
masked anything; the miss is purely that nothing asserted paint.

## Root cause

dogfood-explorer gate pass on the #16 fix, driving the real GPUI app over
its embedded MCP — **the trailing nested-page chevron now toggles, records
state and materialises its content, and the opened page paints ZERO
children: it opens onto nothing**. Reproduced twice with a 4s settle, zero
ERROR lines. ORACLE because the interaction was already covered and already
driven through the production handler — the #16 windowed PBT clicked the
real chevron and asserted the gate `Mutable`, the `materialize_if_gated()`
flag and the painted glyph, and every one of those was GREEN while the
window was empty. Flags about the affordance are not the affordance's
result; nothing asserted painted descendant rows. NO SECONDARY GAP: a
fresh-context verifier REFUTED the harness-masking story this row first
carried by single-variable experiment — swapping ONLY the collection shape
to the shared harness's pre-populated `new_static_with_layout` still FAILS
on the broken tree with the identical `painted = ["\u25BC", "A Nested
Page"]` signature and passes on the fixed tree
(`lane-logs/verify-16c-maskprobe{,-control}.log`). Driver-vs-static is not
the masking factor; the miss is PURE ORACLE — no test ever put a
`live_query` under an `expand_toggle` and asserted painted rows. ROOT CAUSE
(one line, precedent in-tree): `expand_toggle`'s container is content-sized
(`div().w_full().flex().flex_col()`), and it rendered its materialised
content through the generic `super::render`, giving a `live_query` the
`Panel` shell shape — `size_full` + `height: relative(1.0)` — which has no
definite height to resolve against and collapses to 0 px, taking every row
with it. `column::render` already routes a `live_query` child to
`live_query::render_content_height` for exactly this reason, naming the same
failure in its own comment. FIXED same lane: `expand_toggle` adopts that
routing. The new windowed test carries its OWN `BuilderServices` (the shared
`support/mod.rs` is unchanged) and builds the REAL `embedded_page` shape — a
`live_query`, not a static `text` — over a driver-backed
`ReactiveRowProvider`; the driver-backed shape is the more prod-like choice
but, per the verifier's probe, was NOT what made the red possible. A probe
recorded `view_items=2` — the rows were present in the collection — beside
`painted = ["\u25BC", "A Nested Page"]`, which localizes the loss to the
render, not the data. Evidence:
`docs/Testing/fixture-logs-2026-08-08/task16-f1-opens-to-nothing.txt`)

## Missing piece

the windowed PBT asserted gate/materialisation FLAGS about the affordance,
never the affordance's painted result: no test ever put a `live_query` under
an `expand_toggle` and asserted painted rows

## Remedy

FIXED 2026-08-08 — ROOT CAUSE: `expand_toggle`'s container is content-sized
(`flex_col`, no height) and rendered its materialised content through the
generic `super::render`, so a `live_query` got the `Panel` shell shape
(`size_full` + `height: relative(1.0)`), which has no definite height to
resolve against and collapses to 0 px — taking every row with it.
`column::render` already routes a `live_query` child to
`live_query::render_content_height` and names this exact failure in its
comment; `expand_toggle` now adopts the same routing. RED FIRST: the new
windowed test `an_opened_nested_page_paints_its_children` carries its OWN
`BuilderServices` (the shared `support/mod.rs` is untouched) and builds the
REAL `embedded_page` shape — a `live_query`, not the static `text` the
earlier tests used — over a driver-backed `ReactiveRowProvider`. Putting a
`live_query` under the toggle at all is what made the red possible; the
driver-backed shape is merely the more prod-like choice. Red verbatim
`painted = ["\u25BC", "A Nested Page"]` — chevron open, header painted, both
children absent — with the gate and materialisation preconditions asserted
GREEN first so the failure cannot be read as "the toggle did not flip". A
probe recorded `view_items=2` at the same moment, which localizes the loss
to the render rather than the data, and 1s of real settling never changed
it. Mutation-proven: reverting only the routing reds exactly this test, the
other four stay green. **ROUND 5 — REOPENED: the routing fix is NOT the
whole bug.** A targeted LIVE re-check at the landed fix (main 9e1b1431,
worktree agent-af146ce1320de67c5, `lane-logs/dogfood27_check1.log`)
reproduces the symptom: the trailing chevron flips the gate
(`gate_open=False` -> `True`, confirmed by re-click), `from descendants`
returns both rows, an ordinary indented tree child paints immediately — and
the painted set is byte-identical before and after the flip apart from
element serials, with `click_entity` on both child ids reporting "element
bounds never committed". CLASSIFICATION HISTORY, both earlier calls WRONG
and kept visible: (round 3) primary ORACLE + secondary ENVIRONMENT via a
driver-vs-static harness story — REFUTED by the verifier's single-variable
swap; (round 4) "pure ORACLE, no environment gap" — ALSO WRONG, and this
round shows why. There IS an environment gap; it is simply neither of the
two I named. WHAT THE FIXTURE LACKS, stated precisely: the windowed fixture
resolves `render_entity()` — the leaf of the shipped `embedded_page`
item_template — through `StubBuilderServices::get_block_data`, which returns
a canned `table_expr()` for EVERY entity and renders nothing while raising
no error. PROBE (real shipped yaml, `real_profile_embedded_page_probe`,
loaded from assets/default/types/block_profile.yaml rather than a hand-built
DSL): the discriminating hypothesis that the routing's literal
`widget_name() == "live_query"` match misses a wrapped node is REFUTED — the
runtime widget_name IS `Some("live_query")` under the real profile, the
routing fires, and the container is NOT collapsed (expand_toggle h=96,
reactive_shell h=64, two tree_item rows h=32 each, correct geometry). What
is empty is the LEAF: with `render_entity()` both row shells paint no text
and no bullet; replacing ONLY that leaf with `text(col("content"))` makes
`text-block:child-*-content` and `tree_bullet::child-*` appear at the same
coordinates. This CONVERGES with the live evidence rather than contradicting
it: the live check probed the CHILD ENTITY ids, and only the leaf registers
a bounds entry under an entity id — row shells present with empty leaves
therefore reads as "element bounds never committed" from that probe. NOT
FIXED BLIND: the probe cannot distinguish "prod's `render_entity` leaf
paints nothing" from "the stub cannot resolve it", so per the dogfood
contract this lane STOPPED before any second fix. Next fixture increment,
required BEFORE a fix: a services impl with a real `get_block_data` +
profile resolution behind `render_entity()`, so the leaf is prod-shaped and
the red can be earned. Evidence:
`docs/Testing/fixture-logs-2026-08-08/task16-f1-opens-to-nothing.txt` (ROUND
5 section). **ROUND 6 — RED EARNED WITH A REAL ENGINE, AND THE ACTUAL ROOT
CAUSE IS UPSTREAM OF EVERYTHING THIS ROW PREVIOUSLY BLAMED.** Fixture 3
(`frontends/gpui/tests/nested_page_real_engine.rs`) boots a REAL
`ReactiveEngine` + `FrontendSession` and attaches a real GPUI window (donor:
`pbt_harness/windowed_wide.rs`, entered at
`compose_sut_windowed_base_seeded(&ComponentSet::full_headless(), ..)`;
focus via `SutFocusWrite::apply_navigate_focus`). WITH the round-3 routing
fix present the children still do not paint: gate `block_expanded_view` None
-> Some(true), glyph ▶ -> ▼ (so task #16's store unification HOLDS), and the
entire before/after delta is ONE element — `reactive_shell#73 w=848.0 h=0.0`
with NOTHING registered underneath it. ROOT CAUSE, pinned BEFORE the
renderer by two in-test probes against the live QueryEngine running the
shipped `embedded_page` content query: with the context shape the gpui
builder constructs (`frontends/gpui/src/render/builders/live_query.rs:72`,
`context_path_prefix: None`) `from descendants` delivers 0 changes; with a
path-resolved context (`lookup_block_path` =
`/block:real-host-page/block:real-nested-page`) it delivers 2.
`crates/holon/src/api/backend_engine.rs:470-481` turns that `None` into the
sentinel `"__NO_PATH__/"` — so a `from descendants` query issued through the
gpui `live_query` builder is GUARANTEED to match zero rows, silently, with
no error, no warning and no degraded banner. That is the fail-loud rule
inverted (priority 4, silently degrades to look fine) and it explains every
symptom at once: gate open, node present, zero children, ZERO ERROR LINES.
The renderer and the round-3 routing fix are NOT implicated — the rows never
reach them. The routing fix is NOT retracted (it is mutation-proven against
a fixture whose live_query did deliver rows, and it corrects a real 0-px
class `column::render` fixes the same way) but it was NOT the cause of the
live symptom; the lane had conflated those. CLASSIFICATION ATTEMPT #4, all
three earlier calls kept visible and all three wrong: (3) ORACLE+ENVIRONMENT
driver-vs-static — refuted; (4) pure ORACLE no environment gap — wrong; (5)
ENVIRONMENT = the fixture's `render_entity()` leaf is a silent stub — true
of fixture 2 but not the whole story, since fixture 3 resolves the leaf for
real and the rows still never arrive one layer earlier; (6) ENVIRONMENT = no
fixture ever drove the REAL query-context plumbing (every earlier one
supplied its own `watch_query_live` and never exercised context resolution),
with a first-class PERCEPTION component — the default arm degrades silently
AND gpui test targets install no tracing subscriber, so even a loud log
would have been discarded. ESCALATED, NOT CHANGED BY THIS LANE (each is a
codebase-wide call): (i) the sentinel default arm itself — making it loud or
making it resolve changes behaviour for EVERY caller passing `None`; (ii)
the two sibling frontend outliers
`frontends/ply/src/render/builders/live_query.rs:68` and
`frontends/mcp/src/describe_ui_expand.rs:143`, which pass `None`
identically, so `from descendants` is almost certainly dead in ply AND in
MCP `describe_ui` — meaning OUR OWN dogfood instrumentation shares the
defect and may report empty nested pages for the same reason the window
does; (iii) the missing tracing subscriber in gpui test targets.
`holon_service.rs:102`, `block_domain.rs:138` and `backend_engine.rs:1134`
all use `for_block_with_path`, so the service paths resolve it correctly —
the three frontend builders are the outliers. **FIXED 2026-08-08, SCOPED TO
THE GPUI BUILDER ONLY.** `frontends/gpui/src/render/builders/live_query.rs`
now resolves the context path prefix (`QueryContext::for_block_with_path`)
instead of passing `None`. RED on the current tree (`reactive_shell` h=0.0,
both children absent, verbatim signature above); GREEN after (shell h=80.0,
real `tree_item` / `tree_bullet` / `rendered_text` rows painted, element
count 113 -> 137); MUTATION-PROVEN — reverting ONLY the context construction
reds with the identical signature, restored and re-greened. FAIL-LOUD SHAPE,
and it was corrected under challenge: the first version PANICKED in the
render pass on both arms, which would have taken the window down in a
legitimate no-Turso (Loro-only) session where `query_engine()` is documented
to be `None` (reactive.rs:172-179). That was not hypothetical — the repo's
own `ExpandStoreServices` fixture overrides `watch_query` but not
`query_engine`, and the panic FIRED (4 of 2 target tests failed). The arms
are now split: no-query-engine -> `Ok(None)` + `tracing::warn` naming the
block AND the consequence (disclosed degradation, no error panel, the
supported mode still renders); lookup-failure -> `Err` -> a VISIBLE red
error element mirroring `builders/error.rs`. Never silent, never a crash.
SLO guard: the resolution is hoisted BEFORE entity creation behind a new
`LocalEntityScope::get_typed` cache peek, so the blocking matview read
happens once per cache miss as before, not on every render pass. STILL OPEN,
filed separately: the sentinel default arm itself (task #41), the ply + MCP
`describe_ui` siblings that pass `None` identically — so `from descendants`
is likely dead there too, INCLUDING in our own dogfood instrumentation (task
#42), and the missing tracing subscriber in gpui test targets that made the
PERCEPTION half structural (task #43). DISCLOSED NONDETERMINISTIC
OBSERVATION, not explained away: the full windowed suite at -j2 was run
twice — run 1 reported 271 passed / 2 failed, the extra red being
`general_e2e_composed_pbt_windowed` on `inv-no-errors` with a DDL/sync race
under load; it passes in isolation and did not recur in run 2 (272 passed /
1 failed, only the known `capmap` red). Flagged as a load-dependent flake on
the evidence available; NOT proven benign. Evidence: the same fixture log,
ROUND 6 section.
