//! TOON -> Forest. Fail-loud parsing (`Result` everywhere; no silent defaults).

use std::collections::BTreeMap;

use crate::error::Result;
use crate::error::ToonError;
use crate::models::BlockId;
use crate::models::BlockNode;
use crate::models::ContentType;
use crate::models::Forest;
use crate::models::Priority;
use crate::models::TaskState;
use crate::models::ToonBlock;
use crate::schema;
use crate::toon::decode_list;
use crate::toon::decode_props_pairs;
use crate::toon::parse_row;

/// Parse a TOON document into a block forest.
pub fn parse(input: &str) -> Result<Forest> {
    let mut lines = input.lines();

    // Header: first non-empty line.
    let header = loop {
        match lines.next() {
            Some(l) if l.trim().is_empty() => continue,
            Some(l) => break l,
            None => return Err(ToonError::EmptyDocument),
        }
    };
    let declared = parse_header(header)?;

    // Rows: the remaining non-empty lines, indent stripped.
    let row_lines: Vec<&str> = lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.strip_prefix(schema::ROW_INDENT).unwrap_or(l))
        .collect();

    if row_lines.len() != declared {
        return Err(ToonError::RowCountMismatch {
            declared,
            actual: row_lines.len(),
        });
    }

    let mut flat: Vec<(u16, ToonBlock)> = Vec::with_capacity(row_lines.len());
    let mut prev_depth: Option<u16> = None;
    for (i, line) in row_lines.iter().enumerate() {
        let (depth, block) = parse_row_to_block(line, i)?;
        match prev_depth {
            None if depth != 0 => return Err(ToonError::NonRootStart { row: i, depth }),
            Some(prev) if depth > prev + 1 => {
                return Err(ToonError::DepthJump {
                    row: i,
                    depth,
                    prev,
                });
            }
            _ => {}
        }
        prev_depth = Some(depth);
        flat.push((depth, block));
    }

    Ok(reconstruct(flat))
}

/// Validate the exact header line and return the declared row count.
fn parse_header(header: &str) -> Result<usize> {
    let bad = || ToonError::BadHeader {
        got: header.to_string(),
    };
    let prefix = format!("{}[", schema::TABLE_KEY);
    let suffix = format!("]{{{}}}:", schema::COLUMNS.join(","));
    let inner = header
        .strip_prefix(&prefix)
        .and_then(|r| r.strip_suffix(&suffix))
        .ok_or_else(bad)?;
    inner.parse::<usize>().map_err(|_| bad())
}

/// Parse one data row into `(depth, block)`.
fn parse_row_to_block(line: &str, row: usize) -> Result<(u16, ToonBlock)> {
    let cells = parse_row(line, schema::N_COLUMNS, row)?;
    // cells: [id, depth, state, props, body, title]
    let id = BlockId::new(cells[0].clone()).ok_or_else(|| ToonError::BadBlockId {
        row,
        id: cells[0].clone(),
    })?;
    let depth: u16 = cells[1].parse().map_err(|_| ToonError::BadDepth {
        row,
        cell: cells[1].clone(),
    })?;
    let state = if cells[2].is_empty() {
        None
    } else {
        Some(
            TaskState::new(cells[2].clone()).ok_or_else(|| ToonError::BadState {
                row,
                state: cells[2].clone(),
            })?,
        )
    };

    let mut block = ToonBlock::text(id, String::new());
    block.state = state;
    apply_props(&mut block, &cells[3], row)?;
    apply_text_slots(&mut block, &cells[4], &cells[5]);

    Ok((depth, block))
}

/// Decode the props cell and route each pair into the block's typed fields or
/// its arbitrary drawer map.
fn apply_props(block: &mut ToonBlock, cell: &str, row: usize) -> Result<()> {
    let mut properties: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in decode_props_pairs(cell, row)? {
        match key.as_str() {
            schema::K_PRI => {
                let c = value.chars().next().filter(|_| value.len() == 1);
                let p = c.and_then(Priority::from_letter).ok_or_else(|| {
                    ToonError::BadReservedProp {
                        row,
                        key,
                        value: value.clone(),
                        reason: "expected a single priority letter A/B/C".into(),
                    }
                })?;
                block.priority = Some(p);
            }
            schema::K_TAGS => {
                block.tags = decode_list(&value);
            }
            schema::K_KIND => {
                block.content_type = match value.as_str() {
                    schema::KIND_SRC => ContentType::Source,
                    schema::KIND_IMG => ContentType::Image,
                    _ => {
                        return Err(ToonError::BadReservedProp {
                            row,
                            key,
                            value,
                            reason: "expected \"src\" or \"img\"".into(),
                        });
                    }
                };
            }
            schema::K_LANG => block.source_language = Some(value),
            schema::K_NAME => block.source_name = Some(value),
            schema::K_SCHED => block.scheduled = Some(value),
            schema::K_DEADLINE => block.deadline = Some(value),
            schema::K_REQUIRES => {
                block.requires = parse_id_list(&value, row, &key)?;
            }
            schema::K_ADVICE => {
                block.advice_suppressed = parse_id_list(&value, row, &key)?;
            }
            schema::K_COLLAPSED => {
                if value != "t" {
                    return Err(ToonError::BadReservedProp {
                        row,
                        key,
                        value,
                        reason: "expected \"t\"".into(),
                    });
                }
                block.collapsed = true;
            }
            _ => {
                properties.insert(key, value);
            }
        }
    }
    block.properties = properties;
    Ok(())
}

fn parse_id_list(value: &str, row: usize, key: &str) -> Result<Vec<BlockId>> {
    decode_list(value)
        .into_iter()
        .map(|s| {
            BlockId::new(&s).ok_or_else(|| ToonError::BadReservedProp {
                row,
                key: key.to_string(),
                value: value.to_string(),
                reason: format!("{:?} is not a valid bare block id", s),
            })
        })
        .collect()
}

/// Re-hydrate the two text columns into the right block field for its kind.
/// Must run after [`apply_props`] (which sets `content_type`).
fn apply_text_slots(block: &mut ToonBlock, body_cell: &str, title_cell: &str) {
    let opt = |s: &str| {
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    };
    match block.content_type {
        ContentType::Text => {
            block.title = title_cell.to_string();
            block.body = opt(body_cell);
        }
        ContentType::Source => {
            block.title = String::new();
            block.body = opt(body_cell);
        }
        ContentType::Image => {
            block.title = String::new();
            block.content_path = opt(body_cell);
        }
    }
}

/// Rebuild the nested forest from the pre-order `(depth, block)` list. Depth
/// monotonicity was already validated in [`parse`], so this is a plain
/// stack-close-and-push.
fn reconstruct(flat: Vec<(u16, ToonBlock)>) -> Forest {
    struct Frame {
        block: ToonBlock,
        children: Vec<BlockNode>,
    }
    let mut roots: Vec<BlockNode> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();

    let close = |stack: &mut Vec<Frame>, roots: &mut Vec<BlockNode>| {
        let frame = stack.pop().unwrap();
        let node = BlockNode::with_children(frame.block, frame.children);
        match stack.last_mut() {
            Some(parent) => parent.children.push(node),
            None => roots.push(node),
        }
    };

    for (depth, block) in flat {
        while stack.len() > depth as usize {
            close(&mut stack, &mut roots);
        }
        stack.push(Frame {
            block,
            children: Vec::new(),
        });
    }
    while !stack.is_empty() {
        close(&mut stack, &mut roots);
    }

    Forest::new(roots)
}
