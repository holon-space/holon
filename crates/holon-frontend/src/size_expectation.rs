//! Algebraic expression DSL for declaring expected size bounds on widgets.
//!
//! A widget records its layout intent — "I should be at least 16px tall",
//! "my width follows my child", "my height is the sum of my children plus
//! a 4px spacer between each plus an 8px margin on each side" — as an
//! [`Expr`] AST. The PBT layout invariant evaluates the expression against
//! the observed bounds at check time, via an [`EvalCtx`] supplied by the
//! frontend (it knows how to look up self/child/parent observations in
//! whatever registry the frontend uses).
//!
//! Design notes:
//! - The AST is `Clone + Debug + PartialEq` (no closures), so failure messages
//!   can show the full expression and the dump can serialize it.
//! - Arithmetic uses Rust's `Add`/`Sub`/`Mul`/`Div` operator overloads so
//!   widget code reads like an equation: `Expr::children(Y) + (Expr::ChildCount
//!   - 1.0) * 4.0 + 16.0`.
//! - `Free` propagates through ops: any `Free` in an arithmetic chain makes the
//!   whole expression `Free` (= "no constraint"). That matches the semantics "I
//!   have nothing to claim about this dimension".
//! - The interpreter does not recurse through nested wrapper widgets
//!   automatically. If a wrapper says `min_h = Expr::Child("foo", Y)` and that
//!   child's expectation is itself relational, the registry-side `EvalCtx` is
//!   responsible for resolving — typically by storing observed bounds, since
//!   the observed value is what the wrapper actually wraps.

use std::ops::Add;
use std::ops::Div;
use std::ops::Mul;
use std::ops::Sub;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Aggregator {
    Sum,
    Max,
    Min,
}

impl Aggregator {
    pub fn apply(self, vs: &[f32]) -> Option<f32> {
        match self {
            Aggregator::Sum => Some(vs.iter().sum()),
            Aggregator::Max => vs.iter().copied().reduce(f32::max),
            Aggregator::Min => vs.iter().copied().reduce(f32::min),
        }
    }
}

/// A symbolic expression evaluating to an `Option<f32>` at check time.
///
/// `None` means "unconstrained on this axis". `None` propagates through
/// arithmetic (any `Free` makes the whole expression `Free`); use
/// [`Expr::Coalesce`] to fall back to a concrete number.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Pixel constant.
    Const(f32),
    /// Unconstrained — propagates `None` through arithmetic.
    Free,
    /// This element's own observed dimension on `axis`.
    Self_(Axis),
    /// A specific tracked child's observed dimension on `axis`.
    Child(String, Axis),
    /// All tracked children, aggregated.
    Children {
        axis: Axis,
        agg: Aggregator,
    },
    /// Number of tracked children of this element.
    ChildCount,
    /// This element's tracked parent's observed dimension on `axis`.
    Parent(Axis),
    /// Viewport (window) dimension on `axis`.
    Viewport(Axis),

    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),

    /// n-ary minimum. Useful for `min_h = min(content_height, viewport - 100)`.
    MinN(Vec<Expr>),
    /// n-ary maximum.
    MaxN(Vec<Expr>),

    /// First non-`Free` value in the list. Lets a widget say
    /// "use this if you can resolve it, otherwise this".
    Coalesce(Vec<Expr>),
}

impl Expr {
    pub fn child(id: impl Into<String>, axis: Axis) -> Self {
        Expr::Child(id.into(), axis)
    }
    pub fn children(axis: Axis) -> Self {
        Expr::Children {
            axis,
            agg: Aggregator::Sum,
        }
    }
    pub fn max_children(axis: Axis) -> Self {
        Expr::Children {
            axis,
            agg: Aggregator::Max,
        }
    }
    pub fn min_of(parts: Vec<Expr>) -> Self {
        Expr::MinN(parts)
    }
    pub fn max_of(parts: Vec<Expr>) -> Self {
        Expr::MaxN(parts)
    }
    pub fn coalesce(parts: Vec<Expr>) -> Self {
        Expr::Coalesce(parts)
    }
}

