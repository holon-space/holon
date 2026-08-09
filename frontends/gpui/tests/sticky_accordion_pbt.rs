//! Inc C windowed dedicated PBT — the sticky / in-flow accordion variants.
//!
//! Drives a PRODUCTION-shaped `section_stack( <content section>, accordion(
//! sticky, …) )` through the real `builders::render` pipeline over a
//! definite-height window, then evaluates the SHARED observational spec
//! (`holon_frontend::sticky_accordion` checkers — the same pure functions the
//! shared-catalog PBT invariant bodies wrap, so promotion to the keystone is a
//! move, not a rewrite):
//!
//!   position-spec, exactly-one-footer, no-overlap, cap-under-sticky,
//!   overlay-bounds-committed, settle-stability.
//!
//! History: with the sticky overlay stubbed (feature missing) every footer
//! checker went red — no `accordion_sticky_footer` element committed
//! (red-run-stickyimpl.log). `render_sticky` (absolute `.occlude()` + px-cap +
//! definite-height fail-loud) turns them green.
//!
//! Run: `cargo test -p holon-gpui --test sticky_accordion_pbt`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::Entity;
use gpui::Size;
use gpui::TestAppContext;
use gpui::VisualTestContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::sticky_accordion as sa;
use holon_gpui::geometry::BoundsRegistry;
use support::ReactiveFixtureView;

fn text_item(id: String, label: String) -> ReactiveViewModel {
    let mut data = HashMap::new();
    data.insert("id".into(), Value::String(id));
    data.insert("content".into(), Value::String(label.clone()));
    let mut props = HashMap::new();
    props.insert("content".into(), Value::String(label));
    props.insert("field".into(), Value::String("content".into()));
    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = Mutable::new(Arc::new(data)).read_only();
    vm
}

/// `section_stack( column(<content rows>), accordion(sticky, <footer rows>) )`.
/// `footer_rows` tall enough to exceed the cap so the px-cap engages.
fn sticky_root(content_rows: usize, footer_rows: usize, fraction: f64) -> Arc<ReactiveViewModel> {
    let content: Vec<Arc<ReactiveViewModel>> = (0..content_rows)
        .map(|i| Arc::new(text_item(format!("crow-{i}"), format!("content row {i}"))))
        .collect();
    let mut content_col = ReactiveViewModel::from_widget("column", HashMap::new());
    content_col.children = content;

    let footer_children: Vec<Arc<ReactiveViewModel>> = (0..footer_rows)
        .map(|i| Arc::new(text_item(format!("frow-{i}"), format!("footer row {i}"))))
        .collect();
    let mut acc_props = HashMap::new();
    acc_props.insert("title".to_string(), Value::String("Sticky footer".into()));
    acc_props.insert("max_height_fraction".to_string(), Value::Float(fraction));
    acc_props.insert("collapsible".to_string(), Value::Boolean(true));
    acc_props.insert("collapsed".to_string(), Value::Boolean(false));
    acc_props.insert("placement".to_string(), Value::String("sticky".into()));
    let accordion = Arc::new(ReactiveViewModel {
        children: footer_children,
        expanded: Some(Mutable::new(true)),
        ..ReactiveViewModel::from_widget("accordion", acc_props)
    });

    let mut props = HashMap::new();
    props.insert("section_stack".to_string(), Value::Boolean(true));
    let mut ss = ReactiveViewModel::from_widget("section_stack", props);
    ss.children = vec![Arc::new(content_col), accordion];
    Arc::new(ss)
}

fn observe(bounds: &BoundsRegistry) -> Vec<sa::ObservedRect> {
    bounds
        .all_elements()
        .into_iter()
        .map(|(_, i)| sa::ObservedRect {
            widget_type: i.widget_type.to_string(),
            entity_id: i.entity_id.as_deref().map(str::to_string),
            x: i.x,
            y: i.y,
            w: i.width,
            h: i.height,
        })
        .collect()
}

fn settle(
    entity: &Entity<ReactiveFixtureView>,
    vcx: &mut VisualTestContext,
    bounds: &BoundsRegistry,
) {
    for _ in 0..4 {
        entity.update(&mut vcx.cx.clone(), |_, cx| cx.notify());
        vcx.run_until_parked();
    }
    bounds.flush();
}

