# Delivery Proof and the Codex Composer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two defects found by driving a live Codex agent, both of which make a guarantee this tool advertises silently absent. A message that was never delivered is reported as delivered, and the composer guard has stopped running for Codex agents entirely.

**Architecture:** Two independent changes, in two files each. Task group A splits `Proof` in `src/cmd/msg.rs` so that matching a state herdr was asked to wait for is no longer confused with proving *this* message arrived, and adds `agent::wait` to `src/herdr/agent.rs` so the pane can be read between the submission and the wait. Task group B fixes `prompt_box_range` in `src/harness.rs` to anchor on the composer's *top* border rather than on a closing border Codex no longer draws, bounding the body so the footer beneath it is not read as a draft, plus fixtures for the current rendering.

Neither group depends on the other. Either may ship alone.

**Tech Stack:** Rust 2024, clap derive, `thiserror`, `serde`, `tempfile` (dev). No new dependencies.

---

## Findings this plan exists to fix

Both were observed on 2026-07-31 against herdr with Codex v0.146.0 and Claude Code, through the
installed binary. Neither is reproducible in-crate, which is why both are written down rather than
caught by a test that already exists.

### Finding 1 — a lost message reported as delivered

Spawn a Codex agent and message it while its MCP servers are still starting:

    herdr-team spawn probe --placement tab --agent luna
    herdr-team msg probe "reply with the single word: ok" \
      --wait-until idle --wait-until done --timeout 60000

Observed: `messaged wJ:p1T (done)`, exit 0. The target's pane held no `<mail>` element at all and
its composer still showed Codex's own placeholder suggestion. The message was never delivered.

