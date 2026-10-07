//! A live `holon_sql` query block whose SQL cannot be served shows the failure
//! in the rendered page: an `error` widget carrying the engine's message, not
//! an empty result that looks like "no rows".
//!
//! One case per serving route of `BackendEngine::query_and_watch`: a shape the
//! IVM engine refuses at `CREATE MATERIALIZED VIEW`, and a shape served by
//! eager re-execution whose first read fails.
//!
//! @pbt kind harness
//! @pbt covers live-query-serve-failure-visible — a live query that fails at
//! serve time renders an error widget in the page
//! @pbt slips-if-removed a failing live query renders as an empty result and
//! no test sees it

use std::time::Duration;
use std::time::Instant;

use holon_integration_tests::pbt::frontend_slice::components::HeadlessFrontendComponent;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::SutFocusWrite;
use holon_pbt_core::capabilities::SutRenderer;

const BLOCKS_ORG: &str = "#+ID: blocks\n\
* Parent\n:PROPERTIES:\n:ID: parent\n:END:\n\
** Child\n:PROPERTIES:\n:ID: child\n:END:\n";

/// The probe shape: a recursive CTE whose anchor is `WHERE 1=1`. The fork's
/// IVM refuses a `value op value` filter when it builds the matview.
const MATVIEW_REFUSED_SQL: &str = "WITH RECURSIVE d(id) AS (
  SELECT b.id FROM block b
  WHERE 1=1
  UNION ALL
  SELECT c.id FROM block c JOIN d ON c.parent_id = d.id
)
SELECT b.*
FROM block b JOIN d ON d.id = b.id";

/// `EXISTS` routes the query to eager re-execution; the unknown column fails
/// its first read.
const EAGER_FAILING_SQL: &str = "SELECT b.* FROM block b
WHERE b.no_such_column = 'x'
  AND EXISTS (SELECT 1 FROM block_tags bt WHERE bt.block_id = b.id)";

fn query_page(sql: &str) -> String {
    format!(
        "#+ID: query-page\n* Query\n:PROPERTIES:\n:ID: query-head\n:END:\n\
         #+BEGIN_SRC holon_sql :id query-head::src::0\n{sql}\n#+END_SRC\n"
    )
}

async fn assert_failure_rendered(sql: &str, expected_message: &str) {
    let comp = HeadlessFrontendComponent::new_with_loro(
        &[
            ("blocks.org", BLOCKS_ORG),
            ("query-page.org", &query_page(sql)),
        ],
        Duration::from_millis(500),
        true,
    )
    .await;
    comp.apply_navigate_focus(CapRegion::Main, &EntityUri::block("query-page"))
        .await;

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snap = comp.widget_tree_snapshot().await;
        let errors: Vec<String> = snap
            .walk()
            .filter(|n| n.kind == "error")
            .map(|n| format!("{:?}", n.props))
            .collect();
        if errors.iter().any(|e| e.contains(expected_message)) {
            return;
        }
        if Instant::now() >= deadline {
            let head_tree = comp
                .render_tree_of(&EntityUri::parse("block:query-head").expect("id"))
                .await;
            panic!(
                "the failing live query rendered no error widget containing \
                 {expected_message:?}. Error widgets on the page: {errors:?}. Rendered kinds: \
                 {:?}\n--- render_tree_of(query-head): {head_tree:?}",
                snap.walk()
                    .map(|n| (n.kind.clone(), n.entity_id.clone()))
                    .collect::<Vec<_>>(),
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn matview_refused_live_query_renders_its_error() {
    assert_failure_rendered(
        MATVIEW_REFUSED_SQL,
        "Filter predicate must be column op value or column op column",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn eager_served_live_query_renders_its_first_read_error() {
    assert_failure_rendered(EAGER_FAILING_SQL, "no_such_column").await;
}
