//! Invariants declared in a type profile (ruling P-home.a), each with the
//! policy that applies when the converged state violates it.

use std::collections::BTreeMap;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use holon_api::Value;
use holon_api::pattern::Guard;
use holon_api::pattern::Subject;

/// The declared name of an invariant: what a refusal, a repair occurrence and
/// a disclosure print as its source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InvariantName(String);

impl InvariantName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for InvariantName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredInvariant {
    pub name: InvariantName,
    /// Must hold for every subject of the profile's entity in every converged
    /// state.
    pub holds: Guard,
    /// The predicate as written, for refusals and disclosures.
    pub holds_source: String,
    pub on_violation: OnViolation,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OnViolation {
    /// Every violating subject is disclosed; nothing is written.
    Disclose,
    Repair(ConstantRepair),
}

/// A repair that writes constants on the violating subject itself, so two
/// replicas that repair the same merged violation write the same values.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstantRepair {
    pub name: String,
    /// Field of the subject := constant value.
    pub delta: BTreeMap<String, Value>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InvariantRepr {
    holds: String,
    on_violation: OnViolationRepr,
}

#[derive(Debug)]
enum OnViolationRepr {
    Keyword(String),
    Repair(RepairRepr),
}

impl<'de> serde::Deserialize<'de> for OnViolationRepr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Dispatch on the YAML shape, so a malformed repair names its bad key
        // instead of serde's untagged-enum message.
        struct PolicyVisitor;
        impl<'de> serde::de::Visitor<'de> for PolicyVisitor {
            type Value = OnViolationRepr;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(
                    "an on_violation policy: `disclose` or `{ repair: <name>, set: { <field>: \
                     <constant> } }`",
                )
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<OnViolationRepr, E> {
                Ok(OnViolationRepr::Keyword(s.to_string()))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<OnViolationRepr, A::Error> {
                serde::Deserialize::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(OnViolationRepr::Repair)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                _: A,
            ) -> Result<OnViolationRepr, A::Error> {
                Err(serde::de::Error::custom(
                    "a positional `on_violation` list is refused: write it keyed, as `{ repair: \
                     <name>, target: subject, set: { <field>: <constant> }, delete: false }`",
                ))
            }
        }
        deserializer.deserialize_any(PolicyVisitor)
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairRepr {
    repair: String,
    #[serde(default)]
    target: RepairTarget,
    #[serde(default)]
    set: BTreeMap<String, serde_yaml::Value>,
    #[serde(default)]
    delete: bool,
}

/// Whose tokens a repair writes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum RepairTarget {
    /// The violating subject.
    #[default]
    Subject,
    /// A row that witnesses the violation (a hop's match), not the subject.
    Witness,
}

pub(crate) fn parse_invariants(
    entity_name: &str,
    declared: BTreeMap<String, InvariantRepr>,
) -> Result<Vec<DeclaredInvariant>> {
    declared
        .into_iter()
        .map(|(name, repr)| {
            parse_invariant(entity_name, &name, repr)
                .with_context(|| format!("invariant '{name}' of the '{entity_name}' profile"))
        })
        .collect()
}

fn parse_invariant(
    entity_name: &str,
    name: &str,
    repr: InvariantRepr,
) -> Result<DeclaredInvariant> {
    let holds = Guard::parse(&repr.holds)
        .with_context(|| format!("`holds: {}` does not parse", repr.holds))?;
    let iterates = match &holds.subject {
        Subject::Block => "block",
        Subject::Relation(relation) => relation.as_str(),
        Subject::Clock => "clock",
    };
    if iterates != entity_name {
        bail!(
            "`holds: {}` iterates the '{iterates}' relation, but the profile declares '{entity_name}'",
            repr.holds
        );
    }
    let on_violation = match repr.on_violation {
        OnViolationRepr::Keyword(keyword) if keyword == "disclose" => OnViolation::Disclose,
        OnViolationRepr::Keyword(keyword) => bail!(
            "unknown on_violation policy '{keyword}': write `disclose` or a mapping \
             `{{ repair: <name>, set: {{ <field>: <constant> }} }}`"
        ),
        OnViolationRepr::Repair(repair) => OnViolation::Repair(
            constant_repair(entity_name, repair)
                .context("refused by ruling P-conv.a: a repair must be a constant delta on the subject itself, or the policy must be `disclose`")?,
        ),
    };
    Ok(DeclaredInvariant {
        name: InvariantName(name.to_string()),
        holds,
        holds_source: repr.holds,
        on_violation,
    })
}

fn constant_repair(entity_name: &str, repair: RepairRepr) -> Result<ConstantRepair> {
    let name = &repair.repair;
    if repair.target == RepairTarget::Witness {
        bail!(
            "repair '{name}' writes the witness of the violation, not the subject; each replica \
             can see a different witness, so two replicas would write different things"
        );
    }
    if repair.delete {
        bail!(
            "repair '{name}' deletes the subject; deleting authored content is not a constant delta"
        );
    }
    if repair.set.is_empty() {
        bail!("repair '{name}' writes nothing; name the fields it sets");
    }
    let fields = holon_api::schema::builtin_entity(entity_name).ok_or_else(|| {
        anyhow::anyhow!(
            "repair '{name}': '{entity_name}' has no built-in schema to check its fields against"
        )
    })?;
    let delta = repair
        .set
        .iter()
        .map(|(field, value)| {
            if fields.field(field).is_none() {
                bail!("repair '{name}' sets '{field}', which '{entity_name}' does not declare");
            }
            let value = yaml_scalar(value)
                .with_context(|| format!("repair '{name}' sets '{field}' to a non-constant"))?;
            if let Value::String(text) = &value
                && text.starts_with("= ")
            {
                bail!(
                    "repair '{name}' computes '{field}' from `{text}`; only a constant value \
                     converges"
                );
            }
            Ok((field.clone(), value))
        })
        .collect::<Result<_>>()?;
    Ok(ConstantRepair {
        name: name.clone(),
        delta,
    })
}

fn yaml_scalar(value: &serde_yaml::Value) -> Result<Value> {
    Ok(match value {
        serde_yaml::Value::String(s) => Value::String(s.clone()),
        serde_yaml::Value::Bool(b) => Value::Boolean(*b),
        serde_yaml::Value::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => bail!("{n} is not an integer"),
        },
        other => bail!("{other:?} is not a scalar"),
    })
}
