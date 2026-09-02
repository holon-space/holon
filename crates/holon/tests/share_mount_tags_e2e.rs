//! Sharing a page must leave it a page.
//!
//! The sidebar's membership test is the `block_tags` junction (see
//! `LiveDocumentManager::new` in `crates/holon-app/src/turso_seams.rs`), so a
//! mount whose tags never reach that junction is unreachable by navigation on
//! BOTH peers even though every row and every byte of content is present. That
//! is the shape of the 2026-09-02 bugfunnel entry
//! `sharing-a-page-drops-its-page-tag-and-it-vanishes-from-both-sidebars`.
//!
//! The SUT here is the SQL write provider the share backend actually writes
//! through, constructed by the same production factory
//! (`holon_loro_wiring::block_sql_write_provider`) the DI wiring uses — a test
//! that built its own provider would prove nothing about the wiring, which is
//! where the tags were lost.

use std::collections::HashMap;
use std::sync::Arc;

use holon::storage::turso::DbHandle;
use holon::storage::turso::TursoBackend;
use holon_loro::degraded_signal_bus::DegradedSignalBus;
use holon_loro::device_key_store::load_or_create_device_key;
use holon_loro::iroh_advertiser::IrohAdvertiser;
use holon_loro::iroh_sync_adapter::SharedTreeSyncManager;
use holon_loro::loro_document_store::LoroDocumentStore;
use holon_loro::loro_share_backend::LoroShareBackend;
use holon_loro::loro_share_backend::SubtreeShareOperations;
use holon_loro::multi_peer::TREE_NAME;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockMatviewSchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::schema_modules::LinkSchemaModule;
use loro::LoroText;
use serde_json::Value as JsonValue;
use tempfile::TempDir;
use tokio::sync::RwLock;

/// The sidebar's own membership query (`assets/default/index.org`, the
/// `block:left_sidebar::src::0` source block). Verbatim: a paraphrase would
/// stop pinning what the user actually sees.
const SIDEBAR_SQL: &str = "SELECT b.* FROM block b JOIN block_tags bt ON bt.block_id = b.id WHERE \
                           bt.tag = 'Page' AND b.id != 'block:__default__' ORDER BY b.content ASC";

