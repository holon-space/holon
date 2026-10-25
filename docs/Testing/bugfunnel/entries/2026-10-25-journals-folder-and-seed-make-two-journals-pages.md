---
id: 2026-10-25-journals-folder-and-seed-make-two-journals-pages
date: 2026-10-25
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  A vault `Journals/` folder and the seeded `block:journals` page became two
  pages titled "Journals" under the root, so the page-position guard refused
  every later plan that resolved a page there.
---

## Bug
A vault with a `Journals/` folder and no `Journals.org` boots with TWO root
pages titled "Journals": the folder-ingest page (a path-derived id) and the
seeded `block:journals`. The `page_at` guard then refuses any plan that
resolves a page position under the root:

```
convert_block_to_page: planning failed: pages under sentinel:no_parent titled
"Journals" up to case, Unicode normalization and spacing:
[("block:24e2a8f4-3dfb-7a1b-7c80-acea4cd4d40e", "Journals"),
 ("block:journals", "Journals")]; a page position holds one page
```

Found outside the keystone: the org_suite harness test
`convert_block_to_page_materializes_child_file::runtime_convert_block_to_page_materializes_rehomed_child_file`
was red on `main`, and the refusal (not the materialization it tests) was the
cause. Same family as
`2026-10-05-journals-ingest-deletes-seeded-journal-rule-on-boot`.

## Root cause
The two sides keyed the same page differently, and nothing linked them:

- the seed created `block:journals` BY ID from Rust builders in
  holon-frontend, and wrote no `Journals.org` (the disk asset list was empty)
- the folder ingest resolves a folder through its companion file's `#+ID`
  and, with no companion, mints `PageId::for_path("Journals")`
  (crates/holon-filesystem/src/file_sync_controller.rs,
  `resolve_dir_page_chain`)

Boot order is ingest-first (the org initial scan runs before the block seed),
so the ingest page always existed first and the seed added a second one.

## Missing piece
- Coverage: the keystone PBT never boots over a vault that already holds a
  `Journals/` folder, so no generated case reached this state.
- Environment: the page existed only as Rust builders, never as the file the
  folder resolution reads.

## Remedy
The Journals page is an ordinary page shipped as a file.

- `assets/default/Journals.org` declares `#+ID: journals` and fixed `:ID:`s.
  `holon_frontend::DEFAULT_ASSETS` lists it; the Rust builders and consts are
  gone.
- The disk seed (`seed_default_org_assets`, crates/holon-orgmode/src/di.rs)
  writes an asset before the scan only when no org file in the vault declares
  its `#+ID`. The `Journals/` folder then resolves to `block:journals` through
  the companion, the same path every page takes. A page the user moved into
  another file gets no second file.
- The block seed parses the same asset and creates its blocks by id: it skips
  the blocks that exist and creates the missing ones.
- A folder whose existing page differs from its companion's `#+ID` raises the
  sticky condition `CompanionPageIdOverruled` with both ids, beside the WARN.
- Pinned by `org_suite journals_folder_is_the_seeded_page` (one root page,
  the asset's id, every asset block present; and no second file for a moved
  page across a reboot), plus the convert test above.

Open: a user `Journals.org` with another `#+ID` plus the block seed still
gives two root pages titled "Journals".
