//! `rehome_entity`: move a LEAF block out of the file that holds it, into
//! Holon's own storage, and report what the move cost.
//!
//! Operations are data contributed from anywhere, so this one lives with the
//! composition root rather than in the substrate: `holon` may not link
//! `holon-capability` (its own profile has to stay an independent statement
//! about it), and pricing the move needs both homes' profiles. The move itself
//! is dispatched as data — `move_block` through the inner provider — so this
//! adds no second write path.

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Injector;
use holon::core::queryable_cache::QueryableCache;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OperationDescriptor;
use holon_api::OperationParam;
use holon_api::TypeHint;
use holon_api::Value;
use holon_api::live_data::home_by::DurableFormat;
use holon_api::live_data::home_by::HomeAuthority;
use holon_capability::ProfileRegistry;
use holon_capability::profile_of;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::block_ordering::BlockOrdering;
use holon_core::storage::types::StorageEntity;
use holon_orgmode::home_authority::BlockHomeAuthority;
use holon_orgmode::home_authority::DocHome;
use holon_orgmode::home_authority::HomeBurstMemo;

pub const REHOME_ENTITY_OP: &str = "rehome_entity";
/// The key the result carries the move's price under.
pub const REHOMING_COST_KEY: &str = "rehoming_cost";

/// The home a re-home can move an entity INTO.
///
/// Holon's own store is the only variant: every other home would have to
/// CREATE the block in a foreign graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RehomeTarget {
    HolonNative,
}

impl RehomeTarget {
    /// Every refusal names the home it refused and why that home cannot
    /// receive an entity.
    pub fn parse(target: &str) -> std::result::Result<Self, String> {
        match target {
            "holon-native" => Ok(Self::HolonNative),
            "logseq-db" => Err(
                "cannot re-home into `logseq-db`: its writer refuses to create a block by name \
                 (holon-logseq-db kvs_writer.rs:1276-1282), and re-homing an entity into a graph \
                 is a creation"
                    .to_string(),
            ),
            other => Err(format!(
                "cannot re-home into `{other}`: no home of that name can receive an entity"
            )),
        }
    }
}

pub struct RehomeEntityProvider {
    /// Resolved lazily, never at construction: this provider is itself a member
    /// of the set the dispatcher is built from, so resolving either at wiring
    /// time would be a cycle. Resolving the SHARED `BlockOrdering` and router
    /// also keeps this operation off any write path of its own — a second
    /// `SqlBlockOperations` built here would be a Loro-blind writer.
    injector: Injector,
    registry: Arc<ProfileRegistry>,
}

impl RehomeEntityProvider {
    pub fn new(injector: Injector, registry: Arc<ProfileRegistry>) -> Self {
        Self { injector, registry }
    }

    async fn ordering(&self) -> Arc<dyn BlockOrdering> {
        self.injector.resolve_async::<dyn BlockOrdering>().await
    }

    /// The authority that answers which document holds a block, built over the
    /// SHARED ordering and a read-only cache view.
    ///
    /// The reader is `CacheBlockReader::new`, which carries NO block feed
    /// (`turso_seams.rs:82-88`), so it cannot wait for a write to arrive. Under
    /// Loro authority the block store is a projection of the CRDT, and a read
    /// taken immediately after a write can still show the pre-move parent.
    /// Two consequences, both deliberate: the "already at the root" refusal is
    /// taken BEFORE the move, where no lag can reach it, and the `to` fact this
    /// operation reports is measured rather than guaranteed fresh — nothing
    /// refuses on it. A caller needing an authoritative post-move home must
    /// re-read after settle.
    async fn authority(&self) -> BlockHomeAuthority {
        let cache = self
            .injector
            .resolve_async::<QueryableCache<holon_api::block::Block>>()
            .await;
        BlockHomeAuthority::new(
            Arc::new(crate::turso_seams::CacheBlockReader::new(cache)),
            self.ordering().await,
        )
    }

    /// The durable format holding `id` RIGHT NOW, read from the authority.
    ///
    /// Measured on both sides of the move rather than inferred from the target
    /// parameter: a result that reports what it intended instead of what it did
    /// is a witness of nothing.
    async fn home_now(&self, id: &EntityUri) -> Result<Option<DurableFormat>> {
        Ok(self.placement_now(id).await?.0)
    }

