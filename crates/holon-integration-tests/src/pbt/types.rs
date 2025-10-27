//! Core PBT types: mutations, test variants, and marker traits.

use std::collections::HashMap;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::{ContentType, SourceLanguage, Value};

use holon_orgmode::models::OrgBlockExt;

/// Source of a mutation
#[derive(Debug, Clone, PartialEq)]
pub enum MutationSource {
    /// User action via BackendEngine operations (through ctx.execute_op)
    UI,
    /// External change to an Org file (simulates file edit)
    External,
}

/// A mutation to the data model
#[derive(Debug, Clone)]
pub enum Mutation {
    Create {
        entity: String,
        id: EntityUri,
        parent_id: EntityUri,
        fields: HashMap<String, Value>,
    },
    Update {
        entity: String,
        id: EntityUri,
        fields: HashMap<String, Value>,
    },
    Delete {
        entity: String,
        id: EntityUri,
    },
    Move {
        entity: String,
        id: EntityUri,
        new_parent_id: EntityUri,
    },
    /// Simulate app restart: clears OrgSyncController's last_projection.
    /// This tests that re-parsing org files doesn't create orphan blocks in Loro.
    RestartApp,
}

/// Apply org-mode properties (task_state, priority, tags, scheduled, deadline)
/// and custom properties from `fields` onto `block`.
///
/// When `is_create` is true, task_state is always set from fields (no clear-to-None path).
/// The caller handles update-specific task_state clearing separately.
fn apply_org_properties(block: &mut Block, fields: &HashMap<String, Value>, is_create: bool) {
    if is_create {
        if let Some(task_state) = fields
            .get("task_state")
            .or_else(|| fields.get("TODO"))
            .and_then(|v| v.as_string())
        {
            block.set_task_state(Some(holon_api::TaskState::from_keyword(&task_state)));
        }
    }
    if let Some(priority) = fields
        .get("priority")
        .or_else(|| fields.get("PRIORITY"))
        .and_then(|v| v.as_i64())
    {
        block.set_priority(Some(
            holon_api::Priority::from_int(priority as i32)
                .unwrap_or_else(|e| panic!("stored priority {priority} is invalid: {e}")),
        ));
    }
    if let Some(tags) = fields
        .get("tags")
        .or_else(|| fields.get("TAGS"))
        .and_then(|v| v.as_string())
    {
        block.set_tags(holon_api::Tags::from_csv(tags));
    }
    if let Some(scheduled) = fields
        .get("scheduled")
        .or_else(|| fields.get("SCHEDULED"))
        .and_then(|v| v.as_string())
    {
        if let Ok(ts) = holon_api::types::Timestamp::parse(&scheduled) {
            block.set_scheduled(Some(ts));
        }
    }
    if let Some(deadline) = fields
        .get("deadline")
        .or_else(|| fields.get("DEADLINE"))
        .and_then(|v| v.as_string())
    {
        if let Ok(ts) = holon_api::types::Timestamp::parse(&deadline) {
            block.set_deadline(Some(ts));
        }
    }

    let extra_keys: &[&str] = if is_create {
        &[
            "content",
            "content_type",
            "source_language",
            "id",
            "parent_id",
            "task_state",
            "TODO",
            "priority",
            "PRIORITY",
            "tags",
            "TAGS",
            "scheduled",
            "SCHEDULED",
            "deadline",
            "DEADLINE",
        ]
    } else {
        &[
            "content",
            "task_state",
            "TODO",
            "priority",
            "PRIORITY",
            "tags",
            "TAGS",
            "scheduled",
            "SCHEDULED",
            "deadline",
            "DEADLINE",
        ]
    };
    for (k, v) in fields.iter() {
        if !extra_keys.contains(&k.as_str()) {
            block.properties.insert(k.clone(), v.clone());
        }
    }
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

    /// Convert mutation to BackendEngine operation parameters
    pub fn to_operation(&self) -> (String, String, HashMap<String, Value>) {
        match self {
            Mutation::Create {
                entity,
                id,
                parent_id,
                fields,
            } => {
                let mut params = fields.clone();
                params.insert("id".to_string(), id.clone().into());
                params.insert("parent_id".to_string(), parent_id.clone().into());
                (entity.clone(), "create".to_string(), params)
            }
            Mutation::Update { entity, id, fields } => {
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
                    (entity.clone(), "update".to_string(), params)
                } else if let Some((field_name, field_value)) = fields
                    .iter()
                    .find(|(k, _)| *k != "id" && *k != "parent_id")
                    .map(|(k, v)| (k.clone(), v.clone()))
                {
                    params.insert("field".to_string(), Value::String(field_name));
                    params.insert("value".to_string(), field_value);
                    (entity.clone(), "set_field".to_string(), params)
                } else {
                    params.insert("field".to_string(), Value::String("content".to_string()));
                    params.insert("value".to_string(), Value::String(String::new()));
                    (entity.clone(), "set_field".to_string(), params)
                }
            }
            Mutation::Delete { entity, id } => {
                let mut params = HashMap::new();
                params.insert("id".to_string(), id.clone().into());
                (entity.clone(), "delete".to_string(), params)
            }
            Mutation::Move {
                entity,
                id,
                new_parent_id,
            } => {
                let mut params = HashMap::new();
                params.insert("id".to_string(), id.clone().into());
                params.insert("parent_id".to_string(), new_parent_id.clone().into());
                (entity.clone(), "set_field".to_string(), params)
            }
            Mutation::RestartApp => (
                "_restart".to_string(),
                "restart".to_string(),
                HashMap::new(),
            ),
        }
    }

    /// Apply mutation to a vector of blocks (for reference model)
    pub fn apply_to(&self, blocks: &mut Vec<Block>) {
        match self {
            Mutation::Create {
                id,
                parent_id,
                fields,
                ..
            } => {
                let content = fields
                    .get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
                    .to_string();

                let content_type: ContentType = fields
                    .get("content_type")
                    .and_then(|v| v.as_string())
                    .unwrap_or("text")
                    .parse()
                    .unwrap();

                let source_language: Option<SourceLanguage> = fields
                    .get("source_language")
                    .and_then(|v| v.as_string())
                    .map(|s| s.parse::<SourceLanguage>().unwrap());

                let document_id = if parent_id.is_document() {
                    parent_id.clone()
                } else {
                    blocks
                        .iter()
                        .find(|b| b.id == *parent_id)
                        .map(|b| b.document_id.clone())
                        .unwrap_or_else(|| {
                            panic!(
                                "Mutation::Create: parent block '{}' not found in blocks vec",
                                parent_id
                            )
                        })
                };

                let mut block = if content_type == ContentType::Source {
                    let mut b =
                        Block::new_text(id.clone(), parent_id.clone(), document_id, content);
                    b.content_type = ContentType::Source;
                    b.source_language = source_language;
                    b
                } else {
                    Block::new_text(id.clone(), parent_id.clone(), document_id, content)
                };

                apply_org_properties(&mut block, fields, true);

                blocks.push(block);
            }
            Mutation::Update { id, fields, .. } => {
                if let Some(block) = blocks.iter_mut().find(|b| b.id == *id) {
                    if let Some(content) = fields.get("content").and_then(|v| v.as_string()) {
                        block.content = content.to_string();
                    }

                    if fields.contains_key("task_state") || fields.contains_key("TODO") {
                        match fields
                            .get("task_state")
                            .or_else(|| fields.get("TODO"))
                            .and_then(|v| v.as_string())
                        {
                            Some(kw) => {
                                block.set_task_state(Some(holon_api::TaskState::from_keyword(&kw)))
                            }
                            None => block.set_task_state(None),
                        }
                    }
                    apply_org_properties(block, fields, false);
                }
            }
            Mutation::Delete { id, .. } => {
                let mut to_delete: Vec<EntityUri> = vec![id.clone()];
                let mut i = 0;
                while i < to_delete.len() {
                    let parent_id = &to_delete[i];
                    let children: Vec<EntityUri> = blocks
                        .iter()
                        .filter(|b| b.parent_id == *parent_id)
                        .map(|b| b.id.clone())
                        .collect();
                    to_delete.extend(children);
                    i += 1;
                }
                blocks.retain(|b| !to_delete.contains(&b.id));
            }
            Mutation::Move {
                id, new_parent_id, ..
            } => {
                if let Some(block) = blocks.iter_mut().find(|b| b.id == *id) {
                    block.parent_id = new_parent_id.clone();
                }
            }
            Mutation::RestartApp => {}
        }
    }
}

