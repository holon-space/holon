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

use tracing::error;

use crate::secure_client::CONNECT_BUDGET;
use crate::secure_client::MAX_CONCURRENT_GET_STREAMS;
use crate::secure_client::MAX_CONCURRENT_POST_STREAMS;
use crate::secure_client::MAX_DISCLOSED_PEER_TEXT_BYTES;
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
    /// What the peer's unsolicited `resources/updated` notices may park, and
    /// where a bound that refused one is announced.
    pub inbound: Arc<InboundNotices>,
    /// Where a bound that ENDED one of this connection's transport legs says
    /// so. A leg that ends takes every operation of the integration with it,
    /// so this cannot be a log line only.
    pub transport: Arc<BoundTrips>,
}

impl PeerBudget {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            post_streams: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_POST_STREAMS)),
            get_streams: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_GET_STREAMS)),
            held_event_bytes: Arc::new(HeldEventBytes::default()),
            connect_deadline: tokio::time::Instant::now() + CONNECT_BUDGET,
            inbound: Arc::new(InboundNotices::default()),
            transport: Arc::new(BoundTrips::default()),
        })
    }
}

/// The inbound direction of the budget: the one place that says what happens
/// when a bound refuses a peer's `resources/updated` notice.
///
/// A dropped notice means rows that stay stale while the UI shows them as
/// current, so no bound on this path may drop a signal. Each one WIDENS it
/// instead: the per-URI re-syncs the connection owes are replaced by one full
/// re-sync, and the reason is published here for the sync loop to act on and
/// the app to disclose.
#[derive(Debug, Default)]
pub struct InboundNotices(BoundTrips);

impl InboundNotices {
    /// Replace this connection's owed per-URI re-syncs with one full re-sync,
    /// because `reason` names a bound that refused a notice.
    pub(crate) fn collapse(&self, reason: String) {
        error!(
            "[inbound] {reason} — which resource changed is no longer known, so this \
             connection re-syncs every entity once instead"
        );
        self.0.publish(reason);
    }

    /// Fires once per collapse, carrying its reason.
    pub fn collapses(&self) -> tokio::sync::watch::Receiver<Option<String>> {
        self.0.trips()
    }
}

/// The reasons bounds gave, one per trip, for whoever has to disclose them.
///
/// A watch rather than a log line: a bound that trips while the app runs is a
/// degradation someone must be told about, and the only part that says what to
/// do about it is which bound it was.
#[derive(Debug)]
pub struct BoundTrips(tokio::sync::watch::Sender<Option<String>>);

impl Default for BoundTrips {
    fn default() -> Self {
        Self(tokio::sync::watch::Sender::new(None))
    }
}

impl BoundTrips {
    pub(crate) fn publish(&self, reason: String) {
        self.0.send_replace(Some(reason));
    }

    /// Fires once per trip, carrying its reason. A receiver taken before the
    /// trip sees it; one taken after reads it as the current value, so a
    /// watcher that starts late still discloses it.
    pub fn trips(&self) -> tokio::sync::watch::Receiver<Option<String>> {
        self.0.subscribe()
    }
}

/// `text` as a disclosure may carry it: at most
/// [`MAX_DISCLOSED_PEER_TEXT_BYTES`], with what was cut NAMED rather than
/// dropped.
///
/// Peer text reaches a toast, the integration's row and the log, and a peer
/// picks its length. Cutting it silently would leave a reader unable to tell a
/// complete error from a truncated one.
pub fn bounded_peer_text(text: &str) -> String {
    if text.len() <= MAX_DISCLOSED_PEER_TEXT_BYTES {
        return text.to_string();
    }
    let mut end = MAX_DISCLOSED_PEER_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = text.len() - end;
    format!("{}… ({cut} bytes cut)", &text[..end])
}

/// The bytes of unfinished messages a connection holds together: a partial
/// SSE event per HTTP stream, a partial line on the stdio leg.
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
                "the unfinished messages of this MCP connection hold more than \
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
