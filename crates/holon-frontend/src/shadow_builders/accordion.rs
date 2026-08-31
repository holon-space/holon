use super::prelude::*;
use crate::render_context::ContainerCapability;
use crate::render_context::LayoutHint;

/// Default cap fraction when `max_height_fraction` is omitted (§3).
pub const DEFAULT_MAX_HEIGHT_FRACTION: f64 = 0.4;

/// Available width (logical px) below which an accordion starts collapsed:
/// on a phone even a capped body costs the panel most of its room, so the
/// default there is a header row the reader opens on demand.
///
/// Width is the app's one mobile axis — the same 600 px `if_space` and the
/// drawer's Overlay mode key on, so a rotated phone reads as mobile for all
/// three at once. Height would not separate the two: a portrait phone is
/// ~850 logical px tall, taller than many desktop windows.
pub const ACCORDION_MIN_EXPANDED_WIDTH_PX: f32 = 600.0;

/// Where an accordion is placed — parsed once from the `pinned` / `sticky`
/// props (parse-don't-validate). Each variant needs EXACTLY one capability from
/// its container, checked fail-loud at build time:
///
/// | Placement | props                    | container must offer          |
/// |-----------|--------------------------|-------------------------------|
/// | `Pinned`  | default (`pinned:true`)  | `PinToEnd` (a flow column)    |
/// | `InFlow`  | `pinned:false`           | `ScrollSections`              |
/// | `Sticky`  | `sticky:true`            | `ScrollSections`              |
///
/// `sticky:true` wins over `pinned` (a sticky overlay is never a pinned
/// footer). An accordion whose container cannot honour its placement renders
/// the standard error widget — never a silently-misrendered region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccordionPlacement {
    Pinned,
    InFlow,
    Sticky,
}

impl AccordionPlacement {
    /// Parse from the two booleans (both already defaulted): `sticky` first
    /// (it wins), then `pinned`.
    pub fn parse(pinned: bool, sticky: bool) -> Self {
        if sticky {
            AccordionPlacement::Sticky
        } else if !pinned {
            AccordionPlacement::InFlow
        } else {
            AccordionPlacement::Pinned
        }
    }

    /// The prop-string the renderer routes on.
    pub fn as_str(self) -> &'static str {
        match self {
            AccordionPlacement::Pinned => "pinned",
            AccordionPlacement::InFlow => "in_flow",
            AccordionPlacement::Sticky => "sticky",
        }
    }
}

/// Parse-don't-validate boundary for `max_height_fraction` (§3): must be finite
/// and in `(0.0, 1.0]`. A bad value becomes a visible error widget, never a
/// silently-clamped region.
pub(crate) fn validate_fraction(f: f64) -> Result<f64, String> {
    if f.is_finite() && f > 0.0 && f <= 1.0 {
        Ok(f)
    } else {
        Err(format!(
            "max_height_fraction must be finite and in (0.0, 1.0], got {f}"
        ))
    }
}

