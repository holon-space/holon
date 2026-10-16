//! Spike #351 (concept #345): do two devices that resolve a contested slot
//! LOCALLY converge after sync?
//!
//! Two real `holon_net` executors (one per device) run the same
//! one-transition net over two real `loro` documents. The contestant set lives
//! IN the document and syncs; each device projects the set out of its OWN
//! synced document and feeds it to its OWN executor, which writes `Slot`,
//! `Displaced` and `SlotCondition`. No decision crosses the wire — only the
//! document does.
//!
//! Two resolvers behind one seam, carried in the `Claims` token because the
//! net's bodies are plain `fn` pointers:
//!
//! * [`Rule::SetFunction`] — the authority of the claim's origin, then the
//!   claim's immutable stored stamp. A pure function of the contestant SET.
//! * [`Rule::Naive`] — first arrival wins, read off the device's LOCAL arrival
//!   log. The negative control: it must fail convergence.
//!
//! The oracles never call the resolver. They compare the two devices with each
//! other, compare a state with itself across an edit-only step, and check
//! absolute invariants (every id exactly once, no edit lost, equal write-back
//! bytes).
//!
//! Spike, test only: nothing here is production code.

#![cfg(feature = "pbt")]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_net::exec::Body;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::Executor;
use holon_net::exec::NetDef;
use holon_net::exec::Output;
use holon_net::exec::Reader;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
use loro::Container;
use loro::ExportMode;
use loro::LoroDoc;
use loro::LoroList;
use loro::LoroMap;
use loro::LoroText;
use loro::LoroTree;
use loro::LoroValue;
use loro::TreeID;
use loro::TreeParentId;
use loro::ValueOrContainer;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::RngAlgorithm;
use proptest::test_runner::TestError;
use proptest::test_runner::TestRng;
use proptest::test_runner::TestRunner;

// ------------------------------------------------------------------ constants

/// holon-loro's tree name and `STABLE_ID` meta key (`loro_backend.rs:80`): the
/// block id is meta, the `TreeID` is peer-local (ADR 0003).
const TREE: &str = "blocks";
const STABLE_ID: &str = "id";

/// The two contested well-known keys a case may claim.
const KEYS: [&str; 2] = ["journals", "inbox"];
/// The origins a claim may come from. Two claims of one key from ONE origin
/// are a double ingest of one file (MERGE); from two origins they are two
/// entities that collide on one id (PICK).
const ORIGINS: [&str; 3] = ["vault/journals.org", "vault/phone/journals.org", "seed"];

/// The policy row device B has and device A has not, in the skew run.
const SKEW_ORIGIN: &str = "vault/phone/journals.org";
const SKEW_AUTHORITY: i64 = 5;

const PLACE_CLAIMS: &str = "Claims";
const PLACE_SLOT: &str = "Slot";
const PLACE_DISPLACED: &str = "Displaced";
const PLACE_CONDITION: &str = "SlotCondition";
const RESOLVE: &str = "Resolve";

/// Fixed so every log of this spike names a reproducible run.
const SEED: &[u8; 32] = b"holon-351-slot-converge-spike-01";
const CASES: u32 = 64;

// --------------------------------------------------------------- the resolver

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    /// Authority, then the claim's immutable stored stamp: a pure function of
    /// the contestant set.
    SetFunction,
    /// First arrival wins, off the device's local arrival log. Its input is not
    /// a function of the set, so no sync can make two devices agree.
    Naive,
}

impl Rule {
    fn as_str(self) -> &'static str {
        match self {
            Self::SetFunction => "set-function",
            Self::Naive => "naive",
        }
    }
}

/// One live contestant for a key, as a device reads it out of its own
/// document. `authority` is the DEVICE's policy, not document content, which is
/// what the skew run varies.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
struct Claimant {
    /// The STORED claim stamp: the id of the op that created the node, written
    /// into meta at creation, so a delete and an undo keep it.
    stamp: String,
    origin: String,
    authority: i64,
    /// A hybrid logical clock, stored with the claim. Recorded, never ranked;
    /// [`the_stored_hlc_is_not_ranked`] pins that.
    hlc: i64,
}

/// Who holds the slot, who is displaced, and whether the losers are the SAME
/// entity (merge, no loser) or different ones (pick plus a deterministic
/// re-mint).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Decision {
    winner: String,
    losers: Vec<String>,
    merge: bool,
}

/// Authority descending, then the stamp ascending. Both terms are immutable
/// under an edit, so no edit can move the slot.
fn set_function(set: &[Claimant]) -> Decision {
    let winner = set
        .iter()
        .min_by_key(|c| (-c.authority, c.stamp.clone()))
        .expect("resolve is only asked about a non-empty set");
    decide(set, &winner.stamp)
}

/// Same id AND same origin is the same entity re-ingested: the losers merge
/// into the winner and nothing is displaced. Same id alone never means "same
/// entity".
fn decide(set: &[Claimant], winner: &str) -> Decision {
    let origins: BTreeSet<&str> = set.iter().map(|c| &*c.origin).collect();
    Decision {
        winner: winner.to_string(),
        losers: set
            .iter()
            .map(|c| c.stamp.clone())
            .filter(|s| s != winner)
            .collect(),
        merge: origins.len() == 1 && set.len() > 1,
    }
}

