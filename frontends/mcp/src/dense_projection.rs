//! Dense projection registry + the pure projection builder for the
//! `dense_query`/`dense_patch` MCP tool pair.
//!
//! `dense_query` runs the caller's ordinary GQL/PRQL/SQL query through the
//! existing engine (exactly like `execute_query`), takes the resulting block
//! set, and renders it as dense org text (see [`holon_org_format::dense`]):
//! each headline's `:PROPERTIES:/:ID:/:END:` drawer is compressed to a trailing
//! `{#alias}` token. It returns an opaque `projection_handle`; the server keeps
//! handle → per-block {true parent, projection position, alias, version} so the
//! sibling `dense_patch` can resolve edited handles, diff structure RELATIVE to
//! the projection, and reject on concurrent edits.
//!
//! ## Trees with holes
//! A query can select an arbitrary block set — a kept block whose parent was
//! NOT selected would be an orphan the org renderer rejects. So a block whose
//! immediate parent is absent from the result is rendered at the projection top
//! level (nearest-surviving-ancestor, collapsed to the immediate parent since
//! that is all a flat result exposes). This re-rooting is COSMETIC: the true
//! parent is recorded in the handle, and `dense_patch` emits a move ONLY when a
//! block's enclosing rendered block / relative order actually changes in the
//! edited text — an untouched re-rooted block never moves.
//!
//! ## Concurrency token
//! A block's `updated_at` (bumped by every content/field write), captured at
//! projection. Honors the EBO dirty-editor policy — a block edited between
//! project and patch fails loud rather than being clobbered. (A pure structural
//! move that does not bump `updated_at` is not version-guarded; write_seq is a
//! possible future refinement.)

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use anyhow::bail;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_org_format::AliasTable;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgDocumentExt;
use holon_org_format::render_dense;

/// The synthetic render root used when the projection's roots do not share one
/// real parent (a query spanning multiple parents). Blocks re-rooted here have
/// no single natural home; `dense_patch` refuses to create a NEW top-level
/// block against it.
pub const SYNTHETIC_ROOT: &str = "dense-projection-root";

/// Per-block optimistic-concurrency token captured at projection time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockVersion {
    pub updated_at: i64,
}

impl BlockVersion {
    pub fn of(block: &Block) -> BlockVersion {
        BlockVersion {
            updated_at: block.updated_at,
        }
    }
}

/// What the handle records about one projected block, so the patch can diff
/// structure relative to the projection and place edits correctly.
#[derive(Clone, Debug)]
pub struct ProjectedBlock {
    pub block_id: EntityUri,
    /// The block's real parent at projection time (authoritative for moves).
    pub true_parent: EntityUri,
    /// The block's parent AS RENDERED: `Some(parent block id)` when the parent
    /// was also selected, else `None` (rendered at projection top level).
    pub proj_parent: Option<EntityUri>,
    /// Order among projection siblings (blocks sharing the same `proj_parent`),
    /// in render order.
    pub proj_index: usize,
    /// Whether this block is rendered with the elided-ancestor gap marker: its
    /// true parent was NOT selected and is not the render root.
    pub gap: bool,
    /// The block's title (first content line) at projection time — the baseline
    /// the patch diffs against to detect a retitle.
    pub title: String,
    /// The block's task state at projection time — baseline for a state change.
    pub task_state: Option<holon_api::types::TaskState>,
    pub version: BlockVersion,
}

/// A captured projection: everything a later patch needs.
#[derive(Clone, Debug)]
pub struct Projection {
    /// The query that produced it (for diagnostics / re-projection).
    pub query: String,
    /// Render root id (`file_id`): the roots' shared real parent, or
    /// [`SYNTHETIC_ROOT`]. New top-level blocks anchor here.
    pub file_id: EntityUri,
    pub alias_table: AliasTable,
    /// block-id → projection record.
    pub records: HashMap<String, ProjectedBlock>,
    created: Instant,
}

impl Projection {
    pub fn new(
        query: String,
        file_id: EntityUri,
        alias_table: AliasTable,
        records: HashMap<String, ProjectedBlock>,
    ) -> Projection {
        Projection {
            query,
            file_id,
            alias_table,
            records,
            created: Instant::now(),
        }
    }
}

/// In-memory handle → [`Projection`] store with a TTL.
#[derive(Clone)]
pub struct ProjectionRegistry {
    inner: Arc<Mutex<HashMap<String, Projection>>>,
    ttl: Duration,
}

impl ProjectionRegistry {
    pub fn new(ttl: Duration) -> ProjectionRegistry {
        ProjectionRegistry {
            inner: Arc::new(Mutex::new(HashMap::new())),
            ttl,
        }
    }

