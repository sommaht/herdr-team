# herdr-agent-tools Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `herdr-agent-tools` binary — three commands (`spawn`, `prompt`, `presets`) that launch and prompt [herdr](https://herdr.dev) agents in one step, replacing two shell functions.

**Architecture:** One binary crate. Every herdr invocation goes through `herdr::run`/`herdr::run_text`, which keep herdr's two streams separate and lift its JSON error object into a typed `HerdrError` carrying herdr's own code and message. Commands never print: they return a `Display + Serialize` value and push warnings, and `core::sink` renders both as human text or tagged NDJSON. Only the fields the flow branches on are parsed; the rest of herdr's response rides through as `serde_json::Value`.

**Tech Stack:** Rust 2024, edition-pinned toolchain 1.97.0. `clap` (derive) + `clap-stdin`, `serde`/`serde_json`, `toml`, `thiserror`, `derive_more`, `getter-methods`; `tempfile` as a dev-dependency.

**Estimated total:** 8–12 hours of focused work.

---

## Sources you will need

The spec is `docs/superpowers/specs/2026-07-29-herdr-agent-tools-design.md`. Read it before Task 1 and re-read the relevant section at the top of each task.

Two repositories outside this one are **read-only references**. Never edit them.

- **`~/dev/tmux-team`** — the predecessor. Port from it: `Cargo.toml` (lints and profiles), `src/cmd.rs` (the `Cmd` trait, `AsExitStatus`, `ExitStatus`), `src/core/sink.rs` (the `Sink`), `src/core/text.rs` and `src/core/pane.rs` (validated-string newtypes), `src/cmd/spawn.rs` (the shape of an `*Args` + `Cmd` impl).
- **`~/vendor/herdr`** — herdr's source. `src/cli/spec.rs` is the CLI shape, `src/cli/agent.rs` / `src/cli/pane.rs` / `src/cli/tab.rs` are the hand-written argument parsers that actually run, `src/app/agents.rs` holds the agent-name rule and every error code, `src/detect/manifest.rs` holds `prompt_box_body` / `is_horizontal_rule`, and `docs/next/api/herdr-api.schema.json` is the response schema.

**Nothing outside this repository may be named in a shipped file** — not in code, doc comments, the README, or `CLAUDE.md`/`AGENTS.md`. This plan is the one exception. Do not add a section to any file explaining that convention.

## Facts already verified against herdr's source

Do not re-derive these; they are checked.

| Fact | Value |
| --- | --- |
| herdr CLI output | Always JSON on stdout: `{"id":"…","result":{…}}`. **Except `agent read`, which prints the snapshot as raw text.** |
| herdr CLI failure | Exit 1, stderr `{"id":"…","error":{"code":"…","message":"…"}}`. A client-side refusal (bad kind, missing flag) is exit 2 with a plain-text stderr line. |
| `pane split` result | `{"type":"pane_info","pane":{"pane_id":"…", …}}` |
| `tab create` / `workspace create` result | `{"type":"tab_created","tab":{…},"root_pane":{"pane_id":"…", …}}` |
| `agent start` result | `{"type":"agent_started","agent":{…AgentInfo…},"argv":[…]}` |
| `agent get` result | `{"type":"agent_info","agent":{…AgentInfo…}}` |
| `agent prompt` result | `{"type":"agent_prompted","agent":{…AgentInfo…}}` |
| `AgentInfo` fields | `agent` (kind, nullable), `agent_status`, `pane_id`, `tab_id`, `workspace_id`, `terminal_id`, `cwd`, `focused`, `revision`, `name`, … |
| `agent_status` values | `idle`, `working`, `blocked`, `done`, `unknown` |
| herdr's agent-name rule | First char `a`–`z`; total length ≤ 32 bytes; every later char ASCII lowercase, ASCII digit, `-`, or `_`. Message: `agent name must start with a lowercase letter and contain only lowercase letters, digits, '-' or '_' (1-32 characters)` |
| `pane split` positional | `herdr pane split <PANE_ID> …` is accepted (first arg not starting with `--`) |
| `agent prompt` positional | `args[0]` = target, `args[1]` = text, options from index 2. A prompt starting with `--` is safe — it can never be echoed back as `unknown option`. |
| `agent start` separator | `herdr agent start <NAME> --kind K --pane P -- <agent args…>`. Args after `--` are never echoed in an error. |
| Composer markers | Claude Code `❯` (U+276F), Codex `›` (U+203A) |
| herdr kind labels | `claude`, `codex`, `gemini`, `cursor`, `copilot`, … 21 and growing. **Never hardcode this list.** |

## The RS-030 override, and what the gate will say

`docs/STYLE-GUIDE.md` overrides **RS-030** for the `herdr` module: its operations take the thing they act on as their first parameter and stay free functions. The mechanical gate cannot see that override and will report those functions as violations on a full-tree run.

**Expected RS-030 findings, and only these:** `surface::create_tab`, `surface::create_workspace`, `agent::start` — and possibly `surface::split`, depending on whether `PaneId` ends up with a hand-written `impl` block. Every one is in `src/herdr/`.

**An RS-030 finding anywhere outside `src/herdr/` is a real violation — fix it.** The usual cause is a free function taking `&Sink` as its first parameter; move the sink to the last parameter, which is also the honest ordering (the sink is context, not the subject).

When the pre-commit hook fires on one of the four expected findings, confirm it is one of them, then commit with `--no-verify` and say so in the commit body. The hook only sees staged files, so it will usually stay quiet — the full-tree run in Task 12 is where these surface.

## File structure

```
Cargo.toml                        package, lints, profiles
rust-toolchain.toml               already present — verify only
rustfmt.toml                      already present — verify only
README.md                         what it is, the three commands, exit codes, roadmap
fixtures/composer/*.txt           detection snapshots the composer guard is tested against
src/main.rs                       Cli parse, dispatch, exit-code reporting
src/cmd.rs                        Cmd trait, AsExitStatus, ExitStatus, Failure
src/cmd/spawn.rs                  create surface → agent start → deliver first prompt
src/cmd/prompt.rs                 deliver a prompt to an existing agent; owns `deliver`
src/cmd/presets.rs                list what the config file holds
src/core.rs                       PaneId, NonEmptyText, AgentName; re-exports the sink
src/core/sink.rs                  output seam: human text or tagged NDJSON
src/config.rs                     preset file: schema, discovery, parse
src/herdr.rs                      run, run_text, HerdrError, HerdrRef
src/herdr/surface.rs              pane split / tab create / workspace create
src/herdr/agent.rs                agent start / prompt / get / read
src/harness.rs                    composer occupancy
```

## Task order and in-progress allows

Modules land bottom-up, so a module can be finished and committed before its first caller exists. A module in that state carries **one** inner attribute at the top of its file:

```rust
#![allow(dead_code, reason = "wired into `main` in Task 5")]
```

Each task that wires a module in **deletes that line as a numbered step**. RS-062 permits exactly this, and the gate reports it as a candidate prompt rather than a violation. Do not let one survive past the task named in its reason. Never use a crate-level allow in `main.rs`.

---

## Task 1: Crate scaffolding

**Estimate:** 20 minutes.

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `README.md`
- Verify unchanged: `rust-toolchain.toml`, `rustfmt.toml`, `.gitignore`, `.git/hooks/pre-commit`

- [ ] **Step 1: Confirm the scaffolding already in the repo**

Run:

```bash
cat rust-toolchain.toml rustfmt.toml && ls -l .git/hooks/pre-commit
```

Expected: channel `1.97.0` with `profile = "minimal"` and the `clippy`/`rustfmt` components; `max_width = 120`; an executable `pre-commit`. If the hook is missing:

```bash
cp .claude/skills/rust-style/assets/pre-commit .git/hooks/pre-commit && chmod +x .git/hooks/pre-commit
```

- [ ] **Step 2: Write `Cargo.toml`**

The binary's name is the package name — `herdr-agent-tools`, with no `[[bin]]` override. `mod_module_files = "deny"` is what forces `src/core.rs` over `src/core/mod.rs`.

```toml
[package]
name = "herdr-agent-tools"
version = "0.1.0"
edition = "2024"
rust-version = "1.97"
publish = false

[dependencies]
clap = { version = "4.5", features = ["derive"] }
clap-stdin = "0.8"
derive_more = { version = "2", features = ["from", "display", "as_ref", "deref"] }
getter-methods = "2.0.2"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
thiserror = "2.0"
toml = "1.1"

[dev-dependencies]
tempfile = "3.0"

[lints.rust]
unsafe_code = "deny"

[lints.clippy]
mod_module_files = "deny"
undocumented_unsafe_blocks = "deny"

[profile.release]
lto = "thin"
strip = "symbols"

[profile.profiling]
inherits = "release"
debug = true
strip = "none"
```

- [ ] **Step 3: Write `src/main.rs` as a placeholder that compiles**

```rust
//! `herdr-agent-tools` — launch and prompt herdr agents from one command.

fn main() -> std::process::ExitCode {
    std::process::ExitCode::SUCCESS
}
```

- [ ] **Step 4: Write `README.md`**

Nothing in it may name a path, project, or config outside this repository.

````markdown
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

`$XDG_CONFIG_HOME/herdr-agent-tools/presets.toml`, falling back to
`~/.config/herdr-agent-tools/presets.toml`. Override the location with `--config` or with
`HERDR_AGENT_TOOLS_CONFIG`.

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
````

- [ ] **Step 5: Verify the crate builds and every check passes**

Run each and read its output:

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
```

Expected: all silent or `ok`; the gate reports `0 violation(s), 0 candidate prompt(s)`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/main.rs README.md
git commit -m "feat: scaffold the herdr-agent-tools binary crate"
```

---

## Task 2: `core` — the shared primitives

**Estimate:** 50 minutes.

Read the spec's "Validate only what herdr won't" table and the style guide's section of the same
name before starting. The three types are deliberately unequal: `AgentName` validates, `NonEmptyText`
validates, and `PaneId` does not validate anything — it is a marker that turns "passed a tab id to
`--pane`" into a compile error.

`~/dev/tmux-team/src/core/text.rs`, `src/core/name.rs`, and `src/core/pane.rs` are the shapes to
port. Ignore their `crate::prelude::*` import — this crate has no prelude, so each file states its
own imports.

**Files:**
- Create: `src/core.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write the failing tests for the three primitives**

Put these at the bottom of `src/core.rs` under a `Tests` header. Write them first; the file will
not compile until Step 3.

```rust
// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pane_id_is_carried_verbatim_in_both_directions() {
        // herdr hands us the id and we hand it back; nothing here parses it. The newtype exists so
        // a tab id cannot be passed where a pane id belongs.
        let pane = PaneId::from("w4:p17");

        assert_eq!(pane.to_string(), "w4:p17");
        assert_eq!(&*pane, "w4:p17");
        assert_eq!(serde_json::to_string(&pane).unwrap(), r#""w4:p17""#);
        assert_eq!(serde_json::from_str::<PaneId>(r#""w4:p17""#).unwrap(), pane);
    }

    #[test]
    fn text_with_content_is_accepted_and_preserved() {
        for text in ["x", "  padded  ", "line one\nline two\n", "-", "--flag"] {
            assert_eq!(text.parse::<NonEmptyText>().unwrap().to_string(), text);
        }
    }

    #[test]
    fn blank_prompt_text_is_rejected() {
        // Nothing downstream catches this: herdr's prompt argument has no non-empty constraint, so
        // an empty prompt is a bare Enter delivered into a live agent.
        for text in ["", " ", "\t", "\n\n", "  \n \t "] {
            assert_eq!(text.parse::<NonEmptyText>(), Err(BlankTextError));
        }
    }

    #[test]
    fn agent_names_herdr_accepts_are_accepted_here() {
        for value in ["a", "reviewer", "a1", "with-hyphen_and_9", "x".repeat(32).as_str()] {
            assert_eq!(value.parse::<AgentName>().unwrap().to_string(), value);
        }
    }

    #[test]
    fn agent_names_herdr_refuses_are_refused_here_with_herdrs_own_rule() {
        // This is the one pre-check that duplicates a herdr rule, because herdr's refusal would
        // arrive at `agent start` — after the surface exists. The message names herdr as the
        // authority, and these cases pin the rule read out of herdr's own validator.
        for value in ["", "1abc", "Reviewer", "review er", "review.er", "réviseur", "-lead"] {
            assert!(value.parse::<AgentName>().is_err(), "{value:?} must be refused");
        }
        assert!("x".repeat(33).parse::<AgentName>().is_err(), "33 characters is over herdr's limit");

        assert_eq!(
            "Reviewer".parse::<AgentName>().unwrap_err().to_string(),
            "herdr requires an agent name to start with a lowercase letter and hold only lowercase \
             letters, digits, '-' or '_' (1-32 characters)"
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets`
Expected: FAIL — `src/core.rs` does not exist yet, or `cannot find type PaneId in this scope`.

- [ ] **Step 3: Write `src/core.rs` above the test module**

```rust
//! What every other module shares, and deliberately little.
//!
//! Three primitives and the output seam. This is the only module here with no dependency on the
//! others' vocabulary, which is the property that keeps it a leaf: `herdr` reads into these types
//! and writes from them, `harness` answers about a snapshot without naming any of them, and `cmd`
//! executes against all of it.
//!
//! There is no execution context and no target-resolution layer. herdr resolves agent targets
//! server-side — `agent prompt <target>` takes a pane id or a unique agent name and answers
//! `agent_not_found` or `agent_target_ambiguous` itself — so a context type would earn its keep
//! only by holding a read every command answers from, and there is none. Loading anything eagerly
//! would also be wrong: a malformed preset file would then break `prompt`, which never reads one.

#![allow(dead_code, reason = "the first callers land in Tasks 6 through 11")]

use std::borrow::Cow;
use std::str::FromStr;

use derive_more::{AsRef, Deref, Display, From};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// =====================================================================================================================
// Pane Ids
// =====================================================================================================================

/// A herdr pane id, exactly as herdr spelled it.
///
/// Deliberately unvalidated: herdr hands this out and takes it back, and nothing here ever parses
/// one. The newtype is not guarding against bad input, it prevents *our* mistake. Ids are read out
/// of differently shaped responses — `.result.pane.pane_id` from a split, `.result.root_pane.pane_id`
/// from a tab or a workspace — and handed to `agent start --pane`. Passing a tab id there is a
/// mix-up only this code can make, and this makes it a compile error.
///
/// `#[serde(transparent)]` is honest here for the same reason: with no validation to skip, the
/// wire form *is* the string.
#[derive(Clone, Debug, PartialEq, Eq, Hash, From, Display, AsRef, Deref, Serialize, Deserialize)]
#[from(forward)]
#[as_ref(forward)]
#[deref(forward)]
#[serde(transparent)]
pub struct PaneId(String);

/// Borrows the id where a `String` argument is being assembled.
impl<'a> From<&'a PaneId> for Cow<'a, str> {
    fn from(pane: &'a PaneId) -> Self {
        Cow::Borrowed(pane)
    }
}

// =====================================================================================================================
// Prompt Text
// =====================================================================================================================

/// Non-blank text delivered to an agent.
///
/// The one validation this crate performs that herdr does not: herdr's prompt argument has no
/// non-empty constraint, so an empty prompt is at best a confusing refusal and at worst a bare
/// Enter delivered into a live agent.
///
/// Validated through `FromStr` rather than a clap `value_parser` so the same rule covers both
/// sources `MaybeStdin` resolves — an argv value and a stdin document — which a parser on the raw
/// argument cannot see.
#[derive(Clone, Debug, PartialEq, Eq, Display, AsRef, Deref)]
#[as_ref(forward)]
#[deref(forward)]
pub struct NonEmptyText(String);

impl FromStr for NonEmptyText {
    type Err = BlankTextError;

    /// Accepts the text only when it carries something to deliver.
    ///
    /// # Errors
    ///
    /// Returns [`BlankTextError`] when the text is empty or whitespace-only.
    fn from_str(value: &str) -> Result<Self, BlankTextError> {
        if value.trim().is_empty() {
            Err(BlankTextError)
        } else {
            Ok(Self(value.to_owned()))
        }
    }
}

// =====================================================================================================================
// Agent Names
// =====================================================================================================================

/// An agent's name, pre-checked against the rule herdr will apply.
///
/// This is the stated exception to *validate only what herdr won't*: herdr refuses a bad name at
/// `agent start`, by which point the surface exists, and the precondition property — a rejected
/// command has changed nothing — is worth one restated rule. herdr stays the authority: the rule
/// lives in exactly this one place with a test pinning it, the message says it is herdr's, and
/// `invalid_agent_name` is still propagated if anything slips past.
///
/// The same argument would apply to the agent *kind* and is deliberately not acted on. herdr's kind
/// list lives only in its compile-time argument parser, its request schema types the field as a bare
/// string, and the number of kinds it recognizes demonstrably grows — hardcoding it is exactly the
/// drift the rule warns about.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Display, AsRef, Deref)]
#[as_ref(forward)]
#[deref(forward)]
pub struct AgentName(String);

/// Borrows the name where a `String` argument is being assembled.
impl<'a> From<&'a AgentName> for Cow<'a, str> {
    fn from(name: &'a AgentName) -> Self {
        Cow::Borrowed(name)
    }
}

impl FromStr for AgentName {
    type Err = InvalidAgentName;

    /// Applies herdr's rule: a leading lowercase letter, then lowercase letters, digits, `-`, or
    /// `_`, to at most 32 bytes.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidAgentName`] when the value breaks any part of that rule. The parts are not
    /// distinguished, because herdr does not distinguish them either and a second wording would be
    /// a second authority.
    fn from_str(value: &str) -> Result<Self, InvalidAgentName> {
        let mut characters = value.chars();
        let starts_well = matches!(characters.next(), Some('a'..='z'));
        let rest_is_well_formed = characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '-' | '_')
        });

        if starts_well && rest_is_well_formed && value.len() <= MAX_AGENT_NAME_BYTES {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidAgentName)
        }
    }
}

/// herdr's own limit on an agent name, in bytes — its check is `name.len() <= 32`.
const MAX_AGENT_NAME_BYTES: usize = 32;

// =====================================================================================================================
// Errors
// =====================================================================================================================

/// Rejection of empty or whitespace-only prompt text.
///
/// Carries nothing: the text is the prompt, and a prompt may not reach an error message.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("prompt text must not be blank")]
pub struct BlankTextError;

/// Rejection of an agent name herdr would refuse.
///
/// The message names herdr as the authority, so a reader who disagrees with the rule knows which
/// project to take it up with.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error(
    "herdr requires an agent name to start with a lowercase letter and hold only lowercase letters, \
     digits, '-' or '_' (1-32 characters)"
)]
pub struct InvalidAgentName;
```

The `mod sink;` declaration and its re-export are **not** in this file yet — Task 3 adds them
together with `src/core/sink.rs`.

- [ ] **Step 4: Declare the module in `src/main.rs`**

Replace the whole file:

```rust
//! `herdr-agent-tools` — launch and prompt herdr agents from one command.