/// A mutation event with source information
#[derive(Debug, Clone)]
pub struct MutationEvent {
    pub source: MutationSource,
    pub mutation: Mutation,
}

/// Configuration flags controlling which components are enabled in a test run.
#[derive(Debug, Clone)]
pub struct TestVariant {
    /// Enable Loro CRDT layer (false = SQL-only, matching Flutter default)
    pub enable_loro: bool,
}

impl TestVariant {
    pub fn full() -> Self {
        Self { enable_loro: true }
    }

    /// SQL-only, no Loro. Matches Flutter when LORO_ENABLED is unset.
    pub fn sql_only() -> Self {
        Self { enable_loro: false }
    }
}

impl Default for TestVariant {
    fn default() -> Self {
        Self::full()
    }
}

#[allow(async_fn_in_trait)] // only used with static dispatch (generics)
/// Marker trait for test variant selection via generics.
///
/// Variants control both the test configuration (Loro vs SQL-only) and the
/// executor used to receive watch_ui events. The `wait_for_structure` method
/// lets each variant define HOW events are received — on tokio (default) or
/// on a foreign executor (to catch cross-executor waker bugs like GPUI's
/// smol-based event loop not being woken by tokio mpsc sends).
pub trait VariantMarker: std::fmt::Debug + Clone + 'static {
    fn variant() -> TestVariant;

    /// Wait for the first Structure event from a WatchHandle.
    ///
    /// Takes ownership of the handle (to allow cross-thread transfer) and
    /// returns it alongside the WidgetSpec. The caller wraps this in a timeout.
    ///
    /// Default implementation: polls `watch.recv()` on the current (tokio) executor.
    async fn wait_for_structure(
        watch: holon_api::WatchHandle,
    ) -> (holon_api::WidgetSpec, holon_api::WatchHandle) {
        let mut watch = watch;
        loop {
            match watch.recv().await {
                Some(holon_api::UiEvent::Structure { widget_spec, .. }) => {
                    return (widget_spec, watch);
                }
                Some(_) => continue,
                None => panic!("watch_ui stream closed before Structure event"),
            }
        }
    }
}

