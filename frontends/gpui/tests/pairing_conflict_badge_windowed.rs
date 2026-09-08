//! D93.a: a pairing conflict copy wears a visible badge.
//!
//! Pairing a device that was already used solo keeps this device's divergent
//! text as a child of the owner's node, marked with the
//! `pairing_conflict_of` property
//! (`holon_loro::device_pairing_op::CONFLICT_OF_PROPERTY`). The property is the
//! ONLY mark: a "conflicted copy" title would be written back into the user's
//! org file. So the property has to reach the screen, or the merge is silent.
//!
//! A real window over a real booted engine is the tier that answers it: the
//! badge is a `block_profile` variant, and only production resolves a block
//! through the shipped profile.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! pairing_conflict_badge_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use gpui::AssetSource;
use gpui::HeadlessAppContext;
use holon_api::EntityUri;
use holon_frontend::geometry::ElementInfo;
use holon_frontend::geometry::GeometryProvider;
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::launch_holon_window_rebindable;
use holon_gpui::navigation_state::NavigationState;
use holon_integration_tests::pbt::composed::builder::compose_sut_windowed_base_seeded;
use holon_integration_tests::pbt::op_write_cap::IdResolver;
use holon_pbt_core::ComponentSet;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::SutFocusWrite;
use pbt_harness::windowed_wide::real_text_system;
use pbt_harness::windowed_wide::settle_to_fixed_point;

/// The owner's node, and the child holding what this device wrote before it
/// was paired. The child's id is what `conflict_copy_id` derives.
const OWNER_TEXT: &str = "the owners words";
const KEPT_TEXT: &str = "the words this device wrote";

/// What the badge has to say. Asserted verbatim: the user reads these words,
/// so a variant that fires but paints something else is still a silent merge.
const BADGE_LABEL: &str = "kept from this device before pairing";

/// A sibling with no `pairing_conflict_of`, so "every block wears a badge"
/// cannot pass this test.
const PLAIN_TEXT: &str = "an ordinary sibling";

const HOST_PAGE: &str = "pair-conflict-page";
const CONFLICT_COPY: &str = "pair-conflict-owner-before-pairing";

/// The conflict copy as the re-import writes it: content carried verbatim, the
/// diverged id in a property, nothing added to the title.
const HOST_ORG: &str = concat!(
    "#+ID: pair-conflict-page\n",
    "* the owners words\n",
    ":PROPERTIES:\n",
    ":ID: pair-conflict-owner\n",
    ":END:\n",
    "** the words this device wrote\n",
    ":PROPERTIES:\n",
    ":ID: pair-conflict-owner-before-pairing\n",
    ":pairing_conflict_of: block:pair-conflict-owner\n",
    ":END:\n",
    "* an ordinary sibling\n",
    ":PROPERTIES:\n",
    ":ID: pair-conflict-plain\n",
    ":END:\n",
);

const LONG_HOST_PAGE: &str = "pair-conflict-long-page";
const LONG_COPY: &str = "pcl-owner-before-pairing";
const WRAPPED_COPY: &str = "pcw-owner-before-pairing";

/// A copy whose content fills the row, and one whose content fills several
/// rows. The badge shares one flex row with the text, so a copy the user is
/// most likely to have written by hand is exactly the one whose mark the text
/// can push off screen.
const LONG_COPY_TEXT: &str = concat!(
    "the words this device wrote before it was paired, typed out at the ",
    "length of a real note so the row runs out of room for anything else",
);
const WRAPPED_COPY_TEXT: &str = concat!(
    "a second copy whose content is long enough to wrap onto several ",
    "lines of the same row, because a note this device wrote before the ",
    "pair is a paragraph and not a title, and the mark has to survive ",
    "every one of those lines just as it survives a short one",
);

const LONG_VARIANT_ORG: &str = concat!(
    "#+ID: pair-conflict-long-page\n",
    "* the owners words\n",
    ":PROPERTIES:\n",
    ":ID: pcl-owner\n",
    ":END:\n",
    "** the words this device wrote before it was paired, typed out at the ",
    "length of a real note so the row runs out of room for anything else\n",
    ":PROPERTIES:\n",
    ":ID: pcl-owner-before-pairing\n",
    ":pairing_conflict_of: block:pcl-owner\n",
    ":END:\n",
    "** a second copy whose content is long enough to wrap onto several ",
    "lines of the same row, because a note this device wrote before the ",
    "pair is a paragraph and not a title, and the mark has to survive ",
    "every one of those lines just as it survives a short one\n",
    ":PROPERTIES:\n",
    ":ID: pcw-owner-before-pairing\n",
    ":pairing_conflict_of: block:pcl-owner\n",
    ":END:\n",
);

const PAGE_HOST_PAGE: &str = "pair-conflict-variant-page";
const PAGE_CONFLICT_COPY: &str = "pcv-owner-before-pairing";
const PAGE_COPY_TEXT: &str = "a page shaped copy";

