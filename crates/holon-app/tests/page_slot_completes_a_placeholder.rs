//! A page whose `page_slot` is an untitled placeholder row is created by
//! COMPLETING that row with its title
//! (holon_api::Recognition::UnnamedPlaceholder), through the production
//! `LiveDocumentManager` the ingest's `resolve_dir_page_chain` drives.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use fluxdi::Module;
use fluxdi::Provider;
use holon::storage::BLOCK_READ_TABLE;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_api::block::Block;
use holon_app::turso_seams::LiveDocumentManager;
use holon_filesystem::DocumentManager;
use holon_loro_wiring::EventInfraModule;

async fn boot(
    db_path: std::path::PathBuf,
) -> (
    Arc<holon::api::backend_engine::BackendEngine>,
    Arc<dyn holon_core::block_ordering::BlockOrdering>,
) {
    holon::di::create_backend_engine_with_extras(
        db_path,
        std::sync::Arc::new(holon_api::ConditionBus::new()),
        |injector| {
            EventInfraModule
                .configure(injector)
                .map_err(|e| anyhow::anyhow!("configure EventInfraModule: {e}"))?;
            injector.provide_into_set::<dyn holon_core::OperationProvider>(Provider::root(
                |resolver| {
                    let db = resolver
                        .resolve::<dyn holon::di::DbHandleProvider>()
                        .handle();
                    Arc::new(holon::core::SqlOperationProvider::with_edge_fields(
                        db,
                        holon::storage::BLOCK_WRITE_TABLE.to_string(),
                        "block".to_string(),
                        "block".to_string(),
                        holon_turso::schema_module::SchemaModule::edge_fields(
                            &holon_turso::schema_modules::BlockSchemaModule,
                        ),
                    )) as Arc<dyn holon_core::OperationProvider>
                },
            ));
            Ok(())
        },
        |injector| async move {
            injector
                .resolve_async::<dyn holon_core::block_ordering::BlockOrdering>()
                .await
        },
    )
    .await
    .expect("boot the SqlOnly engine")
}

/// Write a `Page`-tagged row and wait until the page LiveData holds it.
async fn page_row(
    engine: &holon::api::backend_engine::BackendEngine,
    docs: &LiveDocumentManager,
    id: &EntityUri,
    content: &str,
    parent: &EntityUri,
) {
    let mut row: holon_api::StorageEntity = HashMap::new();
    row.insert("id".into(), Value::String(id.as_str().to_string()));
    row.insert("content".into(), Value::String(content.to_string()));
    row.insert(
        "parent_id".into(),
        Value::String(parent.as_str().to_string()),
    );
    row.insert(
        "tags".into(),
        Value::Array(vec![Value::String("Page".to_string())]),
    );
    engine
        .execute_operation(&EntityName::new("block"), "create", row, OpOrigin::Sync)
        .await
        .unwrap_or_else(|e| panic!("create the page row {id}: {e:#}"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while docs.get_by_id(id).await.unwrap().is_none() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{id} never reached the page LiveData"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn stored_content(
    engine: &holon::api::backend_engine::BackendEngine,
    id: &EntityUri,
) -> Option<String> {
    engine
        .db_handle()
        .query(
            &format!("SELECT content FROM {BLOCK_READ_TABLE} WHERE id = '{id}'"),
            HashMap::new(),
        )
        .await
        .unwrap()
        .first()
        .and_then(|r| r.get("content"))
        .and_then(|v| v.as_string())
        .map(str::to_string)
}

fn music_id() -> EntityUri {
    holon_api::link_parser::PageId::for_path("Music")
        .unwrap()
        .into_entity_uri()
}

fn music(id: EntityUri) -> Block {
    let mut page = Block::new_text(id, EntityUri::no_parent(), "Music");
    page.set_page(true);
    page
}

fn run(
    test: impl AsyncFnOnce(Arc<holon::api::backend_engine::BackendEngine>, LiveDocumentManager),
) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let (engine, ordering) = boot(dir.path().join("db")).await;
        let docs = LiveDocumentManager::new(ordering, engine.db_handle().clone())
            .await
            .expect("LiveDocumentManager");
        test(engine, docs).await;
    });
}

