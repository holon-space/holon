//! Unified bidirectional sync controller for Org files ↔ block store.
//!
//! Unified bidirectional sync: a single component
//! that uses the **projection + diff-ingestion** pattern:
//!
//! - `last_projection`: what we last wrote to (or confirmed on) disk, per file.
//! - Echo suppression: `disk_content == last_projection[file]` (no timing window).
//! - External edits: detected by diffing against `last_projection`.
//!
//! The controller runs on a single task via `tokio::select!`, so `on_file_changed`
//! and `on_block_changed` are serialized — no concurrent access to `last_projection`.
//!
//! **Decoupled from Loro/Turso**: uses `BlockReader` and `DocumentManager` traits.

use anyhow::{Context, Result};
use holon_api::block::Block;
use holon_api::capability::Consolidator;
use holon_api::{EntityUri, SnapshotBlock, Value};
use holon_api::{POSITION_AFTER_BLOCK_ID_PARAM, ROUTING_DOC_URI_KEY};
use holon_core::file_format::FileFormatAdapter;
use holon_core::fractional_index::default_sort_key;
use holon_core::CanonicalPath;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

use holon_core::block_ordering::BlockOrdering;
use holon_core::DownstreamProjection;
use indexmap::IndexMap;

use crate::sync_ports::{
    AliasRegistrar, BlockReader, DocumentManager, ImageDataProvider, ThreeWayTextMerge,
};
use crate::{BaseKey, BaseStore, FileSystem, SyncBaseStore};

/// Bump when the org renderer changes in a way that alters the canonical
/// projection bytes (formatting, property ordering, directive layout, …).
/// Mismatch on next boot forces a one-shot re-ingest per file so the stored
/// `file.content_hash` snaps to the new canonical form.
pub const RENDERER_VERSION: &str = "1";

/// A single block change carried from the CDC block feed to the org controller,
/// so a per-edit re-render can update just the changed block instead of
/// re-reading the whole document (the O(N) recursive-CTE `get_blocks`).
///
/// `Upsert` carries the feed's (matview-derived) block only for classification
/// — the controller refreshes the block's authoritative content via
/// [`BlockReader::get_block_authoritative`] before writing, so seed and refresh
/// share one authority (`block_raw`).
#[derive(Debug, Clone)]
pub enum BlockDelta {
    /// A block was inserted or updated.
    Upsert(Block),
    /// A block was removed (id only — its document is no longer resolvable
    /// from the feed, so the controller takes the full re-render path).
    Remove(EntityUri),
}

pub struct FileSyncController {
    /// What we last wrote to (or confirmed on) disk, per file.
    /// Uses CanonicalPath to resolve macOS /var → /private/var symlinks,
    /// so scan_org_files and file watcher events match the same key.
    /// Session-only, populated lazily on first miss.
    last_projection: HashMap<CanonicalPath, String>,

    /// Phase 1 fast-path: `sha256(RENDERER_VERSION || render(parsed_blocks))`
    /// per file, persisted via `file.content_hash`. Populated at startup
    /// from `block_reader.load_file_hashes()` so a cold boot of an unchanged
    /// vault skips block-table batches entirely (parses + renders + hashes,
    /// then compares — no SQL writes when the hash matches).
    last_projection_hash: HashMap<CanonicalPath, String>,

    /// Cheap dirty-check signature `(mtime, size)` per tracked path. Used by
    /// `poll_external_changes` to skip the expensive `read_to_string` when
    /// `stat()` shows the file hasn't changed since we last looked. Updated
    /// after every read so subsequent polls compare against the post-read
    /// state. A missing entry forces a full read (treats the path as dirty).
    disk_signatures: HashMap<CanonicalPath, (std::time::SystemTime, u64)>,

    /// Phase 3: the org reconciler's per-file diff **base** — the parsed block
    /// snapshot of `last_projection[file]` (or the consolidated store on cold
    /// boot). The `on_file_changed` diff reads its "old" side from here through
    /// the [`BaseStore`] seam instead of re-parsing `last_projection` or
    /// special-casing the first-run cache read. In-memory (re-seeded per file
    /// each session from the consolidated store), keyed `BaseKey{org, file}`.
    base_store: SyncBaseStore,

    /// The exact `last_projection` string each `base_store` entry was parsed
    /// from. The base for a file is fresh iff this matches the current
    /// `last_projection[file]`; otherwise it is re-parsed. This keys freshness
    /// on content, so the base can never desync from `last_projection` no matter
    /// which render path last updated it.
    base_source: HashMap<CanonicalPath, String>,

    /// Reads blocks by document ID.
    block_reader: Arc<dyn BlockReader>,

    /// Document entity CRUD (decoupled from Turso).
    doc_manager: Arc<dyn DocumentManager>,

    /// Root directory for org files.
    root_dir: PathBuf,

    /// Callback to register doc_id → path aliases in the storage layer.
    /// Set by the DI wiring when Loro is available.
    alias_registrar: Option<Arc<dyn AliasRegistrar>>,

    /// Shell command to run after each org file write (from holon.toml).
    post_write_hook: Option<String>,

    /// Binary image data provider (Loro-backed). Used to materialize image
    /// files to disk on render and ingest them from disk on parse.
    image_data: Option<Arc<dyn ImageDataProvider>>,

    /// File format adapter — delegates parse/render so the controller works
    /// across formats. Defaults to `OrgFormatAdapter`; future markdown /
    /// notion / logseq adapters plug in here without changing the
    /// controller's logic.
    format: Arc<dyn FileFormatAdapter>,

    /// Positional-intent writer. Used during disk-order replay to move
    /// misaligned blocks into the position recorded in the org file.
    ordering: Arc<dyn BlockOrdering>,

    /// Downstream convergent feed (consolidator → SQL sink). Present when a
    /// separate consolidator owns block storage; `None` when the SQL store
    /// itself is the consolidator (degraded mode). After sending create /
    /// relocate intents during a scan, the controller `flush()`es this so the
    /// sink rows are written by the projection — the single sink-writer — never
    /// by org directly.
    downstream: Option<Arc<dyn DownstreamProjection>>,

    /// Disk access port (ADR 0011). Real fs in production; in-memory in tests.
    fs: Arc<dyn FileSystem>,

    /// Per-doc incrementally-maintained block cache, seeded from `get_blocks`
    /// (authoritative `block_raw`, ordered `sort_key, id`) and mutated in place
    /// on content-only edits. `IndexMap` preserves the seed's insertion order —
    /// the order the renderer relies on for sibling layout (`Block` carries no
    /// `sort_key`, so order is trusted from the seed, never re-derived). Keyed
    /// by doc id; evicted implicitly by never being seeded until first edited.
    /// Structural changes (insert/remove/move/`tags`) reseed the whole doc.
    doc_blocks: HashMap<EntityUri, IndexMap<EntityUri, Block>>,

    /// 3-way text-content merger for the no-store conflict path. Present only
    /// when wired (production, via a transient LoroText impl). Consulted only in
    /// `Consolidator::Store` (SqlOnly) mode: when an org-file edit and a UI edit
    /// concurrently changed the SAME block's text content (both diverged from
    /// the BaseStore base), the disk edit is 3-way merged with the current store
    /// content instead of clobbering it (whole-value LWW). In `Upstream` (Loro)
    /// mode this is left unused — the live CRDT already merges concurrent edits,
    /// so adding a second merge here would be wrong.
    text_merge: Option<Arc<dyn ThreeWayTextMerge>>,

    /// Initial-scan feed-barrier batching (boot ingest latency, Options 0+1).
    /// `None` in steady state — each runtime `on_file_changed` pays its own
    /// per-file `wait_for_blocks_in_feed` barrier (unchanged). `Some(buf)` only
    /// between [`begin_initial_scan`](Self::begin_initial_scan) and
    /// [`finish_initial_scan`](Self::finish_initial_scan): the per-file feed
    /// waits (sites A and C) instead push their expected ids into `buf` and skip
    /// the wait, so the whole cold-boot vault ingests without N×(≤2s) round-trips;
    /// `finish_initial_scan` then does ONE convergence wait over the union before
    /// `signal_ready`. `block_raw` is written synchronously per file, so the
    /// per-file `get_blocks` count-check (the intra-file write-success gate) and
    /// wait B (`ordering.children`, the ordering-authority propagation gate) stay
    /// in place and cover correctness; only the sidebar-facing `block`-matview
    /// `LiveData` feed is deferred to end-of-scan. Scoped to the initial scan —
    /// runtime edits keep the per-edit barrier and its fail-loud stall detection.
    scan_feed_ids: Option<Vec<String>>,

    /// Write-back quarantine (dogfood 2026-07-10 region data-loss guard). A file
    /// whose ingest FAILED partway (`ingest_file` returned `Err` — a rejected
    /// block op, a parsed-vs-landed count mismatch, a stalled feed) is recorded
    /// here so no write-back path re-renders it from the DB. The DB holds only a
    /// PREFIX of the file's blocks after a partial ingest, so rendering it would
    /// overwrite the on-disk file with a truncated view — silent data loss. A
    /// quarantined file is skipped by every write-back until a later ingest of it
    /// SUCCEEDS (`ingest_file` returns `Ok`), which clears the entry. Keyed by
    /// the same `CanonicalPath` as `last_projection`.
    quarantined: HashSet<CanonicalPath>,
}

impl FileSyncController {
    /// Construct a controller with an explicit `FileFormatAdapter`. The
    /// format-default convenience ctors live with their format crates (e.g.
    /// `holon_orgmode::new_org_sync_controller`); the engine itself is
    /// format-agnostic.
    pub fn with_format(
        block_reader: Arc<dyn BlockReader>,
        doc_manager: Arc<dyn DocumentManager>,
        root_dir: PathBuf,
        format: Arc<dyn FileFormatAdapter>,
        ordering: Arc<dyn BlockOrdering>,
        fs: Arc<dyn FileSystem>,
    ) -> Self {
        // Canonicalize root_dir so strip_prefix works with canonical file paths
        // (macOS: /var → /private/var symlink resolution).
        let root_dir = CanonicalPath::new(&root_dir).into_path_buf();
        Self {
            last_projection: HashMap::new(),
            last_projection_hash: HashMap::new(),
            disk_signatures: HashMap::new(),
            base_store: SyncBaseStore::in_memory(),
            base_source: HashMap::new(),
            block_reader,
            doc_manager,
            root_dir,
            alias_registrar: None,
            post_write_hook: None,
            image_data: None,
            format,
            ordering,
            downstream: None,
            fs,
            doc_blocks: HashMap::new(),
            text_merge: None,
            scan_feed_ids: None,
            quarantined: HashSet::new(),
        }
    }

    /// Enter initial-scan mode: the per-file feed barriers (sites A and C in
    /// `on_file_changed`) buffer their expected ids instead of waiting, so the
    /// cold-boot vault ingests without N×(≤2s) feed round-trips. Must be paired
    /// with [`finish_initial_scan`](Self::finish_initial_scan), which drains the
    /// buffer with one convergence wait. Boot ingest latency, Option 1.
    pub fn begin_initial_scan(&mut self) {
        self.scan_feed_ids = Some(Vec::new());
    }

    /// Whether the controller is currently in initial-scan (feed-barrier
    /// batching) mode. `false` in steady state — used by tests to prove the
    /// scan flag does not leak past `finish_initial_scan`.
    pub fn in_initial_scan(&self) -> bool {
        self.scan_feed_ids.is_some()
    }

    /// Leave initial-scan mode: do exactly ONE `wait_for_blocks_in_feed` over the
    /// union of every id the scan's deferred barriers buffered, then reset to
    /// steady state. Fail loud (never silently continue) if the `block`-matview
    /// feed has not converged within `budget_ms` — a stalled projection/CDC is a
    /// real bug. Called before `signal_ready` so a stall becomes a scan failure.
    pub async fn finish_initial_scan(&mut self, budget_ms: u64) -> Result<()> {
        let mut ids = self.scan_feed_ids.take().unwrap_or_default();
        ids.sort();
        ids.dedup();
        let t = std::time::Instant::now();
        let caught_up = if ids.is_empty() {
            true
        } else {
            self.block_reader
                .wait_for_blocks_in_feed(&ids, budget_ms)
                .await
        };
        tracing::debug!(
            target: "holon_latency",
            stage = "boot_feed_converge",
            ms = t.elapsed().as_millis() as u64,
            blocks = ids.len() as u64,
            caught_up = caught_up,
            "holon_latency",
        );
        // Steady-state guard: the scan flag must not leak past finish.
        debug_assert!(
            self.scan_feed_ids.is_none(),
            "scan_feed_ids must be None after finish_initial_scan"
        );
        if !caught_up {
            anyhow::bail!(
                "[finish_initial_scan] block feed did not converge within {budget_ms}ms \
                 for {} expected id(s) — projection/CDC stalled during the initial scan",
                ids.len()
            );
        }
        Ok(())
    }

