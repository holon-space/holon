//! @c4 container
//! @c4 layer Services
//! Pattern: MCP Server
//!
//! MCP server frontend (stdio + HTTP).

pub mod browser_relay;
pub mod dense_patch;
pub mod dense_projection;
pub mod describe_ui_expand;
pub mod describe_ui_geometry;
pub mod di;
pub mod key_chord_report;
pub mod keybindings;
pub mod resources;
pub mod self_check;
pub mod server;
pub mod tools;
pub mod types;

// Re-export commonly used types
pub use di::McpInjectorExt;
pub use di::McpServerConfig;
pub use di::McpServerHandle;
