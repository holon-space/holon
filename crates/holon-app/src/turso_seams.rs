//! Turso-backed org-sync seams + the OrgMode DI module (composition root).
//!
//! `CacheBlockReader` / `LiveDocumentManager` are the Turso counterparts of
//! the Loro seams in [`crate::loro_seams`]. They implement holon-orgmode's
//! backend-blind `BlockReader` / `DocumentManager` ports over `QueryableCache`,
//! matview-fed `LiveData`, and raw `DbHandle` SQL — all Turso-side machinery
//! that only the app wiring crate may name (ADR 0004). `OrgModeModule` is the
//! Turso container's registration of those seams plus the org schema/DDL and
//! provider wiring; holon-orgmode itself stays backend-blind
//! (`register_org_file_sync_core`).

use std::sync::Arc;

use fluxdi::{Injector, Module, Provider, Shared};

use holon::core::queryable_cache::QueryableCache;
use holon::storage::schema_module::SchemaModule;
use holon::storage::{BLOCK_READ_TABLE, BLOCK_WRITE_TABLE};
use holon_api::block::{blocks_by_document, Block};
use holon_api::{EntityName, EntityUri};
use holon_core::CrudAuthority;
use holon_core::EventOrigin;
use holon_core::{
    OperationProvider, OperationWrapper, OriginTaggedWrites, SyncTokenStore, SyncableProvider,
};
use holon_filesystem::directory::Directory;
use holon_filesystem::File;
use holon_filesystem::{BlockReader, DocumentManager};
use holon_orgmode::di::{
    register_org_file_sync_core, seed_default_org_assets, FileSyncStarted, OrgModeConfig,
};
use holon_orgmode::OrgModeSyncProvider;
use holon_profiles::TypeRegistry;
use holon_turso::schema_modules::BlockSchemaModule;
use std::path::PathBuf;

/// BlockReader backed by `QueryableCache<Block>`.
///
/// Reads bypass `cache.get_all()` because edge-typed fields like `tags`
/// live in junction tables (`block_tags`, `block_requires`) — a plain
/// `SELECT * FROM block` returns rows with `tags = []` and consumers like
/// the org renderer silently drop the headline tag. Instead, every read
/// goes through `load_all_blocks_with_hydration` which adds correlated
/// subqueries against the junctions so the materialized `Block` values
/// arrive with edge fields populated.
///
/// Junction-aware hydration in this read path also means downstream code
/// (renderer, MCP, link-provider, …) never has to know about junction
/// tables — that storage split stays Turso's concern.
pub struct CacheBlockReader {
    cache: Arc<QueryableCache<Block>>,
    /// Phase 5 keystone: the convergent block feed, drives
    /// `wait_for_blocks_in_feed` (the positional cache catch-up that replaced
    /// the `event_acks` watermark wait). `None` for backends without a feed.
    block_feed: Option<Arc<holon::sync::LiveData<Block>>>,
}

impl CacheBlockReader {
    pub fn new(cache: Arc<QueryableCache<Block>>) -> Self {
        Self {
            cache,
            block_feed: None,
        }
    }

    /// Phase 5 keystone: wire the convergent `LiveData<Block>` feed so
    /// `wait_for_blocks_in_feed` can prove the positional catch-up condition.
    pub fn with_block_feed(mut self, block_feed: Arc<holon::sync::LiveData<Block>>) -> Self {
        self.block_feed = Some(block_feed);
        self
    }

