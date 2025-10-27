//! Reference model for the PBT state machine.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::{ContentType, Region, Value};

use holon::testing::e2e_test_helpers::ChangeType;

use super::query::WatchSpec;
use super::types::TestVariant;

/// Valid render DSL expressions for mutating render source blocks.
pub const VALID_RENDER_EXPRESSIONS: &[&str] = &[
    "table()",
    r#"list(#{item_template: block_ref()})"#,
    r#"columns(#{gap: 4, item_template: block_ref()})"#,
    r#"list(#{item_template: row(text(col("content")))})"#,
];

/// A test entity profile that generates its own YAML and knows how to check
/// whether a block matches its variant condition.
pub struct TestEntityProfile {
    pub profile_name: &'static str,
    pub field_name: &'static str,
}

impl TestEntityProfile {
    fn to_yaml(&self) -> String {
        format!(
            "entity_name: block\ncomputed:\n  has_{field}: \"= {field} != ()\"\ndefault:\n  render: 'row(col(\"content\"))'\nvariants:\n  - name: {name}\n    condition: \"= has_{field}\"\n    render: 'row(col(\"content\"))'\n    operations: []",
            field = self.field_name,
            name = self.profile_name,
        )
    }
}

/// Index 0 in VALID_PROFILE_YAMLS is the "no variants" YAML (always "default").
/// Indices 1..N correspond to TEST_PROFILES[0..N-1].
pub const TEST_PROFILES: &[TestEntityProfile] = &[
    TestEntityProfile {
        profile_name: "task",
        field_name: "task_state",
    },
    TestEntityProfile {
        profile_name: "has_content",
        field_name: "content",
    },
];

const NO_VARIANTS_YAML: &str =
    "entity_name: block\ncomputed: {}\ndefault:\n  render: 'row(col(\"content\"))'\nvariants: []";

pub static VALID_PROFILE_YAMLS: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
    let mut yamls = vec![NO_VARIANTS_YAML.to_string()];
    for tep in TEST_PROFILES {
        yamls.push(tep.to_yaml());
    }
    yamls
});

/// Typed classification of layout block IDs in index.org.
///
/// Layout blocks are split into three categories with different mutation rules:
/// - **headline_ids**: The text headline blocks that parent query/render sources.
///   These can have content, task_state, priority, tags mutated.
/// - **query_source_ids**: PRQL/GQL/SQL source blocks. These are truly immutable
///   because changing them would break `initial_widget()`.
/// - **render_source_ids**: Render DSL source blocks. These can have their content
///   changed to any valid render expression.
#[derive(Debug, Clone, Default)]
pub struct LayoutBlockInfo {
    pub headline_ids: HashSet<String>,
    pub query_source_ids: HashSet<String>,
    pub render_source_ids: HashSet<String>,
}

impl LayoutBlockInfo {
    /// Returns true if the block is part of the layout at all.
    pub fn contains(&self, id: &str) -> bool {
        self.headline_ids.contains(id)
            || self.query_source_ids.contains(id)
            || self.render_source_ids.contains(id)
    }

    /// Returns true if the block must never be mutated (query sources only).
    pub fn is_immutable(&self, id: &str) -> bool {
        self.query_source_ids.contains(id)
    }

    /// Remove a block from all sets.
    pub fn remove(&mut self, id: &str) {
        self.headline_ids.remove(id);
        self.query_source_ids.remove(id);
        self.render_source_ids.remove(id);
    }
}

/// Reference state tracking all expected data (uses production Block struct)
#[derive(Debug, Clone)]
pub struct ReferenceState {
    /// Whether the application has been started
    pub app_started: bool,

    /// Canonical block state (using production Block struct)
    pub blocks: HashMap<String, Block>,

    /// Created documents (doc_uri -> file_name)
    pub documents: HashMap<String, String>,

    /// Expected CDC events not yet observed
    pub pending_cdc_events: VecDeque<ExpectedCDCEvent>,

    /// Active query watches (query_id -> watch spec with TestQuery)
    pub active_watches: HashMap<String, WatchSpec>,

    /// ID counter for generating unique block IDs
    pub next_id: usize,

    /// ID counter for generating unique document IDs
    pub next_doc_id: usize,

    /// Current view filter ("all", "main", "sidebar")
    pub current_view: String,

    /// Navigation history per region (for back/forward navigation)
    pub navigation_history: HashMap<Region, NavigationHistory>,

    /// Mapping of block_id → doc_uri (persists even after blocks are deleted)
    pub block_documents: HashMap<String, String>,

    /// Runtime for async operations
    pub runtime: Arc<tokio::runtime::Runtime>,

    /// Pre-startup directories created (relative paths)
    pub pre_startup_directories: Vec<String>,

    /// Whether git has been initialized
    pub git_initialized: bool,

