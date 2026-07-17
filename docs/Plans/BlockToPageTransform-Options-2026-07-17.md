# Block → Page Transform — Design Options (2026-07-17)

*Decision-options doc for Martin's morning review. Not an implementation. Every
load-bearing claim is cited `file:line`. Read
[docs/Architecture/Model.md](../Architecture/Model.md) first — this doc assumes
its five-layer model and invariants (1)–(12).*

## The dogfood ask

"Turn a block into a page." Concretely three effects the user wants from one
gesture:

1. The block becomes a **page** — a first-class thing with its own `.org` file
   on disk.
2. The block is **removed from its original location** (its old inline position
   in the parent file).
3. Its **content and children** come along (a page with no body is useless).

This is the LogSeq "turn this bullet into a page" move. The whole design
question is *what identity and what links survive the move*, because Holon's
write-back guard treats an unexplained disappearance of a block from a file as
**data loss and vetoes the write** (see §"The hard constraint" below).

---

## Ground truth: how a "page" is represented today

There is **no `doc:` scheme and no document entity type**. A page is an ordinary
block carrying the `"Page"` tag:

- `PAGE_TAG = "Page"` — `crates/holon-api/src/block.rs:394`.
- `Block::is_page()` checks the tag list — `block.rs:399`;
  `Block::set_page(bool)` toggles it — `block.rs:404`.
- The `doc:` scheme is **retired** (H7, 2026-07-02) —
  `docs/Reference/ORG_SYNTAX.md:9-11`: "Pages are ordinary blocks tagged
  `Page`." `EntityUri` offers only `block()`, `file()`, and sentinel
  constructors — no `doc:` equivalent (`crates/holon-api/src/entity_uri.rs`).

So "make this a page" is, at the data level, **`set_page(true)` plus getting the
block to materialize its own file**. A page materializes into its own
`<name-chain>.org` file; the file path is computed by walking the parent chain
(`name_chain`) via the `AliasRegistrar` /
`FileSyncController::doc_id_to_path` — `file_sync_controller.rs:3319`,
`sync_ports.rs:179-184`.

### The hard constraint: no pages under non-pages (row-30 / SiblingGrounding hard-veto)

Commit `079014efeb` ("fix: No pages under non-page parents") added an **interim
topology rule**: every ancestor of a page, up to the root, must itself be a page.

- Invariant `inv-no-page-under-non-page` walks the parent chain and fails if any
  ancestor is a non-page —
  `crates/holon-integration-tests/src/pbt/invariants/bodies/no_page_under_non_page.rs:54,66-111`.
- It is enforced at write-back by the **hard veto**: when a block disappears
  from a source file, `writeback_sibling_grounding` calls `doc_id_to_path` to
  find where it went (`file_sync_controller.rs:3138-3200`). If `name_chain`
  fails loud — the disappeared block would be a page under a non-page — the
  block is **UNRESOLVABLE** (`SiblingGrounding.unresolvable`,
  `file_sync_controller.rs:101-104`) and the write is **ABORTED + quarantined
  regardless of the drop count** (`tripwire_mass_truncation:3243-3262`). This is
  the fix for the first-boot 6,245-line `Projects.org` destruction (749
  name_chain failures that fell under the 25% mass-truncation threshold and
  truncated the file anyway).

**Why this dominates the design:** a block→page transform *is exactly* a block
disappearing from its source file and reappearing as its own page file. The
write-back guard must be able to **ground that absence** against the new sibling
file. Two ways to fail:

1. The new page is placed under a non-page ancestor → `name_chain` fails →
   hard veto → the source-file write is refused and quarantined. Any option that
   promotes a deeply-nested inline block to a page **must also ensure every
   ancestor is a page**, or place the page at a legal anchor.
2. The new page file is not yet on disk when the source file re-renders →
   ungrounded absence → mass-truncation tripwire may fire.

The transform therefore is **not** a single field flip; it is an ordered,
grounded operation: create-the-page-file *first*, then let the source file
re-render its now-legitimate absence.

### Children

