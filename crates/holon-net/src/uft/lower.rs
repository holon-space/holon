//! The lowering of the HL-IR (#167): one [`TransitionDescriptor`] becomes
//! low-level [`TransitionDef`]s and places. Declarations never hand-wire the
//! asking; this module is the one place that knows how `manual` lowers.
//!
//! An `auto` descriptor lowers 1:1 into the work transition `<T>`. A `manual`
//! one (here `AcceptRemoval`: takes `Refused`, reads `Base` and `OnDisk`)
//! lowers to:
//!
//! | Generated | Kind | Arcs | Fires when |
//! |---|---|---|---|
//! | `Asks.<T>` | derived place | — | (T, s) is idle (`Answered.<T>`, `<T>.Running`, `<T>.RetryPending`, `<T>.OutcomeUnknown`, `WorkFailed.<T>` empty), the declared arcs hold and the binding holds; row `hash` (ask hash over the trace of the declared arcs and the binding) and `preview` |
//! | `Answer.<T>` | manual | inhibits exactly the idle set of `Asks.<T>` | an `Asks.<T>` row exists; the guard never sees the value; puts `Answered.<T>{answer, seen, value}` |
//! | `<T>` | auto | takes `Answered.<T>` + the declared arcs; failure port `WorkFailed.<T>` | the situation's ask hash equals `Answered.seen` |
//! | `ExpireAnswer.<T>` | auto | takes `Answered.<T>`; inhibits `WorkFailed.<T>` | the ask is gone or its hash differs; puts `Expired.<T>` keyed by the answer id |
//! | `ReleaseFailed.<T>` | auto | takes `Answered.<T>` + `WorkFailed.<T>` | always, so a re-answer after a failure is not "already answered" |
//! | `ClearFailed.<T>` | auto | takes `WorkFailed.<T>`; inhibits `Answered.<T>` | always: a dropped outcome-unknown consumed its `Answered`, and its `WorkFailed` must not pair with a later answer |
//!
//! The work and the ask derive their hash from one function, so an `Answered`
//! whose `seen` equals the current situation's hash enables the work.
//! `Put` arcs have no executor arc: the body's outputs are the puts.

use std::sync::Arc;

use holon_api::Value;
use holon_api::user_transition::ANSWER_ID_KEY;
use holon_api::user_transition::ArcKind;
use holon_api::user_transition::BodyDecl;
use holon_api::user_transition::ExpireReason;
use holon_api::user_transition::TimerKind;
use holon_api::user_transition::TransitionDescriptor;

use super::BindFn;
use super::NativeBody;
use super::NativeParts;
use crate::exec::AskMeta;
use crate::exec::Body;
use crate::exec::Class;
use crate::exec::Content;
use crate::exec::HASH_FIELD;
use crate::exec::NetDef;
use crate::exec::Output;
use crate::exec::Place;
use crate::exec::REASON_FIELD;
use crate::exec::Reader;
use crate::exec::SEEN_FIELD;
use crate::exec::Subject;
use crate::exec::TransitionDef;
use crate::exec::Trigger;
use crate::exec::answer_transition;
use crate::exec::answered_place;
use crate::exec::ask_hash;
use crate::exec::asks_place;
use crate::exec::expire_reason_text;
use crate::exec::expired_place;
use crate::exec::hash_text;

/// A descriptor the lowering cannot express. Names the transition.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("cannot lower transition '{transition}': {reason}")]
pub struct LowerError {
    pub transition: String,
    pub reason: String,
}

/// The places and transitions the lowering generates for a manual `<T>`,
/// named `<Role>.<T>`, so a viewer maps each back to its HL transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generated {
    Asks,
    Answered,
    WorkFailed,
    Expired,
    Answer,
    ExpireAnswer,
    ReleaseFailed,
    ClearFailed,
}

impl Generated {
    pub const ALL: [Self; 8] = [
        Self::Asks,
        Self::Answered,
        Self::WorkFailed,
        Self::Expired,
        Self::Answer,
        Self::ExpireAnswer,
        Self::ReleaseFailed,
        Self::ClearFailed,
    ];

    fn role(self) -> &'static str {
        match self {
            Self::Asks => "Asks",
            Self::Answered => "Answered",
            Self::WorkFailed => "WorkFailed",
            Self::Expired => "Expired",
            Self::Answer => "Answer",
            Self::ExpireAnswer => "ExpireAnswer",
            Self::ReleaseFailed => "ReleaseFailed",
            Self::ClearFailed => "ClearFailed",
        }
    }

    pub fn name(self, transition: &str) -> String {
        format!("{}.{transition}", self.role())
    }

    /// The role and the HL transition of a generated name.
    pub fn parse(name: &str) -> Option<(Self, &str)> {
        let (role, transition) = name.split_once('.')?;
        Self::ALL
            .into_iter()
            .find(|g| g.role() == role)
            .map(|g| (g, transition))
    }
}

