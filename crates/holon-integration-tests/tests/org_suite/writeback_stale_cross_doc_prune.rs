//! Pin for arm (b) of the journals phantom: an aggregator file holds a STALE
//! copy of a block its day page owns, and the owner's file still holds it.
//!
//! Shape:
//!   * `DayPage.org` (`#+ID: area-daypage`, a Page) OWNS the child block
//!     `bulk-0-0` ("Q1W  q9").
//!   * `Overview.org` (`#+ID: overview`, a Page) ALSO carries `bulk-0-0` flat
//!     on disk, a copy left by a past mis-route / crash / external edit.
//!
//! D229.b: the ingest cannot tell this copy from the first half of a move, so
//! it deletes neither. The day page stays authoritative, both files keep
//! their bytes, a `block-in-two-files` condition names the block, and the org
//! fixed-point is stable. Deleting the copy by hand clears the condition.
//!
//! @pbt kind harness
//! @pbt covers cross-doc-membership(ingest) — a second on-disk copy of an
//! owned block is disclosed, never adopted away from its owner and never
//! removed from disk (journals phantom arm b)
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone's
//! `PasteBlockCopy` reaches the two-copies state only through a cut & paste;
//! these pin a stale copy, an owner file that reads empty or goes missing, a
//! third file, and a copied subtree

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

const STALE_ID: &str = "block:bulk-0-0";
const DAY_PAGE: &str = "block:area-daypage";

const DAY_PAGE_ORG: &str = "\
#+TITLE: DayPage
#+ID: area-daypage
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
";

// The aggregator carrying a STALE flat copy of the day-page's child.
const OVERVIEW_ORG: &str = "\
#+TITLE: Overview
#+ID: overview
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
";

async fn read_file(env: &TestEnvironment, name: &str) -> String {
    use holon_filesystem::FileSystem;
    let path = env.org_root().join(name);
    env.org_fs.read_to_string(&path).await.unwrap_or_default()
}

async fn parent_of(env: &TestEnvironment, id: &str) -> Option<String> {
    let rows = env
        .query_sql(&format!(
            "SELECT parent_id FROM block_raw WHERE id = '{id}'"
        ))
        .await
        .expect("query block_raw parent_id");
    rows.first()
        .and_then(|r| r.get("parent_id").and_then(|v| v.as_string()))
        .map(|s| s.to_string())
}

async fn count_rows(env: &TestEnvironment, id: &str) -> usize {
    env.query_sql(&format!("SELECT id FROM block_raw WHERE id = '{id}'"))
        .await
        .expect("query block_raw count")
        .len()
}

fn two_files_conditions(env: &TestEnvironment) -> Vec<String> {
    env.injector()
        .expect("test environment must expose its injector")
        .resolve::<Arc<holon_api::ConditionBus>>()
        .current()
        .into_iter()
        .filter(|c| c.reason.condition_kind() == holon_api::ConditionKind::BLOCK_IN_TWO_FILES)
        .map(|c| c.subject)
        .collect()
}

#[test]
fn stale_cross_doc_copy_is_disclosed_and_kept() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        // Desktop dogfood shape: SqlOnly (Loro off).
        env.set_enable_loro(false);

        env.write_org_file("DayPage.org", DAY_PAGE_ORG)
            .await
            .expect("write DayPage.org");

        env.start_app(true).await.expect("start_app");

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            if count_rows(&env, STALE_ID).await >= 1 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "bulk-0-0 never landed from DayPage.org"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some(DAY_PAGE),
            "precondition: bulk-0-0 must be owned by the day-page before the phantom appears"
        );
        tokio::time::sleep(Duration::from_millis(400)).await;

        env.write_org_file("Overview.org", OVERVIEW_ORG)
            .await
            .expect("write Overview.org");
        let seq = env.org_fs.last_change_seq();
        env.wait_for_org_change_processed(seq, Duration::from_secs(20))
            .await;
        tokio::time::sleep(Duration::from_millis(800)).await;

        let overview_disk = read_file(&env, "Overview.org").await;
        let day_disk = read_file(&env, "DayPage.org").await;

        assert_eq!(
            count_rows(&env, STALE_ID).await,
            1,
            "bulk-0-0 must exist exactly once. day={day_disk}\noverview={overview_disk}"
        );
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some(DAY_PAGE),
            "bulk-0-0 was ADOPTED away from its owner while DayPage.org still holds it. \
             overview.org=\n{overview_disk}"
        );
        assert!(
            overview_disk.contains("bulk-0-0") && day_disk.contains("bulk-0-0"),
            "both copies must stay on disk.\nday={day_disk}\noverview={overview_disk}"
        );
        assert_eq!(
            two_files_conditions(&env),
            vec![STALE_ID.to_string()],
            "the copy in Overview.org must be disclosed as a block in two files"
        );

        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(
            read_file(&env, "Overview.org").await,
            overview_disk,
            "Overview.org kept changing after settle — org fixed-point oscillates"
        );
        assert_eq!(
            read_file(&env, "DayPage.org").await,
            day_disk,
            "DayPage.org kept changing after settle — org fixed-point oscillates"
        );

        // The user deletes the stale copy by hand.
        env.write_org_file("Overview.org", "#+TITLE: Overview\n#+ID: overview\n")
            .await
            .expect("rewrite Overview.org without the copy");
        let seq = env.org_fs.last_change_seq();
        env.wait_for_org_change_processed(seq, Duration::from_secs(20))
            .await;
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some(DAY_PAGE),
            "deleting the copy must leave the owner's block in place"
        );
        assert!(
            two_files_conditions(&env).is_empty(),
            "the condition must clear once the copy is gone: {:?}",
            two_files_conditions(&env)
        );

        env.stop_app().await.expect("stop_app");
    });
}

/// `(subject, owner file name, copy file names)` of every block-in-two-files
/// condition in effect.
fn copies_disclosed(env: &TestEnvironment) -> Vec<(String, String, Vec<String>)> {
    two_files_disclosure(
        env.injector()
            .expect("test environment must expose its injector")
            .resolve::<Arc<holon_api::ConditionBus>>()
            .current(),
    )
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .unwrap_or_else(|| panic!("{path} has no file name"))
        .to_string_lossy()
        .into_owned()
}

fn two_files_disclosure(
    conditions: impl IntoIterator<Item = holon_api::Condition>,
) -> Vec<(String, String, Vec<String>)> {
    conditions
        .into_iter()
        .filter_map(|c| match c.reason {
            holon_api::ConditionKind::BlockInTwoFiles {
                owner_file,
                copy_files,
            } => Some((
                c.subject,
                file_name(&owner_file),
                copy_files.iter().map(|f| file_name(f)).collect(),
            )),
            _ => None,
        })
        .collect()
}

async fn wait_for_row(env: &TestEnvironment, id: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while count_rows(env, id).await < 1 {
        assert!(std::time::Instant::now() < deadline, "{id} never landed");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(400)).await;
}

async fn save_and_settle(env: &TestEnvironment, name: &str, body: &str) {
    env.write_org_file(name, body)
        .await
        .unwrap_or_else(|e| panic!("write {name}: {e:#}"));
    let seq = env.org_fs.last_change_seq();
    env.wait_for_org_change_processed(seq, Duration::from_secs(20))
        .await;
    tokio::time::sleep(Duration::from_millis(800)).await;
}

async fn started_with_day_page(
    rt: Arc<tokio::runtime::Runtime>,
    day_page: &str,
) -> TestEnvironment {
    let env = TestEnvironment::new(rt).expect("TestEnvironment::new");
    env.set_enable_loro(false);
    env.write_org_file("DayPage.org", day_page)
        .await
        .expect("write DayPage.org");
    env.start_app(true).await.expect("start_app");
    wait_for_row(&env, STALE_ID).await;
    env
}

fn owned_by_day_page(copy_files: &[&str]) -> Vec<(String, String, Vec<String>)> {
    vec![(
        STALE_ID.to_string(),
        "DayPage.org".to_string(),
        copy_files.iter().map(|f| f.to_string()).collect(),
    )]
}

/// A truncate-then-write save of the owner's file shows it at 0 bytes. An
/// ingest of another file holding a copy in that window must not read the
/// empty file as the owner releasing the block.
#[test]
fn an_owner_file_that_reads_empty_keeps_the_block() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_ORG).await;

        save_and_settle(&env, "DayPage.org", "").await;
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some(DAY_PAGE),
            "premise: an empty owner file is a save in flight and keeps its blocks"
        );

        save_and_settle(&env, "Overview.org", OVERVIEW_ORG).await;
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some(DAY_PAGE),
            "the block was handed to Overview.org because its owner's file read empty for one \
             ingest"
        );
        assert_eq!(
            copies_disclosed(&env),
            owned_by_day_page(&["Overview.org"]),
            "the copy in Overview.org must be disclosed with the day page as the owner"
        );

        save_and_settle(&env, "DayPage.org", DAY_PAGE_ORG).await;
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some(DAY_PAGE),
            "the restored day page must still own its block"
        );
        assert_eq!(
            copies_disclosed(&env),
            owned_by_day_page(&["Overview.org"]),
            "the day page is the owner and Overview.org holds the copy, never the reverse"
        );
        assert!(
            read_file(&env, "Overview.org").await.contains("bulk-0-0")
                && read_file(&env, "DayPage.org").await.contains("bulk-0-0"),
            "both copies must stay on disk"
        );

        env.stop_app().await.expect("stop_app");
    });
}

