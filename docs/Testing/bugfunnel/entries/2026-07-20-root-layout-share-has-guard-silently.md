---
id: 2026-07-20-root-layout-share-has-guard-silently
date: 2026-07-20
gap: PERCEPTION
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Root-layout share has no guard — silently succeeds, wraps whole layout under
  mount, collapses UI to blank (mass-truncation tripwire saves disk file)
source_line: 1047
---

## Bug

Root-layout share has no guard — silently succeeds, wraps whole layout under
mount, collapses UI to blank (mass-truncation tripwire saves disk file)

## Missing piece

no guard rejecting share_subtree(root-layout); no test provokes
layout-wrapping share

## Remedy

FIXED+WOVEN 2026-07-21 — interim id-based structural_share_rejection rejects
share_subtree(root-layout) + default-doc root before any tree work, enriched
Err. Pinned by share_rejects_root_layout_block +
share_rejects_default_doc_root. Layout DESCENDANTS not yet guarded —
deferred to ADR 0028 C3. Verifier CONFIRMED
