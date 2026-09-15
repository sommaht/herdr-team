//! `spawn` — create a surface and start a configured agent in it.

use std::fmt::Display;
use std::path::{Path, PathBuf};

use clap::Args;
use clap_stdin::MaybeStdin;
use serde::Serialize;
use thiserror::Error;

use crate::cmd::msg::envelope::{OPERATOR, Reply};
use crate::cmd::msg::{Delivery, Proof, deliver};
use crate::cmd::{AsExitStatus, Cmd, ExitStatus};
use crate::config::{Config, ConfigError, Tuning};
use crate::core::{AgentName, Backoff, NonEmptyText, PaneId, Sink};
use crate::harness;
use crate::herdr::agent::{self, AgentRecord};
use crate::herdr::surface::{self, Checkout, Focus, Placement};
use crate::herdr::{HerdrError, HerdrRef, PANE_VARIABLE};

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// How long `agent start` is retried while the new pane's shell is still starting, in milliseconds.
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
    herdr-team spawn scratch --kind codex -- --no-alt-screen\n  \
    herdr-team spawn big --placement workspace --agent fable -- --resume\n  \
    herdr-team spawn fixer --placement worktree --branch worktree/flake-fix\n  \
    herdr-team spawn quick --agent opus --model sonnet --effort low\n\
    \n\
    A scalar an agent declares is overridden; a vector is extended. So `--model` and `--effort` \
    replace what the agent sets, while `-- <agent args>` follows the agent's own flags rather than \
    replacing them, and the agent CLI's last-flag-wins rule settles any conflict.")]
pub struct SpawnArgs {
    /// The agent's name; must satisfy herdr's rule, which is checked before anything is created.
    name: AgentName,

    /// Where the agent's pane comes from: a split of the calling pane, which needs `HERDR_PANE_ID`,
    /// or the root pane of a new tab, workspace, or Git worktree cut from `--cwd`.
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

    /// Start this agent kind directly, reading no config at all.
    ///
    /// The zero-config route: everything a config would have supplied is then said on the line, with
    /// `-- <agent args>` carrying the flags. Passed to herdr untouched, so a kind this build has
    /// never heard of works the day herdr learns it.
    #[arg(long, value_name = "KIND", conflicts_with_all = ["agent", "config"])]
    kind: Option<String>,

    /// The model to start with, replacing whatever the agent or `--kind` would otherwise run.
    ///
    /// Spelled by the harness this build resolves for the kind, so a kind it has no harness for is
    /// refused rather than started without it.
    #[arg(long, value_name = "MODEL")]
    model: Option<String>,

    /// The reasoning effort to start with, likewise.
    #[arg(long, value_name = "EFFORT")]
    effort: Option<String>,

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
    // `--` already belongs to `agent_args`, so this option must take hyphen-leading text on its own;
    // `--prompt` is the pre-rename spelling, kept working and kept out of the help.
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
    /// Asked of herdr rather than read from `HERDR_WORKSPACE_ID`, which goes stale when a pane moves
    /// workspaces. `None` when there is no calling pane; herdr then uses whichever workspace the UI
    /// has focused, which the warning names.
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
    /// `None` for the non-worktree placements and for a caller already at the repository root.
    fn subdirectory(&self, cwd: &str) -> Result<Option<String>, SpawnError> {
        if self.placement != Placement::Worktree {
            return Ok(None);
        }
        Ok(relative_to(&surface::repo_root(cwd)?, cwd))
    }

    /// Where the first prompt says a reply should go.
    fn reply(&self) -> Reply {
        Reply::from_flags(self.reply_to.as_deref(), self.no_reply)
    }

    /// Whether the new surface takes the user's focus.
    fn focus(&self) -> Focus {
        if self.focus { Focus::Take } else { Focus::Leave }
    }

    /// Refuses `--branch` or `--base` under a placement with no worktree to apply them to.
    ///
    /// A pre-check because clap's `conflicts_with` takes an argument id, not a value predicate.
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

