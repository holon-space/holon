# Event Storm: The Block Lifecycle

> A "Big Picture" Event Storm of everything that touches a `Block` as it crosses
> Org, Markdown, Loro+Iroh, Turso, and the UI. Read the colour legend, then the
> timeline, then the hotspots. Generated 2026-06-28 from a code/ADR sweep;
> **hotspot statuses re-verified 2026-07-02** (the 2026-07-01 sweep found four
> already fixed: H1, H4, H11, H12 — three days of drift was enough to invalidate
> half the red stickies; the 2026-07-02 sweep closed the track — see the status
> board in §4).
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
| **Authoring / Format** | `holon-org-format`, `holon-orgmode` | parse/render disk text ↔ Block | headline / heading / drawer / fence / frontmatter | **bare id** on disk |
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
the wired DI graph* — see ✅ H10 (the two arms are now pinned equivalent).

---

## 2. The event timeline (the orange line)

### Lane A — a user types in an `.org`/`.md` file on disk
```
🔵 FileSyncStarted (DI marker resolved)        🟡 FileSyncController
   🟣 whenever a non-gitignored .org/.md changes on disk…
🟠 OrgFileChanged / MarkdownFileChanged         🟡 OrgFileWatcher (FileChangeSource port, ADR 0011)
🔵 ParseFile(path, content)                      🟡 FileFormatAdapter  (LIVE: org only; markdown impls the trait but is not yet wired — zero prod dependents)
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
| **a block mutation** | `Operation` (descriptor) · op-name string · `OperationIntent` · `ChangeOp` (the only typed enum, parent refs now `EntityUri`) · `BlockDiff` | H2 fixed ✅; the non-`ChangeOp` forms remain boundary dialects |
| **edge fields** | `tags`/`requires` as `Block` fields · `block_tags`/`block_requires` junction rows · Loro meta keys · `EdgeField` enum (closed, iterated at all projection sites) | H1/H12 fixed ✅; `Block` is now serde-free — edge fields carried explicitly by `BlockWire` on every wire path |

---

## 4. Hotspots (🔴 the red stickies)

**Status board (2026-07-02):** ✅ fixed: H1, H2, H3, H4, H6, H7, H8, H10, H11, H12 · 🟡
validated (claims pinned, not "fixed" — the doc/impl gap is real and now
demonstrated): H5 · ⚪ by-design constraint: H9. Fixed entries are kept
(condensed) because their *mechanisms* — lossy serde base, gate/emit mismatch,
blob-LWW — are recurring failure shapes worth recognizing next time.

**🎉 Track closed (2026-07-02).** Every hotspot on this wall is now resolved or
re-dated: H1–H4, H6–H8, H10–H12 are ✅ fixed; H5 is 🟡 validated (its doc/impl gap
is real and now pinned by tests, not silently papered over); H9 is a ⚪ deliberate
by-design constraint. The BlockEventStorm hotspot track (milestones M0–M7) is done.
No 🔴 open hotspots remain. The staleness protocol below still applies — statuses
decay, so re-verify before acting on any single claim.

**H1 — Lossy serde round-trip of edge fields through the projection sidecar.
✅ FIXED — residue closed 2026-07-02.**
Original bug: `Block`'s `tags`/`requires` were `#[serde(skip, default)]`, so
`SnapshotBlock` (serde-persisted into the projection sidecar,
`holon-filesystem/src/sync_base_store.rs`) round-tripped blocks with **empty edge
fields**. On every cold boot the projection diffed a tagged block against an empty
disk base and re-emitted a spurious tags/requires UPDATE → junction DELETE+INSERT
→ matview CDC → first-paint churn proportional to page count (every page carries
the `"Page"` tag). Self-healing and non-corrupting, but real write-amplification;
confirmed firing 2026-06-28.

