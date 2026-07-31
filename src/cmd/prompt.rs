//! `prompt` — deliver a prompt to an agent that already exists.

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
    herdr-agent-tools prompt dispatcher --no-reply \"done: 4 flags are undocumented\"\n  \
    herdr-agent-tools prompt w4:p17 \"go\" --wait-until idle --timeout 120000")]
pub struct PromptArgs {
    /// The agent to prompt: a herdr pane id, or a unique agent name.
    target: String,

    /// The prompt text; `-` reads it from stdin (e.g. a heredoc or a pipe).
    // `allow_hyphen_values` is not in the doc comment because a caller has nothing to do about it.
    // Without it a prompt that opens with `--` is tokenized as a flag, and clap's rejection repeats
    // the would-be prompt back — which the prompt-redaction rule forbids outright. `--` is a repair
    // only here; `spawn --prompt` has no separator to spare, so the fix has to be the same one on
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

    /// Where this message says a reply should go.
    fn reply(&self) -> Reply {
        Reply::from_flags(self.reply_to.as_deref(), self.no_reply)
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
            // The harness decides what it needs to look at; this only performs the read it asks for.
            let readiness = harness::readiness(before.kind(), |source, format, lines| {
                agent::read(&self.target, source, format, lines)
            })?;
            match readiness {
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

        let agent = deliver(&self.target, &self.text, &self.reply(), wait.as_ref(), sink)?;
        Ok(Delivered { delivered: Some(verified), agent })
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
    reply: &Reply,
    wait: Option<&Wait>,
    sink: &Sink,
) -> Result<AgentRecord, PromptError> {
    // Resolved and rendered once, before the re-send: a second submission must deliver the same
    // bytes as the first, and re-resolving would make a second `agent get` call to say so.
    let text = NonEmptyText::composed(Envelope::resolve(reply, sink).wrap(text));

    match agent::prompt(target, &text, wait) {
        Ok(agent) => Ok(agent),
        Err(error) if error.is_undelivered() => {
            sink.warn(&format!("{target} did not acknowledge the prompt; re-sending once"));
            agent::prompt(target, &text, wait).map_err(|error| {
                if error.is_undelivered() {
                    PromptError::Stalled { target: target.to_owned() }
                } else {
                    PromptError::Herdr(error)
                }
            })
        }
        Err(error) => Err(PromptError::Herdr(error)),
    }
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
        let args = parse(&[
            "prompt",
            "reviewer",
            "go",
            "--wait-until",
            "idle",
            "--wait-until",
            "blocked",
        ]);

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
    fn a_prompt_addresses_its_reply_at_the_sender_by_default() {
        assert_eq!(parse(&["prompt", "reviewer", "go"]).reply(), Reply::ToSender);
    }

    #[test]
    fn no_reply_produces_a_message_that_closes_the_loop() {
        // The tail hands the replier this flag, so termination needs no memory: the replier runs the
        // line it was given rather than recalling a convention.
        assert_eq!(
            parse(&["prompt", "reviewer", "done", "--no-reply"]).reply(),
            Reply::None
        );
    }

    #[test]
    fn reply_to_addresses_a_collector_rather_than_the_sender() {
        assert_eq!(
            parse(&["prompt", "worker", "go", "--reply-to", "collector"]).reply(),
            Reply::To("collector".to_owned())
        );
    }

    #[test]
    fn no_reply_and_reply_to_are_refused_together_rather_than_one_silently_winning() {
        // One names an address the other deletes, so passing both is a caller that has not decided.
        // clap answers this with its own usage code, which is the 2 this crate's contract already uses.
        assert!(
            Harness::try_parse_from(["prompt", "reviewer", "go", "--no-reply", "--reply-to", "collector"]).is_err()
        );
    }

    #[test]
    fn the_reply_flags_are_independent_of_the_delivery_flags() {
        // --no-reply shapes the message; --no-verify skips the delivery wait; --force skips the
        // composer guard. No one of them implies another.
        let args = parse(&["prompt", "reviewer", "go", "--no-reply"]);
        assert!(args.wait().is_some());
        assert!(args.guarded());
    }

    #[test]
    fn the_guard_runs_unless_force_says_otherwise() {
        assert!(parse(&["prompt", "reviewer", "go"]).guarded());
        assert!(!parse(&["prompt", "reviewer", "go", "--force"]).guarded());
    }

    /// A prompt that opens with a dash is delivered rather than rejected.
    ///
    /// The parse is the redaction. A value clap accepts is a value no clap diagnostic can repeat,
    /// and repeating it is what the prompt rule forbids — so this test is the leak test too.
    #[test]
    fn a_prompt_that_opens_with_a_dash_is_prompt_text_rather_than_a_flag() {
        for text in ["--force the issue", "-e", "--not-a-flag-here", "-- leading separator"] {
            assert_eq!(parse(&["prompt", "reviewer", text]).text.to_string(), text, "{text}");
        }

        // The two limits, both of them clap's and neither of them a leak. A token that *exactly*
        // matches a declared flag is still that flag, and a bare `--` is still the value terminator.
        // Both are repaired the same way, and the repair is what `--` is for: everything after it is
        // the prompt.
        assert_eq!(
            parse(&["prompt", "reviewer", "--", "--force"]).text.to_string(),
            "--force"
        );
        assert_eq!(parse(&["prompt", "reviewer", "--", "--"]).text.to_string(), "--");
    }

    #[test]
    fn a_flag_after_the_prompt_is_still_a_flag() {
        // The other half of `allow_hyphen_values`: it must claim the prompt's own value and nothing
        // past it, or every flag on this command stops working.
        let args = parse(&["prompt", "reviewer", "--go", "--wait-until", "idle", "--force"]);

        assert_eq!(args.text.to_string(), "--go");
        assert_eq!(args.wait_until, ["idle"]);
        assert!(!args.guarded());
    }

    #[test]
    fn a_blank_prompt_is_refused_at_parse_time() {
        assert!(Harness::try_parse_from(["prompt", "reviewer", "   "]).is_err());
    }

    #[test]
    fn a_delivered_prompt_reports_the_pane_and_the_status_it_reached() {
        let delivered = Delivered {
            delivered: Some(true),
            agent: record(),
        };

        assert_eq!(delivered.to_string(), "prompted w4:p17 (working)");
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
        // What the guard read is someone's half-written message. The refusal says the composer holds
        // unsent text and never says what that text is — there is no field on this variant that
        // could carry it.
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
