# Plan: Fork B — Writeback stops inlining child pages; fileless child pages get materialized

**Date:** 2026-07-12
**Status:** PROPOSED (senior-review pending)
**Owner stream:** Fork B (org WRITEBACK side of the folder-page duplication defect)
**Sibling stream:** Fork A (org INGEST side — foreign-doc-root protection + keystone topology
seeding; being landed red-first in parallel). Fork A and Fork B MUST agree on ONE
child-page definition (see §1.3) and Fork A's seeding is the substrate Fork B's oracles
extend (§5).
**Binding ruling (Martin):** folder-companion files (e.g. `Journals.org` for `block:journals`)
must stop inlining child blocks that are themselves pages/documents — the companion becomes
empty-of-child-pages like `Frontends.org` — and FILELESS child pages (rule-created future
journal dates such as `2026-07-11`, existing only inside the companion) must be MATERIALIZED
into their own files (`Journals/<name>.org`).

**Base-gate:** cut from the current integration chain proven by
`docs/Plans/RefStateSplit-2026-07-12.md` (present in this worktree).

---

## 0. Executive summary — what the code already does, and what is actually missing

The reconnaissance overturned the naive framing ("teach writeback to skip child pages and
write new files"). Most of that machinery **already exists**; the defect is narrower and the
fix is smaller and sharper than the ruling's phrasing implies.

Three load-bearing facts, each verified in the tree:

1. **De-inlining is already implemented — at the document-membership boundary, not the
   renderer.** `get_blocks(doc_id)` (the writeback's block source,
   `crates/holon-app/src/turso_seams.rs:212`) is a recursive CTE that walks down from
   `doc_id`'s children and **excludes any block tagged `Page`** (`bt.tag = 'Page' … IS NULL`,
   lines 226-234). The renderer (`OrgRenderer::render_entitys`,
   `crates/holon-org-format/src/org_renderer.rs:56`) then renders exactly the slice it is
   given and **panics** (WP-F projection assertion, lines 87-101 / 131-149) on any dangling
   parent — so pruning must happen in the CTE, never by editing the slice. **Why
   `Frontends.org` is already empty and `Journals.org` inlines is therefore a single cause:
   the frontend child pages keep their `Page` tag; the journal dates LOST it** (the Page-tag
   demotion bug Fork A fixes). Once the tag is preserved, the CTE excludes the dates and the
   companion renders de-inlined automatically. **Fork B writes no new de-inline logic.**

2. **Materialization is already implemented — latently.** There is no separate "Document
   entity": `DocumentManager` (`crates/holon-filesystem/src/sync_ports.rs:120`) is a *view
   over Page blocks*. `name_chain(page_id)` (line 156) walks Page-tagged ancestors collecting
   `title()`; `doc_id_to_path` (`file_sync_controller.rs:2532`) turns that chain into
   `<root>/<seg>/…/<name>.org`. The block-CDC writeback driver
   (`resolve_doc_for_block`, `crates/holon-orgmode/src/di.rs:524`) resolves a changed block
   to **the nearest `Page` including itself**, so a Page-tagged block's delta routes to
   `on_block_changed(page_id)` (`file_sync_controller.rs:1840`), which does
   `create_dir_all(parent)` + `fs.write(path, rendered)` (lines 1906-1909). **A Page block
   whose ancestors are Pages already materializes to its own file the moment a CDC delta
   touches it.** Fork B does not invent a materialization path; it ensures the *existing* one
   fires for every fileless page and is not blocked by the guard (fact 3).

3. **The writeback guard is per-file and will VETO the migration.**
   `ensure_ingest_lossless(path, source, rendered, root)`
   (`crates/holon-orgmode/src/writeback_guard.rs:114`, wired at
   `crates/holon-orgmode/src/file_format.rs:114`) re-parses the on-disk `source` and the
   `rendered` projection **of that one file** and refuses the write if any source block
   survives in NEITHER `rendered`'s id-set NOR its normalized-content-set. On migration,
   `Journals.org`'s on-disk source still contains the inlined dates, but its de-inlined
   `rendered` omits them → the guard sees them as dropped → **refuses the write and
   quarantines the file** (loud `IngestLoss`). The companion can never converge and the
   children are never written from it. **This is the one genuinely hard correctness seam and
   the core of Fork B's implementation work (§4).**

