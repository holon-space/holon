//! The ADR-0024 doors onto an integration: `set_field`, `begin_oauth` and
//! `open_default_view`.
//!
//! Enablement is reachable only through this door, so MCP, a test driver and an
//! agent all take the path the switch takes. `set_field` writes the AUTHORITY
//! (the `.state.toml` file); `IntegrationStateProjector` follows into
//! `integration_state` on the store's own signal.
//!
//! The value on the wire is a BOOL: the row's toggle is bool-bound, so
//! `state_toggle_intent_bool` dispatches `Value::Boolean`. A state word is not
//! a spelling of it.

use std::sync::Arc;

use async_trait::async_trait;
use fluxdi::Injector;
use holon::navigation::NavigationProvider;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OperationDescriptor;
use holon_api::OperationParam;
use holon_api::TypeHint;
use holon_api::Value;
use holon_api::spawner::Spawner;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;
use holon_filesystem::sync_ports::BlockReader;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::integration_config::provider_content;
use holon_mcp_client::oauth_bootstrap::BrowserOpener;
use holon_mcp_client::oauth_bootstrap::DEFAULT_CONSENT_TIMEOUT;

use crate::integration_projection::integration_row_id;
use crate::integrations_settings::IntegrationsSettingsVm;

/// The entity whose rows are integrations — the scheme of `integration:<p>`.
pub const ENTITY_NAME: &str = "integration";
pub const SHORT_NAME: &str = "integration";

/// The ONE field this entity exposes to `set_field`.
///
/// `configuration` is deliberately absent: the consent flow owns that axis and
/// has its own door, and the store's own `set_enabled` doc states the two are
/// independent. Letting one op write both would let a mis-typed toggle discard
/// a consent the user cannot always grant twice.
pub const ENABLED_FIELD: &str = "enabled";

/// The one-time consent flow, as an operation.
pub const BEGIN_OAUTH: &str = "begin_oauth";

/// Show this integration's view page in the main panel.
pub const OPEN_DEFAULT_VIEW: &str = "open_default_view";

/// The descriptor set `create_profile_resolver` buckets by entity, and thence
/// what `find_set_field_op` finds on an `integration:` row.
pub fn integration_operation_descriptors() -> Vec<OperationDescriptor> {
    vec![
        set_field_descriptor(),
        begin_oauth_descriptor(),
        open_default_view_descriptor(),
    ]
}

/// One param, for the reason [`begin_oauth_descriptor`] gives: the sidebar row
/// dispatches this on a click, and a second required param would make that
/// click a crash.
fn open_default_view_descriptor() -> OperationDescriptor {
    OperationDescriptor {
        entity_name: ENTITY_NAME.into(),
        entity_short_name: SHORT_NAME.to_string(),
        id_column: "id".to_string(),
        name: OPEN_DEFAULT_VIEW.to_string(),
        display_name: "Open".to_string(),
        description: "Show this integration's view page in the main panel".to_string(),
        required_params: vec![OperationParam {
            name: "id".to_string(),
            type_hint: TypeHint::String,
            description: "Integration row id, 'integration:<provider>'".to_string(),
        }],
        affected_fields: vec![],
        param_mappings: vec![],
        target_scope: holon_api::TargetScope::Global,
        boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
        menu_exposure: holon_api::MenuExposure::NotListed {
            surface: holon_api::NonMenuSurface::PointerGesture,
        },
        trigger: None,
        bound_params: Default::default(),
        marking_delta: holon_api::marking::MarkingDelta::Undeclared,
        // No guard on the missing-view case. A guard would make the row
        // silently unclickable, which is the same nothing-happens the loud
        // refusal in `open_default_view` exists to replace.
        guard: holon_api::pattern::OpGuard::None,
        // `Undeclared`, which fails closed: this op resolves the view page from
        // the SIDECAR CONFIG STORE on disk, and an `ArcPlace` names a
        // `relation.field` cell — there is none for a file. Declaring
        // `reads: [integration.default_view]` pointed at the mirror COLUMN,
        // which is presentation-only: writing that cell changes nothing about
        // where this op navigates, so the declaration told a simulator the one
        // thing that is not true. Refusing to simulate beats simulating wrong.
        //
        // What is lost by not saying `emits: []`: nothing on this entity moves
        // either — the focus is written by `navigation.focus`, which declares
        // its own arcs on its own relation.
        arcs: holon_api::arcs::TransitionArcs::Undeclared,
    }
}

