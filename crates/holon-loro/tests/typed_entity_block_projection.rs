//! Increment 1, last item: entity maps living in the Global doc `holon_tree`
//! do not disturb the block projection.
//!
//! The block leg routes a `subscribe_root` fact by its owning tree node, so an
//! entity map — a root map beside the tree — must contribute no block change
//! and must not make the pass unsettled. A commit that touches a block AND an
//! entity map must still project the block, and only it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use holon_loro::CONTENT_RAW;
use holon_loro::CONTENT_TYPE;
use holon_loro::TREE_NAME;
use holon_loro::WriteOrigin;
use holon_loro::loro_backend::PendingChange;
use holon_loro::loro_backend::build_tid_index;
use holon_loro::loro_backend::extract_pending_changes;
use holon_loro::loro_backend::incremental_block_changes;
use holon_loro::loro_document::LoroDocument;
use holon_loro::write_stable_id;
use loro::LoroValue;
use loro::TreeID;
use typed_entity_shape::CANDIDATES;
use typed_entity_shape::FieldPath;
use typed_entity_shape::changed_entities;
use typed_entity_shape::put_field;
use typed_entity_shape::read_entity;

use crate::typed_entity_shape;

const ORIGIN: WriteOrigin = WriteOrigin::Probe("typed_entity_block_projection");
const TYPE: &str = "repair-record";

struct Facts {
    blocks: Arc<Mutex<Vec<PendingChange>>>,
    entities: Arc<Mutex<Vec<(String, String)>>>,
}

fn seeded() -> anyhow::Result<(Arc<LoroDocument>, Vec<TreeID>)> {
    let doc = Arc::new(LoroDocument::new("global".to_string())?);
    let nodes = doc.with_write(ORIGIN, |txn| {
        let tree = txn.get_tree(TREE_NAME);
        let page = tree.create(None)?;
        let meta = tree.get_meta(page)?;
        write_stable_id(txn, page, "page-1")?;
        meta.insert(CONTENT_TYPE, "text")?;
        meta.ensure_mergeable_text(CONTENT_RAW)?.insert(0, "Page")?;
        let mut nodes = vec![page];
        for i in 0..4 {
            let node = tree.create(page)?;
            let meta = tree.get_meta(node)?;
            write_stable_id(txn, node, format!("b{i}").as_str())?;
            meta.insert(CONTENT_TYPE, "text")?;
            meta.ensure_mergeable_text(CONTENT_RAW)?
                .insert(0, &format!("block {i}"))?;
            nodes.push(node);
        }
        Ok(nodes)
    })?;
    Ok((doc, nodes))
}

fn subscribe(doc: &LoroDocument) -> (Facts, loro::Subscription) {
    let facts = Facts {
        blocks: Arc::default(),
        entities: Arc::default(),
    };
    let blocks = facts.blocks.clone();
    let entities = facts.entities.clone();
    let sub = doc
        .with_read(|d| {
            Ok(d.subscribe_root(Arc::new(move |event| {
                blocks
                    .lock()
                    .expect("the block sink is not poisoned")
                    .extend(extract_pending_changes(&event));
                entities
                    .lock()
                    .expect("the entity sink is not poisoned")
                    .extend(changed_entities(&event));
            })))
        })
        .expect("subscribing under the read guard succeeds");
    (facts, sub)
}

fn drain<T>(sink: &Mutex<Vec<T>>) -> Vec<T> {
    std::mem::take(&mut *sink.lock().expect("the sink is not poisoned"))
}

#[test]
fn an_entity_write_contributes_no_block_change() -> anyhow::Result<()> {
    for shape in CANDIDATES {
        let (doc, nodes) = seeded()?;
        let (facts, _sub) = subscribe(&doc);
        // Loro decides whether to record events when a transaction opens, and
        // the open one predates the subscription: let it commit once.
        doc.with_write(ORIGIN, |txn| {
            txn.get_tree(TREE_NAME)
                .get_meta(nodes[1])?
                .insert("warm", "yes")?;
            Ok(())
        })?;
        drain(&facts.blocks);
        drain(&facts.entities);
        let mut tid_index: HashMap<TreeID, String> = doc.with_read(|d| Ok(build_tid_index(d)))?;

        doc.with_write(ORIGIN, |txn| {
            for i in 0..3 {
                let id = format!("rec-{i}");
                put_field(
                    txn,
                    shape,
                    TYPE,
                    &id,
                    &FieldPath::scalar("severity"),
                    LoroValue::from("high"),
                );
                put_field(
                    txn,
                    shape,
                    TYPE,
                    &id,
                    &FieldPath::property("color"),
                    LoroValue::from("red"),
                );
            }
            Ok(())
        })?;

        let pending = drain(&facts.blocks);
        assert!(
            !pending.is_empty(),
            "{}: an entity write must still raise facts",
            shape.label()
        );
        let (changed, settled) =
            doc.with_read(|d| incremental_block_changes(d, &pending, &mut tid_index))?;
        assert!(
            changed.is_empty(),
            "{}: entity facts must project no block; got {changed:?}",
            shape.label()
        );
        assert!(
            settled,
            "{}: entity facts must leave the pass settled",
            shape.label()
        );

        let entities = drain(&facts.entities);
        assert_eq!(
            entities.len(),
            3,
            "{}: the entity leg must name every written id exactly once; got {entities:?}",
            shape.label()
        );
        for i in 0..3 {
            assert!(
                entities.contains(&(TYPE.to_string(), format!("rec-{i}"))),
                "{}: rec-{i} is missing from {entities:?}",
                shape.label()
            );
        }
        assert!(
            doc.with_read(|d| Ok(read_entity(d, shape, TYPE, "rec-1").is_some()))?,
            "{}: the entity is readable from the Global doc",
            shape.label()
        );
    }
    Ok(())
}

#[test]
fn one_commit_touching_a_block_and_an_entity_projects_only_the_block() -> anyhow::Result<()> {
    for shape in CANDIDATES {
        let (doc, nodes) = seeded()?;
        let (facts, _sub) = subscribe(&doc);
        doc.with_write(ORIGIN, |txn| {
            txn.get_tree(TREE_NAME)
                .get_meta(nodes[1])?
                .insert("warm", "yes")?;
            Ok(())
        })?;
        drain(&facts.blocks);
        drain(&facts.entities);
        let mut tid_index: HashMap<TreeID, String> = doc.with_read(|d| Ok(build_tid_index(d)))?;

        doc.with_write(ORIGIN, |txn| {
            txn.get_tree(TREE_NAME)
                .get_meta(nodes[2])?
                .insert("completed", true)?;
            put_field(
                txn,
                shape,
                TYPE,
                "rec-9",
                &FieldPath::scalar("severity"),
                LoroValue::from("critical"),
            );
            Ok(())
        })?;

        let pending = drain(&facts.blocks);
        let (changed, settled) =
            doc.with_read(|d| incremental_block_changes(d, &pending, &mut tid_index))?;
        assert!(settled, "{}: the pass must settle", shape.label());
        let ids: Vec<&String> = changed.keys().collect();
        assert_eq!(
            ids.len(),
            1,
            "{}: exactly the edited block projects; got {ids:?}",
            shape.label()
        );
        assert!(
            ids[0].ends_with("b1"),
            "{}: the projected block must be b1; got {ids:?}",
            shape.label()
        );
        assert_eq!(
            drain(&facts.entities),
            vec![(TYPE.to_string(), "rec-9".to_string())],
            "{}: the entity leg names the entity of the same commit",
            shape.label()
        );
    }
    Ok(())
}
