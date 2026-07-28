//! Unified bidirectional sync controller for Org files ↔ block store.
//!
//! Unified bidirectional sync: a single component
//! that uses the **projection + diff-ingestion** pattern:
//!
//! - `last_projection`: what we last wrote to (or confirmed on) disk, per file.
//! - Echo suppression: `disk_content == last_projection[file]` (no timing
//!   window).
//! - External edits: detected by diffing against `last_projection`.
//!
//! The controller runs on a single task via `tokio::select!`, so
//! `on_file_changed` and `on_block_changed` are serialized — no concurrent
//! access to `last_projection`.
//!
//! **Decoupled from Loro/Turso**: uses `BlockReader` and `DocumentManager`
//! traits.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use holon_api::EntityUri;
use holon_api::POSITION_AFTER_BLOCK_ID_PARAM;
use holon_api::ROUTING_DOC_URI_KEY;
use holon_api::SnapshotBlock;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::capability::Consolidator;
use holon_core::CanonicalPath;
use holon_core::DownstreamProjection;
use holon_core::block_ordering::BlockOrdering;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::WritebackDropVerdict;
use holon_core::fractional_index::default_sort_key;
use indexmap::IndexMap;
use tracing::debug;
use tracing::info;
use tracing::warn;

use crate::BaseKey;
use crate::BaseStore;
use crate::FileSystem;
use crate::SyncBaseStore;
use crate::sync_ports::AliasRegistrar;
use crate::sync_ports::BlockMatchStrategy;
use crate::sync_ports::BlockReader;
use crate::sync_ports::DocumentManager;
use crate::sync_ports::ExistingChild;
use crate::sync_ports::ImageDataProvider;
use crate::sync_ports::IncomingIdentity;
use crate::sync_ports::MatchBasis;
use crate::sync_ports::MatchContext;
use crate::sync_ports::MatchSituation;
use crate::sync_ports::MatchVerdict;
use crate::sync_ports::MountRegistry;
use crate::sync_ports::ThreeWayTextMerge;

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

/// Mass-truncation tripwire (Fork B B1' / ADR 0025): the block-driven
/// write-back path vetoes only when the ungrounded-drop count exceeds
/// `max(TRIPWIRE_MIN_DROP_BLOCKS, block_count / TRIPWIRE_DROP_FRACTION_DENOM)`.
/// `3` and `4` (= 25%) are chosen so the row-28 loss class (an FK-rollback that
/// aborted ~20 contiguous blocks) fires while routine single-block deletions /
/// splits never do. TEMPORARY: tighten toward zero once removals are grounded
/// in ops (per-block `Remove` delivery / the ADR-0025 C2b history relation).
const TRIPWIRE_MIN_DROP_BLOCKS: usize = 3;
const TRIPWIRE_DROP_FRACTION_DENOM: usize = 4;

/// Outcome of grounding a write-back's absent (drop-candidate) blocks against
/// the sibling files they may have de-inlined into.
///
/// `siblings` is the surviving-projection union (each distinct sibling file's
/// on-disk content). `unresolvable` is the id of every absent block whose
/// own-file path could NOT be resolved because `name_chain` failed loud (a
/// prohibited page-under-non-page topology; BugFunnel row 23 / row 29). A
/// non-empty `unresolvable` set means write-back genuinely CANNOT prove where
/// those blocks went — so the guard must ABORT the write (quarantine) rather
/// than silently tolerate the drop under the mass-truncation threshold. This is
/// the fix for the first-boot 6,245-line Projects.org destruction, where a
/// name_chain-failed grounding storm left blocks ungrounded but the count fell
/// under the 25% threshold and the truncated file was written anyway.
#[derive(Debug, Default)]
struct SiblingGrounding {
    siblings: Vec<(PathBuf, String)>,
    unresolvable: Vec<String>,
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
    /// on content, so the base can never desync from `last_projection` no
    /// matter which render path last updated it.
    base_source: HashMap<CanonicalPath, String>,

    /// The write-back render itself, shared with inspection callers (the
    /// `render_org` MCP tool) so "what write-back would produce" is
    /// answered by the code that produces it.
    renderer: Arc<crate::writeback_render::WritebackRenderer>,

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
    /// when wired (production, via a transient LoroText impl). Consulted only
    /// in `Consolidator::Store` (SqlOnly) mode: when an org-file edit and a
    /// UI edit concurrently changed the SAME block's text content (both
    /// diverged from the BaseStore base), the disk edit is 3-way merged
    /// with the current store content instead of clobbering it (whole-value
    /// LWW). In `Upstream` (Loro) mode this is left unused — the live CRDT
    /// already merges concurrent edits, so adding a second merge here would
    /// be wrong.
    text_merge: Option<Arc<dyn ThreeWayTextMerge>>,

    /// Inc 3: authoritative "is this a registered shared-subtree mount?" seam.
    /// Consulted before skipping ingest of a file whose parsed content LOOKS
    /// like a mount, so a hand-authored `:share-role: mount:` file is not
    /// silently skipped (data loss). `None` (SqlOnly / tests) ⇒ never skip.
    mount_registry: Option<Arc<dyn MountRegistry>>,
    /// C2b history port (R3b): records ONE `block_history` op_group for a
    /// genuinely-new doc/day PAGE created by RUNTIME org-ingest. `None` when no
    /// Turso history store is wired. Never records during the initial cold-boot
    /// scan (see `in_initial_scan`), so a vault load does not flood history.
    history: Option<Arc<dyn holon_api::HistoryStore>>,
    /// Clock for the ingest history event's `at_millis` (injected in tests; the
    /// OS `SystemClock` in production).
    clock: Arc<dyn holon_api::Clock>,

    /// Strategy deciding, per minted id-less headline, remap-onto-twin
    /// vs mint (the matching spectrum). Defaults to [`TieredMatcher`]
    /// (T1 exact position, guarded against wrong-twin re-home + T3
    /// content-unique-in-document + T2 descendant-subtree-signature /
    /// relative-position pairing, per RULING A2); `with_block_matcher` swaps
    /// in a different tier (e.g. the narrower [`PositionalExactMatcher`] for
    /// tests).
    block_matcher: Arc<dyn BlockMatchStrategy>,

    /// Initial-scan feed-barrier batching (boot ingest latency, Options 0+1).
    /// `None` in steady state — each runtime `on_file_changed` pays its own
    /// per-file `wait_for_blocks_in_feed` barrier (unchanged). `Some(buf)` only
    /// between [`begin_initial_scan`](Self::begin_initial_scan) and
    /// [`finish_initial_scan`](Self::finish_initial_scan): the per-file feed
    /// waits (sites A and C) instead push their expected ids into `buf` and
    /// skip the wait, so the whole cold-boot vault ingests without N×(≤2s)
    /// round-trips; `finish_initial_scan` then does ONE convergence wait
    /// over the union before `signal_ready`. `block_raw` is written
    /// synchronously per file, so the per-file `get_blocks` count-check
    /// (the intra-file write-success gate) and wait B (`ordering.children`,
    /// the ordering-authority propagation gate) stay in place and cover
    /// correctness; only the sidebar-facing `block`-matview `LiveData` feed
    /// is deferred to end-of-scan. Scoped to the initial scan —
    /// runtime edits keep the per-edit barrier and its fail-loud stall
    /// detection.
    scan_feed_ids: Option<Vec<String>>,

    /// Write-back quarantine (dogfood 2026-07-10 region data-loss guard). A
    /// file whose ingest FAILED partway (`ingest_file` returned `Err` — a
    /// rejected block op, a parsed-vs-landed count mismatch, a stalled
    /// feed) is recorded here so no write-back path re-renders it from the
    /// DB. The DB holds only a PREFIX of the file's blocks after a partial
    /// ingest, so rendering it would overwrite the on-disk file with a
    /// truncated view — silent data loss. A quarantined file is skipped by
    /// every write-back until a later ingest of it SUCCEEDS (`ingest_file`
    /// returns `Ok`), which clears the entry. Keyed by
    /// the same `CanonicalPath` as `last_projection`.
    quarantined: HashSet<CanonicalPath>,
    /// Quarantined files whose write-back skip has already been logged at
    /// ERROR once. Repeat skips log at `debug` — one loud disclosure per
    /// quarantine episode, not an ERROR flood on every subsequent write-back
    /// attempt (real-vault boot 2026-07-12: hundreds of identical lines).
    /// Cleared alongside `quarantined` when a clean re-ingest lifts the
    /// quarantine. Interior mutability because `is_quarantined` is a `&self`
    /// read used from immutable contexts.
    quarantine_skip_logged: std::sync::Mutex<HashSet<CanonicalPath>>,

    /// Copy-on-write seed docs (`holon_api::is_seed_layout_doc`, e.g.
    /// `block:__default__`) stay VIRTUAL through boot: the boot seed/re-seed
    /// writes them into Loro from the current asset, and those writes must NOT
    /// auto-materialize a vault `.org` file (that file-pin is the F4
    /// stale-seed bug). `true` from construction until
    /// [`finish_boot_seeding`](Self::finish_boot_seeding) is called at the end
    /// of the boot ingest (after `materialize_missing_page_files`). Once
    /// `false`, a genuine runtime user edit to a seed doc DOES materialize its
    /// file — the copy-on-write moment, after which the file wins and
    /// re-seeding is suppressed.
    boot_seeding: bool,

    /// Pristine (shipped-asset) render of each VIRTUAL seed doc
    /// (`holon_api::is_seed_layout_doc`), keyed by its file path. Recorded
    /// during boot (when the store still equals the freshly (re-)seeded asset)
    /// and used AFTER boot as the copy-on-write baseline: a seed doc whose
    /// render still equals its pristine value stays virtual (no file written);
    /// a diverged render — a real user edit — materializes the file. This
    /// content baseline (not a boot/after time flag alone) is what makes the
    /// gate RACE-FREE: a late boot-seed delta arriving after `boot_seeding`
    /// flips still renders == pristine, so it is not mistaken for a user edit.
    seed_pristine: HashMap<CanonicalPath, String>,

    /// Paths whose write-back hit a persistent read-only-filesystem error
    /// (EROFS, os error 30) — e.g. a relay/synthetic doc whose resolved path
    /// has no writable backing file, or a vault on a read-only mount. The
    /// FIRST failure logs a loud, information-rich ERROR (doc_id, path, cause)
    /// and records the path here; every subsequent CDC event for that path
    /// SKIPS the write instead of re-issuing the doomed syscall (real-vault
    /// boot 2026-07-22: `Read-only file system (os error 30)` on EVERY CDC
    /// event). Disclosed degraded mode (Fail Loud, Never Fake): one loud
    /// disclosure, then a quiet skip. The SOLE resume trigger is a successful
    /// `ingest_file` of the path (runtime re-ingest via `on_file_changed`, or
    /// a boot re-scan): that path re-resolves the doc identity and
    /// re-registers its alias before clearing the mark, proving the file is
    /// writable-backed again. A pure relay/synthetic doc that is never
    /// ingested (no on-disk `.org`) intentionally never clears — there is no
    /// writable file to resume write-back to.
    writeback_readonly: HashSet<CanonicalPath>,

