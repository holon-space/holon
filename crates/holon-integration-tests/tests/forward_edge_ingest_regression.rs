//! Regression for the dogfood 2026-07-10 forward-edge ingest abort (P0
//! follow-up).
//!
//! Minimal failing corpus: a SINGLE org file whose block `flow-mode-shell`
//! carries a `:REQUIRES:`/`:BLOCKED-BY:` edge to `orient-daily-view` — a
//! sibling parsed LATER in the same file (a legal forward reference). Before
//! the fix, the `block_requires.required_id` FK rejected `flow-mode-shell`'s
//! create at COMMIT (target not yet inserted), the whole file ingest aborted,
//! and every block from `flow-mode-shell` onward (plus its parent subtree) was
//! silently dropped — the error was even MISATTRIBUTED as "parent block not
//! found: ba5ad62d" although the parent existed. The fix drops the soft TARGET
//! FK; dangling targets are a normal representable state consumed by an
//! INNER-JOIN matview.
//!
//! This test goes RED if the target FK is reintroduced (the pre-fix state) and
//! GREEN with it dropped. A/B toggle for the fix = the `required_id` FK in
//! `crates/holon-turso/sql/schema/block_requires.sql`.
//!
//! @pbt kind harness
//! @pbt covers forward-edge-ingest — forward-edge ingest abort regression
//! (dogfood 2026-07-10)

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use holon_integration_tests::TestEnvironmentBuilder;

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("error"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_test_writer()
        .try_init();
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime"),
    )
}

// One file. `flow-mode-shell` forward-references `orient-daily-view` (a later
// sibling) and `now-query-mcp` (never defined = a cross-file dangling target).
// Every :ID: here MUST land: parsed == projected.
const FORWARD_EDGE_ORG: &str = "\
* Cross-platform infrastructure
:PROPERTIES:
:ID: gpui-cross-platform
:END:
Placeholder infra body.
** Three-mode UX shells
:PROPERTIES:
:ID: ba5ad62d-bd47-47b8-873b-1b668a85e9eb
:END:
*** Capture mode overlay
:PROPERTIES:
:ID: capture-mode-overlay
:END:
Placeholder capture body.
*** Flow mode shell
:PROPERTIES:
:ID: flow-mode-shell
:BLOCKED-BY: orient-daily-view
:REQUIRES: orient-daily-view now-query-mcp
:END:
Placeholder flow body.
*** Orient daily view
:PROPERTIES:
:ID: orient-daily-view
:END:
Placeholder orient body.
";

const IDS: &[&str] = &[
    "gpui-cross-platform",
    "ba5ad62d-bd47-47b8-873b-1b668a85e9eb",
    "capture-mode-overlay",
    "flow-mode-shell",
    "orient-daily-view",
];

#[test]
fn forward_requires_edge_all_ids_land() {
    init_tracing();
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .without_loro()
            .with_org_file("Projects/Forward.org", FORWARD_EDGE_ORG)
            .build(rt.clone())
            .await
            .expect("boot");

        tokio::time::sleep(Duration::from_secs(1)).await;

        let rows = env.non_page_block_rows().await;
        let present: HashSet<String> = rows
            .iter()
            .filter_map(|r| {
                r.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.strip_prefix("block:").unwrap_or(s).to_string())
            })
            .collect();

        let missing: Vec<&&str> = IDS.iter().filter(|id| !present.contains(**id)).collect();
        eprintln!(
            "ids present {}/{}; missing {:?}",
            IDS.len() - missing.len(),
            IDS.len(),
            missing
        );
        assert!(
            missing.is_empty(),
            "block ids silently dropped by a forward :REQUIRES: edge (target-FK ingest abort): \
             {:?}",
            missing
        );
    });
}