**Net scope of Fork B** (each an increment in §6):
- **B1** — subtree-scoped writeback guard (fact 3): the guard must treat a block as preserved
  if it survives anywhere in the companion's *materialized subtree*, not only in the companion
  file. This is the hard seam; designed in full in §4.
- **B2** — a fileless-page materialization sweep: enumerate Page blocks whose file is absent
  and drive their `on_block_changed` so the latent path (fact 2) actually fires for them,
  ordered safe-side (children before de-inlined companion, §4.3).
- **B3** — migration convergence + Fork A interplay (§3), proven by an upgrade oracle.
- **B4** — keystone topology + writeback oracles (§5).

---

## 1. Current serialization path (verified file:line map)

### 1.1 Where writeback decides what a document contains
- **Block source (the membership boundary):** `BlockReader::get_blocks(doc_id)` →
  `CacheBlockReader::get_blocks`, `crates/holon-app/src/turso_seams.rs:212-260`. Recursive CTE
  from `doc_id`'s children, **`Page`-tagged blocks excluded** = sub-document boundary. Loro
  path mirror: `crates/holon-app/src/loro_seams.rs:123`.
- **Render:** `FileSyncController::render_doc_blocks`
  (`file_sync_controller.rs:2323`) → `format.render_document(doc, blocks, path, doc.id)` →
  `OrgRenderer::render_document` (`org_renderer.rs:22`) → `render_entitys` (`:56`). The
  renderer walks the given slice as a tree rooted at `file_id`; **WP-F assertions panic on a
  dangling parent or an unreachable block** (`:87`, `:131`). Consequence for Fork B: never
  hand the renderer a partial subtree — prune at the CTE.
- **Full-file vs incremental:** `render_file_by_doc_id` (`:2314`, full read) and
  `render_with_cache` (`:1956`, per-edit cache; reseeds on a `tags` change — H4 Page toggle,
  docstring `:1938`) both feed a complete `&[Block]` to `render_doc_blocks`, so output is
  identical regardless of source.

### 1.2 Why `Frontends.org` stays empty; where materialization hooks
- `Frontends.org` empty ⟺ its children are `Page`-tagged ⟺ excluded by the CTE. Same
  mechanism will empty `Journals.org` once the dates keep their `Page` tag (Fork A).
- **Materialization hook (existing):** `on_block_changed(doc_id, delta)`
  (`file_sync_controller.rs:1840`) — resolves `doc_id_to_path` (`:2532`, via
  `DocumentManager::name_chain` or the Loro `AliasRegistrar`), `create_dir_all` + `write`
  (`:1906-1909`). Driven by `resolve_doc_for_block` (`di.rs:524`) off the block CDC feed.
- **Batch writeback:** `re_render_all_tracked` (`:2170`) iterates **only tracked files**
  (`last_projection` keys) — a fileless page is NOT tracked, so this path alone never
  materializes it. B2 closes that gap (§4.2).

### 1.3 Child-page detection — the ONE definition (shared with Fork A)
A block is a **child page that owns its own file** iff `block.is_page()` — i.e. it carries the
`"Page"` tag (`crates/holon-api/src/block.rs:372-390`, `PAGE_TAG` / `is_page()` / `set_page()`).
There is no separate `owns_file` bit: **Page ⟺ document-root ⟺ owns exactly one file** at the
path given by its `name_chain`. Every consumer already agrees on this predicate:
- CTE membership exclusion — `turso_seams.rs:226`
- CDC doc routing — `resolve_doc_for_block`, `di.rs:531`
- path/name chain — `DocumentManager::name_chain`, `sync_ports.rs:169`
- render-side owning-doc walk — `find_document_id`, `org_format/models.rs:47`