**First fix (pre-dated this milestone):** `SnapshotBlock` (in `block.rs`, moved
out of `loro_backend.rs`) routed through an explicit `SnapshotBlockWire` DTO that
carried the edge fields as siblings — lossless for that one path, but the
type-level weakness stayed: `Block: Deserialize` still silently yielded a
half-built block on *every other* serde path (PBT fixtures dropped edge fields on
replay).

**Residue closed:** `Block` is now **fully serde-free** — the `Serialize`/
`Deserialize` derives are gone (junction-derived fields are marked `#[edge_field]`
so the `Entity` derive still excludes them from the column schema). Every wire
path goes through a `BlockWire` DTO that carries `tags`/`requires` as real fields
(`#[serde(default)]`, disclosed legacy allowance so pre-milestone fixtures /
sidecars parse). Consumers: `SnapshotBlockWire` embeds `BlockWire` (+ a read-only
legacy-sibling fallback for existing sidecars); the PBT transitions serialize
`Vec<Block>` through the `block_wire_vec` adapter; MCP `inspect_loro_blocks` emits
`BlockWire` and fails loud on error. Byte-compat for real vault data is pinned by
`legacy_sidecar_sample_recovers_edge_fields`
(`holon-filesystem`, checked-in pre-M3 sidecar sample) and by
`snapshot_block_serde_round_trip_preserves_edge_fields`. Illegal half-built
serde blocks are now unrepresentable. Anchors: `BlockWire`, `SnapshotBlockWire`,
`block_wire_vec`.

**H2 — `ChangeOp` carries raw schemed-or-bare strings. ✅ FIXED (2026-07-02).**
`Create.parent_id` is now `EntityUri` and `Relocate.parent` is
`Option<EntityUri>`, matching the `after_sibling: Option<EntityUri>` pattern —
the schemed/bare ambiguity is resolved once at decode (`EntityUri::from_raw`)
instead of deferred to every consumer. `decode_create`'s
`unwrap_or_default()` swallow is gone: an absent `parent_id` key is an
explicit root create → the `no_parent` sentinel; a present-but-non-string
value fails loud naming the block id. The "flip to `EntityUri` in Phase 5"
promise in the enum doc is fulfilled and deleted. The block `id` itself
deliberately stays the raw string the op carried — normalization remains the
consolidator's job, per the vocabulary's charter. Anchors: `ChangeOp`,
`decode_create`, `decode_update` in `holon-api/src/change_set.rs`.

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

**H5 — Sharing security is documented but unimplemented. 🟡 VALIDATED (2026-07-02) —
both claims re-verified against the code; the doc/impl gap is confirmed real and
now pinned by tests.**
ADR 0003 / BLOCK_LORODOC describe capability auth (write=secret key, read=public
key), delegation, key rotation on unshare. Reality as of 2026-06-28:
`share_subtree` only picks `HistoryRetention`, no encryption, revocation is
advisory ("can't un-send"), and shallow `None` retention **cannot merge back**
(creates a fresh CRDT base). A reader of the ADRs will badly over-estimate what
shipped. Anchors: `share_subtree`, `HistoryRetention` in
`holon-loro/src/loro_share_backend.rs` / `shared_tree.rs`.

- **Claim (a) — no capability auth/encryption. CONFIRMED.** Trace: `grep -niE
  'encrypt|decrypt|cipher|aead|chacha|nonce'` over `loro_share_backend.rs` +
  `shared_tree.rs` = **zero** call-sites (only the new test's doc comment mentions
  the word). `share_subtree → extract_for_share → commit_share_prune` never touches
  a key; the exported CRDT snapshot is plaintext. The auth surface is entirely out
  of band from the payload: ticket-based (`Ticket::new` / `Ticket::decode`,
  `ticket.rs`) + iroh-endpoint stable identity (`stable_peer_id(device_key,
  shared_tree_id)`, `share_peer_id.rs`; key material from
  `device_key_store::load_or_create_device_key`, `device_key_store.rs`). Threat
  model deferred to `docs/Reference/SUBTREE_SHARING.md`. Pinned by
  `shared_tree::tests::share_subtree_payload_is_plaintext_not_encrypted` (an
  anonymous `LoroDoc` with zero credentials imports the shared snapshot and reads
  the block content in plaintext — impossible if the payload were encrypted).
