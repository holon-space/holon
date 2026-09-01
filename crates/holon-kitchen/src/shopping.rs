//! The shopping-list peer's read leg: the wire vocabulary, the `(name, cat)`
//! ingest key, and the reconciler that turns one complete remote snapshot into
//! intents against the local rows.
//!
//! Holon is a peer here, never the master. The peer issues no item id and no
//! timestamp, so identity is the `(name, cat)` pair and absence inside a
//! *complete* fetch is the only deletion signal there is. Both consequences are
//! spelled out in `docs/Plans/Kitchen.md` §4, which this module implements.
//!
//! **This parses the Garmin-watch endpoint and is not wired to a live poll.**
//! The production target is the phone API in
//! `docs/Plans/ThatShoppingList-API-2026-09-01.md` (`{items, pickedItems,
//! version, options}`); swapping to it, together with its rotating-token auth,
//! the live wiring and the `/commit` write leg, is the next lane and waits on a
//! token-refresh handshake capture. Everything below the response parsing — the
//! key, the snapshot type, the reconciler and its intents — is shared by both
//! shapes.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use anyhow::Context as _;
use anyhow::Result;
use chrono::DateTime;
use chrono::Duration;
use chrono::FixedOffset;

/// Where the peer puts its item array inside a `list-items` response body.
pub const ITEMS_PATH: &str = "data.items";

/// The peer's published category vocabulary, code and English label together so
/// the two cannot drift apart.
macro_rules! known_categories {
    ($($variant:ident => $code:literal, $label:literal;)+) => {
        /// A category code in the peer's published vocabulary.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum KnownCategory {
            $(#[doc = $label] $variant,)+
        }

        impl KnownCategory {
            /// Every code this build knows, in declaration order.
            pub const ALL: &'static [KnownCategory] = &[$(KnownCategory::$variant,)+];

            /// The code as the peer writes it on the wire.
            pub fn code(self) -> &'static str {
                match self { $(KnownCategory::$variant => $code,)+ }
            }

            /// The peer's own English label for this aisle.
            pub fn label(self) -> &'static str {
                match self { $(KnownCategory::$variant => $label,)+ }
            }

            fn from_code(code: &str) -> Option<Self> {
                match code { $($code => Some(KnownCategory::$variant),)+ _ => None }
            }
        }
    };
}

known_categories! {
    FuV => "FuV", "Fruits - Vegetables - Nuts";
    Ca  => "Ca",  "Canned Food";
    MuF => "MuF", "Meat - Seafoods";
    D   => "D",   "Beverages - Spirits";
    DuH => "DuH", "Household - Toiletries - Baby - Pet";
    F   => "F",   "Frozen Food";
    R   => "R",   "Refrigerated - Dairy";
    P   => "P",   "Pasta - Rice";
    B   => "B",   "Bakery";
    S   => "S",   "Spread";
    C   => "C",   "Muesli - Cornflakes - Cereals";
    Cu  => "Cu",  "Cuisine - Baking";
    Sn  => "Sn",  "Sweets - Snacks";
    SuD => "SuD", "Sauces - Spices - Dressings - Oil";
    I   => "I",   "Ready meals - Broth - Gravies";
    CuT => "CuT", "Coffee - Tea";
    DIY => "DIY", "DIY - Electrical - Fixtures";
    O   => "O",   "Others";
    PH  => "PH",  "High priority";
    PM  => "PM",  "Medium priority";
    PL  => "PL",  "Low priority";
    Fw  => "Fw",  "Plants";
    Pa  => "Pa",  "Painting supplies";
    Gr  => "Gr",  "Garden";
    Pt  => "Pt",  "Electrical appliances";
    El  => "El",  "Electrics";
    Wo  => "Wo",  "Wood";
    Ro  => "Ro",  "Roof";
    Pi  => "Pi",  "Plumbing supplies";
    Sa  => "Sa",  "Sanitary";
    Br  => "Br",  "Building materials";
    Ws  => "Ws",  "Occupational safety";
    To  => "To",  "Tools";
    Ir  => "Ir",  "Hardware";
    Car => "Car", "Car - Bicycle";
}