**Contract Fork A + Fork B pin together:** Fork A guarantees ingest never *demotes* a block
that is (or should be) a page to a plain heading; Fork B relies on `is_page()` remaining the
sole predicate. Neither stream may introduce a second notion of "is a page" (e.g. a path-based
or file-existence-based test). **RULED (§7, OQ1): `is_page()` is the sole predicate. Fork A makes
the tag truthful; Fork B reads only the tag. If a spot is found where the tag alone is
insufficient, STOP and report rather than adding a parallel predicate.**
The id/scheme handling is per `docs/Reference/ORG_SYNTAX.md`: on disk the page's identity is
its `#+ID:` (bare, no `block:`); the renderer strips the scheme, the parser re-adds it. A
materialized child file is a normal page file — `#+ID:` = the page block's bare id, `#+TITLE:`
= its `title()`.

---

## 2. Materialization semantics

- **Trigger.** Two entry points feed the *same* `on_block_changed(page_id)` sink:
  1. *Steady state* — the auto-create rule (or a user) creates/tags a Page block; the CDC
     delta routes via `resolve_doc_for_block` to that page's id; `on_block_changed`
     materializes it. Already works once the block is `Page`-tagged at creation.
  2. *Sweep* (B2, §4.2) — a convergence pass enumerates Page blocks with no file on disk and
     drives `on_block_changed(page_id, Remove-sentinel→reseed)` for each, so migration and any
     missed CDC delivery both converge without waiting for a fresh edit.
- **Naming / path.** `name_chain(page_id)` → `["Journals","2026-07-11"]` →
  `<root>/Journals/2026-07-11.org`. Nested folders fall out of the chain (a page under a page
  under a page → three segments). **Sanitization + collision (OQ2, §7):** `title()` may contain
  `/`, `:`, or clash case-insensitively with a sibling. Today `doc_id_to_path` joins titles raw.
  Fork B must define one sanitization at the path boundary (recommend: reject `/` and control
  chars → the page keeps living inline under its parent with a loud warning rather than writing
  a traversal path; this is fail-loud, not silent). Collisions between two Page siblings with the
  same title are a **represented ambiguity** — surface a warning and suffix the second
  (`<name>~<shortid>.org`); do not silently overwrite. This mirrors the links-ruling "ambiguity
  = represented state" stance.
- **What the new file contains.** The child page's own subtree: its `#+ID:`/`#+TITLE:` header
  (`render_document_header`, `models.rs:354`) + `get_blocks(page_id)` (which itself stops at any
  grand-child Page — nested materialization is recursive and each level owns its file).
- **What the companion retains.** *Nothing of the child page* — no heading, no link line. This
  is the LogSeq-parity target (`Frontends.org` retains nothing of GPUI.org) and is consistent
  with links-as-marks: a reference to the child page, if the user wants one, is an explicit
  `[[2026-07-11]]` mark in some block's content, not a structural inline the writeback
  fabricates. The companion is free to contain its *own* non-page blocks (a folder page may have
  direct prose children); only Page-tagged children leave.

---

## 3. Migration — converging existing duplicated vaults without data loss

**Starting state (real vault):** `Journals.org` on disk has dates inlined as plain headings
(they lost `Page`); `Projects.org` similarly. Some dates (`2026-07-11/12`) exist *only* inline.

**Convergence sequence (first boot after upgrade), per folder companion:**
1. **Ingest** `Journals.org`. Fork A's foreign-doc-root protection keeps/here restores the
   `Page` tag on the inlined dates (they are pages that own — or should own — their own file).
   The dates become `Page`-tagged blocks under `block:journals`.
2. The CTE now **excludes** them from `get_blocks(journals)`; the companion's `rendered`
   de-inlines.
3. **Guard veto (the trap).** The per-file guard compares on-disk `Journals.org` (still
   inlined) against the de-inlined `rendered` → dates dropped → refuse + quarantine. **Without
   B1, migration deadlocks here.** With B1 (§4): the guard's surviving-set is the union across
   the companion + the dates' own materialized files, so the dates are "preserved elsewhere"
   and the write proceeds.
