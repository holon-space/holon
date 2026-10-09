---
id: 2026-10-09-two-spellings-of-a-page-title-share-one-file
date: 2026-10-09
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Two spellings of one page title (`[[My Notes]]`/`[[my notes]]`, `café`
  composed/decomposed, `Straße`/`Strasse`) made two root pages; on APFS both
  write one file, so the second page's write-back overwrote the first page's
  file.
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

The round-5 verifier (lane-logs/recreate5-verify.md) reproduced two more
instances on the round-5 fix: a page with a body (`My Notes` + body) was
passed by the link-click position lookup, which keyed on the whole `content`;
and `café`/`cafe\u{301}`, `Straße`/`Strasse`, `σ`/`ς` stayed two pages,
because the key folded with `to_lowercase` only.

## Root cause
Two titles that name one file on a Mac were two pages. The by-name lookups
compared exact strings, then a key that folded less than APFS does
(simple lowercase, no Unicode normalization), and one of them read the whole
`content` instead of the title line. The write-back guard against two
documents on one file (`refuse_contested_path`) compared spellings, which
fold only when the file system that derived them folds (on a Mac once the
first file exists).

## Missing piece
The keystone file system `InMemoryFileSystem`
(crates/holon-filesystem/src/in_memory.rs) was case-sensitive (a `BTreeMap`
keyed by path), and then folded with `to_lowercase` only, so in the harness
the two pages got two files. Model and SUT agreed, and every row stayed green.

## Remedy
- One fold, `holon_api::caseless_fold` (crates/holon-api/src/caseless.rs):
  `NFD(casefold(NFD(s)))`, full case folding, the APFS rule. Pinned against
  the real file system of the host by
  crates/holon-filesystem/tests/path_collision_key_matches_the_host.rs (red
  with `to_lowercase`: lane-logs/recreate6/red-a-key.log).
- File boundary: `holon_filesystem::PathCollisionKey` (each path component
  folded); `refuse_contested_path` compares keys, so a second document is
  refused (`AMBIGUOUS PAGE-FILE PATH`) also where the deriving file system
  keeps two files (crates/holon-orgmode/tests/page_rename_retires_old_file.rs,
  `a_spelling_namesake_is_refused_*`).
- Environment: `InMemoryFileSystem` `PathCase::Insensitive` folds with
  `caseless_fold` and keeps the first spelling, as APFS does; the keystone
  harness uses it. Row `two-spellings-of-a-page-link-name-one-page`
  (keystone.jsonl) was red on the round-4 code
  (lane-logs/recreate5/red-row-case.log).
  Rows `a-page-with-a-body-reached-by-a-second-spelling-is-one-page` and
  `cafe-composed-and-decomposed-name-one-page` were red on the round-5 rule
  (whole content, `to_lowercase` fold; lane-logs/recreate8/red-row-*.log).
  `RenamePage` and `CreatePageAtFreedPath` generate second spellings
  (`holon_api::spelling::another_spelling`: case, NFD, `ß`/`SS`, spacing).
- Product: one title rule, `holon_api::PageTitleKey` (title line, whitespace
  collapsed, `caseless_fold`). `find_by_parent_and_name` (all stores, through
  `holon_filesystem::page_at_position`), `page_slot`, `create_forcing_id`
  (Live and Loro) and `SqlOperationProvider::page_at` compare keys of the
  title line. A position (parent, key) holds at most one page; two are an
  error, also on the link-click path when a link names one of them exactly
  (docs/Plans/PageIdentityDeterminism.md §5.3).
- Still open: link resolution (`resolve_page_name`, block_links) matches the
  leaf title exactly, so a `[[my notes]]` link to page `My Notes` is dangling
  in block_links when written; a click opens the existing page and heals the
  row.
