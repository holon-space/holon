//! The import base: what LogSeq last looked like, per block, per field.
//!
//! The base is what turns a one-way push into a replica. It is the third side
//! of every later three-way merge, and it is the whole of echo suppression:
//! after Holon pushes a value, it records that value here, so the next import
//! sees `logseq_now == base` and reports no LogSeq-side change. No timestamps,
//! no operation ids, no sequence numbers.
//!
//! **The base must carry every field a merge will compare.** A field the base
//! omits is a field whose LogSeq-side change is invisible: the diff cannot see
//! it, so the merge silently keeps Holon's value and no conflict is ever
//! surfaced. That is why this mirrors the whole projection — content, parent,
//! position among siblings, properties and edges — rather than the one field
//! the first push happens to write.
//!
//! Keyed by the bare LogSeq uuid, which is also the Holon block id
//! (`EntityUri::block(uuid)`, see `project`). Identity is shared across the
//! boundary, so no mapping table exists here and none should be added.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use holon_api::Block;
use holon_api::EntityUri;
use holon_api::Value;
use serde::Deserialize;
use serde::Serialize;

use crate::ImportResult;

/// The layout of a persisted base.
///
/// Stored so a later widening is an explicit, detectable migration rather than
/// a silent misread of an older file: a base written before a field existed
/// would otherwise deserialize with that field defaulted and report every block
/// as changed.
pub const BASE_FORMAT_VERSION: u32 = 1;

/// What can go wrong maintaining the base.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BaseError {
    /// Advancing a block the base has never seen would put the base AHEAD of
    /// reality — it would claim LogSeq holds something that was never
    /// confirmed. A genuinely new block goes through
    /// [`ImportBase::witness_create`], which says so at the call site.
    #[error(
        "cannot advance {uuid}: the base has no such block, so advancing would claim \
         LogSeq holds a block that was never observed. A block Holon created goes \
         through witness_create instead"
    )]
    AdvanceUnknown { uuid: String },
    /// Retracting a block the base has never seen means the caller and the base
    /// disagree about what was pushed.
    #[error("cannot retract {uuid}: the base has no such block")]
    RetractUnknown { uuid: String },
    /// A create was witnessed for a uuid the base already tracks.
    #[error("cannot witness a create for {uuid}: the base already tracks it")]
    CreateExisting { uuid: String },
}

/// The last-observed LogSeq state of one block.
///
/// `PartialEq` but not `Eq`: a property may hold a float, and a NaN-valued
/// property therefore compares unequal to itself and reports as perpetually
/// changed. That is an honest reading of a value LogSeq itself cannot compare,
/// and it errs toward refusing a push rather than overwriting one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BaseBlock {
    pub content: String,
    /// The parent's URI as a string, so a re-parent in LogSeq is a diff.
    pub parent_id: String,
    /// This block's index among its parent's ordered children, so a re-order in
    /// LogSeq is a diff. `None` when the parent states no order.
    pub position: Option<usize>,
    pub tags: Vec<String>,
    pub requires: Vec<String>,
    pub contributes_to: Vec<String>,
    pub advice_suppressed: Vec<String>,
    pub properties: BTreeMap<String, Value>,
}

/// The last-observed LogSeq state of every imported block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportBase {
    version: u32,
    /// Bare LogSeq uuid → last-observed state. Ordered so a diff of two base
    /// files is readable.
    ///
    /// This map's order is not on its own enough to make a saved base
    /// byte-stable — the values reach nested `HashMap`s. Stability is
    /// established by [`to_canonical_json`](Self::to_canonical_json), which is
    /// what `save` writes.
    blocks: BTreeMap<String, BaseBlock>,
}

impl Default for ImportBase {
    fn default() -> Self {
        Self {
            version: BASE_FORMAT_VERSION,
            blocks: BTreeMap::new(),
        }
    }
}

