//! Deduped datoms → Holon [`Block`]s.
//!
//! Every uuid-bearing entity becomes exactly one Block. Attributes the mapping
//! understands land on Block fields; attributes the schema declares but the
//! mapping does not model are carried as `_logseq_raw/*` properties rather than
//! dropped. LogSeq's fracdex `:block/order` is read to recover the intended
//! sibling sequence but never stored — Holon's consolidator mints order
//! (invariants 2, 3, 10), so the projection reports the sequence and lets the
//! store boundary realize it.

use std::collections::BTreeMap;
use std::collections::HashMap;

use holon_api::Block;
use holon_api::EntityRef;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::PAGE_TAG;
use holon_api::Value;

use crate::Eid;
use crate::ImportError;
use crate::TransitNode;
use crate::datoms::DatomSet;
use crate::datoms::DatomValue;
use crate::datoms::EntityKind;
use crate::datoms::LogseqAttr;
use crate::datoms::LogseqDatom;
use crate::datoms::Schema;
use crate::datoms::Tx;

/// The prefix under which un-modeled LogSeq attributes reach a Block's
/// properties. Disclosed carriage, not silent loss.
const RAW_PREFIX: &str = "_logseq_raw/";

/// The `:db/ident` namespace of a LogSeq class entity. `:block/tags` points at
/// such entities; the segment after the namespace is the Holon tag name.
const CLASS_NAMESPACE: &str = "logseq.class/";

#[derive(Debug)]
pub struct Projection {
    pub blocks: Vec<Block>,
    /// Per parent, the children in the sequence LogSeq's fracdex implies.
    pub ordered_children: HashMap<EntityUri, Vec<EntityUri>>,
}

/// One entity's datoms, indexed by attribute.
struct Entity<'a> {
    e: Eid,
    by_attr: HashMap<&'a LogseqAttr, Vec<&'a LogseqDatom>>,
}

impl<'a> Entity<'a> {
    /// The current value of a cardinality-one attribute: the datom with the
    /// highest transaction. Datoms without a tx slot lose to any that has one.
    fn one(&self, attr: &LogseqAttr) -> Option<&'a DatomValue> {
        self.by_attr
            .get(attr)?
            .iter()
            .max_by_key(|d| d.tx.unwrap_or(Tx(i64::MIN)))
            .map(|d| &d.v)
    }

    fn all(&self, attr: &LogseqAttr) -> &[&'a LogseqDatom] {
        self.by_attr.get(attr).map_or(&[], Vec::as_slice)
    }

    fn str_value(&self, attr: &LogseqAttr) -> Option<&'a str> {
        match self.one(attr)? {
            DatomValue::Node(TransitNode::Str(s)) => Some(s),
            _ => None,
        }
    }

    fn int_value(&self, attr: &LogseqAttr) -> Option<i64> {
        match self.one(attr)? {
            DatomValue::Node(TransitNode::Int(i)) => Some(*i),
            _ => None,
        }
    }

    fn ref_value(&self, attr: &LogseqAttr) -> Option<Eid> {
        match self.one(attr)? {
            DatomValue::Ref(target) => Some(*target),
            _ => None,
        }
    }
}

