//! Consumer-side ports for the format-agnostic file-sync engine.
//!
//! `FileSyncController` reads/writes blocks and documents, and registers
//! doc-id → path aliases, only through these traits — it never imports a
//! concrete storage backend (Loro, Turso, in-memory). The impls live in the
//! DI wiring layer of each backend crate.

use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use holon_api::EntityUri;
use holon_api::block::Block;

/// Read-only access to blocks, organized by document.
///
/// The org crate uses this to render blocks → org text without knowing
/// how blocks are stored (Loro, in-memory, SQL, etc.).
#[async_trait]
pub trait BlockReader: Send + Sync {
    /// Get all blocks for a document by its ID.
    ///
    /// # Ordering guarantee: WITHIN-PARENT relative order ONLY
    ///
    /// The returned `Vec` is a **set with within-parent relative order**, NOT a
    /// document sequence. Treating consecutive elements as adjacent in the
    /// document is a bug — group by [`Block::parent_id`] first.
    ///
    /// The two implementations legitimately disagree on the overall sequence,
    /// and both satisfy this contract:
    /// - `LoroBlockReader::collect_subtree` — pre-order DFS, so its sequence
    ///   *is* document order (incidentally, not by contract).
    /// - `CacheBlockReader` (Turso) — `ORDER BY sort_key, id`, a GLOBAL sort.
    ///   Fractional indices are minted **per sibling group**, so every group
    ///   restarts at the same low key, rows from different parents tie, and the
    ///   `id` tiebreak interleaves them. A depth-2 grandchild can sort ahead of
    ///   its own grandparent's siblings.
    ///
    /// This is safe because the only consumer that reads order — the org
    /// renderer (`render_entitys`) — re-nests by `parent_id` and reads just the
    /// within-parent order, which both impls preserve. Rendering is
    /// byte-identical from either sequence (proven by
    /// `holon-org-format/tests/get_blocks_flat_order_is_not_document_order.
    /// rs`).
    ///
    /// Per ADR 0005 the canonical order authority is
    /// `BlockOrdering::children`; `sort_key` is a storage encoding chosen by
    /// one adapter and is not a cross-parent ordering key.
    async fn get_blocks(&self, doc_id: &EntityUri) -> Result<Vec<Block>>;

    /// The document's SHAPE: one `(id, parent_id)` pair per block
    /// `get_blocks(doc_id)` would return, children-only, WITHOUT hydrating a
    /// single block body.
    ///
    /// Exists for the write-back fold-completeness gate, which runs on every
    /// block delta. Routing it through `get_blocks` would put full edge
    /// hydration back on the per-edit path — the cost Option C removed and
    /// `edits_render_from_the_holder_and_fire_zero_get_blocks` pins at zero.
    ///
    /// # Why parents, not just ids
    ///
    /// Membership equality is NOT fold-completeness. A holder can hold exactly
    /// the right id set while one member still carries its PRE-MOVE parent —
    /// and that member is then unreachable from the document root, gets
    /// excluded from the render, and the removal guard vetoes a block that
    /// genuinely belongs to the file. Measured at 55% of runs on one keystone
    /// case before the parent was compared. The parent is what makes "the fold
    /// finished" checkable.
    ///
    /// # No default implementation, deliberately
    ///
    /// A defaulted `Ok(vec![])` would make the gate compare an empty holder
    /// against an empty authority, PASS, and render an empty document over a
    /// populated file — silently reintroducing the data-loss shape the gate
    /// exists to prevent. An implementation that genuinely cannot answer must
    /// return `Err`, never an empty set.
    async fn doc_block_topology(&self, doc_id: &EntityUri) -> Result<Vec<(EntityUri, EntityUri)>>;

    /// Authoritative single-block point read, fully edge-hydrated
    /// (`tags`/`requires`/`advice_suppressed`). Reads the **write-authority**
    /// store — `block_raw` under Turso, the Loro tree under Loro — NOT the
    /// lagging `block` matview, and NOT the doc-scoped recursive-CTE walk. An
    /// O(1) indexed lookup.
    ///
    /// Powers the org-writeback incremental cache: after a content-only block
    /// edit, the controller refreshes just the changed block from the same
    /// authority its cache was seeded from (`get_blocks` reads `block_raw`
    /// too), so seed and refresh never straddle the matview-vs-`block_raw`
    /// authority boundary. Returns `None` if the id is absent.
    async fn get_block_authoritative(&self, id: &EntityUri) -> Result<Option<Block>>;

