#[cfg(feature = "test-helpers")]
mod admission_footprint_sweep;
#[cfg(feature = "test-helpers")]
mod api_pbt;
mod api_suite;
mod automations_journal_matview;
mod batch_delete_cascade_updates;
mod block_links_junction;
mod block_live_data_catch_up;
mod block_op_catalog_parity;
#[cfg(feature = "test-helpers")]
mod capability_certification;
mod computed_tier_dual_path;
#[cfg(feature = "test-helpers")]
mod convert_block_to_page_e2e;
mod create_id_collision_semantics;
#[cfg(feature = "test-helpers")]
mod create_page_from_link;
#[cfg(feature = "test-helpers")]
mod cycle_task_state_vocabulary;
#[cfg(feature = "test-helpers")]
mod decision_shape_claims;
mod dispatcher_arc_schema_gate;
mod dispatcher_guard_gate;
#[cfg(feature = "test-helpers")]
mod e2e_backend_engine_test;
mod edge_field_e2e;
#[cfg(feature = "test-helpers")]
mod empty_question_is_unrepresentable;
#[cfg(feature = "test-helpers")]
mod entity_reference_boundary;
#[cfg(feature = "test-helpers")]
mod fts_e2e;
#[cfg(feature = "test-helpers")]
mod guard_resolve_fails_loud;
mod guard_world_projection_schema;
mod history_overhead_bench;
mod history_query_pack;
mod identity_operations;
mod identity_schema_smoke;
#[cfg(feature = "test-helpers")]
mod ingest_property_removal;
mod integration_tests;
mod journal_feed_matview;
#[cfg(feature = "test-helpers")]
mod json_aggregation_e2e_test;
#[cfg(feature = "test-helpers")]
mod kitchen_cookable_now_e2e;
#[cfg(feature = "test-helpers")]
mod link_candidate_dedup_e2e;
mod live_edit_link_marks;
mod loro_binary_benchmark;
#[cfg(feature = "test-helpers")]
mod malformed_reference_engine_route;
#[cfg(feature = "test-helpers")]
mod merge_blocks_in_shared_pages;
#[cfg(feature = "test-helpers")]
mod no_user_value_reaches_a_log_span_or_error;
mod petri_e2e_pbt;
mod prod_db_skew_dryrun;
#[cfg(feature = "test-helpers")]
mod promote_task_keyword_compound;
#[cfg(feature = "test-helpers")]
mod query_history_mcp;
mod reliability_tests;
#[cfg(feature = "test-helpers")]
mod schema_catalog_follows_service_ddl;
#[cfg(feature = "test-helpers")]
mod shape_gate_cost;
#[cfg(feature = "test-helpers")]
mod shape_gate_source_channel;
mod share_mount_tags_e2e;
mod sharing_boundary_enforcement_pbt;
#[cfg(feature = "test-helpers")]
mod shopping_authoring_door_e2e;
#[cfg(feature = "test-helpers")]
mod sql_write_authority_previous_sibling;
mod stress_tests;
mod stuck_actor_reaches_condition_bus;
mod sync_import_read_only_adoption;
#[cfg(feature = "test-helpers")]
mod sync_suite;
mod trust_gate;
mod trust_proposals_matview;
#[cfg(feature = "test-helpers")]
mod turso_block_query_source_round_trip_pbt;
mod turso_storage_pbt;
#[cfg(feature = "test-helpers")]
mod turso_storage_repros;
#[cfg(feature = "test-helpers")]
mod typing_channel_claims;
#[cfg(feature = "test-helpers")]
mod undo_at_the_top_level;
#[cfg(feature = "test-helpers")]
mod undo_compound_interleave;
#[cfg(feature = "test-helpers")]
mod undo_concurrent_keystrokes;
mod undo_create_id_stability;
mod undo_foundation;
mod undo_gesture_atomicity;
#[cfg(feature = "test-helpers")]
mod undo_inverse_wave1;
#[cfg(feature = "test-helpers")]
mod undo_inverse_wave2;
mod undo_marks_consistency_repro;
#[cfg(feature = "test-helpers")]
mod undo_move_block_e2e;
#[cfg(feature = "test-helpers")]
mod watch_context_membership_leak;
