//! TestPlatform-backed windowed replay service — synchronous, single-threaded,
//! no channels, no real NSWindow.
//!
//! Replaces the channel-based `WindowHost`/`WindowedReplayer` pair from
//! `windowed_replay.rs` with a direct-dispatch variant that runs inside
//! `TestApp`. The proptest state machine, signature pinning, capture format,
//! and `seen_counter` semantics are unchanged — this swaps only the host.

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AppContext as _, InputEvent, Keystroke, MouseButton, Pixels, Point, TestApp};
use holon_api::{EntityUri, KeyChord, Region, Value};
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::user_driver::UserDriver;
use holon_frontend::{OperationIntent, ReactiveViewModel};
use holon_gpui::geometry::BoundsRegistry;
use holon_gpui::RebindHandle;
use holon_integration_tests::pbt::fixtures::FixtureStep;
use holon_integration_tests::pbt::phased::{
    replay_fixture_with_driver_sync_callback, PbtReadyContext, PbtReadyResult,
};

// ---------------------------------------------------------------------------
// SimUserDriver — UserDriver over direct TestPlatform dispatch
// ---------------------------------------------------------------------------

/// Direct-dispatch UserDriver for TestPlatform. Holds a raw pointer to the
/// `TestApp` owned by `SimReplayer`. SAFETY: SimReplayer outlives all
/// SimUserDriver instances; all access is from one thread.
pub(crate) struct SimUserDriver {
    app_ptr: *const TestApp,
    window: gpui::AnyWindowHandle,
    bounds: BoundsRegistry,
    engine: Arc<ReactiveEngine>,
}

unsafe impl Sync for SimUserDriver {}
unsafe impl Send for SimUserDriver {}

impl SimUserDriver {
    fn update_app<R>(&self, f: impl FnOnce(&mut gpui::App) -> R) -> R {
        let app = unsafe { &mut *(self.app_ptr as *mut TestApp) };
        app.update(|cx| f(cx))
    }

    fn bounds_center_f32(&self, entity_id: &EntityUri) -> Option<(f32, f32)> {
        let info = self.bounds.element_info(entity_id.as_str())?;
        Some((info.x + info.width / 2.0, info.y + info.height / 2.0))
    }

    fn mouse_point(&self, entity_id: &EntityUri) -> Option<Point<Pixels>> {
        let (cx, cy) = self.bounds_center_f32(entity_id)?;
        Some(Point {
            x: Pixels::from(cx),
            y: Pixels::from(cy),
        })
    }
}

