# Worktree subdirectory placement — design

**Date:** 2026-07-30 · **Status:** proposed

## What this is

`--placement worktree` starts the agent at the checkout root, whatever directory the caller was
working in. This makes it open at the same relative path inside the new checkout, so a caller
working in one project of a repository gets an agent working in that project rather than several
directories above it.

It also settles what `--cwd` means for this placement, which today is not what it means for the
other three.

## The failure

Observed directly. A spawn asking for a worktree from a subdirectory:

```
spawn wtest --placement worktree --cwd <repo>/src
```

reported the agent at the checkout root:

```
"path": "<checkout>"
"cwd":  "<checkout>"          ← not <checkout>/src
```

It is structural rather than a slip. `herdr worktree create` takes `--cwd` as *which repository to
cut from* and `--path` as *where the checkout goes on disk*; the root pane it returns always opens
at the checkout root, and no option opens it deeper.

For a repository holding one project this costs nothing — root is where the work is. For a
repository holding several, the agent starts away from the work with every relative path it was
given pointing at the wrong place, and neither the caller nor the agent is told.

## What `--cwd` means

Today `--cwd` means the agent's working directory for `pane`, `tab`, and `workspace`, and the source
checkout for `worktree`. One flag, two meanings, distinguished by another flag's value.

Unify it. `--cwd` is always *where the agent works*; for `worktree` that path is mapped through the
new checkout:

> `--cwd` names a directory. herdr resolves the repository containing it, the worktree is cut from
> that repository, and the agent opens at the same relative path inside the new checkout.

The caller's own directory remains the default, as it is for the other three placements, so the
common case needs no flag at all.

What this removes: there is no longer a way to say *cut from here, but start the agent at the
checkout root*. That is a real loss and it is accepted — see **Decisions recorded**.

## Resolving the relative path

`herdr worktree list --cwd <path>` answers with the repository that contains the path:

```json
{ "source": { "repo_root": "<repo>", "source_checkout_path": "<repo>" } }
```

It resolves from any directory inside the repository, so the relative path is
`cwd` minus `repo_root`, computed from one read-only call made before anything is created. No `git`
process is involved, which keeps the rule that nothing outside the `herdr` module spawns a process
or names a binary.

A caller already at the repository root yields an empty relative path. That case skips the placement
step entirely rather than running a no-op `cd`.

## Placing the agent

```
worktree create                       → checkout path, root pane
pane run <root_pane> "cd -- '<target>'"
agent start                           → inherits the shell's directory
```

This works because `agent start` launches the agent *through the pane's shell* — it appears in the
pane as a typed command line — so the shell's directory at launch is the agent's directory. Verified
end to end before this was written: a `pane run` of a `cd`, then `agent start`, then `agent get`
reporting the subdirectory as both `cwd` and `foreground_cwd`.

One extra herdr call. No pane is split, moved, or closed, so `kill` still closes one pane and the
result shape is unchanged.

### Alternatives rejected

- **`pane split --cwd <target>` then `pane close <root>`.** Passes the path as an argument rather
  than as shell text, which removes the quoting problem below outright — the one real advantage any
  alternative has. Costs two extra calls, and closing the root pane before the split exists would
  take the workspace, and the worktree with it. Not worth the ordering hazard for a quoting rule
  that is small and testable.
- **`tab create --workspace <ws> --cwd <target>`.** Leaves the checkout's root tab behind as a bare
  shell, which changes what a spawn leaves behind and what `kill` does not clean up.
- **A herdr-side `worktree create --pane-cwd PATH`.** The tidier shape: one atomic call, no shell
  text, no readiness question. It is a change to another project for an outcome one `pane run`
  already reaches, so it is recorded under **Open for later** rather than waited on.

## The path becomes shell text

`pane run` takes a command line, so the target path stops being an argument and becomes syntax. This
is the first place in the crate where that happens, and `--cwd` is caller-supplied — the callers
being agents.

The path is single-quoted, with `'` escaped as `'\''`, and `cd` is given `--` so a path that opens
with a dash cannot be read as a flag. A path holding a command separator is then inert rather than
executed.