    /// The durable format holding `id` RIGHT NOW, and whether it already sits
    /// at the tree root.
    ///
    /// Measured on both sides of the move rather than inferred from the target
    /// parameter: a result that reports what it intended instead of what it did
    /// is a witness of nothing. `at_root` comes from the authority's own
    /// `parent`, which is `None` exactly when the block's parent is the
    /// no-parent sentinel.
    ///
    /// The read can lag a Loro-authoritative write — see [`Self::authority`]
    /// for what that costs and why nothing refuses on the post-move read.
    async fn placement_now(&self, id: &EntityUri) -> Result<(Option<DurableFormat>, bool)> {
        let mut memo = HomeBurstMemo::default();
        let authority = self.authority().await;
        let placement = authority
            .locate(id.as_str(), &mut memo)
            .await
            .map_err(|e| format!("{REHOME_ENTITY_OP}: locating `{id}`: {e}"))?
            .ok_or_else(|| format!("{REHOME_ENTITY_OP}: `{id}` is not a block the store holds"))?;
        let format = match placement.doc {
            // A resolved document is an org file: that is Holon's only file leg.
            DocHome::Resolved(_) => Some(DurableFormat::Org),
            DocHome::Unresolved => None,
        };
        Ok((format, placement.parent.is_none()))
    }
}

/// What moving between these two homes costs, as the clause names that pay.
///
/// A free function so the price can be checked without a container — the
/// provider itself needs a built DI graph, and an unexercised pricing path is
/// how a move quietly starts reporting nothing.
pub fn rehoming_cost_between(
    registry: &ProfileRegistry,
    from: Option<DurableFormat>,
    to: Option<DurableFormat>,
) -> Result<Vec<String>> {
    let from = profile_of(from);
    let to = profile_of(to);
    let losses = registry
        .rehoming_cost(&from, &to)
        .map_err(|e| format!("{REHOME_ENTITY_OP}: pricing {from} -> {to}: {e}"))?;
    Ok(losses.iter().map(|l| l.clause.to_string()).collect())
}

pub fn rehome_entity_descriptor() -> OperationDescriptor {
    OperationDescriptor {
        entity_name: "block".into(),
        entity_short_name: "block".to_string(),
        id_column: "id".to_string(),
        name: REHOME_ENTITY_OP.to_string(),
        display_name: "Move to Holon storage".to_string(),
        description: "Move a leaf block out of the file that holds it, into Holon's own storage"
            .to_string(),
        required_params: vec![
            OperationParam {
                name: "id".to_string(),
                type_hint: TypeHint::String,
                description: "The leaf block to re-home".to_string(),
            },
            OperationParam {
                name: "target".to_string(),
                type_hint: TypeHint::OneOf {
                    values: vec![Value::String("holon-native".to_string())],
                },
                description: "The home to move it into".to_string(),
            },
        ],
        affected_fields: vec!["parent_id".to_string(), "sort_key".to_string()],
        param_mappings: vec![],
        target_scope: holon_api::TargetScope::Block,
        boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
        menu_exposure: holon_api::MenuExposure::NotListed {
            surface: holon_api::NonMenuSurface::PointerGesture,
        },
        trigger: None,
        bound_params: Default::default(),
        guard: holon_api::pattern::OpGuard::None,
        arcs: holon_api::arcs::TransitionArcs::Declared {
            reads: vec![holon_api::arcs::ArcPlace::new("block", "parent_id")],
            emits: vec![
                holon_api::arcs::ArcEmit::Writes(holon_api::arcs::ArcPlace::new(
                    "block",
                    "parent_id",
                )),
                holon_api::arcs::ArcEmit::Writes(holon_api::arcs::ArcPlace::new(
                    "block", "sort_key",
                )),
            ],
        },
    }
}

