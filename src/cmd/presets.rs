//! `presets` — list what the preset file holds.

use std::fmt::Display;
use std::path::PathBuf;

use clap::Args;
use serde::Serialize;

use crate::cmd::{AsExitStatus, Cmd, ExitStatus};
use crate::config::{ConfigError, Presets};
use crate::core::Sink;

// =====================================================================================================================
// Presets Args
// =====================================================================================================================

/// List the presets the config file holds, marking the default.
///
/// Touches herdr not at all, so it answers with no server running — which is what is wanted when
/// the config file itself is what is being debugged.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  \
    herdr-agent-tools presets\n  \
    herdr-agent-tools presets --config ./config.toml --json")]
pub struct PresetsArgs {
    /// Read this preset file instead of the one in the config directory.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

impl Cmd for PresetsArgs {
    type Ok = PresetList;
    type Err = ConfigError;

    fn execute(self, _sink: &Sink) -> Result<Self::Ok, Self::Err> {
        let presets = Presets::load(self.config.as_deref())?;
        Ok(PresetList {
            default: presets.default_name().to_owned(),
            presets: presets
                .iter()
                .map(|(name, preset)| PresetLine {
                    name: name.to_owned(),
                    kind: preset.kind().to_owned(),
                    args: preset.args().to_vec(),
                    default: name == presets.default_name(),
                })
                .collect(),
        })
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

/// Every preset the file holds, in name order.
#[derive(Debug, Serialize)]
pub struct PresetList {
    /// The name of the preset a bare `spawn` uses.
    default: String,
    /// One entry per preset.
    presets: Vec<PresetLine>,
}

/// One preset as the listing reports it.
#[derive(Debug, Serialize)]
struct PresetLine {
    /// The preset's name — its table key in the file.
    name: String,
    /// The agent kind it starts.
    kind: String,
    /// The flags it appends. Safe to render here: the ban is on error messages and logs, and a
    /// listing of the config file is the one place these are the answer.
    args: Vec<String>,
    /// Whether this is the file's `default`.
    default: bool,
}

/// One line per preset, which makes this the one result that spans several lines.
///
/// The sink's usual contract is one line per value; a listing has nothing else it could honestly
/// be, and the `--json` form is one object either way.
impl Display for PresetList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, preset) in self.presets.iter().enumerate() {
            if index > 0 {
                writeln!(f)?;
            }
            write!(f, "{} ({})", preset.name, preset.kind)?;
            for argument in &preset.args {
                write!(f, " {argument}")?;
            }
            if preset.default {
                write!(f, "  [default]")?;
            }
        }
        Ok(())
    }
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn listing() -> PresetList {
        PresetList {
            default: "reviewer".to_owned(),
            presets: vec![
                PresetLine {
                    name: "cheap".to_owned(),
                    kind: "codex".to_owned(),
                    args: vec!["--model".to_owned(), "gpt-5-low".to_owned()],
                    default: false,
                },
                PresetLine {
                    name: "reviewer".to_owned(),
                    kind: "claude".to_owned(),
                    args: vec!["--model".to_owned(), "opus".to_owned()],
                    default: true,
                },
            ],
        }
    }

    #[test]
    fn the_human_listing_is_one_line_per_preset_and_marks_the_default() {
        // The one command whose result is a list, so the one place a result spans several lines.
        assert_eq!(
            listing().to_string(),
            "cheap (codex) --model gpt-5-low\n\
             reviewer (claude) --model opus  [default]"
        );
    }

    #[test]
    fn the_wire_listing_names_the_default_once_and_marks_it_on_the_preset() {
        assert_eq!(
            serde_json::to_string(&listing()).unwrap(),
            r#"{"default":"reviewer","presets":[{"name":"cheap","kind":"codex","args":["--model","gpt-5-low"],"default":false},{"name":"reviewer","kind":"claude","args":["--model","opus"],"default":true}]}"#
        );
    }
}
