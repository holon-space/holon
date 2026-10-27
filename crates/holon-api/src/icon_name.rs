//! The glyph names a layout may ask the `icon` widget for.
//!
//! The renderer's tables are the ones that actually draw something
//! (`frontends/gpui/src/render/builders/icon.rs`), and an unknown name there
//! draws a bullet — a sensible default for a name that a HUMAN typed into a
//! layout and can see go wrong. A name arriving from a config file has no such
//! reader: nobody watches an integration sidecar render. So the name is parsed
//! here, at the config boundary, and a typo is a refusal rather than a bullet.
//!
//! A gpui-side test asserts the two tables agree, so a renderer that learns a
//! new glyph without listing it here fails at build time rather than silently
//! putting a valid name out of a sidecar's reach.

use std::fmt;

/// Every name the renderer draws, SVG-backed and glyph-backed alike.
pub const ICON_NAMES: &[&str] = &[
    "abacus",
    "add",
    "ai",
    "alert",
    "arrow_right",
    "bar_chart",
    "bell",
    "bookmark",
    "books",
    "calendar",
    "chart",
    "check",
    "checkbox",
    "chevron_down",
    "chevron_left",
    "chevron_right",
    "chevron_up",
    "circle",
    "clipboard",
    "clock",
    "close",
    "code",
    "comment",
    "cycle",
    "delete",
    "directory",
    "directory_open",
    "document",
    "document_text",
    "drag",
    "edit",
    "error",
    "eye",
    "eye_off",
    "file",
    "file_text",
    "fire",
    "folder",
    "folder_open",
    "gear",
    "grid",
    "grip",
    "hamburger",
    "hidden",
    "home",
    "hot",
    "idea",
    "inbox",
    "info",
    "label",
    "light_bulb",
    "link",
    "list",
    "list_tree",
    "lock",
    "magic",
    "memo",
    "menu",
    "minus",
    "note",
    "notebook",
    "notification",
    "orgmode",
    "outbox",
    "outliner",
    "pencil",
    "pin",
    "plus",
    "pushpin",
    "refresh",
    "remove",
    "right",
    "robot",
    "scroll",
    "search",
    "settings",
    "source",
    "sparkles",
    "speech",
    "star",
    "sync",
    "table",
    "table_2",
    "tag",
    "thought",
    "time",
    "trash",
    "tree",
    "unlock",
    "visible",
    "warning",
    "x",
];

/// A name [`ICON_NAMES`] holds — proof that the renderer draws it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IconName(&'static str);

impl IconName {
    pub fn parse(raw: &str) -> Result<Self, UnknownIconName> {
        ICON_NAMES
            .iter()
            .find(|n| **n == raw)
            .map(|n| Self(n))
            .ok_or_else(|| UnknownIconName {
                raw: raw.to_string(),
            })
    }

    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for IconName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownIconName {
    pub raw: String,
}

impl fmt::Display for UnknownIconName {
    /// Names the near misses rather than all ninety: an author who typed
    /// `plugin` wants to be told `plus`/`pin` exist, and a full dump would bury
    /// that under the rest of the table.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lowered = self.raw.to_lowercase();
        let stem: String = lowered.chars().take(3).collect();
        let near: Vec<&str> = ICON_NAMES
            .iter()
            .copied()
            .filter(|n| !stem.is_empty() && (n.starts_with(&stem) || lowered.contains(*n)))
            .take(6)
            .collect();
        write!(f, "unknown icon name {:?}", self.raw)?;
        if near.is_empty() {
            write!(
                f,
                "; the renderer draws {} names, listed in \
                 holon_api::icon_name::ICON_NAMES",
                ICON_NAMES.len()
            )
        } else {
            write!(
                f,
                "; did you mean one of {near:?}? The full list is \
                 holon_api::icon_name::ICON_NAMES"
            )
        }
    }
}

impl std::error::Error for UnknownIconName {}

impl serde::Serialize for IconName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for IconName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;