Children are held by `parent_id`. Reparenting is **not** an inline
`set_field("parent_id")` — that is a hard error (Model.md invariant 3). It goes
through the positional op `move_block { id, parent_id, after_block_id }`
(`block_write_field.rs:48,88-92`; dispatched via
`repository.rs:225-232 move_block(id, new_parent_id, after_id)`). "Move children
with the block" means either (a) they keep their existing `parent_id` (the block)
and simply travel because their parent moved, or (b) each child is re-`move_block`ed.
Option (a) is free if the block itself is the thing being retagged in place;
option (b) is required if we create a *new* page block and leave the old one behind.

### Backlinks / junctions (block_links.resolved_id)

Links live as `Link` marks on block content, projected to the `block_links`
junction at the SQL write boundary (`block_links.sql`):

```
block_links(source_block_id, target, kind, resolved_id)  -- PK (source, target, kind)
```

- `resolved_id` = the target's block id once resolution succeeds; `NULL` =
  dangling. `kind` ∈ `page` / `block` / `tag`. **No FK** on `resolved_id` or
  `source_block_id` — soft targets, lazy page creation (`block_links.sql:7-17`).
- Crucial consequence for identity: **if the block keeps its id when it becomes
  a page, every existing `((block-ref))` to it still resolves** — `resolved_id`
  is unchanged. If the transform mints a *new* id for the page, every inbound
  `resolved_id` pointing at the old id goes stale/dangling until re-resolved,
  and any `page`-kind link that named it now resolves to a different block than
  the `block`-kind refs. This is the single biggest identity decision below.

### Undo

- `UndoEntry` holds `ops: Vec<Operation>` (forward) and
  `inverse_ops: Vec<Operation>` (inverse, **stored, never recomputed**) —
  `crates/holon-core/src/undo.rs:125-141`. The `Vec` is explicitly "so a future
  compound split/join is one entry" (`undo.rs:128`). A composite transform *can*
  be one undo entry.
- **But** replay is precondition-fingerprinted on `(entity, field)` `FieldDelta`s
  (`undo.rs:71-115`); a step with no clean field-delta inverse cannot be
  reversed. Per project memory ([undo-ruling]), provider coverage is the gap:
  `create/delete/set_field/split/join/cycle` are currently irreversible (no
  inverse provider), one shared stack, `cmd+z` unbound. So a transform assembled
  from create-page + N×move_block + junction rewrites is reversible **only to the
  extent each constituent op has an inverse provider** — today several do not.
  Structural ops also close the coalescing group and stand alone (`undo.rs:160-173`).

### How LogSeq does it (UX reference)

In LogSeq, "turn a bullet into a page" (or first `[[...]]`-referencing it) keeps
the bullet in place and the page *is* the bullet's linked-reference aggregation —
the original block stays where it is and a page reference remains behind. There
is no "content is moved out and the origin is emptied" gesture; the page's body
is authored on the page, and the origin keeps a link. This is the LogSeq analogue
of **Option B** below, not A or C.

---

## The options

### Option A — Transform-in-place (retag + de-inline the same block)

**What it concretely is:** the selected block *keeps its id*. We
`set_page(true)` on it, ensure its ancestor chain is all-pages (promoting or
re-anchoring as needed), and let the write-back machinery de-inline it: the block
now materializes its own `<name-chain>.org` file, and its absence from the
original file is **grounded by that new sibling file** (`writeback_sibling_grounding`
resolves `doc_id_to_path` → the new page file, absence explained, no veto). Its
children keep their `parent_id` and travel with it for free.

*Worked example:* under `Projects.org` there is a heading `** Rust rewrite` with
id `abc`. User invokes "make page". We emit `set_page(true)` on `abc` (and, if its
parent `* Backend` is not a page, either promote `* Backend` to a page too or
reject with a clear message). On next consolidation `abc` renders into
`Backend/Rust rewrite.org` (name-chain), and `Projects.org` re-renders without
`** Rust rewrite`; the guard grounds that absence against the new file. Every
`((abc))` backlink still resolves — `resolved_id = abc` unchanged.

