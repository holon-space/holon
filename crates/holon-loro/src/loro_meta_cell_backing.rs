//! [`CellBacking<T>`] for a block's scalar fields, backed by the tree node's
//! `meta` map (H3 nested per-property `LoroMap`).
//!
//! Sibling of [`LoroTextCellBacking`](crate::loro_text_cell_backing): where
//! that one owns `block.content` (rich text) this one owns every scalar field
//! (`completed`, `collapsed`, `block_type`, arbitrary properties, …). Read is
//! a typed decode of the stored `Value`; write goes through
//! [`LoroBackend::update_block_fields`] so it touches ONLY the changed key
//! (per-key LWW, H3 — a whole-map re-stamp would resurrect concurrently
//! clobbered keys) and lands the same `updated_at` bump + change event a
//! non-cell scalar write did. The signal observes Loro commits to the node's
//! per-property map and re-reads the field.

use std::marker::PhantomData;
use std::sync::Arc;

use anyhow::Result;
use anyhow::anyhow;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use holon_api::Value;
use holon_core::cell::CellBacking;
use loro::ContainerTrait;
use loro::LoroDoc;
use loro::LoroMap;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::loro_backend::LoroBackend;
use crate::loro_backend::properties_map_container;
use crate::loro_backend::read_scalar_field_from_meta;

/// The scalar Rust types a block field cell can present. Bridges between the
/// typed `Cell<T>` surface and the `Value` a property is stored as. `Value`
/// itself is supported so `write_field` can route an already-`Value`-typed
/// write through one cell path without re-dispatching on the variant.
pub trait LoroScalarField: Clone + Send + Sync + 'static {
    /// Decode the stored property (`None` when the key is absent) into `T`.
    /// A present value of the wrong shape is corruption/caller error — fail
    /// loud.
    fn decode(stored: Option<Value>) -> Result<Self>;
    /// Encode `self` into the `Value` persisted under the property key.
    /// `Value::REMOVED` deletes the key (mirrors `update_block_fields`).
    fn encode(self) -> Value;
}

impl LoroScalarField for bool {
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

impl LoroScalarField for i64 {
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

impl LoroScalarField for String {
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

impl LoroScalarField for Value {
    fn decode(stored: Option<Value>) -> Result<Self> {
        Ok(stored.unwrap_or(Value::Null))
    }
    fn encode(self) -> Value {
        self
    }
}

pub struct LoroMetaCellBacking<T: LoroScalarField> {
    backend: Arc<LoroBackend>,
    /// The tree node's `meta` map — the read root. Cloneable handle onto the
    /// same container the write path mutates, so reads see committed writes.
    meta: LoroMap,
    block_id: String,
    field: String,
    change_tx: broadcast::Sender<()>,
    #[allow(dead_code)]
    subscription: loro::Subscription,
    _marker: PhantomData<T>,
}

impl<T: LoroScalarField> LoroMetaCellBacking<T> {
    /// Wrap `(block_id, field)` on the node's `meta` map into a scalar cell.
    /// Subscribes to the per-property map so peer/authority writes to this
    /// field surface on the signal stream. `meta` must be the resolved tree
    /// node's meta map (see `BlockCellRegistry::resolve_node_meta`).
    pub fn new(
        doc: Arc<LoroDoc>,
        backend: Arc<LoroBackend>,
        meta: LoroMap,
        block_id: String,
        field: String,
    ) -> Result<Self> {
        // The per-property map is where every scalar lives; subscribe to it so
        // a change to any property fires the tick (we re-read our own key).
        let props_map = properties_map_container(&meta)?;
        let target = props_map.id();
        let (change_tx, _) = broadcast::channel(256);
        let tx_cb = change_tx.clone();
        let target_cb = target.clone();
        let subscription = doc.subscribe(
            &target,
            Arc::new(move |event| {
                for diff in &event.events {
                    if diff.target.clone() == target_cb {
                        let _ = tx_cb.send(());
                        break;
                    }
                }
            }),
        );
        Ok(Self {
            backend,
            meta,
            block_id,
            field,
            change_tx,
            subscription,
            _marker: PhantomData,
        })
    }

    fn read(&self) -> T {
        let stored = read_scalar_field_from_meta(&self.meta, &self.field);
        T::decode(stored).unwrap_or_else(|e| {
            panic!(
                "LoroMetaCellBacking::read({}, {}): {e:#}",
                self.block_id, self.field
            )
        })
    }
}

impl<T: LoroScalarField> CellBacking<T> for LoroMetaCellBacking<T> {
    fn current(&self) -> T {
        self.read()
    }