/// Project a deduped datom set into Blocks.
pub fn project(set: &DatomSet) -> Result<Projection, ImportError> {
    let entities = group_by_entity(&set.datoms);
    let uuids = uuid_index(&entities, set)?;
    let class_names = class_index(&entities);

    let mut blocks = Vec::new();
    // Sibling groups keyed by fracdex string, which sorts lexicographically
    // into exactly LogSeq's intended sequence (verified on the fixture).
    let mut siblings: HashMap<EntityUri, BTreeMap<(Option<String>, String), EntityUri>> =
        HashMap::new();

    for (e, entity) in &entities {
        if set.entities.get(e) != Some(&EntityKind::Block) {
            continue;
        }
        let uuid = uuids
            .get(e)
            .expect("a Block-kind entity has a uuid by construction");
        let id = EntityUri::block(uuid);

        let parent = match entity.ref_value(&LogseqAttr::Parent) {
            None => EntityUri::no_parent(),
            Some(target) => {
                let parent_uuid =
                    uuids
                        .get(&target)
                        .ok_or_else(|| ImportError::DanglingReference {
                            from: e.0,
                            attr: ":block/parent".to_string(),
                            to: target.0,
                        })?;
                EntityUri::block(parent_uuid)
            }
        };

        let mut block = Block {
            id: id.clone(),
            parent_id: parent.clone(),
            content: entity
                .str_value(&LogseqAttr::Title)
                .unwrap_or("")
                .to_string(),
            collapsed: matches!(
                entity.one(&LogseqAttr::Collapsed),
                Some(DatomValue::Node(TransitNode::Bool(true)))
            ),
            // A block with no timestamp datom keeps the epoch rather than
            // "now": a fabricated import time would be indistinguishable from
            // a real one, while 1970 is visibly absent.
            created_at: entity.int_value(&LogseqAttr::CreatedAt).unwrap_or(0),
            updated_at: entity.int_value(&LogseqAttr::UpdatedAt).unwrap_or(0),
            ..Block::default()
        };
        // `None` means "plain text"; only a block that actually carries links
        // becomes rich, so an unmarked block still projects as plain.
        block.marks = Some(link_marks(&block.content)).filter(|m| !m.is_empty());

        for datom in entity.all(&LogseqAttr::Tags) {
            let DatomValue::Ref(target) = datom.v else {
                continue;
            };
            let name = class_names
                .get(&target)
                .ok_or_else(|| ImportError::DanglingReference {
                    from: e.0,
                    attr: ":block/tags".to_string(),
                    to: target.0,
                })?;
            block.tags.insert(name.clone());
        }

        // `:block/name` is LogSeq's lower-cased page key; its presence is what
        // makes an entity a page.
        if let Some(name) = entity.str_value(&LogseqAttr::Name) {
            if name.contains('/') {
                return Err(ImportError::NamespacePage {
                    name: name.to_string(),
                });
            }
            block.tags.insert(PAGE_TAG);
        }

        if let Some(day) = entity.int_value(&LogseqAttr::JournalDay) {
            block
                .properties
                .insert("journal-day".to_string(), Value::Integer(day));
        }

        carry_raw(entity, &set.schema, &mut block)?;

        if parent != EntityUri::no_parent() {
            let order = entity
                .str_value(&LogseqAttr::Order)
                .map(|order| order.to_string());
            // The uuid breaks ties so an order-less sibling set is still a
            // deterministic sequence rather than hash order.
            siblings
                .entry(parent)
                .or_default()
                .insert((order, uuid.clone()), id.clone());
        }
        blocks.push(block);
    }

    blocks.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    let ordered_children = siblings
        .into_iter()
        .map(|(parent, ordered)| (parent, ordered.into_values().collect()))
        .collect();
    Ok(Projection {
        blocks,
        ordered_children,
    })
}

/// Carry every attribute the projection does not model into `_logseq_raw/*`.
fn carry_raw(entity: &Entity<'_>, schema: &Schema, block: &mut Block) -> Result<(), ImportError> {
    for (attr, datoms) in &entity.by_attr {
        let LogseqAttr::Raw(ident) = attr else {
            continue;
        };
        let key = format!("{RAW_PREFIX}{}", ident.trim_start_matches(':'));
        let value = if schema.is_cardinality_many(ident) {
            Value::Array(datoms.iter().map(|d| raw_value(&d.v)).collect())
        } else {
            let current = entity
                .one(attr)
                .expect("the attribute has at least one datom");
            raw_value(current)
        };
        block.properties.insert(key, value);
    }
    Ok(())
}

/// Render a datom value for opaque carriage. Nested EDN keeps its decoded
/// shape as JSON so nothing about it is interpreted on the way through.
fn raw_value(value: &DatomValue) -> Value {
    match value {
        DatomValue::Ref(target) => Value::Integer(target.0),
        DatomValue::Node(node) => node_value(node),
    }
}

fn node_value(node: &TransitNode) -> Value {
    match node {
        TransitNode::Nil => Value::Null,
        TransitNode::Bool(b) => Value::Boolean(*b),
        TransitNode::Int(i) => Value::Integer(*i),
        TransitNode::Float(f) => Value::Float(f.get()),
        TransitNode::Str(s) => Value::String(s.clone()),
        // The leading marker is kept so a carried keyword stays recognisable
        // as one rather than becoming an ordinary string.
        TransitNode::Keyword(k) => Value::String(format!(":{k}")),
        TransitNode::Symbol(s) => Value::String(s.clone()),
        TransitNode::Uuid(u) => Value::String(u.clone()),
        TransitNode::Instant(t) => Value::String(t.clone()),
        TransitNode::List(items) => Value::Array(items.iter().map(node_value).collect()),
        TransitNode::Map(pairs) => Value::Object(
            pairs
                .iter()
                .map(|(k, v)| (map_key(k), node_value(v)))
                .collect(),
        ),
        TransitNode::Tagged(tag, inner) => Value::Object(
            [(format!("~#{tag}"), node_value(inner))]
                .into_iter()
                .collect(),
        ),
    }
}

