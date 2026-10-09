//! An overlay block store: reads fall through to the write authority, writes
//! stay in memory.
//!
//! Only the primitive surface is modeled here — `get_by_id`,
//! `children_ordered`, `set_field`, `create`, `delete` and the ordering
//! seam. Every structural op (indent, split, join, move, delete_subtree, ...)
//! runs the SAME default [`BlockOperations`] method production runs, through
//! the same macro dispatch, on top of those primitives. Fields no shape reads
//! (marks, the typed scalar columns, non-tag edge fields) are not modeled.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::EdgeField;
use holon_api::EntityUri;
use holon_api::POSITION_AFTER_BLOCK_ID_PARAM;
use holon_api::StorageEntity;
use holon_api::Tags;
use holon_api::Value;
use holon_api::block::Block;

use crate::BlockNotInWriteAuthority;
use crate::ProjectionRead;
use crate::UnknownOperationError;
use crate::block_ordering::BlockOrdering;
use crate::block_ordering::MintedPosition;
use crate::block_ordering::OrderKeyMinting;
use crate::traits::BlockDataSourceHelpers;
use crate::traits::BlockOperations;
use crate::traits::BlockQueryHelpers;
use crate::traits::CompletionStateInfo;
use crate::traits::CrudOperations;
use crate::traits::DataSource;
use crate::traits::MarkOperations;
use crate::traits::OperationResult;
use crate::traits::Result;
use crate::traits::TaskOperations;
use crate::traits::TextOperations;
use crate::traits::WriteAuthorityReads;

#[derive(Default)]
struct Overlay {
    /// Every block read or written so far; `None` once deleted.
    blocks: HashMap<EntityUri, Option<Block>>,
    /// Child lists read or changed so far, in sibling order.
    children: HashMap<EntityUri, Vec<EntityUri>>,
    /// Each block's parent as the authority held it before the plan.
    base_parent: HashMap<EntityUri, EntityUri>,
    written: BTreeSet<EntityUri>,
    relisted: BTreeSet<EntityUri>,
}

pub struct SimStore {
    base: Arc<dyn WriteAuthorityReads>,
    overlay: Mutex<Overlay>,
}

fn not_found(id: &EntityUri) -> Box<dyn std::error::Error + Send + Sync> {
    format!("block {id} not found").into()
}

pub(super) fn uri(raw: &str) -> Result<EntityUri> {
    EntityUri::parse(raw).map_err(|e| format!("`{raw}` is no entity id: {e}").into())
}

