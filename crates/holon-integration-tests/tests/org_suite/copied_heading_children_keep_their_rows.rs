//! An org ingest under a heading another file holds a copy of orders only
//! blocks that keep a row.
//!
//! `DayPage.org` owns a heading; `Archive.org` holds a copy of it with some of
//! its children. More children are added to the owner after the paste. Then
//! either the owner's file drops some child lines (a child the copy holds
//! stays, one it does not hold is deleted), or the store loses a child's row
//! while the app is stopped (the file re-creates it on the next ingest).
//!
//! @pbt kind harness
//! @pbt covers cross-doc-membership(ingest) — the sibling order an ingest
//! writes holds only blocks with a row: a deleted unheld child leaves it, a
//! lost child is re-created, and the file is never refused
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone reaches this
//! only through a long copy sequence, and never removes a row while stopped

use std::sync::Arc;
use std::time::Duration;

use holon_integration_tests::TestEnvironment;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;

const PARENT: &str = "cp-parent";

/// The copied heading's children in the owner's file order. `held[i]`: the
/// copy holds child `i` (it existed at the paste); the others are added to the
/// owner after the paste. `gone`: the children the step removes.
#[derive(Debug, Clone)]
struct CopyCase {
    held: Vec<bool>,
    gone: Vec<usize>,
    step: Step,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    /// The owner's file drops the `gone` lines.
    DropLines,
    /// The store loses the `gone` rows while the app is stopped, and the
    /// owner's file is edited before the next boot.
    LoseRows,
}

impl CopyCase {
    /// The children the owner keeps, in file order.
    fn kept(&self) -> Vec<usize> {
        (0..self.held.len())
            .filter(|i| match self.step {
                Step::DropLines => self.held[*i] || !self.gone.contains(i),
                Step::LoseRows => true,
            })
            .collect()
    }

    /// The step deletes a child under the copied heading that the copy does
    /// not hold.
    fn deletes_an_unheld_child(&self) -> bool {
        self.step == Step::DropLines && self.gone.iter().any(|i| !self.held[*i])
    }
}

fn copy_case() -> impl proptest::strategy::Strategy<Value = CopyCase> {
    use proptest::prelude::*;
    (2usize..=6)
        .prop_flat_map(|n| {
            (
                proptest::collection::vec(proptest::bool::weighted(0.5), n),
                proptest::collection::vec(proptest::bool::weighted(0.6), n),
                0..n,
                proptest::bool::weighted(0.4),
            )
        })
        .prop_filter("a dropped line set is not empty", |(_, mask, _, lose)| {
            *lose || mask.iter().any(|g| *g)
        })
        .prop_map(|(held, mask, lost, lose)| {
            let n = held.len();
            if lose {
                CopyCase {
                    held,
                    gone: vec![lost],
                    step: Step::LoseRows,
                }
            } else {
                CopyCase {
                    held,
                    gone: (0..n).filter(|i| mask[*i]).collect(),
                    step: Step::DropLines,
                }
            }
        })
}

fn child(i: usize) -> String {
    format!("cp-c{i}")
}

fn heading(stars: &str, title: &str, id: &str) -> String {
    format!("{stars} {title}\n:PROPERTIES:\n:ID: {id}\n:END:\n")
}

/// `DayPage.org` with the copied heading titled `title` over `children`.
fn day_page(title: &str, children: &[usize]) -> String {
    let mut text = String::from("#+TITLE: DayPage\n#+ID: cp-daypage\n");
    text.push_str(&heading("*", title, PARENT));
    for i in children {
        text.push_str(&heading("**", &format!("Child {i}"), &child(*i)));
    }
    text.push_str(&heading("*", "Day keeps this", "cp-day-own"));
    text
}

