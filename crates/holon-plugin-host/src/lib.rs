//! @c4 component
//! @c4 layer Adapters
//! Pattern: Plugin Host
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-rows "typed row JSON Lines" "Rust"
//!
//! The generic wasm plugin host: a vault file format served by a `.wasm`
//! guest and a yaml sidecar instead of by a Rust crate.
//!
//! Three pieces. [`abi`] is the five core-wasm functions everything speaks.
//! [`PluginHost`] runs one guest under a fuel and a memory limit, keeping the
//! instance alive across files. [`PluginFormatAdapter`] turns the JSON Lines
//! the guest emits into a [`holon_core::file_format::FileFormatParseResult`],
//! refusing anything the sidecar did not declare.
//!
//! Guests are PURE FUNCTIONS: the host supplies an empty linker, so a guest
//! that reached for WASI, a clock or the network would fail to instantiate
//! rather than degrade.

pub mod abi;
mod adapter;
mod host;
mod params;
pub mod sidecar;

pub use adapter::PluginFormatAdapter;
pub use adapter::guest_parses;
pub use host::PluginError;
pub use host::PluginHost;
pub use host::PluginLimits;
pub use sidecar::BLOCK_SCOPE;
pub use sidecar::BUNDLED_PLUGINS;
pub use sidecar::BundledPlugin;
pub use sidecar::DOCUMENT_SCOPE;
pub use sidecar::GuestSource;
pub use sidecar::PluginFormat;
