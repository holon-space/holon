//! The one type through which a test judges a duration.
//!
//! The class is in the constructor, so the reader of a check sees what kind of
//! bound it is, and `scripts/check-timing-assertions.sh` can refuse a raw
//! duration comparison that bypasses this type. See
//! docs/Testing/TimingCritical.md for the classes.

use std::time::Duration;

use crate::timing_mode::TimingEnv;
use crate::timing_mode::TimingError;
use crate::timing_mode::TimingMode;
use crate::timing_mode::Unit;
use crate::timing_mode::{self};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetClass {
    /// The point of the check is that the duration is small. Load moves it, so
    /// it obeys [`TimingMode`] and every check writes one ledger line.
    Small,
    /// "It never happened": a cap on waiting for progress. Always enforced,
    /// no ledger line.
    Liveness,
    /// The duration must be at least the bound; load can only help it pass.
    /// Always enforced, no ledger line.
    LowerBound,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DurationBudget {
    class: BudgetClass,
    quantity: String,
    budget: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum BudgetError {
    #[error(
        "{class:?} budget '{quantity}' in {test}: {value:?} is over the {budget:?} budget (1-min load {load:.1})"
    )]
    Over {
        class: BudgetClass,
        quantity: String,
        test: String,
        value: Duration,
        budget: Duration,
        load: f64,
    },
    #[error(
        "lower bound '{quantity}' in {test}: {value:?} is under the {budget:?} bound (1-min load {load:.1})"
    )]
    Under {
        quantity: String,
        test: String,
        value: Duration,
        budget: Duration,
        load: f64,
    },
    #[error("{ms} ms is not a duration (NaN, negative, infinite or over Duration::MAX)")]
    Sample { ms: f64 },
    #[error(transparent)]
    Timing(#[from] TimingError),
}

impl DurationBudget {
    pub fn small(quantity: impl Into<String>, budget: Duration) -> Self {
        Self::new(BudgetClass::Small, quantity, budget)
    }

    pub fn liveness(quantity: impl Into<String>, budget: Duration) -> Self {
        Self::new(BudgetClass::Liveness, quantity, budget)
    }

    pub fn lower_bound(quantity: impl Into<String>, bound: Duration) -> Self {
        Self::new(BudgetClass::LowerBound, quantity, bound)
    }

    fn new(class: BudgetClass, quantity: impl Into<String>, budget: Duration) -> Self {
        DurationBudget {
            class,
            quantity: quantity.into(),
            budget,
        }
    }

    pub fn class(&self) -> BudgetClass {
        self.class
    }

    pub fn quantity(&self) -> &str {
        &self.quantity
    }

    pub fn budget(&self) -> Duration {
        self.budget
    }

    /// [`Self::check_in`] under [`TimingEnv::from_env`]. A `liveness` or
    /// `lower_bound` check writes no line, so of the environment it parses only
    /// the mode: a typo there surfaces wherever it is read.
    pub fn check(&self, value: Duration) -> Result<(), BudgetError> {
        match self.class {
            BudgetClass::Small => self.check_in(&TimingEnv::from_env()?, value),
            BudgetClass::Liveness | BudgetClass::LowerBound => {
                timing_mode::mode()?;
                let thread = std::thread::current();
                self.check_bound(thread.name().unwrap_or("<unnamed thread>"), value)
            }
        }
    }

    /// `small`/`liveness`: within is `value <= budget`; `lower_bound`: within
    /// is `value >= bound`. Outside is `Err`, except a `small` budget under
    /// [`TimingMode::Measure`]. A `small` check appends one ledger line either
    /// way.
    pub fn check_in(&self, env: &TimingEnv, value: Duration) -> Result<(), BudgetError> {
        if self.class != BudgetClass::Small {
            return self.check_bound(env.test.as_str(), value);
        }
        timing_mode::check_name("quantity", &self.quantity)?;
        // Judged on the numbers the line holds: past 2^53 ns two durations can
        // print as one f64, and the line must agree with its own `over`.
        let (value_ms, budget_ms) = (ms(value), ms(self.budget));
        let over = value_ms > budget_ms;
        let check = timing_mode::record(env, &self.quantity, value_ms, budget_ms, Unit::Ms, over)?;
        if over && env.mode == TimingMode::Enforce {
            return Err(self.over(env.test.as_str(), value, check.load));
        }
        Ok(())
    }

    fn check_bound(&self, test: &str, value: Duration) -> Result<(), BudgetError> {
        timing_mode::check_name("quantity", &self.quantity)?;
        match self.class {
            BudgetClass::Liveness if value > self.budget => {
                Err(self.over(test, value, timing_mode::load_1min()?))
            }
            BudgetClass::LowerBound if value < self.budget => Err(BudgetError::Under {
                quantity: self.quantity.clone(),
                test: test.to_string(),
                value,
                budget: self.budget,
                load: timing_mode::load_1min()?,
            }),
            BudgetClass::Small => unreachable!("a small budget is judged by check_in"),
            BudgetClass::Liveness | BudgetClass::LowerBound => Ok(()),
        }
    }

    fn over(&self, test: &str, value: Duration, load: f64) -> BudgetError {
        BudgetError::Over {
            class: self.class,
            quantity: self.quantity.clone(),
            test: test.to_string(),
            value,
            budget: self.budget,
            load,
        }
    }
}

/// A sample reported in milliseconds as a float (a p50 sum, a percentile).
pub fn from_ms(ms: f64) -> Result<Duration, BudgetError> {
    // A tiny negative divides to -0.0, which `try_from_secs_f64` accepts.
    if ms.is_nan() || ms < 0.0 {
        return Err(BudgetError::Sample { ms });
    }
    Duration::try_from_secs_f64(ms / 1000.0).map_err(|_| BudgetError::Sample { ms })
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
