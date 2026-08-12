//! The composed harness's scheduler-seed + kind-mask axis.
//!
//! The keystone awaits every write and settles all three projections to a fixed
//! point between transitions, so it cannot generate a task-ordering bug: no two
//! writes are ever in flight together. This module is the arming switch that
//! lets a NAMED transition kind run through the fire-and-forget dispatch door
//! (the door production GPUI uses) with a seeded pump instead of the immediate
//! await, so intents of one transition overlap.
//!
//! **Unset means unchanged.** [`plan_for`] returns `None` for every kind when
//! `HOLON_PBT_SCHED_KINDS` is unset, and the harness's unmasked arm is the same
//! statements in the same order it ran before this module existed. Arming is
//! per-kind so a newly-visible red names exactly one `(kind, seed)` pair.
//!
//! | Variable | Meaning | Default |
//! |---|---|---|
//! | `HOLON_PBT_SCHED_KINDS` | comma-separated `E2ETransition` variant names, or `all` | unset ⇒ EMPTY mask ⇒ behaviour identical |
//! | `HOLON_PBT_SCHED_SEED` | `u64` scheduler seed | `0` |
//! | `HOLON_PBT_SCHED_STEPS` | max pump steps per masked transition | `8` |
//!
//! Read ONCE into a `OnceLock` at first use and never again, so the arming
//! decision cannot race a concurrent test — the form `reseed_observer.rs`
//! established for `HOLON_PBT_RESEED_ORACLE`.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use crate::pbt::transitions::E2ETransition;

/// The default `HOLON_PBT_SCHED_STEPS`.
const DEFAULT_MAX_STEPS: u32 = 8;

/// How a masked transition is scheduled: the seeded pump budget and the seed
/// that produced it. Attribution handle for a red — `(kind, seed)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterleavePlan {
    /// Pump steps to run after the detached apply, before the settle.
    pub steps: u32,
    /// The per-(kind, tick) seed the steps were drawn from. Reported in the
    /// armed run's log line to attribute the budget — NOT to replay the red:
    /// the pump yields race real thread scheduling, so the same seed widens
    /// the interleaving without reproducing it (see docs/Testing/PBT.md).
    pub seed: u64,
}

/// Which kinds are armed. `all` expands to the whole alphabet at parse time
/// rather than staying a wildcard, so a name that is not an `E2ETransition`
/// variant fails loud in [`parse_kinds`] instead of silently arming nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Mask {
    Empty,
    Kinds(BTreeSet<String>),
}

#[derive(Clone, Debug)]
struct Arming {
    mask: Mask,
    seed: u64,
    max_steps: u32,
}

static ARMING: OnceLock<Arming> = OnceLock::new();

fn arming() -> &'static Arming {
    ARMING.get_or_init(|| Arming {
        mask: parse_kinds(std::env::var("HOLON_PBT_SCHED_KINDS").ok().as_deref()),
        seed: parse_u64("HOLON_PBT_SCHED_SEED", 0),
        max_steps: parse_u64("HOLON_PBT_SCHED_STEPS", DEFAULT_MAX_STEPS as u64) as u32,
    })
}

/// Parse the kind mask at the boundary. Unset/empty ⇒ [`Mask::Empty`]; `all` ⇒
/// the whole alphabet; otherwise every comma-separated name MUST be an
/// `E2ETransition` variant. A typo that silently disarmed the run would make an
/// armed lane report "no red found" for a kind it never armed, which is the
/// worst failure mode this axis has.
fn parse_kinds(raw: Option<&str>) -> Mask {
    let Some(raw) = raw else { return Mask::Empty };
    let raw = raw.trim();
    if raw.is_empty() {
        return Mask::Empty;
    }
    if raw == "all" {
        return Mask::Kinds(
            E2ETransition::VARIANT_NAMES
                .iter()
                .map(|s| s.to_string())
                .collect(),
        );
    }
    let mut kinds = BTreeSet::new();
    for name in raw.split(',') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        assert!(
            E2ETransition::VARIANT_NAMES.contains(&name),
            "HOLON_PBT_SCHED_KINDS names {name:?}, which is not an E2ETransition variant. A \
             mis-spelled kind would arm NOTHING and report a false all-clear. Valid kinds: {:?}",
            E2ETransition::VARIANT_NAMES,
        );
        kinds.insert(name.to_string());
    }
    if kinds.is_empty() {
        Mask::Empty
    } else {
        Mask::Kinds(kinds)
    }
}

fn parse_u64(var: &str, default: u64) -> u64 {
    match std::env::var(var) {
        Err(_) => default,
        Ok(s) if s.trim().is_empty() => default,
        Ok(s) => s
            .trim()
            .parse()
            .unwrap_or_else(|e| panic!("{var} must be a u64, got {s:?}: {e}")),
    }
}