/// How two bases differ, in the direction `self` → `other`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseDiff {
    /// Uuids present in `other` and not in `self`.
    pub created: Vec<String>,
    /// Uuids present in both whose observed state differs in any field.
    pub changed: Vec<String>,
    /// Uuids present in `self` and not in `other`.
    pub removed: Vec<String>,
}

impl BaseDiff {
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }

    pub fn len(&self) -> usize {
        self.created.len() + self.changed.len() + self.removed.len()
    }
}

fn uris(list: &[EntityUri]) -> Vec<String> {
    let mut out: Vec<String> = list.iter().map(EntityUri::to_string).collect();
    // The projection's edge order is not authored, so an unordered set compared
    // in arrival order would report spurious changes.
    out.sort();
    out
}

impl ImportBase {
    /// The base as it stands immediately after an import: exactly what LogSeq
    /// held at that moment, every projected field included.
    pub fn from_import(result: &ImportResult) -> Self {
        let position_of: BTreeMap<&EntityUri, usize> = result
            .ordered_children
            .values()
            .flat_map(|children| children.iter().enumerate().map(|(i, id)| (id, i)))
            .collect();

        let blocks = result
            .blocks
            .iter()
            .map(|block| (block.id.id().to_string(), observe(block, &position_of)))
            .collect();
        Self {
            version: BASE_FORMAT_VERSION,
            blocks,
        }
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn get(&self, uuid: &str) -> Option<&BaseBlock> {
        self.blocks.get(uuid)
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn uuids(&self) -> impl Iterator<Item = &str> {
        self.blocks.keys().map(String::as_str)
    }

    /// Record that a block Holon already tracked now holds `observed`.
    ///
    /// Called only after LogSeq confirms the write, so the base can lag reality
    /// but never lead it. Advancing an unknown uuid is refused rather than
    /// inserted: the base leading reality makes a never-applied edit look
    /// applied, which is the one failure this ordering exists to prevent.
    pub fn advance(&mut self, uuid: &str, observed: BaseBlock) -> Result<(), BaseError> {
        if !self.blocks.contains_key(uuid) {
            return Err(BaseError::AdvanceUnknown {
                uuid: uuid.to_string(),
            });
        }
        self.blocks.insert(uuid.to_string(), observed);
        Ok(())
    }

    /// Record a block Holon created and LogSeq confirmed.
    ///
    /// Separate from [`advance`](Self::advance) so that growing the base is
    /// always a deliberate statement at the call site, never a side effect of
    /// advancing the wrong uuid.
    pub fn witness_create(&mut self, uuid: &str, observed: BaseBlock) -> Result<(), BaseError> {
        if self.blocks.contains_key(uuid) {
            return Err(BaseError::CreateExisting {
                uuid: uuid.to_string(),
            });
        }
        self.blocks.insert(uuid.to_string(), observed);
        Ok(())
    }

    /// Record that a block Holon deleted is gone from LogSeq.
    ///
    /// Without this the base would still hold the block, and the next import
    /// would report Holon's own delete as a LogSeq-side removal — the echo the
    /// base exists to make invisible.
    pub fn retract(&mut self, uuid: &str) -> Result<(), BaseError> {
        self.blocks
            .remove(uuid)
            .map(|_| ())
            .ok_or_else(|| BaseError::RetractUnknown {
                uuid: uuid.to_string(),
            })
    }

    /// What changed between this base and `other`.
    ///
    /// Both sides are ordered maps, so the result is deterministic and a
    /// re-import that changed nothing produces an empty diff rather than a
    /// differently-ordered one.
    pub fn diff_against(&self, other: &Self) -> BaseDiff {
        let mut diff = BaseDiff::default();
        for (uuid, mine) in &self.blocks {
            match other.blocks.get(uuid) {
                None => diff.removed.push(uuid.clone()),
                Some(theirs) if theirs != mine => diff.changed.push(uuid.clone()),
                Some(_) => {}
            }
        }
        for uuid in other.blocks.keys() {
            if !self.blocks.contains_key(uuid) {
                diff.created.push(uuid.clone());
            }
        }
        diff
    }

    /// The base's canonical bytes — the exact form [`save`](Self::save) writes.
    ///
    /// Every object's keys are sorted, recursively. That is not cosmetic: a
    /// `_logseq_raw/*` property carrying a nested map arrives as
    /// `holon_api::Value::Object`, a `HashMap`, whose iteration order follows
    /// the process's hash seed — so two imports of one graph serialize
    /// differently unless an order is imposed somewhere.
    ///
    /// Imposed HERE, at the serialization boundary, rather than by ordering
    /// `Value` itself: `Value` is flutter_rust_bridge-shaped and reaches most
    /// of the tree, and only the persisted form actually needs an order. The
    /// cost of the choice is that in-memory `Value`s stay unordered, so
    /// nothing may assume a nested map's key order survives a round trip
    /// through the base.
    pub fn to_canonical_json(&self) -> Result<String> {
        let value = serde_json::to_value(self).context("serializing the import base")?;
        serde_json::to_string_pretty(&sort_keys(value))
            .context("re-serializing the import base in canonical form")
    }

    /// Persist the base next to the graph it describes.
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = self
            .to_canonical_json()
            .with_context(|| format!("serializing the import base for {}", path.display()))?;
        std::fs::write(path, json)
            .with_context(|| format!("writing the import base to {}", path.display()))
    }

    /// Read a persisted base, refusing a layout this build does not understand.
    ///
    /// The version is checked BEFORE the base is deserialized, so a file from
    /// an older layout is named for what it is instead of surfacing as
    /// whichever field serde happened to miss first. `version` carries no
    /// serde default on purpose: a defaulted version would let a
    /// pre-versioning base load as version 0 and then report every block as
    /// changed.
    pub fn load(path: &Path) -> Result<Self> {
        let json = std::fs::read_to_string(path)
            .with_context(|| format!("reading the import base at {}", path.display()))?;
        let probe: serde_json::Value = serde_json::from_str(&json)
            .with_context(|| format!("parsing the import base at {}", path.display()))?;
        let Some(version) = probe.get("version").and_then(serde_json::Value::as_u64) else {
            anyhow::bail!(
                "the import base at {} carries no format version, so it predates base \
                 versioning and describes only part of a block. Re-import the graph to \
                 rebuild the base; comparing against it would miss every field the old \
                 layout did not store",
                path.display()
            );
        };
        anyhow::ensure!(
            version == u64::from(BASE_FORMAT_VERSION),
            "the import base at {} is format version {}, but this build writes and reads \
             version {}. Re-import the graph to rebuild the base; merging across layouts \
             would report every block as changed",
            path.display(),
            version,
            BASE_FORMAT_VERSION
        );
        serde_json::from_str(&json)
            .with_context(|| format!("parsing the import base at {}", path.display()))
    }
}

/// `value` with every object's keys in sorted order, recursively.
///
/// Arrays keep their order: a list's order is data, only a map's is arbitrary.
fn sort_keys(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().map(|(k, v)| (k, sort_keys(v))).collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(entries.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(sort_keys).collect())
        }
        scalar => scalar,
    }
}

