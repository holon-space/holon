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
//! entity ids (it prepends `render-block-`). Non-block user-verb targets
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
    fn element_center(&self, entity_id: &str) -> Option<(f32, f32)> {
        debug_assert!(
            entity_id.starts_with("block:") || !entity_id.contains(':'),
            "GpuiUserDriver only supports block-scoped entity ids; got {entity_id:?}"
        );
        let el_id = format!("render-block-{entity_id}");
        let info = self.geometry.element_info(&el_id)?;
        Some(info.center())
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
    /// `wait_for_element_bounds` remedy.
    async fn click_entity(&self, entity_id: &str) -> Result<()> {
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
    /// `extra_params` is the canonical channel for UI-observable context
    /// that the chord resolver can't read (today: `split_block` cursor
    /// byte). Drivers that synthesize real input cannot thread this
    /// through the OS/window pipeline, so when `extra_params` is
    /// non-empty we fall through to the shadow-index path that injects
    /// `extra_params` into the matched operation's params. This is NOT
    /// a fallback — it is the intended path for that feature.
    ///
    /// Bounds-missing (with empty `extra_params`) is a hard error. See
    /// module-level doc.
    async fn send_key_chord(
        &self,
        root_block_id: &str,
        root_tree: &ReactiveViewModel,
        entity_id: &str,
        chord: &KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool> {
        if !extra_params.is_empty() {
            return trait_default_send_key_chord(
                self,
                root_block_id,
                root_tree,
                entity_id,
                chord,
                extra_params,
            )
            .await;
        }

        let (cx, cy) = self.require_element_center(entity_id, "send_key_chord")?;
        self.dispatch_event(InteractionEvent::MouseClick {
            position: (cx, cy),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;

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
}

/// Shadow-index dispatch path, used by `send_key_chord` when
/// `extra_params` is non-empty (canonical `split_block` cursor-byte
/// channel). `async_trait` doesn't expose the generated default body,
/// so we replicate it here.
async fn trait_default_send_key_chord(
    driver: &GpuiUserDriver,
    root_block_id: &str,
    root_tree: &ReactiveViewModel,
    entity_id: &str,
    chord: &KeyChord,
    extra_params: HashMap<String, Value>,
) -> Result<bool> {
    use holon_frontend::input::{InputAction, WidgetInput};
    use holon_frontend::shadow_index::IncrementalShadowIndex;

    let shadow_index = IncrementalShadowIndex::build(root_block_id, root_tree);
    let input = WidgetInput::KeyChord {
        keys: chord.0.clone(),
    };
    match shadow_index.bubble_input(entity_id, &input) {
        Some(InputAction::ExecuteOperation {
            entity_name,
            operation,
            entity_id,
        }) => {
            let mut params = HashMap::new();
            params.insert("id".into(), Value::String(entity_id));
            params.extend(extra_params);
            let intent = OperationIntent::new(entity_name, operation.name, params);
            driver.apply_intent(intent).await?;
            Ok(true)
        }
        _ => Ok(false),
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
