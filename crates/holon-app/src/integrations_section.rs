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
///
/// `provider_name` is projected but never painted: it is the stable technical
/// key, and the tests that assert WHICH integrations the section lists identify
/// them by it (`integration_state_projection.rs`). What the row shows is
/// `display_name` — pinned by the windowed rung, which fails if the technical
/// name reaches the screen.
pub const SIDEBAR_SQL: &str = "SELECT id, provider_name, display_name, icon, status FROM \
                               integration_state WHERE enabled = 1 ORDER BY display_name ASC";

/// Control: every bundled provider, enabled or not — the presence axis in
/// full, because a list filtered to the enabled ones would offer no way to
/// switch a disabled integration ON.
///
/// `configurable` and `configure_progress` are the SETUP axis: whether the
/// provider has a consent flow, and what the flow running now has to say.
pub const SETTINGS_SQL: &str = "SELECT id, provider_name, enabled, config_status, status, \
                                configurable, configure_progress FROM integration_state ORDER BY \
                                provider_name ASC";

/// One line per integration: its icon, the name a person would use for it, and
/// its live status as a single glyph held against the row's trailing edge by an
/// elastic `spacer`. Every row of the list is equally wide, so the glyphs line
/// up down the column without any row measuring another — the discovery list
/// reads as a table with no rules drawn.
///
/// `display_name`, not `provider_name`: the technical name is the sidecar's
/// file name and the row's id, and no surface a person reads should show it.
///
/// The line is `selectable`, and the click opens the integration's own view.
/// Not `navigation.focus`: that refuses a target whose scheme is not `block`,
/// and an integration is not one — `integration.open_default_view` is the op
/// that knows what an integration's default view is (and refuses loudly when it
/// has none).
pub const SIDEBAR_ITEM_TEMPLATE: &str = concat!(
    "list(#{item_template: selectable(row(#{gap: 8, align: \"center\"}, ",
    "icon(col(\"icon\")), ",
    "text(col(\"display_name\"), #{truncate: true}), ",
    "spacer(#{grow: true}), ",
    "integration_status(col(\"status\"))), ",
    "#{action: integration_open_default_view(#{id: col(\"id\")})})})"
);

/// Every bundled integration as one row of a columnar table, columns aligned
/// across the header and all rows (Integration / Config / Status / Enabled /
/// Setup). `live_query` applies this as the WHOLE render expression,
/// interpreted once against the delivered row set, and `table` iterates the
/// rows itself — a scalar template would render a single instance
/// (`integrations_section_renders_every_row`).
///
/// The interactive cells carry their own render-exprs; `table` resolves each
/// row's entity profile and attaches its operations, so the `enabled` switch
/// and the `ops_of` op_buttons are wired without a `render_entity` wrapper.
pub const SETTINGS_ITEM_TEMPLATE: &str = concat!(
    "table(#{columns: [",
    "#{header: \"Integration\", cell: text(col(\"provider_name\")), width: flex(2)}, ",
    "#{header: \"Config\", cell: text(col(\"config_status\"), #{muted: true}), width: flex(1)}, ",
    "#{header: \"Status\", cell: text(col(\"status\"), #{muted: true}), width: flex(1)}, ",
    "#{header: \"Enabled\", cell: state_toggle(#{field: \"enabled\", binding: \"bool\", appearance: \"switch\"}), width: fixed(80)}, ",
    "#{header: \"Setup\", cell: row(#{gap: 6, align: \"center\"}, ",
    "list(#{collection: ops_of(col(\"id\")), item_template: op_button(col(\"name\")), horizontal: true, gap: 8}), ",
    "text(col(\"configure_progress\"), #{muted: true})), width: flex(2)}",
    "]})"
);

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
