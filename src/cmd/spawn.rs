//! `spawn` — create a surface and start a preset-configured agent in it.
//!
//! The longest file here because it is the longest command: five ordered steps across three
//! placements, each of which can fail after the previous one has already changed something in herdr.
//! It reads as one transaction, so splitting it would hide that ordering and it stays one file.
//!
//! **RS-033 answered rather than waived.** This sits just over the prompt's line count and has crossed
//! it in both directions, so here is the standing answer. Two candidates to move, and neither is worth
//! it yet:
//!
//! - [`Backoff`] is genuinely separable, and has one consumer twenty lines above it.
//! - [`anchor`] and [`SpawnArgs::workspace`] together answer "where is the caller", which is a real
//!   boundary rather than a line-count one — it is the thing a `--from` flag would override. Until
//!   something other than this flow asks that question, they are steps of the flow directly above
//!   them, and a reader following `execute` finds them where they are used.
//!
//! The trigger is a second caller, not a line count. `--from` landing is what moves the second pair.

use std::fmt::Display;
use std::path::PathBuf;
use std::time::Duration;

use clap::Args;
use clap_stdin::MaybeStdin;
use serde::Serialize;
use thiserror::Error;

use crate::cmd::prompt::deliver;
use crate::cmd::{AsExitStatus, Cmd, ExitStatus};
use crate::config::{ConfigError, Presets};
use crate::core::{AgentName, NonEmptyText, PaneId, Sink};
use crate::herdr::agent::{self, AgentRecord, WORKING, Wait};
use crate::herdr::surface::{self, Focus, Placement};
use crate::herdr::{HerdrError, HerdrRef};

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// The environment variable herdr exports into every pane it owns, holding that pane's id.
const PANE_VARIABLE: &str = "HERDR_PANE_ID";

/// How long `agent start` is retried while the new pane's shell is still starting, in milliseconds.
///
/// Finding 3: creating a tab or workspace races with slow shell init, and herdr correctly refuses a
/// pane that has not reached its prompt. Ten seconds covers a shell running a directory-environment
/// hook without leaving a caller hanging on one that is genuinely broken.
const DEFAULT_SETTLE_MS: u64 = 10_000;

/// The first retry delay, in milliseconds.
const BACKOFF_FIRST_MS: u64 = 250;

/// The longest a single retry waits, in milliseconds.
const BACKOFF_CAP_MS: u64 = 2_000;

// =====================================================================================================================
// Spawn Args
// =====================================================================================================================

/// Create a pane, tab, or workspace and start a preset-configured agent in it.
///
/// herdr starts an agent only in a pane that already exists and is sitting at an interactive shell
/// prompt, so this does both halves: it creates the surface, reads back the new pane's id, and
/// starts the agent there.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  \
    herdr-agent-tools spawn reviewer --placement tab --preset opus\n  \
    herdr-agent-tools spawn fixer --preset sonnet --prompt \"Fix the flaky tests\"\n  \
    git diff | herdr-agent-tools spawn reviewer --prompt -\n  \
    herdr-agent-tools spawn big --placement workspace --preset fable -- --resume")]
pub struct SpawnArgs {
    /// The agent's name; must satisfy herdr's rule, which is checked before anything is created.
    name: AgentName,

    /// Where the agent's pane comes from: a split of the calling pane, which needs `HERDR_PANE_ID`,
    /// or the root pane of a new tab or workspace.
    // Not a doc comment: a field's doc comment on this struct is its `--help` text, and why the
    // default is a string is not something a caller needs. `default_value_t` would want a `Display`
    // on `Placement` whose only consumer is this line, and which has to agree with the value parser
    // to work at all. A parse with no `--placement` exercises the string, so a typo in it fails a
    // test rather than reaching a caller.
    #[arg(long, value_enum, default_value = "pane")]
    placement: Placement,

    /// The preset to start; defaults to the config file's `default`.
    #[arg(long, value_name = "NAME")]
    preset: Option<String>,

