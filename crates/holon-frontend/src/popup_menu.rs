//! Unified popup menu with trait-based providers.
//!
//! A single `PopupMenu` handles all inline popup overlays (slash commands, wiki-links,
//! mentions, etc.). Each popup type is a `PopupProvider` that supplies candidates
//! reactively via futures-signals and handles item selection.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures_signals::signal::{Mutable, Signal, SignalExt};
use futures_signals::signal_vec::{SignalVec, SignalVecExt};
use holon_api::{EntityName, Value};

/// A single item in the popup menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupItem {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
}

/// Action returned from popup interactions.
#[derive(Debug)]
pub enum PopupResult {
    /// Popup not relevant or not active.
    NotActive,
    /// Popup state updated, frontend should re-render.
    Updated,
    /// Popup dismissed.
    Dismissed,
    /// Dispatch an operation (slash commands).
    Execute {
        entity_name: EntityName,
        op_name: String,
        params: HashMap<String, Value>,
    },
    /// Insert text replacing from prefix_start to cursor (links).
    InsertText {
        replacement: String,
        prefix_start: usize,
    },
}

/// Keyboard keys the popup menu handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    Up,
    Down,
    Enter,
    Escape,
    Tab,
}

/// Visible state for the frontend renderer.
#[derive(Debug, Clone)]
pub struct PopupState {
    pub filter: String,
    pub items: Vec<PopupItem>,
    pub selected_index: usize,
}

/// Trait for popup menu content providers.
///
/// Implementations supply candidates reactively and handle item selection.
/// The signal-based `candidates` method supports both sync and async providers:
/// - Sync: `filter.map(|f| compute(f)).to_signal_vec()`
/// - Async: `filter.map_future(|f| tokio_handle.spawn(query(f))).to_signal_vec()`
pub trait PopupProvider: Send + Sync {
    /// Unique source identifier (e.g., "command_menu", "doc_link").
    fn source(&self) -> &str;

    /// Reactive pipeline: filter signal in → items signal out.
    fn candidates(
        &self,
        filter: Pin<Box<dyn Signal<Item = String> + Send + Sync>>,
    ) -> Pin<Box<dyn SignalVec<Item = PopupItem> + Send>>;

    /// Called when the user selects an item.
    fn on_select(&self, item: &PopupItem, filter: &str) -> PopupResult;
}

/// Generic popup menu controller.
///
/// Owns a `Mutable<String>` filter that drives the provider's signal pipeline.
/// Items arrive reactively and are snapshotted for rendering.
pub struct PopupMenu {
    filter: Mutable<String>,
    items: Arc<Mutex<Vec<PopupItem>>>,
    selected_index: usize,
    provider: Option<Arc<dyn PopupProvider>>,
    /// Handle to stop the signal watcher when dismissed. The sender is dropped
    /// to signal cancellation; the watcher task checks the receiver.
    _cancel: Option<tokio::sync::oneshot::Sender<()>>,
}

