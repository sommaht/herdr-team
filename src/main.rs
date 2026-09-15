//! `herdr-team` — launch and message herdr agents from one command.
//!
//! Parses and dispatches, and restates clap's rejection without repeating what the caller typed.

mod cmd;
mod config;
mod core;
mod harness;
mod herdr;

use std::error::Error as _;
use std::process::ExitCode;

use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{CommandFactory, Parser, Subcommand};
use thiserror::Error;

use crate::cmd::{AgentsArgs, AsExitStatus, Cmd, ExitStatus, Failure, KillArgs, MsgArgs, PrimeArgs, SpawnArgs};
use crate::core::{OutputMode, Sink};

// =====================================================================================================================
// Cli
// =====================================================================================================================

#[derive(Debug, Parser)]
#[command(
    name = "herdr-team",
    version,
    about = "Launch and message herdr agents from one command",
    long_about = "Launch and message herdr agents from one command.\n\
        \n\
        herdr starts an agent only in a pane that already exists and is sitting at an interactive \
        shell prompt, so launching one by hand is two steps. `spawn` does both: it creates a pane, \
        tab, or workspace, reads back the new pane's id, and starts a configured agent in it. \
        `msg` delivers text to an agent that already exists, `kill` closes an agent's pane \
        unless it is mid-task, and `agents` lists what the config holds.\n\
        \n\
        Run `herdr-team <command> --help` for details and examples.",
    after_help = "Exit codes:\n  \
        0  success\n  \
        1  general failure\n  \
        2  usage error (bad arguments)\n  \
        3  resource not found\n  \
        5  conflict, retryable (a composer holding unsent text, a pane that is not yet a shell)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Emit machine-readable NDJSON on stdout instead of human-readable text.
    ///
    /// `prime` is the documented exception: its result is a document, so it prints the same text in
    /// either mode. Everything else — results, warnings, and failures — is one tagged object per
    /// line.
    #[arg(long, global = true)]
    json: bool,
}

/// The subcommand set. A doc comment on a variant becomes that command's `--help` summary, so each
/// command's help lives on its `*Args` struct instead.
#[derive(Debug, Subcommand)]
enum Command {
    Spawn(Box<SpawnArgs>),
    // `prompt` is the retired spelling of `msg`; hidden so the help teaches one name.
    #[command(alias = "prompt")]
    Msg(MsgArgs),
    Kill(KillArgs),
    Agents(AgentsArgs),
    Prime(PrimeArgs),
}

// =====================================================================================================================
// Entry Point
// =====================================================================================================================

fn main() -> ExitCode {
    match Cli::try_parse() {
        Ok(cli) => {
            let mode = if cli.json { OutputMode::Json } else { OutputMode::Human };
            let sink = Sink::new(mode);

            match cli.command {
                Command::Spawn(args) => run(*args, &sink),
                Command::Msg(args) => run(args, &sink),
                Command::Kill(args) => run(args, &sink),
                Command::Agents(args) => run(args, &sink),
                Command::Prime(args) => run(args, &sink),
            }
        }
        Err(error) => report_argument_failure(&error),
    }
}

/// Runs one command and maps its outcome to the process exit code.
fn run<C: Cmd>(command: C, sink: &Sink) -> ExitCode {
    match command.execute(sink) {
        Ok(value) => {
            if C::TEXT_IN_BOTH_MODES {
                sink.out_text(&value);
            } else {
                sink.out(&value);
            }
            ExitStatus::Success.into()
        }
        Err(error) => {
            let status = error.exit_status();
            sink.error(&Failure::new(&error), status.into());
            status.into()
        }
    }
}

// =====================================================================================================================
// Argument Failures
// =====================================================================================================================

/// An argument failure, restated in this crate's own words.
///
/// A type rather than a bare string so the sink gives it the same envelope as every other failure.
#[derive(Debug, Error)]
#[error("{0}")]
struct ArgumentError(String);

impl AsExitStatus for ArgumentError {
    fn exit_status(&self) -> ExitStatus {
        ExitStatus::Usage
    }
}

