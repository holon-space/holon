//! Reactive single-value container.
//!
//! Complements `LiveData` (keyed collection) for cases where a single
//! value changes over time — e.g., a pre-interpreted `ViewModel` tree.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};

/// A reactive container for a single value that changes over time.
///
/// Unlike `LiveData` (a keyed collection driven by CDC), `LiveValue`
/// holds one `T` and notifies observers when it changes.
pub struct LiveValue<T: Send + Sync> {
    value: RwLock<T>,
    version: AtomicU64,
    on_change: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl<T: Send + Sync> LiveValue<T> {
    pub fn new(initial: T) -> Arc<Self> {
        Arc::new(Self {
            value: RwLock::new(initial),
            version: AtomicU64::new(1),
            on_change: Mutex::new(None),
        })
    }

    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        self.value.read().unwrap()
    }

    pub fn write(&self, value: T) {
        {
            let mut guard = self.value.write().unwrap();
            *guard = value;
        }
        self.version.fetch_add(1, Ordering::Release);
        if let Some(cb) = self.on_change.lock().unwrap().as_ref() {
            cb();
        }
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Register a callback fired after each `write()`.
    pub fn set_on_change(&self, callback: impl Fn() + Send + 'static) {
        *self.on_change.lock().unwrap() = Some(Box::new(callback));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn read_write_version() {
        let lv = LiveValue::new(42);
        assert_eq!(*lv.read(), 42);
        assert_eq!(lv.version(), 1);

        lv.write(99);
        assert_eq!(*lv.read(), 99);
        assert_eq!(lv.version(), 2);
    }

    #[test]
    fn on_change_fires() {
        let lv = LiveValue::new("hello".to_string());
        let count = Arc::new(AtomicUsize::new(0));
        let count_clone = Arc::clone(&count);
        lv.set_on_change(move || {
            count_clone.fetch_add(1, Ordering::Relaxed);
        });

        lv.write("world".to_string());
        assert_eq!(count.load(Ordering::Relaxed), 1);

        lv.write("!".to_string());
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }
}
