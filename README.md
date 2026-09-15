# herdr-team

## What is this?

A companion CLI for [herdr](https://herdr.dev) that improves the experience of launching and
messaging agents inside it.

herdr starts an agent only in a pane that already exists and is sitting at a shell prompt, so
launching one by hand means creating a surface, digging the pane id out of the JSON, and starting
the agent in it. And once agents are talking to each other, a bare string in a composer says
nothing about who sent it or how to answer. `herdr-team` closes both gaps:

- **`spawn`** — from nothing to a running agent in one command.
  - Starts the agent in a new pane, tab, or workspace.
  - Starts a named agent configuration from a config file: kind, flags, model, effort, and a
    standing brief.
  - Creates a git worktree on a new branch and starts the agent inside it.
  - Takes a multiline first message through stdin.
- **`msg`** — messaging with explicit delivery evidence.
  - Wraps every message in an envelope naming who sent it.
  - Proves delivery from an observed state change or pane read, and warns when proof is
    unavailable; `--no-verify` skips the check.
  - Appends an automatic how-to-reply block when a reply is invited, so the recipient answers
    with a working command instead of guessing at one.

## Installation

```
git clone https://github.com/sommaht/herdr-team.git && cd herdr-team
cargo install --path .
```

### Session-start hook

`prime` prints a brief teaching an agent to drive this CLI, and `--hook <harness>` wraps it in
that harness's session-start envelope. For Claude Code, add it to `~/.claude/settings.json` to
cover every local project, or to `.claude/settings.json` to cover one repository:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "*",
        "hooks": [
          { "type": "command", "command": "herdr-team prime --hook claude", "timeout": 10 }
        ]
      }
    ]
  }
}
```

Every Claude Code session in that scope — including ones this tool spawns — then starts already
knowing the commands. The hook makes no herdr call and cannot fail: a config it cannot read
costs the agent table and nothing else. `--hook codex` is experimental: it emits the same
envelope, but Codex context injection has not been established here.

### Agent configuration

A config defines reusable agent configurations — a kind, its flags, a model and effort, a
standing brief — under names of your choosing. `<name>` names the running agent; `--agent
<name>` selects a configured definition, and a bare `spawn <name>` starts the config's
`default`. With the worked config, `herdr-team spawn reviewer --agent opus` starts an agent
named `reviewer` from the `opus` definition.

Copy [`examples/config.toml`](examples/config.toml) to
`$XDG_CONFIG_HOME/herdr-team/config.toml` (falling back to `~/.config/herdr-team/config.toml`)
and edit. The [Agents](#agents) section covers the format, inheritance, and the repository
layer. None of it is required: `spawn <name> --kind claude` works with no config at all.

## Examples

Launch a reviewer in its own tab and hand it the diff:

```
git diff | herdr-team spawn reviewer --placement tab --agent opus --msg -
```

Give an agent a worktree of its own, on a named branch — then hand it work once its trust
prompt is answered (see [Known issues](#known-issues)):

```
herdr-team spawn fixer --placement worktree --branch fix/flaky-test
herdr-team msg fixer "make the suite green"
```

Message an agent; the command returns after proving delivery, or warns that proof was
unavailable:

```
herdr-team msg reviewer "also check the error paths"
```

Dispatch work and block until it finishes. Name both terminal states — a harness settling to
`done` never reaches `idle`, and one alone times out on work that is done — and invite no reply:

```
herdr-team msg reviewer "run the tests" --no-reply --wait-until idle --wait-until done
```

Fan out across areas, each reporting back to the pane that spawned it:

```
for area in api web cli; do
  herdr-team spawn "$area" --placement tab \
    --msg "audit the $area surface" --reply-to "$HERDR_PANE_ID"
done
```

What the recipient of a `msg` actually sees:

```
<mail from="dispatcher" id="k7m2x9">
audit the CLI surface and list what is undocumented
</mail>
<how-to-reply>
herdr-team msg w4:p3 --no-reply - <<'EOF'
{{your reply}}
EOF
</how-to-reply>
```

## Commands

| Command | Purpose |
| ------- | ------- |
| `spawn` | Create a pane, tab, workspace, or worktree and start an agent in it |
| `msg` | Deliver a message to an agent that already exists |
| `kill` | Close an agent's pane, refusing one that is mid-task |
| `agents` | List what the config holds |
| `prime` | Print an agent-facing brief on driving this CLI |

`msg` and `kill` each refuse one thing by default, and `--force` is the override for both: a
composer holding someone's unsent text, and an agent still working or blocked. Neither refusal
changes anything, and both exit 5. Neither clears on a timer either — a composer is cleared by the
person typing into it — so the retry belongs after the named state changes, not on a loop.

`msg --wait-until` waits for the states you name, and against a target that is already working it
waits out the turn in progress rather than the queued message's own; delivery itself is proven by
reading the target's pane for the message, never by the wait having matched.

The command was called `prompt` until it started wrapping what it sends, and `msg` still answers to
that name. So does `spawn --prompt`, now `--msg`.

## Without a config

`spawn --kind <kind>` starts an agent kind directly and reads no config at all — not the user's, not
the repository's, not one `--config` names. Everything a config would have supplied is said on the
line instead:

```
herdr-team spawn scratch --kind codex -- --no-alt-screen
herdr-team spawn scratch --kind claude --model opus --effort high
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

