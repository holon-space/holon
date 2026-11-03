//! What this device knows of the history every paired device holds, so that
//! history compaction never trims below it.
//!
//! Each sync handshake carries the sender's own version vector and its map
//! `device -> last known version vector` ([`Hello`]). The receiver merges both
//! into its own map, per device the larger vector. Every entry is a version the
//! receiver itself holds after the round, so the meet of the map
//! ([`PeerVersions::floor`]) and the own version is history every device
//! already has: an
//! update any of them sends later depends only on ops above it.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use loro::VersionVector;
use parking_lot::Mutex;
use serde::Deserialize;
use serde::Serialize;

const FILE_NAME: &str = "peer-versions.json";

/// The version map one handshake side sends. `device` is `None` for a
/// document no ledger tracks (a share document); its receiver records nothing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hello {
    pub device: Option<u64>,
    pub vv: Vec<u8>,
    pub known: Vec<(u64, Vec<u8>)>,
}

impl Hello {
    pub fn untracked(vv: &VersionVector) -> Self {
        Self {
            device: None,
            vv: vv.encode(),
            known: Vec::new(),
        }
    }

    pub fn version(&self) -> Result<VersionVector> {
        VersionVector::decode(&self.vv).context("decode the version vector of a sync hello")
    }
}

#[derive(Serialize, Deserialize)]
struct Ledger {
    device: u64,
    known: BTreeMap<u64, Vec<u8>>,
}

/// The persisted map `device -> last known version vector` of one replicated
/// document.
pub struct PeerVersions {
    path: Option<PathBuf>,
    device: u64,
    known: Mutex<BTreeMap<u64, VersionVector>>,
}

impl PeerVersions {
    /// The ledger stored in `dir`, made with a fresh device id on first use.
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join(FILE_NAME);
        let ledger = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Ledger>(&bytes)
                .with_context(|| format!("parse the peer version ledger {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ledger {
                device: rand::random(),
                known: BTreeMap::new(),
            },
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("read the peer version ledger {}", path.display()));
            }
        };
        let known = ledger
            .known
            .iter()
            .map(|(device, bytes)| {
                VersionVector::decode(bytes)
                    .map(|vv| (*device, vv))
                    .with_context(|| format!("decode device {device} in {}", path.display()))
            })
            .collect::<Result<_>>()?;
        let this = Self {
            path: Some(path),
            device: ledger.device,
            known: Mutex::new(known),
        };
        this.save(&this.known.lock())?;
        Ok(this)
    }

    /// A ledger that is never written to disk, for a handshake whose result
    /// is merged into a stored ledger later ([`Self::merge_from`]).
    pub fn in_memory(device: u64) -> Self {
        Self {
            path: None,
            device,
            known: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn device(&self) -> u64 {
        self.device
    }

    pub fn known(&self) -> BTreeMap<u64, VersionVector> {
        self.known.lock().clone()
    }

    pub fn hello(&self, own: &VersionVector) -> Hello {
        Hello {
            device: Some(self.device),
            vv: own.encode(),
            known: self
                .known
                .lock()
                .iter()
                .map(|(device, vv)| (*device, vv.encode()))
                .collect(),
        }
    }

    /// Merge what `hello` says into the map. Call it only after the round
    /// imported the sender's delta: an entry is history this device holds.
    pub fn record(&self, hello: &Hello) -> Result<()> {
        let Some(sender) = hello.device else {
            return Ok(());
        };
        let mut entries = vec![(sender, hello.version()?)];
        for (device, bytes) in &hello.known {
            let vv = VersionVector::decode(bytes)
                .with_context(|| format!("decode device {device} in the hello of {sender}"))?;
            entries.push((*device, vv));
        }
        self.merge(entries)
    }

    pub fn merge_from(&self, other: &PeerVersions) -> Result<()> {
        self.merge(other.known())
    }

    /// The version below which every tracked device holds the history (the
    /// meet of the map), or `None` when no device is tracked: then nothing
    /// may be trimmed, since a device that pairs later is not in the map.
    pub fn floor(&self) -> Option<VersionVector> {
        let known = self.known.lock();
        let mut entries = known.values();
        let first = entries.next()?.clone();
        Some(entries.fold(first, |floor, vv| floor.intersection(vv)))
    }

    fn merge(&self, entries: impl IntoIterator<Item = (u64, VersionVector)>) -> Result<()> {
        let mut known = self.known.lock();
        for (device, vv) in entries {
            if device != self.device {
                known.entry(device).or_default().merge(&vv);
            }
        }
        self.save(&known)
    }

    fn save(&self, known: &BTreeMap<u64, VersionVector>) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let ledger = Ledger {
            device: self.device,
            known: known.iter().map(|(d, vv)| (*d, vv.encode())).collect(),
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("create the ledger dir {}", dir.display()))?;
        }
        holon_filesystem::fs_port::write_durable_blocking(path, &serde_json::to_vec(&ledger)?)
            .with_context(|| format!("write the peer version ledger {}", path.display()))
    }
}
