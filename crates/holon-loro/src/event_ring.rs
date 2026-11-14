//! Bounded in-memory ring of change events with monotonic sequence numbers.
//!
//! Replaces the previous unbounded `Vec<Change<Block>>` event logs in
//! `LoroBackend`/`MemoryBackend`, which grew with every mutation for the
//! process lifetime and were cloned wholesale for every new subscriber.
//!
//! Watermark model: `get_current_version` exposes `next_seq` (little-endian
//! `u64`); a `StreamPosition::Version(w)` subscriber replays exactly the
//! entries with `seq >= w`. When `w` has been evicted from the ring the
//! replay FAILS LOUD (`ReplayWindowExpired`) instead of silently returning
//! partial history — the subscriber must re-sync from
//! `StreamPosition::Beginning`.

use std::collections::VecDeque;

pub const DEFAULT_EVENT_RING_CAPACITY: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayWindowExpired {
    pub requested: u64,
    pub oldest_available: u64,
}

impl std::fmt::Display for ReplayWindowExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "replay window expired: requested watermark {} but oldest retained event is {} — \
             re-subscribe from StreamPosition::Beginning",
            self.requested, self.oldest_available
        )
    }
}

#[derive(Debug, Clone)]
pub struct EventRing<T> {
    entries: VecDeque<(u64, T)>,
    next_seq: u64,
    capacity: usize,
}

impl<T: Clone> EventRing<T> {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "EventRing capacity must be > 0");
        Self {
            entries: VecDeque::with_capacity(capacity.min(1024)),
            next_seq: 0,
            capacity,
        }
    }

    pub fn push(&mut self, item: T) {
        self.entries.push_back((self.next_seq, item));
        self.next_seq += 1;
        if self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    /// The watermark a subscriber should hold to receive exactly the events
    /// pushed after this call.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// All events with `seq >= watermark`, oldest first.
    pub fn replay_since(&self, watermark: u64) -> Result<Vec<T>, ReplayWindowExpired> {
        let oldest = self
            .entries
            .front()
            .map(|(s, _)| *s)
            .unwrap_or(self.next_seq);
        if watermark < oldest {
            return Err(ReplayWindowExpired {
                requested: watermark,
                oldest_available: oldest,
            });
        }
        Ok(self
            .entries
            .iter()
            .filter(|(s, _)| *s >= watermark)
            .map(|(_, item)| item.clone())
            .collect())
    }
}

/// Parse a `StreamPosition::Version` token, which `next_seq` encodes as
/// little-endian `u64` bytes.
pub fn watermark_from_token(token: &[u8]) -> Result<u64, holon_api::ApiError> {
    let bytes: [u8; 8] = token
        .try_into()
        .map_err(|_| holon_api::ApiError::InvalidOperation {
            message: format!(
                "stream version token must be 8 bytes (little-endian u64 watermark), got {} bytes",
                token.len()
            ),
        })?;
    Ok(u64::from_le_bytes(bytes))
}

/// Deliver one change batch to every subscriber, fail-loud.
///
/// The previous `retain(|s| s.try_send(..).is_ok())` conflated two cases:
/// a *closed* channel (normal disconnect — prune silently) and a *full*
/// channel (slow consumer — the subscriber was dropped forever, silently
/// losing all future changes). Now a full channel blocks this delivery task
/// (never the mutator — callers run this in a spawned task) up to a timeout;
/// only on timeout is the subscriber dropped, with an error log.
pub async fn deliver_to_subscribers<T>(
    subscribers: &mut Vec<tokio::sync::mpsc::Sender<Result<Vec<T>, holon_api::ApiError>>>,
    batch: Vec<T>,
) where
    T: Send + Clone + 'static,
{
    use tokio::sync::mpsc::error::TrySendError;
    let mut kept = Vec::with_capacity(subscribers.len());
    for sender in subscribers.drain(..) {
        match sender.try_send(Ok(batch.clone())) {
            Ok(()) => kept.push(sender),
            Err(TrySendError::Closed(_)) => {} // normal disconnect
            Err(TrySendError::Full(payload)) => {
                match tokio::time::timeout(std::time::Duration::from_secs(5), sender.send(payload))
                    .await
                {
                    Ok(Ok(())) => kept.push(sender),
                    Ok(Err(_)) => {} // closed while we waited
                    Err(_) => {
                        tracing::error!(
                            "dropping change subscriber: channel full for >5s (stalled consumer) \
                             — it will miss all further changes"
                        );
                    }
                }
            }
        }
    }
    *subscribers = kept;
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn watermark_token_round_trips(n in any::<u64>()) {
            prop_assert_eq!(watermark_from_token(&n.to_le_bytes()).unwrap(), n);
        }

        #[test]
        fn watermark_token_of_other_length_is_invalid(
            token in prop::collection::vec(any::<u8>(), 0..17).prop_filter("not 8 bytes", |t| t.len() != 8)
        ) {
            // The static examples run on every case count: empty, 7 and 9 bytes.
            for fixed in [&[][..], &[0u8; 7][..], &[0u8; 9][..]] {
                let refused = matches!(
                    watermark_from_token(fixed),
                    Err(holon_api::ApiError::InvalidOperation { .. })
                );
                prop_assert!(refused, "{} bytes are not refused", fixed.len());
            }
            let parsed = watermark_from_token(&token);
            let refused = matches!(parsed, Err(holon_api::ApiError::InvalidOperation { .. }));
            prop_assert!(refused, "{} bytes parsed as {:?}", token.len(), parsed);
        }
    }

    #[test]
    fn replay_from_zero_returns_everything_within_capacity() {
        let mut ring = EventRing::new(8);
        for i in 0..5 {
            ring.push(i);
        }
        assert_eq!(ring.replay_since(0).unwrap(), vec![0, 1, 2, 3, 4]);
        assert_eq!(ring.next_seq(), 5);
    }

    #[test]
    fn watermark_skips_already_seen_events() {
        let mut ring = EventRing::new(8);
        ring.push("a");
        let w = ring.next_seq();
        ring.push("b");
        ring.push("c");
        assert_eq!(ring.replay_since(w).unwrap(), vec!["b", "c"]);
    }

    #[test]
    fn eviction_makes_old_watermarks_fail_loud() {
        let mut ring = EventRing::new(2);
        for i in 0..5 {
            ring.push(i);
        }
        // entries retained: seq 3, 4 — watermark 0 is gone.
        let err = ring.replay_since(0).unwrap_err();
        assert_eq!(err.oldest_available, 3);
        assert_eq!(ring.replay_since(3).unwrap(), vec![3, 4]);
    }

    #[test]
    fn current_watermark_replays_nothing() {
        let mut ring = EventRing::new(2);
        for i in 0..5 {
            ring.push(i);
        }
        assert_eq!(
            ring.replay_since(ring.next_seq()).unwrap(),
            Vec::<i32>::new()
        );
    }

    #[test]
    fn empty_ring_replays_empty_at_zero() {
        let ring: EventRing<i32> = EventRing::new(4);
        assert_eq!(ring.replay_since(0).unwrap(), Vec::<i32>::new());
    }
}
