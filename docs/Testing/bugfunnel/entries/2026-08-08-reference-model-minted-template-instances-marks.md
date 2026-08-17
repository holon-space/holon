---
id: 2026-08-08-reference-model-minted-template-instances-marks
date: 2026-08-08
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The reference model minted template instances marks-free, which both raised
  a false red against correct SqlOnly behaviour and CONCEALED a real one:
  `LoroBlockOperations::create` ignored the top-level `marks` param — the
  shape org/markdown ingest and `plan_instantiation` emit — so under the Loro
  CRUD authority every instantiated rich block was born plain while the SQL
  authority kept its marks.
source_line: 769
---

## Bug

(the composed keystone reached its first `InstantiateTemplate` after the
outdent abort was fixed, then INSPECTION of the resulting divergence) **The
reference model minted template instances marks-free, which both raised a
false red against correct SqlOnly behaviour and CONCEALED a real one:
`LoroBlockOperations::create` ignored the top-level `marks` param — the
shape org/markdown ingest and `plan_instantiation` emit — so under the Loro
CRUD authority every instantiated rich block was born plain while the SQL
authority kept its marks.** The driver has seeded the definition child `see
{{date}} now` with `see` bolded since 2026-07-22 precisely to exercise
`remap_marks`; `ReferenceState::apply_instantiate_template` never modelled
the inheritance, so the existing `watch-rows-template-child-parent` case
drove the mark-dropping create on every gate run and stayed green.

## Root cause

the reference model ASSERTED a prod defect, so the keystone's own
InstantiateTemplate coverage certified it green for eleven days.
`ReferenceState::apply_instantiate_template` minted the instance child
through `Block::new_text` — `marks: None` — while
`DirectUserDriver::instantiate_template` seeds the DEFINITION child `see
{{date}} now` with `see` bolded
(`crates/holon-integration-tests/src/mutation_driver.rs:360-363`, there
since 2026-07-22) precisely so instantiation exercises prod's mark-carrying
path (`plan_instantiation` → `remap_marks`,
`crates/holon-api/src/template_instantiation.rs:343-350,541-571`,
unit-tested by `marks_shift_and_stretch_across_substitution`). TWO
consequences, opposite signs. (i) On a Turso-only draw the SUT carries the
mark and the model does not, which is the `marks: sut=Some([MarkSpan {
start: 0, end: 3, mark: Bold }]) ref=None` red the composed keystone reached
at original line 1817 of the 64-case run, preserved as
`docs/Testing/fixture-logs-2026-08-08/keystone-green-64-marks-divergence.txt`
(trimmed from the foreign worktree's gitignored
`agent-aa73d6025c345f27f/lane-logs/keystone-green-64.log`), once the outdent
abort (d1642fc2) stopped masking it — a FALSE red against correct prod
behaviour. (ii) Far worse, the same wrong model made the Loro arm look
right: `LoroBlockOperations::create` read marks ONLY from a delete-inverse
`content: Object{text, marks}` payload and IGNORED the top-level `marks`
param — the shape org/markdown ingest
(`crates/holon-orgmode/src/block_params.rs:39`,
`crates/holon-markdown/src/params.rs:34`) and `plan_instantiation` both emit
— so under the Loro CRUD authority every instantiated rich block was BORN
PLAIN while the SQL authority kept its marks. Silent, mode-dependent loss of
the user's emphasis and links, with the param additionally leaking into the
properties blob because `marks` was absent from `handled_fields`. The
existing `watch-rows-template-child-parent` hand-authored case drove that
exact create on the full_headless (Loro) wiring on every gate run and stayed
green ONLY because the oracle expected the loss. ORACLE, not COVERAGE: the
interaction was generated constantly and the invariant that compares the
Marks facet (`inv-blocks-match-ref/*`) was engaged and non-vacuous — it was
fed a model that encoded the bug. FIXED both sides: the model now inherits
the definition child's marks (`tpl_child_marks`, span 0..3 sits before the
`{{date}}` slot at byte 4, so prod's remap is the identity here), and
`create` honours a top-level `marks` param, refusing loudly if it disagrees
with a content-Object payload rather than picking a winner. Red logs
preserved in-repo:
`docs/Testing/fixture-logs-2026-08-08/red-oracle-instance-marks.txt`
(sut=Some ref=None) and `.../red-loro-create-drops-marks.txt` (sut=None
ref=Some, first-divergent-layer store/CRDT). Locked by hand-authored
`template-instance-inherits-definition-marks` and three mutation-proven unit
tests: `create_applies_top_level_marks_param`,
`create_with_out_of_bounds_marks_leaves_no_block` (an unapplicable span must
fail BEFORE any write — Peritext applies marks after the node exists, so a
late rejection left a half-created plain block) and
`delete_inverse_ignores_a_legacy_marks_property` (the delete inverse sources
marks from `block.marks` alone; a doc written before `marks` was routed to
Peritext carries a stale `properties["marks"]` that the verbatim properties
splat would have handed back as a real marks param, resurrecting a plain
block RICH))

