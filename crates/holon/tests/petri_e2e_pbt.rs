//! Invariant-based PBT for Petri net materialization & ranking.
//!
//! Instead of duplicating the materialization logic in a reference model (gold standard),
//! we check a set of structural invariants after every operation. Each invariant is
//! independently simple to verify, and together they tightly constrain the implementation.
//!
//! Generators produce diverse task content (wiki links, delegations, questions,
//! sequential deps, combined prefixes) with dynamic parent/person names.
//! Additionally, optional self blocks and prototype blocks verify that the
//! materialization layer correctly reads attributes from persistent blocks.

use chrono::Utc;
use holon::petri::{
    Engine, Executor, PrototypeValue, SelfDescriptor, TaskMarking, TaskNet,
    default_prototype_props, materialize_at, rank_tasks,
};
use holon_api::EntityUri;
use holon_api::Priority;
use holon_api::Value as HValue;
use holon_api::block::Block;
use holon_api::types::DependsOn;
use holon_engine::{Marking, NetDef, TokenState, TransitionDef};
use proptest::prelude::*;
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest, prop_state_machine};
use std::collections::{BTreeMap, BTreeSet, HashSet};

const DONE_KEYWORDS: &[&str] = &["DONE", "CANCELLED", "CLOSED"];

// ---------------------------------------------------------------------------
// Reference state (minimal — just tracks blocks, no PN duplication)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct RefBlock {
    id: String,
    parent_id: String,
    content: String,
    // Ground-truth fields from the generator (NOT re-parsed from content).
    base_text: String,
    has_sequential_dep: bool,
    executor: Executor,
    is_question: bool,
    wiki_links: Vec<String>,
    task_state: String,
    priority: Option<Priority>,
    deadline: Option<String>,
    duration_minutes: Option<i64>,
    depends_on: DependsOn,
    position: usize,
}

impl RefBlock {
    fn is_completed(&self) -> bool {
        DONE_KEYWORDS.contains(&self.task_state.as_str())
    }
}

#[derive(Clone, Debug)]
struct RefSelfBlock {
    energy: f64,
    focus: f64,
    mental_slots_capacity: i64,
}

#[derive(Clone, Debug)]
struct RefPrototypeBlock {
    properties: BTreeMap<String, PrototypeValue>,
}

#[derive(Clone, Debug)]
struct PetriRefState {
    blocks: BTreeMap<String, RefBlock>,
    self_block: Option<RefSelfBlock>,
    prototype_block: Option<RefPrototypeBlock>,
    counter: usize,
}

impl PetriRefState {
    /// Create initial state with two canary blocks that have a known weight relationship.
    /// canary_aaa (position 0, no priority) vs canary_zzz (position 1, priority 3).
    /// With working weights, priority dominates → canary_zzz ranks first.
    /// With broken weights (fallback 1.0), alphabetical ID tiebreak puts canary_aaa first → CAUGHT.
    fn with_canaries() -> Self {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "canary_aaa".to_string(),
            RefBlock {
                id: "canary_aaa".to_string(),
                parent_id: "__canary__".to_string(),
                content: "Canary low priority task".to_string(),
                base_text: "Canary low priority task".to_string(),
                has_sequential_dep: false,
                executor: Executor::SelfExec,
                is_question: false,
                wiki_links: vec![],
                task_state: "TODO".to_string(),
                priority: None,
                deadline: None,
                duration_minutes: Some(60),
                depends_on: DependsOn::default(),
                position: 0,
            },
        );
        blocks.insert(
            "canary_zzz".to_string(),
            RefBlock {
                id: "canary_zzz".to_string(),
                parent_id: "__canary__".to_string(),
                content: "Canary high priority task".to_string(),
                base_text: "Canary high priority task".to_string(),
                has_sequential_dep: false,
                executor: Executor::SelfExec,
                is_question: false,
                wiki_links: vec![],
                task_state: "TODO".to_string(),
                priority: Some(Priority::High),
                deadline: None,
                duration_minutes: Some(60),
                depends_on: DependsOn::default(),
                position: 1,
            },
        );
        Self {
            blocks,
            self_block: None,
            prototype_block: None,
            counter: 2,
        }
    }

    fn active_block_ids(&self) -> Vec<String> {
        self.blocks
            .values()
            .filter(|b| !b.is_completed())
            .map(|b| b.id.clone())
            .collect()
    }

    fn completed_block_ids(&self) -> Vec<String> {
        self.blocks
            .values()
            .filter(|b| b.is_completed())
            .map(|b| b.id.clone())
            .collect()
    }

    fn effective_prototype_props(&self) -> BTreeMap<String, PrototypeValue> {
        let engine = rhai::Engine::new();
        let mut props = default_prototype_props(&engine);
        if let Some(ref pb) = self.prototype_block {
            for (k, v) in &pb.properties {
                props.insert(k.clone(), v.clone());
            }
        }
        props
    }

    fn effective_self(&self) -> SelfDescriptor {
        match &self.self_block {
            Some(sb) => SelfDescriptor {
                energy: sb.energy,
                focus: sb.focus,
                mental_slots_capacity: sb.mental_slots_capacity,
            },
            None => SelfDescriptor::defaults(),
        }
    }
}

// ---------------------------------------------------------------------------
// Generators — shared building blocks
// ---------------------------------------------------------------------------

