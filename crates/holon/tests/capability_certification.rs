//! Certifies `assets/default/capability/holon-native.yaml` against Holon's OWN
//! substrate — Increment 2b.3.
//!
//! CV-B: the native profile is a NORMAL certified yaml, not an implicit top.
//! "The substrate can carry anything" is exactly the kind of claim that is
//! never tested because it sounds obviously true, so this drives the real
//! write → read round trip through the production operation provider and the
//! production schema, and lets the report say what is actually carried.
//!
//! The seam is `E2ETestContext`, NOT a hand-built table: a certification that
//! writes its own DDL certifies the DDL it wrote. `execute_op("block",
//! "create", …)` is the same path the app writes through.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use holon::api::backend_engine::BackendEngine;
use holon::core::queryable_cache::QueryableCache;
use holon::core::sql_block_operations::SqlBlockOperations;
use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::testing::E2ETestContext;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::repository::CoreOperations;
use holon_capability::CapabilityProfile;
use holon_capability::Carrier;
use holon_capability::CertifiableFormat;
use holon_capability::ConstructOutcome;
use holon_capability::Leg;
use holon_capability::Readback;
use holon_capability::RouteReadback;
use holon_capability::certify;
use holon_capability::clause::ClauseId;
use holon_core::OperationProvider;
use holon_loro::LoroBackend;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use tokio::runtime::Handle;

/// The substrate's SQL property carrier: the JSON blob on `block_raw`.
const BLOB_LEG: Carrier = Carrier {
    leg: Leg("block_properties_json"),
    description: "the properties JSON column the SQL operation provider writes",
};

/// The substrate's CRDT property carrier: the per-property JSON string in the
/// Loro tree's properties map.
///
/// A second leg, not a second route on `BLOB_LEG`: it is a different on-disk
/// form with a different failure mode, and it is the form that survives a
/// restart.
const LORO_LEG: Carrier = Carrier {
    leg: Leg("loro_properties_map"),
    description: "the per-property JSON string the Loro backend writes into a node's meta",
};

/// FK anchor the production core schema seeds; every root block needs it.
const ROOT_PARENT: &str = "sentinel:no_parent";

struct HolonNative {
    profile: CapabilityProfile,
    ctx: E2ETestContext,
    loro: Arc<LoroBackend>,
}

