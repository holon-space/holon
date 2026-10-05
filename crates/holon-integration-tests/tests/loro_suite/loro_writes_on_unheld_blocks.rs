//! In a Loro vault, Loro is the write authority for every block (D69.a): a
//! write on a block only the SQL projection holds is refused by name and
//! writes nothing. The minted `::src::` / `::render::` rows are ordinary
//! blocks of that authority, not SQL-only rows.
//!
//! @pbt kind harness
//! @pbt covers loro-unheld-block-writes — every write on a projection-only
//! block in a Loro vault is refused with `BlockNotInWriteAuthority`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_integration_tests::TestEnvironment;
use holon_loro::DocScope;
use holon_loro::LoroBackend;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap(),
    )
}

async fn started_vault(runtime: Arc<tokio::runtime::Runtime>, org: &str) -> TestEnvironment {
    let env = TestEnvironment::new(runtime).expect("TestEnvironment::new");
    assert!(env.loro_enabled(), "this slice needs the Loro wiring");
    env.write_org_file("vault.org", org)
        .await
        .expect("write vault.org");
    env.start_app(true).await.expect("start_app");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let rows = env
            .query_sql("SELECT id FROM block_raw WHERE content = 'second block'")
            .await
            .expect("query block_raw");
        if !rows.is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "org scan never projected vault.org into SQL"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    env.wait_for_loro_quiescence(Duration::from_secs(10)).await;
    env
}

/// The authority across both trees a block can live in.
async fn authority(env: &TestEnvironment) -> LoroBackend {
    let store = env
        .loro_doc_store()
        .expect("loro_doc_store present in Loro wiring")
        .clone();
    let store = store.read().await;
    let global = store.get_doc(DocScope::Global).await.expect("global doc");
    let layout = store.get_doc(DocScope::Layout).await.expect("layout doc");
    LoroBackend::from_document(global).with_layout_doc(layout)
}

#[test]
fn minted_src_and_render_rows_are_held_by_the_write_authority() {
    let rt = runtime();
    rt.clone().block_on(minted_rows(rt));
}

async fn minted_rows(runtime: Arc<tokio::runtime::Runtime>) {
    let env = started_vault(
        runtime,
        "* vault\n:PROPERTIES:\n:ID: d69-vault\n:END:\n#+BEGIN_SRC holon_prql\nfrom \
         children\n#+END_SRC\n- first block\n- second block\n",
    )
    .await;
    let backend = authority(&env).await;
    let rows = env
        .query_sql("SELECT id FROM block_raw WHERE id LIKE '%::src::%' OR id LIKE '%::render::%'")
        .await
        .expect("query minted rows");
    let ids: Vec<String> = rows
        .iter()
        .map(|r| {
            r.get("id")
                .and_then(|v| v.as_string())
                .expect("id column")
                .to_string()
        })
        .collect();
    assert!(
        ids.iter().any(|id| id == "block:d69-vault::src::0"),
        "the vault file's source block must be projected: {ids:?}"
    );
    assert!(
        ids.iter().any(|id| id.contains("::render::")),
        "the seeded layout must project a ::render:: row: {ids:?}"
    );
    let mut unheld = Vec::new();
    for id in &ids {
        let uri = holon_api::EntityUri::parse(id).expect("minted id parses");
        if !backend.is_live_anywhere(uri.id()).await {
            unheld.push(id);
        }
    }
    assert!(
        unheld.is_empty(),
        "minted rows the write authority does not hold: {unheld:?} (of {ids:?})"
    );
}

#[test]
fn writes_on_a_block_only_the_projection_holds_are_refused() {
    let rt = runtime();
    rt.clone().block_on(unheld_writes(rt));
}

async fn unheld_writes(runtime: Arc<tokio::runtime::Runtime>) {
    let env = started_vault(runtime, "* vault\n- first block\n- second block\n").await;
    let doc_root = env
        .resolve_page_uri_by_name("vault.org")
        .await
        .expect("resolve vault.org root");
    let stranded = "block:d69a0000-0000-0000-0000-000000000001";
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(stranded.to_string()));
    params.insert("parent".to_string(), Value::String(doc_root.to_string()));
    env.engine()
        .db_handle()
        .query(
            "INSERT INTO block_raw (id, parent_id, sort_key, content) VALUES ($id, $parent, \
             'Zz', 'stranded content')",
            params,
        )
        .await
        .expect("insert stranded SQL-only block");
    let backend = authority(&env).await;
    assert!(
        !backend.is_live_anywhere(stranded).await,
        "precondition: the stranded block has no Loro node"
    );
    let refusal = holon_core::BlockNotInWriteAuthority {
        block: holon_api::EntityUri::parse(stranded).expect("stranded uri"),
    }
    .to_string();

    let id = || Value::String(stranded.to_string());
    let ops: Vec<(&str, HashMap<String, Value>)> = vec![
        (
            "set_field",
            HashMap::from([
                ("id".to_string(), id()),
                ("field".to_string(), Value::String("content".to_string())),
                ("value".to_string(), Value::String("rewritten".to_string())),
            ]),
        ),
        (
            "move_block",
            HashMap::from([
                ("id".to_string(), id()),
                ("parent_id".to_string(), Value::String(doc_root.to_string())),
            ]),
        ),
        (
            "join_block",
            HashMap::from([
                ("id".to_string(), id()),
                ("position".to_string(), Value::Integer(0)),
            ]),
        ),
        (
            "delete_keep_children",
            HashMap::from([("id".to_string(), id())]),
        ),
        ("delete_subtree", HashMap::from([("id".to_string(), id())])),
        ("delete", HashMap::from([("id".to_string(), id())])),
    ];
    let mut accepted = Vec::new();
    for (op, params) in ops {
        match env.execute_operation("block", op, params).await {
            Ok(_) => accepted.push(format!("{op}: accepted")),
            Err(e) if format!("{e:#}").contains(&refusal) => {}
            Err(e) => accepted.push(format!("{op}: failed without the refusal: {e:#}")),
        }
    }
    let row = env
        .query_sql(&format!(
            "SELECT parent_id, sort_key, content FROM block_raw WHERE id = '{stranded}'"
        ))
        .await
        .expect("query stranded row");
    assert!(
        accepted.is_empty(),
        "every write on {stranded} must be refused by name ({refusal}): {accepted:#?}; row now \
         {row:?}"
    );
    assert_eq!(row.len(), 1, "a refused delete leaves the row: {row:?}");
    let row = &row[0];
    assert_eq!(
        row.get("content").and_then(|v| v.as_string()),
        Some("stranded content")
    );
    assert_eq!(row.get("sort_key").and_then(|v| v.as_string()), Some("Zz"));
    assert_eq!(
        row.get("parent_id").and_then(|v| v.as_string()),
        Some(doc_root.as_str())
    );
    assert!(
        !backend.is_live_anywhere(stranded).await,
        "a refused write mints no Loro node"
    );
}
