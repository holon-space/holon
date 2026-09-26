---
id: 2026-09-26-carrier-id-bypasses-the-drawer-codec
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A multi-line `ID` in a drawer carrier was written raw onto the `:ID:` line:
  the drawer broke, the engine accepted the write, and write-back of the whole
  file stopped (quarantined) or wrote a corrupt drawer, depending on the route.
---

## Bug
Found by the adversarial verifier of org-faithful group A
(`lane-logs/groupA-verify.md`, defect 1). `set_field org_properties
{"ID": "n-target\n* Evil heading\n:PROPERTIES:\n:ID: hijack\n:END:", "note": "keep"}`
returned `Ok`; `notes.org` did not change and never got `:note: keep`. Through
the `properties` bag the same id reached the file and broke the drawer
(`lane-logs/groupA-r2-red-engine-id.log`).

## Root cause
`format_properties_drawer` (`crates/holon-org-format/src/models.rs`) wrote the
carrier's `ID` value verbatim; only the other drawer lines went through the
value codec. The engine key guard checked keys only, and `ID` is a legal key.
The write-back removal guard then saw the block lose its id and vetoed the
write (ERROR + quarantine in `file_sync_controller.rs`), so one value froze
the file.

## Missing piece
The codec PBT drew keys from `k[a-z0-9-]{0,6}`, and the keystone cases used a
clean `ID`, so no generated case put a hostile value on the `:ID:` line.

## Remedy
`DrawerId` (`crates/holon-org-format/src/drawer.rs`): a non-empty token that
forms a URI and is written raw. The engine refuses any other id on the `ID`
property, the `properties` bag, `org_properties` and `file_properties`
(`crates/holon-integration-tests/tests/editing_suite/drawer_id_write_boundary.rs`).
The renderer writes the block's own id with a warning when a bad id reaches
it past the engine; the rest of the file is written
(`a_bad_carrier_id_past_the_engine_does_not_freeze_the_file`). The codec PBT
key generator now reaches `ID`, its case variants, `x+` and the delimiters.
The visible disclosure of that warning (`WritebackDegraded`) is group B work.
