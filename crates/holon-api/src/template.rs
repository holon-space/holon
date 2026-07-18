//! Template property keys and the instantiation operation name — the single
//! spelling shared by the backend op (`holon::core::template_instantiation`,
//! `holon::api::operation_engine`) and the frontend picker
//! (`holon_frontend::command_provider`). Colocated here (the boundary crate
//! both sides depend on) so a rename can never desync the two.
//!
//! See docs/Proposals/Templating-2026-07-12.md.

/// Block property whose presence marks the subtree root a template; its value
/// is the human-readable template name (e.g. `daily-journal`).
pub const TEMPLATE_MARKER_PROPERTY: &str = "template";

/// Block property declaring the template's variables (`name` / `name=default`,
/// comma-separated).
pub const TEMPLATE_VARS_PROPERTY: &str = "template_vars";

/// Provenance edge stamped on an instance root pointing at its template.
pub const INSTANCE_OF_PROPERTY: &str = "instance_of";

/// The engine-level operation that deep-copies a template subtree.
pub const INSTANTIATE_TEMPLATE_OP: &str = "instantiate_template";

/// THE single case-insensitivity authority for template-marker keys.
///
/// The org parser lifts drawer keys VERBATIM (`:TEMPLATE:` → `"TEMPLATE"`,
/// uppercase), while programmatic creation uses lowercase `template`. Both must
/// resolve. Both the backend planner (`template_instantiation`) and the
/// frontend picker (`list_templates`) match markers through THIS function, so
/// they can never diverge on casing again (the round-3 live bug: the picker did
/// a case-sensitive lowercase lookup and never saw org-authored templates).
///
/// Returns the actual key present in `keys` that matches `marker` ignoring
/// ASCII case, if any.
pub fn find_template_marker_key<'a, I>(keys: I, marker: &str) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    keys.into_iter().find(|k| k.eq_ignore_ascii_case(marker))
}

/// Case-insensitive marker lookup over a [`Block`]'s properties, returning the
/// marker's value as a string (the picker's path). `None` when no key matches
/// `marker` (case-insensitively) or its value is not a string.
pub fn template_marker_value<'a>(block: &'a crate::block::Block, marker: &str) -> Option<&'a str> {
    let key = find_template_marker_key(block.properties.keys().map(String::as_str), marker)?;
    block.properties.get(key)?.as_string()
}