impl PopupMenu {
    pub fn new() -> Self {
        Self {
            filter: Mutable::new(String::new()),
            items: Arc::new(Mutex::new(Vec::new())),
            selected_index: 0,
            provider: None,
            _cancel: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.provider.is_some()
    }

    /// Get the current popup state for rendering.
    pub fn popup_state(&self) -> Option<PopupState> {
        self.provider.as_ref()?;
        Some(PopupState {
            filter: self.filter.get_cloned(),
            items: self.items.lock().unwrap().clone(),
            selected_index: self.selected_index,
        })
    }

    /// Activate the popup with a provider and initial filter text.
    ///
    /// Returns the signal that the frontend should watch (via `cx.spawn` + `for_each`)
    /// to update `self.items` and trigger re-renders.
    pub fn activate(
        &mut self,
        provider: Arc<dyn PopupProvider>,
        initial_filter: &str,
    ) -> Pin<Box<dyn Signal<Item = Vec<PopupItem>> + Send>> {
        self.filter.set(initial_filter.to_string());
        self.selected_index = 0;
        self.items.lock().unwrap().clear();
        self.provider = Some(provider.clone());

        let filter_signal = self.filter.signal_cloned();
        let items_signal_vec = provider.candidates(Box::pin(filter_signal));

        // Convert SignalVec → Signal<Vec<T>> so the frontend can snapshot on each change
        let items_ref = self.items.clone();
        Box::pin(items_signal_vec.to_signal_cloned().map(move |items| {
            *items_ref.lock().unwrap() = items.clone();
            items
        }))
    }

    /// Synchronously set items (for tests or sync providers that don't need signals).
    pub fn set_items(&mut self, items: Vec<PopupItem>) {
        *self.items.lock().unwrap() = items;
        self.selected_index = 0;
    }

    /// Update the filter text. The signal pipeline reacts automatically.
    pub fn on_text_changed(&mut self, filter: &str) {
        if !self.is_active() {
            return;
        }
        self.filter.set(filter.to_string());
        self.selected_index = 0;
    }

    /// Dismiss the popup.
    pub fn dismiss(&mut self) {
        self.provider = None;
        self.items.lock().unwrap().clear();
        self.selected_index = 0;
        self._cancel = None;
    }

    /// Handle a keyboard event.
    pub fn on_key(&mut self, key: MenuKey) -> PopupResult {
        if !self.is_active() {
            return PopupResult::NotActive;
        }

        match key {
            MenuKey::Up => {
                if self.selected_index > 0 {
                    self.selected_index -= 1;
                }
                PopupResult::Updated
            }
            MenuKey::Down => {
                let len = self.items.lock().unwrap().len();
                if self.selected_index + 1 < len {
                    self.selected_index += 1;
                }
                PopupResult::Updated
            }
            MenuKey::Enter | MenuKey::Tab => self.select_current(),
            MenuKey::Escape => {
                self.dismiss();
                PopupResult::Dismissed
            }
        }
    }

    fn select_current(&mut self) -> PopupResult {
        let provider = match &self.provider {
            Some(p) => p.clone(),
            None => return PopupResult::NotActive,
        };

        let items = self.items.lock().unwrap();
        if self.selected_index >= items.len() {
            return PopupResult::NotActive;
        }

        let item = items[self.selected_index].clone();
        let filter = self.filter.get_cloned();
        drop(items);

        let result = provider.on_select(&item, &filter);
        self.dismiss();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_signals::signal_vec::MutableVec;

    struct MockProvider {
        items: Vec<PopupItem>,
    }

    impl MockProvider {
        fn new(items: Vec<PopupItem>) -> Self {
            Self { items }
        }
    }

    impl PopupProvider for MockProvider {
        fn source(&self) -> &str {
            "mock"
        }

        fn candidates(
            &self,
            filter: Pin<Box<dyn Signal<Item = String> + Send + Sync>>,
        ) -> Pin<Box<dyn SignalVec<Item = PopupItem> + Send>> {
            let items = self.items.clone();
            let signal = filter.map(move |f| {
                let f_lower = f.to_lowercase();
                items
                    .iter()
                    .filter(|item| f.is_empty() || item.label.to_lowercase().contains(&f_lower))
                    .cloned()
                    .collect::<Vec<_>>()
            });
            Box::pin(signal.to_signal_vec())
        }

        fn on_select(&self, item: &PopupItem, _filter: &str) -> PopupResult {
            PopupResult::Execute {
                entity_name: "test".into(),
                op_name: item.id.clone(),
                params: HashMap::new(),
            }
        }
    }

    fn test_items() -> Vec<PopupItem> {
        vec![
            PopupItem {
                id: "delete".into(),
                label: "Delete".into(),
                icon: None,
            },
            PopupItem {
                id: "embed".into(),
                label: "Embed Entity".into(),
                icon: None,
            },
            PopupItem {
                id: "set_field".into(),
                label: "Set Field".into(),
                icon: None,
            },
        ]
    }

    #[test]
    fn starts_inactive() {
        let menu = PopupMenu::new();
        assert!(!menu.is_active());
        assert!(menu.popup_state().is_none());
    }

    #[test]
    fn activate_makes_active() {
        let mut menu = PopupMenu::new();
        let provider = Arc::new(MockProvider::new(test_items()));
        let _signal = menu.activate(provider, "");
        assert!(menu.is_active());
        assert!(menu.popup_state().is_some());
    }

    #[test]
    fn dismiss_makes_inactive() {
        let mut menu = PopupMenu::new();
        let provider = Arc::new(MockProvider::new(test_items()));
        let _signal = menu.activate(provider, "");
        menu.dismiss();
        assert!(!menu.is_active());
    }

    #[test]
    fn escape_dismisses() {
        let mut menu = PopupMenu::new();
        let provider = Arc::new(MockProvider::new(test_items()));
        let _signal = menu.activate(provider, "");
        let result = menu.on_key(MenuKey::Escape);
        assert!(matches!(result, PopupResult::Dismissed));
        assert!(!menu.is_active());
    }

    #[test]
    fn navigation_updates_index() {
        let mut menu = PopupMenu::new();
        let provider = Arc::new(MockProvider::new(test_items()));
        let _signal = menu.activate(provider, "");
        // Pre-populate items (normally done by signal watcher)
        *menu.items.lock().unwrap() = test_items();

        menu.on_key(MenuKey::Down);
        assert_eq!(menu.popup_state().unwrap().selected_index, 1);

        menu.on_key(MenuKey::Down);
        assert_eq!(menu.popup_state().unwrap().selected_index, 2);

        // Can't go past end
        menu.on_key(MenuKey::Down);
        assert_eq!(menu.popup_state().unwrap().selected_index, 2);

        menu.on_key(MenuKey::Up);
        assert_eq!(menu.popup_state().unwrap().selected_index, 1);

        // Can't go below 0
        menu.on_key(MenuKey::Up);
        menu.on_key(MenuKey::Up);
        assert_eq!(menu.popup_state().unwrap().selected_index, 0);
    }

    #[test]
    fn enter_selects_and_dismisses() {
        let mut menu = PopupMenu::new();
        let provider = Arc::new(MockProvider::new(test_items()));
        let _signal = menu.activate(provider, "");
        *menu.items.lock().unwrap() = test_items();

        let result = menu.on_key(MenuKey::Enter);
        assert!(matches!(result, PopupResult::Execute { op_name, .. } if op_name == "delete"));
        assert!(!menu.is_active());
    }

    #[test]
    fn tab_acts_like_enter() {
        let mut menu = PopupMenu::new();
        let provider = Arc::new(MockProvider::new(test_items()));
        let _signal = menu.activate(provider, "");
        *menu.items.lock().unwrap() = test_items();
        menu.on_key(MenuKey::Down); // select "embed"

        let result = menu.on_key(MenuKey::Tab);
        assert!(matches!(result, PopupResult::Execute { op_name, .. } if op_name == "embed"));
    }

    #[test]
    fn filter_change_resets_selection() {
        let mut menu = PopupMenu::new();
        let provider = Arc::new(MockProvider::new(test_items()));
        let _signal = menu.activate(provider, "");
        *menu.items.lock().unwrap() = test_items();
        menu.on_key(MenuKey::Down);
        assert_eq!(menu.popup_state().unwrap().selected_index, 1);

        menu.on_text_changed("emb");
        assert_eq!(menu.popup_state().unwrap().selected_index, 0);
        assert_eq!(menu.popup_state().unwrap().filter, "emb");
    }

    #[test]
    fn keys_when_inactive_return_not_active() {
        let mut menu = PopupMenu::new();
        assert!(matches!(
            menu.on_key(MenuKey::Enter),
            PopupResult::NotActive
        ));
        assert!(matches!(menu.on_key(MenuKey::Up), PopupResult::NotActive));
    }

    #[test]
    fn enter_with_empty_items_returns_not_active() {
        let mut menu = PopupMenu::new();
        let provider = Arc::new(MockProvider::new(vec![]));
        let _signal = menu.activate(provider, "");
        // items is empty

        let result = menu.on_key(MenuKey::Enter);
        assert!(matches!(result, PopupResult::NotActive));
    }
}
