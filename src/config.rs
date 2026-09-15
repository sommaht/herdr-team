//! The config file: its schema, where the two layers are found, and how they are read.

mod discover;
mod resolve;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use getter_methods::Getters;
use serde::Deserialize;
use thiserror::Error;

use crate::cmd::ExitStatus;
use crate::core::{NonEmptyText, Sink};
use crate::harness;

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// The environment variable that overrides where the user's config file is looked for.
const PATH_VARIABLE: &str = "HERDR_TEAM_CONFIG";

/// What a working config file looks like, quoted back when none was found.
const EXAMPLE: &str = "\
default = 'reviewer'

[agents.cc]
kind = 'claude'
args = ['--disallowed-tools', 'AskUserQuestion']

[agents.reviewer]
base = 'cc'
model = 'opus'
effort = 'xhigh'";

// =====================================================================================================================
// The File
// =====================================================================================================================

/// One config file, exactly as it parses.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    /// The agent a bare `spawn` uses.
    ///
    /// Optional per file, but required of the layers together.
    default: Option<String>,
    /// Every agent this file declares, keyed by name.
    #[serde(default)]
    agents: BTreeMap<String, DeclaredAgent>,
    /// What `[agents]` used to be called, declared only so its presence can be reported.
    presets: Option<toml::Value>,
}

/// One agent as a file declares it.
///
/// Every field is optional, `kind` included: a `base` may supply whatever this file does not state.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredAgent {
    /// The agent kind, passed to `agent start --kind` untouched.
    kind: Option<String>,
    /// Another agent to inherit from, by name.
    base: Option<String>,
    /// The model, which the resolved harness spells as flags.
    model: Option<String>,
    /// The reasoning effort, likewise.
    effort: Option<String>,
    /// The flags appended after `--`, as an argument vector.
    #[serde(default)]
    args: Vec<String>,
    /// A file prepended to this agent's first prompt, relative to the declaring file's directory.
    prompt_file: Option<PathBuf>,
}

/// A declared agent bound to the file that declared it.
#[derive(Clone, Debug)]
struct Declared {
    kind: Option<String>,
    base: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    args: Vec<String>,
    /// Absolute by construction, resolved against the declaring file's directory.
    prompt_file: Option<PathBuf>,
    /// The file this came from.
    source: PathBuf,
}

impl Declared {
    /// Binds one file's declaration to that file.
    ///
    /// [`Path::join`] returns an absolute `prompt_file` unchanged, so a caller may write either form.
    fn bind(agent: DeclaredAgent, source: &Path) -> Self {
        let directory = source.parent().unwrap_or_else(|| Path::new("."));
        Self {
            kind: agent.kind,
            base: agent.base,
            model: agent.model,
            effort: agent.effort,
            args: agent.args,
            prompt_file: agent.prompt_file.map(|path| directory.join(path)),
            source: source.to_owned(),
        }
    }
}

/// One config file, read.
#[derive(Debug)]
struct Layer {
    /// Where it was read from.
    path: PathBuf,
    /// The `default` it declared, if it declared one.
    default: Option<String>,
    /// The agents it declared, bound to it.
    agents: BTreeMap<String, Declared>,
}

impl Layer {
    /// Reads one file.
    ///
    /// `Ok(None)` when it is not there; whether it had to exist is the caller's question.
    fn read(path: &Path) -> Result<Option<Self>, ConfigError> {
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ConfigError::Unreadable { path: path.to_owned(), source });
            }
        };

        let file: ConfigFile =
            toml::from_str(&contents).map_err(|source| ConfigError::Malformed { path: path.to_owned(), source })?;
        // Checked after the parse, so a file with both problems still reports its syntax error.
        if file.presets.is_some() {
            return Err(ConfigError::LegacyPresets { path: path.to_owned() });
        }

        Ok(Some(Self {
            default: file.default,
            agents: file
                .agents
                .into_iter()
                .map(|(name, agent)| (name, Declared::bind(agent, path)))
                .collect(),
            path: path.to_owned(),
        }))
    }
}

