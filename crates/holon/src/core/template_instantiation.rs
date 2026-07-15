//! Template instantiation planning (docs/Proposals/Templating-2026-07-12.md).
//!
//! A template is an ordinary block subtree whose root carries a `template`
//! marker property and a `template_vars` declaration. Instantiation is a pure
//! *plan*: given the subtree and an [`InstantiateRequest`], produce the ordered
//! `create` param maps (parent before child, deterministic ids per ADR 0024
//! P4). All binding/variable failures are loud errors — a missing binding is
//! never an empty substitution.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use holon_api::MarkSpan;
use holon_api::Value;
use holon_api::effect_id::deterministic_instance_id;
use holon_api::marks_from_json;
use holon_api::marks_to_json;
use holon_core::storage::types::StorageEntity;

/// Property marking a block as a template root (value = human-readable name).
pub const TEMPLATE_MARKER_PROPERTY: &str = "template";
/// Property declaring the template's variables: `name` or `name=default`,
/// comma-separated.
pub const TEMPLATE_VARS_PROPERTY: &str = "template_vars";
/// Provenance property stamped on the instance root, pointing at the template.
pub const INSTANCE_OF_PROPERTY: &str = "instance_of";

/// One declared template variable: a name and an optional default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateVarDecl {
    pub name: String,
    pub default: Option<String>,
}

/// The parsed `template_vars` declaration (parse-don't-validate boundary).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TemplateVars(Vec<TemplateVarDecl>);

impl TemplateVars {
    /// Parse a `template_vars` property value: comma-separated `name` or
    /// `name=default` entries. Empty string → no variables. Malformed names
    /// and duplicates are loud errors.
    pub fn parse(raw: &str) -> Result<Self> {
        let mut decls: Vec<TemplateVarDecl> = Vec::new();
        for entry in raw.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            let (name, default) = match entry.split_once('=') {
                Some((n, d)) => (n.trim(), Some(d.trim().to_string())),
                None => (entry, None),
            };
            if !is_valid_var_name(name) {
                bail!(
                    "template_vars entry '{entry}': variable name '{name}' is invalid (expected \
                     [A-Za-z_][A-Za-z0-9_-]*)"
                );
            }
            if decls.iter().any(|d| d.name == name) {
                bail!("template_vars declares variable '{name}' more than once");
            }
            decls.push(TemplateVarDecl {
                name: name.to_string(),
                default,
            });
        }
        Ok(Self(decls))
    }

    pub fn declares(&self, name: &str) -> bool {
        self.0.iter().any(|d| d.name == name)
    }

    fn default_of(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|d| d.name == name)
            .and_then(|d| d.default.as_deref())
    }
}

fn is_valid_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The typed `instantiate_template` request, parsed from raw operation params
/// at the engine boundary. Every field is required except `bindings`.
#[derive(Clone, Debug)]
pub struct InstantiateRequest {
    pub template_id: String,
    pub target_parent: String,
    /// Plays the role of ADR 0024's firing key: rules pass their binding so
    /// re-fires converge; manual callers pass a fresh key per instantiation.
    pub context_key: String,
    pub bindings: BTreeMap<String, String>,
}

impl InstantiateRequest {
    pub fn from_params(params: &StorageEntity) -> Result<Self> {
        let required = |key: &str| -> Result<String> {
            match params.get(key) {
                Some(Value::String(s)) if !s.is_empty() => Ok(s.clone()),
                Some(other) => bail!(
                    "instantiate_template: param '{key}' must be a non-empty string, got {other:?}"
                ),
                None => bail!("instantiate_template: missing required param '{key}'"),
            }
        };
        let template_id = required("template_id")?;
        let target_parent = required("target_parent")?;
        let context_key = required("context_key")?;

        let mut bindings = BTreeMap::new();
        match params.get("bindings") {
            None | Some(Value::Null) => {}
            Some(Value::Object(map)) => {
                for (k, v) in map {
                    let rendered = match v {
                        Value::String(s) => s.clone(),
                        Value::Integer(i) => i.to_string(),
                        Value::Float(f) => f.to_string(),
                        Value::Boolean(b) => b.to_string(),
                        Value::DateTime(s) => s.clone(),
                        other => bail!(
                            "instantiate_template: binding '{k}' must be a scalar, got {other:?}"
                        ),
                    };
                    bindings.insert(k.clone(), rendered);
                }
            }
            Some(other) => bail!(
                "instantiate_template: param 'bindings' must be an object of scalar values, got \
                 {other:?}"
            ),
        }
        Ok(Self {
            template_id,
            target_parent,
            context_key,
            bindings,
        })
    }
}

