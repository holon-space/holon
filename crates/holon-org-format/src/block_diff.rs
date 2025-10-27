//! Org → Loro diff algorithm
//!
//! Compares parsed org-mode blocks and generates minimal operations to apply to Loro.

use crate::models::OrgBlockExt;
use holon_api::block::Block;
use holon_api::EntityUri;
use std::collections::HashMap;
use tracing::debug;

/// Text operation for applying character-level changes
#[derive(Debug, Clone)]
pub enum TextOp {
    /// Insert text at position
    Insert { pos: usize, text: String },
    /// Delete text starting at position
    Delete { pos: usize, len: usize },
}

/// Compute minimal text operations to transform old text to new text.
///
/// Uses a simple character-level diff algorithm.
/// Returns operations in order (should be applied sequentially).
pub fn compute_text_ops(old: &str, new: &str) -> Vec<TextOp> {
    let mut ops = Vec::new();

    // Simple implementation: use Myers diff algorithm via similar crate
    // For now, use a basic character-by-character approach
    // In production, would use similar::TextDiff for better performance

    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();

    // Simple longest common subsequence approach
    let mut i = 0;
    let mut j = 0;

    while i < old_chars.len() || j < new_chars.len() {
        if i < old_chars.len() && j < new_chars.len() && old_chars[i] == new_chars[j] {
            // Characters match, advance both
            i += 1;
            j += 1;
        } else if j < new_chars.len()
            && (i >= old_chars.len()
                || (i < old_chars.len() && j < new_chars.len() && old_chars[i] != new_chars[j]))
        {
            // Insert new character
            let insert_pos = i;
            let mut insert_text = String::new();
            while j < new_chars.len() && (i >= old_chars.len() || old_chars[i] != new_chars[j]) {
                insert_text.push(new_chars[j]);
                j += 1;
            }
            ops.push(TextOp::Insert {
                pos: insert_pos,
                text: insert_text,
            });
        } else if i < old_chars.len() {
            // Delete old character
            let delete_pos = i;
            let mut delete_len = 0;
            while i < old_chars.len() && (j >= new_chars.len() || old_chars[i] != new_chars[j]) {
                delete_len += old_chars[i].len_utf8();
                i += 1;
            }
            ops.push(TextOp::Delete {
                pos: delete_pos,
                len: delete_len,
            });
        }
    }

    ops
}

/// Represents a change to apply to a Loro document.
#[derive(Debug, Clone)]
pub enum BlockDiff {
    /// A new block was created
    Created {
        block: Block,
        parent_id: Option<EntityUri>,
        after_sibling: Option<EntityUri>,
    },
    /// A block was deleted
    Deleted { id: EntityUri },
    /// Block content changed
    ContentChanged {
        id: EntityUri,
        old: String,
        new: String,
    },
    /// Block was moved to a new parent or position
    Moved {
        id: EntityUri,
        new_parent: Option<EntityUri>,
        after_sibling: Option<EntityUri>,
    },
    /// Block properties changed
    PropertiesChanged {
        id: EntityUri,
        changes: Vec<(String, Option<String>, Option<String>)>, // (key, old_value, new_value)
    },
}

/// Parse properties from a Block's drawer properties.
fn parse_block_properties(block: &Block) -> HashMap<String, String> {
    block.drawer_properties()
}

/// Get combined content (title + body) from a Block.
fn get_block_content(block: &Block) -> String {
    let title = block.org_title();
    let body = block.body();
    let mut content = title;
    if let Some(b) = body {
        if !b.is_empty() {
            content.push('\n');
            content.push_str(&b);
        }
    }
    content
}

