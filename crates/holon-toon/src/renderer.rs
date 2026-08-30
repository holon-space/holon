//! Forest -> TOON. Pre-order flatten into one tabular array.

use crate::models::BlockId;
use crate::models::ContentType;
use crate::models::Forest;
use crate::models::ToonBlock;
use crate::schema;
use crate::toon::encode_cell;
use crate::toon::encode_list;
use crate::toon::encode_props_pairs;
use crate::toon::join_row;

/// Render a forest as a TOON document (header + indented rows, trailing
/// newline).
pub fn render(forest: &Forest) -> String {
    let rows = forest.flatten();
    let mut out = String::new();
    out.push_str(&schema::header_line(rows.len()));
    out.push('\n');
    for (depth, block) in &rows {
        out.push_str(schema::ROW_INDENT);
        out.push_str(&render_row(*depth, block));
        out.push('\n');
    }
    out
}

fn render_row(depth: u16, block: &ToonBlock) -> String {
    let (body_cell, title_cell) = text_slots(block);
    let cells = vec![
        encode_cell(block.id.as_str()),
        depth.to_string(),
        encode_cell(block.state.as_ref().map(|s| s.keyword()).unwrap_or("")),
        encode_cell(&encode_props_pairs(&props_pairs(block))),
        encode_cell(&body_cell),
        encode_cell(&title_cell),
    ];
    join_row(&cells)
}

/// The two text columns depend on content kind: Text uses (body, title),
/// Source packs the code into `body` (title empty), Image packs the path into
/// `body` (title empty).
fn text_slots(block: &ToonBlock) -> (String, String) {
    match block.content_type {
        ContentType::Text => (block.body.clone().unwrap_or_default(), block.title.clone()),
        ContentType::Source => (block.body.clone().unwrap_or_default(), String::new()),
        ContentType::Image => (
            block.content_path.clone().unwrap_or_default(),
            String::new(),
        ),
    }
}

/// Ordered `(key, value)` pairs for the props cell — reserved fields first (in
/// a fixed order), then arbitrary drawer keys (already sorted in the BTreeMap).
fn props_pairs(block: &ToonBlock) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();

    if let Some(p) = block.priority {
        pairs.push((schema::K_PRI.into(), p.letter().to_string()));
    }
    if !block.tags.is_empty() {
        pairs.push((schema::K_TAGS.into(), encode_list(&block.tags)));
    }
    match block.content_type {
        ContentType::Source => pairs.push((schema::K_KIND.into(), schema::KIND_SRC.into())),
        ContentType::Image => pairs.push((schema::K_KIND.into(), schema::KIND_IMG.into())),
        ContentType::Text => {}
    }
    if let Some(lang) = &block.source_language {
        pairs.push((schema::K_LANG.into(), lang.clone()));
    }
    if let Some(name) = &block.source_name {
        pairs.push((schema::K_NAME.into(), name.clone()));
    }
    if let Some(s) = &block.scheduled {
        pairs.push((schema::K_SCHED.into(), s.clone()));
    }
    if let Some(d) = &block.deadline {
        pairs.push((schema::K_DEADLINE.into(), d.clone()));
    }
    if !block.requires.is_empty() {
        pairs.push((
            schema::K_REQUIRES.into(),
            encode_list(&id_strings(&block.requires)),
        ));
    }
    if !block.advice_suppressed.is_empty() {
        pairs.push((
            schema::K_ADVICE.into(),
            encode_list(&id_strings(&block.advice_suppressed)),
        ));
    }
    if !block.contributes_to.is_empty() {
        pairs.push((
            schema::K_CONTRIBUTES.into(),
            encode_list(&id_strings(&block.contributes_to)),
        ));
    }
    if block.collapsed {
        pairs.push((schema::K_COLLAPSED.into(), "t".into()));
    }
    if block.widget_only {
        pairs.push((schema::K_WIDGET_ONLY.into(), "t".into()));
    }
    for (k, v) in &block.properties {
        pairs.push((k.clone(), v.clone()));
    }

    pairs
}

fn id_strings(ids: &[BlockId]) -> Vec<String> {
    ids.iter().map(|id| id.as_str().to_string()).collect()
}
