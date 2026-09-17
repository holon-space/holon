---
id: 2026-09-17-search-oracle-compares-sut-ids-to-oracle-keys
date: 2026-09-17
gap: ORACLE
secondary: FALSE-ALARM
status: FIXED
summary: >-
  The keystone `Search` step compared the SUT's returned hit ids against the
  reference model's block keys, which part company on every transition-minted
  block — so a split TAIL could never be found and the step reddened as
  `quick_open_search(...) missed block::split-N` on roughly 29% of keystone-full
  sweeps. No product defect was behind any of those reds.
---

## Bug

The keystone (`general_e2e_composed_pbt`) reddened at
`crates/holon-integration-tests/src/pbt/transitions/search.rs:254`:

```
    quick_open_search("SdbRN1W") missed block::split-N in the In content section:
    ... only 1 of its 30 slots, so nothing was truncated
```

Found by two independent lanes, both reading it as a product regression in the
search path and spending bisect budget on it before root-causing the id space.

- 2026-09-16 19:11 (wave-14 land, ancestor of the tip):
  `/tmp/holon-w14-land/land-w14-keystone-full-1789577849.class:3`.
- 2026-09-17 02:00 at ancestor `4b077fd8b140`:
  `scratchpad/bisect/base-probe.out`, attempt=2, `site=…/transitions/search.rs:254`.

Rate over the seven keystone-full sweeps on disk at triage time: 2/7 = 29%. The
with/without-tip comparison is Fisher ~1.0 — pre-existing, elevated by nothing.

## Root cause

Two id spaces, compared as one. The reference model keys a split TAIL under the
synthetic label it allocated (`block::split-N`), which never changes: the
reconcile's synthetic→real pairing is a side map
(`crates/holon-integration-tests/src/pbt/composed/harness.rs:755`) and
`all_non_seed_block_ids()` returns the model's keys unchanged
(`crates/holon-integration-tests/src/pbt/ref_caps/blocks.rs:223`). The SUT mints
a fresh uuid for that same tail — this step's own precondition is that the
wiring mints uuids — and the driver returns raw SUT ids
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:2355-2360`).

The completeness loop therefore asked `content_hits.contains(&block::split-N)`,
an id the SUT never emits, so the answer was `false` for every split tail
whatever the search did. Consumers that need the other direction already crossed
that bridge (`OpDispatchWriter::resolve`, `EdgeFieldWriter::resolve`,
`DirectUserDriver::resolve`); the read side had no such bridge.

The consequence is not only noise: the message asserts a fact the test cannot
know. It reads a healthy search as a lost block, and cannot tell the difference
between the two — the `syn-real-mint` block-loss family would have produced the
byte-identical signature.

## Missing piece

An oracle that compares ids in ONE space. Nothing was missing from the
generator — the trigger (a drawn query folding onto a split tail's content with
an untruncated section) is drawn all the time, and the two sightings above are
raw keystone draws, not constructed cases. What was missing was any way for the
assertion to resolve a reference-model key through the same shared reconcile map
every other consumer uses before testing membership in a SUT-id set.

Classification note: `gap: ORACLE` because the escaping thing is the assertion's
watch (it could not distinguish a healthy search from a lost split tail), and
`secondary: FALSE-ALARM` because no product defect was behind a single one of
these reds — the literal shape of "an oracle stronger than the property under
test". Recorded under ORACLE so the QA-investment distribution sees it; reclassify
to FALSE-ALARM outright if the convention is that a harness-only gap is not an
escape.

## Remedy

FIXED, harness-only — no product code changed.

`SutSearch` gained the missing bridge,
`fn resolve_block_id(&self, id: &EntityUri) -> EntityUri`
(`crates/holon-pbt-core/src/capabilities.rs:2031`), implemented by the one SUT
that carries the cap as its existing `resolve_id` — the same shared resolver
`jump_to_search_hit` already resolves through
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:2335-2337`).
The completeness loop resolves each model key before the membership test
(`search.rs:248`), and the failure message names both ids when they differ, so a
future red says which space it looked in.

Pinned red-first by the hand-authored keystone case
`search-finds-a-split-tail-by-content`
(`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`):
create `keep REN tail`, split at byte 5 so the tail `REN tail` is the minted
block, then search the case-flipped word `ren`.

- RED (`lane-logs/red-hand-authored.log`), signature byte-identical to the
  sightings: `quick_open_search("ren") missed block::split-1 in the In content
  section: its content "REN tail" contains the query and the section returned
  only 14 of its 30 slots, so nothing was truncated`.
- GREEN with the fix (`lane-logs/green-hand-authored-case.log`).
- The assertion kept its teeth: a scratch run that drops every
  transition-minted block from the SUT's hit set — a genuinely lost split tail —
  still fails, now naming the resolved id
  (`lane-logs/teeth-probe.log`): `missed block::split-1 (SUT id
  block:89e9a8d0-2de1-40dc-900e-b990126d1f16)`.

Likely the same defect, NOT verified — `docs/Testing/KeystoneKnownReds.md`'s
`quick-open-pages-section-misses-a-matching-page` row carries the signature
`quick_open_search("doc_0") missed block:ref-doc-0 in the Pages section` whose
id is the OTHER synthetic scheme the same reconcile pairs (`block:ref-doc-N`,
CreateDocument-minted pages), so it is the same comparison failing in the Pages
section. It surfaced 1/10 draws on an unrelated rev and 0/13 on main, which is
what a draw-dependent oracle defect looks like. Whatever holds, the row stays
registered: this lane ran no sweep that reached a CreateDocument-then-search
draw, so nothing here is evidence the rate went to zero. Re-examine the row if
it recurs — a recurrence after this fix would be its own, different bug.

Adjacent gap, NOT fixed here, recorded so it is not mistaken for covered: the
soundness loop of the same step (`search.rs:206-222`) reads each hit's content
through `state.block_content(&hit.id)` with a SUT id, which misses for exactly
these minted blocks and silently skips them — so a split tail's hit is currently
exempt from the "hit content really contains the query" and the
wrong-section checks. It fails safe (skip, not false alarm) and closing it needs
the reverse lookup that no consumer has today.
