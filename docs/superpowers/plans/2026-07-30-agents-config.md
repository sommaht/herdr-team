# Agents, layered config, and per-agent briefs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename presets to agents, give each agent `model`, `effort`, `base`, and `prompt_file`
fields, and merge a repository-local config over the user's.

**Architecture:** `src/config.rs` becomes a three-file module. Vocabulary is layered and each layer
is a distinct type: `DeclaredAgent` is one file's TOML, `Declared` is that bound to the file it came
from, `Agent` is a resolved agent with its base chain applied. Discovery and base resolution are pure
functions in submodules; the disk read and the errors stay in the module root. How `model` and
`effort` become command-line flags is a new required method on `AgentHarness`, because that is
knowledge about a specific CLI.

**Tech Stack:** Rust 2024, clap 4 derive, serde + toml, thiserror, getter-methods, tempfile (tests).

**Spec:** `docs/superpowers/specs/2026-07-30-agents-config-design.md`

---

## Read first

- `docs/STYLE-GUIDE.md` — house conventions. The ones this plan leans on:
  - Section headers: `// ====…` heavyweight for major sections, padded to `max_width` (129 cols).
    Standard skeleton: `Constants`, one heavyweight header per domain group, `Helpers`, `Tests`.
  - **No `println!`/`eprintln!` outside `core::sink`.** Diagnostics go through `Sink::warn`.
  - **Never log a prompt, an agent's arguments, or terminal content.** This now covers `model` and
    `effort`: they *become* arguments, so a warning names the field, never the value.
  - Tests are inline `#[cfg(test)] mod tests` under a `Tests` header. Automated tests never invoke
    herdr. Tests own their temp directories.
  - Wire forms and argument vectors are pinned by exact-string tests.
- `Cargo.toml` denies `mod_module_files`, so a module with submodules is `src/config.rs` plus
  `src/config/*.rs` — never `src/config/mod.rs`.

## Verification suite

Run after each task's implementation step:

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
```

Task 11 adds `cargo install --path . --force` and a live rehearsal.

## File structure

| File | Change | Responsibility |
| --- | --- | --- |
| `src/harness.rs` | Modify | `tuning` added to the trait |
| `src/harness/claude.rs` | Modify | `--model` / `--effort` |
| `src/harness/codex.rs` | Modify | `--model` / `-c model_reasoning_effort=` |
| `src/config/discover.rs` | Create | Both layers' path search |
| `src/config/resolve.rs` | Create | Base chains, and the warning per agent that fails one |
| `src/config.rs` | Rewrite | Schema, layer read, merge, `Agent`, `Config`, errors |
| `src/cmd/agents.rs` | Create (from `presets.rs`) | The listing |
| `src/cmd/presets.rs` | Delete | Replaced by `agents.rs` |
| `src/cmd.rs` | Modify | `mod agents;` / `pub use agents::AgentsArgs;` |
| `src/main.rs` | Modify | `Agents` subcommand, long-about wording |
| `src/cmd/spawn.rs` | Modify | `--agent`, the renamed-flag refusal, tuning flags, the brief |
| `src/cmd/prime.rs` | Modify | Brief wording and the regenerated table |
| `README.md`, `CLAUDE.md`, `AGENTS.md`, `docs/STYLE-GUIDE.md` | Modify | The rename reaches the docs |

---

### Task 1: `AgentHarness::tuning`

**Files:**
- Modify: `src/harness.rs` (the trait, around line 121-159)
- Modify: `src/harness/claude.rs`
- Modify: `src/harness/codex.rs`

- [ ] **Step 1: Write the failing tests**

In `src/harness/claude.rs`, inside `mod tests`, add:

```rust
    #[test]
    fn model_and_effort_are_both_plain_flags_on_this_cli() {
        assert_eq!(
            ClaudeCode.tuning(Some("opus"), Some("xhigh")),
            ["--model", "opus", "--effort", "xhigh"]
        );
    }

    #[test]
    fn either_half_stands_alone_and_neither_yields_nothing() {
        assert_eq!(ClaudeCode.tuning(Some("opus"), None), ["--model", "opus"]);
        assert_eq!(ClaudeCode.tuning(None, Some("xhigh")), ["--effort", "xhigh"]);
        assert!(ClaudeCode.tuning(None, None).is_empty());
    }
```

In `src/harness/codex.rs`, inside `mod tests`, add:

```rust
    /// The reason this is a method rather than one shared spelling: Codex has no effort flag, so the
    /// same field reaches it as a config override.
    #[test]
    fn effort_reaches_codex_as_a_config_override_rather_than_a_flag() {
        assert_eq!(
            Codex.tuning(Some("gpt-5.6-sol"), Some("xhigh")),
            ["--model", "gpt-5.6-sol", "-c", "model_reasoning_effort=xhigh"]
        );
        assert_eq!(Codex.tuning(None, Some("medium")), ["-c", "model_reasoning_effort=medium"]);
        assert!(Codex.tuning(None, None).is_empty());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-targets harness`
Expected: FAIL, `no method named 'tuning' found`.

- [ ] **Step 3: Add the trait method**

In `src/harness.rs`, inside `pub trait AgentHarness`, after `fn hook(...)` and before `fn probe(...)`:

```rust
    /// The flags this harness's CLI expresses `model` and `effort` as.
    ///
    /// Required rather than defaulted, for the reason [`hook`](Self::hook) is: a harness added later
    /// must not inherit a spelling nobody checked against its CLI. The two fields are independent —
    /// either may be set without the other — and an empty vector is the answer only when neither is.
    ///
    /// A kind absent from [`HARNESSES`] has no answer at all, which is why the config layer drops an
    /// agent that asks for these under one rather than starting it without them.
    fn tuning(&self, model: Option<&str>, effort: Option<&str>) -> Vec<String>;
```

Update the trait's own doc comment: the sentence beginning *"The three declarations — kind, marker,
and hook envelope — are required"* becomes:

```rust
/// The judgment methods have defaults, so an impl states only what makes it different: the deferred
/// placeholder-vs-typed-text work overrides [`composer_occupied`](Self::composer_occupied), and a
/// harness that renders its composer somewhere else overrides [`probe`](Self::probe). The four
/// *declarations* — kind, marker, hook envelope, and how it spells model and effort — are required,
/// because each is a claim about a specific host that someone has to make deliberately.
```

- [ ] **Step 4: Implement it for both harnesses**

In `src/harness/claude.rs`, inside `impl AgentHarness for ClaudeCode`:

```rust
    /// Both are plain flags: `--model opus --effort xhigh`.
    fn tuning(&self, model: Option<&str>, effort: Option<&str>) -> Vec<String> {
        let mut flags = Vec::new();
        if let Some(model) = model {
            flags.extend(["--model".to_owned(), model.to_owned()]);
        }
        if let Some(effort) = effort {
            flags.extend(["--effort".to_owned(), effort.to_owned()]);
        }
        flags
    }
```

In `src/harness/codex.rs`, inside `impl AgentHarness for Codex`:

```rust
    /// `--model` is a flag; effort is not, and reaches Codex as a config override —
    /// `-c model_reasoning_effort=xhigh`.
    fn tuning(&self, model: Option<&str>, effort: Option<&str>) -> Vec<String> {
        let mut flags = Vec::new();
        if let Some(model) = model {
            flags.extend(["--model".to_owned(), model.to_owned()]);
        }
        if let Some(effort) = effort {
            flags.extend(["-c".to_owned(), format!("model_reasoning_effort={effort}")]);
        }
        flags
    }
```

- [ ] **Step 5: Run the verification suite**

Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add src/harness.rs src/harness/claude.rs src/harness/codex.rs
git commit -m "feat(harness): let each harness spell model and effort as its own CLI does"
```

---

### Task 2: `config/discover.rs`

**Files:**
- Create: `src/config/discover.rs`
- Modify: `src/config.rs` (add `mod discover;`, delete the old `discover` fn and its test)

- [ ] **Step 1: Write the new module with its tests, implementation stubbed**

Create `src/config/discover.rs` with the tests below and a body of `todo!()` for both public
functions, so the tests compile and fail.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --all-targets discover`
Expected: FAIL — `not yet implemented`.

- [ ] **Step 3: Write the module**

Create `src/config/discover.rs`:

```rust
//! Where the two config layers are looked for.
//!
//! The user layer's search is pure, with the environment passed in, so its order is testable without
//! setting process variables that leak between tests. The repository layer's is not — deciding
//! whether a file exists is the question — so it takes a directory and answers from disk.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

// =====================================================================================================================
// Constants
// =====================================================================================================================

/// Where the user's file sits under a config directory.
///
/// This tool's own directory, not a second file inside herdr's: a public tool should not squat a
/// filename in another project's config directory, where it would break the day that project claims
/// the name.
const RELATIVE_PATH: &str = "herdr-agent-tools/config.toml";

/// Where a repository carries its own, relative to any directory in it.
///
/// A directory rather than a dotfile, so an agent's `prompt_file` has somewhere to live beside the
/// config that names it.
const REPOSITORY_PATH: &str = ".herdr-agent-tools/config.toml";

// =====================================================================================================================
// The User Layer
// =====================================================================================================================

/// The user layer's path, and whether the caller named it.
///
/// The distinction is the whole point: a caller who passed `--config` is asking about *that* path, so
/// its absence is a failure. The config directory's default location is only a place to look, and a
/// repository layer may be the entire config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserPath {
    /// `--config` or the environment override.
    Named(PathBuf),
    /// The config directory's default location.
    Default(PathBuf),
}