/// The owner's file is gone when another file's copy is ingested (a rename's
/// remove-then-create window, a sync tool replacing it). A missing file is not
/// evidence of a release, so the copy is disclosed. When the removal itself is
/// processed, the page's blocks are deleted and the copy is what remains.
#[test]
fn a_missing_owner_file_does_not_hand_the_block_away() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_ORG).await;
        let bus = env
            .injector()
            .expect("test environment must expose its injector")
            .resolve::<Arc<holon_api::ConditionBus>>();
        let mut changes = bus.subscribe().changes;

        env.write_org_file("Overview.org", OVERVIEW_ORG)
            .await
            .expect("write Overview.org");
        env.org_fs
            .remove_file(&env.org_root().join("DayPage.org"))
            .expect("remove DayPage.org");
        let seq = env.org_fs.last_change_seq();
        env.wait_for_org_change_processed(seq, Duration::from_secs(20))
            .await;
        tokio::time::sleep(Duration::from_millis(800)).await;

        let mut raised = Vec::new();
        while let Ok(change) = changes.try_recv() {
            raised.extend(change.raised());
        }
        assert_eq!(
            two_files_disclosure(raised),
            owned_by_day_page(&["Overview.org"]),
            "Overview.org's copy was adopted without a disclosure because the owner's file was \
             missing when Overview.org was ingested"
        );
        assert_eq!(
            count_rows(&env, STALE_ID).await,
            1,
            "the block must survive the owner file's removal"
        );
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some("block:overview"),
            "once the owner's page is gone, the copy in Overview.org holds the block"
        );
        assert!(
            read_file(&env, "Overview.org").await.contains("bulk-0-0"),
            "the copy must stay on disk"
        );
        assert!(
            copies_disclosed(&env).is_empty(),
            "the block is in one file again: {:?}",
            copies_disclosed(&env)
        );

        env.stop_app().await.expect("stop_app");
    });
}

const OVERVIEW_WITH_OWN_BLOCK: &str = "\
#+TITLE: Overview
#+ID: overview
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
* Overview own line
:PROPERTIES:
:ID: overview-own
:END:
";

const THIRD_ORG: &str = "\
#+TITLE: Third
#+ID: thirdpage
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
* Third own line
:PROPERTIES:
:ID: third-own
:END:
";

/// Three files hold one block. Every copy stays on disk through store edits,
/// and the disclosure names every file.
#[test]
fn three_files_holding_one_block_keep_every_copy() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_ORG).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_OWN_BLOCK).await;
        save_and_settle(&env, "Third.org", THIRD_ORG).await;

        env.update_block_content(STALE_ID, "Q1W  q9 edited in Holon")
            .await
            .expect("edit the block in the store");
        env.update_block_content("block:overview-own", "Overview edited in Holon")
            .await
            .expect("edit a block Overview.org owns");
        tokio::time::sleep(Duration::from_millis(2000)).await;

        for name in ["DayPage.org", "Overview.org", "Third.org"] {
            let disk = read_file(&env, name).await;
            assert!(
                disk.contains("bulk-0-0"),
                "the copy of bulk-0-0 was deleted from {name} on disk:\n{disk}"
            );
        }
        assert_eq!(
            copies_disclosed(&env),
            owned_by_day_page(&["Overview.org", "Third.org"]),
            "the disclosure must name the owner file and both files with a copy"
        );

        env.stop_app().await.expect("stop_app");
    });
}

const DAY_PAGE_WITH_CHILD: &str = "\
#+TITLE: DayPage
#+ID: area-daypage
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
** child line
:PROPERTIES:
:ID: bulk-0-0-kid
:END:
";

const OVERVIEW_WITH_CHILD: &str = "\
#+TITLE: Overview
#+ID: overview
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
** child line
:PROPERTIES:
:ID: bulk-0-0-kid
:END:
";

/// A copied heading with children is one disclosure, named by the heading.
#[test]
fn a_copied_subtree_is_disclosed_once() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_CHILD).await;
        wait_for_row(&env, "block:bulk-0-0-kid").await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;

        assert_eq!(
            copies_disclosed(&env),
            owned_by_day_page(&["Overview.org"]),
            "one condition for the copied heading, none for its children"
        );
        assert_eq!(
            parent_of(&env, "block:bulk-0-0-kid").await.as_deref(),
            Some(STALE_ID),
            "the child stays in its owner's subtree"
        );
        assert!(
            read_file(&env, "Overview.org")
                .await
                .contains("bulk-0-0-kid")
                && read_file(&env, "DayPage.org")
                    .await
                    .contains("bulk-0-0-kid"),
            "both copies of the child must stay on disk"
        );

        env.stop_app().await.expect("stop_app");
    });
}

const DAY_PAGE_RELEASED: &str = "\
#+TITLE: DayPage
#+ID: area-daypage
* Day keeps this
:PROPERTIES:
:ID: day-keeps
:END:
";

const DAY_PAGE_WITH_KEEPER: &str = "\
#+TITLE: DayPage
#+ID: area-daypage
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
* Day keeps this
:PROPERTIES:
:ID: day-keeps
:END:
";

/// `expr` over `id`'s `block_raw` row.
async fn column_of(env: &TestEnvironment, id: &str, expr: &str) -> String {
    let rows = env
        .query_sql(&format!(
            "SELECT {expr} AS v FROM block_raw WHERE id = '{id}'"
        ))
        .await
        .unwrap_or_else(|e| panic!("query {expr} of {id}: {e:#}"));
    let value = rows
        .first()
        .and_then(|r| r.get("v").cloned())
        .unwrap_or_else(|| panic!("{id} has no row for {expr}"));
    match value {
        holon_api::Value::String(s) => s,
        other => format!("{other:?}"),
    }
}

/// `(subject, kind)` of every condition about copies in effect.
fn copy_conditions(env: &TestEnvironment) -> Vec<(String, &'static str)> {
    let mut out: Vec<(String, &'static str)> = env
        .injector()
        .expect("test environment must expose its injector")
        .resolve::<Arc<holon_api::ConditionBus>>()
        .current()
        .into_iter()
        .map(|c| (c.subject, c.reason.condition_kind()))
        .filter(|(_, kind)| {
            [
                holon_api::ConditionKind::BLOCK_IN_TWO_FILES,
                holon_api::ConditionKind::BLOCK_EDITED_IN_TWO_FILES,
                holon_api::ConditionKind::DELETED_BLOCK_KEPT_IN_FILE,
                holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE,
            ]
            .contains(kind)
        })
        .collect();
    out.sort();
    out
}

async fn eventually_on_disk(env: &TestEnvironment, name: &str, needle: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let disk = read_file(env, name).await;
        if disk.contains(needle) {
            return disk;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{name} never held {needle:?}:\n{disk}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// VPROBE 6 of the round-2 verification: while a copy stands, the user edits
/// the copied block in Holon, and the edit reaches the owner's file. When the
/// owner's file then lets the block go, the file with the untouched copy
/// adopts it, and the edit survives the adoption.
#[test]
fn a_holon_edit_to_the_copied_block_survives_its_adoption() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KEEPER).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_OWN_BLOCK).await;
        assert_eq!(
            copies_disclosed(&env),
            owned_by_day_page(&["Overview.org"]),
            "premise: the copy is disclosed"
        );

        env.update_block_content(STALE_ID, "EDITED IN HOLON")
            .await
            .expect("edit the copied block in Holon");
        eventually_on_disk(&env, "DayPage.org", "EDITED IN HOLON").await;

        save_and_settle(&env, "DayPage.org", DAY_PAGE_RELEASED).await;
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some("block:overview"),
            "the owner's file let the block go, so Overview.org adopts it"
        );
        assert_eq!(
            column_of(&env, STALE_ID, "content").await,
            "EDITED IN HOLON",
            "the adoption overwrote the Holon edit with the copy's older text"
        );
        eventually_on_disk(&env, "Overview.org", "EDITED IN HOLON").await;
        assert!(
            copy_conditions(&env).is_empty(),
            "the block is in one file: {:?}",
            copy_conditions(&env)
        );

        env.stop_app().await.expect("stop_app");
    });
}

const THIRD_HOLDS_NEW: &str = "\
#+TITLE: Third
#+ID: thirdpage
* new in overview
:PROPERTIES:
:ID: new-in-overview
:END:
";

/// VPROBE 7 of the round-2 verification: a file that holds a copy still gets
/// Holon's edits, so a block Holon creates under its page reaches its disk.
/// A third file that carries that block's id holds a copy of it; Overview's
/// file never let it go.
#[test]
fn a_file_that_holds_a_copy_still_gets_holons_edits() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_ORG).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_OWN_BLOCK).await;

        env.create_block("block:new-in-overview", "block:overview", "new in overview")
            .await
            .expect("create a block under Overview's page");
        env.update_block_content("block:overview-own", "Overview edited in Holon")
            .await
            .expect("edit a block Overview.org owns");
        eventually_on_disk(&env, "Overview.org", "new-in-overview").await;
        let overview = eventually_on_disk(&env, "Overview.org", "Overview edited in Holon").await;
        assert!(
            overview.contains(":ID: bulk-0-0"),
            "the write-back dropped the copy:\n{overview}"
        );

        save_and_settle(&env, "Third.org", THIRD_HOLDS_NEW).await;
        assert_eq!(
            parent_of(&env, "block:new-in-overview").await.as_deref(),
            Some("block:overview"),
            "Overview.org never let the block go, so Third.org holds a copy of it"
        );
        let mut disclosed = copies_disclosed(&env);
        disclosed.sort();
        assert_eq!(
            disclosed,
            vec![
                (
                    STALE_ID.to_string(),
                    "DayPage.org".to_string(),
                    vec!["Overview.org".to_string()]
                ),
                (
                    "block:new-in-overview".to_string(),
                    "Overview.org".to_string(),
                    vec!["Third.org".to_string()]
                ),
            ],
        );

        env.stop_app().await.expect("stop_app");
    });
}

const OVERVIEW_WITH_TODO_COPY: &str = "\
#+TITLE: Overview
#+ID: overview
* TODO Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
* Overview own line
:PROPERTIES:
:ID: overview-own
:END:
";

