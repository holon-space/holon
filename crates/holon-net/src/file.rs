//! The standalone net file: a whole runtime net — places, transitions, shell
//! effects and probes — as the types one YAML file parses into.
//!
//! This module is the format and nothing else. It holds no marking, runs no
//! effect and builds no [`crate::exec::NetDef`]: the loader (#370), the shell
//! effect (#371) and the probe host (#373) each read these types and own their
//! own behaviour. What the format does own is every refusal a reader can
//! decide from the file alone, and each one names the place, the transition or
//! the effect it refused.
//!
//! The writer emits ONE canonical form — block style, authored key order,
//! durations as `30s` / `5m` / `2h` — so a file in that form is written back
//! byte for byte. Comments are not part of the format; they do not survive a
//! write-back, which is why the net file carries no prose and the format is
//! documented in `docs/Architecture/NetFileFormat.md`.

use std::fmt;
use std::marker::PhantomData;
use std::time::Duration;

use indexmap::IndexMap;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use serde::de;

/// The placeholders a shell effect's log path pattern may name.
const LOG_PLACEHOLDERS: [&str; 4] = ["transition", "subject", "attempt", "occurrence"];

/// The top-level map of a net file a declaration lives in. `Display` is the
/// word a refusal uses for one of its entries, and [`Section::key`] the file's
/// key of the map itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Places,
    Transitions,
    Effects,
    Probes,
}

impl Section {
    pub fn key(self) -> &'static str {
        match self {
            Self::Places => "places",
            Self::Transitions => "transitions",
            Self::Effects => "effects",
            Self::Probes => "probes",
        }
    }
}

impl fmt::Display for Section {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Places => "place",
            Self::Transitions => "transition",
            Self::Effects => "effect",
            Self::Probes => "probe",
        })
    }
}

/// One net, as one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetFile {
    pub name: String,
    #[serde(deserialize_with = "unique_keys")]
    pub places: IndexMap<String, PlaceSpec>,
    #[serde(deserialize_with = "unique_keys")]
    pub transitions: IndexMap<String, TransitionSpec>,
    #[serde(
        default,
        skip_serializing_if = "IndexMap::is_empty",
        deserialize_with = "unique_keys"
    )]
    pub effects: IndexMap<String, EffectSpec>,
    #[serde(
        default,
        skip_serializing_if = "IndexMap::is_empty",
        deserialize_with = "unique_keys"
    )]
    pub probes: IndexMap<String, ProbeSpec>,
}

impl NetFile {
    /// Parses one net file. Every refusal names what it refused.
    pub fn parse(text: &str) -> Result<Self, NetFileError> {
        let file: Self =
            serde_yaml::from_str(text).map_err(|e| NetFileError::Syntax(e.to_string()))?;
        file.check()?;
        Ok(file)
    }

    /// Writes the canonical form. A file already in it is reproduced byte for
    /// byte.
    pub fn to_yaml(&self) -> String {
        serde_yaml::to_string(self).expect("a net file holds only YAML scalars, maps and sequences")
    }

    /// The refusals that need more than one field: a reference to a name the
    /// file does not declare, and a declaration that contradicts itself.
    fn check(&self) -> Result<(), NetFileError> {
        for (place, spec) in &self.places {
            spec.check(place)?;
        }
        for (transition, spec) in &self.transitions {
            spec.check(transition, self)?;
        }
        for (effect, spec) in &self.effects {
            spec.check(effect)?;
        }
        for (probe, spec) in &self.probes {
            spec.check(probe, self)?;
        }
        Ok(())
    }

    fn declares(&self, place: &str) -> bool {
        self.places.contains_key(place)
    }
}

/// How many tokens a place holds per subject, and what a second put does.
///
/// The three kinds of ruling 322/E3.b. A pool (`pool: N`) is the one case
/// where the tokens are interchangeable rather than one per subject, so it
/// needs the queue of `Fifo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceSpec {
    pub capacity: PlaceCapacity,
    /// `N` interchangeable tokens — a slot pool or a budget (#372).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<u32>,
}

impl PlaceSpec {
    fn check(&self, place: &str) -> Result<(), NetFileError> {
        match self.pool {
            None => Ok(()),
            Some(0) => Err(NetFileError::EmptyPool {
                place: place.to_owned(),
            }),
            Some(_) if self.capacity != PlaceCapacity::Fifo => Err(NetFileError::PoolNeedsFifo {
                place: place.to_owned(),
                capacity: self.capacity,
            }),
            Some(_) => Ok(()),
        }
    }
}

