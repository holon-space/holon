//! **The transport seam of the two-instance slice** (D71.b).
//!
//! The slice runs its SAME transitions and oracles over TWO wires:
//!
//! - [`RelayTransport`] — the deterministic model. `push_once` / `pull_once`
//!   over an in-process [`InMemoryRelay`]: an untrusted store-and-forward log,
//!   a membership chain per envelope, a cursor per container.
//! - [`IrohTransport`] — **production**. The publisher runs
//!   [`ContainerRegistry::replicate_all`] over a live [`IrohAdvertiser`]; the
//!   consumer dials each advertised container over a real QUIC endpoint and
//!   runs the version-vector exchange. `RelayMode::Disabled`, exactly as the
//!   shipping pair connects.
//!
//! Test and production must not drift, so the slice picks the wire and nothing
//! else changes. [`TransportChoice::from_env`] reads
//! `HOLON_PBT_SYNC_TRANSPORT`.
//!
//! ## What "authorized" means on each wire
//!
//! The model's authorization is a [`MembershipChain`] the publisher attaches to
//! every envelope; with no share in force the chain is empty and the publisher
//! refuses to put state in front of the relay. Iroh has no envelope to sign:
//! its gate is *enrollment*, so the consumer presents the share's real
//! [`CapabilitySecret`] only while the model says the container is shared, and
//! a fresh (therefore unprovable) one otherwise. Both wires report the same
//! observable — the container lands in `unauthorized`, nothing crosses, and a
//! connection/consultation counter proves the refusal was a decision.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use holon_api::TestClock;
use holon_loro::ContainerRegistry;
use holon_loro::container_registry::ROOT_CONTAINER_ID;
use holon_loro::iroh_advertiser::ALPN_PREFIX;
use holon_loro::iroh_advertiser::Endpoint;
use holon_loro::iroh_advertiser::EndpointAddr;
use holon_loro::iroh_advertiser::IrohAdvertiser;
use holon_loro::iroh_sync_adapter::create_endpoint;
use holon_loro::iroh_sync_adapter::make_alpn;
use holon_loro::iroh_sync_adapter::sync_doc_initiate_enrolled;
use holon_loro::share_enrollment::CapabilitySecret;
use holon_loro::share_enrollment::EnrollmentRefused;
use holon_loro::share_enrollment::ExpiryTime;
use holon_loro::share_enrollment::ShareRoster;
use holon_loro::sync_transport::InMemoryRelay;
use holon_loro::sync_transport::StablePeerId;
use holon_pbt_core::capabilities::SyncTransportKind;
use holon_sharing::acceptor::AcceptorContext;
use holon_sharing::lease::MembershipChain;
use holon_sharing::policy::Principal;
use holon_sharing::policy::UnverifiedVerifier;
use holon_sharing::sync::OutboundAuth;
use holon_sharing::sync::SyncSession;
use holon_sharing::sync::pull_once;
use holon_sharing::sync::push_once;

use crate::pbt::sharing_state::OWNER_PRINCIPAL;
use crate::pbt::sharing_state::RECEIVER_PRINCIPAL;

/// Environment variable selecting the wire. `relay` (default) or `iroh`.
pub const TRANSPORT_ENV: &str = "HOLON_PBT_SYNC_TRANSPORT";

/// Roster window and peer cap for the iroh leg. Both are generous on purpose:
/// neither may ever be the reason a round fails, or a divergence between the
/// wires would be an artifact of the test rig.
const IROH_ROSTER_TTL_SECS: i64 = 3600;
const IROH_ROSTER_MAX_PEERS: usize = 64;

/// How long a dial may take before the round fails LOUDLY. A bounded wait, not
/// a settle sleep: the endpoint either answers or the round is a failure worth
/// reporting.
const DIAL_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportChoice {
    Relay,
    Iroh,
}

impl TransportChoice {
    /// Read the wire from the environment. An unrecognized value is a loud
    /// panic, never a silent fall back to the model — a typo'd variable that
    /// quietly ran the relay would report a production green it never earned.
    pub fn from_env() -> Self {
        match std::env::var(TRANSPORT_ENV).as_deref() {
            Ok("iroh") => Self::Iroh,
            Ok("relay") | Err(std::env::VarError::NotPresent) => Self::Relay,
            other => panic!(
                "{TRANSPORT_ENV}={other:?} is not a transport; use `relay` (the deterministic \
                 model) or `iroh` (production `replicate_all`)"
            ),
        }
    }

    /// The wire this choice is a request FOR. Deliberately derived from the
    /// choice and not from the built transport: an assertion that reads the
    /// kind off the object it is testing compares the object against itself
    /// and cannot catch a `build` that returns the wrong one.
    pub fn kind(self) -> SyncTransportKind {
        match self {
            Self::Relay => SyncTransportKind::Relay,
            Self::Iroh => SyncTransportKind::Iroh,
        }
    }

