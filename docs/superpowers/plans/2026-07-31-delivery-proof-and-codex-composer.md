# Delivery Proof and the Codex Composer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two defects found by driving a live Codex agent, both of which make a guarantee this tool
advertises silently absent. A message that was never delivered is reported as delivered, and the
composer guard has stopped running for Codex agents entirely.

**Architecture:** Two independent changes. Task group A splits `Proof` in `src/cmd/msg.rs` so that
matching a state herdr was asked to wait for is no longer confused with proving *this* message
arrived, and adds `agent::wait` to `src/herdr/agent.rs`. Task group B stops locating a composer by
the box drawn around it — modern Codex draws no box — and makes locating it each harness's own job,
in `src/harness.rs`, `src/harness/codex.rs`, and `src/harness/claude.rs`.

Neither group depends on the other. Either may ship alone.

**Tech Stack:** Rust 2024, clap derive, `thiserror`, `serde`, `tempfile` (dev). No new dependencies.

**Status:** Task B1 is done and committed (`26ed3bc`). B1a needs a live agent. Everything else is
open.

**Provenance:** this plan was reviewed against herdr's source and against every committed fixture
before implementation began, and rewritten twice as a result. Its first version described a Codex
rendering that does not exist; its second claimed a herdr read reaches into scrollback, which it
does not. Treat a factual claim here as checkable, and check it — every citation below names the
file and line it came from for that reason.

---

## Findings this plan exists to fix

Both were observed on 2026-07-31 against herdr with Codex v0.146.0 and Claude Code, through the
installed binary. Neither is reproducible in-crate, which is why both are written down rather than
caught by a test that already exists.

### Finding 1 — a lost message reported as delivered

Spawn a Codex agent and message it while its MCP servers are still starting:

    herdr-team spawn probe --placement tab --agent codex
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

#### The two waits herdr offers are not the same wait

`herdr agent prompt --wait --until X` and `herdr agent wait --until X` differ, and the difference
decides the whole shape of the fix.

