---
id: 2026-11-22-decision-rationale-keyword-line-lifted-by-org
date: 2026-11-22
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A decision rationale holding a `#+KEY:` line inside a drawer or block was stored, but org read the line as a keyword and the decision read back with different text.
---

## Bug
The decision adapter PBT went red about once in 17 runs on main, shown as a withdrawn decision whose org render was not a fixed point. Found by the PBT run itself (intermittent red), not by dogfooding.

## Root cause
`keywords_within` in `crates/holon-org-format/src/parser.rs` lifts a `#+KEY:` line inside a drawer or block into a keyword carrier. The comma-escape codec (`Context::Kept`) does not escape it by ruled design. The adapter `body()` in `crates/holon-api/src/decision_block.rs` accepted such a rationale line, so write and read-back differed.

## Missing piece
The adapter refusal covered only line shapes that break drawers and blocks, not a keyword line inside one. The generator reached the shape rarely, so the red looked intermittent.

## Remedy
`body()` now refuses such a line as `Unencodable`, naming the line. The PBT property is: a write reads back equal or is refused naming the offending line. Static examples pin the refusal and the withdrawn-decision fixed point.