    /// The initial-scan feed barrier (sites A and C). During the scan
    /// (`scan_feed_ids.is_some()`) the expected ids are buffered for the single
    /// end-of-scan convergence wait and this returns immediately. In steady
    /// state it performs the per-file `wait_for_blocks_in_feed` exactly as
    /// before (byte-identical runtime behavior). Emits `boot_feed_wait` on the
    /// `holon_latency` target so the cost — and how much of the 2s ceiling binds
    /// — is measurable per file. `site` is `"updates"` (A) or `"creates"` (C).
    async fn feed_barrier(&mut self, ids: &[String], site: &'static str) -> bool {
        if let Some(buf) = self.scan_feed_ids.as_mut() {
            buf.extend(ids.iter().cloned());
            tracing::debug!(
                target: "holon_latency",
                stage = "boot_feed_wait",
                ms = 0u64,
                caught_up = true,
                skipped = true,
                site = site,
                "holon_latency",
            );
            return true;
        }
        // Steady-state path — byte-identical to the pre-batching per-file wait.
        debug_assert!(self.scan_feed_ids.is_none());
        let t = std::time::Instant::now();
        let caught_up = self.block_reader.wait_for_blocks_in_feed(ids, 2000).await;
        tracing::debug!(
            target: "holon_latency",
            stage = "boot_feed_wait",
            ms = t.elapsed().as_millis() as u64,
            caught_up = caught_up,
            skipped = false,
            site = site,
            "holon_latency",
        );
        caught_up
    }

    pub fn with_alias_registrar(mut self, registrar: Arc<dyn AliasRegistrar>) -> Self {
        self.alias_registrar = Some(registrar);
        self
    }

    /// Wire the 3-way text-content merger (the no-store conflict path). Without
    /// it, a concurrent file-vs-UI edit in SqlOnly mode resolves by whole-value
    /// last-writer-wins; with it, the disk edit is merged against the current
    /// store content through a transient CRDT text (Model.md merge-fidelity
    /// ladder). No-op in `Upstream` (Loro) mode — the live CRDT merges there.
    pub fn with_text_merge(mut self, merger: Arc<dyn ThreeWayTextMerge>) -> Self {
        self.text_merge = Some(merger);
        self
    }

    /// Wire the downstream consolidator→sink projection. Without it the
    /// controller assumes the SQL store is itself the consolidator (degraded
    /// mode) and `create_in_tree` returning `false` routes creates through the
    /// command bus.
    pub fn with_downstream_projection(mut self, projection: Arc<dyn DownstreamProjection>) -> Self {
        self.downstream = Some(projection);
        self
    }

    pub fn with_post_write_hook(mut self, cmd: String) -> Self {
        self.post_write_hook = Some(cmd);
        self
    }

    pub fn with_image_data(mut self, provider: Arc<dyn ImageDataProvider>) -> Self {
        self.image_data = Some(provider);
        self
    }

    /// Initialize last_projection from the block reader's current state.
    ///
    /// Must be called at startup BEFORE scanning files, so that we have a
    /// diff base for detecting external edits.
    pub async fn initialize(&mut self) -> Result<()> {
        // Model.md invariant 11: the vault must not be under a byte-level file
        // syncer. Scan for conflict artifacts (Syncthing/iCloud/Dropbox) and
        // fail loud if any exist — they get re-ingested as duplicate-ID docs.
        let scanned = self
            .fs
            .scan_directory(&self.root_dir)
            .await
            .with_context(|| {
                format!(
                    "[FileSyncController] scan vault {} for sync-conflict artifacts",
                    self.root_dir.display()
                )
            })?;
        let conflicts = crate::sync_conflict::find_sync_conflict_artifacts(&scanned.files);
        if !conflicts.is_empty() {
            return Err(crate::sync_conflict::conflict_artifacts_error(
                &self.root_dir,
                &conflicts,
            ));
        }

        // Phase 1 fast-path: load persisted `(file_id, content_hash)` pairs
        // from the `file` table BEFORE the in-process cache has replayed file
        // events. If an on-disk file's `hash(RENDERER_VERSION || disk_bytes)`
        // matches its stored hash, `on_file_changed` skips block ingest
        // entirely — the dominant cold-boot cost. See plan §Phase 1.
        match self.block_reader.load_file_hashes().await {
            Ok(rows) => {
                for (uri, hash) in rows {
                    if let Some(canonical) = self.file_uri_to_canonical_path(&uri) {
                        self.last_projection_hash.insert(canonical, hash);
                    }
                }
                info!(
                    "[FileSyncController] Loaded last_projection_hash for {} files \
                     (will skip ingest when disk_bytes hash matches)",
                    self.last_projection_hash.len()
                );
            }
            Err(e) => {
                warn!(
                    "[FileSyncController] load_file_hashes failed; cold-boot fast \
                     path disabled, will re-ingest every file. Error: {e}"
                );
            }
        }

        // last_projection (full rendered string) is intentionally NOT eagerly
        // populated by walking every block — it's a session-only cache used
        // for echo suppression, populated lazily on first miss by
        // `on_file_changed`. Walking iter_documents_with_blocks here would
        // pay parse+render cost for every doc on every boot.
        info!("[FileSyncController] Initialize complete");
        Ok(())
    }

    /// Convert a `file:<encoded-path>` EntityUri back to a CanonicalPath
    /// relative to this controller's `root_dir`. Returns None if the URI
    /// scheme isn't `file:` or the on-disk path doesn't exist (which can
    /// legitimately happen if the user deleted the file while the row
    /// lingers — next sync will tombstone it).
    fn file_uri_to_canonical_path(&self, uri: &EntityUri) -> Option<CanonicalPath> {
        if uri.scheme() != "file" {
            return None;
        }
        let encoded = uri.id();
        // `EntityUri::file` percent-encodes path segments; reverse it before
        // joining with root_dir so spaces etc. match the on-disk file name.
        // `decode_utf8_lossy` substitutes U+FFFD for invalid sequences rather
        // than swallowing them — keeps the fast-path correct for ASCII paths
        // and visibly broken for the rare non-UTF-8 case.
        let decoded = percent_encoding::percent_decode_str(encoded).decode_utf8_lossy();
        let abs = self.root_dir.join(decoded.as_ref());
        Some(CanonicalPath::new(&abs))
    }

