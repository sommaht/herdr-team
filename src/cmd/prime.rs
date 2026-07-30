//! `prime` — print an agent-facing brief on driving this CLI.
//!
//! Written to be run from a session-start hook, which sets every constraint here: it makes no herdr
//! call, because nothing guarantees a running server at that point, and it cannot fail, because a
//! hook that fails is worse than one that says little.
//!
//! The brief is hand-written rather than composed from clap's help. Five subcommands render past two
//! hundred lines, and context an agent carries all session has a cost that a reference dump cannot
//! justify when `--help` is one command away. What it gives instead is invocations grouped by intent,
//! with the gotchas attached to the line they qualify — because what an agent cannot look up on demand
//! is which failures are worth retrying and which commands refuse by default.
//!
//! Hand-written means drift is possible, so three tests hold it honest: the commands and flags it names
//! must exist, the commands that exist must be named, and the exit codes it teaches must be the ones
//! the contract reports. The preset table is generated outright, from the same listing `presets`
//! prints, so it cannot disagree with what `spawn --preset` will do.
//!
//! `--hook <harness>` asks the named harness to wrap the brief for its host. This module never learns
//! the envelope's shape.

use std::fmt::Display;
use std::path::PathBuf;

use clap::Args;
use serde::Serialize;
use thiserror::Error;

use crate::cmd::Cmd;
use crate::cmd::presets::PresetList;
use crate::config::Presets;
use crate::core::Sink;
use crate::harness::{self, AgentHarness};

// =====================================================================================================================
// The Brief
// =====================================================================================================================

/// The brief itself.
///
/// Shaped as command examples grouped by intent rather than as explanation, because a caller reaching
/// for this wants the invocation, not the rationale. What prose there is attaches to the line it
/// qualifies: `prompt` returning on delivery rather than completion is a clause on the `--wait-until`
/// row, not a paragraph. The gotchas are the reason this exists — a flag list cannot say which
/// failures are worth retrying, or that two commands refuse by default.
///
/// Subject to a line budget enforced by a test, since this is context an agent carries for a whole
/// session and nothing else pushes back on it growing.
const GUIDANCE: &str = "\
# herdr-agent-tools

Launch and prompt herdr agents. herdr starts an agent only in a pane that already exists and is
sitting at a shell prompt, so `spawn` creates the surface and starts the agent in one step.

> Context recovery: run `herdr-agent-tools prime` again after a compaction or in a new session.

A <target> is a unique agent name or a herdr pane id, resolved by herdr. Your own pane, tab, and
workspace ids come from `herdr pane current`. Every command below is run through
`herdr-agent-tools`, prints one terse line, and takes `--json` for when a pipeline has to parse
the output rather than you reading it. `prime` is the one exception: its brief is a document, so
`--json` prints this same text. Do not pipe it to a JSON parser.

## Launching agents

    spawn <name>                          split the calling pane; needs HERDR_PANE_ID
    spawn <name> --placement tab          give it a tab of its own
    spawn <name> --placement workspace    give it a workspace of its own
    spawn <name> --placement worktree     a Git worktree of its own, on a new branch
    spawn <name> --branch <name>          name that branch; otherwise herdr picks
    spawn <name> --preset <preset>        pick which agent starts; see Presets below
    spawn <name> --prompt \"<text>\"        deliver a first prompt once it is up
    spawn <name> --prompt -               read that first prompt from stdin
    spawn <name> --cwd <path>             start it somewhere other than here
    spawn <name> --focus                  move the cursor to it; off by default
    spawn <name> -- <agent args>          extra args, appended after the preset's

## Prompting agents

    prompt <target> \"<text>\"              returns once delivery is proven, not when the turn ends
    prompt <target> -                     read the prompt from stdin; beats quoting a long one
    prompt <target> \"<text>\" --wait-until idle    wait for the turn to finish instead
    prompt <target> \"<text>\" --force      send even into a composer holding unsent text
    prompt <target> \"<text>\" --no-verify  submit without waiting for proof it landed

