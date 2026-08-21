//! `BlockOrdering::place_all` must realize the TOTAL sibling order in whoever
//! owns order under the active capability profile.
//!
//! The trait documents `place_all` as the full-reorder seam (the org re-ingest
//! case: a file's line order is a complete order that one-at-a-time
//! `after_sibling` inserts don't reliably converge to). `SqlBlockOperations`
//! overrides it with a SQL `sort_key` monotonic relabel — correct only when
//! `Consolidator::Store` owns order. Under `Consolidator::Upstream` (Loro) the
//! tree owns the fractional index and the outbound projector overwrites
//! `sort_key` from it, so the relabel writes are inert and the whole call is a
//! silent no-op.
//!
//! Three hand-made blocks, no importer and no org file: create them in one
//! order, state the reverse, read it back.
//!
//! @pbt kind harness
//! @pbt covers block-ordering — place_all total-reorder contract

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_core::block_ordering::BlockCreateRequest;
use holon_core::block_ordering::BlockOrdering;
use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

fn uri(tag: &str) -> EntityUri {
    EntityUri::block(&format!("22222222-0000-4000-8000-00000000{tag}"))
}

#[test]
fn place_all_restates_the_total_sibling_order() {
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .build(rt.clone())
            .await
            .expect("boot an empty vault");
        let ordering = env
            .injector()
            .expect("start_app must capture the injector")
            .resolve_async::<dyn BlockOrdering>()
            .await;

        let parent = uri("aaaa");
        let children: Vec<EntityUri> = ["bbbb", "cccc", "dddd"].iter().map(|t| uri(t)).collect();

        let mut requests = vec![BlockCreateRequest {
            parent_id: EntityUri::no_parent(),
            id: parent.clone(),
            content: BlockContent::Text {
                raw: "parent".to_string(),
            },
            properties: HashMap::new(),
            edges: BlockEdges::default(),
        }];
        for (i, child) in children.iter().enumerate() {
            requests.push(BlockCreateRequest {
                parent_id: parent.clone(),
                id: child.clone(),
                content: BlockContent::Text {
                    raw: format!("child {i}"),
                },
                properties: HashMap::new(),
                edges: BlockEdges::default(),
            });
        }
        let persisted = ordering
            .create_in_tree_batch(&requests)
            .await
            .expect("create the probe blocks");
        assert!(
            persisted.iter().all(|p| *p),
            "the authority declined a probe create: {persisted:?}"
        );

        let as_created = ordering.children(&parent).await.expect("children");
        assert_eq!(as_created, children, "creation order is request order");

        let reversed: Vec<EntityUri> = children.iter().rev().cloned().collect();
        ordering
            .place_all(&parent, &reversed)
            .await
            .expect("place_all the reversed order");
        env.wait_for_loro_quiescence(Duration::from_secs(30)).await;

        let after_place = ordering.children(&parent).await.expect("children");
        assert_eq!(
            after_place, reversed,
            "place_all must restate the total sibling order in the order owner"
        );

        // A second identical statement must be idempotent, not a re-permute.
        ordering
            .place_all(&parent, &reversed)
            .await
            .expect("place_all again");
        env.wait_for_loro_quiescence(Duration::from_secs(30)).await;
        assert_eq!(
            ordering.children(&parent).await.expect("children"),
            reversed,
            "place_all must be idempotent"
        );
    });
}
