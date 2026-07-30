//! The four ways to make a pane for an agent to start in, and the one way to take it away again.

use clap::ValueEnum;
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

use crate::core::{AgentName, PaneId};
use crate::herdr::{HerdrError, run};

// =====================================================================================================================
// Placement
// =====================================================================================================================

/// Where a new agent's pane comes from.
///
/// `ValueEnum` here is what makes `spawn --placement` parse straight into this type, so the parsed
/// shape *is* the domain type rather than a boolean triple translated into one. clap spells each
/// variant lowercase, which is what `Serialize` already emitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// Split the calling pane.
    Pane,
    /// A new tab, whose root pane the agent takes.
    Tab,
    /// A new workspace, whose root pane the agent takes.
    Workspace,
    /// A new Git worktree, whose workspace's root pane the agent takes.
    Worktree,
}

/// Whether a created surface takes the user's focus.
///
/// An enum rather than a `bool`, because a bare `true` at a call site says nothing about which way
/// it points, and this flag is inverted relative to the CLI flag that sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// Focus the new surface — `--focus`, which is opt-in.
    Take,
    /// Leave the human where they were — `--no-focus`, the default.
    Leave,
}

impl Focus {
    /// The herdr flag this choice spells. Stated in every argument vector rather than relying on
    /// herdr's default, so the vector says what it means.
    fn flag(self) -> &'static str {
        match self {
            Self::Take => "--focus",
            Self::Leave => "--no-focus",
        }
    }
}

// =====================================================================================================================
// Surfaces
// =====================================================================================================================

/// Splits `pane` and reports the pane that appeared.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn split(pane: &PaneId, cwd: &str, focus: Focus) -> Result<PaneId, HerdrError> {
    let created: PaneCreated = run(&split_args(pane, cwd, focus))?;
    Ok(created.pane.pane_id)
}

/// Creates a tab labelled `label` and reports its root pane.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
/// `workspace` pins where the tab opens; see [`tab_args`] for why it is not left to herdr's
/// default. `None` when the caller has no workspace of its own to name.
pub fn create_tab(workspace: Option<&str>, label: &AgentName, cwd: &str, focus: Focus) -> Result<PaneId, HerdrError> {
    let created: RootPaneCreated = run(&tab_args(workspace, label, cwd, focus))?;
    Ok(created.root_pane.pane_id)
}

/// Creates a workspace labelled `label` and reports its root pane.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn create_workspace(label: &AgentName, cwd: &str, focus: Focus) -> Result<PaneId, HerdrError> {
    let created: RootPaneCreated = run(&workspace_args(label, cwd, focus))?;
    Ok(created.root_pane.pane_id)
}

/// Creates a Git worktree of `source`, labels its workspace `label`, and reports its root pane
/// along with the checkout it opened on.
///
/// `source` is the checkout the worktree is cut *from*, not where it lands — herdr chooses the
/// checkout path itself, under the `worktree_directory` in its own config. `branch` and `base` are
/// herdr's to default: it generates a `worktree/`-prefixed branch name for `None` and bases on
/// `HEAD`, and restating either here would be a second authority to keep in step.
///
/// # Errors
///
/// Returns whatever [`run`] returned. `linked_worktree_source` is the refusal worth expecting:
/// herdr will not cut a worktree whose source is itself a linked worktree, so an agent already in
/// one cannot nest another.
pub fn create_worktree(
    label: &AgentName,
    source: &str,
    branch: Option<&str>,
    base: Option<&str>,
    focus: Focus,
) -> Result<(PaneId, Checkout), HerdrError> {
    let created: WorktreeCreated = run(&worktree_args(label, source, branch, base, focus))?;
    Ok((created.root_pane.pane_id, created.worktree))
}

