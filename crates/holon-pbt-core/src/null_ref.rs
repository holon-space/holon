//! Mechanical discovery of the class-1 (self-consistency) invariants.
//!
//! [`NullRef`] implements every `Ref*` capability the composed catalog binds
//! on, with a panicking body. Run the catalog against [`null_ref_caps`] and an
//! invariant that completes never consulted the reference model — class 1. One
//! that panics is class 2, and the panic message names the exact ref method it
//! read.

use std::collections::BTreeSet;
use std::sync::Arc;

use holon_api::EntityUri;

use crate::capabilities::AdviceExpectation;
use crate::capabilities::Audience;
use crate::capabilities::CapCursor;
use crate::capabilities::CapRegion;
use crate::capabilities::RefAdvice;
use crate::capabilities::RefAudience;
use crate::capabilities::RefBackend;
use crate::capabilities::RefBlockTree;
use crate::capabilities::RefClock;
use crate::capabilities::RefEditorMirror;
use crate::capabilities::RefFocus;
use crate::capabilities::RefGlobalFocus;
use crate::capabilities::RefHistoryExpectation;
use crate::capabilities::RefJournalFeed;
use crate::capabilities::RefLayout;
use crate::capabilities::RefNavHistory;
use crate::capabilities::RefSharedView;
use crate::capabilities::RefTaskState;
use crate::capabilities::RefToggle;
use crate::capabilities::RefTypedEntities;
use crate::capabilities::RefUndoRedoBurned;
use crate::capabilities::RefViewSelection;
use crate::capabilities::RefWatch;
use crate::capabilities::WatchRow;
use crate::composition::CapId;
use crate::composition::CapMap;

pub struct NullRef;

/// The trait names `NullRef` answers for — the ref caps the classifier
/// can host. Asserted by the classifier test so a newly registered ref
/// capability cannot silently escape classification.
pub const NULL_REF_CAPS: [&str; 19] = [
    "RefAdvice",
    "RefAudience",
    "RefBackend",
    "RefBlockTree",
    "RefClock",
    "RefEditorMirror",
    "RefFocus",
    "RefGlobalFocus",
    "RefHistoryExpectation",
    "RefJournalFeed",
    "RefLayout",
    "RefNavHistory",
    "RefSharedView",
    "RefTaskState",
    "RefToggle",
    "RefTypedEntities",
    "RefUndoRedoBurned",
    "RefViewSelection",
    "RefWatch",
];

#[allow(unused_variables)]
impl RefAdvice for NullRef {
    fn advice_expectation(&self, anchor: &str) -> AdviceExpectation {
        panic!("class-2: invariant read RefAdvice::advice_expectation")
    }
    fn advice_matview_rows(&self) -> Vec<(String, String, u32)> {
        panic!("class-2: invariant read RefAdvice::advice_matview_rows")
    }
    fn advice_matview_name(&self) -> Option<String> {
        panic!("class-2: invariant read RefAdvice::advice_matview_name")
    }
}

#[allow(unused_variables)]
impl RefAudience for NullRef {
    fn audience_epoch(&self) -> u64 {
        panic!("class-2: invariant read RefAudience::audience_epoch")
    }
    fn shared_block_ids(&self) -> BTreeSet<EntityUri> {
        panic!("class-2: invariant read RefAudience::shared_block_ids")
    }
    fn block_policy_audience(&self, block: &EntityUri) -> Audience {
        panic!("class-2: invariant read RefAudience::block_policy_audience")
    }
    fn block_effective_audience(&self, block: &EntityUri) -> Audience {
        panic!("class-2: invariant read RefAudience::block_effective_audience")
    }
}

#[allow(unused_variables)]
impl RefBackend for NullRef {
    fn non_seed_blocks(&self) -> Vec<holon_api::Block> {
        panic!("class-2: invariant read RefBackend::non_seed_blocks")
    }
    fn seed_block_ids(&self) -> BTreeSet<EntityUri> {
        panic!("class-2: invariant read RefBackend::seed_block_ids")
    }
    fn org_blocks(&self) -> Vec<holon_api::Block> {
        panic!("class-2: invariant read RefBackend::org_blocks")
    }
}