holon_macros::widget_builder! {
    raw fn accordion(ba: BA<'_>) -> ViewModel {
        let pinned = ba.args.get_bool("pinned").unwrap_or(true);
        let sticky = ba.args.get_bool("sticky").unwrap_or(false);
        let placement = AccordionPlacement::parse(pinned, sticky);

        // Fail-loud placement guard (§3 + Inc C): the container declares what it
        // can honour, and a placement whose capability is not on offer is a
        // visible error — never a silently-unbounded/mispositioned region,
        // which is the exact bug class this feature kills.
        let required = match placement {
            AccordionPlacement::Pinned => ContainerCapability::PinToEnd,
            AccordionPlacement::InFlow | AccordionPlacement::Sticky => {
                ContainerCapability::ScrollSections
            }
        };
        if ba.parent_capability != required {
            let need = match placement {
                AccordionPlacement::Pinned => {
                    "a direct child of a container that pins trailing-edge children (a column)"
                }
                AccordionPlacement::InFlow => "inside a section stack (pinned:false)",
                AccordionPlacement::Sticky => "inside a section stack (sticky:true)",
            };
            return ViewModel::error(
                "accordion",
                format!("accordion with placement '{}' must be {need}", placement.as_str()),
            );
        }

        let fraction = match ba.args.get_f64("max_height_fraction") {
            Some(f) => match validate_fraction(f) {
                Ok(f) => f,
                Err(msg) => return ViewModel::error("accordion", msg),
            },
            None => DEFAULT_MAX_HEIGHT_FRACTION,
        };

        let title = ba
            .args
            .get_string("title")
            .map(|s| s.to_string())
            .unwrap_or_default();
        let icon = ba.args.get_string("icon").map(|s| s.to_string());
        let collapsible = ba.args.get_bool("collapsible").unwrap_or(true);
        // Unmeasured available space is desktop-first (the `if_space` default):
        // start expanded.
        let narrow = ba
            .ctx
            .available_space
            .is_some_and(|s| s.width_px < ACCORDION_MIN_EXPANDED_WIDTH_PX);
        let collapsed = ba.args.get_bool("collapsed").unwrap_or(narrow);
        let hide_when_empty = ba.args.get_bool("hide_when_empty").unwrap_or(false);

        // An accordion offers nothing: `ba.ctx` already carries
        // `ContainerCapability::None` (the interpreter strips the container's
        // offer one level down), so a nested accordion errors.
        let children: Vec<ViewModel> = ba
            .args
            .positional_exprs
            .iter()
            .map(|expr| (ba.interpret)(expr, ba.ctx))
            .collect();

        let mut __props = std::collections::HashMap::new();
        __props.insert("title".to_string(), Value::String(title));
        if let Some(icon) = icon {
            __props.insert("icon".to_string(), Value::String(icon));
        }
        __props.insert("max_height_fraction".to_string(), Value::Float(fraction));
        __props.insert("collapsible".to_string(), Value::Boolean(collapsible));
        __props.insert("collapsed".to_string(), Value::Boolean(collapsed));
        // Opt-in: with no content rows the renderer paints nothing at all,
        // not even the title row.
        __props.insert(
            "hide_when_empty".to_string(),
            Value::Boolean(hide_when_empty),
        );
        // The renderer routes on this: pinned → split footer, in_flow → inline,
        // sticky → occlude overlay.
        __props.insert(
            "placement".to_string(),
            Value::String(placement.as_str().to_string()),
        );

        // The pinned placement is a LAYOUT declaration, not an identity: the
        // container reads this hint to decide what to pin at its trailing edge,
        // so no renderer has to know the widget is called "accordion".
        let layout_hint = match placement {
            AccordionPlacement::Pinned => LayoutHint::PinnedToEnd,
            AccordionPlacement::InFlow | AccordionPlacement::Sticky => LayoutHint::default(),
        };

        ViewModel {
            children: children.into_iter().map(Arc::new).collect(),
            layout_hint,
            // Live collapse state is the node's `expanded` Mutable, seeded from
            // `collapsed` (§3) — NOT the `ctx.local` ephemeral-key cache that
            // collapsible.rs uses (keyed by title, so it collides across
            // same-titled instances and resets on title change).
            expanded: Some(futures_signals::signal::Mutable::new(!collapsed)),
            ..ViewModel::from_widget("accordion", __props)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AccordionPlacement;
    use super::validate_fraction;

    #[test]
    fn valid_fractions_pass() {
        assert_eq!(validate_fraction(0.4), Ok(0.4));
        assert_eq!(validate_fraction(0.1), Ok(0.1));
        assert_eq!(validate_fraction(1.0), Ok(1.0));
    }

    #[test]
    fn out_of_range_fractions_error() {
        // Zero, negative, and > 1.0 are all rejected loudly.
        assert!(validate_fraction(0.0).is_err());
        assert!(validate_fraction(-1.0).is_err());
        assert!(validate_fraction(2.0).is_err());
    }

    #[test]
    fn non_finite_fractions_error() {
        assert!(validate_fraction(f64::NAN).is_err());
        assert!(validate_fraction(f64::INFINITY).is_err());
        assert!(validate_fraction(f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn placement_parse_precedence() {
        // sticky wins over pinned; pinned:false ⇒ in-flow; default ⇒ pinned.
        assert_eq!(
            AccordionPlacement::parse(true, false),
            AccordionPlacement::Pinned
        );
        assert_eq!(
            AccordionPlacement::parse(false, false),
            AccordionPlacement::InFlow
        );
        assert_eq!(
            AccordionPlacement::parse(true, true),
            AccordionPlacement::Sticky
        );
        assert_eq!(
            AccordionPlacement::parse(false, true),
            AccordionPlacement::Sticky
        );
    }
}