const TASK_STATES: &[&str] = &["TODO", "DOING", "DONE", "CANCELLED"];
const PRIORITIES: &[Priority] = &[Priority::Low, Priority::Medium, Priority::High];
const DEADLINES: &[&str] = &[
    "<2026-02-01 Sun>",
    "<2026-02-20 Fri>",
    "<2026-03-15 Sun>",
    "<2026-04-01 Wed>",
    "<2026-06-15 Mon>",
    "<2026-12-31 Thu>",
];

fn gen_task_state() -> BoxedStrategy<String> {
    prop::sample::select(
        TASK_STATES
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
    )
    .boxed()
}

fn gen_priority() -> BoxedStrategy<Option<Priority>> {
    prop::option::of(prop::sample::select(PRIORITIES.to_vec())).boxed()
}

fn gen_deadline() -> BoxedStrategy<Option<String>> {
    prop::option::of(prop::sample::select(
        DEADLINES.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    ))
    .boxed()
}

fn gen_id_from(ids: &[String]) -> Option<BoxedStrategy<String>> {
    if ids.is_empty() {
        None
    } else {
        Some(prop::sample::select(ids.to_vec()).boxed())
    }
}

// ---------------------------------------------------------------------------
// Generators — transition strategies
// ---------------------------------------------------------------------------

impl PetriRefState {
    fn gen_create_block(&self) -> BoxedStrategy<PetriTransition> {
        let counter = self.counter;
        let all_ids: Vec<String> = self.blocks.keys().cloned().collect();

        let parent_strat = if all_ids.is_empty() {
            prop::sample::select(vec![
                "parent_0".to_string(),
                "parent_1".to_string(),
                "parent_2".to_string(),
            ])
            .boxed()
        } else {
            let mut parents: Vec<String> = all_ids.iter().take(5).cloned().collect();
            parents.push("parent_0".to_string());
            parents.push("parent_1".to_string());
            parents.push(format!("parent_{counter}"));
            parents.dedup();
            prop::sample::select(parents).boxed()
        };

        let dep_strat = match gen_id_from(&all_ids) {
            Some(s) => prop::option::of(s)
                .prop_map(|opt| opt.map(|id| DependsOn::from(vec![id])).unwrap_or_default())
                .boxed(),
            None => Just(DependsOn::default()).boxed(),
        };

        (
            parent_strat,
            structured_content_strategy(counter),
            gen_task_state(),
            gen_priority(),
            gen_deadline(),
            prop::option::of(1i64..240),
            dep_strat,
        )
            .prop_map(
                move |(
                    parent_id,
                    (content, base_text, has_sequential_dep, executor, is_question, wiki_links),
                    task_state,
                    priority,
                    deadline,
                    duration,
                    dep,
                )| {
                    PetriTransition::CreateBlock {
                        id: format!("blk-{counter}"),
                        parent_id,
                        content,
                        base_text,
                        has_sequential_dep,
                        executor,
                        is_question,
                        wiki_links,
                        task_state,
                        priority,
                        deadline,
                        duration,
                        depends_on: dep.clone(),
                    }
                },
            )
            .boxed()
    }

    fn gen_update_block(&self) -> Option<BoxedStrategy<PetriTransition>> {
        let all_ids: Vec<String> = self.blocks.keys().cloned().collect();
        let id_strat = gen_id_from(&all_ids)?;
        Some(
            (
                id_strat,
                gen_priority(),
                gen_deadline(),
                prop::option::of(gen_task_state()),
            )
                .prop_map(
                    |(id, priority, deadline, state_val)| PetriTransition::UpdateBlock {
                        id,
                        new_priority: priority,
                        new_deadline: deadline,
                        new_state: state_val,
                    },
                )
                .boxed(),
        )
    }

    fn gen_delete_block(&self) -> Option<BoxedStrategy<PetriTransition>> {
        let id_strat = gen_id_from(&self.active_block_ids())?;
        Some(
            id_strat
                .prop_map(|id| PetriTransition::DeleteBlock { id })
                .boxed(),
        )
    }

    fn gen_set_self_block() -> BoxedStrategy<PetriTransition> {
        (
            prop::sample::select(vec![0.3, 0.5, 0.7, 1.0]),
            prop::sample::select(vec![0.2, 0.5, 0.8, 1.0]),
            prop::sample::select(vec![3i64, 5, 7, 10]),
        )
            .prop_map(|(energy, focus, cap)| PetriTransition::SetSelfBlock {
                energy,
                focus,
                mental_slots_capacity: cap,
            })
            .boxed()
    }

    fn gen_set_prototype() -> BoxedStrategy<PetriTransition> {
        (
            prop::sample::select(vec![30.0, 60.0, 90.0, 120.0]),
            prop::sample::select(vec![1.0, 3.0, 5.0, 7.0]),
            prop::sample::select(vec![50.0, 100.0, 200.0, 500.0]),
        )
            .prop_map(|(dur, buf, pen)| PetriTransition::SetPrototype {
                default_duration_minutes: dur,
                deadline_buffer_days: buf,
                deadline_penalty: pen,
            })
            .boxed()
    }

