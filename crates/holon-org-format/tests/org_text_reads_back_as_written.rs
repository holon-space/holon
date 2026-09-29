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
        "Shopping\n:PROPERTIES:\n* heading\n:END:",
        "Shopping\n:NOTES:\n* heading\n,* comma\n:END:",
        "Shopping\n#+begin_quote\n* heading\n#+end_quote",
        "Shopping\n#+begin_verse\n** heading\n#+end_verse",
        "Shopping\n#+begin_center\n:D:\n*\n:END:\n#+end_center",
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
    let preamble = "intro\n* not a heading\n,* nor this\n:NOTES:\n* nor in a drawer\n:END:";
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

const HEADER: &str = "#+ID: p\n#+title: lower case title\n\n#+FILETAGS: :a:b:\n#+STARTUP: fold\n#+AUTHOR: someone\n#+TODO: TODO(t) NEXT | DONE(d)\n#+ID: second\n#+OPTIONS: toc:nil\n\n";

/// Every keyword line before the first headline keeps its bytes and its place,
/// the ones Holon reads included, while Holon's meaning of them is unchanged.
#[test]
fn a_page_header_keeps_every_authored_keyword_line() {
    let file = format!("{HEADER}* Topic\n:PROPERTIES:\n:ID: topic\n:END:\nbody\n");
    let (document, blocks) = parse(&file);
    let (text, losses) = render(&document, &blocks);
    assert_eq!((text.as_str(), losses.len()), (file.as_str(), 0));
}

/// A header value Holon changes is written in the line that declared it; every
/// other authored line stays.
#[test]
fn a_changed_header_value_is_written_in_its_own_line() {
    use holon_org_format::OrgDocumentExt;
    let file = format!("{HEADER}* Topic\n:PROPERTIES:\n:ID: topic\n:END:\nbody\n");
    let (mut document, blocks) = parse(&file);
    document.set_todo_keywords(Some(vec![
        holon_api::TaskState::active("TODO"),
        holon_api::TaskState::done("DONE"),
    ]));
    let (text, losses) = render(&document, &blocks);
    let want = file.replace(
        "#+TODO: TODO(t) NEXT | DONE(d)\n",
        "#+TODO: TODO(t) | DONE(d)\n",
    );
    assert_eq!((text.as_str(), losses.len()), (want.as_str(), 0));
}

/// Org keywords are case-insensitive: `#+id:` names the page, and keeps its
/// spelling.
#[test]
fn a_page_id_keyword_in_any_case_names_the_page() {
    for key in ["#+id:", "#+Id:", "#+iD:"] {
        let file = format!("{key} p\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n");
        let (document, blocks) = parse(&file);
        let (text, losses) = render(&document, &blocks);
        assert_eq!(
            (document.id.as_str(), text.as_str(), losses.len()),
            ("block:p", file.as_str(), 0),
            "{key}"
        );
    }
}

/// A file with CRLF line breaks is written with CRLF line breaks; a file that
/// mixes them cannot be written as it is, and says so.
#[test]
fn line_breaks_are_kept_or_the_file_reports_a_loss() {
    let crlf = "#+ID: p\r\n#+FILETAGS: :a:\r\n* Topic\r\n:PROPERTIES:\r\n:ID: topic\r\n:END:\r\nb1\r\nb2\r\n\r\n** Kid\r\n:PROPERTIES:\r\n:ID: kid\r\n:END:\r\nb3\r\n";
    let (document, blocks) = parse(crlf);
    let (text, losses) = render(&document, &blocks);
    assert_eq!((text.as_str(), losses.len()), (crlf, 0));

    let mixed = "#+ID: p\r\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\nb1\r\n";
    let (document, blocks) = parse(mixed);
    let (text, losses) = render(&document, &blocks);
    assert!(
        text != mixed && losses.iter().any(|l| l.contains("line break")),
        "losses {losses:?}\n{text:?}"
    );
}

#[test]
fn a_header_value_the_file_cannot_hold_is_a_loss() {
    use holon_org_format::OrgDocumentExt;
    let (mut document, blocks) = parse(&format!(
        "{HEADER}* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n"
    ));
    document.set_file_title(Some("two\nlines".to_string()));
    let (text, losses) = render(&document, &blocks);
    assert_eq!(losses.len(), 1, "{text}");
}

