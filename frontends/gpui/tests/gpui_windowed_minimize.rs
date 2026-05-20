//! Windowed capture *minimizer* — greedy ddmin of a failing capture through ONE
//! reused GPUI window, re-pointed at a fresh SUT per candidate via
//! [`holon_gpui::RebindHandle`]. The faithful, window-coupled counterpart of the
//! headless `bisection_pbt::minimize_capture_from_env` (which can't reproduce a
//! runner-coupled failure at all).
//!
//! The window plumbing (bg-thread-drives / main-thread-window split, rebind
//! loop, paint-quiescence wait) lives in the shared
//! [`pbt_harness::windowed_replay`] service; this binary is just the ddmin loop
//! (a thin bg client) plus capture I/O. The proptest *shrinker* in
//! `gpui_ui_pbt` drives the same service.
//!
//! Why a reused window instead of one process per candidate: GPUI owns the main
//! thread and an `Application` is per-process, so spawning a process per ddmin
//! candidate re-pays full app + window init every time. Here the window opens
//! once and each candidate just rebinds it to that candidate's fresh engine.
//! State isolation is intact: every candidate builds its own `E2ESut` (fresh
//! Turso + Loro) and runs `StartApp` from empty.
//!
//! `harness = false` (GPUI needs the main thread). Run with:
//!   cargo test -p holon-gpui --test gpui_windowed_minimize --features pbt
//! Env: HOLON_CAPTURE=/abs.json (default gpui_ui_pbt capture);
//!      HOLON_MINIMIZE_SIGNATURE=<substr> (default inv-blocks-match-ref/loro).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use holon_integration_tests::pbt::fixtures::{json, FixtureStep};
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::ui_harness::{
    set_loro_peer_id_if_unset, set_memory_multiplier_if_unset,
};

use pbt_harness::windowed_replay::{payload_signature_match, windowed_replay_service};

fn capture_path() -> String {
    std::env::var("HOLON_CAPTURE").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/holon-integration-tests/tests/.captures/gpui_ui_pbt.captured.json"
        )
        .to_string()
    })
}

fn main() {
    // Same env setup the shared window harness applies (memory budget, atomic
    // editor, Loro peer). Disable the panic-pause so caught invariant panics
    // don't block the ddmin loop.
    set_memory_multiplier_if_unset("15");
    set_loro_peer_id_if_unset("1");
    for (k, v) in [
        ("PBT_ATOMIC_EDITOR", "1"),
        ("PBT_MUTABLE_TEXT", "1"),
        ("PBT_PAUSE_SECONDS", "0"),
    ] {
        if std::env::var(k).is_err() {
            unsafe { std::env::set_var(k, v) };
        }
    }

    let path = capture_path();
    let signature = std::env::var("HOLON_MINIMIZE_SIGNATURE")
        .unwrap_or_else(|_| "inv-blocks-match-ref/loro".to_string());
    let fixture = json::load_file(std::path::Path::new(&path));
    // Replay under the capture's recorded editor-shape flags — preconditions
    // consult them (e.g. LoroRequiredForAtomicEditor passes via
    // PBT_REAL_EDITOR), so differing flags change the alphabet and the
    // capture rejects mid-replay.
    fixture.apply_recorded_env_flags();
    // Minimize under the wiring the capture was recorded with (pre-header
    // captures lack it — fall back to Full).
    let wiring = fixture
        .wiring
        .clone()
        .unwrap_or_else(holon_pbt_core::Wiring::full);
    let full: Vec<E2ETransition> = fixture
        .steps
        .into_iter()
        .map(|s| match s {
            FixtureStep::Action(t) => t,
            FixtureStep::Assert(_) => {
                panic!("[minimize-window] capture must contain only Action steps")
            }
        })
        .collect();
    eprintln!(
        "[minimize-window] capture {path} ({} transitions), signature={signature:?}",
        full.len()
    );

    let (host, replayer) = windowed_replay_service();
    let out_path = format!("{path}.min.json");

    // ── Background thread: the ddmin loop (a thin client of the service) ──────
    // `replayer` is moved in (consumed only here); no Arc needed.
    let bg_signature = signature.clone();
    let bg_path = path.clone();
    let bg = move || {
        // Oracle: replay `seq` in the reused window; true iff it reproduces the
        // signature failure.
        let reproduces = |seq: &[E2ETransition]| -> bool {
            let steps: Vec<FixtureStep> = seq.iter().cloned().map(FixtureStep::Action).collect();
            match replayer.replay(wiring.clone(), steps, None) {
                Ok(()) => false,
                Err(payload) => payload_signature_match(payload.as_ref(), &bg_signature),
            }
        };

        let mut seq = full.clone();
        if !reproduces(&seq) {
            eprintln!(
                "[minimize-window] capture does NOT reproduce in-window with signature \
                 {bg_signature:?} — nothing to minimize"
            );
            return;
        }
        eprintln!(
            "[minimize-window] start: {} transitions reproduce in-window",
            seq.len()
        );

        loop {
            let mut shrunk = false;
            let mut i = 0;
            while i < seq.len() {
                let mut candidate = seq.clone();
                let dropped = candidate.remove(i);
                if reproduces(&candidate) {
                    eprintln!(
                        "[minimize-window] dropped #{i} {} → {} transitions still reproduce",
                        serde_json::to_string(&dropped).unwrap_or_default(),
                        candidate.len()
                    );
                    seq = candidate;
                    shrunk = true;
                } else {
                    i += 1;
                }
            }
            if !shrunk {
                break;
            }
        }

        eprintln!(
            "[minimize-window] minimal: {} transitions still reproduce",
            seq.len()
        );
        for (i, t) in seq.iter().enumerate() {
            eprintln!(
                "[minimize-window]   [{i}] {}",
                serde_json::to_string(t).unwrap_or_default()
            );
        }
        let min = holon_pbt_core::fixture::Fixture::<E2ETransition, ()> {
            name: format!("windowed-minimized from {bg_path}"),
            description: format!("ddmin via reused GPUI window, signature={bg_signature}"),
            environment: holon_pbt_core::fixture::CaptureEnvironment {
                wiring: Some(wiring.clone()),
                env_flags: holon_pbt_core::fixture::CaptureEnvironment::current_env_flags(),
            },
            initial_state: (),
            transitions: seq,
        };
        min.save(&out_path)
            .unwrap_or_else(|e| panic!("[minimize-window] save {out_path:?}: {e}"));
        eprintln!("[minimize-window] wrote minimized capture to {out_path}");
    };

    // ── Main thread: open the window once, then rebind per candidate ─────────
    host.run_window("Holon Windowed Minimize", bg);
}
