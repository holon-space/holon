//! Scale-soak vault seeder (env-gated, zero-cost when off).
//!
//! The keystone (`general_e2e_composed_pbt`) boots a 3-block focus doc —
//! vault-scale behaviour (the projection/consolidator latency cliff `pass_ms ≈
//! 11.3 + 0.221×blocks`, CDC cost at 5–10k blocks, RSS growth) never manifests,
//! so Martin discovers it by hand. This module inflates ONLY the SUT boot with
//! extra org **doc files** — realistic synthetic pages (deep trees, tasks,
//! links, unicode). They land in the SUT store as separate documents, so
//! `boot_and_seed_wide`'s scaffold math folds their ids into the oracle as seed
//! blocks (`block_documents[id]=no_parent`, exactly like `block:journals`
//! and the boot layout) — they drop out of the block comparison and the
//! focus-root candidate set, so the invariant catalog stays green while every
//! action still pays the whole-vault projection / CDC / consolidator cost.
//!
//! Everything here is a pure function of `HOLON_SOAK_SEED_BLOCKS`
//! (deterministic — same vault every run). When it is unset or `0`,
//! [`soak_org_files`] returns empty and the keystone boots exactly as before.

use std::time::Duration;

use super::reseed_observer::ReseedLeakReason;

/// What the soak reseed-reproduction rung should assert about steady-state
/// full-reseed leaks. Parse-don't-validate target for
/// `HOLON_SOAK_RESEED_EXPECT` (`None` = unset = rung skips).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoakReseedExpect {
    /// Assert the reproduction fires: at least one steady-state leak of the
    /// TARGET reason across the fixed seeds (else fail loud with the full
    /// per-reason summary).
    Reproduce,
    /// Assert the leak is GONE: zero steady-state leaks of any reason (the
    /// guard after the reseed Inc 1-4 fixes land).
    Zero,
}

/// Parse `HOLON_SOAK_RESEED_EXPECT` at the boundary. Unset or empty ⇒ `None`
/// (rung skips). Any other non-empty value fails loud — no silent default.
pub fn soak_reseed_expect() -> Option<SoakReseedExpect> {
    match std::env::var("HOLON_SOAK_RESEED_EXPECT") {
        Err(_) => None,
        Ok(s) if s.trim().is_empty() => None,
        Ok(s) => match s.trim() {
            "reproduce" => Some(SoakReseedExpect::Reproduce),
            "zero" => Some(SoakReseedExpect::Zero),
            other => panic!(
                "HOLON_SOAK_RESEED_EXPECT must be 'reproduce' or 'zero' (or unset), got {other:?}"
            ),
        },
    }
}

/// Parse `HOLON_SOAK_ASSERT_P95_MS` at the boundary: `Some(ms)` hard-asserts
/// the observed p95 wall-clock is under `ms`; unset/empty ⇒ `None` (p95 is an
/// observation only). Fails loud on non-numeric garbage.
pub fn soak_assert_p95_ms() -> Option<u64> {
    match std::env::var("HOLON_SOAK_ASSERT_P95_MS") {
        Err(_) => None,
        Ok(s) if s.trim().is_empty() => None,
        Ok(s) => Some(s.trim().parse().unwrap_or_else(|e| {
            panic!("HOLON_SOAK_ASSERT_P95_MS must be a u64 millisecond value, got {s:?}: {e}")
        })),
    }
}

/// Parse `HOLON_SOAK_RESEED_REASON` at the boundary: the TARGET reason the
/// `Reproduce` mode asserts fires. Unset/empty ⇒ the default lever
/// [`ReseedLeakReason::EmptyPendingMovedFrontier`]. Fails loud on an unknown
/// reason.
pub fn soak_reseed_reason() -> ReseedLeakReason {
    match std::env::var("HOLON_SOAK_RESEED_REASON") {
        Err(_) => ReseedLeakReason::EmptyPendingMovedFrontier,
        Ok(s) if s.trim().is_empty() => ReseedLeakReason::EmptyPendingMovedFrontier,
        Ok(s) => ReseedLeakReason::parse(s.trim())
            .unwrap_or_else(|e| panic!("HOLON_SOAK_RESEED_REASON invalid: {e}")),
    }
}