    /// List all known documents with their blocks (for startup initialization).
    /// Returns (doc_id, blocks) pairs. Path resolution is the caller's concern.
    async fn iter_documents_with_blocks(&self) -> Result<Vec<(EntityUri, Vec<Block>)>>;

    /// Upgrade dangling `EntityRef::Name` link marks in `blocks` to the
    /// resolved `EntityRef::Scheme` form, so the render emits the ratified
    /// `[[<id>][<label>]]` instead of the bare `[[<label>]]`.
    ///
    /// # Why this is a render step and not part of the read
    ///
    /// The upgrade is **byte-visible in org output** but the stored marks are
    /// never touched — it is a property of *rendering*, not of any particular
    /// read. Hanging it off the reads instead made the output depend on where
    /// the values happened to come from: a `Block` from `get_blocks` rendered
    /// the resolved form, the byte-identical `Block` taken from the block feed
    /// rendered the bare form. Applying it to the slice being rendered is what
    /// makes value provenance stop mattering.
    ///
    /// Required, deliberately: a defaulted no-op is how a backend silently
    /// loses the upgrade. An implementation that cannot resolve must say so
    /// and state the consequence.
    async fn resolve_link_marks(&self, blocks: &mut [Block]) -> Result<()>;

    /// Load `(file_id, content_hash)` pairs from the `file` table. Used by
    /// `FileSyncController` at startup to populate the projection-hash cache
    /// (`last_projection_hash`) before CDC has replayed file events into
    /// the in-process cache — so we can short-circuit on-disk-unchanged
    /// re-ingests without doing a single block-table SQL op.
    ///
    /// Returns the URI form (e.g. `file:projects/todo.org`) since the
    /// caller knows its own root directory and can resolve to a concrete
    /// `CanonicalPath`. Default impl returns empty so backends without a
    /// `file` table (in-memory tests) don't have to implement it.
    async fn load_file_hashes(&self) -> Result<Vec<(EntityUri, String)>> {
        Ok(Vec::new())
    }

    /// Phase 1 write-back: persist `content_hash` for a file row so the
    /// next boot's `load_file_hashes` returns the new value and the
    /// fast-path engages. Returns `Ok(())` whether the row existed or
    /// not — `OrgmodeSyncProvider` is the authoritative creator of file
    /// rows, and on first ingest the row may not yet exist; the next
    /// provider sync will create it and we'll succeed on a later
    /// re-ingest. Default impl is a no-op for backends without a `file`
    /// table.
    async fn persist_file_hash(&self, _: &EntityUri, _: &str) -> Result<()> {
        Ok(())
    }

    /// Phase 5 keystone: wait until the convergent block feed
    /// (`LiveData<Block>`, the `block` matview CDC stream) contains every id in
    /// `block_ids`. Returns `true` if all are present before `timeout_ms`,
    /// `false` on timeout.
    ///
    /// This replaced the `event_acks` watermark wait
    /// (`wait_for_cache_caught_up`): that wait was only a *timestamp proxy*
    /// for "the projection that produced these blocks has landed", whereas
    /// this is a *positional* predicate over the actual rows — it cannot
    /// return before the rows are visible, so it closes the load-bearing
    /// stale-cache re-render race (R1) without the watermark machinery.
    /// Default impl returns `true` for backends without a feed (in-memory
    /// tests).
    async fn wait_for_blocks_in_feed(&self, _: &[String], _: u64) -> bool {
        true
    }

    /// How many of `block_ids` are currently present in the convergent feed.
    ///
    /// Powers the controller's progress-grounded feed barrier
    /// (`FileSyncController::wait_for_feed_progress`): as long as this count
    /// keeps rising between `wait_for_blocks_in_feed` slices, the barrier keeps
    /// waiting — completion is grounded in the feed's own progress/quiescence,
    /// never in total wall-clock elapsed (the fixed 2s ceiling expired early on
    /// real-vault cold boots, BugFunnel 2026-07-12). Backends without a feed
    /// report all present, matching the `wait_for_blocks_in_feed` default.
    async fn blocks_in_feed_count(&self, block_ids: &[String]) -> usize {
        block_ids.len()
    }