impl UserPath {
    /// The path itself.
    pub fn path(&self) -> &Path {
        match self {
            Self::Named(path) | Self::Default(path) => path,
        }
    }

    /// Whether the caller named it, which decides whether its absence is a failure.
    pub fn named(&self) -> bool {
        matches!(self, Self::Named(_))
    }
}

/// Where the user's config file is looked for, in order: `--config`, the environment override,
/// `$XDG_CONFIG_HOME`, then `~/.config`.
///
/// `None` when there is nowhere to look at all.
pub fn user(
    explicit: Option<&Path>,
    environment: Option<&OsStr>,
    xdg: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Option<UserPath> {
    if let Some(path) = explicit {
        return Some(UserPath::Named(path.to_owned()));
    }
    if let Some(path) = environment {
        return Some(UserPath::Named(PathBuf::from(path)));
    }
    if let Some(directory) = xdg {
        return Some(UserPath::Default(Path::new(directory).join(RELATIVE_PATH)));
    }
    home.map(|directory| UserPath::Default(Path::new(directory).join(".config").join(RELATIVE_PATH)))
}

// =====================================================================================================================
// The Repository Layer
// =====================================================================================================================

/// The nearest repository config at or above `cwd`, walking to the filesystem root.
///
/// The walk is what makes the layer usable: a spawn is run from wherever the work is, which is rarely
/// the project root, and a config found only in the process directory would be silently absent from
/// every subdirectory.
///
/// It does not stop at a repository boundary. Locating one is a herdr call this crate makes only for
/// worktree spawns, and paying for it on every load — to refuse a file the caller placed on purpose —
/// buys nothing.
pub fn repository(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .map(|directory| directory.join(REPOSITORY_PATH))
        .find(|candidate| candidate.is_file())
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn the_search_order_is_explicit_then_the_environment_then_xdg_then_home() {
        let explicit = PathBuf::from("/explicit/config.toml");

        assert_eq!(
            user(
                Some(&explicit),
                Some("/env".as_ref()),
                Some("/xdg".as_ref()),
                Some("/home".as_ref())
            ),
            Some(UserPath::Named(explicit))
        );
        assert_eq!(
            user(
                None,
                Some("/env/p.toml".as_ref()),
                Some("/xdg".as_ref()),
                Some("/home".as_ref())
            ),
            Some(UserPath::Named(PathBuf::from("/env/p.toml")))
        );
        assert_eq!(
            user(None, None, Some("/xdg".as_ref()), Some("/home".as_ref())),
            Some(UserPath::Default(PathBuf::from("/xdg/herdr-agent-tools/config.toml")))
        );
        assert_eq!(
            user(None, None, None, Some("/home".as_ref())),
            Some(UserPath::Default(PathBuf::from(
                "/home/.config/herdr-agent-tools/config.toml"
            )))
        );
        assert_eq!(user(None, None, None, None), None);
    }

    /// The two the caller named are the two whose absence is a failure.
    #[test]
    fn only_a_path_the_caller_named_is_one_this_tool_must_find() {
        assert!(user(Some(Path::new("/a.toml")), None, None, None).unwrap().named());
        assert!(user(None, Some("/b.toml".as_ref()), None, None).unwrap().named());
        assert!(!user(None, None, Some("/xdg".as_ref()), None).unwrap().named());
        assert!(!user(None, None, None, Some("/home".as_ref())).unwrap().named());
    }

    #[test]
    fn a_repository_config_is_found_from_any_directory_beneath_it() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join(".herdr-agent-tools/config.toml");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, "default = 'x'\n").unwrap();
        let deep = root.path().join("src/api/handlers");
        fs::create_dir_all(&deep).unwrap();

        assert_eq!(repository(&deep), Some(config.clone()));
        assert_eq!(repository(root.path()), Some(config));
    }

    #[test]
    fn the_nearest_one_wins_over_an_ancestors() {
        let root = tempfile::tempdir().unwrap();
        let outer = root.path().join(".herdr-agent-tools/config.toml");
        let inner = root.path().join("nested/.herdr-agent-tools/config.toml");
        for path in [&outer, &inner] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "default = 'x'\n").unwrap();
        }

        assert_eq!(repository(&root.path().join("nested")), Some(inner));
        assert_eq!(repository(root.path()), Some(outer));
    }

    #[test]
    fn a_tree_with_no_repository_config_has_none_rather_than_a_path_that_is_not_there() {
        let root = tempfile::tempdir().unwrap();

        assert_eq!(repository(root.path()), None);
    }

    /// A directory of that name is not a config file, so the walk keeps going.
    #[test]
    fn a_directory_where_the_file_should_be_is_not_a_config() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".herdr-agent-tools/config.toml")).unwrap();

        assert_eq!(repository(root.path()), None);
    }
}
```

- [ ] **Step 4: Wire it into `src/config.rs` and delete what it replaces**

At the top of `src/config.rs`, below the module doc comment, add:

```rust
mod discover;
```

Delete from `src/config.rs`: the `RELATIVE_PATH` constant, the whole `Helpers` section holding
`fn discover(...)`, and the test `the_search_order_is_explicit_then_the_environment_then_xdg_then_home`.
Leave `Presets::load` calling the old name for now — Task 5 rewrites it. To keep the tree compiling
between tasks, change its body's discovery call to:

```rust
        let path = discover::user(explicit, environment.as_deref(), xdg.as_deref(), home.as_deref())
            .ok_or(ConfigError::NoConfigDir)?
            .path()
            .to_owned();
