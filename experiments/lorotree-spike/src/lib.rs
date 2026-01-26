//! LoroTree spike: experiments validating hypotheses for the all-in-tree architecture.
//!
//! Each test module validates a specific hypothesis:
//! 1. tree_crud: Basic LoroTree operations (create, move, delete, fractional index)
//! 2. nested_containers: LoroText inside get_meta() LoroMap
//! 3. fork_and_prune: Extract subtree via fork → delete → shallow_snapshot
//! 4. undo: UndoManager across structure + content operations
//! 5. sync: Two-peer sync via export/import incremental updates
//! 6. gc: Shallow snapshot for GC after subtree extraction

#[cfg(test)]
mod tests;