impl HolonNative {
    async fn load() -> anyhow::Result<Self> {
        let path = std::env::var_os("HOLON_CAPABILITY_PROFILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../assets/default/capability/holon-native.yaml")
            });
        let doc = Arc::new(
            holon_loro::LoroDocument::new("certify".to_string())
                .map_err(|e| anyhow::anyhow!("the certification Loro doc must open: {e:?}"))?,
        );
        let native = Self {
            profile: CapabilityProfile::from_path(&path)
                .context("the holon-native profile must load")?,
            ctx: E2ETestContext::from_engine(block_engine().await?),
            loro: Arc::new(LoroBackend::from_document(doc)),
        };
        // CONTROL, and it is load-bearing: without it a broken WRITE PATH
        // reaches the report as `Readback::Refused` and reads exactly like a
        // format boundary rejecting the value. The first run of this harness
        // did precisely that and accused the substrate of refusing a plain
        // string.
        //
        // It guarantees LESS than "the wiring is right": deleting the
        // `SqlBlockOperations` registration below leaves this control green
        // and every counter identical, because no probe here drives a
        // structural operation. It catches a write path that cannot write, not
        // every wiring change.
        let control = native
            .write_then_read(
                "certify-control",
                "Control",
                &Value::String("carried".into()),
            )
            .await?;
        anyhow::ensure!(
            control == Readback::Present(Value::String("carried".into())),
            "the certification harness is not wired: an ordinary string on an unreserved key \
             must round-trip, got {control:?}"
        );
        // The `set_field` route deliberately gets NO round-trip control, and the
        // reason is a MEASUREMENT: an ordinary key written through it reads back
        // `Absent` here, because `set_field` offers the write to the
        // `BlockCellRegistry` first (sql_block_operations.rs:1052) and returns
        // `Ok` with no synchronous change once Loro takes it, while no outbound
        // projector runs in this wiring.
        //
        // That does not weaken the reserved-key probe, because the two outcomes
        // are already discriminable: a dead route answers `Ok` → `Absent`, and
        // only a real refusal can answer `Refused` with the KEY and the OP named
        // in its reason. The probe requires the latter, so it cannot be
        // satisfied by silence. What it certifies is the ENGINE boundary — which
        // is where the refusal lives and is wiring-independent — not this
        // wiring's storage leg.
        Ok(native)
    }

    /// Bridge the SYNC certifier onto the async substrate.
    ///
    /// `block_in_place` is what makes this legal inside a multi-thread test
    /// runtime; a bare `block_on` on a runtime thread would panic.
    fn blocking<T>(&self, fut: impl std::future::Future<Output = T>) -> T {
        tokio::task::block_in_place(|| Handle::current().block_on(fut))
    }

    /// Write a block carrying `tags`, then read the set back out of
    /// `block_tags` — the junction the substrate actually stores them in.
    ///
    /// Not `block_raw`: `tags` is an EDGE field with no column there
    /// (undo.rs:611 records the same fact), so a probe reading the blob would
    /// report every tag lost and blame the substrate for looking in the wrong
    /// place.
    async fn tags_after_write(&self, id: &str, tags: &[&str]) -> anyhow::Result<Vec<String>> {
        let mut params: holon_api::StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("content".into(), Value::String("certify".to_string()));
        params.insert("parent_id".into(), Value::String(ROOT_PARENT.to_string()));
        // An ARRAY, not a CSV string: `tags` is an edge field and the provider
        // panics by name on any other shape (sql_operation_provider.rs:532).
        params.insert(
            "tags".into(),
            Value::Array(
                tags.iter()
                    .map(|t| Value::String((*t).to_string()))
                    .collect(),
            ),
        );
        self.ctx
            .execute_op("block", "create", params)
            .await
            .map_err(|e| {
                anyhow::anyhow!("the create must land for the probe to mean anything: {e:#}")
            })?;

        self.tags_now(id).await
    }

    /// Replace an existing block's tag set through the production update path.
    async fn tags_after_update(&self, id: &str, tags: &[&str]) -> anyhow::Result<Vec<String>> {
        let mut params: holon_api::StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(format!("block:{id}")));
        params.insert(
            "tags".into(),
            Value::Array(
                tags.iter()
                    .map(|t| Value::String((*t).to_string()))
                    .collect(),
            ),
        );
        self.ctx
            .execute_op("block", "update", params)
            .await
            .map_err(|e| {
                anyhow::anyhow!("the update must land for the probe to mean anything: {e:#}")
            })?;
        self.tags_now(id).await
    }

    /// The tag set the junction currently holds for `id`.
    async fn tags_now(&self, id: &str) -> anyhow::Result<Vec<String>> {
        let sql = format!(
            "SELECT tag FROM block_tags WHERE block_id = 'block:{}'",
            id.replace('\'', "''")
        );
        let rows = self
            .ctx
            .engine()
            .db_handle()
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("reading block_tags failed: {e}"))?;
        // A non-string tag row is a named error, not a skip: dropping one
        // would under-report the set and read as a lost tag.
        let mut tags = Vec::new();
        for row in &rows {
            match row.get("tag") {
                Some(Value::String(tag)) => tags.push(tag.clone()),
                other => anyhow::bail!(
                    "block_tags.tag for {id} came back as {other:?}, which is not a tag name"
                ),
            }
        }
        Ok(tags)
    }

    /// Write one property through the production create path and read the
    /// stored blob back.
    async fn write_then_read(
        &self,
        id: &str,
        key: &str,
        value: &Value,
    ) -> anyhow::Result<Readback> {
        let mut params: holon_api::StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("content".into(), Value::String("certify".to_string()));
        params.insert("parent_id".into(), Value::String(ROOT_PARENT.to_string()));
        params.insert(key.into(), value.clone());

        if let Err(e) = self.ctx.execute_op("block", "create", params).await {
            // A refusal at the write boundary is the law's other legal branch.
            return Ok(Readback::Refused {
                reason: format!("{e:#}"),
            });
        }
        self.read_stored(id, key).await
    }

    /// The SECOND author-reachable route into the same leg: `set_field` names
    /// the property key in a param VALUE rather than a param key, so a refusal
    /// that inspects only param keys never sees it (the D5.a verifier's finding
    /// — the forged stamp persisted and `history_store.rs:55` read it as
    /// authoritative).
    async fn set_field_then_read(
        &self,
        id: &str,
        key: &str,
        value: &Value,
    ) -> anyhow::Result<Readback> {
        // `set_field` updates; it does not create. A missing anchor would make
        // the probe report a refusal that is about the absent block, not the
        // key — so the anchor's own failure is a harness fault, loudly.
        let mut anchor: holon_api::StorageEntity = HashMap::new();
        anchor.insert("id".into(), Value::String(id.to_string()));
        anchor.insert("content".into(), Value::String("certify".to_string()));
        anchor.insert("parent_id".into(), Value::String(ROOT_PARENT.to_string()));
        self.ctx
            .execute_op("block", "create", anchor)
            .await
            .map_err(|e| {
                anyhow::anyhow!("the set_field probe's anchor block must be creatable: {e:#}")
            })?;

        let mut params: holon_api::StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("field".into(), Value::String(key.to_string()));
        params.insert("value".into(), value.clone());
        if let Err(e) = self.ctx.execute_op("block", "set_field", params).await {
            return Ok(Readback::Refused {
                reason: format!("{e:#}"),
            });
        }
        self.read_stored(id, key).await
    }

    /// The THIRD route: hand over the whole property BAG, with the key one
    /// level deeper inside it. `properties` is a real `block_raw` column, so
    /// this takes the direct-column branch and REPLACES the blob rather than
    /// merging — a refusal that inspects only the NAMED field sees
    /// `field="properties"`, which is not engine-owned, and lets it through.
    async fn set_field_bag_then_read(
        &self,
        id: &str,
        key: &str,
        value: &Value,
    ) -> anyhow::Result<Readback> {
        let mut anchor: holon_api::StorageEntity = HashMap::new();
        anchor.insert("id".into(), Value::String(id.to_string()));
        anchor.insert("content".into(), Value::String("certify".to_string()));
        anchor.insert("parent_id".into(), Value::String(ROOT_PARENT.to_string()));
        self.ctx
            .execute_op("block", "create", anchor)
            .await
            .map_err(|e| {
                anyhow::anyhow!("the bag probe's anchor block must be creatable: {e:#}")
            })?;

        let json: serde_json::Value = value.clone().into();
        let bag = serde_json::json!({ key: json });
        let mut params: holon_api::StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("field".into(), Value::String("properties".to_string()));
        params.insert("value".into(), Value::String(bag.to_string()));
        if let Err(e) = self.ctx.execute_op("block", "set_field", params).await {
            return Ok(Readback::Refused {
                reason: format!("{e:#}"),
            });
        }
        self.read_stored(id, key).await
    }

    /// Write one property through the Loro create path and read it back off
    /// the tree — the CRDT leg's own author path, not the SQL one.
    async fn loro_write_then_read(
        &self,
        id: &str,
        key: &str,
        value: &Value,
    ) -> anyhow::Result<Readback> {
        let props = HashMap::from([(key.to_string(), value.clone())]);
        if let Err(e) = self
            .loro
            .create_block_with_properties(
                holon_api::EntityUri::no_parent(),
                holon_api::block::BlockContent::text("certify"),
                Some(holon_api::EntityUri::block(id)),
                &props,
                &holon_api::BlockEdges::default(),
            )
            .await
        {
            return Ok(Readback::Refused {
                reason: format!("{e:?}"),
            });
        }
        self.loro_read(id, key).await
    }

    /// The SECOND author route into the same leg: `update_block_properties`
    /// MERGES rather than replacing, so it reaches the encode choke point
    /// through a different writer than `create` does.
    async fn loro_update_then_read(
        &self,
        id: &str,
        key: &str,
        value: &Value,
    ) -> anyhow::Result<Readback> {
        self.loro
            .create_block_with_properties(
                holon_api::EntityUri::no_parent(),
                holon_api::block::BlockContent::text("certify"),
                Some(holon_api::EntityUri::block(id)),
                &HashMap::new(),
                &holon_api::BlockEdges::default(),
            )
            .await
            .map_err(|e| {
                anyhow::anyhow!("the Loro update probe's anchor must be creatable: {e:?}")
            })?;
        let props = HashMap::from([(key.to_string(), value.clone())]);
        if let Err(e) = self
            .loro
            .update_block_properties(&format!("block:{id}"), &props)
            .await
        {
            return Ok(Readback::Refused {
                reason: format!("{e:?}"),
            });
        }
        self.loro_read(id, key).await
    }

    async fn loro_read(&self, id: &str, key: &str) -> anyhow::Result<Readback> {
        let block = self
            .loro
            .get_block(&format!("block:{id}"))
            .await
            // NOT `Absent`: an unreadable block is a harness fault, and
            // reporting it as a lost property would blame the leg for it.
            .map_err(|e| {
                anyhow::anyhow!("the Loro probe wrote {id:?} but cannot read it: {e:?}")
            })?;
        Ok(match block.properties.get(key) {
            None => Readback::Absent,
            Some(found) => Readback::Present(found.clone()),
        })
    }

    /// Read one property back out of the stored blob.
    async fn read_stored(&self, id: &str, key: &str) -> anyhow::Result<Readback> {
        // The write path promotes a bare id to its `block:` URI at the
        // boundary (MEASURED: the first read missed every row), so the read
        // must ask for the stored form.
        let sql = format!(
            "SELECT properties, property_kinds FROM block_raw WHERE id = 'block:{}'",
            id.replace('\'', "''")
        );
        let rows = self
            .ctx
            .engine()
            .db_handle()
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("reading the stored properties failed: {e}"))?;
        let Some(row) = rows.into_iter().next() else {
            // NOT `Absent`: no row means the write did not land where the read
            // looks, which is a harness fault. Reporting it as a lost property
            // would blame the substrate for the probe's own mistake.
            let all = self
                .ctx
                .engine()
                .db_handle()
                .query("SELECT id FROM block_raw", HashMap::new())
                .await
                .map_err(|e| anyhow::anyhow!("listing block ids failed: {e}"))?;
            let ids: Vec<String> = all
                .iter()
                .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(str::to_string))
                .collect();
            anyhow::bail!(
                "the probe wrote id {id:?} but no such row exists; block_raw holds {ids:?}"
            );
        };
        // The column's shape is the harness's business, not the format's: an
        // unexpected shape means the probe is reading the wrong thing, and
        // calling that a dropped property would blame the substrate.
        let stored_column = row
            .get("properties")
            .cloned()
            .context("block_raw must expose a properties column")?;
        // The production read boundary (`normalize_known_json_columns`) has
        // already parsed the blob and restored the kinds `property_kinds`
        // records, so the bag is read as it stands. Re-serializing it to text
        // and parsing that back would undo exactly the retyping under test.
        let bag = match &stored_column {
            // NULL is the honest empty blob: the row exists and carries no
            // properties at all.
            Value::Null => Default::default(),
            Value::Object(map) => map.clone(),
            other => anyhow::bail!("the properties column came back as {other:?}"),
        };
        Ok(match bag.get(key) {
            None => Readback::Absent,
            Some(found) => Readback::Present(found.clone()),
        })
    }
}