    /// Load every block from `block_raw` with edge-typed fields hydrated
    /// via correlated subquery against `block_tags`. Add more junction
    /// joins here as new edge fields appear.
    ///
    /// Returns `Vec<Block>` with `block.tags` populated. Skipping the cache's
    /// generic `SELECT *` keeps the contract uniform: every consumer of
    /// `BlockReader` sees the same hydrated shape.
    ///
    /// Why `block_raw` and not the `block` matview: matview rows propagate
    /// via CDC, which lags the underlying SQL write by an unbounded window.
    /// `on_block_changed` runs on an `events` CDC delivery whose source
    /// transaction has already committed to `block_raw`, but the `block`
    /// matview may not yet reflect it. Reading the matview here renders a
    /// stale snapshot and writes a stale org file. Same race class as
    /// inv-viewmodel-root-matches-render-expr (`block_with_query_source.sql` → `block_raw`); see
    /// devlog/2026-05-05-110315.md.
    async fn load_all_blocks_with_hydration(&self) -> anyhow::Result<Vec<Block>> {
        let sql = format!(
            "SELECT b.id, b.parent_id, b.depth, b.sort_key, b.content, \
             b.content_type, b.source_language, b.source_name, \
             b.properties, b.marks, b.collapsed, b.completed, \
             b.block_type, b.created_at, b.updated_at, \
             COALESCE((SELECT json_group_array(tag) FROM block_tags WHERE block_id = b.id), '[]') AS tags, \
             COALESCE((SELECT json_group_array(required_id) FROM block_requires WHERE block_id = b.id), '[]') AS requires \
             FROM {BLOCK_WRITE_TABLE} b \
             ORDER BY b.sort_key, b.id"
        );

        let rows = self
            .cache
            .db_handle()
            .query(&sql, std::collections::HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("[CacheBlockReader] hydrating SELECT failed: {e}"))?;

        // NOTE: must go through `Block: TryFrom<HashMap<String, Value>>`,
        // NOT `Block: TryFromEntity`. `tags` is `#[serde(skip, default)]`,
        // and the `Entity` derive treats that as "skip serialization" — so
        // the derived `from_entity` would silently leave `tags` empty even
        // when the row carries a populated `tags` JSON array. The hand-rolled
        // `TryFrom` impl in `holon-api/src/block.rs` parses the column.
        rows.into_iter()
            .map(|row| {
                Block::try_from(row).map_err(|e| {
                    anyhow::anyhow!("[CacheBlockReader] Block::try_from row failed: {e}")
                })
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl BlockReader for CacheBlockReader {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        // Phase 5: push the doc-scoped BFS into SQL via a recursive CTE on
        // `block_raw`. Replaces `load_all_blocks_with_hydration` + in-Rust
        // BFS — that combination fired ~26×/sec on the
        // `on_block_changed → render_file_by_doc_id` path (one full table
        // scan per Loro→SQL block apply). The CTE walks down from
        // `doc_id`'s direct children, excluding any block tagged `Page`
        // (sub-document boundary; the Rust BFS did the same by skipping
        // `block.is_page()`). Depth bound 100 matches existing
        // `find_document_uri` shape.
        let sql = format!(
            "WITH RECURSIVE descendants(id, depth_acc) AS ( \
                SELECT b.id, 0 \
                FROM {table} b \
                LEFT JOIN block_tags bt ON bt.block_id = b.id AND bt.tag = 'Page' \
                WHERE b.parent_id = $doc_id \
                  AND bt.block_id IS NULL \
                UNION ALL \
                SELECT b.id, d.depth_acc + 1 \
                FROM {table} b \
                JOIN descendants d ON b.parent_id = d.id \
                LEFT JOIN block_tags bt ON bt.block_id = b.id AND bt.tag = 'Page' \
                WHERE bt.block_id IS NULL AND d.depth_acc < 100 \
            ) \
            SELECT b.id, b.parent_id, b.depth, b.sort_key, b.content, \
                   b.content_type, b.source_language, b.source_name, \
                   b.properties, b.marks, b.collapsed, b.completed, \
                   b.block_type, b.created_at, b.updated_at, \
                   COALESCE((SELECT json_group_array(tag) FROM block_tags WHERE block_id = b.id), '[]') AS tags, \
                   COALESCE((SELECT json_group_array(required_id) FROM block_requires WHERE block_id = b.id), '[]') AS requires \
            FROM {table} b \
            JOIN descendants d ON d.id = b.id \
            ORDER BY b.sort_key, b.id",
            table = BLOCK_WRITE_TABLE,
        );

        let mut params = std::collections::HashMap::new();
        params.insert(
            "doc_id".to_string(),
            holon_api::Value::String(doc_id.to_string()),
        );

        let rows = self
            .cache
            .db_handle()
            .query(&sql, params)
            .await
            .map_err(|e| anyhow::anyhow!("[CacheBlockReader::get_blocks] CTE query failed: {e}"))?;

        // Same Block::try_from path as load_all_blocks_with_hydration — the
        // derived TryFromEntity would silently leave `tags` empty because
        // of `#[serde(skip, default)]`. See block_two_deserializers memory.
        rows.into_iter()
            .map(|row| {
                Block::try_from(row).map_err(|e| {
                    anyhow::anyhow!(
                        "[CacheBlockReader::get_blocks] Block::try_from row failed: {e}"
                    )
                })
            })
            .collect()
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        let all_blocks = self.load_all_blocks_with_hydration().await?;
        Ok(blocks_by_document(&all_blocks))
    }

    /// Phase 1: load `(file_id, content_hash)` rows directly from the `file`
    /// table via raw SQL — bypasses the in-process file QueryableCache so we
    /// can read at controller startup, before CDC has replayed file events.
    async fn load_file_hashes(&self) -> anyhow::Result<Vec<(holon_api::EntityUri, String)>> {
        let rows = self
            .cache
            .db_handle()
            .query(
                "SELECT id, content_hash FROM file",
                std::collections::HashMap::new(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("[CacheBlockReader] load_file_hashes failed: {e}"))?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id = match row.get("id") {
                Some(holon_api::Value::String(s)) => s.clone(),
                _ => continue,
            };
            let hash = match row.get("content_hash") {
                Some(holon_api::Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            if hash.is_empty() {
                // Skip rows the legacy writer left without a hash — they'd
                // false-match an empty stored value and incorrectly fast-path.
                continue;
            }
            let uri = holon_api::EntityUri::parse(&id).map_err(|e| {
                anyhow::anyhow!("[CacheBlockReader] file.id={id:?} not a valid EntityUri: {e}")
            })?;
            out.push((uri, hash));
        }
        Ok(out)
    }

    async fn wait_for_blocks_in_feed(&self, block_ids: &[String], timeout_ms: u64) -> bool {
        match &self.block_feed {
            Some(feed) => {
                let ids: Vec<String> = block_ids.to_vec();
                feed.wait_until(
                    move |m| ids.iter().all(|id| m.contains_key(id)),
                    std::time::Duration::from_millis(timeout_ms),
                )
                .await
            }
            None => true,
        }
    }

    async fn persist_file_hash(
        &self,
        file_id: &holon_api::EntityUri,
        hash: &str,
    ) -> anyhow::Result<()> {
        let params = vec![
            holon_api::Value::String(hash.to_string()),
            holon_api::Value::String(file_id.to_string()),
        ];
        self.cache
            .db_handle()
            .execute_values("UPDATE file SET content_hash = ? WHERE id = ?", params)
            .await
            .map_err(|e| {
                anyhow::anyhow!("[CacheBlockReader] persist_file_hash UPDATE failed: {e}")
            })?;
        Ok(())
    }
}

/// DocumentManager backed by CDC-driven LiveData over page blocks.
///
/// All reads (`find_by_parent_and_name`, `get_by_id`) are in-memory lookups
/// against a `LiveData<Block>` that stays current via a Turso materialized
/// view CDC stream over blocks whose `tags` JSON list contains `"Page"`.
/// Writes go through `SqlOperationProvider` (SQL); the matview
/// CDC automatically propagates them into the LiveData.
pub struct LiveDocumentManager {
    live: Arc<holon::sync::LiveData<Block>>,
    command_bus: Arc<dyn OriginTaggedWrites>,
    /// Serializes find-then-create against itself so two concurrent
    /// `get_or_create_by_name_chain` calls for the same `(parent_id, title)`
    /// can't both miss the LiveData lookup and INSERT distinct UUIDs. The
    /// previous safeguard was `idx_block_document_unique` (UNIQUE on
    /// `(parent_id, name)`), which was dropped when `name` became a tag.
    create_lock: Arc<tokio::sync::Mutex<()>>,
}

impl LiveDocumentManager {
    /// Create a LiveDocumentManager backed by a materialized view over document blocks.
    pub async fn new(
        command_bus: Arc<dyn OriginTaggedWrites>,
        db_handle: holon::storage::DbHandle,
    ) -> anyhow::Result<Self> {
        let matview_mgr =
            holon::sync::MatviewManager::new(db_handle, Arc::new(tokio::sync::Mutex::new(())));

        // Match any block that has the "Page" tag in the block_tags junction table.
        let watch_sql = format!(
            "SELECT b.* FROM {BLOCK_READ_TABLE} b JOIN block_tags bt ON bt.block_id = b.id WHERE bt.tag = 'Page'"
        );
        let result = matview_mgr.watch(&watch_sql).await?;

        let live = holon::sync::LiveData::new(
            result.initial_rows,
            |row| {
                row.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow::anyhow!("document block row missing 'id'"))
            },
            |row| Block::try_from(row.clone()).map_err(|e| anyhow::anyhow!("{}", e)),
        );
        live.subscribe("document_blocks", result.stream);

        tracing::info!(
            "[LiveDocumentManager] Watching {} document blocks via matview",
            live.read().len()
        );

        Ok(Self {
            live,
            command_bus,
            create_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
}

#[async_trait::async_trait]
impl DocumentManager for LiveDocumentManager {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        let docs = self.live.read();
        Ok(docs
            .values()
            .find(|d| d.parent_id == *parent_id && d.is_page() && d.title() == title)
            .map(|d| (**d).clone()))
    }

    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        use holon_orgmode::build_block_params;

        // Serialize against concurrent creates so two callers asking for the
        // same `(parent_id, title)` page can't both observe LiveData empty
        // and both INSERT distinct UUIDs. Inside the lock, re-check LiveData;
        // if the page now exists (CDC may have caught up while we were
        // waiting), return the existing entry.
        let _guard = self.create_lock.lock().await;
        if let Some(existing) = self
            .find_by_parent_and_name(&doc.parent_id, &doc.title())
            .await?
        {
            tracing::debug!(
                "[LiveDocumentManager] Page {:?} already exists as {} (skipping create)",
                doc.title(),
                existing.id,
            );
            return Ok(existing);
        }

        // Route document creation events to the document's own ID.
        // _routing_doc_uri is only event routing metadata (not stored in DB) —
        // it tells FileSyncController which file to re-render.
        let params = build_block_params(&doc, &doc.parent_id, &doc.id);
        // INSERT OR IGNORE: only triggers on PK collision now that the
        // partial unique index on `(parent_id, name)` is gone. The
        // `create_lock` above is what prevents same-title duplicates.
        // Tag the create event with `EventOrigin::Org` so the
        // `LoroSyncController` inbound gate routes it to `Apply` instead of
        // dropping it as a generic SQL-direct write. This page-creation flow
        // is triggered by `FileSyncController::on_file_changed`; semantically
        // it's an Org-driven event.
        let result = self
            .command_bus
            .execute_operation_with_origin(
                &EntityName::new("block"),
                "create",
                params,
                EventOrigin::Org,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        // If the response carries an existing id, the INSERT was ignored —
        // a page with the same (parent_id, title) already exists in the DB.
        // Return that existing page instead of the one we tried to insert.
        if let Some(holon_api::Value::String(existing_id)) = result.response {
            tracing::debug!(
                "[LiveDocumentManager] Page {:?} already exists as {} (attempted id={})",
                doc.title(),
                existing_id,
                doc.id,
            );
            // ALLOW(entity_uri_from_raw): existing_id from command_bus execute_operation response (SQL Value::String)
            let existing_uri = EntityUri::from_raw(&existing_id);
            if let Some(existing) = self.get_by_id(&existing_uri).await? {
                return Ok(existing);
            }
            // The document exists in SQL but not in LiveData.
            // Insert it so subsequent find_by_parent_and_name / get_by_id lookups succeed.
            let mut existing_doc = doc.clone();
            existing_doc.id = existing_uri;
            self.live.insert(
                existing_doc.id.as_str().to_string(),
                Arc::new(existing_doc.clone()),
            );
            return Ok(existing_doc);
        }

        self.live
            .insert(doc.id.as_str().to_string(), Arc::new(doc.clone()));
        Ok(doc)
    }

    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        let docs = self.live.read();
        Ok(docs.get(id.as_str()).map(|d| (**d).clone()))
    }

    async fn update_metadata(&self, doc: &Block) -> anyhow::Result<()> {
        use holon_orgmode::build_block_params;
        let mut params = build_block_params(doc, &doc.parent_id, &doc.id);
        // `doc.properties` is authoritative for doc-level metadata, but
        // `build_block_params` only emits keys that are PRESENT — and the
        // SQL provider's property merge can't clear what isn't mentioned.
        // Emit a `Value::Null` removal sentinel for every property the
        // previously-known doc had that the new doc no longer carries
        // (e.g. `todo_keywords` after the `#+TODO:` header was deleted).
        if let Some(old) = self.live.read().get(doc.id.as_str()).cloned() {
            for key in old.properties.keys() {
                if !doc.properties.contains_key(key) && !params.contains_key(key.as_str()) {
                    params.insert(key.as_str().into(), holon_api::Value::Null);
                }
            }
        }
        // Tag as `EventOrigin::Org` mirroring sibling `create` above.
        // Without this, the `LoroSyncController` inbound gate
        // drops the event as a generic SQL-direct write — and on doc rows
        // whose `properties` contain `todo_keywords` from a `#+TODO:`
        // directive, the keyword list is silently lost. Surfaced as the
        // `inv-org-render-fixed-point` PBT flake (May 2026, see
        // `devlog/2026-05-19-phase-c-validation-diagnosis.md`).
        self.command_bus
            .execute_operation_with_origin(
                &EntityName::new("block"),
                "update",
                params,
                EventOrigin::Org,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        // Update in-memory cache
        self.live
            .insert(doc.id.as_str().to_string(), Arc::new(doc.clone()));
        Ok(())
    }
}

/// ServiceModule for OrgMode integration (Turso container).
///
/// Registers OrgMode-specific services in the DI container.
/// Loro services are NOT registered here — they come from LoroModule (if enabled).
///
/// OrgMode will detect if LoroBlockOperations is available in DI and use it;
/// otherwise it falls back to SqlOperationProvider.
pub struct OrgModeModule;

impl Module for OrgModeModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        use tracing::{error, info};

        info!("[OrgModeModule] register_services called");

        register_org_file_sync_core(injector)?;

        // Register OrgModeSyncProvider as a factory
        injector.provide::<OrgModeSyncProvider>(Provider::root_async(async |resolver| {
            let config = resolver.resolve::<OrgModeConfig>();
            let token_store = resolver
                .try_resolve_async::<dyn SyncTokenStore>()
                .await
                .expect("[OrgModeModule] SyncTokenStore not found in DI");
            let fs = resolver.resolve::<dyn holon_filesystem::FileSystem>();
            Shared::new(OrgModeSyncProvider::new(
                config.root_directory.clone(),
                token_store,
                fs,
            ))
        }));

        // Register SyncableProvider trait implementation
        injector.provide_into_set::<dyn SyncableProvider>(Provider::root(|resolver| {
            let sync_provider = resolver.resolve::<OrgModeSyncProvider>();
            sync_provider.clone() as Arc<dyn SyncableProvider>
        }));

        // Register filesystem entity types in the TypeRegistry for GQL graph.
        // Done inside an async provider so TypeRegistry is already available.
        injector.provide::<QueryableCache<Directory>>(Provider::root_async(|r| async move {
            let type_registry = r.resolve::<TypeRegistry>();
            if let Err(e) = type_registry.register(Directory::type_definition()) {
                tracing::warn!("[OrgModeModule] Failed to register Directory type: {e}");
            }
            Shared::new(holon::di::create_queryable_cache_async(&r).await)
        }));
        injector.provide::<QueryableCache<File>>(Provider::root_async(|r| async move {
            let type_registry = r.resolve::<TypeRegistry>();
            if let Err(e) = type_registry.register(File::type_definition()) {
                tracing::warn!("[OrgModeModule] Failed to register File type: {e}");
            }
            Shared::new(holon::di::create_queryable_cache_async(&r).await)
        }));

        // File-sync seams (Turso backend). The no-Turso container registers the
        // Loro counterparts (LoroBlockReader / LoroDocumentManager); the
        // FileSyncStarted core resolves whichever the container provides,
        // so it stays backend-blind per ADR 0004. `dyn BlockOrdering` is already
        // provided (SqlBlockOperations via EventInfraModule).
        injector.provide::<dyn BlockReader>(Provider::root_async(|r| async move {
            let block_cache = r.resolve_async::<QueryableCache<Block>>().await;
            let mut reader = CacheBlockReader::new(block_cache);
            if let Some(feed) = r
                .optional_resolve_async::<holon_api::live_data::BlockFeed>()
                .await
            {
                reader = reader.with_block_feed(feed.0.clone());
            }
            Arc::new(reader) as Arc<dyn BlockReader>
        }));
        injector.provide::<dyn DocumentManager>(Provider::root_async(|r| async move {
            let db_handle = r.resolve::<dyn holon::di::DbHandleProvider>().handle();
            let sql_ops = Arc::new(holon::core::SqlOperationProvider::with_edge_fields(
                db_handle.clone(),
                BLOCK_WRITE_TABLE.to_string(),
                "block".to_string(),
                "block".to_string(),
                BlockSchemaModule.edge_fields(),
            ));
            let command_bus: Arc<dyn OriginTaggedWrites> = sql_ops as Arc<dyn OriginTaggedWrites>;
            let mgr = LiveDocumentManager::new(command_bus, db_handle)
                .await
                .expect("Failed to create LiveDocumentManager");
            Arc::new(mgr) as Arc<dyn DocumentManager>
        }));

        // Set up event bus wiring and background tasks.
        // This factory resolves LoroBlockOperations if available (Loro enabled),
        // otherwise creates a SqlOperationProvider for direct SQL block operations.
        injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(move |resolver| {
            async move {
                // ============================================================
                // PHASE 1: Resolve ALL services that run DDL
                // This ensures all schema initialization completes BEFORE
                // any background tasks start using the database.
                // ============================================================
                info!("[OrgMode] Phase 1: Resolving services (DDL)");

                let _dir_cache = resolver.resolve_async::<QueryableCache<Directory>>().await;
                let _file_cache = resolver.resolve_async::<QueryableCache<File>>().await;
                let _block_cache = resolver.resolve_async::<QueryableCache<Block>>().await;
                let sync_provider = resolver.resolve_async::<OrgModeSyncProvider>().await;

                // Resolve remaining services
                let config = resolver.resolve::<OrgModeConfig>();

                // Seed default documents into an empty vault through the
                // FileSystem port BEFORE the initial sync task and the
                // controller's initial scan below — both consume the vault.
                {
                    let seed_fs = resolver.resolve::<dyn holon_filesystem::FileSystem>();
                    seed_default_org_assets(seed_fs.as_ref(), &config).await;
                }

                // Resolve the block-CRUD authority — registered at the app
                // composition root as the Loro provider when a CRDT backend is
                // enabled, absent in SqlOnly. orgmode picks the authority without
                // naming a concrete backend type.
                // ALLOW(ok): optional DI service
                let crud_authority: Option<Arc<CrudAuthority>> =
                    resolver.try_resolve::<CrudAuthority>().ok();

                let loro_available = crud_authority.is_some();
                info!(
                    "[OrgMode] Phase 1 complete: All DDL finished (crud_authority={})",
                    loro_available
                );

                // Resolve DbHandle unconditionally — Turso is always available
                let db_handle_provider = resolver.resolve::<dyn holon::di::DbHandleProvider>();
                let db_handle = db_handle_provider.handle();

                // FileSyncController writes through SQL ops; CacheBlockReader reads from QueryableCache
                // which is also backed by the same Turso database, ensuring consistency.
                let sql_ops = Arc::new(holon::core::SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                ));

                // ============================================================
                // PHASE 2: Create FileSyncController
                // Single controller using last_projection for echo suppression.
                // ============================================================
                info!("[OrgMode] Phase 2: Creating FileSyncController");

                info!("[OrgMode] Phase 2 complete");

                // ============================================================
                // PHASE 3: Spawn background tasks
                // The DatabaseActor serializes all operations, eliminating race conditions
                // between DDL and DML operations.
                // ============================================================
                info!("[OrgMode] Phase 3: Spawning background tasks");

                // Block writes go through FileSyncController → command_bus
                // (`SqlOperationProvider`) → the `block_raw` table directly; the
                // block cache and downstream sinks react via CDC / `LiveData<Block>`.

                // Option B: directory + file caches are fed DIRECTLY from the
                // `OrgModeSyncProvider` change broadcast — the EventBus middleman
                // (`OrgModeEventAdapter` → EventBus → `CacheEventSubscriber`) is
                // gone. We subscribe to the broadcast HERE, *before* the initial
                // sync task below runs, so the snapshot it emits is captured
                // gap-free. (The old EventBus replay/cursor only existed because the
                // adapter subscribed *after* sync — a fixable ordering bug, not an
                // inherent need for a durable replay buffer. Batches are coarse
                // `Vec<Change>` messages, so the broadcast buffer never lags.)
                {
                    let dir_cache = resolver.resolve_async::<QueryableCache<Directory>>().await;
                    let file_cache = resolver.resolve_async::<QueryableCache<File>>().await;
                    let mut dir_rx = sync_provider.subscribe_directories();
                    let mut file_rx = sync_provider.subscribe_files();
                    tokio::spawn(async move {
                        loop {
                            match dir_rx.recv().await {
                                Ok(batch) => {
                                    if let Err(e) = dir_cache.apply_batch(&batch.inner, None).await
                                    {
                                        error!("[directory cache feed] apply_batch failed: {}", e);
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                    tracing::warn!(
                                        "[directory cache feed] lagged by {} batches",
                                        n
                                    );
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    });
                    tokio::spawn(async move {
                        loop {
                            match file_rx.recv().await {
                                Ok(batch) => {
                                    if let Err(e) = file_cache.apply_batch(&batch.inner, None).await
                                    {
                                        error!("[file cache feed] apply_batch failed: {}", e);
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                    tracing::warn!("[file cache feed] lagged by {} batches", n);
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    });
                }

                // Initial sync task
                // The DatabaseActor serializes all operations, eliminating race conditions.
                {
                    let sync_provider_clone = sync_provider.clone();
                    tokio::spawn(async move {
                        use holon_core::SyncableProvider;
                        if let Err(e) = sync_provider_clone
                            .sync(holon_api::StreamPosition::Beginning)
                            .await
                        {
                            error!("[OrgMode] Initial sync failed: {}", e);
                        }
                    });
                }

                // Loro ↔ command/event bus is wired by `LoroModule` via
                // `LoroSyncControllerHandle`; see `crates/holon/src/sync/loro_module.rs`.

                // FileSyncController: backend-blind file ↔ block sync.
                // Resolving FileSyncStarted (registered by
                // register_org_file_sync_core above) builds the controller over
                // the DI-provided seams and spawns its loop. Done HERE — after
                // Phase-1 DDL + seeding — so the initial scan sees a seeded
                // vault. The no-Turso container resolves the same marker after
                // its own seeding.
                resolver.resolve_async::<FileSyncStarted>().await;

                // Block command-path provider for the UI dispatcher.
                //
                // Under Loro authority (Loro enabled) block CRUD
                // (set_field / create / update / delete) must land in the
                // Loro doc — the source of truth — so `LoroSyncController`
                // can project it to the SQL `block_raw` table + matview.
                // Routing these through the generic `SqlOperationProvider`
                // (SQL-direct) instead made content edits on non-rendered
                // blocks drift: SQL got the new value, Loro never did
                // (see `tests/loro_content_drop_pbt.rs`). This realizes the
                // behaviour this factory's header already promised
                // ("resolve LoroBlockOperations if available").
                //
                // In SqlOnly mode there is no Loro, so SQL is the authority
                // and writes go straight to `block_raw`.
                //
                // Structural ops (indent / split / move) are served by the
                // earlier-registered `SqlBlockOperations` (EventInfraModule),
                // which wins them on registration order; this provider only
                // wins the CRUD ops that `SqlBlockOperations` does not
                // advertise.
                match crud_authority {
                    Some(authority) => {
                        let wrapper =
                            OperationWrapper::new(authority.0.clone(), Some(sync_provider));
                        Arc::new(wrapper) as Arc<dyn OperationProvider>
                    }
                    None => {
                        let wrapper = OperationWrapper::new(
                            sql_ops.clone() as Arc<dyn OperationProvider>,
                            Some(sync_provider),
                        );
                        Arc::new(wrapper) as Arc<dyn OperationProvider>
                    }
                }
            }
        }));

        Ok(())
    }
}

/// Extension trait for registering OrgMode services in a [`Injector`]
///
/// This trait provides a convenient method to register all OrgMode-related
/// services with a single call, taking just the root directory as a parameter.
///
/// # Example
///
/// ```rust,ignore
/// use holon_app::turso_seams::OrgModeInjectorExt;
/// use std::path::PathBuf;
///
/// // In your DI setup closure:
/// services.add_orgmode(PathBuf::from("/path/to/org/files"))?;
/// ```
pub trait OrgModeInjectorExt {
    fn add_orgmode(&self, root_directory: PathBuf) -> std::result::Result<(), fluxdi::Error>;
}

impl OrgModeInjectorExt for Injector {
    fn add_orgmode(&self, root_directory: PathBuf) -> std::result::Result<(), fluxdi::Error> {
        self.provide::<OrgModeConfig>(Provider::root(move |_| {
            Shared::new(OrgModeConfig::new(root_directory.clone()))
        }));
        OrgModeModule.configure(self)?;
        Ok(())
    }
}
