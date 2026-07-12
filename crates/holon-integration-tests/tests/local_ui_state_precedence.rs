//! Directed test for the C8 LocalStateStore increment:
//!
//! 1. Slot precedence is resolved IN the query — `COALESCE(local override,
//!    synced choice, default)` — so a local `local_ui_state` row beats the
//!    synced `active_perspective` block property until cleared, then the synced
//!    choice takes over again.
//! 2. `local_ui_state` is a LOCAL-ONLY table outside every projection / reseed
//!    path: a reboot over the SAME DB (which runs the cold-boot full reseed of
//!    the block sink) leaves the row intact. (A DB rebuild loses it —
//!    disclosed, C2b ephemeral-cache doctrine.)

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

const VAULT_ORG: &str = "\
* Tasks Perspective
:PROPERTIES:
:ID: tasks-perspective
:perspective_name: Tasks
:END:
** Tasks Main
:PROPERTIES:
:ID: tasks-main-panel
:END:
#+begin_src holon_sql
SELECT * FROM block WHERE parent_id = 'block:journals'
#+end_src
";

async fn root_render_dbg(env: &TestEnvironment) -> String {
    let expr = env.initial_widget().await.expect("root render");
    format!("{expr:?}")
}

/// Poll the root render until `pred` holds (projection latency window).
async fn wait_for_render(env: &TestEnvironment, what: &str, pred: impl Fn(&str) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let dbg = root_render_dbg(env).await;
        if pred(&dbg) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}; last render: {dbg}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[test]
fn local_override_wins_until_cleared_and_survives_reboot_reseed() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.write_org_file("vault.org", VAULT_ORG)
            .await
            .expect("write vault.org");

        env.start_app(true).await.expect("boot-1 start_app");
        assert!(
            env.wait_for_block("tasks-perspective", Duration::from_secs(15))
                .await,
            "tasks-perspective should sync"
        );

        let root_uri = holon_api::root_layout_block_uri();

        // Default: root slot shows the bundled panels.
        wait_for_render(&env, "default layout", |dbg| {
            dbg.contains("block:default-main-panel")
        })
        .await;

        // Local override points the slot at the seeded perspective — no
        // synced property involved.
        env.engine()
            .local_state()
            .set(&root_uri, "active_perspective", "block:tasks-perspective")
            .await
            .expect("local set");
        wait_for_render(&env, "local override to tasks-perspective", |dbg| {
            dbg.contains("block:tasks-main-panel") && !dbg.contains("block:default-main-panel")
        })
        .await;

        // Clearing the local override falls back to the synced choice /
        // default (COALESCE precedence in the slot query).
        env.engine()
            .local_state()
            .clear(&root_uri, "active_perspective")
            .await
            .expect("local clear");
        wait_for_render(&env, "fallback to default after clear", |dbg| {
            dbg.contains("block:default-main-panel")
        })
        .await;

        // Re-set the override, then reboot over the SAME DB: the cold-boot
        // full reseed reconciles the block sink tables — it must NOT touch
        // local_ui_state.
        env.engine()
            .local_state()
            .set(&root_uri, "active_perspective", "block:tasks-perspective")
            .await
            .expect("local set before reboot");

        env.stop_app().await.expect("stop_app");
        env.start_app(true).await.expect("boot-2 start_app");
        assert!(
            env.wait_for_block("tasks-perspective", Duration::from_secs(15))
                .await,
            "tasks-perspective should re-sync on boot 2"
        );

        let survived = env
            .engine()
            .local_state()
            .get(&root_uri, "active_perspective")
            .await
            .expect("local get after reboot");
        assert_eq!(
            survived.as_deref(),
            Some("block:tasks-perspective"),
            "local_ui_state must survive a reboot + cold-boot reseed over the same DB"
        );

        // And the surviving override still drives the slot.
        wait_for_render(&env, "local override after reboot", |dbg| {
            dbg.contains("block:tasks-main-panel")
        })
        .await;
    });
}
