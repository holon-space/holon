//! Org file serialization utilities

use holon_api::ContentType;
use holon_api::Value;
use holon_api::block::Block;
use holon_orgmode::models::OrgBlockExt;

/// Internal properties that Loro/Org adds but reference model doesn't track
pub const INTERNAL_PROPS: &[&str] = &["sequence", "level", "ID", "id", "created_at", "updated_at"];

/// Extract the first :ID: property value from org content.
///
/// This is useful for waiting on a specific block to sync after writing an org file.
pub fn extract_first_block_id(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(":ID:") {
            return Some(trimmed.strip_prefix(":ID:")?.trim().to_string());
        }
    }
    None
}

/// Serialize Blocks to Org file format
pub fn serialize_blocks_to_org(blocks: &[&Block], doc_uri: &str) -> String {
    let mut root_blocks: Vec<&&Block> = blocks
        .iter()
        .filter(|b| b.parent_id.as_raw_str() == doc_uri)
        .collect();
    // Match production OrgRenderer sorting: source first, then sequence, then ID
    root_blocks.sort_by(|a, b| {
        let a_is_source = (a.content_type == ContentType::Source) as u8;
        let b_is_source = (b.content_type == ContentType::Source) as u8;
        b_is_source
            .cmp(&a_is_source)
            .then_with(|| a.sequence().cmp(&b.sequence()))
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });

    let mut result = String::new();

    for block in root_blocks {
        serialize_block_recursive(block, blocks, &mut result, 1);
    }

    result
}

/// Recursively serialize a block and its children
pub fn serialize_block_recursive(
    block: &Block,
    all_blocks: &[&Block],
    result: &mut String,
    level: usize,
) {
    if block.content_type == ContentType::Source {
        let language = block
            .source_language
            .as_ref()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "text".to_string());
        result.push_str(&format!("#+BEGIN_SRC {} :id {}\n", language, block.id.id()));
        result.push_str(&block.content);
        if !block.content.ends_with('\n') {
            result.push('\n');
        }
        result.push_str("#+END_SRC\n");
        return;
    }

    let mut headline = String::new();
    headline.push_str(&"*".repeat(level));
    headline.push(' ');

    if let Some(ref task_state) = block.task_state() {
        headline.push_str(task_state.keyword.as_str());
        headline.push(' ');
    }

    if let Some(priority) = block.priority() {
        headline.push_str(&format!("[#{}] ", priority.to_letter()));
    }

    headline.push_str(&block.content);

    let tags = block.tags();
    if !tags.is_empty() {
        headline.push_str(&format!(" {}", tags.to_org()));
    }

    result.push_str(&headline);
    result.push('\n');

    if block.scheduled().is_some() || block.deadline().is_some() {
        if let Some(ref scheduled) = block.scheduled() {
            result.push_str(&format!("SCHEDULED: {}\n", scheduled));
        }
        if let Some(ref deadline) = block.deadline() {
            result.push_str(&format!("DEADLINE: {}\n", deadline));
        }
    }

    result.push_str(":PROPERTIES:\n");
    result.push_str(&format!(":ID: {}\n", block.id.id()));

    for (k, v) in &block.properties {
        if k != "ID" && k != "id" && !INTERNAL_PROPS.contains(&k.as_str()) {
            if matches!(
                k.as_str(),
                "task_state" | "priority" | "tags" | "scheduled" | "deadline"
            ) {
                continue;
            }
            let value_str = match v {
                Value::String(s) => s.clone(),
                Value::Integer(i) => i.to_string(),
                Value::Float(f) => f.to_string(),
                Value::Boolean(b) => b.to_string(),
                Value::DateTime(s) => s.clone(),
                Value::Json(s) => s.clone(),
                Value::Array(_) => "[array]".to_string(),
                Value::Object(_) => "[object]".to_string(),
                Value::Null => "".to_string(),
            };
            result.push_str(&format!(":{}: {}\n", k, value_str));
        }
    }
    result.push_str(":END:\n");

    let mut children: Vec<&&Block> = all_blocks
        .iter()
        .filter(|b| b.parent_id.as_raw_str() == block.id.as_str())
        .collect();
    // Match production OrgRenderer sorting: source first, then sequence, then ID
    children.sort_by(|a, b| {
        let a_is_source = (a.content_type == ContentType::Source) as u8;
        let b_is_source = (b.content_type == ContentType::Source) as u8;
        b_is_source
            .cmp(&a_is_source)
            .then_with(|| a.sequence().cmp(&b.sequence()))
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });

    for child in children {
        serialize_block_recursive(child, all_blocks, result, level + 1);
    }
}

