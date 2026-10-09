//! The shape gate's verdict on single ops and plans against a decision
//! subtree held by an in-memory write authority.
//!
//! The keystone (`inv-shape-gate-refuses-illegal-writes`) drives the gate
//! through the production dispatcher; this file pins the structural ops the
//! keystone's edit alphabet does not reach — Enter inside an option, indent
//! into a decision, deleting a chosen option — and measures the gate's cost.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use holon_api::EntityUri;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::TaggedNeighbourhood;
use holon_core::WriteAuthorityReads;
use holon_core::shape_gate::DecisionShape;
use holon_core::shape_gate::PlanOp;
use holon_core::shape_gate::ShapeRefused;
use holon_core::shape_gate::ShapeValidators;
use holon_core::shape_gate::judge_plan;
use holon_core::traits::Result;

/// Blocks and ordered child lists, as a write authority holds them.
#[derive(Default)]
struct MemAuthority {
    blocks: HashMap<EntityUri, Block>,
    children: HashMap<EntityUri, Vec<EntityUri>>,
    /// Roots of page shares this device holds.
    share_roots: HashSet<EntityUri>,
}

impl MemAuthority {
    fn add(&mut self, id: &str, parent: &str, content: &str, props: &[(&str, &str)]) {
        let id = EntityUri::block(id);
        let parent = if parent == "root" {
            EntityUri::no_parent()
        } else {
            EntityUri::block(parent)
        };
        let mut b = Block::new_text(id.clone(), parent.clone(), content);
        for (k, v) in props {
            b.properties
                .insert(k.to_string(), Value::String(v.to_string()));
        }
        self.children.entry(parent).or_default().push(id.clone());
        self.blocks.insert(id, b);
    }

    fn tag(&mut self, id: &str, tag: &str) {
        self.blocks
            .get_mut(&EntityUri::block(id))
            .expect("tagged block exists")
            .tags
            .insert(tag.to_string());
    }
}

#[async_trait]
impl WriteAuthorityReads for MemAuthority {
    async fn block_exists(&self, id: &EntityUri) -> Result<bool> {
        Ok(self.blocks.contains_key(id))
    }

    async fn block_is_page(&self, id: &EntityUri) -> Result<bool> {
        Ok(self.blocks.get(id).is_some_and(Block::is_page))
    }

    async fn block(&self, id: &EntityUri) -> Result<Option<Block>> {
        Ok(self.blocks.get(id).cloned())
    }

    async fn subtree(&self, _: &EntityUri) -> Result<Option<Vec<Block>>> {
        Err("the shape gate reads no subtree".into())
    }

    async fn children(&self, parent: &EntityUri) -> Result<Vec<EntityUri>> {
        Ok(self.children.get(parent).cloned().unwrap_or_default())
    }

    async fn is_share_root(&self, id: &EntityUri) -> Result<bool> {
        Ok(self.share_roots.contains(id))
    }

    async fn tagged_neighbourhood(
        &self,
        tags: &[&str],
        around: &[EntityUri],
        previous_of: Option<&EntityUri>,
    ) -> Result<Option<TaggedNeighbourhood>> {
        let mut near = TaggedNeighbourhood::default();
        let mut around = around.to_vec();
        if let Some(id) = previous_of {
            let parent = &self.blocks[id].parent_id;
            let siblings = &self.children[parent];
            let at = siblings
                .iter()
                .position(|s| s == id)
                .expect("a listed child");
            if let Some(before) = at.checked_sub(1).map(|i| siblings[i].clone()) {
                around.push(before.clone());
                near.set_previous_sibling(before);
            }
        }
        let is_tagged = |id: &EntityUri| {
            self.blocks
                .get(id)
                .is_some_and(|b| tags.iter().any(|t| b.tags.contains(t)))
        };
        for id in around {
            let Some(block) = self.blocks.get(&id) else {
                near.missing(id);
                continue;
            };
            let parent = block.parent_id.clone();
            match self.blocks.get(&parent) {
                Some(p) => near.found(parent.clone(), Some(p.parent_id.clone())),
                None => near.missing(parent.clone()),
            }
            for read in [
                &id,
                &parent,
                &self
                    .blocks
                    .get(&parent)
                    .map_or(parent.clone(), |p| p.parent_id.clone()),
            ] {
                if is_tagged(read) {
                    near.tagged(read.clone());
                }
            }
            near.found(id, Some(parent));
        }
        Ok(Some(near))
    }
}