impl SimStore {
    pub fn new(base: Arc<dyn WriteAuthorityReads>) -> Self {
        Self {
            base,
            overlay: Mutex::new(Overlay::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Overlay> {
        self.overlay.lock().expect("shape-gate overlay poisoned")
    }

    /// The block as the authority holds it, ignoring the plan.
    pub async fn base_block(&self, id: &EntityUri) -> Result<Option<Block>> {
        self.base.block(id).await
    }

    /// The block after the ops applied so far.
    pub async fn block(&self, id: &EntityUri) -> Result<Option<Block>> {
        if let Some(entry) = self.lock().blocks.get(id) {
            return Ok(entry.clone());
        }
        let block = self.base_block(id).await?;
        let mut overlay = self.lock();
        if let Some(b) = &block {
            overlay
                .base_parent
                .entry(id.clone())
                .or_insert_with(|| b.parent_id.clone());
        }
        Ok(overlay.blocks.entry(id.clone()).or_insert(block).clone())
    }

    /// A block the authority does not hold, and the plan did not write, is
    /// refused by name, as every write on it is (D69.a).
    async fn must_block(&self, id: &EntityUri) -> Result<Block> {
        self.block_authoritative(id)
            .await?
            .ok_or_else(|| not_found(id))
    }

    async fn child_ids(&self, parent: &EntityUri) -> Result<Vec<EntityUri>> {
        if let Some(list) = self.lock().children.get(parent) {
            return Ok(list.clone());
        }
        let list =
            if self.lock().written.contains(parent) && self.base_block(parent).await?.is_none() {
                Vec::new()
            } else {
                self.base.children(parent).await?
            };
        Ok(self
            .lock()
            .children
            .entry(parent.clone())
            .or_insert(list)
            .clone())
    }

    pub async fn ordered_children(&self, parent: &EntityUri) -> Result<Vec<Block>> {
        let mut out = Vec::new();
        for id in self.child_ids(parent).await? {
            out.push(self.must_block(&id).await?);
        }
        Ok(out)
    }

    fn put(&self, block: Block) {
        let mut overlay = self.lock();
        overlay.written.insert(block.id.clone());
        overlay.blocks.insert(block.id.clone(), Some(block));
    }

    fn set_list(&self, parent: &EntityUri, list: Vec<EntityUri>) {
        let mut overlay = self.lock();
        overlay.relisted.insert(parent.clone());
        overlay.children.insert(parent.clone(), list);
    }

    /// Blocks the ops wrote (created, changed, moved or deleted).
    pub fn written(&self) -> Vec<EntityUri> {
        self.lock().written.iter().cloned().collect()
    }

    /// Parents whose child list the ops changed.
    pub fn relisted(&self) -> Vec<EntityUri> {
        self.lock().relisted.iter().cloned().collect()
    }

    /// The parent of `id` before the plan and after it.
    pub async fn parents_of(&self, id: &EntityUri) -> Result<Vec<EntityUri>> {
        let before = self.lock().base_parent.get(id).cloned();
        let after = self.block(id).await?.map(|b| b.parent_id);
        Ok(before.into_iter().chain(after).collect())
    }

    /// Apply one block operation, dispatched the way the block providers
    /// dispatch it.
    pub async fn apply(&self, op_name: &str, params: &StorageEntity) -> Result<()> {
        use crate::__operations_block_operations;
        use crate::__operations_crud_operations;
        use crate::__operations_mark_operations;
        use crate::__operations_task_operations;
        use crate::__operations_text_operations;

        let removed = match op_name {
            "join_block" | "delete_keep_children" => Some("id"),
            "restore_join" => Some("deleted_id"),
            _ => None,
        };
        // The op refuses to remove a share's root and writes nothing.
        if let Some(key) = removed {
            let target = uri(param_str(params, key)?)?;
            if self.base.is_share_root(&target).await? {
                return Ok(());
            }
        }
        match op_name {
            "update" => return self.create(params.clone()).await.map(drop),
            "add_tag" | "remove_tag" => return self.tag_op(op_name, params).await,
            "rehome_entity" => {
                let id = uri(param_str(params, "id")?)?;
                return self.rehome(&id).await;
            }
            // The op's own walk chains parent ids, which a placed share's
            // page does not; the subtree goes, shares and all, either way.
            "purge" | "delete_subtree" => {
                let id = uri(param_str(params, "id")?)?;
                return self.purge(&id).await;
            }
            // Planners read; the link-resolution ops rewrite link rows. None
            // changes a tagged block or its children.
            "block_to_page_plan"
            | "merge_blocks_plan"
            | "page_chain_plan"
            | "rewrite_link_resolution"
            | "restore_link_resolution"
            | "heal_page_links" => return Ok(()),
            "dismiss_advice" => {
                param_str(params, "lesson_id")?;
                let anchor = uri(param_str(params, "anchor_id")?)?;
                return self.must_block(&anchor).await.map(drop);
            }
            _ => {}
        }
        macro_rules! attempt {
            ($module:ident) => {
                match $module::dispatch_operation::<_, Block>(self, op_name, params).await {
                    Ok(_) => return Ok(()),
                    Err(e) if UnknownOperationError::is_unknown(e.as_ref()) => {}
                    Err(e) => return Err(e),
                }
            };
        }
        attempt!(__operations_crud_operations);
        attempt!(__operations_block_operations);
        attempt!(__operations_mark_operations);
        attempt!(__operations_text_operations);
        attempt!(__operations_task_operations);
        Err(format!("no block operation `{op_name}`").into())
    }

    async fn tag_op(&self, op_name: &str, params: &StorageEntity) -> Result<()> {
        let id = uri(param_str(params, "id")?)?;
        let tag = param_str(params, "tag")?;
        let mut block = self.must_block(&id).await?;
        if op_name == "add_tag" {
            block.tags.insert(tag.to_string());
        } else {
            block.tags.remove(tag);
        }
        self.put(block);
        Ok(())
    }

    /// `rehome_entity`: a leaf moves to the root, as its first child.
    async fn rehome(&self, id: &EntityUri) -> Result<()> {
        if !self.child_ids(id).await?.is_empty() {
            return Err(format!("rehome_entity: {id} has children; only a leaf moves").into());
        }
        self.place_in_list(id, &EntityUri::no_parent(), None).await
    }

    /// Delete `id` together with its whole subtree.
    async fn purge(&self, id: &EntityUri) -> Result<()> {
        let block = self.must_block(id).await?;
        let mut list = self.child_ids(&block.parent_id).await?;
        list.retain(|c| c != id);
        self.set_list(&block.parent_id, list);
        let mut stack = vec![id.clone()];
        while let Some(next) = stack.pop() {
            stack.extend(self.child_ids(&next).await?);
            let mut overlay = self.lock();
            overlay.written.insert(next.clone());
            overlay.blocks.insert(next, None);
        }
        Ok(())
    }

    /// Put `id` under `parent`, directly after `after` (first when `None`).
    async fn place_in_list(
        &self,
        id: &EntityUri,
        parent: &EntityUri,
        after: Option<&EntityUri>,
    ) -> Result<()> {
        let mut block = self.must_block(id).await?;
        let old_parent = block.parent_id.clone();
        let mut old_list = self.child_ids(&old_parent).await?;
        old_list.retain(|c| c != id);
        self.set_list(&old_parent, old_list);
        let mut list = self.child_ids(parent).await?;
        list.retain(|c| c != id);
        let at = match after {
            None => 0,
            Some(anchor) => {
                list.iter()
                    .position(|c| c == anchor)
                    .ok_or_else(|| format!("anchor {anchor} is no child of {parent}"))?
                    + 1
            }
        };
        list.insert(at, id.clone());
        self.set_list(parent, list);
        block.parent_id = parent.clone();
        self.put(block);
        Ok(())
    }
}

fn param_str<'p>(params: &'p StorageEntity, key: &str) -> Result<&'p str> {
    params
        .get(key)
        .and_then(|v| v.as_string())
        .ok_or_else(|| format!("missing '{key}' parameter").into())
}

fn string_set(value: &Value, field: &str) -> Result<Vec<String>> {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|v| {
                v.as_string()
                    .map(str::to_string)
                    .ok_or_else(|| format!("{field}: non-string member {v:?}").into())
            })
            .collect(),
        Value::Null => Ok(Vec::new()),
        other => Err(format!("{field}: expected an array, got {other:?}").into()),
    }
}

