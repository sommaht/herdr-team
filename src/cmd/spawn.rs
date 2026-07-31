//! `spawn` — create a surface and start a configured agent in it.
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
use std::path::{Path, PathBuf};

use clap::Args;
use clap_stdin::MaybeStdin;
use serde::Serialize;
use thiserror::Error;

use crate::cmd::msg::envelope::{OPERATOR, Reply};
use crate::cmd::msg::{Proof, deliver};
use crate::cmd::{AsExitStatus, Cmd, ExitStatus};
use crate::config::{Config, ConfigError};
use crate::core::{AgentName, Backoff, NonEmptyText, PaneId, Sink};
use crate::herdr::agent::{self, AgentRecord};
use crate::herdr::surface::{self, Checkout, Focus, Placement};
use crate::herdr::{HerdrError, HerdrRef, PANE_VARIABLE};

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// How long `agent start` is retried while the new pane's shell is still starting, in milliseconds.
///
/// Finding 3: creating a tab or workspace races with slow shell init, and herdr correctly refuses a
/// pane that has not reached its prompt. Ten seconds covers a shell running a directory-environment
/// hook without leaving a caller hanging on one that is genuinely broken.
const DEFAULT_SETTLE_MS: u64 = 10_000;

// =====================================================================================================================
// Spawn Args
// =====================================================================================================================

