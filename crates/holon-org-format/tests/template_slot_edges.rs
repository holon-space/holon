//! Typed-edge drawer keys accept `{{var}}` slots INSIDE a template subtree.
//!
//! Dogfood 2026-08-18: Holon ships `assets/default/Compass.org` with
//! `:contributes-to: {{mission}}`, and its own org parser refused it — so the
//! seeded `Templates/Compass.org` was QUARANTINED on every vault seeded from
//! defaults, with a permanent red "bad org file" banner.
//!
//! Both halves were individually right. The template feature owns `{{…}}`:
//! `holon_api::template_instantiation` substitutes slots inside every string
//! PROPERTY value, so `:contributes-to: {{mission}}` becomes a real edge at
//! instantiation. The edge parser owned block ids and knew nothing about
//! templates. Templates must survive ingest — they live in the store as
//! ordinary blocks, because instantiation reads them from storage rows — so a
//! template that cannot be ingested can never be instantiated.
//!
//! Fix: the edge parser classifies a slug into `Block` / `None` / `Slot`, and
//! accepts `Slot` only inside a template subtree for a variable the enclosing
//! `:TEMPLATE_VARS:` declares. A slot-bearing value contributes NO junction row
//! and is carried verbatim as a plain drawer property, so it reaches disk and
//! the store intact. Outside a template subtree `{{…}}` is refused exactly as
//! before.
//!
//! BugFunnel 2026-08-18-shipped-compass-asset-refused-by-own-parser.
//!
//! @pbt kind harness
//! @pbt covers template-slot-edges — shipped assets ingest; `{{var}}` legal
//! only inside a template subtree and only when declared
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone generates
//! synthetic org and never ingests the shipped `assets/default` files

use std::path::Path;

use holon_api::EntityUri;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const ROOT: &str = "/vault";
const FILE: &str = "/vault/doc.org";

fn parse(source: &str) -> anyhow::Result<holon_org_format::ParseResult> {
    parse_org_file(
        Path::new(FILE),
        source,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
}

/// THE GATE. Every org file Holon SHIPS must ingest with the production
/// parser. This is the check whose absence let an asset the parser refuses
/// ship green.
#[test]
fn every_shipped_default_asset_ingests_with_the_production_parser() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/default");
    let assets = assets.canonicalize().expect("assets/default must exist");

    let mut org_files: Vec<_> = std::fs::read_dir(&assets)
        .expect("read assets/default")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "org"))
        .collect();
    org_files.sort();
    assert!(
        !org_files.is_empty(),
        "no shipped .org assets found under {} — this gate would be vacuous",
        assets.display()
    );

    for path in &org_files {
        let source = std::fs::read_to_string(path).expect("read shipped asset");
        if let Err(e) = parse_org_file(path, &source, &EntityUri::no_parent(), &assets) {
            panic!(
                "shipped asset {} does not ingest with the production parser — a vault seeded \
                 from defaults would QUARANTINE it and show a permanent degraded banner: {e:#}",
                path.display()
            );
        }
    }
}

/// Inside a template subtree, a declared slot parses and contributes NO edge —
/// the agenda's reverse closure must never see a template.
#[test]
fn a_declared_slot_inside_a_template_parses_and_contributes_no_edge() {
    let source = "\
* Problem
:PROPERTIES:
:ID: tpl-0
:TEMPLATE: compass-problem
:TEMPLATE_VARS: title, mission
:contributes-to: {{mission}}
:END:
";
    let parsed = parse(source).expect("a declared slot inside a template must parse");
    let block = parsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "tpl-0")
        .expect("template root present");
    assert!(
        block.contributes_to.is_empty(),
        "a slot must contribute no typed edge, got {:?}",
        block.contributes_to
    );
}

/// The slot must round-trip byte-intact: it has to reach disk AND the store so
/// `template_instantiation` still has `{{mission}}` to substitute.
#[test]
fn a_slot_survives_the_write_back_round_trip_verbatim() {
    let source = "\
* Problem
:PROPERTIES:
:ID: tpl-0
:TEMPLATE: compass-problem
:TEMPLATE_VARS: title, mission
:contributes-to: {{mission}}
:END:
";
    let parsed = parse(source).expect("fixture must parse");
    let rendered = OrgRenderer::render_document(
        &parsed.document,
        &parsed.blocks,
        Path::new(FILE),
        &parsed.document.id,
    );
    assert!(
        rendered.contains(":contributes-to: {{mission}}"),
        "the authored slot must survive write-back verbatim, else instantiation loses it and the \
         template file is rewritten without its slots. Rendered:\n{rendered}"
    );
}

/// A slot inherited from an ANCESTOR template root is in scope: the marker sits
/// on the subtree root, not on every descendant.
#[test]
fn a_slot_is_in_scope_for_a_descendant_of_the_template_root() {
    let source = "\
* Goal
:PROPERTIES:
:ID: tpl-root
:TEMPLATE: compass-goal
:TEMPLATE_VARS: mission
:END:
** Sub
:PROPERTIES:
:ID: tpl-child
:contributes-to: {{mission}}
:END:
";
    let parsed = parse(source).expect("a descendant of a template root is inside the template");
    let child = parsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "tpl-child")
        .expect("child present");
    assert!(
        child.contributes_to.is_empty(),
        "a slot contributes no edge"
    );
}

