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

// -- W3(a): template with TODO + bold marks + [[link]] marks in children ---

const TPL_WITH_MARKS_ORG: &str = "\
* {{greeting}}
:PROPERTIES:
:ID: tpl-marks-root
:TEMPLATE: marks-template
:TEMPLATE_VARS: greeting
:END:
** TODO Review *{{greeting}}* notes
:PROPERTIES:
:ID: tpl-marks-todo
:END:
** Check [[https://example.com][the docs]] for {{greeting}}
:PROPERTIES:
:ID: tpl-marks-link
:END:
";

#[test]
fn template_with_marks_round_trips_stable() {
    let adapter = OrgFormatAdapter;
    let parent_dir = EntityUri::no_parent();
    let root = Path::new("/vault");
    let path = Path::new("/vault/templates.org");

    let first = adapter
        .parse(path, TPL_WITH_MARKS_ORG, &parent_dir, root)
        .expect("template with marks must parse");
    assert!(first.blocks_needing_ids.is_empty());

    // Verify the TODO child by its known ID. The org parser strips the
    // TODO keyword from content into a task_state property, so we cannot
    // search content for "TODO" — match by the bare ID instead.
    let todo_child = first
        .blocks
        .iter()
        .find(|b| b.id.id() == "tpl-marks-todo")
        .expect("TODO child must exist");
    assert!(
        todo_child.content.contains("{{greeting}}"),
        "TODO child slot must survive, got: {}",
        todo_child.content
    );

    // Verify the link child by its known ID.
    let link_child = first
        .blocks
        .iter()
        .find(|b| b.id.id() == "tpl-marks-link")
        .expect("link child must exist");
    assert!(
        link_child.content.contains("{{greeting}}"),
        "link child slot must survive"
    );

    // Render back and re-parse.
    let tpl_root = first
        .blocks
        .iter()
        .find(|b| b.id.id() == "tpl-marks-root")
        .expect("template root");
    let file_id = EntityUri::from_raw(tpl_root.parent_id.as_str());
    let rendered =
        adapter.render_document(&first.document, &first.blocks, path, &file_id);
    assert!(
        rendered.contains("{{greeting}}"),
        "slots must render verbatim, got:\n{rendered}"
    );
    assert!(
        rendered.contains("*{{greeting}}*"),
        "bold marks must render, got:\n{rendered}"
    );
    assert!(
        rendered.contains("[[https://example.com][the docs]]"),
        "link marks must render, got:\n{rendered}"
    );

    let second = adapter
        .parse(path, &rendered, &parent_dir, root)
        .expect("rendered template with marks must re-parse");
    let reparsed_todo = second
        .blocks
        .iter()
        .find(|b| b.id.id() == "tpl-marks-todo")
        .expect("TODO child survives round-trip");
    assert!(
        reparsed_todo.content.contains("{{greeting}}"),
        "TODO child slot survives re-parse, got: {}",
        reparsed_todo.content
    );
    let reparsed_link = second
        .blocks
        .iter()
        .find(|b| b.id.id() == "tpl-marks-link")
        .expect("link child survives round-trip");
    assert!(
        reparsed_link.content.contains("{{greeting}}"),
        "link child slot survives re-parse"
    );
}

// -- W3(b): underscore in placeholder — regression against _→subscript -----

const TPL_UNDERSCORE_VAR_ORG: &str = "\
* {{my_var}}
:PROPERTIES:
:ID: tpl-underscore-root
:TEMPLATE: underscore-test
:TEMPLATE_VARS: my_var
:END:
** Value: {{my_var}}
:PROPERTIES:
:ID: tpl-underscore-child
:END:
";

#[test]
fn template_with_underscore_var_round_trips_verbatim() {
    let adapter = OrgFormatAdapter;
    let parent_dir = EntityUri::no_parent();
    let root = Path::new("/vault");
    let path = Path::new("/vault/templates.org");

    let first = adapter
        .parse(path, TPL_UNDERSCORE_VAR_ORG, &parent_dir, root)
        .expect("underscore template must parse");

    let tpl_root = first
        .blocks
        .iter()
        .find(|b| b.content.contains("{{my_var}}")
            && !b.content.contains("Value:"))
        .expect("template root");
    assert_eq!(
        tpl_root.content, "{{my_var}}",
        "underscore placeholder must survive parse verbatim"
    );

    let file_id = EntityUri::from_raw(tpl_root.parent_id.as_str());
    let rendered =
        adapter.render_document(&first.document, &first.blocks, path, &file_id);

    // If the underscore gets mangled (e.g. _ → subscript), {{my_var}} would
    // no longer appear verbatim. This is the regression gate.
    if !rendered.contains("{{my_var}}") {
        // Capture how it was mangled for diagnosis.
        let line_with_var = rendered
            .lines()
            .find(|l| l.contains("my") && l.contains("var"))
            .unwrap_or("(not found)");
        assert!(
            rendered.contains("{{my_var}}"),
            "underscore placeholder must NOT be mangled by org renderer; \
             line content: '{line_with_var}'\nfull render:\n{rendered}"
        );
    }

    let second = adapter
        .parse(path, &rendered, &parent_dir, root)
        .expect("rendered underscore template must re-parse");
    let reparsed = second
        .blocks
        .iter()
        .find(|b| b.content == "{{my_var}}")
        .expect("underscore placeholder survives round-trip");
    assert_eq!(reparsed.content, "{{my_var}}");
}

// -- W3(c): no-vars template (TEMPLATE drawer, no TEMPLATE_VARS) -----------

const TPL_NO_VARS_ORG: &str = "\
* Static Greeting
:PROPERTIES:
:ID: tpl-no-vars-root
:TEMPLATE: static-greeting
:END:
** Hello, World
:PROPERTIES:
:ID: tpl-no-vars-child
:END:
";

#[test]
fn template_without_vars_round_trips_and_has_no_template_vars_property() {
    let adapter = OrgFormatAdapter;
    let parent_dir = EntityUri::no_parent();
    let root = Path::new("/vault");
    let path = Path::new("/vault/templates.org");

    let first = adapter
        .parse(path, TPL_NO_VARS_ORG, &parent_dir, root)
        .expect("no-vars template must parse");
    assert!(first.blocks_needing_ids.is_empty());

    let tpl_root = first
        .blocks
        .iter()
        .find(|b| b.content == "Static Greeting")
        .expect("template root");
    let props = tpl_root.properties_map();
    let template_prop = props
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("template"));
    assert!(
        template_prop.is_some(),
        "TEMPLATE marker must land in properties, got: {props:?}"
    );
    let vars_prop = props
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("template_vars"));
    assert!(
        vars_prop.is_none(),
        "TEMPLATE_VARS must be absent when not declared, got: {props:?}"
    );

    // Round-trip: render → re-parse.
    let file_id = EntityUri::from_raw(tpl_root.parent_id.as_str());
    let rendered =
        adapter.render_document(&first.document, &first.blocks, path, &file_id);
    assert!(
        rendered.to_ascii_uppercase().contains(":TEMPLATE:"),
        "TEMPLATE marker must survive render, got:\n{rendered}"
    );
    assert!(
        !rendered.to_ascii_uppercase().contains(":TEMPLATE_VARS:"),
        "TEMPLATE_VARS must NOT appear when absent, got:\n{rendered}"
    );

    let second = adapter
        .parse(path, &rendered, &parent_dir, root)
        .expect("rendered no-vars template must re-parse");
    let reparsed = second
        .blocks
        .iter()
        .find(|b| b.content == "Static Greeting")
        .expect("template root survives re-parse");
    let reparsed_props = reparsed.properties_map();
    assert!(
        reparsed_props
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("template")),
        "TEMPLATE survives re-parse"
    );
    assert!(
        !reparsed_props
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("template_vars")),
        "TEMPLATE_VARS must stay absent after re-parse, got: {reparsed_props:?}"
    );
}
