# Command reference

[Back to README](../README.md#commands)

Use `herdr-team <command> --help` for the full flag list.

## Spawning into a worktree

`--placement` selects a new pane (the default), tab, workspace, or Git worktree:

```sh
herdr-team spawn fixer --kind claude --placement worktree \
  --branch fix/flaky-test --msg "Fix the flaky test"
```

This creates the worktree, starts the agent, and attempts to send the task. A fresh worktree
can trigger the [trust-prompt issue](../README.md#trust-prompts-can-intercept-the-first-task).

## Messages and replies

`msg` accepts a unique agent name or a herdr pane id. Use `-` to read the message from stdin:

```sh
git diff | herdr-team msg reviewer -
```

Messages include the sender, a message id, and reply instructions:

```text
<mail from="dispatcher" id="k7m2x9">
Review the CLI changes
</mail>
<how-to-reply>
herdr-team msg w4:p3 --no-reply - <<'EOF'
{{your reply}}
EOF
</how-to-reply>
```

`--reply-to <target>` sends replies to a different agent or pane.
`--no-reply` omits the reply instructions. These flags also apply to `spawn --msg`.

The old `prompt` command and `spawn --prompt` flag still work as aliases for `msg`
and `spawn --msg`.

## Delivery checks and waiting

By default, messaging checks delivery through an observed transition to `working` or by
reading the pane for the message's id. A stalled submission is retried once when the timeout
allows it.

A successful command can still warn that delivery was not confirmed. In JSON output, check
`delivered` rather than treating exit code 0 as proof of receipt.
`--no-verify` submits without waiting for delivery confirmation or completion.

To wait for a turn to finish, include both terminal states:

```sh
herdr-team msg reviewer "Run the tests" --no-reply \
  --wait-until idle --wait-until done --timeout 120000
```

`--timeout` is in milliseconds and covers submission, any retry, delivery checks, and waiting.
The default is 15,000 ms. Different agent CLIs finish in different states, so waiting for only
`idle` or only `done` can time out after the work has finished.

When you specify `--wait-until`, delivery is checked through the pane separately from the
state wait. If the agent was already working, the wait can finish at the end of its existing
turn, before it handles your message. See [Known issues](../README.md#known-issues).

## Conflicts and force

`msg` refuses when it detects unsent text in the target's input box.
`kill` refuses when the agent is working or blocked. Both exit with code 5 and leave the
target unchanged.

`--force` overrides those checks. For `msg`, it does not skip delivery verification.

Retry a conflict only after the condition in the error changes. A shell that is still starting
can become ready on its own; an unsent draft requires someone to send or clear it.
The unsent-text check has a [known limitation with large drafts](../README.md#large-drafts-can-bypass-the-unsent-text-check).

## Output

By default, results go to stdout and diagnostics to stderr, one line each. For example:

```text
reviewer (claude) → w4:p9
```

`--json` emits one JSON object per line, all on stdout. Each has a `type` of `result`,
`warning`, or `error`. Agent results include herdr's agent record.

Warnings can appear before the result, so select the result by type:

```sh
herdr-team --json spawn worker --kind claude --placement tab \
  | jq -er 'select(.type == "result") | .agent.pane_id'
```

`prime`, `--help`, and `--version` still print text under `--json`.
Failures, including argument errors and failures from `prime`, use JSON when requested.

herdr-team's argument errors name the rejected option or rule without repeating the supplied
value. Errors from herdr itself pass through verbatim; see the
[privacy limitation](../README.md#errors-from-herdr-may-include-sensitive-text).

## Exit codes

| Code | Meaning |
| ---- | ------- |
| 0 | Success; delivery may still be unconfirmed |
| 1 | General failure |
| 2 | Usage error |
| 3 | Resource not found |
| 5 | Conflict: unsent text, a busy agent, an unavailable shell, or a name already in use |

These codes are stable, so scripts can branch on them without parsing error messages.
