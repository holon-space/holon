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
use holon_api::block::Block;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const ROOT: &str = "/vault";
const FILE: &str = "/vault/doc.org";

/// A template root that declares `mission`, ready for a slot line plus
/// `:END:`. Shared by the fixed-point cases below.
const TEMPLATE_HEAD: &str = "* Problem\n:PROPERTIES:\n:ID: tpl-0\n:TEMPLATE: compass-problem\n:TEMPLATE_VARS: title, mission\n";

fn parse(source: &str) -> anyhow::Result<holon_org_format::ParseResult> {
    parse_org_file(
        Path::new(FILE),
        source,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
}

/// Collect every `.org` file under `dir`, recursively. The gate must cover
/// nested asset directories (e.g. `assets/default/types/`), not only the top
/// level — a non-recursive scan would let a refused asset in a subdirectory
/// ship green.
fn org_files_under(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read asset dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            out.extend(org_files_under(&path));
        } else if path.extension().is_some_and(|x| x == "org") {
            out.push(path);
        }
    }
    out
}

/// THE GATE. Every org file Holon SHIPS must ingest with the production
/// parser. This is the check whose absence let an asset the parser refuses
/// ship green.
#[test]
fn every_shipped_default_asset_ingests_with_the_production_parser() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/default");
    let assets = assets.canonicalize().expect("assets/default must exist");

    let mut org_files = org_files_under(&assets);
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

    // Now render the PLANNED instance and read it back. The org text comes from
    // the plan's own params — not a hand-authored literal — so the chain under
    // test is genuinely asset -> plan -> params -> org -> typed edge.
    //
    // The block is assembled field-by-field because the planner emits create-op
    // PARAMS, and there is no params -> Block deserializer to reuse: the SQL
    // row deserializer requires edge columns (`requires`, `advice_suppressed`,
    // …) that a create param map does not carry.
    let mut instance = Block::new_text(
        EntityUri::parse("block:instance-0").unwrap(),
        EntityUri::parse("block:target-page").unwrap(),
        created
            .get("content")
            .and_then(|v| v.as_string())
            .expect("instance carries content"),
    );
    let serde_json::Value::Object(prop_map) = &props else {
        panic!("instance properties must be a JSON object: {props}");
    };
    for (k, v) in prop_map {
        if let serde_json::Value::String(text) = v {
            instance.set_property(k.as_str(), holon_api::Value::String(text.clone()));
        }
    }

    let mut page = Block::new_text(
        EntityUri::parse("block:target-page").unwrap(),
        EntityUri::no_parent(),
        "Target page",
    );
    page.set_page(true);
    let rendered = OrgRenderer::render_document(
        &page,
        std::slice::from_ref(&instance),
        Path::new(FILE),
        &page.id,
    );
    assert!(
        rendered.contains(":contributes-to: the-mission-block"),
        "the rendered instance must carry the substituted id, not a slot:\n{rendered}"
    );

    // The instance is an ORDINARY block — no `:TEMPLATE:` marker — so that same
    // drawer value parses back as a real typed edge rather than a slot.
    let reparsed = parse(&rendered).expect("the rendered instance must parse");
    let read_back = reparsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "instance-0")
        .expect("instance block present after the round trip");
    assert_eq!(
        read_back.contributes_to,
        vec![EntityUri::parse("block:the-mission-block").unwrap()],
        "the instantiated Compass problem must carry a REAL contributes-to edge"
    );
}

