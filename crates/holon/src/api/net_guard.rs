//! The third pre-provider gate: firing-time legality of the marking an
//! operation would produce (ADR 0032 §3).
//!
//! The dispatcher asks a yes/no question and never learns which policy answered
//! it. A refusal is an `Err` at the call site, so the operation never reaches
//! its provider.

use async_trait::async_trait;
use holon_core::Result;
use holon_core::storage::types::StorageEntity;

/// The param a caller sets to turn a refusal into a confirmed firing. Its
/// value names the [`ConfirmableClass`] the confirmation is minted for.
///
/// Named in every [`NetRefusal`] message so a UI can render the refusal as a
/// confirm dialog and re-dispatch the same operation with it set.
pub const CONFIRM_BREAK_PARAM: &str = "confirm_break";

/// The class of refusal a net-guard policy raises. A confirmation answers
/// exactly one class; `Authorization` has no [`ConfirmableClass`]
/// counterpart, so no confirmation ever answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalClass {
    /// Rule machinery separated from the structure that owns it.
    MachineryContainment,
    /// A destination whose home profile does not host the moved entity's kind.
    DestinationCapability,
    /// The caller may not perform the operation at all.
    Authorization,
}

impl RefusalClass {
    /// The wire spelling — the value a caller sets [`CONFIRM_BREAK_PARAM`] to.
    pub fn as_str(self) -> &'static str {
        match self {
            RefusalClass::MachineryContainment => "machinery_containment",
            RefusalClass::DestinationCapability => "destination_capability",
            RefusalClass::Authorization => "authorization",
        }
    }
}

/// The refusal classes a caller may confirm past. `Authorization` is absent
/// by construction, so a parsed confirmation can never name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmableClass {
    MachineryContainment,
    DestinationCapability,
}

impl ConfirmableClass {
    pub const ALL: [ConfirmableClass; 2] = [
        ConfirmableClass::MachineryContainment,
        ConfirmableClass::DestinationCapability,
    ];

    pub fn as_str(self) -> &'static str {
        RefusalClass::from(self).as_str()
    }
}

impl From<ConfirmableClass> for RefusalClass {
    fn from(class: ConfirmableClass) -> Self {
        match class {
            ConfirmableClass::MachineryContainment => RefusalClass::MachineryContainment,
            ConfirmableClass::DestinationCapability => RefusalClass::DestinationCapability,
        }
    }
}

/// Whether the caller already confirmed the consequence a refusal would name,
/// and for which refusal class.
///
/// Parsed once, at the dispatcher boundary, from
/// [`CONFIRM_BREAK_PARAM`]; a policy reads this instead of the params bag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    Absent,
    Confirmed(ConfirmableClass),
}

impl Confirmation {
    /// Whether this confirmation answers a refusal of `class`. `Absent`
    /// answers nothing, and nothing answers `Authorization`.
    pub fn answers(self, class: RefusalClass) -> bool {
        match self {
            Confirmation::Absent => false,
            Confirmation::Confirmed(minted) => RefusalClass::from(minted) == class,
        }
    }

