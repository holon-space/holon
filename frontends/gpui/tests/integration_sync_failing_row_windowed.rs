//! A failing sync must LOOK different from a healthy one, in a real window.
//!
//! `IntegrationStatus::SyncFailing` writes the word `Sync failing` into
//! `integration_state.status`; the sidebar row paints that word as one glyph.
//! Everything between those two ends — the mirror column, the seeded row
//! template, the `integration_status` widget's table, the gpui text renderer —
//! is what this rung crosses. A row model that carries the right word but
//! paints the healthy glyph (or the `?` an UNMAPPED word paints) is
//! indistinguishable from a working integration on screen, which is the whole
//! failure this state exists to make visible.
//!
//! It is a SIBLING of `integrations_sidebar_rows_windowed.rs` rather than an
//! extension of it because that rung's oracle for a glyph IS
//! `status_symbol_and_color` — the same table it would be testing. Asserting
//! "distinct from healthy, and not the unknown marker" needs glyphs written out
//! LITERALLY, and folding literals into that rung would have contradicted its
//! read-the-table design.
//!
//! Three claims, all off painted text:
//!   1. the `Sync failing` row paints a glyph DIFFERENT from the `Connected`
//!      row's;
//!   2. it is not `?` — that marker is what an unmapped word paints, so seeing
//!      it means the word never reached the table;
//!   3. `Syncing` (in flight) is likewise its own glyph, distinct from both —
//!      the three sync-adjacent states are three readings, not two.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! integration_sync_failing_row_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).
//!
//! @pbt kind harness
//! @pbt covers integration-sync-failing-paints-distinctly
//! @pbt slips-if-removed a failing sync paints the healthy glyph, or the
//! unknown `?`, and every headless rung stays green because the mirror column
//! still holds the right word

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::composed::builder::compose_sut_windowed_base_seeded;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_pbt_core::ComponentSet;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

/// The three rows under test: a provider, the word written into its mirror row,
/// and the glyph a reader must see. The glyphs are written out here instead of
/// looked up, so this rung fails when the table and the screen disagree.
const CASES: [(&str, &str, &str); 3] = [
    ("claude-history", "Connected", "●"),
    ("todoist", "Sync failing", "◍"),
    ("gcal", "Syncing", "◌"),
];

/// What an unmapped status word paints (`shadow_builders::integration_status`).
/// Seeing it on a row means the word did not survive to the table.
const UNKNOWN_MARKER: &str = "?";

fn symbol_id(provider: &str) -> String {
    format!("text-integration:{provider}-status")
}

fn painted(bounds: &BoundsRegistry, provider: &str) -> ElementInfo {
    bounds
        .element_info(&symbol_id(provider))
        .filter(ElementInfo::has_visible_area)
        .unwrap_or_else(|| {
            panic!(
                "{}'s status glyph is not painted — this rung needs the Integrations rows on \
                 screen to judge what they say",
                symbol_id(provider)
            )
        })
}

fn main_test(bounds: &BoundsRegistry) {
    let seen: Vec<(&str, &str, String)> = CASES
        .iter()
        .map(|(provider, status, want)| {
            let info = painted(bounds, provider);
            let got = info
                .displayed_text
                .as_deref()
                .unwrap_or_else(|| panic!("{provider}'s status glyph painted no text"))
                .to_string();
            (*status, *want, got)
        })
        .collect();

    let report = format!(
        "\n=== painted status glyphs ===\n{}\n",
        seen.iter()
            .map(|(status, want, got)| format!("  {status:<14} want {want:?}  got {got:?}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    eprintln!("{report}");

    // Non-vacuity: every case actually painted something.
    assert!(
        seen.iter().all(|(_, _, got)| !got.is_empty()),
        "a row painted an empty glyph, so the comparisons below judge nothing{report}"
    );

    for (status, want, got) in &seen {
        let got = got.as_str();
        assert_ne!(
            got, UNKNOWN_MARKER,
            "{status:?} painted the unknown marker {UNKNOWN_MARKER:?}. That is what an UNMAPPED \
             word paints, so the word never reached \
             `holon_frontend::shadow_builders::integration_status`{report}"
        );
        assert_eq!(got, *want, "{status:?} must paint {want:?}{report}");
    }

    // 1 + 3: three states, three readings. Comparing the painted strings (not
    // the table) is what makes a row that paints a constant fail here.
    let mut distinct: Vec<&str> = seen.iter().map(|(_, _, got)| got.as_str()).collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        seen.len(),
        "two of the three sync states paint the SAME glyph, so a failing sync is \
         indistinguishable from a healthy or an in-flight one on screen{report}"
    );
}

#[test]
fn a_failing_sync_paints_its_own_glyph_not_the_healthy_one() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let home = tempfile::tempdir().expect("tempdir for HOME");
    // SAFETY: single-threaded test binary, set before the app boots.
    unsafe { std::env::set_var("HOME", home.path()) };

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));
    let set = ComponentSet::full_headless();
    let bundle = runtime
        .block_on(async { compose_sut_windowed_base_seeded(&set, &resolver, &[], &[]).await });
    let session = bundle.session.clone().expect("full_headless -> session");
    let engine = bundle
        .reactive
        .clone()
        .expect("full_headless -> reactive engine");
    let backend = bundle
        .engine
        .clone()
        .expect("full_headless -> backend engine");

    runtime.block_on(async {
        let db = backend.db_handle();
        db.execute_values("UPDATE integration_state SET enabled = 1", vec![])
            .await
            .expect("switch every mirrored integration on");
        for (provider, status, _) in CASES {
            let updated = db
                .execute_values(
                    &format!(
                        "UPDATE integration_state SET status = '{status}' WHERE provider_name = \
                         '{provider}'"
                    ),
                    vec![],
                )
                .await
                .unwrap_or_else(|e| panic!("write {status:?} onto {provider}: {e}"));
            assert_eq!(
                updated, 1,
                "{provider} is not a row of integration_state, so its case below would judge \
                 nothing"
            );
        }
    });

    let bounds = BoundsRegistry::new();
    let nav = NavigationState::new();
    let rebind = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                None,
                None,
                "Holon-IntegrationSyncFailing-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        main_test(&bounds);
    }));

    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(rebind);
        app.update(|cx| cx.shutdown());
        app.run_until_parked();
    }));
    std::mem::forget(app);
    std::mem::forget(bundle);

    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