/// One node of a loaded template subtree, in the shape the planner consumes.
/// Produced by a
/// [`TemplateSource`](crate::api::operation_engine::TemplateSource)
/// implementation from its storage rows.
#[derive(Clone, Debug, Default)]
pub struct TemplateNode {
    pub id: String,
    pub parent_id: String,
    pub content: String,
    pub content_type: String,
    pub block_type: String,
    pub sort_key: String,
    pub collapsed: bool,
    pub completed: bool,
    pub source_language: Option<String>,
    pub source_name: Option<String>,
    /// The row's `properties` JSON (an object), if any.
    pub properties: Option<String>,
    /// The row's `marks` JSON (a `Vec<MarkSpan>` array), if any.
    pub marks: Option<String>,
}

/// The computed instantiation: ordered (parent-before-child) `create` param
/// maps plus the instance root id.
#[derive(Clone, Debug)]
pub struct InstantiationPlan {
    pub root_id: String,
    pub creates: Vec<StorageEntity>,
}

/// Build the instantiation plan. `nodes` must be the template subtree in
/// parent-before-child order with the template root first (siblings in
/// `sort_key` order so copied children keep their relative order).
pub fn plan_instantiation(
    nodes: &[TemplateNode],
    request: &InstantiateRequest,
) -> Result<InstantiationPlan> {
    let root = nodes
        .first()
        .with_context(|| format!("template '{}' has no blocks", request.template_id))?;
    if root.id != request.template_id {
        bail!(
            "template subtree root '{}' does not match requested template_id '{}'",
            root.id,
            request.template_id
        );
    }
    let root_props = parse_properties(root)?;
    if template_marker_value(&root_props, TEMPLATE_MARKER_PROPERTY).is_none() {
        bail!(
            "block '{}' is not a template: missing the '{TEMPLATE_MARKER_PROPERTY}' property",
            root.id
        );
    }
    let vars = match template_marker_value(&root_props, TEMPLATE_VARS_PROPERTY) {
        Some(serde_json::Value::String(raw)) => TemplateVars::parse(raw)
            .with_context(|| format!("template '{}': invalid template_vars", root.id))?,
        Some(other) => bail!(
            "template '{}': '{TEMPLATE_VARS_PROPERTY}' must be a string, got {other}",
            root.id
        ),
        None => TemplateVars::default(),
    };

    // Reject typo'd bindings loudly: a binding for an undeclared variable is
    // caller error, not something to silently ignore.
    for name in request.bindings.keys() {
        if !vars.declares(name) {
            bail!(
                "instantiate_template: binding '{name}' is not declared by template '{}' \
                 (declared: {:?})",
                request.template_id,
                vars.0.iter().map(|d| d.name.as_str()).collect::<Vec<_>>()
            );
        }
    }

    // Effective substitution map: defaults overlaid by bindings. Variables
    // with neither stay absent — referencing one is the missing-binding error.
    let mut effective: BTreeMap<String, String> = BTreeMap::new();
    for decl in &vars.0 {
        if let Some(default) = &decl.default {
            effective.insert(decl.name.clone(), default.clone());
        }
    }
    for (k, v) in &request.bindings {
        effective.insert(k.clone(), v.clone());
    }

    // First pass: collect every referenced variable across the subtree so ALL
    // missing bindings are reported at once, before anything is created.
    let mut missing: Vec<String> = Vec::new();
    let mut check_text = |text: &str| -> Result<()> {
        for name in referenced_vars(text)? {
            if !vars.declares(&name) {
                bail!(
                    "template '{}' references undeclared variable '{{{{{name}}}}}' (declare it in \
                     '{TEMPLATE_VARS_PROPERTY}')",
                    request.template_id
                );
            }
            if !effective.contains_key(&name) && !missing.contains(&name) {
                missing.push(name);
            }
        }
        Ok(())
    };
    for node in nodes {
        check_text(&node.content)?;
        for value in parse_properties(node)?.values() {
            if let serde_json::Value::String(s) = value {
                check_text(s)?;
            }
        }
    }
    if !missing.is_empty() {
        missing.sort();
        bail!(
            "instantiate_template: missing bindings for template '{}' variables {missing:?} (no \
             defaults declared)",
            request.template_id
        );
    }

    // Second pass: mint ids, substitute, and build the create params.
    let mut id_map: BTreeMap<&str, String> = BTreeMap::new();
    for node in nodes {
        let new_id =
            deterministic_instance_id(&request.template_id, &request.context_key, &node.id);
        id_map.insert(node.id.as_str(), new_id.as_str().to_string());
    }

    let mut creates = Vec::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        let is_root = index == 0;
        let new_parent = if is_root {
            request.target_parent.clone()
        } else {
            id_map
                .get(node.parent_id.as_str())
                .with_context(|| {
                    format!(
                        "template node '{}' has parent '{}' outside the subtree (nodes must be \
                         parent-before-child)",
                        node.id, node.parent_id
                    )
                })?
                .clone()
        };

        let (content, replacements) = substitute(&node.content, &effective)?;
        let marks = match &node.marks {
            Some(json) if !json.is_empty() => {
                let spans = marks_from_json(json)
                    .with_context(|| format!("template node '{}': invalid marks JSON", node.id))?;
                let remapped = remap_marks(&spans, &replacements);
                Some(marks_to_json(&remapped))
            }
            _ => None,
        };

        let mut props = parse_properties(node)?;
        // Never inherit the template's own authoring stamp — every created
        // node gets a fresh C2a `_provenance` from the executing engine.
        props.remove(holon_api::PROVENANCE_PROPERTY);
        for value in props.values_mut() {
            if let serde_json::Value::String(s) = value {
                let (substituted, _) = substitute(s, &effective)?;
                *value = serde_json::Value::String(substituted);
            }
        }
        if is_root {
            remove_template_marker(&mut props, TEMPLATE_MARKER_PROPERTY);
            remove_template_marker(&mut props, TEMPLATE_VARS_PROPERTY);
            props.insert(
                INSTANCE_OF_PROPERTY.to_string(),
                serde_json::Value::String(request.template_id.clone()),
            );
        }

        let mut params: StorageEntity = StorageEntity::default();
        let mut put = |k: &str, v: Value| {
            params.insert(Arc::from(k), v);
        };
        put("id", Value::String(id_map[node.id.as_str()].clone()));
        put("parent_id", Value::String(new_parent));
        put("content", Value::String(content));
        put("content_type", Value::String(node.content_type.clone()));
        put("block_type", Value::String(node.block_type.clone()));
        put("collapsed", Value::Boolean(node.collapsed));
        put("completed", Value::Boolean(node.completed));
        if !is_root {
            // Copying the template's fractional keys preserves sibling order
            // inside the instance. The root's key is left to the provider —
            // it lands among existing siblings of `target_parent` like any
            // rule-created block.
            put("sort_key", Value::String(node.sort_key.clone()));
        }
        if let Some(lang) = &node.source_language {
            put("source_language", Value::String(lang.clone()));
        }
        if let Some(name) = &node.source_name {
            put("source_name", Value::String(name.clone()));
        }
        if let Some(marks_json) = marks {
            put("marks", Value::String(marks_json));
        }
        if !props.is_empty() {
            let json =
                serde_json::to_string(&serde_json::Value::Object(props.into_iter().collect()))
                    .expect("property map serialization is total");
            put("properties", Value::String(json));
        }
        creates.push(params);
    }

    Ok(InstantiationPlan {
        root_id: id_map[root.id.as_str()].clone(),
        creates,
    })
}

