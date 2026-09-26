//! "Turn into page" out of a document that declares its own `#+TODO:` ring:
//! the new page's file declares the same ring, so the task it takes over is
//! still a task on disk, and one undo takes the whole conversion back.
//!
//! @pbt kind harness
//! @pbt covers convert-block-to-page-inherits-the-ring — a page minted by
//!   convert_block_to_page declares its source document's #+TODO: ring

#![cfg(feature = "pbt")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_filesystem::FileSystem;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("rt"),
    )
}

const RING_ORG: &str = "\
#+TITLE: Errands
#+ID: page-errands
#+TODO: TODO WAITING | DONE

* Origin
:PROPERTIES:
:ID: origin
:END:
** WAITING kid
:PROPERTIES:
:ID: kid
:END:
";

const RING_LINE: &str = "#+TODO: TODO WAITING | DONE";

async fn settle(env: &TestEnvironment) {
    env.wait_for_loro_quiescence(Duration::from_secs(15)).await;
    env.wait_for_cdc_quiescent(Duration::from_millis(400), Duration::from_secs(15))
        .await;
    env.wait_for_org_files_stable(300, Duration::from_secs(15))
        .await;
}

/// Every org file as `(path, content)`.
async fn files(env: &TestEnvironment) -> Vec<(String, String)> {
    let scan = FileSystem::scan_directory(env.org_fs.as_ref(), env.org_root())
        .await
        .expect("scan org dir");
    let mut out = Vec::new();
    for path in &scan.files {
        let content = FileSystem::read_to_string(env.org_fs.as_ref(), path)
            .await
            .expect("read org file");
        out.push((path.display().to_string(), content));
    }
    out
}

fn dump(files: &[(String, String)]) -> String {
    files
        .iter()
        .map(|(path, content)| format!("\n--- {path} ---\n{content}"))
        .collect()
}

#[test]
fn a_converted_page_file_declares_its_source_documents_ring() {
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("Errands.org", RING_ORG)
            .build(rt.clone())
            .await
            .expect("boot");
        settle(&env).await;

        let mut params: HashMap<String, Value> = HashMap::new();
        params.insert("target".into(), Value::String("block:origin".into()));
        env.execute_operation("block", "convert_block_to_page", params)
            .await
            .expect("convert_block_to_page applies");
        settle(&env).await;

        let rows = env
            .query_sql("SELECT parent_id FROM block_raw WHERE id = 'block:kid'")
            .await
            .expect("query kid");
        let page = rows
            .first()
            .and_then(|r| r.get("parent_id"))
            .and_then(|v| v.as_string())
            .expect("block:kid has a parent")
            .to_string();
        let page_marker = format!("#+ID: {}", page.strip_prefix("block:").unwrap_or(&page));
        let after = files(&env).await;
        let page_file = after
            .iter()
            .find(|(_, content)| content.contains(&page_marker))
            .unwrap_or_else(|| panic!("no file holds the new page {page}:{}", dump(&after)));
        assert!(
            page_file.1.contains(RING_LINE) && page_file.1.contains("WAITING kid"),
            "the new page's file declares {RING_LINE:?} and keeps the kid a WAITING task:{}",
            dump(&after)
        );

        let outcome = env.engine().undo().await.expect("undo the convert");
        assert!(outcome.applied(), "undo applies, got {outcome:?}");
        settle(&env).await;
        let undone = files(&env).await;
        assert!(
            !undone
                .iter()
                .any(|(_, content)| content.contains(&page_marker)),
            "undo removes the new page's file:{}",
            dump(&undone)
        );
        let (_, errands) = undone
            .iter()
            .find(|(path, _)| path.ends_with("Errands.org"))
            .unwrap_or_else(|| panic!("Errands.org is gone:{}", dump(&undone)));
        assert!(
            errands.contains("** WAITING kid"),
            "undo puts the kid back under Origin:{}",
            dump(&undone)
        );
    });
}
