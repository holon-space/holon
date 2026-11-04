//! Paired own devices replicating the root container over iroh converge on
//! every edit, whatever saves, restarts and history compactions happened in
//! between, and each device's compaction floor keeps up with its peers.
//!
//! Each peer is a `LoroDocumentStore` on its own dir: its saves and restarts
//! run the production persistence path, whose first snapshot of a session is
//! history-compacted below the version every recorded peer holds. Sync is
//! `ContainerRegistry::replicate_all` on the acceptor and
//! `sync_doc_initiate_enrolled` on the dialer, as a paired device runs them.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use holon_api::sharing::Capabilities;
use holon_conditions::ConditionBus;
use holon_conditions::ConditionKind;
use holon_loro::ContainerRegistry;
use holon_loro::DocScope;
use holon_loro::GLOBAL_SNAPSHOT_NAME;
use holon_loro::LoroDocument;
use holon_loro::LoroDocumentStore;
use holon_loro::WriteOrigin;
use holon_loro::container_registry::ROOT_CONTAINER_ID;
use holon_loro::iroh_advertiser::ALPN_PREFIX;
use holon_loro::iroh_advertiser::Endpoint;
use holon_loro::iroh_advertiser::EndpointAddr;
use holon_loro::iroh_advertiser::IrohAdvertiser;
use holon_loro::iroh_advertiser::SharedRoster;
use holon_loro::iroh_sync_adapter::create_endpoint;
use holon_loro::iroh_sync_adapter::disclose_sync_refusal;
use holon_loro::iroh_sync_adapter::make_alpn;
use holon_loro::iroh_sync_adapter::sync_doc_initiate_enrolled;
use holon_loro::peer_versions::Hello;
use holon_loro::share_enrollment::CapabilitySecret;
use holon_loro::share_enrollment::ExpiryTime;
use holon_loro::share_enrollment::ShareRoster;
use loro::VersionVector;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;
use serial_test::serial;
use tempfile::TempDir;

const EDITS: &str = "edits";
const LEDGER_FILE: &str = "peer-versions.json";
const DIAL_BUDGET: Duration = Duration::from_secs(20);

/// Which device pairs sync with each other.
#[derive(Clone, Copy, Debug)]
enum Topology {
    Single,
    Pair,
    Star3,
    Chain3,
    Cycle3,
}

impl Topology {
    fn peers(self) -> usize {
        match self {
            Topology::Single => 1,
            Topology::Pair => 2,
            Topology::Star3 | Topology::Chain3 | Topology::Cycle3 => 3,
        }
    }

    fn edges(self) -> &'static [(usize, usize)] {
        match self {
            Topology::Single => &[],
            Topology::Pair => &[(0, 1)],
            Topology::Star3 => &[(0, 1), (0, 2)],
            Topology::Chain3 => &[(0, 1), (1, 2)],
            Topology::Cycle3 => &[(0, 1), (1, 2), (2, 0)],
        }
    }
}

