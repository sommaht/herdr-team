# herdr-team

A small companion CLI for [herdr](https://herdr.dev) that makes it easier to launch
coding agents and send them work. Plain herdr starts agents in existing panes at a shell prompt,
so you first have to create a pane and find its id. herdr-team handles that setup and adds:

- **Agent presets across harnesses.** Save a harness, flags, model, effort, and instructions
  under a name, then launch it with `--agent`.
- **One-command spawning.** Create a pane, tab, workspace, or Git worktree and start an agent
  in it. Add `--msg` to send its first task.
- **Delivery checks.** Check that a message reached the agent, retry a stalled submission
  once, and warn when delivery cannot be confirmed.
- **Message envelopes.** Include who sent a message and a ready-to-run command for replying.

It handles launching and messaging. You choose how to divide the work and coordinate the agents.

## Install

You need [herdr](https://herdr.dev), Rust, and the agent CLI you want to run installed.

```sh
git clone https://github.com/sommaht/herdr-team.git
cd herdr-team
cargo install --path .
```

## Quickstart

Run this from a terminal inside herdr. It starts Claude Code in a new tab and sends it a task,
without a config file:

```sh
herdr-team spawn reviewer --kind claude --placement tab \
  --msg "Review the changes in this repository"
```

Once it finishes, close its pane:

```sh
herdr-team kill reviewer
```

If the agent opens on a trust prompt, the task submission can answer that dialog before you
review it, instead of reaching the agent as a task.
See [Known issues](#known-issues) for this and other delivery limitations.

## Common tasks

### Message an agent

Send work to an agent by its name or pane id:

```sh
herdr-team msg reviewer "Run the tests and report any failures"
```

### Spawn an agent with a preset

For this and the next example, first save the `opus` preset from
[Configuration](#configuration). Use `--agent` to select it:

```sh
herdr-team spawn coder --agent opus --placement worktree \
  --msg "Review this repository's test coverage"
```

`coder` is the running agent's name; `opus` supplies its harness, model, and other settings.
`--placement worktree` creates a Git worktree on a new branch and starts the agent inside it.

| Placement | Where the agent starts |
| --------- | ---------------------- |
| `pane` | Splits the calling pane. This is the default and requires running inside a herdr pane. |
| `tab` | Opens a new tab. |
| `workspace` | Opens a new workspace. |
| `worktree` | Creates a Git worktree on a new branch. |

The command also attempts to send the task. A new worktree can trigger the
[trust-prompt issue](#trust-prompts-can-intercept-the-first-task).

### Override a preset option

```sh
herdr-team spawn quick-review --agent opus --effort low \
  --msg "Check the README for broken examples"
```

`--effort low` replaces the preset's effort for this launch. `--model` works the same way;
the saved preset stays unchanged.

## Messages and replies

Messages identify their sender. When a reply address is available, they also include a command
for replying:

```text
<mail from="reviewer" id="k7m2x9">
Found two failing tests.
</mail>
<how-to-reply>
herdr-team msg w4:p3 --no-reply - <<'EOF'
{{your reply}}
EOF
</how-to-reply>
```

The recipient replaces `{{your reply}}` with its response and runs the supplied command.
Use `--no-reply` to omit the reply instructions, or `--reply-to <target>` to direct the reply
elsewhere. Both flags work with `msg` and `spawn --msg`.

See the [command reference](docs/commands.md#messages-and-replies) for more on messaging.

## Configuration

Save presets in `$XDG_CONFIG_HOME/herdr-team/config.toml`, or
`~/.config/herdr-team/config.toml` when `XDG_CONFIG_HOME` is unset:

```toml
default = 'opus'

[agents.opus]
kind = 'claude'
model = 'opus'
effort = 'high'

[agents.codex]
kind = 'codex'
# Keep messages in scrollback for delivery checks.
args = ['--no-alt-screen']
```

Choose a preset with `--agent`:

```sh
herdr-team spawn reviewer --agent opus
herdr-team spawn fixer --agent codex
```

`reviewer` and `fixer` name the running agents. `opus` and `codex` select their presets.
Omit `--agent` to use the configured `default`, or use `--kind` to skip configuration entirely.
Keep `--no-alt-screen` for Codex; the [configuration reference](docs/configuration.md#codex-terminal-mode)
explains its effect on delivery checks.

You can also share project settings in `.herdr-team/config.toml`.
See the [configuration reference](docs/configuration.md) for discovery rules, inheritance,
instruction files, and overrides. [examples/config.toml](examples/config.toml) has a fuller example.

### Environment variables

herdr-team reads these variables:

| Variable | Purpose |
| -------- | ------- |
| `HERDR_TEAM_CONFIG` | Path to the user config file. `--config` takes precedence. |
| `XDG_CONFIG_HOME` | Config directory; looks for `herdr-team/config.toml` inside it. |
| `HOME` | Falls back to `$HOME/.config/herdr-team/config.toml` when `XDG_CONFIG_HOME` is unset. |
| `HERDR_PANE_ID` | Calling pane, normally set by herdr. Used for pane splits, locating the workspace for new tabs, and message sender/reply information. |

Config lookup order: `--config`, `HERDR_TEAM_CONFIG`, `XDG_CONFIG_HOME`, then `HOME`.
Project settings still apply; `--kind` skips all config files.

Calls to herdr also inherit its environment, including `HERDR_SESSION` and `HERDR_SOCKET_PATH`
for session routing. herdr-team does not read those two variables itself.

## Teach agents to use herdr-team

This setup is optional. `herdr-team prime` prints instructions an agent can use to launch
and message other agents.

For Claude Code, merge this hook into `~/.claude/settings.json` for all projects, or
`.claude/settings.json` for one project:

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

New Claude Code sessions, including agents you spawn, receive the instructions at startup.
The hook does not contact herdr. If it cannot read the config, it still prints the instructions
without the agent list.

`--hook codex` is experimental: it emits the same envelope, but context injection into Codex
has not been verified here.

## Commands

| Command | Purpose |
| ------- | ------- |
| `spawn` | Create a pane, tab, workspace, or worktree and start an agent |
| `msg` | Send a message to an existing agent |
| `kill` | Close an agent's pane; refuse if it is working or blocked |
| `agents` | List configured agent presets |
| `prime` | Print usage instructions for agents |

`msg` checks for unsent text in the agent's input box before sending.
`kill` checks whether the agent is working or blocked before closing its pane.
Both refuse with exit code 5 when those checks find a conflict; `--force` overrides the check.
See the [large-draft limitation](#large-drafts-can-bypass-the-unsent-text-check).

Exit code 0 does not guarantee delivery. Check warnings, or the `delivered` field under `--json`,
to see whether receipt was confirmed.

To wait for completion, use `msg --wait-until idle --wait-until done`; agent CLIs differ in which
state they report when finished. For a busy agent, this can wait for its existing turn instead
of your message's turn. See [waiting examples and timeout options](docs/commands.md#delivery-checks-and-waiting).

The [documentation index](docs/README.md) links the configuration and command references.
Use `herdr-team <command> --help` for all flags.

## Known issues

### Trust prompts can intercept the first task

`spawn --msg` attempts to send the task once herdr reports the agent ready. In a new worktree,
Claude Code may still be showing a trust prompt at that point. The submission can answer the
dialog before you review it, instead of delivering the task. In the observed case, the
stalled-submission retry then put the task in the input box.

This was observed with Claude Code using permission checks; Codex opened directly to its input
box. To avoid the issue, spawn without `--msg`, answer the trust prompt, then send the task with
`msg`.

### Large drafts can bypass the unsent-text check

If an unsent draft is taller than its pane, the terminal snapshot may contain only draft text,
with none of the markers needed to identify the input box. The check then fails to detect the
draft: herdr-team warns and sends anyway, potentially overwriting it. Requesting more snapshot
rows does not recover the missing input-box structure.

### Waiting on a busy agent can finish too early

`--wait-until` can finish when the agent's existing turn ends, before it handles your queued
message. herdr tracks state changes rather than individual turns, so this tool cannot distinguish
those completions. It checks delivery separately by looking for the message's id in the pane.
Sending to an idle agent avoids this ambiguity.

### Short timeouts disable the delivery retry

At `msg --timeout 5000` or less, herdr reports a plain timeout instead of the stalled-submission
signal that triggers a retry. A task sent just after startup can therefore be lost without the
usual second attempt.

### Very long messages can be delivered without proof

When delivery requires reading the pane, herdr limits the read to 1,000 lines. A long message
can scroll its own id out of that window, so herdr-team reports delivery as unconfirmed even
when it arrived. `--no-verify` skips confirmation; it does not improve delivery.

### Errors from herdr may include sensitive text

herdr-team's own diagnostics omit prompts, agent arguments, and captured terminal content.
Errors returned by herdr pass through verbatim. If herdr includes sensitive arguments in an
error, they will appear in herdr-team's output too.

## Roadmap

- Choose which pane to split. Currently, `spawn` can only split the pane it runs in.
- Stop an agent while keeping its pane open. Currently, `kill` closes the pane too.
