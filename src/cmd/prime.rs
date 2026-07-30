//! `prime` — print an agent-facing brief on driving this CLI.
//!
//! Written to be run from a session-start hook, which sets every constraint here: it makes no herdr
//! call, because nothing guarantees a running server at that point, and it cannot fail, because a
//! hook that fails is worse than one that says little.
//!
//! The brief is hand-written rather than composed from clap's help. Five subcommands render past two
//! hundred lines, and context an agent carries all session has a cost that a reference dump cannot
//! justify when `--help` is one command away. What an agent cannot look up on demand is that the tool
//! exists, when to reach for it, and which failures are worth retrying — so that is what this says.
//!
//! Two parts are generated anyway, because generating them is what stops them drifting: the command
//! index comes from clap's own tree, and the preset table from the same listing `presets` prints.

use std::fmt::Display;
use std::path::PathBuf;

use clap::{Args, CommandFactory};
use serde::Serialize;

use crate::cmd::Cmd;
use crate::cmd::presets::PresetList;
use crate::config::Presets;
use crate::core::Sink;

// =====================================================================================================================
// The Brief
// =====================================================================================================================

/// What this tool is for, and when to reach for it rather than herdr.
const INTRO: &str = "\
herdr-agent-tools launches and prompts herdr agents. Reach for it instead of `herdr` when
starting one: herdr starts an agent only in a pane that already exists and is sitting at a
shell prompt, so `spawn` creates the surface and starts the agent as one step.";

/// How to drive it: the things a caller cannot learn from a flag list.
///
/// This and [`INTRO`] share a line budget, enforced by a test. Every line here is context an agent
/// carries for a whole session, and nothing else pushes back on it growing.
const PRACTICE: &str = "\
A target is a unique agent name or a pane id; herdr resolves it. Your own pane, tab, and
workspace ids come from `herdr pane current`.

Where it lands. `spawn` splits the calling pane by default; `--placement tab` or
`--placement workspace` gives the agent a surface of its own, and `--preset` chooses which
agent starts there.

Sequencing work. `prompt` returns as soon as delivery is proven, not when the agent has
finished — pass `--wait-until idle` to wait for a result instead. Long or generated prompt
text goes on stdin with `-`, which beats quoting it into an argument.

Exit codes are a protocol, not just failure. 5 is retryable and worth retrying: a composer
holding unsent text, or a new pane whose shell has not started yet. 3 means what you named
does not exist, and 2 means the arguments were wrong; neither improves on a retry.

Two refusals you will meet. `prompt` refuses a composer holding unsent text, because
submitting yours would submit someone's half-written message along with it. `kill` refuses
an agent that is working or blocked. Both are exit 5, and both take `--force` when you
mean it.";

// =====================================================================================================================
// Prime Args
// =====================================================================================================================

/// Print a brief on driving this CLI, for a session-start hook to feed an agent.
///
/// Makes no herdr call and cannot fail: a config it cannot read costs the preset table and nothing
/// else.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  \
    herdr-agent-tools prime\n  \
    herdr-agent-tools prime --config ./config.toml")]
pub struct PrimeArgs {
    /// Read this preset file instead of the one in the config directory.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

impl Cmd for PrimeArgs {
    type Ok = Brief;
    type Err = std::convert::Infallible;

    fn execute(self, _sink: &Sink) -> Result<Self::Ok, Self::Err> {
        // The one place a `ConfigError` is deliberately dropped rather than reported. A hook fires
        // before anyone has necessarily written a config, and failing there would cost the whole
        // brief to say something the brief itself already says.
        let presets = Presets::load(self.config.as_deref())
            .ok()
            .map(|presets| PresetList::of(&presets));
        Ok(Brief {
            intro: INTRO,
            commands: commands(),
            practice: PRACTICE,
            presets,
        })
    }
}

// =====================================================================================================================
// Output
// =====================================================================================================================

/// The brief, in the order it prints.
#[derive(Debug, Serialize)]
pub struct Brief {
    /// What the tool is for.
    intro: &'static str,
    /// One entry per subcommand, from clap's tree rather than from prose.
    commands: Vec<Summary>,
    /// How to drive it.
    practice: &'static str,
    /// The preset table, absent when no config could be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    presets: Option<PresetList>,
}

/// One subcommand as the index lists it.
#[derive(Debug, Serialize)]
struct Summary {
    /// The subcommand's name, as typed.
    name: String,
    /// clap's own one-line description, which is the `*Args` struct's doc comment.
    about: String,
}

/// Every subcommand clap knows, minus its built-in `help`.
///
/// Read from [`crate::Cli`] rather than restated, so a command added, renamed, or re-described shows
/// up here without anyone remembering to edit prose. `help` is dropped: it answers a human browsing,
/// and this list is what an agent picks a command from.
fn commands() -> Vec<Summary> {
    crate::Cli::command()
        .get_subcommands()
        .filter(|command| command.get_name() != "help")
        .map(|command| Summary {
            name: command.get_name().to_owned(),
            about: command.get_about().map(ToString::to_string).unwrap_or_default(),
        })
        .collect()
}

impl Display for Brief {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{}\n", self.intro)?;

        let widest = self
            .commands
            .iter()
            .map(|command| command.name.len())
            .max()
            .unwrap_or(0);
        for command in &self.commands {
            writeln!(f, "  {:<widest$}  {}", command.name, command.about)?;
        }

        write!(f, "\n{}", self.practice)?;

