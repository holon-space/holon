---
id: 2026-09-10-user-launched-gpui-app-never-installed-the-editor-cell-registry
date: 2026-09-10
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  The user-launched GPUI app never installed the block-cell registry, so every
  keystroke wrote through the per-keystroke whole-buffer `set_field`
  fire-and-forget funnel instead of the CRDT cell leg the editor is built on.
---

## Bug

`GpuiModule::on_start` (`frontends/gpui/src/di.rs:108-131`) never installed
`BlockCellRegistry` on the `ReactiveEngine`. The whole-tree writers of that field
were: the `None` initialiser, `holon-app/src/loro_seams.rs` (the shared
installer, called only by harnesses), and `frontends/tui/src/di.rs:88`. The GPUI
path that DID install it — `frontends/gpui/src/reset.rs` — is the MCP
**reset** path (callers: `gpui_rebind_reset_smoke.rs`, `main.rs:280`
`reset_vault`), not start-up.

So the shipping app resolved no `Cell`, and every keystroke fell to the on-blur
/ per-keystroke whole-buffer `set_field` funnel
(`crates/holon-frontend/src/editor_view_model.rs:622-668`). The CRDT cell leg —
which `editor_view.rs:165-184` attaches and seeds `InputState` from — was dead
code in production.

Found by a fresh-context verifier reviewing the `slot-birth` lane, outside any
automated test. No test could have caught it: every harness installs the
registry itself, so every rung ran the leg production did not.

**Fail-loud violation, and it is what hid this.** `editor_view.rs:165` read
`if let Ok(cell) = services.editable_text(...)`, discarding the `Err`. The
"BlockCellRegistry not wired" error was raised and thrown away on every editor
mount, so a whole missing leg produced no log line, no banner and no failure —
the "silently degrades to look fine" case `CLAUDE.md` forbids.

**Listed consequence, not separately reproduced.** The funnel it silently fell
back to dispatches a whole-buffer `set_field` per keystroke as an independent
`tokio::spawn`. Unordered spawns mean a slow write can land after a later one,
so keystrokes can be dropped or reordered. This lane measured exactly that shape
on an adjacent case (typing `ab` yielding `ba`, entry
`2026-09-10-second-keystroke-into-a-fresh-slot-inserts-at-caret-zero`); it is
recorded here as a consequence of the fallback rather than as its own escape,
because it was not reproduced through this path specifically.

## Root cause

An omission, not a decision. The tree's own documentation says the opposite of
what the code did:

- `crates/holon-app/src/wiring.rs:260-264` — "Frontends install it on their
  `ReactiveEngine` at start-up through `install_block_cell_registry` — one
  function, so a harness cannot boot a subtly different frontend than
  production."
- `crates/holon-app/src/loro_seams.rs:716-725` — "Production start-up and every
  test harness call this… a harness that installs the registry differently — or
  not at all — is testing a frontend the user never runs."

No comment anywhere opts GPUI out, and the cell leg was built for GPUI.

## Missing piece

No gate boots the user-launched app's own module graph. Every windowed and
headless harness assembles the frontend itself and installs the registry on the
way, so the one configuration that lacked it was the only one never exercised.

## Remedy

**OPEN — the GPUI app deliberately stays on the no-cell leg until the
`cell-undo` lane lands (D113.a).** Installing the registry puts every keystroke
on the cell leg, and that leg has no undo: cmd+z after typing restores nothing
(`2026-09-11-cmd-z-restores-nothing-after-typing-through-the-editor-cell`).
Shipping the install before undo covers cell writes would trade this escape for
a worse, user-visible one, so the install is the `cell-undo` lane's last step.

What the `slot-birth` lane DID land, all of it still in place:

- `GpuiModule::on_start` does not install the registry, and says why at the
  call site. The TUI's `on_start` and the MCP reset path still do; the TUI has
  no undo binding, so its behaviour is unchanged.
- That installer returns a typed `Result<CellRegistryInstall>` instead of a
  discarded `bool`: `Err` when CRDT is enabled and no registry resolves,
  `Ok(NotWired)` only when CRDT is off. Every caller propagates or asserts.
- The swallow at `editor_view.rs:165` is gone. A missing REGISTRY now logs at
  ERROR naming the consequence; a single row with no node yet — a creation slot
  before its first keystroke — stays the ordinary quiet branch, distinguished by
  the new `BuilderServices::cell_registry_wired`.
- `frontends/tui/src/di.rs` was a second install implementation with different
  semantics; it now calls the shared one.
- `TestEnvironment::start_app` makes the install OPT-IN
  (`enable_block_cell_registry`), so a windowed fixture defaults to the leg the
  app runs. Only the slot-birth windowed PBT opts in, because the in-process
  birth IS its subject.