```

- [ ] **Step 5: Run the verification suite**

Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add src/config.rs src/config/discover.rs
git commit -m "refactor(config): give both config layers one place that knows where they live"
```

---

### Task 3: The file schema

**Files:**
- Modify: `src/config.rs`

This task replaces the `Presets`/`Preset` types with the file-shaped ones and the layer read. It
leaves `Presets` in place as a thin wrapper so the tree keeps compiling; Task 5 removes it.

- [ ] **Step 1: Write the failing tests**

Replace the whole `mod tests` in `src/config.rs` with:

```rust
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
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --all-targets config`
Expected: FAIL — `cannot find type 'Layer'`.

- [ ] **Step 3: Replace the schema types in `src/config.rs`**

Replace the module doc comment with:

```rust
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
```

Replace the `EXAMPLE` constant and the whole `Presets` section with:

```rust
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
                return Err(ConfigError::Unreadable {
                    path: path.to_owned(),
                    source,
                });
            }
        };

        let file: ConfigFile = toml::from_str(&contents).map_err(|source| ConfigError::Malformed {
            path: path.to_owned(),
            source,
        })?;
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
```

Delete the old `Presets` struct, the old `Preset` struct, and `impl Presets`. The tree will not
compile until Task 5 — that is expected, and Steps 4–5 below cover it.

- [ ] **Step 4: Add the two new error variants**

In the `ConfigError` enum in `src/config.rs`, replace the `Missing` variant's message and add two
variants (the rest of the enum is rewritten in Task 5):

```rust
    /// The file is not there. The message carries a working example, since the usual cause is that
    /// it was never written.
    #[error("no config file at {}; create it with:\n\n{EXAMPLE}", path.display())]
    Missing {
        /// Where it was looked for.
        path: PathBuf,
    },
    /// The file still declares the table under its old name.
    #[error("{} declares [presets], which is now [agents]; rename the table", path.display())]
    LegacyPresets {
        /// The file to edit.
        path: PathBuf,
    },
```

And in `exit_status_hint`, add `Self::LegacyPresets { .. }` to the `ExitStatus::Failure` arm.

- [ ] **Step 5: Get the tree compiling by finishing Task 5 first if needed**

Tasks 3, 4, and 5 rewrite one file together. If `cargo test` cannot run at the end of Task 3, proceed
to Tasks 4 and 5 and run the suite once at the end of Task 5. Do not commit a tree that does not
build.

Run: `cargo test --all-targets config`
Expected: the new schema tests PASS once Task 5 is complete.

