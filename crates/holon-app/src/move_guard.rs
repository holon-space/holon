//! The net gate's first policy: where a block may be re-placed (ADR 0032 §3).
//!
//! Two refusals, each overridable only by a confirmation minted for its own
//! class. Machinery
//! containment keeps a rule's blocks under a page whose file can carry the
//! rule; destination capability keeps an entity out of a home whose profile
//! does not declare it can store that kind.
//!
//! It lives with the composition root for the same reason `rehome_entity`
//! does: deciding a destination's capability needs `holon-capability`, which
//! `holon` may not link. It builds no writer — every read goes through the
//! backend-blind `BlockReader` seam and the profile resolver's computed
//! fields, so the guard evaluates with Turso absent (ADR 0032 D10).

use std::sync::Arc;

use async_trait::async_trait;
use fluxdi::Injector;
use holon::api::net_guard::CONFIRM_BREAK_PARAM;
use holon::api::net_guard::ConfirmableClass;
use holon::api::net_guard::Confirmation;
use holon::api::net_guard::NetGuard;
use holon::api::net_guard::NetGuardOp;
use holon::api::net_guard::NetRefusal;
use holon::api::net_guard::NetVerdict;
use holon_api::EntityUri;
use holon_api::live_data::home_by::DurableFormat;
use holon_api::live_data::home_by::HomeAuthority;
use holon_capability::EntityKind;
use holon_capability::ProfileRegistry;
use holon_capability::profile_of;
use holon_core::Result;
use holon_core::block_ordering::BlockOrdering;
use holon_filesystem::BlockReader;
use holon_orgmode::home_authority::BlockHomeAuthority;
use holon_orgmode::home_authority::DocHome;
use holon_orgmode::home_authority::HomeBurstMemo;
use holon_profiles::ProfileResolving;

/// The op whose delta this policy ranges over. `rehome_entity` performs its
/// move by dispatching this one, so guarding it covers both.
const MOVE_BLOCK_OP: &str = "move_block";

/// What the store says about the block being moved.
struct Subject {
    parent_id: EntityUri,
    /// Rule machinery: a source block under a heading that owns a rule head —
    /// which includes the rule head itself. `block_profile.yaml`'s
    /// `is_program` computed field, evaluated — not restated.
    is_program: bool,
    kind: EntityKind,
}

pub struct MoveGuard {
    /// Resolved lazily, never at construction: this guard is consulted from
    /// inside the dispatcher, so resolving the dispatcher's own dependencies
    /// at wiring time would be a cycle.
    injector: Injector,
    registry: Arc<ProfileRegistry>,
}

impl MoveGuard {
    pub fn new(injector: Injector, registry: Arc<ProfileRegistry>) -> Self {
        Self { injector, registry }
    }

    async fn reader(&self) -> Arc<dyn BlockReader> {
        self.injector.resolve_async::<dyn BlockReader>().await
    }

    async fn authority(&self) -> BlockHomeAuthority {
        BlockHomeAuthority::new(
            self.reader().await,
            self.injector.resolve_async::<dyn BlockOrdering>().await,
        )
    }

    /// The subject's block, or `None` when the store does not hold it yet.
    ///
    /// A block the store has not caught up on cannot be classified, and
    /// the classification is what both refusals rest on — so an unseen block
    /// is confirmed rather than refused on a guess.
    ///
    /// `is_program` is the block profile's computed field of that name,
    /// evaluated through the same resolver the renderer uses — the yaml is
    /// the single statement of the predicate, and its `rule_sibling` lookup
    /// carries both storage arms (D10: no Turso required).
    async fn subject(&self, id: &EntityUri) -> Result<Option<Subject>> {
        let Some(block) = self
            .reader()
            .await
            .get_block_authoritative(id)
            .await
            .map_err(|e| format!("net guard: classifying `{id}`: {e}"))?
        else {
            return Ok(None);
        };

        let mut row = std::collections::HashMap::new();
        row.insert(
            "id".to_string(),
            holon_api::Value::String(block.id.as_str().to_string()),
        );
        row.insert(
            "parent_id".to_string(),
            holon_api::Value::String(block.parent_id.as_str().to_string()),
        );
        row.insert(
            "content_type".to_string(),
            holon_api::Value::String(block.content_type.to_string()),
        );
        // Always bound, `Null` for a non-source block: an absent column would
        // leave `is_rule_head` structurally unbound and `is_program` would
        // come back `Null` instead of `false`.
        row.insert(
            "source_language".to_string(),
            match &block.source_language {
                Some(lang) => holon_api::Value::String(lang.to_string()),
                None => holon_api::Value::Null,
            },
        );
        let computed = self
            .injector
            .resolve_async::<dyn ProfileResolving>()
            .await
            .resolve_computed_only(
                &row,
                &holon_api::render_requirements::RenderRequirements::none(),
            );
        let is_program = match computed.get("is_program") {
            Some(holon_api::Value::Boolean(b)) => *b,
            // The evaluator's typed "unbound" — a genuine eval failure is
            // warn-disclosed there. Falsy, exactly as every renderer
            // condition treats it.
            Some(holon_api::Value::Null) => false,
            Some(other) => {
                return Err(format!(
                    "net guard: `is_program` for `{id}` came back as {other:?}, not a flag"
                )
                .into());
            }
            None => {
                return Err(format!(
                    "net guard: the block profile computed no `is_program` for `{id}` — the \
                     resolver is missing the block type profile"
                )
                .into());
            }
        };
        let kind = if is_program {
            EntityKind::Program
        } else if block.tags.contains(holon_api::PAGE_TAG) {
            EntityKind::Page
        } else {
            EntityKind::Block
        };
        Ok(Some(Subject {
            parent_id: block.parent_id,
            is_program,
            kind,
        }))
    }

