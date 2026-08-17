---
id: 2026-07-26-journal-auto-create-rule-reproduces-page
date: 2026-07-26
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The JOURNAL auto-create rule reproduces the page-id collision AUTONOMOUSLY —
  no user gesture (identity audit; surfaced by PR #99 on a random walk in
  which `CreatePageAtFreedPath` was NEVER drawn). After a `RenamePage`
  retitles a journal page, the auto-create rule's guard
  `block_exists("Journals/{today}")` fails (the page no longer carries that
  title), so the rule RE-CREATES the journal page, re-mints the SAME `blake3`
  id (`PageId::for_path`), and lands on `INSERT ... ON CONFLICT(id) DO UPDATE
  SET <every field except id>` (`sql_operation_provider.rs:662-676`),
  clobbering the title back to the date (2026-07-26 directed deterministic
  repro: the re-create RE-PAGES the row — page_tag stays true — so the clobber
  is TITLE-ONLY; the #99 random-walk `Tags({})` observation was likely the
  known matview tag-lag window). Observed verbatim: block `61133fe7-...` shown
  `"2026-01-15"` vs expected `"Moved"`; SUT `tags Tags({})` vs reference `tags
  Tags({"Page"})`. Same collision class as the in-app route two rows up, but
  reached by a production RULE rather than a user gesture — and the `Page`-tag
  loss is a strictly worse symptom.
source_line: 1112
---

## Bug

The JOURNAL auto-create rule reproduces the page-id collision AUTONOMOUSLY —
no user gesture (identity audit; surfaced by PR #99 on a random walk in
which `CreatePageAtFreedPath` was NEVER drawn). After a `RenamePage`
retitles a journal page, the auto-create rule's guard
`block_exists("Journals/{today}")` fails (the page no longer carries that
title), so the rule RE-CREATES the journal page, re-mints the SAME `blake3`
id (`PageId::for_path`), and lands on `INSERT ... ON CONFLICT(id) DO UPDATE
SET <every field except id>` (`sql_operation_provider.rs:662-676`),
clobbering the title back to the date (2026-07-26 directed deterministic
repro: the re-create RE-PAGES the row — page_tag stays true — so the clobber
is TITLE-ONLY; the #99 random-walk `Tags({})` observation was likely the
known matview tag-lag window). Observed verbatim: block `61133fe7-...` shown
`"2026-01-15"` vs expected `"Moved"`; SUT `tags Tags({})` vs reference `tags
Tags({"Page"})`. Same collision class as the in-app route two rows up, but
reached by a production RULE rather than a user gesture — and the `Page`-tag
loss is a strictly worse symptom.

## Root cause

the JOURNAL auto-create rule reproduces the page-id collision AUTONOMOUSLY,
no user gesture (identity audit, surfaced by PR #99 on a random walk with
`CreatePageAtFreedPath` NEVER drawn). After a `RenamePage` retitles a
journal page, the rule's guard `block_exists("Journals/{today}")` fails, it
re-creates the journal page, re-mints the SAME `blake3` id
(`PageId::for_path`), and `INSERT ... ON CONFLICT(id) DO UPDATE`
(sql_operation_provider.rs:662-676) clobbers the title back to the date
(2026-07-26 directed deterministic repro: the re-create RE-PAGES the row —
page_tag stays true — so the clobber is TITLE-ONLY; the #99 random-walk
`Tags({})` observation was likely the known matview tag-lag window) —
strictly worse than the user-gesture variant. Signatures
`inv-displayed-text/viewmodel` (stale title) +
`inv-blocks-match-ref/block_raw` (title + dropped `Page` tag). COVERAGE
primary: same temporal blocker (no pre-#99 rename), reached
RANDOM-WALK-ONLY; NO deterministic case yet — a dedicated hand-authored
keystone.jsonl case is still needed per the 2026-07-25 deterministic-replay
directive.)

## Missing piece

Same temporal blocker as the rows above — no pre-#99 transition renamed a
page, so a rename could never precede the journal tick's guard check.
Reached on a PR #99 random walk with `RenamePage` drawn on a journal page
and `CreatePageAtFreedPath` never drawn (the rule alone re-mints).
Signatures: `inv-displayed-text/viewmodel` (stale title) +
`inv-blocks-match-ref/block_raw` (title AND dropped `Page` tag). NO
deterministic case yet — random-walk-only; a dedicated hand-authored
`keystone.jsonl` case is still needed per the 2026-07-25
deterministic-replay directive (proptest seeds are not our regression
vehicle).

## Remedy

OPEN — random-walk red only, no parked deterministic case yet; fix = guard
the journal auto-create rule (and `create_page_from_link` generally) against
re-minting an id that already exists so `ON CONFLICT DO UPDATE` can never
clobber a live page; reconcile with PageIdentityDeterminism.md §5.3.
