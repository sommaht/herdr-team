//! `herdr-agent-tools` — launch and prompt herdr agents from one command.
//!
//! Parses and dispatches; no command logic lives here. There is no `cli` module: even at five
//! commands the dispatch match is a handful of lines, and a file holding only module declarations
//! plus that match would name no boundary. What this file does own beyond dispatch is the argument
//! failure — clap's rejection has to reach the caller under the same output contract as every other
//! failure, and without repeating what the caller typed.

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

use crate::cmd::{AgentsArgs, AsExitStatus, Cmd, ExitStatus, Failure, KillArgs, PrimeArgs, PromptArgs, SpawnArgs};
use crate::core::{OutputMode, Sink};

// =====================================================================================================================
// Cli
// =====================================================================================================================

#[derive(Debug, Parser)]
#[command(
    name = "herdr-agent-tools",
    version,
    about = "Launch and prompt herdr agents from one command",
    long_about = "Launch and prompt herdr agents from one command.\n\
        \n\
        herdr starts an agent only in a pane that already exists and is sitting at an interactive \
        shell prompt, so launching one by hand is two steps. `spawn` does both: it creates a pane, \
        tab, or workspace, reads back the new pane's id, and starts a configured agent in it. \
        `prompt` delivers text to an agent that already exists, `kill` closes an agent's pane \
        unless it is mid-task, and `agents` lists what the config holds.\n\
        \n\
        Run `herdr-agent-tools <command> --help` for details and examples.",
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

/// The subcommand set. Variants carry no doc comments deliberately: each command's help is owned by
/// its `*Args` struct, next to the flags it documents.
#[derive(Debug, Subcommand)]
enum Command {
    Spawn(SpawnArgs),
    Prompt(PromptArgs),
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
            // Built before any command runs, so a failure that happens before one starts renders
            // under the same contract as everything else.
            let sink = Sink::new(mode);

            match cli.command {
                Command::Spawn(args) => run(args, &sink),
                Command::Prompt(args) => run(args, &sink),
                Command::Kill(args) => run(args, &sink),
                Command::Agents(args) => run(args, &sink),
                Command::Prime(args) => run(args, &sink),
            }
        }
        // `try_parse` rather than `parse`, which prints clap's own prose and exits before a sink
        // exists — see [`report_argument_failure`].
        Err(error) => report_argument_failure(&error),
    }
}

/// Runs one command and maps its outcome to the process exit code.
///
/// The sink comes last: it is the context a command reports through, not the thing the command acts
/// on.
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
/// A type rather than a bare string because the sink renders failures, not messages: this is what
/// gives clap's rejection the same `{"type":"error","status":2,…}` envelope every other failure has.
#[derive(Debug, Error)]
#[error("{0}")]
struct ArgumentError(String);

impl AsExitStatus for ArgumentError {
    /// Always [`ExitStatus::Usage`], which is clap's own code for the same thing.
    fn exit_status(&self) -> ExitStatus {
        ExitStatus::Usage
    }
}

