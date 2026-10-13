//! Shared driver of the executor PBTs: it plays the effect runner and the
//! clock, and picks the naive twin named by `HOLON_NET_TWIN`.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::time::Duration;

use holon_api::Value;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::ExecError;
use holon_net::exec::Executor;
use holon_net::exec::NetDef;

/// The twin under test, or `None` for the real executor. `inhibitor-net` is a
/// net variant, not an executor twin.
pub fn twin_name() -> Option<String> {
    std::env::var("HOLON_NET_TWIN").ok()
}

pub fn executor(net: NetDef) -> Executor {
    let exec = Executor::new(net).expect("the test net loads");
    match twin_name().as_deref() {
        None | Some("inhibitor-net") => exec,
        Some(name) => with_twin(exec, name),
    }
}

#[cfg(feature = "naive-twins")]
fn with_twin(exec: Executor, name: &str) -> Executor {
    use holon_net::exec::Twin;
    let twin = match name {
        "hidden-reservation" => Twin::HiddenReservation,
        "mark-by-version" => Twin::MarkByVersion,
        "no-mark" => Twin::NoMark,
        "no-cas" => Twin::NoCas,
        "no-count-bound" => Twin::NoCountBound,
        "unknown-is-failure" => Twin::UnknownIsFailure,
        "unknown-settles-failed" => Twin::UnknownSettlesFailed,
        "stale-done-returns-input" => Twin::StaleDoneReturnsInput,
        "drop-late-done" => Twin::DropLateDone,
        "drop-late-failed" => Twin::DropLateFailed,
        "failed-settles-alone" => Twin::FailedSettlesAlone,
        other => panic!("HOLON_NET_TWIN={other} names no twin"),
    };
    exec.with_twin(twin)
}

#[cfg(not(feature = "naive-twins"))]
fn with_twin(_: Executor, name: &str) -> Executor {
    panic!("HOLON_NET_TWIN={name} needs --features naive-twins")
}

pub struct Driver {
    pub exec: Executor,
    pub now: Duration,
    /// Effects returned by `step` and not yet completed, oldest first.
    pub pending: VecDeque<EffectRequest>,
    /// Every effect request ever returned.
    pub requests: Vec<EffectRequest>,
}

impl Driver {
    pub fn new(net: NetDef) -> Self {
        let mut d = Self {
            exec: executor(net),
            now: Duration::ZERO,
            pending: VecDeque::new(),
            requests: Vec::new(),
        };
        d.step().expect("the initial marking quiesces");
        d
    }

    pub fn step(&mut self) -> Result<(), ExecError> {
        let new = self.exec.step(self.now)?;
        self.requests.extend(new.iter().cloned());
        self.pending.extend(new);
        Ok(())
    }

    pub fn tick(&mut self, secs: u64) -> Result<(), ExecError> {
        self.now += Duration::from_secs(secs);
        self.step()
    }

    /// Completes the oldest pending effect with `result(request)`; no-op when
    /// none is pending. A result for an abandoned attempt is dropped.
    pub fn complete_oldest(
        &mut self,
        result: impl FnOnce(&EffectRequest) -> EffectResult,
    ) -> Result<(), ExecError> {
        let Some(req) = self.pending.pop_front() else {
            return Ok(());
        };
        let r = result(&req);
        self.exec.complete(req.attempt, r, self.now)?;
        self.step()
    }

    pub fn requests_of(&self, transition: &str) -> impl Iterator<Item = &EffectRequest> {
        self.requests
            .iter()
            .filter(move |r| &*r.transition == transition)
    }
}

pub fn content(fields: &[(&str, Value)]) -> Content {
    fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

pub fn text(v: &str) -> Value {
    Value::String(v.to_string())
}

pub fn int(content: &Content, field: &str) -> i64 {
    match content.get(field) {
        Some(Value::Integer(i)) => *i,
        other => panic!("field '{field}' is {other:?}, not an integer"),
    }
}

pub fn string(content: &Content, field: &str) -> String {
    match content.get(field) {
        Some(Value::String(s)) => s.clone(),
        other => panic!("field '{field}' is {other:?}, not a string"),
    }
}