#[allow(unused_variables)]
impl RefBlockTree for NullRef {
    fn block_content(&self, id: &EntityUri) -> Option<&str> {
        panic!("class-2: invariant read RefBlockTree::block_content")
    }
    fn is_text_block(&self, id: &EntityUri) -> bool {
        panic!("class-2: invariant read RefBlockTree::is_text_block")
    }
    fn main_editable_descendants(&self) -> Vec<EntityUri> {
        panic!("class-2: invariant read RefBlockTree::main_editable_descendants")
    }
    fn focus_root_ids(&self, region: CapRegion) -> BTreeSet<EntityUri> {
        panic!("class-2: invariant read RefBlockTree::focus_root_ids")
    }
    fn previous_sibling(&self, id: &EntityUri) -> Option<EntityUri> {
        panic!("class-2: invariant read RefBlockTree::previous_sibling")
    }
    fn next_sibling(&self, id: &EntityUri) -> Option<EntityUri> {
        panic!("class-2: invariant read RefBlockTree::next_sibling")
    }
    fn parent_of(&self, id: &EntityUri) -> Option<EntityUri> {
        panic!("class-2: invariant read RefBlockTree::parent_of")
    }
    fn grandparent(&self, id: &EntityUri) -> Option<EntityUri> {
        panic!("class-2: invariant read RefBlockTree::grandparent")
    }
    fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri> {
        panic!("class-2: invariant read RefBlockTree::sorted_children")
    }
    fn is_descendant_of_any(&self, id: &EntityUri, ancestors: &BTreeSet<EntityUri>) -> bool {
        panic!("class-2: invariant read RefBlockTree::is_descendant_of_any")
    }
    fn main_panel_renders(&self, id: &EntityUri) -> bool {
        panic!("class-2: invariant read RefBlockTree::main_panel_renders")
    }
    fn is_layout_block(&self, id: &EntityUri) -> bool {
        panic!("class-2: invariant read RefBlockTree::is_layout_block")
    }
    fn is_focusable(&self, id: &EntityUri) -> bool {
        panic!("class-2: invariant read RefBlockTree::is_focusable")
    }
    fn is_no_content_update(&self, id: &EntityUri) -> bool {
        panic!("class-2: invariant read RefBlockTree::is_no_content_update")
    }
    fn is_page_block(&self, id: &EntityUri) -> bool {
        panic!("class-2: invariant read RefBlockTree::is_page_block")
    }
    fn is_source_block(&self, id: &EntityUri) -> bool {
        panic!("class-2: invariant read RefBlockTree::is_source_block")
    }
    fn all_non_seed_block_ids(&self) -> BTreeSet<EntityUri> {
        panic!("class-2: invariant read RefBlockTree::all_non_seed_block_ids")
    }
}

#[allow(unused_variables)]
impl RefClock for NullRef {
    fn today(&self) -> String {
        panic!("class-2: invariant read RefClock::today")
    }
    fn expected_journal_day_count(&self) -> usize {
        panic!("class-2: invariant read RefClock::expected_journal_day_count")
    }
    fn visited_days(&self) -> BTreeSet<String> {
        panic!("class-2: invariant read RefClock::visited_days")
    }
}

#[allow(unused_variables)]
impl RefEditorMirror for NullRef {
    fn active_editor_block(&self) -> Option<EntityUri> {
        panic!("class-2: invariant read RefEditorMirror::active_editor_block")
    }
    fn active_editor_text(&self) -> Option<&str> {
        panic!("class-2: invariant read RefEditorMirror::active_editor_text")
    }
    fn active_editor_cursor(&self) -> Option<usize> {
        panic!("class-2: invariant read RefEditorMirror::active_editor_cursor")
    }
    fn active_editor_dirty(&self) -> bool {
        panic!("class-2: invariant read RefEditorMirror::active_editor_dirty")
    }
}

#[allow(unused_variables)]
impl RefFocus for NullRef {
    fn navigation_focus_rows(&self) -> Vec<(String, Option<String>)> {
        panic!("class-2: invariant read RefFocus::navigation_focus_rows")
    }
    fn expected_focus_root_rows(&self) -> Vec<(String, Vec<String>)> {
        panic!("class-2: invariant read RefFocus::expected_focus_root_rows")
    }
    fn current_focus(&self, region: CapRegion) -> Option<EntityUri> {
        panic!("class-2: invariant read RefFocus::current_focus")
    }
    fn focused_cursor(&self, region: CapRegion) -> Option<CapCursor> {
        panic!("class-2: invariant read RefFocus::focused_cursor")
    }
}

#[allow(unused_variables)]
impl RefGlobalFocus for NullRef {
    fn global_focused_block(&self) -> Option<EntityUri> {
        panic!("class-2: invariant read RefGlobalFocus::global_focused_block")
    }
}