/// What a put into an occupied place does (ruling 322/E3.b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceCapacity {
    /// One token per subject; a second put refuses loudly. The default choice.
    Single,
    /// A queue per subject: candidates, waiting work, a pool of slots.
    Fifo,
    /// One token per subject that a fact put REPLACES — a re-derived value.
    Register,
}

impl PlaceCapacity {
    fn from_word(word: &str) -> Option<Self> {
        match word {
            "single" => Some(Self::Single),
            "fifo" => Some(Self::Fifo),
            "register" => Some(Self::Register),
            _ => None,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Fifo => "fifo",
            Self::Register => "register",
        }
    }
}

impl fmt::Display for PlaceCapacity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.word())
    }
}

impl<'de> Deserialize<'de> for PlaceCapacity {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let word = String::deserialize(deserializer)?;
        Self::from_word(&word).ok_or_else(|| {
            de::Error::custom(format!(
                "`capacity` is one of single, fifo or register; got `{word}`"
            ))
        })
    }
}

/// Who fires a transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// The executor fires it whenever it is enabled.
    Auto,
    /// Only a caller's manual fire does (a decision gate, a human step).
    Manual,
}

/// One transition: its arcs, its order among the producers of a place, the
/// effect it runs and the timers that bound a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionSpec {
    pub trigger: Trigger,
    /// The order among the producers of one place; the higher fires first.
    pub priority: i32,
    /// Input arcs that take the token.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumes: Vec<String>,
    /// Input arcs that leave the token and go stale when it changes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<String>,
    /// Input arcs that enable the transition only while the place is empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inhibits: Vec<String>,
    /// Output arcs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub produces: Vec<String>,
    /// Output arcs whose tokens are facts about the outside world, so they
    /// commit even when the read inputs changed while the effect ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<String>,
    /// The failure port: the place a run that settled failed puts its token
    /// into (ADR 0032 §9.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<String>,
    /// The timeout port: the place the end of the `run` timer puts its token
    /// into when the run never settled (ADR 0032 §9.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_timeout: Option<String>,
    /// The effect descriptor a firing runs; absent means a pure firing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
    /// What resolves an unknown outcome. It comes with `effect` and only with
    /// it: a pure firing has no outcome to resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<Receiver>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timers: Option<TimerSpec>,
}

/// What resolves a run whose outcome is unknown (ADR 0032 §9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Receiver {
    /// A receipt deduped by occurrence id, so the effect may run again — which
    /// is a retry, and a retry needs a `count_bound`.
    Keyed,
    /// A probe of the outside world re-derives the outcome.
    StateProbe,
    /// Nothing resolves it: the run parks until someone fires a manual
    /// transition.
    None,
}

impl TransitionSpec {
    fn check(&self, transition: &str, net: &NetFile) -> Result<(), NetFileError> {
        if self.consumes.is_empty() && self.reads.is_empty() && self.inhibits.is_empty() {
            return Err(NetFileError::NoInputArc {
                transition: transition.to_owned(),
            });
        }
        let arcs = [
            ("consumes", &self.consumes),
            ("reads", &self.reads),
            ("inhibits", &self.inhibits),
            ("produces", &self.produces),
            ("facts", &self.facts),
        ];
        for (field, places) in arcs {
            for place in places {
                if !net.declares(place) {
                    return Err(NetFileError::UndeclaredPlace {
                        owner: Section::Transitions,
                        name: transition.to_owned(),
                        field,
                        place: place.clone(),
                    });
                }
            }
        }
        let ports = [
            ("on_failure", &self.on_failure),
            ("on_timeout", &self.on_timeout),
        ];
        for (field, port) in ports {
            if let Some(place) = port
                && !net.declares(place)
            {
                return Err(NetFileError::UndeclaredPlace {
                    owner: Section::Transitions,
                    name: transition.to_owned(),
                    field,
                    place: place.clone(),
                });
            }
        }
        if let Some(effect) = &self.effect
            && !net.effects.contains_key(effect)
        {
            return Err(NetFileError::UndeclaredEffect {
                transition: transition.to_owned(),
                effect: effect.clone(),
            });
        }
        match (&self.effect, self.receiver) {
            (Some(_), None) => {
                return Err(NetFileError::EffectWithoutReceiver {
                    transition: transition.to_owned(),
                });
            }
            (None, Some(_)) => {
                return Err(NetFileError::ReceiverWithoutEffect {
                    transition: transition.to_owned(),
                });
            }
            _ => {}
        }
        if self.receiver == Some(Receiver::Keyed)
            && self.timers.as_ref().and_then(|t| t.count_bound).is_none()
        {
            return Err(NetFileError::KeyedWithoutCountBound {
                transition: transition.to_owned(),
            });
        }
        if let Some(timers) = &self.timers {
            timers.check(transition)?;
        }
        Ok(())
    }
}