/// A conflict copy that ALSO matches a specialized variant — here `Page`,
/// which routes a block to `embedded_page`. The copy is a real one; what
/// differs is only what it would render as if the disclosure did not outrank
/// the presentation.
const PAGE_VARIANT_ORG: &str = concat!(
    "#+ID: pair-conflict-variant-page\n",
    "* the owners words\n",
    ":PROPERTIES:\n",
    ":ID: pcv-owner\n",
    ":END:\n",
    "** a page shaped copy :Page:\n",
    ":PROPERTIES:\n",
    ":ID: pcv-owner-before-pairing\n",
    ":pairing_conflict_of: block:pcv-owner\n",
    ":END:\n",
);

/// The window every test in this file opens. A badge that fits only because
/// the window is unusually wide is not a badge the user sees.
const WINDOW: &str = "1512x900";
const WINDOW_W: f32 = 1512.0;
const WINDOW_H: f32 = 900.0;

/// What one window painted: every text on screen, a widget census for failure
/// messages, each badge as `(entity id, label)`, and each badge's rect.
struct Painted {
    texts: Vec<String>,
    census: String,
    badges: BTreeSet<(String, String)>,
    badge_boxes: Vec<(String, ElementInfo)>,
    all: Vec<(String, ElementInfo)>,
}