fn content_text(value: &Value) -> Result<String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Object(obj) => obj
            .get("text")
            .and_then(|v| v.as_string())
            .map(str::to_string)
            .ok_or_else(|| "content Object without a 'text' string".into()),
        other => Err(format!("unsupported content shape {other:?}").into()),
    }
}

/// Fields `create` reads as instructions rather than properties, as the
/// block providers do.
const CREATE_HANDLED_FIELDS: [&str; 14] = [
    "parent_id",
    "content",
    "properties",
    "marks",
    "id",
    "content_type",
    "source_language",
    "source_name",
    "source_header_args",
    "source_results",
    POSITION_AFTER_BLOCK_ID_PARAM,
    "after",
    "sort_key",
    "tags",
];

/// A property write as the block providers make it: `Value::REMOVED` deletes
/// the key, anything else (`Null` included) is stored.
fn property_write(properties: &mut HashMap<String, Value>, key: &str, value: Value) {
    match value {
        Value::Removed(_) => {
            properties.remove(key);
        }
        v => {
            properties.insert(key.to_string(), v);
        }
    }
}

#[async_trait]
impl DataSource<Block> for SimStore {
    async fn get_all(&self) -> Result<Vec<Block>> {
        Err("the shape-gate simulator has no whole-store read".into())
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Block>> {
        self.block(&uri(id)?).await
    }

    async fn get_children(&self, parent_id: &EntityUri) -> Result<Vec<Block>> {
        self.ordered_children(parent_id).await
    }
}

#[async_trait]
impl BlockQueryHelpers<Block> for SimStore {
    async fn children_ordered(&self, parent_id: &EntityUri) -> Result<Vec<Block>> {
        self.ordered_children(parent_id).await
    }

    // The sibling reads below answer from the child-id list, so a structural
    // op in a long sibling list reads the one neighbour it names instead of
    // every sibling.

    async fn get_prev_sibling(&self, block_id: &EntityUri) -> Result<Option<Block>> {
        if self.must_block(block_id).await?.parent_id.is_no_parent() {
            return Ok(None);
        }
        match <Self as BlockOrdering>::prev_sibling(self, block_id).await? {
            Some(id) => self.block(&id).await,
            None => Ok(None),
        }
    }

