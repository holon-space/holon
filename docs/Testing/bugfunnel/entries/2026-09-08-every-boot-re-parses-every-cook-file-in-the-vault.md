---
id: 2026-09-08-every-boot-re-parses-every-cook-file-in-the-vault
date: 2026-09-08
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The controller's cold-boot content-hash skip is structurally unreachable for
  any format that embeds no id in its content, so every `.cook` file in the
  vault is re-parsed on every boot — through the wasm interpreter, at ~20x the
  native cost, for no output.
---

## Bug

Found by reading the scan path while wiring lowcode Inc 3 (the cooklang plugin
becoming the only parser). It is a LATENCY bug against the p95
interaction→projection-visible < 200 ms SLO, not a wrong-data bug: the
projection is correct, it is just recomputed from scratch every time.

`FileSyncController` has a cold-boot fast path that skips ingest entirely when
a file's disk hash matches the hash the last ingest stamped
(`crates/holon-filesystem/src/file_sync_controller.rs`, the
`stored == &disk_hash && self.content_present_in_all_stores(root)` skip). Org
files reach it. `.cook` files could not, and neither could any future plugin
format.

## Root cause

The skip has two obligations — prove the content is present in every active
store, and record where the document lives — and both need the document's
identity. That identity was resolved in ONE way: `doc_id_from_content`, which
reads an id embedded in the file (org's `#+ID:`).

A cooklang file embeds no id, and it structurally cannot: an ADR 0034 plugin
guest is a pure function over bytes with no ambient authority, so it can mint
nothing stable. `doc_id_from_content` therefore returned `None`, `disk_root`
was `None`, and the `if let (Some(stored), Some(root))` guard fell through to
the full ingest — every file, every boot.

The cost was invisible while the parser was native Rust. Inc 2a measured the
wasm interpreter at ~20x native (D90.a accepted that ratio on the premise that
a scan re-parses only what changed), which is what turns a redundant parse into
a budget problem.

## Missing piece

Every existing cook test parses ONCE. `crates/holon-kitchen/tests/cook_ingest.rs`
(now `crates/holon-plugin-host/tests/cook_ingest.rs`) drives the adapter
directly, and `crates/holon-integration-tests/tests/cook_vault_ingest.rs` booted
a real vault exactly one time. Nothing booted twice over the same store, so the
second scan's cost was never observable — and no counter existed that a test
could have read even if it had.

## Remedy

OPEN. Lowcode Inc 3 landed the covering test and half the mechanism, and
MEASURED why the other half does not work yet.

Landed:
- `FileFormatAdapter::document_identity()` states which kind a format is —
  `DocumentIdentity::Embedded` (org, the default) or `ByRecordedHome` (every
  plugin format). The skip now asks the right question instead of re-deriving
  an answer from a `None`.
- `BlockReader::load_file_hashes` → `load_file_projections`, returning
  `FileProjection { content_hash, document_id }`, so one boot query carries
  both facts about a file row.

Also landed (measured green):
- `BlockReader::persist_file_hash` → `persist_file_projection`, an UPSERT that
  writes `content_hash` AND `document_id`. The old UPDATE matched no row for a
  `.cook` file, because `OrgModeSyncProvider` creates rows for org files only —
  so nothing about the file survived a boot. Pinned by
  `a_cook_file_records_its_document_on_the_file_row`.
- `probe_share_file` returns `Ordinary` for a `WriteTier::ReadOnly` file
  without parsing it. A shared-subtree projection is something Holon rendered;
  authoritative input cannot be one. Measured A/B (3 runs per side,
  deterministic): a read-only file whose bytes trip the probe's `share-role`
  pre-filter costs 3 guest parses per boot without the guard and 1 with it. It
  saves nothing for a file that does not carry those substrings. Pinned by
  `a_read_only_file_is_never_parsed_by_the_share_probe`.

Still needed:
1. The second boot still re-parses a VARYING number of recipes (measured 6/4/5/5/7
   over five identical runs, expected 1), with the skip's `content_present_in_all_stores`
   probe answering inconsistently for the same root. Tracked separately as
   `2026-09-08-a-cook-vaults-second-boot-re-parses-a-varying-number-of-recipes`.

Covering test (present, `#[ignore]`d as a known red until 1 above lands):
`crates/holon-integration-tests/tests/cook_vault_ingest.rs::a_second_boot_re_parses_only_the_recipe_that_changed`
— boots a vault of three recipes, stops the app, rewrites ONE file, boots
again, asserts `holon_plugin_host::guest_parses()` advanced by exactly 1 and
the untouched recipe's blocks are byte-identical. Red log:
`lane-logs/a2c-incremental.log` ("the second boot ran the guest 6 times").
