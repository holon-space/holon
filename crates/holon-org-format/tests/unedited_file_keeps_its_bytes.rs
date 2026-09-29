//! An org file Holon has not edited is written back with its bytes, or the
//! render names what it changes.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::block::Block;
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

fn write_back(source: &str) -> (String, Vec<String>) {
    let (document, blocks) = parse(source);
    let r = OrgRenderer::render_document(&document, &blocks, Path::new(FILE), &document.id)
        .expect("org render");
    (
        r.text,
        r.losses
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
    )
}

fn changed(files: &[&str]) -> Vec<String> {
    files
        .iter()
        .filter_map(|file| {
            let written = write_back(file);
            (written != (file.to_string(), vec![])).then(|| format!("{file:?} -> {written:?}"))
        })
        .collect()
}

const HEAD: &str = "* H\n:PROPERTIES:\n:ID: h\n:END:\n";

#[test]
fn blank_lines_before_a_source_block_are_kept() {
    let src = "#+BEGIN_SRC sh :id s1\nx\n#+END_SRC\n";
    let files = [
        format!("{HEAD}text\n\n{src}"),
        format!(
            "{HEAD}text\n\n\n{src}\n#+BEGIN_SRC sh :id s2\ny\n#+END_SRC\n\n* J\n:PROPERTIES:\n:ID: j\n:END:\n"
        ),
        format!("{HEAD}\n{src}"),
        format!("{HEAD}- a\n- b\n\n{src}"),
        format!("{HEAD}#+CAPTION: c\n\n{src}"),
        format!("text\n\n{src}\n{HEAD}"),
        format!("#+TITLE: t\n\n{src}{HEAD}"),
        format!("{HEAD}text\n\n{src}").replace('\n', "\r\n"),
    ];
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    let changed = changed(&files);
    assert!(changed.is_empty(), "{}", changed.join("\n"));
}

#[test]
fn blank_lines_and_text_at_the_start_of_a_file_are_kept() {
    let files = [
        format!("hello\n{HEAD}"),
        "hello\n".to_string(),
        format!(":PROPERTIES:\n:ID: p\n:END:\nhello\n{HEAD}"),
        format!("\n\nhello\n\n{HEAD}"),
        format!("\n\n{HEAD}"),
        format!("\n#+TITLE: t\nhello\n{HEAD}"),
        "\n".to_string(),
        format!("\n\n#+BEGIN_SRC sh :id s1\nx\n#+END_SRC\n{HEAD}"),
    ];
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    let changed = changed(&files);
    assert!(changed.is_empty(), "{}", changed.join("\n"));
}

#[test]
fn a_boolean_drawer_value_keeps_its_spelling() {
    let changed = changed(&[
        "* H\n:PROPERTIES:\n:ID: h\n:COLLAPSED: true\n:END:\n",
        "* H\n:PROPERTIES:\n:ID: h\n:collapsed: true\n:END:\n",
        "* H\n:PROPERTIES:\n:ID: h\n:COLLAPSED: nil\n:END:\n",
        "* H\n:PROPERTIES:\n:ID: h\n:WIDGET_ONLY: TRUE\n:END:\n",
    ]);
    assert!(changed.is_empty(), "{}", changed.join("\n"));
}

#[test]
fn text_after_a_source_block_is_a_loss() {
    let file = format!("{HEAD}text1\n\n#+BEGIN_SRC sh :id s1\nx\n#+END_SRC\ntext2\n");
    let (text, losses) = write_back(&file);
    assert_ne!(text, file);
    assert!(
        losses.len() == 1 && losses[0].contains("after a source block"),
        "{losses:?}"
    );
}

/// A source block before the first headline of a file with no `#+ID:` is a
/// child of the page, which keeps its path id.
#[test]
fn a_preamble_source_block_in_a_file_with_no_id_is_a_child_of_the_page() {
    let (document, blocks) = parse(&format!("#+begin_src org\nx\n#+end_src\n{HEAD}"));
    assert_eq!(document.id.as_str(), "file:p.org");
    assert_eq!(blocks[0].parent_id, document.id);
    let r = OrgRenderer::render_document(&document, &blocks, Path::new(FILE), &document.id)
        .expect("org render");
    let (_, reread) = parse(&r.text);
    assert_eq!(
        reread.iter().map(|b| &b.id).collect::<Vec<_>>(),
        blocks.iter().map(|b| &b.id).collect::<Vec<_>>()
    );
}

fn headline(stars: usize, id: &str) -> String {
    format!(
        "{} {id}\n:PROPERTIES:\n:ID: {id}\n:END:\n",
        "*".repeat(stars)
    )
}

#[test]
fn source_block_lines_keep_their_spelling() {
    let files = [
        format!("{HEAD}#+begin_src sh :id s1\nx\n#+end_src\n"),
        format!("{HEAD}#+Begin_Src sh   :results output :id s1\nx\n  #+end_Src\n"),
        format!("{HEAD}#+name: n1\n#+begin_src sh :id s1\nx\n#+end_src\n"),
        format!("#+begin_src sh :id s1\nx\n#+end_src\n{HEAD}"),
    ];
    for file in &files {
        let (_, blocks) = parse(file);
        assert!(
            blocks
                .iter()
                .any(|b| b.id.id() == "s1" && b.source_name.as_deref() != Some("")),
            "{file:?} reads no source block s1"
        );
    }
    let (_, blocks) = parse(&files[2]);
    assert_eq!(blocks[1].source_name.as_deref(), Some("n1"));
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    let changed = changed(&files);
    assert!(changed.is_empty(), "{}", changed.join("\n"));
}

