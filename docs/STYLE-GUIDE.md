# Rust style — herdr-agent-tools domain layer

Core Rust style is owned by the **`rust-style` skill**, vendored into this repository at
`.claude/skills/rust-style/`: `SKILL.md` carries the generation rules, `references/gate.md` is
the binding rule set (stable `RS-###` IDs), the other `references/` files carry the judgment
behind them, and `scripts/check.sh` is the mechanical gate — its `assets/pre-commit` is
installed as this repo's hook. That rule set is not restated here.

This file holds only what is **specific to herdr-agent-tools**. Where it is silent, the skill
applies. An override of a core rule cites its RS ID explicitly; see **RS-030** under "The
herdr seam".

## Package

One binary crate, edition 2024, toolchain pinned in `rust-toolchain.toml`. Module boundaries
do the work a crate graph would — a second crate is a design decision with a document, not a
directory someone adds. Release and profiling profiles stay explicit in `Cargo.toml`.

## Section headers (house convention)

Major file sections use a heavyweight header; sub-groups use a lightweight one, both padded to
`max_width`:

```rust
// =====================================================================================================================
// Section Name
// =====================================================================================================================

// ---------------------------------------------------------------------------------------------------------------------
// Sub-Group Name
// ---------------------------------------------------------------------------------------------------------------------
```

Standard skeleton when the section exists: `Constants` at top, one heavyweight header per
domain group, `Helpers` for file-wide utilities, `Tests` for the inline `#[cfg(test)]` module.
Lightweight sub-headers are earned, not automatic.

## Module map

- **`core`** — what every other module shares, and deliberately little: `PaneId`,
  `NonEmptyText`, `AgentName`, and the output seam `Sink`. It is the only module here with no
  dependency on the others' vocabulary.

  There is **no execution context** and **no target-resolution layer**, because herdr resolves
  agent targets server-side: `agent prompt <target>` takes a pane id or a unique agent name
  and answers `agent_not_found` or `agent_target_ambiguous` itself. A context type would earn
  its keep only by holding a read every command answers from, and there is none — the
  environment lookups are two `env::var` calls. Loading anything eagerly would also be wrong:
  a malformed preset file would then break `prompt`, which never touches presets.
- **`herdr`** — every interaction with herdr, and the seam that runs them. The module root
  carries `run`, `HerdrError`, and the stream discipline both depend on; `herdr::surface` owns
  the three ways to make a pane, and `herdr::agent` owns what a command does to an agent in
  one. It imports the domain types it reads into, all of them `core`'s.
- **`harness`** — what this tool knows about each supported agent CLI, which is one string per
  kind: its composer's prompt marker. Touches no process and no pane — it is handed a
  detection snapshot as `&str` and answers whether the composer holds text. The region rule
  lives here once; each kind supplies only its marker.
- **`config`** — the preset file: its schema, where it is found, and how it is read. The only
  disk I/O in the crate, which is the boundary it names.
- **`cmd`** — the subcommands, their side-effect ordering, and the exit-status contract. There
  is deliberately no parent grouping: three sibling commands share no distinction a parent
  would mark.
- **`main`** — parses and dispatches. No command logic. `cli` is not a module of its own: with
  three commands the dispatch match is a handful of lines, and a file holding only module
  declarations plus that match names no boundary. What it does own beyond dispatch is the
  **argument failure** — clap's rejection restated in this crate's own words, so it renders
  through the sink like every other failure and repeats nothing the caller typed.

There is **no prelude**. It earns its keep when a dozen command modules share one import set;
three modules state their own imports (**RS-050**).

The clap derives and the `Cmd` impl live on the same `*Args` struct: the parser shape and the
command are deliberately one type. The boundary that matters is value-level — `#[arg]` fields
parse straight into domain types, so a bad value fails at parse time and nothing downstream
re-validates.

## The herdr seam

**Every herdr invocation goes through `herdr::run`**, and every call to it lives in the `herdr`
module. No module outside `herdr` spawns a process or names the `herdr` binary.

`run` exists for a discipline, not for ergonomics: **herdr's two streams must stay separate.**
It writes JSON results to stdout and a JSON error object to stderr with exit 1. Capturing with
`2>&1` and feeding the result to a parser works right up until herdr writes anything at all to
stderr on an otherwise successful call — a deprecation notice, a reconnect warning — at which
point the JSON is preceded by prose and the parse dies. The caller would then report a missing
pane id for a surface that was actually created. So `run` captures the streams separately,
checks the exit status, and either parses stdout or lifts stderr's error object into a typed
`HerdrError`.

