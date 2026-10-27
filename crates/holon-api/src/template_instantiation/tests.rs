use super::*;
use crate::EntityUri;
use crate::InlineMark;
use crate::marks_from_json;

fn node(id: &str, parent: &str, content: &str) -> Block {
    Block {
        id: EntityUri::parse(id).expect("test block id is a URI"),
        parent_id: EntityUri::parse(parent).expect("test parent id is a URI"),
        content: content.to_string(),
        ..Block::default()
    }
}

fn plan(
    nodes: impl IntoIterator<Item = Block>,
    request: &InstantiateRequest,
) -> Result<InstantiationPlan> {
    let nodes: Vec<Block> = nodes.into_iter().collect();
    plan_instantiation(&nodes, request)
}

fn props(json: &str) -> std::collections::HashMap<String, Value> {
    serde_json::from_str(json).expect("test properties are a JSON object")
}

fn template_root(id: &str, content: &str, vars: &str) -> Block {
    let mut n = node(id, "block:somewhere", content);
    n.properties = props(&format!(r#"{{"template":"t","template_vars":"{vars}"}}"#));
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
        replace_block: None,
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
    root.properties =
        props(r#"{"template":"t","template_vars":"date, mood=neutral","note":"feeling {{mood}}"}"#);
    let child = node("block:c1", "block:tpl", "Mood check: {{mood}} on {{date}}");
    let plan = plan([root, child], &request(&[("date", "2026-07-12")])).unwrap();

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
fn plan_excludes_org_identity_property_from_every_instance() {
    // Org-parsed template blocks carry their `:ID:` as an "ID" property
    // (block_params.rs). Copying it makes the instance claim the TEMPLATE's
    // id — on org writeback+reload the duplicate collides and destroys the
    // template file. The denylist must strip "ID" from EVERY node.
    let mut root = template_root("block:tpl", "hi", "");
    root.properties = props(r#"{"template":"t","template_vars":"","ID":"tpl-daily","keep":"yes"}"#);
    let mut child = node("block:c1", "block:tpl", "child");
    child.properties = props(r#"{"ID":"tpl-daily-child","keep":"yes"}"#);
    let plan = plan([root, child], &request(&[])).unwrap();

    for params in &plan.creates {
        let props: serde_json::Value = serde_json::from_str(get_str(params, "properties")).unwrap();
        assert!(
            props.get("ID").is_none(),
            "ID must NEVER propagate to an instance (org-roundtrip destruction)"
        );
        assert_eq!(props["keep"], "yes", "non-identity properties still copy");
    }
    // The instance id is the deterministic mint, never the template's "ID".
    assert_ne!(get_str(&plan.creates[0], "id"), "tpl-daily");
}

#[test]
fn a_block_type_is_copied_and_an_absent_one_stays_absent() {
    let nodes = [
        Block {
            block_type: Some(crate::EntityName::new("note")),
            ..template_root("block:tpl", "root", "")
        },
        node("block:c1", "block:tpl", "child"),
    ];
    let plan = plan_instantiation(&nodes, &request(&[])).unwrap();
    assert_eq!(get_str(&plan.creates[0], "block_type"), "note");
    assert_eq!(plan.creates[1].get("block_type"), None);
}

#[test]
fn missing_binding_fails_loud_listing_all() {
    let root = template_root("block:tpl", "{{date}} and {{title}}", "date, title");
    let err = plan([root], &request(&[])).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("missing bindings"), "got: {msg}");
    assert!(msg.contains("date") && msg.contains("title"), "got: {msg}");
}

#[test]
fn undeclared_variable_reference_fails_loud() {
    let root = template_root("block:tpl", "{{typo}}", "date");
    let err = plan([root], &request(&[("date", "2026-07-12")])).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("undeclared variable") && msg.contains("typo"),
        "got: {msg}"
    );
}

#[test]
fn unknown_binding_key_fails_loud() {
    let root = template_root("block:tpl", "{{date}}", "date");
    let err = plan([root], &request(&[("date", "d"), ("nope", "x")])).unwrap_err();
    assert!(format!("{err:#}").contains("binding 'nope' is not declared"));
}

#[test]
fn non_template_root_fails_loud() {
    let root = node("block:tpl", "block:p", "just a block");
    let err = plan([root], &request(&[])).unwrap_err();
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
    let a = plan(mk(), &request(&[("date", "d")])).unwrap();
    let b = plan(mk(), &request(&[("date", "d")])).unwrap();
    assert_eq!(a.root_id, b.root_id, "same context key → same ids");
    assert_eq!(get_str(&a.creates[1], "id"), get_str(&b.creates[1], "id"));

    let mut other = request(&[("date", "d")]);
    other.context_key = "another-key".to_string();
    let c = plan(mk(), &other).unwrap();
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
                target: crate::EntityRef::Name {
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
    root.marks = Some(spans);
    let plan = plan([root], &request(&[("date", "2026-07-12")])).unwrap();
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
    let err = plan([root], &request(&[("date", "d")])).unwrap_err();
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
    bindings.insert("float_val".to_string(), Value::Float(2.75));
    bindings.insert("bool_val".to_string(), Value::Boolean(true));
    bindings.insert(
        "dt_val".to_string(),
        Value::DateTime("2026-07-12T00:00:00Z".into()),
    );
    p.insert(Arc::from("bindings"), Value::Object(bindings));

    let req = InstantiateRequest::from_params(&p).unwrap();
    assert_eq!(req.bindings.get("int_val").unwrap(), "42");
    assert_eq!(req.bindings.get("float_val").unwrap(), "2.75");
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