/// A slot on a SOURCE-BLOCK header arg survives the round trip too.
///
/// Caught by a verifier probe on the first cut of this change: the
/// `contributes-to` / `REQUIRES` header-arg branches OWN their key — the
/// generic property fallthrough beneath them is an `else if` — so a branch that
/// stored no typed edge for a slot dropped the authored text entirely. Silent
/// data loss on a template's own file.
#[test]
fn a_slot_on_a_source_block_header_arg_survives_the_round_trip() {
    let source = "\
* Problem
:PROPERTIES:
:ID: tpl-0
:TEMPLATE: compass-problem
:TEMPLATE_VARS: title, mission
:END:
#+begin_src prql :id s0 :contributes-to {{mission}}
from x
#+end_src
";
    let parsed = parse(source).expect("a src-block slot inside a template must parse");
    let sb = parsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "s0")
        .expect("source block present");
    assert!(
        sb.contributes_to.is_empty(),
        "a slot contributes no typed edge, got {:?}",
        sb.contributes_to
    );

    let rendered = OrgRenderer::render_document(
        &parsed.document,
        &parsed.blocks,
        Path::new(FILE),
        &parsed.document.id,
    );
    assert!(
        rendered.contains("{{mission}}"),
        "the authored slot must survive write-back — dropping it rewrites the template file \
         without its slots. Rendered:\n{rendered}"
    );
}

/// The `none` sentinel belongs to `:contributes-to:` ALONE.
///
/// `:REQUIRES: none` has always meant an edge to a block called `none`.
/// Widening the sentinel across every edge key silently deletes that edge, so
/// the scope is pinned here.
#[test]
fn none_is_a_contributes_to_sentinel_only_and_requires_keeps_its_edge() {
    let contributes =
        parse("* G\n:PROPERTIES:\n:ID: c0\n:contributes-to: none\n:END:\n").expect("parses");
    let c = contributes
        .blocks
        .iter()
        .find(|b| b.id.id() == "c0")
        .expect("block present");
    assert!(
        c.contributes_to.is_empty(),
        "`none` is the authored empty set for contributes-to, got {:?}",
        c.contributes_to
    );

    let requires = parse("* T\n:PROPERTIES:\n:ID: r0\n:REQUIRES: none\n:END:\n").expect("parses");
    let r = requires
        .blocks
        .iter()
        .find(|b| b.id.id() == "r0")
        .expect("block present");
    assert_eq!(
        r.requires,
        vec![EntityUri::parse("block:none").unwrap()],
        "`:REQUIRES: none` names a block called `none` — it is not an empty-set sentinel"
    );
}

/// A headline `:REQUIRES:` / `:BLOCKED-BY:` slot inside a template survives the
/// round trip too — the same OWNS-the-key hazard as `:contributes-to:`. Both
/// dependency spellings route through `edge_ids`; a slot yields no edge and the
/// authored text must be carried as a plain drawer property, not dropped.
#[test]
fn a_headline_requires_slot_inside_a_template_survives_the_round_trip() {
    for key in ["REQUIRES", "BLOCKED-BY"] {
        let source = format!(
            "* Task\n:PROPERTIES:\n:ID: tpl-0\n:TEMPLATE: compass-task\n\
             :TEMPLATE_VARS: dep\n:{key}: {{{{dep}}}}\n:END:\n"
        );
        let parsed = parse(&source).unwrap_or_else(|e| panic!("`:{key}:` slot must parse: {e:#}"));
        let block = parsed
            .blocks
            .iter()
            .find(|b| b.id.id() == "tpl-0")
            .expect("template root present");
        assert!(
            block.requires.is_empty(),
            "a `:{key}:` slot contributes no dependency edge, got {:?}",
            block.requires
        );

        let rendered = OrgRenderer::render_document(
            &parsed.document,
            &parsed.blocks,
            Path::new(FILE),
            &parsed.document.id,
        );
        assert!(
            rendered.contains("{{dep}}"),
            "the authored `:{key}:` slot must survive write-back — dropping it rewrites the \
             template without its dependency slot. Rendered:\n{rendered}"
        );
    }
}

