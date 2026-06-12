//! In-memory navigation provider for no-Turso (Loro-only) sessions.
//!
//! The Turso-backed [`NavigationProvider`](super::NavigationProvider) persists
//! region focus / back-forward history / pins in the `navigation_history` and
//! `navigation_cursor` tables. A Loro-only session has no Turso substrate, so
//! this provider keeps the same state in process memory instead.
//!
//! This is architecturally consistent: navigation state is per-device
//! local-only by design (see the `loro_paths_do_not_reference_navigation_tables`
//! regression test in `mod.rs`) — it is never replicated. Holding it in memory
//! for a Loro session, and letting it live for the lifetime of the process
//! (it need not be reset across a restart-free session boundary), matches that
//! intent.
//!
//! It exposes the *same* [`navigation_operation_descriptors`] as the Turso
//! provider so the render layer builds identical click intents regardless of
//! backend. The actual UI focus signal is driven by the frontend's
//! `maybe_mirror_navigation_focus` mirror (for `focus` / `editor_focus` /
//! `go_home`); this provider's job is to accept every navigation op without
//! error and maintain coherent back/forward history.

use async_trait::async_trait;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;

use super::provider::{ENTITY_NAME, navigation_operation_descriptors};
use crate::core::datasource::{OperationProvider, OperationResult, Result};
use holon_api::{EntityName, NavigationOp, OperationDescriptor, Region, Value};
use holon_core::storage::types::StorageEntity;

/// Per-region back/forward history for one session. `entries[cursor]` is the
/// current view; `None` is the home (no-block) view. `focus` truncates any
/// forward history before pushing, mirroring browser navigation.
#[derive(Default)]
struct RegionHistory {
    entries: Vec<Option<String>>,
    cursor: usize,
}

#[derive(Default)]
struct NavState {
    history: HashMap<String, RegionHistory>,
    /// Pinned block ids per region (move-to-top dedup), mirroring the
    /// open `navigation_history` rows the `focus_roots` matview consumes.
    pins: HashMap<String, Vec<String>>,
}

/// In-memory navigation provider for Loro-only sessions.
#[derive(Default)]
pub struct InMemoryNavigationProvider {
    state: Mutex<NavState>,
}

impl InMemoryNavigationProvider {
    pub fn new() -> Self {
        Self::default()
    }

    fn focus(&self, region: &str, block_id: Option<String>) -> Result<OperationResult> {
        let mut state = self.state.lock().unwrap();
        let hist = state.history.entry(region.to_string()).or_default();
        // Drop forward history, then append and advance the cursor.
        if !hist.entries.is_empty() {
            hist.entries.truncate(hist.cursor + 1);
        }
        hist.entries.push(block_id);
        hist.cursor = hist.entries.len() - 1;
        // Retention cap mirroring the SQL provider's closed-row prune: keep
        // the 100 most recent back-stack entries plus the current one.
        const HISTORY_CAP: usize = 101;
        if hist.entries.len() > HISTORY_CAP {
            let overflow = hist.entries.len() - HISTORY_CAP;
            hist.entries.drain(..overflow);
            hist.cursor -= overflow;
        }
        Ok(OperationResult::irreversible(vec![]))
    }

    fn focus_pin(&self, region: &str, block_id: &str) -> Result<OperationResult> {
        let mut state = self.state.lock().unwrap();
        let pins = state.pins.entry(region.to_string()).or_default();
        pins.retain(|b| b != block_id);
        pins.push(block_id.to_string());
        Ok(OperationResult::irreversible(vec![]))
    }

    fn go_back(&self, region: &str) -> Result<OperationResult> {
        let mut state = self.state.lock().unwrap();
        if let Some(hist) = state.history.get_mut(region)
            && hist.cursor > 0
        {
            hist.cursor -= 1;
        }
        Ok(OperationResult::irreversible(vec![]))
    }

    fn go_forward(&self, region: &str) -> Result<OperationResult> {
        let mut state = self.state.lock().unwrap();
        if let Some(hist) = state.history.get_mut(region)
            && hist.cursor + 1 < hist.entries.len()
        {
            hist.cursor += 1;
        }
        Ok(OperationResult::irreversible(vec![]))
    }
}

#[async_trait]
impl OperationProvider for InMemoryNavigationProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        navigation_operation_descriptors()
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        if entity_name != ENTITY_NAME {
            return Err(format!(
                "InMemoryNavigationProvider: expected entity '{ENTITY_NAME}', got '{entity_name}'"
            )
            .into());
        }

        // `close` carries only `history_id` (no region). In-memory pins have no
        // row ids; the sidebar X-button path is not yet wired for Loro, so this
        // is a coherent no-op rather than a failure.
        if op_name == "close" {
            return Ok(OperationResult::irreversible(vec![]));
        }

        let region: Region = params
            .get("region")
            .cloned()
            .ok_or("Missing required parameter 'region'")?
            .try_into()
            .map_err(|e: Box<dyn std::error::Error + Send + Sync>| e.to_string())?;
        let region_key = region.to_string();

        match NavigationOp::from_str(op_name) {
            Ok(NavigationOp::Focus) => {
                let block_id = params.get("block_id").and_then(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    _ => None,
                });
                self.focus(&region_key, block_id)
            }
            Ok(NavigationOp::FocusPin) => {
                let block_id = params
                    .get("block_id")
                    .and_then(|v| match v {
                        Value::String(s) => Some(s.as_str()),
                        _ => None,
                    })
                    .ok_or_else(|| "Missing required parameter 'block_id'".to_string())?;
                self.focus_pin(&region_key, block_id)
            }
            Ok(NavigationOp::GoBack) => self.go_back(&region_key),
            Ok(NavigationOp::GoForward) => self.go_forward(&region_key),
            Ok(NavigationOp::GoHome) => self.focus(&region_key, None),
            // Pin soft-close is Turso-only (it edits `navigation_history` rows);
            // a Loro-only session has no such table, matching the pre-enum
            // behavior of rejecting `close` here.
            Ok(NavigationOp::Close) => Err(format!(
                "InMemoryNavigationProvider: '{}' is unsupported in Loro-only sessions",
                NavigationOp::Close
            )
            .into()),
            Err(_) => {
                Err(format!("InMemoryNavigationProvider: unknown operation '{op_name}'").into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_history_is_capped() {
        let nav = InMemoryNavigationProvider::new();
        for i in 0..500 {
            nav.focus("main", Some(format!("block:b{i}"))).unwrap();
        }
        let state = nav.state.lock().unwrap();
        let hist = state.history.get("main").unwrap();
        assert_eq!(hist.entries.len(), 101);
        assert_eq!(hist.cursor, 100);
        assert_eq!(hist.entries.last().unwrap().as_deref(), Some("block:b499"));
        assert_eq!(hist.entries.first().unwrap().as_deref(), Some("block:b399"));
    }
}