/// The PRODUCTION SqlOnly block wiring: the CRUD authority plus the structural
/// provider that owns `move_block`.
///
/// Registered explicitly because `E2ETestContext::new()` boots an engine with
/// NO block provider, and a certification that cannot write is a certification
/// of nothing.
async fn block_engine() -> anyhow::Result<Arc<BackendEngine>> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                )) as Arc<dyn OperationProvider>
            })
            .with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                ));
                let mut block_raw_type_def = Block::type_definition();
                block_raw_type_def.name = BLOCK_WRITE_TABLE.to_string();
                let cache = tokio::task::block_in_place(|| {
                    let handle = Handle::current();
                    // ALLOW(block_on): wrapped in `block_in_place`, which is what makes a
                    // blocking wait legal on a multi-thread runtime thread (same contract as
                    // the `blocking` helper above).
                    handle.block_on(QueryableCache::<Block>::new(db_handle, block_raw_type_def))
                })
                .expect("block_raw cache");
                Arc::new(SqlBlockOperations::new(sql_ops, Arc::new(cache)))
                    as Arc<dyn OperationProvider>
            })
    })
    .await
    .context("the certification engine must boot with the block provider")
}

/// A distinct block id per probe: two probes sharing one id would certify the
/// SECOND write against the first one's row.
fn probe_id(key: &str, value: &Value) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&format!("{key}{value:?}"), &mut hasher);
    format!("certify-{:x}", std::hash::Hasher::finish(&hasher))
}