/// THE TEETH, half one. The old refusal stands everywhere outside a template:
/// nothing would ever substitute such a slot, so it names no block and never
/// will. This is the behaviour the pre-existing parser test pinned.
#[test]
fn the_same_slot_outside_a_template_is_still_refused() {
    for source in [
        "* Goal\n:PROPERTIES:\n:ID: p0\n:contributes-to: {{mission}}\n:END:\n",
        "* Task\n:PROPERTIES:\n:ID: p1\n:REQUIRES: {{mission}}\n:END:\n",
        "* Task\n:PROPERTIES:\n:ID: p2\n:BLOCKED-BY: {{mission}}\n:END:\n",
    ] {
        let Err(err) = parse(source) else {
            panic!("a slot outside a template must be refused, not parsed: {source:?}");
        };
        let err = format!("{err:#}");
        assert!(
            err.contains("takes bare block IDs") && err.contains("{{mission}}"),
            "the refusal must name the offending value: {err}"
        );
    }
}

/// THE TEETH, half two. A slot the enclosing template does not DECLARE is a
/// loud error naming the variable and the missing declaration — accepting it
/// would mint a template that fails only at instantiation time.
#[test]
fn an_undeclared_slot_inside_a_template_is_refused_naming_the_variable() {
    let source = "\
* Problem
:PROPERTIES:
:ID: tpl-0
:TEMPLATE: compass-problem
:TEMPLATE_VARS: title
:contributes-to: {{mission}}
:END:
";
    let Err(err) = parse(source) else {
        panic!("an undeclared template variable must be refused");
    };
    let err = format!("{err:#}");
    assert!(
        err.contains("mission") && err.contains("TEMPLATE_VARS"),
        "the refusal must name the variable and the missing declaration: {err}"
    );
}

/// A real block id inside a template is still a real edge — the slot case must
/// not have turned the whole drawer key into inert text.
#[test]
fn a_real_id_inside_a_template_still_becomes_a_typed_edge() {
    let source = "\
* Problem
:PROPERTIES:
:ID: tpl-0
:TEMPLATE: compass-problem
:TEMPLATE_VARS: mission
:contributes-to: some-real-goal
:END:
";
    let parsed = parse(source).expect("fixture must parse");
    let block = parsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "tpl-0")
        .expect("template root present");
    assert_eq!(
        block.contributes_to,
        vec![EntityUri::parse("block:some-real-goal").unwrap()],
        "a bare id inside a template is an ordinary contribution edge"
    );
}

/// END TO END. Instantiating the SHIPPED Compass problem template turns its
/// `{{mission}}` slot into a real block id, and the instance re-parses to a
/// real `contributes_to` EDGE.
///
/// This is the claim the whole fix exists to protect: the parser stops
/// refusing the template, and instantiation still produces the contribution
/// edge the Compass convention is built on (the agenda query is a reverse
/// closure over `block_contributes_to.target_id`).
#[test]
fn instantiating_the_shipped_compass_problem_yields_a_real_contributes_to_edge() {
    use holon_api::template_instantiation::InstantiateRequest;
    use holon_api::template_instantiation::TemplateNode;
    use holon_api::template_instantiation::plan_instantiation;

    let asset = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/default/Compass.org");
    let asset = asset
        .canonicalize()
        .expect("shipped Compass.org must exist");
    let source = std::fs::read_to_string(&asset).expect("read Compass.org");
    let root_dir = asset.parent().expect("assets dir");

    let parsed = parse_org_file(&asset, &source, &EntityUri::no_parent(), root_dir)
        .expect("the shipped Compass template must ingest");

    // The problem template's root — the block whose slot Martin's banner named.
    let root = parsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "compass-problem-tpl-0")
        .expect("compass-problem-tpl-0 present in the shipped asset");
    assert_eq!(
        root.get_property_str("contributes-to").as_deref(),
        Some("{{mission}}"),
        "the slot must reach the store as authored, else instantiation has nothing to substitute"
    );

    let node = TemplateNode {
        id: root.id.id().to_string(),
        parent_id: String::new(),
        content: root.content.clone(),
        properties: Some(
            serde_json::to_string(&root.properties_map()).expect("properties serialize"),
        ),
        ..TemplateNode::default()
    };

    let request = InstantiateRequest {
        template_id: node.id.clone(),
        target_parent: "block:target-page".to_string(),
        context_key: "test-instantiation".to_string(),
        bindings: [
            ("title".to_string(), "Onboarding is slow".to_string()),
            ("mission".to_string(), "the-mission-block".to_string()),
            ("reviewed".to_string(), "2026-08-18".to_string()),
        ]
        .into_iter()
        .collect(),
        replace_block: None,
    };

    let plan = plan_instantiation(std::slice::from_ref(&node), &request)
        .expect("instantiating the shipped compass-problem template must succeed");

    let created = plan.creates.first().expect("one create for the root");
    let props: serde_json::Value = serde_json::from_str(
        created
            .get("properties")
            // ALLOW(jsonb_as_string): not a CDC row — this is the planner's
            // create-op param map, where `properties` is written as
            // `Value::String` (template_instantiation.rs:407).
            .and_then(|v| v.as_string())
            .expect("instance carries properties"),
    )
    .expect("instance properties are JSON");
    assert_eq!(
        props["contributes-to"], "the-mission-block",
        "instantiation must substitute the slot with the bound mission id: {props}"
    );

    // The instance is an ORDINARY block — no `:TEMPLATE:` marker — so the same
    // drawer value now parses as a real typed edge rather than a slot.
    let instance_org = "* Onboarding is slow\n:PROPERTIES:\n:ID: \
                        instance-0\n:contributes-to: the-mission-block\n:END:\n";
    let reparsed = parse(instance_org).expect("the instance must parse");
    let instance = reparsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "instance-0")
        .expect("instance block present");
    assert_eq!(
        instance.contributes_to,
        vec![EntityUri::parse("block:the-mission-block").unwrap()],
        "the instantiated Compass problem must carry a REAL contributes-to edge"
    );
}
