//! The one trait every operator reads and builds rows through, so the storage
//! of a relation can change without a change to any operator.

use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;

use crate::payload::Payload;
use crate::plan::Col;
use crate::plan::ColType;
use crate::plan::Schema;

/// An interned block id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Id(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Datum {
    Bool(bool),
    Int(i64),
    Id(Id),
    Text(Arc<str>),
    Payload(Payload),
}

impl Datum {
    pub fn col_type(&self) -> ColType {
        match self {
            Datum::Bool(_) => ColType::Bool,
            Datum::Int(_) => ColType::Int,
            Datum::Id(_) => ColType::Id,
            Datum::Text(_) => ColType::Text,
            Datum::Payload(_) => ColType::Payload,
        }
    }
}

/// A row of one relation. `Ord` is some total order consistent with `Eq`, not
/// the order of the values: an operator that needs value order decodes the
/// columns through [`Row::get`].
pub trait Row: differential_dataflow::ExchangeData + std::hash::Hash + std::fmt::Debug {
    /// Where each column of one relation sits in the row; made once per plan
    /// node from its schema.
    type Layout: Clone + 'static;

    fn layout(schema: &Schema) -> Self::Layout;
    fn get(&self, layout: &Self::Layout, col: Col) -> Datum;
    fn build(layout: &Self::Layout, cols: impl IntoIterator<Item = Datum>) -> Self;
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DynRow(Box<[Datum]>);

impl Row for DynRow {
    type Layout = ();

    fn layout(_: &Schema) -> Self::Layout {}

    fn get(&self, _: &(), col: Col) -> Datum {
        self.0[col.index()].clone()
    }

    fn build(_: &(), cols: impl IntoIterator<Item = Datum>) -> Self {
        DynRow(cols.into_iter().collect())
    }
}
