//! @c4 component
//! @c4 layer Composition
//! Pattern: Composition Root
//! @c4 uses holon "core orchestration" "Rust"
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-filesystem "filesystem ports" "Rust"
//! @c4 uses holon-frontend "frontend session abstraction" "Rust"
//! @c4 uses holon-mcp-client "MCP client providers" "Rust"
//! @c4 uses holon-orgmode "org-mode sync I/O" "Rust"
//! @c4 uses holon-loro "Loro CRDT backend & P2P sync" "Rust"
//! @c4 uses holon-profiles "entity profile resolution" "Rust"
//!
//! DI assembly crate (composition root) — owns every wiring that names concrete
//! backends: Turso/Loro/OrgMode modules, MCP integrations, and
//! `FrontendSession`.
//!
//! Owns all DI assembly that names concrete backends: the Turso
//! `BackendEngine` stack ([`wiring`]), the no-Turso Loro stack ([`no_turso`]),
//! config-driven session construction ([`session`]) and default-layout seeding
//! ([`seed`]). `holon-frontend` (the shared ViewModel) and `holon-orgmode`
//! stay storage-agnostic: they expose capability traits and
//! `FrontendSession::from_parts`; this crate supplies the implementations.
//!
//! Only frontends that assemble backends depend on this crate (gpui, tui,
//! mcp, integration tests) — never the storage-agnostic layers, and never
//! the wasm frontends (which bring their own wiring).

pub mod boot_error;
pub mod consolidator_epoch;
pub mod headless_builder_services;
pub mod integration_projection;
pub mod integrations_operations;
pub mod integrations_section;
pub mod integrations_settings;
pub mod loro_seams;
pub mod mcp_integrations;
pub mod move_guard;
pub mod no_turso;
#[cfg(test)]
mod no_turso_test;
pub mod ordered_block_crud;
pub mod rehome_entity;
pub mod seed;
pub mod session;
pub mod turso_seams;
pub mod wiring;

pub use boot_error::BootComponent;
pub use boot_error::BootError;
pub use boot_error::BootStage;
pub use headless_builder_services::HeadlessBuilderServices;
// Re-export the connector pending-write types (leases/read-write ruling,
// increment 4c) so the GPUI frontend — which depends on holon-app, not on the
// holon-mcp-client adapter directly — can resolve the shared store from DI and
// render/approve the pending-writes panel.
pub use holon_mcp_client::PendingState;
pub use holon_mcp_client::PendingWriteEvent;
pub use holon_mcp_client::PendingWriteEventKind;
pub use holon_mcp_client::PendingWriteStore;
pub use holon_mcp_client::PendingWriteView;
pub use holon_mcp_client::SharedPendingWrites;
pub use mcp_integrations::McpIntegrationRegistry;
pub use mcp_integrations::McpIntegrationsModule;
pub use no_turso::from_block_query_source;
pub use no_turso::register_block_query_frontend;
pub use seed::seed_default_layout;
pub use session::new_from_config;
pub use session::new_from_config_with_di;
pub use wiring::ConfigDir;
pub use wiring::FrontendInjectorExt;
pub use wiring::HolonFrontendModule;
pub use wiring::LockedKeys;
