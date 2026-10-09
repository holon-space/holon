//! `LoroDocumentManager` keeps the page position rule of
//! docs/Plans/PageIdentityDeterminism.md §5.3: a position (parent, title key)
//! holds one page, and `create_forcing_id` never writes a page over another.

use std::sync::Arc;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_app::loro_seams::LoroDocumentManager;
use holon_filesystem::DocumentManager;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::loro_document::LoroDocument;

async fn manager() -> (LoroDocumentManager, EntityUri) {
    let doc = LoroDocument::new("page-position".to_string()).expect("loro doc");
    let backend = Arc::new(LoroBackend::from_document(Arc::new(doc)));
    let root = backend.create_placeholder_root("root").await.expect("root");
    (
        LoroDocumentManager::new(backend),
        EntityUri::parse(&root).expect("root id"),
    )
}

fn page(id: &str, parent: &EntityUri, content: &str) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), parent.clone(), content.to_string());
    b.set_page(true);
    b
}

#[tokio::test]
async fn a_position_is_found_by_any_spelling_of_its_title_and_twins_are_an_error() {
    let (docs, root) = manager().await;
    docs.create(page("cafe", &root, "caf\u{e9}\nits body"))
        .await
        .unwrap();

    let found = docs
        .find_by_parent_and_name(&root, "CAFE\u{301}")
        .await
        .unwrap()
        .expect("the page titled café");
    assert_eq!(found.id, EntityUri::block("cafe"));

    docs.create(page("cafe-twin", &root, "Caf\u{e9}"))
        .await
        .unwrap();
    let twins = docs.find_by_parent_and_name(&root, "caf\u{e9}").await;
    assert!(
        twins
            .as_ref()
            .is_err_and(|e| e.to_string().contains("a page position holds one page")),
        "two pages at one position: {twins:?}"
    );
}

#[tokio::test]
async fn create_forcing_id_never_writes_over_another_page() {
    let (docs, root) = manager().await;
    docs.create(page("music", &root, "music")).await.unwrap();

    let same = docs
        .create_forcing_id(page("music", &root, "Music"))
        .await
        .unwrap();
    assert_eq!(same.title(), "music", "the first spelling stays");

    let refused = docs.create_forcing_id(page("music", &root, "Songs")).await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.to_string().contains("that id holds page")),
        "a page of another title at the id: {refused:?}"
    );
    let kept = docs
        .get_by_id(&EntityUri::block("music"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kept.title(), "music");

    docs.create(page("blank", &root, "")).await.unwrap();
    let completed = docs
        .create_forcing_id(page("blank", &root, "Blank"))
        .await
        .unwrap();
    assert_eq!(
        completed.title(),
        "Blank",
        "the placeholder takes the title"
    );
}