    async fn get_next_sibling(&self, block_id: &EntityUri) -> Result<Option<Block>> {
        if self.must_block(block_id).await?.parent_id.is_no_parent() {
            return Ok(None);
        }
        match <Self as BlockOrdering>::next_sibling(self, block_id).await? {
            Some(id) => self.block(&id).await,
            None => Ok(None),
        }
    }

    async fn get_first_child(&self, parent_id: Option<&EntityUri>) -> Result<Option<Block>> {
        let Some(parent) = parent_id else {
            return Ok(None);
        };
        match self.child_ids(parent).await?.first() {
            Some(id) => self.block(id).await,
            None => Ok(None),
        }
    }

    async fn get_last_child(&self, parent_id: Option<&EntityUri>) -> Result<Option<Block>> {
        let Some(parent) = parent_id else {
            return Ok(None);
        };
        match self.child_ids(parent).await?.last() {
            Some(id) => self.block(id).await,
            None => Ok(None),
        }
    }
}

#[async_trait]
impl CrudOperations<Block> for SimStore {
    async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult> {
        let id = uri(id)?;
        let mut block = self.must_block(&id).await?;
        match field {
            "content" => block.content = content_text(&value)?,
            "parent_id" => {
                let parent = uri(value
                    .as_string()
                    .ok_or("set_field(parent_id): value is not a string")?)?;
                let last = self.child_ids(&parent).await?.pop();
                self.place_in_list(&id, &parent, last.as_ref()).await?;
                return Ok(OperationResult::irreversible(Vec::new()));
            }
            "tags" => block.tags = Tags::from(string_set(&value, field)?),
            f if EdgeField::is_edge_column(f) => {}
            "marks" | "content_type" | "source_language" | "source_name" | "collapsed"
            | "widget_only" | "block_type" => {}
            "task_state" => match holon_api::TaskStateWrite::parse(&value)? {
                holon_api::TaskStateWrite::Set(state) => {
                    block
                        .properties
                        .insert(field.to_string(), Value::String(state.keyword));
                }
                holon_api::TaskStateWrite::Clear => {
                    block.properties.remove(field);
                }
            },
            key => property_write(&mut block.properties, key, value),
        }
        self.put(block);
        Ok(OperationResult::irreversible(Vec::new()))
    }

