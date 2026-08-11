//! `msg` — deliver a message to an agent that already exists.
//!
//! Called `prompt` until this build; the old spelling stays as a hidden alias.

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
const DEFAULT_TIMEOUT_MS: u64 = 15_000;

/// The window herdr gives a submission to take effect before it will report a stall, in
/// milliseconds.
///
/// herdr's own constant, compared with `<=` — a wait of five seconds exactly is answered with a
/// bare `timeout` instead of `agent_prompt_stalled`, which takes the re-send repair with it.
const STALL_REPORTED_ABOVE_MS: u64 = 5_000;

/// herdr's rendering that rejoins soft-wrapped rows into the logical lines they were written as.
///
/// The sources that walk display rows or stop at the terminal's height can both miss a message
/// that wrapped or scrolled.
const DELIVERY_SOURCE: &str = "recent-unwrapped";

/// The most rows one read can return, which herdr clamps to whatever is asked for.
const DELIVERY_MAX_LINES: u32 = 1_000;

/// How long to keep looking for a message's id before giving up on proving it landed.
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
    // Without `allow_hyphen_values`, clap's rejection of a `--`-opening prompt would repeat the
    // prompt back, which the redaction rule forbids.
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
    /// An explicit `--wait-until` is honored whatever the current status, and produces
    /// [`Proof::Settle`]: a caller-named state may be one the target was going to reach anyway, so
    /// its delivery proof comes from the pane instead.
    fn proof(&self, current: &str) -> Proof {
        if self.no_verify {
            return Proof::None;
        }
        if self.wait_until.is_empty() {
            return Proof::for_delivery(current, self.timeout);
        }
        Proof::Settle {
            // The leg only has to see the target start working, so it takes at most an ordinary
            // delivery's budget and leaves the rest for the turn.
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
    /// The status is read after the guard's own reads, not before: it goes stale the moment it is
    /// taken, and a target that settled while the composer was being inspected would otherwise be
    /// prompted as though it were still mid-turn.
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
        // Taken from this read rather than the guard's, so `--force` resolves a margin too.
        let margin = harness::delivery_margin(before.kind());
        // Always mail: this command's argument is what a sender wrote, so it has an author to name.
        let delivery = Delivery::Mail {
            brief: None,
            body: (*self.text).clone(),
        };
        let submission = deliver(&self.target, &delivery, &self.reply(), &proof, margin, sink)?;

        // Said out loud: the `delivered` field is visible only to the `--json` reader.
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
/// A matched wait is not always proof: a state the *caller* named may be one the target was going
/// to reach regardless. The default `working` is — a settled target does not drift into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Proof {
    /// Nothing is checked, because `--no-verify` asked for nothing.
    None,
    /// herdr waits for `working`, which a settled target does not reach on its own — so matching it
    /// is the proof.
    Delivery(Wait),
    /// herdr waits for the states the caller named. Matching proves a transition happened, not that
    /// this message caused it, so the pane supplies the delivery proof instead.
    Settle {
        /// The wait the submission carries; absent when the target was already working.
        delivery: Option<Wait>,
        /// The caller's states, and the whole operation's budget.
        until: Wait,
    },
    /// The target's pane is read for the id this message carries, and nothing is waited on.
    ///
    /// The case herdr cannot answer: an agent that was working before and is working after produces
    /// no state change for a wait to match, however long it is given.
    Pane {
        /// The caller's budget, which is what bounds the pane poll.
        timeout: u64,
    },
}

impl Proof {
    /// The proof available for an ordinary delivery against a target in `current`.
    pub(super) fn for_delivery(current: &str, timeout: u64) -> Self {
        match delivery_leg(current, timeout) {
            Some(leg) => Self::Delivery(leg),
            None => Self::Pane { timeout },
        }
    }

    /// The wait to hand the submission, absent for the two shapes that submit bare.
    ///
    /// An already-working [`Self::Settle`] hands over the caller's own states: the submission is
    /// the only place herdr will anchor them to its own sequence.
    fn wait(&self) -> Option<&Wait> {
        match self {
            Self::Delivery(wait) | Self::Settle { delivery: Some(wait), .. } => Some(wait),
            Self::Settle { delivery: None, until } => Some(until),
            Self::None | Self::Pane { .. } => None,
        }
    }

    /// The caller's whole-operation budget, which one [`Deadline`] then spends across every step.
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
/// `None` for a target that is already working: an agent working before and after produces no
/// state change for `--until working` to match.
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
/// A trait so the ordering can be exercised without a herdr process.
pub(super) trait Delivering {
    /// Submits the text, optionally carrying a wait.
    fn prompt(&self, text: &NonEmptyText, wait: Option<&Wait>) -> Result<AgentRecord, HerdrError>;

    /// Reads the target's pane, looking for the id an envelope carries.
    fn read(&self, source: &'static str, lines: u32) -> Result<String, HerdrError>;

    /// Waits for the states a caller named, with no submission attached.
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
/// A brief is not mail: it comes from a file the recipient's own config points at, so there is no
/// sender to name and nothing to reply to.
pub(super) enum Delivery {
    /// A sender's own message, wrapped, optionally behind the recipient's brief.
    Mail {
        /// The agent's brief, delivered raw ahead of the envelope. Absent when it has none.
        brief: Option<NonEmptyText>,
        /// What the sender wrote, and the only part an envelope goes around.
        body: NonEmptyText,
    },
    /// A brief with no message behind it, delivered exactly as written.
    ///
    /// Carries no envelope id, so the pane cannot prove its delivery.
    Brief(NonEmptyText),
}

impl Delivery {
    /// The text to submit, and the id proving it landed when there is an envelope to carry one.
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
/// `composed` is sound: a brief is already non-blank, and a wrapping always opens with a tag.
fn after_brief(brief: Option<&NonEmptyText>, mail: &str) -> NonEmptyText {
    match brief {
        Some(brief) => NonEmptyText::composed(format!("{brief}\n\n{mail}")),
        None => NonEmptyText::composed(mail.to_owned()),
    }
}

/// What one submission produced: herdr's record of the agent, and whether delivery was proven.
///
/// `Debug` cannot leak a prompt: neither half carries prompt text.
#[derive(Debug)]
pub(super) struct Submission {
    /// herdr's record of the target after the submission.
    pub agent: AgentRecord,
    /// Whether delivery was actually proven, rather than merely attempted.
    pub proven: bool,
}

/// Looks for `id` in the target's pane until it appears or the poll window closes.
///
/// Never an error: a snapshot that cannot be read, or a rendering this build does not recognize,
/// leaves delivery unproven rather than failing a prompt that did land.
fn confirm_in_pane<E>(
    id: &str,
    text: &NonEmptyText,
    margin: u32,
    poll_ms: u64,
    read: impl Fn(&'static str, u32) -> Result<String, E>,
) -> bool {
    let lines = delivery_window(text.lines().count(), margin);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(poll_ms);

    loop {
        match read(DELIVERY_SOURCE, lines) {
            // The snapshot is captured terminal content and is never reported anywhere.
            Ok(snapshot) if snapshot.contains(id) => return true,
            Ok(_) => {}
            // A failed read leaves delivery unproven; herdr's message is not restated.
            Err(_) => return false,
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(DELIVERY_INTERVAL_MS));
    }
}

/// How many rows to read back when looking for a delivered message: its own height, which is what
/// scrolled the transcript, plus the harness's margin for what it draws below.
///
/// herdr clamps a read at [`DELIVERY_MAX_LINES`], so a message longer than that may have scrolled
/// its id genuinely out of reach — reported unproven.
fn delivery_window(message_lines: usize, margin: u32) -> u32 {
    u32::try_from(message_lines)
        .unwrap_or(DELIVERY_MAX_LINES)
        .saturating_add(margin)
        .min(DELIVERY_MAX_LINES)
}

/// Submits a prompt, proves what can be proven, and waits out the settle a caller asked for.
///
/// # Errors
///
/// [`MsgError::Stalled`] when two submissions both failed to move the agent; otherwise whatever
/// herdr refused.
pub(super) fn deliver(
    target: &str,
    delivery: &Delivery,
    reply: &Reply,
    proof: &Proof,
    margin: u32,
    sink: &Sink,
) -> Result<Submission, MsgError> {
    // Composed once: a re-send must deliver the same bytes, and the same id, as the first.
    let (text, id) = delivery.compose(reply, sink);

    deliver_through(
        &ThroughHerdr { target },
        target,
        &text,
        id.as_deref(),
        proof,
        margin,
        sink,
    )
}

/// Everything [`deliver`] does once the text is composed, over operations a test can supply.
///
/// A settled target submits carrying its delivery leg, reads the pane for the id, and only then
/// issues the caller's settle wait standalone. An already-working target has no settled moment a
/// leg could start from, so the caller's wait rides on the submission instead — and the turn it
/// waits out may be the one already in progress rather than this message's own.
///
/// # Errors
///
/// [`MsgError::Stalled`] when two submissions both failed to move the agent; otherwise whatever
/// herdr refused.
fn deliver_through<D: Delivering>(
    delivering: &D,
    target: &str,
    text: &NonEmptyText,
    id: Option<&str>,
    proof: &Proof,
    margin: u32,
    sink: &Sink,
) -> Result<Submission, MsgError> {
    let deadline = Deadline::within(proof.budget_ms());

    let agent = submit(delivering, target, text, proof.wait(), &deadline, sink)?;

    // A matched delivery wait is proof on its own; every other shape asks the pane.
    let proven = match (proof, id) {
        (Proof::None, _) => false,
        (Proof::Delivery(_), _) => true,
        (Proof::Pane { .. } | Proof::Settle { .. }, Some(id)) => {
            confirm_in_pane(id, text, margin, deadline.share(DELIVERY_POLL_MS), |source, lines| {
                delivering.read(source, lines)
            })
        }
        // A brief carries no id, so there is nothing in the pane to match.
        (Proof::Pane { .. } | Proof::Settle { .. }, None) => false,
    };

    // The one branch whose settle wait did not ride on the submission; its later record supersedes.
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
/// A prompt sent within a few seconds of an agent starting can be silently swallowed; the re-send
/// is the repair. Both attempts draw on the same deadline.
///
/// # Errors
///
/// [`MsgError::Stalled`] when both submissions failed to move the agent; otherwise whatever herdr
/// refused.
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
/// A bare submission never waits, and a wait of [`STALL_REPORTED_ABOVE_MS`] or less is answered
/// with a bare `timeout` rather than a stall.
fn repair_available(wait: Option<&Wait>) -> bool {
    wait.is_some_and(|wait| wait.timeout > STALL_REPORTED_ABOVE_MS)
}

/// What a caller is told when nothing proved the message landed. Names no status: several reach it.
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
    /// Absent when there was no prompt to deliver, which is `spawn`'s case.
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
    /// Deliberately no field for what the guard read: that is someone's half-written message.
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

    /// One harness's margin, for the tests that need a window rather than a particular one.
    fn codex_margin() -> u32 {
        harness::delivery_margin(Some("codex"))
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

    #[test]
    fn the_delivery_proof_is_chosen_the_same_way_wherever_it_is_asked_for() {
        assert_eq!(Proof::for_delivery(WORKING, 10_000), Proof::Pane { timeout: 10_000 });
        assert_eq!(Proof::for_delivery("idle", 10_000), Proof::Delivery(leg(10_000)));
    }

    #[test]
    fn a_settles_leg_is_capped_at_a_delivery_budget_and_an_ordinary_delivery_keeps_the_whole_timeout() {
        let settling = parse(&["msg", "reviewer", "go", "--timeout", "120000", "--wait-until", "done"]);
        let Proof::Settle { delivery, until } = settling.proof("idle") else {
            panic!("an explicit --wait-until is a settle");
        };
        assert_eq!(delivery, Some(leg(DEFAULT_TIMEOUT_MS)), "a slice, not the whole turn");
        assert_eq!(until.timeout, 120_000, "the rest is the turn's");

        let plain = parse(&["msg", "reviewer", "go", "--timeout", "120000"]);
        assert_eq!(plain.proof("idle"), Proof::Delivery(leg(120_000)));
    }

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

    #[test]
    fn only_the_two_shapes_with_something_to_establish_hand_the_submission_a_wait() {
        assert!(Proof::None.wait().is_none());
        assert!(Proof::Pane { timeout: 10_000 }.wait().is_none());
        assert_eq!(Proof::for_delivery("idle", 10_000).wait(), Some(&leg(10_000)));

        let args = parse(&["msg", "reviewer", "go", "--wait-until", "done"]);
        let settled = args.proof("idle");
        assert_eq!(settled.wait(), Some(&leg(15_000)), "the leg, not the caller's states");

        let working = args.proof(WORKING);
        assert_eq!(
            working.wait(),
            Some(&Wait {
                until: vec!["done".to_owned()],
                timeout: 15_000,
            })
        );
    }

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

    #[test]
    fn an_explicit_wait_until_is_honored_even_against_a_working_target() {
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
        assert!(Harness::try_parse_from(["msg", "reviewer", "go", "--no-reply", "--reply-to", "collector"]).is_err());
    }

    #[test]
    fn the_reply_flags_are_independent_of_the_delivery_flags() {
        let args = parse(&["msg", "reviewer", "go", "--no-reply"]);
        assert!(matches!(args.proof("idle"), Proof::Delivery(_)));
        assert!(args.guarded());
    }

    #[test]
    fn the_guard_runs_unless_force_says_otherwise() {
        assert!(parse(&["msg", "reviewer", "go"]).guarded());
        assert!(!parse(&["msg", "reviewer", "go", "--force"]).guarded());
    }

    #[test]
    fn a_prompt_that_opens_with_a_dash_is_prompt_text_rather_than_a_flag() {
        for text in ["--force the issue", "-e", "--not-a-flag-here", "-- leading separator"] {
            assert_eq!(parse(&["msg", "reviewer", text]).text.to_string(), text, "{text}");
        }

        // A token exactly matching a declared flag is still that flag, and a bare `--` is still the
        // terminator; everything after `--` is the prompt.
        assert_eq!(parse(&["msg", "reviewer", "--", "--force"]).text.to_string(), "--force");
        assert_eq!(parse(&["msg", "reviewer", "--", "--"]).text.to_string(), "--");
    }

    #[test]
    fn a_flag_after_the_prompt_is_still_a_flag() {
        let args = parse(&["msg", "reviewer", "--go", "--wait-until", "idle", "--force"]);

        assert_eq!(args.text.to_string(), "--go");
        assert_eq!(args.wait_until, ["idle"]);
        assert!(!args.guarded());
    }

    #[test]
    fn a_blank_prompt_is_refused_at_parse_time() {
        assert!(Harness::try_parse_from(["msg", "reviewer", "   "]).is_err());
    }

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

    #[test]
    fn the_read_window_covers_the_message_plus_room_for_the_harness_furniture() {
        let margin = harness::delivery_margin(Some("codex"));

        assert_eq!(delivery_window(1, margin), 1 + margin);
        assert_eq!(delivery_window(100, margin), 100 + margin);

        assert_eq!(delivery_window(5_000, margin), DELIVERY_MAX_LINES);
        assert_eq!(
            delivery_window(usize::MAX, margin),
            DELIVERY_MAX_LINES,
            "no wrap on the way past u32"
        );
        assert_eq!(
            delivery_window(1, DELIVERY_MAX_LINES),
            DELIVERY_MAX_LINES,
            "a margin past the clamp cannot ask for a read herdr would not answer"
        );
    }

    /// The gaps are live measurements: in a sixty-five-row pane with a three-line message, Codex
    /// left the `<mail>` element about ten rows from the bottom and Claude Code about forty-five.
    #[test]
    fn each_harness_gets_a_window_that_clears_its_own_measured_gap() {
        // The message length those gaps were measured with.
        let window = |kind| delivery_window(3, harness::delivery_margin(Some(kind)));

        for (kind, gap) in [("codex", 10_u32), ("claude", 45)] {
            assert!(window(kind) > gap, "{kind}'s gap of {gap} rows was missed outright");
            assert!(
                window(kind) >= gap * 4,
                "{kind}'s gap of {gap} rows is cleared by too little to survive added padding"
            );
        }

        // Claude Code pads the whole pane above its composer, so its gap scales with the terminal.
        let a_tall_pane = 200_u32;
        assert!(window("claude") > a_tall_pane);

        assert!(window("codex") < window("claude"));
    }

    #[test]
    fn a_kind_this_build_does_not_know_is_read_at_least_as_far_back_as_any_it_does() {
        let unknown = harness::delivery_margin(Some("some-agent-shipped-next-year"));

        for kind in harness::kinds() {
            assert!(
                unknown >= harness::delivery_margin(Some(kind)),
                "{kind} is read further back than an unrecognized harness"
            );
        }
        assert_eq!(
            harness::delivery_margin(None),
            unknown,
            "a pane with no agent, likewise"
        );
    }

    #[test]
    fn a_pane_showing_the_id_is_delivery_proven() {
        let text = NonEmptyText::composed("<mail from=\"dispatcher\" id=\"k7m2x9\">\ngo\n</mail>".to_owned());
        let asked = std::cell::RefCell::new(Vec::new());

        let found = confirm_in_pane("k7m2x9", &text, codex_margin(), DELIVERY_POLL_MS, |source, lines| {
            asked.borrow_mut().push((source, lines));
            Ok::<_, ()>("… transcript …\n  <mail from=\"dispatcher\" id=\"k7m2x9\">\n  go\n".to_owned())
        });

        assert!(found);
        assert_eq!(
            asked.into_inner(),
            [("recent-unwrapped", 3 + codex_margin())],
            "one read, from the source that rejoins wrapped rows"
        );
    }

    #[test]
    fn a_pane_that_cannot_be_read_is_unproven_rather_than_an_error() {
        let text = NonEmptyText::composed("<mail from=\"w4:p2\" id=\"abc123\">\ngo\n</mail>".to_owned());

        let unreadable = |_: &str, _: u32| Err::<String, ()>(());

        assert!(!confirm_in_pane(
            "abc123",
            &text,
            codex_margin(),
            DELIVERY_POLL_MS,
            unreadable
        ));
    }

    #[test]
    fn a_pane_holding_only_an_older_message_is_not_proof_of_this_one() {
        let text = NonEmptyText::composed("<mail from=\"w\" id=\"newone\">\ngo\n</mail>".to_owned());
        let stale = "  <mail from=\"w\" id=\"oldone\">\n  an earlier dispatch\n";
        let looked = std::cell::Cell::new(0_u32);

        // The second look fails so the poll ends after one interval instead of spending the window.
        let found = confirm_in_pane("newone", &text, codex_margin(), DELIVERY_POLL_MS, |_, _| {
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
            // A miss then a failure, so a pane that never shows the id ends the poll in one interval.
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
    fn delivered_through(scripted: &Scripted, proof: &Proof) -> Result<Submission, MsgError> {
        deliver_through(
            scripted,
            "reviewer",
            &NonEmptyText::composed(format!("<mail from=\"operator\" id=\"{SCRIPTED_ID}\">\ngo\n</mail>")),
            Some(SCRIPTED_ID),
            proof,
            codex_margin(),
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

    #[test]
    fn a_submission_that_lands_is_the_only_call_an_ordinary_delivery_makes() {
        let scripted = Scripted::new(vec![Ok(record())], None);

        let submission = delivered_through(&scripted, &Proof::for_delivery("idle", 15_000)).unwrap();

        assert!(submission.proven, "a matched delivery wait is the proof");
        assert_eq!(scripted.submitted.borrow().len(), 1);
        assert_eq!(scripted.reads.get(), 0, "nothing to look for");
        assert!(scripted.settled.borrow().is_empty());
    }

    #[test]
    fn a_stalled_submission_is_re_sent_once_and_the_second_one_stands() {
        let scripted = Scripted::new(vec![Err(refusal("agent_prompt_stalled")), Ok(record())], None);

        let submission = delivered_through(&scripted, &Proof::for_delivery("idle", 15_000)).unwrap();

        assert!(submission.proven);
        assert_eq!(scripted.submitted.borrow().len(), 2, "sent again, not given up on");
    }

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

    #[test]
    fn a_settle_whose_pane_never_showed_the_id_is_unproven_and_still_waits_out_the_turn() {
        let scripted = Scripted::new(vec![Ok(record())], None);

        let submission = delivered_through(&scripted, &settling("idle", 60_000)).unwrap();

        assert!(!submission.proven);
        assert!(scripted.reads.get() >= 2, "the first miss was not the last look");
        assert_eq!(scripted.settled.borrow().len(), 1);
    }

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

    /// The reported failure: a Codex agent still starting its MCP servers settled on its own, and
    /// the matched wait was reported as delivery.
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

    #[test]
    fn no_verify_submits_once_with_no_wait_no_read_and_no_settle() {
        let scripted = Scripted::new(vec![Ok(record())], Some("id=\"k7m2x9\""));

        let submission = delivered_through(&scripted, &Proof::None).unwrap();

        assert_eq!(scripted.submitted.borrow().as_slice(), [None]);
        assert_eq!(scripted.reads.get(), 0);
        assert!(scripted.settled.borrow().is_empty());
        assert!(!submission.proven, "nothing was checked, so nothing is claimed");
    }

    #[test]
    fn each_step_is_given_what_is_left_rather_than_the_whole_budget_again() {
        let deadline = Deadline::within(10_000);

        assert!(deadline.share(60_000) <= 10_000, "never more than the budget");
        assert_eq!(deadline.share(250), 250, "and never more than its own ceiling");

        let spent = Deadline::within(0);
        assert_eq!(spent.share(60_000), 0);
        assert!(spent.share_of(Some(&leg(15_000))).is_none_or(|wait| wait.timeout == 0));
    }

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
        // The literal stands in for composer text a refusal must never echo.
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
