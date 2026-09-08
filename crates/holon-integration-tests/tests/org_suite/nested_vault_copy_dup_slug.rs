//! A vault that contains stale COPIES of itself must ingest only the live
//! files.
//!
//! The live vault keeps ~12 agent jj-workspaces under
//! `.claude/worktrees/agent-*/`, each a full copy carrying
//! `Projects/Holon/Now.org` with the SAME `#+ID:` and headline `:ID:` slugs as
//! the live file. Two legs decide what the vault contains and they disagreed:
//! the real boot walk drops hidden entries (`ignore`'s `hidden(true)`) while
//! the watcher's own filter dropped only `.git`/`.jj`, so a copy was invisible
//! at boot and ingested the moment it changed — and the harness's in-memory
//! scan filtered nothing at all, which is why no test could see it. Write-back
//! then rewrote the live file with blocks resurrected from the copy
//! (`now-query` lane: 265 lines written against a live 187).
//!
//! Both legs are exercised here: a copy present BEFORE boot (the scan) and a
//! copy written WHILE the app runs (the watcher).
//!
//! @pbt kind harness
//! @pbt covers vault-walk-parity — a stale vault copy under a dot-directory
//! must reach neither the store nor the live file's bytes (bugfunnel
//! 2026-09-09-hidden-dir-vault-copy-invisible-at-boot-ingested-by-the-watcher)
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone's generator
//! never materialises a file under a hidden directory

use std::sync::Arc;
use std::time::Duration;

use holon_integration_tests::TestEnvironment;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

/// The live file: two tasks, both slugged.
const LIVE_ORG: &str = "\
#+ID: 68809135-c2da-4e9b-ad02-d78336895688
* TODO Ship the walk parity fix
:PROPERTIES:
:ID: dupslug-live-open
:END:
* TODO Record the escape
:PROPERTIES:
:ID: dupslug-live-second
:END:
";

/// A stale copy: the same first slug as the live file, plus a DONE headline
/// the live file retired. Resurrecting `dupslug-copy-done` is the observable
/// defect.
///
/// Its `#+ID:` is DELIBERATELY distinct. A copy sharing the live document id
/// meets the duplicate-`#+ID:` refusal, whose claimant is whichever file the
/// scan happened to reach first — so the copy would be excluded by luck half
/// the time and this test would say nothing on the other half. A distinct
/// document id isolates the question this test asks: does a file under a
/// hidden directory get ingested at all?
const STALE_COPY_ORG: &str = "\
#+ID: 4f1a77e2-5b30-4c19-9d84-2ac6b0e51137
* TODO Ship the walk parity fix
:PROPERTIES:
:ID: dupslug-live-open
:END:
* DONE Retired long ago
:PROPERTIES:
:ID: dupslug-copy-done
:END:
";

const BOOT_COPY: &str = ".claude/worktrees/agent-boot/Projects/Holon/Live.org";
const LIVE_COPY: &str = ".claude/worktrees/agent-live/Projects/Holon/Live.org";

/// Write straight to the in-memory vault WITHOUT registering a tracked
/// document — a stale copy is a file nobody told Holon about, which is the
/// whole point.
async fn write_untracked(env: &TestEnvironment, rel: &str, content: &str) -> u64 {
    let path = env.org_root().join(rel);
    env.org_fs.mkdir_all(path.parent().expect("a parent dir"));
    holon_filesystem::FileSystem::write(env.org_fs.as_ref(), &path, content.as_bytes())
        .await
        .expect("write the stale copy");
    env.org_fs.last_change_seq()
}

#[test]
fn a_stale_vault_copy_under_a_dot_dir_reaches_neither_the_store_nor_the_live_file() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.set_enable_loro(false);

        env.write_org_file("Live.org", LIVE_ORG)
            .await
            .expect("write the live file");
        write_untracked(&env, BOOT_COPY, STALE_COPY_ORG).await;

        env.start_app(true).await.expect("start_app");

        // The watcher leg: a copy touched while the app runs.
        let seq = write_untracked(&env, LIVE_COPY, STALE_COPY_ORG).await;
        env.wait_for_org_change_processed(seq, Duration::from_secs(20))
            .await;

        // Rung 1 — the copy's own block never enters the store.
        let resurrected = env
            .query_sql("SELECT id FROM block_raw WHERE id = 'block:dupslug-copy-done'")
            .await
            .expect("query block_raw");
        assert!(
            resurrected.is_empty(),
            "a block declared ONLY by a stale vault copy under a dot-directory reached the \
             store; the copy's document merges into the live one and write-back resurrects \
             its retired headlines"
        );

        // Rung 2 — the live file's bytes never gain the copy's headline.
        let live = holon_filesystem::FileSystem::read_to_string(
            env.org_fs.as_ref(),
            &env.org_root().join("Live.org"),
        )
        .await
        .expect("read the live file back");
        assert!(
            !live.contains("dupslug-copy-done"),
            "write-back rewrote the live file with a headline only the stale copy \
             declares:\n{live}"
        );
        assert!(
            live.contains("dupslug-live-second"),
            "the live file lost its own second task:\n{live}"
        );

        env.stop_app().await.expect("stop_app");
    });
}
