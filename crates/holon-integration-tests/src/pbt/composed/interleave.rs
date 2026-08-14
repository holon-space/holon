//! The composed harness's scheduler-seed + kind-mask axis.
//!
//! The keystone awaits every write and settles all three projections to a fixed
//! point between transitions, so it cannot generate a task-ordering bug: no two
//! writes are ever in flight together. This module is the arming switch that
//! lets a NAMED transition kind run through the fire-and-forget dispatch door
//! (the door production GPUI uses) with a seeded SCHEDULE instead of the
//! immediate await, so intents of one transition overlap and the gaps between
//! them are decided by the seed.
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
//! | `HOLON_PBT_SCHED_SHAPE` | `burst` \| `mixed` \| `serial` | `mixed` |
//!
//! Read ONCE into a `OnceLock` at first use and never again, so the arming
//! decision cannot race a concurrent test — the form `reseed_observer.rs`
//! established for `HOLON_PBT_RESEED_ORACLE`.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use crate::pbt::composed::boundary::Boundary;
use crate::pbt::composed::boundary::Resume;
use crate::pbt::transitions::E2ETransition;

/// Which gaps a shape puts between the dispatches of one transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Every gap is `Immediate` — dispatch-all-then-settle. A case recorded
    /// under this schedule replays as the schedule it was recorded under.
    Burst,
    /// Seeded draw across the whole space.
    Mixed,
    /// Every gap waits for one intent to settle — fully drained dispatches.
    Serial,
}

/// How a masked transition is scheduled: the seed, and the shape the gaps are
/// drawn from. Attribution handle for a red — `(kind, seed)`.
///
/// The per-slot predicates are drawn LAZILY ([`InterleavePlan::resume_at`]):
/// a transition's dispatch count is not known until it runs (a `TypeChars`
/// draw dispatches one intent per character), and a lazy draw needs no length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterleavePlan {
    /// The per-(kind, tick) seed every slot's predicate is drawn from.
    /// Reported in the armed run's log line. It reproduces the SCHEDULE
    /// byte-for-byte; it does not reproduce the interleaving, because which of
    /// two permitted completions lands first is still the real system's call
    /// (see docs/Testing/PBT.md).
    pub seed: u64,
    pub shape: Shape,
}

impl InterleavePlan {
    /// The predicate governing the gap AFTER dispatch `slot`.
    pub fn resume_at(&self, slot: u64) -> Resume {
        match self.shape {
            Shape::Burst => Resume::Immediate,
            Shape::Serial => Resume::Wait(Boundary::AfterIntents(1)),
            Shape::Mixed => draw_resume(mix(self.seed ^ slot.wrapping_mul(0xD1B5_4A32_D192_ED03))),
        }
    }
}

/// Half the slots dispatch straight on, so a mixed run keeps reaching the
/// burst corner as well as the drained ones. `AfterQuiescence` is rare: it
/// settles everything, which ends the overlap the arming exists to create.
fn draw_resume(draw: u64) -> Resume {
    match draw % 100 {
        0..=49 => Resume::Immediate,
        50..=74 => Resume::Wait(Boundary::AfterIntents(1)),
        75..=84 => Resume::Wait(Boundary::AfterIntents(2)),
        85..=97 => Resume::Wait(Boundary::AfterCdcBatch),
        _ => Resume::Wait(Boundary::AfterQuiescence),
    }
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
    shape: Shape,
}

static ARMING: OnceLock<Arming> = OnceLock::new();