/// FNV-1a keeps the spike free of a hash dependency.
fn fnv(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in parts.join("\0").as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The id a PICK loser takes: a function of the old id and the loser's stamp
/// ONLY, so two devices mint the same new id and no edit can change it.
fn re_mint(key: &str, stamp: &str) -> String {
    format!("{key}-displaced-{}", fnv(&[key, stamp]))
}

// --------------------------------------------------------------------- the net

fn text(s: &str) -> Value {
    Value::String(s.to_string())
}

fn content(pairs: &[(&str, Value)]) -> Content {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

fn string_of(c: &Content, field: &str) -> String {
    match c.get(field) {
        Some(Value::String(s)) => s.clone(),
        other => panic!("the field '{field}' is {other:?}, not a string"),
    }
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("the spike's own values serialize")
}

/// ONE `Resolve<K>` transition per key. It consumes the whole contestant set in
/// one firing — so `Claims` is empty at quiescence — and writes the slot, the
/// displaced references and the condition in one commit.
fn net() -> NetDef {
    let resolve = TransitionDef::new(
        RESOLVE,
        Trigger::Auto,
        Arc::new(|r: &mut Reader<'_>, s: &Subject| {
            let key = s.as_str();
            Some(content(&[
                ("set", text(&r.text(PLACE_CLAIMS, key, "set")?)),
                ("arrivals", text(&r.text(PLACE_CLAIMS, key, "arrivals")?)),
                ("rule", text(&r.text(PLACE_CLAIMS, key, "rule")?)),
            ]))
        }),
        Body::pure(|s, input| {
            let canon = string_of(input, "set");
            let set: Vec<Claimant> =
                serde_json::from_str(&canon).expect("the device wrote a contestant set");
            let decision = if string_of(input, "rule") == Rule::Naive.as_str() {
                // First arrival wins: the winner comes from the LOCAL arrival
                // log, which no sync makes equal.
                let arrivals: Vec<String> = serde_json::from_str(&string_of(input, "arrivals"))
                    .expect("the device wrote an arrival log");
                let live: BTreeSet<&str> = set.iter().map(|c| &*c.stamp).collect();
                let first = arrivals
                    .iter()
                    .find(|a| live.contains(a.as_str()))
                    .expect("every live contestant was observed once");
                decide(&set, first)
            } else {
                set_function(&set)
            };
            let hash = fnv(&[&canon]);
            let key = s.as_str();
            let condition = if decision.losers.is_empty() {
                String::new()
            } else if decision.merge {
                format!(
                    "{key}: {} contestants of one origin merged into {}",
                    set.len(),
                    decision.winner
                )
            } else {
                format!(
                    "{key}: held by {}, displaced {}",
                    decision.winner,
                    decision.losers.join(", ")
                )
            };
            Ok(vec![
                Output::new(
                    PLACE_SLOT,
                    s,
                    content(&[
                        ("winner", text(&decision.winner)),
                        ("merge", Value::Boolean(decision.merge)),
                        ("set_hash", text(&hash)),
                    ]),
                ),
                Output::new(
                    PLACE_DISPLACED,
                    s,
                    content(&[
                        ("refs", text(&json(&decision.losers))),
                        ("set_hash", text(&hash)),
                    ]),
                ),
                Output::new(
                    PLACE_CONDITION,
                    s,
                    content(&[("text", text(&condition)), ("set_hash", text(&hash))]),
                ),
            ])
        }),
    )
    .consumes(PLACE_CLAIMS);

    NetDef::default()
        .place(PLACE_CLAIMS, Class::Process)
        .place(PLACE_SLOT, Class::Process)
        .place(PLACE_DISPLACED, Class::Process)
        .place(PLACE_CONDITION, Class::Process)
        .transition(resolve)
}

// ------------------------------------------------------------------ the device

struct Device {
    name: &'static str,
    rule: Rule,
    doc: LoroDoc,
    exec: Executor,
    /// Origin to authority. Device-local POLICY, never document content.
    policy: BTreeMap<String, i64>,
    /// The order in which this device first OBSERVED each stamp. The naive
    /// rule's only input; the set function never reads it.
    arrivals: Vec<String>,
    /// The set canon this device last resolved, per key: `Resolve` fires on a
    /// change of the contestant set, never on an edit of a contestant.
    resolved: BTreeMap<String, String>,
    /// Keys this device applied a MERGE decision for. Harness bookkeeping, and
    /// the only place it survives: a merge shrinks the contestant set, which
    /// re-fires `Resolve` on the single survivor, and that firing overwrites
    /// `Slot` with `merge = false` and clears the condition. See the report's
    /// finding on the net holding only the CURRENT set.
    merges: BTreeSet<String>,
}

impl Device {
    fn new(name: &'static str, peer: u64, rule: Rule, policy: BTreeMap<String, i64>) -> Self {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer).expect("a fresh doc takes a peer id");
        doc.get_tree(TREE).enable_fractional_index(0);
        Self {
            name,
            rule,
            doc,
            exec: Executor::new(net()).expect("the slot net loads"),
            policy,
            arrivals: Vec::new(),
            resolved: BTreeMap::new(),
            merges: BTreeSet::new(),
        }
    }

    fn tree(&self) -> LoroTree {
        self.doc.get_tree(TREE)
    }

    /// Every live node carrying `STABLE_ID == key`, in `TreeID` order.
    fn carriers(&self, key: &str) -> Vec<TreeID> {
        let tree = self.tree();
        let mut found: Vec<TreeID> = live_nodes(&tree)
            .into_iter()
            .filter(|n| meta_str(&tree, *n, STABLE_ID).as_deref() == Some(key))
            .collect();
        found.sort();
        found
    }

    /// The contestant set for `key`, projected out of THIS device's document
    /// and ranked by THIS device's policy.
    fn contestants(&self, key: &str) -> Vec<Claimant> {
        let tree = self.tree();
        let mut set: Vec<Claimant> = self
            .carriers(key)
            .into_iter()
            .map(|n| {
                let origin = meta_str(&tree, n, "origin").expect("a claim stores its origin");
                Claimant {
                    stamp: meta_str(&tree, n, "stamp").expect("a claim stores its stamp"),
                    authority: self.policy.get(&origin).copied().unwrap_or(0),
                    origin,
                    hlc: meta_int(&tree, n, "hlc").expect("a claim stores its hlc"),
                }
            })
            .collect();
        set.sort();
        set
    }

    /// Records every stamp this device can now see that it had not seen before.
    /// Bookkeeping for the naive rule's arrival order, nothing else.
    fn observe(&mut self) {
        let tree = self.tree();
        let mut fresh: Vec<String> = live_nodes(&tree)
            .into_iter()
            .filter_map(|n| meta_str(&tree, n, "stamp"))
            .filter(|s| !self.arrivals.contains(s))
            .collect();
        fresh.sort();
        self.arrivals.extend(fresh);
    }

    /// Feeds the contestant set of every key whose set CHANGED, lets the net
    /// quiesce and applies the decision to the document. Repeats until no set
    /// changes any more, so the whole device sits at a fixed point.
    fn resolve_to_fixpoint(&mut self) {
        for _ in 0..12 {
            self.observe();
            let mut fired = false;
            for key in keys_in_play(&self.doc) {
                let set = self.contestants(&key);
                let canon = json(&set);
                if set.is_empty() || self.resolved.get(&key) == Some(&canon) {
                    continue;
                }
                // `write` runs the auto transitions to quiescence itself, so
                // `Resolve` has fired by the time it returns.
                let quiesced = self
                    .exec
                    .write(
                        PLACE_CLAIMS,
                        &key,
                        content(&[
                            ("set", text(&canon)),
                            ("arrivals", text(&json(&self.arrivals))),
                            ("rule", text(self.rule.as_str())),
                        ]),
                        Duration::ZERO,
                    )
                    .expect("the slot net quiesces");
                assert!(
                    quiesced.requests.is_empty(),
                    "{}: Resolve is pure, so it must ask for no effect: {:?}",
                    self.name,
                    quiesced.requests
                );
                self.resolved.insert(key.clone(), canon);
                self.apply(&key, &set);
                fired = true;
            }
            if !fired {
                return;
            }
        }
        panic!("{}: the slot net did not reach a fixed point", self.name);
    }

    /// The effect leg: the net decided, the device writes the decision into its
    /// own document. Both devices run this code on the same decision, so the
    /// writes are identical and Loro merges them into one state.
    fn apply(&mut self, key: &str, set: &[Claimant]) {
        let token = self
            .exec
            .tokens()
            .token(PLACE_SLOT, key)
            .expect("Resolve wrote the slot")
            .content
            .clone();
        let winner_stamp = string_of(&token, "winner");
        let merge = token.get("merge") == Some(&Value::Boolean(true));
        let tree = self.tree();
        let by_stamp: BTreeMap<String, TreeID> = self
            .carriers(key)
            .into_iter()
            .map(|n| {
                (
                    meta_str(&tree, n, "stamp").expect("a claim stores its stamp"),
                    n,
                )
            })
            .collect();
        let winner = *by_stamp
            .get(&winner_stamp)
            .expect("the winner is a live carrier");

        if merge {
            self.merges.insert(key.to_string());
        }
        for loser in set.iter().map(|c| &c.stamp).filter(|s| **s != winner_stamp) {
            let node = *by_stamp.get(loser).expect("the loser is a live carrier");
            if merge {
                self.merge_into(winner, node);
            } else {
                // PICK: the loser keeps ALL its content and takes a
                // deterministically re-minted id, so it stops being a
                // contestant without either device having to agree on more.
                let meta = tree.get_meta(node).expect("a live node has meta");
                meta.insert(STABLE_ID, re_mint(key, loser))
                    .expect("meta takes the re-minted id");
                meta.insert("displaced_from", key)
                    .expect("meta takes the back reference");
            }
        }
        self.doc.commit();
    }

    /// MERGE of one entity ingested twice: children by MOVE (never a copy),
    /// tags by union, the log by append, plain fields last-write-wins, task
    /// state by a three-way check, and the secondary's text kept as a retained
    /// version. Then the emptied secondary goes.
    fn merge_into(&mut self, winner: TreeID, loser: TreeID) {
        let tree = self.tree();
        for kid in live_children(&tree, loser) {
            let at = live_children(&tree, winner).len();
            tree.mov_to(kid, TreeParentId::Node(winner), at)
                .expect("the child moves under the winner");
        }
        let (w, l) = (
            tree.get_meta(winner).expect("the winner has meta"),
            tree.get_meta(loser).expect("the loser has meta"),
        );
        for tag in map_keys(&l, "tags") {
            set_insert(&w, "tags", &tag);
        }
        for entry in list_items(&l, "logbook") {
            list_append_once(&w, "logbook", &entry);
        }
        if let Some(title) = map_str(&l, "title") {
            // A plain field, last write wins: the merge IS a write, and it is
            // the same write on both devices.
            w.insert("title", title).expect("meta takes the title");
        }
        // Task state: a three-way check against the last common value. When
        // both sides moved it away from the base, the net discloses that
        // instead of picking one.
        let base = map_str(&w, "state_base");
        let ours = map_str(&w, "state");
        if let Some(theirs) = map_str(&l, "state")
            && ours.as_deref() != Some(&theirs)
        {
            if base.is_some() && ours != base && Some(theirs.clone()) != base {
                w.insert("state_conflict", theirs)
                    .expect("meta takes the state conflict");
            } else if ours == base {
                w.insert("state", theirs).expect("meta takes the state");
            }
        }
        // Text: the winner's text stays. The secondary's, when different,
        // becomes a retained version — two independently created texts share no
        // history, so the text CRDT must not interleave them.
        let (wt, lt) = (text_of(&w, "text"), text_of(&l, "text"));
        if !lt.is_empty() && lt != wt {
            w.insert("retained_text", lt)
                .expect("meta takes the retained text");
        }
        tree.delete(loser).expect("the emptied loser goes");
    }

    /// A byte-deterministic rendering of the whole tree: the write-back the
    /// vault sees. Two devices at quiescence must produce the SAME bytes.
    fn write_back(&self) -> String {
        let tree = self.tree();
        let mut out = String::new();
        for root in live_children_of(&tree, TreeParentId::Root) {
            render(&tree, root, 1, &mut out);
        }
        out
    }

    /// Every live node's `STABLE_ID`, with how often it occurs.
    fn live_ids(&self) -> BTreeMap<String, usize> {
        let tree = self.tree();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for n in live_nodes(&tree) {
            if let Some(id) = meta_str(&tree, n, STABLE_ID) {
                *counts.entry(id).or_default() += 1;
            }
        }
        counts
    }

    fn token(&self, place: &str, key: &str) -> Option<Content> {
        self.exec
            .tokens()
            .token(place, key)
            .map(|t| t.content.clone())
    }
}

fn render(tree: &LoroTree, node: TreeID, depth: usize, out: &mut String) {
    let meta = tree.get_meta(node).expect("a live node has meta");
    let mut fields: Vec<(String, String)> = Vec::new();
    for key in meta.keys().map(|k| k.to_string()) {
        let rendered = match meta.get(&key) {
            Some(ValueOrContainer::Value(LoroValue::String(s))) => s.to_string(),
            Some(ValueOrContainer::Value(LoroValue::I64(i))) => i.to_string(),
            Some(ValueOrContainer::Value(LoroValue::Bool(b))) => b.to_string(),
            Some(ValueOrContainer::Container(Container::Text(t))) => t.to_string(),
            Some(ValueOrContainer::Container(Container::Map(m))) => {
                let mut ks: Vec<String> = m.keys().map(|k| k.to_string()).collect();
                ks.sort();
                ks.join(",")
            }
            Some(ValueOrContainer::Container(Container::List(l))) => {
                let mut items = list_strings(&l);
                items.sort();
                items.join(",")
            }
            _ => continue,
        };
        fields.push((key, rendered));
    }
    fields.sort();
    out.push_str(&"*".repeat(depth));
    for (k, v) in fields {
        out.push_str(&format!(" {k}={v}"));
    }
    out.push('\n');
    for kid in live_children(tree, node) {
        render(tree, kid, depth + 1, out);
    }
}

// ------------------------------------------------------------- loro accessors

fn live_nodes(tree: &LoroTree) -> Vec<TreeID> {
    tree.nodes()
        .into_iter()
        .filter(|n| !tree.is_node_deleted(n).unwrap_or(true))
        .collect()
}

fn live_children(tree: &LoroTree, parent: TreeID) -> Vec<TreeID> {
    live_children_of(tree, TreeParentId::Node(parent))
}

fn live_children_of(tree: &LoroTree, parent: TreeParentId) -> Vec<TreeID> {
    tree.children(parent)
        .unwrap_or_default()
        .into_iter()
        .filter(|n| !tree.is_node_deleted(n).unwrap_or(true))
        .collect()
}

fn meta_str(tree: &LoroTree, node: TreeID, field: &str) -> Option<String> {
    map_str(&tree.get_meta(node).ok()?, field)
}

fn meta_int(tree: &LoroTree, node: TreeID, field: &str) -> Option<i64> {
    match tree.get_meta(node).ok()?.get(field) {
        Some(ValueOrContainer::Value(LoroValue::I64(i))) => Some(i),
        _ => None,
    }
}

fn map_str(meta: &LoroMap, field: &str) -> Option<String> {
    match meta.get(field) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    }
}