/// The timers that bound one run of a transition (ADR 0032 §9.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimerSpec {
    /// A run without an outcome this long is ended by the timeout port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<DurationSpec>,
    /// An attempt without a result this long after it started is orphaned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abandon: Option<DurationSpec>,
    /// The count bound of a run: attempts, the first included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count_bound: Option<u32>,
    /// The wait before the next attempt. It comes with `count_bound`: one
    /// without the other is half a retry rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff: Option<DurationSpec>,
}

impl TimerSpec {
    fn check(&self, transition: &str) -> Result<(), NetFileError> {
        if self.run.is_none()
            && self.abandon.is_none()
            && self.count_bound.is_none()
            && self.backoff.is_none()
        {
            return Err(NetFileError::NoTimer {
                transition: transition.to_owned(),
            });
        }
        if self.count_bound == Some(0) {
            return Err(NetFileError::ZeroCountBound {
                transition: transition.to_owned(),
            });
        }
        if self.count_bound.is_some() != self.backoff.is_some() {
            return Err(NetFileError::RetryNeedsBoth {
                transition: transition.to_owned(),
            });
        }
        Ok(())
    }
}

/// What a firing does to the outside world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EffectSpec {
    Shell(ShellEffect),
}

impl EffectSpec {
    fn check(&self, effect: &str) -> Result<(), NetFileError> {
        let Self::Shell(shell) = self;
        shell.check(effect)
    }
}

/// A child process group, its log and the meaning of its exit code (#371).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellEffect {
    pub command: CommandSpec,
    /// Where one attempt's output goes. It names `{attempt}` so a retry never
    /// overwrites the log of the attempt before it; `{transition}`,
    /// `{subject}` and `{occurrence}` are the other placeholders.
    pub log_path: String,
    pub exit_codes: ExitCodeMap,
}

impl ShellEffect {
    fn check(&self, effect: &str) -> Result<(), NetFileError> {
        self.command.check(Section::Effects, effect)?;
        check_log_path(effect, &self.log_path)?;
        self.exit_codes.check(effect)
    }
}

/// A total map from a child's exit code to an outcome: the codes that are
/// `Done`, the codes that are `Failed`, and what every other code means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExitCodeMap {
    pub done: Vec<i32>,
    pub failed: Vec<i32>,
    /// Every code neither list names. A code whose meaning is unknown belongs
    /// in `outcome_unknown`, where the receiver decides, not in `failed`.
    pub other: Outcome,
}

impl ExitCodeMap {
    fn check(&self, effect: &str) -> Result<(), NetFileError> {
        let mut seen = Vec::new();
        for code in self.done.iter().chain(&self.failed) {
            if seen.contains(code) {
                return Err(NetFileError::ExitCodeTwice {
                    effect: effect.to_owned(),
                    code: *code,
                });
            }
            seen.push(*code);
        }
        Ok(())
    }
}

/// How a run settles (ADR 0032 §9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    Failed,
    OutcomeUnknown,
}

/// A shell command, with the environment it is allowed to see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    /// The program and its arguments. No shell is interposed: a pipeline is
    /// spelled out as `[bash, -lc, "…"]`.
    pub argv: Vec<String>,
    pub working_dir: String,
    /// The only variables of the host's environment the child inherits.
    pub env_allow: Vec<String>,
    /// The child is killed when it runs longer than this.
    pub timeout: DurationSpec,
}

impl CommandSpec {
    fn check(&self, owner: Section, name: &str) -> Result<(), NetFileError> {
        if self.argv.is_empty() {
            return Err(NetFileError::EmptyArgv {
                owner,
                name: name.to_owned(),
            });
        }
        Ok(())
    }
}