fn set_field_descriptor() -> OperationDescriptor {
    OperationDescriptor {
        entity_name: ENTITY_NAME.into(),
        entity_short_name: SHORT_NAME.to_string(),
        id_column: "id".to_string(),
        name: "set_field".to_string(),
        display_name: "Switch integration".to_string(),
        description: "Switch an integration on or off (the stored decision; takes effect at the \
                      next launch)"
            .to_string(),
        required_params: vec![
            OperationParam {
                name: "id".to_string(),
                type_hint: TypeHint::String,
                description: "Integration row id, 'integration:<provider>'".to_string(),
            },
            OperationParam {
                name: "field".to_string(),
                type_hint: TypeHint::OneOf {
                    values: vec![Value::String(ENABLED_FIELD.to_string())],
                },
                description: "The field to write; only 'enabled' is writable here".to_string(),
            },
            OperationParam {
                name: "value".to_string(),
                type_hint: TypeHint::Bool,
                description: "The decision to store".to_string(),
            },
        ],
        affected_fields: vec![ENABLED_FIELD.to_string()],
        param_mappings: vec![],
        target_scope: holon_api::TargetScope::Global,
        boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
        menu_exposure: holon_api::MenuExposure::NotListed {
            surface: holon_api::NonMenuSurface::PointerGesture,
        },
        trigger: None,
        bound_params: Default::default(),
        // No relational precondition: every bundled provider is switchable in
        // either direction at any time, and the refusals this op does make
        // (unknown provider, wrong field, wrong word) are parameter validity,
        // which ADR 0031 puts in typed params rather than in a guard.
        marking_delta: holon_api::marking::MarkingDelta::Undeclared,
        guard: holon_api::pattern::OpGuard::None,
        arcs: holon_api::arcs::TransitionArcs::Declared {
            // Read-modify-write: the configuration axis is carried through
            // untouched, so it is read even though it is never written.
            reads: vec![
                holon_api::arcs::ArcPlace::new(ENTITY_NAME, ENABLED_FIELD),
                holon_api::arcs::ArcPlace::new(ENTITY_NAME, "config_status"),
            ],
            emits: vec![
                holon_api::arcs::ArcEmit::Writes(holon_api::arcs::ArcPlace::new(
                    ENTITY_NAME,
                    ENABLED_FIELD,
                )),
                holon_api::arcs::ArcEmit::Excluded {
                    place: holon_api::arcs::ArcPlace::new(ENTITY_NAME, "updated_at"),
                    reason: "IntegrationStateProjector stamps it when it mirrors the store; this \
                             op writes the state file, not the table"
                        .to_string(),
                },
            ],
        },
    }
}

/// `begin_oauth` takes ONE param on purpose: `present_op` dispatches an
/// op_button immediately when nothing is missing and panics otherwise, so a
/// second required param would turn a click into a crash.
fn begin_oauth_descriptor() -> OperationDescriptor {
    OperationDescriptor {
        entity_name: ENTITY_NAME.into(),
        entity_short_name: SHORT_NAME.to_string(),
        id_column: "id".to_string(),
        name: BEGIN_OAUTH.to_string(),
        display_name: "Configure…".to_string(),
        description: "Run the one-time consent flow: open the provider's authorization page and \
                      wait for the redirect"
            .to_string(),
        required_params: vec![OperationParam {
            name: "id".to_string(),
            type_hint: TypeHint::String,
            description: "Integration row id, 'integration:<provider>'".to_string(),
        }],
        affected_fields: vec![],
        param_mappings: vec![],
        target_scope: holon_api::TargetScope::Global,
        boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
        menu_exposure: holon_api::MenuExposure::NotListed {
            surface: holon_api::NonMenuSurface::PointerGesture,
        },
        trigger: None,
        bound_params: Default::default(),
        // Offer the flow only where it can run. The three literals are the
        // PROJECTED values: `config_status_value` lowercases the display enum,
        // and `configure_progress` is empty exactly while no flow is running.
        marking_delta: holon_api::marking::MarkingDelta::Undeclared,
        guard: holon_api::pattern::OpGuard::parse(
            "integration.config_status == \"unconfigured\" and integration.configurable == 1 and \
             integration.configure_progress == \"\"",
        )
        .unwrap_or_else(|e| panic!("the begin_oauth guard must parse: {e}")),
        arcs: holon_api::arcs::TransitionArcs::Declared {
            reads: vec![
                holon_api::arcs::ArcPlace::new(ENTITY_NAME, "config_status"),
                holon_api::arcs::ArcPlace::new(ENTITY_NAME, "configurable"),
            ],
            emits: vec![holon_api::arcs::ArcEmit::Excluded {
                place: holon_api::arcs::ArcPlace::new(ENTITY_NAME, "config_status"),
                reason: "the consent flow writes it asynchronously; this op only starts the flow"
                    .to_string(),
            }],
        },
    }
}

