//! The config file: its schema, where the two layers are found, and how they are read.
//!
//! The only disk I/O in the crate, which is the boundary this module names — including the files a
//! config *points at*, since `prompt_file` is a config field and the file it names is part of the
//! config. Nothing here is loaded eagerly: `prompt` never reads a config, and a malformed file must
//! not break it.
//!
//! Three vocabularies, deliberately three types. [`DeclaredAgent`] is what one file's TOML holds,
//! where every field is optional because a `base` may supply any of them. [`Declared`] is that bound
//! to the file it came from, so a relative `prompt_file` and the source the listing reports both
//! survive a merge. [`Agent`] is what a caller gets: a base chain already applied, a `kind` that
//! exists, and nothing left to look up.

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
///
/// `deny_unknown_fields` on purpose. A config is small and hand-written, and a `modle = 'opus'` that
/// parsed to nothing would start the wrong model without saying so. The one unknown key this file
/// *names* is `presets`, so its rename can be reported as a rename rather than as a typo.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    /// The agent a bare `spawn` uses.
    ///
    /// Optional per file — a repository layer may add agents without changing which one is default —
    /// but required of the layers together.
    default: Option<String>,
    /// Every agent this file declares, keyed by name.
    ///
    /// A `BTreeMap` so the listing, an error's list of available names, and the merge all come out in
    /// a stable order without a sort at each site.
    #[serde(default)]
    agents: BTreeMap<String, DeclaredAgent>,
    /// What `[agents]` used to be called, declared only so its presence can be reported.
    presets: Option<toml::Value>,
}

/// One agent as a file declares it.
///
/// Every field is optional, `kind` included: an agent with a `base` inherits whatever it does not
/// state, so which fields are actually required is a question about the *resolved* agent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredAgent {
    /// The agent kind, passed to `agent start --kind` untouched.
    ///
    /// A plain `String` deliberately. herdr answers `unsupported_agent_kind` from its own
    /// compile-time list, that list is not published anywhere machine-readable, and the number of
    /// kinds it recognizes grows — so restating it here would drift.
    kind: Option<String>,
    /// Another agent to inherit from, by name.
    base: Option<String>,
    /// The model, which the resolved harness spells as flags.
    model: Option<String>,
    /// The reasoning effort, likewise.
    effort: Option<String>,
    /// The flags appended after `--`, as an argument vector.
    ///
    /// An array only. A string form would have to be split into shell words, which means
    /// reimplementing shell word-splitting for a value handed straight to `Command`; herdr takes the
    /// agent's arguments as an argument vector, so nothing here needs shell quoting.
    #[serde(default)]
    args: Vec<String>,
    /// A file prepended to this agent's first prompt, relative to the declaring file's directory.
    prompt_file: Option<PathBuf>,
}

/// A declared agent bound to the file that declared it.
///
/// The binding is what lets `prompt_file` be resolved once and `source` be reported after a merge has
/// mixed two files' agents into one map.
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
    /// `Ok(None)` when it is not there, which is not a failure here: whether a given path *had* to
    /// exist is the caller's question, and a repository layer may be the whole config.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Unreadable`] when the file cannot be opened, [`ConfigError::Malformed`] when it
    /// is not valid TOML or holds a key this build does not know, and [`ConfigError::LegacyPresets`]
    /// when it still uses the table's old name.
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
        // Checked after the parse rather than instead of it, so a file with both a `[presets]` table
        // and a syntax error reports the syntax error it would report either way.
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

/// One agent, with its base chain applied.
///
/// The fields stay private and [`resolve`] constructs this anyway — a child module may read its
/// ancestors' private items, which is exactly the visibility wanted here: `resolve` is the only thing
/// that may say what "resolved" means, and everything outside `config` reads through the accessors.
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
    /// The config file that declared this agent, which is the question two layers create.
    source: PathBuf,
}

