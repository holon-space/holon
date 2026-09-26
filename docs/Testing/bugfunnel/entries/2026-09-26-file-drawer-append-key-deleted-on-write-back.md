---
id: 2026-09-26-file-drawer-append-key-deleted-on-write-back
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A file-level drawer line whose key ends in `+` (`:note+: v`) was deleted
  from the file on write-back, because the renderer applied the headline
  drawer's key rule to the file drawer, whose own reader keeps the `+`.
---

## Bug
Found by the adversarial verifier of org-faithful group A, round 2
(`lane-logs/groupA-r2-verify.md`, defect 1): `render(parse(t))` of
`:PROPERTIES:\n:note+: v\n:END:` at file level dropped the line; the round-1
tree kept it. A regression of round 2, warn-only. The live vault has 0 such
keys.

## Root cause
One key type (`DrawerKey`) served three carriers. Its rule matched orgize's
headline reader, which reads `note+` as `note`. The file-level drawer is read
by Holon's own `parse_drawer_line` (`crates/holon-org-format/src/parser.rs`),
which keeps `note+`; `render_document_header`
(`crates/holon-org-format/src/models.rs`) still used the headline rule and
dropped the line. Source header arguments had the same mismatch in the other
direction (`Id`, `a:b`, `x+` read back fine there but were dropped).

## Missing piece
The codec PBT asserted "not written" for every non-plain key on every
carrier, so it pinned the wrong behavior instead of asking each carrier's
reader.

## Remedy
`ValueCarrier::key` gives each carrier its own key rule; the file-drawer rule
calls `parse_drawer_line` itself and the header-argument rule calls
`parse_header_args_from_str`. The engine applies the file-drawer rule to
`file_properties`. Pinned by `an_authored_file_drawer_is_written_back_byte_equal`
and the per-carrier oracles in
`crates/holon-org-format/tests/drawer_value_codec_pbt.rs`, and by
`a_file_drawer_key_ending_in_plus_is_written`
(`crates/holon-integration-tests/tests/editing_suite/drawer_key_write_boundary.rs`).
Red: `lane-logs/groupA-r3-red-orgformat.log`, `lane-logs/groupA-r3-red-engine.log`.
