//! The composed keystone's remote-list fixture: the SHIPPED shopping sidecar
//! and the shipped `shopping_item` type, with the list app itself replaced by
//! an in-process fake.
//!
//! Only the transport is a double. Both jaq mappings are the sidecar's own,
//! run by the real `RestCallSurface`, and the peer the round calls is the real
//! `RestListPeer` — so a mapping that sends the wrong command, or drops a field
//! on the way in or out, fails the keystone.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;

use holon_connections::ListSyncSpec;
use holon_mcp_client::CredentialRoot;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpTransport;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::rest_transport::RESPONSE_VERSION_KEY;
use holon_mcp_client::rest_transport::RestCallSurface;
use rmcp::ServiceError;
use rmcp::model::CallToolRequestParam;
use rmcp::model::CallToolResult;
use rmcp::model::Content;
use rmcp::model::ReadResourceRequestParam;
use rmcp::model::ReadResourceResult;

/// The provider the sidecar is shipped under, and the connection's name.
pub const CONNECTION: &str = "shopping";
pub const ENTITY: &str = "shopping_item";
/// The one category the fixture list uses. The app names an item by its name
/// alone, so a second category would add no identity the fixture exercises.
pub const CAT: &str = "R";

/// The peer-authoritative columns, in the invariant's projection order: the id
/// first, then the columns the peer serves.
pub const PROJECTION: &[&str] = &["id", "name", "cat", "count", "checked"];

/// The identity pair the sidecar's key expression reads.
pub const IDENTITY_COLUMNS: &[&str] = &["name", "cat"];

fn sidecar() -> IntegrationFileConfig {
    let bundled = holon_mcp_client::bundled_sidecar(CONNECTION)
        .unwrap_or_else(|| panic!("this build ships no '{CONNECTION}' sidecar"));
    IntegrationFileConfig::from_yaml(bundled.yaml)
        .unwrap_or_else(|e| panic!("the shipped '{CONNECTION}' sidecar does not parse: {e}"))
}

pub fn list_sync_spec() -> ListSyncSpec {
    sidecar()
        .holon
        .and_then(|holon| holon.list_sync)
        .unwrap_or_else(|| panic!("the shipped '{CONNECTION}' sidecar declares no list_sync"))
}

/// The sidecar's manual as the production transport compiles it. Its mappings
/// are what this fixture borrows; its HTTP leg is never called.
fn shipped_surface() -> RestCallSurface {
    let lookup = |name: &str| match name {
        "SHOPPING_LIST_URL" => Some("https://list.invalid/api/list/fixture".to_string()),
        _ => None,
    };
    let config = sidecar()
        .into_mcp_config_with(
            CONNECTION.to_string(),
            &lookup,
            &CredentialRoot::new("/nonexistent/holon-keystone-credentials"),
        )
        .unwrap_or_else(|e| panic!("the shipped '{CONNECTION}' sidecar does not resolve: {e:#}"));
    match config.transport {
        McpTransport::Rest { manual, .. } => RestCallSurface::new(manual),
        other => panic!("the shipped '{CONNECTION}' sidecar is not a rest connection: {other:?}"),
    }
}

/// The id a row is stored under: the sidecar's `response` mapping derives it
/// from the content pair. Fixture names and categories are plain ASCII, so the
/// mapping's `@uri` leaves them as they are.
pub fn row_id(name: &str, cat: &str) -> String {
    assert!(
        [name, cat]
            .iter()
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric())),
        "fixture names and categories are plain ASCII words; got name '{name}', cat '{cat}'"
    );
    format!("shopping-item:{cat}:{name}")
}

/// One entry of the app's active list, in the app's own fields.
#[derive(Debug, Clone, PartialEq)]
pub struct AppEntry {
    pub name: String,
    pub cat: String,
    pub count: Option<String>,
}

/// One ticked entry, in the app's own fields. Ticking an item moves its amount
/// into `pickedCount`; editing the amount of a ticked item writes `count` and
/// `pickedCount` alike.
#[derive(Debug, Clone, PartialEq)]
pub struct PickedEntry {
    pub cat: String,
    pub count: Option<String>,
    pub picked_count: Option<String>,
}

/// The app's two collections, as its server keeps them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppList {
    /// The active list, in list order. Two entries may share a name: the app
    /// matches by display text and lets a person type one twice.
    pub items: Vec<AppEntry>,
    /// Ticked items, keyed by name as the app keys them.
    pub picked: BTreeMap<String, PickedEntry>,
}

impl AppList {
    /// A person adds an entry in the app, active or ticked.
    pub fn add(&mut self, entry: AppEntry, ticked: bool) {
        if ticked {
            self.picked.insert(
                entry.name,
                PickedEntry {
                    cat: entry.cat,
                    count: None,
                    picked_count: entry.count,
                },
            );
        } else {
            self.items.push(entry);
        }
    }

    /// A person removes every entry of that name, active and ticked.
    pub fn remove(&mut self, name: &str) {
        self.items.retain(|e| e.name != name);
        self.picked.remove(name);
    }

