//! Concrete examples + fail-loud error paths (the paths the happy-path PBT
//! generator never reaches).

use holon_toon::ToonError;
use holon_toon::models::BlockId;
use holon_toon::models::BlockNode;
use holon_toon::models::ContentType;
use holon_toon::models::Forest;
use holon_toon::models::TaskState;
use holon_toon::models::ToonBlock;
use holon_toon::parse;
use holon_toon::render;

fn bid(s: &str) -> BlockId {
    BlockId::new(s).unwrap()
}

#[test]
fn golden_small_forest() {
    // A page with two children, one DONE, mirroring the motivating file's shape.
    let mut fix = ToonBlock::text(bid("id-fix"), "Fix bugs");
    fix.state = TaskState::new("DOING");
    let mut done = ToonBlock::text(bid("id-done"), "Make tests deterministic");
    done.state = TaskState::new("DONE");
    let page = ToonBlock::text(bid("id-page"), "Example project page");

    let forest = Forest::new(vec![BlockNode::with_children(
        page,
        vec![BlockNode::with_children(fix, vec![BlockNode::leaf(done)])],
    )]);

    let rendered = render(&forest);
    assert_eq!(
        rendered,
        "blocks[3]{id,depth,state,props,body,title}:\n\
         \x20\x20id-page,0,,,,Example project page\n\
         \x20\x20id-fix,1,DOING,,,Fix bugs\n\
         \x20\x20id-done,2,DONE,,,Make tests deterministic\n"
    );
    assert_eq!(parse(&rendered).unwrap(), forest);
}

#[test]
fn source_block_with_clashing_characters_roundtrips() {
    // SQL body full of the exact characters that clash with TOON: colons,
    // brackets, braces, commas, and newlines.
    let code = "SELECT b.*\nFROM block b\nWHERE json_extract(b.properties, '$.state') = 'TODO'\n  AND EXISTS (SELECT 1 FROM x WHERE y IN [1,2,3]);";
    let mut src = ToonBlock::text(bid("q::src::0"), String::new());
    src.content_type = ContentType::Source;
    src.source_language = Some("holon_sql".into());
    src.body = Some(code.to_string());

    let forest = Forest::new(vec![BlockNode::leaf(src)]);
    let rendered = render(&forest);
    // The whole source collapses to ONE quoted, \n-escaped cell (the legibility
    // cost documented in MAPPING.md).
    assert!(rendered.contains("\\n"));
    assert!(!rendered.contains("\nSELECT")); // no real newline leaked structurally
    assert_eq!(parse(&rendered).unwrap(), forest);
}

#[test]
fn arbitrary_drawer_with_special_values_roundtrips() {
    let mut b = ToonBlock::text(bid("b1"), "claimed block");
    b.collapsed = true;
    b.properties
        .insert("assigned-to".into(), "agent-1234".into());
    b.properties
        .insert("claimed-at".into(), "2026-01-02T03:04:05.678+00:00".into());
    b.properties
        .insert("claimed-from".into(), "/tmp/example/workspace".into());
    let forest = Forest::new(vec![BlockNode::leaf(b)]);
    assert_eq!(parse(&render(&forest)).unwrap(), forest);
}

#[test]
fn tags_and_requires_with_awkward_content_roundtrip() {
    let mut b = ToonBlock::text(bid("b1"), "t");
    b.tags = vec!["needs review".into(), "a,b".into(), "x:y".into()];
    b.requires = vec![bid("dep-1"), bid("dep:2")];
    let forest = Forest::new(vec![BlockNode::leaf(b)]);
    assert_eq!(parse(&render(&forest)).unwrap(), forest);
}

#[test]
fn every_edge_field_roundtrips() {
    let mut b = ToonBlock::text(bid("b1"), "t");
    b.tags = vec!["task".into()];
    b.requires = vec![bid("dep-1")];
    b.advice_suppressed = vec![bid("lesson-1")];
    b.contributes_to = vec![bid("goal-1"), bid("goal:2")];
    let forest = Forest::new(vec![BlockNode::leaf(b)]);
    assert_eq!(parse(&render(&forest)).unwrap(), forest);
}

#[test]
fn empty_document_is_an_error() {
    assert_eq!(parse("   \n\n"), Err(ToonError::EmptyDocument));
}

#[test]
fn bad_header_is_an_error() {
    let err = parse("blocks[2]{id,title}:\n  a,b\n").unwrap_err();
    assert!(matches!(err, ToonError::BadHeader { .. }), "got {err:?}");
}

#[test]
fn row_count_mismatch_is_an_error() {
    let doc = "blocks[3]{id,depth,state,props,body,title}:\n  a,0,,,,t\n";
    assert_eq!(
        parse(doc),
        Err(ToonError::RowCountMismatch {
            declared: 3,
            actual: 1
        })
    );
}

#[test]
fn cell_count_mismatch_is_an_error() {
    let doc = "blocks[1]{id,depth,state,props,body,title}:\n  a,0,DONE\n";
    let err = parse(doc).unwrap_err();
    assert!(
        matches!(err, ToonError::CellCountMismatch { .. }),
        "got {err:?}"
    );
}

#[test]
fn depth_jump_is_an_error() {
    // depth goes 0 -> 2 with no depth-1 parent between.
    let doc = "blocks[2]{id,depth,state,props,body,title}:\n  a,0,,,,root\n  b,2,,,,orphan\n";
    let err = parse(doc).unwrap_err();
    assert!(
        matches!(
            err,
            ToonError::DepthJump {
                depth: 2,
                prev: 0,
                ..
            }
        ),
        "got {err:?}"
    );
}

#[test]
fn non_root_start_is_an_error() {
    let doc = "blocks[1]{id,depth,state,props,body,title}:\n  a,3,,,,x\n";
    let err = parse(doc).unwrap_err();
    assert!(
        matches!(err, ToonError::NonRootStart { depth: 3, .. }),
        "got {err:?}"
    );
}

#[test]
fn bad_reserved_prop_is_an_error() {
    // @kind must be src|img.
    let doc = "blocks[1]{id,depth,state,props,body,title}:\n  a,0,,@kind=weird,,x\n";
    let err = parse(doc).unwrap_err();
    assert!(
        matches!(err, ToonError::BadReservedProp { .. }),
        "got {err:?}"
    );
}
