//! postMessage bridge to the holon-worker Web Worker.
//!
//! `WorkerBridge` wraps the RPC call pattern and the snapshot subscription
//! mechanism. All communication is via `worker.postMessage` / `onmessage`.
//!
//! Protocol (mirrors `web/worker-entry.mjs`):
//! - **RPC**: send `{ id, kind, args: [] }` → receive `{ id, ok, value | error }`
//! - **Snapshots**: worker sends `{ kind: 'snapshot', handle, snapshotJson }`
//!   whenever the reactive engine emits a new ViewModel for a watched block.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use futures::channel::oneshot;
use js_sys::{Array, Object, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, Worker, WorkerOptions, WorkerType};

type PendingMap = HashMap<u32, oneshot::Sender<Result<JsValue, String>>>;

/// Snapshot callback wrapped so we can clone the `Rc` out, drop the outer
/// map borrow, and only then invoke the user closure. Without this, a
/// callback that touches `snapshot_listeners` (e.g. registering another
/// listener, dropping a subscription, or setting a Dioxus signal that
/// synchronously mounts a component which registers a listener) panics
/// with `already borrowed: BorrowMutError`.
type SnapshotCallback = Rc<RefCell<dyn FnMut(String)>>;
type SnapshotMap = HashMap<u32, SnapshotCallback>;

struct BridgeInner {
    worker: Worker,
    next_id: Cell<u32>,
    pending: Rc<RefCell<PendingMap>>,
    snapshot_listeners: Rc<RefCell<SnapshotMap>>,
    // Kept alive so the callback isn't dropped.
    _onmessage: Closure<dyn Fn(MessageEvent)>,
}

/// Cheap-to-clone handle to the holon-worker Web Worker.
///
/// Backed by `Rc` — single-threaded WASM only.
#[derive(Clone)]
pub struct WorkerBridge(Rc<BridgeInner>);

impl WorkerBridge {
    /// Spawn the worker at `worker_url` and wait for it to emit `{ kind: 'ready' }`.
    pub async fn spawn(worker_url: &str) -> Result<Self, String> {
        let mut opts = WorkerOptions::new();
        opts.set_type(WorkerType::Module);
        let worker = Worker::new_with_options(worker_url, &opts)
            .map_err(|e| format!("Worker::new failed: {e:?}"))?;

        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), String>>();
        let ready_slot: Rc<RefCell<Option<_>>> = Rc::new(RefCell::new(Some(ready_tx)));
        let ready_clone = ready_slot.clone();

        let pending: Rc<RefCell<PendingMap>> = Rc::default();
        let snapshot_listeners: Rc<RefCell<SnapshotMap>> = Rc::default();
        let pending_c = pending.clone();
        let snapshot_c = snapshot_listeners.clone();

        let onmessage: Closure<dyn Fn(MessageEvent)> =
            Closure::wrap(Box::new(move |e: MessageEvent| {
                let data = e.data();
                let kind = Reflect::get(&data, &"kind".into())
                    .ok()
                    .and_then(|v| v.as_string());

                if kind.as_deref() == Some("ready") {
                    if let Some(tx) = ready_clone.borrow_mut().take() {
                        let _ = tx.send(Ok(()));
                    }
                    return;
                }

                if kind.as_deref() == Some("snapshot") {
                    let handle = Reflect::get(&data, &"handle".into())
                        .ok()
                        .and_then(|v| v.as_f64())
                        .map(|v| v as u32);
                    let json = Reflect::get(&data, &"snapshotJson".into())
                        .ok()
                        .and_then(|v| v.as_string());
                    if let (Some(h), Some(j)) = (handle, json) {
                        // Clone the Rc out before the user callback runs so
                        // the map borrow is released. See SnapshotCallback.
                        let cb: Option<SnapshotCallback> = snapshot_c.borrow().get(&h).cloned();
                        if let Some(cb) = cb {
                            cb.borrow_mut()(j);
                        }
                    }
                    return;
                }

                let id = Reflect::get(&data, &"id".into())
                    .ok()
                    .and_then(|v| v.as_f64())
                    .map(|v| v as u32);
                if let Some(id) = id {
                    if let Some(tx) = pending_c.borrow_mut().remove(&id) {
                        let ok = Reflect::get(&data, &"ok".into())
                            .ok()
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let _ = if ok {
                            let val = Reflect::get(&data, &"value".into())
                                .ok()
                                .unwrap_or(JsValue::UNDEFINED);
                            tx.send(Ok(val))
                        } else {
                            let err = Reflect::get(&data, &"error".into())
                                .ok()
                                .and_then(|v| v.as_string())
                                .unwrap_or_else(|| "unknown error".into());
                            tx.send(Err(err))
                        };
                    }
                }
            }));

        worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

        // Surface worker errors on the main page console. Without this,
        // panics inside wasm running in the worker are invisible to the
        // Dioxus app — the worker just silently stops processing.
        let onerror: Closure<dyn Fn(web_sys::Event)> =
            Closure::wrap(Box::new(move |e: web_sys::Event| {
                // ErrorEvent carries message/filename/lineno; fall back
                // to the generic Event if the cast fails.
                let msg = e
                    .dyn_ref::<web_sys::ErrorEvent>()
                    .map(|ee| ee.message())
                    .unwrap_or_else(|| format!("{:?}", e.type_()));
                tracing::error!("[worker error] {msg}");
            }));
        worker.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        onerror.forget(); // leak on purpose — lives for the app lifetime
        let onmessage_error: Closure<dyn Fn(MessageEvent)> =
            Closure::wrap(Box::new(move |e: MessageEvent| {
                tracing::error!("[worker messageerror] data={:?}", e.data().as_string());
            }));
        worker.set_onmessageerror(Some(onmessage_error.as_ref().unchecked_ref()));
        onmessage_error.forget();

        // Race the worker's `{kind: 'ready'}` signal against a timeout so
        // a broken worker (wasm 404, instantiation crash, missing import
        // stubs) surfaces as a BootState::Failed instead of hanging the
        // UI at "booting…" forever.
        const READY_TIMEOUT_MS: u32 = 10_000;
        let ready_result = futures::future::select(
            ready_rx,
            gloo_timers::future::TimeoutFuture::new(READY_TIMEOUT_MS),
        )
        .await;
        match ready_result {
            futures::future::Either::Left((rx_result, _)) => {
                rx_result.map_err(|_| "worker ready channel dropped".to_string())??
            }
            futures::future::Either::Right(_) => {
                return Err(format!(
                    "worker did not emit `ready` within {READY_TIMEOUT_MS}ms — \
                     likely failed to instantiate (check wasm URL, imports, \
                     or the worker console for link errors)"
                ));
            }
        }

        Ok(WorkerBridge(Rc::new(BridgeInner {
            worker,
            next_id: Cell::new(1),
            pending,
            snapshot_listeners,
            _onmessage: onmessage,
        })))
    }

    /// Send an RPC call to the worker and await the response value.
    pub async fn call(
        &self,
        kind: &str,
        args: impl IntoIterator<Item = JsValue>,
    ) -> Result<JsValue, String> {
        let id = self.0.next_id.get();
        self.0.next_id.set(id + 1);

        let (tx, rx) = oneshot::channel();
        self.0.pending.borrow_mut().insert(id, tx);

        let msg = Object::new();
        Reflect::set(&msg, &"id".into(), &(id as f64).into()).unwrap();
        Reflect::set(&msg, &"kind".into(), &kind.into()).unwrap();
        let arr = Array::new();
        for v in args {
            arr.push(&v);
        }
        Reflect::set(&msg, &"args".into(), &arr).unwrap();

        if let Err(e) = self.0.worker.post_message(&msg) {
            self.0.pending.borrow_mut().remove(&id);
            return Err(format!("postMessage failed: {e:?}"));
        }

        rx.await.map_err(|_| "RPC channel closed".to_string())?
    }

    /// Register a snapshot callback for a subscription handle.
    ///
    /// The callback fires with the JSON-serialized `ViewModel` on every change.
    pub fn on_snapshot(&self, handle: u32, cb: impl FnMut(String) + 'static) {
        self.0
            .snapshot_listeners
            .borrow_mut()
            .insert(handle, Rc::new(RefCell::new(cb)));
    }

    /// Remove the snapshot callback and abort the worker subscription.
    pub async fn drop_subscription(&self, handle: u32) -> Result<(), String> {
        self.0.snapshot_listeners.borrow_mut().remove(&handle);
        self.call("engineDropSubscription", [JsValue::from_f64(handle as f64)])
            .await?;
        Ok(())
    }
}
