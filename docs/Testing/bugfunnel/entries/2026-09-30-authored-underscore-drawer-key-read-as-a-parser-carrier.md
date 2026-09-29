---
id: 2026-09-30-authored-underscore-drawer-key-read-as-a-parser-carrier
date: 2026-09-30
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  An authored drawer line whose key starts with `_` shared the property bag
  with Holon's own `_` keys: `:_drawer_raw: nonsense` panicked the org parse,
  and `:_note: keep me` (and every other `_` key) was deleted on write-back
  with no loss. Org reads both as ordinary properties.
---

## Bug
Found by the org-faithful group B r10 verifier (`lane-logs/groupB-r10-verify.md`,
D1 and D2). `* H\n:PROPERTIES:\n:ID: h\n:_drawer_raw: nonsense\n:END:\n`
panicked at `models.rs` `canonical_drawer` (`.expect("the parser writes a
readable carrier")`). `:_note: keep me` was dropped by the first write-back,
`losses=[]`. Emacs reads `_NOTE=keep me` (`lane-logs/B11-emacs-drawers.log`,
shapes b11, b12).

## Root cause
The parser stored every authored drawer key flat in the block's property bag
under its own name. The bag reserves `_` keys for Holon (parser carriers,
`_provenance`, routing metadata): the renderer read an authored
`_drawer_raw` as the carrier, and `drawer_properties()` left every `_` key out
of the drawer it rebuilds.

## Missing piece
No generator or test wrote a drawer key that starts with `_`; the keystone's
render fixed point only renders generated stores.

## Remedy
`AuthoredKey` (`drawer.rs`) separates the namespaces by type: an authored key
that starts with `_` or `\` is stored behind a `\` (`:_note:` is the property
`\_note`), and `drawer_properties()` / `block_params` map it back. No file can
write a carrier. `canonical_drawer` returns an error in place of its `expect`.
Pinned by `org_reads_as_emacs_reads.rs::an_underscore_drawer_key_is_a_property`
(red: `lane-logs/B11-red.log`).
