//! The shopping-list peer: the per-list wire vocabulary, the `(name, cat)`
//! ingest key, and the reconciler that turns one complete remote snapshot into
//! intents against the local rows and commands for the peer.
//!
//! Holon is a peer here, never the master. The peer issues no item id and no
//! per-item timestamp, so identity is the `(name, cat)` pair and absence inside
//! a *complete* fetch is the only deletion signal there is. Both consequences
//! are spelled out in `docs/Plans/Kitchen.md` §4, which this module implements.
//!
//! The wire contract is `docs/Plans/ThatShoppingList-API-2026-09-01.md`:
//! `{items, pickedItems, version, options}`. `items` is the ACTIVE list and
//! `pickedItems` the checked-off ones keyed by name, so membership IS the
//! checked flag; `options.cats` is THAT list's category vocabulary; `version`
//! carries the optimistic concurrency the write leg commits against.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use anyhow::Context as _;
use anyhow::Result;
use chrono::DateTime;
use chrono::Duration;
use chrono::FixedOffset;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::file_format::TypedRowSet;

/// One entry of a list's `options.cats`.
///
/// The wire text is `<code>` or `<code>_<icon>_<color>`
/// (`Kleidung_clothes_1976D2`); the leading segment is what an item's `cat`
/// names and the rest is presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryEntry {
    code: String,
    icon: Option<String>,
    color: Option<String>,
}

impl CategoryEntry {
    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn icon(&self) -> Option<&str> {
        self.icon.as_deref()
    }

    pub fn color(&self) -> Option<&str> {
        self.color.as_deref()
    }
}

/// The category vocabulary of ONE list, as that list published it.
///
/// There is no compiled-in aisle table: each list carries its own
/// `options.cats` and two lists need not agree, so a fixed enum would mislabel
/// one of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CategoryVocabulary {
    by_code: BTreeMap<String, CategoryEntry>,
}

impl CategoryVocabulary {
    /// Resolve an item's `cat` against this list's vocabulary.
    ///
    /// Total by construction: a code the list did not publish yields an
    /// unrecognized category carrying the wire text verbatim, so one new aisle
    /// can never fail a fetch and take the whole list down with it. A view
    /// shows such a category as degraded rather than pretending it
    /// understood it.
    pub fn resolve(&self, raw: &str) -> ShoppingCategory {
        ShoppingCategory {
            wire: raw.to_string(),
            entry: self.by_code.get(raw).cloned(),
        }
    }

    pub fn codes(&self) -> impl Iterator<Item = &str> {
        self.by_code.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_code.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_code.is_empty()
    }
}

/// An item's `cat`, resolved against the list that served it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShoppingCategory {
    wire: String,
    entry: Option<CategoryEntry>,
}

impl PartialOrd for CategoryEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CategoryEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.code.cmp(&other.code)
    }
}

impl std::hash::Hash for CategoryEntry {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.code.hash(state);
    }
}

impl ShoppingCategory {
    /// A category outside any vocabulary — for a local row read back from SQL,
    /// where the `cat` column holds the wire text and no list is at hand.
    pub fn unresolved(raw: &str) -> Self {
        Self {
            wire: raw.to_string(),
            entry: None,
        }
    }

    /// The exact string the peer sent. This is the identity half that goes back
    /// on the wire, so it is stored and round-tripped verbatim.
    pub fn as_wire(&self) -> &str {
        &self.wire
    }

    /// Whether the serving list published this code.
    pub fn is_recognized(&self) -> bool {
        self.entry.is_some()
    }

    pub fn entry(&self) -> Option<&CategoryEntry> {
        self.entry.as_ref()
    }

    /// Reader-facing text: the list's own code, which is also its label — the
    /// vocabulary publishes no separate display string.
    pub fn label(&self) -> &str {
        &self.wire
    }
}

/// The reconciliation key. The peer issues no id, so the pair below IS the
/// identity — with the two costs `docs/Plans/Kitchen.md` §4 states, both
/// confirmed against the phone API: duplicate names in one category collapse
/// into one item, and a rename is emitted as `del` + `add`, so local-only state
/// attached to the old key cannot survive it (R4, accepted as disclosed).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemKey {
    pub name: String,
    pub cat: String,
}

impl ItemKey {
    pub fn new(name: impl Into<String>, category: &ShoppingCategory) -> Self {
        Self {
            name: name.into(),
            cat: category.as_wire().to_string(),
        }
    }

    /// The `shopping_item.id` this key owns. Derived rather than minted so a
    /// re-add after a deletion lands on the same row instead of accumulating
    /// orphans. A category code never contains `:`, so the first two segments
    /// are unambiguous however the name is punctuated.
    pub fn row_id(&self) -> String {
        format!("shopping:{}:{}", self.cat, self.name)
    }
}

