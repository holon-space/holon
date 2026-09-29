//! Tagged block shapes as a write boundary (Model.md invariant 17).
//!
//! A tag may register a [`ShapeValidator`]: a pure judgement of a block
//! carrying that tag together with its direct children. A Holon-side write
//! plan is admitted only when every tagged block it touches still passes its
//! validators AFTER the whole plan. The post-plan state is simulated
//! ([`sim::SimStore`]) over the write authority's reads, so a refused plan
//! writes nothing.

mod decision;
pub mod sim;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;

pub use decision::DecisionShape;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::block::Block;
use sim::SimStore;

use crate::traits::Result;
use crate::traits::WriteAuthorityReads;

/// Why a tagged block and its children break their shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeViolation {
    /// The shape's own name for the broken rule (`B4`, `DC2`, ...).
    pub rule: &'static str,
    pub message: String,
}

/// Judges one tagged block together with its direct children, in sibling
/// order. Pure: it sees no store.
pub trait ShapeValidator: Send + Sync {
    fn tag(&self) -> &'static str;
    fn validate(&self, root: &Block, children: &[Block])
    -> std::result::Result<(), ShapeViolation>;
    /// How a block of this shape is made instead, told to a writer whose write
    /// would TURN a block into this shape and is refused.
    fn created_by(&self) -> &'static str;
    /// Whether a change to a block's text alone can make `validate` refuse.
    /// A shape that answers `false` is never consulted for a text-only write.
    fn judges_text(&self) -> bool;
}

/// A write plan refused because its result breaks a tagged shape. Nothing of
/// the plan was written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("refused: after this write the `{tag}` block {root} breaks rule {rule}: {message}")]
pub struct ShapeRefused {
    pub tag: String,
    pub root: EntityUri,
    pub rule: String,
    pub message: String,
}

/// The validators of this composition, keyed by tag. Insert-only: a second
/// validator for a tag is a composition error.
pub struct ShapeValidators {
    by_tag: BTreeMap<&'static str, Arc<dyn ShapeValidator>>,
}

impl ShapeValidators {
    pub fn new(validators: Vec<Arc<dyn ShapeValidator>>) -> Self {
        let mut by_tag = BTreeMap::new();
        for v in validators {
            let tag = v.tag();
            assert!(
                by_tag.insert(tag, v).is_none(),
                "two shape validators registered for tag `{tag}`"
            );
        }
        Self { by_tag }
    }

    /// Every shape this build knows, one validator per tag. A new typed shape
    /// registers here.
    pub fn registered() -> Self {
        Self::new(vec![Arc::new(DecisionShape)])
    }

    /// The tags of [`Self::registered`] that this registry has no validator
    /// for. A production composition misses none.
    pub fn missing_registered(&self) -> Vec<&'static str> {
        Self::registered()
            .by_tag
            .into_keys()
            .filter(|tag| !self.by_tag.contains_key(tag))
            .collect()
    }

    fn of(&self, block: &Block) -> Vec<&Arc<dyn ShapeValidator>> {
        self.by_tag
            .iter()
            .filter(|(tag, _)| block.tags.contains(tag))
            .map(|(_, v)| v)
            .collect()
    }

    fn is_shaped(&self, block: &Block) -> bool {
        !self.of(block).is_empty()
    }

    fn judge_text(&self) -> bool {
        self.by_tag.values().any(|v| v.judges_text())
    }
}

/// Whether `op` changes nothing of a block but its text (and the marks over
/// it).
fn text_only(op: &PlanOp<'_>) -> bool {
    match op.op_name {
        "set_field" => matches!(
            op.params.get("field").and_then(|v| v.as_string()),
            Some("content" | "marks")
        ),
        "insert_text" | "delete_text" | "apply_mark" | "remove_mark" | "set_title" => true,
        _ => false,
    }
}

/// The block ids an op names by `key`, parsed.
fn named(op: &PlanOp<'_>, key: &str) -> Result<Option<EntityUri>> {
    match op.params.get(key) {
        Some(Value::String(raw)) => Ok(Some(sim::uri(raw)?)),
        Some(Value::Null) | None => Ok(None),
        Some(other) => Err(format!("`{key}` of `{}` is {other:?}", op.op_name).into()),
    }
}

