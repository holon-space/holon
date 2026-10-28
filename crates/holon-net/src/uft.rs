//! The executor side of the user-facing-transitions contract (#167): the
//! Rust half of a transition ([`NativeParts`]) and its lowering into a net
//! ([`lower`]). The shared types live in [`holon_transition_contract`].

use std::sync::Arc;

mod lower;

pub use lower::Generated;
pub use lower::LowerError;
pub use lower::LowerTwin;
pub use lower::lower;
#[cfg(feature = "naive-twins")]
pub use lower::lower_twin;

use crate::exec::Cause;
use crate::exec::Content;
use crate::exec::Output;
use crate::exec::Reader;
use crate::exec::Receiver;
use crate::exec::Subject;

/// A binding as a closure (ruling 1.a), so a runtime IR can lower too. `None`
/// is a false guard; `Some` is the bound situation.
pub type BindFn = Arc<dyn Fn(&mut Reader<'_>, &Subject) -> Option<Content> + Send + Sync>;

/// A pure body as a closure (ruling 1.a).
pub type PureBodyFn = Arc<dyn Fn(&Subject, &Content) -> Result<Vec<Output>, Cause> + Send + Sync>;

/// A preview sees the bound situation ONLY (ruling 8): no reader, no clock,
/// so the ask hash covers everything the user can see.
pub type PreviewFn = Arc<dyn Fn(&Subject, &Content) -> String + Send + Sync>;

#[derive(Clone)]
pub enum NativeBody {
    Pure(PureBodyFn),
    Effectful(Receiver),
}

/// The Rust half of a transition: the binding, body and preview that lane M's
/// generated adapter supplies, or that lane Y builds from #166 expressions.
#[derive(Clone)]
pub struct NativeParts {
    pub bind: BindFn,
    pub body: NativeBody,
    pub preview: Option<PreviewFn>,
}
