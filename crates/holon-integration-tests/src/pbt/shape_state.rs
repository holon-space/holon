//! Reference-model fragment for edits of tagged subtrees: the outcome the
//! model expects of each `EditDecisionSubtree`, in dispatch order.
//!
//! The model decides an outcome by running the decision block adapter on its
//! OWN post-edit subtree, never on the store's, so a gate that judges the
//! wrong state disagrees with it.

use holon_pbt_core::capabilities::ExpectedShapeOutcome;

#[derive(Debug, Clone, Default)]
pub struct ShapeRefState {
    outcomes: Vec<ExpectedShapeOutcome>,
    /// The operations of each planned edit, in the order of `outcomes`. The
    /// model applies an edit before the SUT does, so the SUT's operations are
    /// planned against the model's PRE-edit subtree and handed over here.
    planned: Vec<Vec<holon_api::Operation>>,
    /// The rule the shape gate must refuse the CURRENT step's write with, when
    /// a transition other than `EditDecisionSubtree` would break a decision.
    step_refusal: Option<String>,
}

impl ShapeRefState {
    pub fn step_refusal(&self) -> Option<&str> {
        self.step_refusal.as_deref()
    }

    pub fn record(&mut self, outcome: ExpectedShapeOutcome, ops: Vec<holon_api::Operation>) {
        self.outcomes.push(outcome);
        self.planned.push(ops);
    }

    /// The operations of the last planned edit.
    pub fn planned(&self) -> &[holon_api::Operation] {
        self.planned.last().expect("an edit was planned")
    }

    /// The operations of the last `n` planned edits, oldest first.
    pub fn last_planned(&self, n: usize) -> &[Vec<holon_api::Operation>] {
        &self.planned[self.planned.len() - n..]
    }

    pub fn outcomes(&self) -> &[ExpectedShapeOutcome] {
        &self.outcomes
    }
}

/// Transitions whose writes do NOT pass the shape gate, so the model applies
/// them as written: foreign input (org files, peers, remote lists, ingest),
/// the editor's keystroke channel, and the edit transition that models the
/// gate's refusals itself.
const NOT_JUDGED: &[&str] = &[
    "EditDecisionSubtree",
    "EditDecisionPairHeld",
    "TypeChars",
    "WriteOrgFile",
    "ApplyMutation",
    "BulkExternalAdd",
    "StaleExternalRewrite",
    "PasteBlockCopy",
    "EditBlockCopy",
    "DeleteLineFromFile",
    "FinishCutPaste",
    "MoveBlockBetweenFiles",
    "CreateDocument",
    "RenameDocument",
    "DeleteDocument",
    "ExternalWriteWhileFocused",
    "ExternalWriteSameBlockFocused",
    "ReceiverCreateBlock",
    "SyncNow",
    "EmitMcpData",
    "RemoteListSync",
    "AttemptIngestCompoundOnReadOnly",
];

pub fn judged_by_the_shape_gate(variant: &str) -> bool {
    !NOT_JUDGED.contains(&variant)
}

/// The first decision a step changed — its own fields or its direct children —
/// that no longer parses in `after`, and the rule it breaks.
pub fn broken_shape(
    before: &crate::pbt::block_state::BlockState,
    after: &crate::pbt::block_state::BlockState,
) -> Option<(holon_api::EntityUri, String)> {
    use holon_orgmode::OrgBlockExt;

    let subtree = |state: &crate::pbt::block_state::BlockState, root: &holon_api::EntityUri| {
        let mut children: Vec<holon_api::block::Block> = state
            .blocks
            .values()
            .filter(|b| b.parent_id == *root)
            .cloned()
            .collect();
        children.sort_by_key(|b| (b.sequence(), b.id.clone()));
        (state.blocks.get(root).cloned(), children)
    };
    after
        .blocks
        .values()
        .filter(|b| b.tags.contains(holon_api::decision_block::DECISION_TAG))
        .find_map(|root| {
            let now = subtree(after, &root.id);
            if subtree(before, &root.id) == now {
                return None;
            }
            let (Some(decision), children) = now else {
                unreachable!("the root was read from `after`")
            };
            holon_api::decision_block::parse(&decision, &children)
                .err()
                .map(|e| (root.id.clone(), e.code().to_string()))
        })
}

/// What a refused write leaves untouched: the model's documents and the undo
/// history.
pub struct WriteSnapshot {
    domain: crate::pbt::reference_domain_state::ReferenceDomainState,
    files: crate::pbt::file_adapter_state::FileAdapterState,
    undo_stack: Vec<crate::pbt::block_state::BlockState>,
    redo_stack: Vec<crate::pbt::block_state::BlockState>,
}

/// Taken before every step the shape gate judges: a step may break a decision
/// that exists, or tag a block into one that does not parse.
pub fn before_step(
    variant: &str,
    state: &mut crate::pbt::reference_state::ReferenceState,
) -> Option<WriteSnapshot> {
    state.shape.step_refusal = None;
    judged_by_the_shape_gate(variant).then(|| WriteSnapshot {
        domain: state.domain.clone(),
        files: state.files.clone(),
        undo_stack: state.action.undo_stack.clone(),
        redo_stack: state.action.redo_stack.clone(),
    })
}

/// A step whose write breaks a decision is refused by the shape gate: the
/// model keeps its documents and undo history, expects the rule, and expects
/// the user to be told. What the gesture did besides the write (a focus
/// click) stands.
pub fn after_step(
    snapshot: Option<WriteSnapshot>,
    state: &mut crate::pbt::reference_state::ReferenceState,
) {
    let Some(snapshot) = snapshot else {
        return;
    };
    let Some((root, rule)) = broken_shape(&snapshot.domain.block_state, &state.domain.block_state)
    else {
        return;
    };
    // Id minting is monotonic: a refused write never hands an id back.
    let next_id = state.domain.block_state.next_id;
    state.domain = snapshot.domain;
    state.domain.block_state.next_id = next_id;
    state.files = snapshot.files;
    state.action.undo_stack = snapshot.undo_stack;
    state.action.redo_stack = snapshot.redo_stack;
    state.shape.step_refusal = Some(rule);
    state.conditions.raise(
        root.to_string(),
        holon_api::ConditionKind::EDIT_REFUSED_BY_SHAPE,
    );
}

/// The shape refusal the SUT half of the running step must meet, if any.
static ARMED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Arm the SUT half of a step with the refusal its model expects.
pub fn arm(rule: Option<&str>) {
    *ARMED.lock().expect("shape refusal slot poisoned") = rule.map(str::to_string);
}

/// Whether the running step's model expects a shape refusal.
pub fn refusal_armed() -> bool {
    ARMED.lock().expect("shape refusal slot poisoned").is_some()
}

/// Whether a driver error is the shape refusal this step's model expects. A
/// driver that answers a failed write by panicking asks this first; every
/// other error stays fatal.
pub fn is_expected_refusal(message: &str) -> bool {
    ARMED
        .lock()
        .expect("shape refusal slot poisoned")
        .as_deref()
        .is_some_and(|rule| {
            message.contains("refused: after this write")
                && message.contains(&format!("rule {rule}"))
        })
}
