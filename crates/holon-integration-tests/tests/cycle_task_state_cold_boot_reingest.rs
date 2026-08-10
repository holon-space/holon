//! Two cold-boot legs over one harness: whatever the store decided about a
//! block's task-ness must survive a re-ingest that has ONLY the rendered bytes
//! to go on.
//!
//! F3 — cycling a task in a document that declares its own `#+TODO:`
//! vocabulary.
//!
//! The bug is not the wrong keyword — it is the SILENT DELETION that follows.
//! `cycle_task_state` writing `TODO` into a `#+TODO: NEXT WAITING | DONE`
//! document renders `* TODO delta external`; the org parser of that document
//! does not know `TODO`, so the next full parse reads the headline as PLAIN
//! BODY TEXT with content `"TODO delta external"` and no `task_state`. The task
//! is gone, and nothing anywhere reported an error.
//!
//! WHY A COLD BOOT AND NOT AN EDIT. An in-place re-ingest only re-parses files
//! whose bytes CHANGED, and the changed-file path reconciles against the store
//! it already has — so it can preserve a `task_state` the parser alone would
//! not derive, and the test FALSE-PASSES. The second boot here runs against a
//! FRESH DATABASE with nothing but the rendered bytes, which is the only
//! configuration where the parser is the sole authority.
//!
//! F2 — promotion followed by UNDO. Under the convergence ruling (2026-08-10)
//! the undo is semantically void: the content inverse restores the verbatim
//! typed text, which IS keyword-headed, so the store converges it straight back
//! and the block is still the task. What this leg proves is that the ruled
//! state is a FIXED POINT of the whole loop — store, render, cold re-ingest —
//! which is the dogfood F2 shape exactly: `defvocab-b4` came back re-promoted
//! and the keyword-only block came back with its word erased, because the two
//! layers disagreed. They cannot disagree now, and this asserts it rather than
//! reasoning about it.
//!
//! @pbt kind harness
//! @pbt covers task-state-cycle-vocabulary — a cycled keyword survives a
//!   fresh-DB re-ingest of the document that declares it
//! @pbt covers promotion-undo-cold-boot — the converged post-undo state is a
//!   fixed point of render + fresh-DB re-ingest, in both the non-empty and the
//!   empty-remainder arm

use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
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

const VOCAB_ORG: &str = "\
#+TITLE: Errands
#+ID: page-errands
#+TODO: NEXT WAITING | DONE

* delta external
:PROPERTIES:
:ID: blk-delta
:END:
";

const BLOCK_ID: &str = "block:blk-delta";

/// The F2 leg's document. NO `#+TODO:` line, so the DEFAULT vocabulary governs
/// and `TODO` is a keyword — the dogfood's `DefaultVocab` page.
const PLAIN_ORG: &str = "\
#+TITLE: Inbox
#+ID: page-inbox

* alpha four
:PROPERTIES:
:ID: blk-alpha
:END:
* beta
:PROPERTIES:
:ID: blk-beta
:END:
";

const ALPHA_ID: &str = "block:blk-alpha";
const BETA_ID: &str = "block:blk-beta";

