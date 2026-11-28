// Binds the SqlOnly editor (EditorViewModel and evaluate_data_sync_echo) to
// docs/Testing/ModelChecking/Keystroke.tla: each run drives a real view model
// against a store double, records one spec action per step, and tla-checker
// replays the recording against the spec in process.
//
// The store double does what the set_field arm of SqlOperationProvider does to one
// text block: it stores the canonical text and updates write_seq only when
// the write carries one. Each row change reaches the editor's subscription.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use holon_api::Value;
use holon_api::content_canonical::canonicalize_stored_content;
use holon_api::render_types::OperationDescriptor;
use holon_api::render_types::OperationParam;
use holon_api::render_types::OperationWiring;
use holon_api::render_types::TypeHint;
use holon_api::types::EntityName;
use holon_frontend::echo::EchoDecision;
use holon_frontend::echo::evaluate_data_sync_echo;
use holon_frontend::editor_view_model::EditorAction;
use holon_frontend::editor_view_model::EditorViewModel;
use holon_frontend::operations::OperationIntent;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;

/// Keys and Spaces of Keystroke_trace_*.cfg.
const KEYS: u8 = 6;
const EXT_CHAR: char = 'x';
const EXT_KEY: u8 = 9;
const NO_SEQ: i64 = -1;

/// Key k types this character: the whitespace keys of the trace configs (3
/// and 6) type the two whitespace characters, every other key a letter.
fn key_char(k: u8) -> char {
    match k {
        3 => ' ',
        6 => '\t',
        _ => (b'a' + k - 1) as char,
    }
}

fn key_of(c: char) -> u8 {
    match c {
        ' ' => 3,
        '\t' => 6,
        EXT_CHAR => EXT_KEY,
        'a'..='f' => c as u8 - b'a' + 1,
        _ => panic!("{c:?} is no character of the spec's alphabet"),
    }
}

fn tla_text(s: &str) -> String {
    let keys: Vec<String> = s.chars().map(|c| key_of(c).to_string()).collect();
    format!("<<{}>>", keys.join(", "))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum External {
    None,
    WithSeq,
    WithoutSeq,
}

impl External {
    const ALL: [External; 3] = [External::None, External::WithSeq, External::WithoutSeq];

    fn cfg(self) -> &'static str {
        match self {
            External::WithSeq => "Keystroke_trace_fresh.cfg",
            External::None | External::WithoutSeq => "Keystroke_trace_sticky.cfg",
        }
    }
}

/// The intent the editor dispatched last before an echo decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Keystroke,
    Flush,
    Structural,
}

const DECISIONS: [&str; 5] = [
    "InSync",
    "DropStale",
    "DropNoSeq",
    "AdoptBaseline",
    "Converge",
];

fn decision_name(d: EchoDecision) -> &'static str {
    match d {
        EchoDecision::InSync { .. } => "InSync",
        EchoDecision::DropStale => "DropStale",
        EchoDecision::DropNoSeq => "DropNoSeq",
        EchoDecision::AdoptBaseline { .. } => "AdoptBaseline",
        EchoDecision::Converge { .. } => "Converge",
    }
}

/// One drawn step, named as the spec's action. `Deliver(i)` delivers the
/// i-th row change in transit (modulo their number): the subscription may
/// skip to a later change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drawn {
    Type,
    Deliver(u8),
    Blur,
    Chord,
    External,
    Apply,
    Stop,
}

fn drawn_strategy(external: External) -> BoxedStrategy<Drawn> {
    let ext_weight = if external == External::None { 0 } else { 1 };
    prop_oneof![
        4 => Just(Drawn::Type),
        4 => any::<u8>().prop_map(Drawn::Deliver),
        1 => Just(Drawn::Blur),
        1 => Just(Drawn::Chord),
        ext_weight => Just(Drawn::External),
        4 => Just(Drawn::Apply),
        1 => Just(Drawn::Stop),
    ]
    .boxed()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteKind {
    Key,
    Flush,
    Ext,
}

impl WriteKind {
    fn tla(self) -> &'static str {
        match self {
            WriteKind::Key => "key",
            WriteKind::Flush => "flush",
            WriteKind::Ext => "ext",
        }
    }
}

#[derive(Debug, Clone)]
struct Write {
    kind: WriteKind,
    value: String,
    seq: Option<i64>,
}

/// The spec state the trace constrains after a step, in spec terms.
#[derive(Debug, Clone)]
struct Abs {
    nk: u8,
    stopped: bool,
    mounted: bool,
    buf: String,
    last: i64,
    clock: i64,
    queue: Vec<(WriteKind, String, i64)>,
    sc: String,
    ss: i64,
    ech: Vec<(String, i64)>,
    ext_done: bool,
    dec: &'static str,
    sop_done: bool,
}