    fn gen_fire_transition(&self) -> Option<BoxedStrategy<PetriTransition>> {
        if self.blocks.is_empty() {
            return None;
        }
        let blocks = PetriSUT::rebuild_blocks(self);
        let prototype_props = self.effective_prototype_props();
        let self_desc = self.effective_self();
        let (net, marking) = materialize_at(&blocks, &self_desc, &prototype_props, Utc::now());
        let engine = Engine::new();
        let enabled = engine.enabled(&net, &marking);
        let ranked = engine.rank(&net, &marking, &enabled);
        let ranked_ids: Vec<String> = ranked
            .iter()
            .map(|r| r.binding.transition_id.clone())
            .filter(|id| !id.ends_with("_delegate"))
            .collect();
        let id_strat = gen_id_from(&ranked_ids)?;
        Some(
            id_strat
                .prop_map(|id| PetriTransition::FireTransition { transition_id: id })
                .boxed(),
        )
    }
}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
#[allow(clippy::enum_variant_names)]
enum PetriTransition {
    CreateBlock {
        id: String,
        parent_id: String,
        content: String,
        base_text: String,
        has_sequential_dep: bool,
        executor: Executor,
        is_question: bool,
        wiki_links: Vec<String>,
        task_state: String,
        priority: Option<Priority>,
        deadline: Option<String>,
        duration: Option<i64>,
        depends_on: DependsOn,
    },
    UpdateBlock {
        id: String,
        new_priority: Option<Priority>,
        new_deadline: Option<String>,
        new_state: Option<String>,
    },
    DeleteBlock {
        id: String,
    },
    SetSelfBlock {
        energy: f64,
        focus: f64,
        mental_slots_capacity: i64,
    },
    SetPrototype {
        default_duration_minutes: f64,
        deadline_buffer_days: f64,
        deadline_penalty: f64,
    },
    /// Fire a specific PN transition by ID and verify invariants.
    /// The ID is chosen by the generator from the ranked enabled set.
    FireTransition {
        transition_id: String,
    },
}

// ---------------------------------------------------------------------------
// Reference state machine
// ---------------------------------------------------------------------------

impl ReferenceStateMachine for PetriRefState {
    type State = Self;
    type Transition = PetriTransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        Just(PetriRefState::with_canaries()).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        let mut strategies: Vec<BoxedStrategy<PetriTransition>> = vec![
            state.gen_create_block(),
            PetriRefState::gen_set_self_block(),
            PetriRefState::gen_set_prototype(),
        ];
        strategies.extend(state.gen_update_block());
        strategies.extend(state.gen_delete_block());
        strategies.extend(state.gen_fire_transition());

        prop::strategy::Union::new(strategies).boxed()
    }

    fn apply(state: Self::State, transition: &Self::Transition) -> Self::State {
        let mut state = state;
        match transition {
            PetriTransition::CreateBlock {
                id,
                parent_id,
                content,
                base_text,
                has_sequential_dep,
                executor,
                is_question,
                wiki_links,
                task_state,
                priority,
                deadline,
                duration,
                depends_on,
            } => {
                let position = state.counter;
                state.blocks.insert(
                    id.clone(),
                    RefBlock {
                        id: id.clone(),
                        parent_id: parent_id.clone(),
                        content: content.clone(),
                        base_text: base_text.clone(),
                        has_sequential_dep: *has_sequential_dep,
                        executor: executor.clone(),
                        is_question: *is_question,
                        wiki_links: wiki_links.clone(),
                        task_state: task_state.clone(),
                        priority: *priority,
                        deadline: deadline.clone(),
                        duration_minutes: *duration,
                        depends_on: depends_on.clone(),
                        position,
                    },
                );
                state.counter += 1;
            }
            PetriTransition::UpdateBlock {
                id,
                new_priority,
                new_deadline,
                new_state,
            } => {
                if let Some(block) = state.blocks.get_mut(id) {
                    if let Some(p) = new_priority {
                        block.priority = Some(*p);
                    }
                    if let Some(d) = new_deadline {
                        block.deadline = Some(d.clone());
                    }
                    if let Some(s) = new_state {
                        block.task_state = s.clone();
                    }
                }
            }
            PetriTransition::DeleteBlock { id } => {
                state.blocks.remove(id);
            }
            PetriTransition::SetSelfBlock {
                energy,
                focus,
                mental_slots_capacity,
            } => {
                state.self_block = Some(RefSelfBlock {
                    energy: *energy,
                    focus: *focus,
                    mental_slots_capacity: *mental_slots_capacity,
                });
            }
            PetriTransition::SetPrototype {
                default_duration_minutes,
                deadline_buffer_days,
                deadline_penalty,
            } => {
                let mut props = BTreeMap::new();
                props.insert(
                    "default_duration_minutes".to_string(),
                    PrototypeValue::Literal(*default_duration_minutes),
                );
                props.insert(
                    "deadline_buffer_days".to_string(),
                    PrototypeValue::Literal(*deadline_buffer_days),
                );
                props.insert(
                    "deadline_penalty".to_string(),
                    PrototypeValue::Literal(*deadline_penalty),
                );
                state.prototype_block = Some(RefPrototypeBlock { properties: props });
            }
            PetriTransition::FireTransition { .. } => {}
        }
        state
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        match transition {
            PetriTransition::CreateBlock { .. } => true,
            PetriTransition::UpdateBlock { id, .. } => {
                state.blocks.contains_key(id) && !id.starts_with("canary_")
            }
            PetriTransition::DeleteBlock { id } => {
                state.blocks.contains_key(id) && !id.starts_with("canary_")
            }
            PetriTransition::SetSelfBlock { .. } => true,
            PetriTransition::SetPrototype { .. } => true,
            PetriTransition::FireTransition { .. } => !state.blocks.is_empty(),
        }
    }
}

// ---------------------------------------------------------------------------
// Structured content generator — generates components then serializes
// ---------------------------------------------------------------------------