#[test]
fn a_page_slot_at_an_untitled_placeholder_writes_the_title() {
    run(async |engine, docs| {
        let id = music_id();
        let root = EntityUri::no_parent();
        page_row(&engine, &docs, &id, "", &root).await;

        assert_eq!(
            docs.find_by_parent_and_name(&root, "Music").await.unwrap(),
            None
        );
        let slot = docs.page_slot("Music", &root, "Music").await.unwrap();
        assert_eq!(
            slot,
            holon_api::PageSlot::Create(holon_api::link_parser::PageId::for_path("Music").unwrap())
        );
        let created = docs
            .create_forcing_id(music(id.clone()))
            .await
            .expect("create Music");

        assert_eq!(created.title(), "Music", "the returned page");
        assert_eq!(
            stored_content(&engine, &id).await.as_deref(),
            Some("Music"),
            "the stored page row"
        );
    });
}

/// An untitled placeholder under another parent is not this page's slot: the
/// page takes the next id beside it and the placeholder stays where it is.
#[test]
fn a_placeholder_under_another_parent_is_passed() {
    run(async |engine, docs| {
        let id = music_id();
        let root = EntityUri::no_parent();
        let elsewhere = EntityUri::block("elsewhere");
        page_row(&engine, &docs, &elsewhere, "Elsewhere", &root).await;
        page_row(&engine, &docs, &id, "", &elsewhere).await;

        let beside = holon_api::link_parser::PageId::for_path_beside("Music", &id).unwrap();
        let slot = docs.page_slot("Music", &root, "Music").await.unwrap();
        assert_eq!(slot, holon_api::PageSlot::Create(beside.clone()));
        let created = docs
            .create_forcing_id(music(beside.into_entity_uri()))
            .await
            .expect("create Music beside the placeholder");
        assert_eq!(created.parent_id, root);
        assert_eq!(stored_content(&engine, &id).await.as_deref(), Some(""));

        let refused = docs.create_forcing_id(music(id.clone())).await;
        assert!(
            refused
                .as_ref()
                .is_err_and(|e| e.to_string().contains("sits under")),
            "completing the placeholder under another parent: {refused:?}"
        );
    });
}

/// `create_forcing_id` at an id held by a page of another title fails loud;
/// at one held by this page up to case and spacing it returns that page.
#[test]
fn create_forcing_id_returns_only_the_page_of_that_title() {
    run(async |engine, docs| {
        let id = music_id();
        let root = EntityUri::no_parent();
        page_row(&engine, &docs, &id, "music", &root).await;
        let same = docs.create_forcing_id(music(id.clone())).await.unwrap();
        assert_eq!(same.title(), "music", "the first spelling stays");

        let mut other = Block::new_text(id.clone(), root.clone(), "Songs");
        other.set_page(true);
        let refused = docs.create_forcing_id(other).await;
        assert!(
            refused
                .as_ref()
                .is_err_and(|e| e.to_string().contains("that id holds page")),
            "a page of another title at the id: {refused:?}"
        );
    });
}

/// One position, any spelling: a page whose title differs only in case,
/// Unicode normalization or spacing is found, and two pages at one position
/// are an error.
#[test]
fn a_position_is_found_by_any_spelling_and_twins_are_an_error() {
    run(async |engine, docs| {
        let root = EntityUri::no_parent();
        let cafe = EntityUri::block("cafe");
        page_row(&engine, &docs, &cafe, "caf\u{e9}\nits body", &root).await;
        let found = docs
            .find_by_parent_and_name(&root, "CAFE\u{301}")
            .await
            .unwrap()
            .expect("the page titled café");
        assert_eq!(found.id, cafe);

        page_row(
            &engine,
            &docs,
            &EntityUri::block("cafe-twin"),
            "Caf\u{e9}",
            &root,
        )
        .await;
        let twins = docs.find_by_parent_and_name(&root, "caf\u{e9}").await;
        assert!(
            twins
                .as_ref()
                .is_err_and(|e| e.to_string().contains("a page position holds one page")),
            "two pages at one position: {twins:?}"
        );
    });
}