## Ending agents

    kill <target>                         close its pane; refuses a working or blocked agent
    kill <target> --force                 close it anyway, losing whatever is not on disk

## Reference

    presets                               list what the preset config holds
    prime                                 print this brief again; text in both modes

## Two refusals you will meet

Both exit 5 and both leave everything unchanged. Neither clears on a timer, so wait for the
state the message names rather than retrying on a loop. `--force` overrides either.

    prompt    the composer holds someone's unsent text; a person has to send or clear it
    kill      the target is working, or blocked and waiting on someone

## Exit codes are a protocol

    5    a conflict; retry once the state the message names has changed, not on a loop. A
         pane whose shell is still starting clears itself. An occupied composer, a working
         agent, and a name already taken do not — read the message and act on it.
    3    what you named does not exist
    2    the arguments were wrong, whether this tool rejected them or herdr did
    1    something else failed

## Common workflows

Launch a reviewer in its own tab and hand it the diff:

    git diff | herdr-agent-tools spawn reviewer --placement tab --prompt -

Dispatch work and block until there is a result to read:

    herdr-agent-tools prompt reviewer \"run the tests and report failures\" --wait-until idle

Fan out, then clean up when one is done:

    for area in api web cli; do
      herdr-agent-tools spawn \"$area\" --placement tab --prompt \"audit the $area surface\"
    done
    herdr-agent-tools kill api

Collect pane ids for a script rather than for reading. One run may print warning lines before
its result, so select the result instead of taking the first line:

    herdr-agent-tools --json spawn worker --placement tab \\
      | jq -er 'select(.type == \"result\") | .agent.pane_id'";

// =====================================================================================================================
// Prime Args
// =====================================================================================================================

/// Print a brief on driving this CLI, for a session-start hook to feed an agent.
///
/// Makes no herdr call and cannot fail: a config it cannot read costs the preset table and nothing
/// else.
///
/// This is the crate's one `--json` exception. The brief is a document rather than a record, so
/// both modes print the same text — wrapping prose in a JSON envelope buys escaping and no
/// information. Said here, in `--json`'s own help, and in the brief itself, so a consumer learns it
/// before a parser does.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  \
    herdr-agent-tools prime\n  \
    herdr-agent-tools prime --config ./config.toml\n\
    \n\
    --json prints this same text: the brief is a document, not a record.")]
pub struct PrimeArgs {
    /// Read this preset file instead of the one in the config directory.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Wrap the brief in this harness's session-start hook envelope, for a host to inject as context.
    ///
    /// Rejected at parse time for a harness this build does not know, so a hook with a typo in it
    /// fails loudly rather than quietly feeding an agent nothing.
    #[arg(long, value_name = "HARNESS", value_parser = known_harness)]
    hook: Option<String>,
}

/// A `--hook` value naming a harness this build cannot wrap for.
///
/// A typed error rather than the `String` clap would also accept: the choices are data, and spelling
/// them into a string at the point of failure would put the formatting somewhere no test can reach.
#[derive(Debug, Error)]
#[error("unknown harness; this build wraps for {}", known.join(", "))]
struct UnknownHarness {
    /// Every kind that would have been accepted.
    known: Vec<&'static str>,
}

/// Accepts a harness name this build can wrap for, and names the choices when it cannot.
///
/// A `value_parser` rather than a check inside `execute`, which keeps a bad name an exit-2 argument
/// error and leaves this command otherwise infallible. A `ValueEnum` would need a second list of
/// harnesses beside [`harness::HARNESSES`] to derive on.
fn known_harness(value: &str) -> Result<String, UnknownHarness> {
    if harness::by_kind(value).is_some() {
        Ok(value.to_owned())
    } else {
        Err(UnknownHarness { known: harness::kinds() })
    }
}

impl Cmd for PrimeArgs {
    type Ok = Brief;
    type Err = std::convert::Infallible;

    /// The crate's one `--json` exception. A brief is a document, so both modes print it.
    const TEXT_IN_BOTH_MODES: bool = true;