/// The code half of a `cat` value.
///
/// A code outside [`KnownCategory`] is an expected state, not corruption: the
/// peer's own reference consumer displays an unrecognized code rather than
/// rejecting the item, and its shipped aisle order already names one (`Fish`)
/// that its label table omits. The wire text is kept verbatim so nothing is
/// lost and nothing is guessed onto a neighbouring aisle.
///
/// The phone API serves each list its OWN vocabulary in `options.cats`, so the
/// next lane replaces this fixed set with that per-list data.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CategoryCode {
    Known(KnownCategory),
    Unrecognized(String),
}

impl CategoryCode {
    pub fn as_wire(&self) -> &str {
        match self {
            CategoryCode::Known(k) => k.code(),
            CategoryCode::Unrecognized(raw) => raw,
        }
    }

    /// What to show a reader: the peer's label for a known code, the raw code
    /// itself for one this build does not know.
    pub fn label(&self) -> &str {
        match self {
            CategoryCode::Known(k) => k.label(),
            CategoryCode::Unrecognized(raw) => raw,
        }
    }
}

/// A `cat` field exactly as the peer issues it: a vocabulary code, optionally
/// followed by `_<qualifier>`.
///
/// The qualifier is carried rather than discarded because it is part of the
/// item's identity — two items that differ only in qualifier are two items to
/// the peer, and the write leg will have to send back the value it was given.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShoppingCategory {
    code: CategoryCode,
    qualifier: Option<String>,
}

impl ShoppingCategory {
    /// Parse a wire `cat`. Total by construction: every string denotes some
    /// category, known or not, so one new aisle on the peer can never fail a
    /// fetch and take the whole list down with it.
    pub fn parse(raw: &str) -> Self {
        let (code, qualifier) = match raw.split_once('_') {
            Some((code, qualifier)) => (code, Some(qualifier.to_string())),
            None => (raw, None),
        };
        let code = match KnownCategory::from_code(code) {
            Some(known) => CategoryCode::Known(known),
            None => CategoryCode::Unrecognized(code.to_string()),
        };
        Self { code, qualifier }
    }

    pub fn code(&self) -> &CategoryCode {
        &self.code
    }

    pub fn qualifier(&self) -> Option<&str> {
        self.qualifier.as_deref()
    }

    /// Whether this build recognizes the code. A view shows an unrecognized
    /// category as degraded rather than pretending it understood it.
    pub fn is_recognized(&self) -> bool {
        matches!(self.code, CategoryCode::Known(_))
    }

    /// The exact string the peer sent, rebuilt.
    pub fn as_wire(&self) -> String {
        match &self.qualifier {
            Some(q) => format!("{}_{}", self.code.as_wire(), q),
            None => self.code.as_wire().to_string(),
        }
    }

    /// Reader-facing text: the aisle label, with any qualifier appended.
    pub fn label(&self) -> String {
        match &self.qualifier {
            Some(q) => format!("{} ({q})", self.code.label()),
            None => self.code.label().to_string(),
        }
    }
}

/// The reconciliation key. The peer issues no id, so the pair below IS the
/// identity — with the two costs `docs/Plans/Kitchen.md` §4 states, both since
/// confirmed against the phone API: duplicate names in one category collapse
/// into one item, and a rename is emitted as `del` + `add`, so local-only state
/// attached to the old key cannot survive it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemKey {
    pub name: String,
    pub cat: String,
}