## Missing piece

Nothing was missing from generation or wiring: the interaction ran
constantly and `inv-blocks-match-ref/*` compares the Marks facet. The model
itself asserted the defect — the reference must state prod's specified
behaviour (an instance inherits the definition's rich text), not the
behaviour the implementation happens to have.

## Remedy

**FIXED 2026-08-08.** Model: `seed_template_definition` /
`apply_instantiate_template` carry `tpl_child_marks()`. Prod: `create`
honours a top-level `marks` param via the existing Peritext re-apply, errs
when it disagrees with a content-`Object` payload, and no longer leaks
`marks` into the properties blob. Red logs preserved at
`docs/Testing/fixture-logs-2026-08-08/{red-oracle-instance-marks,red-loro-create-drops-marks}.txt`;
locked by hand-authored `template-instance-inherits-definition-marks` +
three mutation-proven unit tests (`create_applies_top_level_marks_param`,
`create_with_out_of_bounds_marks_leaves_no_block`,
`delete_inverse_ignores_a_legacy_marks_property`). SCOPE WIDENING RETRACTED
2026-08-08 — mechanism refuted by measurement, superseding the same-day
static trace. The retracted claim, verbatim, so the ledger stays honest:
"`split_block`'s fallback create (`crates/holon-core/src/traits.rs:1282` →
`:1347`) also emits the top-level `marks` param for the right half and DID
reach `LoroBlockOperations::create` — the branch gates on `cells() == None`,
which the Loro provider never overrides, NOT on SqlOnly as its adjacent
ALLOW comment claims — so pre-fix, splitting a marked block under the Loro
authority silently dropped every mark falling right of the split." Panic +
`eprintln` probes over the full hand-authored suite measured the opposite:
the `!wrote_create_via_cell` fallback IS reached by the composed keystone,
but only in **SqlOnly-wired draws** (e.g.
`main-panel-drops-refocused-split-block`, storage `["Org","Turso"]`), always
with `Self = SqlBlockOperations` — whose `create` honours the `marks` param
— and always with `right_marks == 0` (8/8 reaches).
`LoroBlockOperations::create` was NEVER observed receiving a split's marks
param. Under `full_headless` a split takes the cell/Peritext
`BlockContent::RichText` route instead, which is where marked-split coverage
lands. `LoroBlockOperations::create`'s marks handling is real and
load-bearing for INSTANTIATION and INGEST, and stays guarded by its unit
tests. BOTH follow-ups CLOSED 2026-08-08 (task #8). (i) The hardcoded ref
marks are gone: driver seed and reference expectation now come from ONE
fixture (`crates/holon-integration-tests/src/template_fixture.rs`), and the
expectation is COMPUTED by a ref-side substitution+remap instead of written
down. The definition child gained `Italic 4..16`, a span that covers the
`{{date}}` slot AND the text after it, so it must stretch and shift; against
the old literal expectation that is red (`marks: sut=Some([Bold 0..3, Italic
4..14]) ref=Some([Bold 0..3])`, `lane-logs/t8-a-red-crossing-mark.txt`), and
prod AGREES with the ref remap on it — a fixture unit test cross-checks the
two against `plan_instantiation` directly. (ii) Marked-split-under-Loro is
covered by hand-authored
`split-of-a-marked-block-keeps-the-right-half-rich-under-loro` (instantiate,
then split the rich instance child at 6 so the Italic span straddles);
mutation-proven by withholding split_block's RichText marks, which reds it
at the split on the right half (`marks: sut=None ref=Some([Italic 0..8])`)
while the template row stays green. The covered route is the cell/Peritext
create, which is the route a `full_headless` split actually takes (the
dispatching provider's `cells()` is `Some`); the `!wrote_create_via_cell`
fallback is the SqlOnly path measured above, and a Loro-only draw carries
neither `SutBlockTreeWrite` nor `SutTemplateInstantiate`, so no draw both
mints a marked block and splits it on a Loro-only provider stack.
