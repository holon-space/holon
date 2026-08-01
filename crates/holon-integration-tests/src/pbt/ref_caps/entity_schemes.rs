//! `RefEntitySchemes` — which entity types an agent has minted at runtime.
//!
//! @pbt kind ref
//! @pbt covers entity-scheme-registration — the once-only gate on
//!   `RegisterEntityScheme`.
//!
//! Reads the MCP-actor fragment's `registered_schemes`. Nothing else in the
//! model may consult it: the `block_links` junction is registration
//! independent, so any expectation that varied with this set would contradict
//! the property the transition exists to expose.

use holon_pbt_core::capabilities::RefEntitySchemes;
use holon_pbt_core::capabilities::RefEntitySchemesMut;

use super::super::reference_state::ReferenceState;

impl RefEntitySchemes for ReferenceState {
    fn entity_scheme_registered(&self, entity_name: &str) -> bool {
        self.mcp.registered_schemes.contains(entity_name)
    }
}

impl RefEntitySchemesMut for ReferenceState {
    fn note_entity_scheme_registered(&mut self, entity_name: &str) {
        self.mcp.registered_schemes.insert(entity_name.to_string());
    }
}
