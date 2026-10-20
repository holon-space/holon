//! Atom-count measurement for the keystone's two Loro write legs.
//!
//! A Lamport height delta IS an atom count, so the two legs are compared by
//! the height each spends on the SAME logical transition: the SUT primary doc
//! (production `LoroBackend`) and the oracle's shadow primary
//! (`ShadowMesh`, via `holon_loro::multi_peer`).
//!
//! BOTH tests are `#[ignore]`: the keystone gate is the measured bottleneck
//! (~5.4 h per run) and this lane must add NO gate time. Run them with
//! `just atom-counts`.
//!
//! @pbt kind infra
//! @pbt covers atom-counts — measurement only, never a gate assertion.

use std::cell::RefCell;
use std::collections::BTreeMap;

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::known_reds::KnownReds;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_loro::multi_peer::lamport_height;
use holon_loro_testing::atom_counts;
use loro::LoroDoc;
use proptest::test_runner::Config;
use proptest::test_runner::TestRunner;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

/// Min/max of a sampled atom count, plus how many ticks were sampled.
#[derive(Default)]
struct Range {
    min: u32,
    max: u32,
    n: u32,
}

impl Range {
    fn note(&mut self, v: u32) {
        if self.n == 0 {
            self.min = v;
            self.max = v;
        } else {
            self.min = self.min.min(v);
            self.max = self.max.max(v);
        }
        self.n += 1;
    }

    fn show(&self) -> String {
        match self.n {
            0 => "-".to_string(),
            _ if self.min == self.max => format!("{}", self.min),
            _ => format!("{}..{}", self.min, self.max),
        }
    }
}

#[derive(Default)]
struct KindStats {
    ticks: u32,
    sut: Range,
    shadow_op: Range,
    shadow_peer: Range,
    shadow_pad: Range,
    /// Ticks whose `SutLoroLog` cap was absent (a SqlOnly draw) — no SUT
    /// height exists to compare, so the row's SUT column undercounts them.
    sut_absent: u32,
    mismatched: u32,
}

/// (A) Instrumented keystone run: the per-kind atom table over a real draw.
///
/// A draw that trips an already-registered known red is spent, not the end of
/// the measurement — the rows it already produced stay. A novel panic is
/// counted and disclosed too: a measurement run must print its table, and
/// this test prints numbers and never turns one into a verdict.
#[test]
#[ignore = "measurement only; run via `just atom-counts`"]
fn keystone_atom_counts() {
    // The recorder reads this once, on its first call.
    unsafe { std::env::set_var("HOLON_ATOM_COUNTS", "1") };
    assert!(atom_counts::enabled(), "recorder must be armed");

    let cases: u32 = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(16);

    let mut config = proptest::test_runner::contextualize_config(Config {
        cases,
        // Shrinking a red would re-run the same draw many times and pollute
        // the sample with its ticks; a measurement wants breadth, not a
        // minimal counterexample.
        max_shrink_iters: 0,
        failure_persistence: None,
        ..Config::default()
    });
    config.test_name = Some(concat!(module_path!(), "::keystone_atom_counts"));
    config.source_file = Some(file!());

    let strategy = <WideE2EMachine as ReferenceStateMachine>::sequential_strategy(1..40);
    let mut runner = TestRunner::new(config.clone());
    let known_reds = KnownReds::load().expect("known-reds registry");
    let stats: RefCell<BTreeMap<String, KindStats>> = RefCell::new(BTreeMap::new());
    let reds = RefCell::new((0u32, 0u32));

    let _ = runner.run(&strategy, |(initial_state, transitions, seen_counter)| {
        let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            <ComposedSut<WideE2E> as StateMachineTest>::test_sequential(
                config.clone(),
                initial_state,
                transitions,
                seen_counter,
            );
        }));
        if let Err(payload) = drawn {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_default();
            let mut reds = reds.borrow_mut();
            match known_reds.classify(&message) {
                Some(_) => reds.0 += 1,
                None => {
                    reds.1 += 1;
                    eprintln!("[atom-counts] novel red in a measurement case: {message}");
                }
            }
        }
        let mut stats = stats.borrow_mut();
        for row in atom_counts::take_rows() {
            let e = stats.entry(row.kind.clone()).or_default();
            e.ticks += 1;
            if row.kind_mismatch.is_some() {
                e.mismatched += 1;
                continue;
            }
            match row.sut_atoms {
                Some(a) => e.sut.note(a),
                None => e.sut_absent += 1,
            }
            e.shadow_op.note(row.shadow_op_atoms());
            e.shadow_peer.note(row.shadow_peer_atoms());
            e.shadow_pad.note(row.shadow_pad_atoms());
        }
        Ok(())
    });

    let stats = stats.into_inner();
    let (known, novel) = reds.into_inner();
    println!("\n## Keystone atom counts ({cases} cases, known reds {known}, novel {novel})\n");
    println!(
        "| transition kind | ticks | SUT atoms | shadow primary atoms | shadow peer atoms | shadow pad atoms |"
    );
    println!("| --- | --- | --- | --- | --- | --- |");
    for kind in E2ETransition::VARIANT_NAMES {
        match stats.get(*kind) {
            None => println!("| {kind} | not drawn in {cases} cases | - | - | - | - |"),
            Some(s) => {
                let mut notes = String::new();
                if s.sut_absent > 0 {
                    notes.push_str(&format!(" (no SUT Loro log in {} ticks)", s.sut_absent));
                }
                if s.mismatched > 0 {
                    notes.push_str(&format!(" (MISMATCH in {} ticks)", s.mismatched));
                }
                println!(
                    "| {kind} | {} | {}{} | {} | {} | {} |",
                    s.ticks,
                    s.sut.show(),
                    notes,
                    s.shadow_op.show(),
                    s.shadow_peer.show(),
                    s.shadow_pad.show(),
                );
            }
        }
    }
    // A kind the recorder saw that the alphabet does not know would mean the
    // two sides name kinds differently — the table would silently drop it.
    for kind in stats.keys() {
        assert!(
            E2ETransition::VARIANT_NAMES.contains(&kind.as_str()),
            "recorded kind {kind} is not in the kind alphabet"
        );
    }
}