fn sub_map(meta: &LoroMap, field: &str) -> LoroMap {
    match meta.get(field) {
        Some(ValueOrContainer::Container(Container::Map(m))) => m,
        _ => meta
            .insert_container(field, LoroMap::new())
            .expect("meta takes a map container"),
    }
}

fn sub_list(meta: &LoroMap, field: &str) -> LoroList {
    match meta.get(field) {
        Some(ValueOrContainer::Container(Container::List(l))) => l,
        _ => meta
            .insert_container(field, LoroList::new())
            .expect("meta takes a list container"),
    }
}

fn sub_text(meta: &LoroMap, field: &str) -> LoroText {
    match meta.get(field) {
        Some(ValueOrContainer::Container(Container::Text(t))) => t,
        _ => meta
            .insert_container(field, LoroText::new())
            .expect("meta takes a text container"),
    }
}

fn map_keys(meta: &LoroMap, field: &str) -> Vec<String> {
    match meta.get(field) {
        Some(ValueOrContainer::Container(Container::Map(m))) => {
            let mut ks: Vec<String> = m.keys().map(|k| k.to_string()).collect();
            ks.sort();
            ks
        }
        _ => Vec::new(),
    }
}

fn list_strings(list: &LoroList) -> Vec<String> {
    (0..list.len())
        .filter_map(|i| match list.get(i) {
            Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
            _ => None,
        })
        .collect()
}

