//! The vault profiles in effect, the invariants they declare and their
//! refusals depend only on the current profile blocks. Any sequence of adds,
//! edits and deletes ends where a fresh boot of the current blocks starts: a
//! refused block is absent, whatever version of it loaded before.
//!
//! @pbt kind property
//! @pbt covers vault-profile-admission-history-free — the mirror after any
//!   history equals a fresh boot of the final set

use std::collections::BTreeMap;
use std::sync::Arc;

use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_profiles::EntityProfile;
use holon_profiles::TypeRegistry;
use holon_profiles::vault_profiles::VaultProfileDerivation;
use proptest::prelude::*;

const IDS: [&str; 4] = ["block:a", "block:b", "block:c", "block:d"];
const BROKEN: usize = 4;

fn invariant(name: &str, open_state: &str) -> String {
    format!(
        "  {name}:\n    \
           holds: 'block.task_state == \"DONE\" => not child(block.task_state == \"{open_state}\")'\n    \
           on_violation: {{ repair: reopen_parent, set: {{ task_state: DOING }} }}\n"
    )
}

fn content(index: usize) -> String {
    let profile =
        |invariants: &[String]| format!("entity_name: block\ninvariants:\n{}", invariants.concat());
    match index {
        0 => profile(&[invariant("inv_x", "TODO")]),
        1 => profile(&[invariant("inv_y", "TODO")]),
        2 => profile(&[invariant("inv_x", "TODO"), invariant("inv_y", "TODO")]),
        3 => profile(&[invariant("inv_y", "DOING")]),
        BROKEN => "entity_name: [".to_string(),
        _ => unreachable!("content index {index}"),
    }
}

#[derive(Debug, Clone)]
enum Op {
    Set(usize, usize),
    Delete(usize),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0..IDS.len(), 0..=BROKEN).prop_map(|(id, c)| Op::Set(id, c)),
        1 => (0..IDS.len()).prop_map(Op::Delete),
    ]
}

/// One registry and its mirror derivation, as a session holds them.
struct Session {
    registry: Arc<TypeRegistry>,
    bus: Arc<ConditionBus>,
    derivation: VaultProfileDerivation<
        Box<
            dyn Fn(
                &BTreeMap<String, String>,
            ) -> BTreeMap<String, holon_profiles::VaultProfileAdmission>,
        >,
    >,
    in_effect: BTreeMap<String, Arc<EntityProfile>>,
}

impl Session {
    fn boot() -> Self {
        let registry = Arc::new(TypeRegistry::new());
        registry
            .register(holon_api::block::Block::type_definition())
            .expect("register block");
        let bus = Arc::new(ConditionBus::new());
        let derivation = VaultProfileDerivation::new(
            Box::new(registry.vault_profile_admitter()) as Box<_>,
            Arc::clone(&bus),
        );
        Self {
            registry,
            bus,
            derivation,
            in_effect: BTreeMap::new(),
        }
    }

    fn feed(&mut self, rows: &BTreeMap<String, String>) {
        let rows = rows
            .iter()
            .map(|(id, c)| (id.clone(), Arc::new(c.clone())))
            .collect();
        if let Some(in_effect) = self.derivation.derive(&rows) {
            self.in_effect = in_effect;
        }
    }

    fn declared(&self) -> Vec<String> {
        self.registry
            .declared_invariants("block")
            .iter()
            .map(|i| i.name.to_string())
            .collect()
    }

    fn refusals(&self) -> BTreeMap<String, String> {
        self.bus
            .current()
            .into_iter()
            .map(|c| match c.reason {
                ConditionKind::ProfileRefused { error } => (c.subject, error),
                other => panic!("unexpected condition {other:?}"),
            })
            .collect()
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn any_history_ends_where_a_fresh_boot_of_the_current_blocks_starts(
        ops in proptest::collection::vec(op(), 1..12)
    ) {
        let mut live = Session::boot();
        let mut rows: BTreeMap<String, usize> = BTreeMap::new();
        for (step, op) in ops.iter().enumerate() {
            match *op {
                Op::Set(id, c) => {
                    rows.insert(IDS[id].to_string(), c);
                }
                Op::Delete(id) => {
                    rows.remove(IDS[id]);
                }
            }
            let current: BTreeMap<String, String> =
                rows.iter().map(|(id, c)| (id.clone(), content(*c))).collect();
            live.feed(&current);
            let mut fresh = Session::boot();
            fresh.feed(&current);

            prop_assert_eq!(live.declared(), fresh.declared(), "declared invariants after step {} {:?}", step, op);
            prop_assert_eq!(
                live.in_effect.keys().collect::<Vec<_>>(),
                fresh.in_effect.keys().collect::<Vec<_>>(),
                "profiles in effect after step {} {:?}", step, op
            );
            let live_refusals = live.refusals();
            prop_assert_eq!(&live_refusals, &fresh.refusals(), "refusals after step {} {:?}", step, op);
            for (id, error) in &live_refusals {
                if let Some(winner) = error
                    .split("declared by vault profile '")
                    .nth(1)
                    .and_then(|rest| rest.split('\'').next())
                {
                    prop_assert!(winner < id.as_str(), "{} lost to the larger id {}", id, winner);
                }
            }
        }
    }
}

/// The resolver puts each admitted set in effect on its own task, so it can
/// apply an older set after a newer admission. That must not take back the
/// newer admission's invariants.
#[test]
fn a_stale_resolver_apply_keeps_the_latest_admissions_invariants() {
    let registry = Arc::new(TypeRegistry::new());
    registry
        .register(holon_api::block::Block::type_definition())
        .expect("register block");
    let admit = registry.vault_profile_admitter();
    let claims = registry.vault_profile_claims();
    let candidates = |rows: &[(&str, usize)]| {
        rows.iter()
            .map(|(id, c)| (id.to_string(), content(*c)))
            .collect::<BTreeMap<_, _>>()
    };
    let in_effect = |admissions: BTreeMap<String, holon_profiles::VaultProfileAdmission>| {
        admissions
            .into_iter()
            .filter_map(|(id, a)| match a {
                holon_profiles::VaultProfileAdmission::InEffect(p) => Some((id, Arc::new(p))),
                holon_profiles::VaultProfileAdmission::Refused(_) => None,
            })
            .collect::<BTreeMap<_, _>>()
    };
    let first = in_effect(admit(&candidates(&[("block:a", 0)])));
    let second = in_effect(admit(&candidates(&[("block:a", 0), ("block:c", 1)])));
    claims.apply(&first, || {});
    claims.apply(&second, || {});
    let declared: Vec<String> = registry
        .declared_invariants("block")
        .iter()
        .map(|i| i.name.to_string())
        .collect();
    assert_eq!(
        declared,
        vec!["inv_x".to_string(), "inv_y".to_string()],
        "a stale apply dropped the latest admission's invariants"
    );
}
