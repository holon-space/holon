#![cfg(feature = "pbt")]
//! The Loro and the SqlOnly write authority give the same `owning_page` and
//! `children` answers for the same vault.
//!
//! A cyclic or dangling parent chain cannot be stored in either vault through
//! the op path (the Loro tree refuses both shapes, the SQL foreign key refuses
//! the dangling one), so the `Broken` answers are pinned on the shared walk
//! itself in `holon-core` (`owning_page_by_hops` tests).
//!
//! @pbt kind harness
//! @pbt covers write-authority-owning-page-parity — both write authorities
//!   name the same owning page, children, absence and no-owner answers

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_api::Value;
use holon_core::OwningPage;
use holon_core::WriteAuthorityReads;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_org_format::OrgDocumentExt;

const FIXTURE: &str = "#+TITLE: Parity\n#+ID: par-page\n#+TODO: NEXT | DONE\n\
* Alpha\n:PROPERTIES:\n:ID: par-alpha\n:END:\n\
** Alpha one\n:PROPERTIES:\n:ID: par-alpha-one\n:END:\n\
*** Alpha deep\n:PROPERTIES:\n:ID: par-alpha-deep\n:END:\n\
** Alpha two\n:PROPERTIES:\n:ID: par-alpha-two\n:END:\n\
* Beta\n:PROPERTIES:\n:ID: par-beta\n:END:\n";

const ROOT_BLOCK: &str = "block:par-unowned";
const MISSING: &str = "block:par-missing";

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

async fn boot(rt: Arc<tokio::runtime::Runtime>, loro: bool) -> TestEnvironment {
    let builder =
        TestEnvironmentBuilder::new().with_org_file("Parity.org".to_string(), FIXTURE.to_string());
    let builder = if loro {
        builder
    } else {
        builder.without_loro()
    };
    let env = builder.build(rt).await.expect("boot the vault");
    if loro {
        env.wait_for_loro_quiescence(Duration::from_secs(60)).await;
    }
    let params: HashMap<String, Value> = [
        ("id", Value::String(ROOT_BLOCK.to_string())),
        (
            "parent_id",
            Value::String(EntityUri::no_parent().to_string()),
        ),
        ("content", Value::String("unowned".to_string())),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    env.execute_operation("block", "create", params)
        .await
        .expect("create a block under the root sentinel");
    env
}

fn uri(id: &str) -> EntityUri {
    EntityUri::parse(id).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// Every answer as one comparable line per probe.
async fn observe(authority: &dyn WriteAuthorityReads) -> Vec<String> {
    let mut lines = Vec::new();
    for id in [
        "block:par-page",
        "block:par-alpha",
        "block:par-alpha-deep",
        "block:par-beta",
        ROOT_BLOCK,
        MISSING,
    ] {
        let answer = match authority.owning_page(&uri(id)).await {
            Ok(OwningPage::Page(page)) => format!(
                "Page({}, todo={:?})",
                page.block.id,
                page.block.todo_keywords()
            ),
            Ok(other) => format!("{other:?}"),
            Err(e) => format!("Err({e})"),
        };
        lines.push(format!("owning_page({id}) = {answer}"));
    }
    for id in [
        "block:par-page",
        "block:par-alpha",
        "block:par-alpha-deep",
        MISSING,
    ] {
        let answer = match authority.children(&uri(id)).await {
            Ok(kids) => format!("{:?}", kids.iter().map(|k| k.as_str()).collect::<Vec<_>>()),
            Err(_) => "Err".to_string(),
        };
        lines.push(format!("children({id}) = {answer}"));
    }
    lines
}

fn expected() -> Vec<String> {
    let todo = "Some([TaskState { keyword: \"NEXT\", category: Active }, TaskState { keyword: \
                \"DONE\", category: Done }])";
    vec![
        format!("owning_page(block:par-page) = Page(block:par-page, todo={todo})"),
        format!("owning_page(block:par-alpha) = Page(block:par-page, todo={todo})"),
        format!("owning_page(block:par-alpha-deep) = Page(block:par-page, todo={todo})"),
        format!("owning_page(block:par-beta) = Page(block:par-page, todo={todo})"),
        format!("owning_page({ROOT_BLOCK}) = NoOwner"),
        format!("owning_page({MISSING}) = Absent"),
        "children(block:par-page) = [\"block:par-alpha\", \"block:par-beta\"]".to_string(),
        "children(block:par-alpha) = [\"block:par-alpha-one\", \"block:par-alpha-two\"]"
            .to_string(),
        "children(block:par-alpha-deep) = []".to_string(),
        format!("children({MISSING}) = Err"),
    ]
}

#[test]
fn both_write_authorities_answer_owning_page_and_children_alike() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let loro_env = boot(rt.clone(), true).await;
        let loro = loro_env
            .injector()
            .expect("booted injector")
            .resolve::<dyn WriteAuthorityReads>();
        let loro_lines = observe(loro.as_ref()).await;

        let sql_env = boot(rt, false).await;
        let sql = holon::core::sql_write_authority::SqlWriteAuthority::new(
            sql_env.engine().db_handle().clone(),
        );
        let sql_lines = observe(&sql).await;

        let want = expected();
        let report = |leg: &str, got: &[String]| {
            got.iter()
                .zip(&want)
                .filter(|(g, w)| g != w)
                .map(|(g, w)| format!("{leg}: got `{g}`, want `{w}`"))
                .collect::<Vec<_>>()
        };
        let mut mismatches = report("Loro", &loro_lines);
        mismatches.extend(report("SQL", &sql_lines));
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    });
}
