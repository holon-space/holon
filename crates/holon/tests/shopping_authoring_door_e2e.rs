//! The authoring door of a synced-peer type: a person's `create` and `delete`
//! arriving through the generic operation surface. Writes go through
//! [`E2ETestContext::execute_op`], the production door that stamps
//! `_provenance` and routes by the declared type's own authority.
//!
//! - `docs/Testing/bugfunnel/entries/
//!   2026-09-02-a-shopping-item-can-never-be-added-in-holon.md`
//! - `docs/Testing/bugfunnel/entries/
//!   2026-09-02-deleting-a-shopping-item-is-undone-by-the-next-sync.md`

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use holon::testing::e2e_test_helpers::E2ETestContext;
use holon_api::QueryLanguage;
use holon_api::Value;
use holon_core::storage::types::StorageEntity;
use holon_kitchen::shopping::CompleteSnapshot;
use holon_kitchen::shopping::LocalShoppingItem;
use holon_kitchen::shopping::ShoppingCategory;
use holon_kitchen::shopping::ShoppingReconciler;
use holon_kitchen::shopping_sync::CommitAck;
use holon_kitchen::shopping_sync::CommitBatch;
use holon_kitchen::shopping_sync::ShoppingPeer;
use holon_kitchen::shopping_sync::ShoppingRowReader;
use holon_kitchen::shopping_sync::SyncOutcome;
use holon_kitchen::shopping_sync::local_intent_operation;
use holon_kitchen::shopping_sync::sync_once;
use holon_rows::RowMapper;

/// The shipped sidecar. The fake peer answers a pull the way production reads
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

const TABLE: &str = "shopping_item_raw";
const DEVICE_ID: &str = "device-under-test";
/// The vocabulary the fake list publishes.
const CATS: &[&str] = &["Trocken", "Fleisch", "R"];