    pub fn build(self) -> Box<dyn TwoInstanceTransport> {
        match self {
            Self::Relay => Box::new(RelayTransport::new()),
            Self::Iroh => Box::new(IrohTransport::new()),
        }
    }
}

/// One side of the pair. Which instance publishes flips per round, and each
/// side owns its own transport-level state (a sync session on the relay, an
/// advertiser and a dialing endpoint on iroh).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Owner,
    Receiver,
}

/// Everything one bounded round needs. Both wires get the same request, so a
/// divergence between them is a property of the transport, not of the call.
pub struct RoundRequest<'a> {
    pub publisher: &'a ContainerRegistry,
    pub consumer: &'a ContainerRegistry,
    /// Which side publishes. The consumer is the other one.
    pub publisher_side: Side,
    /// Loro peer id of the publishing side (the relay envelope's sender).
    pub sender: StablePeerId,
    /// The owner-issued grant, or `None` while nothing is shared.
    pub grant: Option<MembershipChain>,
    pub clock: &'a TestClock,
}

/// What one round did, in the vocabulary both wires can honestly answer.
#[derive(Debug, Default, Clone)]
pub struct RoundOutcome {
    pub containers_visited: usize,
    pub pushed: usize,
    pub imported: usize,
    pub refusals: Vec<String>,
    pub unmounted: Vec<String>,
    pub unauthorized: Vec<String>,
}

/// Cumulative proof that the wire itself ran. Read off the transport, never
/// derived from what the round claims it did.
#[derive(Debug, Default, Clone, Copy)]
pub struct WireCounters {
    pub consultations: u64,
    pub envelopes: usize,
    pub connections_opened: u64,
    pub bytes_on_wire: u64,
}

#[async_trait::async_trait(?Send)]
pub trait TwoInstanceTransport: Send + Sync {
    fn kind(&self) -> SyncTransportKind;

    /// Drive ONE bounded round. Every failure is an `Err` — a transport that
    /// swallowed its own errors would make convergence unfalsifiable.
    async fn round(&self, req: RoundRequest<'_>) -> Result<RoundOutcome>;

    fn wire(&self) -> WireCounters;
}

// ─── The model: in-process relay ──────────────────────────────────────

pub struct RelayTransport {
    relay: InMemoryRelay,
    /// One durable-enough sync position per side, held across rounds.
    sessions: tokio::sync::Mutex<BTreeMap<&'static str, SyncSession>>,
}

impl RelayTransport {
    pub fn new() -> Self {
        Self {
            relay: InMemoryRelay::new(),
            sessions: tokio::sync::Mutex::new(BTreeMap::new()),
        }
    }
}

impl Default for RelayTransport {
    fn default() -> Self {
        Self::new()
    }
}

fn side_key(side: Side) -> &'static str {
    match side {
        Side::Owner => "owner",
        Side::Receiver => "receiver",
    }
}

fn other(side: Side) -> Side {
    match side {
        Side::Owner => Side::Receiver,
        Side::Receiver => Side::Owner,
    }
}

#[async_trait::async_trait(?Send)]
impl TwoInstanceTransport for RelayTransport {
    fn kind(&self) -> SyncTransportKind {
        SyncTransportKind::Relay
    }

    async fn round(&self, req: RoundRequest<'_>) -> Result<RoundOutcome> {
        // The admitting side of THIS round: whichever side CONSUMES the push.
        // Both the audience stamped on the push and the identity the pull
        // verifies against must be that peer, or the owner ends up checking
        // the receiver's authorization instead of its own (D72.a).
        let admitter = Principal(
            match other(req.publisher_side) {
                Side::Receiver => RECEIVER_PRINCIPAL,
                Side::Owner => OWNER_PRINCIPAL,
            }
            .to_string(),
        );
        let auth = OutboundAuth {
            sender: req.sender,
            audience: admitter.clone(),
            epoch: 0,
            // Unshared: an EMPTY chain. `push_once` then publishes NOTHING and
            // reports every container as `unauthorized` — state must not reach
            // an untrusted relay under an unproven claim.
            chain: req
                .grant
                .clone()
                .unwrap_or_else(|| MembershipChain::new(Vec::new())),
        };
        let ctx = AcceptorContext {
            receiver: &admitter,
            clock: req.clock,
            verifier: &UnverifiedVerifier,
        };

        let mut sessions = self.sessions.lock().await;
        let mut pub_session = sessions
            .remove(side_key(req.publisher_side))
            .unwrap_or_default();
        let mut con_session = sessions
            .remove(side_key(other(req.publisher_side)))
            .unwrap_or_default();

        let push = push_once(req.publisher, &self.relay, &mut pub_session, &auth)
            .await
            .context("push_once surfaces transport failure as Err; the slice must not hide it")?;
        let pull = pull_once(req.consumer, &self.relay, &mut con_session, &ctx)
            .await
            .context("pull_once surfaces transport failure as Err; the slice must not hide it")?;

        sessions.insert(side_key(req.publisher_side), pub_session);
        sessions.insert(side_key(other(req.publisher_side)), con_session);

        Ok(RoundOutcome {
            containers_visited: push.containers_visited.max(pull.containers_visited),
            pushed: push.pushed.len(),
            imported: pull.imported.len(),
            refusals: pull
                .refusals
                .iter()
                .map(|(c, d)| format!("{c}: {d:?}"))
                .collect(),
            unmounted: pull.unmounted.iter().map(ToString::to_string).collect(),
            unauthorized: push.unauthorized.iter().map(ToString::to_string).collect(),
        })
    }