**Decisive tradeoff:** *cleanest identity, hardest topology.* Because the id is
preserved, backlinks and `block_links.resolved_id` need **zero** rewrites — the
strongest correctness property. The cost is the no-page-under-non-page rule: an
inline block deep under non-page headings cannot become a page without either
promoting its whole ancestor chain to pages (surprising, cascading file
materializations) or re-anchoring it to a legal parent (moving it, which
re-introduces the move problem). A/B/C differ almost entirely on how they pay
this bill.

**A recommendation for A rests on:** the transform being offered *only* where the
ancestor chain is already all-pages (or the UI making ancestor-promotion an
explicit, previewed consequence), **and** confirming that de-inlining an
in-place block reliably grounds through `writeback_sibling_grounding` in the
create-file-first ordering (needs the file materialized before the source
re-renders — else the mass-truncation tripwire, `:3264-3280`, can fire on the
transient ungrounded absence).

### Option B — Create page + replace block with a link (LogSeq style)

**What it concretely is:** we mint a **new** page block (new id `P`), move the
original block's content/children under `P`, and **leave a link behind** in the
original location — the origin block's content becomes `[[P]]` (or the origin
stays and gains a reference). The origin file is never "emptied"; it keeps a
resolvable reference, so write-back sees a *content change*, not a drop.

*Worked example:* `** Rust rewrite` (id `abc`) under a non-page `* Backend`.
We create page block `P` tagged `Page` at a legal page anchor (e.g. vault root
or a chosen parent page), `move_block` the children from `abc` to `P`, rewrite
`abc`'s content to `[[Rust rewrite]]` (a `page`-kind link with `resolved_id = P`).
`Projects.org` still contains `abc`, now rendering as a link line — **no absence,
no veto**. The new `Rust rewrite.org` is grounded as a fresh page file.

**Decisive tradeoff:** *sidesteps the hard-veto entirely, at the cost of identity
churn and a semantic change.* Because the origin never disappears, the
SiblingGrounding veto and the no-page-under-non-page rule are **not on the
critical path** — the page can be anchored anywhere legal, independent of where
the origin sits. But the page gets a **new id**, so this is *not* "the same
thing is now a page": inbound `((abc))` block-refs still point at the now-stub
origin, not the page; the user must understand that the origin became a pointer.
This is precisely LogSeq's model, and it matches the `block_links` design (soft
targets, lazy page create, content-carries-the-link — `block_links.sql`,
[links-ruling]).

**A recommendation for B rests on:** accepting LogSeq semantics as the product
intent (origin becomes a reference, page is a new entity), and deciding what
happens to inbound block-refs of the origin — leave them on the stub (LogSeq
behavior) or rewrite them to `P` (extra junction maintenance, and there is **no
FK** to cascade it — `block_links.sql:7-11` — so it is an explicit matview/rewrite
pass, not free).

### Option C — Create page + move content + leave nothing

**What it concretely is:** the maximal move — a new page block `P`, all content
and children moved to it, and the origin block **deleted** from its original
location (nothing left behind).

*Worked example:* `** Rust rewrite` (id `abc`) becomes page `P`; children
re-`move_block`ed to `P`; `abc` deleted. `Projects.org` re-renders **without**
`abc` and without a link.

**Decisive tradeoff:** *cleanest result on disk, worst interaction with the guard
and with identity.* This is the exact shape the hard-veto is built to stop: a
block vanishes from a file. To not be vetoed, the deletion of `abc` must be a
**sanctioned removal** (its id in the triggering delta's `Remove` set —
`tripwire_mass_truncation` sanctions delivered `Remove` ops,
`file_sync_controller.rs:3202-3211`) *and* the content must be provably grounded
in the new page file. Meanwhile `abc`'s id is gone: every inbound `((abc))` is
now dangling with no origin and no automatic re-point. Undo is the worst here
too — a delete with no inverse provider (memory: delete is irreversible) means
the composite cannot be cleanly reversed.