impl From<f32> for Expr {
    fn from(v: f32) -> Self {
        Expr::Const(v)
    }
}

// ─── Operator overloads ───
//
// Implemented for `Expr op Expr`, `Expr op f32`, `f32 op Expr`. Each just
// builds the corresponding AST node; no eager evaluation.

macro_rules! impl_binop {
    ($trait:ident, $method:ident, $variant:ident) => {
        impl $trait<Expr> for Expr {
            type Output = Expr;
            fn $method(self, rhs: Expr) -> Expr {
                Expr::$variant(Box::new(self), Box::new(rhs))
            }
        }
        impl $trait<f32> for Expr {
            type Output = Expr;
            fn $method(self, rhs: f32) -> Expr {
                Expr::$variant(Box::new(self), Box::new(Expr::Const(rhs)))
            }
        }
        impl $trait<Expr> for f32 {
            type Output = Expr;
            fn $method(self, rhs: Expr) -> Expr {
                Expr::$variant(Box::new(Expr::Const(self)), Box::new(rhs))
            }
        }
    };
}

impl_binop!(Add, add, Add);
impl_binop!(Sub, sub, Sub);
impl_binop!(Mul, mul, Mul);
impl_binop!(Div, div, Div);

impl std::iter::Sum<Expr> for Expr {
    fn sum<I: Iterator<Item = Expr>>(iter: I) -> Self {
        let v: Vec<Expr> = iter.collect();
        if v.is_empty() {
            return Expr::Const(0.0);
        }
        let mut it = v.into_iter();
        let first = it.next().unwrap();
        it.fold(first, |acc, e| acc + e)
    }
}

// ─── SizeBounds ───

/// Per-element layout expectation: lower and upper bounds on width and
/// height. All four default to [`Expr::Free`] (= no constraint).
#[derive(Debug, Clone, PartialEq)]
pub struct SizeBounds {
    pub min_w: Expr,
    pub min_h: Expr,
    pub max_w: Expr,
    pub max_h: Expr,
}

impl Default for SizeBounds {
    fn default() -> Self {
        Self {
            min_w: Expr::Free,
            min_h: Expr::Free,
            max_w: Expr::Free,
            max_h: Expr::Free,
        }
    }
}

impl SizeBounds {
    /// "Must be at least this big" — leaves like `text`, `selectable`,
    /// `vms_button` use this to assert tap-target / line-height invariants.
    pub fn at_least(min_w: f32, min_h: f32) -> Self {
        Self {
            min_w: Expr::Const(min_w),
            min_h: Expr::Const(min_h),
            max_w: Expr::Free,
            max_h: Expr::Free,
        }
    }

    /// "Must be exactly this big" — rare; for fixed-pixel widgets like icons.
    pub fn fixed(w: f32, h: f32) -> Self {
        Self {
            min_w: Expr::Const(w),
            min_h: Expr::Const(h),
            max_w: Expr::Const(w),
            max_h: Expr::Const(h),
        }
    }

    /// Wrapper that takes its bounds from a single tracked child by id.
    /// Useful when a wrapper's purpose is to be transparent to layout
    /// (e.g. `view_mode_switcher` wrapping its body slot).
    pub fn follows_child(child_el_id: impl Into<String>) -> Self {
        let id = child_el_id.into();
        Self {
            min_w: Expr::Child(id.clone(), Axis::X),
            min_h: Expr::Child(id.clone(), Axis::Y),
            max_w: Expr::Child(id.clone(), Axis::X),
            max_h: Expr::Child(id, Axis::Y),
        }
    }

    /// "A user can aim at this" — an interaction wrapper must be laid out with
    /// real extent on both axes. A degenerate rect is unclickable AND its
    /// centre resolves onto whatever is painted at the clip edge, so a driver
    /// aiming here silently drives a neighbour instead.
    ///
    /// Deliberately 1px rather than a platform tap-target size: the defect
    /// class is total collapse, and a larger floor would encode theme
    /// decisions the geometry layer has no business asserting.
    pub fn non_degenerate() -> Self {
        Self::at_least(1.0, 1.0)
    }