impl Abs {
    fn primed(&self) -> String {
        let b = |v: bool| if v { "TRUE" } else { "FALSE" };
        let mut parts = vec![
            format!("nk' = {}", self.nk),
            format!("stopped' = {}", b(self.stopped)),
            format!("mounted' = {}", b(self.mounted)),
            format!("buf' = {}", tla_text(&self.buf)),
            format!("last' = {}", self.last),
            format!("clock' = {}", self.clock),
            format!("Len(queue') = {}", self.queue.len()),
            format!("sc'[1] = {}", tla_text(&self.sc)),
            format!("ss'[1] = {}", self.ss),
            format!("extDone' = {}", b(self.ext_done)),
            format!("dec' = \"{}\"", self.dec),
            format!("sop' = \"{}\"", if self.sop_done { "done" } else { "none" }),
        ];
        for (i, (k, c, s)) in self.queue.iter().enumerate() {
            let i = i + 1;
            parts.push(format!("queue'[{i}].k = \"{}\"", k.tla()));
            if *k != WriteKind::Ext {
                parts.push(format!("queue'[{i}].c = {}", tla_text(c)));
            }
            parts.push(format!("queue'[{i}].s = {s}"));
        }
        let ech: Vec<String> = self
            .ech
            .iter()
            .map(|(c, s)| format!("[c |-> {}, s |-> {s}]", tla_text(c)))
            .collect();
        parts.push(format!("ech' = <<{}>>", ech.join(", ")));
        parts.join(" /\\ ")
    }
}

#[derive(Debug, Clone)]
struct Step {
    action: &'static str,
    state: Abs,
}

fn set_field_wiring() -> OperationWiring {
    let param = |name: &str| OperationParam {
        name: name.into(),
        type_hint: TypeHint::String,
        description: String::new(),
    };
    OperationWiring {
        modified_param: String::new(),
        descriptor: OperationDescriptor {
            entity_name: EntityName::new("block"),
            entity_short_name: "block".into(),
            name: "set_field".into(),
            display_name: "set_field".into(),
            required_params: vec![param("id"), param("field"), param("value")],
            optional_params: vec![],
            affected_fields: vec!["content".into()],
            id_column: "id".to_string(),
            description: String::new(),
            param_mappings: vec![],
            target_scope: holon_api::TargetScope::Block,
            boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
            menu_exposure: holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Test,
            },
            trigger: None,
            bound_params: Default::default(),
            marking_delta: holon_api::marking::MarkingDelta::Undeclared,
            guard: holon_api::pattern::OpGuard::None,
            arcs: holon_api::arcs::TransitionArcs::Undeclared,
        },
    }
}

/// One focused editor on one block, the store double and the row changes in
/// transit to the editor.
struct World {
    vm: EditorViewModel,
    external: External,
    nk: u8,
    stopped: bool,
    mounted: bool,
    clock: i64,
    /// write_seq values the code allocated -> the spec's clock values.
    seqs: HashMap<i64, i64>,
    queue: Vec<Write>,
    sc: String,
    ss: i64,
    ech: Vec<(String, Option<i64>)>,
    ext_done: bool,
    dec: &'static str,
    sop_done: bool,
    last_kind: Kind,
    /// (decision, last intent kind) of every echo decided in this run.
    decided: Vec<(&'static str, Kind)>,
    steps: Vec<Step>,
}

impl World {
    fn new(external: External) -> Self {
        let context = HashMap::from([("id".to_string(), Value::String("block:k1".into()))]);
        let vm = EditorViewModel::new(
            vec![set_field_wiring()],
            Vec::new(),
            context,
            "content".into(),
            String::new(),
        );
        Self {
            vm,
            external,
            nk: 0,
            stopped: false,
            mounted: true,
            clock: 0,
            seqs: HashMap::from([(0, 0)]),
            queue: Vec::new(),
            sc: String::new(),
            ss: 0,
            // the subscription's first emission is the row as mounted
            ech: vec![(String::new(), Some(0))],
            ext_done: false,
            dec: "none",
            sop_done: false,
            last_kind: Kind::Keystroke,
            decided: Vec::new(),
            steps: Vec::new(),
        }
    }

    fn spec_seq(&self, seq: i64) -> i64 {
        *self
            .seqs
            .get(&seq)
            .unwrap_or_else(|| panic!("write_seq {seq} was never allocated in this run"))
    }