/// The workspace a pane currently belongs to.
///
/// Asked rather than read from herdr's `HERDR_WORKSPACE_ID`, which it injects when a pane is created
/// and cannot update afterwards — nothing can rewrite a running shell's environment from outside. Move
/// a pane to another workspace and that variable still names the old one, silently, where this call
/// reports the new one. One extra call buys an answer that is right in both cases.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn workspace_of(pane: &str) -> Result<String, HerdrError> {
    let info: PaneWorkspace = run(&get_args(pane))?;
    Ok(info.pane.workspace_id)
}

/// Runs one command line in a pane's shell, as if a person had typed it there.
///
/// This is what places a worktree spawn's agent in a subdirectory: `agent start` launches the agent
/// *through* the pane's shell — it appears in the pane as a typed command line — so the shell's
/// directory at launch is the agent's directory. One call, and no pane is split, moved, or closed.
///
/// The one place in this crate where a path stops being an argument and becomes syntax, which is why
/// the quoting lives here beside the call: see [`shell_quote`] and the `External effects` section of
/// the style guide.
///
/// # Errors
///
/// Returns whatever [`run`] returned. Note what a success does *not* mean: herdr types the text and
/// presses Enter without checking that the shell has reached its prompt, so text sent too early is
/// lost outright. The caller confirms the outcome with [`foreground_cwd`] rather than assuming this
/// landed.
pub fn open_at(pane: &PaneId, directory: &str) -> Result<(), HerdrError> {
    // herdr answers `{"type":"ok"}`. That the text was sent is the whole result.
    run::<IgnoredAny>(&run_args(pane, directory))?;
    Ok(())
}

/// The root of the repository herdr resolves for `cwd`, answered from any directory inside it.
///
/// Read before a worktree is cut, so `spawn` can work out where the caller sits inside its
/// repository and reopen the new checkout's pane at the same place. One read-only call, and no `git`
/// process — which is what keeps the rule that nothing outside this module spawns one.
///
/// # Errors
///
/// Returns whatever [`run`] returned. `not_git_worktree` is the refusal worth expecting: `cwd` is
/// not inside a repository at all, which `worktree create` would refuse a moment later anyway — and
/// meeting it here is better, because nothing has been created yet.
pub fn repo_root(cwd: &str) -> Result<String, HerdrError> {
    let listed: WorktreeListed = run(&list_args(cwd))?;
    Ok(listed.source.repo_root)
}

/// Closes a pane, taking whatever was running in it.
///
/// The target is a `&str` rather than a [`PaneId`], unlike the three above: theirs come from the
/// environment or from a herdr response, while this one can be a string a caller typed. Whether it
/// names a pane at all is herdr's to answer, and passing it through unexamined is how this crate
/// stays out of guessing at an id format.
///
/// # Errors
///
/// Returns whatever [`run`] returned.
pub fn close(pane: &str) -> Result<(), HerdrError> {
    // herdr answers `{"type":"ok"}`. That the call happened is the whole result.
    run::<IgnoredAny>(&close_args(pane))?;
    Ok(())
}

// =====================================================================================================================
// Responses
// =====================================================================================================================

/// `pane split`'s result: the pane it made.
#[derive(Debug, Deserialize)]
struct PaneCreated {
    pane: PaneRef,
}

/// `tab create` and `workspace create`'s result. Both also report the tab, and the workspace form
/// reports the workspace; neither is read here, and serde ignores what it is not asked for.
#[derive(Debug, Deserialize)]
struct RootPaneCreated {
    root_pane: PaneRef,
}

/// The one field either response is read for.
#[derive(Debug, Deserialize)]
struct PaneRef {
    pane_id: PaneId,
}

/// `worktree create`'s result: the root pane, and the checkout the new workspace sits on.
#[derive(Debug, Deserialize)]
struct WorktreeCreated {
    root_pane: PaneRef,
    worktree: Checkout,
}