    /// Whether jj has been initialized
    pub jj_initialized: bool,

    /// Number of pre-startup org files created (for weighting StartApp)
    pub pre_startup_file_count: usize,

    /// Typed layout block classification for index.org.
    pub layout_blocks: LayoutBlockInfo,

    /// Profile block IDs (blocks with source_language = holon_entity_profile_yaml)
    pub profile_block_ids: HashSet<String>,

    /// Current active profile YAML index per entity_name.
    pub active_profiles: HashMap<String, (String, usize)>,

    /// Test variant configuration (which components are enabled)
    pub variant: TestVariant,

    /// TODO keyword set for task_state mutations (generated once per test case)
    pub keyword_set: Option<super::generators::TodoKeywordSet>,
}

/// Expected CDC event
#[derive(Debug, Clone)]
pub struct ExpectedCDCEvent {
    pub query_id: String,
    pub change_type: ChangeType,
    pub entity_id: String,
}

/// Navigation history for a region (for back/forward navigation)
#[derive(Debug, Clone)]
pub struct NavigationHistory {
    /// History entries: None = home view, Some(id) = focused on block
    pub entries: Vec<Option<String>>,
    /// Current cursor position in history
    pub cursor: usize,
}

impl NavigationHistory {
    pub fn new() -> Self {
        Self {
            entries: vec![None],
            cursor: 0,
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.cursor < self.entries.len().saturating_sub(1)
    }

    pub fn current_focus(&self) -> Option<String> {
        self.entries.get(self.cursor).cloned().flatten()
    }
}

impl ReferenceState {
    pub fn empty() -> Self {
        Self::with_variant(TestVariant::default())
    }

    pub fn with_variant(variant: TestVariant) -> Self {
        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        Self {
            app_started: false,
            blocks: HashMap::new(),
            documents: HashMap::new(),
            pending_cdc_events: VecDeque::new(),
            active_watches: HashMap::new(),
            next_id: 0,
            next_doc_id: 0,
            current_view: "all".to_string(),
            navigation_history: HashMap::new(),
            block_documents: HashMap::new(),
            runtime,
            pre_startup_directories: Vec::new(),
            git_initialized: false,
            jj_initialized: false,
            pre_startup_file_count: 0,
            layout_blocks: LayoutBlockInfo::default(),
            profile_block_ids: HashSet::new(),
            active_profiles: HashMap::new(),
            variant,
            keyword_set: None,
        }
    }

    pub fn with_blocks(blocks: Vec<Block>) -> Self {
        let runtime = Arc::new(tokio::runtime::Runtime::new().unwrap());
        let blocks_map: HashMap<String, Block> = blocks
            .iter()
            .map(|b| (b.id.to_string(), b.clone()))
            .collect();
        let block_documents: HashMap<String, String> = blocks
            .iter()
            .filter_map(|b| {
                if b.parent_id.is_doc() {
                    Some((b.id.to_string(), b.parent_id.as_raw_str().to_string()))
                } else {
                    None
                }
            })
            .collect();
        Self {
            app_started: true,
            blocks: blocks_map,
            documents: HashMap::new(),
            pending_cdc_events: VecDeque::new(),
            active_watches: HashMap::new(),
            next_id: 0,
            next_doc_id: 0,
            current_view: "all".to_string(),
            navigation_history: HashMap::new(),
            block_documents,
            runtime,
            pre_startup_directories: Vec::new(),
            git_initialized: false,
            jj_initialized: false,
            pre_startup_file_count: 0,
            layout_blocks: LayoutBlockInfo::default(),
            profile_block_ids: HashSet::new(),
            active_profiles: HashMap::new(),
            variant: TestVariant::default(),
            keyword_set: None,
        }
    }

    pub fn current_focus(&self, region: Region) -> Option<String> {
        self.navigation_history
            .get(&region)
            .and_then(|h| h.current_focus())
    }

    pub fn can_go_back(&self, region: Region) -> bool {
        self.navigation_history
            .get(&region)
            .map(|h| h.can_go_back())
            .unwrap_or(false)
    }

    pub fn can_go_forward(&self, region: Region) -> bool {
        self.navigation_history
            .get(&region)
            .map(|h| h.can_go_forward())
            .unwrap_or(false)
    }

    pub fn from_structure(_structure: Vec<Block>) -> Self {
        Self::empty()
    }

    pub fn current_view(&self) -> String {
        self.current_view.clone()
    }

    /// Returns expected query results for a watch using the TestQuery evaluator.
    pub fn query_results(&self, watch_spec: &WatchSpec) -> Vec<HashMap<String, Value>> {
        watch_spec.query.evaluate(&self.blocks)
    }