    /// The spec's clock ticks once for each Type, BlurAway, Structural and
    /// ExternalWrite; a write_seq the code allocates is named by that tick.
    fn tick(&mut self, allocated: Option<i64>) {
        self.clock += 1;
        if let Some(seq) = allocated {
            assert!(
                self.seqs.insert(seq, self.clock).is_none(),
                "write_seq {seq} was allocated twice"
            );
        }
    }

    fn write_of(&self, intent: OperationIntent, kind: WriteKind) -> Write {
        assert_eq!(intent.op_name, "set_field", "{intent:?}");
        assert_eq!(
            intent.params.get("field"),
            Some(&Value::String("content".into()))
        );
        let value = match intent.params.get("value") {
            Some(Value::String(v)) => v.clone(),
            other => panic!("set_field value is {other:?}"),
        };
        let seq = intent.params.get("write_seq").map(|v| match v {
            Value::Integer(s) => *s,
            other => panic!("write_seq is {other:?}"),
        });
        Write { kind, value, seq }
    }

    fn observe(&self) -> Abs {
        let seq = |s: Option<i64>| s.map_or(NO_SEQ, |s| self.spec_seq(s));
        Abs {
            nk: self.nk,
            stopped: self.stopped,
            mounted: self.mounted,
            buf: self.vm.buffer().to_string(),
            last: self.spec_seq(self.vm.last_local_seq()),
            clock: self.clock,
            queue: self
                .queue
                .iter()
                .map(|w| (w.kind, w.value.clone(), seq(w.seq)))
                .collect(),
            sc: self.sc.clone(),
            ss: self.spec_seq(self.ss),
            ech: self.ech.iter().map(|(c, s)| (c.clone(), seq(*s))).collect(),
            ext_done: self.ext_done,
            dec: self.dec,
            sop_done: self.sop_done,
        }
    }

    /// Runs one step; `None` when the spec's guard for it is false here.
    fn act(&mut self, drawn: Drawn) -> Option<&'static str> {
        let action = match drawn {
            Drawn::Type => {
                if self.stopped || !self.mounted || self.nk == KEYS {
                    return None;
                }
                self.nk += 1;
                let text = format!("{}{}", self.vm.buffer(), key_char(self.nk));
                let intent = self
                    .vm
                    .apply_local_edit(&text)
                    .expect("a keystroke into a SqlOnly block is accepted")
                    .expect("a changed buffer of a SqlOnly block writes");
                let write = self.write_of(intent, WriteKind::Key);
                self.tick(Some(
                    write.seq.expect("a keystroke write carries write_seq"),
                ));
                self.queue.push(write);
                self.last_kind = Kind::Keystroke;
                "Type"
            }
            Drawn::Deliver(i) => {
                if self.ech.is_empty() {
                    return None;
                }
                let i = i as usize % self.ech.len();
                let (content, seq) = self.ech[i].clone();
                self.ech.drain(..=i);
                let decision = evaluate_data_sync_echo(
                    self.vm.buffer(),
                    &content,
                    seq,
                    self.vm.last_local_seq(),
                );
                self.dec = decision_name(decision);
                self.decided.push((self.dec, self.last_kind));
                if let Some(directive) = self.vm.converge_from_data_sync(&content, seq) {
                    // the adapter's converge_input
                    let seq = self.vm.last_local_seq();
                    self.vm.set_buffer_from_authority(&directive.target, seq);
                }
                "DeliverEcho"
            }
            Drawn::Blur => {
                if self.stopped || !self.mounted {
                    return None;
                }
                let live = self.vm.buffer().to_string();
                match self.vm.on_blur(&live) {
                    EditorAction::Execute(intent) => {
                        let write = self.write_of(intent, WriteKind::Flush);
                        self.queue.push(write);
                    }
                    EditorAction::None => {}
                    other => panic!("blur returned {other:?}"),
                }
                self.tick(None);
                self.mounted = false;
                self.ech.clear();
                self.last_kind = Kind::Flush;
                "BlurAway"
            }
            Drawn::Chord => {
                if self.stopped || !self.mounted || self.sop_done {
                    return None;
                }
                let live = self.vm.buffer().to_string();
                if let Some(intent) = self.vm.chord_commit_intent(&live) {
                    let write = self.write_of(intent, WriteKind::Flush);
                    self.queue.push(write);
                }
                self.tick(None);
                self.sop_done = true;
                self.last_kind = Kind::Structural;
                "Structural"
            }
            Drawn::External => {
                if self.external == External::None || self.ext_done || !self.mounted {
                    return None;
                }
                let seq = (self.external == External::WithSeq)
                    .then(|| holon_api::write_seq::next().get());
                self.tick(seq);
                self.queue.push(Write {
                    kind: WriteKind::Ext,
                    value: String::new(),
                    seq,
                });
                self.ext_done = true;
                "ExternalWrite"
            }
            Drawn::Apply => {
                if self.queue.is_empty() {
                    return None;
                }
                let write = self.queue.remove(0);
                let (content, seq) = match write.kind {
                    WriteKind::Key | WriteKind::Flush => (
                        canonicalize_stored_content(&write.value, false),
                        write.seq.unwrap_or(self.ss),
                    ),
                    WriteKind::Ext => (
                        format!("{}{EXT_CHAR}", self.sc),
                        write.seq.unwrap_or(self.ss),
                    ),
                };
                let changed = content != self.sc || seq != self.ss;
                self.sc = content;
                self.ss = seq;
                if changed && self.mounted {
                    self.ech.push((self.sc.clone(), Some(self.ss)));
                }
                "Apply"
            }
            Drawn::Stop => {
                if self.stopped {
                    return None;
                }
                self.stopped = true;
                "Stop"
            }
        };
        let state = self.observe();
        self.steps.push(Step { action, state });
        Some(action)
    }
}

