# Configuration

[Back to README](../README.md#configuration)

## Config files

The user config is `$XDG_CONFIG_HOME/herdr-team/config.toml`, or
`~/.config/herdr-team/config.toml` when `XDG_CONFIG_HOME` is unset.
Override its location with `--config` or `HERDR_TEAM_CONFIG`.
The lookup order is `--config`, `HERDR_TEAM_CONFIG`, `XDG_CONFIG_HOME`, then `HOME`.
See [Environment variables](../README.md#environment-variables) for the full list read by herdr-team.

For project settings, use either `.herdr-team/config.toml` or `.herdr-team.config.toml`.
Discovery walks upward from the directory where you run `spawn`, all the way to the filesystem
root. It does not stop at the Git repository boundary. The nearest config wins;
`.herdr-team/config.toml` wins if both forms exist in the same directory.

Discovered settings are trusted without confirmation and can change agent flags and instructions.
Review them before spawning from an untrusted checkout; see the
[repository-config trust warning](../README.md#repository-configuration-is-trusted-automatically).

Project settings merge over user settings:

- A project `default` replaces the user default.
- A project agent replaces the entire user agent with the same name.
- `base` can refer to agents from either file, after merging.

Either file can be used on its own. `--config` changes the user config path; project settings
still apply.

## Agent presets

```toml
default = 'reviewer'

[agents.claude]
kind = 'claude'
args = ['--disallowed-tools', 'AskUserQuestion']

[agents.reviewer]
base = 'claude'
model = 'opus'
effort = 'high'
```

`herdr-team spawn audit --agent reviewer` starts an agent named `audit` using the
`reviewer` preset. Without `--agent`, it uses `default`.

`kind` selects the agent CLI and is passed to herdr. Use an array for `args`, with each
argument as its own entry. In this example, the shared Claude flags disable interactive
questions; omit them if you want agents to ask you questions.

`base` inherits another preset. Fields you specify replace inherited values, except `args`,
which appends to the inherited arguments. You cannot remove inherited arguments; define
a separate preset when you need a different set.

An unknown base, an inheritance cycle, a missing kind, or unsupported model/effort settings
make that preset unavailable. herdr-team warns and continues loading the other presets.

### Codex terminal mode

Keep `args = ['--no-alt-screen']` in Codex presets so messages remain in terminal scrollback.
In alternate-screen mode, a long message can scroll its id out of the history herdr-team can
read, preventing confirmation through the pane. This affects messages sent to a busy agent
and messages sent with `--wait-until`; delivery can also be confirmed through an observed
transition to `working`, which does not require reading the pane.

## Models, effort, and extra arguments

`model` and `effort` use the same configuration fields for Claude Code and Codex.
herdr-team translates them to each CLI's flags. Other herdr-supported kinds can use `kind`
and `args`, but this tool currently supports `model` and `effort` only for Claude Code and Codex.

Override either setting for one launch:

```sh
herdr-team spawn quick --agent reviewer --effort low
```

`--model` and `--effort` replace the configured fields. Extra arguments after `--` append
to the configured arguments and are passed through unchanged:

```sh
herdr-team spawn reviewer --agent reviewer -- --resume
```

The agent CLI decides how to handle conflicting flags.

## Instruction files

Use `prompt_file` to give an agent standing instructions:

```toml
[agents.reviewer]
kind = 'claude'
prompt_file = 'review.md'
```

The path is relative to the config file that declared it. A missing or blank file causes
`spawn` to refuse before creating anything.

```sh
herdr-team spawn audit --agent reviewer --msg "Start with auth"
```

This sends the file contents followed by a blank line and the message in one submission.
The instructions appear before the message's mail envelope.

Without `--msg`, the instructions are sent on their own, without an envelope or reply
instructions. `--no-reply` and `--reply-to` require a message and are refused in that case.

## Run without a config

`--kind` skips all config files:

```sh
herdr-team spawn scratch --kind codex -- --no-alt-screen
```

Or, for Claude Code:

```sh
herdr-team spawn scratch --kind claude --model opus --effort high
```

Use a different running-agent name if you want both at once.

`--kind` cannot be combined with `--agent` or `--config`. For kinds other than Claude Code
and Codex, pass model or effort flags through `--`; herdr-team refuses its own `--model` and
`--effort` options for those kinds.

See [examples/config.toml](../examples/config.toml) for more presets.