`spawn --model <model>` and `spawn --effort <effort>` replace what the agent declares, for one
launch and without editing the config:

```
herdr-team spawn quick --agent reviewer --effort low
```

They are a replacement rather than an append, so the vector carries one `--model` and not two, and
they apply to `--kind` as well, which declares neither. Under a kind this build cannot drive either
flag is refused outright — unlike the config's own fields, which only drop the agent — because a
spawn that asked for a model must not quietly start on another.

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
The walk does not stop at a repository boundary — it continues to the filesystem root, so a config
in a directory above your checkout still applies.
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

- **A composer taller than its pane cannot be read, so the guard fails open on the draft it most
  ought to protect.** The guard finds a composer by looking at the bottom of a terminal snapshot,
  and herdr sizes that snapshot to the pane's own row count — a request for more rows finds more
  only in a pane that has them. Paste sixty lines into a Codex composer in a forty-row pane and
  every row of the snapshot is draft: no marker, no border, nothing to locate. The message is
  delivered with a warning, over the top of the paste. Reading further is not available: the
  rendering that carries a composer's styling is the viewport and stops at the pane's edge. Nobody
  minds losing four typed words, which is what makes this the wrong way round.

- **`--wait-until` against an agent that is already working waits out whatever turn is running, not
  the one your message starts.** herdr's check after a submission compares state-change sequence
  numbers rather than turns, so the turn that was already in progress satisfies it by ending. A
  message sent to a busy agent is queued behind an unknown amount of work, and nothing this CLI can
  ask distinguishes that work finishing from your own message being answered. Delivery is still
  proven — the pane is read for the message's own id — so `delivered` means what it says; it is the
  *wait* that cannot promise whose turn it waited for. Against a settled agent there is no
  ambiguity, because the submission establishes a turn that began after it.

- **A `--timeout` of 5000ms or less silently switches off the re-send repair.** A prompt submitted
  within a few seconds of an agent starting is sometimes swallowed, and the repair is to notice and
  send once more. Noticing depends on herdr reporting a stalled prompt rather than a plain timeout,
  and it only does so when the wait it was given is longer than its own five-second window. So the
  shortest leashes lose the repair that short-lived agents need most. Deliberate rather than
  overlooked — the alternative is spending more time than the caller allowed — and worth knowing
  before choosing a small number.

- **A message taller than herdr's read window cannot have its delivery proven.** Delivery is proven
  by reading the pane back for the message's own id, and herdr clamps any read at 1,000 lines. A
  message longer than that can scroll its own id out of reach, so it is reported unproven even when
  it landed. The report is honest — `--no-verify` is the escape for callers who already trust the
  submission — but a very long message and a proven delivery cannot currently be had together.

- **An error herdr reports is repeated in herdr's own words, so herdr's restraint is part of the
  privacy contract.** This tool's own diagnostics never carry a prompt, an agent's arguments, or
  captured terminal content — an argument error names the argument and the rule, never the value
  that broke it. herdr's errors pass through verbatim: a refusal's message, or the first line of
  stderr when there is none. The argv behind them can hold protected text — the argument to
  `agent prompt` is the prompt — so a herdr rejection that ever quoted a value back would land
  that quote in this tool's output. Rewording herdr's diagnostics on a guess about their shape
  would trade accuracy for a promise this tool cannot keep; carrying them verbatim keeps them
  true and leaves what they contain to herdr.

## Roadmap

Each of these is a decision to defer, not an oversight.

- **Choosing a different pane to split** — `spawn` infers where it is from the environment, so it
  can only split the pane it runs in, and only when it runs in one. A flag naming a pane would let a
  caller outside a session split anyway, and one inside anchor somewhere other than itself.
- **Ending an agent while keeping its pane** — `kill` closes the pane, so the seat goes with the
  agent. Reusing a warm seat would skip the slowest part of a launch, but every route to it means
  either reporting state to herdr on a harness's behalf or sending harness-specific keys.
