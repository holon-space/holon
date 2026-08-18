//! The Integrations list, authored ONCE per surface.
//!
//! Two surfaces show integrations and they want different things. The seeded
//! left-sidebar section is DISCOVERY: which integrations are live right now.
//! The Settings modal is CONTROL: every provider this build bundles, each with
//! the switch that decides it. Both read the same mirror table; the query and
//! the item template are what differ, and both live here so neither surface
//! drifts from the seed that embeds it.
//!
//! The seeded `assets/default/index.org` carries [`sidebar_live_query_src`]
//! verbatim, which `holon-app`'s seed test asserts.
//!
//! Placement is NOT shared. Where a list sits, what header it carries and
//! whether the user may delete it are properties of each surface.
//!
//! The two item templates are two templates rather than two variants of the
//! `integration` profile: a variant is chosen by row data and UI state
//! (`pick_active_variant`), neither of which can tell the same row shown in the
//! sidebar from the same row shown in Settings. Only the sidebar's template is
//! spelled out here; the Settings row belongs to the entity, which is why that
//! one is `render_entity()`.

/// Discovery: the integrations that are switched on, and how far their boot
/// connect got.
pub const SIDEBAR_SQL: &str = "SELECT id, provider_name, status FROM integration_state WHERE \
                               enabled = 1 ORDER BY provider_name ASC";

/// Control: every bundled provider, enabled or not — the presence axis in
/// full, because a list filtered to the enabled ones would offer no way to
/// switch a disabled integration ON.
///
/// `configurable` and `configure_progress` are the SETUP axis: whether the
/// provider has a consent flow, and what the flow running now has to say.
pub const SETTINGS_SQL: &str = "SELECT id, provider_name, enabled, config_status, status, \
                                configurable, configure_progress FROM integration_state ORDER BY \
                                provider_name ASC";

/// A read-only line: the provider and its live status, and nothing to click.
///
/// No `selectable`, because `navigation.focus` refuses a target whose scheme is
/// not `block` — a click would surface as a refusal banner rather than as
/// navigation.
pub const SIDEBAR_ITEM_TEMPLATE: &str = "list(#{item_template: row(#{gap: 8, align: \"center\"}, \
                                         text(col(\"provider_name\")), text(col(\"status\"), \
                                         #{muted: true}))})";

/// A COLLECTION wrapping `render_entity()`, and both halves are load-bearing.
///
/// `live_query` applies its `item_template` as the WHOLE render expression,
/// interpreted once against the delivered row set, so `list(...)` is what
/// iterates the rows; a scalar template renders a single instance. Pinned by
/// `integrations_section_renders_every_row`.
///
/// `render_entity()` inside it, rather than an inline `row(...)`, because
/// `shared_render_entity_build` resolves the entity profile and is therefore
/// what attaches `operations` to the node — the switch is inert without them.
/// The entity owns its row, the layout owns its placement.
pub const SETTINGS_ITEM_TEMPLATE: &str = "list(#{item_template: render_entity()})";

/// The words beside the switches. The switch stores a decision and does not act
/// on the running fleet, so the surface has to say so; a silent next-launch
/// effect is the "silently degrades to look fine" case.
pub const NEXT_LAUNCH_NOTICE: &str = "Switching an integration on or off is saved immediately and takes effect at the next launch \
     — this does not start or stop a running integration.";

fn live_query_src(sql: &str, item_template: &str) -> String {
    format!("live_query(#{{sql: \"{sql}\", item_template: {item_template}}})")
}

/// The discovery list, as render-DSL source. The seed embeds this exact string.
pub fn sidebar_live_query_src() -> String {
    live_query_src(SIDEBAR_SQL, SIDEBAR_ITEM_TEMPLATE)
}

/// The control list, as render-DSL source.
pub fn settings_live_query_src() -> String {
    live_query_src(SETTINGS_SQL, SETTINGS_ITEM_TEMPLATE)
}

/// The control list plus its heading and disclosure — the shape a surface that
/// owns no heading of its own (the Settings modal) renders.
pub fn settings_section_src() -> String {
    format!(
        "column(#{{gap: 6}}, text(\"Integrations\", #{{bold: true}}), text(\"{NEXT_LAUNCH_NOTICE}\", \
         #{{muted: true}}), {})",
        settings_live_query_src()
    )
}
