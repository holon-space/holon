# Kickoff prompt for next session

Copy the prompt below into a fresh `claude` session in this repo:

---

Please read `devlog/handoff-block-tree-chord-ops-followup.md` first — it captures what the previous session fixed and where the PBT now stops.

Goal for this session: close **Bug X** from that handoff — `ToggleState` after `SwitchView { view_name: "sidebar" }` times out at `crates/holon-integration-tests/src/pbt/sut.rs:1727` with "entity ... did not appear in the resolved ViewModel within 5s".

Minimal shrink reproducer (with chord-op weights still on):
```sh
PROPTEST_CASES=20 PBT_WEIGHT_INDENT=10 PBT_WEIGHT_OUTDENT=10 PBT_WEIGHT_SPLIT_BLOCK=10 \
PBT_WEIGHT_CLICK_BLOCK=10 PBT_WEIGHT_DEFAULT=1 \
cargo nextest run -p holon-integration-tests --test general_e2e_pbt \
  -E 'binary(general_e2e_pbt) and test(=general_e2e_pbt)' 2>&1 | tee /tmp/pbt-toggle-after-switchview.log
```

The shrink involves:
1. `WriteOrgFile` of a single block with query/render/GQL src children
2. `StartApp`
3. `SwitchView { view_name: "sidebar" }`
4. `ToggleState { block:..., new_state: "DONE" }` ← times out

Two leading hypotheses (handoff §"What's left" → "Bug X"):
- **(a)** Ref-state's `SwitchView` apply branch should clear `focused_entity_id[Main]` when the new view doesn't render the previously-focused entity, so `ToggleState` preconditions reject the transition rather than firing it against a panel that no longer shows the block.
- **(b)** The SUT should wait longer or re-navigate after `SwitchView` so the entity surfaces in the main panel.

(a) is more conservative — it makes the ref-state precondition match what production can actually act on. Start there: look at `state_machine.rs` `SwitchView` apply + `ToggleState` precondition. Check `reference_state.rs` for `current_view` / `focused_entity_id` state and how view changes interact with focus.

Once Bug X is closed, push `PROPTEST_CASES` to 100+ overnight to surface deeper interactions before declaring chord-ops settled.

Standing rules from `CLAUDE.md`:
- Always `tee` before filtering test output.
- No defensive programming / no swallowed errors.
- Use `debugger-mcp` over `eprintln` when investigating.
- Don't add new PBT tests — extend `general_e2e_pbt.rs` or fix preconditions/refs.
