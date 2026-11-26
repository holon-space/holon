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

mod bound;
mod mem;
mod net;
mod store;
mod view;

pub use bound::Bound;
pub use bound::Evaluation;
pub use bound::Fired;
pub use bound::FiredOf;
pub use bound::Footprint;
pub use bound::Settled;
pub use bound::StoreMismatch;
pub use bound::Unsettled;
pub use bound::WhyNot;
pub use mem::MemRowStamp;
pub use mem::MemStamp;
pub use mem::MemStore;
pub use net::ArcSpec;
pub use net::Bind;
pub use net::BuildError;
pub use net::Fire;
pub use net::KeySel;
pub use net::Net;
pub use net::NetBuilder;
pub use net::Transition;
pub use net::TransitionId;
pub use net::Trigger;
pub use store::Applied;
pub use store::RowView;
pub use store::Store;
pub use view::View;

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

/// Where a moved token lands in its row. Stamps have no order, so a token
/// that must go back before newer ones says so here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum At {
    /// Before every token of the row; legal only on Fifo places.
    Front,
    Back,
}

/// One output of a firing.
#[derive(Clone, Debug)]
pub enum Out<K: Kinds> {
    /// A new token; it gets a fresh stamp.
    New(K::Token),
    /// The `from`-th consumed token, unchanged, with its stamp. At most once
    /// per consumed token; a consumed token that no output moves is gone.
    Moved { from: usize, at: At },
    /// One unit into a Counted row.
    Unit,
}