This lives in the `herdr` module beside the call that needs it, because it is part of spelling that
call, and it is pinned by exact-string tests over a hostile path in the way the envelope's rendering
is. The quoting rule is a wire form: it is what another program parses.

## When the subdirectory is not in the checkout

A directory present in the caller's checkout can be absent from a fresh one — it is ignored, it is
untracked, or it does not exist in the ref `--base` names.

Decided by a filesystem check on `<checkout>/<relative>` before anything is run. The checkout is
local and so is this tool, so this is one `is_dir` rather than a process, a herdr call, or a read of
the shell's own error text. When it is absent the agent starts at the checkout root and the caller is
told why.

Not a refusal. By the time this is knowable the checkout exists, and this crate does not tear down
what it created — the same reason a failure after the surface exists leaves the pane open and names
it. A worktree deleted to report a directory that was ignored anyway is a worse answer than one that
works from the root and says so.

The warning names the directory and nothing else.

## Shell readiness

`pane run` types into a shell, so a `cd` sent before that shell reaches its prompt is lost — and the
agent then starts at the checkout root with nothing said, which is the silent wrong answer this whole
placement is being fixed to avoid. A directory-environment hook in the shell's startup makes the race
ordinary rather than rare.

The retry is the one `agent start` already uses: `Backoff::within(--settle-timeout)`, the same budget
and the same schedule.

The success signal is `pane get`'s `foreground_cwd`, which reports where the shell actually is.
Retrying until it matches the target tests the outcome rather than assuming the command landed. A
budget spent without a match warns and starts at the checkout root, for the reason given above.

Two failures share this step and are deliberately kept apart: an absent directory is answered by the
filesystem check before the loop runs, so the loop only ever waits for a shell.

## Where it lands

| File | Change | Responsibility |
| --- | --- | --- |
| `src/herdr/surface.rs` | Modify | `pane run`, the shell quoting, and the repo-root read |
| `src/cmd/spawn.rs` | Modify | Relative path, the placement step, the two warnings |

## Testing

Every case below is exercised without a herdr process, per the rule that automated tests never
invoke one.

- **The relative path**, as a pure function over `(repo_root, cwd)`: a subdirectory, a nested one,
  the repository root itself yielding nothing to do, and a trailing separator on either side.
- **The quoting**, by exact string, over an ordinary path and over one carrying a quote, a command
  separator, and a leading dash.
- **The absent directory**, by pointing the check at a path that does not exist and asserting the
  target falls back to the checkout root.
- **The warnings**, asserting each names a directory and carries nothing else.

What no test here covers is the ordering against a live shell — that a `cd` lands before
`agent start` types its command. That is a rehearsal, and it is the one thing worth running by hand
against a real session before this is called done.

## Decisions recorded

- **`--cwd` is where the agent works, for every placement.** One flag with two meanings was the
  underlying defect; the subdirectory bug was a symptom. The cost is that *cut from here, start at
  the root* is no longer expressible. It is accepted because the caller's own directory is the
  default, so a caller who wants the root can run from the root, and because a flag whose meaning
  turns on another flag's value is worse than a missing option.
- **`pane run` over `pane split`.** Two herdr calls saved, and the pane the caller ends up with is
  the pane herdr created, rather than one this tool substituted. Paid for with a quoting rule.
- **An absent subdirectory degrades rather than refuses.** The checkout already exists at that point,
  and an agent working from the root is useful where a torn-down worktree is not.
- **`foreground_cwd` over reading the shell's output.** The pane's own report of where it is beats
  matching against text a shell chose to print, which changes with the shell.

## Open for later

- **The trust dialog.** A fresh checkout is a path the agent's harness has never seen, so it opens
  on a trust prompt rather than a composer — while herdr reports the pane interactive and ready.
  Prompting into that state answers the dialog and consumes the prompt, and only the existing
  re-send rescues it. Tabled deliberately: the local fix is a blanket trust that helps one machine
  and hides the problem everywhere else. Nothing in this document depends on it, and it is worth
  fixing on its own terms.
- **`worktree create --pane-cwd PATH` in herdr.** Would reduce the placement step to nothing and
  delete the quoting rule with it.
