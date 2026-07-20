//! Once-per-key-per-boot gate for degraded-render warnings.
//!
//! A single corrupt persisted mark makes the clamp/degrade path fire on EVERY
//! paint of its block — the 2026-07-20 clamp fix logged ERROR 1847× for one
//! visible corrupt block (BugFunnel N7). Fail-loud is right, but per-frame
//! repetition is noise, not signal. This gate keeps the FIRST occurrence per
//! key loud (ERROR, so the corruption is disclosed once) and lets callers drop
//! the repeats to `debug!`. It is NOT a global mute: a distinct corruption
//! (different block / span) gets its own loud first line.
//!
//! Keying granularity is the caller's choice (per block id, per corrupt span,
//! …). Scope is the process lifetime ("per boot"); there is no reset.

use std::collections::HashSet;
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Mutex;

static SEEN: Mutex<Option<HashSet<u64>>> = Mutex::new(None);

/// Returns `true` the first time `key` is observed this boot, `false` on every
/// subsequent call with an equal key. Callers log ERROR on `true`, `debug!` on
/// `false`.
pub fn first_time_seen<K: Hash>(key: K) -> bool {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let hashed = hasher.finish();
    let mut guard = SEEN.lock().expect("warn_throttle SEEN mutex poisoned");
    guard.get_or_insert_with(HashSet::new).insert(hashed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_time_true_then_false_for_same_key() {
        let k = ("warn_throttle_test_alpha", 42u64);
        assert!(first_time_seen(k), "first observation must be loud");
        assert!(!first_time_seen(k), "repeat observation must be throttled");
        assert!(!first_time_seen(k));
    }

    #[test]
    fn distinct_keys_each_get_a_first_loud() {
        assert!(first_time_seen(("warn_throttle_test_beta", 1u64)));
        assert!(first_time_seen(("warn_throttle_test_beta", 2u64)));
    }
}