/// Create a pane, tab, or workspace and start a configured agent in it.
///
/// herdr starts an agent only in a pane that already exists and is sitting at an interactive shell
/// prompt, so this does both halves: it creates the surface, reads back the new pane's id, and
/// starts the agent there.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  \
    herdr-team spawn reviewer --placement tab --agent opus\n  \
    herdr-team spawn fixer --agent sonnet --msg \"Fix the flaky tests\"\n  \
    git diff | herdr-team spawn reviewer --msg -\n  \
    herdr-team spawn big --placement workspace --agent fable -- --resume\n  \
    herdr-team spawn fixer --placement worktree --branch worktree/flake-fix")]
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

    /// The agent to start; defaults to the config's `default`.
    #[arg(long, value_name = "NAME")]
    agent: Option<String>,

    /// Renamed to `--agent`; declared only so the rename can be reported rather than guessed at.
    #[arg(long, value_name = "NAME", hide = true)]
    preset: Option<String>,

    /// Read this config file instead of the one in the config directory.
    ///
    /// The repository's own config is still merged over it.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// A first message to deliver once the agent is up; `-` reads it from stdin.
    ///
    /// Wrapped in the same envelope `msg` sends, which is why the reply flags below apply to it.
    // `allow_hyphen_values` for the reason given on `msg`'s own text argument: text opening with
    // `--` must be delivered rather than echoed back in a parser diagnostic. `--` cannot repair it
    // here at all — that separator already belongs to `agent_args`. `--prompt` is the spelling this
    // carried until the command was renamed, kept working and kept out of the help.
    #[arg(long, alias = "prompt", value_name = "TEXT", allow_hyphen_values = true)]
    msg: Option<MaybeStdin<NonEmptyText>>,

    /// Omit the first message's reply instructions, closing the loop instead of inviting an answer.
    #[arg(long, conflicts_with = "reply_to")]
    no_reply: bool,

    /// Address the first message's reply instructions at this target instead of at the sender.
    #[arg(long, value_name = "TARGET")]
    reply_to: Option<String>,

    /// The directory this agent works on; defaults to the current one. A worktree spawn cuts from
    /// the repository holding it and reopens at the same relative path inside the new checkout.
    #[arg(long, value_name = "PATH")]
    cwd: Option<PathBuf>,

    /// Focus the new surface. Off by default, so a background launch does not steal the cursor.
    #[arg(long)]
    focus: bool,

    /// Milliseconds to keep retrying `agent start` while the new pane's shell is still starting.
    #[arg(long, value_name = "MS", default_value_t = DEFAULT_SETTLE_MS)]
    settle_timeout: u64,

    /// Extra arguments appended after the agent's, passed to the agent verbatim.
    ///
    /// No merging and no de-duplication, so the agent's own last-flag-wins rules settle any conflict
    /// with the config.
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

    /// The caller's own directory relative to its repository root, for a worktree spawn to reopen
    /// at.
    ///
    /// One read-only herdr call, made before anything is created, and only for the placement that
    /// has a second checkout to map into. `None` from a caller already at the repository root, which
    /// skips the placement step rather than running a no-op `cd`.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Herdr`] when herdr cannot resolve a repository for `cwd` — `not_git_worktree`
    /// for a caller outside one. That refusal now arrives here rather than from `worktree create`,
    /// which is where a precondition belongs: nothing has been created yet, so a rejected command
    /// has changed nothing.
    fn subdirectory(&self, cwd: &str) -> Result<Option<String>, SpawnError> {
        if self.placement != Placement::Worktree {
            return Ok(None);
        }
        Ok(relative_to(&surface::repo_root(cwd)?, cwd))
    }

    /// Where the first prompt says a reply should go.
    ///
    /// The flags are declared here rather than flattened in from `prompt` — a shared group would put
    /// both commands' flags in one help section — but the decision they encode is
    /// [`Reply::from_flags`]'s, so a first prompt cannot mean something different by them.
    fn reply(&self) -> Reply {
        Reply::from_flags(self.reply_to.as_deref(), self.no_reply)
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

    /// Refuses a flag this build renamed, naming what replaced it.
    ///
    /// Not left to clap: its suggestion machinery scores `--preset` too far from `--agent` to offer
    /// it, so the rejection a caller would otherwise meet says only that the argument is
    /// unrecognized.
    ///
    /// # Errors
    ///
    /// [`SpawnError::RenamedFlag`].
    fn check_renamed_flags(&self) -> Result<(), SpawnError> {
        if self.preset.is_some() {
            return Err(SpawnError::RenamedFlag { old: "--preset", new: "--agent" });
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

    /// Pre-checks, config, surface, agent, first prompt — in that order, because a pre-check that
    /// runs after a surface exists is not a pre-check.
    fn execute(self, sink: &Sink) -> Result<Self::Ok, Self::Err> {
        // `AgentName` derefs to `str`, so this compares the name itself rather than the newtype.
        if &*self.name == OPERATOR {
            return Err(SpawnError::ReservedName);
        }
        self.check_renamed_flags()?;
        self.check_worktree_flags()?;
        let anchor = match self.placement {
            Placement::Pane => Some(anchor(std::env::var(PANE_VARIABLE).ok().as_deref())?),
            Placement::Tab | Placement::Workspace | Placement::Worktree => None,
        };

        // Resolved before the config, because it is where the repository layer's walk starts: the
        // config that applies is the one belonging to the tree this agent will work in.
        let cwd = match &self.cwd {
            Some(path) => path.clone(),
            None => std::env::current_dir().map_err(SpawnError::NoWorkingDirectory)?,
        };

        // `as_path`, not `&cwd`: `Option<&PathBuf>` does not coerce to `Option<&Path>`.
        let config = Config::load(self.config.as_deref(), Some(cwd.as_path()), sink)?;
        let agent = config.resolve(self.agent.as_deref())?;
        let mut args = agent.agent_args();
        args.extend(self.agent_args.iter().cloned());
        let configured = Configured {
            kind: agent.kind().to_owned(),
            args,
            // Read here, among the other pre-checks: a missing brief is refused while a refusal is
            // still free, rather than after a surface exists.
            brief: agent.brief()?,
        };

        let cwd = cwd.to_string_lossy().into_owned();

        // Read before anything is created, so a caller outside a repository is refused while a
        // refusal is still free.
        let subdirectory = self.subdirectory(&cwd)?;

        // Everything above is read-only, so nothing exists yet if any of it failed.
        let (pane, worktree) = self.create_surface(anchor, &cwd, sink)?;

        // From here on a failure leaves the pane open and names it: whatever went wrong is on
        // screen in it, and closing it would throw the error away with it. Every step past this
        // point is inside one call, so that note has one owner rather than a wrapper per step.
        self.start_and_prompt(&pane, &configured, worktree, subdirectory.as_deref(), sink)
            .map_err(|error| error.note_open_pane(&pane))
    }
}

/// What the config settled for this spawn.
///
/// One value rather than three parameters, because they are one answer — the agent `--agent` named,
/// resolved — and they are read together at the single call site that starts it.
struct Configured {
    /// The agent kind, passed to `agent start --kind` untouched.
    kind: String,
    /// The exact argument vector, the caller's `-- <extra>` already appended.
    args: Vec<String>,
    /// The agent's brief, which precedes the caller's first prompt.
    brief: Option<NonEmptyText>,
}

impl SpawnArgs {
    /// Places the pane, starts the agent, retrying one whose shell has not settled, then delivers
    /// the first prompt if there is one.
    ///
    /// Everything that happens after the surface exists, which is what makes it the one place the
    /// caller's `note_open_pane` has to wrap.
    fn start_and_prompt(
        &self,
        pane: &PaneId,
        configured: &Configured,
        worktree: Option<Checkout>,
        subdirectory: Option<&str>,
        sink: &Sink,
    ) -> Result<Spawned, SpawnError> {
        // Before the agent, because `agent start` inherits the shell's directory — after it, the
        // shell would move and the agent would not.
        if let (Some(checkout), Some(relative)) = (&worktree, subdirectory) {
            self.open_subdirectory(pane, &checkout.path, relative, sink)?;
        }

        let started = self.start_when_settled(pane, &configured.kind, &configured.args)?;

        // The agent's configured brief and the caller's `--msg` are one message: an agent that got
        // two would answer the first before hearing the second.
        let Some(text) = first_prompt(configured.brief.as_ref(), self.msg.as_deref()) else {
            return Ok(Spawned {
                placement: self.placement,
                delivered: None,
                worktree,
                agent: started,
            });
        };

        // No composer guard here: no human has touched the pane this just created, and the
        // submission follows `agent start` immediately.
        //
        // `prompt`'s problem is met here too, and by the same rule: an agent that came up working
        // has no state change left for herdr to match, so its pane is read for the message instead.
        let proof = Proof::for_delivery(started.status(), DEFAULT_SETTLE_MS);
        let submission = deliver(pane, &text, &self.reply(), &proof, sink)?;

        // Said out loud rather than left to the `delivered` field, which only the `--json` reader
        // sees — a caller reading the one line would otherwise wait forever on work that was never
        // proven to start.
        if !submission.proven {
            sink.warn(&format!(
                "{} was already working when its first prompt was sent, and its pane never showed \
                 the message, so delivery is unproven",
                self.name
            ));
        }

        Ok(Spawned {
            placement: self.placement,
            delivered: Some(submission.proven),
            worktree,
            agent: submission.agent,
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

    /// Opens the new checkout's pane at the caller's own subdirectory, before the agent starts in
    /// it.
    ///
    /// `agent start` launches the agent *through* the pane's shell, so the shell's directory at
    /// launch is the agent's directory — which is what makes one [`surface::open_at`] enough, with
    /// no pane split, moved, or closed.
    ///
    /// Never a refusal. Both ways of missing the subdirectory warn and return `Ok`, for the reason
    /// [`Unplaced`] records.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Herdr`] if herdr refused the `pane run` or the `pane get` outright — a
    /// transport failure rather than a shell that has not caught up.
    fn open_subdirectory(&self, pane: &PaneId, checkout: &str, relative: &str, sink: &Sink) -> Result<(), SpawnError> {
        let Some(target) = target_in(checkout, relative) else {
            sink.warn(&Unplaced::Absent.warning(relative));
            return Ok(());
        };
        let target = target.to_string_lossy().into_owned();

        // The same budget and schedule `agent start` retries on, and for the same cause: a shell
        // running a directory-environment hook is not at its prompt yet, and `pane run` types into
        // it regardless — so text sent too early is lost outright rather than queued. Re-sent each
        // round rather than polled, because a lost `cd` is never going to arrive on its own.
        for delay in Backoff::within(self.settle_timeout) {
            if arrived(pane, &target)? {
                return Ok(());
            }
            std::thread::sleep(delay);
        }

        // One last attempt after the budget is spent, so a zero settle timeout still tries once.
        if !arrived(pane, &target)? {
            sink.warn(&Unplaced::NeverArrived.warning(relative));
        }
        Ok(())
    }
}

// =====================================================================================================================
// Placement
// =====================================================================================================================

/// Why an agent could not be started in the subdirectory the caller asked for.
///
/// Neither is a failure. By the time either is knowable the checkout exists, and this crate does not
/// tear down what it created — the same reason a failure after a surface exists leaves the pane open
/// and names it. A worktree deleted to report a directory that was gitignored anyway is a worse
/// answer than one that works from the root and says so.
///
/// One type for both because they share a wording rule: name the directory that was missed and carry
/// nothing else. The checkout path is the result line's to report.
#[derive(Clone, Copy, Debug)]
enum Unplaced {
    /// The directory is not in the fresh checkout: gitignored, untracked, or absent from `--base`.
    Absent,
    /// The pane's shell never reached the directory inside the settle window.
    NeverArrived,
}

impl Unplaced {
    /// What the caller is told.
    fn warning(self, directory: &str) -> String {
        match self {
            Self::Absent => format!("{directory} is not in the new checkout, so the agent starts at its root"),
            Self::NeverArrived => {
                format!("the new pane never reached {directory}, so the agent starts at the checkout root")
            }
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
    /// The agent name is the one reserved to mark a human sender.
    #[error("'{OPERATOR}' is reserved: it marks a human sender in delivered mail")]
    ReservedName,
    /// `--branch` or `--base` under a placement that makes no worktree.
    #[error("{flag} needs --placement worktree")]
    WorktreeOnlyFlag {
        /// The flag that cannot be honored here.
        flag: &'static str,
    },
    /// A flag this build renamed.
    #[error("{old} is now {new}")]
    RenamedFlag {
        /// What the caller typed.
        old: &'static str,
        /// What it is called now.
        new: &'static str,
    },
    /// The config could not be read, or did not hold the named agent.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// There is no current directory to hand the new surface.
    #[error("cannot read the current directory to use as the new surface's cwd: {0}")]
    NoWorkingDirectory(#[source] std::io::Error),
    /// herdr refused something; its message and code are carried verbatim.
    #[error(transparent)]
    Herdr(#[from] HerdrError),
    /// The first message could not be delivered.
    #[error(transparent)]
    Msg(#[from] crate::cmd::msg::MsgError),
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
            Self::MissingAnchor | Self::WorktreeOnlyFlag { .. } | Self::RenamedFlag { .. } | Self::ReservedName => {
                ExitStatus::Usage
            }
            Self::Config(error) => error.exit_status_hint(),
            Self::NoWorkingDirectory(_) => ExitStatus::Failure,
            Self::Herdr(error) => error.exit_status(),
            Self::Msg(error) => error.exit_status(),
            Self::PaneNeverSettled { .. } => ExitStatus::Conflict,
            Self::AfterSurface { error, .. } => error.exit_status(),
        }
    }

    fn herdr(&self) -> Option<HerdrRef> {
        match self {
            Self::Herdr(error) => Some(error.reference()),
            Self::Msg(error) => error.herdr(),
            Self::AfterSurface { error, .. } => error.herdr(),
            Self::MissingAnchor
            | Self::WorktreeOnlyFlag { .. }
            | Self::RenamedFlag { .. }
            | Self::ReservedName
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

/// The first prompt: the agent's brief, the caller's text, or the brief followed by it.
///
/// The brief comes first so the caller's instruction reads as an instruction about it, separated by a
/// blank line so a brief ending mid-paragraph does not run into the instruction.
///
/// `None` only when there is neither, which is a spawn with nothing to deliver. Composed rather than
/// re-parsed: both halves are already non-blank, so the result cannot be.
fn first_prompt(brief: Option<&NonEmptyText>, text: Option<&NonEmptyText>) -> Option<NonEmptyText> {
    match (brief, text) {
        (Some(brief), Some(text)) => Some(NonEmptyText::composed(format!("{brief}\n\n{text}"))),
        (Some(only), None) | (None, Some(only)) => Some(only.clone()),
        (None, None) => None,
    }
}

/// Where `cwd` sits inside `repo_root`, or `None` when there is nothing to place.
///
/// `None` covers two cases that want the same answer: a caller already at the repository root, which
/// has no subdirectory to reopen at, and a `cwd` that is not inside `repo_root` at all.
///
/// [`Path::strip_prefix`] rather than [`str::strip_prefix`], because it compares components. The
/// string form answers `"y/src"` for `/repo` against `/repository/src`, and it would have to be
/// taught about a trailing separator on either side, which the path form already knows.
fn relative_to(repo_root: &str, cwd: &str) -> Option<String> {
    let relative = Path::new(cwd).strip_prefix(repo_root).ok()?;
    if relative.as_os_str().is_empty() {
        None
    } else {
        Some(relative.to_string_lossy().into_owned())
    }
}

/// The directory in a fresh checkout to open at, or `None` when the checkout does not have it.
///
/// One `is_dir` rather than a process, a herdr call, or a read of the shell's own error text: the
/// checkout is local and so is this tool. Answered before anything is typed into the pane, which is
/// what keeps the two failures apart — the retry loop that follows only ever waits for a shell.
fn target_in(checkout: &str, relative: &str) -> Option<PathBuf> {
    let target = Path::new(checkout).join(relative);
    target.is_dir().then_some(target)
}

/// Sends the directory change, then asks the pane where its shell actually ended up.
///
/// Testing the outcome rather than assuming the command landed is the whole point: `pane run`
/// succeeds whether the text reached a live prompt or a shell that had not started, and only
/// [`surface::foreground_cwd`] can tell those apart.
///
/// # Errors
///
/// Whatever either herdr call returned.
fn arrived(pane: &PaneId, target: &str) -> Result<bool, HerdrError> {
    surface::open_at(pane, target)?;
    Ok(surface::foreground_cwd(pane)?.as_deref() == Some(target))
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
            let args = parse(&["spawn", "reviewer", "--msg", text]);

            assert_eq!(args.msg.expect("a prompt was given").to_string(), text, "{text}");
        }
    }

    #[test]
    fn a_flag_after_the_first_prompt_is_still_a_flag() {
        // `allow_hyphen_values` must claim this option's own value and nothing past it.
        let args = parse(&[
            "spawn",
            "reviewer",
            "--msg",
            "--go",
            "--placement",
            "tab",
            "--",
            "--resume",
        ]);

        assert_eq!(args.msg.as_ref().expect("a prompt was given").to_string(), "--go");
        assert_eq!(args.placement, Placement::Tab);
        assert_eq!(args.agent_args, ["--resume"]);
    }

    #[test]
    fn a_first_prompt_addresses_its_reply_at_the_sender_by_default() {
        assert_eq!(
            parse(&["spawn", "worker", "--msg", "audit the CLI"]).reply(),
            Reply::ToSender
        );
    }

    #[test]
    fn a_fan_out_can_route_every_workers_report_at_one_collector() {
        assert_eq!(
            parse(&["spawn", "worker", "--msg", "audit the CLI", "--reply-to", "collector"]).reply(),
            Reply::To("collector".to_owned())
        );
    }

    #[test]
    fn a_first_prompt_can_close_the_loop_like_any_other() {
        assert_eq!(
            parse(&["spawn", "worker", "--msg", "fyi", "--no-reply"]).reply(),
            Reply::None
        );
    }

    #[test]
    fn spawn_refuses_the_two_reply_flags_together_the_same_way_msg_does() {
        assert!(
            Harness::try_parse_from([
                "spawn",
                "worker",
                "--msg",
                "go",
                "--no-reply",
                "--reply-to",
                "collector"
            ])
            .is_err()
        );
    }

    #[test]
    fn a_name_herdr_would_refuse_fails_at_parse_time_before_any_surface_exists() {
        assert!(Harness::try_parse_from(["spawn", "Reviewer"]).is_err());
        assert!(Harness::try_parse_from(["spawn", "1st"]).is_err());
    }

    #[test]
    fn the_operator_name_is_refused_so_a_human_sender_cannot_be_impersonated() {
        let error = SpawnError::ReservedName;

        assert_eq!(error.exit_status(), ExitStatus::Usage);
        assert_eq!(
            error.to_string(),
            "'operator' is reserved: it marks a human sender in delivered mail"
        );
    }

    /// The reservation is this crate's rule, not herdr's, and the type that mirrors herdr stays clean.
    ///
    /// `AgentName` restates herdr's rule and its rejection names herdr as the authority. A reservation
    /// herdr does not have would make it refuse a name herdr accepts and blame herdr for it — the exact
    /// disagreement the "validate only what herdr won't" rule exists to prevent.
    #[test]
    fn the_reservation_lives_in_spawn_rather_than_in_the_name_type() {
        assert!("operator".parse::<crate::core::AgentName>().is_ok());
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
    fn the_agent_is_named_with_agent_rather_than_preset() {
        assert_eq!(
            parse(&["spawn", "reviewer", "--agent", "opus"]).agent.as_deref(),
            Some("opus")
        );
        assert_eq!(
            parse(&["spawn", "reviewer"]).agent,
            None,
            "the config's default applies"
        );
    }

    /// The old flag is declared only so its rename can be reported.
    ///
    /// clap's suggestion machinery will not reach `--agent` from `preset`, and a bare *unrecognized
    /// argument* is a poor way to learn about a rename.
    #[test]
    fn the_old_flag_is_answered_with_the_one_that_replaced_it() {
        let args = parse(&["spawn", "reviewer", "--preset", "opus"]);

        let error = args.check_renamed_flags().unwrap_err();

        assert_eq!(error.exit_status(), ExitStatus::Usage);
        assert_eq!(error.to_string(), "--preset is now --agent");
    }

    #[test]
    fn a_spawn_that_uses_neither_flag_passes_the_rename_check() {
        assert!(
            parse(&["spawn", "reviewer", "--agent", "opus"])
                .check_renamed_flags()
                .is_ok()
        );
    }

    /// The brief comes first, so the caller's instruction reads as an instruction about it.
    #[test]
    fn a_brief_and_a_prompt_are_delivered_as_one_message_with_a_blank_line_between() {
        let brief = "You review Rust.".parse::<NonEmptyText>().unwrap();
        let text = "start with the auth module".parse::<NonEmptyText>().unwrap();

        assert_eq!(
            first_prompt(Some(&brief), Some(&text)).unwrap().to_string(),
            "You review Rust.\n\nstart with the auth module"
        );
    }

    #[test]
    fn either_half_alone_is_delivered_unchanged_and_neither_delivers_nothing() {
        let brief = "You review Rust.".parse::<NonEmptyText>().unwrap();
        let text = "audit the CLI".parse::<NonEmptyText>().unwrap();

        assert_eq!(
            first_prompt(Some(&brief), None).unwrap().to_string(),
            "You review Rust."
        );
        assert_eq!(first_prompt(None, Some(&text)).unwrap().to_string(), "audit the CLI");
        assert!(first_prompt(None, None).is_none());
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
    fn a_caller_inside_a_project_is_placed_at_the_same_path_in_the_new_checkout() {
        assert_eq!(relative_to("/work/repo", "/work/repo/src").as_deref(), Some("src"));
        assert_eq!(
            relative_to("/work/repo", "/work/repo/src/api/handlers").as_deref(),
            Some("src/api/handlers")
        );
    }

    #[test]
    fn a_caller_at_the_repository_root_has_nothing_to_place_and_skips_the_step() {
        // Not an empty string that a later `cd` would run as a no-op: the pane already opens at the
        // checkout root, so there is nothing for the placement step to do.
        assert_eq!(relative_to("/work/repo", "/work/repo"), None);
    }

    #[test]
    fn a_trailing_separator_on_either_side_is_not_part_of_the_relative_path() {
        assert_eq!(relative_to("/work/repo/", "/work/repo/src").as_deref(), Some("src"));
        assert_eq!(relative_to("/work/repo", "/work/repo/src/").as_deref(), Some("src"));
        assert_eq!(relative_to("/work/repo/", "/work/repo/"), None);
    }

    /// The reason this compares path components rather than string prefixes.
    ///
    /// `str::strip_prefix` would answer `"y/src"` here and send the agent somewhere that does not exist.
    /// The second case is the caller already inside a linked worktree, whose `repo_root` is the main
    /// checkout it is not under — herdr refuses to cut a worktree from there, and answering `None`
    /// keeps this from computing nonsense in the moment before that refusal arrives.
    #[test]
    fn a_directory_that_only_looks_like_a_prefix_of_the_root_is_not_inside_it() {
        assert_eq!(relative_to("/work/repo", "/work/repository/src"), None);
        assert_eq!(relative_to("/work/repo", "/work/trees/repo-8e01/src"), None);
    }

    /// `--cwd` is where the agent works, under every placement.
    ///
    /// The old help said it meant the source checkout "for a worktree" — one flag with two meanings,
    /// distinguished by another flag's value, which was the defect under the subdirectory bug rather
    /// than a wording slip. Asserted as an absence because the replacement wording is prose that should
    /// be free to improve.
    #[test]
    fn the_cwd_help_no_longer_claims_a_second_meaning_for_one_placement() {
        let mut command = <Harness as clap::CommandFactory>::command();
        let rendered = command.render_help().to_string();
        // Collapsed to single spaces first: clap wraps help text to the terminal width, so a phrase
        // asserted against the raw rendering can be split across two lines and pass vacuously.
        let flattened = rendered.split_whitespace().collect::<Vec<&str>>().join(" ");

        assert!(flattened.contains("--cwd"), "the flag is still there");
        assert!(
            !flattened.contains("the checkout it is cut from"),
            "--cwd means where the agent works, for every placement: {flattened}"
        );
    }

    /// Three of the four placements answer without asking herdr anything.
    ///
    /// That is what makes this testable at all — no automated test in this crate invokes herdr, so a
    /// version that called out for every placement would have nothing to assert here. The worktree arm
    /// is the one this cannot cover, and it is covered by the live rehearsal instead.
    #[test]
    fn only_a_worktree_spawn_asks_where_the_caller_sits_in_its_repository() {
        for placement in ["pane", "tab", "workspace"] {
            let args = parse(&["spawn", "reviewer", "--placement", placement]);

            assert_eq!(args.subdirectory("/work/repo/src").unwrap(), None, "{placement}");
        }
    }

    #[test]
    fn a_subdirectory_the_fresh_checkout_does_not_have_falls_back_to_the_checkout_root() {
        // Absent because it is gitignored, untracked, or not in the ref `--base` named. `None` *is* the
        // checkout root: it skips the placement step, and the pane herdr made already opens there.
        let checkout = tempfile::tempdir().unwrap();

        assert_eq!(target_in(&checkout.path().to_string_lossy(), "src/api"), None);
    }

    #[test]
    fn a_subdirectory_the_fresh_checkout_does_have_is_where_the_pane_is_sent() {
        let checkout = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(checkout.path().join("src/api")).unwrap();

        assert_eq!(
            target_in(&checkout.path().to_string_lossy(), "src/api"),
            Some(checkout.path().join("src/api"))
        );

        // A file of that name is not a directory to `cd` into either.
        std::fs::write(checkout.path().join("README.md"), "x").unwrap();
        assert_eq!(target_in(&checkout.path().to_string_lossy(), "README.md"), None);
    }

    /// Both warnings name the directory that was missed, and neither carries anything else.
    ///
    /// The checkout path is deliberately absent: the result line already reports it, and a warning that
    /// repeats it is noise on the one line a caller reads. What a caller cannot get anywhere else is
    /// which directory it asked for and did not get.
    #[test]
    fn both_warnings_name_the_directory_that_was_missed_and_carry_nothing_else() {
        assert_eq!(
            Unplaced::Absent.warning("src/api"),
            "src/api is not in the new checkout, so the agent starts at its root"
        );
        assert_eq!(
            Unplaced::NeverArrived.warning("src/api"),
            "the new pane never reached src/api, so the agent starts at the checkout root"
        );

        for warning in [Unplaced::Absent, Unplaced::NeverArrived] {
            let rendered = warning.warning("src/api");
            assert!(rendered.contains("src/api"), "{rendered}");
            assert!(
                !rendered.contains("/work/trees"),
                "the checkout path is the result line's to report, not a warning's"
            );
        }
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
