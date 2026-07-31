# herdr-team

Launching a [herdr](https://herdr.dev) agent by hand is two steps: herdr starts an agent only in a
pane that already exists and is sitting at an interactive shell prompt, so you create a surface,
dig the new pane's id out of the JSON, and then start the agent in it. This does both in one
command, resolves the agent's kind and its usual flags from a named agent in the config, and takes
message text on stdin. It runs with no config at all: `--kind` names a herdr agent kind directly.

## Commands

| Command | Purpose |
| ------- | ------- |
| `spawn` | Create a pane, tab, or workspace and start a configured agent in it |
| `msg` | Deliver a message to an agent that already exists |
| `kill` | Close an agent's pane, refusing one that is mid-task |
| `agents` | List what the config holds |
| `prime` | Print an agent-facing brief on driving this CLI |

`prime` is written for a session-start hook. `--hook <harness>` asks that harness to wrap the brief
in its host's envelope; each harness owns its own shape, so a host whose contract differs is one impl
rather than a flag change. It makes no herdr call and a config it cannot read costs the agent table
and nothing else, because a hook that fails is worse than one that says little.

```
herdr-team spawn reviewer --placement tab --agent opus --msg "Review the branch"
herdr-team spawn scratch --kind codex
git diff | herdr-team msg reviewer -
herdr-team kill reviewer
herdr-team agents
```

`msg` and `kill` each refuse one thing by default, and `--force` is the override for both: a
composer holding someone's unsent text, and an agent still working or blocked. Neither refusal
changes anything, and both exit 5. Neither clears on a timer either — a composer is cleared by the
person typing into it — so the retry belongs after the named state changes, not on a loop.

The command was called `prompt` until it started wrapping what it sends, and `msg` still answers to
that name. So does `spawn --prompt`, now `--msg`.

## Without a config

`spawn --kind <kind>` starts an agent kind directly and reads no config at all — not the user's, not
the repository's, not one `--config` names. Everything a config would have supplied is said on the
line instead:

```
herdr-team spawn scratch --kind codex -- --no-alt-screen
```

The kind goes to herdr untouched, for the reason the config's own `kind` field is a plain string:
herdr answers `unsupported_agent_kind` from a list this build does not restate, so a kind herdr
learns tomorrow works without a release here. `--kind` is refused beside `--agent` and `--config`,
which it would otherwise silently ignore.

A config earns its keep once you want a name for a set of flags, a default, or a brief.

## Agents

`$XDG_CONFIG_HOME/herdr-team/config.toml`, falling back to
`~/.config/herdr-team/config.toml`. Override the location with `--config` or with
`HERDR_TEAM_CONFIG`. `examples/config.toml` in this repository is a worked one to copy.

Agents are what the file holds today, and the filename deliberately does not say so: a name
that names one table has to change the first time a second one is added.

```toml
default = 'reviewer'

[agents.cc]
kind = 'claude'
args = ['--disallowed-tools', 'AskUserQuestion']

[agents.reviewer]
base = 'cc'
model = 'opus'
effort = 'xhigh'
```

The agent name is the table key, so a duplicate name is inexpressible. `args` is an array only:
a string form would have to be split into shell words, and herdr takes the agent's arguments as an
argument vector, so nothing here needs shell quoting.

An agent may inherit from another. `base` names one, and every field the child states wins —
except `args`, which appends after the base's, since agent CLIs are last-flag-wins and a child
adding one flag should not have to restate the rest.

`model` and `effort` are fields rather than flags because each CLI spells them differently: Claude
Code takes `--model opus --effort xhigh`, Codex takes `--model … -c model_reasoning_effort=xhigh`.
This tool knows how to drive two of herdr's kinds, so an agent that sets either under a third is
dropped with a warning naming the kinds that can express them.

`prompt_file` names a file that is prepended to the agent's first prompt, resolved relative to the
directory of the config file that declared it:

```toml
[agents.reviewer]
base = 'opus'
prompt_file = 'review.md'
```

`spawn reviewer --msg "start with auth"` then delivers the file's contents, a blank line, and the
message, in one submission — an agent handed two would answer the first before it heard the second.
A file that is missing or blank is a refusal, raised before anything is created.

The brief rides *outside* the mail envelope. A message is wrapped in `<mail from="…">` naming who
sent it, and a brief has no sender to name: it comes from a file the recipient's own config points
at, so wrapping it would have the envelope claim that whoever ran `spawn` wrote it. With no `--msg`
the brief is delivered alone and unwrapped, which invites no reply — so `--no-reply` and
`--reply-to` are refused there, having no envelope to shape.

## The repository layer

A repository may carry its own config, found by walking up from the directory a spawn is run in.
Either `.herdr-team/config.toml` or `.herdr-team.config.toml` — the directory when an
agent's `prompt_file` wants somewhere to live beside the config that names it, the flat file when
one file is the whole config. Both are tried at each directory on the way up, so the nearer one
wins whichever form it takes, and the directory wins a tie between the two in one place.

It merges over the user's: its `default` wins when it declares one,
and its agents replace the user's **by name and whole** — a repository agent that wants the user's
flags says `base = '<name>'`, which resolves across both files. Either layer alone is enough.
`--config` points the user layer somewhere else and the repository layer still merges over it.

Four things make one agent unusable, and each warns and drops that agent rather than failing the
command: a `base` no config declares, a cycle, no `kind` anywhere in the chain, and `model` or
`effort` under a kind this build cannot drive. Everything else in the config keeps working.

## Output

Human-readable text by default: results on stdout, diagnostics on stderr. `--json` emits tagged
NDJSON — one object per line, all on stdout, each carrying a `type` of `result`, `warning`, or
`error`. Everything shares one stream because a consumer cannot rely on two streams' relative
ordering once either is redirected.

Text is the default rather than JSON because the output is usually *read*, including by an agent. A
`spawn` prints `reviewer (claude) → w4:p9`; its JSON form nests herdr's whole agent record, which is
what a pipeline wants and roughly twenty-five times the size for the same actionable fact.

One run may print warnings before its result, so a pipeline selects rather than taking the first
line:

```
herdr-team --json spawn worker --placement tab \
  | jq -er 'select(.type == "result") | .agent.pane_id'
```

`prime` is the one exception: `--json` prints the same brief as no flag at all. Its result is a
document, and wrapping prose in an envelope buys escaping and no information — as do `--help` and
`--version`, which clap answers before a command is chosen. Failures are still JSON under `--json`
for every command, `prime` included, and so are the argument errors clap would otherwise print as
prose on stderr.

An argument error names only what this build declares — the argument, the values it accepts, the
rule it enforces — and never repeats the value that was rejected. The rejected value may be prompt
text, and a prompt does not belong in a diagnostic.

## Exit codes

| Code | Meaning |
| ---- | ------- |
| 0 | success |
| 1 | general failure |
| 2 | usage error — the arguments were wrong, whether this tool rejected them or herdr did |
| 3 | resource not found |
| 5 | conflict — a composer holding unsent text, an agent mid-task, a pane that is not yet an available shell, a name already taken |

The codes are stable, so a caller can branch on them without parsing stderr.

Code 5 is the one worth retrying, and only once the state its message names has changed. A pane
whose shell is still starting clears itself; an occupied composer, a working agent, and a taken
name do not.

## Known issues

A defect rather than a deferral: understood, reproduced, and not yet fixed.

- **A worktree spawn opens on its harness's trust prompt rather than a composer.** A fresh checkout
  is a directory the harness has never seen, so it asks whether the directory is trusted before it
  will accept input — while herdr reports the pane interactive and ready, because a live prompt is
  what it can see. A first prompt delivered into that state answers the dialog instead of being
  read, and only the re-send on a stalled submission puts the real prompt in the composer. Observed
  with a Claude Code agent that does not skip permission checks; a Codex agent opened straight to a
  composer. There is no fix inside this tool that does not amount to answering someone's security
  prompt for them, which is why it is recorded rather than worked around.

## Roadmap

Each of these is a decision to defer, not an oversight.

- **Witnessed delivery** — proving a message was received rather than that it was submitted.
- **Spawning somewhere other than here** — `spawn` infers where it is from the environment, so it
  can only split the pane it runs in, and only when it runs in one. A flag naming a pane would let a
  caller outside a session split anyway, and one inside anchor somewhere other than itself.
- **Ending an agent while keeping its pane** — `kill` closes the pane, so the seat goes with the
  agent. Reusing a warm seat would skip the slowest part of a launch, but every route to it means
  either reporting state to herdr on a harness's behalf or sending harness-specific keys.
