//! Transition: edit a `decision`-tagged subtree through the production engine,
//! as ONE user gesture of generic block writes.
//!
//! @pbt rung dispatch
//! @pbt covers shape-write-boundary — a Holon-side write whose result the
//!   decision block adapter refuses must be refused before it lands, and a
//!   write whose result it accepts must land, even when only the whole gesture
//!   is legal
//!
//! The oracle applies the edit to ITS OWN copy of the subtree and runs the
//! block adapter on the result. An accepted result becomes the model's state; a
//! refused one leaves the model unchanged and records the rule the SUT must
//! name. The SUT's outcome is recorded, not asserted, and
//! `inv-shape-gate-refuses-illegal-writes` judges it.

use std::collections::HashMap;

use holon_api::EntityUri;
use holon_api::Operation;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::decision_block;
use holon_orgmode::OrgBlockExt;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::ExpectedShapeOutcome;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutShapeEdit;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;

const DECIDER: &str = "person:martin";
const DECIDED_AT: &str = "2026-09-29T10:00:00Z";

holon_pbt_core::step_field_via_json!(
    DecisionEdit,
    vec![
        DecisionEdit::Decide {
            keys: vec!["z".to_string()],
        },
        DecisionEdit::RemoveDecisionTag,
    ]
);

/// One edit of a decision, in the vocabulary of its block shape.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DecisionEdit {
    /// Keyword DONE plus the ruling keys.
    Decide {
        keys: Vec<String>,
    },
    /// Keyword `?` and the ruling keys removed.
    Reopen,
    SetChoose {
        raw: String,
    },
    /// A new option child after the last child.
    AddOption {
        id: String,
        key: String,
        label: String,
    },
    RemoveOption {
        key: String,
    },
    /// Re-key the option child that holds `key`.
    RekeyOption {
        key: String,
        to: String,
    },
    RemoveDecisionTag,
    /// An AGENT writes the decision's whole source line (keyword + title)
    /// through the generic `set_field("source_text")` — not the editor's
    /// keystroke channel, so the shape gate judges it.
    SourceTextAsAgent {
        source: String,
    },
    /// A new `choose` and a ruling in one gesture: legal only as a whole when
    /// the ruling's size fits the new `choose` and not the old one.
    ChooseAndDecide {
        choose: String,
        keys: Vec<String>,
    },
    /// A structural op on the decision, one of its children, or the block
    /// after it: the tree edits the gate must judge by the shape they leave.
    Structural {
        op: TreeOp,
        target: Target,
    },
}

/// A structural block op, as the block providers name it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TreeOp {
    Indent,
    Outdent,
    /// Split at this character offset, modulo the text length.
    Split {
        at: usize,
    },
    Join,
    /// Move to the decision's parent, directly after the decision.
    MoveOut,
}

/// Which block of the decision's neighbourhood a structural op acts on.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Target {
    Decision,
    /// The child at this index, modulo the child count.
    Child {
        index: usize,
    },
    /// The decision's next sibling.
    NextSibling,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I apply {edit} to decision {root}")]
pub struct EditDecisionSubtree {
    pub root: String,
    pub edit: DecisionEdit,
}

/// The decision and its direct children, in sibling order.
fn subtree(state: &ReferenceState, root: &EntityUri) -> Option<(Block, Vec<Block>)> {
    let blocks = &state.domain.block_state.blocks;
    let decision = blocks.get(root)?.clone();
    let mut children: Vec<Block> = blocks
        .values()
        .filter(|b| b.parent_id == *root)
        .cloned()
        .collect();
    children.sort_by_key(|b| (b.sequence(), b.id.clone()));
    Some((decision, children))
}

pub(super) fn decision_roots(state: &ReferenceState) -> Vec<String> {
    state
        .domain
        .block_state
        .blocks
        .values()
        .filter(|b| b.tags.contains(decision_block::DECISION_TAG))
        .map(|b| b.id.to_string())
        .collect()
}

fn text(s: &str) -> Value {
    Value::String(s.to_string())
}

fn set_field(id: &EntityUri, field: &str, value: Value) -> Operation {
    Operation::new(
        "block",
        "set_field",
        "set_field",
        HashMap::from([
            ("id".to_string(), text(id.as_str())),
            ("field".to_string(), text(field)),
            ("value".to_string(), value),
        ]),
    )
}

