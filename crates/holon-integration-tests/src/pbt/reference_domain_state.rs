//! Tier-1 domain fragment of the PBT reference model (ADR 0004 / 0005).
//!
//! `ReferenceDomainState` is the **PBT reference oracle for the domain**, not
//! the production domain itself. The production domain stays a *logical
//! canonical projection* — the consensus across wired adapters after
//! quiescence; no struct "holds" it (ADR-0004 canonicity reframing). This
//! struct isolates the sort_key-free, adapter-independent domain data so it can
//! be the single fragment shared across all wirings.
//!
//! @pbt kind ref
//! @pbt covers domain-truth — single-homed Tier-1 domain fragment (block tree,
//!   layout/profile classification, author-intent render config, seed profile).
//!   The one owner of these facts; the actor/UI/loro fragments are read-only
//!   lenses over it. No datum is double-homed here.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

use holon_api::ContentType;
use holon_api::EntityName;
use holon_api::Region;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::render_types::RenderExpr;

use super::block_state::BlockState;
use super::block_state::LayoutBlockInfo;
use super::query::WatchSpec;

/// Tier-1 domain data extracted from `ReferenceState` (ADR 0004 Phase 2).
///
/// Holds only the canonical, adapter-independent domain facts: the block tree,
/// the author-intent render config, the layout/profile classification, and the
/// seed profile. Viewer prefs, navigation, focus, watches and undo/redo stay on
/// `ReferenceState` (they are actor/UI concerns, split out in later phases).
#[derive(Debug, Clone)]
pub struct ReferenceDomainState {
    /// Block data affected by undo/redo
    pub block_state: BlockState,

    /// Typed layout block classification for index.org.
    pub layout_blocks: LayoutBlockInfo,

    /// Profile block IDs (blocks with source_language =
    /// holon_entity_profile_yaml)
    pub profile_block_ids: HashSet<EntityUri>,

    /// Current active profile YAML index per entity_name.
    pub active_profiles: HashMap<EntityName, (EntityUri, usize)>,

    /// Active render expressions per render source block (block_id →
    /// RenderExpr). Updated when render source blocks are created or
    /// mutated. `BTreeMap` for deterministic iteration (see
    /// `BlockState::blocks`).
    pub render_expressions: BTreeMap<EntityUri, RenderExpr>,

    /// Parsed entity profile from the seed YAML (or custom org file).
    /// Used by `BuilderServices::resolve_profile` for ViewModel construction.
    pub seed_profile: Option<holon::entity_profile::EntityProfile>,

    /// Block entity operations (set_field, create, update, delete,
    /// cycle_task_state). Used by `BuilderServices::resolve_profile` to
    /// inject operations into RowProfile.
    pub block_operations: Vec<holon_api::render_types::OperationDescriptor>,
}

impl ReferenceDomainState {
    pub fn new() -> Self {
        Self {
            block_state: BlockState {
                blocks: BTreeMap::new(),
                block_documents: BTreeMap::new(),
                next_id: 0,
            },
            layout_blocks: LayoutBlockInfo::default(),
            profile_block_ids: HashSet::new(),
            active_profiles: HashMap::new(),
            render_expressions: BTreeMap::new(),
            seed_profile: None,
            block_operations: default_block_operations(),
        }
    }

    /// Block IDs whose `content` must NEVER be mutated by an edit transition:
    /// query / render source blocks (would corrupt the active layout) and
    /// entity-profile blocks (typed YAML, not free-form text).
    pub fn no_content_update_set(&self) -> HashSet<EntityUri> {
        self.layout_blocks
            .render_source_ids
            .iter()
            .chain(self.layout_blocks.query_source_ids.iter())
            .chain(self.profile_block_ids.iter())
            .cloned()
            .collect()
    }

    pub fn has_blocks_profile(&self) -> bool {
        self.active_profiles.contains_key("block")
    }

    /// Whether the system has a valid root layout (from seed blocks or
    /// user-written index.org). Used to gate render_entity, ReactiveEngine,
    /// and ViewModel checks.
    pub fn is_properly_setup(&self) -> bool {
        !self.layout_blocks.query_source_ids.is_empty() || self.has_user_index_org()
    }

