---
id: 2026-09-30-headline-trailing-whitespace-dropped-undisclosed
date: 2026-09-30
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  Trailing whitespace on a headline line is dropped on write-back of an
  unedited file, and no loss is reported.
---

## Bug
Found by the org-faithful group B r13 verifier (`lane-logs/groupB-r13-verify.md`,
F-2; probe output in `lane-logs/B13v2-gate3b.log:89-108` and
`lane-logs/B13v2-gate4.log:37-95`). Each of these headlines is written back
without the trailing blank, with `losses=[]`, in an unedited file:
`* title ` (one space), `* title  ` (two spaces), `* title<TAB>`,
`* title :tag: `, `* TODO title `, and `*  ` (written as `* `).

Emacs reads the title as `title` in every case (`lane-logs/B13v2-emacs.log`,
`emacs -Q --batch` 30.2). The read is faithful. The defect is the silent byte
change of an unedited file.

The defect exists on main too. The fix in
[2026-09-28-blank-title-or-text-edges-lost-without-disclosure](2026-09-28-blank-title-or-text-edges-lost-without-disclosure.md)
covered only part of it.

## Root cause
Not yet analysed. The headline line is written from the parsed title, which
carries no trailing blank, and the read-back check does not compare the
headline line itself.

## Missing piece
No invariant compares the headline line's trailing blanks. The generator in
`crates/holon-org-format/tests/deep_authored_text_never_aborts.rs:94` applies
`.trim_end()` to the generated title, so the shape is never drawn.

## Remedy
Open. Either write the trailing blank back, or report it as a loss. Remove the
`.trim_end()` from the generator.