`prompt_agent` (herdr's `src/api/wait.rs:175-302`) does two things a standalone wait does not.
When the target was **not** working it first waits for any state change past the submission's own
`state_change_seq`, capped at `AGENT_PROMPT_EFFECT_TIMEOUT_MS` (5000, `wait.rs:20`). When the
target **was** working it skips that gate but still carries `after_state_change_seq =
Some(prompt_state_change_seq)` into the settle wait (`wait.rs:229-296`), so the states named can
only be matched by a transition that happened after the submission.

`wait_for_agent` (`wait.rs:139-150`) has neither. It reads the current status and returns
immediately if it already matches:

    let until = agent_wait_statuses(params.until);
    if agent_wait_matches(&initial, &until, None) {
        return agent_wait_success(request_id, initial).map(Some);
    }

Two consequences, and the plan is shaped around both.

**A settle wait issued after a bare submit can match a stale turn.** Against an idle target,
`msg reviewer "run the tests" --wait-until idle --wait-until done` would submit, find the id in the
pane within a few hundred milliseconds, and ask herdr to wait while herdr still reports `idle` —
it sleeps `AGENT_PROMPT_SUBMIT_DELAY` (300ms, `app/api/agents.rs:13`) before pressing Enter. The
wait matches instantly and the caller is told the turn finished before it began. Against a target
that was **already** working, the same wait can match the *previous* turn's `done`. A pane hit
proves the message was queued, not that its turn ran.

**A bare submit cannot stall.** `agent_prompt_stalled` is only ever produced by the wait path —
`prompt_agent` returns early when `params.wait` is `None` — so the re-send-once repair in `deliver`
would stop running. That repair exists because a prompt sent seconds after a spawn is swallowed,
which is Finding 1's own scenario.

### Finding 2 — the composer guard no longer runs for Codex

Every message to a Codex agent, including one fully started and visibly idle at its composer,
printed:

    could not locate the composer in the target's snapshot; delivering unguarded

That is `Composer::NoPromptBox` — the guard failing open. It fails open by design when it cannot
see what it needs, so nothing was refused wrongly; the protection simply is not there. A message
sent to a Codex agent will overwrite whatever half-written text a person has in its composer.

`prompt_box_range` takes the last two horizontal rules in the snapshot and treats what lies between
them as the composer. `fixtures/composer/codex-bordered-empty.txt` matches that shape:

    ──────────────────────────
    ›
    ──────────────────────────
      ⌃C quit                              12% context

**Codex v0.146.0 draws no border at all — neither edge, not just the closing one.** From a live
capture, agent idle, composer untouched:

    18 |• You have 1 usage limit reset available. Run /usage to use one.|
    19 ||
    20 ||
    21 |› Find and fix a bug in @filename|
    22 ||
    23 |  gpt-5.6-codex medium · /work · Context 100% left · Fast off|

There is nothing to anchor on. Anchoring on a top border, which an earlier draft of this plan
proposed, anchors on something that does not exist.

#### The rules that *are* there belong to the transcript

Codex draws a full-width rule after a tool call. Two of them bound a region of transcript, and
today's code reads that region as the composer — this is `fixtures/composer/codex-empty.txt`:

    53 |───────────────────────────────────────────────────────────|
    54 ||
    55 |• 0000000 a commit subject|
    56 |  0000000 a commit subject|
    57 |  0000000 a commit subject|
    58 ||
    59 |───────────────────────────────────────────────────────────|
    60 ||
    61 ||
    62 |› Find and fix a bug in @filename        <- the actual composer|

It answers `UnknownMarker` today and fails open, which is luck rather than design: the region
happens to start with `•`. Codex echoes every submitted message back into its transcript behind the
same `›` the composer uses, so a region starting with one would be read as unsent text and **every
message to that agent refused**, naming a draft that does not exist. The guard is not merely off;
it is primed to jam.

#### The marker is echoed too, so the last one is not always the live one

Marker lines in one 66-line snapshot: **8, 19, 38, 53**. Only 53 is the composer; the rest are past
messages. `fixtures/composer/codex-working.txt` is the shape in miniature — echo at line 4, block
marker at 7, live composer at 12. Claude Code does the same: `claude-working-queued.txt` has
transcript markers at 28 and 33 and its live composer at 35-37.

herdr has a discriminator for Codex (`current_codex_prompt_index`, its `src/detect/manifest.rs:1405`):
take the last marker line, then reject it if any block marker — `•` `■` `✗` `✓` — appears *after*
it. A marker with transcript below it is transcript.

**Claude Code has no equivalent and must not be given the marker scan at all.** It draws both
borders reliably, so its existing two-rule locator is both correct and already proven; a marker
scan would accept its line-33 echo whenever the live composer sits past the window. This is why
Task B2 makes locating the composer a per-harness decision rather than one shared rule.

#### A large paste hides the composer, and reading more does not always help

Sixty pasted lines fill a Codex composer, and every line of the snapshot is draft:

      0 |  line 23 of a long pasted block of text that someone dropped into the composer|
    ...
     37 |  line 60 of a long pasted block of text that someone dropped into the composer|
     38 ||
     39 |  gpt-5.6-codex medium · /work · Context 100% left · Fast off|

No marker, no rule. That is `fixtures/composer/codex-pasted.txt`, and its correct answer is
`NoPromptBox`: the guard genuinely cannot see that composer and fails open. This is the case the
guard is most worth having — nobody minds losing four typed words.

**`detection` does not reach into scrollback, and an earlier draft of this plan wrongly said it
does.** herdr builds that rendering from the pane's own row count and only then applies the
caller's `--lines`:

    fn ghostty_detection_text(core: &GhosttyPaneCore) -> Result<String, crate::ghostty::Error> {
        let lines = core.terminal.rows().ok().map(|rows| usize::from(rows).max(1))
            .unwrap_or(DEFAULT_DETECTION_ROWS);
        ghostty_recent_text(core, lines)
    }

— `~/vendor/herdr/src/pane/terminal.rs:2264-2271`. `src/cmd/msg.rs:40-42` already says the same
thing about this source. So raising the request from 40 lines yields more only up to the pane's
height, and **a composer taller than the pane is unreachable, full stop**. Raising it is a
mitigation worth having, since most panes are taller than forty rows, and it is not a fix.

Claude Code does not need it: it collapses a paste into `[Pasted text #1 +13 lines]` chips, so
`claude-pasted.txt` still holds both rules and the live marker inside forty lines.

#### The bound below the marker is a blank line

Codex's composer grows downward across *contiguous* lines — `codex-multiline.txt`:

    35 |› first line of the draft|
    36 |  second line of the draft|
    37 |  third line of the draft|
    38 ||
    39 |  gpt-5.6-codex medium · /work · Context 95% left · Fast off|

A blank line separates it from whatever is beneath, which for Codex is the footer and for a Codex
with its `/` menu open is that menu. Without that bound the body swallows the footer, and
`occupied_after` counts any non-empty line in the body as a draft:

    || body.iter().enumerate().any(|(index, line)| index != head && !line.trim().is_empty())

So an unbounded body turns a guard that never fires into one that refuses every Codex message.

Every committed capture supports the bound, and none of them covers a draft that *opens* with a
deliberate blank line. Do not write it down as universal.

#### The footer is not a fixed shape and nothing may key on it

Claude Code's status line is user-configurable — the captures were taken on a machine running a
three-line powerline status line where a stock install draws one line. Codex rewrites its own
footer depending on what the composer holds (`tab to queue message` appears only when a draft sits
in a working agent's composer). `fixtures/composer/claude-stock-footer-*.txt` is kept for this
reason. **No part of this change may identify the footer.** The blank line above it is the bound;
its content and its line count are an operator's configuration.

---

## Conventions this codebase enforces

Read these before Task A1; every task assumes them. They are the same conventions the other plans
in this directory list, restated because a plan is read on its own.

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
- **Fixtures come in pairs.** `<name>.txt` is the plain `detection` read and `<name>.ansi.txt` the
  styled `visible` one; `fixtures/composer/README.md` says what each scenario proves.
- **Verification** is `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test --all-targets`, `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`, and
  `bash .claude/skills/rust-style/scripts/check.sh src` — then `cargo install --path . --force`,
  because `herdr-team` on `PATH` resolves to `~/.cargo/bin` and a green `cargo test` says nothing
  about the command the user is about to type.

---

## Task group A — delivery proof

### Task A1 — add `agent::wait` to the seam

- [ ] Add `pub fn wait(target: &str, wait: &Wait) -> Result<AgentRecord, HerdrError>` to
      `src/herdr/agent.rs`, invoking `agent wait <TARGET> --until <STATE>… --timeout <MS>`. Read
      `herdr agent` for the exact spelling rather than copying it from this plan.
- [ ] Its result is `{"agent": {…}}`, the same envelope `agent get` returns
      (`~/vendor/herdr/src/api/wait.rs:599-607`), so it deserializes through a struct shaped like
      `AgentInfo` rather than a new one.
- [ ] Pin the argument vector with an exact-string test beside the existing ones, including the
      repeated `--until` for several states.
- [ ] Reuse the existing `Wait` type rather than taking the parts separately, so `msg` cannot build
      one shape for `prompt` and another for `wait`.
- [ ] The doc comment carries the difference this seam exists because of, and the warning that goes
      with it: unlike the wait attached to a prompt, this one matches a status the target already
      holds, so it proves a transition only for a caller that has already established one.

### Task A2 — split `Proof` so a settle wait is not read as delivery proof

- [ ] In `src/cmd/msg.rs`, replace `Proof::Wait` with two variants that name why each proves what
      it proves:

          /// herdr waits for `working`, which a settled target does not reach on its own — so
          /// matching it is the proof. The no-`--wait-until` path, unchanged.
          Delivery(Wait),
          /// herdr waits for the states the caller named. Matching proves a transition happened,
          /// not that this message caused it, so the pane supplies the delivery proof instead.
          Settle {
              /// The wait the submission carries, establishing a turn that began after it.
              /// Absent when the target was already working, which is the branch that keeps the
              /// caller's wait on the submission — see Task A3.
              delivery: Option<Wait>,
              /// The caller's states.
              until: Wait,
          },

- [ ] Factor the "what wait does a submission against `current` carry" rule into one function
      returning `Option<Wait>`, and have `for_delivery` map it to `Delivery`/`Pane`. `Settle`'s
      `delivery` field is that same function, so the two paths cannot drift.
- [ ] Move the `agent::get` in `execute` to **after** the composer guard, so the status the branch
      is chosen from is as fresh as the decision. This narrows the stale-status window; A3's
      handling of a delivery-leg timeout is what closes it.
- [ ] Update the doc comment on `Proof` to carry Finding 1: a caller-named state may be one the
      target was going to reach anyway, and a wait that matched it proves nothing about this
      message.

### Task A3 — the settle path, branched on what the target was doing

The two branches are not symmetric, and the asymmetry is the design rather than an oversight.

- [ ] **Settled target** (`Settle { delivery: Some(wait), until }`): submit carrying the delivery
      leg, read the pane for the envelope's id, then call `agent::wait` for the caller's states.
      The leg establishes a turn that began after the submission, so a later `idle`/`done` cannot
      be a stale one. `proven` comes from the pane.
- [ ] **Already-working target** (`Settle { delivery: None, until }`): keep today's shape exactly —
      the caller's wait rides on `agent prompt`. The pane is read afterwards for `proven`.
- [ ] **This branch waits out the turn in progress, and that is all it can claim.** herdr's
      `after_state_change_seq` requires only that the matched status carry a higher sequence than
      the submission (`agent_wait_matches`, `~/vendor/herdr/src/api/wait.rs:539-545`), and the turn
      that was already running satisfies that when it ends. Neither the shape kept here nor any
      other reachable through this CLI can tell that ending from the queued message's own. Say so
      in the doc comment and in `--help`; do not write a guarantee the code cannot keep.
- [ ] Write down why the second branch is not the first: with no settled moment to start from,
      there is no delivery leg that can establish a new turn. The cost is the one this plan
      otherwise removes — a turn long enough to scroll the opening tag past herdr's thousand-row
      clamp reports a delivered message as unproven. That is a stated limit, and the better trade
      than a wait that lies.
- [ ] **A delivery-leg timeout must not fall through into the standalone settle wait.** Pane proof
      may still answer `proven`, but the caller asked to wait for a settle this path can no longer
      honestly perform, so it returns the timeout rather than a success. Exhausting the delivery
      allowance cannot become a successful settle because the pane happened to show the id.
- [ ] `proven` becomes: `None` false, `Delivery(_)` true, `Pane` and both `Settle` shapes from
      `confirm_in_pane`.
- [ ] **The unproven warning at `src/cmd/msg.rs:190-194` becomes status-neutral.** It says the
      target "was already working", which was the only way to reach it before and is now false for
      a settled target whose pane never showed the id. Say what is actually known — the pane never
      showed the message, so delivery is unproven — and name no status. Same for the doc on
      `Delivered::delivered` (`msg.rs:448-452`), which documents only the old case.
- [ ] A submission that comes back `is_undelivered()` still re-sends once, as today. The settle
      wait runs once, after whichever submission succeeded, and never twice.

### Task A4 — one deadline, and a seam that can be tested

- [ ] **One monotonic deadline** covers the first prompt, the re-send, the pane polling, and the
      settle wait. Each operation receives the time remaining. Do not recompute a fresh
      `min(caller_timeout, DEFAULT_TIMEOUT_MS)` for the re-send — a phase budget recomputed per
      attempt exceeds the total the caller asked for.
- [ ] The delivery leg's share is `min(remaining, DEFAULT_TIMEOUT_MS)`. At **5000ms or less** herdr
      answers a bare `timeout` instead of `agent_prompt_stalled` — the comparison is `timeout_ms <=
      AGENT_PROMPT_EFFECT_TIMEOUT_MS` (`wait.rs:231-244`), so five seconds exactly is already on
      the losing side — which takes the re-send repair with it. That is an acceptable contract for
      a caller who asked for a short leash, and it is only acceptable if it is deliberate: pin the
      boundary at 5000 and 5001 in a test named for it, and say it in `--help`.
- [ ] Factor `deliver` over injected prompt, read, and wait operations — or extract the transition
      driver as a pure function — so the ordering can be exercised without a herdr process, per the
      rule that automated tests never start one.
- [ ] Tests over that seam, counting calls rather than inspecting values: first submission
      succeeds; first submission stalls then succeeds; both stall; the delivery leg times out; the
      pane never shows the id; and the settle wait is issued exactly once, after delivery, and not
      at all when the delivery leg timed out.

### Task A5 — tests for the proof decision

- [ ] `MsgArgs::proof` returns `Settle` for an explicit `--wait-until` and `Delivery` for none.
- [ ] `Settle`'s `delivery` leg is `Some(--until working)` against a settled target and `None`
      against a working one, matching what `for_delivery` chooses for the same status.
- [ ] `Proof::wait()` — the wait handed to the submission — answers correctly for all four shapes,
      including that an already-working `Settle` hands over the caller's states rather than a
      delivery leg.
- [ ] A regression test named for Finding 1: a `Settle` wait that matched, with a pane that never
      showed the id, reports `delivered: false`.
- [ ] The unproven warning names no status, so a settled target that goes unproven is not told it
      was already working. Assert the text rather than only the flag — the wording is the defect.

### Task A6 — say it in the docs

- [ ] `msg --help`: `--wait-until` waits for the states named and, against a target already
      working, waits out the turn in progress rather than the queued message's own; delivery is
      proven by reading the target's pane; `--timeout` is the whole operation's budget, and at
      5000ms or less the re-send repair does not run.
- [ ] README, in the `msg` section: the first two clauses, in one sentence.
- [ ] The prime brief only if it fits the 120-line budget. It teaches invocations rather than
      internals, so this may be one the brief simply does not carry.

---

## Task group B — the Codex composer

### Task B1 — capture the current rendering as fixtures ✅ done (`26ed3bc`)

Nineteen scenarios captured from a live Codex v0.146.0 and a live Claude Code, each as the two
renderings the guard reads, scrubbed and committed with a README saying what each proves. The two
fixtures from a Codex that drew borders are kept as `codex-bordered-*`, and a stock-footer Claude
pair beside the custom-footer captures.

### Task B1a — one capture the committed set cannot supply

- [ ] `codex-pasted.txt` was taken at `--lines 40` and holds no marker, so it proves the guard
      *fails*, which is its job. B4 needs a capture of the same paste that holds one. Take it at
      `--lines 80` **in a pane taller than forty rows**, since `detection` is generated from the
      pane's row count and the request can only truncate it — in a shorter pane the two captures
      are identical and prove nothing.
- [ ] Take the matching `visible`/`ansi` half at 40 lines, per the fixture contract: `readiness`
      performs the styled reread whenever the plain read says occupied, which this capture will.
      Reusing another scenario's styled file is allowed only if the plan says which and why.
- [ ] Record the pane height and the requested line count in the README row for both, so a later
      reader can tell whether the pair still demonstrates anything.
- [ ] Keep the 40-line capture. It is the honest "cannot see it" case and B5 asserts it.

### Task B2 — make locating the composer the harness's own job

- [ ] Add a locator to `AgentHarness`: given the snapshot's lines, return the range its composer
      occupies, or `None`. This replaces the single shared `prompt_box_range`, whose sharedness is
      the bug — Codex draws no box and the rules it does draw belong to its transcript.
- [ ] `ClaudeCode` keeps today's two-rule search, unchanged and unmoved. Its behaviour must be
      provably untouched; the rehearsal checks it, and the stock-footer fixtures pin it.
- [ ] `Codex` scans **upward from the bottom** for a line whose first non-blank character is its
      marker, and ends the body at whichever comes first: a blank line, a horizontal rule, or the
      end of the snapshot.
- [ ] **Each attempt is atomic — one harness's locator plus that same harness's marker
      recognition.** `HARNESSES` is Claude-first (`src/harness.rs:171-172`), so a search that let
      Claude's two-rule locator succeed before Codex's recognition ran would hand back a region
      bounded by Codex's *transcript* rules and never reach the live marker.
- [ ] Carry the harness that recognized the plain composer into the styled reread, so the second
      read cannot select a different region than the first.
- [ ] The generic two-rule search survives only as a last fallback, reached when no harness
      recognized a composer, and only to tell `UnknownMarker` from `NoPromptBox`.
- [ ] The module doc currently claims locating the box is harness-agnostic. It is not any more —
      say so, and say why.
- [ ] The blank-line bound keeps the footer out. Write that reason at the site, referencing the
      footer **by position only**, and say that a draft opening with a deliberate blank line is
      uncovered by any fixture.

### Task B3 — let Codex reject a marker that belongs to its transcript

- [ ] Codex's locator rejects a candidate marker when any line after it begins with `•`, `■`, `✗`,
      or `✓`, and keeps scanning upward. Port those four characters from herdr's
      `codex_block_marker_line` and say in the doc comment that herdr is where they come from, so a
      future divergence has somewhere to be checked against.
- [ ] Claude Code gets no such rule and no marker scan. Say why at the site: it has no block-marker
      vocabulary to reject with, and it does not need one because it draws both borders.

### Task B4 — read enough of the pane to see the composer

- [ ] Raise `Probe::default`'s line count from 40 to 80, and replace the "forty lines is more than
      any composer needs" sentence with what the captures showed — including the limit: `detection`
      is the pane's own rows, so this reaches further only in a pane taller than forty, and a
      composer taller than the pane is unreachable. State that the guard fails open there.
- [ ] **`src/harness.rs:270` passes `probe.lines` to the styled read, not `STYLED.lines`.** Change
      it, or raising the plain probe silently raises the styled one and leaves `STYLED.lines` dead.
- [ ] Say at the site why the styled read stays at 40: `visible` is the viewport
      (`~/vendor/herdr/src/pane/terminal.rs:2251-2261`), so a larger number misreports how much was
      looked at, and a box it cannot find already produces the answer that refuses
      (`suggestion_only` returns `false`, `src/harness.rs:317-324`).
- [ ] B5 asserts the two reads separately, by source, rather than asserting one line count.

### Task B5 — tests

- [ ] Every fixture in `fixtures/composer/` gets a case, with `README.md` as the index of what each
      is expected to answer. The bordered Codex pair and the stock-footer Claude pair keep their
      current answers unchanged.
- [ ] Assert that the bordered and unbordered Codex renderings of the same scenario agree, so the
      guard does not depend on which Codex is installed.
- [ ] `codex-empty` is the Finding 2 regression: rules in the transcript, an empty composer below
      them, and the answer is `Empty` rather than anything read from between those rules.
- [ ] `codex-pasted` (40 lines) reads `NoPromptBox`; the B1a capture reads `Occupied`. Drive both
      through a closure that truncates to `Probe::default().lines`, so the pair proves the line
      count does something rather than proving the file is long.
- [ ] `claude-pasted` reads `Occupied` through Claude's own two-rule locator, with no raised probe
      needed — it holds both rules within forty lines because Claude collapses a paste into chips.
- [ ] `codex-working.txt` truncated before its line 12: the live composer is gone, the line-4
      candidate is a transcript echo with a block marker below it, and the answer is `NoPromptBox`
      rather than a draft read out of the transcript. This is the only case that exercises B3's
      rejection branch — the untruncated fixture never asks, because the bottom-up scan finds the
      live marker first.
- [ ] `claude-working-queued` reads `Empty`: the harness writes `Press up to edit queued messages`
      into the composer itself, faint, so this exercises the styled tier rather than the plain one.
- [ ] A snapshot whose only rule sits above a footer, with no composer anywhere, is `NoPromptBox`
      rather than a false `Occupied`.
- [ ] `no-rules.txt` and `unknown-marker.txt` keep answering as they do today.
- [ ] The styled fixtures are real captures, so the existing `against_styled` helper — which fakes
      the plain read by stripping escapes from the styled one — no longer stands in for both. Give
      the pair-based cases a helper that serves each read from its own file.

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
- [ ] Dispatch to a settled Codex agent with `--wait-until idle --wait-until done` and time it. It
      must return when the turn ends, not within a second of being asked.
- [ ] Dispatch the same way to an agent that is *already working*. It returns when the turn in
      progress ends, which may be the one that was already running — confirm the behaviour matches
      what `--help` now says, rather than expecting it to wait for the queued message's own turn.
- [ ] Message a Codex agent that is fully up. The composer-guard warning must be gone.
- [ ] Type text into a Codex agent's composer by hand and message it. It must be refused with
      exit 5, and the refusal must not repeat what was typed.
- [ ] Paste something long enough to fill the pane into a Codex composer and message it. This is
      the case the raised probe exists for; in a pane taller than forty rows it must refuse.
- [ ] Repeat the last three against a Claude Code agent, which must be unchanged.