4. **Ordering (the loss risk).** Children are materialized to `Journals/<date>.org` **before**
   the de-inlined `Journals.org` is written (§4.3). A crash between leaves *duplication*
   (companion still inlined AND child file present) — recoverable — never *loss*.
5. **Idempotent re-run.** On the next boot the duplication re-ingests: the inlined date in
   `Journals.org` and the same date owned by `Journals/2026-07-11.org` collide →
   `find_foreign_blocks` (`sync_ports.rs:87`) reports the conflict → **Fork A's foreign-doc-root
   protection resolves it** (the date is owned by its own file; the inline is dropped on the
   next de-inlined render). This is why **Fork A remains necessary after Fork B**: B stops
   *emitting* the inline and materializes the file; A disambiguates the *transient duplication*
   that any non-atomic migration inevitably produces, and guards the steady state against a
   future re-demotion. They are complementary, not redundant.

**Interaction summary:**
- *B needs A:* without A the dates re-demote on every ingest and the CTE re-inlines them —
  B's de-inline never sticks.
- *A needs B:* without B the companion keeps inlining on writeback even when A tagged the
  dates pages — the disk never converges and the fileless dates are never written anywhere
  (data loss for `2026-07-11/12`).
- *No conflict:* A touches ingest (`parse` → tag decisions); B touches writeback (`get_blocks`
  membership is already A-friendly; guard scope; materialization sweep). They meet only at the
  `is_page()` predicate (§1.3) and at the transient-duplication handoff (step 5).

---

## 4. The guard-interplay design (hardest seam — B1)

### 4.1 Requirement
`ensure_ingest_lossless` must stop treating a block de-inlined from a companion as loss **iff**
that block genuinely survives in a sibling file of the same materialized subtree, and must
STILL fail loud if the block vanished from everywhere (real ingest loss — its whole reason to
exist, BugFunnel row 28).

### 4.2 Chosen shape — evidence-based union projection (recommended)
Refactor the guard from *"parse one `rendered` string"* to *"check against a precomputed
surviving projection"*:

```
struct SurvivingProjection { ids: HashSet<String>, contents: HashSet<String> }
fn ensure_ingest_lossless(path, source, surviving: &SurvivingProjection, root) -> Result<()>
```

- **Per-file callers (unchanged behavior):** build `surviving` from just that file's `rendered`
  (parse it once, collect ids+normalized contents) — byte-identical to today's semantics.
- **Companion writeback (new):** `FileSyncController` assembles `surviving` as the **union** of
  the companion's own `rendered` AND the `rendered` of every child-page file it is materializing
  in this same convergence pass. A date de-inlined from `Journals.org` is in
  `Journals/2026-07-11.org`'s render → present in the union → not dropped. A block that fell out
  of *both* the companion and every child → absent from the union → flagged, loud refuse,
  quarantine (row-28 protection intact).

**Why union-of-evidence, not a moved-id whitelist.** The rejected alternative (keep
`rendered: &str`, add `moved_to_children: &HashSet<id>`) trusts the controller's *assertion*
that a block moved; a bug that miscomputes the moved-set would wave real loss through. The union
shape is **evidence**: a block counts as preserved only because it is *actually present in a
render that is actually being written*. Fail-safe beats fail-declared, per the fail-loud
directive. Cost: the companion guard needs the child renders in hand first — which the ordering
in §4.3 requires anyway.

**Parse-don't-validate note:** `SurvivingProjection` is the parsed evidence set; the guard no
longer re-parses a `rendered` string on the companion path (it still parses `source`). This
also removes the "concatenated multi-file text won't parse as one org file" problem that a naive
"pass the concatenation as `rendered`" hack would hit.

### 4.3 Atomicity / ordering (no cross-file fs transaction exists)
Per folder companion whose subtree materializes N child files, one convergence pass does:
1. Compute companion `rendered` (de-inlined) and each child `rendered`.
2. Build the union `SurvivingProjection`; run the companion guard against it. **Abort the whole
   unit on `Err` (quarantine companion, do not touch children).**
