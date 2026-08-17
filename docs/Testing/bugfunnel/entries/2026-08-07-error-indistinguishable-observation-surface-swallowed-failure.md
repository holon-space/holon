---
id: 2026-08-07-error-indistinguishable-observation-surface-swallowed-failure
date: 2026-08-07
gap: PERCEPTION
secondary: ORACLE
status: FIXED
summary: >-
  The `org render is DEGRADED` ERROR is indistinguishable, at the observation
  surface, from a swallowed failure — so the same by-design disclosure was
  re-triaged on each of its 4 sightings and was twice attributed to the wrong
  branch.
source_line: 1177
---

## Bug

(overnight dogfood-explorer, same session — registered so the family CLOSES;
the render itself is NOT a defect) **The `org render is DEGRADED` ERROR is
indistinguishable, at the observation surface, from a swallowed failure — so
the same by-design disclosure was re-triaged on each of its 4 sightings and
was twice attributed to the wrong branch.** Captured payload: `org render is
DEGRADED for this block: DROPPING styling and protective (1 mark(s)) — the
content bytes survive: no quote delimiter in ['=', '~'] renders content
"nested a ~b~ c" (marks [MarkSpan { start: 10, end: 11, mark: Code }]) back
to "nested a ~b~ c"`. TWO corrections to the standing account. (i) The site
is `models.rs:1152`, which is the ONE `tracing::error!` inside
`disclose_degraded_render` and is therefore shared by all four
`DegradeReason`s — it does not identify a branch. The branch actually taken
is ladder rung 3 (`RenderFidelity::ProtectiveDropped`), NOT the terminal "NO
emission of this block settles" arm at `models.rs:1120`, which stays
unreached. (ii) The shape is unrepresentable in org, so the emission is the
best available outcome, not a failure to find one: the `Code` delimiter IS
`~`, and the only bytes that could carry both a literal `~` and a code span
over `b` are `~~b~~`, which org reads as ONE code span over `~b~`. Splitting
the quoting (`=~=~b~=~=`) does not escape it either — measured, orgize takes
that as a single verbatim over `~=~b~=~`, and real org refuses it too
because an emphasis opener may not follow `=`. Prod therefore drops the
mark, re-seals the literal as `=~b~=`, keeps EVERY content byte, and settles
on the first cycle (verified: cycle 2 emits identical bytes at fidelity
`Exact`). The limitation is narrow, not general — a mark elsewhere in
content that merely CONTAINS `~b~`, content holding BOTH quote delimiters,
and a mark spanning the literal's own delimiters all render `Exact`.

## Root cause

secondary ORACLE: the org-render DEGRADED family, registered so it CLOSES —
the render itself is NOT a defect. `models.rs:1152` is the single
`tracing::error!` shared by all four `DegradeReason`s, so it names no
branch; the branch actually taken for the captured `"nested a ~b~ c"` +
`Code(10,11)` payload is ladder rung 3 (`ProtectiveDropped`), not the
terminal "no emission settles" arm at `models.rs:1120`, which stays
unreached. The shape is genuinely unrepresentable in org — the code
delimiter IS `~`, so `~~b~~` can only mean one code span over `~b~` — and
prod already does the best available thing: drops the mark, re-seals as
`=~b~=`, keeps every content byte, settles on cycle 1. The escape is that
this designed fallback is emitted at the SAME severity as a swallowed error
and names no rung, so each of its 4 sightings cost a fresh root-cause. No
code change; three regression-lock tests landed instead, and the severity
policy is escalated to Martin)

## Missing piece

Not a coverage gap: `marked_content_strategy`'s `MarkGeometry::Inside`
generates exactly this geometry and
`any_generated_store_state_reaches_a_fixed_point` (600 cases) exercises it —
it passes because degrading correctly IS the contract, so no headless
assertion could ever have "caught" it. The escape is at the observation
surface: a designed, content-preserving fallback is emitted at the SAME
severity as a real swallowed error and names no rung, so an operator reading
a log cannot separate the two, and each sighting costs a fresh root-cause.
Secondary ORACLE for the latent consequence: `inv-no-observed-errors`
classifies any ERROR log as a swallowed problem, so the day a keystone
transition puts a mark inside a markup literal in block content, the
keystone goes RED for behaviour that is correct by design. Not reachable
from today's transitions (they type plain text), hence latent rather than
firing. Missing piece = a severity/allowlist policy that separates
"disclosed degradation" from "swallowed error", which is a product call, not
an implementation detail.

## Remedy

CLOSED-AS-BEHAVIOUR 2026-08-07 — no code change; the render is correct and
stays as-is. Registration + regression lock is the whole fix. Locked by
three tests in
`crates/holon-org-format/tests/render_marks_fixed_point_pbt.rs`:
`a_code_mark_between_literal_code_delimiters_keeps_every_byte_and_settles`
(pins the exact payload, the exact emitted bytes `nested a =~b~= c`, the
RUNG `ProtectiveDropped` — so a fall-through to `ContentUnpreserved` reds —
and byte survival),
`a_styling_mark_between_literal_code_delimiters_stops_at_the_styling_rung`
(the same span one rung up), and
`a_mark_outside_the_literal_keeps_full_fidelity` (the discriminating
control: if the ladder ever degrades these three shapes it has become
over-eager and the rows above stop describing org rather than the code).
Both mutation-proven: replacing rung 3's `keep(&[DataBearing])` with
`keep(&[DataBearing, Protective])` reds the first test with `left:
AllMarksDropped, right: ProtectiveDropped`; neutering
`quotable_markup_spans`' sealed-span exclusion reds the control. Both
sources sha256-restored byte-identical after each probe. SEVERITY RESOLVED
2026-08-07 by Martin's ruling (option 2 — demote the content-preserving
rungs): `disclose_degraded_render` now emits WARN for every rung whose bytes
survive (`StylingDropped`, `ProtectiveDropped`, `AllMarksDropped`, per the
philosophy's priority 2 "falls back visibly") and keeps ERROR only for
`Unrepresentable`, the one rung whose emission no longer re-parses to the
stored content (priority 3). Every emission, DEBUG repeat included, now
carries a structured `rung` field, so a reader no longer has to re-derive
the branch from the prose — that was the other half of this escape. This
also disarms the latent keystone false red: `inv-no-observed-errors` sees
nothing for a correctly-degrading mark. Locked by
`crates/holon-org-format/tests/degraded_render_severity.rs` (4 tests, teeth
both ways: the two content-preserving rungs must emit exactly one WARN
naming their rung, `ContentUnpreserved` must still emit ERROR naming
`Unrepresentable`, and an `Exact` render must emit nothing). Mutation-proven
in both directions — see the lane report. The three rung/bytes locks above
are untouched and stay green.