/// A command the host runs on its own interval, whose stdout is facts: one
/// JSON object per line, each becoming one token in `facts_into` (#373).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeSpec {
    pub command: CommandSpec,
    /// How long the host waits between two runs.
    pub interval: DurationSpec,
    pub facts_into: String,
    /// The field of each JSON line that names the token's subject.
    pub subject_field: String,
}

impl ProbeSpec {
    fn check(&self, probe: &str, net: &NetFile) -> Result<(), NetFileError> {
        self.command.check(Section::Probes, probe)?;
        if !net.declares(&self.facts_into) {
            return Err(NetFileError::UndeclaredPlace {
                owner: Section::Probes,
                name: probe.to_owned(),
                field: "facts_into",
                place: self.facts_into.clone(),
            });
        }
        Ok(())
    }
}

/// A duration, written `250ms`, `30s`, `5m` or `2h`.
///
/// Canonical form is the largest unit that divides it exactly, which is what
/// makes a write-back byte-stable: `90s` is `90s`, `120s` is `2m`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurationSpec(Duration);

impl DurationSpec {
    pub fn duration(self) -> Duration {
        self.0
    }

    fn parse(text: &str) -> Result<Self, String> {
        let split = text
            .find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| format!("`{text}` carries no duration unit; expected ms, s, m or h"))?;
        let (digits, unit) = text.split_at(split);
        let count: u64 = digits
            .parse()
            .map_err(|_| format!("`{text}` does not begin with a whole number of {unit}"))?;
        let per_unit = match unit {
            "ms" => 1,
            "s" => 1_000,
            "m" => 60_000,
            "h" => 3_600_000,
            other => {
                return Err(format!(
                    "`{text}` names the unknown duration unit `{other}`; expected ms, s, m or h"
                ));
            }
        };
        let millis = count
            .checked_mul(per_unit)
            .ok_or_else(|| format!("`{text}` is longer than a duration can hold"))?;
        if millis == 0 {
            return Err(format!(
                "`{text}` is zero; a timer and a timeout are longer than zero"
            ));
        }
        Ok(Self(Duration::from_millis(millis)))
    }
}

impl fmt::Display for DurationSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let millis = self.0.as_millis();
        let (count, unit) = if millis.is_multiple_of(3_600_000) {
            (millis / 3_600_000, "h")
        } else if millis.is_multiple_of(60_000) {
            (millis / 60_000, "m")
        } else if millis.is_multiple_of(1_000) {
            (millis / 1_000, "s")
        } else {
            (millis, "ms")
        };
        write!(f, "{count}{unit}")
    }
}

impl Serialize for DurationSpec {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DurationSpec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(de::Error::custom)
    }
}

fn check_log_path(effect: &str, pattern: &str) -> Result<(), NetFileError> {
    let unbalanced = || NetFileError::UnbalancedLogPath {
        effect: effect.to_owned(),
    };
    let mut rest = pattern;
    loop {
        let Some(open) = rest.find('{') else {
            if rest.contains('}') {
                return Err(unbalanced());
            }
            break;
        };
        if rest[..open].contains('}') {
            return Err(unbalanced());
        }
        rest = &rest[open + 1..];
        let close = rest.find('}').ok_or_else(unbalanced)?;
        let placeholder = &rest[..close];
        if !LOG_PLACEHOLDERS.contains(&placeholder) {
            return Err(NetFileError::UnknownLogPlaceholder {
                effect: effect.to_owned(),
                placeholder: placeholder.to_owned(),
            });
        }
        rest = &rest[close + 1..];
    }
    if !pattern.contains("{attempt}") {
        return Err(NetFileError::LogPathWithoutAttempt {
            effect: effect.to_owned(),
        });
    }
    Ok(())
}

/// A map whose keys must not repeat: the last of two equal keys would
/// otherwise silently win over the first.
fn unique_keys<'de, D, V>(deserializer: D) -> Result<IndexMap<String, V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct Keys<V>(PhantomData<V>);

    impl<'de, V: Deserialize<'de>> de::Visitor<'de> for Keys<V> {
        type Value = IndexMap<String, V>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a map whose keys do not repeat")
        }

        fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut out = IndexMap::new();
            while let Some(key) = map.next_key::<String>()? {
                let value = map.next_value::<V>()?;
                if out.insert(key.clone(), value).is_some() {
                    return Err(de::Error::custom(format!(
                        "the key `{key}` is declared twice"
                    )));
                }
            }
            Ok(out)
        }
    }

    deserializer.deserialize_map(Keys(PhantomData))
}