fn executor_strategy(counter: usize) -> BoxedStrategy<Executor> {
    let person_idx = counter % 10;
    prop::sample::select(vec![
        Executor::SelfExec,
        Executor::SelfExec,
        Executor::Delegated {
            person: format!("Person_{person_idx}"),
        },
        Executor::Delegated {
            person: "Team_Lead".into(),
        },
        Executor::Delegated { person: "M".into() },
    ])
    .boxed()
}

fn wiki_links_strategy(counter: usize) -> BoxedStrategy<Vec<String>> {
    let doc_idx = counter % 8;
    prop::sample::select(vec![
        vec![],
        vec![format!("Doc_{doc_idx}")],
        vec![format!("People/Person_{}", counter % 10)],
        vec![format!("Doc_{doc_idx}"), format!("Report_{counter}")],
    ])
    .boxed()
}

/// Build base_text with wiki links embedded inline, matching real org content.
fn format_base_with_links(counter: usize, links: &[String]) -> String {
    match links.len() {
        0 => format!("Do task number {counter}"),
        1 => {
            let l = &links[0];
            let templates: &[fn(&str, usize) -> String] = &[
                |l, _| format!("Read [[{l}]] carefully"),
                |l, _| format!("Ask [[{l}]] about status"),
                |l, c| format!("Update [[{l}]] before release {c}"),
                |l, _| format!("Check [[{l}]] and report back"),
            ];
            templates[counter % templates.len()](l, counter)
        }
        _ => {
            let a = &links[0];
            let b = &links[1];
            format!("Compare [[{a}]] with [[{b}]] for review")
        }
    }
}

fn serialize_content(has_seq: bool, exec: &Executor, is_question: bool, base_text: &str) -> String {
    let mut parts = Vec::new();
    if has_seq {
        parts.push(">".to_string());
    }
    if let Executor::Delegated { person } = exec {
        parts.push(format!("@[[{person}]]:"));
    }
    if is_question {
        parts.push("?".to_string());
    }
    parts.push(base_text.to_string());
    parts.join(" ")
}

/// Structured content: independently generates prefixes/links, then serializes.
/// Returns (content, base_text, has_sequential_dep, executor, is_question, wiki_links).
fn structured_content_strategy(
    counter: usize,
) -> BoxedStrategy<(String, String, bool, Executor, bool, Vec<String>)> {
    (
        prop::bool::ANY,
        executor_strategy(counter),
        prop::bool::ANY,
        wiki_links_strategy(counter),
    )
        .prop_map(move |(has_seq, exec, is_question, links)| {
            let base_text = format_base_with_links(counter, &links);
            let content = serialize_content(has_seq, &exec, is_question, &base_text);
            (content, base_text, has_seq, exec, is_question, links)
        })
        .boxed()
}

// ---------------------------------------------------------------------------
// Invariant checks
// ---------------------------------------------------------------------------

fn check_all_invariants(
    ref_state: &PetriRefState,
    net: &TaskNet,
    marking: &TaskMarking,
    self_desc: &SelfDescriptor,
    prototype_props: &BTreeMap<String, PrototypeValue>,
) {
    check_self_token(ref_state, marking, self_desc);
    check_no_duplicate_token_ids(marking);
    check_no_duplicate_transition_ids(net);
    check_active_task_transition_bijection(ref_state, net);
    check_completed_tasks(ref_state, net, marking);
    check_transition_labels_clean(ref_state, net);
    check_transition_durations(ref_state, net, prototype_props);
    check_dependency_arcs(ref_state, net);
    check_wiki_link_tokens(ref_state, marking);
    check_delegation(ref_state, net, marking);
    check_question_knowledge(ref_state, net);
    check_output_arcs(net);
    check_net_constraints(net, marking);
}

fn check_engine_invariants(ref_state: &PetriRefState, net: &TaskNet, marking: &TaskMarking) {
    let engine = Engine::new();
    let enabled = engine.enabled(net, marking);
    let ranked = engine.rank(net, marking, &enabled);

    let enabled_ids: BTreeSet<String> = enabled.iter().map(|b| b.transition_id.clone()).collect();

    // Ranked transitions must be a subset of enabled
    for rt in &ranked {
        assert!(
            enabled_ids.contains(&rt.binding.transition_id),
            "Ranked transition {} not in enabled set",
            rt.binding.transition_id
        );
    }

    // Ranked must be sorted by delta_per_minute (descending)
    for pair in ranked.windows(2) {
        assert!(
            pair[0].delta_per_minute >= pair[1].delta_per_minute,
            "Ranking not sorted: {} ({}) before {} ({})",
            pair[0].binding.transition_id,
            pair[0].delta_per_minute,
            pair[1].binding.transition_id,
            pair[1].delta_per_minute,
        );
    }

    // Tasks with unmet dependencies must NOT be enabled
    let completed_ids: BTreeSet<String> = ref_state.completed_block_ids().into_iter().collect();
    for block in ref_state.blocks.values() {
        if block.is_completed() {
            continue;
        }
        // Check explicit depends_on (all deps must be completed)
        for dep_id in block.depends_on.iter() {
            if !completed_ids.contains(dep_id) {
                assert!(
                    !enabled_ids.contains(&block.id),
                    "Task {} is enabled but depends on incomplete {}",
                    block.id,
                    dep_id
                );
            }
        }

        // Check sequential deps: find previous sibling
        if block.has_sequential_dep {
            let prev_sibling = find_previous_sibling(ref_state, block);
            if let Some(prev_id) = prev_sibling {
                if !completed_ids.contains(&prev_id) {
                    assert!(
                        !enabled_ids.contains(&block.id),
                        "Task {} is enabled but sequential dep {} is incomplete",
                        block.id,
                        prev_id
                    );
                }
            }
        }
    }
}