    /// Read this preset file instead of the one in the config directory.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// A first prompt to deliver once the agent is up; `-` reads it from stdin.
    #[arg(long, value_name = "TEXT")]
    prompt: Option<MaybeStdin<NonEmptyText>>,

    /// The new surface's working directory; defaults to the current one.
    #[arg(long, value_name = "PATH")]
    cwd: Option<PathBuf>,

    /// Focus the new surface. Off by default, so a background launch does not steal the cursor.
    #[arg(long)]
    focus: bool,

    /// Milliseconds to keep retrying `agent start` while the new pane's shell is still starting.
    #[arg(long, value_name = "MS", default_value_t = DEFAULT_SETTLE_MS)]
    settle_timeout: u64,

    /// Extra arguments appended after the preset's, passed to the agent verbatim.
    ///
    /// No merging and no de-duplication, so the agent's own last-flag-wins rules settle any
    /// conflict with the preset.
    #[arg(last = true, value_name = "AGENT_ARG")]
    agent_args: Vec<String>,
}

impl SpawnArgs {
    /// The workspace a new tab opens in: the calling pane's, as herdr currently reports it.
    ///
    /// Asked of herdr rather than read from `HERDR_WORKSPACE_ID`. That variable is injected once, when
    /// the pane is created, and nothing can rewrite a running shell's environment afterwards — so a
    /// pane moved to another workspace still carries the old id and would send this tab there.
    ///
    /// `None` is the one case nothing can pin: no calling pane at all, which is a caller outside a
    /// herdr session. herdr's default then applies, and it resolves to whichever workspace the *UI*
    /// has focused — so a tab meant for this project opens wherever the human last clicked. Warned
    /// about rather than left silent, because that outcome looks like a bug in this tool.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Herdr`] if herdr cannot answer for a pane this is running in, which happens
    /// before any surface exists.
    fn workspace(&self, sink: &Sink) -> Result<Option<String>, SpawnError> {
        let pane = std::env::var(PANE_VARIABLE)
            .ok()
            .filter(|value| !value.trim().is_empty());
        let Some(pane) = pane else {
            sink.warn(&format!(
                "{PANE_VARIABLE} is unset, so this tab opens in whichever workspace herdr has focused"
            ));
            return Ok(None);
        };
        Ok(Some(surface::workspace_of(pane.trim())?))
    }

    /// Whether the new surface takes the user's focus.
    fn focus(&self) -> Focus {
        if self.focus { Focus::Take } else { Focus::Leave }
    }
}

impl Cmd for SpawnArgs {
    type Ok = Spawned;
    type Err = SpawnError;

    /// Pre-checks, preset, surface, agent, first prompt — in that order, because a pre-check that
    /// runs after a surface exists is not a pre-check.
    fn execute(self, sink: &Sink) -> Result<Self::Ok, Self::Err> {
        let anchor = match self.placement {
            Placement::Pane => Some(anchor(std::env::var(PANE_VARIABLE).ok().as_deref())?),
            Placement::Tab | Placement::Workspace => None,
        };

        let presets = Presets::load(self.config.as_deref())?;
        let preset = presets.resolve(self.preset.as_deref())?;
        let kind = preset.kind().to_owned();
        let mut agent_args = preset.args().to_vec();
        agent_args.extend(self.agent_args.iter().cloned());

        let cwd = match &self.cwd {
            Some(path) => path.clone(),
            None => std::env::current_dir().map_err(SpawnError::NoWorkingDirectory)?,
        };
        let cwd = cwd.to_string_lossy().into_owned();

        // Everything above is read-only, so nothing exists yet if any of it failed.
        let pane = match (self.placement, anchor) {
            (Placement::Pane, Some(anchor)) => surface::split(&anchor, &cwd, self.focus())?,
            (Placement::Tab, _) => {
                let workspace = self.workspace(sink)?;
                surface::create_tab(workspace.as_deref(), &self.name, &cwd, self.focus())?
            }
            (Placement::Workspace, _) => surface::create_workspace(&self.name, &cwd, self.focus())?,
            // Unreachable: `anchor` is `Some` for exactly `Placement::Pane`, three lines above.
            (Placement::Pane, None) => return Err(SpawnError::MissingAnchor),
        };

        // From here on a failure leaves the pane open and names it: whatever went wrong is on
        // screen in it, and closing it would throw the error away with it.
        self.start_and_prompt(&pane, &kind, &agent_args, sink)
            .map_err(|error| error.note_open_pane(&pane))
    }
}