/// The copy and Holon's version were both edited apart, so the release is
/// refused and the block goes back to its owner's file. The user's next
/// deletion from the owner's file is the choice: the copy is adopted as it is,
/// with no second refusal.
#[test]
fn a_conflict_ends_with_the_users_next_deletion() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KEEPER).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_OWN_BLOCK).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_TODO_COPY).await;
        let mut params: holon_api::StorageEntity = std::collections::HashMap::new();
        params.insert("id".into(), holon_api::Value::String(STALE_ID.to_string()));
        params.insert(
            "field".into(),
            holon_api::Value::String("task_state".into()),
        );
        params.insert("value".into(), holon_api::Value::String("DONE".into()));
        env.test_ctx()
            .execute_op("block", "set_field", params)
            .await
            .expect("set the copied block DONE in Holon");
        eventually_on_disk(&env, "DayPage.org", "DONE Q1W").await;

        save_and_settle(&env, "DayPage.org", DAY_PAGE_RELEASED).await;
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some(DAY_PAGE),
            "the release must be refused: both sides set the task state apart"
        );
        eventually_on_disk(&env, "DayPage.org", "DONE Q1W").await;
        assert!(read_file(&env, "Overview.org").await.contains("TODO Q1W"));
        assert_eq!(
            copy_conditions(&env),
            vec![(
                STALE_ID.to_string(),
                holon_api::ConditionKind::BLOCK_EDITED_IN_TWO_FILES
            )],
        );

        save_and_settle(&env, "DayPage.org", DAY_PAGE_RELEASED).await;
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some("block:overview"),
            "deleting the block from the owner's file again chooses the copy"
        );
        assert_eq!(
            column_of(&env, STALE_ID, "json_extract(properties, '$.task_state')").await,
            "TODO"
        );
        assert!(
            !read_file(&env, "DayPage.org").await.contains("bulk-0-0"),
            "the block must not come back to the owner's file a second time"
        );
        assert!(
            copy_conditions(&env).is_empty(),
            "{:?}",
            copy_conditions(&env)
        );

        env.stop_app().await.expect("stop_app");
    });
}

/// A block deleted in Holon while a copy stands: no copy is deleted from
/// disk, the file with the copy brings the block back as its own, and the
/// user is told.
#[test]
fn a_block_deleted_in_holon_lives_on_in_its_copy() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KEEPER).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_OWN_BLOCK).await;

        env.delete_block(STALE_ID)
            .await
            .expect("delete the copied block in Holon");
        tokio::time::sleep(Duration::from_millis(2000)).await;

        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some("block:overview"),
            "the copy in Overview.org must bring the block back as Overview's"
        );
        assert!(
            read_file(&env, "Overview.org")
                .await
                .contains(":ID: bulk-0-0")
        );
        assert!(!read_file(&env, "DayPage.org").await.contains("bulk-0-0"));
        assert_eq!(
            copy_conditions(&env),
            vec![(
                STALE_ID.to_string(),
                holon_api::ConditionKind::DELETED_BLOCK_KEPT_IN_FILE
            )],
        );

        env.delete_block(STALE_ID)
            .await
            .expect("delete it again, now from Overview");
        tokio::time::sleep(Duration::from_millis(2000)).await;
        assert_eq!(count_rows(&env, STALE_ID).await, 0);
        assert!(
            copy_conditions(&env).is_empty(),
            "{:?}",
            copy_conditions(&env)
        );

        env.stop_app().await.expect("stop_app");
    });
}

const OVERVIEW_OWN_ONLY: &str = "\
#+TITLE: Overview
#+ID: overview
* Overview own line
:PROPERTIES:
:ID: overview-own
:END:
";

/// The block moved while Holon was closed. Whichever file the boot scan reads
/// first, the file with the block adopts it: the block is never deleted and
/// re-created, which would lose what only the store holds (its `created_at`).
fn moved_while_closed_is_adopted(copy_file: &'static str) {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KEEPER).await;
        save_and_settle(&env, copy_file, OVERVIEW_OWN_ONLY).await;
        let created_at = column_of(&env, STALE_ID, "created_at").await;
        env.stop_app().await.expect("stop_app");

        env.write_org_file("DayPage.org", DAY_PAGE_RELEASED)
            .await
            .expect("save DayPage.org without the block");
        env.write_org_file(copy_file, OVERVIEW_WITH_OWN_BLOCK)
            .await
            .expect("save the block into the other file");
        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;

        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some("block:overview"),
            "{copy_file} must adopt the block that moved while Holon was closed"
        );
        assert_eq!(
            column_of(&env, STALE_ID, "created_at").await,
            created_at,
            "the block was deleted and re-created at boot"
        );
        assert!(
            copy_conditions(&env).is_empty(),
            "{:?}",
            copy_conditions(&env)
        );

        env.stop_app().await.expect("stop_app");
    });
}

#[test]
fn moved_while_closed_is_adopted_when_the_owner_boots_first() {
    moved_while_closed_is_adopted("Overview.org");
}

#[test]
fn moved_while_closed_is_adopted_when_the_copy_boots_first() {
    moved_while_closed_is_adopted("Aaa-overview.org");
}

const DAY_PAGE_WITH_KID_AND_KEEPER: &str = "\
#+TITLE: DayPage
#+ID: area-daypage
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
** child line
:PROPERTIES:
:ID: bulk-0-0-kid
:END:
* Day keeps this
:PROPERTIES:
:ID: day-keeps
:END:
";

const DAY_PAGE_KID_REMOVED: &str = "\
#+TITLE: DayPage
#+ID: area-daypage
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
* Day keeps this
:PROPERTIES:
:ID: day-keeps
:END:
";

const OVERVIEW_COPY_WITHOUT_KID: &str = "\
#+TITLE: Overview
#+ID: overview
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
";

const KID: &str = "block:bulk-0-0-kid";

/// VPROBE K of the round-3 verification: a child of a copied heading that the
/// user deleted in Holon stays deleted when the copy is adopted, and the copy's
/// file loses it.
#[test]
fn a_child_deleted_in_holon_stays_deleted_when_its_copy_is_adopted() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        wait_for_row(&env, KID).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;
        env.delete_block(KID)
            .await
            .expect("delete the child in Holon");
        tokio::time::sleep(Duration::from_millis(2000)).await;
        assert_eq!(
            count_rows(&env, KID).await,
            0,
            "premise: the child is deleted"
        );

        save_and_settle(&env, "DayPage.org", DAY_PAGE_RELEASED).await;
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some("block:overview"),
            "premise: Overview.org adopts the heading"
        );
        assert_eq!(
            count_rows(&env, KID).await,
            0,
            "the adoption brought back the child the user deleted in Holon"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while read_file(&env, "Overview.org")
            .await
            .contains("bulk-0-0-kid")
        {
            assert!(
                std::time::Instant::now() < deadline,
                "Overview.org still holds the deleted child:\n{}",
                read_file(&env, "Overview.org").await
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            copy_conditions(&env).is_empty(),
            "{:?}",
            copy_conditions(&env)
        );

        env.stop_app().await.expect("stop_app");
    });
}

/// A child the copy no longer holds, and Holon did not change, is deleted
/// when the copy is adopted.
#[test]
fn a_child_the_copy_dropped_is_deleted_when_the_copy_is_adopted() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        wait_for_row(&env, KID).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;

        save_and_settle(&env, "DayPage.org", DAY_PAGE_RELEASED).await;
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some("block:overview")
        );
        assert_eq!(
            count_rows(&env, KID).await,
            0,
            "the child the copy dropped survived the adoption"
        );
        assert!(
            copy_conditions(&env).is_empty(),
            "{:?}",
            copy_conditions(&env)
        );

        env.stop_app().await.expect("stop_app");
    });
}

/// A child edited in Holon while the copy dropped it: the adoption is
/// refused, both stay, and the user is told.
#[test]
fn a_child_edited_in_holon_and_dropped_from_the_copy_refuses_the_adoption() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        wait_for_row(&env, KID).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;
        env.update_block_content(KID, "child edited in Holon")
            .await
            .expect("edit the child in Holon");
        eventually_on_disk(&env, "DayPage.org", "child edited in Holon").await;

        save_and_settle(&env, "DayPage.org", DAY_PAGE_RELEASED).await;
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some(DAY_PAGE),
            "the adoption must be refused"
        );
        assert_eq!(
            column_of(&env, KID, "content").await,
            "child edited in Holon"
        );
        assert_eq!(
            copy_conditions(&env),
            vec![(
                STALE_ID.to_string(),
                holon_api::ConditionKind::BLOCK_EDITED_IN_TWO_FILES
            )],
        );

        env.stop_app().await.expect("stop_app");
    });
}

/// VPROBE O of the round-3 verification: the user deletes a child line from
/// the owner's own file while a copy of its heading stands. Holon writes it
/// back, because a block another file holds is never deleted, and says so.
#[test]
fn a_deletion_undone_because_of_a_copy_is_disclosed() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        wait_for_row(&env, KID).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;

        save_and_settle(&env, "DayPage.org", DAY_PAGE_KID_REMOVED).await;
        assert_eq!(
            count_rows(&env, KID).await,
            1,
            "the child stays: a copy holds it"
        );
        eventually_on_disk(&env, "DayPage.org", "bulk-0-0-kid").await;
        let undone: Vec<(String, String, Vec<String>)> = env
            .injector()
            .expect("injector")
            .resolve::<Arc<holon_api::ConditionBus>>()
            .current()
            .into_iter()
            .filter_map(|c| match c.reason {
                holon_api::ConditionKind::DeletionUndoneBlockInOtherFile { file, copy_files } => {
                    Some((
                        c.subject,
                        file_name(&file),
                        copy_files.iter().map(|f| file_name(f)).collect(),
                    ))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            undone,
            vec![(
                KID.to_string(),
                "DayPage.org".to_string(),
                vec!["Overview.org".to_string()]
            )],
            "the write-back undid the user's deletion silently"
        );

        // The user deletes the copy: no copy holds the child, so its deletion
        // stands.
        save_and_settle(&env, "Overview.org", "#+TITLE: Overview\n#+ID: overview\n").await;
        assert!(
            copy_conditions(&env).is_empty(),
            "{:?}",
            copy_conditions(&env)
        );
        assert_eq!(
            count_rows(&env, KID).await,
            0,
            "the deletion does not stand"
        );

        env.stop_app().await.expect("stop_app");
    });
}