fn open_stack<'a>(
    cx: &'a mut TestAppContext,
    vm: Arc<ReactiveViewModel>,
    viewport: Size<gpui::Pixels>,
) -> (
    Entity<ReactiveFixtureView>,
    &'a mut VisualTestContext,
    BoundsRegistry,
) {
    cx.update(|cx| gpui_component::init(cx));
    let bounds = BoundsRegistry::new();
    let (entity, vcx) = cx.add_window_view({
        let (v, b) = (vm.clone(), bounds.clone());
        move |_, _| ReactiveFixtureView::with_bounds(v, viewport, b)
    });
    vcx.run_until_parked();
    settle(&entity, vcx, &bounds);
    (entity, vcx, bounds)
}

#[gpui::test]
fn sticky_accordion_overlay_holds_the_spec(cx: &mut TestAppContext) {
    let fraction = 0.4_f64;
    let viewport = size(px(420.0), px(320.0));
    let (entity, vcx, bounds) = open_stack(cx, sticky_root(12, 16, fraction), viewport);

    let snap_a = observe(&bounds);
    settle(&entity, vcx, &bounds);
    let snap_b = observe(&bounds);

    let mut failures = sa::check_all_single(&snap_a, fraction as f32);
    if let Err(e) = sa::check_settle_stability(&snap_a, &snap_b) {
        failures.push(e);
    }

    if !failures.is_empty() {
        eprintln!("[sticky_accordion_pbt] {} spec failure(s):", failures.len());
        for f in &failures {
            eprintln!("  RED {f}");
        }
    }
    assert!(
        failures.is_empty(),
        "sticky accordion spec RED: {} failure(s) (see stderr)",
        failures.len()
    );
    eprintln!("[sticky_accordion_pbt] GREEN — all 6 observational checks pass");
}

#[gpui::test]
fn in_flow_accordion_renders_inline_not_error(cx: &mut TestAppContext) {
    // pinned:false in-flow accordion inside a section stack renders a real
    // capped region, NOT the placement error widget.
    let viewport = size(px(420.0), px(320.0));
    let mut acc_props = HashMap::new();
    acc_props.insert("title".to_string(), Value::String("In-flow".into()));
    acc_props.insert("max_height_fraction".to_string(), Value::Float(0.4));
    acc_props.insert("placement".to_string(), Value::String("in_flow".into()));
    let accordion = Arc::new(ReactiveViewModel {
        children: vec![Arc::new(text_item("if-0".into(), "row 0".into()))],
        expanded: Some(Mutable::new(true)),
        ..ReactiveViewModel::from_widget("accordion", acc_props)
    });
    let mut props = HashMap::new();
    props.insert("section_stack".to_string(), Value::Boolean(true));
    let mut ss = ReactiveViewModel::from_widget("section_stack", props);
    ss.children = vec![accordion];
    let (_e, _vcx, bounds) = open_stack(cx, Arc::new(ss), viewport);

    let obs = observe(&bounds);
    let has_error = obs
        .iter()
        .any(|o| o.widget_type == "error" || o.widget_type == "accordion");
    // The in-flow accordion renders as a tracked section, never the misplaced
    // "accordion" error path.
    assert!(
        !has_error,
        "in-flow accordion rendered an error/misplaced widget: {:?}",
        obs.iter().map(|o| &o.widget_type).collect::<Vec<_>>()
    );
}

// ── Inc E: Journals-shaped multi-section stack + the three real catalog bodies
// ──

use std::time::Duration;

use holon_api::EntityUri;
use holon_integration_tests::pbt::invariants::bodies::sticky_accordion_spec::InvStickyAccordionSpec;
use holon_integration_tests::pbt::invariants::bodies::wheel_occlusion_routing::InvWheelOcclusionRouting;
use holon_integration_tests::pbt::invariants::bodies::wheel_two_mode_motion_law::InvWheelTwoModeMotionLaw;
use holon_pbt_core::capabilities::RenderedElement;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantResult;

/// Minimal `SutLayout` over a captured geometry snapshot — lets the dedicated
/// windowed test drive the REAL composed catalog invariant bodies
/// (`InvStickyAccordionSpec` / `InvWheel*`) without booting a full
/// `ComposedSut`, so promotion to the keystone (when Journals gets a
/// section_stack profile) is a move, not a rewrite. Only `rendered_elements` is
/// exercised by the three bodies; the rest are inert.
struct StickyLayoutSut {
    elements: Vec<RenderedElement>,
}

