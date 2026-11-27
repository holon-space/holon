# Terminal helpers for Orca. Scripts source the file that $TERMINAL_HELPERS names;
# every backend file defines the same functions.
# Only jj makes and removes workspaces: Orca sees a jj workspace through its colocated git
# worktree (one way only, https://github.com/stablyai/orca/issues/1082), so never run
# `orca worktree create|remove`; select the workspace with `path:`.

# term_spawn <title> <dir> <command>: open a tab titled <title> in <dir>, run <command>, print the handle.
term_spawn() {
  orca terminal create --worktree "path:$2" --title "$1" --command "$3" --json | jq -er '.result.terminal.handle'
}

# term_close <handle>
term_close() {
  orca terminal close --terminal "$1"
}