    /// Phase 1: `sha256(RENDERER_VERSION || consolidator_tag || disk_bytes)`.
    /// Same hash function is used both to gate ingest on read and to stamp
    /// `file.content_hash` after write so the next boot's gate compares
    /// like-for-like.
    ///
    /// The consolidator tag makes flipping `[loro] enabled` invalidate every
    /// stored hash: a vault written under SqlOnly must NOT take the cold-boot
    /// fast path on its first Loro-enabled boot — that skip is exactly what
    /// left pre-Loro vaults with a populated SQL DB and an empty Loro tree
    /// (the 2026-06-10 live bug). The forced re-ingest runs the diff loop's
    /// re-seed pass; the hash is then re-stamped under the new tag, so only
    /// the first boot after a flip pays the full parse.
    fn projection_hash(&self, disk_bytes: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(RENDERER_VERSION.as_bytes());
        hasher.update(b"\0");
        hasher.update(format!("{:?}", self.ordering.consolidator()).as_bytes());
        hasher.update(b"\0");
        hasher.update(disk_bytes.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Cold-boot fast-path guard: is this file's content present in EVERY
    /// active store? The caller has already proven the SQL side (its stored
    /// `content_hash` matched the disk bytes); this proves the Loro side.
    ///
    /// - SqlOnly mode (Loro not an active store): `in_tree` answers `None`, so
    ///   the check degrades to SQL-only — the historical behavior. `true`.
    /// - Loro mode: the doc's root block (`block:<#+ID>`) must resolve to a
    ///   Loro tree node. `Some(false)` is the reset hole — SQL kept the row but
    ///   the Loro tree was reset to empty — so refuse the skip and re-ingest.
    ///
    /// A file the fast path can even reach was rendered by Holon (its hash
    /// matched a hash we stamped), so it always carries `#+ID:`. If it somehow
    /// does not, we cannot cheaply resolve the root block, so we refuse the
    /// skip and let the full ingest resolve identity — never skip blind.
    async fn content_present_in_all_stores(&self, disk_content: &str) -> Result<bool> {
        let Some(bare) = self.format.doc_id_from_content(disk_content) else {
            return Ok(false);
        };
        let root = EntityUri::block(&bare);
        let present = self
            .ordering
            .in_tree(&root)
            .await
            .map_err(|e| anyhow::anyhow!("[FileSyncController] in_tree({root}): {e:#}"))?;
        // None → no separate tree (SqlOnly): SQL is the only active store.
        Ok(present.unwrap_or(true))
    }

    /// Handle an EXTERNAL file deletion (the user removed the org file outside
    /// Holon — `rm` in the vault, a file manager, a git checkout). Reached from
    /// `on_file_changed` when the changed path no longer exists, and from
    /// `poll_tracked_files` when a tracked path stops stat-ing.
    ///
    /// Cascade-deletes the vanished document's blocks from the store: content
    /// blocks bottom-up (children before parents, so each delete targets a
    /// still-present node regardless of whether the tree backing cascades
    /// subtree deletes), then the page block itself. All deletes go through
    /// `BlockOrdering::delete_in_tree` — the same single org→block write seam
    /// the diff-ingestion delete pass uses.
    #[tracing::instrument(skip(self, canonical), name = "org.on_file_deleted", fields(path = %path.display()))]
    async fn on_file_deleted(&mut self, path: &Path, canonical: &CanonicalPath) -> Result<()> {
        // Resolve the vanished file's document. The disk bytes are gone, so
        // identity comes from the last projected content's `#+ID:` (survives
        // renames, same authority as the ingest path); when this session never
        // projected the file, fall back to name-chain lookup (get-only — a
        // deletion must never mint page blocks).
        let last = self.last_projection.get(canonical).cloned();
        let document = match last
            .as_deref()
            .and_then(|l| self.format.doc_id_from_content(l))
        {
            Some(bare) => self.doc_manager.get_by_id(&EntityUri::block(&bare)).await?,
            None => {
                let rel_path = path.strip_prefix(&self.root_dir).map_err(|e| {
                    anyhow::anyhow!(
                        "Deleted file {} not under root {}: {}",
                        path.display(),
                        self.root_dir.display(),
                        e
                    )
                })?;
                let segments = path_to_name_chain(rel_path);
                let segment_refs: Vec<&str> = segments.iter().map(|s| s.as_str()).collect();
                self.doc_manager.find_by_name_chain(&segment_refs).await?
            }
        };
        let Some(document) = document else {
            // A file we never ingested vanished — nothing in the store to
            // delete. Disclosed, then drop any per-file tracking state.
            debug!(
                "[FileSyncController] Deleted file {} has no document entity — nothing to cascade",
                path.display()
            );
            self.forget_file_state(canonical);
            return Ok(());
        };
        let document_uri = document.id.clone();

        let blocks = self.block_reader.get_blocks(&document_uri).await?;
        info!(
            "[FileSyncController] File deleted externally: {} — cascade-deleting document {} \
             ({} blocks)",
            path.display(),
            document_uri,
            blocks.len(),
        );

        // Order children before parents: depth (hops until the parent leaves
        // the doc's block set) descending.
        // Owned parent map (no borrows of `blocks` escape into the closure —
        // the `#[instrument]` async wrapper otherwise infers a 'static bound).
        let parent_of: HashMap<EntityUri, EntityUri> = blocks
            .iter()
            .map(|b| (b.id.clone(), b.parent_id.clone()))
            .collect();
        let depth_of = |id: &EntityUri| -> usize {
            let mut depth = 0;
            let mut cur = id;
            while let Some(parent) = parent_of.get(cur) {
                if parent == cur {
                    break; // self-parent guard
                }
                cur = parent;
                depth += 1;
                if depth > 100 {
                    break; // cycle guard, matches the parser's depth bound
                }
            }
            depth
        };
        let mut ordered: Vec<EntityUri> = blocks
            .iter()
            .map(|b| b.id.clone())
            .filter(|id| *id != document_uri)
            .collect();
        ordered.sort_by_key(|id| std::cmp::Reverse(depth_of(id)));

        for block_id in ordered {
            let mut params: holon_api::StorageEntity = HashMap::new();
            params.insert("id".into(), Value::String(block_id.to_string()));
            params.insert(
                ROUTING_DOC_URI_KEY.into(),
                Value::String(document_uri.to_string()),
            );
            self.ordering.delete_in_tree(params).await.map_err(|e| {
                anyhow::anyhow!(
                    "delete_in_tree({block_id}) for deleted file {}: {e:#}",
                    path.display()
                )
            })?;
        }

        // The page block last — its children are gone.
        let mut params: holon_api::StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(document_uri.to_string()));
        params.insert(
            ROUTING_DOC_URI_KEY.into(),
            Value::String(document_uri.to_string()),
        );
        self.ordering.delete_in_tree(params).await.map_err(|e| {
            anyhow::anyhow!(
                "delete_in_tree(page {}) for deleted file {}: {e:#}",
                document_uri,
                path.display()
            )
        })?;

        // Publish the consolidator's accumulated deletes to the SQL sink
        // (same single-sink-writer contract as the ingest path's flush).
        if let Some(downstream) = &self.downstream {
            downstream
                .flush()
                .await
                .map_err(|e| anyhow::anyhow!("downstream projection flush after delete: {e}"))?;
        }

        self.forget_file_state(canonical);
        // Also clear the diff base so a later re-create of the same document
        // id starts from an empty base (all blocks are creates), not from the
        // deleted snapshot.
        self.base_store
            .put_base(&BaseKey::file("org", document_uri.as_str()), HashMap::new());
        Ok(())
    }

    /// Drop every per-file tracking entry for a vanished path.
    fn forget_file_state(&mut self, canonical: &CanonicalPath) {
        self.last_projection.remove(canonical);
        self.last_projection_hash.remove(canonical);
        self.disk_signatures.remove(canonical);
        self.base_source.remove(canonical);
    }

    /// Handle a file change event from the FileWatcher.
    ///
    /// Thin write-back-quarantine wrapper around [`ingest_file`](Self::ingest_file):
    /// a partial ingest (an `Err`) records the file in `quarantined` so no
    /// write-back path re-renders its truncated DB state over disk (dogfood
    /// 2026-07-10 region data-loss guard). A successful ingest clears the
    /// quarantine. The `Err` is still propagated so the caller's degraded-mode
    /// banner / survival logic is unchanged.
    pub async fn on_file_changed(&mut self, path: &Path) -> Result<()> {
        let canonical = CanonicalPath::new(path);
        match self.ingest_file(path).await {
            Ok(()) => {
                if self.quarantined.remove(&canonical) {
                    info!(
                        "[FileSyncController] write-back quarantine CLEARED for {} \
                         (ingest fully succeeded)",
                        path.display()
                    );
                }
                Ok(())
            }
            Err(e) => {
                // Partial ingest: the DB now holds only a PREFIX of this file's
                // blocks. Quarantine it so write-back never renders that prefix
                // over the intact on-disk file. Loud + disclosed.
                if self.quarantined.insert(canonical) {
                    tracing::error!(
                        path = %path.display(),
                        error = %format!("{e:#}"),
                        "[FileSyncController] ingest FAILED partway — QUARANTINING this file \
                         from write-back so its truncated DB state is not rendered over disk. \
                         Un-quarantines on the next fully-successful ingest.",
                    );
                }
                Err(e)
            }
        }
    }

    /// True when `path` is quarantined from write-back (its last ingest failed
    /// partway). A quarantined file's DB state is a truncated prefix, so any
    /// write-back path must SKIP it (loud + disclosed) rather than render that
    /// prefix over the intact on-disk file. See [`quarantined`](Self::quarantined).
    fn is_quarantined(&self, path: &Path) -> bool {
        if self.quarantined.contains(&CanonicalPath::new(path)) {
            tracing::error!(
                path = %path.display(),
                "[FileSyncController] SKIPPING write-back for quarantined file — its last \
                 ingest failed partway, so the DB holds only a truncated prefix of its \
                 blocks; rendering it over disk would DESTROY the un-ingested lines. \
                 The on-disk file is left intact until a clean re-ingest clears the quarantine.",
            );
            true
        } else {
            false
        }
    }

    /// Echo suppression: if disk content matches last_projection, skip.
    /// Otherwise, diff against last_projection to compute create/update/delete ops.
    #[tracing::instrument(skip(self), name = "org.ingest_file", fields(path = %path.display()))]
    async fn ingest_file(&mut self, path: &Path) -> Result<()> {
        // Model.md invariant 11: skip (only) a byte-syncer conflict artifact that
        // appears at runtime — ingesting it would create a duplicate-ID document.
        // Disclosed, never silent; normal files are unaffected.
        if crate::sync_conflict::is_sync_conflict_artifact(path) {
            tracing::error!(
                path = %path.display(),
                "[FileSyncController] Model.md invariant 11: byte-syncer conflict artifact detected \
                 at runtime — SKIPPING ingestion of this file. A byte-level file syncer \
                 (Syncthing/iCloud/Dropbox) on the vault is out of contract; cross-device \
                 convergence must go through Loro/P2P."
            );
            return Ok(());
        }
        let canonical = CanonicalPath::new(path);
        let disk_content = match self.fs.read_to_string(path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // External deletion (user removed the file outside Holon):
                // cascade-delete the document's blocks. No echo-suppression
                // needed — no Holon code path removes org files, so a vanished
                // file is always an external deletion.
                return self.on_file_deleted(path, &canonical).await;
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("[FileSyncController] Cannot read {}", path.display())
                });
            }
        };

        let last = self
            .last_projection
            .get(&canonical)
            .map(|s| s.as_str())
            .unwrap_or("");

        tracing::debug!(
            "[ORGSYNC_ENTER] {} disk_len={} last_len={} has_key={} equal={}",
            path.display(),
            disk_content.len(),
            last.len(),
            self.last_projection.contains_key(&canonical),
            disk_content == last,
        );

        // Echo suppression: skip if we have a prior projection and content matches.
        // An absent entry means "first time seeing this file" — always process it
        // to create the document entity (needed for block→file sync).
        if self.last_projection.contains_key(&canonical) && disk_content == last {
            debug!(
                "[FileSyncController] Skipping {} — matches last_projection",
                path.display()
            );
            return Ok(());
        }

        // Phase 1 cold-boot fast-path: when `last_projection` has no entry
        // (first time we see this file this session) but `last_projection_hash`
        // — loaded from `file.content_hash` at startup — matches the disk
        // bytes hashed with the same renderer-version-prefixed sha256, the
        // ingest path is a guaranteed no-op (we wrote this content last time
        // and nothing changed on disk). Skip ingest entirely; stamp
        // `last_projection` so subsequent in-session echo-suppression hits.
        //
        // Approach A (disk-bytes hash, not projection hash): the false-miss
        // case (user externally edited in a benign way — trailing newline,
        // property reorder — that re-renders to the same projection) costs
        // exactly one parse + diff + zero block ops (Phase 2 ensures the
        // edge sets don't churn either), then re-stamps the hash. Bounded
        // and only fires on actual edits. Approach B (projection hash) would
        // parse + render every file on every boot to confirm "skip" — a
        // guaranteed cost per boot we don't pay here.
        let disk_hash = self.projection_hash(&disk_content);
        if let Some(stored) = self.last_projection_hash.get(&canonical) {
            // Invariant: fast-path skip requires the content present in EVERY
            // active store, not just SQL. The matching hash proves the SQL side;
            // `content_present_in_all_stores` additionally proves the Loro side
            // when Loro is an active store. A SQL hash match with an empty Loro
            // tree (the 2026-07-06 reset hole: fresh `.loro` + retained SQL row)
            // must NOT skip — skipping leaves SQL and Loro silently diverged and
            // the next Loro create fails at `resolve_parent_tree_id`.
            if stored == &disk_hash && self.content_present_in_all_stores(&disk_content).await? {
                debug!(
                    "[FileSyncController] Skipping {} — disk hash matches \
                     stored file.content_hash and content present in all active \
                     stores (cold-boot fast path)",
                    path.display()
                );
                self.last_projection.insert(canonical.clone(), disk_content);
                return Ok(());
            }
        }

        info!(
            "[FileSyncController] Processing external change: {}",
            path.display()
        );

        // Boot-ingest instrumentation (holon_latency target, Option 0). Marks the
        // start of the real ingest path (past echo-suppression + the cold-boot
        // fast-path skip). `boot_parse` / `boot_write` / `boot_place_wait` /
        // `boot_feed_wait` split this file's cost; `boot_file` (per-file total) is
        // emitted by the scan driver in `run_file_sync_controller`.
        let t_ingest = std::time::Instant::now();

        // An external ingest mutates `block_raw` for arbitrary blocks of this
        // file's document — invalidate the incremental block cache so the next
        // `on_block_changed` reseeds from authority rather than rendering stale
        // cached content. Reached only past echo-suppression and the cold-boot
        // fast path, so our own write-back echo never clears it. Coarse (whole
        // map) because external ingests are rare relative to block edits and
        // resolving this path's doc id here would cost an extra lookup; the only
        // effect is a one-time reseed on each doc's next edit.
        self.doc_blocks.clear();

        let rel_path = path.strip_prefix(&self.root_dir).map_err(|e| {
            anyhow::anyhow!(
                "File {} not under root {}: {}",
                path.display(),
                self.root_dir.display(),
                e
            )
        })?;

        // Resolve the document entity. `#+ID: <bare>` (when present) is the
        // authoritative identity — it survives renames. When absent we fall
        // back to name-chain resolution and emit `#+ID:` on the next render
        // so subsequent loads pick up the stable identity from the file.
        let bare_id_in_file = self.format.doc_id_from_content(&disk_content);
        let segments = path_to_name_chain(rel_path);
        let segment_refs: Vec<&str> = segments.iter().map(|s| s.as_str()).collect();
        let document = match bare_id_in_file.as_deref() {
            Some(bare) => {
                let id = EntityUri::block(bare);
                match self.doc_manager.get_by_id(&id).await? {
                    Some(doc) => doc,
                    None => {
                        let title = segments
                            .last()
                            .cloned()
                            .unwrap_or_else(|| "unknown".to_string());
                        let parent_id = if segments.len() > 1 {
                            let parent_segments: Vec<&str> =
                                segment_refs[..segments.len() - 1].to_vec();
                            self.doc_manager
                                .get_or_create_by_name_chain(&parent_segments)
                                .await?
                                .id
                        } else {
                            EntityUri::no_parent()
                        };
                        let mut new_doc = Block::new_text(id, parent_id, title);
                        new_doc.set_page(true);
                        // FORCE the `#+ID` as the page identity. A sibling file
                        // scanned earlier under a same-named subdirectory (e.g.
                        // `Frontends/GPUI.org` next to `Frontends.org`) mints a
                        // random-id name-chain placeholder page for the shared
                        // `Frontends` segment; a plain `create` would de-dup by
                        // `(parent, title)` and hand that placeholder's id back,
                        // so writeback would re-mint this file's `#+ID` (data
                        // loss). `create_forcing_id` keeps the authoritative id.
                        self.doc_manager.create_forcing_id(new_doc).await?
                    }
                }
            }
            None => {
                self.doc_manager
                    .get_or_create_by_name_chain(&segment_refs)
                    .await?
            }
        };
        let document_uri = document.id.clone();

        // The document is a block too. Send it to the consolidator as a create
        // intent so it becomes a real node carrying its content + `Page` tag —
        // not a content-less placeholder auto-created when a child's
        // `create_in_tree` can't resolve its parent. Without this, the
        // downstream projection would write that empty placeholder over the
        // document's real row (orphaning every doc). Idempotent on re-scan
        // (the node already exists → position-only). No-op in degraded mode
        // (`create_in_tree` returns false; the doc manager owns the row).
        self.ordering
            .create_in_tree(
                &document.parent_id,
                None,
                &document_uri,
                holon_api::BlockContent::text(document.content.clone()),
                &document.properties,
                &document.tags,
                &document.requires,
                &document.advice_suppressed,
            )
            .await
            .map_err(|e| anyhow::anyhow!("create_in_tree(document {document_uri}): {e:#}"))?;

        // Register UUID → file path alias (if Loro is available)
        if let Some(ref registrar) = self.alias_registrar {
            registrar.register_alias(&document_uri, path).await;
        }

        // Old state = this file's diff **base**, read through the `BaseStore`
        // seam (Phase 3). The base is the parsed snapshot of `last_projection`
        // (what we last projected for this file) or, on cold boot, the
        // consolidated store — so seed-default-layout blocks are treated as
        // updates, not re-creates. The base is reused across calls and only
        // re-seeded when stale, which folds the former first-run cache special
        // case into the one base mechanism.
        //
        // Freshness is keyed on the exact `last_projection` string the base was
        // parsed from (`base_source`), so the base can never desync from
        // `last_projection` regardless of which render path last wrote it.
        let base_key = BaseKey::file("org", document_uri.as_str());
        let base_fresh = self.base_source.get(&canonical).map(String::as_str) == Some(last);
        let old_blocks: HashMap<EntityUri, Block> =
            if base_fresh && self.base_store.is_base_seeded(&base_key) {
                self.base_store
                    .get_base(&base_key)
                    .values()
                    .map(|s| (s.block.id.clone(), s.block.clone()))
                    .collect()
            } else {
                // (Re)seed the base. On first run (no `last_projection`) the
                // consolidated store may already hold blocks (e.g. from
                // seed_default_layout); querying it ensures they are treated as
                // updates. Otherwise parse the last projected content.
                let seed: HashMap<EntityUri, Block> = if last.is_empty() {
                    self.block_reader
                        .get_blocks(&document_uri)
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .map(|b| (b.id.clone(), b))
                        .collect()
                } else {
                    match self
                        .format
                        .parse(path, last, &EntityUri::no_parent(), &self.root_dir)
                    {
                        Ok(result) => result
                            .blocks
                            .into_iter()
                            .map(|b| (b.id.clone(), b))
                            .collect(),
                        Err(_) => HashMap::new(),
                    }
                };
                // Org has no fractional index — order is document position — so
                // the base's `sort_key` slot is inert here (default key). The
                // org reconciler diffs Block content; ordering is applied
                // separately via `place_all` from document order (ADR 0005).
                let snapshot: HashMap<String, SnapshotBlock> = seed
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.to_string(),
                            SnapshotBlock {
                                block: v.clone(),
                                sort_key: default_sort_key(),
                            },
                        )
                    })
                    .collect();
                self.base_store.put_base(&base_key, snapshot);
                self.base_source.insert(canonical.clone(), last.to_string());
                seed
            };

        let new_parse =
            self.format
                .parse(path, &disk_content, &EntityUri::no_parent(), &self.root_dir)?;

        // Sync format-specific document-header metadata (org `#+TODO:` keywords)
        // from the parsed file onto the document block. The parser extracts these
        // from the file header, but the document entity (created via
        // DocumentManager) doesn't carry them. Without this, re-renders via
        // render_document() omit the header.
        let mut doc = document;
        if self
            .format
            .sync_document_metadata(&new_parse.document, &mut doc)
        {
            self.doc_manager.update_metadata(&doc).await?;
        }

        let new_blocks_vec = new_parse.blocks;
        let new_blocks: HashMap<EntityUri, Block> = new_blocks_vec
            .iter()
            .map(|b| (b.id.clone(), b.clone()))
            .collect();

        // Check for duplicate block IDs owned by other documents
        let new_block_ids: Vec<EntityUri> = new_blocks_vec
            .iter()
            .filter(|b| !old_blocks.contains_key(&b.id))
            .map(|b| b.id.clone())
            .collect();
        let conflicts = self
            .block_reader
            .find_foreign_blocks(&new_block_ids, &document_uri)
            .await?;
        let conflict_ids: std::collections::HashSet<EntityUri> =
            conflicts.iter().map(|(id, _)| id.clone()).collect();
        if !conflicts.is_empty() {
            info!(
                "[FileSyncController] Re-parenting {} blocks from other documents to {} \
                 (blocks exist under different doc URIs, e.g. from seed_default_layout). \
                 File: {}",
                conflicts.len(),
                document_uri,
                path.display(),
            );
        }

        // Collect all block operations into a batch
        let mut operations: Vec<(String, holon_api::StorageEntity)> = Vec::new();
        let mut has_structural_changes = false;
        // Set when the updates pass 3-way merged a concurrent file-vs-UI content
        // edit. A pure content update is not "structural", so the early-return
        // below would skip the disk write-back — but a merge produces content
        // that is on NEITHER disk nor in `last_projection`, so we must force the
        // re-render/write-back so disk converges to the merged text.
        let mut did_text_merge = false;
        let mut created_ids: Vec<String> = Vec::new();
        let mut updated_via_conflict_ids: Vec<String> = Vec::new();

        // Current store content, keyed by id — the "mine" side of the 3-way text
        // merge (the live UI/store edit). Fetched once, only when the no-store
        // conflict path is active: a merger is wired AND this session's
        // consolidator owns the store directly (SqlOnly / `Store`). In Loro
        // (`Upstream`) mode the live CRDT already merges, so we skip entirely and
        // never pay the read. Per Replication invariant 1 the merge BASE comes
        // from the BaseStore (`old_blocks`), never from this cache read — this
        // read supplies only "mine", the current store value.
        let text_merge_active = self.text_merge.is_some()
            && matches!(self.ordering.consolidator(), Consolidator::Store);
        let store_content: HashMap<EntityUri, String> = if text_merge_active {
            self.block_reader
                .get_blocks(&document_uri)
                .await
                .with_context(|| {
                    format!("read current store blocks for 3-way text merge (doc {document_uri})")
                })?
                .into_iter()
                .map(|b| (b.id.clone(), b.content))
                .collect()
        } else {
            HashMap::new()
        };

        // Creates (in document order so parents before children).
        // Blocks that already exist under a different document are re-parented
        // via "update" instead of "create" (INSERT OR IGNORE would silently skip them).
        //
        // For each new block we attach the typed positional intent
        // `after_block_id = <previous sibling in file under same parent>`,
        // tracked in `last_block_per_parent` as we walk `new_blocks_vec`
        // (which is in DFS document order). The predecessor may be an
        // existing block (already in old_blocks, already in the
        // consolidator's tree) or a freshly-created block earlier in this
        // batch — both work, because the consolidator processes Created
        // events serially, so the
        // predecessor is in the tree by the time `apply_create` resolves
        // the position.
        //
        // Without this, the inbound CDC path fell back to a sort_key
        // sibling-scan that compared the org parser's `gen_n_keys` values
        // against the consolidator's auto-assigned order keys — two
        // generation strategies that don't share a value space, so the
        // scan picked the wrong predecessor (or none) and collapsed
        // children to the front of the list. See Phase 3.7 / the Stage 2
        // cleanup devlog for the empirical confirmation.
        // Walk in document order, tracking the predecessor under each parent
        // as we go. Existing blocks anchor the position of subsequent new
        // siblings, so the cursor advances for every block — not just the
        // new ones. Records each block's predecessor (or `None` for "first
        // child") in `predecessors`; both the creates pass below and the
        // updates pass further down look it up to attach the typed
        // `after_block_id` param.
        let mut last_block_per_parent: HashMap<EntityUri, EntityUri> = HashMap::new();
        let mut predecessors: HashMap<EntityUri, Option<EntityUri>> = HashMap::new();
        for block in &new_blocks_vec {
            let parent_id = if block.parent_id == new_parse.document.id {
                &document_uri
            } else {
                &block.parent_id
            };
            let pred = last_block_per_parent.get(parent_id).cloned();
            predecessors.insert(block.id.clone(), pred);
            last_block_per_parent.insert(parent_id.clone(), block.id.clone());
        }

        // Creates pass. A block create is an INTENT to the consolidator: the
        // ordering authority's `create_in_tree` persists the block and the
        // downstream feed writes the SQL sink row — org never writes that sink
        // directly. The return value is a contract: `true` means the
        // consolidator persisted the create (the downstream flush will write
        // the sink), `false` means the SQL store is itself the consolidator
        // (degraded, no separate downstream) so org routes the create through
        // the command bus as it does updates/deletes. No storage-mode branch
        // here — only the contract. Exact positioning is the place loop's job,
        // so `after_id` is `None`.
        let mut consolidator_creates: usize = 0;
        // Ids of consolidator-persisted creates (Loro mode): their sink rows are
        // written by the downstream flush at site B, not by the `operations`
        // batch — so they are excluded from the site-A feed catch-up set below.
        let mut consolidator_create_ids: Vec<String> = Vec::new();
        for block in &new_blocks_vec {
            // Upgrade-path re-seed: a PRE-EXISTING row (SQL populated by a
            // pre-Loro session) whose block the authoritative tree never
            // adopted. `new_blocks_vec` is DFS document order, so parents
            // re-seed before their children — the same parent-first contract
            // `create_in_tree` requires of genuine creates. Document blocks
            // are owned by the doc manager and excluded.
            let needs_reseed = old_blocks.contains_key(&block.id)
                && block.id != new_parse.document.id
                && matches!(self.ordering.consolidator(), Consolidator::Upstream)
                && self
                    .ordering
                    .in_tree(&block.id)
                    .await
                    .map_err(|e| anyhow::anyhow!("in_tree({}): {e:#}", block.id))?
                    == Some(false);
            if needs_reseed {
                let parent_uri = if block.parent_id == new_parse.document.id {
                    document_uri.clone()
                } else {
                    block.parent_id.clone()
                };
                let persisted = self
                    .ordering
                    .create_in_tree(
                        &parent_uri,
                        None,
                        &block.id,
                        block.to_block_content(),
                        &block.properties,
                        &block.tags,
                        &block.requires,
                        &block.advice_suppressed,
                    )
                    .await
                    .map_err(|e| anyhow::anyhow!("re-seed create_in_tree({}): {e:#}", block.id))?;
                if persisted {
                    has_structural_changes = true;
                    created_ids.push(block.id.to_string());
                    consolidator_creates += 1;
                    consolidator_create_ids.push(block.id.to_string());
                    tracing::info!(
                        block_id = %block.id,
                        parent = %parent_uri,
                        "re-seeded pre-Loro vault block into the Loro tree"
                    );
                } else {
                    // The cell registry declined (e.g. its own unseeded-vault
                    // guard: parent still missing). Order stays SQL-owned for
                    // this block — ALLOW(fallback): disclosed via warn, the
                    // place loop's pre-existing-block guard then skips it.
                    tracing::warn!(
                        block_id = %block.id,
                        parent = %parent_uri,
                        "re-seed declined by the tree backing — order stays SQL-owned"
                    );
                }
                continue;
            }
            if !old_blocks.contains_key(&block.id) {
                let parent_id = if block.parent_id == new_parse.document.id {
                    &document_uri
                } else {
                    &block.parent_id
                };
                let mut params = self
                    .format
                    .build_block_params(block, parent_id, &document_uri);
                if let Some(Some(prev)) = predecessors.get(&block.id) {
                    params.insert(
                        POSITION_AFTER_BLOCK_ID_PARAM.into(),
                        Value::String(prev.to_string()),
                    );
                }
                let op = if conflict_ids.contains(&block.id) {
                    "update"
                } else {
                    "create"
                };
                if op == "create" {
                    has_structural_changes = true;
                    created_ids.push(block.id.to_string());
                    let parent_uri = if block.parent_id == new_parse.document.id {
                        document_uri.clone()
                    } else {
                        block.parent_id.clone()
                    };
                    let block_uri = block.id.clone();
                    // Full typed content (`to_block_content` preserves source vs
                    // text + language) so a `#+BEGIN_SRC` block isn't degraded
                    // to text by the downstream projection.
                    let persisted = self
                        .ordering
                        .create_in_tree(
                            &parent_uri,
                            None,
                            &block_uri,
                            block.to_block_content(),
                            &block.properties,
                            &block.tags,
                            &block.requires,
                            &block.advice_suppressed,
                        )
                        .await
                        .map_err(|e| anyhow::anyhow!("create_in_tree({}): {e:#}", block.id))?;
                    if persisted {
                        consolidator_creates += 1;
                        consolidator_create_ids.push(block.id.to_string());
                    } else {
                        operations.push((op.to_string(), params));
                    }
                } else {
                    updated_via_conflict_ids.push(block.id.to_string());
                    operations.push((op.to_string(), params));
                }
            }
        }
        tracing::debug!(
            "[ORGSYNC_DIFF] {} old={} new={} creates={} conflict_updates={} creates_ids={:?}",
            path.display(),
            old_blocks.len(),
            new_blocks_vec.len(),
            created_ids.len(),
            updated_via_conflict_ids.len(),
            created_ids,
        );

        // Updates pass. Existing blocks may have moved within their parent's
        // children list (e.g. when a 2nd BulkExternalAdd grows the sibling
        // set, every sibling's `gen_n_keys`-assigned sort_key gets
        // re-canonicalised). Inject the typed `after_block_id` here too so
        // `apply_update_with_backend` can `tree.mov_after` against the
        // file's predecessor instead of relying on the sort_key sibling-scan
        // — same gen-strategy-mismatch concern as creates.
        //
        // Iterate `new_blocks_vec` (document order), NOT `new_blocks`
        // (HashMap, non-deterministic). Update events get applied
        // sequentially by the consolidator, and each tree move
        // depends on the *current* tree state at apply time. If updates
        // arrived in HashMap order, a later sibling could be moved after its
        // predecessor *before* the predecessor itself had been moved,
        // scrambling the children list.
        for new_block in &new_blocks_vec {
            let id = &new_block.id;
            if let Some(old_block) = old_blocks.get(id) {
                // No-store conflict path: when the disk content for this block
                // diverged from the base AND the store holds a competing edit,
                // 3-way merge the two instead of clobbering with the disk value
                // (whole-value LWW). `text_merge_active` already restricts this
                // to SqlOnly (`Store`) mode with a wired merger; the base is the
                // BaseStore snapshot (`old_block`), "theirs" the disk parse,
                // "mine" the current store content. Text CONTENT only — edge and
                // structural fields still take the disk value.
                let mut merged_block: Option<Block> = None;
                if text_merge_active && new_block.content != old_block.content {
                    if let Some(mine) = store_content.get(id) {
                        let merger = self
                            .text_merge
                            .as_ref()
                            .expect("text_merge_active implies a wired merger");
                        let (resolved, merged) = three_way_text_content(
                            &old_block.content,
                            &new_block.content,
                            mine,
                            merger.as_ref(),
                        )?;
                        if merged {
                            tracing::info!(
                                block = %id,
                                doc = %document_uri,
                                "concurrent file-vs-UI edit 3-way text-merged \
                                 (base/disk/current) in Direct (SqlOnly) mode"
                            );
                            let mut b = new_block.clone();
                            b.content = resolved;
                            merged_block = Some(b);
                            did_text_merge = true;
                        }
                    }
                }
                let effective = merged_block.as_ref().unwrap_or(new_block);
                if self.format.content_differs(old_block, effective) {
                    let parent_id = if effective.parent_id == new_parse.document.id {
                        &document_uri
                    } else {
                        &effective.parent_id
                    };
                    let mut params =
                        self.format
                            .build_block_params(effective, parent_id, &document_uri);
                    if let Some(Some(prev)) = predecessors.get(id) {
                        params.insert(
                            POSITION_AFTER_BLOCK_ID_PARAM.into(),
                            Value::String(prev.to_string()),
                        );
                    }
                    // Phase 2: drop edge fields from params when unchanged, so
                    // SqlOperationProvider's edge_field_replace_sql (DELETE +
                    // re-INSERT into block_requires/block_tags) is not invoked.
                    // Junction values are order-undefined on read, so compare as
                    // sets. Idempotent re-ingests of an unchanged vault stop
                    // churning ~2,400 statements per startup.
                    strip_unchanged_edge_fields(&mut params, old_block, effective);
                    operations.push(("update".to_string(), params));
                }
            }
        }

        // Deletes
        for id in old_blocks.keys() {
            if !new_blocks.contains_key(id) {
                has_structural_changes = true;
                let mut params: holon_api::StorageEntity = HashMap::new();
                params.insert("id".into(), Value::String(id.to_string()));
                // Phase 3: pin the document URI so the provider's prepare_delete
                // skips the WITH RECURSIVE Page-walk (find_document_uri).
                params.insert(
                    ROUTING_DOC_URI_KEY.into(),
                    Value::String(document_uri.to_string()),
                );
                operations.push(("delete".to_string(), params));
            }
        }

        // Apply each operation through `BlockOrdering` — the single org→block
        // write seam. There is no command bus: `update_in_tree` routes Loro-mode
        // writes field-by-field into Loro (the outbound projector emits the SQL
        // row) and SqlOnly writes straight to SQL; `delete_in_tree` deletes from
        // Loro (projector emits the SQL DELETE) or from SQL directly. `"create"`
        // ops only occur in SqlOnly (Loro creates persisted via `create_in_tree`
        // returning true and were counted in `consolidator_creates`); they share
        // the `update_in_tree` upsert path, which picks the right CDC op kind.
        //
        // `consolidator_creates` blocks were sent to the consolidator via
        // `create_in_tree` — their sink rows are written by the downstream flush
        // below, not here. Exclude them from the post-apply cache-catch-up
        // expectation; the full "every block present" check happens after the
        // flush.
        let expected_block_count = new_blocks.len() - consolidator_creates;
        tracing::debug!(
            target: "holon_latency",
            stage = "boot_parse",
            ms = t_ingest.elapsed().as_millis() as u64,
            blocks = new_blocks.len() as u64,
            path = %path.display(),
            "holon_latency",
        );
        tracing::debug!(
            "[ORGSYNC_OPS] {} ops={:?}",
            path.display(),
            operations
                .iter()
                .map(|(op, p)| format!(
                    "{op}:{}",
                    p.get("id").and_then(|v| v.as_string()).unwrap_or("?")
                ))
                .collect::<Vec<_>>(),
        );
        let t_write = std::time::Instant::now();
        if !operations.is_empty() {
            for (op, params) in operations {
                match op.as_str() {
                    "create" | "update" => {
                        self.ordering.update_in_tree(params).await.map_err(|e| {
                            anyhow::anyhow!("update_in_tree for {}: {e:#}", path.display())
                        })?;
                    }
                    "delete" => {
                        self.ordering.delete_in_tree(params).await.map_err(|e| {
                            anyhow::anyhow!("delete_in_tree for {}: {e:#}", path.display())
                        })?;
                    }
                    other => {
                        anyhow::bail!(
                            "on_file_changed: unknown block op {other:?} for {}",
                            path.display()
                        );
                    }
                }
            }
            tracing::debug!(
                target: "holon_latency",
                stage = "boot_write",
                ms = t_write.elapsed().as_millis() as u64,
                blocks = expected_block_count as u64,
                path = %path.display(),
                "holon_latency",
            );

            // Phase 5 cutover (site A): wait on the positional `LiveData<Block>`
            // catch-up — every block this scan expects in the cache is visible
            // in the convergent feed — instead of the `event_acks` watermark
            // (`wait_for_cache_caught_up`), replacing the original push-based-but-
            // timestamp-proxy wait with a push-based positional one. The expected
            // set is every parsed block EXCEPT the consolidator-persisted creates
            // (their sink rows are written by the downstream flush at site B). The
            // `block` matview feed is strictly downstream of `block_raw` (what
            // `get_blocks` reads), so feed-present ⟹ block_raw-present; the count
            // check below is then guaranteed and kept as the ground-truth gate.
            let expected_present_ids: Vec<String> = new_blocks_vec
                .iter()
                .map(|b| b.id.to_string())
                .filter(|id| !consolidator_create_ids.contains(id))
                .collect();
            // Site A feed barrier. During the initial scan this buffers the ids
            // for the single end-of-scan convergence wait and returns at once;
            // in steady state it waits per file, unchanged. The `get_blocks`
            // count-check below stays UNCONDITIONAL — `block_raw` is written
            // synchronously by the ops above, so it is the real intra-file
            // write-success gate independent of the (async, sidebar-facing) feed.
            let caught_up = self.feed_barrier(&expected_present_ids, "updates").await;
            let cached_blocks = self.block_reader.get_blocks(&document_uri).await?;
            if cached_blocks.len() < expected_block_count {
                anyhow::bail!(
                    "[on_file_changed] block feed did not catch up within 2s for {} \
                     (expected {} blocks, cache has {}, feed_caught_up={})",
                    path.display(),
                    expected_block_count,
                    cached_blocks.len(),
                    caught_up
                );
            }
        }

        // (Block creates were already sent to the consolidator via
        // `create_in_tree` in the creates pass above, so they're visible to
        // `children()` before the place loop runs; the downstream flush below
        // writes their sink rows.)

        // Disk-order replay: move any block that is not already in the position
        // recorded in the parsed org file. One `children()` call per distinct
        // parent (cached in `live_children`), O(N) total reads.
        //
        // Before reading children we wait for every newly-created block to be
        // visible to the ordering layer. `execute_batch_with_origin` above
        // published `EventOrigin::Org` create events whose consolidator-side
        // application is asynchronous (the consolidator's inbound
        // consumer processes them off the EventBus). The CDC-cache wait at
        // ~line 528 only gates on the sink projection; if we proceed straight
        // to `ordering.place` we may reposition a block whose tree node
        // hasn't been created yet, surfacing as `Block not found: <id>` —
        // the block then lands at the consolidator's default position and
        // the renderer's children-of-doc query never finds it.
        // Polling `ordering.children(parent)` reads through the same path
        // `ordering.place` will use, so once a created id appears there the
        // subsequent `place` is guaranteed to find it.
        let t_place = std::time::Instant::now();
        {
            let mut live_children: HashMap<EntityUri, Vec<String>> = HashMap::new();
            let mut expected_per_parent: HashMap<EntityUri, HashSet<String>> = HashMap::new();
            // `BlockOrdering::children` filters `b.parent_id.as_str() == parent_id`,
            // and `EntityUri::as_str()` returns the FULL URI (`"block:ref-doc-0"`).
            // Keys here are full URIs so the compare matches.
            for new_block in &new_blocks_vec {
                if !created_ids.contains(&new_block.id.to_string()) {
                    continue;
                }
                let parent_key = if new_block.parent_id == new_parse.document.id {
                    document_uri.clone()
                } else {
                    new_block.parent_id.clone()
                };
                // Compare against the full URI form returned by
                // `BlockOrdering::children` (kids are `b.id.as_str()`).
                expected_per_parent
                    .entry(parent_key)
                    .or_default()
                    .insert(new_block.id.as_str().to_string());
            }
            let propagate_deadline =
                tokio::time::Instant::now() + tokio::time::Duration::from_millis(2000);
            for (parent_key, expected_ids) in &expected_per_parent {
                loop {
                    let kids: Vec<String> = self
                        .ordering
                        .children(parent_key)
                        .await
                        .map_err(|e| anyhow::anyhow!("ordering.children failed: {e}"))?
                        .into_iter()
                        .map(|u| u.as_str().to_string())
                        .collect();
                    let present: HashSet<&str> = kids.iter().map(String::as_str).collect();
                    if expected_ids.iter().all(|id| present.contains(id.as_str())) {
                        live_children.insert(parent_key.clone(), kids);
                        break;
                    }
                    if tokio::time::Instant::now() >= propagate_deadline {
                        let missing: Vec<&String> = expected_ids
                            .iter()
                            .filter(|id| !present.contains(id.as_str()))
                            .collect();
                        anyhow::bail!(
                            "[on_file_changed] new blocks did not appear in ordering for \
                             parent {parent_key} within 2s: missing {missing:?}; \
                             present children: {kids:?}"
                        );
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                }
            }
            // Backfill children lists for parents that only contained
            // pre-existing blocks (no creates) — the wait loop above skipped
            // them entirely.
            for new_block in &new_blocks_vec {
                let parent_key = if new_block.parent_id == new_parse.document.id {
                    document_uri.clone()
                } else {
                    new_block.parent_id.clone()
                };
                #[allow(clippy::map_entry)]
                // async fetch between check + insert, entry API doesn't fit
                if !live_children.contains_key(&parent_key) {
                    let kids: Vec<String> = self
                        .ordering
                        .children(&parent_key)
                        .await
                        .map_err(|e| anyhow::anyhow!("ordering.children failed: {e}"))?
                        .into_iter()
                        .map(|u| u.as_str().to_string())
                        .collect();
                    live_children.insert(parent_key, kids);
                }
            }

            if matches!(self.ordering.consolidator(), Consolidator::Upstream) {
                // Loro owns order: place each text block after its file
                // predecessor. `update_block_position` reads the LIVE tree and
                // no-ops cheaply when already positioned, so doc-order placement
                // is order-correct regardless of the initial layout.
                for new_block in &new_blocks_vec {
                    // Source / image children are grouped ahead of text by
                    // `OrgRenderer::render_entity_tree` regardless of sort_key
                    // (see assertions.rs `render_group`). They also don't land
                    // in the Loro tree — synthetic ids like `<parent>::render::0`
                    // exist only in SQL — so `place()` would surface
                    // `Block not found` through `update_block_position`.
                    if !matches!(new_block.content_type, holon_api::ContentType::Text) {
                        continue;
                    }
                    let parent = if new_block.parent_id == new_parse.document.id {
                        &document_uri
                    } else {
                        &new_block.parent_id
                    };
                    // Full-URI form throughout: `BlockOrdering::children` /
                    // `prev_sibling` return `b.id.as_str()` = `"block:foo"`, and
                    // `place()`'s internal comparisons (sql_block_operations.rs:182)
                    // also use `as_str()`. Mixing bare ids here silently skips
                    // every block.
                    let want_after: Option<&EntityUri> =
                        predecessors.get(&new_block.id).and_then(|p| p.as_ref());

                    let siblings = live_children.get(parent).map(Vec::as_slice).unwrap_or(&[]);
                    // Presence sanity only — the wait-loop guarantees newly-created
                    // blocks are in `live_children` and pre-existing ones were
                    // backfilled.
                    if !siblings.iter().any(|s| s == new_block.id.as_str()) {
                        if created_ids.contains(&new_block.id.to_string()) {
                            anyhow::bail!(
                                "[on_file_changed] block {} not found in live_children under {}: {:?}",
                                new_block.id.as_str(),
                                parent.as_str(),
                                siblings
                            );
                        }
                        // Unseeded-vault guard (same family as `create_entity`
                        // / `write_field` / `live_children`): a PRE-EXISTING
                        // block (SQL row from a pre-Loro session) with no Loro
                        // tree node. Loro cannot place it; its order stays
                        // SQL-owned until a seed/repair pass exists —
                        // ALLOW(fallback): disclosed via warn; bailing here
                        // aborted the whole initial scan and the app never
                        // started on `[loro] enabled = true` over an upgraded
                        // vault.
                        tracing::warn!(
                            block_id = new_block.id.as_str(),
                            parent = parent.as_str(),
                            "[on_file_changed] pre-existing block missing from the Loro tree \
                             (unseeded vault) — skipping Loro placement, SQL owns its order"
                        );
                        continue;
                    }

                    self.ordering
                        .place(&new_block.id, parent, want_after)
                        .await
                        .map_err(|e| anyhow::anyhow!("ordering.place failed: {e}"))?;
                }
            } else {
                // No Loro: the SQL store is the sole order owner, and the file's
                // line order is the authoritative TOTAL order. Incremental
                // `place` can't converge a full reorder (it inserts one block at
                // a time relative to a mutating store), which is the
                // `inv-live-children-match-ref` divergence. Instead mint one
                // fresh, gap-free key sequence per parent over its text children
                // in document order via `place_all` — total by construction, so
                // `ORDER BY sort_key` reproduces the file exactly
                // (Replication.md §5/§11: one owner, projected verbatim).
                // Source/synthetic children are render-grouped ahead of text
                // regardless of sort_key and are not re-keyed here.
                let mut per_parent: Vec<(EntityUri, Vec<EntityUri>)> = Vec::new();
                let mut parent_slot: HashMap<EntityUri, usize> = HashMap::new();
                for new_block in &new_blocks_vec {
                    if !matches!(new_block.content_type, holon_api::ContentType::Text) {
                        continue;
                    }
                    let parent_key = if new_block.parent_id == new_parse.document.id {
                        document_uri.clone()
                    } else {
                        new_block.parent_id.clone()
                    };
                    let slot = *parent_slot.entry(parent_key.clone()).or_insert_with(|| {
                        per_parent.push((parent_key.clone(), Vec::new()));
                        per_parent.len() - 1
                    });
                    per_parent[slot].1.push(new_block.id.clone());
                }
                for (parent_key, ordered_ids) in &per_parent {
                    self.ordering
                        .place_all(parent_key, ordered_ids)
                        .await
                        .map_err(|e| anyhow::anyhow!("ordering.place_all failed: {e}"))?;
                }
            }
        }
        tracing::debug!(
            target: "holon_latency",
            stage = "boot_place_wait",
            ms = t_place.elapsed().as_millis() as u64,
            path = %path.display(),
            "holon_latency",
        );

        // Downstream convergent feed: publish the consolidator's accumulated
        // changes from this scan (creates + placements) to the SQL sink. This
        // is the single sink-writer for consolidator-persisted creates — it
        // writes their rows with the authoritative order key + properties,
        // closing the projection-totality gap (a created-but-unmoved block
        // still gets its real order key, not the struct default). Absent in
        // degraded mode, where the command-bus batch + `place` already wrote
        // the rows and their order keys directly.
        match &self.downstream {
            Some(downstream) => {
                downstream
                    .flush()
                    .await
                    .map_err(|e| anyhow::anyhow!("downstream projection flush: {e}"))?;
            }
            None => {
                // Fail loud: a create the consolidator persisted (create_in_tree
                // returned true) has no command-bus row, so without a downstream
                // feed its sink row would never be written. That's a wiring bug,
                // not a degraded-but-fine state.
                if consolidator_creates > 0 {
                    anyhow::bail!(
                        "[on_file_changed] {consolidator_creates} block create(s) were \
                         persisted by a separate consolidator (create_in_tree returned \
                         true) but no downstream projection is wired — their sink rows \
                         would never be written. DI wiring bug."
                    );
                }
            }
        }
        if !created_ids.is_empty() {
            // Phase 5 cutover (site B): wait on the positional `LiveData<Block>`
            // catch-up — every just-created id visible in the convergent feed —
            // instead of the `event_acks` watermark (`wait_for_cache_caught_up`).
            // The feed (the `block` matview CDC stream) is downstream of the same
            // `block_raw` the renderer reads, so its catch-up is a sound, push-
            // based, positional proxy. Validated by the Step-0 shadow: 33/33 PBT
            // cases caught up at 0 ms, 0 misses. Fail loud on timeout — a stuck
            // feed is a real bug, not a state to silently continue past.
            // During the initial scan (site C) this buffers `created_ids` for the
            // single end-of-scan convergence wait instead of blocking per file;
            // the fail-loud check then fires once in `finish_initial_scan`.
            let feed_caught_up = self.feed_barrier(&created_ids, "creates").await;
            if !feed_caught_up {
                anyhow::bail!(
                    "[on_file_changed] LiveData<Block> feed did not contain all {} \
                     created id(s) within 2s for {} — projection/CDC stalled",
                    created_ids.len(),
                    path.display()
                );
            }
        }

        // Ingest image files from disk into the image data provider (if any).
        // At this point blocks are in the store and image files are on disk.
        self.ingest_images(&document_uri).await?;

        // For UPDATE-only ingestion (no creates/deletes), the disk content already
        // reflects the authoritative state — we just parsed it and persisted the
        // diff to SQL. Re-rendering from the CDC cache here would be racy: count-
        // based waiting can't detect property updates, so the cache may still
        // return the pre-update row and we'd overwrite the file with stale data,
        // losing the properties we just ingested. Skip the round-trip entirely
        // and record the disk content as the new projection.
        //
        // EXCEPTION: when the file lacks a `#+ID:` directive, force the round-trip
        // so the renderer can persist `#+ID: <uuid>` to disk. This makes the
        // document's identity rename-safe and lets future loads short-circuit the
        // name-chain lookup.
        let needs_id_writeback = bare_id_in_file.is_none();
        // `did_text_merge` forces the round-trip: a merge produced content that
        // is on NEITHER disk nor in `last_projection`, so recording disk as the
        // projection and returning would strand the merged text (disk would
        // never converge). The re-render below reads the merged store content
        // and writes it back to disk.
        if !has_structural_changes && !needs_id_writeback && !did_text_merge {
            self.last_projection
                .insert(canonical.clone(), disk_content.to_string());
            self.persist_disk_hash_for(&canonical, rel_path, &disk_hash)
                .await;
            return Ok(());
        }

        // Structural changes occurred — re-project from cache so the file reflects
        // any merges (e.g. conflict re-parenting, seed layout integration).
        let rendered = self.render_file_by_doc_id(&document_uri, path).await?;
        assert!(
            new_blocks.is_empty() || !rendered.trim().is_empty(),
            "[FileSyncController] BUG: Just created/updated {} blocks for doc_id={} \
             but render_file_by_doc_id returned empty for {}. \
             This would wipe the file!",
            new_blocks.len(),
            document_uri,
            path.display(),
        );

        if rendered != disk_content {
            // TOCTOU guard: re-read the disk NOW. If it changed since we parsed
            // it, a concurrent external write has landed new content — writing
            // `rendered` (derived from a stale CDC cache) would wipe that
            // external write off disk. Defer to the next on_file_changed
            // invocation (FSEvents and the poll backstop will both fire for the
            // new disk content), and stamp `last_projection` with the version
            // we reconciled so the next diff sees the true external delta.
            match self.fs.read_to_string(path).await {
                Ok(now) if now != disk_content => {
                    tracing::debug!(
                        "[ORGSYNC_TOCTOU] {} disk changed during processing \
                         (parsed_len={} disk_now_len={}); skipping write-back, \
                         stamping last_projection with parsed content so next \
                         diff picks up the external delta.",
                        path.display(),
                        disk_content.len(),
                        now.len(),
                    );
                    self.last_projection.insert(canonical.clone(), disk_content);
                    return Ok(());
                }
                Ok(_) => {
                    if let Some(parent) = path.parent() {
                        self.fs.create_dir_all(parent).await?;
                    }
                    self.fs.write(path, rendered.as_bytes()).await?;
                    self.run_post_write_hook(path);
                    info!(
                        "[FileSyncController] Wrote merged content to {}",
                        path.display()
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // File deleted since we parsed it. Nothing to do.
                    return Ok(());
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!(
                            "[FileSyncController] TOCTOU re-read failed for {}",
                            path.display()
                        )
                    });
                }
            }
        }

        // Phase 1: stamp file.content_hash for the bytes that are NOW on
        // disk (= `rendered` if we wrote it, else still == disk_content;
        // in both cases `rendered` is the canonical projection). Updates
        // in-memory map and persists to SQL so next boot's fast-path engages.
        let final_hash = self.projection_hash(&rendered);
        self.persist_disk_hash_for(&canonical, rel_path, &final_hash)
            .await;

        // Update last_projection
        self.last_projection.insert(canonical.clone(), rendered);

        Ok(())
    }

    /// Update `last_projection_hash` in memory and persist to
    /// `file.content_hash` via the BlockReader's raw-SQL write-back. Best-
    /// effort: a failure to persist (e.g. file row not yet created by
    /// `OrgmodeSyncProvider`) does not abort the ingest — we've already
    /// committed the block ops and don't want to bail the controller. The
    /// next sync will create the row and the following boot will write
    /// the hash successfully. Logged at warn so the case is observable.
    async fn persist_disk_hash_for(
        &mut self,
        canonical: &CanonicalPath,
        rel_path: &Path,
        hash: &str,
    ) {
        self.last_projection_hash
            .insert(canonical.clone(), hash.to_string());
        let rel = rel_path.to_string_lossy();
        let file_uri = EntityUri::file(&rel);
        if let Err(e) = self.block_reader.persist_file_hash(&file_uri, hash).await {
            warn!(
                "[FileSyncController] persist_file_hash failed for {} ({}): {} \
                 (in-memory hash updated; next boot will re-ingest)",
                file_uri,
                canonical.as_path_buf().display(),
                e
            );
        }
    }

    /// Handle a block change notification (from EventBus or Loro).
    ///
    /// Re-renders the affected file and writes if content changed.
    /// Returns `true` if a matching document file was found and re-rendered,
    /// `false` if the doc_id didn't map to any known file.
    #[tracing::instrument(skip(self, delta), name = "org.on_block_changed", fields(doc_id = %doc_id))]
    pub async fn on_block_changed(
        &mut self,
        doc_id: &EntityUri,
        delta: &BlockDelta,
    ) -> Result<bool> {
        let path = match self.doc_id_to_path(doc_id).await {
            Some(p) => p,
            None => return Ok(false),
        };
        let canonical = CanonicalPath::new(&path);

        // If disk content differs from last_projection, there's a pending external
        // change that the file watcher hasn't delivered yet. Ingest it first so
        // the re-render below includes both the block event and the external edit.
        //
        // Only treat this as a pending external change when we have a baseline
        // (`last_projection` already holds the file). Without a baseline,
        // `last == ""` would always differ from any non-empty disk content and
        // we'd incorrectly re-ingest the on-disk file — which can revert the
        // user's just-issued UPDATE if the file watcher hasn't yet delivered the
        // initial WriteOrgFile event. The watcher will catch up on its own.
        let disk_content = read_disk_or_empty(&self.fs, &path).await?;
        let last = self
            .last_projection
            .get(&canonical)
            .map(|s| s.as_str())
            .unwrap_or("");
        if self.last_projection.contains_key(&canonical) && disk_content != last {
            info!(
                "[FileSyncController] Processing pending external change for {} before re-render",
                path.display()
            );
            self.on_file_changed(&path).await?;
        }

        let rendered = self.render_with_cache(doc_id, &path, delta).await?;

        let current_last = self
            .last_projection
            .get(&canonical)
            .map(|s| s.as_str())
            .unwrap_or("");

        if rendered == current_last {
            return Ok(true);
        }

        // TOCTOU guard: disk may have changed again since we read it above
        // (concurrent external write). Writing `rendered` here — derived
        // from the CDC cache which may lag behind the new disk content —
        // would wipe the external write. Re-read and bail if changed.
        let disk_at_write = read_disk_or_empty(&self.fs, &path).await?;
        if disk_at_write != disk_content {
            tracing::debug!(
                "[ORGSYNC_TOCTOU on_block_changed] {} disk changed during processing \
                 (initial_len={} disk_now_len={}); skipping write-back.",
                path.display(),
                disk_content.len(),
                disk_at_write.len(),
            );
            return Ok(true);
        }

        if self.is_quarantined(&path) {
            return Ok(true);
        }
        if let Some(parent) = path.parent() {
            self.fs.create_dir_all(parent).await?;
        }
        self.fs.write(&path, rendered.as_bytes()).await?;
        self.run_post_write_hook(&path);
        // H2 image-gate: `materialize_images` re-reads the whole doc (a 2nd
        // recursive-CTE `get_blocks`). Content-only keystrokes never add images,
        // so only pay it when THIS delta upserts an image block. Image edits are
        // rare and can afford the full read.
        if let BlockDelta::Upsert(b) = delta {
            if b.is_image_block() {
                self.materialize_images(doc_id).await?;
            }
        }
        self.last_projection.insert(canonical, rendered);

        info!(
            "[FileSyncController] Wrote block changes to {}",
            path.display()
        );

        Ok(true)
    }

    /// Render `doc_id`'s file, serving the block list from the per-doc
    /// incremental cache (`doc_blocks`) and updating that cache from `delta`.
    ///
    /// Hot path (content-only upsert of a block already cached with unchanged
    /// structure): refresh just that block via an authoritative `block_raw`
    /// point read (`get_block_authoritative`, O(1), no recursive CTE) and
    /// replace it in place, preserving sibling order. Everything else — cold
    /// doc, `Remove`, an id not yet cached (structural insert), a `parent_id`
    /// move, or any `tags` change (H4: a `Page` toggle re-partitions the doc's
    /// subtree) — reseeds the whole doc via `get_blocks` (authoritative,
    /// `sort_key, id`-ordered). Structural intent is decided from the
    /// AUTHORITATIVE row, not the (matview-lagged) feed delta, so a structural
    /// change the delta didn't yet reflect still reseeds.
    async fn render_with_cache(
        &mut self,
        doc_id: &EntityUri,
        path: &Path,
        delta: &BlockDelta,
    ) -> Result<String> {
        let cheap_incremental_candidate = match delta {
            BlockDelta::Upsert(b) => self
                .doc_blocks
                .get(doc_id)
                .and_then(|c| c.get(&b.id))
                .is_some_and(|cached| cached.parent_id == b.parent_id && cached.tags == b.tags),
            BlockDelta::Remove(_) => false,
        };

        if cheap_incremental_candidate {
            let BlockDelta::Upsert(b) = delta else {
                unreachable!("cheap_incremental_candidate implies Upsert")
            };
            if let Some(auth) = self.block_reader.get_block_authoritative(&b.id).await? {
                let cached = self
                    .doc_blocks
                    .get(doc_id)
                    .and_then(|c| c.get(&b.id))
                    .expect("warm + present by cheap_incremental_candidate");
                if auth.parent_id == cached.parent_id && auth.tags == cached.tags {
                    // Content-only: `IndexMap::insert` on an existing key keeps
                    // its position, so sibling order is unchanged.
                    self.doc_blocks
                        .get_mut(doc_id)
                        .expect("warm by cheap_incremental_candidate")
                        .insert(auth.id.clone(), auth);
                    return self.render_cached_doc(doc_id, path).await;
                }
            }
        }

        self.reseed_doc_blocks(doc_id).await?;
        self.render_cached_doc(doc_id, path).await
    }

    /// Reseed the per-doc block cache from the authoritative doc-scoped read
    /// (`get_blocks` over `block_raw`, ordered `sort_key, id`). The resulting
    /// `IndexMap` preserves that order for the renderer.
    async fn reseed_doc_blocks(&mut self, doc_id: &EntityUri) -> Result<()> {
        let blocks = self.block_reader.get_blocks(doc_id).await?;
        let map: IndexMap<EntityUri, Block> =
            blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        self.doc_blocks.insert(doc_id.clone(), map);
        Ok(())
    }

    /// Render `doc_id` from its (already-seeded) cache values, in cache order.
    async fn render_cached_doc(&self, doc_id: &EntityUri, path: &Path) -> Result<String> {
        let blocks: Vec<Block> = self
            .doc_blocks
            .get(doc_id)
            .expect("render_cached_doc requires a seeded doc cache")
            .values()
            .cloned()
            .collect();
        self.render_doc_blocks(doc_id, path, &blocks).await
    }

    /// Poll all tracked files for pending external changes that the file
    /// watcher may have missed (FSEvents on macOS can coalesce or drop
    /// events under load). For each file whose disk content differs from
    /// `last_projection`, call `on_file_changed` to ingest the edit.
    ///
    /// Called from a periodic timer in the DI sync loop as a backstop for
    /// notify-driven delivery. Returns the number of files that were
    /// ingested (0 if everything was already in sync).
    #[tracing::instrument(skip(self), name = "org.poll_external_changes")]
    pub async fn poll_external_changes(&mut self) -> Result<usize> {
        let mut ingested = self.poll_tracked_files().await?;
        ingested += self.poll_new_files().await?;
        Ok(ingested)
    }

    /// Phase A: re-check every path we already track for modifications or
    /// deletions. Echo-suppressed by `last_projection`; further short-circuited
    /// by a `(mtime, size)` signature so unchanged files don't cost a read.
    #[tracing::instrument(skip(self), name = "org.poll_tracked_files")]
    pub async fn poll_tracked_files(&mut self) -> Result<usize> {
        let mut ingested = 0;
        let keys: Vec<(CanonicalPath, PathBuf)> = self
            .last_projection
            .keys()
            .map(|k| (k.clone(), (**k).to_path_buf()))
            .collect();

        for (canonical, path) in keys {
            // Cheap dirty check: stat() the file and compare (mtime, size)
            // against the cached signature. Avoids the per-tick full-file
            // read_to_string for every tracked org file (~38 files at 10Hz
            // dominated idle CPU before this).
            let meta = match self.fs.metadata(&path).await {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Backstop for a deletion the event watcher missed: a
                    // tracked file vanished — cascade-delete its document
                    // (also drops the path from `last_projection`, so the
                    // next poll no longer visits it).
                    self.on_file_deleted(&path, &canonical).await?;
                    ingested += 1;
                    continue;
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("[poll_external_changes] Cannot stat {}", path.display())
                    });
                }
            };
            let sig = (meta.modified, meta.len);
            if self.disk_signatures.get(&canonical) == Some(&sig) {
                continue;
            }

            let disk_content = match self.fs.read_to_string(&path).await {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Deleted between the stat above and this read (TOCTOU) —
                    // same external-deletion handling as the stat arm.
                    self.on_file_deleted(&path, &canonical).await?;
                    ingested += 1;
                    continue;
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("[poll_external_changes] Cannot read {}", path.display())
                    });
                }
            };
            // Stamp signature *before* the diff so the next poll skips this
            // path even if the content matches last_projection (echo) — only
            // a fresh mtime/size change re-enters the read path.
            self.disk_signatures.insert(canonical.clone(), sig);

            let last = self
                .last_projection
                .get(&canonical)
                .map(|s| s.as_str())
                .unwrap_or("");
            if disk_content != last {
                info!(
                    "[FileSyncController] poll_external_changes: ingesting {} (disk != last_projection)",
                    path.display()
                );
                self.on_file_changed(&path).await?;
                ingested += 1;
            }
        }

        Ok(ingested)
    }

    /// Phase B: walk the tree and discover NEW files (paths not yet in
    /// `last_projection`). Backstops `notify`'s recursive watcher during its
    /// unarmed window on macOS (`notify::watch(dir, Recursive)` can take 9+s
    /// to register, leaving files created during that window invisible).
    ///
    /// Each call rebuilds `ignore::WalkBuilder` (gitignore regex DFAs), which
    /// is non-trivial — call sites should tick this much less often than the
    /// cheap `poll_tracked_files` path.
    #[tracing::instrument(skip(self), name = "org.poll_new_files")]
    pub async fn poll_new_files(&mut self) -> Result<usize> {
        let mut ingested = 0;
        let root_dir = self.root_dir.clone();
        let mut scanned =
            self.fs.scan_directory(&root_dir).await.with_context(|| {
                format!("[poll_new_files] scan of {} failed", root_dir.display())
            })?;
        // Keep only files this controller's format adapter handles, so a vault
        // hosting more than one format doesn't ingest foreign extensions.
        let exts = self.format.extensions();
        scanned.files.retain(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.contains(&e))
        });
        for path in scanned.files {
            let canonical = CanonicalPath::new(&path);
            if !self.last_projection.contains_key(&canonical) {
                info!(
                    "[FileSyncController] poll_new_files: discovered new file {}",
                    path.display()
                );
                self.on_file_changed(&path).await?;
                ingested += 1;
            }
        }
        Ok(ingested)
    }

    /// Re-render all tracked files (used for events where the doc_id is unknown,
    /// e.g. block.deleted, block.fields_changed).
    pub async fn re_render_all_tracked(&mut self) -> Result<()> {
        let keys: Vec<CanonicalPath> = self.last_projection.keys().cloned().collect();

        for canonical in keys {
            let path: PathBuf = (*canonical).to_path_buf();
            // If disk content differs from last_projection, ingest the pending external
            // change first so the re-render includes both the block event and external edit.
            let disk_content = match self.fs.read_to_string(&path).await {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    debug!("[re_render_all_tracked] File deleted: {}", path.display(),);
                    continue;
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("[re_render_all_tracked] Cannot read {}", path.display())
                    });
                }
            };
            let last = self
                .last_projection
                .get(&canonical)
                .map(|s| s.as_str())
                .unwrap_or("");
            if disk_content != last {
                info!(
                    "[FileSyncController] Processing pending external change for {} before re-render",
                    path.display()
                );
                self.on_file_changed(&path).await?;
            }

            // Resolve path → doc_id
            let rel_path = path.strip_prefix(&self.root_dir).with_context(|| {
                format!(
                    "[re_render_all_tracked] {} not under root_dir {}",
                    path.display(),
                    self.root_dir.display(),
                )
            })?;
            // Resolve the document by its authoritative `#+ID` first. Name-chain
            // resolution (below) is ambiguous when a same-named subdirectory has
            // minted a placeholder page with the file's title, so it can pick the
            // wrong page and re-mint the file's `#+ID` on write-back (data loss).
            // The disk bytes carry the id, so prefer it whenever present.
            let doc = match self
                .format
                .doc_id_from_content(&disk_content)
                .map(|bare| EntityUri::block(&bare))
            {
                Some(id) => match self.doc_manager.get_by_id(&id).await {
                    Ok(Some(doc)) => Some(doc),
                    Ok(None) => None,
                    Err(e) => {
                        warn!(
                            "[re_render_all_tracked] get_by_id({id}) failed for {}: {} — skipping",
                            path.display(),
                            e
                        );
                        continue;
                    }
                },
                None => None,
            };
            let segments = path_to_name_chain(rel_path);
            let segment_refs: Vec<&str> = segments.iter().map(|s| s.as_str()).collect();
            let doc = match doc {
                Some(doc) => doc,
                None => match self.doc_manager.find_by_name_chain(&segment_refs).await {
                    Ok(Some(doc)) => doc,
                    Ok(None) => {
                        // Path was tracked but no document entity exists (e.g.
                        // empty file was registered before the skip-empty guard).
                        // Downgraded to debug: re_render_all_tracked is now
                        // debounced and runs on every burst, so warn-level would
                        // flood the log on every initial scan.
                        debug!(
                            "[re_render_all_tracked] No document found for path {} (segments: {:?}) — skipping",
                            path.display(),
                            segment_refs
                        );
                        continue;
                    }
                    Err(e) => {
                        warn!(
                            "[re_render_all_tracked] Doc lookup error for {}: {} — skipping",
                            path.display(),
                            e
                        );
                        continue;
                    }
                },
            };

            let rendered = self.render_file_by_doc_id(&doc.id, &path).await?;

            let current_last = self
                .last_projection
                .get(&canonical)
                .map(|s| s.as_str())
                .unwrap_or("");

            if rendered == current_last {
                continue;
            }

            // TOCTOU guard: re-read disk. If it changed since we read it
            // at the top of the loop (concurrent external write), writing
            // `rendered` — derived from a potentially stale CDC cache —
            // would wipe that new content. Skip this file; the next
            // on_file_changed will pick up the external delta.
            let disk_at_write = read_disk_or_empty(&self.fs, &path).await?;
            if disk_at_write != disk_content {
                tracing::debug!(
                    "[ORGSYNC_TOCTOU re_render_all_tracked] {} disk changed during processing \
                     (initial_len={} disk_now_len={}); skipping write-back.",
                    path.display(),
                    disk_content.len(),
                    disk_at_write.len(),
                );
                continue;
            }

            if self.is_quarantined(&path) {
                continue;
            }
            if let Some(parent) = path.parent() {
                self.fs.create_dir_all(parent).await?;
            }
            self.fs.write(&path, rendered.as_bytes()).await?;
            self.run_post_write_hook(&path);
            self.materialize_images(&doc.id).await?;
            self.last_projection.insert(canonical, rendered);

            info!("[FileSyncController] Re-rendered {}", path.display());
        }
        Ok(())
    }

    /// Render blocks for a document by its ID.
    ///
    /// Fetches the Document to preserve file-level metadata (e.g. `#+TODO:` keywords)
    /// in the rendered output. Falls back to block-only rendering if the Document
    /// is not found.
    async fn render_file_by_doc_id(&self, doc_id: &EntityUri, path: &Path) -> Result<String> {
        let blocks = self.block_reader.get_blocks(doc_id).await?;
        self.render_doc_blocks(doc_id, path, &blocks).await
    }

    /// Render an already-resolved, ordered block slice for `doc_id`. Shared by
    /// the full-read path (`render_file_by_doc_id`) and the incremental cache
    /// path (`render_cached_doc`) — the renderer is fed a full `&[Block]`
    /// either way, so output is byte-identical regardless of the block source.
    async fn render_doc_blocks(
        &self,
        doc_id: &EntityUri,
        path: &Path,
        blocks: &[Block],
    ) -> Result<String> {
        let rendered = match self.doc_manager.get_by_id(doc_id).await? {
            // Use the document block's actual ID as the root parent reference,
            // since blocks have parent_id = doc.id (may differ from the doc_id
            // used for lookup, e.g. file: vs block: URI schemes).
            Some(doc) => self.format.render_document(&doc, blocks, path, &doc.id),
            None => self.format.render_blocks(blocks, path, doc_id),
        };
        assert!(
            blocks.is_empty() || !rendered.trim().is_empty(),
            "[render_doc_blocks] {} blocks for doc {} but render is empty!\n\
             Blocks: {:?}",
            blocks.len(),
            doc_id,
            blocks
                .iter()
                .map(|b| format!(
                    "{{id={}, parent_id={}, content_type={}}}",
                    b.id, b.parent_id, b.content_type
                ))
                .collect::<Vec<_>>()
        );
        Ok(rendered)
    }

    /// Write image files to disk for all image blocks in this document.
    ///
    /// Called after rendering an org file — the `[[file:path]]` links exist in the
    /// org text, but the actual binary files may not yet be on disk. Reads bytes
    /// from the `ImageDataProvider` and writes to `{root_dir}/{block.content}`.
    /// Skips blocks whose files already exist.
    async fn materialize_images(&self, doc_id: &EntityUri) -> Result<()> {
        let Some(ref provider) = self.image_data else {
            return Ok(());
        };
        let blocks = self.block_reader.get_blocks(doc_id).await?;

        for block in blocks.iter().filter(|b| b.is_image_block()) {
            let image_path = self.resolve_image_path(&block.content)?;
            if self.fs.exists(&image_path) {
                continue;
            }

            let data = provider.read_image_data(&block.id).await.with_context(|| {
                format!(
                    "Failed to read image data for block {} (path: {})",
                    block.id, block.content
                )
            })?;

            let Some(data) = data else {
                debug!(
                    "[FileSyncController] No image data stored for block {} — \
                     file {} will be missing on disk",
                    block.id, block.content
                );
                continue;
            };

            if let Some(parent) = image_path.parent() {
                self.fs.create_dir_all(parent).await?;
            }
            self.fs.write(&image_path, &data).await.with_context(|| {
                format!(
                    "Failed to write image file {} for block {}",
                    image_path.display(),
                    block.id
                )
            })?;
            info!(
                "[FileSyncController] Materialized image {} ({} bytes)",
                image_path.display(),
                data.len()
            );
        }
        Ok(())
    }

    /// Read image files from disk and store them via `ImageDataProvider`.
    ///
    /// Called after parsing an org file that contains `[[file:path]]` image links.
    /// The blocks have been created in the store, but the binary data needs to be
    /// ingested so it's available for cross-peer sync and Loro storage.
    async fn ingest_images(&self, doc_id: &EntityUri) -> Result<()> {
        let Some(ref provider) = self.image_data else {
            return Ok(());
        };
        let blocks = self.block_reader.get_blocks(doc_id).await?;

        for block in blocks.iter().filter(|b| b.is_image_block()) {
            let image_path = match self.resolve_image_path(&block.content) {
                Ok(p) => p,
                Err(e) => {
                    debug!(
                        "[FileSyncController] Skipping image ingestion for block {}: {}",
                        block.id, e
                    );
                    continue;
                }
            };
            if !self.fs.exists(&image_path) {
                continue;
            }

            let data = self.fs.read(&image_path).await.with_context(|| {
                format!(
                    "Failed to read image file {} for block {}",
                    image_path.display(),
                    block.id
                )
            })?;
            provider
                .write_image_data(&block.id, data)
                .await
                .with_context(|| {
                    format!(
                        "Failed to store image data for block {} (path: {})",
                        block.id, block.content
                    )
                })?;
            info!(
                "[FileSyncController] Ingested image {} for block {}",
                image_path.display(),
                block.id
            );
        }
        Ok(())
    }

    /// Resolve a relative image path to an absolute path under root_dir.
    /// Returns Err if the resolved path escapes the root directory (path traversal).
    fn resolve_image_path(&self, relative_path: &str) -> Result<PathBuf> {
        let joined = self.root_dir.join(relative_path);
        let canonical_root = self
            .fs
            .canonicalize(&self.root_dir)
            .unwrap_or_else(|_| self.root_dir.clone());
        // For paths that don't exist yet, canonicalize the parent and append the filename
        let resolved = if self.fs.exists(&joined) {
            self.fs.canonicalize(&joined)?
        } else if let Some(parent) = joined.parent() {
            let canonical_parent = if self.fs.exists(parent) {
                self.fs.canonicalize(parent)?
            } else {
                parent.to_path_buf()
            };
            canonical_parent.join(joined.file_name().unwrap_or_default())
        } else {
            joined.clone()
        };
        assert!(
            resolved.starts_with(&canonical_root) || joined.starts_with(&self.root_dir),
            "Image path traversal blocked: {} resolves to {} which is outside {}",
            relative_path,
            resolved.display(),
            self.root_dir.display()
        );
        Ok(joined)
    }

    /// Run the post-org-write hook (fire-and-forget).
    fn run_post_write_hook(&self, path: &Path) {
        let Some(ref cmd) = self.post_write_hook else {
            return;
        };
        let cmd = cmd.clone();
        let root_dir = self.root_dir.clone();
        let file_path = path.to_path_buf();
        tokio::spawn(async move {
            let result = tokio::process::Command::new("sh")
                .arg("-l")
                .arg("-c")
                .arg(&cmd)
                .current_dir(&root_dir)
                .env("HOLON_FILE", &file_path)
                .output()
                .await;
            match result {
                Ok(output) if output.status.success() => {
                    info!(
                        "[FileSyncController] post_write hook succeeded for {}",
                        file_path.display()
                    );
                }
                Ok(output) => {
                    tracing::warn!(
                        "[FileSyncController] post_write hook failed (exit={}) for {}: {}",
                        output.status,
                        file_path.display(),
                        String::from_utf8_lossy(&output.stderr),
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "[FileSyncController] post_write hook spawn failed for {}: {}",
                        file_path.display(),
                        e,
                    );
                }
            }
        });
    }

    /// Resolve a doc_id to a filesystem path via DocumentManager.
    async fn doc_id_to_path(&self, doc_id: &EntityUri) -> Option<PathBuf> {
        // Try alias registrar first (fastest path)
        if let Some(ref registrar) = self.alias_registrar {
            if let Some(path) = registrar.resolve_alias_to_path(doc_id).await {
                return Some(path);
            }
        }

        // Walk the Document hierarchy to compute the path
        match self.doc_manager.name_chain(doc_id).await {
            Ok(chain) if !chain.is_empty() => {
                let path = self.root_dir.join(chain.join("/")).with_extension("org");
                Some(path)
            }
            Ok(_) => None,
            Err(_) => None,
        }
    }
}