    /// Check if any of the given block IDs already exist under a DIFFERENT
    /// document. Returns Vec<(block_id, owning_doc_uri)> for conflicts
    /// found.
    ///
    /// Default implementation uses `iter_documents_with_blocks()` to correctly
    /// attribute nested blocks to their document root (not just direct parent).
    async fn find_foreign_blocks(
        &self,
        block_ids: &[EntityUri],
        expected_doc_uri: &EntityUri,
    ) -> Result<Vec<(EntityUri, EntityUri)>> {
        if block_ids.is_empty() {
            return Ok(Vec::new());
        }

        let id_set: std::collections::HashSet<&EntityUri> = block_ids.iter().collect();
        let documents = self.iter_documents_with_blocks().await?;

        let mut conflicts = Vec::new();
        for (doc_uri, blocks) in &documents {
            if doc_uri == expected_doc_uri {
                continue;
            }
            for block in blocks {
                if id_set.contains(&block.id) {
                    conflicts.push((block.id.clone(), doc_uri.clone()));
                }
            }
        }
        Ok(conflicts)
    }
}

/// CRUD operations on page blocks (blocks tagged `"Page"`).
///
/// Convenience methods (`name_chain`, `find_by_name_chain`,
/// `get_or_create_by_name_chain`) are file-format agnostic — reusable by org,
/// markdown, or any file-based adapter. The "name" of a page is now its title:
/// the first line of `content`.
#[async_trait]
pub trait DocumentManager: Send + Sync {
    /// Find a page block by parent_id and title (first line of content).
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> Result<Option<Block>>;

    /// Create a new page block.
    async fn create(&self, doc: Block) -> Result<Block>;

    /// Create a page block, FORCING `doc.id` as the page identity — never
    /// substituting a pre-existing same-`(parent, title)` page.
    ///
    /// Used by org ingest when the file carries an authoritative `#+ID`: that
    /// id IS the page's identity (parse-don't-validate). A name-chain
    /// placeholder minted with a RANDOM id for the same directory by an
    /// earlier-scanned sibling file must NOT win — if it did, writeback would
    /// render the placeholder's id as the file's `#+ID`, silently re-minting
    /// (and thus breaking every reference to) the document's identity.
    ///
    /// Default delegates to [`create`](Self::create); stores whose `create`
    /// de-duplicates by `(parent, title)` override this to bypass that
    /// substitution and honor the supplied `doc.id`.
    async fn create_forcing_id(&self, doc: Block) -> Result<Block> {
        self.create(doc).await
    }

    /// Get a page block by its ID.
    async fn get_by_id(&self, id: &EntityUri) -> Result<Option<Block>>;

    /// Update page metadata (e.g. todo_keywords) on the page block.
    async fn update_metadata(&self, doc: &Block) -> Result<()>;