/// The path of every refused file, of every format.
fn ingest_refusals(env: &TestEnvironment) -> Vec<String> {
    let bus = env
        .injector()
        .expect("injector")
        .resolve::<Arc<holon_api::ConditionBus>>();
    bus.current()
        .into_iter()
        .filter_map(|c| match c.reason {
            holon_api::ConditionKind::VaultIngestFailed(refusals) => {
                Some(bus.refused_files(refusals.format()))
            }
            _ => None,
        })
        .flatten()
        .map(|file| file.path)
        .collect()
}

/// The owner's file is edited while a copy of one of its headings, with a
/// child, stands in another file: the edit is ingested, and the owner is not
/// refused as declaring the copy's ids.
#[test]
fn the_owner_of_a_copied_subtree_is_still_ingested() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        wait_for_row(&env, KID).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;

        save_and_settle(
            &env,
            "DayPage.org",
            &DAY_PAGE_WITH_KID_AND_KEEPER.replace("** child line", "** child edited"),
        )
        .await;
        assert_eq!(ingest_refusals(&env), Vec::<String>::new());
        assert_eq!(column_of(&env, KID, "content").await, "child edited");

        env.stop_app().await.expect("stop_app");
    });
}

const JOURNALS_ORG: &str = "#+TITLE: Journals\n#+ID: journals\n";

const JOURNAL_DAY_ORG: &str = "\
#+ID: day-0116
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
** child line
:PROPERTIES:
:ID: bulk-0-0-kid
:END:
";

/// The journal shape of the same: a day page's heading is pasted into
/// `Journals.org`, the file of the page the day page sits under.
#[test]
fn a_journal_day_page_whose_heading_is_copied_into_journals_is_still_ingested() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.set_enable_loro(false);
        env.write_org_file("Journals.org", JOURNALS_ORG)
            .await
            .expect("write Journals.org");
        env.write_org_file("Journals/2026-01-16.org", JOURNAL_DAY_ORG)
            .await
            .expect("write the day file");
        env.start_app(true).await.expect("start_app");
        wait_for_row(&env, KID).await;

        let copy = JOURNAL_DAY_ORG.trim_start_matches("#+ID: day-0116\n");
        save_and_settle(&env, "Journals.org", &format!("{JOURNALS_ORG}{copy}")).await;
        assert_eq!(
            copy_conditions(&env),
            vec![(
                STALE_ID.to_string(),
                holon_api::ConditionKind::BLOCK_IN_TWO_FILES
            )],
        );

        save_and_settle(
            &env,
            "Journals/2026-01-16.org",
            &JOURNAL_DAY_ORG.replace("** child line", "** child edited"),
        )
        .await;
        assert_eq!(ingest_refusals(&env), Vec::<String>::new());
        assert_eq!(column_of(&env, KID, "content").await, "child edited");

        env.stop_app().await.expect("stop_app");
    });
}

/// VPROBE V9 of the round-4 verification: the user ends an undone deletion
/// the way the warning says, by deleting the child from the copy too. The
/// warning clears and the deletion from the owner's file stands.
#[test]
fn deleting_the_child_from_the_copy_too_lets_its_deletion_stand() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        wait_for_row(&env, KID).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;
        save_and_settle(&env, "DayPage.org", DAY_PAGE_KID_REMOVED).await;
        assert_eq!(
            copy_conditions(&env),
            vec![
                (
                    STALE_ID.to_string(),
                    holon_api::ConditionKind::BLOCK_IN_TWO_FILES
                ),
                (
                    KID.to_string(),
                    holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE
                ),
            ],
            "premise: the deletion is undone and disclosed"
        );

        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;
        assert_eq!(
            copy_conditions(&env),
            vec![(
                STALE_ID.to_string(),
                holon_api::ConditionKind::BLOCK_IN_TWO_FILES
            )],
            "the undone deletion is still disclosed after the copy lost the child"
        );
        assert_eq!(
            count_rows(&env, KID).await,
            0,
            "the deletion does not stand"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while read_file(&env, "DayPage.org")
            .await
            .contains("bulk-0-0-kid")
        {
            assert!(
                std::time::Instant::now() < deadline,
                "DayPage.org still holds the deleted child:\n{}",
                read_file(&env, "DayPage.org").await
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        env.stop_app().await.expect("stop_app");
    });
}

const THIRD_WITH_CHILD: &str = "\
#+TITLE: Third
#+ID: thirdpage
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
** child line
:PROPERTIES:
:ID: bulk-0-0-kid
:END:
* Third own line
:PROPERTIES:
:ID: third-own
:END:
";

const THIRD_WITHOUT_COPY: &str = "\
#+TITLE: Third
#+ID: thirdpage
* Third own line
:PROPERTIES:
:ID: third-own
:END:
";

const OVERVIEW_NO_COPY: &str = "#+TITLE: Overview\n#+ID: overview\n";

async fn assert_the_deletion_stands(env: &TestEnvironment) {
    assert_eq!(count_rows(env, KID).await, 0, "the deletion does not stand");
    assert!(
        !copy_conditions(env)
            .iter()
            .any(|(_, kind)| *kind == holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE),
        "the undone-deletion warning is still raised: {:?}",
        copy_conditions(env)
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while read_file(env, "DayPage.org").await.contains("bulk-0-0-kid") {
        assert!(
            std::time::Instant::now() < deadline,
            "DayPage.org still holds the child the user deleted:\n{}",
            read_file(env, "DayPage.org").await
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn undone_with_two_copies(rt: Arc<tokio::runtime::Runtime>) -> TestEnvironment {
    let env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
    wait_for_row(&env, KID).await;
    save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;
    save_and_settle(&env, "Third.org", THIRD_WITH_CHILD).await;
    save_and_settle(&env, "DayPage.org", DAY_PAGE_KID_REMOVED).await;
    assert!(
        copy_conditions(&env).contains(&(
            KID.to_string(),
            holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE
        )),
        "premise: the undone deletion is disclosed"
    );
    env
}

/// VPROBE VP1 of the round-5 verification: a copy file that drops the whole
/// copied heading no longer holds the child either.
#[test]
fn an_undone_deletion_stands_when_the_other_copy_drops_the_whole_heading() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = undone_with_two_copies(rt).await;
        save_and_settle(&env, "Third.org", THIRD_WITHOUT_COPY).await;
        assert_eq!(
            count_rows(&env, KID).await,
            1,
            "Overview.org still holds the child"
        );
        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;
        assert_the_deletion_stands(&env).await;
        env.stop_app().await.expect("stop_app");
    });
}

/// VPROBE VP2: a copy file deleted from disk holds nothing.
#[test]
fn an_undone_deletion_stands_when_the_other_copy_file_is_deleted() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = undone_with_two_copies(rt).await;
        let third = env.org_root().join("Third.org");
        env.org_fs.remove_file(&third).expect("remove Third.org");
        let seq = env.org_fs.last_change_seq();
        env.wait_for_org_change_processed(seq, Duration::from_secs(20))
            .await;
        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert_eq!(
            count_rows(&env, KID).await,
            1,
            "Overview.org still holds the child"
        );
        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;
        assert_the_deletion_stands(&env).await;
        env.stop_app().await.expect("stop_app");
    });
}

/// VPROBE VP3: the user deletes the whole copied heading from the last copy
/// file, as the two-files banner says; the child's deletion stands.
#[test]
fn an_undone_deletion_stands_when_the_last_copy_is_deleted() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        wait_for_row(&env, KID).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;
        save_and_settle(&env, "DayPage.org", DAY_PAGE_KID_REMOVED).await;
        assert_eq!(
            copy_conditions(&env),
            vec![
                (
                    STALE_ID.to_string(),
                    holon_api::ConditionKind::BLOCK_IN_TWO_FILES
                ),
                (
                    KID.to_string(),
                    holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE
                ),
            ],
        );
        save_and_settle(&env, "Overview.org", OVERVIEW_NO_COPY).await;
        assert_the_deletion_stands(&env).await;
        assert!(
            copy_conditions(&env).is_empty(),
            "{:?}",
            copy_conditions(&env)
        );
        env.stop_app().await.expect("stop_app");
    });
}

/// No copy holds the child, so nothing puts it back.
#[test]
fn a_child_no_copy_holds_is_deleted_at_once() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        wait_for_row(&env, KID).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;
        save_and_settle(&env, "DayPage.org", DAY_PAGE_KID_REMOVED).await;
        assert_the_deletion_stands(&env).await;
        assert_eq!(
            copy_conditions(&env),
            vec![(
                STALE_ID.to_string(),
                holon_api::ConditionKind::BLOCK_IN_TWO_FILES
            )],
        );
        env.stop_app().await.expect("stop_app");
    });
}

/// The copy is adopted while a child's deletion is undone: the owner's side
/// deleted the child, the copy did not change it, so it stays deleted.
#[test]
fn an_undone_deletion_stands_when_the_copy_is_adopted() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        wait_for_row(&env, KID).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;
        save_and_settle(&env, "DayPage.org", DAY_PAGE_KID_REMOVED).await;
        save_and_settle(&env, "DayPage.org", DAY_PAGE_RELEASED).await;
        assert_eq!(
            parent_of(&env, STALE_ID).await.as_deref(),
            Some("block:overview"),
            "premise: Overview.org adopts the heading"
        );
        assert_eq!(
            count_rows(&env, KID).await,
            0,
            "the deletion does not stand"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while read_file(&env, "Overview.org")
            .await
            .contains("bulk-0-0-kid")
        {
            assert!(
                std::time::Instant::now() < deadline,
                "Overview.org still holds the deleted child:\n{}",
                read_file(&env, "Overview.org").await
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            copy_conditions(&env).is_empty(),
            "{:?}",
            copy_conditions(&env)
        );
        env.stop_app().await.expect("stop_app");
    });
}