/// Writes enablement through the store that owns it, and starts consent flows
/// on the view model whose progress cells the mirror projects.
pub struct IntegrationsOperationProvider {
    vm: Arc<IntegrationsSettingsVm>,
    store: Arc<IntegrationConfigStore>,
    browser: Arc<dyn BrowserOpener>,
    spawner: Arc<dyn Spawner>,
    /// Resolved lazily, never at construction: this provider is registered into
    /// the same `dyn OperationProvider` set as the navigation provider it
    /// dispatches to, so resolving it at wiring time would be a cycle.
    injector: Injector,
}

impl IntegrationsOperationProvider {
    pub fn new(
        store: Arc<IntegrationConfigStore>,
        vm: Arc<IntegrationsSettingsVm>,
        browser: Arc<dyn BrowserOpener>,
        spawner: Arc<dyn Spawner>,
        injector: Injector,
    ) -> Self {
        Self {
            vm,
            store,
            browser,
            spawner,
            injector,
        }
    }

    /// Focus `provider`'s view page in the main panel.
    ///
    /// Resolves the page from the SIDECAR CONFIG STORE. The mirror's
    /// `integration_state.default_view` column is presentation-only — a
    /// projection of the same sidecar value for surfaces that can only read
    /// SQL — so writing that column does not change where this op navigates.
    ///
    /// Three loud refusals, because every one of them would otherwise present
    /// as a click that does nothing: no `default_view` in the sidecar, a
    /// sidecar that will not read, and a `default_view` naming a block the
    /// store does not hold.
    async fn open_default_view(&self, provider: &'static str) -> Result<OperationResult> {
        let content = provider_content(self.store.dir(), provider).map_err(|e| {
            format!(
                "IntegrationsOperationProvider: could not read '{provider}'s sidecar to find its \
                 default view: {e:#}"
            )
        })?;
        let Some(bare_id) = content.config.default_view else {
            return Err(format!(
                "IntegrationsOperationProvider: '{provider}' declares no `default_view`, so \
                 '{OPEN_DEFAULT_VIEW}' has nothing to open. Add `default_view: <page-block-id>` to \
                 assets/integrations/{provider}.yaml naming the page this integration should show."
            )
            .into());
        };

        // Org files carry bare ids; the scheme is added at the boundary, and
        // navigation.focus refuses a target that has none.
        let target = EntityUri::block(&bare_id);
        let present = self
            .injector
            .resolve_async::<dyn BlockReader>()
            .await
            .get_block_authoritative(&target)
            .await
            .map_err(|e| {
                format!(
                    "IntegrationsOperationProvider: looking up '{provider}'s default view \
                     `{target}`: {e}"
                )
            })?;
        if present.is_none() {
            return Err(format!(
                "IntegrationsOperationProvider: '{provider}' names `{bare_id}` as its \
                 `default_view`, but no block with that id exists. The page the sidecar points at \
                 has to be authored (assets/default/index.org) before the row can open it."
            )
            .into());
        }

        let mut params = StorageEntity::new();
        params.insert("region".into(), Value::String("main".to_string()));
        params.insert(
            "block_id".into(),
            Value::String(target.as_str().to_string()),
        );
        // The rule below bans this call as a way to DRIVE navigation in place
        // of the keyboard pipeline. This is production op composition:
        // `open_default_view` is the door, and it reaches the focus through the
        // navigation provider's own operation rather than by writing the
        // navigation tables itself.
        self.injector
            .resolve_async::<NavigationProvider>()
            .await
            // ALLOW(navigation_execute_op): production op composition, not a test driver
            .execute_operation(&EntityName::new("navigation"), "focus", params)
            .await
    }

