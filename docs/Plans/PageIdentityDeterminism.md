# Page Identity Determinism — Options for Ruling (2026-07-18)

**Status: options document for Martin's ruling. No production code changed.**
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
