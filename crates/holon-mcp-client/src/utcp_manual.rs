//! UTCP 1.x manuals, as Holon's own serde types.
//!
//! A sidecar's `utcp:` section is a VERBATIM manual: the same document a
//! standard UTCP client reads, with no Holon key inside it. Everything the
//! standard lacks — the request envelope, the cadence, the response mapping —
//! lives beside it under `holon:` ([`crate::integration_config::HolonSection`]).
//!
//! **The reader is forward-tolerant, by the rule Holon is proposing upstream as
//! PR-1.** A `utcp:` key this build does not model is IGNORED and PRESERVED,
//! with a warning naming it; a tool whose `call_template_type` this build
//! cannot drive is SKIPPED, with a warning naming it — the rest of the manual
//! still loads. So a real published 1.x manual carrying `info`, `auth`,
//! `query_params` or extra call-template fields imports without being edited
//! down first. `holon:` keys get the opposite treatment and stay loudly
//! refused: those are ours, and a typo there is a mapping that silently never
//! runs.
//!
//! Preservation is by content, not by position: an unmodelled key survives an
//! export but is written after the keys this build names.
//!
//! `rs-utcp` is deliberately not a dependency — it tracks spec 0.3 while the
//! spec is at 1.x, and the schema here is three top-level fields plus one call
//! template.

use anyhow::Result;
use serde::Deserialize;
use serde::Serialize;

/// A UTCP manual: the tools one external system publishes.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct UtcpManual {
    /// The spec version the manual is written against.
    pub utcp_version: String,
    /// The publisher's own version of this manual's content.
    pub manual_version: String,
    pub tools: Vec<UtcpTool>,
    /// Manual-level keys this build does not model, kept so an export gives
    /// back what was imported.
    #[serde(flatten, default)]
    pub extra: Extras,
}

/// Keys a manual carried that this build does not model.
///
/// A `BTreeMap` rather than an ordered list because `#[serde(flatten)]` needs a
/// map: content survives an export, the position it sat in does not. An empty
/// one contributes no keys, so a manual that carried none exports byte-identically.
pub type Extras = std::collections::BTreeMap<String, OrderedJson>;

impl UtcpManual {
    /// Every key in this manual this build does not model, dotted, in the order
    /// a reader meets them. Non-empty is normal, not an error: it is what the
    /// forward-tolerance rule buys, and each one is disclosed at load.
    pub fn unmodelled_keys(&self) -> Vec<String> {
        let mut found: Vec<String> = self
            .extra
            .iter()
            .map(|(k, _)| format!("utcp.{k}"))
            .collect();
        for tool in &self.tools {
            for (k, _) in &tool.extra {
                found.push(format!("utcp.tools[{}].{k}", tool.name));
            }
            for (k, _) in &tool.tool_call_template.extra {
                found.push(format!("utcp.tools[{}].tool_call_template.{k}", tool.name));
            }
        }
        found
    }

    /// The tool named `name`, or a loud failure listing what the manual does
    /// publish. A `holon:` entry for a tool the manual never declared is a
    /// mapping that can never run.
    pub fn tool(&self, name: &str) -> Result<&UtcpTool> {
        self.tools.iter().find(|t| t.name == name).ok_or_else(|| {
            anyhow::anyhow!(
                "the manual declares no tool named '{name}' (it declares: {:?})",
                self.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
            )
        })
    }
}

/// One callable tool. `inputs`/`outputs` are JSON Schema documents the manual
/// carries for discovery; Holon does not evaluate them, so they are held
/// verbatim rather than parsed into a schema model that would lose the parts
/// it does not model.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct UtcpTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<OrderedJson>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<OrderedJson>,
    pub tool_call_template: CallTemplate,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Tool-level keys this build does not model (`auth`, `query_params`, …).
    #[serde(flatten, default)]
    pub extra: Extras,
}