    fn wire(&self) -> WireCounters {
        WireCounters {
            consultations: self.relay.witness().total(),
            envelopes: self.relay.stored_envelopes(),
            connections_opened: 0,
            bytes_on_wire: 0,
        }
    }
}

// ─── Production: replicate_all over live iroh endpoints ───────────────

/// The advertiser + dialing endpoint one side owns, created on that side's
/// first round and kept alive for the rest of the case.
///
/// Both live for the whole case on purpose. The advertiser must, because
/// `start_share_gated` refuses to advertise the same container twice — so
/// `replicate_all` runs ONCE per side and later rounds are dials against the
/// addresses it returned, which is also how a shipping device behaves. The
/// dialing endpoint must, because the acceptor's roster pins peers by their
/// QUIC fingerprint: a fresh endpoint per round would enroll a new "device"
/// every time and exhaust the peer cap.
struct SideWire {
    advertiser: IrohAdvertiser,
    advertised: Vec<(String, EndpointAddr)>,
    dialer: Option<Endpoint>,
}

impl SideWire {
    fn new() -> Self {
        Self {
            advertiser: IrohAdvertiser::new(),
            advertised: Vec::new(),
            dialer: None,
        }
    }
}

struct IrohState {
    owner: SideWire,
    receiver: SideWire,
    /// The share's real capability. The consumer presents it only while the
    /// model says the container is shared.
    capability: CapabilitySecret,
    roster: Option<holon_loro::iroh_advertiser::SharedRoster>,
}

pub struct IrohTransport {
    state: tokio::sync::Mutex<IrohState>,
    dials: AtomicU64,
    connections: AtomicU64,
    bytes: AtomicU64,
}

impl IrohTransport {
    pub fn new() -> Self {
        Self {
            state: tokio::sync::Mutex::new(IrohState {
                owner: SideWire::new(),
                receiver: SideWire::new(),
                capability: CapabilitySecret::generate(),
                roster: None,
            }),
            dials: AtomicU64::new(0),
            connections: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        }
    }
}

impl Default for IrohTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait(?Send)]
impl TwoInstanceTransport for IrohTransport {
    fn kind(&self) -> SyncTransportKind {
        SyncTransportKind::Iroh
    }

