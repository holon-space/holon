---
id: 2026-08-17-editor-drops-external-write-with-no-write-seq
date: 2026-08-17
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  A focused editor silently drops any external content change whose row
  carries no write_seq token, 29 times in one real-vault session.
---

## Bug

Found by log analysis of `/private/tmp/holon-cold.log` (2026-08-17, real
vault, ~1930 blocks). 29 occurrences starting line 6000, e.g. `data-sync
echo has no write_seq column; dropping (schema/projection regression)
row_id=Some("block:e77dcf00-...") new=DONE Denis nach...` — a task-state
toggle (`new=DONE`) landing on a block while the editor held it open. The
external change is logged and discarded; the editor's visible buffer stays
on the pre-change content.

## Root cause

`EditorViewModel::converge_from_data_sync`
(`crates/holon-frontend/src/editor_view_model.rs:539-563`) runs
`evaluate_data_sync_echo` (`crates/holon-frontend/src/echo.rs:53-90`) against
every SqlOnly data-sync row for the focused block. When the row's content
differs from the buffer AND carries no `write_seq` (`echo_seq: None`), the
function returns `EchoDecision::DropNoSeq` — by design (its own doc comment:
"never converge blindly: that is the stale-echo data loss we are
preventing"), because ordering can't be proven. `echo.rs:73` documents that
non-editor writers (split/join/org, and evidently task-state-cycle writes)
"do NOT bump write_seq" — so ANY external structural change landing while a
block is focused hits this branch and is dropped, not merely a malformed
edge case.

## Missing piece

COVERAGE: the pure decision function IS unit-tested (`echo.rs:276` pins
`DropNoSeq` for a synthetic no-seq input), so the LOGIC is covered. But the
INTEGRATION scenario — a real non-editor writer producing a genuinely
seq-less row while the keystone's driver holds a block focused — is not:
the one keystone transition that models this shape,
`external_write_same_block_focused.rs`, states in its own header comment
that non-editor writers don't bump `write_seq` but reaches the
`Converge`/`AdoptBaseline` arms, meaning its generated external write still
carries SOME `echo_seq` value (not a true `None`). No transition drives a
genuinely-null-`write_seq` external write against a focused block, so this
exact failure mode — real content silently lost from the editor's view — is
structurally ungeneratable by the composed keystone today. ORACLE secondary,
weaker: even if generated, no invariant currently asserts "a focused
editor's buffer eventually reflects an external structural write" (only the
narrower `DropStale`/`AdoptBaseline` correctness is pinned at the unit
level).

## Remedy

NOT FIXED. Two independent gaps, either closes half: (1) extend
`external_write_same_block_focused.rs`'s generator to occasionally produce a
write through a path that leaves `write_seq` genuinely NULL (matching
task-state-cycle / org-ingest writes), so the keystone can reach
`DropNoSeq` end-to-end; (2) decide the actual product remedy — is silently
dropping the external change and leaving the editor stale the INTENDED
behavior (safer than corrupting an in-flight edit), or should there be a
visible "this block changed externally, reload?" affordance instead of a
log-only signal the user never sees? That's a product decision, not
something to guess at here.