/// The keyword and the category sidecar every task-state write pairs with it.
fn set_keyword(block: &mut Block, keyword: &str) {
    block.properties.insert("task_state".into(), text(keyword));
    block.properties.insert(
        "task_state_category".into(),
        text(
            holon_api::TaskState::from_keyword(keyword)
                .category
                .as_str(),
        ),
    );
}

fn option_child<'c>(children: &'c mut [Block], key: &str) -> Option<&'c mut Block> {
    children
        .iter_mut()
        .find(|c| c.get_property_str("option").as_deref() == Some(key))
}

impl DecisionEdit {
    /// The edited subtree and the operations that make the same edit in the
    /// store. `None` when the edit names nothing in this subtree.
    fn apply(
        &self,
        mut decision: Block,
        mut children: Vec<Block>,
    ) -> Option<(Block, Vec<Block>, Vec<Operation>)> {
        let id = decision.id.clone();
        let mut ops = Vec::new();
        let decide = |decision: &mut Block, ops: &mut Vec<Operation>, keys: &[String]| {
            let chosen = keys.join(" ");
            set_keyword(decision, "DONE");
            ops.push(set_field(&id, "task_state", text("DONE")));
            for (key, value) in [
                ("chosen", chosen.as_str()),
                ("decider", DECIDER),
                ("decided", DECIDED_AT),
            ] {
                decision.properties.insert(key.to_string(), text(value));
                ops.push(set_field(&id, key, text(value)));
            }
        };
        match self {
            DecisionEdit::Decide { keys } => decide(&mut decision, &mut ops, keys),
            DecisionEdit::Reopen => {
                if decision.get_property_str("task_state").as_deref() != Some("DONE") {
                    return None;
                }
                set_keyword(&mut decision, "?");
                ops.push(set_field(&id, "task_state", text("?")));
                for key in ["chosen", "decider", "decided"] {
                    decision.properties.remove(key);
                    ops.push(set_field(&id, key, Value::REMOVED));
                }
            }
            DecisionEdit::SetChoose { raw } => {
                decision.properties.insert("choose".into(), text(raw));
                ops.push(set_field(&id, "choose", text(raw)));
            }
            DecisionEdit::AddOption {
                id: new_id,
                key,
                label,
            } => {
                let new_id = EntityUri::block(new_id);
                if children.iter().any(|c| c.id == new_id) {
                    return None;
                }
                let mut params = HashMap::from([
                    ("id".to_string(), text(new_id.as_str())),
                    ("parent_id".to_string(), text(id.as_str())),
                    ("content".to_string(), text(label)),
                    (
                        "properties".to_string(),
                        Value::Object(HashMap::from([("option".to_string(), text(key))])),
                    ),
                ]);
                let last = children.last();
                if let Some(last) = last {
                    params.insert("after_block_id".into(), text(last.id.as_str()));
                }
                let next = last.map_or(0, |b| b.sequence() + 1);
                let mut option = Block::new_text(new_id, id.clone(), label);
                option.properties.insert("option".into(), text(key));
                option.set_sequence(next);
                children.push(option);
                ops.push(Operation::new("block", "create", "create", params));
            }
            DecisionEdit::RemoveOption { key } => {
                let at = children
                    .iter()
                    .position(|c| c.get_property_str("option").as_deref() == Some(key.as_str()))?;
                let gone = children.remove(at);
                ops.push(Operation::new(
                    "block",
                    "delete",
                    "delete",
                    HashMap::from([("id".to_string(), text(gone.id.as_str()))]),
                ));
            }
            DecisionEdit::RekeyOption { key, to } => {
                let child = option_child(&mut children, key)?;
                child.properties.insert("option".into(), text(to));
                ops.push(set_field(&child.id, "option", text(to)));
            }
            DecisionEdit::RemoveDecisionTag => {
                decision.tags.remove(decision_block::DECISION_TAG);
                let rest: Vec<Value> = decision.tags.iter().map(|t| text(t)).collect();
                ops.push(set_field(&id, "tags", Value::Array(rest)));
            }
            DecisionEdit::SourceTextAsAgent { source } => {
                let vocabulary = holon_org_format::TaskKeywordVocabulary::default();
                match holon_org_format::converge_keyword_headed(source, &vocabulary) {
                    Some(p) => {
                        decision.content = p.stripped.clone();
                        set_keyword(&mut decision, &p.keyword.keyword);
                    }
                    None => {
                        decision.content = source.clone();
                        decision.properties.remove("task_state");
                        decision.properties.remove("task_state_category");
                    }
                }
                ops.push(set_field(&id, holon_api::SOURCE_TEXT_FIELD, text(source)));
            }
            DecisionEdit::ChooseAndDecide { choose, keys } => {
                decision.properties.insert("choose".into(), text(choose));
                ops.push(set_field(&id, "choose", text(choose)));
                decide(&mut decision, &mut ops, keys);
            }
            DecisionEdit::Structural { .. } => {
                unreachable!("a structural edit is planned over the whole tree")
            }
        }
        Some((decision, children, ops))
    }
}

