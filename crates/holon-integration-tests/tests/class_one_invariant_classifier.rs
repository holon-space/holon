//! Machine-checked classification of the composed invariant catalog into
//! class-1 (self-consistency — never consults the reference model) and class-2
//! (ref-comparative), by running every registered invariant against
//! [`holon_pbt_core::null_ref::NullRef`].
//!
//! Two independent signals, deliberately combined:
//!
//! - **Declared** — an invariant's `needs().ref_present` is data the selector
//!   reads. Empty ⇒ the wire site claims class 1.
//! - **Observed** — `NullRef` answers every `Ref*` capability with a panic that
//!   names the method read. An invariant that completes against it touched no
//!   ref on that path.
//!
//! Neither alone is sound: a declaration can over-approximate (a body that
//! declares a ref cap but only reads it behind an applicability gate), and an
//! observation can under-approximate (a body whose ref read sits in a branch
//! this SUT state does not reach). Their DISAGREEMENT is the interesting
//! signal, and `declares_no_ref_but_reads_one` turns it into a hard failure —
//! that combination is a body reaching past its declared needs, which would
//! make the live self-check suite claim a check it cannot honestly perform.
//!
//! @pbt kind infra
//! @pbt covers invariant-classification — the class-1 set the live self-check
//! suite is allowed to run

use std::collections::BTreeSet;
use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::Mutex;

use futures::FutureExt;
use holon_integration_tests::pbt::composed::builder::compose_sut;
use holon_integration_tests::pbt::composed::catalog::composed_invariant_catalog;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_pbt_core::ComponentSet;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::null_ref::null_ref_cap_ids;
use holon_pbt_core::null_ref::null_ref_caps;

/// The class-1 set, pinned. A new invariant that reads the reference model
/// self-classifies OUT of this list (its `needs()` names a `Ref*` cap); one
/// that does not, lands IN it and must be added here deliberately — the act of
/// editing this list is the act of asserting "the live self-check suite may run
/// this against a running app with no reference model".
///
/// Cross-check: the hand census in the dogfood-recorder plan §1.2 counted 29,
/// and this machine-derived set is 29 — independently, over a 75-entry catalog
/// (the census counted body modules; the catalog also holds the
/// correspondence-derived per-store families).
///
/// Class 1 is NOT the same as "safe to run against a live app with no history":
/// four of these are the plan's class-3 temporal/budget checks
/// (`inv-sql-budget`, `inv-settle-budget`,
/// `inv-matview-consistent-with-recompute`, `inv-no-steady-reseed-leak`) — they
/// consult no reference model but do need per-tick accounting, so a live
/// self-check suite must exclude them explicitly rather than inherit them from
/// this list.
const CLASS_ONE: &[&str] = &[
    "inv-display-placement-canonical-inert",
    "inv-frontend-engine",
    "inv-frontend-no-error-widgets",
    "inv-frontend-root-not-error",
    "inv-inline-row-mount-present",
    "inv-live-block-shell-present",
    "inv-live-tree-matches-fresh",
    "inv-loro-no-errors",
    "inv-mark-bounds-within-content",
    "inv-matview-consistent-with-recompute",
    "inv-no-errors",
    "inv-no-observed-errors",
    "inv-no-orphan-blocks",
    "inv-no-parent-cycles",
    "inv-no-steady-reseed-leak",
    "inv-no-write-outside-vault-root",
    "inv-org-render-fixed-point",
    "inv-paint-text-styling",
    "inv-settle-budget",
    "inv-source-language-iff-source",
    "inv-sql-budget",
    "inv-sticky-accordion-spec",
    "inv-viewmodel-editable-text-triggers",
    "inv-viewmodel-no-error-widgets",
    "inv-viewmodel-shows-source-when-no-query",
    "inv-viewmodel-snapshot",
    "inv-wheel-occlusion-routing",
    "inv-wheel-two-mode-motion-law",
    "inv-window-focus-matches-engine-focus",
];

fn resolver() -> IdResolver {
    Arc::new(Mutex::new(std::collections::BTreeMap::new()))
}