    /// Ingest quarantine for NEW-file discovery (`poll_new_files`, Inc 3b). A
    /// freshly-discovered file whose ingest FAILED is recorded here keyed by
    /// the exact `(mtime, size)` signature it failed at. `poll_new_files`
    /// skips a path still carrying its recorded signature, so a single
    /// poisoned org file is NOT re-attempted (and NOT re-logged) on every
    /// discovery tick -- before this, one poison aborted the whole walk
    /// with `?`, so every later healthy file was never ingested and each
    /// tick re-hit the same poison first. The entry is dropped the moment
    /// the file CHANGES (signature no longer matches -> re-attempted) or
    /// re-ingests cleanly. This is the disk->DB (ingest-side) counterpart
    /// of the DB->disk write-back [`quarantined`](Self::quarantined) set.
    ingest_quarantine: HashMap<CanonicalPath, (std::time::SystemTime, u64)>,
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
        let renderer = Arc::new(crate::writeback_render::WritebackRenderer::new(
            block_reader.clone(),
            doc_manager.clone(),
            format.clone(),
        ));
        Self {
            last_projection: HashMap::new(),
            last_projection_hash: HashMap::new(),
            disk_signatures: HashMap::new(),
            base_store: SyncBaseStore::in_memory(),
            base_source: HashMap::new(),
            renderer,
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
            block_matcher: Arc::new(TieredMatcher),
            mount_registry: None,
            history: None,
            clock: Arc::new(holon_api::SystemClock),
            scan_feed_ids: None,
            quarantined: HashSet::new(),
            quarantine_skip_logged: std::sync::Mutex::new(HashSet::new()),
            boot_seeding: true,
            seed_pristine: HashMap::new(),
            writeback_readonly: HashSet::new(),
            ingest_quarantine: HashMap::new(),
        }
    }

    /// Enter initial-scan mode: the per-file feed barriers (sites A and C in
    /// `on_file_changed`) buffer their expected ids instead of waiting, so the
    /// cold-boot vault ingests without N×(≤2s) feed round-trips. Must be paired
    /// with [`finish_initial_scan`](Self::finish_initial_scan), which drains
    /// the buffer with one convergence wait. Boot ingest latency, Option 1.
    pub fn begin_initial_scan(&mut self) {
        self.scan_feed_ids = Some(Vec::new());
    }

    /// End the boot seed/re-seed phase. Called once by the boot ingest after
    /// `materialize_missing_page_files`, so that from here on a runtime user
    /// edit to a copy-on-write seed doc (`holon_api::is_seed_layout_doc`)
    /// materializes its vault file (copy-on-write), while every boot-time
    /// re-seed write stayed virtual.
    pub fn finish_boot_seeding(&mut self) {
        self.boot_seeding = false;
    }

    /// Copy-on-write gate for a VIRTUAL seed doc (`is_seed_layout_doc`, e.g.
    /// `block:__default__`). Returns `true` when THIS render must NOT be
    /// written to disk (the doc stays virtual). Non-seed docs and seed docs
    /// that already own a file on disk are never gated (`false` — write
    /// normally; a present file WINS). During boot it records the pristine
    /// shipped-asset render and always skips. After boot it skips only
    /// while the render still equals that pristine baseline; a diverged
    /// render (a real user edit) falls through (`false`) to materialize the
    /// file — copy-on-write — after which the on-disk file wins.
    /// Content-based, so a late boot-seed delta (render == pristine) is
    /// never mistaken for a user edit.
    fn gate_virtual_seed_write(
        &mut self,
        doc_id: &EntityUri,
        canonical: &CanonicalPath,
        rendered: &str,
        disk_nonempty: bool,
    ) -> bool {
        if !holon_api::is_seed_layout_doc(doc_id) {
            return false;
        }
        if disk_nonempty || self.last_projection.contains_key(canonical) {
            return false;
        }
        if self.boot_seeding {
            self.seed_pristine
                .insert(canonical.clone(), rendered.to_string());
            return true;
        }
        // After boot: virtual only while still byte-identical to the pristine
        // asset render. No baseline recorded ⇒ default to virtual (never
        // auto-create a seed file we have no evidence the user changed).
        self.seed_pristine
            .get(canonical)
            .map_or(true, |pristine| pristine == rendered)
    }

    /// Whether the controller is currently in initial-scan (feed-barrier
    /// batching) mode. `false` in steady state — used by tests to prove the
    /// scan flag does not leak past `finish_initial_scan`.
    pub fn in_initial_scan(&self) -> bool {
        self.scan_feed_ids.is_some()
    }

    /// Leave initial-scan mode: do exactly ONE feed-convergence wait over the
    /// union of every id the scan's deferred barriers buffered, then reset to
    /// steady state. Fail loud (never silently continue) if the `block`-matview
    /// feed goes QUIESCENT while ids are still missing — a stalled
    /// projection/CDC is a real bug. `stall_ms` is a no-progress window, not a
    /// wall-clock ceiling: as long as the feed keeps landing expected ids the
    /// wait continues (a real-vault cold boot legitimately takes minutes; the
    /// old fixed budget expired early under load, BugFunnel 2026-07-12).
    /// Called before `signal_ready` so a genuine stall becomes a scan failure.
    pub async fn finish_initial_scan(&mut self, stall_ms: u64) -> Result<()> {
        let mut ids = self.scan_feed_ids.take().unwrap_or_default();
        ids.sort();
        ids.dedup();
        let t = std::time::Instant::now();
        let caught_up = if ids.is_empty() {
            true
        } else {
            self.wait_for_feed_progress(&ids, stall_ms).await
        };
        tracing::debug!(
            target: "holon_latency",
            stage = "boot_feed_converge",
            ms = t.elapsed().as_millis() as u64,
            blocks = ids.len() as u64,
            caught_up = caught_up,
            "holon_latency",
        );
        // The once-per-boot INFO replacement for the demoted per-batch chatter:
        // matview/CDC delivery is what the readiness gate actually waits on.
        tracing::info!(
            "[InitialScan] feed convergence: {} block(s) in {}ms (caught_up={})",
            ids.len(),
            t.elapsed().as_millis(),
            caught_up,
        );
        // Steady-state guard: the scan flag must not leak past finish.
        debug_assert!(
            self.scan_feed_ids.is_none(),
            "scan_feed_ids must be None after finish_initial_scan"
        );
        if !caught_up {
            let present = self.block_reader.blocks_in_feed_count(&ids).await;
            anyhow::bail!(
                "[finish_initial_scan] block feed did not converge — no progress for {stall_ms}ms \
                 with {} of {} expected id(s) still missing — projection/CDC stalled during the \
                 initial scan",
                ids.len() - present,
                ids.len()
            );
        }
        Ok(())
    }

    /// Progress-grounded feed wait (replaces the fixed wall-clock ceiling).
    ///
    /// Waits in `stall_ms` slices via `wait_for_blocks_in_feed`; after each
    /// unsuccessful slice it re-counts how many expected ids are present. As
    /// long as that count is rising the feed is alive and the wait continues —
    /// there is deliberately NO total-elapsed cap, because total ingest time is
    /// a function of vault size, not of health. Returns `false` only when a
    /// full `stall_ms` window passes with zero new expected ids (feed quiescent
    /// AND incomplete) — that is a real projection/CDC defect, never a "vault
    /// too big" artifact. Never fakes progress: `true` requires every id
    /// present.
    async fn wait_for_feed_progress(&self, ids: &[String], stall_ms: u64) -> bool {
        let mut present = self.block_reader.blocks_in_feed_count(ids).await;
        loop {
            if self
                .block_reader
                .wait_for_blocks_in_feed(ids, stall_ms)
                .await
            {
                return true;
            }
            let now = self.block_reader.blocks_in_feed_count(ids).await;
            if now <= present {
                return false;
            }
            tracing::debug!(
                target: "holon_latency",
                stage = "boot_feed_progress",
                present = now as u64,
                expected = ids.len() as u64,
                "holon_latency",
            );
            present = now;
        }
    }

    /// The initial-scan feed barrier (sites A and C). During the scan
    /// (`scan_feed_ids.is_some()`) the expected ids are buffered for the single
    /// end-of-scan convergence wait and this returns immediately. In steady
    /// state it performs the per-file `wait_for_blocks_in_feed` exactly as
    /// before (byte-identical runtime behavior). Emits `boot_feed_wait` on the
    /// `holon_latency` target so the cost — and how much of the 2s ceiling
    /// binds — is measurable per file. `site` is `"updates"` (A) or
    /// `"creates"` (C).
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
        // Steady-state path: progress-grounded wait with a 2s STALL window
        // (not a 2s total ceiling — a busy projection that is still landing
        // rows keeps the wait alive).
        debug_assert!(self.scan_feed_ids.is_none());
        let t = std::time::Instant::now();
        let caught_up = self.wait_for_feed_progress(ids, 2000).await;
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

    /// Inject a block-matching strategy for the ID-less external-rewrite
    /// reconcile. Without it the controller uses [`PositionalExactMatcher`]
    /// (PR #81 exact-content-at-position). Mirrors `with_text_merge` /
    /// `with_mount_registry`.
    pub fn with_block_matcher(mut self, matcher: Arc<dyn BlockMatchStrategy>) -> Self {
        self.block_matcher = matcher;
        self
    }

    /// Wire the authoritative mount registry (Inc 3). Without it, a file whose
    /// parsed content looks like a shared-subtree projection is NOT treated as
    /// one (ingested normally) — the guard only skips ids the registry
    /// confirms.
    pub fn with_mount_registry(mut self, registry: Arc<dyn MountRegistry>) -> Self {
        self.mount_registry = Some(registry);
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

    /// Wire the C2b history store (R3b): the org-ingest doc-page create then
    /// records one op_group through it. Absent in org-standalone wirings.
    pub fn with_history_store(mut self, history: Arc<dyn holon_api::HistoryStore>) -> Self {
        self.history = Some(history);
        self
    }

    /// Override the clock used for ingest history timestamps (test
    /// determinism).
    pub fn with_clock(mut self, clock: Arc<dyn holon_api::Clock>) -> Self {
        self.clock = clock;
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
                    "[FileSyncController] Loaded last_projection_hash for {} files (will skip \
                     ingest when disk_bytes hash matches)",
                    self.last_projection_hash.len()
                );
            }
            Err(e) => {
                warn!(
                    "[FileSyncController] load_file_hashes failed; cold-boot fast path disabled, \
                     will re-ingest every file. Error: {e}"
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
        use sha2::Digest;
        use sha2::Sha256;
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

    /// Boot store-health sweep (dogfood 2026-07-21, BugFunnel row 295): repair
    /// every title-less (empty-content) `Page` doc-root a broken
    /// `convert_block_to_page`/delete left behind, UNCONDITIONALLY after the
    /// initial scan and INDEPENDENT of the ingest byte-identity fast-path.
    ///
    /// Why a dedicated sweep and not a fast-path predicate: the skip certifies
    /// byte-identity only; a degraded empty-content `Page` is byte-identical to
    /// a healthy `#+ID:`-only sibling (the discriminator is STORE state, not
    /// disk bytes), so an unchanged degraded file is skipped by ingest on
    /// every boot. Its repair therefore cannot live inside ingest — it
    /// belongs to this store-health seam, which reaches the same
    /// [`heal_title_less_doc_root`] implementation the file-watch path
    /// uses.
    ///
    /// Iterates the vault's org files because the file PATH is the
    /// authoritative title source that an orphaned empty root cannot supply
    /// from the store. Idempotent: a healthy vault writes nothing.
    pub async fn heal_title_less_doc_roots(&mut self) -> Result<()> {
        let scanned = self
            .fs
            .scan_directory(&self.root_dir)
            .await
            .with_context(|| {
                format!(
                    "[FileSyncController] store-health sweep: scan {}",
                    self.root_dir.display()
                )
            })?;
        let mut healed = 0usize;
        for file in scanned.files {
            if file.extension().and_then(|e| e.to_str()) != Some("org") {
                continue;
            }
            if self
                .heal_title_less_doc_root(&file)
                .await
                .with_context(|| {
                    format!(
                        "[FileSyncController] store-health sweep at {}",
                        file.display()
                    )
                })?
            {
                healed += 1;
            }
        }
        if healed > 0 {
            info!("[FileSyncController] store-health sweep healed {healed} title-less doc-root(s)");
        }
        Ok(())
    }

    /// The SINGLE title-less doc-root heal (BugFunnel row 295). Resolve
    /// `path`'s `#+ID` doc-root; if it is an empty-content `Page` (the
    /// broken convert/delete product — an empty-content `Page` is never
    /// legal), re-derive its title from the filename and, when it is
    /// orphaned (parent lost to the root sentinel), reparent it under its
    /// folder chain — through the SAME `update_in_tree` seam a normal block
    /// update uses. Idempotent: a healthy / absent / non-`#+ID` doc-root is
    /// a no-op (returns `false`). Fail-loud WARN when the filename has no
    /// derivable stem (the disclosed render-side `(untitled)` placeholder
    /// from PR #59 covers that row).
    ///
    /// Invoked ONLY from the store-health seam — the boot sweep
    /// ([`heal_title_less_doc_roots`]) and the runtime file-watch reingest —
    /// never from the ingest fast-path, which certifies byte-identity only.
    /// Returns whether a heal was written.
    async fn heal_title_less_doc_root(&mut self, path: &Path) -> Result<bool> {
        let disk_content = match self.fs.read_to_string(path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("[FileSyncController] heal: cannot read {}", path.display())
                });
            }
        };
        let Some(bare) = self.format.doc_id_from_content(&disk_content) else {
            return Ok(false);
        };
        let id = EntityUri::block(&bare);
        let Some(mut doc) = self.doc_manager.get_by_id(&id).await? else {
            return Ok(false);
        };
        if !doc.content.trim().is_empty() {
            return Ok(false); // healthy doc-root — nothing to heal
        }
        let rel_path = path.strip_prefix(&self.root_dir).map_err(|e| {
            anyhow::anyhow!(
                "File {} not under root {}: {}",
                path.display(),
                self.root_dir.display(),
                e
            )
        })?;
        let segments = path_to_name_chain(rel_path);
        let segment_refs: Vec<&str> = segments.iter().map(|s| s.as_str()).collect();
        let filename_title = segments.last().cloned().unwrap_or_default();
        if filename_title.trim().is_empty() {
            // Residual case: an empty-content `Page` whose file name has NO
            // derivable stem (a pathological empty stem, e.g. a bare `.org`).
            // Re-deriving an empty title would silently re-leave the blank row,
            // so disclose loudly by doc id and leave content empty; the sidebar
            // row is covered by the disclosed render-side `(untitled)` placeholder
            // (`render_eval` `empty:` named-arg, PR #59).
            tracing::warn!(
                doc_id = %doc.id,
                path = %path.display(),
                "cannot heal title-less Page doc-root: the file name has no derivable stem, so no \
                 title can be re-derived — leaving content empty. The disclosed render-side \
                 '(untitled)' placeholder covers the sidebar row; fix the file name or re-title \
                 the page."
            );
            return Ok(false);
        }
        let derived_parent = if (doc.parent_id == EntityUri::no_parent()
            || doc.parent_id.is_sentinel())
            && segments.len() > 1
        {
            let parent_segments: Vec<&str> = segment_refs[..segments.len() - 1].to_vec();
            self.resolve_dir_page_chain(&parent_segments).await?.id
        } else {
            doc.parent_id.clone()
        };
        tracing::warn!(
            doc_id = %doc.id,
            filename_title = %filename_title,
            old_parent = %doc.parent_id,
            new_parent = %derived_parent,
            path = %path.display(),
            "healing title-less Page doc-root: empty content re-derived from filename (broken \
             convert/delete product); reparenting when orphaned"
        );
        doc.content = filename_title;
        doc.parent_id = derived_parent;
        // Converge through the single org→block write seam a normal block update
        // uses. A MINIMAL params map (no `tags`/`requires` keys) so `update_in_tree`
        // leaves the `Page` tag + junctions untouched and only rewrites content +
        // parent. `ROUTING_DOC_URI_KEY` is the doc-root's own id (it IS the doc).
        let mut params = holon_api::StorageEntity::new();
        params.insert("id".into(), Value::String(doc.id.to_string()));
        params.insert("parent_id".into(), Value::String(doc.parent_id.to_string()));
        params.insert("content".into(), Value::String(doc.content.clone()));
        params.insert(
            "content_type".into(),
            Value::String(doc.content_type.to_string()),
        );
        params.insert(
            holon_api::ROUTING_DOC_URI_KEY.into(),
            Value::String(doc.id.to_string()),
        );
        self.ordering
            .apply_ingest_batch(vec![("update".to_string(), params)])
            .await
            .map_err(|e| anyhow::anyhow!("heal update for {}: {e:#}", path.display()))?;
        // Publish the healed row to the SQL sink (same single-sink-writer
        // contract as the ingest / delete paths' flush).
        if let Some(downstream) = &self.downstream {
            downstream
                .flush()
                .await
                .map_err(|e| anyhow::anyhow!("downstream flush after title-less heal: {e}"))?;
        }
        Ok(true)
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

        // Refutation fix (2026-07-27) — id-based reunification safety net
        // (recognition principle). Before ANY cascade, check whether this doc's
        // identity (`#+ID`) already lives at ANOTHER tracked path that still
        // exists on disk. If so, the vanished file is the SOURCE side of a
        // rename whose destination we already track — the watcher's pairing fell
        // back to a bare `Remove` (a byte-syncer / lock file interposed, or the
        // pair timed out), and the title-based D3 guard below cannot catch it
        // because the title has not yet followed. Re-home instead of
        // cascade-deleting a LIVE doc. A TRUE move-out-of-vault (the id lives
        // nowhere else) falls through and still cascades below.
        //
        // Bounded: scans only the in-memory `last_projection` (the tracked set),
        // not the whole vault. The Remove-arrives-before-the-destination-ingest
        // ordering (which this scan cannot see yet) is prevented at the source by
        // the pairing's relevance-gate + timeout-only flush, and any residual is
        // repaired by `poll_new_files` + re-ingest.
        let reunion: Option<PathBuf> = self
            .last_projection
            .iter()
            .find_map(|(p, content)| {
                if p == canonical {
                    return None;
                }
                match self.format.doc_id_from_content(content) {
                    Some(bare) if EntityUri::block(&bare) == document_uri => {
                        Some(p.as_path_buf().clone())
                    }
                    _ => None,
                }
            })
            .filter(|dest| self.fs.exists(dest));
        if let Some(dest) = reunion {
            info!(
                "[FileSyncController] Deleted file {} is the SOURCE side of a rename — document                  {} already lives at {}; re-homing (id-based reunification) instead of                  cascade-deleting",
                path.display(),
                document_uri,
                dest.display(),
            );
            // Box::pin breaks the async recursion cycle (on_file_deleted ->
            // on_file_renamed -> on_file_changed -> on_file_deleted).
            return Box::pin(self.on_file_renamed(path, &dest)).await;
        }

        // D3 / identity plan §5 guard (order matters: guard BEFORE any delete). A
        // page rename re-homes the doc to a NEW `<new-title>.org` and removes the
        // OLD file; that removal fires THIS handler for the old path, whose
        // `#+ID:` still resolves to the (renamed, re-homed) doc. Cascade-deleting
        // here would DELETE the very doc the rename just moved — the double
        // defect the RenameDocument SUT comments flag. So: never cascade-delete a
        // doc whose CURRENT authoritative file path is a DIFFERENT file than the
        // vanished one. The vanished file is stale rename-cleanup — forget it and
        // stop.
        if document.is_page() {
            let current_chain = self.authoritative_name_chain(&document_uri).await?;
            if !current_chain.is_empty() {
                let current_path = self
                    .root_dir
                    .join(current_chain.join("/"))
                    .with_extension("org");
                if CanonicalPath::new(&current_path) != *canonical {
                    info!(
                        "[FileSyncController] Deleted file {} is stale — document {} now lives at \
                         {} (page-rename cleanup); NOT cascading the delete",
                        path.display(),
                        document_uri,
                        current_path.display(),
                    );
                    self.forget_file_state(canonical);
                    return Ok(());
                }
            }
        }

        let blocks = self.block_reader.get_blocks(&document_uri).await?;
        info!(
            "[FileSyncController] File deleted externally: {} — cascade-deleting document {} ({} \
             blocks)",
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
        // A deleted file must not leave a stale ingest-quarantine entry: if the
        // same path reappears it starts un-quarantined (fresh discovery).
        self.ingest_quarantine.remove(canonical);
    }

    /// Move every per-file tracking entry from `from` to `to` — the rename
    /// analog of [`forget_file_state`](Self::forget_file_state). Echo
    /// suppression, the cold-boot hash, disk signatures, the diff base source,
    /// and any quarantine follow the file to its new home WITHOUT a
    /// forget/re-discover round-trip (which would re-ingest the file as new).
    /// The diff `base` itself is keyed by document id, not path, so it needs no
    /// migration.
    fn migrate_file_state(&mut self, from: &CanonicalPath, to: &CanonicalPath) {
        if let Some(v) = self.last_projection.remove(from) {
            self.last_projection.insert(to.clone(), v);
        }
        if let Some(v) = self.last_projection_hash.remove(from) {
            self.last_projection_hash.insert(to.clone(), v);
        }
        if let Some(v) = self.disk_signatures.remove(from) {
            self.disk_signatures.insert(to.clone(), v);
        }
        if let Some(v) = self.base_source.remove(from) {
            self.base_source.insert(to.clone(), v);
        }
        if let Some(v) = self.ingest_quarantine.remove(from) {
            self.ingest_quarantine.insert(to.clone(), v);
        }
        if let Some(v) = self.seed_pristine.remove(from) {
            self.seed_pristine.insert(to.clone(), v);
        }
        if self.quarantined.remove(from) {
            self.quarantined.insert(to.clone());
        }
        if self.writeback_readonly.remove(from) {
            self.writeback_readonly.insert(to.clone());
        }
    }

    /// Handle an ATOMIC on-disk rename (`mv A.org B.org` in the vault). Reached
    /// from the org sync loop when the change source delivers
    /// `FileChangeKind::Rename { from }` — the maximal-information path that
    /// carries BOTH the old and new paths in ONE event, so the owning document
    /// is re-homed WITHOUT the delete-then-create window that makes a rename
    /// indistinguishable from a delete. That window is the exact hazard
    /// `on_file_deleted`'s D3 guard cannot close for a file rename whose title
    /// has not yet followed: the vanished path's `#+ID:` still resolves to the
    /// (not-yet-retitled) doc, whose authoritative title-chain still points at
    /// the OLD file, so the guard reads "same file" and cascade-deletes the
    /// very document the move re-homed.
    ///
    /// Re-homes the doc: migrates the per-file tracking state from `from` to
    /// `to`, re-points the doc_id→path alias, retitles the doc-root page to the
    /// new file stem (the file-move spec: a page's title FOLLOWS its file
    /// name), then ingests `to` to reconcile its bytes. The doc KEEPS its
    /// id — a rename never re-mints (ruled D1). NEVER cascade-deletes.
    #[tracing::instrument(skip(self), name = "org.on_file_renamed", fields(from = %from.display(), to = %to.display()))]
    pub async fn on_file_renamed(&mut self, from: &Path, to: &Path) -> Result<()> {
        let from_canon = CanonicalPath::new(from);
        let to_canon = CanonicalPath::new(to);

        // Resolve the document that owned `from`. Identity comes from the last
        // projected content's `#+ID:` (same authority `on_file_deleted` uses);
        // the moved bytes now live at `to` carrying that same `#+ID:`. When we
        // never projected `from` this session, fall back to a get-only
        // name-chain lookup off `from`'s path (a rename must never MINT a doc).
        let last = self.last_projection.get(&from_canon).cloned();
        let document = match last
            .as_deref()
            .and_then(|l| self.format.doc_id_from_content(l))
        {
            Some(bare) => self.doc_manager.get_by_id(&EntityUri::block(&bare)).await?,
            None => match from.strip_prefix(&self.root_dir) {
                Ok(rel) => {
                    let segments = path_to_name_chain(rel);
                    let segment_refs: Vec<&str> = segments.iter().map(|s| s.as_str()).collect();
                    self.doc_manager.find_by_name_chain(&segment_refs).await?
                }
                Err(_) => None,
            },
        };

        let Some(document) = document else {
            // `from` was never tracked as a document (e.g. a brand-new file
            // moved in before its first ingest). Drop any stale from-state and
            // ingest `to` as a fresh file — the standard discovery path.
            self.forget_file_state(&from_canon);
            info!(
                "[FileSyncController] Rename {} -> {}: source had no known document; ingesting                  the destination as a new file",
                from.display(),
                to.display()
            );
            return self.on_file_changed(to).await;
        };
        let document_uri = document.id.clone();

        info!(
            "[FileSyncController] Atomic rename {} -> {}: re-homing document {} (no delete window)",
            from.display(),
            to.display(),
            document_uri,
        );

        // Migrate per-file tracking state (echo-suppression, hashes, base
        // source, quarantine) from the old path to the new one.
        self.migrate_file_state(&from_canon, &to_canon);

        // Re-point the alias so `inv-every-page-has-its-own-file` and every
        // file-tracking consumer resolve the doc to its NEW home immediately.
        if let Some(ref registrar) = self.alias_registrar {
            registrar.register_alias(&document_uri, to).await;
        }

        // Reconcile the destination bytes into the store (children edits,
        // header) and stamp `last_projection[to]`. The bytes are unchanged by a
        // pure move, so echo-suppression usually short-circuits this — the doc
        // stays alive throughout, never passing through a deleted state. Done
        // BEFORE the retitle so the retitle is the LAST write and always wins,
        // even when a rename coincides with a content edit that re-ingests.
        self.on_file_changed(to).await?;

        // File-move spec (D2): a document page's title FOLLOWS its file name.
        // Retitle the doc-root page to the new file stem through the SAME single
        // org->block write seam a normal update uses (the heal path's mechanism).
        // A no-op when the stem is unchanged or the file is not a page.
        if document.is_page() {
            if let Ok(rel) = to.strip_prefix(&self.root_dir) {
                let segments = path_to_name_chain(rel);
                if let Some(new_stem) = segments.last() {
                    if &document.content != new_stem {
                        let mut params = holon_api::StorageEntity::new();
                        params.insert("id".into(), Value::String(document_uri.to_string()));
                        params.insert(
                            "parent_id".into(),
                            Value::String(document.parent_id.to_string()),
                        );
                        params.insert("content".into(), Value::String(new_stem.clone()));
                        params.insert(
                            "content_type".into(),
                            Value::String(document.content_type.to_string()),
                        );
                        params.insert(
                            ROUTING_DOC_URI_KEY.into(),
                            Value::String(document_uri.to_string()),
                        );
                        self.ordering
                            .apply_ingest_batch(vec![("update".to_string(), params)])
                            .await
                            .map_err(|e| {
                                anyhow::anyhow!(
                                    "retitle-on-rename {} -> {} for {}: {e:#}",
                                    from.display(),
                                    to.display(),
                                    document_uri
                                )
                            })?;
                        if let Some(downstream) = &self.downstream {
                            downstream.flush().await.map_err(|e| {
                                anyhow::anyhow!("downstream flush after rename retitle: {e}")
                            })?;
                        }
                        info!(
                            "[FileSyncController] Rename retitled page {} to new file stem {:?}",
                            document_uri, new_stem
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Handle a file change event from the FileWatcher.
    ///
    /// Thin write-back-quarantine wrapper around
    /// [`ingest_file`](Self::ingest_file): a partial ingest (an `Err`)
    /// records the file in `quarantined` so no write-back path re-renders
    /// its truncated DB state over disk (dogfood 2026-07-10 region
    /// data-loss guard). A successful ingest clears the quarantine. The
    /// `Err` is still propagated so the caller's degraded-mode
    /// banner / survival logic is unchanged.
    pub async fn on_file_changed(&mut self, path: &Path) -> Result<()> {
        let canonical = CanonicalPath::new(path);
        // Store-health seam (BugFunnel row 295): a runtime file-watch reingest of
        // a title-less doc-root heals it through the SAME single implementation
        // the boot sweep uses. Gated to post-boot edits — during the initial scan
        // the one-shot `heal_title_less_doc_roots` sweep runs once after the scan
        // instead, so per-file healing here would be redundant. Never in the
        // ingest fast-path, which certifies byte-identity only.
        if !self.in_initial_scan() {
            self.heal_title_less_doc_root(path).await?;
        }
        match self.ingest_file(path).await {
            Ok(()) => {
                if self.quarantined.remove(&canonical) {
                    self.quarantine_skip_logged
                        .lock()
                        .expect("quarantine_skip_logged poisoned")
                        .remove(&canonical);
                    info!(
                        "[FileSyncController] write-back quarantine CLEARED for {} (ingest fully \
                         succeeded)",
                        path.display()
                    );
                }
                Ok(())
            }
            Err(e) => {
                // Partial ingest: the DB now holds only a PREFIX of this file's
                // blocks. Quarantine it so write-back never renders that prefix
                // over the intact on-disk file. Loud + disclosed.
                if self.quarantined.insert(canonical.clone()) {
                    // New quarantine episode: re-arm the once-per-episode
                    // skip-log so the first write-back skip is loud again.
                    self.quarantine_skip_logged
                        .lock()
                        .expect("quarantine_skip_logged poisoned")
                        .remove(&canonical);
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
    /// prefix over the intact on-disk file. See
    /// [`quarantined`](Self::quarantined).
    fn is_quarantined(&self, path: &Path) -> bool {
        let canonical = CanonicalPath::new(path);
        if self.quarantined.contains(&canonical) {
            let first_skip = self
                .quarantine_skip_logged
                .lock()
                .expect("quarantine_skip_logged poisoned")
                .insert(canonical);
            if first_skip {
                tracing::error!(
                    path = %path.display(),
                    "[FileSyncController] SKIPPING write-back for quarantined file — its last \
                     ingest failed partway, so the DB holds only a truncated prefix of its \
                     blocks; rendering it over disk would DESTROY the un-ingested lines. \
                     The on-disk file is left intact until a clean re-ingest clears the \
                     quarantine. (Further skips of this file log at debug.)",
                );
            } else {
                tracing::debug!(
                    path = %path.display(),
                    "[FileSyncController] write-back skipped again for quarantined file",
                );
            }
            true
        } else {
            false
        }
    }

    /// The bare `#+ID` (document identity) declared by the folder-companion
    /// file for a directory `rel_dir`, if one exists on disk. The companion for
    /// a directory `Areas/` is the sibling file `Areas.<ext>` next to it. Read
    /// through the format adapter so this is format-agnostic (org's `#+ID:`,
    /// etc.). Returns `Ok(None)` when there is no companion, or the companion
    /// carries no explicit id (a name-chain-only page).
    async fn companion_doc_id(&self, rel_dir: &str) -> Result<Option<String>> {
        for ext in self.format.extensions() {
            let candidate = self.root_dir.join(format!("{rel_dir}.{ext}"));
            if self.fs.exists(&candidate) {
                let content = self
                    .fs
                    .read_to_string(&candidate)
                    .await
                    .with_context(|| format!("read companion {}", candidate.display()))?;
                if let Some(bare) = self.format.doc_id_from_content(&content) {
                    return Ok(Some(bare));
                }
            }
        }
        Ok(None)
    }

    /// Resolve (creating as needed) the page for each segment of a name chain,
    /// returning the leaf page. Companion-aware replacement for the doc
    /// manager's `get_or_create_by_name_chain`: when a directory segment has no
    /// page yet, it ADOPTS the segment's folder-companion `#+ID` as the page
    /// identity instead of minting a path-derived placeholder.
    ///
    /// This makes folder-companion reconciliation ORDER-INDEPENDENT (F5,
    /// dogfood 2026-07-22). A child ingested before its folder companion used
    /// to mint a `PageId::for_path(segment)` phantom container; the companion
    /// then landed under its own authoritative `#+ID` via `create_forcing_id`,
    /// leaving TWO pages for the same folder — one owning the subtree, one
    /// childless. Adopting the companion's `#+ID` up front means whoever
    /// ingests first creates the page under the id the companion resolves to,
    /// so no phantom is ever produced. When the companion has no `#+ID` (or no
    /// companion file exists) the deterministic `PageId::for_path` id is used —
    /// an org page and a `[[link]]`-created page for the same path still
    /// converge on one merge key.
    async fn resolve_dir_page_chain(&self, chain: &[&str]) -> Result<Block> {
        assert!(!chain.is_empty(), "name chain must not be empty");

        let mut current_parent_id = EntityUri::no_parent();
        let mut current_doc: Option<Block> = None;
        let mut accumulated = String::new();

        for segment in chain {
            accumulated = if accumulated.is_empty() {
                segment.to_string()
            } else {
                format!("{accumulated}/{segment}")
            };

            // Adopt the companion `#+ID` when present; else the deterministic
            // path-derived id. Computed even when a page already exists so a
            // divergent claim can be disclosed loudly (never silently picked).
            let companion_id = self
                .companion_doc_id(&accumulated)
                .await?
                .map(|bare| EntityUri::block(&bare));
            let path_id = holon_api::link_parser::PageId::for_path(&accumulated)
                .map_err(anyhow::Error::msg)?
                .into_entity_uri();
            let intended_id = companion_id.clone().unwrap_or_else(|| path_id.clone());

            match self
                .doc_manager
                .find_by_parent_and_name(&current_parent_id, segment)
                .await?
            {
                Some(existing) => {
                    // A page already claims this (parent, title). If a companion
                    // file claims a DIFFERENT authoritative id, two roots
                    // genuinely contend for this folder — disclose both ids
                    // loudly (fail-loud philosophy) rather than silently
                    // adopt-by-guess. Keep the existing page so we don't mint a
                    // third; a one-time dedup migration reconciles legacy rows.
                    if let Some(comp) = &companion_id {
                        if comp != &existing.id {
                            tracing::warn!(
                                folder = %accumulated,
                                existing_page_id = %existing.id,
                                companion_id = %comp,
                                "two roots claim the same folder page: an existing page and the \
                                 folder-companion `#+ID` disagree. Keeping the existing page; the \
                                 companion's `#+ID` is NOT adopted here (would orphan the existing \
                                 subtree). Reconcile with a dedup migration."
                            );
                        }
                    }
                    current_parent_id = existing.id.clone();
                    current_doc = Some(existing);
                }
                None => {
                    let mut new_doc = Block::new_text(
                        intended_id,
                        current_parent_id.clone(),
                        segment.to_string(),
                    );
                    new_doc.set_page(true);
                    // `create_forcing_id`: the adopted companion `#+ID` (or the
                    // deterministic path id) IS this page's identity — never
                    // substitute a same-`(parent,title)` row minted elsewhere.
                    let created = self.doc_manager.create_forcing_id(new_doc).await?;
                    current_parent_id = created.id.clone();
                    current_doc = Some(created);
                }
            }
        }

        Ok(current_doc.unwrap())
    }

    /// Resolve the AUTHORITATIVE owning document of a block by walking its
    /// `parent_id` chain in the write authority (`block_raw` / the Loro tree,
    /// via `get_block_authoritative`) up to the nearest `Page` — the bf071003
    /// pattern, never the lagging matview. `Ok(None)` when the id (or an
    /// ancestor) is absent from the authority: a brand-new / id-less / unknown
    /// block, which is normal ingest and must be left untouched. Depth-bounded.
    async fn resolve_authoritative_doc(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
        let mut cursor = id.clone();
        for _ in 0..50 {
            let Some(block) = self.block_reader.get_block_authoritative(&cursor).await? else {
                return Ok(None);
            };
            if block.is_page() {
                return Ok(Some(block.id));
            }
            cursor = block.parent_id;
        }
        Ok(None)
    }

    /// Echo suppression: if disk content matches last_projection, skip.
    /// Otherwise, diff against last_projection to compute create/update/delete
    /// ops.
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

        // Inc 3 (Model.md invariant 11): a shared-subtree org file is a one-way
        // PROJECTION SINK, rendered FROM the shared Loro doc (which converges
        // across devices over iroh). Re-ingesting it as fresh global intent
        // would duplicate the shared blocks into the GLOBAL doc — colliding
        // with the shared-doc→SQL projection (`first_local_collision` would then
        // refuse the honest projection).
        //
        // The skip decision keys on AUTHORITATIVE state (a real mount node in
        // the global Loro tree, via `mount_registry`), NOT on parsed drawer
        // content: `:share-role: mount:` / `:shared-tree-id:` round-trip
        // verbatim from ANY user file, so skipping on content alone would let a
        // hand-authored file be silently dropped (a page that never loads).
        // Content is only a cheap PRE-FILTER; the registry is the authority. No
        // registry (SqlOnly / tests) ⇒ never skip. A cheap substring pre-filter
        // keeps normal files off the parse path — it matches EITHER marker
        // (`share-role` on the mount, `shared-tree-id` stamped on descendants),
        // since a page share's mount drawer does not always round-trip while its
        // descendants' `shared-tree-id` always does.
        let lc = disk_content.to_ascii_lowercase();
        if lc.contains("share-role") || lc.contains("shared-tree-id") {
            let probe =
                self.format
                    .parse(path, &disk_content, &EntityUri::no_parent(), &self.root_dir)?;
            if is_shared_subtree_projection(&probe.document, &probe.blocks) {
                let registered = match &self.mount_registry {
                    Some(reg) => reg.is_registered_mount(&probe.document.id).await?,
                    None => false,
                };
                if registered {
                    tracing::warn!(
                        path = %path.display(),
                        page_id = %probe.document.id,
                        "[FileSyncController] Model.md invariant 11: registered shared-subtree \
                         projection file — SKIPPING ingest. Its truth is the shared Loro doc \
                         (converges over iroh); the org file is a one-way projection sink, so \
                         re-ingesting it would duplicate the shared blocks into the global doc."
                    );
                    // Stamp last_projection so in-session echo-suppression treats
                    // the file as up to date (it IS our own projection output).
                    self.last_projection
                        .insert(canonical.clone(), disk_content.clone());
                    return Ok(());
                }
                // Content looks like a mount but the page id is NOT a registered
                // mount — a hand-authored / imported / templated `share-role`
                // drawer. Disclosed, then ingested as a normal file (never
                // silently dropped).
                tracing::warn!(
                    path = %path.display(),
                    page_id = %probe.document.id,
                    "[FileSyncController] a `share-role` drawer property was found on a page that \
                     is NOT a registered shared-subtree mount — ingesting it as a normal file."
                );
            }
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
            //
            // This predicate certifies ONLY byte-identity + store presence. It
            // deliberately does NOT reason about store HEALTH (e.g. a title-less
            // doc-root) — encoding a specific degradation here would leak the
            // next degradation class through the same skip. Store-health repair
            // is a separate, unconditional concern owned by
            // `heal_title_less_doc_roots` (boot sweep) + the file-watch heal seam.
            if stored == &disk_hash && self.content_present_in_all_stores(&disk_content).await? {
                debug!(
                    "[FileSyncController] Skipping {} — disk hash matches stored \
                     file.content_hash and content present in all active stores (cold-boot fast \
                     path)",
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
        // Filename-derived page title: the last path segment with the extension
        // stripped — the SAME default `parse_org_file` applies when a file has
        // no `#+TITLE:`. A `Page` must never carry empty content.
        let filename_title = segments
            .last()
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        // A title-less (empty-content) `Page` doc-root left by a broken
        // convert/delete is NOT repaired here — the byte-identity fast-path above
        // skips an unchanged degraded file before this point, so healing in ingest
        // would be unreachable exactly for the case that needs it. Repair is the
        // store-health seam's job (`heal_title_less_doc_roots` boot sweep +
        // the file-watch heal in `on_file_changed`), which runs before/around this
        // ingest, so the doc resolved here is already healed in the store.
        // `doc_was_created` (R3b): whether THIS ingest minted the doc-page (vs
        // resolved an existing one). Computed from the resolution itself —
        // BEFORE `create_forcing_id`/`resolve_dir_page_chain` materialise the row
        // — so a re-ingest/edit of an existing doc never counts as a create.
        let (document, doc_was_created) = match bare_id_in_file.as_deref() {
            Some(bare) => {
                let id = EntityUri::block(bare);
                match self.doc_manager.get_by_id(&id).await? {
                    Some(doc) => (doc, false),
                    None => {
                        let parent_id = if segments.len() > 1 {
                            let parent_segments: Vec<&str> =
                                segment_refs[..segments.len() - 1].to_vec();
                            self.resolve_dir_page_chain(&parent_segments).await?.id
                        } else {
                            EntityUri::no_parent()
                        };
                        let mut new_doc = Block::new_text(id, parent_id, filename_title.clone());
                        new_doc.set_page(true);
                        // FORCE the `#+ID` as the page identity. A sibling file
                        // scanned earlier under a same-named subdirectory (e.g.
                        // `Frontends/GPUI.org` next to `Frontends.org`) mints a
                        // random-id name-chain placeholder page for the shared
                        // `Frontends` segment; a plain `create` would de-dup by
                        // `(parent, title)` and hand that placeholder's id back,
                        // so writeback would re-mint this file's `#+ID` (data
                        // loss). `create_forcing_id` keeps the authoritative id.
                        (self.doc_manager.create_forcing_id(new_doc).await?, true)
                    }
                }
            }
            // No `#+ID`: name-chain-derived identity. A genuine NEW doc-page iff
            // the store has no page for this name chain BEFORE
            // `resolve_dir_page_chain` create-if-absents it.
            None => {
                let existed = self
                    .doc_manager
                    .find_by_name_chain(&segment_refs)
                    .await?
                    .is_some();
                (self.resolve_dir_page_chain(&segment_refs).await?, !existed)
            }
        };
        let document_uri = document.id.clone();

        // R3b (doc-ingest history): record ONE `block_history` op_group for a
        // genuinely-new doc/day PAGE created by RUNTIME org-ingest (a user
        // CreateDocument / external new file), so the C2 provenance floor
        // (`inv-history-records-all-creates`) covers ingest creates, not only
        // engine-routed ones. Cold-boot scan is excluded (`in_initial_scan`) so a
        // vault load never floods history. Ingest-origin: recorded, never on the
        // undo stack (undo-reach of ingest ops is a separate item).
        let doc_page_is_new_runtime =
            doc_was_created && !self.in_initial_scan() && self.history.is_some();

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

        if doc_page_is_new_runtime {
            let history = self
                .history
                .as_ref()
                .expect("doc_page_is_new_runtime implies a wired history store");
            history
                .record(holon_api::HistoryEvent::create_event(
                    "block",
                    document_uri.as_str(),
                    &holon_api::OpOrigin::Ingest,
                    self.clock.now_millis(),
                ))
                .await
                .with_context(|| format!("record ingest doc-create history for {document_uri}"))?;
        }

        // Register UUID → file path alias (if Loro is available)
        if let Some(ref registrar) = self.alias_registrar {
            registrar.register_alias(&document_uri, path).await;
        }
        // EROFS row 346: this is the SOLE resume trigger. A successful
        // `ingest_file` of this file (runtime re-ingest via `on_file_changed`,
        // or a boot re-scan) reaches HERE only after the doc's identity was
        // resolved and its alias re-registered — proof the path is
        // writable-backed again — so lift any prior read-only write-back skip
        // and edits resume. Pure relay/synthetic docs that are never ingested
        // never reach this point, and intentionally stay skipped (there is no
        // writable file to resume to).
        if self.writeback_readonly.remove(&CanonicalPath::new(path)) {
            tracing::info!(
                path = %path.display(),
                "[FileSyncController] read-only write-back skip CLEARED \
                 (file re-ingested with a writable backing path)",
            );
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

        let mut new_blocks_vec = new_parse.blocks;

        // ── ID-less headline reconciliation (external re-edit duplicate guard) ──
        // An org headline with no `:ID:` is minted a FRESH `Uuid::new_v4()` on
        // EVERY parse (`parser::extract_or_generate_id`). So the classic
        // external-editor workflow — write an ID-less headline, let the app
        // ingest + write it back (minting id A), then write a STALE pre-mint copy
        // of the same text — re-parses that headline to a DIFFERENT id B, and the
        // by-id diff below sees `B ∉ old_blocks` → CREATE. In the steady state the
        // delete pass then removes the orphaned twin A (a churned identity, but no
        // duplicate); under a CONCURRENT external re-write the writeback of the
        // minted `:ID:`s is skipped by the TOCTOU guard, so `last_projection`
        // (hence the diff base `old_blocks`) desyncs from the store and the twin
        // survives — the block DUPLICATES under two ids (observed at ~60-block
        // scale on the live vault; see BugFunnel 2026-07-22 / PR #76).
        //
        // Remedy: before the id-keyed diff runs, remap an ID-less incoming block
        // (`blocks_needing_ids`) onto its already-minted twin when that twin sits
        // under the SAME parent at the SAME sibling position with EXACTLY equal
        // content, so the stale re-write reconciles as an idempotent update
        // instead of a re-mint (which churns identity or, on base desync,
        // duplicates). We match against the STORE's CURRENT children (ground
        // truth), NOT the diff base `old_blocks`: the base is parsed from
        // `last_projection` and desyncs precisely in the duplicating case, holding
        // throwaway ids that match neither the real twin nor the new mint — so
        // matching the base cannot find the twin and the duplicate still lands.
        // Deterministic + conservative: positional 1:1, so two genuinely-distinct
        // ID-less siblings with identical content stay two blocks; a content match
        // at a DIFFERENT position is disclosed (WARN) and left to mint rather than
        // guessed into a merge.
        // Inc 3: set when the id-less reconcile binds id-less blocks onto EXISTING
        // store ids (remaps). Such a bind is a pure UPDATE (no create/delete), so
        // `has_structural_changes` stays false and the UPDATE-only fast path below
        // would return WITHOUT re-rendering -- leaving the minted `:ID:` drawers off
        // disk forever (the Inc 0 guard-(c) echo loop). Forcing the round-trip here
        // stamps them, mirroring `needs_id_writeback` for the doc `#+ID`.
        let mut needs_block_id_writeback = false;
        if !new_parse.blocks_needing_ids.is_empty() {
            let existing_children = self
                .block_reader
                .get_blocks(&document_uri)
                .await
                .with_context(|| {
                    format!("read store children for ID-less reconcile (doc {document_uri})")
                })?;

            let minted: HashSet<&str> = new_parse
                .blocks_needing_ids
                .iter()
                .map(String::as_str)
                .collect();

            // `seq` is the block's DOCUMENT-ORDER position within its parent —
            // the tier tiered_match's T1 (content-at-same-position) keys on.
            // `get_blocks` already returns children in canonical document order
            // (`block_raw ORDER BY sort_key, id`), so the position is a
            // per-parent running index over that ordered list. It is NOT the
            // `"sequence"` PROPERTY: the org parser stamps that on every
            // parsed block from a DOCUMENT-GLOBAL DFS counter, while blocks
            // created in-app afterwards (splits, creates) carry none and
            // `OrgBlockExt::sequence()` collapses them to 0. Sorting by it
            // therefore interleaves minted blocks ahead of their parsed
            // siblings, making T1 match id-less incoming blocks against the
            // WRONG existing twin — churning a stale re-edit's identity
            // (MintAmbiguous) or remapping onto a mis-positioned sibling
            // (sibling-order swap in the org projection). Mirrors the oracle's
            // per-parent document-order
            // `seq` in `stale_external_rewrite::apply_to_ref`.
            let mut existing_seq_per_parent: HashMap<EntityUri, i64> = HashMap::new();
            let existing: Vec<ExistingChild> = existing_children
                .iter()
                .map(|b| {
                    let counter = existing_seq_per_parent
                        .entry(b.parent_id.clone())
                        .or_insert(0);
                    let seq = *counter;
                    *counter += 1;
                    ExistingChild {
                        id: b.id.clone(),
                        parent: b.parent_id.clone(),
                        seq,
                        content: b.content.clone(),
                    }
                })
                .collect();

            // Top-level headlines parse with `parent_id == new_parse.document.id`;
            // the store keys the same children under `document_uri` — normalise so
            // they group together for positional matching.
            let incoming: Vec<IncomingIdentity> = new_blocks_vec
                .iter()
                .map(|b| IncomingIdentity {
                    id: b.id.clone(),
                    parent: if b.parent_id == new_parse.document.id {
                        document_uri.clone()
                    } else {
                        b.parent_id.clone()
                    },
                    content: b.content.clone(),
                    minted: minted.contains(b.id.id()),
                })
                .collect();

            let situation = detect_match_situation(&existing, &incoming);
            let verdicts = self
                .block_matcher
                .match_blocks(MatchContext {
                    document_uri: &document_uri,
                    existing: &existing,
                    incoming: &incoming,
                    situation,
                })
                .await
                .with_context(|| {
                    format!("block-match strategy for ID-less reconcile (doc {document_uri})")
                })?;
            let remaps: HashMap<EntityUri, EntityUri> = verdicts
                .iter()
                .filter_map(|v| match v {
                    MatchVerdict::Remap { minted, onto, .. } => {
                        Some((minted.clone(), onto.clone()))
                    }
                    _ => None,
                })
                .collect();
            // Provenance: the basis each remap was decided on (WARN/debug trail).
            let basis_by_minted: HashMap<EntityUri, MatchBasis> = verdicts
                .iter()
                .filter_map(|v| match v {
                    MatchVerdict::Remap { minted, basis, .. } => Some((minted.clone(), *basis)),
                    _ => None,
                })
                .collect();
            needs_block_id_writeback = !remaps.is_empty();

            if !remaps.is_empty() {
                // Apply id + parent remaps IN PLACE. A child of a remapped ID-less
                // headline has `parent_id == <old minted parent>` (a remap key), so
                // rewriting parent links here reparents it onto the existing twin.
                for block in new_blocks_vec.iter_mut() {
                    if let Some(existing_id) = remaps.get(&block.parent_id) {
                        block.parent_id = existing_id.clone();
                    }
                    if let Some(existing_id) = remaps.get(&block.id) {
                        debug!(
                            "[FileSyncController] reconciled ID-less headline onto its \
                             already-minted store twin (basis {:?}) (file {}, headline {:?}): {} \
                             -> {}",
                            basis_by_minted.get(&block.id),
                            path.display(),
                            block.content,
                            block.id,
                            existing_id,
                        );
                        block.id = existing_id.clone();
                        // Keep the flat `ID` property consistent with the remapped
                        // id (build_block_params falls back to `block.id.id()`, but
                        // a future reader of the parse block must not see the stale
                        // mint).
                        block.set_property("ID", Value::String(existing_id.id().to_string()));
                    }
                }
            }
            for verdict in &verdicts {
                let MatchVerdict::MintAmbiguous {
                    minted: minted_id,
                    candidates,
                } = verdict
                else {
                    continue;
                };
                let headline = new_blocks_vec
                    .iter()
                    .find(|b| &b.id == minted_id)
                    .map(|b| b.content.as_str())
                    .unwrap_or("");
                // Content matched existing store blocks but not uniquely in the
                // document (multiple twins, or the incoming side is duplicated) —
                // not deterministically resolvable. Mint rather than guess a
                // merge, disclosed with the candidate ids for auditing.
                warn!(
                    "[FileSyncController] ID-less headline content matches existing store \
                     block(s) but not uniquely in the document — minting a fresh id rather than \
                     guessing a merge (external-edit dup guard). file={}, headline={:?}, \
                     minted_id={}, candidates={:?}",
                    path.display(),
                    headline,
                    minted_id,
                    candidates,
                );
            }
        }

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
                "[FileSyncController] Re-parenting {} blocks from other documents to {} (blocks \
                 exist under different doc URIs, e.g. from seed_default_layout). File: {}",
                conflicts.len(),
                document_uri,
                path.display(),
            );
        }

        // Foreign PAGE doc-root protection (dogfood 2026-07-12). A block that is
        // CURRENTLY a `Page` owned by a DIFFERENT page-file is authoritatively
        // that page. A folder-companion / aggregating file (e.g. `Journals.org`)
        // that inlines the page-file's doc-root as a plain heading must NOT
        // create, update, re-parent, or delete it — above all it must not strip
        // its `Page` tag. The page-file stays the SOLE authority for the page's
        // identity, content, tags, and placement; letting the companion write
        // would be a silent last-writer-wins between two on-disk representations
        // of the SAME logical page (SqlOnly: the `Page` tag is stripped; Loro:
        // the re-`create_in_tree` of an already-rooted id never lands under the
        // companion's parent and the whole ingest times out + quarantines).
        //
        // Contract (externally visible): the `Page` tag stays truthful — a
        // foreign file inlining a doc-root as a heading cannot demote it. The
        // discriminator is `doc_manager.get_by_id`, which reads the Page matview
        // (`tag='Page'`) — the tag IS the page-authority signal, no second
        // ownership/path predicate leaks to any consumer.
        //
        // Why not `find_foreign_blocks`: its `blocks_by_document` attribution
        // CANNOT see a doc-root (page blocks are excluded from every document's
        // descendant list, and a page is never a member of its own document), so
        // the create-path conflict re-parent above never fires for exactly this
        // topology. The Page matview answers "is this id a page RIGHT NOW"
        // authoritatively regardless of that attribution gap.
        let mut foreign_page_ids: std::collections::HashSet<EntityUri> =
            std::collections::HashSet::new();
        for block in &new_blocks_vec {
            if block.id == document_uri || block.id == new_parse.document.id {
                continue;
            }
            if self.doc_manager.get_by_id(&block.id).await?.is_some() {
                foreign_page_ids.insert(block.id.clone());
            }
        }
        if !foreign_page_ids.is_empty() {
            info!(
                "[FileSyncController] Skipping {} foreign PAGE doc-root(s) inlined as headings in \
                 {} — the owning page-file stays authoritative for their Page identity (no demote \
                 / re-parent): {:?}",
                foreign_page_ids.len(),
                path.display(),
                foreign_page_ids,
            );
        }

        // File-authority extends to the WHOLE inlined subtree, not just the
        // page root (real-vault cold-boot escape, 2026-07-12: a folder-companion
        // `Journals.org` inlining 3 page-files' roots + their 14 descendants
        // re-parented those descendants into itself, and the ingest gate then
        // expected blocks that `get_blocks`'s Page-boundary walk can NEVER
        // return — permanent count-check failure, file quarantined, retry
        // flood). Every parsed block whose parsed parent chain passes through a
        // foreign page root belongs to that page's own document: never create,
        // update, re-parent, or place it from this file. `new_blocks_vec` is
        // DFS document order (parents before children), so one forward pass
        // reaches the transitive closure.
        let mut foreign_subtree_ids = foreign_page_ids.clone();
        for block in &new_blocks_vec {
            if foreign_subtree_ids.contains(&block.parent_id) {
                foreign_subtree_ids.insert(block.id.clone());
            }
        }
        if foreign_subtree_ids.len() > foreign_page_ids.len() {
            info!(
                "[FileSyncController] Skipping {} descendant block(s) of foreign page root(s) \
                 inlined in {} — the owning page-file(s) stay authoritative for their subtrees.",
                foreign_subtree_ids.len() - foreign_page_ids.len(),
                path.display(),
            );
        }

        // Blocks the post-ingest gate must NOT expect from `get_blocks(doc)`:
        // its recursive walk stops at `Page`-tagged boundaries, so the skipped
        // foreign subtrees AND any parsed block that itself carries a `Page`
        // tag (plus its parsed descendants) are structurally invisible to the
        // doc walk even when their rows land. Counting them made the gate
        // unsatisfiable and quarantined the file forever.
        // Cross-doc-membership guard — arm (b) of the journals phantom
        // (on-disk STALE cross-doc copy). A block parsed into THIS file whose
        // AUTHORITATIVE routing (`block_raw` Page-walk, never a matview) lands
        // under a DIFFERENT document is a stale copy left on disk by a past
        // mis-route / crash / external edit. The matview-based
        // `find_foreign_blocks` re-parent above would ADOPT it (author a Move
        // into this file's doc); the day-page then re-adopts on its next
        // writeback and the org fixed-point oscillates forever. Instead: never
        // adopt it (fold into the skip set so no create/update/place/gate pass
        // touches it), disclose loudly (block + both docs), and let THIS file's
        // own honest re-render — which reads `block_raw` and routes the block
        // back to its real owner — PRUNE it from disk (sanctioned below so the
        // writeback-lossless guard does not read the prune as data loss).
        //
        // Fail-loud, never fake, never touches USER content: only fires when the
        // id ALREADY exists in the store (`get_block_authoritative` = `Some`)
        // under a resolvable page that is NOT this file's doc. An id-less /
        // brand-new / unknown block resolves to `None` → normal ingest. Foreign
        // PAGE inlines (`foreign_subtree_ids`) are the de-inline workstream's
        // concern (deferred, not pruned) and are excluded here.
        let mut stale_cross_doc_ids: HashSet<EntityUri> = HashSet::new();
        for block in &new_blocks_vec {
            if block.id == document_uri
                || block.id == new_parse.document.id
                || foreign_subtree_ids.contains(&block.id)
            {
                continue;
            }
            if let Some(auth_doc) = self.resolve_authoritative_doc(&block.id).await? {
                if auth_doc != document_uri && auth_doc != new_parse.document.id {
                    tracing::warn!(
                        block_id = %block.id,
                        ingesting_doc = %document_uri,
                        authoritative_doc = %auth_doc,
                        path = %path.display(),
                        "[FileSyncController] cross-doc membership: a block parsed into this \
                         file is authoritatively owned by a DIFFERENT document (block_raw \
                         routing) — NOT adopting the stale on-disk copy; pruning it from this \
                         file's writeback so it converges to its real owner."
                    );
                    stale_cross_doc_ids.insert(block.id.clone());
                }
            }
        }
        // Fold into the skip set so every create/update/place/gate pass leaves
        // these blocks untouched (identical handling to a foreign page subtree).
        foreign_subtree_ids.extend(stale_cross_doc_ids.iter().cloned());
        // String ids for the writeback-lossless sanctioned-removals seam
        // (`as_str()` form, matching the guard's `block.id.as_str()` compare).
        let stale_removals: HashSet<String> = stale_cross_doc_ids
            .iter()
            .map(|u| u.as_str().to_string())
            .collect();
        let mut gate_excluded_ids = foreign_subtree_ids.clone();
        for block in &new_blocks_vec {
            if block.id == document_uri || block.id == new_parse.document.id {
                continue;
            }
            if block.is_page() || gate_excluded_ids.contains(&block.parent_id) {
                gate_excluded_ids.insert(block.id.clone());
            }
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
            // A foreign page subtree is not placed in THIS file's tree, so it
            // must not anchor a later sibling's `after_block_id` — skip it so the
            // cursor stays on the previous real sibling.
            if foreign_subtree_ids.contains(&block.id) {
                continue;
            }
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
            // Foreign page subtree: owned by another page-file, inlined here as
            // headings. Never create/re-seed/re-parent it (root: that is the
            // demote; descendants: that is the steal).
            if foreign_subtree_ids.contains(&block.id) {
                continue;
            }
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
            // Foreign page subtree inlined as headings: the owning page-file is
            // authoritative — never emit an update that would strip the root's
            // `Page` tag, rewrite identities/parents, or clobber descendant
            // content.
            if foreign_subtree_ids.contains(id) {
                continue;
            }
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
                // Never delete a foreign page doc-root. If a companion file
                // stops inlining a page's heading, that page still lives in its
                // own page-file — its deletion is that file's concern, not ours.
                // (`foreign_page_ids` is built from the NEW parse, so an id that
                // vanished from this file isn't in it — re-check the Page matview.)
                if *id != document_uri && self.doc_manager.get_by_id(id).await?.is_some() {
                    info!(
                        "[FileSyncController] NOT deleting {} on ingest of {} — it is a Page \
                         doc-root owned by its own page-file (was inlined here).",
                        id,
                        path.display(),
                    );
                    continue;
                }
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
        // flush. `gate_excluded_ids` (foreign page subtrees + Page-tagged parse
        // blocks) are structurally invisible to `get_blocks`'s Page-boundary
        // walk, so expecting them made the gate unsatisfiable (2026-07-12
        // quarantined-vault escape).
        // The blocks expected present in `block_raw` (site-A cache) after this
        // apply: every parsed block that is NEITHER gate-excluded (foreign page
        // subtrees + Page-tagged parse blocks — structurally invisible to
        // `get_blocks`'s Page-boundary walk) NOR consolidator-persisted (their
        // sink rows flush downstream at site B). Computed with the SAME double
        // predicate as `expected_present_ids` below so the count and the id-set
        // never disagree. NB the two sets OVERLAP (a newly-ingested `:Page:`-tagged
        // child is BOTH gate-excluded AND a consolidator create), so the former
        // `count(non-gate-excluded) - consolidator_creates` underflowed (subtracting
        // a create that was never in the count) — the row-137 subdir fileless
        // journals topology tripped exactly that. This double-filter cannot
        // underflow and equals `expected_present_ids.len()` by construction.
        let expected_block_count = new_blocks_vec
            .iter()
            .filter(|b| !gate_excluded_ids.contains(&b.id))
            .filter(|b| !consolidator_create_ids.contains(&b.id.to_string()))
            .count();
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
            // One batched apply per file: in SqlOnly mode the whole op-vector is
            // one `db_handle.transaction()`, so the live-watch matview IVM
            // maintenance runs once per file instead of once per block (the O(N²)
            // cold-boot ingest, BugFunnel row 32). In Loro mode `apply_ingest_batch`
            // falls back to the per-op seam (Loro owns order). Document order is
            // preserved — the vector is already creates→updates→deletes in
            // parse order, and rows-then-edges + deferred FK settle parents at
            // COMMIT regardless of intra-batch row order.
            self.ordering
                .apply_ingest_batch(operations)
                .await
                .map_err(|e| anyhow::anyhow!("apply_ingest_batch for {}: {e:#}", path.display()))?;
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
                .filter(|b| !gate_excluded_ids.contains(&b.id))
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
                let present: HashSet<&str> = cached_blocks.iter().map(|b| b.id.as_str()).collect();
                let missing: Vec<&String> = expected_present_ids
                    .iter()
                    .filter(|id| !present.contains(id.as_str()))
                    .take(10)
                    .collect();
                anyhow::bail!(
                    "[on_file_changed] doc walk (`get_blocks`) returned {} of {} expected blocks \
                     after ingest of {} (doc {}, feed_caught_up={}) — blocks failed to land under \
                     this document; first missing: {missing:?}",
                    cached_blocks.len(),
                    expected_block_count,
                    path.display(),
                    document_uri,
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
                            "[on_file_changed] new blocks did not appear in ordering for parent \
                             {parent_key} within 2s: missing {missing:?}; present children: \
                             {kids:?}"
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
                    // Foreign page subtree: it lives in its OWN page-file's tree,
                    // not this companion's — never place it here.
                    if foreign_subtree_ids.contains(&new_block.id) {
                        continue;
                    }
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
                                "[on_file_changed] block {} not found in live_children under {}: \
                                 {:?}",
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
                    // Foreign page subtree: owned by its own page-file — exclude
                    // from this companion's per-parent order (a `place_all` here
                    // would re-key it under the companion's parents).
                    if foreign_subtree_ids.contains(&new_block.id) {
                        continue;
                    }
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
                        "[on_file_changed] {consolidator_creates} block create(s) were persisted \
                         by a separate consolidator (create_in_tree returned true) but no \
                         downstream projection is wired — their sink rows would never be written. \
                         DI wiring bug."
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
                    "[on_file_changed] LiveData<Block> feed went quiescent with created id(s) \
                     still missing ({} expected) for {} — projection/CDC stalled",
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
        if !has_structural_changes
            && !needs_id_writeback
            && !needs_block_id_writeback
            && !did_text_merge
            && stale_cross_doc_ids.is_empty()
        {
            self.last_projection
                .insert(canonical.clone(), disk_content.to_string());
            self.persist_disk_hash_for(&canonical, rel_path, &disk_hash)
                .await;
            return Ok(());
        }

        // Foreign-page subtrees were skipped above, so a re-render from this
        // document's blocks would rewrite the file WITHOUT the inlined
        // page-owned headings — a silent de-inline of the user's file, which is
        // the writeback-side workstream's decision (de-inline + materialization),
        // not this ingest's. Defer the write-back: disk already reflects every
        // block this ingest processed (the ops came FROM this parse), so
        // recording it as the projection is sound. ALLOW(fallback): disclosed.
        if !foreign_subtree_ids.is_empty() && stale_cross_doc_ids.is_empty() && !did_text_merge {
            info!(
                "[FileSyncController] Deferring write-back of {} — it inlines {} block(s) owned \
                 by other page-files; rewriting now would de-inline them. Disk left as-is; DB \
                 state for this document is complete.",
                path.display(),
                foreign_subtree_ids.len(),
            );
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
            "[FileSyncController] BUG: Just created/updated {} blocks for doc_id={} but \
             render_file_by_doc_id returned empty for {}. This would wipe the file!",
            new_blocks.len(),
            document_uri,
            path.display(),
        );

        // Ingest→write-back data-loss guard (BugFunnel row 28, P0). `rendered`
        // is the re-projection of the blocks that ACTUALLY landed. If ingest
        // silently dropped blocks (e.g. an FK rollback that aborted part of the
        // file without surfacing an error), `rendered` is a TRUNCATED prefix and
        // writing it below would delete those lines from the user's file. Refuse
        // loudly when the projection lost a block present on disk; the `?`
        // propagates to `on_file_changed`, whose Err arm QUARANTINES the file so
        // no write-back path renders the truncated state over disk. A legal
        // canonical reformat / 3-way merge preserves every block and passes.
        //
        // ADR 0025: this ingest re-project is one of the two irreducibly
        // intent-less boundaries — it holds no op, so it grounds ONLY via the
        // file's own projection (no sibling union, no sanctioned removals).
        if let Err(lossy) = self.format.check_writeback_lossless(
            path,
            &disk_content,
            &rendered,
            &[],
            &stale_removals,
            &self.root_dir,
        ) {
            // Inc 3 carry-forward (risk-register #2). If the SOLE reason we left the
            // UPDATE-only fast path is `needs_block_id_writeback` (a pure id-less
            // reconcile that bound blocks onto existing store ids), the re-render
            // exists only to stamp `:ID:` drawers. For most files it is lossless and
            // converges inv-org-render-fixed-point. But a companion / round-trip-lossy
            // page (a heading the renderer models elsewhere) DROPS a block on re-render
            // -- the exact loss the fast path was avoiding. Rather than quarantine,
            // carry the re-stamp obligation forward: preserve disk EXACTLY as the
            // pre-Inc-3 fast path did. The lossless guard stays fully fatal for any
            // ingest with real structural changes -- only this pure re-stamp degrades.
            let restamp_only = needs_block_id_writeback
                && !has_structural_changes
                && !needs_id_writeback
                && !did_text_merge;
            if restamp_only {
                debug!(
                    "[FileSyncController] re-stamp round-trip for {} would drop a block on \
                     re-render (companion / round-trip-lossy page) — preserving disk and \
                     carrying the id re-stamp forward rather than quarantining.",
                    path.display(),
                );
                self.last_projection
                    .insert(canonical.clone(), disk_content.to_string());
                self.persist_disk_hash_for(&canonical, rel_path, &disk_hash)
                    .await;
                return Ok(());
            }
            return Err(lossy).with_context(|| {
                format!(
                    "[FileSyncController] REFUSING write-back of {} — ingest was lossy (see the \
                     INGEST DATA LOSS error). The on-disk file is left intact; the file is \
                     quarantined until a clean re-ingest.",
                    path.display()
                )
            });
        }

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
                        "[ORGSYNC_TOCTOU] {} disk changed during processing (parsed_len={} \
                         disk_now_len={}); skipping write-back, stamping last_projection with \
                         parsed content so next diff picks up the external delta.",
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
                "[FileSyncController] persist_file_hash failed for {} ({}): {} (in-memory hash \
                 updated; next boot will re-ingest)",
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
        // Fork B: a page starts its OWN document. When THIS delta upserts a block
        // that is authoritatively a `Page`, ensure its identity file exists even if
        // childless — INDEPENDENT of which document `resolve_doc_for_block` routed
        // this delta to. That router reads the block-feed, whose `is_page` can lag
        // the authoritative store (matview enrich), so a just-minted page can be
        // routed to its PARENT document (de-inlined there) and its own file never
        // written. The authoritative re-check + registry-free materialization below
        // closes that gap generally (rule-minted journal dates,
        // `convert_block_to_page` on a childless block). Idempotent (see the method).
        // Failures here are the SAME R11 class the doc-resolution guard below
        // absorbs (a prohibited topology fails loud inside the name chain): log
        // and fall through so this pre-flight cannot re-propagate what the guard
        // exists to bound.
        if let BlockDelta::Upsert(b) = delta {
            match self.block_reader.get_block_authoritative(&b.id).await {
                Ok(Some(auth)) if auth.is_page() => {
                    if let Err(e) = self.materialize_page_identity_file(&auth.id).await {
                        tracing::error!(
                            doc_id = %doc_id,
                            block_id = %auth.id,
                            error = %format!("{e:#}"),
                            "[FileSyncController] on_block_changed: page identity-file \
                             pre-flight failed — continuing with this document's normal \
                             write-back path.",
                        );
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::error!(
                        doc_id = %doc_id,
                        block_id = %b.id,
                        error = %format!("{e:#}"),
                        "[FileSyncController] on_block_changed: authoritative read for the \
                         page identity-file pre-flight failed — continuing with this \
                         document's normal write-back path.",
                    );
                }
            }
        }

        let path = match self.doc_id_to_path(doc_id).await {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(false),
            Err(e) => {
                // §3.1 Finding A / R11: name_chain failed loud (e.g. a
                // prohibited page-under-non-page topology). Do NOT swallow this
                // into a silent no-op — surface it, then skip only THIS block's
                // write so the sync loop keeps serving every other document.
                tracing::error!(
                    doc_id = %doc_id,
                    error = %format!("{e:#}"),
                    "[FileSyncController] on_block_changed: could not resolve doc to a \
                     page-file path (name_chain failed loud) — SKIPPING this document's \
                     write-back; all other documents continue.",
                );
                return Ok(false);
            }
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

        let (rendered, reseeded) = self.render_with_cache(doc_id, &path, delta).await?;

        let current_last = self
            .last_projection
            .get(&canonical)
            .map(|s| s.as_str())
            .unwrap_or("");

        if rendered == current_last {
            return Ok(true);
        }

        // Copy-on-write: keep a VIRTUAL seed layout doc (`block:__default__`)
        // off disk while its render still matches the pristine shipped asset.
        // A diverged render (a real user edit routed to the seed doc) falls
        // through and materializes the file — copy-on-write — after which the
        // on-disk file wins (`disk_content` non-empty ⇒ not gated). Race-free:
        // a late boot-seed delta renders == pristine and is never written.
        if self.gate_virtual_seed_write(doc_id, &canonical, &rendered, !disk_content.is_empty()) {
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

        // Fork B B1' / ADR 0025: mass-truncation tripwire on the block-driven path
        // (previously UNGUARDED — a P0 hole: this path could SILENTLY drop a large
        // chunk present on disk). Only a reseed can shrink the block set, so the
        // cheap content-only path skips the check (no per-keystroke re-parse).
        // Grounding = the delta's `Remove` id (a sanctioned deletion) UNION the
        // sibling files a de-inlined child page moved into; the tripwire vetoes
        // only on the row-28 mass-truncation signature (single/small drops pass
        // silently — cross-doc moves and matview-lag races still shrink a file
        // without a `Remove` op; see `tripwire_mass_truncation`). On veto the
        // file is quarantined and the Err propagates to `on_block_feed`
        // (di.rs), which logs it.
        if reseeded {
            let sanctioned_removals = match delta {
                BlockDelta::Remove(id) => HashSet::from([id.as_str().to_string()]),
                BlockDelta::Upsert(_) => HashSet::new(),
            };
            self.tripwire_mass_truncation(&path, &disk_at_write, &rendered, &sanctioned_removals)
                .await?;
        }

        // EROFS row 346: skip-with-one-loud-error for a doc whose path has no
        // writable backing file — first failure discloses, later CDC events
        // skip (no per-event storm). A skip returns Ok(true) up-stack (the loop
        // survives) but does NOT stamp `last_projection`, so a later-writable
        // path re-attempts.
        if !self
            .write_back_or_skip_readonly(doc_id, &path, rendered.as_bytes())
            .await?
        {
            return Ok(true);
        }
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
    /// structure AND unchanged sibling position): refresh just that block via
    /// an authoritative `block_raw` point read (`get_block_authoritative`,
    /// O(1), no recursive CTE) and replace it in place, preserving sibling
    /// order. Everything else — cold doc, `Remove`, an id not yet cached
    /// (structural insert), a `parent_id` move, a `tags` change (H4: a `Page`
    /// toggle re-partitions the doc's subtree), or a same-parent REORDER
    /// (sort_key moved among unchanged siblings) — reseeds the whole doc via
    /// `get_blocks` (authoritative, `sort_key, id`-ordered). Structural intent
    /// is decided from the AUTHORITATIVE row, not the (matview-lagged) feed
    /// delta, so a structural change the delta didn't yet reflect still
    /// reseeds.
    ///
    /// The domain `Block` doesn't carry `sort_key` (ADR 0005), so a
    /// same-parent reorder is invisible to a `parent_id`/`tags` comparison
    /// alone — a block can change sibling position without either changing.
    /// Left unguarded, `IndexMap::insert` on an existing key keeps its OLD
    /// position, so the file would keep rendering the pre-reorder sibling
    /// order forever even though SQL/live reads already show the new one
    /// (bug: disk sibling order stale after a same-parent move). The live
    /// `ordering.children(parent)` read (O(siblings), not O(doc)) is compared
    /// against the cached sibling order to detect this before trusting the
    /// cheap path.
    /// Returns `(rendered, reseeded)`. `reseeded` is `true` when this render
    /// took the authoritative full-doc reseed (`get_blocks`) — the only
    /// path whose block set can SHRINK relative to disk (a de-inline, a
    /// delete, a `Page` re-partition). The cheap content-only path
    /// (`false`) refreshes one block in place and provably preserves the id
    /// set, so the write-back guard can skip it (Fork B B1' latency gate:
    /// no per-keystroke re-parse of disk).
    async fn render_with_cache(
        &mut self,
        doc_id: &EntityUri,
        path: &Path,
        delta: &BlockDelta,
    ) -> Result<(String, bool)> {
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
                    let cached_siblings: Vec<EntityUri> = self
                        .doc_blocks
                        .get(doc_id)
                        .expect("warm by cheap_incremental_candidate")
                        .values()
                        .filter(|blk| blk.parent_id == auth.parent_id)
                        .map(|blk| blk.id.clone())
                        .collect();
                    let live_siblings = self
                        .ordering
                        .children(&auth.parent_id)
                        .await
                        .map_err(|e| anyhow::anyhow!("ordering.children failed: {e}"))?;
                    if live_siblings == cached_siblings {
                        // Content-only, position-unchanged: `IndexMap::insert`
                        // on an existing key keeps its position, so sibling
                        // order is unchanged.
                        self.doc_blocks
                            .get_mut(doc_id)
                            .expect("warm by cheap_incremental_candidate")
                            .insert(auth.id.clone(), auth);
                        return Ok((self.render_cached_doc(doc_id, path).await?, false));
                    }
                    // Sibling order moved (a same-parent reorder) — fall
                    // through to the full reseed below.
                }
            }
        }

        self.reseed_doc_blocks(doc_id).await?;
        Ok((self.render_cached_doc(doc_id, path).await?, true))
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
                    "[FileSyncController] poll_external_changes: ingesting {} (disk != \
                     last_projection)",
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
            if self.last_projection.contains_key(&canonical) {
                continue;
            }

            // (mtime, size) signature — the ingest-quarantine key. A new file
            // whose ingest failed is quarantined AT this signature; it is
            // re-attempted only when the signature changes (the user editing
            // the file to fix it). Reused as the fault-containment key below.
            let sig = match self.fs.metadata(&path).await {
                Ok(m) => (m.modified, m.len),
                // Vanished between the scan and this stat (TOCTOU): nothing to
                // ingest. A later discovery tick re-observes it if it reappears.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("[poll_new_files] Cannot stat {}", path.display())
                    });
                }
            };

            // Still the exact poisoned bytes we already reported loudly once:
            // skip WITHOUT re-attempting or re-logging at ERROR, so one broken
            // file does not storm the log every discovery tick (mirrors the
            // write-back quarantine's once-per-episode disclosure).
            if self.ingest_quarantine.get(&canonical) == Some(&sig) {
                tracing::debug!(
                    path = %path.display(),
                    "[poll_new_files] skipping ingest-quarantined new file (unchanged since its \
                     failure was reported); re-attempted when its content changes",
                );
                continue;
            }

            info!(
                "[FileSyncController] poll_new_files: discovered new file {}",
                path.display()
            );

            // PER-FILE CONTAINMENT (Inc 3b). One poisoned org file must NOT
            // abort the whole discovery walk: propagating the `?` here left
            // every later healthy file un-ingested, and the next tick re-hit the
            // same poison first, so healthy files were NEVER ingested while the
            // poison persisted. Instead: on failure log ONE loud, chain-rich
            // error (Fail Loud, fall back VISIBLY), quarantine the file at its
            // current signature, and CONTINUE so healthy files still ingest.
            match self.on_file_changed(&path).await {
                Ok(()) => {
                    if self.ingest_quarantine.remove(&canonical).is_some() {
                        info!(
                            "[FileSyncController] poll_new_files: ingest quarantine CLEARED for {} \
                             (re-ingest succeeded after the file changed)",
                            path.display()
                        );
                    }
                    ingested += 1;
                }
                Err(e) => {
                    self.ingest_quarantine.insert(canonical.clone(), sig);
                    tracing::error!(
                        path = %path.display(),
                        error = %format!("{e:#}"),
                        "[FileSyncController] poll_new_files: CONTAINED a failed ingest -- this \
                         file is QUARANTINED from re-ingest until its content changes; the \
                         discovery walk CONTINUES so healthy files still ingest. Fix the file to \
                         un-quarantine it. (on_file_changed also write-back-quarantined it.)",
                    );
                }
            }
        }
        Ok(ingested)
    }

    /// Re-render all tracked files (used for events where the doc_id is
    /// unknown, e.g. block.deleted, block.fields_changed).
    ///
    /// ADR 0025: this is a RECOVERY path — state-driven by nature (like a
    /// reseed). Since the ROOT-ITEM threading it is no longer op-BLIND:
    /// `sanctioned_removals` carries every per-block `Remove` id the feed
    /// delivered since the last flush that could not be routed to a single
    /// file (deleted pages owning their own file, cold `last_projection`),
    /// so a deletion landing here is grounded exactly like on the per-block
    /// path. Absences are additionally grounded via the sibling-file union
    /// (a de-inlined child page). What remains UNGROUNDED here are absences
    /// with no delivered op at all — for those the guard fires
    /// the MASS-TRUNCATION tripwire only (see `tripwire_mass_truncation`): a
    /// single/small ungrounded drop passes, a row-28-shaped mass truncation
    /// vetoes + quarantines that one file. Follow-up named by ADR 0025: feed it
    /// the C2b history relation so EVERY removal is grounded and the tripwire
    /// tightens toward zero.
    pub async fn re_render_all_tracked(
        &mut self,
        sanctioned_removals: &HashSet<String>,
    ) -> Result<()> {
        let keys: Vec<CanonicalPath> = self.last_projection.keys().cloned().collect();

        for canonical in keys {
            let path: PathBuf = (*canonical).to_path_buf();
            // If disk content differs from last_projection, ingest the pending external
            // change first so the re-render includes both the block event and external
            // edit.
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
                    "[FileSyncController] Processing pending external change for {} before \
                     re-render",
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
                            "[re_render_all_tracked] No document found for path {} (segments: \
                             {:?}) — skipping",
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

            // ADR 0025 recovery-path tripwire: absences are grounded via the
            // accumulated per-block `Remove` ids (`sanctioned_removals`, delivered
            // by the feed and routed here when no single file could consume them)
            // plus the sibling-file union. Removals with no delivered op remain
            // indistinguishable from loss, so single/small ungrounded drops pass
            // silently; only a MASS truncation (the row-28 signature)
            // vetoes+quarantines that one file. A parse/IO defect propagates
            // (loud); a tripwire veto skips just this file and the batch
            // continues. Grounding those last removals is the ADR-0025 C2b
            // history-relation follow-up.
            if let Err(e) = self
                .tripwire_mass_truncation(&path, &disk_content, &rendered, sanctioned_removals)
                .await
            {
                if self.is_quarantined(&path) {
                    // Tripwire fired (already quarantined + logged): skip this file.
                    continue;
                }
                // A real parse/IO defect — surface it loudly.
                return Err(e).with_context(|| {
                    format!(
                        "[re_render_all_tracked] write-back guard failed for {}",
                        path.display()
                    )
                });
            }

            // EROFS row 346: skip-with-one-loud-error (see on_block_changed).
            if !self
                .write_back_or_skip_readonly(&doc.id, &path, rendered.as_bytes())
                .await?
            {
                continue;
            }
            self.run_post_write_hook(&path);
            self.materialize_images(&doc.id).await?;
            self.last_projection.insert(canonical, rendered);

            info!("[FileSyncController] Re-rendered {}", path.display());
        }
        Ok(())
    }

    /// Build a page's `/`-joined title chain reading ONLY the authoritative
    /// block store (`block_raw` + `block_tags`) via `block_reader`, so it works
    /// for a page the documents registry (the lagging `WHERE tag='Page'`
    /// matview behind `DocumentManager::name_chain`) has NOT caught up to
    /// yet — e.g. a page a rule or `convert_block_to_page` just minted.
    /// Mirrors `DocumentManager::name_chain`'s page-boundary + fail-loud
    /// rules: a non-page STARTING block owns no file (empty chain); a
    /// non-page ANCESTOR of a real page is prohibited (interim ruling
    /// 2026-07-13) and fails loud.
    async fn authoritative_name_chain(&self, page_id: &EntityUri) -> Result<Vec<String>> {
        let mut chain: Vec<String> = Vec::new();
        let mut current = page_id.clone();
        let mut is_self = true;
        let mut guard = 0usize;
        // The nearest page-ancestor already folded into `chain` (self included):
        // the top of the page-chain walked so far. Used to resolve a
        // file-owning subtree root when the walk reaches a non-page ancestor.
        let mut subtree_root_page: Option<EntityUri> = None;
        loop {
            if current == EntityUri::no_parent() || current.is_sentinel() {
                break;
            }
            guard += 1;
            if guard > 1024 {
                anyhow::bail!(
                    "authoritative_name_chain({page_id}): parent chain too deep (cycle?)"
                );
            }
            let block = match self.block_reader.get_block_authoritative(&current).await? {
                Some(b) => b,
                None if is_self => return Ok(Vec::new()),
                None => anyhow::bail!(
                    "authoritative_name_chain({page_id}): ancestor '{current}' of a page is absent \
                     from the block store — a page's structural ancestors must themselves all be \
                     pages (interim ruling 2026-07-13)"
                ),
            };
            if block.is_page() {
                chain.push(block.title());
                subtree_root_page = Some(current.clone());
            } else if !is_self {
                // Non-page ancestor. A page directly under a non-page is normally
                // prohibited (interim ruling 2026-07-13) — EXCEPT when the page
                // subtree already walked is rooted at a page that OWNS its own
                // on-disk file whose doc-root was ingested under a synthetic
                // (empty) document-root sentinel rather than a page-chain (e.g. a
                // subdir journal date file `Journals/<date>.org`). That root page
                // is file-resolvable via the alias registry, so a runtime page
                // minted beneath it — e.g. `convert_block_to_page`'s new page —
                // must nest under that file, not error out and vanish from disk
                // (job 72446a9c). Resolve the subtree-root page's own path and
                // prepend its ancestor segments; only bail when the root page
                // owns no resolvable file either.
                let root_page = subtree_root_page
                    .as_ref()
                    .expect("a non-self walk has folded in >=1 page");
                match self.doc_id_to_path(root_page).await? {
                    Some(path) => {
                        let rel = path.strip_prefix(&self.root_dir).unwrap_or(&path);
                        let mut segs = path_to_name_chain(rel);
                        // The root page's own title is already in `chain`; keep
                        // only its ancestor segments as the prefix.
                        segs.pop();
                        for seg in segs.into_iter().rev() {
                            chain.push(seg);
                        }
                        break;
                    }
                    None => anyhow::bail!(
                        "authoritative_name_chain({page_id}): non-page ancestor '{current}' while \
                         walking to root and subtree-root page '{root_page}' owns no resolvable \
                         file — pages under non-pages are prohibited (interim ruling 2026-07-13)"
                    ),
                }
            } else {
                // Starting block is not a page — it owns no file.
                return Ok(Vec::new());
            }
            is_self = false;
            current = block.parent_id.clone();
        }
        chain.reverse();
        Ok(chain)
    }

    /// Fork B / LogSeq-parity daily-note ruling (2026-07-19): ensure a page
    /// owns its identity file on disk even when CHILDLESS.
    /// `inv-every-page-has-its-own- file` is UNCONDITIONAL — a page's file
    /// existence must never depend on it having children. A runtime-created
    /// page (a rule-minted journal date, or `convert_block_to_page` on a
    /// childless block) otherwise stays FILELESS: the reactive re-render
    /// resolves the page's path + doc-root header through the documents
    /// registry (a `WHERE tag='Page'` matview), which LAGS the
    /// authoritative `block_raw` write, so `doc_id_to_path`/`render_doc_blocks`
    /// see nothing and skip it — and no later CDC event re-fires the page. Here
    /// we resolve BOTH the path (`authoritative_name_chain`) and the
    /// `#+ID:` header (rendered from the page's authoritative doc-root
    /// block) from the block store, bypassing the registry lag entirely.
    /// Idempotent: a page that already owns a file (tracked in
    /// `last_projection` or present on disk) is skipped, so this is safe to
    /// fire on every page upsert.
    async fn materialize_page_identity_file(&mut self, page_id: &EntityUri) -> Result<()> {
        let Some(page_block) = self.block_reader.get_block_authoritative(page_id).await? else {
            return Ok(());
        };
        if !page_block.is_page() {
            return Ok(());
        }
        let chain = self.authoritative_name_chain(page_id).await?;
        if chain.is_empty() {
            return Ok(());
        }
        let path = self.root_dir.join(chain.join("/")).with_extension("org");
        let canonical = CanonicalPath::new(&path);
        // D3 / identity plan §5: a page RENAME changes its authoritative title,
        // so its file moves from `<old-title>.org` to this `<new-title>.org`.
        // Capture the page's PREVIOUS on-disk home (the alias registry is
        // rewritten to the new path below) so the now-orphaned old file can be
        // removed after the new one is written — otherwise the page is
        // DOUBLE-HOMED across two files (inv-every-page-has-its-own-file). The
        // removal's own delete event is made safe by the stale-file guard in
        // `on_file_deleted`.
        let prior_path = if let Some(ref registrar) = self.alias_registrar {
            registrar.resolve_alias_to_path(page_id).await
        } else {
            None
        };
        // Already ours (we wrote it) — nothing to do.
        if self.last_projection.contains_key(&canonical) {
            return Ok(());
        }
        // Already on disk (ingested / materialized elsewhere) — do not clobber.
        let disk = read_disk_or_empty(&self.fs, &path).await?;
        if !disk.is_empty() {
            return Ok(());
        }
        // Render the page's own doc: the `#+ID:` header (from the authoritative
        // doc-root block) + any children the page already has. `render_document`
        // emits `#+ID: <id>` for a block-scheme doc-root, so a childless page
        // renders NON-empty — its identity file exists.
        let children = self.block_reader.get_blocks(page_id).await?;
        let rendered = self
            .format
            .render_document(&page_block, &children, &path, &page_block.id);
        if rendered.trim().is_empty() {
            return Ok(());
        }
        // Copy-on-write: keep a virtual seed layout doc off disk (disk is empty
        // here — checked above). During boot this records the pristine asset
        // render; a post-boot user edit (render diverges) falls through and
        // materializes the file.
        if self.gate_virtual_seed_write(page_id, &canonical, &rendered, false) {
            return Ok(());
        }
        // EROFS row 346: skip-with-one-loud-error. A skip returns early WITHOUT
        // registering the alias — a path that could not be written does not own
        // a file to advertise.
        if !self
            .write_back_or_skip_readonly(page_id, &path, rendered.as_bytes())
            .await?
        {
            return Ok(());
        }
        self.run_post_write_hook(&path);
        // Register the UUID → file-path alias NOW (mirrors the ingest path's
        // `register_alias`), so the page immediately OWNS its file in the alias
        // registry — `inv-every-page-has-its-own-file` (and every file-tracking
        // consumer) reads that registry, and a runtime materialize must not wait for
        // a watcher re-ingest round-trip to surface the file.
        if let Some(ref registrar) = self.alias_registrar {
            registrar.register_alias(page_id, &path).await;
        }
        // Remove the orphaned old file left by a page rename (see `prior_path`).
        if let Some(prior) = prior_path {
            let prior_canonical = CanonicalPath::new(&prior);
            if prior_canonical != canonical && self.fs.exists(&prior) {
                self.fs.remove(&prior).await.map_err(|e| {
                    anyhow::anyhow!(
                        "materialize_page_identity_file({page_id}): removing orphaned old file {} \
                         after rename to {}: {e}",
                        prior.display(),
                        path.display()
                    )
                })?;
                self.forget_file_state(&prior_canonical);
                self.run_post_write_hook(&prior);
                tracing::info!(
                    "[FileSyncController] Removed orphaned old file {} after page {} renamed to {}",
                    prior.display(),
                    page_id,
                    path.display(),
                );
            }
        }
        self.last_projection.insert(canonical, rendered);
        tracing::info!(
            "[FileSyncController] Materialized identity file for runtime-created page {} -> {}",
            page_id,
            path.display()
        );
        Ok(())
    }

    /// Fork B B2 — materialize every page (document root) that owns NO file on
    /// disk into its own `<name-chain>.org`. A FILELESS page (a `Page` block
    /// that exists in the store but backs no file — e.g. a rule-created
    /// journal date, or a companion-inlined page-heading the CTE
    /// de-inlines) otherwise vanishes on store-rebuild-from-disk and is
    /// invisible to file-based sync/backup.
    ///
    /// Runs after the initial scan so migration converges without waiting for a
    /// per-page CDC edit (OQ4 RULED: the boot sweep is the migration
    /// mechanism). Idempotent: a page already owning a file (tracked or
    /// present on disk) is skipped. New-file writes seed `last_projection`
    /// (echo suppression) so the watcher event for our own write is not
    /// re-ingested.
    pub async fn materialize_missing_page_files(&mut self) -> Result<()> {
        let docs = self.block_reader.iter_documents_with_blocks().await?;
        for (doc_id, blocks) in docs {
            if blocks.is_empty() {
                continue;
            }
            let path = match self.doc_id_to_path(&doc_id).await {
                Ok(Some(p)) => p,
                Ok(None) => continue,
                Err(e) => {
                    // §3.1 Finding A / R11: name_chain failed loud for this
                    // document. Surface it and skip only this one — the boot
                    // sweep continues materializing every other fileless page.
                    tracing::error!(
                        doc_id = %doc_id,
                        error = %format!("{e:#}"),
                        "[FileSyncController] materialize_missing_page_files: could not \
                         resolve doc to a page-file path (name_chain failed loud) — \
                         SKIPPING this document; sweep continues.",
                    );
                    continue;
                }
            };
            let canonical = CanonicalPath::new(&path);
            if self.last_projection.contains_key(&canonical) {
                continue;
            }
            let disk = read_disk_or_empty(&self.fs, &path).await?;
            if !disk.is_empty() {
                continue;
            }
            let rendered = self.render_doc_blocks(&doc_id, &path, &blocks).await?;
            if rendered.trim().is_empty() {
                continue;
            }
            // Copy-on-write: a virtual seed doc (`block:__default__`) is NEVER
            // materialized by the boot sweep (the F4 stale-seed pin). Record its
            // pristine asset render as the post-boot copy-on-write baseline and
            // skip the write. `disk` is empty here (checked above).
            if self.gate_virtual_seed_write(&doc_id, &canonical, &rendered, false) {
                continue;
            }
            // EROFS row 346: skip-with-one-loud-error (see on_block_changed).
            if !self
                .write_back_or_skip_readonly(&doc_id, &path, rendered.as_bytes())
                .await?
            {
                continue;
            }
            self.run_post_write_hook(&path);
            self.last_projection.insert(canonical, rendered);
            info!(
                "[FileSyncController] Materialized fileless page {} -> {}",
                doc_id,
                path.display()
            );
        }
        Ok(())
    }

    /// Render a document from the authoritative doc-scoped read.
    async fn render_file_by_doc_id(&self, doc_id: &EntityUri, path: &Path) -> Result<String> {
        self.renderer.render_document(doc_id, path).await
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
        self.renderer.render_blocks(doc_id, path, blocks).await
    }

    /// Write image files to disk for all image blocks in this document.
    ///
    /// Called after rendering an org file — the `[[file:path]]` links exist in
    /// the org text, but the actual binary files may not yet be on disk.
    /// Reads bytes from the `ImageDataProvider` and writes to
    /// `{root_dir}/{block.content}`. Skips blocks whose files already
    /// exist.
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
                    "[FileSyncController] No image data stored for block {} — file {} will be \
                     missing on disk",
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
    /// Called after parsing an org file that contains `[[file:path]]` image
    /// links. The blocks have been created in the store, but the binary
    /// data needs to be ingested so it's available for cross-peer sync and
    /// Loro storage.
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
    /// Returns Err if the resolved path escapes the root directory (path
    /// traversal).
    fn resolve_image_path(&self, relative_path: &str) -> Result<PathBuf> {
        let joined = self.root_dir.join(relative_path);
        let canonical_root = self
            .fs
            .canonicalize(&self.root_dir)
            .unwrap_or_else(|_| self.root_dir.clone());
        // For paths that don't exist yet, canonicalize the parent and append the
        // filename
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

    /// Fork B B1' / ADR 0025: compute the ungrounded drops a block-driven
    /// write-back of `rendered` over `source` would cause, as DATA.
    ///
    /// `source` is the on-disk content about to be overwritten; `rendered` is
    /// the projection about to be written. Grounds each absence via the
    /// sibling-file union (see
    /// [`writeback_sibling_grounding`](Self::writeback_sibling_grounding))
    /// plus `sanctioned_removals` (the triggering delta's `Remove` ids; empty
    /// on recovery/ingest paths). Returns `(verdict, unresolvable)`: the
    /// verdict (dropped `id: excerpt` list + source block count for the
    /// tripwire threshold) and the ids of absent blocks whose own-file path
    /// could not be resolved (name_chain failed loud — a HARD-veto signal,
    /// see [`SiblingGrounding`]). Real parse/IO defects propagate as `Err`
    /// (never swallowed).
    async fn writeback_drops(
        &self,
        path: &Path,
        source: &str,
        rendered: &str,
        sanctioned_removals: &HashSet<String>,
    ) -> Result<(WritebackDropVerdict, Vec<String>)> {
        let grounding = self
            .writeback_sibling_grounding(path, source, rendered, sanctioned_removals)
            .await?;
        let sibling_refs: Vec<(&Path, &str)> = grounding
            .siblings
            .iter()
            .map(|(p, c)| (p.as_path(), c.as_str()))
            .collect();
        let verdict = self.format.writeback_drops(
            path,
            source,
            rendered,
            &sibling_refs,
            sanctioned_removals,
            &self.root_dir,
        )?;
        Ok((verdict, grounding.unresolvable))
    }

    /// Collect the sibling-file grounding for the write-back guard: the on-disk
    /// content of the own-file page that each block present in `source` but
    /// absent from `rendered` (and not `sanctioned_removals`) de-inlined
    /// into. A legitimate de-inline (a child page that moved to its own
    /// materialized file) resolves to a DISTINCT sibling file whose content
    /// grounds the absence; a genuine drop resolves to no distinct sibling
    /// and stays ungrounded so the guard vetoes. Only absent blocks pay a
    /// file read — the hot no-absence case (content edit, addition) does
    /// none.
    async fn writeback_sibling_grounding(
        &self,
        path: &Path,
        source: &str,
        rendered: &str,
        sanctioned_removals: &HashSet<String>,
    ) -> Result<SiblingGrounding> {
        let parent = EntityUri::no_parent();
        let source_parsed = self.format.parse(path, source, &parent, &self.root_dir)?;
        let rendered_parsed = self.format.parse(path, rendered, &parent, &self.root_dir)?;
        let rendered_ids: HashSet<&str> = rendered_parsed
            .blocks
            .iter()
            .map(|b| b.id.as_str())
            .collect();

        let self_canonical = CanonicalPath::new(path);
        let mut grounding = SiblingGrounding::default();
        let mut seen: HashSet<CanonicalPath> = HashSet::new();
        for block in &source_parsed.blocks {
            let id = block.id.as_str();
            if rendered_ids.contains(id) || sanctioned_removals.contains(id) {
                continue;
            }
            let sibling = match self.doc_id_to_path(&block.id).await {
                Ok(Some(p)) => p,
                Ok(None) => continue,
                Err(e) => {
                    // §3.1 Finding A / R11 + BugFunnel row 23/29: this absent
                    // (drop-candidate) block's own-file path could NOT be
                    // resolved because `name_chain` failed loud (a prohibited
                    // page-under-non-page topology). We genuinely cannot prove
                    // where this block went, so it is UNRESOLVABLE — record it
                    // and surface it loudly. The tripwire treats any unresolvable
                    // drop as a HARD veto (abort + quarantine), independent of the
                    // mass-truncation threshold: a name_chain-failed grounding
                    // storm must ABORT the write, never let the truncated
                    // projection reach disk (the first-boot 6,245-line Projects.org
                    // destruction: 749 such failures fell under the 25% threshold
                    // and the file was rewritten anyway).
                    tracing::error!(
                        block_id = %block.id,
                        path = %path.display(),
                        error = %format!("{e:#}"),
                        "[FileSyncController] writeback_sibling_grounding: could not resolve \
                         an absent block's own-file path (name_chain failed loud) — UNRESOLVABLE; \
                         the guard will ABORT + quarantine this write (the safe outcome).",
                    );
                    grounding.unresolvable.push(block.id.as_str().to_string());
                    continue;
                }
            };
            let sibling_canonical = CanonicalPath::new(&sibling);
            if sibling_canonical == self_canonical || !seen.insert(sibling_canonical) {
                continue;
            }
            let content = read_disk_or_empty(&self.fs, &sibling).await?;
            if !content.trim().is_empty() {
                grounding.siblings.push((sibling, content));
            }
        }
        Ok(grounding)
    }

    /// Fork B B1' / ADR 0025 recovery-path MASS-TRUNCATION tripwire.
    ///
    /// Since the ADR 0025 ROOT-ITEM threading the block-driven paths DO receive
    /// per-block `Remove` ids: `on_block_changed` sanctions its delta's
    /// `Remove`, the block-feed resolver (di.rs) routes BOTH a genuine feed
    /// removal AND a cross-doc DEPARTURE (via `LiveData::group_by`'s
    /// `Remove{old doc}`) as an `on_block_changed` `Remove` delta grounded in
    /// the owning file, and `re_render_all_tracked` handles the residual bulk
    /// recovery reseeds (which carry no per-block intent). Every op-delivered
    /// deletion — and now every group-key departure — is therefore grounded.
    /// The residual UNGROUNDED-drop tolerance below exists because state-driven
    /// renders can still legitimately shrink a file without a delivered
    /// `Remove` op:
    /// - a sanction consumed by a render whose write was TOCTOU-skipped — the
    ///   id is spent, the shrink reappears on a later state-driven render;
    /// - matview-lag races between store truth and the delta that triggered the
    ///   render.
    /// So this tripwire fires ONLY on the row-28 loss SIGNATURE — a MASS
    /// truncation, where an FK-rollback aborted a large contiguous chunk (~20
    /// blocks) at once. It vetoes+quarantines when the ungrounded-drop count
    /// exceeds `max(TRIPWIRE_MIN_DROP_BLOCKS, 25% of the file's block count)`,
    /// and lets single/small drops pass SILENTLY (no log — else routine
    /// moves/races flood `inv-no-observed-errors`).
    ///
    /// Returns `Ok(())` to proceed with the write, `Err` (after quarantining)
    /// to refuse it. Real parse/IO defects propagate as `Err` WITHOUT
    /// quarantining (they are bugs to surface, not truncations). This is
    /// ADR 0025's "recovery paths carry the guard as tripwire" posture.
    /// Tightening toward zero needs the classes above grounded too — the
    /// C2b history relation follow-up.
    async fn tripwire_mass_truncation(
        &mut self,
        path: &Path,
        source: &str,
        rendered: &str,
        sanctioned_removals: &HashSet<String>,
    ) -> Result<()> {
        let (verdict, unresolvable) = self
            .writeback_drops(path, source, rendered, sanctioned_removals)
            .await?;

        // HARD veto (BugFunnel row 23/29): any absent block whose own-file path
        // could NOT be resolved (name_chain failed loud — a prohibited topology)
        // is UNRESOLVABLE. We cannot prove it was preserved elsewhere, so the
        // grounding failure must ABORT the write regardless of the count — never
        // let it fall under the mass-truncation threshold. This is the fix for the
        // first-boot 6,245-line Projects.org destruction (749 name_chain failures
        // that stayed under the 25% threshold and truncated the file anyway).
        if !unresolvable.is_empty() {
            let err = anyhow::anyhow!(
                "UNRESOLVABLE WRITE-BACK DROP: {} on-disk block(s) are absent from the projection \
                 AND their own-file path could not be resolved (name_chain failed loud — a \
                 prohibited page-under-non-page topology). Write-back cannot prove these blocks \
                 were preserved elsewhere, so the write is REFUSED to avoid silent data loss \
                 (BugFunnel row 23). Unresolvable: {:?}",
                unresolvable.len(),
                unresolvable,
            );
            self.quarantine_writeback(path, &err);
            return Err(err);
        }

        let threshold = std::cmp::max(
            TRIPWIRE_MIN_DROP_BLOCKS,
            verdict.source_block_count / TRIPWIRE_DROP_FRACTION_DENOM,
        );
        if verdict.dropped.len() > threshold {
            let err = anyhow::anyhow!(
                "MASS TRUNCATION on the block-driven write-back path: {} of {} on-disk block(s) \
                 would be DELETED, grounded by neither a sibling materialized file nor a \
                 sanctioned removal — the row-28 loss signature (threshold {}). Dropped: {:?}",
                verdict.dropped.len(),
                verdict.source_block_count,
                threshold,
                verdict.dropped,
            );
            self.quarantine_writeback(path, &err);
            return Err(err);
        }
        Ok(())
    }

    /// Quarantine `path` from write-back after a mass-truncation tripwire fire
    /// (Fork B B1', row-28 posture): the store projection would DELETE a large
    /// chunk of blocks present on disk, grounded by nothing — refuse the write
    /// and skip this file until a clean re-ingest clears the quarantine.
    /// Loud + disclosed (an ERROR here means a real truncation was caught,
    /// not routine noise — single/small drops never reach this).
    fn quarantine_writeback(&mut self, path: &Path, err: &anyhow::Error) {
        if self.quarantined.insert(CanonicalPath::new(path)) {
            self.quarantine_skip_logged
                .lock()
                .expect("quarantine_skip_logged poisoned")
                .remove(&CanonicalPath::new(path));
            tracing::error!(
                path = %path.display(),
                error = %format!("{err:#}"),
                "[FileSyncController] block-driven write-back VETOED (mass-truncation tripwire) — \
                 QUARANTINING this file so its truncated projection is not rendered over disk. \
                 Un-quarantines on the next fully-successful ingest.",
            );
        }
    }

    /// Resolve a doc_id to a filesystem path via DocumentManager.
    ///
    /// Return-type contract (Fork B B1 / §3.1 Finding A — do NOT collapse the
    /// two `None`-like cases into one):
    /// - `Ok(Some(path))` — the doc resolved to a page-file path.
    /// - `Ok(None)` — the doc is **legitimately not a page** (empty
    ///   name-chain). A silent skip is correct here (a non-page block owns no
    ///   file).
    /// - `Err(e)` — `name_chain` FAILED LOUD (e.g. the no-pages-under-non-pages
    ///   assertion tripped, or a hierarchy read errored). This is a real,
    ///   previously-unseen condition and MUST NOT be swallowed into the same
    ///   bucket as "not a page". Every caller `tracing::error!`s it and skips
    ///   only THIS document (bounded blast radius — never crash the sync loop).
    async fn doc_id_to_path(&self, doc_id: &EntityUri) -> Result<Option<PathBuf>> {
        // Try alias registrar first (fastest path)
        if let Some(ref registrar) = self.alias_registrar {
            if let Some(path) = registrar.resolve_alias_to_path(doc_id).await {
                return Ok(Some(path));
            }
        }

        // Walk the Document hierarchy to compute the path. An error here
        // (no-pages-under-non-pages assertion, missing ancestor) propagates
        // loudly — the callers decide the bounded blast radius.
        let chain = self.doc_manager.name_chain(doc_id).await?;
        if chain.is_empty() {
            return Ok(None);
        }
        let path = self.root_dir.join(chain.join("/")).with_extension("org");
        Ok(Some(path))
    }

    /// Write `rendered` to `path` for `doc_id`, applying the read-only
    /// skip-with-one-loud-error posture (BugFunnel EROFS row 346).
    ///
    /// - `Ok(true)`  — the write succeeded (caller runs its post-write steps).
    /// - `Ok(false)` — SKIPPED because this path is on a read-only filesystem
    ///   (EROFS). The FIRST such failure logs a loud ERROR and marks the path;
    ///   every later CDC event for it returns `Ok(false)` WITHOUT touching the
    ///   fs — no per-event retry storm. `last_projection` is deliberately NOT
    ///   updated by the caller on a skip, so if the path later becomes writable
    ///   (alias change / re-ingest clears the mark) the next event re-attempts.
    /// - `Err(e)`    — a non-EROFS IO error propagates LOUDLY, per-event: only
    ///   the persistent read-only condition is de-duplicated; a transient or
    ///   unexpected fault stays visible on every occurrence.
    async fn write_back_or_skip_readonly(
        &mut self,
        doc_id: &EntityUri,
        path: &Path,
        rendered: &[u8],
    ) -> Result<bool> {
        let canonical = CanonicalPath::new(path);
        if self.writeback_readonly.contains(&canonical) {
            tracing::debug!(
                doc_id = %doc_id,
                path = %path.display(),
                "[FileSyncController] write-back skipped for read-only path                  (already disclosed once)",
            );
            return Ok(false);
        }
        if let Some(parent) = path.parent() {
            match self.fs.create_dir_all(parent).await {
                Ok(()) => {}
                Err(e) if is_read_only_fs(&e) => {
                    self.mark_readonly_writeback(doc_id, path, &e, canonical);
                    return Ok(false);
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("create parent dir for org write-back to {}", path.display())
                    });
                }
            }
        }
        match self.fs.write(path, rendered).await {
            Ok(()) => Ok(true),
            Err(e) if is_read_only_fs(&e) => {
                self.mark_readonly_writeback(doc_id, path, &e, canonical);
                Ok(false)
            }
            Err(e) => {
                Err(e).with_context(|| format!("org write-back to {} failed", path.display()))
            }
        }
    }

    /// Record `path` as read-only for write-back and emit the ONE loud ERROR
    /// (Fail Loud, Never Fake: disclose the degraded mode, then skip quietly).
    fn mark_readonly_writeback(
        &mut self,
        doc_id: &EntityUri,
        path: &Path,
        err: &std::io::Error,
        canonical: CanonicalPath,
    ) {
        if self.writeback_readonly.insert(canonical) {
            tracing::error!(
                doc_id = %doc_id,
                path = %path.display(),
                error = %err,
                "[FileSyncController] org write-back FAILED on a read-only                  filesystem (EROFS os error 30) — this doc has no writable                  backing file (relay/synthetic doc, or a read-only vault                  mount). DISABLING write-back for this path so subsequent CDC                  events do NOT retry the doomed write; re-enabled when the doc                  (re)gains a writable backing file or on a clean re-ingest.",
            );
        }
    }
}

/// True when an IO error is a persistent read-only-filesystem condition
/// (EROFS, os error 30) — either the mapped `ErrorKind::ReadOnlyFilesystem`
/// or the raw errno directly (belt-and-suspenders for adapters that build the
/// error by kind without an errno, and for platforms whose mapping lags).
fn is_read_only_fs(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::ReadOnlyFilesystem || e.raw_os_error() == Some(30)
}

/// Read a file's content, treating a missing file as empty content (a
/// legitimate "no baseline yet" state for org sync) but propagating any other
/// IO error loudly. Distinguishing absence from a real read failure prevents a
/// transient IO error from masquerading as empty disk content and wiping the
/// user's data on write-back.
async fn read_disk_or_empty(fs: &Arc<dyn FileSystem>, path: &Path) -> Result<String> {
    match fs.read_to_string(path).await {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("reading {} for org sync", path.display())),
    }
}

/// Inc 3: whether a parsed org file is a shared-subtree PROJECTION (a mount
/// page and its shared descendants), which must NOT be re-ingested as fresh
/// global intent (Model.md invariant 11 — its truth is the shared Loro doc).
/// True when the page block IS the mount, or any block carries the mount role
/// or a `shared-tree-id` stamp.
fn is_shared_subtree_projection(document: &Block, blocks: &[Block]) -> bool {
    document.is_share_mount()
        || blocks
            .iter()
            .any(|b| b.is_share_mount() || b.shared_tree_id().is_some())
}

/// Convert a relative path (e.g. "projects/todo.org") to a name chain
/// (["projects", "todo"]).
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

/// Outcome of the ID-less content+position reconcile.
#[derive(Debug, Default)]
struct IdlessRemap {
    /// minted incoming id → existing store id it reconciles onto.
    remaps: HashMap<EntityUri, EntityUri>,
    /// minted incoming ids whose content matched an existing sibling only at a
    /// DIFFERENT position — left to mint, caller discloses via WARN.
    ambiguous: Vec<EntityUri>,
}

/// Classify the reconcile situation from data already in hand (store child
/// count, minted fraction). Informational: ships in `MatchContext` so a future
/// situational strategy can branch, but the v0 `PositionalExactMatcher` ignores
/// it -- so this is behavior-neutral.
fn detect_match_situation(
    existing: &[ExistingChild],
    incoming: &[IncomingIdentity],
) -> MatchSituation {
    if existing.is_empty() {
        return MatchSituation::PristineIngest;
    }
    let minted = incoming.iter().filter(|i| i.minted).count();
    if minted == 0 {
        return MatchSituation::SyncMerge;
    }
    let idless_fraction = minted as f64 / incoming.len() as f64;
    if idless_fraction >= 0.5 {
        MatchSituation::StaleRewrite { idless_fraction }
    } else {
        MatchSituation::SyncMerge
    }
}

/// Match ID-less (freshly-minted) incoming headlines onto their already-minted
/// twins among the store's CURRENT children, by exact content at the same
/// sibling position under the same parent.
///
/// Pure and deterministic. Positional 1:1: two genuinely-distinct ID-less
/// siblings with identical content match their two twins in order (never merged
/// to one), and true surplus mints. A content match at a DIFFERENT position is
/// reported `ambiguous` (caller keeps the mint) rather than guessed into a
/// merge. `incoming` MUST be in document order (parents before children) so a
/// remapped ID-less parent regroups its subtree before the children are
/// matched.
fn compute_idless_remaps(existing: &[ExistingChild], incoming: &[IncomingIdentity]) -> IdlessRemap {
    let mut by_parent: HashMap<EntityUri, Vec<&ExistingChild>> = HashMap::new();
    for e in existing {
        by_parent.entry(e.parent.clone()).or_default().push(e);
    }
    for sibs in by_parent.values_mut() {
        sibs.sort_by_key(|e| e.seq);
    }

    // Existing ids the incoming set already matches verbatim by id are claimed —
    // an ID-less block cannot also absorb one of those.
    let incoming_ids: HashSet<&EntityUri> = incoming.iter().map(|i| &i.id).collect();
    let mut claimed: HashSet<EntityUri> = existing
        .iter()
        .filter(|e| incoming_ids.contains(&e.id))
        .map(|e| e.id.clone())
        .collect();

    let mut out = IdlessRemap::default();
    let mut pos_in_parent: HashMap<EntityUri, usize> = HashMap::new();
    for inc in incoming {
        // A parent that is itself a remapped ID-less headline regroups onto its
        // existing twin before its children are positioned/matched.
        let parent = out
            .remaps
            .get(&inc.parent)
            .cloned()
            .unwrap_or_else(|| inc.parent.clone());
        let pos = {
            let counter = pos_in_parent.entry(parent.clone()).or_insert(0);
            let p = *counter;
            *counter += 1;
            p
        };
        if !inc.minted {
            continue; // authored `:ID:` — the by-id diff handles it
        }
        let sibs = by_parent.get(&parent);
        let twin = sibs
            .and_then(|s| s.get(pos))
            .filter(|e| e.content == inc.content && !claimed.contains(&e.id));
        if let Some(e) = twin {
            claimed.insert(e.id.clone());
            out.remaps.insert(inc.id.clone(), e.id.clone());
        } else if sibs
            .map(|s| {
                s.iter()
                    .any(|e| e.content == inc.content && !claimed.contains(&e.id))
            })
            .unwrap_or(false)
        {
            out.ambiguous.push(inc.id.clone());
        }
    }
    out
}

/// v0 strategy: exact content at the same sibling position (PR #81's rule),
/// wrapping the pure `compute_idless_remaps`. This is the DEFAULT injected
/// matcher and is behavior-frozen relative to the pre-seam controller: it
/// ignores `ctx.situation` and `ctx.document_uri`.
pub struct PositionalExactMatcher;

#[async_trait::async_trait]
impl BlockMatchStrategy for PositionalExactMatcher {
    fn id(&self) -> &'static str {
        "positional-exact"
    }

    async fn match_blocks(&self, ctx: MatchContext<'_>) -> Result<Vec<MatchVerdict>> {
        let IdlessRemap { remaps, ambiguous } = compute_idless_remaps(ctx.existing, ctx.incoming);
        let ambiguous: HashSet<EntityUri> = ambiguous.into_iter().collect();
        let mut verdicts = Vec::with_capacity(ctx.incoming.len());
        for inc in ctx.incoming {
            if !inc.minted {
                continue; // authored `:ID:` -- never minted; the by-id diff owns it
            }
            let verdict = if let Some(onto) = remaps.get(&inc.id) {
                MatchVerdict::Remap {
                    minted: inc.id.clone(),
                    onto: onto.clone(),
                    basis: MatchBasis::ContentAtPosition,
                }
            } else if ambiguous.contains(&inc.id) {
                // Candidates = existing children with equal content -- the ids the
                // caller's WARN discloses. Informational; does not gate behavior.
                let candidates = ctx
                    .existing
                    .iter()
                    .filter(|e| e.content == inc.content)
                    .map(|e| e.id.clone())
                    .collect();
                MatchVerdict::MintAmbiguous {
                    minted: inc.id.clone(),
                    candidates,
                }
            } else {
                MatchVerdict::MintFresh {
                    minted: inc.id.clone(),
                }
            };
            verdicts.push(verdict);
        }
        Ok(verdicts)
    }
}

/// v1 strategy, RULING A2. Per minted (id-less) incoming headline, among the
/// unclaimed EXISTING store children with equal content:
/// - **empty** -> `MintFresh` (the ONLY fresh-mint path: genuinely new block).
/// - **T1 exact position** (PR #81), GUARDED: the twin at the same sibling
///   position under the resolved parent wins only if it is the SOLE candidate
///   OR its descendant subtree signature matches -- so a position tie between
///   several same-content twins with DIFFERENT subtrees defers to T2 rather
///   than silently re-homing children onto the wrong twin.
/// - **T3 content unique in the WHOLE DOCUMENT on BOTH sides** -> remap (basis
///   `ContentUniqueInDocument`; handles cross-parent moves).
/// - **T2 tie-break** (RULING A2, replaces the old MintAmbiguous branch):
///   multiple same-content twins and/or a duplicated incoming side. Pair
///   deterministically by DESCENDANT SUBTREE SIGNATURE first (a twin keeps its
///   own children), then by relative sibling position (identical-subtree twins
///   are interchangeable). This tier always claims a candidate.
///
/// Consequently `tiered_match` never emits `MintAmbiguous`: identical-content
/// siblings stop duplicating, and children are never re-homed onto the wrong
/// twin. (The `MintAmbiguous` variant survives for `PositionalExactMatcher`.)
pub struct TieredMatcher;

/// Structural fingerprint of a block's DESCENDANTS (ids irrelevant): the
/// ordered list of each child's `(content, child-subtree-signature)`,
/// recursively. Existing children are ordered by `seq`, incoming children by
/// document order. Two blocks share a signature iff their whole subtrees are
/// identical in content and order -- the RULING A2 discriminator that pairs a
/// same-content twin with the store twin that kept its exact children.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SubtreeSig(Vec<(String, SubtreeSig)>);

/// Post-order subtree signature of an existing store block, memoized by id.
fn existing_subtree_sig(
    id: &EntityUri,
    by_parent: &HashMap<EntityUri, Vec<&ExistingChild>>,
    cache: &mut HashMap<EntityUri, SubtreeSig>,
) -> SubtreeSig {
    if let Some(s) = cache.get(id) {
        return s.clone();
    }
    let sig = SubtreeSig(
        by_parent
            .get(id)
            .map(|kids| {
                kids.iter()
                    .map(|c| {
                        (
                            c.content.clone(),
                            existing_subtree_sig(&c.id, by_parent, cache),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    );
    cache.insert(id.clone(), sig.clone());
    sig
}

/// Post-order subtree signature of an incoming parsed block, memoized by id.
fn incoming_subtree_sig(
    id: &EntityUri,
    children_by_parent: &HashMap<EntityUri, Vec<&IncomingIdentity>>,
    cache: &mut HashMap<EntityUri, SubtreeSig>,
) -> SubtreeSig {
    if let Some(s) = cache.get(id) {
        return s.clone();
    }
    let sig = SubtreeSig(
        children_by_parent
            .get(id)
            .map(|kids| {
                kids.iter()
                    .map(|c| {
                        (
                            c.content.clone(),
                            incoming_subtree_sig(&c.id, children_by_parent, cache),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    );
    cache.insert(id.clone(), sig.clone());
    sig
}

#[async_trait::async_trait]
impl BlockMatchStrategy for TieredMatcher {
    fn id(&self) -> &'static str {
        "tiered-v1"
    }

    async fn match_blocks(&self, ctx: MatchContext<'_>) -> Result<Vec<MatchVerdict>> {
        Ok(tiered_match(ctx.existing, ctx.incoming))
    }
}

/// Pure v1 matcher. Processes `incoming` in document order (parents-first) so a
/// remapped parent regroups its subtree for the T1 positional tie-breaker and
/// so descendant subtree signatures are consistent across the pass. Greedy 1:1
/// claiming keeps each store id claimed at most once.
pub fn tiered_match(
    existing: &[ExistingChild],
    incoming: &[IncomingIdentity],
) -> Vec<MatchVerdict> {
    // Positional index by parent (T1), sorted by DFS sequence.
    let mut by_parent: HashMap<EntityUri, Vec<&ExistingChild>> = HashMap::new();
    for e in existing {
        by_parent.entry(e.parent.clone()).or_default().push(e);
    }
    for sibs in by_parent.values_mut() {
        sibs.sort_by_key(|e| e.seq);
    }

    // Incoming children, grouped by (raw) parent in document order -- the parse
    // tree used to compute descendant subtree signatures (RULING A2 T2).
    let mut incoming_children: HashMap<EntityUri, Vec<&IncomingIdentity>> = HashMap::new();
    for i in incoming {
        incoming_children
            .entry(i.parent.clone())
            .or_default()
            .push(i);
    }

    // Precompute both sides' descendant subtree signatures (memoized post-order).
    let mut existing_sig_cache: HashMap<EntityUri, SubtreeSig> = HashMap::new();
    for e in existing {
        existing_subtree_sig(&e.id, &by_parent, &mut existing_sig_cache);
    }
    let mut incoming_sig_cache: HashMap<EntityUri, SubtreeSig> = HashMap::new();
    for i in incoming {
        incoming_subtree_sig(&i.id, &incoming_children, &mut incoming_sig_cache);
    }

    // Existing ids matched verbatim by an incoming id are claimed up front -- an
    // id-less block can never absorb an authored twin.
    let incoming_ids: HashSet<&EntityUri> = incoming.iter().map(|i| &i.id).collect();
    let mut claimed: HashSet<EntityUri> = existing
        .iter()
        .filter(|e| incoming_ids.contains(&e.id))
        .map(|e| e.id.clone())
        .collect();

    // Incoming-side document-wide content multiplicity (minted only). The
    // both-sides-uniqueness gate needs the incoming count to be exactly 1.
    let mut incoming_dupes: HashMap<&str, usize> = HashMap::new();
    for inc in incoming.iter().filter(|i| i.minted) {
        *incoming_dupes.entry(inc.content.as_str()).or_insert(0) += 1;
    }

    let mut remaps: HashMap<EntityUri, EntityUri> = HashMap::new();
    let mut pos_in_parent: HashMap<EntityUri, usize> = HashMap::new();
    let mut verdicts = Vec::new();

    for inc in incoming {
        // A parent that is itself a remapped id-less headline regroups onto its
        // existing twin before its children are positioned/matched.
        let parent = remaps
            .get(&inc.parent)
            .cloned()
            .unwrap_or_else(|| inc.parent.clone());
        let pos = {
            let counter = pos_in_parent.entry(parent.clone()).or_insert(0);
            let p = *counter;
            *counter += 1;
            p
        };
        if !inc.minted {
            continue; // authored `:ID:` -- the by-id diff owns it
        }

        // Same-content, unclaimed existing candidates, ordered by sibling
        // position (seq) then id -- a deterministic relative-position order.
        let mut candidates: Vec<&ExistingChild> = existing
            .iter()
            .filter(|e| e.content == inc.content && !claimed.contains(&e.id))
            .collect();
        candidates.sort_by(|a, b| a.seq.cmp(&b.seq).then_with(|| a.id.id().cmp(b.id.id())));

        if candidates.is_empty() {
            // No candidate left -- genuinely new (RULING A2: the sole mint path).
            verdicts.push(MatchVerdict::MintFresh {
                minted: inc.id.clone(),
            });
            continue;
        }

        let inc_sig = &incoming_sig_cache[&inc.id];

        // T1: exact content at the SAME sibling position under the (resolved)
        // parent (PR #81). GUARDED (RULING A2): the positional twin wins only if
        // it is the SOLE candidate or its subtree signature matches -- otherwise
        // several same-content twins with different subtrees would let position
        // silently re-home children onto the wrong twin; defer to T2.
        let positional = by_parent
            .get(&parent)
            .and_then(|s| s.get(pos))
            .filter(|e| e.content == inc.content && !claimed.contains(&e.id));
        if let Some(e) = positional {
            if candidates.len() == 1 || existing_sig_cache[&e.id] == *inc_sig {
                claimed.insert(e.id.clone());
                remaps.insert(inc.id.clone(), e.id.clone());
                verdicts.push(MatchVerdict::Remap {
                    minted: inc.id.clone(),
                    onto: e.id.clone(),
                    basis: MatchBasis::ContentAtPosition,
                });
                continue;
            }
        }

        // T3: exact content UNIQUE in the whole document on BOTH sides.
        let incoming_unique = incoming_dupes
            .get(inc.content.as_str())
            .copied()
            .unwrap_or(0)
            == 1;
        if candidates.len() == 1 && incoming_unique {
            let e = candidates[0];
            claimed.insert(e.id.clone());
            remaps.insert(inc.id.clone(), e.id.clone());
            verdicts.push(MatchVerdict::Remap {
                minted: inc.id.clone(),
                onto: e.id.clone(),
                basis: MatchBasis::ContentUniqueInDocument,
            });
            continue;
        }

        // T2 (RULING A2): multiple same-content twins and/or a duplicated
        // incoming side -- neither position-exact nor content-unique. Pair
        // deterministically: prefer the candidate whose DESCENDANT SUBTREE
        // SIGNATURE matches (that twin keeps its own children -- never re-homed
        // onto the wrong twin); among matches, and when none match, the lowest
        // (seq, id) candidate wins (relative position; identical-subtree twins
        // are interchangeable). Always claims a candidate -- never MintAmbiguous.
        let (e, basis) = match candidates
            .iter()
            .copied()
            .find(|e| existing_sig_cache[&e.id] == *inc_sig)
        {
            Some(e) => (e, MatchBasis::SubtreeSignature),
            None => (candidates[0], MatchBasis::ContentAtRelativePosition),
        };
        claimed.insert(e.id.clone());
        remaps.insert(inc.id.clone(), e.id.clone());
        verdicts.push(MatchVerdict::Remap {
            minted: inc.id.clone(),
            onto: e.id.clone(),
            basis,
        });
    }
    verdicts
}

#[cfg(test)]
mod idless_reconcile_tests {
    use std::collections::HashSet;

    use holon_api::EntityUri;

    use super::BlockMatchStrategy;
    use super::ExistingChild;
    use super::IncomingIdentity;
    use super::MatchBasis;
    use super::MatchContext;
    use super::MatchSituation;
    use super::MatchVerdict;
    use super::PositionalExactMatcher;

    fn doc() -> EntityUri {
        EntityUri::block("doc")
    }

    fn existing(id: &str, parent: &EntityUri, seq: i64, content: &str) -> ExistingChild {
        ExistingChild {
            id: EntityUri::block(id),
            parent: parent.clone(),
            seq,
            content: content.to_string(),
        }
    }

    fn incoming(id: &str, parent: &EntityUri, content: &str, minted: bool) -> IncomingIdentity {
        IncomingIdentity {
            id: EntityUri::block(id),
            parent: parent.clone(),
            content: content.to_string(),
            minted,
        }
    }

    /// Drive the DEFAULT v0 strategy (`PositionalExactMatcher`) through the
    /// `BlockMatchStrategy` port -- the same path the controller uses.
    async fn verdicts(
        existing: &[ExistingChild],
        incoming: &[IncomingIdentity],
    ) -> Vec<MatchVerdict> {
        PositionalExactMatcher
            .match_blocks(MatchContext {
                document_uri: &doc(),
                existing,
                incoming,
                situation: MatchSituation::SyncMerge,
            })
            .await
            .expect("PositionalExactMatcher never errs")
    }

    fn remap(vs: &[MatchVerdict], minted: &str) -> Option<(EntityUri, MatchBasis)> {
        vs.iter().find_map(|v| match v {
            MatchVerdict::Remap {
                minted: m,
                onto,
                basis,
            } if m.id() == minted => Some((onto.clone(), *basis)),
            _ => None,
        })
    }

    fn remap_onto(vs: &[MatchVerdict], minted: &str) -> Option<EntityUri> {
        remap(vs, minted).map(|(onto, _)| onto)
    }

    fn is_ambiguous(vs: &[MatchVerdict], minted: &str) -> bool {
        vs.iter()
            .any(|v| matches!(v, MatchVerdict::MintAmbiguous { minted: m, .. } if m.id() == minted))
    }

    fn is_mint_fresh(vs: &[MatchVerdict], minted: &str) -> bool {
        vs.iter()
            .any(|v| matches!(v, MatchVerdict::MintFresh { minted: m } if m.id() == minted))
    }

    /// The core dup guard: a freshly-minted ID-less headline whose content +
    /// position matches an existing store twin remaps onto that twin (basis
    /// `ContentAtPosition`) -- so the stale re-write updates in place instead
    /// of re-minting (which churns identity or, on base desync,
    /// duplicates).
    #[tokio::test]
    async fn minted_headline_remaps_onto_positional_twin() {
        let d = doc();
        let existing = vec![existing("A", &d, 0, "Prepare personal usage")];
        let incoming = vec![incoming("B", &d, "Prepare personal usage", true)];
        let vs = verdicts(&existing, &incoming).await;
        assert_eq!(
            remap(&vs, "B"),
            Some((EntityUri::block("A"), MatchBasis::ContentAtPosition)),
            "minted headline must reconcile onto its store twin"
        );
        assert!(!is_ambiguous(&vs, "B"));
    }

    /// The flagged caveat: two genuinely-distinct ID-less siblings with
    /// IDENTICAL content match their two twins 1:1 IN ORDER -- never both onto
    /// the first twin (which would drop one) and never merged.
    #[tokio::test]
    async fn two_identical_idless_siblings_match_1to1() {
        let d = doc();
        let existing = vec![existing("A1", &d, 0, "Foo"), existing("A2", &d, 1, "Foo")];
        let incoming = vec![
            incoming("B1", &d, "Foo", true),
            incoming("B2", &d, "Foo", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        assert_eq!(remap_onto(&vs, "B1"), Some(EntityUri::block("A1")));
        assert_eq!(remap_onto(&vs, "B2"), Some(EntityUri::block("A2")));
    }

    /// Fresh ingest (no existing twins): two identical ID-less siblings both
    /// mint (no remap, no false merge) -- they stay two distinct blocks.
    #[tokio::test]
    async fn identical_siblings_without_twins_both_mint() {
        let d = doc();
        let existing: Vec<ExistingChild> = vec![];
        let incoming = vec![
            incoming("B1", &d, "Foo", true),
            incoming("B2", &d, "Foo", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        assert!(is_mint_fresh(&vs, "B1"), "no twins -> B1 mints");
        assert!(is_mint_fresh(&vs, "B2"), "no twins -> B2 mints");
        assert!(remap_onto(&vs, "B1").is_none());
        assert!(!is_ambiguous(&vs, "B1"));
    }

    /// Surplus: one existing twin, two incoming identical ID-less siblings --
    /// the first matches positionally, the second is genuine surplus and mints.
    #[tokio::test]
    async fn surplus_idless_sibling_mints() {
        let d = doc();
        let existing = vec![existing("A1", &d, 0, "Foo")];
        let incoming = vec![
            incoming("B1", &d, "Foo", true),
            incoming("B2", &d, "Foo", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        assert_eq!(remap_onto(&vs, "B1"), Some(EntityUri::block("A1")));
        // B2's content matches A1 but A1 is claimed -> not ambiguous, just mints.
        assert!(is_mint_fresh(&vs, "B2"));
        assert!(!is_ambiguous(&vs, "B2"));
    }

    /// A content match at a DIFFERENT position is ambiguous: we do NOT guess a
    /// merge -- the id mints and is disclosed (MintAmbiguous).
    #[tokio::test]
    async fn content_match_at_other_position_is_ambiguous() {
        let d = doc();
        let existing = vec![existing("A", &d, 0, "Foo"), existing("X", &d, 1, "Bar")];
        // Incoming order swapped: Bar at pos0, Foo at pos1.
        let incoming = vec![
            incoming("Y", &d, "Bar", true),
            incoming("B", &d, "Foo", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        assert!(
            remap_onto(&vs, "Y").is_none(),
            "no positional twin -> no remap"
        );
        assert!(
            remap_onto(&vs, "B").is_none(),
            "no positional twin -> no remap"
        );
        assert!(is_ambiguous(&vs, "Y"));
        assert!(is_ambiguous(&vs, "B"));
    }

    /// Nested: a remapped ID-less parent regroups its subtree onto the existing
    /// parent, so an ID-less child then matches the existing child.
    #[tokio::test]
    async fn nested_child_regroups_after_parent_remap() {
        let d = doc();
        let p_old = EntityUri::block("Bp"); // minted parent id this parse
        let existing = vec![
            existing("P", &d, 0, "Parent"),
            existing("C", &EntityUri::block("P"), 0, "Child"),
        ];
        let incoming = vec![
            incoming("Bp", &d, "Parent", true),
            incoming("Bc", &p_old, "Child", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        assert_eq!(remap_onto(&vs, "Bp"), Some(EntityUri::block("P")));
        assert_eq!(
            remap_onto(&vs, "Bc"),
            Some(EntityUri::block("C")),
            "child must regroup under the remapped parent and match the store child"
        );
    }

    /// An authored (`:ID:`-carrying, non-minted) incoming headline is never
    /// remapped and never appears as a verdict, even if its content collides
    /// with a store sibling -- the by-id diff owns it.
    #[tokio::test]
    async fn authored_headline_is_never_remapped() {
        let d = doc();
        let existing = vec![existing("A", &d, 0, "Foo")];
        let incoming = vec![incoming("authored", &d, "Foo", false)];
        let vs = verdicts(&existing, &incoming).await;
        assert!(vs.is_empty(), "authored (non-minted) yields no verdict");
    }

    // ── Trait-contract tests (apply to every BlockMatchStrategy impl) ──

    /// 1:1 partial matching: no store id is claimed as `onto` by two verdicts.
    #[tokio::test]
    async fn contract_one_to_one_partial_matching() {
        let d = doc();
        let existing = vec![existing("A1", &d, 0, "Foo"), existing("A2", &d, 1, "Foo")];
        // Three identical incoming, only two twins -> at most two remaps, all distinct.
        let incoming = vec![
            incoming("B1", &d, "Foo", true),
            incoming("B2", &d, "Foo", true),
            incoming("B3", &d, "Foo", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        let ontos: Vec<EntityUri> = vs
            .iter()
            .filter_map(|v| match v {
                MatchVerdict::Remap { onto, .. } => Some(onto.clone()),
                _ => None,
            })
            .collect();
        let uniq: HashSet<&EntityUri> = ontos.iter().collect();
        assert_eq!(ontos.len(), uniq.len(), "no store id claimed by two remaps");
        assert_eq!(ontos.len(), 2, "two twins -> exactly two distinct remaps");
    }

    /// Authored ids are never minted: only `minted == true` incoming blocks
    /// appear as a verdict's `minted` id.
    #[tokio::test]
    async fn contract_authored_ids_never_minted() {
        let d = doc();
        let existing = vec![existing("A", &d, 0, "Foo")];
        // B (minted) sits at pos 0 and matches A; the authored block follows at
        // pos 1 with distinct content so it never shifts B's position.
        let incoming = vec![
            incoming("B", &d, "Foo", true),
            incoming("authored", &d, "Bar", false),
        ];
        let vs = verdicts(&existing, &incoming).await;
        assert!(
            vs.iter().all(|v| v.minted().id() != "authored"),
            "authored id must never be a verdict's minted id"
        );
        assert_eq!(remap_onto(&vs, "B"), Some(EntityUri::block("A")));
    }

    /// Exhaustiveness: every minted incoming block receives EXACTLY one
    /// verdict; non-minted blocks receive none.
    #[tokio::test]
    async fn contract_exhaustiveness() {
        let d = doc();
        let existing = vec![existing("A", &d, 0, "Foo")];
        let incoming = vec![
            incoming("m1", &d, "Foo", true),
            incoming("auth", &d, "Foo", false),
            incoming("m2", &d, "Bar", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        let got: HashSet<String> = vs.iter().map(|v| v.minted().id().to_string()).collect();
        let want: HashSet<String> = ["m1", "m2"].iter().map(|s| (*s).to_string()).collect();
        assert_eq!(got, want, "exactly the minted incoming ids get verdicts");
        assert_eq!(vs.len(), 2, "exactly one verdict per minted incoming");
    }

    /// The strategy exposes its provenance tag.
    #[test]
    fn positional_matcher_has_provenance_id() {
        assert_eq!(PositionalExactMatcher.id(), "positional-exact");
    }
}

#[cfg(test)]
mod tiered_matcher_tests {
    use std::collections::HashSet;

    use holon_api::EntityUri;

    use super::BlockMatchStrategy;
    use super::ExistingChild;
    use super::IncomingIdentity;
    use super::MatchBasis;
    use super::MatchContext;
    use super::MatchSituation;
    use super::MatchVerdict;
    use super::TieredMatcher;

    fn doc() -> EntityUri {
        EntityUri::block("doc")
    }

    fn existing(id: &str, parent: &EntityUri, seq: i64, content: &str) -> ExistingChild {
        ExistingChild {
            id: EntityUri::block(id),
            parent: parent.clone(),
            seq,
            content: content.to_string(),
        }
    }

    fn incoming(id: &str, parent: &EntityUri, content: &str, minted: bool) -> IncomingIdentity {
        IncomingIdentity {
            id: EntityUri::block(id),
            parent: parent.clone(),
            content: content.to_string(),
            minted,
        }
    }

    async fn verdicts(
        existing: &[ExistingChild],
        incoming: &[IncomingIdentity],
    ) -> Vec<MatchVerdict> {
        TieredMatcher
            .match_blocks(MatchContext {
                document_uri: &doc(),
                existing,
                incoming,
                situation: MatchSituation::StaleRewrite {
                    idless_fraction: 1.0,
                },
            })
            .await
            .expect("TieredMatcher never errs")
    }

    fn remap(vs: &[MatchVerdict], minted: &str) -> Option<(EntityUri, MatchBasis)> {
        vs.iter().find_map(|v| match v {
            MatchVerdict::Remap {
                minted: m,
                onto,
                basis,
            } if m.id() == minted => Some((onto.clone(), *basis)),
            _ => None,
        })
    }

    fn remap_onto(vs: &[MatchVerdict], minted: &str) -> Option<EntityUri> {
        remap(vs, minted).map(|(onto, _)| onto)
    }

    fn is_mint_fresh(vs: &[MatchVerdict], minted: &str) -> bool {
        vs.iter()
            .any(|v| matches!(v, MatchVerdict::MintFresh { minted: m } if m.id() == minted))
    }

    /// The captured red class: a drawer/insert shifts sibling offsets so the
    /// id-less headline's content matches its store twin at a DIFFERENT
    /// position. Document-unique content -> remap (basis
    /// ContentUniqueInDocument) where v0 minted-ambiguous.
    #[tokio::test]
    async fn shifted_position_unique_content_remaps() {
        let d = doc();
        let existing = vec![existing("A", &d, 0, "Alpha")];
        // A new id-less note is inserted before the twin, pushing "Alpha" to pos 1.
        let incoming = vec![
            incoming("note", &d, "Note", true),
            incoming("i", &d, "Alpha", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        assert_eq!(
            remap(&vs, "i"),
            Some((EntityUri::block("A"), MatchBasis::ContentUniqueInDocument)),
            "shifted unique content must remap onto its twin"
        );
        assert!(
            is_mint_fresh(&vs, "note"),
            "the genuinely-new note mints fresh"
        );
    }

    /// Cross-parent move (new, per the ruling): a block moves under a different
    /// parent, content unique in the document -> remap onto the moved twin.
    #[tokio::test]
    async fn cross_parent_move_remaps() {
        let existing = vec![existing("C", &EntityUri::block("P1"), 0, "Moved")];
        // Same content, now parented under P2 (an external restructure).
        let incoming = vec![incoming("c", &EntityUri::block("P2"), "Moved", true)];
        let vs = verdicts(&existing, &incoming).await;
        assert_eq!(
            remap(&vs, "c"),
            Some((EntityUri::block("C"), MatchBasis::ContentUniqueInDocument)),
            "document-unique content must remap across a parent change"
        );
    }

    /// RULING A2 (was `reordered_identical_twins_both_mint_ambiguous`,
    /// semantics FLIPPED): two identical LEAF twins whose positions all
    /// shifted no longer mint-ambiguous. Their subtrees are equal (empty),
    /// so they are interchangeable and pair by relative position -- both
    /// remap onto distinct store ids, neither mints.
    #[tokio::test]
    async fn reordered_identical_twins_pair_by_position() {
        let d = doc();
        let existing = vec![existing("A", &d, 0, "Dup"), existing("X", &d, 1, "Dup")];
        // Both incoming twins land under a different parent -> no positional twin.
        let other = EntityUri::block("other");
        let incoming = vec![
            incoming("i1", &other, "Dup", true),
            incoming("i2", &other, "Dup", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        assert_eq!(
            remap_onto(&vs, "i1"),
            Some(EntityUri::block("A")),
            "first twin pairs onto the lowest-position store twin"
        );
        assert_eq!(
            remap_onto(&vs, "i2"),
            Some(EntityUri::block("X")),
            "second twin pairs onto the remaining store twin"
        );
        assert!(!is_mint_fresh(&vs, "i1") && !is_mint_fresh(&vs, "i2"));
    }

    /// RULING A2 signature discrimination: same-content twins with DIFFERENT
    /// subtrees pair by descendant signature, crossing positions -- the twin
    /// carrying `childB` reconciles onto the store twin that owns `childB`, not
    /// the positionally-first one. This is what stops children being re-homed
    /// onto the wrong same-content twin.
    #[tokio::test]
    async fn twins_with_distinct_subtrees_pair_by_signature() {
        let d = doc();
        let other = EntityUri::block("other");
        // Store: twin A owns childA (seq 0), twin X owns childB (seq 1).
        let existing = vec![
            existing("A", &d, 0, "Dup"),
            existing("ca", &EntityUri::block("A"), 0, "childA"),
            existing("X", &d, 1, "Dup"),
            existing("cb", &EntityUri::block("X"), 0, "childB"),
        ];
        // Incoming (reordered): i1 carries childB, i2 carries childA.
        let incoming = vec![
            incoming("i1", &other, "Dup", true),
            incoming("i1c", &EntityUri::block("i1"), "childB", true),
            incoming("i2", &other, "Dup", true),
            incoming("i2c", &EntityUri::block("i2"), "childA", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        assert_eq!(
            remap(&vs, "i1"),
            Some((EntityUri::block("X"), MatchBasis::SubtreeSignature)),
            "the childB-carrying twin must pair onto the store twin that owns childB"
        );
        assert_eq!(
            remap(&vs, "i2"),
            Some((EntityUri::block("A"), MatchBasis::SubtreeSignature)),
            "the childA-carrying twin must pair onto the store twin that owns childA"
        );
    }

    /// RULING A2 (was `incoming_side_duplicates_mint`, semantics FLIPPED): one
    /// store twin, two incoming id-less blocks with that content. The first
    /// pairs onto the store twin; the second has NO unclaimed candidate left
    /// and mints fresh (genuinely new).
    #[tokio::test]
    async fn incoming_side_duplicate_pairs_one_and_mints_the_rest() {
        let d = doc();
        let existing = vec![existing("A", &d, 0, "Solo")];
        let other = EntityUri::block("other");
        let incoming = vec![
            incoming("i1", &other, "Solo", true),
            incoming("i2", &other, "Solo", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        assert_eq!(
            remap_onto(&vs, "i1"),
            Some(EntityUri::block("A")),
            "the first incoming twin pairs onto the sole store twin"
        );
        assert!(
            is_mint_fresh(&vs, "i2"),
            "the surplus twin has no candidate left -> mints fresh"
        );
    }

    /// Claimed-id exclusion: an existing id matched verbatim by an authored
    /// (non-minted) incoming block is claimed; an id-less block with the same
    /// content cannot steal it -> mints fresh.
    #[tokio::test]
    async fn claimed_id_excluded_from_remap() {
        let d = doc();
        let existing = vec![existing("A", &d, 0, "Foo")];
        let incoming = vec![
            // Authored block carrying id A claims the twin.
            incoming("A", &d, "Foo", false),
            incoming("i", &d, "Foo", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        assert!(
            remap_onto(&vs, "i").is_none(),
            "claimed twin cannot be remapped onto"
        );
        assert!(is_mint_fresh(&vs, "i"), "no unclaimed twin -> mint fresh");
        assert!(
            vs.iter().all(|v| v.minted().id() != "A"),
            "authored id never a verdict's minted id"
        );
    }

    /// RULING A2 (was `document_wide_duplicate_content_is_ambiguous`, semantics
    /// FLIPPED): the same content appears under multiple store parents; a
    /// single id-less incoming with that content no longer mints-ambiguous.
    /// Both store twins are leaf/interchangeable, so it pairs onto the
    /// lowest-position one (A) -- the other is left for the whole-doc diff
    /// to converge/delete.
    #[tokio::test]
    async fn document_wide_duplicate_content_pairs_by_position() {
        let existing = vec![
            existing("A", &EntityUri::block("P1"), 0, "Foo"),
            existing("B", &EntityUri::block("P2"), 0, "Foo"),
        ];
        let incoming = vec![incoming("i", &doc(), "Foo", true)];
        let vs = verdicts(&existing, &incoming).await;
        assert_eq!(
            remap_onto(&vs, "i"),
            Some(EntityUri::block("A")),
            "single incoming twin pairs onto the lowest (seq, id) store twin"
        );
        assert!(
            !is_mint_fresh(&vs, "i"),
            "a candidate exists -> never mints"
        );
    }

    // ── Trait-contract tests (same contract as v0) ──

    #[tokio::test]
    async fn contract_one_to_one_partial_matching() {
        let d = doc();
        let existing = vec![existing("A1", &d, 0, "Foo"), existing("A2", &d, 1, "Foo")];
        let incoming = vec![
            incoming("B1", &d, "Foo", true),
            incoming("B2", &d, "Foo", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        let ontos: Vec<EntityUri> = vs
            .iter()
            .filter_map(|v| match v {
                MatchVerdict::Remap { onto, .. } => Some(onto.clone()),
                _ => None,
            })
            .collect();
        let uniq: HashSet<&EntityUri> = ontos.iter().collect();
        assert_eq!(ontos.len(), uniq.len(), "no store id claimed by two remaps");
    }

    #[tokio::test]
    async fn contract_authored_ids_never_minted() {
        let d = doc();
        let existing = vec![existing("A", &d, 0, "Foo")];
        let incoming = vec![
            incoming("B", &d, "Foo", true),
            incoming("authored", &d, "Bar", false),
        ];
        let vs = verdicts(&existing, &incoming).await;
        assert!(vs.iter().all(|v| v.minted().id() != "authored"));
    }

    #[tokio::test]
    async fn contract_exhaustiveness() {
        let d = doc();
        let existing = vec![existing("A", &d, 0, "Foo")];
        let incoming = vec![
            incoming("m1", &d, "Foo", true),
            incoming("auth", &d, "Foo", false),
            incoming("m2", &d, "Bar", true),
        ];
        let vs = verdicts(&existing, &incoming).await;
        let got: HashSet<String> = vs.iter().map(|v| v.minted().id().to_string()).collect();
        let want: HashSet<String> = ["m1", "m2"].iter().map(|s| (*s).to_string()).collect();
        assert_eq!(got, want, "exactly the minted incoming ids get verdicts");
        assert_eq!(vs.len(), 2);
    }

    #[test]
    fn tiered_matcher_has_provenance_id() {
        assert_eq!(TieredMatcher.id(), "tiered-v1");
    }
}

#[cfg(test)]
mod shared_projection_guard_tests {
    use holon_api::EntityUri;
    use holon_api::block::Block;
    use holon_api::share_props::SHARE_ROLE_MOUNT;
    use holon_api::share_props::SHARE_ROLE_PROPERTY;
    use holon_api::share_props::SHARED_TREE_ID_PROPERTY;

    use super::is_shared_subtree_projection;

    fn page(id: &str) -> Block {
        let mut b = Block::new_text(EntityUri::block(id), EntityUri::no_parent(), "Page");
        b.set_page(true);
        b
    }

    // A normal page + normal blocks are NOT a shared projection — must ingest.
    #[test]
    fn normal_file_is_not_a_shared_projection() {
        let doc = page("doc");
        let blocks = vec![Block::new_text(
            EntityUri::block("b1"),
            EntityUri::block("doc"),
            "hi",
        )];
        assert!(!is_shared_subtree_projection(&doc, &blocks));
    }

    // The page block IS the mount (adopt-and-collapse page share) → skip ingest.
    #[test]
    fn mount_page_is_a_shared_projection() {
        let mut doc = page("mount");
        doc.set_property(SHARE_ROLE_PROPERTY, SHARE_ROLE_MOUNT);
        doc.set_property(SHARED_TREE_ID_PROPERTY, "stid-1");
        assert!(is_shared_subtree_projection(&doc, &[]));
    }

    // Synthetic-container share: the page is a plain synthetic page, but a child
    // carries the mount role / shared-tree-id stamp → skip ingest.
    #[test]
    fn descendant_with_share_stamp_is_a_shared_projection() {
        let doc = page("synthetic");
        let mut child = Block::new_text(
            EntityUri::block("shared-root"),
            EntityUri::block("synthetic"),
            "shared",
        );
        child.set_property(SHARED_TREE_ID_PROPERTY, "stid-2");
        assert!(is_shared_subtree_projection(&doc, &[child]));
    }
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
