---
id: 2026-11-06-snapshot-loaded-loro-doc-refuses-inline-marks
date: 2026-11-06
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  An org file with a verbatim mark failed its ingest with "LoroText mark verbatim: StyleConfigMissing"
  on a boot that loaded its Loro doc from a snapshot.
---

## Bug
Dogfooding a boot with a deleted holon.db and a kept Loro store: the vault's new-month template
failed its ingest with `LoroText mark verbatim: StyleConfigMissing`.

## Root cause
Loro keeps the text style config per `LoroDoc` instance, not in the snapshot. The fresh-doc path
installed Holon's style config, but the snapshot load built its doc with a bare `LoroDoc::new()`.
Loro's default config lacks `verbatim`, `strike`, `sub` and `super`, so a mark of those keys failed,
and gives `code` expand None instead of Holon's After, so typing at a code span's end did not extend
it (a silent divergence).

## Missing piece
No test wrote inline marks into a doc that a warm boot loaded from a snapshot.
- COVERAGE litmus: no transition sequence reached a mark write on a snapshot-loaded doc.
- ORACLE litmus: a mark write that fails would have gone red in any test that reached it.

## Remedy
- `styled_loro_doc` (crates/holon-loro/src/loro_backend.rs) is the one constructor of a doc that
  receives marks, and the snapshot load uses it.
- The PBT `every_mark_behaves_alike_on_a_warm_and_a_cold_boot`
  (crates/holon/tests/marks_survive_warm_boot_pbt.rs) draws marks of every key from the shared
  generator `inline_mark_strategy` (crates/holon-block-roundtrip-testing/src/lib.rs) on a cold and a
  warm boot. It was red on the base for every key that Loro's default config lacks, and for `code`.
