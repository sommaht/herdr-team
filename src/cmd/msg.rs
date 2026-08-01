//! `msg` — deliver a message to an agent that already exists.
//!
//! Called `prompt` until this build, and renamed because what lands is no longer a prompt: it is a
//! prompt inside an envelope naming its sender and how to answer. The old spelling stays as a hidden
//! alias. Everything below still says *prompt* for the text itself, which is what herdr's own
//! `agent prompt` takes and what the redaction rule is written about.

pub(super) mod envelope;

use std::fmt::Display;

use clap::Args;
use clap_stdin::MaybeStdin;
use serde::Serialize;
use thiserror::Error;

use self::envelope::{Envelope, Reply};
use crate::cmd::{AsExitStatus, Cmd, ExitStatus};
use crate::core::{NonEmptyText, Sink};
use crate::harness::{self, Composer};
use crate::herdr::agent::{self, AgentRecord, WORKING, Wait};
use crate::herdr::{HerdrError, HerdrRef};

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// How long herdr waits for the states that prove delivery, in milliseconds.
///
/// Long enough for a busy harness to acknowledge input, short enough that a dispatch does not hang
/// on an agent that is never going to answer. Also the ceiling on a delivery leg, which is the same
/// question asked inside a longer operation — see [`delivery_leg`].
const DEFAULT_TIMEOUT_MS: u64 = 15_000;

/// The window herdr gives a submission to take effect before it will report a stall, in
/// milliseconds.
///
/// Not ours to choose: it is `AGENT_PROMPT_EFFECT_TIMEOUT_MS` in herdr's own wait path, and the
/// comparison there is `timeout_ms <= AGENT_PROMPT_EFFECT_TIMEOUT_MS` — so five seconds exactly is
/// already on the losing side. A submission whose wait is that short is answered with a bare
/// `timeout` instead of `agent_prompt_stalled`, which takes the re-send repair with it. Named here
/// so the contract `--help` states has one place to be checked against.
const STALL_REPORTED_ABOVE_MS: u64 = 5_000;

/// herdr's rendering that rejoins soft-wrapped rows into the logical lines they were written as.
///
/// Not `recent`, which walks display rows: Codex announces a queued message with a banner running to
/// eighty-eight characters, and in any narrower pane — a vertical split, which is the default
/// placement — that banner breaks across two rows, taking any match with it.
///
/// Not `detection` either, which is where the composer guard reads. herdr builds it as `recent`
/// pinned to the terminal's own row count, so `--lines` can only ever shrink it. A message long
/// enough to scroll the transcript is a message `detection` cannot see.
const DELIVERY_SOURCE: &str = "recent-unwrapped";

/// Rows read past the message's own height, covering the harness furniture drawn beneath it.
const DELIVERY_MARGIN: u32 = 40;

/// The most rows one read can return, which herdr clamps to whatever is asked for.
const DELIVERY_MAX_LINES: u32 = 1_000;

/// How long to keep looking for a message's id before giving up on proving it landed.
///
/// A harness renders a queued message within a frame or two, so this is generous. It is spent only
/// when the proof is not there, which is either a genuinely undelivered prompt or a harness whose
/// rendering this build no longer recognizes.
const DELIVERY_POLL_MS: u64 = 3_000;

/// How long to wait between looks.
const DELIVERY_INTERVAL_MS: u64 = 250;

// =====================================================================================================================
// Msg Args
// =====================================================================================================================

/// Deliver a message to an agent that already exists.
///
/// The target is a herdr pane id or a unique agent name; herdr resolves it server-side and answers
/// `agent_not_found` or `agent_target_ambiguous` itself.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  \
    herdr-team msg reviewer \"run the test suite and report failures\"\n  \
    git diff | herdr-team msg reviewer -\n  \
    herdr-team msg dispatcher --no-reply \"done: 4 flags are undocumented\"\n  \
    herdr-team msg w4:p17 \"go\" --wait-until idle --wait-until done --timeout 120000\n\
    \n\
    Called `prompt` until this build; that spelling still works.")]
pub struct MsgArgs {
    /// The agent to message: a herdr pane id, or a unique agent name.
    target: String,

    /// The prompt text; `-` reads it from stdin (e.g. a heredoc or a pipe).
    // `allow_hyphen_values` is not in the doc comment because a caller has nothing to do about it.
    // Without it a prompt that opens with `--` is tokenized as a flag, and clap's rejection repeats
    // the would-be prompt back — which the prompt-redaction rule forbids outright. `--` is a repair
    // only here; `spawn --msg` has no separator to spare, so the fix has to be the same one on
    // both inputs.
    #[arg(allow_hyphen_values = true)]
    text: MaybeStdin<NonEmptyText>,

    /// Wait for these states instead of `working`; repeat for more than one.
    ///
    /// Delivery is then proven by reading the target's pane for this message rather than by the
    /// wait, because a state you name may be one the target was going to reach anyway — a matched
    /// wait says a transition happened, never that yours caused it. Against a target that is
    /// already working it waits out the turn *in progress*, which may not be this message's own.
    #[arg(long, value_name = "STATE")]
    wait_until: Vec<String>,

    /// Milliseconds for the whole operation: the submission, the re-send it may need, the pane
    /// check, and any --wait-until.
    ///
    /// At 5000 or less the re-send repair does not run. herdr answers a plain timeout rather than
    /// the stall that repair keys on, so a short leash gives it up.
    #[arg(long, value_name = "MS", default_value_t = DEFAULT_TIMEOUT_MS)]
    timeout: u64,

    /// Submit without waiting for proof that the agent received it.
    #[arg(long)]
    no_verify: bool,

    /// Omit the reply instructions, closing the loop instead of inviting an answer.
    #[arg(long, conflicts_with = "reply_to")]
    no_reply: bool,

    /// Address the reply instructions at this target instead of at the sender.
    #[arg(long, value_name = "TARGET")]
    reply_to: Option<String>,

    /// Send even if the target's composer holds unsent text.
    #[arg(long)]
    force: bool,
}

impl MsgArgs {
    /// How this submission's delivery will be proven, given what the target is doing now.
    ///
    /// An explicit `--wait-until` is honored whatever the current status, because the caller is
    /// asking about a transition rather than about delivery: waiting on a settled state against a
    /// working agent is a request to wait out the turn, and that transition does happen. It is
    /// *only* a question about a transition, though — which is why it produces [`Proof::Settle`],
    /// whose delivery proof comes from the pane instead. A caller-named state may be one the target
    /// was going to reach on its own, and a wait that matched it proves nothing about this message.
    ///
    /// Which states settle is the caller's to name, and naming only one is the trap: a Claude Code
    /// agent finishes at `done` and never reaches `idle`, so `--wait-until idle` alone spends the
    /// whole timeout and then reports a failure for a message that was delivered and answered.
    /// Every example this crate ships names both.
    fn proof(&self, current: &str) -> Proof {
        if self.no_verify {
            return Proof::None;
        }
        if self.wait_until.is_empty() {
            return Proof::for_delivery(current, self.timeout);
        }
        Proof::Settle {
            // The leg only has to see the target start working, so it takes at most an ordinary
            // delivery's budget out of the caller's and leaves the rest for the turn. A caller
            // asking for two minutes is asking about the turn, and the turn is what `until` waits
            // out. The no-`--wait-until` path has no larger operation to leave anything for, so it
            // hands the whole timeout to the leg exactly as it always did.
            delivery: delivery_leg(current, self.timeout.min(DEFAULT_TIMEOUT_MS)),
            until: Wait {
                until: self.wait_until.clone(),
                timeout: self.timeout,
            },
        }
    }

    /// Whether the composer guard runs.
    fn guarded(&self) -> bool {
        !self.force
    }

    /// Where this message says a reply should go.
    fn reply(&self) -> Reply {
        Reply::from_flags(self.reply_to.as_deref(), self.no_reply)
    }
}

impl Cmd for MsgArgs {
    type Ok = Delivered;
    type Err = MsgError;

