//! The canned inline template the keystone's `InstantiateTemplate` uses, and
//! the reference model of what instantiating it produces.
//!
//! ONE definition of the template lives here so the SUT driver's seed
//! (`DirectUserDriver::instantiate_template`) and the oracle's expectation
//! (`pbt::transitions::instantiate_template`) cannot drift apart. The
//! expectation is COMPUTED by [`instantiate`] rather than written down, so a
//! mark that crosses or follows a `{{var}}` slot is modelled instead of
//! accidentally agreeing with prod.

use holon_api::InlineMark;
use holon_api::MarkSpan;

pub const TPL_ROOT: &str = "block:tpl";
pub const TPL_CHILD: &str = "block:tpl-c1";
pub const TPL_ROOT_CONTENT: &str = "{{date}}";
pub const TPL_CHILD_CONTENT: &str = "see {{date}} now";
pub const TPL_VARS: &str = "date, mood=neutral";

/// The definition child's rich text. `Bold` sits entirely before the
/// `{{date}}` slot (identity under remapping) while `Italic` starts at the
/// slot and runs past it, so instantiation must both STRETCH the span over the
/// substituted value and SHIFT its end — the case a hardcoded expectation gets
/// wrong. Both spans render as well-formed org (`*see* /{{date}} now/`), so the
/// definition and every instance survive the org round-trip unchanged.
pub fn tpl_child_marks() -> Vec<MarkSpan> {
    vec![
        MarkSpan::new(0, 3, InlineMark::Bold),
        MarkSpan::new(4, 16, InlineMark::Italic),
    ]
}

/// The `marks` create-param the driver seeds the definition child with — the
/// same wire JSON production's org/markdown ingest emits.
pub fn tpl_child_marks_json() -> String {
    holon_api::marks_to_json(&tpl_child_marks())
}

/// One template node's content and marks after instantiation.
pub struct Instantiated {
    pub content: String,
    pub marks: Option<Vec<MarkSpan>>,
}

/// Substitute every `{{name}}` slot from `bindings` and carry `marks` across
/// the substitution, mirroring `holon_api::template_instantiation`: offsets
/// after a slot shift by the length delta, and a boundary strictly inside a
/// slot snaps outward (start to the slot's start, end past the substituted
/// value) so a mark covering a slot stretches over what replaced it.
///
/// Offsets are Unicode-scalar, as `MarkSpan` requires. A missing binding or an
/// unterminated slot is a fixture authoring error and panics.
///
/// One measured divergence from prod, currently unreachable: an EMPTY
/// substitution collapses an in-slot span to `Some([w..w])` here where
/// `plan_instantiation` yields `Some([])`. The generator draws `[a-z]{3,6}`, so
/// no binding is ever empty; this becomes real the moment empty bindings are
/// drawable.
pub fn instantiate(
    template: &str,
    marks: &[MarkSpan],
    bindings: &[(String, String)],
) -> Instantiated {
    let chars: Vec<char> = template.chars().collect();
    let mut content = String::new();
    // (slot start, slot length, substituted length) in ORIGINAL scalar offsets.
    let mut edits: Vec<(usize, usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '{' && chars.get(i + 1) == Some(&'{') {
            let close = (i + 2..chars.len().saturating_sub(1))
                .find(|&j| chars[j] == '}' && chars[j + 1] == '}')
                .unwrap_or_else(|| {
                    panic!("template fixture: unterminated '{{{{' slot in {template:?}")
                });
            let name = chars[i + 2..close].iter().collect::<String>();
            let name = name.trim();
            let value = bindings
                .iter()
                .find(|(k, _)| k == name)
                .unwrap_or_else(|| {
                    panic!("template fixture: no binding for '{name}' in {template:?}")
                })
                .1
                .clone();
            content.push_str(&value);
            edits.push((i, close + 2 - i, value.chars().count()));
            i = close + 2;
        } else {
            content.push(chars[i]);
            i += 1;
        }
    }

    let map_offset = |offset: usize, is_end: bool| -> usize {
        let mut delta: i64 = 0;
        for &(start, old_len, new_len) in &edits {
            if offset <= start {
                break;
            }
            if offset >= start + old_len {
                delta += new_len as i64 - old_len as i64;
            } else {
                let base = (start as i64 + delta) as usize;
                return if is_end { base + new_len } else { base };
            }
        }
        (offset as i64 + delta) as usize
    };
    let remapped: Vec<MarkSpan> = marks
        .iter()
        .map(|s| MarkSpan {
            start: map_offset(s.start, false),
            end: map_offset(s.end, true),
            mark: s.mark.clone(),
        })
        .collect();

    Instantiated {
        content,
        marks: (!remapped.is_empty()).then_some(remapped),
    }
}

