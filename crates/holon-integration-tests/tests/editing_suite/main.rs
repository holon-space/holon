//! Block editing operations: split, merge, undo/redo, task-state cycling.

mod cycle_task_state_cold_boot_reingest;
mod editor_pure_pbt;
mod merge_blocks_pbt;
mod schedule_boundary_observables;
mod split_block_content_pbt;
mod split_undo_redo_reconcile;
mod undo_cycle_task_state_coverage;
mod undo_prod_session_wiring;