/// Reports clap's rejection through the sink and maps it to the exit-status contract.
fn report_argument_failure(error: &clap::Error) -> ExitCode {
    let mode = if json_requested(argv()) {
        OutputMode::Json
    } else {
        OutputMode::Human
    };
    let sink = Sink::new(mode);

    // Help and version are answers, not failures: clap reports them as errors only to stop parsing.
    if matches!(error.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
        // clap ends its rendering with a newline and the sink writes one of its own.
        sink.out_text(error.render().to_string().trim_end());
        return ExitStatus::Success.into();
    }

    let failure = ArgumentError(describe(error, &argv()));
    let status = failure.exit_status();
    sink.error(&Failure::new(&failure), status.into());
    status.into()
}

/// Restates one clap rejection without repeating anything the caller supplied.
///
/// [`ContextKind::InvalidValue`] is never read — that context *is* the caller's token.
fn describe(error: &clap::Error, arguments: &[String]) -> String {
    let declared = declared_spellings();
    let ours = |kind| {
        strings_at(error, kind)
            .into_iter()
            .filter(|value| declared.iter().any(|spelling| spelling == value))
            .collect::<Vec<String>>()
    };

    let detail = match error.kind() {
        ErrorKind::MissingRequiredArgument => with_names("missing a required argument", &ours(ContextKind::InvalidArg)),
        ErrorKind::UnknownArgument => match ours(ContextKind::SuggestedArg).first() {
            Some(suggestion) => format!("unrecognized argument, closest match {suggestion}"),
            None => "unrecognized argument".to_owned(),
        },
        ErrorKind::InvalidSubcommand | ErrorKind::MissingSubcommand => "no such command".to_owned(),
        // Read unfiltered: clap builds this context from the argument's own possible values.
        ErrorKind::InvalidValue => with_values(
            &with_names("invalid value for", &ours(ContextKind::InvalidArg)),
            &strings_at(error, ContextKind::ValidValue),
        ),
        // The source is one of this crate's own value parsers, so it is safe to quote.
        ErrorKind::ValueValidation => match error.source() {
            Some(source) => format!(
                "{}: {source}",
                with_names("invalid value for", &ours(ContextKind::InvalidArg))
            ),
            None => with_names("invalid value for", &ours(ContextKind::InvalidArg)),
        },
        ErrorKind::ArgumentConflict => with_names("conflicting arguments", &ours(ContextKind::PriorArg)),
        ErrorKind::NoEquals => with_names("missing a value for", &ours(ContextKind::InvalidArg)),
        ErrorKind::TooManyValues | ErrorKind::TooFewValues | ErrorKind::WrongNumberOfValues => {
            with_names("wrong number of values for", &ours(ContextKind::InvalidArg))
        }
        // clap grows kinds; an unrecognized one still exits 2.
        _ => "invalid arguments".to_owned(),
    };

    match subcommand_named(arguments) {
        Some(command) => format!("{detail}; run `herdr-team {command} --help`"),
        None => format!("{detail}; run `herdr-team --help`"),
    }
}

/// Every spelling this build declares, in the forms clap puts into an error context.
///
/// The allowlist *is* the redaction: clap puts the caller's own tokens in the same context slots.
fn declared_spellings() -> Vec<String> {
    fn spellings_of(command: &clap::Command, into: &mut Vec<String>) {
        for argument in command.get_arguments() {
            // The usage spelling — `--wait-until <STATE>` — plus the bare flags for the contexts
            // that carry those instead.
            into.push(argument.to_string());
            into.extend(argument.get_long().map(|long| format!("--{long}")));
            into.extend(argument.get_short().map(|short| format!("-{short}")));
        }
    }

    // `build()` first: an argument renders its usage spelling only once clap has finalized it.
    let mut root = Cli::command();
    root.build();

    let mut spellings = Vec::new();
    spellings_of(&root, &mut spellings);
    for command in root.get_subcommands() {
        spellings_of(command, &mut spellings);
    }
    spellings
}

/// One context's strings, whether clap stored one or several.
fn strings_at(error: &clap::Error, kind: ContextKind) -> Vec<String> {
    match error.get(kind) {
        Some(ContextValue::String(one)) => vec![one.clone()],
        Some(ContextValue::Strings(many)) => many.clone(),
        _ => Vec::new(),
    }
}

