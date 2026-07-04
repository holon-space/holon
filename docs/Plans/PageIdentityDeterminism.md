# Page Identity Determinism — Options for Ruling (2026-07-18)

**Status: ACCEPTED / IMPLEMENTED (2026-07-18).** Ruling: the §4 recommendation —
**Option 2 (path-hash id for new writes) + a bounded slice of Option 3
((name,parent) repair)**. The guard PBT is un-`#[ignore]`d and green. See
"Ruling & implementation notes" at the end for what shipped and the local
decisions taken where this memo left detail open.

Original framing (kept for the record) —
A RED guard PBT is landed (`#[ignore]`d) —
`crates/holon/tests/create_page_from_link.rs::inv_page_name_unique_converges_across_peers`.

Question: the user's screenshot shows **two pages named "Areas" coexisting in
the sidebar**. Why can duplicate same-named pages exist, and what is the correct
page-identity scheme that makes them converge?

Grounding: every claim carries a file:line citation from the current worktree.

---

## 1. Root cause — page identity is not a function of the name

A page is a `Page`-tagged `block_raw` row. Its identity is its block id, which is
the CRDT (Loro) merge key. There is **no uniqueness constraint** on
`(name, parent)`. Duplicates therefore survive whenever two page-creating events
mint **different ids for the same logical page** — because the merge is a union
by id, both rows persist.

The reproduction (RED PBT, shrinks to `name = "a"`): two independent peers each
run the production lazy-page-creation op `create_page_from_link("a")`; each mints
a **random** id; the merged vault holds two `Page` blocks named "a".

### Divergence table — id scheme per creation path for a page named "Areas"

| Path | Site | Id scheme | Deterministic? |
|---|---|---|---|
| Link target computation (`[[Areas]]` parse) | `crates/holon-api/src/link_parser.rs:157-158` | `deterministic_entity_id("block", normalize_for_hash("Areas"))` — blake3→UUID | **Yes** (but only computes the *target* id; not used to create the page) |
| Lazy page creation (click / link-create) | `crates/holon/src/core/sql_operation_provider.rs:2030` | `format!("block:{}", uuid::Uuid::new_v4())` | **No — random per call** |
| Org-file ingest (file/heading page) | `crates/holon-org-format/src/parser.rs:50` `generate_file_id` → `file:<rel-path>`, resolved to `block:<uuid>` by FileSyncController (`parser.rs:42-44`) | file-path id, then a per-peer `block:<uuid>` | **No** |
| Within-store dedup (papers over, per store) | `sql_operation_provider.rs:1046-1076` `resolve_page_name` → name lookup, `ORDER BY b.id LIMIT 1` | name→id lookup, arbitrary pick on tie | n/a — cannot see an unmerged peer's page |
| Link→page resolution at write | `sql_operation_provider.rs:1021-1035` `block_link_statements` | resolves by **name** (`resolve_page_name`), **ignoring** the deterministic `target_id` the parser already computed | n/a |

Three independent id schemes for the same page name, none agreeing. The parser
computes a deterministic id and then **nobody creates the page with it**; both
the click-create op and org-ingest mint fresh ids; the only thing keeping a
single store consistent is a name lookup that structurally cannot see another
peer. That is why offline peers, or an org-ingested page plus a link-created
page, produce duplicates.

---

## 2. Why this is a ruling, not a one-line fix

The obvious patch — make `create_page_from_link` call `deterministic_entity_id`
instead of `Uuid::new_v4()` — forces a choice on **what the id hashes**, and that
choice locks in page-hierarchy semantics that are explicitly unruled:

- **row-25 dedupe fork**: is a page keyed by `name`, by full `path`, or by
  `(name, parent)`? `link_parser` today hashes the **full path string**
  (`normalize_for_hash(target)`, so "Areas" ≠ "Life/Areas"), but the links-ruling
  says *path is a typing/resolution hint only; ambiguity is represented state* —
  which argues against path being identity.
- **vault-compat O1-O5**: org-ingested pages carry **file-path** identity by
  design (`generate_file_id`). Making them adopt name-hash identity is the O1-O5
  fork; if they *don't*, an org page + a link page for the same name still
  diverge — so fixing `create_page_from_link` alone does **not** close the bug.
- **page-hierarchy PARKED** (interim: no pages under non-pages). Name-global
  identity forecloses nested same-name pages permanently.