3. **Write child files first**; confirm each on disk.
4. **Write the de-inlined companion last.**
5. Seed `last_projection[child]` and `last_projection[companion]` to the exact bytes written,
   and register child paths as tracked, **before** releasing the watcher (§4.4).

Crash between (3) and (4): duplication, not loss (companion still inlined) → converges next boot
(§3 step 5). Crash during (3): some children written, companion untouched → duplication of a
subset → converges. **There is no interleaving that loses a block**, because the companion is
only ever de-inlined *after* its children exist on disk.

### 4.4 Watcher feedback loop (materialization creates files ingest then re-reads)
Writing `Journals/2026-07-11.org` fires the file watcher → `on_file_changed` → re-ingest, which
could re-mint the page under a new id or loop. The existing defense is `last_projection` +
`_change_origin` echo suppression: a watcher event whose disk bytes equal `last_projection[path]`
is a self-induced echo and is skipped. **B2 must seed `last_projection[new_child_path] =
rendered` and mark the path tracked at the moment of the write** (step 5), exactly as
`on_block_changed` already does for existing files (`:1920`). New-file creation must not bypass
that seeding — otherwise the fresh file has no baseline, `on_file_changed` treats it as an
external change, and re-ingests it (the loop). This is the one place B2 touches shared
controller state and the review focus for that increment.

---

## 5. Keystone coverage (extends Fork A's topology seeding)

Fork A is adding subdirectory page-files + companion seeding to `wide_e2e.rs` (today the seed is
one page root + three leaf siblings; no folder companion — Fork A scout confirmed). Fork B adds
**writeback-side oracles** on top of that seed. Coordinate: Fork B consumes Fork A's seeded
topology; do not fork a second topology.

Seed shape Fork B needs (request into Fork A's seeding, else add behind the same seed flag):
a folder page `block:journals` (a `Page`), with (a) a child page that owns a file
(`Journals/2026-07-10.org`), and (b) a **fileless** child page (`2026-07-11`, `Page`-tagged, no
file) — plus a migration variant where the folder companion `Journals.org` starts with the
child **inlined** (pre-Fork-A-fix disk shape) to exercise convergence.

New/extended invariants (bodies under
`crates/holon-integration-tests/src/pbt/invariants/bodies/`, wired in the composed catalog):
- **inv-companion-has-no-child-page-headings** (NEW): after settle, for every folder companion
  file, the parsed disk blocks contain **no block that is `Page`-tagged** (child pages left the
  companion). Directly encodes the ruling.
- **inv-every-page-has-its-own-file** (NEW): after settle, for every `Page` block with a
  resolvable `name_chain`, a file exists at `doc_id_to_path` and its `#+ID:` equals the page's
  bare id. Covers both "fileless page got materialized" and "no page is double-homed".
- **inv-fileless-page-materialized** (NEW, migration): starting from the inlined-companion
  variant, after settle the fileless child's file exists AND the companion no longer inlines it
  AND no block was lost (the guard passed via the union set, not by quarantine).
- **inv-org-render-fixed-point** (EXTEND, `org_render_fixed_point.rs:36`): already re-renders
  every tracked file and asserts disk == render. Extend the tracked set to include newly
  materialized child files so the fixed-point covers them (they must be idempotent: a second
  render equals the first — catches a companion↔child echo loop where each re-inlines/re-strips).
- **inv-blocks-match-ref** (already the id-set equality oracle,
  folded-in `inv-ingest-totality`): the reference model must model the de-inline+materialize as
  a *relocation*, not a deletion — total block id set is invariant across the migration (nothing
  lost, nothing duplicated after convergence).

Reference-model change (oracle side): the `ReferenceState` domain must predict, for a given
block topology, **which file each Page's subtree renders to** — i.e. a page's blocks belong to
that page's file, not its parent's. If the oracle still attributes child-page blocks to the
folder companion, `inv-companion-has-no-child-page-headings` and `inv-org-render-fixed-point`
will (correctly) go red against a fixed prod — so the oracle update lands in the same increment
as the prod change (red-first: write the oracle expectation, watch it fail on today's inlining
prod, then fix prod). **This is the CLAUDE.md rule made concrete: the keystone must reproduce
the folder-page inlining bug before Fork B fixes it.**

---

## 6. Increments — RE-SCOPED 2026-07-12 after empirical B0 findings

### Empirical findings that re-scope the plan (verified in this tree)

Two findings from building B0 (both via `HeadlessFrontendComponent` boots + on-disk dumps)
overturn the plan's original guard-veto premise:

1. **The companion de-inline ALREADY works, losslessly, for a page that owns its own file — no
   Fork B fix needed.** Seed `child-note.org` (`#+ID: child-note`, a Page) + `my-notes.org`
   inlining its id; after settle `my-notes.org` → bare `#+ID: my-notes` (de-inlined), the page
   survives in `child-note.org`, all oracles green. Root cause: Fork A re-homes the page's
   doc-root to `no_parent` (owned by its file), so it is never a child of the companion → the
   companion's `get_blocks` render is naturally bare → nothing is dropped → clean. (Green
   regression lock: `structural_pbt::folder_companion_deinlines_owned_child_page`.)