mod core;

fn main() -> std::process::ExitCode {
    std::process::ExitCode::SUCCESS
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --all-targets`
Expected: `test result: ok. 5 passed`.

- [ ] **Step 6: Run every check**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
```

Expected: silent; the gate reports `0 violation(s)` and one RS-062 candidate prompt for the
in-progress allow in `core.rs`. That prompt is correct and expected.

- [ ] **Step 7: Commit**

```bash
git add src/core.rs src/main.rs
git commit -m "feat(core): add the PaneId, NonEmptyText, and AgentName primitives"
```

---

## Task 3: `core::sink` — the output seam

**Estimate:** 50 minutes.

Read the style guide's "Output goes through the sink". `~/dev/tmux-team/src/core/sink.rs` is the
shape to port, with **one deliberate difference**: that sink nests a result under a `data` key and
builds its JSON with `serde_json::json!`, which sorts keys alphabetically. This one flattens the
value into the tagged object, which is what the spec's wire example shows:

```json
{"type":"result","placement":"tab","delivered":true,"agent":{…}}
```

Flattening is done with a `#[serde(flatten)]` field on a small generic struct, not by hand-building
a map — hand-assembling a wire shape is RS-021. Field order is declaration order, so `type` and
`status` come first. This has been verified against `serde_json`.

A result and a failure both flatten, so both must serialize as a JSON object. A warning is a
sentence, so the sink shapes that one itself as `{"type":"warning","message":"…"}`.

**Files:**
- Create: `src/core/sink.rs`
- Modify: `src/core.rs`

- [ ] **Step 1: Write the failing tests**

Put these at the bottom of `src/core/sink.rs` under a `Tests` header.

```rust
// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    /// A writer the test can read back, since `Sink` owns its writers.
    #[derive(Clone, Default)]
    struct SharedBuf(Rc<RefCell<Vec<u8>>>);

    impl SharedBuf {
        fn contents(&self) -> String {
            String::from_utf8(self.0.borrow().clone()).unwrap()
        }
    }

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[derive(Serialize)]
    struct Spawned {
        placement: &'static str,
        delivered: bool,
    }

    impl Display for Spawned {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "reviewer (claude) → w4:p17")
        }
    }

    fn spawned() -> Spawned {
        Spawned { placement: "tab", delivered: true }
    }

    /// A sink writing into two buffers the test keeps handles on.
    fn sink(mode: OutputMode) -> (Sink, SharedBuf, SharedBuf) {
        let out = SharedBuf::default();
        let err = SharedBuf::default();
        let sink = Sink::with_writers(mode, Box::new(out.clone()), Box::new(err.clone()));
        (sink, out, err)
    }

    #[test]
    fn human_mode_writes_the_display_form_to_the_matching_stream() {
        let (sink, out, err) = sink(OutputMode::Human);

        sink.out(&spawned());
        assert_eq!(out.contents(), "reviewer (claude) → w4:p17\n");
        assert_eq!(err.contents(), "");

        sink.warn("delivery could not be verified");
        assert_eq!(err.contents(), "delivery could not be verified\n");
    }

    #[test]
    fn json_mode_flattens_the_value_into_the_tagged_object() {
        // The tag and the status lead, then the value's own fields in declaration order — the shape
        // the design pins. Nesting under a `data` key would make a consumer unwrap every line.
        let (sink, out, err) = sink(OutputMode::Json);

        sink.out(&spawned());

        assert_eq!(
            out.contents(),
            "{\"type\":\"result\",\"placement\":\"tab\",\"delivered\":true}\n"
        );
        assert_eq!(err.contents(), "", "json mode puts everything on stdout");
    }

    #[test]
    fn json_mode_puts_warnings_on_the_result_stream_with_their_own_tag() {
        // One ordered NDJSON document: a consumer cannot rely on two streams' relative ordering
        // once either is redirected, and the tag carries what the stream choice would have said.
        let (sink, out, err) = sink(OutputMode::Json);

        sink.warn("composer could not be located");

        assert_eq!(
            out.contents(),
            "{\"type\":\"warning\",\"message\":\"composer could not be located\"}\n"
        );
        assert_eq!(err.contents(), "");
    }

    #[test]
    fn a_failure_renders_as_a_line_for_humans_and_a_tagged_object_carrying_its_status() {
        #[derive(Serialize)]
        struct Failure {
            message: &'static str,
            herdr: Herdr,
        }
        #[derive(Serialize)]
        struct Herdr {
            command: &'static str,
            code: &'static str,
        }
        impl Display for Failure {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.message)
            }
        }
        let failure = Failure {
            message: "agent target pane w4:p16 is not an available shell",
            herdr: Herdr { command: "agent start", code: "agent_pane_busy" },
        };

        let (human, _out, err) = sink(OutputMode::Human);
        human.error(&failure, 5);
        assert_eq!(err.contents(), "agent target pane w4:p16 is not an available shell\n");

        let (json, out, _err) = sink(OutputMode::Json);
        json.error(&failure, 5);
        assert_eq!(
            out.contents(),
            "{\"type\":\"error\",\"status\":5,\
             \"message\":\"agent target pane w4:p16 is not an available shell\",\
             \"herdr\":{\"command\":\"agent start\",\"code\":\"agent_pane_busy\"}}\n"
        );
    }

    #[test]
    fn json_mode_emits_one_self_contained_object_per_line() {
        let (sink, out, _err) = sink(OutputMode::Json);

        sink.warn("first");
        sink.out(&spawned());

        assert_eq!(out.contents().lines().count(), 2);
        for line in out.contents().lines() {
            serde_json::from_str::<serde_json::Value>(line).expect("each line parses alone");
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets`
Expected: FAIL — `cannot find function with_writers`, or the file does not compile.

- [ ] **Step 3: Write `src/core/sink.rs` above the test module**

```rust
//! The output seam: every command result and every diagnostic passes through here.
//!
//! Commands do not print. They return a value and push diagnostics, and this renders both — as
//! human text or as tagged NDJSON — so the `--json` contract lives in one place rather than at
//! every write site.

use std::cell::RefCell;
use std::fmt::Display;
use std::io::{self, Write};

use serde::Serialize;

// =====================================================================================================================
// OutputMode
// =====================================================================================================================

/// How a [`Sink`] renders what it is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputMode {
    /// One line of `Display` text per value; results to stdout, diagnostics to stderr.
    Human,
    /// Tagged NDJSON, everything on stdout — see [`Sink`].
    Json,
}

// =====================================================================================================================
// Sink
// =====================================================================================================================

/// Where a command's output goes, and in which form.
///
/// A concrete struct rather than a trait: [`Sink::out`] is generic over `Display + Serialize`, and
/// a generic method is not object-safe, so a `Box<dyn Sink>` would not compile. Owning the writers
/// is also what makes output assertable in tests.
///
/// Writes are best-effort. A failed write to stdout — a closed pipe, say — is swallowed rather than
/// panicking, which is what `println!` would do.
pub struct Sink {
    mode: OutputMode,
    out: RefCell<Box<dyn Write>>,
    err: RefCell<Box<dyn Write>>,
}

impl Sink {
    /// A sink over the process's own streams.
    ///
    /// In [`OutputMode::Json`] both handles are stdout: a machine consumer reading two streams
    /// cannot rely on their relative ordering once either is redirected, and the `type` tag already
    /// carries what the stream choice would have.
    pub fn new(mode: OutputMode) -> Self {
        let err: Box<dyn Write> = match mode {
            OutputMode::Human => Box::new(io::stderr()),
            OutputMode::Json => Box::new(io::stdout()),
        };
        Self::with_writers(mode, Box::new(io::stdout()), err)
    }

    /// A sink over caller-supplied writers.
    pub fn with_writers(mode: OutputMode, out: Box<dyn Write>, err: Box<dyn Write>) -> Self {
        Self {
            mode,
            out: RefCell::new(out),
            err: RefCell::new(err),
        }
    }

    /// Emits a command's result.
    ///
    /// In [`OutputMode::Json`] the value's own fields are flattened into the tagged object, so a
    /// consumer reads `line["placement"]` rather than `line["data"]["placement"]`. That requires
    /// the value to serialize as a JSON object, which every command result does.
    pub fn out<T: Display + Serialize + ?Sized>(&self, value: &T) {
        self.emit(&self.out, "result", None, value);
    }

    /// Emits a diagnostic — a warning on a path that still succeeds.
    ///
    /// Takes a string rather than a value, because a warning *is* a sentence: there is no
    /// structure under it worth flattening, and the three warnings this crate emits say the
    /// delivery was unverified or the composer could not be read. Separate from a `Result`'s `Err`
    /// so a warning does not have to be spelled as a failure — it does not decide the exit status.
    ///
    /// In [`OutputMode::Json`] this goes to the *result* stream, so the whole run is one ordered
    /// NDJSON document, and the `type` tag is what separates a warning from a result.
    pub fn warn(&self, message: &str) {
        self.emit(self.diagnostic_stream(), "warning", None, &Warning { message });
    }

    /// Emits a command failure, carrying the exit status it maps to.
    ///
    /// The status rides in the wire form so a consumer that already has the line does not also
    /// have to read `$?`. It is passed in rather than read off the value because the value is a
    /// rendering of the failure, and the exit status belongs to the process.
    pub fn error<T: Display + Serialize + ?Sized>(&self, value: &T, status: u8) {
        self.emit(self.diagnostic_stream(), "error", Some(status), value);
    }

    /// The stream diagnostics go to: stderr for a human, the single stdout stream for a machine.
    fn diagnostic_stream(&self) -> &RefCell<Box<dyn Write>> {
        match self.mode {
            OutputMode::Human => &self.err,
            OutputMode::Json => &self.out,
        }
    }

    /// Renders one value onto one stream, in this sink's mode.
    fn emit<T: Display + Serialize + ?Sized>(
        &self,
        stream: &RefCell<Box<dyn Write>>,
        tag: &'static str,
        status: Option<u8>,
        value: &T,
    ) {
        match self.mode {
            OutputMode::Human => write_line(stream, value),
            OutputMode::Json => match serde_json::to_string(&Tagged { tag, status, value }) {
                Ok(line) => write_line(stream, &line),
                // Only reachable for a value that does not serialize as a JSON object, which the
                // flatten in `Tagged` requires. Reported as a line rather than swallowed, because a
                // consumer reading NDJSON would otherwise see a silently missing record.
                Err(error) => write_line(stream, &format!(r#"{{"type":"error","status":1,"message":"{error}"}}"#)),
            },
        }
    }
}

// =====================================================================================================================
// Wire Forms
// =====================================================================================================================

/// One NDJSON line: the tag, the exit status where there is one, then the value's own fields.
///
/// `#[serde(flatten)]` rather than a hand-built `serde_json::Map`, so the wire shape stays
/// described by derives (RS-021) and field order stays declaration order.
#[derive(Serialize)]
struct Tagged<'a, T: Serialize + ?Sized> {
    #[serde(rename = "type")]
    tag: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u8>,
    #[serde(flatten)]
    value: &'a T,
}

/// A warning's wire form — the one output the sink shapes itself, since a warning is a sentence.
#[derive(Serialize)]
struct Warning<'a> {
    message: &'a str,
}

impl Display for Warning<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// Writes one rendered line to a stream and flushes it.
///
/// The single place an output failure is swallowed, and the only place it can be justified: the
/// sink *is* the reporting channel, so there is nowhere to report a failed write to, and the
/// command whose result this is has already succeeded. `println!` would panic on the same closed
/// pipe.
fn write_line<T: Display + ?Sized>(stream: &RefCell<Box<dyn Write>>, line: &T) {
    let mut stream = stream.borrow_mut();
    let _ = writeln!(stream, "{line}");
    let _ = stream.flush();
}
```

- [ ] **Step 4: Wire the module into `src/core.rs`**

Add immediately below the `#![allow(…)]` line and above the `use std::borrow::Cow;` line:

```rust
mod sink;

pub use sink::{OutputMode, Sink};
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --all-targets`
Expected: `test result: ok. 10 passed`.

If the flatten test fails on key order, do **not** switch to `serde_json::json!` — read the failure,
which will show whichever field order serde produced, and check the field order in `Tagged`.

- [ ] **Step 6: Run every check**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
```

Expected: silent; the gate reports `0 violation(s)` plus the RS-062 prompt for `core.rs`'s allow and
two RS-061 prompts for the two `let _ =` lines in `write_line`. Those two are correct: the doc
comment above them states why the write failure is ignored.

- [ ] **Step 7: Commit**

```bash
git add src/core.rs src/core/sink.rs
git commit -m "feat(core): add the output sink and its two wire forms"
```

---

## Task 4: `cmd` — the exit-status contract

**Estimate:** 40 minutes.

Read the style guide's "Exit-status contract". Port `~/dev/tmux-team/src/cmd.rs`, minus its
`Context` and `metadata_keys`, which this crate has no use for.

The one addition over the predecessor is `Failure`. That sink takes a failure as a `&str`, because
its error types wrap non-`Serialize` sources; this one has to carry herdr's command and code nested
under a `herdr` key, so the boundary in `main` converts a failure into a small type that *is*
`Display + Serialize`. `Failure` and `AsExitStatus::herdr` are what make that conversion possible
without every error type deriving `Serialize`.

`HerdrRef` does not exist yet — it arrives with the seam in Task 7, and nothing here may be a
placeholder for it. So this task ships `AsExitStatus` with only `exit_status`, and `Failure` with
only its message field; **Task 7 adds the `herdr` method and the nested field** at the same time as
`HerdrRef` itself.

**Files:**
- Create: `src/cmd.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write the failing tests**

At the bottom of `src/cmd.rs` under a `Tests` header:

```rust
// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("no preset named opus")]
    struct NoSuchPreset;

    impl AsExitStatus for NoSuchPreset {
        fn exit_status(&self) -> ExitStatus {
            ExitStatus::NotFound
        }
    }

    #[test]
    fn the_contracted_codes_are_the_numbers_a_caller_branches_on() {
        // Pinned as numbers, not as variants: the whole point of the contract is that a caller can
        // branch on `$?` without parsing stderr, so a renumbering must fail here.
        assert_eq!(u8::from(ExitStatus::Success), 0);
        assert_eq!(u8::from(ExitStatus::Failure), 1);
        assert_eq!(u8::from(ExitStatus::Usage), 2);
        assert_eq!(u8::from(ExitStatus::NotFound), 3);
        assert_eq!(u8::from(ExitStatus::Conflict), 5);
    }

    #[test]
    fn a_failure_renders_its_errors_display_form_in_both_modes() {
        let failure = Failure::new(&NoSuchPreset);

        assert_eq!(failure.to_string(), "no preset named opus");
        assert_eq!(
            serde_json::to_string(&failure).unwrap(),
            r#"{"message":"no preset named opus"}"#
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets`
Expected: FAIL — `src/cmd.rs` is not declared, or `cannot find type ExitStatus`.

- [ ] **Step 3: Write `src/cmd.rs` above the test module**

```rust
//! The executable subcommands, their side-effect ordering, and the exit-status contract.
//!
//! There is deliberately no parent grouping: three sibling commands share no distinction a parent
//! would mark.

#![allow(dead_code, reason = "the commands land in Tasks 6, 10, and 11")]

use std::fmt::Display;

use serde::Serialize;

use crate::core::Sink;

// =====================================================================================================================
// Command
// =====================================================================================================================

/// One executable subcommand, implemented directly by its clap `*Args` struct.
///
/// The parser shape and the command are one type on purpose: `#[arg]` fields parse straight into
/// domain types, so a bad value fails at parse time and nothing downstream re-validates.
pub trait Cmd {
    /// What this command produces on success — rendered by the sink, never printed here.
    ///
    /// `Display` and `Serialize` are independent impls rather than one derived from the other,
    /// because the human and wire forms genuinely differ: `spawn` prints one line and serializes a
    /// record with herdr's whole agent object nested inside it.
    type Ok: Display + Serialize;

    /// This command's failure, which knows its own exit status.
    type Err: AsExitStatus;

    /// Checks preconditions and runs the flow, returning what it produced.
    ///
    /// Takes the sink rather than returning its diagnostics, because a warning is not a failure and
    /// does not decide the exit status — a command that warns and succeeds still returns `Ok`.
    ///
    /// # Errors
    ///
    /// Returns [`Self::Err`] describing the first failed step. Read-only precondition checks run
    /// first, so a precondition failure has changed nothing in herdr. Where a step fails *after* a
    /// surface exists, the surface is deliberately left open and its pane id reported.
    fn execute(self, sink: &Sink) -> Result<Self::Ok, Self::Err>;
}

// =====================================================================================================================
// Exit Statuses
// =====================================================================================================================

/// A command failure that knows its place in the exit-status contract.
pub trait AsExitStatus: std::error::Error {
    /// The exit status the process reports for this failure.
    fn exit_status(&self) -> ExitStatus;
}

/// A command that cannot fail. The body matches on an uninhabited type, which is the honest way to
/// write "there is no value here to map".
impl AsExitStatus for std::convert::Infallible {
    fn exit_status(&self) -> ExitStatus {
        match *self {}
    }
}

/// The process exit-status contract: stable codes a caller can branch on without parsing stderr.
///
/// clap owns code 2 for argument-syntax rejections; [`ExitStatus::Usage`] extends the same meaning
/// to arguments that parse but cannot be honored. There is no `ErrorCode` enum beside this one —
/// the exit status *is* the machine-readable classification, and it comes from the value a command
/// returns, never from a diagnostic pushed along the way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitStatus {
    /// The command succeeded.
    Success = 0,
    /// A failure with no more specific code below.
    Failure = 1,
    /// The arguments were rejected.
    Usage = 2,
    /// A resource named by an argument does not exist.
    NotFound = 3,
    /// The operation collides with state the target already holds and may succeed on a retry once
    /// that state changes — a composer with unsent text, a pane that is not yet an available shell.
    Conflict = 5,
}

/// The one place the `#[repr(u8)]` discriminant is read as a number, so every other caller — the
/// process exit code, the sink's JSON `status` field — goes through a conversion rather than its
/// own cast.
impl From<ExitStatus> for u8 {
    fn from(status: ExitStatus) -> Self {
        status as Self
    }
}

impl From<ExitStatus> for std::process::ExitCode {
    fn from(status: ExitStatus) -> Self {
        Self::from(u8::from(status))
    }
}

// =====================================================================================================================
// Failure
// =====================================================================================================================

/// A command failure in the shape the sink renders.
///
/// Error types wrap `io::Error` and `serde_json::Error`, neither of which is `Serialize`, so
/// deriving it on them would cascade hand-written impls through unrelated types. They already carry
/// `Display`, which is all the wire form needs — this is where that rendering happens, once, at the
/// boundary in `main`.
#[derive(Debug, Serialize)]
pub struct Failure {
    /// The failure's own `Display` form, verbatim. A herdr failure's `Display` *is* herdr's
    /// message, and nothing re-words it.
    message: String,
}

