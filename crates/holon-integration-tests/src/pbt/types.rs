//! Core PBT types: mutations, test variants, and marker traits.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::{ContentType, SourceLanguage, Value};

use holon_orgmode::models::OrgBlockExt;

/// Shared, `Send`-safe handle to the reference-stable-id → resolved-UUID map.
///
/// Lives in this neutral module (not `sut`) so component SUTs like
/// [`crate::pbt::sut_loro::LoroSut`] can hold a clone without depending on the
/// `E2ESut` facade. `std::sync::Mutex` (not `RefCell`) because the SUT is moved
/// across threads at teardown.
pub type DocUriMap = Arc<Mutex<HashMap<EntityUri, EntityUri>>>;

/// Source of a mutation
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MutationSource {
    /// User action via BackendEngine operations (through ctx.execute_op)
    UI,
    /// External change to an Org file (simulates file edit)
    External,
    /// Block created by an action watcher (trigger query → action execution)
    Action,
    /// Mutation applied through a Loro CRDT *peer* (not the primary instance).
    /// Diverges from the primary until a separate `SyncWithPeer`/`MergeFromPeer`
    /// transition converges it. `peer_idx` selects which peer (from prior
    /// `AddPeer`s) the mutation targets.
    LoroPeer { peer_idx: usize },
}

/// A mutation to the data model
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// Simulate app restart: clears FileSyncController's last_projection.
    /// This tests that re-parsing org files doesn't create orphan blocks in Loro.
    RestartApp,
}