/// Reports clap's rejection through the sink and maps it to the exit-status contract.
///
/// Letting clap print for itself breaks both contracts this binary advertises. A `--json` caller
/// gets multi-line prose on stderr and nothing at all on stdout — failing to parse on exactly the
/// errors it most needs to classify — and the prose repeats the offending value back, which for
/// `prompt` and `spawn --prompt` is the prompt text.
///
/// So the rejection is rebuilt from clap's structured context, and the rebuild is an allowlist:
/// every word in the result is either a constant in [`describe`] or a name this build declares. See
/// [`declared_spellings`] for why it is an allowlist rather than a filter over the caller's tokens.
fn report_argument_failure(error: &clap::Error) -> ExitCode {
    let mode = if json_requested(argv()) {
        OutputMode::Json
    } else {
        OutputMode::Human
    };
    let sink = Sink::new(mode);

    // Help and version are answers, not failures: clap reports them as errors because that is how it
    // stops parsing. Both are documents, so they print as text in either mode — `prime`'s exception,
    // for `prime`'s reason.
    if matches!(error.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
        // Trimmed because clap ends its rendering with a newline and the sink writes one of its own,
        // and a blank line after `--version` is a blank line a caller has to strip.
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
/// The headline comes from the kind, the detail from the contexts that hold only this build's own
/// vocabulary: which argument, which values it accepts, and what a value parser of ours said about
/// it. [`ContextKind::InvalidValue`] is never read — that context *is* the caller's token, and for
/// two of these commands the caller's token is a prompt.
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
        // Stated rather than asked, because the trailer follows: a question mark mid-sentence reads
        // as two messages spliced together.
        ErrorKind::UnknownArgument => match ours(ContextKind::SuggestedArg).first() {
            Some(suggestion) => format!("unrecognized argument, closest match {suggestion}"),
            None => "unrecognized argument".to_owned(),
        },
        ErrorKind::InvalidSubcommand | ErrorKind::MissingSubcommand => "no such command".to_owned(),
        // The valid values are read unfiltered: clap builds that context from the argument's own
        // possible values, so it can hold nothing but ours.
        ErrorKind::InvalidValue => with_values(
            &with_names("invalid value for", &ours(ContextKind::InvalidArg)),
            &strings_at(error, ContextKind::ValidValue),
        ),
        // The source is one of this crate's own value parsers — the agent-name rule, the blank-prompt
        // refusal — which is the most useful sentence available and safe for the same reason.
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
        // clap grows kinds, and an unrecognized one is a message rather than a compile error — the
        // exit status is the classification a caller branches on, and it is 2 either way.
        _ => "invalid arguments".to_owned(),
    };

    match subcommand_named(arguments) {
        Some(command) => format!("{detail}; run `herdr-agent-tools {command} --help`"),
        None => format!("{detail}; run `herdr-agent-tools --help`"),
    }
}

/// Every spelling this build declares, in the forms clap puts into an error context.
///
/// The allowlist *is* the redaction. clap's context holds the caller's own tokens beside this
/// build's names, in the same [`ContextKind::InvalidArg`] slot — an unexpected positional lands
/// there verbatim. No filter that inspected those tokens could be trusted, because a prompt is
/// arbitrary text and may spell anything, so the only safe question to ask of a string is whether it
/// is one of ours.
fn declared_spellings() -> Vec<String> {
    fn spellings_of(command: &clap::Command, into: &mut Vec<String>) {
        for argument in command.get_arguments() {
            // The usage spelling — `--wait-until <STATE>`, `<TEXT>` — which is the form clap puts in
            // a context, plus the bare flags for the contexts that carry those instead.
            into.push(argument.to_string());
            into.extend(argument.get_long().map(|long| format!("--{long}")));
            into.extend(argument.get_short().map(|short| format!("-{short}")));
        }
    }

    // Built first: an argument renders its usage spelling only once clap has finalized it, and
    // `command()` hands back the unfinished builder.
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
/// Matched against the declared names rather than taken positionally, so the word that reaches the
/// message is this build's and not the caller's.
fn subcommand_named(arguments: &[String]) -> Option<String> {
    let root = Cli::command();
    arguments.iter().skip(1).find_map(|argument| {
        root.get_subcommands()
            .map(clap::Command::get_name)
            .find(|name| *name == argument)
            .map(ToOwned::to_owned)
    })
}

/// Whether argv asked for `--json`, read without a parse.
///
/// The failure path has no parsed [`Cli`] to read the flag off, and a `--json` caller that meets
/// prose there is the case the contract most has to survive. Two tokens are stepped over: a bare
/// `--`, past which everything belongs to the agent, and the value after `--prompt`, which is
/// arbitrary text and may spell this flag.
fn json_requested(arguments: Vec<String>) -> bool {
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--json" => return true,
            "--prompt" => {
                arguments.next();
            }
            "--" => return false,
            _ => {}
        }
    }
    false
}

