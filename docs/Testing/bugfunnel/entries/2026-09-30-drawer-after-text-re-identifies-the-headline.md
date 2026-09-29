---
id: 2026-09-30-drawer-after-text-re-identifies-the-headline
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A headline whose `:PROPERTIES:` drawer followed text got a freshly minted
  uuid, and write-back put a new drawer above the text, with no loss: the
  headline lost the id its `:ID:` line named.
---

## Bug
Found by the org-faithful group B r10 verifier (`lane-logs/groupB-r10-verify.md`,
D3): `* H\ntext\n:PROPERTIES:\n:ID: h\n:END:\nbody\n` was written back as
`* H\n:PROPERTIES:\n:ID: <uuid>\n:END:\ntext\n:PROPERTIES:\n:ID: h\n:END:\nbody\n`,
`losses=[]`. Org reads no id there (`ENTRY-GET-ID nil`,
`lane-logs/B11-emacs-drawers.log`, shape b16).

## Root cause
`properties_drawer` (`parser.rs`) only looks at the first section child, so
this drawer gave no id and `extract_or_generate_id` minted one.

## Missing piece
No test put a PROPERTIES drawer after text.

## Remedy
`headline_id` reads the first `:ID:` line of a PROPERTIES drawer in the body
when the headline has none; the parser records an empty `_drawer_text`, and
the renderer (`drawer_from_body`, `models.rs`) writes no drawer while the body
still names the id, recording a loss on every render. An edited block gets a
drawer with the same id above its text. Pinned by
`org_reads_as_emacs_reads.rs::a_drawer_after_text_keeps_the_headline_id_and_is_disclosed`.
