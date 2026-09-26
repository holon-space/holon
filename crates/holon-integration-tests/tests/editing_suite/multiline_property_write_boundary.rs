//! A drawer property is one line of org. The engine refuses a property write
//! whose value holds a line break, and the org file keeps the block intact.
//!
//! @pbt kind harness
//! @pbt covers multiline-property-write-boundary — a line break in a
//!   property value is refused before it reaches the store or the org file

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
const INJECTION: &str = "line one\n* Evil heading\n:PROPERTIES:\n:ID: hijack\n:END:";

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

async fn write(t: &TestEnvironment, op: &str, params: &[(&str, &str)]) -> anyhow::Result<()> {
    let mut p = holon_api::StorageEntity::new();
    for (k, v) in params {
        p.insert((*k).into(), Value::String((*v).to_string()));
    }
    t.engine()
        .execute_operation(&EntityName::new("block"), op, p, OpOrigin::User)
        .await
        .map(|_| ())
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

fn assert_refused_and_intact(result: anyhow::Result<()>, file: &str, block: &str) {
    let err = match result {
        Ok(()) => panic!("a line break in a property landed; notes.org now:\n{file}"),
        Err(e) => format!("{e:#}"),
    };
    assert!(
        err.contains(block) && err.contains("note"),
        "the refusal must name the block and the key: {err}"
    );
    assert!(
        file.contains(":ID: n-target"),
        "notes.org lost the block:\n{file}"
    );
    assert!(
        !file.contains("Evil heading"),
        "notes.org took the value:\n{file}"
    );
}

#[test]
fn set_field_of_a_multi_line_property_is_refused() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let t = booted(rt.clone()).await;
        let result = write(
            &t,
            "set_field",
            &[("id", TARGET_ID), ("field", "note"), ("value", INJECTION)],
        )
        .await;
        let file = org_file_after_settle(&t).await;
        assert_refused_and_intact(result, &file, TARGET_ID);
    });
}

#[test]
fn create_with_a_multi_line_property_is_refused() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let t = booted(rt.clone()).await;
        let result = write(
            &t,
            "create",
            &[
                ("id", "block:n-child"),
                ("parent_id", TARGET_ID),
                ("content", "child"),
                ("note", INJECTION),
            ],
        )
        .await;
        let file = org_file_after_settle(&t).await;
        assert_refused_and_intact(result, &file, "block:n-child");
    });
}

async fn write_values(
    t: &TestEnvironment,
    op: &str,
    params: Vec<(&str, Value)>,
) -> anyhow::Result<()> {
    let mut p = holon_api::StorageEntity::new();
    for (k, v) in params {
        p.insert(k.into(), v);
    }
    t.engine()
        .execute_operation(&EntityName::new("block"), op, p, OpOrigin::User)
        .await
        .map(|_| ())
}

fn bag() -> Value {
    Value::Object([("note".to_string(), Value::String(INJECTION.into()))].into())
}

fn bag_json() -> String {
    serde_json::json!({ "note": INJECTION }).to_string()
}

fn text(s: &str) -> Value {
    Value::String(s.to_string())
}

fn refused_case(op: &'static str, block: &'static str, params: fn() -> Vec<(&'static str, Value)>) {
    let rt = runtime();
    rt.clone().block_on(async move {
        let t = booted(rt.clone()).await;
        let result = write_values(&t, op, params()).await;
        let file = org_file_after_settle(&t).await;
        assert_refused_and_intact(result, &file, block);
    });
}

#[test]
fn create_with_a_multi_line_value_in_the_properties_object_is_refused() {
    refused_case("create", "block:n-child", || {
        vec![
            ("id", text("block:n-child")),
            ("parent_id", text(TARGET_ID)),
            ("content", text("child")),
            ("properties", bag()),
        ]
    });
}

#[test]
fn create_with_a_multi_line_value_in_the_properties_json_is_refused() {
    refused_case("create", "block:n-child", || {
        vec![
            ("id", text("block:n-child")),
            ("parent_id", text(TARGET_ID)),
            ("content", text("child")),
            ("properties", text(&bag_json())),
        ]
    });
}

#[test]
fn set_field_of_the_whole_properties_bag_with_a_multi_line_value_is_refused() {
    refused_case("set_field", TARGET_ID, || {
        vec![
            ("id", text(TARGET_ID)),
            ("field", text("properties")),
            ("value", bag()),
        ]
    });
}

#[test]
fn set_field_of_the_org_drawer_carrier_with_a_multi_line_value_is_refused() {
    refused_case("set_field", TARGET_ID, || {
        vec![
            ("id", text(TARGET_ID)),
            ("field", text("org_properties")),
            ("value", text(&bag_json())),
        ]
    });
}
