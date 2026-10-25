---
id: 2026-10-25-the-asset-seed-overwrites-a-users-journals-org
date: 2026-10-25
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The disk asset seed wrote the bundled `Journals.org` over a user's
  `Journals.org` that declared another `#+ID` or none, so the user's file
  content was lost on boot without a word.
---

## Bug
A vault whose `Journals.org` declares an `#+ID` other than the bundled
asset's (`journals`), or no `#+ID`, lost that file's whole content on the next
boot: the seed replaced it with the bundled asset. Nothing was logged.

Found by the fresh-context verifier of the change that ships the Journals page
as the asset `assets/default/Journals.org`, not by the lane's own tests.

Exposure: the controller writes a name-chain uuid `#+ID` onto every org file
without one (`OrgFormatAdapter::doc_id_from_content`,
crates/holon-orgmode/src/file_format.rs), so any `Journals.org` a user had
before carries an id other than `journals`.

## Root cause
`seed_default_org_assets` (crates/holon-orgmode/src/di.rs) keyed "already
seeded" on the asset's `#+ID` only. When no org file declared that id, it
wrote `root.join(filename)` without checking whether a file was at that path.

## Missing piece
- Coverage litmus: no transition sequence in the keystone catalog boots over a
  vault that already holds a file at a bundled asset's path, so no generated
  case reached this state. The lane's seed tests covered "a file declares the
  id" and "no file at all", never "a file at the asset's path with another id".
- Oracle litmus: a case that reached this state would have gone red on the
  block-content invariants (the user's blocks vanish), so the invariants were
  not the gap.

## Remedy
- The seed skips an asset when a file exists at its path, keeps that file
  unchanged, and logs a WARN naming the file and the asset's id.
- Pinned by `the_seed_keeps_a_users_journals_org_that_declares_another_id`
  (crates/holon-orgmode/tests/the_seed_keeps_a_users_file.rs): red with the
  seed before the fix and with the fix reverted, green with the fix.
- Open: in that vault shape the block seed still creates the `journals` page
  beside the user's page, so the root holds two "Journals" pages
  (`2026-10-25-journals-folder-and-seed-make-two-journals-pages`).
