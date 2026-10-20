//! Per-transition Lamport-atom recorder for the keystone's two Loro write
//! legs.
//!
//! @pbt kind infra
//! @pbt covers atom-counts — measurement only; records the SUT primary doc's
//!   Lamport delta and the shadow doc's Lamport delta per keystone transition.
//!   Never asserts.
//!
//! A Lamport height moves by one per *atom* (one map set, one tree
//! create/move/delete, one text element), so a height delta IS an atom count.
//! The recorder is OFF unless `HOLON_ATOM_COUNTS=1`, so a gate run pays
//! nothing but the `enabled()` load.
//!
//! ## How a row is assembled
//! `proptest_state_machine::test_sequential` runs ref-apply N, then sut-apply
//! N. So per tick: [`begin_ref_transition`] (from the
//! `declare_e2e_transitions!` ref dispatch) clears the window list,
//! [`measure`] appends one window per shadow write site, and [`note_tick`]
//! (from the composed harness, which only runs during EXECUTION) closes the
//! row with the SUT's own delta. Generation-phase ref applies record windows
//! that the next `begin_ref_transition` discards — no run is polluted by them.

use std::sync::Mutex;
use std::sync::OnceLock;

/// One shadow write site inside a transition, and the atoms it spent.
#[derive(Debug, Clone)]
pub struct ShadowWindow {
    /// `<transition site>:<step>`, e.g. `catch_up:pad` or `sync:merge`.
    pub site: &'static str,
    /// Which shadow doc moved: `primary`, or `peer` for a peer-doc write.
    pub doc: &'static str,
    /// Lamport atoms the step wrote.
    pub atoms: u32,
}

/// One tick: the SUT primary's atoms and every shadow window of the same
/// logical transition.
#[derive(Debug, Clone)]
pub struct Row {
    /// `E2ETransition::variant_name()` of the SUT-side apply.
    pub kind: String,
    /// `None` when the drawn wiring exposes no `SutLoroLog` (SqlOnly) or the
    /// doc is not live yet.
    pub sut_atoms: Option<u32>,
    pub shadow: Vec<ShadowWindow>,
    /// The ref-side kind, when it disagreed with the SUT-side kind — the
    /// windows then belong to another transition and the row is disclosed as
    /// unpaired rather than silently attributed.
    pub kind_mismatch: Option<String>,
}

impl Row {
    /// Atoms the shadow spent on the logical op itself — every primary window
    /// except the clock padding, which is an artifact of the Lamport
    /// side-channel and not part of the op.
    pub fn shadow_op_atoms(&self) -> u32 {
        self.shadow
            .iter()
            .filter(|w| w.doc == "primary" && !w.site.ends_with(":pad"))
            .map(|w| w.atoms)
            .sum()
    }

    /// Atoms the shadow spent on a PEER doc. The SUT's peer is a separate
    /// instance whose own log this recorder does not read, so these have no
    /// SUT counterpart — they are reported apart from the primary comparison.
    pub fn shadow_peer_atoms(&self) -> u32 {
        self.shadow
            .iter()
            .filter(|w| w.doc == "peer")
            .map(|w| w.atoms)
            .sum()
    }

    /// Atoms the shadow spent padding its primary up to the SUT's height.
    pub fn shadow_pad_atoms(&self) -> u32 {
        self.shadow
            .iter()
            .filter(|w| w.site.ends_with(":pad"))
            .map(|w| w.atoms)
            .sum()
    }
}

#[derive(Default)]
struct State {
    /// Kind of the ref transition currently applying.
    kind: Option<String>,
    /// Shadow windows of that transition, awaiting their SUT delta.
    pending: Vec<ShadowWindow>,
    rows: Vec<Row>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

/// Whether to record. Read once: a measurement run sets the env before the
/// first transition.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("HOLON_ATOM_COUNTS").is_ok_and(|v| v == "1"))
}

/// Open a tick: name the ref transition and drop any windows the previous
/// (possibly generation-phase) transition left unpaired.
pub fn begin_ref_transition(kind: &str) {
    if !enabled() {
        return;
    }
    let mut s = state().lock().expect("atom-counts state lock");
    s.kind = Some(kind.to_string());
    s.pending.clear();
}

/// Run one shadow write step and record the atoms it wrote to `doc`.
pub fn measure<T>(
    site: &'static str,
    doc_label: &'static str,
    doc: &crate::shadow_mesh::ShadowDoc,
    step: impl FnOnce() -> T,
) -> T {
    if !enabled() {
        return step();
    }
    let before = doc.lamport_height();
    let out = step();
    let after = doc.lamport_height();
    assert!(
        after >= before,
        "{site}: shadow Lamport height went backwards ({before} -> {after})"
    );
    state()
        .lock()
        .expect("atom-counts state lock")
        .pending
        .push(ShadowWindow {
            site,
            doc: doc_label,
            atoms: after - before,
        });
    out
}

/// Close the tick with the SUT primary's own delta. Called from the composed
/// harness, which runs only during execution.
pub fn note_tick(kind: &str, sut_atoms: Option<u32>) {
    if !enabled() {
        return;
    }
    let mut s = state().lock().expect("atom-counts state lock");
    let ref_kind = s.kind.take();
    let kind_mismatch = match ref_kind {
        Some(k) if k == kind => None,
        other => other.or(Some("<no ref apply>".to_string())),
    };
    let shadow = std::mem::take(&mut s.pending);
    s.rows.push(Row {
        kind: kind.to_string(),
        sut_atoms,
        shadow,
        kind_mismatch,
    });
}

/// Record a window whose atoms the caller already knows, for a step that
/// CREATES the doc it writes (there is no "before" height to read).
pub fn record(site: &'static str, doc_label: &'static str, atoms: u32) {
    if !enabled() {
        return;
    }
    state()
        .lock()
        .expect("atom-counts state lock")
        .pending
        .push(ShadowWindow {
            site,
            doc: doc_label,
            atoms,
        });
}

/// Every row recorded so far, leaving the recorder empty.
pub fn take_rows() -> Vec<Row> {
    if !enabled() {
        return Vec::new();
    }
    std::mem::take(&mut state().lock().expect("atom-counts state lock").rows)
}