fn list_items(meta: &LoroMap, field: &str) -> Vec<String> {
    match meta.get(field) {
        Some(ValueOrContainer::Container(Container::List(l))) => list_strings(&l),
        _ => Vec::new(),
    }
}

fn text_of(meta: &LoroMap, field: &str) -> String {
    match meta.get(field) {
        Some(ValueOrContainer::Container(Container::Text(t))) => t.to_string(),
        _ => String::new(),
    }
}

/// A set as a map: concurrent inserts of one tag converge and a union is
/// idempotent.
fn set_insert(meta: &LoroMap, field: &str, value: &str) {
    sub_map(meta, field)
        .insert(value, true)
        .expect("the set takes the tag");
}

/// An append-only log. The merge re-appends the secondary's entries, so an
/// append must not duplicate one that is already there.
fn list_append_once(meta: &LoroMap, field: &str, value: &str) {
    let list = sub_list(meta, field);
    if list_strings(&list).iter().any(|e| e == value) {
        return;
    }
    list.push(value.to_string())
        .expect("the log takes the entry");
}

/// Every key some live node claims.
fn keys_in_play(doc: &LoroDoc) -> Vec<String> {
    let tree = doc.get_tree(TREE);
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for n in live_nodes(&tree) {
        if let Some(id) = meta_str(&tree, n, STABLE_ID)
            && KEYS.contains(&&*id)
        {
            keys.insert(id);
        }
    }
    keys.into_iter().collect()
}

// ------------------------------------------------------------------- the world

/// What the test itself authored, so "no edit lost" is checked against the
/// harness and never against the resolver.
#[derive(Default)]
struct Ledger {
    /// Child `STABLE_ID`s created and not explicitly withdrawn.
    children: BTreeSet<String>,
    /// Tags by the STAMP of the carrier they were written on. Keyed by the
    /// stamp and not by the id, because a PICK re-mints the id while the stamp
    /// is immutable; a withdraw of that carrier drops its row, so an explicit
    /// delete is not a lost edit, while a MERGE keeps it — the union must carry
    /// the loser's tags over to the winner.
    tags: BTreeMap<String, BTreeSet<String>>,
    logs: BTreeMap<String, BTreeSet<String>>,
}

impl Ledger {
    fn tags(&self) -> BTreeSet<&str> {
        self.tags.values().flatten().map(String::as_str).collect()
    }

    fn logs(&self) -> BTreeSet<&str> {
        self.logs.values().flatten().map(String::as_str).collect()
    }
}

struct World {
    a: Device,
    b: Device,
    ledger: Ledger,
    /// A counter for the ids the test mints, shared so the two devices never
    /// collide by accident.
    seq: u32,
    /// The slot winner at each quiescence, per key: the no-oscillation oracle.
    history: BTreeMap<String, Vec<String>>,
}

impl World {
    fn new(rule: Rule, skew: bool) -> Self {
        let plain = BTreeMap::from([("seed".to_string(), 1_i64)]);
        let mut skewed = plain.clone();
        if skew {
            skewed.insert(SKEW_ORIGIN.to_string(), SKEW_AUTHORITY);
        }
        Self {
            a: Device::new("A", 1, rule, plain),
            b: Device::new("B", 2, rule, skewed),
            ledger: Ledger::default(),
            seq: 0,
            history: BTreeMap::new(),
        }
    }

    fn device(&mut self, which: bool) -> &mut Device {
        if which { &mut self.a } else { &mut self.b }
    }

    fn next(&mut self) -> u32 {
        self.seq += 1;
        self.seq
    }

    /// One sync round in both directions. An export against the peer's version
    /// vector carries exactly what that peer is missing.
    fn sync(&mut self) {
        let to_b = self
            .a
            .doc
            .export(ExportMode::updates(&self.b.doc.oplog_vv()))
            .expect("the export succeeds");
        let to_a = self
            .b
            .doc
            .export(ExportMode::updates(&self.a.doc.oplog_vv()))
            .expect("the export succeeds");
        self.b.doc.import(&to_b).expect("the import succeeds");
        self.a.doc.import(&to_a).expect("the import succeeds");
    }

    /// Syncs, resolves both devices to a fixed point and syncs again, until the
    /// two documents stop moving. The decision writes are themselves document
    /// operations, so this has to alternate.
    fn quiesce(&mut self) {
        for _ in 0..12 {
            self.sync();
            let before = (self.a.write_back(), self.b.write_back());
            self.a.resolve_to_fixpoint();
            self.b.resolve_to_fixpoint();
            self.sync();
            if (self.a.write_back(), self.b.write_back()) == before {
                self.record_history();
                return;
            }
        }
        panic!("the two devices did not reach a fixed point");
    }

    fn record_history(&mut self) {
        for key in contested_keys(self) {
            if let Some(slot) = self.a.token(PLACE_SLOT, &key) {
                self.history
                    .entry(key)
                    .or_default()
                    .push(string_of(&slot, "winner"));
            }
        }
    }
}

/// Every key either device ever resolved, plus every key still in play. Taken
/// from the net's OUTPUT place, not from the resolver — a rule that displaces
/// every carrier of a key must still have its slot compared.
fn contested_keys(world: &World) -> Vec<String> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for dev in [&world.a, &world.b] {
        keys.extend(keys_in_play(&dev.doc));
        keys.extend(
            dev.exec
                .tokens()
                .subjects(PLACE_SLOT)
                .iter()
                .map(|s| s.as_str().to_string()),
        );
    }
    keys.into_iter().collect()
}

// ------------------------------------------------------------------- the steps

