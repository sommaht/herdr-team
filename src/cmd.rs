//! The executable subcommands, their side-effect ordering, and the exit-status contract.
//!
//! There is deliberately no parent grouping: three sibling commands share no distinction a parent
//! would mark.

#![allow(dead_code, reason = "the commands land in Tasks 6, 10, and 11")]

use std::fmt::Display;

use serde::Serialize;

use crate::core::Sink;

// =====================================================================================================================
// Command
// =====================================================================================================================

/// One executable subcommand, implemented directly by its clap `*Args` struct.
///
/// The parser shape and the command are one type on purpose: `#[arg]` fields parse straight into
/// domain types, so a bad value fails at parse time and nothing downstream re-validates.
pub trait Cmd {
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
}

impl Failure {
    /// Renders a command failure for the sink.
    pub fn new<E: AsExitStatus + ?Sized>(error: &E) -> Self {
        Self { message: error.to_string() }
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
    #[error("no preset named opus")]
    struct NoSuchPreset;

    impl AsExitStatus for NoSuchPreset {
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

    #[test]
    fn a_failure_renders_its_errors_display_form_in_both_modes() {
        let failure = Failure::new(&NoSuchPreset);

        assert_eq!(failure.to_string(), "no preset named opus");
        assert_eq!(
            serde_json::to_string(&failure).unwrap(),
            r#"{"message":"no preset named opus"}"#
        );
    }
}
