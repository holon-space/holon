use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use holon_api::entity::FieldSchema;
use holon_api::entity::TypeDefinition;
use holon_api::render_types::PreconditionChecker;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct McpSidecar {
    /// Prefix prepended to all entity names for table names, ID schemes, etc.
    /// E.g. `entity_prefix: "cc_"` turns entity `session` into table
    /// `cc_session` with ID scheme `cc_session:`.
    #[serde(default)]
    pub entity_prefix: Option<String>,
    pub entities: HashMap<String, EntityConfig>,
    /// Master write switch (leases/read-write ruling). Absent = `disabled` =
    /// today's fail-loud behaviour: every non-`read` tool is denied at
    /// dispatch. `enabled` lets `idempotent`/`keyed` tools through;
    /// `once_only` tools stay blocked until a writer/lease is configured
    /// (increment 4).
    #[serde(default)]
    pub writes: WritesPolicy,
    /// Writer designation for this connector's `once_only` effects (leases/
    /// read-write ruling, increment 4). Absent = `confirm_manually` = the safe
    /// default: once_only becomes usable but every such write pauses for
    /// explicit human confirmation and never fires unattended. `always_run`
    /// means this device holds write authority and dispatches immediately.
    #[serde(default)]
    pub once_only: OnceOnlyAuthorization,
    #[serde(default)]
    pub tools: HashMap<String, ToolConfig>,
    /// Sidecar-declared derived views, created as Turso materialized views at
    /// integration startup (after the cache tables exist). Names get
    /// `entity_prefix` applied like entities do.
    ///
    /// The SQL must stay within the Turso-IVM-supported dialect:
    /// - single-level `GROUP BY` aggregates, including the arg-max concat idiom
    ///   `substr(MAX(ts || '|' || col), N)`
    /// - NO correlated subqueries, NO self-joins back to the aggregated table,
    ///   NO non-equijoin LEFT JOINs — these fail at CREATE.
    ///
    /// A view that fails to create is a hard, loud config error at connect.
    #[serde(default)]
    pub views: Vec<ViewConfig>,
}

/// A sidecar-declared derived view: `CREATE MATERIALIZED VIEW {prefix}{name} AS
/// {sql}`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ViewConfig {
    /// View name; `entity_prefix` is applied (e.g. `session_status` ->
    /// `cc_session_status`).
    pub name: String,
    /// The SELECT statement (IVM-supported dialect, see [`McpSidecar::views`]).
    pub sql: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EntityConfig {
    /// Short display name. Defaults to the entity key name if omitted.
    #[serde(default)]
    pub short_name: Option<String>,
    /// The entity name as reported by the MCP server's resource templates.
    /// Used to match auto-discovered schemas when the YAML key differs from
    /// the server's entity name (e.g. YAML key `cc-session` ↔ server
    /// `session`).
    #[serde(default)]
    pub source_name: Option<String>,
    /// Primary key column. Defaults to "id" if omitted.
    #[serde(default)]
    pub id_column: Option<String>,
    /// Schema fields for cache table DDL generation. If present, the entity
    /// can use `QueryableCache::<DynamicEntity>` with a runtime schema.
    #[serde(default)]
    pub schema: Vec<FieldSchema>,
    /// Sync configuration for pulling data from the MCP server (cache table
    /// mode).
    pub sync: Option<SyncConfig>,
    /// Virtual table configuration for on-demand querying (foreign table mode).
    /// When set, a Turso foreign table is registered that translates SQL WHERE
    /// constraints into MCP tool parameters.
    #[serde(default)]
    pub vtable: Option<crate::mcp_vtable::VtableConfig>,
    /// Render variants for this entity (presentation layer).
    /// Passed through to `TypeDefinition.profile_variants`.
    #[serde(default)]
    pub profile_variants: Vec<holon_api::ProfileVariant>,
}

impl EntityConfig {
    /// Resolve short_name with fallback to entity key name. // ALLOW(fallback):
    /// describes default-branch path, not error swallowing
    pub fn short_name_or(&self, entity_name: &str) -> String {
        self.short_name
            .clone()
            .unwrap_or_else(|| entity_name.to_string())
    }

    /// Resolve id_column with fallback to "id". // ALLOW(fallback): describes
    /// default-branch path, not error swallowing
    pub fn id_column_or_default(&self) -> String {
        self.id_column.clone().unwrap_or_else(|| "id".to_string())
    }
}