    /// The guard, then the status, then the submission.
    ///
    /// The guard runs before anything is sent, so a refusal has changed nothing.
    ///
    /// Two reads of the target rather than one, because the two answers keep differently. The
    /// *kind* selects a harness and cannot change under a running agent, so the guard is handed the
    /// one read before it. The *status* chooses which proof this delivery can offer and goes stale
    /// the moment it is read, so it is taken after the guard's own reads rather than before them —
    /// a target that settled while the composer was being inspected would otherwise be prompted as
    /// though it were still mid-turn. An unguarded `--force` send reads once, as it always did.
    fn execute(self, sink: &Sink) -> Result<Self::Ok, Self::Err> {
        if self.guarded() {
            let kind = agent::get(&self.target)?;
            // The harness decides what it needs to look at; this only performs the read it asks for.
            let readiness = harness::readiness(kind.kind(), |source, format, lines| {
                agent::read(&self.target, source, format, lines)
            })?;
            match readiness {
                Composer::Occupied => {
                    return Err(MsgError::ComposerOccupied { target: self.target });
                }
                // Failing open: delivered without the guarantee, and said so.
                answer => {
                    if let Some(warning) = answer.warning() {
                        sink.warn(warning);
                    }
                }
            }
        }

        let before = agent::get(&self.target)?;
        let proof = self.proof(before.status());
        // Always mail: this command's argument is what a sender wrote, so it always has an author to
        // name. Only `spawn` has a brief, and only `spawn` can deliver one alone.
        let delivery = Delivery::Mail {
            brief: None,
            body: (*self.text).clone(),
        };
        let submission = deliver(&self.target, &delivery, &self.reply(), &proof, sink)?;

        // Said out loud rather than left to the `delivered` field, which only the `--json` reader
        // sees. A caller reading the one line would otherwise treat an unproven dispatch as a
        // started one.
        if !submission.proven && proof != Proof::None {
            sink.warn(&unproven(&self.target));
        }

        Ok(Delivered {
            delivered: Some(submission.proven),
            agent: submission.agent,
        })
    }
}

// =====================================================================================================================
// Delivery
// =====================================================================================================================

/// How one submission's delivery is to be proven.
///
/// An enum rather than the `Option<Wait>` this used to be, because the absent case had grown two
/// meanings that call for opposite handling: a caller who asked for no proof, and a target for which
/// herdr's kind of proof does not exist.
///
/// **A matched wait is not always proof, and telling the two apart is what these four shapes are
/// for.** herdr answers the states it was handed, and a state the *caller* named may be one the
/// target was going to reach regardless: a Codex agent finishing its own startup goes `idle → done`
/// with nothing delivered to it, and a wait for those states matches that transition happily. The
/// default `working` is different in kind — a settled target does not drift into it — so matching
/// that really is proof of this message. Every other shape reads the target's pane for the id the
/// envelope carries, and says so when the pane never showed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Proof {
    /// Nothing is checked, because `--no-verify` asked for nothing.
    None,
    /// herdr waits for `working`, which a settled target does not reach on its own — so matching it
    /// is the proof. The no-`--wait-until` path, unchanged.
    Delivery(Wait),
    /// herdr waits for the states the caller named. Matching proves a transition happened, not that
    /// this message caused it, so the pane supplies the delivery proof instead.
    Settle {
        /// The wait the submission carries, establishing a turn that began after it.
        ///
        /// Absent when the target was already working, which is the branch that keeps the caller's
        /// wait on the submission — there is no settled moment a leg could start from.
        delivery: Option<Wait>,
        /// The caller's states, and the whole operation's budget.
        until: Wait,
    },
    /// The target's pane is read for the id this message carries, and nothing is waited on.
    ///
    /// The case herdr cannot answer. A `--until` state matches only once the agent's state-change
    /// sequence has passed the submission's, and that sequence advances only when the status
    /// actually changes — so an agent that was working before and is working after satisfies
    /// nothing, however long it is given. The message is queued all the same, and the harness
    /// renders it where it can be read.
    Pane {
        /// The caller's budget, which is what bounds the pane poll.
        timeout: u64,
    },
}

impl Proof {
    /// The proof available for an ordinary delivery against a target in `current`.
    ///
    /// Shared with `spawn`, whose first prompt has exactly this problem: an agent that came up
    /// working has no state change left for herdr to match either.
    pub(super) fn for_delivery(current: &str, timeout: u64) -> Self {
        match delivery_leg(current, timeout) {
            Some(leg) => Self::Delivery(leg),
            None => Self::Pane { timeout },
        }
    }

    /// The wait to hand the submission, absent for the two shapes that submit bare.
    ///
    /// An already-working [`Self::Settle`] hands over the caller's own states rather than a
    /// delivery leg. That is the one branch with nothing to establish a fresh turn from, so the
    /// caller's wait rides on the submission — which is the only place herdr will anchor it to the
    /// submission's own sequence.
    fn wait(&self) -> Option<&Wait> {
        match self {
            Self::Delivery(wait) | Self::Settle { delivery: Some(wait), .. } => Some(wait),
            Self::Settle { delivery: None, until } => Some(until),
            Self::None | Self::Pane { .. } => None,
        }
    }

    /// The caller's whole-operation budget, which one [`Deadline`] then spends across every step.
    ///
    /// Zero for [`Self::None`], which times nothing: it submits bare and returns.
    fn budget_ms(&self) -> u64 {
        match self {
            Self::None => 0,
            Self::Delivery(wait) => wait.timeout,
            Self::Settle { until, .. } => until.timeout,
            Self::Pane { timeout } => *timeout,
        }
    }
}

/// The wait a submission against a target in `current` carries to establish a turn of its own.
///
/// `None` for a target that is already working: `--until working` matches only through a state
/// change past the submission's own sequence, and an agent working before and after produces none.
///
/// One function rather than two, so the no-`--wait-until` path and the `delivery` leg of a
/// [`Proof::Settle`] cannot answer the same status differently — which status earns a leg is the
/// rule that must not drift.
///
/// The *ceiling* is the caller's, because that is what genuinely differs: a settle hands over a
/// slice of its budget, and an ordinary delivery has nothing else to spend it on and hands over all
/// of it. Both are then cut again by whatever is left of the [`Deadline`] when the submission runs.
fn delivery_leg(current: &str, timeout: u64) -> Option<Wait> {
    (current != WORKING).then(|| Wait {
        until: vec![WORKING.to_owned()],
        timeout,
    })
}

// ---------------------------------------------------------------------------------------------------------------------
// Deadline
// ---------------------------------------------------------------------------------------------------------------------

/// The caller's whole-operation budget, spent by whichever step asks for it first.
///
/// One deadline covers the submission, the re-send it may need, the pane poll, and the settle wait.
/// Each names its own ceiling and receives whichever is smaller, so a phase budget recomputed per
/// attempt cannot add up past what `--timeout` asked for — which is what two submissions each
/// handed the full timeout used to do.
///
/// A start plus a budget rather than a moment in the future: `Instant + Duration` panics on
/// overflow, and `--timeout` is a `u64` a caller can set to anything.
struct Deadline {
    /// When the budget started being spent.
    start: std::time::Instant,
    /// The whole budget, in milliseconds.
    budget_ms: u64,
}

impl Deadline {
    /// Starts the clock on `budget_ms`.
    fn within(budget_ms: u64) -> Self {
        Self {
            start: std::time::Instant::now(),
            budget_ms,
        }
    }

    /// What is left of the budget, capped at `ceiling`. Zero once it is spent.
    fn share(&self, ceiling: u64) -> u64 {
        let spent = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.budget_ms.saturating_sub(spent).min(ceiling)
    }

