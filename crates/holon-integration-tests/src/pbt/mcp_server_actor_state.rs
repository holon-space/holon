//! MCP server actor fragment of the PBT reference model (ADR 0004 / 0006).
//!
//! `MCPServerActorState` holds the state owned by the MCP-server actor: the
//! set of active query watches the server is serving. Per ADR 0004 this state
//! must vanish when the MCP server isn't wired — isolating it here lets a
//! non-MCP wiring drop the fragment instead of carrying dead state.
//!
//! @pbt kind ref
//! @pbt covers mcp-actor-state — the active query-watch set (`query_id →
//!   WatchSpec`) the MCP server serves; single-homed watch registry.

use std::collections::HashMap;

use super::query::WatchSpec;

/// MCP server actor state extracted from `ReferenceState` (ADR 0004 Phase 4).
#[derive(Debug, Clone)]
pub struct MCPServerActorState {
    /// Active query watches (query_id -> watch spec with TestQuery).
    pub active_watches: HashMap<String, WatchSpec>,
}

impl MCPServerActorState {
    pub fn new() -> Self {
        Self {
            active_watches: HashMap::new(),
        }
    }
}

impl Default for MCPServerActorState {
    fn default() -> Self {
        Self::new()
    }
}