#[derive(Clone, Debug)]
enum Op {
    Edit {
        peer: usize,
    },
    Save {
        peer: usize,
    },
    Restart {
        peer: usize,
    },
    /// `to` dials `from`.
    Sync {
        from: usize,
        to: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Expect {
    /// No import is refused and every edit reaches every peer.
    Converge,
    /// A device that joined without adopting the trimmed history is refused
    /// with a condition, and keeps its own edits.
    Refused,
}

#[derive(Clone, Debug)]
struct Case {
    topology: Topology,
    /// Peers `0..adopted` share peer 0's history from the start, as a paired
    /// device does after adoption; the others start an independent history.
    adopted: usize,
    ops: Vec<Op>,
    expect: Expect,
}

impl Case {
    fn adopted(topology: Topology, ops: Vec<Op>) -> Self {
        Self {
            topology,
            adopted: topology.peers(),
            ops,
            expect: Expect::Converge,
        }
    }
}

/// Fixed cases run before the random ones; each is a failure the generator
/// found or a shape worth pinning.
fn static_examples() -> Vec<Case> {
    let independent = |ops| Case {
        topology: Topology::Pair,
        adopted: 1,
        ops,
        expect: Expect::Converge,
    };
    vec![
        // Peer 1 restarts after a concurrent edit of peer 0; with no recorded
        // peer it must not trim.
        independent(vec![
            Op::Edit { peer: 1 },
            Op::Edit { peer: 0 },
            Op::Restart { peer: 1 },
        ]),
        independent(vec![Op::Restart { peer: 1 }, Op::Edit { peer: 0 }]),
        independent(vec![Op::Edit { peer: 0 }, Op::Restart { peer: 0 }]),
        // The same shape after adoption: peer 1 records peer 0 at the
        // adopted version and trims at most down to it.
        Case::adopted(
            Topology::Pair,
            vec![
                Op::Edit { peer: 1 },
                Op::Edit { peer: 0 },
                Op::Restart { peer: 1 },
            ],
        ),
        // B (1) trims after syncing A (0); C's (2) edit is concurrent to that
        // trim and reaches B only through A.
        Case::adopted(
            Topology::Star3,
            vec![
                Op::Edit { peer: 0 },
                Op::Sync { from: 0, to: 1 },
                Op::Restart { peer: 1 },
                Op::Edit { peer: 2 },
                Op::Sync { from: 0, to: 2 },
                Op::Sync { from: 0, to: 1 },
            ],
        ),
        // The ledger written before a restart drives the compaction after it.
        Case::adopted(
            Topology::Pair,
            vec![
                Op::Edit { peer: 1 },
                Op::Sync { from: 0, to: 1 },
                Op::Restart { peer: 1 },
                Op::Edit { peer: 0 },
                Op::Restart { peer: 1 },
            ],
        ),
        // Peer 2 never syncs during the ops: the others trim no history above
        // its last version, so its late edit still imports everywhere.
        Case::adopted(
            Topology::Star3,
            vec![
                Op::Edit { peer: 2 },
                Op::Sync { from: 0, to: 1 },
                Op::Restart { peer: 0 },
                Op::Restart { peer: 1 },
                Op::Edit { peer: 0 },
                Op::Sync { from: 0, to: 1 },
                Op::Restart { peer: 0 },
                Op::Restart { peer: 1 },
                Op::Edit { peer: 2 },
            ],
        ),
        // Peer 0 trims below peer 1's hello, whose head is concurrent to peer
        // 0's own edit; the trimmed snapshot must still load on restart.
        Case::adopted(
            Topology::Pair,
            vec![
                Op::Edit { peer: 0 },
                Op::Edit { peer: 1 },
                Op::Sync { from: 0, to: 1 },
                Op::Restart { peer: 0 },
            ],
        ),
        // A device with no recorded peer never trims.
        Case::adopted(
            Topology::Single,
            vec![
                Op::Edit { peer: 0 },
                Op::Restart { peer: 0 },
                Op::Edit { peer: 0 },
                Op::Restart { peer: 0 },
            ],
        ),
        // Three concurrent first changes with no merge above them; the full
        // sync and the compacting restarts of `check` follow. The trim below
        // all three has no common ancestor to start at.
        Case::adopted(
            Topology::Cycle3,
            vec![
                Op::Edit { peer: 0 },
                Op::Edit { peer: 1 },
                Op::Edit { peer: 2 },
            ],
        ),
        // The same roots with a merge above them: the trim starts at it.
        Case::adopted(
            Topology::Cycle3,
            vec![
                Op::Edit { peer: 0 },
                Op::Edit { peer: 1 },
                Op::Edit { peer: 2 },
                Op::Sync { from: 1, to: 0 },
                Op::Sync { from: 2, to: 0 },
                Op::Edit { peer: 0 },
            ],
        ),
        // Peer 2 joins after peer 0 trimmed, without adopting its history.
        Case {
            topology: Topology::Star3,
            adopted: 2,
            ops: vec![
                Op::Edit { peer: 0 },
                Op::Sync { from: 0, to: 1 },
                Op::Sync { from: 1, to: 0 },
                Op::Restart { peer: 0 },
                Op::Edit { peer: 2 },
                Op::Sync { from: 0, to: 2 },
            ],
            expect: Expect::Refused,
        },
    ]
}

/// Every device is adopted: production pairs only by adoption, and pairing
/// with a trimmed owner is refused (`pairing_refuses_trimmed_owner`). Syncs
/// run only along the topology's edges, in either direction. The adopted
/// history is empty, so the drawn first changes before any op are concurrent
/// roots.
fn case_strategy() -> impl Strategy<Value = Case> {
    prop_oneof![
        Just(Topology::Pair),
        Just(Topology::Star3),
        Just(Topology::Chain3),
        Just(Topology::Cycle3),
    ]
    .prop_flat_map(|topology| {
        let peers = topology.peers();
        let edges = topology.edges();
        let op = prop_oneof![
            3 => (0..peers).prop_map(|peer| Op::Edit { peer }),
            1 => (0..peers).prop_map(|peer| Op::Save { peer }),
            1 => (0..peers).prop_map(|peer| Op::Restart { peer }),
            2 => (0..edges.len(), any::<bool>()).prop_map(move |(e, flip)| {
                let (a, b) = edges[e];
                if flip { Op::Sync { from: b, to: a } } else { Op::Sync { from: a, to: b } }
            }),
        ];
        (
            // Weighted so that most cases start from a concurrent root on every peer.
            prop::collection::vec(prop::bool::weighted(0.8), peers),
            prop::collection::vec(op, 0..12),
        )
            .prop_map(move |(first, ops)| {
                let first = (0..peers)
                    .filter(|&p| first[p])
                    .map(|peer| Op::Edit { peer });
                Case::adopted(topology, first.chain(ops).collect())
            })
    })
}

struct Peer {
    dir: TempDir,
    id: u64,
    bus: Arc<ConditionBus>,
    store: LoroDocumentStore,
    doc: Arc<LoroDocument>,
    advertiser: IrohAdvertiser,
    addr: EndpointAddr,
    dialer: Endpoint,
    /// Every edit this peer authored, by key.
    authored: Vec<String>,
}

impl Peer {
    async fn open(dir: TempDir, id: u64, roster: &SharedRoster) -> Result<Self> {
        let bus = Arc::new(ConditionBus::new());
        let store = LoroDocumentStore::new(dir.path().to_path_buf())
            .with_peer_id(Some(id))
            .with_condition_bus(bus.clone());
        let doc = store.get_doc(DocScope::Global).await?;
        let advertiser = IrohAdvertiser::new().with_degraded_bus(bus.clone());
        let addr = advertise(&store, &advertiser, roster).await?;
        let dialer = create_endpoint(vec![make_alpn(ALPN_PREFIX, ROOT_CONTAINER_ID)]).await?;
        Ok(Self {
            dir,
            id,
            bus,
            store,
            doc,
            advertiser,
            addr,
            dialer,
            authored: Vec::new(),
        })
    }

    /// Quit and relaunch: checkpoint, then a fresh store over the same dir.
    async fn restart(&mut self, roster: &SharedRoster) -> Result<()> {
        self.store.checkpoint().await?;
        self.advertiser.drop_share(ROOT_CONTAINER_ID).await?;
        self.store = LoroDocumentStore::new(self.dir.path().to_path_buf())
            .with_peer_id(Some(self.id))
            .with_condition_bus(self.bus.clone());
        self.doc = self.store.get_doc(DocScope::Global).await?;
        self.addr = advertise(&self.store, &self.advertiser, roster).await?;
        Ok(())
    }

    fn holds(&self, key: &str) -> Result<bool> {
        self.doc
            .with_read(|d| Ok(d.get_map(EDITS).get(key).is_some()))
    }

    fn version(&self) -> Result<VersionVector> {
        self.doc.with_read(|d| Ok(d.oplog_vv()))
    }

    fn trimmed_below(&self) -> Result<VersionVector> {
        self.doc.with_read(|d| Ok(d.shallow_since_vv().to_vv()))
    }

    fn is_shallow(&self) -> Result<bool> {
        self.doc.with_read(|d| Ok(d.is_shallow()))
    }

    fn heads(&self) -> Result<loro::Frontiers> {
        self.doc.with_read(|d| Ok(d.oplog_frontiers()))
    }

    fn trimmed_at(&self) -> Result<loro::Frontiers> {
        self.doc.with_read(|d| Ok(d.shallow_since_frontiers()))
    }

    /// Whether the snapshot file the last save wrote imports into a fresh doc.
    fn snapshot_loads(&self) -> Result<Result<(), String>> {
        let path = self.dir.path().join(GLOBAL_SNAPSHOT_NAME);
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        Ok(loro::LoroDoc::new()
            .import(&bytes)
            .map(|_| ())
            .map_err(|e| format!("a fresh doc refuses {}: {e}", path.display())))
    }

    fn known(&self) -> Result<BTreeMap<u64, VersionVector>> {
        Ok(self.store.peer_versions()?.known())
    }

    /// The version this device would compact below now, as persistence
    /// computes it; `None` when it would not compact.
    fn floor(&self) -> Result<Option<VersionVector>> {
        let vv = self.version()?;
        Ok(self
            .store
            .peer_versions()?
            .floor()
            .map(|floor| floor.intersection(&vv)))
    }

    fn refusals(&self) -> Vec<String> {
        self.bus
            .current()
            .into_iter()
            .filter(|c| matches!(c.reason, ConditionKind::PeerUpdateRefused { .. }))
            .map(|c| format!("{c:?}"))
            .collect()
    }
}

async fn advertise(
    store: &LoroDocumentStore,
    advertiser: &IrohAdvertiser,
    roster: &SharedRoster,
) -> Result<EndpointAddr> {
    let advertised = ContainerRegistry::new(store.clone())
        .replicate_all(advertiser, roster.clone())
        .await?;
    let [(id, addr)] = advertised.as_slice() else {
        anyhow::bail!("expected the root container alone, advertised {advertised:?}");
    };
    anyhow::ensure!(id == ROOT_CONTAINER_ID, "advertised {id}, not the root");
    Ok(addr.clone())
}

/// The joiner starts from the owner's stored history, without its ledger.
fn adopt(owner: &Peer, joiner_dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(owner.dir.path())? {
        let entry = entry?;
        anyhow::ensure!(
            entry.file_type()?.is_file(),
            "the store dir holds a non-file {}",
            entry.path().display()
        );
        if entry.file_name() != LEDGER_FILE {
            std::fs::copy(entry.path(), joiner_dir.join(entry.file_name()))
                .with_context(|| format!("copy {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Each side records the other as pairing does: the owner records the joiner
/// at an empty version, the joiner records the owner's hello.
fn record_pairing(owner: &Peer, joiner: &Peer) -> Result<()> {
    let owner_versions = owner.store.peer_versions()?;
    let joiner_versions = joiner.store.peer_versions()?;
    owner_versions.record(&Hello {
        device: Some(joiner_versions.device()),
        vv: VersionVector::new().encode(),
        known: Vec::new(),
    })?;
    joiner_versions.record(&owner_versions.hello(&owner.version()?))
}

/// `to` dials `from`; both sides import the other's delta and record the
/// other's hello. A failed dial is disclosed on the dialer's bus as the
/// production dialers do, and comes back as the dial's error.
async fn sync(peers: &[Peer], from: usize, to: usize, cap: &CapabilitySecret) -> Result<()> {
    let alpn = make_alpn(ALPN_PREFIX, ROOT_CONTAINER_ID);
    // A reference clone of the same doc, as the transport needs an owned handle.
    let doc = peers[to].doc.with_read(|d| Ok(Arc::new(d.clone())))?;
    let versions = peers[to].store.peer_versions()?;
    let dialed = tokio::time::timeout(
        DIAL_BUDGET,
        sync_doc_initiate_enrolled(
            &peers[to].dialer,
            &doc,
            &alpn,
            peers[from].addr.clone(),
            cap,
            ROOT_CONTAINER_ID,
            Capabilities::read_write(),
            Some(&versions),
        ),
    )
    .await
    .with_context(|| format!("peer {to} dialing peer {from} took over {DIAL_BUDGET:?}"))?;
    if let Err(e) = dialed {
        disclose_sync_refusal(&peers[to].bus, ROOT_CONTAINER_ID, &e);
        // The acceptor discloses its own refusal after the dialer returns.
        tokio::time::sleep(Duration::from_millis(200)).await;
        return Err(e);
    }
    Ok(())
}

/// The invariants that hold after every step: no device trimmed history some
/// paired device lacks, a device with no recorded peer is not trimmed.
fn trim_violations(peers: &[Peer], adopted: usize) -> Result<Vec<String>> {
    let mut violations = Vec::new();
    for (d, peer) in peers.iter().enumerate() {
        if peer.known()?.is_empty() && peer.is_shallow()? {
            violations.push(format!("peer {d} trimmed with no recorded peer"));
        }
        if d >= adopted {
            continue;
        }
        let trimmed = peer.trimmed_below()?;
        for (e, other) in peers.iter().enumerate().take(adopted) {
            if e != d && !other.version()?.includes_vv(&trimmed) {
                violations.push(format!(
                    "peer {d} trimmed below {trimmed:?}, which peer {e} at {:?} lacks",
                    other.version()?
                ));
            }
        }
    }
    Ok(violations)
}

struct Run {
    case: Case,
    compaction: bool,
    peers: Vec<Peer>,
    cap: CapabilitySecret,
    roster: SharedRoster,
    log: Vec<String>,
}

impl Run {
    async fn open(case: &Case, compaction: bool) -> Result<Self> {
        let cap = CapabilitySecret::generate();
        let roster: SharedRoster = Arc::new(tokio::sync::Mutex::new(ShareRoster::new(
            ROOT_CONTAINER_ID,
            cap.clone(),
            ExpiryTime(chrono::Utc::now().timestamp() + 3600),
            64,
        )));
        let mut peers = vec![Peer::open(TempDir::new()?, 1000, &roster).await?];
        if case.adopted > 1 {
            // A fresh session, so the owner's next restart is its first
            // snapshot of a session, as for every other peer.
            peers[0].restart(&roster).await?;
        }
        for p in 1..case.topology.peers() {
            let dir = TempDir::new()?;
            if p < case.adopted {
                adopt(&peers[0], dir.path())?;
            }
            peers.push(Peer::open(dir, 1000 + p as u64, &roster).await?);
            if p < case.adopted {
                record_pairing(&peers[0], &peers[p])?;
            }
        }
        Ok(Self {
            case: case.clone(),
            compaction,
            peers,
            cap,
            roster,
            log: Vec::new(),
        })
    }

    /// Runs `op`; an `Err` is a broken invariant.
    async fn step(&mut self, op: &Op) -> Result<Result<(), String>> {
        match *op {
            Op::Edit { peer } => {
                let key = format!("p{peer}-e{}", self.peers[peer].authored.len());
                self.peers[peer]
                    .doc
                    .with_write(WriteOrigin::BlockOps, |txn| {
                        txn.get_map(EDITS).insert(&key, true)?;
                        Ok(())
                    })?;
                self.peers[peer].authored.push(key);
            }
            Op::Save { peer } => {
                self.peers[peer].store.save_all().await?;
                if let Err(e) = self.peers[peer].snapshot_loads()? {
                    return Ok(Err(format!("after {op:?}: {e}")));
                }
            }
            Op::Restart { peer } => {
                let before = self.peers[peer].known()?;
                self.peers[peer].restart(&self.roster).await?;
                let after = self.peers[peer].known()?;
                if before != after {
                    return Ok(Err(format!(
                        "peer {peer}'s ledger changed across a restart: {before:?} -> {after:?}"
                    )));
                }
            }
            Op::Sync { from, to } => {
                if let Err(e) = sync(&self.peers, from, to, &self.cap).await {
                    self.log.push(format!("{op:?}: {e:#}"));
                    return Ok(Err(format!("{to} dialing {from} failed: {e:#}")));
                }
            }
        }
        let violations = trim_violations(&self.peers, self.case.adopted)?;
        if !violations.is_empty() {
            return Ok(Err(format!("after {op:?}: {violations:?}")));
        }
        Ok(Ok(()))
    }

    /// Every edge syncs once each way.
    async fn full_round(&mut self) -> Result<Result<(), String>> {
        for &(a, b) in self.case.topology.edges() {
            for op in [Op::Sync { from: a, to: b }, Op::Sync { from: b, to: a }] {
                if let Err(e) = self.step(&op).await? {
                    return Ok(Err(format!("full round: {e}")));
                }
            }
        }
        Ok(Ok(()))
    }

    fn floors(&self) -> Result<Vec<Option<VersionVector>>> {
        self.peers.iter().map(Peer::floor).collect()
    }

    fn report(&self) -> Result<String> {
        let mut lines = vec![format!("{:?}", self.case)];
        for (p, peer) in self.peers.iter().enumerate() {
            lines.push(format!(
                "peer {p}: shallow={} version={:?} floor={:?} known={:?} conditions={:?}",
                peer.is_shallow()?,
                peer.version()?,
                peer.floor()?,
                peer.known()?,
                peer.bus.current(),
            ));
        }
        lines.push(format!("sync errors: {:#?}", self.log));
        Ok(lines.join("\n"))
    }

    async fn check(&mut self) -> Result<Result<(), String>> {
        for op in self.case.ops.clone() {
            let stepped = self.step(&op).await?;
            match (self.case.expect, stepped) {
                (_, Ok(())) => {}
                (Expect::Refused, Err(_)) if matches!(op, Op::Sync { .. }) => {}
                (_, Err(e)) => return Ok(Err(e)),
            }
        }
        if self.case.expect == Expect::Refused {
            return self.check_refused();
        }

        // Enough rounds for an edit to cross the longest path.
        for _ in 1..self.case.topology.peers() {
            if let Err(e) = self.full_round().await? {
                return Ok(Err(e));
            }
        }
        let mut missing = Vec::new();
        for author in &self.peers {
            for key in &author.authored {
                for (p, peer) in self.peers.iter().enumerate() {
                    if !peer.holds(key)? {
                        missing.push(format!("{key} on peer {p}"));
                    }
                }
            }
        }
        if !missing.is_empty() {
            return Ok(Err(format!(
                "after a full sync these edits are missing: {missing:?}"
            )));
        }

        // Liveness: with no new edits, rounds of syncs lift every floor to
        // the converged version, and no floor moves down on the way.
        if self.case.topology.edges().is_empty() {
            return Ok(Ok(()));
        }
        let mut floors = self.floors()?;
        for _ in 0..2 * self.peers.len() + 1 {
            let reached = self
                .peers
                .iter()
                .zip(&floors)
                .map(|(peer, floor)| Ok(floor.as_ref() == Some(&peer.version()?)))
                .collect::<Result<Vec<bool>>>()?;
            if reached.iter().all(|r| *r) {
                return self.check_trim_advances().await;
            }
            if let Err(e) = self.full_round().await? {
                return Ok(Err(e));
            }
            let next = self.floors()?;
            for (p, (old, new)) in floors.iter().zip(&next).enumerate() {
                if let (Some(old), Some(new)) = (old, new)
                    && !new.includes_vv(old)
                {
                    return Ok(Err(format!(
                        "peer {p}'s floor moved down: {old:?} -> {new:?}"
                    )));
                }
            }
            floors = next;
        }
        Ok(Err(format!(
            "the floors never reached the converged version: {floors:?}"
        )))
    }

    /// With every floor at the converged version, a compaction save of a doc
    /// with a single head trims all history below it, and any compaction
    /// loads on restart. The second restart's checkpoint is its store's first
    /// save, so a compaction.
    async fn check_trim_advances(&mut self) -> Result<Result<(), String>> {
        if !self.compaction {
            return Ok(Ok(()));
        }
        for p in 0..self.peers.len() {
            let heads = self.peers[p].heads()?;
            for _ in 0..2 {
                self.peers[p].restart(&self.roster).await?;
            }
            let trimmed_at = self.peers[p].trimmed_at()?;
            if heads.len() == 1 && trimmed_at != heads {
                return Ok(Err(format!(
                    "peer {p}'s compaction at the single head {heads:?} trimmed only at {trimmed_at:?}"
                )));
            }
        }
        Ok(Ok(()))
    }

    fn check_refused(&self) -> Result<Result<(), String>> {
        let late = self.case.adopted;
        anyhow::ensure!(
            self.peers[0].is_shallow()?,
            "peer 0 did not trim before the late join; the example proves nothing"
        );
        if self.log.is_empty() {
            return Ok(Err("the late join synced without an error".into()));
        }
        let refusals: Vec<String> = self.peers.iter().flat_map(Peer::refusals).collect();
        if refusals.is_empty() {
            return Ok(Err(
                "the refused late join raised no refusal condition".into()
            ));
        }
        for key in &self.peers[late].authored {
            if !self.peers[late].holds(key)? {
                return Ok(Err(format!("the late joiner lost its own edit {key}")));
            }
        }
        Ok(Ok(()))
    }
}

async fn run(case: &Case, compaction: bool) -> Result<Result<(), String>> {
    let mut run = Run::open(case, compaction).await?;
    let mut verdict = run.check().await?;
    if verdict.is_ok() && case.expect == Expect::Converge {
        let refusals: Vec<String> = run.peers.iter().flat_map(Peer::refusals).collect();
        if !refusals.is_empty() {
            verdict = Err(format!("a refusal condition was raised: {refusals:?}"));
        }
    }
    Ok(match verdict {
        Ok(()) => Ok(()),
        Err(e) => Err(format!("{e}\n{}", run.report()?)),
    })
}

fn check(rt: &tokio::runtime::Runtime, case: &Case, compaction: bool) -> Result<(), TestCaseError> {
    rt.block_on(run(case, compaction))
        .map_err(|e| TestCaseError::fail(format!("harness error on {case:?}: {e:#}")))?
        .map_err(TestCaseError::fail)
}

fn every_edit_reaches_every_peer(compaction: bool) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let mut failures: Vec<String> = static_examples()
        .iter()
        // Without compaction nothing is trimmed, so nothing refuses a late join.
        .filter(|case| compaction || case.expect == Expect::Converge)
        .filter_map(|case| check(&rt, case, compaction).err())
        .map(|e| format!("static example: {e}"))
        .collect();
    let mut runner = TestRunner::new(ProptestConfig {
        cases: 8,
        max_shrink_iters: 32,
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    if let Err(e) = runner.run(&case_strategy(), |case| check(&rt, &case, compaction)) {
        failures.push(format!("generated (shrunk): {e}"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
#[serial(holon_loro_compact)]
fn every_edit_reaches_every_peer_with_history_compaction() {
    // SAFETY: serialized with the only other test that touches this variable.
    unsafe { std::env::remove_var("HOLON_LORO_COMPACT") };
    every_edit_reaches_every_peer(true);
}

/// The exclusion check: with compaction off the same property holds, so a red
/// in the test above comes from the compaction, not from the harness.
#[test]
#[serial(holon_loro_compact)]
fn every_edit_reaches_every_peer_without_history_compaction() {
    // SAFETY: serialized with the only other test that touches this variable.
    unsafe { std::env::set_var("HOLON_LORO_COMPACT", "off") };
    every_edit_reaches_every_peer(false);
    unsafe { std::env::remove_var("HOLON_LORO_COMPACT") };
}