/// One item as the peer serves it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteShoppingItem {
    pub name: String,
    pub category: ShoppingCategory,
    pub count: Option<f64>,
    /// True iff the peer carried this item in `pickedItems` rather than
    /// `items`.
    pub checked: bool,
}

impl RemoteShoppingItem {
    pub fn key(&self) -> ItemKey {
        ItemKey::new(self.name.clone(), &self.category)
    }
}

/// The optimistic-concurrency token of one list.
///
/// The peer versions the active list and the picked-items map separately and a
/// commit must echo both. A read response carries only `version`; the
/// `pickedItemsVersion` a commit answers with is therefore the only place the
/// second number is observed, and a snapshot that has not seen one reuses
/// `version` — stated here because it is a reading of the capture, not a
/// measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListVersion {
    pub list: i64,
    pub picked: i64,
}

/// Every item one COMPLETE fetch carried, duplicate-folded on [`ItemKey`],
/// together with the vocabulary and version that fetch published.
///
/// Only a fetch that succeeded and parsed in full can produce one. That is what
/// licenses the reconciler to read absence as deletion: a truncated, failed or
/// malformed fetch has no way to reach it (`docs/Plans/Kitchen.md` §4).
#[derive(Debug, Clone)]
pub struct CompleteSnapshot {
    items: BTreeMap<ItemKey, RemoteShoppingItem>,
    vocabulary: CategoryVocabulary,
    version: ListVersion,
    fetched_at: String,
}

impl CompleteSnapshot {
    /// Build one whole-list snapshot from the rows the connection's `response`
    /// mapping produced (`assets/integrations/shopping.yaml`,
    /// `holon.tools.pull_list.response`).
    ///
    /// Nothing here knows the peer's JSON shape. The mapping selects the two
    /// item collections, folds duplicates on the `(name, cat)` identity this
    /// id-less peer forces, and raises rather than skipping a malformed record
    /// — so only a fetch that succeeded and mapped IN FULL can reach this
    /// constructor. That is what licenses the reconciler to read absence as
    /// deletion (`docs/Plans/Kitchen.md` §4).
    pub fn from_rows(rows: &[TypedRowSet], fetched_at: impl Into<String>) -> Result<Self> {
        let list = one_row(rows, "shopping_list")?;
        let version = ListVersion {
            list: int(list, "version")?,
            picked: int(list, "picked_items_version")?,
        };

        let mut by_code: BTreeMap<String, CategoryEntry> = BTreeMap::new();
        for row in rows_of(rows, "shopping_category") {
            let entry = CategoryEntry {
                code: text(row, "code")?,
                icon: optional_text(row, "icon")?,
                color: optional_text(row, "color")?,
            };
            anyhow::ensure!(
                by_code.insert(entry.code.clone(), entry).is_none(),
                "two `shopping_category` rows carry the same code, so an item's `cat` would \
                 resolve through whichever the map kept"
            );
        }
        let vocabulary = CategoryVocabulary { by_code };

        let mut items: BTreeMap<ItemKey, RemoteShoppingItem> = BTreeMap::new();
        for row in rows_of(rows, "shopping_item") {
            let item = RemoteShoppingItem {
                name: text(row, "name")?,
                category: vocabulary.resolve(&text(row, "cat")?),
                count: optional_number(row, "count")?,
                checked: boolean(row, "checked")?,
            };
            let key = item.key();
            anyhow::ensure!(
                items.insert(key.clone(), item).is_none(),
                "two `shopping_item` rows carry the key {key:?}; the mapping folds duplicates, so \
                 a repeated key means it did not"
            );
        }

        Ok(Self {
            items,
            vocabulary,
            version,
            fetched_at: fetched_at.into(),
        })
    }

    pub fn items(&self) -> impl Iterator<Item = &RemoteShoppingItem> {
        self.items.values()
    }

    pub fn vocabulary(&self) -> &CategoryVocabulary {
        &self.vocabulary
    }

    pub fn version(&self) -> ListVersion {
        self.version
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// When this fetch completed, RFC 3339. Becomes the `last_seen_remote`
    /// watermark of every item it carried.
    pub fn fetched_at(&self) -> &str {
        &self.fetched_at
    }
}

/// The single row of `type_name`, or a loud failure. A snapshot needs exactly
/// one version, and both zero and two are a mapping that did not do its job.
fn one_row<'a>(rows: &'a [TypedRowSet], type_name: &'static str) -> Result<&'a StorageEntity> {
    let found: Vec<_> = rows_of(rows, type_name).collect();
    match found.as_slice() {
        [row] => Ok(row),
        other => anyhow::bail!(
            "the mapped stream carries {} `{type_name}` rows, and a snapshot has exactly one",
            other.len()
        ),
    }
}

fn rows_of<'a>(
    rows: &'a [TypedRowSet],
    type_name: &'static str,
) -> impl Iterator<Item = &'a StorageEntity> {
    rows.iter()
        .filter(move |s| s.type_name == type_name)
        .flat_map(|s| s.rows.iter())
}