- **pre-existing duplicates**: the user already has two "Areas". No id scheme
  retro-merges them; that needs a migration/merge-time dedup decision.

---

## 3. Options (pick the identity key; §3.4 is orthogonal repair)

### Option 1 — Global name identity: `id = hash(normalized leaf name)`
All paths mint `deterministic_entity_id("block", normalize_for_hash(leaf))`.
- **For**: simplest; literally enforces "no two pages share a name"; matches the
  flat interim model (no pages under non-pages).
- **Decisive tradeoff**: permanently forecloses two legitimately-distinct pages
  named "Areas" under different parents. If nested same-name pages ever become a
  product goal, this is a data-migration to undo.

### Option 2 — Path identity: `id = hash(normalized full path)` (reuse the parser's scheme)
Align `create_page_from_link` (and org-ingest page resolution) to the id
`link_parser.rs:158` **already computes**, keyed on the accumulated path.
- **For**: zero new scheme — the link layer's `target_id` and the created page's
  id finally coincide, so links resolve by **id** (robust) instead of the fragile
  name lookup; distinct parents keep distinct pages.
- **Decisive tradeoff**: path is a *hint*, not identity (links-ruling), so two
  peers reaching the same page via different typed paths ("Areas" vs
  "Life/Areas") still diverge — and org-ingest's file path is yet another path
  string that won't equal a typed link path unless O1-O5 unifies them.

