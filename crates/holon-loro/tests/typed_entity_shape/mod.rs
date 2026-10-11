//! The candidate on-disk shapes of the Loro typed-entity store (the plan
//! `LoroTypedEntityStore.md` section 4), shared by the convergence PBT, the
//! scale bench and the read-side bench. No production wiring: these tests
//! decide the shape before any of it exists.
//!
//! Three shapes:
//! - A  `MergeableChild`: one mergeable child map per entity, at key = the id
//! - B  `FlatKeys`: flat keys `<id>/<field>` in the type root map
//! - A' `PlainContainerChild`: A built with `insert_container` instead — the
//!   negative control the convergence oracle must refute
//!
//! Every write is padded to a Lamport height no peer has used yet, so no two
//! writes to one key ever tie. That makes the last-writer-wins oracle exact
//! without re-simulating Loro's peer-id tie-break; the padding follows the
//! clock-parity precedent in `crates/holon-loro/src/multi_peer.rs`.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use holon_loro::loro_backend::doc_lamport_height;
use loro::ContainerID;
use loro::ExportMode;
use loro::Index;
use loro::LoroDoc;
use loro::LoroMap;
use loro::LoroValue;
use loro::event::Diff;
use loro::event::DiffEvent;

/// The overflow-bag key inside an entity (plan section 4).
pub const PROPERTIES: &str = "properties";
/// The soft-delete stamp field (plan section 4, question 2b).
pub const TOMBSTONE: &str = "_deleted";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// (A) one mergeable child map per entity, at key = the id.
    MergeableChild,
    /// (B) flat keys `<id>/<field>` in the type root map.
    FlatKeys,
    /// (A') A with a plain `insert_container` child: two peers that create one
    /// id concurrently get two containers and the parent key's LWW discards
    /// one of them, with every field only that peer wrote.
    PlainContainerChild,
}

impl Shape {
    pub fn label(self) -> &'static str {
        match self {
            Shape::MergeableChild => "A mergeable-child",
            Shape::FlatKeys => "B flat-keys",
            Shape::PlainContainerChild => "A' plain-container-child",
        }
    }
}

/// The two shapes the plan puts forward as candidates.
pub const CANDIDATES: [Shape; 2] = [Shape::MergeableChild, Shape::FlatKeys];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FieldPath {
    /// A persisted field declared by the `TypeDefinition`.
    Scalar(String),
    /// One key of the overflow bag.
    Property(String),
}

impl FieldPath {
    pub fn scalar(f: &str) -> Self {
        FieldPath::Scalar(f.to_string())
    }

    pub fn property(k: &str) -> Self {
        FieldPath::Property(k.to_string())
    }

    /// The shape-independent path the oracle and the readers key on.
    pub fn flat(&self) -> String {
        match self {
            FieldPath::Scalar(f) => f.clone(),
            FieldPath::Property(k) => format!("{PROPERTIES}/{k}"),
        }
    }
}

/// Loro refuses a root container name that is empty or carries `/` or `\0`
/// (`loro-common/src/lib.rs:1001`), so the plan's `entity/<type>` cannot be a
/// root name — the separator is `:`, the same one the entity URIs use.
pub fn root_name(ty: &str) -> String {
    format!("{ROOT_PREFIX}{ty}")
}

pub const ROOT_PREFIX: &str = "entity:";

pub fn root(doc: &LoroDoc, ty: &str) -> LoroMap {
    doc.get_map(root_name(ty).as_str())
}

fn flat_key(id: &str, field: &FieldPath) -> String {
    format!("{id}/{}", field.flat())
}

/// One peer of a run. The doc carries both the entity maps and the clock pad.
pub struct Peer {
    pub doc: LoroDoc,
    pub peer_id: u64,
}

impl Peer {
    pub fn new(peer_id: u64) -> Self {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer_id)
            .expect("a fresh doc accepts a peer id");
        Self { doc, peer_id }
    }
}

/// A Lamport clock shared by the peers of one run.
#[derive(Default)]
pub struct Clock(u32);

