//! @c4 component
//! @c4 layer Adapters
//! Pattern: Adapter
//! @c4 uses holon "core orchestration" "Rust"
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//! @c4 uses holon-profiles "entity profile resolution" "Rust"
//!
//! Reusable MCP client: connects to MCP servers and exposes their tools as
//! `OperationProvider`s.

pub mod bundled_sidecars;
pub mod credential_store;
pub mod entity_mirror;
pub mod integration_config;
pub mod integration_state;
pub mod mcp_call_surface;
pub mod mcp_integration;
pub mod mcp_notification_handler;
pub mod mcp_provider;
pub mod mcp_resource_discovery;
pub mod mcp_schema_mapping;
pub mod mcp_sidecar;
pub mod mcp_sync_engine;
pub mod mcp_sync_strategy;
pub mod mcp_vtable;
pub mod oauth_bootstrap;
pub mod rest_oauth2;
pub mod rest_transport;
pub mod sync_freshness;
pub mod write_authorization;

pub use bundled_sidecars::BUNDLED_SIDECARS;
pub use bundled_sidecars::BundledSidecar;
pub use bundled_sidecars::SIDECAR_SCHEMA_VERSION;
pub use bundled_sidecars::bundled_sidecar;
pub use holon_core::SyncGate;
pub use integration_config::IgnoredReason;
pub use integration_config::IgnoredSidecar;
pub use integration_config::IntegrationFileConfig;
pub use integration_config::LoadedIntegrations;
pub use integration_config::SupersededSidecar;
pub use integration_config::load_integration_configs;
pub use integration_state::Configuration;
pub use integration_state::CredentialRef;
pub use integration_state::Credentials;
pub use integration_state::ENABLE_COMMAND;
pub use integration_state::INTEGRATION_STATE_SCHEMA_VERSION;
pub use integration_state::IntegrationConfigStore;
pub use integration_state::IntegrationState;
pub use mcp_integration::AuthMode;
pub use mcp_integration::McpConnectionResult;
pub use mcp_integration::McpIntegration;
pub use mcp_integration::McpIntegrationConfig;
pub use mcp_integration::McpTransport;
pub use mcp_integration::PendingOAuthFlows;
pub use mcp_integration::SyncEvent;
pub use mcp_integration::SyncLoopTuning;
pub use mcp_integration::assert_no_cross_sidecar_entity_collisions;
pub use mcp_integration::build_mcp_integration;
pub use mcp_integration::register_sidecar_entity_types;
pub use mcp_integration::spawn_sync_event_loop;
pub use mcp_notification_handler::NotifyingClientHandler;
pub use mcp_notification_handler::ResourceUpdateReceiver;
pub use mcp_provider::EntityFieldReader;
pub use mcp_provider::McpOperationProvider;
pub use mcp_provider::McpRunningService;
pub use mcp_provider::connect_mcp;
pub use mcp_provider::connect_mcp_child;
pub use mcp_provider::connect_mcp_child_with_handler;
pub use mcp_provider::connect_mcp_oauth;
pub use mcp_provider::connect_mcp_oauth_with_handler;
pub use mcp_provider::connect_mcp_with_handler;
pub use mcp_resource_discovery::ResourceEntityMeta;
pub use mcp_resource_discovery::parse_resource_template_meta;
pub use mcp_sidecar::McpSidecar;
pub use mcp_sidecar::OnceOnlyAuthorization;
pub use mcp_sidecar::SyncInterval;
pub use mcp_sidecar::ViewConfig;
pub use mcp_sidecar::canonical_entity_name;
pub use mcp_sync_engine::McpSyncEngine;
pub use mcp_sync_engine::VtableSubscription;
pub use mcp_sync_strategy::FetchResult;
pub use mcp_sync_strategy::Projection;
pub use mcp_sync_strategy::ResourceSync;
pub use mcp_sync_strategy::SyncStrategy;
pub use mcp_sync_strategy::ToolSync;
pub use mcp_vtable::McpForeignDataWrapper;
pub use mcp_vtable::VtableConfig;
pub use rest_oauth2::OAuth2TokenProvider;
pub use rest_oauth2::RestOAuth2Config;
pub use rest_transport::Pagination;
pub use rest_transport::ResponseFormat;
pub use rest_transport::RestAuth;
pub use rest_transport::RestCall;
pub use rest_transport::RestCallSurface;
pub use rest_transport::RestManual;
pub use sync_freshness::FreshnessPlan;
pub use sync_freshness::ProbedResourceCapabilities;
pub use sync_freshness::freshness_plan;
pub use write_authorization::PendingState;
pub use write_authorization::PendingWriteEvent;
pub use write_authorization::PendingWriteEventKind;
pub use write_authorization::PendingWriteStore;
pub use write_authorization::PendingWriteView;
pub use write_authorization::SharedPendingWrites;
pub use write_authorization::WriteAuthorizationPolicy;
pub use write_authorization::WriteDecision;
pub use write_authorization::WriteIntent;