/// The user deletes the child from its own file, Holon puts it back, and Holon
/// is closed.
async fn undone_then_closed(rt: Arc<tokio::runtime::Runtime>) -> TestEnvironment {
    let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
    wait_for_row(&env, KID).await;
    save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;
    save_and_settle(&env, "DayPage.org", DAY_PAGE_KID_REMOVED).await;
    assert!(
        copy_conditions(&env).contains(&(
            KID.to_string(),
            holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE
        )),
        "premise: the undone deletion is disclosed"
    );
    env.stop_app().await.expect("stop_app");
    env
}

/// The record is vault state, beside the Loro snapshot: the database is
/// rebuilt from the files and would lose it.
#[test]
fn an_undone_deletion_is_kept_beside_the_loro_snapshot() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = undone_then_closed(rt).await;
        assert!(
            read_file(&env, ".loro/undone-deletions.json")
                .await
                .contains("bulk-0-0-kid"),
            "the undone deletion is not kept beside the Loro snapshot"
        );
    });
}

/// The copy loses the child while Holon is closed: at boot the deletion from
/// the owner's file stands.
#[test]
fn an_undone_deletion_stands_when_the_copy_lets_go_while_closed() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = undone_then_closed(rt).await;
        env.write_org_file("Overview.org", OVERVIEW_COPY_WITHOUT_KID)
            .await
            .expect("remove the child from the copy");
        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_the_deletion_stands(&env).await;
        assert_eq!(
            copy_conditions(&env),
            vec![(
                STALE_ID.to_string(),
                holon_api::ConditionKind::BLOCK_IN_TWO_FILES
            )],
        );
        env.stop_app().await.expect("stop_app");
    });
}

/// Holon restarts while the deletion is undone: the disclosure comes back,
/// and the copy losing the child afterwards still lets the deletion stand.
#[test]
fn an_undone_deletion_survives_a_restart() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = undone_then_closed(rt).await;
        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(
            copy_conditions(&env).contains(&(
                KID.to_string(),
                holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE
            )),
            "the restart forgot the undone deletion: {:?}",
            copy_conditions(&env)
        );
        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;
        assert_the_deletion_stands(&env).await;
        env.stop_app().await.expect("stop_app");
    });
}

fn condition_subjects(env: &TestEnvironment, kind: &str) -> Vec<String> {
    env.injector()
        .expect("injector")
        .resolve::<Arc<holon_api::ConditionBus>>()
        .current()
        .into_iter()
        .filter(|c| c.reason.condition_kind() == kind)
        .map(|c| c.subject)
        .collect()
}

/// Boot over an undone deletion whose record reads as `bytes`: the record is
/// disclosed and moved aside, the vault still syncs, and nothing is lost.
fn an_unreadable_undone_deletions_record(bytes: &'static str) {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = undone_then_closed(rt).await;
        env.write_org_file(".loro/undone-deletions.json", bytes)
            .await
            .expect("damage the record");
        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let unreadable = condition_subjects(&env, holon_api::ConditionKind::VAULT_STATE_UNREADABLE);
        assert_eq!(
            unreadable.len(),
            1,
            "the unreadable record is not disclosed: {unreadable:?}"
        );
        assert!(unreadable[0].ends_with(".loro/undone-deletions.json"));
        assert!(
            read_file(&env, ".loro/undone-deletions.json")
                .await
                .is_empty(),
            "the unreadable record is still in place"
        );
        assert!(
            condition_subjects(&env, holon_api::ConditionKind::VAULT_SYNC_NOT_STARTED).is_empty(),
            "an unreadable record must not stop the sync"
        );
        assert_eq!(count_rows(&env, KID).await, 1, "the put-back child is gone");

        save_and_settle(
            &env,
            "Later.org",
            "#+TITLE: Later\n#+ID: later\n* After\n:PROPERTIES:\n:ID: after-1\n:END:\n",
        )
        .await;
        assert_eq!(
            count_rows(&env, "block:after-1").await,
            1,
            "the vault stopped syncing"
        );
        env.stop_app().await.expect("stop_app");
    });
}

#[test]
fn an_unparseable_undone_deletions_record_is_set_aside_and_disclosed() {
    an_unreadable_undone_deletions_record("{ this is not the json we wrote");
}

#[test]
fn an_empty_undone_deletions_record_is_set_aside_and_disclosed() {
    an_unreadable_undone_deletions_record("");
}

/// Stop a session whose file-sync controller died or never ran. What it had
/// queued is never written, and the shutdown names it; whether anything was
/// queued depends on where the controller died, so both outcomes are accepted.
async fn stop_after_the_controller_died(env: &mut TestEnvironment) {
    if let Err(e) = env.stop_app().await {
        assert!(
            format!("{e:#}").contains("the org write-back did not settle"),
            "stop_app after the controller died: {e:#}"
        );
    }
}

/// A controller that fails before it watches the vault says so: the user
/// sees that nothing syncs and why, instead of a healthy-looking window.
#[test]
fn a_file_sync_controller_that_cannot_start_is_disclosed() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.set_enable_loro(false);
        env.write_org_file("DayPage.org", DAY_PAGE_WITH_KID_AND_KEEPER)
            .await
            .expect("write DayPage.org");
        env.write_org_file(
            "DayPage.sync-conflict-20260101-000000-ABCDEFG.org",
            DAY_PAGE_WITH_KID_AND_KEEPER,
        )
        .await
        .expect("write a sync-conflict artifact");
        env.start_app(true).await.expect("start_app");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let not_started =
            condition_subjects(&env, holon_api::ConditionKind::VAULT_SYNC_NOT_STARTED);
        assert_eq!(
            not_started.len(),
            1,
            "the file sync never started and nothing says so"
        );
        stop_after_the_controller_died(&mut env).await;
    });
}

/// The app dies at `crash_point` while it undoes the user's deletion of the
/// child: the record of the undone deletion reached the vault first, so the
/// next boot still knows the deletion is pending, and it stands once the copy
/// lets go of the child.
fn an_undone_deletion_survives_a_crash_at(crash_point: &'static str, put_back_before_crash: bool) {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        wait_for_row(&env, KID).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;
        holon_filesystem::crash_injection::arm(crash_point);
        // The controller dies in this ingest, so it never reports it processed.
        env.write_org_file("DayPage.org", DAY_PAGE_KID_REMOVED)
            .await
            .expect("delete the child from DayPage.org");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            read_file(&env, "DayPage.org")
                .await
                .contains("bulk-0-0-kid"),
            put_back_before_crash,
            "premise: the crash at {crash_point} is on the wrong side of the put-back"
        );
        stop_after_the_controller_died(&mut env).await;

        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        eventually_on_disk(&env, "DayPage.org", "bulk-0-0-kid").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(
            copy_conditions(&env).contains(&(
                KID.to_string(),
                holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE
            )),
            "the crash lost the undone deletion: {:?}",
            copy_conditions(&env)
        );
        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;
        assert_the_deletion_stands(&env).await;
        env.stop_app().await.expect("stop_app");
    });
}

/// The put-back line is in the file, and the record written before it.
#[test]
fn an_undone_deletion_survives_a_crash_after_the_put_back() {
    an_undone_deletion_survives_a_crash_at("after_ingest_write_back", true);
}

/// Only the record reached the vault: the child is gone from its own file and
/// still in the store. The next boot puts it back again.
#[test]
fn an_undone_deletion_survives_a_crash_before_the_put_back() {
    an_undone_deletion_survives_a_crash_at("after_undone_deletions_persisted", false);
}

/// A vault the app cannot watch says so: file changes stop reaching Holon,
/// and the user sees that and why.
#[test]
fn a_vault_that_cannot_be_watched_is_disclosed() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
        env.set_enable_loro(false);
        env.write_org_file("DayPage.org", DAY_PAGE_WITH_KID_AND_KEEPER)
            .await
            .expect("write DayPage.org");
        holon_filesystem::crash_injection::arm("arm_watcher");
        env.start_app(true).await.expect("start_app");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let not_started =
            condition_subjects(&env, holon_api::ConditionKind::VAULT_SYNC_NOT_STARTED);
        assert_eq!(
            not_started.len(),
            1,
            "the vault is not watched and nothing says so"
        );
        env.stop_app().await.expect("stop_app");
    });
}

/// What ends the undone deletion of the child.
#[derive(Clone, Copy, Debug)]
enum Ending {
    /// The user cuts the whole heading from its own file; the copy adopts it.
    CopyAdopted,
    /// The user deletes the child from the copy too.
    CopyLetsGo,
    /// The user deletes the file that holds the copy.
    CopyFileDeleted,
    /// The user deletes the child in Holon.
    DeletedInHolon,
}

async fn end_the_undone_deletion(env: &TestEnvironment, ending: Ending) {
    match ending {
        Ending::CopyAdopted => {
            env.write_org_file("DayPage.org", DAY_PAGE_RELEASED)
                .await
                .expect("cut the heading from DayPage.org");
        }
        Ending::CopyLetsGo => {
            env.write_org_file("Overview.org", OVERVIEW_COPY_WITHOUT_KID)
                .await
                .expect("delete the child from Overview.org");
        }
        Ending::CopyFileDeleted => env
            .org_fs
            .remove_file(&env.org_root().join("Overview.org"))
            .expect("delete Overview.org"),
        Ending::DeletedInHolon => env
            .delete_block(KID)
            .await
            .expect("delete the child in Holon"),
    }
}

