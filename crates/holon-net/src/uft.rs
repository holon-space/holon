//! Stubs of the user-facing-transitions contract (#167) on the executor
//! side. The shared types live in [`holon_api::user_transition`]; each stub
//! returns [`NotImplemented`] naming the lane that fills it.

use std::sync::Arc;

use holon_api::user_transition::Lane;
use holon_api::user_transition::NotImplemented;
use holon_api::user_transition::TransitionDescriptor;

use crate::exec::Cause;
use crate::exec::Content;
use crate::exec::NetDef;
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

/// Lowers one high-level transition into `net`: the work transition and, for
/// a `manual` one, its `Asks`, `Answer` and `ExpireAnswer`. Lane L fills it.
pub fn lower(
    _: NetDef,
    _: &TransitionDescriptor,
    _: Option<NativeParts>,
) -> Result<NetDef, NotImplemented> {
    Err(NotImplemented {
        lane: Lane::Lowering,
        what: "uft::lower",
    })
}