    async fn create(&self, fields: StorageEntity) -> Result<(String, OperationResult)> {
        // A parent the create does not name: an upsert keeps the block where
        // it is; a new block is rooted by the SQL write authority (the Loro
        // one refuses it). An empty or null parent is the root.
        let named_parent = match fields.get("parent_id") {
            None => None,
            Some(v) => match v.as_string() {
                Some(raw) if !raw.is_empty() => Some(uri(raw)?),
                _ => Some(EntityUri::no_parent()),
            },
        };
        let id = match fields.get("id").and_then(|v| v.as_string()) {
            Some(raw) => uri(raw)?,
            // ALLOW(identity_minting): a throwaway key for the overlay; the id
            // never leaves the simulator, the provider mints the real one.
            None => EntityUri::block_random(),
        };
        let content = match fields.get("content") {
            Some(v) => content_text(v)?,
            None => String::new(),
        };
        let mut props: HashMap<String, Value> = fields
            .iter()
            .filter(|(k, _)| {
                !CREATE_HANDLED_FIELDS.contains(&k.as_ref()) && !EdgeField::is_edge_column(k)
            })
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        match fields.get("properties") {
            None => {}
            Some(Value::Object(bag)) => {
                for (k, v) in bag {
                    props.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            Some(Value::String(json)) => {
                let bag: HashMap<String, Value> = serde_json::from_str(json)
                    .map_err(|e| format!("create: 'properties' is not a JSON object: {e}"))?;
                for (k, v) in bag {
                    props.entry(k).or_insert(v);
                }
            }
            Some(other) => {
                return Err(
                    format!("create: 'properties' must be an object, got {other:?}").into(),
                );
            }
        }
        let anchor = match fields
            .get(POSITION_AFTER_BLOCK_ID_PARAM)
            .or_else(|| fields.get("after"))
        {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::String(a)) => Some(Some(uri(a)?)),
            Some(other) => {
                return Err(format!("create: anchor must be a string, got {other:?}").into());
            }
        };

        let existing = self.block(&id).await?;
        let mut block = match existing {
            Some(mut b) => {
                if let Some(parent) = named_parent.as_ref().filter(|p| **p != b.parent_id) {
                    self.place_in_list(&id, parent, None).await?;
                    b = self.must_block(&id).await?;
                }
                b.content = content;
                b
            }
            None => {
                let parent = named_parent.unwrap_or_else(EntityUri::no_parent);
                let b = Block::new_text(id.clone(), parent.clone(), &content);
                self.put(b.clone());
                let mut list = self.child_ids(&parent).await?;
                list.push(id.clone());
                self.set_list(&parent, list);
                b
            }
        };
        if let Some(tags) = fields.get("tags") {
            block.tags = Tags::from(string_set(tags, "tags")?);
        }
        for (k, v) in props {
            property_write(&mut block.properties, &k, v);
        }
        self.put(block);
        if let Some(after) = anchor {
            let parent = self.must_block(&id).await?.parent_id;
            self.place_in_list(&id, &parent, after.as_ref()).await?;
        }
        Ok((id.to_string(), OperationResult::irreversible(Vec::new())))
    }

    /// The SQL write authority cascades a delete to the subtree and the Loro
    /// one refuses a non-leaf; the cascade is the result either may reach.
    async fn delete(&self, id: &str) -> Result<OperationResult> {
        self.purge(&uri(id)?).await?;
        Ok(OperationResult::irreversible(Vec::new()))
    }
}

/// A structural op decides only on blocks the write authority holds (D64.b),
/// as the block providers do: one it does not hold, and the plan did not write,
/// is refused by name.
#[async_trait]
impl BlockDataSourceHelpers<Block> for SimStore {
    async fn block_authoritative(&self, id: &EntityUri) -> Result<Option<Block>> {
        let block = self.block(id).await?;
        if block.is_none() && !id.is_no_parent() && !self.lock().written.contains(id) {
            return Err(BlockNotInWriteAuthority::new(id.clone(), ProjectionRead::NotRead).into());
        }
        Ok(block)
    }

    async fn prev_sibling_authoritative(&self, id: &EntityUri) -> Result<Option<Block>> {
        self.block_authoritative(id).await?;
        self.get_prev_sibling(id).await
    }

    async fn next_sibling_authoritative(&self, id: &EntityUri) -> Result<Option<Block>> {
        self.block_authoritative(id).await?;
        self.get_next_sibling(id).await
    }
}

impl BlockOperations<Block> for SimStore {
    fn ordering(&self) -> Option<&dyn BlockOrdering> {
        Some(self)
    }

    fn order_key_minter(&self) -> Option<&dyn OrderKeyMinting> {
        Some(self)
    }
}

#[async_trait]
impl BlockOrdering for SimStore {
    async fn place(
        &self,
        uri: &EntityUri,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
    ) -> Result<()> {
        self.place_in_list(uri, parent_id, after_id).await
    }

    async fn prev_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
        let parent = self.must_block(id).await?.parent_id;
        let list = self.child_ids(&parent).await?;
        let at = list.iter().position(|c| c == id);
        Ok(at.and_then(|i| i.checked_sub(1)).map(|i| list[i].clone()))
    }

    async fn next_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
        let parent = self.must_block(id).await?.parent_id;
        let list = self.child_ids(&parent).await?;
        let at = list.iter().position(|c| c == id);
        Ok(at.and_then(|i| list.get(i + 1)).cloned())
    }

    async fn first_child(&self, parent_id: &EntityUri) -> Result<Option<EntityUri>> {
        Ok(self.child_ids(parent_id).await?.first().cloned())
    }

    async fn last_child(&self, parent_id: &EntityUri) -> Result<Option<EntityUri>> {
        Ok(self.child_ids(parent_id).await?.last().cloned())
    }

    async fn update_in_tree(&self, _: StorageEntity) -> Result<()> {
        Err("the shape-gate simulator models no ingest writes".into())
    }

    async fn delete_in_tree(&self, _: StorageEntity) -> Result<()> {
        Err("the shape-gate simulator models no ingest writes".into())
    }

    async fn children(&self, parent_id: &EntityUri) -> Result<Vec<EntityUri>> {
        self.child_ids(parent_id).await
    }
}

