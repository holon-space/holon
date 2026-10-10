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
    /// The bytes of list items this connection's enumerations have collected,
    /// and what that leaves a page of one.
    pub(crate) listed_bytes: Arc<ListedBytes>,
    /// What this connection's sidecar may still write into the log.
    pub(crate) stderr: StderrAllowance,
}

impl PeerBudget {
    pub fn new() -> Arc<Self> {
        let listed_bytes = Arc::new(ListedBytes::default());
        Arc::new(Self {
            post_streams: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_POST_STREAMS)),
            get_streams: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_GET_STREAMS)),
            held_event_bytes: Arc::new(HeldEventBytes::new(listed_bytes.clone())),
            connect_deadline: tokio::time::Instant::now() + CONNECT_BUDGET,
            inbound: Arc::new(InboundNotices::default()),
            transport: Arc::new(BoundTrips::default()),
            listed_bytes,
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
    /// The total already disclosed, so the two paths that ask for it — the
    /// sidecar's stderr ending and the connection closing — say it once
    /// between them.
    reported: AtomicU64,
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
    pub(crate) fn charge_line(&self, line: &str) -> StderrVerdict {
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

    /// What this bound has to disclose about the lines it dropped, and nothing
    /// once it has been said: whichever of the sidecar's stderr ending and the
    /// connection closing asks first says it.
    pub(crate) fn dropped_report(&self) -> Option<String> {
        let dropped = self.dropped.load(Ordering::Relaxed);
        let said = self.reported.swap(dropped, Ordering::Relaxed);
        (dropped > said).then(|| {
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
pub(crate) struct ListedBytes {
    held: AtomicUsize,
    /// How many enumerations of this connection are fetching pages.
    enumerating: AtomicUsize,
}

impl ListedBytes {
    /// Charge `more` bytes of items, refused once this connection's
    /// enumerations hold more than [`MAX_LIST_BYTES`] between them. The
    /// refusal names the bound and what was held when it tripped.
    pub(crate) fn charge(&self, more: usize) -> Result<(), String> {
        let held = self.held.fetch_add(more, Ordering::Relaxed) + more;
        if held > MAX_LIST_BYTES {
            self.held.fetch_sub(more, Ordering::Relaxed);
            return Err(format!(
                "collected {held} bytes of list items, past MAX_LIST_BYTES ({MAX_LIST_BYTES} \
                 bytes) for this connection"
            ));
        }
        Ok(())
    }

    /// Mark one enumeration of this connection as fetching pages for as long
    /// as the returned guard lives.
    pub(crate) fn enumeration(self: &Arc<Self>) -> Enumerating {
        self.enumerating.fetch_add(1, Ordering::Relaxed);
        Enumerating(self.clone())
    }

    /// What one reply body of this connection may hold.
    ///
    /// A page of an enumeration may hold no more than the enumeration itself
    /// may still collect: charged after the decode, a page four times
    /// [`MAX_LIST_BYTES`] is received and deserialized before the charge
    /// refuses it.
    pub(crate) fn body_cap(&self) -> BodyCap {
        if self.enumerating.load(Ordering::Relaxed) == 0 {
            return BodyCap::ResponseBody;
        }
        let left = MAX_LIST_BYTES.saturating_sub(self.held.load(Ordering::Relaxed));
        if left >= MAX_RESPONSE_BODY_BYTES {
            return BodyCap::ResponseBody;
        }
        BodyCap::Enumeration(left)
    }
}

/// One enumeration of a connection, fetching pages for as long as this lives.
pub(crate) struct Enumerating(Arc<ListedBytes>);

impl Drop for Enumerating {
    fn drop(&mut self) {
        self.0.enumerating.fetch_sub(1, Ordering::Relaxed);
    }
}

/// How many bytes of ONE reply body a connection may take, and which bound
/// says so.
pub(crate) enum BodyCap {
    /// What any reply body may hold.
    ResponseBody,
    /// What [`MAX_LIST_BYTES`] leaves the enumeration this body is a page of.
    Enumeration(usize),
}

impl BodyCap {
    pub(crate) fn bytes(&self) -> usize {
        match self {
            Self::ResponseBody => MAX_RESPONSE_BODY_BYTES,
            Self::Enumeration(left) => *left,
        }
    }
}

impl std::fmt::Display for BodyCap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ResponseBody => {
                write!(
                    f,
                    "MAX_RESPONSE_BODY_BYTES ({MAX_RESPONSE_BODY_BYTES} bytes)"
                )
            }
            Self::Enumeration(left) => write!(
                f,
                "the {left} bytes MAX_LIST_BYTES ({MAX_LIST_BYTES} bytes) leaves this \
                 connection's enumeration"
            ),
        }
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

/// Every character a disclosure escapes instead of passing on: the C0/C1
/// controls, the separators anything laying text out breaks a line on, and the
/// Unicode bidi controls, which reorder the text around them.
fn is_display_control(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}'
                | '\u{2029}'
                | '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

/// `text` as a disclosure may carry it: at most
/// [`MAX_DISCLOSED_PEER_TEXT_BYTES`] of it, every [`is_display_control`]
/// character escaped, and what was cut NAMED rather than dropped.
///
/// Peer text reaches a toast, the integration's row and the log, and the peer
/// picks both its length and its bytes. Cutting it silently would leave a
/// reader unable to tell a complete error from a truncated one, and a raw
/// control character would let the peer write a log line of its own (a
/// newline), move the terminal's cursor (a CSI sequence) or reverse the row it
/// lands in (U+202E).
pub fn bounded_peer_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_DISCLOSED_PEER_TEXT_BYTES));
    let mut kept = 0;
    for c in text.chars() {
        let mut utf8 = [0u8; 4];
        let escaped = is_display_control(c).then(|| c.escape_default().collect::<String>());
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
#[derive(Debug)]
pub(crate) struct HeldEventBytes {
    held: AtomicUsize,
    listed: Arc<ListedBytes>,
}

impl HeldEventBytes {
    pub(crate) fn new(listed: Arc<ListedBytes>) -> Self {
        Self {
            held: AtomicUsize::new(0),
            listed,
        }
    }

    /// Move a stream's held size from `old` to `new`, refused once the
    /// connection's streams would hold more than [`ListedBytes::body_cap`]
    /// between them.
    ///
    /// A refusal leaves the stream charged for `old`, so the caller keeps the
    /// size it had before the chunk it could not take.
    pub(crate) fn recharge(&self, old: usize, new: usize) -> std::io::Result<()> {
        if new < old {
            self.held.fetch_sub(old - new, Ordering::Relaxed);
            return Ok(());
        }
        let more = new - old;
        let held = self.held.fetch_add(more, Ordering::Relaxed) + more;
        let cap = self.listed.body_cap();
        if held > cap.bytes() {
            self.held.fetch_sub(more, Ordering::Relaxed);
            return Err(std::io::Error::other(format!(
                "the unfinished messages of this MCP connection hold more than {cap} between \
                 them; reading stopped there"
            )));
        }
        Ok(())
    }

    pub(crate) fn release(&self, held: usize) {
        self.held.fetch_sub(held, Ordering::Relaxed);
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

    /// A disclosure is read left to right, and these characters change what
    /// that means without being `Cc`: the bidi controls reorder the text
    /// around them in a UI row, and U+2028 breaks the line wherever text is
    /// laid out.
    #[test]
    fn separators_and_bidi_controls_of_peer_text_are_escaped_too() {
        assert_eq!(
            bounded_peer_text("resu\u{202e}nimda\u{2028}\u{2066}\u{200f}second"),
            "resu\\u{202e}nimda\\u{2028}\\u{2066}\\u{200f}second"
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

    /// Two paths ask for the total — the sidecar's stderr ending and the
    /// connection closing — because neither is guaranteed to happen first.
    /// Said twice, a reader counts the drops twice.
    #[test]
    fn the_dropped_stderr_total_is_said_once() {
        let allowance = StderrAllowance::default();
        let line = "x".repeat(4096);
        for _ in 0..(MAX_STDERR_BYTES / line.len() + 2) {
            allowance.charge_line(&line);
        }

        let said = allowance
            .dropped_report()
            .expect("lines past MAX_STDERR_BYTES were dropped, so there is a total to say");
        assert!(
            said.contains("MAX_STDERR_BYTES"),
            "the total must name the bound that dropped them; got {said:?}"
        );
        assert_eq!(
            allowance.dropped_report(),
            None,
            "the total was already said once"
        );
    }
}
