//! The id check every row producer owes the write path.

use anyhow::Result;
use anyhow::bail;
use holon_api::EntityUri;

/// Refuse a derived id that would not LAND as `{entity}:{local}`.
///
/// The check is the write path's own `from_raw_for`, so it catches both silent
/// failures at once: a file name the URI grammar rejects (a space) would panic
/// inside a spawned ingest task, and one that parses as an ALREADY-schemed URI
/// (a `:` in the path) would be stored unprefixed, leaving every reference to
/// it joining to nothing.
pub fn checked_local_id(entity: &str, local: &str) -> Result<()> {
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
    Ok(())
}
