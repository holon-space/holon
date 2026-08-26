//! Map Gherkin `Then` steps to PBT assertions.
//!
//! Action steps (`Given`/`When`) are NOT here: they are read by the generated
//! step vocabulary (`E2ETransition::parse_step`), the one action vocabulary.
//! This module is what remains until the Then-steps join it.
//!
//! Unmatched steps are a HARD ERROR (fail loud) — a `.feature` referencing
//! a step we don't understand must not silently pass.

use gherkin::Step;
use holon_pbt_core::step_vocabulary::StepVocabulary;
use regex::Regex;

use super::assert::Assertion;
use super::assert_steps::BlockComesAfter;
use super::assert_steps::BlockHasDanglingLink;
use super::assert_steps::BlockHasNoTaskState;
use super::assert_steps::BlockHasTaskState;
use super::assert_steps::BlockIsChildOf;
use super::assert_steps::BlockIsCollapsed;
use super::assert_steps::BlockIsNotCollapsed;
use super::assert_steps::BlockIsNthChild;
use super::assert_steps::BlockIsTopLevelOf;
use super::assert_steps::BlockResolvesLink;
use super::assert_steps::SlashMenuOffers;
use super::assert_steps::SlashMenuOmits;

/// Refuse a `within N seconds` prefix on an assertion that has no retry
/// budget to give it. An ABSENCE is true the instant it is read, so a budget
/// could only mask a still-arriving render — and honouring the prefix by
/// dropping it would silently discard an instruction the author wrote on
/// purpose.
fn refuse_within(raw: &str, within_secs: Option<u64>) -> Result<(), String> {
    match within_secs {
        None => Ok(()),
        Some(secs) => Err(format!(
            "step {raw:?} asks for a `within {secs} seconds` budget on an assertion that takes \
             none: an absence is true the instant it is read, so retrying could only hide a \
             render that had not arrived yet. Drop the prefix, and order the step AFTER a \
             positive assertion that proves the surface has settled."
        )),
    }
}