2. **The ingest-loss guard runs on exactly ONE site — the ingest reproject path
   (`file_sync_controller.rs:1821`), NOT the block-driven writeback (`on_block_changed` /
   `re_render_all_tracked`).** So the plan's original premise ("the guard vetoes the de-inline;
   B1's union lifts the veto") is false for the steady-state block-driven path. But it IS the
   crux of the FILELESS case: a companion inlining a `Page`-tagged heading with NO backing file
   (`* child-note :Page:`, no `child-note.org`) makes the page a child of the companion; the
   ingest reproject de-inlines it (CTE excludes Page), the guard at :1821 sees the drop and
   **quarantines** the companion (row 28 protection — content preserved on disk, inline, but the
   file can never converge), and nothing materializes the page into its own file. (RED-first:
   `structural_pbt::fileless_page_writeback_materializes`, `#[ignore]`d.)

**Consequence:** the real bug is **fileless-page materialization (B2)**, not a de-inline veto.
The `SurvivingProjection` union guard is resurrected with a *better* purpose (B1', below): the
finding that the block-driven writeback path writes files with NO loss guard is itself a
**P0-class coverage hole** — that path could silently drop user blocks with zero protection, row
28 covering only the ingest site. Extending the guard there is where the union becomes
load-bearing (it lets a legitimate de-inline pass — the block survives in a sibling/materialized
file — while a real drop still vetoes).

### Increment gate
```
cargo check -p holon-integration-tests --features pbt --all-targets | tee /tmp/forkb-check.log
cargo nextest run -p holon-orgmode -E 'test(writeback_guard)' | tee /tmp/forkb-guard.log
cargo nextest run -p holon-integration-tests --features pbt \
  -E 'test(folder_companion_deinlines_owned_child_page) + test(fileless_page_writeback_materializes) + test(companion_has_no_child_page_headings)' | tee /tmp/forkb-b0.log
cargo nextest run -p holon-integration-tests -E 'test(general_e2e_composed_pbt)' | tee /tmp/forkb-keystone.log
```

- **B0 — red-first repro + oracle + observability fix. DONE (this commit).**
  - `inv-companion-has-no-child-page-headings` (body + composed wiring + catalog; body units
    green) — a companion `.org` must retain no heading for a ref-modeled `Page`. Dormant on the
    default keystone (inert unless a companion inlines a ref-page id).
  - **Observability fix** (`components.rs`): register zero-block page-file docs via
    `parsed.document.id` so `SutOrgRender::snapshot_org_render_pairs` surfaces page-files +
    companions (was: only files with headline blocks → every page-file invisible to the org
    readers). Verified: `child-note.org` + `my-notes.org` now both surface.
  - **Topology: NON-RESERVED, FLAT** (`child-note.org` / `my-notes.org`), chosen over Fork A's
    `Journals.org` seed: `Journals` is the app's reserved page — seeded programmatically then
    written back as the journals-view machinery, which erases seeded companion content before an
    oracle sees it (empirically confirmed); and the subdir shape trips the pre-existing
    `row_origin.rs` "disjoint root rows" render panic. Fork-B-owned seed; Fork A's seed untouched.
  - GREEN lock (`folder_companion_deinlines_owned_child_page`) + RED-first
    (`fileless_page_writeback_materializes`, `#[ignore]`d: asserts `child-note` owns a file AND
    the companion converges — both fail today).
  - **Data-loss verdict (from the probe, no BugFunnel spawn):** the programmatic-journals
    writeback is a superset MERGE, not an overwrite — a user's non-page content in `Journals.org`
    survives (proven). The fileless-page loss vector is Fork B's own target (B2), covered here.
- **B2 (PRIMARY FIX) — fileless-page materialization sweep + watcher seeding.** Materialize every
  `Page` block that owns no file into its own `<name>.org` via the latent `on_block_changed` →
  `doc_id_to_path` → write path (§2), plus a boot-time sweep (OQ4 RULED) that enumerates
  file-less pages so migration converges without waiting for a fresh edit. Seed `last_projection`
  + tracking on new-file writes (§4.4). DONE: `fileless_page_writeback_materializes` flips GREEN
  (child-note gets `child-note.org`; the companion then de-inlines + converges — fixed-point +
  companion-heading go green as a consequence). **Explicit echo test (RULED, DONE gate):**
  materialize a fileless page, pump the watcher, assert the file is written exactly once and the
  page is NOT re-minted. **Subdir caveat:** a fileless page nested under a folder page
  materializes to `<folder>/<name>.org` — the same subdir shape that trips the `row_origin.rs`
  nested-page render panic. B2 must confirm whether headless materialization hits it; if so,
  coordinate the `row_origin.rs` fix (separately filed) as a B2 dependency. **Flag for review:
  the echo-suppression seeding (§4.4) — the one shared-state touch; a miss is an infinite
  ingest/writeback loop.** Tier: executor → Opus if the loop/subdir proves subtle; verifier on
  the echo test.
- **B1' — extend the ingest-loss guard to the block-driven writeback path (union becomes
  load-bearing). AFTER B2.** The mechanical `SurvivingProjection` refactor already landed
  (`ssqtmslr`) and is now the right substrate — not latent. Add the guard call at the
  `on_block_changed` / `re_render_all_tracked` write sites, with the surviving set = union across
  the file being written + the sibling files the same convergence pass materialized (§4.2–4.3),
  so a legitimate de-inline (block moved to its own materialized file) passes while a genuine
  drop still vetoes + quarantines. **Own red-first: a synthetic block-driven writeback that drops
  a block with no sibling home must VETO** (today that path writes it unguarded — the P0 hole).
  DONE: the new veto test is red-before/green-after; existing `writeback_guard` units unchanged;
  owned + fileless B0 tests stay green. **Flag: P0 data-loss guard — a weakening re-opens row 28
  on a second path.** Tier: **Opus executor + mandatory verifier.**