    /// "I'm allowed to collapse to nothing" — the live_block / spacer
    /// case. Same as [`Default`], named for clarity at call sites.
    pub fn collapsible() -> Self {
        Self::default()
    }

    /// Evaluate the bounds against observed `(w, h)`. Returns the first
    /// violation found, or `Ok(())` if observed satisfies all four
    /// declared constraints. `Free` constraints are skipped. Bounds where
    /// the expression evaluates to `None` (couldn't resolve — e.g. a
    /// `Child` that doesn't exist yet) are also skipped, on the principle
    /// that an unresolvable constraint is not a failure to *check*.
    pub fn check(
        &self,
        observed_w: f32,
        observed_h: f32,
        ctx: &dyn EvalCtx,
    ) -> Result<(), BoundsViolation> {
        if let Some(min_w) = eval(&self.min_w, ctx) {
            if observed_w + EPS < min_w {
                return Err(BoundsViolation {
                    axis: Axis::X,
                    kind: ViolationKind::BelowMin,
                    bound: min_w,
                    observed: observed_w,
                    expr: self.min_w.clone(),
                });
            }
        }
        if let Some(min_h) = eval(&self.min_h, ctx) {
            if observed_h + EPS < min_h {
                return Err(BoundsViolation {
                    axis: Axis::Y,
                    kind: ViolationKind::BelowMin,
                    bound: min_h,
                    observed: observed_h,
                    expr: self.min_h.clone(),
                });
            }
        }
        if let Some(max_w) = eval(&self.max_w, ctx) {
            if observed_w > max_w + EPS {
                return Err(BoundsViolation {
                    axis: Axis::X,
                    kind: ViolationKind::AboveMax,
                    bound: max_w,
                    observed: observed_w,
                    expr: self.max_w.clone(),
                });
            }
        }
        if let Some(max_h) = eval(&self.max_h, ctx) {
            if observed_h > max_h + EPS {
                return Err(BoundsViolation {
                    axis: Axis::Y,
                    kind: ViolationKind::AboveMax,
                    bound: max_h,
                    observed: observed_h,
                    expr: self.max_h.clone(),
                });
            }
        }
        Ok(())
    }
}

/// Tolerance for float compare in [`SizeBounds::check`]. Sub-pixel rounding
/// from Taffy/winit can produce 0.5px differences — don't fail on those.
const EPS: f32 = 0.5;

#[derive(Debug, Clone, PartialEq)]
pub struct BoundsViolation {
    pub axis: Axis,
    pub kind: ViolationKind,
    pub bound: f32,
    pub observed: f32,
    pub expr: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationKind {
    BelowMin,
    AboveMax,
}

impl std::fmt::Display for BoundsViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let axis = match self.axis {
            Axis::X => "width",
            Axis::Y => "height",
        };
        let cmp = match self.kind {
            ViolationKind::BelowMin => "<",
            ViolationKind::AboveMax => ">",
        };
        write!(
            f,
            "{axis} {observed} {cmp} {bound} (from {expr:?})",
            observed = self.observed,
            bound = self.bound,
            expr = self.expr,
        )
    }
}

// ─── Evaluation ───

/// Frontend-supplied context that resolves [`Expr`] terms against the
/// concrete render state. The PBT invariant builds one of these per
/// element being checked — the `Self_`/`Parent`/`Child` accessors are
/// scoped to "the element under inspection".
pub trait EvalCtx {
    /// The element under inspection's observed dimension on `axis`.
    fn self_observed(&self, axis: Axis) -> Option<f32>;
    /// A tracked child's observed dimension. The id is whatever the
    /// frontend used at `tracked()` time.
    fn child_observed(&self, child_el_id: &str, axis: Axis) -> Option<f32>;
    /// All tracked children's observed dimensions on `axis`. Order is not
    /// guaranteed; aggregators are commutative.
    fn children_observed(&self, axis: Axis) -> Vec<f32>;
    /// Number of tracked direct children.
    fn child_count(&self) -> usize;
    /// The element's tracked parent's observed dimension. `None` at the
    /// root or if the parent isn't tracked.
    fn parent_observed(&self, axis: Axis) -> Option<f32>;
    /// The frontend's viewport (window) dimension. `None` if unknown.
    fn viewport(&self, axis: Axis) -> Option<f32>;
}

