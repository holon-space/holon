//! The deferred-re-import banner, in a REAL window over a REAL booted session.
//!
//! When a pair's re-import has nowhere to put the blocks this device wrote, the
//! app boots anyway (D94.a) and the only thing standing between the user and a
//! silent loss is a banner that names the count and the archive, plus the one
//! button that re-runs the work. Neither has a command or a keybinding, so this
//! is the tier that answers "does the user see it, and can they press it".
//!
//! Every state change after the boot call is caused by a real mouse click on
//! that button: the store, the bus, the `DevicePairing` and the operation
//! dispatch are all the composed session's own, so the click reaches the same
//! instance the boot call used.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! pairing_deferred_reimport_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use gpui::InputEvent;
use gpui::MouseButton;
use gpui::Pixels;
use gpui::Point;
use holon_api::BlockContent;
use holon_api::BlockEdges;
use holon_api::EntityUri;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::TITLE_ROW_ID;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_gpui::share_ui::DEFERRED_REIMPORT_BANNER;
use holon_gpui::share_ui::DEFERRED_REIMPORT_RETRY;
use holon_gpui::share_ui::DEGRADED_TOAST_STACK;
use holon_integration_tests::pbt::composed::builder::compose_sut_windowed_base_seeded;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_loro::DocScope;
use holon_loro::LoroDocumentStore;
use holon_loro::device_pairing_op::PairingCompletion;
use holon_loro::loro_backend::LoroBackend;
use holon_loro::loro_backend::NewBlockWithProperties;
use holon_loro::pairing_swap::PairingMarker;
use holon_pbt_core::ComponentSet;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

/// The archived content this device wrote before the pair: a page and a note
/// under it. The page hangs under the bundled layout root, which the re-import
/// never carries and this store does not hold, so both blocks are owed until
/// the store gains a node for the page itself.
const OWED_PAGE: &str = "pair-owed-page";
const ABSENT_PARENT: &str = "root-layout";
const OWED_NOTE: &str = "pair-owed-note";
const OWED_BLOCKS: usize = 2;
const OWED_PAGE_TEXT: &str = "Phone page";

fn new_block(parent: EntityUri, id: &str, content: &str) -> NewBlockWithProperties {
    NewBlockWithProperties {
        parent_id: parent,
        id: EntityUri::block(id),
        content: BlockContent::text(content),
        properties: HashMap::new(),
        edges: BlockEdges::default(),
    }
}

async fn write_into(store: &LoroDocumentStore, blocks: Vec<NewBlockWithProperties>) {
    let doc = store
        .get_doc(DocScope::Global)
        .await
        .expect("the global doc");
    LoroBackend::from_document(doc)
        .create_blocks_with_properties(blocks)
        .await
        .expect("writing blocks into the global doc");
}

async fn live_ids(store: &LoroDocumentStore) -> Vec<String> {
    store
        .get_doc(DocScope::Global)
        .await
        .expect("the global doc")
        .with_read(|d| Ok(holon_loro::build_tid_index(d)))
        .expect("reading the tid index")
        .into_values()
        .collect()
}

/// Leave this session's store looking like a device whose pair swapped in the
/// owner's document and whose re-import never ran: the marker on disk and the
/// archive holding a subtree the store has no parent for.
async fn owe_a_reimport(store_dir: &std::path::Path) -> PairingMarker {
    let archive = store_dir.join("archive").join("20260905T101500Z");
    std::fs::create_dir_all(&archive).expect("the archive directory");
    let archived = LoroDocumentStore::new(archive.clone());
    write_into(
        &archived,
        vec![
            new_block(EntityUri::no_parent(), ABSENT_PARENT, "Layout"),
            new_block(EntityUri::block(ABSENT_PARENT), OWED_PAGE, OWED_PAGE_TEXT),
            new_block(EntityUri::block(OWED_PAGE), OWED_NOTE, "bought milk"),
        ],
    )
    .await;
    archived.save_all().await.expect("the archive persists");

    let marker = PairingMarker {
        archive,
        staging: store_dir.join("staging-20260905T101500Z"),
        owner: "owner-endpoint".to_string(),
        started_at: "2026-09-05T10:15:00Z".to_string(),
    };
    holon_loro::pairing_swap::write_marker(store_dir, &marker).expect("the pairing marker");
    marker
}

