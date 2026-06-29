# Event Storm: The Block Lifecycle

> A "Big Picture" Event Storm of everything that touches a `Block` as it crosses
> Org, Markdown, Loro+Iroh, Turso, and the UI. Read the colour legend, then the
> timeline, then the hotspots. Generated 2026-06-28 from a code/ADR sweep.

**Legend** (Event Storming colours):
🟠 Domain Event (past tense, a fact that happened) ·
🔵 Command (imperative, an intent to change) ·
🟡 Actor / Aggregate (who/what owns the rule) ·
🟣 Policy ("whenever X then Y") ·
🟢 Read Model (what someone looks at to decide) ·
🔴 Hotspot (contradiction, debt, hand-wavy seam — the whole point of the exercise)

---

## 1. Bounded contexts (where the language changes)

The single biggest finding: **`Block` is not one concept — it is six dialects of one
word, bridged by anti-corruption layers.** The seams below are real context
boundaries, each with its own vocabulary and its own translation rules.

| Context | Crate(s) | Owns | Calls a block a… | Block identity is… |
|---|---|---|---|---|
| **Authoring / Format** | `holon-org-format`, `holon-orgmode`, `holon-markdown` | parse/render disk text ↔ Block | headline / heading / drawer / fence / frontmatter | **bare id** on disk |
| **CRDT of record** | `holon-loro` | the authoritative block store + merge | LoroTree **node** (`TreeID`) + meta `LoroMap` | `STABLE_ID` meta key (`TreeID` is peer-local!) |
| **P2P transport** | `holon-loro` (Iroh) | replicate the CRDT between peers | version-vector delta bytes | per-share `stable_peer_id` |
| **Read projection / Query** | `holon-turso` | SQL read model + CDC | **row** in `block` matview / `block_raw` | `id` column (not SQLite ROWID!) |
| **Interaction / Render** | `holon-frontend`, `frontends/gpui`,`/tui` | Block → ViewModel; gesture → intent | **row** → ViewModel → widget | `EntityUri` carried by the intent |
| **Shared kernel** | `holon-api`, `holon-core` | the `Block` struct, `Change`, `EntityUri`, `ChangeOp` | `Block` | `EntityUri` (`block:` scheme) |
| **Orchestration** | `holon` | command bus, controllers, `EventOrigin` routing | — | — |

The **direction of authority** is the spine of the whole system:

```
Org / Markdown ──parse──▶ command bus ──▶ ordering authority (consolidator) ──▶ LORO  (write-of-record)
                                                                                  │
                                              LoroProjection.project()  ──────────┤
                                                                                  ▼
                                                                      TURSO block matview  (read model)
                                                                                  │ CDC (matviews only!)
                                                                                  ▼
                                                                      ReactiveViewModel ──▶ widget
                                                                                  │
   user gesture ──▶ OperationIntent ──▶ command bus ──────────────────────────────┘  (loop closes)
```

Loro is the **write-of-record**; Turso is a **projection**. There are two parallel
`*_block_query_source.rs` (Loro and Turso) and *which one reads a block depends on
the wired DI graph* — see 🔴 H10.

---

## 2. The event timeline (the orange line)

### Lane A — a user types in an `.org`/`.md` file on disk
```
🔵 FileSyncStarted (DI marker resolved)        🟡 FileSyncController
   🟣 whenever a non-gitignored .org/.md changes on disk…
🟠 OrgFileChanged / MarkdownFileChanged         🟡 OrgFileWatcher (FileChangeSource port, ADR 0011)
🔵 ParseFile(path, content)                      🟡 FileFormatAdapter  (org | markdown — SAME trait)
🟠 FileParsedIntoBlocks                          🟡 parser  (adds block:/doc: schemes to bare ids)
   🟢 old_blocks snapshot (read model for the diff)
🔵 DiffBlocks(old, new)                           🟡 block_diff
🟠 BlockCreated/ContentChanged/Moved/Deleted (tagged EventOrigin::Org)
   🟣 EventOrigin::Org makes the inbound gate APPLY instead of dropping as echo
🔵 create_in_tree / execute_operation_with_origin 🟡 command bus → ordering authority
```

