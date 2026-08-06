//! The doc-boundary lock: the mechanism that makes every observer of a
//! `LoroDoc` see only COMMIT-BOUNDARY states.
//!
//! Loro applies each local op under its own acquisition of the internal state
//! lock, so between any two ops of a write batch a concurrent reader, exporter
//! or saver can observe the batch interior (a tree node created but not yet
//! carrying its `stable_id`, text whose marks have been cleared but not yet
//! re-applied). This lock closes that window at the API boundary: writers hold
//! it across the whole closure *through* `commit()`, readers and exporters hold
//! it shared.
//!
//! The lock is keyed by the `Arc<LoroDoc>` identity, not by the `LoroDocument`
//! wrapper — several wrappers over one inner doc
//! (`LoroDocument::from_existing`, `LoroBackend::target_doc`) must share one
//! lock or the seal is decorative.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::Weak;
use std::time::Duration;

use anyhow::Result;
use anyhow::bail;
use loro::LoroDoc;
use parking_lot::RwLock;

/// A lock wait longer than this is a bug, not contention: writes are
/// human-scale and short. Report it instead of hanging forever.
const LOCK_WAIT_BUDGET: Duration = Duration::from_secs(30);

/// Identity of the inner doc a lock guards. Stable for the doc's lifetime.
type DocKey = usize;

pub(crate) struct DocLock {
    key: DocKey,
    lock: Arc<RwLock<()>>,
}

impl DocLock {
    /// The lock for `doc`, creating it on first sight. Any two `LoroDocument`s
    /// wrapping the same `Arc<LoroDoc>` receive the same lock.
    pub(crate) fn for_doc(doc: &Arc<LoroDoc>) -> Self {
        static REGISTRY: OnceLock<Mutex<HashMap<DocKey, (Weak<LoroDoc>, Arc<RwLock<()>>)>>> =
            OnceLock::new();
        let key = Arc::as_ptr(doc) as DocKey;
        let mut map = REGISTRY
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .expect("doc-lock registry poisoned");
        // A live `Arc` pins its address, so an entry under this key whose
        // `Weak` is dead belonged to a freed doc that happened to sit at the
        // same address — dropping it cannot steal a lock still in use.
        if map.len() > 64 {
            map.retain(|_, (weak, _)| weak.strong_count() > 0);
        }
        if map.get(&key).is_some_and(|(w, _)| w.strong_count() == 0) {
            map.remove(&key);
        }
        let lock = map
            .entry(key)
            .or_insert_with(|| (Arc::downgrade(doc), Arc::new(RwLock::new(()))))
            .1
            .clone();
        Self { key, lock }
    }
}

/// What this thread already holds for a given doc. Read/write nesting through
/// helper functions is legitimate — the outer holder already has the access the
/// inner call is asking for — but an outer READ that tries to become a WRITE is
/// an unsatisfiable upgrade and must be reported, not deadlocked on.
#[derive(Default, Clone, Copy)]
struct Held {
    reads: u32,
    writes: u32,
}

thread_local! {
    static HELD: std::cell::RefCell<HashMap<DocKey, Held>> =
        std::cell::RefCell::new(HashMap::new());
}

fn held(key: DocKey) -> Held {
    HELD.with(|h| h.borrow().get(&key).copied().unwrap_or_default())
}

fn enter(key: DocKey, write: bool) {
    HELD.with(|h| {
        let mut m = h.borrow_mut();
        let e = m.entry(key).or_default();
        if write { e.writes += 1 } else { e.reads += 1 }
    });
}

fn leave(key: DocKey, write: bool) {
    HELD.with(|h| {
        let mut m = h.borrow_mut();
        let e = m.get_mut(&key).expect("doc-lock depth underflow");
        if write {
            e.writes -= 1
        } else {
            e.reads -= 1
        }
        if e.reads == 0 && e.writes == 0 {
            m.remove(&key);
        }
    });
}

/// Restores this thread's nesting depth even if the guarded closure panics.
struct DepthGuard {
    key: DocKey,
    write: bool,
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        leave(self.key, self.write);
    }
}

