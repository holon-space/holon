//! Holon's views, as plans over the `blocks` and `focus_roots` relations, and
//! the parse of a block and a focus root into their rows.

use std::rc::Rc;

use holon_api::EntityUri;
use holon_api::block::SnapshotBlock;

use crate::error::EngineError;
use crate::intern::Interner;
use crate::payload::Payload;
use crate::plan::Catalog;
use crate::plan::Col;
use crate::plan::ColType;
use crate::plan::Expr;
use crate::plan::Plan;
use crate::plan::RelationId;
use crate::plan::Schema;
use crate::row::Datum;
use crate::row::Row;

/// `blocks(id: Id, parent: Id, sort: Text, is_page: Bool, payload: Payload)`
pub const BLOCKS: RelationId = RelationId(0);
pub const ID: Col = Col(0);
pub const PARENT: Col = Col(1);
pub const SORT: Col = Col(2);
pub const IS_PAGE: Col = Col(3);
pub const PAYLOAD: Col = Col(4);

pub(crate) fn blocks_schema() -> Schema {
    Schema(vec![
        ColType::Id,
        ColType::Id,
        ColType::Text,
        ColType::Bool,
        ColType::Payload,
    ])
}

/// `focus_roots(history: Int, region: Text, root: Id)`: the open focus of
/// each navigation history entry, keyed by `history`.
pub const FOCUS_ROOTS: RelationId = RelationId(1);
pub const HISTORY: Col = Col(0);
pub const REGION: Col = Col(1);
pub const ROOT: Col = Col(2);

pub(crate) fn focus_roots_schema() -> Schema {
    Schema(vec![ColType::Int, ColType::Text, ColType::Id])
}

pub fn catalog() -> Catalog {
    Catalog {
        relations: vec![blocks_schema(), focus_roots_schema()],
    }
}

/// The views of one dataflow. They share one scan of `blocks`, so the
/// dataflow arranges `blocks` once per key.
pub struct Views {
    /// `(parent, sort, id)`, read by `parent`; a reader orders the children
    /// by `(sort, id)`.
    pub children: Rc<Plan>,
    /// `(node, page)`: the nearest page at or above `node`, read by `node`. A
    /// block with no page above it has no row.
    pub owning_page: Rc<Plan>,
    /// `(id, payload)`, read by `id`.
    pub row: Rc<Plan>,
    /// `(region, root, history)`, read by `region`. The root need not be a
    /// block of the same version.
    pub focus_roots: Rc<Plan>,
}

pub fn views() -> Views {
    let col = |c: Col| Expr::Col(c);
    let blocks = Plan::scan(BLOCKS);
    let pages = blocks.filter(col(IS_PAGE)).project(vec![col(ID), col(ID)]);
    let non_pages = blocks.filter(Expr::Not(Box::new(col(IS_PAGE))));
    // `Recur` is `(node, page)`; the joined row is `(node, page)` ++ the child.
    let child_id = Col(2 + ID.0);
    let inherit = Plan::recur()
        .join(&non_pages, vec![(Col(0), PARENT)])
        .project(vec![col(child_id), col(Col(1))]);
    Views {
        children: blocks.project(vec![col(PARENT), col(SORT), col(ID)]),
        owning_page: pages.iterate(&inherit),
        row: blocks.project(vec![col(ID), col(PAYLOAD)]),
        focus_roots: Plan::scan(FOCUS_ROOTS).project(vec![col(REGION), col(ROOT), col(HISTORY)]),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Children,
    OwningPage,
    Row,
    FocusRoots,
}

impl View {
    pub const ALL: [View; 4] = [
        View::Children,
        View::OwningPage,
        View::Row,
        View::FocusRoots,
    ];
}

impl Views {
    pub fn plan(&self, view: View) -> &Rc<Plan> {
        match view {
            View::Children => &self.children,
            View::OwningPage => &self.owning_page,
            View::Row => &self.row,
            View::FocusRoots => &self.focus_roots,
        }
    }

    /// In the order of [`View::ALL`].
    pub fn plans(&self) -> [Rc<Plan>; 4] {
        View::ALL.map(|view| self.plan(view).clone())
    }
}

/// The `blocks` row of `block`; ids interned in `interner`.
pub fn block_row<R: Row>(interner: &mut Interner, block: &SnapshotBlock) -> Result<R, EngineError> {
    let payload = Payload::encode(block)?;
    Ok(R::build(
        &R::layout(&blocks_schema()),
        [
            Datum::Id(interner.intern(&block.block.id)),
            Datum::Id(interner.intern(&block.block.parent_id)),
            Datum::Text(block.sort_key.as_str().into()),
            Datum::Bool(block.block.is_page()),
            Datum::Payload(payload),
        ],
    ))
}

/// An open `navigation_history` entry that focuses a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusRoot {
    pub history: i64,
    pub region: String,
    pub root: EntityUri,
}

/// The `focus_roots` row of `focus`; ids interned in `interner`.
pub fn focus_root_row<R: Row>(interner: &mut Interner, focus: &FocusRoot) -> R {
    R::build(
        &R::layout(&focus_roots_schema()),
        [
            Datum::Int(focus.history),
            Datum::Text(focus.region.as_str().into()),
            Datum::Id(interner.intern(&focus.root)),
        ],
    )
}