/// The model's state after the edit, the SUT operations for it, and the
/// outcome the adapter decides for the result.
fn plan(
    state: &ReferenceState,
    me: &EditDecisionSubtree,
) -> Option<(ReferenceState, Vec<Operation>, ExpectedShapeOutcome)> {
    let root = EntityUri::parse(&me.root).expect("a decision root is drawn as a block uri");
    let (decision, children) = subtree(state, &root)?;
    if !decision.tags.contains(decision_block::DECISION_TAG) {
        return None;
    }
    let mut after = state.clone();
    let ops = match &me.edit {
        DecisionEdit::Structural { op, target } => {
            structural(&mut after, &decision, &children, op, target)?
        }
        edit => {
            let (decision, children, ops) = edit.apply(decision, children)?;
            let blocks = &mut after.domain.block_state.blocks;
            blocks.retain(|_, b| b.parent_id != root);
            blocks.insert(root, decision);
            for child in children {
                blocks.insert(child.id.clone(), child);
            }
            ops
        }
    };
    let outcome = match crate::pbt::shape_state::broken_shape(
        &state.domain.block_state,
        &after.domain.block_state,
    ) {
        Some((_, rule)) => ExpectedShapeOutcome::Refused { rule },
        None => ExpectedShapeOutcome::Applied,
    };
    Some((after, ops, outcome))
}