/// A variant of the lowering that drops exactly one rule, so a property test
/// of that rule shows red for its own reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LowerTwin {
    /// The work guard ignores the ask hash.
    NoAskHash,
    /// `Answer` has no inhibitor.
    NoAnswerInhibitor,
    /// `Answer` is not gated on the work in flight — neither by its inhibitor
    /// nor by the ask it needs: a second answer lands while the first
    /// answer's work runs.
    DoubleAnswer,
    /// No `ExpireAnswer`.
    NoExpire,
    /// `Asks` offers the ask while the work is in flight.
    AskWhileInFlight,
    /// `Asks` offers the ask while a retry backoff is pending.
    AskWhileRetryPending,
    /// `Asks` offers the ask while an occurrence is parked with an unknown
    /// outcome.
    AskWhileOutcomeUnknown,
}

/// Lowers one high-level transition into `net`: the work transition and, for
/// a `manual` one, its `Asks`, `Answer`, `ExpireAnswer` and `ReleaseFailed`.
pub fn lower(
    net: NetDef,
    d: &TransitionDescriptor,
    parts: Option<NativeParts>,
) -> Result<NetDef, LowerError> {
    lower_as(net, d, parts, None)
}

#[cfg(feature = "naive-twins")]
pub fn lower_twin(
    net: NetDef,
    d: &TransitionDescriptor,
    parts: Option<NativeParts>,
    twin: LowerTwin,
) -> Result<NetDef, LowerError> {
    lower_as(net, d, parts, Some(twin))
}

