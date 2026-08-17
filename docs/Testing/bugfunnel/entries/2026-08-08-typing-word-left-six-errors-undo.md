---
id: 2026-08-08-typing-word-left-six-errors-undo
date: 2026-08-08
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  Typing one word left six `undo: dropped stale entry` ERRORs and an undo that
  walked back ~1 character per press instead of one word group — the history
  silently loses steps.
source_line: 755
---

## Bug

(task #29 lane, dogfood-explorer pass 2 P2 F3) **Typing one word left six
`undo: dropped stale entry` ERRORs and an undo that walked back ~1 character
per press instead of one word group — the history silently loses steps.**
`expected String("second lin") but found Some(String("second li"))` and five
more, ending `expected String("second line") but found None`. The editor
spawns one un-awaited task per keystroke
(`crates/holon-frontend/src/operations.rs:39`; the log shows eleven writes
to one block in flight at once), so `set_field`'s separate prior-read
(`sql_operation_provider.rs:2401,2471`) captures an already-superseded value
— the stored inverse skips characters — and the undo entry is journaled in
completion rather than write order. `verify_precondition` then legitimately
fails against a history recorded wrong. Round 2 (verifier): the ENGINE-LEVEL
COMPOUNDS (`convert_block_to_page`, `merge_blocks`) run the same
read-modify-write-journal step but dispatch their constituents straight at
the dispatcher, so the first fix never covered them.

## Root cause

task #29 lane, found by the dogfood-explorer pass 2 (P2 F3) driving the real
GPUI app: **typing one word left SIX `undo: dropped stale entry` ERRORs and
an undo that walked back ~1 character per press instead of one word group —
the user's history silently loses steps.** Verbatim from `app2.log`: `state
changed under undo: block:5260462b-\u2026.content expected String("second
lin") but found Some(String("second li"))`, five more of the same shape,
ending `expected String("second line") but found None`. ROOT CAUSE is a
write race, not the staleness check:
`holon_frontend::operations::dispatch_operation`
(`crates/holon-frontend/src/operations.rs:39`) SPAWNS one un-awaited task
per keystroke, and the log proves the concurrency directly \u2014 the eleven
keystrokes of "second line" are issued at 18.673\u201318.750 while their
completions trail to 18.844, so the last keystroke is dispatched with six
earlier writes still in flight. Two things then break at once: (a)
`SqlOperationProvider::set_field` reads the prior content in a statement
separate from its UPDATE
(`crates/holon/src/core/sql_operation_provider.rs:2401,2471`), so an
interleaved keystroke's stored INVERSE skips a character the user typed
through \u2014 visible in the log as undo replays writing "second li",
"secon", "seco", "se"; and (b) the undo entry is pushed after the dispatch
returns, i.e. in COMPLETION order, so the stack order need not match the
write order, and the multi-character deltas the stale prior produces are not
word-boundary-coalescible (`crates/holon-core/src/undo.rs`
`classify_delta`), which is exactly the ~1-char-per-press symptom.
`verify_precondition` (`undo.rs:53-66`) then legitimately fails against a
history that was recorded wrong, and drops the entry. ENVIRONMENT,
**re-triaged from the dogfood pass's proposed ORACLE**: every harness rung
awaits each write before issuing the next, so no rung can put two writes to
one block in flight \u2014 the failing timing does not exist in the test
environment, the ENVIRONMENT litmus. The oracle would have fired the moment
the interleave existed (the new test's first assertion is a drop, with no
new invariant machinery). Secondary ORACLE for the residual: no invariant
states "every edit the user made stays undoable". FIXED in-lane:
`EntityWriteLocks` in `operation_engine.rs` makes
capture-prior/write/journal ONE step per entity (64 fair
`tokio::sync::Mutex` stripes keyed by (entity, id), acquired after the
compound interceptors so no hold nests on the same entity; `replay` takes
the same lock). Red-first via
`crates/holon/tests/undo_concurrent_keystrokes.rs`, which types with the
editor's real spawn-per-keystroke shape and asserts differentially against
the same typing done one awaited write at a time \u2014 3/3 red at base with
the production signature, green after. SCOPE WIDENED IN ROUND 2 after a
verifier refuted the first fix's reach: the ENGINE-LEVEL COMPOUNDS took no
stripe at all. `convert_block_to_page` and `merge_blocks` do the same
read-modify-write-journal step in the engine — planner read, constituent
writes, ONE composite entry — but their constituents call
`dispatcher.execute_operation` DIRECTLY (`dispatch_constituent` :672,
`dispatch_merge_constituent` :1228), so nothing about the singles fix
applied to them and their composite pushes (:973, :1281) sat outside every
hold; `turn_into_page` is exactly the chord task #28 made reachable from a
live window mid-typing. Both now take the stripe of the block they rewrite
(`target` / `canonical`) for their whole span, ONE hold at a time — the
constituents keep dispatching directly, so no hold nests and the striping
stays deadlock-free without a lock order. Red-first for both in
`crates/holon/tests/undo_compound_interleave.rs`, each killed by removing
its own stripe alone (C1a/C1b) with the production signature. DISCLOSED
RESIDUAL: only the rewritten block is held — a convert's minted page,
re-homed children and re-pointed linkers, and a merge's duplicate, get no
stripe. THE M2 MUTANT IS NOW UNREPRESENTABLE rather than merely unkilled:
the push goes through `journal_step(guard.as_ref(), entry)`, so releasing
the stripe first fails to compile (`borrow of moved value: write_guard`) —
every scheduling probe built for it (uniform forced pause,
state-synchronized racer) had survived, because any pause placed at the same
point in every writer delays them all equally. DISCLOSURE half: MCP already
replied `success:false` with the reason (the dogfood script discarded the
reply, which is why it read as silent), but every surface said the wrong
thing about a PERMANENT loss \u2014 the GPUI toast said "Undo/redo failed"
(the press did not work) and MCP said "Undo skipped (stale)" (try again).
All three now read `holon_api::undo_step_dropped_detail`, "history step
dropped \u2014 this edit can no longer be undone (<reason>)", with the GPUI
toast on its own `DegradedKind::UndoStepDropped`; the ROUTING (not just the
words) is pinned by `undo_disclosure`, whose test reds when the kind is
reverted. Evidence:
`docs/Testing/fixture-logs-2026-08-08/task29-undo-entries-dropped-keystroke-race.txt`)

## Missing piece

Every rung awaits each write before issuing the next, so no harness can put
two writes to one block in flight — the failing timing does not exist in the
test environment (re-triaged from the pass's proposed ORACLE: the oracle
fired on the first case once the interleave existed). Missing piece = a
concurrent-write rung for one entity; residual ORACLE = no invariant states
"every edit the user made stays undoable".

## Remedy

**FIXED in-lane 2026-08-08 (task #29), scope widened round 2.**
`EntityWriteLocks` (`crates/holon/src/api/operation_engine.rs`) makes
capture-prior/write/journal one step per entity — 64 fair
`tokio::sync::Mutex` stripes keyed by (entity, id) — and BOTH compounds now
take the stripe of the block they rewrite (`target` / `canonical`) across
planner read, constituents and composite push; one hold at a time,
constituents still dispatch directly, so nothing nests and no lock order is
needed. `replay` takes the same lock. Red-first:
`undo_concurrent_keystrokes.rs` (differential against the same typing done
one awaited write at a time) 3/3 red with the production signature;
`undo_compound_interleave.rs` red for each compound, killed by removing that
compound's own stripe alone. M1 (no lock) kills the singles tests; M2
(journal outside the hold) NO LONGER COMPILES —
`journal_step(guard.as_ref(), entry)` makes the guard the evidence, after
every scheduling probe for it survived. Residual: only the rewritten block
is held, not a compound's other targets. Disclosure:
`holon_api::undo_step_dropped_detail` is read by the GPUI toast (its own
`DegradedKind::UndoStepDropped`), the dispatch journal and the MCP reply;
the routing is pinned, not just the words. Evidence
`docs/Testing/fixture-logs-2026-08-08/task29-undo-entries-dropped-keystroke-race.txt`.
