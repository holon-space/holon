//! Paired own devices replicating the root container over iroh converge on
//! every edit, whatever saves and restarts happened in between.
//!
//! Each peer is a `LoroDocumentStore` on its own dir: its saves and restarts
//! run the production persistence path, whose first snapshot of a session is
//! history-compacted. Sync is `ContainerRegistry::replicate_all` on the
//! acceptor and `sync_doc_initiate_enrolled` on the dialer, as a paired device
//! runs them.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use holon_api::sharing::Capabilities;
use holon_conditions::ConditionBus;
use holon_loro::ContainerRegistry;
use holon_loro::DocScope;
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
use holon_loro::iroh_sync_adapter::make_alpn;
use holon_loro::iroh_sync_adapter::sync_doc_initiate_enrolled;
use holon_loro::share_enrollment::CapabilitySecret;
use holon_loro::share_enrollment::ExpiryTime;
use holon_loro::share_enrollment::ShareRoster;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;
use serial_test::serial;
use tempfile::TempDir;

const EDITS: &str = "edits";
const DIAL_BUDGET: Duration = Duration::from_secs(20);

#[derive(Clone, Debug)]
enum Op {
    Edit { peer: usize },
    Save { peer: usize },
    Restart { peer: usize },
    Sync { from: usize, to: usize },
}

#[derive(Clone, Debug)]
struct Case {
    peers: usize,
    ops: Vec<Op>,
}

/// Fixed cases run before the random ones; each is a failure the generator
/// found or a shape worth pinning.
fn static_examples() -> Vec<Case> {
    vec![
        // Peer 1 compacts on restart; peer 0's concurrent edit can then never
        // be imported into it.
        Case {
            peers: 2,
            ops: vec![
                Op::Edit { peer: 1 },
                Op::Edit { peer: 0 },
                Op::Restart { peer: 1 },
            ],
        },
        // Shrunk by the generator: the compacted peer authored nothing, yet
        // the other peer's history (its schema init) is concurrent to the trim.
        Case {
            peers: 2,
            ops: vec![Op::Restart { peer: 1 }, Op::Edit { peer: 0 }],
        },
        Case {
            peers: 2,
            ops: vec![Op::Edit { peer: 0 }, Op::Restart { peer: 0 }],
        },
    ]
}

