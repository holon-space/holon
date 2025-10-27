//! @c4 component
//! @c4 layer Adapters
//! Pattern: Adapter
//! @c4 uses holon "core orchestration" "Rust"
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//!
//! Reusable MCP client: connects to MCP servers and exposes their tools as `OperationProvider`s.

pub mod credential_store;
pub mod integration_config;
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
pub mod sync_freshness;

pub use integration_config::{IntegrationFileConfig, load_integration_configs};
pub use mcp_integration::{
    AuthMode, McpConnectionResult, McpIntegration, McpIntegrationConfig, McpTransport,
    PendingOAuthFlows, SyncEvent, build_mcp_integration, spawn_sync_event_loop,
};
pub use mcp_notification_handler::{NotifyingClientHandler, ResourceUpdateReceiver};
pub use mcp_provider::{
    EntityFieldReader, McpOperationProvider, McpRunningService, connect_mcp, connect_mcp_child,
    connect_mcp_child_with_handler, connect_mcp_oauth, connect_mcp_oauth_with_handler,
    connect_mcp_with_handler,
};
pub use mcp_resource_discovery::{ResourceEntityMeta, parse_resource_template_meta};
pub use mcp_sidecar::{McpSidecar, SyncInterval, ViewConfig};
pub use mcp_sync_engine::{McpSyncEngine, VtableSubscription};
pub use mcp_sync_strategy::{FetchResult, ResourceSync, SyncStrategy, ToolSync};
pub use mcp_vtable::{McpForeignDataWrapper, VtableConfig};
pub use sync_freshness::{FreshnessPlan, ProbedResourceCapabilities, freshness_plan};
