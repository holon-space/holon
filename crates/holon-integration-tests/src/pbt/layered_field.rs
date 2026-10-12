//! Reference model of a field that lives in more than one layer (FOLD-scope.a,
//! holon-work#170): `collapsed`, declared `[base, device]`.
//!
//! Base is the entity's home document, replicated to every device; the device
//! layer belongs to one device and never syncs. A device-layer value records
//! WHY it is there: `Fallback` (base had no slot when it was written) or
//! `Override` (the user meant "only on this device"). [`Rule::Hybrid`] is the
//! ruled read/write rule; [`Rule::Plain`] (later layer wins, no intent) is kept
//! as the rule the invariants were shown to reject.
//!
//! @pbt kind ref
//! @pbt covers layered-field — base/device slots, write intent, the hybrid
//!   write and read rules, base sync between two devices, and `:COLLAPSED:`
//!   in an own page's org file

use std::collections::BTreeMap;
use std::path::Path;

use holon_api::EntityUri;
use holon_org_format::org_renderer::OrgRenderer;
use holon_org_format::parser::parse_org_file;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    Plain,
    Hybrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Device {
    A,
    B,
}

impl Device {
    pub const ALL: [Device; 2] = [Device::A, Device::B];
}

/// The fold targets the keystone fixtures seed, one per kind of home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// A heading with children in the user's own org file.
    OwnPage,
    /// An integration row with children; its home stores `collapsed` only
    /// after [`LayeredField::home_gains_field`].
    IntegrationRow,
    /// A step of a `.cook` recipe: the read-only tier admits no write.
    CookLeaf,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::OwnPage, Target::IntegrationRow, Target::CookLeaf];

    pub fn uri(self) -> EntityUri {
        EntityUri::block(match self {
            Target::OwnPage => "layered-own-page",
            Target::IntegrationRow => "layered-integration-row",
            Target::CookLeaf => "layered-cook-step",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Fallback,
    Override,
}

/// A base value with the Lamport stamp sync merges by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamped {
    value: bool,
    stamp: (u64, Device),
}

#[derive(Debug, Clone, Default)]
struct DeviceState {
    base: BTreeMap<Target, Stamped>,
    layer: BTreeMap<Target, (bool, Intent)>,
    /// Lamport clock: devices write base offline, so two writes can carry
    /// the same counter and the device id breaks the tie.
    clock: u64,
    /// This device's copy of the own page's org file.
    own_page_file: String,
}

const OWN_PAGE_PATH: &str = "/vault/own.org";
const VAULT_ROOT: &str = "/vault";

/// The page a write-back must not damage. Every `Block` field `PartialEq`
/// compares is authored away from its default here, so the whole-file
/// invariant can see a write-back that drops any one of them.
pub fn seed_own_page_file() -> String {
    format!(
        "* Own page with *bold* text :home:pinned:
:PROPERTIES:
:ID: {page}
:AREA: kitchen
:REQUIRES: layered-own-child
:ADVICE_SUPPRESSED: layered-own-child
:contributes-to: layered-own-child
:WIDGET_ONLY: t
:END:
** Child
:PROPERTIES:
:ID: layered-own-child
:END:
#+name: own-page-query
#+begin_src prql :id layered-own-src
from blocks
#+end_src
",
        page = Target::OwnPage.uri().id()
    )
}

#[derive(Debug, Clone)]
pub struct LayeredField {
    rule: Rule,
    integration_stores_field: bool,
    devices: BTreeMap<Device, DeviceState>,
}

impl LayeredField {
    pub fn new(rule: Rule) -> Self {
        let devices = Device::ALL
            .into_iter()
            .map(|d| {
                (
                    d,
                    DeviceState {
                        own_page_file: seed_own_page_file(),
                        ..DeviceState::default()
                    },
                )
            })
            .collect();
        Self {
            rule,
            integration_stores_field: false,
            devices,
        }
    }

    /// `Slot(entity, base)`: the home stores the field and its tier admits the
    /// write. The device layer always offers a slot for `collapsed`.
    pub fn base_offers_slot(&self, target: Target) -> bool {
        match target {
            Target::OwnPage => true,
            Target::IntegrationRow => self.integration_stores_field,
            Target::CookLeaf => false,
        }
    }

    pub fn home_gains_field(&mut self) {
        assert!(
            !self.integration_stores_field,
            "home_gains_field: the integration home already stores `collapsed`"
        );
        self.integration_stores_field = true;
    }

    pub fn layer(&self, device: Device, target: Target) -> Option<(bool, Intent)> {
        self.devices[&device].layer.get(&target).copied()
    }

    pub fn base(&self, device: Device, target: Target) -> Option<bool> {
        self.devices[&device].base.get(&target).map(|s| s.value)
    }

    /// The Lamport stamp of `device`'s base replica for `target`.
    pub fn base_stamp(&self, device: Device, target: Target) -> Option<(u64, Device)> {
        self.devices[&device].base.get(&target).map(|s| s.stamp)
    }

    /// `device`'s copy of the own page's org file, as the write-back left it.
    pub fn own_page_file(&self, device: Device) -> &str {
        &self.devices[&device].own_page_file
    }

    /// The value `device` renders; untouched entities are expanded.
    pub fn effective(&self, device: Device, target: Target) -> bool {
        let state = &self.devices[&device];
        let base = state.base.get(&target).map(|s| s.value);
        let layer = state.layer.get(&target).copied();
        match self.rule {
            Rule::Plain => layer.map(|(v, _)| v).or(base).unwrap_or(false),
            Rule::Hybrid => match (layer, base) {
                (Some((v, Intent::Override)), _) => v,
                (_, Some(b)) => b,
                (Some((v, Intent::Fallback)), None) => v,
                (None, None) => false,
            },
        }
    }

    /// The user clicks the fold chevron on `device`; returns the value written.
    pub fn fold(&mut self, device: Device, target: Target) -> bool {
        let value = !self.effective(device, target);
        self.write(device, target, value);
        value
    }

    /// The future "only on this device" gesture.
    pub fn write_override(&mut self, device: Device, target: Target, value: bool) {
        self.layer_mut(device)
            .insert(target, (value, Intent::Override));
    }

    fn write(&mut self, device: Device, target: Target, value: bool) {
        let has_override = matches!(
            self.devices[&device].layer.get(&target),
            Some((_, Intent::Override))
        );
        if self.rule == Rule::Hybrid && has_override {
            self.layer_mut(device)
                .insert(target, (value, Intent::Override));
        } else if self.base_offers_slot(target) {
            self.write_base(device, target, value);
            if self.rule == Rule::Hybrid {
                self.layer_mut(device).remove(&target);
            }
        } else {
            self.layer_mut(device)
                .insert(target, (value, Intent::Fallback));
        }
    }

    fn write_base(&mut self, device: Device, target: Target, value: bool) {
        let state = self.devices.get_mut(&device).unwrap();
        state.clock += 1;
        let stamped = Stamped {
            value,
            stamp: (state.clock, device),
        };
        state.base.insert(target, stamped);
        if target == Target::OwnPage {
            self.rewrite_own_page_file(device);
        }
    }

    /// Both devices exchange their base replicas; the later stamp wins.
    pub fn sync(&mut self) {
        let mut merged: BTreeMap<Target, Stamped> = BTreeMap::new();
        for state in self.devices.values() {
            for (target, s) in &state.base {
                merged
                    .entry(*target)
                    .and_modify(|m| {
                        if s.stamp > m.stamp {
                            *m = *s;
                        }
                    })
                    .or_insert(*s);
            }
        }
        let clock = self.devices.values().map(|s| s.clock).max().unwrap();
        for device in Device::ALL {
            let state = self.devices.get_mut(&device).unwrap();
            state.clock = clock;
            let own_page_changed = state.base.get(&Target::OwnPage) != merged.get(&Target::OwnPage);
            state.base = merged.clone();
            if own_page_changed {
                self.rewrite_own_page_file(device);
            }
        }
    }

    /// The org write-back of today: parse the file, set the field, render.
    fn rewrite_own_page_file(&mut self, device: Device) {
        let value = self.devices[&device].base[&Target::OwnPage].value;
        let state = self.devices.get_mut(&device).unwrap();
        let mut blocks = parse_own_page(&state.own_page_file);
        blocks
            .iter_mut()
            .find(|b| b.id == Target::OwnPage.uri())
            .expect("own page file lost its page heading")
            .collapsed = value;
        let path = Path::new(OWN_PAGE_PATH);
        let file_id = holon_org_format::parser::generate_file_id(path, Path::new(VAULT_ROOT));
        state.own_page_file = OrgRenderer::render_entitys(&blocks, path, &file_id)
            .expect("own page org render")
            .text;
    }

    fn layer_mut(&mut self, device: Device) -> &mut BTreeMap<Target, (bool, Intent)> {
        &mut self.devices.get_mut(&device).unwrap().layer
    }

    /// Invariant 2, no hidden upstream: without an override, a device shows
    /// what its base holds.
    pub fn check_no_hidden_upstream(&self) {
        for device in Device::ALL {
            let state = &self.devices[&device];
            for target in Target::ALL {
                let overridden = matches!(state.layer.get(&target), Some((_, Intent::Override)));
                if let (false, Some(base)) = (overridden, state.base.get(&target)) {
                    assert_eq!(
                        self.effective(device, target),
                        base.value,
                        "hidden upstream: {target:?} on {device:?} shows a device-layer value \
                         over base {} without an override (layer: {:?})",
                        base.value,
                        state.layer.get(&target)
                    );
                }
            }
        }
    }
}

pub fn parse_own_page(text: &str) -> Vec<holon_api::block::Block> {
    parse_org_file(
        Path::new(OWN_PAGE_PATH),
        text,
        &EntityUri::no_parent(),
        Path::new(VAULT_ROOT),
    )
    .expect("own page org parse")
    .blocks
}