/// Find the previous sibling (same parent, position just before this block).
fn find_previous_sibling(ref_state: &PetriRefState, block: &RefBlock) -> Option<String> {
    let mut siblings: Vec<&RefBlock> = ref_state
        .blocks
        .values()
        .filter(|b| b.parent_id == block.parent_id && b.position < block.position)
        .collect();
    siblings.sort_by_key(|b| b.position);
    siblings.last().map(|b| b.id.clone())
}

// --- Individual invariant functions ---

fn check_self_token(ref_state: &PetriRefState, marking: &TaskMarking, self_desc: &SelfDescriptor) {
    let self_tok = marking.token("self").expect("self token must always exist");
    assert_eq!(self_tok.id(), "self", "self token id() must return 'self'");
    assert_eq!(
        self_tok.token_type(),
        "person",
        "self token must be type person"
    );

    let cap = self_tok.get("mental_slots_capacity");
    assert_eq!(
        cap,
        Some(&holon_engine::value::Value::Int(
            self_desc.mental_slots_capacity
        )),
        "mental_slots_capacity must match self descriptor"
    );

    let energy = self_tok.get("energy");
    assert_eq!(
        energy,
        Some(&holon_engine::value::Value::Float(self_desc.energy)),
        "energy must match self descriptor"
    );

    let focus = self_tok.get("focus");
    assert_eq!(
        focus,
        Some(&holon_engine::value::Value::Float(self_desc.focus)),
        "focus must match self descriptor"
    );

    // mental_slots_occupied must match DOING count
    let doing_count = ref_state
        .blocks
        .values()
        .filter(|b| b.task_state == "DOING")
        .count() as i64;
    let occupied = self_tok.get("mental_slots_occupied");
    assert_eq!(
        occupied,
        Some(&holon_engine::value::Value::Int(doing_count)),
        "mental_slots_occupied must match DOING task count"
    );
}

fn check_no_duplicate_token_ids(marking: &TaskMarking) {
    let mut seen = HashSet::new();
    for token in marking.tokens() {
        let id = token.id();
        assert!(seen.insert(id.to_string()), "Duplicate token ID: {}", id);
    }
}

fn check_no_duplicate_transition_ids(net: &TaskNet) {
    let mut seen = HashSet::new();
    for t in net.transitions() {
        let id = t.id();
        assert!(
            seen.insert(id.to_string()),
            "Duplicate transition ID: {}",
            id
        );
    }
}

fn check_active_task_transition_bijection(ref_state: &PetriRefState, net: &TaskNet) {
    let active_ids: BTreeSet<String> = ref_state.active_block_ids().into_iter().collect();
    let transition_ids: BTreeSet<String> = net
        .transitions()
        .filter(|t| !t.id().ends_with("_delegate"))
        .map(|t| t.id().to_string())
        .collect();

    assert_eq!(
        active_ids,
        transition_ids,
        "Active tasks and non-delegate transitions must be in bijection.\n\
         Active only: {:?}\nTransitions only: {:?}",
        active_ids.difference(&transition_ids).collect::<Vec<_>>(),
        transition_ids.difference(&active_ids).collect::<Vec<_>>(),
    );
}

fn check_completed_tasks(ref_state: &PetriRefState, net: &TaskNet, marking: &TaskMarking) {
    for id in ref_state.completed_block_ids() {
        // No transition for completed tasks
        assert!(
            net.transition(&id).is_none(),
            "Completed task {} should not have a transition",
            id
        );

        // Completion token must exist
        let completion_id = format!("completed_{id}");
        assert!(
            marking.token(&completion_id).is_some(),
            "Completed task {} must have completion token {}",
            id,
            completion_id
        );
    }
}

fn check_transition_labels_clean(ref_state: &PetriRefState, net: &TaskNet) {
    for block in ref_state.blocks.values() {
        if block.is_completed() {
            continue;
        }
        let transition = net
            .transition(&block.id)
            .unwrap_or_else(|| panic!("Missing transition for active task {}", block.id));

        // The label should be the base_text (prefixes already stripped by generator)
        let expected_label = block.base_text.lines().next().unwrap_or("");
        assert_eq!(
            transition.label, expected_label,
            "Transition {} label mismatch: got {:?}, expected {:?}",
            block.id, transition.label, expected_label
        );
    }
}

fn check_transition_durations(
    ref_state: &PetriRefState,
    net: &TaskNet,
    prototype_props: &BTreeMap<String, PrototypeValue>,
) {
    let default_duration = prototype_props
        .get("default_duration_minutes")
        .and_then(|v| v.as_literal())
        .unwrap_or(60.0);

    for block in ref_state.blocks.values() {
        if block.is_completed() {
            continue;
        }
        let transition = net.transition(&block.id).unwrap();
        let expected = block
            .duration_minutes
            .map(|m| m as f64)
            .unwrap_or(default_duration);
        assert!(
            (transition.duration_minutes() - expected).abs() < f64::EPSILON,
            "Task {} duration: got {}, expected {}",
            block.id,
            transition.duration_minutes(),
            expected
        );
    }
}

