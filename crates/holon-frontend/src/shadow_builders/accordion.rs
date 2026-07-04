use super::prelude::*;

/// Default cap fraction when `max_height_fraction` is omitted (§3).
pub const DEFAULT_MAX_HEIGHT_FRACTION: f64 = 0.4;

/// Flag key the `column` builder stamps onto its direct children's context so
/// an `accordion` can prove — at build time — that it is a direct flow-panel
/// column child. Absent ⇒ a `pinned` accordion is misplaced (§3 placement
/// guard).
pub(crate) const ACCORDION_PARENT_FLAG: &str = "accordion_parent";

/// Flag key the `section_stack` builder stamps onto its direct children's
/// context so an in-flow (`pinned:false`) or `sticky` accordion can prove — at
/// build time — that it lives inside a section stack. Absent ⇒ those variants
/// are misplaced (Inc C placement guard).
pub(crate) const SECTION_STACK_FLAG: &str = "section_stack_parent";

/// Where an accordion is placed — parsed once from the `pinned` / `sticky`
/// props (parse-don't-validate). Each variant has EXACTLY one legal context,
/// checked fail-loud at build time:
///
/// | Placement | props                    | legal context           |
/// |-----------|--------------------------|-------------------------|
/// | `Pinned`  | default (`pinned:true`)  | direct flow-column child|
/// | `InFlow`  | `pinned:false`           | inside a section stack  |
/// | `Sticky`  | `sticky:true`            | inside a section stack  |
///
/// `sticky:true` wins over `pinned` (a sticky overlay is never a pinned
/// footer). Anything placed outside its legal context renders the standard
/// error widget — never a silently-misrendered region.
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

        // Fail-loud placement guard (§3 + Inc C): each placement has exactly
        // one legal context, proven by a flag its container stamps. A misplaced
        // accordion is a visible error, never a silently-unbounded/mispositioned
        // region — silent degradation here recreates the exact bug class this
        // feature kills.
        let is_column_child = ba
            .ctx
            .flags
            .get(ACCORDION_PARENT_FLAG)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let is_section_stack_child = ba
            .ctx
            .flags
            .get(SECTION_STACK_FLAG)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let placement_ok = match placement {
            AccordionPlacement::Pinned => is_column_child,
            AccordionPlacement::InFlow | AccordionPlacement::Sticky => is_section_stack_child,
        };
        if !placement_ok {
            let need = match placement {
                AccordionPlacement::Pinned => "a direct child of a main-panel column",
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
        let collapsed = ba.args.get_bool("collapsed").unwrap_or(false);

        // Interpret children with BOTH parent flags CLEARED so a nested
        // accordion (an accordion is neither a column nor a section stack)
        // errors.
        let child_ctx = ba.ctx.without_flags();
        let children: Vec<ViewModel> = ba
            .args
            .positional_exprs
            .iter()
            .map(|expr| (ba.interpret)(expr, &child_ctx))
            .collect();

        let mut __props = std::collections::HashMap::new();
        __props.insert("title".to_string(), Value::String(title));
        if let Some(icon) = icon {
            __props.insert("icon".to_string(), Value::String(icon));
        }
        __props.insert("max_height_fraction".to_string(), Value::Float(fraction));
        __props.insert("collapsible".to_string(), Value::Boolean(collapsible));
        __props.insert("collapsed".to_string(), Value::Boolean(collapsed));
        // The renderer routes on this: pinned → split footer, in_flow → inline,
        // sticky → occlude overlay.
        __props.insert(
            "placement".to_string(),
            Value::String(placement.as_str().to_string()),
        );

        ViewModel {
            children: children.into_iter().map(Arc::new).collect(),
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
