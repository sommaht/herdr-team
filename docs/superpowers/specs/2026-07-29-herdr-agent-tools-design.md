# herdr-agent-tools — design

**Date:** 2026-07-29 · **Status:** implemented, amendment included

## What this is

A CLI that makes launching and prompting [herdr](https://herdr.dev) agents smoother. herdr can
start an agent only in a pane that already exists and is sitting at an interactive shell prompt, so
launching one by hand is always two steps: create a surface, dig the new pane's id out of the JSON,
then start the agent in it. This does both in one command, resolves the agent's kind and its usual
flags from a named preset, and takes prompt text on stdin.

It replaces two shell functions that did the same job, and fixes the problems they had in practice —
recorded under "The findings" below.

## Scope

Three commands, which the amendment at the end later took to five — it adds `kill` and `prime`, and
records why `whoami` and a `--from` flag were declined.

| Command | Purpose |
| ------- | ------- |
| `spawn` | Create a pane, tab, or workspace and start a preset-configured agent in it |
| `prompt` | Deliver a prompt to an agent that already exists |
| `presets` | List what the preset config holds |

**Out of scope for this MVP**, recorded as decisions rather than omissions:

- **Inter-agent messaging and witnessed delivery.** A message envelope carrying the sender's
  identity and a reply path, and the delivery machinery that proves a message arrived, are a
  separate piece of work. What this MVP does carry is delivery *verification* for its own prompts,
  because it is nearly free (see `prompt` step 3).
- **Per-pane agent metadata.** No `ls`, `whoami`, `set`, `get`, or team scope. herdr resolves agent
  targets server-side and owns agent identity, so there is nothing here for a metadata namespace to
  do that herdr does not already do better.
- **The composer guard's styling discrimination.** See "Known limitations".

## The findings this addresses

Observed while running eight concurrent agent seats over four rounds of prompting.

1. **A piped first prompt did not arrive.** The shell function read stdin, called the prompt
   command, reported success, and exited 0. The pane showed an empty composer and a zero context
   count. Nothing had been submitted. A separate prompt sent afterwards worked every time.
2. **A prompt sent within a few seconds of starting an agent was silently swallowed.** Same symptom
   with an explicit call, so it was not the wrapper. The agent's TUI was not yet accepting
   bracketed-paste input, and the prompt command reported status `idle` and exit 0 — a *successful
   submission of nothing*. Waiting and re-sending worked in every case.
3. **Creating a tab or workspace raced with slow shell init.** Twice in six launches,
   `agent start` refused the just-created pane with `agent_pane_busy` — "not an available shell" —
   because the pane was still running `direnv` and the shell had not reached its prompt. herdr was
   correct to refuse; the caller has to retry rather than abandon the seat.
4. **The shell function was not callable from a non-interactive shell.** Its private helper was
   absent from the shell's function snapshot, so every invocation had to `source` a dotfile first.
   More importantly, that made every script depending on it machine-specific.
5. **Prompting a pane a human may be typing in submits their half-written message with yours.**
   This happened twice: user-typed text was sitting unsubmitted in two panes, and sending would have
   submitted their line as part of the prompt.

Findings 1 and 2 turn out to be largely a wrapper bug rather than a missing herdr feature. herdr's
`agent prompt --wait` waits for an observed lifecycle change and returns `agent_prompt_stalled` if
the agent does not move within 5000ms, and `agent start` already blocks on a settle delay plus
interactive-readiness detection. The shell function passed neither. Finding 4 disappears by
construction: a binary on `PATH` has no `source` problem.

## Architecture

### The herdr seam

Every interaction with herdr goes through one function, and every call to it lives in the `herdr`
module. Nothing outside that module spawns a process.

```rust
pub fn run<T: DeserializeOwned>(args: &[&str]) -> Result<T, HerdrError>
```

herdr writes JSON results to stdout and a JSON error object to stderr with exit 1. The two streams
must stay separate: capturing with `2>&1` and feeding the result to a parser works right up until
herdr writes anything at all to stderr on an otherwise successful call — a deprecation notice, a
reconnect warning — at which point the JSON is preceded by prose and the parse dies. The caller
would then report a missing pane id for a surface that was actually created. So `run` captures the
streams separately, checks the exit status, and either parses stdout into `T` or lifts stderr's
`{"error":{"code","message"}}` into a typed `HerdrError` carrying that code.

There is deliberately **no macro** here. The predecessor to this tool drove a different multiplexer
and wrapped its calls in a macro, for two reasons that do not apply: the library it used batched
several command builders into one invocation, so a variadic form did real work, and that library
reported a multiplexer which ran and failed as success, so the macro patched a missing check. Every
herdr call here is one process with one argument list, and there is no third-party builder to
correct. A macro would be sugar over a single function call.

### Propagate rather than restate

**Parse only the fields we branch on; pass everything else through untouched.**

herdr's `agent start` returns a full agent record — `pane_id`, `tab_id`, `workspace_id`,
`terminal_id`, `agent_status`, `cwd`. Under `--json` that record is handed back nested inside our
result rather than translated into a shape of our own, which would have to be revised every time
herdr adds a field. We branch on `pane_id` (read back from surface creation) and `agent_status`
(delivery verification); those get small typed structs, and the rest stays a `serde_json::Value`.

The same holds for failures. herdr's `code` and `message` are carried verbatim: `HerdrError`'s
`Display` *is* herdr's message, and `--json` nests herdr's code alongside it. Nested rather than
passed through flat, so a consumer can still tell our failures from herdr's.

### Validate only what herdr won't

Anything we check that herdr also checks is a check that can *disagree* with herdr — refusing what
herdr would accept, or wording a refusal differently. So each candidate validation is audited
against what herdr already refuses:

| Value | herdr's own check | Verdict |
| --- | --- | --- |
| Prompt target | `agent_not_found`, `agent_target_ambiguous`, resolved server-side | Plain `String` |
| Agent kind | `unsupported_agent_kind`, from its own compile-time list | Plain `String` |
| Prompt text | none — its CLI spec has no non-empty constraint | `NonEmptyText` |
| Pane id | n/a — never parsed, herdr hands it to us | `PaneId` |
| Agent name | `invalid_agent_name` | `AgentName`, see the exception |

`NonEmptyText` earns its place because nothing downstream will catch an empty prompt: at best it is
a confusing refusal, at worst a bare Enter delivered into a live agent.

`PaneId` is not validating input — it prevents *our* mistake. Pane ids are read out of differently
shaped responses (`.result.pane.pane_id` from a split, `.result.root_pane.pane_id` from a tab or
workspace) and handed to `agent start --pane`. Passing a tab id there is a mix-up only this code can
make, and the newtype turns it into a compile error.

**The exception, which amends the rule:** *validate only what herdr won't — except where herdr's
refusal would arrive after we have already changed something.* A bad agent name is only refused at
`agent start`, by which point the tab exists; the pre-check keeps the precondition property that a
rejected command has changed nothing. herdr remains the authority — its rule is restated in one
place with a test pinning it, the error message names it as herdr's, and `invalid_agent_name` is
still propagated if anything slips past.

The same argument would apply to the agent kind, and is deliberately **not** acted on. herdr's kind
list exists only in its compile-time argument parser; its own request schema types the field as a
bare string, and the number of kinds it recognizes demonstrably grows. Hardcoding that list is
exactly the drift the rule warns about, so a bad kind fails at `agent start` with the surface left
open.

### Module map

Twelve files. The rule is the one the predecessor's style guide states: a module boundary has to
name a real dependency. Applying it deletes most of that structure, because **herdr resolves agent
targets server-side** — `agent prompt <target>` takes a pane id or a unique agent name and answers
`agent_not_found` or `agent_target_ambiguous` itself. Target resolution, a team model, and a
one-sweep-per-command execution context all exist to do work herdr already does.

```
src/main.rs          Cli parse, dispatch, exit-code reporting
src/cmd.rs           Cmd trait, AsExitStatus, ExitStatus contract
src/cmd/spawn.rs     create surface → agent start → deliver first prompt
src/cmd/prompt.rs    deliver a prompt to an existing agent
src/cmd/presets.rs   list what the config file holds
src/core.rs          PaneId, NonEmptyText, AgentName
src/core/sink.rs     output seam: human text or tagged NDJSON
src/config.rs        preset file: schema, discovery, parse
src/herdr.rs         run(), HerdrError carrying herdr's code and message
src/herdr/surface.rs pane split / tab create / workspace create
src/herdr/agent.rs   agent start / prompt / get / read
src/harness.rs       composer occupancy
```

What carries over from the predecessor in spirit: the `Cmd` trait (one trait, one `execute` method, clap
derives on the same `*Args` struct so a bad value fails at parse time and nothing downstream
re-validates), the `Sink`, and the exit-status contract.

Three of the predecessor's modules are deliberately absent:

- **No `prelude`.** It exists there so eleven command modules share one import set. Three modules do
  not need a shared import list.
- **No execution `Context`.** The predecessor's earns its keep by holding the one expensive read every
  command answers from. There is no shared expensive read here — the environment lookups are two
  `env::var` calls — and loading eagerly would be actively wrong, since a malformed preset file
  would then break `prompt`, which never touches presets. `main` builds the `Sink` and passes it;
  each command reads the environment or config it needs.
- **No `Screen` type.** The detection snapshot arrives as text and the guard is a function over
  `&str`. A row-vector newtype would buy nothing.

`herdr`'s operations take the thing they act on as their first parameter and stay free functions —
`surface::split(&pane, …)`, `agent::start(&name, …)`. Hanging them off `PaneId` would drag the whole
seam into `core`: a pane id that can split, start, and prompt is no longer a primitive. The module
path is the qualifier a method receiver would otherwise supply.

## Commands

### `spawn`

```
spawn <NAME> [--pane|--tab|--workspace] [--preset <P>] [--prompt <TEXT>|-]
             [--cwd <PATH>] [--focus] [--settle-timeout <MS>] [-- <agent args>]
```

1. **Pre-checks, before anything is created.** The name against herdr's rule. Placement flags are
   mutually exclusive and report the conflicting pair rather than letting the last one win —
   `--tab --pane` is a typo, not a choice. `--pane` outside a herdr pane (`$HERDR_PANE_ID` unset) is
   a usage error naming `--tab` and `--workspace` as the fix.
2. **Resolve the preset** — explicit `--preset`, otherwise the config's `default`. An unknown preset
   lists what is available and exits 3.
3. **Create the surface**, passing `--cwd` explicitly in all three modes. A split would otherwise
   inherit the *pane's* working directory, which stops matching the shell's the moment you `cd`.
   - `pane` → `pane split $HERDR_PANE_ID --direction right --cwd <cwd>`, read `.result.pane.pane_id`
   - `tab` → `tab create --cwd <cwd> --label <name>`, read `.result.root_pane.pane_id`
   - `workspace` → `workspace create --cwd <cwd> --label <name>`, read `.result.root_pane.pane_id`

   `$HERDR_PANE_ID`, never herdr's `--current`: that flag resolves server-side to whichever pane is
   *focused*, which is not this one when the command runs from an unfocused pane.
4. **`agent start <name> --kind <kind> --pane <id> [-- <preset args> <extra args>]`, retrying
   `agent_pane_busy` with backoff** until `--settle-timeout` (default 10000ms). This is finding 3.
   Any *other* failure leaves the surface open and reports its pane id: whatever went wrong is on
   screen in it, and closing the pane would throw the error away with it.

   Extra args after `--` are appended after the preset's args with no merging or de-duplication, so
   the agent's own last-flag-wins rules settle any conflict.
5. **Deliver the first prompt**, if one was given, through the same code path as `prompt`.

Two deliberate changes from the shell function being replaced:

- **Focus is opt-in.** `--focus` rather than focusing by default. A tool meant to be driven by agents
  should not steal the human's focus, and herdr's own guidance is to use `--no-focus` for background
  work. An interactive shell alias can put `--focus` back.
- **stdin is explicit.** The old behaviour read stdin whenever stdin was not a terminal, so a script
  whose own stdin was a pipe had that data silently swallowed and sent as the new agent's first
  prompt — a footgun that required a load-bearing `</dev/null` on every scripted launch. Prompt text
  now arrives on stdin only when asked for with `-`.

### `prompt`

```
prompt <TARGET> <TEXT|-> [--wait-until <STATE>...] [--timeout <MS>] [--no-verify] [--force]
```

The two skip flags are not interchangeable and neither implies the other: **`--force` skips the
composer guard** in step 2, and **`--no-verify` skips the delivery wait** in step 3.

1. **`agent get <target>`** — one call giving both the harness kind, which selects the composer's
   prompt marker, and the current status, which decides whether delivery is verifiable.
2. **The composer guard**, unless `--force`. Detailed below. Occupied means a refusal with exit 5
   and no side effect.
3. **`agent prompt <target> <text> --wait --until working --timeout <ms>`**, where `--timeout`
   defaults to 15000ms.

   `--until working` is the load-bearing choice, and it is finding 2's rule in herdr's own
   vocabulary: treat a prompt as delivered when the status has actually moved, never on the exit
   code. herdr's bare `--wait` waits for the *turn to finish* — idle, done, or blocked — which is
   wrong for a dispatch that should return promptly. `--until working` returns as soon as delivery is
   proven. On `agent_prompt_stalled` or a timeout, re-send once, then fail with exit 5.

   `--wait-until` overrides the states for a caller that wants the full settle wait instead;
   `--no-verify` skips the wait entirely.

**One honest gap:** if the agent was already `working` at step 1, `--until working` matches instantly
and proves nothing. That case succeeds with a sink warning saying delivery was unverified, rather
than reporting a guarantee it does not have.

### `presets`

Reads the config and lists each preset's name, kind, and args, marking the default. Touches herdr
not at all, so it still works with no server running — which is what is wanted when the config file
itself is what is being debugged.

## The composer guard

Finding 5: prompting a pane a human may be typing in submits their half-written message with yours.
The guard reads the target's composer and refuses if it holds text.

herdr's own agent detection already extracts the composer as a named region called
`prompt_box_body`, and that region is **harness-agnostic**: it is the lines between the last two
horizontal rules of the detection snapshot. Only herdr's Claude Code manifest currently declares a
rule using it, so it cannot be read out of herdr's detection-explain output for every kind — but the
same rule applied to the snapshot ourselves is around thirty lines.

```
herdr agent read <target> --source detection --format text --lines 40
```

`--source detection` is the plain-text bottom-buffer snapshot herdr's own agent detection reads. It
is absent from that subcommand's usage line but accepted, and verified working against a live agent.
Forty lines is more than any composer needs and cheap; only the bottom of the snapshot is used.

```
────────────────────────────────────────── project ──
❯
─────────────────────────────────────────────────────
   <status line>
```

Take the lines between the last two horizontal rules, strip the leading prompt marker, and test the
remainder for non-whitespace. Per-harness knowledge is one string: the marker. A horizontal rule is
a line whose leading run of `─` is either the whole trimmed line or at least three characters long,
which is how the labelled top border above still counts as one.

Marker selection is two-tiered, so an unfamiliar kind still gets a check rather than none. The kind
reported by `agent get` selects its marker; if that kind is one this tool holds no marker for, every
known marker is tried against the first non-empty body line and the first that matches is used.

**The guard fails open** in exactly two cases: the body cannot be located (fewer than two horizontal
rules in the snapshot), or no known marker matches. Both emit a sink warning and deliver anyway —
delivered without the guarantee rather than guessed at. A tool that refused every pane it could not
parse would be unusable the first time a harness changed its rendering.

`spawn` does not run the guard on the pane it just created. No human has touched that pane, and the
`--prompt` delivery follows `agent start` immediately.

### Known limitations

- **A placeholder hint reads as occupied.** Some harnesses render dimmed placeholder text inside an
  empty composer. In a plain-text snapshot that is indistinguishable from typed text, so the guard
  refuses. The refusal is retryable and `--force` overrides it. Discriminating by SGR attributes from
  an `--format ansi` read is possible and deliberately deferred — it trades one fragile rule for a
  more fragile one.
- **The guard is a snapshot, not a lock.** A human can start typing between the read and the
  submission. Narrowing that window further requires something herdr does not currently expose.

## Preset configuration

`$XDG_CONFIG_HOME/herdr-agent-tools/config.toml`, falling back to
`~/.config/herdr-agent-tools/config.toml`. Overridable by `--config` and by an environment variable.

```toml
default = 'reviewer'

[presets.reviewer]
kind = 'claude'
args = ['--model', 'opus']

[presets.cheap]
kind = 'codex'
args = ['--model', 'gpt-5-low', '--no-alt-screen']
```

Four decisions here:

- **The file is named for the tool, not for its one table.** Presets are all it holds today, and
  `presets.toml` would have to be renamed the first time a second table is added — a breaking change
  for everyone who already has the file. `config.toml` costs nothing now and buys that room.
- **The preset name is the table key**, which makes a duplicate name inexpressible rather than
  last-one-wins, and drops a `name` field that could disagree with nothing.
- **`args` is an array only.** A string form would have to be split into shell words, which in Rust
  means reimplementing shell word-splitting for a value handed to `Command` — a real hazard for no
  gain. The array form is quoting-safe.
- **The file is this tool's own**, not a second file inside herdr's config directory. A public tool
  should not squat a filename in another project's config directory, where it would break the day
  that project claims the name.

A missing config file exits 3 with the example above in the message. Malformed TOML exits 1 naming
the parse error. A Rust TOML parser fails *closed*, which is the point: the shell version's TOML
reader failed *open* — malformed input yielded an empty document and exit 0 — so one typo in the
config surfaced as "preset not found" and sent you hunting through your command line instead of the
config file.

## Errors and output

| Code | Meaning | Cases |
| ---- | ------- | ----- |
| 0 | success | |
| 1 | general failure | herdr not on `PATH` or will not spawn; an unclassified herdr code; config unreadable or malformed |
| 2 | usage | clap's own; conflicting placement flags; `--pane` with no `$HERDR_PANE_ID`; `agent_target_ambiguous` |
| 3 | not found | preset not in the config; config file absent; `agent_not_found`, `agent_pane_not_found` |
| 5 | conflict, retryable | composer occupied; `agent_pane_busy` after the retry window; `agent_prompt_stalled` after the re-send; `agent_name_taken` |

The codes are stable so a caller can branch on them without parsing stderr. `AsExitStatus for
HerdrError` matches on herdr's `code` and maps only the codes with a meaningful non-`1` answer;
an unrecognized code is a general failure. There is no error-code enum of our own — the exit status
*is* the machine-readable classification, and it comes from the value a command returns.

**Neither a prompt nor a captured composer may reach an error message or a log.** `HerdrError`
records the herdr command *name*, never its arguments, because the argument to `agent prompt` is the
prompt text. The composer guard has the same hazard from the other side: its refusal says the
composer holds unsent text and never says what that text is.

Output goes through the sink; no command prints.

- **Human mode** is the default: results on stdout, diagnostics on stderr, one line each. `spawn`
  prints `<name> (<kind>) → <pane id>`.
- **`--json`** emits tagged NDJSON — one object per line, all on stdout, each carrying a `type` of
  `result`, `warning`, or `error`. One stream, because a consumer cannot rely on two streams'
  relative ordering once either is redirected, and the tag already carries what the stream choice
  would have said.

```json
{"type":"result","placement":"tab","delivered":true,
 "agent":{"agent":"claude","agent_status":"working","pane_id":"w4:p17",
          "tab_id":"w4:t3","workspace_id":"w4","terminal_id":"term_…","cwd":"/…"}}
```

```json
{"type":"error","status":5,"message":"agent target pane w4:p16 is not an available shell",
 "herdr":{"command":"agent start","code":"agent_pane_busy"}}
```

Writes are best-effort: a failed write to a closed pipe is swallowed rather than panicking, which is
what a bare `println!` would have done.

## Testing

**No automated test invokes herdr, starts an agent, or touches the user's session.** What remains is
reachable in-crate, so there is no integration-test binary; tests are inline `#[cfg(test)] mod tests`
under a `Tests` header.

1. **Config** — name-as-key parse, a missing `default`, an unknown preset, malformed TOML.
2. **Argument-vector construction, pinned by exact-string tests**, one per herdr call. This is where
   the seam's correctness actually lives.
3. **Composer occupancy, table-driven over committed fixtures** — detection snapshots asserting
   empty, occupied, and both fail-open paths (no rules found, unrecognized marker).

   The fixtures carry neutral filler above the composer. A snapshot captured from a live pane holds
   that pane's whole transcript, and the guard reads only the composer's structure — the rules, the
   marker, and whether anything follows it — so the surrounding text is filler by design.
4. **Error-code mapping** — herdr code to `ExitStatus`, including the unknown-code default.
5. **Sink wire forms** — exact-string, in both modes.

Verification runs `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test --all-targets`, and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`. The rustdoc check
earns its place by catching a class the others cannot see: doc comments here carry the reasoning
behind decisions and link the types that reasoning names, so a link that stops resolving is the
first sign a doc has outlived its code.

Nothing above exercises a live herdr server. Verification claims name the command that produced
them, and static checks are reported separately from any manual live rehearsal.

## Repository scaffolding

One binary crate. `rust-toolchain.toml` pins the channel with a minimal profile plus clippy and
rustfmt. `rustfmt.toml` sets `max_width = 120`. `Cargo.toml` denies `unsafe_code` and clippy's
`mod_module_files` — the latter is what forces `core.rs` over `core/mod.rs` — and keeps the release
and profiling profiles explicit.

Dependencies: `clap` with `derive`, `clap-stdin`, `serde`, `serde_json`, `thiserror`, `derive_more`,
`getter-methods`, and `toml`. `tempfile` as a dev-dependency. `toml` is the one addition relative to
the predecessor, and it replaces the two external command-line tools the shell version needed.

`docs/STYLE-GUIDE.md` states every rule it relies on inline, and its verification section lists only
the four commands above.

`CLAUDE.md` and `AGENTS.md` carry three items: follow the style guide; **herdr is the authority for
its own CLI syntax** — print a command group to read it rather than guessing a flag; and never put
prompt text or captured terminal content in an error message.

## Amendment — placement, `kill`, `prime`

**Status:** implemented. Everything above describes the first three commands; this section supersedes
the lines it names, and where the two disagree this one is what the code does.

Two ideas raised alongside these three were rejected, and both are recorded because the reasoning is
the useful part:

- **A `whoami` command** printing the caller's pane, tab, and workspace. Rejected because `herdr pane
  current` already does it: with no arguments it resolves the *caller's* pane, not the focused one —
  verified against a live unfocused pane, where it returned that pane rather than the focused one.
  It reports `pane_id`, `tab_id`, and `workspace_id` together. This restores the Scope section's
  original decision, which named `whoami` among the metadata commands herdr already does better.
  `prime` carries a line pointing at it instead.
- **A `--from <pane-id>` flag** for `spawn`, deferred rather than declined; see "Open for later".

### `--placement`, superseding the synopsis at `spawn` step 1

```
spawn <NAME> [--placement pane|tab|workspace] …
```

One value flag replacing the `--pane|--tab|--workspace` group, defaulting to `pane`. `Placement`
already exists as an enum in `herdr/surface.rs`, so a `ValueEnum` derive makes the parsed shape *be*
the domain type instead of a boolean triple translated into it — which deletes `PlacementFlags` and
`SpawnArgs::placement()`, about twenty-five lines, and resolves the RS-033 pressure that file's
module doc currently justifies.

The default becomes declarative (`default_value_t`) rather than the implicit `else` of an if-chain,
and `--help` prints the possible values on one line, which also communicates the exclusivity that
three separate bools only imply. This retires step 1's note about reporting the conflicting pair:
`--tab --pane` is no longer expressible.

The extra characters are the one cost, and they are the cost this project has already accepted for
`herdr-agent-tools` over `herdr-agent` — length is a cost when a human types it, and an agent is the
caller.

### `kill`

```
kill <TARGET> [--force]
```

**This is `pane close` that refuses to destroy work in progress.** herdr's own `pane close` takes a
pane id and no flags at all: no confirmation, no guard, nothing between a misread status and lost
work. That refusal is the whole product, and the precedent is `prompt`, which is accepted on exactly
this bargain — `herdr agent prompt` is also one command, and `prompt` earns its keep by declining to
submit into a composer someone is typing in, not by saving keystrokes.

Two things make the hazard worse here than in the composer's case. Submitting into an occupied
composer is recoverable, since the text is still on screen; killing a working agent loses whatever it
has not written to disk. And the caller is usually another agent acting on a lifecycle status it may
have misread — the one kind of caller that cannot look at the screen and think twice.

1. **`agent get <target>`** — one call giving both the pane id to close and the status the guard
   reads. No second call: the resolution and the guard share one response.
2. **The status guard, unless `--force`.** Refuse `working` and `blocked` with exit 5. Warn and
   proceed on `unknown`; proceed silently on `idle` and `done`. It reuses the composer guard's shape
   exactly — exit 5, retryable, the same `--force` — and it fails **open** on `unknown` for that
   guard's reason: refusing on absent evidence is a refusal the guard never earned.
3. **`pane close <pane-id>`.**

If step 1 answers `agent_not_found`, close the target directly as a pane id and let herdr judge it.
An agent that exited leaving its pane open is the main thing anyone wants to clean up, and refusing
that would be perverse. Passing the target through rather than testing its shape keeps id-format
knowledge out of this crate, and herdr's rejection of a non-pane-id propagates unchanged.

Collapsing name resolution into the same command is a **side benefit, not the reason.** `spawn`
prints the new pane's id, so the caller most likely to kill an agent already has what `pane close`
wants; resolution only helps a caller that has a name and lost the id, and `agent list` hands out ids
too.

**Named `kill` rather than `stop`, despite the guard.** The guard is a precondition, not a change of
semantics: with `--force`, and on `idle`/`done`/`unknown` without one, the agent dies and its pane
disappears. Guards do not soften verbs — `rm` refuses a directory without `-r` and is still `rm`. And
`stop` would mis-suggest twice: it implies an inverse, as `docker stop` and `systemctl stop` both
have, where this operation has none; and *ending an agent while leaving its pane* is a real, separate
thing in herdr's vocabulary that this is not. The alarming name is also part of the guard, since
`prime` puts this synopsis in front of every agent that reads it.

### `prime`

```
prime
```

Prints a compact, agent-facing brief on driving this CLI. Written for a `SessionStart` hook, which
sets every constraint that follows.

**No herdr calls, and a missing config file is not a failure.** The hook fires before anything
guarantees a running server, and a hook that fails is worse than one that says little. An absent or
unreadable config prints the prose with "no presets configured" where the table goes, and exits 0.

**Invocations grouped by intent, not explanation.** Composing it from clap's command tree was
considered and rejected: five subcommands render past two hundred lines, and always-on context has a
cost that a reference dump cannot justify when `--help` is one command away.

The first draft went too far the other way and was mostly paragraphs. A caller reaching for this wants
the invocation, so the brief is blocks of runnable lines under `## Launching agents`, `## Prompting
agents`, `## Ending agents`, and `## Common workflows` — with the prose that remains attached to the
line it qualifies. `prompt` returning on delivery rather than on completion is a clause on the
`--wait-until` row, not a section. Two blocks stay tabular because they are the things a flag list
genuinely cannot say: which failures are worth retrying, and which commands refuse by default.

**`--hook <harness>` wraps it for a host.** A session-start hook wants JSON on stdout, not text, and
the shape is the host's to define. Each harness answers for its own via `AgentHarness::hook`, so this
command never learns an envelope's shape — the same bargain `readiness` already makes in the other
direction.

`hook` is a **required** trait method, not a defaulted one, even though both impls call the same shared
envelope builder today. A default would let a harness added later inherit an envelope nobody checked
against its host; requiring it means the author has to state what that host reads, and each impl's doc
comment is where the evidence for that claim lives. The two claims are not equally strong, and saying
so is the point:

- **Claude Code** — its documented `SessionStart` shape, and the basis for the shared builder.
- **Codex** — the same envelope on weaker evidence. Established: it runs a `SessionStart` hook, and its
  hooks answer with JSON on stdout, since a generated Codex hook config falls back to `echo '{}'` on
  every event. Not established: which keys it reads to inject context. The shape rests on the tool this
  borrowed the idea from wrapping all three of its hosts in one envelope from a single flag.

That asymmetry is exactly what a defaulted method would have hidden. The flag takes a harness rather
than being a boolean for the same reason: it makes the host explicit, and gives a divergence somewhere
to land without a flag change.

**The presets table is generated**, from the same `PresetList` the `presets` command renders, so what
the brief says about presets cannot disagree with what `spawn --preset` will do.

**`--json` prints the plain brief** — the crate's one exception to its own output contract, carried by
`Cmd::TEXT_IN_BOTH_MODES`, which only `prime` overrides. A brief is a document; a JSON envelope around
prose buys escaping and no information. Failures stay JSON under `--json` for every command, because a
consumer branching on failures needs the tag whatever the command was.

Four tests hold the hand-written part honest, since a static text blob is otherwise the least-tested
artifact here:

1. **The brief names exactly the commands that exist**, in both directions. A command renamed out from
   under the brief fails one way; a command *added* without being documented fails the other, and that
   is the one that would otherwise go unnoticed — an agent cannot reach for what the brief never
   mentions.
2. **Every flag it names still exists** somewhere in the clap tree. It cannot catch a flag attributed
   to the wrong command; with five commands, that is what reading the brief is for. Two details are
   load-bearing: global flags live on the *root*, not on the subcommands clap propagates them to, which
   is what made a first version reject `--json`; and the assertion that the extracted set is *non-empty*
   is what stops the test passing vacuously when the trimming breaks, which it did.
3. **Every exit code it tabulates is one the contract reports.** Asserted in that direction rather than
   the reverse: a first version checked that each status *appeared* and passed a renumbering, because
   the brief names the retryable code twice and `contains` was satisfied by the mention that had not
   changed. Only rows whose first token is a bare integer are read, so a `--timeout` default in
   milliseconds is not mistaken for a status.
4. **The brief stays inside an eighty-line budget.** Raised from forty when it became command blocks
   instead of paragraphs — runnable lines earn their length in a way explanation does not. Still a
   ceiling, and it covers the hand-written part only: the preset table's length belongs to whoever wrote
   the config.

Each was verified by mutation rather than by assuming it would fire; two of them did not, and are the
reason the notes above are specific.

The first draft, kept for the record — its synopsis and its explanatory shape are both superseded:

```text
herdr-agent-tools launches and prompts herdr agents. Reach for it instead of `herdr` when
starting one: herdr needs a pane that already exists and is sitting at a shell prompt, so
`spawn` creates the surface and starts the agent as one step.

  spawn <name> [--placement pane|tab|workspace] [--preset <p>] [--prompt <text>|-]
  prompt <target> <text|->
  kill <target> [--force]
  presets

A target is a unique agent name or a pane id; herdr resolves it. Your own ids come from
`herdr pane current`.

Sequencing work. `prompt` returns as soon as delivery is proven, not when the agent has
finished — pass `--wait-until idle` to wait for a result instead. Long or generated prompt
text goes on stdin with `-` rather than being quoted into an argument.

Exit codes are a protocol, not just failure. 5 is retryable and worth retrying: a composer
holding unsent text, or a new pane whose shell has not started yet. 3 means what you named
does not exist and 2 means the arguments were wrong; neither improves on a retry.

Two refusals you will meet. `prompt` refuses a composer holding unsent text, because
submitting yours would submit someone's half-written message along with it. `kill` refuses
an agent that is working or blocked. Both are exit 5, and both take `--force` when you
mean it.

Presets:
<generated table>
```

This retires the "printed conventions block" entry in "Open for later", which is what it is.

### What the amendment adds to the module map

Three files, taking the map from twelve to seventeen — `src/cmd/kill.rs`, `src/cmd/prime.rs`, and
`src/core/backoff.rs`, plus the two `src/harness/` files an earlier refactor added. `surface.rs` grows
`close` and `workspace_of`, which makes it the three ways to make a pane, the one way to take one
away, and the one question asked about one. Neither new command needed a module of its own beneath
`cmd`, and `prime` reaches sideways for `presets::PresetList` rather than minting a second renderer of
the same table.

`core/backoff.rs` is the retry schedule, moved out of `spawn` once that file crossed RS-033's line
count. RS-032 is what settles where it went: it forbids minting a file for a handful of lines, and
blesses a *substantial* single-consumer module — this is forty non-test lines with thirty of tests,
and it is a value type rather than a role carved out of one. It sits in `core` rather than nesting
under `spawn` because it knows nothing of herdr, agents, or panes; it is `Duration` arithmetic, which
is what `core` is for. `prompt`'s re-send is the plausible second caller, if it ever grows a delay.

## Amendment — `--placement worktree`

**Status:** specified, not yet built. A fourth value for the flag the previous amendment introduced;
everything that section says about the other three still holds.

```
spawn <NAME> [--placement pane|tab|workspace|worktree] [--branch <NAME>] [--base <REF>] …
```

A worktree is a fourth answer to the question `Placement` already asks — where does this agent's
pane come from — so it is a variant rather than a `--worktree` bool sitting beside the flag. A bool
would need `conflicts_with`, which fires even on an explicit `--placement workspace` and reports a
pair where the value flag reports the whole set. That is the trade the previous amendment made in
the other direction, and re-adding a bool would undo it in miniature.

### What herdr does with it

`worktree create` creates a *workspace*, so this placement is workspace-shaped. It answers
`{workspace, tab, root_pane, worktree}`, and `root_pane.pane_id` is the field `tab create` and
`workspace create` are already read for.

```
worktree create --cwd <CWD> [--branch <NAME>] [--base <REF>] --label <NAME> --(no-)focus
```

`--cwd` names the **source checkout**, not the new surface's working directory — herdr picks where
the checkout lands, under its own `worktree_directory`. `spawn --cwd` still means what it meant,
"the directory this agent works on"; only what herdr does with it differs, and the flag's value is
unchanged.

It is passed on every call and never omitted. Given neither `--cwd` nor `--workspace`, herdr
resolves the source to the active workspace — the *focused* one. This is the third place in this
crate where a herdr default depends on what the human last clicked, after `pane split --current`
and `tab create` without `--workspace`. The second of those was fixed only after a rehearsal launch
opened its tab in an unrelated project; here the cost would be worse than a misplaced surface, since
a worktree would be cut from whichever repo the human happened to be looking at.

### `--branch` and `--base`

Pass-throughs with no default of this crate's own. herdr generates
`worktree/<adjective>-<noun>-<hex>` for an absent branch and uses `HEAD` for an absent base;
restating either here would be a second authority to keep in step, which is "propagate rather than
restate" applied to defaults rather than to fields.

A branch defaulting to the agent's name was considered and rejected, and the reasoning is the
useful part. It reads far better afterwards — `worktree/reviewer` in `git branch` says who made it,
where a generated slug says nothing — but it collides on reuse. `kill` closes a pane and never runs
`worktree remove`, so the branch outlives the agent, and the second `spawn reviewer --placement
worktree` fails at `git worktree add`. A command that works once and then stops is a worse failure
than an opaque name, and the attribution it would buy is already carried by the label.

`--path` is not exposed: herdr's `worktree_directory` config owns where checkouts land, and a caller
overriding it per spawn is fighting their own configuration.

`--label` is the agent's name, as it is for a tab and a workspace. It carries more weight here —
with a generated branch, the workspace label is the only thing tying a checkout back to the agent
that made it.

### Worktree-only flags under another placement

`--branch` or `--base` with any other placement is a usage error, checked in `execute` before
anything is created. clap 4 cannot express "valid only when `--placement` is `worktree`" —
`conflicts_with` takes an argument id, not a value predicate — so this is one of the "arguments that
parse but cannot be honored" that row 2 of the exit-status contract already covers.

Ignoring the flag was the alternative and is worse: a caller templating `--branch` into every spawn
would never learn that the isolation it asked for did not happen.

### What the seam grows

`surface::create_worktree(branch, base, label, source, focus) -> Result<(PaneId, Checkout),
HerdrError>`, and one public type:

```rust
pub struct Checkout { pub branch: Option<String>, pub path: String }
```

`Checkout` is deserialized from herdr's response and serialized into ours unchanged — what "parse
only the fields we branch on" looks like when the fields are not branched on at all, but reported.
herdr's `WorktreeInfo` also carries `is_bare`, `is_detached`, `is_prunable`, `is_linked_worktree`,
`open_workspace_id`, and `label`; serde drops what it is not asked for. Those six are constants on
something created a millisecond ago, unlike `AgentRecord`, which is nested whole because it is an
identity rather than a snapshot. `branch` stays `Option` because herdr's field is optional, not
because a create is expected to produce a detached checkout.

The return is a tuple, not a named struct. `surface.rs` makes four things now, and `(PaneId,
Checkout)` names both halves without minting a fifth type to hold them.

### What `spawn` reports

`Spawned` grows `worktree`, omitted for the other three placements the way `delivered` is already
omitted when no prompt was given:

```json
{"type":"result","placement":"worktree",
 "worktree":{"branch":"worktree/lucky-harbor-8e01",
             "path":"/…/worktrees/herdr/worktree-lucky-harbor-8e01"},
 "agent":{"agent":"claude","agent_status":"working","pane_id":"w9:p1", …}}
```

Human mode gains a suffix: `reviewer (claude) → w9:p1 [worktree/lucky-harbor-8e01]`.

Reported because the branch is herdr's to generate, which makes this response the one cheap moment
it is knowable. Without it, every caller that cares runs `worktree list` against the source repo and
then guesses which of several checkouts is the one it just made.

### One exit-status correction

`worktree_operation_in_progress` — another create or remove already running against that checkout —
joins the codes mapped to 5. It is transient by construction, and row 5 is for exactly that. The
three other codes a worktree spawn can meet stay at 1, correctly: `not_git_worktree` and
`linked_worktree_source` are about where the caller is, and `worktree_create_failed` is git
refusing. None of them improves on a retry.

`linked_worktree_source` is the one worth knowing in advance. herdr refuses to create a worktree
whose source is itself a linked worktree, so a worktree agent cannot spawn a nested worktree agent.
herdr's rule and herdr's message, forwarded.

### What the amendment adds to the module map

Nothing. `surface.rs` gains a fourth creator and the type it reports, which is the module's stated
job, and `spawn.rs` gains two flags and a match arm.

## Open for later

Kept in the README's roadmap so each stays a decision rather than an omission:

- **Removing a worktree when its agent ends.** `kill` closes a pane, and `worktree remove` is a
  separate herdr call, so a killed worktree agent leaves its checkout and its branch behind. This is
  what makes a name-derived branch collide on reuse, above.

  Deferred rather than declined, because the safe version needs a dirty-checkout policy this MVP has
  no opinion on. herdr's `worktree remove --force` exists for precisely that question, and answering
  it on behalf of a caller who may have uncommitted work in there is not a default worth guessing.

- **Inter-agent messaging with an envelope carrying the sender's identity and a reply path.** Not
  polish on `prompt` — a different job, and the distinction is worth stating because it decides
  when this becomes necessary rather than nice.

  A lifecycle wait tells a caller **when**: it needs no cooperation from the agent, works on one
  that was never told a reply path exists, and cannot be forgotten. It carries no content. An
  envelope with a `reply with:` line tells a caller **what**, at the cost of an explicit
  instruction in the prompt and a recipient that honors it.

  Everything this MVP dispatches is work whose result lands on disk or in git, so the artifact
  *is* the reply and the caller verifies it directly — the wait suffices. Dispatch a *question*
  instead of a task, and the wait reports only that the agent stopped; without a reply path the
  caller is back to scraping a pane for the answer. That is the boundary this feature sits on.
- **Witnessed delivery — proving a message was received rather than that it was submitted.**
  Distinct from the verification `prompt` already does: `--until working` proves herdr accepted the
  text and the agent reacted, not that the agent read what it was sent.
- Discriminating placeholder text from typed text in the composer guard, via styling attributes.
- **Spawning somewhere other than here.** `spawn` infers where it is from the environment, so it can
  only split the pane it runs in, and only when it runs in one. A `--from <pane-id>` flag would let a
  caller outside a herdr session split anyway, and one inside anchor somewhere other than itself.

  A pane id is the only id worth accepting there, which is the part worth writing down: it *is* the
  split anchor, and it *derives* the workspace a new tab opens in, so it answers both questions
  `spawn` asks. A tab or workspace id answers strictly fewer, and an agent name fewer still — it
  cannot name a pane that hosts no agent, and it adds `agent_not_found` and `agent_target_ambiguous`
  to a flag whose job is purely positional.
- **Ending an agent while keeping its pane.** `kill` closes the pane, so the seat goes with the agent.
  A variant that ends the agent and leaves its pane at a shell prompt would let a caller reuse a warm
  seat instead of paying `spawn`'s settle race again, which is the slowest part of a launch.

  Deferred because both routes to it are bad. `pane release-agent` takes `--source` and `--agent`
  because it is how a harness reports *its own* state to herdr, so calling it from outside means
  lying to herdr about state that is not ours to report. Ending the process while keeping the shell
  means sending harness-specific keys, which is precisely the per-harness mechanism the `harness`
  module exists to keep down to one character. This is the one place `stop` would be the right name.