// =====================================================================================================================
// Agents
// =====================================================================================================================

/// The model and effort an agent starts with, where a caller overrides what the agent declares.
///
/// Default is every field unset, which is the agent's own tuning unchanged.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tuning<'a> {
    /// Replaces the agent's `model` when set.
    pub model: Option<&'a str>,
    /// Replaces the agent's `effort` when set.
    pub effort: Option<&'a str>,
}

/// One agent, with its base chain applied.
#[derive(Clone, Debug, Getters)]
pub struct Agent {
    /// The agent kind, passed to `agent start --kind` untouched.
    kind: String,
    /// The model, which the resolved harness spells as flags.
    model: Option<String>,
    /// The reasoning effort, likewise.
    effort: Option<String>,
    /// The chain's flags, root first.
    args: Vec<String>,
    /// A file prepended to this agent's first prompt, absolute.
    prompt_file: Option<PathBuf>,
    /// The config file that declared this agent.
    source: PathBuf,
}

impl Agent {
    /// Every argument `agent start` receives from the config: the chain's flags, then the ones the
    /// resolved harness spells `model` and `effort` as, with `tuning` standing in for either field
    /// it overrides.
    ///
    /// The order is the precedence: agent CLIs are last-flag-wins, so a first-class field beats an
    /// `args` entry setting the same thing, and `spawn -- <extra>`, appended after this, beats both.
    pub fn agent_args(&self, tuning: Tuning<'_>) -> Vec<String> {
        let mut args = self.args.clone();
        if let Some(harness) = harness::by_kind(&self.kind) {
            args.extend(harness.tuning(
                tuning.model.or(self.model.as_deref()),
                tuning.effort.or(self.effort.as_deref()),
            ));
        }
        args
    }

    /// This agent's brief, read from `prompt_file`.
    ///
    /// A missing or blank file is a refusal, not a warning: the agent must not silently start
    /// without the brief it was configured with.
    pub fn brief(&self) -> Result<Option<NonEmptyText>, ConfigError> {
        let Some(path) = &self.prompt_file else {
            return Ok(None);
        };
        let text = std::fs::read_to_string(path)
            .map_err(|source| ConfigError::UnreadableBrief { path: path.clone(), source })?;
        // RS-002: `BlankTextError` is a unit type carrying nothing; the replacement names the file.
        text.parse()
            .map(Some)
            .map_err(|_| ConfigError::BlankBrief { path: path.clone() })
    }
}

// =====================================================================================================================
// Config
// =====================================================================================================================

/// Every agent, from every layer, resolved.
#[derive(Debug)]
pub struct Config {
    /// The agent a bare `spawn` uses.
    default: String,
    /// Every agent that resolved, keyed by name.
    agents: BTreeMap<String, Agent>,
}

impl Config {
    /// Reads the config, merging a repository layer over the user's.
    ///
    /// `cwd` is where the repository walk starts; `None` means the process directory.
    pub fn load(explicit: Option<&Path>, cwd: Option<&Path>, sink: &Sink) -> Result<Self, ConfigError> {
        let environment = std::env::var_os(PATH_VARIABLE);
        let xdg = std::env::var_os("XDG_CONFIG_HOME");
        let home = std::env::var_os("HOME");
        let user = discover::user(explicit, environment.as_deref(), xdg.as_deref(), home.as_deref())
            .ok_or(ConfigError::NoConfigDir)?;

        let mut layers = Vec::new();
        match Layer::read(user.path())? {
            Some(layer) => layers.push(layer),
            None if user.named() => {
                return Err(ConfigError::Missing { path: user.path().to_owned() });
            }
            None => {}
        }

        if let Some(directory) = working_directory(cwd, sink)
            && let Some(path) = discover::repository(&directory)
            && let Some(layer) = Layer::read(&path)?
        {
            layers.push(layer);
        }

        if layers.is_empty() {
            return Err(ConfigError::Missing { path: user.path().to_owned() });
        }
        Self::of(layers, sink)
    }

