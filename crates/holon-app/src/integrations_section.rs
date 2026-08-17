//! The Integrations list, authored ONCE.
//!
//! Two surfaces show it — the seeded left-sidebar section and the Settings
//! modal — and they are the same list, not two lists that resemble each other.
//! The query and the item template live here so neither surface can drift; the
//! seeded `assets/default/index.org` carries [`live_query_src`] verbatim, which
//! `holon-app`'s seed test asserts.
//!
//! Placement is NOT shared. Where the list sits, what header it carries and
//! whether the user may delete it are properties of each surface — that is the
//! whole point of the section being layout data.

/// The one query behind both surfaces.
///
/// Every BUNDLED provider, enabled or not: the presence axis in full, because
/// each row now carries a switch and a list filtered to `enabled = 1` would
/// offer no way to switch a disabled integration ON. This supersedes the
/// discovery-only `WHERE enabled = 1` reading, which was correct while the list
/// was read-only.
///
/// `enabled` is the toggle's STATE WORD, projected from the mirror's
/// `enabled_state` column rather than derived here: a `CASE` in this query put
/// a view CREATE inside every interaction window.
pub const SECTION_SQL: &str = "SELECT id, provider_name, enabled_state AS enabled, \
                               config_status, status FROM integration_state ORDER BY \
                               provider_name ASC";

/// A COLLECTION wrapping `render_entity()`, and both halves are load-bearing.
///
/// `live_query` applies its `item_template` as the WHOLE render expression,
/// interpreted once against the delivered row set, so `list(...)` is what
/// iterates the rows; a scalar template here renders a single instance.
/// Pinned by `integrations_section_renders_every_row`.
///
/// `render_entity()` inside it, rather than an inline `row(...)`, because
/// `shared_render_entity_build` resolves the entity profile and is therefore
/// what attaches `operations` to the node — the switch is inert without them.
/// The entity owns its row, the layout owns its placement.
pub const ITEM_TEMPLATE: &str = "list(#{item_template: render_entity()})";

/// The words beside the list. The switch stores a decision and does not act on
/// the running fleet, so the surface has to say so; a silent next-launch effect
/// is the "silently degrades to look fine" case.
pub const NEXT_LAUNCH_NOTICE: &str = "Switching an integration on or off is saved immediately and takes effect at the next launch \
     — this does not start or stop a running integration.";

/// The list, as render-DSL source. Both surfaces embed this exact string.
pub fn live_query_src() -> String {
    format!("live_query(#{{sql: \"{SECTION_SQL}\", item_template: {ITEM_TEMPLATE}}})")
}

/// The list plus its heading and disclosure — the shape a surface that owns no
/// heading of its own (the Settings modal) renders.
pub fn section_src() -> String {
    format!(
        "column(#{{gap: 6}}, text(\"Integrations\", #{{bold: true}}), text(\"{NEXT_LAUNCH_NOTICE}\", \
         #{{muted: true}}), {})",
        live_query_src()
    )
}