/// Declarative sync recipe: either tool-based or resource-based.
///
/// Field presence determines strategy:
/// - `list_tool` present → ToolSync (call tool, extract via `extract_path`)
/// - `list_resource` present → ResourceSync (read resource URI)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SyncConfig {
    /// MCP tool name to call for listing records (e.g. "list_emails").
    /// Required for tool-based sync.
    pub list_tool: Option<String>,
    /// JSON key in the tool response containing the records array.
    /// Required when `list_tool` is set.
    pub extract_path: Option<String>,
    /// Static parameters passed to the list tool.
    #[serde(default)]
    pub list_params: HashMap<String, serde_json::Value>,
    /// Optional cursor-based incremental sync configuration (tool sync only).
    pub cursor: Option<CursorConfig>,
    /// Optional per-column field projection (tool sync only): lifts nested JSON
    /// scalars into flat columns (e.g. Google Calendar's `start.dateTime` →
    /// `start`, `start.date` presence → `all_day`). See
    /// [`crate::mcp_sync_strategy::Projection`].
    #[serde(default)]
    pub project: HashMap<String, crate::mcp_sync_strategy::Projection>,
    /// MCP resource URI (or URI template) to read for listing records.
    /// Required for resource-based sync.
    pub list_resource: Option<String>,
    /// Parameters to expand in the resource URI template (e.g. `{project_id}`).
    #[serde(default)]
    pub uri_params: HashMap<String, String>,
    /// Optional polling interval. When set, the integration emits a poll tick
    /// for this entity on this cadence (serialized with notification resyncs).
    /// This is the freshness mechanism for servers without
    /// `resources.subscribe`, and an explicit belt-and-braces cadence for
    /// servers that have it. Accepts an integer (seconds) or a humantime-style
    /// string (`"30s"`, `"5m"`, `"1h"`).
    #[serde(default)]
    pub interval: Option<SyncInterval>,
}

/// A polling interval parsed from YAML: integer seconds or
/// `"30s"`/`"5m"`/`"1h"`.
///
/// Parse-don't-validate: an unparseable interval fails YAML deserialization
/// loudly instead of becoming a silently-ignored setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncInterval(std::time::Duration);

impl SyncInterval {
    pub fn duration(&self) -> std::time::Duration {
        self.0
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (digits, unit) = match s.find(|c: char| !c.is_ascii_digit()) {
            Some(idx) => s.split_at(idx),
            None => (s, ""),
        };
        if digits.is_empty() {
            return Err(format!(
                "invalid sync interval '{s}': expected <number>[s|m|h] or plain seconds"
            ));
        }
        let n: u64 = digits
            .parse()
            .map_err(|e| format!("invalid sync interval '{s}': {e}"))?;
        let secs = match unit.trim() {
            "" | "s" | "sec" | "secs" | "second" | "seconds" => n,
            "m" | "min" | "mins" | "minute" | "minutes" => n * 60,
            "h" | "hr" | "hour" | "hours" => n * 3600,
            other => {
                return Err(format!(
                    "invalid sync interval '{s}': unknown unit '{other}' (use s, m, or h)"
                ));
            }
        };
        if secs == 0 {
            return Err(format!("invalid sync interval '{s}': must be > 0"));
        }
        Ok(Self(std::time::Duration::from_secs(secs)))
    }
}

impl std::fmt::Display for SyncInterval {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}s", self.0.as_secs())
    }
}

impl Serialize for SyncInterval {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.0.as_secs())
    }
}

impl<'de> Deserialize<'de> for SyncInterval {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Secs(u64),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Secs(0) => Err(serde::de::Error::custom(
                "invalid sync interval: must be > 0 seconds",
            )),
            Raw::Secs(n) => Ok(Self(std::time::Duration::from_secs(n))),
            Raw::Text(s) => Self::parse(&s).map_err(serde::de::Error::custom),
        }
    }
}

