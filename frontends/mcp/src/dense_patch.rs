//! Pure planner for `dense_patch`: given a captured [`Projection`] and the
//! agent-edited dense text (parsed by [`holon_org_format::parse_dense`]), it
//! computes a typed [`PatchPlan`] — the batch of block operations, plus the set
//! of concurrency tokens to verify — WITHOUT touching any engine. The engine
//! applier (in the MCP tool) and the PBT both consume the same plan.
//!
//! Structure is diffed RELATIVE to the projection (Martin ruling 2026-07-23):
//! a block emits a move ONLY when its enclosing rendered block or its order
//! relative to its surviving siblings actually changed in the edited text. A
//! re-rooted / gap-marked block that the agent left in place emits no move —
//! the `{#alias^}` marker is display-only and carries no semantic weight. New
//! blocks (rows with no `{#alias}`) are created at their tree position; blocks
//! omitted from the text are NOT deleted (deletion is explicit via
//! `delete_aliases`).

use std::collections::HashMap;
use std::collections::HashSet;

use anyhow::Result;
use anyhow::bail;
use holon_api::EntityUri;
use holon_api::types::TaskState;
use holon_org_format::Alias;
use holon_org_format::DenseParse;
use holon_org_format::OrgBlockExt;

use crate::dense_projection::BlockVersion;
use crate::dense_projection::Projection;
use crate::dense_projection::SYNTHETIC_ROOT;

/// A reference to a block that may not have a real id yet (a not-yet-created
/// new block). The engine/model applier resolves `New` to a minted id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ref {
    /// The projection render root (`file_id`) — a top-level position.
    Root,
    /// An existing block.
    Existing(EntityUri),
    /// A NEW block, identified by its row index in the parsed patch.
    New(usize),
}

/// One typed operation in a patch plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchOp {
    Create {
        temp: usize,
        parent: Ref,
        after: Option<Ref>,
        title: String,
        task_state: Option<TaskState>,
    },
    UpdateTitle {
        block_id: EntityUri,
        title: String,
    },
    SetState {
        block_id: EntityUri,
        task_state: Option<TaskState>,
    },
    Move {
        block_id: EntityUri,
        parent: Ref,
        after: Option<Ref>,
    },
    Delete {
        block_id: EntityUri,
    },
}

/// A computed patch plan.
#[derive(Clone, Debug, Default)]
pub struct PatchPlan {
    /// Structural ops (Create/Move) in apply pre-order, then content ops
    /// (UpdateTitle/SetState), then Deletes.
    pub ops: Vec<PatchOp>,
    /// Existing blocks whose current version must equal the captured version
    /// before applying (optimistic concurrency).
    pub verify: Vec<(EntityUri, BlockVersion)>,
}

impl PatchPlan {
    /// Count of move ops — used by the "untouched blocks don't move" invariant.
    pub fn move_count(&self) -> usize {
        self.ops
            .iter()
            .filter(|o| matches!(o, PatchOp::Move { .. }))
            .count()
    }
}

/// A conflict: existing blocks that changed since projection.
#[derive(Clone, Debug)]
pub struct Conflict {
    pub blocks: Vec<EntityUri>,
}

/// Parent key for grouping patched siblings.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum ParentKey {
    Root,
    Id(String),
    New(usize),
}

