---
id: 2026-10-07-same-stem-org-and-recipe-merge-into-one-document
date: 2026-10-07
gap: COVERAGE
secondary: null
status: PARTIAL
summary: >-
  `Pasta.org` and `Pasta.cook` in one folder resolve to ONE document by name
  chain, so the file ingested last takes the home: either the recipe's steps
  are written into `Pasta.org`, or the org page leaves the write authority and
  every edit to it is refused.
---

## Bug
An agent probe in the org-scan-boot lane (D108.a, boot order: writable files
before read-only files) booted a vault holding `Pasta.org` and `Pasta.cook`
side by side, with and without `#+ID:` on the org file.

- Cook ingested first, then org (the old boot order, and any runtime `.org`
  created beside an ingested recipe): the recipe step was rendered into
  `Pasta.org` (`* Boil the water…` with `:ID: Pasta.cook::b::0`). Read-only
  content reached disk in a writable file.
- Org ingested first, then cook (the D108.a boot order): `Pasta.org` kept its
  bytes, but `block:pasta-note` fell out of the write authority, so every edit
  of the org page was refused ("not in the write authority").

Both `file` rows recorded the same `document_id`. Martin's vault has no
same-stem pair today; the defect is latent but corrupts user data when it
fires.

Warm-boot leg (found by the verifier osb-verify-2, probe
`probe_p1_warm_db_org_added_beside_an_already_ingested_recipe`, log
`lane-logs/osb-verify-2/p-probes-1.log`): boot 1 ingests `Pasta.cook`, the
user adds `Pasta.org` while the app is stopped, and boot 2 renders the recipe
step into `Pasta.org` on disk. `Pasta.cook` is then refused and its step
leaves the write authority. The first fix (below) did not hold here.

## Root cause
A `.cook` file embeds no id (`DocumentIdentity::ByRecordedHome`), and an org
file without `#+ID:` has none either, so both resolve their document through
`path_to_name_chain`, which strips the extension
(`crates/holon-filesystem/src/file_sync_controller.rs`, `ingest_file`, the
`None =>` arm of the document resolution). The duplicate-document refusal
(`live_claimant_of`) ran only for an id found in the file or recorded for the
path, never for a name-chain resolution, so the second file silently merged
into the document the first one homes and took its home.

Warm-boot leg: `live_claimant_of` answers only from `doc_home`, and nothing
filled `doc_home` from the persisted `file` rows at `initialize`. Under
D108.a every writable file ingests before every read-only file, so on boot 2
`Pasta.org` found no claimant and adopted the recipe's document. A/B: the
pre-D108.a order (recipe first) passes the same probe
(`lane-logs/osb-verify-2/p1-oldorder-1.log`).

Evidence: probe logs `lane-logs/probe-collision-2.log` (new order) and
`lane-logs/probe-collision-base-1.log` (old order) in the org-scan-boot
workspace.

## Missing piece
The keystone's vault has exactly one read-only file (`keystone-recipe.cook`)
and no transition writes an org file whose stem equals it, so no generated
sequence reaches two files sharing a name chain. For the warm-boot leg, the
first fix's tests booted a fresh database only; the stop/start harness
`a_recipe_edit_is_refused_after_a_reboot` already used was not applied.

## Remedy
`ingest_file` now asks `live_claimant_of` after a name-chain resolution too: a
file whose name chain resolves to a document another live file homes is
refused with `RefusedWhileClaimed(Document)` and disclosed (ERROR + the
`vault-ingest-failed` condition naming both files). Nothing of the refused
file is stored or written. The first claimant keeps its home.

Warm-boot leg: `FileSyncController::claim_persisted_read_only_homes` runs in
`initialize`, before any boot ingest. Every persisted `file` row of a
read-only path records its document's home (`doc_home`, plus the read-only
registry from the row's `read_only_blocks`). A persisted recipe is therefore
the first claimant, and a same-stem `.org` added between runs is refused and
disclosed with its bytes untouched.

Pinned by `cook_vault_ingest.rs`:
`an_org_page_and_a_same_stem_recipe_stay_two_files_at_boot`,
`an_org_file_written_beside_an_ingested_recipe_is_refused` and
`an_org_file_added_beside_a_recipe_between_boots_is_refused` (red without
the claim: `lane-logs/r4-warm-red-1.log`, `r4-warm-teeth-1.log`; green:
`r4-warm-green-1.log`). The keystone still cannot generate the pair (see
Missing piece). A runtime rename that creates the pair is a separate open
escape: `2026-10-07-runtime-rename-to-a-same-stem-recipe-collides-silently`.

## Open legs
- Offline vanish and offline move (verifier osb-verify-3, D4/D5): the recipe
  is deleted, or moved into a folder, while the app is off, and `Pasta.org`
  exists at its old stem. Boot 2 writes the recipe's step into `Pasta.org` on
  disk, with nothing disclosed. Pinned red by `cook_vault_ingest.rs`:
  `a_recipe_deleted_while_the_app_is_off_leaves_nothing_in_an_org_file_at_its_stem`
  and `a_recipe_moved_while_the_app_is_off_leaves_nothing_in_an_org_file_at_its_old_path`
  (`lane-logs/r5-d45-red-1.log`). Retiring the vanished home through the live
  deletion path at `initialize` is not enough: the boot runs with the Loro→SQL
  projection unarmed, so the org file's ingest deletes the step in Loro only,
  and its write-back renders the step back from SQL
  (`lane-logs/r5-d45-green-3.log`). Same root cause as
  `2026-10-07-headline-deleted-while-off-is-written-back-at-boot`; with the
  projection armed both tests pass (`lane-logs/r5-experiment-armed-2.log`).

## Persisted homes not loaded (FIXED)
If `load_file_projections` fails at `initialize`, no persisted recipe claims
its page, and the warm-boot leg wrote the step into `Pasta.org` again,
disclosed only at WARN (osb-verify-3 `loadfail-1.log`). Now the failure raises
the vault-start-incomplete condition, and for the rest of the session a
name-chain resolution refuses (and discloses) a page that no home of this
session holds while it holds blocks the file does not declare. Pinned by
`cook_vault_ingest.rs`
`an_org_file_beside_a_recipe_is_refused_when_the_recorded_file_state_is_unreadable`,
failure injected at the `load_file_projections` crash-injection point (red
`lane-logs/r6-loadfail-red-1.log`, green `r6-loadfail-green-1.log`, teeth
`r6-loadfail-teeth-1.log` and `-2.log`).
