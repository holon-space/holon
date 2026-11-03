---
id: 2026-11-03-cut-paste-of-a-copied-block-raises-unedited-two-files-disclosure
date: 2026-11-03
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  After an edit of a block copy and a cut-paste, the store raises
  block-in-two-files where the model expects block-edited-in-two-files, and a
  duplicate-id ERROR quarantines a journal file.
---

## Bug
The composed keystone went red in a random draw (seed 17253210760330187199)
during the settle A/B population runs. First panic, harness.rs:2231, after
`EditBlockCopy` and then `FinishCutPaste`:
`inv-conditions-match-ref`: "1 expected disclosure(s) are NOT in effect:
block-edited-in-two-files on ... `block:archive-51`, naming the files
[decision-page.org, Journals.org]". The store raised `block-in-two-files` for
`block:archive-51` with the same two files instead. In the same transition,
`inv-no-observed-errors` reports the ERROR "DUPLICATE BLOCK ID: 'block:kd-1'
is declared by BOTH decision-page.org and Journals.org", which quarantines
Journals.org from write-back (file_sync_controller.rs:3033). The run then ends
in a nextest TIMEOUT while it shrinks. The same seed is red on the base tree
(24ef65bd) and on the settle change with the same first panic, so the settle
change did not cause it.

## Root cause
Not found. Not root-caused in the lane that found it: the failing path is the
org ingest of a block in two files (holon-filesystem), outside that lane's
files. Open question: is the model's "edited" expectation or the store's
disclosure kind the wrong one.

## Missing piece
The keystone can generate the sequence and its invariants fire, but no gate
run had drawn it: the gate's draws did not reach an edited block copy followed
by a cut-paste that moves a second block id into the same file.

## Remedy
Open. Replay `PROPTEST_RNG_SEED=17253210760330187199` on the composed keystone
and root-cause the first panic (harness.rs:2231): first decide which
disclosure kind is right for an edited copy after the cut-paste, then why
`block:kd-1` is declared in two files.