fn model_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    Path::new(&manifest).join("../../docs/Testing/ModelChecking")
}

struct SpecModel {
    prepared: tla_checker::load::Prepared,
    defs: tla_checker::eval::Definitions,
}

impl SpecModel {
    fn load(cfg: &str) -> Self {
        let dir = model_dir();
        let prepared = tla_checker::load::prepare_from_path(
            &dir.join("Keystroke.tla"),
            Some(&dir.join(cfg)),
            &[],
        )
        .unwrap_or_else(|e| panic!("tla-checker cannot load Keystroke.tla with {cfg}: {e}"));
        let defs = tla_checker::scenario::build_definitions(&prepared.spec);
        Self { prepared, defs }
    }

    /// `Err` names the first step the spec does not allow.
    fn conforms(&self, steps: &[Step]) -> Result<(), String> {
        let text: String = steps
            .iter()
            .map(|s| format!("action: {}; {}\n", s.action, s.state.primed()))
            .collect();
        let scenario = tla_checker::scenario::parse_scenario(&text)
            .unwrap_or_else(|e| panic!("tla-checker cannot parse the recorded trace: {e}\n{text}"));
        let result = tla_checker::scenario::execute_scenario_with(
            &self.prepared.spec,
            &scenario,
            &self.prepared.domains,
            &self.defs,
        )
        .unwrap_or_else(|e| panic!("tla-checker cannot evaluate the trace: {e:?}\n{text}"));
        let Some(failure) = result.failure else {
            return Ok(());
        };
        let i = failure.step_index;
        Err(format!(
            "step {} ({}) is not a step of Keystroke: {}\n  after: {:?}\n  the spec allows: {:#?}\n  \
             trace:\n{text}",
            i + 1,
            steps[i].action,
            failure.message,
            steps[i].state,
            failure.available_actions
        ))
    }
}

use Drawn::Apply as A;
use Drawn::Chord as C;
use Drawn::Deliver as D;
use Drawn::External as X;
use Drawn::Type as T;

/// Fixed runs, one or more for every reachable (decision, last intent kind,
/// external write) cell; `unreachable_cells` names the rest.
fn fixed_cases() -> Vec<(External, Vec<Drawn>)> {
    let mut cases = Vec::new();
    for external in External::ALL {
        // keystroke: the own echo (InSync), the seed echo after a key
        // (DropStale), a trailing whitespace key (AdoptBaseline)
        cases.push((external, vec![T, A, D(1)]));
        cases.push((external, vec![T, D(0)]));
        cases.push((external, vec![T, T, T, A, A, A, D(3)]));
        // structural: the same echoes decided after the chord
        cases.push((external, vec![T, A, C, D(1)]));
        cases.push((external, vec![T, C, D(0)]));
        cases.push((external, vec![T, T, T, A, A, A, C, D(3)]));
    }
    for external in [External::WithSeq, External::WithoutSeq] {
        // an external write that lands after the last key converges
        cases.push((external, vec![T, A, D(1), X, A, D(0)]));
        cases.push((external, vec![T, A, D(1), C, X, A, D(0)]));
        // the lost update: a key typed before the external echo arrives
        cases.push((external, vec![T, A, D(1), X, A, T, A, D(0), D(0)]));
        cases.push((external, vec![T, X, T, A, A, A, D(0), D(0), D(0), D(0)]));
    }
    cases
}

