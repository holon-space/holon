---
id: 2026-09-30-headline-trailing-whitespace-dropped-undisclosed
date: 2026-09-30
gap: ORACLE
secondary: COVERAGE
status: FIXED
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
The parser read the headline line into title, tags, keyword and cookie, and
kept nothing of the blanks after them. The renderer wrote the line from those
parts only.

## Missing piece
No invariant compared the headline line's trailing blanks. The generator in
`crates/holon-org-format/tests/deep_authored_text_never_aborts.rs` applied
`.trim_end()` to the generated title, so the shape was never drawn.

## Remedy
The parser keeps the blanks as the carrier `_headline_end`
(`headline_line_end`, `crates/holon-org-format/src/parser.rs`). The renderer
writes them at the end of the headline line (`headline_end`,
`crates/holon-org-format/src/models.rs`); a carrier that is not only spaces
and tabs is a disclosed loss. Pinned by
`a_headline_line_keeps_its_trailing_blanks`
(`crates/holon-org-format/tests/unedited_file_keeps_its_bytes.rs`, red
`lane-logs/r14-f2-red.log`, green `lane-logs/r14-f2-green.log`). The
generator no longer trims the title.
