//! The frontend (ViewModel/Renderer) composed PBT slice — a real **headless**
//! `FrontendSession` + `ReactiveEngine` over a Turso `BackendEngine`, built
//! through the production DI path (`holon_app::new_from_config_with_di`) but
//! windowless. The ViewModel/Renderer slice of the future `E2ESut` replacement.
//!
//! Its `HeadlessFrontendComponent` provides `SutRenderer` (over the real
//! CDC→watch→`interpret_pure` render path) + `SutBackend` (over `block_raw`), so
//! the renderer invariants *and* the block-tree catalog both run over it.

pub mod builders;
pub mod components;

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod navigation_pbt;

#[cfg(test)]
mod structural_pbt;

pub use builders::frontend_wide;
pub use components::HeadlessFrontendComponent;
