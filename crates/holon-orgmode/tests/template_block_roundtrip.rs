//! Templates are ordinary blocks (docs/Proposals/Templating-2026-07-12.md §7):
//! a template subtree — `template`/`template_vars` properties + `{{var}}`
//! slots in content — must round-trip through the org adapter with NO
//! special-casing. This is the regression gate for that claim.

use std::path::Path;

use holon_api::entity_uri::EntityUri;
use holon_core::file_format::FileFormatAdapter;
use holon_orgmode::file_format::OrgFormatAdapter;

const TEMPLATE_ORG: &str = "\
* {{date}}
:PROPERTIES:
:ID: journal-day-template
:TEMPLATE: daily-journal
:TEMPLATE_VARS: date, mood=neutral
:END:
** Agenda for {{date}}
:PROPERTIES:
:ID: journal-day-template-agenda
:END:
** Mood: {{mood}}
:PROPERTIES:
:ID: journal-day-template-mood
:END:
";

#[test]
fn template_subtree_round_trips_through_org_adapter() {
    let adapter = OrgFormatAdapter;
    let parent_dir = EntityUri::no_parent();
    let root = Path::new("/vault");
    let path = Path::new("/vault/templates.org");

    let first = adapter
        .parse(path, TEMPLATE_ORG, &parent_dir, root)
        .expect("template org must parse like any org file");
    assert!(
        first.blocks_needing_ids.is_empty(),
        "all template blocks carry explicit ids"
    );

    let tpl_root = first
        .blocks
        .iter()
        .find(|b| b.content.contains("{{date}}") && !b.content.contains("Agenda"))
        .expect("template root block parsed");
    assert_eq!(
        tpl_root.content, "{{date}}",
        "variable slot survives ingest verbatim"
    );
    let props = tpl_root.properties_map();
    let template_prop = props
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("template"))
        .map(|(_, v)| format!("{v:?}"));
    assert!(
        template_prop.is_some(),
        "TEMPLATE drawer property must land in block properties, got: {props:?}"
    );
    let vars_prop = props
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("template_vars"));
    assert!(
        vars_prop.is_some(),
        "TEMPLATE_VARS drawer property must land in block properties, got: {props:?}"
    );

    // Render back and re-parse: slots and declarations must be stable. Render
    // under the file id the parser stamped as the roots' parent, so the
    // renderer's parent-chain invariant (roots must parent to the file node)
    // holds.
    let file_id = EntityUri::from_raw(tpl_root.parent_id.as_str());
    let rendered = adapter.render_document(&first.document, &first.blocks, path, &file_id);
    assert!(
        rendered.contains("{{date}}") && rendered.contains("{{mood}}"),
        "slots must render verbatim, got:\n{rendered}"
    );
    assert!(
        rendered.to_ascii_uppercase().contains(":TEMPLATE:"),
        "template marker drawer must render, got:\n{rendered}"
    );

    let second = adapter
        .parse(path, &rendered, &parent_dir, root)
        .expect("rendered template must re-parse");
    let reparsed_root = second
        .blocks
        .iter()
        .find(|b| b.content == "{{date}}")
        .expect("template root survives the round trip");
    let reparsed_props = reparsed_root.properties_map();
    let reparsed_vars = reparsed_props
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("template_vars"))
        .map(|(_, v)| format!("{v:?}"))
        .expect("template_vars survives the round trip");
    assert!(
        reparsed_vars.contains("date") && reparsed_vars.contains("mood=neutral"),
        "variable declarations must survive verbatim, got: {reparsed_vars}"
    );
}
