//! `spawn` — create a surface and start a preset-configured agent in it.
//!
//! The longest file here because it is the longest command: five ordered steps across four
//! placements, each of which can fail after the previous one has already changed something in herdr.
//! It reads as one transaction, so splitting it would hide that ordering and it stays one file.
//!
//! [`anchor`] and [`SpawnArgs::workspace`] together answer "where is the caller", which is a real
//! boundary rather than a line-count one — it is what a `--from` flag would override, and that is the
//! day they move. Until something other than this flow asks that question they are steps of the flow
//! directly above them, and a reader following `execute` finds them where they are used.

use std::fmt::Display;
use std::path::PathBuf;

use clap::Args;
use clap_stdin::MaybeStdin;
use serde::Serialize;
use thiserror::Error;

use crate::cmd::prompt::deliver;
use crate::cmd::{AsExitStatus, Cmd, ExitStatus};
use crate::config::{ConfigError, Presets};
use crate::core::{AgentName, Backoff, NonEmptyText, PaneId, Sink};
use crate::herdr::agent::{self, AgentRecord, WORKING, Wait};
use crate::herdr::surface::{self, Checkout, Focus, Placement};
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
    herdr-agent-tools spawn big --placement workspace --preset fable -- --resume\n  \
    herdr-agent-tools spawn fixer --placement worktree --branch worktree/flake-fix")]
pub struct SpawnArgs {
    /// The agent's name; must satisfy herdr's rule, which is checked before anything is created.
    name: AgentName,

    /// Where the agent's pane comes from: a split of the calling pane, which needs `HERDR_PANE_ID`,
    /// or the root pane of a new tab, workspace, or Git worktree cut from `--cwd`.
    // Not a doc comment: a field's doc comment on this struct is its `--help` text, and why the
    // default is a string is not something a caller needs. `default_value_t` would want a `Display`
    // on `Placement` whose only consumer is this line, and which has to agree with the value parser
    // to work at all. A parse with no `--placement` exercises the string, so a typo in it fails a
    // test rather than reaching a caller.
    #[arg(long, value_enum, default_value = "pane")]
    placement: Placement,

    /// The branch a `--placement worktree` spawn checks out; herdr generates one when unset.
    #[arg(long, value_name = "NAME")]
    branch: Option<String>,

    /// The ref that branch starts from; herdr uses `HEAD` when unset.
    #[arg(long, value_name = "REF")]
    base: Option<String>,

    /// The preset to start; defaults to the config file's `default`.
    #[arg(long, value_name = "NAME")]
    preset: Option<String>,

    /// Read this preset file instead of the one in the config directory.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// A first prompt to deliver once the agent is up; `-` reads it from stdin.
    // `allow_hyphen_values` for the reason given on `prompt`'s own text argument: a prompt opening
    // with `--` must be delivered rather than echoed back in a parser diagnostic. `--` cannot repair
    // it here at all — that separator already belongs to `agent_args`.
    #[arg(long, value_name = "TEXT", allow_hyphen_values = true)]
    prompt: Option<MaybeStdin<NonEmptyText>>,

    /// The directory this agent works on, or for a worktree the checkout it is cut from; defaults
    /// to the current one.
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

    /// Refuses `--branch` or `--base` under a placement with no worktree to apply them to.
    ///
    /// clap cannot express this: `conflicts_with` takes an argument id, not a value predicate, so
    /// "valid only when `--placement` is `worktree`" has nowhere to live but a pre-check. It runs
    /// among the others, before anything exists, which is what keeps it a pre-check.
    ///
    /// Ignoring the flag was the alternative and is worse. A caller templating `--branch` into
    /// every spawn would never learn that the isolation it asked for did not happen — and a wrong
    /// answer nobody is told about is the failure this whole tool is shaped to avoid.
    ///
    /// # Errors
    ///
    /// [`SpawnError::WorktreeOnlyFlag`], naming the flag and the placement that would honor it.
    fn check_worktree_flags(&self) -> Result<(), SpawnError> {
        if self.placement == Placement::Worktree {
            return Ok(());
        }
        for (flag, value) in [("--branch", &self.branch), ("--base", &self.base)] {
            if value.is_some() {
                return Err(SpawnError::WorktreeOnlyFlag { flag });
            }
        }
        Ok(())
    }