#[derive(Clone, Debug)]
enum Step {
    /// A device claims `key` from `origin` with an hlc of `hlc`.
    Claim {
        device: bool,
        key: usize,
        origin: usize,
        hlc: i64,
    },
    /// A device deletes its `nth` carrier of `key`.
    Withdraw {
        device: bool,
        key: usize,
        nth: usize,
    },
    /// A device adds a child under its `nth` carrier of `key`.
    AddChild {
        device: bool,
        key: usize,
        nth: usize,
    },
    /// An EDIT of a contestant: it must never move the slot.
    Edit {
        device: bool,
        key: usize,
        nth: usize,
        what: EditKind,
    },
    Sync,
    Resolve {
        device: bool,
    },
}

#[derive(Clone, Copy, Debug)]
enum EditKind {
    Title,
    Tag,
    Log,
    Text,
    State,
}

fn steps() -> impl Strategy<Value = Vec<Step>> {
    let step = prop_oneof![
        4 => (any::<bool>(), 0..KEYS.len(), 0..ORIGINS.len(), 0_i64..1000)
            .prop_map(|(device, key, origin, hlc)| Step::Claim { device, key, origin, hlc }),
        1 => (any::<bool>(), 0..KEYS.len(), 0_usize..3)
            .prop_map(|(device, key, nth)| Step::Withdraw { device, key, nth }),
        3 => (any::<bool>(), 0..KEYS.len(), 0_usize..3)
            .prop_map(|(device, key, nth)| Step::AddChild { device, key, nth }),
        4 => (any::<bool>(), 0..KEYS.len(), 0_usize..3, prop_oneof![
            Just(EditKind::Title),
            Just(EditKind::Tag),
            Just(EditKind::Log),
            Just(EditKind::Text),
            Just(EditKind::State),
        ])
            .prop_map(|(device, key, nth, what)| Step::Edit { device, key, nth, what }),
        3 => Just(Step::Sync),
        3 => any::<bool>().prop_map(|device| Step::Resolve { device }),
    ];
    prop::collection::vec(step, 1..14)
}

impl World {
    fn run(&mut self, steps: &[Step]) {
        for step in steps {
            match *step {
                Step::Claim {
                    device,
                    key,
                    origin,
                    hlc,
                } => self.claim(device, KEYS[key], ORIGINS[origin], hlc),
                Step::Withdraw { device, key, nth } => self.withdraw(device, KEYS[key], nth),
                Step::AddChild { device, key, nth } => self.add_child(device, KEYS[key], nth),
                Step::Edit {
                    device,
                    key,
                    nth,
                    what,
                } => self.edit(device, KEYS[key], nth, what),
                Step::Sync => self.sync(),
                Step::Resolve { device } => self.device(device).resolve_to_fixpoint(),
            }
        }
    }

    fn claim(&mut self, which: bool, key: &str, origin: &str, hlc: i64) {
        let dev = self.device(which);
        let tree = dev.tree();
        let node = tree
            .create(TreeParentId::Root)
            .expect("the root accepts a child");
        let meta = tree.get_meta(node).expect("a fresh node has meta");
        meta.insert(STABLE_ID, key).expect("meta takes the id");
        // The STORED claim stamp: the id of the creating op, written at
        // creation. It is stored and never re-read off the node, so a delete
        // and an undo keep it (step 0 (c)).
        meta.insert("stamp", format!("{:020}:{:010}", node.peer, node.counter))
            .expect("meta takes the stamp");
        meta.insert("origin", origin)
            .expect("meta takes the origin");
        meta.insert("hlc", hlc).expect("meta takes the hlc");
        meta.insert("title", format!("title-{origin}"))
            .expect("meta takes the title");
        meta.insert("state", "TODO").expect("meta takes the state");
        meta.insert("state_base", "TODO")
            .expect("meta takes the state base");
        sub_text(&meta, "text")
            .insert(0, &format!("text from {origin}"))
            .expect("the text takes its seed");
        // Created by the claiming op, never lazily on first use. A container
        // reached by get-or-create is a write of the map KEY, so two devices
        // writing their first tag would meet under last-write-wins and one
        // side's entries would be dropped whole — pinned by step 0 (e) in
        // crates/holon-loro/tests/slot_loro_mechanics.rs.
        sub_map(&meta, "tags");
        sub_list(&meta, "logbook");
        dev.doc.commit();
    }

    fn nth_carrier(&mut self, which: bool, key: &str, nth: usize) -> Option<TreeID> {
        let dev = self.device(which);
        let carriers = dev.carriers(key);
        if carriers.is_empty() {
            return None;
        }
        Some(carriers[nth % carriers.len()])
    }

    fn withdraw(&mut self, which: bool, key: &str, nth: usize) {
        let Some(node) = self.nth_carrier(which, key, nth) else {
            return;
        };
        // A withdraw deletes the subtree, so what it took leaves the ledger: an
        // explicit delete is not a lost edit.
        let gone = self.subtree_ids(which, node);
        let dev = self.device(which);
        let stamp = meta_str(&dev.tree(), node, "stamp").expect("a carrier stores its stamp");
        dev.tree().delete(node).expect("the delete lands");
        dev.doc.commit();
        for id in gone {
            self.ledger.children.remove(&id);
        }
        self.ledger.tags.remove(&stamp);
        self.ledger.logs.remove(&stamp);
    }

    fn subtree_ids(&mut self, which: bool, node: TreeID) -> Vec<String> {
        let dev = self.device(which);
        let tree = dev.tree();
        let mut out = Vec::new();
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            if let Some(id) = meta_str(&tree, n, STABLE_ID) {
                out.push(id);
            }
            stack.extend(live_children(&tree, n));
        }
        out
    }

    fn add_child(&mut self, which: bool, key: &str, nth: usize) {
        let Some(parent) = self.nth_carrier(which, key, nth) else {
            return;
        };
        let id = format!("kid-{}", self.next());
        let dev = self.device(which);
        let tree = dev.tree();
        let node = tree
            .create(TreeParentId::Node(parent))
            .expect("the parent accepts a child");
        tree.get_meta(node)
            .expect("a fresh node has meta")
            .insert(STABLE_ID, id.clone())
            .expect("meta takes the id");
        dev.doc.commit();
        self.ledger.children.insert(id);
    }

    fn edit(&mut self, which: bool, key: &str, nth: usize, what: EditKind) {
        let Some(node) = self.nth_carrier(which, key, nth) else {
            return;
        };
        let n = self.next();
        let dev = self.device(which);
        let tree = dev.tree();
        let stamp = meta_str(&tree, node, "stamp").expect("a carrier stores its stamp");
        let meta = tree.get_meta(node).expect("a live node has meta");
        let mut tag = None;
        let mut log = None;
        match what {
            EditKind::Title => meta
                .insert("title", format!("title-{n}"))
                .expect("meta takes the title"),
            EditKind::Tag => {
                let t = format!("tag-{n}");
                set_insert(&meta, "tags", &t);
                tag = Some(t);
            }
            EditKind::Log => {
                let e = format!("log-{n}");
                list_append_once(&meta, "logbook", &e);
                log = Some(e);
            }
            EditKind::Text => sub_text(&meta, "text")
                .insert(0, &format!("[{n}]"))
                .expect("the text takes the insert"),
            EditKind::State => meta
                .insert("state", if n.is_multiple_of(2) { "DONE" } else { "DOING" })
                .expect("meta takes the state"),
        }
        dev.doc.commit();
        if let Some(t) = tag {
            self.ledger.tags.entry(stamp.clone()).or_default().insert(t);
        }
        if let Some(e) = log {
            self.ledger.logs.entry(stamp).or_default().insert(e);
        }
    }
}

