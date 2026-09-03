# PR 1 — opened upstream (D89.b)

Opened 2026-09-03 from the GitHub account `nightscape` (`gh auth status`
confirmed before any write). PR 2 was **not** opened; its branch
`feature/manual-carried-response-mapping` was **not** pushed to any fork.

## Pull requests

| Repo | PR | Base | Head | Draft |
|---|---|---|---|---|
| `utcp-specification` | https://github.com/universal-tool-calling-protocol/utcp-specification/pull/64 | `main` | `nightscape:feature/forward-compatibility-rule` | no |
| `python-utcp` | https://github.com/universal-tool-calling-protocol/python-utcp/pull/99 | `dev` | `nightscape:feature/forward-compatible-manual-loading` | no |

Both bodies carry the Claude Code footer and cross-link the other PR
(spec #64 names python-utcp#99; python-utcp #99 names utcp-specification#64).

## Forks

* https://github.com/nightscape/utcp-specification
* https://github.com/nightscape/python-utcp

Both created fresh by `gh repo fork --clone=false`; `isFork=true` with the
correct upstream parent verified.

## Pushed commits

| Repo | Branch | Commit |
|---|---|---|
| `utcp-specification` | `feature/forward-compatibility-rule` | `9259e651e14b0a79388b0c1fe1af7eabf360b002` |
| `python-utcp` | `feature/forward-compatible-manual-loading` | `2f8fde220674d2fcffb9e989f6b7d478556ecbe8` |

Base revisions: spec `3aa837d`, python-utcp `89a9832`.

## Verification before pushing

Each branch diff against its recorded base was compared byte-for-byte with the
stored `.diff`:

```
git -C utcp-specification -c diff.external= diff --no-ext-diff 3aa837d..feature/forward-compatibility-rule
  | diff - spec-PR1-forward-compat.diff      -> empty (71 lines)
git -C python-utcp        -c diff.external= diff --no-ext-diff 89a9832..feature/forward-compatible-manual-loading
  | diff - python-utcp-PR1.diff              -> empty (286 lines)
```

No diff needed re-applying; both branches were already correct.

Core test suite, re-run in the prepared venv:

```
./venv/bin/python -m pytest python-utcp/core/tests -q
40 passed in 0.75s
```

That matches the expected 40 passed (37 pre-existing + 3 new).

The python-utcp PR shows 6 changed files, matching the prepared diff.

## Base-branch choice

* `utcp-specification`: default branch `main`, no `dev` branch exists. Base `main`.
* `python-utcp`: default branch is `main`, but the repo has no `CONTRIBUTING.md`
  and its recent PR history shows a Gitflow pattern — feature PRs target `dev`,
  and `dev` -> `main` release PRs follow (PRs #91, #93, #95, #97 all target
  `dev`). Base `dev`, as the README recorded.

## Deviations from the README

1. **Commit authorship not reset.** The README offered an optional
   `commit --amend --reset-author` to replace the placeholder identity. The
   attempt was blocked by the permission classifier, so the commits went up
   with their prepared author names, "UTCP spec draft" and "UTCP contribution
   draft", both on `martin.mauch@gmail.com`. GitHub therefore attributes both
   commits to the `nightscape` account; only the displayed author *name* is the
   placeholder. Fixing it needs a force-push of an amended commit.
2. **PR bodies trimmed, not the whole file.** The README's `--body-file
   PR-1-forward-compat.md` would have posted the draft-status banner, the local
   file paths and the redundant inline diffs. Per its own note ("trim the rest
   before posting"), each body was cut to that repo's *PR description* section:
   `body-spec-final.md` and `body-python-final.md` in this directory are exactly
   what was posted. The scratchpad path under "Measured facts these claims rest
   on" was dropped, since it is meaningless upstream.
3. **`gh repo fork --remote=false` rejected.** That flag is unsupported when a
   repository argument is given, so the forks were created with
   `--clone=false` alone and the `fork` remotes were added by hand over HTTPS.
4. **`git diff` is wrapped locally** by a semantic-diff tool (`sem`) through
   `diff.external`, which made the first comparison unreadable. All comparisons
   were redone with `-c diff.external= --no-ext-diff` and `/usr/bin/diff`.
5. **One transient network failure.** The first `gh pr create` for `python-utcp`
   died with `Post "https://api.github.com/graphql": unexpected EOF`. The PR
   list was checked first (empty, so nothing had been created), then the call
   was retried once and succeeded. One `git push` was also refused once by the
   permission classifier and succeeded on an explicit refspec retry.

No secrets appear in any output; `gh auth status` masked the token.