/// A keyword line written in Emacs in a block's text is not block text: it is
/// written back raw, in its place, while a line typed into the text stays text
/// and is written comma-escaped.
#[test]
fn an_emacs_keyword_line_in_block_text_is_written_back_raw() {
    let kid = |n: &str| format!("** K{n}\n:PROPERTIES:\n:ID: k{n}\n:END:\n");
    let files = [
        format!("{PAGE}{}#+FOO: bar\ntext\n", kid("1")),
        format!("{PAGE}{}text\n#+STARTUP: fold\n", kid("1")),
        format!("{PAGE}{}#+STARTUP: fold\n", kid("1")),
        format!("{PAGE}{}a\n\n#+FOO: x\n\nb\n", kid("1")),
        format!("{PAGE}{}#+CAPTION: cap\n| a |\n", kid("1")),
        format!(
            "{PAGE}{}#+foo: lower\ntext\n{}#+BAR: two\n",
            kid("1"),
            kid("2")
        ),
        format!("{PAGE}{}text\n,#+typed: t\n#+RAW: r\nend\n", kid("1")),
        format!("{PAGE}{}- a\n#+FOO: x\n\n{}second\n", kid("1"), kid("2")),
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

    let (_, blocks) = parse(&format!(
        "{PAGE}{}text\n,#+typed: t\n#+RAW: r\nend\n",
        kid("1")
    ));
    let content = blocks
        .iter()
        .find(|b| b.id.id() == "k1")
        .map(|b| b.content.as_str());
    assert_eq!(content, Some("K1\ntext\n#+typed: t\nend"));
}

#[test]
fn a_keyword_line_the_file_reads_as_text_is_a_loss() {
    let mut b = kid("Shopping\nmilk");
    b.set_keyword_lines(vec![holon_org_format::models::KeywordLine {
        before_line: 1,
        raw: "not a keyword\n".to_string(),
        after_source: false,
    }]);
    let (text, losses, _) = round_trip(PAGE, b);
    assert_eq!(losses.len(), 1, "{text}");
}

fn assert_unchanged(files: &[String]) {
    let mut wrong = Vec::new();
    for file in files {
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

/// Block text holding a private-use character (a pasted icon glyph) reads and
/// writes back like any other text.
#[test]
fn text_with_a_private_use_character_reads_back() {
    let kid = "** K1\n:PROPERTIES:\n:ID: k1\n:END:\n";
    assert_unchanged(&[
        format!("{PAGE}{kid}\u{E000}text\n"),
        format!("{PAGE}{kid}\u{E000}0\n"),
        format!("{PAGE}{kid}\u{E000}9\n"),
        format!("{PAGE}{kid}a\n#+FOO: x\n\u{E000}0\nb\n"),
        "#+ID: p\n\u{E000} a note with a glyph\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n"
            .to_string(),
    ]);
}

fn body_line() -> proptest::strategy::BoxedStrategy<String> {
    use proptest::prelude::*;
    prop_oneof![
        proptest::string::string_regex("[\u{E000}-\u{E00F}#+*:a-z0-9 \t,|\\[\\]-]{0,12}")
            .expect("regex"),
        any::<String>().prop_map(|s| s.replace(['\n', '\r'], "")),
        Just("#+FOO: x".to_string()),
        Just("#+begin_src".to_string()),
        Just("#+end_src".to_string()),
        Just("\u{E000}0".to_string()),
    ]
    .boxed()
}

proptest::proptest! {
    /// The parser and the renderer never panic, whatever a block's text holds.
    #[test]
    fn any_block_text_parses_and_renders_without_a_panic(
        lines in proptest::collection::vec(body_line(), 0..6),
        preamble in proptest::prelude::any::<bool>(),
    ) {
        let text = lines.join("\n");
        let file = if preamble {
            format!("#+ID: p\n{text}\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n")
        } else {
            format!("{PAGE}** K1\n:PROPERTIES:\n:ID: k1\n:END:\n{text}\n")
        };
        if let Ok(parsed) =
            parse_org_file(Path::new(FILE), &file, &EntityUri::no_parent(), Path::new("/vault"))
        {
            let _ = OrgRenderer::render_document(
                &parsed.document,
                &parsed.blocks,
                Path::new(FILE),
                &parsed.document.id,
            );
        }
    }
}

/// A comma-escaped `#+TITLE:` or `#+TODO:` line inside a block is text of that
/// block, and org collects no `#+TODO:` line inside an example block.
#[test]
fn an_escaped_keyword_line_is_no_page_value() {
    use holon_org_format::OrgDocumentExt;
    let rest = "* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n";
    let as_written = [
        format!("#+ID: p\n#+BEGIN_SRC org :id p::src::0\n,#+TITLE: inside\n#+END_SRC\n{rest}"),
        format!("#+ID: p\n#+begin_example\n,#+TODO: X | Y\n#+end_example\n{rest}"),
        format!("#+ID: p\n{rest}#+begin_example\n,#+TITLE: deep\n#+end_example\n"),
    ];
    let raw_ring = format!("#+ID: p\n#+begin_example\n#+TODO: X | Y\n#+end_example\n{rest}");
    for file in as_written.iter().chain([&raw_ring]) {
        let (document, _) = parse(file);
        assert_eq!(
            (document.file_title(), document.todo_keywords()),
            (None, None),
            "{file}"
        );
    }
    assert_unchanged(&as_written);
}

/// Org keywords are case-insensitive: `#+title:`, `#+todo:` and `#+seq_todo:`
/// declare the page's title and task keywords, and keep their spelling.
#[test]
fn page_keywords_are_read_in_any_case() {
    use holon_org_format::OrgDocumentExt;
    let (document, _) = parse("#+title: Lower\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n");
    assert_eq!(document.file_title().as_deref(), Some("Lower"));
    for key in ["#+todo:", "#+seq_todo:", "#+Todo:"] {
        let file =
            format!("{key} WAIT | SHIPPED\n* SHIPPED Topic\n:PROPERTIES:\n:ID: topic\n:END:\n");
        let (_, blocks) = parse(&file);
        let topic = blocks.iter().find(|b| b.id.id() == "topic").expect("topic");
        assert_eq!(
            (
                topic.content.as_str(),
                topic.task_state().map(|s| s.keyword)
            ),
            ("Topic", Some("SHIPPED".to_string())),
            "{key}"
        );
    }
    assert_unchanged(&[
        "#+title: Lower\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n".to_string(),
        "#+seq_todo: WAIT | SHIPPED\n* SHIPPED Topic\n:PROPERTIES:\n:ID: topic\n:END:\n"
            .to_string(),
    ]);
    let (mut document, blocks) =
        parse("#+seq_todo: WAIT | SHIPPED\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n");
    document.set_todo_keywords(Some(vec![
        holon_api::TaskState::active("A"),
        holon_api::TaskState::done("C"),
    ]));
    let (text, losses) = render(&document, &blocks);
    assert_eq!(
        (text.as_str(), losses.len()),
        (
            "#+seq_todo: A | C\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n",
            0
        )
    );
}

/// A keyword line before the first headline keeps its place among the text
/// there.
#[test]
fn preamble_keyword_lines_keep_their_order() {
    let rest = "* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n";
    assert_unchanged(&[
        format!("#+ID: p\n# a comment line\n#+STARTUP: fold\n{rest}"),
        format!("#+ID: p\n- a list item\n#+STARTUP: fold\n{rest}"),
        format!("#+ID: p\n:LOGBOOK:\nx\n:END:\n#+STARTUP: fold\n{rest}"),
        format!("#+ID: p\nsome intro text\n#+STARTUP: fold\nmore\n{rest}"),
    ]);
}

/// A keyword line after a source block child cannot be written there; the
/// render says so.
#[test]
fn a_keyword_line_after_a_source_child_is_a_loss() {
    let file = format!(
        "{PAGE}** K1\n:PROPERTIES:\n:ID: k1\n:END:\ntext\n#+BEGIN_SRC rust :id k1::src::0\nx\n#+END_SRC\n#+FOO: after\n"
    );
    let (document, blocks) = parse(&file);
    let (text, losses) = render(&document, &blocks);
    assert!(text == file || !losses.is_empty(), "silent: {text}");
}

/// A drawer value keeps its authored spacing while its value is unchanged.
#[test]
fn a_drawer_value_keeps_its_authored_spacing() {
    let file = |line: &str| format!("{PAGE}** K1\n:PROPERTIES:\n:ID: k1\n{line}\n:END:\n");
    assert_unchanged(&[
        file(":NOTE: two "),
        file(":NOTE:  lead"),
        file(":NOTE: a\tb\t"),
        file(":NOTE: "),
    ]);
}

/// A crafted keyword-line carrier that is no keyword line is not written.
#[test]
fn a_keyword_line_carrier_that_holds_no_keyword_line_is_not_written() {
    let mut b = kid("Shopping\nmilk");
    b.set_keyword_lines(vec![holon_org_format::models::KeywordLine {
        before_line: 1,
        raw: "* Injected\n:PROPERTIES:\n:ID: stolen\n:END:\n".to_string(),
        after_source: false,
    }]);
    let (text, losses, back) = round_trip(PAGE, b);
    assert!(
        !text.contains("Injected") && !losses.is_empty() && text_ids(&back) == ["topic", "kid"],
        "losses {losses:?}\n{text}"
    );
}

/// A crafted authored drawer value holding a line break is not written raw.
#[test]
fn a_drawer_raw_value_with_a_line_break_is_not_written_raw() {
    let mut b = kid("Shopping");
    b.set_property("NOTE", holon_api::Value::String("x".to_string()));
    b.set_property(
        "_drawer_raw",
        holon_api::Value::String(r#"{"NOTE":" x\n* Injected"}"#.to_string()),
    );
    let (text, _, back) = round_trip(PAGE, b);
    assert!(
        !text.contains("Injected") && text_ids(&back) == ["topic", "kid"],
        "{text}"
    );
}

fn states(document: &Block) -> Vec<(String, bool)> {
    use holon_org_format::OrgDocumentExt;
    document
        .todo_keywords()
        .unwrap_or_default()
        .into_iter()
        .map(|s| (s.keyword.clone(), s.is_done()))
        .collect()
}

/// `#+TITLE:` and the task-keyword lines are document keywords wherever they
/// stand outside a block, as org reads them (org-collect-keywords): several
/// TITLE lines join with one space; every TYP_TODO, TODO and SEQ_TODO line adds
/// to the ring, in that order.
#[test]
fn document_keywords_accumulate_wherever_they_stand() {
    use holon_org_format::OrgDocumentExt;
    let file = "#+TITLE: first\n#+TODO: A | B\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n#+title: second\n#+SEQ_TODO: C D\n#+TYP_TODO: X | Y\n** C task\n:PROPERTIES:\n:ID: kid\n:END:\n";
    let (document, blocks) = parse(file);
    assert_eq!(document.file_title().as_deref(), Some("first second"));
    assert_eq!(
        states(&document),
        [
            ("X".to_string(), false),
            ("Y".to_string(), true),
            ("A".to_string(), false),
            ("B".to_string(), true),
            ("C".to_string(), false),
            ("D".to_string(), true),
        ]
    );
    let kid = blocks.iter().find(|b| b.id.id() == "kid").expect("kid");
    assert_eq!(kid.task_state().map(|s| s.keyword), Some("C".to_string()));
    assert_unchanged(&[file.to_string()]);
}

/// A title Holon changes goes into the first TITLE line; the other TITLE lines
/// are removed, and the render names each of them.
#[test]
fn a_changed_title_goes_into_its_first_line() {
    use holon_org_format::OrgDocumentExt;
    let file = "#+title: first\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n#+TITLE: second\n";
    let (mut document, blocks) = parse(file);
    document.set_file_title(Some("renamed".to_string()));
    let (text, losses) = render(&document, &blocks);
    assert_eq!(
        text,
        "#+title: renamed\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n"
    );
    assert!(
        losses.len() == 1 && losses[0].contains("#+TITLE: second"),
        "{losses:?}"
    );
}

/// A ring Holon changes rewrites only the lines whose own keywords changed,
/// each in place; a line left with none is removed and named; an added keyword
/// goes into the first TODO or SEQ_TODO line.
#[test]
fn a_changed_ring_rewrites_only_its_own_lines() {
    use holon_api::TaskState;
    use holon_org_format::OrgDocumentExt;
    let file = "#+TYP_TODO: X | Y\n#+seq_todo: A | B\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n#+TODO: C | D\n";
    let (mut document, blocks) = parse(file);
    document.set_todo_keywords(Some(vec![
        TaskState::active("X"),
        TaskState::done("Y"),
        TaskState::active("A"),
        TaskState::active("N"),
        TaskState::done("B"),
    ]));
    let (text, losses) = render(&document, &blocks);
    assert_eq!(
        text,
        "#+TYP_TODO: X | Y\n#+seq_todo: A N | B\n* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n"
    );
    assert!(
        losses.len() == 1 && losses[0].contains("#+TODO: C | D"),
        "{losses:?}"
    );
}

/// A drawer key with no value is valid org; the drawer, its `:ID:` and the key
/// are kept.
#[test]
fn a_drawer_key_with_no_value_keeps_the_drawer_and_its_id() {
    let file = format!("{PAGE}** K1\n:PROPERTIES:\n:ID: k1\n:NOTE:\n:END:\nbody\n");
    let (_, blocks) = parse(&file);
    let k1 = blocks
        .iter()
        .find(|b| b.id.id() == "k1")
        .expect("k1 keeps its id");
    assert_eq!(k1.content, "K1\nbody");
    assert_eq!(
        k1.get_property("NOTE"),
        Some(holon_api::Value::String(String::new()))
    );
    assert_unchanged(&[file]);
}

/// A `#+` line the author wrote where org reads it literally keeps its bytes
/// while the text is unchanged: inside an example or source block, and inside
/// a drawer, where org does not remove a comma.
#[test]
fn an_authored_literal_keyword_line_keeps_its_bytes() {
    let rest = "* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n";
    assert_unchanged(&[
        format!("#+ID: p\n#+BEGIN_SRC org :id p::src::0\n#+TITLE: inside\n#+END_SRC\n{rest}"),
        format!("#+ID: p\n#+begin_example\n#+TODO: X | Y\n#+end_example\n{rest}"),
        format!("#+ID: p\n{rest}#+begin_example\n#+TITLE: deep\n#+end_example\n"),
        format!("#+ID: p\n{rest}text\n:LOGBOOK:\n- #+a line in a drawer\n#+NOTTITLE x\n:END:\n"),
    ]);
}

/// Org removes one comma from a source line: `,,*` reads as `,*`. Text Holon
/// changes is written escaped, never in its old authored form.
#[test]
fn a_source_line_loses_one_comma_and_changed_text_is_escaped() {
    let rest = "* Topic\n:PROPERTIES:\n:ID: topic\n:END:\n";
    let file =
        format!("#+ID: p\n#+BEGIN_SRC org :id p::src::0\n,,* deep\n#+TITLE: x\n#+END_SRC\n{rest}");
    let (document, mut blocks) = parse(&file);
    let source = blocks
        .iter_mut()
        .find(|b| b.id.id() == "p::src::0")
        .expect("the source block");
    assert_eq!(source.content, ",* deep\n#+TITLE: x");
    assert_unchanged(std::slice::from_ref(&file));
    source.content = ",* deep\n#+TITLE: y".to_string();
    let (text, losses) = render(&document, &blocks);
    assert!(
        text.contains(",,* deep\n,#+TITLE: y\n") && losses.is_empty(),
        "{losses:?}\n{text}"
    );
}

/// Text typed into a block inside a drawer is not comma-escaped: org would not
/// remove the comma there.
#[test]
fn typed_text_inside_a_drawer_is_not_comma_escaped() {
    let content = "Shop\n:LOGBOOK:\n#+not a keyword\n:END:";
    let (text, losses, back) = round_trip(PAGE, kid(content));
    let read = back
        .iter()
        .find(|b| b.id.id() == "kid")
        .map(|b| b.content.clone());
    assert!(
        !text.contains(",#+") && read.as_deref() == Some(content) && losses.is_empty(),
        "{losses:?}\n{text}"
    );
}

fn carrier_json() -> proptest::strategy::BoxedStrategy<String> {
    use proptest::prelude::*;
    prop_oneof![
        any::<String>(),
        Just("not json".to_string()),
        Just("{}".to_string()),
        Just("[]".to_string()),
        Just("[1,2]".to_string()),
        Just(r#"{"NOTE": 5}"#.to_string()),
        Just(r#"[{"before_line": "x"}]"#.to_string()),
        Just(r#"{"before_body": 3, "after": null}"#.to_string()),
        Just("\"Crlf\"".to_string()),
        Just("null".to_string()),
    ]
    .boxed()
}

proptest::proptest! {
    /// A stored carrier holding anything at all never panics the renderer; one
    /// it cannot read is a loss.
    #[test]
    fn a_malformed_carrier_is_a_loss_not_a_panic(
        json in carrier_json(),
        carrier in proptest::sample::select(vec![
            "_drawer_raw", "_keyword_lines", "_blank_lines", "_header_lines",
            "_header_places", "_line_breaks",
        ]),
    ) {
        let (mut document, mut blocks) = parse(&format!(
            "{PAGE}** K1\n:PROPERTIES:\n:ID: k1\n:NOTE: v\n:END:\nbody\n"
        ));
        let target: &mut Block = if carrier.starts_with("_header") || carrier == "_line_breaks" {
            &mut document
        } else {
            blocks.iter_mut().find(|b| b.id.id() == "k1").expect("k1")
        };
        target.set_property(carrier, holon_api::Value::String(json.clone()));
        let rendered = OrgRenderer::render_document(&document, &blocks, Path::new(FILE), &document.id);
        if let Ok(r) = rendered {
            let readable = match carrier {
                "_drawer_raw" => serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&json)
                    .is_ok_and(|m| m.values().all(serde_json::Value::is_string)),
                _ => true,
            };
            proptest::prop_assert!(readable || !r.losses.is_empty(), "{json:?} in {carrier}: no loss");
        }
    }
}