    /// Refuses a reply flag on a spawn with no message for it to shape.
    ///
    /// A pre-check because clap's `requires` cannot condition on another argument's absence.
    fn check_reply_flags(&self) -> Result<(), SpawnError> {
        if self.msg.is_some() {
            return Ok(());
        }
        for (flag, given) in [("--no-reply", self.no_reply), ("--reply-to", self.reply_to.is_some())] {
            if given {
                return Err(SpawnError::ReplyFlagWithoutMessage { flag });
            }
        }
        Ok(())
    }

    /// Refuses a flag this build renamed, naming what replaced it.
    ///
    /// Not left to clap, whose suggestion machinery scores `--preset` too far from `--agent` to offer it.
    fn check_renamed_flags(&self) -> Result<(), SpawnError> {
        if self.preset.is_some() {
            return Err(SpawnError::RenamedFlag { old: "--preset", new: "--agent" });
        }
        Ok(())
    }

    /// The model and effort this spawn overrides, as the two flags gave them.
    fn tuning(&self) -> Tuning<'_> {
        Tuning {
            model: self.model.as_deref(),
            effort: self.effort.as_deref(),
        }
    }

    /// Refuses a tuning flag under a kind this build has no harness to spell it for.
    ///
    /// A pre-check rather than a silent drop: an agent asked for a model must not start on another.
    fn check_tunable(&self, kind: &str) -> Result<(), SpawnError> {
        let flags = match (self.model.is_some(), self.effort.is_some()) {
            (true, true) => "--model and --effort",
            (true, false) => "--model",
            (false, true) => "--effort",
            (false, false) => return Ok(()),
        };
        if harness::by_kind(kind).is_none() {
            return Err(SpawnError::Untunable { kind: kind.to_owned(), flags });
        }
        Ok(())
    }

    /// What to start: the kind `--kind` named, or the agent the config resolves.
    ///
    /// `--kind` reads no config at all, so a missing or malformed file cannot break that route.
    fn configured(&self, cwd: &Path, sink: &Sink) -> Result<Configured, SpawnError> {
        if let Some(kind) = &self.kind {
            self.check_tunable(kind)?;
            let mut args = harness::by_kind(kind)
                .map(|harness| harness.tuning(self.model.as_deref(), self.effort.as_deref()))
                .unwrap_or_default();
            args.extend(self.agent_args.iter().cloned());
            return Ok(Configured {
                kind: kind.clone(),
                args,
                // No config, so no agent, so nothing that could carry a brief.
                brief: None,
            });
        }

        let config = Config::load(self.config.as_deref(), Some(cwd), sink)?;
        let agent = config.resolve(self.agent.as_deref())?;
        self.check_tunable(agent.kind())?;
        let mut args = agent.agent_args(self.tuning());
        args.extend(self.agent_args.iter().cloned());
        Ok(Configured {
            kind: agent.kind().to_owned(),
            args,
            brief: agent.brief()?,
        })
    }

    /// Makes the surface this placement calls for, and reports the checkout when it made one.
    fn create_surface(
        &self,
        anchor: Option<PaneId>,
        cwd: &str,
        sink: &Sink,
    ) -> Result<(PaneId, Option<Checkout>), SpawnError> {
        // The worktree brings its own workspace, so unlike a tab none is pinned for it.
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

    /// Pre-checks, config, surface, agent, first prompt — in that order.
    fn execute(self, sink: &Sink) -> Result<Self::Ok, Self::Err> {
        // `AgentName` derefs to `str`, so this compares the name itself rather than the newtype.
        if &*self.name == OPERATOR {
            return Err(SpawnError::ReservedName);
        }
        self.check_renamed_flags()?;
        self.check_worktree_flags()?;
        self.check_reply_flags()?;
        let anchor = match self.placement {
            Placement::Pane => Some(anchor(std::env::var(PANE_VARIABLE).ok().as_deref())?),
            Placement::Tab | Placement::Workspace | Placement::Worktree => None,
        };

        // Resolved before the config: the config that applies belongs to the tree the agent works in.
        let cwd = match &self.cwd {
            Some(path) => path.clone(),
            None => std::env::current_dir().map_err(SpawnError::NoWorkingDirectory)?,
        };

        let configured = self.configured(cwd.as_path(), sink)?;

        let cwd = cwd.to_string_lossy().into_owned();

        let subdirectory = self.subdirectory(&cwd)?;

        // Everything above is read-only, so nothing exists yet if any of it failed.
        let (pane, worktree) = self.create_surface(anchor, &cwd, sink)?;

        // From here on a failure leaves the pane open and names it.
        self.start_and_prompt(&pane, &configured, worktree, subdirectory.as_deref(), sink)
            .map_err(|error| error.note_open_pane(&pane))
    }
}