/// What the navigation-latency soak rung (`soak_nav_latency`) should assert.
/// Parse-don't-validate target for `HOLON_SOAK_NAV` (`None` = unset = rung
/// skips). Sibling of [`SoakReseedExpect`], for the ORTHOGONAL latency axis:
/// the cold focus-descendant matview materialization cliff (BugFunnel row 78 —
/// `navigate focus` = 2674ms @4000 blocks), not the CRDT reseed leak.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoakNavExpect {
    /// RED-FIRST proof: assert the cold-matview navigation cliff REPRODUCES —
    /// first-visit navigations to a fresh focus root are dramatically slower
    /// than warm revisits AND breach a generous absolute floor. Fails loud as a
    /// HEADLINE FINDING if the cliff does NOT reproduce at scale (e.g. the
    /// harness pre-warmed the descendant matviews at boot ⇒ make it colder to
    /// match prod, per CLAUDE.md's "make E2E and prod more similar" rule).
    Reproduce,
    /// Post-fix guard: assert EVERY navigation (cold included) settles under
    /// the [`soak_nav_budget_ms`] wall-clock budget.
    Zero,
}

/// Parse `HOLON_SOAK_NAV` at the boundary. Unset or empty ⇒ `None` (rung
/// skips). Any other non-empty value fails loud — no silent default.
pub fn soak_nav_expect() -> Option<SoakNavExpect> {
    match std::env::var("HOLON_SOAK_NAV") {
        Err(_) => None,
        Ok(s) if s.trim().is_empty() => None,
        Ok(s) => match s.trim() {
            "reproduce" => Some(SoakNavExpect::Reproduce),
            "zero" => Some(SoakNavExpect::Zero),
            other => {
                panic!("HOLON_SOAK_NAV must be 'reproduce' or 'zero' (or unset), got {other:?}")
            }
        },
    }
}

/// The per-navigation wall-clock budget (ms) the `zero`-mode guard and the
/// opt-in [`soak_nav_hard_assert`] enforce against p95. Default `2000`
/// (generous, sized for a RELEASE-profile run) so CI never flakes on a slow
/// host. Override with `HOLON_SOAK_NAV_BUDGET_MS`; fails loud on garbage.
pub fn soak_nav_budget_ms() -> u64 {
    match std::env::var("HOLON_SOAK_NAV_BUDGET_MS") {
        Err(_) => 2000,
        Ok(s) if s.trim().is_empty() => 2000,
        Ok(s) => s.trim().parse().unwrap_or_else(|e| {
            panic!("HOLON_SOAK_NAV_BUDGET_MS must be a u64 millisecond value, got {s:?}: {e}")
        }),
    }
}

/// Whether the opt-in hard p95<budget assertion is on
/// (`HOLON_SOAK_NAV_ASSERT=1` or `=true`). Off by default so wall-clock timing
/// (host-dependent) is an OBSERVATION unless a run explicitly opts into
/// enforcing it — mirrors the reseed rung's `HOLON_SOAK_ASSERT_P95_MS`.
pub fn soak_nav_hard_assert() -> bool {
    matches!(
        std::env::var("HOLON_SOAK_NAV_ASSERT").ok().as_deref(),
        Some("1") | Some("true")
    )
}

/// The seed TREE SHAPE — the depth-vs-count lever for the nav-latency probe.
/// Parse-don't-validate target for `HOLON_SOAK_SHAPE` (default `Wide`).
///
/// The recursive focus-descendant matview's cycle guard is an O(path-length)
/// string scan PER recursion step (`','||visited||',' NOT LIKE
/// '%,'||id||',%'`), so its cost grows ~quadratically with tree DEPTH per node
/// — a lever that a shallow-wide seed (the default) cannot pull. Prod's ~11.9s
/// cliff was at only 1038 REAL blocks (deep outline chains), vs sub-second at
/// 2000 shallow synthetic blocks; DEPTH, not count, is the suspected driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoakShape {
    /// Shallow (depth ≤ 4), wide fan-out — `HOLON_SOAK_BLOCKS_PER_DOC` blocks
    /// per page. The original soak shape; recursive descent is trivial.
    Wide,
    /// A few LINEAR chains of depth `HOLON_SOAK_DEPTH` (default 200), one page
    /// per chain — mirrors a real deep outline. Each chain is `page → b0 → b1 →
    /// … → b{L-1}` (heading level `j+1`), so a navigate-to-page materializes an
    /// L-deep recursion that stresses the cycle-guard string scan.
    Deep,
    /// Half wide, half deep (disjoint contiguous page indices).
    Mixed,
}

