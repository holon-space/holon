//! Property-based test of one shopping-sync ROUND against a peer that both
//! sides mutate.
//!
//! The generator produces interleavings — peer-side adds/deletes/checks, local
//! adds/deletes, and sync rounds that may be perturbed by a STALE verifying
//! re-pull and by a concurrent writer landing between the commit and that
//! re-pull. Hand-enumerating that space is what the example suite used to do,
//! and it is exactly the space where `sync_once`'s retry rule can double-apply.
//!
//! **The mock peer honours the documented idempotency key**
//! (`docs/Plans/ThatShoppingList-API-2026-09-01.md`: each command's `id` is
//! "client-generated, idempotency/ordering key"): a command whose id it has
//! already applied is ignored. It deliberately does NOT deduplicate by
//! `(name, cat)` — the peer allows two entries with one name, and Holon's own
//! parser folds them, so a duplicate the sync CREATES would be invisible from
//! the local side. Letting the mock keep the duplicate is what makes the damage
//! observable at all.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use holon_kitchen::shopping::CompleteSnapshot;
use holon_kitchen::shopping::ItemKey;
use holon_kitchen::shopping::LocalIntent;
use holon_kitchen::shopping::LocalShoppingItem;
use holon_kitchen::shopping::ShoppingCategory;
use holon_kitchen::shopping::ShoppingReconciler;
use holon_kitchen::shopping_sync::CommitAck;
use holon_kitchen::shopping_sync::CommitBatch;
use holon_kitchen::shopping_sync::ShoppingPeer;
use holon_kitchen::shopping_sync::ShoppingRowReader;
use holon_kitchen::shopping_sync::sync_once;
use holon_rows::RowMapper;
use proptest::prelude::*;

/// The shipped sidecar. The mock peer answers a pull the way production reads
/// one: the body through this file's `response` mapping, then the rows.
const SIDECAR: &str = include_str!("../../../assets/integrations/shopping.yaml");

/// A snapshot built the way production builds one: the response through the
/// shipped sidecar's `response` mapping, then the rows.
fn snapshot_from_body(
    body: &serde_json::Map<String, serde_json::Value>,
    fetched_at: &str,
) -> Result<CompleteSnapshot> {
    let doc: serde_yaml::Value = serde_yaml::from_str(SIDECAR).expect("the sidecar parses");
    let filter = doc["holon"]["tools"]["pull_list"]["response"]
        .as_str()
        .expect("holon.tools.pull_list.response is a jaq filter");
    let mapper = RowMapper::compile("shopping/pull_list.response", filter)?;
    let rows = mapper.map_to_row_sets(&serde_json::Value::Object(body.clone()))?;
    CompleteSnapshot::from_rows(&rows, fetched_at)
}

/// `(id, verb, name, cat)` for every command the batch carries, read off the
/// row stream the write leg actually sends.
fn commands_of(batch: &CommitBatch) -> Vec<(String, String, String, String)> {
    let stream = batch.to_row_stream();
    stream["rows"]
        .as_array()
        .expect("the row stream carries rows")
        .iter()
        .filter(|entry| entry["type"] == "shopping_command")
        .map(|entry| {
            let row = &entry["row"];
            let field = |key: &str| {
                row[key]
                    .as_str()
                    .unwrap_or_else(|| panic!("a command row carries `{key}`"))
                    .to_string()
            };
            (
                field("command_id"),
                field("verb"),
                field("name"),
                field("cat"),
            )
        })
        .collect()
}

const DEVICE_ID: &str = "device-under-test";
const CATS: &[&str] = &["R", "B"];
const NAMES: &[&str] = &["Milk", "Bread", "Eggs"];

/// A small alphabet keeps interleavings dense: three names over two aisles
/// means generated steps collide on the same key often, which is where the
/// conflict rules actually live.
fn item() -> impl Strategy<Value = (String, String)> {
    (0..NAMES.len(), 0..CATS.len()).prop_map(|(n, c)| (NAMES[n].to_string(), CATS[c].to_string()))
}

#[derive(Debug, Clone)]
enum Step {
    PeerAdd((String, String)),
    PeerDel((String, String)),
    PeerCheck((String, String)),
    LocalAdd((String, String)),
    LocalDelete((String, String)),
    /// One round of `sync_once`. `stale_repull` serves the verifying pull from
    /// the snapshot taken BEFORE the commit — a cached GET. `concurrent_write`
    /// is another party's insert landing between the commit and that re-pull.
    Sync {
        stale_repull: bool,
        concurrent_write: Option<(String, String)>,
    },
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        2 => item().prop_map(Step::PeerAdd),
        1 => item().prop_map(Step::PeerDel),
        1 => item().prop_map(Step::PeerCheck),
        2 => item().prop_map(Step::LocalAdd),
        1 => item().prop_map(Step::LocalDelete),
        3 => (any::<bool>(), proptest::option::of(item())).prop_map(|(stale_repull, concurrent_write)| {
            Step::Sync { stale_repull, concurrent_write }
        }),
    ]
}

