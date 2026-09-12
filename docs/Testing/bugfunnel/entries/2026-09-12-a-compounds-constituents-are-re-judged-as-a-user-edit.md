---
id: 2026-09-12-a-compounds-constituents-are-re-judged-as-a-user-edit
date: 2026-09-12
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A compound dispatched with a non-user origin decomposed into constituents the
  engine sent under a hardcoded `OpOrigin::User`, so an ingest-origin source
  rewrite of a read-only-homed block was refused by a gate that exempts ingest.
---

## Bug

Found by reading in lane `reimport-write-tier` (ruling D118.a), then measured
against the keystone. The engine's compounds — block→page, the task-keyword
family, `set_field(source_text)`, `merge_blocks`, the trust gate's proposal
coercion — send their constituent writes straight to the dispatcher through
`OperationProvider::execute_operation`, which resolves to
`execute_operation_with_input` and hardcodes `OpOrigin::User`
(`crates/holon/src/api/operation_dispatcher.rs`).

Every gate that reads the origin therefore judged a constituent as a human's
gesture. The one that acts on it today is the write tier: it exempts `Ingest`
because ingest is the file telling the store what it says, so an ingest-origin
rewrite of a block homed in a `WriteTier::ReadOnly` file was refused — the file
rejected because the store may not tell the file.

## Root cause

`origin` reached the compound (`run_set_source_text(&params, &origin)`) and
stopped there. The constituents took a dispatcher entry point that has no
origin parameter, so the provenance the caller stated was dropped one level
below where it was needed. Not a missing check: a value that had nowhere to
travel.

## Missing piece

No keystone seam could dispatch a non-user origin. Every SUT
`execute_operation` in the composed harness passes `OpOrigin::User`, and the
transitions that look like sync (`ShareContainer`, `SyncNow`,
`ReceiverCreateBlock`) bind `SutTwoInstance`, which the single-instance
keystone does not supply. The interaction was ungeneratable, so the gate's
exempt half was never exercised at all — only its refusing half was.

## Remedy

One seam, `DispatchingOperationEngine::dispatch_constituent_op`, routes every
compound constituent through `execute_operation_with_provenance` under the
COMPOUND's origin (`Verbatim` unchanged: the compound computed those bytes).
The three constituent dispatchers and the trust gate's proposal writes all go
through it, and the post-write convergence repairs carry the same origin.

The dead `OpOrigin::Sync` arm of `enforce_write_tier` went with it: no
production site constructs that origin, the share backend and the pairing
re-import now ask the authority at their own seams, and the arm's only caller
was its own unit test.

Covered in the keystone: transition `AttemptIngestCompoundOnReadOnly` aims an
ingest-origin `set_field(source_text)` at a `.cook`-homed step, writing the
block's own stored source back so an accepted one leaves `block_raw`
unchanged; `inv-read-only-home-refuses-writes` gained the clause that judges
it. Replayed deterministically as
`an-ingest-origin-compound-against-a-read-only-homed-block-is-not-refused` in
`hand-authored-regressions/keystone.jsonl`. Red for the right reason with the
seam reverted to the hardcoded user origin: `1 of 1 INGEST-origin compound(s)
aimed at a read-only-homed block were refused by the write-tier gate, which
exempts that origin`.

`OpOrigin::Sync` the VARIANT stays. It is a declared, parseable class of the
trust profile's policy language (`crates/holon-profiles/src/trust.rs`), so
deleting it would leave `origin: sync` in a user's profile naming a class
nothing can produce — the unrepresentable state would be created by the
deletion, not removed by it.