#[async_trait::async_trait]
impl UserDriver for SimUserDriver {
    async fn synthetic_dispatch(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<(), anyhow::Error> {
        let intent = OperationIntent::new(entity.into(), op.into(), params);
        self.engine.dispatch_intent_sync(intent).await
    }

    async fn send_key_chord(
        &self,
        _: &EntityUri,
        _: &ReactiveViewModel,
        _: &EntityUri,
        chord: &KeyChord,
        _: HashMap<String, Value>,
    ) -> Result<bool, anyhow::Error> {
        // Dispatch each key in the chord via real keystrokes.
        for key in &chord.0 {
            let ks_str = holon_gpui::user_driver::keystroke_name(key)
                .unwrap_or_else(|| format!("{:?}", key));
            self.update_app(|cx| {
                if let Ok(ks) = Keystroke::parse(&ks_str) {
                    self.window
                        .update(cx, |_, window, cx| {
                            window.dispatch_keystroke(ks, cx);
                        })
                        .unwrap();
                }
            });
        }
        // In sim mode we can't synchronously check if the chord was consumed.
        // Return true and let invariants catch mismatches.
        Ok(true)
    }

    async fn click_entity(&self, entity_id: &EntityUri, _: &str) -> Result<(), anyhow::Error> {
        let Some(pos) = self.mouse_point(entity_id) else {
            anyhow::bail!("entity {entity_id} not in bounds");
        };
        self.update_app(|cx| {
            self.window
                .update(cx, |_, window, cx| {
                    window.dispatch_event(
                        gpui::MouseDownEvent {
                            position: pos,
                            button: MouseButton::Left,
                            modifiers: Default::default(),
                            click_count: 1,
                            first_mouse: false,
                        }
                        .to_platform_input(),
                        cx,
                    );
                    window.dispatch_event(
                        gpui::MouseUpEvent {
                            position: pos,
                            button: MouseButton::Left,
                            modifiers: Default::default(),
                            click_count: 1,
                        }
                        .to_platform_input(),
                        cx,
                    );
                })
                .unwrap();
        });
        Ok(())
    }

    async fn click_entity_with_tree(
        &self,
        _: &EntityUri,
        _: &ReactiveViewModel,
        entity_id: &EntityUri,
        _: &str,
    ) -> Result<bool, anyhow::Error> {
        self.click_entity(entity_id, "").await?;
        Ok(false)
    }

    fn is_widget_visible(&self, entity_id: &EntityUri) -> bool {
        self.bounds
            .element_info(entity_id.as_str())
            .map(|info| info.has_visible_area())
            .unwrap_or(false)
    }

    fn is_in_region(&self, entity_id: &EntityUri, _: Region) -> bool {
        self.bounds.element_info(entity_id.as_str()).is_some()
    }

    fn entities_in_region(&self, _: Region) -> Vec<EntityUri> {
        self.bounds
            .all_elements()
            .into_iter()
            .filter_map(|(_, info)| info.entity_id.and_then(|id| EntityUri::parse(&id).ok()))
            .collect()
    }

    fn reachable_entities_in_region(&self, region: Region) -> Vec<EntityUri> {
        self.entities_in_region(region)
    }

    async fn scroll_to_entity(&self, entity_id: &EntityUri) -> Result<(), anyhow::Error> {
        self.scroll_entity(entity_id, 0.0, -1000.0).await
    }

    fn click_intent_of(&self, _: &EntityUri) -> Option<OperationIntent> {
        None
    }

    fn displayed_text(&self, entity_id: &EntityUri) -> Option<String> {
        self.bounds
            .element_info(entity_id.as_str())
            .and_then(|info| info.displayed_text.map(|s| s.to_string()))
    }

    async fn scroll_at(&self, x: f32, y: f32, dx: f32, dy: f32) -> Result<(), anyhow::Error> {
        let point = Point {
            x: Pixels::from(x),
            y: Pixels::from(y),
        };
        self.update_app(|cx| {
            self.window
                .update(cx, |_, window, cx| {
                    window.dispatch_event(
                        gpui::ScrollWheelEvent {
                            position: point,
                            delta: gpui::ScrollDelta::Lines(Point { x: dx, y: dy }),
                            modifiers: Default::default(),
                            touch_phase: gpui::TouchPhase::Moved,
                        }
                        .to_platform_input(),
                        cx,
                    );
                })
                .unwrap();
        });
        Ok(())
    }

    async fn scroll_entity(
        &self,
        entity_id: &EntityUri,
        dx: f32,
        dy: f32,
    ) -> Result<(), anyhow::Error> {
        let Some((cx, cy)) = self.bounds_center_f32(entity_id) else {
            return Ok(());
        };
        self.scroll_at(cx, cy, dx, dy).await
    }

    async fn drop_entity(
        &self,
        _: &EntityUri,
        source_id: &EntityUri,
        target_id: &EntityUri,
    ) -> Result<bool, anyhow::Error> {
        let Some(src) = self.mouse_point(source_id) else {
            anyhow::bail!("source {source_id} not in bounds");
        };
        let Some(dst) = self.mouse_point(target_id) else {
            anyhow::bail!("target {target_id} not in bounds");
        };
        self.update_app(|cx| {
            self.window
                .update(cx, |_, window, cx| {
                    window.dispatch_event(
                        gpui::MouseDownEvent {
                            position: src,
                            button: MouseButton::Left,
                            modifiers: Default::default(),
                            click_count: 1,
                            first_mouse: false,
                        }
                        .to_platform_input(),
                        cx,
                    );
                    window.dispatch_event(
                        gpui::MouseMoveEvent {
                            position: dst,
                            modifiers: Default::default(),
                            pressed_button: Some(MouseButton::Left),
                        }
                        .to_platform_input(),
                        cx,
                    );
                    window.dispatch_event(
                        gpui::MouseUpEvent {
                            position: dst,
                            button: MouseButton::Left,
                            modifiers: Default::default(),
                            click_count: 1,
                        }
                        .to_platform_input(),
                        cx,
                    );
                })
                .unwrap();
        });
        Ok(true)
    }

    async fn send_raw_keystroke(&self, keystroke: &str, _: &[&str]) -> Result<(), anyhow::Error> {
        if let Ok(ks) = Keystroke::parse(keystroke) {
            self.update_app(|cx| {
                self.window
                    .update(cx, |_, window, cx| {
                        window.dispatch_keystroke(ks, cx);
                    })
                    .unwrap();
            });
        }
        Ok(())
    }

    async fn send_raw_keystroke_until_handled(
        &self,
        keystroke: &str,
        modifiers: &[&str],
        _: Duration,
    ) -> Result<(), anyhow::Error> {
        self.send_raw_keystroke(keystroke, modifiers).await
    }

    fn dispatches_chords_via_raw_keystroke(&self) -> bool {
        true
    }

    fn editor_cursor_byte(&self, _: &EntityUri) -> Result<Option<usize>, String> {
        Err("editor caret not observable by SimUserDriver".to_string())
    }
}

// ---------------------------------------------------------------------------
// SimReplayer — synchronous replay host
// ---------------------------------------------------------------------------

pub(crate) struct SimReplayer {
    app: UnsafeCell<TestApp>,
    rebind_handle: UnsafeCell<RebindHandle>,
    window: gpui::AnyWindowHandle,
    bounds: BoundsRegistry,
    rt_handle: tokio::runtime::Handle,
}

unsafe impl Sync for SimReplayer {}

impl SimReplayer {
    pub(crate) fn new(
        app: TestApp,
        rebind_handle: RebindHandle,
        bounds: BoundsRegistry,
        rt_handle: tokio::runtime::Handle,
    ) -> Self {
        let window = rebind_handle.window();
        Self {
            app: UnsafeCell::new(app),
            rebind_handle: UnsafeCell::new(rebind_handle),
            window,
            bounds,
            rt_handle,
        }
    }

