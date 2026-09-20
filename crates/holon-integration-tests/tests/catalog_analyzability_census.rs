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

use std::collections::BTreeMap;
use std::sync::Arc;

use holon_integration_tests::TestEnvironmentBuilder;
use holon_net::Analyzability;
use holon_net::guards::ResidueCause;

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
    "op:block.create",
    "op:block.delete",
    "op:block.join_block",
    "op:block.move_block",
    "op:block.rehome_entity",
    "op:block.set_field",
    "op:block.split_block",
    "op:type.declare_type",
    "rule:block:journals::action::0",
];

/// Every transition still carrying a guard predicate the arc language cannot
/// express, named with its count. Residue is what the enabledness query
/// cannot answer for, so this set SHRINKING is the measure of progress on the
/// arc language, and this set growing is a regression.
///
/// The one entry left is a NEGATED EXISTENCE test
/// (`not block_exists("Journals/{today}")`), which needs the inhibitor rather
/// than the hop: emptiness of a place, not a second entity's attributes.
const EXPECTED_RESIDUE: &[(&str, usize)] = &[("rule:block:journals::action::0", 1)];

/// Residue by CAUSE, so the totals say WHICH language gap is costing what.
/// `negation` is the one the next increment closes: there is no De Morgan
/// pass, so a negated conjunct is opaque whatever it wraps.
const EXPECTED_RESIDUE_BY_CAUSE: &[(&str, usize)] = &[("negation", 1)];

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
        let mut residue: Vec<(String, usize)> = Vec::new();
        let mut by_cause: BTreeMap<&'static str, usize> = BTreeMap::new();
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
            if !transition.residue.is_empty() {
                residue.push((
                    transition.key().as_str().to_string(),
                    transition.residue.len(),
                ));
                for entry in &transition.residue {
                    *by_cause
                        .entry(ResidueCause::of(&entry.predicate).as_str())
                        .or_default() += 1;
                }
            }
            rows.push(format!(
                "[census] {:<46} {:<34} arcs={:<3} residue={}",
                transition.key().as_str(),
                verdict,
                transition.arcs().len(),
                transition.residue.len()
            ));
        }
        rows.sort();
        analyzable.sort();
        residue.sort();
        for row in &rows {
            eprintln!("{row}");
        }

        let total = net.transitions.len();
        eprintln!(
            "[census] TOTALS transitions={total} analyzable={} unanalyzable={} \
             residue_predicates={} transitions_with_residue={}",
            analyzable.len(),
            total - analyzable.len(),
            residue.iter().map(|(_, n)| n).sum::<usize>(),
            residue.len()
        );
        for (cause, count) in &by_cause {
            eprintln!("[census] residue cause {cause:<16} {count}");
        }

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
        let by_cause: Vec<(&str, usize)> = by_cause.into_iter().collect();
        assert_eq!(
            by_cause, EXPECTED_RESIDUE_BY_CAUSE,
            "the residue causes moved. Each cause is one gap in the arc language, so a cause \
             emptying is progress and this constant is then stale; a cause appearing means a \
             guard shape stopped compiling."
        );
        let expected_residue: Vec<(String, usize)> = EXPECTED_RESIDUE
            .iter()
            .map(|(key, n)| ((*key).to_string(), *n))
            .collect();
        assert_eq!(
            residue, expected_residue,
            "the residue set moved. Residue SHRINKING is the goal — every predicate the arc \
             language learns to express leaves this set, so a smaller set means this constant \
             is stale. A GROWN set means a guard stopped compiling to arcs and the enabledness \
             query quietly went back to answering Unknown for it."
        );
    });
}
