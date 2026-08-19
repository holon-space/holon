//! Turso store surface: materialized views, watches, FTS, and query
//! equivalence.

mod chat_view_message_join;
mod fts_query_block_e2e;
mod matview_duplicate_row_repro;
mod matview_reboot_duplicate_repro;
mod query_equivalence_pbt;
mod ref_entity_lookup_parity;
mod turso_ivm_index_bug;
mod watch_guard_raii;
mod watch_query_ordering_spec_wiring;
mod watch_recovers_when_table_appears;
mod watch_ui;