impl CertifiableFormat for HolonNative {
    fn profile(&self) -> &CapabilityProfile {
        &self.profile
    }

    /// `LORO_LEG` is deliberately NOT here — see
    /// `the_loro_leg_keeps_every_declared_kind`, which drives it directly.
    ///
    /// `certify` runs EVERY clause over every carrier, including
    /// `property_keys.engine_owned_keys`. That clause asks whether AUTHORING
    /// `_provenance` is refused, and the refusal lives at the operation engine
    /// (`operation_engine.rs:400`), which then STAMPS the key itself
    /// (`:423-426`) — so every storage leg must accept it. `BLOB_LEG` drives
    /// the clause honestly because its probe goes through `execute_op`; a
    /// storage-leg probe cannot, and adding one here would report the engine's
    /// own stamp as a violation of the leg.
    fn carriers(&self) -> &'static [Carrier] {
        &[BLOB_LEG]
    }

    fn round_trip_property(
        &self,
        carrier: Carrier,
        key: &str,
        value: &Value,
    ) -> anyhow::Result<Readback> {
        let id = probe_id(key, value);
        if carrier.leg == LORO_LEG.leg {
            self.blocking(self.loro_write_then_read(&id, key, value))
        } else {
            self.blocking(self.write_then_read(&id, key, value))
        }
    }

    /// `block_properties_json` has TWO authored write routes, and the second
    /// one names its key in a param VALUE — so `create` alone measures half the
    /// leg. Declared here rather than as a second `Carrier` because it is the
    /// same storage leg, and giving it a leg of its own would run every other
    /// axis probe through an operation that cannot create a block.
    fn extra_property_write_routes(
        &self,
        carrier: Carrier,
        key: &str,
        value: &Value,
    ) -> anyhow::Result<Option<Vec<RouteReadback>>> {
        if carrier.leg == LORO_LEG.leg {
            // The Loro leg's own second author route. `set_field` and the bag
            // route below are SQL-side operations and say nothing about it.
            let up = format!("{}-loro-up", probe_id(key, value));
            return Ok(Some(vec![RouteReadback {
                route: "update_block_properties",
                readback: self.blocking(self.loro_update_then_read(&up, key, value))?,
            }]));
        }
        let sf = format!("{}-sf", probe_id(key, value));
        let bag = format!("{}-bag", probe_id(key, value));
        Ok(Some(vec![
            RouteReadback {
                route: "set_field",
                readback: self.blocking(self.set_field_then_read(&sf, key, value))?,
            },
            // THIRD route: the key one level deeper, inside the property BAG.
            // `properties` is a real column, so this replaces the whole blob —
            // a refusal reading only the NAMED field never sees the key.
            RouteReadback {
                route: "set_field(properties bag)",
                readback: self.blocking(self.set_field_bag_then_read(&bag, key, value))?,
            },
        ]))
    }

    /// Attach: write a block carrying a tag and read the junction back.
    fn attach_existing_tag(&self) -> anyhow::Result<Option<ConstructOutcome>> {
        self.blocking(async {
            let back = self
                .tags_after_write("certify-tags-attach", &["keep", "added"])
                .await?;
            Ok(Some(
                if back.iter().any(|t| t == "added") && back.iter().any(|t| t == "keep") {
                    ConstructOutcome::Survived
                } else if back.iter().any(|t| t == "added") {
                    ConstructOutcome::Changed {
                        got: format!("{back:?}"),
                    }
                } else {
                    ConstructOutcome::Lost
                },
            ))
        })
    }

    /// Detach: write BOTH tags, then update to one, and require the junction
    /// to hold the survivor and not the dropped name.
    ///
    /// The tag really is present first. A probe that created the block with
    /// the smaller set would find the name absent — but it was never there,
    /// so "gone" would be true before the write and the clause would confirm
    /// on nothing.
    fn detach_existing_tag(&self) -> anyhow::Result<Option<ConstructOutcome>> {
        self.blocking(async {
            let id = "certify-tags-detach";
            let staged = self.tags_after_write(id, &["keep", "drop"]).await?;
            anyhow::ensure!(
                staged.iter().any(|t| t == "drop"),
                "the setup must really attach the tag it then removes; got {staged:?}"
            );
            let back = self.tags_after_update(id, &["keep"]).await?;
            Ok(Some(
                if !back.iter().any(|t| t == "drop") && back.iter().any(|t| t == "keep") {
                    ConstructOutcome::Survived
                } else if !back.iter().any(|t| t == "keep") {
                    ConstructOutcome::Lost
                } else {
                    ConstructOutcome::Changed {
                        got: format!("{back:?}"),
                    }
                },
            ))
        })
    }

    /// A tag name nothing has used before.
    ///
    /// The substrate stores tags as strings in a junction, so there is no
    /// entity for a name to fail to resolve to: the two observable answers are
    /// REFUSED and carried-into-existence.
    fn reference_unknown_tag(&self) -> anyhow::Result<Option<ConstructOutcome>> {
        self.blocking(async {
            let back = self
                .tags_after_write("certify-tags-unknown", &["neverseenbefore"])
                .await?;
            Ok(Some(if back.iter().any(|t| t == "neverseenbefore") {
                ConstructOutcome::Survived
            } else {
                ConstructOutcome::Lost
            }))
        })
    }
}

