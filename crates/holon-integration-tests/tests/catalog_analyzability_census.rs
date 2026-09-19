//! Which operation-catalog transitions the derived net can analyze.
//!
//! A transition is analyzable only when BOTH declaration halves are present —
//! `#[reads]`/`#[emits]` and `#[marking_delta]`. A missing half lowers
//! `Unanalyzable`, whose runtime meaning is "cannot say"
//! (`crates/holon-net/src/net.rs`), so an enabledness query can answer only
//! for the analyzable set. The printed table is the per-transition detail.
//!
//! The count is the ZERO-INTEGRATION floor. `holon_app::mcp_integrations`
//! registers one `OperationProvider` per configured MCP integration, and this
//! vault configures none; a vault with integrations connected carries more
//! transitions than the total below.
//!
//! Analyzability is a property of the catalog, not of the vault, so the
//! corpus is the smallest one that ingests.
//!
//! @pbt kind harness
//! @pbt covers catalog-analyzability-census — which catalog transitions the
//! derived net can analyze

use std::sync::Arc;

use holon_integration_tests::TestEnvironmentBuilder;
use holon_net::Analyzability;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

/// Every transition whose declarations are complete enough for the net to
/// answer. Named rather than counted, so a change says WHICH one moved.
const EXPECTED_ANALYZABLE: &[&str] = &[
    "op:block.rehome_entity",
    "op:block.set_field",
    "op:type.declare_type",
    "rule:block:journals::action::0",
];

const VAULT: &str =
    "#+TITLE: Census\n#+ID: census-doc\n* Node\n:PROPERTIES:\n:ID: census-n0\n:END:\n";

#[test]
fn catalog_analyzability_census() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("Census.org", VAULT)
            .build(rt.clone())
            .await
            .expect("boot the census vault");

        let net = env.engine().derived_net().expect("derive the net");

        let mut analyzable: Vec<String> = Vec::new();
        let mut rows: Vec<String> = Vec::new();
        for transition in &net.transitions {
            let verdict = match &transition.analyzability {
                Analyzability::Analyzable => {
                    analyzable.push(transition.key().as_str().to_string());
                    "Analyzable".to_string()
                }
                Analyzability::Unanalyzable { undeclared } => {
                    format!("Unanalyzable{undeclared:?}")
                }
            };
            rows.push(format!(
                "[census] {:<46} {:<34} arcs={:<3} residue={}",
                transition.key().as_str(),
                verdict,
                transition.arcs.len(),
                transition.residue.len()
            ));
        }
        rows.sort();
        analyzable.sort();
        for row in &rows {
            eprintln!("{row}");
        }

        let total = net.transitions.len();
        eprintln!(
            "[census] TOTALS transitions={total} analyzable={} unanalyzable={}",
            analyzable.len(),
            total - analyzable.len()
        );

        assert!(
            total > 0,
            "the census saw an EMPTY net: the app booted without a catalog, so the totals \
             above describe nothing"
        );
        assert_eq!(
            analyzable, EXPECTED_ANALYZABLE,
            "the analyzable set moved. Declaring both halves on more operations is the goal, so \
             a GROWN set means this constant is stale — widen it. A SHRUNK set means a \
             declaration was lost and the enabledness query silently stopped answering for it."
        );
    });
}