// ----------------------------------------------------------------- the oracles

/// Everything that must hold once both devices sit at a fixed point, with the
/// resolver never consulted.
fn assert_converged(world: &World) -> Result<(), TestCaseError> {
    for key in contested_keys(world) {
        prop_assert_eq!(
            world.a.token(PLACE_SLOT, &key),
            world.b.token(PLACE_SLOT, &key),
            "the slot of '{}' differs between the devices",
            key
        );
        prop_assert_eq!(
            world.a.token(PLACE_DISPLACED, &key),
            world.b.token(PLACE_DISPLACED, &key),
            "the displaced set of '{}' differs between the devices",
            key
        );
        prop_assert_eq!(
            world.a.token(PLACE_CONDITION, &key),
            world.b.token(PLACE_CONDITION, &key),
            "the condition of '{}' differs between the devices",
            key
        );
        for dev in [&world.a, &world.b] {
            // `Resolve` consumes the whole set in one firing.
            prop_assert!(
                dev.token(PLACE_CLAIMS, &key).is_none(),
                "{}: Claims of '{}' is not empty at quiescence",
                dev.name,
                key
            );
            // One condition per (key, contestant set): the condition each
            // device holds names the set it resolved.
            let slot = dev
                .token(PLACE_SLOT, &key)
                .expect("a contested key has a slot");
            let cond = dev
                .token(PLACE_CONDITION, &key)
                .expect("a contested key has a condition");
            prop_assert_eq!(
                string_of(&slot, "set_hash"),
                string_of(&cond, "set_hash"),
                "{}: the condition of '{}' names another contestant set than the slot",
                dev.name,
                key
            );
        }
    }
    assert_shared_invariants(world)
}

/// The invariants that hold whatever the resolver decides, policy skew
/// included.
fn assert_shared_invariants(world: &World) -> Result<(), TestCaseError> {
    for dev in [&world.a, &world.b] {
        let live = dev.live_ids();
        // Every id exactly once: a merge MOVES children, so no device may end
        // with two copies of one.
        for (id, n) in &live {
            prop_assert_eq!(
                *n,
                1,
                "{}: '{}' is live {} times, not once",
                dev.name,
                id,
                n
            );
        }
        // No edit lost.
        for id in &world.ledger.children {
            prop_assert!(
                live.contains_key(id),
                "{}: the child '{}' was lost",
                dev.name,
                id
            );
        }
        let back = dev.write_back();
        for tag in world.ledger.tags() {
            prop_assert!(
                back.contains(tag),
                "{}: the tag '{}' was lost",
                dev.name,
                tag
            );
        }
        for entry in world.ledger.logs() {
            prop_assert!(
                back.contains(entry),
                "{}: the log entry '{}' was lost",
                dev.name,
                entry
            );
        }
    }
    // Equal write-back bytes: the vault is often synced outside Loro, so two
    // devices must write the same file.
    prop_assert_eq!(
        world.a.write_back(),
        world.b.write_back(),
        "the devices would write different bytes"
    );
    // No swapping back and forth: once the contestant set stops changing, the
    // winner stops changing too.
    for (key, seen) in &world.history {
        if let [.., second_last, last] = &seen[..] {
            prop_assert_eq!(
                second_last,
                last,
                "the slot of '{}' kept swapping: {:?}",
                key,
                seen
            );
        }
    }
    Ok(())
}

/// An edit of a contestant must not move the slot: both terms of the preference
/// are immutable under an edit, and `Resolve` fires on a change of the
/// contestant SET only.
fn assert_edit_only_is_stable(world: &mut World) -> Result<(), TestCaseError> {
    let keys = contested_keys(world);
    let before: BTreeMap<String, Option<Content>> = keys
        .iter()
        .map(|k| (k.clone(), world.a.token(PLACE_SLOT, k)))
        .collect();
    let versions: BTreeMap<String, u64> = keys
        .iter()
        .map(|k| (k.clone(), world.a.exec.tokens().version(PLACE_SLOT, k)))
        .collect();
    for key in &keys {
        for what in [
            EditKind::Title,
            EditKind::Tag,
            EditKind::Log,
            EditKind::Text,
        ] {
            world.edit(true, key, 0, what);
            world.edit(false, key, 0, what);
        }
    }
    world.quiesce();
    for key in &keys {
        let after = world.a.token(PLACE_SLOT, key);
        prop_assert_eq!(
            after.as_ref(),
            before[key].as_ref(),
            "an edit-only step moved the slot of '{}'",
            key
        );
        prop_assert_eq!(
            world.a.exec.tokens().version(PLACE_SLOT, key),
            versions[key],
            "an edit-only step re-fired Resolve for '{}'",
            key
        );
    }
    assert_shared_invariants(world)
}

// -------------------------------------------------------------------- the runs

/// Run 1 of the card: equal policies on both devices.
fn converges(steps: &[Step], rule: Rule) -> Result<(), TestCaseError> {
    let mut world = World::new(rule, false);
    world.run(steps);
    world.quiesce();
    assert_converged(&world)?;
    assert_edit_only_is_stable(&mut world)
}

