//! GPUI `UserDriver` — dispatches UI mutations via the MCP
//! `interaction_tx` channel, which the GPUI window drains into real
//! `PlatformInput` events.
//!
//! This driver never touches the host cursor and works regardless of
//! whether the window is visible, minimized, or on another Space. It is
//! the driver that MCP tools inject into — see `setup_interaction_pump`
//! in this crate's `lib.rs`.
//!
//! `*_entity` methods look up the element's screen position via the
//! injected `GeometryProvider` (backed by `BoundsRegistry`) and delegate
//! to the corresponding coordinate-based variant. **Bounds-missing is a
//! hard error**: the `BoundsRegistry` double-buffers staged → committed
//! per render pass, so interacting with a just-created element before
//! the next `begin_pass` legitimately returns `None`. Tests must call
//! `holon_integration_tests::polling::wait_for_element_bounds` after
//! structural mutations; drivers fail loud instead of synthesizing
//! dispatches that bypass the input path.
//!
//! Current constraint: `element_center` only resolves `block:`-style
//! entity ids (it prepends `render-entity-`). Non-entity user-verb targets
//! are unsupported by this driver.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::channel::mpsc::Sender;
use holon_api::{KeyChord, Value};
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::user_driver::UserDriver;
use holon_mcp::server::{InteractionCommand, InteractionEvent, InteractionResponse};

/// Channel-based `UserDriver` for GPUI. Sends `InteractionCommand`s on
/// the shared `interaction_tx` channel; the GPUI interaction pump drains
/// them on the main thread and dispatches real `PlatformInput` events
/// against the window.
pub struct GpuiUserDriver {
    tx: Sender<InteractionCommand>,
    geometry: Arc<dyn GeometryProvider>,
    engine: Arc<ReactiveEngine>,
}

impl GpuiUserDriver {
    pub fn new(
        tx: Sender<InteractionCommand>,
        geometry: Arc<dyn GeometryProvider>,
        engine: Arc<ReactiveEngine>,
    ) -> Self {
        Self {
            tx,
            geometry,
            engine,
        }
    }

    /// Send an `InteractionEvent` on the channel and await the pump's
    /// oneshot response.
    async fn dispatch_event(&self, event: InteractionEvent) -> Result<InteractionResponse> {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        self.tx
            .clone()
            .try_send(InteractionCommand {
                event,
                response_tx: resp_tx,
            })
            .map_err(|e| anyhow::anyhow!("interaction channel send failed: {e}"))?;
        resp_rx
            .await
            .context("GPUI interaction pump dropped the response channel")
    }

    /// Look up an element's window-space center from the `GeometryProvider`.
    ///
    /// Current constraint: only resolves `block:`-prefixed entity ids. The
    /// debug_assert guards against non-block entity ids silently resolving
    /// to `None` (which would look identical to an un-rendered block).
    ///
    /// Lookup chain: `render-entity-{id}` → `selectable-{id}` → entity_id
    /// scan. The default `index.org` sidebar wraps each row in
    /// `selectable(row(...))` directly, with no outer `render_entity()`,
    /// so sidebar rows register under `selectable-{id}`. Without the
    /// fallback, `click_entity` on a sidebar item would always miss.
    fn element_center(&self, entity_id: &str) -> Option<(f32, f32)> {
        debug_assert!(
            entity_id.starts_with("block:") || !entity_id.contains(':'),
            "GpuiUserDriver only supports block-scoped entity ids; got {entity_id:?}"
        );
        for el_id in [
            format!("render-entity-{entity_id}"),
            format!("selectable-{entity_id}"),
        ] {
            if let Some(info) = self.geometry.element_info(&el_id) {
                return Some(info.center());
            }
        }
        self.geometry
            .find_by_entity_id(entity_id)
            .map(|info| info.center())
    }

    /// Fail-loud bounds lookup used by the user-verb methods. Returns an
    /// error with enough context for test authors to understand whether
    /// the element was never rendered or simply hasn't been promoted from
    /// the staged buffer yet.
    fn require_element_center(&self, entity_id: &str, verb: &str) -> Result<(f32, f32)> {
        self.element_center(entity_id).with_context(|| {
            format!(
                "GpuiUserDriver::{verb}: no bounds recorded for entity {entity_id:?} — \
                 element not rendered, or BoundsRegistry hasn't promoted staged → committed \
                 since it was added. Tests should call \
                 `holon_integration_tests::polling::wait_for_element_bounds` before \
                 driving input on a freshly-rendered element."
            )
        })
    }
}