    /// Store a projection and return a fresh opaque handle. Opportunistically
    /// evicts expired entries.
    pub fn insert(&self, projection: Projection) -> String {
        let handle = format!("proj:{}", uuid::Uuid::new_v4());
        let mut map = self.inner.lock().expect("projection registry poisoned");
        map.retain(|_, p| p.created.elapsed() < self.ttl);
        map.insert(handle.clone(), projection);
        handle
    }

    /// Resolve a handle. Fails loud (never guesses) on an unknown or expired
    /// handle — a stale handle is a caller error the patch tool must surface.
    pub fn get(&self, handle: &str) -> Result<Projection> {
        let map = self.inner.lock().expect("projection registry poisoned");
        match map.get(handle) {
            Some(p) if p.created.elapsed() < self.ttl => Ok(p.clone()),
            Some(_) => bail!(
                "projection handle {handle} has expired (TTL {}s) — re-run dense_query to get a \
                 fresh projection",
                self.ttl.as_secs()
            ),
            None => bail!(
                "unknown projection handle {handle} — it was never issued or has been evicted; \
                 re-run dense_query"
            ),
        }
    }
}

/// The result of building a projection: the dense text plus everything needed
/// to register the handle.
pub struct BuiltProjection {
    pub file_id: EntityUri,
    pub ordered_blocks: Vec<Block>,
    pub alias_table: AliasTable,
    pub records: HashMap<String, ProjectedBlock>,
    pub dense_text: String,
}

/// Build a dense projection from a query's block result. Pure (no I/O), so it
/// is unit- and PBT-testable. Does NOT filter — the query already selected the
/// blocks. Re-roots blocks whose parent is absent (see module docs), assigns
/// aliases, records per-block structure/version, and renders.
///
/// `blocks` arrive in the query's result order; that order is preserved among
/// siblings. Page (container) blocks are dropped — they are not content.
pub fn build_projection(blocks: Vec<Block>) -> Result<BuiltProjection> {
    let selected: Vec<Block> = blocks.into_iter().filter(|b| !b.is_page()).collect();
    let in_set: HashSet<String> = selected.iter().map(|b| b.id.as_str().to_string()).collect();

    // Projection parent of each block: its real parent if that parent is also
    // selected, else None (rendered at top level).
    let proj_parent = |b: &Block| -> Option<EntityUri> {
        if in_set.contains(b.parent_id.as_str()) {
            Some(b.parent_id.clone())
        } else {
            None
        }
    };

    // Render root: the shared real parent of all top-level (re-rooted) blocks
    // when unique, else the synthetic root.
    let root_true_parents: HashSet<String> = selected
        .iter()
        .filter(|b| proj_parent(b).is_none())
        .map(|b| b.parent_id.as_str().to_string())
        .collect();
    let file_id = if root_true_parents.len() == 1 {
        selected
            .iter()
            .find(|b| proj_parent(b).is_none())
            .map(|b| b.parent_id.clone())
            .expect("one root parent exists")
    } else {
        EntityUri::block(SYNTHETIC_ROOT)
    };

    // Render copies: parent_id rewritten to the projection parent (or file_id
    // for a top-level block).
    let mut render_blocks: Vec<Block> = Vec::with_capacity(selected.len());
    for b in &selected {
        let mut rb = b.clone();
        rb.parent_id = proj_parent(b).unwrap_or_else(|| file_id.clone());
        render_blocks.push(rb);
    }

    // Pre-order so parents precede children and proj_index is stable.
    let ordered = preorder(&render_blocks, &file_id);

    let alias_table = AliasTable::assign(ordered.iter().map(|b| b.id.clone()));

    // Per-block records. proj_index = position among siblings sharing the same
    // proj_parent, in pre-order.
    let true_parent_of: HashMap<&str, EntityUri> = selected
        .iter()
        .map(|b| (b.id.as_str(), b.parent_id.clone()))
        .collect();
    let mut sibling_counter: HashMap<String, usize> = HashMap::new();
    let mut records: HashMap<String, ProjectedBlock> = HashMap::new();
    let mut gap_ids: HashSet<String> = HashSet::new();
    for rb in &ordered {
        let parent_key = rb.parent_id.as_str().to_string();
        let idx = sibling_counter.entry(parent_key).or_insert(0);
        let proj_index = *idx;
        *idx += 1;

        let true_parent = true_parent_of
            .get(rb.id.as_str())
            .cloned()
            .expect("every rendered block came from the selected set");
        let proj_parent = if rb.parent_id == file_id {
            None
        } else {
            Some(rb.parent_id.clone())
        };
        // Gap: the block's true parent was elided (not selected) and is not the
        // render root/container. The page container is not an "elided ancestor".
        let gap = !in_set.contains(true_parent.as_str()) && true_parent != file_id;
        if gap {
            gap_ids.insert(rb.id.as_str().to_string());
        }
        records.insert(
            rb.id.as_str().to_string(),
            ProjectedBlock {
                block_id: rb.id.clone(),
                true_parent,
                proj_parent,
                proj_index,
                gap,
                title: rb.org_title(),
                task_state: rb.task_state(),
                version: BlockVersion::of(rb),
            },
        );
    }

    let doc_block = synth_doc_block(&file_id, &ordered);
    let dense_text = render_dense(&doc_block, &ordered, &file_id, &alias_table, &gap_ids);

    Ok(BuiltProjection {
        file_id,
        ordered_blocks: ordered,
        alias_table,
        records,
        dense_text,
    })
}