There is deliberately **no macro** wrapping it. A macro over a seam earns its place when it
batches several operations into one invocation or patches a check a third-party builder omits.
Every herdr call here is one process with one argument list and there is no builder to
correct, so a macro would be sugar over a single function call (**RS-050**).

`HerdrError` carries the herdr command **names**, never its arguments — the argument to
`agent prompt` *is* the prompt text.

**Override RS-030** (free function with an obvious owner). The `herdr` module's operations take
the thing they act on as their first parameter and stay free functions — `surface::split(&pane,
…)`, `agent::start(&name, …)`, `agent::read(&target, …)`. RS-030 would make them methods on
`PaneId`, but `PaneId` is a `core` primitive: an id herdr handed us and that we hand back.
Hanging every herdr verb off it would drag the whole seam into `core` — a pane id that can
split, start, prompt, and read is no longer a primitive. The rule's own escape, an `*Ext`
trait, buys nothing here: the module holds no methods on `PaneId` to be inconsistent with, so
the mixed-module smell RS-030 exists to catch is absent. The module path is the qualifier a
method receiver would otherwise supply.

## Propagate rather than restate

**Parse only the fields we branch on; pass everything else through untouched.**

herdr's responses are the wire format, and re-describing them in types of our own buys a
translation layer that has to be revised every time herdr adds a field. So the seam
deserializes small structs holding only what the flow branches on — `pane_id` from surface
creation, `agent_status` from a prompt — and carries the rest as `serde_json::Value` to be
nested in the result verbatim.

The same holds for failures. herdr's `code` and `message` are carried as they came:
`HerdrError`'s `Display` *is* herdr's message, and nothing re-words it. Under `--json` herdr's
code is nested inside our envelope rather than emitted flat, so a consumer can still tell our
failures from herdr's.

## Validate only what herdr won't

Anything checked here that herdr also checks is a check that can **disagree** with herdr —
refusing what herdr would accept, or wording a refusal differently. So a validation earns its
place only by covering something herdr does not:

- **`NonEmptyText`** earns it. herdr's prompt argument has no non-empty constraint, so an empty
  prompt is at best a confusing refusal and at worst a bare Enter delivered into a live agent.
- **`PaneId`** earns it for a different reason: it is not validating input, it prevents *our*
  mistake. Ids are read out of differently shaped responses and handed to `agent start
  --pane`; passing a tab id there is a mix-up only this code can make, and the newtype makes
  it a compile error.
- **A prompt target, and an agent kind, do not.** Both are plain `String`. herdr answers
  `agent_not_found` / `agent_target_ambiguous` and `unsupported_agent_kind`, and its kind list
  is not published anywhere machine-readable — hardcoding it would drift every time herdr
  learns a new agent.

**The one exception, which amends the rule:** *validate only what herdr won't — except where
herdr's refusal would arrive after something has already changed.* An agent name is refused
only at `agent start`, by which point the surface exists, so `AgentName` pre-checks it and the
precondition property holds — a rejected command has changed nothing. herdr stays the
authority: the rule is restated in one place with a test pinning it, the message names it as
herdr's, and `invalid_agent_name` is still propagated if anything slips past.

Preconditions are otherwise checked before anything in herdr changes. Where a step fails
*after* creating a surface, the surface is deliberately left open and its pane id reported:
whatever went wrong is on screen in it, and closing the pane would throw the error away.

## Neither prompts nor terminal content may be logged

A prompt is the argument to `agent prompt`, and the composer guard's input is a snapshot of
someone's half-written message. Neither may reach an error message, a diagnostic, or a log.
The guard's refusal says the composer holds unsent text and never says what that text is.

**The parser is inside the rule, not outside it.** clap repeats the offending value in its own
rejection, and for `prompt` and `spawn --prompt` that value is the prompt — so `main` parses
with `try_parse` and rebuilds the rejection from clap's structured context. The rebuild is an
**allowlist**: a string reaches the message only if it is a spelling this build declares, taken
from `Cli::command()` itself. Filtering the caller's tokens instead would be a guess about what
a prompt can look like, and a prompt is arbitrary text.

Both prompt inputs also carry `allow_hyphen_values`, so text opening with `--` is delivered
rather than rejected. That is the same rule from the other side: a value that parses is a value
no diagnostic can repeat.

## Exit-status contract