/// Evaluate `expr` against `ctx`. Returns `None` for expressions that
/// reduce to `Free` or can't be resolved (e.g. a `Child` lookup miss);
/// `Some(v)` for a concrete pixel value.
pub fn eval(expr: &Expr, ctx: &dyn EvalCtx) -> Option<f32> {
    match expr {
        Expr::Const(v) => Some(*v),
        Expr::Free => None,
        Expr::Self_(axis) => ctx.self_observed(*axis),
        Expr::Child(id, axis) => ctx.child_observed(id, *axis),
        Expr::Children { axis, agg } => {
            let vs = ctx.children_observed(*axis);
            agg.apply(&vs)
        }
        Expr::ChildCount => Some(ctx.child_count() as f32),
        Expr::Parent(axis) => ctx.parent_observed(*axis),
        Expr::Viewport(axis) => ctx.viewport(*axis),
        Expr::Add(a, b) => Some(eval(a, ctx)? + eval(b, ctx)?),
        Expr::Sub(a, b) => Some(eval(a, ctx)? - eval(b, ctx)?),
        Expr::Mul(a, b) => Some(eval(a, ctx)? * eval(b, ctx)?),
        Expr::Div(a, b) => {
            let av = eval(a, ctx)?;
            let bv = eval(b, ctx)?;
            // Division by zero → unconstrained, not an exception.
            if bv == 0.0 { None } else { Some(av / bv) }
        }
        Expr::MinN(items) => items.iter().filter_map(|e| eval(e, ctx)).reduce(f32::min),
        Expr::MaxN(items) => items.iter().filter_map(|e| eval(e, ctx)).reduce(f32::max),
        Expr::Coalesce(items) => items.iter().find_map(|e| eval(e, ctx)),
    }
}

// ─── Tests ───

#[cfg(test)]
mod tests {
    use super::*;

    /// Test EvalCtx with hand-picked observations.
    #[derive(Default)]
    struct TestCtx {
        self_w: Option<f32>,
        self_h: Option<f32>,
        children_h: Vec<f32>,
        child_count: usize,
        viewport_y: Option<f32>,
    }
    impl EvalCtx for TestCtx {
        fn self_observed(&self, a: Axis) -> Option<f32> {
            match a {
                Axis::X => self.self_w,
                Axis::Y => self.self_h,
            }
        }
        fn child_observed(&self, _: &str, _: Axis) -> Option<f32> {
            None
        }
        fn children_observed(&self, a: Axis) -> Vec<f32> {
            match a {
                Axis::Y => self.children_h.clone(),
                _ => vec![],
            }
        }
        fn child_count(&self) -> usize {
            self.child_count
        }
        fn parent_observed(&self, _: Axis) -> Option<f32> {
            None
        }
        fn viewport(&self, a: Axis) -> Option<f32> {
            match a {
                Axis::Y => self.viewport_y,
                _ => None,
            }
        }
    }

    #[test]
    fn const_evaluates() {
        let ctx = TestCtx::default();
        assert_eq!(eval(&Expr::Const(42.0), &ctx), Some(42.0));
    }

    #[test]
    fn free_evaluates_to_none() {
        let ctx = TestCtx::default();
        assert_eq!(eval(&Expr::Free, &ctx), None);
    }

    #[test]
    fn arithmetic_overloads_compose() {
        let ctx = TestCtx::default();
        let e = Expr::Const(8.0) + 4.0;
        assert_eq!(eval(&e, &ctx), Some(12.0));
        let e = 16.0 - Expr::Const(4.0);
        assert_eq!(eval(&e, &ctx), Some(12.0));
        let e = (Expr::Const(2.0) + 3.0) * 4.0;
        assert_eq!(eval(&e, &ctx), Some(20.0));
    }

