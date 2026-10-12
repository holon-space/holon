---
id: 2026-10-12-the-block-slash-menu-no-longer-offers-delete
date: 2026-10-12
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The block slash menu offers "Delete Subtree" and "Delete Keep Children" but
  no longer offers "Delete", which the LogSeq-parity fixture pinned as a real
  registry-derived menu entry on 2026-08-26.
---

## Bug

Found during lane `itests-green` (holon-space/holon-work#163) while triaging
the full suite on main. `catalog_suite logseq_parity_replay::logseq_parity_corpus_replay`
fails (a real 292 s failure once the machine is unloaded enough for it not to
time out) with:

    [slash-menu] expected block "block:c1"'s menu to offer "Delete", but it
    offers ["Indent", "Outdent", "Move Up", "Move Down", "Delete Subtree",
    "Delete Keep Children", "Cycle Task State", …]

The fixture
`crates/holon-integration-tests/tests/fixtures/logseq-parity/tasks.feature:98-118`
pins four of the menu entries with a MEASURED 2026-08-26 comment that lists
the full menu of the day — `Delete` among them — and states the guard's
purpose: "a registry change that drops or renames an op cannot pass
unnoticed". So the guard is firing exactly as designed.

## Root cause

NOT established — two candidates, and I did not root-cause either because the
lane's job was classification:

1. The block entity's op set no longer contains the generic `delete`
   descriptor. That descriptor is built by the typed-entity write rule
   (`crates/holon-core/src/type_write_rule.rs:315-338`: `name: "delete"`,
   `display_name: "Delete"`, `menu_exposure: Listed { slash_menu: true }`),
   so either the block entity stopped being fed through that rule or its
   exposure is filtered out downstream.
2. The pin is stale: `delete` was deliberately superseded for blocks by the
   two explicit ops (`delete_subtree`,
   `crates/holon-core/src/traits.rs:2704`, and `delete_keep_children`, :2757)
   because a plain delete of a block WITH CHILDREN is ambiguous, and the
   fail-closed design steers the user to the explicit pair. The seed's `c1`
   does have children.

Candidate 2 would make this a FALSE-ALARM. It is not recorded as one, because
the triage rule keeps an unexplained failure in its escape class until it is
root-caused, and no commit was found that documents the removal.

## Missing piece

The only guard on the block slash menu's op set is a hand-measured literal in
a gherkin fixture inside a replay that routinely exceeds 290 s and times out
under load — so when it IS the thing that notices, the signal arrives as a
timeout rather than as a named assertion. There is no catalog-level oracle
asserting which registered ops are slash-menu-exposed for `block`, even
though that set is fully derivable from the descriptors and the keystone can
already type "/" through the production driver.

## Remedy

Open, and it is a ruling, not a lane fix: either `block.delete` must come
back to the menu, or the fixture's `Delete` expectation is replaced by the two
explicit ops and the MEASURED comment is re-measured. Whichever way it goes,
the durable remedy is a catalog-level pin of the slash-menu-exposed op set per
entity, so the next drop or rename reds in seconds instead of in a 292 s
replay.
