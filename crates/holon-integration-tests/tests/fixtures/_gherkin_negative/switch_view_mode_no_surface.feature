Feature: A view-mode switch on a block that renders no switcher must fail loud

  Negative control for `SwitchViewMode`. The headless composed medium surfaces
  no `view_mode_switcher` at all (`LayoutRefState::switchable_handles` on
  `ReferenceState` is empty), so this switch CANNOT take effect. Strict replay
  must refuse it as `NoModeSwitchableSurface`; the ONLY other outcome the
  machinery can produce is
  the silent no-op this control exists to forbid — the click resolves the
  `vms_button::…` handle to an entity nothing answers, focus lands on a ghost
  URI, and the scenario passes having changed nothing.

  Scenario: Switching a block that offers no view modes
    When I switch block "block:c1" to view mode "table_view"
