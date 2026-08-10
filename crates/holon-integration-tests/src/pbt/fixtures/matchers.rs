//! Map Gherkin `Then` steps to PBT assertions.
//!
//! Action steps (`Given`/`When`) are NOT here: they are read by the generated
//! step vocabulary (`E2ETransition::parse_step`), the one action vocabulary.
//! This module is what remains until the Then-steps join it.
//!
//! Unmatched steps are a HARD ERROR (fail loud) — a `.feature` referencing
//! a step we don't understand must not silently pass.

use gherkin::Step;
use regex::Regex;

use super::assert::Assertion;

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