/// Pre-order the blocks (parent before child) from `file_id`, preserving input
/// order among siblings. Blocks unreachable from `file_id` are appended in
/// input order (should not happen after re-rooting).
fn preorder(blocks: &[Block], file_id: &EntityUri) -> Vec<Block> {
    let mut children_by_parent: HashMap<&str, Vec<&Block>> = HashMap::new();
    for b in blocks {
        children_by_parent
            .entry(b.parent_id.as_str())
            .or_default()
            .push(b);
    }
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    let mut visited: HashSet<&str> = HashSet::new();
    fn walk<'a>(
        parent: &str,
        children_by_parent: &HashMap<&'a str, Vec<&'a Block>>,
        out: &mut Vec<Block>,
        visited: &mut HashSet<&'a str>,
    ) {
        if let Some(kids) = children_by_parent.get(parent) {
            for kid in kids {
                if visited.insert(kid.id.as_str()) {
                    out.push((*kid).clone());
                    walk(kid.id.as_str(), children_by_parent, out, visited);
                }
            }
        }
    }
    walk(
        file_id.as_str(),
        &children_by_parent,
        &mut out,
        &mut visited,
    );
    for b in blocks {
        if visited.insert(b.id.as_str()) {
            out.push(b.clone());
        }
    }
    out
}

/// Build a projection-only document block whose `#+TODO:` config covers every
/// distinct task-state keyword in `blocks`, so parse_dense recovers each
/// block's category regardless of the vault's custom keyword dialect.
fn synth_doc_block(file_id: &EntityUri, blocks: &[Block]) -> Block {
    use holon_api::types::TaskState;
    let mut seen: Vec<TaskState> = Vec::new();
    for b in blocks {
        if let Some(st) = b.task_state() {
            if !seen.iter().any(|s| s.keyword == st.keyword) {
                seen.push(st);
            }
        }
    }
    let mut doc = Block::new_text(
        file_id.clone(),
        EntityUri::block("dense-projection-anchor"),
        "Projection".to_string(),
    );
    doc.set_page(true);
    if !seen.is_empty() {
        doc.set_todo_keywords(Some(seen));
    }
    doc
}

#[cfg(test)]
mod tests {
    use holon_api::types::TaskState;

    use super::*;

    fn blk(id: &str, parent: &str, title: &str, state: Option<TaskState>) -> Block {
        let mut b = Block::new_text(
            EntityUri::block(id),
            EntityUri::block(parent),
            title.to_string(),
        );
        b.set_task_state(state);
        b
    }

    /// A flat result of the page's active tasks (children of page P, not
    /// selected) all render at top level; each gets a distinct alias.
    #[test]
    fn flat_result_renders_all_at_top_level() {
        let all = vec![
            blk("a", "P", "alpha", Some(TaskState::active("TODO"))),
            blk("b", "P", "beta", Some(TaskState::active("NEXT"))),
        ];
        let built = build_projection(all).unwrap();
        assert_eq!(
            built.file_id,
            EntityUri::block("P"),
            "shared parent = file_id"
        );
        assert_eq!(built.alias_table.len(), 2);
        assert!(built.records["block:a"].proj_parent.is_none());
        assert!(!built.dense_text.contains(":ID:"));
        assert!(built.dense_text.contains("{#"));
    }

    /// A selected parent + selected child nest; the child's proj_parent and
    /// true_parent are the parent.
    #[test]
    fn selected_parent_child_nest() {
        let all = vec![blk("a", "P", "alpha", None), blk("a1", "a", "child", None)];
        let built = build_projection(all).unwrap();
        assert_eq!(
            built.records["block:a1"].proj_parent,
            Some(EntityUri::block("a"))
        );
        assert_eq!(built.records["block:a1"].true_parent, EntityUri::block("a"));
        assert!(built.dense_text.contains("** "), "child renders at level 2");
    }

    /// A hole: only A (under P) and C (under absent B) are selected. C re-roots
    /// to top level but its true_parent stays B; roots span two parents so the
    /// render root is synthetic.
    #[test]
    fn hole_reroots_child_but_records_true_parent() {
        let all = vec![blk("a", "P", "alpha", None), blk("c", "b", "gamma", None)];
        let built = build_projection(all).unwrap();
        assert_eq!(built.file_id, EntityUri::block(SYNTHETIC_ROOT));
        assert!(built.records["block:c"].proj_parent.is_none());
        assert_eq!(built.records["block:c"].true_parent, EntityUri::block("b"));
    }
}
