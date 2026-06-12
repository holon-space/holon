//! Stable Loro peer-id derivation for shared subtrees.
//!
//! Each `LoroDoc` needs a unique peer-id per device. Using `rand::random()`
//! (as the fork-and-prune code does today) works for one-shot extraction but
//! breaks CRDT lineage across restarts: after restart, the same device would
//! rejoin the shared tree with a *different* peer-id, confusing convergence.
//!
//! We derive a 64-bit peer-id from `(device_secret.public(), shared_tree_id)`
//! so the same device+share always gets the same id.

use iroh::SecretKey;
use std::hash::{Hash, Hasher};

/// Derive a stable `u64` peer-id from a device key and a share id.
pub fn stable_peer_id(device: &SecretKey, shared_tree_id: &str) -> u64 {
    // Two-round std hasher gives enough mixing for CRDT peer-id purposes.
    // We don't need cryptographic uniqueness — just determinism per device+share.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    device.public().as_bytes().hash(&mut h);
    shared_tree_id.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_per_device_and_share() {
        let sk = SecretKey::generate(&mut rand::rng());
        assert_eq!(stable_peer_id(&sk, "abc"), stable_peer_id(&sk, "abc"));
    }

    #[test]
    fn differs_across_shares() {
        let sk = SecretKey::generate(&mut rand::rng());
        assert_ne!(stable_peer_id(&sk, "abc"), stable_peer_id(&sk, "def"));
    }

    #[test]
    fn differs_across_devices() {
        let sk1 = SecretKey::generate(&mut rand::rng());
        let sk2 = SecretKey::generate(&mut rand::rng());
        assert_ne!(stable_peer_id(&sk1, "abc"), stable_peer_id(&sk2, "abc"));
    }
}
