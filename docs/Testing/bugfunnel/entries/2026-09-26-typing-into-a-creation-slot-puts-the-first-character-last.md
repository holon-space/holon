---
id: 2026-09-26-typing-into-a-creation-slot-puts-the-first-character-last
date: 2026-09-26
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A user who types more than one character into an empty page's creation slot
  gets the text stored with its first character moved to the end: `a milk`
  becomes `milka`, `? milk` becomes `milk?`.
---

## Bug
User-visible production defect. Found while measuring the read cost of a draw
that births a block with keyword-headed text (lane `questions`, round 4, item
F); localized by the verifier (`lane-logs/inc1d-verify.md`, section "F —
PRODUCTION bug"). A probe case copied
`a-jump-into-an-empty-page-seats-the-slot-and-the-first-keystroke-births` and
changed only the typed text. Every multi-character draw diverged from the
reference on the new block's content on every layer (viewmodel, Loro,
block_raw, org), so the wrong bytes are really stored:

| Typed | Stored |
|-------|--------|
| `? milk` | `milk?` |
| `a milk` | `milka` |
| `TODO milk` | `ODO milkT` |
| `? m` | `m?` |

Logs: `lane-logs/inc1d-birth-probe.log`,
`lane-logs/inc1d-birth-probe-probe-birth-{a,todo,qq}.log`. The single-character
case (`z`) stays green. The same path serves a real user, and slower typing
gives the newborn's editor more time to mount, so it cannot hide the bug. A
typed question or task keyword is also scrambled, so the block is never
promoted.

## Root cause
Every edit resolves its target through `ReactiveEngine::caret_block_for_edit`
(`crates/holon-frontend/src/reactive.rs:4617`). A `Caret::Slot` resolves by
calling `birth_creation_affordance` (`reactive.rs:3066-3130`), which creates
the node with empty content and arms the caret at offset 0
(`reactive.rs:3099`, `set_focus_with_caret(id, 0)`). The first character is
applied, then the newborn's editor mounts, reads the armed seed of 0, and puts
the caret in front of that character. Every later keystroke is inserted ahead
of it; leading whitespace is then trimmed, which is why `? milk` loses its
space.

## Missing piece
The only hand-authored birth-by-typing case types one character, so no
keystroke ever follows the birth.

## Remedy
OPEN, routed to another lane. Red first: a hand-authored case that types
`a milk` into a seated slot, then seat the newborn's caret after the character
that birthed it.