fn archive(children: &[usize]) -> String {
    let mut text = String::from("#+TITLE: Archive\n#+ID: cp-archive\n");
    text.push_str(&heading("*", "Parent", PARENT));
    for i in children {
        text.push_str(&heading("**", &format!("Child {i}"), &child(*i)));
    }
    text.push_str(&heading("*", "Archive own line", "cp-archive-own"));
    text
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

async fn settle(env: &TestEnvironment) {
    let seq = env.org_fs.last_change_seq();
    env.wait_for_org_change_processed(seq, Duration::from_secs(20))
        .await;
    tokio::time::sleep(Duration::from_millis(800)).await;
}

async fn save_and_settle(env: &TestEnvironment, name: &str, body: &str) {
    env.write_org_file(name, body)
        .await
        .unwrap_or_else(|e| panic!("write {name}: {e:#}"));
    settle(env).await;
}

/// The children of the copied heading in `block_raw`, in `sort_key` order.
async fn stored_children(env: &TestEnvironment) -> Vec<String> {
    env.query_sql(&format!(
        "SELECT id FROM block_raw WHERE parent_id = 'block:{PARENT}' ORDER BY sort_key, id"
    ))
    .await
    .unwrap_or_else(|e| panic!("read the stored children: {e:#}"))
    .into_iter()
    .map(|r| match r.get("id") {
        Some(holon_api::Value::String(s)) => s.clone(),
        other => panic!("id of a block_raw row: {other:?}"),
    })
    .collect()
}

/// The ids of the copied heading's children in `DayPage.org`, in file order.
async fn disk_children(env: &TestEnvironment) -> Vec<String> {
    let path = env.org_root().join("DayPage.org");
    let disk = holon_filesystem::FileSystem::read_to_string(env.org_fs.as_ref(), &path)
        .await
        .unwrap_or_else(|e| panic!("read DayPage.org: {e}"));
    disk.lines()
        .filter_map(|l| l.trim().strip_prefix(":ID: "))
        .filter(|id| id.starts_with("cp-c"))
        .map(|id| format!("block:{id}"))
        .collect()
}

fn refused_files(env: &TestEnvironment) -> Vec<String> {
    env.injector()
        .expect("injector")
        .resolve::<Arc<holon_conditions::ConditionBus>>()
        .refused_files("org")
        .into_iter()
        .map(|f| format!("{}: {}", f.path, f.reason))
        .collect()
}

async fn wait_for_rows(env: &TestEnvironment, ids: &[usize]) {
    let mut stored = Vec::new();
    for _ in 0..200 {
        stored = stored_children(env).await;
        if ids
            .iter()
            .all(|i| stored.contains(&format!("block:{}", child(*i))))
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("children {ids:?} never landed: {stored:?}");
}

async fn remove_rows_while_stopped(env: &mut TestEnvironment, ids: &[usize]) {
    env.stop_app().await.expect("stop_app");
    let db = holon_turso::turso::TursoBackend::open_database(env.temp_path().join("test.db"))
        .expect("open the stopped app's database");
    let conn = db
        .connect(holon_turso::turso::ConnectOpts::new())
        .expect("connect to the stopped app's database");
    for i in ids {
        conn.execute(format!(
            "DELETE FROM block_raw WHERE id = 'block:{}'",
            child(*i)
        ))
        .unwrap_or_else(|e| panic!("remove the row of {}: {e}", child(*i)));
    }
    drop(conn);
    drop(db);
}

async fn run_case(rt: Arc<tokio::runtime::Runtime>, case: &CopyCase) -> Result<(), TestCaseError> {
    let n = case.held.len();
    let all: Vec<usize> = (0..n).collect();
    let pasted: Vec<usize> = all.iter().copied().filter(|i| case.held[*i]).collect();

    let mut env = TestEnvironment::new(rt).expect("TestEnvironment::new");
    env.set_enable_loro(false);
    env.write_org_file("DayPage.org", &day_page("Parent", &pasted))
        .await
        .expect("write DayPage.org");
    env.start_app(true).await.expect("start_app");
    wait_for_rows(&env, &pasted).await;
    settle(&env).await;
    save_and_settle(&env, "Archive.org", &archive(&pasted)).await;
    save_and_settle(&env, "DayPage.org", &day_page("Parent", &all)).await;
    wait_for_rows(&env, &all).await;

    match case.step {
        Step::DropLines => {
            let left: Vec<usize> = all
                .iter()
                .copied()
                .filter(|i| !case.gone.contains(i))
                .collect();
            save_and_settle(&env, "DayPage.org", &day_page("Parent", &left)).await;
        }
        Step::LoseRows => {
            remove_rows_while_stopped(&mut env, &case.gone).await;
            env.write_org_file("DayPage.org", &day_page("Parent edited", &all))
                .await
                .expect("edit DayPage.org while stopped");
            env.start_app(true)
                .await
                .expect("start_app after the lost rows");
            settle(&env).await;
        }
    }

    let want: Vec<String> = case
        .kept()
        .iter()
        .map(|i| format!("block:{}", child(*i)))
        .collect();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let (stored, disk) = loop {
        let stored = stored_children(&env).await;
        let disk = disk_children(&env).await;
        if (stored == want && disk == want) || std::time::Instant::now() >= deadline {
            break (stored, disk);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let refused = refused_files(&env);
    proptest::prop_assert!(
        refused.is_empty(),
        "no org file is refused or quarantined: {refused:?} ({case:?})"
    );
    proptest::prop_assert_eq!(
        &stored,
        &want,
        "block_raw holds the kept children in file order"
    );
    proptest::prop_assert_eq!(
        &disk,
        &want,
        "DayPage.org is written back with the kept children"
    );
    Ok(())
}

#[test]
fn ingest_under_a_copied_heading_orders_only_children_with_a_row() {
    let rt = runtime();
    let unheld_deletes = std::cell::Cell::new(0usize);
    let lost_rows = std::cell::Cell::new(0usize);
    let check = |case: CopyCase| rt.block_on(run_case(rt.clone(), &case));

    // A child added after the paste is deleted from the owner's file.
    check(CopyCase {
        held: vec![true, false],
        gone: vec![1],
        step: Step::DropLines,
    })
    .expect("static example: an unheld child is deleted");
    // A child the copy holds loses its row while the app is stopped.
    check(CopyCase {
        held: vec![true, true, false],
        gone: vec![1],
        step: Step::LoseRows,
    })
    .expect("static example: a held child's row is lost");

    TestRunner::new(Config {
        cases: 12,
        failure_persistence: None,
        ..Config::default()
    })
    .run(&copy_case(), |case| {
        if case.deletes_an_unheld_child() {
            unheld_deletes.set(unheld_deletes.get() + 1);
        }
        if case.step == Step::LoseRows {
            lost_rows.set(lost_rows.get() + 1);
        }
        check(case)
    })
    .expect("ingest under a copied heading");
    eprintln!(
        "draws: {} case(s) delete an unheld child, {} lose a row",
        unheld_deletes.get(),
        lost_rows.get()
    );
    assert!(
        unheld_deletes.get() > 0 && lost_rows.get() > 0,
        "the generator must draw both steps: {} unheld deletes, {} lost rows",
        unheld_deletes.get(),
        lost_rows.get()
    );
}