    /// Makes the surface this placement calls for, and reports the checkout when it made one.
    ///
    /// Named rather than inlined into [`execute`](Cmd::execute) because it is the one step with a
    /// shape of its own: four placements, one of which produces a second thing worth reporting.
    /// `execute` keeps the ordering the transaction depends on, and this keeps the branching, so
    /// neither has to be read for the other's sake.
    ///
    /// `anchor` is resolved by the caller because it is a *pre-check* — a `--placement pane` with
    /// no calling pane has to fail before this runs, not inside it.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Herdr`] if herdr refused to make the surface, or [`SpawnError::MissingAnchor`]
    /// for the split that arrived without one.
    fn create_surface(
        &self,
        anchor: Option<PaneId>,
        cwd: &str,
        sink: &Sink,
    ) -> Result<(PaneId, Option<Checkout>), SpawnError> {
        // Only the worktree arm has a second thing to report. `--cwd` means the source checkout
        // there rather than the pane's own directory, and no workspace is pinned: a worktree brings
        // its own, and herdr resolves the source from the path instead.
        match (self.placement, anchor) {
            (Placement::Pane, Some(anchor)) => Ok((surface::split(&anchor, cwd, self.focus())?, None)),
            (Placement::Tab, _) => {
                let workspace = self.workspace(sink)?;
                let pane = surface::create_tab(workspace.as_deref(), &self.name, cwd, self.focus())?;
                Ok((pane, None))
            }
            (Placement::Workspace, _) => Ok((surface::create_workspace(&self.name, cwd, self.focus())?, None)),
            (Placement::Worktree, _) => {
                let (pane, checkout) = surface::create_worktree(
                    &self.name,
                    cwd,
                    self.branch.as_deref(),
                    self.base.as_deref(),
                    self.focus(),
                )?;
                Ok((pane, Some(checkout)))
            }
            // Unreachable: the caller resolves `anchor` to `Some` for exactly `Placement::Pane`.
            (Placement::Pane, None) => Err(SpawnError::MissingAnchor),
        }
    }
}

impl Cmd for SpawnArgs {
    type Ok = Spawned;
    type Err = SpawnError;

    /// Pre-checks, preset, surface, agent, first prompt — in that order, because a pre-check that
    /// runs after a surface exists is not a pre-check.
    fn execute(self, sink: &Sink) -> Result<Self::Ok, Self::Err> {
        self.check_worktree_flags()?;
        let anchor = match self.placement {
            Placement::Pane => Some(anchor(std::env::var(PANE_VARIABLE).ok().as_deref())?),
            Placement::Tab | Placement::Workspace | Placement::Worktree => None,
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
        let (pane, worktree) = self.create_surface(anchor, &cwd, sink)?;

        // From here on a failure leaves the pane open and names it: whatever went wrong is on
        // screen in it, and closing it would throw the error away with it.
        self.start_and_prompt(&pane, &kind, &agent_args, worktree, sink)
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
        worktree: Option<Checkout>,
        sink: &Sink,
    ) -> Result<Spawned, SpawnError> {
        let started = self.start_when_settled(pane, kind, agent_args)?;

        let Some(text) = &self.prompt else {
            return Ok(Spawned {
                placement: self.placement,
                delivered: None,
                worktree,
                agent: started,
            });
        };

        // No composer guard here: no human has touched the pane this just created, and the
        // submission follows `agent start` immediately.
        let wait = Wait {
            until: vec![WORKING.to_owned()],
            timeout: DEFAULT_SETTLE_MS,
        };

        // `prompt`'s honest gap, met here too: an agent that is already `working` matches
        // `--until working` instantly, which proves nothing. Said out loud rather than left to the
        // `delivered` field, which only the `--json` reader sees — a caller reading the one line
        // would otherwise wait forever on work that was never proven to start.
        let verified = started.status() != WORKING;
        if !verified {
            sink.warn(&format!(
                "{} was already working when its first prompt was sent, so delivery could not be verified",
                self.name
            ));
        }

        let agent = deliver(pane, text, Some(&wait), sink)?;
        Ok(Spawned {
            placement: self.placement,
            delivered: Some(verified),
            worktree,
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
        for delay in Backoff::within(self.settle_timeout) {
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
    /// Which of the four surfaces was created.
    placement: Placement,
    /// Whether the first prompt's delivery was proven; absent when no prompt was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    delivered: Option<bool>,
    /// The checkout a `--placement worktree` spawn landed on; absent for the other three.
    ///
    /// Reported because the branch is usually herdr's to generate, which makes the response that
    /// created it the one cheap moment it is knowable. Without this a caller runs `worktree list`
    /// against the source repo and then guesses which checkout is the one it just made. Both fields
    /// reach the human line too — see this type's [`Display`].
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<Checkout>,
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
        )?;
        // Both halves of the checkout, because both are answers only this response holds: the branch
        // is usually herdr's to generate, and the path is where a caller has to `cd` to work in it.
        // The line is longer for it, and a re-query the caller cannot make cheaply is worse. The
        // branch may be absent, so the path stands alone rather than leaving an empty bracket.
        if let Some(checkout) = &self.worktree {
            match &checkout.branch {
                Some(branch) => write!(f, " [{branch} in {}]", checkout.path)?,
                None => write!(f, " [{}]", checkout.path)?,
            }
        }
        Ok(())
    }
}

// =====================================================================================================================
// Errors
// =====================================================================================================================

/// Failure of the spawn flow.
#[derive(Debug, Error)]
pub enum SpawnError {
    /// `--placement pane` with no `HERDR_PANE_ID`.
    #[error(
        "--placement pane needs a calling herdr pane and HERDR_PANE_ID is unset; \
         use --placement tab, workspace, or worktree"
    )]
    MissingAnchor,
    /// `--branch` or `--base` under a placement that makes no worktree.
    #[error("{flag} needs --placement worktree")]
    WorktreeOnlyFlag {
        /// The flag that cannot be honored here.
        flag: &'static str,
    },
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
            Self::MissingAnchor | Self::WorktreeOnlyFlag { .. } => ExitStatus::Usage,
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
            Self::MissingAnchor
            | Self::WorktreeOnlyFlag { .. }
            | Self::Config(_)
            | Self::NoWorkingDirectory(_)
            | Self::PaneNeverSettled { .. } => None,
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
            ["pane", "tab", "workspace", "worktree"]
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
        assert_eq!(
            parse(&["spawn", "reviewer", "--placement", "worktree"]).placement,
            Placement::Worktree
        );
    }

