---
id: 2026-09-29-malformed-parser-carrier-panics-the-renderer
date: 2026-09-29
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `set_field` could write any JSON into a parser-owned carrier such as
  `_drawer_raw`; the next org render panicked on the malformed value.
---

## Bug
Found by the org-faithful group B r8 verifier: `set_field(_drawer_raw, ...)`
with a value that is not a map of strings through the operation engine, then
a write-back: the renderer panicked in the carrier's deserializer.

## Root cause
The parser-owned carriers (`_drawer_raw`, `_keyword_lines`, `_header_lines`,
`_header_places`, `_blank_lines`, ...) were writable by every engine
operation, and the renderer read them with `expect`.

## Missing piece
No generator wrote a carrier through an operation; no property put arbitrary
JSON into a carrier.

## Remedy
`org_props::PARSER_CARRIERS` lists every carrier only the parser writes.
`OperationEngine::execute_operation` refuses an operation that writes one,
by name, unless the origin is the ingest or a peer
(`reject_parser_carriers`). The renderer reads every carrier through
`read_carrier` / `carrier_or_loss`: a malformed value is skipped and
recorded as a `WritebackLossy`. Pinned by the engine test
`a_parser_carrier_is_refused_unless_the_ingest_or_a_peer_writes_it` and the
property in `org_text_reads_back_as_written.rs` (arbitrary JSON in every
carrier: no panic, a loss).

The page's file drawer `_file_properties` is the one carrier a user may
write, as its own field (`set_field`), where the engine checks every key and
id. The r12 verifier (`lane-logs/groupB-r12-verify.md`, V2) found that the
renderer still read it with a panicking parse: `garbage`, `""`, `[]`, `null`
and `{` panicked `render_document`, and a bag key `_file_properties` passed
the engine unchecked. Now `OrgDocumentExt::file_drawer` reads it through
`read_carrier`, so such a value renders the page without the drawer (the id
as `#+ID:`) and records a loss (`an_unreadable_file_drawer_carrier_is_a_disclosed_loss`),
and `reject_parser_carriers` refuses `_file_properties` inside a property bag
unless the ingest or a peer writes it
(`a_file_drawer_in_a_bag_is_refused_unless_the_ingest_or_a_peer_writes_it`).
