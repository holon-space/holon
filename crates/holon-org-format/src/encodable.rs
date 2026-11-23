//! Refuses block text that org storage does not give back as written.
//!
//! The decision is the org parser's own: the text is rendered as a block,
//! parsed again, and compared.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::Operation;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::decision_block::BlockDecisionError;

use crate::OrgRenderer;
use crate::parse_org_file;

const FILE: &str = "/probe/probe.org";
const ROOT: &str = "/probe";
const SKELETON: &str = "#+ID: probe\n* Topic\n:PROPERTIES:\n:ID: probe-topic\n:END:\n";
const CHILD: &str = "probe-child";
/// A stored block carries a property drawer ahead of its text, which changes
/// how org reads the text.
const PROPERTY: &str = "probe";

fn parse(source: &str) -> crate::parser::ParseResult {
    parse_org_file(
        Path::new(FILE),
        source,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("the probe skeleton parses")
}

/// Whether org stores `content` as the text of a block and reads it back
/// unchanged. Text org cannot render or parse does not read back.
fn reads_back(content: &str) -> bool {
    let skeleton = parse(SKELETON);
    let topic = skeleton.blocks[0].id.clone();
    let child = EntityUri::block(CHILD);
    let mut blocks = skeleton.blocks;
    let mut block = Block::new_text(child.clone(), topic, content);
    block.set_property(PROPERTY, Value::String(PROPERTY.to_string()));
    blocks.push(block);
    let render = |document: &Block, blocks: &[Block]| {
        OrgRenderer::render_document(document, blocks, Path::new(FILE), &document.id)
            .map(|rendered| rendered.text)
    };
    let Ok(text) = render(&skeleton.document, &blocks) else {
        return false;
    };
    let Ok(parsed) = parse_org_file(
        Path::new(FILE),
        &text,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    ) else {
        return false;
    };
    let kept = parsed
        .blocks
        .iter()
        .find(|b| b.id == child)
        .is_some_and(|b| b.content == content);
    kept && render(&parsed.document, &parsed.blocks).is_ok_and(|again| again == text)
}

/// Refuses a multi-line block text org would change. It names the first line
/// that, replaced by plain text, makes the text read back; failing that, the
/// first line whose addition makes the text stop reading back.
fn check(content: &str) -> Result<(), BlockDecisionError> {
    if reads_back(content) {
        return Ok(());
    }
    let lines: Vec<&str> = content.split('\n').collect();
    let blamed = (1..lines.len())
        .find(|&i| {
            let mut plain = lines.clone();
            plain[i] = "x";
            reads_back(&plain.join("\n"))
        })
        .or_else(|| (1..lines.len()).find(|&i| !reads_back(&lines[..=i].join("\n"))))
        .unwrap_or_else(|| panic!("org changes {content:?} but no line of it is to blame"));
    Err(BlockDecisionError::Unencodable {
        field: "content",
        text: lines[blamed].to_string(),
        reason: "org does not read this line back as written",
    })
}

/// Refuses `ops` that create a block whose multi-line content org would not
/// read back as written.
pub(crate) fn refuse_unreadable_content(ops: &[Operation]) -> Result<(), BlockDecisionError> {
    for op in ops.iter().filter(|op| op.op_name.as_str() == "create") {
        let content = op
            .params
            .get("content")
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| panic!("a create operation carries text content: {op:?}"));
        if content.contains('\n') {
            check(content)?;
        }
    }
    Ok(())
}
