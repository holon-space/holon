//! The id check every row producer owes the write path.

use anyhow::Result;
use anyhow::bail;
use holon_api::EntityUri;

/// Parse a file-derived local id into the [`EntityUri`] it will be STORED as.
///
/// A file keys its rows in its own keyspace — bare ids, as org and cook files
/// store them (`docs/Reference/ORG_SYNTAX.md`). This is the boundary that adds
/// the scheme, so everything downstream carries a reference that names its
/// entity and the operation boundary has nothing left to add.
///
/// It refuses what would not LAND as `{entity}:{local}`, catching two silent
/// failures at once: a file name the URI grammar rejects (a space) would panic
/// inside a spawned ingest task, and one that parses as an ALREADY-schemed URI
/// (a `:` in the path) would be stored unprefixed, leaving every reference to
/// it joining to nothing.
pub fn parse_local_id(entity: &str, local: &str) -> Result<EntityUri> {
    let intended = format!("{entity}:{local}");
    if EntityUri::parse(&intended).is_err() {
        bail!(
            "derived {entity} id {local:?} is not a storable URI path. Rename the file to one \
             the id grammar admits."
        );
    }
    let landed = EntityUri::from_raw_for(entity, local).to_string();
    if landed != intended {
        bail!(
            "derived {entity} id {local:?} would land as {landed:?} rather than {intended:?} — it \
             already reads as a schemed URI, so it is stored unprefixed and every reference to it \
             joins to nothing. Rename the file."
        );
    }
    EntityUri::parse(&intended)
}
