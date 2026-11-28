// Binds the supervised home_by fold, its write-back consumer and the block
// feed's delivery cursor to docs/Testing/ModelChecking/HomeBySettle.tla: each
// drawn run records one spec action per step, and tla-checker replays the
// recording against the spec in process.

use std::cell::Cell;
use std::cell::RefCell;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;

use proptest::test_runner::Config;
use proptest::test_runner::TestRunner;
use tokio::sync::mpsc::UnboundedSender;

use super::*;
use crate::delivery_ledger::DeliveryLedger;
use crate::delivery_ledger::Seq;
use crate::delivery_ledger::SettleVerdict;
use crate::delivery_ledger::ViewKey;
use crate::delivery_ledger::settle_verdict;
use crate::live_data::DeliveryCursor;
use crate::live_data::reporting_cursor;
use crate::live_data::reporting_to;
use crate::streaming::Batch;
use crate::streaming::BatchMetadata;
use crate::streaming::BatchWithMetadata;
use crate::streaming::WithMetadata;

fn model_dir() -> std::path::PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    Path::new(&manifest).join("../../docs/Testing/ModelChecking")
}

/// The reference authority behind a hold: while held, every read parks, so a
/// poll leaves the fold inside its burst.
struct HeldAuthority {
    inner: TestAuthority,
    hold: AtomicBool,
    blocked: AtomicBool,
}

impl HeldAuthority {
    async fn gate(&self) {
        std::future::poll_fn(|_| {
            if self.hold.load(SeqCst) {
                self.blocked.store(true, SeqCst);
                Poll::Pending
            } else {
                self.blocked.store(false, SeqCst);
                Poll::Ready(())
            }
        })
        .await
    }
}

#[async_trait]
impl HomeAuthority<String> for HeldAuthority {
    type Memo = ();

    async fn locate(&self, id: &str, m: &mut ()) -> Result<Option<Placement<String>>> {
        self.gate().await;
        self.inner.locate(id, m).await
    }
    async fn children_of(&self, parent: Option<&str>, m: &mut ()) -> Result<Vec<String>> {
        self.gate().await;
        self.inner.children_of(parent, m).await
    }
    async fn prev_sibling(&self, id: &str, m: &mut ()) -> Result<Option<String>> {
        self.gate().await;
        self.inner.prev_sibling(id, m).await
    }
    async fn subtree_of(&self, id: &str, m: &mut ()) -> Result<Vec<String>> {
        self.gate().await;
        self.inner.subtree_of(id, m).await
    }
    async fn locate_batch(
        &self,
        ids: &[String],
        m: &mut (),
    ) -> Result<BTreeMap<String, Placement<String>>> {
        self.gate().await;
        self.inner.locate_batch(ids, m).await
    }
}

/// The spec's actions, named as in its `Next`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Action {
    FeedChange,
    PumpHeld,
    Pump,
    Take,
    DeliverCursor,
    SettleRead,
}

impl Action {
    const ALL: [Action; 6] = [
        Action::FeedChange,
        Action::PumpHeld,
        Action::Pump,
        Action::Take,
        Action::DeliverCursor,
        Action::SettleRead,
    ];
}

/// One drawn step: an action, with the feed edit a `FeedChange` makes.
#[derive(Debug, Clone)]
enum Drawn {
    FeedChange(Op),
    PumpHeld,
    Pump,
    Take,
    DeliverCursor,
    SettleRead,
}

impl Drawn {
    fn action(&self) -> Action {
        match self {
            Drawn::FeedChange(_) => Action::FeedChange,
            Drawn::PumpHeld => Action::PumpHeld,
            Drawn::Pump => Action::Pump,
            Drawn::Take => Action::Take,
            Drawn::DeliverCursor => Action::DeliverCursor,
            Drawn::SettleRead => Action::SettleRead,
        }
    }
}