/// The interleaving plan for one transition, or `None` when the kind is not
/// masked — which is EVERY kind unless `HOLON_PBT_SCHED_KINDS` is set.
///
/// `tick` is the transition's index in the sequence, so two occurrences of the
/// same kind in one run get different pump budgets from one seed.
pub fn plan_for(kind: &str, tick: u64) -> Option<InterleavePlan> {
    plan_with(arming(), kind, tick)
}

fn plan_with(arming: &Arming, kind: &str, tick: u64) -> Option<InterleavePlan> {
    let Mask::Kinds(kinds) = &arming.mask else {
        return None;
    };
    if !kinds.contains(kind) {
        return None;
    }
    let seed = mix(arming.seed ^ hash_kind(kind) ^ tick.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let steps = if arming.max_steps == 0 {
        0
    } else {
        (seed % (arming.max_steps as u64 + 1)) as u32
    };
    Some(InterleavePlan { steps, seed })
}

/// splitmix64 — the same deterministic stream `soak_seed` uses, so a seed
/// reproduces the PUMP BUDGET byte-for-byte across hosts. The budget is not
/// the schedule: the yields it buys are raced against the ambient tokio
/// runtime, so the resulting interleaving is not reproducible.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn hash_kind(kind: &str) -> u64 {
    kind.bytes().fold(0xCBF2_9CE4_8422_2325_u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A4: the mask keys on the kind string the harness computes, and that
    /// string is `E2ETransition::variant_name()` (the slice's `transition_kind`
    /// override), so the parseable alphabet IS the variant-name alphabet. This
    /// pins the two together — a variant added without a name here would be
    /// unmaskable.
    #[test]
    fn every_variant_name_is_a_parseable_kind() {
        assert!(!E2ETransition::VARIANT_NAMES.is_empty());
        for name in E2ETransition::VARIANT_NAMES {
            let Mask::Kinds(kinds) = parse_kinds(Some(name)) else {
                panic!("{name} did not parse into a non-empty mask");
            };
            assert_eq!(
                kinds.iter().map(String::as_str).collect::<Vec<_>>(),
                vec![*name]
            );
        }
    }

    #[test]
    fn all_arms_the_whole_alphabet() {
        let Mask::Kinds(kinds) = parse_kinds(Some("all")) else {
            panic!("`all` must arm every kind");
        };
        assert_eq!(kinds.len(), E2ETransition::VARIANT_NAMES.len());
    }

    #[test]
    fn unset_and_blank_are_the_empty_mask() {
        assert_eq!(parse_kinds(None), Mask::Empty);
        assert_eq!(parse_kinds(Some("")), Mask::Empty);
        assert_eq!(parse_kinds(Some("   ")), Mask::Empty);
    }

    #[test]
    #[should_panic(expected = "not an E2ETransition variant")]
    fn an_unknown_kind_fails_loud_rather_than_arming_nothing() {
        parse_kinds(Some("TypeChar"));
    }

    /// The landing gate in miniature: with the empty mask NO kind has a plan,
    /// so the harness takes its unmasked arm for all 71 of them.
    #[test]
    fn the_empty_mask_plans_nothing_for_any_kind() {
        let arming = Arming {
            mask: Mask::Empty,
            seed: 0,
            max_steps: DEFAULT_MAX_STEPS,
        };
        for name in E2ETransition::VARIANT_NAMES {
            assert!(
                plan_with(&arming, name, 0).is_none(),
                "{name} got a plan under the empty mask"
            );
        }
    }

    #[test]
    fn a_masked_kind_plans_and_an_unmasked_neighbour_does_not() {
        let arming = Arming {
            mask: Mask::Kinds(["TypeChars".to_string()].into_iter().collect()),
            seed: 7,
            max_steps: DEFAULT_MAX_STEPS,
        };
        assert!(plan_with(&arming, "TypeChars", 0).is_some());
        assert!(plan_with(&arming, "DeleteBackward", 0).is_none());
    }

    /// A pump budget is reproducible from its seed, and two ticks of the same
    /// kind do not get the same budget (otherwise one seed explores one
    /// interleaving).
    #[test]
    fn the_plan_is_seed_deterministic_and_tick_varying() {
        let arming = Arming {
            mask: Mask::Kinds(["TypeChars".to_string()].into_iter().collect()),
            seed: 42,
            max_steps: DEFAULT_MAX_STEPS,
        };
        let a = plan_with(&arming, "TypeChars", 3).expect("masked");
        let b = plan_with(&arming, "TypeChars", 3).expect("masked");
        assert_eq!(a, b);
        let budgets: BTreeSet<u32> = (0..16)
            .map(|t| plan_with(&arming, "TypeChars", t).expect("masked").steps)
            .collect();
        assert!(
            budgets.len() > 1,
            "every tick drew the same pump budget — the tick is not reaching the seed"
        );
        assert!(budgets.iter().all(|s| *s <= DEFAULT_MAX_STEPS));
    }
}