async fn setup_db() -> (TursoBackend, DbHandle) {
    let (backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("in-memory turso backend");
    handle
        .execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .expect("enable FKs");
    CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("core schema");
    BlockSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("block junction schema");
    // The block create path cleans `block_links` explicitly, so the junction
    // has to exist even though this test writes no links.
    LinkSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("link schema");
    BlockMatviewSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("block matview");
    (backend, handle)
}

fn share_backend(dir: &TempDir, handle: DbHandle) -> Arc<LoroShareBackend> {
    let store = Arc::new(RwLock::new(LoroDocumentStore::new(
        dir.path().to_path_buf(),
    )));
    let bus = Arc::new(DegradedSignalBus::new());
    let snapshot_store = Arc::new(holon_loro::shared_snapshot_store::SharedSnapshotStore::new(
        dir.path().to_path_buf(),
        bus.clone(),
    ));
    let manager = Arc::new(SharedTreeSyncManager::new());
    let key = load_or_create_device_key(dir.path()).expect("device key");
    let advertiser = Arc::new(IrohAdvertiser::new_with_key(key.clone()));
    LoroShareBackend::new_with_sql(
        store,
        snapshot_store,
        manager,
        advertiser,
        bus,
        key,
        Some(holon_loro_wiring::block_sql_write_provider(handle)),
        None,
    )
}

/// Seed a top-level block with `tags` and two children into the global Loro
/// doc. A `Page` in `tags` makes it a page. Top-level so the mount's SQL parent
/// resolves to the `no_parent` sentinel, which `CoreSchemaModule` already seeds
/// as the FK anchor.
async fn seed_page(backend: &LoroShareBackend, id: &str, title: &str, tags: &[&str]) {
    let collab = backend.test_global_doc().await;
    let doc_arc = collab.doc();
    let doc = &*doc_arc;
    let tree = doc.get_tree(TREE_NAME);

    let page = tree.create(None).expect("create page node");
    let meta = tree.get_meta(page).expect("page meta");
    meta.insert("id", loro::LoroValue::from(id)).expect("id");
    let text: LoroText = meta
        .ensure_mergeable_text("content_raw")
        .expect("content_raw");
    text.insert(0, title).expect("title");
    // Tags live in the node's `meta` as a JSON array string — the shape
    // `LoroBackend::set_block_tags` writes and `read_block_from_tree` reads.
    let tags_json = serde_json::to_string(tags).expect("tags json");
    meta.insert("tags", loro::LoroValue::from(tags_json.as_str()))
        .expect("tags");

    for (i, child) in ["Flights", "Hotel"].iter().enumerate() {
        let node = tree.create(Some(page)).expect("create child");
        let cmeta = tree.get_meta(node).expect("child meta");
        cmeta
            .insert("id", loro::LoroValue::from(format!("{id}-{i}").as_str()))
            .expect("child id");
        let ctext: LoroText = cmeta
            .ensure_mergeable_text("content_raw")
            .expect("child content_raw");
        ctext.insert(0, child).expect("child title");
    }
    doc.commit();
}

async fn sidebar_titles(handle: &DbHandle) -> Vec<String> {
    let rows = handle
        .query(SIDEBAR_SQL, HashMap::new())
        .await
        .expect("sidebar query");
    rows.into_iter()
        .filter_map(|r| {
            r.get("content")
                .and_then(|v| v.as_string())
                .map(str::to_string)
        })
        .collect()
}

/// Every `(block_id, tag)` row in the junction, sorted — so a tag claimed by
/// two rows is visible as duplication, which `tags_of` on one id cannot show.
async fn all_tag_rows(handle: &DbHandle) -> Vec<(String, String)> {
    let rows = handle
        .query(
            "SELECT block_id, tag FROM block_tags ORDER BY block_id, tag",
            HashMap::new(),
        )
        .await
        .expect("block_tags query");
    rows.into_iter()
        .filter_map(|r| {
            let id = r.get("block_id").and_then(|v| v.as_string())?.to_string();
            let tag = r.get("tag").and_then(|v| v.as_string())?.to_string();
            Some((id, tag))
        })
        .collect()
}

async fn tags_of(handle: &DbHandle, block_id: &str) -> Vec<String> {
    let sql = format!(
        "SELECT tag FROM block_tags WHERE block_id = '{}' ORDER BY tag",
        block_id.replace('\'', "''")
    );
    let rows = handle
        .query(&sql, HashMap::new())
        .await
        .expect("tags query");
    rows.into_iter()
        .filter_map(|r| r.get("tag").and_then(|v| v.as_string()).map(str::to_string))
        .collect()
}

/// Share `block:trip` and return the sharer's mount block id plus the ticket.
async fn share_trip(backend: &LoroShareBackend) -> (String, String) {
    let resp = backend
        .share_subtree("block:trip", "none".into())
        .await
        .expect("share_subtree");
    let json: JsonValue = match resp.response.expect("share response") {
        holon_api::Value::String(s) => serde_json::from_str(&s).expect("response json"),
        other => panic!("unexpected share response: {other:?}"),
    };
    (
        json["mount_block_id"]
            .as_str()
            .expect("mount_block_id in share response")
            .to_string(),
        json["ticket"]
            .as_str()
            .expect("ticket in share response")
            .to_string(),
    )
}

/// P-SHARE-STAYS-A-PAGE: a page that was in the sidebar before `share_subtree`
/// is still in the sidebar after it — asserted through the sidebar's own query,
/// because "the rows arrived" and "the user can reach them" are different
/// checks and only the first was ever made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sharing_a_page_keeps_it_in_the_sidebar() {
    let dir = TempDir::new().unwrap();
    let (_backend_guard, handle) = setup_db().await;
    let backend = share_backend(&dir, handle.clone());

    seed_page(&backend, "trip", "Trip planning", &["Page", "Travel"]).await;

    let (mount_id, _ticket) = share_trip(&backend).await;

    assert_eq!(
        tags_of(&handle, &mount_id).await,
        vec!["Page".to_string(), "Travel".to_string()],
        "the mount must carry the shared root's whole tag set — every tag-driven \
         query has the sidebar's blind spot, not just the sidebar"
    );
    assert_eq!(
        sidebar_titles(&handle).await,
        vec!["Trip planning".to_string()],
        "the shared page vanished from the sidebar: its mount is not a `Page` in \
         the `block_tags` junction the sidebar selects on"
    );
}

