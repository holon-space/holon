---
id: 2026-07-26-page-collision-destroys-rename-app-route
date: 2026-07-26
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Page-id collision DESTROYS a rename (in-app route, identity audit): page ids
  are `blake3(path)` (`PageId::for_path`, holon-api/src/link_parser.rs), so
  after renaming page A→B via the production `set_field` op (the id correctly
  does NOT re-mint), typing a NEW `[[A]]` and clicking it mints
  `PageId::for_path("A")` — the id the renamed page still holds.
  `create_page_and_navigate` (holon-frontend/src/reactive.rs:3487-3510) has no
  collision guard, `resolve_page_name`
  (holon/src/core/sql_operation_provider.rs:1054-1084) matches on TITLE only
  and finds nothing titled "A", and `prepare_create` issues `INSERT … ON
  CONFLICT(id) DO UPDATE SET <every field except id> = excluded.<field>`
  (sql_operation_provider.rs:662-676) — so the renamed page's content is
  silently overwritten back to "A". Observed verbatim: `after rename A->B via
  set_field: content="B"` then `after re-create A: content="A"`, SAME ID; both
  ops returned `Ok`, no error, no warning. Aggravating:
  `create_page_from_link` returns `declared_irreversible`, so undo does NOT
  restore the title. Narrowing: the trigger requires a NEWLY typed `[[A]]` —
  pre-existing `[[A]]` links keep `block_links.resolved_id = H("A")` and still
  resolve/navigate to the renamed page instead of creating. SUSPECTED
  (reasoned from the code, NOT demonstrated): because `upsert_set` covers
  every non-id field, the same UPSERT plausibly also rewrites `parent_id`,
  tags and timestamps, which would yank a nested renamed page to the vault
  root. Direct spec violation: `docs/Plans/PageIdentityDeterminism.md` §5.3
  states verbatim that "A new page created later under the new name gets a new
  id; that is correct (it is a different logical page)" — the code does the
  opposite.
source_line: 1108
---

## Bug

Page-id collision DESTROYS a rename (in-app route, identity audit): page ids
are `blake3(path)` (`PageId::for_path`, holon-api/src/link_parser.rs), so
after renaming page A→B via the production `set_field` op (the id correctly
does NOT re-mint), typing a NEW `[[A]]` and clicking it mints
`PageId::for_path("A")` — the id the renamed page still holds.
`create_page_and_navigate` (holon-frontend/src/reactive.rs:3487-3510) has no
collision guard, `resolve_page_name`
(holon/src/core/sql_operation_provider.rs:1054-1084) matches on TITLE only
and finds nothing titled "A", and `prepare_create` issues `INSERT … ON
CONFLICT(id) DO UPDATE SET <every field except id> = excluded.<field>`
(sql_operation_provider.rs:662-676) — so the renamed page's content is
silently overwritten back to "A". Observed verbatim: `after rename A->B via
set_field: content="B"` then `after re-create A: content="A"`, SAME ID; both
ops returned `Ok`, no error, no warning. Aggravating:
`create_page_from_link` returns `declared_irreversible`, so undo does NOT
restore the title. Narrowing: the trigger requires a NEWLY typed `[[A]]` —
pre-existing `[[A]]` links keep `block_links.resolved_id = H("A")` and still
resolve/navigate to the renamed page instead of creating. SUSPECTED
(reasoned from the code, NOT demonstrated): because `upsert_set` covers
every non-id field, the same UPSERT plausibly also rewrites `parent_id`,
tags and timestamps, which would yank a nested renamed page to the vault
root. Direct spec violation: `docs/Plans/PageIdentityDeterminism.md` §5.3
states verbatim that "A new page created later under the new name gets a new
id; that is correct (it is a different logical page)" — the code does the
opposite.

## Root cause

page-id collision DESTROYS a rename (identity audit, in-app route) — page
ids are `blake3(path)`, so after renaming A→B via the production `set_field`
op (id correctly not re-minted) a NEWLY typed `[[A]]` mints
`PageId::for_path("A")` = the id the renamed page still holds;
`create_page_and_navigate` (reactive.rs:3487-3510) has no collision guard,
`resolve_page_name` matches on title only, and `prepare_create`'s `ON
CONFLICT(id) DO UPDATE SET <every field except id>` overwrites the renamed
page's content back to "A" — both ops `Ok`, no warning, and
`declared_irreversible` means undo does not restore the title. Directly
contradicts PageIdentityDeterminism.md §5.3. Keystone is STRUCTURALLY unable
to reach it: the property is TEMPORAL (free a name, then reuse it) and no
transition renames anything — `focus_editable_text.rs:174` excludes page
blocks, `create_document.rs:50` draws from a monotonic counter that never
decrements. COVERAGE primary, no secondary — once reachable, existing
content oracles would plausibly fire.)

## Missing piece

The keystone is STRUCTURALLY unable to generate the trigger: the property is
TEMPORAL (a name must be FREED and then REUSED) and no transition in the
catalog renames anything. `focus_editable_text.rs:174` explicitly excludes
page blocks (`state.is_text_block(&id) && !state.is_page_block(&id)`), so no
editing rung can retitle a page; `create_document.rs:50` draws names from a
MONOTONIC counter (`format!("doc_{}.org", state.next_doc_id())`,
`action_actor_state.rs:56` increments and never decrements, incl. across
DeleteDocument), so a freed path is never re-minted; `write_org_file.rs`
draws from a random `[a-z_]+_[0-9]+\.org` alphabet with no reuse intent.
Missing pieces: (a) a rename-page transition driving the production
`set_field` op, and (b) a create-page-by-link rung that can re-mint a freed
name. Once reachable, the existing content/row-match oracles would plausibly
fire — the gap is generation, not assertion.

## Remedy

OPEN (found by identity audit outside any automated test; fix must close the
COVERAGE gap first — rename + freed-name-reuse transitions,
red-for-the-right-reason — then guard `create_page_from_link` against
minting an id that already exists, and reconcile with
PageIdentityDeterminism.md §5.3)
