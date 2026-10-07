---
id: 2026-10-07-runtime-rename-to-a-same-stem-recipe-collides-silently
date: 2026-10-07
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Renaming `Nudel.cook` to `Pasta.cook` beside `Pasta.org` at runtime discloses
  nothing: two pages carry the title "Pasta", the recipe is never ingested
  under its new path, and a stale `file:Nudel.cook` row stays for a path that
  no longer exists.
---

## Bug
Found by the verifier osb-verify-2 (org-scan-boot lane, D108.a), probe
`probe_p4_a_runtime_rename_creates_the_collision`, logs
`lane-logs/osb-verify-2/p4-1.log` and `p4-2.log` in the org-scan-boot
workspace. Boot a vault holding `Nudel.cook` and `Pasta.org`, then
`rename_file(Nudel.cook -> Pasta.cook)`:

- `refused cooklang=[] org=[]`: nothing is disclosed.
- `block:Pasta.cook::b::0` never reaches the store; `block:Nudel.cook::b::0`
  stays in it.
- Two pages carry the content `Pasta`: the org file's page and the recipe's
  re-titled page.
- The `file` table still maps `file:Nudel.cook` to the recipe's document.

Both files on disk keep their bytes and the org page stays editable, so no
content is lost. The same probe gives the same state on the controller before
the same-stem fix (`base-ab-1.log`), so this is pre-existing.

## Root cause
`FileSyncController::on_file_renamed`
(`crates/holon-filesystem/src/file_sync_controller.rs`, `on_file_renamed`)
re-homes the document of `from` onto `to` with `note_doc_home` and re-titles
its page to the new stem. It never asks whether another live file already
homes the page that `to`'s name chain names, so the guard in `ingest_file`
(`live_claimant_of` after a name-chain resolution, bugfunnel
`2026-10-07-same-stem-org-and-recipe-merge-into-one-document`) is not reached.
A recipe's block ids derive from its path (`<file>::b::<n>`), so the moved
bytes are an echo of nothing the store holds under the new ids, and the stale
`file` row of `from` is not migrated or removed.

## Missing piece
No test renames a read-only file at runtime: the cook suite
(`crates/holon-integration-tests/tests/cook_vault_ingest.rs`) renames only a
REFUSED recipe, and the keystone has one read-only file
(`keystone-recipe.cook`) and no transition that renames it or creates a
same-stem org file beside it.

## Remedy
Open; not fixed in the round that found it. Red-test plan:

1. Add `a_recipe_renamed_onto_an_org_pages_name_is_refused` to
   `cook_vault_ingest.rs`: boot `Nudel.cook` + `Pasta.org` (no `#+ID:`), wait
   for `block:Nudel.cook::b::0`, then `rename_file(Nudel.cook, Pasta.cook)`
   and `wait_for_org_files_stable`. Assert:
   - `refused_names(&app, "cooklang") == ["Pasta.cook"]`;
   - `Pasta.org` and `Pasta.cook` keep their bytes;
   - `block:pasta-note` stays editable (`edit_refusal` is `None`);
   - exactly one page carries the content `Pasta`;
   - no `file` row names `Nudel.cook` and no `block:Nudel.cook::b::*` row
     remains.
   Red today on the "disclosed as refused" assertion.
2. Fix: in `on_file_renamed`, resolve `to`'s name chain before the re-home;
   when it names a document another live file homes, retire `from`'s state as
   a deletion and refuse `to` through the same `RefusedWhileClaimed(Document)`
   + `disclose_shared_name_chain` path that `ingest_file` uses. Migrate or
   delete the `file` row of `from` in every branch.