    /// The layers, merged and resolved.
    fn of(layers: Vec<Layer>, sink: &Sink) -> Result<Self, ConfigError> {
        let paths = layers
            .iter()
            .map(|layer| layer.path.display().to_string())
            .collect::<Vec<String>>()
            .join(", ");

        let mut default = None;
        let mut declared: BTreeMap<String, Declared> = BTreeMap::new();
        for layer in layers {
            // `extend` replaces by key: a later layer replaces an agent whole, never field-by-field.
            default = layer.default.or(default);
            declared.extend(layer.agents);
        }
        let default = default.ok_or(ConfigError::NoDefault { paths })?;

        let resolved = resolve::resolve(&declared);
        for warning in &resolved.warnings {
            sink.warn(warning);
        }

        Ok(Self { default, agents: resolved.agents })
    }

    /// The agent a bare `spawn` uses.
    pub fn default_name(&self) -> &str {
        &self.default
    }

    /// Looks an agent up, falling back to the config's `default`.
    ///
    /// An agent dropped during resolution is absent from the error's list of names; the warning
    /// saying why was printed when the config was read.
    pub fn resolve(&self, name: Option<&str>) -> Result<&Agent, ConfigError> {
        let name = name.unwrap_or(&self.default);
        self.agents.get(name).ok_or_else(|| ConfigError::UnknownAgent {
            name: name.to_owned(),
            available: self.agents.keys().cloned().collect::<Vec<String>>().join(", "),
        })
    }

    /// Every agent in name order, for the `agents` listing.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Agent)> {
        self.agents.iter().map(|(name, agent)| (name.as_str(), agent))
    }
}

// =====================================================================================================================
// Errors
// =====================================================================================================================

/// Why the config could not be read, or did not hold what was asked for.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// No `--config`, no environment override, and no config or home directory to fall back on.
    #[error("no config directory to look for a config file in; pass --config <PATH>")]
    NoConfigDir,
    /// A path the caller named is not there, or no layer was found at all.
    #[error("no config file at {}; create it with:\n\n{EXAMPLE}", path.display())]
    Missing {
        /// Where it was looked for.
        path: PathBuf,
    },
    /// A file is there but could not be opened.
    #[error("cannot read {}: {source}", path.display())]
    Unreadable {
        /// The file that could not be opened.
        path: PathBuf,
        /// Why not.
        #[source]
        source: std::io::Error,
    },
    /// A file is not valid TOML, or is valid TOML that does not describe agents.
    #[error("{} is not valid TOML: {source}", path.display())]
    Malformed {
        /// The file that would not parse.
        path: PathBuf,
        /// The parse failure, verbatim.
        #[source]
        source: toml::de::Error,
    },
    /// A file still declares the table under its old name.
    #[error("{} declares [presets], which is now [agents]; rename the table", path.display())]
    LegacyPresets {
        /// The file to edit.
        path: PathBuf,
    },
    /// No layer declared which agent a bare `spawn` uses.
    #[error("no `default` agent is declared in {paths}; add `default = '<name>'`")]
    NoDefault {
        /// The files that were read, comma-separated.
        paths: String,
    },
    /// `--agent` named something no layer holds, or whose resolution failed.
    #[error("no agent named {name}; the config holds: {available}")]
    UnknownAgent {
        /// The name that was asked for.
        name: String,
        /// The names the config holds, comma-separated. Names only — never their arguments.
        available: String,
    },
    /// An agent's `prompt_file` is not there or could not be opened.
    #[error("cannot read the prompt file {}: {source}", path.display())]
    UnreadableBrief {
        /// The file the agent points at.
        path: PathBuf,
        /// Why it could not be read.
        #[source]
        source: std::io::Error,
    },
    /// An agent's `prompt_file` holds nothing to deliver.
    #[error("the prompt file {} holds nothing to deliver", path.display())]
    BlankBrief {
        /// The file that is blank.
        path: PathBuf,
    },
}