- **B3 — migration convergence green + keystone wiring.** Fold the companion topology into the
  keystone catalog run (behind a seed gate, like Fork A's `sidebar_page_tag_preserved`); confirm
  the composed keystone is green post-B2/B1' on both owned + fileless variants; add the persisted
  regression seed. Re-evaluate wiring `inv-every-page-has-its-own-file` into the catalog once a
  full-topology seed gives every scaffold page a file (it was dropped from B0 for scaffold-noise;
  the SUT+SUT+RefBlockTree body is designed and recoverable from this commit's history). Tier:
  executor.
- **B4 — hardening + real-vault dry run.** Run the migration on a COPY of the real vault
  (`Journals.org` + `Projects.org`) via a scratch instance (NEVER the user's vault, NEVER port
  8520); assert convergence + zero block loss by id-set diff. Update `docs/Architecture/Sync.md`
  (companion de-inline + fileless materialization + the two-site guard as the documented model).
  Tier: executor; **verifier confirms the id-set diff on the vault copy.**

Sequencing: B0 → B1 → B2 → B3 → B4, strictly serial (all touch the writeback path + oracles).
B1 and B2 could overlap in disjoint files (guard vs. sweep) but both edit `FileSyncController`
convergence ordering, so serialize to avoid a merge on the ordering code.

---

## 7. Risk register + open questions

| # | Risk | Increment | Mitigation |
|---|---|---|---|
| R1 | **Migration data loss** (top risk): companion de-inlined before children exist on disk | B1/B3 | Strict children-first ordering (§4.3); crash-window is duplication, never loss; id-set invariant oracle (§5) |
| R2 | Guard weakened → row-28 silent loss re-opens | B1 | Evidence-based union (§4.2), not a trust-the-controller whitelist; existing guard units must pass verbatim; Opus + verifier |
| R3 | Watcher feedback loop: new child file re-ingested → re-mint / infinite loop | B2 | Seed `last_projection` + tracking at write (§4.4); soak asserting single write per convergence |
| R4 | Fork A/B disagree on "is a page" at the ingest/writeback seam | all | Pin `is_page()` as the sole predicate (§1.3); OQ1 resolved with Fork A before B1 |
| R5 | Path collision / unsafe title (`/`, `:`, case-fold) mints a traversal or overwrites a sibling | B2 | Sanitize at the boundary, fail-loud (§2); collisions suffixed + warned, never silently merged |
| R6 | Oracle mis-attributes child blocks to the companion → false red or masks a real bug | B0 | Oracle file-attribution change reviewed as the behavior spec before prod changes |
| R7 | Nested materialization (page under page under page) mis-nests paths | B2/B3 | `name_chain` already recurses; keystone seeds a 2-level nest; fixed-point oracle covers it |

**Increments flagged for extra (Opus/verifier) review:** B1 (P0 guard) and B2 (loop-prone
shared-state seeding). B0's oracle attribution is a correctness-spec decision worth a senior eye
even though it is test-only.

**RULED (senior review, 2026-07-12) — the four open questions are decided:**
- **OQ1 → RULED: `is_page()` is the sole child-page predicate.** The `Page` tag IS the authority
  signal; Fork A's entire job is making that tag *truthful* (survive the foreign-file reconcile).
  Fork B reads NO second signal — no `block_documents` ownership row, no path/file-existence
  check. Fork A and Fork B meet at exactly `is_page()`. **If any spot is found where the tag alone
  is insufficient, STOP and report — do NOT add a parallel predicate.** (§1.3 is authoritative.)
- **OQ2 → RULED: fail-loud reject of `/` + control chars, suffix-on-collision, no silent
  slugging** (§2 as written). The file name is not the identity (`#+ID:` is), but we still do not
  silently rewrite what the user sees on disk.
- **OQ3 → RULED: the companion retains literally NOTHING of child pages.** No heading, no
  back-link line. LogSeq parity; a backlink, if wanted, is a user-authored `[[…]]` mark
  (links-as-marks), never a writeback artifact.
- **OQ4 → RULED: the boot-time SWEEP is the migration mechanism** (B2, §4.2). Robustness wins for
  migration correctness. Fork A's CDC tag-change deltas are treated as a *bonus accelerant, never
  a dependency* — migration convergence must hold with the sweep alone even if no delta arrives.

---

## 8. Done-criteria (whole stream)
1. A folder companion (`Journals.org`) renders with **no `Page`-tagged child** after settle;
   `Frontends.org`-style emptiness holds for every folder page.
2. Every `Page` block with a resolvable name-chain has exactly one file; no page is double-homed;
   fileless rule-created dates are materialized.
3. Migration of a duplicated vault converges with **zero block loss** (id-set invariant) and the
   guard passes via the union set, never by quarantine.
4. The keystone reproduces the inlining bug (B0 red) and goes green (B3); the migration regression
   seed is persisted.
5. The row-28 guard's data-loss protection is provably intact (its units pass verbatim; a
   vanished block still refuses loud).
6. Fork A and Fork B agree on `is_page()` as the sole child-page predicate; both remain wired
   (A protects ingest + steady state; B fixes writeback + materialization).