// ---------------------------------------------------------------------------
// The peer
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct PeerState {
    /// Duplicates are ALLOWED here — see the module note.
    items: Vec<(String, String)>,
    picked: Vec<(String, String)>,
    version: i64,
}

impl PeerState {
    fn body(&self) -> serde_json::Map<String, serde_json::Value> {
        let items: Vec<serde_json::Value> = self
            .items
            .iter()
            .map(|(name, cat)| serde_json::json!({"name": name, "cat": cat}))
            .collect();
        let picked: serde_json::Map<String, serde_json::Value> = self
            .picked
            .iter()
            .map(|(name, cat)| {
                (
                    name.clone(),
                    serde_json::json!({"cat": cat, "date": "2026-09-01T08:00:00Z"}),
                )
            })
            .collect();
        serde_json::json!({
            "items": items,
            "pickedItems": picked,
            "version": self.version,
            "options": {"prices": false, "cats": CATS},
        })
        .as_object()
        .cloned()
        .expect("the peer body is an object")
    }

    fn duplicate_keys(&self) -> Vec<(String, String)> {
        let mut seen: BTreeMap<(String, String), usize> = BTreeMap::new();
        for entry in &self.items {
            *seen.entry(entry.clone()).or_default() += 1;
        }
        seen.into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(k, _)| k)
            .collect()
    }
}

#[derive(Default)]
struct MockInner {
    state: PeerState,
    /// The documented idempotency key: a command id applied once never applies
    /// again.
    applied_ids: BTreeSet<String>,
    /// Set by a commit when the NEXT pull must be served stale.
    pending_stale: Option<PeerState>,
    /// Arming for the next commit, set by the harness before a round.
    stale_next_commit: bool,
    concurrent_write: Option<(String, String)>,
    commits: usize,
    /// Every (verb, name, cat) this peer actually APPLIED, with the id that
    /// carried it. Two entries sharing the triple are a logical command applied
    /// twice.
    applied_log: Vec<(String, String, String, String)>,
}

#[derive(Default)]
struct MockPeer {
    inner: Mutex<MockInner>,
}

