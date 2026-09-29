---
id: 2026-09-29-empty-quote-block-panics-a-debug-build
date: 2026-09-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A file with an empty `#+begin_quote`/`#+end_quote` (or center) block panicked
  the org parser in every debug build (tests, dev app): orgize's
  `debug_assert!(!input.is_empty())` in `element_nodes`.
---

## Bug
Found by the org-faithful group B r10 lane: a render read-back of a quote block
panicked in `inline_marks::parse_inline`. The same input authored in Emacs
panics `parse_org_file`. A release build reads it as org does.

## Root cause
orgize parses the contents of a greater block with `element_nodes`, whose
debug assertion refuses empty input.

## Missing piece
No generator writes an empty greater block.

## Remedy
`Cargo.toml`: `[profile.dev.package.orgize] debug-assertions = false`, so a
debug build reads like a release build. The assertion belongs in the orgize
fork (holon-space/orgize); removing it there is the lasting fix. Pinned by
`org_reads_as_emacs_reads.rs` (`an_empty_greater_block_reads_back`), red log
`lane-logs/B10-red-empty-block.log`.