fn block_op(op: &str, params: Vec<(&str, Value)>) -> Operation {
    Operation::new(
        "block",
        op,
        op,
        params
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

/// Apply a structural op to the model's whole tree, the way production does,
/// and return the operation that makes it in the store. `None` when the op
/// does not apply to the target here.
fn structural(
    state: &mut ReferenceState,
    decision: &Block,
    children: &[Block],
    op: &TreeOp,
    target: &Target,
) -> Option<Vec<Operation>> {
    let id = match target {
        Target::Decision => decision.id.clone(),
        Target::Child { index } if !children.is_empty() => {
            children[index % children.len()].id.clone()
        }
        Target::Child { .. } => return None,
        Target::NextSibling => state.next_sibling(&decision.id)?,
    };
    if state.is_page_block(&id) {
        return None;
    }
    let param = |v: &EntityUri| text(v.as_str());
    Some(vec![match op {
        TreeOp::Indent => {
            let prev = state.previous_sibling(&id)?;
            if state.is_page_block(&prev) {
                return None;
            }
            let after = state.sorted_children(&prev).last().cloned();
            state.move_block(&id, prev, after.as_ref());
            block_op("indent", vec![("id", param(&id))])
        }
        TreeOp::Outdent => {
            // ADR 0028 D1: a page's direct child does not leave its page.
            let parent = state.parent_of(&id)?;
            if state.is_page_block(&parent) || state.parent_of(&parent).is_none() {
                return None;
            }
            state.outdent_block(&id);
            block_op("outdent", vec![("id", param(&id))])
        }
        TreeOp::Split { at } => {
            let content = state.block_content(&id)?.to_string();
            let chars = content.chars().count();
            let position = content
                .char_indices()
                .nth(at % (chars + 1))
                .map_or(content.len(), |(i, _)| i);
            state.split_block(&id, position);
            block_op(
                "split_block",
                vec![
                    ("id", param(&id)),
                    ("position", Value::Integer(position as i64)),
                ],
            )
        }
        TreeOp::Join => {
            holon_pbt_core::capabilities::join_merge_target(&id, &*state)?;
            state.join_block(&id);
            block_op(
                "join_block",
                vec![("id", param(&id)), ("position", Value::Integer(0))],
            )
        }
        TreeOp::MoveOut => {
            if id == decision.id {
                return None;
            }
            state.move_block(&id, decision.parent_id.clone(), Some(&decision.id));
            block_op(
                "move_block",
                vec![
                    ("id", param(&id)),
                    ("parent_id", param(&decision.parent_id)),
                    ("after_block_id", param(&decision.id)),
                ],
            )
        }
    }])
}
fn key_set() -> impl Strategy<Value = Vec<String>> {
    proptest::sample::subsequence(vec!["a", "b", "c", "d", "z"], 0..=3)
        .prop_map(|keys| keys.into_iter().map(str::to_string).collect())
}

/// Whether this draw's store can take a property REMOVAL through the
/// dispatcher. The SQL-only operation log serializes op params to JSON and
/// panics on `Value::REMOVED` (holon-pattern `value.rs`); until that is
/// fixed, a removal is drawn only where Loro is the write authority.
pub(super) fn removal_writable(state: &ReferenceState) -> bool {
    state
        .harness
        .wiring
        .storage_adapters
        .contains(&holon_pbt_core::StorageAdapter::Loro)
}

pub(super) fn edit_strategy(removal_writable: bool) -> BoxedStrategy<DecisionEdit> {
    let choose = proptest::sample::select(vec!["1", "2", "0..1", "1..2", "3", "4", "x"])
        .prop_map(str::to_string);
    let key = proptest::sample::select(vec!["a", "b", "c", "d", "z"]).prop_map(str::to_string);
    let new_key = proptest::sample::select(vec!["a", "d", "e", "Bad"]).prop_map(str::to_string);
    let mut arms: Vec<(u32, BoxedStrategy<DecisionEdit>)> = vec![
        (
            3,
            key_set()
                .prop_map(|keys| DecisionEdit::Decide { keys })
                .boxed(),
        ),
        (
            2,
            choose
                .clone()
                .prop_map(|raw| DecisionEdit::SetChoose { raw })
                .boxed(),
        ),
        (
            2,
            (new_key.clone(), 0u32..1000)
                .prop_map(|(key, n)| DecisionEdit::AddOption {
                    id: format!("kd-opt-{n}"),
                    label: format!("Option {key}"),
                    key,
                })
                .boxed(),
        ),
        (
            1,
            key.clone()
                .prop_map(|key| DecisionEdit::RemoveOption { key })
                .boxed(),
        ),
        (
            1,
            (key, new_key)
                .prop_map(|(key, to)| DecisionEdit::RekeyOption { key, to })
                .boxed(),
        ),
        (1, Just(DecisionEdit::RemoveDecisionTag).boxed()),
        (
            2,
            proptest::sample::select(vec![
                "? Which store holds the decision now?",
                "TODO Which store holds the decision?",
                "DONE Which store holds the decision?",
                "Which store holds the decision?",
            ])
            .prop_map(|source| DecisionEdit::SourceTextAsAgent {
                source: source.to_string(),
            })
            .boxed(),
        ),
        (
            2,
            (choose, key_set())
                .prop_map(|(choose, keys)| DecisionEdit::ChooseAndDecide { choose, keys })
                .boxed(),
        ),
    ];
    let tree_op = prop_oneof![
        Just(TreeOp::Indent),
        Just(TreeOp::Outdent),
        (0usize..40).prop_map(|at| TreeOp::Split { at }),
        Just(TreeOp::Join),
        Just(TreeOp::MoveOut),
    ];
    let target = prop_oneof![
        1 => Just(Target::Decision),
        4 => (0usize..8).prop_map(|index| Target::Child { index }),
        1 => Just(Target::NextSibling),
    ];
    arms.push((
        4,
        (tree_op, target)
            .prop_map(|(op, target)| DecisionEdit::Structural { op, target })
            .boxed(),
    ));
    if removal_writable {
        arms.push((1, Just(DecisionEdit::Reopen).boxed()));
    }
    proptest::strategy::Union::new_weighted(arms).boxed()
}

impl TransitionFactory<ReferenceState> for EditDecisionSubtree {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let roots = decision_roots(state);
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!roots.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            let admitted_yet = state
                .shape
                .outcomes()
                .contains(&ExpectedShapeOutcome::Applied);
            let openers = if admitted_yet {
                Vec::new()
            } else {
                legal_openers(state, &roots)
            };
            if !openers.is_empty() {
                return (
                    FIRST_ADMIT_WEIGHT,
                    proptest::sample::select(openers).boxed(),
                );
            }
            let strat = (
                proptest::sample::select(roots),
                edit_strategy(removal_writable(state)),
            )
                .prop_map(|(root, edit)| EditDecisionSubtree { root, edit })
                .boxed();
            (40, strat)
        })
    }
}