fn observe(block: &Block, position_of: &BTreeMap<&EntityUri, usize>) -> BaseBlock {
    BaseBlock {
        content: block.content.clone(),
        parent_id: block.parent_id.to_string(),
        position: position_of.get(&block.id).copied(),
        tags: {
            let mut tags: Vec<String> = block.tags.iter().map(ToString::to_string).collect();
            tags.sort();
            tags
        },
        requires: uris(&block.requires),
        contributes_to: uris(&block.contributes_to),
        advice_suppressed: uris(&block.advice_suppressed),
        properties: block
            .properties
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use holon_api::Tags;

    use super::*;

    /// Every field of `observe`'s output, checked against a block where no
    /// field holds its default.
    ///
    /// The committed fixture cannot do this job: `requires`,
    /// `contributes_to` and `advice_suppressed` are empty on all 206 of its
    /// blocks, so hard-coding any of the three to `vec![]` in [`observe`]
    /// leaves the whole suite green. The diff-layer tests do not catch it
    /// either — they build `BaseBlock`s directly and never call `observe`.
    ///
    /// The assertion destructures `BaseBlock` exhaustively, so adding a ninth
    /// field breaks this test at COMPILE time. That is deliberate: a new field
    /// that `observe` forgets to populate is a field whose LogSeq-side change
    /// is invisible to every later merge, and it must not be possible to add
    /// one silently.
    #[test]
    fn observe_carries_every_field_of_the_projection() {
        let id = EntityUri::block("11111111-1111-4111-8111-111111111111");
        let block = Block {
            id: id.clone(),
            parent_id: EntityUri::block("22222222-2222-4222-8222-222222222222"),
            content: "the content".to_string(),
            tags: Tags::from_tag_iter(["Page".to_string(), "Task".to_string()]),
            requires: vec![EntityUri::block("33333333-3333-4333-8333-333333333333")],
            contributes_to: vec![EntityUri::block("44444444-4444-4444-8444-444444444444")],
            advice_suppressed: vec![EntityUri::block("55555555-5555-4555-8555-555555555555")],
            properties: [("TODO".to_string(), Value::String("DONE".to_string()))]
                .into_iter()
                .collect(),
            ..Block::default()
        };
        let position_of = BTreeMap::from([(&id, 7usize)]);

        // Exhaustive on purpose — see the doc comment.
        let BaseBlock {
            content,
            parent_id,
            position,
            tags,
            requires,
            contributes_to,
            advice_suppressed,
            properties,
        } = observe(&block, &position_of);

        assert_eq!(content, "the content");
        assert_eq!(parent_id, "block:22222222-2222-4222-8222-222222222222");
        assert_eq!(position, Some(7));
        assert_eq!(tags, vec!["Page".to_string(), "Task".to_string()]);
        assert_eq!(
            requires,
            vec!["block:33333333-3333-4333-8333-333333333333".to_string()]
        );
        assert_eq!(
            contributes_to,
            vec!["block:44444444-4444-4444-8444-444444444444".to_string()]
        );
        assert_eq!(
            advice_suppressed,
            vec!["block:55555555-5555-4555-8555-555555555555".to_string()]
        );
        assert_eq!(
            properties,
            BTreeMap::from([("TODO".to_string(), Value::String("DONE".to_string()))])
        );
    }

    /// A block whose parent states no order carries no position, rather than a
    /// position of 0 that would read as "first child".
    #[test]
    fn observe_reports_no_position_when_the_parent_states_no_order() {
        let block = Block {
            id: EntityUri::block("11111111-1111-4111-8111-111111111111"),
            ..Block::default()
        };
        assert_eq!(observe(&block, &BTreeMap::new()).position, None);
    }

    /// The projection's edge order is not authored, so `observe` sorts. Without
    /// it two imports of the same graph could differ only in arrival order and
    /// report every edge-bearing block as changed.
    #[test]
    fn observe_sorts_edges_and_tags_so_arrival_order_is_not_a_diff() {
        let mut block = Block {
            id: EntityUri::block("11111111-1111-4111-8111-111111111111"),
            tags: Tags::from_tag_iter(["Zeta".to_string(), "Alpha".to_string()]),
            requires: vec![EntityUri::block("99999999-9999-4999-8999-999999999999")],
            ..Block::default()
        };
        let first = observe(&block, &BTreeMap::new());

        block
            .requires
            .insert(0, EntityUri::block("00000000-0000-4000-8000-000000000000"));
        let second = observe(&block, &BTreeMap::new());

        assert_eq!(first.tags, vec!["Alpha".to_string(), "Zeta".to_string()]);
        assert_eq!(
            second.requires,
            vec![
                "block:00000000-0000-4000-8000-000000000000".to_string(),
                "block:99999999-9999-4999-8999-999999999999".to_string(),
            ],
            "edges must be sorted, not left in arrival order"
        );
    }
}
