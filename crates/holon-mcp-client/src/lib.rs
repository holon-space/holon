pub mod credential_store;
pub mod integration_config;
pub mod mcp_integration;
pub mod mcp_notification_handler;
pub mod mcp_provider;
pub mod mcp_resource_discovery;
pub mod mcp_schema_mapping;
pub mod mcp_sidecar;
pub mod mcp_sync_engine;
pub mod mcp_sync_strategy;

pub use integration_config::{IntegrationFileConfig, load_integration_configs};
pub use mcp_integration::{
    AuthMode, McpConnectionResult, McpIntegration, McpIntegrationConfig, McpTransport,
    PendingOAuthFlows, build_mcp_integration,
};
pub use mcp_notification_handler::{NotifyingClientHandler, ResourceUpdateReceiver};
pub use mcp_provider::{
    EntityFieldReader, McpOperationProvider, McpRunningService, connect_mcp, connect_mcp_child,
    connect_mcp_child_with_handler, connect_mcp_oauth, connect_mcp_oauth_with_handler,
    connect_mcp_with_handler,
};
pub use mcp_resource_discovery::{ResourceEntityMeta, parse_resource_template_meta};
pub use mcp_sidecar::McpSidecar;
pub use mcp_sync_engine::McpSyncEngine;
pub use mcp_sync_strategy::{FetchResult, ResourceSync, SyncStrategy, ToolSync};
