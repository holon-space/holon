//! Frontend surface: panels, sidebar, perspectives, integrations rows, layout.

mod fire_and_forget_dispatch_disclosure;
mod integration_configure_button_visibility;
mod integration_connect_stall_blocks_boot;
mod integration_state_boot_population;
mod integration_state_boot_records_status;
mod integration_state_section_refreshes;
mod integration_state_sync_failing;
mod integrations_section_renders_every_row;
mod layout_bridge_smoke;
mod live_query_serve_failure_is_visible;
mod local_ui_state_precedence;
mod navigate_back_keeps_panel_populated;
#[cfg(feature = "pbt")]
mod now_query_task_rows_render_structured;
#[cfg(feature = "pbt")]
mod page_title_survives_a_never_task_block;
mod perspective_slot_resolution;
#[cfg(feature = "pbt")]
mod recursive_query_renders_descendants;
mod render_source_panels;
mod sidebar_modifier_click_open_tab_probe;
mod split_block_stale_display_regression;
mod tour_spike;
mod widget_only_headline;