fn lower_as(
    net: NetDef,
    d: &TransitionDescriptor,
    parts: Option<NativeParts>,
    twin: Option<LowerTwin>,
) -> Result<NetDef, LowerError> {
    let t = d.name.0.as_str();
    let refuse = |reason: &str| LowerError {
        transition: t.to_owned(),
        reason: reason.to_owned(),
    };
    let parts = parts.ok_or_else(|| refuse("no native parts (binding and body)"))?;
    if d.guard.is_some() {
        return Err(refuse(
            "a descriptor guard is an #166 expression, which does not lower yet",
        ));
    }
    let body = match (&d.body, parts.body) {
        (
            BodyDecl::Native {
                effectful: false, ..
            }
            | BodyDecl::Expr(_),
            NativeBody::Pure(b),
        ) => Body::Pure(b),
        (
            BodyDecl::Native {
                effectful: true, ..
            }
            | BodyDecl::Expr(_),
            NativeBody::Effectful(r),
        ) => Body::Effectful(r),
        (BodyDecl::Tool { .. }, _) => {
            return Err(refuse("a sidecar tool body does not lower yet"));
        }
        (BodyDecl::Native { effectful, .. }, _) => {
            return Err(refuse(&format!(
                "the body is declared effectful={effectful}, the native parts disagree"
            )));
        }
    };
    let Some(ask) = &d.ask else {
        if parts.preview.is_some() {
            return Err(refuse("an auto transition has a preview"));
        }
        if d.undoes.is_some() {
            return Err(refuse("an auto transition declares undoes (ADR 0032 §7)"));
        }
        let work = arcs(
            TransitionDef::new(t, Trigger::Auto, parts.bind, body),
            d,
            &refuse,
        )?;
        return Ok(net.transition(work));
    };
    if ask.persist {
        return Err(refuse("persistent asks lower later (UFT-scope.a)"));
    }
    if ask.preview.is_some() != parts.preview.is_some() {
        return Err(refuse(
            "the ask declares a preview but the native parts disagree",
        ));
    }
    if d.arcs.iter().any(|a| a.kind == ArcKind::OnFailure) {
        return Err(refuse(&format!(
            "a manual transition's failure port is generated ({})",
            Generated::WorkFailed.name(t)
        )));
    }

    let (answered, failed, expired, asks) = (
        answered_place(t),
        Generated::WorkFailed.name(t),
        expired_place(t),
        asks_place(t),
    );
    let running = Place::running(t);
    let situation = Situation {
        transition: Arc::from(t),
        needs: d
            .arcs
            .iter()
            .filter(|a| matches!(a.kind, ArcKind::Take | ArcKind::Read))
            .map(|a| a.place.0.clone())
            .collect(),
        inhibits: d
            .arcs
            .iter()
            .filter(|a| a.kind == ArcKind::Inhibit)
            .map(|a| a.place.0.clone())
            .collect(),
        bind: parts.bind,
    };
    let situation = Arc::new(situation);

    // (T, s) is idle iff none of these holds a token: an ask exists only then,
    // and `Answer` is inhibited by exactly these, so an offered ask always
    // has an enabled `Answer` (D151.a).
    let busy = [
        // No twin: no `Answered` stands at quiescence (P-quiescent).
        (answered.clone(), None),
        (running.to_string(), Some(LowerTwin::AskWhileInFlight)),
        (
            Place::retry_pending(t).to_string(),
            Some(LowerTwin::AskWhileRetryPending),
        ),
        (
            Place::outcome_unknown(t).to_string(),
            Some(LowerTwin::AskWhileOutcomeUnknown),
        ),
        // No twin: no `WorkFailed` stands at quiescence (P-quiescent).
        (failed.clone(), None),
    ];
    let derive: BindFn = {
        let (sit, preview) = (situation.clone(), parts.preview.clone());
        let busy: Vec<String> = busy
            .iter()
            .filter(|(_, skip)| skip.is_none() || twin != *skip)
            .map(|(p, _)| p.clone())
            .collect();
        Arc::new(move |r: &mut Reader<'_>, s: &Subject| {
            if busy.iter().any(|p| r.present(p, s)) {
                return None;
            }
            let (content, hash) = sit.eval(r, s)?;
            let mut row = Content::from([(HASH_FIELD.to_owned(), Value::String(hash))]);
            if let Some(p) = &preview {
                row.insert("preview".to_owned(), Value::String(p(s, &content)));
            }
            Some(row)
        })
    };

    let answer_bind: BindFn = {
        let asks = asks.to_string();
        let in_flight = (twin == Some(LowerTwin::DoubleAnswer)).then(|| running.to_string());
        Arc::new(move |r: &mut Reader<'_>, s: &Subject| {
            let gate = r.present(&asks, s) || in_flight.as_ref().is_some_and(|p| r.present(p, s));
            gate.then(Content::new)
        })
    };
    let answer_body = {
        let answered = answered.to_string();
        Arc::new(move |s: &Subject, input: &Content| {
            Ok(vec![Output::new(&answered, s, input.clone())])
        })
    };
    let mut answer = TransitionDef::new(
        &answer_transition(t),
        Trigger::Manual,
        answer_bind,
        Body::Pure(answer_body),
    );
    if twin != Some(LowerTwin::NoAnswerInhibitor) {
        for (p, _) in &busy {
            if twin == Some(LowerTwin::DoubleAnswer) && p.as_str() == running.as_str() {
                continue;
            }
            answer = answer.inhibits(p);
        }
    }

    let work_bind: BindFn = {
        let (sit, answered) = (situation.clone(), answered.to_string());
        let check_hash = twin != Some(LowerTwin::NoAskHash);
        Arc::new(move |r: &mut Reader<'_>, s: &Subject| {
            let (mut content, hash) = sit.eval(r, s)?;
            let seen = r.text(&answered, s, SEEN_FIELD)?;
            if check_hash && hash != seen {
                return None;
            }
            let answer = r.text(&answered, s, ANSWER_ID_KEY)?;
            for (field, value) in [(ANSWER_ID_KEY, answer), (SEEN_FIELD, seen)] {
                let clash = content.insert(field.to_owned(), Value::String(value));
                assert!(
                    clash.is_none(),
                    "the binding of '{}' returns the reserved field '{field}'",
                    sit.transition
                );
            }
            Some(content)
        })
    };
    let mut work = arcs(
        TransitionDef::new(t, Trigger::Auto, work_bind, body).consumes(&answered),
        d,
        &refuse,
    )?
    .on_failure(&failed);
    // An undo never races the work it undoes (ADR 0032 §7): the answer waits
    // in `Answered` until that work is neither running nor unsettled.
    if let Some(target) = &d.undoes {
        let u = target.0.as_str();
        for busy in [
            Place::running(u),
            Place::retry_pending(u),
            Place::outcome_unknown(u),
        ] {
            work = work.inhibits(&busy.to_string());
        }
    }

    let expire_bind: BindFn = {
        let (sit, answered, preview) = (situation.clone(), answered.to_string(), parts.preview);
        Arc::new(move |r: &mut Reader<'_>, s: &Subject| {
            let seen = r.text(&answered, s, SEEN_FIELD)?;
            let answer = r.field(&answered, s, ANSWER_ID_KEY)?;
            let (reason, why, preview) = match sit.eval(r, s) {
                None => (
                    ExpireReason::AskGone,
                    "the question no longer applies",
                    String::new(),
                ),
                Some((content, hash)) if hash != seen => (
                    ExpireReason::SituationChanged,
                    "the situation changed",
                    preview
                        .as_ref()
                        .map_or_else(String::new, |p| p(s, &content)),
                ),
                Some(_) => return None,
            };
            Some(Content::from([
                (ANSWER_ID_KEY.to_owned(), answer),
                (
                    REASON_FIELD.to_owned(),
                    Value::String(expire_reason_text(reason).to_owned()),
                ),
                ("why".to_owned(), Value::String(why.to_owned())),
                ("preview".to_owned(), Value::String(preview)),
            ]))
        })
    };
    let expire_body = {
        let expired = expired.to_string();
        Arc::new(move |s: &Subject, input: &Content| {
            let Some(Value::String(answer)) = input.get(ANSWER_ID_KEY) else {
                panic!("an Answered token without a text answer id: {input:?}");
            };
            let mut out = input.clone();
            out.insert("subject".to_owned(), Value::String(s.to_string()));
            Ok(vec![Output::new(&expired, &Subject::new(answer), out)])
        })
    };
    let release: BindFn = Arc::new(|_: &mut Reader<'_>, _: &Subject| Some(Content::new()));

    let mut net = net
        .place(&answered, Class::Process)
        .place(&failed, Class::Process)
        .place(&expired, Class::Process)
        .derived(&asks, derive)
        .ask(
            t,
            AskMeta {
                input: ask.input.clone(),
                irreversible: d.irreversible,
                has_preview: ask.preview.is_some(),
            },
        )
        .transition(answer)
        .transition(work);
    // ExpireAnswer goes before ReleaseFailed, so only its WorkFailed inhibitor,
    // not the firing order, keeps a failed answer from expiring.
    if twin != Some(LowerTwin::NoExpire) {
        net = net.transition(
            TransitionDef::new(
                &Generated::ExpireAnswer.name(t),
                Trigger::Auto,
                expire_bind,
                Body::Pure(expire_body),
            )
            .consumes(&answered)
            .inhibits(&failed),
        );
    }
    let net = net
        .transition(
            TransitionDef::new(
                &Generated::ReleaseFailed.name(t),
                Trigger::Auto,
                release.clone(),
                Body::Pure(Arc::new(|_: &Subject, _: &Content| Ok(vec![]))),
            )
            .consumes(&answered)
            .consumes(&failed),
        )
        .transition(
            TransitionDef::new(
                &Generated::ClearFailed.name(t),
                Trigger::Auto,
                release,
                Body::Pure(Arc::new(|_: &Subject, _: &Content| Ok(vec![]))),
            )
            .consumes(&failed)
            .inhibits(&answered),
        );
    Ok(net)
}

