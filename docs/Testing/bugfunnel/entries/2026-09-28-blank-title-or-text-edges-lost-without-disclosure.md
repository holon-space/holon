---
id: 2026-09-28-blank-title-or-text-edges-lost-without-disclosure
date: 2026-09-28
gap: ORACLE
secondary: null
status: PARTIAL
summary: >-
  Leading or trailing blanks of a title, a carriage return, and blank lines at
  the edges of block text were dropped on write-back with no loss reported.
---

## Bug
Found by the org-faithful group B r2 verifier (`lane-logs/B2v-probe.log`,
`lane-logs/B2v-probe2.log`).

## Root cause
The headline read-back check compared the trimmed title, and nothing compared
the body.

## Missing piece
The render check never saw the exact text the block meant.

## Remedy
`check_block_reads_back` (`crates/holon-org-format/src/models.rs`) parses the
block's written org text with the parser's own section reader and compares
the exact title and body. Text the file cannot hold is a loss. Pinned by
`org_text_reads_back_as_written.rs`.

## Still open
Trailing blanks on a headline line are still dropped on write-back with no
loss reported. See
[2026-09-30-headline-trailing-whitespace-dropped-undisclosed](2026-09-30-headline-trailing-whitespace-dropped-undisclosed.md).