/// Whether `op` provably changes no tagged block and no tagged block's direct
/// children. `false` sends the plan to the simulator, so an op this cannot
/// place is judged, never admitted blind.
fn outside(
    near: &crate::traits::TaggedNeighbourhood,
    tags: &[&str],
    op: &PlanOp<'_>,
    alone: bool,
) -> Result<bool> {
    if text_only(op) {
        return Ok(true);
    }
    let tags_shape = |value: Option<&Value>| match value {
        Some(Value::Array(items)) => items
            .iter()
            .any(|v| v.as_string().is_some_and(|t| tags.contains(&t))),
        Some(Value::String(raw)) => raw.split([',', ' ']).any(|t| tags.contains(&t)),
        _ => false,
    };
    let target = named(op, "id")?;
    // Only a create may leave its id to the provider; any other op without
    // one is for the simulator to reject.
    let Some(target) = target else {
        return Ok(op.op_name == "create"
            && !is_tagged(near, &named(op, "parent_id")?)?
            && !tags_shape(op.params.get("tags")));
    };
    if near.is_tagged(&target)? || near.is_under_tagged(&target)? {
        return Ok(false);
    }
    Ok(match op.op_name {
        "set_field" => {
            let field = op.params.get("field").and_then(|v| v.as_string());
            match field {
                Some("parent_id") => false,
                Some("tags") => !tags_shape(op.params.get("value")),
                Some(_) => true,
                None => false,
            }
        }
        "create" => {
            !is_tagged(near, &named(op, "parent_id")?)?
                && !tags_shape(op.params.get("tags"))
                && !op
                    .params
                    .get("properties")
                    .and_then(|p| match p {
                        Value::Object(map) => Some(tags_shape(map.get("tags"))),
                        _ => None,
                    })
                    .unwrap_or(false)
        }
        "move_block" => !is_tagged(near, &named(op, "parent_id")?)?,
        "delete" | "move_up" | "move_down" | "split_block" => true,
        // Both read the block's place before the PLAN, so only a plan of one
        // op may rely on it.
        "indent" | "outdent" if !alone => false,
        // The new parent is the previous sibling.
        "indent" => match near.previous_sibling() {
            Some(before) => !near.is_tagged(before)?,
            None => false,
        },
        // The new parent is the grandparent: a grandchild can land among a
        // tagged block's children.
        "outdent" => match near.parent(&target)? {
            Some(parent) => !near.is_under_tagged(parent)?,
            None => false,
        },
        _ => false,
    })
}

fn is_tagged(near: &crate::traits::TaggedNeighbourhood, id: &Option<EntityUri>) -> Result<bool> {
    match id {
        Some(id) => near.is_tagged(id),
        None => Ok(false),
    }
}

/// One block operation of a plan, as the dispatcher routes it.
pub struct PlanOp<'a> {
    pub op_name: &'a str,
    pub params: &'a StorageEntity,
}

/// A tagged block and its direct children as the simulator predicts them
/// after a plan.
pub type Simulated = (Block, Vec<Block>);

/// The verdict on a plan, and the tagged blocks it judged.
pub struct Judgement {
    pub verdict: std::result::Result<(), ShapeRefused>,
    pub simulated: Vec<Simulated>,
    /// Every block whose shape the plan's ops can change on their way to the
    /// end state: what [`judge_stored`] reads when the plan stops part way.
    pub touched: Vec<EntityUri>,
    /// Blocks the plan creates. No other write can name them before the plan
    /// lands, and the simulator mints a new id for each at every judgement.
    pub created: BTreeSet<EntityUri>,
}

impl Judgement {
    /// Every block whose change can alter the parse of a tagged block the plan
    /// shapes: each such root, its direct children after the plan, and every
    /// block the plan writes on the way, which holds each child that leaves.
    /// A block the plan creates is not claimed. Empty when the plan shapes no
    /// tagged block.
    pub fn shape_claim(&self) -> BTreeSet<EntityUri> {
        if self.simulated.is_empty() {
            return BTreeSet::new();
        }
        self.simulated
            .iter()
            .flat_map(|(root, children)| {
                std::iter::once(root.id.clone()).chain(children.iter().map(|c| c.id.clone()))
            })
            .chain(self.touched.iter().cloned())
            .filter(|id| !self.created.contains(id))
            .collect()
    }
}