    /// Walk parent chain to root, return title segments: ["projects", "todo"]
    ///
    /// **No-pages-under-non-pages (2026-07-13 interim ruling,
    /// `docs/Proposals/PageHierarchy-2026-07-13.md`; enforced Fork B B1):** a
    /// page's structural ancestors, up to a root, must themselves all be pages.
    /// Every ancestor iteration (NOT the starting `doc_id` itself — that block
    /// is asserted `is_page()` separately by the one caller that matters,
    /// `doc_id_to_path`, so a non-page `doc_id` fails loud there rather than
    /// silently truncating its own segment here) that is not a page is a
    /// **fail-loud error**, never a silent skip — mirrors the existing
    /// `seed_routing_doc` precedent (`holon-app/src/seed.rs`) which asserts the
    /// same invariant for the seed set. A caller must not reparent, coerce, or
    /// slug around this; propagate the error (see `doc_id_to_path`).
    ///
    /// **Empty titles are rejected, never returned.** A page whose title is
    /// empty (e.g. a never-completed placeholder root) names no directory
    /// entry; returning it would let `root.join(chain)` resolve OUTSIDE the
    /// vault. Fail loud so the offending page is named, rather than sanitized
    /// away — see [`VaultPath`](crate::VaultPath).
    ///
    /// Root stop recognizes BOTH `no_parent()` and the broader `is_sentinel()`
    /// (synthetic ids) as valid roots — matching the loop's own break
    /// condition, so a legitimate sentinel-rooted chain is never mistaken for a
    /// non-page ancestor.
    async fn name_chain(&self, doc_id: &EntityUri) -> Result<Vec<String>> {
        let mut chain = Vec::new();
        let mut current_id = doc_id.clone();
        let mut is_self = true;

        loop {
            if current_id == EntityUri::no_parent() || current_id.is_sentinel() {
                break;
            }

            let doc = match self.get_by_id(&current_id).await? {
                Some(doc) => doc,
                None if is_self => {
                    // The page store tracks ONLY `Page`-tagged blocks (e.g.
                    // `LiveDocumentManager`'s `WHERE tag='Page'` matview). A miss
                    // on the STARTING block therefore means it is simply not a
                    // page — it owns no file. Resolve to an empty chain;
                    // `doc_id_to_path` maps that to `Ok(None)` ("not a page,
                    // skip"). This is the folder-companion de-inline case: the
                    // non-page content headings of a de-inlined child page are
                    // absent from the companion's projection, and write-back
                    // grounding must NOT treat them as an unresolvable hard-veto
                    // (BugFunnel row 23/29 — the 749-error `Projects.org` storm).
                    return Ok(Vec::new());
                }
                None => {
                    // A miss on an ANCESTOR (we are walking up from a real page)
                    // means that page's structural ancestry is broken — its
                    // parent is not itself a page in the store. That IS a
                    // fail-loud error (interim ruling 2026-07-13).
                    anyhow::bail!(
                        "name_chain({doc_id}): ancestor '{current_id}' of a page is absent from \
                         the page store — a page's structural ancestors must themselves all be \
                         pages (interim ruling 2026-07-13); chain so far (root-to-here, reversed \
                         at return) = {:?}",
                        chain
                    );
                }
            };

            if doc.is_page() {
                let title = doc.title();
                if title.is_empty() {
                    anyhow::bail!(
                        "name_chain({doc_id}): page '{current_id}' has an EMPTY title and so \
                         contributes no path segment — no page file can be named for it inside \
                         the vault root (an empty segment escapes it); chain so far \
                         (root-to-here, reversed at return) = {:?}",
                        chain
                    );
                }
                chain.push(title);
            } else if !is_self {
                anyhow::bail!(
                    "name_chain({doc_id}): non-page ancestor '{current_id}' found while walking \
                     to root — pages under non-pages are prohibited (interim ruling 2026-07-13); \
                     chain so far (root-to-here, reversed at return) = {:?}",
                    chain
                );
            }
            is_self = false;
            current_id = doc.parent_id.clone();
        }

        chain.reverse();
        Ok(chain)
    }

    /// Resolve a title chain to a page Block: ["projects", "todo"] → Block
    async fn find_by_name_chain(&self, chain: &[&str]) -> Result<Option<Block>> {
        let mut current_parent_id = EntityUri::no_parent();
        let mut current_doc: Option<Block> = None;

        for segment in chain {
            match self
                .find_by_parent_and_name(&current_parent_id, segment)
                .await?
            {
                Some(doc) => {
                    current_parent_id = doc.id.clone();
                    current_doc = Some(doc);
                }
                None => return Ok(None),
            }
        }

        Ok(current_doc)
    }

    /// Get or create the full chain, creating intermediate page blocks as
    /// needed.
    async fn get_or_create_by_name_chain(&self, chain: &[&str]) -> Result<Block> {
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
            match self
                .find_by_parent_and_name(&current_parent_id, segment)
                .await?
            {
                Some(existing) => {
                    current_parent_id = existing.id.clone();
                    current_doc = Some(existing);
                }
                None => {
                    // DETERMINISTIC page id keyed on the accumulated name-chain
                    // path (`Life/Areas`), minted through the single `PageId`
                    // constructor the link-create op also uses. An org file page
                    // and a `[[Areas]]` link-created page for the same path now
                    // converge on one CRDT merge key (inv-page-name-unique)
                    // instead of each peer minting a random UUID.
                    let page_id = holon_api::link_parser::PageId::for_path(&accumulated)
                        .map_err(anyhow::Error::msg)?
                        .into_entity_uri();
                    let mut new_doc =
                        Block::new_text(page_id, current_parent_id.clone(), segment.to_string());
                    new_doc.set_page(true);
                    let created = self.create(new_doc).await?;
                    current_parent_id = created.id.clone();
                    current_doc = Some(created);
                }
            }
        }

        Ok(current_doc.unwrap())
    }
}

