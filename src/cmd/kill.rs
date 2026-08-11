//! `kill` — close an agent's pane, refusing one that is mid-task.

use std::fmt::Display;

use clap::Args;
use serde::Serialize;
use thiserror::Error;

use crate::cmd::{AsExitStatus, Cmd, ExitStatus};
use crate::core::{PaneId, Sink};
use crate::herdr::agent::{self, AgentRecord, WORKING};
use crate::herdr::{HerdrError, HerdrRef, surface};

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// herdr statuses that mean the agent is mid-task, so closing its pane would lose work.
const BUSY: [&str; 2] = [WORKING, "blocked"];

/// herdr statuses that mean there is nothing in flight for this to destroy.
///
/// A closed set: a status neither list names — herdr's own `unknown`, or one it grows later — warns
/// rather than reading as safe to destroy.
const SETTLED: [&str; 2] = ["idle", "done"];

// =====================================================================================================================
// Kill Args
// =====================================================================================================================

/// Close an agent's pane, refusing one that is mid-task.
///
/// This is herdr's `pane close` with a guard in front of it. That command takes a pane id and no
/// flags at all, so nothing stands between a misread status and work that was never written to disk —
/// and the caller here is usually another agent, acting on a status it may have read wrong.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  \
    herdr-team kill reviewer\n  \
    herdr-team kill w4:p17 --force")]
pub struct KillArgs {
    /// The agent to close: a herdr pane id, or a unique agent name.
    target: String,

    /// Close the pane even while the agent is still working or blocked.
    #[arg(long)]
    force: bool,
}

impl KillArgs {
    /// Whether the status guard runs.
    fn guarded(&self) -> bool {
        !self.force
    }

    /// herdr's record for the target, or `None` when herdr says no agent is there.
    ///
    /// `agent_not_found` is not a failure here: it is the signal to close the target as a bare pane.
    fn resolve(&self) -> Result<Option<AgentRecord>, KillError> {
        match agent::get(&self.target) {
            Ok(agent) => Ok(Some(agent)),
            Err(error) if error.code() == Some("agent_not_found") => Ok(None),
            Err(error) => Err(KillError::Herdr(error)),
        }
    }
}

impl Cmd for KillArgs {
    type Ok = Killed;
    type Err = KillError;

    /// One `agent get`, then the guard, then `pane close`.
    fn execute(self, sink: &Sink) -> Result<Self::Ok, Self::Err> {
        let Some(agent) = self.resolve()? else {
            // No agent here, so the target goes to `pane close` as it arrived and herdr judges it.
            surface::close(&self.target)?;
            return Ok(Killed {
                // herdr accepted it, so by now it does name a pane.
                pane: PaneId::from(self.target.as_str()),
                agent: None,
            });
        };

        if self.guarded() {
            match Liveness::of(agent.status()) {
                Liveness::Busy => {
                    return Err(KillError::Busy {
                        target: self.target,
                        status: agent.status().to_owned(),
                    });
                }
                // Failing open: absent evidence never earns a refusal. Closed anyway, and said so.
                answer => {
                    if let Some(warning) = answer.warning(&self.target, agent.status()) {
                        sink.warn(&warning);
                    }
                }
            }
        }

        let pane = agent.pane().clone();
        surface::close(&pane)?;
        Ok(Killed { pane, agent: Some(agent) })
    }
}

// =====================================================================================================================
// Liveness
// =====================================================================================================================

/// What a target's status says about closing its pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Liveness {
    /// Mid-task: closing the pane loses whatever is not yet on disk.
    Busy,
    /// Settled, so there is nothing in flight for this to destroy.
    Settled,
    /// A status neither list names, which is not evidence in either direction.
    Unrecognized,
}

impl Liveness {
    /// What herdr's status word says, with anything unlisted reading as [`Self::Unrecognized`].
    fn of(status: &str) -> Self {
        if BUSY.contains(&status) {
            Self::Busy
        } else if SETTLED.contains(&status) {
            Self::Settled
        } else {
            Self::Unrecognized
        }
    }

    /// The diagnostic for a pane closed without evidence that was safe.
    ///
    /// `None` for [`Self::Settled`], which had evidence, and [`Self::Busy`], which is a refusal instead.
    fn warning(self, target: &str, status: &str) -> Option<String> {
        match self {
            Self::Unrecognized => Some(format!(
                "herdr reports {target} as {status}, which is not a status this recognizes; closing anyway"
            )),
            Self::Busy | Self::Settled => None,
        }
    }
}

// =====================================================================================================================
// Output
// =====================================================================================================================

/// What `kill` produced.
#[derive(Debug, Serialize)]
pub struct Killed {
    /// The pane that was closed.
    ///
    /// Renamed on the wire to herdr's own `pane_id`, the key every other pane id in this crate uses.
    #[serde(rename = "pane_id")]
    pane: PaneId,
    /// herdr's record of the agent that was in it, absent when the target hosted none.
    ///
    /// Read before the pane was closed, so it describes what was there rather than what is.
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<AgentRecord>,
}

impl Display for Killed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.agent {
            Some(agent) => write!(
                f,
                "killed {} ({}, was {})",
                self.pane,
                agent.name_or_unknown(),
                agent.status()
            ),
            None => write!(f, "killed {} (no agent)", self.pane),
        }
    }
}