    fn execute(self, _sink: &Sink) -> Result<Self::Ok, Self::Err> {
        // The one place a `ConfigError` is deliberately dropped rather than reported. A hook fires
        // before anyone has necessarily written a config, and failing there would cost the whole
        // brief to say something the brief itself already says.
        let presets = Presets::load(self.config.as_deref())
            .ok()
            .map(|presets| PresetList::of(&presets));
        // Resolved here rather than carried as a name, so `Display` has nothing left to look up.
        // `expect` is safe because `known_harness` rejected anything `by_kind` cannot resolve.
        let host = self
            .hook
            .as_deref()
            .map(|kind| harness::by_kind(kind).expect("the value parser accepted this harness"));
        Ok(Brief { guidance: GUIDANCE, presets, host })
    }
}

// =====================================================================================================================
// Output
// =====================================================================================================================

/// The brief, in the order it prints.
#[derive(Debug, Serialize)]
pub struct Brief {
    /// The brief itself, verbatim.
    guidance: &'static str,
    /// The preset table, absent when no config could be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    presets: Option<PresetList>,
    /// The host to wrap for, absent when the brief is printed bare.
    ///
    /// Skipped on the wire: a trait object has nothing to serialize, and `--hook` is a framing choice
    /// rather than part of the brief.
    #[serde(skip)]
    host: Option<&'static dyn AgentHarness>,
}

/// Told to the reader rather than to the host, because the host is what truncated it.
///
/// Costs one line against an agent silently acting on half a brief. Cheap here in a way it is not for
/// tools that persist hook output elsewhere: re-running this command is the whole recovery.
const TRUNCATION_NOTE: &str =
    "[herdr-agent-tools prime] If your host truncated this, run `herdr-agent-tools prime` to read it in full.";

impl Display for Brief {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Some(host) = self.host else {
            return f.write_str(&self.text());
        };
        // The note goes inside the envelope, since a truncating host is what makes it worth saying.
        let context = format!("{TRUNCATION_NOTE}\n\n{}", self.text());
        // RS-002: the source is discarded because `fmt::Error` is a unit type with nowhere to carry
        // one, and `Display` cannot return anything else. Nothing is lost that a caller could act on —
        // per `AgentHarness::hook`, the only way this fails is a serde bug over two string fields.
        f.write_str(&host.hook(&context).map_err(|_| std::fmt::Error)?)
    }
}

impl Brief {
    /// The brief as text: the guidance, then the preset table.
    ///
    /// Shared by both forms, because the hook envelope carries exactly what a reader would have seen.
    fn text(&self) -> String {
        match &self.presets {
            Some(presets) => format!("{}\n\n## Presets\n\n{presets}", self.guidance),
            // Said rather than omitted: an agent that knows presets exist and sees none knows not to
            // reach for `--preset`.
            None => format!("{}\n\n## Presets\n\nNone configured.", self.guidance),
        }
    }
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;
    use crate::cmd::ExitStatus;

    /// The longest the brief may run.
    ///
    /// Lives here because it exists only as an assertion — a budget rather than a suggestion, and
    /// raising it is then a deliberate edit to a test rather than a number that quietly drifts.
    ///
    /// Raised from forty when the brief became command blocks instead of paragraphs. Runnable lines
    /// earn their length in a way explanation does not, and five commands' worth of invocations plus
    /// worked examples does not fit in forty. It is still a ceiling: this is context an agent carries
    /// for a whole session.
    ///
    /// A hundred rather than the eighty that first replaced forty, which the brief immediately came
    /// within three lines of — a ceiling that tight makes the next ordinary edit a budget negotiation.
    /// The reference this borrows from runs about a hundred and fifty lines for forty-odd commands, so
    /// a hundred for five is already generous, and reaching it should prompt a rewrite rather than
    /// another raise.
    const LINE_BUDGET: usize = 100;

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

