//! Shared block-construction helpers for the markdown flavors: turning a raw
//! inline line into a `Block` with marks, tags, task state, and (when a
//! construct is unsupported) a disclosed opaque block.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::Path;

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

/// The block URI for a bare id, or why org could not carry the id: a declared
/// block can be moved into an org file, so only an id an org id line can hold
/// as the same id is accepted.
pub fn block_id(value: &str) -> Result<EntityUri, impl std::fmt::Display> {
    holon_org_format::DrawerId::parse(value).map(|id| EntityUri::block(id.as_str()))
}

/// The block id a file declares (`carrier` names where), refused by name when
/// [`block_id`] refuses it.
pub fn declared_block_id(carrier: &str, value: &str) -> anyhow::Result<EntityUri> {
    block_id(value).map_err(|e| anyhow::anyhow!("{carrier} declares an unusable id: {e}"))
}

/// Source lines of the ids one file has declared so far, so a second
/// declaration of an id is refused instead of overwriting the first block.
#[derive(Default)]
pub struct IdLedger(HashMap<String, usize>);

impl IdLedger {
    /// `line` is 1-based.
    pub fn declare(&mut self, id: &EntityUri, line: usize) -> anyhow::Result<()> {
        match self.0.entry(id.to_string()) {
            Entry::Occupied(first) => anyhow::bail!(
                "the id {:?} is declared on line {} and again on line {line}",
                id.id(),
                first.get()
            ),
            Entry::Vacant(slot) => {
                slot.insert(line);
                Ok(())
            }
        }
    }
}

/// Refuse an authored property key the store cannot hold as a property: a
/// `block_raw` column name, or an internal key (hidden from the drawer, so it
/// would reach neither the store nor the disk). `at` says where the key was
/// authored.
pub fn refuse_unstorable_key(key: &str, at: impl std::fmt::Display) -> anyhow::Result<()> {
    if holon_api::schema::is_block_column(key)
        || holon_org_format::models::is_hidden_drawer_key(key)
    {
        anyhow::bail!(
            "{at}: the property `{key}` names a storage column or an internal key and cannot be stored as a property; rename the property"
        );
    }
    Ok(())
}

/// The error of a render call on a read-only adapter: a write to a foreign
/// vault is loss until anchored write-back exists, so the caller's write fails
/// instead of the app.
pub fn read_only_render(adapter: &str, path: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "{adapter} is read-only (Tier R/O): refused to render {}",
        path.display()
    )
}

/// `path` relative to `root` as UTF-8 text, the vault-relative name a file
/// URI is built from.
pub fn relative_name<'a>(path: &'a Path, root: &Path) -> anyhow::Result<&'a str> {
    path.strip_prefix(root)
        .map_err(|_| {
            anyhow::anyhow!(
                "{} is not under the vault root {}",
                path.display(),
                root.display()
            )
        })?
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("{} is not valid UTF-8", path.display()))
}

/// The file stem of `path`, as the title of a page that declares none.
pub fn file_title(path: &Path) -> anyhow::Result<&str> {
    path.file_stem()
        .ok_or_else(|| anyhow::anyhow!("{} has no file name", path.display()))?
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("{} is not valid UTF-8", path.display()))
}

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
            let letter = rest.as_bytes()[0] as char;
            if let Ok(p) = Priority::from_letter(letter) {
                return (rest[2..].trim_start().to_string(), Some(p));
            }
        }
    }
    (raw.to_string(), None)
}
