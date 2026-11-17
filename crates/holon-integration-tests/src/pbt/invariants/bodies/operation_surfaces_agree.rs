//! The comparison every operation-surface invariant shares: the dispatcher
//! catalog carries the required operations, and the profile resolver and the
//! last re-render offer exactly the catalog.

use std::collections::BTreeSet;
use std::fmt;
use std::time::Duration;
use std::time::Instant;

use holon_pbt_core::capabilities::OperationSurfaces;

/// The first clause of the agreement that the surfaces break.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clause {
    MissingRequired,
    ProfileDiffers,
    RerenderedDiffers,
}

impl Clause {
    fn of(required: &BTreeSet<String>, surfaces: &OperationSurfaces) -> Option<Self> {
        if !surfaces.dispatcher.is_superset(required) {
            Some(Self::MissingRequired)
        } else if surfaces.profile != surfaces.dispatcher {
            Some(Self::ProfileDiffers)
        } else if surfaces
            .rerendered
            .as_ref()
            .is_some_and(|rerendered| rerendered != &surfaces.dispatcher)
        {
            Some(Self::RerenderedDiffers)
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct SurfacesDisagree {
    pub clause: Clause,
    pub required: BTreeSet<String>,
    pub surfaces: OperationSurfaces,
}

impl fmt::Display for SurfacesDisagree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let clause = match self.clause {
            Clause::MissingRequired => "the dispatcher misses required operations",
            Clause::ProfileDiffers => "the profile offers different operations than the dispatcher",
            Clause::RerenderedDiffers => {
                "the re-rendered row offers different operations than the dispatcher"
            }
        };
        write!(
            f,
            "{clause}\n  required:   {:?}\n  dispatcher: {:?}\n  profile:    {:?}\n  rerendered: {:?}",
            self.required,
            self.surfaces.dispatcher,
            self.surfaces.profile,
            self.surfaces.rerendered
        )
    }
}

/// Poll `read` until the surfaces agree. The profile resolver learns of a
/// registration asynchronously, so a disagreement counts only once it outlives
/// the deadline; the last surfaces read are the error.
pub async fn operation_surfaces_agree(
    required: &BTreeSet<String>,
    mut read: impl AsyncFnMut() -> OperationSurfaces,
) -> Result<OperationSurfaces, SurfacesDisagree> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let surfaces = read().await;
        let Some(clause) = Clause::of(required, &surfaces) else {
            return Ok(surfaces);
        };
        if Instant::now() >= deadline {
            return Err(SurfacesDisagree {
                clause,
                required: required.clone(),
                surfaces,
            });
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