impl SpawnArgs {
    /// Starts the agent, retrying a pane whose shell has not settled, then delivers the first
    /// prompt if there is one.
    fn start_and_prompt(
        &self,
        pane: &PaneId,
        kind: &str,
        agent_args: &[String],
        sink: &Sink,
    ) -> Result<Spawned, SpawnError> {
        let started = self.start_when_settled(pane, kind, agent_args)?;

        let Some(text) = &self.prompt else {
            return Ok(Spawned {
                placement: self.placement,
                delivered: None,
                agent: started,
            });
        };

        // No composer guard here: no human has touched the pane this just created, and the
        // submission follows `agent start` immediately.
        let wait = Wait {
            until: vec![WORKING.to_owned()],
            timeout: DEFAULT_SETTLE_MS,
        };
        let agent = deliver(pane, text, Some(&wait), sink)?;
        Ok(Spawned {
            placement: self.placement,
            delivered: Some(started.status() != WORKING),
            agent,
        })
    }

    /// `agent start`, retried while herdr says the pane is not yet an available shell.
    ///
    /// Finding 3: twice in six launches, `agent start` refused a just-created pane because its
    /// shell had not reached its prompt. herdr is right to refuse; the caller has to retry rather
    /// than abandon the seat. Every *other* failure returns immediately.
    ///
    /// The retry is silent. A `direnv` or `nvm` in the shell's rc file makes it fire on essentially
    /// every launch, so a diagnostic here would be printed on the success path almost always —
    /// which is not a warning but a progress indicator, and under `--json` an object every caller
    /// skips. What a caller needs is the case where retrying did not help, and
    /// [`SpawnError::PaneNeverSettled`] carries that with the budget it exhausted.
    fn start_when_settled(&self, pane: &PaneId, kind: &str, agent_args: &[String]) -> Result<AgentRecord, SpawnError> {
        for delay in backoff(self.settle_timeout) {
            match agent::start(&self.name, kind, pane, agent_args) {
                Ok(agent) => return Ok(agent),
                Err(error) if error.code() == Some("agent_pane_busy") => std::thread::sleep(delay),
                Err(error) => return Err(SpawnError::Herdr(error)),
            }
        }

        // One last attempt after the budget is spent, so a zero settle timeout still tries once.
        match agent::start(&self.name, kind, pane, agent_args) {
            Ok(agent) => Ok(agent),
            Err(error) if error.code() == Some("agent_pane_busy") => Err(SpawnError::PaneNeverSettled {
                pane: pane.clone(),
                budget_ms: self.settle_timeout,
            }),
            Err(error) => Err(SpawnError::Herdr(error)),
        }
    }
}

// =====================================================================================================================
// Output
// =====================================================================================================================

/// What `spawn` produced.
#[derive(Debug, Serialize)]
pub struct Spawned {
    /// Which of the three surfaces was created.
    placement: Placement,
    /// Whether the first prompt's delivery was proven; absent when no prompt was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    delivered: Option<bool>,
    /// herdr's agent record, nested verbatim.
    agent: AgentRecord,
}

impl Display for Spawned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}) → {}",
            self.agent.name_or_unknown(),
            self.agent.kind().unwrap_or("unknown"),
            self.agent.pane()
        )
    }
}