/// A slot on a SOURCE-BLOCK `:REQUIRES` header arg survives too — the src-block
/// dependency branch OWNS its key exactly like the headline path, so the same
/// silent-drop hazard applies.
#[test]
fn a_requires_slot_on_a_source_block_header_arg_survives_the_round_trip() {
    let source = "\
* Task
:PROPERTIES:
:ID: tpl-0
:TEMPLATE: compass-task
:TEMPLATE_VARS: dep
:END:
#+begin_src prql :id s0 :REQUIRES {{dep}}
from x
#+end_src
";
    let parsed = parse(source).expect("a src-block REQUIRES slot inside a template must parse");
    let sb = parsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "s0")
        .expect("source block present");
    assert!(
        sb.requires.is_empty(),
        "a slot contributes no dependency edge, got {:?}",
        sb.requires
    );

    let rendered = OrgRenderer::render_document(
        &parsed.document,
        &parsed.blocks,
        Path::new(FILE),
        &parsed.document.id,
    );
    assert!(
        rendered.contains("{{dep}}"),
        "the authored src-block `:REQUIRES` slot must survive write-back. Rendered:\n{rendered}"
    );
}

/// EVERY edge key on EVERY block kind carries a slot through a full
/// write -> read -> write cycle, byte-identically.
///
/// The per-key tests above assert the slot is PRESENT after one render. This
/// one pins the stronger property the round trip actually needs: rendering the
/// re-parsed document reproduces the first render exactly, so a template file
/// on disk is a fixed point rather than something that drifts each time
/// write-back touches it.
///
/// `:BLOCKED-BY:` is included deliberately: it canonicalises onto `:REQUIRES:`
/// at render, so its slot has to survive a key rename as well as the carrier.
#[test]
fn every_edge_key_and_block_kind_reaches_a_slot_preserving_fixed_point() {
    let cases: [(&str, String); 4] = [
        (
            "headline contributes-to",
            format!("{TEMPLATE_HEAD}:contributes-to: {{{{mission}}}}\n:END:\n"),
        ),
        (
            "headline REQUIRES",
            format!("{TEMPLATE_HEAD}:REQUIRES: {{{{mission}}}}\n:END:\n"),
        ),
        (
            "headline BLOCKED-BY",
            format!("{TEMPLATE_HEAD}:BLOCKED-BY: {{{{mission}}}}\n:END:\n"),
        ),
        (
            "source-block BLOCKED-BY header arg",
            format!(
                "{TEMPLATE_HEAD}:END:\n#+begin_src prql :id s0 :BLOCKED-BY                  {{{{mission}}}}\nfrom x\n#+end_src\n"
            ),
        ),
    ];

    for (name, source) in cases {
        let first = parse(&source).unwrap_or_else(|e| panic!("{name}: must parse: {e:#}"));
        let r1 = OrgRenderer::render_document(
            &first.document,
            &first.blocks,
            Path::new(FILE),
            &first.document.id,
        );
        assert!(
            r1.contains("{{mission}}"),
            "{name}: the slot was dropped on the first write-back:\n{r1}"
        );

        let second = parse(&r1).unwrap_or_else(|e| panic!("{name}: re-parse must succeed: {e:#}"));
        let r2 = OrgRenderer::render_document(
            &second.document,
            &second.blocks,
            Path::new(FILE),
            &second.document.id,
        );
        assert_eq!(
            r1, r2,
            "{name}: write -> read -> write is not a fixed point, so the file drifts on every \
             sync tick"
        );
    }
}

