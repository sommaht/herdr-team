//! The executable subcommands, their side-effect ordering, and the exit-status contract.
//!
//! There is deliberately no parent grouping: these siblings share no distinction a parent would mark.

mod agents;
mod kill;
mod prime;
mod prompt;
mod spawn;

pub use agents::AgentsArgs;
pub use kill::KillArgs;
pub use prime::PrimeArgs;
pub use prompt::PromptArgs;
pub use spawn::SpawnArgs;

use std::fmt::Display;

use serde::Serialize;

use crate::core::Sink;
use crate::herdr::HerdrRef;

// =====================================================================================================================
// Command
// =====================================================================================================================

/// One executable subcommand, implemented directly by its clap `*Args` struct.
///
/// The parser shape and the command are one type on purpose: `#[arg]` fields parse straight into
/// domain types, so a bad value fails at parse time and nothing downstream re-validates.
pub trait Cmd {
    /// Whether this command's result is the same text in both output modes.
    ///
    /// The one exception to the crate's `--json` contract, and `prime` is the only override. Its result
    /// *is* a document: wrapping prose in a JSON envelope buys escaping and no information, and a
    /// caller that habitually passes `--json` is better served the brief than a single escaped string.
    /// Everything else has fields worth flattening, so the default is `false`.
    ///
    /// An exception a consumer cannot discover is a trap, so this one is stated three times over: in
    /// `--json`'s own help, in `prime --help`, and in the brief. `--help` and `--version` print as
    /// documents for the same reason, without needing a flag here — clap answers those before a
    /// command is chosen.
    ///
    /// Only the *result* is affected. A failure is still JSON under `--json`, because a consumer
    /// branching on failures needs the tag and the status whatever the command was.
    const TEXT_IN_BOTH_MODES: bool = false;

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

    /// herdr's own command and code, when this failure came from herdr.
    ///
    /// Defaulted to `None`, because most failures are this crate's own. A command's error enum
    /// forwards this from whichever variant wraps a [`HerdrError`](crate::herdr::HerdrError).
    fn herdr(&self) -> Option<HerdrRef> {
        None
    }
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

impl Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("no agent named opus")]
    struct NoSuchAgent;

    impl AsExitStatus for NoSuchAgent {
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

    #[test]
    fn a_failure_renders_its_errors_display_form_in_both_modes() {
        let failure = Failure::new(&NoSuchAgent);

        assert_eq!(failure.to_string(), "no agent named opus");
        assert_eq!(
            serde_json::to_string(&failure).unwrap(),
            r#"{"message":"no agent named opus"}"#
        );
    }
}