fn parse_properties(node: &TemplateNode) -> Result<BTreeMap<String, serde_json::Value>> {
    match &node.properties {
        None => Ok(BTreeMap::new()),
        Some(raw) if raw.trim().is_empty() => Ok(BTreeMap::new()),
        Some(raw) => {
            let parsed: serde_json::Value = serde_json::from_str(raw).with_context(|| {
                format!("template node '{}': properties is not valid JSON", node.id)
            })?;
            match parsed {
                serde_json::Value::Object(map) => Ok(map.into_iter().collect()),
                other => bail!(
                    "template node '{}': properties must be a JSON object, got {other}",
                    node.id
                ),
            }
        }
    }
}

/// Case-insensitive lookup for template-marker keys. The org parser
/// writes `:TEMPLATE:` / `:TEMPLATE_VARS:` drawer properties with the
/// exact casing from the org file; direct programmatic creation uses
/// lowercase. Both must resolve.
fn template_marker_key<'a>(
    props: &'a BTreeMap<String, serde_json::Value>,
    marker: &str,
) -> Option<&'a String> {
    props.keys().find(|k| k.eq_ignore_ascii_case(marker))
}

fn template_marker_value<'a>(
    props: &'a BTreeMap<String, serde_json::Value>,
    marker: &str,
) -> Option<&'a serde_json::Value> {
    template_marker_key(props, marker).and_then(|k| props.get(k))
}