/// The app dies at `crash_point` while `ending` ends the undone deletion of
/// the child: after the restart the user's deletion stands, and nothing about
/// it is left disclosed.
fn a_standing_deletion_survives_a_crash_at(ending: Ending, crash_point: &'static str) {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        wait_for_row(&env, KID).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;
        save_and_settle(&env, "DayPage.org", DAY_PAGE_KID_REMOVED).await;
        assert!(
            copy_conditions(&env).contains(&(
                KID.to_string(),
                holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE
            )),
            "premise: the undone deletion is disclosed"
        );
        holon_filesystem::crash_injection::arm(crash_point);
        end_the_undone_deletion(&env, ending).await;
        tokio::time::sleep(Duration::from_millis(2000)).await;
        assert_eq!(
            holon_filesystem::crash_injection::armed(),
            None,
            "premise: {ending:?} never reached {crash_point}"
        );
        stop_after_the_controller_died(&mut env).await;

        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_the_deletion_stands(&env).await;
        if matches!(ending, Ending::CopyAdopted) {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while read_file(&env, "Overview.org")
                .await
                .contains("bulk-0-0-kid")
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "Overview.org still holds the deleted child:\n{}",
                    read_file(&env, "Overview.org").await
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        env.stop_app().await.expect("stop_app");
    });
}

#[test]
fn an_adopted_copy_keeps_the_deletion_after_a_crash_before_its_store_change() {
    a_standing_deletion_survives_a_crash_at(
        Ending::CopyAdopted,
        "after_undone_deletions_persisted",
    );
}

#[test]
fn an_adopted_copy_keeps_the_deletion_after_a_crash_after_its_write_back() {
    a_standing_deletion_survives_a_crash_at(Ending::CopyAdopted, "after_ingest_write_back");
}

#[test]
fn an_adopted_copy_keeps_the_deletion_after_a_crash_before_the_record_is_written() {
    a_standing_deletion_survives_a_crash_at(Ending::CopyAdopted, "before_undone_deletions_written");
}

#[test]
fn a_copy_letting_go_keeps_the_deletion_after_a_crash_before_its_store_change() {
    a_standing_deletion_survives_a_crash_at(Ending::CopyLetsGo, "after_undone_deletions_persisted");
}

#[test]
fn a_copy_letting_go_keeps_the_deletion_after_a_crash_before_the_record_is_written() {
    a_standing_deletion_survives_a_crash_at(Ending::CopyLetsGo, "before_undone_deletions_written");
}

#[test]
fn a_deleted_copy_file_keeps_the_deletion_after_a_crash_before_the_record_is_written() {
    a_standing_deletion_survives_a_crash_at(
        Ending::CopyFileDeleted,
        "before_undone_deletions_written",
    );
}

#[test]
fn a_holon_deletion_keeps_the_deletion_after_a_crash_before_the_record_is_written() {
    a_standing_deletion_survives_a_crash_at(
        Ending::DeletedInHolon,
        "before_undone_deletions_written",
    );
}

const OVERVIEW_WITH_CHILD_REFUSED: &str = "\
#+TITLE: Overview
#+ID: doc:overview
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
** child line
:PROPERTIES:
:ID: bulk-0-0-kid
:END:
";

/// The copy file is refused at boot while it still holds the child: whether
/// it holds it is unknown, so the deletion stays undone and disclosed, naming
/// the file.
#[test]
fn an_undone_deletion_stays_while_its_copy_file_cannot_be_read() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = undone_then_closed(rt).await;
        env.write_org_file("Overview.org", OVERVIEW_WITH_CHILD_REFUSED)
            .await
            .expect("give Overview.org a page id Holon refuses");
        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            ingest_refusals(&env).len(),
            1,
            "premise: Overview.org is refused"
        );
        assert_eq!(
            count_rows(&env, KID).await,
            1,
            "the deletion stood over a copy Holon could not read"
        );
        assert!(
            read_file(&env, "DayPage.org")
                .await
                .contains("bulk-0-0-kid"),
            "the put-back line left DayPage.org"
        );
        let undone: Vec<_> = env
            .injector()
            .expect("injector")
            .resolve::<Arc<holon_api::ConditionBus>>()
            .current()
            .into_iter()
            .filter_map(|c| match c.reason {
                holon_api::ConditionKind::DeletionUndoneBlockInOtherFile { copy_files, .. } => {
                    Some((c.subject, copy_files))
                }
                _ => None,
            })
            .collect();
        assert_eq!(undone.len(), 1, "the undone deletion is not disclosed");
        assert_eq!(undone[0].0, KID);
        assert!(
            undone[0].1.len() == 1 && undone[0].1[0].ends_with("Overview.org"),
            "the disclosure does not name the unread copy file: {undone:?}"
        );

        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;
        assert_eq!(
            count_rows(&env, KID).await,
            1,
            "the repaired copy lost the child"
        );
        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;
        assert_the_deletion_stands(&env).await;
        env.stop_app().await.expect("stop_app");
    });
}

/// One record of the undone deletions names a file outside the vault: that
/// record is refused and disclosed, the other records stand, and the vault
/// syncs.
#[test]
fn an_undone_deletion_record_outside_the_vault_is_refused_alone() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = undone_then_closed(rt).await;
        let valid = read_file(&env, ".loro/undone-deletions.json").await;
        let mut records: Vec<serde_json::Value> =
            serde_json::from_str(&valid).expect("premise: the record parses");
        assert_eq!(records.len(), 1, "premise: one undone deletion");
        records.push(serde_json::json!({
            "block": "block:elsewhere",
            "root": "block:elsewhere-root",
            "file": "/nowhere-outside-the-vault/Else.org",
            "copy_files": ["Overview.org"],
            "put_back": "0",
        }));
        env.write_org_file(
            ".loro/undone-deletions.json",
            &serde_json::to_string_pretty(&records).expect("serialize"),
        )
        .await
        .expect("write the record");
        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let unreadable = condition_subjects(&env, holon_api::ConditionKind::VAULT_STATE_UNREADABLE);
        assert_eq!(
            unreadable.len(),
            1,
            "the refused record is not disclosed: {unreadable:?}"
        );
        assert!(
            copy_conditions(&env).contains(&(
                KID.to_string(),
                holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE
            )),
            "the valid record was dropped with the refused one: {:?}",
            copy_conditions(&env)
        );
        save_and_settle(
            &env,
            "Later.org",
            "#+TITLE: Later\n#+ID: later\n* After\n:PROPERTIES:\n:ID: after-1\n:END:\n",
        )
        .await;
        assert_eq!(
            count_rows(&env, "block:after-1").await,
            1,
            "the vault stopped syncing"
        );
        assert!(
            ingest_refusals(&env).is_empty(),
            "a file is blamed for the refused record: {:?}",
            ingest_refusals(&env)
        );
        env.stop_app().await.expect("stop_app");
    });
}

/// A step of the vault's start fails after the sync started: the user is
/// told what did not run.
#[test]
fn a_failed_start_step_of_the_vault_is_disclosed() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = undone_then_closed(rt).await;
        holon_filesystem::crash_injection::arm("settle_undone_deletions");
        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            holon_filesystem::crash_injection::armed(),
            None,
            "premise: the boot never settled the undone deletions"
        );
        let incomplete = condition_subjects(&env, holon_api::ConditionKind::VAULT_START_INCOMPLETE);
        assert_eq!(
            incomplete.len(),
            1,
            "the failed start step is not disclosed"
        );
        env.stop_app().await.expect("stop_app");
    });
}

const DAY_PAGE_KID_RETYPED: &str = "\
#+TITLE: DayPage
#+ID: area-daypage
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
** child line I typed again on purpose
:PROPERTIES:
:ID: bulk-0-0-kid
:END:
* Day keeps this
:PROPERTIES:
:ID: day-keeps
:END:
";

async fn assert_the_deletion_ended_by_edit(env: &TestEnvironment, text: &str) {
    assert_eq!(
        count_rows(env, KID).await,
        1,
        "the edited block was deleted"
    );
    assert_eq!(column_of(env, KID, "content").await, text);
    assert!(
        read_file(env, "DayPage.org").await.contains(text),
        "DayPage.org lost the edited block:\n{}",
        read_file(env, "DayPage.org").await
    );
    assert_eq!(
        condition_subjects(env, holon_api::ConditionKind::DELETION_ENDED_BY_EDIT),
        vec![KID.to_string()],
        "the end of the deletion is not disclosed"
    );
    assert!(
        !copy_conditions(env)
            .iter()
            .any(|(_, kind)| *kind == holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE),
        "the undone-deletion warning is still raised: {:?}",
        copy_conditions(env)
    );
}

/// While Holon is closed, the copy lets go of the child and the user types
/// the child back into its own file, same id, new text: the new text stays.
#[test]
fn a_child_typed_back_with_new_text_while_closed_keeps_the_new_text() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = undone_then_closed(rt).await;
        env.write_org_file("Overview.org", OVERVIEW_COPY_WITHOUT_KID)
            .await
            .expect("the copy lets go");
        env.write_org_file("DayPage.org", DAY_PAGE_KID_RETYPED)
            .await
            .expect("the user types the child back with new text");
        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_the_deletion_ended_by_edit(&env, "child line I typed again on purpose").await;
        assert!(
            !read_file(&env, ".loro/undone-deletions.json")
                .await
                .contains("bulk-0-0-kid"),
            "the record of the ended deletion stays in the vault"
        );
        env.stop_app().await.expect("stop_app");
    });
}