/// Cells no run of the current design reaches, with the reason.
fn unreachable_cells() -> BTreeMap<(&'static str, Kind, External), &'static str> {
    let mut out = BTreeMap::new();
    for external in External::ALL {
        for d in DECISIONS {
            out.insert(
                (d, Kind::Flush, external),
                "a blur unmounts the editor and drops its subscription",
            );
        }
        for kind in [Kind::Keystroke, Kind::Structural] {
            out.insert(
                ("DropNoSeq", kind, external),
                "every row carries write_seq (NOT NULL DEFAULT 0, projected by every block source)",
            );
        }
    }
    for kind in [Kind::Keystroke, Kind::Structural] {
        out.insert(
            ("Converge", kind, External::None),
            "the editor's own writes echo with a seq below or at last_local_seq and the trimmed text",
        );
    }
    out
}

fn run(
    spec: &SpecModel,
    external: External,
    drawn: &[Drawn],
    strict: bool,
) -> Result<World, String> {
    let mut world = World::new(external);
    for (i, d) in drawn.iter().enumerate() {
        if world.act(*d).is_none() && strict {
            return Err(format!("fixed step {} ({d:?}) is not enabled", i + 1));
        }
    }
    spec.conforms(&world.steps)?;
    Ok(world)
}

/// `KEYSTROKE_RANDOM_CASES` sets the number of drawn runs per external class.
fn random_cases() -> u32 {
    match std::env::var("KEYSTROKE_RANDOM_CASES") {
        Err(std::env::VarError::NotPresent) => 256,
        Ok(v) => v
            .parse()
            .unwrap_or_else(|e| panic!("KEYSTROKE_RANDOM_CASES={v}: {e}")),
        Err(e) => panic!("KEYSTROKE_RANDOM_CASES: {e}"),
    }
}

/// Every run of the editor and its echo decision is a behaviour of the spec.
#[test]
fn every_editor_run_is_a_behaviour_of_the_keystroke_spec() {
    let specs: BTreeMap<&str, SpecModel> = External::ALL
        .iter()
        .map(|e| (e.cfg(), SpecModel::load(e.cfg())))
        .collect();
    let mut fixed_cells = BTreeSet::new();
    let fixed = fixed_cases();
    for (i, (external, drawn)) in fixed.iter().enumerate() {
        let world = run(&specs[external.cfg()], *external, drawn, true)
            .unwrap_or_else(|e| panic!("fixed case {i} {external:?} {drawn:?}:\n{e}"));
        fixed_cells.extend(world.decided.iter().map(|(d, k)| (*d, *k, *external)));
    }
    let unreachable = unreachable_cells();
    let mut all = BTreeSet::new();
    for d in DECISIONS {
        for k in [Kind::Keystroke, Kind::Flush, Kind::Structural] {
            for e in External::ALL {
                all.insert((d, k, e));
            }
        }
    }
    let reachable: BTreeSet<_> = all
        .iter()
        .filter(|c| !unreachable.contains_key(*c))
        .copied()
        .collect();
    eprintln!(
        "FIXED_CASES {} cells: {} of {} reachable, {} unreachable",
        fixed.len(),
        fixed_cells.len(),
        reachable.len(),
        unreachable.len()
    );
    assert_eq!(
        fixed_cells, reachable,
        "the fixed cases do not reach exactly the reachable cells"
    );

    let random_cells = std::cell::RefCell::new(BTreeSet::new());
    let runs = std::cell::Cell::new(0u64);
    for external in External::ALL {
        let spec = &specs[external.cfg()];
        let mut runner = TestRunner::new(Config {
            cases: random_cases(),
            failure_persistence: None,
            ..Config::default()
        });
        let result = runner.run(
            &prop::collection::vec(drawn_strategy(external), 1..40),
            |drawn| {
                runs.set(runs.get() + 1);
                let world = run(spec, external, &drawn, false).map_err(TestCaseError::fail)?;
                random_cells.borrow_mut().extend(world.decided.iter().map(|(d, k)| (*d, *k, external)));
                Ok(())
            },
        );
        if let Err(e) = result {
            panic!("{external:?}: {e}");
        }
    }
    let random_cells = random_cells.into_inner();
    eprintln!("RANDOM_RUNS {} cells reached: {}", runs.get(), random_cells.len());
    let beyond: Vec<_> = random_cells.difference(&reachable).collect();
    assert!(
        beyond.is_empty(),
        "drawn runs reached cells listed as unreachable: {beyond:?}"
    );
}
