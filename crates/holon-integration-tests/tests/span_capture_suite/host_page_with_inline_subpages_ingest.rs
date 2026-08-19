//! A hand-written page file that inlines `:Page:`-tagged child headings must
//! ingest CLEANLY: the children de-inline into their own files and the host
//! file is rewritten without them. It must NOT be quarantined as INGEST DATA
//! LOSS.
//!
//! The ingest write-back guard grounds an absent block ONLY against the file's
//! own re-projection. `render_file_by_doc_id`'s walk stops at `Page`
//! boundaries, so every child page this very ingest created is missing from the
//! host's render — read as loss, quarantined, red banner — even though the
//! store relocated each child to its own page file. The observable harm is a
//! DOUBLE-HOMED page: the child's own file is materialized while the host file
//! keeps the frozen inline copy forever.
//!
//! @pbt kind harness
//! @pbt covers host-page-inline-subpages-ingest — first ingest of a
//!   hand-authored page inlining `:Page:` children de-inlines them instead of
//!   quarantining the file

#![cfg(feature = "pbt")]

use std::sync::Arc;
use std::time::Duration;

use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("rt"),
    )
}

/// The ordinary authoring pattern: write a page, give it subpages inline.
const HOST_ORG: &str = "\
#+ID: hostpage
#+TITLE: Host Page
* Ordinary Note
:PROPERTIES:
:ID: plain-note
:END:
a note that stays in the host file
* Sub One :Page:
:PROPERTIES:
:ID: sub-one
:END:
body of sub one
* Sub Two :Page:
:PROPERTIES:
:ID: sub-two
:END:
body of sub two
* Sub Three :Page:
:PROPERTIES:
:ID: sub-three
:END:
body of sub three
";

/// (heading title, authored `:ID:`) of each inlined child page.
const CHILDREN: [(&str, &str); 3] = [
    ("Sub One", "sub-one"),
    ("Sub Two", "sub-two"),
    ("Sub Three", "sub-three"),
];

#[test]
fn hand_written_page_inlining_page_tagged_children_de_inlines_instead_of_quarantining() {
    // Installs the tracing subscriber, so the quarantine's ERROR chain is in
    // the failure output rather than only the scan verdict.
    holon_integration_tests::test_tracing::SpanCollector::global();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("Host Page.org", HOST_ORG)
            .build(rt.clone())
            .await
            .expect("boot");
        env.wait_for_cdc_quiescent(Duration::from_millis(400), Duration::from_secs(20))
            .await;
        env.wait_for_org_files_stable(300, Duration::from_secs(20))
            .await;

        // The initial scan's own verdict — the value `holon-app` turns into the
        // red degraded banner. A quarantined file reports here as a failed file,
        // and that report is PERMANENT for the boot even though a later poll
        // re-ingests the file cleanly.
        let scan_outcome = env
            .injector()
            .expect("injector")
            .resolve::<holon_orgmode::FileWatcherReadySignal>()
            .wait_ready()
            .await;
        assert!(
            scan_outcome.is_ok(),
            "the initial scan reported the hand-written host page as a failed file — the ingest \
             write-back guard read its de-inlined children as data loss and quarantined it \
             (degraded banner): {:#}",
            scan_outcome.unwrap_err(),
        );

        use holon_filesystem::FileSystem;
        let scan = FileSystem::scan_directory(env.org_fs.as_ref(), env.org_root())
            .await
            .expect("scan org dir");
        let mut dump = String::new();
        let mut host = String::new();
        let mut homed: Vec<&str> = Vec::new();
        for p in &scan.files {
            let content = FileSystem::read_to_string(env.org_fs.as_ref(), p)
                .await
                .unwrap_or_default();
            dump.push_str(&format!("\n--- {} ---\n{content}", p.display()));
            for (name, id) in CHILDREN {
                if content.contains(&format!("#+ID: {id}")) {
                    homed.push(name);
                }
            }
            if p.file_name().and_then(|s| s.to_str()) == Some("Host Page.org") {
                host = content;
            }
        }

        // Each child page relocated to its own file: the store's de-inline is
        // the intended behavior and is what makes the host's render lossless.
        let missing: Vec<&str> = CHILDREN
            .into_iter()
            .map(|(n, _)| n)
            .filter(|n| !homed.contains(n))
            .collect();
        assert!(
            missing.is_empty(),
            "child page(s) {missing:?} own no identity file — the de-inline never \
             materialized them. Files:{dump}"
        );

        // ...and the host file no longer inlines them. A quarantined host file
        // is left byte-identical to the authored source, so its inline copies
        // survive next to the child files — the DOUBLE-HOMED shape.
        let still_inlined: Vec<&str> = CHILDREN
            .into_iter()
            .map(|(n, _)| n)
            .filter(|name| host.contains(&format!("{name} :Page:")))
            .collect();
        assert!(
            still_inlined.is_empty(),
            "host file still inlines de-inlined child page(s) {still_inlined:?} — write-back was \
             refused (INGEST DATA LOSS quarantine) and the pages are now DOUBLE-HOMED. \
             Files:{dump}"
        );
    });
}