fn params(pairs: &[(&str, Value)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| (Arc::from(*k), v.clone()))
        .collect()
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

// A fake peer: a stateful list, in process, without the transport the HTTP mock
// in `holon-app/tests/shopping_pull_mock.rs` carries.

#[derive(Default)]
struct PeerState {
    /// `(name, cat)`, the active list.
    items: Vec<(String, String)>,
    version: i64,
    /// Every command the peer was told about, in order.
    received: Vec<(String, String)>,
}

struct FakePeer {
    state: Arc<Mutex<PeerState>>,
}

impl FakePeer {
    fn seeded(items: &[(&str, &str)]) -> Self {
        Self {
            state: Arc::new(Mutex::new(PeerState {
                items: items
                    .iter()
                    .map(|(n, c)| (n.to_string(), c.to_string()))
                    .collect(),
                version: 7,
                received: Vec::new(),
            })),
        }
    }

    /// `(verb, name)` for every command this peer has been sent.
    fn commands(&self) -> Vec<(String, String)> {
        self.state.lock().expect("peer state").received.clone()
    }

    fn lists(&self, name: &str) -> bool {
        self.state
            .lock()
            .expect("peer state")
            .items
            .iter()
            .any(|(n, _)| n == name)
    }
}

#[async_trait]
impl ShoppingPeer for FakePeer {
    async fn pull(&self) -> Result<CompleteSnapshot> {
        let state = self.state.lock().expect("peer state");
        let items: Vec<serde_json::Value> = state
            .items
            .iter()
            .map(|(name, cat)| serde_json::json!({"name": name, "cat": cat}))
            .collect();
        let body = serde_json::json!({
            "items": items,
            "pickedItems": {},
            "version": state.version,
            "options": {"prices": false, "cats": CATS},
        });
        snapshot_from_body(
            body.as_object().expect("the fake body is an object"),
            &now_rfc3339(),
        )
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<CommitAck> {
        let mut state = self.state.lock().expect("peer state");
        let stream = batch.to_row_stream();
        let rows = stream["rows"]
            .as_array()
            .expect("the row stream carries rows");
        for entry in rows.iter().filter(|e| e["type"] == "shopping_command") {
            let row = &entry["row"];
            let field = |key: &str| {
                row[key]
                    .as_str()
                    .unwrap_or_else(|| panic!("a command row carries `{key}`"))
                    .to_string()
            };
            let (verb, name, cat) = (field("verb"), field("name"), field("cat"));
            state.received.push((verb.clone(), name.clone()));
            match verb.as_str() {
                // Intent words: the batch reaches this mock before the
                // sidecar mapping that spells `remove` as this peer's `del`.
                "add" => state.items.push((name, cat)),
                "remove" => state.items.retain(|(n, c)| !(n == &name && c == &cat)),
                other => anyhow::bail!("the fake peer received an unknown command '{other}'"),
            }
        }
        state.version += 1;
        Ok(CommitAck {
            version: state.version,
            picked_items_version: state.version,
        })
    }
}

/// Reads the local rows the reconciler decides against, straight off the raw
/// write table — the same read `ShoppingOperations::Rows` performs in prod.
struct Rows<'a> {
    ctx: &'a E2ETestContext,
}

#[async_trait]
impl ShoppingRowReader for Rows<'_> {
    async fn load(&self) -> Result<Vec<LocalShoppingItem>> {
        let rows = self
            .ctx
            .query(
                format!(
                    "SELECT id, name, cat, count, checked, product_id, deleted_at, \
                     last_seen_remote FROM {TABLE}"
                ),
                QueryLanguage::HolonSql,
                HashMap::new(),
            )
            .await?;
        rows.iter()
            .map(|row| {
                let text = |column: &str| {
                    row.get(column)
                        .and_then(Value::as_string)
                        .map(str::to_string)
                };
                let category = ShoppingCategory::unresolved(
                    &text("cat").ok_or_else(|| anyhow::anyhow!("a row carries no `cat`"))?,
                );
                Ok(LocalShoppingItem {
                    id: text("id").ok_or_else(|| anyhow::anyhow!("a row carries no `id`"))?,
                    name: text("name").ok_or_else(|| anyhow::anyhow!("a row carries no `name`"))?,
                    category,
                    count: row.get("count").and_then(number),
                    checked: row.get("checked").and_then(number).unwrap_or(0.0) != 0.0,
                    product_id: text("product_id"),
                    deleted_at: text("deleted_at"),
                    last_seen_remote: text("last_seen_remote"),
                })
            })
            .collect()
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(f) => Some(*f),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

/// Every local row as `(name, deleted_at)`, sorted — the raw table, so a
/// tombstoned row is still visible here.
async fn stored_rows(ctx: &E2ETestContext) -> Result<Vec<(String, Option<String>)>> {
    let rows = ctx
        .query(
            format!("SELECT name, deleted_at FROM {TABLE}"),
            QueryLanguage::HolonSql,
            HashMap::new(),
        )
        .await?;
    let mut out: Vec<(String, Option<String>)> = rows
        .iter()
        .map(|row| {
            (
                row.get("name")
                    .and_then(Value::as_string)
                    .unwrap_or_default()
                    .to_string(),
                row.get("deleted_at")
                    .and_then(Value::as_string)
                    .map(str::to_string),
            )
        })
        .collect();
    out.sort();
    Ok(out)
}

/// The stored id of the row named `name`. Ids are minted with the entity
/// prefix, so a delete must name the STORED id, not the one a create supplied.
async fn stored_id(ctx: &E2ETestContext, name: &str) -> Result<String> {
    let rows = ctx
        .query(
            format!("SELECT id, name FROM {TABLE}"),
            QueryLanguage::HolonSql,
            HashMap::new(),
        )
        .await?;
    rows.iter()
        .find(|row| row.get("name").and_then(Value::as_string) == Some(name))
        .and_then(|row| row.get("id").and_then(Value::as_string))
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("no local shopping row is named '{name}'"))
}

/// Run one round and apply every local write it decided, through the same
/// generic authority production uses for the follow-ups.
async fn sync_round(
    ctx: &E2ETestContext,
    peer: &FakePeer,
    reconciler: &ShoppingReconciler,
) -> Result<SyncOutcome> {
    let rows = Rows { ctx };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let outcome = sync_once(peer, &rows, reconciler, DEVICE_ID, now_ms).await?;
    for intent in &outcome.local {
        let operation = local_intent_operation(intent);
        ctx.execute_op(
            operation.entity_name.as_str(),
            &operation.op_name,
            operation
                .params
                .iter()
                .map(|(k, v)| (Arc::from(k.as_str()), v.clone()))
                .collect(),
        )
        .await?;
    }
    Ok(outcome)
}

/// Entry `docs/Testing/bugfunnel/entries/
/// 2026-09-02-a-shopping-item-can-never-be-added-in-holon.md`: a person adds an
/// item through the operation surface.
#[tokio::test(flavor = "multi_thread")]
async fn adding_a_shopping_item_through_the_generic_surface_stores_it() -> Result<()> {
    let ctx = E2ETestContext::new().await?;

    ctx.execute_op(
        "shopping_item",
        "create",
        params(&[
            (
                "id",
                Value::String("shopping:Fleisch:Guanciale".to_string()),
            ),
            ("name", Value::String("Guanciale".to_string())),
            ("cat", Value::String("Fleisch".to_string())),
            ("count", Value::Float(1.0)),
            ("checked", Value::Integer(0)),
        ]),
    )
    .await?;

    assert_eq!(
        stored_rows(&ctx).await?,
        vec![("Guanciale".to_string(), None)],
        "the added item must be on the list, un-tombstoned"
    );
    Ok(())
}

/// Entry `docs/Testing/bugfunnel/entries/
/// 2026-09-02-deleting-a-shopping-item-is-undone-by-the-next-sync.md`: a
/// deletion survives the next pull and reaches the peer.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_shopping_item_is_pushed_to_the_peer_and_does_not_come_back() -> Result<()> {
    let ctx = E2ETestContext::new().await?;
    let peer = FakePeer::seeded(&[("Milch", "R"), ("Spaghetti", "Trocken")]);
    let reconciler = ShoppingReconciler::default();

    sync_round(&ctx, &peer, &reconciler).await?;
    assert_eq!(
        stored_rows(&ctx).await?,
        vec![("Milch".to_string(), None), ("Spaghetti".to_string(), None),],
        "the pull leg brings both remote items in"
    );

    let spaghetti = stored_id(&ctx, "Spaghetti").await?;
    ctx.execute_op(
        "shopping_item",
        "delete",
        params(&[("id", Value::String(spaghetti))]),
    )
    .await?;

    let after_delete = stored_rows(&ctx).await?;
    assert_eq!(
        after_delete.len(),
        2,
        "a soft-deleted row stays on the write table until the peer has been told; rows: \
         {after_delete:?}"
    );
    let tombstone = after_delete
        .iter()
        .find(|(name, _)| name == "Spaghetti")
        .map(|(_, deleted_at)| deleted_at.clone())
        .expect("the deleted row is still on the write table");
    assert!(
        tombstone.is_some(),
        "the delete must WRITE the `deleted_at` tombstone the sync leg pushes from"
    );

    sync_round(&ctx, &peer, &reconciler).await?;
    assert_eq!(
        peer.commands(),
        vec![("remove".to_string(), "Spaghetti".to_string())],
        "the deletion must reach the peer as a `del` command"
    );
    assert!(
        !peer.lists("Spaghetti"),
        "the peer must no longer carry the deleted item"
    );
    assert_eq!(
        stored_rows(&ctx).await?,
        vec![("Milch".to_string(), None)],
        "the tombstone is reaped once the peer has confirmed the deletion, and the item does not \
         come back"
    );
    Ok(())
}
