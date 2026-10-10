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
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use tracing::error;

use crate::secure_client::CONNECT_BUDGET;
use crate::secure_client::MAX_CONCURRENT_GET_STREAMS;
use crate::secure_client::MAX_CONCURRENT_POST_STREAMS;
use crate::secure_client::MAX_DISCLOSED_PEER_TEXT_BYTES;
use crate::secure_client::MAX_LIST_BYTES;
use crate::secure_client::MAX_RESPONSE_BODY_BYTES;
use crate::secure_client::MAX_STDERR_BYTES;

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
    /// The bytes of list items this connection's enumerations have collected.
    pub(crate) listed_bytes: ListedBytes,
    /// What this connection's sidecar may still write into the log.
    pub(crate) stderr: StderrAllowance,
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
            listed_bytes: ListedBytes::default(),
            stderr: StderrAllowance::default(),
        })
    }
}

/// What a sidecar's stderr may add to the log, and what is said once it has
/// added that much.
///
/// stderr is peer-written text going to a sink that may be an append-only
/// file, so its volume is a resource the peer grows. Past the bound the lines
/// are dropped — but never quietly: the first drop names the bound and the
/// running count is repeated at every power of ten, so a flood of N lines
/// costs log(N) log lines.
#[derive(Default, Debug)]
pub(crate) struct StderrAllowance {
    spent: AtomicUsize,
    dropped: AtomicU64,
}

/// What to do with one stderr line of a sidecar.
pub(crate) enum StderrVerdict {
    /// Write this to the log; bounded and escaped.
    Forward(String),
    /// Write this to the log: the bound has something to say.
    Say(String),
    /// Dropped, and there is nothing new to say about it.
    Nothing,
}

impl StderrAllowance {
    /// Charge `line` to this connection's stderr allowance, refused once it
    /// has spent [`MAX_STDERR_BYTES`].
    pub(crate) fn admit(&self, line: &str) -> StderrVerdict {
        let text = bounded_peer_text(line);
        let spent = self.spent.fetch_add(text.len(), Ordering::Relaxed) + text.len();
        if spent <= MAX_STDERR_BYTES {
            return StderrVerdict::Forward(text);
        }
        self.spent.fetch_sub(text.len(), Ordering::Relaxed);
        let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        if dropped == 1 {
            return StderrVerdict::Say(format!(
                "MAX_STDERR_BYTES ({MAX_STDERR_BYTES} bytes) of this sidecar's stderr are in the \
                 log; further lines are dropped and counted"
            ));
        }
        if is_power_of_ten(dropped) {
            return StderrVerdict::Say(format!(
                "{dropped} stderr lines dropped since MAX_STDERR_BYTES ({MAX_STDERR_BYTES} bytes) \
                 was spent"
            ));
        }
        StderrVerdict::Nothing
    }

    /// What this bound has to disclose about the lines it dropped.
    pub(crate) fn dropped_report(&self) -> Option<String> {
        let dropped = self.dropped.load(Ordering::Relaxed);
        (dropped > 0).then(|| {
            format!(
                "{dropped} stderr lines were dropped by MAX_STDERR_BYTES ({MAX_STDERR_BYTES} \
                 bytes)"
            )
        })
    }
}

fn is_power_of_ten(n: u64) -> bool {
    let mut power = 10u64;
    while power < n {
        match power.checked_mul(10) {
            Some(next) => power = next,
            None => return false,
        }
    }
    power == n
}

/// The bytes of list items a connection holds: a page of an enumeration is
/// kept for as long as the connection serves the catalog built from it, so
/// nothing is ever released here.
#[derive(Default, Debug)]
pub(crate) struct ListedBytes(AtomicUsize);

impl ListedBytes {
    /// Charge `more` bytes of items, refused once this connection's
    /// enumerations hold more than [`MAX_LIST_BYTES`] between them. The
    /// refusal names the bound and what was held when it tripped.
    pub(crate) fn charge(&self, more: usize) -> Result<(), String> {
        let held = self.0.fetch_add(more, Ordering::Relaxed) + more;
        if held > MAX_LIST_BYTES {
            self.0.fetch_sub(more, Ordering::Relaxed);
            return Err(format!(
                "collected {held} bytes of list items, past MAX_LIST_BYTES ({MAX_LIST_BYTES} \
                 bytes) for this connection"
            ));
        }
        Ok(())
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
/// [`MAX_DISCLOSED_PEER_TEXT_BYTES`] of it, every control character escaped,
/// and what was cut NAMED rather than dropped.
///
/// Peer text reaches a toast, the integration's row and the log, and the peer
/// picks both its length and its bytes. Cutting it silently would leave a
/// reader unable to tell a complete error from a truncated one, and a raw
/// control character would let the peer write a log line of its own (a
/// newline), move the terminal's cursor (a CSI sequence) or ring it (BEL).
pub fn bounded_peer_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_DISCLOSED_PEER_TEXT_BYTES));
    let mut kept = 0;
    for c in text.chars() {
        let mut utf8 = [0u8; 4];
        let escaped = c
            .is_control()
            .then(|| c.escape_default().collect::<String>());
        let piece = match &escaped {
            Some(e) => e.as_str(),
            None => c.encode_utf8(&mut utf8),
        };
        if out.len() + piece.len() > MAX_DISCLOSED_PEER_TEXT_BYTES {
            break;
        }
        out.push_str(piece);
        kept += c.len_utf8();
    }
    let cut = text.len() - kept;
    if cut == 0 {
        return out;
    }
    format!("{out}… ({cut} bytes cut)")
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A peer that answers with escape sequences writes into a line-oriented
    /// log and a terminal: a newline forges a log line of its own, a CSI
    /// sequence rewrites what is already on screen, BEL rings it.
    #[test]
    fn control_characters_of_peer_text_are_escaped_and_named() {
        let disclosed = bounded_peer_text("harmless\u{1b}[2J\u{7}\nsecond line\r\n");

        assert!(
            !disclosed.chars().any(char::is_control),
            "no control character may reach the sink verbatim; got {disclosed:?}"
        );
        for escape in ["\\u{1b}", "\\u{7}", "\\n", "\\r"] {
            assert!(
                disclosed.contains(escape),
                "a control character must be VISIBLE, not dropped: {escape} missing from \
                 {disclosed:?}"
            );
        }
        assert!(
            disclosed.starts_with("harmless") && disclosed.contains("second line"),
            "the peer's readable text must survive: {disclosed:?}"
        );
    }

    /// Escaping grows the text, so the bound is charged on what reaches the
    /// sink — not on the bytes the peer sent.
    #[test]
    fn escaping_does_not_widen_the_bound() {
        let disclosed = bounded_peer_text(&"\u{1b}".repeat(1 << 16));

        assert!(
            disclosed.len() <= MAX_DISCLOSED_PEER_TEXT_BYTES + 64,
            "escaped control characters must be charged to MAX_DISCLOSED_PEER_TEXT_BYTES \
             ({MAX_DISCLOSED_PEER_TEXT_BYTES}); got {} bytes",
            disclosed.len()
        );
        assert!(
            disclosed.contains("bytes cut"),
            "what was cut must be named: {}",
            &disclosed[..disclosed.len().min(120)]
        );
    }
}
