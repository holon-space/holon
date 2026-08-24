//! Model-based checks of the conflict and cycle analyses over generated
//! nets, plus the flagship self-inhibited-create pin.
//!
//! The models restate the definitions naively — per-place triple loops and
//! brute-force reachability — independent of the analyses' indexing and SCC
//! machinery.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use holon_net::Analyzability;
use holon_net::ArcOrigin;
use holon_net::CompiledNet;
use holon_net::Flow;
use holon_net::NetArc;
use holon_net::NetTransition;
use holon_net::TransitionSource;
use holon_net::compile::compile_rule;
use holon_net::net::UndeclaredHalf;
use holon_pattern::arcs::ArcPlace;
use proptest::prelude::*;

const PLACES: &[(&str, &str)] = &[
    ("block", "id"),
    ("block", "parent_id"),
    ("block", "content"),
    ("block", "tags"),
    ("clock", "today"),
    ("integration", "enabled"),
];

fn arb_arc() -> impl Strategy<Value = NetArc> {
    (
        0..PLACES.len(),
        prop_oneof![
            Just(Flow::Read),
            Just(Flow::Produce),
            Just(Flow::Consume),
            Just(Flow::Relocate),
        ],
        prop_oneof![
            Just(ArcOrigin::DeclaredRead),
            Just(ArcOrigin::DeclaredEmit),
            Just(ArcOrigin::GuardFootprint),
            Just(ArcOrigin::RuleEmit),
        ],
    )
        .prop_map(|(place, flow, origin)| {
            let (relation, field) = PLACES[place];
            NetArc {
                place: ArcPlace::new(relation, field),
                flow,
                origin,
                refinement: None,
                binding: None,
            }
        })
}

fn arb_transition(index: usize) -> impl Strategy<Value = NetTransition> {
    (
        prop::collection::vec(arb_arc(), 0..5),
        prop::bool::weighted(0.15),
    )
        .prop_map(move |(arcs, unanalyzable)| NetTransition {
            source: TransitionSource::Operation {
                entity: "block".to_string(),
                op: format!("op_{index}"),
            },
            analyzability: if unanalyzable {
                Analyzability::Unanalyzable {
                    undeclared: vec![UndeclaredHalf::Arcs],
                }
            } else {
                Analyzability::Analyzable
            },
            arcs,
            residue: vec![],
        })
}

fn arb_net() -> impl Strategy<Value = CompiledNet> {
    (1usize..8)
        .prop_flat_map(|n| (0..n).map(arb_transition).collect::<Vec<_>>())
        .prop_map(|transitions| CompiledNet { transitions })
}

fn writes(t: &NetTransition) -> BTreeSet<String> {
    t.arcs
        .iter()
        .filter(|a| matches!(a.flow, Flow::Produce | Flow::Relocate))
        .map(|a| a.place.to_string())
        .collect()
}

fn reads(t: &NetTransition) -> BTreeSet<String> {
    t.arcs
        .iter()
        .filter(|a| matches!(a.flow, Flow::Read | Flow::Consume | Flow::Relocate))
        .map(|a| a.place.to_string())
        .collect()
}

fn analyzable_indices(net: &CompiledNet) -> Vec<usize> {
    net.transitions
        .iter()
        .enumerate()
        .filter(|(_, t)| t.analyzability == Analyzability::Analyzable)
        .map(|(i, _)| i)
        .collect()
}

proptest! {
    /// Per-place writer/reader sets equal a naive recount, and unanalyzable
    /// transitions are listed, never silently conflict-free.
    #[test]
    fn conflict_report_equals_the_naive_recount(net in arb_net()) {
        let report = holon_net::conflicts(&net);

        let expected_unanalyzable: Vec<usize> = net
            .transitions
            .iter()
            .enumerate()
            .filter(|(_, t)| t.analyzability != Analyzability::Analyzable)
            .map(|(i, _)| i)
            .collect();
        prop_assert_eq!(&report.unanalyzable, &expected_unanalyzable);

        let mut model: BTreeMap<String, (BTreeSet<usize>, BTreeSet<usize>)> = BTreeMap::new();
        for i in analyzable_indices(&net) {
            for place in writes(&net.transitions[i]) {
                model.entry(place).or_default().0.insert(i);
            }
            for place in reads(&net.transitions[i]) {
                model.entry(place).or_default().1.insert(i);
            }
        }
        let expected: BTreeMap<String, (Vec<usize>, Vec<usize>)> = model
            .into_iter()
            .filter_map(|(place, (writers, readers))| {
                let readers: Vec<usize> = readers.difference(&writers).copied().collect();
                if writers.is_empty() || writers.len() + readers.len() < 2 {
                    return None;
                }
                Some((place, (writers.into_iter().collect(), readers)))
            })
            .collect();
        let got: BTreeMap<String, (Vec<usize>, Vec<usize>)> = report
            .contentions
            .iter()
            .map(|c| (c.place.to_string(), (c.writers.clone(), c.readers.clone())))
            .collect();
        prop_assert_eq!(got, expected);
    }

    /// The union of cycle members equals the brute-force "reaches itself"
    /// set over the produces-into-reads edges.
    #[test]
    fn cycle_members_equal_brute_force_self_reachability(net in arb_net()) {
        let report = holon_net::cycles(&net);
        let nodes = analyzable_indices(&net);

        let mut edges: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for &a in &nodes {
            for &b in &nodes {
                if !writes(&net.transitions[a]).is_disjoint(&reads(&net.transitions[b])) {
                    edges.entry(a).or_default().insert(b);
                }
            }
        }
        let mut expected = BTreeSet::new();
        for &start in &nodes {
            // Reaches itself over ≥1 edge?
            let mut frontier: Vec<usize> =
                edges.get(&start).map(|t| t.iter().copied().collect()).unwrap_or_default();
            let mut seen = BTreeSet::new();
            while let Some(node) = frontier.pop() {
                if node == start {
                    expected.insert(start);
                    break;
                }
                if seen.insert(node)
                    && let Some(targets) = edges.get(&node)
                {
                    frontier.extend(targets.iter().copied());
                }
            }
        }

        let got: BTreeSet<usize> = report
            .cycles
            .iter()
            .flat_map(|c| c.transitions.iter().copied())
            .collect();
        prop_assert_eq!(got, expected);
    }
}

/// The at-most-once journal rule: its inhibitor guard's footprint reads the
/// very places its emit produces into, so the compiled net must report a
/// self-loop — the first documented benign cycle finding.
#[test]
fn the_self_inhibited_journal_rule_reports_a_self_loop() {
    let rule = holon_rules::parse_holon_rule(
        r#"
name: daily_journal
when: 'not block_exists("Journals/{today}")'
emit:
  place: page(journals)
  name: "{today}"
"#,
    )
    .expect("the journal rule parses");
    let net = CompiledNet {
        transitions: vec![compile_rule(&holon_net::RuleSource {
            block_id: "block:rule-daily-journal".to_string(),
            rule,
        })],
    };
    let report = holon_net::cycles(&net);
    assert_eq!(
        report
            .cycles
            .iter()
            .map(|c| c.transitions.clone())
            .collect::<Vec<_>>(),
        vec![vec![0]],
        "emit produces the places the inhibitor footprint reads: {:#?}",
        net.transitions[0].arcs
    );

    let conflict = holon_net::conflicts(&net);
    assert!(
        conflict.contentions.iter().all(|c| c.writers == vec![0]),
        "a single transition cannot contend with anything else: {:#?}",
        conflict.contentions
    );
}
