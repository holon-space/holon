//! Smoke test for `WidgetSnapshot` + the renderer-required invariant
//! pattern. Mirrors `inv-viewmodel-state-toggle-correct` shape against
//! a hand-crafted snapshot — proves the IR carries enough data for
//! cross-slice invariant evaluation.

use std::collections::BTreeMap;

use holon_pbt_core::capabilities::{
    CapCursor, CapRegion, EntityUri, RefBlockTree, RefTaskState, SutLoroTaskState, SutRenderer,
    SutSqlProjection, WidgetSnapshot,
};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

fn make_state_toggle(entity_id: &str, current: &str, has_set_field: bool) -> WidgetSnapshot {
    let mut props = BTreeMap::new();
    props.insert("field".into(), "task_state".into());
    props.insert("current".into(), current.into());
    props.insert("label".into(), "Open".into());
    props.insert("states".into(), "[\"TODO\",\"DONE\"]".into());
    let operations = if has_set_field {
        vec!["set_field:task_state:value".into()]
    } else {
        vec![]
    };
    WidgetSnapshot {
        kind: "state_toggle".into(),
        entity_id: Some(entity_id.into()),
        props,
        operations,
        children: vec![],
    }
}

fn root_with(children: Vec<WidgetSnapshot>) -> WidgetSnapshot {
    WidgetSnapshot {
        kind: "column".into(),
        entity_id: None,
        props: BTreeMap::new(),
        operations: vec![],
        children,
    }
}

// ── Toy ref + SUT mirroring the production trait shape ───────────────

struct ToyRef {
    task_states: std::collections::HashMap<EntityUri, String>,
}

impl RefBlockTree for ToyRef {
    fn block_content(&self, id: &EntityUri) -> Option<&str> {
        self.task_states.get(id).map(String::as_str)
    }
    fn is_text_block(&self, _: &EntityUri) -> bool {
        true
    }
    fn main_editable_descendants(&self) -> Vec<EntityUri> {
        vec![]
    }
    fn focus_root_ids(&self, _: CapRegion) -> std::collections::BTreeSet<EntityUri> {
        Default::default()
    }
    fn previous_sibling(&self, _: &EntityUri) -> Option<EntityUri> {
        None
    }
    fn next_sibling(&self, _: &EntityUri) -> Option<EntityUri> {
        None
    }
    fn parent_of(&self, _: &EntityUri) -> Option<EntityUri> {
        None
    }
    fn grandparent(&self, _: &EntityUri) -> Option<EntityUri> {
        None
    }
    fn sorted_children(&self, _: &EntityUri) -> Vec<EntityUri> {
        vec![]
    }
    fn is_descendant_of_any(
        &self,
        _: &EntityUri,
        _: &std::collections::BTreeSet<EntityUri>,
    ) -> bool {
        false
    }
    fn is_layout_block(&self, _: &EntityUri) -> bool {
        false
    }
    fn is_focusable(&self, _: &EntityUri) -> bool {
        true
    }
    fn is_no_content_update(&self, _: &EntityUri) -> bool {
        false
    }
    fn is_page_block(&self, _: &EntityUri) -> bool {
        false
    }
    fn all_non_seed_block_ids(&self) -> std::collections::BTreeSet<EntityUri> {
        Default::default()
    }
}

impl RefTaskState for ToyRef {
    fn task_state_of(&self, id: &EntityUri) -> Option<String> {
        self.task_states.get(id).cloned()
    }
}

struct ToySut {
    root: WidgetSnapshot,
}

#[allow(async_fn_in_trait)]
impl SutRenderer for ToySut {
    async fn render_tree_of(&self, _: &EntityUri) -> Option<String> {
        None
    }
    async fn widget_tree_snapshot(&self) -> WidgetSnapshot {
        self.root.clone()
    }
    async fn root_data_row_ids(&self) -> std::collections::BTreeSet<EntityUri> {
        Default::default()
    }
    async fn widget_tree_for(&self, _: &EntityUri) -> Option<WidgetSnapshot> {
        None
    }
    async fn root_content_comparison(&self, _: &[String]) -> Option<(Vec<String>, Vec<String>)> {
        None
    }
    async fn root_render_ready(&self) -> bool {
        false
    }
}

