//! Shared block-construction helpers for the markdown flavors: turning a raw
//! inline line into a `Block` with marks, tags, task state, and (when a
//! construct is unsupported) a disclosed opaque block.

use holon_api::EntityUri;
use holon_api::Priority;
use holon_api::StateCategory;
use holon_api::TaskState;
use holon_api::Timestamp;
use holon_api::block::Block;
use holon_org_format::OrgBlockExt;

use crate::inline::scan_inline;

/// Internal property naming the unsupported construct kind carried by an opaque
/// block (`_`-prefixed = DECLARED internal per ADR 0025; never round-trips as
/// user content, but disclosed in the UI as "unsupported: <kind>").
pub const FOREIGN_OPAQUE_KEY: &str = "_foreign_opaque";

/// LogSeq/org task markers recognized at the start of a block's content.
pub const TASK_MARKERS: &[&str] = &[
    "TODO",
    "DOING",
    "DONE",
    "LATER",
    "NOW",
    "WAITING",
    "CANCELED",
    "CANCELLED",
    "IN-PROGRESS",
];
const DONE_MARKERS: &[&str] = &["DONE", "CANCELED", "CANCELLED"];

/// Build a text/rich block from raw inline content, extracting marks, tags,
/// a leading task marker, and a `[#A]` priority. `content` is the block body
/// (already stripped of the bullet / heading prefix by the caller).
pub fn text_block(id: EntityUri, parent_id: EntityUri, raw: &str) -> Block {
    let (raw, task) = split_task_marker(raw);
    let (raw, priority) = split_priority(&raw);

    let parsed = scan_inline(&raw);
    let mut block = if parsed.marks.is_empty() {
        Block::new_text(id, parent_id, parsed.content)
    } else {
        Block::new_rich(id, parent_id, parsed.content, parsed.marks)
    };
    for t in parsed.tags {
        block.tags.insert(t);
    }
    if let Some(ts) = task {
        block.set_task_state(Some(ts));
    }
    if let Some(p) = priority {
        block.set_priority(Some(p));
    }
    block
}

/// A disclosed opaque block: exact source bytes preserved in `content`, kind
/// recorded under the internal `_foreign_opaque` property. Never dropped.
pub fn opaque_block(id: EntityUri, parent_id: EntityUri, kind: &str, source: &str) -> Block {
    let mut block = Block::new_text(id, parent_id, source.to_string());
    block.set_property(FOREIGN_OPAQUE_KEY, kind.to_string());
    block
}

/// Set a scheduled/deadline planning timestamp, ignoring an unparseable date
/// loudly-but-locally (a malformed date is not block loss).
pub fn apply_planning(block: &mut Block, line: &str) {
    if let Some(rest) = line.trim().strip_prefix("SCHEDULED:") {
        if let Ok(ts) = Timestamp::parse(rest.trim()) {
            block.set_scheduled(Some(ts));
        }
    } else if let Some(rest) = line.trim().strip_prefix("DEADLINE:") {
        if let Ok(ts) = Timestamp::parse(rest.trim()) {
            block.set_deadline(Some(ts));
        }
    }
}

fn split_task_marker(raw: &str) -> (String, Option<TaskState>) {
    for m in TASK_MARKERS {
        if let Some(rest) = raw.strip_prefix(m) {
            if rest.is_empty() || rest.starts_with(' ') {
                let cat = if DONE_MARKERS.contains(m) {
                    StateCategory::Done
                } else {
                    StateCategory::Active
                };
                return (rest.trim_start().to_string(), Some(TaskState::new(*m, cat)));
            }
        }
    }
    (raw.to_string(), None)
}

fn split_priority(raw: &str) -> (String, Option<Priority>) {
    // `[#A]` immediately after the (already-stripped) task marker.
    let t = raw.trim_start();
    if let Some(rest) = t.strip_prefix("[#") {
        if rest.len() >= 2 && rest.as_bytes()[1] == b']' {
            let letter = &rest[..1];
            if let Ok(p) = Priority::from_letter(letter) {
                return (rest[2..].trim_start().to_string(), Some(p));
            }
        }
    }
    (raw.to_string(), None)
}