/// Where a worktree spawn landed, read from herdr's response and reported onward unchanged.
///
/// The one public type in this section, because it is the one a caller is given rather than a
/// field this module reads and discards. herdr's worktree object also carries `is_bare`,
/// `is_detached`, `is_prunable`, `is_linked_worktree`, `open_workspace_id`, and `label`; serde
/// drops what it is not asked for, and all six are constants on something created a millisecond
/// ago.
///
/// `branch` is optional because herdr's field is, not because a create is expected to produce a
/// detached checkout.
#[derive(Debug, Deserialize, Serialize)]
pub struct Checkout {
    /// The branch the worktree checked out — herdr's, whether generated or the one asked for.
    pub branch: Option<String>,
    /// The absolute path herdr put the checkout at.
    pub path: String,
}

/// `worktree list`'s result, read only for the repository its `--cwd` resolved to.
///
/// herdr also reports every worktree of that repository; serde drops what it is not asked for, and
/// this call is made for one string.
#[derive(Debug, Deserialize)]
struct WorktreeListed {
    source: WorktreeSource,
}

/// The one field a `worktree list` is made for here.
///
/// herdr's source object also carries `repo_key`, `repo_name`, `source_checkout_path` and
/// `source_workspace_id`. `source_checkout_path` is the one worth naming as *not* read: it differs
/// from `repo_root` only when the caller is inside a linked worktree, which is a source herdr
/// refuses to cut from.
#[derive(Debug, Deserialize)]
struct WorktreeSource {
    repo_root: String,
}

/// `pane get`'s result, read only for the workspace the pane sits in.
///
/// A separate type from [`PaneCreated`] rather than an optional field on [`PaneRef`]: a creation
/// response does not report a workspace at all, and one struct spanning both would make the field
/// optional in the one place it is required.
#[derive(Debug, Deserialize)]
struct PaneWorkspace {
    pane: WorkspaceRef,
}

/// The one field a `pane get` is made for here.
#[derive(Debug, Deserialize)]
struct WorkspaceRef {
    workspace_id: String,
}

// =====================================================================================================================
// Helpers
// =====================================================================================================================

/// `herdr pane split <PANE> --direction right --cwd <CWD> --(no-)focus`.
///
/// The anchor is passed as `$HERDR_PANE_ID`, never as herdr's `--current`: that flag resolves
/// server-side to whichever pane is *focused*, which is not this one when the command runs from an
/// unfocused pane.
fn split_args(pane: &str, cwd: &str, focus: Focus) -> Vec<String> {
    [
        "pane",
        "split",
        pane,
        "--direction",
        "right",
        "--cwd",
        cwd,
        focus.flag(),
    ]
    .map(str::to_owned)
    .to_vec()
}

/// `herdr tab create [--workspace <ID>] --cwd <CWD> --label <LABEL> --(no-)focus`.
///
/// `--workspace` is passed whenever the caller's own is known, for the same reason `split` names
/// `$HERDR_PANE_ID` rather than herdr's `--current`: omitting it makes herdr resolve the workspace
/// server-side to whichever one is *UI-focused*, which is not the caller's whenever the human has
/// clicked elsewhere. A tab meant for this project then opens in whatever the user was last
/// looking at. `None` is for a caller with no pane of its own, where there is nothing to pin and
/// herdr's default is all there is — see [`workspace_of`] for where the id comes from otherwise.
fn tab_args(workspace: Option<&str>, label: &str, cwd: &str, focus: Focus) -> Vec<String> {
    let mut args = vec!["tab".to_owned(), "create".to_owned()];
    if let Some(workspace) = workspace {
        args.push("--workspace".to_owned());
        args.push(workspace.to_owned());
    }
    args.extend(["--cwd", cwd, "--label", label, focus.flag()].map(str::to_owned));
    args
}

/// `herdr workspace create --cwd <CWD> --label <LABEL> --(no-)focus`.
fn workspace_args(label: &str, cwd: &str, focus: Focus) -> Vec<String> {
    ["workspace", "create", "--cwd", cwd, "--label", label, focus.flag()]
        .map(str::to_owned)
        .to_vec()
}