fn arming() -> &'static Arming {
    ARMING.get_or_init(|| Arming {
        mask: parse_kinds(std::env::var("HOLON_PBT_SCHED_KINDS").ok().as_deref()),
        seed: parse_u64("HOLON_PBT_SCHED_SEED", 0),
        shape: parse_shape(std::env::var("HOLON_PBT_SCHED_SHAPE").ok().as_deref()),
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

/// Parse the shape at the boundary. An unknown name fails loud for the same
/// reason a mis-spelled kind does: it would silently run a schedule nobody
/// asked for and report its result as if it were the requested one.
fn parse_shape(raw: Option<&str>) -> Shape {
    match raw.map(str::trim) {
        None | Some("") | Some("mixed") => Shape::Mixed,
        Some("burst") => Shape::Burst,
        Some("serial") => Shape::Serial,
        Some(other) => {
            panic!("HOLON_PBT_SCHED_SHAPE must be burst, mixed or serial, got {other:?}")
        }
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
/// same kind in one run get different schedules from one seed.
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
    Some(InterleavePlan {
        seed,
        shape: arming.shape,
    })
}

/// splitmix64 — the same deterministic stream `soak_seed` uses, so a seed
/// reproduces the SCHEDULE byte-for-byte across hosts.
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
            shape: Shape::Mixed,
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
            shape: Shape::Mixed,
        };
        assert!(plan_with(&arming, "TypeChars", 0).is_some());
        assert!(plan_with(&arming, "DeleteBackward", 0).is_none());
    }

    /// A schedule is reproducible from its seed, and two ticks of the same kind
    /// do not get the same one (otherwise one seed explores one schedule).
    #[test]
    fn the_schedule_is_seed_deterministic_and_tick_varying() {
        let arming = Arming {
            mask: Mask::Kinds(["TypeChars".to_string()].into_iter().collect()),
            seed: 42,
            shape: Shape::Mixed,
        };
        let a = plan_with(&arming, "TypeChars", 3).expect("masked");
        let b = plan_with(&arming, "TypeChars", 3).expect("masked");
        assert_eq!(a, b);
        assert_eq!(schedule_of(&a), schedule_of(&b));

        let schedules: BTreeSet<Vec<Resume>> = (0..16)
            .map(|t| schedule_of(&plan_with(&arming, "TypeChars", t).expect("masked")))
            .collect();
        assert!(
            schedules.len() > 1,
            "every tick drew the same schedule — the tick is not reaching the seed"
        );
    }

    fn schedule_of(plan: &InterleavePlan) -> Vec<Resume> {
        (0..24).map(|slot| plan.resume_at(slot)).collect()
    }

    /// The compatibility contract: `burst` is dispatch-all-then-settle at every
    /// slot, so a case recorded under that schedule replays under it.
    #[test]
    fn the_burst_shape_never_waits() {
        let arming = Arming {
            mask: Mask::Kinds(["TypeChars".to_string()].into_iter().collect()),
            seed: 42,
            shape: Shape::Burst,
        };
        let plan = plan_with(&arming, "TypeChars", 5).expect("masked");
        assert!(schedule_of(&plan).iter().all(|r| *r == Resume::Immediate));
    }

    #[test]
    fn the_serial_shape_drains_every_slot() {
        let arming = Arming {
            mask: Mask::Kinds(["TypeChars".to_string()].into_iter().collect()),
            seed: 42,
            shape: Shape::Serial,
        };
        let plan = plan_with(&arming, "TypeChars", 5).expect("masked");
        assert!(
            schedule_of(&plan)
                .iter()
                .all(|r| *r == Resume::Wait(Boundary::AfterIntents(1)))
        );
    }

    /// A mixed schedule must contain both kinds of slot: all-`Immediate` would
    /// silently be `burst`, and no-`Immediate` would never reach the corner the
    /// recorded regressions live on.
    #[test]
    fn the_mixed_shape_draws_both_immediate_and_waiting_slots() {
        let arming = Arming {
            mask: Mask::Kinds(["TypeChars".to_string()].into_iter().collect()),
            seed: 1,
            shape: Shape::Mixed,
        };
        let drawn = schedule_of(&plan_with(&arming, "TypeChars", 0).expect("masked"));
        assert!(drawn.iter().any(|r| *r == Resume::Immediate), "{drawn:?}");
        assert!(
            drawn.iter().any(|r| matches!(r, Resume::Wait(_))),
            "{drawn:?}"
        );
    }

    #[test]
    #[should_panic(expected = "HOLON_PBT_SCHED_SHAPE")]
    fn an_unknown_shape_fails_loud() {
        parse_shape(Some("drained"));
    }
}