/// What this spawn settled on for the agent it starts.
#[derive(Debug)]
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
    /// Everything that happens after the surface exists — the region `note_open_pane` wraps.
    fn start_and_prompt(
        &self,
        pane: &PaneId,
        configured: &Configured,
        worktree: Option<Checkout>,
        subdirectory: Option<&str>,
        sink: &Sink,
    ) -> Result<Spawned, SpawnError> {
        // Before the agent: `agent start` inherits the shell's directory at launch.
        if let (Some(checkout), Some(relative)) = (&worktree, subdirectory) {
            self.open_subdirectory(pane, &checkout.path, relative, sink)?;
        }

        let started = self.start_when_settled(pane, &configured.kind, &configured.args)?;

        // One submission whatever it holds: an agent handed two answers the first before hearing the second.
        let Some(delivery) = first_delivery(configured.brief.clone(), self.msg.as_deref().cloned()) else {
            return Ok(Spawned {
                placement: self.placement,
                delivered: None,
                worktree,
                agent: started,
            });
        };

        // No composer guard here: no human has touched the pane this just created.
        //
        // An agent that came up working leaves herdr no state change to match, so its pane is read
        // for the message instead.
        let proof = Proof::for_delivery(started.status(), DEFAULT_SETTLE_MS);
        // herdr's own detection, not `configured.kind`: the margin describes the pane herdr identified.
        let margin = harness::delivery_margin(started.kind());
        let submission = deliver(pane, &delivery, &self.reply(), &proof, margin, sink)?;

        // Warned out loud: only the `--json` reader sees the `delivered` field.
        if !submission.proven {
            sink.warn(&format!(
                "{} was already working when its first prompt was sent, so delivery is unproven",
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
    /// The retry is silent: a shell-init hook like `direnv` makes it fire on almost every launch.
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
    /// Never a refusal: both ways of missing the subdirectory warn and return `Ok`.
    fn open_subdirectory(&self, pane: &PaneId, checkout: &str, relative: &str, sink: &Sink) -> Result<(), SpawnError> {
        let Some(target) = target_in(checkout, relative) else {
            sink.warn(&Unplaced::Absent.warning(relative));
            return Ok(());
        };
        let target = target.to_string_lossy().into_owned();

        // `pane run` types into the shell whether or not it is at its prompt, so text sent too early
        // is lost rather than queued — hence re-sent each round instead of polled.
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
/// Neither is a failure: by the time either is knowable the checkout exists, so both warn instead.
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
    /// The branch is usually herdr's to generate, so this response is the one cheap moment it is
    /// knowable.
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
    /// A reply flag on a spawn with no message for it to shape.
    #[error("{flag} needs --msg; a spawn delivering only the agent's brief sends no envelope")]
    ReplyFlagWithoutMessage {
        /// The flag that has nothing to apply to.
        flag: &'static str,
    },
    /// A tuning flag under a kind this build has no harness to spell it for.
    #[error("{flags} cannot be expressed for kind '{kind}'; this build expresses them for {}", harness::kinds().join(", "))]
    Untunable {
        /// The kind that would have been started.
        kind: String,
        /// Which flags were given — the spellings only, never a model or an effort.
        flags: &'static str,
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
    Config(ConfigError),
    /// There is no config to resolve an agent against.
    ///
    /// Distinct from [`ConfigError::Missing`] only in what it recommends: `--kind` needs no config.
    #[error("no config file to resolve an agent from; pass --kind <KIND> to start one directly, \
             or write a config at {}", path.display())]
    NoConfig {
        /// Where a config was looked for.
        path: PathBuf,
    },
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
    fn note_open_pane(self, pane: &PaneId) -> Self {
        Self::AfterSurface {
            pane: pane.clone(),
            error: Box::new(self),
        }
    }
}

/// Hand-written rather than `#[from]`, for the one arm the derive cannot express.
///
/// A conversion rather than a call-site helper, so no `?` on a [`ConfigError`] can bypass the mapping.
impl From<ConfigError> for SpawnError {
    fn from(error: ConfigError) -> Self {
        match error {
            ConfigError::Missing { path } => Self::NoConfig { path },
            other => Self::Config(other),
        }
    }
}

impl AsExitStatus for SpawnError {
    fn exit_status(&self) -> ExitStatus {
        match self {
            Self::MissingAnchor
            | Self::WorktreeOnlyFlag { .. }
            | Self::ReplyFlagWithoutMessage { .. }
            | Self::RenamedFlag { .. }
            | Self::Untunable { .. }
            | Self::ReservedName => ExitStatus::Usage,
            Self::Config(error) => error.exit_status_hint(),
            Self::NoConfig { .. } => ExitStatus::NotFound,
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
            | Self::ReplyFlagWithoutMessage { .. }
            | Self::RenamedFlag { .. }
            | Self::Untunable { .. }
            | Self::ReservedName
            | Self::Config(_)
            | Self::NoConfig { .. }
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
/// Never herdr's `--current`, which resolves server-side to whichever pane is *focused*.
fn anchor(variable: Option<&str>) -> Result<PaneId, SpawnError> {
    variable
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PaneId::from)
        .ok_or(SpawnError::MissingAnchor)
}

/// What this spawn delivers once the agent is up, if anything.
///
/// Only `--msg` gets an envelope: a brief comes from a file and has no author to name as sender.
fn first_delivery(brief: Option<NonEmptyText>, msg: Option<NonEmptyText>) -> Option<Delivery> {
    match (brief, msg) {
        (brief, Some(body)) => Some(Delivery::Mail { brief, body }),
        (Some(brief), None) => Some(Delivery::Brief(brief)),
        (None, None) => None,
    }
}

/// Where `cwd` sits inside `repo_root`, or `None` when there is nothing to place.
///
/// `None` for a caller already at the root and for a `cwd` outside `repo_root` entirely.
fn relative_to(repo_root: &str, cwd: &str) -> Option<String> {
    let relative = Path::new(cwd).strip_prefix(repo_root).ok()?;
    if relative.as_os_str().is_empty() {
        None
    } else {
        Some(relative.to_string_lossy().into_owned())
    }
}

/// The directory in a fresh checkout to open at, or `None` when the checkout does not have it.
fn target_in(checkout: &str, relative: &str) -> Option<PathBuf> {
    let target = Path::new(checkout).join(relative);
    target.is_dir().then_some(target)
}

/// Sends the directory change, then asks the pane where its shell actually ended up.
///
/// `pane run` succeeds whether or not the text reached a live prompt; only the cwd read can tell.
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

    /// The temp directory is a `cwd` the repository walk could look in if it ran; the point is it does not.
    #[test]
    fn a_kind_starts_an_agent_with_no_config_read_at_all() {
        let elsewhere = tempfile::tempdir().unwrap();
        let args = parse(&["spawn", "scratch", "--kind", "codex", "--", "--no-alt-screen"]);

        let configured = args
            .configured(elsewhere.path(), &Sink::new(crate::core::OutputMode::Human))
            .expect("a kind needs nothing else");

        assert_eq!(configured.kind, "codex");
        assert_eq!(
            configured.args,
            ["--no-alt-screen"],
            "the caller's args, and only those"
        );
        assert!(configured.brief.is_none(), "no agent, so nothing that carries a brief");
    }

    #[test]
    fn a_kind_this_build_has_no_harness_for_is_passed_through_rather_than_refused() {
        let elsewhere = tempfile::tempdir().unwrap();
        let args = parse(&["spawn", "scratch", "--kind", "some-agent-shipped-next-year"]);

        let configured = args
            .configured(elsewhere.path(), &Sink::new(crate::core::OutputMode::Human))
            .expect("herdr is the authority on kinds");

        assert_eq!(configured.kind, "some-agent-shipped-next-year");
    }

    #[test]
    fn a_kind_is_tuned_by_the_flags_the_harness_for_it_spells() {
        let elsewhere = tempfile::tempdir().unwrap();
        let args = parse(&[
            "spawn",
            "scratch",
            "--kind",
            "codex",
            "--model",
            "gpt-5",
            "--effort",
            "high",
            "--",
            "--no-alt-screen",
        ]);

        let configured = args
            .configured(elsewhere.path(), &Sink::new(crate::core::OutputMode::Human))
            .expect("codex is a kind this build drives");

        assert_eq!(
            configured.args,
            [
                "--model",
                "gpt-5",
                "-c",
                "model_reasoning_effort=high",
                "--no-alt-screen"
            ],
            "the caller's own args stay last, so they still win"
        );
    }

    #[test]
    fn a_tuning_flag_under_a_kind_this_build_cannot_drive_is_refused_without_its_value() {
        let elsewhere = tempfile::tempdir().unwrap();
        let args = parse(&[
            "spawn",
            "scratch",
            "--kind",
            "some-agent-shipped-next-year",
            "--model",
            "gpt-9",
        ]);

        let error = args
            .configured(elsewhere.path(), &Sink::new(crate::core::OutputMode::Human))
            .unwrap_err();

        assert_eq!(error.exit_status(), ExitStatus::Usage);
        assert!(error.to_string().contains("--model"), "{error}");
        assert!(!error.to_string().contains("gpt-9"), "{error}");
    }

    #[test]
    fn a_tuning_refusal_names_every_flag_that_was_given_and_nothing_is_refused_when_none_was() {
        let both = parse(&["spawn", "w", "--kind", "elsewhen", "--model", "m", "--effort", "e"]);
        assert!(
            both.check_tunable("elsewhen")
                .unwrap_err()
                .to_string()
                .contains("--model and --effort"),
            "{}",
            both.check_tunable("elsewhen").unwrap_err()
        );

        let effort_only = parse(&["spawn", "w", "--kind", "elsewhen", "--effort", "e"]);
        let error = effort_only.check_tunable("elsewhen").unwrap_err().to_string();
        assert!(error.contains("--effort") && !error.contains("--model"), "{error}");

        assert!(
            parse(&["spawn", "w", "--kind", "elsewhen"])
                .check_tunable("elsewhen")
                .is_ok()
        );
    }

    /// The two flags are the same type, so which one lands where is worth pinning.
    #[test]
    fn each_tuning_flag_becomes_the_field_of_the_override_it_is_named_for() {
        let args = parse(&["spawn", "w", "--model", "sonnet", "--effort", "low"]);
        let tuning = args.tuning();

        assert_eq!(tuning.model, Some("sonnet"));
        assert_eq!(tuning.effort, Some("low"));
        assert_eq!(parse(&["spawn", "w"]).tuning().model, None);
    }

    #[test]
    fn a_kind_is_refused_beside_the_two_flags_it_would_otherwise_ignore() {
        assert!(Harness::try_parse_from(["spawn", "w", "--kind", "codex", "--agent", "sol"]).is_err());
        assert!(Harness::try_parse_from(["spawn", "w", "--kind", "codex", "--config", "./c.toml"]).is_err());
        // Neither flag is harmed on its own.
        assert_eq!(parse(&["spawn", "w", "--kind", "codex"]).kind.as_deref(), Some("codex"));
        assert_eq!(parse(&["spawn", "w", "--agent", "sol"]).agent.as_deref(), Some("sol"));
    }

    #[test]
    fn no_config_is_answered_with_the_path_to_write_and_the_flag_that_needs_none() {
        let error = SpawnError::from(ConfigError::Missing {
            path: PathBuf::from("/home/someone/.config/herdr-team/config.toml"),
        });

        assert_eq!(error.exit_status(), ExitStatus::NotFound);
        assert!(error.to_string().contains("--kind"), "{error}");
        assert!(error.to_string().contains("/home/someone/.config"), "{error}");
    }

    #[test]
    fn a_config_that_exists_and_is_broken_is_reported_as_the_config_failure_it_is() {
        let error = SpawnError::from(ConfigError::NoDefault {
            paths: "/named/config.toml".to_owned(),
        });

        assert!(matches!(error, SpawnError::Config(_)), "got {error:?}");
        assert!(!error.to_string().contains("--kind"), "{error}");
    }

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
        // Also exercises the `default_value` string: a typo in it fails here rather than reaching a caller.
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
        assert_eq!(parse(&["spawn", "reviewer"]).focus(), Focus::Leave);
        assert_eq!(parse(&["spawn", "reviewer", "--focus"]).focus(), Focus::Take);
    }

    #[test]
    fn a_first_prompt_that_opens_with_a_dash_is_prompt_text_rather_than_a_flag() {
        for text in ["--force the issue", "-e", "--focus"] {
            let args = parse(&["spawn", "reviewer", "--msg", text]);

            assert_eq!(args.msg.expect("a prompt was given").to_string(), text, "{text}");
        }
    }

    #[test]
    fn a_flag_after_the_first_prompt_is_still_a_flag() {
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

    /// The reservation is this crate's rule, not herdr's, so `AgentName` must keep accepting the name.
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
        // Worktree included: it creates its own workspace, so it too needs no calling pane.
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

    #[test]
    fn a_message_is_mail_and_the_brief_rides_outside_the_envelope() {
        let brief = "You review Rust.".parse::<NonEmptyText>().unwrap();
        let body = "start with the auth module".parse::<NonEmptyText>().unwrap();

        let delivery = first_delivery(Some(brief), Some(body)).expect("there is a message");

        match delivery {
            Delivery::Mail { brief, body } => {
                assert_eq!(brief.expect("the agent has one").to_string(), "You review Rust.");
                assert_eq!(body.to_string(), "start with the auth module");
            }
            Delivery::Brief(_) => panic!("a spawn with --msg delivers mail"),
        }
    }

    #[test]
    fn a_brief_with_no_message_behind_it_is_delivered_unwrapped() {
        let brief = "You review Rust.".parse::<NonEmptyText>().unwrap();

        match first_delivery(Some(brief), None).expect("the brief is delivered") {
            Delivery::Brief(text) => assert_eq!(text.to_string(), "You review Rust."),
            Delivery::Mail { .. } => panic!("a brief has no sender to put on an envelope"),
        }
    }

    #[test]
    fn a_message_with_no_brief_is_mail_with_nothing_ahead_of_it_and_neither_is_nothing() {
        let body = "audit the CLI".parse::<NonEmptyText>().unwrap();

        match first_delivery(None, Some(body)).expect("there is a message") {
            Delivery::Mail { brief, body } => {
                assert!(brief.is_none());
                assert_eq!(body.to_string(), "audit the CLI");
            }
            Delivery::Brief(_) => panic!("a spawn with --msg delivers mail"),
        }

        assert!(first_delivery(None, None).is_none(), "nothing to deliver");
    }

    #[test]
    fn a_reply_flag_is_refused_on_a_spawn_that_sends_no_envelope() {
        for flag in [vec!["--no-reply"], vec!["--reply-to", "collector"]] {
            let mut argv = vec!["spawn", "worker"];
            argv.extend(flag.iter().copied());

            let error = parse(&argv).check_reply_flags().expect_err("nothing to shape");

            assert_eq!(error.exit_status(), ExitStatus::Usage);
            assert!(error.to_string().contains("--msg"), "{error}");
        }

        // A message is all either flag needs.
        assert!(
            parse(&["spawn", "worker", "--msg", "go", "--no-reply"])
                .check_reply_flags()
                .is_ok()
        );
        assert!(
            parse(&["spawn", "worker", "--msg", "go", "--reply-to", "collector"])
                .check_reply_flags()
                .is_ok()
        );
        // And a spawn that asks for neither is not refused for delivering nothing.
        assert!(parse(&["spawn", "worker"]).check_reply_flags().is_ok());
    }

    #[test]
    fn branch_and_base_are_refused_under_a_placement_that_makes_no_worktree() {
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

        // Neither is required: herdr generates a branch and bases on HEAD.
        let neither = parse(&["spawn", "reviewer", "--placement", "worktree"]);
        assert!(neither.check_worktree_flags().is_ok());
        assert_eq!(neither.branch, None);
        assert_eq!(neither.base, None);
    }

    #[test]
    fn an_anchor_is_read_from_the_environment_rather_than_resolved_by_herdr() {
        assert_eq!(anchor(Some("w4:p1")).unwrap(), PaneId::from("w4:p1"));
        assert!(anchor(Some("   ")).is_err(), "a blank variable is as good as unset");
    }

    /// The last-attempt-after-the-budget path is not covered here: proving it needs a live herdr.
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
        assert_eq!(relative_to("/work/repo", "/work/repo"), None);
    }

    #[test]
    fn a_trailing_separator_on_either_side_is_not_part_of_the_relative_path() {
        assert_eq!(relative_to("/work/repo/", "/work/repo/src").as_deref(), Some("src"));
        assert_eq!(relative_to("/work/repo", "/work/repo/src/").as_deref(), Some("src"));
        assert_eq!(relative_to("/work/repo/", "/work/repo/"), None);
    }

    /// The second case is a caller inside a linked worktree, whose `repo_root` is the main checkout.
    #[test]
    fn a_directory_that_only_looks_like_a_prefix_of_the_root_is_not_inside_it() {
        assert_eq!(relative_to("/work/repo", "/work/repository/src"), None);
        assert_eq!(relative_to("/work/repo", "/work/trees/repo-8e01/src"), None);
    }

    /// Asserted as an absence, so the replacement wording stays free to improve.
    #[test]
    fn the_cwd_help_no_longer_claims_a_second_meaning_for_one_placement() {
        let mut command = <Harness as clap::CommandFactory>::command();
        let rendered = command.render_help().to_string();
        // Collapsed to single spaces: clap wraps help text, so a split phrase could pass vacuously.
        let flattened = rendered.split_whitespace().collect::<Vec<&str>>().join(" ");

        assert!(flattened.contains("--cwd"), "the flag is still there");
        assert!(
            !flattened.contains("the checkout it is cut from"),
            "--cwd means where the agent works, for every placement: {flattened}"
        );
    }

    /// The worktree arm needs a live herdr, so the rehearsal covers it instead.
    #[test]
    fn only_a_worktree_spawn_asks_where_the_caller_sits_in_its_repository() {
        for placement in ["pane", "tab", "workspace"] {
            let args = parse(&["spawn", "reviewer", "--placement", placement]);

            assert_eq!(args.subdirectory("/work/repo/src").unwrap(), None, "{placement}");
        }
    }

    #[test]
    fn a_subdirectory_the_fresh_checkout_does_not_have_falls_back_to_the_checkout_root() {
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