The codes are stable so a caller can branch on them without parsing stderr:

| Code | Meaning |
| ---- | ------- |
| 0 | success |
| 1 | general failure |
| 2 | usage error — clap's own code, extended to arguments that parse but cannot be honored |
| 3 | resource not found |
| 5 | conflict (state the target already holds — a composer with unsent text, a pane that is not yet an available shell); retryable |

There is no `ErrorCode` enum: the exit status *is* the machine-readable classification, and it
comes from the value a command returns — never from a diagnostic pushed along the way.
`AsExitStatus for HerdrError` matches on herdr's own code and maps only the codes with a
meaningful non-`1` answer; an unrecognized code is a general failure, not a compile error.

One thing is read before that code: herdr's own process exit status, and only the value 2.
herdr answers a bad argument the way this crate does — a plain line and no error object, so no
code — and every argument in the vector it rejected came from a flag this crate's caller set.
Forwarding it is what keeps a value passed straight through, such as `--wait-until`, a usage
error rather than an operational one.
(The `AsExitStatus` mapping itself and fix-first message wording are core rules — see the
skill's `references/errors.md`.)

## Output goes through the sink

**No command prints.** A command returns its result and pushes its diagnostics; `core::sink`
decides what that looks like. `Cmd::Ok` carries independent `Display` and `Serialize` impls,
because the human and wire forms genuinely differ.

The `Sink` is built in `main` *before* any command runs, so a failure that happens before a
command starts renders under the same contract as everything else.

- **Human mode** is the default: results on stdout, diagnostics on stderr, one line each.
- **`--json`** emits tagged NDJSON — one object per line, all on stdout, each carrying a `type`
  of `result`, `warning`, or `error`. Everything shares one stream because a consumer cannot
  rely on two streams' relative ordering once either is redirected, and the tag already carries
  what the stream choice would have said.

Writes are best-effort: a failed write to a closed pipe is swallowed rather than panicking,
which is what `println!` would have done. Wire forms are pinned by exact-string tests.

## External effects

Every process invocation is built from `Command` args — never an interpolated shell string.
There is no exception: unlike a multiplexer that starts its child through a shell, herdr takes
the agent's arguments as an argument vector, so nothing here needs shell quoting.

A preset's `args` is therefore an **array only**. A string form would have to be split into
shell words, which means reimplementing shell word-splitting for a value handed to `Command`.

## Tests

- **Automated tests never invoke herdr**, start an agent, or touch the user's session. A test
  assembles what it needs through the same constructors the read path uses.
- No integration-test binary: everything worth testing is reachable in-crate, and what is not
  needs a live herdr server. Tests are inline `#[cfg(test)] mod tests` under the `Tests`
  header.
- **Argument vectors are pinned by exact-string tests**, one per herdr call. That is where the
  seam's correctness actually lives.
- Wire forms are pinned the same way — the sink's two modes, the record's JSON.
- **Composer fixtures carry neutral filler above the composer.** A snapshot captured from a
  live pane holds that pane's whole transcript, and the guard reads only the composer's
  structure, so the surrounding text is filler by design.
- Tests own their temp directories.

## Verification

`cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test --all-targets`, `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`, and
`bash .claude/skills/rust-style/scripts/check.sh src`.

The rustdoc one earns its place by catching a class none of the others can see. Doc comments
here carry the reasoning behind decisions and link the types that reasoning names, so a link
that stops resolving is the first sign a doc has outlived its code — a variant that moved
between enums, a helper that was renamed.

Verification claims name the command that produced them, and static checks are reported
separately from any manual rehearsal against a live herdr session — nothing above exercises a
real herdr server.

## Dependencies

Extending the skill's sanctioned set, this repo also uses `clap` + `clap-stdin` (CLI) and
`toml` (the preset file), plus `tempfile` for test temp directories.

## Forbidden

- Spawning a process, naming the `herdr` binary, or shelling out, outside the `herdr` module.
- `println!` or `eprintln!` outside `core::sink` — a command that prints has bypassed the
  `--json` contract, and its output cannot be asserted in a test.
- Shell-string interpolation anywhere; every process invocation is built from `Command` args.
- Process I/O in `harness`, whose contract is answering from the snapshot it is handed.
- A prompt, a preset's arguments, or captured terminal content in an error message or a log.
- Re-wording a herdr error, or restating a herdr response in a type of our own beyond the
  fields the flow branches on.
- Duplicating a validation herdr already performs, absent the stated precondition exception.