- [ ] **Step 6: Commit (after Task 5's suite is green)**

Commit Tasks 3–5 together:

```bash
git add src/config.rs src/config/resolve.rs
git commit -m "feat(config): rename presets to agents and give each one base, model, and effort"
```

---

### Task 4: `config/resolve.rs`

**Files:**
- Create: `src/config/resolve.rs`
- Modify: `src/config.rs` (add `mod resolve;`)

- [ ] **Step 1: Write the module, tests first**

Create `src/config/resolve.rs`:

```rust
//! Applying `base` chains to declared agents, and reporting what that made unusable.
//!
//! Pure. It is handed a merged declaration map and answers with the agents that resolved plus one
//! warning per agent that did not — the warnings stay data until `load` drains them into the sink,
//! which is what keeps every case here testable without one.
//!
//! Every failure is per-agent rather than per-map. A config with one broken entry keeps working for
//! every other entry, and the listing — the command a caller debugs with — still answers.

use std::collections::{BTreeMap, BTreeSet};

use super::{Agent, Declared};
use crate::harness;

// =====================================================================================================================
// Resolution
// =====================================================================================================================

/// What resolution produced.
pub struct Resolved {
    /// Every agent whose chain resolved, in name order.
    pub agents: BTreeMap<String, Agent>,
    /// One line per agent that did not, naming it and why.
    pub warnings: Vec<String>,
}

/// Applies every agent's base chain.
pub fn resolve(declared: &BTreeMap<String, Declared>) -> Resolved {
    let mut agents = BTreeMap::new();
    let mut warnings = Vec::new();

    for name in declared.keys() {
        match one(name, declared) {
            Ok(agent) => drop(agents.insert(name.clone(), agent)),
            Err(unusable) => warnings.push(unusable.warning(name)),
        }
    }

    Resolved { agents, warnings }
}

/// One agent, with its whole chain applied.
fn one(name: &str, declared: &BTreeMap<String, Declared>) -> Result<Agent, Unusable> {
    let chain = chain(name, declared)?;

    // Root first, so a child's value overwrites its base's and a child's args follow them. Appending
    // rather than replacing is what makes a child adding one flag a one-line agent: agent CLIs are
    // last-flag-wins, so nothing a child wants to change is lost by keeping the base's vector.
    let mut kind = None;
    let mut model = None;
    let mut effort = None;
    let mut prompt_file = None;
    let mut args = Vec::new();
    for link in &chain {
        kind = link.kind.clone().or(kind);
        model = link.model.clone().or(model);
        effort = link.effort.clone().or(effort);
        prompt_file = link.prompt_file.clone().or(prompt_file);
        args.extend(link.args.iter().cloned());
    }

    let kind = kind.ok_or(Unusable::NoKind)?;
    // `model` and `effort` exist only as whatever flags a harness spells them, and this build has
    // harnesses for two of herdr's kinds. An agent asking for them under any other is dropped rather
    // than started without them — a wrong answer nobody is told about is the failure this tool is
    // shaped to avoid.
    if (model.is_some() || effort.is_some()) && harness::by_kind(&kind).is_none() {
        return Err(Unusable::Untunable {
            kind,
            fields: fields_named(model.is_some(), effort.is_some()),
        });
    }

    Ok(Agent {
        kind,
        model,
        effort,
        args,
        prompt_file,
        // The agent's own declaration, not its base's: it is the file a reader goes to to change it.
        source: chain
            .last()
            .expect("a chain holds at least the agent itself")
            .source
            .clone(),
    })
}

/// The base chain, root first, ending with the agent itself.
///
/// # Errors
///
/// [`Unusable::DanglingBase`] when a `base` names an agent no layer declares, and [`Unusable::Cycle`]
/// when the walk returns to an agent it has already visited. The first lookup cannot dangle — the
/// caller iterates the map's own keys — so a failed lookup is always a `base`.
fn chain<'a>(name: &str, declared: &'a BTreeMap<String, Declared>) -> Result<Vec<&'a Declared>, Unusable> {
    let mut links = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = name.to_owned();

    loop {
        if !seen.insert(current.clone()) {
            return Err(Unusable::Cycle);
        }
        let link = declared.get(&current).ok_or(Unusable::DanglingBase { base: current })?;
        links.push(link);
        match &link.base {
            Some(base) => current = base.clone(),
            None => break,
        }
    }

    links.reverse();
    Ok(links)
}

// =====================================================================================================================
// Unusable Agents
// =====================================================================================================================

/// Why one agent did not resolve.
#[derive(Debug, PartialEq, Eq)]
enum Unusable {
    /// `base` names an agent no layer declares.
    DanglingBase {
        /// The name that was not found.
        base: String,
    },
    /// The chain returns to an agent it has already visited.
    Cycle,
    /// Nothing in the chain declares a kind.
    NoKind,
    /// The chain sets model or effort under a kind this build has no harness for.
    Untunable {
        /// That kind.
        kind: String,
        /// Which of the two fields were set.
        fields: String,
    },
}

impl Unusable {
    /// What the caller is told.
    ///
    /// Names the agent, the reason, and that the agent is gone — never a value out of the file.
    /// `model` and `effort` *become* the agent's command line, so the rule against logging an agent's
    /// arguments covers the fields that build one: the warning says which field was set, not what it
    /// was set to.
    fn warning(&self, name: &str) -> String {
        match self {
            Self::DanglingBase { base } => {
                format!("agent '{name}' bases on '{base}', which no config declares; '{name}' is unavailable")
            }
            Self::Cycle => format!("agent '{name}' is part of a base cycle; '{name}' is unavailable"),
            Self::NoKind => {
                format!("agent '{name}' declares no kind, and no base supplies one; '{name}' is unavailable")
            }
            Self::Untunable { kind, fields } => format!(
                "agent '{name}' sets {fields} under kind '{kind}', which this build can express only for {}; \
                 '{name}' is unavailable",
                harness::kinds().join(", ")
            ),
        }
    }
}

/// Which of the two tuning fields were set, for a warning to name.
fn fields_named(model: bool, effort: bool) -> String {
    match (model, effort) {
        (true, true) => "model and effort".to_owned(),
        (true, false) => "model".to_owned(),
        _ => "effort".to_owned(),
    }
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A declared agent, with only the fields a case is about.
    #[derive(Default)]
    struct Build {
        kind: Option<&'static str>,
        base: Option<&'static str>,
        model: Option<&'static str>,
        effort: Option<&'static str>,
        args: &'static [&'static str],
        prompt_file: Option<&'static str>,
    }

    impl Build {
        fn declared(self) -> Declared {
            Declared {
                kind: self.kind.map(ToOwned::to_owned),
                base: self.base.map(ToOwned::to_owned),
                model: self.model.map(ToOwned::to_owned),
                effort: self.effort.map(ToOwned::to_owned),
                args: self.args.iter().map(|argument| (*argument).to_owned()).collect(),
                prompt_file: self.prompt_file.map(PathBuf::from),
                source: PathBuf::from("/config.toml"),
            }
        }
    }

    /// A declaration map from `(name, agent)` pairs.
    fn map(agents: Vec<(&str, Build)>) -> BTreeMap<String, Declared> {
        agents
            .into_iter()
            .map(|(name, build)| (name.to_owned(), build.declared()))
            .collect()
    }

    #[test]
    fn an_agent_with_no_base_resolves_to_what_it_declared() {
        let declared = map(vec![(
            "cc",
            Build {
                kind: Some("claude"),
                args: &["--disallowed-tools", "AskUserQuestion"],
                ..Build::default()
            },
        )]);

        let resolved = resolve(&declared);

        assert!(resolved.warnings.is_empty());
        assert_eq!(resolved.agents["cc"].kind, "claude");
        assert_eq!(resolved.agents["cc"].args, ["--disallowed-tools", "AskUserQuestion"]);
    }

    #[test]
    fn a_child_inherits_kind_and_overrides_the_fields_it_states() {
        let declared = map(vec![
            (
                "cc",
                Build {
                    kind: Some("claude"),
                    model: Some("sonnet"),
                    ..Build::default()
                },
            ),
            (
                "opus",
                Build {
                    base: Some("cc"),
                    model: Some("opus"),
                    effort: Some("xhigh"),
                    ..Build::default()
                },
            ),
        ]);

        let resolved = resolve(&declared);

        let opus = &resolved.agents["opus"];
        assert_eq!(opus.kind, "claude", "inherited");
        assert_eq!(opus.model.as_deref(), Some("opus"), "the child wins");
        assert_eq!(opus.effort.as_deref(), Some("xhigh"));
    }

    /// The one field that appends rather than replacing, root first.
    #[test]
    fn args_are_concatenated_up_the_whole_chain_with_the_root_first() {
        let declared = map(vec![
            (
                "a",
                Build {
                    kind: Some("claude"),
                    args: &["--one"],
                    ..Build::default()
                },
            ),
            (
                "b",
                Build {
                    base: Some("a"),
                    args: &["--two"],
                    ..Build::default()
                },
            ),
            (
                "c",
                Build {
                    base: Some("b"),
                    args: &["--three"],
                    ..Build::default()
                },
            ),
        ]);

        let resolved = resolve(&declared);

        assert_eq!(resolved.agents["c"].args, ["--one", "--two", "--three"]);
    }

    #[test]
    fn a_prompt_file_is_inherited_and_a_child_may_replace_it() {
        let declared = map(vec![
            (
                "a",
                Build {
                    kind: Some("claude"),
                    prompt_file: Some("/config/a.md"),
                    ..Build::default()
                },
            ),
            (
                "b",
                Build {
                    base: Some("a"),
                    ..Build::default()
                },
            ),
            (
                "c",
                Build {
                    base: Some("a"),
                    prompt_file: Some("/config/c.md"),
                    ..Build::default()
                },
            ),
        ]);

        let resolved = resolve(&declared);

        assert_eq!(
            resolved.agents["b"].prompt_file.as_deref(),
            Some(PathBuf::from("/config/a.md").as_path())
        );
        assert_eq!(
            resolved.agents["c"].prompt_file.as_deref(),
            Some(PathBuf::from("/config/c.md").as_path())
        );
    }

    #[test]
    fn a_base_no_config_declares_drops_that_agent_and_leaves_the_rest() {
        let declared = map(vec![
            (
                "cc",
                Build {
                    kind: Some("claude"),
                    ..Build::default()
                },
            ),
            (
                "broken",
                Build {
                    base: Some("typo"),
                    ..Build::default()
                },
            ),
        ]);

        let resolved = resolve(&declared);

        assert!(resolved.agents.contains_key("cc"), "the rest still works");
        assert!(!resolved.agents.contains_key("broken"));
        assert_eq!(
            resolved.warnings,
            ["agent 'broken' bases on 'typo', which no config declares; 'broken' is unavailable"]
        );
    }

    #[test]
    fn a_cycle_drops_every_agent_in_it() {
        let declared = map(vec![
            (
                "a",
                Build {
                    base: Some("b"),
                    ..Build::default()
                },
            ),
            (
                "b",
                Build {
                    base: Some("a"),
                    ..Build::default()
                },
            ),
            (
                "self",
                Build {
                    base: Some("self"),
                    ..Build::default()
                },
            ),
        ]);

        let resolved = resolve(&declared);

        assert!(resolved.agents.is_empty());
        assert_eq!(resolved.warnings.len(), 3);
        assert!(resolved.warnings.iter().all(|warning| warning.contains("base cycle")));
    }

    #[test]
    fn a_chain_that_never_declares_a_kind_is_unusable() {
        let declared = map(vec![
            ("a", Build::default()),
            (
                "b",
                Build {
                    base: Some("a"),
                    model: Some("opus"),
                    ..Build::default()
                },
            ),
        ]);

        let resolved = resolve(&declared);

        assert!(resolved.agents.is_empty());
        assert!(
            resolved
                .warnings
                .iter()
                .all(|warning| warning.contains("declares no kind")),
            "got {:?}",
            resolved.warnings
        );
    }

    /// The tuning fields need a harness, and this build has two.
    #[test]
    fn model_or_effort_under_a_kind_this_build_cannot_drive_drops_that_agent() {
        let declared = map(vec![(
            "gem",
            Build {
                kind: Some("gemini"),
                model: Some("pro"),
                ..Build::default()
            },
        )]);

        let resolved = resolve(&declared);

        assert!(resolved.agents.is_empty());
        let warning = &resolved.warnings[0];
        assert!(warning.contains("sets model under kind 'gemini'"), "got {warning}");
        assert!(warning.contains("claude, codex"), "got {warning}");
        assert!(!warning.contains("pro"), "the value is an argument and must not be logged");
    }

    /// An unknown kind is only refused when it asks for something this build must spell.
    #[test]
    fn an_unknown_kind_that_asks_for_neither_is_passed_through_untouched() {
        let declared = map(vec![(
            "gem",
            Build {
                kind: Some("gemini"),
                args: &["--yolo"],
                ..Build::default()
            },
        )]);

        let resolved = resolve(&declared);

        assert!(resolved.warnings.is_empty());
        assert_eq!(resolved.agents["gem"].kind, "gemini");
        assert_eq!(resolved.agents["gem"].args, ["--yolo"]);
    }

    #[test]
    fn the_warning_names_whichever_of_the_two_fields_was_set() {
        assert_eq!(fields_named(true, true), "model and effort");
        assert_eq!(fields_named(true, false), "model");
        assert_eq!(fields_named(false, true), "effort");
    }
}
```

- [ ] **Step 2: Wire it into `src/config.rs`**

Below `mod discover;` add:

```rust
mod resolve;
```

- [ ] **Step 3: Run to verify (with Task 5)**

Run: `cargo test --all-targets resolve`
Expected: PASS once Task 5 defines `Agent`.

---

### Task 5: `Agent`, `Config`, and the layered load

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: Write the failing tests**

Append these to `src/config.rs`'s `mod tests`:

```rust
    /// Two layers written to one temp directory tree, with the repository config beneath it.
    fn layered(user: &str, repository: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let user_path = root.path().join("user.toml");
        fs::write(&user_path, user).unwrap();
        let repository_path = root.path().join("project/.herdr-agent-tools/config.toml");
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
        fs::write(&path, "default = 'a'\n\n[agents.a]\nkind = 'claude'\nprompt_file = 'brief.md'\n").unwrap();
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
        fs::write(&path, "default = 'a'\n\n[agents.a]\nkind = 'claude'\nprompt_file = 'brief.md'\n").unwrap();

        let config = merged(&[&path]);
        let error = config.resolve(None).unwrap().brief().unwrap_err();
        assert_eq!(error.exit_status_hint(), ExitStatus::NotFound);

        fs::write(directory.path().join("brief.md"), "   \n\n").unwrap();
        let error = config.resolve(None).unwrap().brief().unwrap_err();
        assert_eq!(error.exit_status_hint(), ExitStatus::NotFound);
        assert!(error.to_string().contains("nothing to deliver"), "got {error}");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --all-targets config`
Expected: FAIL — `cannot find type 'Config'`.

- [ ] **Step 3: Add the `Agent` and `Config` sections**

In `src/config.rs`, after the `The File` section, add:

```rust
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
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::UnreadableBrief {
            path: path.clone(),
            source,
        })?;
        text.parse()
            .map(Some)
            .map_err(|_| ConfigError::BlankBrief { path: path.clone() })
    }
}

// =====================================================================================================================
// Config
// =====================================================================================================================

/// Every agent, from every layer, resolved.
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
                return Err(ConfigError::Missing {
                    path: user.path().to_owned(),
                });
            }
            None => {}
        }

        // Nested rather than a let-chain, to keep this readable under the pinned toolchain.
        if let Some(directory) = working_directory(cwd, sink) {
            if let Some(path) = discover::repository(&directory) {
                if let Some(layer) = Layer::read(&path)? {
                    layers.push(layer);
                }
            }
        }

        if layers.is_empty() {
            return Err(ConfigError::Missing {
                path: user.path().to_owned(),
            });
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

        Ok(Self {
            default,
            agents: resolved.agents,
        })
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
```

Add the `Helpers` section at the end, before `Tests`:

```rust
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
```

- [ ] **Step 4: Replace the error enum**

Replace the whole `Errors` section of `src/config.rs` with:

```rust
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
```

- [ ] **Step 5: Fix the imports at the top of `src/config.rs`**

```rust
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use getter_methods::Getters;
use serde::Deserialize;
use thiserror::Error;

use crate::cmd::ExitStatus;
use crate::core::{NonEmptyText, Sink};
use crate::harness;
```

`std::ffi::OsStr` moves to `discover.rs` and is no longer needed here.

- [ ] **Step 6: Run the verification suite**

Run the full suite. Expected: all green. Tasks 3–5's tests all pass.

**If `Getters` does not produce `Option<&str>` for the `Option<String>` fields**, that is fine —
nothing outside `config` calls `model()`/`effort()` in a way that depends on the exact reference type
(Task 6 uses `.map(ToOwned::to_owned)`). Do not change the derive.

- [ ] **Step 7: Commit Tasks 3–5**

```bash
git add src/config.rs src/config/resolve.rs
git commit -m "feat(config): rename presets to agents, add base, model, effort, and a repo layer"
```

---

### Task 6: The `agents` listing

**Files:**
- Create: `src/cmd/agents.rs`
- Delete: `src/cmd/presets.rs`
- Modify: `src/cmd.rs`

- [ ] **Step 1: Write the new command with its tests**

```bash
git mv src/cmd/presets.rs src/cmd/agents.rs
```

Replace the contents of `src/cmd/agents.rs` with:

```rust
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
                    args: vec!["--no-alt-screen".to_owned(), "--model".to_owned(), "gpt-5-low".to_owned()],
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
```

- [ ] **Step 2: Update `src/cmd.rs`**

Change `mod presets;` to `mod agents;` (keeping the module list alphabetical: `agents`, `kill`,
`prime`, `prompt`, `spawn`) and `pub use presets::PresetsArgs;` to `pub use agents::AgentsArgs;`.

- [ ] **Step 3: Run to verify**

Run: `cargo test --all-targets`
Expected: FAIL in `main.rs` and `prime.rs`, which Tasks 7 and 10 fix. The `agents` module's own tests
pass.

- [ ] **Step 4: Commit (after Task 7)**

Tasks 6 and 7 land together, since neither compiles alone.

---

### Task 7: Dispatch

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Rename the subcommand**

In the `use` line, replace `PresetsArgs` with `AgentsArgs`:

```rust
use crate::cmd::{AgentsArgs, AsExitStatus, Cmd, ExitStatus, Failure, KillArgs, PrimeArgs, PromptArgs, SpawnArgs};
```

In `enum Command`, replace `Presets(PresetsArgs)` with `Agents(AgentsArgs)`.

In `main`'s dispatch match, replace `Command::Presets(args) => run(args, &sink),` with
`Command::Agents(args) => run(args, &sink),`.

- [ ] **Step 2: Update the long-about**

Replace the final clause of the `long_about` string:

```rust
        `prompt` delivers text to an agent that already exists, `kill` closes an agent's pane \
        unless it is mid-task, and `agents` lists what the config holds.\n\
```

- [ ] **Step 3: Update the two tests naming the old command**

In `mod tests`, replace `presets` with `agents` in
`the_json_flag_is_recovered_from_argv_wherever_it_sits`:

```rust
    #[test]
    fn the_json_flag_is_recovered_from_argv_wherever_it_sits() {
        assert!(json_requested(argv(&["herdr-agent-tools", "--json", "agents"])));
        assert!(json_requested(argv(&["herdr-agent-tools", "agents", "--json"])));
        assert!(!json_requested(argv(&["herdr-agent-tools", "agents"])));
    }
```

- [ ] **Step 4: Run to verify**

Run: `cargo test --all-targets`
Expected: FAIL only in `src/cmd/prime.rs` and `src/cmd/spawn.rs`, which Tasks 8–10 fix.

- [ ] **Step 5: Commit (after Task 10)**

---

### Task 8: `spawn --agent`

**Files:**
- Modify: `src/cmd/spawn.rs`

- [ ] **Step 1: Write the failing tests**

Add to `src/cmd/spawn.rs`'s `mod tests`:

```rust
    #[test]
    fn the_agent_is_named_with_agent_rather_than_preset() {
        assert_eq!(
            parse(&["spawn", "reviewer", "--agent", "opus"]).agent.as_deref(),
            Some("opus")
        );
        assert_eq!(parse(&["spawn", "reviewer"]).agent, None, "the config's default applies");
    }

    /// The old flag is declared only so its rename can be reported.
    ///
    /// clap's suggestion machinery will not reach `--agent` from `preset`, and a bare *unrecognized
    /// argument* is a poor way to learn about a rename.
    #[test]
    fn the_old_flag_is_answered_with_the_one_that_replaced_it() {
        let args = parse(&["spawn", "reviewer", "--preset", "opus"]);

        let error = args.check_renamed_flags().unwrap_err();

        assert_eq!(error.exit_status(), ExitStatus::Usage);
        assert_eq!(error.to_string(), "--preset is now --agent");
    }

    #[test]
    fn a_spawn_that_uses_neither_flag_passes_the_rename_check() {
        assert!(parse(&["spawn", "reviewer", "--agent", "opus"]).check_renamed_flags().is_ok());
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --all-targets spawn`
Expected: FAIL — no field `agent`.

- [ ] **Step 3: Change the flags**

In `SpawnArgs`, replace the `preset` field with:

```rust
    /// The agent to start; defaults to the config's `default`.
    #[arg(long, value_name = "NAME")]
    agent: Option<String>,

    /// Renamed to `--agent`; declared only so the rename can be reported rather than guessed at.
    #[arg(long, value_name = "NAME", hide = true)]
    preset: Option<String>,
```

Update the `config` field's doc comment:

```rust
    /// Read this config file instead of the one in the config directory.
    ///
    /// The repository's `.herdr-agent-tools/config.toml` is still merged over it.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
```

Update the `agent_args` field's doc comment:

```rust
    /// Extra arguments appended after the agent's, passed to the agent verbatim.
    ///
    /// No merging and no de-duplication, so the agent's own last-flag-wins rules settle any conflict
    /// with the config.
```

Update the `after_help` examples: `--preset opus` → `--agent opus`, `--preset sonnet` →
`--agent sonnet`, `--preset fable` → `--agent fable`.

- [ ] **Step 4: Add the rename check and rewire `execute`**

Add to `impl SpawnArgs`, beside `check_worktree_flags`:

```rust
    /// Refuses a flag this build renamed, naming what replaced it.
    ///
    /// Not left to clap: its suggestion machinery scores `--preset` too far from `--agent` to offer
    /// it, so the rejection a caller would otherwise meet says only that the argument is
    /// unrecognized.
    ///
    /// # Errors
    ///
    /// [`SpawnError::RenamedFlag`].
    fn check_renamed_flags(&self) -> Result<(), SpawnError> {
        if self.preset.is_some() {
            return Err(SpawnError::RenamedFlag {
                old: "--preset",
                new: "--agent",
            });
        }
        Ok(())
    }
```

In `execute`, replace the preset block. The whole opening becomes:

```rust
    fn execute(self, sink: &Sink) -> Result<Self::Ok, Self::Err> {
        // `AgentName` derefs to `str`, so this compares the name itself rather than the newtype.
        if &*self.name == OPERATOR {
            return Err(SpawnError::ReservedName);
        }
        self.check_renamed_flags()?;
        self.check_worktree_flags()?;
        let anchor = match self.placement {
            Placement::Pane => Some(anchor(std::env::var(PANE_VARIABLE).ok().as_deref())?),
            Placement::Tab | Placement::Workspace | Placement::Worktree => None,
        };

        // Resolved before the config, because it is where the repository layer's walk starts: the
        // config that applies is the one belonging to the tree this agent will work in.
        let cwd = match &self.cwd {
            Some(path) => path.clone(),
            None => std::env::current_dir().map_err(SpawnError::NoWorkingDirectory)?,
        };

        // `as_path`, not `&cwd`: `Option<&PathBuf>` does not coerce to `Option<&Path>`.
        let config = Config::load(self.config.as_deref(), Some(cwd.as_path()), sink)?;
        let agent = config.resolve(self.agent.as_deref())?;
        let kind = agent.kind().to_owned();
        let mut agent_args = agent.agent_args();
        agent_args.extend(self.agent_args.iter().cloned());
        // Read here, among the other pre-checks: a missing brief is refused while a refusal is still
        // free, rather than after a surface exists.
        let brief = agent.brief()?;

        let cwd = cwd.to_string_lossy().into_owned();

        // Read before anything is created, so a caller outside a repository is refused while a
        // refusal is still free.
        let subdirectory = self.subdirectory(&cwd)?;

        // Everything above is read-only, so nothing exists yet if any of it failed.
        let (pane, worktree) = self.create_surface(anchor, &cwd, sink)?;

        // From here on a failure leaves the pane open and names it: whatever went wrong is on
        // screen in it, and closing it would throw the error away with it. Every step past this
        // point is inside one call, so that note has one owner rather than a wrapper per step.
        self.start_and_prompt(
            &pane,
            &kind,
            &agent_args,
            brief.as_ref(),
            worktree,
            subdirectory.as_deref(),
            sink,
        )
        .map_err(|error| error.note_open_pane(&pane))
    }
```

Update the import: `use crate::config::{Config, ConfigError};`.

- [ ] **Step 5: Add the error variant**

In `SpawnError`, after `WorktreeOnlyFlag`:

```rust
    /// A flag this build renamed.
    #[error("{old} is now {new}")]
    RenamedFlag {
        /// What the caller typed.
        old: &'static str,
        /// What it is called now.
        new: &'static str,
    },
```

In `AsExitStatus for SpawnError`, add `Self::RenamedFlag { .. }` to the `ExitStatus::Usage` arm and to
the `None` arm of `herdr()`.

- [ ] **Step 6: Run to verify**

Run: `cargo test --all-targets spawn`
Expected: FAIL — `start_and_prompt` takes six arguments. Task 9 supplies the seventh.

---

### Task 9: The brief at spawn

**Files:**
- Modify: `src/cmd/spawn.rs`

- [ ] **Step 1: Write the failing tests**

Add to `src/cmd/spawn.rs`'s `mod tests`:

```rust
    /// The brief comes first, so the caller's instruction reads as an instruction about it.
    #[test]
    fn a_brief_and_a_prompt_are_delivered_as_one_message_with_a_blank_line_between() {
        let brief = "You review Rust.".parse::<NonEmptyText>().unwrap();
        let text = "start with the auth module".parse::<NonEmptyText>().unwrap();

        assert_eq!(
            first_prompt(Some(&brief), Some(&text)).unwrap().to_string(),
            "You review Rust.\n\nstart with the auth module"
        );
    }

    #[test]
    fn either_half_alone_is_delivered_unchanged_and_neither_delivers_nothing() {
        let brief = "You review Rust.".parse::<NonEmptyText>().unwrap();
        let text = "audit the CLI".parse::<NonEmptyText>().unwrap();

        assert_eq!(
            first_prompt(Some(&brief), None).unwrap().to_string(),
            "You review Rust."
        );
        assert_eq!(first_prompt(None, Some(&text)).unwrap().to_string(), "audit the CLI");
        assert!(first_prompt(None, None).is_none());
    }
```

Add `use crate::core::NonEmptyText;` to the test module's imports if it is not already in scope from
the file's own imports (it is — `NonEmptyText` is imported at the top of `spawn.rs`).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --all-targets spawn`
Expected: FAIL — `cannot find function 'first_prompt'`.

- [ ] **Step 3: Add the helper**

In `src/cmd/spawn.rs`'s `Helpers` section:

```rust
/// The first prompt: the agent's brief, the caller's text, or the brief followed by it.
///
/// The brief comes first so the caller's instruction reads as an instruction about it, separated by a
/// blank line so a brief ending mid-paragraph does not run into the instruction.
///
/// `None` only when there is neither, which is a spawn with nothing to deliver. Composed rather than
/// re-parsed: both halves are already non-blank, so the result cannot be.
fn first_prompt(brief: Option<&NonEmptyText>, text: Option<&NonEmptyText>) -> Option<NonEmptyText> {
    match (brief, text) {
        (Some(brief), Some(text)) => Some(NonEmptyText::composed(format!("{brief}\n\n{text}"))),
        (Some(only), None) | (None, Some(only)) => Some(only.clone()),
        (None, None) => None,
    }
}
```

- [ ] **Step 4: Thread the brief through `start_and_prompt`**

Replace `start_and_prompt`'s signature and its prompt block:

```rust
    fn start_and_prompt(
        &self,
        pane: &PaneId,
        kind: &str,
        agent_args: &[String],
        brief: Option<&NonEmptyText>,
        worktree: Option<Checkout>,
        subdirectory: Option<&str>,
        sink: &Sink,
    ) -> Result<Spawned, SpawnError> {
        // Before the agent, because `agent start` inherits the shell's directory — after it, the
        // shell would move and the agent would not.
        if let (Some(checkout), Some(relative)) = (&worktree, subdirectory) {
            self.open_subdirectory(pane, &checkout.path, relative, sink)?;
        }

        let started = self.start_when_settled(pane, kind, agent_args)?;

        // The agent's configured brief and the caller's `--prompt` are one message: an agent that got
        // two would answer the first before hearing the second.
        let Some(text) = first_prompt(brief, self.prompt.as_deref()) else {
            return Ok(Spawned {
                placement: self.placement,
                delivered: None,
                worktree,
                agent: started,
            });
        };
```

The rest of the function is unchanged except the `deliver` call, which now takes the composed text:

```rust
        let agent = deliver(pane, &text, &self.reply(), Some(&wait), sink)?;
```

- [ ] **Step 5: Run the verification suite**

Expected: green except `src/cmd/prime.rs`, which Task 10 fixes.

- [ ] **Step 6: Commit (after Task 10)**

---

### Task 10: The brief `prime` prints

**Files:**
- Modify: `src/cmd/prime.rs`

- [ ] **Step 1: Update the imports and the `Cmd` impl**

```rust
use crate::cmd::agents::AgentList;
use crate::config::Config;
```

```rust
    fn execute(self, sink: &Sink) -> Result<Self::Ok, Self::Err> {
        // The one place a `ConfigError` is deliberately dropped rather than reported. A hook fires
        // before anyone has necessarily written a config, and failing there would cost the whole
        // brief to say something the brief itself already says. The warnings a partly-broken config
        // produces still reach the sink, which is the one thing worth saying here.
        let agents = Config::load(self.config.as_deref(), None, sink)
            .ok()
            .map(|config| AgentList::of(&config));
        // Resolved here rather than carried as a name, so `Display` has nothing left to look up.
        // `expect` is safe because `known_harness` rejected anything `by_kind` cannot resolve.
        let host = self
            .hook
            .as_deref()
            .map(|kind| harness::by_kind(kind).expect("the value parser accepted this harness"));
        Ok(Brief { guidance: GUIDANCE, agents, host })
    }
```

- [ ] **Step 2: Rename the `Brief` field and its rendering**

```rust
    /// The agent table, absent when no config could be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    agents: Option<AgentList>,
```

```rust
    /// The brief as text: the guidance, then the agent table.
    ///
    /// Shared by both forms, because the hook envelope carries exactly what a reader would have seen.
    fn text(&self) -> String {
        match &self.agents {
            Some(agents) => format!("{}\n\n## Agents\n\n{agents}", self.guidance),
            // Said rather than omitted: an agent that knows agents exist and sees none knows not to
            // reach for `--agent`.
            None => format!("{}\n\n## Agents\n\nNone configured.", self.guidance),
        }
    }
```

Update `PrimeArgs`'s `config` doc comment to *"Read this config file instead of the one in the config
directory."*

- [ ] **Step 3: Update the GUIDANCE text**

Four edits inside the `GUIDANCE` constant:

```
    spawn <name> --agent <agent>          pick which agent starts; see Agents below
```

```
    spawn <name> -- <agent args>          extra args, appended after the config's
```

```
## Reference

    agents                                list what the config holds
    prime                                 print this brief again; text in both modes
```

Also add one line to the `Launching agents` block, after the `--prompt -` row, so an agent knows a
brief may already be attached:

```
    spawn <name> --agent <agent>          an agent may carry a brief; it precedes your prompt
```

If the line-budget test fails, drop that added line rather than raising the budget — the budget is a
deliberate ceiling.

- [ ] **Step 4: Update the tests naming the old table**

In `src/cmd/prime.rs`'s `mod tests`, change the assertion
`brief.contains("## Presets\n\nNone configured.")` to `"## Agents\n\nNone configured."`, and the
`Brief { … presets: None … }` literal to `agents: None`.

- [ ] **Step 5: Run the verification suite**

Expected: all green.

- [ ] **Step 6: Commit Tasks 6–10**

```bash
git add src/cmd.rs src/cmd/agents.rs src/cmd/prime.rs src/cmd/spawn.rs src/main.rs
git commit -m "feat(cli): rename presets to agents and prepend an agent's brief to its first prompt"
```

---

### Task 11: Docs, install, and rehearsal

**Files:**
- Modify: `README.md`, `CLAUDE.md`, `AGENTS.md`, `docs/STYLE-GUIDE.md`

- [ ] **Step 1: `README.md`**

Rewrite every `preset` reference. The changes, by current line:

- Line 6: *"resolves the agent's kind and its usual flags from a named preset"* → *"resolves the
  agent's kind and its usual flags from a named agent in the config"*.
- Line 13: *"start a preset-configured agent in it"* → *"start a configured agent in it"*.
- Line 16: the `presets` table row → `` `agents` | List what the config holds ``.
- Line 21: *"costs the preset table"* → *"costs the agent table"*.
- Line 25: `--preset opus` → `--agent opus`.
- Line 28: `herdr-agent-tools presets` → `herdr-agent-tools agents`.
- Line 36: the `## Presets` heading → `## Agents`.
- Line 42: the paragraph about the filename not naming presets → keep its argument, replace
  *"Presets are what the file holds today"* with *"Agents are what the file holds today"*.
- Lines 48–52: the `[presets.reviewer]` / `[presets.cheap]` example → the `EXAMPLE` constant's
  content from `src/config.rs`.
- Line 57: *"The preset name is the table key"* → *"The agent name is the table key"*.
- Line 124: *"preset that does not skip permission checks; a Codex preset"* → *"agent that does not
  skip permission checks; a Codex agent"*.

Then add this section after the config example:

````markdown
An agent may inherit from another. `base` names one, and every field the child states wins —
except `args`, which appends after the base's, since agent CLIs are last-flag-wins and a child
adding one flag should not have to restate the rest.

```toml
[agents.cc]
kind = 'claude'
args = ['--disallowed-tools', 'AskUserQuestion']

[agents.opus]
base = 'cc'
model = 'opus'
effort = 'xhigh'
```

`model` and `effort` are fields rather than flags because each CLI spells them differently: Claude
Code takes `--model opus --effort xhigh`, Codex takes `--model … -c model_reasoning_effort=xhigh`.
This tool knows how to drive two of herdr's kinds, so an agent that sets either under a third is
dropped with a warning naming the kinds that can express them.

`prompt_file` names a file that is prepended to the agent's first prompt, resolved relative to the
directory of the config file that declared it:

```toml
[agents.reviewer]
base = 'opus'
prompt_file = 'review.md'
```

`spawn reviewer --prompt "start with auth"` then delivers the file's contents, a blank line, and
the prompt, as one message. With no `--prompt` the file is delivered alone. A file that is missing
or blank is a refusal, raised before anything is created.

## The repository layer

A repository may carry its own `.herdr-agent-tools/config.toml`, found by walking up from the
directory a spawn is run in. It merges over the user's: its `default` wins when it declares one,
and its agents replace the user's **by name and whole** — a repository agent that wants the user's
flags says `base = '<name>'`, which resolves across both files. Either layer alone is enough.
`--config` points the user layer somewhere else and the repository layer still merges over it.

Four things make one agent unusable, and each warns and drops that agent rather than failing the
command: a `base` no config declares, a cycle, no `kind` anywhere in the chain, and `model` or
`effort` under a kind this build cannot drive. Everything else in the config keeps working.
````

- [ ] **Step 2: `CLAUDE.md` and `AGENTS.md`**

These two files are identical in content. In rule 3 of each, replace *"a preset's arguments"* with
*"an agent's arguments"*, in both the opening sentence and the parenthetical.

- [ ] **Step 3: `docs/STYLE-GUIDE.md`**

Three edits:

- Module map, the `config` bullet: *"the preset file: its schema, where it is found, and how it is
  read"* → *"the config file: its schema, where its two layers are found, and how they are read"*.
  Add: *"It is also where an agent's `prompt_file` is read, since the file a config points at is part
  of the config."*
- Module map, the `core` bullet: *"a malformed preset file would then break `prompt`"* → *"a malformed
  config would then break `prompt`"*.
- `External effects`: *"A preset's `args` is therefore an array only"* → *"An agent's `args` is
  therefore an array only"*.
- `Forbidden`: *"A prompt, a preset's arguments, or captured terminal content"* → *"A prompt, an
  agent's arguments or the `model` and `effort` that become them, or captured terminal content"*.

- [ ] **Step 4: Run the verification suite, then install**

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
bash .claude/skills/rust-style/scripts/check.sh src
cargo install --path . --force
```

`herdr-agent-tools` on `PATH` resolves to `~/.cargo/bin`, never to `target/`, so a green
`cargo test` says nothing about the command the user is about to type.

- [ ] **Step 5: Migrate the live config**

The user's own config still uses `[presets.*]`. Before rehearsing, rename its tables to `[agents.*]`
and move each entry's `--model`/`--effort` out of `args` into the new fields, collapsing the shared
flags into a `base`. Ask the user before editing their config file.

- [ ] **Step 6: Rehearse against a live herdr session**

Report these separately from the static checks — nothing above exercises a real herdr server.

1. `herdr-agent-tools agents` — the listing renders, marks the default, and shows tuning flags.
2. `herdr-agent-tools agents --json | jq .` — parses, and `source` names the file each came from.
3. `herdr-agent-tools spawn t1 --agent opus --placement tab` — the pane's command line carries
   `--model opus --effort xhigh`, confirming Claude Code accepts `--effort`.
4. `herdr-agent-tools spawn t2 --agent sol --placement tab` — the pane's command line carries
   `-c model_reasoning_effort=…` intact, confirming Codex takes it.
5. Write a `.herdr-agent-tools/config.toml` in a scratch repository with one agent and a
   `prompt_file`, then `spawn` it from a subdirectory of that repository: the walk finds the config,
   and the agent's first message is the brief.
6. `herdr-agent-tools spawn t4 --preset opus` — exits 2 saying `--preset is now --agent`.
7. Add a `[agents.broken] base = 'nope'` entry and run `herdr-agent-tools agents` — it warns, drops
   `broken`, and still lists everything else.
8. `herdr-agent-tools kill t1 t2` (one per invocation) to clean up.

- [ ] **Step 7: Commit**

```bash
git add README.md CLAUDE.md AGENTS.md docs/STYLE-GUIDE.md
git commit -m "docs: the config holds agents, not presets"
```

---

## Notes for the implementer

- **Tasks 3, 4, and 5 rewrite one file together.** The tree does not compile between them. Run the
  suite once at the end of Task 5, and commit all three at once. Same for 6+7 and 8+9+10.
- **`Sink::new(OutputMode::Human)` in a config test** writes warnings to stderr during the run. That
  is intended — the sink is the only thing allowed to print, and a test asserting on warnings should
  assert on `resolve::resolve`'s returned `Vec<String>` instead, which is why resolution returns them
  as data.
- **Do not add a `--prompt-file` flag to `spawn`.** The brief is a config field; a flag for it is out
  of scope and was not designed.
- **Warnings never carry a `model` or `effort` value.** They become the agent's command line, and the
  no-logging rule covers the fields that build one. The warning names the field, never the value.