/// Simulate `ops` in order over `authority` and judge every tagged block the
/// plan touched. An op the simulator cannot apply is an `Err`: a plan whose
/// result is unknown is never admitted.
pub async fn judge_plan(
    validators: &ShapeValidators,
    authority: Arc<dyn WriteAuthorityReads>,
    ops: &[PlanOp<'_>],
) -> Result<Judgement> {
    if !validators.judge_text() && ops.iter().all(text_only) {
        return Ok(Judgement {
            verdict: Ok(()),
            simulated: Vec::new(),
            touched: Vec::new(),
            created: BTreeSet::new(),
        });
    }
    let tags: Vec<&str> = validators.by_tag.keys().copied().collect();
    let alone = ops.len() == 1;
    let mut around = Vec::new();
    for op in ops {
        around.extend(named(op, "id")?);
        around.extend(named(op, "parent_id")?);
    }
    let indented = match alone && ops[0].op_name == "indent" {
        true => named(&ops[0], "id")?,
        false => None,
    };
    if let Some(near) = authority
        .tagged_neighbourhood(&tags, &around, indented.as_ref())
        .await?
    {
        let mut shapes_nothing = true;
        for op in ops {
            if !outside(&near, &tags, op, alone)? {
                shapes_nothing = false;
                break;
            }
        }
        if shapes_nothing {
            return Ok(Judgement {
                verdict: Ok(()),
                simulated: Vec::new(),
                touched: Vec::new(),
                created: BTreeSet::new(),
            });
        }
    }
    let sim = SimStore::new(authority);
    for op in ops {
        sim.apply(op.op_name, op.params).await.map_err(|e| {
            if e.is::<crate::BlockNotInWriteAuthority>() {
                return e;
            }
            format!(
                "shape gate: `block.{}` could not be simulated, so its result cannot be \
                 judged: {e}",
                op.op_name
            )
            .into()
        })?;
    }
    let touched = candidates(&sim).await?;
    let mut created = BTreeSet::new();
    for id in sim.written() {
        if sim.base_block(&id).await?.is_none() {
            created.insert(id);
        }
    }
    let mut simulated = Vec::new();
    for root in shaped(validators, &sim, &touched).await? {
        let children = sim.ordered_children(&root.id).await?;
        for v in validators.of(&root) {
            if let Err(mut violation) = v.validate(&root, &children) {
                let was_shaped = sim
                    .base_block(&root.id)
                    .await?
                    .is_some_and(|b| b.tags.contains(v.tag()));
                if !was_shaped {
                    violation.message = format!("{} — {}", violation.message, v.created_by());
                }
                return Ok(Judgement {
                    verdict: Err(ShapeRefused {
                        tag: v.tag().to_string(),
                        root: root.id.clone(),
                        rule: violation.rule.to_string(),
                        message: violation.message,
                    }),
                    simulated: Vec::new(),
                    touched: Vec::new(),
                    created: BTreeSet::new(),
                });
            }
        }
        simulated.push((root, children));
    }
    Ok(Judgement {
        verdict: Ok(()),
        simulated,
        touched: touched.into_iter().collect(),
        created,
    })
}

/// What a write claims before it runs when the writes that follow it are not
/// known yet: each tagged block that one of `targets` is or is a direct child
/// of, each tagged block in the subtree of one of `subtrees`, and the direct
/// children of all of them, as `authority` holds them now.
pub async fn neighbourhood(
    validators: &ShapeValidators,
    authority: &dyn WriteAuthorityReads,
    targets: &[EntityUri],
    subtrees: &[EntityUri],
) -> Result<BTreeSet<EntityUri>> {
    let mut roots = BTreeSet::new();
    let tags: Vec<&str> = validators.by_tag.keys().copied().collect();
    // The judgement of a target's own op makes this same read.
    match authority.tagged_neighbourhood(&tags, targets, None).await? {
        Some(near) => {
            for target in targets {
                if near.is_tagged(target)? {
                    roots.insert(target.clone());
                }
                if near.is_under_tagged(target)? {
                    let parent = near
                        .parent(target)?
                        .expect("a block under a tagged one has a parent");
                    roots.insert(parent.clone());
                }
            }
        }
        None => {
            for target in targets {
                // An absent block is the op's own error to report.
                let Some(stored) = authority.block(target).await? else {
                    continue;
                };
                if validators.is_shaped(&stored.block) {
                    roots.insert(target.clone());
                }
                if let Some(parent) = authority.block(&stored.block.parent_id).await? {
                    if validators.is_shaped(&parent.block) {
                        roots.insert(parent.block.id);
                    }
                }
            }
        }
    }
    for root in subtrees {
        let Some(subtree) = authority.subtree(root).await? else {
            continue;
        };
        for stored in subtree {
            if validators.is_shaped(&stored.block) {
                roots.insert(stored.block.id);
            }
        }
    }
    let mut claim = roots.clone();
    for root in &roots {
        claim.extend(authority.children(root).await?);
    }
    Ok(claim)
}

/// Judge what `authority` holds now for the blocks a plan touched: the state
/// an admitted plan left when it stopped part way.
pub async fn judge_stored(
    validators: &ShapeValidators,
    authority: &dyn WriteAuthorityReads,
    touched: &[EntityUri],
) -> Result<std::result::Result<(), ShapeRefused>> {
    for id in touched {
        let Some(root) = authority.block(id).await? else {
            continue;
        };
        let root = root.block;
        let shapes = validators.of(&root);
        if shapes.is_empty() {
            continue;
        }
        let mut children = Vec::new();
        for child in authority.children(id).await? {
            let child = authority
                .block(&child)
                .await?
                .ok_or_else(|| format!("child {child} of {id} listed but absent"))?;
            children.push(child.block);
        }
        for v in shapes {
            if let Err(violation) = v.validate(&root, &children) {
                return Ok(Err(ShapeRefused {
                    tag: v.tag().to_string(),
                    root: root.id.clone(),
                    rule: violation.rule.to_string(),
                    message: violation.message,
                }));
            }
        }
    }
    Ok(Ok(()))
}

/// A differential check of the simulator against the write authority: after
/// an admitted plan ran, the tagged blocks it judged must read back as the
/// simulator predicted them. Off unless a harness enables it.
#[derive(Default)]
pub struct ShapeAudit {
    enabled: std::sync::atomic::AtomicBool,
    log: std::sync::Mutex<AuditLog>,
}

#[derive(Debug, Clone, Default)]
pub struct AuditLog {
    /// Admitted plans that touched at least one tagged block.
    pub compared: usize,
    pub divergences: Vec<String>,
}

/// What a shape reads of one block: its tags, its text and its properties,
/// minus the bookkeeping keys no shape reads.
fn shape_view(b: &Block) -> (Vec<String>, String, Vec<(String, String)>) {
    const BOOKKEEPING: [&str; 13] = [
        "task_state_category",
        "created_at",
        "updated_at",
        "collapsed",
        "widget_only",
        "completed",
        "block_type",
        "content_type",
        "sequence",
        "level",
        "ID",
        "id",
        "document_id",
    ];
    let mut tags: Vec<String> = b.tags.iter().cloned().collect();
    tags.sort();
    let mut props: Vec<(String, String)> = b
        .properties
        .iter()
        .filter(|(k, _)| !k.starts_with('_') && !BOOKKEEPING.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), format!("{v:?}")))
        .collect();
    props.sort();
    (tags, b.content.clone(), props)
}