/// The increment in one assertion: every restriction the native profile
/// declares is REAL, measured against the production substrate.
#[tokio::test(flavor = "multi_thread")]
async fn the_native_profile_declares_only_restrictions_that_are_real() -> anyhow::Result<()> {
    let format = HolonNative::load().await?;
    let report = certify(&format).context("the certification harness must run")?;

    println!("{}", report.render());

    let dir = std::env::var_os("HOLON_CAPABILITY_REPORT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/capability-certification")
        });
    let written = report.write_report(format.profile().id(), &dir)?;
    println!("report: {}", written.display());

    assert!(
        report.confirmed > 0,
        "a run that generated NOTHING must not pass as clean:\n{}",
        report.render()
    );
    assert!(
        report.is_clean(),
        "the holon-native profile declares {} restriction(s) the substrate does not honour, and \
         {} coverage gap(s):\n{}",
        report.violations.len(),
        report.gaps.len(),
        report.render()
    );
    Ok(())
}

/// S2 — the profile's `types` claim holds on the LORO leg too, on every one of
/// its author routes.
///
/// The claim is a claim about the substrate, and the substrate has two durable
/// property legs. Before S2 the Loro leg answered `String` for `date_time` and
/// `json`: `Value` is untagged, so both serialize as bare JSON strings and the
/// first matching variant on the way back is `String`. The kinds now travel in
/// an envelope inside the value (`holon-pattern/src/kind_envelope.rs`).
///
/// Driven here rather than through `carriers()` for the reason given on
/// `HolonNative::carriers` — this drives the clause `carriers()` cannot.
#[tokio::test(flavor = "multi_thread")]
async fn the_loro_leg_keeps_every_declared_kind() -> anyhow::Result<()> {
    let format = HolonNative::load().await?;
    // Read from the PROFILE, so a kind added to the yaml is driven here
    // without anyone remembering to add it.
    let declared = &format.profile().property_values().types;
    assert!(
        declared.contains(&holon_capability::axes::ValueKind::DateTime)
            && declared.contains(&holon_capability::axes::ValueKind::Json),
        "this test exists for the two ambiguous kinds; the profile no longer declares them"
    );

    let key = "Probe";
    for kind in declared {
        let sent = specimen(*kind);
        let mut routes = vec![RouteReadback {
            route: "create",
            readback: format.round_trip_property(LORO_LEG, key, &sent)?,
        }];
        routes.extend(
            format
                .extra_property_write_routes(LORO_LEG, key, &sent)?
                .unwrap_or_default(),
        );
        for RouteReadback { route, readback } in routes {
            assert_eq!(
                readback,
                Readback::Present(sent.clone()),
                "the profile declares {kind:?} carried, but the Loro leg answered {readback:?} \
                 on route {route}"
            );
        }
    }
    Ok(())
}

