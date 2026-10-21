//! The loader: one net file becomes the runtime net the sans-IO executor
//! runs. The file owns the shape — places with their capacity, the consume,
//! read and inhibitor arcs, the failure and timeout ports, the timers, the
//! effect a firing runs — and the caller hands in the Rust the format cannot
//! hold: the guard and binding of every transition, and the body of each
//! transition that runs no effect.
//!
//! `produces` is the file's declaration of a transition's outputs. The
//! executor takes the outputs from the firing itself, so the loader builds
//! nothing from it; the format checks that each one names a declared place.
//!
//! What the format can express and this executor cannot, the loader refuses
//! rather than dropping: a `pool` of interchangeable tokens has no form in an
//! executor that holds one token per subject.

use std::fs;
use std::path::Path;

use crate::exec::Body;
use crate::exec::Class;
use crate::exec::NetDef;
use crate::exec::PureBody;
use crate::exec::Read;
use crate::exec::Receiver;
use crate::exec::TransitionDef;
use crate::exec::Trigger;
use crate::file::NetFile;
use crate::file::NetFileError;
use crate::file::PlaceCapacity;
use crate::file::Section;
use crate::file::TransitionSpec;

/// The Rust a net file cannot hold: a transition's guard and binding, and the
/// firing of one that runs no declared effect.
pub struct Logic {
    bind: Read,
    body: Option<PureBody>,
}

impl Logic {
    /// A transition whose firing is this body. The file declares no effect for
    /// it.
    pub fn pure(bind: Read, body: PureBody) -> Self {
        Self {
            bind,
            body: Some(body),
        }
    }

    /// A transition whose firing runs the effect the file declares.
    pub fn effect(bind: Read) -> Self {
        Self { bind, body: None }
    }
}

/// Builds the net the file at `path` declares. `logic` is asked for every
/// transition the file holds, by name.
pub fn load(path: &Path, logic: impl FnMut(&str) -> Logic) -> Result<NetDef, NetLoadError> {
    let path = path.display().to_string();
    let text = fs::read_to_string(&path).map_err(|e| NetLoadError::Unreadable {
        path: path.clone(),
        message: e.to_string(),
    })?;
    net_def(&path, &text, logic)
}

/// [`load`] on text already in hand. `path` is what the refusals name.
pub fn net_def(
    path: &str,
    text: &str,
    mut logic: impl FnMut(&str) -> Logic,
) -> Result<NetDef, NetLoadError> {
    let file = NetFile::parse(text).map_err(|source| NetLoadError::Refused {
        at: at(path, text, source.declaration()),
        source,
    })?;
    let mut net = NetDef::default();
    for (place, spec) in &file.places {
        if spec.pool.is_some() {
            return Err(NetLoadError::Pool {
                at: at(path, text, Some((Section::Places, place))),
                place: place.clone(),
            });
        }
        net = match spec.capacity {
            PlaceCapacity::Single => net.place(place, Class::Process),
            PlaceCapacity::Register => net.register(place, Class::Process),
            PlaceCapacity::Fifo => net.fifo(place, Class::Process),
        };
    }
    // The executor fires in declaration order, so the authored priority — the
    // higher fires first — becomes that order.
    let mut transitions: Vec<_> = file.transitions.iter().collect();
    transitions.sort_by_key(|t| std::cmp::Reverse(t.1.priority));
    for (name, spec) in transitions {
        net = net.transition(transition(name, spec, logic(name)));
    }
    Ok(net)
}

fn transition(name: &str, spec: &TransitionSpec, logic: Logic) -> TransitionDef {
    let body = match (&spec.effect, logic.body) {
        (None, Some(body)) => Body::Pure(body),
        (Some(_), None) => Body::Effectful(receiver(
            spec.receiver
                .expect("the format pairs an effect with a receiver"),
        )),
        (None, None) => {
            panic!("transition `{name}` declares no effect, so its firing needs a body")
        }
        (Some(effect), Some(_)) => {
            panic!("transition `{name}` runs the effect `{effect}`, so its firing is not a body")
        }
    };
    let mut t = TransitionDef::new(name, trigger(spec.trigger), logic.bind, body);
    for place in &spec.consumes {
        t = t.consumes(place);
    }
    for place in &spec.reads {
        t = t.reads(place);
    }
    for place in &spec.inhibits {
        t = t.inhibits(place);
    }
    for place in &spec.facts {
        t = t.fact(place);
    }
    if let Some(place) = &spec.on_failure {
        t = t.on_failure(place);
    }
    if let Some(place) = &spec.on_timeout {
        t = t.on_timeout(place);
    }
    if let Some(timers) = &spec.timers {
        if let Some(run) = timers.run {
            t = t.run_timer(run.duration());
        }
        if let Some(abandon) = timers.abandon {
            t = t.abandon(abandon.duration());
        }
        if let Some(count_bound) = timers.count_bound {
            let backoff = timers
                .backoff
                .expect("the format pairs a count bound with a backoff");
            t = t.retry(backoff.duration(), count_bound);
        }
    }
    t
}

fn trigger(trigger: crate::file::Trigger) -> Trigger {
    match trigger {
        crate::file::Trigger::Auto => Trigger::Auto,
        crate::file::Trigger::Manual => Trigger::Manual,
    }
}

fn receiver(receiver: crate::file::Receiver) -> Receiver {
    match receiver {
        crate::file::Receiver::Keyed => Receiver::Keyed,
        crate::file::Receiver::StateProbe => Receiver::StateProbe,
        crate::file::Receiver::None => Receiver::None,
    }
}

/// The file and, for a refusal that names a declaration, the line that
/// declaration is on. A declaration written in flow style has no such line,
/// and then a reader gets the file alone rather than a wrong line.
fn at(path: &str, text: &str, declaration: Option<(Section, &str)>) -> String {
    let line = declaration.and_then(|(section, key)| line_in_section(text, section, key));
    match line {
        Some(index) => format!("{path}:{index}"),
        None => path.to_string(),
    }
}

/// The 1-based line `key` is declared on inside the file's `section` map. A
/// name can be a place AND a transition, so the search stays inside the one
/// map; its entries are the lines at the indent of its first content line.
fn line_in_section(text: &str, section: Section, key: &str) -> Option<usize> {
    let mut inside = false;
    let mut entry_indent = None;
    for (index, line) in text.lines().enumerate() {
        let content = line.trim_start();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        let indent = line.len() - content.len();
        if indent == 0 {
            inside = content.trim_end() == format!("{}:", section.key());
            entry_indent = None;
        } else if inside
            && *entry_indent.get_or_insert(indent) == indent
            && content.trim_end() == format!("{key}:")
        {
            return Some(index + 1);
        }
    }
    None
}

/// Why a net file is not a runnable net. Every refusal names the file, the
/// line of the declaration it refused and the field.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum NetLoadError {
    #[error("{path}: {message}")]
    Unreadable { path: String, message: String },
    #[error("{at}: {source}")]
    Refused {
        at: String,
        #[source]
        source: NetFileError,
    },
    #[error(
        "{at}: place `{place}`: a `pool` of interchangeable tokens has no executable form; the executor holds one token per subject"
    )]
    Pool { at: String, place: String },
}
