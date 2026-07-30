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
| `presets` | List what the preset config holds |

```
herdr-agent-tools spawn reviewer --tab --preset opus --prompt "Review the branch"
git diff | herdr-agent-tools prompt reviewer -
herdr-agent-tools presets
```

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
- **A conventions block** — so an agent driving this CLI picks the conventions up without being
  told them in every prompt.