/// This process's arguments, with anything not valid UTF-8 replaced rather than refused.
///
/// A lossy read is right for both readers: [`json_requested`] compares against ASCII flags that
/// cannot survive the replacement, and [`subcommand_named`] compares against names this build
/// declares.
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

    /// The leak test, and the reason this file rebuilds clap's message at all.
    ///
    /// Every one of these argv is a way prompt text reaches the parser and is rejected there. clap's
    /// own rendering repeats the offending token in the error and again in its tip; this must not.
    #[test]
    fn a_rejected_prompt_is_never_repeated_back() {
        let secret = "wait, before you commit, the staging password is hunter2";

        for words in [
            argv(&["herdr-agent-tools", "prompt", "reviewer", secret, "extra"]),
            argv(&["herdr-agent-tools", "prompt", secret]),
            argv(&["herdr-agent-tools", "spawn", "reviewer", "--prompt"]),
            argv(&["herdr-agent-tools", "spawn", "reviewer", "--prompt", secret, "extra"]),
            argv(&["herdr-agent-tools", "prompt", "reviewer", "--wait-until", secret]),
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

    /// An unexpected extra positional is where the caller's own token lands in `InvalidArg`, which
    /// is the context every other arm reads. It is dropped by the allowlist rather than by a check
    /// on its shape.
    #[test]
    fn a_stray_positional_is_reported_without_being_named() {
        let rendered = rejection(&["herdr-agent-tools", "kill", "reviewer", "extra"]);

        assert!(!rendered.contains("extra"), "{rendered}");
        assert!(rendered.contains("unrecognized argument"), "{rendered}");
    }

    #[test]
    fn a_missing_argument_names_the_argument_and_the_help_that_describes_it() {
        assert_eq!(
            rejection(&["herdr-agent-tools", "prompt", "reviewer"]),
            "missing a required argument <TEXT>; run `herdr-agent-tools prompt --help`"
        );
    }

    #[test]
    fn a_value_the_argument_does_not_accept_is_answered_with_the_ones_it_does() {
        // The valid list is the useful half and it is all this build's own vocabulary; the value
        // that was rejected is the caller's and stays out.
        let rendered = rejection(&["herdr-agent-tools", "spawn", "reviewer", "--placement", "tba"]);

        assert!(!rendered.contains("tba"), "{rendered}");
        for placement in ["pane", "tab", "workspace", "worktree"] {
            assert!(rendered.contains(placement), "{rendered}");
        }
    }

    /// A value parser of ours failing is the one case where the detail is worth carrying: the rule
    /// it states is herdr's, and a caller that cannot see it has to guess at the name it may use.
    #[test]
    fn a_rule_this_build_enforces_is_quoted_because_it_is_this_build_speaking() {
        let rendered = rejection(&["herdr-agent-tools", "spawn", "Reviewer"]);

        assert!(rendered.contains("lowercase"), "{rendered}");
        assert!(!rendered.contains("Reviewer"), "{rendered}");
    }

    #[test]
    fn a_mistyped_flag_is_answered_with_the_flag_it_resembles() {
        let rendered = rejection(&["herdr-agent-tools", "spawn", "reviewer", "--placemnt", "tab"]);

        assert!(rendered.contains("--placement"), "{rendered}");
    }

    #[test]
    fn an_unknown_command_is_reported_against_the_top_level_help() {
        assert_eq!(
            rejection(&["herdr-agent-tools", "sprawn"]),
            "no such command; run `herdr-agent-tools --help`"
        );
    }

    /// The mode a rejection renders in has to be recovered from argv, since nothing parsed.
    #[test]
    fn the_json_flag_is_recovered_from_argv_wherever_it_sits() {
        assert!(json_requested(argv(&["herdr-agent-tools", "--json", "agents"])));
        assert!(json_requested(argv(&["herdr-agent-tools", "agents", "--json"])));
        assert!(!json_requested(argv(&["herdr-agent-tools", "agents"])));
    }

    /// Neither of the two places `--json` may appear as data is read as the flag.
    #[test]
    fn a_json_that_is_data_rather_than_a_flag_does_not_switch_the_mode() {
        // A prompt may spell anything, and everything past `--` is the agent's.
        assert!(!json_requested(argv(&[
            "herdr-agent-tools",
            "spawn",
            "worker",
            "--prompt",
            "--json"
        ])));
        assert!(!json_requested(argv(&[
            "herdr-agent-tools",
            "spawn",
            "worker",
            "--",
            "--json"
        ])));
    }

    #[test]
    fn help_and_version_are_answers_rather_than_failures() {
        for flag in ["--help", "--version"] {
            let error = Cli::try_parse_from(["herdr-agent-tools", flag]).expect_err("clap stops parsing");

            assert!(
                matches!(error.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion),
                "{flag} reported {:?}",
                error.kind()
            );
        }
    }

    /// Every argument spelling clap can name is one the allowlist holds, or the redaction would drop
    /// the useful half of every message.
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