    /// `wait` with its timeout cut to what is left, or `None` for a submission that carries none.
    fn share_of(&self, wait: Option<&Wait>) -> Option<Wait> {
        wait.map(|wait| Wait {
            until: wait.until.clone(),
            timeout: self.share(wait.timeout),
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Delivering
// ---------------------------------------------------------------------------------------------------------------------

/// The three herdr operations one delivery drives, all against the same target.
///
/// A trait rather than three direct calls, for the reason [`harness::readiness`] takes its read as
/// a closure: the ordering here — which submission carries which wait, when the pane is read, and
/// whether the settle wait runs at all — is the part worth testing, and automated tests never start
/// a herdr process. The one real impl is a line per method.
pub(super) trait Delivering {
    /// Submits the text, optionally carrying a wait.
    ///
    /// # Errors
    ///
    /// Whatever herdr answered.
    fn prompt(&self, text: &NonEmptyText, wait: Option<&Wait>) -> Result<AgentRecord, HerdrError>;

    /// Reads the target's pane, looking for the id an envelope carries.
    ///
    /// # Errors
    ///
    /// Whatever herdr answered.
    fn read(&self, source: &'static str, lines: u32) -> Result<String, HerdrError>;

    /// Waits for the states a caller named, with no submission attached.
    ///
    /// # Errors
    ///
    /// Whatever herdr answered.
    fn wait(&self, wait: &Wait) -> Result<AgentRecord, HerdrError>;
}

/// The real one: each method is its `herdr::agent` call against the target this was built for.
struct ThroughHerdr<'a> {
    /// The agent all three calls name.
    target: &'a str,
}

impl Delivering for ThroughHerdr<'_> {
    fn prompt(&self, text: &NonEmptyText, wait: Option<&Wait>) -> Result<AgentRecord, HerdrError> {
        agent::prompt(self.target, text, wait)
    }

    fn read(&self, source: &'static str, lines: u32) -> Result<String, HerdrError> {
        agent::read(self.target, source, "text", lines)
    }

    fn wait(&self, wait: &Wait) -> Result<AgentRecord, HerdrError> {
        agent::wait(self.target, wait)
    }
}

/// What one submission puts in the recipient's composer.
///
/// Two variants because two different things get delivered and only one of them is mail. A `<mail>`
/// envelope names who wrote the body and how to answer them; an agent's configured brief has neither
/// — it comes from a file the recipient's own config points at, so there is no sender to name and
/// nothing to reply to. Wrapping it anyway would have the envelope claim that whoever ran `spawn`
/// wrote it, which is the one thing the envelope exists to say truthfully.
///
/// An enum rather than two optional fields, so "neither" cannot be constructed: a spawn with nothing
/// to deliver returns before it gets here.
pub(super) enum Delivery {
    /// A sender's own message, wrapped, optionally behind the recipient's brief.
    ///
    /// One text rather than two submissions: an agent handed two would answer the first before it
    /// heard the second. The brief leads, so the message reads as an instruction about it.
    Mail {
        /// The agent's brief, delivered raw ahead of the envelope. Absent when it has none.
        brief: Option<NonEmptyText>,
        /// What the sender wrote, and the only part an envelope goes around.
        body: NonEmptyText,
    },
    /// A brief with no message behind it, delivered exactly as written.
    ///
    /// Costs the pane proof, which searches for an envelope's id and has none to search for here.
    /// Only against a target that came up already working, though — anything else is proven by the
    /// state change herdr waits on, which is what a freshly started agent gives.
    Brief(NonEmptyText),
}

impl Delivery {
    /// The text to submit, and the id proving it landed when there is an envelope to carry one.
    ///
    /// The `agent get` behind [`Envelope::resolve`] is skipped outright for a brief, which is right
    /// twice over: there is no sender to resolve, and a spawn that delivers only a brief makes one
    /// fewer herdr call than it used to.
    fn compose(&self, reply: &Reply, sink: &Sink) -> (NonEmptyText, Option<String>) {
        match self {
            Self::Brief(brief) => (brief.clone(), None),
            Self::Mail { brief, body } => {
                let envelope = Envelope::resolve(reply, sink);
                let text = after_brief(brief.as_ref(), &envelope.wrap(body));
                (text, Some(envelope.id().to_owned()))
            }
        }
    }
}

/// A rendered envelope behind the brief that introduces it, or on its own when there is none.
///
/// Split out of [`Delivery::compose`] so the composition is testable: the resolution it sits beside
/// makes a herdr call, and automated tests here never start one. A blank line between the two, so a
/// brief ending mid-paragraph does not run into the envelope's opening tag.
///
/// Composed rather than re-parsed: a brief is already non-blank and a wrapping always opens with a
/// tag, so neither half can make the result blank.
fn after_brief(brief: Option<&NonEmptyText>, mail: &str) -> NonEmptyText {
    match brief {
        Some(brief) => NonEmptyText::composed(format!("{brief}\n\n{mail}")),
        None => NonEmptyText::composed(mail.to_owned()),
    }
}

/// What one submission produced: herdr's record of the agent, and whether delivery was proven.
///
/// A named pair rather than a tuple because both halves cross a module boundary, and `spawn` reads
/// them into differently named fields of its own result.
///
/// `Debug` because a test asserting a failure has to name the success type; neither half can carry
/// prompt text, so the derive cannot leak one.
#[derive(Debug)]
pub(super) struct Submission {
    /// herdr's record of the target after the submission.
    pub agent: AgentRecord,
    /// Whether delivery was actually proven, rather than merely attempted.
    pub proven: bool,
}

/// Looks for `id` in the target's pane until it appears or the poll window closes.
///
/// This is the proof for a target that was already working. The harness renders a queued message
/// where a reader can see it — Claude Code expands the whole body into its transcript, Codex lists it
/// under a "messages to be submitted" banner — and both render the opening tag that carries the id.
///
/// The window is sized to the message, because the message is what pushed the transcript along: a
/// hundred-line dispatch scrolls its own opening out of any fixed window. herdr clamps the request
/// at [`DELIVERY_MAX_LINES`] regardless, so a message longer than that is read from its tail and the
/// id may genuinely be gone — reported as unproven, which is the honest answer.
///
/// Never an error. A snapshot that cannot be read, or a harness whose rendering this build does not
/// recognize, leaves delivery unproven rather than failing a prompt that did land — the same stance
/// the composer guard takes when it cannot see what it needs.
/// Takes the read as a closure for the reason [`harness::readiness`] does: it is what lets the whole
/// decision be exercised without a herdr process, per the rule that automated tests never start one.
///
/// `poll_ms` is this step's share of the caller's budget rather than a fixed window, so a short
/// `--timeout` bounds the looking too.
fn confirm_in_pane<E>(
    id: &str,
    text: &NonEmptyText,
    poll_ms: u64,
    read: impl Fn(&'static str, u32) -> Result<String, E>,
) -> bool {
    let lines = delivery_window(text.lines().count());
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(poll_ms);

    loop {
        match read(DELIVERY_SOURCE, lines) {
            // The id is this build's own six digits, so a hit is this message rather than a
            // quotation of an older one. The snapshot itself is never reported anywhere: it is the
            // recipient's screen, and the prompt rule covers captured terminal content outright.
            Ok(snapshot) if snapshot.contains(id) => return true,
            Ok(_) => {}
            // A read that failed says nothing about the prompt, and the answer here is the same
            // either way. herdr's own message is not restated, because nothing acts on it.
            Err(_) => return false,
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(DELIVERY_INTERVAL_MS));
    }
}

/// How many rows to read back when looking for a delivered message.
///
/// Sized to the message, because the message is what pushed the transcript along: a hundred-line
/// dispatch scrolls its own opening tag out of any fixed window, and the opening tag is where the id
/// rides. herdr clamps a read at a thousand rows whatever is asked for, so a message longer than that
/// is read from its tail and its id may genuinely be gone — reported unproven, which is honest.
/// Takes the line count rather than the message, which is all it is about. RS-030 would otherwise
/// make this a method on `NonEmptyText`, dragging this command's read policy into `core` — the
/// module the style guide keeps free of every other module's vocabulary.
fn delivery_window(message_lines: usize) -> u32 {
    u32::try_from(message_lines)
        .unwrap_or(DELIVERY_MAX_LINES)
        .saturating_add(DELIVERY_MARGIN)
        .min(DELIVERY_MAX_LINES)
}

/// Submits a prompt, proves what can be proven, and waits out the settle a caller asked for.
///
/// Shared with `spawn`, whose first prompt goes through exactly this path. The sink comes last: it
/// is the channel a warning is reported through, not the thing being acted on.
///
/// # Errors
///
/// [`MsgError::Herdr`] for anything herdr refused outright, and [`MsgError::Stalled`] when two
/// submissions both failed to move the agent.
pub(super) fn deliver(
    target: &str,
    delivery: &Delivery,
    reply: &Reply,
    proof: &Proof,
    sink: &Sink,
) -> Result<Submission, MsgError> {
    // Composed once, before the re-send: a second submission must deliver the same bytes as the
    // first, and re-composing would make a second `agent get` call to say so — and mint a second id,
    // leaving the pane check hunting for a token no delivered copy carries.
    let (text, id) = delivery.compose(reply, sink);

    deliver_through(&ThroughHerdr { target }, target, &text, id.as_deref(), proof, sink)
}

/// Everything [`deliver`] does once the text is composed, over operations a test can supply.
///
/// The composition stays outside: resolving an envelope's sender is itself a herdr call, and what
/// is worth exercising here is the ordering rather than the text.
///
/// The ordering is the design, and the two [`Proof::Settle`] branches are deliberately not
/// symmetric.
///
/// **A settled target** submits carrying its delivery leg, so the turn herdr then reports on is one
/// that began after the submission. The pane is read for the envelope's id, and only then is the
/// caller's settle wait issued as a standalone `agent wait` — which is safe here precisely because
/// the leg already moved the target off the status it was sitting on.
///
/// **An already-working target** keeps the caller's wait on the submission instead, because there
/// is no settled moment a leg could start from. That branch waits out *the turn in progress*, and
/// that is all it can claim: herdr's anchor requires only that the matched status carry a higher
/// sequence than the submission, which the already-running turn satisfies the moment it ends.
/// Nothing reachable through this CLI can tell that ending from the queued message's own. The cost
/// is the one this shape otherwise removes — a turn long enough to scroll the opening tag past
/// herdr's thousand-row clamp reports a delivered message as unproven — and that is the better
/// trade than a wait that lies.
///
/// A submission herdr reports as undelivered is re-sent once: a prompt sent within a few seconds of
/// starting an agent is silently swallowed, and waiting and re-sending worked in every observed
/// case. Only that one code, per [`HerdrError::is_undelivered`] — herdr submits before it waits, so
/// any other wait failure describes a prompt that landed, and re-sending on `timeout` was
/// delivering every such message twice. The settle wait runs once, after whichever submission
/// succeeded, and never twice.
///
/// A submission that failed outright returns that failure, so an exhausted delivery leg cannot
/// become a successful settle because the pane happened to show the id.
///
/// # Errors
///
/// [`MsgError::Herdr`] for anything herdr refused outright, and [`MsgError::Stalled`] when two
/// submissions both failed to move the agent.
fn deliver_through<D: Delivering>(
    delivering: &D,
    target: &str,
    text: &NonEmptyText,
    id: Option<&str>,
    proof: &Proof,
    sink: &Sink,
) -> Result<Submission, MsgError> {
    let deadline = Deadline::within(proof.budget_ms());

    let agent = submit(delivering, target, text, proof.wait(), &deadline, sink)?;

    // A matched delivery wait is proof on its own: `working` is a status a settled target does not
    // reach without something arriving. Every other shape asks the pane.
    let proven = match (proof, id) {
        (Proof::None, _) => false,
        (Proof::Delivery(_), _) => true,
        (Proof::Pane { .. } | Proof::Settle { .. }, Some(id)) => {
            confirm_in_pane(id, text, deadline.share(DELIVERY_POLL_MS), |source, lines| {
                delivering.read(source, lines)
            })
        }
        // An unattributed brief carries no id, so there is nothing in the pane to match. Unproven is
        // the honest answer, and the same one a snapshot that could not be read produces.
        (Proof::Pane { .. } | Proof::Settle { .. }, None) => false,
    };

    // The one branch whose settle wait did not ride on the submission. herdr's answer supersedes the
    // submission's record, because it is the later reading of the same agent.
    let agent = match proof {
        Proof::Settle { delivery: Some(_), until } => delivering.wait(&Wait {
            until: until.until.clone(),
            timeout: deadline.share(until.timeout),
        })?,
        Proof::None | Proof::Delivery(_) | Proof::Pane { .. } | Proof::Settle { delivery: None, .. } => agent,
    };

    Ok(Submission { agent, proven })
}

/// Submits `text`, re-sending once if herdr reports it did not land.
///
/// Both attempts draw on the same deadline, so the second is given what is left rather than a fresh
/// copy of the caller's timeout — two submissions each handed the full budget spent twice what was
/// asked for.
///
/// # Errors
///
/// [`MsgError::Stalled`] when both submissions failed to move the agent, [`MsgError::Herdr`] for
/// anything else herdr refused.
fn submit<D: Delivering>(
    delivering: &D,
    target: &str,
    text: &NonEmptyText,
    wait: Option<&Wait>,
    deadline: &Deadline,
    sink: &Sink,
) -> Result<AgentRecord, MsgError> {
    let first = deadline.share_of(wait);

    match delivering.prompt(text, first.as_ref()) {
        Ok(agent) => Ok(agent),
        Err(error) if error.is_undelivered() && repair_available(first.as_ref()) => {
            sink.warn(&format!("{target} did not acknowledge the prompt; re-sending once"));
            let second = deadline.share_of(wait);
            delivering.prompt(text, second.as_ref()).map_err(|error| {
                if error.is_undelivered() {
                    MsgError::Stalled { target: target.to_owned() }
                } else {
                    MsgError::Herdr(error)
                }
            })
        }
        Err(error) => Err(MsgError::Herdr(error)),
    }
}

/// Whether a submission carrying `wait` could produce the stall the re-send repair keys on.
///
/// Two ways it cannot, and both are herdr's rather than ours. A bare submission never waits, and
/// `agent_prompt_stalled` is only ever produced by the wait path — herdr returns early when a
/// prompt carries no wait. And a wait of [`STALL_REPORTED_ABOVE_MS`] or less is answered with a bare
/// `timeout` instead, which is not [`HerdrError::is_undelivered`] and so would not re-send anyway.
///
/// Stated here rather than left to herdr's answer, because it is the caller's leash that decides it:
/// a short `--timeout` buys a short delivery leg and gives up the repair with it, and a re-send that
/// fired outside that contract would spend budget the caller did not offer.
fn repair_available(wait: Option<&Wait>) -> bool {
    wait.is_some_and(|wait| wait.timeout > STALL_REPORTED_ABOVE_MS)
}

/// What a caller is told when nothing proved the message landed.
///
/// Names no status. It used to say the target "was already working", which was the only way to
/// reach this line before and is false now — a settled target whose pane never showed the id
/// reaches it too. What is actually known is all it says.
fn unproven(target: &str) -> String {
    format!("{target}'s pane never showed the message, so delivery is unproven")
}

// =====================================================================================================================
// Output
// =====================================================================================================================

/// What `msg` produced: whether delivery was proven, and herdr's record of the agent.
#[derive(Debug, Serialize)]
pub struct Delivered {
    /// Whether delivery was actually proven, rather than merely attempted.
    ///
    /// `false` under `--no-verify`, and `false` whenever the proof this message could offer was the
    /// pane and the pane never showed the id. Two ways to be in that position: the target was
    /// already `working`, so no state change was left for herdr to match, or the caller named its
    /// own `--wait-until` states — which prove a transition happened and never that this message
    /// caused it. Absent when there was no prompt to deliver, which is `spawn`'s case.
    #[serde(skip_serializing_if = "Option::is_none")]
    delivered: Option<bool>,
    /// herdr's agent record, nested verbatim.
    agent: AgentRecord,
}

impl Display for Delivered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "messaged {} ({})", self.agent.pane(), self.agent.status())
    }
}

// =====================================================================================================================
// Errors
// =====================================================================================================================

/// Failure of the delivery flow.
#[derive(Debug, Error)]
pub enum MsgError {
    /// herdr refused something; its message and code are carried verbatim.
    #[error(transparent)]
    Herdr(#[from] HerdrError),
    /// The target's composer holds unsent text.
    ///
    /// Carries the target and nothing else. What the guard read is someone's half-written message,
    /// so there is deliberately no field here that could hold it.
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

impl AsExitStatus for MsgError {
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
        args: MsgArgs,
    }

    fn parse(argv: &[&str]) -> MsgArgs {
        Harness::try_parse_from(argv).expect("parses").args
    }

    fn record() -> AgentRecord {
        serde_json::from_str(r#"{"agent":"claude","agent_status":"working","pane_id":"w4:p17"}"#).unwrap()
    }

    /// A wait for `working` with the given ceiling, which is what a delivery leg always is.
    fn leg(timeout: u64) -> Wait {
        Wait {
            until: vec![WORKING.to_owned()],
            timeout,
        }
    }

    #[test]
    fn the_default_proof_is_a_wait_for_the_status_moving_to_working() {
        let args = parse(&["msg", "reviewer", "ship it"]);

        assert_eq!(args.proof("idle"), Proof::Delivery(leg(15_000)));
    }

    /// The bug this shape exists to prevent.
    ///
    /// herdr matches a `--until` state only once the state-change sequence has passed the
    /// submission's, and that sequence advances only on a real status change. An agent working
    /// before and after never satisfies `--until working`, so asking spends the whole timeout and
    /// then calls a delivered prompt undelivered.
    #[test]
    fn a_target_that_is_already_working_is_proven_by_its_pane_rather_than_by_a_wait() {
        let args = parse(&["msg", "reviewer", "ship it"]);

        assert_eq!(
            args.proof(WORKING),
            Proof::Pane { timeout: 15_000 },
            "no state change is left to observe"
        );
        for settled in ["idle", "blocked", "done"] {
            assert!(matches!(args.proof(settled), Proof::Delivery(_)), "{settled}");
        }
    }

    /// `spawn`'s first prompt takes the same route, so the rule has one owner.
    #[test]
    fn the_delivery_proof_is_chosen_the_same_way_wherever_it_is_asked_for() {
        assert_eq!(Proof::for_delivery(WORKING, 10_000), Proof::Pane { timeout: 10_000 });
        assert_eq!(Proof::for_delivery("idle", 10_000), Proof::Delivery(leg(10_000)));
    }

    /// A settle's leg takes a delivery's budget out of the turn's; an ordinary delivery takes it all.
    #[test]
    fn a_settles_leg_is_capped_at_a_delivery_budget_and_an_ordinary_delivery_keeps_the_whole_timeout() {
        let settling = parse(&["msg", "reviewer", "go", "--timeout", "120000", "--wait-until", "done"]);
        let Proof::Settle { delivery, until } = settling.proof("idle") else {
            panic!("an explicit --wait-until is a settle");
        };
        assert_eq!(delivery, Some(leg(DEFAULT_TIMEOUT_MS)), "a slice, not the whole turn");
        assert_eq!(until.timeout, 120_000, "the rest is the turn's");

        // Nothing else to leave any for, so this path is the one it always was.
        let plain = parse(&["msg", "reviewer", "go", "--timeout", "120000"]);
        assert_eq!(plain.proof("idle"), Proof::Delivery(leg(120_000)));
    }

    /// One function behind both, so the two paths cannot answer the same status differently.
    #[test]
    fn the_settle_legs_delivery_wait_is_the_same_one_an_ordinary_delivery_would_have_used() {
        let args = parse(&["msg", "reviewer", "go", "--wait-until", "idle", "--wait-until", "done"]);

        for status in ["idle", "blocked", "done", WORKING] {
            let Proof::Settle { delivery, .. } = args.proof(status) else {
                panic!("an explicit --wait-until is a settle, whatever the status");
            };
            assert_eq!(delivery.is_some(), delivery_leg(status, 15_000).is_some(), "{status}");
        }
    }

    /// The delivery leg is present against a settled target and absent against a working one.
    #[test]
    fn a_settle_carries_a_delivery_leg_only_when_there_is_a_settled_moment_to_start_it_from() {
        let args = parse(&["msg", "reviewer", "go", "--wait-until", "idle", "--wait-until", "done"]);

        assert_eq!(
            args.proof("idle"),
            Proof::Settle {
                delivery: Some(leg(15_000)),
                until: Wait {
                    until: vec!["idle".to_owned(), "done".to_owned()],
                    timeout: 15_000,
                },
            }
        );
        assert_eq!(
            args.proof(WORKING),
            Proof::Settle {
                delivery: None,
                until: Wait {
                    until: vec!["idle".to_owned(), "done".to_owned()],
                    timeout: 15_000,
                },
            },
            "no settled moment, so the caller's wait rides on the submission instead"
        );
    }

    /// What each of the four shapes hands the submission.
    #[test]
    fn only_the_two_shapes_with_something_to_establish_hand_the_submission_a_wait() {
        assert!(Proof::None.wait().is_none());
        assert!(Proof::Pane { timeout: 10_000 }.wait().is_none());
        assert_eq!(Proof::for_delivery("idle", 10_000).wait(), Some(&leg(10_000)));

        let args = parse(&["msg", "reviewer", "go", "--wait-until", "done"]);
        let settled = args.proof("idle");
        assert_eq!(settled.wait(), Some(&leg(15_000)), "the leg, not the caller's states");

        // The already-working branch is the exception: with no leg to send, the caller's own states
        // ride on the submission, which is the only place herdr anchors them to it.
        let working = args.proof(WORKING);
        assert_eq!(
            working.wait(),
            Some(&Wait {
                until: vec!["done".to_owned()],
                timeout: 15_000,
            })
        );
    }

    /// One deadline covers every step, and it is the caller's own `--timeout`.
    #[test]
    fn every_shape_spends_the_callers_timeout_and_nothing_else() {
        let args = parse(&["msg", "reviewer", "go", "--timeout", "42000"]);
        assert_eq!(args.proof("idle").budget_ms(), 42_000);
        assert_eq!(args.proof(WORKING).budget_ms(), 42_000);

        let settling = parse(&["msg", "reviewer", "go", "--timeout", "42000", "--wait-until", "done"]);
        assert_eq!(settling.proof("idle").budget_ms(), 42_000);
        assert_eq!(settling.proof(WORKING).budget_ms(), 42_000);

        assert_eq!(Proof::None.budget_ms(), 0, "nothing is timed");
    }

    /// An explicit `--wait-until` is a question about a transition, not about delivery.
    #[test]
    fn an_explicit_wait_until_is_honored_even_against_a_working_target() {
        // `--wait-until idle` against a working agent is a request to wait out the turn, and that
        // transition really does happen — so dropping it here would break the settle wait.
        let args = parse(&["msg", "reviewer", "go", "--wait-until", "idle"]);

        let Proof::Settle { until, .. } = args.proof(WORKING) else {
            panic!("an explicit --wait-until is a settle");
        };
        assert_eq!(until.until, ["idle"]);
    }

    #[test]
    fn wait_until_replaces_the_states_for_a_caller_that_wants_the_full_settle_wait() {
        let args = parse(&[
            "msg",
            "reviewer",
            "go",
            "--wait-until",
            "idle",
            "--wait-until",
            "blocked",
        ]);

        let Proof::Settle { until, .. } = args.proof("idle") else {
            panic!("an explicit --wait-until is a settle");
        };
        assert_eq!(until.until, ["idle", "blocked"]);
    }

    #[test]
    fn no_verify_skips_the_wait_and_force_does_not() {
        // The two flags are not interchangeable and neither implies the other: --force skips the
        // composer guard, --no-verify skips the delivery wait.
        assert_eq!(
            parse(&["msg", "reviewer", "go", "--no-verify"]).proof("idle"),
            Proof::None
        );
        assert!(matches!(
            parse(&["msg", "reviewer", "go", "--force"]).proof("idle"),
            Proof::Delivery(_)
        ));
        assert!(!parse(&["msg", "reviewer", "go", "--force"]).guarded());
        assert!(parse(&["msg", "reviewer", "go", "--no-verify"]).guarded());
    }

    #[test]
    fn a_prompt_addresses_its_reply_at_the_sender_by_default() {
        assert_eq!(parse(&["msg", "reviewer", "go"]).reply(), Reply::ToSender);
    }

    #[test]
    fn no_reply_produces_a_message_that_closes_the_loop() {
        // The tail hands the replier this flag, so termination needs no memory: the replier runs the
        // line it was given rather than recalling a convention.
        assert_eq!(parse(&["msg", "reviewer", "done", "--no-reply"]).reply(), Reply::None);
    }

    #[test]
    fn reply_to_addresses_a_collector_rather_than_the_sender() {
        assert_eq!(
            parse(&["msg", "worker", "go", "--reply-to", "collector"]).reply(),
            Reply::To("collector".to_owned())
        );
    }

    #[test]
    fn no_reply_and_reply_to_are_refused_together_rather_than_one_silently_winning() {
        // One names an address the other deletes, so passing both is a caller that has not decided.
        // clap answers this with its own usage code, which is the 2 this crate's contract already uses.
        assert!(Harness::try_parse_from(["msg", "reviewer", "go", "--no-reply", "--reply-to", "collector"]).is_err());
    }

    #[test]
    fn the_reply_flags_are_independent_of_the_delivery_flags() {
        // --no-reply shapes the message; --no-verify skips the delivery wait; --force skips the
        // composer guard. No one of them implies another.
        let args = parse(&["msg", "reviewer", "go", "--no-reply"]);
        assert!(matches!(args.proof("idle"), Proof::Delivery(_)));
        assert!(args.guarded());
    }

    #[test]
    fn the_guard_runs_unless_force_says_otherwise() {
        assert!(parse(&["msg", "reviewer", "go"]).guarded());
        assert!(!parse(&["msg", "reviewer", "go", "--force"]).guarded());
    }

    /// A prompt that opens with a dash is delivered rather than rejected.
    ///
    /// The parse is the redaction. A value clap accepts is a value no clap diagnostic can repeat,
    /// and repeating it is what the prompt rule forbids — so this test is the leak test too.
    #[test]
    fn a_prompt_that_opens_with_a_dash_is_prompt_text_rather_than_a_flag() {
        for text in ["--force the issue", "-e", "--not-a-flag-here", "-- leading separator"] {
            assert_eq!(parse(&["msg", "reviewer", text]).text.to_string(), text, "{text}");
        }

        // The two limits, both of them clap's and neither of them a leak. A token that *exactly*
        // matches a declared flag is still that flag, and a bare `--` is still the value terminator.
        // Both are repaired the same way, and the repair is what `--` is for: everything after it is
        // the prompt.
        assert_eq!(parse(&["msg", "reviewer", "--", "--force"]).text.to_string(), "--force");
        assert_eq!(parse(&["msg", "reviewer", "--", "--"]).text.to_string(), "--");
    }

    #[test]
    fn a_flag_after_the_prompt_is_still_a_flag() {
        // The other half of `allow_hyphen_values`: it must claim the prompt's own value and nothing
        // past it, or every flag on this command stops working.
        let args = parse(&["msg", "reviewer", "--go", "--wait-until", "idle", "--force"]);

        assert_eq!(args.text.to_string(), "--go");
        assert_eq!(args.wait_until, ["idle"]);
        assert!(!args.guarded());
    }

    #[test]
    fn a_blank_prompt_is_refused_at_parse_time() {
        assert!(Harness::try_parse_from(["msg", "reviewer", "   "]).is_err());
    }

    /// A brief is not mail, so it is delivered exactly as written.
    ///
    /// This arm reaches no herdr call at all, which is what lets it be exercised here: there is no
    /// sender to resolve, because nobody sent it. The reply is passed in anyway to pin that it makes
    /// no difference — an envelope is what a reply address rides on, and there is none.
    #[test]
    fn a_brief_is_delivered_unwrapped_and_carries_no_id_to_be_proven_by() {
        let brief = "You review Rust.".parse::<NonEmptyText>().unwrap();

        for reply in [Reply::ToSender, Reply::None, Reply::To("collector".to_owned())] {
            let (text, id) = Delivery::Brief(brief.clone()).compose(&reply, &Sink::new(crate::core::OutputMode::Human));

            assert_eq!(text.to_string(), "You review Rust.", "{reply:?}");
            assert!(id.is_none(), "nothing to search a pane for");
            assert!(!text.to_string().contains("<mail"), "{reply:?}");
            assert!(!text.to_string().contains("<how-to-reply>"), "{reply:?}");
        }
    }

    /// The brief leads, so the message reads as an instruction about it.
    #[test]
    fn a_brief_sits_ahead_of_the_envelope_with_a_blank_line_between_them() {
        let brief = "You review Rust.".parse::<NonEmptyText>().unwrap();
        let mail = "<mail from=\"operator\" id=\"k7m2x9\">\nstart with auth\n</mail>";

        assert_eq!(
            after_brief(Some(&brief), mail).to_string(),
            format!("You review Rust.\n\n{mail}")
        );
        assert_eq!(after_brief(None, mail).to_string(), mail);
    }

    /// The window is sized to the message, which is what scrolled the transcript in the first place.
    #[test]
    fn the_read_window_covers_the_message_plus_room_for_the_harness_furniture() {
        assert_eq!(delivery_window(1), 1 + DELIVERY_MARGIN);
        assert_eq!(delivery_window(100), 100 + DELIVERY_MARGIN);

        // herdr clamps a read at a thousand rows whatever is asked for, so asking for more would
        // only misreport how much was actually looked at.
        assert_eq!(delivery_window(5_000), DELIVERY_MAX_LINES);
        assert_eq!(
            delivery_window(usize::MAX),
            DELIVERY_MAX_LINES,
            "no wrap on the way past u32"
        );
    }

    /// The id in the snapshot is the proof, and it is read from the source that can actually hold it.
    #[test]
    fn a_pane_showing_the_id_is_delivery_proven() {
        let text = NonEmptyText::composed("<mail from=\"dispatcher\" id=\"k7m2x9\">\ngo\n</mail>".to_owned());
        let asked = std::cell::RefCell::new(Vec::new());

        let found = confirm_in_pane("k7m2x9", &text, DELIVERY_POLL_MS, |source, lines| {
            asked.borrow_mut().push((source, lines));
            Ok::<_, ()>("… transcript …\n  <mail from=\"dispatcher\" id=\"k7m2x9\">\n  go\n".to_owned())
        });

        assert!(found);
        assert_eq!(
            asked.into_inner(),
            [("recent-unwrapped", 3 + DELIVERY_MARGIN)],
            "one read, from the source that rejoins wrapped rows"
        );
    }

    /// A read this build cannot make leaves delivery unproven rather than failing a landed prompt.
    #[test]
    fn a_pane_that_cannot_be_read_is_unproven_rather_than_an_error() {
        let text = NonEmptyText::composed("<mail from=\"w4:p2\" id=\"abc123\">\ngo\n</mail>".to_owned());

        let unreadable = |_: &str, _: u32| Err::<String, ()>(());

        assert!(!confirm_in_pane("abc123", &text, DELIVERY_POLL_MS, unreadable));
    }

    /// An id belonging to some earlier message is not this message's proof.
    ///
    /// The id is what is matched, not the tag around it — a pane full of mail from the same sender
    /// proves nothing about the message just sent.
    #[test]
    fn a_pane_holding_only_an_older_message_is_not_proof_of_this_one() {
        let text = NonEmptyText::composed("<mail from=\"w\" id=\"newone\">\ngo\n</mail>".to_owned());
        let stale = "  <mail from=\"w\" id=\"oldone\">\n  an earlier dispatch\n";
        let looked = std::cell::Cell::new(0_u32);

        // The second look fails rather than missing again, which ends the poll after one interval.
        // Spending the whole window here would put three seconds into every run of the suite to
        // re-assert what the first miss already showed.
        let found = confirm_in_pane("newone", &text, DELIVERY_POLL_MS, |_, _| {
            looked.set(looked.get() + 1);
            if looked.get() == 1 {
                Ok(stale.to_owned())
            } else {
                Err(())
            }
        });

        assert!(!found);
        assert_eq!(looked.get(), 2, "the first miss should not have been the last look");
    }

    // -----------------------------------------------------------------------------------------------------------------
    // The delivery seam
    // -----------------------------------------------------------------------------------------------------------------

    /// A [`Delivering`] answering from a script, counting what it was asked to do.
    ///
    /// The ordering is what these tests are about — which submission carried which wait, whether the
    /// pane was read, and whether the settle wait ran — so this records calls rather than modelling
    /// an agent.
    struct Scripted {
        /// One answer per submission, taken in order.
        answers: std::cell::RefCell<std::collections::VecDeque<Result<AgentRecord, HerdrError>>>,
        /// The wait each submission carried.
        submitted: std::cell::RefCell<Vec<Option<Wait>>>,
        /// Every standalone wait issued, which must be at most one.
        settled: std::cell::RefCell<Vec<Wait>>,
        /// How many times the pane was read.
        reads: std::cell::Cell<u32>,
        /// What the pane shows, or `None` for a pane that never shows the id.
        pane: Option<&'static str>,
    }

    impl Scripted {
        /// A double whose submissions answer `answers` in order.
        fn new(answers: Vec<Result<AgentRecord, HerdrError>>, pane: Option<&'static str>) -> Self {
            Self {
                answers: std::cell::RefCell::new(answers.into()),
                submitted: std::cell::RefCell::new(Vec::new()),
                settled: std::cell::RefCell::new(Vec::new()),
                reads: std::cell::Cell::new(0),
                pane,
            }
        }
    }

    impl Delivering for Scripted {
        fn prompt(&self, _text: &NonEmptyText, wait: Option<&Wait>) -> Result<AgentRecord, HerdrError> {
            self.submitted.borrow_mut().push(wait.cloned());
            self.answers.borrow_mut().pop_front().expect("a scripted answer")
        }

        fn read(&self, _source: &'static str, _lines: u32) -> Result<String, HerdrError> {
            self.reads.set(self.reads.get() + 1);
            // A miss on the first look and a failure after it, so a pane that never shows the id
            // ends the poll in one interval rather than spending the whole window in every run.
            match self.pane {
                Some(snapshot) => Ok(snapshot.to_owned()),
                None if self.reads.get() == 1 => Ok("… transcript …".to_owned()),
                None => Err(refusal("agent_read_failed")),
            }
        }

        fn wait(&self, wait: &Wait) -> Result<AgentRecord, HerdrError> {
            self.settled.borrow_mut().push(wait.clone());
            Ok(record())
        }
    }

    /// A herdr refusal carrying `code`, for scripting a submission's answer.
    fn refusal(code: &str) -> HerdrError {
        HerdrError::Refused {
            command: "agent prompt".to_owned(),
            code: code.to_owned(),
            message: "…".to_owned(),
        }
    }

    /// The id every scripted delivery carries, so a pane can be written to hold it or not.
    const SCRIPTED_ID: &str = "k7m2x9";

    /// `deliver_through` against a double, with a body short enough to keep the read window small.
    ///
    /// The text is handed over already composed, which is what keeps this out of herdr: resolving an
    /// envelope's sender is an `agent get` of its own.
    fn delivered_through(scripted: &Scripted, proof: &Proof) -> Result<Submission, MsgError> {
        deliver_through(
            scripted,
            "reviewer",
            &NonEmptyText::composed(format!("<mail from=\"operator\" id=\"{SCRIPTED_ID}\">\ngo\n</mail>")),
            Some(SCRIPTED_ID),
            proof,
            &Sink::new(crate::core::OutputMode::Human),
        )
    }

    /// A settle that names the caller's states, against a target in `current`.
    fn settling(current: &str, timeout: u64) -> Proof {
        parse(&[
            "msg",
            "reviewer",
            "go",
            "--timeout",
            &timeout.to_string(),
            "--wait-until",
            "idle",
            "--wait-until",
            "done",
        ])
        .proof(current)
    }

    /// The ordinary case: one submission carrying the leg, and nothing else asked of herdr.
    #[test]
    fn a_submission_that_lands_is_the_only_call_an_ordinary_delivery_makes() {
        let scripted = Scripted::new(vec![Ok(record())], None);

        let submission = delivered_through(&scripted, &Proof::for_delivery("idle", 15_000)).unwrap();

        assert!(submission.proven, "a matched delivery wait is the proof");
        assert_eq!(scripted.submitted.borrow().len(), 1);
        assert_eq!(scripted.reads.get(), 0, "nothing to look for");
        assert!(scripted.settled.borrow().is_empty());
    }

    /// The repair: a submission herdr saw no effect from is sent once more.
    #[test]
    fn a_stalled_submission_is_re_sent_once_and_the_second_one_stands() {
        let scripted = Scripted::new(vec![Err(refusal("agent_prompt_stalled")), Ok(record())], None);

        let submission = delivered_through(&scripted, &Proof::for_delivery("idle", 15_000)).unwrap();

        assert!(submission.proven);
        assert_eq!(scripted.submitted.borrow().len(), 2, "sent again, not given up on");
    }

    /// Two stalls is a retryable conflict rather than a silent success.
    #[test]
    fn two_stalled_submissions_are_a_conflict_and_there_is_no_third() {
        let scripted = Scripted::new(
            vec![
                Err(refusal("agent_prompt_stalled")),
                Err(refusal("agent_prompt_stalled")),
            ],
            None,
        );

        let error = delivered_through(&scripted, &Proof::for_delivery("idle", 15_000)).unwrap_err();

        assert_eq!(error.exit_status(), ExitStatus::Conflict);
        assert!(matches!(error, MsgError::Stalled { .. }), "{error:?}");
        assert_eq!(scripted.submitted.borrow().len(), 2, "re-sent once, never twice");
    }

    /// The boundary herdr draws, and the contract `--help` states because of it.
    ///
    /// At or below five seconds herdr answers a bare `timeout` rather than `agent_prompt_stalled`,
    /// so the repair cannot fire — a caller who asked for a short leash has given it up. Pinned
    /// either side of the boundary, because "five seconds exactly" is the losing side.
    #[test]
    fn the_re_send_repair_stops_at_the_five_second_boundary_herdr_draws() {
        assert!(
            !repair_available(None),
            "a bare submission never waits, so it never stalls"
        );
        assert!(!repair_available(Some(&leg(STALL_REPORTED_ABOVE_MS))));
        assert!(repair_available(Some(&leg(STALL_REPORTED_ABOVE_MS + 1))));

        // And the branch that reads it: a stall reported under a short leash is not re-sent.
        let scripted = Scripted::new(vec![Err(refusal("agent_prompt_stalled"))], None);
        let error = delivered_through(&scripted, &Proof::for_delivery("idle", STALL_REPORTED_ABOVE_MS)).unwrap_err();

        assert!(matches!(error, MsgError::Herdr(_)), "{error:?}");
        assert_eq!(scripted.submitted.borrow().len(), 1, "no repair to run");
    }

    /// The settled branch: leg, then pane, then the caller's states — each exactly once.
    #[test]
    fn a_settle_against_a_settled_target_sends_the_leg_reads_the_pane_and_then_waits_once() {
        let scripted = Scripted::new(vec![Ok(record())], Some("… <mail from=\"operator\" id=\"k7m2x9\">"));
        let proof = settling("idle", 60_000);
        let Proof::Settle { delivery: Some(_), .. } = &proof else {
            panic!("a settled target gets a leg");
        };

        let submission = delivered_through(&scripted, &proof).unwrap();

        assert_eq!(
            scripted.submitted.borrow().as_slice(),
            [Some(leg(DEFAULT_TIMEOUT_MS))],
            "the submission carries the leg, never the caller's states"
        );
        assert_eq!(scripted.reads.get(), 1, "the pane is what proves this one");
        assert_eq!(scripted.settled.borrow().len(), 1, "issued once, after delivery");
        assert_eq!(scripted.settled.borrow()[0].until, ["idle", "done"]);
        assert!(submission.proven, "the pane showed the id this message carries");
    }

    /// A pane that never shows the id leaves a settle unproven, and the settle wait still runs.
    ///
    /// The caller asked two things — deliver this, and tell me when the turn ends — and the second
    /// is still honestly answerable when the first cannot be shown.
    #[test]
    fn a_settle_whose_pane_never_showed_the_id_is_unproven_and_still_waits_out_the_turn() {
        let scripted = Scripted::new(vec![Ok(record())], None);

        let submission = delivered_through(&scripted, &settling("idle", 60_000)).unwrap();

        assert!(!submission.proven);
        assert!(scripted.reads.get() >= 2, "the first miss was not the last look");
        assert_eq!(scripted.settled.borrow().len(), 1);
    }

    /// The already-working branch keeps today's shape: the caller's wait rides on the submission.
    #[test]
    fn a_settle_against_a_working_target_rides_the_callers_wait_on_the_submission_and_waits_no_more() {
        let scripted = Scripted::new(vec![Ok(record())], None);

        let submission = delivered_through(&scripted, &settling(WORKING, 60_000)).unwrap();

        let submitted = scripted.submitted.borrow();
        assert_eq!(submitted.len(), 1);
        assert_eq!(
            submitted[0].as_ref().expect("a wait rode along").until,
            ["idle", "done"],
            "no settled moment to start a leg from, so the states go on the submission"
        );
        assert!(scripted.settled.borrow().is_empty(), "already waited for");
        assert!(!submission.proven, "the pane never showed the id");
    }

    /// Finding 1: a matched settle wait is not proof that this message was delivered.
    ///
    /// The reported failure. A Codex agent messaged while its MCP servers were still starting
    /// transitioned `idle → done` on its own, herdr matched the wait, and the caller was told
    /// `messaged wJ:p1T (done)` with exit 0 — for a message whose target's pane held no `<mail>`
    /// element at all.
    #[test]
    fn a_settle_wait_that_matched_is_not_delivery_proof_when_the_pane_never_showed_the_message() {
        for current in ["idle", WORKING] {
            let scripted = Scripted::new(vec![Ok(record())], None);

            let submission = delivered_through(&scripted, &settling(current, 2_000)).unwrap();

            assert!(
                !submission.proven,
                "{current}: the wait matched a transition, which is not this message"
            );
        }
    }

    /// An exhausted delivery leg cannot become a successful settle.
    ///
    /// The caller asked to be told when the turn ended. A submission that failed says nothing about
    /// a turn, so the failure is what comes back — even though the pane check might still have found
    /// the id, and even though the standalone wait would probably have matched something.
    #[test]
    fn a_delivery_leg_that_timed_out_returns_the_timeout_rather_than_falling_through_to_the_settle() {
        let scripted = Scripted::new(vec![Err(refusal("timeout"))], Some("id=\"k7m2x9\""));

        let error = delivered_through(&scripted, &settling("idle", 60_000)).unwrap_err();

        assert!(matches!(error, MsgError::Herdr(_)), "{error:?}");
        assert_eq!(
            scripted.reads.get(),
            0,
            "nothing to prove about a submission that failed"
        );
        assert!(scripted.settled.borrow().is_empty(), "and no settle to report on");
    }

    /// `--no-verify` submits bare and asks herdr nothing else at all.
    #[test]
    fn no_verify_submits_once_with_no_wait_no_read_and_no_settle() {
        let scripted = Scripted::new(vec![Ok(record())], Some("id=\"k7m2x9\""));

        let submission = delivered_through(&scripted, &Proof::None).unwrap();

        assert_eq!(scripted.submitted.borrow().as_slice(), [None]);
        assert_eq!(scripted.reads.get(), 0);
        assert!(scripted.settled.borrow().is_empty());
        assert!(!submission.proven, "nothing was checked, so nothing is claimed");
    }

    /// One deadline, and every step takes its share of it rather than a fresh copy.
    #[test]
    fn each_step_is_given_what_is_left_rather_than_the_whole_budget_again() {
        let deadline = Deadline::within(10_000);

        assert!(deadline.share(60_000) <= 10_000, "never more than the budget");
        assert_eq!(deadline.share(250), 250, "and never more than its own ceiling");

        // A spent budget leaves nothing, rather than wrapping past zero into a fresh one.
        let spent = Deadline::within(0);
        assert_eq!(spent.share(60_000), 0);
        assert!(spent.share_of(Some(&leg(15_000))).is_none_or(|wait| wait.timeout == 0));
    }

    /// The wording is the defect, so the wording is what is asserted.
    ///
    /// It used to say the target "was already working", which a settled target whose pane never
    /// showed the id now reaches too. A caller reading that would go looking for a turn that was
    /// never running.
    #[test]
    fn the_unproven_warning_names_no_status() {
        let warning = unproven("reviewer");

        assert_eq!(
            warning,
            "reviewer's pane never showed the message, so delivery is unproven"
        );
        for status in [WORKING, "idle", "done", "blocked"] {
            assert!(!warning.contains(status), "{status}");
        }
    }

    #[test]
    fn a_delivered_prompt_reports_the_pane_and_the_status_it_reached() {
        let delivered = Delivered {
            delivered: Some(true),
            agent: record(),
        };

        assert_eq!(delivered.to_string(), "messaged w4:p17 (working)");
        assert_eq!(
            serde_json::to_string(&delivered).unwrap(),
            r#"{"delivered":true,"agent":{"agent":"claude","agent_status":"working","pane_id":"w4:p17"}}"#
        );
    }

    #[test]
    fn an_unverified_delivery_says_so_on_the_wire_rather_than_claiming_a_guarantee() {
        let delivered = Delivered {
            delivered: Some(false),
            agent: record(),
        };

        assert_eq!(serde_json::to_value(&delivered).unwrap()["delivered"], false);
    }

    #[test]
    fn a_refusal_and_a_stall_are_both_retryable_conflicts() {
        let occupied = MsgError::ComposerOccupied { target: "reviewer".to_owned() };
        assert_eq!(occupied.exit_status(), ExitStatus::Conflict);
        assert_eq!(
            occupied.to_string(),
            "reviewer's composer holds unsent text; wait for it to clear, or pass --force to send anyway"
        );

        let stalled = MsgError::Stalled { target: "reviewer".to_owned() };
        assert_eq!(stalled.exit_status(), ExitStatus::Conflict);
    }

    #[test]
    fn a_refusal_never_says_what_the_composer_held() {
        // What the guard read is someone's half-written message. The refusal says the composer holds
        // unsent text and never says what that text is — there is no field on this variant that
        // could carry it.
        let occupied = MsgError::ComposerOccupied { target: "reviewer".to_owned() };

        assert!(!occupied.to_string().contains("wait, before you commit"));
    }

    #[test]
    fn a_herdr_failure_forwards_herdrs_own_status_and_provenance() {
        let error = MsgError::Herdr(crate::herdr::HerdrError::Refused {
            command: "agent get".to_owned(),
            code: "agent_not_found".to_owned(),
            message: "agent target reviewer not found".to_owned(),
        });

        assert_eq!(error.exit_status(), ExitStatus::NotFound);
        assert_eq!(error.to_string(), "agent target reviewer not found");
        assert!(error.herdr().is_some());
    }
}