impl SyncConfig {
    /// Build a `SyncStrategy` from this config.
    ///
    /// Panics if neither `list_tool` nor `list_resource` is set.
    pub fn into_strategy(&self) -> anyhow::Result<Box<dyn crate::mcp_sync_strategy::SyncStrategy>> {
        use crate::mcp_sync_strategy::ResourceSync;
        use crate::mcp_sync_strategy::ToolSync;
        use crate::mcp_sync_strategy::expand_uri_template;

        if let Some(ref list_tool) = self.list_tool {
            let extract_path = self
                .extract_path
                .clone()
                .ok_or_else(|| anyhow::anyhow!("list_tool requires extract_path"))?;
            Ok(Box::new(ToolSync {
                list_tool: list_tool.clone(),
                extract_path,
                list_params: self.list_params.clone(),
                cursor: self.cursor.clone(),
                project: self.project.clone(),
            }))
        } else if let Some(ref list_resource) = self.list_resource {
            let uri = expand_uri_template(list_resource, &self.uri_params)?;
            Ok(Box::new(ResourceSync { uri }))
        } else {
            anyhow::bail!("SyncConfig must have either list_tool or list_resource");
        }
    }
}

/// Cursor configuration for incremental sync.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CursorConfig {
    /// Field in the tool response containing the new cursor value
    pub response_field: String,
    /// Parameter name to pass the cursor back to the list tool
    pub request_param: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ToolConfig {
    pub entity: Option<String>,
    pub display_name: Option<String>,
    pub affected_fields: Option<Vec<String>>,
    pub triggered_by: Option<Vec<TriggerConfig>>,
    pub precondition: Option<RhaiPrecondition>,
    pub param_overrides: Option<HashMap<String, ParamOverride>>,
    pub undo: Option<UndoConfig>,
    /// Write-effect classification (leases/read-write ruling). Governs whether
    /// the dispatch chokepoint lets this tool through under the connector's
    /// `writes` policy. A write-shaped tool (has `affected_fields` or `undo`)
    /// MUST declare this — an absent `effect` on such a tool is a loud config
    /// error at load. Absent on a non-write-shaped tool means
    /// [`ToolEffect::Read`].
    #[serde(default)]
    pub effect: Option<ToolEffect>,
    /// For `effect: keyed` tools, names the tool argument that carries the
    /// minted idempotency key. Required when `effect` is `keyed`; a loud config
    /// error otherwise.
    #[serde(default)]
    pub key_param: Option<String>,
    /// How this tool's *successful* response reports whether the effect
    /// provably landed. Absent means a transport-level ack is itself the proof
    /// — correct only for providers that never ack an unproven effect.
    #[serde(default)]
    pub outcome: Option<OutcomeConfig>,
}

/// Which response field carries the provider's own delivery verdict, and which
/// of its values PROVE the effect landed.
///
/// Some providers deliberately ack an effect they cannot prove happened (the
/// claude-history server returns `{"outcome":"unconfirmed"}` as a success
/// whenever it cannot confirm a message reached the session). Without this
/// declaration such an ack is indistinguishable from proof of delivery, and the
/// UI claims a message landed that may never have.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OutcomeConfig {
    /// Response field holding the verdict, e.g. `outcome`.
    pub field: String,
    /// Values that PROVE the effect landed.
    pub proven: Vec<String>,
    /// Values that explicitly state the effect is NOT proven to have landed.
    #[serde(default)]
    pub unproven: Vec<String>,
}

/// What a successful response says about whether the effect landed. Parsed
/// from the provider's payload at the dispatch chokepoint, never re-decided
/// downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckVerdict {
    /// The provider vouched for delivery.
    Proven,
    /// The call succeeded but delivery is not proven; `detail` is the
    /// provider's own wording, for verbatim disclosure.
    Unproven { detail: String },
}

impl OutcomeConfig {
    /// Classify a successful tool response. A field that is missing, non-string
    /// or carries a word this declaration does not know is UNPROVEN with a
    /// detail naming exactly what was seen: the one thing we may never do is
    /// assume delivery from a payload we could not read.
    pub fn classify(&self, tool: &str, response: &holon_api::Value) -> AckVerdict {
        let holon_api::Value::Object(map) = response else {
            return AckVerdict::Unproven {
                detail: format!(
                    "'{tool}' declares `outcome.field: {}` but its response is not an object: \
                     {response:?}",
                    self.field
                ),
            };
        };
        let Some(holon_api::Value::String(verdict)) = map.get(self.field.as_str()) else {
            return AckVerdict::Unproven {
                detail: format!(
                    "'{tool}' response carries no string `{}` field, so delivery is unproven: \
                     {response:?}",
                    self.field
                ),
            };
        };
        if self.proven.iter().any(|p| p == verdict) {
            return AckVerdict::Proven;
        }
        if self.unproven.iter().any(|u| u == verdict) {
            return AckVerdict::Unproven {
                detail: format!(
                    "'{tool}' acked `{}: {verdict}` — dispatched, but delivery is NOT proven",
                    self.field
                ),
            };
        }
        AckVerdict::Unproven {
            detail: format!(
                "'{tool}' acked `{}: {verdict}`, which the sidecar classifies as neither proven \
                 nor unproven — treating it as unproven",
                self.field
            ),
        }
    }
}