/// A page holding a plain block, an open decision with options a, b, c, and
/// a plain block after the decision.
fn vault() -> MemAuthority {
    let mut a = MemAuthority::default();
    a.add("page", "root", "page", &[]);
    a.tag("page", "Page");
    a.add("before", "page", "a plain block", &[]);
    a.add(
        "d",
        "page",
        "Which store?",
        &[("task_state", "?"), ("choose", "1")],
    );
    a.tag("d", "decision");
    a.add("d-a", "d", "Loro", &[("option", "a")]);
    a.add("d-b", "d", "Turso", &[("option", "b")]);
    a.add("d-c", "d", "Both", &[("option", "c")]);
    a.add("after", "page", "after the decision", &[]);
    a
}

fn params(pairs: &[(&str, Value)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| (Arc::from(*k), v.clone()))
        .collect()
}

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

/// A `task_state` write as the engine hands it to the gate: classified.
fn keyword(k: &str) -> Value {
    holon_api::TaskStateWrite::Set(holon_api::TaskState::from_keyword(k)).to_value()
}

fn set(id: &str, field: &str, value: Value) -> (&'static str, StorageEntity) {
    (
        "set_field",
        params(&[
            ("id", s(&format!("block:{id}"))),
            ("field", s(field)),
            ("value", value),
        ]),
    )
}