/// Org reads the blank lines at the start and end of a source block as its
/// text (`src-block :value="\nx\n"`, `lane-logs/B12-emacs.log`).
#[test]
fn blank_lines_inside_a_source_block_are_its_text() {
    for (file, text) in [
        (
            format!("{HEAD}#+BEGIN_SRC sh :id s1\n\nx\n#+END_SRC\n"),
            "\nx",
        ),
        (
            format!("{HEAD}#+BEGIN_SRC sh :id s1\nx\n\n#+END_SRC\n"),
            "x\n",
        ),
    ] {
        let (_, blocks) = parse(&file);
        assert_eq!(blocks[1].content, text);
        assert_eq!(write_back(&file), (file, vec![]));
    }
}

#[test]
fn a_source_block_with_no_id_keeps_its_bytes() {
    let changed = changed(&[
        &format!("{HEAD}#+begin_src sh\nx\n#+end_src\n"),
        &format!("{HEAD}#+BEGIN_SRC sh :id s0\nx\n#+END_SRC\n#+BEGIN_SRC sh\ny\n#+END_SRC\n"),
        "#+begin_src sh\nx\n#+end_src\n",
    ]);
    assert!(changed.is_empty(), "{}", changed.join("\n"));
}

/// A source block with no `:id` whose place Holon changes gets its id
/// written, and the render names it.
#[test]
fn a_moved_source_block_with_no_id_has_its_id_written() {
    let file = format!("{HEAD}#+begin_src sh\nx\n#+end_src\n{}", headline(1, "j"));
    let (document, mut blocks) = parse(&file);
    let src = blocks
        .iter()
        .position(|b| b.id.id() == "h::src::0")
        .expect("src block");
    blocks[src].parent_id = EntityUri::block("j");
    let moved = blocks.remove(src);
    blocks.push(moved);
    let r = OrgRenderer::render_document(&document, &blocks, Path::new(FILE), &document.id)
        .expect("org render");
    let (_, reread) = parse(&r.text);
    let src = reread.iter().find(|b| b.id.id() == "h::src::0");
    assert_eq!(
        src.map(|b| b.parent_id.id()),
        Some("j"),
        "the source block must keep its id under j: {:?}",
        r.text
    );
    assert!(
        r.losses.iter().any(|l| l.block.id() == "h::src::0"),
        "{:?}",
        r.losses
    );
}

/// Org reads a headline's level from its stars (`**** deep` is level 4,
/// `lane-logs/B12-emacs.log`), and the tree from the levels.
#[test]
fn a_headline_keeps_its_star_count() {
    let files = [
        format!("{HEAD}{}{}", headline(4, "d"), headline(2, "m")),
        format!("{}{}", headline(2, "a"), headline(1, "b")),
        format!(
            "{HEAD}{}{}{}",
            headline(3, "c"),
            headline(5, "g"),
            headline(4, "c2")
        ),
    ];
    let files: Vec<&str> = files.iter().map(String::as_str).collect();
    let changed = changed(&files);
    assert!(changed.is_empty(), "{}", changed.join("\n"));
}

/// When Holon changes the tree, each headline is written at a level org reads
/// as its place in the tree.
#[test]
fn a_moved_headline_is_written_where_org_reads_its_place() {
    let file = format!(
        "{HEAD}{}{}{}",
        headline(3, "first"),
        headline(2, "second"),
        headline(1, "k")
    );
    let (document, blocks) = parse(&file);
    let place = |blocks: &[Block]| -> Vec<(String, String)> {
        let mut v: Vec<_> = blocks
            .iter()
            .map(|b| (b.id.id().to_string(), b.parent_id.id().to_string()))
            .collect();
        v.sort();
        v
    };
    let reorder = |order: &[&str], reparent: Option<(&str, &str)>| {
        let mut moved: Vec<Block> = order
            .iter()
            .map(|id| blocks.iter().find(|b| b.id.id() == *id).unwrap().clone())
            .collect();
        if let Some((id, parent)) = reparent {
            let b = moved.iter_mut().find(|b| b.id.id() == id).unwrap();
            b.parent_id = EntityUri::block(parent);
        }
        let r = OrgRenderer::render_document(&document, &moved, Path::new(FILE), &document.id)
            .expect("org render");
        assert_eq!(place(&parse(&r.text).1), place(&moved), "{:?}", r.text);
    };
    reorder(&["h", "second", "first", "k"], None);
    reorder(&["h", "first", "k", "second"], Some(("second", "k")));
    reorder(&["h", "second", "k", "first"], Some(("first", "second")));
}

/// A property a person names like one of the parser's carriers is their data:
/// the page renders, and the file keeps its bytes.
#[test]
fn a_user_property_named_like_a_carrier_is_user_data() {
    let file = format!("#+ID: p\n{HEAD}");
    for key in ["file_properties", "file_id_keyword"] {
        let (mut document, blocks) = parse(&file);
        document.set_property(
            key,
            holon_api::Value::String("{\"ID\":\"evil\"}".to_string()),
        );
        let r = OrgRenderer::render_document(&document, &blocks, Path::new(FILE), &document.id)
            .unwrap_or_else(|e| panic!("{key}: {e:#}"));
        assert_eq!(
            (r.text.as_str(), r.losses.len()),
            (file.as_str(), 0),
            "{key}"
        );
    }
}
