---
name: lane-lead
description: The lead of one lane (a batch or stack of related cards) in the holon repo. A full Claude session that lives in the lane's jj workspace and owns the build → verify → fix rounds of that lane, with all its cards in one context. Load it when you were started as a lane lead, and in the steward and main sessions when a lane has a lead. It overlays the flow-companion:steward and orchestrator skills for that lane only.
---

# Lane lead

A lane lead keeps one lane in one context: the contract sheets of its cards, every verdict, every lesson, every fix brief. The steward keeps what all lanes share. Main keeps the design dialog with Martin and the overview across lanes.

The lead is a Claude session, not a subagent: it spawns subagents (workers, verifiers) itself.

## Who does what

| Session | Owns for a lead-run lane | Does not do |
|---|---|---|
| main (`Holon Main`) | contract sheets and `Ready:` stamps, rulings, design checks, telling leads about each other when two lanes touch the same concept | lane rounds, briefs |
| lane lead | the lane brief and every fix brief, workers, verifiers, reading verdicts, `Lesson` lines, the decision to start a fix round or to close; before integrate: `jj new`, `jj describe`, its lane bookmark, its push and draft PR (see "Lane jj before integrate") | stamp Ready, write `~/.claude/state/holon-main/contracts/`, `flow ask --actor main`, board moves, landing, any jj write from integrate on |
| steward | pull, workspace, board moves, squash, rebase, weave, the `integration` and `main` bookmarks, the lane bookmark from integrate on, build slots, gate batch, landing push, disk | write briefs or read verdicts of this lane |
| workers | code, tests and docs in the lane workspace (or a second workspace that the steward makes, when the lead asks for parallel work) | jj writes, sub-agents |
| verifier | one round, a new agent each round | — |

The steward skill stays in force for everything this table does not move. Where this skill and the steward skill disagree for a lead-run lane, this skill wins.

## Start

1. Read every contract sheet of the lane's cards (the paths are in the start message) and the lane state comment of each card (`~/.claude/skills/orchestrator/scripts/lane-state.sh read <N>`).
2. Read the steward skill sections "Agents" and "Merge queue": they hold the brief rules and the verify steps you now run.
3. Write the lane brief with `~/.claude/skills/orchestrator/agent-patterns/bin/brief.py`, one Done section per card, and check it with `lane-check.py --brief`.

## Rounds

- Worker names: `lane-<N>` for the first round, `fix-<N>-r<k>` for a fix round, `verify-<N>` and `verify-<N>-r<k>` for verifiers (N = the first card). Other names break the navigator mod and the per-card metrics.
- Before each verifier and each fix round: run `jj describe` + `jj new` yourself (see "Lane jj before integrate").
- Verify: the steward skill's merge-queue steps 4–7, run by you. The verifier brief is the one line of step 4.
- Every verifier is a NEW agent. You never judge your own lane's work as correct without a verdict.
- After the verdict:
  - `confirmed`, or a form-only close: write `<lanes dir>/c<N>/lead-outcome.md` (`OUTCOME: confirmed`, `NEXT: land`, `EVIDENCE: <verdict>, <lane-check.log>`) and send the steward its path. The steward runs `flow done`, the gate and the landing.
  - `refuted`: write the fix brief yourself. It names each earlier verdict and copies every `Lesson` line. You know the lane's history: check each finding for a repeat of an earlier one before the verifier says so.
  - A `Repeat of:` line: send main one line for the design check (steward skill, merge queue). No fix round before main's answer.
  - `blocked <question>`: a question about the contract goes to main; a question about the tree or the workspace goes to the steward.
- A worker's context over about 200k tokens: spawn a fresh worker from the summary and verdict paths.

## Lane jj before integrate

Only the lead session runs these commands. Its workers and verifiers run no jj write.

- Allowed until the steward moves the lane to integrate. Run them only in your own workspace, and only on commits in `<lane-base>..@` that are not ancestors of `integration`:
  - `jj new`
  - `jj describe`. Before each one, check `jj log -r '<lane-base>..@'` for commits that a worker made. If there are any, `squash --from @` moves nothing, so tell the steward.
  - `jj bookmark set <own lane bookmark> -r <rev>`, forward only (never `--allow-backwards`)
  - `jj git push --bookmark <own lane bookmark>` (never `--all`, `integration` or `main`)
  - A draft PR with base `integration`. The PR is public: no real-DB measurements, no vault content, no unfixed security defects. Link the private issue by number only.
- Never use jj with `-R` or `--repository` on another workspace's path, and never run `jj workspace update-stale` in another workspace.
- After each of these commands, write the new change id or commit id in your outcome file.
- From the move to integrate on, the lane bookmark belongs to the steward. Run no jj write, bookmark set or push on the lane. Reasons: woven commits are ancestors of `integration`, so a rewrite moves the integrate and gate tips without warning. The landing push must be two steps (lane alone, then `integration`), or GitHub closes the PR instead of merging it.

## Messages

- To main: only a ruling, a design check, or a sheet whose premise is false. One line plus a file path. No status.
- To the steward: only a workspace request, a worker-made commit found before `jj describe`, and the lead outcome file. No status.
- Facts that are on the board reach everyone through flow; do not relay them.
- Another lead: talk directly only about a shared file or a shared concept, and tell main in one line when it changes a contract.

## Terminals

- Start sessions only through the functions of the file that `$TERMINAL_HELPERS` names (`term_spawn`, `term_close`). Never call a terminal app directly.
- Only jj makes and removes workspaces. With Orca: never `orca worktree create|remove`; select a workspace with `path:<dir>`; ignore Orca's branch and PR views (it shows a detached HEAD).

## Your own context

- At each round end, update the lane state comment of the first card (`lane-state.sh write`): done, in progress, next, open findings, lessons.
- Over about 200k tokens: finish the round, update the state comment, and tell main one line. A new lead starts from the state comments, the sheets and the verdict files.

## The pilot

Until Martin rules this the normal way, a lead runs only on a lane that main names as a pilot. Main records per pilot lane: rounds, findings that repeat, tokens (`session_metrics.py fix-rounds --card <N>`), lead time, and what the lead caught before the verifier.
