//! The SqlOnly-mode cell registry: the storage-agnostic twin of the Loro
//! `BlockCellRegistry`.
//!
//! SqlOnly mode owns no CRDT authority, so every authoritative seam on
//! [`EntityCellRegistry`] falls through to the trait's `not handled` default
//! and the caller runs its SQL path. What this registry DOES own is the
//! `Cell<T>` surface: reads come from the convergent `LiveData<Block>` entity
//! cache and writes route through the injected `set_field` path, so a caller
//! sees the same cell shape in both modes.

use std::any::Any;
use std::any::TypeId;
use std::sync::Arc;

use anyhow::Result;
use anyhow::anyhow;
use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::stream::StreamExt;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::live_data::LiveData;

use crate::cell::CellBacking;
use crate::cell::LwwScalarBacking;
use crate::cell::LwwTextCellBacking;
use crate::cell_registry::CellCache;
use crate::cell_registry::EntityCellRegistry;

/// Injected write path for SqlOnly cells: routes a `(uri, field, value)` scalar
/// write through the engine's `set_field` operation.
pub type SqlScalarWriteFn =
    Arc<dyn Fn(EntityUri, String, Value) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// Deps that make SqlOnly cells resolve to a live `LwwScalarBacking` /
/// `LwwTextCellBacking` instead of erroring: the convergent `LiveData<Block>`
/// entity cache (sync `read()` for `current()`, `signal_map()` for the CDC
/// signal) plus the SQL `set_field` write path. Injected via the DI seam so
/// this crate never names the engine-side `SqlOperationProvider`.
struct SqlCellWiring {
    live: Arc<LiveData<Block>>,
    write: SqlScalarWriteFn,
}

/// write through one cell path without re-dispatching on the variant.
pub trait ScalarField: Clone + Send + Sync + 'static {
    /// Decode the stored property (`None` when the key is absent) into `T`.
    /// A present value of the wrong shape is corruption/caller error — fail
    /// loud.
    fn decode(stored: Option<Value>) -> Result<Self>;
    /// Encode `self` into the `Value` persisted under the property key.
    /// `Value::Null` deletes the key (mirrors `update_block_fields`).
    fn encode(self) -> Value;
}

impl ScalarField for bool {
    fn decode(stored: Option<Value>) -> Result<Self> {
        match stored {
            None | Some(Value::Null) => Ok(false),
            Some(Value::Boolean(b)) => Ok(b),
            Some(other) => Err(anyhow!(
                "scalar field decode: expected Boolean, got {other:?}"
            )),
        }
    }
    fn encode(self) -> Value {
        Value::Boolean(self)
    }
}

impl ScalarField for i64 {
    fn decode(stored: Option<Value>) -> Result<Self> {
        match stored {
            None | Some(Value::Null) => Ok(0),
            Some(Value::Integer(i)) => Ok(i),
            Some(other) => Err(anyhow!(
                "scalar field decode: expected Integer, got {other:?}"
            )),
        }
    }
    fn encode(self) -> Value {
        Value::Integer(self)
    }
}

impl ScalarField for String {
    fn decode(stored: Option<Value>) -> Result<Self> {
        match stored {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(s)) => Ok(s),
            Some(other) => Err(anyhow!(
                "scalar field decode: expected String, got {other:?}"
            )),
        }
    }
    fn encode(self) -> Value {
        Value::String(self)
    }
}

impl ScalarField for Value {
    fn decode(stored: Option<Value>) -> Result<Self> {
        Ok(stored.unwrap_or(Value::Null))
    }
    fn encode(self) -> Value {
        self
    }
}

/// SqlOnly-mode [`EntityCellRegistry`].
///
/// `wiring` is `Some` once the composition root injects the entity-cache read +
/// `set_field` write seam ([`Self::wired`]); `None` for non-DI / synthetic-test
/// construction ([`Self::new`]), where any `live_field_any` call errors loudly
/// rather than handing back a cell that silently reads nothing.
pub struct SqlOnlyCellRegistry {
    cache: CellCache,
    wiring: Option<SqlCellWiring>,
}

impl Default for SqlOnlyCellRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SqlOnlyCellRegistry {
    /// Construct with no injected cell seam. Any `live_field_any` call errors
    /// loudly: there is no editor in this configuration and synthetic test
    /// stores have no entity cache to read.
    pub fn new() -> Self {
        Self {
            cache: CellCache::new(),
            wiring: None,
        }
    }

    /// Construct wired to the convergent `LiveData<Block>` entity cache (read +
    /// CDC signal) and the SQL `set_field` write path.
    pub fn wired(live: Arc<LiveData<Block>>, write: SqlScalarWriteFn) -> Self {
        Self {
            cache: CellCache::new(),
            wiring: Some(SqlCellWiring { live, write }),
        }
    }
}