#[async_trait::async_trait(?Send)]
impl SutLayout for StickyLayoutSut {
    async fn rendered_elements(&self) -> Vec<RenderedElement> {
        self.elements.clone()
    }
    async fn visual_content_fraction(&self) -> Option<f32> {
        None
    }
    async fn has_registered_bounds(&self, _: &EntityUri) -> bool {
        false
    }
    async fn has_draggable_handle(&self, _: &EntityUri) -> bool {
        false
    }
    async fn any_error_widget(&self) -> bool {
        false
    }
    async fn wait_for_bounds(&self, _: &EntityUri, _: Duration) -> Result<(), String> {
        Ok(())
    }
    async fn wait_for_widget_kind(
        &self,
        _: &EntityUri,
        _: &[&str],
        _: Duration,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn wait_for_window_focused_editor(
        &self,
        _: &EntityUri,
        _: Duration,
    ) -> Result<(), String> {
        Ok(())
    }
}

fn to_rendered(bounds: &BoundsRegistry) -> Vec<RenderedElement> {
    bounds
        .all_elements()
        .into_iter()
        .map(|(el_id, i)| RenderedElement {
            el_id,
            widget_type: i.widget_type.to_string(),
            entity_id: i
                .entity_id
                .as_deref()
                .map(|s| EntityUri::parse(s).unwrap_or_else(|_| EntityUri::block(s))),
            displayed_text: None,
            x: i.x,
            y: i.y,
            width: i.width,
            height: i.height,
            has_content: true,
            parent_id: None,
            expected_size_violation: None,
            is_error_widget: false,
            focused: None,
            styled_runs: None,
        })
        .collect()
}

/// `section_stack( column(rows)×N , accordion(sticky, footer_rows) )` — N
/// variable-height content sections + one active sticky footer.
fn journals_root(
    section_rows: &[usize],
    footer_rows: usize,
    fraction: f64,
) -> Arc<ReactiveViewModel> {
    let mut children: Vec<Arc<ReactiveViewModel>> = Vec::new();
    for (s, &n) in section_rows.iter().enumerate() {
        let rows: Vec<Arc<ReactiveViewModel>> = (0..n)
            .map(|r| {
                Arc::new(text_item(
                    format!("s{s}r{r}"),
                    format!("section {s} row {r}"),
                ))
            })
            .collect();
        let mut col = ReactiveViewModel::from_widget("column", HashMap::new());
        col.children = rows;
        children.push(Arc::new(col));
    }
    let footer_children: Vec<Arc<ReactiveViewModel>> = (0..footer_rows)
        .map(|i| Arc::new(text_item(format!("frow-{i}"), format!("footer row {i}"))))
        .collect();
    let mut acc_props = HashMap::new();
    acc_props.insert("title".to_string(), Value::String("Journals".into()));
    acc_props.insert("max_height_fraction".to_string(), Value::Float(fraction));
    acc_props.insert("collapsible".to_string(), Value::Boolean(true));
    acc_props.insert("collapsed".to_string(), Value::Boolean(false));
    acc_props.insert("placement".to_string(), Value::String("sticky".into()));
    children.push(Arc::new(ReactiveViewModel {
        children: footer_children,
        expanded: Some(Mutable::new(true)),
        ..ReactiveViewModel::from_widget("accordion", acc_props)
    }));

    let mut props = HashMap::new();
    props.insert("section_stack".to_string(), Value::Boolean(true));
    let mut ss = ReactiveViewModel::from_widget("section_stack", props);
    ss.children = children;
    Arc::new(ss)
}

fn y_of(obs: &[sa::ObservedRect], entity_id: &str) -> f32 {
    obs.iter()
        .find(|o| o.entity_id.as_deref() == Some(entity_id))
        .map(|o| o.y)
        .unwrap_or(f32::NAN)
}

fn footer_top(obs: &[sa::ObservedRect]) -> f32 {
    obs.iter()
        .find(|o| o.widget_type == sa::STICKY_FOOTER_WIDGET)
        .map(|o| o.y)
        .unwrap_or(f32::NAN)
}

fn engaged(r: &InvariantResult) -> bool {
    matches!(r, InvariantResult::Ok | InvariantResult::Fail(_))
}

#[gpui::test]
fn journals_multi_section_engages_all_three_invariants(cx: &mut TestAppContext) {
    let fraction = 0.4_f64;
    let viewport = size(px(440.0), px(340.0));
    let (entity, vcx, bounds) =
        open_stack(cx, journals_root(&[6, 9, 7, 8], 16, fraction), viewport);

    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("rt");

    // Section stack + one active sticky footer rendered.
    let pre_obs = observe(&bounds);
    let pre_top = footer_top(&pre_obs);
    let outer_id = "section:section-stack:0";
    let outer_pre = y_of(&pre_obs, outer_id);
    let footer_row_pre = y_of(&pre_obs, "frow-0");

    // (1) inv-sticky-accordion-spec over the multi-section render.
    let sut = StickyLayoutSut {
        elements: to_rendered(&bounds),
    };
    let r_sticky = rt.block_on(InvStickyAccordionSpec.check(&(), &sut));
    let sticky_engaged = engaged(&r_sticky) as u32;

    // Drive a real wheel over the OUTER list (mid-height, left of the footer).
    let list_pos = gpui::point(px(120.0), px(80.0));
    support::simulate_wheel_at(vcx, list_pos, px(60.0));
    settle(&entity, vcx, &bounds);
    let post_list = observe(&bounds);
    let obs_list = sa::WheelObservation {
        over_footer: false,
        delta_y: 60.0,
        footer_top_before: pre_top,
        footer_top_after: footer_top(&post_list),
        outer_offset_before: outer_pre,
        outer_offset_after: y_of(&post_list, outer_id),
        footer_offset_before: footer_row_pre,
        footer_offset_after: y_of(&post_list, "frow-0"),
    };

    // (2)+(3) wheel invariants over the OUTER-LIST wheel.
    sa::set_wheel_observation(Some(obs_list));
    let sut2 = StickyLayoutSut {
        elements: to_rendered(&bounds),
    };
    let r_motion_list = rt.block_on(InvWheelTwoModeMotionLaw.check(&(), &sut2));
    sa::set_wheel_observation(Some(obs_list));
    let r_occl_list = rt.block_on(InvWheelOcclusionRouting.check(&(), &sut2));

    // Drive a real wheel over the FOOTER (occluded) — internal scroll only.
    let ftop_now = footer_top(&post_list);
    let footer_pos = gpui::point(px(220.0), px(ftop_now + 30.0));
    let outer_before_footer = y_of(&post_list, outer_id);
    let footer_row_before = y_of(&post_list, "frow-0");
    support::simulate_wheel_at(vcx, footer_pos, px(60.0));
    settle(&entity, vcx, &bounds);
    let post_footer = observe(&bounds);
    let obs_footer = sa::WheelObservation {
        over_footer: true,
        delta_y: 60.0,
        footer_top_before: ftop_now,
        footer_top_after: footer_top(&post_footer),
        outer_offset_before: outer_before_footer,
        outer_offset_after: y_of(&post_footer, outer_id),
        footer_offset_before: footer_row_before,
        footer_offset_after: y_of(&post_footer, "frow-0"),
    };
    sa::set_wheel_observation(Some(obs_footer));
    let sut3 = StickyLayoutSut {
        elements: to_rendered(&bounds),
    };
    let r_motion_footer = rt.block_on(InvWheelTwoModeMotionLaw.check(&(), &sut3));
    sa::set_wheel_observation(Some(obs_footer));
    let r_occl_footer = rt.block_on(InvWheelOcclusionRouting.check(&(), &sut3));

    let motion_engaged = engaged(&r_motion_list) as u32 + engaged(&r_motion_footer) as u32;
    let occl_engaged = engaged(&r_occl_list) as u32 + engaged(&r_occl_footer) as u32;

    eprintln!(
        "[journals-engagement] inv-sticky-accordion-spec={sticky_engaged} \
         inv-wheel-two-mode-motion-law={motion_engaged} inv-wheel-occlusion-routing={occl_engaged}"
    );
    eprintln!(
        "  sticky={r_sticky:?}\n  motion(list)={r_motion_list:?}\n  motion(footer)={r_motion_footer:?}\n  \
         occl(list)={r_occl_list:?}\n  occl(footer)={r_occl_footer:?}"
    );

    // DONE-CRITERIA: non-zero engagement (Ok/Fail, never all-Skip) for each body.
    assert!(
        sticky_engaged >= 1,
        "inv-sticky-accordion-spec never engaged (vacuous)"
    );
    assert!(
        motion_engaged >= 1,
        "inv-wheel-two-mode-motion-law never engaged (vacuous)"
    );
    assert!(
        occl_engaged >= 1,
        "inv-wheel-occlusion-routing never engaged (vacuous)"
    );
    // And the engaged runs must PASS (Ok, not Fail).
    for (name, r) in [
        ("sticky", &r_sticky),
        ("motion(list)", &r_motion_list),
        ("motion(footer)", &r_motion_footer),
        ("occl(list)", &r_occl_list),
        ("occl(footer)", &r_occl_footer),
    ] {
        assert!(
            !matches!(r, InvariantResult::Fail(_)),
            "invariant {name} FAILED: {r:?}"
        );
    }
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
