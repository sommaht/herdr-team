//! `herdr-agent-tools` — launch and prompt herdr agents from one command.
//!
//! Parses and dispatches; no command logic lives here. There is no `cli` module: even at five
//! commands the dispatch match is a handful of lines, and a file holding only module declarations
//! plus that match would name no boundary.

mod cmd;
mod config;
mod core;
mod harness;
mod herdr;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::cmd::{AsExitStatus, Cmd, ExitStatus, Failure, KillArgs, PresetsArgs, PrimeArgs, PromptArgs, SpawnArgs};
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
        tab, or workspace, reads back the new pane's id, and starts a preset-configured agent in \
        it. `prompt` delivers text to an agent that already exists, `kill` closes an agent's pane \
        unless it is mid-task, and `presets` lists what the config file holds.\n\
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
    Presets(PresetsArgs),
    Prime(PrimeArgs),
}

// =====================================================================================================================
// Entry Point
// =====================================================================================================================

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mode = if cli.json { OutputMode::Json } else { OutputMode::Human };
    // Built before any command runs, so a failure that happens before one starts renders under the
    // same contract as everything else.
    let sink = Sink::new(mode);

    match cli.command {
        Command::Spawn(args) => run(args, &sink),
        Command::Prompt(args) => run(args, &sink),
        Command::Kill(args) => run(args, &sink),
        Command::Presets(args) => run(args, &sink),
        Command::Prime(args) => run(args, &sink),
    }
}

/// Runs one command and maps its outcome to the process exit code.
///
/// The sink comes last: it is the context a command reports through, not the thing the command acts
/// on.
fn run<C: Cmd>(command: C, sink: &Sink) -> ExitCode {
    match command.execute(sink) {
        Ok(value) => {
            sink.out(&value);
            ExitStatus::Success.into()
        }
        Err(error) => {
            let status = error.exit_status();
            sink.error(&Failure::new(&error), status.into());
            status.into()
        }
    }
}