    #[test]
    fn free_propagates_through_arithmetic() {
        let ctx = TestCtx::default();
        let e = Expr::Free + 1.0;
        assert_eq!(eval(&e, &ctx), None);
        let e = 1.0 * Expr::Free;
        assert_eq!(eval(&e, &ctx), None);
    }

    #[test]
    fn sum_of_iterator_works() {
        let ctx = TestCtx::default();
        let e: Expr = vec![Expr::Const(1.0), Expr::Const(2.0), Expr::Const(3.0)]
            .into_iter()
            .sum();
        assert_eq!(eval(&e, &ctx), Some(6.0));
    }

    #[test]
    fn user_example_sum_with_spacers_and_margin() {
        // "Sum of children's heights, 4px between each, 8px margin around"
        // 3 children of heights 10, 20, 30 → 10+20+30 + 4*2 + 16 = 84
        let ctx = TestCtx {
            children_h: vec![10.0, 20.0, 30.0],
            child_count: 3,
            ..Default::default()
        };
        let inner = Expr::children(Axis::Y);
        let spacers = (Expr::ChildCount - 1.0) * 4.0;
        let margins = 16.0;
        let h = inner + spacers + margins;
        assert_eq!(eval(&h, &ctx), Some(84.0));
    }

    #[test]
    fn coalesce_picks_first_resolvable() {
        let ctx = TestCtx::default();
        let e = Expr::coalesce(vec![Expr::Free, Expr::Const(5.0), Expr::Const(99.0)]);
        assert_eq!(eval(&e, &ctx), Some(5.0));
    }

    #[test]
    fn min_of_skips_free() {
        let ctx = TestCtx::default();
        let e = Expr::min_of(vec![Expr::Const(10.0), Expr::Free, Expr::Const(3.0)]);
        assert_eq!(eval(&e, &ctx), Some(3.0));
    }

    #[test]
    fn divide_by_zero_unconstrains() {
        let ctx = TestCtx::default();
        let e = Expr::Const(10.0) / 0.0;
        assert_eq!(eval(&e, &ctx), None);
    }

    #[test]
    fn at_least_check_passes_when_above_min() {
        let bounds = SizeBounds::at_least(10.0, 20.0);
        let ctx = TestCtx::default();
        assert!(bounds.check(15.0, 25.0, &ctx).is_ok());
    }

    #[test]
    fn at_least_check_fails_when_below_min() {
        let bounds = SizeBounds::at_least(10.0, 20.0);
        let ctx = TestCtx::default();
        let v = bounds.check(15.0, 5.0, &ctx).unwrap_err();
        assert_eq!(v.axis, Axis::Y);
        assert_eq!(v.kind, ViolationKind::BelowMin);
        assert_eq!(v.bound, 20.0);
        assert_eq!(v.observed, 5.0);
    }

    #[test]
    fn collapsible_default_passes_zero_size() {
        let bounds = SizeBounds::collapsible();
        let ctx = TestCtx::default();
        assert!(bounds.check(0.0, 0.0, &ctx).is_ok());
    }

    #[test]
    fn at_least_passes_when_min_unresolvable() {
        // min_h is `Child(...)` but the child isn't in the ctx — treat as
        // unconstrained rather than panicking.
        let bounds = SizeBounds {
            min_h: Expr::Child("missing".into(), Axis::Y),
            ..Default::default()
        };
        let ctx = TestCtx::default();
        assert!(bounds.check(10.0, 0.0, &ctx).is_ok());
    }

    #[test]
    fn eps_tolerance() {
        let bounds = SizeBounds::at_least(10.0, 10.0);
        let ctx = TestCtx::default();
        // 9.6 < 10.0 but within 0.5 epsilon
        assert!(bounds.check(10.0, 9.6, &ctx).is_ok());
        // 9.4 outside epsilon → fail
        assert!(bounds.check(10.0, 9.4, &ctx).is_err());
    }
}
