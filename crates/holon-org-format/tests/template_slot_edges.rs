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