impl ConfigError {
    /// The exit status this failure maps to.
    pub fn exit_status_hint(&self) -> ExitStatus {
        match self {
            Self::Missing { .. }
            | Self::UnknownAgent { .. }
            | Self::UnreadableBrief { .. }
            | Self::BlankBrief { .. } => ExitStatus::NotFound,
            Self::NoConfigDir
            | Self::Unreadable { .. }
            | Self::Malformed { .. }
            | Self::LegacyPresets { .. }
            | Self::NoDefault { .. } => ExitStatus::Failure,
        }
    }
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// Where the repository walk starts.
///
/// An unreadable process directory costs only the repository layer, so it is warned, not failed.
fn working_directory(cwd: Option<&Path>, sink: &Sink) -> Option<PathBuf> {
    match cwd {
        Some(path) => Some(path.to_owned()),
        None => match std::env::current_dir() {
            Ok(directory) => Some(directory),
            Err(error) => {
                sink.warn(&format!(
                    "cannot read the current directory ({error}), so no repository config was looked for"
                ));
                None
            }
        },
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

[agents.cc]
kind = 'claude'
args = ['--disallowed-tools', 'AskUserQuestion']

[agents.reviewer]
base = 'cc'
model = 'opus'
effort = 'xhigh'
prompt_file = 'review.md'
";

    /// A config file in a temp directory the test owns.
    fn written(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, contents).unwrap();
        (directory, path)
    }

    /// Two layers written to one temp directory tree, with the repository config beneath it.
    fn layered(user: &str, repository: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let user_path = root.path().join("user.toml");
        fs::write(&user_path, user).unwrap();
        let repository_path = root.path().join("project/.herdr-team/config.toml");
        fs::create_dir_all(repository_path.parent().unwrap()).unwrap();
        fs::write(&repository_path, repository).unwrap();
        (root, user_path, repository_path)
    }

    /// `Config::of` over the layers a test wrote, with a sink nothing reads.
    fn merged(paths: &[&PathBuf]) -> Config {
        let layers = paths
            .iter()
            .map(|path| Layer::read(path).unwrap().expect("the test wrote it"))
            .collect();
        Config::of(layers, &Sink::new(crate::core::OutputMode::Human)).unwrap()
    }

    #[test]
    fn an_agents_name_is_its_table_key_and_every_field_but_that_is_optional() {
        let (_directory, path) = written(SAMPLE);

        let layer = Layer::read(&path).unwrap().expect("the file is there");

        assert_eq!(layer.default.as_deref(), Some("reviewer"));
        let cc = &layer.agents["cc"];
        assert_eq!(cc.kind.as_deref(), Some("claude"));
        assert_eq!(cc.base, None);
        assert_eq!(cc.args, ["--disallowed-tools", "AskUserQuestion"]);
        let reviewer = &layer.agents["reviewer"];
        assert_eq!(reviewer.kind, None, "a base supplies it");
        assert_eq!(reviewer.base.as_deref(), Some("cc"));
        assert_eq!(reviewer.model.as_deref(), Some("opus"));
        assert_eq!(reviewer.effort.as_deref(), Some("xhigh"));
        assert!(reviewer.args.is_empty());
    }

    #[test]
    fn a_prompt_file_is_absolute_by_the_time_it_leaves_the_layer() {
        let (directory, path) = written(SAMPLE);

        let layer = Layer::read(&path).unwrap().unwrap();

        assert_eq!(
            layer.agents["reviewer"].prompt_file.as_deref(),
            Some(directory.path().join("review.md").as_path())
        );
        assert_eq!(layer.agents["cc"].prompt_file, None);
    }

    #[test]
    fn every_agent_records_the_file_that_declared_it() {
        let (_directory, path) = written(SAMPLE);

        let layer = Layer::read(&path).unwrap().unwrap();

        assert_eq!(layer.agents["cc"].source, path);
    }

    #[test]
    fn a_file_still_using_the_old_table_name_is_told_what_it_is_now_called() {
        let (_directory, path) = written("default = 'x'\n\n[presets.x]\nkind = 'claude'\n");

        let error = Layer::read(&path).unwrap_err();

        assert_eq!(error.exit_status_hint(), ExitStatus::Failure);
        assert!(error.to_string().contains("[presets]"), "got {error}");
        assert!(error.to_string().contains("[agents]"), "got {error}");
    }

    #[test]
    fn a_misspelled_field_is_a_parse_failure_rather_than_a_field_that_does_nothing() {
        let (_directory, path) = written("[agents.x]\nkind = 'claude'\nmodle = 'opus'\n");

        let error = Layer::read(&path).unwrap_err();

        assert!(matches!(error, ConfigError::Malformed { .. }), "got {error:?}");
    }

    #[test]
    fn malformed_toml_fails_closed_and_names_the_parse_error() {
        let (_directory, path) = written("default = ");

        let error = Layer::read(&path).unwrap_err();

        assert_eq!(error.exit_status_hint(), ExitStatus::Failure);
        assert!(error.to_string().contains("is not valid TOML"), "got {error}");
    }

    #[test]
    fn an_absent_file_is_an_absent_layer_rather_than_an_error() {
        let directory = tempfile::tempdir().unwrap();

        assert!(Layer::read(&directory.path().join("absent.toml")).unwrap().is_none());
    }

    #[test]
    fn a_layer_may_add_agents_without_naming_a_default_or_name_one_without_adding_agents() {
        let (_a, agents_only) = written("[agents.x]\nkind = 'claude'\n");
        let (_b, default_only) = written("default = 'x'\n");

        let agents_only = Layer::read(&agents_only).unwrap().unwrap();
        assert_eq!(agents_only.default, None);
        assert_eq!(agents_only.agents.len(), 1);

        let default_only = Layer::read(&default_only).unwrap().unwrap();
        assert_eq!(default_only.default.as_deref(), Some("x"));
        assert!(default_only.agents.is_empty());
    }

    #[test]
    fn a_repository_agent_replaces_the_users_whole_rather_than_field_by_field() {
        let (_root, user, repository) = layered(
            "default = 'opus'\n\n[agents.opus]\nkind = 'claude'\nmodel = 'opus'\neffort = 'xhigh'\n",
            "[agents.opus]\nkind = 'claude'\nargs = ['--verbose']\n",
        );

        let config = merged(&[&user, &repository]);

        let opus = config.resolve(Some("opus")).unwrap();
        assert_eq!(opus.args, ["--verbose"]);
        assert_eq!(opus.model, None, "not blended in from the user layer");
        assert_eq!(opus.effort, None);
    }

    #[test]
    fn a_repository_agent_may_base_on_one_the_user_declared() {
        let (_root, user, repository) = layered(
            "default = 'cc'\n\n[agents.cc]\nkind = 'claude'\nargs = ['--disallowed-tools', 'AskUserQuestion']\n",
            "[agents.reviewer]\nbase = 'cc'\nmodel = 'opus'\n",
        );

        let config = merged(&[&user, &repository]);

        let reviewer = config.resolve(Some("reviewer")).unwrap();
        assert_eq!(reviewer.kind, "claude");
        assert_eq!(reviewer.args, ["--disallowed-tools", "AskUserQuestion"]);
        assert_eq!(reviewer.model.as_deref(), Some("opus"));
    }

    #[test]
    fn a_repository_default_wins_and_an_absent_one_leaves_the_users_alone() {
        let (_root, user, repository) = layered(
            "default = 'sol'\n\n[agents.sol]\nkind = 'codex'\n",
            "default = 'own'\n\n[agents.own]\nkind = 'claude'\n",
        );
        assert_eq!(merged(&[&user, &repository]).default_name(), "own");

        let (_root, user, repository) = layered(
            "default = 'sol'\n\n[agents.sol]\nkind = 'codex'\n",
            "[agents.own]\nkind = 'claude'\n",
        );
        assert_eq!(merged(&[&user, &repository]).default_name(), "sol");
    }

    #[test]
    fn layers_that_between_them_declare_no_default_are_refused() {
        let (_root, user, repository) = layered("[agents.a]\nkind = 'claude'\n", "[agents.b]\nkind = 'codex'\n");
        let layers = vec![
            Layer::read(&user).unwrap().unwrap(),
            Layer::read(&repository).unwrap().unwrap(),
        ];

        let error = Config::of(layers, &Sink::new(crate::core::OutputMode::Human)).unwrap_err();

        assert_eq!(error.exit_status_hint(), ExitStatus::Failure);
        assert!(error.to_string().contains("default"), "got {error}");
    }

    #[test]
    fn an_unknown_agent_lists_the_names_the_config_holds_and_never_their_args() {
        let (_directory, path) = written(SAMPLE);
        let config = merged(&[&path]);

        let error = config.resolve(Some("nope")).unwrap_err();

        assert_eq!(error.exit_status_hint(), ExitStatus::NotFound);
        assert_eq!(error.to_string(), "no agent named nope; the config holds: cc, reviewer");
        assert!(!error.to_string().contains("--disallowed-tools"));
    }

    #[test]
    fn the_argument_vector_is_the_configs_flags_then_the_ones_the_harness_spells() {
        let (_directory, path) = written(
            "default = 'a'\n\n[agents.a]\nkind = 'claude'\nargs = ['--model', 'sonnet']\nmodel = 'opus'\n\
             effort = 'xhigh'\n",
        );

        let config = merged(&[&path]);

        assert_eq!(
            config.resolve(None).unwrap().agent_args(Tuning::default()),
            ["--model", "sonnet", "--model", "opus", "--effort", "xhigh"]
        );
    }

    #[test]
    fn a_caller_may_replace_either_tuning_field_and_leave_the_other_as_declared() {
        let (_directory, path) = written(
            "default = 'a'\n\n[agents.a]\nkind = 'claude'\nargs = ['--verbose']\nmodel = 'opus'\n\
             effort = 'xhigh'\n",
        );
        let config = merged(&[&path]);
        let agent = config.resolve(None).unwrap();

        assert_eq!(
            agent.agent_args(Tuning { model: Some("sonnet"), effort: None }),
            ["--verbose", "--model", "sonnet", "--effort", "xhigh"]
        );
        assert_eq!(
            agent.agent_args(Tuning { model: None, effort: Some("low") }),
            ["--verbose", "--model", "opus", "--effort", "low"]
        );
    }

    #[test]
    fn a_caller_may_tune_a_field_the_agent_itself_declares_nothing_for() {
        let (_directory, path) = written("default = 'a'\n\n[agents.a]\nkind = 'codex'\n");

        assert_eq!(
            merged(&[&path])
                .resolve(None)
                .unwrap()
                .agent_args(Tuning { model: None, effort: Some("high") }),
            ["-c", "model_reasoning_effort=high"]
        );
    }

    #[test]
    fn an_agent_with_no_prompt_file_has_no_brief_to_read() {
        let (_directory, path) = written("default = 'a'\n\n[agents.a]\nkind = 'claude'\n");

        assert!(merged(&[&path]).resolve(None).unwrap().brief().unwrap().is_none());
    }

    #[test]
    fn a_brief_is_read_from_the_path_the_declaring_file_resolved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "default = 'a'\n\n[agents.a]\nkind = 'claude'\nprompt_file = 'brief.md'\n",
        )
        .unwrap();
        fs::write(directory.path().join("brief.md"), "You review Rust.").unwrap();

        let brief = merged(&[&path]).resolve(None).unwrap().brief().unwrap();

        assert_eq!(brief.unwrap().to_string(), "You review Rust.");
    }

    #[test]
    fn a_prompt_file_that_is_missing_or_blank_is_a_refusal() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "default = 'a'\n\n[agents.a]\nkind = 'claude'\nprompt_file = 'brief.md'\n",
        )
        .unwrap();

        let config = merged(&[&path]);
        let error = config.resolve(None).unwrap().brief().unwrap_err();
        assert_eq!(error.exit_status_hint(), ExitStatus::NotFound);

        fs::write(directory.path().join("brief.md"), "   \n\n").unwrap();
        let error = config.resolve(None).unwrap().brief().unwrap_err();
        assert_eq!(error.exit_status_hint(), ExitStatus::NotFound);
        assert!(error.to_string().contains("nothing to deliver"), "got {error}");
    }
}