/// The instance child of the canned template for one set of bindings.
pub fn instantiated_child(bindings: &[(String, String)]) -> Instantiated {
    instantiate(TPL_CHILD_CONTENT, &tpl_child_marks(), bindings)
}

/// The instance root of the canned template for one set of bindings
/// (marks-free).
pub fn instantiated_root(bindings: &[(String, String)]) -> Instantiated {
    instantiate(TPL_ROOT_CONTENT, &[], bindings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(v: &str) -> Vec<(String, String)> {
        vec![
            ("date".to_string(), v.to_string()),
            ("mood".to_string(), "neutral".to_string()),
        ]
    }

    #[test]
    fn the_canned_child_stretches_and_shifts_its_marks() {
        let out = instantiated_child(&date("xyz"));
        assert_eq!(out.content, "see xyz now");
        assert_eq!(
            out.marks,
            Some(vec![
                MarkSpan::new(0, 3, InlineMark::Bold),
                MarkSpan::new(4, 11, InlineMark::Italic),
            ])
        );
    }

    #[test]
    fn a_boundary_inside_a_slot_snaps_outward() {
        // start 6 and end 10 both sit strictly inside `{{date}}` (4..12).
        let marks = vec![MarkSpan::new(6, 10, InlineMark::Underline)];
        let out = instantiate(TPL_CHILD_CONTENT, &marks, &date("wednesday"));
        assert_eq!(out.content, "see wednesday now");
        assert_eq!(
            out.marks,
            Some(vec![MarkSpan::new(4, 13, InlineMark::Underline)])
        );
    }

    #[test]
    fn a_span_after_a_slot_only_shifts() {
        let marks = vec![MarkSpan::new(13, 16, InlineMark::Code)];
        let out = instantiate(TPL_CHILD_CONTENT, &marks, &date("xyz"));
        assert_eq!(
            out.marks,
            Some(vec![MarkSpan::new(8, 11, InlineMark::Code)])
        );
    }

    /// The reference remap must agree with production's own planner on the
    /// canned fixture — otherwise the oracle is modelling a different template
    /// engine than the one under test.
    #[test]
    fn the_remap_agrees_with_production_plan_instantiation() {
        use holon_api::template_instantiation::InstantiateRequest;
        use holon_api::template_instantiation::TemplateNode;
        use holon_api::template_instantiation::plan_instantiation;

        let root = TemplateNode {
            id: TPL_ROOT.to_string(),
            parent_id: String::new(),
            content: TPL_ROOT_CONTENT.to_string(),
            content_type: "text".to_string(),
            block_type: "text".to_string(),
            sort_key: "A0".to_string(),
            properties: Some(format!(
                r#"{{"template":"t","template_vars":"{TPL_VARS}"}}"#
            )),
            ..TemplateNode::default()
        };
        let child = TemplateNode {
            id: TPL_CHILD.to_string(),
            parent_id: TPL_ROOT.to_string(),
            content: TPL_CHILD_CONTENT.to_string(),
            content_type: "text".to_string(),
            block_type: "text".to_string(),
            sort_key: "A0".to_string(),
            marks: Some(tpl_child_marks_json()),
            ..TemplateNode::default()
        };
        let bindings = date("xyz");
        let request = InstantiateRequest {
            template_id: TPL_ROOT.to_string(),
            target_parent: "block:target".to_string(),
            context_key: "fixture".to_string(),
            bindings: bindings.iter().cloned().collect(),
            replace_block: None,
        };
        let plan = plan_instantiation(&[root, child], &request).expect("plan");

        let expected_child = instantiated_child(&bindings);
        let child_params = &plan.creates[1];
        assert_eq!(
            child_params.get("content").and_then(|v| v.as_string()),
            Some(expected_child.content.as_str())
        );
        assert_eq!(
            child_params.get("marks").and_then(|v| v.as_string()),
            Some(holon_api::marks_to_json(expected_child.marks.as_ref().unwrap()).as_str())
        );
        assert_eq!(
            plan.creates[0].get("content").and_then(|v| v.as_string()),
            Some(instantiated_root(&bindings).content.as_str())
        );
    }
}