// =====================================================================================================================
// Errors
// =====================================================================================================================

/// Failure of the spawn flow.
#[derive(Debug, Error)]
pub enum SpawnError {
    /// `--placement pane` with no `HERDR_PANE_ID`.
    #[error("--placement pane needs a calling herdr pane and HERDR_PANE_ID is unset; use --placement tab or workspace")]
    MissingAnchor,
    /// The preset file could not be read, or did not hold the named preset.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// There is no current directory to hand the new surface.
    #[error("cannot read the current directory to use as the new surface's cwd: {0}")]
    NoWorkingDirectory(#[source] std::io::Error),
    /// herdr refused something; its message and code are carried verbatim.
    #[error(transparent)]
    Herdr(#[from] HerdrError),
    /// The first prompt could not be delivered.
    #[error(transparent)]
    Prompt(#[from] crate::cmd::prompt::PromptError),
    /// The new pane never reached its shell prompt inside the settle window.
    #[error("pane {pane} was still not an available shell after {budget_ms}ms; raise --settle-timeout and retry")]
    PaneNeverSettled {
        /// The pane that stayed busy.
        pane: PaneId,
        /// The window that was spent on it.
        budget_ms: u64,
    },
    /// A failure after the surface was created. The surface is left open and named.
    #[error("{error} (pane {pane} left open)")]
    AfterSurface {
        /// The pane that is still there.
        pane: PaneId,
        /// What actually went wrong.
        #[source]
        error: Box<SpawnError>,
    },
}

impl SpawnError {
    /// Notes that `pane` was created before this failure, so the message says where to look.
    ///
    /// The pane is deliberately not closed: whatever went wrong is on screen in it, and closing it
    /// would throw the error away with it.
    fn note_open_pane(self, pane: &PaneId) -> Self {
        Self::AfterSurface {
            pane: pane.clone(),
            error: Box::new(self),
        }
    }
}

impl AsExitStatus for SpawnError {
    fn exit_status(&self) -> ExitStatus {
        match self {
            Self::MissingAnchor => ExitStatus::Usage,
            Self::Config(error) => error.exit_status_hint(),
            Self::NoWorkingDirectory(_) => ExitStatus::Failure,
            Self::Herdr(error) => error.exit_status(),
            Self::Prompt(error) => error.exit_status(),
            Self::PaneNeverSettled { .. } => ExitStatus::Conflict,
            Self::AfterSurface { error, .. } => error.exit_status(),
        }
    }

    fn herdr(&self) -> Option<HerdrRef> {
        match self {
            Self::Herdr(error) => Some(error.reference()),
            Self::Prompt(error) => error.herdr(),
            Self::AfterSurface { error, .. } => error.herdr(),
            Self::MissingAnchor | Self::Config(_) | Self::NoWorkingDirectory(_) | Self::PaneNeverSettled { .. } => None,
        }
    }
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// The calling pane a split anchors on, read from the environment.
///
/// Never herdr's `--current`: that flag resolves server-side to whichever pane is *focused*, which
/// is not this one when the command runs from an unfocused pane.
///
/// # Errors
///
/// [`SpawnError::MissingAnchor`] when the variable is unset or blank, naming the two flags that work
/// outside a herdr pane.
fn anchor(variable: Option<&str>) -> Result<PaneId, SpawnError> {
    variable
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PaneId::from)
        .ok_or(SpawnError::MissingAnchor)
}

/// The delays a `agent_pane_busy` retry spends waiting, inside `budget_ms`.
///
/// Doubles from 250ms to a 2000ms cap, with the last delay shortened so the total lands exactly on
/// the budget rather than overrunning it. An empty schedule means one attempt and no retry.
fn backoff(budget_ms: u64) -> Backoff {
    Backoff {
        remaining_ms: budget_ms,
        next_ms: BACKOFF_FIRST_MS,
    }
}

/// The retry schedule as a sequence: doubling delays, capped, and truncated so the total never
/// overruns the budget.
///
/// An iterator rather than a collected `Vec`, because the sequence is state and a step's length
/// depends on what is left — expressing it as `next` puts the doubling, the cap, and the truncation
/// in one place, and the caller consumes it lazily inside its retry loop rather than paying for a
/// schedule it will usually abandon on the second attempt.
struct Backoff {
    /// Budget not yet handed out; the sequence ends when it reaches zero.
    remaining_ms: u64,
    /// The delay this step would like, before it is truncated to what remains.
    next_ms: u64,
}

impl Iterator for Backoff {
    type Item = Duration;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining_ms == 0 {
            return None;
        }
        let step = self.next_ms.min(self.remaining_ms);
        self.remaining_ms -= step;
        self.next_ms = self.next_ms.saturating_mul(2).min(BACKOFF_CAP_MS);
        Some(Duration::from_millis(step))
    }
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use clap::Parser;

    use super::*;

    #[derive(Debug, Parser)]
    struct Harness {
        #[command(flatten)]
        args: SpawnArgs,
    }

    fn parse(argv: &[&str]) -> SpawnArgs {
        Harness::try_parse_from(argv).expect("parses").args
    }

    fn record() -> AgentRecord {
        serde_json::from_str(r#"{"agent":"claude","agent_status":"working","pane_id":"w4:p17","name":"reviewer"}"#)
            .unwrap()
    }

    /// A typo is answered with the whole value set, which the flag group it replaced could not do:
    /// that reported whichever pair happened to conflict and never named the third.
    #[test]
    fn an_unknown_placement_is_rejected_and_the_error_lists_the_ones_that_exist() {
        let error = Harness::try_parse_from(["spawn", "reviewer", "--placement", "tba"]).unwrap_err();

        let rendered = error.to_string();
        assert!(
            ["pane", "tab", "workspace"]
                .iter()
                .all(|value| rendered.contains(value)),
            "got {rendered}"
        );
    }

    #[test]
    fn placement_defaults_to_splitting_the_calling_pane() {
        // Also the test that exercises the string default: a typo in `default_value` fails here
        // rather than reaching a caller.
        assert_eq!(parse(&["spawn", "reviewer"]).placement, Placement::Pane);
        assert_eq!(
            parse(&["spawn", "reviewer", "--placement", "tab"]).placement,
            Placement::Tab
        );
        assert_eq!(
            parse(&["spawn", "reviewer", "--placement", "workspace"]).placement,
            Placement::Workspace
        );
    }

    #[test]
    fn focus_is_opt_in() {
        // A tool meant to be driven by agents should not steal the human's focus. An interactive
        // shell alias can put --focus back.
        assert_eq!(parse(&["spawn", "reviewer"]).focus(), Focus::Leave);
        assert_eq!(parse(&["spawn", "reviewer", "--focus"]).focus(), Focus::Take);
    }

    #[test]
    fn a_name_herdr_would_refuse_fails_at_parse_time_before_any_surface_exists() {
        assert!(Harness::try_parse_from(["spawn", "Reviewer"]).is_err());
        assert!(Harness::try_parse_from(["spawn", "1st"]).is_err());
    }

    #[test]
    fn splitting_outside_a_herdr_pane_is_a_usage_error_naming_the_fix() {
        let error = anchor(None);

        assert_eq!(error.unwrap_err().exit_status(), ExitStatus::Usage);
    }

    #[test]
    fn the_usage_error_says_which_placements_work_instead() {
        assert_eq!(
            anchor(None).unwrap_err().to_string(),
            "--placement pane needs a calling herdr pane and HERDR_PANE_ID is unset; use --placement tab or workspace"
        );
    }

    #[test]
    fn an_anchor_is_read_from_the_environment_rather_than_resolved_by_herdr() {
        // Never herdr's `--current`: that resolves server-side to whichever pane is *focused*, which
        // is not this one when the command runs from an unfocused pane.
        assert_eq!(anchor(Some("w4:p1")).unwrap(), PaneId::from("w4:p1"));
        assert!(anchor(Some("   ")).is_err(), "a blank variable is as good as unset");
    }

    #[test]
    fn the_retry_window_backs_off_and_lands_exactly_on_its_budget() {
        // 250ms doubling to a 2000ms cap, truncated so the total never overruns --settle-timeout.
        let delays: Vec<u64> = backoff(10_000).map(|delay| delay.as_millis() as u64).collect();

        assert_eq!(delays, [250, 500, 1000, 2000, 2000, 2000, 2000, 250]);
        assert_eq!(delays.iter().sum::<u64>(), 10_000);
    }

    #[test]
    fn a_zero_budget_means_one_attempt_and_no_retry() {
        assert_eq!(backoff(0).next(), None);
        assert_eq!(backoff(100).collect::<Vec<_>>(), [Duration::from_millis(100)]);
        assert_eq!(
            backoff(300).collect::<Vec<_>>(),
            [Duration::from_millis(250), Duration::from_millis(50)]
        );
    }

    /// The schedule is lazy: a retry loop that succeeds early pays for no delay it did not use.
    #[test]
    fn the_schedule_is_produced_a_step_at_a_time() {
        let mut delays = backoff(10_000);
        assert_eq!(delays.next(), Some(Duration::from_millis(250)));
        assert_eq!(delays.next(), Some(Duration::from_millis(500)));
        // The remaining 9_250ms is never computed, because nothing asked for it.
    }

    #[test]
    fn a_spawn_reports_the_agent_the_placement_and_whether_the_first_prompt_landed() {
        let spawned = Spawned {
            placement: Placement::Tab,
            delivered: Some(true),
            agent: record(),
        };

        assert_eq!(spawned.to_string(), "reviewer (claude) → w4:p17");
        assert_eq!(
            serde_json::to_string(&spawned).unwrap(),
            r#"{"placement":"tab","delivered":true,"agent":{"agent":"claude","agent_status":"working","pane_id":"w4:p17","name":"reviewer"}}"#
        );
    }

    #[test]
    fn a_spawn_with_no_prompt_omits_delivered_rather_than_reporting_false() {
        let spawned = Spawned {
            placement: Placement::Pane,
            delivered: None,
            agent: record(),
        };

        assert_eq!(
            serde_json::to_string(&spawned).unwrap(),
            r#"{"placement":"pane","agent":{"agent":"claude","agent_status":"working","pane_id":"w4:p17","name":"reviewer"}}"#
        );
    }

    #[test]
    fn a_failure_after_the_surface_exists_reports_the_pane_it_left_open() {
        // Whatever went wrong is on screen in that pane, and closing it would throw the error away.
        let error = SpawnError::AfterSurface {
            pane: PaneId::from("w4:p17"),
            error: Box::new(SpawnError::Herdr(crate::herdr::HerdrError::Refused {
                command: "agent start".to_owned(),
                code: "agent_name_taken".to_owned(),
                message: "agent name reviewer is already used".to_owned(),
            })),
        };

        assert_eq!(
            error.to_string(),
            "agent name reviewer is already used (pane w4:p17 left open)"
        );
        assert_eq!(
            error.exit_status(),
            ExitStatus::Conflict,
            "the wrapper forwards the inner status"
        );
        assert!(error.herdr().is_some(), "and herdr's provenance too");
    }

    #[test]
    fn a_busy_pane_that_never_settles_is_a_retryable_conflict() {
        let error = SpawnError::PaneNeverSettled {
            pane: PaneId::from("w4:p17"),
            budget_ms: 10_000,
        };

        assert_eq!(error.exit_status(), ExitStatus::Conflict);
        assert_eq!(
            error.to_string(),
            "pane w4:p17 was still not an available shell after 10000ms; raise --settle-timeout and retry"
        );
    }
}
