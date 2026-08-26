//! Read/write-set conflict detection and cycle detection over a
//! [`CompiledNet`].
//!
//! Both analyses are over-approximate by construction (ADR 0032 §2): they
//! work at place granularity and ignore every predicate — a predicate can
//! only make a reported edge unreachable, never add one. Every finding is
//! therefore "possible", and neither analysis is model checking.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use holon_pattern::arcs::ArcPlace;
use serde::Deserialize;
use serde::Serialize;

use crate::bridge::TransitionKey;
use crate::net::Analyzability;
use crate::net::ArcOrigin;
use crate::net::CompiledNet;
use crate::net::NetTransition;

/// Transitions contending for one place, named by the keys
/// [`CompiledNet::transition`] resolves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceContention {
    pub place: ArcPlace,
    /// Transitions that write the place (produce, consume, or relocate a
    /// token in it).
    pub writers: Vec<TransitionKey>,
    /// Transitions whose enabledness reads the place without writing it.
    pub readers: Vec<TransitionKey>,
    /// True when a participant touches the place only through a guard
    /// footprint — predicate-opaque, so the contention may be vacuous.
    pub over_approximate: bool,
}

/// Which transitions contend for which places.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictReport {
    pub contentions: Vec<PlaceContention>,
    /// Transitions the analysis cannot speak for ("cannot say", never "no
    /// conflicts").
    pub unanalyzable: Vec<TransitionKey>,
}

/// A set of transitions whose produced places feed back into their own read
/// or consumed places — a possible cycle, guards not consulted. A rule
/// inhibited by its own output (the at-most-once pattern) reports here as a
/// benign self-loop through its inhibitor's footprint read.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CycleFinding {
    pub transitions: Vec<TransitionKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleReport {
    pub cycles: Vec<CycleFinding>,
    pub unanalyzable: Vec<TransitionKey>,
}

/// Pairwise read/write contention per place, over the analyzable transitions.
pub fn conflicts(net: &CompiledNet) -> ConflictReport {
    let unanalyzable = unanalyzable_keys(net);
    let mut writers: BTreeMap<&ArcPlace, BTreeSet<TransitionKey>> = BTreeMap::new();
    let mut readers: BTreeMap<&ArcPlace, BTreeSet<TransitionKey>> = BTreeMap::new();
    let mut footprint_touch: BTreeSet<(&ArcPlace, TransitionKey)> = BTreeSet::new();
    for (_, transition) in analyzable(net) {
        let key = transition.key();
        for place in transition.written_places() {
            writers.entry(place).or_default().insert(key.clone());
        }
        for place in transition.read_places() {
            readers.entry(place).or_default().insert(key.clone());
        }
        for arc in &transition.arcs {
            if arc.origin == ArcOrigin::GuardFootprint {
                footprint_touch.insert((&arc.place, key.clone()));
            }
        }
    }

    let mut contentions = Vec::new();
    for (place, place_writers) in &writers {
        let place_readers: BTreeSet<TransitionKey> = readers
            .get(place)
            .map(|r| r - place_writers)
            .unwrap_or_default();
        if place_writers.len() + place_readers.len() < 2 {
            continue;
        }
        let over_approximate = place_writers
            .iter()
            .chain(&place_readers)
            .any(|t| footprint_touch.contains(&(place, t.clone())));
        contentions.push(PlaceContention {
            place: (*place).clone(),
            writers: place_writers.iter().cloned().collect(),
            readers: place_readers.into_iter().collect(),
            over_approximate,
        });
    }
    ConflictReport {
        contentions,
        unanalyzable,
    }
}

/// Strongly connected components of the produces-into-reads graph, over the
/// analyzable transitions. Reported: components of two or more, and
/// self-loops.
pub fn cycles(net: &CompiledNet) -> CycleReport {
    let unanalyzable = unanalyzable_keys(net);
    let nodes: Vec<usize> = analyzable(net).map(|(i, _)| i).collect();
    let edges = feed_edges(net, &nodes);

    let mut cycles = Vec::new();
    for component in strongly_connected_components(&nodes, &edges) {
        let is_cycle = component.len() > 1
            || edges
                .get(&component[0])
                .is_some_and(|targets| targets.contains(&component[0]));
        if is_cycle {
            let mut transitions: Vec<TransitionKey> = component
                .into_iter()
                .map(|i| net.transitions[i].key())
                .collect();
            transitions.sort();
            cycles.push(CycleFinding { transitions });
        }
    }
    cycles.sort();
    CycleReport {
        cycles,
        unanalyzable,
    }
}

/// Edge a→b: a writes a place b's enabledness reads.
fn feed_edges(net: &CompiledNet, nodes: &[usize]) -> BTreeMap<usize, BTreeSet<usize>> {
    let mut edges: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for &a in nodes {
        let written = net.transitions[a].written_places();
        for &b in nodes {
            if !written.is_disjoint(&net.transitions[b].read_places()) {
                edges.entry(a).or_default().insert(b);
            }
        }
    }
    edges
}

fn analyzable(net: &CompiledNet) -> impl Iterator<Item = (usize, &NetTransition)> {
    net.transitions
        .iter()
        .enumerate()
        .filter(|(_, t)| t.analyzability == Analyzability::Analyzable)
}

fn unanalyzable_keys(net: &CompiledNet) -> Vec<TransitionKey> {
    net.transitions
        .iter()
        .filter(|t| !matches!(t.analyzability, Analyzability::Analyzable))
        .map(NetTransition::key)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Kosaraju over the node subset, iterative. Components come back sorted
/// internally; caller sorts the component list.
fn strongly_connected_components(
    nodes: &[usize],
    edges: &BTreeMap<usize, BTreeSet<usize>>,
) -> Vec<Vec<usize>> {
    let node_set: BTreeSet<usize> = nodes.iter().copied().collect();
    let mut reversed: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (&from, targets) in edges {
        for &to in targets {
            reversed.entry(to).or_default().insert(from);
        }
    }

    let mut finished = Vec::new();
    let mut visited = BTreeSet::new();
    for &start in nodes {
        if visited.contains(&start) {
            continue;
        }
        // Iterative DFS recording finish order: (node, children started?).
        let mut stack = vec![(start, false)];
        while let Some((node, expanded)) = stack.pop() {
            if expanded {
                finished.push(node);
                continue;
            }
            if !visited.insert(node) {
                continue;
            }
            stack.push((node, true));
            if let Some(targets) = edges.get(&node) {
                for &next in targets {
                    if !visited.contains(&next) && node_set.contains(&next) {
                        stack.push((next, false));
                    }
                }
            }
        }
    }

    let mut assigned = BTreeSet::new();
    let mut components = Vec::new();
    for &root in finished.iter().rev() {
        if assigned.contains(&root) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if !assigned.insert(node) {
                continue;
            }
            component.push(node);
            if let Some(sources) = reversed.get(&node) {
                for &prev in sources {
                    if !assigned.contains(&prev) && node_set.contains(&prev) {
                        stack.push(prev);
                    }
                }
            }
        }
        component.sort();
        components.push(component);
    }
    components
}
