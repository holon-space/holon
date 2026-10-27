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

use crate::Block;
// Property keys live in `crate::template` (single spelling shared with the
// frontend picker); re-exported here so existing `TEMPLATE_MARKER_PROPERTY`
// references and `{TEMPLATE_MARKER_PROPERTY}` format strings keep resolving.
pub use crate::INSTANCE_OF_PROPERTY;
use crate::MarkSpan;
// `StorageEntity` is the SAME `HashMap<Arc<str>, Value>` alias in both crates;
// this planner is pure and backend-free, so it uses holon-api's own alias.
use crate::StorageEntity;
pub use crate::TEMPLATE_MARKER_PROPERTY;
pub use crate::TEMPLATE_VARS_PROPERTY;
use crate::Value;
use crate::effect_id::deterministic_instance_id;
use crate::marks_to_json;

/// The org identity property (`:ID:` drawer entry, lifted into `properties` by
/// the org parser). It DERIVES the block's id — so it must never be copied.
const ORG_ID_PROPERTY: &str = "ID";

/// Identity/meta properties that must NEVER propagate from a template node into
/// its instance (parse-don't-validate: an explicit denylist, not ad-hoc
/// per-key `remove`s). Copying any of these corrupts data:
/// - `ID`: the instance would claim the TEMPLATE's org id. On writeback+reload
///   the duplicate `:ID:` collides on the derived block id — observed to EMPTY
///   the template's org file and turn the template into an instance of itself
///   (BugFunnel: org-roundtrip destruction).
/// - `_provenance`: the engine's authorship stamp, re-minted fresh per create.
///
/// Template MARKER properties (`template`/`template_vars`) are handled
/// separately (stripped from the instantiated ROOT only, so nested templates
/// still round-trip verbatim per the proposal).
const NON_COPYABLE_PROPERTIES: &[&str] = &[ORG_ID_PROPERTY, crate::PROVENANCE_PROPERTY];

/// A block's task keyword and its derived category. Both live in the property
/// bag, but a write carries the keyword as its own field.
const TASK_STATE_PROPERTY: &str = "task_state";
const TASK_STATE_CATEGORY_PROPERTY: &str = "task_state_category";

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

    /// Test-only helper (the planner resolves defaults inline via
    /// [`Self::declares`] + the decl list, not through this accessor).
    #[cfg(test)]
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
    /// Optional: an *empty* block the instance supersedes (the frontend
    /// picker's empty→in-place placement,
    /// docs/Proposals/Templating-2026-07-12.md §8). When set, the engine
    /// deletes it AFTER the instance is created, so a failed instantiation
    /// never destroys the target. Empty blocks carry no content, so this
    /// never mutates existing content.
    pub replace_block: Option<String>,
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
        let replace_block = match params.get("replace_block") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            Some(other) => bail!(
                "instantiate_template: param 'replace_block' must be a non-empty string, got \
                 {other:?}"
            ),
        };

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
            replace_block,
        })
    }
}

/// The computed instantiation: ordered (parent-before-child) `create` param
/// maps plus the instance root id.
#[derive(Clone, Debug)]
pub struct InstantiationPlan {
    pub root_id: String,
    pub creates: Vec<StorageEntity>,
}

