//! Increment 4, store-boundary contract: what the importer HANDS the store.
//!
//! A recording [`BlockOrdering`] double captures the two calls the ingest makes
//! — the create batch and the per-parent order statement — so their contract
//! can be asserted exactly and deterministically. Two things must hold, and
//! both are silent-corruption spots rather than crashes:
//!
//!  1. Every block is created AFTER its parent. `create_in_tree_batch` creates
//!     in request order, so a child that arrives first has nowhere to attach.
//!  2. The sibling sequence handed to `place_all` is LogSeq's fracdex order.
//!     Re-minting a WRONG sequence produces a graph that looks healthy and
//!     reads in the wrong order (risk 6 in the plan's register).
//!
//! That the re-minted keys land in a real store is the separate integration
//! keystone; this pins the intent the store is given.

use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::EntityUri;
use holon_core::block_ordering::BlockCreateRequest;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result;
use holon_logseq_db::ingest::enter_store;
use holon_logseq_db::project;
use holon_logseq_db::read_datoms;

/// The Aug-20 journal's children, in the order their fracdex `:block/order`
/// strings imply. Measured directly from the fixture with the stage-0 spike's
/// Python decoder, independently of the Rust under test.
const AUG20_UUID: &str = "00000001-2026-0820-0000-000000000000";
const AUG20_CHILDREN_IN_FRACDEX_ORDER: &[&str] = &[
    // e195  order a0
    "6a86c98a-d818-4787-8ff1-e3b619b15f2d",
    // e214  order a01
    "6a86d4a0-4ae2-4ecd-98bf-c5a10e36604a",
    // e213  order a02
    "6a86d453-ebf9-4a43-a4dc-6a29f19fee38",
    // e211  order a04
    "6a86d3d4-82f8-4bb7-becc-a06ffb3e814e",
    // e209  order a08
    "6a86cfb1-07a4-4c07-9c7b-477438c99fad",
    // e206  order a0G
    "6a86cf5f-2cc4-4f32-b6ba-9496235db709",
    // e202  order a0V
    "6a86ce83-7433-4313-b8a8-57295bb08feb",
    // e200  order a1
    "6a86cdb3-e5af-4a4b-8bca-4100966474b1",
    // e203  order a2
    "6a86ce9d-9fe6-434e-b07f-bd629bb68ae9",
];

const EXPECTED_BLOCKS: usize = 206;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logseq-db/holontest.sqlite")
}

#[derive(Default)]
struct RecordingOrdering {
    created: Mutex<Vec<(EntityUri, EntityUri)>>,
    ordered: Mutex<Vec<(EntityUri, Vec<EntityUri>)>>,
}

#[async_trait]
impl BlockOrdering for RecordingOrdering {
    async fn place(&self, _: &EntityUri, _: &EntityUri, _: Option<&EntityUri>) -> Result<()> {
        Ok(())
    }

    async fn place_all(&self, parent_id: &EntityUri, ordered_ids: &[EntityUri]) -> Result<()> {
        self.ordered
            .lock()
            .unwrap()
            .push((parent_id.clone(), ordered_ids.to_vec()));
        Ok(())
    }

    async fn create_in_tree_batch(&self, requests: &[BlockCreateRequest]) -> Result<Vec<bool>> {
        let mut created = self.created.lock().unwrap();
        for request in requests {
            created.push((request.id.clone(), request.parent_id.clone()));
        }
        Ok(vec![true; requests.len()])
    }

    async fn prev_sibling(&self, _: &EntityUri) -> Result<Option<EntityUri>> {
        Ok(None)
    }
    async fn next_sibling(&self, _: &EntityUri) -> Result<Option<EntityUri>> {
        Ok(None)
    }
    async fn first_child(&self, _: &EntityUri) -> Result<Option<EntityUri>> {
        Ok(None)
    }
    async fn last_child(&self, _: &EntityUri) -> Result<Option<EntityUri>> {
        Ok(None)
    }

    // A read-only import never updates, deletes, or re-reads children; these
    // panic rather than returning a plausible answer, so the test fails loudly
    // if the ingest ever starts using them.
    async fn children(&self, _: &EntityUri) -> Result<Vec<EntityUri>> {
        unimplemented!("import never re-reads children")
    }
    async fn update_in_tree(&self, _: holon_api::StorageEntity) -> Result<()> {
        unimplemented!("import never updates")
    }
    async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> Result<()> {
        unimplemented!("import never deletes")
    }
}

#[tokio::test]
async fn import_hands_the_store_a_creatable_batch_in_fracdex_order() {
    let set = read_datoms(&fixture_path())
        .await
        .expect("read HolonTest datoms");
    let projection = project(&set).expect("project blocks");
    let ordering = RecordingOrdering::default();

    let report = enter_store(&projection, &ordering)
        .await
        .expect("enter the store");
    assert_eq!(report.blocks_created, EXPECTED_BLOCKS);

    // --- 1. parents precede children ---
    let created = ordering.created.lock().unwrap();
    assert_eq!(
        created.len(),
        EXPECTED_BLOCKS,
        "every block is created once"
    );
    let mut seen: Vec<EntityUri> = Vec::with_capacity(created.len());
    for (id, parent) in created.iter() {
        if *parent != EntityUri::no_parent() {
            assert!(
                seen.contains(parent),
                "block {id} is created before its parent {parent}"
            );
        }
        seen.push(id.clone());
    }

    // --- 2. the stated sibling sequence is the fracdex sequence ---
    let ordered = ordering.ordered.lock().unwrap();
    let aug20 = EntityUri::block(AUG20_UUID);
    let (_, children) = ordered
        .iter()
        .find(|(parent, _)| *parent == aug20)
        .expect("the Aug-20 journal's order was stated");
    let expected: Vec<EntityUri> = AUG20_CHILDREN_IN_FRACDEX_ORDER
        .iter()
        .map(|uuid| EntityUri::block(uuid))
        .collect();
    assert_eq!(
        children, &expected,
        "the sibling sequence handed to place_all must be LogSeq's fracdex order"
    );

    // Every parent that has children states an order, so no sibling group is
    // left to whatever sequence the create batch happened to use.
    assert_eq!(
        ordered.len(),
        report.parents_ordered,
        "one place_all per parent with children"
    );
    assert!(report.parents_ordered > 0, "some parent has children");
}