#[allow(async_fn_in_trait)]
impl SutSqlProjection for ToySut {
    async fn block_row(&self, _: &EntityUri) -> Option<Vec<String>> {
        None
    }
    async fn current_focus_rows(&self) -> Vec<(String, Option<String>)> {
        Vec::new()
    }
    async fn focus_roots_rows(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    async fn nav_history_open_rows(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    async fn all_block_ids(&self) -> std::collections::BTreeSet<EntityUri> {
        Default::default()
    }
    async fn sorted_children(&self, _: &EntityUri) -> Vec<EntityUri> {
        vec![]
    }
    async fn watch_row_count(&self, _: &str) -> Option<usize> {
        None
    }
    async fn block_raw_row(&self, _: &EntityUri) -> Option<Vec<String>> {
        None
    }
    async fn block_tag_block_ids(&self) -> std::collections::BTreeSet<EntityUri> {
        Default::default()
    }
    async fn block_task_state(&self, _: &EntityUri) -> Option<String> {
        None
    }
    async fn block_content(&self, _: &EntityUri) -> Option<String> {
        None
    }
}

#[allow(async_fn_in_trait)]
impl SutLoroTaskState for ToySut {
    async fn loro_task_state_of(&self, _: &str) -> Option<String> {
        None
    }
}

// ── Mirror of the production invariant ───────────────────────────────

struct InvStateToggle;

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvStateToggle
where
    R: RefBlockTree + RefTaskState,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        InvariantId("inv-state-toggle-toy")
    }
    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let root = sut.widget_tree_snapshot().await;
        for node in root.walk() {
            if node.kind != "state_toggle" {
                continue;
            }
            let Some(block_id_str) = node.entity_id.as_ref() else {
                return InvariantResult::Fail("no entity id".into());
            };
            let block_id = EntityUri::parse(block_id_str)
                .expect("widget snapshot entity_id must be a valid EntityUri");
            if ref_.block_content(&block_id).is_none() {
                continue;
            }
            let expected = ref_.task_state_of(&block_id).unwrap_or_default();
            let current = node.props.get("current").map(String::as_str).unwrap_or("");
            if current != expected {
                return InvariantResult::Fail(format!(
                    "current '{current}' != expected '{expected}' for {block_id}"
                ));
            }
            if !expected.is_empty() && node.find_op("set_field:task_state:").is_none() {
                return InvariantResult::Fail(format!("no set_field op for {block_id}"));
            }
        }
        InvariantResult::Ok
    }
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    use std::pin::pin;
    use std::task::{Context, Waker};

    let waker = Waker::noop();
    let mut ctx = Context::from_waker(waker);
    let mut fut = pin!(f);
    loop {
        if let std::task::Poll::Ready(v) = fut.as_mut().poll(&mut ctx) {
            return v;
        }
    }
}

#[test]
fn state_toggle_correct_passes_on_matching_state() {
    let ref_state = ToyRef {
        task_states: [(EntityUri::parse("block:a").unwrap(), "TODO".to_string())]
            .into_iter()
            .collect(),
    };
    let sut = ToySut {
        root: root_with(vec![make_state_toggle("block:a", "TODO", true)]),
    };
    let result = block_on(InvStateToggle.check(&ref_state, &sut));
    assert!(matches!(result, InvariantResult::Ok), "got {result:?}");
}

#[test]
fn state_toggle_fails_on_state_drift() {
    let ref_state = ToyRef {
        task_states: [(EntityUri::parse("block:a").unwrap(), "DONE".to_string())]
            .into_iter()
            .collect(),
    };
    let sut = ToySut {
        root: root_with(vec![make_state_toggle("block:a", "TODO", true)]),
    };
    let result = block_on(InvStateToggle.check(&ref_state, &sut));
    assert!(
        matches!(result, InvariantResult::Fail(_)),
        "expected Fail, got {result:?}"
    );
}

#[test]
fn state_toggle_fails_on_missing_set_field_op() {
    let ref_state = ToyRef {
        task_states: [(EntityUri::parse("block:a").unwrap(), "TODO".to_string())]
            .into_iter()
            .collect(),
    };
    let sut = ToySut {
        root: root_with(vec![make_state_toggle("block:a", "TODO", false)]),
    };
    let result = block_on(InvStateToggle.check(&ref_state, &sut));
    assert!(
        matches!(result, InvariantResult::Fail(_)),
        "expected Fail, got {result:?}"
    );
}

#[test]
fn cursor_default_is_origin() {
    let c = CapCursor::default();
    assert_eq!(c.line, 0);
    assert_eq!(c.column, 0);
}

#[test]
fn walk_visits_pre_order() {
    let leaf = WidgetSnapshot {
        kind: "leaf".into(),
        entity_id: None,
        props: BTreeMap::new(),
        operations: vec![],
        children: vec![],
    };
    let parent = WidgetSnapshot {
        kind: "parent".into(),
        entity_id: None,
        props: BTreeMap::new(),
        operations: vec![],
        children: vec![leaf.clone(), leaf.clone()],
    };
    let kinds: Vec<&str> = parent.walk().map(|n| n.kind.as_str()).collect();
    assert_eq!(kinds, vec!["parent", "leaf", "leaf"]);
}
