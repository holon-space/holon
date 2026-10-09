//! Everything one MCP connection's peer is allowed to consume, in one place.
//!
//! An MCP peer is untrusted and every one of these resources is one IT grows:
//! it decides how many pages a list has, how long a page takes, how many reply
//! and notification streams stay unfinished, and how many bytes of a partial
//! event each of those holds. Rounds 1-4 of this lane bounded one such path at
//! a time and each time the next one next to it turned out to be unbounded, so
//! the bounds live here, together, and every call to a peer goes through a
//! [`BudgetedPeer`] that holds one of these.
//!
//! `PeerBudget::new` arms the connect deadline, and a peer handle cannot be
//! built without a budget, so the clock starts before the handshake it bounds.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crate::secure_client::CONNECT_BUDGET;
use crate::secure_client::MAX_CONCURRENT_GET_STREAMS;
use crate::secure_client::MAX_CONCURRENT_POST_STREAMS;
use crate::secure_client::MAX_RESPONSE_BODY_BYTES;

/// One connection's share of every resource its peer can grow.
#[derive(Debug)]
pub struct PeerBudget {
    /// One slot per unfinished POST reply stream.
    pub(crate) post_streams: Arc<tokio::sync::Semaphore>,
    /// One slot per open notification (GET) stream.
    pub(crate) get_streams: Arc<tokio::sync::Semaphore>,
    /// The bytes of partial SSE events every stream of this connection holds,
    /// together.
    pub(crate) held_event_bytes: Arc<HeldEventBytes>,
    /// When the handshake and every enumeration page of the connect must be
    /// over. Armed by [`Self::new`]; see [`CONNECT_BUDGET`].
    pub(crate) connect_deadline: tokio::time::Instant,
}

impl PeerBudget {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            post_streams: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_POST_STREAMS)),
            get_streams: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_GET_STREAMS)),
            held_event_bytes: Arc::new(HeldEventBytes::default()),
            connect_deadline: tokio::time::Instant::now() + CONNECT_BUDGET,
        })
    }
}

/// The partial-event bytes a connection's streams hold together.
#[derive(Default, Debug)]
pub(crate) struct HeldEventBytes(AtomicUsize);

impl HeldEventBytes {
    /// Move a stream's held size from `old` to `new`, refused once the
    /// connection's streams would hold more than [`MAX_RESPONSE_BODY_BYTES`]
    /// between them.
    ///
    /// A refusal leaves the stream charged for `old`, so the caller keeps the
    /// size it had before the chunk it could not take.
    pub(crate) fn recharge(&self, old: usize, new: usize) -> std::io::Result<()> {
        if new < old {
            self.0.fetch_sub(old - new, Ordering::Relaxed);
            return Ok(());
        }
        let more = new - old;
        let held = self.0.fetch_add(more, Ordering::Relaxed) + more;
        if held > MAX_RESPONSE_BODY_BYTES {
            self.0.fetch_sub(more, Ordering::Relaxed);
            return Err(std::io::Error::other(format!(
                "the unfinished events of this MCP connection hold more than \
                 MAX_RESPONSE_BODY_BYTES ({MAX_RESPONSE_BODY_BYTES} bytes) between them; reading \
                 stopped there"
            )));
        }
        Ok(())
    }

    pub(crate) fn release(&self, held: usize) {
        self.0.fetch_sub(held, Ordering::Relaxed);
    }
}
