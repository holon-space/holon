//! A vault `Journals/` folder resolves to the page the shipped
//! `Journals.org` asset declares by its `#+ID`, so the folder ingest and the
//! seed never stand up two pages titled "Journals" under the root.
//!
//! Boot order is ingest-first: the org initial scan runs before the seed. The
//! folder resolves through its companion file's `#+ID`, the same path every
//! page takes, so the companion has to be on disk when the scan meets the
//! folder.
//!
//! @pbt kind harness
//! @pbt covers seeded-page-identity(journals) — the `Journals/` folder segment
//!   resolves to the asset's page, and every block of the asset exists after an
//!   ingest-first boot

#![cfg(feature = "pbt")]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use holon_api::EntityUri;
use holon_filesystem::FileSystem;
use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("rt"),
    )
}

const ASSET: &str = include_str!("../../../../assets/default/Journals.org");

const DAY_ORG: &str = "* a bullet\n:PROPERTIES:\n:ID: jf-0\n:END:\n";

fn pinned_clock() -> Arc<holon_api::TestClock> {
    let ms = chrono::NaiveDate::from_ymd_opt(2026, 6, 15)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp_millis();
    Arc::new(holon_api::TestClock::new(ms))
}

async fn ids_of(
    env: &holon_integration_tests::TestEnvironment,
    sql: &str,
    col: &str,
) -> Vec<String> {
    env.query_sql(sql)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e:#}"))
        .iter()
        .map(|r| {
            r.get(col)
                .and_then(|v| v.as_string())
                .unwrap_or_else(|| panic!("{sql}: row without {col}: {r:?}"))
                .to_string()
        })
        .collect()
}

#[test]
fn ingest_first_boot_keeps_one_journals_page_with_its_seed_children() {
    let asset = holon_orgmode::parse_org_file(
        Path::new("Journals.org"),
        ASSET,
        &EntityUri::no_parent(),
        Path::new(""),
    )
    .expect("parse the shipped Journals.org asset");
    let page = asset.document.id.as_str().to_string();

    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_clock(pinned_clock())
            .with_org_file("Journals/2026-01-15.org", DAY_ORG)
            .build(rt.clone())
            .await
            .expect("boot");
        env.wait_for_cdc_quiescent(Duration::from_millis(400), Duration::from_secs(10))
            .await;
        env.wait_for_org_files_stable(300, Duration::from_secs(10))
            .await;

        let pages = ids_of(
            &env,
            "SELECT b.id FROM block_raw b JOIN block_tags t ON t.block_id = b.id AND t.tag = \
             'Page' WHERE b.content = 'Journals' AND b.parent_id = 'sentinel:no_parent'",
            "id",
        )
        .await;
        assert_eq!(
            pages,
            vec![page.clone()],
            "the `Journals/` folder must resolve to the page the asset declares; a page \
             position holds one page"
        );

        for block in &asset.blocks {
            let rows = ids_of(
                &env,
                &format!(
                    "SELECT id FROM block_raw WHERE id = '{}'",
                    block.id.as_str()
                ),
                "id",
            )
            .await;
            assert_eq!(
                rows.len(),
                1,
                "asset block {} ({:?}) is missing after an ingest-first boot",
                block.id,
                block.content
            );
        }

        let day_parents = ids_of(
            &env,
            "SELECT parent_id FROM block_raw b JOIN block_tags t ON t.block_id = b.id AND \
             t.tag = 'Page' WHERE b.content = '2026-01-15'",
            "parent_id",
        )
        .await;
        assert_eq!(
            day_parents,
            vec![page],
            "the ingested day page must hang under the asset's journals page"
        );
    });
}

/// The seed is keyed on the asset's `#+ID`, not its file name: a page the user
/// moved into another file is not written back as a second `Journals.org`.
#[test]
fn a_page_moved_to_another_file_gets_no_second_file_after_reboot() {
    let rt = runtime();
    rt.clone().block_on(async {
        let mut env =
            holon_integration_tests::TestEnvironment::new(rt.clone()).expect("TestEnvironment");
        env.write_org_file(
            "Diary.org",
            "#+ID: journals\n* a note\n:PROPERTIES:\n:ID: jm-0\n:END:\n",
        )
        .await
        .expect("write the moved page");
        for boot in 1..=2 {
            env.start_app(true).await.expect("start_app");
            env.wait_for_org_files_stable(300, Duration::from_secs(10))
                .await;
            assert!(
                !env.org_fs.exists(&env.org_root().join("Journals.org")),
                "boot {boot}: the seed wrote Journals.org although Diary.org already holds its \
                 `#+ID`"
            );
            env.stop_app().await.expect("stop_app");
        }
    });
}