/// The `(file name, bytes)` of the org file that carries the page — found by
/// its `#+ID:` rather than by name, because the write-back is free to rename
/// the file after the page's title.
async fn rendered_page(env: &TestEnvironment, page_marker: &str) -> (String, String) {
    use holon_filesystem::FileSystem;

    let mut hits: Vec<(String, String)> = Vec::new();
    let mut targets = env.org_fs.write_targets();
    targets.sort();
    targets.dedup();
    for path in targets {
        if path.extension().and_then(|e| e.to_str()) != Some("org") {
            continue;
        }
        let Ok(body) = env.org_fs.read_to_string(&path).await else {
            continue;
        };
        if !body.contains(page_marker) {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("utf-8 file name")
            .to_string();
        hits.push((name, body));
    }
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one org file rooting {page_marker}, found {:?}",
        hits.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    hits.remove(0)
}

async fn wait_for_seed(env: &TestEnvironment, block_id: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    loop {
        let rows = env
            .query_sql(&format!("SELECT id FROM block_raw WHERE id = '{block_id}'"))
            .await
            .expect("query block_raw");
        if !rows.is_empty() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "org scan never populated {block_id}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn settle(env: &TestEnvironment) {
    env.wait_for_loro_quiescence(Duration::from_secs(15)).await;
    env.wait_for_cdc_quiescent(Duration::from_millis(250), Duration::from_secs(15))
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// `(content, task_state)` of the block, straight out of `block_raw`.
async fn block_row(env: &TestEnvironment, block_id: &str) -> (String, Option<String>) {
    let rows = env
        .query_sql(&format!(
            "SELECT content, json_extract(properties, '$.task_state') AS ts FROM block_raw WHERE \
             id = '{block_id}'"
        ))
        .await
        .expect("query block_raw");
    let row = rows
        .first()
        .unwrap_or_else(|| panic!("{block_id} missing from block_raw"));
    let content = row
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or_default()
        .to_string();
    let task_state = row
        .get("ts")
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    (content, task_state)
}

#[test]
fn a_cycled_keyword_survives_a_fresh_db_reingest_of_its_own_document() {
    let rt = runtime();
    rt.clone().block_on(async move {
        // ── Boot 1: seed the vault, cycle the child once ────────────────
        let mut env = TestEnvironment::new(rt.clone()).expect("TestEnvironment::new");
        env.write_org_file("errands.org", VOCAB_ORG)
            .await
            .expect("write errands.org");
        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_seed(&env, BLOCK_ID).await;
        settle(&env).await;

        // Nothing seeds the vocabulary: the `#+TODO:` header on disk is its
        // only source, exactly as in the field. The premise assertion below
        // (`NEXT`, not `TODO`) is what proves the ingest delivered it.
        let mut params = holon_api::StorageEntity::new();
        params.insert("id".into(), Value::String(BLOCK_ID.into()));
        env.engine()
            .execute_operation(
                &EntityName::new("block"),
                "cycle_task_state",
                params,
                OpOrigin::User,
            )
            .await
            .expect("cycle_task_state");
        settle(&env).await;

        let (content_1, state_1) = block_row(&env, BLOCK_ID).await;
        assert_eq!(
            state_1.as_deref(),
            Some("NEXT"),
            "boot-1 premise: the cycle must land on a keyword THIS document \
             declares (content = {content_1:?})"
        );

        // The rendered projection is the only thing that crosses to boot 2.
        let (rendered_name, rendered) = rendered_page(&env, "page-errands").await;
        eprintln!("[cold-boot] rendered {rendered_name}:\n{rendered}");
        assert!(
            rendered.contains("#+TODO:"),
            "the rendered file lost its vocabulary declaration, so boot 2 would judge the \
             keyword against the defaults and this test would prove nothing:\n{rendered}"
        );
        env.stop_app().await.expect("stop_app after boot-1");

        // ── Boot 2: a FRESH database over those bytes ───────────────────
        let mut cold = TestEnvironment::new(rt).expect("cold TestEnvironment::new");
        cold.write_org_file(&rendered_name, &rendered)
            .await
            .expect("seed the cold vault");
        cold.start_app(true).await.expect("boot-2 start_app");
        wait_for_seed(&cold, BLOCK_ID).await;
        settle(&cold).await;

        let (content_2, state_2) = block_row(&cold, BLOCK_ID).await;
        cold.stop_app().await.expect("stop_app after boot-2");

        assert_eq!(
            state_2.as_deref(),
            Some("NEXT"),
            "the cycled task VANISHED across a fresh-DB re-ingest: the keyword written by \
             `cycle_task_state` is not one this document declares, so the parser read the \
             headline back as body text (content = {content_2:?})"
        );
        assert_eq!(
            content_2, "delta external",
            "the keyword must not be sitting inside the block's text"
        );
    });
}

/// Commit a block's editable surface the way the editor does: the FULL raw
/// text the author sees, keyword included, through the SOURCE channel — where
/// the store's parse decides what those bytes mean.
async fn promote(env: &TestEnvironment, block_id: &str, typed: &str) {
    let mut params = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(block_id.into()));
    params.insert(
        "field".into(),
        Value::String(holon_api::SOURCE_TEXT_FIELD.into()),
    );
    params.insert("value".into(), Value::String(typed.into()));
    env.engine()
        .execute_operation(
            &EntityName::new("block"),
            "set_field",
            params,
            OpOrigin::User,
        )
        .await
        .unwrap_or_else(|e| panic!("source commit on {block_id}: {e:#}"));
}

/// F2, the whole loop. The dogfood drove this by hand and got a DIVERGENCE both
/// times; here the pre-boot and post-boot readings must be identical, field for
/// field, in both arms:
///
/// * non-empty remainder (`blk-alpha`, the dogfood's `defvocab-b4`) — was
///   `content="TODO alpha four"` / no task in the app and came back
///   `content="alpha four"` / `task_state=TODO` after a cold boot;
/// * empty remainder (`blk-beta`, the dogfood's keyword-only block) — was
///   `content="TODO"` / no task and came back `content=""` / `task_state=TODO`,
///   the typed word gone.
///
/// Both readings the cold boot produced were RIGHT about the bytes; the store's
/// were wrong. So the fix is not to make the boot agree with the app — it is to
/// make the app hold what the bytes say from the first write, which is what
/// makes both arms fixed points here.
#[test]
fn an_undone_promotion_is_a_fixed_point_of_a_fresh_db_reingest() {
    let rt = runtime();
    rt.clone().block_on(async move {
        // ── Boot 1: seed, promote, undo ─────────────────────────────────
        let mut env = TestEnvironment::new(rt.clone()).expect("TestEnvironment::new");
        env.write_org_file("inbox.org", PLAIN_ORG)
            .await
            .expect("write inbox.org");
        env.start_app(true).await.expect("boot-1 start_app");
        wait_for_seed(&env, ALPHA_ID).await;
        wait_for_seed(&env, BETA_ID).await;
        settle(&env).await;

        // Non-empty remainder: the author typed the keyword at the head.
        promote(&env, ALPHA_ID, "TODO alpha four").await;
        // Empty remainder: the whole committed text WAS the keyword.
        promote(&env, BETA_ID, "TODO").await;
        settle(&env).await;

        // ONE undo press per source commit. Under arm (d) the press genuinely
        // walks the gesture back: the inverse restores the CONVERGED value the
        // write replaced, and the block stops being a task. What matters here
        // is that the post-undo state is REPRESENTABLE — neither block is left
        // holding keyword-headed text with no task state, which is the illegal
        // pair the cold boot would disagree with.
        env.engine()
            .undo()
            .await
            .expect("undo the beta source commit");
        env.engine()
            .undo()
            .await
            .expect("undo the alpha source commit");
        settle(&env).await;

        let alpha_1 = block_row(&env, ALPHA_ID).await;
        let beta_1 = block_row(&env, BETA_ID).await;
        assert_eq!(
            (alpha_1.0.as_str(), alpha_1.1.as_deref()),
            ("alpha four", None),
            "boot-1 premise: undo restores the state the gesture replaced — the block was \
             plain text before, and `alpha four` is plain text, so nothing illegal is held"
        );
        assert_eq!(
            (beta_1.0.as_str(), beta_1.1.as_deref()),
            ("beta", None),
            "boot-1 premise, empty-remainder arm: the source commit REPLACED the block's \
             text with the bare keyword, and undo puts that text back plain"
        );

        let (rendered_name, rendered) = rendered_page(&env, "page-inbox").await;
        eprintln!("[cold-boot F2] rendered {rendered_name}:\n{rendered}");
        assert!(
            !rendered.contains("#+TODO:"),
            "this leg must run under the DEFAULT vocabulary, or `TODO` would not be a \
             keyword and the test would prove nothing:\n{rendered}"
        );
        env.stop_app().await.expect("stop_app after boot-1");

        // ── Boot 2: a FRESH database over those bytes ───────────────────
        let mut cold = TestEnvironment::new(rt).expect("cold TestEnvironment::new");
        cold.write_org_file(&rendered_name, &rendered)
            .await
            .expect("seed the cold vault");
        cold.start_app(true).await.expect("boot-2 start_app");
        wait_for_seed(&cold, ALPHA_ID).await;
        wait_for_seed(&cold, BETA_ID).await;
        settle(&cold).await;

        let alpha_2 = block_row(&cold, ALPHA_ID).await;
        let beta_2 = block_row(&cold, BETA_ID).await;
        cold.stop_app().await.expect("stop_app after boot-2");

        assert_eq!(
            alpha_2, alpha_1,
            "the cold boot read the block differently than the store held it — the F2 \
             divergence, back. Rendered bytes:\n{rendered}"
        );
        assert_eq!(
            beta_2, beta_1,
            "empty-remainder arm diverged across the cold boot; rendered bytes:\n{rendered}"
        );
    });
}