/// Remove a template-marker key case-insensitively, returning the
/// actual key that was removed (if any).
fn remove_template_marker(
    props: &mut BTreeMap<String, serde_json::Value>,
    marker: &str,
) -> Option<String> {
    let key = template_marker_key(props, marker).cloned();
    if let Some(ref k) = key {
        props.remove(k);
    }
    key
}

/// A single `{{var}}` → value replacement inside one content string, in
/// Unicode-scalar offsets of the ORIGINAL string (for mark remapping).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Replacement {
    start: usize,
    old_len: usize,
    new_len: usize,
}

/// Scan `text` for `{{name}}` slots and return the referenced names. A `{{`
/// without a matching `}}` or with an invalid name is a loud error — a
/// half-written slot silently passing through would be the "looks fine"
/// failure mode this design forbids.
fn referenced_vars(text: &str) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let close = after
            .find("}}")
            .with_context(|| format!("unterminated '{{{{' template slot in: {text:?}"))?;
        let name = after[..close].trim();
        if !is_valid_var_name(name) {
            bail!("invalid template variable name '{name}' in: {text:?}");
        }
        names.push(name.to_string());
        rest = &after[close + 2..];
    }
    Ok(names)
}

/// Substitute every `{{name}}` slot from `values`, returning the new string
/// and the replacement records (original-string Unicode-scalar offsets).
/// Callers guarantee every referenced name is present (`plan_instantiation`
/// pre-checks); a miss here is a programming error and fails loud.
fn substitute(text: &str, values: &BTreeMap<String, String>) -> Result<(String, Vec<Replacement>)> {
    let mut out = String::with_capacity(text.len());
    let mut replacements = Vec::new();
    let mut rest = text;
    let mut consumed_scalars = 0usize;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let close = after
            .find("}}")
            .with_context(|| format!("unterminated '{{{{' template slot in: {text:?}"))?;
        let raw_name = &after[..close];
        let name = raw_name.trim();
        let value = values.get(name).with_context(|| {
            format!("no binding for template variable '{name}' (pre-check missed it)")
        })?;

        out.push_str(&rest[..open]);
        out.push_str(value);

        let slot_scalars = 2 + raw_name.chars().count() + 2;
        replacements.push(Replacement {
            start: consumed_scalars + rest[..open].chars().count(),
            old_len: slot_scalars,
            new_len: value.chars().count(),
        });
        consumed_scalars += rest[..open].chars().count() + slot_scalars;
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    Ok((out, replacements))
}