    #[test]
    fn focus_is_opt_in() {
        // A tool meant to be driven by agents should not steal the human's focus. An interactive
        // shell alias can put --focus back.
        assert_eq!(parse(&["spawn", "reviewer"]).focus(), Focus::Leave);
        assert_eq!(parse(&["spawn", "reviewer", "--focus"]).focus(), Focus::Take);
    }

    /// A first prompt that opens with a dash is delivered rather than rejected.
    ///
    /// The parse is the redaction: a value clap accepts is a value no clap diagnostic can repeat.
    /// `--` is no repair here — it already belongs to `agent_args` — so this argument has to take
    /// hyphen-leading text on its own.
    #[test]
    fn a_first_prompt_that_opens_with_a_dash_is_prompt_text_rather_than_a_flag() {
        for text in ["--force the issue", "-e", "--focus"] {
            let args = parse(&["spawn", "reviewer", "--prompt", text]);

            assert_eq!(args.prompt.expect("a prompt was given").to_string(), text, "{text}");
        }
    }

    #[test]
    fn a_flag_after_the_first_prompt_is_still_a_flag() {
        // `allow_hyphen_values` must claim this option's own value and nothing past it.
        let args = parse(&[
            "spawn",
            "reviewer",
            "--prompt",
            "--go",
            "--placement",
            "tab",
            "--",
            "--resume",
        ]);

        assert_eq!(args.prompt.as_ref().expect("a prompt was given").to_string(), "--go");
        assert_eq!(args.placement, Placement::Tab);
        assert_eq!(args.agent_args, ["--resume"]);
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
        // All three that need no calling pane, worktree included: it creates its own workspace and
        // resolves its source from `--cwd`, so it works outside a herdr session too.
        assert_eq!(
            anchor(None).unwrap_err().to_string(),
            "--placement pane needs a calling herdr pane and HERDR_PANE_ID is unset; \
             use --placement tab, workspace, or worktree"
        );
    }

    #[test]
    fn branch_and_base_are_refused_under_a_placement_that_makes_no_worktree() {
        // Not ignored. A caller templating --branch into every spawn would otherwise never learn
        // that the isolation it asked for did not happen.
        for placement in ["pane", "tab", "workspace"] {
            for flag in ["--branch", "--base"] {
                let error = parse(&["spawn", "reviewer", "--placement", placement, flag, "x"])
                    .check_worktree_flags()
                    .unwrap_err();

                assert_eq!(error.to_string(), format!("{flag} needs --placement worktree"));
                assert_eq!(error.exit_status(), ExitStatus::Usage);
            }
        }
    }