impl Clock {
    /// Pad `doc` up to a height no write has used yet, so the next op's
    /// Lamport is globally unique.
    fn reserve(&mut self, doc: &LoroDoc) {
        let target = self.0.max(doc_lamport_height(doc));
        let pad = doc.get_text("clock_pad");
        loop {
            let h = doc_lamport_height(doc);
            assert!(h <= target, "clock pad overshot: {h} > {target}");
            if h == target {
                break;
            }
            pad.insert(0, "x").expect("the pad text accepts an atom");
            doc.commit();
        }
    }

    fn settle(&mut self, height: u32) {
        self.0 = self.0.max(height);
    }
}

fn open_entity(root: &LoroMap, id: &str, shape: Shape) -> LoroMap {
    match shape {
        Shape::MergeableChild => root
            .ensure_mergeable_map(id)
            .expect("a fresh key takes a mergeable child"),
        Shape::PlainContainerChild => match root.get(id) {
            Some(loro::ValueOrContainer::Container(loro::Container::Map(m))) => m,
            None => root
                .insert_container(id, LoroMap::new())
                .expect("a fresh key takes a plain child"),
            other => panic!("entity key {id:?} holds {other:?}, not a map"),
        },
        Shape::FlatKeys => panic!("shape B has no entity container"),
    }
}

fn open_properties(entity: &LoroMap, shape: Shape) -> LoroMap {
    match shape {
        Shape::MergeableChild => entity
            .ensure_mergeable_map(PROPERTIES)
            .expect("a fresh key takes a mergeable child"),
        Shape::PlainContainerChild => match entity.get(PROPERTIES) {
            Some(loro::ValueOrContainer::Container(loro::Container::Map(m))) => m,
            None => entity
                .insert_container(PROPERTIES, LoroMap::new())
                .expect("a fresh key takes a plain child"),
            other => panic!("the overflow bag holds {other:?}, not a map"),
        },
        Shape::FlatKeys => panic!("shape B has no overflow-bag container"),
    }
}

/// Place one field in the shape, with no clock padding and no commit — the
/// seeding path of the scale benches, and the body of [`write_field`].
pub fn put_field(
    doc: &LoroDoc,
    shape: Shape,
    ty: &str,
    id: &str,
    field: &FieldPath,
    value: LoroValue,
) {
    let root = root(doc, ty);
    match shape {
        Shape::FlatKeys => root
            .insert(&flat_key(id, field), value)
            .expect("the type root map takes a scalar"),
        Shape::MergeableChild | Shape::PlainContainerChild => {
            let entity = open_entity(&root, id, shape);
            match field {
                FieldPath::Scalar(f) => entity.insert(f, value).expect("an entity takes a field"),
                FieldPath::Property(k) => open_properties(&entity, shape)
                    .insert(k, value)
                    .expect("the overflow bag takes a key"),
            }
        }
    }
}

/// Write one field and return the Lamport of the op that did it, or `None`
/// when Loro skipped the write because the key already held that value (the
/// skip is load-bearing for the oracle: a skipped write never competes).
///
/// The field insert is the LAST op of the commit — any container creation
/// precedes it — so `height - 1` is its Lamport.
pub fn write_field(
    peer: &Peer,
    clock: &mut Clock,
    shape: Shape,
    ty: &str,
    id: &str,
    field: &FieldPath,
    value: LoroValue,
) -> Option<u32> {
    clock.reserve(&peer.doc);
    let before = doc_lamport_height(&peer.doc);
    put_field(&peer.doc, shape, ty, id, field, value);
    peer.doc.commit();
    let after = doc_lamport_height(&peer.doc);
    clock.settle(after);
    if after == before {
        return None;
    }
    Some(after - 1)
}

/// Remove the entity's keys outright: the key-removal delete of question 2a,
/// and the purge leg of the tombstone rule of question 2b.
pub fn purge_entity(peer: &Peer, clock: &mut Clock, shape: Shape, ty: &str, id: &str) {
    clock.reserve(&peer.doc);
    let root = root(&peer.doc, ty);
    match shape {
        Shape::FlatKeys => {
            let prefix = format!("{id}/");
            for key in flat_keys(&root) {
                if key.starts_with(&prefix) {
                    root.delete(&key).expect("an existing key is removable");
                }
            }
        }
        Shape::MergeableChild | Shape::PlainContainerChild => {
            root.delete(id).expect("an existing key is removable");
        }
    }
    peer.doc.commit();
    clock.settle(doc_lamport_height(&peer.doc));
}