/// One inhabitant per declared kind. A local copy of the certifier's own
/// specimens, which are private to it.
fn specimen(kind: holon_capability::axes::ValueKind) -> Value {
    use holon_capability::axes::ValueKind;
    match kind {
        ValueKind::String => Value::String("plain".to_string()),
        ValueKind::Integer => Value::Integer(42),
        ValueKind::Float => Value::Float(1.5),
        ValueKind::Boolean => Value::Boolean(true),
        ValueKind::DateTime => Value::DateTime("2026-08-22T10:00:00Z".to_string()),
        ValueKind::Json => Value::Json(r#"{"a":1}"#.to_string()),
        ValueKind::Array => Value::Array(vec![Value::String("one".to_string())]),
        ValueKind::Object => Value::Object(Default::default()),
        ValueKind::Null => Value::Null,
    }
}

/// Ruling D5.a at the production write boundary: the engine mints
/// `_provenance`, so an authored one is a NAMED refusal — never the silent
/// replacement that would report success while discarding the author's value.
#[tokio::test(flavor = "multi_thread")]
async fn an_authored_engine_owned_key_is_refused_at_the_write_boundary() -> anyhow::Result<()> {
    let native = HolonNative::load().await?;
    for key in holon_api::ENGINE_OWNED_PARAM_KEYS {
        for op in ["create", "update"] {
            let mut params: holon_api::StorageEntity = HashMap::new();
            params.insert("id".into(), Value::String(format!("d5a-{op}")).clone());
            params.insert("content".into(), Value::String("authored".to_string()));
            params.insert("parent_id".into(), Value::String(ROOT_PARENT.to_string()));
            params.insert((*key).into(), Value::String("authored-by-hand".to_string()));

            let err = native
                .ctx
                .execute_op("block", op, params)
                .await
                .err()
                .with_context(|| {
                    format!("'{op}' carrying the engine-owned '{key}' must be REFUSED")
                })?;
            let msg = format!("{err:#}");
            anyhow::ensure!(
                msg.contains(key) && msg.contains(op),
                "the refusal must name the offending key and the operation, got: {msg}"
            );
        }

        // ROUTE 2 — `set_field` names the key in a param VALUE. Refusing only
        // param KEYS leaves this open, and it is STRICTLY worse than the
        // silent-replace it replaces: the forged stamp PERSISTS and
        // `history_store.rs:55` / `trust_proposals_matview.sql:6-10` read it as
        // authoritative, so any origin that can set_field could name another
        // origin as the author.
        let readback = native
            .set_field_then_read(
                "d5a-set-field",
                key,
                &Value::String("forged-by-set-field".to_string()),
            )
            .await?;
        match readback {
            Readback::Refused { reason } => anyhow::ensure!(
                reason.contains(key) && reason.contains("set_field"),
                "the set_field refusal must name the offending key and the operation, got: \
                 {reason}"
            ),
            other => anyhow::bail!(
                "set_field(field={key:?}) must be REFUSED — it is a second author-reachable \
                 route into the same declared leg — but the substrate answered {other:?}"
            ),
        }

        // ROUTE 3 — the key one level deeper, inside the property BAG.
        // `properties` is a real column, so this REPLACES the whole blob and a
        // refusal reading only the named field waves it through.
        let via_bag = native
            .set_field_bag_then_read(
                "d5a-bag",
                key,
                &Value::String("forged-in-a-bag".to_string()),
            )
            .await?;
        match via_bag {
            Readback::Refused { reason } => anyhow::ensure!(
                reason.contains(key) && reason.contains("set_field"),
                "the bag refusal must name the offending key and the operation, got: {reason}"
            ),
            other => anyhow::bail!(
                "set_field(field=\"properties\") carrying {key:?} inside the bag must be \
                 REFUSED — it is a third author-reachable route into the same declared leg — \
                 but the substrate answered {other:?}"
            ),
        }
    }
    Ok(())
}

/// The promotion story's price tag, between the two REAL profiles.
///
/// Not a fixture comparison: these are the yamls the two harnesses certify, so
/// a loss reported here is a loss a user would actually take. It is also the
/// directionality proof — the two moves cost different things, and a diff that
/// returned the same answer both ways would be comparing sets, not homes.
#[test]
fn moving_between_the_two_certified_homes_has_a_price_in_both_directions() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let native =
        CapabilityProfile::from_path(root.join("assets/default/capability/holon-native.yaml"))
            .expect("the native profile loads");
    let org = CapabilityProfile::from_path(root.join("crates/holon-org-format/profile.yaml"))
        .expect("the org profile loads");

    let to_org = native.diff(&org);
    let kinds = to_org
        .iter()
        .find(|l| l.clause == ClauseId::PropertyValuesTypes)
        .unwrap_or_else(|| panic!("org declares types: [string]; the other kinds must be reported as lost:\n{to_org:#?}"));
    assert!(
        kinds.source.contains("Integer"),
        "the loss must name the kinds that do not fit: {kinds}"
    );

    let to_native = org.diff(&native);
    assert!(
        to_native
            .iter()
            .any(|l| l.clause == ClauseId::ContentInlineConstructs),
        "org carries inline constructs the substrate does not declare:\n{to_native:#?}"
    );
    assert!(
        to_native
            .iter()
            .any(|l| l.clause == ClauseId::PropertyValuesReferenceValues),
        "org carries references; the substrate declares none, so the LINK is the price:\n{to_native:#?}"
    );
}

