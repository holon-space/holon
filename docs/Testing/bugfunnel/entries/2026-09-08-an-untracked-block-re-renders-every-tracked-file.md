---
id: 2026-09-08-an-untracked-block-re-renders-every-tracked-file
date: 2026-09-08
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A block no document owns armed the vault-wide recovery re-render on every
  write, because one `DocHome::Unresolved` meant both "nothing owns this"
  and "the owner could not be resolved".
---

## Bug

The same symptom record as
`2026-09-08-a-zero-byte-org-file-deletes-the-documents-blocks`
(vault `Plain-Text Layer.org`, `:ID: fb1a49a2-fbc1-487c-9cbb-1d9cc1073d11`):
5 Hz ERROR lines and a re-render of every tracked file per event. Backlog row
`routing-applicability-vs-missing`, whose done-criterion is "the fallback
fires only when a routed doc id fails to resolve".

Found by Martin dogfooding a production vault; the amplification half was
already partly diagnosed by the proposal-write lane
(`crates/holon-orgmode/tests/proposal_writes_do_not_amplify.rs`), which
bought back the proposal population with a predicate rather than fixing the
type that conflated them.

## Root cause

`DocHome` had two variants (`crates/holon-orgmode/src/home_authority.rs`),
and the walk that produced it, `nearest_page_ancestor`
(`crates/holon-filesystem/src/sync_ports.rs`), answered `Option<Block>` — so
FOUR distinct outcomes arrived as one `None`:

1. the chain reached the root sentinel with no `Page` above it,
2. a row on the chain was not in the store,
3. the chain was cyclic,
4. the chain exceeded `MAX_PAGE_WALK`.

Only 2–4 are faults. Case 1 is an ordinary top-level block: no document owns
it, it appears in no file on disk, and nothing is broken. Yet
`route_homed_block` mapped the single `Unresolved` to `BlockRoute::Recover`,
which sends `OrgRerender::All` — a re-render of EVERY tracked file, read from
the authority. So each write to any such block cost the whole vault, and the
`Remove` (departure) arm in `di.rs` did the same thing for a block leaving a
home that was never a document.

The debounce landed earlier (50 ms coalescing, vault row `Debounce the bulk
re-render`) made the storm survivable, but it is a rate limit on the wrong
decision: the pass still runs, and it still cannot affect the block that
armed it.

## Missing piece

ORACLE: the keystone PBT can generate a top-level non-page block and a write
to it — that is an ordinary transition — but no invariant asks how much work
one write caused. Nothing anywhere counts documents re-rendered per
interaction, so a decision that re-renders the whole vault for a block that
appears in none of it is invisible to every existing assertion. The one place
that DOES count renders is the proposal amplification test, and it counts
them only for proposal blocks.

Secondary, COVERAGE: cases 2–4 (a parent the store does not hold, a cycle,
the depth bound) have no generator at all — no transition produces a dangling
parent or corrupt parentage — so the population the recovery pass genuinely
exists for is never exercised end to end. The walk's own unit tests
(`crates/holon-filesystem/tests/nearest_page_ancestor_walk.rs`) covered the
walk terminating, not what the routing then did with the answer.

The keystone cannot reproduce this as a failure today: it would need a
per-interaction render-count budget (the natural ORACLE remedy) before the
amplification is expressible.

## Remedy

Fixed by parsing the distinction where it is produced, rather than
re-deriving it at each consumer.

`nearest_page_ancestor` now returns `PageAncestor`
(`Page(Block)` | `NoOwner` | `Broken(PageWalkBreak)`), where `PageWalkBreak`
names which of the three faults ended the walk; the depth bound, which
previously returned silently, is disclosed. Callers to whom every "no page"
answer means the same thing collapse it through the explicit
`PageAncestor::into_page()` — one named place where the distinction is
dropped, rather than a `None` that never carried it.

`DocHome` follows: `Resolved(EntityUri)` | `Untracked` | `Unresolvable`,
built by `DocHome::from_walk`. Both absences stay ordinary keys, so blocks
sharing one still group and retract correctly in `home_by`. The batch path
`locate_batch` classifies its own root-sentinel arm as `Untracked`, and a
block its top-down pass failed to cover as `Unresolvable` with a loud ERROR —
that is a defect in the pass, and recovering is the honest answer to it.

`route_homed_block` then routes `Untracked` to `BlockRoute::Drop` and
`Unresolvable` to `BlockRoute::Recover`, the latter with a WARN naming the
block, so the fallback is disclosed rather than silent. The departure arm,
which was a second inline transcription of the same decision inside the feed
loop, is now the shared `route_remove`, so a test driving it drives
production's own call.

Pinned by `crates/holon-orgmode/tests/routing_applicability.rs`, driving the
real `BlockHomeAuthority::locate`. Red before the fix
(`.claude/worktrees/plaintext-layer/lane-logs/red3-88231.log`):

```
an_untracked_block_arms_no_bulk_pass
  a block no document owns routes nowhere
    left: Recover
   right: Drop
```

Two neighbouring tests were strengthened rather than adapted:
`proposal_writes_do_not_amplify.rs` now passes `Unresolvable` where it
asserts the proposal-id predicate produces the Drop, so the home cannot
produce that Drop on its own; `nearest_page_ancestor_walk.rs` asserts the
exact variant instead of `is_none()`.

Residual: `Untracked` blocks are dropped, not reported. A block that SHOULD
be inside a page but sits at the root because a re-home half-landed now
routes nowhere silently rather than converging by accident. That accident was
never a guarantee — the bulk pass cannot place a block no file contains — but
nothing yet notices such a block exists. A store-health sweep over root-level
non-page blocks is the natural home for that, and is not in this change.
