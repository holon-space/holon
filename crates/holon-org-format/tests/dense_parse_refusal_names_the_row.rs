//! A dense text the org parser refuses is refused by the row the agent wrote,
//! never by an id the parse minted.

use holon_org_format::parse_dense;

fn uuid_in(msg: &str) -> Option<&str> {
    msg.split(|c: char| !(c.is_ascii_hexdigit() || c == '-'))
        .find(|word| word.len() == 36 && word.matches('-').count() == 4)
}

fn refusal(text: &str) -> String {
    match parse_dense(text) {
        Ok(_) => format!("parsed:\n{text}"),
        Err(e) => format!("{e:#}"),
    }
}

#[test]
fn every_parse_refusal_names_its_row() {
    let existing = |headline: &str, drawer: &str| {
        format!(
            "#+TITLE: P\n* Other {{#1}}\n* {headline} {{#0}}\n:PROPERTIES:\n{drawer}\n:END:\nbody\n"
        )
    };
    let cases: Vec<(&str, String, &str, &str)> = vec![
        (
            "task_state key",
            existing("Plan", ":owner: me\n:Task_State: x"),
            "row {#0}: ",
            "Task_State",
        ),
        (
            "task_state key in upper case",
            existing("Plan", ":TASK_STATE: x"),
            "row {#0}: ",
            "TASK_STATE",
        ),
        (
            "task_state_category key",
            existing("Plan", ":task_state_category: done"),
            "row {#0}: ",
            "task_state_category",
        ),
        (
            "WIDGET_ONLY value",
            existing("Plan", ":WIDGET_ONLY: banana"),
            "row {#0}: ",
            "banana",
        ),
        (
            "drawer priority",
            existing("Plan", ":priority: 1"),
            "row {#0}: ",
            "priority",
        ),
        (
            "two drawer priorities",
            existing("Plan", ":PRIORITY: A\n:priority: B"),
            "row {#0}: ",
            "disagree",
        ),
        (
            "cookie and drawer priority",
            existing("[#A] Plan", ":priority: B"),
            "row {#0}: ",
            "disagree",
        ),
        (
            "edge slot outside a template",
            existing("Plan", ":REQUIRES: {{mission}}"),
            "row {#0}: ",
            "REQUIRES",
        ),
        (
            "schemed id",
            existing("Plan", ":ID: block:x"),
            "row {#0}: ",
            "block:x",
        ),
        (
            "an id two rows claim",
            "#+TITLE: P\n* Other {#1}\n:PROPERTIES:\n:ID: same\n:END:\n* Plan {#0}\n:PROPERTIES:\n:ID: same\n:END:\n"
                .to_string(),
            "row {#0}: ",
            "same",
        ),
        (
            "task_state key with tags after the token",
            "#+TITLE: P\n* Other {#1}\n* TODO Plan {#0} :alpha:\n:PROPERTIES:\n:Task_State: x\n:END:\n"
                .to_string(),
            "row {#0}: ",
            "Task_State",
        ),
        (
            "task_state key with tags on a new row",
            "#+TITLE: P\n* Other {#1}\n* Fresh :kt:\n:PROPERTIES:\n:task_state: x\n:END:\n"
                .to_string(),
            "new row \"Fresh\": ",
            "task_state",
        ),
        (
            "task_state key on a new row",
            "#+TITLE: P\n* Other {#1}\n* Fresh\n:PROPERTIES:\n:task_state: x\n:END:\n".to_string(),
            "new row \"Fresh\": ",
            "task_state",
        ),
    ];
    let mut broken = Vec::new();
    for (name, text, label, cause) in cases {
        let msg = refusal(&text);
        if !msg.starts_with(label) || !msg.contains(cause) {
            broken.push(format!("{name}: wanted `{label}…{cause}…`, got: {msg}"));
        } else if let Some(minted) = uuid_in(&msg) {
            broken.push(format!("{name}: names the minted id {minted}: {msg}"));
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

/// A line of stars is a row only when a space follows the stars, as org
/// reads it (emacs 30.2): `dense_rows` splits the text into the rows the
/// parser reads.
#[test]
fn a_line_of_stars_is_a_row_only_with_a_space_after_the_stars() {
    for (line, is_row) in [
        ("*", false),
        ("**", false),
        ("*****", false),
        ("*\t", false),
        ("*\tTab", false),
        ("**\tx", false),
        ("*x", false),
        ("* ", true),
        ("** ", true),
        ("* x", true),
        ("*  x", true),
    ] {
        let text = format!("* Plan {{#0}}\nbody\n{line}\n* Other {{#1}}\n");
        let rows = holon_org_format::dense_rows(&text);
        let parsed = parse_dense(&text).unwrap_or_else(|e| panic!("{line:?}: {e:#}"));
        assert_eq!(
            rows.len(),
            parsed.blocks.len(),
            "{line:?}: dense_rows splits {rows:?}, the parser reads {} rows",
            parsed.blocks.len()
        );
        assert_eq!(rows.len(), if is_row { 3 } else { 2 }, "{line:?}: {rows:?}");
    }
}
