//! Every interaction with herdr, and the seam that runs them.

pub mod agent;
pub mod surface;

use std::process::Command;

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::cmd::ExitStatus;

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// The binary every call in this crate runs.
const BINARY: &str = "herdr";

/// The environment variable herdr exports into every pane it owns, holding that pane's id.
pub const PANE_VARIABLE: &str = "HERDR_PANE_ID";

// =====================================================================================================================
// Run
// =====================================================================================================================

/// Runs one herdr command and deserializes its `result` into `T`.
///
/// # Errors
///
/// Returns a [`HerdrError`] naming which stage failed.
pub fn run<T: DeserializeOwned>(args: &[String]) -> Result<T, HerdrError> {
    let stdout = run_text(args)?;
    serde_json::from_str::<Envelope<T>>(&stdout)
        .map(|envelope| envelope.result)
        .map_err(|source| HerdrError::Unreadable { command: command_name(args), source })
}

/// Runs one herdr command and returns its stdout verbatim, for the calls herdr answers with raw
/// text rather than JSON.
///
/// The two streams are captured separately; see the style guide's herdr seam section.
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
        Err(classify(command_name(args), &output.stderr, output.status.code()))
    }
}

/// herdr's response envelope; only `result` is read.
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
/// `agent prompt` is the prompt text.
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
    /// The message is stderr's **first line only**, which can never hold a prompt or an agent's
    /// arguments.
    #[error("herdr {command} failed: {message}")]
    Failed {
        /// The herdr command that failed.
        command: String,
        /// herdr's first line of stderr.
        message: String,
        /// herdr's own process exit status, absent when a signal ended it.
        status: Option<i32>,
    },
    /// herdr succeeded but printed something this call could not read.
    ///
    /// Never quotes the output — `agent read`'s output is someone's terminal.
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
    /// Only the codes with a meaningful non-`1` answer are mapped; an unrecognized code is a
    /// general failure, not a compile error. herdr's own exit 2 — its parser rejecting an argument
    /// a caller's flag supplied — is forwarded as a usage error rather than an operational one.
    pub fn exit_status(&self) -> ExitStatus {
        if let Self::Failed { status: Some(2), .. } = self {
            return ExitStatus::Usage;
        }
        match self.code() {
            Some("agent_target_ambiguous") => ExitStatus::Usage,
            Some("agent_not_found" | "agent_pane_not_found" | "pane_not_found") => ExitStatus::NotFound,
            // `worktree_operation_in_progress` is transient — another create or remove is running;
            // the other worktree codes do not improve on a retry.
            Some(
                "agent_pane_busy" | "agent_prompt_stalled" | "agent_name_taken" | "worktree_operation_in_progress",
            ) => ExitStatus::Conflict,
            Some(_) | None => ExitStatus::Failure,
        }
    }

    /// Whether herdr is saying a submission may never have landed, rather than that it refused it.
    ///
    /// `agent_prompt_stalled` only. `timeout` must stay out: herdr submits *before* it waits, so a
    /// timeout never means the text failed to land, and re-sending on it delivers twice.
    pub fn is_undelivered(&self) -> bool {
        matches!(self.code(), Some("agent_prompt_stalled"))
    }

    /// Whether herdr answered that the target does not exist.
    pub fn is_not_found(&self) -> bool {
        matches!(self.code(), Some("agent_not_found"))
    }

    /// herdr's command and code, for nesting inside this crate's own error envelope.
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
/// Stopping at two words is what keeps a target and a prompt out of every error message.
fn command_name(args: &[String]) -> String {
    args.iter().take(2).cloned().collect::<Vec<String>>().join(" ")
}

