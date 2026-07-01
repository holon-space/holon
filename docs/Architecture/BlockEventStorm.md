# Event Storm: The Block Lifecycle

> A "Big Picture" Event Storm of everything that touches a `Block` as it crosses
> Org, Markdown, Loro+Iroh, Turso, and the UI. Read the colour legend, then the
> timeline, then the hotspots. Generated 2026-06-28 from a code/ADR sweep;
> **hotspot statuses re-verified 2026-07-01** (four had already been fixed:
> H1, H4, H11, H12 — three days of drift was enough to invalidate half the red
> stickies).
>
> ⚠ **Staleness protocol**: hotspot statuses decay fast. Before acting on any
> 🔴, grep for the anchor *symbol* (not the line number — several drifted within
> days) and confirm the claim still holds. Each hotspot below carries a status
> line and its verification anchor.

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

**The canonical glossary lives in [CONTEXT.md](../../CONTEXT.md)** — its §2 (core
vocabulary), §3 (per-context dialects), and §4 (synonym & deprecation registry)
carry every stable term with its canonical form and cleanup action. This section
keeps only the rows that are *hotspot evidence*: places where the naming drift IS
(or was) a live bug tracked in §4 below. When a row's hotspot closes, the row
graduates to CONTEXT.md §4 and is deleted here.

| One concept | Names in the wild | Risk |
|---|---|---|
| **"this block is a page"** | `PAGE_TAG = "Page"` via `Block::is_page()` (canonical) · `doc:` URI scheme (deprecated, still in ~15 files) · `set_is_document` op name | 🔴 H7 — `is_document()` deleted ✅, but two legacy encodings remain |
| **a block mutation** | `Operation` (descriptor) · op-name string · `OperationIntent` · `ChangeOp` (the only typed enum) · `BlockDiff` | parse-don't-validate gap (H2) |
| **edge fields** | `tags`/`requires` as `Block` fields · `block_tags`/`block_requires` junction rows · Loro meta keys · `EdgeField` enum (closed, iterated at all projection sites) | H1/H12 fixed ✅; `Block`'s serde still skips them (see H1 residue) |

---

## 4. Hotspots (🔴 the red stickies)

**Status board (2026-07-02):** ✅ fixed: H1, H3, H4, H8, H11, H12 · 🔴 open: H2,
H5, H6, H7 (narrowed), H10 · ⚪ by-design constraint: H9. Fixed entries are kept
(condensed) because their *mechanisms* — lossy serde base, gate/emit mismatch,
blob-LWW — are recurring failure shapes worth recognizing next time.

**H1 — Lossy serde round-trip of edge fields through the projection sidecar.
✅ FIXED (verified 2026-07-01).**
`Block`'s `tags`/`requires` are still `#[serde(skip, default)]`
(`holon-api/src/block.rs`, `struct Block`) — only `impl TryFrom<StorageEntity> for
Block` hydrates edge fields from the matview. That used to mean `SnapshotBlock`
(which embeds a `Block` and is serde-persisted into the projection sidecar,
`holon-filesystem/src/sync_base_store.rs`) round-tripped blocks with **empty edge
fields**, so on every cold boot the projection diffed a tagged block against an
empty disk base and re-emitted a spurious tags/requires UPDATE → junction
DELETE+INSERT → matview CDC → first-paint churn proportional to page count
(every page carries the `"Page"` tag). Self-healing and non-corrupting, but real
write-amplification; confirmed firing 2026-06-28.

**Fix shipped — option (b) as recommended:** `SnapshotBlock` (now in
`holon-api/src/block.rs`, moved out of `loro_backend.rs`) serializes through an
explicit `SnapshotBlockWire` DTO (`#[serde(into/from = "SnapshotBlockWire")]`)
that carries `tags`/`requires` as sibling fields, making the sidecar round-trip
lossless. Regression test: `snapshot_block_serde_round_trip_preserves_edge_fields`.
The underlying type-level weakness — `Block: Deserialize` still silently yields a
half-built block on any *other* serde path — remains; the `StoredBlock`-newtype
idea (option (a)) is still open as hardening. Anchor: `SnapshotBlockWire`.

**H2 — `ChangeOp` carries raw schemed-or-bare strings. 🔴 STILL OPEN (verified
2026-07-01).**
`parent`/`parent_id` stay `String` "in Phase 1 (they flip to typed refs later)";
the consolidator normalizes later. Classic *validate-don't-parse* debt; the
schemed/bare ambiguity is exactly the bug the bare-id convention was invented to
avoid. Anchor: `ChangeOp` in `holon-api/src/change_set.rs` (~line 87).

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