fn flat_keys(root: &LoroMap) -> Vec<String> {
    match root.get_value() {
        LoroValue::Map(m) => m.keys().cloned().collect(),
        other => panic!("a type root map read as {other:?}"),
    }
}

/// Flatten one entity's deep value into `field path -> value`, lifting the
/// overflow bag to `properties/<key>`.
fn flatten_entity(deep: &LoroValue) -> BTreeMap<String, LoroValue> {
    let map = match deep {
        LoroValue::Map(m) => m,
        other => panic!("an entity read as {other:?}, not a map"),
    };
    let mut out = BTreeMap::new();
    for (key, value) in map.iter() {
        match (key.as_str(), value) {
            (PROPERTIES, LoroValue::Map(props)) => {
                for (p, v) in props.iter() {
                    out.insert(format!("{PROPERTIES}/{p}"), v.clone());
                }
            }
            (PROPERTIES, other) => panic!("the overflow bag read as {other:?}, not a map"),
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    out
}

fn entity_map(root: &LoroMap, id: &str) -> Option<LoroMap> {
    match root.get(id) {
        Some(loro::ValueOrContainer::Container(loro::Container::Map(m))) => Some(m),
        None => None,
        other => panic!("entity key {id:?} holds {other:?}, not a map"),
    }
}

/// A point read of one entity: every field it holds, or `None` when the id has
/// no state at all.
pub fn read_entity(
    doc: &LoroDoc,
    shape: Shape,
    ty: &str,
    id: &str,
) -> Option<BTreeMap<String, LoroValue>> {
    let root = root(doc, ty);
    let fields = match shape {
        Shape::MergeableChild | Shape::PlainContainerChild => {
            flatten_entity(&entity_map(&root, id)?.get_deep_value())
        }
        Shape::FlatKeys => {
            let prefix = format!("{id}/");
            let map = match root.get_value() {
                LoroValue::Map(m) => m,
                other => panic!("a type root map read as {other:?}"),
            };
            map.iter()
                .filter_map(|(k, v)| {
                    k.strip_prefix(&prefix)
                        .map(|field| (field.to_string(), v.clone()))
                })
                .collect()
        }
    };
    if fields.is_empty() {
        None
    } else {
        Some(fields)
    }
}

/// A point read that knows the type's declared fields, so neither shape has to
/// discover them: shape A opens one child map, shape B does one key lookup per
/// field. This is the read a `TypeDefinition`-driven provider makes.
pub fn read_entity_declared(
    doc: &LoroDoc,
    shape: Shape,
    ty: &str,
    id: &str,
    fields: &[FieldPath],
) -> BTreeMap<String, LoroValue> {
    fields
        .iter()
        .filter_map(|field| {
            read_field(doc, shape, ty, id, field).map(|value| (field.flat(), value))
        })
        .collect()
}

/// One field of one entity, without materializing the rest — the read the
/// no-Turso guards make.
pub fn read_field(
    doc: &LoroDoc,
    shape: Shape,
    ty: &str,
    id: &str,
    field: &FieldPath,
) -> Option<LoroValue> {
    let root = root(doc, ty);
    match shape {
        Shape::FlatKeys => match root.get(&flat_key(id, field)) {
            Some(loro::ValueOrContainer::Value(v)) => Some(v),
            None => None,
            other => panic!("a flat field holds {other:?}, not a value"),
        },
        Shape::MergeableChild | Shape::PlainContainerChild => {
            let entity = entity_map(&root, id)?;
            let (holder, key) = match field {
                FieldPath::Scalar(f) => (entity, f.clone()),
                FieldPath::Property(k) => match entity.get(PROPERTIES) {
                    Some(loro::ValueOrContainer::Container(loro::Container::Map(m))) => {
                        (m, k.clone())
                    }
                    None => return None,
                    other => panic!("the overflow bag holds {other:?}, not a map"),
                },
            };
            match holder.get(&key) {
                Some(loro::ValueOrContainer::Value(v)) => Some(v),
                None => None,
                other => panic!("field {key:?} holds {other:?}, not a value"),
            }
        }
    }
}

/// Every entity of one type with every field it holds — the whole state the
/// convergence oracle compares against.
pub fn read_all(
    doc: &LoroDoc,
    shape: Shape,
    ty: &str,
) -> BTreeMap<String, BTreeMap<String, LoroValue>> {
    let root = root(doc, ty);
    let mut out: BTreeMap<String, BTreeMap<String, LoroValue>> = BTreeMap::new();
    match shape {
        Shape::MergeableChild | Shape::PlainContainerChild => {
            for id in flat_keys(&root) {
                let fields = flatten_entity(
                    &entity_map(&root, &id)
                        .expect("a listed key is a map")
                        .get_deep_value(),
                );
                if !fields.is_empty() {
                    out.insert(id, fields);
                }
            }
        }
        Shape::FlatKeys => {
            let map = match root.get_value() {
                LoroValue::Map(m) => m,
                other => panic!("a type root map read as {other:?}"),
            };
            for (key, value) in map.iter() {
                let (id, field) = key
                    .split_once('/')
                    .expect("a shape B key is `<id>/<field path>`");
                out.entry(id.to_string())
                    .or_default()
                    .insert(field.to_string(), value.clone());
            }
        }
    }
    out
}

/// The ids of one type, without reading their fields — a type scan.
pub fn scan_ids(doc: &LoroDoc, shape: Shape, ty: &str) -> BTreeSet<String> {
    let root = root(doc, ty);
    match shape {
        Shape::MergeableChild | Shape::PlainContainerChild => {
            flat_keys(&root).into_iter().collect()
        }
        Shape::FlatKeys => flat_keys(&root)
            .into_iter()
            .map(|k| {
                k.split_once('/')
                    .expect("a shape B key is `<id>/<field path>`")
                    .0
                    .to_string()
            })
            .collect(),
    }
}

/// The ids whose `field` equals `want` — the filter the no-Turso guards run
/// without an IVM.
pub fn filter_by_field(
    doc: &LoroDoc,
    shape: Shape,
    ty: &str,
    field: &FieldPath,
    want: &LoroValue,
) -> BTreeSet<String> {
    let root = root(doc, ty);
    match shape {
        Shape::MergeableChild | Shape::PlainContainerChild => flat_keys(&root)
            .into_iter()
            .filter(|id| read_field(doc, shape, ty, id, field).as_ref() == Some(want))
            .collect(),
        Shape::FlatKeys => {
            let suffix = format!("/{}", field.flat());
            let map = match root.get_value() {
                LoroValue::Map(m) => m,
                other => panic!("a type root map read as {other:?}"),
            };
            map.iter()
                .filter(|(k, v)| k.ends_with(&suffix) && *v == want)
                .map(|(k, _)| {
                    k.strip_suffix(&suffix)
                        .expect("the key was just matched")
                        .to_string()
                })
                .collect()
        }
    }
}

// -- sync --

pub fn sync_pair(a: &LoroDoc, b: &LoroDoc) {
    let b_vv = b.oplog_vv();
    let delta = a
        .export(ExportMode::updates(&b_vv))
        .expect("export updates");
    if !delta.is_empty() {
        b.import(&delta).expect("import updates");
    }
    let a_vv = a.oplog_vv();
    let delta = b
        .export(ExportMode::updates(&a_vv))
        .expect("export updates");
    if !delta.is_empty() {
        a.import(&delta).expect("import updates");
    }
}

/// Sync every pair until all peers hold the same oplog. Fails loud if that
/// does not close — a non-converging mesh is a harness bug, not a shape fact.
pub fn sync_to_closure(peers: &[Peer]) {
    for _ in 0..peers.len() + 1 {
        for i in 0..peers.len() {
            for j in (i + 1)..peers.len() {
                sync_pair(&peers[i].doc, &peers[j].doc);
            }
        }
        let first = peers[0].doc.oplog_vv();
        if peers.iter().all(|p| p.doc.oplog_vv() == first) {
            return;
        }
    }
    panic!("the peer mesh did not reach a common oplog");
}

// -- the projector's entity leg --

/// The `(type, id)` pairs a `subscribe_root` event names, with no doc access —
/// the entity-leg counterpart of `extract_pending_changes`.
pub fn changed_entities(event: &DiffEvent) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for cd in event.events.iter() {
        // Shape A: the changed container is the entity child map. A path
        // element is `(that item's container, its index in its parent)`, so
        // the id is the key of the element whose PREDECESSOR is the type root.
        // A mergeable child is itself a root container under a synthetic name
        // (`🤝:$<parent>><key>`), which is why the parent must come from the
        // previous element and not from the child's own id.
        for pair in cd.path.windows(2) {
            if let (ContainerID::Root { name, .. }, _) = &pair[0]
                && let (_, Index::Key(key)) = &pair[1]
                && let Some(ty) = name.to_string().strip_prefix(ROOT_PREFIX)
            {
                out.insert((ty.to_string(), key.to_string()));
            }
        }
        // Shape B, and shape A's container-pointer insert: the changed
        // container IS the type root and the ids are the keys' first segment.
        if let (ContainerID::Root { name, .. }, Diff::Map(delta)) = (cd.target, &cd.diff)
            && let Some(ty) = name.to_string().strip_prefix(ROOT_PREFIX)
        {
            for key in delta.updated.keys() {
                let id = key.split('/').next().expect("a map key is non-empty");
                out.insert((ty.to_string(), id.to_string()));
            }
        }
    }
    out
}

// -- the oracle --

/// Per-field last-writer-wins in causal order. With the Lamport padding above,
/// "later" is a total order over the writes of one run, so the winner of every
/// key is exact and Loro's peer-id tie-break is deliberately out of the model.
#[derive(Default)]
pub struct Oracle {
    winners: BTreeMap<(String, String), (u32, LoroValue)>,
}

impl Oracle {
    pub fn record(&mut self, id: &str, field: &FieldPath, value: LoroValue, lamport: u32) {
        let key = (id.to_string(), field.flat());
        if let Some((seen, _)) = self.winners.get(&key) {
            assert_ne!(
                *seen, lamport,
                "two writes to {key:?} share Lamport {lamport} — the clock padding failed"
            );
            if *seen > lamport {
                return;
            }
        }
        self.winners.insert(key, (lamport, value));
    }

    pub fn forget(&mut self, id: &str) {
        self.winners.retain(|(entity, _), _| entity != id);
    }

    pub fn expected(&self) -> BTreeMap<String, BTreeMap<String, LoroValue>> {
        let mut out: BTreeMap<String, BTreeMap<String, LoroValue>> = BTreeMap::new();
        for ((id, field), (_, value)) in self.winners.iter() {
            out.entry(id.clone())
                .or_default()
                .insert(field.clone(), value.clone());
        }
        out
    }
}

/// Compare one peer's state against the oracle, naming the first divergence as
/// the report the PBT and the negative control both print.
pub fn diverges(
    label: &str,
    actual: &BTreeMap<String, BTreeMap<String, LoroValue>>,
    expected: &BTreeMap<String, BTreeMap<String, LoroValue>>,
) -> Option<String> {
    for (id, want) in expected {
        let Some(got) = actual.get(id) else {
            return Some(format!(
                "{label}: entity {id:?} is missing; expected {want:?}"
            ));
        };
        for (field, value) in want {
            match got.get(field) {
                None => {
                    return Some(format!(
                        "{label}: entity {id:?} lost field {field:?} (expected {value:?}); holds {got:?}"
                    ));
                }
                Some(seen) if seen != value => {
                    return Some(format!(
                        "{label}: entity {id:?} field {field:?} is {seen:?}, expected {value:?}"
                    ));
                }
                Some(_) => {}
            }
        }
        for field in got.keys() {
            if !want.contains_key(field) {
                return Some(format!(
                    "{label}: entity {id:?} holds unexpected field {field:?}"
                ));
            }
        }
    }
    for id in actual.keys() {
        if !expected.contains_key(id) {
            return Some(format!(
                "{label}: entity {id:?} exists but was never written"
            ));
        }
    }
    None
}
