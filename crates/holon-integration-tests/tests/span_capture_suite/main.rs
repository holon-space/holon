//! Tests that read back ERROR events and background-worker panics through
//! `test_tracing::SpanCollector`.
//!
//! `SpanCollector::global()` installs the process-global subscriber carrying
//! `ErrorCaptureLayer` and PANICS if anything else claimed the global default
//! first. So no member of this suite may install a subscriber of its own, and
//! no test that installs one (see `boot_suite`) may move in here. What the
//! collector CAPTURES is per-`TestScope`, not global, so these tests still run
//! in parallel in one process.

mod declared_column_parity;
mod host_page_with_inline_subpages_ingest;
mod interaction_trace_connectivity;
mod region_writeback_loss;
mod split_doc_root_idless_duplicates;
mod substrate_corruption_faults;