/// Compute the patch plan. Fails loud on unknown alias, dangling parent, an
/// alias both edited and deleted, or a new top-level block against a synthetic
/// (multi-parent) render root.
pub fn plan_patch(
    projection: &Projection,
    parse: &DenseParse,
    delete_aliases: &[Alias],
) -> Result<PatchPlan> {
    // Resolve deletes.
    let mut delete_ids: Vec<EntityUri> = Vec::new();
    let mut deleted_set: HashSet<String> = HashSet::new();
    for a in delete_aliases {
        let id = projection
            .alias_table
            .id_of(a)
            .ok_or_else(|| anyhow::anyhow!("delete: unknown alias {a}"))?;
        delete_ids.push(id.clone());
        deleted_set.insert(id.as_str().to_string());
    }

    // Row identity + parse_id index.
    let mut parse_id_to_row: HashMap<String, usize> = HashMap::new();
    for (i, db) in parse.blocks.iter().enumerate() {
        parse_id_to_row.insert(db.parse_id.as_str().to_string(), i);
    }
    // Identity of each row.
    let mut row_ident: Vec<Ref> = Vec::with_capacity(parse.blocks.len());
    for (i, db) in parse.blocks.iter().enumerate() {
        match &db.alias {
            Some(a) => {
                let id = projection
                    .alias_table
                    .id_of(a)
                    .ok_or_else(|| anyhow::anyhow!("patch references unknown alias {a}"))?;
                if deleted_set.contains(id.as_str()) {
                    bail!("alias {a} is both present in the patch text and in the delete list");
                }
                row_ident.push(Ref::Existing(id.clone()));
            }
            None => row_ident.push(Ref::New(i)),
        }
    }

    // Patched parent of each row + children order per parent (parse order is
    // pre-order, so append preserves sibling order).
    let mut row_parent: Vec<ParentKey> = Vec::with_capacity(parse.blocks.len());
    let mut children: HashMap<ParentKey, Vec<usize>> = HashMap::new();
    for (i, db) in parse.blocks.iter().enumerate() {
        let pkey = match &db.parent_parse_id {
            None => ParentKey::Root,
            Some(pid) => {
                let prow = *parse_id_to_row.get(pid.as_str()).ok_or_else(|| {
                    anyhow::anyhow!("patch row {i} has a dangling parent reference")
                })?;
                match &row_ident[prow] {
                    Ref::Existing(id) => ParentKey::Id(id.as_str().to_string()),
                    Ref::New(idx) => ParentKey::New(*idx),
                    Ref::Root => ParentKey::Root,
                }
            }
        };
        row_parent.push(pkey.clone());
        children.entry(pkey).or_default().push(i);
    }

    // Helper: the `after` ref for a row = the identity of the immediately
    // preceding sibling under the same patched parent, or None if first.
    let after_ref = |row: usize| -> Option<Ref> {
        let pkey = &row_parent[row];
        let sibs = &children[pkey];
        let pos = sibs
            .iter()
            .position(|&r| r == row)
            .expect("row is its own sibling");
        if pos == 0 {
            None
        } else {
            Some(row_ident[sibs[pos - 1]].clone())
        }
    };

    // Helper: projection parent key of an existing block.
    let proj_parent_key = |id: &EntityUri| -> ParentKey {
        let rec = &projection.records[id.as_str()];
        match &rec.proj_parent {
            None => ParentKey::Root,
            Some(p) => ParentKey::Id(p.as_str().to_string()),
        }
    };

    let mut structural: Vec<PatchOp> = Vec::new();
    let mut content: Vec<PatchOp> = Vec::new();
    let mut verify: Vec<(EntityUri, BlockVersion)> = Vec::new();
    let mut verified: HashSet<String> = HashSet::new();
    let mark_verify = |id: &EntityUri,
                       verify: &mut Vec<(EntityUri, BlockVersion)>,
                       verified: &mut HashSet<String>| {
        if verified.insert(id.as_str().to_string()) {
            verify.push((id.clone(), projection.records[id.as_str()].version.clone()));
        }
    };

    // Which existing blocks are reparented (parent changed) — needed so the
    // reorder LIS only ranks blocks that stayed under the same parent.
    let mut reparented: HashSet<String> = HashSet::new();
    for (i, ident) in row_ident.iter().enumerate() {
        if let Ref::Existing(id) = ident {
            if row_parent[i] != proj_parent_key(id) {
                reparented.insert(id.as_str().to_string());
            }
        }
    }

    // Reorder detection: per patched parent, among existing rows that stayed
    // under this parent, keep the longest run whose projection order is
    // preserved (LIS by proj_index); the rest move.
    let mut needs_reorder_move: HashSet<String> = HashSet::new();
    for (pkey, rows) in &children {
        // existing, same-parent (not reparented-in) rows in patched order
        let stable_candidates: Vec<(usize, &EntityUri)> = rows
            .iter()
            .filter_map(|&r| match &row_ident[r] {
                Ref::Existing(id)
                    if !reparented.contains(id.as_str()) && proj_parent_key(id) == *pkey =>
                {
                    Some((r, id))
                }
                _ => None,
            })
            .collect();
        if stable_candidates.len() < 2 {
            continue;
        }
        let ranks: Vec<usize> = stable_candidates
            .iter()
            .map(|(_, id)| projection.records[id.as_str()].proj_index)
            .collect();
        let keep = lis_indices(&ranks);
        for (pos, (_, id)) in stable_candidates.iter().enumerate() {
            if !keep.contains(&pos) {
                needs_reorder_move.insert(id.as_str().to_string());
            }
        }
    }

    // Emit structural ops in patched pre-order.
    for (i, db) in parse.blocks.iter().enumerate() {
        match &row_ident[i] {
            Ref::New(idx) => {
                let parent = parent_ref(&row_parent[i], &projection.file_id)?;
                structural.push(PatchOp::Create {
                    temp: *idx,
                    parent,
                    after: after_ref(i),
                    title: db.block.org_title(),
                    task_state: db.block.task_state(),
                });
            }
            Ref::Existing(id) => {
                let moved =
                    reparented.contains(id.as_str()) || needs_reorder_move.contains(id.as_str());
                if moved {
                    let parent = parent_ref(&row_parent[i], &projection.file_id)?;
                    structural.push(PatchOp::Move {
                        block_id: id.clone(),
                        parent,
                        after: after_ref(i),
                    });
                    mark_verify(id, &mut verify, &mut verified);
                }
                // content / state diff
                let rec = &projection.records[id.as_str()];
                let new_title = db.block.org_title();
                if new_title != rec.title {
                    content.push(PatchOp::UpdateTitle {
                        block_id: id.clone(),
                        title: new_title,
                    });
                    mark_verify(id, &mut verify, &mut verified);
                }
                let new_state = db.block.task_state();
                if new_state != rec.task_state {
                    content.push(PatchOp::SetState {
                        block_id: id.clone(),
                        task_state: new_state,
                    });
                    mark_verify(id, &mut verify, &mut verified);
                }
            }
            Ref::Root => unreachable!("a row is never Root"),
        }
    }

    let mut ops = structural;
    ops.extend(content);
    for id in &delete_ids {
        ops.push(PatchOp::Delete {
            block_id: id.clone(),
        });
        mark_verify(id, &mut verify, &mut verified);
    }

    Ok(PatchPlan { ops, verify })
}