/// How a tool is reached. The spec allows several `call_template_type`s (`http`,
/// `cli`, `mcp`, …); this build serves `http` and SKIPS a tool declaring any
/// other, disclosing the skip by name ([`Self::unsupported_reason`]). One tool
/// Holon cannot drive therefore costs that tool, not the manual: a peer
/// publishing an `mcp` tool beside its `http` ones is still a peer we can talk
/// to. Only a MALFORMED manual is refused whole.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CallTemplate {
    /// The publisher's name for the endpoint group. Carried for export;
    /// Holon addresses tools by [`UtcpTool::name`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub call_template_type: String,
    /// May be, or contain, a `${VAR}` reference. Referencing a variable is
    /// what marks its value secret, which is what strips it from every error,
    /// log line and toast (`assets/integrations/README.md` §2).
    pub url: String,
    pub http_method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// The input property whose value becomes the request body. Holon builds
    /// the body from the `holon:` section's template instead, so this is
    /// carried for export only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_field: Option<String>,
    /// Call-template keys this build does not model (`headers`, `auth`, …).
    #[serde(flatten, default)]
    pub extra: Extras,
}

impl CallTemplate {
    /// Whether this build can drive the tool, or the reason it cannot.
    ///
    /// A transport we do not serve is a reason to SKIP one tool, not to refuse
    /// the manual: a peer that publishes an `mcp` tool beside its `http` ones
    /// is a peer we can still talk to. The caller discloses the skip.
    pub fn unsupported_reason(&self, tool: &str) -> Option<String> {
        (self.call_template_type != "http").then(|| {
            format!(
                "tool '{tool}' declares call_template_type '{}', and this build serves only \
                 'http'; the tool is skipped and the rest of the manual still loads",
                self.call_template_type
            )
        })
    }
}

/// A JSON document whose object keys keep the order they were written in.
///
/// `serde_json::Map` is a `BTreeMap` in this workspace, so a schema read
/// through it comes back alphabetized — and an export that reorders a
/// publisher's manual is not the manual they gave us.
#[derive(Debug, Clone, PartialEq)]
pub enum OrderedJson {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<OrderedJson>),
    Object(Vec<(String, OrderedJson)>),
}

impl Serialize for OrderedJson {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            OrderedJson::Null => serializer.serialize_unit(),
            OrderedJson::Bool(b) => serializer.serialize_bool(*b),
            OrderedJson::Number(n) => n.serialize(serializer),
            OrderedJson::String(s) => serializer.serialize_str(s),
            OrderedJson::Array(items) => serializer.collect_seq(items),
            OrderedJson::Object(entries) => {
                serializer.collect_map(entries.iter().map(|(k, v)| (k, v)))
            }
        }
    }
}

impl<'de> Deserialize<'de> for OrderedJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(OrderedJsonVisitor)
    }
}

struct OrderedJsonVisitor;

impl<'de> serde::de::Visitor<'de> for OrderedJsonVisitor {
    type Value = OrderedJson;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_unit<E>(self) -> Result<OrderedJson, E> {
        Ok(OrderedJson::Null)
    }

    fn visit_none<E>(self) -> Result<OrderedJson, E> {
        Ok(OrderedJson::Null)
    }

    fn visit_bool<E>(self, v: bool) -> Result<OrderedJson, E> {
        Ok(OrderedJson::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<OrderedJson, E> {
        Ok(OrderedJson::Number(v.into()))
    }

    fn visit_u64<E>(self, v: u64) -> Result<OrderedJson, E> {
        Ok(OrderedJson::Number(v.into()))
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<OrderedJson, E> {
        serde_json::Number::from_f64(v)
            .map(OrderedJson::Number)
            .ok_or_else(|| E::custom(format!("{v} is not a JSON number")))
    }

    fn visit_str<E>(self, v: &str) -> Result<OrderedJson, E> {
        Ok(OrderedJson::String(v.to_string()))
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<OrderedJson, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(OrderedJson::Array(items))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<OrderedJson, A::Error> {
        let mut entries = Vec::new();
        while let Some((key, value)) = map.next_entry::<String, OrderedJson>()? {
            entries.push((key, value));
        }
        Ok(OrderedJson::Object(entries))
    }
}