/// The user edits the put-back child in Holon: the deletion no longer stands,
/// so the copy letting go keeps the edit.
#[test]
fn a_put_back_child_edited_in_holon_stays_when_the_copy_lets_go() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        wait_for_row(&env, KID).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;
        save_and_settle(&env, "DayPage.org", DAY_PAGE_KID_REMOVED).await;
        env.update_block_content(KID, "child edited in Holon")
            .await
            .expect("edit the put-back child in Holon");
        eventually_on_disk(&env, "DayPage.org", "child edited in Holon").await;
        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;
        assert_the_deletion_ended_by_edit(&env, "child edited in Holon").await;
        env.stop_app().await.expect("stop_app");
    });
}

/// The copy file the boot could not read is deleted by the user: the deletion
/// stands at once, and nothing names the vanished file.
#[test]
fn an_unread_copy_file_the_user_deletes_lets_the_deletion_stand() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = undone_then_closed(rt).await;
        env.write_org_file("Overview.org", OVERVIEW_WITH_CHILD_REFUSED)
            .await
            .expect("give Overview.org a page id Holon refuses");
        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            count_rows(&env, KID).await,
            1,
            "premise: the deletion stays undone"
        );
        env.org_fs
            .remove_file(&env.org_root().join("Overview.org"))
            .expect("delete Overview.org");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_the_deletion_stands(&env).await;
        env.stop_app().await.expect("stop_app");
    });
}

/// The walk over the vault's files fails at boot: the start is disclosed as
/// incomplete.
#[test]
fn a_vault_walk_that_fails_at_boot_is_disclosed() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = undone_then_closed(rt).await;
        holon_filesystem::crash_injection::arm("scan_vault_files");
        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            holon_filesystem::crash_injection::armed(),
            None,
            "premise: the boot never walked the vault"
        );
        let incomplete: Vec<String> = env
            .injector()
            .expect("injector")
            .resolve::<Arc<holon_api::ConditionBus>>()
            .current()
            .into_iter()
            .filter_map(|c| match c.reason {
                holon_api::ConditionKind::VaultStartIncomplete { step, .. } => Some(step),
                _ => None,
            })
            .collect();
        assert_eq!(
            incomplete,
            vec!["reading the vault's files".to_string()],
            "the failed walk is not disclosed"
        );
        env.stop_app().await.expect("stop_app");
    });
}

const DAY_PAGE_KID_WITH_GRANDCHILD: &str = "\
#+TITLE: DayPage
#+ID: area-daypage
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
** child line
:PROPERTIES:
:ID: bulk-0-0-kid
:END:
*** grandchild the user wrote
:PROPERTIES:
:ID: kid-gc
:END:
* Day keeps this
:PROPERTIES:
:ID: day-keeps
:END:
";

const NOTES_ORG: &str = "\
#+TITLE: Notes
#+ID: notes
* Note one
:PROPERTIES:
:ID: notes-1
:END:
";

const GRANDCHILD: &str = "block:kid-gc";

async fn put_back_kid(env: &TestEnvironment) {
    wait_for_row(env, KID).await;
    save_and_settle(env, "Overview.org", OVERVIEW_WITH_CHILD).await;
    save_and_settle(env, "DayPage.org", DAY_PAGE_KID_REMOVED).await;
    eventually_on_disk(env, "DayPage.org", "bulk-0-0-kid").await;
    assert!(
        copy_conditions(env).contains(&(
            KID.to_string(),
            holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE
        )),
        "premise: the undone deletion is disclosed: {:?}",
        copy_conditions(env)
    );
}

async fn assert_kid_and_grandchild_kept(env: &TestEnvironment) {
    assert_eq!(
        (
            count_rows(env, KID).await,
            count_rows(env, GRANDCHILD).await
        ),
        (1, 1),
        "the standing deletion took the block the user wrote under the put-back child"
    );
    let day_page = read_file(env, "DayPage.org").await;
    assert!(
        day_page.contains("bulk-0-0-kid") && day_page.contains("kid-gc"),
        "DayPage.org lost the child or the grandchild:\n{day_page}"
    );
    assert_eq!(
        condition_subjects(env, holon_api::ConditionKind::DELETION_ENDED_BY_EDIT),
        vec![KID.to_string()],
        "the end of the deletion is not disclosed"
    );
}

/// The user writes a grandchild under the put-back child in its own file;
/// the copy then lets go of the child: both stay.
#[test]
fn a_grandchild_written_under_the_put_back_child_stays_when_the_copy_lets_go() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        put_back_kid(&env).await;
        save_and_settle(&env, "DayPage.org", DAY_PAGE_KID_WITH_GRANDCHILD).await;
        wait_for_row(&env, GRANDCHILD).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;
        assert_kid_and_grandchild_kept(&env).await;
        env.stop_app().await.expect("stop_app");
    });
}

/// The user creates a grandchild under the put-back child in Holon; the copy
/// then lets go of the child: both stay.
#[test]
fn a_grandchild_created_in_holon_under_the_put_back_child_stays_when_the_copy_lets_go() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        put_back_kid(&env).await;
        env.create_block("kid-gc", "bulk-0-0-kid", "grandchild the user wrote")
            .await
            .expect("create the grandchild in Holon");
        eventually_on_disk(&env, "DayPage.org", "grandchild the user wrote").await;
        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;
        assert_kid_and_grandchild_kept(&env).await;
        env.stop_app().await.expect("stop_app");
    });
}

/// The user moves the put-back child in Holon to another page; the copy then
/// lets go of it: it stays on that page.
#[test]
fn a_put_back_child_moved_in_holon_to_another_page_stays_when_the_copy_lets_go() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KID_AND_KEEPER).await;
        save_and_settle(&env, "Notes.org", NOTES_ORG).await;
        wait_for_row(&env, "block:notes-1").await;
        put_back_kid(&env).await;
        let mut params = std::collections::HashMap::new();
        params.insert("id".to_string(), holon_api::Value::String(KID.to_string()));
        params.insert(
            "parent_id".to_string(),
            holon_api::Value::String("block:notes-1".to_string()),
        );
        env.execute_operation("block", "move_block", params)
            .await
            .expect("move the child in Holon to Notes");
        eventually_on_disk(&env, "Notes.org", "bulk-0-0-kid").await;
        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;
        assert_eq!(
            count_rows(&env, KID).await,
            1,
            "the moved child was deleted"
        );
        assert_eq!(parent_of(&env, KID).await.as_deref(), Some("block:notes-1"));
        let notes = read_file(&env, "Notes.org").await;
        assert!(
            notes.contains("bulk-0-0-kid"),
            "Notes.org lost the child:\n{notes}"
        );
        assert_eq!(
            condition_subjects(&env, holon_api::ConditionKind::DELETION_ENDED_BY_EDIT),
            vec![KID.to_string()],
            "the end of the deletion is not disclosed"
        );
        env.stop_app().await.expect("stop_app");
    });
}

/// The copy holds the child but not its grandchild. The user deletes the
/// child with its grandchild from its own file: the grandchild goes, the
/// child is put back unedited, and its deletion stands once the copy lets go.
#[test]
fn a_put_back_child_whose_grandchild_no_copy_holds_still_lets_its_deletion_stand() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_KID_WITH_GRANDCHILD).await;
        wait_for_row(&env, GRANDCHILD).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_CHILD).await;
        save_and_settle(&env, "DayPage.org", DAY_PAGE_KID_REMOVED).await;
        eventually_on_disk(&env, "DayPage.org", "bulk-0-0-kid").await;
        assert_eq!(
            count_rows(&env, GRANDCHILD).await,
            0,
            "premise: the grandchild no copy holds is deleted"
        );
        assert_eq!(
            condition_subjects(&env, holon_api::ConditionKind::DELETION_ENDED_BY_EDIT),
            Vec::<String>::new(),
            "the put-back child is said to be edited, though nobody edited it"
        );
        assert!(
            copy_conditions(&env).contains(&(
                KID.to_string(),
                holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE
            )),
            "the undone deletion is not disclosed: {:?}",
            copy_conditions(&env)
        );
        save_and_settle(&env, "Overview.org", OVERVIEW_COPY_WITHOUT_KID).await;
        assert_the_deletion_stands(&env).await;
        env.stop_app().await.expect("stop_app");
    });
}

const OVERVIEW_WITH_KID_AND_GRANDCHILD: &str = "\
#+TITLE: Overview
#+ID: overview
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
** child line
:PROPERTIES:
:ID: bulk-0-0-kid
:END:
*** grandchild the user wrote
:PROPERTIES:
:ID: kid-gc
:END:
";

/// The child and its grandchild are both put back. The user edits the
/// grandchild in Holon: its deletion ends, and so does the child's, which now
/// holds a block that stays.
#[test]
fn an_edit_of_a_put_back_grandchild_ends_the_put_back_child_too() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_KID_WITH_GRANDCHILD).await;
        wait_for_row(&env, GRANDCHILD).await;
        save_and_settle(&env, "Overview.org", OVERVIEW_WITH_KID_AND_GRANDCHILD).await;
        save_and_settle(&env, "DayPage.org", DAY_PAGE_KID_REMOVED).await;
        eventually_on_disk(&env, "DayPage.org", "kid-gc").await;
        assert_eq!(
            copy_conditions(&env)
                .into_iter()
                .filter(|(_, kind)| *kind
                    == holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE)
                .map(|(subject, _)| subject)
                .collect::<Vec<_>>(),
            vec![KID.to_string(), GRANDCHILD.to_string()],
            "premise: both deletions are undone"
        );
        env.update_block_content(GRANDCHILD, "grandchild edited in Holon")
            .await
            .expect("edit the put-back grandchild in Holon");
        eventually_on_disk(&env, "DayPage.org", "grandchild edited in Holon").await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        let mut ended = condition_subjects(&env, holon_api::ConditionKind::DELETION_ENDED_BY_EDIT);
        ended.sort();
        assert_eq!(
            ended,
            vec![KID.to_string(), GRANDCHILD.to_string()],
            "the child's deletion still stands over the grandchild that stays"
        );
        assert!(
            !copy_conditions(&env)
                .iter()
                .any(|(_, kind)| *kind
                    == holon_api::ConditionKind::DELETION_UNDONE_BLOCK_IN_OTHER_FILE),
            "an undone-deletion warning is still raised: {:?}",
            copy_conditions(&env)
        );
        env.stop_app().await.expect("stop_app");
    });
}

