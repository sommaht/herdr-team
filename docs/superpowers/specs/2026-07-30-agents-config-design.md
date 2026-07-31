# Agents, layered config, and per-agent briefs — design

**Date:** 2026-07-30 · **Status:** proposed

## What this is

Four changes to the config file, which together turn it from a table of launch flags into a
description of the agents this tool can start.

1. **`presets` becomes `agents`**, in the file, on the command line, and in the brief.
2. **`model` and `effort` become first-class fields**, and each harness says how its CLI expresses
   them.
3. **`base` lets one agent inherit another**, so a model variant is three lines rather than a copy.
4. **A repository may carry its own config**, merged over the user's, and an agent may name a
   **`prompt_file`** that is prepended to its first prompt.

## Why the rename

`preset` names the file's shape rather than what it holds. What the file holds is a set of agents:
which one to start, how to configure it, and — with `prompt_file` — what it should know on arrival.
An agent asked to spawn `reviewer` reaches for `--agent reviewer` before it reaches for
`--preset reviewer`, and the listing command reads as *list my agents* rather than *list my
presets*.

The rename is a clean break. `[presets]` in a config file and `--preset` on the command line both
fail, each naming its replacement. There is one file to edit; two spellings would be permanent.

## The file

```toml
default = 'sol'

[agents.cc]
kind = 'claude'
args = ['--disallowed-tools', 'AskUserQuestion']

[agents.opus]
base = 'cc'
model = 'opus'
effort = 'xhigh'

[agents.reviewer]
base = 'opus'
prompt_file = 'review.md'
```

| Field | Required | Meaning |
| --- | --- | --- |
| `kind` | after resolution | herdr's agent kind, passed to `agent start --kind` untouched |
| `base` | no | another agent to inherit from, by name |
| `model` | no | the model, expressed as flags by the resolved harness |
| `effort` | no | the reasoning effort, likewise |
| `args` | no | flags appended after `--`, as an argument vector |
| `prompt_file` | no | a file prepended to this agent's first prompt |

`kind` is required of a *resolved* agent, not of a declared one — an agent with a `base` inherits it.
That makes two types rather than one: a declared agent whose every field is optional, and a resolved
agent with a `String` kind, a concatenated argument vector, and an absolute `prompt_file`.

`args` remains an array only, for the reason it always was: a string form would have to be split into
shell words for a value handed straight to `Command`.

## Layers

Two files, merged lowest-first:

| Layer | Where it is looked for |
| --- | --- |
| user | `--config PATH`, then `$HERDR_AGENT_TOOLS_CONFIG`, then `$XDG_CONFIG_HOME/herdr-agent-tools/config.toml`, then `~/.config/herdr-agent-tools/config.toml` |
| repository | the nearest `.herdr-agent-tools/config.toml`, walking up from `--cwd` — or the process directory when there is none — to the filesystem root |

The walk means a spawn from `src/api` finds the config at the project root, which is where a spawn
is actually run from. It stops at the filesystem root rather than at a repository boundary, because
locating that boundary is a herdr call this tool makes only for worktree spawns, and paying for it
on every load to refuse a config the caller placed deliberately buys nothing.

**Merging.** The repository's `default` overrides the user's when present. The repository's agents
replace the user's **by name and whole** — no field-level blending. A repository agent that wants the
user's flags says `base = 'opus'`, which is the inheritance mechanism this design already has. One
rule instead of two.

**Either layer alone is enough.** Today a missing user config is a failure; with a repository layer
it stops being one, and a repository config alone can drive the tool. What must still hold is that
some layer declares `default`, and that an explicit `--config` names a file that exists — a caller
who named a path is asking about that path.

`--config` keeps its current meaning: read *this* instead of the one in the config directory. It
replaces the user layer, and the repository layer still merges over it.

## Inheritance

`base` is resolved against the **merged** map, so a repository agent may inherit from a user agent,
and a chain may be any depth.

- `kind`, `model`, `effort`, `prompt_file` — the child's value wins; absent, the base's is inherited.
- `args` — concatenated, **root first**: the base's vector, then the child's.

Appending rather than overriding matches the chain already in place. `spawn -- <extra>` appends after
the config's arguments and agent CLIs are last-flag-wins, so a child adding one flag does not have to
restate its base's whole vector.

**Four ways an agent can be unusable, all of which warn and drop it:**

- `base` names an agent no layer declares.
- A cycle — `a` bases on `b` bases on `a`, or an agent bases on itself. Every agent in the cycle is
  dropped.
- No `kind` survives resolution.
- `model` or `effort` under a kind this build has no harness for, which is the next section.

