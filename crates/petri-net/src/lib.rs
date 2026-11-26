//! @c4 component
//! @c4 layer Core
//! Pattern: Interpreter
//!
//! A coloured Petri-net kernel. Tokens live in rows addressed by (place, key);
//! each place has a discipline. Transitions declare their arcs, and one bind
//! function per transition is guard, binding and fire in one. The net fires
//! with level semantics: one enabled (transition, key) pair at a time, then
//! check again from scratch.
//!
//! Every change of the marking is a firing of a declared transition. This is a
//! single-replica law: a replicated store that merges rows needs its own entry
//! and a settle after it.

use std::fmt::Debug;

mod mem;
mod store;

pub use mem::{MemRowStamp, MemStamp, MemStore};
pub use store::{Applied, RowView, Store};

#[cfg(feature = "testkit")]
pub mod testkit;

/// The types a net is generic over, bundled so signatures stay short.
pub trait Kinds: 'static {
    type Place: Ord + Clone + Debug;
    type Key: Ord + Clone + Debug;
    type Token: Clone + Debug;
    /// What a caller supplies to an external firing.
    type Binding: Debug;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Discipline {
    /// At most one token; a produce into a full row is refused.
    Single,
    /// At most one token; a produce replaces it.
    Register,
    /// A queue; consume takes the head, produce appends.
    Fifo,
    /// A count of units up to `size`; a produce beyond it is refused.
    Counted { size: u32 },
}

impl Discipline {
    pub fn is_counted(self) -> bool {
        matches!(self, Discipline::Counted { .. })
    }
}

/// One output of a firing.
#[derive(Clone, Debug)]
pub enum Out<K: Kinds> {
    /// A new token; it gets a fresh stamp.
    New(K::Token),
    /// The i-th consumed token, unchanged, with its stamp. At most once per
    /// consumed token; a consumed token that no output moves is gone.
    Moved(usize),
    /// One unit into a Counted row.
    Unit,
}