/// Why a net file was refused. Every variant names what it refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NetFileError {
    /// The YAML itself, or one field of it: the message carries the field and
    /// the position the parser refused at.
    #[error("the net file is not a net: {0}")]
    Syntax(String),
    #[error("place `{place}`: `pool` needs `capacity: fifo`, not `{capacity}`")]
    PoolNeedsFifo {
        place: String,
        capacity: PlaceCapacity,
    },
    #[error("place `{place}`: a `pool` of 0 tokens can never be taken from")]
    EmptyPool { place: String },
    #[error(
        "transition `{transition}`: no input arc — `consumes`, `reads` or `inhibits` names at least one place"
    )]
    NoInputArc { transition: String },
    #[error(
        "{owner} `{name}`: `{field}` names the place `{place}`, which the file does not declare"
    )]
    UndeclaredPlace {
        owner: Section,
        name: String,
        field: &'static str,
        place: String,
    },
    #[error(
        "transition `{transition}`: `effect` names `{effect}`, which no effect descriptor declares"
    )]
    UndeclaredEffect { transition: String, effect: String },
    #[error(
        "transition `{transition}`: an `effect` needs a `receiver` — what resolves an unknown outcome"
    )]
    EffectWithoutReceiver { transition: String },
    #[error(
        "transition `{transition}`: a `receiver` without an `effect` — a pure firing has no outcome to resolve"
    )]
    ReceiverWithoutEffect { transition: String },
    #[error(
        "transition `{transition}`: a `keyed` receiver lets the effect run again, so it needs a `count_bound`"
    )]
    KeyedWithoutCountBound { transition: String },
    #[error("transition `{transition}`: `count_bound` and `backoff` come together")]
    RetryNeedsBoth { transition: String },
    #[error("transition `{transition}`: `timers` declares no timer")]
    NoTimer { transition: String },
    #[error("transition `{transition}`: a `count_bound` of 0 forbids the first attempt")]
    ZeroCountBound { transition: String },
    #[error("effect `{effect}`: the exit code {code} is mapped twice")]
    ExitCodeTwice { effect: String, code: i32 },
    #[error("effect `{effect}`: `log_path` names the unknown placeholder `{{{placeholder}}}`")]
    UnknownLogPlaceholder { effect: String, placeholder: String },
    #[error("effect `{effect}`: `log_path` has an unbalanced brace")]
    UnbalancedLogPath { effect: String },
    #[error(
        "effect `{effect}`: `log_path` must name `{{attempt}}`, or a retry overwrites the log of the attempt before it"
    )]
    LogPathWithoutAttempt { effect: String },
    #[error("{owner} `{name}`: `argv` is empty, so there is no program to run")]
    EmptyArgv { owner: Section, name: String },
}

impl NetFileError {
    /// The declaration the refusal names, as the map it lives in and its key,
    /// so a reader can be pointed at the line that declares it. A name can be
    /// a place AND a transition, so the map is part of the answer. `None` for
    /// a refusal of the parser, whose own message carries the position.
    pub fn declaration(&self) -> Option<(Section, &str)> {
        match self {
            Self::Syntax(_) => None,
            Self::PoolNeedsFifo { place, .. } | Self::EmptyPool { place } => {
                Some((Section::Places, place))
            }
            Self::NoInputArc { transition }
            | Self::UndeclaredEffect { transition, .. }
            | Self::EffectWithoutReceiver { transition }
            | Self::ReceiverWithoutEffect { transition }
            | Self::KeyedWithoutCountBound { transition }
            | Self::RetryNeedsBoth { transition }
            | Self::NoTimer { transition }
            | Self::ZeroCountBound { transition } => Some((Section::Transitions, transition)),
            Self::ExitCodeTwice { effect, .. }
            | Self::UnknownLogPlaceholder { effect, .. }
            | Self::UnbalancedLogPath { effect }
            | Self::LogPathWithoutAttempt { effect } => Some((Section::Effects, effect)),
            Self::UndeclaredPlace { owner, name, .. } | Self::EmptyArgv { owner, name } => {
                Some((*owner, name))
            }
        }
    }
}
