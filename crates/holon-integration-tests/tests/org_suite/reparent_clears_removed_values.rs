//! A block the store holds outside any page, adopted by an org file that no
//! longer authors its priority, `SCHEDULED:` line or drawer property, must not
//! keep those values: the ingesting file is authoritative for the block.
//!
//! This is the ingest's re-parent path (`find_foreign_blocks` → an `update`
//! for a block absent from the file's diff base). A headline moved between two
//! page files never reaches it — the cross-doc membership guard or a delete +
//! create handles that — so the stored block here is a top-level one.
//!
//! Entry `2026-09-26-org-ingest-never-clears-a-removed-planning-line`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_api::Value;
use holon_integration_tests::TestEnvironment;

const ADOPTED: &str = "block:adopted-h";

const BETA_STRIPPED: &str = "\
#+TITLE: Beta
#+ID: page-beta
* TODO Buy milk
:PROPERTIES:
:ID: adopted-h
:END:
";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

async fn stored_row(t: &TestEnvironment) -> Option<(Value, HashMap<String, Value>)> {
    let rows = t
        .query_sql(&format!(
            "SELECT parent_id, properties FROM block_raw WHERE id = '{ADOPTED}'"
        ))
        .await
        .expect("query block_raw");
    rows.first().map(|r| {
        let parent = r.get("parent_id").cloned().expect("parent_id column");
        let bag = match r.get("properties") {
            Some(Value::Object(bag)) => bag.clone(),
            other => panic!("`properties` is not an object: {other:?}"),
        };
        (parent, bag)
    })
}

fn adopted_block_loses_what_the_file_does_not_author(loro: bool) {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut t = TestEnvironment::new(rt).expect("TestEnvironment::new");
        t.set_enable_loro(loro);
        t.start_app(true).await.expect("start_app");

        let params = HashMap::from([
            ("id".to_string(), Value::String(ADOPTED.to_string())),
            (
                "parent_id".to_string(),
                Value::String(EntityUri::no_parent().to_string()),
            ),
            ("content".to_string(), Value::String("Buy milk".to_string())),
            (
                "content_type".to_string(),
                Value::String("text".to_string()),
            ),
            ("task_state".to_string(), Value::String("TODO".to_string())),
            ("priority".to_string(), Value::Integer(1)),
            (
                "scheduled".to_string(),
                Value::String("<2026-01-01 Thu>".to_string()),
            ),
            ("aisle".to_string(), Value::String("3".to_string())),
        ]);
        t.execute_operation("block", "create", params)
            .await
            .expect("create the top-level block");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            if stored_row(&t)
                .await
                .is_some_and(|(_, bag)| bag.contains_key("aisle"))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the top-level block never landed: {:?}",
                stored_row(&t).await
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        t.write_org_file("Beta.org", BETA_STRIPPED)
            .await
            .expect("write Beta.org");
        let seq = t.org_fs.last_change_seq();
        t.wait_for_org_change_processed(seq, Duration::from_secs(20))
            .await;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let (parent, bag) = loop {
            let row = stored_row(&t).await.expect("the adopted block is gone");
            if row.0 == Value::String("block:page-beta".to_string()) {
                break row;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Beta.org never adopted the block: {row:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        t.wait_for_org_files_stable(300, Duration::from_secs(10))
            .await;
        let (_, bag_after_settle) = stored_row(&t).await.expect("the adopted block is gone");
        assert_eq!(bag, bag_after_settle, "the stored bag was still changing");

        for key in ["priority", "scheduled", "aisle"] {
            assert_eq!(
                bag.get(key),
                None,
                "loro={loro}: Beta.org authors no `{key}`, yet the adopted block kept it \
                 (parent {parent:?}): {bag:?}"
            );
        }
        t.stop_app().await.expect("stop_app");
    });
}

#[test]
fn adopted_block_loses_what_the_file_does_not_author_sqlonly() {
    adopted_block_loses_what_the_file_does_not_author(false);
}

#[test]
fn adopted_block_loses_what_the_file_does_not_author_loro() {
    adopted_block_loses_what_the_file_does_not_author(true);
}