/// NV-1 (ruling D29.a): the two kinds JSON cannot spell survive the schemaless
/// bag, because `property_kinds` carries the kind beside it.
///
/// Drives the production write path (`execute_op("block", "create", …)`) and
/// the production read boundary, so a kind that survives only inside the test's
/// own head cannot pass.
#[tokio::test(flavor = "multi_thread")]
async fn date_time_and_json_keep_their_kind_across_the_properties_bag() -> anyhow::Result<()> {
    let native = HolonNative::load().await?;

    let when = Value::DateTime("2026-08-22T10:00:00Z".to_string());
    assert_eq!(
        native
            .write_then_read("nv1-datetime", "when", &when)
            .await?,
        Readback::Present(when.clone()),
        "a DateTime must not come back as the String JSON kept"
    );

    let doc = Value::Json(r#"{"a":1}"#.to_string());
    assert_eq!(
        native.write_then_read("nv1-json", "doc", &doc).await?,
        Readback::Present(doc.clone()),
        "a Json document must not come back as the Object JSON parsed it into"
    );

    // The neighbours must not be dragged along: a plain string that merely
    // LOOKS like a timestamp has no kind entry and stays a String.
    assert_eq!(
        native
            .write_then_read(
                "nv1-plain",
                "looks-like-a-date",
                &Value::String("2026-08-22T10:00:00Z".to_string()),
            )
            .await?,
        Readback::Present(Value::String("2026-08-22T10:00:00Z".to_string())),
        "only the keys the kind map names may be re-typed"
    );
    Ok(())
}

/// A kind entry the stored value cannot inhabit is corruption, and the read
/// boundary says so instead of handing back a plausible `Value::String`.
#[tokio::test(flavor = "multi_thread")]
async fn a_kind_map_disagreeing_with_its_bag_fails_the_read_loudly() -> anyhow::Result<()> {
    let native = HolonNative::load().await?;
    let id = "nv1-corrupt";
    native
        .write_then_read(
            id,
            "when",
            &Value::DateTime("2026-08-22T10:00:00Z".to_string()),
        )
        .await?;

    // Overwrite the VALUE while leaving the kind map claiming a date_time —
    // the shape a partial or foreign write would leave behind.
    // ALLOW(sole_block_writer): fabricates the corrupt state under test.
    native
        .ctx
        .engine()
        .db_handle()
        .execute_values(
            &format!(
                "UPDATE block_raw SET properties = '{{\"when\":\"not a date\"}}' WHERE id = \
                 'block:{id}'"
            ),
            vec![],
        )
        .await
        .map_err(|e| anyhow::anyhow!("the corrupting write must land: {e}"))?;

    let err = native
        .read_stored(id, "when")
        .await
        .expect_err("a date_time kind over a non-date must fail the read, not read as a String");
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("when") && rendered.contains("date_time"),
        "the error must name the key and the kind that disagree, got: {rendered}"
    );
    Ok(())
}

