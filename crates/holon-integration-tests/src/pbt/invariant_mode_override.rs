//! `HOLON_PBT_INVARIANTS` — runtime, environment-only softening of individual
//! invariants for the ONE composed keystone (`general_e2e_composed_pbt`).
//!
//! This is the invariant analog of the per-transition `HOLON_PBT_WEIGHTS` knob
//! (`transition_dispatch.rs`). It lets a run **de-escalate** (or escalate)
//! specific invariants *without touching the source-of-truth catalog* — so the
//! committed test is never weakened, only the environment is. Use it to get a
//! **disclosed, temporary green run** while a real fix is built for an
//! unfixable-locally red (e.g. an upstream Turso-IVM matview-drift invariant
//! like `inv-focus-roots` / `inv-matview-consistent-with-ref/root_layout`).
//!
//! Relocated out of the native `invariants/registry.rs` (which is being deleted
//! with the native runner core) so the composed check
//! ([`ComposedSut::check_invariants`](crate::pbt::composed::harness)) can
//! honour it: a matched `warn`/`skip` failure is logged loudly and made
//! non-fatal; a green run under active overrides is a DISCLOSED degraded run,
//! not a clean pass.
//!
//! Format: `HOLON_PBT_INVARIANTS="pattern:mode,pattern:mode,…"`, mode ∈
//! `strict|warn|skip`, pattern is an invariant-id glob with a single optional
//! `*` (`inv-focus-roots`, `inv-value-fn-provider*`, `*match-ref*`, `*`),
//! case-insensitive, first-match-wins.

/// A runtime override of an invariant's effective failure disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeOverride {
    /// Run the check; a failure terminates the test (force fatal).
    Strict,
    /// Run the check; a failure is logged but does not fail the run.
    Warn,
    /// Treat any failure as softened (logged, non-fatal). In the composed path
    /// the invariant has already run via `run_selected`, so `skip` and `warn`
    /// collapse to the same disclosed-softening disposition.
    Skip,
}

/// One parsed `pattern:mode` rule. The pattern matches invariant id strings
/// with a single optional `*` wildcard, case-insensitively — the same glob
/// shape as the transition-weight patterns, kept local to avoid coupling the
/// subsystems.
#[derive(Debug, Clone)]
enum IdPattern {
    Exact(String),
    Prefix(String),
    Suffix(String),
    Contains(String),
    Star,
}

impl IdPattern {
    fn parse(raw: &str) -> Self {
        let p = raw.trim().to_ascii_lowercase();
        if p == "*" {
            IdPattern::Star
        } else if let Some(inner) = p.strip_prefix('*').and_then(|s| s.strip_suffix('*')) {
            IdPattern::Contains(inner.to_string())
        } else if let Some(suf) = p.strip_prefix('*') {
            IdPattern::Suffix(suf.to_string())
        } else if let Some(pre) = p.strip_suffix('*') {
            IdPattern::Prefix(pre.to_string())
        } else {
            IdPattern::Exact(p)
        }
    }

    fn matches(&self, id: &str) -> bool {
        let id = id.to_ascii_lowercase();
        match self {
            IdPattern::Star => true,
            IdPattern::Exact(s) => id == *s,
            IdPattern::Prefix(s) => id.starts_with(s),
            IdPattern::Suffix(s) => id.ends_with(s),
            IdPattern::Contains(s) => id.contains(s),
        }
    }
}

fn parse_invariant_overrides() -> Vec<(IdPattern, ModeOverride)> {
    let raw = match std::env::var("HOLON_PBT_INVARIANTS") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return Vec::new(),
    };
    let mut rules = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((pat, mode)) = entry.split_once(':') else {
            eprintln!("[HOLON_PBT_INVARIANTS] ignoring malformed entry (no ':'): {entry:?}");
            continue;
        };
        let mode = match mode.trim().to_ascii_lowercase().as_str() {
            "strict" => ModeOverride::Strict,
            "warn" => ModeOverride::Warn,
            "skip" => ModeOverride::Skip,
            other => {
                eprintln!(
                    "[HOLON_PBT_INVARIANTS] ignoring entry with unknown mode {other:?} (expected \
                     strict|warn|skip): {entry:?}"
                );
                continue;
            }
        };
        rules.push((IdPattern::parse(pat), mode));
    }
    if !rules.is_empty() {
        // Disclose the active softening loudly — a green run under these
        // overrides is a DISCLOSED degraded run, not a clean pass.
        eprintln!(
            "[HOLON_PBT_INVARIANTS] {} invariant mode override rule(s) active from env",
            rules.len()
        );
    }
    rules
}

/// Effective [`ModeOverride`] for `invariant_id` from `HOLON_PBT_INVARIANTS`,
/// or `None` when no rule matches (use the default: a failure is fatal).
/// First-match-wins in declaration order.
pub fn invariant_mode_override(invariant_id: &str) -> Option<ModeOverride> {
    static PARSED: std::sync::OnceLock<Vec<(IdPattern, ModeOverride)>> = std::sync::OnceLock::new();
    let rules = PARSED.get_or_init(parse_invariant_overrides);
    rules
        .iter()
        .find(|(pat, _)| pat.matches(invariant_id))
        .map(|(_, mode)| *mode)
}
