//! The marks `parse_dense` gives a row index the row's content, whose title
//! has no `{#alias}` token or tag group: each span covers the text the markup
//! wrapped.

use holon_org_format::parse_dense;

fn marked_texts(text: &str) -> Vec<String> {
    let parsed = parse_dense(text).expect("dense text parses");
    let block = &parsed.blocks[0].block;
    let chars: Vec<char> = block.content.chars().collect();
    block
        .marks
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|span| chars[span.start..span.end].iter().collect())
        .collect()
}

#[test]
fn a_body_mark_spans_its_text_under_a_tokened_title() {
    assert_eq!(marked_texts("* Plan {#0}\na *bold* word\n"), ["bold"]);
}

#[test]
fn a_body_mark_spans_its_text_under_a_tagged_tokened_title() {
    assert_eq!(
        marked_texts("* Plan :alpha:beta: {#0}\na /slanted/ word\n"),
        ["slanted"]
    );
}

#[test]
fn a_title_mark_spans_its_text() {
    assert_eq!(marked_texts("* A *bold* title {#0}\nbody\n"), ["bold"]);
}