/// Read/write binary image data for blocks.
///
/// Decoupled from Loro — the DI layer provides the concrete implementation.
/// The org sync controller uses this to materialize image files to disk
/// (block → file) and ingest them back (file → block).
#[async_trait]
pub trait ImageDataProvider: Send + Sync {
    /// Read image bytes for a block. Returns None if no data is stored.
    async fn read_image_data(&self, block_id: &EntityUri) -> Result<Option<Vec<u8>>>;

    /// Store image bytes for a block.
    async fn write_image_data(&self, block_id: &EntityUri, data: Vec<u8>) -> Result<()>;
}

/// Callback for registering doc_id → path aliases in the storage layer.
/// Implemented by Loro wiring; the controller itself doesn't know about Loro.
#[async_trait]
pub trait AliasRegistrar: Send + Sync {
    async fn register_alias(&self, doc_id: &EntityUri, path: &Path);
    async fn resolve_alias_to_path(&self, doc_id: &EntityUri) -> Option<PathBuf>;
}

/// 3-way text-content merge for the no-store conflict path (SqlOnly /
/// `Consolidator::Store` mode).
///
/// When an org-file edit races a UI edit to the SAME block's text content, the
/// store-owner mode has no live CRDT to merge them, so a naive ingest resolves
/// by whole-value last-writer-wins — one side's characters are lost. The
/// merge-fidelity ladder (`docs/Architecture/Model.md`) says this mode must
/// instead degrade to a base-3-way TEXT merge: the org reconciler already holds
/// the common ancestor (the last-projected snapshot, read through the
/// [`BaseStore`](crate::BaseStore) seam), so it feeds `(base, theirs, mine)`
/// through this seam and ingests the merged text.
///
/// The merge is performed by a **transient** CRDT text — a throwaway document
/// spun up per conflict and dropped afterwards (Model.md: *transient = merge
/// function only*, never stored, never cached). The concrete impl lives in
/// holon-loro; holon-filesystem stays storage-agnostic and never depends on
/// Loro (holon-loro already depends on holon-filesystem — the reverse would be
/// a cycle).
pub trait ThreeWayTextMerge: Send + Sync {
    /// Merge concurrent edits of one block's text content. `base` is the common
    /// ancestor (last-projected snapshot from the BaseStore), `theirs` the
    /// on-disk edit, `mine` the current store content. Returns the merged text.
    /// Callers invoke this only when BOTH `theirs` and `mine` differ from
    /// `base` (a genuine concurrent edit); the non-conflict cases never reach
    /// here.
    fn merge_text(&self, base: &str, theirs: &str, mine: &str) -> Result<String>;
}

/// Disclosure seam for shared-subtree write-back gaps.
///
/// Edits to a block inside a shared/mounted subtree route into the shared Loro
/// doc + SQL and sync to peers (that path works), but until the mount is
/// materialized as a page-file the write-back layer cannot resolve a disk path
/// for the shared content — so on-disk org is stale. That gap must be
/// DISCLOSED, never silently dropped (`docs/Architecture/Model.md` inv-11 +
/// CLAUDE.md fail-loud). The concrete impl lives in the app wiring layer and
/// forwards to `holon-loro`'s `DegradedSignalBus`; holon-filesystem /
/// holon-orgmode stay storage-agnostic (they do not depend on holon-loro — the
/// reverse would be a cycle), mirroring [`ThreeWayTextMerge`].
pub trait ShareWritebackDisclosure: Send + Sync {
    /// Signal that `block_id` (belonging to share `shared_tree_id`) was edited
    /// but could not be materialized to a dedicated on-disk org file. Emit a
    /// user-visible degraded banner; the edit itself is safe in Loro + SQL.
    fn shared_subtree_not_materialized(&self, block_id: &EntityUri, shared_tree_id: &str);
}

/// Disclosure seam for the write-back stream giving up entirely.
///
/// Its own port rather than a call onto the degraded-signal bus, because the
/// bus type lives downstream (holon-loro) of everything that can raise this:
/// the supervisor sits in holon-orgmode, which must not learn about a concrete
/// bus to be able to shout. Same shape and same reason as
/// [`ShareWritebackDisclosure`]; the wiring crate bridges both.
pub trait WritebackDisclosure: Send + Sync {
    /// Signal that org write-back is DOWN — the supervised stream died more
    /// often than its restart budget allows, so edits reach Loro + SQL but stop
    /// reaching disk. `detail` carries the supervisor's escalation summary
    /// (what died, how many restarts it spent). Sticky: nothing lifts it but a
    /// successful respawn, which is accurate — the disk projection stays stale
    /// for exactly that long.
    fn writeback_degraded(&self, detail: &str);
}