fn parent_ref(pkey: &ParentKey, file_id: &EntityUri) -> Result<Ref> {
    match pkey {
        ParentKey::Root => {
            if file_id.id() == SYNTHETIC_ROOT {
                bail!(
                    "cannot place a top-level block: this projection spans multiple parents \
                     (synthetic render root). Nest the block under an existing block instead."
                );
            }
            Ok(Ref::Root)
        }
        ParentKey::Id(id) => Ok(Ref::Existing(EntityUri::parse(id)?)),
        ParentKey::New(idx) => Ok(Ref::New(*idx)),
    }
}

/// Longest strictly-increasing subsequence — returns the set of POSITIONS in
/// `seq` that belong to one LIS (patience sorting with parent links).
fn lis_indices(seq: &[usize]) -> HashSet<usize> {
    let n = seq.len();
    if n == 0 {
        return HashSet::new();
    }
    let mut tails: Vec<usize> = Vec::new(); // positions of pile tops
    let mut tails_val: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        // first tail with value >= seq[i]  (strictly increasing → lower_bound)
        let pos = tails_val.partition_point(|&v| v < seq[i]);
        if pos == tails.len() {
            tails.push(i);
            tails_val.push(seq[i]);
        } else {
            tails[pos] = i;
            tails_val[pos] = seq[i];
        }
        prev[i] = if pos > 0 { Some(tails[pos - 1]) } else { None };
    }
    let mut out = HashSet::new();
    let mut k = tails.last().copied();
    while let Some(i) = k {
        out.insert(i);
        k = prev[i];
    }
    out
}