/// A headline with the argument names it is about, or the headline alone when none survived.
fn with_names(headline: &str, names: &[String]) -> String {
    if names.is_empty() {
        headline.to_owned()
    } else {
        format!("{headline} {}", names.join(", "))
    }
}

/// A detail with the values the argument would have accepted, when clap listed them.
fn with_values(detail: &str, values: &[String]) -> String {
    if values.is_empty() {
        detail.to_owned()
    } else {
        format!("{detail}; expected one of {}", values.join(", "))
    }
}

/// The subcommand argv named, when it named one this build defines.
///
/// Matched against declared names so the word in the message is this build's, not the caller's;
/// an alias answers with the canonical name.
fn subcommand_named(arguments: &[String]) -> Option<String> {
    let root = Cli::command();
    arguments.iter().skip(1).find_map(|argument| {
        root.get_subcommands()
            .find(|command| command.get_name() == argument || command.get_all_aliases().any(|alias| alias == argument))
            .map(|command| command.get_name().to_owned())
    })
}

/// Whether argv asked for `--json`, read without a parse — the failure path has no parsed [`Cli`].
///
/// Steps over a bare `--` and the value after either spelling of the message flag, which is
/// arbitrary text and may spell this flag.
fn json_requested(arguments: Vec<String>) -> bool {
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--json" => return true,
            "--msg" | "--prompt" => {
                arguments.next();
            }
            "--" => return false,
            _ => {}
        }
    }
    false
}