#[async_trait]
impl EntityCellRegistry for SqlOnlyCellRegistry {
    fn live_field_any(
        &self,
        uri: &EntityUri,
        field: &str,
        type_id: TypeId,
    ) -> Result<Arc<dyn Any + Send + Sync>> {
        let Some(wiring) = self.wiring.as_ref() else {
            return Err(anyhow!(
                "SqlOnlyCellRegistry::live_field_any: no cell seam is wired for field {field:?}. \
                 Construct with `SqlOnlyCellRegistry::wired` at the composition root."
            ));
        };
        // `content` is the block's text: it exists only as `Cell<String>`. Asking
        // for any other `T` is a caller bug, and answering it from the generic
        // scalar builder would hand back a property-backed cell that merely
        // shares the name — silently wrong. Mirrors the Loro twin's guard.
        if field == "content" && type_id != TypeId::of::<String>() {
            return Err(anyhow!(
                "SqlOnlyCellRegistry::live_field_any: field \"content\" requires T=String \
                 (caller asked for a different type)"
            ));
        }
        if field == "content" {
            return self.cache.get_or_construct::<String, _>(uri, field, || {
                Ok(build_sql_content_cell(wiring, uri))
            });
        }
        build_sql_scalar_cell(&self.cache, wiring, uri, field, type_id)
    }

    fn on_entity_deleted(&self, uri: &EntityUri) {
        self.cache.evict_uri(uri);
    }
}

/// Build the wired `content` cell: an LWW `Cell<String>` reading the block's
/// text from the entity cache and writing via `set_field("content")`.
fn build_sql_content_cell(wiring: &SqlCellWiring, uri: &EntityUri) -> Arc<dyn CellBacking<String>> {
    let live = wiring.live.clone();
    let key = uri.to_string();

    let read_live = live.clone();
    let read_key = key.clone();
    let read = Arc::new(move || -> String {
        read_live
            .read()
            .get(&read_key)
            .map(|b| b.content_text().to_string())
            .unwrap_or_default()
    });

    let write_fn = wiring.write.clone();
    let write_uri = uri.clone();
    let write = Arc::new(move |v: String| {
        let write_fn = write_fn.clone();
        let uri = write_uri.clone();
        Box::pin(async move { (write_fn)(uri, "content".to_string(), Value::String(v)).await })
            as BoxFuture<'static, Result<()>>
    });

    let sig_live = live;
    let sig_key = key;
    let signal_factory = Arc::new(move || -> BoxStream<'static, String> {
        use futures_signals::signal::SignalExt;
        use futures_signals::signal_map::SignalMapExt;
        Box::pin(
            sig_live
                .signal_map()
                .key_cloned(sig_key.clone())
                .to_stream()
                .map(|opt: Option<Arc<Block>>| {
                    opt.map(|b| b.content_text().to_string())
                        .unwrap_or_default()
                }),
        )
    });

    Arc::new(LwwTextCellBacking::new(read, write, signal_factory)) as Arc<dyn CellBacking<String>>
}

/// Dispatch a wired scalar `live_field` to a typed `LwwScalarBacking`. The type
/// set mirrors the Loro twin's `live_field_any` dispatch so the two mode
/// surfaces are symmetric: `T \u{2208} {bool, i64, String, Value}`.
fn build_sql_scalar_cell(
    cache: &CellCache,
    wiring: &SqlCellWiring,
    uri: &EntityUri,
    field: &str,
    type_id: TypeId,
) -> Result<Arc<dyn Any + Send + Sync>> {
    if type_id == TypeId::of::<bool>() {
        cache.get_or_construct::<bool, _>(uri, field, || {
            Ok(sql_scalar_backing::<bool>(wiring, uri, field))
        })
    } else if type_id == TypeId::of::<i64>() {
        cache.get_or_construct::<i64, _>(uri, field, || {
            Ok(sql_scalar_backing::<i64>(wiring, uri, field))
        })
    } else if type_id == TypeId::of::<String>() {
        cache.get_or_construct::<String, _>(uri, field, || {
            Ok(sql_scalar_backing::<String>(wiring, uri, field))
        })
    } else if type_id == TypeId::of::<Value>() {
        cache.get_or_construct::<Value, _>(uri, field, || {
            Ok(sql_scalar_backing::<Value>(wiring, uri, field))
        })
    } else {
        Err(anyhow!(
            "SqlOnlyCellRegistry::live_field_any: scalar field {field:?} has no cell for the \
             requested type (supported: bool, i64, String, Value)"
        ))
    }
}

