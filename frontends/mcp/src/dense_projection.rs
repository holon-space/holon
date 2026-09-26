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

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use holon_api::EntityUri;
use holon_api::Tags;
use holon_api::block::Block;
use holon_api::types::TaskState;
use holon_org_format::Alias;
use holon_org_format::AliasTable;
use holon_org_format::DenseBlock;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgDocumentExt;
use holon_org_format::TaskKeywordVocabulary;
use holon_org_format::is_headline;
use holon_org_format::models::is_hidden_drawer_key;
use holon_org_format::parse_dense;
use holon_org_format::render_dense;

use crate::dense_patch::RowAttributes;
use crate::dense_patch::RowView;

/// The synthetic render root used when the projection's roots do not share one
/// real parent (a query spanning multiple parents). Blocks re-rooted here have
/// no single natural home; `dense_patch` refuses to create a NEW top-level
/// block against it.
pub const SYNTHETIC_ROOT: &str = "dense-projection-root";

/// The task keywords of the document a block's children live in, by that
/// block's id: each row is read, and judged, by its OWN document's ring.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocVocabularies {
    /// Every document has this vocabulary (a test over a single file).
    Uniform(TaskKeywordVocabulary),
    /// Keyed by parent block id; a parent the query result needs must be
    /// present.
    ByParent(HashMap<String, TaskKeywordVocabulary>),
}

impl DocVocabularies {
    /// The vocabulary of the document a child of `parent` lives in.
    pub fn under(&self, parent: &EntityUri) -> &TaskKeywordVocabulary {
        match self {
            DocVocabularies::Uniform(v) => v,
            DocVocabularies::ByParent(map) => map.get(parent.as_str()).unwrap_or_else(|| {
                panic!("no task vocabulary was read for the document under {parent}")
            }),
        }
    }

    /// The parents read here.
    pub fn parents(&self) -> Vec<String> {
        match self {
            DocVocabularies::ByParent(map) => map.keys().cloned().collect(),
            DocVocabularies::Uniform(_) => panic!("a uniform vocabulary is not read per parent"),
        }
    }

    /// The parents whose document `now` reads with another vocabulary.
    pub fn changed_in(&self, now: &DocVocabularies) -> Vec<String> {
        let (DocVocabularies::ByParent(then), DocVocabularies::ByParent(now)) = (self, now) else {
            panic!("vocabularies are compared per parent")
        };
        let mut changed: Vec<String> = then
            .iter()
            .filter(|(parent, vocabulary)| now.get(parent.as_str()) != Some(vocabulary))
            .map(|(parent, _)| parent.clone())
            .collect();
        changed.sort();
        changed
    }
}

/// `keyword` as `vocabulary` classifies it.
pub fn state_in(vocabulary: &TaskKeywordVocabulary, keyword: &str) -> TaskState {
    if vocabulary.done_keywords().iter().any(|k| k == keyword) {
        TaskState::done(keyword)
    } else {
        TaskState::active(keyword)
    }
}

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
    /// The block as stored, which a plan's writes start from.
    pub stored: Block,
    /// The stored task state, which the shown row may not match: org reads a
    /// stored keyword-headed title as a task.
    pub task_state: Option<TaskState>,
    /// The stored tags, which a headline edit replaces.
    pub tags: Tags,
    /// The stored authored drawer key order, which places drawer lines.
    pub drawer_order: Vec<String>,
    /// The stored carriers an edit of the row's text leaves in place.
    pub kept: holon_org_format::KeptCarriers,
    /// The task keywords of the block's own document, which is also the
    /// document its children live in.
    pub vocabulary: TaskKeywordVocabulary,
    /// The row the projection text shows for this block, parsed back: what an
    /// edit is detected against. `None` for a block rendered without a token.
    pub shown: Option<RowView>,
    /// That row's text, as [`holon_org_format::row_lines`] reads it: a row
    /// whose text is unchanged is not an edit, whatever org reads it as.
    pub shown_lines: Vec<String>,
    pub version: BlockVersion,
}