/// Parse `HOLON_SOAK_SHAPE` at the boundary. Unset/empty ⇒ `Wide`. Fails loud
/// on any other value.
pub fn soak_shape() -> SoakShape {
    match std::env::var("HOLON_SOAK_SHAPE") {
        Err(_) => SoakShape::Wide,
        Ok(s) if s.trim().is_empty() => SoakShape::Wide,
        Ok(s) => match s.trim() {
            "wide" => SoakShape::Wide,
            "deep" => SoakShape::Deep,
            "mixed" => SoakShape::Mixed,
            other => {
                panic!(
                    "HOLON_SOAK_SHAPE must be 'wide' | 'deep' | 'mixed' (or unset), got {other:?}"
                )
            }
        },
    }
}

/// Per-chain nesting depth for the `deep`/`mixed` shapes (org heading levels).
/// `HOLON_SOAK_DEPTH` (default `200`) — deep enough to make the cycle-guard
/// string scan's ~quadratic-in-depth cost dominate.
fn soak_depth() -> usize {
    std::env::var("HOLON_SOAK_DEPTH")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or(200)
}

/// The deterministic set of navigable focus roots the nav rung drives: the
/// synthetic soak DOC pages `block:soak-doc-0 .. block:soak-doc-{n-1}`. Each is
/// a real ingested page in the SUT store (`#+ID: soak-doc-K` — see
/// [`soak_org_files`]) that renders in the LeftSidebar, so a first-visit
/// `NavigateFocus` to it registers a fresh focus-descendant watch = the cold
/// materialization under test. The count is SHAPE-dependent (wide: many small
/// pages; deep: few deep-chain pages). `0` when the soak is off.
pub fn soak_doc_count() -> usize {
    let total = soak_block_count();
    if total == 0 {
        return 0;
    }
    match soak_shape() {
        SoakShape::Wide => total.div_ceil(blocks_per_doc()),
        SoakShape::Deep => total.div_ceil(soak_depth()),
        SoakShape::Mixed => {
            let deep_total = total / 2;
            let wide_total = total - deep_total;
            wide_total.div_ceil(blocks_per_doc()) + deep_total.div_ceil(soak_depth())
        }
    }
}

