//! Holon reads an org file as Emacs org reads it. Each expected reading here
//! was measured with `emacs -Q --batch` (org-element-parse-buffer,
//! org-entry-get, org-get-title, org-todo-keywords-1); the measurements are in
//! `lane-logs/B10-emacs-*.log`.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgDocumentExt;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const FILE: &str = "/vault/p.org";

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

fn block<'a>(blocks: &'a [Block], id: &str) -> &'a Block {
    blocks
        .iter()
        .find(|b| b.id.id() == id)
        .unwrap_or_else(|| panic!("no block {id} in {:?}", ids(blocks)))
}

fn ids(blocks: &[Block]) -> Vec<String> {
    blocks.iter().map(|b| b.id.id().to_string()).collect()
}

fn page(body: &str) -> String {
    format!("#+ID: p\n* H\n:PROPERTIES:\n:ID: h\n:END:\n{body}")
}

/// A leading comma org keeps (it removes one only in example, export and src
/// blocks) is text: the block reads with it, and the file is written back as
/// it was. Before a headline Holon removes one comma there, as in paragraph
/// text (D230.a): a headline ends the block, so that comma is Holon's escape.
#[test]
fn a_comma_org_keeps_is_text() {
    let cases = [
        ("#+begin_quote\n", "#+end_quote\n"),
        ("#+begin_center\n", "#+end_center\n"),
        ("#+begin_verse\n", "#+end_verse\n"),
        ("#+begin_comment\n", "#+end_comment\n"),
        ("#+begin_foo\n", "#+end_foo\n"),
        ("#+BEGIN: clocktable\n", "#+END:\n"),
        ("text\n:LOGBOOK:\n", ":END:\n"),
    ];
    let mut wrong = Vec::new();
    for (open, close) in cases {
        let file = page(&format!("{open},* x\n,#+FOO: bar\n,,* xx\n,x\n{close}"));
        let (document, blocks) = parse(&file);
        let content = &block(&blocks, "h").content;
        let (text, losses) = render(&document, &blocks);
        let holon_reads = format!("{open}* x\n,#+FOO: bar\n,* xx\n,x\n{close}");
        if !format!("{content}\n").ends_with(&holon_reads) || text != file || !losses.is_empty() {
            wrong.push(format!(
                "{open:?}: read {content:?}, wrote {text:?}, losses {losses:?}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Inside example and export blocks, and in a source block's lines at any
/// indentation, org removes one comma before `*` or `#+`.
#[test]
fn a_comma_org_removes_is_removed() {
    let file = page("#+begin_example\n,* x\n  ,#+y\n,,* xx\n#+end_example\n");
    let (document, blocks) = parse(&file);
    assert_eq!(
        block(&blocks, "h").content,
        "H\n#+begin_example\n* x\n  #+y\n,* xx\n#+end_example"
    );
    assert_eq!(render(&document, &blocks), (file, vec![]));

    let file = page("#+BEGIN_SRC org :id h::src::0\n  ,#+y\n  ,* z\n#+END_SRC\n");
    let (document, blocks) = parse(&file);
    assert_eq!(block(&blocks, "h::src::0").content, "  #+y\n  * z");
    assert_eq!(render(&document, &blocks), (file, vec![]));
}

/// Holon writes no comma org keeps: a keyword line inside a quote block is
/// written as it is, and the render names the block. A headline line there is
/// written with the comma Holon removes, and reads back with no loss.
#[test]
fn a_line_org_would_read_otherwise_is_a_loss_not_a_comma() {
    for (text, written_line) in [
        ("H\n#+begin_quote\n#+FOO: bar\n#+end_quote", None),
        ("H\n#+begin_quote\n* x\n#+end_quote", Some(",* x\n")),
        ("H\ntext\n:LOGBOOK:\n* x\n:END:", Some(",* x\n")),
    ] {
        let (document, mut blocks) = parse(&page(""));
        blocks
            .iter_mut()
            .find(|b| b.id.id() == "h")
            .unwrap()
            .content = text.to_string();
        let (written, losses) = render(&document, &blocks);
        match written_line {
            None => {
                assert!(
                    !written.contains(",#+FOO") && losses.iter().any(|l| l.contains("block:h")),
                    "{text:?}: wrote {written:?}, losses {losses:?}"
                );
            }
            Some(line) => {
                let (_, back) = parse(&written);
                assert!(
                    written.contains(line)
                        && losses.is_empty()
                        && block(&back, "h").content == text,
                    "{text:?}: wrote {written:?}, losses {losses:?}"
                );
            }
        }
    }
}

/// Text Holon writes into an example block gets the comma org removes there.
#[test]
fn holon_text_in_an_example_block_is_escaped_as_org_escapes_it() {
    let (document, mut blocks) = parse(&page(""));
    blocks
        .iter_mut()
        .find(|b| b.id.id() == "h")
        .unwrap()
        .content = "H\n#+begin_example\n* x\n  #+y\n#+end_example".to_string();
    let (written, losses) = render(&document, &blocks);
    assert!(
        written.contains("#+begin_example\n,* x\n  ,#+y\n#+end_example") && losses.is_empty(),
        "{written:?} {losses:?}"
    );
}

fn drawer_file(drawer: &str) -> String {
    format!("#+ID: p\n* H\n{drawer}body\n")
}

/// A drawer org reads as the headline's property drawer keeps its authored
/// bytes while Holon does not change the block: spacing, key case,
/// indentation, `:END:` spacing, a lowercase `:properties:`, a key org
/// reads with a colon in it or an append key.
#[test]
fn an_unedited_property_drawer_keeps_its_bytes() {
    let mut wrong = Vec::new();
    for drawer in [
        ":PROPERTIES:\n:ID:   aaa  \n:END:\n",
        ":PROPERTIES:\n:id: aaa\n:END:\n",
        "  :PROPERTIES:\n  :ID: aaa\n  :END:\n",
        ":PROPERTIES:\n:ID:\taaa\n:END:\n",
        ":PROPERTIES:\n:ID: aaa\n:END:  \n",
        ":properties:\n:ID: aaa\n:end:\n",
        ":PROPERTIES:\n:ID: aaa\n:a:b: v\n:END:\n",
        ":PROPERTIES:\n:ID: aaa\n:NOTE+: v\n:END:\n",
        ":PROPERTIES:\n:ID: aaa\n:NOTE:\n:END:\n",
    ] {
        let file = drawer_file(drawer);
        let (document, blocks) = parse(&file);
        let (text, losses) = render(&document, &blocks);
        if ids(&blocks) != ["aaa"] || text != file || !losses.is_empty() {
            wrong.push(format!(
                "{drawer:?}: ids {:?}, wrote {text:?}, losses {losses:?}",
                ids(&blocks)
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Org reads `:a:b: v` as the key `a:b`, as it reads every property line.
#[test]
fn a_property_key_is_read_as_org_reads_it() {
    let (_, blocks) = parse(&drawer_file(":PROPERTIES:\n:ID: aaa\n:a:b: v\n:END:\n"));
    assert_eq!(
        block(&blocks, "aaa").get_property("a:b"),
        Some(Value::String("v".into()))
    );
    assert_eq!(block(&blocks, "aaa").get_property("a"), None);
}

/// A PROPERTIES drawer org does not read as one (a key with a space, a blank
/// line or a text line in it, `:ID:` with no space before the value) holds no
/// property for org. Holon keeps the id it names so the block keeps its
/// identity, reads no other property from it, writes it back unchanged, and
/// the render says that org sees no id there.
#[test]
fn a_property_drawer_org_does_not_read_keeps_its_bytes_and_is_disclosed() {
    let mut wrong = Vec::new();
    for drawer in [
        ":PROPERTIES:\n:ID: aaa\n:MY KEY: v\n:END:\n",
        ":PROPERTIES:\n:ID: aaa\n\n:NOTE: v\n:END:\n",
        ":PROPERTIES:\n:ID: aaa\nnot a prop\n:END:\n",
        ":PROPERTIES:\n:ID:aaa\n:END:\n",
    ] {
        let file = drawer_file(drawer);
        let (document, blocks) = parse(&file);
        let (text, losses) = render(&document, &blocks);
        let b = block(&blocks, "aaa");
        let disclosed = losses.len() == 1 && losses[0].contains("block:aaa");
        if b.content != "H\nbody" || b.get_property("NOTE").is_some() || text != file || !disclosed
        {
            wrong.push(format!(
                "{drawer:?}: content {:?}, NOTE {:?}, wrote {text:?}, losses {losses:?}",
                b.content,
                b.get_property("NOTE")
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Org reads a drawer with two `:ID:` lines, and org-entry-get (which org-id
/// links resolve through) takes the last. Holon keeps the last too, writes the
/// drawer back unchanged and names both ids.
#[test]
fn a_drawer_with_two_ids_keeps_its_bytes_and_is_disclosed() {
    let file = drawer_file(":PROPERTIES:\n:ID: aaa\n:ID: bbb\n:END:\n");
    let (document, blocks) = parse(&file);
    assert_eq!(ids(&blocks), ["bbb"]);
    let (text, losses) = render(&document, &blocks);
    assert_eq!(text, file);
    assert!(
        losses.len() == 1
            && losses[0].contains("block:bbb")
            && losses[0].contains(":ID: aaa")
            && losses[0].contains(":ID: bbb"),
        "{losses:?}"
    );
}

/// When Holon changes a block whose drawer org does not read, the new
/// property drawer is written and the authored drawer is kept below it as
/// text, with a loss; no authored line is lost.
#[test]
fn an_edited_block_keeps_the_drawer_org_does_not_read_as_text() {
    let drawer = ":PROPERTIES:\n:ID: aaa\n:MY KEY: v\n:END:\n";
    let (document, mut blocks) = parse(&drawer_file(drawer));
    blocks[0].set_property("NOTE", Value::String("new".into()));
    let (text, losses) = render(&document, &blocks);
    assert!(
        text.contains(":NOTE: new") && text.contains(drawer) && !losses.is_empty(),
        "{text:?} {losses:?}"
    );
}

/// When Holon changes a block whose drawer has a line the renderer cannot
/// keep (a second `:ID:`), the render names that line.
#[test]
fn an_edited_drawer_names_each_line_it_drops() {
    let (document, mut blocks) = parse(&drawer_file(":PROPERTIES:\n:ID: aaa\n:ID: bbb\n:END:\n"));
    blocks[0].set_property("NOTE", Value::String("new".into()));
    let (text, losses) = render(&document, &blocks);
    assert!(
        text.contains(":NOTE: new")
            && !text.contains(":ID: aaa")
            && losses.iter().any(|l| l.contains(":ID: aaa")),
        "{text:?} {losses:?}"
    );
}

/// Org reads the title from every `#+TITLE:` line of the file, inside any
/// block too.
#[test]
fn the_title_is_read_wherever_org_reads_it() {
    let mut wrong = Vec::new();
    for body in [
        "#+BEGIN_SRC org :id h::src::0\n#+TITLE: t\n#+END_SRC\n",
        "#+begin_quote\n#+TITLE: t\n#+end_quote\n",
        "#+begin_example\n#+TITLE: t\n#+end_example\n",
        "#+begin_comment\n#+TITLE: t\n#+end_comment\n",
        "#+begin_verse\n#+TITLE: t\n#+end_verse\n",
    ] {
        let file = page(body);
        let (document, blocks) = parse(&file);
        let (text, losses) = render(&document, &blocks);
        if document.file_title().as_deref() != Some("t") || text != file || !losses.is_empty() {
            wrong.push(format!(
                "{body:?}: title {:?}, wrote {text:?}, losses {losses:?}",
                document.file_title()
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

fn ring(document: &Block) -> Vec<String> {
    document
        .todo_keywords()
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.keyword)
        .collect()
}

/// Org collects `#+TODO:` lines inside quote, center and special blocks and
/// in list items, but not inside verse, example, src, export or comment
/// blocks.
#[test]
fn task_keywords_are_read_where_org_reads_them() {
    let mut wrong = Vec::new();
    for (open, close, read) in [
        ("#+begin_quote\n", "#+end_quote\n", true),
        ("#+begin_center\n", "#+end_center\n", true),
        ("#+begin_foo\n", "#+end_foo\n", true),
        ("- item\n  ", "", true),
        ("#+begin_verse\n", "#+end_verse\n", false),
        ("#+begin_example\n", "#+end_example\n", false),
        ("#+begin_export org\n", "#+end_export\n", false),
        ("#+begin_comment\n", "#+end_comment\n", false),
    ] {
        let file = page(&format!("{open}#+TODO: XA | YD\n{close}"));
        let (document, blocks) = parse(&file);
        let (text, losses) = render(&document, &blocks);
        let expected: Vec<String> = if read {
            vec!["XA".into(), "YD".into()]
        } else {
            vec![]
        };
        if ring(&document) != expected || text != file || !losses.is_empty() {
            wrong.push(format!(
                "{open:?}: ring {:?}, wrote {text:?}, losses {losses:?}",
                ring(&document)
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A fast-access key in parentheses is not part of the keyword
/// (org-remove-keyword-keys).
#[test]
fn fast_access_keys_are_not_part_of_a_task_keyword() {
    let file = "#+TODO: NEXT(n) WAIT(w@/!) | DONE(d)\n* NEXT H\n:PROPERTIES:\n:ID: h\n:END:\n";
    let (document, blocks) = parse(file);
    assert_eq!(ring(&document), ["NEXT", "WAIT", "DONE"]);
    assert_eq!(
        block(&blocks, "h").task_state().map(|s| s.keyword),
        Some("NEXT".to_string())
    );
    assert_eq!(render(&document, &blocks), (file.to_string(), vec![]));
}

/// A title line inside a block cannot be changed with the page title: the
/// page's own title line gets the new value and the render names the page.
#[test]
fn a_title_line_inside_a_block_is_not_rewritten_and_is_a_loss() {
    let file = "#+ID: p\n#+TITLE: page\n* H\n:PROPERTIES:\n:ID: h\n:END:\n#+begin_example\n#+TITLE: inner\n#+end_example\n";
    let (mut document, blocks) = parse(file);
    assert_eq!(document.file_title().as_deref(), Some("page inner"));
    document.set_file_title(Some("new".to_string()));
    let (text, losses) = render(&document, &blocks);
    assert!(
        text.contains("#+TITLE: new\n") && text.contains("#+TITLE: inner") && !losses.is_empty(),
        "{text:?} {losses:?}"
    );
}

/// An empty quote or center block is valid org and reads back as written.
#[test]
fn an_empty_greater_block_reads_back() {
    for body in [
        "#+begin_quote\n#+end_quote\n",
        "#+begin_center\n#+end_center\n",
    ] {
        let file = page(body);
        let (document, blocks) = parse(&file);
        assert_eq!(
            render(&document, &blocks),
            (file.clone(), vec![]),
            "{body:?}"
        );
    }
}

/// A PROPERTIES drawer after a blank line is no property drawer for org. It
/// is kept as authored, its `:ID:` still names the block, and every render
/// says that org finds no id there.
#[test]
fn a_drawer_after_a_blank_line_keeps_its_bytes_and_is_disclosed() {
    for drawer in [
        "\n:PROPERTIES:\n:ID: aaa\n:END:\n",
        "\n\n:PROPERTIES:\n:ID: aaa\n:NOTE: v\n:END:\n\n",
    ] {
        let file = drawer_file(drawer);
        let (document, blocks) = parse(&file);
        assert_eq!(ids(&blocks), ["aaa"], "{drawer:?}");
        assert_eq!(block(&blocks, "aaa").content, "H\nbody", "{drawer:?}");
        assert_eq!(
            block(&blocks, "aaa").get_property("NOTE"),
            None,
            "{drawer:?}"
        );
        let (text, losses) = render(&document, &blocks);
        assert_eq!(text, file, "{drawer:?}");
        assert!(
            losses.len() == 1 && losses[0].contains("blank lines"),
            "{losses:?}"
        );
    }
}

/// A drawer key that starts with `_` is a property to org like any other
/// (`ENTRY-PROPERTIES … "_NOTE=keep me"`, `lane-logs/B11-emacs-drawers.log`),
/// whatever its name and value. Holon reads it as that property, keeps its
/// line, and keeps it when the block is edited.
#[test]
fn an_underscore_drawer_key_is_a_property() {
    let keys = [
        "_note",
        "_drawer_raw",
        "_drawer_text",
        "_drawer_order",
        "_blank_lines",
        "_keyword_lines",
        "_header_lines",
        "_header_places",
        "_line_breaks",
        "_priority_drawer_only",
        "_authored_text",
        "_text_after_source",
        "_provenance",
        "_source_lines",
        "_stars",
        "_file_properties",
        "_file_id_keyword",
        "\\_note",
    ];
    let values = ["keep me", "nonsense", "5", "[1]", "{\"a\":1}", "\"{}\""];
    let mut wrong = Vec::new();
    for key in keys {
        for value in values {
            let file = format!("* H\n:PROPERTIES:\n:ID: h\n:{key}: {value}\n:END:\nbody\n");
            let outcome = std::panic::catch_unwind(|| {
                let (document, mut blocks) = parse(&file);
                let read = block(&blocks, "h").drawer_properties().get(key).cloned();
                let unedited = render(&document, &blocks);
                blocks[0].content = "H\nedited".to_string();
                let (edited, _) = render(&document, &blocks);
                (read, unedited, edited)
            });
            let line = format!(":{key}: {value}\n");
            match outcome {
                Err(_) => wrong.push(format!("{file:?}: panicked")),
                Ok((read, unedited, edited)) => {
                    if read.as_deref() != Some(value)
                        || unedited != (file.clone(), vec![])
                        || !edited.contains(&line)
                    {
                        wrong.push(format!(
                            "{file:?}: read {read:?}, wrote {unedited:?}, edited {edited:?}"
                        ));
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A PROPERTIES drawer after text is text to org (`ENTRY-GET-ID nil`,
/// `lane-logs/B11-emacs-drawers.log`). The headline keeps the id its `:ID:`
/// line names, the drawer stays in the body as written, and every render says
/// that org finds no id there. A block edited to hold another drawer value
/// gets a property drawer above its text, with the same id.
#[test]
fn a_drawer_after_text_keeps_the_headline_id_and_is_disclosed() {
    let file = "* H\ntext\n:PROPERTIES:\n:ID: h\n:END:\nbody\n";
    let (document, mut blocks) = parse(file);
    assert_eq!(ids(&blocks), ["h"]);
    assert_eq!(
        block(&blocks, "h").content,
        "H\ntext\n:PROPERTIES:\n:ID: h\n:END:\nbody"
    );
    let (text, losses) = render(&document, &blocks);
    assert_eq!(text, file);
    assert!(
        losses.len() == 1 && losses[0].contains("org finds none"),
        "{losses:?}"
    );

    blocks[0].set_property("NOTE", Value::String("v".to_string()));
    let (text, losses) = render(&document, &blocks);
    assert_eq!(
        text,
        "* H\n:PROPERTIES:\n:ID: h\n:NOTE: v\n:END:\ntext\n:PROPERTIES:\n:ID: h\n:END:\nbody\n"
    );
    assert!(!losses.is_empty());
    assert_eq!(ids(&parse(&text).1), ["h"]);
}

/// Org reads `:NOTE: ""` as the two characters `""`, and a value-less
/// `:NOTE:` as the empty value
/// (`ENTRY-GET-NOTE "\"\""` and `""`, `lane-logs/B12-emacs.log`). A value
/// Holon sets to empty reads back empty.
#[test]
fn a_quoted_empty_drawer_value_is_read_as_org_reads_it() {
    let file = "* H\n:PROPERTIES:\n:ID: h\n:NOTE: \"\"\n:END:\n";
    let (document, blocks) = parse(file);
    assert_eq!(
        block(&blocks, "h")
            .drawer_properties()
            .get("NOTE")
            .map(String::as_str),
        Some("\"\"")
    );
    assert_eq!(render(&document, &blocks), (file.to_string(), vec![]));

    let (document, mut blocks) = parse("* H\n:PROPERTIES:\n:ID: h\n:END:\n");
    blocks[0].set_property("NOTE", Value::String(String::new()));
    let (text, losses) = render(&document, &blocks);
    assert!(losses.is_empty(), "{losses:?}");
    assert_eq!(
        block(&parse(&text).1, "h")
            .drawer_properties()
            .get("NOTE")
            .map(String::as_str),
        Some(""),
        "{text:?}"
    );
}
