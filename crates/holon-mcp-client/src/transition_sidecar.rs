//! The `transitions:` section of an MCP sidecar, loaded into the high-level
//! transition IR ([`TransitionDescriptor`], #167). The macro emits the same IR
//! for Rust declarations, so one lowering serves both.

use std::collections::BTreeMap;

use anyhow::Context;
use anyhow::bail;
use anyhow::ensure;
use holon_api::Value;
use holon_api::render_types::TypeHint;
use holon_api::user_transition::ArcKind;
use holon_api::user_transition::AskDecl;
use holon_api::user_transition::BodyDecl;
use holon_api::user_transition::HlArc;
use holon_api::user_transition::Lane;
use holon_api::user_transition::NotImplemented;
use holon_api::user_transition::PlaceName;
use holon_api::user_transition::TimerDecl;
use holon_api::user_transition::TimerKind;
use holon_api::user_transition::ToolArg;
use holon_api::user_transition::TransitionDescriptor;
use holon_api::user_transition::TransitionName;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::de::Error as _;
use serde::de::MapAccess;
use serde::de::Visitor;

use crate::mcp_sidecar::McpSidecar;
use crate::mcp_sidecar::SyncInterval;
use crate::mcp_sidecar::UndoConfig;

/// One entry of `transitions:`, as the sidecar spells it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionSpec {
    #[serde(default)]
    pub manual: bool,
    #[serde(default)]
    pub irreversible: bool,
    #[serde(default)]
    pub persist: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub take: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inhibit: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<InputSpec>,
    pub effect: EffectSpec,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timers: Vec<TimerSpec>,
}

/// The answer type of a manual transition: exactly one key is set. A map
/// rather than an enum because `serde_yaml` reads enums only from `!tags`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InputSpec {
    /// `bool`, `string`, `number` or `entity_id:<entity>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_hint: Option<String>,
    #[serde(default, rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_: Option<EnumSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newtype: Option<String>,
    /// The subject of a token in this place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnumSpec {
    pub name: String,
    pub variants: Vec<String>,
}

/// The sidecar tool a transition calls, with its arguments.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectSpec {
    pub tool: String,
    /// Tool argument → its value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub map: BTreeMap<String, ArgSpec>,
}

