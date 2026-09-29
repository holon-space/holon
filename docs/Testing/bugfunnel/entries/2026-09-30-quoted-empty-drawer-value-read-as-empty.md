---
id: 2026-09-30-quoted-empty-drawer-value-read-as-empty
date: 2026-09-30
gap: ORACLE
secondary: null
status: PARTIAL
summary: >-
  An authored drawer value `""` was read as the empty string; org reads the
  two characters `""`.
---

## Bug
Found by the org-faithful group B r11 verifier
(`lane-logs/groupB-r11-verify.md`, D3v), measured with `emacs -Q --batch`
(`lane-logs/B12-emacs.log`: `:NOTE: ""` reads `"\"\""`, `:NOTE:` reads `""`).

## Root cause
The headline drawer wrote an empty value as the literal `""`, and the decoder
reads a literal that the encoder would write, so the authored `""` decoded to
the empty value.

## Missing piece
The drawer codec PBT compares Holon's writer with Holon's reader; no oracle
compares an authored value with org's reading.

## Remedy
The headline drawer writes an empty value as nothing after `:KEY:` (org reads
it as empty), so `""` is no literal Holon writes and reads as its two
characters (`crates/holon-org-format/src/drawer.rs` `survives_raw`). Pinned by
`a_quoted_empty_drawer_value_is_read_as_org_reads_it`.

## Open
The same collision remains for every value that has the shape of Holon's own
JSON literal. Org reads the raw text; Holon decodes it (measured in
`lane-logs/B13-json-values.md`, emacs -Q 30.2):

| authored | org reads | Holon reads |
|---|---|---|
| `" "` | `" "` | one space |
| `"  x  "` | `"  x  "` | `  x  ` |
| `" lead"` | `" lead"` | ` lead` |
| `"a\nb"` | `"a\nb"` | `a`, a line break, `b` |
| `"\t"` | `"\t"` | a TAB |
| `"\" \""` | `"\" \""` | `" "` |

In general: a quoted value whose content has a space or tab at the start or
end, a line break or another control character, or content that is itself
such a literal (header arguments also: whitespace inside, or a `:`-token).
The file bytes stay unchanged. The vault has no such value (0 of 4488 drawer
values start with `"`). The fix waits for Martin's ruling on the options in
`lane-logs/B13-json-values.md`.
