//! E0c-(b) make-or-break: prove the Holon widget tree yields **real, deterministic
//! layout geometry** under gpui `TestPlatform` — no on-screen window.
//!
//! This is the one non-mechanical risk gating the `E2ESut`-dissolution endgame
//! (Bundle E in `docs/Testing/PbtCompositionBacklog.md`). `SutLayout` geometry
//! (`rendered_elements` → `BoundsRegistry`) is the single cap the headless
//! `frontend_slice` can't provide, because there are no element bounds without a
//! layout/paint pass. The question: can a `TestPlatform` window produce that
//! geometry **deterministically** (the occlusion/blur flakiness that drove the
//! real-window→TestPlatform migration was a *real-window* property; TestPlatform's
//! fake dispatcher + fake clock should be reproducible)?
//!
//! `test_platform_smoke.rs` already proves a single boot yields non-empty bounds.
//! This test strengthens that to the actual risk:
//!   1. **Real** — each boot yields non-degenerate geometry (≥1 element with real
//!      width/height) and no leftover `"loading"` placeholders.
//!   2. **Deterministic** — N independent boots settle to byte-identical geometry
//!      (same element set, same widget types, same pixel bounds). Independent boots
//!      (fresh backend + window each) is a *stronger* determinism claim than one
//!      window rebound across ticks (no shared-window state to mask drift).
//!
//! PASS ⇒ the windowed frontend is "just another component"; E4 (`GpuiWindowComponent`)
//! is mechanical. FAIL ⇒ keep a real-window residue (the slim-residue fallback).
//!
//! Reuses the production launcher (`launch_holon_window_rebindable`) and the proven
//! cross-runtime fixed-point settle — the exact path E4's component will stand up.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{AssetSource, PlatformTextSystem, TestApp};
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::test_environment::TestEnvironment;

/// Independent boots to compare. ≥2 proves determinism; 3 guards against a lucky
/// pair. Each boot is a full backend + window stand-up + settle, so this is the
/// slow part — keep it small.
const BOOTS: usize = 3;

fn real_text_system() -> Arc<dyn PlatformTextSystem> {
    // `true` = TestPlatform (headless). Same `text_system()` real macOS layout
    // engine either way — only the windowing/dispatch is faked.
    gpui_platform::current_platform(true).text_system()
}

/// Cross-runtime fixed-point settle (copied from `test_platform_smoke.rs`): runs
/// real tokio time between gpui pump cycles so backend→frontend signals deliver
/// through the channel bridge, advances the fake clock for timer-driven code,
/// and promotes staged bounds. Returns once the element count is stable and no
/// `"loading"` placeholders remain. Panics loudly if it never settles.
fn settle_to_fixed_point(
    app: &mut TestApp,
    bounds: &BoundsRegistry,
    runtime: &tokio::runtime::Runtime,
    timeout: Duration,
) {
    let start = Instant::now();
    let mut last_count = 0usize;
    let mut stable_iters = 0u32;
    while start.elapsed() < timeout {
        runtime.block_on(async { tokio::time::sleep(Duration::from_millis(20)).await });
        app.run_until_parked();
        app.advance_clock(Duration::from_secs(1));
        app.run_until_parked();
        bounds.flush();
        let elements = bounds.all_elements();
        let count = elements.len();
        let still_loading = elements
            .iter()
            .any(|(_, info)| info.widget_type.as_ref() == "loading");
        if count == last_count && count > 0 && !still_loading {
            stable_iters += 1;
            if stable_iters >= 5 {
                return;
            }
        } else {
            stable_iters = 0;
        }
        last_count = count;
    }
    let elements = bounds.all_elements();
    panic!(
        "geometry never reached a fixed point within {timeout:?}: {} elements, loading={}",
        elements.len(),
        elements
            .iter()
            .filter(|(_, info)| info.widget_type.as_ref() == "loading")
            .count()
    );
}

/// One element's geometry as a canonical tuple: (widget_type, x, y, w, h), pixel
/// bounds rounded to whole pixels. The make-or-break is *gross geometry*
/// determinism — does identical content lay out to identical pixels each boot —
/// so the key is the **shape** (widget type + bounds), deliberately **not** the
/// `entity_id`. Independent boots seed a fresh vault and mint **fresh random block
/// UUIDs**, so entity ids legitimately differ boot-to-boot while the layout is
/// pixel-identical; in real PBT use the ids come from a seeded `ReferenceState`
/// (fixed ids) and are orthogonal to this geometry question.
type Geom = (String, i32, i32, i32, i32);

/// A settled snapshot: the multiset of element geometries (sorted so record order
/// in `all_elements()` is irrelevant) plus structural counts that must also match.
struct Snap {
    /// Sorted multiset of (widget_type, x, y, w, h) — the geometry shape.
    geom: Vec<Geom>,
    /// Distinct entity bindings — structural identity count, UUID values aside.
    distinct_entities: usize,
}