/// Total number of extra vault blocks to seed. `HOLON_SOAK_SEED_BLOCKS`
/// (default `0` — soak off, keystone behaviour unchanged).
pub fn soak_block_count() -> usize {
    std::env::var("HOLON_SOAK_SEED_BLOCKS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Blocks per synthetic doc file. `HOLON_SOAK_BLOCKS_PER_DOC` (default `200`) —
/// a real vault is many pages, so the seed is split across `ceil(N / this)` doc
/// files.
fn blocks_per_doc() -> usize {
    std::env::var("HOLON_SOAK_BLOCKS_PER_DOC")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or(200)
}

/// The per-action convergence-settle budget. `HOLON_SOAK_SETTLE_MS` (default =
/// the keystone's `wide_e2e::SETTLE` = 150ms). MUST be raised for a scale run:
/// at 5–10k blocks the projection/CDC drain far exceeds 150ms, and a too-small
/// budget makes `converge_projections` return BEFORE quiescence — which both
/// risks stale-read invariant flakes AND caps the measured `action_total` below
/// the true latency (it would hide the very cliff the soak exists to quantify).
pub fn soak_settle() -> Duration {
    match std::env::var("HOLON_SOAK_SETTLE_MS")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        Some(ms) => Duration::from_millis(ms),
        None => super::wide_e2e::SETTLE,
    }
}

/// Deterministic pseudo-random stream (splitmix64) — variety without a proptest
/// RNG, so the vault is byte-identical across runs at a given size.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A pool of unicode-rich content fragments, exercising multi-byte parsing at
/// scale.
const FRAGMENTS: &[&str] = &[
    "cafe resume naive — grapheme edge cases",
    "日本語のテキスト — CJK wide chars",
    "Ελληνικά κείμενο — Greek",
    "Кириллица — Cyrillic sample",
    "emoji run 🚀🔥🧠📚✅ — astral plane",
    "mixed العربية RTL segment",
    "math ∑∫∂∇ ≠ ≤ ≥ ∞ — symbols",
    "plain ascii note for baseline width",
];

/// Generate the synthetic vault as org doc files: `Vec<(filename, content)>`.
/// Empty when the soak is off. SHAPE-selected (`HOLON_SOAK_SHAPE`): `wide` =
/// many shallow pages; `deep` = a few deep chains; `mixed` = both (disjoint,
/// contiguous `soak-doc-K` indices so the nav rung's root set stays dense).
pub fn soak_org_files() -> Vec<(String, String)> {
    let total = soak_block_count();
    if total == 0 {
        return Vec::new();
    }
    match soak_shape() {
        SoakShape::Wide => wide_docs(total, 0).0,
        SoakShape::Deep => deep_docs(total, 0).0,
        SoakShape::Mixed => {
            let deep_total = total / 2;
            let wide_total = total - deep_total;
            let (mut files, next_k) = wide_docs(wide_total, 0);
            let (deep, _) = deep_docs(deep_total, next_k);
            files.extend(deep);
            files
        }
    }
}

/// Shallow-wide docs (the original shape): `ceil(total / blocks_per_doc)`
/// pages, each a depth-≤4 random-walk tree. Returns the files and the next free
/// page index (for `mixed`).
fn wide_docs(total: usize, start_k: usize) -> (Vec<(String, String)>, usize) {
    let per_doc = blocks_per_doc();
    let doc_count = total.div_ceil(per_doc);
    let mut files = Vec::with_capacity(doc_count);
    let mut emitted = 0usize;

    for i in 0..doc_count {
        let k = start_k + i;
        let mut body = format!("#+ID: soak-doc-{k}\n#+TITLE: Soak Page {k}\n");
        let mut rng = 0x5EED_0000_0000_0000u64 ^ (k as u64);
        // Current heading depth (org `*` count). Random-walk in 1..=4, biased to stay
        // shallow, so the tree is genuinely nested (parents + descendants) not flat.
        let mut depth: usize = 1;
        for j in 0..per_doc {
            if emitted >= total {
                break;
            }
            emitted += 1;
            let r = splitmix(&mut rng);
            // Walk depth: 40% deeper, 30% shallower, 30% same — clamped to 1..=4, and
            // never deeper than one below the previous (valid org nesting).
            depth = match r % 10 {
                0..=3 => (depth + 1).min(4),
                4..=6 => depth.saturating_sub(1).max(1),
                _ => depth,
            };
            let stars = "*".repeat(depth);
            let marker = match (r >> 8) % 6 {
                0 => "TODO ",
                1 => "DONE ",
                2 => "DOING ",
                _ => "",
            };
            let frag = FRAGMENTS[((r >> 16) as usize) % FRAGMENTS.len()];
            // Every ~7th block carries an intra-vault link, exercising link parsing.
            let link = if j % 7 == 3 && j > 0 {
                format!(" see [[block:soak-{k}-0][top]]")
            } else {
                String::new()
            };
            body.push_str(&format!(
                "{stars} {marker}{frag} · block {k}-{j}{link}\n:PROPERTIES:\n:ID: \
                 soak-{k}-{j}\n:END:\n"
            ));
        }
        files.push((format!("soak-{k}.org"), body));
    }
    (files, start_k + doc_count)
}

/// Deep-chain docs: `ceil(total / soak_depth())` pages, each a single LINEAR
/// nesting chain (`page → b0 → b1 → … → b{L-1}`, heading level `j+1`) of depth
/// up to `HOLON_SOAK_DEPTH`. A navigate-to-page here materializes an L-deep
/// recursive descent — the cycle-guard-string-scan lever. Returns the files and
/// the next free page index (for `mixed`).
fn deep_docs(total: usize, start_k: usize) -> (Vec<(String, String)>, usize) {
    let depth = soak_depth();
    let mut files = Vec::new();
    let mut emitted = 0usize;
    let mut k = start_k;
    while emitted < total {
        let chain_len = depth.min(total - emitted);
        let mut body = format!("#+ID: soak-doc-{k}\n#+TITLE: Soak Chain {k}\n");
        for j in 0..chain_len {
            // Heading level `j+1`: each block is the single child of the previous,
            // so the whole doc is one depth-`chain_len` chain under the page.
            let stars = "*".repeat(j + 1);
            let frag = FRAGMENTS[(k + j) % FRAGMENTS.len()];
            body.push_str(&format!(
                "{stars} block {k}-{j} · {frag}\n:PROPERTIES:\n:ID: soak-{k}-{j}\n:END:\n"
            ));
        }
        files.push((format!("soak-{k}.org"), body));
        emitted += chain_len;
        k += 1;
    }
    (files, k)
}