    /// Tokens in the brief that look like long flags.
    ///
    /// The brief writes flags bare in its command blocks and backticked in prose, so a token can
    /// arrive as "`--placement". Everything that is neither alphanumeric nor a dash comes off both
    /// ends, which leaves a leading `--` intact. The length guard drops the bare `--` separator on
    /// spawn's extra-args row, which is a separator rather than a flag.
    fn flags_named() -> Vec<&'static str> {
        GUIDANCE
            .split_whitespace()
            .map(|word| word.trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '-'))
            .filter(|word| word.starts_with("--") && word.len() > 2)
            .collect()
    }

    #[test]
    fn a_config_that_cannot_be_read_costs_the_table_and_nothing_else() {
        // The hook constraint: firing before anyone wrote a config must still produce the brief.
        let brief = rendered();

        assert!(brief.contains("## Presets\n\nNone configured."), "got {brief}");
        assert!(brief.contains("## Launching agents"), "the brief survived");
    }

    /// The brief names every command, and every command it names exists.
    ///
    /// Both directions matter and they catch different things. A command renamed out from under the
    /// brief fails the first; a command *added* without being documented fails the second, which is
    /// the one that would otherwise go unnoticed — an agent cannot reach for what the brief never
    /// mentions. clap's built-in `help` is excluded: it answers a human browsing.
    #[test]
    fn the_brief_documents_exactly_the_commands_that_exist() {
        let root = crate::Cli::command();
        let commands: Vec<String> = root
            .get_subcommands()
            .map(|command| command.get_name().to_owned())
            .filter(|name| name != "help")
            .collect();

        assert!(!commands.is_empty(), "clap reported no subcommands at all");
        for command in &commands {
            assert!(
                GUIDANCE.contains(command.as_str()),
                "the command {command} exists but the brief never names it"
            );
        }

        // The reverse: every full invocation in the brief names a command that exists. Splitting on
        // the binary plus a space deliberately misses the title line, which has no space after it.
        // Leading tokens that start with a dash are skipped, since `--json` can precede the command.
        for tail in GUIDANCE.split("herdr-agent-tools ").skip(1) {
            let Some(candidate) = tail
                .split_whitespace()
                .find(|token| !token.starts_with('-'))
                .map(|token| token.trim_matches(|character: char| !character.is_ascii_alphanumeric()))
            else {
                continue;
            };
            assert!(
                commands.iter().any(|command| command == candidate),
                "the brief invokes `herdr-agent-tools {candidate}`, which is not a command"
            );
        }
    }

    /// Every flag the brief names still exists somewhere in the CLI.
    ///
    /// Catches the drift that actually happens — a flag renamed or removed while the brief keeps
    /// recommending it. It cannot catch a flag attributed to the wrong command; with five commands
    /// that is what reading the brief is for.
    #[test]
    fn every_flag_the_brief_recommends_still_exists() {
        let root = crate::Cli::command();
        // Global flags such as `--json` live on the root, not on the subcommands clap propagates them
        // to — reading only the subcommands is what made a first version of this reject `--json`.
        let mut known: Vec<String> = root
            .get_arguments()
            .filter_map(|argument| argument.get_long().map(|long| format!("--{long}")))
            .collect();
        for command in root.get_subcommands() {
            for argument in command.get_arguments() {
                known.extend(argument.get_long().map(|long| format!("--{long}")));
            }
        }

        let mentioned = flags_named();

        // Without this the test passes vacuously the moment the extraction stops working.
        assert!(
            !mentioned.is_empty(),
            "the brief names no flags at all, which is suspicious"
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
    /// Scanning the whole brief for digits was the obvious version and it does not survive real
    /// content: a `--timeout` row naming a default in milliseconds, or any number in an example, would
    /// read as a bogus exit code. Only rows whose *first* token is a bare integer are the table, which
    /// is exactly how that block is written.
    ///
    /// What it does not catch: prose that thins out. Deleting the table still leaves the retryable code
    /// named in the refusals section, and it passes — asserting otherwise would mean pinning wording,
    /// which is the thing that makes a test get weakened later.
    #[test]
    fn every_exit_code_the_brief_teaches_is_one_the_contract_reports() {
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

        let tabled: Vec<&str> = GUIDANCE
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|token| token.chars().all(|character| character.is_ascii_digit()))
            .collect();

        assert!(!tabled.is_empty(), "the brief tabulates no exit codes at all");
        for code in &tabled {
            assert!(
                contracted.iter().any(|contracted| contracted == code),
                "the brief teaches exit code {code}, which the contract does not report"
            );
        }

        // The one status whose absence would actually hurt: an agent that never learns which code is
        // retryable gives up where it should retry.
        let retryable = u8::from(ExitStatus::Conflict).to_string();
        assert!(
            tabled.contains(&retryable.as_str()),
            "the brief never tabulates {retryable} as the retryable code"
        );
    }

    #[test]
    fn the_brief_stays_inside_its_line_budget() {
        // Context an agent carries all session, with nothing else pushing back on it growing.
        let lines = GUIDANCE.lines().count();

        assert!(
            lines <= LINE_BUDGET,
            "the prose is {lines} lines, over the {LINE_BUDGET} budget"
        );
    }

    /// The brief shows invocations, not explanations.
    ///
    /// The shape is the point, so it is worth one assertion: every command gets a block of runnable
    /// lines, and there is a worked pipeline at the end. A rewrite back into paragraphs fails here.
    #[test]
    fn the_brief_is_built_from_runnable_lines_rather_than_paragraphs() {
        let indented = GUIDANCE
            .lines()
            .filter(|line| line.starts_with("    ") && !line.trim().is_empty())
            .count();

        assert!(indented >= 20, "only {indented} runnable lines, which reads as prose");
        assert!(GUIDANCE.contains("## Common workflows"), "no worked examples");
        assert!(GUIDANCE.contains("| jq"), "nothing shows what --json is for");
    }

    #[test]
    fn the_wire_form_carries_the_brief_and_the_generated_table_separately() {
        let brief = Brief {
            guidance: "the brief",
            presets: None,
            host: None,
        };

        assert_eq!(serde_json::to_string(&brief).unwrap(), r#"{"guidance":"the brief"}"#);
    }

    /// The hook form is one line of valid JSON carrying the whole brief.
    ///
    /// Parsed rather than string-matched: the brief holds quotes, backticks and newlines, and the point
    /// of building the envelope with serde is that a host can actually parse what comes out.
    #[test]
    fn the_hook_form_is_parseable_json_carrying_the_brief_verbatim() {
        let rendered = Harness::try_parse_from(["prime", "--hook", "claude"])
            .expect("parses")
            .args
            .execute(&Sink::new(crate::core::OutputMode::Human))
            .expect("prime cannot fail")
            .to_string();

        assert_eq!(rendered.lines().count(), 1, "a hook reads one line of stdout");

        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("a host must be able to parse this");
        let payload = &parsed["hookSpecificOutput"];
        assert_eq!(payload["hookEventName"], "SessionStart");

        let context = payload["additionalContext"].as_str().expect("context is a string");
        assert!(context.contains("## Launching agents"), "the brief is inside");
        assert!(context.contains(TRUNCATION_NOTE), "and so is the truncation note");
    }

    /// Every harness wraps for its own host, even where the shape is currently shared.
    ///
    /// The seam is the assertion: `prime` asks the harness and does not know the shape. When a host's
    /// contract is read and found to differ, this test is where the difference shows up.
    #[test]
    fn each_harness_answers_for_its_own_host() {
        for kind in harness::kinds() {
            let host = harness::by_kind(kind).expect("kinds() names resolvable harnesses");
            let wrapped = host.hook("a brief").expect("two string fields serialize");

            let parsed: serde_json::Value = serde_json::from_str(&wrapped).expect("valid JSON");
            assert_eq!(
                parsed["hookSpecificOutput"]["additionalContext"], "a brief",
                "{kind} dropped or mangled the brief"
            );
        }
    }

    #[test]
    fn a_harness_this_build_cannot_wrap_for_is_rejected_with_the_ones_it_can() {
        let error = Harness::try_parse_from(["prime", "--hook", "emacs"])
            .expect_err("an unknown harness is an argument error")
            .to_string();

        for kind in harness::kinds() {
            assert!(error.contains(kind), "the rejection should name {kind}: {error}");
        }
    }
}