// TODO: Move to some sut_org.rs or similar
/// Apply org-mode properties (task_state, priority, tags, scheduled, deadline)
/// and custom properties from `fields` onto `block`.
///
/// When `is_create` is true, task_state is always set from fields (no clear-to-None path).
/// The caller handles update-specific task_state clearing separately.
fn apply_org_properties(block: &mut Block, fields: &HashMap<String, Value>, is_create: bool) {
    if is_create
        && let Some(task_state) = fields
            .get("task_state")
            .or_else(|| fields.get("TODO"))
            .and_then(|v| v.as_string())
    {
        block.set_task_state(Some(holon_api::TaskState::from_keyword(task_state)));
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
        && let Ok(ts) = holon_api::types::Timestamp::parse(scheduled)
    {
        block.set_scheduled(Some(ts));
    }
    if let Some(deadline) = fields
        .get("deadline")
        .or_else(|| fields.get("DEADLINE"))
        .and_then(|v| v.as_string())
        && let Ok(ts) = holon_api::types::Timestamp::parse(deadline)
    {
        block.set_deadline(Some(ts));
    }
    // TODO: These are probably duplicated somewhere in non-test code.
    // Let's reuse that
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

// TODO: Move to sut_org.rb
/// Normalize content to match what an org round-trip will produce.
///
/// For Text blocks the first line becomes the org headline, which the parser
/// `.trim()`s (both ends) on re-parse, so leading *and* trailing whitespace
/// on the first line is stripped. Trailing whitespace on the entire string
/// is also stripped. Source blocks preserve content verbatim and are returned
/// unchanged (aside from overall trailing-whitespace trim, which the
/// renderer's `push_str(content); push('\n')` path doesn't reintroduce
/// differently).
pub fn normalize_content_for_org_roundtrip(content: &str, content_type: ContentType) -> String {
    if content_type == ContentType::Source {
        return content.trim_end().to_string();
    }
    // One trim+mark-extraction pass is NOT idempotent (e.g. `[[ x]]` →
    // label ` x` → next round-trip trims to `x`), and the SUT keeps
    // round-tripping the file until it converges — so the reference must
    // normalize to the FIXED POINT, not the first iterate. Terminates: every
    // pass either shrinks the string or leaves it unchanged. Surfaced by
    // extended-gen axis 1 (`[[ Hbplihw7UF]]` after promotion).
    let mut current = content.to_string();
    loop {
        let trimmed_end = current.trim_end();
        let trimmed = match trimmed_end.split_once('\n') {
            Some((first, rest)) => format!("{}\n{}", first.trim(), rest),
            None => trimmed_end.trim_start().to_string(),
        };
        // The parser also extracts inline org markup (`[[…]]` links, `*bold*`,
        // …) into `block.marks` and stores only the RENDERED LABEL as content
        // (parse-don't-validate at the boundary). Content that happens to spell
        // org markup therefore normalizes to its label after a file round-trip
        // — identity for plain text, so the default ASCII generators are
        // unaffected. Surfaced by extended-gen axis 1 (`[[x]]` → `x`).
        let (rendered, _marks) = holon_orgmode::inline_marks::extract_inline_marks(&trimmed);
        if rendered == current {
            return rendered;
        }
        current = rendered;
    }
}

/// Apply the org HEADLINE-TAG lens to a block: the first content line is the
/// headline title on disk, so a trailing `:tag1:tag2:` group there is org TAG
/// syntax (org has no escape for it) and re-parses into `block.tags`, not
/// content. Mirrors the parser exactly via
/// [`holon_orgmode::parser::split_headline_tags`]; pinned in
/// holon-org-format/tests/properties_prefix_headline_repro.rs.
///
/// Use wherever the reference models a block crossing the org-FILE boundary
/// (external file writes that the SUT parses into its stores, and the ref's
/// on-disk org view). In-memory stores (SQL/Loro) keep the raw content — the
/// reinterpretation happens only at the file parse. Surfaced by extended-gen
/// axis 1 (`:PROPERTIES:` → empty title + tag `PROPERTIES`).
pub fn apply_org_headline_tag_split(block: &mut Block) {
    if block.content_type == ContentType::Source {
        return;
    }
    let (first, rest) = match block.content.split_once('\n') {
        Some((f, r)) => (f, Some(r.to_string())),
        None => (block.content.as_str(), None),
    };
    let (title, tags) = holon_orgmode::parser::split_headline_tags(first);
    if tags.is_empty() {
        return;
    }
    block.content = match rest {
        Some(r) => format!("{title}\n{r}"),
        None => title,
    };
    for t in tags {
        block.tags.insert(t);
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
                let content_type: ContentType = fields
                    .get("content_type")
                    .and_then(|v| v.as_string())
                    .unwrap_or("text")
                    .parse()
                    .unwrap();

                // Normalize content to match the org round-trip.
                // For text blocks, the first line becomes the org headline —
                // the parser calls `.trim()` on it, so trailing whitespace on
                // the first line is stripped on re-parse. Trailing whitespace
                // on the whole string is also stripped. Source blocks preserve
                // content verbatim (no headline involved).
                let raw = fields
                    .get("content")
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();
                let content = normalize_content_for_org_roundtrip(raw, content_type);

                let source_language: Option<SourceLanguage> = fields
                    .get("source_language")
                    .and_then(|v| v.as_string())
                    .map(|s| s.parse::<SourceLanguage>().unwrap());

                let mut block = if content_type == ContentType::Source {
                    let mut b = Block::new_text(id.clone(), parent_id.clone(), content);
                    b.content_type = ContentType::Source;
                    b.source_language = source_language;
                    b
                } else {
                    Block::new_text(id.clone(), parent_id.clone(), content)
                };

                apply_org_properties(&mut block, fields, true);

                // Mirror production: a Create without an explicit sort_key
                // lands on `block.sort_key='a0'` (SQL column default), which
                // sorts *after* every gen_n_keys-assigned sibling (lowercase
                // 'a' > uppercase hex digits used by FractionalIndex). The
                // canonicalizer in `assign_reference_sequences_canonical`
                // sorts siblings by `sequence` then id; if the new block
                // inherits the default `sequence=0`, the id tie-break decides
                // the slot, which is arbitrary. Push it past every existing
                // sibling instead so canonicalization places it last.
                let max_sibling_seq = blocks
                    .iter()
                    .filter(|b| b.parent_id == *parent_id)
                    .map(|b| b.sequence())
                    .max()
                    .unwrap_or(-1);
                block.set_sequence(max_sibling_seq + 1);

                blocks.push(block);
            }
            Mutation::Update { id, fields, .. } => {
                if let Some(block) = blocks.iter_mut().find(|b| b.id == *id) {
                    if let Some(content) = fields.get("content").and_then(|v| v.as_string()) {
                        block.content =
                            normalize_content_for_org_roundtrip(content, block.content_type);
                    }

                    if fields.contains_key("task_state") || fields.contains_key("TODO") {
                        match fields
                            .get("task_state")
                            .or_else(|| fields.get("TODO"))
                            .and_then(|v| v.as_string())
                        {
                            Some(kw) => {
                                block.set_task_state(Some(holon_api::TaskState::from_keyword(kw)))
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MutationEvent {
    pub source: MutationSource,
    pub mutation: Mutation,
}