/// Authoritative "is this block id a registered shared-subtree mount?" seam.
///
/// The Inc 3 ingest guard must decide whether an org file is a shared-subtree
/// PROJECTION SINK (skip ingest — its truth is the shared Loro doc) purely from
/// AUTHORITATIVE state, never from parsed drawer content: `share-role`/
/// `shared-tree-id` drawer properties round-trip verbatim from ANY user file,
/// so keying on them (even after they land in SQL) would let a hand-authored
/// `:share-role: mount:` file be silently skipped — a page that never loads and
/// edits that vanish. The sound signal is a real mount NODE in the global Loro
/// tree (created only by share/accept, non-user-authorable). The concrete impl
/// lives in the wiring layer over `holon-loro`; holon-filesystem stays
/// storage-agnostic (mirrors [`ThreeWayTextMerge`]). Absent seam (SqlOnly /
/// tests) ⇒ the guard treats nothing as a projection and ingests normally.
#[async_trait]
pub trait MountRegistry: Send + Sync {
    /// True iff `block_id` is an authoritatively-registered shared-subtree
    /// mount (a real mount node in the global tree), NOT merely a block
    /// carrying a user-authored `share-role` drawer property.
    async fn is_registered_mount(&self, block_id: &EntityUri) -> Result<bool>;
}

// ── Block-matching strategy seam (ID-less external-rewrite reconcile) ──
//
// When an external editor re-writes a doc's file with block `:ID:` drawers
// stripped, every id-less headline is minted a FRESH uuid on parse. Before the
// id-keyed diff runs, the controller must decide -- per minted headline --
// whether it reconciles onto an already-minted STORE twin (remap) or is a
// genuinely new block (mint). That decision is the matching SPECTRUM (exact
// position -> content-unique-in-siblings -> content-unique-in-document ->
// fuzzy/embedding/LLM tie-breakers); this port lets the strategy evolve behind
// a stable call site. The house pattern mirrors [`ThreeWayTextMerge`] /
// [`MountRegistry`]: `Arc<dyn ...>`, builder injection, storage-agnostic.

/// One existing store child, as the ID-less reconcile sees it.
#[derive(Debug, Clone)]
pub struct ExistingChild {
    pub id: EntityUri,
    pub parent: EntityUri,
    /// The block's 0-based DOCUMENT-ORDER position within its parent: dense
    /// per parent, derived from the canonical `get_blocks` ordering
    /// (`ORDER BY sort_key, id`). Deliberately NOT the org parser's
    /// `sequence` property.
    pub seq: i64,
    pub content: String,
}

/// One incoming parsed block's identity, as the ID-less reconcile sees it.
#[derive(Debug, Clone)]
pub struct IncomingIdentity {
    pub id: EntityUri,
    /// Top-level headlines are normalised by the caller to the document uri so
    /// they group with the store's top-level children.
    pub parent: EntityUri,
    pub content: String,
    /// The id was freshly minted this parse (an ID-less headline).
    pub minted: bool,
}

/// Situation the reconcile finds itself in -- detected by the controller from
/// data already in hand, consumed by strategies (composites may branch on it).
/// Ships in the context from day one so a future situational strategy needs no
/// call-site change; the v0 strategy ignores it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MatchSituation {
    /// No existing store children for this document (first ingest).
    PristineIngest,
    /// Incoming parse is wholly/mostly id-less while the store is id-full
    /// (the StaleExternalRewrite shape).
    StaleRewrite { idless_fraction: f64 },
    /// Mixed ids, normal external edit.
    SyncMerge,
}

/// The basis on which a minted headline was reconciled onto a store id --
/// provenance for the WARN/history trail and for auditing spectrum moves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MatchBasis {
    IdIdentity,
    ContentAtPosition,
    ContentUniqueInSiblings,
    ContentUniqueInDocument,
    /// RULING A2: same-content twins tie-broken by an equal DESCENDANT subtree
    /// signature (the incoming block's whole subtree matches this store
    /// twin's), so the twin keeps its own children instead of being
    /// re-homed.
    SubtreeSignature,
    /// RULING A2: same-content twins with no signature match, paired by
    /// relative sibling position (identical-subtree twins are
    /// interchangeable).
    ContentAtRelativePosition,
}

