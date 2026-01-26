//! System under test — pretend this is a real database / engine.
//!
//! In a real PBT this is the production code path. Here it's a tiny
//! `BTreeMap` so we can focus on the file-per-transition pattern.
//!
//! The SUT exposes **no per-transition methods**. Each transition owns
//! its SUT-side logic and mutates `Sut`'s public fields directly.
//! Adding a new transition therefore never touches this file.

use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub struct Sut {
    pub items: BTreeMap<u32, String>,
}
