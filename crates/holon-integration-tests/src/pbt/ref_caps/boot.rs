//! `RefLifecycle` / `RefBoot` / `RefBootMut`.

use holon_api::Region;
use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::RefBoot;
use holon_pbt_core::capabilities::RefBootMut;
use holon_pbt_core::capabilities::RefLifecycle;

use super::super::reference_state::ReferenceState;

impl RefLifecycle for ReferenceState {
    fn app_started(&self) -> bool {
        self.action.app_started
    }
    fn is_properly_setup(&self) -> bool {
        self.is_properly_setup()
    }
    fn enable_loro(&self) -> bool {
        self.wiring
            .has_storage(holon_pbt_core::StorageAdapter::Loro)
    }
    fn has_editor_buffer(&self) -> bool {
        ReferenceState::has_editor_buffer(self)
    }
    fn renders_block_interactively(&self, block_id: &EntityUri) -> bool {
        ReferenceState::renders_block_interactively(self, block_id)
    }
    fn last_transition_kind(&self) -> Option<&'static str> {
        self.action.last_transition_kind
    }
    fn next_doc_id(&self) -> usize {
        self.action.next_doc_id
    }
    fn next_block_id(&self) -> usize {
        self.domain.block_state.next_id
    }
    fn has_undo_history(&self) -> bool {
        !self.action.undo_stack.is_empty()
    }
    fn has_redo_history(&self) -> bool {
        !self.action.redo_stack.is_empty()
    }
}

impl RefBoot for ReferenceState {
    fn pre_startup_directory_count(&self) -> usize {
        self.pre_startup_directories.len()
    }
    fn pre_startup_file_count(&self) -> usize {
        self.pre_startup_file_count
    }
    fn git_initialized(&self) -> bool {
        self.git_initialized
    }
    fn jj_initialized(&self) -> bool {
        self.jj_initialized
    }
    fn root_layout_block_id(&self) -> Option<EntityUri> {
        ReferenceState::root_layout_block_id(self)
    }
}

impl RefBootMut for ReferenceState {
    fn push_pre_startup_directory(&mut self, path: &str) {
        self.pre_startup_directories.push(path.to_string());
    }
    fn mark_git_initialized(&mut self) {
        self.git_initialized = true;
    }
    fn mark_jj_initialized(&mut self) {
        self.jj_initialized = true;
        self.git_initialized = true; // jj git init also creates .git
    }
    fn boot_app(&mut self) {
        use crate::pbt::transitions::start_app::SEEDED_SIDEBAR_WATCH_ID;
        use crate::pbt::transitions::start_app::load_seed_profile_into_ref;
        use crate::pbt::transitions::start_app::seed_booted_layout_into_ref;
        use crate::pbt::transitions::start_app::seeded_sidebar_watch_spec;

        self.action.app_started = true;

        // Freshness mirrors prod `seed_default_layout`: the default layout is only
        // seeded when `block:root-layout` is absent at boot. A pre-startup user
        // `index.org` keeps the well-known root id, so it suppresses the default seed.
        let fresh = !self
            .domain
            .block_state
            .blocks
            .contains_key(&holon_api::root_layout_block_uri());

        // Default layout boots both sidebars as open drawers.
        if fresh {
            self.ui
                .tab
                .drawer_open
                .insert("block:default-left-sidebar".to_string(), true);
            self.ui
                .tab
                .drawer_open
                .insert("block:default-right-sidebar".to_string(), true);
        }

        seed_booted_layout_into_ref(self, fresh);

        // Register the production seeded left-sidebar watch on the ref side.
        self.mcp.active_watches.insert(
            SEEDED_SIDEBAR_WATCH_ID.to_string(),
            seeded_sidebar_watch_spec(),
        );

        load_seed_profile_into_ref(self);

        // FU-10 mirror: prod `seed_default_layout` calls `navigation::focus(Main,
        // block:journals)` on fresh DBs ONLY, inserting a navigation_history row.
        if fresh {
            let journals_uri = EntityUri::block("journals");
            let history = self
                .ui
                .tab
                .navigation_history
                .entry(Region::Main)
                .or_default();
            history.entries.truncate(history.cursor + 1);
            history.entries.push(Some(journals_uri.clone()));
            history.cursor = history.entries.len() - 1;

            let history_id = self.ui.tab.next_history_id;
            self.ui.tab.next_history_id += 1;
            let added_ts_logical = self.ui.user.next_pin_ts;
            self.ui.user.next_pin_ts += 1;
            let pins = self.ui.user.open_pins.entry(Region::Main).or_default();
            pins.clear();
            pins.push(super::super::ui_types::OpenPinEntry {
                history_id,
                block_id: Some(journals_uri),
                added_ts_logical,
            });
        }
    }
}