/// The routes this wiring CANNOT drive, pinned exactly.
///
/// `undriven_routes` is printed with the report, and an ordinary captured
/// `cargo nextest` run swallows a passing test's stdout — so the disclosure is
/// invisible in the normal gate. Asserting the set makes GROWTH fail: a route
/// that silently stops writing later, and so stops certifying the types clause,
/// turns this red instead of quietly shrinking coverage.
///
/// `set_field` and the bag form both reach `SqlBlockOperations`, which offers
/// the write to the `BlockCellRegistry` and returns `Ok` with no synchronous
/// SQL write, so `SqlOperationProvider::set_field` is never reached here. That
/// leg is driven directly in
/// `holon::core::sql_operation_provider::set_field_property_kinds_test`.
#[tokio::test(flavor = "multi_thread")]
async fn exactly_the_two_set_field_routes_are_undriven() -> anyhow::Result<()> {
    let format = HolonNative::load().await?;
    let report = certify(&format).context("the certification harness must run")?;

    let mut got: Vec<String> = report
        .undriven_routes
        .iter()
        .map(|r| r.split(':').next().unwrap_or(r).to_string())
        .collect();
    got.sort();

    assert_eq!(
        got,
        vec![
            "block_properties_json/set_field".to_string(),
            "block_properties_json/set_field(properties bag)".to_string(),
        ],
        "the undriven-route set changed. A route that GAINED coverage is good news — update \
         this list. A route that LOST it means the types clause is now certified over fewer \
         author paths than the profile claims:\n{}",
        report.render()
    );
    Ok(())
}
