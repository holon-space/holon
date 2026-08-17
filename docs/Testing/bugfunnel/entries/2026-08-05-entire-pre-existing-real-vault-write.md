---
id: 2026-08-05-entire-pre-existing-real-vault-write
date: 2026-08-05
gap: COVERAGE
secondary: PERCEPTION
status: NOTED
summary: >-
  The entire pre-existing real-vault write-back churn (27/141 files, reported
  as 6911 unstable lines) is 332 edits of ONE deliberate shape: the author's
  single blank line between a body and the next headline/src-block, dropped
  once by documented renderer policy (models.rs:1246-1265), deterministic and
  idempotent — a one-time normalization, zero data loss.
source_line: 774
---

## Bug

(Increment-C byte-stability gate → #31 investigation) **The entire
pre-existing real-vault write-back churn (27/141 files, reported as 6911
unstable lines) is 332 edits of ONE deliberate shape: the author's single
blank line between a body and the next headline/src-block, dropped once by
documented renderer policy (models.rs:1246-1265), deterministic and
idempotent — a one-time normalization, zero data loss.**

## Root cause

pre-existing real-vault write-back churn (#31) classified — 27 of 141 files
move under parse->render and the ENTIRE delta is one shape: the author's
single blank line between a block body and the next construct dropped (332
occurrences; 312 pre-headline, 20 pre-src; deliberate policy
models.rs:1246-1265 — renderer re-emits only when body_needs_list_terminator
says load-bearing). DETERMINISTIC (3 processes x 3 copies byte-identical) +
IDEMPOTENT (cycle-2/3 churn 0) = one-time normalization, cancels in any
pre/post A/B; zero data loss (headlines 1128->1128, src 35->35, :ID:
1135->1135), zero '[[' lines. GAP: synthetic corpora render bodies the
renderer's own way, so no generator produces the hand-authored blank; the
harness scored it index-wise (332 real edits reported as 6911 — #36).
Product decision preserve-vs-normalize = #37, RULED 2026-08-05: accept
normalization. Latent edge: body_needs_list_terminator inspects only the
LAST non-blank line — ordered-list item ending in an indented continuation
loses its terminator before a col-0 src block (1 occurrence, harmless
today))

## Missing piece

Synthetic corpora only contain renderer-authored spacing, so the
hand-authored blank is ungenerable; secondary PERCEPTION: the harness's
index-wise changed_lines inflated 332 real edits into 6911, burying real
regressions in shift noise (#36).

## Remedy

RULED 2026-08-05 (Martin): ACCEPT the one-time normalization — no
trailing_blank preservation metadata; the 27-file/332-line vault diff lands
on next real write-back and is stable thereafter (idempotence proven). Still
open from this row: #36 (harness index-wise diff inflation) and the latent
body_needs_list_terminator continuation-line edge.