    /// A tick that reached the app the way its own client sends one
    /// (`list.js` `removeItem` + `addPickedItem`): every active entry of that
    /// name leaves, and the ticked entry carries the first one's amount as
    /// `pickedCount`.
    pub fn tick(&mut self, name: &str) {
        let at = self
            .items
            .iter()
            .position(|e| e.name == name)
            .unwrap_or_else(|| panic!("the list app holds no active '{name}' to tick"));
        let entry = self.items.remove(at);
        self.items.retain(|e| e.name != name);
        self.picked.insert(
            entry.name,
            PickedEntry {
                cat: entry.cat,
                count: None,
                picked_count: entry.count,
            },
        );
    }

    /// A person un-ticks a ticked item in the app: it goes back on the active
    /// list with its picked amount (`list.js` `removePickedItem` + `cmd: add`).
    pub fn untick(&mut self, name: &str) {
        let ticked = self
            .picked
            .remove(name)
            .unwrap_or_else(|| panic!("the list app holds no ticked '{name}' to un-tick"));
        self.items.push(AppEntry {
            name: name.to_string(),
            cat: ticked.cat,
            count: ticked.picked_count.or(ticked.count),
        });
    }

    /// A person changes an item's amount in the app: the first active entry
    /// of that name, else its ticked entry.
    pub fn edit_amount(&mut self, name: &str, amount: &str) {
        if let Some(entry) = self.items.iter_mut().find(|e| e.name == name) {
            entry.count = Some(amount.to_string());
            return;
        }
        let entry = self
            .picked
            .get_mut(name)
            .unwrap_or_else(|| panic!("the list app holds no '{name}' to edit"));
        entry.count = Some(amount.to_string());
        entry.picked_count = Some(amount.to_string());
    }

    /// Whether the list holds `name` both active and ticked.
    pub fn is_folded(&self, name: &str) -> bool {
        self.picked.contains_key(name) && self.items.iter().any(|e| e.name == name)
    }

    /// Every entry as `[place, name, cat, count, pickedCount]`, an absent
    /// field as the empty string, sorted.
    pub fn entries(&self) -> Vec<Vec<String>> {
        let text = |field: &Option<String>| field.clone().unwrap_or_default();
        let mut entries: Vec<Vec<String>> = self
            .items
            .iter()
            .map(|e| {
                vec![
                    "active".to_string(),
                    e.name.clone(),
                    e.cat.clone(),
                    text(&e.count),
                    String::new(),
                ]
            })
            .chain(self.picked.iter().map(|(name, e)| {
                vec![
                    "ticked".to_string(),
                    name.clone(),
                    e.cat.clone(),
                    text(&e.count),
                    text(&e.picked_count),
                ]
            }))
            .collect();
        entries.sort();
        entries
    }

    fn body(&self, version: i64) -> serde_json::Value {
        let items: Vec<serde_json::Value> = self
            .items
            .iter()
            .map(|entry| {
                let mut item = serde_json::json!({"name": entry.name, "cat": entry.cat});
                if let Some(count) = &entry.count {
                    item["count"] = serde_json::json!(count);
                }
                item
            })
            .collect();
        let picked: serde_json::Map<String, serde_json::Value> = self
            .picked
            .iter()
            .map(|(name, entry)| {
                let mut value =
                    serde_json::json!({"cat": entry.cat, "date": "2026-09-15T08:00:00Z"});
                if let Some(count) = &entry.picked_count {
                    value["pickedCount"] = serde_json::json!(count);
                }
                if let Some(count) = &entry.count {
                    value["count"] = serde_json::json!(count);
                }
                (name.clone(), value)
            })
            .collect();
        serde_json::json!({
            "items": items,
            "pickedItems": picked,
            "version": version,
            "pickedItemsVersion": version,
            "options": {"prices": false, "cats": [CAT]},
        })
    }