/// Translate a single Gherkin `Then` step into an assertion. An optional
/// `within <N> seconds ` prefix sets a retry budget on the assertion.
pub fn match_assertion(step: &Step) -> Result<Assertion, String> {
    let raw = step.value.trim();

    // Optional `within <N> seconds ` retry-budget prefix.
    let within_re = Regex::new(r"(?i)^within\s+(?P<secs>\d+)\s+seconds?\s+(?P<rest>.+)$").unwrap();
    let (within_secs, text) = match within_re.captures(raw) {
        Some(caps) => {
            let secs = caps["secs"]
                .parse::<u64>()
                .map_err(|e| format!("invalid `within N seconds` count in {raw:?}: {e}"))?;
            (Some(secs), caps.name("rest").unwrap().as_str().trim())
        }
        None => (None, raw),
    };

    // Derive-authored templates first (`assert_steps`). A template that
    // structurally matches but whose fields do not parse is a HARD error, never
    // a fall-through to the regexes.
    let docstring = step.docstring.as_deref();
    if let Some(v) = BlockIsChildOf::parse_step(text, docstring)
        .map_err(|e| format!("step {text:?} matches `is a child of` but its fields do not: {e}"))?
    {
        return Ok(Assertion::ParentIs {
            child_id: v.child_id.to_string(),
            parent_id: v.parent_id.to_string(),
            within_secs,
        });
    }
    if let Some(v) = BlockIsTopLevelOf::parse_step(text, docstring).map_err(|e| {
        format!("step {text:?} matches `is a top-level block of` but its fields do not: {e}")
    })? {
        return Ok(Assertion::ParentIs {
            child_id: v.block_id.to_string(),
            parent_id: v.page_id.to_string(),
            within_secs,
        });
    }

    if let Some(v) = BlockIsNthChild::parse_step(text, docstring)
        .map_err(|e| format!("step {text:?} matches `is child N of` but its fields do not: {e}"))?
    {
        return Ok(Assertion::ChildIndex {
            block_id: v.block_id.to_string(),
            index: v.index,
            parent_id: v.parent_id.to_string(),
            within_secs,
        });
    }
    if let Some(v) = BlockComesAfter::parse_step(text, docstring)
        .map_err(|e| format!("step {text:?} matches `comes after` but its fields do not: {e}"))?
    {
        return Ok(Assertion::ComesAfter {
            block_id: v.block_id.to_string(),
            other_id: v.other_id.to_string(),
            within_secs,
        });
    }

    if let Some(v) = BlockHasTaskState::parse_step(text, docstring)
        .map_err(|e| format!("step {text:?} matches `has task state` but its fields do not: {e}"))?
    {
        return Ok(Assertion::TaskState {
            block_id: v.block_id.to_string(),
            expected: Some(v.state),
            within_secs,
        });
    }
    if let Some(v) = BlockHasNoTaskState::parse_step(text, docstring).map_err(|e| {
        format!("step {text:?} matches `has no task state` but its fields do not: {e}")
    })? {
        return Ok(Assertion::TaskState {
            block_id: v.block_id.to_string(),
            expected: None,
            within_secs,
        });
    }

    if let Some(v) = BlockIsCollapsed::parse_step(text, docstring)
        .map_err(|e| format!("step {text:?} matches `is collapsed` but its fields do not: {e}"))?
    {
        return Ok(Assertion::Collapsed {
            block_id: v.block_id.to_string(),
            expected: true,
            within_secs,
        });
    }
    if let Some(v) = BlockIsNotCollapsed::parse_step(text, docstring).map_err(|e| {
        format!("step {text:?} matches `is not collapsed` but its fields do not: {e}")
    })? {
        return Ok(Assertion::Collapsed {
            block_id: v.block_id.to_string(),
            expected: false,
            within_secs,
        });
    }

    if let Some(v) = BlockResolvesLink::parse_step(text, docstring)
        .map_err(|e| format!("step {text:?} matches `resolves link` but its fields do not: {e}"))?
    {
        return Ok(Assertion::LinkResolves {
            block_id: v.block_id.to_string(),
            target: v.target,
            resolved_id: Some(v.resolved_id.to_string()),
            within_secs,
        });
    }
    if let Some(v) = BlockHasDanglingLink::parse_step(text, docstring).map_err(|e| {
        format!("step {text:?} matches `has a dangling link` but its fields do not: {e}")
    })? {
        return Ok(Assertion::LinkResolves {
            block_id: v.block_id.to_string(),
            target: v.target,
            resolved_id: None,
            within_secs,
        });
    }

    // The negative form is tried FIRST: `does not offer` must never fall
    // through to the positive template.
    if let Some(v) = SlashMenuOmits::parse_step(text, docstring)
        .map_err(|e| format!("step {text:?} matches `does not offer` but its fields do not: {e}"))?
    {
        return Ok(Assertion::SlashMenuOffers {
            block_id: v.block_id.to_string(),
            label: v.label,
            expected: false,
            within_secs,
        });
    }
    if let Some(v) = SlashMenuOffers::parse_step(text, docstring)
        .map_err(|e| format!("step {text:?} matches `offers` but its fields do not: {e}"))?
    {
        return Ok(Assertion::SlashMenuOffers {
            block_id: v.block_id.to_string(),
            label: v.label,
            expected: true,
            within_secs,
        });
    }

    // `the widget does not contain "<text>"` / `block "<id>" does not contain
    // "<text>"` — the negative widget forms, for chrome Holon deliberately
    // does not draw. Matched BEFORE the positive `contains` regexes so the
    // longer phrase always wins.
    //
    // An absence assertion takes NO retry budget (see `Assertion::WidgetOmits`),
    // so a `within N seconds` prefix on one is refused rather than dropped:
    // silently ignoring an explicit instruction is exactly the degradation the
    // fail-loud rule forbids.
    let re_root_omits =
        Regex::new(r#"(?i)^the widget does not (?:contain|show)\s+"(?P<text>.*)"$"#).unwrap();
    if let Some(caps) = re_root_omits.captures(text) {
        refuse_within(raw, within_secs)?;
        return Ok(Assertion::WidgetOmits {
            locator: None,
            text: caps["text"].to_string(),
        });
    }
    let re_block_omits =
        Regex::new(r#"(?i)^block\s+"(?P<id>[^"]+)"\s+does not (?:contain|show)\s+"(?P<text>.*)"$"#)
            .unwrap();
    if let Some(caps) = re_block_omits.captures(text) {
        refuse_within(raw, within_secs)?;
        return Ok(Assertion::WidgetOmits {
            locator: Some(caps["id"].to_string()),
            text: caps["text"].to_string(),
        });
    }

    // `the widget renders no error widget` / `block "<id>" renders no error
    // widget` — no `ViewKind::Error` node in the scope. Budget-free for the
    // same reason as the omission forms.
    let re_root_no_error = Regex::new(r"(?i)^the widget renders no error widget$").unwrap();
    if re_root_no_error.is_match(text) {
        refuse_within(raw, within_secs)?;
        return Ok(Assertion::NoErrorWidget { locator: None });
    }
    let re_block_no_error =
        Regex::new(r#"(?i)^block\s+"(?P<id>[^"]+)"\s+renders no error widget$"#).unwrap();
    if let Some(caps) = re_block_no_error.captures(text) {
        refuse_within(raw, within_secs)?;
        return Ok(Assertion::NoErrorWidget {
            locator: Some(caps["id"].to_string()),
        });
    }

    // `the widget shows exactly "<text>"`
    let re_root_exact = Regex::new(r#"(?i)^the widget shows exactly\s+"(?P<text>.*)"$"#).unwrap();
    if let Some(caps) = re_root_exact.captures(text) {
        return Ok(Assertion::WidgetContains {
            locator: None,
            text: caps["text"].to_string(),
            exact: true,
            within_secs,
        });
    }

    // `the widget contains "<text>"` / `the widget shows "<text>"`
    let re_root = Regex::new(r#"(?i)^the widget (?:contains|shows)\s+"(?P<text>.*)"$"#).unwrap();
    if let Some(caps) = re_root.captures(text) {
        return Ok(Assertion::WidgetContains {
            locator: None,
            text: caps["text"].to_string(),
            exact: false,
            within_secs,
        });
    }

    // `block "<id>" contains "<text>"` / `block "<id>" shows "<text>"`
    let re_block =
        Regex::new(r#"(?i)^block\s+"(?P<id>[^"]+)"\s+(?:contains|shows)\s+"(?P<text>.*)"$"#)
            .unwrap();
    if let Some(caps) = re_block.captures(text) {
        return Ok(Assertion::WidgetContains {
            locator: Some(caps["id"].to_string()),
            text: caps["text"].to_string(),
            exact: false,
            within_secs,
        });
    }

    // `focus is on block "<id>"` / `block "<id>" is focused`
    let re_focus1 = Regex::new(r#"(?i)^focus is on block\s+"(?P<id>[^"]+)"$"#).unwrap();
    let re_focus2 = Regex::new(r#"(?i)^block\s+"(?P<id>[^"]+)"\s+is focused$"#).unwrap();
    if let Some(caps) = re_focus1
        .captures(text)
        .or_else(|| re_focus2.captures(text))
    {
        return Ok(Assertion::FocusOn {
            block_id: caps["id"].to_string(),
            within_secs,
        });
    }

    Err(format!(
        "no assertion matcher for step ({:?}): {text:?}",
        step.keyword.trim()
    ))
}
