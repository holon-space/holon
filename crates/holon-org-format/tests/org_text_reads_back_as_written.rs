//! What the org renderer writes reads back as the same block, or the render
//! names the block it could not write faithfully.

use std::path::Path;

use holon_api::EntityRef;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::Tags;
use holon_api::block::Block;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const FILE: &str = "/vault/p.org";
const PAGE: &str = "#+ID: p\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n";

fn parse(source: &str) -> (Block, Vec<Block>) {
    let parsed = parse_org_file(
        Path::new(FILE),
        source,
        &EntityUri::no_parent(),
        Path::new("/vault"),
    )
    .unwrap_or_else(|e| panic!("parse of {source:?} failed: {e:#}"));
    (parsed.document, parsed.blocks)
}

fn render(document: &Block, blocks: &[Block]) -> (String, Vec<String>) {
    let r = OrgRenderer::render_document(document, blocks, Path::new(FILE), &document.id)
        .expect("org render");
    (
        r.text,
        r.losses
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
    )
}

fn kid(content: &str) -> Block {
    Block::new_text(
        EntityUri::block("kid"),
        EntityUri::block("topic"),
        content.to_string(),
    )
}

/// Renders `kid` under the page's `Topic` heading and parses the file back.
fn round_trip(page: &str, kid: Block) -> (String, Vec<String>, Vec<Block>) {
    let (document, mut blocks) = parse(page);
    blocks.push(kid);
    let (text, losses) = render(&document, &blocks);
    let (_, back) = parse(&text);
    (text, losses, back)
}

fn text_ids(blocks: &[Block]) -> Vec<String> {
    blocks.iter().map(|b| b.id.id().to_string()).collect()
}

