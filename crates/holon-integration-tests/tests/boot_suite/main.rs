//! Cold-boot, restart, and boot-time ingest regressions.
//!
//! Six members install a `tracing_subscriber::fmt()` subscriber of their own
//! (`init_tracing`), which claims the process-global default. That is why no
//! `SpanCollector`-based test may join this suite — see `span_capture_suite`.

mod boot_projector_gated_on_scan;
mod boot_scan_bad_file_survives;
mod cold_boot_share_disclosure;
mod forward_edge_ingest_regression;
mod idonly_folder_companion_identity_collision;
mod journals_restart_survival;
mod junction_survives_reboot_repro;
mod marks_lengthening_batch_no_quarantine;
mod mcp_mirrored_entity_write_authority;
mod phantom_loro_exists_repro;
mod wiki_link_ingest_marks_junction;