fn map_key(node: &TransitNode) -> String {
    match node {
        TransitNode::Keyword(k) => format!(":{k}"),
        TransitNode::Str(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

/// The inline links a LogSeq title carries.
///
/// `:block/refs` is dropped on import because Holon derives its own reference
/// graph — but that derivation (`holon_api::derive_block_links`) is a function
/// of a block's inline MARKS, and marks are a stored field the writer supplies.
/// Nothing re-derives them from content afterwards. So without this the whole
/// reference graph is lost while every import count stays green.
///
/// LogSeq DB keeps references in the title text itself, in two forms:
/// `[[…]]` (page/block reference) and `((…)))` (block embed). When the inner
/// text is a uuid it names an entity we are importing, which is a `Scheme`
/// target; otherwise it is a wiki NAME, which Holon already represents as a
/// dangling `Name` target that resolves if a page with that name appears.
///
/// Offsets are Unicode scalar offsets over `content`, per [`MarkSpan`], and the
/// span covers the brackets so the mark's extent matches the link's text.
fn link_marks(content: &str) -> Vec<MarkSpan> {
    let chars: Vec<char> = content.chars().collect();
    let mut marks = Vec::new();
    let mut i = 0usize;
    while i + 4 <= chars.len() {
        // A `[[…]]` may name a page as well as a node, so a non-uuid inner is
        // still a link. A `((…))` is ONLY ever a node reference, so a non-uuid
        // inner is not a link at all — it is ordinary parenthesised prose, and
        // treating it as a wiki name manufactures a reference the author never
        // wrote (`((rate*(x)))` → a page named `rate*(x`).
        let (closing, names_only_nodes) = match (chars[i], chars[i + 1]) {
            ('[', '[') => ((']', ']'), false),
            ('(', '(') => (((')', ')')), true),
            _ => {
                i += 1;
                continue;
            }
        };
        let Some(close_at) =
            (i + 2..chars.len().saturating_sub(1)).find(|&j| (chars[j], chars[j + 1]) == closing)
        else {
            i += 1;
            continue;
        };
        let inner: String = chars[i + 2..close_at].iter().collect();
        if inner.is_empty() {
            i += 1;
            continue;
        }
        let target = match (uuid::Uuid::parse_str(&inner), names_only_nodes) {
            (Ok(_), _) => EntityRef::Scheme {
                raw: EntityUri::block(&inner).as_str().to_string(),
            },
            (Err(_), false) => EntityRef::Name {
                name: inner.clone(),
            },
            (Err(_), true) => {
                i += 1;
                continue;
            }
        };
        marks.push(MarkSpan::new(
            i,
            close_at + 2,
            InlineMark::Link {
                target,
                label: inner,
            },
        ));
        i = close_at + 2;
    }
    marks
}

fn group_by_entity(datoms: &[LogseqDatom]) -> HashMap<Eid, Entity<'_>> {
    let mut out: HashMap<Eid, Entity<'_>> = HashMap::new();
    for datom in datoms {
        out.entry(datom.e)
            .or_insert_with(|| Entity {
                e: datom.e,
                by_attr: HashMap::new(),
            })
            .by_attr
            .entry(&datom.a)
            .or_default()
            .push(datom);
    }
    out
}

/// Bare uuid per Block-kind entity. A uuid datom whose value is not a uuid is
/// a graph we cannot address, so it stops the import.
fn uuid_index(
    entities: &HashMap<Eid, Entity<'_>>,
    set: &DatomSet,
) -> Result<HashMap<Eid, String>, ImportError> {
    let mut out = HashMap::new();
    for (e, entity) in entities {
        if set.entities.get(e) != Some(&EntityKind::Block) {
            continue;
        }
        match entity.one(&LogseqAttr::Uuid) {
            Some(DatomValue::Node(TransitNode::Uuid(uuid))) => {
                out.insert(*e, uuid.clone());
            }
            other => {
                return Err(ImportError::MalformedDatom {
                    addr: -1,
                    detail: format!(
                        "entity {} carries :block/uuid {:?}, expected a uuid value",
                        entity.e.0, other
                    ),
                });
            }
        }
    }
    Ok(out)
}

/// Tag name per class entity, from its `:db/ident :logseq.class/<Name>`.
fn class_index(entities: &HashMap<Eid, Entity<'_>>) -> HashMap<Eid, String> {
    entities
        .iter()
        .filter_map(|(e, entity)| {
            let DatomValue::Node(TransitNode::Keyword(ident)) = entity.one(&LogseqAttr::DbIdent)?
            else {
                return None;
            };
            let name = ident.strip_prefix(CLASS_NAMESPACE)?;
            Some((*e, name.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datoms::Tx;

    fn datom(e: i64, a: LogseqAttr, v: DatomValue, tx: Option<i64>) -> LogseqDatom {
        LogseqDatom {
            e: Eid(e),
            a,
            v,
            tx: tx.map(Tx),
        }
    }

    fn text(s: &str) -> DatomValue {
        DatomValue::Node(TransitNode::Str(s.to_string()))
    }

    /// The bug this guards: a cardinality-one attribute with one datom per
    /// transaction must resolve to the LATEST, not to whichever the iteration
    /// order reached first.
    #[test]
    fn cardinality_one_resolves_to_the_highest_transaction() {
        let datoms = vec![
            datom(1, LogseqAttr::Title, text("old"), Some(100)),
            datom(1, LogseqAttr::Title, text("new"), Some(200)),
        ];
        let grouped = group_by_entity(&datoms);
        assert_eq!(
            grouped[&Eid(1)].str_value(&LogseqAttr::Title),
            Some("new"),
            "the current title is the one from the latest transaction"
        );
    }

    #[test]
    fn a_datom_with_a_transaction_beats_one_without() {
        let datoms = vec![
            datom(1, LogseqAttr::Title, text("untransacted"), None),
            datom(1, LogseqAttr::Title, text("transacted"), Some(1)),
        ];
        let grouped = group_by_entity(&datoms);
        assert_eq!(
            grouped[&Eid(1)].str_value(&LogseqAttr::Title),
            Some("transacted")
        );
    }

    #[test]
    fn class_entities_yield_their_tag_name() {
        let datoms = vec![datom(
            4,
            LogseqAttr::DbIdent,
            DatomValue::Node(TransitNode::Keyword("logseq.class/Page".into())),
            None,
        )];
        let grouped = group_by_entity(&datoms);
        assert_eq!(
            class_index(&grouped).get(&Eid(4)),
            Some(&"Page".to_string())
        );
    }

    /// A `:db/ident` outside the class namespace is not a tag — `:block/tags`
    /// pointing at one must not silently produce a garbage tag name.
    #[test]
    fn non_class_idents_are_not_tag_names() {
        let datoms = vec![datom(
            79,
            LogseqAttr::DbIdent,
            DatomValue::Node(TransitNode::Keyword("logseq.property/status.done".into())),
            None,
        )];
        let grouped = group_by_entity(&datoms);
        assert!(class_index(&grouped).is_empty());
    }

    /// The fixture's exact case: e206's title. Without this mark the
    /// `block_links` junction stays empty and Project Alpha has no backlinks,
    /// which is the reference graph vanishing while every count stays green.
    #[test]
    fn a_bracketed_uuid_title_yields_a_block_link_mark() {
        let alpha = "6a86cf74-3882-4ebd-a19d-c1fa46f58380";
        let content = format!("Link to [[{alpha}]]");
        let marks = link_marks(&content);
        assert_eq!(marks.len(), 1, "one link, got {marks:?}");
        assert_eq!(marks[0].start, 8, "the span starts at the first bracket");
        assert_eq!(
            marks[0].end,
            content.chars().count(),
            "the span covers the closing brackets"
        );
        let InlineMark::Link { target, .. } = &marks[0].mark else {
            panic!("expected a link mark")
        };
        assert_eq!(
            target,
            &EntityRef::Scheme {
                raw: format!("block:{alpha}")
            },
            "a uuid target names the block being imported"
        );
        // The representation Holon turns into a junction row.
        assert_eq!(target.entity_uri(), Some(EntityUri::block(alpha)));
    }

    /// Ordinary parenthesised prose must NOT become a link. `((rate*(x)))` is
    /// arithmetic, not a reference to a page named `rate*(x` — manufacturing
    /// one is silent graph corruption, and the mis-measured span (the inner
    /// stops at the first `))`) makes it worse.
    #[test]
    fn parenthesised_prose_is_not_a_link() {
        assert!(link_marks("((rate*(x)))").is_empty());
        assert!(link_marks("total ((a+b)) done").is_empty());
        // The bracket form still admits a wiki name — only `((…)` is restricted.
        assert_eq!(link_marks("see [[Project Beta]]").len(), 1);
    }

    /// LogSeq's block-embed form carries a reference too.
    #[test]
    fn a_double_paren_uuid_is_also_a_block_link() {
        let uuid = "6a86c98a-d818-4787-8ff1-e3b619b15f2d";
        let marks = link_marks(&format!("Ref: (({uuid}))"));
        assert_eq!(marks.len(), 1);
        let InlineMark::Link { target, .. } = &marks[0].mark else {
            panic!("expected a link mark")
        };
        assert_eq!(target.entity_uri(), Some(EntityUri::block(uuid)));
    }

    /// A non-uuid target is a wiki NAME — Holon's dangling-name target, which
    /// resolves later if a page by that name appears. Not silently dropped.
    #[test]
    fn a_bracketed_name_yields_a_dangling_name_target() {
        let marks = link_marks("see [[Project Beta]] please");
        assert_eq!(marks.len(), 1);
        let InlineMark::Link { target, label } = &marks[0].mark else {
            panic!("expected a link mark")
        };
        assert_eq!(
            target,
            &EntityRef::Name {
                name: "Project Beta".to_string()
            }
        );
        assert_eq!(label, "Project Beta");
        assert_eq!(target.entity_uri(), None, "a name names no entity yet");
    }

    #[test]
    fn ordinary_text_carries_no_marks() {
        assert!(link_marks("Project Alpha").is_empty());
        assert!(link_marks("").is_empty());
        assert!(
            link_marks("[[]]").is_empty(),
            "an empty target is not a link"
        );
    }

    /// Offsets are Unicode scalar offsets, so a multi-byte prefix must not
    /// shift the span — a byte-offset bug would put the mark mid-character.
    #[test]
    fn spans_are_measured_in_characters_not_bytes() {
        let uuid = "6a86c98a-d818-4787-8ff1-e3b619b15f2d";
        let marks = link_marks(&format!("café [[{uuid}]]"));
        assert_eq!(marks[0].start, 5, "5 characters precede the link");
    }

    #[test]
    fn two_links_in_one_title_both_survive() {
        let a = "6a86cf74-3882-4ebd-a19d-c1fa46f58380";
        let b = "6a86c98a-d818-4787-8ff1-e3b619b15f2d";
        let marks = link_marks(&format!("[[{a}]] and [[{b}]]"));
        assert_eq!(marks.len(), 2);
        assert!(marks[0].end <= marks[1].start, "spans do not overlap");
    }

    /// The digit values of a fracdex key, as base-62 means them.
    fn base62_digits(key: &str) -> Vec<u8> {
        key.chars()
            .map(|c| match c {
                '0'..='9' => c as u8 - b'0',
                'A'..='Z' => c as u8 - b'A' + 10,
                'a'..='z' => c as u8 - b'a' + 36,
                other => unreachable!("generator restricts the alphabet, got {other:?}"),
            })
            .collect()
    }

    proptest::proptest! {
        /// Sibling order is recovered by sorting `:block/order` as a plain
        /// string. That is only correct if byte order agrees with base-62
        /// DIGIT order over the alphabet LogSeq draws keys from. It does —
        /// ASCII puts `0-9` below `A-Z` below `a-z`, matching digit values
        /// 0-9, 10-35, 36-61 — but "it happened to hold on one fixture" is not
        /// an invariant, so this states it over the whole key space, prefix
        /// pairs like `a0`/`a01` included.
        #[test]
        fn byte_order_of_fracdex_keys_matches_base62_digit_order(
            a in "[0-9A-Za-z]{1,4}",
            b in "[0-9A-Za-z]{1,4}",
        ) {
            proptest::prop_assert_eq!(
                a.cmp(&b),
                base62_digits(&a).cmp(&base62_digits(&b)),
                "byte order and base-62 digit order disagree for {:?} vs {:?}", a, b
            );
        }
    }

    #[test]
    fn nested_collections_carry_as_structured_json() {
        let node = TransitNode::Map(vec![(
            TransitNode::Keyword("icon".into()),
            TransitNode::List(vec![TransitNode::Int(1), TransitNode::Str("x".into())]),
        )]);
        let Value::Object(map) = node_value(&node) else {
            panic!("a Transit map carries as an object")
        };
        assert_eq!(
            map.get(":icon"),
            Some(&Value::Array(vec![
                Value::Integer(1),
                Value::String("x".to_string())
            ]))
        );
    }
}
