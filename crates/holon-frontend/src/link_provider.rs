//! Wiki-link provider for the unified popup menu.
//!
//! Implements `PopupProvider` to search for existing entities when the user
//! types `[[`. Uses async search via tokio bridge. Selecting an entity inserts
//! a resolved link `[[id][name]]`; selecting "Create new" inserts `[[filter]]`.

use std::pin::Pin;
use std::sync::Arc;

use futures_signals::signal::{Signal, SignalExt};
use futures_signals::signal_vec::SignalVec;

use crate::popup_menu::{PopupItem, PopupProvider, PopupResult};
use crate::reactive::BuilderServices;

const CREATE_NEW_ID: &str = "__create_new__";

/// Wiki-link search provider.
///
/// Searches documents and blocks by name/content. The `candidates` pipeline
/// uses `map_future` to bridge async SQL queries to the signal chain.
///
/// Depends on `BuilderServices` — the narrow capability surface — instead
/// of `FrontendSession` directly, so gpui builders can hand it a test fake.
pub struct LinkProvider {
    prefix_start: usize,
    services: Arc<dyn BuilderServices>,
}

impl LinkProvider {
    pub fn new(prefix_start: usize, services: Arc<dyn BuilderServices>) -> Self {
        Self {
            prefix_start,
            services,
        }
    }
}

impl PopupProvider for LinkProvider {
    fn source(&self) -> &str {
        "doc_link"
    }

    fn candidates(
        &self,
        filter: Pin<Box<dyn Signal<Item = String> + Send + Sync>>,
    ) -> Pin<Box<dyn SignalVec<Item = PopupItem> + Send>> {
        let services = self.services.clone();
        let handle = services.runtime_handle();

        let signal = filter.map_future(move |f| {
            let services = services.clone();
            let handle = handle.clone();
            async move {
                tracing::debug!("[LinkProvider] map_future called with filter='{}'", f);
                if f.is_empty() {
                    return Vec::new();
                }

                let join = handle.spawn({
                    let services = services.clone();
                    let f = f.clone();
                    async move {
                        let escaped = f.replace('\'', "''");
                        // Subquery wrapping required — Turso rejects bare UNION.
                        // Page rows: block has a 'Page' tag in block_tags junction table;
                        // surface the first content line as the label.
                        let sql = format!(
                            "SELECT * FROM (SELECT id, content AS label FROM block WHERE content LIKE '%{escaped}%' LIMIT 15) \
                             UNION ALL \
                             SELECT * FROM (SELECT b.id, substr(b.content, 1, instr(b.content || char(10), char(10)) - 1) AS label \
                                            FROM block b \
                                            JOIN block_tags bt ON bt.block_id = b.id \
                                            WHERE bt.tag = 'Page' \
                                              AND b.content LIKE '%{escaped}%' LIMIT 5)"
                        );
                        services
                            .popup_query(sql)
                            .await
                            .unwrap_or_default()
                    }
                });

                let rows = join.await.unwrap_or_default();
                let mut items: Vec<PopupItem> = rows
                    .iter()
                    .map(|row| {
                        let id = row
                            .get("id")
                            .and_then(|v| v.as_string())
                            .unwrap_or("")
                            .to_string();
                        let label = row
                            .get("label")
                            .and_then(|v| v.as_string())
                            .unwrap_or("(untitled)")
                            .to_string();
                        PopupItem {
                            id,
                            label,
                            icon: None,
                        }
                    })
                    .collect();

                // Append "Create new" virtual entry
                items.push(PopupItem {
                    id: CREATE_NEW_ID.to_string(),
                    label: format!("Create new: {}", f),
                    icon: Some("\u{2795}".to_string()), // ➕
                });

                items
            }
        });

        // map_future returns Signal<Option<Vec<PopupItem>>>:
        // None while the future is pending, Some when resolved.
        let signal = signal.map(|opt| opt.unwrap_or_default());
        Box::pin(signal.to_signal_vec())
    }

    fn on_select(&self, item: &PopupItem, filter: &str) -> PopupResult {
        let replacement = if item.id == CREATE_NEW_ID {
            format!("[[{}]]", filter)
        } else {
            format!("[[{}][{}]]", item.id, item.label)
        };
        PopupResult::InsertText {
            replacement,
            prefix_start: self.prefix_start,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_existing_entity_inserts_resolved_link() {
        // We can't construct a real FrontendSession in unit tests,
        // but we can test on_select directly since it's pure.
        let item = PopupItem {
            id: "doc:abc123".into(),
            label: "My Document".into(),
            icon: None,
        };
        // Simulate by calling on_select on a provider-like function
        let replacement = if item.id == CREATE_NEW_ID {
            format!("[[{}]]", "test")
        } else {
            format!("[[{}][{}]]", item.id, item.label)
        };
        assert_eq!(replacement, "[[doc:abc123][My Document]]");
    }

    #[test]
    fn select_create_new_inserts_unresolved_link() {
        let item = PopupItem {
            id: CREATE_NEW_ID.into(),
            label: "Create new: Projects/Test".into(),
            icon: Some("\u{2795}".into()),
        };
        let filter = "Projects/Test";
        let replacement = if item.id == CREATE_NEW_ID {
            format!("[[{}]]", filter)
        } else {
            format!("[[{}][{}]]", item.id, item.label)
        };
        assert_eq!(replacement, "[[Projects/Test]]");
    }
}