/// Map mark spans across the substitutions. Offsets after a replaced slot
/// shift by the length delta; a span boundary strictly inside a slot snaps
/// outward (start → slot start, end → end of the substituted value), so a
/// mark covering a slot stretches over the substituted text.
fn remap_marks(spans: &[MarkSpan], replacements: &[Replacement]) -> Vec<MarkSpan> {
    let map_offset = |offset: usize, is_end: bool| -> usize {
        let mut delta: i64 = 0;
        for r in replacements {
            if offset <= r.start {
                break;
            }
            if offset >= r.start + r.old_len {
                delta += r.new_len as i64 - r.old_len as i64;
            } else {
                // Inside the slot: snap to the substituted value's boundary.
                let base = (r.start as i64 + delta) as usize;
                return if is_end { base + r.new_len } else { base };
            }
        }
        (offset as i64 + delta) as usize
    };
    spans
        .iter()
        .map(|s| MarkSpan {
            start: map_offset(s.start, false),
            end: map_offset(s.end, true),
            mark: s.mark.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use holon_api::InlineMark;

    use super::*;

    fn node(id: &str, parent: &str, content: &str) -> TemplateNode {
        TemplateNode {
            id: id.to_string(),
            parent_id: parent.to_string(),
            content: content.to_string(),
            content_type: "text".to_string(),
            block_type: "text".to_string(),
            sort_key: "A0".to_string(),
            ..TemplateNode::default()
        }
    }

    fn template_root(id: &str, content: &str, vars: &str) -> TemplateNode {
        let mut n = node(id, "block:somewhere", content);
        n.properties = Some(format!(r#"{{"template":"t","template_vars":"{vars}"}}"#));
        n
    }

    fn request(bindings: &[(&str, &str)]) -> InstantiateRequest {
        InstantiateRequest {
            template_id: "block:tpl".to_string(),
            target_parent: "block:target".to_string(),
            context_key: "2026-07-12".to_string(),
            bindings: bindings
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn get_str<'a>(params: &'a StorageEntity, key: &str) -> &'a str {
        match params.get(key) {
            Some(Value::String(s)) => s,
            other => panic!("param '{key}' expected string, got {other:?}"),
        }
    }

    #[test]
    fn template_vars_parse_names_and_defaults() {
        let vars = TemplateVars::parse("date, mood=neutral").unwrap();
        assert!(vars.declares("date"));
        assert_eq!(vars.default_of("mood"), Some("neutral"));
        assert_eq!(vars.default_of("date"), None);
        assert!(TemplateVars::parse("date, date").is_err(), "duplicate");
        assert!(TemplateVars::parse("1bad").is_err(), "invalid name");
    }

    #[test]
    fn plan_substitutes_content_and_properties_with_bindings_and_defaults() {
        let mut root = template_root("block:tpl", "{{date}}", "date, mood=neutral");
        root.properties = Some(
            r#"{"template":"t","template_vars":"date, mood=neutral","note":"feeling {{mood}}"}"#
                .to_string(),
        );
        let child = node("block:c1", "block:tpl", "Mood check: {{mood}} on {{date}}");
        let plan = plan_instantiation(&[root, child], &request(&[("date", "2026-07-12")])).unwrap();

        assert_eq!(plan.creates.len(), 2);
        let root_params = &plan.creates[0];
        assert_eq!(get_str(root_params, "content"), "2026-07-12");
        assert_eq!(get_str(root_params, "parent_id"), "block:target");
        let props: serde_json::Value =
            serde_json::from_str(get_str(root_params, "properties")).unwrap();
        assert_eq!(props["note"], "feeling neutral");
        assert_eq!(props["instance_of"], "block:tpl");
        assert!(props.get("template").is_none(), "marker stripped");
        assert!(props.get("template_vars").is_none(), "marker stripped");

        let child_params = &plan.creates[1];
        assert_eq!(
            get_str(child_params, "content"),
            "Mood check: neutral on 2026-07-12"
        );
        assert_eq!(
            get_str(child_params, "parent_id"),
            get_str(root_params, "id"),
            "child hangs under the new instance root"
        );
    }

    #[test]
    fn missing_binding_fails_loud_listing_all() {
        let root = template_root("block:tpl", "{{date}} and {{title}}", "date, title");
        let err = plan_instantiation(&[root], &request(&[])).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("missing bindings"), "got: {msg}");
        assert!(msg.contains("date") && msg.contains("title"), "got: {msg}");
    }

    #[test]
    fn undeclared_variable_reference_fails_loud() {
        let root = template_root("block:tpl", "{{typo}}", "date");
        let err = plan_instantiation(&[root], &request(&[("date", "2026-07-12")])).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("undeclared variable") && msg.contains("typo"),
            "got: {msg}"
        );
    }

    #[test]
    fn unknown_binding_key_fails_loud() {
        let root = template_root("block:tpl", "{{date}}", "date");
        let err =
            plan_instantiation(&[root], &request(&[("date", "d"), ("nope", "x")])).unwrap_err();
        assert!(format!("{err:#}").contains("binding 'nope' is not declared"));
    }

    #[test]
    fn non_template_root_fails_loud() {
        let root = node("block:tpl", "block:p", "just a block");
        let err = plan_instantiation(&[root], &request(&[])).unwrap_err();
        assert!(format!("{err:#}").contains("is not a template"));
    }

    #[test]
    fn ids_are_deterministic_per_context_key_and_distinct_across_keys() {
        let mk = || {
            vec![
                template_root("block:tpl", "{{date}}", "date"),
                node("block:c1", "block:tpl", "child"),
            ]
        };
        let a = plan_instantiation(&mk(), &request(&[("date", "d")])).unwrap();
        let b = plan_instantiation(&mk(), &request(&[("date", "d")])).unwrap();
        assert_eq!(a.root_id, b.root_id, "same context key → same ids");
        assert_eq!(get_str(&a.creates[1], "id"), get_str(&b.creates[1], "id"));

        let mut other = request(&[("date", "d")]);
        other.context_key = "another-key".to_string();
        let c = plan_instantiation(&mk(), &other).unwrap();
        assert_ne!(a.root_id, c.root_id, "different context key → new instance");
    }

    #[test]
    fn marks_shift_and_stretch_across_substitution() {
        // content: "see {{date}} now", link mark covers the slot (4..12),
        // bold covers " now"'s 'n' (13..16 in the original).
        let mut root = template_root("block:tpl", "see {{date}} now", "date");
        let spans = vec![
            MarkSpan {
                start: 4,
                end: 12,
                mark: InlineMark::Link {
                    target: holon_api::EntityRef::Name {
                        name: "Journals".to_string(),
                    },
                    label: "date".to_string(),
                },
            },
            MarkSpan {
                start: 13,
                end: 16,
                mark: InlineMark::Bold,
            },
        ];
        root.marks = Some(marks_to_json(&spans));
        let plan = plan_instantiation(&[root], &request(&[("date", "2026-07-12")])).unwrap();
        let params = &plan.creates[0];
        assert_eq!(get_str(params, "content"), "see 2026-07-12 now");
        let remapped = marks_from_json(get_str(params, "marks")).unwrap();
        // "{{date}}" (8 scalars) → "2026-07-12" (10 scalars): link stretches
        // to 4..14, bold shifts +2 to 15..18.
        assert_eq!((remapped[0].start, remapped[0].end), (4, 14));
        assert_eq!((remapped[1].start, remapped[1].end), (15, 18));
    }

    #[test]
    fn unterminated_slot_fails_loud() {
        let root = template_root("block:tpl", "broken {{date", "date");
        let err = plan_instantiation(&[root], &request(&[("date", "d")])).unwrap_err();
        assert!(format!("{err:#}").contains("unterminated"));
    }

    // -- W1: InstantiateRequest::from_params boundary tests -----------------

    fn params(pairs: &[(&str, Value)]) -> StorageEntity {
        pairs
            .iter()
            .map(|(k, v)| (Arc::from(*k), v.clone()))
            .collect()
    }

    #[test]
    fn from_params_missing_required_template_id() {
        let p = params(&[
            ("target_parent", Value::String("block:parent".into())),
            ("context_key", Value::String("key1".into())),
        ]);
        let err = InstantiateRequest::from_params(&p).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("missing required param 'template_id'"),
            "got: {msg}"
        );
    }

    #[test]
    fn from_params_missing_target_parent() {
        let p = params(&[
            ("template_id", Value::String("block:tpl".into())),
            ("context_key", Value::String("key1".into())),
        ]);
        let err = InstantiateRequest::from_params(&p).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("missing required param 'target_parent'"),
            "got: {msg}"
        );
    }

    #[test]
    fn from_params_missing_context_key() {
        let p = params(&[
            ("template_id", Value::String("block:tpl".into())),
            ("target_parent", Value::String("block:parent".into())),
        ]);
        let err = InstantiateRequest::from_params(&p).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("missing required param 'context_key'"),
            "got: {msg}"
        );
    }

    #[test]
    fn from_params_empty_string_param_fails() {
        let p = params(&[
            ("template_id", Value::String("".into())),
            ("target_parent", Value::String("block:parent".into())),
            ("context_key", Value::String("key1".into())),
        ]);
        let err = InstantiateRequest::from_params(&p).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("must be a non-empty string"), "got: {msg}");
    }

    #[test]
    fn from_params_non_string_param_fails() {
        let p = params(&[
            ("template_id", Value::Integer(42)),
            ("target_parent", Value::String("block:parent".into())),
            ("context_key", Value::String("key1".into())),
        ]);
        let err = InstantiateRequest::from_params(&p).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("must be a non-empty string"), "got: {msg}");
    }

    #[test]
    fn from_params_scalar_bindings_rendered_to_strings() {
        let mut p = params(&[
            ("template_id", Value::String("block:tpl".into())),
            ("target_parent", Value::String("block:parent".into())),
            ("context_key", Value::String("key1".into())),
        ]);
        let mut bindings = std::collections::HashMap::new();
        bindings.insert("int_val".to_string(), Value::Integer(42));
        bindings.insert("float_val".to_string(), Value::Float(3.14));
        bindings.insert("bool_val".to_string(), Value::Boolean(true));
        bindings.insert(
            "dt_val".to_string(),
            Value::DateTime("2026-07-12T00:00:00Z".into()),
        );
        p.insert(Arc::from("bindings"), Value::Object(bindings));

        let req = InstantiateRequest::from_params(&p).unwrap();
        assert_eq!(req.bindings.get("int_val").unwrap(), "42");
        assert_eq!(req.bindings.get("float_val").unwrap(), "3.14");
        assert_eq!(req.bindings.get("bool_val").unwrap(), "true");
        assert_eq!(req.bindings.get("dt_val").unwrap(), "2026-07-12T00:00:00Z");
    }

    #[test]
    fn from_params_non_scalar_binding_fails() {
        let mut p = params(&[
            ("template_id", Value::String("block:tpl".into())),
            ("target_parent", Value::String("block:parent".into())),
            ("context_key", Value::String("key1".into())),
        ]);
        let mut bindings = std::collections::HashMap::new();
        bindings.insert("arr".to_string(), Value::Array(vec![Value::Integer(1)]));
        p.insert(Arc::from("bindings"), Value::Object(bindings));

        let err = InstantiateRequest::from_params(&p).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("must be a scalar"), "got: {msg}");
    }

    #[test]
    fn from_params_bindings_not_object_fails() {
        let mut p = params(&[
            ("template_id", Value::String("block:tpl".into())),
            ("target_parent", Value::String("block:parent".into())),
            ("context_key", Value::String("key1".into())),
        ]);
        p.insert(Arc::from("bindings"), Value::String("not-an-object".into()));

        let err = InstantiateRequest::from_params(&p).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("must be an object"), "got: {msg}");
    }

    #[test]
    fn from_params_null_bindings_tolerated() {
        // No bindings key at all.
        let p = params(&[
            ("template_id", Value::String("block:tpl".into())),
            ("target_parent", Value::String("block:parent".into())),
            ("context_key", Value::String("key1".into())),
        ]);
        let req = InstantiateRequest::from_params(&p).unwrap();
        assert!(req.bindings.is_empty());

        // Explicit null.
        let mut p2 = params(&[
            ("template_id", Value::String("block:tpl".into())),
            ("target_parent", Value::String("block:parent".into())),
            ("context_key", Value::String("key1".into())),
        ]);
        p2.insert(Arc::from("bindings"), Value::Null);
        let req2 = InstantiateRequest::from_params(&p2).unwrap();
        assert!(req2.bindings.is_empty());
    }
}
