//! Process-global diagnostic counters for the memory monitor.
//!
//! Counters live in the layer that produces them (matview leases in
//! holon-turso, watcher/interpretation counts in holon-frontend, the entity
//! cache in the GPUI shell). Each owner registers a reader here; the 30s
//! sampler in `holon_frontend::memory_monitor` only reads. This is the one
//! seam that lets the sampler report numbers from crates it cannot depend on.
//!
//! Registration is keyed by owner, so re-creating an owner (every test that
//! builds a `MatviewManager`) replaces its entry instead of accumulating
//! stale readers.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

/// Reads one owner's counters as `(name, value)` pairs.
pub type StatsReader = Arc<dyn Fn() -> Vec<(&'static str, u64)> + Send + Sync>;

type Registry = Mutex<BTreeMap<&'static str, StatsReader>>;

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn lock() -> std::sync::MutexGuard<'static, BTreeMap<&'static str, StatsReader>> {
    registry()
        .lock()
        .expect("memstats registry mutex poisoned — a stats reader panicked")
}

/// Register (or replace) `owner`'s counter reader.
pub fn register(owner: &'static str, reader: StatsReader) {
    lock().insert(owner, reader);
}

/// Drop `owner`'s reader. Owners with a meaningful lifetime (the GPUI root)
/// deregister on teardown so the sampler never reads a dead cache.
pub fn deregister(owner: &'static str) {
    lock().remove(owner);
}

/// All registered counters as `owner.name = value`, sorted by owner.
pub fn snapshot() -> Vec<(String, u64)> {
    let readers: Vec<(&'static str, StatsReader)> = lock()
        .iter()
        .map(|(owner, reader)| (*owner, reader.clone()))
        .collect();
    readers
        .into_iter()
        .flat_map(|(owner, reader)| {
            reader()
                .into_iter()
                .map(move |(name, value)| (format!("{owner}.{name}"), value))
        })
        .collect()
}

/// Render [`snapshot`] as a single `a=1 b=2` line for the sampler log.
pub fn snapshot_line() -> String {
    snapshot()
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_reports_registered_counters_and_reregistration_replaces() {
        register("probe", Arc::new(|| vec![("a", 1), ("b", 2)]));
        let line = snapshot_line();
        assert!(line.contains("probe.a=1"), "{line}");
        assert!(line.contains("probe.b=2"), "{line}");

        register("probe", Arc::new(|| vec![("a", 7)]));
        let line = snapshot_line();
        assert!(line.contains("probe.a=7"), "{line}");
        assert!(!line.contains("probe.b="), "{line}");

        deregister("probe");
        assert!(!snapshot_line().contains("probe."), "reader must be gone");
    }
}