/// A captured projection: everything a later patch needs.
#[derive(Clone, Debug)]
pub struct Projection {
    /// The query that produced it (for diagnostics / re-projection).
    pub query: String,
    /// The task keywords of the document a new top-level row lands in; `None`
    /// under the synthetic root, where no new top-level row may land.
    pub root_vocabulary: Option<TaskKeywordVocabulary>,
    /// The document vocabularies the projection was read with; a patch
    /// against a document whose vocabulary changed since is a conflict.
    pub vocabularies: DocVocabularies,
    /// The non-blank lines the text shows before its first row.
    pub header: Vec<String>,
    /// Rows whose text does not parse back as one row; a patch against this
    /// projection is refused.
    pub unreadable: BTreeMap<Alias, String>,
    /// Render root id (`file_id`): the roots' shared real parent, or
    /// [`SYNTHETIC_ROOT`]. New top-level blocks anchor here.
    pub file_id: EntityUri,
    pub alias_table: AliasTable,
    /// block-id → projection record.
    pub records: HashMap<String, ProjectedBlock>,
    created: Instant,
}

impl Projection {
    pub fn new(query: String, built: &BuiltProjection) -> Projection {
        Projection {
            query,
            header: preamble(&built.dense_text)
                .into_iter()
                .map(str::to_string)
                .collect(),
            unreadable: built.unreadable.clone(),
            root_vocabulary: built.root_vocabulary.clone(),
            vocabularies: built.vocabularies.clone(),
            file_id: built.file_id.clone(),
            alias_table: built.alias_table.clone(),
            records: built.records.clone(),
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
    /// The task keywords of the document a new top-level row lands in; `None`
    /// under the synthetic root, where no new top-level row may land.
    pub root_vocabulary: Option<TaskKeywordVocabulary>,
    pub vocabularies: DocVocabularies,
    pub ordered_blocks: Vec<Block>,
    pub alias_table: AliasTable,
    pub records: HashMap<String, ProjectedBlock>,
    pub dense_text: String,
    /// Per row, the stored property keys its drawer does not show, because
    /// an org drawer line cannot spell them. Rows that show every property
    /// are absent.
    pub omitted: BTreeMap<Alias, Vec<String>>,
    /// Per row, how its text shows the block otherwise than it is stored.
    pub unfaithful: BTreeMap<Alias, String>,
    /// The rows of [`Self::unfaithful`] whose text does not even parse back
    /// as one row; a patch of the projection is refused.
    pub unreadable: BTreeMap<Alias, String>,
}

/// The non-blank lines of `text` before its first org headline.
pub fn preamble(text: &str) -> Vec<&str> {
    text.lines()
        .take_while(|line| !is_headline(line))
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect()
}

/// Build a dense projection from a query's block result. Pure (no I/O), so it
/// is unit- and PBT-testable. Does NOT filter — the query already selected the
/// blocks. Re-roots blocks whose parent is absent (see module docs), assigns
/// aliases, records per-block structure/version, and renders.
///
/// `blocks` arrive in the query's result order; that order is preserved among
/// siblings. Page (container) blocks are dropped — they are not content.
pub fn build_projection(
    blocks: Vec<Block>,
    vocabularies: &DocVocabularies,
) -> Result<BuiltProjection> {
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
    let selected_by_id: HashMap<&str, &Block> =
        selected.iter().map(|b| (b.id.as_str(), b)).collect();
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
        let vocabulary = vocabularies.under(&true_parent).clone();
        records.insert(
            rb.id.as_str().to_string(),
            ProjectedBlock {
                block_id: rb.id.clone(),
                true_parent,
                proj_parent,
                proj_index,
                gap,
                stored: selected_by_id[rb.id.as_str()].clone(),
                task_state: rb.task_state(),
                tags: rb.tags(),
                drawer_order: rb
                    .authored_drawer_order()
                    .with_context(|| format!("block {} holds an unreadable drawer order", rb.id))?,
                kept: holon_org_format::KeptCarriers::of(rb),
                vocabulary,
                shown: None,
                shown_lines: Vec::new(),
                version: BlockVersion::of(rb),
            },
        );
    }

    let root_vocabulary = (file_id.id() != SYNTHETIC_ROOT).then(|| vocabularies.under(&file_id));
    let doc_block = synth_doc_block(&file_id, &ordered, &records, root_vocabulary);
    let dense_text = render_dense(&doc_block, &ordered, &file_id, &alias_table, &gap_ids)?;

    // Parsing the rendered text back is what makes an unedited row diff to
    // nothing, whatever the store's own spelling is.
    let reparsed = parse_dense(&dense_text)?;
    let stored: HashMap<&str, &Block> = ordered.iter().map(|b| (b.id.as_str(), b)).collect();
    let mut omitted = BTreeMap::new();
    let mut unfaithful = BTreeMap::new();
    let mut unreadable = BTreeMap::new();
    let mut previous: Option<&Alias> = None;
    for row in &reparsed.blocks {
        let Some(alias) = &row.alias else {
            let owner = previous.unwrap_or_else(|| {
                panic!("a row without a token precedes every projected row:\n{dense_text}")
            });
            let title = row.block.org_title();
            let line = dense_text
                .lines()
                .find(|l| is_headline(l) && l.trim_start_matches('*').trim() == title)
                .unwrap_or(&title);
            unreadable.entry(owner.clone()).or_insert_with(|| {
                format!(
                    "the body line `{line}` reads back as a row of its own, so the text cannot \
                     show this block"
                )
            });
            continue;
        };
        previous = Some(alias);
        let id = alias_table
            .id_of(alias)
            .unwrap_or_else(|| panic!("rendered alias {alias} is not in the projection table"));
        let record = records
            .get_mut(id.as_str())
            .unwrap_or_else(|| panic!("rendered alias {alias} has no projection record"));
        if record.shown.is_some() {
            unreadable.insert(
                alias.clone(),
                "its token appears on more than one row of the text".to_string(),
            );
        }
        record.shown = Some(RowView::of(row)?);
        let hidden = unshown_property_keys(stored[id.as_str()], row);
        if !hidden.is_empty() {
            omitted.insert(alias.clone(), hidden.clone());
        }
        if let Some(why) = shown_unlike_stored(stored[id.as_str()], row, &hidden) {
            unfaithful.insert(alias.clone(), why);
        }
    }
    for row in holon_org_format::dense_rows(&dense_text) {
        let Some(alias) = holon_org_format::row_alias(&row)? else {
            continue;
        };
        let id = alias_table
            .id_of(&alias)
            .unwrap_or_else(|| panic!("rendered alias {alias} is not in the projection table"));
        records
            .get_mut(id.as_str())
            .unwrap_or_else(|| panic!("rendered alias {alias} has no projection record"))
            .shown_lines = holon_org_format::row_lines(&row)?;
    }
    for (alias, why) in &unreadable {
        unfaithful.insert(alias.clone(), why.clone());
    }

    Ok(BuiltProjection {
        root_vocabulary: root_vocabulary.cloned(),
        vocabularies: vocabularies.clone(),
        file_id,
        ordered_blocks: ordered,
        alias_table,
        records,
        dense_text,
        omitted,
        unfaithful,
        unreadable,
    })
}

/// How `row`, the text `block` renders to read back by org, shows the block
/// otherwise than it is stored, or `None` when it shows it as stored. Keys the
/// drawer cannot spell are disclosed as omitted instead.
fn shown_unlike_stored(block: &Block, row: &DenseBlock, omitted: &[String]) -> Option<String> {
    let mut differences = Vec::new();
    let keyword = |state: Option<holon_api::types::TaskState>| state.map(|s| s.keyword);
    let (stored_state, shown_state) =
        (keyword(block.task_state()), keyword(row.block.task_state()));
    if stored_state != shown_state {
        differences.push(format!(
            "task state {} is shown as {}",
            stored_state.as_deref().unwrap_or("(none)"),
            shown_state.as_deref().unwrap_or("(none)")
        ));
    }
    let (stored_title, stored_body) = match block.content.split_once('\n') {
        Some((title, body)) => (title, Some(body.to_string())),
        None => (block.content.as_str(), None),
    };
    let (shown_title, shown_tags) = (row.block.org_title(), row.block.tags());
    if stored_title.trim_end() != shown_title || block.tags() != shown_tags {
        differences.push(format!(
            "title {stored_title:?} with tags {:?} is shown as {shown_title:?} with tags {:?}",
            block.tags().to_vec(),
            shown_tags.to_vec()
        ));
    }
    if stored_body != row.block.body() {
        differences.push(format!(
            "body {stored_body:?} is shown as {:?}",
            row.block.body()
        ));
    }
    let mut stored_props = RowAttributes::of(block).properties;
    stored_props.retain(|k, _| !omitted.contains(k));
    let shown_props = RowAttributes::of(&row.block).properties;
    if stored_props != shown_props {
        differences.push(format!(
            "drawer {stored_props:?} is shown as {shown_props:?}"
        ));
    }
    (!differences.is_empty()).then(|| format!("the stored {}", differences.join("; the stored ")))
}

/// The keys of `block`'s properties that `row`, its rendered drawer, does not
/// show.
fn unshown_property_keys(block: &Block, row: &DenseBlock) -> Vec<String> {
    let shown: HashSet<&str> = row.drawer.iter().map(|(k, _)| k.as_str()).collect();
    let keys: BTreeSet<String> = block
        .drawer_properties()
        .into_keys()
        .chain(
            block
                .properties
                .keys()
                .filter(|k| !is_hidden_drawer_key(k))
                .cloned(),
        )
        .filter(|k| !shown.contains(k.as_str()))
        .collect();
    keys.into_iter().collect()
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

/// Build a projection-only document block whose `#+TODO:` config covers the
/// task keywords of every document a row lives in (its own ring, or org's
/// defaults when it declares none) and every keyword a row holds, so the dense
/// text reads each row's keyword as its document does. Which keyword a row may
/// be GIVEN is its own document's call, not the header's: see `plan_patch`.
fn synth_doc_block(
    file_id: &EntityUri,
    blocks: &[Block],
    records: &HashMap<String, ProjectedBlock>,
    root_vocabulary: Option<&TaskKeywordVocabulary>,
) -> Block {
    let mut seen: Vec<TaskState> = Vec::new();
    let mut declare = |state: TaskState| {
        if !seen.iter().any(|s| s.keyword == state.keyword) {
            seen.push(state);
        }
    };
    for vocabulary in blocks
        .iter()
        .map(|b| &records[b.id.as_str()].vocabulary)
        .chain(root_vocabulary)
    {
        for keyword in vocabulary.all_keywords() {
            declare(state_in(vocabulary, &keyword));
        }
    }
    for b in blocks {
        if let Some(st) = b.task_state() {
            declare(st);
        }
    }
    let mut doc = Block::new_text(
        file_id.clone(),
        EntityUri::block("dense-projection-anchor"),
        "Projection".to_string(),
    );
    doc.set_page(true);
    doc.set_todo_keywords(Some(seen));
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
        let built = build_projection(
            all,
            &crate::dense_projection::DocVocabularies::Uniform(
                holon_org_format::TaskKeywordVocabulary::default(),
            ),
        )
        .unwrap();
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
        let built = build_projection(
            all,
            &crate::dense_projection::DocVocabularies::Uniform(
                holon_org_format::TaskKeywordVocabulary::default(),
            ),
        )
        .unwrap();
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
        let built = build_projection(
            all,
            &crate::dense_projection::DocVocabularies::Uniform(
                holon_org_format::TaskKeywordVocabulary::default(),
            ),
        )
        .unwrap();
        assert_eq!(built.file_id, EntityUri::block(SYNTHETIC_ROOT));
        assert!(built.records["block:c"].proj_parent.is_none());
        assert_eq!(built.records["block:c"].true_parent, EntityUri::block("b"));
    }
}