fn text(row: &StorageEntity, column: &str) -> Result<String> {
    match row.get(column) {
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
        other => anyhow::bail!("column `{column}` must be a non-empty string, got {other:?}"),
    }
}

fn optional_text(row: &StorageEntity, column: &str) -> Result<Option<String>> {
    match row.get(column) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        other => anyhow::bail!("column `{column}` must be a string or null, got {other:?}"),
    }
}

fn int(row: &StorageEntity, column: &str) -> Result<i64> {
    match row.get(column) {
        Some(Value::Integer(n)) => Ok(*n),
        other => anyhow::bail!("column `{column}` must be a whole number, got {other:?}"),
    }
}

/// A count arrives as either integer or float: the peer writes `2`, the fold of
/// two rows under one key writes `2.0`, and both mean two of the thing.
fn optional_number(row: &StorageEntity, column: &str) -> Result<Option<f64>> {
    match row.get(column) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Integer(n)) => Ok(Some(*n as f64)),
        Some(Value::Float(f)) => Ok(Some(*f)),
        other => anyhow::bail!("column `{column}` must be a number or null, got {other:?}"),
    }
}

fn boolean(row: &StorageEntity, column: &str) -> Result<bool> {
    match row.get(column) {
        Some(Value::Boolean(b)) => Ok(*b),
        other => anyhow::bail!("column `{column}` must be a boolean, got {other:?}"),
    }
}

/// A row of the local `shopping_item` table.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalShoppingItem {
    pub id: String,
    pub name: String,
    pub category: ShoppingCategory,
    pub count: Option<f64>,
    /// Filled from the peer's `pickedItems` membership on the way in, and never
    /// pushed on the way out: the commit encoding for a check toggle is the one
    /// command shape the capture did not isolate (see [`PushIntent`]).
    pub checked: bool,
    /// Local-only: filled by the product binding, never sent to the peer.
    pub product_id: Option<String>,
    /// Set when the item is deleted locally, so the next pull does not
    /// resurrect it before the deletion has been pushed. RFC 3339.
    pub deleted_at: Option<String>,
    /// The last complete fetch that carried this item, RFC 3339. `None` marks a
    /// row the peer has never sent — a local addition, which absence must
    /// therefore never delete.
    pub last_seen_remote: Option<String>,
}

impl LocalShoppingItem {
    pub fn key(&self) -> ItemKey {
        ItemKey::new(self.name.clone(), &self.category)
    }
}

/// A change to apply to the local `shopping_item` rows.
#[derive(Debug, Clone, PartialEq)]
pub enum LocalIntent {
    Insert(LocalShoppingItem),
    /// The peer's count for a row we already hold.
    SetCount {
        id: String,
        count: Option<f64>,
    },
    /// Move the watermark forward for a row this snapshot still carried.
    TouchLastSeenRemote {
        id: String,
        at: String,
    },
    /// The key is gone from a complete snapshot: an authoritative deletion.
    Delete {
        id: String,
    },
    /// The peer no longer carries a key we had tombstoned, so the tombstone has
    /// done its job and the row can go.
    ReapTombstone {
        id: String,
    },
    /// The peer reports the item checked off and we do not.
    ///
    /// There is deliberately no un-check intent. With no timestamp to arbitrate
    /// with, §4 rules check-off wins over un-check on asymmetric cost: a wrong
    /// check skips one item, a wrong uncheck re-buys it every trip until
    /// someone notices. Making the losing direction unrepresentable is
    /// cheaper than remembering the rule at each call site.
    Check {
        id: String,
    },
}

/// A change the peer needs, delivered as a `/commit` command.
///
/// A local check carries no variant. The peer moves an item between `items` and
/// `pickedItems` with a command shape the capture recorded only alongside an
/// add/del pair, so sending a guessed encoding risks deleting the item instead
/// of ticking it. Until an isolated check/uncheck capture pins it, `checked`
/// travels inbound only — stated in `docs/Plans/Kitchen.md` §4 and asserted in
/// the reconciler tests.
#[derive(Debug, Clone, PartialEq)]
pub enum PushIntent {
    Add(LocalShoppingItem),
    Remove { key: ItemKey, deleted_at: String },
}

/// What one reconciliation decided.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReconcileOutcome {
    pub local: Vec<LocalIntent>,
    pub push: Vec<PushIntent>,
}

/// Turns `(local rows, one complete remote snapshot)` into intents.
pub struct ShoppingReconciler {
    tombstone_window: Duration,
}

