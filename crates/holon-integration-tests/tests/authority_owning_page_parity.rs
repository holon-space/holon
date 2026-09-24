#![cfg(feature = "pbt")]
//! The Loro and the SqlOnly write authority give the same `owning_page` and
//! `children` answers for the same vault.
//!
//! A cyclic or dangling parent chain cannot be stored in either vault through
//! the op path (the Loro tree refuses both shapes, the SQL foreign key refuses
//! the dangling one), so the `Broken` answers are pinned on the shared walk
//! itself in `holon-core` (`owning_page_by_hops` tests).
//!
//! Around a share both authorities follow D197.a: `owning_page` is the page
//! whose org file stores the block. `children` at the mount keep each
//! authority's own shape (Model.md invariant 11), asserted per authority.
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
/// A block of the bundled layout doc, not of the global tree.
const LAYOUT_BLOCK: &str = "block:root-layout";

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
        LAYOUT_BLOCK,
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
    // The global and the layout doc each order their own roots; the two
    // sequences have no common order, so the root answer compares as a set.
    let mut roots: Vec<String> = authority
        .children(&EntityUri::no_parent())
        .await
        .unwrap_or_else(|e| panic!("children of the root sentinel: {e}"))
        .iter()
        .map(|k| k.to_string())
        .collect();
    roots.sort();
    lines.push(format!("children(root) = {roots:?}"));
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
        format!("owning_page({LAYOUT_BLOCK}) = Page(block:__default__, todo=None)"),
        format!("owning_page({MISSING}) = Absent"),
        "children(block:par-page) = [\"block:par-alpha\", \"block:par-beta\"]".to_string(),
        "children(block:par-alpha) = [\"block:par-alpha-one\", \"block:par-alpha-two\"]"
            .to_string(),
        "children(block:par-alpha-deep) = []".to_string(),
        format!("children({MISSING}) = Err"),
        format!(
            "children(root) = {:?}",
            [
                "block:__default__",
                "block:journals",
                "block:par-page",
                ROOT_BLOCK
            ]
        ),
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

const SHARE_HOST: &str = "#+TITLE: Host\n#+ID: shr-host\n\
* Gamma\n:PROPERTIES:\n:ID: shr-gamma\n:END:\n\
** Gamma one\n:PROPERTIES:\n:ID: shr-gamma-one\n:END:\n";
const SHARED_PAGE: &str = "#+TITLE: Shared\n#+ID: shr-page\n#+TODO: WAIT | GONE\n\
* Delta\n:PROPERTIES:\n:ID: shr-delta\n:END:\n\
** Delta one\n:PROPERTIES:\n:ID: shr-delta-one\n:END:\n";

/// Shares `id` and returns the mount block the share minted.
async fn share(env: &TestEnvironment, id: &str) -> EntityUri {
    let params: holon_api::StorageEntity = [("id", id), ("retention", "none")]
        .into_iter()
        .map(|(k, v)| (k.into(), Value::String(v.to_string())))
        .collect();
    let response = env
        .engine()
        .execute_operation(
            &"tree".to_string().into(),
            "share_subtree",
            params,
            holon_api::OpOrigin::User,
        )
        .await
        .unwrap_or_else(|e| panic!("share {id}: {e:#}"))
        .response
        .and_then(|v| v.as_string().map(str::to_string))
        .unwrap_or_else(|| panic!("share {id} answered no response"));
    let json: serde_json::Value = serde_json::from_str(&response)
        .unwrap_or_else(|e| panic!("share {id} response `{response}`: {e}"));
    let mount = json["mount_block_id"]
        .as_str()
        .unwrap_or_else(|| panic!("share {id} response names no mount_block_id: {response}"));
    uri(mount)
}

async fn owning_page_id(authority: &dyn WriteAuthorityReads, id: &str) -> String {
    match authority.owning_page(&uri(id)).await {
        Ok(OwningPage::Page(page)) => page.block.id.to_string(),
        Ok(other) => format!("{other:?}"),
        Err(e) => panic!("owning_page({id}): {e}"),
    }
}

async fn child_ids(authority: &dyn WriteAuthorityReads, id: &str) -> Vec<String> {
    authority
        .children(&uri(id))
        .await
        .unwrap_or_else(|e| panic!("children({id}): {e}"))
        .iter()
        .map(|k| k.to_string())
        .collect()
}

#[test]
fn share_answers_follow_the_file_model() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("Host.org".to_string(), SHARE_HOST.to_string())
            .with_org_file("Shared.org".to_string(), SHARED_PAGE.to_string())
            .build(rt.clone())
            .await
            .expect("boot the vault");
        env.wait_for_loro_quiescence(Duration::from_secs(60)).await;
        let block_mount = share(&env, "block:shr-gamma").await;
        let page_mount = share(&env, "block:shr-page").await;
        env.wait_for_loro_quiescence(Duration::from_secs(60)).await;
        env.wait_for_cdc_quiescent(Duration::from_millis(300), Duration::from_secs(60))
            .await;
        let loro = env
            .injector()
            .expect("booted injector")
            .resolve::<dyn WriteAuthorityReads>();
        let loro = loro.as_ref();
        let sql = holon::core::sql_write_authority::SqlWriteAuthority::new(
            env.engine().db_handle().clone(),
        );
        let m = block_mount.to_string();

        // Block share: the mount page M stores the subtree in both authorities.
        for id in ["block:shr-gamma", "block:shr-gamma-one", m.as_str()] {
            assert_eq!(owning_page_id(loro, id).await, m, "Loro owning_page({id})");
            assert_eq!(owning_page_id(&sql, id).await, m, "SQL owning_page({id})");
        }
        // Loro's mount stands for the shared root; SQL keeps the mount as a row.
        assert_eq!(child_ids(loro, "block:shr-host").await, ["block:shr-gamma"]);
        assert_eq!(child_ids(loro, &m).await, ["block:shr-gamma-one"]);
        assert_eq!(child_ids(&sql, "block:shr-host").await, [m.as_str()]);
        assert_eq!(child_ids(&sql, &m).await, ["block:shr-gamma"]);
        for authority in [loro, &sql as &dyn WriteAuthorityReads] {
            assert_eq!(
                child_ids(authority, "block:shr-gamma").await,
                ["block:shr-gamma-one"]
            );
        }

        // Page share: the shared page P stores the subtree.
        let p = "block:shr-page";
        let n = page_mount.to_string();
        for id in [p, "block:shr-delta", "block:shr-delta-one", n.as_str()] {
            assert_eq!(owning_page_id(loro, id).await, p, "Loro owning_page({id})");
        }
        let OwningPage::Page(shared_page) = loro
            .owning_page(&uri("block:shr-delta-one"))
            .await
            .expect("Loro owning_page(block:shr-delta-one)")
        else {
            panic!("Loro names no page for block:shr-delta-one");
        };
        assert_eq!(
            shared_page.block.todo_keywords(),
            Some(vec![
                holon_api::TaskState::active("WAIT"),
                holon_api::TaskState::done("GONE"),
            ]),
            "the shared page carries its own task vocabulary"
        );
        assert_eq!(child_ids(loro, &n).await, ["block:shr-delta"]);
        for authority in [loro, &sql as &dyn WriteAuthorityReads] {
            assert_eq!(child_ids(authority, p).await, ["block:shr-delta"]);
        }
        for id in [p, "block:shr-delta", "block:shr-delta-one"] {
            assert_eq!(owning_page_id(&sql, id).await, p, "SQL owning_page({id})");
        }
    });
}