/// Read a file's content, treating a missing file as empty content (a legitimate
/// "no baseline yet" state for org sync) but propagating any other IO error loudly.
/// Distinguishing absence from a real read failure prevents a transient IO error
/// from masquerading as empty disk content and wiping the user's data on write-back.
async fn read_disk_or_empty(fs: &Arc<dyn FileSystem>, path: &Path) -> Result<String> {
    match fs.read_to_string(path).await {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("reading {} for org sync", path.display())),
    }
}

/// Convert a relative path (e.g. "projects/todo.org") to a name chain (["projects", "todo"]).
fn path_to_name_chain(rel_path: &Path) -> Vec<String> {
    let doc_path = rel_path.with_extension("");
    doc_path
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(|s| s.to_string()))
        .collect()
}

/// Check if two blocks differ in ways that require an update.
/// Phase 2: when an UPDATE op's edge sets (`tags`, `requires`) match the
/// old block's, strip those keys from `params` so the provider doesn't
/// emit a wipe-and-rebuild on the `block_tags` / `block_requires` junction.
///
/// Compares as `HashSet<&str>` because junction reads have undefined row
/// order; vector compare would flag false diffs.
fn strip_unchanged_edge_fields(
    params: &mut holon_api::StorageEntity,
    old_block: &Block,
    new_block: &Block,
) {
    if old_block.tags == new_block.tags {
        params.remove("tags");
    }
    if set_eq(&old_block.requires, &new_block.requires) {
        params.remove("requires");
    }
}

