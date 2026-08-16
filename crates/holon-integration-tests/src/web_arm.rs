//! Web arm: replay hand-authored keystone cases against the dioxus-web
//! frontend, and cross-check the two oracle channels D4.a rules for.
//!
//! The corpus, its schema and its loader are the keystone's own
//! ([`crate::pbt::hand_authored`]) — this module only supplies the two things
//! the browser medium needs: how a transition becomes a gesture, and what
//! "correct" means when the SUT lives behind a DOM.
//!
//! # Declared caps (fix-cap-not-withhold)
//!
//! A transition is replayable here only if a real user could produce it with a
//! pointer and a keyboard. That rules out, and [`unsupported_reason`] names,
//! two classes:
//!
//! * **Op-floor transitions with a pinned id** — `CreateBlockUnderFocus`
//!   carries the id the oracle and SUT must share. No gesture can choose an id,
//!   so replaying it in a browser needs the synthetic→real reconcile the
//!   composed harness runs. Tracked, not hidden.
//! * **Non-UI transitions** — external writes, peers, restarts, org-file edits.
//!   They act on layers the browser build does not have (there is no org parser
//!   in the wasm worker).
//!
//! # Caret discipline
//!
//! `WebUserDriver::send_key_chord` focus-clicks its target and a click lands at
//! the element's centre, so it MOVES THE CARET (see that module's hazard note).
//! Every transition here that depends on a caret therefore seats the caret
//! explicitly with `home` + N × `right` rather than trusting where a click left
//! it. Byte position equals key presses only for ASCII, which
//! [`seat_caret`] asserts rather than assumes.

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use holon_api::EntityUri;
use holon_frontend::user_driver::UserDriver;

use crate::pbt::hand_authored::HandAuthoredCase;
use crate::pbt::transitions::E2ETransition;
use crate::web_relay_oracle::EngineSnapshot;
use crate::web_user_driver::RenderedNode;
use crate::web_user_driver::WebUserDriver;

/// Why this transition cannot be driven through the DOM, or `None` when it
/// can. Kept as one exhaustive-by-default match so a newly added transition
/// shows up as unsupported-with-a-reason rather than silently skipped.
pub fn unsupported_reason(transition: &E2ETransition) -> Option<String> {
    match transition {
        E2ETransition::FocusEditableText(_)
        | E2ETransition::TypeChars(_)
        | E2ETransition::MoveCursor(_)
        | E2ETransition::SplitBlock(_)
        | E2ETransition::DeleteBackward(_)
        | E2ETransition::PressKey(_)
        | E2ETransition::Outdent(_)
        | E2ETransition::Indent(_) => None,
        E2ETransition::CreateBlockUnderFocus(_) => Some(
            "CreateBlockUnderFocus pins the created block's id so oracle and SUT share it; no \
             pointer/keyboard gesture can choose an id, so the web arm needs the composed \
             harness's synthetic→real reconcile first"
                .to_string(),
        ),
        other => Some(format!(
            "{} has no pointer/keyboard realization in the browser build",
            other.variant_name()
        )),
    }
}

/// The reasons a whole case is out of the web arm's reach, empty when the case
/// is replayable.
pub fn case_caps(case: &HandAuthoredCase) -> Vec<String> {
    let mut reasons: Vec<String> = case
        .transitions
        .iter()
        .filter_map(unsupported_reason)
        .collect();
    reasons.dedup();
    reasons
}

