//! The preset file: its schema, where it is found, and how it is read.
//!
//! The only disk I/O in the crate, which is the boundary this module names. Nothing here is loaded
//! eagerly: `prompt` never reads a preset, and a malformed file must not break it.

mod discover;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use getter_methods::Getters;
use serde::Deserialize;
use thiserror::Error;

use crate::cmd::ExitStatus;

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// The environment variable that overrides where the preset file is looked for.
const PATH_VARIABLE: &str = "HERDR_AGENT_TOOLS_CONFIG";

/// What a working preset file looks like, quoted back when none was found.
const EXAMPLE: &str = "\
default = 'reviewer'

[presets.reviewer]
kind = 'claude'
args = ['--model', 'opus']

[presets.cheap]
kind = 'codex'
args = ['--model', 'gpt-5-low', '--no-alt-screen']";

// =====================================================================================================================
// Presets
// =====================================================================================================================

/// The whole preset file.
#[derive(Debug, Deserialize)]
pub struct Presets {
    /// The preset `spawn` uses when `--preset` is absent. Required: a file with no default is a
    /// file that cannot answer the common case, and failing on it beats a confusing refusal later.
    default: String,
    /// Every preset, keyed by name.
    ///
    /// A `BTreeMap` so the `presets` listing and an error's list of available names both come out
    /// in a stable order without a sort at each site.
    presets: BTreeMap<String, Preset>,
}

/// One preset: which agent to start, and the flags it usually gets.
#[derive(Debug, Deserialize, Getters)]
pub struct Preset {
    /// The agent kind, passed to `agent start --kind` untouched.
    ///
    /// A plain `String` deliberately. herdr answers `unsupported_agent_kind` from its own
    /// compile-time list, that list is not published anywhere machine-readable, and the number of
    /// kinds it recognizes grows — so restating it here would drift.
    kind: String,
    /// The flags appended after `--`, as an argument vector.
    ///
    /// An array only. A string form would have to be split into shell words, which means
    /// reimplementing shell word-splitting for a value handed straight to `Command`; herdr takes
    /// the agent's arguments as an argument vector, so nothing here needs shell quoting.
    #[serde(default)]
    args: Vec<String>,
}

impl Presets {
    /// Reads the preset file, looking where [`discover`] says to look.
    ///
    /// # Errors
    ///
    /// [`ConfigError::NoConfigDir`] when there is nowhere to look, [`ConfigError::Missing`] when
    /// the file is not there, [`ConfigError::Unreadable`] when it cannot be opened, and
    /// [`ConfigError::Malformed`] when it is not valid TOML or is missing `default`.
    pub fn load(explicit: Option<&Path>) -> Result<Self, ConfigError> {
        let environment = std::env::var_os(PATH_VARIABLE);
        let xdg = std::env::var_os("XDG_CONFIG_HOME");
        let home = std::env::var_os("HOME");
        let path = discover::user(explicit, environment.as_deref(), xdg.as_deref(), home.as_deref())
            .ok_or(ConfigError::NoConfigDir)?
            .path()
            .to_owned();

        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(ConfigError::Missing { path });
            }
            Err(source) => return Err(ConfigError::Unreadable { path, source }),
        };

        toml::from_str(&contents).map_err(|source| ConfigError::Malformed { path, source })
    }

    /// The preset a bare `spawn` uses.
    pub fn default_name(&self) -> &str {
        &self.default
    }

    /// Looks a preset up, falling back to the file's `default`.
    ///
    /// # Errors
    ///
    /// [`ConfigError::UnknownPreset`], listing the names the file holds — names only, because a
    /// preset's arguments must not reach an error message.
    pub fn resolve(&self, name: Option<&str>) -> Result<&Preset, ConfigError> {
        let name = name.unwrap_or(&self.default);
        self.presets.get(name).ok_or_else(|| ConfigError::UnknownPreset {
            name: name.to_owned(),
            available: self.presets.keys().cloned().collect::<Vec<String>>().join(", "),
        })
    }

    /// Every preset in name order, for the `presets` listing.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Preset)> {
        self.presets.iter().map(|(name, preset)| (name.as_str(), preset))
    }
}

// =====================================================================================================================
// Errors
// =====================================================================================================================