/// Holon writes an edit to Notes.org and closes; the user restores the bytes
/// Holon read before the edit; the next boot reads the restore.
#[test]
fn a_restore_of_earlier_bytes_while_closed_is_read_at_boot() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KEEPER).await;
        save_and_settle(&env, "Notes.org", NOTES_ORG).await;
        wait_for_row(&env, "block:notes-1").await;
        let read_before_the_edit = read_file(&env, "Notes.org").await;
        env.update_block_content("block:notes-1", "Note edited in Holon")
            .await
            .expect("edit notes-1 in Holon");
        eventually_on_disk(&env, "Notes.org", "Note edited in Holon").await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        env.stop_app().await.expect("stop_app");
        env.write_org_file("Notes.org", &read_before_the_edit)
            .await
            .expect("restore the earlier bytes while closed");
        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            read_file(&env, "Notes.org").await,
            read_before_the_edit,
            "the boot wrote over the restore"
        );
        assert_eq!(
            column_of(&env, "block:notes-1", "content").await,
            "Note one",
            "the boot did not read the restore"
        );
        env.stop_app().await.expect("stop_app");
    });
}

/// Holon dies after it wrote an edit to Notes.org and before it recorded what
/// it wrote; the user restores the bytes Holon read before the edit. The next
/// boot reads the restore.
#[test]
fn a_restore_after_a_crash_before_the_written_hash_is_recorded_is_read_at_boot() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KEEPER).await;
        save_and_settle(&env, "Notes.org", NOTES_ORG).await;
        wait_for_row(&env, "block:notes-1").await;
        let read_before_the_edit = read_file(&env, "Notes.org").await;
        holon_filesystem::crash_injection::arm("after_write_back");
        env.update_block_content("block:notes-1", "Note edited in Holon")
            .await
            .expect("edit notes-1 in Holon");
        eventually_on_disk(&env, "Notes.org", "Note edited in Holon").await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(
            holon_filesystem::crash_injection::armed(),
            None,
            "premise: the controller did not die after the write"
        );
        stop_after_the_controller_died(&mut env).await;
        env.write_org_file("Notes.org", &read_before_the_edit)
            .await
            .expect("restore the earlier bytes while closed");
        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            column_of(&env, "block:notes-1", "content").await,
            "Note one",
            "the boot did not read the restore"
        );
        assert_eq!(
            read_file(&env, "Notes.org").await,
            read_before_the_edit,
            "the boot wrote over the restore"
        );
        env.stop_app().await.expect("stop_app");
    });
}

/// `(kind, reason)` of every condition on the vault file `name`.
fn conditions_on_file(env: &TestEnvironment, name: &str) -> Vec<(&'static str, String)> {
    env.injector()
        .expect("test environment must expose its injector")
        .resolve::<Arc<holon_api::ConditionBus>>()
        .current()
        .into_iter()
        .filter(|c| c.subject.ends_with(&format!("/{name}")))
        .map(|c| (c.reason.condition_kind(), format!("{:?}", c.reason)))
        .collect()
}

async fn drop_the_file_table(env: &TestEnvironment) {
    use holon::di::DbHandleProvider;
    env.injector()
        .expect("injector")
        .resolve::<dyn DbHandleProvider>()
        .handle()
        .execute_values("DROP TABLE file", vec![])
        .await
        .expect("drop the file table");
}

/// The `file` table refuses writes, then Holon edits a Notes.org block.
/// Holon cannot record which bytes it would write, so it leaves the file as
/// it is and says so on the file, naming the file table.
#[test]
fn a_write_back_the_file_table_cannot_record_is_not_written_and_says_why() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KEEPER).await;
        save_and_settle(&env, "Notes.org", NOTES_ORG).await;
        wait_for_row(&env, "block:notes-1").await;
        let before = read_file(&env, "Notes.org").await;
        drop_the_file_table(&env).await;
        env.update_block_content("block:notes-1", "Edited after the drop")
            .await
            .expect("edit notes-1 in Holon");
        tokio::time::sleep(Duration::from_millis(2000)).await;
        assert_eq!(
            read_file(&env, "Notes.org").await,
            before,
            "the edit reached disk with no record of the bytes written"
        );
        let conditions = conditions_on_file(&env, "Notes.org");
        assert!(
            conditions.iter().any(|(kind, reason)| {
                *kind == holon_api::ConditionKind::WRITEBACK_DEGRADED && reason.contains("file row")
            }),
            "the unwritten edit is not disclosed with its cause: {conditions:?}"
        );
        let refused = ingest_refusals(&env);
        assert!(
            !refused.iter().any(|path| path.ends_with("/Notes.org")),
            "the store-side failure is disclosed as a failed ingest: {refused:?}"
        );
        if let Err(e) = env.stop_app().await {
            assert!(format!("{e:#}").contains("Notes.org"), "stop_app: {e:#}");
        }
    });
}

const NOTES_WITH_A_COPY: &str = "\
#+TITLE: Notes
#+ID: notes
* Note one
:PROPERTIES:
:ID: notes-1
:END:
* Q1W  q9
:PROPERTIES:
:ID: bulk-0-0
:END:
";

/// Notes.org is read, the user pastes a copy of a DayPage block into it, and
/// Holon edits a Notes.org block. While Holon is closed the user restores the
/// bytes from before the paste. The next boot reads them.
#[test]
fn a_restore_of_pre_paste_bytes_while_closed_is_read_at_boot() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KEEPER).await;
        save_and_settle(&env, "Notes.org", NOTES_ORG).await;
        wait_for_row(&env, "block:notes-1").await;
        let before_the_paste = read_file(&env, "Notes.org").await;
        save_and_settle(&env, "Notes.org", NOTES_WITH_A_COPY).await;
        env.update_block_content("block:notes-1", "Note edited in Holon")
            .await
            .expect("edit notes-1 in Holon");
        eventually_on_disk(&env, "Notes.org", "Note edited in Holon").await;
        tokio::time::sleep(Duration::from_millis(800)).await;
        env.stop_app().await.expect("stop_app");
        env.write_org_file("Notes.org", &before_the_paste)
            .await
            .expect("restore the bytes from before the paste");
        env.start_app(true).await.expect("restart");
        wait_for_row(&env, "block:day-keeps").await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            column_of(&env, "block:notes-1", "content").await,
            "Note one",
            "the boot did not read the restore"
        );
        assert_eq!(
            read_file(&env, "Notes.org").await,
            before_the_paste,
            "the boot wrote over the restore"
        );
        env.stop_app().await.expect("stop_app");
    });
}

async fn file_row_hash(env: &TestEnvironment, name: &str) -> String {
    let id = holon_api::EntityUri::file(name);
    let rows = env
        .query_sql(&format!(
            "SELECT content_hash AS v FROM file WHERE id = '{}'",
            id.as_str()
        ))
        .await
        .unwrap_or_else(|e| panic!("query the file row of {name}: {e:#}"));
    match rows.first().and_then(|r| r.get("v").cloned()) {
        Some(holon_api::Value::String(s)) => s,
        other => panic!("the file row of {name} holds no hash: {other:?}"),
    }
}

/// Holon writes an edit to Notes.org: the file row holds no hash while the
/// write is unrecorded, and the hash once the sync is idle.
#[test]
fn the_hash_of_a_write_back_is_recorded_when_the_sync_is_idle() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KEEPER).await;
        save_and_settle(&env, "Notes.org", NOTES_ORG).await;
        wait_for_row(&env, "block:notes-1").await;
        env.update_block_content("block:notes-1", "Note edited in Holon")
            .await
            .expect("edit notes-1 in Holon");
        eventually_on_disk(&env, "Notes.org", "Note edited in Holon").await;
        assert_eq!(
            file_row_hash(&env, "Notes.org").await,
            "",
            "the file row holds a hash before the write is recorded"
        );
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while file_row_hash(&env, "Notes.org").await.is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the idle sync did not record the hash of the bytes written"
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        env.stop_app().await.expect("stop_app");
    });
}

/// `subject` and reason of every written-files-unrecorded condition.
fn unrecorded_writes(env: &TestEnvironment) -> Vec<String> {
    env.injector()
        .expect("injector")
        .resolve::<Arc<holon_api::ConditionBus>>()
        .current()
        .into_iter()
        .filter(|c| c.reason.condition_kind() == holon_api::ConditionKind::WRITTEN_FILES_UNRECORDED)
        .map(|c| format!("{} {:?}", c.subject, c.reason))
        .collect()
}

/// Holon wrote an edit to Notes.org, then the `file` table refuses writes.
/// The idle sync cannot record the bytes written and says so on the vault,
/// naming Notes.org.
#[test]
fn written_bytes_the_file_table_cannot_record_are_disclosed() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let mut env = started_with_day_page(rt, DAY_PAGE_WITH_KEEPER).await;
        save_and_settle(&env, "Notes.org", NOTES_ORG).await;
        wait_for_row(&env, "block:notes-1").await;
        env.update_block_content("block:notes-1", "Note edited in Holon")
            .await
            .expect("edit notes-1 in Holon");
        eventually_on_disk(&env, "Notes.org", "Note edited in Holon").await;
        drop_the_file_table(&env).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while unrecorded_writes(&env).is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the unrecorded write is not disclosed"
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let shown = unrecorded_writes(&env);
        assert!(
            shown.iter().any(|c| c.contains("Notes.org")),
            "the disclosure does not name Notes.org: {shown:?}"
        );
        env.stop_app().await.expect("stop_app");
    });
}