    pub fn replay(
        &self,
        wiring: holon_pbt_core::Wiring,
        steps: Vec<FixtureStep>,
        seen_counter: Option<Arc<std::sync::atomic::AtomicUsize>>,
    ) -> Result<(), Box<dyn std::any::Any + Send>> {
        let app_ptr = self.app.get();
        let rebind_ptr = self.rebind_handle.get();
        let window = self.window;
        let bounds = self.bounds.clone();

        let on_ready = move |ctx: &PbtReadyContext| -> Option<PbtReadyResult> {
            let app = unsafe { &mut *app_ptr };
            let rebind = unsafe { &*rebind_ptr };

            // Rebind the window to the new engine.
            app.update(|cx| {
                rebind.rebind(ctx.session.clone(), ctx.reactive_engine.clone(), cx);
            });

            let driver: Arc<dyn UserDriver> = Arc::new(SimUserDriver {
                app_ptr: app_ptr,
                window,
                bounds: bounds.clone(),
                engine: ctx.reactive_engine.clone(),
            });
            Some(PbtReadyResult {
                driver: Some(driver),
                frontend_engine: Some(ctx.reactive_engine.clone()),
                frontend_geometry: Some(Box::new(bounds.clone())),
                frontend_visual_state: None,
            })
        };

        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            replay_fixture_with_driver_sync_callback(wiring, steps, on_ready, seen_counter)
                .expect("sim replay setup failed");
        }))
    }
}
