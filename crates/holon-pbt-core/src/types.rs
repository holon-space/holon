//! Shared PBT type aliases hoisted to `holon-pbt-core` (Phase 1a Step 0) so
//! companion `*-testing` crates and SUT adapters can name them without
//! depending on `holon-integration-tests`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use holon_api::entity_uri::EntityUri;
use holon_api::Value;

/// Maps a document's stable block-id key to its live doc URI, shared across a
/// SUT adapter and the composed builder. (Moved verbatim from
/// `holon-integration-tests::pbt::types`.)
pub type DocUriMap = Arc<Mutex<HashMap<EntityUri, EntityUri>>>;

// ── Block-mutation intent (relocated from holon-integration-tests, Phase 1a
// Step 2 / B1) ────────────────────────────────────────────────────────────
//
// The org-free CORE of the PBT harness's mutation model lives here so the
// mutation-carrying SUT caps (`SutSeamMutate`, `SutApplyMutation`) can live in
// `holon-pbt-core` (below `holon-orgmode`). The org-dependent reference-model
// applier (`Mutation::apply_to` + its `OrgBlockExt` helpers) stays in
// `holon-integration-tests` as a `MutationApply` extension trait.

/// Source of a mutation — WHO/HOW it entered the system. Test-scenario
/// vocabulary (`LoroPeer { peer_idx }` is meaningless in prod), which is why
/// this is harness-only and not in `holon-api`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MutationSource {
    /// User action via BackendEngine operations (through ctx.execute_op)
    UI,
    /// External change to an Org file (simulates file edit)
    External,
    /// Block created by an action watcher (trigger query → action execution)
    Action,
    /// Mutation applied through a Loro CRDT *peer* (not the primary instance).
    /// Diverges from the primary until a separate
    /// `SyncWithPeer`/`MergeFromPeer` transition converges it. `peer_idx`
    /// selects which peer (from prior `AddPeer`s) the mutation targets.
    LoroPeer { peer_idx: usize },
}

/// A block-model mutation intent.
///
/// **Harness twin of production `holon_api::change_set::ChangeOp` /
/// `ChangeSet`.** Same Create / Update≈SetField / Move≈Relocate / Delete shape;
/// `MutationEvent` ≈ a single-op `ChangeSet` + source. They are the same
/// concept expressed twice (once for prod, once for the harness). Physically
/// unifying them (harness speaks `ChangeOp`) is tracked follow-up **B2** —
/// `/tmp/holon-b2-mutation-changeop-dedup-handoff.md` — deliberately NOT done
/// here (it would drag `RestartApp` + the `LoroPeer` source taxonomy into prod
/// `holon-api`, which is backwards).
///
/// The redundant `entity: String` field (always `"block"`) was dropped in B1
/// (parse-don't-validate: a constant is not data). `to_operation` now emits the
/// `"block"` entity literally.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Mutation {
    Create {
        id: EntityUri,
        parent_id: EntityUri,
        fields: HashMap<String, Value>,
    },
    Update {
        id: EntityUri,
        fields: HashMap<String, Value>,
    },
    Delete {
        id: EntityUri,
    },
    Move {
        id: EntityUri,
        new_parent_id: EntityUri,
    },
    /// Simulate app restart: clears FileSyncController's last_projection.
    /// This tests that re-parsing org files doesn't create orphan blocks in
    /// Loro.
    RestartApp,
}

impl Mutation {
    /// Returns the block ID targeted by this mutation, if any.
    pub fn target_block_id(&self) -> Option<EntityUri> {
        match self {
            Mutation::Create { id, .. }
            | Mutation::Update { id, .. }
            | Mutation::Delete { id, .. }
            | Mutation::Move { id, .. } => Some(id.clone()),
            Mutation::RestartApp => None,
        }
    }

