use std::sync::Arc;

use holon_core::storage::from_sync;
use holon_core::storage::BlockQuerySource;
use holon_core::storage::BlockSnapshot;
use holon_core::storage::FocusRoot;

use crate::no_turso::from_block_query_source;

fn empty_source() -> Arc<dyn BlockQuerySource> {
    Arc::new(from_sync(|| {
        Ok(BlockSnapshot::from_ordered(
            Vec::new(),
            Vec::<FocusRoot>::new(),
        ))
    })) as Arc<dyn BlockQuerySource>
}

#[tokio::test]
async fn from_block_query_source_has_no_engine_but_a_source() {
    let session = from_block_query_source(empty_source(), None);
    // No Turso query engine wired on a no-Turso session.
    assert!(session.query_engine().is_none());
    // `block_query` is total (non-Option) — a snapshot is always reachable.
    assert!(session.block_query().snapshot().await.unwrap().is_empty());
    // No operation engine passed → operation capability reports absent.
    assert!(session.operation_engine().is_none());
}