/// Connector-wide write policy (leases/read-write ruling).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WritesPolicy {
    /// No external writes: every non-`read` tool is denied loud at dispatch.
    #[default]
    Disabled,
    /// `idempotent`/`keyed` tools may execute; `once_only` stays gated on a
    /// writer/lease (increment 4).
    Enabled,
}

/// Per-connector writer designation for `once_only` effects (leases/read-write
/// ruling, increment 4; Martin 2026-07-19). Parse-don't-validate: a fixed set
/// of legal values parsed at sidecar load, mapped to a
/// [`crate::write_authorization::WriteAuthorizationPolicy`] impl. Future
/// designations (vault-block role, TTL lease) add variants here without
/// touching the dispatch chokepoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnceOnlyAuthorization {
    /// Safe default: pause every once_only write for explicit human
    /// confirmation; never fire unattended.
    #[default]
    ConfirmManually,
    /// This device holds write authority: dispatch once_only writes
    /// immediately.
    AlwaysRun,
}

/// Per-tool write-effect classification (ADR 0024 P4 taxonomy). Parse-don't-
/// validate: a fixed set of legal values, parsed at sidecar load, so the
/// dispatch chokepoint matches on an enum instead of re-deciding from strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEffect {
    /// No mutation; always allowed regardless of `writes` policy.
    Read,
    /// Convergent by construction — re-applying yields the same state (e.g.
    /// set-field, complete). Allowed when `writes: enabled`.
    Idempotent,
    /// Non-idempotent but dedupable via a minted idempotency key (increment 3).
    /// Allowed when `writes: enabled`.
    Keyed,
    /// Once-only external effect (send, create-without-key). Needs a writer/
    /// lease asymmetry (increment 4); blocked until then even when enabled.
    OnceOnly,
}

impl ToolEffect {
    /// The YAML spelling, for diagnostics.
    pub fn as_str(&self) -> &'static str {
        match self {
            ToolEffect::Read => "read",
            ToolEffect::Idempotent => "idempotent",
            ToolEffect::Keyed => "keyed",
            ToolEffect::OnceOnly => "once_only",
        }
    }
}

impl ToolConfig {
    /// A tool is write-shaped if it declares mutation metadata. Such a tool
    /// MUST carry an explicit `effect` — see [`McpSidecar::from_yaml`]
    /// validation.
    fn is_write_shaped(&self) -> bool {
        self.affected_fields.is_some() || self.undo.is_some()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum UndoConfig {
    Irreversible { reversible: bool },
    Mirror { tool: String, capture: Vec<String> },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TriggerConfig {
    pub from: String,
    pub provides: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ParamOverride {
    /// e.g. "entity_id:project" → TypeHint::EntityId { entity_name:
    /// "todoist_projects" }
    pub type_hint: Option<String>,
}

/// A Rhai expression validated at parse time. Guarantees the expression
/// compiles.
#[derive(Debug, Clone)]
pub struct RhaiPrecondition(String);

impl RhaiPrecondition {
    pub fn parse(expr: &str) -> Result<Self, String> {
        let engine = rhai::Engine::new();
        engine
            .compile_expression(expr)
            .map_err(|e| format!("invalid Rhai precondition '{expr}': {e}"))?;
        Ok(Self(expr.to_string()))
    }

    pub fn to_checker(&self) -> Arc<Box<PreconditionChecker>> {
        let expr = self.0.clone();
        Arc::new(Box::new(move |fields| {
            let engine = rhai::Engine::new();
            let mut scope = rhai::Scope::new();
            for (k, v) in fields {
                if let Some(b) = v.downcast_ref::<bool>() {
                    scope.push(k.clone(), *b);
                } else if let Some(s) = v.downcast_ref::<String>() {
                    scope.push(k.clone(), s.clone());
                } else if let Some(n) = v.downcast_ref::<f64>() {
                    scope.push(k.clone(), *n);
                } else if let Some(n) = v.downcast_ref::<i64>() {
                    scope.push(k.clone(), *n);
                }
            }
            let ast = engine
                .compile_expression(&expr)
                .map_err(|e| format!("Rhai compile error: {e}"))?;
            engine
                .eval_ast::<bool>(&ast)
                .map_err(|e| format!("Rhai eval error for '{expr}': {e}"))
        }))
    }
}

impl Serialize for RhaiPrecondition {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RhaiPrecondition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        RhaiPrecondition::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl EntityConfig {
    /// Convert the YAML-declared schema fields into a `TypeDefinition`.
    ///
    /// Sets `graph_label` (PascalCase of table name) and `primary_key` so
    /// `GraphSchemaRegistry` can build GQL schema directly from the result.
    /// Returns `None` if no schema fields are declared.
    pub fn to_type_definition(&self, table_name: &str) -> Option<TypeDefinition> {
        if self.schema.is_empty() {
            return None;
        }
        let mut td = TypeDefinition::new(table_name, self.schema.clone());
        td.graph_label = Some(pascal_case(table_name));
        td.primary_key = self.id_column_or_default();
        td.profile_variants = self.profile_variants.clone();
        Some(td)
    }
}

/// Convert a snake_case or kebab-case name to PascalCase.
fn pascal_case(s: &str) -> String {
    s.split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut out = first.to_uppercase().to_string();
                    out.extend(chars);
                    out
                }
            }
        })
        .collect()
}

impl McpSidecar {
    /// Return the primary key column name for an entity (defaults to "id").
    pub fn id_column(&self, entity_name: &str) -> String {
        self.entities
            .get(entity_name)
            .map(|c| c.id_column_or_default())
            .unwrap_or_else(|| "id".to_string())
    }