#[async_trait]
impl UserDriver for GpuiUserDriver {
    async fn synthetic_dispatch(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()> {
        // Inline-dispatch into the reactive engine. No channel dispatch
        // because this is the synthetic path; when callers want the real
        // click pipeline they go through `click_entity` / `send_key_chord`.
        let intent = OperationIntent::new(entity.into(), op.into(), params);
        self.engine.dispatch_intent_sync(intent).await
    }

    /// Focus the target via a real mouse click dispatched on the
    /// interaction channel. Fails loud when geometry isn't available —
    /// see module-level doc for the rationale and the
    /// `wait_for_element_bounds` remedy. The `region` arg matches the
    /// trait signature; the GPUI driver synthesizes a real mouse event,
    /// so the region is implicit in the click coordinates and the arg is
    /// unused here.
    #[tracing::instrument(skip(self), name = "GpuiUserDriver.click_entity", fields(%entity_id))]
    async fn click_entity(&self, entity_id: &str, _region: &str) -> Result<()> {
        let (cx, cy) = self.require_element_center(entity_id, "click_entity")?;

        self.dispatch_event(InteractionEvent::MouseClick {
            position: (cx, cy),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(())
    }

    /// Focus the target via click, then dispatch each character of
    /// `text` as a keystroke through the interaction channel. Mirrors
    /// MCP's `type_text` tool so both paths exercise the same pipeline.
    /// Fails loud when bounds aren't available.
    async fn type_text(&self, entity_id: &str, text: &str) -> Result<()> {
        let (cx, cy) = self.require_element_center(entity_id, "type_text")?;

        self.dispatch_event(InteractionEvent::MouseClick {
            position: (cx, cy),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;

        for ch in text.chars() {
            self.dispatch_event(InteractionEvent::KeyDown {
                keystroke: ch.to_string(),
                modifiers: Vec::new(),
            })
            .await?;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(())
    }

    /// Focus the entity via click, then press the chord keys through
    /// the interaction channel. Modifier keys are pressed, the regular
    /// keys are clicked, and modifiers are released in reverse.
    ///
    /// `extra_params["position"]` (when present) is treated as a desired
    /// cursor byte offset that a real user would have set up before
    /// pressing the chord (today: `split_block`). We click to focus, then
    /// emit a `home` keystroke and `position` `right` keystrokes through
    /// the real `PlatformInput` pipeline so the focused `InputState`'s
    /// cursor lands at the requested offset. The production chord handler
    /// (`EditorView`'s capture-phase `Enter`) then reads that cursor byte
    /// itself — there is no server-side injection.
    ///
    /// Caveat: `right` advances by a grapheme boundary in `InputState`,
    /// not by raw bytes. ASCII content means byte == grapheme; non-ASCII
    /// content can land the cursor a few bytes off. Tests that need
    /// byte-exact placement on multi-byte content must either generate
    /// ASCII or read back the actual cursor byte after positioning.
    ///
    /// Bounds-missing is a hard error. See module-level doc.
    async fn send_key_chord(
        &self,
        _root_block_id: &str,
        _root_tree: &ReactiveViewModel,
        entity_id: &str,
        chord: &KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool> {
        let (cx, cy) = self.require_element_center(entity_id, "send_key_chord")?;
        self.dispatch_event(InteractionEvent::MouseClick {
            position: (cx, cy),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Position the cursor with real input when the caller specified a
        // byte offset. Click lands the cursor somewhere in the line; we
        // re-anchor with `home` and then advance with `right`. This goes
        // through `PlatformInput` and `InputState::move_to_*`, so any
        // bug in cursor handling surfaces here just like in production.
        if let Some(Value::Integer(target)) = extra_params.get("position") {
            let target = (*target).max(0) as usize;
            self.dispatch_event(InteractionEvent::KeyDown {
                keystroke: "home".into(),
                modifiers: Vec::new(),
            })
            .await?;
            for _ in 0..target {
                self.dispatch_event(InteractionEvent::KeyDown {
                    keystroke: "right".into(),
                    modifiers: Vec::new(),
                })
                .await?;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let (modifiers, regulars): (Vec<_>, Vec<_>) =
            chord.0.iter().cloned().partition(is_modifier);

        let mod_names: Vec<String> = modifiers
            .iter()
            .filter_map(|k| modifier_name(k).map(str::to_string))
            .collect();

        for key in &regulars {
            let Some(name) = keystroke_name(key) else {
                continue;
            };
            self.dispatch_event(InteractionEvent::KeyDown {
                keystroke: name,
                modifiers: mod_names.clone(),
            })
            .await?;
        }

        tokio::time::sleep(Duration::from_millis(30)).await;
        Ok(true)
    }

    /// Turn the scroll wheel at a window coordinate via the interaction
    /// channel. `dx` / `dy` are line-based deltas (positive `dy` = down).
    async fn scroll_at(&self, x: f32, y: f32, dx: f32, dy: f32) -> Result<()> {
        self.dispatch_event(InteractionEvent::ScrollWheel {
            position: (x, y),
            delta: (dx, dy),
            modifiers: Vec::new(),
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(())
    }

    /// Scroll over an entity — looks up its window-space center via the
    /// `GeometryProvider` and delegates to `scroll_at`. Fails loud when
    /// bounds aren't available; MCP clients now receive an error
    /// instead of a silent no-op (observable behavior change).
    async fn scroll_entity(&self, entity_id: &str, dx: f32, dy: f32) -> Result<()> {
        let (cx, cy) = self.require_element_center(entity_id, "scroll_entity")?;
        self.scroll_at(cx, cy, dx, dy).await
    }

    /// Drag the source block onto the target via real pointer events:
    /// `MouseDown(source)` → several `MouseMove(…, pressed=Left)` past
    /// GPUI's drag threshold → `MouseUp(target)`. The window's input
    /// pump turns each into a `PlatformInput`; GPUI populates
    /// `cx.active_drag` from the draggable's `on_drag` closure on the
    /// first qualifying move, and the drop_zone's `on_drop` closure
    /// fires on `MouseUp` over the target. Fails loud when either
    /// element's bounds aren't available — see module-level doc.
    async fn drop_entity(
        &self,
        _root_block_id: &str,
        source_id: &str,
        target_id: &str,
    ) -> Result<bool> {
        let (sx, sy) = self.require_element_center(source_id, "drop_entity (source)")?;
        let (tx, ty) = self.require_element_center(target_id, "drop_entity (target)")?;

        self.dispatch_event(InteractionEvent::MouseDown {
            position: (sx, sy),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Step the cursor toward the target in increments. Each step must
        // exceed GPUI's drag threshold (~5 logical px) for the drag state
        // to engage, so we use 5 small steps with `pressed_button=Left`.
        let steps = 5;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let mx = sx + (tx - sx) * t;
            let my = sy + (ty - sy) * t;
            self.dispatch_event(InteractionEvent::MouseMove {
                position: (mx, my),
                pressed_button: Some("left".into()),
                modifiers: Vec::new(),
            })
            .await?;
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        self.dispatch_event(InteractionEvent::MouseUp {
            position: (tx, ty),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(30)).await;
        Ok(true)
    }

    async fn send_raw_keystroke(&self, keystroke: &str, modifiers: &[&str]) -> Result<()> {
        self.dispatch_event(InteractionEvent::KeyDown {
            keystroke: keystroke.to_string(),
            modifiers: modifiers.iter().map(|s| s.to_string()).collect(),
        })
        .await?;
        // Brief settle so the editor's `capture_action` chain completes
        // before the next keystroke (matches the existing 20-30ms cadence
        // used elsewhere in this driver).
        tokio::time::sleep(Duration::from_millis(15)).await;
        Ok(())
    }
}

fn is_modifier(k: &holon_api::Key) -> bool {
    use holon_api::Key;
    matches!(k, Key::Cmd | Key::Ctrl | Key::Alt | Key::Shift)
}

fn modifier_name(k: &holon_api::Key) -> Option<&'static str> {
    use holon_api::Key;
    Some(match k {
        Key::Cmd => "cmd",
        Key::Ctrl => "ctrl",
        Key::Alt => "alt",
        Key::Shift => "shift",
        _ => return None,
    })
}

fn keystroke_name(k: &holon_api::Key) -> Option<String> {
    use holon_api::Key;
    Some(match k {
        Key::Up => "up".into(),
        Key::Down => "down".into(),
        Key::Left => "left".into(),
        Key::Right => "right".into(),
        Key::Home => "home".into(),
        Key::End => "end".into(),
        Key::PageUp => "pageup".into(),
        Key::PageDown => "pagedown".into(),
        Key::Tab => "tab".into(),
        Key::Enter => "enter".into(),
        Key::Backspace => "backspace".into(),
        Key::Delete => "delete".into(),
        Key::Escape => "escape".into(),
        Key::Space => "space".into(),
        Key::Char(c) => c.to_string(),
        Key::F(n) => format!("f{n}"),
        Key::Cmd | Key::Ctrl | Key::Alt | Key::Shift => return None,
    })
}
