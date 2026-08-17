//! The ADR-0024 door onto integration enablement: `integration.set_field`.
//!
//! Before this, the GPUI switch's mouse handler called
//! [`IntegrationsSettingsVm::set_enabled`] directly, so the frontend owned an
//! entity value and no other caller — MCP, a test driver, an agent — could
//! reach enablement at all. The operation makes the store's decision reachable
//! through the one action language, and it writes the AUTHORITY (the
//! `.state.toml` file); `IntegrationStateProjector` follows into
//! `integration_state` on the store's own signal.
//!
//! The value on the wire is a STATE WORD, not a bool: `state_toggle` cycles
//! through its `states` list and dispatches `Value::String(next)`
//! (`holon_frontend::operations::state_toggle_intent`). Parsing `"on"`/`"off"`
//! into a bool is therefore this provider's boundary job, and anything else is
//! refused rather than coerced.

use std::sync::Arc;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::OperationParam;
use holon_api::TypeHint;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;
use holon_mcp_client::IntegrationConfigStore;

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

/// The two words the toggle cycles between, in `states` order.
pub const STATE_OFF: &str = "off";
pub const STATE_ON: &str = "on";

/// The descriptor set `create_profile_resolver` buckets by entity, and thence
/// what `find_set_field_op` finds on an `integration:` row.
pub fn integration_operation_descriptors() -> Vec<OperationDescriptor> {
    vec![OperationDescriptor {
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
                type_hint: TypeHint::OneOf {
                    values: vec![
                        Value::String(STATE_OFF.to_string()),
                        Value::String(STATE_ON.to_string()),
                    ],
                },
                description: "The state word to store: 'on' or 'off'".to_string(),
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
    }]
}

/// Writes enablement through the store that owns it.
pub struct IntegrationsOperationProvider {
    vm: IntegrationsSettingsVm,
    store: Arc<IntegrationConfigStore>,
}

impl IntegrationsOperationProvider {
    pub fn new(store: Arc<IntegrationConfigStore>) -> Self {
        Self {
            vm: IntegrationsSettingsVm::new(store.clone()),
            store,
        }
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

/// The state word as a stored decision.
///
/// Parse, don't validate: the wire carries one of two words and everything
/// else is a refusal here, so no downstream caller re-checks it.
fn parse_state_word(value: Option<&Value>) -> Result<bool> {
    match value {
        Some(Value::String(s)) if s == STATE_ON => Ok(true),
        Some(Value::String(s)) if s == STATE_OFF => Ok(false),
        other => Err(format!(
            "IntegrationsOperationProvider: 'value' must be {STATE_ON:?} or {STATE_OFF:?}, got \
             {other:?}"
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
        if op_name != "set_field" {
            return Err(format!(
                "IntegrationsOperationProvider: '{ENTITY_NAME}' exposes only 'set_field', got \
                 '{op_name}'"
            )
            .into());
        }

        let raw_id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| "IntegrationsOperationProvider: missing required parameter 'id'")?;
        let provider = self.provider_of(raw_id)?;

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

        let enabled = parse_state_word(params.get("value"))?;
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

        let word = |on: bool| Value::String(if on { STATE_ON } else { STATE_OFF }.to_string());
        Ok(OperationResult::declared_irreversible(
            vec![holon_core::FieldDelta::new(
                integration_row_id(provider),
                ENABLED_FIELD,
                word(was),
                word(enabled),
            )],
            "an integration switch is a settings decision, outside the content undo stack",
        ))
    }
}