**H4 — RichText marks were a Phase-1 stub in Loro. ✅ FIXED (verified 2026-07-01).**
Marks now live in Loro Peritext on the `CONTENT_RAW` `LoroText`: writes go through
`text.mark`/`text.unmark` (wholesale replace in `update_block_text`'s re-apply, plus
incremental `apply_inline_mark`/`remove_inline_mark`), reads through
`read_marks_from_text`, and `blocks_differ` compares `marks` so changes project to
SQL ("Phase 2 authority flip"; `_expected_marks` compare-and-set guards dropped —
single SQL writer per field). Rich-text formatting **is** replicated by the CRDT of
record; the SQL `marks` column is a projection. Covered by `marks_outbound_tests`
and `loro_backend::tests::marks_round_trip_through_loro`. Anchor:
`apply_inline_mark` in `holon-loro/src/loro_backend.rs`.

**H5 — Sharing security is documented but unimplemented. 🔴 OPEN (claims NOT
re-verified 2026-07-01 — see validation task below).**
ADR 0003 / BLOCK_LORODOC describe capability auth (write=secret key, read=public
key), delegation, key rotation on unshare. Reality as of 2026-06-28:
`share_subtree` only picks `HistoryRetention`, no encryption, revocation is
advisory ("can't un-send"), and shallow `None` retention **cannot merge back**
(creates a fresh CRDT base). A reader of the ADRs will badly over-estimate what
shipped. The "cannot merge back" claim in particular has never been demonstrated
by a test — validate before relying on it. Anchors: `share_subtree`,
`HistoryRetention` in `holon-loro/src/loro_share_backend.rs` / `shared_tree.rs`.

**H6 — Markdown identity drift. 🔴 STILL OPEN (charset rule confirmed present
2026-07-01; full round-trip behaviour not re-traced).**
Block ids whose charset isn't Obsidian-friendly are dropped from rendered text, so a
re-parse mints a fresh UUID → the block loses identity across a round-trip. Also,
paragraph bodies are folded into the heading block — only headings/fences/images are
addressable. This is a textbook round-trip-identity PBT target. Anchor: the charset
comment in `holon-markdown/src/renderer.rs` (~line 185) and `parser.rs`.

**H7 — "Page" has multiple coexisting encodings. 🔴 STILL OPEN, but narrower
(verified 2026-07-01).**
Progress since 2026-06-28: `Block::is_document()` is **deleted** — the canonical
representation is now `PAGE_TAG = "Page"` via `Block::is_page()`/`set_page()`.
Still live: the `doc:` URI scheme (`#[deprecated]` in `entity_uri.rs` yet
referenced in ~15 files: `link_parser`, `link_provider`, `focus_path`,
`backend_engine`, `prql_stdlib.prql`, `holon-profiles`, PBT/test harnesses) and
the `set_is_document` op name (`holon-core/src/traits.rs`). Migration is
half-done; new code can still pick the wrong encoding. Anchors: `PAGE_TAG`,
`set_is_document`, `deprecated` in `entity_uri.rs`.