    fn signal(&self) -> BoxStream<'static, T> {
        let rx = self.change_tx.subscribe();
        let meta = self.meta.clone();
        let field = self.field.clone();
        let block_id = self.block_id.clone();
        let initial = self.read();
        let tail = BroadcastStream::new(rx).filter_map(move |r| {
            let meta = meta.clone();
            let field = field.clone();
            let block_id = block_id.clone();
            async move {
                match r {
                    Ok(()) => Some(
                        T::decode(read_scalar_field_from_meta(&meta, &field)).unwrap_or_else(|e| {
                            panic!("LoroMetaCellBacking::signal({block_id}, {field}): {e:#}")
                        }),
                    ),
                    Err(_) => {
                        tracing::warn!(
                            "LoroMetaCellBacking signal lagged ({block_id}, {field}); consumer \
                             should call current() and resync"
                        );
                        None
                    }
                }
            }
        });
        Box::pin(futures::stream::once(async move { initial }).chain(tail))
    }

    fn apply_replace(&self, v: T) -> BoxFuture<'static, Result<()>> {
        let backend = self.backend.clone();
        let id = self.block_id.clone();
        let field = self.field.clone();
        let value = v.encode();
        Box::pin(async move {
            backend
                .update_block_fields(&id, &[(field.clone(), Value::Null, value)])
                .await
                .map_err(|e| anyhow!("LoroMetaCellBacking write ({id}, {field}): {e:#}"))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loro_backend::STABLE_ID;
    use crate::loro_backend::TREE_NAME;

    fn doc_with_block(block_id: &str) -> (Arc<LoroDoc>, Arc<LoroBackend>) {
        let doc = Arc::new(LoroDoc::new());
        doc.set_peer_id(1).unwrap();
        let tree = doc.get_tree(TREE_NAME);
        tree.enable_fractional_index(0);
        let node = tree.create(None).unwrap();
        let meta = tree.get_meta(node).unwrap();
        meta.insert(STABLE_ID, block_id.to_string()).unwrap();
        doc.commit();
        let loro_doc = crate::loro_document::LoroDocument::from_existing(doc.clone(), "test");
        let backend = Arc::new(LoroBackend::from_document(Arc::new(loro_doc)));
        (doc, backend)
    }

    fn node_meta(doc: &Arc<LoroDoc>, block_id: &str) -> LoroMap {
        let tree = doc.get_tree(TREE_NAME);
        for node in tree.get_nodes(false) {
            let meta = tree.get_meta(node.id).unwrap();
            let matches = matches!(
                meta.get(STABLE_ID),
                Some(loro::ValueOrContainer::Value(v)) if v.as_string().is_some_and(|s| s.to_string() == block_id)
            );
            if matches {
                return meta;
            }
        }
        panic!("block {block_id} not found");
    }

    #[tokio::test]
    async fn bool_scalar_round_trip_and_cross_cell_signal() -> Result<()> {
        let (doc, backend) = doc_with_block("abc");
        let meta = node_meta(&doc, "abc");
        let writer = LoroMetaCellBacking::<bool>::new(
            doc.clone(),
            backend.clone(),
            meta.clone(),
            "block:abc".into(),
            "completed".into(),
        )?;
        // A second, independent cell on the same (uri, field) must observe the
        // first cell's write — cross-consumer coherence (invariant 12 surface).
        let observer = LoroMetaCellBacking::<bool>::new(
            doc.clone(),
            backend.clone(),
            meta.clone(),
            "block:abc".into(),
            "completed".into(),
        )?;

        assert!(!writer.current(), "absent property decodes to false");
        assert!(!observer.current());

        let mut obs_signal = observer.signal();
        // Drain the initial emission.
        assert_eq!(obs_signal.next().await, Some(false));

        writer.apply_replace(true).await?;

        assert!(writer.current(), "write is visible on the writer");
        assert!(
            observer.current(),
            "write is visible on the observer's own handle"
        );
        assert_eq!(
            obs_signal.next().await,
            Some(true),
            "observer's signal saw the writer's commit"
        );
        Ok(())
    }

    #[tokio::test]
    async fn string_and_i64_scalars_round_trip() -> Result<()> {
        let (doc, backend) = doc_with_block("xyz");
        let meta = node_meta(&doc, "xyz");
        let s = LoroMetaCellBacking::<String>::new(
            doc.clone(),
            backend.clone(),
            meta.clone(),
            "block:xyz".into(),
            "block_type".into(),
        )?;
        s.apply_replace("todo".into()).await?;
        assert_eq!(s.current(), "todo");

        let n = LoroMetaCellBacking::<i64>::new(
            doc.clone(),
            backend.clone(),
            meta.clone(),
            "block:xyz".into(),
            "priority".into(),
        )?;
        n.apply_replace(7).await?;
        assert_eq!(n.current(), 7);
        // The string cell is unaffected by the i64 write (per-key storage).
        assert_eq!(s.current(), "todo");
        Ok(())
    }
}