/// Parse-don't-validate: every minted incoming id gets an explicit verdict;
/// ambiguity is a typed variant, not a side-channel WARN.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchVerdict {
    /// Reconcile the minted id onto an existing store id.
    Remap {
        minted: EntityUri,
        onto: EntityUri,
        basis: MatchBasis,
    },
    /// Content matched an existing sibling but not deterministically (e.g. at a
    /// different position, or a document-wide collision) -- mint, caller WARNs
    /// loudly with the candidate ids.
    MintAmbiguous {
        minted: EntityUri,
        candidates: Vec<EntityUri>,
    },
    /// Genuinely new block -- mint a fresh id.
    MintFresh { minted: EntityUri },
}

impl MatchVerdict {
    /// The minted incoming id this verdict concerns.
    pub fn minted(&self) -> &EntityUri {
        match self {
            MatchVerdict::Remap { minted, .. }
            | MatchVerdict::MintAmbiguous { minted, .. }
            | MatchVerdict::MintFresh { minted } => minted,
        }
    }
}

/// Inputs to a matching decision. `existing` is the store's CURRENT children
/// (PR #81 invariant -- NOT the diff base, which desyncs in the duplicating
/// case); `incoming` is document-order, parents-first.
pub struct MatchContext<'a> {
    pub document_uri: &'a EntityUri,
    pub existing: &'a [ExistingChild],
    pub incoming: &'a [IncomingIdentity],
    pub situation: MatchSituation,
}

/// Decide, per minted (id-less) incoming headline, whether it reconciles onto a
/// store twin or mints. Implementations MUST honour the contract:
/// - **1:1 partial matching**: no `onto` id claimed by two verdicts, and ids
///   the incoming set already matches verbatim are never re-remapped.
/// - **authored ids are never minted**: only `incoming` blocks with `minted ==
///   true` appear as a verdict's `minted` id.
/// - **exhaustiveness**: every minted incoming block receives exactly one
///   verdict (parse, don't validate -- ambiguity is a variant, not an
///   omission).
///
/// Async + `Result` keep the door open for future strategies that do embedding
/// lookups / LLM adjudication, and keep fail-loud (a strategy that cannot
/// decide errs, it never guesses silently).
#[async_trait]
pub trait BlockMatchStrategy: Send + Sync {
    /// Provenance tag recorded alongside the decision.
    fn id(&self) -> &'static str;
    async fn match_blocks(&self, ctx: MatchContext<'_>) -> Result<Vec<MatchVerdict>>;
}

#[cfg(test)]
mod name_chain_tests {
    //! Red-first coverage for the no-pages-under-non-pages ruling
    //! (`docs/Proposals/PageHierarchy-2026-07-13.md`, Fork B B1, §3/§5 item 1).
    //!
    //! Before the fix, `name_chain` SILENTLY dropped a non-page ancestor from
    //! the path (`if doc.is_page() { push }` with no `else`), so
    //! `A(page) > b1(non-page) > P(page)` returned `Ok(["A","P"])` — a
    //! plausible-looking path that collides with any sibling of `b1`. These
    //! tests pin the fail-loud replacement.

    use std::collections::HashMap;

    use super::*;

    /// Minimal in-memory `DocumentManager` whose only real method is
    /// `get_by_id` — the sole method `name_chain`'s default impl calls.
    struct MockDocManager {
        by_id: HashMap<EntityUri, Block>,
    }

    impl MockDocManager {
        fn new(blocks: Vec<Block>) -> Self {
            Self {
                by_id: blocks.into_iter().map(|b| (b.id.clone(), b)).collect(),
            }
        }
    }

    #[async_trait]
    impl DocumentManager for MockDocManager {
        async fn find_by_parent_and_name(&self, _: &EntityUri, _: &str) -> Result<Option<Block>> {
            unimplemented!("not exercised by name_chain")
        }

        async fn create(&self, _: Block) -> Result<Block> {
            unimplemented!("not exercised by name_chain")
        }

        async fn get_by_id(&self, id: &EntityUri) -> Result<Option<Block>> {
            Ok(self.by_id.get(id).cloned())
        }

