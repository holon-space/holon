---
id: 2026-10-12-a-new-doc-escape-reached-main-with-gate-arch-red
date: 2026-10-12
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  A new `LoroDocument::doc()` escape in the typed-entity SUT reached main with
  `gate-arch` red, because the lane that landed it never ran the gate that
  already covers the rule.
---

## Bug

`cargo test -p holon-architecture-tests` was red on main at `27376da3`:
`loro_doc_escapes_match_the_allow_list` reported
`crates/holon-integration-tests/src/pbt/typed_entity_sut.rs: allow-listed 0,
found 1`. Found by the steward reading the arch-test state on main, not by any
gate run. The escape arrived with the typed-entity datatype axis (#168).

## Root cause

Two separate things, and only the first is a product-side defect:

1. `LoroTypedEntityReader::loro_typed_entity_rows` took the raw
   `Arc<LoroDoc>` out from under the doc-boundary lock
   (`crates/holon-integration-tests/src/pbt/typed_entity_sut.rs:95`) and read
   the `entity:<type>` root map from it. As a Loro-shape oracle this reader can
   observe a write batch's interior, so the oracle it serves could read a
   half-applied typed-entity write and call the shape wrong.
2. The escape reached main because the landing gate was not run, NOT because
   no gate covers the rule. `just gate-arch` — landing-gate step 7 — runs
   `cargo nextest run --workspace --features <CANON> --test architecture_rules`,
   which selects exactly this crate's only test target. Measured on main in
   this lane: `9 tests run: 8 passed, 1 failed`, `EXIT=100`
   (`target/archlint-gatearch-red.txt`). The brief's premise that "no gate runs
   that crate" is therefore refuted; the 2026-08-12 blind spot it recalls was
   already closed by that recipe.

## Missing piece

Nothing in the gate LIST. What is missing is enforcement that a lane's
landing gate actually ran green on the tree it lands: the gate is cheap (~19 s
warm for the arch step) and correct, and it was simply skipped. A red
architecture rule can therefore still reach main at any time, which is the
same escape shape as 2026-08-12 with a different cause.

## Remedy

- Routed the read through the sanctioned API: the whole row build now runs
  inside `LoroDocument::with_read`, so the oracle reads under the boundary
  lock and a lock error panics loudly instead of being swallowed. No
  allow-list entry was added — the site is not a long-lived
  transport/subscription nor a retained container handle, so it is not a real
  exception.
- `just gate-arch` green after the fix: `9 tests run: 9 passed, 0 skipped`,
  `EXIT=0` (`target/archlint-gatearch-green.txt`, test profile).
- OPEN, not done here: the "did the lane run its landing gate" enforcement is
  a process/steward change, outside this lane's scope.