/// THE CROSS-KEY CLOBBER (verifier probe, 2026-08-19). `:REQUIRES:` and
/// `:BLOCKED-BY:` are two spellings of the SAME `block_requires` edge, so a
/// slot under one and a real id under the other are ONE mixed dependency list
/// split across two keys — semantically identical to `:REQUIRES: {{mission}}
/// real-dep` in a single value.
///
/// The pre-fix parser lifted the real side into `block.requires` AND carried
/// the slot as a plain `REQUIRES` property. At render both claimed the
/// canonical `REQUIRES` drawer key, and the typed-edge writer (an unconditional
/// `insert`) clobbered the carried slot — `{{mission}}` was silently dropped on
/// the FIRST write-back. A template that can no longer be instantiated,
/// rewritten to disk without a word of warning.
///
/// The fix resolves the whole group as a unit: any slot in it means NO typed
/// edge and ONE merged verbatim value under `REQUIRES`, authored order kept.
#[test]
fn a_cross_key_slot_and_real_id_survive_the_round_trip() {
    // (label, slot spelling, real spelling, slot-first?) — both drawer spellings
    // in both orders. The merged carried value is authored order, first line first.
    let cases: [(&str, &str, &str, bool); 4] = [
        (
            "REQUIRES-slot then BLOCKED-BY-real",
            "REQUIRES",
            "BLOCKED-BY",
            true,
        ),
        (
            "BLOCKED-BY-slot then REQUIRES-real",
            "BLOCKED-BY",
            "REQUIRES",
            true,
        ),
        (
            "REQUIRES-real then BLOCKED-BY-slot",
            "REQUIRES",
            "BLOCKED-BY",
            false,
        ),
        (
            "BLOCKED-BY-real then REQUIRES-slot",
            "BLOCKED-BY",
            "REQUIRES",
            false,
        ),
    ];
    for (label, slot_key, real_key, slot_first) in cases {
        let (first_line, second_line, expected_merged) = if slot_first {
            (
                format!(":{slot_key}: {{{{mission}}}}"),
                format!(":{real_key}: real-dep"),
                "{{mission}} real-dep",
            )
        } else {
            (
                format!(":{real_key}: real-dep"),
                format!(":{slot_key}: {{{{mission}}}}"),
                "real-dep {{mission}}",
            )
        };
        let source = format!("{TEMPLATE_HEAD}{first_line}\n{second_line}\n:END:\n");

        let first = parse(&source).unwrap_or_else(|e| panic!("{label}: must parse: {e:#}"));
        let block = first
            .blocks
            .iter()
            .find(|b| b.id.id() == "tpl-0")
            .expect("template root present");
        assert!(
            block.requires.is_empty(),
            "{label}: a group holding a slot contributes NO typed edge (the slot has no junction \
             row), got {:?}",
            block.requires
        );

        let r1 = OrgRenderer::render_document(
            &first.document,
            &first.blocks,
            Path::new(FILE),
            &first.document.id,
        );
        assert!(
            r1.contains("{{mission}}"),
            "{label}: the slot was clobbered by the typed edge on the first write-back:\n{r1}"
        );
        assert!(
            r1.contains("real-dep"),
            "{label}: the real dependency was dropped on the first write-back:\n{r1}"
        );
        assert!(
            r1.contains(&format!(":REQUIRES: {expected_merged}")),
            "{label}: the two spellings must merge into one canonical `:REQUIRES:` value in \
             authored order (expected {expected_merged:?}):\n{r1}"
        );

        let second = parse(&r1).unwrap_or_else(|e| panic!("{label}: re-parse must succeed: {e:#}"));
        let r2 = OrgRenderer::render_document(
            &second.document,
            &second.blocks,
            Path::new(FILE),
            &second.document.id,
        );
        assert_eq!(
            r1, r2,
            "{label}: write -> read -> write is not a fixed point:\n{r1}\n---\n{r2}"
        );
    }
}