    async fn round(&self, req: RoundRequest<'_>) -> Result<RoundOutcome> {
        let mut state = self.state.lock().await;

        // `replicate_all` takes ONE roster for the whole replication set, but a
        // `ShareRoster` binds ONE `shared_tree_id` and the enrollment proof is
        // bound to it too — so a second container could never authenticate.
        // Fail loudly rather than advertise something no peer can enroll into.
        let set = req.publisher.replication_set().await?;
        if set.len() != 1 || set[0].id != ROOT_CONTAINER_ID {
            return Err(anyhow!(
                "the iroh leg replicates the root container only; `replicate_all`'s single-roster \
                 signature cannot gate {} container(s) ({:?}) because the enrollment proof binds \
                 one shared_tree_id",
                set.len(),
                set.iter().map(|c| c.id.clone()).collect::<Vec<_>>()
            ));
        }

        if state.roster.is_none() {
            let roster = Arc::new(tokio::sync::Mutex::new(ShareRoster::new(
                ROOT_CONTAINER_ID,
                state.capability.clone(),
                ExpiryTime(chrono::Utc::now().timestamp() + IROH_ROSTER_TTL_SECS),
                IROH_ROSTER_MAX_PEERS,
            )));
            state.roster = Some(roster);
        }
        let roster = state.roster.clone().expect("roster minted above");

        // ── Publisher: the PRODUCTION call. No filter, no per-doc predicate.
        {
            let side = match req.publisher_side {
                Side::Owner => &mut state.owner,
                Side::Receiver => &mut state.receiver,
            };
            if side.advertised.is_empty() {
                side.advertised = req
                    .publisher
                    .replicate_all(&side.advertiser, roster.clone())
                    .await
                    .context("replicate_all failed to advertise the replication set")?;
            }
        }
        let advertised = match req.publisher_side {
            Side::Owner => state.owner.advertised.clone(),
            Side::Receiver => state.receiver.advertised.clone(),
        };

        // ── Consumer: one long-lived dialing endpoint, ALPNs from what the
        // publisher advertised.
        let consumer_side = other(req.publisher_side);
        let dialer = {
            let side = match consumer_side {
                Side::Owner => &mut state.owner,
                Side::Receiver => &mut state.receiver,
            };
            match &side.dialer {
                Some(ep) => ep.clone(),
                None => {
                    let alpns = advertised
                        .iter()
                        .map(|(id, _)| make_alpn(ALPN_PREFIX, id))
                        .collect();
                    let ep = create_endpoint(alpns)
                        .await
                        .context("creating the consumer's iroh dialing endpoint")?;
                    side.dialer = Some(ep.clone());
                    ep
                }
            }
        };

        // Unshared rounds present an UNPROVABLE capability: the wire runs, the
        // acceptor refuses, and nothing crosses. That is the iroh analog of the
        // relay's empty membership chain.
        let presented = match req.grant {
            Some(_) => state.capability.clone(),
            None => CapabilitySecret::generate(),
        };
        let authorized = req.grant.is_some();

        let consumer_set = req.consumer.replication_set().await?;
        let mut outcome = RoundOutcome {
            containers_visited: advertised.len().max(consumer_set.len()),
            ..RoundOutcome::default()
        };

        for (id, addr) in &advertised {
            let container = consumer_set.iter().find(|c| &c.id == id).ok_or_else(|| {
                anyhow!(
                    "the publisher advertises container `{id}` but the consumer has no such \
                     container mounted; the two-instance slice replicates a shared set"
                )
            })?;
            let alpn = make_alpn(ALPN_PREFIX, id);
            // ALLOW(loro_doc_escape): the iroh sync adapter owns the document
            // for the length of the dial and imports on its own thread, so it
            // needs the raw handle — a long-lived transport site. The
            // watermark reads around it go through the doc boundary.
            let doc = container.doc.doc();
            let before = container.doc.with_read(|d| Ok(d.oplog_vv()))?;

            self.dials.fetch_add(1, Ordering::SeqCst);
            let dial = tokio::time::timeout(
                DIAL_BUDGET,
                sync_doc_initiate_enrolled(&dialer, &doc, &alpn, addr.clone(), &presented, id),
            )
            .await
            .with_context(|| {
                format!("dialing container `{id}` did not finish within {DIAL_BUDGET:?}")
            })?;

            match dial {
                Ok(conn) => {
                    self.connections.fetch_add(1, Ordering::SeqCst);
                    let stats = conn.stats();
                    self.bytes.fetch_add(
                        stats.udp_tx.bytes.saturating_add(stats.udp_rx.bytes),
                        Ordering::SeqCst,
                    );
                    // A direct link has no store-and-forward step: what the
                    // publisher offered and what the consumer took are the same
                    // event, observed as the consumer's oplog advancing.
                    if container.doc.with_read(|d| Ok(d.oplog_vv()))? != before {
                        outcome.pushed += 1;
                        outcome.imported += 1;
                    }
                }
                Err(e) if !authorized && e.downcast_ref::<EnrollmentRefused>().is_some() => {
                    // ONLY the acceptor's typed refusal counts. Every failure
                    // past `connect()` carries the same "enrollment" wording,
                    // so a substring test would let a 10s timeout or a dropped
                    // connection stand in for a security decision — the exact
                    // false green this witness exists to prevent.
                    self.connections.fetch_add(1, Ordering::SeqCst);
                    outcome.unauthorized.push(id.clone());
                    outcome.refusals.push(format!("{id}: {e:#}"));
                }
                Err(e) if !authorized => {
                    return Err(e).with_context(|| {
                        format!(
                            "the unauthorized dial of `{id}` failed WITHOUT the acceptor refusing \
                             it, so nothing about authorization was decided"
                        )
                    });
                }
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("an AUTHORIZED dial of container `{id}` failed"));
                }
            }
        }

        Ok(outcome)
    }

    fn wire(&self) -> WireCounters {
        WireCounters {
            consultations: self.dials.load(Ordering::SeqCst),
            envelopes: 0,
            connections_opened: self.connections.load(Ordering::SeqCst),
            bytes_on_wire: self.bytes.load(Ordering::SeqCst),
        }
    }
}
