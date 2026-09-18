---
id: 2026-09-18-search-oracle-soundness-compares-sut-ids-to-oracle-keys
date: 2026-09-18
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The Search step's SOUNDNESS loop read each returned hit's content and section
  by the SUT id the engine reports, so for every block a transition minted (a
  split tail, a `CreateDocument` page) the read missed and the check was
  silently skipped — the oracle could not flag a metacharacter treated as a
  wildcard, nor a wrong-section filing, on exactly the blocks its sibling
  completeness fix was about.
---

## Bug

Found by reading the landed sibling's own disclosure
(`docs/Testing/bugfunnel/entries/2026-09-17-search-oracle-compares-sut-ids-to-oracle-keys.md`,
"Adjacent gap, NOT fixed here"): the Search step's soundness loop
(`crates/holon-integration-tests/src/pbt/transitions/search.rs:206-222`) read
`state.block_content(&hit.id)` with a SUT id, which is `None` for every
transition-minted block, so `continue` skipped both the "hit content really
contains the query" assert and the "hit is filed in the right section" assert.

The same lane also carried the registry's last UNCURED instance of the family:
`quick-open-pages-section-misses-a-matching-page`
(`docs/Testing/KeystoneKnownReds.md`), whose signature
`quick_open_search("doc_0") missed block:ref-doc-0 in the Pages section` names
`block:ref-doc-N` — the OTHER synthetic scheme the same reconcile pairs.

## Root cause

Two id spaces, compared as one, in the reading direction this time. The reference
model keys a created page `block:ref-doc-N` (`pbt/action_actor_state.rs:52`,
`next_synthetic_doc_uri`) or a split tail `block::split-N`; the SUT mints a fresh
uuid and the driver returns raw SUT ids
(`pbt/frontend_slice/components.rs:2355-2360`). The landed fix added the
forward bridge (`SutSearch::resolve_block_id`, `holon-pbt-core/src/capabilities.rs:2031`)
for the completeness loop; the soundness loop needed the REVERSE direction, which
no consumer had, so it kept asking the model for a block it did not have and read
the answer as "nothing to check".

The Pages known-red is the completeness half of the same defect, in the Pages
branch. The landed fix's `sut.resolve_block_id(&id)` sits above the section split
and serves both branches, so it cures that arm too — the row's own observation
(1/10 draws on `pn-ordering` rev-2, 0/13 on main `8c8c564d`) predates the fix.

## Missing piece

Any bridge from a SUT id back to the model id, in both the soundness loop of the
Search step and (for pages) the completeness loop. Nothing was missing from the
generator: `CreateDocument` then a `Search` draw is an ordinary sequence, and the
completeness red was reproduced by a two-transition hand-authored case.

## Remedy

FIXED, harness-only — no product code changed.

`search.rs` now builds the model's id pairing ONCE, as
`Vec<(oracle_id, sut_id)>` over `RefBlockTree::all_non_seed_block_ids()` through
the SAME `SutSearch::resolve_block_id` the completeness loop already used, plus a
`BTreeMap` reverse index for the soundness loop. Deriving both directions from
one resolver is what makes them unable to disagree; the reverse index asserts its
own injectivity, so a mispair fails loudly naming both model ids instead of
reading the wrong block's content. The two loops then consume that one pass
instead of each walking the vault.

- RED, pre-fix: the lock case reds with the registered signature byte-for-byte
  but for the query (`lane-logs/red-pages-probe-prefix.log`) —
  `quick_open_search("DOC_0") missed block:ref-doc-0 in the Pages section: its
  content "doc_0" contains the query and the section returned only 1 of its 20
  slots, so nothing was truncated`.
- GREEN with the fix (`lane-logs/green-pages-probe.log`).
- Teeth for the completeness half, by inversion: dropping every Page-section hit
  from the SUT's result still reds, naming the resolved id
  (`lane-logs/teeth-pages-drop-all-pages.log`).
- Teeth for the soundness half, by inversion, and the isolating pair that proves
  the hole was real — the SUT is made to return the minted page as a CONTENT hit
  for the non-matching query `%`:
  - pre-fix the case PASSES, i.e. the bogus hit is silently skipped
    (`lane-logs/teeth-soundness-prefix-hollow.log`);
  - with the fix it reds naming the model id
    (`lane-logs/teeth-soundness-with-fix.log`) —
    `quick_open_search("%") returned block:317c2395-… (model id block:ref-doc-0)
    whose content "doc_0" does not contain the query`.
- Lock: hand-authored keystone case `search-finds-a-created-doc-page-by-title`
  (`hand-authored-regressions/keystone.jsonl`), which reproduces the registered
  known-red and keeps both loops live for minted pages.
- Registry: `quick-open-pages-section-misses-a-matching-page` flipped to
  `fixed-pending-soak`, so a recurrence now classifies as NOVEL.