fn set_eq<T: Eq + std::hash::Hash>(a: &[T], b: &[T]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let sa: HashSet<&T> = a.iter().collect();
    let sb: HashSet<&T> = b.iter().collect();
    sa == sb
}

/// Resolve one block's text content for ingest when the on-disk copy (`theirs`)
/// has diverged from the diff `base`. Implements the 3-way conflict rule of the
/// merge-fidelity ladder for the no-store (`Consolidator::Store`) mode:
///
/// - **only disk changed** (`mine == base`) → normal ingest: take `theirs`
///   verbatim, no merge. The store never touched this block.
/// - **both converged** (`theirs == mine`) → no real conflict: take `theirs`
///   (equal to `mine`), no merge.
/// - **both diverged** (`theirs != base` && `mine != base` && `theirs != mine`)
///   → a genuine concurrent file-vs-UI edit: 3-way merge `(base, theirs, mine)`
///   through the transient CRDT text.
///
/// Returns `(content_to_ingest, merged)` where `merged` is `true` only in the
/// last case (so the caller can disclose it and force a disk write-back).
/// Precondition: `theirs != base` (disk changed) — the caller only invokes this
/// inside the existing disk-vs-base content-diff gate. Structural conflicts
/// (parent/order) are out of scope: this is text CONTENT only.
fn three_way_text_content(
    base: &str,
    theirs: &str,
    mine: &str,
    merger: &dyn ThreeWayTextMerge,
) -> Result<(String, bool)> {
    debug_assert_ne!(
        theirs, base,
        "caller must gate on disk-changed (theirs != base)"
    );
    if mine == base || theirs == mine {
        // Only the disk side changed (or both landed on the same text): the
        // store held no competing edit, so the disk content wins as today.
        return Ok((theirs.to_string(), false));
    }
    // Both sides diverged from the common ancestor → merge, don't clobber.
    let merged = merger
        .merge_text(base, theirs, mine)
        .with_context(|| "3-way text merge of concurrent file-vs-UI edit failed")?;
    Ok((merged, true))
}

