//! `file.document_id` is restored at boot as the document the file was
//! recorded against: the id written comes back as the same id, and text that
//! names no document is refused by name instead of becoming another id.

use std::collections::HashMap;
use std::sync::Arc;

use holon::core::queryable_cache::QueryableCache;
use holon::di::test_helpers::create_test_engine_with_setup;
use holon::storage::DbHandle;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_app::turso_seams::CacheBlockReader;
use holon_filesystem::BlockReader;
use holon_filesystem::FileProjection;

async fn reader() -> (CacheBlockReader, DbHandle) {
    let engine = create_test_engine_with_setup(":memory:".into(), |_| Ok(()))
        .await
        .unwrap();
    let db = engine.db_handle().clone();
    let mut block_raw = Block::type_definition();
    block_raw.name = "block_raw".to_string();
    let cache = Arc::new(
        QueryableCache::<Block>::new(db.clone(), block_raw)
            .await
            .unwrap(),
    );
    (CacheBlockReader::new(cache), db)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_recorded_document_is_restored_as_the_same_id() {
    let (reader, _db) = reader().await;
    let documents = [
        "block:notes-doc",
        "block:a#b",
        "block:a?b",
        "block::split-1",
        "block:a:b",
        "file:Notes/Sub.org",
        "file:Pancakes.cook",
    ];
    for (n, raw) in documents.iter().enumerate() {
        let projection = FileProjection {
            content_hash: format!("hash-{n}"),
            document_id: Some(EntityUri::parse(raw).unwrap()),
            read_only_blocks: Vec::new(),
        };
        reader
            .persist_file_projection(
                &EntityUri::file(&format!("f{n}.org")),
                &format!("f{n}.org"),
                ".",
                &projection,
            )
            .await
            .unwrap_or_else(|e| panic!("{raw}: {e:#}"));
    }
    let restored: HashMap<String, Option<EntityUri>> = reader
        .load_file_projections()
        .await
        .unwrap_or_else(|e| panic!("{e:#}"))
        .into_iter()
        .map(|(file, p)| (file.to_string(), p.document_id))
        .collect();
    for (n, raw) in documents.iter().enumerate() {
        let file = EntityUri::file(&format!("f{n}.org")).to_string();
        assert_eq!(
            restored[&file].as_ref().map(EntityUri::as_str),
            Some(*raw),
            "{file} was recorded against {raw}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stored_text_that_names_no_document_is_refused_by_name() {
    for text in [":x", "a b", "café", "a%zz", "a#b", "block:"] {
        let (reader, db) = reader().await;
        let mut row = HashMap::new();
        row.insert("doc".to_string(), Value::String(text.to_string()));
        db.query(
            "INSERT INTO file (id, name, parent_id, content_hash, document_id) VALUES \
             ('file:bad.org', 'bad.org', '.', 'h', $doc)",
            row,
        )
        .await
        .unwrap();
        let restored = tokio::spawn(async move { reader.load_file_projections().await })
            .await
            .unwrap_or_else(|panic| panic!("restoring {text:?} panicked: {panic}"));
        match restored {
            Ok(rows) => panic!("{text:?} was restored as {:?}", rows[0].1.document_id),
            Err(e) => {
                let msg = format!("{e:#}");
                assert!(
                    msg.contains(&format!("{text:?}")) && msg.contains("file:bad.org"),
                    "the refusal names the text and the file: {msg}"
                );
            }
        }
    }
}