/// The same cross-key clobber on a SOURCE-BLOCK's header args. `header_args` is
/// a `HashMap`, so authored order is unrecoverable — the carried value is
/// sorted for determinism, and the pin asserts only that BOTH the slot and the
/// real id survive and the round trip is a fixed point (no order claim).
#[test]
fn a_cross_key_slot_and_real_id_on_a_source_block_survive_the_round_trip() {
    for (label, line) in [
        (
            "REQUIRES slot, BLOCKED-BY real",
            ":REQUIRES {{mission}} :BLOCKED-BY real-dep",
        ),
        (
            "BLOCKED-BY slot, REQUIRES real",
            ":BLOCKED-BY {{mission}} :REQUIRES real-dep",
        ),
    ] {
        let source =
            format!("{TEMPLATE_HEAD}:END:\n#+begin_src prql :id s0 {line}\nfrom x\n#+end_src\n");
        let first = parse(&source).unwrap_or_else(|e| panic!("{label}: must parse: {e:#}"));
        let sb = first
            .blocks
            .iter()
            .find(|b| b.id.id() == "s0")
            .expect("source block present");
        assert!(
            sb.requires.is_empty(),
            "{label}: a group holding a slot contributes NO typed edge, got {:?}",
            sb.requires
        );

        let r1 = OrgRenderer::render_document(
            &first.document,
            &first.blocks,
            Path::new(FILE),
            &first.document.id,
        );
        assert!(
            r1.contains("{{mission}}") && r1.contains("real-dep"),
            "{label}: slot or real id dropped on first write-back:\n{r1}"
        );

        let second = parse(&r1).unwrap_or_else(|e| panic!("{label}: re-parse must succeed: {e:#}"));
        let r2 = OrgRenderer::render_document(
            &second.document,
            &second.blocks,
            Path::new(FILE),
            &second.document.id,
        );
        assert_eq!(
            r1, r2,
            "{label}: write -> read -> write is not a fixed point:\n{r1}\n---\n{r2}"
        );
    }
}

/// The single-value mixed list is the reference shape the cross-key fix funnels
/// into: `:REQUIRES: {{mission}} real-dep` yields NO edge and is carried
/// verbatim, byte-stable across the round trip.
#[test]
fn a_mixed_list_in_one_value_yields_no_edge_and_round_trips() {
    let source = format!("{TEMPLATE_HEAD}:REQUIRES: {{{{mission}}}} real-dep\n:END:\n");
    let first = parse(&source).expect("mixed list must parse inside a template");
    let block = first
        .blocks
        .iter()
        .find(|b| b.id.id() == "tpl-0")
        .expect("template root present");
    assert!(
        block.requires.is_empty(),
        "a value holding a slot contributes no edge, got {:?}",
        block.requires
    );

    let r1 = OrgRenderer::render_document(
        &first.document,
        &first.blocks,
        Path::new(FILE),
        &first.document.id,
    );
    assert!(
        r1.contains(":REQUIRES: {{mission}} real-dep"),
        "the mixed list must be carried verbatim:\n{r1}"
    );
    let second = parse(&r1).expect("re-parse must succeed");
    let r2 = OrgRenderer::render_document(
        &second.document,
        &second.blocks,
        Path::new(FILE),
        &second.document.id,
    );
    assert_eq!(r1, r2, "mixed list is not a fixed point:\n{r1}\n---\n{r2}");
}

/// The slot boundary at the edge tokenizer. A declared slot parses; an
/// undeclared one, an empty `{{}}`, and an unclosed `{{x}` are each a loud
/// error, never silently swallowed. (`{{ mission }}` with inner spaces is NOT a
/// slot: the value tokenizes on whitespace, so it is three slugs — pinned as a
/// refusal so that stays visible.)
#[test]
fn slot_spelling_variants_at_the_edge_boundary() {
    for (label, value, ok) in [
        ("declared", "{{mission}}", true),
        ("undeclared", "{{zzz}}", false),
        ("empty", "{{}}", false),
        ("unclosed", "{{mission}", false),
        ("inner-spaces", "{{ mission }}", false),
    ] {
        let source = format!("{TEMPLATE_HEAD}:contributes-to: {value}\n:END:\n");
        let outcome = parse(&source);
        assert_eq!(
            outcome.is_ok(),
            ok,
            "{label}: `:contributes-to: {value}` expected {}, got {}",
            if ok { "OK" } else { "ERR" },
            if outcome.is_ok() { "OK" } else { "ERR" }
        );
    }
}