#[derive(Debug)]
struct Classification {
    /// Every registered id — the census denominator.
    all: BTreeSet<&'static str>,
    /// Declares no `Ref*` cap in its wire site's `needs()`. SUT-independent, so
    /// this is the class-1 set the live suite may run.
    class_one: BTreeSet<&'static str>,
    /// Declares at least one ref cap (whether or not it read it here).
    declares_ref: BTreeSet<&'static str>,
    /// Panicked against `NullRef` — the panic message names the method.
    read_ref: Vec<(&'static str, String)>,
    /// Ran to completion against `NullRef`: `class_one` membership OBSERVED,
    /// not merely declared.
    completed: BTreeSet<&'static str>,
    /// Not selected against this SUT: its SUT caps are absent, so the dynamic
    /// signal is unavailable and only the declaration classifies it. Disclosed,
    /// never silently dropped.
    deselected: BTreeSet<&'static str>,
}

/// Run every catalog invariant against a real headless SUT and a `NullRef`
/// reference, one at a time so a class-2 panic classifies that invariant
/// instead of aborting the run.
async fn classify() -> Classification {
    let catalog = composed_invariant_catalog();
    let sut = compose_sut(&ComponentSet::full_headless(), &resolver()).await;
    let ref_ = null_ref_caps();
    let have_sut = sut.caps.cap_set();
    let have_ref = ref_.cap_set();

    let mut out = Classification {
        all: BTreeSet::new(),
        class_one: BTreeSet::new(),
        declares_ref: BTreeSet::new(),
        read_ref: Vec::new(),
        completed: BTreeSet::new(),
        deselected: BTreeSet::new(),
    };

    // `NullRef` panics are the classifier's DATA, not a failure. The guard
    // filters exactly those payloads and forwards every other panic, so a
    // sibling thread's failure mid-sweep is still reported.
    let _hook = holon_pbt_core::panic_filter::SweepPanicHook::install();

    for inv in &catalog {
        let id = inv.id().0;
        let needs = inv.needs();
        out.all.insert(id);
        if needs.ref_present.is_empty() {
            out.class_one.insert(id);
        } else {
            out.declares_ref.insert(id);
        }
        if !needs.selected_against(&have_sut, &have_ref) {
            out.deselected.insert(id);
            continue;
        }
        match AssertUnwindSafe(inv.check_boxed(&sut.caps, &ref_))
            .catch_unwind()
            .await
        {
            Ok(_) => {
                out.completed.insert(id);
            }
            Err(payload) => out.read_ref.push((id, panic_message(payload))),
        }
    }

    out
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    "<non-string panic payload>".to_string()
}

/// The pinned class-1 set is exactly what the catalog yields. Editing
/// [`CLASS_ONE`] is the assertion act; this test is the lock.
#[tokio::test(flavor = "multi_thread")]
async fn class_one_set_is_exactly_the_pinned_list() {
    let c = classify().await;
    let pinned: BTreeSet<&str> = CLASS_ONE.iter().copied().collect();
    let found: BTreeSet<&str> = c.class_one.iter().copied().collect();
    let confirmed = c.class_one.intersection(&c.completed).count();
    assert_eq!(
        found,
        pinned,
        "\ncatalog entries: {}   class-1: {}   class-2: {}\n\
         \nCLASS 1 (declares no Ref* cap), {} of them — {confirmed} CONFIRMED by \
         completing against NullRef, the rest deselected here for want of SUT caps:\n{}\n\
         \nDESELECTED against ComponentSet::full_headless (no dynamic signal): {}\n{}\n\
         \nREAD THE REF (class 2, observed), {} of them:\n{}\n",
        c.all.len(),
        c.class_one.len(),
        c.declares_ref.len(),
        c.class_one.len(),
        c.class_one
            .iter()
            .map(|id| format!("    \"{id}\","))
            .collect::<Vec<_>>()
            .join("\n"),
        c.deselected.len(),
        c.deselected
            .iter()
            .map(|id| format!("    {id}"))
            .collect::<Vec<_>>()
            .join("\n"),
        c.read_ref.len(),
        c.read_ref
            .iter()
            .map(|(id, msg)| format!("    {id}: {msg}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// The rot-detector: a body that reaches a `Ref*` capability its wire site did
/// not declare. Selection would still admit it (the ref map carries the cap),
/// so nothing else catches this — and it is exactly the failure that would let
/// the live self-check suite run an invariant whose answer depends on a
/// reference model it does not have.
#[tokio::test(flavor = "multi_thread")]
async fn declares_no_ref_but_reads_one() {
    let c = classify().await;
    let undeclared: Vec<&(&str, String)> = c
        .read_ref
        .iter()
        .filter(|(id, _)| !c.declares_ref.contains(id))
        .collect();
    assert!(
        undeclared.is_empty(),
        "these invariants read the reference model without declaring a Ref* cap \
         in their wire()'s needs — the declaration and the body disagree:\n{}",
        undeclared
            .iter()
            .map(|(id, msg)| format!("    {id}: {msg}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// `NullRef` must answer every `Ref*` capability any catalog invariant
/// declares. A newly introduced ref capability that `NullRef` does not host
/// would make its invariants deselect against the null map — silently shrinking
/// the census instead of classifying them.
#[test]
fn null_ref_hosts_every_declared_ref_cap() {
    let hosted: HashSet<CapId> = null_ref_cap_ids().into_iter().collect();
    let mut missing: BTreeSet<&'static str> = BTreeSet::new();
    for inv in &composed_invariant_catalog() {
        for cap in inv.needs().ref_present {
            if !hosted.contains(&cap) {
                missing.insert(cap.name());
            }
        }
    }
    assert!(
        missing.is_empty(),
        "NullRef does not host these Ref* caps, so the invariants declaring them \
         cannot be classified: {missing:?}",
    );
}