/// `herdr worktree create --cwd <SOURCE> [--branch <NAME>] [--base <REF>] --label <LABEL>
/// --(no-)focus`.
///
/// `--cwd` is passed on every call for the same reason `tab create` always names `--workspace`:
/// omitting it makes herdr resolve the source to whichever workspace is *UI-focused*, so the
/// worktree would be cut from whatever repository the human was last looking at. Here `--cwd` names
/// the source checkout rather than the new surface's working directory, which herdr derives.
///
/// `--branch` and `--base` are omitted entirely when the caller did not give them, so herdr applies
/// its own defaults rather than ours.
fn worktree_args(label: &str, source: &str, branch: Option<&str>, base: Option<&str>, focus: Focus) -> Vec<String> {
    let mut args = ["worktree", "create", "--cwd", source].map(str::to_owned).to_vec();
    if let Some(branch) = branch {
        args.push("--branch".to_owned());
        args.push(branch.to_owned());
    }
    if let Some(base) = base {
        args.push("--base".to_owned());
        args.push(base.to_owned());
    }
    args.extend(["--label", label, focus.flag()].map(str::to_owned));
    args
}

/// `herdr pane get <PANE>`.
fn get_args(pane: &str) -> Vec<String> {
    ["pane", "get", pane].map(str::to_owned).to_vec()
}

/// `herdr pane run <PANE> "cd -- '<DIRECTORY>'"`.
///
/// The command line is one argv element because herdr joins everything after the pane id with a
/// space before typing it — three elements here would arrive as the same one string, and building it
/// as one is what makes the vector say what herdr will see.
fn run_args(pane: &str, directory: &str) -> Vec<String> {
    vec![
        "pane".to_owned(),
        "run".to_owned(),
        pane.to_owned(),
        format!("cd -- {}", shell_quote(directory)),
    ]
}

/// `herdr worktree list --cwd <CWD>`.
///
/// `--cwd` is a directory rather than a repository root: herdr resolves the repository containing
/// it, which is the whole reason this answers the question `spawn` has.
fn list_args(cwd: &str) -> Vec<String> {
    ["worktree", "list", "--cwd", cwd].map(str::to_owned).to_vec()
}

/// A path as one shell word: single-quoted, with an embedded `'` closed, escaped, and reopened.
///
/// Single quotes are the strong form — inside them a shell expands nothing, so `$HOME`, a backtick
/// and a `;` are literal characters and a path holding a command separator is inert rather than
/// executed. The one thing a single-quoted string cannot hold is a single quote, which is why one is
/// spelled `'\''`: close, escape, reopen.
///
/// Quoting alone does not cover a path that opens with a dash, which a shell would read as a flag.
/// [`run_args`] covers that by giving `cd` a `--` separator.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// `herdr pane close <PANE>`.
fn close_args(pane: &str) -> Vec<String> {
    ["pane", "close", pane].map(str::to_owned).to_vec()
}