impl Failure {
    /// Renders a command failure for the sink.
    pub fn new<E: AsExitStatus + ?Sized>(error: &E) -> Self {
        Self { message: error.to_string() }
    }
}

impl Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
```

- [ ] **Step 4: Declare the module in `src/main.rs`**

```rust
//! `herdr-agent-tools` — launch and prompt herdr agents from one command.

mod cmd;
mod core;

fn main() -> std::process::ExitCode {
    std::process::ExitCode::SUCCESS
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --all-targets`
Expected: `test result: ok. 12 passed`.

- [ ] **Step 6: Run every check, then commit**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
git add src/cmd.rs src/main.rs
git commit -m "feat(cmd): add the Cmd trait and the exit-status contract"
```

Expected from the gate: `0 violation(s)`, with RS-062 prompts for the in-progress allows.

---

## Task 5: `config` — the preset file

**Estimate:** 1 hour.

Read the spec's "Preset configuration". This is the only disk I/O in the crate, which is the
boundary the module names.

Two things the shell version got wrong and this must not: its TOML reader failed **open**, so
malformed input yielded an empty document and exit 0, and one typo surfaced as "preset not found".
A Rust TOML parser fails closed, which is the point. And a preset's `args` must never reach an error
message — `UnknownPreset` lists preset **names** only. Printing args in the `presets` *result* is
fine; the ban is on error messages, diagnostics, and logs.

**Files:**
- Create: `src/config.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write the failing tests**

```rust
// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const SAMPLE: &str = "\
default = 'reviewer'

[presets.reviewer]
kind = 'claude'
args = ['--model', 'opus']

[presets.cheap]
kind = 'codex'
";

    /// A preset file in a temp directory the test owns.
    fn written(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("presets.toml");
        fs::write(&path, contents).unwrap();
        (directory, path)
    }

    #[test]
    fn a_presets_name_is_its_table_key_and_args_default_to_none() {
        // The name is the key, so a duplicate name is inexpressible rather than last-one-wins, and
        // there is no `name` field that could disagree with it.
        let (_directory, path) = written(SAMPLE);

        let presets = Presets::load(Some(&path)).unwrap();

        assert_eq!(presets.default_name(), "reviewer");
        assert_eq!(presets.resolve(None).unwrap().kind(), "claude");
        assert_eq!(presets.resolve(None).unwrap().args(), &["--model", "opus"]);
        assert_eq!(presets.resolve(Some("cheap")).unwrap().kind(), "codex");
        assert!(presets.resolve(Some("cheap")).unwrap().args().is_empty());
    }

    #[test]
    fn an_unknown_preset_lists_the_names_the_file_holds_and_never_their_args() {
        // Names only: a preset's arguments must not reach an error message.
        let (_directory, path) = written(SAMPLE);
        let presets = Presets::load(Some(&path)).unwrap();

        let error = presets.resolve(Some("opus")).unwrap_err();

        assert_eq!(error.exit_status_hint(), ExitStatus::NotFound);
        assert_eq!(error.to_string(), "no preset named opus; the file holds: cheap, reviewer");
        assert!(!error.to_string().contains("--model"), "a preset's args must not be logged");
    }

    #[test]
    fn a_file_with_no_default_is_a_parse_failure_rather_than_a_silent_empty_document() {
        let (_directory, path) = written("[presets.only]\nkind = 'claude'\n");

        let error = Presets::load(Some(&path)).unwrap_err();

        assert!(matches!(error, ConfigError::Malformed { .. }), "got {error:?}");
        assert!(error.to_string().contains("missing field `default`"));
    }

    #[test]
    fn malformed_toml_fails_closed_and_names_the_parse_error() {
        // The shell version's reader failed open: malformed input yielded an empty document and
        // exit 0, so one typo surfaced as "preset not found" and sent you hunting the wrong file.
        let (_directory, path) = written("default = ");

        let error = Presets::load(Some(&path)).unwrap_err();

        assert_eq!(error.exit_status_hint(), ExitStatus::Failure);
        assert!(error.to_string().contains("is not valid TOML"), "got {error}");
    }

    #[test]
    fn a_missing_file_reports_where_it_looked_and_what_to_put_there() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("absent.toml");

        let error = Presets::load(Some(&path)).unwrap_err();

        assert_eq!(error.exit_status_hint(), ExitStatus::NotFound);
        assert!(error.to_string().contains("[presets.reviewer]"), "the message carries the example");
    }

    #[test]
    fn the_search_order_is_explicit_then_the_environment_then_xdg_then_home() {
        let explicit = PathBuf::from("/explicit/presets.toml");

        assert_eq!(
            discover(Some(&explicit), Some("/env".as_ref()), Some("/xdg".as_ref()), Some("/home".as_ref())),
            Some(explicit.clone())
        );
        assert_eq!(
            discover(None, Some("/env/p.toml".as_ref()), Some("/xdg".as_ref()), Some("/home".as_ref())),
            Some(PathBuf::from("/env/p.toml"))
        );
        assert_eq!(
            discover(None, None, Some("/xdg".as_ref()), Some("/home".as_ref())),
            Some(PathBuf::from("/xdg/herdr-agent-tools/presets.toml"))
        );
        assert_eq!(
            discover(None, None, None, Some("/home".as_ref())),
            Some(PathBuf::from("/home/.config/herdr-agent-tools/presets.toml"))
        );
        assert_eq!(discover(None, None, None, None), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets`
Expected: FAIL — `src/config.rs` is not declared.

- [ ] **Step 3: Write `src/config.rs` above the test module**

```rust
//! The preset file: its schema, where it is found, and how it is read.
//!
//! The only disk I/O in the crate, which is the boundary this module names. Nothing here is loaded
//! eagerly: `prompt` never reads a preset, and a malformed file must not break it.

#![allow(dead_code, reason = "wired into `presets` in Task 6 and `spawn` in Task 11")]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use getter_methods::Getters;
use serde::Deserialize;
use thiserror::Error;

use crate::cmd::ExitStatus;

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// The environment variable that overrides where the preset file is looked for.
const PATH_VARIABLE: &str = "HERDR_AGENT_TOOLS_CONFIG";

/// Where the file sits under a config directory.
///
/// This tool's own directory, not a second file inside herdr's: a public tool should not squat a
/// filename in another project's config directory, where it would break the day that project
/// claims the name.
const RELATIVE_PATH: &str = "herdr-agent-tools/presets.toml";

/// What a working preset file looks like, quoted back when none was found.
const EXAMPLE: &str = "\
default = 'reviewer'

[presets.reviewer]
kind = 'claude'
args = ['--model', 'opus']

[presets.cheap]
kind = 'codex'
args = ['--model', 'gpt-5-low', '--no-alt-screen']";

// =====================================================================================================================
// Presets
// =====================================================================================================================

/// The whole preset file.
#[derive(Debug, Deserialize)]
pub struct Presets {
    /// The preset `spawn` uses when `--preset` is absent. Required: a file with no default is a
    /// file that cannot answer the common case, and failing on it beats a confusing refusal later.
    default: String,
    /// Every preset, keyed by name.
    ///
    /// A `BTreeMap` so the `presets` listing and an error's list of available names both come out
    /// in a stable order without a sort at each site.
    presets: BTreeMap<String, Preset>,
}

/// One preset: which agent to start, and the flags it usually gets.
#[derive(Debug, Deserialize, Getters)]
pub struct Preset {
    /// The agent kind, passed to `agent start --kind` untouched.
    ///
    /// A plain `String` deliberately. herdr answers `unsupported_agent_kind` from its own
    /// compile-time list, that list is not published anywhere machine-readable, and the number of
    /// kinds it recognizes grows — so restating it here would drift.
    kind: String,
    /// The flags appended after `--`, as an argument vector.
    ///
    /// An array only. A string form would have to be split into shell words, which means
    /// reimplementing shell word-splitting for a value handed straight to `Command`; herdr takes
    /// the agent's arguments as an argument vector, so nothing here needs shell quoting.
    #[serde(default)]
    args: Vec<String>,
}

impl Presets {
    /// Reads the preset file, looking where [`discover`] says to look.
    ///
    /// # Errors
    ///
    /// [`ConfigError::NoConfigDir`] when there is nowhere to look, [`ConfigError::Missing`] when
    /// the file is not there, [`ConfigError::Unreadable`] when it cannot be opened, and
    /// [`ConfigError::Malformed`] when it is not valid TOML or is missing `default`.
    pub fn load(explicit: Option<&Path>) -> Result<Self, ConfigError> {
        let environment = std::env::var_os(PATH_VARIABLE);
        let xdg = std::env::var_os("XDG_CONFIG_HOME");
        let home = std::env::var_os("HOME");
        let path = discover(explicit, environment.as_deref(), xdg.as_deref(), home.as_deref())
            .ok_or(ConfigError::NoConfigDir)?;

        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(ConfigError::Missing { path });
            }
            Err(source) => return Err(ConfigError::Unreadable { path, source }),
        };

        toml::from_str(&contents).map_err(|source| ConfigError::Malformed { path, source })
    }

    /// The preset a bare `spawn` uses.
    pub fn default_name(&self) -> &str {
        &self.default
    }

    /// Looks a preset up, falling back to the file's `default`.
    ///
    /// # Errors
    ///
    /// [`ConfigError::UnknownPreset`], listing the names the file holds — names only, because a
    /// preset's arguments must not reach an error message.
    pub fn resolve(&self, name: Option<&str>) -> Result<&Preset, ConfigError> {
        let name = name.unwrap_or(&self.default);
        self.presets.get(name).ok_or_else(|| ConfigError::UnknownPreset {
            name: name.to_owned(),
            available: self.presets.keys().cloned().collect::<Vec<String>>().join(", "),
        })
    }

    /// Every preset in name order, for the `presets` listing.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Preset)> {
        self.presets.iter().map(|(name, preset)| (name.as_str(), preset))
    }
}

// =====================================================================================================================
// Errors
// =====================================================================================================================

/// Why the preset file could not be read or did not hold what was asked for.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// No `--config`, no environment override, and no config or home directory to fall back on.
    #[error("no config directory to look for a preset file in; pass --config <PATH>")]
    NoConfigDir,
    /// The file is not there. The message carries a working example, since the usual cause is that
    /// it was never written.
    #[error("no preset file at {}; create it with:\n\n{EXAMPLE}", path.display())]
    Missing {
        /// Where it was looked for.
        path: PathBuf,
    },
    /// The file is there but could not be opened.
    #[error("cannot read {}: {source}", path.display())]
    Unreadable {
        /// The file that could not be opened.
        path: PathBuf,
        /// Why not.
        #[source]
        source: std::io::Error,
    },
    /// The file is not valid TOML, or is valid TOML that does not describe presets.
    #[error("{} is not valid TOML: {source}", path.display())]
    Malformed {
        /// The file that would not parse.
        path: PathBuf,
        /// The parse failure, verbatim.
        #[source]
        source: toml::de::Error,
    },
    /// `--preset` named something the file does not hold.
    #[error("no preset named {name}; the file holds: {available}")]
    UnknownPreset {
        /// The name that was asked for.
        name: String,
        /// The names the file holds, comma-separated. Names only — never their arguments.
        available: String,
    },
}

impl ConfigError {
    /// The exit status this failure maps to.
    ///
    /// A plain method rather than an [`AsExitStatus`](crate::cmd::AsExitStatus) impl, because two
    /// commands wrap this in enums of their own and both delegate here — one owner for the mapping,
    /// reachable from either.
    pub fn exit_status_hint(&self) -> ExitStatus {
        match self {
            Self::Missing { .. } | Self::UnknownPreset { .. } => ExitStatus::NotFound,
            Self::NoConfigDir | Self::Unreadable { .. } | Self::Malformed { .. } => ExitStatus::Failure,
        }
    }
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// Where the preset file is looked for, in order: `--config`, the environment override,
/// `$XDG_CONFIG_HOME`, then `~/.config`.
///
/// Pure, with the environment passed in, so the search order is testable without setting process
/// environment variables that leak between tests.
fn discover(explicit: Option<&Path>, environment: Option<&OsStr>, xdg: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path.to_owned());
    }
    if let Some(path) = environment {
        return Some(PathBuf::from(path));
    }
    if let Some(directory) = xdg {
        return Some(Path::new(directory).join(RELATIVE_PATH));
    }
    home.map(|directory| Path::new(directory).join(".config").join(RELATIVE_PATH))
}
```

- [ ] **Step 4: Declare the module in `src/main.rs`**

Add `mod config;` above `mod core;`, keeping the list alphabetical.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --all-targets`
Expected: `test result: ok. 18 passed`.

Two likely stumbles. `Getters` may name the generated methods differently from `kind()`/`args()` —
read the derive's output in the compiler error and adjust the *test*, not the field names. And
`ConfigError::Malformed`'s message for a missing `default` comes from `toml`, which words it
`missing field \`default\``; if the wording differs on the resolved version, update the assertion to
whatever `toml` actually says.