    /// Apply one command the way the app's own client applies it
    /// (`www/js/list.js` `applyCommands` / `addPickedItem`). Anything this
    /// connection has no business sending fails the round.
    fn apply(&mut self, command: &serde_json::Value) {
        let good = &command["good"];
        let text = |field: &str| -> Option<String> {
            match &good[field] {
                serde_json::Value::Null => None,
                serde_json::Value::String(s) => Some(s.clone()),
                other => panic!(
                    "fake list app: `good.{field}` must be text, as the app writes it; got \
                     {other} in {command}"
                ),
            }
        };
        let entry = AppEntry {
            name: text("name")
                .unwrap_or_else(|| panic!("fake list app: no `good.name` in {command}")),
            cat: text("cat").unwrap_or_else(|| panic!("fake list app: no `good.cat` in {command}")),
            count: text("count"),
        };
        match (command["cmd"].as_str(), command["picked"].as_str()) {
            // The app replaces the first entry of that name in place, so a
            // repeated add is how an amount changes.
            (Some("add"), None) => match self.items.iter_mut().find(|e| e.name == entry.name) {
                Some(existing) => *existing = entry,
                None => self.items.push(entry),
            },
            // A `del` reaches the active list only, and is a no-op where the
            // name is not on it.
            (Some("del"), None) => {
                if let Some(at) = self.items.iter().position(|e| e.name == entry.name) {
                    self.items.remove(at);
                }
            }
            (None, Some("del")) => {
                self.picked.remove(&entry.name);
            }
            // The picked list is keyed by name: the entry of that name becomes
            // `good` as sent.
            (None, Some("add")) => {
                self.picked.insert(
                    entry.name,
                    PickedEntry {
                        cat: entry.cat,
                        count: entry.count,
                        picked_count: text("pickedCount"),
                    },
                );
            }
            // The ticked entry becomes `good` as sent, both amount fields
            // included.
            (None, Some("replace")) => {
                let old = command["name"].as_str().unwrap_or_else(|| {
                    panic!("fake list app: a `picked: replace` names no old item: {command}")
                });
                assert!(
                    self.picked.remove(old).is_some(),
                    "fake list app: a `picked: replace` of '{old}', which is not ticked; the app \
                     only sends one for an item in its picked list"
                );
                self.picked.insert(
                    entry.name,
                    PickedEntry {
                        cat: entry.cat,
                        count: entry.count,
                        picked_count: text("pickedCount"),
                    },
                );
            }
            _ => panic!("fake list app: a command this connection never sends: {command}"),
        }
    }
}

#[derive(Debug, Default)]
struct AppState {
    list: AppList,
    version: i64,
    /// Commands applied across every commit.
    applied: usize,
    /// The body a caching front serves to a pull without the freshness
    /// argument.
    cache: Option<serde_json::Value>,
}

/// The list app, behind the sidecar's real mappings.
#[derive(Debug, Default)]
pub struct FakeListApp {
    state: Mutex<AppState>,
}

impl FakeListApp {
    /// The app holding `list`, first pull at version 0.
    pub fn seeded(list: AppList) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(AppState {
                list,
                ..AppState::default()
            }),
        })
    }

    /// A person changes the list in the app.
    pub fn change(&self, person: impl FnOnce(&mut AppList)) {
        let mut state = self.state.lock().expect("the fake list");
        person(&mut state.list);
        state.version += 1;
    }

    pub fn list(&self) -> AppList {
        self.state.lock().expect("the fake list").list.clone()
    }

    pub fn commands_applied(&self) -> usize {
        self.state.lock().expect("the fake list").applied
    }
}

/// [`FakeListApp`] as the production peer reaches it: its calls answered by
/// the fake, its mappings by the shipped manual.
#[derive(Debug)]
pub struct FakeAppSurface {
    app: Arc<FakeListApp>,
    shipped: RestCallSurface,
    spec: ListSyncSpec,
}

impl FakeAppSurface {
    pub fn new(app: Arc<FakeListApp>) -> Self {
        Self {
            app,
            shipped: shipped_surface(),
            spec: list_sync_spec(),
        }
    }

    fn answer(&self, params: &CallToolRequestParam) -> serde_json::Value {
        let args = params.arguments.clone().unwrap_or_default();
        let mut state = self.app.state.lock().expect("the fake list");
        if params.name == self.spec.pull_tool.as_str() {
            // A caching front answers from what it holds unless the pull
            // carries the freshness argument the connection declares.
            if !args.contains_key("nocache")
                && let Some(cached) = &state.cache
            {
                return cached.clone();
            }
            let body = state.list.body(state.version);
            state.cache = Some(body.clone());
            body
        } else if params.name == self.spec.commit_tool.as_str() {
            let commands = args["commands"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("fake list app: a commit carries no `commands`: {args:?}")
                })
                .clone();
            for command in &commands {
                state.list.apply(command);
                state.applied += 1;
            }
            if !commands.is_empty() {
                state.version += 1;
            }
            serde_json::json!({
                "version": state.version,
                "pickedItemsVersion": state.version,
                RESPONSE_VERSION_KEY: state.version,
            })
        } else {
            panic!(
                "fake list app: a call to '{}', which the connection never makes",
                params.name
            )
        }
    }
}

#[async_trait::async_trait]
impl McpCallSurface for FakeAppSurface {
    async fn call_tool(
        &self,
        params: CallToolRequestParam,
    ) -> Result<CallToolResult, ServiceError> {
        let body = self.answer(&params);
        Ok(CallToolResult {
            content: vec![Content::text(body.to_string())],
            structured_content: Some(body),
            is_error: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParam,
    ) -> Result<ReadResourceResult, ServiceError> {
        panic!(
            "fake list app: the list connection reads no resource, got '{}'",
            params.uri
        )
    }

    fn map_response(
        &self,
        call_name: &str,
        response: &serde_json::Value,
    ) -> anyhow::Result<Vec<holon_core::file_format::TypedRowSet>> {
        self.shipped.map_response(call_name, response)
    }

    fn map_request(
        &self,
        call_name: &str,
        rows: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
        self.shipped.map_request(call_name, rows)
    }
}