#[allow(unused_variables)]
impl RefHistoryExpectation for NullRef {
    fn ever_created_ids(&self) -> BTreeSet<EntityUri> {
        panic!("class-2: invariant read RefHistoryExpectation::ever_created_ids")
    }
    fn min_recorded_op_groups(&self) -> usize {
        panic!("class-2: invariant read RefHistoryExpectation::min_recorded_op_groups")
    }
}

#[allow(unused_variables)]
impl RefJournalFeed for NullRef {
    fn feed_day_pages(&self) -> Vec<EntityUri> {
        panic!("class-2: invariant read RefJournalFeed::feed_day_pages")
    }
}

#[allow(unused_variables)]
impl RefNavHistory for NullRef {
    fn can_go_back(&self, region: holon_api::Region) -> bool {
        panic!("class-2: invariant read RefNavHistory::can_go_back")
    }
    fn can_go_forward(&self, region: holon_api::Region) -> bool {
        panic!("class-2: invariant read RefNavHistory::can_go_forward")
    }
    fn predicts_navigation_focus(&self, block_id: &EntityUri, region: holon_api::Region) -> bool {
        panic!("class-2: invariant read RefNavHistory::predicts_navigation_focus")
    }
    fn predicted_sidebar_navigation_targets(&self) -> Vec<EntityUri> {
        panic!("class-2: invariant read RefNavHistory::predicted_sidebar_navigation_targets")
    }
    fn drawer_is_open(&self, panel_id: &str) -> bool {
        panic!("class-2: invariant read RefNavHistory::drawer_is_open")
    }
}

#[allow(unused_variables)]
impl RefTypedEntities for NullRef {
    fn typed_entity_schemas(&self) -> Vec<(String, Vec<String>)> {
        panic!("class-2: invariant read RefTypedEntities::typed_entity_schemas")
    }
    fn expected_typed_entity_rows(&self, type_name: &str) -> Vec<Vec<String>> {
        panic!("class-2: invariant read RefTypedEntities::expected_typed_entity_rows")
    }
    fn typed_entity_ids(&self) -> BTreeSet<String> {
        panic!("class-2: invariant read RefTypedEntities::typed_entity_ids")
    }
}

#[allow(unused_variables)]
impl RefLayout for NullRef {
    fn layout_block_ids(&self) -> BTreeSet<EntityUri> {
        panic!("class-2: invariant read RefLayout::layout_block_ids")
    }
    fn profile_block_ids(&self) -> BTreeSet<EntityUri> {
        panic!("class-2: invariant read RefLayout::profile_block_ids")
    }
    fn has_blocks_profile(&self) -> bool {
        panic!("class-2: invariant read RefLayout::has_blocks_profile")
    }
    fn has_user_index_org(&self) -> bool {
        panic!("class-2: invariant read RefLayout::has_user_index_org")
    }
    fn all_block_ids(&self) -> BTreeSet<EntityUri> {
        panic!("class-2: invariant read RefLayout::all_block_ids")
    }
    fn expected_visible_content_ids(&self, region: CapRegion) -> BTreeSet<EntityUri> {
        panic!("class-2: invariant read RefLayout::expected_visible_content_ids")
    }
    fn has_user_documents(&self) -> bool {
        panic!("class-2: invariant read RefLayout::has_user_documents")
    }
    fn region_entity_focused(&self, region: CapRegion) -> bool {
        panic!("class-2: invariant read RefLayout::region_entity_focused")
    }
}

#[allow(unused_variables)]
impl RefSharedView for NullRef {
    fn is_shared(&self) -> bool {
        panic!("class-2: invariant read RefSharedView::is_shared")
    }
    fn receiver_principal(&self) -> String {
        panic!("class-2: invariant read RefSharedView::receiver_principal")
    }
    fn owner_to_receiver_rounds(&self) -> u64 {
        panic!("class-2: invariant read RefSharedView::owner_to_receiver_rounds")
    }
    fn shared_audience(&self) -> Audience {
        panic!("class-2: invariant read RefSharedView::shared_audience")
    }
}

#[allow(unused_variables)]
impl RefTaskState for NullRef {
    fn task_state_of(&self, id: &EntityUri) -> Option<String> {
        panic!("class-2: invariant read RefTaskState::task_state_of")
    }
}

#[allow(unused_variables)]
impl RefToggle for NullRef {
    fn is_expanded(&self, id: &EntityUri) -> bool {
        panic!("class-2: invariant read RefToggle::is_expanded")
    }
}

#[allow(unused_variables)]
impl RefUndoRedoBurned for NullRef {
    fn burned_block_ids(&self) -> BTreeSet<EntityUri> {
        panic!("class-2: invariant read RefUndoRedoBurned::burned_block_ids")
    }
}

