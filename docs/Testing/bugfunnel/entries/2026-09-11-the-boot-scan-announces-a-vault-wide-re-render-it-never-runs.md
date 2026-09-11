---
id: 2026-09-11-the-boot-scan-announces-a-vault-wide-re-render-it-never-runs
date: 2026-09-11
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  One boot printed 2535 WARN lines announcing a re-render of every tracked file,
  from a walk over blocks the ingest had not created yet, and no re-render ran.
---

## Bug

Martin's app log for one boot over his production vault carries 2535 WARN lines
reading

```
[nearest_page_ancestor] the chain from block:<id> leaves the store at block:<id>
 — no owning page can be named while that row is missing; routing falls back to
 a re-render of every tracked file
```

2535 distinct block ids, every one inside the `org.initial_scan.ingest` span,
none after the scan. Found by Martin reading his own boot log; the lane was
opened on the premise that the boot scan was taking the `Broken` branch of the
routing fixed by
`2026-09-08-an-untracked-block-re-renders-every-tracked-file` 2535 times.

**That premise does not hold, and the measurement is the point of this entry.**
The 2026-09-08 fix is intact. In the same log:

| Marker | Count |
|---|---|
| `[nearest_page_ancestor] ... leaves the store` | 2535 |
| of those, where the walk broke at the START block (`start == cur`) | 2535 |
| `[OrgMode] no document could be resolved` (the only site that arms `Recover`) | 0 |
| `[nearest_page_ancestor] parent cycle` / depth-bound | 0 |

Not one routing decision saw these walks, and not one bulk pass was armed. The
log was announcing a consequence that did not follow.

## Root cause

`nearest_page_ancestor` (`crates/holon-filesystem/src/sync_ports.rs`) answered
`Broken(ChainLeftTheStore)` for two different questions:

1. an ANCESTOR row on the chain is missing — genuinely broken parentage, which
   the vault-wide recovery pass exists for;
2. the START block is not in the store at all — the caller asked about an id the
   store does not hold.

Case 2 is not a break. There is no chain. It is what
`FileSyncController::resolve_authoritative_doc` asks of every parsed block of a
file being ingested for the first time, through the cross-doc membership guard
at `file_sync_controller.rs:3960`, and that guard's own contract already reads
"an id-less / brand-new / unknown block resolves to `None` → normal ingest". All
2535 lines are case 2: the walk broke at the start block, and the caller
collapsed the answer through `into_page()` to `None` and did exactly the right
thing with it.

The disclosure was also authored at the wrong layer. The walk has three callers;
two of them (`resolve_authoritative_doc`, `owning_file_of`) collapse every "no
page" answer to `None` and re-render nothing. Only `route_homed_block` /
`route_remove` (`crates/holon-orgmode/src/di.rs`) turn a break into
`OrgRerender::All`, and they already disclose it themselves. So the walk was
narrating a consequence only its caller can know, and getting it wrong for two
callers out of three.

Cost: the emission of 2535 WARN lines, not the boot. Neither the store reads nor
any routing changed, so this bug does not account for the 80,367 ms
`boot_ingest_total` in that log — the per-file `boot_parse` and `projection`
stages do, and they are a separate concern. The real damage is to the fail-loud
channel: a WARN that reports a fault where there is none is misinformation, and
it sent this lane hunting a re-render storm that never happened.

## Missing piece

ORACLE. The interaction is the most ordinary one the system has — ingest a file
for the first time — and `crates/holon-orgmode/tests/ingest_contract.rs` already
drove it through the real `FileSyncController` several times per run. Nothing
anywhere asked whether an ingest REPORTED a fault while succeeding. Every
existing assertion reads the store after the ingest; the log is not part of any
ingest contract, so a success that shouts on its way through was invisible.

The keystone PBT (`general_e2e_composed_pbt.rs`) cannot express this either, for
the same reason: it has no notion of "an operation that succeeded must also have
claimed nothing". The natural home was the ingest contract that already owned
the wiring, which is where the pin went — no new harness, and the test drives
production's own controller.

## Remedy

Parsed apart at the walk, the way the 2026-09-08 fix parsed `NoOwner` apart from
`Broken`:

- `PageAncestor` grows `StartAbsent` — the store does not hold `start`, so there
  is no chain and nothing is broken. It is silent: asking about an unknown id is
  an ordinary question.
- `PageWalkBreak::ChainLeftTheStore` now strictly means an ANCESTOR row, and its
  WARN states the fact without naming a consequence it cannot know. The
  vault-wide-recovery disclosure stays where it is true, at the routing
  decision.
- `DocHome::from_walk` maps `StartAbsent` to
  `Unresolvable(UnresolvedHome::StartAbsent)`. `BlockHomeAuthority::locate`
  reads the block's row before it walks, so the authority can only reach this if
  the burst memo lost a row it just held — a defect, answered with the loud
  disclosed fallback rather than a silent drop.

A first cut of this fix reproduced the very mistake it removes, and a verifier
caught it. `BlockHomeAuthority::locate_batch` covers a block's parent from its
own snapshot; when the batch does not carry that parent it pays one
authoritative walk starting AT THE PARENT ID. For a block whose parent the vault
lost, that walk finds no row and answered `StartAbsent`, whose message blamed an
internal burst defect for an ordinary vault condition the reader has to repair.
The batch path now classifies its own question: a walk it starts from an
ancestor reports `Walk(ChainLeftTheStore)`, which is what that condition is and
what the code said before this entry's change.

That leaves `StartAbsent` reachable only from `locate`, where the row was read
one statement earlier — so the "burst lost a row it held" reading is now true
rather than merely asserted. Pinned by
`a_batch_orphan_with_a_dangling_parent_names_the_chain_not_the_burst`
(`crates/holon-orgmode/tests/routing_applicability.rs`), which asserts the
variant, the sentence the reader gets, and that the routing is still `Recover`.

**Reading the two cases apart is only possible in post-fix logs.** Before this
change both conditions printed the same `ChainLeftTheStore` line, so Martin's
boot log cannot be re-examined to ask how many of its 2535 lines were dangling
parents. For that log the question does not arise — every line came from
`resolve_authoritative_doc` on the ingest path and broke at the start block, not
from the home authority — but no earlier log supports the distinction, and none
should be read as if it did.

Pinned by `a_first_ingest_announces_no_vault_wide_re_render` in
`crates/holon-orgmode/tests/ingest_contract.rs`, which ingests three files —
including a nested file before its folder companion — through the real
controller and asserts the run reported no broken walk and claimed no vault-wide
re-render. Red before the fix (`lane-logs/red1-1789158203.log`) with 6 lines
carrying the production signature, one per parsed block.

Anti-overcorrection controls, both green throughout:
`an_absent_ancestor_row_is_still_a_broken_chain`
(`crates/holon-filesystem/tests/nearest_page_ancestor_walk.rs`) and the existing
`a_missing_routing_still_arms_the_bulk_pass`
(`crates/holon-orgmode/tests/routing_applicability.rs`).

Residual: the 80 s boot is untouched and unexplained by this entry. So is the
question of why `resolve_authoritative_doc` asks the authority about every
parsed block of a brand-new file at all — one point read per block at cold-boot
scale — which is a cost question, not a correctness one, and is not in this
change.