#[cfg(test)]
mod three_way_text_tests {
    use super::*;

    /// Stub merger: records that it was called and returns a sentinel so tests
    /// can assert whether the controller path chose to merge vs pass through.
    struct StubMerge;
    impl ThreeWayTextMerge for StubMerge {
        fn merge_text(&self, base: &str, theirs: &str, mine: &str) -> Result<String> {
            Ok(format!("MERGED({base}|{theirs}|{mine})"))
        }
    }

    #[test]
    fn both_changed_triggers_merge() {
        // base "abc", disk "Xabc", store "abcY" — a true concurrent edit.
        let (content, merged) = three_way_text_content("abc", "Xabc", "abcY", &StubMerge).unwrap();
        assert!(merged, "both sides diverged → must merge");
        assert_eq!(content, "MERGED(abc|Xabc|abcY)");
    }

    #[test]
    fn only_disk_changed_passes_theirs_through() {
        // store never touched this block (mine == base): disk wins, no merge.
        let (content, merged) = three_way_text_content("abc", "Xabc", "abc", &StubMerge).unwrap();
        assert!(!merged, "only disk changed → no merge");
        assert_eq!(content, "Xabc");
    }

    #[test]
    fn converged_edits_pass_through() {
        // both sides independently produced the same text: no real conflict.
        let (content, merged) = three_way_text_content("abc", "abZ", "abZ", &StubMerge).unwrap();
        assert!(!merged, "theirs == mine → no merge");
        assert_eq!(content, "abZ");
    }
}