#[async_trait]
impl OrderKeyMinting for SimStore {
    // ALLOW(order_minting): the simulator orders by list position; the key is
    // never stored.
    async fn new_child_anchor(
        &self,
        _: &EntityUri,
        _: Option<&EntityUri>,
    ) -> Result<MintedPosition> {
        Ok(MintedPosition::alone(String::new()))
    }
}

#[async_trait]
impl TaskOperations<Block> for SimStore {
    async fn set_title(&self, id: &str, title: &str) -> Result<OperationResult> {
        let block = self.must_block(&uri(id)?).await?;
        let body: Vec<&str> = block.content.lines().skip(1).collect();
        let content = if body.is_empty() {
            title.to_string()
        } else {
            format!("{title}\n{}", body.join("\n"))
        };
        self.set_field(id, "content", Value::String(content)).await
    }

    fn completion_states_with_progress(&self) -> Vec<CompletionStateInfo> {
        ["TODO", "DOING", "DONE"]
            .into_iter()
            .map(|state| CompletionStateInfo {
                state: state.into(),
                progress: 0.0,
                is_done: state == "DONE",
                is_active: state != "DONE",
            })
            .collect()
    }

    async fn set_state(&self, id: &str, state: String) -> Result<OperationResult> {
        let write = if state.is_empty() {
            holon_api::TaskStateWrite::Clear
        } else {
            holon_api::TaskStateWrite::Set(holon_api::TaskState::from_keyword(&state))
        };
        self.set_field(id, "task_state", write.to_value()).await
    }

    async fn cycle_task_state(&self, id: &str) -> Result<OperationResult> {
        let block = self.must_block(&uri(id)?).await?;
        let current = block.get_property_str("task_state").unwrap_or_default();
        let states: Vec<String> = std::iter::once(String::new())
            .chain(
                self.completion_states_with_progress()
                    .into_iter()
                    .map(|s| s.state),
            )
            .collect();
        let next = holon_api::render_eval::cycle_state(&current, &states);
        self.set_state(id, next).await
    }

    async fn set_priority(&self, id: &str, priority: i64) -> Result<OperationResult> {
        self.set_field(id, "PRIORITY", Value::Integer(priority))
            .await
    }

    async fn set_due_date(
        &self,
        id: &str,
        due_date: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<OperationResult> {
        let value = match due_date {
            Some(dt) => Value::String(dt.to_rfc3339()),
            None => Value::REMOVED,
        };
        self.set_field(id, "DEADLINE", value).await
    }
}

#[async_trait]
impl TextOperations<Block> for SimStore {
    async fn insert_text(&self, id: &str, pos: i64, text: String) -> Result<OperationResult> {
        let pos = usize::try_from(pos).map_err(|_| format!("insert_text: negative pos {pos}"))?;
        let block = self.must_block(&uri(id)?).await?;
        let mut chars: Vec<char> = block.content.chars().collect();
        if pos > chars.len() {
            return Err(format!("insert_text: pos {pos} beyond {} scalars", chars.len()).into());
        }
        chars.splice(pos..pos, text.chars());
        self.set_field(id, "content", Value::String(chars.into_iter().collect()))
            .await
    }

    async fn delete_text(&self, id: &str, pos: i64, len: i64) -> Result<OperationResult> {
        let pos = usize::try_from(pos).map_err(|_| format!("delete_text: negative pos {pos}"))?;
        let len = usize::try_from(len).map_err(|_| format!("delete_text: negative len {len}"))?;
        let block = self.must_block(&uri(id)?).await?;
        let mut chars: Vec<char> = block.content.chars().collect();
        if pos + len > chars.len() {
            return Err(format!("delete_text: {pos}+{len} beyond {} scalars", chars.len()).into());
        }
        chars.drain(pos..pos + len);
        self.set_field(id, "content", Value::String(chars.into_iter().collect()))
            .await
    }
}

#[async_trait]
impl MarkOperations<Block> for SimStore {
    async fn apply_mark(&self, id: &str, _: i64, _: i64, _: String) -> Result<OperationResult> {
        self.must_block(&uri(id)?).await?;
        Ok(OperationResult::irreversible(Vec::new()))
    }

    async fn remove_mark(&self, id: &str, _: i64, _: i64, _: String) -> Result<OperationResult> {
        self.must_block(&uri(id)?).await?;
        Ok(OperationResult::irreversible(Vec::new()))
    }
}