    /// Return the prefixed `EntityName` for an entity.
    ///
    /// E.g. prefix `"cc_"` + entity `"session"` → `EntityName("cc-session")`.
    /// Use `.as_str()` for URI schemes, `.table_name()` for SQL identifiers.
    pub fn prefixed_name(&self, entity_name: &str) -> holon_api::EntityName {
        let raw = match &self.entity_prefix {
            Some(prefix) => format!("{prefix}{entity_name}"),
            None => entity_name.to_string(),
        };
        holon_api::EntityName::new(raw)
    }

    /// Find a YAML entity key that maps to the given MCP server entity name.
    ///
    /// Checks (in order):
    /// 1. Direct key match (entity key == source name)
    /// 2. Per-entity `source_name` override
    pub fn find_key_by_source_name(&self, source: &str) -> Option<&str> {
        self.entities
            .iter()
            .find(|(_, config)| config.source_name.as_deref() == Some(source))
            .map(|(k, _)| k.as_str())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_yaml(&content)
    }

    pub fn from_yaml(yaml: &str) -> anyhow::Result<Self> {
        let sidecar: McpSidecar = serde_yaml::from_str(yaml)?;
        sidecar.validate_write_policy()?;
        Ok(sidecar)
    }

    /// Fail loud at load if the write policy is under-specified — the same
    /// discipline as a failing `views:` reconcile. A write-shaped tool with no
    /// `effect`, or a `keyed` tool with no `key_param`, is a config error, not
    /// a silently-defaulted setting (parse, don't validate).
    fn validate_write_policy(&self) -> anyhow::Result<()> {
        for (name, tool) in &self.tools {
            match tool.effect {
                None if tool.is_write_shaped() => anyhow::bail!(
                    "sidecar tool '{name}' declares write metadata (affected_fields/undo) but no \
                     `effect:` — classify it as read|idempotent|keyed|once_only"
                ),
                Some(ToolEffect::Keyed) if tool.key_param.is_none() => anyhow::bail!(
                    "sidecar tool '{name}' is `effect: keyed` but declares no `key_param:` — name \
                     the tool argument that carries the idempotency key"
                ),
                _ => {}
            }
            if let Some(outcome) = &tool.outcome {
                if outcome.proven.is_empty() {
                    anyhow::bail!(
                        "sidecar tool '{name}' declares `outcome:` with no `proven:` values — \
                         nothing could ever count as delivery"
                    );
                }
                if let Some(clash) = outcome.proven.iter().find(|p| outcome.unproven.contains(p)) {
                    anyhow::bail!(
                        "sidecar tool '{name}' lists outcome value '{clash}' as BOTH proven and \
                         unproven"
                    );
                }
            }
        }
        Ok(())
    }

