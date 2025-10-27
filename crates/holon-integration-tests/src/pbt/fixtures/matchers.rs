//! Map Gherkin steps to PBT transitions and assertions.
//!
//! A typed step registry: [`match_action`] turns `Given`/`When` steps into
//! `E2ETransition`s, [`match_assertion`] turns `Then` steps into `Assertion`s.
//! Each matcher constructs the typed struct and converts via `…::from`, so
//! there is no untyped intermediate.
//!
//! Unmatched steps are a HARD ERROR (fail loud) — a `.feature` referencing
//! a step we don't understand must not silently pass.

use gherkin::Step;
use holon_api::{EntityUri, Region};
use regex::Regex;

use super::assert::Assertion;
use crate::pbt::transitions::{
    ClickBlock, DeleteBackward, E2ETransition, FocusEditableText, Indent, NavigateFocus, Outdent,
    SplitBlock, StartApp, TypeChars, WriteOrgFile,
};

/// Translate a single `Given`/`When` step into an action transition.
/// (`Then` steps are handled by [`match_assertion`].)
pub fn match_action(step: &Step) -> Result<E2ETransition, String> {
    let text = step.value.trim();

    // "the app is started" (optionally "... with loro")
    let re_start = Regex::new(r"(?i)^(the )?app is started").unwrap();
    if re_start.is_match(text) {
        let enable_loro = text.to_lowercase().contains("with loro");
        return Ok(StartApp {
            wait_for_ready: true,
            enable_fake_mcp: true,
            enable_loro,
        }
        .into());
    }

    // `an org file "<name>":` followed by a docstring holding the org content.
    let re_org = Regex::new(r#"(?i)^an? org file\s+"(?P<filename>[^"]+)"\s*:?$"#).unwrap();
    if let Some(caps) = re_org.captures(text) {
        let filename = caps["filename"].to_string();
        let content = step.docstring.clone().ok_or_else(|| {
            format!("org-file step requires a docstring with org content: {text:?}")
        })?;
        return Ok(WriteOrgFile::from_org_text(filename, &content)
            .map_err(|e| format!("failed to parse org-file step content: {e}"))?
            .into());
    }

    // `I focus block "<id>" in region "<region>"`
    let re_focus =
        Regex::new(r#"(?i)^I focus block\s+"(?P<id>[^"]+)"\s+in region\s+"(?P<region>[^"]+)"$"#)
            .unwrap();
    if let Some(caps) = re_focus.captures(text) {
        // ALLOW(entity_uri_from_raw): id from regex over Gherkin step text (test DSL boundary)
        let block_id = EntityUri::from_raw(&caps["id"]);
        let region = parse_region(&caps["region"])?;
        return Ok(NavigateFocus { region, block_id }.into());
    }

    // `I split block "<id>" at position <n>`
    let re_split =
        Regex::new(r#"(?i)^I split block\s+"(?P<id>[^"]+)"\s+at position\s+(?P<pos>\d+)$"#)
            .unwrap();
    if let Some(caps) = re_split.captures(text) {
        // ALLOW(entity_uri_from_raw): id from regex over Gherkin step text (test DSL boundary)
        let block_id = EntityUri::from_raw(&caps["id"]);
        let position: usize = caps["pos"]
            .parse()
            .map_err(|e| format!("invalid position in {text:?}: {e}"))?;
        return Ok(SplitBlock { block_id, position }.into());
    }

    // `I click block "<id>"` (optionally `in region "<region>"`, default main)
    let re_click = Regex::new(
        r#"(?i)^I click block\s+"(?P<id>[^"]+)"(?:\s+in region\s+"(?P<region>[^"]+)")?$"#,
    )
    .unwrap();
    if let Some(caps) = re_click.captures(text) {
        // ALLOW(entity_uri_from_raw): id from regex over Gherkin step text (test DSL boundary)
        let block_id = EntityUri::from_raw(&caps["id"]);
        let region = match caps.name("region") {
            Some(m) => parse_region(m.as_str())?,
            None => Region::Main,
        };
        return Ok(ClickBlock { region, block_id }.into());
    }

    // `I focus the editor of block "<id>"`
    let re_focus_editor =
        Regex::new(r#"(?i)^I focus the editor of block\s+"(?P<id>[^"]+)"$"#).unwrap();
    if let Some(caps) = re_focus_editor.captures(text) {
        return Ok(FocusEditableText {
            // ALLOW(entity_uri_from_raw): id from regex over Gherkin step text (test DSL boundary)
            block_id: EntityUri::from_raw(&caps["id"]),
        }
        .into());
    }

    // `I type "<text>"`
    let re_type = Regex::new(r#"(?i)^I type\s+"(?P<text>.*)"$"#).unwrap();
    if let Some(caps) = re_type.captures(text) {
        return Ok(TypeChars {
            text: caps["text"].to_string(),
        }
        .into());
    }

    // `I indent block "<id>"`
    let re_indent = Regex::new(r#"(?i)^I indent block\s+"(?P<id>[^"]+)"$"#).unwrap();
    if let Some(caps) = re_indent.captures(text) {
        return Ok(Indent {
            // ALLOW(entity_uri_from_raw): id from regex over Gherkin step text (test DSL boundary)
            block_id: EntityUri::from_raw(&caps["id"]),
        }
        .into());
    }

    // `I outdent block "<id>"`
    let re_outdent = Regex::new(r#"(?i)^I outdent block\s+"(?P<id>[^"]+)"$"#).unwrap();
    if let Some(caps) = re_outdent.captures(text) {
        return Ok(Outdent {
            // ALLOW(entity_uri_from_raw): id from regex over Gherkin step text (test DSL boundary)
            block_id: EntityUri::from_raw(&caps["id"]),
        }
        .into());
    }

    // `I press backspace` / `I press backspace <n> times`
    let re_backspace = Regex::new(r#"(?i)^I press backspace(?:\s+(?P<n>\d+)\s+times?)?$"#).unwrap();
    if let Some(caps) = re_backspace.captures(text) {
        let count = match caps.name("n") {
            Some(m) => m
                .as_str()
                .parse::<usize>()
                .map_err(|e| format!("invalid backspace count in {text:?}: {e}"))?,
            None => 1,
        };
        return Ok(DeleteBackward { count }.into());
    }

    Err(format!(
        "no matcher for step ({:?}): {text:?}",
        step.keyword.trim()
    ))
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

fn parse_region(s: &str) -> Result<Region, String> {
    match s.to_lowercase().as_str() {
        "main" => Ok(Region::Main),
        "left" | "left_sidebar" | "leftsidebar" => Ok(Region::LeftSidebar),
        "right" | "right_sidebar" | "rightsidebar" => Ok(Region::RightSidebar),
        other => Err(format!("unknown region: {other:?}")),
    }
}
