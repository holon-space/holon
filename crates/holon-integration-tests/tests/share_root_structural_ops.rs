#![cfg(feature = "pbt")]
//! Structural ops on a placed page-share root decide from the parent its mount
//! gives it.
//!
//! A shared page has no parent in its own doc: where it hangs on this device is
//! its mount in the global tree. SQL and `get_block` answer the mount's parent,
//! and the write authority's decision read must answer the same, or a move
//! records the root as the page's old parent and indent refuses the page as a
//! root block.
//!
//! @pbt kind harness
//! @pbt covers share-root-structural-ops — move + undo, indent and outdent of a
//!   placed page-share root decide on the mount's parent

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_api::Value;
use holon_core::WriteAuthorityReads;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;

const HOST: &str = "#+TITLE: Host\n#+ID: sr-host\n\
* Note\n:PROPERTIES:\n:ID: sr-note\n:END:\n";
const SUB: &str = "#+TITLE: Sub\n#+ID: sr-sub\n\
* Sub note\n:PROPERTIES:\n:ID: sr-sub-note\n:END:\n";
const OTHER: &str = "#+TITLE: Other\n#+ID: sr-other\n\
* Other note\n:PROPERTIES:\n:ID: sr-other-note\n:END:\n";
const SHARED: &str = "#+TITLE: Shared\n#+ID: sr-page\n\
* Delta\n:PROPERTIES:\n:ID: sr-delta\n:END:\n";

const PAGE: &str = "block:sr-page";
const HOST_ID: &str = "block:sr-host";
const SUB_ID: &str = "block:sr-sub";

fn uri(id: &str) -> EntityUri {
    EntityUri::parse(id).unwrap_or_else(|e| panic!("{id} is not a uri: {e}"))
}

async fn settle(env: &TestEnvironment) {
    env.wait_for_loro_quiescence(Duration::from_secs(60)).await;
    env.wait_for_cdc_quiescent(Duration::from_millis(300), Duration::from_secs(60))
        .await;
}

async fn op(env: &TestEnvironment, entity: &str, name: &str, params: &[(&str, &str)]) {
    try_op(env, entity, name, params)
        .await
        .unwrap_or_else(|e| panic!("{entity}.{name} {params:?}: {e:#}"));
    settle(env).await;
}

async fn try_op(
    env: &TestEnvironment,
    entity: &str,
    name: &str,
    params: &[(&str, &str)],
) -> anyhow::Result<()> {
    let params: HashMap<String, Value> = params
        .iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
        .collect();
    env.execute_operation(entity, name, params).await
}

async fn sql_parent(env: &TestEnvironment, id: &str) -> String {
    let rows = env
        .query_sql(&format!("SELECT parent_id FROM block WHERE id = '{id}'"))
        .await
        .unwrap_or_else(|e| panic!("parent of {id}: {e:#}"));
    assert_eq!(rows.len(), 1, "exactly one row for {id}: {rows:?}");
    rows[0]
        .get("parent_id")
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| panic!("{id} has no parent_id: {rows:?}"))
        .to_string()
}

async fn authority_parent(authority: &dyn WriteAuthorityReads, id: &str) -> String {
    authority
        .block(&uri(id))
        .await
        .unwrap_or_else(|e| panic!("authority block({id}): {e:#}"))
        .unwrap_or_else(|| panic!("the write authority holds no {id}"))
        .block
        .parent_id
        .to_string()
}

#[test]
fn structural_ops_on_a_placed_page_share_root_decide_on_its_mounts_parent() {
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    );
    rt.clone().block_on(async move {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("Host.org".to_string(), HOST.to_string())
            .with_org_file("Sub.org".to_string(), SUB.to_string())
            .with_org_file("Other.org".to_string(), OTHER.to_string())
            .with_org_file("Shared.org".to_string(), SHARED.to_string())
            .build(rt.clone())
            .await
            .expect("boot the vault");
        settle(&env).await;
        let authority = env
            .injector()
            .expect("booted injector")
            .resolve::<dyn WriteAuthorityReads>();
        let authority = authority.as_ref();

        // Host holds [note, sub, P]: the page above P is a page, so P can indent.
        let sub_move = [
            ("id", SUB_ID),
            ("parent_id", HOST_ID),
            ("after_block_id", "block:sr-note"),
        ];
        op(&env, "block", "move_block", &sub_move).await;
        op(
            &env,
            "tree",
            "share_subtree",
            &[("id", PAGE), ("retention", "none")],
        )
        .await;
        let place = [
            ("id", PAGE),
            ("parent_id", HOST_ID),
            ("after_block_id", SUB_ID),
        ];
        op(&env, "block", "move_block", &place).await;
        assert_eq!(
            sql_parent(&env, PAGE).await,
            HOST_ID,
            "setup: P placed under host"
        );

        let mut mismatches = Vec::new();
        let mut expect = |what: &str, got: String, want: &str| {
            if got != want {
                mismatches.push(format!("{what}: got `{got}`, want `{want}`"));
            }
        };

        expect(
            "authority parent of the placed P",
            authority_parent(authority, PAGE).await,
            HOST_ID,
        );

        op(
            &env,
            "block",
            "move_block",
            &[("id", PAGE), ("parent_id", "block:sr-other")],
        )
        .await;
        if let Err(e) = env.engine().undo().await {
            expect("undo of the move of P", format!("Err({e:#})"), "Ok");
            op(&env, "block", "move_block", &place).await;
        }
        settle(&env).await;
        expect(
            "SQL parent of P after move + undo",
            sql_parent(&env, PAGE).await,
            HOST_ID,
        );

        match try_op(&env, "block", "indent", &[("id", PAGE)]).await {
            Ok(()) => {
                settle(&env).await;
                expect(
                    "SQL parent of P after indent",
                    sql_parent(&env, PAGE).await,
                    SUB_ID,
                );
            }
            Err(e) => expect("indent of P", format!("Err({e:#})"), "Ok"),
        }

        // P's parent is a page whichever indent left it under, so outdent is
        // the page-boundary refusal, never the root-block one.
        let outdent = try_op(&env, "block", "outdent", &[("id", PAGE)]).await;
        let outdent = match outdent {
            Ok(()) => "Ok".to_string(),
            Err(e) if format!("{e:#}").contains("ADR 0028 D1") => "page-boundary refusal".into(),
            Err(e) => format!("Err({e:#})"),
        };
        expect("outdent of P", outdent, "page-boundary refusal");

        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    });
}