    /// Whether the user has written an index.org with query+render blocks.
    /// Used to gate block comparison invariants (seed blocks don't round-trip
    /// through org files).
    pub fn has_user_index_org(&self) -> bool {
        let index_doc_uri = match self.block_state.doc_uri_by_name("index") {
            Some(uri) => uri,
            None => return false,
        };

        let root_blocks: Vec<&Block> = self
            .block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == index_doc_uri)
            .collect();

        root_blocks.iter().any(|root_block| {
            self.block_state.blocks.values().any(|child| {
                child.parent_id == root_block.id
                    && child.content_type == ContentType::Source
                    && child
                        .source_language
                        .as_ref()
                        .and_then(|sl| sl.as_query())
                        .is_some()
            })
        })
    }

    /// Get the first root layout block ID from index.org (a heading with a
    /// query source child).
    pub fn root_layout_block_id(&self) -> Option<EntityUri> {
        let index_doc_uri = self.block_state.doc_uri_by_name("index")?;
        self.block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == index_doc_uri)
            .find(|root_block| {
                self.block_state.blocks.values().any(|child| {
                    child.parent_id == root_block.id
                        && child.content_type == ContentType::Source
                        && child
                            .source_language
                            .as_ref()
                            .and_then(|sl| sl.as_query())
                            .is_some()
                })
            })
            .map(|b| b.id.clone())
    }

    /// The id of the layout's main-panel container block, when the active
    /// layout has one. Identified semantically: a render-source block whose
    /// parent is the main-panel container resolves the container id back out.
    /// Returns `None` in layout-less mode (no main-panel render source).
    ///
    /// The well-known default-layout main panel id is the seed id
    /// `block:default-main-panel`; this accessor returns it from the resolved
    /// block state so callers (e.g. `inv-viewmodel-root-matches-render-expr`)
    /// never embed that literal themselves.
    pub fn main_panel_block_id(&self) -> Option<EntityUri> {
        let main_panel_id = EntityUri::parse("block:default-main-panel").expect("static id");
        self.block_state
            .blocks
            .contains_key(&main_panel_id)
            .then(|| main_panel_id.clone())
    }

    /// Get the main panel's render expression (the render source child of the
    /// main panel headline).
    pub fn main_panel_render_expr(&self) -> Option<&RenderExpr> {
        let main_panel_id = self.main_panel_block_id()?;
        self.layout_blocks
            .render_source_ids
            .iter()
            .find(|id| {
                self.block_state
                    .blocks
                    .get(*id)
                    .is_some_and(|b| b.parent_id == main_panel_id)
            })
            .and_then(|id| self.render_expressions.get(id))
    }

    /// Get the active `RenderExpr` for the root layout's render source block.
    /// Returns `None` if no render source is tracked.
    pub fn root_render_expr(&self) -> Option<&RenderExpr> {
        let root_id = self.root_layout_block_id()?;
        // Find the render source block that is a child of the root layout
        self.layout_blocks
            .render_source_ids
            .iter()
            .find(|id| {
                self.block_state
                    .blocks
                    .get(*id)
                    .map(|b| b.parent_id == root_id)
                    .unwrap_or(false)
            })
            .and_then(|id| self.render_expressions.get(id))
    }

    /// Name of the active render expression for `region` (e.g. "tree",
    /// "outline", "list"). Used by `build_reference_navigator` to pick
    /// the right `CollectionNavigator` shape for arrow-key navigation.
    pub fn active_render_expr_name(&self, _: Region) -> Option<String> {
        // For now, use the main panel's render expression (region is ignored
        // because the PBT currently only has one navigable region).
        let expr = self.main_panel_render_expr().or(self.root_render_expr())?;
        match expr {
            RenderExpr::FunctionCall { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// Whether a click on `uri` in `region` is predicted to dispatch
    /// `navigation.focus(region=main, block_id=uri)` — the bound action the
    /// default LeftSidebar wraps each doc selectable in.
    ///
    /// The default sidebar PRQL selects page blocks with non-special
    /// titles (not "index" / "__default__"), and the layout wraps every
    /// row in `selectable(action: navigation.focus(region="main",
    /// block_id=col("id")))`. Used by `ClickBlock::apply_to_ref`
    /// (LeftSidebar branch) and `NavigateFocus` to gate the
    /// navigation-history + open_pins mutations on whether prod would
    /// actually dispatch the bound intent. Without this, the ref model
    /// would push nav-history entries for sidebar clicks on entities
    /// prod treats as plain editor-focus targets, breaking
    /// `inv-focus-roots-consistent-with-ref`.
    pub fn predicts_navigation_focus(&self, uri: &EntityUri, region: Region) -> bool {
        if region != Region::LeftSidebar {
            return false;
        }
        // A user index.org replaces the whole 3-column layout: there is no
        // LeftSidebar live_block at all, so no sidebar row ever binds
        // `navigation.focus` (same family as DragDropBlock's
        // CustomLayoutNotDraggable gate).
        if self.has_user_index_org() {
            return false;
        }
        let Some(block) = self.block_state.blocks.get(uri) else {
            return false;
        };
        if block.content_type != ContentType::Text || !block.is_page() {
            return false;
        }
        let t = block.title();
        !t.is_empty() && t != "index" && t != "__default__"
    }

    /// Block IDs in the predicted LeftSidebar render set — the same set
    /// the default sidebar PRQL produces. Each entry is wrapped by the
    /// default layout in a selectable bound to `navigation.focus`, so
    /// this is also the candidate set for `ClickBlock(LeftSidebar)` and
    /// `NavigateFocus` generators.
    pub fn predicted_sidebar_navigation_targets(&self) -> Vec<EntityUri> {
        self.block_state
            .blocks
            .values()
            .filter(|b| {
                if b.content_type != ContentType::Text || !b.is_page() {
                    return false;
                }
                let t = b.title();
                !t.is_empty() && t != "index" && t != "__default__"
            })
            .map(|b| b.id.clone())
            .collect()
    }

    /// Returns expected query results for a watch using the TestQuery
    /// evaluator.
    pub fn query_results(&self, watch_spec: &WatchSpec) -> Vec<HashMap<String, Value>> {
        watch_spec.query.evaluate(&self.block_state.blocks)
    }
}

impl Default for ReferenceDomainState {
    fn default() -> Self {
        Self::new()
    }
}

fn default_block_operations() -> Vec<holon_api::render_types::OperationDescriptor> {
    use holon_api::render_types::OperationDescriptor;
    use holon_api::render_types::OperationParam;
    use holon_api::render_types::TypeHint;

    let entity_name = "block".to_string();
    let entity_short_name = "block".to_string();
    let id_param = OperationParam {
        name: "id".to_string(),
        type_hint: TypeHint::String,
        description: "Entity ID".to_string(),
    };

    vec![
        OperationDescriptor {
            entity_name: entity_name.clone().into(),
            entity_short_name: entity_short_name.clone(),
            name: "set_field".to_string(),
            display_name: "Set Field".to_string(),
            description: "Set a field on block".to_string(),
            required_params: vec![
                id_param.clone(),
                OperationParam {
                    name: "field".to_string(),
                    type_hint: TypeHint::String,
                    description: "Field name".to_string(),
                },
                OperationParam {
                    name: "value".to_string(),
                    type_hint: TypeHint::String,
                    description: "Field value".to_string(),
                },
            ],
            id_column: "id".to_string(),
            affected_fields: vec![],
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Block,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Test,
            },
            trigger: None,
            bound_params: Default::default(),
            guard: holon_api::pattern::OpGuard::None,
        },
        OperationDescriptor {
            entity_name: entity_name.clone().into(),
            entity_short_name: entity_short_name.clone(),
            name: "cycle_task_state".to_string(),
            display_name: "Cycle Task State".to_string(),
            description: "Cycle to the next task state".to_string(),
            required_params: vec![id_param],
            affected_fields: vec!["task_state".to_string()],
            id_column: "id".to_string(),
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Block,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::Listed {
                surfaces: holon_api::SurfaceSet {
                    slash_menu: true,
                    action_bar: false,
                },
            },
            trigger: None,
            bound_params: Default::default(),
            guard: holon_api::pattern::OpGuard::None,
        },
    ]
}
