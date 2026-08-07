# Composer fixtures

Terminal snapshots the composer guard reads, captured from live agents rather than written by
hand. Each scenario is two files, because the guard reads two renderings and they are not the
same text:

| File | herdr source | What it is |
| --- | --- | --- |
| `<name>.txt` | `agent read --source detection --format text` | escapes stripped, soft-wrapped rows rejoined, sized to the pane's own row count |
| `<name>.ansi.txt` | `agent read --source visible --format ansi` | escapes kept, wrapped as drawn, and never taller than the pane |

Neither reaches into scrollback beyond the pane. herdr builds the plain one from the pane's row
count and only then applies `--lines`, so asking for more rows finds more only in a pane that has
them, and a composer taller than its pane cannot be read at all. Where a scenario's own line count
matters, the row below says which pane height and which `--lines` it was taken at — without both
numbers a later reader cannot tell whether the file still demonstrates anything.

The plain file answers "is there anything in the composer". The styled one answers the follow-up
the plain one cannot: whether that content is a draft or something the harness drew for itself,
which is a distinction only the faint attribute carries.

Every one has been scrubbed of anything naming a machine, a repository, or a person. The
structure — line count, marker positions, escape sequences — is otherwise as captured.

**The footer beneath a composer is not a fixed shape.** Claude Code's status line is
user-configurable, and the machine these were captured on runs a custom one: three lines of
powerline segments where a stock install draws a single line. Codex rewrites its own footer
depending on what the composer holds. So the `claude-stock-footer-*` pair is kept alongside the
captures, and nothing that reads a composer may key on what sits below it.

## Codex

| Fixture | What it is there to prove |
| --- | --- |
| `codex-empty` | The composer holds only Codex's own faint placeholder, and horizontal rules sit in the transcript above it. The rules are the trap: they are not the composer's borders and must not be read as them. |
| `codex-draft` | One typed line. |
| `codex-multiline` | A three-line draft: the marker leads and the continuations are indented, with no blank line inside it. |
| `codex-pasted` | Sixty pasted lines, read at forty in a sixty-five-row pane. No marker and no rule survive, so the composer cannot be located at all and the guard fails open. The honest "cannot see it" case. |
| `codex-pasted-tall` | The same sixty lines and the same pane, read at eighty. The plain half is sixty-five lines and holds the marker; its last forty are byte-identical to `codex-pasted.txt`, which is what makes the pair prove that the line count did the work. The styled half stays at forty, because `visible` is the viewport and asking for more would only misreport how much was looked at. |
| `codex-working` | Mid-turn, composer untouched. |
| `codex-working-queued` | Mid-turn with messages queued: a banner above the composer, which is itself still empty. |
| `codex-working-queued-draft` | Queued *and* drafted at once, which is also where the footer changes shape. |
| `codex-slash-menu` | The command menu, which Codex draws *below* the composer and which replaces the footer. |
| `codex-at-menu` | The file picker, likewise below. |
| `codex-bordered-*` | An older Codex that drew both borders. Kept so the build keeps working against whichever is installed. |

## Claude Code

| Fixture | What it is there to prove |
| --- | --- |
| `claude-empty` | Both borders drawn, composer empty. |
| `claude-occupied` | One typed line. |
| `claude-multiline` | A three-line draft. |
| `claude-pasted` | Sixty pasted lines. Claude collapses most of them into `[Pasted text #1 +13 lines]` chips, so both borders and the live marker still fit inside forty lines — the contrast with `codex-pasted`, which does not. |
| `claude-working` | Mid-turn, composer untouched. |
| `claude-working-draft` | Mid-turn with a draft. |
| `claude-working-queued` | A queued message, where the harness writes `Press up to edit queued messages` into the composer itself — faint, so it is a suggestion rather than a draft. |
| `claude-transcript` | Settled after several turns, with the marker echoed into the transcript above the live composer. |
| `claude-slash-menu` | The command menu, which Claude Code draws *above* the composer rather than below. |
| `claude-at-menu` | The file picker, likewise above. |
| `claude-suggestion` | A faint suggested action in an otherwise untouched composer. |
| `claude-highlighted-draft` | A coloured draft, which is emphasis rather than faintness and so is still a draft. |
| `claude-stock-footer-*` | The same empty and drafted composers under a stock one-line status line, and with the repository name in the top border. Hand-written rather than captured, and the reason is the point: a footer is whatever its operator configured. |

## Neither

| Fixture | What it is there to prove |
| --- | --- |
| `no-rules` | A pane hosting no agent at all. |
| `unknown-marker` | A composer drawn with a marker this build does not know, which fails open rather than guessing. |