fn drawn_strategy() -> impl Strategy<Value = Drawn> {
    prop_oneof![
        8 => op_strategy().prop_map(Drawn::FeedChange),
        2 => Just(Drawn::PumpHeld),
        3 => Just(Drawn::Pump),
        3 => Just(Drawn::Take),
        2 => Just(Drawn::DeliverCursor),
        2 => Just(Drawn::SettleRead),
    ]
}

/// The spec's variables after a step. Kept by the test: `taken` (the fold's
/// seq is private) and `chan` while the channel is not empty (its items are
/// unread). `mid` is read from the authority double; every other field from
/// the code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Abs {
    feed: u64,
    taken: u64,
    mid: bool,
    busy: bool,
    chan: u64,
    cons: u64,
    applied: u64,
    returned: bool,
}

impl Abs {
    fn primed(&self) -> String {
        let b = |v: bool| if v { "TRUE" } else { "FALSE" };
        format!(
            "feed' = {} /\\ taken' = {} /\\ mid' = {} /\\ busy' = {} /\\ chan' = {} /\\ cons' = {} \
             /\\ applied' = {} /\\ returned' = {}",
            self.feed,
            self.taken,
            b(self.mid),
            b(self.busy),
            self.chan,
            self.cons,
            self.applied,
            b(self.returned)
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct Step {
    action: Action,
    state: Abs,
}

#[derive(Debug, Clone, Default)]
struct Trace {
    init: Abs,
    steps: Vec<Step>,
}

type HomedEmission = Supervised<Homed<String, Item>>;

const VIEW: &str = "home-by-settle-spec";

/// The fold under supervision, fed by the production `LiveData::subscribe`
/// actor and drained by a consumer shaped like the org write-back one.
struct Slice {
    rt: tokio::runtime::Runtime,
    tree: Arc<Mutex<Tree>>,
    feed: Arc<LiveData<Item>>,
    batches: UnboundedSender<BatchWithMetadata<Change<StorageEntity>>>,
    authority: Arc<HeldAuthority>,
    progress: Arc<HomeByProgress>,
    cursor: Arc<DeliveryCursor>,
    supervisor: Pin<Box<dyn Future<Output = ()>>>,
    rx: UnboundedReceiver<HomedEmission>,
    folded: Folded,
    emitted: u64,
    taken: u64,
    chan: u64,
    cons: u64,
    trace: Trace,
}

impl Slice {
    /// `feed_taken`: a subscription took the write-back cursor, as the matview
    /// manager does for a reporting subscription.
    fn boot(feed_taken: bool) -> Self {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime builds");
        let tree = Arc::new(Mutex::new(Tree::new()));
        let authority = Arc::new(HeldAuthority {
            inner: TestAuthority::new(tree.clone()),
            hold: AtomicBool::new(false),
            blocked: AtomicBool::new(false),
        });
        let feed = test_feed();
        seed_root(&tree, &feed);
        // Every block starts in the snapshot, so most drawn ops change the
        // feed.
        for k in 0..BLOCKS.len() as u8 {
            step(
                &tree,
                &feed,
                &Op::Set {
                    k,
                    parent: 0,
                    after: 0,
                    content: 0,
                },
            );
        }
        let (batches, batch_rx) = tokio::sync::mpsc::unbounded_channel();
        {
            let _in_runtime = rt.enter();
            feed.subscribe(
                VIEW,
                tokio_stream::wrappers::UnboundedReceiverStream::new(batch_rx),
            );
        }
        let progress = Arc::new(HomeByProgress::default());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<HomedEmission>();
        let (f, a, p) = (feed.clone(), authority.clone(), progress.clone());
        let supervisor: Pin<Box<dyn Future<Output = ()>>> = Box::pin(run_supervised(
            VIEW,
            move || f.home_by_with_progress(a.clone(), p.clone()),
            tx,
            |component, restarts, err: &anyhow::Error| {
                panic!("{component} gave up after {restarts} restarts: {err:#}")
            },
            tokio_util::sync::CancellationToken::new(),
        ));
        let mut slice = Self {
            rt,
            tree,
            feed,
            batches,
            authority,
            progress,
            cursor: Arc::new(DeliveryCursor::default()),
            supervisor,
            rx,
            folded: Folded::new(),
            emitted: 0,
            taken: 0,
            chan: 0,
            cons: 0,
            trace: Trace::default(),
        };
        if feed_taken {
            let cursor = slice.cursor.clone();
            slice
                .rt
                .block_on(reporting_to(cursor, async { reporting_cursor() }))
                .expect("reporting_to scopes a cursor");
        }
        slice.poll();
        match slice.rx.try_recv() {
            Ok(Supervised::Reset) => {}
            other => panic!("the supervisor did not open with Reset: {other:?}"),
        }
        slice.take();
        let init = slice.observe(false);
        assert_eq!(
            init,
            Abs::default(),
            "boot left the slice outside the spec's Init"
        );
        slice.trace.init = init;
        slice
    }

    fn poll(&mut self) {
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(
            self.supervisor.as_mut().poll(&mut cx).is_pending(),
            "the supervisor finished"
        );
    }

    fn observe(&self, returned: bool) -> Abs {
        Abs {
            feed: self.feed.consumed_seq(),
            taken: self.taken,
            mid: self.authority.blocked.load(SeqCst),
            busy: self.progress.is_folding(),
            chan: if self.rx.is_empty() { 0 } else { self.chan },
            cons: self.cons,
            applied: self.cursor.applied(),
            returned,
        }
    }

    /// One CDC batch: the router routes it to the write-back cursor, then the
    /// subscribe actor applies it. `false` when the op changes nothing.
    fn feed_change(&mut self, op: &Op) -> bool {
        let mut tree = self.tree.lock().unwrap().clone();
        let Some(diff) = to_diff(&mut tree, op) else {
            return false;
        };
        // A removal the fold has not caught up with leaves it a diff the
        // authority no longer knows; the fold then dies and restarts
        // (Supervised::Reset), which the spec does not model.
        let removes = match &diff {
            MapDiff::Insert { .. } | MapDiff::Update { .. } => false,
            MapDiff::Remove { .. } | MapDiff::Clear {} => true,
            MapDiff::Replace { entries } => self
                .feed
                .read()
                .keys()
                .any(|k| !entries.iter().any(|(e, _)| e == k)),
        };
        let fold_idle =
            self.feed.consumed_seq() == self.taken && !self.authority.blocked.load(SeqCst);
        if removes && !fold_idle {
            return false;
        }
        *self.tree.lock().unwrap() = tree;
        self.emitted += 1;
        let seq = self.emitted;
        self.cursor.routed(seq);
        let items = changes_of(&self.feed, diff);
        self.batches
            .send(WithMetadata {
                inner: Batch { items },
                metadata: BatchMetadata {
                    seq,
                    ..Default::default()
                },
            })
            .expect("the subscribe actor is alive");
        let feed = self.feed.clone();
        self.rt.block_on(async move {
            for _ in 0..1000 {
                if feed.consumed_seq() >= seq {
                    return;
                }
                tokio::task::yield_now().await;
            }
            panic!("the subscribe actor did not apply batch {seq}");
        });
        true
    }

    fn pump(&mut self, hold: bool) {
        self.authority.hold.store(hold, SeqCst);
        let (len, mid) = (self.rx.len(), self.authority.blocked.load(SeqCst));
        self.poll();
        let grew = self.rx.len() > len;
        if grew || self.authority.blocked.load(SeqCst) != mid {
            self.taken = self.feed.consumed_seq();
        }
        if grew {
            self.chan = self.taken;
        }
    }

    /// The org write-back consumer: fold every diff, remember the marker.
    fn take(&mut self) {
        while let Ok(item) = self.rx.try_recv() {
            match item {
                Supervised::Diff(Homed::Diff(diff)) => {
                    fold_emitted(&mut self.folded, vec![Ok(diff)])
                }
                Supervised::Diff(Homed::Through(seq)) => self.cons = seq,
                Supervised::Reset => panic!("a supervised restart is outside the spec"),
            }
        }
    }

    /// The session's settle verdict (`settle_verdict`) over the values the
    /// code gives: the ledger the matview manager builds from the write-back
    /// cursor answers `complete_through_for`. `owes_empty` is the write-back
    /// stage's answer: the slice writes no documents, so the caller chooses it.
    fn verdict(&self, owes_empty: bool) -> SettleVerdict {
        let view = ViewKey {
            name: VIEW.into(),
            instance: Arc::as_ptr(&self.cursor) as usize as u64,
        };
        let mut ledger = DeliveryLedger::default();
        ledger.emitted(Seq(self.emitted));
        self.cursor.feed(&mut ledger, view.clone(), true);
        settle_verdict(
            self.cursor.is_taken(),
            Some(self.emitted),
            || ledger.complete_through_view(&view),
            || owes_empty,
        )
    }

    fn settled(&self) -> bool {
        self.verdict(true) == SettleVerdict::Settled
    }

    /// Runs one step; `false` when a `FeedChange` changes nothing (no step).
    fn act(&mut self, drawn: &Drawn) -> bool {
        let mut returned = false;
        match drawn {
            Drawn::FeedChange(op) => {
                if !self.feed_change(op) {
                    return false;
                }
            }
            Drawn::PumpHeld => self.pump(true),
            Drawn::Pump => self.pump(false),
            Drawn::Take => self.take(),
            Drawn::DeliverCursor => self.cursor.applied_through(self.cons),
            Drawn::SettleRead => returned = self.settled(),
        }
        let state = self.observe(returned);
        self.trace.steps.push(Step {
            action: drawn.action(),
            state,
        });
        true
    }
}

/// The CDC changes that turn the feed's content into `diff`'s result.
fn changes_of(
    feed: &LiveData<Item>,
    diff: MapDiff<String, Arc<Item>>,
) -> Vec<Change<StorageEntity>> {
    let origin = || ChangeOrigin::Local {
        operation_id: None,
        trace_id: None,
    };
    let created = |key: String, value: &Item| Change::Created {
        data: StorageEntity::from([
            (Arc::from("id"), Value::String(key)),
            (
                Arc::from("content"),
                Value::Integer(i64::from(value.content)),
            ),
        ]),
        origin: origin(),
    };
    let deleted = |id: String| Change::Deleted {
        id,
        origin: origin(),
    };
    let live: Vec<String> = feed.read().keys().cloned().collect();
    match diff {
        MapDiff::Insert { key, value } | MapDiff::Update { key, value } => {
            vec![created(key, &value)]
        }
        MapDiff::Remove { key } => vec![deleted(key)],
        MapDiff::Clear {} => live.into_iter().map(deleted).collect(),
        MapDiff::Replace { entries } => {
            let keep: BTreeSet<String> = entries.iter().map(|(k, _)| k.clone()).collect();
            live.into_iter()
                .filter(|k| !keep.contains(k))
                .map(deleted)
                .chain(entries.into_iter().map(|(k, v)| created(k, &v)))
                .collect()
        }
    }
}

struct SpecModel {
    prepared: tla_checker::load::Prepared,
    defs: tla_checker::eval::Definitions,
}

impl SpecModel {
    fn load(tla: &Path, cfg: &Path) -> std::result::Result<Self, String> {
        let prepared = tla_checker::load::prepare_from_path(tla, Some(cfg), &[])
            .map_err(|e| format!("tla-checker cannot load {}: {e}", tla.display()))?;
        let defs = tla_checker::scenario::build_definitions(&prepared.spec);
        Ok(Self { prepared, defs })
    }

    fn home_by_settle() -> Self {
        let dir = model_dir();
        Self::load(
            &dir.join("HomeBySettle.tla"),
            &dir.join("HomeBySettle_trace.cfg"),
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    /// The names of the action definitions `Next` is a disjunction of.
    fn actions(&self) -> BTreeSet<String> {
        use tla_checker::ast::Expr;
        let next = self
            .prepared
            .spec
            .next
            .as_ref()
            .expect("the spec has a Next");
        let mut out = BTreeSet::new();
        let mut disjuncts = vec![next];
        while let Some(e) = disjuncts.pop() {
            if let Expr::Or(a, b) = e {
                disjuncts.extend([a.as_ref(), b.as_ref()]);
                continue;
            }
            let name = self
                .defs
                .iter()
                .find(|(_, (params, body))| params.is_empty() && body.as_ref() == e)
                .map(|(name, _)| name.to_string())
                .unwrap_or_else(|| panic!("a disjunct of Next is no action definition: {e:?}"));
            out.insert(name);
        }
        out
    }
}

#[derive(Debug)]
struct StepRefused {
    /// 1-based: step 1 is the first action after `Init`.
    index: usize,
    action: Action,
    before: Abs,
    after: Abs,
    allowed: Vec<String>,
    message: String,
}

#[derive(Debug)]
enum Refusal {
    Step(Box<StepRefused>),
    /// The tool could not parse or evaluate: no verdict.
    Tool(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Step(s) => write!(
                f,
                "step {} ({:?}) is not a step of HomeBySettle: {}\n  before: {:?}\n  after:  {:?}\n  \
                 the spec allows: {:?}",
                s.index, s.action, s.message, s.before, s.after, s.allowed
            ),
            Refusal::Tool(m) => write!(f, "no verdict: {m}"),
        }
    }
}

fn conforms(spec: &SpecModel, trace: &Trace) -> std::result::Result<(), Refusal> {
    let text: String = trace
        .steps
        .iter()
        .map(|s| format!("action: {:?}; {}\n", s.action, s.state.primed()))
        .collect();
    let scenario = tla_checker::scenario::parse_scenario(&text)
        .map_err(|e| Refusal::Tool(format!("tla-checker cannot parse the recorded trace: {e}")))?;
    let result = tla_checker::scenario::execute_scenario_with(
        &spec.prepared.spec,
        &scenario,
        &spec.prepared.domains,
        &spec.defs,
    )
    .map_err(|e| Refusal::Tool(format!("tla-checker cannot evaluate the trace: {e:?}")))?;
    let Some(failure) = result.failure else {
        return Ok(());
    };
    let i = failure.step_index;
    Err(Refusal::Step(Box::new(StepRefused {
        index: i + 1,
        action: trace.steps[i].action,
        before: if i == 0 {
            trace.init
        } else {
            trace.steps[i - 1].state
        },
        after: trace.steps[i].state,
        allowed: failure.available_actions,
        message: failure.message,
    })))
}

/// `HOME_BY_SPEC_CHECK=off` skips the replay, to show what the check alone
/// catches.
fn check_on() -> bool {
    match std::env::var("HOME_BY_SPEC_CHECK") {
        Err(std::env::VarError::NotPresent) => true,
        Ok(v) if v == "off" => false,
        other => panic!("HOME_BY_SPEC_CHECK must be unset or off, is {other:?}"),
    }
}

fn static_examples() -> Vec<Vec<Drawn>> {
    let b1 = Drawn::FeedChange(Op::Set {
        k: 0,
        parent: 0,
        after: 0,
        content: 1,
    });
    let b2 = Drawn::FeedChange(Op::Set {
        k: 1,
        parent: 0,
        after: 0,
        content: 2,
    });
    vec![
        // A feed change the fold has not taken, then a settle read.
        vec![b1.clone(), Drawn::SettleRead],
        // A settle read while the fold is parked inside its burst.
        vec![
            b1.clone(),
            Drawn::PumpHeld,
            Drawn::SettleRead,
            Drawn::Pump,
            Drawn::Take,
            Drawn::DeliverCursor,
            Drawn::SettleRead,
        ],
        // A batch that changes nothing starts a burst that reads no authority.
        vec![
            Drawn::FeedChange(Op::Clear),
            Drawn::Pump,
            Drawn::FeedChange(Op::Clear),
            Drawn::PumpHeld,
        ],
        // The consumer drains before, between and after two emissions.
        vec![
            Drawn::Take,
            b1,
            Drawn::Pump,
            Drawn::Take,
            b2,
            Drawn::Pump,
            Drawn::Take,
            Drawn::DeliverCursor,
            Drawn::SettleRead,
        ],
    ]
}

/// Runs the static examples, then the drawn cases, over a write-back cursor
/// that a subscription took when `feed_taken`: `each_step` after every step,
/// `at_end` once per run. Prints how often each spec action ran.
fn run_slice_of(
    test: &str,
    feed_taken: bool,
    each_step: impl Fn(&Slice) -> std::result::Result<(), String>,
    at_end: impl Fn(&Slice) -> std::result::Result<(), String>,
) {
    let counts = RefCell::new(BTreeMap::<Action, u64>::new());
    let runs = Cell::new(0u64);
    let run = |drawn: &[Drawn], every_step_counts: bool| -> std::result::Result<(), String> {
        runs.set(runs.get() + 1);
        let mut slice = Slice::boot(feed_taken);
        for (i, d) in drawn.iter().enumerate() {
            if !slice.act(d) {
                if every_step_counts {
                    return Err(format!("step {} ({d:?}) changed nothing", i + 1));
                }
                continue;
            }
            *counts.borrow_mut().entry(d.action()).or_default() += 1;
            each_step(&slice)?;
        }
        at_end(&slice)
    };
    for (i, example) in static_examples().iter().enumerate() {
        if let Err(e) = run(example, true) {
            panic!("{test}: static example {i} {example:?}:\n{e}");
        }
    }
    let mut runner = TestRunner::new(Config {
        failure_persistence: None,
        ..Config::default()
    });
    let result = runner.run(&prop::collection::vec(drawn_strategy(), 1..30), |drawn| {
        run(&drawn, false).map_err(TestCaseError::fail)
    });
    eprintln!(
        "ACTION_COUNTS {test} runs={} {:?}",
        runs.get(),
        counts.borrow()
    );
    if let Err(e) = result {
        panic!("{test}: {e}");
    }
}

/// Every run of the code is a behaviour of the spec.
#[test]
fn every_run_is_a_behaviour_of_the_spec() {
    let spec = SpecModel::home_by_settle();
    let check = check_on();
    eprintln!("SPEC_CHECK {}", if check { "on" } else { "off" });
    run_slice_of(
        "conformance",
        true,
        |_| Ok(()),
        |slice| {
            if !check {
                return Ok(());
            }
            conforms(&spec, &slice.trace)
                .map_err(|e| format!("{e}\nrecorded trace: {:#?}", slice.trace))
        },
    );
}

/// SettleProperties!SettledReadsNewest on the code: a settle read that finds
/// nothing owed sees the consumer hold the newest documents.
fn a_quiet_settle_read_sees_the_newest_documents(test: &str, feed_taken: bool) {
    let quiet_after_change = Cell::new(0u64);
    run_slice_of(
        test,
        feed_taken,
        |slice| {
            let last = slice.trace.steps.last().expect("a step was recorded");
            if !last.state.returned {
                return Ok(());
            }
            if last.state.feed > 0 {
                quiet_after_change.set(quiet_after_change.get() + 1);
            }
            let (consumer, authority) =
                (observed(&slice.folded, &slice.tree), reference(&slice.tree));
            if consumer != authority || last.state.cons != last.state.feed {
                return Err(format!(
                    "a settle read found nothing owed, but the consumer is behind\n  consumer: \
                     {consumer:?}\n  authority: {authority:?}\n  trace: {:#?}",
                    slice.trace
                ));
            }
            Ok(())
        },
        |_| Ok(()),
    );
    eprintln!("QUIET_READS_AFTER_A_CHANGE {}", quiet_after_change.get());
    assert!(
        quiet_after_change.get() > 0,
        "no settle read found nothing owed after a feed change: the property held vacuously"
    );
}

#[test]
fn a_quiet_settle_read_of_an_untaken_feed_sees_the_newest_documents() {
    a_quiet_settle_read_sees_the_newest_documents("untaken feed", false);
}

#[test]
fn a_quiet_settle_read_of_a_taken_feed_sees_the_newest_documents() {
    a_quiet_settle_read_sees_the_newest_documents("taken feed", true);
}

#[test]
fn the_action_alphabet_is_the_spec_alphabet() {
    let code: BTreeSet<String> = Action::ALL.iter().map(|a| format!("{a:?}")).collect();
    let spec = SpecModel::home_by_settle().actions();
    eprintln!("ALPHABET code={code:?} spec={spec:?}");
    assert_eq!(code, spec, "the action enum and the spec's Next differ");
}

/// A spec the tool cannot parse or evaluate yields an error, never a
/// verdict.
#[test]
fn a_spec_the_tool_cannot_read_is_an_error() {
    let dir = std::env::temp_dir().join(format!("home-by-settle-spec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("Model.cfg");
    std::fs::write(&cfg, "INIT Init\nNEXT Next\n").unwrap();
    let trace = Trace {
        init: Abs::default(),
        steps: vec![Step {
            action: Action::FeedChange,
            state: Abs {
                feed: 1,
                ..Abs::default()
            },
        }],
    };
    let cases = [
        ("Unparsable", "VARIABLES x\nInit == x =\nNext == x' = x\n"),
        (
            "Unevaluable",
            "VARIABLES x\nInit == x = 0\nNext == x' = CHOOSE y \\in {} : TRUE\n",
        ),
    ];
    for (name, body) in cases {
        let tla = dir.join(format!("{name}.tla"));
        std::fs::write(&tla, format!("---- MODULE {name} ----\n{body}====\n")).unwrap();
        let verdict = match SpecModel::load(&tla, &cfg) {
            Err(e) => Refusal::Tool(e),
            Ok(spec) => match conforms(&spec, &trace) {
                Err(refusal) => refusal,
                Ok(()) => panic!("{name}: the tool read the spec as conforming"),
            },
        };
        match verdict {
            Refusal::Tool(m) => eprintln!("TOOL_ERROR {name}: {m}"),
            Refusal::Step(s) => panic!("{name}: a tool failure read as a refused step: {s:?}"),
        }
    }
}

/// A settle read with a write-back still handed over does not return, whatever
/// the feed stage says: it waits on the feed when the feed is behind and on
/// the write-back when it is not.
fn a_writeback_that_owes_holds_a_settle_read_open(test: &str, feed_taken: bool) {
    let feed_settled_reads = Cell::new(0u64);
    run_slice_of(
        test,
        feed_taken,
        |slice| {
            let last = slice.trace.steps.last().expect("a step was recorded");
            if last.action != Action::SettleRead {
                return Ok(());
            }
            let owing = slice.verdict(false);
            if last.state.returned {
                feed_settled_reads.set(feed_settled_reads.get() + 1);
                if owing != SettleVerdict::WritebackOwes {
                    return Err(format!(
                        "the feed stage settled but a write-back is handed over: {owing:?}"
                    ));
                }
            } else if !matches!(owing, SettleVerdict::FeedBehind(_)) {
                return Err(format!(
                    "the feed stage is behind but the verdict is {owing:?}"
                ));
            }
            Ok(())
        },
        |_| Ok(()),
    );
    eprintln!("FEED_SETTLED_READS {}", feed_settled_reads.get());
    assert!(
        feed_settled_reads.get() > 0,
        "no settle read found the feed settled: the write-back arm was never reached"
    );
}

#[test]
fn a_writeback_that_owes_holds_a_settle_read_of_an_untaken_feed_open() {
    a_writeback_that_owes_holds_a_settle_read_open("owes untaken feed", false);
}

#[test]
fn a_writeback_that_owes_holds_a_settle_read_of_a_taken_feed_open() {
    a_writeback_that_owes_holds_a_settle_read_open("owes taken feed", true);
}
