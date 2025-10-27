//! Tier-1 domain fragment of the PBT reference model (ADR 0004 / 0005).
//!
//! `ReferenceDomainState` is the **PBT reference oracle for the domain**, not the
//! production domain itself. The production domain stays a *logical canonical
//! projection* — the consensus across wired adapters after quiescence; no struct
//! "holds" it (ADR-0004 canonicity reframing). This struct isolates the
//! sort_key-free, adapter-independent domain data so it can be the single
//! fragment shared across all wirings.

use std::collections::{BTreeMap, HashMap, HashSet};

use holon_api::EntityName;
use holon_api::entity_uri::EntityUri;
use holon_api::render_types::RenderExpr;

use super::reference_state::{BlockState, LayoutBlockInfo};

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

    /// Profile block IDs (blocks with source_language = holon_entity_profile_yaml)
    pub profile_block_ids: HashSet<EntityUri>,

    /// Current active profile YAML index per entity_name.
    pub active_profiles: HashMap<EntityName, (EntityUri, usize)>,

    /// Active render expressions per render source block (block_id → RenderExpr).
    /// Updated when render source blocks are created or mutated.
    /// `BTreeMap` for deterministic iteration (see `BlockState::blocks`).
    pub render_expressions: BTreeMap<EntityUri, RenderExpr>,

    /// Parsed entity profile from the seed YAML (or custom org file).
    /// Used by `BuilderServices::resolve_profile` for ViewModel construction.
    pub seed_profile: Option<holon::entity_profile::EntityProfile>,

    /// Block entity operations (set_field, create, update, delete, cycle_task_state).
    /// Used by `BuilderServices::resolve_profile` to inject operations into RowProfile.
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
}

impl Default for ReferenceDomainState {
    fn default() -> Self {
        Self::new()
    }
}

fn default_block_operations() -> Vec<holon_api::render_types::OperationDescriptor> {
    use holon_api::render_types::{OperationDescriptor, OperationParam, TypeHint};

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
            ..Default::default()
        },
        OperationDescriptor {
            entity_name: entity_name.clone().into(),
            entity_short_name: entity_short_name.clone(),
            name: "cycle_task_state".to_string(),
            display_name: "Cycle Task State".to_string(),
            description: "Cycle to the next task state".to_string(),
            required_params: vec![id_param],
            affected_fields: vec!["task_state".to_string()],
            ..Default::default()
        },
    ]
}
