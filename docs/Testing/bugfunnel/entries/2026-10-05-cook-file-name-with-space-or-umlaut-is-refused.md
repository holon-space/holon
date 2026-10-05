---
id: 2026-10-05-cook-file-name-with-space-or-umlaut-is-refused
date: 2026-10-05
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A `.cook` file whose vault-relative path holds a space or a non-ASCII
  letter is refused whole ("derived recipe id ... is not a storable URI
  path"), so about 73% of a real German recipe vault never ingests.
---

## Bug
Found by Martin dogfooding 2026-10-05: he dropped 4352 `.cook` recipes into
`Resources/Rezepte/` of his vault. In the live log (dev build, first 27 min of
the boot scan), 441 of the 604 recipe files scanned were refused with
`derived recipe id "<path>" is not a storable URI path. Rename the file to one
the id grammar admits.` Nothing of a refused file is ingested.

## Root cause
The cooklang guest uses the RAW vault-relative path as the recipe row id and
inside the ingredient-use ids (`guests/cooklang/src/lib.rs:68`, `:112`,
`:128`). The host turns that local id into `recipe:<path>` and parses it as an
RFC 3986 URI (`crates/holon-rows/src/ids.rs:19-35`, called from
`crates/holon-plugin-host/src/adapter.rs:463`). A space or any non-ASCII
letter is not a URI character, so the parse fails and the whole file is
refused. The file's own `file:` id does not have this problem:
`EntityUri::file` percent-encodes each path segment
(`crates/holon-api/src/entity_uri.rs:180-192`).

Measured: a predictor "path is not made only of RFC 3986 path characters"
matched the log exactly (441 true positive, 0 false negative; 1 false
positive was the file still in flight at log end). Over the whole vault it
predicts 3182 of 4352 files refused (2669 with a space, 513 with only
non-ASCII letters). Red test:
`crates/holon-plugin-host/tests/cook_ingest.rs::a_file_name_with_a_space_and_an_umlaut_ingests`
fails with the same message (`lane-logs/cook-red-1.log:593`).

The refusal is disclosed, not silent. The adapter error returns through
`on_file_changed_unpersisted`'s `Err` branch
(`crates/holon-filesystem/src/file_sync_controller.rs`, `AdapterRefusal`) and
is recorded per file with `ConditionBus::vault_ingest_refused`
(`crates/holon-api/src/condition_bus.rs`). The bus raises ONE
`VaultIngestFailed` condition per format, not per file: its subject is the
format, and it carries the refused-file count and the first 3 paths with their
reasons (`IngestRefusals`). The GPUI shows that one condition as one toast
(`frontends/gpui/src/share_ui.rs`, `push_condition_toast`). It clears when the
last refused file of that format ingests or is gone. So the user saw one
`cooklang` toast that counted the refused recipes, and the log holds one
`QUARANTINING this file` line per refused recipe (336 in the second boot).

## Missing piece
The keystone seeds ONE recipe with a fixed ASCII, hyphenated name
(`crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs:361`,
`keystone-recipe.cook`). No generator draws a `.cook` file name with a space
or a non-ASCII letter, so the refusing path never runs.

## Remedy
FIXED. The cooklang plugin no longer joins an id string. It emits
`holon-plugin-rows` typed lines whose ids are `LocalId` path segments plus
parts. The host parses them at one point (`Stream::from_jsonl`,
`crates/holon-plugin-host/src/adapter.rs`) and renders each id with
`EntityUri::from_segments` (`crates/holon-api/src/entity_uri.rs`), which
percent-encodes every component like `EntityUri::file`. `parse_local_id`
(`crates/holon-rows/src/ids.rs`) is deleted. No stored id changes: 0 of 1170
currently accepted vault paths contain a character that encoding changes.

The gap is closed at two levels:
- Keystone: the read-only recipe file is now `Grüne Keystone-Soße.cook`
  (`wide_e2e.rs:363`). The keystone covers this only on draws that boot a
  ViewModel projection, because only that arm seeds the recipe
  (`wide_e2e.rs` read-only-homes rung). A default `just keystone-smoke` draw
  without a ViewModel does not exercise it.
- Plugin host: `crates/holon-plugin-host/tests/cook_file_names_pbt.rs`
  (`any_file_name_ingests_under_its_encoded_path`, 48 cases) draws names with
  spaces and non-ASCII letters.

## Evidence
- B red (plugin host): `lane-logs/red-b-before.log:445`, "Rezepte/Two Words.cook
  must ingest: derived recipe id ... is not a storable URI path".
- B green: `lane-logs/final-gate-unit.log:721`
  (`cook_ingest a_file_name_with_a_space_and_an_umlaut_ingests` PASS) and
  `:786` (`cook_file_names_pbt any_file_name_ingests_under_its_encoded_path`
  PASS); summary "774 tests run: 774 passed".
- E red (keystone, seed 1 draws ViewModel): `lane-logs/red-e-seeds-before/seed-1.log:15`,
  "[read-only homes] Grüne Keystone-Soße.cook did not ingest its steps".
- E green: `lane-logs/green-keystone-smoke.log` (smoke passes) and
  `lane-logs/red-e-seeds-after/seed-1*.log`: the read-only-homes rung no
  longer fires and `inv-read-only-home-refuses-writes=1/1` is engaged. Seed 1
  still ends red on other known-red families (`org-blocks-ref-diverge`; a
  `[boot journal]` budget timeout in shrink replays under machine load),
  classified in `lane-logs/green-e-classify.log`.
- Re-verified on main 33661529, where the path is refused before the guest
  runs:
  - B red on main: `lane-logs/revive-red-main-cook-ingest.log`, "Rezepte/Two
    Words.cook must ingest: derived recipe id ... is not a storable URI path".
  - E red on main (seed 4 draws `{Turso}` + ViewModel):
    `lane-logs/revive-red-main-keystone-seed4.log`, "[read-only homes] Grüne
    Keystone-Soße.cook did not ingest its steps".
  - E green with the fix, same seed: 10 of 10 runs pass
    (`lane-logs/revive-ab-seed4/lane-*.log`).
