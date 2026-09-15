//! `RefRemoteListSync` — the remote-list sync oracle read cap.
//!
//! @pbt kind ref
//! @pbt covers remote-list-mirror-matches-ref — the fixture peer's declared
//!   list, which the SUT mirror must equal after every round.
//!
//! Reads the peer list the `RemoteListSync` transition maintains. The shape is
//! whatever [`crate::pbt::remote_list_fixture`] declares; nothing here names a
//! type beyond that shared source of truth.

use holon_pbt_core::capabilities::RefRemoteListSync;

use super::super::reference_state::ReferenceState;

impl RefRemoteListSync for ReferenceState {
    fn remote_list_expected_rows(&self) -> Vec<Vec<String>> {
        self.remote_list.expected_rows()
    }
}