impl DocLock {
    /// Run `f` with exclusive access to the doc.
    ///
    /// Reentrant writes pass through: the caller already holds exclusive
    /// access, and re-acquiring a non-reentrant lock would deadlock.
    pub(crate) fn write<R>(&self, doc_id: &str, f: impl FnOnce() -> Result<R>) -> Result<R> {
        let h = held(self.key);
        if h.writes > 0 {
            return f();
        }
        if h.reads > 0 {
            bail!(
                "doc '{doc_id}': a write was requested while this thread holds the doc read lock. \
                 A read guard cannot be upgraded; hoist the write out of the enclosing read."
            );
        }
        let Some(_guard) = self.lock.try_write_for(LOCK_WAIT_BUDGET) else {
            bail!(
                "doc '{doc_id}': timed out after {LOCK_WAIT_BUDGET:?} waiting for the doc write \
                 lock. Another thread is holding it — a doc-lock callback that re-enters the doc, \
                 or a writer blocked on I/O."
            );
        };
        enter(self.key, true);
        let _depth = DepthGuard {
            key: self.key,
            write: true,
        };
        f()
    }

    /// Run `f` with shared access to the doc. Passes through when this thread
    /// already holds the write lock, and takes a recursive read otherwise, so
    /// helper functions may read from inside either kind of guard.
    pub(crate) fn read<R>(&self, doc_id: &str, f: impl FnOnce() -> Result<R>) -> Result<R> {
        let h = held(self.key);
        if h.writes > 0 || h.reads > 0 {
            return f();
        }
        let Some(_guard) = self.lock.try_read_recursive_for(LOCK_WAIT_BUDGET) else {
            bail!(
                "doc '{doc_id}': timed out after {LOCK_WAIT_BUDGET:?} waiting for the doc read \
                 lock. A writer is holding it — a long write batch, or a doc-lock callback that \
                 re-enters the doc."
            );
        };
        enter(self.key, false);
        let _depth = DepthGuard {
            key: self.key,
            write: false,
        };
        f()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Arc<LoroDoc> {
        Arc::new(LoroDoc::new())
    }

    #[test]
    fn two_wrappers_over_one_inner_doc_share_one_lock() {
        let d = doc();
        let a = DocLock::for_doc(&d);
        let b = DocLock::for_doc(&d);
        assert!(Arc::ptr_eq(&a.lock, &b.lock));
        assert!(!Arc::ptr_eq(&a.lock, &DocLock::for_doc(&doc()).lock));
    }

    #[test]
    fn a_write_blocks_a_reader_on_another_thread() {
        let d = doc();
        let lock = DocLock::for_doc(&d);
        let (tx, rx) = std::sync::mpsc::channel();
        let inner = DocLock::for_doc(&d);
        std::thread::scope(|s| {
            lock.write("t", || {
                let h = s.spawn(move || inner.read("t", || Ok(tx.send(()).unwrap())));
                assert!(
                    rx.recv_timeout(Duration::from_millis(250)).is_err(),
                    "the reader observed the doc while the write lock was held"
                );
                Ok(h)
            })
            .unwrap()
            .join()
            .unwrap()
            .unwrap();
        });
    }

    #[test]
    fn nesting_passes_through_instead_of_deadlocking() {
        let d = doc();
        let lock = DocLock::for_doc(&d);
        lock.write("t", || lock.write("t", || lock.read("t", || Ok(()))))
            .unwrap();
        lock.read("t", || lock.read("t", || Ok(()))).unwrap();
    }

    #[test]
    fn a_write_inside_a_read_is_reported_not_deadlocked() {
        let d = doc();
        let lock = DocLock::for_doc(&d);
        let err = lock
            .read("t", || lock.write("t", || Ok(())))
            .expect_err("upgrading a read guard must fail loud");
        assert!(err.to_string().contains("cannot be upgraded"), "{err}");
    }

    #[test]
    fn a_panicking_closure_does_not_leak_thread_depth() {
        let d = doc();
        let lock = DocLock::for_doc(&d);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lock.write::<()>("t", || panic!("boom"))
        }));
        assert!(r.is_err());
        assert_eq!(held(lock.key).writes, 0);
        lock.write("t", || Ok(())).unwrap();
    }
}