fn check_dependency_arcs(ref_state: &PetriRefState, net: &TaskNet) {
    for block in ref_state.blocks.values() {
        if block.is_completed() {
            continue;
        }
        let transition = net.transition(&block.id).unwrap();

        // Check explicit depends_on (every dep ID must have a corresponding arc)
        for dep_id in block.depends_on.iter() {
            let has_dep_arc = transition.inputs().iter().any(|i| {
                i.token_type == "completion"
                    && i.precond.get("source_task").map(|v| v.as_str()) == Some(dep_id.as_str())
            });
            assert!(
                has_dep_arc,
                "Task {} should have dependency arc for {}",
                block.id, dep_id
            );
        }

        // Check sequential dep
        if block.has_sequential_dep {
            if let Some(prev_id) = find_previous_sibling(ref_state, block) {
                let has_seq_arc = transition.inputs().iter().any(|i| {
                    i.token_type == "completion"
                        && i.precond.get("source_task").map(|v| v.as_str())
                            == Some(prev_id.as_str())
                });
                assert!(
                    has_seq_arc,
                    "Task {} should have sequential dep arc for previous sibling {}",
                    block.id, prev_id
                );
            }
        }
    }
}

fn check_wiki_link_tokens(ref_state: &PetriRefState, marking: &TaskMarking) {
    let mut expected_links = HashSet::new();
    for block in ref_state.blocks.values() {
        if block.is_completed() {
            continue;
        }
        for link in &block.wiki_links {
            expected_links.insert(link.clone());
        }
    }

    for link in &expected_links {
        assert!(
            marking.token(link).is_some(),
            "Wiki link [[{}]] should have a token in marking",
            link
        );

        let token = marking.token(link).unwrap();
        let expected_type = if link.starts_with("People/") {
            "person"
        } else {
            "document"
        };
        assert_eq!(
            token.token_type(),
            expected_type,
            "Wiki link [[{}]] should have type {}, got {}",
            link,
            expected_type,
            token.token_type()
        );
    }
}

fn check_delegation(ref_state: &PetriRefState, net: &TaskNet, marking: &TaskMarking) {
    for block in ref_state.blocks.values() {
        if block.is_completed() {
            continue;
        }
        if let Executor::Delegated { ref person } = block.executor {
            // Delegate person token must exist
            let person_token_id = format!("person_{person}");
            assert!(
                marking.token(&person_token_id).is_some(),
                "Delegated task {} requires person token {}",
                block.id,
                person_token_id
            );

            // Delegate sub-transition must exist
            let delegate_id = format!("{}_delegate", block.id);
            assert!(
                net.transition(&delegate_id).is_some(),
                "Delegated task {} must have delegate transition {}",
                block.id,
                delegate_id
            );

            // Main transition must have waiting arc
            let main_t = net.transition(&block.id).unwrap();
            let has_waiting_arc = main_t.inputs().iter().any(|i| i.token_type == "waiting");
            assert!(
                has_waiting_arc,
                "Delegated task {} main transition must have waiting input arc",
                block.id
            );
        }
    }
}

fn check_question_knowledge(ref_state: &PetriRefState, net: &TaskNet) {
    for block in ref_state.blocks.values() {
        if block.is_completed() {
            continue;
        }
        if block.is_question {
            let transition = net.transition(&block.id).unwrap();
            let has_knowledge = transition
                .creates()
                .iter()
                .any(|c| c.token_type == "knowledge");
            assert!(
                has_knowledge,
                "Question task {} must create a knowledge token",
                block.id
            );
        }
    }
}

/// Every non-consumed input arc must have a matching output arc (token flows back).
/// This is a fundamental Petri net structural invariant: tokens borrowed by a transition
/// are returned when it fires.
fn check_output_arcs(net: &TaskNet) {
    for t in net.transitions() {
        let output_froms: HashSet<&str> = t.outputs().iter().map(|o| o.from.as_str()).collect();
        for input in t.inputs() {
            if !input.consume {
                assert!(
                    output_froms.contains(input.bind.as_str()),
                    "Transition {}: non-consumed input '{}' has no matching output arc. \
                     Outputs: {:?}",
                    t.id(),
                    input.bind,
                    output_froms,
                );
            }
        }
    }
}

/// Verify net constraints are satisfiable and discount_rate is non-negative.
fn check_net_constraints(net: &TaskNet, _marking: &TaskMarking) {
    assert!(
        net.constraints().is_empty(),
        "TaskNet should have no constraints, got: {:?}",
        net.constraints()
    );
    assert!(
        net.discount_rate() >= 0.0,
        "discount_rate must be non-negative, got: {}",
        net.discount_rate()
    );
}

// ---------------------------------------------------------------------------
// Semantic PN invariants — exercise engine behavior end-to-end
// ---------------------------------------------------------------------------

