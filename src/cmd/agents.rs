//! `agents` — list what the config holds.

use std::fmt::Display;
use std::path::{Path, PathBuf};

use clap::Args;
use serde::Serialize;

use crate::cmd::{AsExitStatus, Cmd, ExitStatus};
use crate::config::{Config, ConfigError};
use crate::core::Sink;

// =====================================================================================================================
// Agents Args
// =====================================================================================================================

/// List the agents the config holds, marking the default.
///
/// Touches herdr not at all, so it answers with no server running — which is what is wanted when the
/// config itself is what is being debugged. An agent that failed to resolve is absent from the
/// listing and warned about above it, so this is also where a broken config announces itself.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  \
    herdr-agent-tools agents\n  \
    herdr-agent-tools agents --config ./config.toml --json")]
pub struct AgentsArgs {
    /// Read this config file instead of the one in the config directory.
    ///
    /// The repository's `.herdr-agent-tools/config.toml` is still merged over it.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

impl Cmd for AgentsArgs {
    type Ok = AgentList;
    type Err = ConfigError;

    fn execute(self, sink: &Sink) -> Result<Self::Ok, Self::Err> {
        Ok(AgentList::of(&Config::load(self.config.as_deref(), None, sink)?))
    }
}

impl AsExitStatus for ConfigError {
    fn exit_status(&self) -> ExitStatus {
        self.exit_status_hint()
    }
}

// =====================================================================================================================
// Output
// =====================================================================================================================

/// Every agent the config holds, in name order.
#[derive(Debug, Serialize)]
pub struct AgentList {
    /// The name of the agent a bare `spawn` uses.
    default: String,
    /// One entry per agent.
    agents: Vec<AgentLine>,
}

impl AgentList {
    /// The listing for a loaded config.
    ///
    /// Shared with `prime`, which renders the same table inside its brief: one place builds it, so
    /// what `prime` tells an agent about the config cannot disagree with what `agents` prints or with
    /// what `spawn --agent` will actually do.
    pub(super) fn of(config: &Config) -> Self {
        Self {
            default: config.default_name().to_owned(),
            agents: config
                .iter()
                .map(|(name, agent)| AgentLine {
                    name: name.to_owned(),
                    kind: agent.kind().to_owned(),
                    model: agent.model().map(ToOwned::to_owned),
                    effort: agent.effort().map(ToOwned::to_owned),
                    args: agent.agent_args(),
                    prompt_file: agent.prompt_file().map(|path| path.display().to_string()),
                    source: agent.source().display().to_string(),
                    default: name == config.default_name(),
                })
                .collect(),
        }
    }
}

/// One agent as the listing reports it.
#[derive(Debug, Serialize)]
struct AgentLine {
    /// The agent's name — its table key in the file.
    name: String,
    /// The agent kind it starts.
    kind: String,
    /// The model, as the config states it rather than as a flag.
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    /// The reasoning effort, likewise.
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    /// The exact vector `agent start` receives, tuning flags included.
    ///
    /// Safe to render here: the ban is on error messages and logs, and a listing of the config is the
    /// one place these are the answer.
    args: Vec<String>,
    /// The file prepended to this agent's first prompt, absolute.
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_file: Option<String>,
    /// The config file that declared this agent, which is the question two layers create.
    source: String,
    /// Whether this is the config's `default`.
    default: bool,
}

/// One line per agent, which makes this the one result that spans several lines.
///
/// The sink's usual contract is one line per value; a listing has nothing else it could honestly be,
/// and the `--json` form is one object either way.
///
/// The brief is rendered by file name rather than by path: the whole path is long, it is already in
/// the `--json` form exactly, and what a reader is checking here is *which* brief.
impl Display for AgentList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, agent) in self.agents.iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            write!(f, "{} ({})", agent.name, agent.kind)?;
            for argument in &agent.args {
                write!(f, " {argument}")?;
            }
            if let Some(brief) = agent.prompt_file.as_deref().and_then(named) {
                write!(f, "  [prompt: {brief}]")?;
            }
            if agent.default {
                write!(f, "  [default]")?;
            }
        }
        Ok(())
    }
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// A path's file name, for the human line.
fn named(path: &str) -> Option<&str> {
    Path::new(path).file_name()?.to_str()
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn listing() -> AgentList {
        AgentList {
            default: "reviewer".to_owned(),
            agents: vec![
                AgentLine {
                    name: "cheap".to_owned(),
                    kind: "codex".to_owned(),
                    model: Some("gpt-5-low".to_owned()),
                    effort: None,
                    args: vec![
                        "--no-alt-screen".to_owned(),
                        "--model".to_owned(),
                        "gpt-5-low".to_owned(),
                    ],
                    prompt_file: None,
                    source: "/config/config.toml".to_owned(),
                    default: false,
                },
                AgentLine {
                    name: "reviewer".to_owned(),
                    kind: "claude".to_owned(),
                    model: Some("opus".to_owned()),
                    effort: Some("xhigh".to_owned()),
                    args: vec![
                        "--model".to_owned(),
                        "opus".to_owned(),
                        "--effort".to_owned(),
                        "xhigh".to_owned(),
                    ],
                    prompt_file: Some("/repo/.herdr-agent-tools/review.md".to_owned()),
                    source: "/repo/.herdr-agent-tools/config.toml".to_owned(),
                    default: true,
                },
            ],
        }
    }

    #[test]
    fn the_human_listing_is_one_line_per_agent_and_marks_the_default() {
        // The one command whose result is a list, so the one place a result spans several lines.
        assert_eq!(
            listing().to_string(),
            "cheap (codex) --no-alt-screen --model gpt-5-low\n\
             reviewer (claude) --model opus --effort xhigh  [prompt: review.md]  [default]"
        );
    }

    /// The human line carries the brief's name; the whole path is the wire form's job.
    #[test]
    fn a_brief_is_named_rather_than_pathed_on_the_line_a_person_reads() {
        assert_eq!(named("/repo/.herdr-agent-tools/review.md"), Some("review.md"));
        assert_eq!(named("review.md"), Some("review.md"));
    }

    #[test]
    fn the_wire_listing_names_the_default_once_and_marks_it_on_the_agent() {
        assert_eq!(
            serde_json::to_string(&listing()).unwrap(),
            r#"{"default":"reviewer","agents":[{"name":"cheap","kind":"codex","model":"gpt-5-low","args":["--no-alt-screen","--model","gpt-5-low"],"source":"/config/config.toml","default":false},{"name":"reviewer","kind":"claude","model":"opus","effort":"xhigh","args":["--model","opus","--effort","xhigh"],"prompt_file":"/repo/.herdr-agent-tools/review.md","source":"/repo/.herdr-agent-tools/config.toml","default":true}]}"#
        );
    }
}