        async fn update_metadata(&self, _: &Block) -> Result<()> {
            unimplemented!("not exercised by name_chain")
        }
    }

    fn page(id: &str, parent: EntityUri, title: &str) -> Block {
        let mut b = Block::new_text(EntityUri::block(id), parent, title.to_string());
        b.set_page(true);
        b
    }

    fn non_page(id: &str, parent: EntityUri, title: &str) -> Block {
        Block::new_text(EntityUri::block(id), parent, title.to_string())
    }

    /// A well-formed chain of pages rooted at `no_parent()` resolves normally.
    #[tokio::test]
    async fn well_formed_page_chain_resolves() {
        let a = page("A", EntityUri::no_parent(), "projects");
        let p = page("P", a.id.clone(), "todo");
        let mgr = MockDocManager::new(vec![a, p.clone()]);
        let chain = mgr
            .name_chain(&p.id)
            .await
            .expect("well-formed chain resolves");
        assert_eq!(chain, vec!["projects".to_string(), "todo".to_string()]);
    }

    /// `A(page) > b1(non-page) > P(page)` — the prohibited topology. Before the
    /// fix this returned `Ok(["A","P"])`; now it fails loud naming `b1`.
    #[tokio::test]
    async fn non_page_ancestor_fails_loud() {
        let a = page("A", EntityUri::no_parent(), "projects");
        let b1 = non_page("b1", a.id.clone(), "a plain heading");
        let p = page("P", b1.id.clone(), "todo");
        let mgr = MockDocManager::new(vec![a, b1.clone(), p.clone()]);

        let err = mgr
            .name_chain(&p.id)
            .await
            .expect_err("a page under a non-page ancestor must fail loud, not coerce a path");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("non-page ancestor") && msg.contains(b1.id.as_str()),
            "error must name the offending non-page ancestor '{}'; got: {msg}",
            b1.id
        );
    }

    /// PROD-FAITHFUL (BugFunnel row 23/29): the page store (e.g.
    /// `LiveDocumentManager`'s `WHERE tag='Page'` matview) contains ONLY pages,
    /// so `get_by_id` on a NON-page content block returns `None`. `name_chain`
    /// of such a block must resolve to an EMPTY chain ("not a page → owns no
    /// file"), NOT fail loud: a non-page content heading is a legitimate benign
    /// input (the folder-companion de-inline storm where the nested non-page
    /// content of a de-inlined child page is absent from the companion's
    /// projection), and `doc_id_to_path` maps the empty chain to `Ok(None)`
    /// (skip). Before the fix this hit the ancestor-not-found `bail` and
    /// produced the 749-error `Projects.org` UNRESOLVABLE storm.
    #[tokio::test]
    async fn non_page_self_absent_from_page_store_resolves_empty() {
        // The store holds ONLY the page root; the content heading is not a page
        // and is therefore absent from the page store (get_by_id → None).
        let root = page("Projects", EntityUri::no_parent(), "Projects");
        let mgr = MockDocManager::new(vec![root]);
        let content_id = EntityUri::block("content-heading");
        let chain = mgr.name_chain(&content_id).await.expect(
            "a non-page block absent from the page store must resolve to an empty chain, not fail \
             loud",
        );
        assert!(chain.is_empty(), "expected empty chain, got {chain:?}");
    }

    /// A chain rooted at a NON-`no_parent` sentinel (a synthetic root that is
    /// still `is_sentinel()`) is a valid root stop — it must NOT be mistaken
    /// for a non-page ancestor (Finding B: the root check matches both
    /// predicates).
    #[tokio::test]
    async fn sentinel_rooted_chain_is_a_valid_root() {
        let synthetic_root = EntityUri::new("sentinel", "synthetic_root");
        assert!(synthetic_root.is_sentinel() && synthetic_root != EntityUri::no_parent());
        // The page's parent is the synthetic sentinel directly — the loop breaks
        // at the sentinel before ever trying to read it as a (non-page) block.
        let p = page("P", synthetic_root, "todo");
        let mgr = MockDocManager::new(vec![p.clone()]);
        let chain = mgr
            .name_chain(&p.id)
            .await
            .expect("a sentinel-rooted chain resolves without a false non-page-ancestor bail");
        assert_eq!(chain, vec!["todo".to_string()]);
    }
}