### Lane B — the write lands in the CRDT of record
```
🔵 create_block / update_block_text / update_block_position / set_block_tags …  🟡 LoroBackend
🟠 BlockCreated / BlockUpdated / BlockFieldsChanged  (Change<Block>, origin=Local)
   ↳ children order = the ordered child list (ADR 0005); LoroTree fractional index is its Loro materialization — NOT a Block field
🟠 LoroDocChanged                                 🟡 LoroDoc
🔵 CommitDebounced                                🟡 DebouncedCommitWorker
```

### Lane C — a peer syncs over Iroh
```
🔵 sync_doc_initiate(peer_vv)                     🟡 IrohSyncAdapter (QUIC, ALPN = prefix/shared_tree_id)
🟠 DeltaExported (ExportMode::updates(peer_vv))    🟣 fall back to full Snapshot if peer behind compacted log (disclosed warn!)
🔵 apply_update(bytes)
🟠 PeerUpdateImported
🟠 BlocksDiffedAfterImport → BlockCreated/Updated/Deleted (origin=Remote)  🟡 diff_and_emit_after_import
```

### Lane D — the projection rebuilds the read model
```
🟣 whenever LoroDocChanged…                       🟡 LoroSyncController.on_loro_changed
🔵 project()                                       🟡 LoroProjection
🟠 BlocksProjectedToSql  (diff_snapshots_to_ops → INSERT/UPDATE block_raw + junction DELETE+INSERT)
🟠 FrontiersPersisted    (sidecar)
   ── block_raw is a BASE TABLE → emits NO CDC ──
🟠 MatviewRecomputed     (block matview LEFT-JOINs block_tags/block_requires, json_group_array hydrates edge fields)
🟠 CdcBatchEmitted       (RowChange{relation_name = matview, change}, origin travels in _change_origin col)
   🟣 coalesce: DELETE+INSERT(same id) → Updated;  INSERT+DELETE → dropped (anti-flicker)
```

### Lane E — the UI renders and the user acts
```
🔵 watch_ui(block_id)                              🟡 ui_watcher (structural matview + switch_map)
🟠 StructureRendered  (UiEvent::Structure{render_expr, candidates, generation})
🟠 RowsUpdated        (UiEvent::Data{batch, generation})  — stale generations dropped
🟢 ReactiveRenderedRows (loading() placeholder until first Structure)  🟡 ReactiveViewModel tree
🟠 WidgetMounted                                   🟡 GPUI RenderEntityView / TUI / MCP snapshot
   — user acts —
🔵 click / Cmd+Enter / Tab / drag …                🟡 UserDriver ladder (Gpui ⊐ ReactiveEngine ⊐ Direct)
🟠 BlockClicked / TaskStateToggled / BlockExpanded / BlockIndented / TextEdited / EntityDropped
🔵 OperationIntent{entity_name, op_name, params}   🟡 dispatch_intent  → back to the command bus (Lane B)
   🟣 expand/collapse + scroll + cursor are Tier-1 LOCAL (Cell/Mutable) — never round-trip
```

**Pivotal events** (the ones that flip the system between contexts, worth a big
purple frame on the wall): `FileSyncStarted`, `BlocksProjectedToSql`,
`CdcBatchEmitted`, `StructureRendered`, `PeerUpdateImported`.

---

## 3. The ubiquitous-language audit (the highest-value output)

Event Storming earns its keep by exposing where one concept wears many names.
Each row below is a candidate for a glossary entry — and several are latent bugs.