        match &self.presets {
            Some(presets) => write!(f, "\n\nPresets:\n{presets}"),
            // Said rather than omitted: an agent that knows presets exist and sees none knows not to
            // reach for `--preset`.
            None => write!(f, "\n\nPresets: none configured."),
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
    use crate::cmd::ExitStatus;

    /// The longest the hand-written prose may run.
    ///
    /// Lives here because it exists only as an assertion — a budget rather than a suggestion, and
    /// raising it is then a deliberate edit to a test rather than a number that quietly drifts.
    const LINE_BUDGET: usize = 40;

    #[derive(Debug, Parser)]
    struct Harness {
        #[command(flatten)]
        args: PrimeArgs,
    }

    /// The brief as a reader receives it, over a config path that does not exist.
    ///
    /// Not a fixture: rendering it is the only way to check what the generated halves actually
    /// contribute, and a missing config is the case a hook is most likely to hit.
    fn rendered() -> String {
        Harness::try_parse_from(["prime", "--config", "/nonexistent/config.toml"])
            .expect("parses")
            .args
            .execute(&Sink::new(crate::core::OutputMode::Human))
            .expect("prime cannot fail")
            .to_string()
    }

    #[test]
    fn a_config_that_cannot_be_read_costs_the_table_and_nothing_else() {
        // The hook constraint: firing before anyone wrote a config must still produce the brief.
        let brief = rendered();

        assert!(brief.contains("Presets: none configured."), "got {brief}");
        assert!(brief.contains("herdr-agent-tools launches"), "the prose survived");
    }

    /// Every flag the prose names still exists somewhere in the CLI.
    ///
    /// Catches the drift that actually happens — a flag renamed or removed while the prose keeps
    /// recommending it. It cannot catch a flag attributed to the wrong command; with four commands
    /// that is what reading the prose is for.
    #[test]
    fn every_flag_the_prose_recommends_still_exists() {
        let mut known: Vec<String> = Vec::new();
        for command in crate::Cli::command().get_subcommands() {
            for argument in command.get_arguments() {
                known.extend(argument.get_long().map(|long| format!("--{long}")));
            }
        }

        // The prose backticks its flags, so a token arrives as "`--placement" and has to be trimmed
        // down to the flag before it can be looked up. Everything that is neither alphanumeric nor a
        // dash comes off both ends, which leaves a leading `--` intact.
        let mentioned: Vec<&str> = PRACTICE
            .split_whitespace()
            .map(|word| word.trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '-'))
            .filter(|word| word.starts_with("--"))
            .collect();

        // Without this the test passes vacuously the moment the extraction above stops working.
        assert!(
            !mentioned.is_empty(),
            "the prose names no flags at all, which is suspicious"
        );
        for flag in mentioned {
            assert!(
                known.iter().any(|long| long == flag),
                "{flag} is named but does not exist"
            );
        }
    }

    /// Every exit code the prose teaches is one the contract actually reports.
    ///
    /// Checked in this direction on purpose. Asserting the reverse — that each status *appears* — is
    /// what a first attempt did, and it passes vacuously: the prose names the retryable code twice, so
    /// changing either mention leaves the other to satisfy a `contains`. Asking instead that every
    /// number the prose names be real catches a renumbering on the first mention, and survives a
    /// reword that a phrase-matching test would break on.
    ///
    /// What it does not catch: prose that thins out. Deleting the paragraph that *explains* the codes
    /// still leaves the retryable one named further down, and it passes — asserting otherwise would
    /// mean pinning wording, which is the thing above that makes a test get weakened later.
    #[test]
    fn every_exit_code_the_prose_teaches_is_one_the_contract_reports() {
        let contracted: Vec<String> = [
            ExitStatus::Success,
            ExitStatus::Failure,
            ExitStatus::Usage,
            ExitStatus::NotFound,
            ExitStatus::Conflict,
        ]
        .into_iter()
        .map(|status| u8::from(status).to_string())
        .collect();

        let mentioned: Vec<&str> = PRACTICE
            .split_whitespace()
            .map(|word| word.trim_matches(|character: char| !character.is_ascii_digit()))
            .filter(|word| !word.is_empty())
            .collect();

        assert!(!mentioned.is_empty(), "the prose names no exit codes at all");
        for code in &mentioned {
            assert!(
                contracted.iter().any(|contracted| contracted == code),
                "the prose teaches exit code {code}, which the contract does not report"
            );
        }

        // The one status whose absence would actually hurt: an agent that never learns which code is
        // retryable gives up where it should retry.
        let retryable = u8::from(ExitStatus::Conflict).to_string();
        assert!(
            mentioned.contains(&retryable.as_str()),
            "the prose never names {retryable} as the retryable code"
        );
    }

    #[test]
    fn the_prose_stays_inside_its_line_budget() {
        // Context an agent carries all session, with nothing else pushing back on it growing.
        let lines = INTRO.lines().count() + PRACTICE.lines().count();

        assert!(
            lines <= LINE_BUDGET,
            "the prose is {lines} lines, over the {LINE_BUDGET} budget"
        );
    }

    #[test]
    fn the_command_index_comes_from_clap_rather_than_from_prose() {
        let brief = rendered();

        // Every command clap knows is listed, with the about clap already carries.
        assert!(
            brief.contains("spawn    Create a pane, tab, or workspace"),
            "got {brief}"
        );
        assert!(brief.contains("kill     Close an agent's pane"), "got {brief}");
        // clap's own `help` is not something an agent drives.
        assert!(!brief.contains("Print this message"), "got {brief}");
    }

    #[test]
    fn the_wire_form_carries_the_prose_and_the_generated_halves_separately() {
        let brief = Brief {
            intro: "intro",
            commands: vec![Summary {
                name: "spawn".to_owned(),
                about: "start an agent".to_owned(),
            }],
            practice: "practice",
            presets: None,
        };

        assert_eq!(
            serde_json::to_string(&brief).unwrap(),
            r#"{"intro":"intro","commands":[{"name":"spawn","about":"start an agent"}],"practice":"practice"}"#
        );
    }
}