- **Claim (b) — `HistoryRetention::None` share cannot merge back. CONFIRMED, and the
  failure path is LOUD (not a silent divergence).** `unmount(.., Some(shared_doc))`
  for a `None`-retention share returns `Err` ("Failed to reintegrate shared tree
  root … is deleted or does not exist") because the shallow snapshot severed CRDT
  lineage — the collaborative edit does **not** merge back into the personal tree.
  The degraded outcome is disclosed via that error, satisfying the "fail loud, never
  fake" contract. Pinned by
  `shared_tree::tests::none_retention_reintegration_fails_loudly` (a
  characterization test: asserts the loud `Err` **and** that the `[COLLAB]` edit is
  absent from the reintegrated tree). Contrast `unmount_with_reintegration`
  (`HistoryRetention::Full`) which succeeds because lineage is preserved.

**H6 — Markdown identity drift. ✅ FIXED (2026-07-02) — MOOT since 2026-07-06: the
`holon-markdown` crate was removed as unwired dead code (recoverable from git history).**
An out-of-charset block id used to be *silently* dropped from rendered text (empty
`^` marker), so the re-parse minted a fresh UUID → the block lost identity across a
round-trip. `block_id_marker` (`holon-markdown/src/renderer.rs`) now returns a loud
`MarkdownRenderError::{OutOfCharsetBlockId, EmptyBlockId}` instead of an empty marker;
the error propagates through the whole inherent render path (`render_document →
render_blocks → render_tree → render_heading`). Valid-charset ids (`[A-Za-z0-9_-]`,
UUIDs included) still round-trip identically. Pinned by the focused in-crate PBT
`holon-markdown/tests/markdown_block_round_trip_pbt.rs` (`parse(render(block)).id ==
block.id` for valid ids; loud error, no silent remint, for out-of-charset ids) —
modeled on `holon-orgmode/tests/org_block_round_trip_pbt.rs`.

- **Wiring gap (do not forget):** `holon-markdown` still has **zero prod
  dependents** — it is not wired into `FileSyncController` (`holon-core/src/
  file_format.rs` speaks of it in the subjunctive; the live disk path is org-only).
  The fix is therefore latent. The `FileFormatAdapter` trait's `render_document /
  render_blocks` return `String`; the markdown adapter surfaces the render error by
  panicking loudly at that seam (see the `unwrap_or_else` in
  `holon-markdown/src/file_format.rs`) rather than widening the shared trait to
  `Result` for a path the live org renderer would have to change to serve. **When
  markdown graduates into file-sync:** widen the trait to `Result`, drop that panic,
  and lift this round-trip property into the composed invariant catalog (per the
  `pbt-composition` skill), retiring the standalone in-crate PBT.
- **Deliberate addressability limit:** paragraph bodies fold into their heading
  block — only headings/fences/images are independently addressable on disk. This is
  by design (the org adapter folds the same way); it is *not* an identity bug.

**H7 — "Page" has multiple coexisting encodings. ✅ FIXED (2026-07-02).**
The canonical (and now only) representation is `PAGE_TAG = "Page"` via
`Block::is_page()`/`set_page()`. Deleted: `Block::is_document()` (2026-06-28),
and — 2026-07-02 — the entire `doc:` URI scheme (`as_document_id`, the `from_raw`
`doc` acceptance, `classify_link`'s `doc:` arm; `link_parser` mints `block:` for
creation intents, name-hash unchanged since the scheme was never a hash input),
the `set_is_document` op (`holon-core/src/traits.rs`, zero callers), and the
silently-empty `roots` PRQL stdlib relation that filtered on `doc:` parents.
`doc:` survives only in frozen turso repros (`crates/holon/examples/turso_ivm_*`,
`tests/turso_storage_repros/`) and in `link_parser`'s negative tooth
(`test_doc_scheme_no_longer_resolved`). Anchors: `PAGE_TAG`, `classify_link`,
ADR: `docs/adr/0014-doc-scheme-retirement.md`.

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

**H10 — Two block query sources, DI-selected. ✅ FIXED (2026-07-02).**
`crates/holon/src/sync/loro_block_query_source.rs` and
`.../turso_block_query_source.rs` both exist; "what reads a block" depends on the
wired graph, so the same logical read has two implementations that could drift.
**Fix (pinned, not merged):** a query-source *equivalence* PBT now proves the two
arms agree. `tests/turso_block_query_source_round_trip_pbt.rs::loro_and_turso_query_sources_agree`
generates one store, writes it through the production Turso create path *and* seeds
a Turso-free `LoroBackend` with the same blocks, snapshots both via their
`BlockQuerySource`, and asserts the two `BlockSnapshot`s reproduce the generated
store field-for-field (id-keyed) **and** in canonical per-parent sibling order.
This is meaningful now (not earlier) because `requires` hydration (M1) and the
`BlockWire` edge-field types (M3) are correct — the read arms carry the same fields
to compare. **Scope — a disclosed asymmetry, not a bug:** the property compares
BLOCKS only. The Loro arm returns empty `focus_roots` by design (navigation focus
is a Turso matview with no Loro-native source — see the `snapshot()` note in
`loro_block_query_source.rs`), so `focus_roots` is a known, documented divergence
between the two sources and is out of scope for this equivalence. **Route:** hosted
as a standalone in-crate PBT rather than a composed-catalog invariant, because the
composed harness wires a *no-Turso* frontend (only the Loro arm is reachable there);
comparing both arms from one seed would need a new component holding both backends
plus a new cap — real plumbing, not the cheap wire()+catalog-line the composed
route is reserved for. Anchor: `loro_and_turso_query_sources_agree`,
`seed_loro_backend`.

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
(`holon-loro/src/loro_sync_controller.rs`).

---

## 5. What the storm says about the architecture

**Strengths the wall makes visible.** The format ACL is genuinely clean — the
`FileFormatAdapter` trait is a real seam: org is the live implementation, and
markdown implements the *same* trait ready to drop in (though it is not yet wired
into file-sync — zero prod dependents today; `holon-core/src/file_format.rs` still
speaks of it in the subjunctive). The bare-id↔scheme
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
a type-level marker for
"intentionally not round-tripped" columns (`depth`, `sort_key`, `collapsed`, …);
the `StoredBlock` newtype so a serde-path `Block` can't impersonate a
matview-hydrated one (H1 residue). Done since first writing: BLOCK_LORODOC
marked superseded (H11); `TryFrom<StorageEntity>` fails loud on
missing/malformed columns (H8); `ChangeOp` parent refs typed `EntityUri` (H2);
`doc:`-scheme eliminated and `set_is_document` retired (H7, 2026-07-02).

**These hotspots are also PBT targets.** Every 🟠 event is a candidate state-machine
transition and every 🔴 a candidate invariant. No open candidates remain. Realized:
Loro-vs-Turso query-source equivalence (H10 — now pinned by
`loro_and_turso_query_sources_agree`, a standalone in-crate PBT; graduates into the
composed catalog if/when a dual-backend component makes hosting it cheap),
share/merge-back behaviour (H5 —
now pinned by `shared_tree::tests::{none_retention_reintegration_fails_loudly,
share_subtree_payload_is_plaintext_not_encrypted}`, 2026-07-02),
markdown round-trip identity (H6 — now pinned by an in-crate PBT; graduates into the
composed catalog when markdown is wired into file-sync), edge-field
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