/// Why the preset file could not be read or did not hold what was asked for.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// No `--config`, no environment override, and no config or home directory to fall back on.
    #[error("no config directory to look for a preset file in; pass --config <PATH>")]
    NoConfigDir,
    /// The file is not there. The message carries a working example, since the usual cause is that
    /// it was never written.
    #[error("no preset file at {}; create it with:\n\n{EXAMPLE}", path.display())]
    Missing {
        /// Where it was looked for.
        path: PathBuf,
    },
    /// The file is there but could not be opened.
    #[error("cannot read {}: {source}", path.display())]
    Unreadable {
        /// The file that could not be opened.
        path: PathBuf,
        /// Why not.
        #[source]
        source: std::io::Error,
    },
    /// The file is not valid TOML, or is valid TOML that does not describe presets.
    #[error("{} is not valid TOML: {source}", path.display())]
    Malformed {
        /// The file that would not parse.
        path: PathBuf,
        /// The parse failure, verbatim.
        #[source]
        source: toml::de::Error,
    },
    /// `--preset` named something the file does not hold.
    #[error("no preset named {name}; the file holds: {available}")]
    UnknownPreset {
        /// The name that was asked for.
        name: String,
        /// The names the file holds, comma-separated. Names only — never their arguments.
        available: String,
    },
}

impl ConfigError {
    /// The exit status this failure maps to.
    ///
    /// A plain method rather than an [`AsExitStatus`](crate::cmd::AsExitStatus) impl, because two
    /// commands wrap this in enums of their own and both delegate here — one owner for the mapping,
    /// reachable from either.
    pub fn exit_status_hint(&self) -> ExitStatus {
        match self {
            Self::Missing { .. } | Self::UnknownPreset { .. } => ExitStatus::NotFound,
            Self::NoConfigDir | Self::Unreadable { .. } | Self::Malformed { .. } => ExitStatus::Failure,
        }
    }
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const SAMPLE: &str = "\
default = 'reviewer'

[presets.reviewer]
kind = 'claude'
args = ['--model', 'opus']

[presets.cheap]
kind = 'codex'
";

    /// A preset file in a temp directory the test owns.
    fn written(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, contents).unwrap();
        (directory, path)
    }

    #[test]
    fn a_presets_name_is_its_table_key_and_args_default_to_none() {
        // The name is the key, so a duplicate name is inexpressible rather than last-one-wins, and
        // there is no `name` field that could disagree with it.
        let (_directory, path) = written(SAMPLE);

        let presets = Presets::load(Some(&path)).unwrap();

        assert_eq!(presets.default_name(), "reviewer");
        assert_eq!(presets.resolve(None).unwrap().kind(), "claude");
        assert_eq!(presets.resolve(None).unwrap().args(), &["--model", "opus"]);
        assert_eq!(presets.resolve(Some("cheap")).unwrap().kind(), "codex");
        assert!(presets.resolve(Some("cheap")).unwrap().args().is_empty());
    }

    #[test]
    fn an_unknown_preset_lists_the_names_the_file_holds_and_never_their_args() {
        // Names only: a preset's arguments must not reach an error message.
        let (_directory, path) = written(SAMPLE);
        let presets = Presets::load(Some(&path)).unwrap();

        let error = presets.resolve(Some("opus")).unwrap_err();

        assert_eq!(error.exit_status_hint(), ExitStatus::NotFound);
        assert_eq!(
            error.to_string(),
            "no preset named opus; the file holds: cheap, reviewer"
        );
        assert!(
            !error.to_string().contains("--model"),
            "a preset's args must not be logged"
        );
    }

    #[test]
    fn a_file_with_no_default_is_a_parse_failure_rather_than_a_silent_empty_document() {
        let (_directory, path) = written("[presets.only]\nkind = 'claude'\n");

        let error = Presets::load(Some(&path)).unwrap_err();

        assert!(matches!(error, ConfigError::Malformed { .. }), "got {error:?}");
        assert!(error.to_string().contains("missing field `default`"));
    }

    #[test]
    fn malformed_toml_fails_closed_and_names_the_parse_error() {
        // The shell version's reader failed open: malformed input yielded an empty document and
        // exit 0, so one typo surfaced as "preset not found" and sent you hunting the wrong file.
        let (_directory, path) = written("default = ");

        let error = Presets::load(Some(&path)).unwrap_err();

        assert_eq!(error.exit_status_hint(), ExitStatus::Failure);
        assert!(error.to_string().contains("is not valid TOML"), "got {error}");
    }

    #[test]
    fn a_missing_file_reports_where_it_looked_and_what_to_put_there() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("absent.toml");

        let error = Presets::load(Some(&path)).unwrap_err();

        assert_eq!(error.exit_status_hint(), ExitStatus::NotFound);
        assert!(
            error.to_string().contains("[presets.reviewer]"),
            "the message carries the example"
        );
    }
}
