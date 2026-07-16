//! Regression: an org-ingested inline `[[link]]` must round-trip its `marks`
//! through the Loro authority into `block_raw` and back onto disk. Before the
//! fix, the Loro org-ingest seam (`update_in_tree`) dropped the `marks` param
//! into generic properties instead of applying it as Peritext, so the store
//! held `marks = NULL`, write-back re-rendered the stripped label, and the
//! `[[…]]` link syntax was destroyed on disk (`inv-blocks-match-ref/org` marks
//! divergence in the composed keystone). Loro is the default authority here.
//!
//! @pbt kind harness
//! @pbt covers link-marks-roundtrip — inline [[link]] marks round-trip through
//! Loro to block_raw and disk

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("rt"),
    )
}

const ORG: &str = "* [[6Er 6Hw7]]\n:PROPERTIES:\n:ID: t1\n:END:\n";

#[test]
fn org_ingested_link_marks_survive_to_block_raw() {
    let rt = runtime();
    rt.clone().block_on(async {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("T.org", ORG)
            .build(rt.clone())
            .await
            .expect("boot");
        tokio::time::sleep(Duration::from_secs(2)).await;

        let rows = env
            .engine()
            .execute_query(
                "SELECT id, content, marks FROM block_raw WHERE id = 'block:t1'".to_string(),
                HashMap::new(),
                None,
            )
            .await
            .expect("query");
        assert_eq!(rows.len(), 1, "block t1 must exist in block_raw");
        let content = rows[0].get("content").cloned().unwrap_or(Value::Null);
        let marks = rows[0].get("marks").cloned().unwrap_or(Value::Null);
        // The rendered label is stored in `content`; the link mark must persist
        // in the `marks` column (NULL = the dropped-mark data-loss regression).
        assert_eq!(content, Value::String("6Er 6Hw7".to_string()));
        assert!(
            matches!(&marks, Value::String(s) if s.contains("Link")),
            "org-ingested [[link]] mark dropped: block_raw.marks={marks:?} (expected a Link span \
             JSON)"
        );
    });
}