/// Reads herdr's stderr into a typed failure.
///
/// herdr's error object is `{"error":{"code":…,"message":…}}`; anything else is a client-side
/// refusal printed as a plain line.
fn classify(command: String, stderr: &[u8], status: Option<i32>) -> HerdrError {
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
        // RS-002: a parse failure over a line that was never JSON says less than herdr's own words.
        Err(_) => HerdrError::Failed {
            command,
            message: first_line(&text),
            status,
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

        let error = classify("agent start".to_owned(), stderr, Some(1));

        assert_eq!(error.code(), Some("agent_pane_busy"));
        assert_eq!(error.to_string(), "agent target pane w4:p16 is not an available shell");
    }

    #[test]
    fn stderr_that_is_not_a_json_error_object_becomes_a_plain_failure_naming_the_command() {
        let error = classify(
            "agent start".to_owned(),
            b"unsupported interactive agent kind: clawd\n",
            Some(2),
        );

        assert_eq!(error.code(), None);
        assert_eq!(
            error.to_string(),
            "herdr agent start failed: unsupported interactive agent kind: clawd"
        );
    }

    #[test]
    fn herdrs_own_argument_rejection_stays_a_usage_error_on_this_side_of_the_seam() {
        let rejected = classify(
            "agent prompt".to_owned(),
            b"error: invalid value 'nonsense' for '--until <STATUS>'\n",
            Some(2),
        );
        assert_eq!(rejected.exit_status(), ExitStatus::Usage);

        // Every other status is herdr failing at the work rather than at the arguments.
        for status in [Some(1), Some(3), None] {
            let failed = classify("pane split".to_owned(), b"something broke\n", status);
            assert_eq!(failed.exit_status(), ExitStatus::Failure, "{status:?}");
        }
    }

    #[test]
    fn empty_stderr_still_produces_a_message() {
        let error = classify("pane split".to_owned(), b"", Some(1));

        assert_eq!(error.to_string(), "herdr pane split failed: unknown error");
    }

    #[test]
    fn only_the_codes_with_a_meaningful_answer_leave_the_general_failure_default() {
        for (code, expected) in [
            ("agent_target_ambiguous", ExitStatus::Usage),
            ("agent_not_found", ExitStatus::NotFound),
            ("agent_pane_not_found", ExitStatus::NotFound),
            ("pane_not_found", ExitStatus::NotFound),
            ("agent_pane_busy", ExitStatus::Conflict),
            ("agent_prompt_stalled", ExitStatus::Conflict),
            ("agent_name_taken", ExitStatus::Conflict),
            ("worktree_operation_in_progress", ExitStatus::Conflict),
            ("not_git_worktree", ExitStatus::Failure),
            ("linked_worktree_source", ExitStatus::Failure),
            ("worktree_create_failed", ExitStatus::Failure),
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
    fn only_the_code_that_means_the_prompt_may_never_have_landed_is_undelivered() {
        let stalled = HerdrError::Refused {
            command: "agent prompt".to_owned(),
            code: "agent_prompt_stalled".to_owned(),
            message: "…".to_owned(),
        };
        assert!(stalled.is_undelivered());

        // `timeout` is the one that must stay out — see [`HerdrError::is_undelivered`].
        for code in ["timeout", "agent_not_found", "agent_target_ambiguous"] {
            let error = HerdrError::Refused {
                command: "agent prompt".to_owned(),
                code: code.to_owned(),
                message: "…".to_owned(),
            };
            assert!(!error.is_undelivered(), "{code}");
        }
    }

    #[test]
    fn a_missing_agent_is_recognised_so_a_caller_can_tell_it_from_a_transport_failure() {
        let missing = HerdrError::Refused {
            command: "agent get".to_owned(),
            code: "agent_not_found".to_owned(),
            message: "agent target w4:p3 not found".to_owned(),
        };
        assert!(missing.is_not_found());

        let other = HerdrError::Refused {
            command: "agent get".to_owned(),
            code: "agent_target_ambiguous".to_owned(),
            message: "agent target reviewer is ambiguous".to_owned(),
        };
        assert!(!other.is_not_found());
    }

    #[test]
    fn the_pane_variable_is_the_one_herdr_exports_into_every_pane_it_owns() {
        assert_eq!(PANE_VARIABLE, "HERDR_PANE_ID");
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
            status: Some(1),
        };
        assert_eq!(
            serde_json::to_string(&plain.reference()).unwrap(),
            r#"{"command":"pane split"}"#
        );
    }
}
