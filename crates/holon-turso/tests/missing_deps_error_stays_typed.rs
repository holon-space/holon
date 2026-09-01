//! Contract: a matview over tables nobody registers fails with a `StorageError`
//! the render path can still recognise.
//!
//! The UI watcher attributes such a failure to the integration that owns the
//! missing tables, which it can only do by downcasting to
//! `StorageError::MissingDependencies`. Spelling the cause into a fresh
//! `anyhow!` — as `MatviewManager::ensure_view` once did — erases the variant
//! and the block is left showing five internal identifiers and a matview hash.

use std::sync::Arc;

use holon_core::storage::types::StorageError;
use holon_turso::matview_manager::MatviewManager;
use holon_turso::turso::TursoBackend;

#[tokio::test]
async fn a_matview_over_an_unregistered_table_fails_with_a_downcastable_missing_dependencies() {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);

    // Registration closed with nothing having promised `cc_session` — exactly
    // the state an integration that failed to connect leaves behind.
    handle.transition_to_ready().await.expect("go Ready");

    let mgr = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
    let Err(err) = mgr
        .watch("SELECT id, title FROM cc_session WHERE message_count > 0")
        .await
    else {
        panic!("a matview over a table nobody registers must fail, not park");
    };

    let Some(StorageError::MissingDependencies { missing, .. }) =
        err.downcast_ref::<StorageError>()
    else {
        panic!(
            "the matview failure must still be a typed MissingDependencies for the render path \
             to attribute it; got: {err:#}"
        );
    };
    assert_eq!(missing, &vec!["cc_session".to_string()]);

    assert!(
        format!("{err:#}").contains("cc_session"),
        "the chained rendering must still carry the cause: {err:#}"
    );
}
