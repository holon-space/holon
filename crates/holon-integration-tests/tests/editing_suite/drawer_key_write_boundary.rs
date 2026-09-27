//! A headline drawer key is one token of org that reads back as itself: no
//! whitespace, no `:`, not a drawer delimiter, no trailing `+`, not a
//! spelling of `ID`. The file-level drawer has its own reader and its own
//! key rule. The engine refuses a property write under a key its carrier
//! cannot hold, and the org file keeps the block intact.
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

#[test]
fn set_field_under_an_append_key_is_refused() {
    refused_case(
        "set_field",
        "note+",
        vec![
            ("id", text(TARGET_ID)),
            ("field", text("note+")),
            ("value", text("v")),
        ],
    );
}

#[test]
fn set_field_under_a_case_variant_of_id_is_refused() {
    refused_case(
        "set_field",
        "Id",
        vec![
            ("id", text(TARGET_ID)),
            ("field", text("Id")),
            ("value", text("v")),
        ],
    );
}

#[test]
fn a_properties_bag_key_spelled_id_in_lower_case_is_refused() {
    refused_case(
        "create",
        "id",
        vec![
            ("id", text("block:n-child")),
            ("parent_id", text(TARGET_ID)),
            ("content", text("child")),
            (
                "properties",
                Value::Object([("id".to_string(), text("v"))].into()),
            ),
        ],
    );
}

#[test]
fn an_org_drawer_carrier_key_spelled_id_in_mixed_case_is_refused() {
    refused_case(
        "set_field",
        "iD",
        vec![
            ("id", text(TARGET_ID)),
            ("field", text("org_properties")),
            (
                "value",
                text(&serde_json::json!({ "ID": "n-target", "iD": "v" }).to_string()),
            ),
        ],
    );
}

/// The file-level drawer has its own reader, which keeps a trailing `+` in a
/// key; the engine accepts such a key there and it reaches the file.
#[test]
fn a_file_drawer_key_ending_in_plus_is_written() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let t = booted(rt.clone()).await;
        let mut p = holon_api::StorageEntity::new();
        p.insert("id".into(), text("block:page-notes"));
        p.insert("field".into(), text("file_properties"));
        p.insert(
            "value".into(),
            text(&serde_json::json!({ "note+": "v" }).to_string()),
        );
        let result = t
            .engine()
            .execute_operation(&EntityName::new("block"), "set_field", p, OpOrigin::User)
            .await;
        let file = org_file_after_settle(&t).await;
        if let Err(e) = result {
            panic!("the file-drawer key \"note+\" was refused: {e:#}\nnotes.org:\n{file}");
        }
        assert!(
            file.starts_with(":PROPERTIES:\n") && file.contains("\n:note+: v\n"),
            "the file-drawer key \"note+\" did not reach notes.org:\n{file}"
        );
    });
}

/// A key the parser reads back as a typed field, not as a property: an edge
/// in any spelling the drawer lifts, or a storage column.
#[test]
fn a_properties_bag_key_spelling_an_edge_is_refused() {
    refused_case(
        "create",
        "Contributes-To",
        vec![
            ("id", text("block:n-child")),
            ("parent_id", text(TARGET_ID)),
            ("content", text("child")),
            (
                "properties",
                Value::Object([("Contributes-To".to_string(), text("n-target"))].into()),
            ),
        ],
    );
}

#[test]
fn a_properties_bag_key_naming_a_storage_column_is_refused() {
    refused_case(
        "set_field",
        "sort_key",
        vec![
            ("id", text(TARGET_ID)),
            ("field", text("org_properties")),
            (
                "value",
                text(&serde_json::json!({ "ID": "n-target", "sort_key": "a0" }).to_string()),
            ),
        ],
    );
}

#[test]
fn set_field_under_a_drawer_spelling_of_an_edge_is_refused() {
    refused_case(
        "set_field",
        "REQUIRES",
        vec![
            ("id", text(TARGET_ID)),
            ("field", text("REQUIRES")),
            ("value", text("n-target")),
        ],
    );
}

/// Every key the org ingest lifts into a typed field, in the cases a person
/// may type it.
fn typed_field_spellings() -> Vec<String> {
    let mut all: Vec<String> = holon_org_format::TypedDrawerKey::spellings()
        .iter()
        .flat_map(|(spelling, _)| {
            let mut title = spelling.to_ascii_lowercase();
            title[..1].make_ascii_uppercase();
            [
                spelling.clone(),
                spelling.to_ascii_lowercase(),
                spelling.to_ascii_uppercase(),
                title,
            ]
        })
        .collect();
    all.sort();
    all.dedup();
    all
}

/// A write under a key the org ingest lifts into a typed field that is that
/// field's own write: `ID` (checked as an id), `priority`, a flat edge column.
fn is_own_write(key: &str, in_bag: bool) -> bool {
    key == "ID"
        || key == holon_org_format::org_props::PRIORITY
        || (!in_bag && holon_api::EdgeField::is_edge_column(key))
}

#[test]
fn every_spelling_the_ingest_lifts_is_refused_as_a_property() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let t = booted(rt.clone()).await;
        let mut accepted = Vec::new();
        for (i, key) in typed_field_spellings().into_iter().enumerate() {
            let flat_is_property = matches!(
                holon_api::BlockWriteField::parse(&key),
                Ok(holon_api::BlockWriteField::Property(_))
            );
            let bag = (
                "create",
                "properties",
                [
                    ("id", text(&format!("block:n-child-{i}"))),
                    ("parent_id", text(TARGET_ID)),
                    ("content", text("child")),
                    (
                        "properties",
                        Value::Object([(key.clone(), text("n-target"))].into()),
                    ),
                ]
                .to_vec(),
            );
            let flat = (
                "set_field",
                key.as_str(),
                [
                    ("id", text(TARGET_ID)),
                    ("field", text(&key)),
                    ("value", text("n-target")),
                ]
                .to_vec(),
            );
            for (in_bag, (op, via, params)) in [(true, bag), (false, flat)] {
                if is_own_write(&key, in_bag) || (!in_bag && !flat_is_property) {
                    continue;
                }
                let mut p = holon_api::StorageEntity::new();
                for (k, v) in params {
                    p.insert(k.into(), v);
                }
                match t
                    .engine()
                    .execute_operation(&EntityName::new("block"), op, p, OpOrigin::User)
                    .await
                {
                    Ok(_) => accepted.push(format!("{key:?} via {op} `{via}`")),
                    Err(e) => assert!(
                        format!("{e:#}").contains(&format!("{key:?}")),
                        "the refusal of {key:?} via {op} `{via}` must name the key: {e:#}"
                    ),
                }
            }
        }
        let file = org_file_after_settle(&t).await;
        assert!(
            accepted.is_empty(),
            "accepted as a property although the org ingest reads the key as a typed field: \
             {accepted:?}\nnotes.org:\n{file}"
        );
    });
}