impl ShapeAudit {
    pub fn global() -> &'static ShapeAudit {
        static AUDIT: std::sync::OnceLock<ShapeAudit> = std::sync::OnceLock::new();
        AUDIT.get_or_init(ShapeAudit::default)
    }

    pub fn enable(&self) {
        self.enabled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn reset(&self) {
        *self.log.lock().expect("shape audit poisoned") = AuditLog::default();
    }

    pub fn log(&self) -> AuditLog {
        self.log.lock().expect("shape audit poisoned").clone()
    }

    /// Compare what `authority` now holds with what the simulator predicted.
    pub async fn compare(
        &self,
        authority: &dyn WriteAuthorityReads,
        what: &str,
        simulated: &[Simulated],
    ) -> Result<()> {
        if simulated.is_empty() {
            return Ok(());
        }
        let mut found = Vec::new();
        for (root, children) in simulated {
            let Some(actual) = authority.block(&root.id).await? else {
                found.push(format!(
                    "{what}: {} predicted, absent in the store",
                    root.id
                ));
                continue;
            };
            if shape_view(&actual.block) != shape_view(root) {
                found.push(format!(
                    "{what}: {} predicted {:?}, stored {:?}",
                    root.id,
                    shape_view(root),
                    shape_view(&actual.block)
                ));
            }
            let mut stored = Vec::new();
            for id in authority.children(&root.id).await? {
                let child = authority
                    .block(&id)
                    .await?
                    .ok_or_else(|| format!("child {id} of {} listed but absent", root.id))?;
                stored.push(shape_view(&child.block));
            }
            let predicted: Vec<_> = children.iter().map(shape_view).collect();
            if stored != predicted {
                found.push(format!(
                    "{what}: children of {} predicted {predicted:?}, stored {stored:?}",
                    root.id
                ));
            }
        }
        let mut log = self.log.lock().expect("shape audit poisoned");
        log.compared += 1;
        log.divergences.extend(found);
        Ok(())
    }
}

/// Every block whose own fields or child list the plan changed, or one of
/// whose children the plan wrote.
async fn candidates(sim: &SimStore) -> Result<BTreeSet<EntityUri>> {
    let mut candidates: BTreeSet<EntityUri> = BTreeSet::new();
    for id in sim.written() {
        candidates.insert(id.clone());
        for parent in sim.parents_of(&id).await? {
            candidates.insert(parent);
        }
    }
    candidates.extend(sim.relisted());
    Ok(candidates)
}

/// The `candidates` that carry a registered tag after the plan.
async fn shaped(
    validators: &ShapeValidators,
    sim: &SimStore,
    candidates: &BTreeSet<EntityUri>,
) -> Result<Vec<Block>> {
    let mut roots = Vec::new();
    for id in candidates {
        if let Some(block) = sim.block(id).await? {
            if validators.is_shaped(&block) {
                roots.push(block);
            }
        }
    }
    Ok(roots)
}
