//! The **windowed** composed PBT slice — E4 of the `E2ESut`-dissolution endgame
//! (`docs/Testing/PbtCompositionBacklog.md`). It provides the one cap the headless
//! `frontend_slice` cannot: [`SutLayout`] geometry (real element bounds), backed by
//! a live gpui window's [`BoundsRegistry`].
//!
//! ## The Send / `!Send` split (the key E4 design point)
//!
//! The gpui `TestApp` that owns the window is `!Send`/single-threaded and must be
//! driven (pumped) on its owning thread — so it lives in the **test harness**
//! (`frontends/gpui/tests/`), never in a cap. But the *geometry data* it produces
//! is shareable: [`BoundsRegistry`] is `Send + Sync + Clone` (an `Arc<RwLock<…>>`),
//! and reads go through the abstract [`GeometryProvider`] port. So
//! [`GpuiWindowComponent`] holds only a `Box<dyn GeometryProvider>` clone and is an
//! ordinary `Send` component hosting an ordinary `async fn(&self)` [`SutLayout`]
//! cap on a `CapMap` — exactly like the Loro/SQL components. The harness pumps the
//! window to a fixed point (its realization-specific *settle*); the cap then reads
//! the freshly-committed bounds. The cap model is unchanged.
//!
//! [`SutLayout`]: holon_pbt_core::capabilities::SutLayout
//! [`GeometryProvider`]: holon_frontend::geometry::GeometryProvider
//! [`BoundsRegistry`]: https://docs — `holon_gpui::geometry::BoundsRegistry`
//! [`GpuiWindowComponent`]: components::GpuiWindowComponent

pub mod builders;
pub mod components;
pub mod seed;