fn case_strategy() -> impl Strategy<Value = Case> {
    (2usize..=3).prop_flat_map(|peers| {
        let op = prop_oneof![
            3 => (0..peers).prop_map(|peer| Op::Edit { peer }),
            1 => (0..peers).prop_map(|peer| Op::Save { peer }),
            1 => (0..peers).prop_map(|peer| Op::Restart { peer }),
            2 => (0..peers, 0..peers)
                .prop_filter("a peer does not dial itself", |(a, b)| a != b)
                .prop_map(|(from, to)| Op::Sync { from, to }),
        ];
        prop::collection::vec(op, 0..12).prop_map(move |ops| Case { peers, ops })
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

    fn is_shallow(&self) -> Result<bool> {
        self.doc.with_read(|d| Ok(d.is_shallow()))
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

/// `to` dials `from`; both sides import the other's delta. A refused import
/// comes back as the dial's error.
async fn sync(peers: &[Peer], from: usize, to: usize, cap: &CapabilitySecret) -> Result<()> {
    let alpn = make_alpn(ALPN_PREFIX, ROOT_CONTAINER_ID);
    tokio::time::timeout(
        DIAL_BUDGET,
        sync_doc_initiate_enrolled(
            &peers[to].dialer,
            &peers[to].doc.doc(),
            &alpn,
            peers[from].addr.clone(),
            cap,
            ROOT_CONTAINER_ID,
            Capabilities::read_write(),
        ),
    )
    .await
    .with_context(|| format!("peer {to} dialing peer {from} took over {DIAL_BUDGET:?}"))??;
    // The acceptor imports after the dialer returns.
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(())
}

async fn run(case: &Case) -> Result<Result<(), String>> {
    let cap = CapabilitySecret::generate();
    let roster: SharedRoster = Arc::new(tokio::sync::Mutex::new(ShareRoster::new(
        ROOT_CONTAINER_ID,
        cap.clone(),
        ExpiryTime(chrono::Utc::now().timestamp() + 3600),
        64,
    )));
    let mut peers = Vec::new();
    for p in 0..case.peers {
        peers.push(Peer::open(TempDir::new()?, 1000 + p as u64, &roster).await?);
    }
    let mut sync_errors = Vec::new();
    for op in &case.ops {
        match *op {
            Op::Edit { peer } => {
                let key = format!("p{peer}-e{}", peers[peer].authored.len());
                peers[peer].doc.with_write(WriteOrigin::BlockOps, |txn| {
                    txn.get_map(EDITS).insert(&key, true)?;
                    Ok(())
                })?;
                peers[peer].authored.push(key);
            }
            Op::Save { peer } => peers[peer].store.save_all().await?,
            Op::Restart { peer } => peers[peer].restart(&roster).await?,
            Op::Sync { from, to } => {
                if let Err(e) = sync(&peers, from, to, &cap).await {
                    sync_errors.push(format!("during ops, {to} dialing {from}: {e:#}"));
                }
            }
        }
    }

    // Two full rounds: the second shows whether a refused delta is resent and
    // refused again.
    let mut full_sync_errors = vec![Vec::new(), Vec::new()];
    for round in &mut full_sync_errors {
        for from in 0..case.peers {
            for to in 0..case.peers {
                if from != to
                    && let Err(e) = sync(&peers, from, to, &cap).await
                {
                    round.push(format!("{to} dialing {from}: {e:#}"));
                }
            }
        }
    }

    let mut lost_by_author = Vec::new();
    let mut missing = Vec::new();
    for author in &peers {
        for key in &author.authored {
            if !author.holds(key)? {
                lost_by_author.push(key.clone());
            }
            for (p, peer) in peers.iter().enumerate() {
                if !peer.holds(key)? {
                    missing.push(format!("{key} on peer {p}"));
                }
            }
        }
    }
    let report = || -> Result<String> {
        let mut shallow = Vec::new();
        let mut conditions = Vec::new();
        for (p, peer) in peers.iter().enumerate() {
            shallow.push(format!("peer {p}: shallow={}", peer.is_shallow()?));
            for c in peer.bus.current() {
                conditions.push(format!("peer {p}: {c:?}"));
            }
        }
        Ok(format!(
            "{case:?}\nmissing: {missing:?}\n{}\nconditions raised: {conditions:?}\n\
             sync errors during ops: {sync_errors:#?}\n\
             full sync round 1 errors: {:#?}\nfull sync round 2 errors: {:#?}",
            shallow.join(", "),
            full_sync_errors[0],
            full_sync_errors[1],
        ))
    };
    if !lost_by_author.is_empty() {
        return Ok(Err(format!(
            "an author no longer holds its own edits {lost_by_author:?}\n{}",
            report()?
        )));
    }
    if !missing.is_empty() {
        return Ok(Err(format!(
            "after a full sync not every edit is on every peer\n{}",
            report()?
        )));
    }
    Ok(Ok(()))
}

fn check(rt: &tokio::runtime::Runtime, case: &Case) -> Result<(), TestCaseError> {
    rt.block_on(run(case))
        .map_err(|e| TestCaseError::fail(format!("harness error on {case:?}: {e:#}")))?
        .map_err(TestCaseError::fail)
}

fn every_edit_reaches_every_peer() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let mut failures: Vec<String> = static_examples()
        .iter()
        .filter_map(|case| check(&rt, case).err())
        .map(|e| format!("static example: {e}"))
        .collect();
    let mut runner = TestRunner::new(ProptestConfig {
        cases: 8,
        max_shrink_iters: 32,
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    if let Err(e) = runner.run(&case_strategy(), |case| check(&rt, &case)) {
        failures.push(format!("generated (shrunk): {e}"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
#[serial(holon_loro_compact)]
fn every_edit_reaches_every_peer_with_history_compaction() {
    // SAFETY: serialized with the only other test that touches this variable.
    unsafe { std::env::remove_var("HOLON_LORO_COMPACT") };
    every_edit_reaches_every_peer();
}

/// The exclusion check: with compaction off the same property holds, so a red
/// in the test above comes from the compaction, not from the harness.
#[test]
#[serial(holon_loro_compact)]
fn every_edit_reaches_every_peer_without_history_compaction() {
    // SAFETY: serialized with the only other test that touches this variable.
    unsafe { std::env::set_var("HOLON_LORO_COMPACT", "off") };
    every_edit_reaches_every_peer();
    unsafe { std::env::remove_var("HOLON_LORO_COMPACT") };
}