    /// Start `provider`'s consent flow and return.
    ///
    /// The flow waits on a human in a browser for up to
    /// [`DEFAULT_CONSENT_TIMEOUT`], so awaiting it here would hold the
    /// dispatcher for minutes. Its outcome is observable on the row through the
    /// view model's progress cell, which the mirror projects.
    fn start_consent_flow(&self, provider: &'static str) {
        let vm = self.vm.clone();
        let browser = self.browser.clone();
        self.spawner.spawn(Box::pin(async move {
            if let Err(e) = vm
                .configure(provider, browser.as_ref(), DEFAULT_CONSENT_TIMEOUT)
                .await
            {
                tracing::warn!(
                    provider,
                    "the consent flow for '{provider}' failed: {e:#} (the row's \
                     configure_progress carries the same reason)"
                );
            }
        }));
    }

    /// The provider `raw` addresses, or why it addresses none.
    ///
    /// Both refusals name what arrived AND what would have been accepted: an
    /// id that reaches here at all came from a rendered row, so a mismatch is a
    /// wiring bug somebody has to locate, not user error.
    fn provider_of(&self, raw: &str) -> Result<&'static str> {
        let Some((scheme, provider)) = raw.split_once(':') else {
            return Err(format!(
                "IntegrationsOperationProvider: id {raw:?} is not an entity URI; expected \
                 'integration:<provider>'"
            )
            .into());
        };
        if scheme != ENTITY_NAME {
            return Err(format!(
                "IntegrationsOperationProvider: id {raw:?} addresses entity {scheme:?}, not \
                 '{ENTITY_NAME}'"
            )
            .into());
        }
        self.store
            .providers()
            .into_iter()
            .find(|p| *p == provider)
            .ok_or_else(|| {
                format!(
                    "IntegrationsOperationProvider: {provider:?} is not an integration this build \
                     bundles; bundled providers are {:?}",
                    self.store.providers()
                )
                .into()
            })
    }
}

/// The decision on the wire.
///
/// Parse, don't validate: the wire carries a bool and everything else is a
/// refusal here, so no downstream caller re-checks it.
fn parse_decision(value: Option<&Value>) -> Result<bool> {
    match value {
        Some(Value::Boolean(b)) => Ok(*b),
        other => Err(format!(
            "IntegrationsOperationProvider: 'value' must be a boolean, got {other:?}"
        )
        .into()),
    }
}

#[async_trait]
impl OperationProvider for IntegrationsOperationProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        integration_operation_descriptors()
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        if entity_name != ENTITY_NAME {
            return Err(format!(
                "IntegrationsOperationProvider: expected entity '{ENTITY_NAME}', got \
                 '{entity_name}'"
            )
            .into());
        }
        let raw_id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| "IntegrationsOperationProvider: missing required parameter 'id'")?;
        let provider = self.provider_of(raw_id)?;

        if op_name == BEGIN_OAUTH {
            self.start_consent_flow(provider);
            return Ok(OperationResult::declared_irreversible(
                vec![],
                "a consent grant lives with the provider, not in the content undo stack",
            ));
        }
        if op_name == OPEN_DEFAULT_VIEW {
            return self.open_default_view(provider).await;
        }
        if op_name != "set_field" {
            return Err(format!(
                "IntegrationsOperationProvider: '{ENTITY_NAME}' exposes 'set_field', \
                 '{BEGIN_OAUTH}' and '{OPEN_DEFAULT_VIEW}', got '{op_name}'"
            )
            .into());
        }

        let field = params
            .get("field")
            .and_then(|v| v.as_string())
            .ok_or_else(|| "IntegrationsOperationProvider: missing required parameter 'field'")?;
        if field != ENABLED_FIELD {
            return Err(format!(
                "IntegrationsOperationProvider: only '{ENABLED_FIELD}' is writable on an \
                 integration, got {field:?}. The configuration axis is written by the consent \
                 flow, not by this operation."
            )
            .into());
        }

        let enabled = parse_decision(params.get("value"))?;
        let was = self
            .store
            .get(provider)
            .map_err(|e| format!("IntegrationsOperationProvider: reading '{provider}': {e:#}"))?
            .enabled;
        self.vm.set_enabled(provider, enabled).map_err(|e| {
            format!(
                "IntegrationsOperationProvider: could not store the decision for '{provider}': \
                 {e:#}"
            )
        })?;

        Ok(OperationResult::declared_irreversible(
            vec![holon_core::FieldDelta::new(
                integration_row_id(provider),
                ENABLED_FIELD,
                Value::Boolean(was),
                Value::Boolean(enabled),
            )],
            "an integration switch is a settings decision, outside the content undo stack",
        ))
    }
}