// =====================================================================================================================
// Errors
// =====================================================================================================================

/// Failure of the kill flow.
#[derive(Debug, Error)]
pub enum KillError {
    /// herdr refused something; its message and code are carried verbatim.
    #[error(transparent)]
    Herdr(#[from] HerdrError),
    /// The agent is mid-task, and the guard would not end it.
    #[error("{target} is {status}; closing its pane would lose work in progress — wait for it, or pass --force")]
    Busy {
        /// The agent whose pane is still open.
        target: String,
        /// herdr's own status word for it, never re-spelled.
        status: String,
    },
}

impl AsExitStatus for KillError {
    fn exit_status(&self) -> ExitStatus {
        match self {
            Self::Herdr(error) => error.exit_status(),
            Self::Busy { .. } => ExitStatus::Conflict,
        }
    }

    fn herdr(&self) -> Option<HerdrRef> {
        match self {
            Self::Herdr(error) => Some(error.reference()),
            Self::Busy { .. } => None,
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

    #[derive(Debug, Parser)]
    struct Harness {
        #[command(flatten)]
        args: KillArgs,
    }

    fn parse(argv: &[&str]) -> KillArgs {
        Harness::try_parse_from(argv).expect("parses").args
    }

    fn record(status: &str) -> AgentRecord {
        serde_json::from_str(&format!(
            r#"{{"agent":"claude","agent_status":"{status}","pane_id":"w4:p17","name":"reviewer"}}"#
        ))
        .expect("a fixture record parses")
    }

    #[test]
    fn a_mid_task_agent_is_the_one_the_guard_refuses() {
        assert_eq!(Liveness::of("working"), Liveness::Busy);
        assert_eq!(Liveness::of("blocked"), Liveness::Busy);
    }

    #[test]
    fn a_settled_agent_has_nothing_in_flight_to_lose() {
        assert_eq!(Liveness::of("idle"), Liveness::Settled);
        assert_eq!(Liveness::of("done"), Liveness::Settled);
    }

    #[test]
    fn a_status_this_does_not_know_warns_rather_than_being_assumed_settled() {
        assert_eq!(Liveness::of("unknown"), Liveness::Unrecognized);
        assert_eq!(Liveness::of("compacting"), Liveness::Unrecognized);
        assert_eq!(Liveness::of(""), Liveness::Unrecognized);
    }

    #[test]
    fn only_an_unrecognized_status_warns() {
        assert!(Liveness::Settled.warning("reviewer", "idle").is_none());
        assert!(Liveness::Busy.warning("reviewer", "working").is_none());

        let warning = Liveness::Unrecognized
            .warning("reviewer", "compacting")
            .expect("an unrecognized status is reported");
        assert!(
            warning.contains("reviewer") && warning.contains("compacting"),
            "got {warning}"
        );
    }

    #[test]
    fn the_guard_runs_unless_force_says_otherwise() {
        assert!(parse(&["kill", "reviewer"]).guarded());
        assert!(!parse(&["kill", "reviewer", "--force"]).guarded());
    }

    #[test]
    fn a_refusal_is_a_retryable_conflict_that_names_the_status_it_saw() {
        let busy = KillError::Busy {
            target: "reviewer".to_owned(),
            status: "working".to_owned(),
        };

        assert_eq!(busy.exit_status(), ExitStatus::Conflict);
        assert_eq!(
            busy.to_string(),
            "reviewer is working; closing its pane would lose work in progress — wait for it, or pass --force"
        );
        assert!(busy.herdr().is_none(), "this refusal is ours, not herdr's");
    }

    #[test]
    fn a_kill_reports_the_pane_it_closed_and_what_was_in_it() {
        let killed = Killed {
            pane: PaneId::from("w4:p17"),
            agent: Some(record("idle")),
        };

        assert_eq!(killed.to_string(), "killed w4:p17 (reviewer, was idle)");
        assert_eq!(
            serde_json::to_string(&killed).unwrap(),
            r#"{"pane_id":"w4:p17","agent":{"agent":"claude","agent_status":"idle","pane_id":"w4:p17","name":"reviewer"}}"#
        );
    }

    #[test]
    fn closing_a_pane_that_hosted_no_agent_says_so_rather_than_inventing_a_record() {
        let killed = Killed {
            pane: PaneId::from("w4:p7"),
            agent: None,
        };

        assert_eq!(killed.to_string(), "killed w4:p7 (no agent)");
        assert_eq!(serde_json::to_string(&killed).unwrap(), r#"{"pane_id":"w4:p7"}"#);
    }

    #[test]
    fn a_herdr_failure_forwards_herdrs_own_status_and_provenance() {
        let error = KillError::Herdr(HerdrError::Refused {
            command: "pane close".to_owned(),
            code: "pane_not_found".to_owned(),
            message: "pane w4:p99 not found".to_owned(),
        });

        assert_eq!(error.exit_status(), ExitStatus::NotFound);
        assert_eq!(error.to_string(), "pane w4:p99 not found");
        assert!(error.herdr().is_some());
    }
}