#[test]
fn a_body_line_that_starts_with_a_star_reads_back_as_body_text() {
    let bodies = [
        "Shopping\n* milk",
        "Shopping\n** deep",
        "Shopping\n*",
        "Shopping\n* milk\n* eggs",
        "Shopping\n,* written with a comma",
        "Shopping\n,,* two commas",
        "Shopping\n* a\n,* b\n,,,* c",
        "Shopping\n,not before a star",
        "Shopping\n  * an indented list item",
    ];
    let mut wrong = Vec::new();
    for content in bodies {
        let (text, losses, back) = round_trip(PAGE, kid(content));
        let read = back.iter().find(|b| b.id.id() == "kid");
        let ok = text_ids(&back) == ["topic", "kid"]
            && read.is_some_and(|b| b.content == content)
            && losses.is_empty();
        if !ok {
            wrong.push(format!(
                "{content:?} -> blocks {:?}, kid content {:?}, losses {losses:?}\n{text}",
                text_ids(&back),
                read.map(|b| b.content.clone()),
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n---\n"));
}

#[test]
fn a_star_line_round_trip_is_a_fixed_point() {
    let (text, _, back) = round_trip(PAGE, kid("Shopping\n* milk\n,* eggs"));
    let (document, _) = parse(&text);
    let (again, _) = render(&document, &back);
    assert_eq!(again, text);
}

#[test]
fn a_page_preamble_line_that_starts_with_a_star_reads_back_as_preamble() {
    let (mut document, blocks) = parse("#+ID: p\n\nintro\n");
    let preamble = "intro\n* not a heading\n,* nor this";
    let title = document.content.lines().next().unwrap_or("").to_string();
    document.content = format!("{title}\n{preamble}");
    let (text, _) = render(&document, &blocks);
    let (back, back_blocks) = parse(&text);
    assert!(
        back_blocks.is_empty() && back.content == document.content,
        "page content {:?} read back as {:?} with blocks {:?}:\n{text}",
        document.content,
        back.content,
        text_ids(&back_blocks)
    );
}

#[test]
fn source_block_lines_keep_every_comma() {
    let (document, mut blocks) = parse(PAGE);
    let code = "* a\n,* b\n,,* c\n#+x\n,#+y\n,,#+z\n,plain";
    blocks.push(Block::new_source(
        EntityUri::block("src"),
        EntityUri::block("topic"),
        "python",
        code,
    ));
    let (text, _) = render(&document, &blocks);
    let (_, back) = parse(&text);
    let src = back
        .iter()
        .find(|b| b.id.id() == "src")
        .unwrap_or_else(|| panic!("the source block is gone:\n{text}"));
    assert_eq!(src.content, code, "file:\n{text}");
}

/// A stored title the headline cannot hold: it reads back with a task state,
/// a priority or tags the block does not have.
#[test]
fn a_title_the_headline_reads_differently_is_a_loss() {
    let declared = "#+TODO: NEXT | DONE\n#+ID: p\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n";
    let rows = [
        (PAGE, "TODO buy milk"),
        (PAGE, "DONE it"),
        (PAGE, "[#A] plan"),
        (PAGE, "TODO"),
        (PAGE, ":t:"),
        (PAGE, ":a:b:"),
        (declared, "NEXT x"),
        (PAGE, "trailing space "),
        (PAGE, "  leading spaces"),
        (PAGE, "\ttab first"),
        (PAGE, "Shopping\r\n* milk"),
        (PAGE, "Shopping\n* milk\r"),
        (PAGE, "Shopping\n\ntrailing blank line"),
        (PAGE, "Shopping\nlast line\n"),
        (PAGE, "Shopping\n[[file:photo.png]]"),
    ];
    let mut silent = Vec::new();
    for (page, title) in rows {
        let (text, losses, back) = round_trip(page, kid(title));
        if losses.len() != 1 {
            let read = back.iter().find(|b| b.id.id() == "kid");
            silent.push(format!(
                "{title:?} reads back as {:?}, losses {losses:?}\n{text}",
                read.map(|b| (
                    b.content.clone(),
                    b.task_state().map(|s| s.to_string()),
                    b.priority().map(|p| p.letter()),
                    b.tags().to_vec()
                )),
            ));
        }
    }
    assert!(silent.is_empty(), "{}", silent.join("\n---\n"));
}

#[test]
fn a_headline_that_reads_back_as_written_has_no_loss() {
    let declared = "#+TODO: NEXT | DONE\n#+ID: p\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n";
    let rows: Vec<(&str, &str, Option<&str>, Vec<&str>)> = vec![
        (PAGE, "Plain", None, vec![]),
        (PAGE, "With tags", None, vec!["a", "b"]),
        (PAGE, "Pick:", None, vec!["decision"]),
        (PAGE, "Pick ::", None, vec!["decision"]),
        (PAGE, "Ratio 1:2:", None, vec!["t"]),
        (PAGE, "Meeting :urgent:", None, vec!["later"]),
        (PAGE, "", None, vec!["t"]),
        (PAGE, "URL http://a.b/x", None, vec![]),
        (PAGE, "", Some("TODO"), vec!["t"]),
        (PAGE, "TODO x", Some("TODO"), vec![]),
        (PAGE, ":::", None, vec![]),
        (PAGE, "Foo :::", None, vec![]),
        (declared, "TODO x", None, vec![]),
    ];
    let mut false_losses = Vec::new();
    for (page, title, state, tags) in rows {
        let mut b = kid(title);
        b.set_task_state(state.map(holon_api::TaskState::from_keyword));
        b.set_tags(Tags::from(
            tags.iter().map(|s| (*s).to_string()).collect::<Vec<_>>(),
        ));
        let (text, losses, _) = round_trip(page, b);
        if !losses.is_empty() {
            false_losses.push(format!("{title:?} {state:?} {tags:?}: {losses:?}\n{text}"));
        }
    }
    assert!(false_losses.is_empty(), "{}", false_losses.join("\n---\n"));
}

#[test]
fn a_tag_group_that_names_no_tag_is_title_text() {
    for title in [":::", "Foo :::", "Foo ::::"] {
        let (_, blocks) = parse(&format!(
            "#+ID: p\n* {title}\n:PROPERTIES:\n:ID: k\n:END:\n"
        ));
        let k = blocks.iter().find(|b| b.id.id() == "k").expect("k");
        assert_eq!(
            (k.content.as_str(), k.tags().to_vec()),
            (title, Vec::<String>::new()),
            "headline `* {title}`"
        );
    }
}

#[test]
fn a_keyword_line_in_block_text_reads_back_as_block_text() {
    let file_page = "* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n";
    let rows = [
        (PAGE, "Shopping\n#+TITLE: hijacked"),
        (file_page, "Shopping\n#+ID: hijacked"),
        (PAGE, "Shopping\n#+TODO: Shopping | Done"),
        (PAGE, "Shopping\n#+begin_src\nx\n#+end_src"),
        (PAGE, "Shopping\n#+foo: bar"),
        (PAGE, "Shopping\n,#+foo: bar"),
        (PAGE, "Shopping\n#+begin_example\n* y\n#+end_example"),
        (PAGE, "Shopping\n  #+foo: an indented keyword"),
        (PAGE, "Shopping\n\t,#+foo: indented with a comma"),
        (PAGE, "Shopping\n  #+ID: indented"),
    ];
    let mut wrong = Vec::new();
    for (page, content) in rows {
        let (document, mut blocks) = parse(page);
        let mut sibling = kid("Shopping list for Monday");
        sibling.id = EntityUri::block("sibling");
        blocks.push(kid(content));
        blocks.push(sibling);
        let (text, losses) = render(&document, &blocks);
        let (back_doc, back) = parse(&text);
        let read = |id: &str| {
            back.iter()
                .find(|b| b.id.id() == id)
                .map(|b| (b.content.clone(), b.task_state().map(|s| s.to_string())))
        };
        let ok = back_doc.id == document.id
            && back_doc.content == document.content
            && text_ids(&back) == ["topic", "kid", "sibling"]
            && read("kid") == Some((content.to_string(), None))
            && read("sibling") == Some(("Shopping list for Monday".to_string(), None))
            && losses.is_empty();
        if !ok {
            wrong.push(format!(
                "{content:?} -> page {:?} {:?}, blocks {:?}, kid {:?}, sibling {:?}, losses {losses:?}\n{text}",
                back_doc.id,
                back_doc.content,
                text_ids(&back),
                read("kid"),
                read("sibling"),
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n---\n"));
}

#[test]
fn blanks_the_file_can_hold_read_back_with_no_loss() {
    for content in [
        "Shopping\nlast line has a trailing space ",
        "Shopping\n* milk \t",
        "Shopping\nmiddle \nend",
    ] {
        let (text, losses, back) = round_trip(PAGE, kid(content));
        let read = back
            .iter()
            .find(|b| b.id.id() == "kid")
            .map(|b| b.content.clone());
        assert_eq!(
            (read.as_deref(), losses.len()),
            (Some(content), 0),
            "losses {losses:?}\n{text}"
        );
    }
}

#[test]
fn a_file_the_parser_would_refuse_is_not_rendered() {
    for title in ["[#1] numeric cookie", "[#a] lowercase cookie"] {
        let (document, mut blocks) = parse(PAGE);
        blocks.push(kid(title));
        let rendered =
            OrgRenderer::render_document(&document, &blocks, Path::new(FILE), &document.id);
        let err = match rendered {
            Ok(r) => panic!("{title:?} rendered a file:\n{}", r.text),
            Err(e) => format!("{e:#}"),
        };
        assert!(err.contains("refused"), "{title:?}: {err}");
    }
}

#[test]
fn a_task_state_the_file_does_not_declare_is_a_loss() {
    let declared = "#+TODO: NEXT | DONE\n#+ID: p\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n";
    let mut b = kid("buy milk");
    b.set_task_state(Some(holon_api::TaskState::from_keyword("TODO")));
    let (text, losses, _) = round_trip(declared, b);
    assert_eq!(losses.len(), 1, "losses {losses:?}\n{text}");
}

#[test]
fn a_file_line_org_reads_as_text_is_written_back_unchanged() {
    for line in [
        "*bold* at the start",
        "**bold** at the start",
        "#+begin_example",
        "#+end_quote",
    ] {
        let file = format!("{PAGE}** Shopping\n:PROPERTIES:\n:ID: kid\n:END:\n{line}\n");
        let (document, blocks) = parse(&file);
        let (text, losses) = render(&document, &blocks);
        assert_eq!(
            (text.as_str(), losses.len()),
            (file.as_str(), 0),
            "{line:?}"
        );
    }
}

fn marked_kid(content: &str, marks: Vec<MarkSpan>) -> Block {
    let mut b = kid(content);
    b.marks = Some(marks);
    b
}

fn link_over(content: &str) -> Vec<MarkSpan> {
    vec![MarkSpan::new(
        0,
        content.chars().count(),
        InlineMark::Link {
            target: EntityRef::Scheme {
                raw: "block:target-1".to_string(),
            },
            label: content.to_string(),
        },
    )]
}

#[test]
fn a_mark_across_an_escaped_line_reads_back_exactly() {
    let bold = |start, end| vec![MarkSpan::new(start, end, InlineMark::Bold)];
    let cases = [
        (
            "a link label\nplain\ntail",
            link_over("a link label\nplain\ntail"),
        ),
        (
            "a link label\n#+p: v\ntail",
            link_over("a link label\n#+p: v\ntail"),
        ),
        (
            "a link label\n* milk\ntail",
            link_over("a link label\n* milk\ntail"),
        ),
        (
            "a link label\n,* written with a comma\ntail",
            link_over("a link label\n,* written with a comma\ntail"),
        ),
        (
            "a link label\n#+ID: x\ntail",
            link_over("a link label\n#+ID: x\ntail"),
        ),
        ("#+p: v\n* milk\ntail", link_over("#+p: v\n* milk\ntail")),
        ("tail\n* milk", bold(7, 11)),
        ("tail\n#+p: v w", bold(10, 11)),
    ];
    let mut wrong = Vec::new();
    for (content, marks) in cases {
        let sent = marked_kid(content, marks);
        let (text, losses, back) = round_trip(PAGE, sent.clone());
        let read = back.iter().find(|b| b.id.id() == "kid");
        let ok = read.is_some_and(|b| b.content == sent.content && b.marks == sent.marks)
            && losses.is_empty();
        if !ok {
            wrong.push(format!(
                "{content:?} {:?} -> {:?}, losses {losses:?}\n{text}",
                sent.marks,
                read.map(|b| (&b.content, &b.marks))
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n---\n"));
}

#[test]
fn a_mark_the_file_cannot_hold_is_a_loss() {
    let content = "a link label\n- a list item\ntail";
    let (text, losses, back) = round_trip(PAGE, marked_kid(content, link_over(content)));
    let read = back.iter().find(|b| b.id.id() == "kid");
    assert!(
        read.is_some_and(|b| b.marks.is_some()) || !losses.is_empty(),
        "the link was dropped with no loss: {:?}\n{text}",
        read.map(|b| &b.marks)
    );
}

#[test]
fn an_unchanged_file_keeps_its_blank_lines() {
    let kid = |n: &str| format!("** K{n}\n:PROPERTIES:\n:ID: k{n}\n:END:\n");
    let files = [
        format!("{PAGE}{}body\n\n{}second\n", kid("1"), kid("2")),
        format!("{PAGE}{}body\n\n\n{}second\n", kid("1"), kid("2")),
        format!("{PAGE}{}\n{}second\n", kid("1"), kid("2")),
        format!("{PAGE}{}- a\n\n\n{}second\n", kid("1"), kid("2")),
        format!(
            "{PAGE}{}body\n\n*** C\n:PROPERTIES:\n:ID: c\n:END:\nc\n",
            kid("1")
        ),
        format!(
            "{PAGE}{}body\n#+BEGIN_SRC rust :id k1::src::0\nx\n#+END_SRC\n\n{}second\n",
            kid("1"),
            kid("2")
        ),
        format!("{PAGE}{}\nbody\n", kid("1")),
        format!("{PAGE}{}\n\nbody\n\n{}second\n", kid("1"), kid("2")),
        "#+ID: p\n\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n".to_string(),
        format!("{PAGE}{}body\n\n", kid("1")),
        format!("{PAGE}{}body\n\n\n", kid("1")),
        format!("{PAGE}{}- a\n\n\n", kid("1")),
        format!("{PAGE}{}body\n  \n{}second\n", kid("1"), kid("2")),
        format!("{PAGE}{}body\n\t\n \n", kid("1")),
        format!("{PAGE}{}\n  \nbody\n", kid("1")),
    ];
    let mut wrong = Vec::new();
    for file in &files {
        let (document, blocks) = parse(file);
        let (text, losses) = render(&document, &blocks);
        if text != *file || !losses.is_empty() {
            wrong.push(format!(
                "--file--\n{file}--written--\n{text}--losses {losses:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_page_id_line_in_block_text_stays_block_text() {
    let file_page = "* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n";
    let body = "** Kid\n:PROPERTIES:\n:ID: kid\n:END:\nbody\n#+ID: hijacked\n";
    for page in [PAGE, file_page] {
        let file = format!("{page}{body}");
        let (document, blocks) = parse(&file);
        let (bare, _) = parse(page);
        let (text, losses) = render(&document, &blocks);
        let kid = blocks
            .iter()
            .find(|b| b.id.id() == "kid")
            .map(|b| &b.content);
        assert_eq!(
            (
                &document.id,
                kid.map(String::as_str),
                text.as_str(),
                losses.len()
            ),
            (
                &bare.id,
                Some("Kid\nbody\n#+ID: hijacked"),
                file.as_str(),
                0
            ),
        );
    }
}

/// The page id is the `#+ID:` keyword of the text before the first headline as
/// org reads it: a line inside a block there is no keyword, and a line org
/// reads as a headline ends that text.
#[test]
fn the_page_id_is_read_where_org_reads_the_preamble() {
    let mut wrong = Vec::new();
    for (open, close) in [
        ("#+begin_src org", "#+end_src"),
        ("#+begin_example", "#+end_example"),
        ("#+begin_quote", "#+end_quote"),
        ("#+begin_verse", "#+end_verse"),
        ("#+begin_comment", "#+end_comment"),
        ("#+begin_center", "#+end_center"),
        ("#+begin_export html", "#+end_export"),
    ] {
        for (inner, id) in [
            ("*", "block:p"),
            ("#+ID: q", "block:p"),
            ("* x", "file:p.org"),
        ] {
            let file = format!(
                "{open}\n{inner}\n{close}\n#+ID: p\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n"
            );
            let (document, _) = parse(&file);
            let probed = holon_org_format::parser::parse_doc_uri_any_carrier(&file)
                .expect("probe")
                .map(|u| u.to_string());
            let parsed = document.id.to_string();
            let expected_probe = (id != "file:p.org").then(|| id.to_string());
            if !parsed.starts_with(id) || probed != expected_probe {
                wrong.push(format!(
                    "{file:?}: parsed {parsed}, probed {probed:?}, want {id}"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
