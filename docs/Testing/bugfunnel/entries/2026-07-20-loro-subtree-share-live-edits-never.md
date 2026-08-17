---
id: 2026-07-20-loro-subtree-share-live-edits-never
date: 2026-07-20
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Loro subtree share: live edits never sync — `fork_at on shallow docs`
  unimplemented breaks share.project SQL projection on both peers
  (retention=none is only mode)
source_line: 1045
---

## Bug

Loro subtree share: live edits never sync — `fork_at on shallow docs`
unimplemented breaks share.project SQL projection on both peers
(retention=none is only mode)

## Missing piece

keystone has no two-peer P2P share+shallow-doc projection path; no invariant
on share→accept→edit→converge

## Remedy

FIXED+WOVEN 2026-07-21 — Holon loro fork (branch fork-at-shallow,
shallow_snapshot.rs) implements real shallow encode_snapshot_at for
at/after-shallow-root (before-root stays loud); Holon pinned to fork
nightscape/loro@4d179c68 via [patch.crates-io]. Restores the per-share SQL
projection worker (loro_share_backend.rs:362). Pinned by
fork_at_watermark_on_shallow_recipient_doc; holon-loro 193/0. Verifier
CONFIRMED 5/5; dogfood-round3 booted the fork chain clean. N2 live
two-instance retest pending