**A recommendation for C rests on:** there being no inbound references to the
origin worth preserving (e.g. a scratch block being promoted), **and** the
delete being routed as a sanctioned `Remove` so the guard grounds it — otherwise
C is the option most likely to trip the row-30 quarantine.

---

## Comparison

| Dimension | A — in-place retag | B — page + link (LogSeq) | C — page + move + delete |
|---|---|---|---|
| Page id | **same as block** | new id `P` | new id `P` |
| Inbound `((refs))` / `resolved_id` | **all still resolve** | resolve to stub origin (or rewrite) | **dangle** (origin gone) |
| Origin location | de-inlined → own file | keeps a `[[P]]` reference | emptied / deleted |
| SiblingGrounding hard-veto | **on critical path** (absence must ground) | **avoided** (no absence) | **on critical path** (delete must be sanctioned) |
| no-page-under-non-page rule | **must satisfy for origin's own chain** | page anchored independently | must satisfy for page anchor |
| Children | travel for free (same parent) | N×`move_block` to `P` | N×`move_block` to `P` |
| Undo | 1 entry: `set_page` + (promotions) | composite: create+move+content-edit | composite incl. **delete (irreversible)** |
| Matches user "same thing is now a page" | **yes** | no (origin is a pointer) | partial (content yes, identity no) |
| Matches LogSeq UX | no | **yes** | no |

---

## Recommendation (for Martin to rule)

**Ship B as the default gesture, keep A as a power move where the chain is
already all-pages.** Reasoning:

- B is the only option that **structurally avoids the SiblingGrounding hard-veto
  and the no-page-under-non-page rule** — it never creates an unexplained absence
  and anchors the page independently of the origin's messy inline location. That
  makes it the *safe, shippable-now* path against tonight's veto machinery.
- B matches the LogSeq mental model users are migrating from (origin keeps a
  reference), and it is exactly what the `block_links` substrate was built for
  (soft page targets, content-carries-the-link).
- A is the *correct* semantics ("the same thing is now a page", zero backlink
  churn) and should be offered where it is cheap — i.e. the block's ancestors are
  already pages — but as a first cut it collides with the interim topology rule
  and needs the create-file-first ordering proven against the tripwire.
- C is not recommended as a default: it maximizes both veto risk and identity
  loss, and its delete is irreversible under today's undo providers.

This recommendation **rests on** the product decision that "turn into page"
means LogSeq's "a reference stays behind," not "the origin is emptied." If Martin
wants the origin *emptied* (true move), the ranking flips toward A (preserve id,
de-inline) and the no-page-under-non-page rule must be resolved first (it is
currently PARKED — [Rulings 2026-07-13] page-hierarchy).

---

## Open questions requiring a ruling

1. **Identity:** does "turn into page" preserve the block's id (A/true-move) or
   mint a new page id and leave the origin as a reference (B/LogSeq)? This single
   choice decides whether inbound `((refs))` survive untouched.
2. **Origin residue:** reference-left-behind (B), de-inlined-to-own-file (A), or
   nothing (C)?
3. **Ancestor promotion:** if the block sits under non-page headings, do we
   (i) auto-promote the whole ancestor chain to pages (cascading file
   materializations), (ii) re-anchor the page to a legal parent (a move the user
   may not expect), or (iii) refuse with a clear message? This is gated on the
   PARKED page-hierarchy ruling.
4. **Backlink rewrite (B/C):** when the id changes, do we rewrite inbound
   `block_links.resolved_id` to the new page (explicit matview/rewrite pass — no
   FK cascade exists), or leave them pointing at the origin/dangling?
5. **Undo atomicity:** is one composite `UndoEntry` (create + moves + content
   edit / delete) acceptable given several constituent ops lack inverse providers
   today, or does this transform wait on the undo provider-coverage workstream
   ([undo-ruling])?
6. **Keystone coverage:** should the transform be added as a keystone PBT
   transition (per CLAUDE.md rule) so the SiblingGrounding grounding of the
   de-inline/replace is proven, not just hand-tested?