/// The work's binding without the answer, plus the arc checks the executor
/// makes before a binding runs. `Asks`, the work and `ExpireAnswer` all
/// evaluate it, so they agree on the ask hash.
struct Situation {
    transition: Arc<str>,
    needs: Vec<String>,
    inhibits: Vec<String>,
    bind: BindFn,
}

impl Situation {
    /// The bound situation and the hex ask hash over the trace of this check.
    fn eval(&self, r: &mut Reader<'_>, s: &Subject) -> Option<(Content, String)> {
        let (content, trace) = r.traced(|r| {
            if !self.needs.iter().all(|p| r.present(p, s))
                || self.inhibits.iter().any(|p| r.present(p, s))
            {
                return None;
            }
            (self.bind)(r, s)
        });
        let hash = hash_text(&ask_hash(&self.transition, s, &trace));
        Some((content?, hash))
    }
}

/// The declared arcs, ports and timers of `d` on its work transition.
fn arcs(
    mut work: TransitionDef,
    d: &TransitionDescriptor,
    refuse: &dyn Fn(&str) -> LowerError,
) -> Result<TransitionDef, LowerError> {
    for a in &d.arcs {
        let p = a.place.0.as_str();
        work = match a.kind {
            ArcKind::Take => work.consumes(p),
            ArcKind::Read => work.reads(p),
            ArcKind::Inhibit => work.inhibits(p),
            ArcKind::Put => work,
            ArcKind::OnFailure if work.on_failure.is_none() => work.on_failure(p),
            ArcKind::OnTimeout if work.on_timeout.is_none() => work.on_timeout(p),
            ArcKind::OnFailure | ArcKind::OnTimeout => {
                return Err(refuse(&format!("a second {:?} port ({p})", a.kind)));
            }
        };
    }
    for timer in &d.timers {
        work = match timer.kind {
            TimerKind::Retry { max_attempts } if work.retry.is_none() => {
                work.retry(timer.after, max_attempts)
            }
            TimerKind::Run if work.run_timer.is_none() => work.run_timer(timer.after),
            TimerKind::Abandon if work.abandon.is_none() => work.abandon(timer.after),
            TimerKind::Delay => {
                return Err(refuse("a Delay timer (default answer, E6) lowers later"));
            }
            _ => return Err(refuse(&format!("a second {:?} timer", timer.kind))),
        };
    }
    Ok(work)
}
