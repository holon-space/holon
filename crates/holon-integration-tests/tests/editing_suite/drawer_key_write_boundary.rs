//! A drawer key is one token of org: no whitespace, no `:`, not a drawer
//! delimiter. The engine refuses a property write under a key org cannot
//! hold, and the org file keeps the block intact.
//!
//! @pbt kind harness
//! @pbt covers drawer-key-write-boundary — a property key org cannot hold is
//!   refused before it reaches the store or the org file

use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_filesystem::FileSystem;
use holon_integration_tests::TestEnvironment;

const NOTES_ORG: &str = "\
#+TITLE: Notes
#+ID: page-notes

* keep me whole
:PROPERTIES:
:ID: n-target
:END:
";

const TARGET_ID: &str = "block:n-target";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

async fn booted(rt: Arc<tokio::runtime::Runtime>) -> TestEnvironment {
    let t = TestEnvironment::new(rt).expect("TestEnvironment::new");
    t.write_org_file("notes.org", NOTES_ORG)
        .await
        .expect("write notes.org");
    t.start_app(true).await.expect("start_app");
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    while t
        .query_sql(&format!(
            "SELECT id FROM block_raw WHERE id = '{TARGET_ID}'"
        ))
        .await
        .expect("query block_raw")
        .is_empty()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "org scan never populated {TARGET_ID}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    t
}

async fn org_file_after_settle(t: &TestEnvironment) -> String {
    t.wait_for_loro_quiescence(Duration::from_secs(15)).await;
    t.wait_for_cdc_quiescent(Duration::from_millis(250), Duration::from_secs(15))
        .await;
    t.wait_for_org_files_stable(300, Duration::from_secs(15))
        .await;
    t.org_fs
        .read_to_string(&t.org_file_path("notes.org"))
        .await
        .expect("read notes.org")
}

fn refused_case(op: &'static str, key: &'static str, params: Vec<(&'static str, Value)>) {
    let rt = runtime();
    rt.clone().block_on(async move {
        let t = booted(rt.clone()).await;
        let mut p = holon_api::StorageEntity::new();
        for (k, v) in params {
            p.insert(k.into(), v);
        }
        let result = t
            .engine()
            .execute_operation(&EntityName::new("block"), op, p, OpOrigin::User)
            .await;
        let file = org_file_after_settle(&t).await;
        let err = match result {
            Ok(_) => panic!("key {key:?} was accepted; notes.org now:\n{file}"),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            err.contains(&format!("{key:?}")),
            "the refusal must name the key: {err}"
        );
        assert!(
            file.contains(":ID: n-target"),
            "notes.org lost the block:\n{file}"
        );
    });
}

fn text(s: &str) -> Value {
    Value::String(s.to_string())
}

#[test]
fn set_field_under_a_key_with_a_space_is_refused() {
    refused_case(
        "set_field",
        "a b",
        vec![
            ("id", text(TARGET_ID)),
            ("field", text("a b")),
            ("value", text("v")),
        ],
    );
}

#[test]
fn create_with_a_key_holding_a_line_break_is_refused() {
    refused_case(
        "create",
        "k\n* Evil",
        vec![
            ("id", text("block:n-child")),
            ("parent_id", text(TARGET_ID)),
            ("content", text("child")),
            ("k\n* Evil", text("v")),
        ],
    );
}

#[test]
fn a_properties_bag_key_with_a_colon_is_refused() {
    refused_case(
        "create",
        "a:b",
        vec![
            ("id", text("block:n-child")),
            ("parent_id", text(TARGET_ID)),
            ("content", text("child")),
            (
                "properties",
                Value::Object([("a:b".to_string(), text("v"))].into()),
            ),
        ],
    );
}
