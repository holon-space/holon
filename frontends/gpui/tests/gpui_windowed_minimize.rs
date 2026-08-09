//! Windowed capture *minimizer* — greedy ddmin of a failing capture through the
//! composed windowed path (increment 4c: repointed off the phased
//! `windowed_replay` rebind service onto per-candidate
//! `replay_fixture_windowed` boots). The faithful, window-coupled counterpart
//! of the headless `bisection_pbt::minimize_capture_from_env` (which can't
//! reproduce a runner-coupled failure at all).
//!
//! Each ddmin candidate boots a fresh windowed `ComposedSut<WideE2E>` (window +
//! wide seed + settle, ~10s — the same per-case cost the 4b loop pays), replays
//! the candidate steps, and classifies any panic payload against the signature.
//! Captures must be POST-BOOT (the composed alphabet has no `StartApp`).
//!
//! `harness = false`. Run with:
//!   HOLON_CAPTURE=/abs.json cargo test -p holon-gpui \
//!     --test gpui_windowed_minimize --features pbt
//! Minimizing is OPT-IN: without `HOLON_CAPTURE` this is a disclosed no-op, so
//! the suite stays hermetic no matter what sits in the gitignored `.captures/`.
//! Env: HOLON_MINIMIZE_SIGNATURE=<substr> (default inv-blocks-match-ref/loro).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::fixtures::FixtureStep;
use holon_integration_tests::pbt::fixtures::NamedFixture;
use holon_integration_tests::pbt::fixtures::json;
use holon_integration_tests::pbt::transitions::E2ETransition;
use pbt_harness::windowed_wide::payload_signature_match;
use pbt_harness::windowed_wide::replay_fixture_windowed;

fn main() {
    if pbt_harness::handled_list_protocol("gpui_windowed_minimize") {
        return;
    }
    // A capture is by construction a RECORDED FAILING sequence, and `.captures/`
    // is gitignored — so minimizing whatever happens to sit at a default path
    // would flip the suite red from state version control cannot see. Minimizing
    // is opt-in; an explicitly requested capture that is missing fails loud.
    let Ok(path) = std::env::var("HOLON_CAPTURE") else {
        eprintln!("[minimize-window] no capture requested; set HOLON_CAPTURE=<path> to minimize");
        return;
    };
    assert!(
        std::path::Path::new(&path).exists(),
        "[minimize-window] HOLON_CAPTURE={path} does not exist"
    );
    let signature = std::env::var("HOLON_MINIMIZE_SIGNATURE")
        .unwrap_or_else(|_| "inv-blocks-match-ref/loro".to_string());
    let fixture = json::load_file(std::path::Path::new(&path));
    // Replay under the capture's recorded editor-shape flags — preconditions
    // consult them, so differing flags change the alphabet and the capture
    // rejects mid-replay.
    fixture.apply_recorded_env_flags();
    // Minimize under the wiring the capture was recorded with. The composed
    // windowed base is fixed full_headless — check ONCE up front so a mismatch
    // is a loud error, not N per-candidate "does not reproduce" boots.
    if let Some(recorded) = &fixture.wiring {
        assert_eq!(
            *recorded,
            wide_e2e_ref().harness.wiring,
            "[minimize-window] capture {path} was recorded under wiring {recorded:?}, but the \
             windowed composed base is fixed to full_headless — minimize headless instead"
        );
    }
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

    let wiring = fixture.wiring.clone();
    // Oracle: replay `seq` through a fresh windowed composed boot; true iff it
    // reproduces the signature failure.
    let reproduces = |seq: &[E2ETransition]| -> bool {
        let candidate = NamedFixture {
            name: "ddmin candidate".to_string(),
            description: String::new(),
            wiring: wiring.clone(),
            env_flags: Default::default(),
            steps: seq.iter().cloned().map(FixtureStep::Action).collect(),
        };
        match replay_fixture_windowed("minimize-window", &candidate) {
            Ok(()) => false,
            Err(payload) => payload_signature_match(payload.as_ref(), &signature),
        }
    };

    let mut seq = full.clone();
    if !reproduces(&seq) {
        eprintln!(
            "[minimize-window] capture does NOT reproduce in-window with signature {signature:?} \
             — nothing to minimize"
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
    let out_path = format!("{path}.min.json");
    let min = holon_pbt_core::fixture::Fixture::<E2ETransition, ()> {
        name: format!("windowed-minimized from {path}"),
        description: format!(
            "ddmin via per-candidate composed windowed boots, signature={signature}"
        ),
        environment: holon_pbt_core::fixture::CaptureEnvironment {
            wiring: wiring.clone(),
            env_flags: holon_pbt_core::fixture::CaptureEnvironment::current_env_flags(),
        },
        initial_state: (),
        transitions: seq,
    };
    min.save(&out_path)
        .unwrap_or_else(|e| panic!("[minimize-window] save {out_path:?}: {e}"));
    eprintln!("[minimize-window] wrote minimized capture to {out_path}");
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