async fn judge(
    authority: MemAuthority,
    plan: &[(&'static str, StorageEntity)],
) -> std::result::Result<(), Box<ShapeRefused>> {
    let validators = ShapeValidators::new(vec![Arc::new(DecisionShape)]);
    let ops: Vec<PlanOp<'_>> = plan
        .iter()
        .map(|(op_name, params)| PlanOp { op_name, params })
        .collect();
    judge_plan(&validators, Arc::new(authority), &ops)
        .await
        .expect("the in-memory authority answers every read")
        .verdict
        .map_err(Box::new)
}

fn decide(keys: &str) -> Vec<(&'static str, StorageEntity)> {
    vec![
        set("d", "task_state", keyword("DONE")),
        set("d", "chosen", s(keys)),
        set("d", "decider", s("person:martin")),
        set("d", "decided", s("2026-09-29T10:00:00Z")),
    ]
}

#[tokio::test]
async fn a_ruling_that_names_a_non_option_is_refused_with_dc2() {
    let refused = judge(vault(), &decide("z")).await.expect_err("refused");
    assert_eq!(refused.rule, "DC2", "{refused}");
    assert!(
        !refused.message.contains("`ask`"),
        "an edit of an existing decision is not told to re-create it: {refused}"
    );
    assert_eq!(refused.root, EntityUri::block("d"));
}

#[tokio::test]
async fn a_ruling_is_legal_only_as_a_whole_plan() {
    judge(vault(), &decide("a"))
        .await
        .expect("the whole ruling is legal");
    let first_op_alone = judge(vault(), &decide("a")[..1]).await;
    assert_eq!(first_op_alone.expect_err("DONE without chosen").rule, "DC4");
}

#[tokio::test]
async fn enter_at_the_end_of_an_option_is_admitted() {
    let split = (
        "split_block",
        params(&[("id", s("block:d-a")), ("position", Value::Integer(4))]),
    );
    judge(vault(), &[split])
        .await
        .expect("a new plain child is legal");
}

#[tokio::test]
async fn enter_at_the_start_of_an_option_is_admitted() {
    let split = (
        "split_block",
        params(&[("id", s("block:d-b")), ("position", Value::Integer(0))]),
    );
    judge(vault(), &[split])
        .await
        .expect("an empty child above is legal");
}

#[tokio::test]
async fn indenting_the_next_sibling_into_a_decision_is_admitted() {
    let indent = ("indent", params(&[("id", s("block:after"))]));
    judge(vault(), &[indent])
        .await
        .expect("a plain child is legal");
}

/// A sibling of a decision whose previous sibling is a plain block is placed
/// by the authority's reads alone.
#[tokio::test]
async fn indenting_a_later_sibling_of_a_decision_is_not_simulated() {
    let mut vault = vault();
    vault.add("later", "page", "a later block", &[]);
    let indent = params(&[("id", s("block:later"))]);
    let ops = [PlanOp {
        op_name: "indent",
        params: &indent,
    }];
    let judged = judge_plan(
        &ShapeValidators::new(vec![Arc::new(DecisionShape)]),
        Arc::new(vault),
        &ops,
    )
    .await
    .expect("the in-memory authority answers every read");
    judged.verdict.expect("a plain child is legal");
    assert!(
        judged.touched.is_empty(),
        "the simulator ran over {:?}",
        judged.touched
    );
}

/// A create that tags its block through its property bag is judged whether
/// or not it names its own id.
#[tokio::test]
async fn a_create_tagged_through_its_properties_is_simulated_with_or_without_an_id() {
    let validators = ShapeValidators::new(vec![Arc::new(DecisionShape)]);
    for id in [Some("block:new"), None] {
        let mut create = params(&[
            ("parent_id", s("block:page")),
            ("content", s("Which queue?")),
            (
                "properties",
                Value::Object(HashMap::from([(
                    "tags".to_string(),
                    Value::Array(vec![s("decision")]),
                )])),
            ),
        ]);
        if let Some(id) = id {
            create.insert(Arc::from("id"), s(id));
        }
        let ops = [PlanOp {
            op_name: "create",
            params: &create,
        }];
        let judged = judge_plan(&validators, Arc::new(vault()), &ops)
            .await
            .expect("the in-memory authority answers every read");
        assert!(
            !judged.created.is_empty(),
            "the create with id {id:?} tagged through its properties was admitted without the \
             simulator"
        );
    }
}

/// The previous sibling of a block outside every decision is the decision
/// itself: the indent makes it an option, here one whose key is taken.
#[tokio::test]
async fn indenting_a_block_under_the_decision_before_it_is_judged() {
    let mut vault = vault();
    vault
        .blocks
        .get_mut(&EntityUri::block("after"))
        .expect("after")
        .properties
        .insert("option".into(), s("a"));
    let indent = ("indent", params(&[("id", s("block:after"))]));
    let refused = judge(vault, &[indent])
        .await
        .expect_err("the decision would hold option a twice");
    assert_eq!(refused.rule, "DC1", "{refused}");
}

/// A grandchild of the decision is outside it; outdenting it lands it among
/// the decision's children.
#[tokio::test]
async fn outdenting_a_grandchild_into_the_decision_is_judged() {
    let mut vault = vault();
    vault.add("d-a-note", "d-a", "a note", &[("option", "b")]);
    let outdent = ("outdent", params(&[("id", s("block:d-a-note"))]));
    let refused = judge(vault, &[outdent])
        .await
        .expect_err("the decision would hold option b twice");
    assert_eq!(refused.rule, "DC1", "{refused}");
}

#[tokio::test]
async fn deleting_the_chosen_option_of_a_decided_decision_is_refused() {
    let mut a = vault();
    for (k, v) in [
        ("task_state", "DONE"),
        ("chosen", "b"),
        ("decider", "person:martin"),
        ("decided", "2026-09-29T10:00:00Z"),
    ] {
        a.blocks
            .get_mut(&EntityUri::block("d"))
            .expect("decision")
            .properties
            .insert(k.into(), s(v));
    }
    let delete = ("delete", params(&[("id", s("block:d-b"))]));
    assert_eq!(judge(a, &[delete]).await.expect_err("refused").rule, "DC2");
}

/// `d` decided for `chosen`, with a page share placed under option `d-c`.
fn decided_with_a_share_under_c(chosen: &str) -> MemAuthority {
    let mut a = vault();
    for (k, v) in [
        ("task_state", "DONE"),
        ("chosen", chosen),
        ("decider", "person:martin"),
        ("decided", "2026-09-29T10:00:00Z"),
    ] {
        a.blocks
            .get_mut(&EntityUri::block("d"))
            .expect("decision")
            .properties
            .insert(k.into(), s(v));
    }
    a.add("shared", "d-c", "a received page", &[]);
    a.share_roots.insert(EntityUri::block("shared"));
    a
}

/// Deleting a subtree that holds a share takes the share off this device and
/// removes the subtree all the same.
#[tokio::test]
async fn deleting_the_subtree_of_a_chosen_option_holding_a_share_is_refused() {
    let delete = ("delete_subtree", params(&[("id", s("block:d-c"))]));
    let refused = judge(decided_with_a_share_under_c("c"), &[delete])
        .await
        .expect_err("the chosen option is gone");
    assert_eq!(refused.rule, "DC2", "{refused}");
}

/// Only a share's root refuses a join; a block holding one joins, carrying
/// its children along.
#[tokio::test]
async fn joining_a_chosen_option_holding_a_share_away_is_refused() {
    let join = (
        "join_block",
        params(&[("id", s("block:d-c")), ("position", Value::Integer(0))]),
    );
    let refused = judge(decided_with_a_share_under_c("c"), &[join])
        .await
        .expect_err("the chosen option is gone");
    assert_eq!(refused.rule, "DC2", "{refused}");
}

/// The op refuses to join a share's root away, so the plan writes nothing.
#[tokio::test]
async fn joining_a_shares_root_away_changes_nothing() {
    let mut a = decided_with_a_share_under_c("c");
    a.share_roots.insert(EntityUri::block("d-c"));
    let join = params(&[("id", s("block:d-c")), ("position", Value::Integer(0))]);
    let ops = [PlanOp {
        op_name: "join_block",
        params: &join,
    }];
    let judged = judge_plan(
        &ShapeValidators::new(vec![Arc::new(DecisionShape)]),
        Arc::new(a),
        &ops,
    )
    .await
    .expect("the in-memory authority answers every read");
    judged.verdict.expect("nothing is written");
    assert!(judged.touched.is_empty(), "touched {:?}", judged.touched);
}

#[tokio::test]
async fn outdenting_the_last_option_out_of_a_one_option_decision_is_refused() {
    let mut a = MemAuthority::default();
    a.add("page", "root", "page", &[]);
    a.tag("page", "Page");
    a.add("top", "page", "top", &[]);
    a.add("d", "top", "Q?", &[("task_state", "?")]);
    a.tag("d", "decision");
    a.add("d-a", "d", "only", &[("option", "a")]);
    let outdent = ("outdent", params(&[("id", s("block:d-a"))]));
    assert_eq!(judge(a, &[outdent]).await.expect_err("refused").rule, "DC1");
}

#[tokio::test]
async fn rehoming_an_option_is_judged_like_any_move() {
    let rehome = (
        "rehome_entity",
        params(&[("id", s("block:d-a")), ("target", s("holon-native"))]),
    );
    judge(vault(), &[rehome]).await.expect("two options remain");
}

#[tokio::test]
async fn tagging_a_plain_block_decision_is_refused() {
    let tag = (
        "set_field",
        params(&[
            ("id", s("block:before")),
            ("field", s("tags")),
            ("value", Value::Array(vec![s("decision")])),
        ]),
    );
    let refused = judge(vault(), &[tag]).await.expect_err("refused");
    assert_eq!(refused.rule, "B4");
    assert!(
        refused.message.contains("in one `dense_patch` batch"),
        "a hand-tagged decision is refused with the way to make one: {refused}"
    );
}

#[tokio::test]
async fn tagging_a_question_that_already_has_its_options_is_admitted() {
    let mut authority = vault();
    authority.add(
        "q",
        "page",
        "Which DB?",
        &[("task_state", "?"), ("choose", "1")],
    );
    authority.add("q-a", "q", "Postgres", &[("option", "a")]);
    authority.add("q-b", "q", "SQLite", &[("option", "b")]);
    let tag = (
        "set_field",
        params(&[
            ("id", s("block:q")),
            ("field", s("tags")),
            ("value", Value::Array(vec![s("decision")])),
        ]),
    );
    judge(authority, &[tag])
        .await
        .expect("a block that already has the whole shape may be tagged");
}

/// An op the simulator cannot apply is never admitted, even far from any
/// tagged block: its result is unknown.
#[tokio::test]
async fn an_op_the_simulator_cannot_apply_is_an_error() {
    let validators = ShapeValidators::registered();
    let place = params(&[
        ("id", s("block:after")),
        ("parent_id", s("block:page")),
        ("sort_key", s("a0")),
    ]);
    let ops = [PlanOp {
        op_name: "place",
        params: &place,
    }];
    let result = judge_plan(&validators, Arc::new(vault()), &ops).await;
    let err = result.err().expect("an unsimulated op is an error");
    assert!(err.to_string().contains("could not be simulated"), "{err}");
}

/// `MemAuthority` without the neighbourhood read, as the Loro authority
/// answers: the gate simulates every plan that is not text-only.
struct Unplaced(MemAuthority);

#[async_trait]
impl WriteAuthorityReads for Unplaced {
    async fn block_exists(&self, id: &EntityUri) -> Result<bool> {
        self.0.block_exists(id).await
    }

    async fn block_is_page(&self, id: &EntityUri) -> Result<bool> {
        self.0.block_is_page(id).await
    }

    async fn block(&self, id: &EntityUri) -> Result<Option<Block>> {
        self.0.block(id).await
    }

    async fn subtree(&self, root: &EntityUri) -> Result<Option<Vec<Block>>> {
        self.0.subtree(root).await
    }

    async fn children(&self, parent: &EntityUri) -> Result<Vec<EntityUri>> {
        self.0.children(parent).await
    }
}

/// A structural op on a block the write authority does not hold is refused
/// by the gate with the authority's own typed error (D64.b), as the block
/// providers refuse it, not simulated over an absent block.
#[tokio::test]
async fn a_structural_op_on_a_block_the_authority_does_not_hold_is_refused_by_name() {
    let validators = ShapeValidators::registered();
    let stranded = EntityUri::block("stranded");
    let split = params(&[("id", s("block:stranded")), ("position", Value::Integer(3))]);
    let indent = params(&[("id", s("block:stranded"))]);
    for (op_name, params) in [("split_block", &split), ("indent", &indent)] {
        let ops = [PlanOp { op_name, params }];
        let err = judge_plan(&validators, Arc::new(Unplaced(vault())), &ops)
            .await
            .err()
            .unwrap_or_else(|| panic!("{op_name} on a block the authority lacks is refused"));
        assert_eq!(
            err.downcast_ref::<holon_core::BlockNotInWriteAuthority>(),
            Some(&holon_core::BlockNotInWriteAuthority::new(
                stranded.clone(),
                holon_core::ProjectionRead::NotRead,
            )),
            "{op_name}: {err}"
        );
    }
}

#[tokio::test]
async fn removing_the_decision_tag_is_admitted() {
    let untag = (
        "remove_tag",
        params(&[("id", s("block:d")), ("tag", s("decision"))]),
    );
    judge(vault(), &[untag])
        .await
        .expect("an untagged block has no shape");
}

#[tokio::test]
async fn an_answer_with_a_blank_body_is_legal() {
    let create = (
        "create",
        params(&[
            ("id", s("block:d-s1")),
            ("parent_id", s("block:d")),
            ("content", s("Answer\n   ")),
            (
                "properties",
                Value::Object(HashMap::from([
                    ("answerer".to_string(), s("model:jev-1")),
                    ("pick".to_string(), s("a")),
                    ("answered".to_string(), s("2026-09-29T10:00:00Z")),
                ])),
            ),
        ]),
    );
    judge(vault(), &[create])
        .await
        .expect("a blank body is no rationale");
}

/// Wall-clock cost of the gate per write, with an in-memory authority, so
/// the number is the simulator's own overhead. Run in RELEASE:
/// `cargo test -p holon-core --release --test shape_gate -- --ignored
/// --nocapture`.
#[tokio::test]
#[ignore]
async fn gate_cost_per_write() {
    let mut base = vault();
    for i in 0..2000 {
        base.add(&format!("n{i}"), "page", "a note", &[]);
    }
    let base = Arc::new(base);
    let validators = ShapeValidators::new(vec![Arc::new(DecisionShape)]);
    let cases: Vec<(&str, Vec<(&'static str, StorageEntity)>)> = vec![
        (
            "set_field on a plain block",
            vec![set("n1000", "content", s("x"))],
        ),
        (
            "indent of a plain block",
            vec![("indent", params(&[("id", s("block:n1000"))]))],
        ),
        (
            "set_field(task_state) on a plain block",
            vec![set("n1000", "task_state", keyword("TODO"))],
        ),
        (
            "set_field on the decision",
            vec![set("d", "choose", s("1"))],
        ),
        (
            "split inside an option",
            vec![(
                "split_block",
                params(&[("id", s("block:d-a")), ("position", Value::Integer(2))]),
            )],
        ),
    ];
    for (name, plan) in cases {
        let ops: Vec<PlanOp<'_>> = plan
            .iter()
            .map(|(op_name, params)| PlanOp { op_name, params })
            .collect();
        let mut samples = Vec::new();
        for _ in 0..500 {
            let t = Instant::now();
            let j = judge_plan(&validators, base.clone(), &ops)
                .await
                .expect("reads");
            samples.push(t.elapsed());
            j.verdict.expect("legal");
        }
        samples.sort();
        eprintln!(
            "[shape-gate cost] {name}: p50={:?} p95={:?} max={:?} (500 samples)",
            samples[250], samples[475], samples[499]
        );
    }
}

#[path = "shape_gate/claims.rs"]
mod claims;