// =====================================================================================================================
// Tests
// =====================================================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_split_names_the_anchor_pane_and_always_passes_a_cwd() {
        // The pane id is positional, which herdr accepts as the first argument that does not start
        // with `--`. `--cwd` is passed in all three modes: a split would otherwise inherit the
        // *pane's* working directory, which stops matching the shell's the moment you `cd`.
        assert_eq!(
            split_args("w4:p1", "/work/repo", Focus::Leave),
            [
                "pane",
                "split",
                "w4:p1",
                "--direction",
                "right",
                "--cwd",
                "/work/repo",
                "--no-focus"
            ]
        );
    }

    /// A tab is pinned to the caller's workspace, never left to herdr's focus-dependent default.
    ///
    /// Regression: without `--workspace`, herdr resolves the workspace server-side to whichever one
    /// the UI has focused. A rehearsal launch opened its tab in an unrelated project's workspace
    /// because the human had clicked there — the same trap `split` avoids by naming
    /// `$HERDR_PANE_ID` instead of `--current`. The id itself comes from [`workspace_of`], not from
    /// the environment, so it is also right for a pane that has since been moved.
    #[test]
    fn a_tab_names_the_callers_own_workspace_when_it_has_one() {
        assert_eq!(
            tab_args(Some("w7"), "reviewer", "/work/repo", Focus::Leave),
            [
                "tab",
                "create",
                "--workspace",
                "w7",
                "--cwd",
                "/work/repo",
                "--label",
                "reviewer",
                "--no-focus"
            ]
        );
    }

    #[test]
    fn a_tab_and_a_workspace_are_labelled_with_the_agents_name() {
        assert_eq!(
            tab_args(None, "reviewer", "/work/repo", Focus::Leave),
            [
                "tab",
                "create",
                "--cwd",
                "/work/repo",
                "--label",
                "reviewer",
                "--no-focus"
            ]
        );
        assert_eq!(
            workspace_args("reviewer", "/work/repo", Focus::Leave),
            [
                "workspace",
                "create",
                "--cwd",
                "/work/repo",
                "--label",
                "reviewer",
                "--no-focus"
            ]
        );
    }

    #[test]
    fn focus_is_always_stated_rather_than_left_to_herdrs_default() {
        // Opt-in: a tool meant to be driven by agents should not steal the human's focus. Stated
        // explicitly in both directions so the argument vector says what it means.
        assert_eq!(split_args("w4:p1", "/w", Focus::Take).last().unwrap(), "--focus");
        assert_eq!(tab_args(None, "reviewer", "/w", Focus::Take).last().unwrap(), "--focus");
        assert_eq!(workspace_args("reviewer", "/w", Focus::Take).last().unwrap(), "--focus");
        assert_eq!(
            worktree_args("reviewer", "/w", None, None, Focus::Take).last().unwrap(),
            "--focus"
        );
    }

    /// A worktree names its source checkout, never leaving herdr to pick one.
    ///
    /// The same trap `tab create` is pinned against, one call along: given neither `--cwd` nor
    /// `--workspace`, herdr resolves the source to the *focused* workspace, so the worktree would be
    /// cut from whichever repository the human had last clicked on. `--cwd` here is the checkout to
    /// branch from, not the new surface's working directory — herdr derives that from the checkout
    /// it creates.
    #[test]
    fn a_worktree_names_the_source_checkout_and_the_branch_and_base_when_it_has_them() {
        assert_eq!(
            worktree_args(
                "reviewer",
                "/work/repo",
                Some("worktree/fix-flake"),
                Some("origin/main"),
                Focus::Leave
            ),
            [
                "worktree",
                "create",
                "--cwd",
                "/work/repo",
                "--branch",
                "worktree/fix-flake",
                "--base",
                "origin/main",
                "--label",
                "reviewer",
                "--no-focus"
            ]
        );
    }

    #[test]
    fn a_worktree_omits_branch_and_base_rather_than_restating_herdrs_defaults() {
        // herdr generates a `worktree/`-prefixed branch name and bases on HEAD. Spelling either
        // here would be a second authority that drifts the day herdr changes its mind.
        assert_eq!(
            worktree_args("reviewer", "/work/repo", None, None, Focus::Leave),
            [
                "worktree",
                "create",
                "--cwd",
                "/work/repo",
                "--label",
                "reviewer",
                "--no-focus"
            ]
        );
    }

    #[test]
    fn a_split_reads_its_pane_id_from_a_different_field_than_a_tab_or_a_workspace() {
        // The reason `PaneId` exists: `.result.pane.pane_id` from a split, `.result.root_pane.pane_id`
        // from a tab or a workspace. Both feed `agent start --pane`, and mixing them up is a
        // mistake only this code can make.
        let split: PaneCreated =
            serde_json::from_str(r#"{"type":"pane_info","pane":{"pane_id":"w4:p18","tab_id":"w4:t1"}}"#).unwrap();
        assert_eq!(split.pane.pane_id, PaneId::from("w4:p18"));

        let tab: RootPaneCreated =
            serde_json::from_str(r#"{"type":"tab_created","tab":{"tab_id":"w4:t3"},"root_pane":{"pane_id":"w4:p17"}}"#)
                .unwrap();
        assert_eq!(tab.root_pane.pane_id, PaneId::from("w4:p17"));
    }

    /// A worktree response is read for two things, and everything else herdr says about it is
    /// dropped.
    ///
    /// `root_pane` is the same field a tab and a workspace are read for — `worktree create` makes a
    /// workspace, so it answers in that shape. The `worktree` object is the one place the branch is
    /// cheaply knowable, because herdr generated it; the six flags beside it are constants on a
    /// checkout this old and none of them is worth reporting.
    #[test]
    fn a_worktree_is_read_for_its_root_pane_and_its_checkout_and_nothing_else() {
        let created: WorktreeCreated = serde_json::from_str(
            r#"{"type":"worktree_created","workspace":{"workspace_id":"w9"},"tab":{"tab_id":"w9:t1"},
                "root_pane":{"pane_id":"w9:p1"},
                "worktree":{"path":"/work/trees/repo/worktree-lucky-harbor-8e01",
                            "branch":"worktree/lucky-harbor-8e01","is_bare":false,
                            "is_detached":false,"is_prunable":false,"is_linked_worktree":true,
                            "label":"repo"}}"#,
        )
        .unwrap();

        assert_eq!(created.root_pane.pane_id, PaneId::from("w9:p1"));
        assert_eq!(created.worktree.branch.as_deref(), Some("worktree/lucky-harbor-8e01"));
        assert_eq!(created.worktree.path, "/work/trees/repo/worktree-lucky-harbor-8e01");
    }

    #[test]
    fn a_checkout_is_reported_as_the_two_fields_it_was_read_for() {
        let checkout = Checkout {
            branch: Some("worktree/lucky-harbor-8e01".to_owned()),
            path: "/work/trees/repo/worktree-lucky-harbor-8e01".to_owned(),
        };

        assert_eq!(
            serde_json::to_string(&checkout).unwrap(),
            r#"{"branch":"worktree/lucky-harbor-8e01","path":"/work/trees/repo/worktree-lucky-harbor-8e01"}"#
        );
    }

    #[test]
    fn a_workspace_lookup_asks_about_one_pane() {
        assert_eq!(get_args("w4:p17"), ["pane", "get", "w4:p17"]);
    }

    /// The workspace comes from herdr, never from the pane's environment.
    ///
    /// Regression, and the reason this call exists at all: `HERDR_WORKSPACE_ID` is injected when a
    /// pane is created and cannot be rewritten in a running shell afterwards. A pane moved to another
    /// workspace keeps the old id, so a tab spawned from it would open in the workspace it *used* to
    /// be in — verified live, where `pane get` reported the new workspace after a move while the
    /// variable could not have.
    #[test]
    fn a_pane_get_is_read_only_for_the_workspace_and_ignores_the_rest() {
        let info: PaneWorkspace = serde_json::from_str(
            r#"{"pane":{"agent":"claude","agent_status":"working","cwd":"/work","focused":false,
                "pane_id":"wJ:p1","revision":20,"tab_id":"wJ:t1","workspace_id":"wJ"}}"#,
        )
        .unwrap();

        assert_eq!(info.pane.workspace_id, "wJ");
    }

    #[test]
    fn a_close_names_the_pane_and_nothing_else() {
        // No flags exist on herdr's side: `pane close` takes a pane id and acts. Every guard in front
        // of it is this crate's.
        assert_eq!(close_args("w4:p17"), ["pane", "close", "w4:p17"]);
    }

    #[test]
    fn a_repository_lookup_asks_about_one_directory() {
        // Any directory inside the repository answers, which is the property that makes this callable
        // from wherever the caller happens to be standing.
        assert_eq!(
            list_args("/work/repo/src/api"),
            ["worktree", "list", "--cwd", "/work/repo/src/api"]
        );
    }

    /// A listing is read for one string, and everything else herdr says about the repository is dropped.
    ///
    /// `source_checkout_path` sits beside `repo_root` and is deliberately not the field read. The two
    /// differ only when the caller is already inside a linked worktree, and herdr refuses to cut a
    /// worktree from there — see [`create_worktree`].
    #[test]
    fn a_worktree_listing_is_read_for_the_repository_root_and_nothing_else() {
        let listed: WorktreeListed = serde_json::from_str(
            r#"{"type":"worktree_list",
            "source":{"repo_key":"repo","repo_name":"repo","repo_root":"/work/repo",
                      "source_checkout_path":"/work/repo","source_workspace_id":"w4"},
            "worktrees":[{"path":"/work/repo","branch":"main","is_bare":false,"is_detached":false,
                          "is_prunable":false,"is_linked_worktree":false,"label":"repo"}]}"#,
        )
        .unwrap();

        assert_eq!(listed.source.repo_root, "/work/repo");
    }

    #[test]
    fn a_directory_change_is_typed_into_the_pane_as_one_quoted_shell_word() {
        // The command line is one argv element: herdr joins everything after the pane id with a
        // space before typing it, so splitting it here would only be rejoined.
        assert_eq!(
            run_args("w9:p1", "/work/trees/repo-8e01/src"),
            ["pane", "run", "w9:p1", "cd -- '/work/trees/repo-8e01/src'"]
        );
    }

    /// The quoting is a wire form: it is what another program parses, so it is pinned by exact string.
    ///
    /// `--cwd` is caller-supplied and the callers are agents, so the hostile cases are the point. Inside
    /// single quotes a shell expands nothing, which makes `$HOME`, a backtick and a `;` literal
    /// characters; the one thing a single-quoted string cannot hold is a single quote, which is why one
    /// is spelled `'\''`. `cd --` is what keeps a leading dash a path rather than a flag.
    #[test]
    fn a_path_that_could_be_read_as_shell_syntax_is_inert_inside_its_quotes() {
        assert_eq!(shell_quote("/work/repo/src"), "'/work/repo/src'");
        assert_eq!(shell_quote("/work/don't/stop"), r"'/work/don'\''t/stop'");
        assert_eq!(shell_quote("/work/a; rm -rf ~"), "'/work/a; rm -rf ~'");
        assert_eq!(shell_quote("/work/$HOME/`id`"), "'/work/$HOME/`id`'");
        assert_eq!(shell_quote("--rf"), "'--rf'");

        // The separator is what covers the leading dash, since the quotes alone do not.
        assert_eq!(
            run_args("w9:p1", "-rf").last().unwrap(),
            "cd -- '-rf'",
            "a path opening with a dash is a path"
        );
    }

    #[test]
    fn a_close_result_carries_nothing_worth_reading() {
        // herdr answers `{"type":"ok"}`, which is why `close` deserializes into `IgnoredAny` rather
        // than minting a response struct with no fields anyone reads.
        assert!(serde_json::from_str::<IgnoredAny>(r#"{"type":"ok"}"#).is_ok());
    }

    #[test]
    fn a_placement_serializes_as_the_flag_that_chose_it() {
        assert_eq!(serde_json::to_string(&Placement::Pane).unwrap(), r#""pane""#);
        assert_eq!(serde_json::to_string(&Placement::Tab).unwrap(), r#""tab""#);
        assert_eq!(serde_json::to_string(&Placement::Workspace).unwrap(), r#""workspace""#);
        assert_eq!(serde_json::to_string(&Placement::Worktree).unwrap(), r#""worktree""#);
    }
}