/// All components enabled (Loro + SQL). Default for existing tests.
#[derive(Debug, Clone)]
pub struct Full;
impl VariantMarker for Full {
    fn variant() -> TestVariant {
        TestVariant::full()
    }
}

/// SQL-only, no Loro. Matches Flutter production default.
#[derive(Debug, Clone)]
pub struct SqlOnly;
impl VariantMarker for SqlOnly {
    fn variant() -> TestVariant {
        TestVariant::sql_only()
    }
}

/// Like Full, but receives watch_ui events on a non-tokio executor
/// (futures::executor on a dedicated thread). Catches cross-executor waker
/// bugs — e.g. GPUI's smol executor not being woken by tokio mpsc sends.
#[derive(Debug, Clone)]
pub struct CrossExecutor;
impl VariantMarker for CrossExecutor {
    fn variant() -> TestVariant {
        TestVariant::full()
    }

    async fn wait_for_structure(
        watch: holon_api::WatchHandle,
    ) -> (holon_api::WidgetSpec, holon_api::WatchHandle) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let result = futures::executor::block_on(async move {
                let mut watch = watch;
                loop {
                    match watch.recv().await {
                        Some(holon_api::UiEvent::Structure { widget_spec, .. }) => {
                            return (widget_spec, watch);
                        }
                        Some(_) => continue,
                        None => panic!("watch_ui stream closed before Structure event"),
                    }
                }
            });
            tx.send(result).ok();
        });
        rx.await.expect("cross-executor recv thread panicked")
    }
}