/// This process's arguments, with non-UTF-8 replaced: both readers compare against ASCII spellings.
fn argv() -> Vec<String> {
    std::env::args_os()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    /// The rejection a caller sees for one argv, as [`report_argument_failure`] would render it.
    fn rejection(words: &[&str]) -> String {
        let error = Cli::try_parse_from(words).expect_err("this argv is rejected");
        describe(&error, &argv(words))
    }

    /// Each argv is a way prompt text reaches the parser and is rejected there.
    #[test]
    fn a_rejected_prompt_is_never_repeated_back() {
        let secret = "wait, before you commit, the staging password is hunter2";

        for words in [
            argv(&["herdr-team", "prompt", "reviewer", secret, "extra"]),
            argv(&["herdr-team", "prompt", secret]),
            argv(&["herdr-team", "spawn", "reviewer", "--msg"]),
            argv(&["herdr-team", "spawn", "reviewer", "--msg", secret, "extra"]),
            argv(&["herdr-team", "spawn", "reviewer", "--prompt", secret, "extra"]),
            argv(&["herdr-team", "prompt", "reviewer", "--wait-until", secret]),
        ] {
            let borrowed: Vec<&str> = words.iter().map(String::as_str).collect();
            let error = Cli::try_parse_from(&borrowed).expect_err("this argv is rejected");
            let rendered = describe(&error, &words);

            assert!(
                !rendered.contains("hunter2") && !rendered.contains("password"),
                "{rendered}"
            );
        }
    }

    /// A stray positional lands in `InvalidArg`, the context every other arm reads.
    #[test]
    fn a_stray_positional_is_reported_without_being_named() {
        let rendered = rejection(&["herdr-team", "kill", "reviewer", "extra"]);

        assert!(!rendered.contains("extra"), "{rendered}");
        assert!(rendered.contains("unrecognized argument"), "{rendered}");
    }

    #[test]
    fn a_missing_argument_names_the_argument_and_the_help_that_describes_it() {
        assert_eq!(
            rejection(&["herdr-team", "msg", "reviewer"]),
            "missing a required argument <TEXT>; run `herdr-team msg --help`"
        );
    }

    /// clap silently takes a variant's doc comment as that subcommand's `--help` summary.
    #[test]
    fn every_commands_summary_is_the_one_line_its_own_args_declares() {
        let mut root = Cli::command();
        root.build();

        for command in root.get_subcommands() {
            let about = command
                .get_about()
                .unwrap_or_else(|| panic!("{} has no summary at all", command.get_name()))
                .to_string();

            // A length bound is the cheap shape of "this is a summary".
            assert!(
                about.lines().count() == 1 && about.len() <= 100,
                "{}'s summary is not a summary: {about}",
                command.get_name()
            );
        }
    }

    /// The retired spelling still parses, and still reaches the command it was renamed to.
    #[test]
    fn the_command_that_was_renamed_still_answers_to_what_it_was_called() {
        let parsed = Cli::try_parse_from(["herdr-team", "prompt", "reviewer", "go"]).expect("the alias parses");

        assert!(matches!(parsed.command, Command::Msg(_)));
    }

    /// A rejection under the old spelling points at the new one, which is where the flags are.
    #[test]
    fn a_rejection_under_the_old_spelling_names_the_command_it_is_now() {
        let rendered = rejection(&["herdr-team", "prompt", "reviewer"]);

        assert!(rendered.contains("herdr-team msg --help"), "{rendered}");
        assert!(!rendered.contains("prompt"), "{rendered}");
    }

    /// The flag was renamed alongside its command, and both spellings still deliver.
    #[test]
    fn the_first_message_flag_answers_to_both_of_its_spellings() {
        for flag in ["--msg", "--prompt"] {
            assert!(
                Cli::try_parse_from(["herdr-team", "spawn", "worker", flag, "go"]).is_ok(),
                "{flag}"
            );
        }
    }

    #[test]
    fn a_value_the_argument_does_not_accept_is_answered_with_the_ones_it_does() {
        let rendered = rejection(&["herdr-team", "spawn", "reviewer", "--placement", "tba"]);

        assert!(!rendered.contains("tba"), "{rendered}");
        for placement in ["pane", "tab", "workspace", "worktree"] {
            assert!(rendered.contains(placement), "{rendered}");
        }
    }

    #[test]
    fn a_rule_this_build_enforces_is_quoted_because_it_is_this_build_speaking() {
        let rendered = rejection(&["herdr-team", "spawn", "Reviewer"]);

        assert!(rendered.contains("lowercase"), "{rendered}");
        assert!(!rendered.contains("Reviewer"), "{rendered}");
    }

    #[test]
    fn a_mistyped_flag_is_answered_with_the_flag_it_resembles() {
        let rendered = rejection(&["herdr-team", "spawn", "reviewer", "--placemnt", "tab"]);

        assert!(rendered.contains("--placement"), "{rendered}");
    }

    #[test]
    fn an_unknown_command_is_reported_against_the_top_level_help() {
        assert_eq!(
            rejection(&["herdr-team", "sprawn"]),
            "no such command; run `herdr-team --help`"
        );
    }

    /// The mode a rejection renders in has to be recovered from argv, since nothing parsed.
    #[test]
    fn the_json_flag_is_recovered_from_argv_wherever_it_sits() {
        assert!(json_requested(argv(&["herdr-team", "--json", "agents"])));
        assert!(json_requested(argv(&["herdr-team", "agents", "--json"])));
        assert!(!json_requested(argv(&["herdr-team", "agents"])));
    }

    /// Neither of the two places `--json` may appear as data is read as the flag.
    #[test]
    fn a_json_that_is_data_rather_than_a_flag_does_not_switch_the_mode() {
        for flag in ["--msg", "--prompt"] {
            assert!(
                !json_requested(argv(&["herdr-team", "spawn", "worker", flag, "--json"])),
                "{flag}"
            );
        }
        assert!(!json_requested(argv(&[
            "herdr-team",
            "spawn",
            "worker",
            "--",
            "--json"
        ])));
    }

    #[test]
    fn help_and_version_are_answers_rather_than_failures() {
        for flag in ["--help", "--version"] {
            let error = Cli::try_parse_from(["herdr-team", flag]).expect_err("clap stops parsing");

            assert!(
                matches!(error.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion),
                "{flag} reported {:?}",
                error.kind()
            );
        }
    }

    #[test]
    fn the_allowlist_holds_the_spelling_clap_actually_reports() {
        let declared = declared_spellings();

        for spelling in ["<TEXT>", "--placement <PLACEMENT>", "--json", "--force"] {
            assert!(
                declared.iter().any(|known| known == spelling),
                "{spelling} is not in the allowlist"
            );
        }
    }
}