fn snapshot(bounds: &BoundsRegistry) -> Snap {
    let elements = bounds.all_elements();
    let mut geom: Vec<Geom> = elements
        .iter()
        .map(|(_, info)| {
            (
                info.widget_type.to_string(),
                info.x.round() as i32,
                info.y.round() as i32,
                info.width.round() as i32,
                info.height.round() as i32,
            )
        })
        .collect();
    geom.sort();
    let distinct_entities = elements
        .iter()
        .filter_map(|(_, info)| info.entity_id.as_ref())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    Snap {
        geom,
        distinct_entities,
    }
}

/// Boot a fresh backend + TestPlatform window, settle to a fixed point, and return
/// the canonical geometry snapshot. Each call is fully independent (own backend,
/// own window). The `TestApp` is intentionally leaked at the end: shutdown clears
/// windows but detached pump tasks still hold entity handles and gpui's leak
/// detector runs before the dispatcher drops them.
fn boot_and_snapshot(runtime: &Arc<tokio::runtime::Runtime>) -> Snap {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = TestApp::with_text_system_and_assets(text_system, assets);

    let mut env = runtime
        .block_on(async { TestEnvironment::new(runtime.clone()) })
        .expect("test environment");
    runtime.block_on(async { env.start_app(true).await.expect("start_app") });

    let session = env.session_arc();
    let engine = env
        .reactive_engine
        .get()
        .cloned()
        .expect("reactive engine after start_app");
    let debug_services = env.debug_services().cloned().expect("debug services");

    let bounds = BoundsRegistry::new();
    let nav = NavigationState::new();

    let rebind_handle = app
        .update(|cx| {
            launch_holon_window_rebindable(
                session.clone(),
                engine.clone(),
                runtime.handle().clone(),
                nav,
                bounds.clone(),
                Some(debug_services.clone()),
                "Holon-TestPlatform-Determinism",
                cx,
            )
        })
        .expect("window opened");

    settle_to_fixed_point(&mut app, &bounds, runtime, Duration::from_secs(30));

    let snap = snapshot(&bounds);

    // Keep the booted backend alive past the snapshot (the forgotten app holds
    // handles into it); leak both — the process exits right after the test.
    drop(rebind_handle);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(env);

    snap
}

/// Symmetric-difference of two geometry multisets, for actionable failure output.
fn geom_diff(a: &[Geom], b: &[Geom]) -> String {
    let count = |v: &[Geom]| {
        let mut m: BTreeMap<Geom, i32> = BTreeMap::new();
        for g in v {
            *m.entry(g.clone()).or_default() += 1;
        }
        m
    };
    let (ca, cb) = (count(a), count(b));
    let mut keys: std::collections::BTreeSet<&Geom> = ca.keys().collect();
    keys.extend(cb.keys());
    let mut s = String::new();
    for k in keys {
        let (na, nb) = (
            ca.get(k).copied().unwrap_or(0),
            cb.get(k).copied().unwrap_or(0),
        );
        if na != nb {
            let (wt, x, y, w, h) = k;
            s.push_str(&format!(
                "  {wt} @ ({x},{y} {w}x{h}): boot0×{na} vs other×{nb}\n"
            ));
        }
    }
    s
}

#[test]
fn test_platform_geometry_is_real_and_deterministic() {
    // One shared runtime across boots; backends are created sequentially on it.
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));

    let snaps: Vec<Snap> = (0..BOOTS).map(|_| boot_and_snapshot(&runtime)).collect();

    // (1) REAL: every boot produced non-degenerate geometry.
    for (i, snap) in snaps.iter().enumerate() {
        assert!(
            !snap.geom.is_empty(),
            "boot {i}: BoundsRegistry empty after settle (no geometry produced)"
        );
        let non_degenerate = snap
            .geom
            .iter()
            .filter(|(_, _, _, w, h)| *w > 1 && *h > 1)
            .count();
        assert!(
            non_degenerate >= 1,
            "boot {i}: every element is degenerate (w/h ≤ 1px) — a zero-size or \
             occluded window records collapsed rects."
        );
        eprintln!(
            "[determinism] boot {i}: {} elements, {non_degenerate} non-degenerate, \
             {} distinct entities",
            snap.geom.len(),
            snap.distinct_entities
        );
    }

    // (2) DETERMINISTIC: all boots settled to identical geometry *shape* (widget
    // types + pixel bounds) and identical structural entity-count. Entity UUIDs
    // differ per boot (fresh vault) and are deliberately excluded — see `Geom`.
    let first = &snaps[0];
    for (i, snap) in snaps.iter().enumerate().skip(1) {
        assert!(
            snap.geom == first.geom,
            "geometry shape is NOT deterministic: boot 0 vs boot {i} differ.\n\
             boot0 has {} elements, boot{i} has {}.\ndiff (count mismatches):\n{}",
            first.geom.len(),
            snap.geom.len(),
            geom_diff(&first.geom, &snap.geom)
        );
        assert_eq!(
            snap.distinct_entities, first.distinct_entities,
            "structural entity count differs: boot 0 has {}, boot {i} has {}",
            first.distinct_entities, snap.distinct_entities
        );
    }

    eprintln!(
        "[determinism] PASS — {BOOTS} independent TestPlatform boots produced identical \
         non-degenerate geometry ({} elements, {} entities) modulo fresh-vault UUIDs",
        first.geom.len(),
        first.distinct_entities
    );
}
