---
id: 2026-10-09-two-spellings-of-a-page-title-share-one-file
date: 2026-10-09
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Links `[[My Notes]]` then `[[my notes]]` made two root pages; on a
  case-insensitive file system both write `My Notes.org`, so the second
  page's write-back overwrote the first page's file.
---

## Bug
Found by the adversarial verifier of the recreate lane, round 4
(lane-logs/recreate4-verify.md, probe-a2.log and probe-fs.log). Round 4 made
`page_slot` compare titles exactly. A page id hashes the title folded for case
and spacing (`normalize_for_hash`, crates/holon-api/src/link_parser.rs), so
`[[my notes]]` found `My Notes` at its derived id, passed it and minted a
second page. A page's file is its raw title plus `.org`. On APFS the two names
are one file. Before the first file exists, `refuse_contested_path` compares
two different lexical keys and does not refuse, and the second write replaces
the first page's bytes.

## Root cause
Two titles that differ only in case or spacing were two pages, but one file
name on macOS, Windows and Android. The by-name lookups
(`find_by_parent_and_name` in crates/holon-app/src/turso_seams.rs and
loro_seams.rs) and `page_slot` (crates/holon-api/src/identity_recognition.rs)
compared exact strings. The id compared folded strings.

## Missing piece
The keystone file system `InMemoryFileSystem`
(crates/holon-filesystem/src/in_memory.rs) was case-sensitive (a `BTreeMap`
keyed by path), so in the harness the two pages got two files. Model and SUT
agreed, and every row stayed green.

## Remedy
- Environment: `InMemoryFileSystem` has `PathCase::Insensitive` (an entry
  keeps its first spelling, as APFS does), and the keystone harness uses it
  (crates/holon-integration-tests/src/test_environment.rs).
- Row `two-spellings-of-a-page-link-name-one-page` (keystone.jsonl). It was red
  on the round-4 code with `inv-every-page-has-its-own-file` and a
  DUPLICATE DOCUMENT ID error (lane-logs/recreate5/red-row-case.log).
- Product: one title rule, `holon_api::PageTitleKey`. `page_slot`,
  `find_by_parent_and_name`, `create_forcing_id` and the new by-position lookup
  `SqlOperationProvider::page_at` all compare keys. A position (parent, key)
  holds at most one page, and the first spelling stays the stored title
  (docs/Plans/PageIdentityDeterminism.md §5.3).
- Still open: link resolution (`resolve_page_name`, block_links) matches the
  leaf title exactly, so a `[[my notes]]` link to page `My Notes` stays
  dangling in block_links, but clicking it opens the existing page.
