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
//! @pbt covers entity-scheme-registration — which entity types an agent has
//!   minted through the `create_entity_type` tool.

use std::collections::BTreeSet;
use std::collections::HashMap;

use super::query::WatchSpec;

/// MCP server actor state extracted from `ReferenceState` (ADR 0004 Phase 4).
#[derive(Debug, Clone)]
pub struct MCPServerActorState {
    /// Active query watches (query_id -> watch spec with TestQuery).
    pub active_watches: HashMap<String, WatchSpec>,
    /// Entity types (SQL table spelling) an agent has registered through the
    /// `create_entity_type` tool. Bookkeeping only — it stops
    /// `RegisterEntityScheme` from minting the same entity twice and is read by
    /// no link oracle.
    pub registered_schemes: BTreeSet<String>,
}

impl MCPServerActorState {
    pub fn new() -> Self {
        Self {
            active_watches: HashMap::new(),
            registered_schemes: BTreeSet::new(),
        }
    }
}

impl Default for MCPServerActorState {
    fn default() -> Self {
        Self::new()
    }
}