Dropped, not fatal. The agent disappears from the listing with a warning naming it and the reason;
spawning it is then `no agent named x`, which lists the ones that survived. A config with one broken
entry keeps working for every other entry, and the listing — the command a caller debugs with — still
answers.

## Model and effort belong to the harness

`model = 'opus'` is not a flag. Claude Code spells it `--model opus --effort xhigh`; Codex spells the
same pair `--model gpt-5.6-sol -c model_reasoning_effort=xhigh`. Which flags a model becomes is
knowledge about a specific CLI, and this crate already has one place for that:

```rust
/// The flags this harness's CLI expresses `model` and `effort` as.
fn tuning(&self, model: Option<&str>, effort: Option<&str>) -> Vec<String>;
```

Required rather than defaulted, for the reason `hook` is: a harness added later must not inherit a
spelling nobody checked against its CLI.

**Argument order** handed to `agent start`:

```
resolved args   →   tuning flags   →   spawn -- <extra>
```

Last-flag-wins settles every conflict, and the order says which layer is more authoritative: a
first-class field beats an `args` entry that sets the same thing, and what the caller typed beats
both.

**An unknown kind.** herdr recognizes around twenty kinds; this build has harnesses for two. A kind
outside those two has no `tuning` to call, so an agent that sets `model` or `effort` under one is
dropped with a warning naming the kinds that can express them. It is the same fail-open the base
failures get, for the same reason — one unusable entry must not take the CLI with it.

An agent of an unknown kind that sets *neither* is untouched. `kind` is passed through verbatim and
always was; this only refuses to invent a spelling.

## Per-agent briefs

`prompt_file` names a file that is prepended to the agent's first prompt:

```
spawn reviewer --prompt "start with the auth module"

delivers:
    <contents of review.md>

    start with the auth module
```

With no `--prompt`, the file is delivered alone. It goes inside the normal mail envelope, as the body
— an agent receiving one sees the brief and the instruction as one message from one sender, which is
what it is.

**Resolution.** Relative to the directory of the config file that *declared* the field. A repository
agent's `prompt_file = 'review.md'` is `<repo>/.herdr-agent-tools/review.md`; a user agent's is
beside the user config. Inheritance carries the already-resolved path, so an agent in a repository
config inheriting from a user agent gets the user's file rather than a path reinterpreted against its
own directory.

**Spawn only.** A later `prompt <target>` cannot apply it: herdr reports a pane's *kind*, not which
configured agent spawned it, and this tool keeps no state that would bridge the two. Making it work
everywhere means recording agent identity somewhere, which is a larger change than this earns.

**A file that is missing, unreadable, or blank is a refusal** — exit 3, raised among the other
read-only pre-checks, before any surface is created. An agent that silently starts without the brief
it was configured with is the failure this whole tool is shaped to avoid, and at pre-check time a
refusal costs nothing.

## The command surface

| Before | After |
| --- | --- |
| `herdr-agent-tools presets` | `herdr-agent-tools agents` |
| `spawn --preset <name>` | `spawn --agent <name>` |
| `[presets.x]` | `[agents.x]` |

Both old spellings fail naming their replacement. `[presets]` is detected outright rather than left
to serde's unknown-key message, so the error is `<path> uses [presets], which is now [agents]; rename
the table`. `--preset` stays declared as a hidden argument whose only job is that message — clap's
suggestion machinery will not reach `--agent` from `preset`, and a bare *unrecognized argument* is a
poor way to learn about a rename.

**The listing** renders the command line that will actually run, which is what the previous listing
did and what makes it worth reading:

```
cc (claude) --disallowed-tools AskUserQuestion
opus (claude) --disallowed-tools AskUserQuestion --model opus --effort xhigh
reviewer (claude) --disallowed-tools AskUserQuestion --model opus --effort xhigh  [prompt: review.md]
sol (codex) --no-alt-screen --model gpt-5.6-sol -c model_reasoning_effort=xhigh  [default]
```

The `--json` form carries the fields structured — `name`, `kind`, `model`, `effort`, `args`,
`prompt_file`, `source`, `default` — where `args` is the exact vector `agent start` receives, tuning
flags included, so the two forms cannot disagree. `source` is the config file the agent came from,
which is the question two layers create.

Rendering arguments here remains correct under the no-logging rule: that rule governs error messages,
diagnostics, and logs, and a listing of the config file is the one place these are the answer.

## Where it lands

| File | Change | Responsibility |
| --- | --- | --- |
| `src/config.rs` | Rewrite | Schema types, `load`, errors — the module's public face |
| `src/config/discover.rs` | New | Both layers' path search, pure |
| `src/config/resolve.rs` | New | Merge and base chains, pure, returns agents and warnings |
| `src/harness.rs` | Modify | `tuning` on the trait; `kinds()` already exists for the warning |
| `src/harness/claude.rs` | Modify | `--model` / `--effort` |
| `src/harness/codex.rs` | Modify | `--model` / `-c model_reasoning_effort=` |
| `src/cmd/presets.rs` → `src/cmd/agents.rs` | Rename | The listing, with the new fields |
| `src/cmd/spawn.rs` | Modify | `--agent`, hidden `--preset`, tuning flags, the prompt-file pre-check and prepend |
| `src/cmd/prime.rs` | Modify | `## Agents`, `--agent`, the regenerated table |
| `src/main.rs` | Modify | `Agents` subcommand, the long-about sentence |
| `README.md`, `CLAUDE.md` | Modify | The rename reaches rule 3's wording |

