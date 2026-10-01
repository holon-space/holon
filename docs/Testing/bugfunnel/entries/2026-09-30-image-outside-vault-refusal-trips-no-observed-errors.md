---
id: 2026-09-30-image-outside-vault-refusal-trips-no-observed-errors
date: 2026-09-30
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  The file-sync controller refuses to materialize an image outside the vault
  root and disclosed the refusal at ERROR, which the keystone's
  `inv-no-observed-errors` reads as a swallowed failure. The refusal is now
  disclosed at WARN.
---

## Bug
The keystone's image-path alphabet draws traversal paths (`../escape.png`) on
purpose. A run that draws one failed with `[inv-no-observed-errors] 1 swallowed
problem(s) (ERROR log / panic)`, the ERROR being `[FileSyncController] refusing
to materialize an image OUTSIDE the vault root`. First seen in the decision
Inc 7 round-4 verifier's `just pbt general 12` run 3.

## Root cause
Two written contracts disagreed about the level. `holon-filesystem` disclosed
a permanent, per-block refusal at ERROR. The test tracing contract
(`crates/holon-integration-tests/src/test_tracing.rs`, `ProblemKind`) reads
ERROR as a swallowed failure and WARN as a visibly disclosed degradation. The
refusal is correct and disclosed, so it is a WARN. The product was wrong, not
the invariant.

## Missing piece
No deterministic case named a traversal image. Only a random draw hit it, and
its shrink moved to an unrelated red, so the family was never pinned.

## Remedy
`file_sync_controller.rs` logs the first refusal per block at `tracing::warn!`
(message and fields unchanged). The hand-authored case
`image-outside-vault-is-disclosed-at-warn-not-error` writes four org files, each
with an image `../escape.png`. Four files, because the first write-back can be
skipped by a startup membership race, which hides the refusal; one file was red
in about half of the runs, four files red in 6 of 6.

Evidence: `lane-logs/row4-red.log` (RED before the fix, ERROR line, exit 101)
and `lane-logs/row4-green.log` (GREEN after, WARN line, exit 0) in the
`row4-warn` workspace.
