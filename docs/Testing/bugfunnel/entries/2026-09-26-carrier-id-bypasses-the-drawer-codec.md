---
id: 2026-09-26-carrier-id-bypasses-the-drawer-codec
date: 2026-09-26
gap: COVERAGE
secondary: null
status: PARTIAL
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
Store to file through the engine: fixed. `DrawerId`
(`crates/holon-org-format/src/drawer.rs`) is a bare block id; the engine
refuses any other id on the `ID` property, the `properties` bag,
`org_properties`, `file_properties` and a `create`'s `block:` id
(`crates/holon-integration-tests/tests/editing_suite/drawer_id_write_boundary.rs`).
The codec PBT key generator reaches `ID`, its case variants, `x+` and the
delimiters. The bare-id rule is
`2026-09-26-schemed-or-reserved-id-accepted-on-the-id-line.md`.

A bad id that reaches the renderer past the engine (sync, peer merge,
ingest) fails the render by name on every org id carrier — heading `:ID:`,
source block `:id`, page `#+ID:` — and that file's write-back is refused with
an ERROR log while the other files of a batch pass are still written
(`a_bad_carrier_id_past_the_engine_refuses_the_write_back`,
`crates/holon-orgmode/tests/render_refusal_is_per_file.rs`).

Open:
- The refusal is not disclosed to the user yet (`WritebackDegraded`,
  org-faithful group B).
- A duplicate or case-variant `:ID:` line is dropped on read.
