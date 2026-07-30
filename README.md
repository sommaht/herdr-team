# herdr-agent-tools

Launching a [herdr](https://herdr.dev) agent by hand is two steps: herdr starts an agent only in a
pane that already exists and is sitting at an interactive shell prompt, so you create a surface,
dig the new pane's id out of the JSON, and then start the agent in it. This does both in one
command, resolves the agent's kind and its usual flags from a named preset, and takes prompt text
on stdin.

## Commands

| Command | Purpose |
| ------- | ------- |
| `spawn` | Create a pane, tab, or workspace and start a preset-configured agent in it |
| `prompt` | Deliver a prompt to an agent that already exists |
| `kill` | Close an agent's pane, refusing one that is mid-task |
| `presets` | List what the preset config holds |
| `prime` | Print an agent-facing brief on driving this CLI |

`prime` is written for a session-start hook. `--hook <harness>` asks that harness to wrap the brief
in its host's envelope; each harness owns its own shape, so a host whose contract differs is one impl
rather than a flag change. It makes no herdr call and a config it cannot read costs the preset table
and nothing else, because a hook that fails is worse than one that says little.

```
herdr-agent-tools spawn reviewer --placement tab --preset opus --prompt "Review the branch"
git diff | herdr-agent-tools prompt reviewer -
herdr-agent-tools kill reviewer
herdr-agent-tools presets
```

`prompt` and `kill` each refuse one thing by default, and `--force` is the override for both: a
composer holding someone's unsent text, and an agent still working or blocked. Neither refusal
changes anything, and both exit 5.

## Presets

`$XDG_CONFIG_HOME/herdr-agent-tools/config.toml`, falling back to
`~/.config/herdr-agent-tools/config.toml`. Override the location with `--config` or with
`HERDR_AGENT_TOOLS_CONFIG`.

Presets are what the file holds today, and the filename deliberately does not say so: a name
that names one table has to change the first time a second one is added.

```toml
default = 'reviewer'

[presets.reviewer]
kind = 'claude'
args = ['--model', 'opus']

[presets.cheap]
kind = 'codex'
args = ['--model', 'gpt-5-low', '--no-alt-screen']
```

The preset name is the table key, so a duplicate name is inexpressible. `args` is an array only:
a string form would have to be split into shell words, and herdr takes the agent's arguments as an
argument vector, so nothing here needs shell quoting.

## Output

Human-readable text by default: results on stdout, diagnostics on stderr. `--json` emits tagged
NDJSON — one object per line, all on stdout, each carrying a `type` of `result`, `warning`, or
`error`. Everything shares one stream because a consumer cannot rely on two streams' relative
ordering once either is redirected.

Text is the default rather than JSON because the output is usually *read*, including by an agent. A
`spawn` prints `reviewer (claude) → w4:p9`; its JSON form nests herdr's whole agent record, which is
what a pipeline wants and roughly twenty-five times the size for the same actionable fact.

`prime` is the one exception: `--json` prints the same brief as no flag at all. Its result is a
document, and wrapping prose in an envelope buys escaping and no information. Failures are still JSON
under `--json` for every command, `prime` included.

## Exit codes

| Code | Meaning |
| ---- | ------- |
| 0 | success |
| 1 | general failure |
| 2 | usage error |
| 3 | resource not found |
| 5 | conflict, retryable — a composer holding unsent text, a pane that is not yet an available shell |

The codes are stable, so a caller can branch on them without parsing stderr.

## Roadmap

Each of these is a decision to defer, not an oversight.

- **Inter-agent messaging** — a message envelope carrying the sender's identity and a reply path.
- **Witnessed delivery** — proving a message was received rather than that it was submitted.
- **Placeholder discrimination in the composer guard** — telling a harness's dimmed placeholder
  hint apart from typed text, via styling attributes rather than plain text.
- **Spawning somewhere other than here** — `spawn` infers where it is from the environment, so it
  can only split the pane it runs in, and only when it runs in one. A flag naming a pane would let a
  caller outside a session split anyway, and one inside anchor somewhere other than itself.
- **Ending an agent while keeping its pane** — `kill` closes the pane, so the seat goes with the
  agent. Reusing a warm seat would skip the slowest part of a launch, but every route to it means
  either reporting state to herdr on a harness's behalf or sending harness-specific keys.