### Option 3 — Keep ids opaque; enforce a `(normalized name, parent_id)` unique merge
Add a uniqueness guard on `(name, parent)` with a deterministic survivor rule
(e.g. lowest id wins) that rewrites references at merge/write time.
- **For**: the **only** option that also repairs already-duplicated vaults (the
  user's current state) and unifies *all* creation paths — link, org-ingest, MCP
  — without each having to adopt the same hash.
- **Decisive tradeoff**: heaviest; needs a CRDT-aware dedup + reference-rewrite
  pass and a survivor rule that is itself deterministic across peers; it is the
  reconciliation-semantics fork the reseed workstream already touches.

### 3.4 — Orthogonal, needed regardless
Add a `UNIQUE(content, parent_id) WHERE tag='Page'`-style guard (or a projection
assertion) so a future regression fails **loud** at write, and delete
`resolve_page_name`'s `LIMIT 1` arbitrary-pick once identity converges.

---

## 4. Recommendation

**Option 2 for new writes + a bounded slice of Option 3 for the existing vault.**

- Forward-fix: route `create_page_from_link` and the FileSyncController
  `file:→block:` resolution through the **same** `deterministic_entity_id`
  the parser already uses, so the id a link points at *is* the id the page gets.
  This removes the largest source (random `Uuid::new_v4()`) and makes link
  resolution id-based.
- Repair slice: a one-shot `(name, parent)` dedup with lowest-id-survivor to
  collapse the user's existing duplicate "Areas".

**What the recommendation rests on** — and why it still needs your ruling: it
assumes the identity key is **path/parent-scoped, not global name** (Option 2
over Option 1), and it assumes org-ingested pages **should** adopt name/path-hash
identity (the O1-O5 fork). Both are exactly the calls the links-ruling and O1-O5
left open. If nested same-name pages are a non-goal forever, Option 1 is simpler
and strictly closes the invariant; if the org file must remain the identity
authority for its pages, Option 2 cannot converge an org page with a link page
and Option 3 becomes mandatory.

The RED guard PBT stays `#[ignore]`d until this is ruled; removing the attribute
replays the stored regression as the green gate.

---

## 5. Ruling & implementation notes (2026-07-18, ACCEPTED / IMPLEMENTED)

**Ruling: §4 recommendation.** Path-hash identity for all NEW page writes, plus
a bounded `(name, parent)` repair for the already-duplicated vault.

### 5.1 The single constructor (parse-don't-validate)

A new `PageId` newtype in `crates/holon-api/src/link_parser.rs` is the ONE
sanctioned way to mint a new page id:

```rust
PageId::for_path(path) == deterministic_entity_id("block", &normalize_for_hash(path))
```

- **Hash input** = the page's full `/`-joined path, root→leaf (`"Life/Areas"`),
  **segment-trimmed** (split on `/`, trim each segment, rejoin) and then run
  through `normalize_for_hash` (lowercase, collapse internal whitespace). Segment
  trimming is the single canonicalization site, so `"Areas / Sub"` and
  `"Areas/Sub"` mint the SAME id — the link parser's optimistic id can never
  drift from the writer's (H2). `for_path` is **fail-loud**: an empty segment
  (leading/trailing or doubled `/`) is a malformed path and returns `Err`,
  never a silent `a//b`→`a/b` collapse.
- Pages are always `block`-scheme, so scheme is NOT a caller parameter — that
  structurally closes the `EntityName::named` scheme-bypass class (memory note).
  A non-page scheme (`person:`) is simply not a `PageId`.
- `EntityUri`, not a bare `String`, is threaded out (`into_entity_uri` /
  `as_entity_uri` / `as_str`).

### 5.2 The three write paths, unified

1. **Link parser** (`classify_link`, `link_parser.rs`): the block-scheme
   `target_id` now routes through `PageId::for_path`, so a `[[Areas]]` link's
   target id is *exactly* the id the page will be created with — link
   resolution is id-aligned by construction, not just name-lookup luck. (Non-
   block schemes keep the generic `deterministic_entity_id`.)
2. **Click / lazy link-create** (`create_page_from_link`,
   `sql_operation_provider.rs`): replaced `format!("block:{}", Uuid::new_v4())`
   with `PageId::for_path(&seg_path)`, where `seg_path` is the accumulated
   root→segment path. This was the largest divergence source (random UUID).
3. **Org-file ingest** (`get_or_create_by_name_chain`, `sync_ports.rs`):
   replaced `EntityUri::block_random()` with
   `PageId::for_path(&accumulated_name_chain)`. This is the FileSyncController
   `file:→block:` resolution the memo named: a file-page and a link-created
   page for the same path now converge on one id. `#+ID:`-carrying files keep
   their authoritative id (respected, unchanged).

### 5.3 Rename semantics (local decision)

Identity is assigned **once, at creation**, from the then-current path, and
stored on the CRDT. A later **rename is an ordinary edit to the existing
entity** — the id does NOT re-mint, so history/links survive. A *new* page
created later under the new name gets a new id; that is correct (it is a
different logical page). Convergence is over independent *creation* of the same
page (the PBT), not over post-hoc renames.

### 5.4 Bounded repair — `SqlOperationProvider::dedup_pages`

One-shot, fail-loud collapse of existing `(content, parent_id)` duplicate
`Page` groups:

- **Survivor rule**: lexicographically-lowest block id in the group.
  Deterministic and peer-stable (every merged peer sees the same id set →
  same survivor, no coordination).
- **Rewrite**: re-home losers' children (`block_raw.parent_id`) and inbound
  links (`block_links.resolved_id`) onto the survivor, delete the losers'
  OUTBOUND `block_links` rows (redundant with the survivor's; would otherwise
  orphan on the row delete), then delete loser rows + tags — all in one
  transaction (no partial apply).
- **Bounds (all fail loud, never a silent mass-rewrite)**:
  `MAX_DUPES_PER_GROUP = 16`, `MAX_DUP_GROUPS = 64`, and an ancestor-cycle walk
  bounded at `MAX_ANCESTOR_DEPTH = 512` that errors if a loser is an ancestor
  of its survivor or the walk fails to terminate.
- Scope: this is the SqlOnly-store repair (the mode the guard PBT and the
  user's vault run in). It is a callable maintenance op, not an automatic
  merge-time hook — deliberately, so it stays observable and one-shot.

### 5.5 Deliberately OUT of scope (open, unchanged)

- **Heading pages inside a file** (`extract_or_generate_id`, `parser.rs:655`)
  still mint a UUID then write it back as `:ID:` — the org-file-as-authority
  model (vault-compat O1-O5). They reach per-file determinism via write-back;
  giving them path-hash identity needs the heading's ancestor-title chain and
  risks cross-file same-name collisions. Left as the O1-O5 fork intended.
- **`resolve_page_name`'s `ORDER BY … LIMIT 1`** arbitrary tie-break is left in
  place as a within-store safety net; with convergent ids it should rarely see
  a tie. §3.4's hard `UNIQUE(content,parent)` DB constraint was deliberately
  NOT added — under CRDT union-by-id it could reject a legitimate merge; the
  green invariant PBT is the regression gate instead.
- **Loro-mode (Full-store) dedup** beyond the SQL projection is not addressed
  here.
