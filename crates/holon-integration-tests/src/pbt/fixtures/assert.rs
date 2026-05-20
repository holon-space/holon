//! `Then` assertions, evaluated against the live SUT.
//!
//! An [`Assertion`] is the assert-side counterpart of an `E2ETransition`. It
//! is evaluated through [`evaluate_assertion`], a free function bounded on the
//! capability traits it needs (`SutRenderer`, `SutDriver`, `RefFocus`). The
//! generic runner can't reach these — they live on the inner `E2ESut` — so a
//! macro-generated `FixtureAssertable` bridge calls this with `&sut.inner` and
//! the shared runtime (see `super::FixtureAssertable`).
//!
//! Vocabulary v1 (see `matchers::match_assertion`):
//! - `the widget contains "<text>"` / `the widget shows exactly "<text>"`
//! - `block "<id>" contains "<text>"`
//! - `focus is on block "<id>"` / `block "<id>" is focused`
//!
//! Any of these may be prefixed with `within <N> seconds ` to retry until the
//! assertion holds or the timeout elapses — the escape hatch for CDC-lag
//! windows where a read can briefly trail the settled state.
//!
//! Block ids in assertions are reference-model ids: they are resolved through
//! `resolve_ref_block_id` (so `block:ref-doc-0`, a `block::split-N` synthetic,
//! and a stable `:ID:` all work). `WidgetContains`/`FocusOn` need a renderer /
//! focus source; absent both, they fail loud rather than passing vacuously.

use std::time::Duration;

use holon_pbt_core::capabilities::{
    CapRegion, EngineFocus, EntityUri, RefFocus, SutDriver, SutRenderer, WidgetSnapshot,
};

#[derive(Debug, Clone)]
pub enum Assertion {
    /// The rendered widget tree contains `text`. `locator = None` matches the
    /// root widget; `Some(block_id)` scopes to that block's subtree.
    WidgetContains {
        locator: Option<String>,
        text: String,
        exact: bool,
        within_secs: Option<u64>,
    },
    /// The SUT's focused block resolves to `block_id` (a reference-model id;
    /// remapped through `resolve_ref_block_id`).
    FocusOn {
        block_id: String,
        within_secs: Option<u64>,
    },
}

impl Assertion {
    fn within_secs(&self) -> Option<u64> {
        match self {
            Assertion::WidgetContains { within_secs, .. } => *within_secs,
            Assertion::FocusOn { within_secs, .. } => *within_secs,
        }
    }
}

/// Evaluate an assertion against the reference state and live SUT. Returns
/// `Err(message)` on mismatch (the runner panics on `Err`). When the assertion
/// carries a `within N seconds` budget, the check is retried until it holds or
/// the budget elapses, returning the last failure message on timeout.
pub async fn evaluate_assertion<R, S>(
    assertion: &Assertion,
    ref_: &R,
    sut: &S,
) -> Result<(), String>
where
    R: RefFocus,
    S: SutRenderer + SutDriver,
{
    let deadline = assertion
        .within_secs()
        .map(|secs| tokio::time::Instant::now() + Duration::from_secs(secs));

    loop {
        let result = match assertion {
            Assertion::WidgetContains {
                locator,
                text,
                exact,
                ..
            } => widget_contains(sut, locator.as_deref(), text, *exact).await,
            Assertion::FocusOn { block_id, .. } => focus_on(ref_, sut, block_id).await,
        };
        match result {
            Ok(()) => return Ok(()),
            Err(msg) => match deadline {
                Some(d) if tokio::time::Instant::now() < d => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                _ => return Err(msg),
            },
        }
    }
}

async fn widget_contains<S: SutRenderer + SutDriver>(
    sut: &S,
    locator: Option<&str>,
    text: &str,
    exact: bool,
) -> Result<(), String> {
    let (scope, haystack) = match locator {
        None => ("root widget".to_string(), {
            let snap = sut.widget_tree_snapshot().await;
            snapshot_text(&snap)
        }),
        Some(id) => {
            let id_uri = EntityUri::parse(id).map_err(|e| {
                format!("[widget-contains] locator {id:?} is not a valid EntityUri: {e}")
            })?;
            let resolved = sut.resolve_ref_block_id(&id_uri);
            let snap = sut.widget_tree_for(&resolved).await.ok_or_else(|| {
                format!("[widget-contains] block {id:?} (resolved {resolved:?}) did not render (no widget tree)")
            })?;
            (format!("block {id:?}"), snapshot_text(&snap))
        }
    };

    let matched = if exact {
        haystack.trim() == text.trim()
    } else {
        haystack.contains(text)
    };
    if matched {
        return Ok(());
    }
    let qualifier = if exact { "exactly " } else { "" };
    Err(format!(
        "[widget-contains] expected {scope} to contain {qualifier}{text:?}, but rendered text was:\n{haystack}"
    ))
}

fn snapshot_text(snap: &WidgetSnapshot) -> String {
    let mut out = String::new();
    for node in snap.walk() {
        if let Some(entity_id) = &node.entity_id {
            out.push_str(entity_id);
            out.push('\n');
        }
        for value in node.props.values() {
            out.push_str(value);
            out.push('\n');
        }
    }
    out
}

async fn focus_on<R: RefFocus, S: SutDriver>(
    ref_: &R,
    sut: &S,
    block_id: &str,
) -> Result<(), String> {
    let block_id_uri = EntityUri::parse(block_id)
        .map_err(|e| format!("[focus-on] block id {block_id:?} is not a valid EntityUri: {e}"))?;
    let expected = sut.resolve_ref_block_id(&block_id_uri);

    // Prefer the reactive/frontend engine's focus; fall back to the SQL
    // `current_focus` matview only when no engine is installed (SqlOnly
    // mode). An installed-but-unfocused engine is a real lost-focus state
    // and must NOT be papered over by the SQL fallback.
    let sut_focus = match sut.engine_focused_block().await {
        EngineFocus::Focused(focus) => focus,
        EngineFocus::Unfocused => {
            return Err(format!(
                "[focus-on] engine has no focused block (lost focus). Expected focus on \
                 {block_id:?} (SQL current_focus = {:?})",
                sut.driver_current_focus().await
            ));
        }
        EngineFocus::NoEngine => sut.driver_current_focus().await.ok_or_else(|| {
            format!(
                "[focus-on] no SUT focus available (no frontend engine and SQL current_focus \
                 empty). Expected focus on {block_id:?}"
            )
        })?,
    };

    if sut_focus == expected || sut_focus.as_str() == block_id {
        return Ok(());
    }
    let ref_focus = ref_.current_focus(CapRegion::Main);
    Err(format!(
        "[focus-on] expected focus on {block_id:?} (resolved {expected:?}), but SUT focus = \
         {sut_focus:?} (reference model focus = {ref_focus:?})"
    ))
}