impl Painted {
    /// Every rect recorded for one block, so a badge failure names what took
    /// the room.
    fn row_rects(&self, entity: &str) -> String {
        self.all
            .iter()
            .filter(|(_, i)| i.entity_id.as_deref() == Some(entity))
            .map(|(id, i)| format!("{id} x={} w={} h={}", i.x, i.width, i.height))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn paint(page_id: &str, file: &str, org: &str, window_title: &'static str) -> Painted {
    // Read by `launch_holon_window_impl`; must be set before the window opens.
    // SAFETY: single-threaded test setup, before any window or runtime thread
    // reads the environment.
    unsafe { std::env::set_var("HOLON_INITIAL_WINDOW_SIZE", WINDOW) };
    let text_system = real_text_system();
    let assets: Arc<dyn AssetSource> = Arc::new(());
    let mut app = HeadlessAppContext::with_platform(text_system, assets, || {
        gpui_platform::current_headless_renderer()
    });

    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let resolver: IdResolver = Arc::new(Mutex::new(BTreeMap::new()));

    let set = ComponentSet::full_headless();
    let seed_files = [(file, org)];
    let bundle = runtime.block_on(async {
        compose_sut_windowed_base_seeded(&set, &resolver, &seed_files, &[]).await
    });
    let session = bundle
        .session
        .clone()
        .expect("full_headless -> booted FrontendSession");
    let engine = bundle
        .reactive
        .clone()
        .expect("full_headless -> booted ReactiveEngine");
    let comp = bundle
        .frontend
        .clone()
        .expect("full_headless -> booted HeadlessFrontendComponent");

    // Focus main on the seeded page BEFORE the window attaches, so the first
    // frame already paints it.
    let host = EntityUri::block(page_id);
    runtime.block_on(async {
        comp.apply_navigate_focus(CapRegion::Main, &host).await;
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
                window_title,
                cx,
            )
        })
        .expect("window opened over the booted session");

    settle_to_fixed_point(&mut app, &bounds, &runtime, Duration::from_secs(30));

    let texts = painted_texts(&bounds);
    let census = painted_widget_census(&bounds);
    // Only the badge builder's own tracker records the words; the registry
    // wrapper around it carries the same widget type and no text.
    let badges: BTreeSet<(String, String)> = bounds
        .all_elements()
        .into_iter()
        .filter(|(_, info)| info.widget_type.as_ref() == "badge")
        .filter_map(|(_, info)| {
            let words = info.displayed_text?.to_string();
            Some((
                info.entity_id.map(|e| e.to_string()).unwrap_or_default(),
                words,
            ))
        })
        .collect();
    let badge_boxes: Vec<(String, ElementInfo)> = bounds
        .all_elements()
        .into_iter()
        .filter(|(_, info)| info.widget_type.as_ref() == "badge")
        .filter(|(_, info)| info.displayed_text.is_some())
        .map(|(id, info)| (id, info))
        .collect();
    let all: Vec<(String, ElementInfo)> = bounds.all_elements().into_iter().collect();

    // Teardown BEFORE the assertions so a red does not also trip the gpui leak
    // detector, which would bury the real failure.
    drop(rebind);
    app.update(|cx| cx.shutdown());
    app.run_until_parked();
    std::mem::forget(app);
    std::mem::forget(bundle);

    Painted {
        texts,
        census,
        badges,
        badge_boxes,
        all,
    }
}

/// Recorded rects are clipped to the content mask, so a badge squeezed out of
/// its row registers with no area or lands outside the window.
fn assert_badges_are_on_screen(painted: &Painted) {
    for (id, info) in &painted.badge_boxes {
        assert!(
            info.has_visible_area()
                && info.x >= 0.0
                && info.y >= 0.0
                && info.x + info.width <= WINDOW_W
                && info.y + info.height <= WINDOW_H,
            "the badge {id} on {:?} must be painted inside the {WINDOW} window; it sits at \
             x={} y={} w={} h={}. Its row: {}",
            info.entity_id,
            info.x,
            info.y,
            info.width,
            info.height,
            painted.row_rects(info.entity_id.as_deref().unwrap_or_default())
        );
    }
}

fn painted_texts(bounds: &BoundsRegistry) -> Vec<String> {
    bounds
        .all_elements()
        .into_iter()
        .filter_map(|(_, i)| i.displayed_text.map(|t| t.to_string()))
        .collect()
}

/// Every widget tag the window painted, with counts, so a failure separates
/// "the page never rendered" from "it rendered and the badge is missing".
fn painted_widget_census(bounds: &BoundsRegistry) -> String {
    let mut tags: BTreeMap<String, usize> = BTreeMap::new();
    for (_, info) in bounds.all_elements() {
        if let Some(node) = &info.vm_node {
            *tags.entry(node.tag.to_string()).or_default() += 1;
        }
    }
    format!("{tags:?}")
}

#[test]
fn a_pairing_conflict_copy_paints_its_badge() {
    let painted = paint(
        HOST_PAGE,
        "pair-conflict-page.org",
        HOST_ORG,
        "Holon-PairingConflictBadge-Windowed",
    );

    assert!(
        painted.texts.iter().any(|t| t.contains(KEPT_TEXT)),
        "precondition: the conflict copy's own content must be on screen, else the badge \
         assertion below judges a page that never rendered. painted: {:?}",
        painted.texts
    );
    assert!(
        painted.texts.iter().any(|t| t.contains(OWNER_TEXT)),
        "precondition: the owner's node must be on screen too. painted: {:?}",
        painted.texts
    );
    let expected: BTreeSet<(String, String)> = [(
        EntityUri::block(CONFLICT_COPY).to_string(),
        BADGE_LABEL.to_string(),
    )]
    .into();
    assert_eq!(
        painted.badges, expected,
        "exactly the block carrying `pairing_conflict_of` must wear the badge, and the badge \
         must name the block it marks — a mark nobody can attribute is not a disclosure. Not \
         the owner node, not the {PLAIN_TEXT:?} sibling. Painted widgets: {}",
        painted.census
    );
    assert_badges_are_on_screen(&painted);
}

/// Content length must not decide whether the merge is disclosed. A copy whose
/// text fills the row, and one whose text fills several, wear the same mark as
/// a short one — and wear it where the user can read it.
#[test]
fn a_long_conflict_copy_still_paints_a_visible_badge() {
    let painted = paint(
        LONG_HOST_PAGE,
        "pair-conflict-long-page.org",
        LONG_VARIANT_ORG,
        "Holon-PairingConflictLong-Windowed",
    );

    for text in [LONG_COPY_TEXT, WRAPPED_COPY_TEXT] {
        assert!(
            painted.texts.iter().any(|t| t.contains(text)),
            "precondition: the copy's own content must be on screen. painted: {:?}",
            painted.texts
        );
    }
    let expected: BTreeSet<(String, String)> = [LONG_COPY, WRAPPED_COPY]
        .into_iter()
        .map(|id| (EntityUri::block(id).to_string(), BADGE_LABEL.to_string()))
        .collect();
    assert_eq!(
        painted.badges, expected,
        "a conflict copy whose content fills the row must still wear its badge. Painted \
         widgets: {}",
        painted.census
    );
    assert_badges_are_on_screen(&painted);
}

/// The disclosure outranks the presentation. A conflict copy that also matches
/// a specialized variant (`Page` -> `embedded_page`) renders as the copy it is;
/// the alternative is the silent merge this feature exists to prevent, wearing
/// the costume of an ordinary page.
#[test]
fn a_conflict_copy_that_is_also_a_page_still_wears_its_badge() {
    let painted = paint(
        PAGE_HOST_PAGE,
        "pair-conflict-variant-page.org",
        PAGE_VARIANT_ORG,
        "Holon-PairingConflictVariant-Windowed",
    );

    assert!(
        painted.texts.iter().any(|t| t.contains(PAGE_COPY_TEXT)),
        "precondition: the copy's own content must be on screen. painted: {:?}",
        painted.texts
    );
    let expected: BTreeSet<(String, String)> = [(
        EntityUri::block(PAGE_CONFLICT_COPY).to_string(),
        BADGE_LABEL.to_string(),
    )]
    .into();
    assert_eq!(
        painted.badges, expected,
        "a `Page`-tagged conflict copy must still wear its badge: `embedded_page` outranking \
         `pairing_conflict` hides the copy behind an ordinary page. Painted widgets: {}",
        painted.census
    );
    assert_badges_are_on_screen(&painted);
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