| One concept | Names in the wild | Risk |
|---|---|---|
| **"this block is a page"** | `is_document()` (deprecated) · `doc:` URI scheme (deprecated, still in 6 crates) · `PAGE_TAG = "Page"` tag · `Block::is_page()` · `set_is_document` op | 🔴 H7 — **three live representations**, mid-migration |
| **a block mutation** | `Operation` (descriptor) · op-name string · `OperationIntent` · `ChangeOp` (the only typed enum) · `BlockDiff` | parse-don't-validate gap (H2) |
| **sibling order** | **ordered child list** (ADR 0005, canonical) · fractional index (Loro materialization) · `sort_key` column in `block_raw` (Turso materialization) · `SnapshotBlock.sort_key:String` · `sequence` (legacy) · `after_block_id` (positional intent) | index/`sort_key` are per-system representations of the ordered list, not the authority |
| **the rendered unit** | domain **Block** · matview **row** (`DataRow`) · **ViewModel** · **widget** | one block → many rows (panels) |
| **ViewModel** | `ReactiveViewModel` (live MVVM node) · `ViewModel` (frozen snapshot for tests/MCP) | 🔴 easy to conflate — same word, two types |
| **widget** | `ViewKind` tag · `shadow_builders/*` · native `AnyElement` | overloaded ×3 |
| **a change event** | `Change<T>` (neutral) · `RowChange` (Turso-tagged) · `ChangeData` · `UiEvent::Data` | tag stripped/added at seams |
| **block identity** | `EntityUri` (`block:`) · bare id (disk) · `TreeID` (peer-local, Loro) · `STABLE_ID` (Loro meta) · `id` column · SQLite ROWID (must NOT use) | 🔴 5 id spaces, one is a trap |
| **edge fields** | `tags`/`requires` as `Block` fields · `block_tags`/`block_requires` junction rows · Loro meta keys | shape differs read vs write (H1) |

---

## 4. Hotspots (🔴 the red stickies)

**H1 — Two Block deserializers; only one is complete — and the serde one is on a
live path.**
`#[derive(Serialize, Deserialize)]` marks `tags` and `requires` `#[serde(skip,
default)]`, so the serde path (both directions) silently yields a block with **no
page-ness and no dependencies**. Only `impl TryFrom<StorageEntity> for Block` hydrates
edge fields from the matview's `json_group_array`. *(`holon-api/src/block.rs:261` vs
`:698`.)* This is the textbook "make illegal states unrepresentable" violation — the
type permits a half-built Block.

It is **not** purely latent. `SnapshotBlock { block: Block, sort_key }`
(`holon-loro/src/loro_backend.rs:434`) embeds a `Block` and is serde-serialized into
the Loro projection sidecar (`sync_base_store.rs`). Because `#[serde(skip)]` drops
`tags`/`requires` on serialize *and* deserialize, **the sidecar always round-trips
blocks with empty edge fields.** The projection diff *does* compare them
(`blocks_differ` `loro_sync_controller.rs:911`; `block_diff_params` `:836/:840`), so a
cold boot that seeds `before` from the sidecar will diff a tagged block against an
empty baseline and **re-emit a spurious tags/requires UPDATE** → junction
DELETE+INSERT → spurious CDC/UI churn.

