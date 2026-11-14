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

/// One `place_all` call against the SQL order owner: `children` created in
/// index order, the rows of `removed` deleted from `block_raw`, then `order`
/// (a permutation of the indices) stated.
#[derive(Debug, Clone)]
struct OrderCase {
    children: usize,
    removed: Vec<usize>,
    order: Vec<usize>,
}

fn order_case() -> impl proptest::strategy::Strategy<Value = OrderCase> {
    use proptest::prelude::*;
    (2usize..=6)
        .prop_flat_map(|n| {
            (
                Just(n),
                proptest::collection::vec(proptest::bool::weighted(0.3), n),
                Just((0..n).collect::<Vec<_>>()).prop_shuffle(),
            )
        })
        .prop_map(|(children, mask, order)| OrderCase {
            children,
            removed: (0..children).filter(|i| mask[*i]).collect(),
            order,
        })
}

/// `(id, sort_key)` of `parent`'s rows in `block_raw`, in `sort_key` order.
async fn stored_order(
    db: &holon::storage::turso::DbHandle,
    parent: &EntityUri,
) -> Vec<(String, String)> {
    db.query(
        &format!(
            "SELECT id, sort_key FROM block_raw WHERE parent_id = '{}' ORDER BY sort_key, id",
            parent.as_str()
        ),
        HashMap::new(),
    )
    .await
    .expect("read the stored sibling order")
    .into_iter()
    .map(|row| {
        let text = |k: &str| match row.get(k) {
            Some(holon_api::Value::String(s)) => s.clone(),
            other => panic!("{k} of a block_raw row: {other:?}"),
        };
        (text("id"), text("sort_key"))
    })
    .collect()
}

/// `place_all` on the SQL order owner over ids some of which have no
/// `block_raw` row: refused before any write, so the stored order and keys
/// stay as they were; with every row present it realizes exactly the stated
/// order, and restating it writes nothing.
#[test]
fn place_all_refuses_ids_without_a_row_and_writes_nothing() {
    use proptest::test_runner::Config;
    use proptest::test_runner::TestCaseError;
    use proptest::test_runner::TestRunner;

    let rt = runtime();
    let env = rt
        .block_on(
            TestEnvironmentBuilder::new()
                .without_loro()
                .build(rt.clone()),
        )
        .expect("boot an empty SQL-only vault");
    let ordering = rt.block_on(
        env.injector()
            .expect("start_app must capture the injector")
            .resolve_async::<dyn BlockOrdering>(),
    );
    let db = env.engine().db_handle().clone();
    let next_parent = std::sync::atomic::AtomicUsize::new(0);

    let check = |case: OrderCase| -> Result<(), TestCaseError> {
        rt.block_on(async {
            let p = next_parent.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let id = |c: usize| EntityUri::block(&format!("33333333-0000-4000-8000-{p:06}{c:06}"));
            let parent = id(999_999);
            let create = |id: &EntityUri, parent: &EntityUri| {
                let params: HashMap<String, holon_api::Value> = [
                    ("id", id.as_str()),
                    ("parent_id", parent.as_str()),
                    ("content", "child"),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_string(), holon_api::Value::String(v.to_string())))
                .collect();
                env.execute_operation("block", "create", params)
            };
            create(&parent, &EntityUri::no_parent())
                .await
                .expect("create the parent");
            for c in 0..case.children {
                create(&id(c), &parent).await.expect("create a child");
            }
            for c in &case.removed {
                db.execute(
                    &format!("DELETE FROM block_raw WHERE id = '{}'", id(*c).as_str()),
                    vec![],
                )
                .await
                .expect("remove a child's row");
            }
            let before = stored_order(&db, &parent).await;
            let stated: Vec<EntityUri> = case.order.iter().map(|c| id(*c)).collect();
            let result = ordering.place_all(&parent, &stated).await;
            let after = stored_order(&db, &parent).await;

            if !case.removed.is_empty() {
                proptest::prop_assert!(
                    result.is_err(),
                    "ids without a row must be refused: {case:?}"
                );
                proptest::prop_assert_eq!(&after, &before, "a refused call must write nothing");
                return Ok(());
            }
            result.map_err(|e| TestCaseError::fail(format!("place_all: {e}")))?;
            let after_ids: Vec<&str> = after.iter().map(|(id, _)| id.as_str()).collect();
            let stated_ids: Vec<&str> = stated.iter().map(|u| u.as_str()).collect();
            proptest::prop_assert_eq!(after_ids, stated_ids, "the stated order is stored");
            ordering
                .place_all(&parent, &stated)
                .await
                .map_err(|e| TestCaseError::fail(format!("place_all again: {e}")))?;
            proptest::prop_assert_eq!(
                stored_order(&db, &parent).await,
                after,
                "restating the stored order writes nothing"
            );
            Ok(())
        })
    };

    // A key is written before the row-less id is reached.
    check(OrderCase {
        children: 3,
        removed: vec![2],
        order: vec![1, 0, 2],
    })
    .expect("static example");
    TestRunner::new(Config {
        cases: 24,
        failure_persistence: None,
        ..Config::default()
    })
    .run(&order_case(), check)
    .expect("place_all over generated sibling sets");
}