    /// The durable format the destination parent's home is stored in.
    async fn destination_format(&self, parent: &EntityUri) -> Result<Option<DurableFormat>> {
        if parent.is_no_parent() {
            return Ok(None);
        }
        let mut memo = HomeBurstMemo::default();
        let placement = self
            .authority()
            .await
            .locate(parent.as_str(), &mut memo)
            .await
            .map_err(|e| format!("net guard: locating destination `{parent}`: {e}"))?
            .ok_or_else(|| {
                format!("net guard: destination `{parent}` is not a block the store holds")
            })?;
        Ok(match placement.doc {
            DocHome::Resolved(_) => Some(DurableFormat::Org),
            DocHome::Unresolved => None,
        })
    }

    /// Whether the destination's home profile declares it can store `kind`.
    fn destination_hosts_kind(
        &self,
        format: Option<DurableFormat>,
        kind: EntityKind,
    ) -> Result<Option<String>> {
        let home = profile_of(format);
        let profile = self
            .registry
            .get(&home)
            .ok_or_else(|| format!("net guard: no capability profile named `{home}`"))?;
        if profile.hosted_entity_kinds().contains(&kind) {
            return Ok(None);
        }
        Ok(Some(format!(
            "`{home}` does not declare that it can store an entity of kind `{kind}`"
        )))
    }
}

#[async_trait]
impl NetGuard for MoveGuard {
    async fn check(&self, op: &NetGuardOp<'_>) -> Result<NetVerdict> {
        if op.entity_name != "block" || op.op_name != MOVE_BLOCK_OP {
            return Ok(NetVerdict::Confirm);
        }
        let (Some(id), Some(destination)) = (
            op.params.get("id").and_then(|v| v.as_string()),
            op.params.get("parent_id").and_then(|v| v.as_string()),
        ) else {
            return Ok(NetVerdict::Confirm);
        };
        let id = EntityUri::parse(id)
            .map_err(|e| format!("net guard: `id` is not an entity uri: {e}"))?;
        let destination = EntityUri::parse(destination)
            .map_err(|e| format!("net guard: `parent_id` is not an entity uri: {e}"))?;
        let Some(subject) = self.subject(&id).await? else {
            return Ok(NetVerdict::Confirm);
        };

        // A rule is read from a head and the blocks beside it, so the heading
        // that head sits under IS the rule. Machinery under any other parent is
        // machinery no rule can be read from — which the destination's home has
        // no bearing on, an ordinary org page being just as wrong as the root.
        // An operation that relocates the whole owning heading says so with
        // `confirm_break`; from one move's delta the two are indistinguishable.
        let refusal = if subject.is_program && destination != subject.parent_id {
            Some((
                ConfirmableClass::MachineryContainment,
                format!(
                    "re-homing a rule's action block breaks the rule: `{id}` is rule machinery \
                     owned by `{}`, and `{destination}` is outside it",
                    subject.parent_id
                ),
            ))
        } else {
            self.destination_hosts_kind(self.destination_format(&destination).await?, subject.kind)?
                .map(|why| {
                    (
                        ConfirmableClass::DestinationCapability,
                        format!("`{id}` cannot move into `{destination}`: {why}"),
                    )
                })
        };
        let Some((class, reason)) = refusal else {
            return Ok(NetVerdict::Confirm);
        };
        if op.confirmation.answers(class.into()) {
            return Ok(NetVerdict::Confirm);
        }
        let mismatch = match op.confirmation {
            Confirmation::Confirmed(minted) => format!(
                " — a `{}` confirmation does not answer this `{}` refusal",
                minted.as_str(),
                class.as_str()
            ),
            Confirmation::Absent => String::new(),
        };
        Ok(NetVerdict::Refuse(NetRefusal {
            class: class.into(),
            reason: format!(
                "{reason}. Re-dispatch with `{CONFIRM_BREAK_PARAM}: \"{}\"` to do it \
                 anyway{mismatch}",
                class.as_str()
            ),
        }))
    }
}