impl ItemKey {
    pub fn new(name: impl Into<String>, category: &ShoppingCategory) -> Self {
        Self {
            name: name.into(),
            cat: category.as_wire(),
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
}

impl RemoteShoppingItem {
    pub fn key(&self) -> ItemKey {
        ItemKey::new(self.name.clone(), &self.category)
    }
}

/// Every item one COMPLETE fetch carried, duplicate-folded on [`ItemKey`].
///
/// Only a fetch that succeeded and parsed in full can produce one. That is what
/// licenses the reconciler to read absence as deletion: a truncated, failed or
/// malformed fetch has no way to reach it (`docs/Plans/Kitchen.md` §4).
#[derive(Debug, Clone)]
pub struct CompleteSnapshot {
    items: BTreeMap<ItemKey, RemoteShoppingItem>,
    fetched_at: String,
}

impl CompleteSnapshot {
    /// Parse one whole `list-items` response body.
    ///
    /// The generic entity mirror cannot do this job: it keys rows on a
    /// server-issued id column and fails loud without one, and this peer issues
    /// none. So the item array is selected here and identity is decided by
    /// [`ItemKey`].
    pub fn from_response(
        response: &serde_json::Map<String, serde_json::Value>,
        fetched_at: impl Into<String>,
    ) -> Result<Self> {
        let items = response
            .get("data")
            .and_then(|d| d.as_object())
            .and_then(|d| d.get("items"))
            .ok_or_else(|| anyhow::anyhow!("response has no `{ITEMS_PATH}` array"))?
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("`{ITEMS_PATH}` is not an array"))?;
        let records = items
            .iter()
            .map(|v| {
                v.as_object()
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("`{ITEMS_PATH}` holds a non-object entry: {v}"))
            })
            .collect::<Result<Vec<_>>>()?;
        Self::from_records(&records, fetched_at)
    }

    /// Parse the records a completed item-array extraction yielded.
    ///
    /// A record missing `name`/`cat`, or carrying them at the wrong JSON type,
    /// fails the whole snapshot. Skipping the bad record would silently turn
    /// it into a deletion of a real item, which is the one outcome absence-as-
    /// deletion must never produce from bad input.
    pub fn from_records(
        records: &[serde_json::Map<String, serde_json::Value>],
        fetched_at: impl Into<String>,
    ) -> Result<Self> {
        let mut items: BTreeMap<ItemKey, RemoteShoppingItem> = BTreeMap::new();
        for (index, record) in records.iter().enumerate() {
            let item = parse_record(record)
                .with_context(|| format!("shopping item #{index} in the fetched list"))?;
            let key = item.key();
            match items.get_mut(&key) {
                // Two rows under one key are one item to us; folding their
                // counts is the only reading that does not lose a unit. An
                // absent count means one of the thing.
                Some(held) => {
                    held.count = Some(held.count.unwrap_or(1.0) + item.count.unwrap_or(1.0));
                }
                None => {
                    items.insert(key, item);
                }
            }
        }
        Ok(Self {
            items,
            fetched_at: fetched_at.into(),
        })
    }

    pub fn items(&self) -> impl Iterator<Item = &RemoteShoppingItem> {
        self.items.values()
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

fn parse_record(record: &serde_json::Map<String, serde_json::Value>) -> Result<RemoteShoppingItem> {
    let name = match record.get("name") {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => s.clone(),
        Some(other) => anyhow::bail!("`name` must be a non-empty string, got {other}"),
        None => anyhow::bail!("`name` is missing"),
    };
    let cat = match record.get("cat") {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => s.as_str(),
        Some(other) => anyhow::bail!("`cat` must be a non-empty string, got {other}"),
        None => anyhow::bail!("`cat` is missing"),
    };
    let count = match record.get("count") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(n)) => Some(
            n.as_f64()
                .ok_or_else(|| anyhow::anyhow!("`count` is not representable as a number: {n}"))?,
        ),
        Some(other) => anyhow::bail!("`count` must be a number, got {other}"),
    };
    Ok(RemoteShoppingItem {
        name,
        category: ShoppingCategory::parse(cat),
        count,
    })
}

/// A row of the local `shopping_item` table.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalShoppingItem {
    pub id: String,
    pub name: String,
    pub category: ShoppingCategory,
    pub count: Option<f64>,
    /// Local-only here: this endpoint carries no checked state. The phone API
    /// does, as membership of its `pickedItems` map, so the next lane fills
    /// this column from the wire.
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
}

/// A change the peer needs and that only the write leg can deliver.
///
/// Nothing sends these yet. They are computed regardless, because the
/// reconciler cannot decide the local side without them: a row the peer has
/// never carried is a pending addition, and deleting it on absence would
/// destroy a local edit rather than mirror a remote one.
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
        checked: false,
        product_id: None,
        deleted_at: None,
        last_seen_remote: Some(snapshot.fetched_at().to_string()),
    }
}

fn parse_timestamp(raw: &str, what: &str) -> Result<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(raw)
        .with_context(|| format!("{what} is not an RFC 3339 timestamp: '{raw}'"))
}
