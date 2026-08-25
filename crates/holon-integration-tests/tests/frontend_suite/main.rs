//! Frontend surface: panels, sidebar, perspectives, integrations rows, layout.

mod integration_configure_button_visibility;
mod integration_state_boot_population;
mod integration_state_boot_records_status;
mod integration_state_section_refreshes;
mod integrations_section_renders_every_row;
mod layout_bridge_smoke;
mod local_ui_state_precedence;
mod navigate_back_keeps_panel_populated;
#[cfg(feature = "pbt")]
mod now_query_task_rows_render_structured;
mod perspective_slot_resolution;
mod sidebar_modifier_click_open_tab_probe;
mod split_block_stale_display_regression;
mod tour_spike;
mod widget_only_headline;
