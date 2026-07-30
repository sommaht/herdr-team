//! Every interaction with herdr, and the seam that runs them.
//!
//! No module outside this one spawns a process or names the `herdr` binary. The module root carries
//! [`run`], [`run_text`], and the stream discipline both depend on; `surface` owns the three ways to
//! make a pane and `agent` owns what a command does to an agent in one.

#![allow(
    dead_code,
    reason = "the submodules arrive in Task 8 and their callers in Tasks 10 and 11"
)]

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
        .map_err(|source| HerdrError::Unreadable { command: command_name(args), source })
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
        .map_err(|source| HerdrError::Spawn { command: command_name(args), source })?;

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
        Err(_) => HerdrError::Failed { command, message: first_line(&text) },
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
        assert_eq!(
            command_name(&args(&["agent", "prompt", "reviewer", "ship it"])),
            "agent prompt"
        );
        assert_eq!(
            command_name(&args(&["pane", "split", "w4:p1", "--cwd", "/w"])),
            "pane split"
        );
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
        assert_eq!(
            error.to_string(),
            "herdr agent start failed: unsupported interactive agent kind: clawd"
        );
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

        let plain = HerdrError::Failed {
            command: "pane split".to_owned(),
            message: "…".to_owned(),
        };
        assert_eq!(
            serde_json::to_string(&plain.reference()).unwrap(),
            r#"{"command":"pane split"}"#
        );
    }
}