    pub fn default_entity(&self) -> &str {
        self.entities
            .keys()
            .next()
            .map(|s| s.as_str())
            .expect("sidecar must have at least one entity")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome_sidecar(extra: &str) -> anyhow::Result<McpSidecar> {
        McpSidecar::from_yaml(&format!(
            r#"
entities:
  session:
    id_column: id
tools:
  send_message:
    entity: session
    effect: once_only
    undo:
      reversible: false
    outcome:
      field: outcome
{extra}
"#
        ))
    }

    fn verdict(response: serde_json::Value) -> AckVerdict {
        let sidecar = outcome_sidecar("      proven: [delivered]\n      unproven: [unconfirmed]")
            .expect("valid outcome declaration");
        sidecar.tools["send_message"]
            .outcome
            .as_ref()
            .expect("declared")
            .classify("send_message", &holon_api::Value::from(response))
    }

    #[test]
    fn a_proven_outcome_value_is_proof_of_delivery() {
        assert_eq!(
            verdict(serde_json::json!({"outcome": "delivered"})),
            AckVerdict::Proven
        );
    }

    /// The headline: a SUCCESSFUL response the provider labels unproven must
    /// not be read as delivery, and the disclosure must quote its word.
    #[test]
    fn a_declared_unproven_outcome_is_not_proof_of_delivery() {
        let AckVerdict::Unproven { detail } =
            verdict(serde_json::json!({"outcome": "unconfirmed"}))
        else {
            panic!("`unconfirmed` must not classify as proven")
        };
        assert!(detail.contains("unconfirmed"), "got: {detail}");
    }

    /// An outcome word the declaration does not know is disclosed as unproven,
    /// never silently defaulted into success.
    #[test]
    fn an_unknown_outcome_value_is_not_proof_of_delivery() {
        let AckVerdict::Unproven { detail } = verdict(serde_json::json!({"outcome": "who_knows"}))
        else {
            panic!("an unknown outcome word must not classify as proven")
        };
        assert!(detail.contains("who_knows"), "got: {detail}");
    }

    /// A response missing the declared field means the sidecar and the provider
    /// disagree — that is a reason to doubt delivery, not to assume it.
    #[test]
    fn a_missing_outcome_field_is_not_proof_of_delivery() {
        let AckVerdict::Unproven { detail } = verdict(serde_json::json!({"ok": true})) else {
            panic!("a missing outcome field must not classify as proven")
        };
        assert!(detail.contains("outcome"), "got: {detail}");
    }

    #[test]
    fn an_outcome_declaration_with_no_proven_values_is_a_loud_config_error() {
        let err = outcome_sidecar("      proven: []").unwrap_err().to_string();
        assert!(err.contains("no `proven:` values"), "got: {err}");
    }

    #[test]
    fn an_outcome_value_that_is_both_proven_and_unproven_is_a_loud_config_error() {
        let err = outcome_sidecar("      proven: [delivered]\n      unproven: [delivered]")
            .unwrap_err()
            .to_string();
        assert!(err.contains("BOTH proven and unproven"), "got: {err}");
    }

    #[test]
    fn parse_valid_precondition() {
        let p = RhaiPrecondition::parse("completed == false").unwrap();
        assert_eq!(p.0, "completed == false");
    }

    #[test]
    fn parse_invalid_precondition() {
        let err = RhaiPrecondition::parse("if {{{").unwrap_err();
        assert!(err.contains("invalid Rhai precondition"));
    }

    #[test]
    fn deserialize_sidecar_yaml() {
        let yaml = r#"
entities:
  todoist_tasks:
    short_name: task
    id_column: id

tools:
  complete-tasks:
    entity: todoist_tasks
    affected_fields: [completed]
    precondition: "completed == false"
  update-tasks:
    entity: todoist_tasks
    affected_fields: [content, description]
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(sidecar.entities.len(), 1);
        assert_eq!(sidecar.tools.len(), 2);
        assert!(sidecar.tools["complete-tasks"].precondition.is_some());
    }

    #[test]
    fn deserialize_undo_config() {
        let yaml = r#"
entities:
  todoist_tasks:
    short_name: task
    id_column: id

tools:
  update-tasks:
    entity: todoist_tasks
    affected_fields: [content, description]
    undo:
      tool: update-tasks
      capture: [content, description]
  complete-tasks:
    entity: todoist_tasks
    undo:
      reversible: false
  find-tasks:
    entity: todoist_tasks
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();

        let update = &sidecar.tools["update-tasks"];
        match update.undo.as_ref().unwrap() {
            UndoConfig::Mirror { tool, capture } => {
                assert_eq!(tool, "update-tasks");
                assert_eq!(capture, &["content", "description"]);
            }
            other => panic!("expected Mirror, got {:?}", other),
        }

        let complete = &sidecar.tools["complete-tasks"];
        match complete.undo.as_ref().unwrap() {
            UndoConfig::Irreversible { reversible } => assert!(!reversible),
            other => panic!("expected Irreversible, got {:?}", other),
        }

        assert!(sidecar.tools["find-tasks"].undo.is_none());
    }

    #[test]
    fn invalid_precondition_fails_deserialization() {
        let yaml = r#"
entities:
  todoist_tasks:
    short_name: task
    id_column: id
tools:
  bad-op:
    precondition: "if {{{"
"#;
        let result: Result<McpSidecar, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_resource_sync_config() {
        let yaml = r#"
entities:
  session:
    short_name: session
    id_column: id
    sync:
      list_resource: "claude-history://projects/{project_id}/sessions"
      uri_params:
        project_id: "-Users-martin-Workspaces-pkm-holon"
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        let sync = sidecar.entities["session"].sync.as_ref().unwrap();
        assert!(sync.list_tool.is_none());
        assert_eq!(
            sync.list_resource.as_deref(),
            Some("claude-history://projects/{project_id}/sessions")
        );
        assert_eq!(
            sync.uri_params["project_id"],
            "-Users-martin-Workspaces-pkm-holon"
        );

        let strategy = sync.into_strategy().unwrap();
        assert_eq!(
            strategy.subscribe_uri(),
            Some("claude-history://projects/-Users-martin-Workspaces-pkm-holon/sessions")
        );
    }

    #[test]
    fn deserialize_tool_sync_config() {
        let yaml = r#"
entities:
  task:
    short_name: task
    id_column: id
    sync:
      list_tool: get-tasks
      extract_path: tasks
      list_params:
        project_id: "123"
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        let sync = sidecar.entities["task"].sync.as_ref().unwrap();
        assert_eq!(sync.list_tool.as_deref(), Some("get-tasks"));
        assert_eq!(sync.extract_path.as_deref(), Some("tasks"));
        assert!(sync.list_resource.is_none());

        let strategy = sync.into_strategy().unwrap();
        assert!(strategy.subscribe_uri().is_none());
    }

    #[test]
    fn sync_config_neither_tool_nor_resource_fails() {
        let yaml = r#"
entities:
  bad:
    short_name: bad
    id_column: id
    sync:
      list_params: {}
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        let sync = sidecar.entities["bad"].sync.as_ref().unwrap();
        let result = sync.into_strategy();
        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("list_tool or list_resource")
        );
    }

    #[test]
    fn minimal_sidecar_entry_defaults() {
        let yaml = r#"
entities:
  session:
    sync:
      list_resource: "history://sessions"
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        let entity = &sidecar.entities["session"];
        assert_eq!(entity.short_name_or("session"), "session");
        assert_eq!(entity.id_column_or_default(), "id");
        assert!(entity.schema.is_empty());
        assert!(entity.sync.is_some());
    }

    #[test]
    fn parse_sync_interval_forms() {
        let yaml = r#"
entities:
  session:
    sync:
      list_resource: "history://sessions"
      interval: 60
  message:
    sync:
      list_resource: "history://messages"
      interval: "5m"
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        let session = sidecar.entities["session"].sync.as_ref().unwrap();
        assert_eq!(
            session.interval.unwrap().duration(),
            std::time::Duration::from_secs(60)
        );
        let message = sidecar.entities["message"].sync.as_ref().unwrap();
        assert_eq!(
            message.interval.unwrap().duration(),
            std::time::Duration::from_secs(300)
        );
    }

    #[test]
    fn parse_sync_interval_absent_is_none() {
        let yaml = r#"
entities:
  session:
    sync:
      list_resource: "history://sessions"
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        assert!(
            sidecar.entities["session"]
                .sync
                .as_ref()
                .unwrap()
                .interval
                .is_none()
        );
    }

    #[test]
    fn invalid_sync_interval_fails_deserialization() {
        for bad in ["\"5 fortnights\"", "\"abc\"", "0", "\"0s\""] {
            let yaml = format!(
                r#"
entities:
  session:
    sync:
      list_resource: "history://sessions"
      interval: {bad}
"#
            );
            let result: Result<McpSidecar, _> = serde_yaml::from_str(&yaml);
            assert!(result.is_err(), "interval {bad} should fail to parse");
        }
    }

    #[test]
    fn parse_views_section() {
        let yaml = r#"
entity_prefix: "cc_"
entities:
  session:
    sync:
      list_resource: "history://sessions"
views:
  - name: session_status
    sql: "SELECT session_id, MAX(ts) AS last_ts FROM cc_message GROUP BY session_id"
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(sidecar.views.len(), 1);
        assert_eq!(sidecar.views[0].name, "session_status");
        assert!(sidecar.views[0].sql.starts_with("SELECT session_id"));
        // entity_prefix applies to view names the same way it does to tables
        assert_eq!(
            sidecar.prefixed_name(&sidecar.views[0].name).table_name(),
            "cc_session_status"
        );
    }

    #[test]
    fn views_absent_is_empty() {
        let yaml = r#"
entities:
  session:
    sync:
      list_resource: "history://sessions"
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        assert!(sidecar.views.is_empty());
    }

    #[test]
    fn pascal_case_conversion() {
        assert_eq!(super::pascal_case("todoist_task"), "TodoistTask");
        assert_eq!(super::pascal_case("my-entity"), "MyEntity");
        assert_eq!(super::pascal_case("simple"), "Simple");
        assert_eq!(super::pascal_case("a_b_c"), "ABC");
    }

    #[test]
    fn to_type_definition_sets_graph_label_and_primary_key() {
        let yaml = r#"
entities:
  email:
    short_name: mail
    id_column: msg_id
    schema:
      - name: msg_id
        sql_type: TEXT
        primary_key: true
        indexed: true
      - name: subject
        sql_type: TEXT
      - name: read
        sql_type: INTEGER
        nullable: true
"#;
        let sidecar: McpSidecar = serde_yaml::from_str(yaml).unwrap();
        let td = sidecar.entities["email"]
            .to_type_definition("email")
            .expect("should produce TypeDefinition");
        assert_eq!(td.name, "email");
        assert_eq!(td.primary_key, "msg_id");
        assert_eq!(td.graph_label.as_deref(), Some("Email"));
        assert_eq!(td.fields.len(), 3);

        // Verify fields carry through correctly
        assert_eq!(td.fields[0].name, "msg_id");
        assert!(!td.fields[0].nullable);
        assert!(td.fields[2].nullable);
    }

    #[test]
    fn writes_policy_defaults_to_disabled() {
        let yaml = "entities:\n  x:\n    short_name: x\n";
        let sidecar = McpSidecar::from_yaml(yaml).unwrap();
        assert_eq!(sidecar.writes, WritesPolicy::Disabled);
    }

    #[test]
    fn parses_writes_and_effect() {
        let yaml = r#"
writes: enabled
entities:
  x:
    short_name: x
tools:
  set-thing:
    entity: x
    effect: idempotent
    affected_fields: [thing]
  send-thing:
    entity: x
    effect: keyed
    key_param: idempotency_key
"#;
        let sidecar = McpSidecar::from_yaml(yaml).unwrap();
        assert_eq!(sidecar.writes, WritesPolicy::Enabled);
        assert_eq!(
            sidecar.tools["set-thing"].effect,
            Some(ToolEffect::Idempotent)
        );
        assert_eq!(sidecar.tools["send-thing"].effect, Some(ToolEffect::Keyed));
    }

    #[test]
    fn write_shaped_tool_without_effect_fails_loud() {
        let yaml = r#"
entities:
  x:
    short_name: x
tools:
  set-thing:
    entity: x
    affected_fields: [thing]
"#;
        let err = McpSidecar::from_yaml(yaml).unwrap_err().to_string();
        assert!(
            err.contains("set-thing") && err.contains("effect"),
            "config error must name the tool and demand an effect, got: {err}"
        );
    }

    #[test]
    fn keyed_tool_without_key_param_fails_loud() {
        let yaml = r#"
entities:
  x:
    short_name: x
tools:
  send-thing:
    entity: x
    effect: keyed
"#;
        let err = McpSidecar::from_yaml(yaml).unwrap_err().to_string();
        assert!(
            err.contains("send-thing") && err.contains("key_param"),
            "config error must demand a key_param, got: {err}"
        );
    }
}