/// Fire the specified transition and verify the resulting marking.
/// The transition_id is chosen by the PBT generator from the ranked enabled set.
fn check_fire_invariants(
    ref_state: &PetriRefState,
    net: &TaskNet,
    marking: &TaskMarking,
    transition_id: &str,
) {
    let engine = Engine::new();
    let enabled = engine.enabled(net, marking);
    let binding = enabled
        .iter()
        .find(|b| b.transition_id == transition_id)
        .unwrap_or_else(|| {
            panic!(
                "Generator chose transition '{transition_id}' but it is not enabled. \
                 Enabled: {:?}",
                enabled.iter().map(|b| &b.transition_id).collect::<Vec<_>>()
            )
        });
    let transition = net.transition(transition_id).unwrap();
    let pre_marking = marking.clone();
    let mut post_marking = marking.clone();
    engine
        .fire(net, &mut post_marking, binding, 0)
        .unwrap_or_else(|e| panic!("fire failed for {}: {e}", binding.transition_id));

    // a) Completion token created for the fired transition
    let completion_id = format!("completed_{}", binding.transition_id);
    assert!(
        post_marking.token(&completion_id).is_some(),
        "Firing {} must create completion token {}",
        binding.transition_id,
        completion_id
    );

    // b) If question task: knowledge token created
    if let Some(block) = ref_state.blocks.get(&binding.transition_id) {
        if block.is_question {
            let knowledge_id = format!("knowledge_{}", binding.transition_id);
            assert!(
                post_marking.token(&knowledge_id).is_some(),
                "Question task {} must create knowledge token after firing",
                binding.transition_id
            );
        }
    }

    // c) Consumed tokens (consume: true arcs) are gone
    for input in transition.inputs() {
        if input.consume {
            if let Some(token_id) = binding.token_bindings.get(&input.bind) {
                assert!(
                    post_marking.token(token_id).is_none(),
                    "Consumed token {} should be removed after firing {}",
                    token_id,
                    binding.transition_id
                );
            }
        }
    }

    // d) Non-consumed tokens still exist
    for input in transition.inputs() {
        if !input.consume {
            if let Some(token_id) = binding.token_bindings.get(&input.bind) {
                assert!(
                    post_marking.token(token_id).is_some(),
                    "Non-consumed token {} should still exist after firing {}",
                    token_id,
                    binding.transition_id
                );
            }
        }
    }

    // e) Clock advanced by exactly duration_minutes
    let expected_clock =
        pre_marking.clock() + chrono::Duration::minutes(transition.duration_minutes() as i64);
    assert_eq!(
        post_marking.clock(),
        expected_clock,
        "Clock must advance by {} minutes after firing {}",
        transition.duration_minutes(),
        binding.transition_id
    );
}

/// Verify that when two enabled tasks differ only in priority (no deadlines),
/// the higher-priority task ranks higher.
fn check_ranking_reflects_priority(
    ref_state: &PetriRefState,
    net: &TaskNet,
    marking: &TaskMarking,
) {
    let engine = Engine::new();
    let enabled = engine.enabled(net, marking);
    let ranked = engine.rank(net, marking, &enabled);

    let ranked_order: Vec<&str> = ranked
        .iter()
        .map(|r| r.binding.transition_id.as_str())
        .collect();

    for i in 0..ranked_order.len() {
        for j in (i + 1)..ranked_order.len() {
            let id_a = ranked_order[i];
            let id_b = ranked_order[j];
            let block_a = ref_state.blocks.get(id_a);
            let block_b = ref_state.blocks.get(id_b);
            if let (Some(a), Some(b)) = (block_a, block_b) {
                // Only compare when neither has a deadline (deadlines affect urgency)
                if a.deadline.is_none() && b.deadline.is_none() {
                    let pa = a.priority.map(|p| p.to_int()).unwrap_or(0);
                    let pb = b.priority.map(|p| p.to_int()).unwrap_or(0);
                    if pb > pa {
                        // b has higher priority but ranks lower — that's wrong
                        assert!(
                            ranked[i].delta_per_minute >= ranked[j].delta_per_minute,
                            "Task {} (priority {}) ranks above {} (priority {}) \
                             but has lower priority. delta_per_min: {} vs {}",
                            id_a,
                            pa,
                            id_b,
                            pb,
                            ranked[i].delta_per_minute,
                            ranked[j].delta_per_minute,
                        );
                    }
                }
            }
        }
    }
}

/// Verify the objective expression references every active task.
fn check_objective_references_all_tasks(ref_state: &PetriRefState, net: &TaskNet) {
    let obj = net.objective_expr();
    for id in ref_state.active_block_ids() {
        let completion_ref = format!("completed_{id}");
        assert!(
            obj.source.contains(&completion_ref),
            "Objective expr must reference '{}' for active task {}. Got: {}",
            completion_ref,
            id,
            obj.source
        );
    }
}

/// Exercise rank_tasks() end-to-end: builds blocks, calls rank_tasks, checks result.
fn check_rank_tasks_e2e(ref_state: &PetriRefState, blocks: &[Block]) {
    let result = rank_tasks(blocks);

    // mental_slots.occupied == count of DOING blocks
    let doing_count = ref_state
        .blocks
        .values()
        .filter(|b| b.task_state == "DOING")
        .count();
    assert_eq!(
        result.mental_slots.occupied, doing_count,
        "mental_slots.occupied must match DOING task count"
    );

    // mental_slots.capacity matches self descriptor
    let expected_cap = ref_state.effective_self().mental_slots_capacity as usize;
    assert_eq!(
        result.mental_slots.capacity, expected_cap,
        "mental_slots.capacity must match self descriptor"
    );

    // All ranked tasks correspond to active tasks (delegate transitions have _delegate suffix)
    let active_ids: BTreeSet<String> = ref_state.active_block_ids().into_iter().collect();
    for rt in &result.ranked {
        let base_id = rt
            .block_id
            .strip_suffix("_delegate")
            .unwrap_or(&rt.block_id);
        assert!(
            active_ids.contains(base_id),
            "Ranked task {} must correspond to an active task",
            rt.block_id
        );
    }

    // Ranked tasks sorted by delta_per_minute descending
    for pair in result.ranked.windows(2) {
        assert!(
            pair[0].delta_per_minute >= pair[1].delta_per_minute,
            "rank_tasks result not sorted: {} ({}) before {} ({})",
            pair[0].block_id,
            pair[0].delta_per_minute,
            pair[1].block_id,
            pair[1].delta_per_minute,
        );
    }
}

