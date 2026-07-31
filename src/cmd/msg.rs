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
/// on an agent that is never going to answer.
const DEFAULT_TIMEOUT_MS: u64 = 15_000;

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
    herdr-team msg w4:p17 \"go\" --wait-until idle --timeout 120000\n\
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
    #[arg(long, value_name = "STATE")]
    wait_until: Vec<String>,

    /// Milliseconds to wait for delivery before giving up.
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
    /// asking about a transition rather than about delivery: `--wait-until idle` against a working
    /// agent is a request to wait out the turn, and that transition does happen.
    fn proof(&self, current: &str) -> Proof {
        if self.no_verify {
            return Proof::None;
        }
        if self.wait_until.is_empty() {
            return Proof::for_delivery(current, self.timeout);
        }
        Proof::Wait(Wait {
            until: self.wait_until.clone(),
            timeout: self.timeout,
        })
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

    /// One `agent get`, then the guard, then the submission.
    ///
    /// The guard runs before anything is sent, so a refusal has changed nothing.
    fn execute(self, sink: &Sink) -> Result<Self::Ok, Self::Err> {
        let before = agent::get(&self.target)?;

        if self.guarded() {
            // The harness decides what it needs to look at; this only performs the read it asks for.
            let readiness = harness::readiness(before.kind(), |source, format, lines| {
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

        let proof = self.proof(before.status());
        let submission = deliver(&self.target, &self.text, &self.reply(), &proof, sink)?;

        // Said out loud rather than left to the `delivered` field, which only the `--json` reader
        // sees. A caller reading the one line would otherwise treat an unproven dispatch as a
        // started one.
        if !submission.proven && proof != Proof::None {
            sink.warn(&format!(
                "{} was already working and its pane never showed the message, so delivery is unproven",
                self.target
            ));
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Proof {
    /// Nothing is checked, because `--no-verify` asked for nothing.
    None,
    /// herdr waits for a state change that only a delivered prompt could have produced.
    Wait(Wait),
    /// The target's pane is read for the id this message carries.
    ///
    /// The case herdr cannot answer. A `--until` state matches only once the agent's state-change
    /// sequence has passed the submission's, and that sequence advances only when the status
    /// actually changes — so an agent that was working before and is working after satisfies
    /// nothing, however long it is given. The message is queued all the same, and the harness
    /// renders it where it can be read.
    Pane,
}

impl Proof {
    /// The proof available for an ordinary delivery against a target in `current`.
    ///
    /// Shared with `spawn`, whose first prompt has exactly this problem: an agent that came up
    /// working has no state change left for herdr to match either.
    pub(super) fn for_delivery(current: &str, timeout: u64) -> Self {
        if current == WORKING {
            return Self::Pane;
        }
        Self::Wait(Wait {
            until: vec![WORKING.to_owned()],
            timeout,
        })
    }

    /// The wait to hand herdr, absent for the two kinds of proof herdr does not perform.
    fn wait(&self) -> Option<&Wait> {
        match self {
            Self::Wait(wait) => Some(wait),
            Self::None | Self::Pane => None,
        }
    }
}

/// What one submission produced: herdr's record of the agent, and whether delivery was proven.
///
/// A named pair rather than a tuple because both halves cross a module boundary, and `spawn` reads
/// them into differently named fields of its own result.
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
fn confirm_in_pane<E>(id: &str, text: &NonEmptyText, read: impl Fn(&'static str, u32) -> Result<String, E>) -> bool {
    let lines = delivery_window(text.lines().count());
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(DELIVERY_POLL_MS);

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

/// Submits a prompt, re-sending once if herdr reports it did not land.
///
/// Shared with `spawn`, whose first prompt goes through exactly this path. The sink comes last: it
/// is the channel a warning is reported through, not the thing being acted on.
///
/// On `agent_prompt_stalled` the prompt is re-sent once — finding 2 was that a prompt sent within a
/// few seconds of starting an agent is silently swallowed, and waiting and re-sending worked in
/// every observed case. A second failure is a retryable conflict rather than a silent success.
///
/// Only that one code, per [`HerdrError::is_undelivered`]: herdr submits before it waits, so any
/// other wait failure describes a prompt that landed. Re-sending on `timeout` was delivering every
/// such message twice.
///
/// # Errors
///
/// [`MsgError::Herdr`] for anything herdr refused outright, and [`MsgError::Stalled`] when
/// two submissions both failed to move the agent.
pub(super) fn deliver(
    target: &str,
    text: &NonEmptyText,
    reply: &Reply,
    proof: &Proof,
    sink: &Sink,
) -> Result<Submission, MsgError> {
    // Resolved and rendered once, before the re-send: a second submission must deliver the same
    // bytes as the first, and re-resolving would make a second `agent get` call to say so — and mint
    // a second id, leaving the pane check hunting for a token no delivered copy carries.
    let envelope = Envelope::resolve(reply, sink);
    let text = NonEmptyText::composed(envelope.wrap(text));
    let wait = proof.wait();

    let agent = match agent::prompt(target, &text, wait) {
        Ok(agent) => agent,
        Err(error) if error.is_undelivered() => {
            sink.warn(&format!("{target} did not acknowledge the prompt; re-sending once"));
            agent::prompt(target, &text, wait).map_err(|error| {
                if error.is_undelivered() {
                    MsgError::Stalled { target: target.to_owned() }
                } else {
                    MsgError::Herdr(error)
                }
            })?
        }
        Err(error) => return Err(MsgError::Herdr(error)),
    };

    // A returned wait is a matched wait: herdr answers the states it was given or it errors, so
    // there is nothing left to check for that arm.
    let proven = match proof {
        Proof::None => false,
        Proof::Wait(_) => true,
        Proof::Pane => confirm_in_pane(envelope.id(), &text, |source, lines| {
            agent::read(target, source, "text", lines)
        }),
    };

    Ok(Submission { agent, proven })
}

// =====================================================================================================================
// Output
// =====================================================================================================================

/// What `msg` produced: whether delivery was proven, and herdr's record of the agent.
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

    /// The wait inside a proof, for the arms that carry one.
    fn waited(proof: &Proof) -> &Wait {
        match proof {
            Proof::Wait(wait) => wait,
            other => panic!("expected a wait, got {other:?}"),
        }
    }

    #[test]
    fn the_default_proof_is_a_wait_for_the_status_moving_to_working() {
        let args = parse(&["msg", "reviewer", "ship it"]);

        let proof = args.proof("idle");
        assert_eq!(waited(&proof).until, [WORKING]);
        assert_eq!(waited(&proof).timeout, 15_000);
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

        assert_eq!(args.proof(WORKING), Proof::Pane, "no state change is left to observe");
        for settled in ["idle", "blocked", "done"] {
            assert!(matches!(args.proof(settled), Proof::Wait(_)), "{settled}");
        }
    }

    /// `spawn`'s first prompt takes the same route, so the rule has one owner.
    #[test]
    fn the_delivery_proof_is_chosen_the_same_way_wherever_it_is_asked_for() {
        assert_eq!(Proof::for_delivery(WORKING, 10_000), Proof::Pane);
        assert_eq!(
            Proof::for_delivery("idle", 10_000),
            Proof::Wait(Wait {
                until: vec![WORKING.to_owned()],
                timeout: 10_000
            })
        );
    }

    #[test]
    fn only_a_wait_is_handed_to_herdr_and_the_other_two_arms_submit_bare() {
        assert!(Proof::None.wait().is_none());
        assert!(Proof::Pane.wait().is_none());
        assert!(Proof::for_delivery("idle", 10_000).wait().is_some());
    }

    /// An explicit `--wait-until` is a question about a transition, not about delivery.
    #[test]
    fn an_explicit_wait_until_is_honored_even_against_a_working_target() {
        // `--wait-until idle` against a working agent is a request to wait out the turn, and that
        // transition really does happen — so dropping it here would break the settle wait.
        let args = parse(&["msg", "reviewer", "go", "--wait-until", "idle"]);

        let proof = args.proof(WORKING);
        assert_eq!(waited(&proof).until, ["idle"]);
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

        let proof = args.proof("idle");
        assert_eq!(waited(&proof).until, ["idle", "blocked"]);
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
            Proof::Wait(_)
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
        assert!(matches!(args.proof("idle"), Proof::Wait(_)));
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

        let found = confirm_in_pane("k7m2x9", &text, |source, lines| {
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

        assert!(!confirm_in_pane("abc123", &text, |_, _| Err::<String, _>(())));
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
        let found = confirm_in_pane("newone", &text, |_, _| {
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