The cause is in `deliver`. Delivery is reported from the proof, and every matched wait counts:

    let proven = match (proof, &id) {
        (Proof::None, _) => false,
        (Proof::Wait(_), _) => true,          // <- this arm
        ...

herdr really did observe `idle -> done`, so the wait matched. That transition was Codex finishing
its own startup, not an answer to anything we sent. The wait proved *a* state change and the code
read it as proof of *our* delivery.

`MsgArgs::proof` already knows these are different things — its doc says an explicit `--wait-until`
is "asking about a transition rather than about delivery" — and then reports the transition as
delivery proof anyway.

The default path is not affected and must stay as it is: with no `--wait-until` the wait is for
`working`, which is a state a settled target does not drift into on its own, so matching it really
is proof. It is the caller-named states that may be ones the target was going to reach regardless.

### Finding 2 — the composer guard no longer runs for Codex

Every message to a Codex agent, including one fully started and visibly idle at its composer,
printed:

    could not locate the composer in the target's snapshot; delivering unguarded

That is `Composer::NoPromptBox` — the guard failing open. It fails open by design when it cannot
see what it needs, so nothing was refused wrongly; the protection simply is not there. A message
sent to a Codex agent will overwrite whatever half-written text a person has in its composer.

`prompt_box_range` takes the last two horizontal rules in the snapshot and treats what lies between
them as the composer. The fixture in `fixtures/composer/codex-empty.txt` matches that shape:

    ──────────────────────────
    ›
    ──────────────────────────
      ⌃C quit                              12% context

Codex v0.146.0 draws no closing rule:

    ──────────────────────────
                                      <- blank
    › Summarize recent commits
                                      <- blank
      gpt-5.6-luna medium · /private/tmp · Context 96% left · weekly 89% left · Fast off

One rule, so `rules.next()` yields it as `bottom`, the second `next()` is `None`, and the whole
function returns `None`.

herdr's own detector already handles this. `prompt_box_body` in its `src/detect/manifest.rs`
anchors on the **top** border and ends at the next rule *or at the end of the content*:

    let top = prompt_box_top_border_index(&lines)?;
    let end_index = lines[top + 1..]
        .iter()
        .position(|line| is_horizontal_rule(line))
        .map(|relative| top + 1 + relative)
        .unwrap_or(lines.len());

**Copying that rule alone is not enough, and this is the trap in this task.** With no closing rule
the body runs to the end of the snapshot, which means it swallows Codex's footer status line. And
`occupied_after` counts *any* non-empty line in the body other than the marker's own as a draft:

    || body.iter().enumerate().any(|(index, line)| index != head && !line.trim().is_empty())

So a naive port turns a guard that never fires into one that refuses every Codex message, with a
message saying someone has unsent text when nobody does. That is worse than the defect.

The live rendering separates the composer from the footer with a blank line, and Codex's composer
grows downward across contiguous lines as a draft wraps — which `src/harness/codex.rs` already
documents. So the body has an end: the first blank line after the marker's line.

---

## Conventions this codebase enforces

Read these before Task 1; every task assumes them. They are the same conventions the other plans in
this directory list, restated because a plan is read on its own.

- **Section headers.** Copy the separator verbatim from an adjacent file. Heavyweight for a major
  section; the skeleton is `Constants`, one header per domain group, `Helpers`, `Tests`.
- **The herdr seam.** Every herdr invocation goes through `herdr::run`, and every call to it lives
  in the `herdr` module. No module outside `herdr` spawns a process or names the `herdr` binary.
- **Argument vectors are pinned by exact-string tests**, one per herdr call. That is where the
  seam's correctness lives.
- **Automated tests never invoke herdr**, start an agent, or touch the user's session. A test
  assembles what it needs through the same constructors the read path uses, and a function that
  needs a herdr answer takes it as a parameter or a closure so the decision stays testable.
- **Neither prompts nor terminal content may be logged.** A refusal says the composer holds unsent
  text; it never says what that text is.
- **Composer fixtures carry neutral filler above the composer**, because the guard reads only the
  composer's structure and a real snapshot holds a whole transcript.
- **Verification** is `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test --all-targets`, `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`, and
  `bash .claude/skills/rust-style/scripts/check.sh src` — then `cargo install --path . --force`,
  because `herdr-team` on `PATH` resolves to `~/.cargo/bin` and a green `cargo test` says nothing
  about the command the user is about to type.

---

## Task group A — delivery proof

### Task A1 — add `agent::wait` to the seam

- [ ] Add `pub fn wait(target: &str, wait: &Wait) -> Result<AgentRecord, HerdrError>` to
      `src/herdr/agent.rs`, invoking `agent wait <target> --until <STATE>... --timeout <MS>`.
      Read `herdr agent` for the exact spelling rather than copying it from this plan.
- [ ] Pin the argument vector with an exact-string test beside the existing ones, including the
      repeated `--until` for several states.
- [ ] Reuse the existing `Wait` type rather than taking the parts separately, so `msg` cannot build
      one shape for `prompt` and another for `wait`.

### Task A2 — split `Proof` so a settle wait is not read as delivery proof

- [ ] In `src/cmd/msg.rs`, replace `Proof::Wait` with two variants that name why each proves what
      it proves:
      - `Delivery(Wait)` — herdr waits for `working`, a state a settled target does not reach on its
        own, so matching it *is* the proof. This is the no-`--wait-until` path and it does not
        change.
      - `Settle(Wait)` — herdr waits for the states the caller named. Matching proves a transition
        happened, not that this message caused it, so the pane supplies the proof instead.
- [ ] `Proof::for_delivery` keeps returning `Delivery` or `Pane` exactly as it does now. Only
      `MsgArgs::proof`'s explicit-`--wait-until` arm returns `Settle`.
- [ ] Update the doc comment on `Proof` to carry Finding 1: a caller-named state may be one the
      target was going to reach anyway, and a wait that matched it proves nothing about this
      message.

### Task A3 — read the pane before the wait, not after

- [ ] In `deliver`, the `Settle` path submits **without** handing herdr the wait, reads the pane for
      the envelope's id, and only then calls `agent::wait`. The returned record comes from the wait,
      since that is the later and more useful state.
- [ ] The ordering is the whole point and belongs in a comment: the id rides on the message's
      opening tag, and a turn that runs for a hundred lines scrolls that tag past the thousand-row
      clamp herdr applies. Reading after the wait would report a delivered message as unproven.
- [ ] `proven` becomes: `None` false, `Delivery(_)` true, `Pane` and `Settle(_)` from
      `confirm_in_pane`.
- [ ] A `Settle` submission that comes back `is_undelivered()` still re-sends once, as today. Check
      that the re-send does not send twice under the new ordering.

### Task A4 — tests for A2 and A3

- [ ] `MsgArgs::proof` returns `Settle` for an explicit `--wait-until` and `Delivery` for none.
- [ ] `Proof::wait()` answers `Some` for both wait-carrying variants and `None` for `Pane`/`None`.
- [ ] The proof decision maps to `proven` correctly for all four variants, with `confirm_in_pane`
      driven by a closure rather than a herdr call, as the existing tests do.
- [ ] A regression test named for Finding 1: a `Settle` wait that matched, with a pane that never
      showed the id, reports `delivered: false`.

### Task A5 — say it in the docs

- [ ] `msg --help`: `--wait-until` waits for the states named; delivery is proven by reading the
      target's pane.
- [ ] README, in the `msg` section: the same, in one sentence.
- [ ] The prime brief only if it fits the 120-line budget. It teaches invocations rather than
      internals, so this may be one the brief simply does not carry.

---

## Task group B — the Codex composer

### Task B1 — capture the current rendering as fixtures

- [ ] Capture a real Codex v0.146.0 detection snapshot with an **empty** composer and save it as
      `fixtures/composer/codex-empty-unbordered.txt`. Neutral filler above the composer, per the
      fixture convention.
- [ ] Capture the same with a **draft** in the composer, as `codex-occupied-unbordered.txt`.
- [ ] Keep `codex-empty.txt` and `codex-multiline.txt` exactly as they are. An older Codex still
      renders the closing rule and must keep working — that is the point of keeping both.
- [ ] Capture the placeholder-suggestion case too if the styled tier is to be trusted for it;
      `suggestion_only` needs a styled read, so this fixture is only worth adding alongside a test
      that exercises that path.

### Task B2 — anchor the box on its top border

- [ ] Rewrite `prompt_box_range` in `src/harness.rs` to find the **last** horizontal rule that has a
      composer below it, then end the body at whichever comes first: the next horizontal rule, a
      blank line, or the end of the snapshot.
- [ ] The blank-line bound is what keeps the footer out, and it is the half a naive port of herdr's
      rule gets wrong — write that reason at the site, referencing Codex's status line by shape
      rather than by content.
- [ ] Confirm the bounded body still contains a wrapped multi-line draft. Codex's composer grows
      downward across *contiguous* lines, so a wrapped draft has no blank line inside it, but verify
      against the `codex-multiline` fixture rather than trusting this sentence.
- [ ] Check the change against `claude-*` fixtures too: Claude Code draws both borders, so the
      closing-rule bound must still win there and nothing about those cases may move.

### Task B3 — tests

- [ ] Both new fixtures: empty reads `Composer::Empty`, drafted reads `Composer::Occupied`.
- [ ] Every existing fixture keeps its current answer. Add the assertion that the bordered and
      unbordered Codex renderings agree, so the guard does not depend on which Codex is installed.
- [ ] A snapshot whose only rule sits above a footer and no composer must still be `NoPromptBox`
      rather than a false `Occupied` — this is the failure mode B2 exists to avoid, so it needs its
      own test.
- [ ] `no-rules.txt` and `unknown-marker.txt` keep answering as they do today.

---

## Verification

Both groups, in order, then:

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --all-targets`
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`
- [ ] `bash .claude/skills/rust-style/scripts/check.sh src` — the candidate set should not grow
- [ ] `cargo install --path . --force`

Then rehearse against a live herdr session, and report that rehearsal separately from the static
checks:

- [ ] Spawn a Codex agent and message it *immediately*, while its MCP servers are still starting.
      It must not report a delivery that did not happen. Read the target's pane to confirm what
      actually arrived.
- [ ] Message a Codex agent that is fully up. The composer-guard warning must be gone.
- [ ] Type text into a Codex agent's composer by hand and message it. It must be refused with
      exit 5, and the refusal must not repeat what was typed.
- [ ] Repeat the last two against a Claude Code agent, which must be unchanged.