/// Dispatch a real left click at `center`.
fn click_at(
    app: &mut HeadlessAppContext,
    window: gpui::AnyWindowHandle,
    center: Point<Pixels>,
    what: &str,
) {
    app.update(|cx| {
        window
            .update(cx, |_, win, cx| {
                win.dispatch_event(
                    gpui::MouseMoveEvent {
                        position: center,
                        pressed_button: None,
                        modifiers: Default::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
                win.dispatch_event(
                    gpui::MouseDownEvent {
                        position: center,
                        button: MouseButton::Left,
                        modifiers: Default::default(),
                        click_count: 1,
                        first_mouse: false,
                    }
                    .to_platform_input(),
                    cx,
                );
                win.dispatch_event(
                    gpui::MouseUpEvent {
                        position: center,
                        button: MouseButton::Left,
                        modifiers: Default::default(),
                        click_count: 1,
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap_or_else(|e| panic!("window alive for the {what} click: {e}"));
    });
}

fn center_of(info: &holon_frontend::geometry::ElementInfo) -> Point<Pixels> {
    let (x, y) = info.center();
    Point {
        x: Pixels::from(x),
        y: Pixels::from(y),
    }
}

/// Every text the window painted, so a failure names what the user would have
/// seen instead of only that an assertion tripped.
fn painted_texts(bounds: &BoundsRegistry) -> Vec<String> {
    bounds
        .all_elements()
        .into_iter()
        .filter_map(|(_, info)| info.displayed_text.as_deref().map(str::to_string))
        .collect()
}

fn bottom_of(info: &holon_frontend::geometry::ElementInfo) -> f32 {
    info.y + info.height
}

fn overlaps_vertically(
    a: &holon_frontend::geometry::ElementInfo,
    b: &holon_frontend::geometry::ElementInfo,
) -> bool {
    a.y < bottom_of(b) && b.y < bottom_of(a)
}

/// Every data-bound element the window painted, topmost first. Chrome carries
/// no entity, so what is left is the content the bar must not sit on.
fn content_rows(bounds: &BoundsRegistry) -> Vec<(String, holon_frontend::geometry::ElementInfo)> {
    let mut rows: Vec<(String, holon_frontend::geometry::ElementInfo)> = bounds
        .all_elements()
        .into_iter()
        .filter(|(_, info)| info.entity_id.is_some() && info.height > 0.0 && info.width > 0.0)
        .collect();
    rows.sort_by(|(_, a), (_, b)| a.y.total_cmp(&b.y));
    rows
}

fn tracked_text(bounds: &BoundsRegistry, id: &str) -> String {
    bounds
        .element_info(id)
        .and_then(|info| info.displayed_text)
        .map(|text| text.to_string())
        .unwrap_or_default()
}

#[test]
fn the_window_paints_the_deferred_reimport_banner_and_its_retry_completes_the_pair() {
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));

    let set = ComponentSet::full_headless();
    let bundle = runtime
        .block_on(async { compose_sut_windowed_base_seeded(&set, &resolver, &[], &[]).await });
    let session = bundle
        .session
        .clone()
        .expect("full_headless -> booted FrontendSession");
    let engine = bundle
        .reactive
        .clone()
        .expect("full_headless -> booted ReactiveEngine");
    let frontend = bundle
        .frontend
        .clone()
        .expect("full_headless -> booted frontend component");

    // The store, the bus and the pairing instance are all this session's own,
    // so the retry button reaches exactly what the boot call below touched.
    let store = frontend
        .loro_doc_store()
        .expect("the booted session's Loro store");
    let bus = frontend
        .degraded_bus()
        .expect("the booted session's degraded bus");
    let pairing = frontend
        .device_pairing()
        .expect("the booted session's DevicePairing");
    let store_dir = store.storage_dir().to_path_buf();

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
                Some(bus.clone()),
                "Holon-PairingDeferredReimport-Windowed",
                cx,
            )
        })
        .expect("window opened over the booted session");
    let window = rebind.window();

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    assert!(
        bounds.element_info(DEFERRED_REIMPORT_BANNER).is_none(),
        "precondition: nothing is owed yet, so the banner must not be painted"
    );

    let live = runtime.block_on(async { live_ids(&store).await });
    assert!(
        !live
            .iter()
            .any(|id| id == &format!("block:{ABSENT_PARENT}")),
        "precondition: the archived page must have nowhere to go; the store holds: {live:?}"
    );

    let marker = runtime.block_on(async { owe_a_reimport(&store_dir).await });

    // The boot call. Its refusal is the one D94.a turns into a degraded boot.
    let outcome = runtime.block_on(async { pairing.complete_interrupted_pairing(&marker).await });
    let PairingCompletion::Deferred { orphans, .. } =
        outcome.expect("an unsatisfiable re-import must not stop the app")
    else {
        panic!("the archived blocks have no parent, so the pair cannot report completion");
    };
    assert_eq!(orphans.len(), OWED_BLOCKS, "owed: {orphans:?}");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let texts = painted_texts(&bounds);
    let banner = bounds.element_info(DEFERRED_REIMPORT_BANNER).unwrap_or_else(|| {
        panic!(
            "a deferred re-import must paint {DEFERRED_REIMPORT_BANNER}; without it the user is \
             never told that {OWED_BLOCKS} of their blocks are missing. painted: {texts:?}"
        )
    });
    let painted = banner
        .displayed_text
        .as_deref()
        .unwrap_or_default()
        .to_string();
    assert!(
        painted.contains(&OWED_BLOCKS.to_string()),
        "the banner must say HOW MANY blocks are missing; it painted: {painted:?}"
    );
    let archive = marker.archive.display().to_string();
    assert!(
        painted.contains(&archive),
        "the banner must name the archive {archive} — it is the only handle on the missing \
         blocks; it painted: {painted:?}"
    );
    let title_row = bounds
        .element_info(TITLE_ROW_ID)
        .unwrap_or_else(|| panic!("the window paints a title row; painted: {texts:?}"));
    assert!(
        !overlaps_vertically(&banner, &title_row),
        "the bar must not cover the title row; the bar spans y {}..{} and the title row spans y \
         {}..{}",
        banner.y,
        bottom_of(&banner),
        title_row.y,
        bottom_of(&title_row)
    );

    let rows = content_rows(&bounds);
    assert!(
        !rows.is_empty(),
        "the window paints content to compare the bar against; painted: {texts:?}"
    );
    if let Some((id, row)) = rows
        .iter()
        .find(|(_, row)| overlaps_vertically(&banner, row))
    {
        panic!(
            "the bar must not cover content; it spans y {}..{} and {id} spans y {}..{}",
            banner.y,
            bottom_of(&banner),
            row.y,
            bottom_of(row)
        );
    }

    let retry = bounds.element_info(DEFERRED_REIMPORT_RETRY).unwrap_or_else(|| {
        panic!(
            "the banner must paint {DEFERRED_REIMPORT_RETRY}: the re-import has no command and no \
             keybinding, so this button is the only way to run it. painted: {texts:?}"
        )
    });

    // Pressing retry while the parent is still absent must leave the banner
    // standing and say why — a banner that clears on a refusal would tell the
    // user their blocks arrived when they did not.
    click_at(&mut app, window, center_of(&retry), "Retry re-import");
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));
    assert!(
        bounds.element_info(DEFERRED_REIMPORT_BANNER).is_some(),
        "the blocks still have no parent, so the banner must survive the press"
    );
    let toasts = tracked_text(&bounds, DEGRADED_TOAST_STACK);
    for named in [OWED_PAGE, OWED_NOTE] {
        assert!(
            toasts.contains(named),
            "the press must tell the user which blocks are waiting on what; the toast stack \
             painted {toasts:?} and does not name block:{named}"
        );
    }

    // The store gains a node for the archived page, so the same button can now
    // finish the pair. `block:pair-owed-page` becomes an id both sides hold,
    // and the note hangs under this one.
    let host = {
        let mut ids = runtime.block_on(async { live_ids(&store).await });
        ids.sort();
        ids.into_iter()
            .next()
            .expect("the booted store holds at least one block")
    };
    runtime.block_on(async {
        write_into(
            &store,
            vec![new_block(
                EntityUri::parse(&host).expect("a live block id is a uri"),
                OWED_PAGE,
                OWED_PAGE_TEXT,
            )],
        )
        .await;
    });

    click_at(
        &mut app,
        window,
        center_of(&retry),
        "Retry re-import (satisfiable)",
    );
    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    assert!(
        holon_loro::pairing_swap::read_marker(&store_dir)
            .expect("reading the marker")
            .is_none(),
        "a completed re-import owes nothing, so the marker is gone"
    );
    let ids = runtime.block_on(async { live_ids(&store).await });
    assert!(
        ids.iter().any(|id| id == &format!("block:{OWED_NOTE}")),
        "the retry must have put the archived note in the store; it holds: {ids:?}"
    );
    assert!(
        bounds.element_info(DEFERRED_REIMPORT_BANNER).is_none(),
        "the banner lifts when the condition clears; it is still painted: {:?}",
        painted_texts(&bounds)
    );

    // The detached bridge pump still holds an `Entity<ShareUiState>` when
    // gpui's leak detector runs on drop.
    std::mem::forget(app);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