/// Canary ordering invariant: canary_zzz (priority 3) must always rank above
/// canary_aaa (no priority). If the weight pipeline is broken (resolve_prototype,
/// build_context_props, etc. return empty), both get fallback weight 1.0 and
/// position tiebreak favors canary_aaa (position 0) → ordering violated → CAUGHT.
fn check_canary_ordering(net: &TaskNet, marking: &TaskMarking) {
    let engine = Engine::new();
    let enabled = engine.enabled(net, marking);
    let ranked = engine.rank(net, marking, &enabled);

    let ranked_ids: Vec<&str> = ranked
        .iter()
        .map(|r| r.binding.transition_id.as_str())
        .collect();

    let high_pos = ranked_ids.iter().position(|id| *id == "canary_zzz");
    let low_pos = ranked_ids.iter().position(|id| *id == "canary_aaa");

    if let (Some(h), Some(l)) = (high_pos, low_pos) {
        assert!(
            h < l,
            "Canary ordering violated: canary_zzz (priority 3) at position {h}, \
             canary_aaa (no priority) at position {l}. \
             canary_zzz must rank before canary_aaa. \
             Ranked order: {ranked_ids:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// System under test
// ---------------------------------------------------------------------------

struct PetriSUT {
    blocks: Vec<Block>,
}

impl PetriSUT {
    fn rebuild_blocks(ref_state: &PetriRefState) -> Vec<Block> {
        let mut blocks: Vec<&RefBlock> = ref_state.blocks.values().collect();
        blocks.sort_by_key(|b| b.position);
        let mut result: Vec<Block> = blocks
            .into_iter()
            .map(|rb| {
                let mut block = Block::new_text(
                    EntityUri::from_raw(&rb.id),
                    EntityUri::from_raw(&rb.parent_id),
                    &rb.content,
                );
                block.set_property("task_state", HValue::String(rb.task_state.clone()));
                if let Some(p) = rb.priority {
                    block.set_property("priority", HValue::Integer(p.to_int() as i64));
                }
                if let Some(ref d) = rb.deadline {
                    block.set_property("deadline", HValue::String(d.clone()));
                }
                if let Some(dur) = rb.duration_minutes {
                    block.set_property("duration", HValue::Integer(dur));
                }
                if !rb.depends_on.is_empty() {
                    block.set_property("depends_on", HValue::String(rb.depends_on.to_csv()));
                }
                block
            })
            .collect();

        // Add self block if present
        if let Some(ref sb) = ref_state.self_block {
            let mut self_block = Block::new_text(
                EntityUri::block("__self__"),
                EntityUri::block("__meta__"),
                "Self",
            );
            self_block.set_property("is_self", HValue::Boolean(true));
            self_block.set_property("energy", HValue::Float(sb.energy));
            self_block.set_property("focus", HValue::Float(sb.focus));
            self_block.set_property(
                "mental_slots_capacity",
                HValue::Integer(sb.mental_slots_capacity),
            );
            result.push(self_block);
        }

        // Add prototype block if present
        if let Some(ref pb) = ref_state.prototype_block {
            let mut proto_block = Block::new_text(
                EntityUri::block("__prototype__"),
                EntityUri::block("__meta__"),
                "Task Prototype",
            );
            proto_block.set_property("prototype_for", HValue::String("task".to_string()));
            for (k, v) in &pb.properties {
                match v {
                    PrototypeValue::Literal(f) => {
                        proto_block.set_property(k, HValue::Float(*f));
                    }
                    PrototypeValue::Computed(compiled) => {
                        proto_block
                            .set_property(k, HValue::String(format!("={}", compiled.source)));
                    }
                }
            }
            result.push(proto_block);
        }

        result
    }
}

impl StateMachineTest for PetriSUT {
    type SystemUnderTest = Self;
    type Reference = PetriRefState;

    fn init_test(ref_state: &<Self::Reference as ReferenceStateMachine>::State) -> Self {
        Self {
            blocks: Self::rebuild_blocks(ref_state),
        }
    }

    fn apply(
        mut state: Self,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: <Self::Reference as ReferenceStateMachine>::Transition,
    ) -> Self {
        state.blocks = Self::rebuild_blocks(ref_state);

        let prototype_props = ref_state.effective_prototype_props();
        let self_desc = ref_state.effective_self();
        let test_clock = Utc::now();
        let (net, marking) =
            materialize_at(&state.blocks, &self_desc, &prototype_props, test_clock);

        // Check structural invariants after every operation
        check_all_invariants(ref_state, &net, &marking, &self_desc, &prototype_props);

        // On FireTransition, exercise the engine end-to-end
        if let PetriTransition::FireTransition { ref transition_id } = transition {
            check_engine_invariants(ref_state, &net, &marking);
            check_fire_invariants(ref_state, &net, &marking, transition_id);
            check_ranking_reflects_priority(ref_state, &net, &marking);
            check_objective_references_all_tasks(ref_state, &net);
            check_rank_tasks_e2e(ref_state, &state.blocks);
            check_canary_ordering(&net, &marking);
        }

        state
    }
}

prop_state_machine! {
    #[test]
    fn petri_e2e_pbt(
        sequential
        5..20
        =>
        PetriSUT
    );
}
