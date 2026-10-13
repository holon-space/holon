//! Stubs of the user-facing-transitions contract (#167) on the executor
//! side. The shared types live in [`holon_api::user_transition`]; each stub
//! returns [`NotImplemented`] naming the lane that fills it.

use std::sync::Arc;
use std::time::Duration;

use holon_api::user_transition::AnswerCall;
use holon_api::user_transition::AnswerId;
use holon_api::user_transition::AnswerSettlement;
use holon_api::user_transition::AskHash;
use holon_api::user_transition::AskRow;
use holon_api::user_transition::Lane;
use holon_api::user_transition::NotImplemented;
use holon_api::user_transition::TransitionDescriptor;
use holon_api::user_transition::UftError;

use crate::exec::Cause;
use crate::exec::Content;
use crate::exec::Executor;
use crate::exec::NetDef;
use crate::exec::Output;
use crate::exec::Reader;
use crate::exec::Receiver;
use crate::exec::Subject;
use crate::exec::Trace;

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

/// The ask identity over the transition, the subject and the read-trace
/// content at derivation. Lane X fills it.
pub fn ask_hash(_: &str, _: &Subject, _: &Trace) -> Result<AskHash, NotImplemented> {
    Err(executor("uft::ask_hash"))
}

impl Executor {
    /// The rows of the derived place `Asks`. Lane X fills it.
    pub fn asks(&self) -> Result<Vec<AskRow>, NotImplemented> {
        Err(executor("Executor::asks"))
    }

    /// Fires `Answer`: puts the `Answered` token. Lane X fills it.
    pub fn answer(&mut self, _: AnswerCall, _: Duration) -> Result<AnswerId, UftError> {
        Err(executor("Executor::answer").into())
    }

    /// The settled chain of one answer. Lane X fills it.
    pub fn answer_settlement(&self, _: &AnswerId) -> Result<AnswerSettlement, UftError> {
        Err(executor("Executor::answer_settlement").into())
    }
}

fn executor(what: &'static str) -> NotImplemented {
    NotImplemented {
        lane: Lane::Executor,
        what,
    }
}