    /// Convert mutation to BackendEngine operation parameters.
    pub fn to_operation(&self) -> (String, String, HashMap<String, Value>) {
        match self {
            Mutation::Create {
                id,
                parent_id,
                fields,
            } => {
                let mut params = fields.clone();
                params.insert("id".to_string(), id.clone().into());
                params.insert("parent_id".to_string(), parent_id.clone().into());
                ("block".to_string(), "create".to_string(), params)
            }
            Mutation::Update { id, fields } => {
                let mut params = HashMap::new();
                params.insert("id".to_string(), id.clone().into());

                // Check if update targets a known SQL column or a custom property.
                // Known columns use set_field (single-field update); custom properties
                // use the "update" operation which packs unknown keys into the
                // `properties` JSON column via partition_params.
                const KNOWN_COLUMNS: &[&str] = &[
                    "content",
                    "parent_id",
                    "content_type",
                    "source_language",
                    "source_name",
                    "collapsed",
                    "completed",
                    "block_type",
                ];

                let has_custom_props = fields.keys().any(|k| !KNOWN_COLUMNS.contains(&k.as_str()));

                if has_custom_props {
                    // Use "update" operation — partition_params will pack custom
                    // keys into the properties JSON column.
                    for (k, v) in fields.iter() {
                        params.insert(k.clone(), v.clone());
                    }
                    ("block".to_string(), "update".to_string(), params)
                } else if let Some((field_name, field_value)) = fields
                    .iter()
                    .find(|(k, _)| *k != "id" && *k != "parent_id")
                    .map(|(k, v)| (k.clone(), v.clone()))
                {
                    params.insert("field".to_string(), Value::String(field_name));
                    params.insert("value".to_string(), field_value);
                    ("block".to_string(), "set_field".to_string(), params)
                } else {
                    params.insert("field".to_string(), Value::String("content".to_string()));
                    params.insert("value".to_string(), Value::String(String::new()));
                    ("block".to_string(), "set_field".to_string(), params)
                }
            }
            Mutation::Delete { id } => {
                let mut params = HashMap::new();
                params.insert("id".to_string(), id.clone().into());
                ("block".to_string(), "delete".to_string(), params)
            }
            Mutation::Move { id, new_parent_id } => {
                let mut params = HashMap::new();
                params.insert("id".to_string(), id.clone().into());
                params.insert("parent_id".to_string(), new_parent_id.clone().into());
                ("block".to_string(), "set_field".to_string(), params)
            }
            Mutation::RestartApp => (
                "_restart".to_string(),
                "restart".to_string(),
                HashMap::new(),
            ),
        }
    }
}

/// A mutation event with source information.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MutationEvent {
    pub source: MutationSource,
    pub mutation: Mutation,
}

// ── Task-state cycle vocabulary + fixture-corruption kind (relocated from
// holon-integration-tests, Phase 1a Step 2) so the caps that name them
// (`SutMutate`, `SutFixtureFs`) can live in holon-pbt-core. ─────────────────

/// The production task-state cycle, in click order.
pub const TASK_STATE_CYCLE: &[&str] = &["", "TODO", "DOING", "DONE"];

/// Parse-don't-validate target state for `ToggleState`: clicking the
/// state_toggle widget can only ever land on a production-cycle member, so the
/// type makes off-cycle targets unrepresentable. Serialized as the keyword
/// string (`""`/`"TODO"`/…) so old capture JSONs (plain strings) still load.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum CycleTarget {
    Clear,
    Todo,
    Doing,
    Done,
}

impl CycleTarget {
    pub const ALL: [CycleTarget; 4] = [Self::Clear, Self::Todo, Self::Doing, Self::Done];

    pub fn keyword(self) -> &'static str {
        match self {
            Self::Clear => "",
            Self::Todo => "TODO",
            Self::Doing => "DOING",
            Self::Done => "DONE",
        }
    }

    pub fn idx(self) -> usize {
        match self {
            Self::Clear => 0,
            Self::Todo => 1,
            Self::Doing => 2,
            Self::Done => 3,
        }
    }
}

impl From<CycleTarget> for String {
    fn from(t: CycleTarget) -> String {
        t.keyword().to_string()
    }
}

impl TryFrom<String> for CycleTarget {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|t| t.keyword() == s)
            .ok_or_else(|| {
                format!("not a production-cycle keyword: {s:?} (cycle {TASK_STATE_CYCLE:?})")
            })
    }
}

/// How a stale/corrupt Loro snapshot fixture is malformed (drives
/// `CreateStaleLoro` via `SutFixtureFs::create_stale_loro`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LoroCorruptionType {
    /// Empty file (0 bytes)
    Empty,
    /// File with partial/truncated Loro header
    Truncated,
    /// File with invalid magic bytes
    InvalidHeader,
}