/// Ten times an ordinary decision edit, so a case that carries a decision
/// admits an edit early while every other variant stays drawable from the
/// initial state:
/// `shape_sim_matches_authority::assert_reached` fails a run with no admitted
/// edit and no simulator comparison.
const FIRST_ADMIT_WEIGHT: u32 = 400;

/// A new option under each decision, kept only where the model admits it.
fn legal_openers(state: &ReferenceState, roots: &[String]) -> Vec<EditDecisionSubtree> {
    roots
        .iter()
        .enumerate()
        .flat_map(|(i, root)| {
            ["d", "e", "f"].map(|key| EditDecisionSubtree {
                root: root.clone(),
                edit: DecisionEdit::AddOption {
                    id: format!("kd-first-{i}-{key}"),
                    key: key.to_string(),
                    label: format!("Option {key}"),
                },
            })
        })
        .filter(|edit| {
            matches!(
                plan(state, edit),
                Some((_, _, ExpectedShapeOutcome::Applied))
            )
        })
        .collect()
}

impl TransitionRef<ReferenceState> for EditDecisionSubtree {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(plan(state, self).is_some(), Reason::PreconditionFailed),
            check(
                self.edit != DecisionEdit::Reopen || removal_writable(state),
                Reason::PreconditionFailed,
            ),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let (after, ops, outcome) =
            plan(state, self).expect("preconditions hold only when the edit plans");
        match &outcome {
            ExpectedShapeOutcome::Applied => {
                // An agent's write never enters the human undo stack.
                if !matches!(self.edit, DecisionEdit::SourceTextAsAgent { .. }) {
                    state.push_undo_snapshot();
                }
                state.domain = after.domain;
            }
            // A refused user gesture is disclosed; an agent gets the error
            // alone.
            ExpectedShapeOutcome::Refused { .. } if self.origin().is_user() => {
                state.conditions.raise(
                    self.root.clone(),
                    holon_api::ConditionKind::EDIT_REFUSED_BY_SHAPE,
                );
            }
            ExpectedShapeOutcome::Refused { .. } => {}
        }
        state.shape.record(outcome, ops);
    }
}

impl EditDecisionSubtree {
    pub(crate) fn declared_caps() -> Vec<holon_pbt_core::composition::CapId> {
        vec![holon_pbt_core::composition::CapId::of::<dyn SutShapeEdit>()]
    }

    pub(super) fn origin(&self) -> holon_api::OpOrigin {
        match self.edit {
            DecisionEdit::SourceTextAsAgent { .. } => holon_api::OpOrigin::Agent {
                session_id: "keystone".into(),
                tool_call_id: "edit-decision-subtree".into(),
            },
            _ => holon_api::OpOrigin::User,
        }
    }
}

// Written out rather than through `cap_transition!`: the SUT operations are
// planned from the concrete model's subtree.
#[allow(async_fn_in_trait)]
impl<S: SutShapeEdit> holon_pbt_core::TransitionImpl<ReferenceState, S> for EditDecisionSubtree {
    async fn apply_to_sut(&self, state: &ReferenceState, sut: &mut S) {
        let ops = state.shape.planned().to_vec();
        // Judged by `inv-shape-gate-refuses-illegal-writes`; a refusal here is
        // an expected outcome, not a harness failure.
        let _ = sut.apply_shape_edit(ops, self.origin()).await;
    }
}

#[cfg(feature = "otel-testing")]
impl holon_pbt_core::budget::SqlBudget for EditDecisionSubtree {
    fn expected_sql<R2: holon_pbt_core::capabilities::RefSqlCardinality>(
        &self,
        state: &R2,
    ) -> holon_pbt_core::budget::ExpectedSql {
        let mut sql = expected_sql_for_kind(
            MutationKind::Update,
            state.active_watch_count(),
            state.block_count(),
            state.document_count(),
        );
        sql.tolerance += 40;
        sql
    }
}
