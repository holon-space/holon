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

#[test]
fn a_page_slot_at_an_untitled_placeholder_writes_the_title() {
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

        let id = holon_api::link_parser::PageId::for_path("Music")
            .unwrap()
            .into_entity_uri();
        let mut placeholder: holon_api::StorageEntity = HashMap::new();
        placeholder.insert("id".into(), Value::String(id.as_str().to_string()));
        placeholder.insert("content".into(), Value::String(String::new()));
        placeholder.insert(
            "parent_id".into(),
            Value::String(EntityUri::no_parent().as_str().to_string()),
        );
        placeholder.insert(
            "tags".into(),
            Value::Array(vec![Value::String("Page".to_string())]),
        );
        engine
            .execute_operation(
                &EntityName::new("block"),
                "create",
                placeholder,
                OpOrigin::Sync,
            )
            .await
            .expect("create the untitled placeholder page row");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while docs.get_by_id(&id).await.unwrap().is_none() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the placeholder never reached the page LiveData"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let root = EntityUri::no_parent();
        assert_eq!(
            docs.find_by_parent_and_name(&root, "Music").await.unwrap(),
            None
        );
        let slot = docs.page_slot("Music", &root, "Music").await.unwrap();
        assert_eq!(
            slot,
            holon_api::PageSlot::Create(holon_api::link_parser::PageId::for_path("Music").unwrap())
        );
        let mut page = Block::new_text(id.clone(), root.clone(), "Music");
        page.set_page(true);
        let created = docs.create_forcing_id(page).await.expect("create Music");

        assert_eq!(created.title(), "Music", "the returned page");
        let rows = engine
            .db_handle()
            .query(
                &format!("SELECT content FROM {BLOCK_READ_TABLE} WHERE id = '{id}'"),
                HashMap::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            rows.first()
                .and_then(|r| r.get("content"))
                .and_then(|v| v.as_string()),
            Some("Music"),
            "the stored page row"
        );
    });
}
