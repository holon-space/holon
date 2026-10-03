#![cfg(feature = "pbt")]
//! A focus on a block created just before it commits to SQL while the block's
//! Loro commit still waits for its projection. The view engine must not
//! release the focus root before the block: every released version that holds
//! the focus row holds the block too (`inv-engine-focus-root-present`).
//!
//! The keystone settles every projection after each transition, so no
//! keystone sequence puts a focus inside a block's projection window; the lag
//! here holds that window open.
//!
//! ## Why this binary holds exactly ONE test
//!
//! Same reason as `projector_lag_lock.rs`: the lag is a process-global
//! environment variable read by the projector on every pass, so a second test
//! here would silently run under it.
//!
//! @pbt kind harness
//! @pbt covers engine-focus-root-present — a focus root released ahead of the
//! Loro commit that creates its block, under projection lag

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::commit_clock::CommitClock;
use holon_api::commit_clock::CommitSource;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_views::batch::Multiset;
use holon_views::batch::add;
use holon_views::engine::Field;
use holon_views::engine::ViewBatch;
use holon_views::engine::ViewEngine;
use holon_views::views::View;

const LAG_MS: &str = "3000";

const PARENT: &str = "block:focus-lag-parent";
const TARGET: &str = "block:focus-lag-target";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

/// Each released version of a view as `(below, rows)`, folded from its
/// snapshot and every batch received since.
fn versions(
    snapshot: ViewBatch,
    batches: &Receiver<ViewBatch>,
) -> Vec<(u64, Multiset<Vec<Field>>)> {
    let mut state = Multiset::new();
    let mut out = Vec::new();
    for batch in std::iter::once(snapshot).chain(batches.try_iter()) {
        for (row, n) in batch.deltas {
            add(&mut state, row, n);
        }
        out.push((batch.below.get(), state.clone()));
    }
    out
}

/// The version of `versions` released at or before `below`.
fn at(versions: &[(u64, Multiset<Vec<Field>>)], below: u64) -> &Multiset<Vec<Field>> {
    &versions
        .iter()
        .rev()
        .find(|(b, _)| *b <= below)
        .unwrap_or_else(|| panic!("no Row version at or before {below}"))
        .1
}

#[test]
fn a_focus_root_is_released_no_earlier_than_its_block() {
    // SAFETY: single-threaded test entry, before any engine, runtime or
    // projector task exists, and this binary holds no other test (module docs).
    unsafe {
        std::env::set_var("HOLON_TEST_PROJECTOR_LAG_MS", LAG_MS);
    }

    let rt = runtime();
    rt.clone().block_on(async move {
        let world = TestEnvironmentBuilder::new()
            .with_org_file(
                "FocusLag.org".to_string(),
                "#+TITLE: Focus Lag\n* Parent\n:PROPERTIES:\n:ID: focus-lag-parent\n:END:\n"
                    .to_string(),
            )
            .build(rt.clone())
            .await
            .expect("boot the vault");
        world
            .wait_for_loro_quiescence(Duration::from_secs(120))
            .await;
        world
            .wait_for_cdc_quiescent(Duration::from_millis(300), Duration::from_secs(120))
            .await;
        let engine = world
            .injector()
            .expect("the world booted")
            .resolve::<ViewEngine>();
        let (focus_start, focus_batches) = engine
            .snapshot_and_subscribe(View::FocusRoots)
            .expect("the view engine runs");
        let (row_start, row_batches) = engine
            .snapshot_and_subscribe(View::Row)
            .expect("the view engine runs");

        world
            .create_block(TARGET, PARENT, "Target")
            .await
            .expect("create the target block");
        world
            .execute_operation(
                "navigation",
                "focus",
                HashMap::from([
                    ("region".to_string(), Value::String("main".to_string())),
                    ("block_id".to_string(), Value::String(TARGET.to_string())),
                ]),
            )
            .await
            .expect("focus the target block");
        let projected = world
            .query_sql(&format!("SELECT id FROM block_raw WHERE id = '{TARGET}'"))
            .await
            .expect("query the projection");
        let clock = world
            .injector()
            .expect("the world booted")
            .resolve::<CommitClock>();
        assert!(
            clock.low_watermark() < clock.high_water(),
            "no Loro commit is outstanding below the focus commit (Loro {:?}), so the engine has \
             nothing to hold the focus root back for — raise HOLON_TEST_PROJECTOR_LAG_MS",
            clock.outstanding(CommitSource::LoroGlobal),
        );
        assert!(
            projected.is_empty(),
            "the projection already holds the target, so this run does not exercise the lag \
             window at all — raise HOLON_TEST_PROJECTOR_LAG_MS"
        );

        world
            .wait_for_loro_quiescence(Duration::from_secs(120))
            .await;
        world
            .wait_for_cdc_quiescent(Duration::from_millis(300), Duration::from_secs(120))
            .await;
        engine
            .snapshot_and_subscribe(View::Row)
            .expect("the view engine still runs");

        let target = EntityUri::parse(TARGET).unwrap();
        let rows = versions(row_start, &row_batches);
        let mut focused = 0;
        for (below, roots) in versions(focus_start, &focus_batches) {
            let holds_focus = roots
                .iter()
                .any(|(root, _)| matches!(&root[..], [_, Field::Uri(r), _] if *r == target));
            if !holds_focus {
                continue;
            }
            focused += 1;
            let holds_block = at(&rows, below)
                .iter()
                .any(|(row, _)| matches!(row.first(), Some(Field::Uri(id)) if *id == target));
            assert!(
                holds_block,
                "[inv-engine-focus-root-present] the version below Stamp({below}) holds the focus \
                 root {TARGET} but not the block"
            );
        }
        assert!(
            focused > 0,
            "no released version holds the focus root {TARGET}"
        );
    });
}
