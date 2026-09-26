//! An `:ID:` drawer line is a block's bare id, not free text. The engine
//! refuses a write that puts any other id on a route that reaches the line,
//! and the org file keeps the block intact.
//!
//! @pbt kind harness
//! @pbt covers drawer-id-write-boundary — an id that is not a bare block id
//!   is refused before it reaches the store or the org file

use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_filesystem::FileSystem;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::test_tracing;

const NOTES_ORG: &str = "\
#+TITLE: Notes
#+ID: page-notes

* keep me whole
:PROPERTIES:
:ID: n-target
:END:
";

const TARGET_ID: &str = "block:n-target";
const BAD_ID: &str = "n-target\nline one\n* Evil heading\n:PROPERTIES:\n:ID: hijack\n:END:";

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

fn refused_case(op: &'static str, params: fn() -> Vec<(&'static str, Value)>) {
    let rt = runtime();
    rt.clone().block_on(async move {
        let t = booted(rt.clone()).await;
        let before = org_file_after_settle(&t).await;
        let mut p = holon_api::StorageEntity::new();
        for (k, v) in params() {
            p.insert(k.into(), v);
        }
        let result = t
            .engine()
            .execute_operation(&EntityName::new("block"), op, p, OpOrigin::User)
            .await;
        let file = org_file_after_settle(&t).await;
        let err = match result {
            Ok(_) => panic!("the id {BAD_ID:?} was accepted; notes.org now:\n{file}"),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            err.contains(&format!("{BAD_ID:?}")) && err.contains("ID"),
            "the refusal must name the id and its value: {err}"
        );
        assert_eq!(file, before, "notes.org changed");
    });
}

fn text(s: &str) -> Value {
    Value::String(s.to_string())
}

#[test]
fn set_field_of_the_org_drawer_carrier_with_a_multi_line_id_is_refused() {
    refused_case("set_field", || {
        vec![
            ("id", text(TARGET_ID)),
            ("field", text("org_properties")),
            (
                "value",
                text(&serde_json::json!({ "ID": BAD_ID, "note": "keep" }).to_string()),
            ),
        ]
    });
}

#[test]
fn set_field_of_a_multi_line_id_property_is_refused() {
    refused_case("set_field", || {
        vec![
            ("id", text(TARGET_ID)),
            ("field", text("ID")),
            ("value", text(BAD_ID)),
        ]
    });
}

#[test]
fn create_with_a_multi_line_id_property_is_refused() {
    refused_case("create", || {
        vec![
            ("id", text("block:n-child")),
            ("parent_id", text(TARGET_ID)),
            ("content", text("child")),
            ("ID", text(BAD_ID)),
        ]
    });
}

#[test]
fn a_multi_line_id_in_the_properties_bag_is_refused() {
    refused_case("create", || {
        vec![
            ("id", text("block:n-child")),
            ("parent_id", text(TARGET_ID)),
            ("content", text("child")),
            (
                "properties",
                Value::Object([("ID".to_string(), text(BAD_ID))].into()),
            ),
        ]
    });
}

#[test]
fn a_multi_line_id_in_the_file_drawer_carrier_is_refused() {
    refused_case("set_field", || {
        vec![
            ("id", text("block:page-notes")),
            ("field", text("file_properties")),
            (
                "value",
                text(&serde_json::json!({ "ID": BAD_ID }).to_string()),
            ),
        ]
    });
}