/// Assign sequence numbers to blocks that don't already have them set.
///
/// For each parent group where no child has a non-zero sequence (i.e., sequences
/// were not set from file order by WriteOrgFile), assigns the canonical ordering:
/// source blocks first, then text blocks, sorted by ID within each group.
/// This matches the ordering used by `serialize_blocks_to_org`.
///
/// Parent groups where any child already has sequence > 0 are skipped, since
/// those sequences were set from actual file order and should be preserved.
pub fn assign_reference_sequences(blocks: &mut [Block]) {
    use std::collections::{HashMap, HashSet};

    let parent_ids: HashSet<String> = blocks.iter().map(|b| b.parent_id.to_string()).collect();

    let mut seq_map: HashMap<String, i64> = HashMap::new();
    for parent_id in &parent_ids {
        let children: Vec<&Block> = blocks
            .iter()
            .filter(|b| b.parent_id.as_raw_str() == parent_id.as_str())
            .collect();
        // Skip if any child already has a sequence set (from file order)
        if children.iter().any(|b| b.sequence() > 0) {
            continue;
        }
        let mut sorted: Vec<(String, bool)> = children
            .iter()
            .map(|b| (b.id.to_string(), b.content_type == ContentType::Source))
            .collect();
        sorted.sort_by(|(a_id, a_src), (b_id, b_src)| {
            (*b_src as u8)
                .cmp(&(*a_src as u8))
                .then_with(|| a_id.cmp(b_id))
        });
        for (i, (id, _)) in sorted.iter().enumerate() {
            seq_map.insert(id.clone(), i as i64);
        }
    }

    for block in blocks.iter_mut() {
        if let Some(&seq) = seq_map.get(block.id.as_str()) {
            block.set_sequence(seq);
        }
    }
}

/// Force-assign canonical sequence numbers to all blocks, overwriting any existing values.
///
/// Used when the org file is re-written via `serialize_blocks_to_org` (e.g., after
/// an external mutation), which always sorts in canonical order regardless of
/// existing sequences.
pub fn assign_reference_sequences_canonical(blocks: &mut [Block]) {
    use std::collections::{HashMap, HashSet};

    let parent_ids: HashSet<String> = blocks.iter().map(|b| b.parent_id.to_string()).collect();

    let mut seq_map: HashMap<String, i64> = HashMap::new();
    for parent_id in &parent_ids {
        let mut children: Vec<(String, bool, i64)> = blocks
            .iter()
            .filter(|b| b.parent_id.as_raw_str() == parent_id.as_str())
            .map(|b| {
                (
                    b.id.to_string(),
                    b.content_type == ContentType::Source,
                    b.sequence(),
                )
            })
            .collect();
        // Match production OrgRenderer sorting: source first, then sequence, then ID
        children.sort_by(|(a_id, a_src, a_seq), (b_id, b_src, b_seq)| {
            (*b_src as u8)
                .cmp(&(*a_src as u8))
                .then_with(|| a_seq.cmp(b_seq))
                .then_with(|| a_id.cmp(b_id))
        });
        for (i, (id, _, _)) in children.iter().enumerate() {
            seq_map.insert(id.clone(), i as i64);
        }
    }

    for block in blocks.iter_mut() {
        if let Some(&seq) = seq_map.get(block.id.as_str()) {
            block.set_sequence(seq);
        }
    }
}