/// Atoms one step spends on `doc`. Both ends are committed: `lamport_height`
/// reads the oplog frontiers, and an uncommitted op puts an id there whose
/// change the oplog does not hold yet.
fn atoms<T>(doc: &LoroDoc, step: impl FnOnce() -> T) -> u32 {
    doc.commit();
    let before = lamport_height(doc);
    step();
    doc.commit();
    lamport_height(doc) - before
}

/// (B) Op-shape atom counts, deterministic and fast: the primitives the two
/// legs are built from, measured on raw docs. This is what answers the two
/// named sub-cases (the tied-sibling index insert, and the first
/// `ensure_mergeable_text`) exactly, instead of waiting for a lucky draw.
#[test]
#[ignore = "measurement only; run via `just atom-counts`"]
fn loro_op_shape_atom_counts() {
    let doc = LoroDoc::new();
    let tree = doc.get_tree(holon_loro::TREE_NAME);
    // Production enables jitter-0 fractional indexes
    // (crates/holon-loro/src/loro_backend.rs:2505); index inserts need them.
    tree.enable_fractional_index(0);

    println!("\n## Loro op shapes (atoms per op)\n");
    println!("| op | atoms |");
    println!("| --- | --- |");

    let root = tree.create(None).expect("create root");
    println!(
        "| tree.create(parent) — append | {} |",
        atoms(&doc, || tree.create(root).expect("create child"))
    );

    let meta = tree.get_meta(root).expect("meta");
    let first_text = atoms(&doc, || {
        meta.ensure_mergeable_text("content_raw").expect("text")
    });
    let again_text = atoms(&doc, || {
        meta.ensure_mergeable_text("content_raw").expect("text")
    });
    println!("| meta.ensure_mergeable_text — first call | {first_text} |");
    println!("| meta.ensure_mergeable_text — later call | {again_text} |");

    let text: loro::LoroText = meta.ensure_mergeable_text("content_raw").expect("text");
    println!(
        "| text.insert(0, \"abc\") | {} |",
        atoms(&doc, || text.insert(0, "abc").expect("insert"))
    );
    println!(
        "| text.update(\"abd\") — 1-char diff | {} |",
        atoms(&doc, || text
            .update("abd", Default::default())
            .expect("update"))
    );
    println!(
        "| meta.insert(\"content_type\", …) — one map set | {} |",
        atoms(&doc, || meta.insert("content_type", "text").expect("set"))
    );

    // Untied siblings: an index insert costs exactly its own move.
    let a = tree.create(root).expect("a");
    let b = tree.create(root).expect("b");
    doc.commit();
    assert_ne!(
        tree.fractional_index(a),
        tree.fractional_index(b),
        "freshly appended siblings must not be tied"
    );
    let c = tree.create(root).expect("c");
    doc.commit();
    println!(
        "| tree.mov_after — untied siblings | {} |",
        atoms(&doc, || tree.mov_after(c, a).expect("mov_after"))
    );

    println!("\n### Index insert next to TIED siblings\n");
    for tied in 2..=4usize {
        let (reported, resets) = tied_sibling_index_insert(tied);
        println!(
            "- {tied} tied siblings: insert costs {reported} atoms, {resets} of them sibling re-spread moves"
        );
    }
}

/// Build `tied` siblings that share one fractional index — every fork appends
/// the first child of `parent`, from the SAME snapshot, so each picks the same
/// first key — then index-insert next to them and report (atoms of the insert,
/// atoms beyond the insert's own move).
fn tied_sibling_index_insert(tied: usize) -> (u32, u32) {
    let base = LoroDoc::new();
    let base_tree = base.get_tree(holon_loro::TREE_NAME);
    base_tree.enable_fractional_index(0);
    let parent = base_tree.create(None).expect("parent");
    base.commit();
    let snapshot = base.export(loro::ExportMode::Snapshot).expect("snapshot");

    let forks: Vec<LoroDoc> = (0..tied)
        .map(|_| {
            let peer = LoroDoc::new();
            peer.import(&snapshot).expect("import base");
            let peer_tree = peer.get_tree(holon_loro::TREE_NAME);
            peer_tree.enable_fractional_index(0);
            peer_tree.create(parent).expect("concurrent append");
            peer.commit();
            peer
        })
        .collect();
    for peer in &forks {
        base.import(
            &peer
                .export(loro::ExportMode::all_updates())
                .expect("updates"),
        )
        .expect("import peer");
    }
    base.commit();

    let children = base_tree.children(parent).expect("children");
    assert_eq!(children.len(), tied, "every concurrent insert must survive");
    let keys: Vec<Option<String>> = children
        .iter()
        .map(|&c| base_tree.fractional_index(c))
        .collect();
    let distinct: std::collections::BTreeSet<_> = keys.iter().collect();
    assert_eq!(
        distinct.len(),
        1,
        "the concurrent inserts must be TIED for this measurement; keys {keys:?}"
    );

    // The insert between two tied siblings: no key fits, so Loro re-spreads.
    let newcomer = base_tree.create(parent).expect("newcomer");
    base.commit();
    let spent = atoms(&base, || {
        base_tree
            .mov_after(newcomer, children[0])
            .expect("index insert next to tied siblings")
    });
    (spent, spent.saturating_sub(1))
}