/// How long a local deletion outlives the row, so a pull cannot resurrect it
/// before the write leg has pushed it. Comfortably above any poll cadence.
pub const DEFAULT_TOMBSTONE_WINDOW_DAYS: i64 = 7;

impl Default for ShoppingReconciler {
    fn default() -> Self {
        Self {
            tombstone_window: Duration::days(DEFAULT_TOMBSTONE_WINDOW_DAYS),
        }
    }
}

impl ShoppingReconciler {
    pub fn with_tombstone_window(tombstone_window: Duration) -> Self {
        Self { tombstone_window }
    }

    pub fn reconcile(
        &self,
        local: &[LocalShoppingItem],
        snapshot: &CompleteSnapshot,
    ) -> Result<ReconcileOutcome> {
        let fetched_at = parse_timestamp(snapshot.fetched_at(), "the snapshot's fetch time")?;
        let mut outcome = ReconcileOutcome::default();
        let mut by_key: BTreeMap<ItemKey, &LocalShoppingItem> = BTreeMap::new();
        for row in local {
            let key = row.key();
            anyhow::ensure!(
                by_key.insert(key.clone(), row).is_none(),
                "two local shopping rows share the key {key:?}; the (name, cat) key is the row \
                 identity, so a duplicate means the table was written past the reconciler"
            );
        }

        let mut seen: BTreeSet<ItemKey> = BTreeSet::new();
        for remote in snapshot.items() {
            let key = remote.key();
            seen.insert(key.clone());
            match by_key.get(&key) {
                None => outcome.local.push(LocalIntent::Insert(row_from(
                    remote,
                    &key.row_id(),
                    snapshot,
                ))),
                Some(row) => match &row.deleted_at {
                    // A live tombstone means WE deleted it and the peer has not
                    // been told yet; the pull must not undo our own delete.
                    Some(deleted_at)
                        if !self.expired(
                            parse_timestamp(deleted_at, "a local tombstone")?,
                            fetched_at,
                        ) =>
                    {
                        outcome.push.push(PushIntent::Remove {
                            key: key.clone(),
                            deleted_at: deleted_at.clone(),
                        });
                    }
                    // An expired tombstone has outlived its purpose; the peer
                    // still lists the item, so it comes back.
                    Some(_) => outcome
                        .local
                        .push(LocalIntent::Insert(row_from(remote, &row.id, snapshot))),
                    None => {
                        // No local write timestamp exists to arbitrate with, so
                        // the fetched value stands (§4's last-writer rule
                        // becomes decidable once the write leg records one).
                        if row.count != remote.count {
                            outcome.local.push(LocalIntent::SetCount {
                                id: row.id.clone(),
                                count: remote.count,
                            });
                        }
                        // One-way by §4: a peer-side check reaches the row, a
                        // peer-side un-check never clears a local one.
                        if remote.checked && !row.checked {
                            outcome
                                .local
                                .push(LocalIntent::Check { id: row.id.clone() });
                        }
                        outcome.local.push(LocalIntent::TouchLastSeenRemote {
                            id: row.id.clone(),
                            at: snapshot.fetched_at().to_string(),
                        });
                    }
                },
            }
        }

        for (key, row) in &by_key {
            if seen.contains(key) {
                continue;
            }
            match (&row.deleted_at, &row.last_seen_remote) {
                (Some(_), _) => outcome
                    .local
                    .push(LocalIntent::ReapTombstone { id: row.id.clone() }),
                // The peer has carried this item before and no longer does.
                // Inside a COMPLETE snapshot that is the deletion signal.
                (None, Some(_)) => outcome
                    .local
                    .push(LocalIntent::Delete { id: row.id.clone() }),
                // Never sent by the peer, so its absence says nothing about it.
                (None, None) => outcome.push.push(PushIntent::Add((*row).clone())),
            }
        }

        Ok(outcome)
    }

    fn expired(&self, deleted_at: DateTime<FixedOffset>, now: DateTime<FixedOffset>) -> bool {
        now - deleted_at > self.tombstone_window
    }
}

fn row_from(
    remote: &RemoteShoppingItem,
    id: &str,
    snapshot: &CompleteSnapshot,
) -> LocalShoppingItem {
    LocalShoppingItem {
        id: id.to_string(),
        name: remote.name.clone(),
        category: remote.category.clone(),
        count: remote.count,
        checked: remote.checked,
        product_id: None,
        deleted_at: None,
        last_seen_remote: Some(snapshot.fetched_at().to_string()),
    }
}

fn parse_timestamp(raw: &str, what: &str) -> Result<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(raw)
        .with_context(|| format!("{what} is not an RFC 3339 timestamp: '{raw}'"))
}