/// A write that reaches the store past the engine (here: a sync-origin write
/// straight into the dispatcher) cannot be refused there. The renderer then
/// writes the block's own id, discloses the dropped carrier id, and the rest
/// of the file still reaches disk.
#[test]
fn a_bad_carrier_id_past_the_engine_does_not_freeze_the_file() {
    let collector = test_tracing::SpanCollector::global();
    let scope = test_tracing::begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.worker_threads(2).enable_all();
    test_tracing::attach_scope_to_runtime(&mut builder, scope);
    let rt = Arc::new(builder.build().expect("build runtime"));
    rt.clone().block_on(async move {
        let t = booted(rt.clone()).await;
        let mut p = holon_api::StorageEntity::new();
        p.insert("id".into(), text(TARGET_ID));
        p.insert("field".into(), text("org_properties"));
        p.insert(
            "value".into(),
            text(&serde_json::json!({ "ID": BAD_ID, "note": "keep" }).to_string()),
        );
        t.engine()
            .get_dispatcher()
            .execute_operation_with_provenance(
                &EntityName::new("block"),
                "set_field",
                p,
                holon::api::operation_dispatcher::AuthoredInput::Verbatim,
                OpOrigin::Sync,
            )
            .await
            .expect("the dispatcher stores the write");
        let file = org_file_after_settle(&t).await;
        let errors = collector.captured_problems();
        let warnings = collector.captured_warnings();
        assert!(
            file.contains(":ID: n-target\n") && file.contains(":note: keep\n"),
            "write-back did not reach notes.org; errors: {errors:#?}\nnotes.org:\n{file}"
        );
        assert!(
            !file.lines().any(|l| l.starts_with("* Evil")),
            "the id broke out of its drawer line:\n{file}"
        );
        assert!(errors.is_empty(), "write-back raised errors: {errors:#?}");
        assert!(
            warnings
                .iter()
                .any(|w| w.message.contains(&format!("{BAD_ID:?}"))),
            "the dropped carrier id was not disclosed; warnings: {warnings:#?}"
        );
    });
}

/// Ids that are not a bare block id: a schemed URI, a drawer delimiter, org's
/// append or headline syntax, a non-URI-safe character, an overlong token.
const NOT_BARE_IDS: &[&str] = &[
    "block:n-scheme",
    "doc:n-scheme",
    "file:n-scheme",
    "sentinel:no_parent",
    ":END:",
    ":PROPERTIES:",
    "#+ID:",
    "*kid",
    "a:b",
    "kid+",
];

/// `(op, route, params)`.
type Route = (&'static str, &'static str, Vec<(&'static str, Value)>);

#[test]
fn an_id_that_is_not_a_bare_block_id_is_refused_on_every_route() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let t = booted(rt.clone()).await;
        let overlong = "x".repeat(5000);
        let ids: Vec<&str> = NOT_BARE_IDS
            .iter()
            .copied()
            .chain([overlong.as_str()])
            .collect();
        let mut accepted = Vec::new();
        for id in ids {
            let routes: Vec<Route> = vec![
                (
                    "set_field",
                    "org_properties",
                    vec![
                        ("id", text(TARGET_ID)),
                        ("field", text("org_properties")),
                        (
                            "value",
                            text(&serde_json::json!({ "ID": id, "note": "keep" }).to_string()),
                        ),
                    ],
                ),
                (
                    "set_field",
                    "ID",
                    vec![
                        ("id", text(TARGET_ID)),
                        ("field", text("ID")),
                        ("value", text(id)),
                    ],
                ),
                (
                    "set_field",
                    "file_properties",
                    vec![
                        ("id", text("block:page-notes")),
                        ("field", text("file_properties")),
                        ("value", text(&serde_json::json!({ "ID": id }).to_string())),
                    ],
                ),
                (
                    "create",
                    "id",
                    vec![
                        ("id", text(&format!("block:{id}"))),
                        ("parent_id", text(TARGET_ID)),
                        ("content", text("child")),
                    ],
                ),
            ];
            for (op, route, params) in routes {
                let before = org_file_after_settle(&t).await;
                let mut p = holon_api::StorageEntity::new();
                for (k, v) in params {
                    p.insert(k.into(), v);
                }
                let result = t
                    .engine()
                    .execute_operation(&EntityName::new("block"), op, p, OpOrigin::User)
                    .await;
                let file = org_file_after_settle(&t).await;
                let shown: String = id.chars().take(40).collect();
                match result {
                    Ok(_) => accepted.push(format!("{route}: {shown:?} accepted")),
                    Err(e) if file != before => accepted.push(format!(
                        "{route}: {shown:?} refused ({e:#}) but the file changed"
                    )),
                    Err(e) => {
                        let err = format!("{e:#}");
                        if !err.contains(&format!("{id:?}")) {
                            accepted.push(format!(
                                "{route}: {shown:?} refused without naming it: {err}"
                            ));
                        }
                    }
                }
            }
        }
        assert!(
            accepted.is_empty(),
            "ids that are not bare block ids got through:\n{}",
            accepted.join("\n")
        );
    });
}
