//! A keystroke on the Loro authority writes only the block it names: its
//! children, siblings and parent are unchanged in the Loro tree, also after
//! the org write-back and its re-ingest settle. The typing channel admits a
//! claim on that one block, so a write outside it would run unclaimed.
//!
//! @pbt kind harness
//! @pbt covers admission-footprint(commit_keystroke, loro) — the Loro leg of
//!   the census row in crates/holon/tests/admission_footprint_sweep.rs

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::SnapshotBlock;
use holon_api::SourceKeystroke;
use holon_integration_tests::TestEnvironment;
use holon_loro::DocScope;
use holon_loro::snapshot_blocks_from_doc;

const VAULT_ORG: &str = "\
#+TITLE: Vault
#+ID: page-kf

* Bravo
:PROPERTIES:
:ID: kf-bravo
:END:
** Child
:PROPERTIES:
:ID: kf-child
:END:
* Other
:PROPERTIES:
:ID: kf-other
:END:
";

const TYPED: &str = "block:kf-bravo";
const NEIGHBOURS: &[&str] = &["block:kf-child", "block:kf-other"];

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime"),
    )
}

async fn settle(env: &TestEnvironment) {
    env.wait_for_loro_quiescence(Duration::from_secs(15)).await;
    env.wait_for_cdc_quiescent(Duration::from_millis(250), Duration::from_secs(15))
        .await;
}

async fn loro_blocks(env: &TestEnvironment) -> HashMap<String, SnapshotBlock> {
    let doc = env
        .loro_doc_store()
        .expect("Loro-enabled session")
        .read()
        .await
        .get_doc(DocScope::Global)
        .await
        .expect("global Loro doc");
    doc.with_read(|d| Ok(snapshot_blocks_from_doc(d)))
        .expect("read the global Loro doc")
}

fn changed(
    before: &HashMap<String, SnapshotBlock>,
    after: &HashMap<String, SnapshotBlock>,
) -> BTreeSet<String> {
    before
        .keys()
        .chain(after.keys())
        .filter(|id| before.get(*id) != after.get(*id))
        .cloned()
        .collect()
}

#[test]
fn a_keystroke_on_the_loro_authority_changes_only_its_block() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        assert!(env.loro_enabled(), "the default environment is Loro-backed");
        env.write_org_file("vault.org", VAULT_ORG)
            .await
            .expect("write vault.org");
        env.start_app(true).await.expect("start_app");
        settle(&env).await;
        let seeded = loro_blocks(&env).await;
        for id in std::iter::once(&TYPED).chain(NEIGHBOURS) {
            assert!(seeded.contains_key(*id), "{id} is not in the Loro tree");
        }

        let sources = [
            "TODO bravo typed",
            "bravo typed\nsecond line\n** looks like a child",
            "DONE bravo again",
        ];
        for source in sources {
            let before = loro_blocks(&env).await;
            env.engine()
                .commit_keystroke(SourceKeystroke {
                    id: TYPED.to_string(),
                    source: source.to_string(),
                    write_seq: None,
                })
                .await
                .unwrap_or_else(|e| panic!("commit_keystroke({source:?}): {e:#}"));
            let written = loro_blocks(&env).await;
            settle(&env).await;
            let settled = loro_blocks(&env).await;

            let only_typed = BTreeSet::from([TYPED.to_string()]);
            let at_return = changed(&before, &written);
            let after_settle = changed(&before, &settled);
            println!(
                "[loro keystroke footprint] {source:?}: at return {at_return:?}, \
                 after settle {after_settle:?}, content now {:?}",
                settled[TYPED].block.content
            );
            assert_eq!(
                at_return, only_typed,
                "keystroke {source:?} changed other blocks on the Loro authority"
            );
            assert_eq!(
                after_settle, only_typed,
                "keystroke {source:?}: the write-back and re-ingest changed other blocks"
            );
        }
    });
}