/// Every property runs from the SAME recorded seed and case count, so each log
/// names a reproducible run.
fn run_property(
    name: &str,
    body: impl Fn(Vec<Step>) -> Result<(), TestCaseError>,
) -> Result<(), TestError<Vec<Step>>> {
    println!(
        "{name}: cases={CASES} rng=ChaCha seed={}",
        String::from_utf8_lossy(SEED)
    );
    let config = Config {
        cases: CASES,
        max_shrink_iters: 4096,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner =
        TestRunner::new_with_rng(config, TestRng::from_seed(RngAlgorithm::ChaCha, SEED));
    runner.run(&steps(), body)
}

/// The property the spike exists for: the set-function rule converges. It does
/// NOT hold yet, so this is a KNOWN RED that checks itself — registered as
/// `slot-withdrawn-last-claim-no-retraction` in
/// `docs/Testing/KeystoneKnownReds.md`.
///
/// The gap is the empty contestant set: `Resolve<K>` never fires on it, so a
/// device that resolved a key and then lost its last carrier keeps a slot no
/// device can retract, while a device that saw the carrier already deleted
/// holds none. Ruling `345/total-resolve.a` gives `Resolve<K>` a total domain;
/// holon-space/holon-work#345 implements it, and
/// [`a_withdrawn_last_claim_leaves_a_slot_no_device_can_retract`] pins the
/// mechanism deterministically.
///
/// So this test asserts the failure AND its named reason: a slot differs
/// because one device holds no slot at all, after a claim was withdrawn. Any
/// other failure is novel and fails here. When the property starts to hold,
/// this test fails and the marker goes.
#[test]
fn the_set_function_rule_converges_on_both_devices() {
    match run_property("set-function convergence", |s| {
        converges(&s, Rule::SetFunction)
    }) {
        Err(TestError::Fail(reason, counterexample)) => {
            // The reason is printed, never panicked with: the registry pattern
            // matches its text, so putting it in a panic message would let a
            // NOVEL failure wear the known red's words.
            println!("KNOWN RED slot-withdrawn-last-claim-no-retraction (#345), as registered.");
            println!("reason: {reason}");
            println!("shrunk counterexample: {counterexample:#?}");
            let reason = reason.to_string();
            assert!(
                reason.contains("differs between the devices"),
                "the set-function rule failed for a reason the known red \
                 slot-withdrawn-last-claim-no-retraction (#345) does not cover: \
                 no place of a device diverged (see the printed reason above)"
            );
            assert!(
                reason.contains("left: `None`") || reason.contains("right: `None`"),
                "the set-function rule diverged with BOTH devices holding a place, \
                 which the known red slot-withdrawn-last-claim-no-retraction (#345) \
                 does not cover: its shape is one device holding nothing at all"
            );
            assert!(
                format!("{counterexample:?}").contains("Withdraw"),
                "the set-function rule diverged WITHOUT a withdrawn claim, which the \
                 known red slot-withdrawn-last-claim-no-retraction (#345) does not \
                 cover: its mechanism is the empty contestant set"
            );
        }
        Err(TestError::Abort(why)) => panic!("the set-function run aborted: {why}"),
        Ok(()) => panic!(
            "the set-function rule converged in every case: remove the known-red \
             marker, #345 fixed it"
        ),
    }
}

/// The negative control. A rule whose input is the local arrival order cannot
/// converge, so a run that finds NO counterexample is itself the failure. The
/// shrunk counterexample it prints is what the red log records.
#[test]
fn the_naive_first_arrival_rule_does_not_converge() {
    match run_property("naive first-arrival (negative control)", |s| {
        converges(&s, Rule::Naive)
    }) {
        Err(TestError::Fail(reason, counterexample)) => {
            println!("NAIVE RULE RED, as required.");
            println!("reason: {reason}");
            println!("shrunk counterexample: {counterexample:#?}");
        }
        Err(TestError::Abort(why)) => panic!("the naive run aborted: {why}"),
        Ok(()) => panic!(
            "the naive first-arrival rule converged in every case: \
             the negative control has no teeth"
        ),
    }
}

/// Run 2 of the card: device B carries one extra well-known-id policy row. The
/// slot is NOT expected to agree — the contestant set converges but the ranking
/// of it does not. What must still hold is that no edit is lost, no id is
/// duplicated and the write-back bytes stay equal.
#[test]
fn a_policy_skew_keeps_every_edit_but_not_the_slot() {
    run_property("policy skew", |s| {
        let mut world = World::new(Rule::SetFunction, true);
        world.run(&s);
        world.quiesce();
        assert_shared_invariants(&world)
    })
    .expect("a policy skew must not lose an edit or duplicate an id");
}

/// The hlc is STORED with the claim and never ranked: inverting every clock
/// must not change the outcome. A metamorphic oracle — it compares the resolver
/// with itself under a transformation, never with an expected value.
#[test]
fn the_stored_hlc_is_not_ranked() {
    run_property("hlc is not ranked", |s| {
        let plain = {
            let mut w = World::new(Rule::SetFunction, false);
            w.run(&s);
            w.quiesce();
            w.a.write_back()
        };
        let inverted: Vec<Step> = s
            .iter()
            .map(|step| match *step {
                Step::Claim {
                    device,
                    key,
                    origin,
                    hlc,
                } => Step::Claim {
                    device,
                    key,
                    origin,
                    // An offline device's clock runs behind, a skewed one
                    // ahead. Either way the winner may not move.
                    hlc: 999_999 - hlc,
                },
                ref other => other.clone(),
            })
            .collect();
        let shifted = {
            let mut w = World::new(Rule::SetFunction, false);
            w.run(&inverted);
            w.quiesce();
            w.a.write_back()
        };
        // The hlc itself is part of the write-back, so compare it masked out.
        let mask = |s: String| -> Vec<String> {
            s.lines()
                .map(|l| {
                    l.split(' ')
                        .filter(|f| !f.starts_with("hlc="))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect()
        };
        prop_assert_eq!(mask(plain), mask(shifted), "a clock skew moved the outcome");
        Ok(())
    })
    .expect("the stored hlc must not rank the contestants");
}

// ---------------------------------------------------- hand-authored edge cases

/// A double ingest of ONE file: the same id under the same origin is the same
/// entity, so it merges and nothing is displaced.
#[test]
fn a_double_ingest_of_one_file_merges_with_no_loser() {
    let mut world = World::new(Rule::SetFunction, false);
    world.claim(true, KEYS[0], ORIGINS[0], 10);
    world.add_child(true, KEYS[0], 0);
    world.sync();
    world.claim(false, KEYS[0], ORIGINS[0], 20);
    world.add_child(false, KEYS[0], 0);
    world.quiesce();

    // The merge is a TRANSIENT of the resolution sequence, not a durable field
    // of the slot: it shrinks the contestant set from two to one, and `Resolve`
    // re-fires on the survivor with `merge = false` and an empty condition. The
    // net holds the CURRENT set only, so the decision is asserted where it
    // survives — both devices having applied it.
    for dev in [&world.a, &world.b] {
        assert!(
            dev.merges.contains(KEYS[0]),
            "{}: a double ingest of one origin did not merge",
            dev.name
        );
    }
    let slot = world
        .a
        .token(PLACE_SLOT, KEYS[0])
        .expect("the slot is written");
    assert_eq!(
        slot.get("merge"),
        Some(&Value::Boolean(false)),
        "the follow-up firing on the single survivor should report no contest"
    );
    let displaced = world
        .a
        .token(PLACE_DISPLACED, KEYS[0])
        .expect("the displaced set is written");
    assert_eq!(
        string_of(&displaced, "refs"),
        "[]",
        "a merge produced a loser"
    );
    let carriers = world.a.carriers(KEYS[0]);
    assert_eq!(carriers.len(), 1, "the merge left two carriers");
    // The secondary node has no children at quiescence: it was emptied by
    // MOVES and then deleted, so both children sit under the winner.
    assert_eq!(
        live_children(&world.a.tree(), carriers[0]).len(),
        2,
        "the merge did not move every child under the winner"
    );
    assert_eq!(
        world.a.write_back(),
        world.b.write_back(),
        "the devices would write different bytes"
    );
}

/// A copied file carries the same id for a DIFFERENT entity. Expect a PICK plus
/// a deterministic re-mint, never a merge — and the re-minted id must be the
/// same on both devices.
#[test]
fn a_copied_file_with_the_same_id_is_picked_and_re_minted_alike() {
    let mut world = World::new(Rule::SetFunction, false);
    world.claim(true, KEYS[0], ORIGINS[0], 10);
    world.claim(false, KEYS[0], ORIGINS[1], 20);
    world.quiesce();

    let slot = world
        .a
        .token(PLACE_SLOT, KEYS[0])
        .expect("the slot is written");
    assert_eq!(
        slot.get("merge"),
        Some(&Value::Boolean(false)),
        "two origins were merged into one entity"
    );
    assert_eq!(
        world.a.carriers(KEYS[0]).len(),
        1,
        "the pick left the key contested"
    );
    let a_ids: Vec<String> = world.a.live_ids().into_keys().collect();
    let b_ids: Vec<String> = world.b.live_ids().into_keys().collect();
    assert_eq!(a_ids, b_ids, "the two devices re-minted different ids");
    let prefix = format!("{}-displaced-", KEYS[0]);
    assert!(
        a_ids.iter().any(|i| i.starts_with(&prefix)),
        "no deterministic re-mint happened: {a_ids:?}"
    );
}

/// An offline device returns with an OLDER claim. The set function ranks the
/// stamp, not recency, so the older claim may well win — the oracle is that no
/// edit is lost either way.
#[test]
fn an_offline_device_returning_with_an_older_claim_loses_no_edit() {
    let mut world = World::new(Rule::SetFunction, false);
    // A claims and is edited all week while B is offline.
    world.claim(true, KEYS[0], ORIGINS[0], 900);
    for _ in 0..4 {
        world.edit(true, KEYS[0], 0, EditKind::Log);
        world.add_child(true, KEYS[0], 0);
    }
    // B's claim is OLDER by the clock and was never synced.
    world.claim(false, KEYS[0], ORIGINS[1], 1);
    world.edit(false, KEYS[0], 0, EditKind::Log);
    world.quiesce();

    assert_shared_invariants(&world).expect("no edit may be lost");
    assert_converged(&world).expect("the devices must agree");
}

/// A delete and its undo must not move the slot: the stamp is STORED, so the
/// new op id the undo gives the node is not what the preference reads.
#[test]
fn a_delete_and_undo_with_no_swap() {
    let mut world = World::new(Rule::SetFunction, false);
    world.claim(true, KEYS[0], ORIGINS[0], 10);
    world.claim(true, KEYS[0], ORIGINS[1], 20);
    world.quiesce();
    let before = world
        .a
        .token(PLACE_SLOT, KEYS[0])
        .expect("the slot is written");

    // Delete the holder, then put an identical claim back: same origin, same
    // STORED stamp, a new op id.
    let holder = world.a.carriers(KEYS[0])[0];
    let tree = world.a.tree();
    let stamp = meta_str(&tree, holder, "stamp").expect("the holder has a stamp");
    let origin = meta_str(&tree, holder, "origin").expect("the holder has an origin");
    tree.delete(holder).expect("the delete lands");
    world.a.doc.commit();
    world.quiesce();

    let tree = world.a.tree();
    let node = tree
        .create(TreeParentId::Root)
        .expect("the root accepts a child");
    let meta = tree.get_meta(node).expect("a fresh node has meta");
    meta.insert(STABLE_ID, KEYS[0]).expect("meta takes the id");
    meta.insert("stamp", stamp).expect("meta takes the stamp");
    meta.insert("origin", origin)
        .expect("meta takes the origin");
    meta.insert("hlc", 10_i64).expect("meta takes the hlc");
    meta.insert("title", "title-restored")
        .expect("meta takes the title");
    meta.insert("state", "TODO").expect("meta takes the state");
    meta.insert("state_base", "TODO")
        .expect("meta takes the state base");
    world.a.doc.commit();
    world.quiesce();

    assert_eq!(
        world.a.token(PLACE_SLOT, KEYS[0]),
        Some(before),
        "a delete and an undo of the same claim moved the slot"
    );
    assert_converged(&world).expect("the devices must agree after a delete and undo");
}

/// THE FINDING of this spike, as the shrunk counterexample of
/// [`the_set_function_rule_converges_on_both_devices`] reduced it:
///
/// ```text
/// [Claim { device: true, key: 0, origin: 0, hlc: 0 },
///  Resolve { device: true },
///  Withdraw { device: true, key: 0, nth: 0 }]
/// ```
///
/// One device, one claim, no contest and no second claimant. A resolves the
/// key, then withdraws the only carrier. The slot A wrote is never retracted,
/// because the contestant set is now EMPTY and nothing resolves an empty set:
/// `keys_in_play` enumerates only keys a LIVE node claims, so the key is not
/// even revisited. B, which first saw the key already deleted, never wrote a
/// slot at all. The two markings differ for good.
///
/// The preference function is NOT implicated: there is nothing to prefer. What
/// the case refutes is the completeness of "the winner is a pure function of
/// the contestant set" (#345) — a pure function of the set must be TOTAL over
/// sets, and the empty set is in its domain. Without a firing on the empty set
/// there is no retraction, and a derived place that is never retracted is not a
/// function of its input but of the local history of inputs.
///
/// This test pins the CURRENT behaviour so the finding is evidence in the tree.
/// It goes red when #345 gains its retraction rule — which is the point: the
/// red is then the proof the gap is closed, and this test is deleted with it.
#[test]
fn a_withdrawn_last_claim_leaves_a_slot_no_device_can_retract() {
    let mut world = World::new(Rule::SetFunction, false);
    world.claim(true, KEYS[0], ORIGINS[0], 0);
    world.a.resolve_to_fixpoint();
    world.withdraw(true, KEYS[0], 0);
    world.quiesce();

    // Both devices agree the key has no carrier left...
    for dev in [&world.a, &world.b] {
        assert!(
            dev.carriers(KEYS[0]).is_empty(),
            "{}: the withdraw left a carrier",
            dev.name
        );
    }
    // ...and the documents themselves have converged: equal write-back bytes.
    assert_eq!(
        world.a.write_back(),
        world.b.write_back(),
        "the documents did not converge, which would be a different bug"
    );
    // But the NET's output place did not. A holds a slot naming a contestant
    // that no longer exists; B holds none.
    let (a_slot, b_slot) = (
        world.a.token(PLACE_SLOT, KEYS[0]),
        world.b.token(PLACE_SLOT, KEYS[0]),
    );
    assert!(
        a_slot.is_some(),
        "A resolved the key before withdrawing it, so it holds a slot"
    );
    assert_eq!(
        b_slot, None,
        "B never saw a live carrier, so it never resolved the key"
    );
    assert_ne!(
        a_slot, b_slot,
        "the slot converged after all: #345 grew its retraction rule and this \
         pinned finding is obsolete - delete this test"
    );
}