- [ ] **Step 6: Run every check, then commit**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
git add src/config.rs src/main.rs
git commit -m "feat(config): read the preset file, failing closed on malformed TOML"
```

---

## Task 6: `presets` and the CLI spine

**Estimate:** 1 hour.

The first end-to-end slice: clap parses, a command runs, the sink renders, and the process exits
with a contracted code. `presets` touches herdr not at all, so it still works with no server
running — which is exactly what is wanted when the config file itself is what is being debugged.

`~/dev/tmux-team/src/cli.rs` is the shape for `main`'s dispatch and its blanket `Run` impl, minus
the context load.

**Files:**
- Create: `src/cmd/presets.rs`
- Modify: `src/cmd.rs`, `src/main.rs`

- [ ] **Step 1: Write the failing tests for the listing's two forms**

At the bottom of `src/cmd/presets.rs`:

```rust
// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn listing() -> PresetList {
        PresetList {
            default: "reviewer".to_owned(),
            presets: vec![
                PresetLine {
                    name: "cheap".to_owned(),
                    kind: "codex".to_owned(),
                    args: vec!["--model".to_owned(), "gpt-5-low".to_owned()],
                    default: false,
                },
                PresetLine {
                    name: "reviewer".to_owned(),
                    kind: "claude".to_owned(),
                    args: vec!["--model".to_owned(), "opus".to_owned()],
                    default: true,
                },
            ],
        }
    }

    #[test]
    fn the_human_listing_is_one_line_per_preset_and_marks_the_default() {
        // The one command whose result is a list, so the one place a result spans several lines.
        assert_eq!(
            listing().to_string(),
            "cheap (codex) --model gpt-5-low\n\
             reviewer (claude) --model opus  [default]"
        );
    }

    #[test]
    fn the_wire_listing_names_the_default_once_and_marks_it_on_the_preset() {
        assert_eq!(
            serde_json::to_string(&listing()).unwrap(),
            r#"{"default":"reviewer","presets":[{"name":"cheap","kind":"codex","args":["--model","gpt-5-low"],"default":false},{"name":"reviewer","kind":"claude","args":["--model","opus"],"default":true}]}"#
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets`
Expected: FAIL — `cannot find type PresetList`.

- [ ] **Step 3: Write `src/cmd/presets.rs` above the test module**

```rust
//! `presets` — list what the preset file holds.

use std::fmt::Display;
use std::path::PathBuf;

use clap::Args;
use serde::Serialize;

use crate::cmd::{AsExitStatus, Cmd, ExitStatus};
use crate::config::{ConfigError, Presets};
use crate::core::Sink;

// =====================================================================================================================
// Presets Args
// =====================================================================================================================

/// List the presets the config file holds, marking the default.
///
/// Touches herdr not at all, so it answers with no server running — which is what is wanted when
/// the config file itself is what is being debugged.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  \
    herdr-agent-tools presets\n  \
    herdr-agent-tools presets --config ./presets.toml --json")]
pub struct PresetsArgs {
    /// Read this preset file instead of the one in the config directory.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

impl Cmd for PresetsArgs {
    type Ok = PresetList;
    type Err = ConfigError;

    fn execute(self, _sink: &Sink) -> Result<Self::Ok, Self::Err> {
        let presets = Presets::load(self.config.as_deref())?;
        Ok(PresetList {
            default: presets.default_name().to_owned(),
            presets: presets
                .iter()
                .map(|(name, preset)| PresetLine {
                    name: name.to_owned(),
                    kind: preset.kind().to_owned(),
                    args: preset.args().to_vec(),
                    default: name == presets.default_name(),
                })
                .collect(),
        })
    }
}

impl AsExitStatus for ConfigError {
    fn exit_status(&self) -> ExitStatus {
        self.exit_status_hint()
    }
}

// =====================================================================================================================
// Output
// =====================================================================================================================

/// Every preset the file holds, in name order.
#[derive(Debug, Serialize)]
pub struct PresetList {
    /// The name of the preset a bare `spawn` uses.
    default: String,
    /// One entry per preset.
    presets: Vec<PresetLine>,
}

/// One preset as the listing reports it.
#[derive(Debug, Serialize)]
struct PresetLine {
    /// The preset's name — its table key in the file.
    name: String,
    /// The agent kind it starts.
    kind: String,
    /// The flags it appends. Safe to render here: the ban is on error messages and logs, and a
    /// listing of the config file is the one place these are the answer.
    args: Vec<String>,
    /// Whether this is the file's `default`.
    default: bool,
}

/// One line per preset, which makes this the one result that spans several lines.
///
/// The sink's usual contract is one line per value; a listing has nothing else it could honestly
/// be, and the `--json` form is one object either way.
impl Display for PresetList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, preset) in self.presets.iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            write!(f, "{} ({})", preset.name, preset.kind)?;
            for argument in &preset.args {
                write!(f, " {argument}")?;
            }
            if preset.default {
                write!(f, "  [default]")?;
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Declare the command in `src/cmd.rs`**

Add at the top of the file, below the module doc comment and above the `#![allow(…)]`:

```rust
mod presets;

pub use presets::PresetsArgs;
```

Then **delete** the `#![allow(dead_code, reason = "the commands land in Tasks 6, 10, and 11")]` line from `src/cmd.rs`
— `Cmd`, `AsExitStatus`, `ExitStatus`, and `Failure` all have callers after Step 5.

- [ ] **Step 5: Write `src/main.rs` in full**

```rust
//! `herdr-agent-tools` — launch and prompt herdr agents from one command.
//!
//! Parses and dispatches; no command logic lives here. There is no `cli` module: with three
//! commands the dispatch match is a handful of lines, and a file holding only module declarations
//! plus that match would name no boundary.

mod cmd;
mod config;
mod core;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::cmd::{AsExitStatus, Cmd, ExitStatus, Failure, PresetsArgs};
use crate::core::{OutputMode, Sink};

// =====================================================================================================================
// Cli
// =====================================================================================================================

#[derive(Debug, Parser)]
#[command(
    name = "herdr-agent-tools",
    version,
    about = "Launch and prompt herdr agents from one command",
    long_about = "Launch and prompt herdr agents from one command.\n\
        \n\
        herdr starts an agent only in a pane that already exists and is sitting at an interactive \
        shell prompt, so launching one by hand is two steps. `spawn` does both: it creates a pane, \
        tab, or workspace, reads back the new pane's id, and starts a preset-configured agent in \
        it. `prompt` delivers text to an agent that already exists, and `presets` lists what the \
        config file holds.\n\
        \n\
        Run `herdr-agent-tools <command> --help` for details and examples.",
    after_help = "Exit codes:\n  \
        0  success\n  \
        1  general failure\n  \
        2  usage error (bad arguments)\n  \
        3  resource not found\n  \
        5  conflict, retryable (a composer holding unsent text, a pane that is not yet a shell)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Emit machine-readable NDJSON on stdout instead of human-readable text.
    #[arg(long, global = true)]
    json: bool,
}

/// The subcommand set. Variants carry no doc comments deliberately: each command's help is owned by
/// its `*Args` struct, next to the flags it documents.
#[derive(Debug, Subcommand)]
enum Command {
    Presets(PresetsArgs),
}

// =====================================================================================================================
// Entry Point
// =====================================================================================================================

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mode = if cli.json { OutputMode::Json } else { OutputMode::Human };
    // Built before any command runs, so a failure that happens before one starts renders under the
    // same contract as everything else.
    let sink = Sink::new(mode);

    match cli.command {
        Command::Presets(args) => run(args, &sink),
    }
}

/// Runs one command and maps its outcome to the process exit code.
///
/// The sink comes last: it is the context a command reports through, not the thing the command acts
/// on.
fn run<C: Cmd>(command: C, sink: &Sink) -> ExitCode {
    match command.execute(sink) {
        Ok(value) => {
            sink.out(&value);
            ExitStatus::Success.into()
        }
        Err(error) => {
            let status = error.exit_status();
            sink.error(&Failure::new(&error), status.into());
            status.into()
        }
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --all-targets`
Expected: `test result: ok. 20 passed`.

- [ ] **Step 7: Run the binary and read its output**

```bash
printf "default = 'reviewer'\n\n[presets.reviewer]\nkind = 'claude'\nargs = ['--model', 'opus']\n" > /tmp/hat-presets.toml
cargo run -q -- presets --config /tmp/hat-presets.toml
cargo run -q -- presets --config /tmp/hat-presets.toml --json
cargo run -q -- presets --config /tmp/absent.toml; echo "exit=$?"
```

Expected:

```
reviewer (claude) --model opus  [default]
{"type":"result","default":"reviewer","presets":[{"name":"reviewer","kind":"claude","args":["--model","opus"],"default":true}]}
no preset file at /tmp/absent.toml; create it with:
… the example …
exit=3
```

- [ ] **Step 8: Run every check, then commit**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
git add src/cmd.rs src/cmd/presets.rs src/main.rs
git commit -m "feat(presets): list the preset file through the CLI spine"
```

---

## Task 7: The herdr seam

**Estimate:** 1 hour 15 minutes.

Read the spec's "The herdr seam" and the style guide's section of the same name. Two rules carry the
whole task.

**The streams must stay separate.** herdr writes JSON results to stdout and a JSON error object to
stderr with exit 1. Capturing with `2>&1` works right up until herdr writes anything at all to
stderr on an otherwise successful call — a deprecation notice, a reconnect warning — at which point
the JSON is preceded by prose and the parse dies, and the caller reports a missing pane id for a
surface that was actually created.

**`HerdrError` carries herdr's command names, never its arguments.** The argument to `agent prompt`
*is* the prompt text. `command_name` takes the first two argv words and nothing else, which is
exactly the `<group> <subcommand>` pair every call in this crate is built as.

There is deliberately **no macro** here, and the style guide says why: a macro over a seam earns its
place when it batches operations or patches a check a third-party builder omits, and neither applies.

**Files:**
- Create: `src/herdr.rs`
- Modify: `src/cmd.rs`, `src/main.rs`

- [ ] **Step 1: Write the failing tests**

Everything testable here is pure: the command name, the stderr classification, and the exit-status
mapping. **No test invokes herdr.**

```rust
// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn a_command_name_is_the_group_and_the_subcommand_and_never_an_argument() {
        // The third argv word is the target, and for `agent prompt` the fourth is the prompt text.
        // Neither may reach an error message, so the name stops at two words.
        assert_eq!(command_name(&args(&["agent", "prompt", "reviewer", "ship it"])), "agent prompt");
        assert_eq!(command_name(&args(&["pane", "split", "w4:p1", "--cwd", "/w"])), "pane split");
        assert_eq!(command_name(&args(&["agent", "list"])), "agent list");
        assert_eq!(command_name(&args(&["agent"])), "agent");
        assert_eq!(command_name(&[]), "");
    }

    #[test]
    fn a_json_error_object_becomes_a_refusal_carrying_herdrs_code_and_message_verbatim() {
        let stderr = br#"{"id":"cli:agent:start","error":{"code":"agent_pane_busy","message":"agent target pane w4:p16 is not an available shell"}}"#;

        let error = classify("agent start".to_owned(), stderr);

        assert_eq!(error.code(), Some("agent_pane_busy"));
        // The Display form *is* herdr's message; nothing re-words it.
        assert_eq!(error.to_string(), "agent target pane w4:p16 is not an available shell");
    }

    #[test]
    fn stderr_that_is_not_a_json_error_object_becomes_a_plain_failure_naming_the_command() {
        // herdr refuses some things client-side with a plain line and exit 2 — an unsupported kind,
        // a missing flag. Those carry no code, so they map to a general failure.
        let error = classify("agent start".to_owned(), b"unsupported interactive agent kind: clawd\n");

        assert_eq!(error.code(), None);
        assert_eq!(error.to_string(), "herdr agent start failed: unsupported interactive agent kind: clawd");
        assert_eq!(error.exit_status(), ExitStatus::Failure);
    }

    #[test]
    fn empty_stderr_still_produces_a_message() {
        let error = classify("pane split".to_owned(), b"");

        assert_eq!(error.to_string(), "herdr pane split failed: unknown error");
    }

    #[test]
    fn only_the_codes_with_a_meaningful_answer_leave_the_general_failure_default() {
        for (code, expected) in [
            ("agent_target_ambiguous", ExitStatus::Usage),
            ("agent_not_found", ExitStatus::NotFound),
            ("agent_pane_not_found", ExitStatus::NotFound),
            ("agent_pane_busy", ExitStatus::Conflict),
            ("agent_prompt_stalled", ExitStatus::Conflict),
            ("agent_name_taken", ExitStatus::Conflict),
            // Anything herdr grows later is a general failure, not a compile error.
            ("agent_launch_pending", ExitStatus::Failure),
            ("something_herdr_added_last_week", ExitStatus::Failure),
        ] {
            let error = HerdrError::Refused {
                command: "agent start".to_owned(),
                code: code.to_owned(),
                message: "…".to_owned(),
            };
            assert_eq!(error.exit_status(), expected, "{code}");
        }
    }

    #[test]
    fn a_reference_nests_herdrs_command_and_code_so_a_consumer_can_tell_them_from_ours() {
        let error = HerdrError::Refused {
            command: "agent start".to_owned(),
            code: "agent_pane_busy".to_owned(),
            message: "…".to_owned(),
        };

        assert_eq!(
            serde_json::to_string(&error.reference()).unwrap(),
            r#"{"command":"agent start","code":"agent_pane_busy"}"#
        );

        let plain = HerdrError::Failed { command: "pane split".to_owned(), message: "…".to_owned() };
        assert_eq!(
            serde_json::to_string(&plain.reference()).unwrap(),
            r#"{"command":"pane split"}"#
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets`
Expected: FAIL — `src/herdr.rs` is not declared.

- [ ] **Step 3: Write `src/herdr.rs` above the test module**

```rust
//! Every interaction with herdr, and the seam that runs them.
//!
//! No module outside this one spawns a process or names the `herdr` binary. The module root carries
//! [`run`], [`run_text`], and the stream discipline both depend on; `surface` owns the three ways to
//! make a pane and `agent` owns what a command does to an agent in one.

#![allow(dead_code, reason = "the submodules arrive in Task 8 and their callers in Tasks 10 and 11")]

use std::process::Command;

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::cmd::ExitStatus;

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// The binary every call in this crate runs. Named in exactly one place.
const BINARY: &str = "herdr";

// =====================================================================================================================
// Run
// =====================================================================================================================

/// Runs one herdr command and deserializes its `result` into `T`.
///
/// # Errors
///
/// Returns a [`HerdrError`]: [`Spawn`](HerdrError::Spawn) when herdr cannot be launched,
/// [`Refused`](HerdrError::Refused) when herdr answered with its own error object,
/// [`Failed`](HerdrError::Failed) when it exited non-zero without one, and
/// [`Unreadable`](HerdrError::Unreadable) when it succeeded but printed something `T` could not be
/// read from.
pub fn run<T: DeserializeOwned>(args: &[String]) -> Result<T, HerdrError> {
    let stdout = run_text(args)?;
    serde_json::from_str::<Envelope<T>>(&stdout)
        .map(|envelope| envelope.result)
        .map_err(|source| HerdrError::Unreadable {
            command: command_name(args),
            source,
        })
}

/// Runs one herdr command and returns its stdout verbatim.
///
/// `agent read` is the reason this exists: it prints the terminal snapshot as raw text rather than
/// as JSON, so the composer guard needs the bytes rather than a parse. Everything else goes through
/// [`run`], which is this plus a deserialize.
///
/// The two streams are captured **separately**. Merging them works right up until herdr writes
/// anything at all to stderr on an otherwise successful call — a deprecation notice, a reconnect
/// warning — at which point the JSON is preceded by prose and the parse dies, and the caller reports
/// a missing pane id for a surface that was actually created.
///
/// # Errors
///
/// As [`run`], minus the deserialize.
pub fn run_text(args: &[String]) -> Result<String, HerdrError> {
    let output = Command::new(BINARY)
        .args(args)
        .output()
        .map_err(|source| HerdrError::Spawn {
            command: command_name(args),
            source,
        })?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(classify(command_name(args), &output.stderr))
    }
}

/// herdr's response envelope. Only `result` is read: `id` is the request id this crate set and has
/// nothing to say back.
#[derive(serde::Deserialize)]
struct Envelope<T> {
    result: T,
}

// =====================================================================================================================
// Errors
// =====================================================================================================================

/// Failure of a herdr invocation.
///
/// Every variant carries the herdr command's **name** and never its arguments — the argument to
/// `agent prompt` is the prompt text, and a preset's arguments ride on `agent start`.
#[derive(Debug, Error)]
pub enum HerdrError {
    /// herdr could not be launched — not installed, or not on `PATH`.
    #[error("failed to run herdr {command}: {source}")]
    Spawn {
        /// The herdr command that could not be launched.
        command: String,
        /// Why not.
        #[source]
        source: std::io::Error,
    },
    /// herdr answered with its own error object. The `Display` form *is* herdr's message.
    #[error("{message}")]
    Refused {
        /// The herdr command that was refused.
        command: String,
        /// herdr's own code, carried verbatim.
        code: String,
        /// herdr's own message, carried verbatim and never re-worded.
        message: String,
    },
    /// herdr exited non-zero without an error object — a client-side refusal, which it reports as a
    /// plain line with exit 2.
    ///
    /// The message is stderr's **first line only**. That line can never hold a prompt or a preset's
    /// arguments: herdr takes the prompt positionally at index 1 before it starts reading options,
    /// and agent arguments live after `--`, past everything its parser echoes.
    #[error("herdr {command} failed: {message}")]
    Failed {
        /// The herdr command that failed.
        command: String,
        /// herdr's first line of stderr.
        message: String,
    },
    /// herdr succeeded but printed something this call could not read.
    ///
    /// Deliberately does not quote the output. `agent read`'s output is someone's terminal, and one
    /// variant that sometimes carries terminal content is one variant too many.
    #[error("herdr {command} printed output this build could not read: {source}")]
    Unreadable {
        /// The herdr command whose output could not be read.
        command: String,
        /// The parse failure.
        #[source]
        source: serde_json::Error,
    },
}

impl HerdrError {
    /// herdr's own code, when herdr supplied one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Refused { code, .. } => Some(code),
            Self::Spawn { .. } | Self::Failed { .. } | Self::Unreadable { .. } => None,
        }
    }

    /// The exit status this failure maps to.
    ///
    /// Matches on herdr's own code and maps only the codes with a meaningful non-`1` answer; an
    /// unrecognized code is a general failure, not a compile error. herdr grows codes, and a match
    /// that had to be exhaustive over them would be a second copy of herdr's vocabulary.
    ///
    /// A plain method rather than an [`AsExitStatus`](crate::cmd::AsExitStatus) impl, because every
    /// command wraps this in an enum of its own and all of them delegate here.
    pub fn exit_status(&self) -> ExitStatus {
        match self.code() {
            Some("agent_target_ambiguous") => ExitStatus::Usage,
            Some("agent_not_found" | "agent_pane_not_found") => ExitStatus::NotFound,
            Some("agent_pane_busy" | "agent_prompt_stalled" | "agent_name_taken") => ExitStatus::Conflict,
            Some(_) | None => ExitStatus::Failure,
        }
    }

    /// herdr's command and code, for nesting inside this crate's own error envelope.
    ///
    /// Nested rather than emitted flat, so a consumer can still tell our failures from herdr's.
    pub fn reference(&self) -> HerdrRef {
        HerdrRef {
            command: self.command().to_owned(),
            code: self.code().map(ToOwned::to_owned),
        }
    }

    /// The herdr command's name, which every variant carries.
    fn command(&self) -> &str {
        match self {
            Self::Spawn { command, .. }
            | Self::Refused { command, .. }
            | Self::Failed { command, .. }
            | Self::Unreadable { command, .. } => command,
        }
    }
}

/// herdr's provenance for a failure, nested under a `herdr` key in the wire form.
#[derive(Clone, Debug, Serialize)]
pub struct HerdrRef {
    /// The herdr command's name — never its arguments.
    command: String,
    /// herdr's own code, absent when herdr supplied none.
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// The herdr command's name: the group and the subcommand, and nothing after them.
///
/// Every call in this crate is built as `<group> <subcommand> [target] [args…]`, so two words is
/// the whole name — and stopping there is what keeps a target, a prompt, and a preset's arguments
/// out of every error message this module produces.
fn command_name(args: &[String]) -> String {
    args.iter().take(2).cloned().collect::<Vec<String>>().join(" ")
}

/// Reads herdr's stderr into a typed failure.
///
/// herdr's error object is `{"error":{"code":…,"message":…}}`; anything else is a client-side
/// refusal it printed as a plain line.
fn classify(command: String, stderr: &[u8]) -> HerdrError {
    #[derive(serde::Deserialize)]
    struct Reported {
        error: Body,
    }
    #[derive(serde::Deserialize)]
    struct Body {
        code: String,
        message: String,
    }

    let text = String::from_utf8_lossy(stderr);
    match serde_json::from_str::<Reported>(text.trim()) {
        Ok(reported) => HerdrError::Refused {
            command,
            code: reported.error.code,
            message: reported.error.message,
        },
        // RS-002: the parse failure says nothing useful about a line that was never JSON, and the
        // replacement carries strictly more — herdr's own words.
        Err(_) => HerdrError::Failed {
            command,
            message: first_line(&text),
        },
    }
}

/// herdr's first line of stderr, for a one-line error message.
fn first_line(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("unknown error")
        .to_owned()
}
```

`classify`'s `Err(_)` discards the parse failure deliberately, and the site comment above it is what
RS-002 requires: the replacement captures strictly more useful data — herdr's actual stderr line —
than "expected value at line 1 column 1" would.

- [ ] **Step 4: Add `AsExitStatus::herdr` and `Failure`'s nested field in `src/cmd.rs`**

Add the import `use crate::herdr::HerdrRef;`, then extend the trait:

```rust
pub trait AsExitStatus: std::error::Error {
    /// The exit status the process reports for this failure.
    fn exit_status(&self) -> ExitStatus;

    /// herdr's own command and code, when this failure came from herdr.
    ///
    /// Defaulted to `None`, because most failures are this crate's own. A command's error enum
    /// forwards this from whichever variant wraps a [`HerdrError`](crate::herdr::HerdrError).
    fn herdr(&self) -> Option<HerdrRef> {
        None
    }
}
```

and the failure:

```rust
#[derive(Debug, Serialize)]
pub struct Failure {
    /// The failure's own `Display` form, verbatim.
    message: String,
    /// herdr's command and code, when the failure came from herdr.
    #[serde(skip_serializing_if = "Option::is_none")]
    herdr: Option<HerdrRef>,
}

impl Failure {
    /// Renders a command failure for the sink.
    pub fn new<E: AsExitStatus + ?Sized>(error: &E) -> Self {
        Self {
            message: error.to_string(),
            herdr: error.herdr(),
        }
    }
}
```

- [ ] **Step 5: Extend the `Failure` test in `src/cmd.rs`**

Add to the existing test module:

```rust
    #[derive(Debug, thiserror::Error)]
    #[error("agent target pane w4:p16 is not an available shell")]
    struct PaneBusy;

    impl AsExitStatus for PaneBusy {
        fn exit_status(&self) -> ExitStatus {
            ExitStatus::Conflict
        }

        fn herdr(&self) -> Option<HerdrRef> {
            crate::herdr::HerdrError::Refused {
                command: "agent start".to_owned(),
                code: "agent_pane_busy".to_owned(),
                message: "agent target pane w4:p16 is not an available shell".to_owned(),
            }
            .reference()
            .into()
        }
    }

    #[test]
    fn a_herdr_failure_nests_herdrs_command_and_code_beside_its_message() {
        assert_eq!(
            serde_json::to_string(&Failure::new(&PaneBusy)).unwrap(),
            r#"{"message":"agent target pane w4:p16 is not an available shell","herdr":{"command":"agent start","code":"agent_pane_busy"}}"#
        );
    }
```

- [ ] **Step 6: Declare the module in `src/main.rs`**

Add `mod herdr;` after `mod core;`.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --all-targets`
Expected: `test result: ok. 27 passed`.

- [ ] **Step 8: Run every check, then commit**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
git add src/herdr.rs src/cmd.rs src/main.rs
git commit -m "feat(herdr): add the process seam and its typed failures"
```

---

## Task 8: `herdr::surface` and `herdr::agent`

**Estimate:** 1 hour 30 minutes.

This is where the seam's correctness actually lives, so **every argument vector is pinned by an
exact-string test**, one per herdr call. Read the spec's "Commands" section for the flags, and
"Propagate rather than restate" for why `AgentRecord` names only three fields.

Two shapes to keep straight:

- The public functions take the thing they act on first and stay free functions — the documented
  RS-030 override. Expect the gate to flag `create_tab`, `create_workspace`, `start`, and possibly
  `split`.
- The private `*_args` builders take `&str`, not the newtypes. The compile-time protection a `PaneId`
  buys belongs at the public boundary; inside, string assembly is string assembly, and `&str`
  parameters keep the builders testable from literals and keep the gate quiet.

**Files:**
- Create: `src/herdr/surface.rs`, `src/herdr/agent.rs`
- Modify: `src/herdr.rs`

- [ ] **Step 1: Write the failing argument-vector tests for `surface`**

```rust
// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_split_names_the_anchor_pane_and_always_passes_a_cwd() {
        // The pane id is positional, which herdr accepts as the first argument that does not start
        // with `--`. `--cwd` is passed in all three modes: a split would otherwise inherit the
        // *pane's* working directory, which stops matching the shell's the moment you `cd`.
        assert_eq!(
            split_args("w4:p1", "/work/repo", Focus::Leave),
            ["pane", "split", "w4:p1", "--direction", "right", "--cwd", "/work/repo", "--no-focus"]
        );
    }

    #[test]
    fn a_tab_and_a_workspace_are_labelled_with_the_agents_name() {
        assert_eq!(
            tab_args("reviewer", "/work/repo", Focus::Leave),
            ["tab", "create", "--cwd", "/work/repo", "--label", "reviewer", "--no-focus"]
        );
        assert_eq!(
            workspace_args("reviewer", "/work/repo", Focus::Leave),
            ["workspace", "create", "--cwd", "/work/repo", "--label", "reviewer", "--no-focus"]
        );
    }

    #[test]
    fn focus_is_always_stated_rather_than_left_to_herdrs_default() {
        // Opt-in: a tool meant to be driven by agents should not steal the human's focus. Stated
        // explicitly in both directions so the argument vector says what it means.
        assert_eq!(split_args("w4:p1", "/w", Focus::Take).last().unwrap(), "--focus");
        assert_eq!(tab_args("reviewer", "/w", Focus::Take).last().unwrap(), "--focus");
        assert_eq!(workspace_args("reviewer", "/w", Focus::Take).last().unwrap(), "--focus");
    }

    #[test]
    fn a_split_reads_its_pane_id_from_a_different_field_than_a_tab_or_a_workspace() {
        // The reason `PaneId` exists: `.result.pane.pane_id` from a split, `.result.root_pane.pane_id`
        // from a tab or a workspace. Both feed `agent start --pane`, and mixing them up is a
        // mistake only this code can make.
        let split: PaneCreated = serde_json::from_str(
            r#"{"type":"pane_info","pane":{"pane_id":"w4:p18","tab_id":"w4:t1"}}"#,
        )
        .unwrap();
        assert_eq!(split.pane.pane_id, PaneId::from("w4:p18"));

        let tab: RootPaneCreated = serde_json::from_str(
            r#"{"type":"tab_created","tab":{"tab_id":"w4:t3"},"root_pane":{"pane_id":"w4:p17"}}"#,
        )
        .unwrap();
        assert_eq!(tab.root_pane.pane_id, PaneId::from("w4:p17"));
    }

    #[test]
    fn a_placement_serializes_as_the_flag_that_chose_it() {
        assert_eq!(serde_json::to_string(&Placement::Pane).unwrap(), r#""pane""#);
        assert_eq!(serde_json::to_string(&Placement::Tab).unwrap(), r#""tab""#);
        assert_eq!(serde_json::to_string(&Placement::Workspace).unwrap(), r#""workspace""#);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets`
Expected: FAIL — `src/herdr/surface.rs` is not declared.

- [ ] **Step 3: Write `src/herdr/surface.rs` above the test module**

```rust
//! The three ways to make a pane for an agent to start in.

use serde::{Deserialize, Serialize};

use crate::core::{AgentName, PaneId};
use crate::herdr::{HerdrError, run};

// =====================================================================================================================
// Placement
// =====================================================================================================================

/// Where a new agent's pane comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// Split the calling pane.
    Pane,
    /// A new tab, whose root pane the agent takes.
    Tab,
    /// A new workspace, whose root pane the agent takes.
    Workspace,
}

/// Whether a created surface takes the user's focus.
///
/// An enum rather than a `bool`, because a bare `true` at a call site says nothing about which way
/// it points, and this flag is inverted relative to the CLI flag that sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// Focus the new surface — `--focus`, which is opt-in.
    Take,
    /// Leave the human where they were — `--no-focus`, the default.
    Leave,
}

impl Focus {
    /// The herdr flag this choice spells. Stated in every argument vector rather than relying on
    /// herdr's default, so the vector says what it means.
    fn flag(self) -> &'static str {
        match self {
            Self::Take => "--focus",
            Self::Leave => "--no-focus",
        }
    }
}

// =====================================================================================================================
// Surfaces
// =====================================================================================================================

/// Splits `pane` and reports the pane that appeared.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn split(pane: &PaneId, cwd: &str, focus: Focus) -> Result<PaneId, HerdrError> {
    let created: PaneCreated = run(&split_args(pane, cwd, focus))?;
    Ok(created.pane.pane_id)
}

/// Creates a tab labelled `label` and reports its root pane.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn create_tab(label: &AgentName, cwd: &str, focus: Focus) -> Result<PaneId, HerdrError> {
    let created: RootPaneCreated = run(&tab_args(label, cwd, focus))?;
    Ok(created.root_pane.pane_id)
}

/// Creates a workspace labelled `label` and reports its root pane.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn create_workspace(label: &AgentName, cwd: &str, focus: Focus) -> Result<PaneId, HerdrError> {
    let created: RootPaneCreated = run(&workspace_args(label, cwd, focus))?;
    Ok(created.root_pane.pane_id)
}

// =====================================================================================================================
// Responses
// =====================================================================================================================

/// `pane split`'s result: the pane it made.
#[derive(Debug, Deserialize)]
struct PaneCreated {
    pane: PaneRef,
}

/// `tab create` and `workspace create`'s result. Both also report the tab, and the workspace form
/// reports the workspace; neither is read here, and serde ignores what it is not asked for.
#[derive(Debug, Deserialize)]
struct RootPaneCreated {
    root_pane: PaneRef,
}

/// The one field either response is read for.
#[derive(Debug, Deserialize)]
struct PaneRef {
    pane_id: PaneId,
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// `herdr pane split <PANE> --direction right --cwd <CWD> --(no-)focus`.
///
/// The anchor is passed as `$HERDR_PANE_ID`, never as herdr's `--current`: that flag resolves
/// server-side to whichever pane is *focused*, which is not this one when the command runs from an
/// unfocused pane.
fn split_args(pane: &str, cwd: &str, focus: Focus) -> Vec<String> {
    [
        "pane",
        "split",
        pane,
        "--direction",
        "right",
        "--cwd",
        cwd,
        focus.flag(),
    ]
    .map(str::to_owned)
    .to_vec()
}

/// `herdr tab create --cwd <CWD> --label <LABEL> --(no-)focus`.
fn tab_args(label: &str, cwd: &str, focus: Focus) -> Vec<String> {
    ["tab", "create", "--cwd", cwd, "--label", label, focus.flag()]
        .map(str::to_owned)
        .to_vec()
}

/// `herdr workspace create --cwd <CWD> --label <LABEL> --(no-)focus`.
fn workspace_args(label: &str, cwd: &str, focus: Focus) -> Vec<String> {
    ["workspace", "create", "--cwd", cwd, "--label", label, focus.flag()]
        .map(str::to_owned)
        .to_vec()
}
```

- [ ] **Step 4: Write the failing argument-vector tests for `agent`**

```rust
// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const STARTED: &str = r#"{"type":"agent_started","argv":["claude"],"agent":{"agent":"claude","agent_status":"working","pane_id":"w4:p17","name":"reviewer","tab_id":"w4:t3","workspace_id":"w4","terminal_id":"term_1","cwd":"/work","focused":false,"revision":7}}"#;

    #[test]
    fn a_start_names_the_kind_and_the_pane_and_puts_preset_args_after_the_separator() {
        // Extra args are appended after the preset's with no merging and no de-duplication, so the
        // agent's own last-flag-wins rules settle any conflict.
        assert_eq!(
            start_args("reviewer", "claude", "w4:p17", &["--model".to_owned(), "opus".to_owned()]),
            ["agent", "start", "reviewer", "--kind", "claude", "--pane", "w4:p17", "--", "--model", "opus"]
        );
    }

    #[test]
    fn a_start_with_no_agent_args_omits_the_separator_entirely() {
        assert_eq!(
            start_args("reviewer", "claude", "w4:p17", &[]),
            ["agent", "start", "reviewer", "--kind", "claude", "--pane", "w4:p17"]
        );
    }

    #[test]
    fn a_prompt_waits_for_the_states_that_prove_delivery() {
        // `--until working` is the load-bearing choice: treat a prompt as delivered when the status
        // has actually moved, never on the exit code. herdr's bare `--wait` waits for the *turn* to
        // finish, which is wrong for a dispatch that should return promptly.
        let wait = Wait { until: vec![WORKING.to_owned()], timeout: 15_000 };
        assert_eq!(
            prompt_args("reviewer", "ship it", Some(&wait)),
            ["agent", "prompt", "reviewer", "ship it", "--wait", "--until", "working", "--timeout", "15000"]
        );
    }

    #[test]
    fn a_prompt_repeats_until_once_per_state() {
        let wait = Wait { until: vec!["idle".to_owned(), "blocked".to_owned()], timeout: 60_000 };
        assert_eq!(
            prompt_args("w4:p17", "go", Some(&wait)),
            ["agent", "prompt", "w4:p17", "go", "--wait", "--until", "idle", "--until", "blocked", "--timeout", "60000"]
        );
    }

    #[test]
    fn no_verify_submits_without_waiting_for_anything() {
        assert_eq!(prompt_args("reviewer", "go", None), ["agent", "prompt", "reviewer", "go"]);
    }

    #[test]
    fn a_read_asks_for_the_detection_snapshot_as_plain_text() {
        // `--source detection` is the plain-text bottom-buffer snapshot herdr's own agent detection
        // reads. It is absent from that subcommand's usage line but accepted.
        assert_eq!(
            read_args("reviewer", 40),
            ["agent", "read", "reviewer", "--source", "detection", "--format", "text", "--lines", "40"]
        );
    }

    #[test]
    fn a_get_is_one_call_for_both_the_kind_and_the_status() {
        assert_eq!(get_args("reviewer"), ["agent", "get", "reviewer"]);
    }

    #[test]
    fn a_record_names_only_the_fields_the_flow_branches_on_and_carries_the_rest_untouched() {
        let started: AgentStarted = serde_json::from_str(STARTED).unwrap();
        let record = started.agent;

        assert_eq!(record.kind(), Some("claude"));
        assert_eq!(record.status(), WORKING);
        assert_eq!(record.pane(), &PaneId::from("w4:p17"));
        assert_eq!(record.name_or_unknown(), "reviewer");

        // Everything else rides through: no field of herdr's is dropped, and none is renamed.
        let wire: serde_json::Value = serde_json::to_value(&record).unwrap();
        assert_eq!(wire["terminal_id"], "term_1");
        assert_eq!(wire["workspace_id"], "w4");
        assert_eq!(wire["cwd"], "/work");
        assert_eq!(wire["revision"], 7);
    }

    #[test]
    fn a_pane_herdr_did_not_name_reads_as_unnamed_and_stays_absent_on_the_wire() {
        let record: AgentRecord =
            serde_json::from_str(r#"{"agent":"claude","agent_status":"idle","pane_id":"w4:p2"}"#).unwrap();

        assert_eq!(record.name_or_unknown(), "(unnamed)");
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"agent":"claude","agent_status":"idle","pane_id":"w4:p2"}"#,
            "herdr omitted the field, so it stays omitted"
        );
    }

    #[test]
    fn a_pane_with_no_agent_reports_no_kind_rather_than_failing_to_parse() {
        let record: AgentRecord =
            serde_json::from_str(r#"{"agent":null,"agent_status":"unknown","pane_id":"w4:p2"}"#).unwrap();

        assert_eq!(record.kind(), None);
        assert_eq!(record.status(), "unknown");
    }

    #[test]
    fn a_status_herdr_adds_later_parses_and_round_trips() {
        // The status stays a `String` deliberately: an enum would either fail to parse a status
        // herdr grew, or re-spell it on the way out. herdr owns this vocabulary.
        let record: AgentRecord =
            serde_json::from_str(r#"{"agent":"claude","agent_status":"compacting","pane_id":"w4:p2"}"#).unwrap();

        assert_eq!(record.status(), "compacting");
        assert_eq!(serde_json::to_value(&record).unwrap()["agent_status"], "compacting");
    }
}
```

- [ ] **Step 5: Write `src/herdr/agent.rs` above the test module**

```rust
//! What a command does to an agent in a pane.

use serde::{Deserialize, Serialize};

use crate::core::{AgentName, NonEmptyText, PaneId};
use crate::herdr::{HerdrError, run, run_text};

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// herdr's status for an agent that is mid-turn.
///
/// One spelling shared by the `--until` this crate passes and the check for the honest gap where an
/// agent was already working before a prompt was sent.
pub const WORKING: &str = "working";

/// How much of the detection snapshot to read.
///
/// More than any composer needs and cheap; only the bottom of the snapshot is used.
pub const COMPOSER_LINES: u32 = 40;

// =====================================================================================================================
// Operations
// =====================================================================================================================

/// Reads an agent's record — one call giving both the harness kind and the current status.
///
/// # Errors
///
/// Returns whatever [`run`] returned; `agent_not_found` and `agent_target_ambiguous` are herdr's
/// answers about the target, resolved server-side.
pub fn get(target: &str) -> Result<AgentRecord, HerdrError> {
    let info: AgentInfo = run(&get_args(target))?;
    Ok(info.agent)
}

/// Reads the plain-text detection snapshot the composer guard inspects.
///
/// Goes through [`run_text`] rather than [`run`] because `agent read` prints the snapshot itself
/// rather than a JSON envelope.
///
/// # Errors
///
/// Returns whatever [`run_text`] returned.
pub fn read(target: &str, lines: u32) -> Result<String, HerdrError> {
    run_text(&read_args(target, lines))
}

/// Starts an agent in an existing pane.
///
/// # Errors
///
/// Returns whatever [`run`] returned. `agent_pane_busy` is the retryable one: the pane exists but
/// its shell has not reached its prompt yet, and the caller retries rather than abandoning the
/// surface.
pub fn start(name: &AgentName, kind: &str, pane: &PaneId, args: &[String]) -> Result<AgentRecord, HerdrError> {
    let started: AgentStarted = run(&start_args(name, kind, pane, args))?;
    Ok(started.agent)
}

/// Submits a prompt, optionally waiting for the states that prove it was delivered.
///
/// # Errors
///
/// Returns whatever [`run`] returned. `agent_prompt_stalled` means herdr observed no state change
/// within its own 5000ms window, and `timeout` means the requested states never arrived.
pub fn prompt(target: &str, text: &NonEmptyText, wait: Option<&Wait>) -> Result<AgentRecord, HerdrError> {
    let prompted: AgentPrompted = run(&prompt_args(target, text, wait))?;
    Ok(prompted.agent)
}

/// The delivery wait attached to a submission.
#[derive(Clone, Debug)]
pub struct Wait {
    /// The states that count as delivered. Each becomes one `--until`.
    pub until: Vec<String>,
    /// Milliseconds before herdr gives up.
    pub timeout: u64,
}

// =====================================================================================================================
// Responses
// =====================================================================================================================

/// An agent as herdr reports it, with only the fields this crate branches on named.
///
/// Everything else rides through as a flattened map and is nested back into the result verbatim.
/// Re-describing herdr's record in a shape of our own would buy a translation layer that has to be
/// revised every time herdr adds a field.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentRecord {
    /// The kind herdr detected, absent for a pane that hosts no agent. Selects the composer marker.
    agent: Option<String>,
    /// The current status, which decides whether delivery is verifiable.
    ///
    /// A `String`, not an enum. herdr owns this vocabulary and grows it; an enum would either fail
    /// to parse a status herdr added or re-spell an unrecognized one on the way out, and both are
    /// worse than carrying the word herdr chose. Exactly one comparison is made against it, to
    /// [`WORKING`].
    agent_status: String,
    /// The pane the agent runs in.
    pane_id: PaneId,
    /// The name herdr recorded, absent for a pane herdr did not name.
    ///
    /// Skipped when absent rather than written as `null`: herdr omits the field for an unnamed
    /// pane, and sending a `null` back where herdr sent nothing would be restating rather than
    /// propagating.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Every other field herdr reported, untouched.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

impl AgentRecord {
    /// The kind herdr detected, or `None` for a pane hosting no agent.
    pub fn kind(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    /// The current status, in herdr's own spelling.
    pub fn status(&self) -> &str {
        &self.agent_status
    }

    /// The pane the agent runs in.
    pub fn pane(&self) -> &PaneId {
        &self.pane_id
    }

    /// The agent's name as herdr recorded it, or a placeholder for a pane herdr did not name.
    ///
    /// One spelling shared by every caller, so a nameless pane reads the same everywhere.
    pub fn name_or_unknown(&self) -> &str {
        self.name.as_deref().unwrap_or("(unnamed)")
    }
}

/// `agent get`'s result.
#[derive(Debug, Deserialize)]
struct AgentInfo {
    agent: AgentRecord,
}

/// `agent start`'s result. It also reports the `argv` it ran, which is not read here.
#[derive(Debug, Deserialize)]
struct AgentStarted {
    agent: AgentRecord,
}

/// `agent prompt`'s result.
#[derive(Debug, Deserialize)]
struct AgentPrompted {
    agent: AgentRecord,
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// `herdr agent get <TARGET>`.
fn get_args(target: &str) -> Vec<String> {
    ["agent", "get", target].map(str::to_owned).to_vec()
}

/// `herdr agent read <TARGET> --source detection --format text --lines <N>`.
fn read_args(target: &str, lines: u32) -> Vec<String> {
    ["agent", "read", target, "--source", "detection", "--format", "text", "--lines"]
        .map(str::to_owned)
        .into_iter()
        .chain([lines.to_string()])
        .collect()
}

/// `herdr agent start <NAME> --kind <KIND> --pane <ID> [-- <ARGS…>]`.
///
/// The separator is omitted when there are no agent arguments, so an empty preset produces the same
/// vector a hand-typed launch would.
fn start_args(name: &str, kind: &str, pane: &str, args: &[String]) -> Vec<String> {
    let mut vector = ["agent", "start", name, "--kind", kind, "--pane", pane]
        .map(str::to_owned)
        .to_vec();
    if !args.is_empty() {
        vector.push("--".to_owned());
        vector.extend(args.iter().cloned());
    }
    vector
}

/// `herdr agent prompt <TARGET> <TEXT> [--wait --until <STATE>… --timeout <MS>]`.
///
/// The text is the second positional, which is where herdr reads it before it starts reading
/// options — so a prompt beginning with `--` is delivered rather than misread.
fn prompt_args(target: &str, text: &str, wait: Option<&Wait>) -> Vec<String> {
    let mut vector = ["agent", "prompt", target, text].map(str::to_owned).to_vec();
    if let Some(wait) = wait {
        vector.push("--wait".to_owned());
        for state in &wait.until {
            vector.push("--until".to_owned());
            vector.push(state.clone());
        }
        vector.push("--timeout".to_owned());
        vector.push(wait.timeout.to_string());
    }
    vector
}
```

- [ ] **Step 6: Declare the submodules in `src/herdr.rs`**

Add below the module doc comment:

```rust
pub mod agent;
pub mod surface;
```

Two `pub mod` declarations rather than a curated `pub use` facade, deliberately: `surface::split`
and `agent::start` read as what they are, and the module path is the qualifier a method receiver
would otherwise supply. That is the stated design RS-035 asks for.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --all-targets`
Expected: `test result: ok. 43 passed`.

`start_args` takes `name: &str` but is called with `&AgentName`; deref coercion handles that. If it
does not compile, pass `name.as_ref()`.

- [ ] **Step 8: Run every check**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
```

**The gate will now report RS-030 findings.** Confirm every one is in `src/herdr/` and names
`create_tab`, `create_workspace`, `start`, or `split`. Anything else is a real violation.

- [ ] **Step 9: Commit**

```bash
git add src/herdr.rs src/herdr/surface.rs src/herdr/agent.rs
git commit --no-verify -m "feat(herdr): add the surface and agent operations

The pre-commit gate reports RS-030 for the free functions in src/herdr/ that
take their subject as the first parameter. docs/STYLE-GUIDE.md states that
override under 'The herdr seam': PaneId and AgentName are core primitives, and
hanging every herdr verb off them would drag the whole seam into core."
```

---

## Task 9: `harness` — the composer guard

**Estimate:** 1 hour 15 minutes.

Read the spec's "The composer guard" and its "Known limitations". This module touches no process
and no pane: it is handed a detection snapshot as `&str` and answers whether the composer holds
text. Process I/O in `harness` is forbidden by the style guide.

The region rule is herdr's own, reimplemented here because only herdr's Claude Code manifest
declares a rule using `prompt_box_body`, so it cannot be read out of `detection-explain` for every
kind. The two functions to mirror are `prompt_box_body` and `is_horizontal_rule` in
`~/vendor/herdr/src/detect/manifest.rs`. **This algorithm has already been prototyped against the
six fixtures below and all six pass** — implement it as written.

Per-harness knowledge is one string: the marker.

- [ ] **Step 1: Write the six fixtures**

Each carries neutral filler above the composer. A snapshot captured from a live pane holds that
pane's whole transcript, and the guard reads only the composer's structure — the rules, the marker,
and whether anything follows it — so the surrounding text is filler by design.

```bash
mkdir -p fixtures/composer

printf '  ⏺ Read src/main.rs (42 lines)\n\n  ⏺ The parser now handles the empty case.\n\n────────────────────────────────────────── repo ──\n❯\n──────────────────────────────────────────────────\n  ⏵⏵ accept edits on                    17k tokens\n' > fixtures/composer/claude-empty.txt

printf '  ⏺ Read src/main.rs (42 lines)\n\n────────────────────────────────────────── repo ──\n❯ wait, before you commit that\n──────────────────────────────────────────────────\n  ⏵⏵ accept edits on                    17k tokens\n' > fixtures/composer/claude-occupied.txt

printf '  • Ran cargo test\n\n──────────────────────────────────────────────────\n› \n──────────────────────────────────────────────────\n  ⌃C quit                              12%% context\n' > fixtures/composer/codex-empty.txt

printf '  • Ran cargo test\n\n──────────────────────────────────────────────────\n› \n  second line of the draft\n──────────────────────────────────────────────────\n  ⌃C quit                              12%% context\n' > fixtures/composer/codex-multiline.txt

printf 'error: could not compile `probe`\n\nwarning: build failed, waiting for other jobs\n$ \n' > fixtures/composer/no-rules.txt

printf '  some transcript line\n\n──────────────────────────────────────────────────\n▸ \n──────────────────────────────────────────────────\n  status line\n' > fixtures/composer/unknown-marker.txt
```

Verify they came out right — the rules must be U+2500 `─`, and `claude-empty.txt` must show a
labelled top border, which is the case `is_horizontal_rule`'s three-character branch exists for:

```bash
for f in fixtures/composer/*.txt; do echo "== $f"; cat "$f"; done
```

- [ ] **Step 2: Write the failing table-driven test**

At the bottom of `src/harness.rs`:

```rust
// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// One fixture, the kind `agent get` reported for it, and the answer it must produce.
    struct Case {
        name: &'static str,
        kind: Option<&'static str>,
        snapshot: &'static str,
        expected: Composer,
    }

    const CASES: [Case; 7] = [
        Case {
            name: "an idle Claude Code composer holds nothing",
            kind: Some("claude"),
            snapshot: include_str!("../fixtures/composer/claude-empty.txt"),
            expected: Composer::Empty,
        },
        Case {
            name: "a half-written line beside the marker is unsent text",
            kind: Some("claude"),
            snapshot: include_str!("../fixtures/composer/claude-occupied.txt"),
            expected: Composer::Occupied,
        },
        Case {
            name: "an idle Codex composer holds nothing",
            kind: Some("codex"),
            snapshot: include_str!("../fixtures/composer/codex-empty.txt"),
            expected: Composer::Empty,
        },
        Case {
            name: "a draft on a later line of the box counts too",
            kind: Some("codex"),
            snapshot: include_str!("../fixtures/composer/codex-multiline.txt"),
            expected: Composer::Occupied,
        },
        Case {
            // The second tier: an unfamiliar kind still gets a check rather than none, by trying
            // every known marker against the first non-empty body line.
            name: "an unknown kind falls back to probing every known marker",
            kind: Some("some-agent-this-build-has-never-heard-of"),
            snapshot: include_str!("../fixtures/composer/codex-empty.txt"),
            expected: Composer::Empty,
        },
        Case {
            name: "a snapshot with fewer than two rules fails open",
            kind: Some("claude"),
            snapshot: include_str!("../fixtures/composer/no-rules.txt"),
            expected: Composer::NoPromptBox,
        },
        Case {
            name: "a box whose marker matches nothing known fails open",
            kind: None,
            snapshot: include_str!("../fixtures/composer/unknown-marker.txt"),
            expected: Composer::UnknownMarker,
        },
    ];

    #[test]
    fn the_guard_answers_each_fixture_the_way_the_design_says() {
        for case in &CASES {
            assert_eq!(composer(case.kind, case.snapshot), case.expected, "{}", case.name);
        }
    }

    #[test]
    fn both_fail_open_answers_carry_a_warning_and_neither_success_answer_does() {
        // Failing open is the whole point: a tool that refused every pane it could not parse would
        // be unusable the first time a harness changed its rendering. Delivered without the
        // guarantee, and said so — never guessed at.
        assert!(Composer::NoPromptBox.warning().is_some());
        assert!(Composer::UnknownMarker.warning().is_some());
        assert!(Composer::Empty.warning().is_none());
        assert!(Composer::Occupied.warning().is_none());
    }

    #[test]
    fn a_labelled_top_border_still_counts_as_a_rule() {
        // Claude Code labels the top of its box (`──── repo ──`), so a rule is a line whose leading
        // run of `─` is either the whole trimmed line or at least three characters long.
        assert!(is_horizontal_rule("─────── repo ──"));
        assert!(is_horizontal_rule("──────────────"));
        assert!(is_horizontal_rule("  ────  "));
        assert!(!is_horizontal_rule("─ x"), "one dash then text is a bullet, not a rule");
        assert!(!is_horizontal_rule("❯ ─────"));
        assert!(!is_horizontal_rule(""));
        assert!(!is_horizontal_rule("   "));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --all-targets`
Expected: FAIL — `src/harness.rs` is not declared.

- [ ] **Step 4: Write `src/harness.rs` above the test module**

```rust
//! What this tool knows about each supported agent CLI, which is one string per kind: its
//! composer's prompt marker.
//!
//! Touches no process and no pane — it is handed a detection snapshot as `&str` and answers whether
//! the composer holds text. The region rule lives here once; each kind supplies only its marker.

#![allow(dead_code, reason = "wired into `prompt` in Task 10")]

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// The composer prompt marker for each kind this build knows, keyed by herdr's own kind label.
///
/// Deliberately short. This is not a registry of supported agents — herdr's kind list has 21
/// entries and grows, and restating it here would drift. It is the per-harness half of one rule,
/// and a kind that is missing from it still gets a check through the probe tier below.
const MARKERS: [(&str, char); 2] = [("claude", '❯'), ("codex", '›')];

// =====================================================================================================================
// Composer
// =====================================================================================================================

/// What a detection snapshot says about the target's composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Composer {
    /// The composer was located and holds nothing.
    Empty,
    /// The composer was located and holds unsent text.
    Occupied,
    /// The composer could not be located: fewer than two horizontal rules in the snapshot.
    NoPromptBox,
    /// A box was located but no known marker matched its first non-empty line.
    UnknownMarker,
}

impl Composer {
    /// The warning this answer carries, for the two that fail open.
    ///
    /// Neither says what the composer holds — that is someone's half-written message.
    pub fn warning(self) -> Option<&'static str> {
        match self {
            Self::NoPromptBox => {
                Some("could not locate the composer in the target's snapshot; delivering unguarded")
            }
            Self::UnknownMarker => {
                Some("the target's composer uses a prompt marker this build does not know; delivering unguarded")
            }
            Self::Empty | Self::Occupied => None,
        }
    }
}

/// Whether the target's composer holds unsent text, read from a detection snapshot.
///
/// `kind` is what `agent get` reported, which selects the marker. Marker selection is two-tiered so
/// an unfamiliar kind still gets a check rather than none: a kind this build holds a marker for uses
/// it, and any other kind falls back to trying every known marker against the first non-empty body
/// line, taking the first that matches.
///
/// **Fails open** in exactly two cases — the body cannot be located, or no known marker matched.
/// Both deliver anyway, with a warning, because a tool that refused every pane it could not parse
/// would be unusable the first time a harness changed its rendering.
///
/// The guard is a snapshot, not a lock: a human can start typing between this read and the
/// submission. Narrowing that window further would need something herdr does not expose.
pub fn composer(kind: Option<&str>, snapshot: &str) -> Composer {
    let lines: Vec<&str> = snapshot.lines().collect();
    let Some(body) = prompt_box_body(&lines) else {
        return Composer::NoPromptBox;
    };
    // A body with no non-empty line at all is a box that is not a composer. Reported as an unknown
    // marker rather than as `Empty`, because no composer was identified and claiming one is empty
    // is a guarantee this did not earn.
    let Some(head) = body.iter().position(|line| !line.trim().is_empty()) else {
        return Composer::UnknownMarker;
    };
    let Some(marker) = marker_for(kind, body[head]) else {
        return Composer::UnknownMarker;
    };

    let leading = body[head].trim_start();
    let after_marker = leading.strip_prefix(marker).unwrap_or(leading);
    let occupied = !after_marker.trim().is_empty()
        || body
            .iter()
            .enumerate()
            .any(|(index, line)| index != head && !line.trim().is_empty());

    if occupied { Composer::Occupied } else { Composer::Empty }
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// The marker to strip: this kind's if the build knows it, otherwise the first known marker the
/// body's leading line actually starts with.
fn marker_for(kind: Option<&str>, head: &str) -> Option<char> {
    if let Some(kind) = kind
        && let Some((_, marker)) = MARKERS.iter().find(|(known, _)| *known == kind)
    {
        return Some(*marker);
    }
    MARKERS
        .iter()
        .find(|(_, marker)| head.trim_start().starts_with(*marker))
        .map(|(_, marker)| *marker)
}

/// The lines between the last two horizontal rules, which is where every harness renders its
/// composer.
///
/// Harness-agnostic by construction, and the same region herdr's own agent detection extracts as
/// `prompt_box_body`.
fn prompt_box_body<'a>(lines: &'a [&'a str]) -> Option<&'a [&'a str]> {
    let mut rules = lines
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, line)| is_horizontal_rule(line))
        .map(|(index, _)| index);
    let bottom = rules.next()?;
    let top = rules.next()?;
    Some(&lines[top + 1..bottom])
}

/// Whether a line is one of the box's borders.
///
/// A rule is a line whose leading run of `─` is either the whole trimmed line or at least three
/// characters long — which is how a labelled top border (`──── repo ──`) still counts as one, while
/// a single dash followed by prose does not.
fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    let rule_characters = trimmed.chars().take_while(|character| *character == '─').count();
    if rule_characters == 0 {
        return false;
    }
    let suffix: String = trimmed.chars().skip(rule_characters).collect();
    suffix.trim_start().is_empty() || rule_characters >= 3
}
```

- [ ] **Step 5: Declare the module in `src/main.rs`**

Add `mod harness;` after `mod core;`, keeping the list alphabetical.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --all-targets`
Expected: `test result: ok. 45 passed`.

If a fixture case fails, print what the guard saw before changing the algorithm — the usual cause is
a fixture written with `-` instead of `─`:

```bash
cargo test --all-targets -- --nocapture the_guard_answers
```

- [ ] **Step 7: Run every check, then commit**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
git add src/harness.rs src/main.rs fixtures/composer
git commit -m "feat(harness): add the composer guard and its fixtures"
```

If the gate reports RS-030 on `composer`, `marker_for`, `prompt_box_body`, or `is_horizontal_rule`,
that is a **real** finding and not the documented override — none of them takes a crate type first.
Re-read the signatures.

---

## Task 10: `prompt`

**Estimate:** 1 hour 30 minutes.

Read the spec's `prompt` section in full. Three things carry it.

**The two skip flags are not interchangeable and neither implies the other.** `--force` skips the
composer guard; `--no-verify` skips the delivery wait.

**`--until working` is finding 2's rule in herdr's own vocabulary:** treat a prompt as delivered when
the status has actually moved, never on the exit code. herdr's bare `--wait` waits for the *turn* to
finish, which is wrong for a dispatch that should return promptly.

**The honest gap:** if the agent was already `working` at step 1, `--until working` matches instantly
and proves nothing. That case succeeds with a warning saying delivery was unverified, rather than
reporting a guarantee it does not have.

This task also owns `deliver`, which `spawn` calls in Task 11 — the first prompt goes through the
same code path as every other. Note the parameter order: **the sink comes last**, or the gate will
report RS-030 against `Sink`.

**Files:**
- Create: `src/cmd/prompt.rs`
- Modify: `src/cmd.rs`, `src/main.rs`

- [ ] **Step 1: Write the failing tests**

The flow itself calls herdr, so what is tested here is what is pure: the wait the flags build, the
result's two forms, and the exit-status mapping.

```rust
// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        args: PromptArgs,
    }

    fn parse(argv: &[&str]) -> PromptArgs {
        Harness::try_parse_from(argv).expect("parses").args
    }

    fn record() -> AgentRecord {
        serde_json::from_str(r#"{"agent":"claude","agent_status":"working","pane_id":"w4:p17"}"#).unwrap()
    }

    #[test]
    fn the_default_wait_proves_delivery_by_the_status_moving_to_working() {
        let args = parse(&["prompt", "reviewer", "ship it"]);

        let wait = args.wait().expect("verification is on by default");
        assert_eq!(wait.until, [WORKING]);
        assert_eq!(wait.timeout, 15_000);
    }

    #[test]
    fn wait_until_replaces_the_states_for_a_caller_that_wants_the_full_settle_wait() {
        let args = parse(&["prompt", "reviewer", "go", "--wait-until", "idle", "--wait-until", "blocked"]);

        let wait = args.wait().expect("still verifying, just for different states");
        assert_eq!(wait.until, ["idle", "blocked"]);
    }

    #[test]
    fn no_verify_skips_the_wait_and_force_does_not() {
        // The two flags are not interchangeable and neither implies the other: --force skips the
        // composer guard, --no-verify skips the delivery wait.
        assert!(parse(&["prompt", "reviewer", "go", "--no-verify"]).wait().is_none());
        assert!(parse(&["prompt", "reviewer", "go", "--force"]).wait().is_some());
        assert!(!parse(&["prompt", "reviewer", "go", "--force"]).guarded());
        assert!(parse(&["prompt", "reviewer", "go", "--no-verify"]).guarded());
    }

    #[test]
    fn the_guard_runs_unless_force_says_otherwise() {
        assert!(parse(&["prompt", "reviewer", "go"]).guarded());
        assert!(!parse(&["prompt", "reviewer", "go", "--force"]).guarded());
    }

    #[test]
    fn a_blank_prompt_is_refused_at_parse_time() {
        assert!(Harness::try_parse_from(["prompt", "reviewer", "   "]).is_err());
    }

    #[test]
    fn a_delivered_prompt_reports_the_pane_and_the_status_it_reached() {
        let delivered = Delivered { delivered: Some(true), agent: record() };

        assert_eq!(delivered.to_string(), "prompted w4:p17 (working)");
        assert_eq!(
            serde_json::to_string(&delivered).unwrap(),
            r#"{"delivered":true,"agent":{"agent":"claude","agent_status":"working","pane_id":"w4:p17"}}"#
        );
    }

    #[test]
    fn an_unverified_delivery_says_so_on_the_wire_rather_than_claiming_a_guarantee() {
        let delivered = Delivered { delivered: Some(false), agent: record() };

        assert_eq!(serde_json::to_value(&delivered).unwrap()["delivered"], false);
    }

    #[test]
    fn a_refusal_and_a_stall_are_both_retryable_conflicts() {
        let occupied = PromptError::ComposerOccupied { target: "reviewer".to_owned() };
        assert_eq!(occupied.exit_status(), ExitStatus::Conflict);
        assert_eq!(
            occupied.to_string(),
            "reviewer's composer holds unsent text; wait for it to clear, or pass --force to send anyway"
        );

        let stalled = PromptError::Stalled { target: "reviewer".to_owned() };
        assert_eq!(stalled.exit_status(), ExitStatus::Conflict);
    }

    #[test]
    fn a_refusal_never_says_what_the_composer_held() {
        // The guard's input is a snapshot of someone's half-written message. The refusal says the
        // composer holds unsent text and never says what that text is — there is no field on this
        // variant that could carry it.
        let occupied = PromptError::ComposerOccupied { target: "reviewer".to_owned() };

        assert!(!occupied.to_string().contains("wait, before you commit"));
    }

    #[test]
    fn a_herdr_failure_forwards_herdrs_own_status_and_provenance() {
        let error = PromptError::Herdr(crate::herdr::HerdrError::Refused {
            command: "agent get".to_owned(),
            code: "agent_not_found".to_owned(),
            message: "agent target reviewer not found".to_owned(),
        });

        assert_eq!(error.exit_status(), ExitStatus::NotFound);
        assert_eq!(error.to_string(), "agent target reviewer not found");
        assert!(error.herdr().is_some());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets`
Expected: FAIL — `cannot find type PromptArgs`.

- [ ] **Step 3: Write `src/cmd/prompt.rs` above the test module**

```rust
//! `prompt` — deliver a prompt to an agent that already exists.

use std::fmt::Display;

use clap::Args;
use clap_stdin::MaybeStdin;
use serde::Serialize;
use thiserror::Error;

use crate::cmd::{AsExitStatus, Cmd, ExitStatus};
use crate::core::{NonEmptyText, Sink};
use crate::harness::{self, Composer};
use crate::herdr::agent::{self, AgentRecord, COMPOSER_LINES, WORKING, Wait};
use crate::herdr::{HerdrError, HerdrRef};

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// How long herdr waits for the states that prove delivery, in milliseconds.
///
/// Long enough for a busy harness to acknowledge input, short enough that a dispatch does not hang
/// on an agent that is never going to answer.
const DEFAULT_TIMEOUT_MS: u64 = 15_000;

// =====================================================================================================================
// Prompt Args
// =====================================================================================================================

/// Deliver a prompt to an agent that already exists.
///
/// The target is a herdr pane id or a unique agent name; herdr resolves it server-side and answers
/// `agent_not_found` or `agent_target_ambiguous` itself.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  \
    herdr-agent-tools prompt reviewer \"run the test suite and report failures\"\n  \
    git diff | herdr-agent-tools prompt reviewer -\n  \
    herdr-agent-tools prompt w4:p17 \"go\" --wait-until idle --timeout 120000")]
pub struct PromptArgs {
    /// The agent to prompt: a herdr pane id, or a unique agent name.
    target: String,

    /// The prompt text; `-` reads it from stdin (e.g. a heredoc or a pipe).
    text: MaybeStdin<NonEmptyText>,

    /// Wait for these states instead of `working`; repeat for more than one.
    #[arg(long, value_name = "STATE")]
    wait_until: Vec<String>,

    /// Milliseconds to wait for delivery before giving up.
    #[arg(long, value_name = "MS", default_value_t = DEFAULT_TIMEOUT_MS)]
    timeout: u64,

    /// Submit without waiting for proof that the agent received it.
    #[arg(long)]
    no_verify: bool,

    /// Send even if the target's composer holds unsent text.
    #[arg(long)]
    force: bool,
}

impl PromptArgs {
    /// The delivery wait these flags build, or `None` under `--no-verify`.
    ///
    /// `--until working` is the load-bearing default: it returns as soon as delivery is proven,
    /// where herdr's bare `--wait` would wait for the whole turn to finish.
    fn wait(&self) -> Option<Wait> {
        if self.no_verify {
            return None;
        }
        let until = if self.wait_until.is_empty() {
            vec![WORKING.to_owned()]
        } else {
            self.wait_until.clone()
        };
        Some(Wait { until, timeout: self.timeout })
    }

    /// Whether the composer guard runs.
    fn guarded(&self) -> bool {
        !self.force
    }
}

impl Cmd for PromptArgs {
    type Ok = Delivered;
    type Err = PromptError;

    /// One `agent get`, then the guard, then the submission.
    ///
    /// The guard runs before anything is sent, so a refusal has changed nothing.
    fn execute(self, sink: &Sink) -> Result<Self::Ok, Self::Err> {
        let before = agent::get(&self.target)?;

        if self.guarded() {
            let snapshot = agent::read(&self.target, COMPOSER_LINES)?;
            match harness::composer(before.kind(), &snapshot) {
                Composer::Occupied => {
                    return Err(PromptError::ComposerOccupied { target: self.target });
                }
                // Failing open: delivered without the guarantee, and said so.
                answer => {
                    if let Some(warning) = answer.warning() {
                        sink.warn(warning);
                    }
                }
            }
        }

        // The honest gap: an agent that was already `working` matches `--until working` instantly,
        // which proves nothing. Reported as unverified rather than as a guarantee this did not earn.
        let wait = self.wait();
        let verified = wait.is_some() && before.status() != WORKING;
        if wait.is_some() && !verified {
            sink.warn(&format!(
                "{} was already working, so delivery could not be verified",
                self.target
            ));
        }

        let agent = deliver(&self.target, &self.text, wait.as_ref(), sink)?;
        Ok(Delivered {
            delivered: Some(verified),
            agent,
        })
    }
}

// =====================================================================================================================
// Delivery
// =====================================================================================================================

/// Submits a prompt, re-sending once if herdr reports it did not land.
///
/// Shared with `spawn`, whose first prompt goes through exactly this path. The sink comes last: it
/// is the channel a warning is reported through, not the thing being acted on.
///
/// On `agent_prompt_stalled` or `timeout` the prompt is re-sent once — finding 2 was that a prompt
/// sent within a few seconds of starting an agent is silently swallowed, and waiting and re-sending
/// worked in every observed case. A second failure is a retryable conflict rather than a silent
/// success.
///
/// # Errors
///
/// [`PromptError::Herdr`] for anything herdr refused outright, and [`PromptError::Stalled`] when
/// two submissions both failed to move the agent.
pub(super) fn deliver(
    target: &str,
    text: &NonEmptyText,
    wait: Option<&Wait>,
    sink: &Sink,
) -> Result<AgentRecord, PromptError> {
    match agent::prompt(target, text, wait) {
        Ok(agent) => Ok(agent),
        Err(error) if undelivered(&error) => {
            sink.warn(&format!("{target} did not acknowledge the prompt; re-sending once"));
            agent::prompt(target, text, wait).map_err(|error| {
                if undelivered(&error) {
                    PromptError::Stalled { target: target.to_owned() }
                } else {
                    PromptError::Herdr(error)
                }
            })
        }
        Err(error) => Err(PromptError::Herdr(error)),
    }
}

/// Whether herdr is saying the submission did not move the agent, rather than that it refused it.
fn undelivered(error: &HerdrError) -> bool {
    matches!(error.code(), Some("agent_prompt_stalled" | "timeout"))
}

// =====================================================================================================================
// Output
// =====================================================================================================================

/// What `prompt` produced: whether delivery was proven, and herdr's record of the agent.
#[derive(Debug, Serialize)]
pub struct Delivered {
    /// Whether the wait actually proved delivery.
    ///
    /// `false` under `--no-verify`, and `false` for the honest gap: an agent that was already
    /// `working` matches `--until working` instantly, which proves nothing. Absent when there was
    /// no prompt to deliver, which is `spawn`'s case.
    #[serde(skip_serializing_if = "Option::is_none")]
    delivered: Option<bool>,
    /// herdr's agent record, nested verbatim.
    agent: AgentRecord,
}

impl Display for Delivered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "prompted {} ({})", self.agent.pane(), self.agent.status())
    }
}

// =====================================================================================================================
// Errors
// =====================================================================================================================

/// Failure of the prompt flow.
#[derive(Debug, Error)]
pub enum PromptError {
    /// herdr refused something; its message and code are carried verbatim.
    #[error(transparent)]
    Herdr(#[from] HerdrError),
    /// The target's composer holds unsent text.
    ///
    /// Carries the target and nothing else. The guard's input is a snapshot of someone's
    /// half-written message, so there is deliberately no field here that could hold it.
    #[error("{target}'s composer holds unsent text; wait for it to clear, or pass --force to send anyway")]
    ComposerOccupied {
        /// The agent that was not prompted.
        target: String,
    },
    /// Two submissions both failed to move the agent.
    #[error("{target} did not start working after two prompts; try again once it is responsive")]
    Stalled {
        /// The agent that did not acknowledge.
        target: String,
    },
}

impl AsExitStatus for PromptError {
    fn exit_status(&self) -> ExitStatus {
        match self {
            Self::Herdr(error) => error.exit_status(),
            // Both are state the target already holds, and both clear on their own.
            Self::ComposerOccupied { .. } | Self::Stalled { .. } => ExitStatus::Conflict,
        }
    }

    fn herdr(&self) -> Option<HerdrRef> {
        match self {
            Self::Herdr(error) => Some(error.reference()),
            Self::ComposerOccupied { .. } | Self::Stalled { .. } => None,
        }
    }
}
```

The `sink.warn(&format!(…))` in `deliver` interpolates the **target**, never the text. Keep it that
way.

- [ ] **Step 4: Wire the command in**

In `src/cmd.rs`, add `mod prompt;` and `pub use prompt::PromptArgs;` beside the `presets` pair.

In `src/main.rs`, add `Prompt(PromptArgs)` to the `Command` enum and
`Command::Prompt(args) => run(args, &sink),` to the match, and add `PromptArgs` to the `crate::cmd`
import.

Then **delete** the `#![allow(dead_code, reason = "wired into `prompt` in Task 10")]` line from
`src/harness.rs`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --all-targets`
Expected: `test result: ok. 56 passed`.

- [ ] **Step 6: Check the help reads correctly**

```bash
cargo run -q -- prompt --help
```

Expected: the two skip flags described distinctly, and the three examples.

- [ ] **Step 7: Run every check, then commit**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
git add src/cmd.rs src/cmd/prompt.rs src/harness.rs src/main.rs
git commit -m "feat(prompt): deliver a prompt behind the composer guard"
```

RS-030 findings must still be confined to `src/herdr/`. If `deliver` is reported, its first
parameter is not `&str` — fix the order.

---

## Task 11: `spawn`

**Estimate:** 2 hours.

Read the spec's `spawn` section in full. Five ordered steps, and the ordering is the design:

1. **Pre-checks, before anything is created.** The name against herdr's rule (clap does this at
   parse time, because the field is an `AgentName`). Placement flags mutually exclusive, reporting
   the conflicting pair rather than letting the last one win — `--tab --pane` is a typo, not a
   choice. `--pane` with no `$HERDR_PANE_ID` is a usage error naming `--tab` and `--workspace` as
   the fix.
2. **Resolve the preset.** An unknown preset lists what is available and exits 3.
3. **Create the surface**, passing `--cwd` explicitly in all three modes.
4. **`agent start`, retrying `agent_pane_busy` with backoff** until `--settle-timeout`. Any *other*
   failure leaves the surface open and reports its pane id: whatever went wrong is on screen in it,
   and closing the pane would throw the error away with it.
5. **Deliver the first prompt**, if one was given, through `prompt`'s `deliver`.

Two deliberate changes from the shell function being replaced: **focus is opt-in**, and **stdin is
explicit** — prompt text arrives on stdin only when asked for with `-`, so a script whose own stdin
is a pipe no longer has that data silently swallowed.

`spawn` does **not** run the composer guard: no human has touched the pane it just created.

**Files:**
- Create: `src/cmd/spawn.rs`
- Modify: `src/cmd.rs`, `src/main.rs`, `src/config.rs`, `src/core.rs`, `src/herdr.rs`

- [ ] **Step 1: Write the failing tests**

```rust
// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        args: SpawnArgs,
    }

    fn parse(argv: &[&str]) -> SpawnArgs {
        Harness::try_parse_from(argv).expect("parses").args
    }

    fn record() -> AgentRecord {
        serde_json::from_str(
            r#"{"agent":"claude","agent_status":"working","pane_id":"w4:p17","name":"reviewer"}"#,
        )
        .unwrap()
    }

    #[test]
    fn conflicting_placement_flags_are_rejected_rather_than_last_one_wins() {
        // `--tab --pane` is a typo, not a choice, and clap names both flags in its own exit-2 error.
        let error = Harness::try_parse_from(["spawn", "reviewer", "--tab", "--pane"]).unwrap_err();

        let rendered = error.to_string();
        assert!(rendered.contains("--tab") && rendered.contains("--pane"), "got {rendered}");
    }

    #[test]
    fn placement_defaults_to_splitting_the_calling_pane() {
        assert_eq!(parse(&["spawn", "reviewer"]).placement(), Placement::Pane);
        assert_eq!(parse(&["spawn", "reviewer", "--tab"]).placement(), Placement::Tab);
        assert_eq!(parse(&["spawn", "reviewer", "--workspace"]).placement(), Placement::Workspace);
    }

    #[test]
    fn focus_is_opt_in() {
        // A tool meant to be driven by agents should not steal the human's focus. An interactive
        // shell alias can put --focus back.
        assert_eq!(parse(&["spawn", "reviewer"]).focus(), Focus::Leave);
        assert_eq!(parse(&["spawn", "reviewer", "--focus"]).focus(), Focus::Take);
    }

    #[test]
    fn a_name_herdr_would_refuse_fails_at_parse_time_before_any_surface_exists() {
        assert!(Harness::try_parse_from(["spawn", "Reviewer"]).is_err());
        assert!(Harness::try_parse_from(["spawn", "1st"]).is_err());
    }

    #[test]
    fn splitting_outside_a_herdr_pane_is_a_usage_error_naming_the_fix() {
        let error = anchor(None);

        assert_eq!(error.unwrap_err().exit_status(), ExitStatus::Usage);
    }

    #[test]
    fn the_usage_error_says_which_flags_work_instead() {
        assert_eq!(
            anchor(None).unwrap_err().to_string(),
            "--pane needs a calling herdr pane and HERDR_PANE_ID is unset; use --tab or --workspace"
        );
    }

    #[test]
    fn an_anchor_is_read_from_the_environment_rather_than_resolved_by_herdr() {
        // Never herdr's `--current`: that resolves server-side to whichever pane is *focused*, which
        // is not this one when the command runs from an unfocused pane.
        assert_eq!(anchor(Some("w4:p1")).unwrap(), PaneId::from("w4:p1"));
        assert!(anchor(Some("   ")).is_err(), "a blank variable is as good as unset");
    }

    #[test]
    fn the_retry_window_backs_off_and_lands_exactly_on_its_budget() {
        // 250ms doubling to a 2000ms cap, truncated so the total never overruns --settle-timeout.
        let delays: Vec<u64> = backoff(10_000).iter().map(Duration::as_millis).map(|ms| ms as u64).collect();

        assert_eq!(delays, [250, 500, 1000, 2000, 2000, 2000, 2000, 250]);
        assert_eq!(delays.iter().sum::<u64>(), 10_000);
    }

    #[test]
    fn a_zero_budget_means_one_attempt_and_no_retry() {
        assert!(backoff(0).is_empty());
        assert_eq!(backoff(100), [Duration::from_millis(100)]);
        assert_eq!(backoff(300), [Duration::from_millis(250), Duration::from_millis(50)]);
    }

    #[test]
    fn a_spawn_reports_the_agent_the_placement_and_whether_the_first_prompt_landed() {
        let spawned = Spawned {
            placement: Placement::Tab,
            delivered: Some(true),
            agent: record(),
        };

        assert_eq!(spawned.to_string(), "reviewer (claude) → w4:p17");
        assert_eq!(
            serde_json::to_string(&spawned).unwrap(),
            r#"{"placement":"tab","delivered":true,"agent":{"agent":"claude","agent_status":"working","pane_id":"w4:p17","name":"reviewer"}}"#
        );
    }

    #[test]
    fn a_spawn_with_no_prompt_omits_delivered_rather_than_reporting_false() {
        let spawned = Spawned {
            placement: Placement::Pane,
            delivered: None,
            agent: record(),
        };

        assert_eq!(
            serde_json::to_string(&spawned).unwrap(),
            r#"{"placement":"pane","agent":{"agent":"claude","agent_status":"working","pane_id":"w4:p17","name":"reviewer"}}"#
        );
    }

    #[test]
    fn a_failure_after_the_surface_exists_reports_the_pane_it_left_open() {
        // Whatever went wrong is on screen in that pane, and closing it would throw the error away.
        let error = SpawnError::AfterSurface {
            pane: PaneId::from("w4:p17"),
            error: Box::new(SpawnError::Herdr(crate::herdr::HerdrError::Refused {
                command: "agent start".to_owned(),
                code: "agent_name_taken".to_owned(),
                message: "agent name reviewer is already used".to_owned(),
            })),
        };

        assert_eq!(
            error.to_string(),
            "agent name reviewer is already used (pane w4:p17 left open)"
        );
        assert_eq!(error.exit_status(), ExitStatus::Conflict, "the wrapper forwards the inner status");
        assert!(error.herdr().is_some(), "and herdr's provenance too");
    }

    #[test]
    fn a_busy_pane_that_never_settles_is_a_retryable_conflict() {
        let error = SpawnError::PaneNeverSettled {
            pane: PaneId::from("w4:p17"),
            budget_ms: 10_000,
        };

        assert_eq!(error.exit_status(), ExitStatus::Conflict);
        assert_eq!(
            error.to_string(),
            "pane w4:p17 was still not an available shell after 10000ms; raise --settle-timeout and retry"
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets`
Expected: FAIL — `cannot find type SpawnArgs`.

- [ ] **Step 3: Write `src/cmd/spawn.rs` above the test module**

```rust
//! `spawn` — create a surface and start a preset-configured agent in it.

use std::fmt::Display;
use std::path::PathBuf;
use std::time::Duration;

use clap::Args;
use clap_stdin::MaybeStdin;
use serde::Serialize;
use thiserror::Error;

use crate::cmd::prompt::deliver;
use crate::cmd::{AsExitStatus, Cmd, ExitStatus};
use crate::config::{ConfigError, Presets};
use crate::core::{AgentName, NonEmptyText, PaneId, Sink};
use crate::herdr::agent::{self, AgentRecord, WORKING, Wait};
use crate::herdr::surface::{self, Focus, Placement};
use crate::herdr::{HerdrError, HerdrRef};

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// The environment variable herdr exports into every pane it owns, holding that pane's id.
const PANE_VARIABLE: &str = "HERDR_PANE_ID";

/// How long `agent start` is retried while the new pane's shell is still starting, in milliseconds.
///
/// Finding 3: creating a tab or workspace races with slow shell init, and herdr correctly refuses a
/// pane that has not reached its prompt. Ten seconds covers a shell running a directory-environment
/// hook without leaving a caller hanging on one that is genuinely broken.
const DEFAULT_SETTLE_MS: u64 = 10_000;

/// The first retry delay, in milliseconds.
const BACKOFF_FIRST_MS: u64 = 250;

/// The longest a single retry waits, in milliseconds.
const BACKOFF_CAP_MS: u64 = 2_000;

// =====================================================================================================================
// Spawn Args
// =====================================================================================================================

/// Create a pane, tab, or workspace and start a preset-configured agent in it.
///
/// herdr starts an agent only in a pane that already exists and is sitting at an interactive shell
/// prompt, so this does both halves: it creates the surface, reads back the new pane's id, and
/// starts the agent there.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  \
    herdr-agent-tools spawn reviewer --tab --preset opus\n  \
    herdr-agent-tools spawn fixer --preset sonnet --prompt \"Fix the flaky tests\"\n  \
    git diff | herdr-agent-tools spawn reviewer --prompt -\n  \
    herdr-agent-tools spawn big --workspace --preset fable -- --resume")]
pub struct SpawnArgs {
    /// The agent's name; must satisfy herdr's rule, which is checked before anything is created.
    name: AgentName,

    #[command(flatten)]
    placement: PlacementFlags,

    /// The preset to start; defaults to the config file's `default`.
    #[arg(long, value_name = "NAME")]
    preset: Option<String>,

    /// Read this preset file instead of the one in the config directory.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// A first prompt to deliver once the agent is up; `-` reads it from stdin.
    #[arg(long, value_name = "TEXT")]
    prompt: Option<MaybeStdin<NonEmptyText>>,

    /// The new surface's working directory; defaults to the current one.
    #[arg(long, value_name = "PATH")]
    cwd: Option<PathBuf>,

    /// Focus the new surface. Off by default, so a background launch does not steal the cursor.
    #[arg(long)]
    focus: bool,

    /// Milliseconds to keep retrying `agent start` while the new pane's shell is still starting.
    #[arg(long, value_name = "MS", default_value_t = DEFAULT_SETTLE_MS)]
    settle_timeout: u64,

    /// Extra arguments appended after the preset's, passed to the agent verbatim.
    ///
    /// No merging and no de-duplication, so the agent's own last-flag-wins rules settle any
    /// conflict with the preset.
    #[arg(last = true, value_name = "AGENT_ARG")]
    agent_args: Vec<String>,
}

/// Where the agent's pane comes from. Mutually exclusive, so clap reports the conflicting pair.
#[derive(Debug, Args)]
#[group(multiple = false)]
struct PlacementFlags {
    /// Split the calling pane. The default, and it needs `HERDR_PANE_ID`.
    #[arg(long)]
    pane: bool,
    /// Open a new tab labelled with the agent's name.
    #[arg(long)]
    tab: bool,
    /// Open a new workspace labelled with the agent's name.
    #[arg(long)]
    workspace: bool,
}

impl SpawnArgs {
    /// Which surface to create. Splitting the calling pane is the default.
    fn placement(&self) -> Placement {
        if self.placement.tab {
            Placement::Tab
        } else if self.placement.workspace {
            Placement::Workspace
        } else {
            Placement::Pane
        }
    }

    /// Whether the new surface takes the user's focus.
    fn focus(&self) -> Focus {
        if self.focus { Focus::Take } else { Focus::Leave }
    }
}

impl Cmd for SpawnArgs {
    type Ok = Spawned;
    type Err = SpawnError;

    /// Pre-checks, preset, surface, agent, first prompt — in that order, because a pre-check that
    /// runs after a surface exists is not a pre-check.
    fn execute(self, sink: &Sink) -> Result<Self::Ok, Self::Err> {
        let placement = self.placement();
        let anchor = match placement {
            Placement::Pane => Some(anchor(std::env::var(PANE_VARIABLE).ok().as_deref())?),
            Placement::Tab | Placement::Workspace => None,
        };

        let presets = Presets::load(self.config.as_deref())?;
        let preset = presets.resolve(self.preset.as_deref())?;
        let kind = preset.kind().to_owned();
        let mut agent_args = preset.args().to_vec();
        agent_args.extend(self.agent_args.iter().cloned());

        let cwd = match &self.cwd {
            Some(path) => path.clone(),
            None => std::env::current_dir().map_err(SpawnError::NoWorkingDirectory)?,
        };
        let cwd = cwd.to_string_lossy().into_owned();

        // Everything above is read-only, so nothing exists yet if any of it failed.
        let pane = match (placement, anchor) {
            (Placement::Pane, Some(anchor)) => surface::split(&anchor, &cwd, self.focus())?,
            (Placement::Tab, _) => surface::create_tab(&self.name, &cwd, self.focus())?,
            (Placement::Workspace, _) => surface::create_workspace(&self.name, &cwd, self.focus())?,
            // Unreachable: `anchor` is `Some` for exactly `Placement::Pane`, three lines above.
            (Placement::Pane, None) => return Err(SpawnError::MissingAnchor),
        };

        // From here on a failure leaves the pane open and names it: whatever went wrong is on
        // screen in it, and closing it would throw the error away with it.
        self.start_and_prompt(&pane, &kind, &agent_args, sink)
            .map_err(|error| error.note_open_pane(&pane))
    }
}

impl SpawnArgs {
    /// Starts the agent, retrying a pane whose shell has not settled, then delivers the first
    /// prompt if there is one.
    fn start_and_prompt(
        &self,
        pane: &PaneId,
        kind: &str,
        agent_args: &[String],
        sink: &Sink,
    ) -> Result<Spawned, SpawnError> {
        let started = self.start_when_settled(pane, kind, agent_args, sink)?;

        let Some(text) = &self.prompt else {
            return Ok(Spawned {
                placement: self.placement(),
                delivered: None,
                agent: started,
            });
        };

        // No composer guard here: no human has touched the pane this just created, and the
        // submission follows `agent start` immediately.
        let wait = Wait {
            until: vec![WORKING.to_owned()],
            timeout: DEFAULT_SETTLE_MS,
        };
        let agent = deliver(pane, text, Some(&wait), sink)?;
        Ok(Spawned {
            placement: self.placement(),
            delivered: Some(started.status() != WORKING),
            agent,
        })
    }

    /// `agent start`, retried while herdr says the pane is not yet an available shell.
    ///
    /// Finding 3: twice in six launches, `agent start` refused a just-created pane because its
    /// shell had not reached its prompt. herdr is right to refuse; the caller has to retry rather
    /// than abandon the seat. Every *other* failure returns immediately.
    fn start_when_settled(
        &self,
        pane: &PaneId,
        kind: &str,
        agent_args: &[String],
        sink: &Sink,
    ) -> Result<AgentRecord, SpawnError> {
        let mut announced = false;
        for delay in backoff(self.settle_timeout) {
            match agent::start(&self.name, kind, pane, agent_args) {
                Ok(agent) => return Ok(agent),
                Err(error) if error.code() == Some("agent_pane_busy") => {
                    if !announced {
                        sink.warn(&format!("pane {pane} is not an available shell yet; retrying"));
                        announced = true;
                    }
                    std::thread::sleep(delay);
                }
                Err(error) => return Err(SpawnError::Herdr(error)),
            }
        }

        // One last attempt after the budget is spent, so a zero settle timeout still tries once.
        match agent::start(&self.name, kind, pane, agent_args) {
            Ok(agent) => Ok(agent),
            Err(error) if error.code() == Some("agent_pane_busy") => Err(SpawnError::PaneNeverSettled {
                pane: pane.clone(),
                budget_ms: self.settle_timeout,
            }),
            Err(error) => Err(SpawnError::Herdr(error)),
        }
    }
}

// =====================================================================================================================
// Output
// =====================================================================================================================

/// What `spawn` produced.
#[derive(Debug, Serialize)]
pub struct Spawned {
    /// Which of the three surfaces was created.
    placement: Placement,
    /// Whether the first prompt's delivery was proven; absent when no prompt was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    delivered: Option<bool>,
    /// herdr's agent record, nested verbatim.
    agent: AgentRecord,
}

impl Display for Spawned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}) → {}",
            self.agent.name_or_unknown(),
            self.agent.kind().unwrap_or("unknown"),
            self.agent.pane()
        )
    }
}

// =====================================================================================================================
// Errors
// =====================================================================================================================

/// Failure of the spawn flow.
#[derive(Debug, Error)]
pub enum SpawnError {
    /// `--pane` with no `HERDR_PANE_ID`.
    #[error("--pane needs a calling herdr pane and HERDR_PANE_ID is unset; use --tab or --workspace")]
    MissingAnchor,
    /// The preset file could not be read, or did not hold the named preset.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// There is no current directory to hand the new surface.
    #[error("cannot read the current directory to use as the new surface's cwd: {0}")]
    NoWorkingDirectory(#[source] std::io::Error),
    /// herdr refused something; its message and code are carried verbatim.
    #[error(transparent)]
    Herdr(#[from] HerdrError),
    /// The first prompt could not be delivered.
    #[error(transparent)]
    Prompt(#[from] crate::cmd::prompt::PromptError),
    /// The new pane never reached its shell prompt inside the settle window.
    #[error("pane {pane} was still not an available shell after {budget_ms}ms; raise --settle-timeout and retry")]
    PaneNeverSettled {
        /// The pane that stayed busy.
        pane: PaneId,
        /// The window that was spent on it.
        budget_ms: u64,
    },
    /// A failure after the surface was created. The surface is left open and named.
    #[error("{error} (pane {pane} left open)")]
    AfterSurface {
        /// The pane that is still there.
        pane: PaneId,
        /// What actually went wrong.
        #[source]
        error: Box<SpawnError>,
    },
}

impl SpawnError {
    /// Notes that `pane` was created before this failure, so the message says where to look.
    ///
    /// The pane is deliberately not closed: whatever went wrong is on screen in it, and closing it
    /// would throw the error away with it.
    fn note_open_pane(self, pane: &PaneId) -> Self {
        Self::AfterSurface {
            pane: pane.clone(),
            error: Box::new(self),
        }
    }
}

impl AsExitStatus for SpawnError {
    fn exit_status(&self) -> ExitStatus {
        match self {
            Self::MissingAnchor => ExitStatus::Usage,
            Self::Config(error) => error.exit_status_hint(),
            Self::NoWorkingDirectory(_) => ExitStatus::Failure,
            Self::Herdr(error) => error.exit_status(),
            Self::Prompt(error) => error.exit_status(),
            Self::PaneNeverSettled { .. } => ExitStatus::Conflict,
            Self::AfterSurface { error, .. } => error.exit_status(),
        }
    }

    fn herdr(&self) -> Option<HerdrRef> {
        match self {
            Self::Herdr(error) => Some(error.reference()),
            Self::Prompt(error) => error.herdr(),
            Self::AfterSurface { error, .. } => error.herdr(),
            Self::MissingAnchor
            | Self::Config(_)
            | Self::NoWorkingDirectory(_)
            | Self::PaneNeverSettled { .. } => None,
        }
    }
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// The calling pane a split anchors on, read from the environment.
///
/// Never herdr's `--current`: that flag resolves server-side to whichever pane is *focused*, which
/// is not this one when the command runs from an unfocused pane.
///
/// # Errors
///
/// [`SpawnError::MissingAnchor`] when the variable is unset or blank, naming the two flags that work
/// outside a herdr pane.
fn anchor(variable: Option<&str>) -> Result<PaneId, SpawnError> {
    variable
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PaneId::from)
        .ok_or(SpawnError::MissingAnchor)
}

/// The delays a `agent_pane_busy` retry spends waiting, inside `budget_ms`.
///
/// Doubles from 250ms to a 2000ms cap, with the last delay shortened so the total lands exactly on
/// the budget rather than overrunning it. An empty schedule means one attempt and no retry.
fn backoff(budget_ms: u64) -> Vec<Duration> {
    let mut delays = Vec::new();
    let mut spent = 0;
    let mut delay = BACKOFF_FIRST_MS;

    while spent < budget_ms {
        let step = delay.min(budget_ms - spent);
        delays.push(Duration::from_millis(step));
        spent += step;
        delay = delay.saturating_mul(2).min(BACKOFF_CAP_MS);
    }
    delays
}
```

- [ ] **Step 4: Confirm `deliver` is reachable from `spawn`**

No change should be needed: `cmd::spawn` and `cmd::prompt` are siblings inside `cmd`, so the private
`mod prompt;` and `deliver`'s `pub(super)` are both visible from `spawn`.

`deliver`'s first parameter is `&str` and `spawn` passes a `&PaneId`; deref coercion handles that.
If it does not compile, pass `pane.as_ref()`.

- [ ] **Step 5: Wire the command in**

In `src/cmd.rs`, add `mod spawn;` and `pub use spawn::SpawnArgs;`.

In `src/main.rs`, add `Spawn(SpawnArgs)` as the **first** variant of `Command` — it is the command
the tool exists for, and clap lists variants in declaration order — plus its match arm and its
import.

Then **delete** the remaining in-progress allows:

- the `dead_code` allow at the top of `src/core.rs`
- the `dead_code` allow at the top of `src/config.rs`
- the `dead_code` allow at the top of `src/herdr.rs`

Run `cargo clippy --all-targets -- -D warnings` after each deletion. If one reports a genuinely
unused item, **delete the item** rather than restoring the allow — an unused item at this point is
something the design does not need.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --all-targets`
Expected: `test result: ok. 69 passed`.

- [ ] **Step 7: Check the three commands' help**

```bash
cargo run -q -- --help
cargo run -q -- spawn --help
```

Expected: `spawn`, `prompt`, `presets` in that order; the exit-code table in the root `after_help`;
`--focus` described as opt-in and `--prompt` documenting `-`.

- [ ] **Step 8: Run every check, then commit**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
git add src/cmd.rs src/cmd/spawn.rs src/core.rs src/config.rs src/herdr.rs src/main.rs
git commit --no-verify -m "feat(spawn): create a surface and start a preset-configured agent in it"
```

Use `--no-verify` only if the gate reports one of the four documented RS-030 findings in
`src/herdr/`, and say so in the commit body the way Task 8 did.

---

## Task 12: Full verification and the live rehearsal

**Estimate:** 45 minutes, plus however long the rehearsal takes.

The style guide's rule: **verification claims name the command that produced them, and static checks
are reported separately from any manual rehearsal against a live herdr session.** Nothing automated
above exercises a real herdr server, and nothing here changes that.

**Files:**
- Modify: none expected. Fix whatever the checks surface.

- [ ] **Step 1: Run the full static suite and record the exact output**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
```

Expected: the first four silent or `ok`. The gate reports **only** RS-030 findings inside
`src/herdr/`, plus RS-061 candidate prompts for the two `let _ =` lines in `core/sink.rs`, and
whatever RS-031/RS-033 prompts the file sizes earn. Every remaining in-progress allow must be gone,
so there should be **no RS-062 prompt at all**.

- [ ] **Step 2: Confirm no shipped file references anything outside this repository**

```bash
grep -rn "tmux-team\|/Users/\|vendor/herdr\|dotfiles\|\$HOME/" \
  src README.md CLAUDE.md Cargo.toml docs/STYLE-GUIDE.md fixtures 2>/dev/null
```

Expected: no output. `~/.config` in the README is a generic path a user has, not a reference to
anything outside the repo, and `HERDR_PANE_ID` / `XDG_CONFIG_HOME` / `HOME` are environment
variables, so tune the pattern rather than the files if either shows up.

- [ ] **Step 3: Confirm no prompt, preset argument, or terminal content can reach an error**

Walk every error variant and check what it carries:

```bash
grep -rn '#\[error(' src
```

For each, confirm the interpolated fields are a target, a pane id, a command name, a path, or a
count — never `text`, never `args`, never a snapshot. `PromptError::ComposerOccupied` and
`HerdrError::Unreadable` are the two the design calls out specifically; neither has a field that
could hold the content.

- [ ] **Step 4: Exercise every command that does not need a server**

```bash
cargo run -q -- --help
cargo run -q -- --version
cargo run -q -- presets --config /tmp/absent.toml; echo "exit=$?"
cargo run -q -- presets --config /tmp/absent.toml --json; echo "exit=$?"
cargo run -q -- spawn Reviewer 2>&1 | head -3; echo "exit=${PIPESTATUS[0]}"
cargo run -q -- spawn reviewer --tab --pane 2>&1 | head -3
cargo run -q -- prompt reviewer "" 2>&1 | head -3
```

Expected: exit 3 for the missing preset file in both modes, with the JSON form one NDJSON object
carrying `"type":"error"` and `"status":3`; exit 2 for the rejected name, the conflicting flags, and
the blank prompt.

- [ ] **Step 5: Report the static result**

Write down, in the words a report will use: which commands were run, that they passed, and that
**none of them exercised a live herdr server**. Do not blend this with Step 6.

- [ ] **Step 6: Rehearse against a live herdr session — separately**

This is manual and only makes sense from inside a herdr session. Report it as a rehearsal, distinct
from the static checks, and say which parts were exercised and which were not.

```bash
cargo build --release
export PATH="$PWD/target/release:$PATH"

# 1. A tab, no prompt, no focus stolen.
herdr-agent-tools spawn rehearsal --tab --preset <a real preset>

# 2. A prompt to it, with the guard armed.
herdr-agent-tools prompt rehearsal "reply with the single word: acknowledged"

# 3. The guard's refusal: type something into the rehearsal pane, do not submit, then
herdr-agent-tools prompt rehearsal "this must be refused"; echo "exit=$?"   # expect 5

# 4. The override.
herdr-agent-tools prompt rehearsal "this must land" --force

# 5. A split from inside a herdr pane, with a first prompt on stdin.
echo "summarise this repository in one sentence" | \
  herdr-agent-tools spawn rehearsal-two --pane --preset <a real preset> --prompt -

# 6. The machine form.
herdr-agent-tools prompt rehearsal "one more" --json
```

What to check, in order: the tab appears without taking focus; step 2 returns promptly rather than
waiting for the whole turn; step 3 exits 5 and the pane's half-written text is still unsent; step 4
delivers; step 5's stdin becomes the first prompt rather than being swallowed; step 6 emits one
NDJSON object per line with `type` on each.

Close the rehearsal surfaces afterwards.

- [ ] **Step 7: Commit anything the verification changed**

```bash
git status
git add -A
git commit -m "chore: fixes from the full verification pass"
```

If nothing changed, say so rather than making an empty commit.

---

## Self-review notes

Checked against the spec section by section. Coverage:

| Spec section | Task |
| --- | --- |
| Scope — three commands | 6, 10, 11 |
| The herdr seam, no macro | 7 |
| Propagate rather than restate | 7 (`Envelope`, `HerdrError`), 8 (`AgentRecord`) |
| Validate only what herdr won't | 2 (`NonEmptyText`, `PaneId`, `AgentName`), 8 (kind as `String`) |
| Module map — twelve files | file structure above; every one is created |
| `spawn`, all five steps | 11 |
| `prompt`, all three steps | 10 |
| `presets` | 6 |
| The composer guard, both fail-open paths | 9 |
| Preset configuration, three decisions | 5 |
| Errors and output, the code table | 4 (`ExitStatus`), 7 (herdr's codes), 5/10/11 (each command's) |
| Neither prompts nor terminal content logged | 7, 10, 12 Step 3 |
| Testing, all five groups | 5 (config), 8 (argument vectors), 9 (composer), 7 (error codes), 3 (sink wire forms) |
| Repository scaffolding | 1 |
| README roadmap | 1 |

Two things the spec leaves open that this plan decides, both stated at the site:

- **`prompt`'s human line** is `prompted <pane id> (<status>)`. The spec pins only `spawn`'s.
- **`delivered` is `Option<bool>`**, absent rather than `false` when there was no prompt. The spec's
  JSON example shows the `true` case, which this matches exactly.

One deviation from the spec's literal text, for a reason the spec could not have known: `run`'s
signature is `run(args: &[String])` rather than `run(args: &[&str])`, because every argument vector
is built from owned pieces (a cwd, a timeout rendered as a string, a preset's args). And `run_text`
exists alongside it because **`herdr agent read` prints raw text, not JSON** — verified in herdr's
`print_read_response`. The composer guard could not work without it.