impl Agent {
    /// Every argument `agent start` receives from the config: the chain's flags, then the ones the
    /// resolved harness spells `model` and `effort` as.
    ///
    /// The order is the precedence. Agent CLIs are last-flag-wins, so a first-class field beats an
    /// `args` entry that sets the same thing — and `spawn -- <extra>`, appended after this, beats
    /// both.
    ///
    /// A kind with no harness contributes nothing here, which is safe because [`resolve`] has already
    /// dropped any agent that set `model` or `effort` under one.
    pub fn agent_args(&self) -> Vec<String> {
        let mut args = self.args.clone();
        if let Some(harness) = harness::by_kind(&self.kind) {
            args.extend(harness.tuning(self.model.as_deref(), self.effort.as_deref()));
        }
        args
    }

    /// This agent's brief, read from `prompt_file`.
    ///
    /// Read here rather than by the caller because `prompt_file` is a config field and the file it
    /// names is part of the config, which keeps every disk read inside the module that names disk I/O
    /// as its boundary.
    ///
    /// # Errors
    ///
    /// [`ConfigError::UnreadableBrief`] when the file is not there or cannot be opened, and
    /// [`ConfigError::BlankBrief`] when it holds nothing to deliver. Both are refusals rather than
    /// warnings: an agent that silently starts without the brief it was configured with is the
    /// failure this exists to prevent, and a caller learns it before anything has been created.
    pub fn brief(&self) -> Result<Option<NonEmptyText>, ConfigError> {
        let Some(path) = &self.prompt_file else {
            return Ok(None);
        };
        let text = std::fs::read_to_string(path)
            .map_err(|source| ConfigError::UnreadableBrief { path: path.clone(), source })?;
        // RS-002: the source is discarded because it carries nothing — `BlankTextError` is a unit
        // type, deliberately, since the text it rejected is a prompt. This says strictly more: which
        // file was blank, which is the half a reader has to act on.
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
    /// `cwd` is where the repository walk starts — `spawn`'s `--cwd`, so the config found is the one
    /// belonging to the tree the agent will work in. `None` means the process directory.
    ///
    /// # Errors
    ///
    /// [`ConfigError::NoConfigDir`] when there is nowhere to look, [`ConfigError::Missing`] when a
    /// path the caller named is not there or no layer was found at all, [`ConfigError::NoDefault`]
    /// when the layers between them declare none, and whatever [`Layer::read`] returned.
    pub fn load(explicit: Option<&Path>, cwd: Option<&Path>, sink: &Sink) -> Result<Self, ConfigError> {
        let environment = std::env::var_os(PATH_VARIABLE);
        let xdg = std::env::var_os("XDG_CONFIG_HOME");
        let home = std::env::var_os("HOME");
        let user = discover::user(explicit, environment.as_deref(), xdg.as_deref(), home.as_deref())
            .ok_or(ConfigError::NoConfigDir)?;

        let mut layers = Vec::new();
        match Layer::read(user.path())? {
            Some(layer) => layers.push(layer),
            // A caller who named a path is asking about that path. The config directory's default
            // location is only a place to look, and a repository layer may be the whole config.
            None if user.named() => {
                return Err(ConfigError::Missing { path: user.path().to_owned() });
            }
            None => {}
        }

        // One chain rather than three nestings: each link is a step of the same lookup, and a
        // repository layer exists only when every one of them answers.
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
    ///
    /// Split from [`load`](Self::load) so the merge rule and the `default` requirement are testable
    /// against files a test wrote, with no environment and no discovery.
    ///
    /// # Errors
    ///
    /// [`ConfigError::NoDefault`] when no layer declared one.
    fn of(layers: Vec<Layer>, sink: &Sink) -> Result<Self, ConfigError> {
        let paths = layers
            .iter()
            .map(|layer| layer.path.display().to_string())
            .collect::<Vec<String>>()
            .join(", ");

        let mut default = None;
        let mut declared: BTreeMap<String, Declared> = BTreeMap::new();
        for layer in layers {
            // `extend` replaces by key, which is the whole-agent replacement the design calls for: a
            // repository agent that wants the user's flags says `base`, rather than having its fields
            // blended with a file it cannot see.
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
    /// # Errors
    ///
    /// [`ConfigError::UnknownAgent`], listing the names that resolved — names only, because an
    /// agent's arguments must not reach an error message. An agent dropped during resolution is
    /// absent from that list, and the warning saying why was printed when the config was read.
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
    /// A path the caller named is not there, or no layer was found at all. The message carries a
    /// working example, since the usual cause is that it was never written.
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
    ///
    /// A plain method rather than an [`AsExitStatus`](crate::cmd::AsExitStatus) impl, because two
    /// commands wrap this in enums of their own and both delegate here — one owner for the mapping,
    /// reachable from either.
    ///
    /// The three brief failures share one code with the two lookup failures: they are one class from
    /// a caller's side — you named something and it is not there — and one code is what a caller can
    /// branch on.
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
/// `None` only when the caller named no directory and the process's own cannot be read — which is not
/// a config failure, because the caller did not ask about a directory. It costs the repository layer,
/// so it is said rather than swallowed.
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
        // The name is the key, so a duplicate name is inexpressible rather than last-one-wins, and
        // there is no `name` field that could disagree with it.
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

    /// A relative `prompt_file` is resolved as the layer is read, against the file that declared it.
    ///
    /// Done here rather than at spawn because a merge mixes two files' agents into one map, after
    /// which nothing downstream can say which directory a path was written relative to.
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

    /// The rename is reported as a rename, not as a typo.
    #[test]
    fn a_file_still_using_the_old_table_name_is_told_what_it_is_now_called() {
        let (_directory, path) = written("default = 'x'\n\n[presets.x]\nkind = 'claude'\n");

        let error = Layer::read(&path).unwrap_err();

        assert_eq!(error.exit_status_hint(), ExitStatus::Failure);
        assert!(error.to_string().contains("[presets]"), "got {error}");
        assert!(error.to_string().contains("[agents]"), "got {error}");
    }

    /// A hand-written file is small enough that a silently-ignored key is worse than a refusal.
    #[test]
    fn a_misspelled_field_is_a_parse_failure_rather_than_a_field_that_does_nothing() {
        let (_directory, path) = written("[agents.x]\nkind = 'claude'\nmodle = 'opus'\n");

        let error = Layer::read(&path).unwrap_err();

        assert!(matches!(error, ConfigError::Malformed { .. }), "got {error:?}");
    }

    #[test]
    fn malformed_toml_fails_closed_and_names_the_parse_error() {
        // The shell version's reader failed open: malformed input yielded an empty document and
        // exit 0, so one typo surfaced as "agent not found" and sent you hunting the wrong file.
        let (_directory, path) = written("default = ");

        let error = Layer::read(&path).unwrap_err();

        assert_eq!(error.exit_status_hint(), ExitStatus::Failure);
        assert!(error.to_string().contains("is not valid TOML"), "got {error}");
    }

    /// A file that is not there is not a failure *here*: whether it had to exist is the caller's
    /// question, and a repository layer may be the whole config.
    #[test]
    fn an_absent_file_is_an_absent_layer_rather_than_an_error() {
        let directory = tempfile::tempdir().unwrap();

        assert!(Layer::read(&directory.path().join("absent.toml")).unwrap().is_none());
    }

    /// Neither layer has to declare `default`, and neither has to declare agents.
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
        // `base` is how a repository inherits; a second, implicit blend across files would make the
        // effective config unreadable from either file alone.
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
        // Names only: an agent's arguments must not reach an error message.
        let (_directory, path) = written(SAMPLE);
        let config = merged(&[&path]);

        let error = config.resolve(Some("nope")).unwrap_err();

        assert_eq!(error.exit_status_hint(), ExitStatus::NotFound);
        assert_eq!(error.to_string(), "no agent named nope; the config holds: cc, reviewer");
        assert!(!error.to_string().contains("--disallowed-tools"));
    }

    /// The order is the precedence: a first-class field beats an `args` entry setting the same thing.
    #[test]
    fn the_argument_vector_is_the_configs_flags_then_the_ones_the_harness_spells() {
        let (_directory, path) = written(
            "default = 'a'\n\n[agents.a]\nkind = 'claude'\nargs = ['--model', 'sonnet']\nmodel = 'opus'\n\
             effort = 'xhigh'\n",
        );

        let config = merged(&[&path]);

        assert_eq!(
            config.resolve(None).unwrap().agent_args(),
            ["--model", "sonnet", "--model", "opus", "--effort", "xhigh"]
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

    /// A refusal rather than a drop, and deliberately: an agent that silently starts without the
    /// brief it was configured with is the failure this is here to prevent.
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