#[async_trait]
impl ShoppingPeer for MockPeer {
    async fn pull(&self) -> Result<CompleteSnapshot> {
        let mut inner = self.inner.lock().expect("mock peer");
        let served = match inner.pending_stale.take() {
            Some(cached) => cached,
            None => inner.state.clone(),
        };
        snapshot_from_body(&served.body(), "2026-09-01T10:00:00Z")
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<CommitAck> {
        let mut inner = self.inner.lock().expect("mock peer");
        inner.commits += 1;

        // What a cached GET issued right after this commit would still show.
        let pre_commit = inner.state.clone();

        for (id, verb, name, cat) in commands_of(batch) {
            if !inner.applied_ids.insert(id.clone()) {
                continue;
            }
            match verb.as_str() {
                // Intent words, not this peer's wire words: a `CommitBatch`
                // reaches the mock before the sidecar's `request` mapping,
                // which is where `remove` becomes the peer's `del`.
                "add" => inner.state.items.push((name.clone(), cat.clone())),
                "remove" => {
                    inner
                        .state
                        .items
                        .retain(|e| e != &(name.clone(), cat.clone()));
                    inner
                        .state
                        .picked
                        .retain(|e| e != &(name.clone(), cat.clone()));
                }
                other => panic!("the mock peer received an unknown command '{other}'"),
            }
            inner.applied_log.push((verb, name, cat, id));
        }

        if let Some(entry) = inner.concurrent_write.take() {
            if !inner.state.items.contains(&entry) {
                inner.state.items.push(entry);
            }
        }
        inner.state.version += 1;

        if inner.stale_next_commit {
            inner.stale_next_commit = false;
            inner.pending_stale = Some(pre_commit);
        }
        Ok(CommitAck {
            version: inner.state.version,
            picked_items_version: inner.state.version,
        })
    }
}

struct Rows(Vec<LocalShoppingItem>);

#[async_trait]
impl ShoppingRowReader for Rows {
    async fn load(&self) -> Result<Vec<LocalShoppingItem>> {
        Ok(self.0.clone())
    }
}

// ---------------------------------------------------------------------------
// Local side
// ---------------------------------------------------------------------------

fn new_local(name: &str, cat: &str) -> LocalShoppingItem {
    let category = ShoppingCategory::unresolved(cat);
    LocalShoppingItem {
        id: ItemKey::new(name, &category).row_id(),
        name: name.to_string(),
        category,
        count: None,
        checked: false,
        product_id: None,
        deleted_at: None,
        // A row the peer has never carried: a pending Add.
        last_seen_remote: None,
    }
}

/// Stand in for the dispatcher: the intents the round returns are what the
/// generic write authority would have executed.
fn apply_local(rows: &mut Vec<LocalShoppingItem>, intents: &[LocalIntent]) {
    for intent in intents {
        match intent {
            LocalIntent::Insert(row) => match rows.iter_mut().find(|r| r.id == row.id) {
                Some(held) => *held = row.clone(),
                None => rows.push(row.clone()),
            },
            LocalIntent::SetCount { id, count } => {
                if let Some(row) = rows.iter_mut().find(|r| &r.id == id) {
                    row.count = *count;
                }
            }
            LocalIntent::TouchLastSeenRemote { id, at } => {
                if let Some(row) = rows.iter_mut().find(|r| &r.id == id) {
                    row.last_seen_remote = Some(at.clone());
                }
            }
            LocalIntent::Check { id } => {
                if let Some(row) = rows.iter_mut().find(|r| &r.id == id) {
                    row.checked = true;
                }
            }
            LocalIntent::Delete { id } | LocalIntent::ReapTombstone { id } => {
                rows.retain(|r| &r.id != id);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The property
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(256),
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_round_converges_without_applying_a_command_twice(
        steps in proptest::collection::vec(step(), 1..14)
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime");

        let peer = MockPeer::default();
        let mut rows: Vec<LocalShoppingItem> = Vec::new();
        let mut now_ms: i64 = 1_756_700_000_000;

        for step in &steps {
            match step {
                Step::PeerAdd(entry) => {
                    let mut inner = peer.inner.lock().expect("mock peer");
                    if !inner.state.items.contains(entry) {
                        inner.state.items.push(entry.clone());
                        inner.state.version += 1;
                    }
                }
                Step::PeerDel(entry) => {
                    let mut inner = peer.inner.lock().expect("mock peer");
                    inner.state.items.retain(|e| e != entry);
                    inner.state.picked.retain(|e| e != entry);
                    inner.state.version += 1;
                }
                Step::PeerCheck(entry) => {
                    let mut inner = peer.inner.lock().expect("mock peer");
                    if !inner.state.picked.contains(entry) {
                        inner.state.picked.push(entry.clone());
                        inner.state.version += 1;
                    }
                }
                Step::LocalAdd((name, cat)) => {
                    let row = new_local(name, cat);
                    if !rows.iter().any(|r| r.id == row.id) {
                        rows.push(row);
                    }
                }
                Step::LocalDelete((name, cat)) => {
                    let id = ItemKey::new(name.clone(), &ShoppingCategory::unresolved(cat)).row_id();
                    if let Some(row) = rows.iter_mut().find(|r| r.id == id) {
                        row.deleted_at = Some("2026-09-01T09:00:00Z".to_string());
                    }
                }
                Step::Sync { stale_repull, concurrent_write } => {
                    {
                        let mut inner = peer.inner.lock().expect("mock peer");
                        inner.stale_next_commit = *stale_repull;
                        inner.concurrent_write = concurrent_write.clone();
                    }

                    // Pending local Adds this round is responsible for: rows the
                    // peer has never carried and that are not locally deleted.
                    let pending: Vec<ItemKey> = rows
                        .iter()
                        .filter(|r| r.last_seen_remote.is_none() && r.deleted_at.is_none())
                        .map(|r| r.key())
                        .collect();
                    let checked_before: BTreeSet<String> = rows
                        .iter()
                        .filter(|r| r.checked)
                        .map(|r| r.id.clone())
                        .collect();

                    let before = Rows(rows.clone());
                    let outcome = runtime.block_on(sync_once(
                        &peer,
                        &before,
                        &ShoppingReconciler::default(),
                        DEVICE_ID,
                        now_ms,
                    ));
                    now_ms += 1_000;

                    let Ok(outcome) = outcome else {
                        // A round that FAILS is allowed — refusing to converge
                        // loudly is the designed answer to a list moving faster
                        // than a round. It must simply never have damaged the
                        // peer, which the invariants below still check.
                        let inner = peer.inner.lock().expect("mock peer");
                        prop_assert!(
                            inner.state.duplicate_keys().is_empty(),
                            "a failed round still duplicated {:?} at the peer",
                            inner.state.duplicate_keys()
                        );
                        continue;
                    };
                    apply_local(&mut rows, &outcome.local);

                    let inner = peer.inner.lock().expect("mock peer");

                    // O1 — no logical command was applied twice. This is the
                    // damage a stale re-pull can cause: the retry re-sends a
                    // command the peer already ran, and if its id differs the
                    // idempotency key cannot stop it.
                    let mut applied: BTreeMap<(String, String, String), usize> = BTreeMap::new();
                    for (verb, name, cat, _) in &inner.applied_log {
                        *applied
                            .entry((verb.clone(), name.clone(), cat.clone()))
                            .or_default() += 1;
                    }
                    prop_assert!(
                        inner.state.duplicate_keys().is_empty(),
                        "the peer holds duplicate item(s) {:?} — a command was applied twice; \
                         applied log: {:?}",
                        inner.state.duplicate_keys(),
                        inner.applied_log
                    );

                    // O2 — no local Add is lost. Every row the peer had never
                    // carried, and that is not locally deleted, reached it.
                    for key in &pending {
                        let entry = (key.name.clone(), key.cat.clone());
                        let still_local_deleted = rows
                            .iter()
                            .any(|r| r.key() == *key && r.deleted_at.is_some());
                        if still_local_deleted {
                            continue;
                        }
                        prop_assert!(
                            inner.state.items.contains(&entry)
                                || inner.state.picked.contains(&entry),
                            "a local addition {entry:?} never reached the peer; peer items {:?}",
                            inner.state.items
                        );
                    }

                    // O3 — §4's asymmetric rule: a check is never cleared by a
                    // pull.
                    for id in &checked_before {
                        prop_assert!(
                            rows.iter().all(|r| &r.id != id || r.checked),
                            "a pull cleared the local check on {id}"
                        );
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Each defence, on its own
// ---------------------------------------------------------------------------
//
// The property above is satisfied by EITHER defence: the stable id makes a
// re-send harmless, and the freshness floor stops the re-send happening. That
// is the behaviour worth having, but it means the property cannot tell which
// defence is carrying it. The two tests below pin them separately, so removing
// one is still caught.

/// Defence 1 — the idempotency key identifies the COMMAND, so two sends of one
/// logical command inside a round carry one id.
#[test]
fn a_retried_command_keeps_its_idempotency_key() {
    use holon_kitchen::shopping::ListVersion;
    use holon_kitchen::shopping::PushIntent;

    let row = new_local("Milk", "R");
    let push = vec![PushIntent::Add(row)];
    let round_ms = 1_756_700_000_000;

    let first = CommitBatch::from_push_intents(
        &push,
        ListVersion { list: 7, picked: 7 },
        DEVICE_ID,
        round_ms,
    );
    // The retry commits against the version the SECOND pull returned, so only
    // the version differs — the command is the same one.
    let retry = CommitBatch::from_push_intents(
        &push,
        ListVersion { list: 9, picked: 9 },
        DEVICE_ID,
        round_ms,
    );

    assert_eq!(
        first.commands[0].id, retry.commands[0].id,
        "a retried command changed its id, so the peer's idempotency key cannot recognise it"
    );

    // ...and a DIFFERENT logical command still gets a different id.
    let other = CommitBatch::from_push_intents(
        &[PushIntent::Add(new_local("Bread", "B"))],
        ListVersion { list: 7, picked: 7 },
        DEVICE_ID,
        round_ms,
    );
    assert_ne!(first.commands[0].id, other.commands[0].id);
}

/// Defence 2 — a verifying read older than a commit we KNOW landed is refused,
/// loudly, rather than being reconciled against and re-committed.
#[test]
fn a_permanently_stale_read_fails_the_round_instead_of_re_committing() {
    /// Serves the same version forever, so every post-commit read is older than
    /// the ack — an aggressively caching front end.
    #[derive(Default)]
    struct FrozenPeer {
        commits: Mutex<usize>,
    }

    #[async_trait]
    impl ShoppingPeer for FrozenPeer {
        async fn pull(&self) -> Result<CompleteSnapshot> {
            let frozen = PeerState {
                items: Vec::new(),
                picked: Vec::new(),
                version: 1,
            };
            snapshot_from_body(&frozen.body(), "2026-09-01T10:00:00Z")
        }

        async fn commit(&self, _batch: &CommitBatch) -> Result<CommitAck> {
            *self.commits.lock().expect("frozen peer") += 1;
            Ok(CommitAck {
                version: 99,
                picked_items_version: 99,
            })
        }
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("test runtime");
    let peer = FrozenPeer::default();
    let rows = Rows(vec![new_local("Milk", "R")]);

    let err = runtime
        .block_on(sync_once(
            &peer,
            &rows,
            &ShoppingReconciler::default(),
            DEVICE_ID,
            1_756_700_000_000,
        ))
        .expect_err("a read older than our own commit must not decide anything");
    let text = format!("{err:#}");
    assert!(
        text.contains("older than"),
        "the failure did not name the staleness: {text}"
    );
    assert_eq!(
        *peer.commits.lock().expect("frozen peer"),
        1,
        "the round committed again on the strength of a provably stale read"
    );
}
