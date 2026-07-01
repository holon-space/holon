# CONTEXT — Holon Ubiquitous Language

The domain-language source of truth for Holon. When code, docs, and conversation
disagree on what a word means, **this file wins** — change it deliberately, then make
the code follow.

Derived from the Block-lifecycle Event Storm
([docs/Architecture/BlockEventStorm.md](docs/Architecture/BlockEventStorm.md)) and the
ADRs in [docs/adr/](docs/adr/). Read those for the *why*; read this for the *words*.

How to use it:
- Before naming a new type, field, or op, check the canonical term below and reuse it.
- If a concept here has several names, the **bold** one is canonical; the rest are
  either legacy (being removed) or context-local dialect (allowed only inside that
  context's anti-corruption layer).
- New synonyms are a smell. If you need one, add a row to §4 with a reason, or you are
  growing the next H7.

---

## 1. Bounded contexts

`Block` is one concept spoken in six dialects, bridged by anti-corruption layers
(ACLs). Each context owns its own words; the words only cross at a translation seam.

| Context | Crate(s) | A block is called… | Identity is… |
|---|---|---|---|
| **Authoring / Format** | `holon-org-format`, `holon-orgmode`, `holon-markdown` | headline / heading / drawer / fence / frontmatter | **bare id** on disk |
| **CRDT of record** | `holon-loro` | LoroTree **node** + meta `LoroMap` | `STABLE_ID` meta key |
| **P2P transport** | `holon-loro` (Iroh) | version-vector delta bytes | per-share `stable_peer_id` |
| **Read projection** | `holon-turso` | **row** (`block` matview / `block_raw`) | `id` column |
| **Interaction / Render** | `holon-frontend`, `frontends/*` | **row** → ViewModel → widget | `EntityUri` on the intent |
| **Shared kernel** | `holon-api`, `holon-core` | **Block** | `EntityUri` (`block:`) |
| **Orchestration** | `holon` | — (routes the above) | — |

**Authority direction** (the spine): Org/Markdown → command bus → consolidator →
**Loro (write-of-record)** → projection → **Turso (read model)** → CDC → ViewModel →
widget → user intent → back to the command bus. See the storm for the full loop.

---

## 2. Core domain vocabulary (the shared kernel)

These terms mean the same thing everywhere. Use them verbatim.

- **Block** — the universal content unit. A flat, row-shaped entity (`holon-api`
  `block.rs`). Everything is a block, including pages. Fields: `id`, `parent_id`,
  `content`, `content_type`, `properties`, `marks`, edge fields `tags`/`requires`,
  timestamps.
- **EntityUri** — the canonical block identity. Scheme + bare id, e.g. `block:abc-123`.
  The root parent is the sentinel `sentinel:no_parent`.
- **Page** — a block that is a top-level document/file. **Canonical encoding: the
  `"Page"` tag** (`PAGE_TAG`) / `Block::is_page()`. (See §4 for the deprecated
  encodings still in the tree.)
- **ParentRef** — conceptual name for `Block.parent_id: EntityUri` + the `CHILD_OF`
  edge. There is no `ParentRef` *type*; do not invent one without an ADR.
- **ContentType** — `Text` | `Source` | `Image`. Also fixes sibling-order group
  (Source/Image sort before Text — ADR 0005).
- **TaskState** — `{ keyword, category: Active | Done }`. **Stored inside
  `properties`, not as a column or a typed field.** (This is what makes H3 a bug.)
- **Edge field** — a block field backed by a relationship rather than a scalar:
  `tags` (→ `block_tags`) and `requires` (→ `block_requires`). Set-shaped; persisted
  as junction rows, hydrated back on read.
- **Sibling order** — the **canonical representation is the ordered child list**
  (children-as-ordered-list, ADR 0005). It is *not* a `Block` field. The fractional
  index (Loro) and the `sort_key` column (Turso) are **per-system materializations**
  of that ordered list, not the authority themselves. Positional *intent* is expressed
  as `after_block_id` (a predecessor block id, never a sort key or index).
- **Change** — a domain event about a block: `Created` | `Updated` | `Deleted` |
  `FieldsChanged`, tagged with **ChangeOrigin** (`Local` | `Remote`) for echo
  suppression.
- **Operation / OperationIntent** — a command to mutate a block (`{entity_name,
  op_name, params}`). **ChangeOp** is the only *typed* mutation enum
  (`Create`/`SetField`/`Relocate`/`Delete`); prefer it over stringly op-names where
  you can.
- **EventOrigin** — provenance tag on an inbound write (`Org`, …) that tells the
  sync gate to *apply* rather than drop it as an echo. Distinct from `ChangeOrigin`.

---

## 3. Per-context dialect (allowed only behind the ACL)

These words are legitimate **inside one context**. They must be translated to §2
terms at the boundary — never leaked outward.

**Authoring / Format**
- **bare id** — an id with no scheme prefix, as stored on disk. The parser *adds*
  `block:`; the renderer *strips* it. This translation is the format ACL's core
  invariant (see `ORG_SYNTAX.md`).
- **headline / drawer / property / planning / fence / frontmatter / wikilink** — org
  & markdown surface syntax. Map to Block / properties / `content_type=Source` etc.
- **FileFormatAdapter** — the shared trait org *and* markdown implement; the seam
  where text becomes Blocks.

**CRDT of record (Loro)**
- **node / `TreeID`** — a LoroTree node = one block. `TreeID` is **peer-local** — not
  a stable cross-peer identity. Translate via `STABLE_ID`, never persist a `TreeID`.
- **meta `LoroMap`** — the per-node container holding block fields.
- **container vs field** — a *container* is a CRDT object (`LoroTree`/`LoroMap`/
  `LoroText`, merges); a *field* is a key in the meta map (LWW). `content_raw` is a
  `LoroText` container (character-merges); `properties` is a single string field (does
  not — H3).
- **fractional index** — Loro's *materialization* of the canonical ordered child list
  (§2 Sibling order); a per-system representation, not the order authority itself.

**P2P transport (Iroh)**
- **version vector (VV) / delta / snapshot** — what actually crosses the wire (Loro
  update bytes), never Blocks.
- **shared tree / ticket / ALPN / stable_peer_id** — sharing-transport vocabulary.

**Read projection (Turso)**
- **row** — a `block` matview / `block_raw` row. One block, but **one block can
  project to many rows** across panels.
- **`block_raw` vs `block`** — writes go to the `block_raw` base table; reads/CDC go
  through the `block` matview (which hydrates edge fields). **CDC fires from matviews
  only, never base tables.**
- **`relation_name`** — the *matview's* generated name on a `RowChange`, not "block".
- **ROWID** — SQLite's internal rowid. **A trap: reusable after DELETE — never use it
  as a block identity or widget key.** Identity is the `id` column.

**Interaction / Render**
- **ViewModel** — ⚠ two distinct types: **`ReactiveViewModel`** (the live MVVM node)
  vs **`ViewModel`** (its frozen snapshot for tests/MCP). Always qualify which.
- **widget** — overloaded ×3: a `ViewKind` tag, a `shadow_builders/*` builder, and a
  native `AnyElement`. Say which layer you mean.
- **driver ladder** — `GpuiUserDriver ⊐ ReactiveEngineDriver ⊐ DirectUserDriver`; the
  layered write path that turns a gesture into an `OperationIntent`.
- **Cell / Mutable** — local UI state (expand, scroll, cursor) that is Tier-1 and
  **never round-trips** to the backend.

---

## 4. Synonym & deprecation registry (the cleanup list)

Each row is a place the language drifted. **Canonical** is the target; everything
else should converge or die. These are the named hotspots from the Event Storm.

| Concept | Canonical | Deprecated / dialect still in tree | Action | Storm ref |
|---|---|---|---|---|
| a block is a page | **`"Page"` tag** / `is_page()` | `is_document()`; `doc:` URI scheme (still in `holon-markdown`, `link_parser`, `backend_engine`, `prql_stdlib.prql`, PBT); `set_is_document` op | finish `doc:` elimination; collapse to the tag | H7 |
| block mutation | **`ChangeOp`** (typed) | stringly `op_name`; `Operation`; `OperationIntent`; `BlockDiff` | tighten toward typed; document the rest as boundary forms | H2 |
| sibling order | **ordered child list** (ADR 0005) | fractional index (Loro materialization); `sort_key` column (Turso materialization); `SnapshotBlock.sort_key`; `sequence` (legacy) | treat index/`sort_key` as per-system materializations of the ordered list; retire `sequence` | — |
| the rendered unit | **Block** + **row** (distinct) | conflating "block" and "row" | keep distinct: block = entity, row = projection | — |
| live vs frozen VM | **`ReactiveViewModel`** vs **`ViewModel`** | using "ViewModel" unqualified | always qualify | — |
| widget | **native element** (`AnyElement`) | `ViewKind` tag; `shadow_builders/*` — "widget" is overloaded ×3 | qualify which layer is meant | — |
| change event | **`Change<T>`** | `RowChange` (Turso-tagged); `ChangeData`; `UiEvent::Data` | treat tagged forms as boundary-local | — |
| block identity | **`EntityUri`** / `STABLE_ID` | bare id (disk); `TreeID` (peer-local); SQLite ROWID (trap) | never persist `TreeID`/ROWID as identity | — |

---

## 5. Known model tensions (open decisions, not yet resolved)

These are where the *language itself* over-promises today. Flagged so nobody treats
them as settled.

- **"Loro is the write-of-record" leaks.** RichText marks are a Phase-1 stub
  persisted only in Turso's `marks` column (H4); `properties` is a single-key string
  inside the meta `LoroMap`, so it gets blob-level LWW instead of per-property merge
  (H3 — concurrent `TODO` vs `PRIORITY` edits clobber). Until resolved, the honest
  phrasing is **"Loro owns structure; SQL currently owns some presentation/property
  state."** Decide: make Loro total (nested `LoroMap` for properties, marks in Loro),
  or rename the principle.
- **Two Block deserializers** (H1). The serde path drops edge fields (`tags`/
  `requires` are `#[serde(skip)]`); only the matview `TryFrom<StorageEntity>` is
  complete. Not purely latent: `SnapshotBlock` embeds a `Block` and is serde-persisted
  into the Loro projection sidecar, so the sidecar round-trips blocks with empty edge
  fields — a cold boot seeded from it can re-emit spurious tags/requires writes (see
  H1 in the storm for the seeding-path caveat). Worth a `StoredBlock` newtype that only
  the matview path can mint. Removing `Deserialize` from `Block` is blocked
  (`SnapshotBlock` needs it).
- **Sharing security is documented but unimplemented** (H5). ADR 0003 describes
  capability auth / revocation that does not exist yet. Don't cite it as shipped.

---

## See also
- [docs/Architecture/BlockEventStorm.md](docs/Architecture/BlockEventStorm.md) — the
  full event timeline, hotspots H1–H11, and file:line references.
- [docs/adr/0003-all-in-lorotree-architecture.md](docs/adr/0003-all-in-lorotree-architecture.md)
  — the CRDT-of-record architecture.
- [docs/adr/0005-children-as-ordered-list.md](docs/adr/0005-children-as-ordered-list.md)
  — sibling order / fractional index.
- [docs/Reference/ORG_SYNTAX.md](docs/Reference/ORG_SYNTAX.md) — the bare-id↔scheme
  boundary convention.
