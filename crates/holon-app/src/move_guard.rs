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
//! shared block projection and the document-home authority.

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
use holon::core::queryable_cache::QueryableCache;
use holon_api::EntityUri;
use holon_api::live_data::home_by::DurableFormat;
use holon_api::live_data::home_by::HomeAuthority;
use holon_capability::EntityKind;
use holon_capability::ProfileRegistry;
use holon_capability::profile_of;
use holon_core::Result;
use holon_core::block_ordering::BlockOrdering;
use holon_orgmode::home_authority::BlockHomeAuthority;
use holon_orgmode::home_authority::DocHome;
use holon_orgmode::home_authority::HomeBurstMemo;

/// The op whose delta this policy ranges over. `rehome_entity` performs its
/// move by dispatching this one, so guarding it covers both.
const MOVE_BLOCK_OP: &str = "move_block";

/// What the projection says about the block being moved.
struct Subject {
    parent_id: EntityUri,
    /// Rule machinery: a source block under a heading that owns a rule head —
    /// which includes the rule head itself. The same two clauses
    /// `block_profile.yaml`'s `is_program` states, read here as one predicate.
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

    async fn cache(&self) -> Arc<QueryableCache<holon_api::block::Block>> {
        self.injector
            .resolve_async::<QueryableCache<holon_api::block::Block>>()
            .await
    }

    async fn authority(&self) -> BlockHomeAuthority {
        let cache = self.cache().await;
        BlockHomeAuthority::new(
            Arc::new(crate::turso_seams::CacheBlockReader::new(cache)),
            self.injector.resolve_async::<dyn BlockOrdering>().await,
        )
    }

    /// The subject's row, or `None` when the projection does not hold it yet.
    ///
    /// A block the projection has not caught up on cannot be classified, and
    /// the classification is what both refusals rest on — so an unseen block
    /// is confirmed rather than refused on a guess.
    async fn subject(&self, id: &EntityUri) -> Result<Option<Subject>> {
        let mut params = std::collections::HashMap::new();
        params.insert(
            "id".to_string(),
            holon_api::Value::String(id.as_str().to_string()),
        );
        let rows = self
            .cache()
            .await
            .db_handle()
            .query(
                "SELECT b.parent_id AS parent_id, \
                 (b.content_type = 'source' AND EXISTS (SELECT 1 FROM block_raw s WHERE \
                 s.parent_id = b.parent_id AND s.content_type = 'source' AND s.source_language IN \
                 ('holon_rule', 'action'))) AS is_program, \
                 EXISTS (SELECT 1 FROM block_tags t WHERE t.block_id = b.id AND t.tag = 'Page') AS \
                 is_page \
                 FROM block_raw b WHERE b.id = $id",
                params,
            )
            .await
            .map_err(|e| format!("net guard: classifying `{id}`: {e}"))?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let flag = |key: &str| {
            row.get(key)
                .map(|v| match v {
                    holon_api::Value::Boolean(b) => *b,
                    holon_api::Value::Integer(i) => *i != 0,
                    other => panic!("net guard: `{key}` came back as {other:?}, not a flag"),
                })
                .unwrap_or(false)
        };
        let parent_id = row
            .get("parent_id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| format!("net guard: `{id}` has no parent_id in the projection"))?;
        let is_program = flag("is_program");
        let kind = if is_program {
            EntityKind::Program
        } else if flag("is_page") {
            EntityKind::Page
        } else {
            EntityKind::Block
        };
        Ok(Some(Subject {
            parent_id: EntityUri::parse(parent_id)
                .map_err(|e| format!("net guard: `{id}`'s parent_id is not an entity uri: {e}"))?,
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