    #[test]
    fn branch_and_base_are_accepted_under_worktree_placement_and_optional_there() {
        let both = parse(&[
            "spawn",
            "reviewer",
            "--placement",
            "worktree",
            "--branch",
            "worktree/flake-fix",
            "--base",
            "origin/main",
        ]);

        assert!(both.check_worktree_flags().is_ok());
        assert_eq!(both.branch.as_deref(), Some("worktree/flake-fix"));
        assert_eq!(both.base.as_deref(), Some("origin/main"));

        // Neither is required: herdr generates a branch and bases on HEAD, and this crate states
        // no default of its own.
        let neither = parse(&["spawn", "reviewer", "--placement", "worktree"]);
        assert!(neither.check_worktree_flags().is_ok());
        assert_eq!(neither.branch, None);
        assert_eq!(neither.base, None);
    }

    #[test]
    fn an_anchor_is_read_from_the_environment_rather_than_resolved_by_herdr() {
        // Never herdr's `--current`: that resolves server-side to whichever pane is *focused*, which
        // is not this one when the command runs from an unfocused pane.
        assert_eq!(anchor(Some("w4:p1")).unwrap(), PaneId::from("w4:p1"));
        assert!(anchor(Some("   ")).is_err(), "a blank variable is as good as unset");
    }

    /// The settle window is spent on a schedule this file no longer owns.
    ///
    /// [`Backoff`]'s own tests cover the doubling, the cap, and landing exactly on the budget. What is
    /// spawn's is the *policy*: which failure is worth retrying, and how long to keep at it — and the
    /// one attempt after the schedule runs out, so a zero `--settle-timeout` still tries once. That
    /// last part is not tested here, because proving it needs a herdr that refuses on demand and no
    /// test in this crate invokes herdr.
    #[test]
    fn the_settle_window_is_the_default_unless_a_caller_narrows_it() {
        assert_eq!(parse(&["spawn", "reviewer"]).settle_timeout, DEFAULT_SETTLE_MS);
        assert_eq!(
            parse(&["spawn", "reviewer", "--settle-timeout", "0"]).settle_timeout,
            0,
            "a caller that wants no retry at all can say so"
        );
    }

    #[test]
    fn a_spawn_reports_the_agent_the_placement_and_whether_the_first_prompt_landed() {
        let spawned = Spawned {
            placement: Placement::Tab,
            delivered: Some(true),
            worktree: None,
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
            worktree: None,
            agent: record(),
        };

        assert_eq!(
            serde_json::to_string(&spawned).unwrap(),
            r#"{"placement":"pane","agent":{"agent":"claude","agent_status":"working","pane_id":"w4:p17","name":"reviewer"}}"#
        );
    }

    /// A worktree spawn reports the checkout, because nothing else will.
    ///
    /// The branch is herdr's to generate unless the caller named one, so this response is the one
    /// cheap moment either half is knowable — and the path is what a caller has to `cd` to. Both
    /// forms carry both.
    #[test]
    fn a_worktree_spawn_reports_the_branch_it_landed_on_and_the_path_beside_it() {
        let spawned = Spawned {
            placement: Placement::Worktree,
            delivered: None,
            worktree: Some(Checkout {
                branch: Some("worktree/lucky-harbor-8e01".to_owned()),
                path: "/work/trees/repo/worktree-lucky-harbor-8e01".to_owned(),
            }),
            agent: record(),
        };

        assert_eq!(
            spawned.to_string(),
            "reviewer (claude) → w4:p17 [worktree/lucky-harbor-8e01 in /work/trees/repo/worktree-lucky-harbor-8e01]"
        );
        assert_eq!(
            serde_json::to_string(&spawned).unwrap(),
            r#"{"placement":"worktree","worktree":{"branch":"worktree/lucky-harbor-8e01","path":"/work/trees/repo/worktree-lucky-harbor-8e01"},"agent":{"agent":"claude","agent_status":"working","pane_id":"w4:p17","name":"reviewer"}}"#
        );
    }

    #[test]
    fn a_worktree_with_no_branch_reports_the_path_alone_rather_than_an_empty_bracket() {
        // herdr's branch field is optional, so the human line has to survive a `None` it does not
        // expect. The path is the half that is always there, and it is the half a caller acts on.
        let spawned = Spawned {
            placement: Placement::Worktree,
            delivered: None,
            worktree: Some(Checkout {
                branch: None,
                path: "/work/trees/repo/detached".to_owned(),
            }),
            agent: record(),
        };

        assert_eq!(
            spawned.to_string(),
            "reviewer (claude) → w4:p17 [/work/trees/repo/detached]"
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
