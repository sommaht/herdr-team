//! The executable subcommands, their side-effect ordering, and the exit-status contract.

mod agents;
mod kill;
mod msg;
mod prime;
mod spawn;

pub use agents::AgentsArgs;
pub use kill::KillArgs;
pub use msg::MsgArgs;
pub use prime::PrimeArgs;
pub use spawn::SpawnArgs;

use std::fmt::Display;

use serde::Serialize;

use crate::core::Sink;
use crate::herdr::HerdrRef;

// =====================================================================================================================
// Command
// =====================================================================================================================

/// One executable subcommand, implemented directly by its clap `*Args` struct.
pub trait Cmd {
    /// Whether this command's result is the same text in both output modes.
    ///
    /// `prime` is the only override: its result is a document. Only the *result* is affected — a
    /// failure is still JSON under `--json` whatever the command.
    const TEXT_IN_BOTH_MODES: bool = false;

    /// What this command produces on success — rendered by the sink, never printed here.
    type Ok: Display + Serialize;

    /// This command's failure, which knows its own exit status.
    type Err: AsExitStatus;

    /// Checks preconditions and runs the flow, returning what it produced.
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
    fn herdr(&self) -> Option<HerdrRef> {
        None
    }
}

/// A command that cannot fail has no status to map.
impl AsExitStatus for std::convert::Infallible {
    fn exit_status(&self) -> ExitStatus {
        match *self {}
    }
}

/// The process exit-status contract: stable codes a caller can branch on without parsing stderr.
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
    /// The operation collides with state the target already holds; retryable once that state changes.
    Conflict = 5,
}

/// The one place the `#[repr(u8)]` discriminant is read as a number.
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
