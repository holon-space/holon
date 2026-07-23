use super::prelude::*;

/// Default cap fraction when `max_height_fraction` is omitted (§3).
pub const DEFAULT_MAX_HEIGHT_FRACTION: f64 = 0.4;

/// Flag key the `column` builder stamps onto its direct children's context so
/// an `accordion` can prove — at build time — that it is a direct flow-panel
/// column child. Absent ⇒ the accordion is misplaced (§3 placement guard).
pub(crate) const ACCORDION_PARENT_FLAG: &str = "accordion_parent";

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
        // Fail-loud placement guard (§3, senior-review amendment): the
        // bounded-footer split only works when the accordion is a DIRECT child
        // of a flow-panel `column`. Anywhere else (root, inside a `row`, inside
        // another accordion, the drawer branch) is a visible error, never a
        // silently-unbounded region — silent degradation here would recreate
        // the exact bug class this feature kills.
        let is_column_child = ba
            .ctx
            .flags
            .get(ACCORDION_PARENT_FLAG)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !is_column_child {
            return ViewModel::error(
                "accordion",
                "accordion must be a direct child of a main-panel column",
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

        // Interpret children with the parent flag CLEARED so a nested
        // `accordion(accordion(...))` errors (an accordion is not a column).
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
}