/// One typed wired scalar backing. `read`/signal decode the field from the
/// entity cache (a present-but-wrong-shape value is corruption -> panic);
/// `write` encodes and routes through the injected `set_field` path.
fn sql_scalar_backing<T: ScalarField>(
    wiring: &SqlCellWiring,
    uri: &EntityUri,
    field: &str,
) -> Arc<dyn CellBacking<T>> {
    let live = wiring.live.clone();
    let key = uri.to_string();

    let read_live = live.clone();
    let read_key = key.clone();
    let read_field = field.to_string();
    let read = Arc::new(move || -> T {
        let snap = read_live.read();
        let stored = snap
            .get(&read_key)
            .and_then(|b| b.get_property(&read_field));
        T::decode(stored)
            .unwrap_or_else(|e| panic!("SqlOnly scalar read ({read_key}, {read_field}): {e:#}"))
    });

    let write_fn = wiring.write.clone();
    let write_uri = uri.clone();
    let write_field = field.to_string();
    let write = Arc::new(move |v: T| {
        let write_fn = write_fn.clone();
        let uri = write_uri.clone();
        let field = write_field.clone();
        let value = v.encode();
        Box::pin(async move { (write_fn)(uri, field, value).await })
            as BoxFuture<'static, Result<()>>
    });

    let sig_live = live;
    let sig_key = key;
    let sig_field = field.to_string();
    let signal_factory = Arc::new(move || -> BoxStream<'static, T> {
        use futures_signals::signal::SignalExt;
        use futures_signals::signal_map::SignalMapExt;
        let field = sig_field.clone();
        Box::pin(
            sig_live
                .signal_map()
                .key_cloned(sig_key.clone())
                .to_stream()
                .map(move |opt: Option<Arc<Block>>| {
                    let stored = opt.and_then(|b| b.get_property(&field));
                    T::decode(stored)
                        .unwrap_or_else(|e| panic!("SqlOnly scalar signal ({field}): {e:#}"))
                }),
        )
    });

    Arc::new(LwwScalarBacking::<T>::new(read, write, signal_factory)) as Arc<dyn CellBacking<T>>
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;
    use crate::cell::Cell;
    use crate::cell_registry::EntityCellRegistryExt;

    /// `content` is text and exists only as `Cell<String>`. Asking for another
    /// `T` must fail loudly rather than hand back a property-backed cell that
    /// merely shares the name. The Loro twin has always rejected this; this is
    /// the SqlOnly side of the same contract.
    #[test]
    fn content_with_a_non_string_type_errs_loudly() {
        let live: Arc<LiveData<Block>> = LiveData::new(
            Vec::new(),
            |row: &holon_api::StorageEntity| {
                row.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow!("row missing id"))
            },
            |row: &holon_api::StorageEntity| Block::try_from(row.clone()),
        );
        let write: SqlScalarWriteFn = Arc::new(move |_, _, _| {
            Box::pin(async move { Ok(()) }) as BoxFuture<'static, Result<()>>
        });
        let registry: Box<dyn EntityCellRegistry> =
            Box::new(SqlOnlyCellRegistry::wired(live, write));
        let uri = EntityUri::block("abc");

        let err = registry
            .as_ref()
            .live_field::<bool>(&uri, "content")
            .err()
            .expect("content as T=bool must be refused, not silently property-backed");
        let msg = format!("{err:#}");
        assert!(msg.contains("requires T=String"), "msg = {msg}");

        registry
            .as_ref()
            .live_field::<String>(&uri, "content")
            .expect("content as T=String is the supported shape");
    }

    #[test]
    fn unwired_registry_errs_loudly() {
        let registry: Box<dyn EntityCellRegistry> = Box::new(SqlOnlyCellRegistry::new());
        let uri = EntityUri::block("abc");
        let res = registry.as_ref().live_field::<String>(&uri, "content");
        assert!(
            res.is_err(),
            "an unwired registry must not hand back a cell"
        );
    }

    /// Spec 0008 §2.2: wired SqlOnly mode presents the same scalar cell surface
    /// as Full (Loro) mode. The write callback emulates `set_field` → CDC by
    /// updating the entity cache, proving `live_field::<bool>` round-trips a
    /// write and observes it via the cell — no Loro doc involved.
    #[tokio::test]
    async fn sql_only_wired_scalar_round_trips_via_entity_cache() -> Result<()> {
        use holon_api::StorageEntity;
        use holon_api::block::Block;
        use holon_api::live_data::LiveData;

        let live: Arc<LiveData<Block>> = LiveData::new(
            Vec::new(),
            |row: &StorageEntity| {
                row.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow!("row missing id"))
            },
            |row: &StorageEntity| Block::try_from(row.clone()),
        );

        let uri = EntityUri::block("abc");
        let key = uri.to_string();
        let block = Block::new_text(uri.clone(), EntityUri::block("root"), "hello");
        live.insert(key.clone(), Arc::new(block));

        // set_field write path: emulate the SQL write + CDC reflection by
        // updating the entity cache with the encoded property.
        let live_for_write = live.clone();
        let write: SqlScalarWriteFn =
            Arc::new(move |uri: EntityUri, field: String, value: Value| {
                let live = live_for_write.clone();
                Box::pin(async move {
                    let key = uri.to_string();
                    let mut b = live
                        .read()
                        .get(&key)
                        .map(|b| (**b).clone())
                        .ok_or_else(|| anyhow!("block {key} absent from entity cache"))?;
                    b.set_property(field, value);
                    live.insert(key, Arc::new(b));
                    Ok(())
                }) as BoxFuture<'static, Result<()>>
            });

        let registry = SqlOnlyCellRegistry::wired(live.clone(), write);
        let cell: Cell<bool> =
            (&registry as &dyn EntityCellRegistry).live_field::<bool>(&uri, "completed")?;
        assert!(!cell.current(), "absent property decodes to false");
        cell.set(true).await?;
        assert!(
            cell.current(),
            "the write is visible through the cell via the entity cache"
        );
        Ok(())
    }
}
