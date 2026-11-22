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
The adapter refusal covered only line shapes that break drawers and blocks, not a keyword line inside one. The generator reached the shape rarely, so the red looked intermittent. A hand-written model of the org line classes later missed `#+BEGINX:` keys and drawers inside source blocks, and refused lines org keeps in verse and comment blocks, because the generator drew only the key spellings the model named.

## Remedy
`holon_org_format::refuse_unreadable_content` decides by the org parser itself: it renders the block text, parses it back, and refuses the write as `Unencodable`, naming the first line whose replacement by plain text makes the text read back. The hand model in `holon-api` is deleted. The PBT draws keyword keys of any spelling and drawers and blocks of every kind nested in each other, with a ground-truth arm: a write reads back equal, or is refused naming a line org changes, and a refused text that org keeps fails the property. Static examples pin the cases the generator found.