/// Every block id a case names. A gesture can only reach a block the renderer
/// has mounted, so these are the ids that must be addressable before the case
/// can run at all.
pub fn addressed_blocks(case: &HandAuthoredCase) -> Vec<EntityUri> {
    let mut ids: Vec<EntityUri> = case
        .transitions
        .iter()
        .filter_map(|t| match t {
            E2ETransition::FocusEditableText(t) => Some(t.block_id.clone()),
            E2ETransition::SplitBlock(t) => Some(t.block_id.clone()),
            E2ETransition::Indent(t) => Some(t.block_id.clone()),
            E2ETransition::Outdent(t) => Some(t.block_id.clone()),
            _ => None,
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Cap reason when a case names a block the browser does not render, `None`
/// when every named block is reachable by pointer.
///
/// This is a HARD cap today, not a missing gesture: the corpus is authored over
/// the wide seed (`block:parent`/`c1`/`c2`), and the only way to install that
/// seed in the browser is the `reset_vault` tool — which rebuilds the worker's
/// engine but leaves the live page bound to the torn-down one, so the seeded
/// blocks never reach the DOM. See this module's escalation note.
pub fn unaddressable(driver: &WebUserDriver, case: &HandAuthoredCase) -> Option<String> {
    let rendered: std::collections::BTreeSet<String> =
        driver.snapshot().into_iter().map(|n| n.id).collect();
    let missing: Vec<String> = addressed_blocks(case)
        .into_iter()
        .filter(|id| !rendered.contains(id.as_str()))
        .map(|id| id.to_string())
        .collect();
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "names block(s) the browser does not render: {missing:?} (rendered: {rendered:?}). The \
         corpus is authored over the wide seed; `reset_vault` installs it in the worker but the \
         page keeps rendering the torn-down engine, so those blocks are unreachable by gesture."
    ))
}

/// Seat the caret at `byte_position` of `block_id`'s editor with real
/// keystrokes: focus, `home`, then one `right` per character.
///
/// `home` is what makes this deterministic — a focus click lands at the
/// element's centre, so the caret's starting point is a function of pixel
/// geometry, not of the model.
async fn seat_caret(
    driver: &WebUserDriver,
    block_id: &EntityUri,
    byte_position: usize,
) -> Result<()> {
    driver.click_entity(block_id, "main").await?;
    let text = driver
        .displayed_text(block_id)
        .with_context(|| format!("no displayed text for {block_id}, so no caret to seat"))?;
    if !text.is_ascii() {
        bail!(
            "caret seating counts `right` presses, which equals the byte position only for \
             ASCII; {block_id} holds {text:?}. Add a code-point-aware seat before replaying \
             cases with non-ASCII content."
        );
    }
    driver.send_raw_keystroke("home", &[]).await?;
    for _ in 0..byte_position {
        driver.send_raw_keystroke("right", &[]).await?;
    }
    Ok(())
}

/// Drive one keystone transition through the browser. Fails loud on anything
/// [`unsupported_reason`] rejects — a skipped transition would make the replay
/// a different case than the one it names.
pub async fn apply(driver: &WebUserDriver, transition: &E2ETransition) -> Result<()> {
    if let Some(reason) = unsupported_reason(transition) {
        bail!("web arm cannot drive {transition:?}: {reason}");
    }
    match transition {
        E2ETransition::FocusEditableText(t) => driver.click_entity(&t.block_id, "main").await,
        E2ETransition::TypeChars(t) => {
            // One keystroke at a time, as prod delivers them: each runs the
            // whole edit→commit sink, and a single bulk insert would judge a
            // state the app never held (see `type_chars_apply_to_ref`).
            for ch in t.text.chars() {
                driver.send_raw_keystroke(&ch.to_string(), &[]).await?;
            }
            Ok(())
        }
        E2ETransition::MoveCursor(t) => {
            let focused = driver
                .oracle()
                .context("MoveCursor needs the relay oracle to know which editor is open")?
                .engine_snapshot()
                .await?
                .focused_block
                .context("MoveCursor with no focused block — the case's precondition is unmet")?;
            let block = EntityUri::parse(&focused)
                .with_context(|| format!("engine reported a non-uri focus {focused:?}"))?;
            seat_caret(driver, &block, t.byte_position).await
        }
        E2ETransition::SplitBlock(t) => {
            seat_caret(driver, &t.block_id, t.position).await?;
            driver.send_raw_keystroke("enter", &[]).await
        }
        E2ETransition::DeleteBackward(t) => {
            for _ in 0..t.count {
                driver.send_raw_keystroke("backspace", &[]).await?;
            }
            Ok(())
        }
        E2ETransition::PressKey(t) => {
            let keys: Vec<String> = t.chord.0.iter().map(ToString::to_string).collect();
            let (modifiers, plain): (Vec<&String>, Vec<&String>) = keys
                .iter()
                .partition(|k| matches!(k.as_str(), "cmd" | "ctrl" | "alt" | "shift"));
            let [key] = plain.as_slice() else {
                bail!("PressKey needs exactly one non-modifier key, got {keys:?}");
            };
            let modifiers: Vec<&str> = modifiers.iter().map(|m| m.as_str()).collect();
            driver.send_raw_keystroke(key, &modifiers).await
        }
        E2ETransition::Indent(t) => {
            driver.click_entity(&t.block_id, "main").await?;
            driver.send_raw_keystroke("tab", &[]).await
        }
        E2ETransition::Outdent(t) => {
            driver.click_entity(&t.block_id, "main").await?;
            driver.send_raw_keystroke("tab", &["shift"]).await
        }
        other => bail!("web arm has no gesture for {other:?} (unsupported_reason disagrees)"),
    }
}

/// What the two channels saw at one point in a run.
#[derive(Debug)]
pub struct DualOracleReading {
    pub engine: EngineSnapshot,
    pub rendered: Vec<RenderedNode>,
    /// `block_raw` row count — a third point, one layer below the block-query
    /// seam `debug_pbt_snapshot` reads.
    pub raw_block_rows: usize,
}

/// Read both channels and assert they agree. This is the differential the arm
/// exists for: a DOM assertion alone cannot tell "the engine has no such block"
/// from "the renderer dropped it", and an engine assertion alone cannot see a
/// renderer that shows a block the engine retired.
///
/// The known blind spot is that both channels can agree on the same wrong
/// answer — they share the worker. `raw_block_rows` is the cheap third point
/// against that: it reads SQL directly rather than the block-query projection.
pub async fn read_and_cross_check(
    driver: &WebUserDriver,
    phase: &str,
) -> Result<DualOracleReading> {
    let oracle = driver
        .oracle()
        .context("dual-oracle cross-check needs a driver launched with the relay oracle")?;
    let engine = oracle.engine_snapshot().await?;
    let rendered = driver.refresh_snapshot().await?;
    let rows = oracle
        .raw_sql("select count(*) as n from block_raw")
        .await?;
    let raw_block_rows = rows
        .first()
        .and_then(|r| r.get("n"))
        .and_then(serde_json::Value::as_u64)
        .with_context(|| format!("block_raw count query returned no `n` column: {rows:?}"))?
        as usize;

    // 1. Every block the DOM shows must exist in the engine. The converse does NOT
    //    hold — the main panel renders one page, not the whole store.
    let engine_ids: std::collections::BTreeSet<&str> =
        engine.block_ids.iter().map(String::as_str).collect();
    let phantom: Vec<&RenderedNode> = rendered
        .iter()
        .filter(|n| n.role == "rendered-text" || n.role == "editor-cell")
        .filter(|n| !engine_ids.contains(n.id.as_str()))
        .collect();
    if !phantom.is_empty() {
        bail!(
            "[{phase}] DOM renders block(s) the engine does not hold: {:?}\n  engine holds: \
             {:?}",
            phantom.iter().map(|n| (&n.id, &n.text)).collect::<Vec<_>>(),
            engine.block_ids
        );
    }

    // 2. The block query must not report more blocks than its own storage holds. A
    //    COUNT comparison, deliberately, not a set subset: `block_raw` also holds
    //    rows the block query filters out, so only this direction is sound without
    //    re-implementing that filter here.
    if raw_block_rows < engine.block_ids.len() {
        bail!(
            "[{phase}] block_raw holds {raw_block_rows} rows but the block query reports {} live \
             blocks — the projection is ahead of its own storage",
            engine.block_ids.len()
        );
    }

    // 3. Committed text must match. The editor cell is excluded: it holds
    //    pre-commit keystrokes by design, so a mismatch there is expected.
    for node in rendered.iter().filter(|n| n.role == "rendered-text") {
        let Some(content) = engine.block_content.get(&node.id) else {
            continue;
        };
        if !node.text.contains(content.trim()) && !content.trim().is_empty() {
            bail!(
                "[{phase}] rendered text for {} disagrees with engine content\n  DOM:    {:?}\n  \
                 engine: {:?}",
                node.id,
                node.text,
                content
            );
        }
    }

    Ok(DualOracleReading {
        engine,
        rendered,
        raw_block_rows,
    })
}