/// Compare old and new blocks and generate diffs.
///
/// # Arguments
/// * `old_blocks` - Previously parsed blocks (indexed by ID)
/// * `new_blocks` - Newly parsed blocks (indexed by ID)
///
/// # Returns
/// Vector of diffs to apply
pub fn diff_blocks(
    old_blocks: &HashMap<String, Block>,
    new_blocks: &HashMap<String, Block>,
) -> Vec<BlockDiff> {
    let mut diffs = Vec::new();

    // Find deleted blocks (in old but not in new)
    for (id, _) in old_blocks {
        if !new_blocks.contains_key(id) {
            diffs.push(BlockDiff::Deleted {
                id: EntityUri::from_raw(id),
            });
        }
    }

    // Find created and changed blocks
    for (id, new_block) in new_blocks {
        match old_blocks.get(id) {
            Some(old_block) => {
                // Block exists - check for changes
                let entity_id = EntityUri::from_raw(id);

                // Check content change
                let old_content = get_block_content(old_block);
                let new_content = get_block_content(new_block);
                if old_content != new_content {
                    diffs.push(BlockDiff::ContentChanged {
                        id: entity_id.clone(),
                        old: old_content,
                        new: new_content,
                    });
                }

                // Check parent change (move)
                if old_block.parent_id != new_block.parent_id {
                    diffs.push(BlockDiff::Moved {
                        id: entity_id.clone(),
                        new_parent: Some(new_block.parent_id.clone()),
                        after_sibling: None,
                    });
                }

                // Check property changes
                let old_props = parse_block_properties(old_block);
                let new_props = parse_block_properties(new_block);

                let mut prop_changes = Vec::new();
                let mut all_keys: Vec<_> = old_props.keys().collect();
                for key in new_props.keys() {
                    if !all_keys.contains(&key) {
                        all_keys.push(key);
                    }
                }

                for key in all_keys {
                    let old_val = old_props.get(key);
                    let new_val = new_props.get(key);
                    if old_val != new_val {
                        prop_changes.push((key.clone(), old_val.cloned(), new_val.cloned()));
                    }
                }

                if !prop_changes.is_empty() {
                    diffs.push(BlockDiff::PropertiesChanged {
                        id: entity_id,
                        changes: prop_changes,
                    });
                }
            }
            None => {
                // New block
                diffs.push(BlockDiff::Created {
                    block: new_block.clone(),
                    parent_id: Some(new_block.parent_id.clone()),
                    after_sibling: None,
                });
            }
        }
    }

    debug!(
        "Generated {} diffs from {} old and {} new blocks",
        diffs.len(),
        old_blocks.len(),
        new_blocks.len()
    );
    diffs
}

/// Convert a list of Blocks to a tree-indexed map.
///
/// Returns blocks indexed by their ID for use in diff operations.
pub fn blocks_to_map(blocks: &[Block]) -> HashMap<String, Block> {
    blocks
        .iter()
        .map(|b| {
            let id = b.get_block_id().unwrap_or_else(|| b.id.to_string());
            (id, b.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use holon_api::Value;

    fn create_test_block(id: &str, parent_id: &str, content: &str) -> Block {
        let now = chrono::Utc::now().timestamp_millis();
        let mut properties = HashMap::new();
        properties.insert("ID".to_string(), Value::String("local://test".to_string()));
        Block {
            id: holon_api::EntityUri::from_raw(id),
            parent_id: holon_api::EntityUri::from_raw(parent_id),
            tags: Vec::new(),
            content: content.to_string(),
            content_type: holon_api::types::ContentType::Text,
            source_language: None,
            source_name: None,
            properties,
            marks: None,
            created_at: now,
            updated_at: now,
            sort_key: "A0".to_string(),
        }
    }

    #[test]
    fn test_diff_created() {
        let old_blocks = HashMap::new();
        let mut new_blocks = HashMap::new();

        let new_block = create_test_block("local://test", "root", "Test");
        new_blocks.insert("local://test".to_string(), new_block);

        let diffs = diff_blocks(&old_blocks, &new_blocks);
        assert_eq!(diffs.len(), 1);
        match &diffs[0] {
            BlockDiff::Created { block, .. } => {
                assert_eq!(block.id.as_str(), "local://test");
            }
            _ => panic!("Expected Created diff"),
        }
    }

    #[test]
    fn test_diff_deleted() {
        let mut old_blocks = HashMap::new();
        let new_blocks = HashMap::new();

        let old_block = create_test_block("local://test", "root", "Test");
        old_blocks.insert("local://test".to_string(), old_block);

        let diffs = diff_blocks(&old_blocks, &new_blocks);
        assert_eq!(diffs.len(), 1);
        match &diffs[0] {
            BlockDiff::Deleted { id } => {
                assert_eq!(id.as_str(), "local://test");
            }
            _ => panic!("Expected Deleted diff"),
        }
    }

    #[test]
    fn test_diff_content_changed() {
        let mut old_blocks = HashMap::new();
        let mut new_blocks = HashMap::new();

        let old_block = create_test_block("local://test", "root", "Old");
        old_blocks.insert("local://test".to_string(), old_block);

        let new_block = create_test_block("local://test", "root", "New");
        new_blocks.insert("local://test".to_string(), new_block);

        let diffs = diff_blocks(&old_blocks, &new_blocks);
        assert_eq!(diffs.len(), 1);
        match &diffs[0] {
            BlockDiff::ContentChanged { id, old, new } => {
                assert_eq!(id.as_str(), "local://test");
                assert_eq!(old, "Old");
                assert_eq!(new, "New");
            }
            _ => panic!("Expected ContentChanged diff"),
        }
    }
}
