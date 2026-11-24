---
id: 2026-11-24-lane-test-selection-skips-the-source-scanning-architecture-rules
date: 2026-11-24
gap: ENVIRONMENT
secondary: null
status: PARTIAL
summary: >-
  New raw `LoroDocument::doc()` calls passed the lane check and its verify
  round, then turned the integrate gate red, because the lane's test selection
  never runs the architecture rules that scan every crate's sources.
---

## Bug

The integrate gate of a lane that changed `crates/holon-loro` and
`crates/holon-app/tests` went red in two steps:

- `gate-arch`: `loro_doc_escapes_match_the_allow_list`
  (`crates/holon-architecture-tests/tests/architecture_rules.rs:435`) found one
  new `doc()` call in `crates/holon-loro/src/loro_entity_operations.rs` and two
  in `crates/holon-app/tests/runtime_type_survives_restart.rs`.
- `extra-nextest`: the same test ran there too and was the step's only novel
  signature; its other reds matched rows of `docs/Testing/KeystoneKnownReds.md`.

Found by the integrate gate, after the lane check had passed and a verify
round had confirmed the lane. The rule existed and was correct; the lane never
ran it.

## Root cause

- The lane selection (`selection`, `scripts/select-tests.py:497`) picks the
  crates that own a changed path and their reverse dependencies.
  `holon-architecture-tests` depends on none of them, so its tests are not in
  the lane's nextest filter.
- The source scans of that crate read every crate's files from the file system,
  not through a dependency or a string literal, so the asset-reader rule does
  not see them either.
- The tree steps (`TREE_STEPS`, `scripts/select-tests.py:552`, with
  `gate-arch`) run only in the landing gate selection (`gate_selection`,
  `scripts/select-tests.py:725`). The lane selection never prints them, so
  `tests-cover-diff` asks no log for them, and the verify round checks only
  the selected steps.

## Missing piece

The lane check has no step whose result changes when a crate's source text
changes in a way that a source-scanning rule forbids. The landing gate is the
first run of `gate-arch` on the lane's tree. This is the escape of
`2026-10-12-a-new-doc-escape-reached-main-with-gate-arch-red` one stage
earlier: the gate now catches it, but only after the lane was verified.

## Remedy

- Done: the new calls go through `with_read` / `with_write`; no allow-list
  entry was added.
- OPEN (proposal): the lane selection treats `holon-architecture-tests` as a
  reader of every workspace crate's sources, so a changed `.rs` file under
  `crates/` or `frontends/` adds `package(holon-architecture-tests)` to the
  lane's nextest filter. The test is cheap and cannot be red for host timing.
  The verify round then checks it with the other selected steps, and the
  selector's self-check gets a golden case for it.
