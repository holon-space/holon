//! Session status derivation per the H8 matrix (Hypotheses.org):
//!
//! | Last meaningful role | Age of last activity | Derived status  |
//! |----------------------|----------------------|-----------------|
//! | user                 | < 30 sec             | active          |
//! | assistant            | < 5 min              | waiting-on-user |
//! | (anything else)      |                      | idle            |
//!
//! No Claude Code hooks — this is a heuristic over the session's modified
//! timestamp + last-message role, recomputed on every sync pass.

use chrono::{DateTime, Duration, Utc};

use super::boundary::LastRole;

/// How long after a user message the session counts as actively processing.
pub const ACTIVE_WINDOW: Duration = Duration::seconds(30);
/// How long after an assistant message the session counts as waiting on the user.
pub const WAITING_WINDOW: Duration = Duration::minutes(5);

/// Derived liveness status of a Claude Code session. Closed enum — the raw
/// role/timestamp signals never leave the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// User spoke last and very recently — Claude is (likely) processing.
    Active,
    /// Assistant spoke last and recently — the session is waiting on the user.
    WaitingOnUser,
    /// No recent activity.
    Idle,
}

impl SessionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::WaitingOnUser => "waiting_on_user",
            Self::Idle => "idle",
        }
    }

    /// Derive the status from the last meaningful role and the session's
    /// modified timestamp. A `modified` in the future (clock skew between
    /// the MCP server's machine and ours) counts as age zero.
    pub fn derive(last_role: LastRole, modified: DateTime<Utc>, now: DateTime<Utc>) -> Self {
        let age = (now - modified).max(Duration::zero());
        match last_role {
            LastRole::User if age < ACTIVE_WINDOW => Self::Active,
            LastRole::Assistant if age < WAITING_WINDOW => Self::WaitingOnUser,
            _ => Self::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> DateTime<Utc> {
        "2026-07-04T12:00:00Z".parse().unwrap()
    }

    fn at(seconds_before_now: i64) -> DateTime<Utc> {
        t0() - Duration::seconds(seconds_before_now)
    }

    #[test]
    fn status_matrix() {
        use LastRole::*;
        use SessionStatus::*;
        let cases = [
            (User, 0, Active),
            (User, 29, Active),
            (User, 30, Idle),          // user-last past the active window is not "waiting"
            (User, 299, Idle),
            (Assistant, 0, WaitingOnUser),
            (Assistant, 29, WaitingOnUser),
            (Assistant, 299, WaitingOnUser),
            (Assistant, 300, Idle),
            (User, 10_000, Idle),
            (Assistant, 10_000, Idle),
        ];
        for (role, age_secs, expected) in cases {
            let got = SessionStatus::derive(role, at(age_secs), t0());
            assert_eq!(got, expected, "role={role:?} age={age_secs}s");
        }
    }

    #[test]
    fn future_modified_counts_as_age_zero() {
        let future = t0() + Duration::seconds(120);
        assert_eq!(
            SessionStatus::derive(LastRole::User, future, t0()),
            SessionStatus::Active
        );
        assert_eq!(
            SessionStatus::derive(LastRole::Assistant, future, t0()),
            SessionStatus::WaitingOnUser
        );
    }
}