**H8 — `block_raw` columns silently dropped on read. ✅ FIXED (2026-07-02).**
`impl TryFrom<crate::StorageEntity> for Block` is now strict, three ways per
column (absent-key / present-Null / wrong-type): all 8 `.expect()/.unwrap()`
panics became `Err`s naming the column + block id; the silent defaults
(`unwrap_or("")` content, `unwrap_or("text")` content_type, `unwrap_or(0)`
timestamps, `filter_map`-swallowed array elements, `_ =>` catch-alls) are gone.
`tags`/`requires` are **required columns** — every reader must COALESCE them to
`'[]'`; Null or an absent key now hard-errors instead of hydrating an empty vec.
Nullable-by-schema columns (`marks`, `properties`, `source_language`,
`source_name`) map Null→None; a Null `parent_id` maps to the `no_parent`
sentinel, but an *absent* `parent_id` key is a reader bug and errors.
Closing the strictness gap surfaced a **latent prod bug**: two readers omitted
`requires` from their SELECTs, so blocks hydrated with empty `requires` —
`CacheBlockReader::load_all_blocks_with_hydration` (`holon-orgmode/src/di.rs`)
and `TursoSinkReader::read_blocks` (`holon/src/storage/turso_sink_reader.rs`).
The latter even *documented* the omission as deliberate ("requires is not part
of `blocks_differ`") — that rationale went stale the day H12's fix made
`blocks_differ` iterate `EdgeField::ALL`; both SELECTs now hydrate the
junction, and the stale comment is deleted. `task_blocks_for_petri.sql` widened
its projection to include the matview's COALESCE'd `tags`/`requires`.
Still open as H1-residue hardening (not H8): `depth`, `sort_key`, `collapsed`,
`completed`, `block_type` remain intentionally not round-tripped into `Block`
with no type-level marker. Anchors:
`impl TryFrom<crate::StorageEntity> for Block`, `require_string_array`
(`holon-api/src/block.rs`); `TursoSinkReader::read_blocks`;
`load_all_blocks_with_hydration`.

**H9 — CDC only fires from matviews, never base tables.**
A hard Turso/IVM constraint that shapes the entire read path: writes go to
`block_raw`, reads/CDC must go through the `block` matview. Not a bug, but an
**invisible coupling** — anyone who "optimizes" by reading the base table breaks
reactivity silently. Characterized in `cdc_base_vs_matview_repro.rs`.

**H10 — Two block query sources, DI-selected. 🔴 STILL OPEN (verified 2026-07-01).**
`crates/holon/src/sync/loro_block_query_source.rs` and
`.../turso_block_query_source.rs` both exist; "what reads a block" depends on the
wired graph. The same logical read has two implementations that can drift.
Partial mitigation exists: `tests/turso_block_query_source_round_trip_pbt.rs`
exercises the Turso side; an *equivalence* property (same query → both sources →
same blocks) is the natural composed-catalog invariant here.

**H11 — Stale ADR contradicts shipped architecture. ✅ FIXED (verified 2026-07-01).**
`docs/adr/BLOCK_LORODOC_ARCHITECTURE.md` now reads **Status: Superseded** with a
pointer to ADR 0003 (all-in-one-LoroTree), which is what the code implements. A
front-to-back ADR read now yields the right mental model.

**H12 — `blocks_differ` omitted `requires` from its change gate. ✅ FIXED
(verified 2026-07-01)** *(found while tracing H1, 2026-06-28).*
A block whose only in-session change was `requires` never satisfied the projection
gate, so depends-on/blocked-by edits were silently not projected to SQL. **Fix
shipped, and type-driven:** the gate now iterates `EdgeField::ALL` (a closed enum
covering `tags` + `requires`, the same enum all four projection sites iterate), so
adding a future edge field cannot silently miss the gate again. Fixed in the right
order (after the H1 sidecar round-trip), so closing the gate did not reintroduce
cold-boot churn. Caught via the composed `SetEdgeField` PBT transition +
`SutEdgeFieldWrite` cap. Anchor: `fn blocks_differ`
(`holon-loro/src/loro_sync_controller.rs:886`).

---

## 5. What the storm says about the architecture

**Strengths the wall makes visible.** The format ACL is genuinely clean — org and
markdown implement the *same* `FileFormatAdapter` trait and the bare-id↔scheme
translation is consistently applied at exactly one boundary. The
command/event/origin model (`EventOrigin::Org`, `ChangeOrigin::{Local,Remote}`) is a
real, working echo-suppression policy. The driver ladder is a principled write path.
This is an event-sourced reactive system that mostly already obeys DDD strategic
design without having named it.

**The one structural tension worth a decision — largely RESOLVED (2026-07-01).**
"Loro is the write-of-record" used to leak: marks lived only in SQL (H4) and
`properties` wasn't really CRDT-merged (H3). Both are now in Loro (Peritext marks;
nested per-key-LWW `PROPERTIES_MAP`). The remaining, *deliberate* exception is
UI-local state (`collapsed`, scroll, cursor — Tier-1 LOCAL by design). The honest
phrasing of the principle today: **"Loro owns everything collaborative; UI-local
presentation state intentionally never enters the CRDT."** Keep new fields honest
against that line — the failure mode is a field that is collaborative in intent
but SQL-only in implementation, which is exactly what H4 was.

**The cheap, high-value cleanups still open** (language, not architecture):
finish the `doc:`-scheme elimination and
retire the `set_is_document` op name (H7); a type-level marker for
"intentionally not round-tripped" columns (`depth`, `sort_key`, `collapsed`, …);
type the `ChangeOp` parent refs (H2); the
`StoredBlock` newtype so a serde-path `Block` can't impersonate a matview-hydrated
one (H1 residue). Done since first writing: BLOCK_LORODOC marked superseded
(H11); `TryFrom<StorageEntity>` fails loud on missing/malformed columns (H8).

**These hotspots are also PBT targets.** Every 🟠 event is a candidate state-machine
transition and every 🔴 a candidate invariant. Open candidates: markdown round-trip
identity (H6 — the highest-value untested one), Loro-vs-Turso query-source
equivalence (H10), share/merge-back behaviour (H5). Already realized: edge-field
projection (H12 was *caught* by the composed `SetEdgeField` transition — the
pattern works), concurrent-property convergence (`h3_property_convergence_tests`;
still a candidate to lift into the composed catalog), mark replication
(`marks_round_trip_through_loro`).

**What this storm still doesn't cover** (gaps found in the 2026-07-01 review):
the **deletion lane** (BlockDeleted → junction cleanup → CDC coalesce → widget
unmount — never traced end-to-end), **file rename/move on disk** (does block
identity survive a path change, or is it a mass delete+create?), **failure lanes**
(parse error, projection error, Iroh disconnect mid-import — an Event Storm
normally has these as explicit events; this one has none), and **concurrent
Lane A × Lane C** (disk edit racing a peer import over the same block).
