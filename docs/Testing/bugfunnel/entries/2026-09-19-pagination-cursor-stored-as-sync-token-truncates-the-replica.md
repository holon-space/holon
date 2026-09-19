---
id: 2026-09-19-pagination-cursor-stored-as-sync-token-truncates-the-replica
date: 2026-09-19
gap: ORACLE
status: FIXED
summary: >-
  A sidecar's pagination cursor is persisted as the incremental SYNC TOKEN and
  only one page is fetched per sync, so the last page (which carries no
  `nextCursor`) falls into the full-sync diff and deletes every row not on that
  one page. The Todoist project replica collapsed 96 -> 13 and stayed there,
  silently, with `sync()` returning Ok.
---

## Bug

Split out of `2026-09-19-todoist-full-sync-mirror-divergence-kills-the-replica`
by adversarial verification of that entry's first fix. The dogfood observation
was `SELECT count(*) FROM todoist_projects` → `96` before a boot, `13` after
it, stable across later boots. That symptom has a different cause from the
declined-rows defect in the sibling entry, and the sibling's fix does not touch
it.

Reproduced deterministically with every column present, so the declined-rows
mechanism is provably not in play. Four projects, page 1 carrying three plus a
`nextCursor`, page 2 carrying the fourth and no cursor:

```
PROBE after_page1=3 after_page2=1 second_is_ok=true
```

Three rows deleted, and the sync returned `Ok` — no error, no `Sync failing`
status, nothing above `info` in the log.

## Root cause

`assets/integrations/todoist.yaml` declares, for both entities:

```yaml
      cursor:
        request_param: cursor
        response_field: nextCursor
```

That is Todoist's REST pagination cursor: `find-projects` and `find-tasks`
return `nextCursor` to page through ONE result set. It is not a "give me what
changed since" token. (Todoist's separate Sync API does have such a token; this
sidecar does not use it.) `CursorConfig` nevertheless documents itself as
"cursor configuration for incremental sync", and the engine treats it that way.

`ToolSync::fetch_records` fetches exactly one page per sync — there is no page
loop — and hands the page cursor back as `FetchResult::new_cursor`.
`sync_entity_inner` then branches on it:

- `new_cursor: Some` (pages remain) → `apply_incremental`, and the PAGE cursor
  is persisted through `SyncTokenStore::save_token` as the entity's sync token.
- `new_cursor: None` (the LAST page) → `apply_full_sync`, which diffs the whole
  cache table against that single page and emits `Change::Deleted` for every
  row not on it.

So a two-page provider lands page 1, saves its cursor, and on the next sync
resumes at page 2 — which is the last page, so the full-sync diff deletes
everything that was not on page 2. The token is never cleared at the end of
pagination, so every later sync refetches that same last page and re-pins the
table at its size. That is `96 → 13`, stable across boots.

Two conflated concepts sit behind it: a PAGE cursor is valid only within one
fetch and must never outlive it, while a SYNC TOKEN is resumable across syncs.
One config key and one `Option<String>` carried both.

## Missing piece

ORACLE. Nothing in the tree exercises a provider that paginates. Every sync
fixture — `rest_transport_mock.rs`, `fake_mcp_module.rs`, the
`mirror_cutover_tests` PBT — returns its whole record set in one response, so
the page loop that does not exist is never missed and the last-page-as-full-sync
branch is never reached. The generator can reach the state; no fixture and no
invariant judges it. Todoist is the only shipped sidecar declaring `cursor:`,
so the one configuration that triggers this is the one nothing covers.

## Remedy

FIXED. A page cursor and a sync token are now distinct types, so the YAML says
which one it means:

1. `PaginationConfig` (`paginate:` in a sidecar's `sync:`) is new, and carries
   the doc comment that `CursorConfig` used to carry wrongly. `CursorConfig`
   (`cursor:`) is now documented as what it always was — a RESUMABLE sync
   token, persisted across syncs. Declaring both is refused at
   `SyncConfig::into_strategy`, because a config that declares both has not
   decided which it means.
2. `ToolSync::fetch_all_pages` follows the page cursor to exhaustion inside ONE
   fetch and returns `new_cursor: None`, so the caller applies the complete set
   through the full-sync diff exactly once. A page cursor is never persisted. A
   provider that repeats a cursor, or never stops offering one, fails loudly
   rather than looping or truncating.
3. `assets/integrations/todoist.yaml` moves both entities from `cursor:` to
   `paginate:`, which is what Todoist's `nextCursor` always was.
4. `assets/integrations/README.md` taught `cursor:` for pagination, which is
   how the sidecar came to declare it. That section now documents the two keys
   side by side with the question that tells them apart.
5. A resource sync (`list_resource`) silently dropped `paginate`, `cursor`,
   `list_params` and `project`. It now refuses them by name at load, so a page
   cursor declared where nothing can follow it is a loud error rather than a
   truncated replica.

CONSIDERED AND REJECTED: bumping `SIDECAR_SCHEMA_VERSION` 2 → 3 to force
installed copies back to the bundled content. `paginate:` is purely additive —
no previously valid file stops parsing — so the constant's own stated trigger
is not met, and the bump has a real cost: an INTRODUCED (non-bundled)
connection at an older generation has no bundled copy to fall back to and is
recorded `Unusable` (crates/holon-mcp-client/src/integration_config.rs:882-893),
which would have killed the `github` connection on Martin's profile at the next
boot.

RESIDUAL, accepted and disclosed: a profile holding an INSTALLED
`todoist.yaml` that declares `schema_version: 2` keeps using that file, so it
keeps the old `cursor:` semantics and the truncation until the file is
re-installed. Martin's profile holds no installed `todoist.yaml`, so nothing
reachable today is affected.

MEMORY: `fetch_all_pages` accumulates every page in memory before applying, so
a provider with very many pages holds the whole result set at once, bounded
only by the 10,000-page cap. Acceptable for the mirror sizes in play (Todoist
projects and tasks) and unchanged in shape from the previous single-page
behaviour, which also held one full response; a provider that genuinely streams
needs a different pipeline, not a bigger cap.

Covered by `one_sync_fetches_every_page_of_a_paginating_provider` and
`a_second_sync_does_not_delete_the_rows_from_earlier_pages` in
crates/holon-integration-tests/tests/mcp_sync_partial_write_divergence.rs, both
driving the real engine over a real `QueryableCache`. Red log:
lane-logs/red-12-pagination.log, reproducing the probe shape exactly —
`left: (3, 1)` against `right: (4, 4)` for the two-sync case, and one tool call
where two are required.