#[async_trait::async_trait]
impl OperationProvider for RehomeEntityProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        vec![rehome_entity_descriptor()]
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        if entity_name.as_str() != "block" || op_name != REHOME_ENTITY_OP {
            return Err(format!(
                "RehomeEntityProvider: advertises only 'block::{REHOME_ENTITY_OP}', got \
                 '{entity_name}::{op_name}'"
            )
            .into());
        }

        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| format!("{REHOME_ENTITY_OP}: missing required parameter 'id'"))?;
        let id = EntityUri::parse(id)
            .map_err(|e| format!("{REHOME_ENTITY_OP}: 'id' is not an entity uri: {e}"))?;
        let target = params
            .get("target")
            .and_then(|v| v.as_string())
            .ok_or_else(|| format!("{REHOME_ENTITY_OP}: missing required parameter 'target'"))?;
        let RehomeTarget::HolonNative = RehomeTarget::parse(target)?;

        let children = self
            .ordering()
            .await
            .children(&id)
            .await
            .map_err(|e| format!("{REHOME_ENTITY_OP}: reading `{id}`'s children: {e}"))?;
        if !children.is_empty() {
            return Err(format!(
                "{REHOME_ENTITY_OP}: refusing to re-home `{id}` — it holds {} child block(s), and \
                 only a leaf can move home in this operation",
                children.len()
            )
            .into());
        }

        let (from, at_root) = self.placement_now(&id).await?;
        if at_root {
            // Re-parenting cannot change the home of a block already at the
            // root — a top-level page owns its file by BEING that file, which
            // this operation has no way to undo. Refused in the operation's own
            // words rather than left to `move_block`'s "Cannot move root
            // block", which names neither the block nor the reason.
            return Err(format!(
                "{REHOME_ENTITY_OP}: refusing to re-home `{id}` — it already sits at the tree \
                 root, so re-parenting it cannot move it out of any file"
            )
            .into());
        }
        if from.is_none() {
            return Err(format!(
                "{REHOME_ENTITY_OP}: refusing to re-home `{id}` — no document holds it, so it is \
                 already in Holon's own storage and there is nothing to move"
            )
            .into());
        }

        // The move, as data: `move_block` to the no-parent root leaves the
        // block with no page ancestor, which is what leaving the file means.
        let mut move_params = StorageEntity::new();
        move_params.insert("id".into(), Value::String(id.as_str().to_string()));
        move_params.insert(
            "parent_id".into(),
            Value::String(EntityUri::no_parent().as_str().to_string()),
        );
        move_params.insert("after_block_id".into(), Value::Null);
        let dispatcher = self
            .injector
            .resolve_async::<holon::api::operation_dispatcher::OperationDispatcher>()
            .await;
        let mut result = dispatcher
            .execute_operation(entity_name, "move_block", move_params)
            .await?;

        let to = self.home_now(&id).await?;
        let mut facts: HashMap<String, Value> = HashMap::new();
        facts.insert(
            "from".to_string(),
            Value::String(profile_of(from).to_string()),
        );
        facts.insert("to".to_string(), Value::String(profile_of(to).to_string()));
        facts.insert(
            REHOMING_COST_KEY.to_string(),
            Value::Array(
                rehoming_cost_between(&self.registry, from, to)?
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        result.response = Some(Value::Object(facts));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holon_native_is_the_only_home_that_can_receive_an_entity() {
        assert_eq!(
            RehomeTarget::parse("holon-native"),
            Ok(RehomeTarget::HolonNative)
        );
    }

    /// The LogSeq refusal is the WRITER's, and the message has to say so —
    /// "unknown home" would misreport a graph Holon reads perfectly well.
    #[test]
    fn logseq_db_is_refused_for_its_writers_creation_refusal() {
        let err = RehomeTarget::parse("logseq-db").expect_err("logseq-db cannot receive");
        assert!(
            err.contains("creation") && err.contains("kvs_writer"),
            "the refusal must cite the writer's own creation refusal: {err}"
        );
    }

    #[test]
    fn an_unknown_home_is_refused_by_name() {
        let err = RehomeTarget::parse("notion").expect_err("no such home");
        assert!(err.contains("notion"), "the refusal must name it: {err}");
    }

    fn shipped() -> ProfileRegistry {
        holon_capability::registry::shipped_profiles().expect("the shipped profiles parse")
    }

    /// org → holon-native is the move this operation makes, and it must come
    /// back with the clauses that pay for it.
    #[test]
    fn leaving_an_org_file_for_holon_native_has_a_stated_price() {
        let cost = rehoming_cost_between(&shipped(), Some(DurableFormat::Org), None)
            .expect("both homes are registered");
        assert!(
            !cost.is_empty(),
            "org and holon-native differ, so the move cannot be free"
        );
    }

    /// The degenerate direction still answers, and answers nothing owed —
    /// a home cannot cost anything against itself.
    #[test]
    fn a_move_between_identical_homes_costs_nothing() {
        assert!(
            rehoming_cost_between(&shipped(), None, None)
                .expect("holon-native is registered")
                .is_empty()
        );
    }
}