/// Build the instantiation plan. `nodes` must be the template subtree in
/// parent-before-child order with the template root first and siblings in
/// order: the creates come out in that order, and each lands as its parent's
/// last child, so the instance keeps the template's sibling order.
pub fn plan_instantiation(
    nodes: &[Block],
    request: &InstantiateRequest,
) -> Result<InstantiationPlan> {
    let root = nodes
        .first()
        .with_context(|| format!("template '{}' has no blocks", request.template_id))?;
    if root.id.as_str() != request.template_id {
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
        Some(Value::String(raw)) => TemplateVars::parse(raw)
            .with_context(|| format!("template '{}': invalid template_vars", root.id))?,
        Some(other) => bail!(
            "template '{}': '{TEMPLATE_VARS_PROPERTY}' must be a string, got {other:?}",
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
            if let Value::String(s) = value {
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
            deterministic_instance_id(&request.template_id, &request.context_key, node.id.as_str());
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
        let marks = node
            .marks
            .as_ref()
            .map(|spans| marks_to_json(&remap_marks(spans, &replacements)));

        let mut props = parse_properties(node)?;
        // Strip identity/meta properties that must not propagate (the `ID`
        // copy is the org-roundtrip destruction bug; `_provenance` is
        // re-minted per create). Denylist, not ad-hoc removes.
        for key in NON_COPYABLE_PROPERTIES {
            props.remove(*key);
        }
        // The engine classifies the keyword by the ring of the document the
        // instance lands in.
        props.remove(TASK_STATE_CATEGORY_PROPERTY);
        let task_state = props.remove(TASK_STATE_PROPERTY);
        for value in props.values_mut() {
            if let Value::String(s) = value {
                let (substituted, _) = substitute(s, &effective)?;
                *value = Value::String(substituted);
            }
        }
        if is_root {
            remove_template_marker(&mut props, TEMPLATE_MARKER_PROPERTY);
            remove_template_marker(&mut props, TEMPLATE_VARS_PROPERTY);
            props.insert(
                INSTANCE_OF_PROPERTY.to_string(),
                Value::String(request.template_id.clone()),
            );
        }

        let mut params: StorageEntity = StorageEntity::default();
        let mut put = |k: &str, v: Value| {
            params.insert(Arc::from(k), v);
        };
        put("id", Value::String(id_map[node.id.as_str()].clone()));
        put("parent_id", Value::String(new_parent));
        put("content", Value::String(content));
        put("content_type", Value::String(node.content_type.to_string()));
        put("collapsed", Value::Boolean(node.collapsed));
        put("widget_only", Value::Boolean(node.widget_only));
        if let Some(lang) = &node.source_language {
            put("source_language", Value::String(lang.to_string()));
        }
        if let Some(name) = &node.source_name {
            put("source_name", Value::String(name.clone()));
        }
        if let Some(marks_json) = marks {
            put("marks", Value::String(marks_json));
        }
        if let Some(block_type) = &node.block_type {
            put("block_type", block_type.clone().into());
        }
        if let Some(task_state) = task_state {
            put(TASK_STATE_PROPERTY, task_state);
        }
        if !props.is_empty() {
            let json = serde_json::to_string(&props).expect("property map serialization is total");
            put("properties", Value::String(json));
        }
        creates.push(params);
    }

    Ok(InstantiationPlan {
        root_id: id_map[root.id.as_str()].clone(),
        creates,
    })
}

fn parse_properties(node: &Block) -> Result<BTreeMap<String, Value>> {
    node.properties
        .iter()
        .map(|(k, v)| match v {
            Value::Removed(_) => bail!(
                "template node '{}': property '{k}' holds the write-leg removal sentinel",
                node.id
            ),
            v => Ok((k.clone(), v.clone())),
        })
        .collect()
}

/// Case-insensitive lookup for template-marker keys. Delegates the casing rule
/// to the shared authority in `crate::template` so the planner and the
/// frontend picker can never diverge on `:TEMPLATE:` (org, uppercase) vs
/// `template` (programmatic, lowercase).
fn template_marker_key<'a>(props: &'a BTreeMap<String, Value>, marker: &str) -> Option<&'a String> {
    let matched =
        crate::template::find_template_marker_key(props.keys().map(String::as_str), marker)?;
    props.get_key_value(matched).map(|(k, _)| k)
}

fn template_marker_value<'a>(
    props: &'a BTreeMap<String, Value>,
    marker: &str,
) -> Option<&'a Value> {
    template_marker_key(props, marker).and_then(|k| props.get(k))
}

/// Remove a template-marker key case-insensitively, returning the
/// actual key that was removed (if any).
fn remove_template_marker(props: &mut BTreeMap<String, Value>, marker: &str) -> Option<String> {
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
mod tests;