    /// # Errors
    /// A `confirm_break` that does not name a [`ConfirmableClass`] — a bare
    /// Boolean would answer any refusal, which is what the class prevents,
    /// and `"authorization"` names a class that is not confirmable.
    pub fn parse(params: &StorageEntity) -> Result<Self> {
        let confirmable = || {
            ConfirmableClass::ALL
                .iter()
                .map(|c| format!("{:?}", c.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match params.get(CONFIRM_BREAK_PARAM) {
            None | Some(holon_api::Value::Null) => Ok(Self::Absent),
            Some(holon_api::Value::String(named)) => {
                if let Some(class) = ConfirmableClass::ALL
                    .into_iter()
                    .find(|c| c.as_str() == named)
                {
                    return Ok(Self::Confirmed(class));
                }
                if named == RefusalClass::Authorization.as_str() {
                    return Err(format!(
                        "`{CONFIRM_BREAK_PARAM}`: the `authorization` refusal class is not \
                         confirmable"
                    )
                    .into());
                }
                Err(format!(
                    "`{CONFIRM_BREAK_PARAM}` must name the refusal class it answers; {named:?} \
                     is not one of [{}]",
                    confirmable()
                )
                .into())
            }
            Some(other) => Err(format!(
                "`{CONFIRM_BREAK_PARAM}` must name the refusal class it answers, one of [{}]; \
                 got {other:?}",
                confirmable()
            )
            .into()),
        }
    }
}

/// One dispatched operation, as the net guard sees it.
pub struct NetGuardOp<'a> {
    pub entity_name: &'a str,
    pub op_name: &'a str,
    pub params: &'a StorageEntity,
    pub confirmation: Confirmation,
}

/// Why the resulting marking is illegal, in words a user can act on.
pub struct NetRefusal {
    pub class: RefusalClass,
    pub reason: String,
}

pub enum NetVerdict {
    Confirm,
    Refuse(NetRefusal),
}

/// Answers whether the marking an operation would produce is legal.
///
/// # Unification with [`crate::api::guard_world::GuardWorld`]
/// Both seams answer enabledness, and they stay separate only while their
/// inputs differ: a declared guard is a subject-bound predicate over the
/// current world, this one reads the whole delta an operation would write.
/// They unify once the derived net projection exists AND lived experience
/// shows the declared-guard predicates are expressible as net arcs — by
/// generalizing `GuardWorld` to marking-aware whole-delta evaluation and
/// folding this trait into it. Until then a policy that could be written as a
/// `#[require]` predicate belongs there, not here.
#[async_trait]
pub trait NetGuard: Send + Sync {
    async fn check(&self, op: &NetGuardOp<'_>) -> Result<NetVerdict>;
}

/// Confirms every operation, for a composition site that hosts no placement
/// policy — a Loro-only session, which has neither capability profiles nor a
/// document home to resolve a destination against.
///
/// Installed explicitly so the gate is never merely absent: absence is what
/// [`crate::api::operation_dispatcher::OperationDispatcher::assert_net_guard_installed`]
/// crashes on.
pub struct InertNetGuard;

#[async_trait]
impl NetGuard for InertNetGuard {
    async fn check(&self, _: &NetGuardOp<'_>) -> Result<NetVerdict> {
        Ok(NetVerdict::Confirm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params_with(value: holon_api::Value) -> StorageEntity {
        let mut p = StorageEntity::new();
        p.insert(CONFIRM_BREAK_PARAM.into(), value);
        p
    }

    #[test]
    fn absent_and_null_parse_to_absent() {
        assert_eq!(
            Confirmation::parse(&StorageEntity::new()).expect("parses"),
            Confirmation::Absent
        );
        assert_eq!(
            Confirmation::parse(&params_with(holon_api::Value::Null)).expect("parses"),
            Confirmation::Absent
        );
    }

    #[test]
    fn a_confirmation_parses_to_the_class_it_was_minted_for() {
        for class in ConfirmableClass::ALL {
            assert_eq!(
                Confirmation::parse(&params_with(holon_api::Value::String(
                    class.as_str().to_string()
                )))
                .expect("parses"),
                Confirmation::Confirmed(class)
            );
        }
    }

    #[test]
    fn the_authorization_class_is_not_confirmable() {
        let err = Confirmation::parse(&params_with(holon_api::Value::String(
            "authorization".to_string(),
        )))
        .expect_err("refuses");
        let msg = err.to_string();
        assert!(
            msg.contains("authorization") && msg.contains("not confirmable"),
            "the refusal must say the class cannot be confirmed: {msg}"
        );
    }

    #[test]
    fn a_bare_boolean_mints_no_confirmation() {
        for value in [
            holon_api::Value::Boolean(true),
            holon_api::Value::Boolean(false),
        ] {
            let err = Confirmation::parse(&params_with(value)).expect_err("refuses");
            assert!(
                err.to_string().contains("must name the refusal class"),
                "the refusal must demand a class: {err}"
            );
        }
    }

    #[test]
    fn an_unknown_class_is_refused_naming_the_confirmable_ones() {
        let err = Confirmation::parse(&params_with(holon_api::Value::String(
            "everything".to_string(),
        )))
        .expect_err("refuses");
        let msg = err.to_string();
        assert!(
            msg.contains("machinery_containment") && msg.contains("destination_capability"),
            "the refusal must list the confirmable classes: {msg}"
        );
    }

    #[test]
    fn a_confirmation_answers_only_its_own_class_and_nothing_answers_authorization() {
        let all = [
            Confirmation::Absent,
            Confirmation::Confirmed(ConfirmableClass::MachineryContainment),
            Confirmation::Confirmed(ConfirmableClass::DestinationCapability),
        ];
        for confirmation in all {
            assert!(!confirmation.answers(RefusalClass::Authorization));
            for class in ConfirmableClass::ALL {
                assert_eq!(
                    confirmation.answers(class.into()),
                    confirmation == Confirmation::Confirmed(class)
                );
            }
        }
    }
}
