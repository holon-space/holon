---
id: 2026-08-08-write-path-e2e-latency-clock-cannot
date: 2026-08-08
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The write-path e2e latency clock cannot close for a tokenless op on a block
  that was ever typed into, and it fails silently
source_line: 1184
---

## Bug

(task-#37 exploratory profiling lane, found by READING the correlator while
re-measuring SplitBlock windowed) **The write-path e2e latency clock cannot
close for a tokenless op on a block that was ever typed into, and it fails
silently** — so `split_block`/`indent`/`outdent`/`join` produced `dispatch`
samples and ZERO `e2e` samples on any block the user had typed into.
`block_raw.write_seq` is stamped only by editor content writes and is STICKY
ON THE ROW, so the split's own delivered CDC row arrives carrying the stale
editor token: `close_delivered` rule 1 finds no pending entry holding it,
rule 2 needs a tokenless row for the target and the batch has none (the new
child's `parent_id` names the PARENT, not the split target), so nothing
closes; the entry expires and is pruned in silence, because only `navigate`
warned on expiry. Proven windowed: 8 typed-gesture splits → 8 dispatch / 0
e2e; a single-block probe twice on two instances shows `write_seq` 0 → type
→ 2 → Enter → `dispatch split_block ms=37`, no `e2e`, `write_seq` still 2
for 24s. The instrument therefore reported structural-op latency ONLY from
never-typed blocks, and any sample that did close, closed on a later
unrelated delivery — which is how the phantom 803/944/975ms `split_block`
figure entered this ledger.

## Root cause

found by an exploratory profiling lane (task #37) reading the correlator
while re-measuring SplitBlock, not by any test — **the write-path e2e clock
cannot close for a tokenless op on a block that was ever typed into, and the
failure is SILENT**. `block_raw.write_seq` is stamped only by editor content
writes and is sticky ON THE ROW, so every later projection of that row
repeats it. `split_block`/`indent`/`outdent`/`join` carry no token, so their
own delivered row arrived as `(target, Some(stale_seq))`: `close_delivered`
rule 1 found no pending entry holding that token, rule 2 required a
*tokenless* row for the target and the batch had none (the new child's
`parent_id` names the PARENT, not the split target), and rule 3 closed
nothing. The entry then expired and was pruned in silence — only `navigate`
warned on expiry (`latency_e2e.rs:151`). Proven windowed: 8 typed-gesture
splits → 8 `dispatch` samples, 0 `e2e` samples; single-block probe twice on
two instances (`write_seq` 0 → type → 2 → Enter → `dispatch split_block
ms=37`, no `e2e`, `write_seq` still 2 for 24s). CONSEQUENCE: every `e2e`
figure for a structural op is drawn from never-typed blocks only, and any
sample that did close, closed on a later unrelated delivery — which is what
put the phantom 803/944/975ms `split_block` row (annotated above) into this
ledger and into the latency-ceiling discussion. Classified ORACLE per this
skill's latency carve-out: the keystone can generate type-then-split freely,
the correlator runs byte-identically in test and prod (so not ENVIRONMENT),
and nothing asserted that a dispatched interaction yields an e2e sample at
all — the instrument was measuring nothing and no invariant noticed. FIXED
in this lane: an anonymous delivery (only tokens no pending entry is waiting
for) no longer vetoes a tokenless entry's closure, and EVERY expiry is
disclosed as `stage="e2e_expired"`. Evidence: task-#37 lane report §3
(`docs/Testing/fixture-logs-2026-08-08/task37-windowed-latency-report.txt`),
`docs/Testing/fixture-logs-2026-08-08/latency-correlator-typed-gesture-8-splits.txt`
(the 8-typed-vs-8-untyped split asymmetry, verbatim),
`.../latency-correlator-probe-typed-split-no-e2e.txt`; red/green
`lane-logs/task10-red-correlator.txt` / `task10-green-correlator.txt`)

## Missing piece

The keystone generates type-then-split freely and the correlator runs
byte-identically in test and prod (so not ENVIRONMENT); what was missing is
any assertion that a dispatched interaction yields an e2e sample at all — an
instrument that measures nothing looks exactly like an instrument that
measures fast. Missing piece = correlation-completeness assertions at the
correlator's own unit surface (now added:
`structural_op_closes_over_a_stale_row_token`,
`typed_then_split_the_same_block_both_measure`,
`expired_write_entry_is_disclosed`), and, at keystone level, a check that
every dispatched write transition eventually yields an `e2e` sample or a
disclosed expiry.

## Remedy

FIXED 2026-08-08 (task #10, `crates/holon-api/src/latency_e2e.rs`). An
ANONYMOUS delivery — a batch whose only tokens for the target are ones no
pending entry is waiting for — no longer vetoes a tokenless entry's closure,
because such a token is the sticky row column, not evidence about the op in
flight; a TOKENFUL entry still waits for its own token, which preserves the
2026-07-13 phantom-steal protection. Every expiry is now disclosed
(`stage="e2e_expired" action=… block=… waited_ms=…`), not only `navigate`'s.
ADVERSARIAL VERIFICATION found one new phantom path and it is CLOSED at the
source: rule 3 also lets a STALE TOKENLESS entry — a REFUSED op, which
writes nothing — be closed by any unrelated later delivery for that row
(probe: a refused `outdent` closing 25s later as `closed=[("outdent",
25000)]`, `lane-logs/task10-probe-h-phantom-red.txt`), which would
false-fire the latency-slo oracle. `latency_e2e::interaction_failed` now
retires the entry from the `Err` arm of all four op-dispatch seams
(`holon-frontend/src/operations.rs`, `reactive.rs` dispatch_intent / _sync /
_awaitable), matching action AND target and emitting `stage="e2e_retired"`;
refusals surface as `Err` at every one of those seams, so the seam is
reachable without new plumbing. REMAINING LIMITATION: an op that SUCCEEDS
but writes no delta (an identity re-commit of a TOKENLESS op) still leaves
an entry that a later unrelated delivery can close — the dispatch seam
cannot see "produced no CDC delta", only "returned Err". Red/green:
`lane-logs/task10-red-correlator.txt` (3 reds: 0-vs-1 closures ×2,
disclosure list `["navigate"]`) → `task10-green-correlator.txt` 13/13.
Residuals, NOT fixed here: the `navigate` clock-bleed is a different
mechanism (it closes on any child row of the page) — FIXED 2026-08-08 by
task #13, which made the block-row/focus-root distinction explicit on both
ends (`Observable`); and a split dispatched while an editor write on the
same block is still pending can still lose that batch to the exact-token
match. Evidence: task-#37 report §3
(`docs/Testing/fixture-logs-2026-08-08/task37-windowed-latency-report.txt`).
