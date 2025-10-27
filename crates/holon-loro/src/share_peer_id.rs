//! Stable Loro peer-id derivation for shared subtrees.
//!
//! Each `LoroDoc` needs a peer-id per device. `rand::random()` (used by the
//! fork-and-prune throwaway docs) is wrong for a persisted share: it is neither
//! namespaced by device (two devices could collide on the same 64-bit id) nor
//! reproducible. We instead derive a 64-bit peer-id from
//! `(device_secret.public(), shared_tree_id, generation)`: namespaced by the
//! device key so distinct devices never collide, and deterministic given the
//! generation. Distinct peer-ids across sessions are *fine* for CRDT
//! convergence (this is the standard per-session-actor pattern) — what breaks
//! convergence is *reusing* a peer-id with different counter content, which the
//! `generation` prevents (below).
//!
//! **Why the `generation`.** Snapshot saves are debounced, so a process exit
//! between an outbound sync (ops already pushed to a peer) and the debounced
//! save reloads a *stale* snapshot on restart. Without a generation, the
//! device re-mints the exact same `(peer_id, counter)` and authors *different*
//! content under it — both peers' version vectors then believe they already
//! hold each other's ops, so they silently never converge. The generation is a
//! monotonic per-share counter bumped on every mint (share/accept/rehydrate);
//! folding it into the peer-id guarantees post-restart edits author under a
//! *fresh* peer-id, so a stale loaded snapshot cannot cause counter reuse.
//!
//! **Why blake3, not `DefaultHasher`.** `DefaultHasher`'s algorithm is not
//! guaranteed stable across std versions (or platforms), so using it for a
//! *persisted* identity is a latent bug: a std upgrade could silently change
//! every device's peer-id and orphan existing shares. blake3 is a fixed,
//! portable digest.

use iroh::SecretKey;

/// Derive a stable `u64` peer-id from a device key, a share id, and a
/// monotonic generation. Same `(device, share, generation)` → same id.
pub fn stable_peer_id(device: &SecretKey, shared_tree_id: &str, generation: u64) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(device.public().as_bytes());
    hasher.update(shared_tree_id.as_bytes());
    hasher.update(&generation.to_le_bytes());
    let digest = hasher.finalize();
    let mut first_eight = [0u8; 8];
    first_eight.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(first_eight)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_per_device_and_share() {
        let sk = SecretKey::generate(&mut rand::rng());
        assert_eq!(stable_peer_id(&sk, "abc", 0), stable_peer_id(&sk, "abc", 0));
    }

    #[test]
    fn differs_across_shares() {
        let sk = SecretKey::generate(&mut rand::rng());
        assert_ne!(stable_peer_id(&sk, "abc", 0), stable_peer_id(&sk, "def", 0));
    }

    #[test]
    fn differs_across_devices() {
        let sk1 = SecretKey::generate(&mut rand::rng());
        let sk2 = SecretKey::generate(&mut rand::rng());
        assert_ne!(
            stable_peer_id(&sk1, "abc", 0),
            stable_peer_id(&sk2, "abc", 0)
        );
    }

    #[test]
    fn differs_across_generations() {
        let sk = SecretKey::generate(&mut rand::rng());
        assert_ne!(stable_peer_id(&sk, "abc", 0), stable_peer_id(&sk, "abc", 1));
    }
}