/// One `effect.map` value: `input` (the answer value) or `{ literal: v }`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ArgSpec {
    Bound(String),
    Literal(LiteralSpec),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiteralSpec {
    pub literal: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimerSpec {
    pub kind: TimerKindSpec,
    pub after: SyncInterval,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerKindSpec {
    Retry,
    Run,
    Abandon,
    Delay,
}

/// The value a sidecar writes in `effect.map` for the answer value.
const MAP_INPUT: &str = "input";

/// Deserializes `transitions:`, refusing a name declared twice (a plain map
/// keeps the last one silently).
pub(crate) fn unique_transitions<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, TransitionSpec>, D::Error> {
    struct Unique;
    impl<'de> Visitor<'de> for Unique {
        type Value = BTreeMap<String, TransitionSpec>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a map of transition names to transitions")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut transitions = BTreeMap::new();
            while let Some((name, spec)) = map.next_entry::<String, TransitionSpec>()? {
                if transitions.contains_key(&name) {
                    return Err(A::Error::custom(format!(
                        "transition `{name}` is declared twice"
                    )));
                }
                transitions.insert(name, spec);
            }
            Ok(transitions)
        }
    }
    deserializer.deserialize_map(Unique)
}

/// Reads the `transitions:` of a sidecar YAML into the HL-IR.
pub fn transitions_from_sidecar(yaml: &str) -> anyhow::Result<Vec<TransitionDescriptor>> {
    McpSidecar::from_yaml(yaml)?.transition_descriptors()
}

/// Loads every transition of `sidecar`, in name order.
pub(crate) fn load_transitions(sidecar: &McpSidecar) -> anyhow::Result<Vec<TransitionDescriptor>> {
    sidecar
        .transitions
        .iter()
        .map(|(name, spec)| {
            load_transition(sidecar, name, spec).with_context(|| format!("transition `{name}`"))
        })
        .collect()
}

fn load_transition(
    sidecar: &McpSidecar,
    name: &str,
    spec: &TransitionSpec,
) -> anyhow::Result<TransitionDescriptor> {
    if spec.guard.is_some() {
        bail!("guards come with #166; not supported yet");
    }
    if spec.preview.is_some() {
        bail!("a sidecar transition has no `preview` in increment 1");
    }
    if !spec.manual {
        ensure!(
            spec.input.is_none() && !spec.persist,
            "`input` and `persist` need `manual: true`"
        );
    }

    let arcs = [
        (ArcKind::Take, &spec.take),
        (ArcKind::Read, &spec.read),
        (ArcKind::Inhibit, &spec.inhibit),
    ]
    .into_iter()
    .flat_map(|(kind, places)| places.iter().map(move |p| (kind, p)))
    .map(|(kind, place)| {
        Ok(HlArc {
            kind,
            place: place_of(sidecar, place)?,
        })
    })
    .collect::<anyhow::Result<Vec<_>>>()?;

    let tool = sidecar.tools.get(&spec.effect.tool).with_context(|| {
        format!(
            "effect tool `{}` is not declared in `tools:`",
            spec.effect.tool
        )
    })?;
    ensure!(
        tool.mutates(),
        "effect tool `{}` is `effect: read`; a transition effect writes",
        spec.effect.tool
    );
    let tool_irreversible = matches!(
        tool.undo,
        Some(UndoConfig::Irreversible { reversible: false })
    );
    ensure!(
        !tool_irreversible || spec.irreversible,
        "effect tool `{}` is irreversible (`undo: {{ reversible: false }}`), so the transition \
         must say `irreversible: true`",
        spec.effect.tool
    );

    let ask = if spec.manual {
        Some(AskDecl {
            input: spec
                .input
                .as_ref()
                .map(|input| input_type(sidecar, input))
                .transpose()?,
            preview: None,
            persist: spec.persist,
        })
    } else {
        None
    };

    Ok(TransitionDescriptor {
        name: TransitionName(name.to_owned()),
        arcs,
        body: tool_call(&spec.effect, spec.input.is_some())?,
        irreversible: spec.irreversible,
        timers: spec
            .timers
            .iter()
            .map(timer)
            .collect::<anyhow::Result<_>>()?,
        guard: None,
        ask,
    })
}

/// A place is one of the sidecar's own entities.
fn place_of(sidecar: &McpSidecar, place: &str) -> anyhow::Result<PlaceName> {
    ensure!(
        sidecar.entities.contains_key(place),
        "place `{place}` is not an entity of this sidecar (`entities:`)"
    );
    Ok(PlaceName(place.to_owned()))
}

fn input_type(sidecar: &McpSidecar, input: &InputSpec) -> anyhow::Result<TypeHint> {
    Ok(match input {
        InputSpec {
            type_hint: Some(hint),
            enum_: None,
            newtype: None,
            place: None,
        } => parse_type_hint(sidecar, hint)?,
        InputSpec {
            type_hint: None,
            enum_: Some(EnumSpec { name, variants }),
            newtype: None,
            place: None,
        } => {
            ensure!(!variants.is_empty(), "enum `{name}` has no variants");
            TypeHint::Enum {
                type_name: name.clone(),
                variants: variants.clone(),
            }
        }
        InputSpec {
            type_hint: None,
            enum_: None,
            newtype: Some(type_name),
            place: None,
        } => TypeHint::Newtype {
            type_name: type_name.clone(),
        },
        InputSpec {
            type_hint: None,
            enum_: None,
            newtype: None,
            place: Some(place),
        } => TypeHint::PlaceId {
            place: place_of(sidecar, place)?,
        },
        _ => bail!("`input` takes exactly one of `type_hint`, `enum`, `newtype`, `place`"),
    })
}

/// The `type_hint` grammar of `param_overrides`, minus its default to
/// `String`: an answer type the loader cannot read is a load error.
fn parse_type_hint(sidecar: &McpSidecar, hint: &str) -> anyhow::Result<TypeHint> {
    Ok(match hint {
        "bool" | "boolean" => TypeHint::Bool,
        "string" | "str" => TypeHint::String,
        "number" | "integer" | "int" | "i64" | "i32" => TypeHint::Number,
        _ => match hint.strip_prefix("entity_id:") {
            Some("") => bail!("type hint `{hint}` names no entity"),
            Some(entity) if sidecar.entities.contains_key(entity) => TypeHint::EntityId {
                entity_name: sidecar.prefixed_name(entity),
            },
            Some(_) => {
                return Err(anyhow::Error::new(NotImplemented {
                    lane: Lane::Integration,
                    what: "resolving an entity type of another integration (a system-wide entity \
                           registry)",
                })
                .context(format!(
                    "input type `{hint}` names no entity of this sidecar"
                )));
            }
            #[cfg(feature = "naive-twins")]
            None => TypeHint::String,
            #[cfg(not(feature = "naive-twins"))]
            None => {
                bail!("unknown type hint `{hint}`; use bool, string, number or entity_id:<entity>")
            }
        },
    })
}

/// `answers`: the transition has an answer value to bind.
fn tool_call(effect: &EffectSpec, answers: bool) -> anyhow::Result<BodyDecl> {
    let args = effect
        .map
        .iter()
        .map(|(arg, value)| {
            let value = match value {
                ArgSpec::Bound(name) => {
                    ensure!(
                        name == MAP_INPUT,
                        "effect argument `{arg}: {name}`: only `{MAP_INPUT}` (the answer value) \
                         can be bound in increment 1; token fields come with #166"
                    );
                    ensure!(
                        answers,
                        "effect argument `{arg}` binds the answer value, but the transition asks \
                         for no `input:`"
                    );
                    ToolArg::Answer
                }
                ArgSpec::Literal(LiteralSpec { literal }) => {
                    ToolArg::Literal(Value::from_json_value(literal.clone()))
                }
            };
            Ok((arg.clone(), value))
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(BodyDecl::Tool {
        tool: effect.tool.clone(),
        args,
    })
}

fn timer(spec: &TimerSpec) -> anyhow::Result<TimerDecl> {
    let kind = match (spec.kind, spec.max_attempts) {
        (TimerKindSpec::Retry, Some(max_attempts)) => TimerKind::Retry { max_attempts },
        (TimerKindSpec::Retry, None) => bail!("a `retry` timer needs `max_attempts`"),
        (_, Some(_)) => bail!("only a `retry` timer takes `max_attempts`"),
        (TimerKindSpec::Run, None) => TimerKind::Run,
        (TimerKindSpec::Abandon, None) => TimerKind::Abandon,
        (TimerKindSpec::Delay, None) => TimerKind::Delay,
    };
    Ok(TimerDecl {
        kind,
        after: spec.after.duration(),
    })
}