**Cold-boot trace — CONFIRMED, it fires (2026-06-28).** `SyncBaseStore::
from_frontiers_sidecar` (the Loro→SQL projection's base store) calls `load_base()` at
construction (`sync_base_store.rs:115`), so on the 2nd+ launch `is_base_seeded(global())`
is true (`:190`) and `before = get_base()` = the **disk base**, which `put_base`
persisted through serde (`:144`) with `tags`/`requires` dropped. Meanwhile `after =
snapshot_blocks_from_doc_settled` reads full edge fields from Loro meta
(`loro_backend.rs:415-421`). `blocks_differ` compares `tags` (`loro_sync_controller.rs:911`),
so **every block carrying a tag — and every *page* carries the `"Page"` tag — triggers
a spurious UPDATE on the first projection after boot**, re-writing the `block_tags`
(and `block_requires`) junction → spurious matview CDC → first-paint churn (the exact
cost the projection's startup-promptness logging at `:465` watches). It is
**self-healing** (the first pass's `put_base(after)` restores the full in-memory base,
so it's one pass per boot) and **non-corrupting** (re-writes correct values) — so the
impact is cold-boot write-amplification + first-paint churn proportional to the page
count, not data loss. The `was_seeded=false` → `read_sql_snapshot` re-seed path
(`:379/:513`) only runs on the *first-ever* boot or when the sidecar fails to decode.

**Fix is entangled with H12 — decide, don't patch piecemeal.** Removing `Deserialize`
from `Block` is blocked (`SnapshotBlock: Deserialize` needs it). Options: (a) a
`StoredBlock` newtype only the matview can mint; (b) give `SnapshotBlock` an explicit
serde representation that carries edge fields (so the base round-trip is lossless).
Prefer (b) for this specific bug; do it **before** touching H12, or the churn gets
worse.

**H2 — `ChangeOp` carries raw schemed-or-bare strings.**
`parent`/`parent_id` stay `String` "until Phase 5"; the consolidator normalizes
later. Classic *validate-don't-parse* debt; the schemed/bare ambiguity is exactly
the bug the bare-id convention was invented to avoid. *(`change_set.rs:79-90`.)*

**H3 — Loro `properties` blob-LWW convergence bug. ✅ FIXED (2026-06-29).**
Previously `write_properties_to_meta` collapsed the whole property map into **one**
`LoroMap` key holding **one** opaque JSON string, so the merge granularity was the
whole blob: two peers concurrently setting different properties (e.g. `TODO` vs
`PRIORITY`) did not merge — one peer's whole JSON string won and silently dropped
the other's. TaskState lives in `properties`, so this was a real convergence bug.
**Fix:** properties now live in a **nested `LoroMap`** under `PROPERTIES_MAP`
(`loro_backend.rs`), one key per property, so per-key LWW merges concurrent edits
to distinct properties. Crucially the *update* paths (`update_block_properties`,
`update_block_fields`) write **only the changed keys** — a read-modify-write of the
whole set would re-stamp untouched keys with stale values and reintroduce the
clobber at per-key level. The legacy single-blob `PROPERTIES` key is read-migrated
on the next write (untouched keys copied into the nested map, then the blob deleted
— self-healing, no dual representation). Covered by `h3_property_convergence_tests`
(concurrent-distinct-property merge, legacy migration, exact-set replace).

**H4 — RichText marks are a Phase-1 stub in Loro.**
Marks are written via the plain-text path and actually persisted only in the SQL
`marks` column. So rich-text formatting is **not replicated by the CRDT of record** —
it survives only because Turso is currently durable, contradicting "Loro is the
write-of-record." *(`loro_backend.rs:501-507`.)*

**H5 — Sharing security is documented but unimplemented.**
ADR 0003 / BLOCK_LORODOC describe capability auth (write=secret key, read=public
key), delegation, key rotation on unshare. Reality: `share_subtree` only picks
`HistoryRetention`, no encryption, revocation is advisory ("can't un-send"), and
shallow `None` retention **cannot merge back** (creates a fresh CRDT base). A reader
of the ADRs will badly over-estimate what shipped.

**H6 — Markdown identity drift.**
Block ids whose charset isn't Obsidian-friendly are dropped from rendered text, so a
re-parse mints a fresh UUID → the block loses identity across a round-trip. Also,
paragraph bodies are folded into the heading block — only headings/fences/images are
addressable. *(`markdown/renderer.rs:182`, `parser.rs`.)*

**H7 — "Page" has three coexisting encodings** (see table). `doc:` scheme is
`#[deprecated]` with note "being eliminated… now blocks with is_document=true" yet is
still referenced in `holon-markdown`, `link_parser`, `backend_engine`,
`prql_stdlib.prql`, and PBT components. Migration is half-done; new code can pick the
wrong one. *(`entity_uri.rs:173`.)*

**H8 — `block_raw` columns silently dropped on read.**
`depth`, `sort_key`, `collapsed`, `completed`, `block_type` exist in SQL but are not
deserialized into `Block`. `collapsed` being UI-local is *correct and deliberate*
(avoids collaborative churn) — but it's invisible at the type level; nothing marks
these as "intentionally not round-tripped." *(`block.rs:698-809`.)*

**H9 — CDC only fires from matviews, never base tables.**
A hard Turso/IVM constraint that shapes the entire read path: writes go to
`block_raw`, reads/CDC must go through the `block` matview. Not a bug, but an
**invisible coupling** — anyone who "optimizes" by reading the base table breaks
reactivity silently. Characterized in `cdc_base_vs_matview_repro.rs`.

**H10 — Two block query sources, DI-selected.**
`loro_block_query_source.rs` and `turso_block_query_source.rs` both exist; "what
reads a block" depends on the wired graph. The same logical read has two
implementations that can drift.

**H11 — Stale ADR contradicts shipped architecture.**
`docs/adr/BLOCK_LORODOC_ARCHITECTURE.md` is still **Status: Proposed** and describes
the *two-layer* (LoroTree + per-block content LoroDocs) design that ADR 0003
(all-in-one-LoroTree) **superseded and the code implemented**. A future agent reading
ADRs front-to-back will build the wrong mental model.

**H12 — `blocks_differ` omits `requires` from its change gate** *(found while
tracing H1, 2026-06-28).*
`blocks_differ` (`loro_sync_controller.rs:905-913`) compares `sort_key`, `content`,
`parent_id`, `content_type`, `source_language`, `source_name`, `tags`,
`properties_map`, `marks` — but **not `requires`**. Yet `block_diff_params` (`:840`)
*does* emit `requires`. So a block whose only in-session change is `requires` never
satisfies the gate → **the `requires` (depends-on / blocked-by) change is silently
not projected to SQL.** Independent of H1, this is a real correctness bug.
⚠ It is entangled with H1: simply adding `requires` to the gate would make every
`requires`-bearing block also churn on cold boot (H1) until the lossy sidecar
round-trip is fixed. Fix the H1 round-trip first, then close the gate.

---

## 5. What the storm says about the architecture

**Strengths the wall makes visible.** The format ACL is genuinely clean — org and
markdown implement the *same* `FileFormatAdapter` trait and the bare-id↔scheme
translation is consistently applied at exactly one boundary. The
command/event/origin model (`EventOrigin::Org`, `ChangeOrigin::{Local,Remote}`) is a
real, working echo-suppression policy. The driver ladder is a principled write path.
This is an event-sourced reactive system that mostly already obeys DDD strategic
design without having named it.

**The one structural tension worth a decision.** "Loro is the write-of-record" is
asserted but **leaks**: marks (H4) and arguably collapsed/UI-state live elsewhere,
and `properties` isn't really CRDT-merged (H3). Either commit to Loro as the total
source of truth (move marks + property-level merge into Loro) or rename the principle
to "Loro is the structural source of truth, SQL owns presentation state." Right now
the language over-promises.

**The cheap, high-value cleanups** (language, not architecture): write the glossary
in §3 into `CONTEXT.md`; finish the `doc:`-scheme elimination (H7); mark
BLOCK_LORODOC superseded (H11); add a type-level marker for "intentionally not
round-tripped" columns (H8); make the serde-vs-matview Block deserialization
impossible to get wrong (H1) — e.g. a `StoredBlock` newtype that *only* the matview
path can produce.

**These hotspots are also PBT targets.** Every 🟠 event is a candidate state-machine
transition and every 🔴 a candidate invariant: round-trip identity (H6), edge-field
preservation across the serde seam (H1), concurrent-property convergence (H3 — now
covered by `h3_property_convergence_tests`; a candidate to lift into the composed
catalog), mark replication through Loro (H4).