/// P-CONTAINER-CLAIMS-NOTHING: sharing a NON-page block wraps it in a synthetic
/// container page, and a container is not the block. It gets `Page` (it owns an
/// on-disk org file) and nothing else: the shared root's row SURVIVES on this
/// path — `project_descendants_to_sql` drops it only for a page — so a tag
/// copied onto the container would exist twice, and the container would surface
/// in tag feeds it has nothing to do with.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sharing_a_non_page_block_leaves_its_tags_on_the_block() {
    let dir = TempDir::new().unwrap();
    let (_backend_guard, handle) = setup_db().await;
    let backend = share_backend(&dir, handle.clone());

    seed_page(&backend, "trip", "Trip planning", &["Travel"]).await;

    let (mount_id, _ticket) = share_trip(&backend).await;

    assert_eq!(
        tags_of(&handle, &mount_id).await,
        vec!["Page".to_string()],
        "a synthetic container must claim only its own page-ness, never the \
         shared block's tags"
    );
    assert_eq!(
        all_tag_rows(&handle).await,
        vec![
            (mount_id.clone(), "Page".to_string()),
            ("block:trip".to_string(), "Travel".to_string()),
        ],
        "`Travel` must belong to exactly one block — the shared root keeps it, \
         the container never gains a second claim on it"
    );
}

/// P-ACCEPT-IS-A-PAGE: the accepter's mount is a page in its own sidebar too.
/// This is where the miss cost the most — accept a share, see nothing, with no
/// error to explain it. The accepter mints its own mount id, so the sidebar
/// query (not an id comparison) is the only peer-independent oracle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accepting_a_shared_page_puts_it_in_the_sidebar() {
    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    let (_guard_a, handle_a) = setup_db().await;
    let (_guard_b, handle_b) = setup_db().await;
    let a = share_backend(&dir_a, handle_a.clone());
    let b = share_backend(&dir_b, handle_b.clone());

    seed_page(&a, "trip", "Trip planning", &["Page", "Travel"]).await;
    let (_mount_a, ticket) = share_trip(&a).await;

    // No page ancestor above the accept parent, so the mount attaches under the
    // "Shared with me" root — which `accept_shared_subtree` projects first.
    seed_page(&b, "root-b", "Root B", &[]).await;
    let accept = b
        .accept_shared_subtree("block:root-b", ticket)
        .await
        .expect("accept_shared_subtree");
    let accept_json: JsonValue = match accept.response.expect("accept response") {
        holon_api::Value::String(s) => serde_json::from_str(&s).expect("accept response json"),
        other => panic!("unexpected accept response: {other:?}"),
    };
    let mount_b = accept_json["mount_block_id"]
        .as_str()
        .expect("mount_block_id in accept response")
        .to_string();

    assert_eq!(
        tags_of(&handle_b, &mount_b).await,
        vec!["Page".to_string(), "Travel".to_string()],
        "the accepter's mount must carry the shared root's tag set"
    );
    assert!(
        sidebar_titles(&handle_b)
            .await
            .contains(&"Trip planning".to_string()),
        "the accepted page never appeared in the accepter's sidebar"
    );

    a.advertiser_for_test().close_all().await;
    b.advertiser_for_test().close_all().await;
}