    /// Check if index.org exists with the structure required by initial_widget().
    pub fn has_valid_index_org(&self) -> bool {
        let index_doc_uri = EntityUri::doc("index.org").to_string();
        if !self.documents.contains_key(&index_doc_uri) {
            return false;
        }

        let root_blocks: Vec<&Block> = self
            .blocks
            .values()
            .filter(|b| b.parent_id.as_raw_str() == index_doc_uri)
            .collect();

        root_blocks.iter().any(|root_block| {
            self.blocks.values().any(|child| {
                child.parent_id.as_raw_str() == root_block.id.as_str()
                    && child.content_type == ContentType::Source
                    && child
                        .source_language
                        .as_ref()
                        .and_then(|sl| sl.as_query())
                        .is_some()
            })
        })
    }

    /// Get the first root layout block ID from index.org (a heading with a query source child).
    pub fn root_layout_block_id(&self) -> Option<String> {
        let index_doc_uri = EntityUri::doc("index.org").to_string();
        self.blocks
            .values()
            .filter(|b| b.parent_id.as_raw_str() == index_doc_uri)
            .find(|root_block| {
                self.blocks.values().any(|child| {
                    child.parent_id.as_raw_str() == root_block.id.as_str()
                        && child.content_type == ContentType::Source
                        && child
                            .source_language
                            .as_ref()
                            .and_then(|sl| sl.as_query())
                            .is_some()
                })
            })
            .map(|b| b.id.as_str().to_string())
    }

    /// Get IDs of text blocks only (not source blocks).
    pub fn text_block_ids(&self) -> Vec<String> {
        self.blocks
            .iter()
            .filter(|(_, b)| b.content_type == ContentType::Text)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Returns the set of block IDs that should appear in `focus_roots` for a region.
    /// Mirrors the SQL in `navigation.sql:32-39` (focus_roots matview):
    /// children of the focus target + the focus target itself.
    pub fn expected_focus_root_ids(&self, region: Region) -> BTreeSet<String> {
        let focus_id = match self.current_focus(region) {
            None => return BTreeSet::new(),
            Some(id) => id,
        };
        let mut roots = BTreeSet::new();
        // Self (block with id == focus_id)
        if self.blocks.contains_key(&focus_id) {
            roots.insert(focus_id.clone());
        }
        // Children (blocks whose parent_id == focus_id)
        for block in self.blocks.values() {
            if block.parent_id.as_str() == focus_id {
                roots.insert(block.id.as_str().to_string());
            }
        }
        roots
    }

    pub fn has_blocks_profile(&self) -> bool {
        self.active_profiles.contains_key("block")
    }

    pub fn blocks_profile_yaml_index(&self) -> Option<usize> {
        self.active_profiles.get("block").map(|(_, idx)| *idx)
    }

    /// Predict the expected RowProfile.name for a block, given the active profile YAML.
    /// Uses Block ground truth from `self.blocks` instead of query row data.
    pub fn expected_profile_name(&self, block_id: &EntityUri) -> Option<String> {
        let yaml_idx = self.blocks_profile_yaml_index()?;
        if yaml_idx == 0 {
            return Some("default".into());
        }

        let block = self.blocks.get(block_id.as_str())?;
        let tep = &TEST_PROFILES[yaml_idx - 1];

        let has_field = match tep.field_name {
            // Direct Block fields (not in properties map)
            "content" => !block.content.is_empty(),
            // Properties stored in the properties JSON map
            _ => block
                .properties
                .get(tep.field_name)
                .map_or(false, |v| !matches!(v, Value::Null)),
        };
        Some(if has_field {
            tep.profile_name.to_string()
        } else {
            "default".into()
        })
    }

    /// Rebuild profile tracking from current blocks state.
    pub fn rebuild_profile_tracking(&mut self) {
        self.profile_block_ids.clear();
        self.active_profiles.clear();
        for (block_key, block) in &self.blocks {
            // Skip seeded default layout blocks — they exist in the DB but
            // the profile resolver picks them up independently from the
            // ProfileResolver's LiveData source, not from the test's org files.
            if self
                .block_documents
                .get(block_key)
                .map_or(false, |doc| doc == "doc:__default__")
            {
                continue;
            }
            if block
                .source_language
                .as_ref()
                .map(|sl| sl.to_string())
                .as_deref()
                == Some("holon_entity_profile_yaml")
            {
                self.profile_block_ids.insert(block_key.clone());
                if let Some(yaml_idx) = VALID_PROFILE_YAMLS
                    .iter()
                    .position(|y| block.content.trim() == y.trim())
                {
                    if let Some(entity_name) = block
                        .content
                        .lines()
                        .next()
                        .and_then(|l| l.strip_prefix("entity_name: "))
                    {
                        self.active_profiles.insert(
                            entity_name.trim().to_string(),
                            (block_key.clone(), yaml_idx),
                        );
                    }
                }
            }
        }
    }
}
