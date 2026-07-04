//! `inv-window-focus-matches-engine-focus`.
//!
//! SUT-internal coherence between the two focus authorities (ADR 0010):
//! the engine's in-memory `focused_block` (moves synchronously on
//! click/split/join op responses) and WINDOW focus (follows via a spawned
//! binding, recorded per committed frame as `RenderedElement::focused` by the
//! `editable_text` builder). After settle the two must agree — a divergence
//! is exactly the focus steal-back / zombie-editor family: a keystroke sent
//! "to the focused block" gets consumed by a stale editor.
//!
//! Two directions, both polled (window focus is allowed to LAG one or two
//! frames, never to settle elsewhere):
//! - A window-focused editor whose entity differs from `focused_block`
//!   (or with `focused_block == None`) → Fail (steal-back / zombie editor).
//! - `focused_block == Some(id)` while id's `editable_text` is mounted in
//!   the frame but does NOT hold window focus → Fail (lost handoff). When no
//!   `editable_text` for id is in the frame at all the check skips — engine
//!   focus on a row without an editor (e.g. sidebar navigation focus) is
//!   legitimate.
//!
//! Skipped when no geometry is installed (headless) or no frontend engine
//! exists (SqlOnly). Needs no reference-model capability: it compares the SUT
//! against itself.
//!
//! Status: functional.

use holon_pbt_core::capabilities::{EngineFocus, EntityUri, SutDriver, SutLayout};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

use holon_pbt_core::retry::retry_until_ok;

pub struct InvWindowFocusMatchesEngineFocus;

impl InvWindowFocusMatchesEngineFocus {
    pub const ID: InvariantId = InvariantId("inv-window-focus-matches-engine-focus");
    const LABEL: &'static str = "inv-window-focus-matches-engine-focus";
}

/// One settled observation of both focus authorities, or a reason to skip.
enum Observation {
    Skip(String),
    /// `(engine_focus, window_focused_entities, engine_block_editor_mounted)`
    Mismatch {
        engine: Option<EntityUri>,
        window: Vec<EntityUri>,
        editor_mounted: bool,
    },
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvWindowFocusMatchesEngineFocus
where
    S: SutDriver + SutLayout,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        // Window focus follows `focused_block` via a spawned binding — up to a
        // couple of frames behind. Poll until the two authorities agree; only a
        // *settled* divergence fails.
        let result = retry_until_ok(
            std::time::Duration::from_millis(1000),
            std::time::Duration::from_millis(20),
            async || {
                // MUST be the fresh (uncached, frame-pumping) read: the
                // per-tick CachingProxy memoises `rendered_elements`, so the
                // cached method would return the same frozen snapshot on
                // every retry — turning the legitimate 1-2 frame window-focus
                // lag into a guaranteed "settled" failure (2026-06-11
                // NavigateHome zombie-editor false positive).
                let elements = sut.rendered_elements_fresh().await;
                if elements.is_empty() {
                    return Ok(Observation::Skip(
                        "no geometry installed / nothing rendered yet".into(),
                    ));
                }
                let window: Vec<EntityUri> = elements
                    .iter()
                    .filter(|el| el.focused == Some(true))
                    .filter_map(|el| el.entity_id.clone())
                    .collect();
                match sut.engine_focused_block().await {
                    EngineFocus::NoEngine => {
                        Ok(Observation::Skip("no frontend engine (SqlOnly)".into()))
                    }
                    EngineFocus::Unfocused => {
                        if window.is_empty() {
                            Ok(Observation::Skip("both authorities unfocused".into()))
                        } else {
                            // A window-focused editor with no engine focus is a
                            // zombie editor — keep polling (the unmount may be
                            // in flight), fail if it settles.
                            Err(Observation::Mismatch {
                                engine: None,
                                window,
                                editor_mounted: false,
                            })
                        }
                    }
                    EngineFocus::Focused(engine_id) => {
                        if window.len() == 1 && window[0] == engine_id {
                            return Ok(Observation::Skip(String::new()));
                        }
                        let editor_mounted = elements.iter().any(|el| {
                            el.widget_type == "editable_text"
                                && el.entity_id.as_ref() == Some(&engine_id)
                        });
                        if window.is_empty() && !editor_mounted {
                            // Engine focus on an entity with no mounted editor
                            // (sidebar/nav focus) — window focus legitimately
                            // lives elsewhere (or nowhere we record).
                            return Ok(Observation::Skip(format!(
                                "engine focus {engine_id} has no mounted editable_text"
                            )));
                        }
                        Err(Observation::Mismatch {
                            engine: Some(engine_id),
                            window,
                            editor_mounted,
                        })
                    }
                }
            },
        )
        .await;

        match result {
            Ok(Observation::Skip(reason)) if reason.is_empty() => InvariantResult::Ok,
            Ok(Observation::Skip(reason)) => {
                InvariantResult::Skipped(format!("[{}] {reason}", Self::LABEL))
            }
            Ok(Observation::Mismatch { .. }) => {
                unreachable!("Ok never carries Mismatch")
            }
            Err(Observation::Skip(_)) => unreachable!("Err never carries Skip"),
            Err(Observation::Mismatch {
                engine,
                window,
                editor_mounted,
            }) => InvariantResult::Fail(format!(
                "[{label}] window focus diverged from engine focus (settled, polled 1s): \
                 engine.focused_block() = {engine:?}, window-focused editor(s) in the \
                 committed frame = {window:?}, engine block's editable_text mounted: \
                 {editor_mounted}. A stale window-focused editor consumes keystrokes \
                 meant for the engine-focused block (steal-back / zombie-editor, \
                 ADR 0010).",
                label = Self::LABEL,
            )),
        }
    }
}