#[allow(unused_variables)]
impl RefViewSelection for NullRef {
    fn current_view(&self) -> String {
        panic!("class-2: invariant read RefViewSelection::current_view")
    }
    fn active_render_expr_name(&self, region: CapRegion) -> Option<String> {
        panic!("class-2: invariant read RefViewSelection::active_render_expr_name")
    }
    fn root_render_expr_name(&self) -> Option<String> {
        panic!("class-2: invariant read RefViewSelection::root_render_expr_name")
    }
    fn has_root_render_expr(&self) -> bool {
        panic!("class-2: invariant read RefViewSelection::has_root_render_expr")
    }
    fn root_visible_columns(&self) -> Vec<String> {
        panic!("class-2: invariant read RefViewSelection::root_visible_columns")
    }
    fn main_panel_block_id(&self) -> Option<EntityUri> {
        panic!("class-2: invariant read RefViewSelection::main_panel_block_id")
    }
    fn main_panel_render_expr_name(&self) -> Option<String> {
        panic!("class-2: invariant read RefViewSelection::main_panel_render_expr_name")
    }
}

#[allow(unused_variables)]
impl RefWatch for NullRef {
    fn active_watch_ids(&self) -> Vec<String> {
        panic!("class-2: invariant read RefWatch::active_watch_ids")
    }
    fn expected_watch_rows(&self, query_id: &str) -> Vec<WatchRow> {
        panic!("class-2: invariant read RefWatch::expected_watch_rows")
    }
    fn watch_query_columns(&self, query_id: &str) -> Vec<String> {
        panic!("class-2: invariant read RefWatch::watch_query_columns")
    }
}

/// A reference `CapMap` in which every `Ref*` capability is answered by
/// `NullRef`. Running the invariant catalog against it discovers the
/// class-1 (self-consistency) set: the invariants that complete.
pub fn null_ref_caps() -> CapMap {
    let nr = Arc::new(NullRef);
    let mut caps = CapMap::default();
    caps.insert(nr.clone() as Arc<dyn RefAdvice>);
    caps.insert(nr.clone() as Arc<dyn RefAudience>);
    caps.insert(nr.clone() as Arc<dyn RefBackend>);
    caps.insert(nr.clone() as Arc<dyn RefBlockTree>);
    caps.insert(nr.clone() as Arc<dyn RefClock>);
    caps.insert(nr.clone() as Arc<dyn RefEditorMirror>);
    caps.insert(nr.clone() as Arc<dyn RefFocus>);
    caps.insert(nr.clone() as Arc<dyn RefGlobalFocus>);
    caps.insert(nr.clone() as Arc<dyn RefHistoryExpectation>);
    caps.insert(nr.clone() as Arc<dyn RefJournalFeed>);
    caps.insert(nr.clone() as Arc<dyn RefLayout>);
    caps.insert(nr.clone() as Arc<dyn RefNavHistory>);
    caps.insert(nr.clone() as Arc<dyn RefSharedView>);
    caps.insert(nr.clone() as Arc<dyn RefTaskState>);
    caps.insert(nr.clone() as Arc<dyn RefToggle>);
    caps.insert(nr.clone() as Arc<dyn RefTypedEntities>);
    caps.insert(nr.clone() as Arc<dyn RefUndoRedoBurned>);
    caps.insert(nr.clone() as Arc<dyn RefViewSelection>);
    caps.insert(nr as Arc<dyn RefWatch>);
    caps
}

pub fn null_ref_cap_ids() -> Vec<CapId> {
    vec![
        CapId::of::<dyn RefAdvice>(),
        CapId::of::<dyn RefAudience>(),
        CapId::of::<dyn RefBackend>(),
        CapId::of::<dyn RefBlockTree>(),
        CapId::of::<dyn RefClock>(),
        CapId::of::<dyn RefEditorMirror>(),
        CapId::of::<dyn RefFocus>(),
        CapId::of::<dyn RefGlobalFocus>(),
        CapId::of::<dyn RefHistoryExpectation>(),
        CapId::of::<dyn RefJournalFeed>(),
        CapId::of::<dyn RefLayout>(),
        CapId::of::<dyn RefNavHistory>(),
        CapId::of::<dyn RefSharedView>(),
        CapId::of::<dyn RefTaskState>(),
        CapId::of::<dyn RefToggle>(),
        CapId::of::<dyn RefTypedEntities>(),
        CapId::of::<dyn RefUndoRedoBurned>(),
        CapId::of::<dyn RefViewSelection>(),
        CapId::of::<dyn RefWatch>(),
    ]
}