`config.rs` is 349 lines and roughly doubles. The split is by seam rather than by line count:
discovery is a pure question about paths, resolution is a pure transform from declared tables to
resolved agents, and what remains in `config.rs` is the schema, the disk read, and the errors. The
warnings stay data until `load` drains them into the sink, which keeps resolution testable without
one.

## Testing

Every case is exercised without a herdr process, per the rule that automated tests never invoke one.

- **Discovery**, as pure functions over an environment passed in: the user layer's four-step order,
  unchanged; the repository walk finding the nearest ancestor, stopping at the root, honouring `--cwd`
  over the process directory.
- **Layering**: the repository replacing an agent whole rather than by field, its `default` winning, a
  repository config with no user config beside it, and an explicit `--config` that does not exist
  still failing.
- **Base**: one hop, a chain of three, argument concatenation order, each field's child-wins rule, a
  dangling base warning and dropping, a cycle dropping every member, a self-base, and an agent left
  with no kind.
- **Tuning**: each harness's spelling by exact vector; model without effort and effort without model;
  an unknown kind setting one dropped with a warning that names the kinds that can express it; an
  unknown kind setting neither passing through untouched.
- **`prompt_file`**: resolved against the declaring file's directory, and an inherited path staying
  with its declarer rather than being reinterpreted.
- **The rename**: `[presets]` producing the message that names `[agents]`, and `--preset` likewise.
- **The listing**: the human line and the JSON shape by exact string, as the current tests do.
- **Spawn**: the prompt file prepended with a blank line between it and the caller's text, the file
  delivered alone when there is no `--prompt`, and a missing file refused before anything is created.
- **The brief**: `prime` naming agents rather than presets, and its table matching the listing.

What no test here covers is a real launch — that `--effort xhigh` is a flag Claude Code accepts, and
that `-c model_reasoning_effort=` reaches Codex intact. That is a rehearsal against a live session,
and it is the thing worth running by hand before this is called done.

## Decisions recorded

- **A clean break over dual spellings.** One config file to edit, and a deprecation path that never
  ends is worse than a message that names the fix.
- **`args` appends through a base chain; every other field overrides.** The exception is justified by
  last-flag-wins: appending loses nothing a child wants to change, and overriding would make a child
  restate its base's vector to add one flag.
- **A repository agent replaces the user's whole.** `base` already expresses partial inheritance, and
  a second, implicit mechanism that blends fields across files would make the effective config hard
  to predict from either file alone.
- **The walk stops at the filesystem root.** Stopping at the repository root costs a herdr call on
  every load to refuse a file the caller placed on purpose.
- **Model and effort live on the harness trait.** They are claims about a specific CLI, which is what
  that trait is for; putting the mapping in `config.rs` would put CLI knowledge in the module that
  reads files.
- **Every config failure short of an unreadable file warns and drops one agent.** A tool whose
  listing refuses to run because of an entry unrelated to what was asked for cannot be used to fix
  itself.
- **A missing `prompt_file` refuses.** The opposite of the rule above, and deliberately: dropping the
  agent hides that its brief was the missing piece, and unlike a config-shape problem this is knowable
  only at spawn, where a refusal has cost nothing yet.

## Open for later

- **`prompt_file` on every prompt, not just the first.** Wants agent identity recorded somewhere
  herdr or this tool can read back from a pane id.
- **A third harness.** `tuning` is the second required method on the trait; a kind list that keeps
  growing while this build knows two is the pressure that eventually makes the mapping data rather
  than code.
- **`config path` / `config check`.** With two layers there is now a real question — which files am I
  actually reading — answered today only by the `source` field in the listing's JSON.
